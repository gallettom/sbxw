// assets/thread-start.js, the SessionStart hook that tells a thread its
// worktree's state: run against a temporary repository with a thread
// worktree, inside a thread and outside one.
// Run: node --test tests/js/*.test.mjs
import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const HOOK = fileURLToPath(new URL('../../assets/thread-start.js', import.meta.url));
let dir, repo, wt;

const env = {
  ...process.env,
  GIT_AUTHOR_NAME: 't', GIT_AUTHOR_EMAIL: 't@example.invalid',
  GIT_COMMITTER_NAME: 't', GIT_COMMITTER_EMAIL: 't@example.invalid',
  GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_SYSTEM: '/dev/null',
};
delete env.SBXW_THREAD;
const git = (cwd, ...args) => execFileSync('git', args, { cwd, env, encoding: 'utf8' });
const run = (cwd, thread) => {
  const e = { ...env };
  if (thread !== undefined) e.SBXW_THREAD = thread;
  const r = spawnSync('node', [HOOK], { cwd, env: e, encoding: 'utf8', input: '{"hook_event_name":"SessionStart"}' });
  return r;
};
const context = (r) => JSON.parse(r.stdout).hookSpecificOutput.additionalContext;

before(() => {
  dir = mkdtempSync(join(tmpdir(), 'thread-start-'));
  repo = join(dir, 'repo');
  mkdirSync(repo);
  git(repo, 'init', '-q', '-b', 'main');
  writeFileSync(join(repo, 'a.txt'), 'a\n');
  git(repo, 'add', '-A');
  git(repo, 'commit', '-qm', 'base');
  wt = join(repo, '.sbxw-worktrees', 'api');
  git(repo, 'worktree', 'add', '-q', '-b', 'sbxw/api', wt);
  git(repo, 'commit', '-q', '--allow-empty', '-m', 'main moves on');
  writeFileSync(join(wt, 'a.txt'), 'changed\n');
  for (let i = 0; i < 40; i++) writeFileSync(join(wt, `new-${String(i).padStart(2, '0')}.txt`), 'x\n');
  mkdirSync(join(wt, 'target-lt'));
  git(wt, 'worktree', 'add', '-q', '--detach', join(wt, 'scratch'));
});

after(() => rmSync(dir, { recursive: true, force: true }));

test('outside a thread it prints nothing', () => {
  const r = run(wt);
  assert.equal(r.status, 0);
  assert.equal(r.stdout, '');
  assert.equal(run(wt, '').stdout, '');
});

test('in a thread it gives the branch, the tip, the status clipped, leftovers and main', () => {
  const r = run(wt, 'api');
  assert.equal(r.status, 0);
  const c = context(r);
  assert.match(c, /^\[sbxw\] Thread "api": branch sbxw\/api, tip [0-9a-f]+ base\./);
  assert.match(c, /Uncommitted \(\d+\):/);
  assert.match(c, / M a\.txt/);
  assert.match(c, /\.\.\. and \d+ more/);
  const statusLines = c.split('\n').filter(l => /^ {2}(\?\?| M)/.test(l));
  assert.equal(statusLines.length, 30, 'clipped to 30 lines');
  assert.match(c, /Scratch worktrees under this one: scratch\./);
  assert.match(c, /Leftover target dirs: target-lt/);
  assert.match(c, /main has 1 commit\(s\) this branch lacks: check `git merge-tree --write-tree main HEAD`/);
  assert.match(c, /Commit before long builds/);
});

test('silent on any error: outside a repository, nothing and exit 0', () => {
  const r = run(dir, 'api');
  assert.equal(r.status, 0);
  assert.equal(r.stdout, '');
});

test('fast: well under its two seconds', () => {
  const t = Date.now();
  run(wt, 'api');
  assert.ok(Date.now() - t < 2000);
});
