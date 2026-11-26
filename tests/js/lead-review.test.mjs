// scripts/lead-review.sh against a temporary repository: a main branch, three
// thread branches (one clean, one conflicting with main and with a sibling,
// one already merged), a worktree with leftovers.
// Run: node --test tests/js/*.test.mjs
import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const SCRIPT = fileURLToPath(new URL('../../scripts/lead-review.sh', import.meta.url));
let dir, repo;

const env = {
  ...process.env,
  GIT_AUTHOR_NAME: 't', GIT_AUTHOR_EMAIL: 't@example.invalid',
  GIT_COMMITTER_NAME: 't', GIT_COMMITTER_EMAIL: 't@example.invalid',
  GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_SYSTEM: '/dev/null',
};
const git = (cwd, ...args) => execFileSync('git', args, { cwd, env, encoding: 'utf8' });
const put = (p, s) => { mkdirSync(join(p, '..'), { recursive: true }); writeFileSync(p, s); };
const commit = (cwd, msg) => { git(cwd, 'add', '-A'); git(cwd, 'commit', '-qm', msg); };
const run = (...args) => spawnSync('bash', [SCRIPT, ...args], { cwd: repo, env, encoding: 'utf8' });

before(() => {
  dir = mkdtempSync(join(tmpdir(), 'lead-review-'));
  repo = join(dir, 'repo');
  mkdirSync(repo);
  git(repo, 'init', '-q', '-b', 'main');
  put(join(repo, 'src/a.rs'), 'fn a() {}\n');
  put(join(repo, 'src/shared.rs'), 'one\n');
  commit(repo, 'base');

  // alpha: in scope src/a.rs, plus a doc outside it and a log line; a worktree.
  const wt = join(repo, '.sbxw-worktrees', 'alpha');
  git(repo, 'worktree', 'add', '-q', '-b', 'sbxw/alpha', wt);
  put(join(wt, 'src/a.rs'), 'fn a() {\n    tracing::info!("hello");\n}\n');
  put(join(wt, 'docs/x.md'), 'doc\n');
  commit(wt, 'alpha work');
  put(join(wt, '.sbxw-artifacts/report.md'), 'r\n');
  mkdirSync(join(wt, 'target-x'));
  put(join(wt, 'dirty.txt'), 'x\n');
  git(wt, 'worktree', 'add', '-q', '--detach', join(wt, 'scratch'));

  // beta and gamma both change src/shared.rs: they conflict with each other.
  git(repo, 'branch', 'sbxw/beta');
  git(repo, 'checkout', '-q', 'sbxw/beta');
  put(join(repo, 'src/shared.rs'), 'beta\n');
  commit(repo, 'beta work');
  git(repo, 'checkout', '-q', '-b', 'sbxw/gamma', 'main');
  put(join(repo, 'src/shared.rs'), 'gamma\n');
  commit(repo, 'gamma work');
  // done: merged into main already, so not live.
  git(repo, 'checkout', '-q', '-b', 'sbxw/done', 'main');
  put(join(repo, 'src/done.rs'), 'd\n');
  commit(repo, 'done work');
  git(repo, 'checkout', '-q', 'main');
  git(repo, 'merge', '-q', '--no-ff', '-m', 'merge done', 'sbxw/done');
  // A plan's CI branch, built by sbxw from beta: never listed as a thread's.
  git(repo, 'checkout', '-q', '-b', 'sbxw/ci-p-1', 'main');
  git(repo, 'merge', '-q', '--no-ff', '-m', 'ci', 'sbxw/beta');
  git(repo, 'checkout', '-q', 'main');
});

after(() => rmSync(dir, { recursive: true, force: true }));

test('a missing branch exits 2; anything else exits 0', () => {
  assert.equal(run('nope').status, 2);
  assert.equal(run().status, 2);
  assert.equal(run('alpha').status, 0);
});

test('a thread review prints commits, stat, merge state, scope, worktree, leftovers and log lines', () => {
  const out = run('alpha', '--scope', 'src/').stdout;
  assert.match(out, /== Commits on top of main ==\n[0-9a-f]+ alpha work\n/);
  assert.match(out, /docs\/x\.md +\| 1 \+/);
  assert.match(out, /^main: CLEAN$/m);
  assert.match(out, /^sbxw\/beta: CLEAN$/m);
  assert.ok(!out.includes('sbxw/done:'), 'a merged branch is not live');
  assert.match(out, /== Files outside the scope ==\ndocs\/x\.md\n/);
  assert.match(out, /DIRTY \(\d+ entries\)/);
  assert.match(out, /\.sbxw-artifacts\/report\.md/);
  assert.match(out, /target-x\/ \(target dir\)/);
  assert.match(out, /scratch \(registered worktree\)/);
  assert.match(out, /src\/a\.rs:2: +tracing::info!\("hello"\);/);
});

test('conflicts are named, against main and against a sibling', () => {
  const out = run('beta').stdout;
  assert.match(out, /^main: CLEAN$/m);
  assert.match(out, /^sbxw\/gamma: CONFLICT: src\/shared\.rs$/m);
  assert.doesNotMatch(out, /sbxw\/ci-p-1/, 'a CI branch is not a sibling');
  assert.match(out, /\(no --scope given\)/);
  assert.match(out, /\(no worktree registered on sbxw\/beta\)/);
  assert.match(out, /== Added lines that log ==\n\(none\)/);
});

test('--after-merge lists the live branches and their state against main', () => {
  const r = run('--after-merge');
  assert.equal(r.status, 0);
  const lines = r.stdout.trim().split('\n').slice(1);
  assert.deepEqual(lines.map(l => l.split(':')[0]), ['sbxw/alpha', 'sbxw/beta', 'sbxw/gamma']);
  assert.match(r.stdout, /sbxw\/alpha: 1 ahead, 2 behind: CLEAN/);
});

test('the output is stable from one run to the next', () => {
  assert.equal(run('alpha', '--scope', 'src').stdout, run('alpha', '--scope', 'src').stdout);
});
