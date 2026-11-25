// assets/team-guard.js: the Bash reading (quotes, here-documents) and what a
// Bash call is held to have written when several threads share one worktree.
// The hook runs as a process, as Claude Code runs it, beside a fake relay.js
// that records what it would send the daemon and answers from a file.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const GUARD = path.resolve(import.meta.dirname, '../../assets/team-guard.js');

const FAKE_RELAY = `
const fs = require('fs');
exports.me = 'sb';
exports.request = async (route, body) => {
  fs.appendFileSync(process.env.FAKE_LOG, JSON.stringify({ route, body }) + '\\n');
  const replies = JSON.parse(fs.readFileSync(process.env.FAKE_REPLIES, 'utf8'));
  return replies[route] || { ok: true };
};
`;

function sandbox() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'team-guard-'));
  const repo = path.join(dir, 'repo');
  fs.mkdirSync(repo);
  fs.copyFileSync(GUARD, path.join(dir, 'team-guard.js'));
  fs.writeFileSync(path.join(dir, 'relay.js'), FAKE_RELAY);
  const git = (...a) => execFileSync('git', a, { cwd: repo, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
  git('init', '-q');
  git('config', 'user.name', 't');
  git('config', 'user.email', 't@example.invalid');
  fs.writeFileSync(path.join(repo, 'seed.txt'), 'seed\n');
  git('add', 'seed.txt');
  git('commit', '-q', '-m', 'seed');
  const log = path.join(dir, 'log.jsonl');
  const replies = path.join(dir, 'replies.json');
  fs.writeFileSync(log, '');
  fs.writeFileSync(replies, '{}');
  const write = (name, text = 'x\n') => fs.writeFileSync(path.join(repo, name), text);
  /// One hook call by `thread`; the daemon's replies are `answers` by route.
  const hook = (event, thread, tool, input, id, answers = {}) => {
    fs.writeFileSync(replies, JSON.stringify(answers));
    fs.writeFileSync(log, '');
    const r = spawnSync('node', [path.join(dir, 'team-guard.js')], {
      cwd: repo,
      input: JSON.stringify({ hook_event_name: event, tool_name: tool, tool_use_id: id, cwd: repo, tool_input: input }),
      encoding: 'utf8',
      env: { PATH: process.env.PATH, HOME: dir, TMPDIR: dir, SBXW_TEAM: 'shared', SBXW_THREAD: thread, FAKE_LOG: log, FAKE_REPLIES: replies },
    });
    const sent = fs.readFileSync(log, 'utf8').split('\n').filter(Boolean).map((l) => JSON.parse(l));
    return { code: r.status, stderr: r.stderr, stdout: r.stdout, sent };
  };
  const pre = (thread, command, id = 'pre') => hook('PreToolUse', thread, 'Bash', { command }, id);
  const post = (thread, command, id = 'pre', answers) => hook('PostToolUse', thread, 'Bash', { command }, id, answers);
  const checked = (r) => r.sent.filter((s) => s.route === '/api/board/check').flatMap((s) => s.body.files);
  return { dir, repo, git, write, pre, post, checked, hook, cleanup: () => fs.rmSync(dir, { recursive: true, force: true }) };
}

const withSandbox = (fn) => () => {
  const s = sandbox();
  try {
    return fn(s);
  } finally {
    s.cleanup();
  }
};

// ── Quotes and here-documents ─────────────────────────────────────────────

test('a commit message with ; && | inside quotes does not cut the command', withSandbox((s) => {
  for (const cmd of [
    'git commit -m "a; b" -- f.txt',
    "git commit -m 'a && b | c' -- f.txt",
    'git commit -m "say \\"hi\\"; ok" -- f.txt',
    'git commit -m "first line\nsecond; line" -- f.txt',
    'git commit -m "$(cat <<\'EOF\'\nsubject; body | more\nEOF\n)" -- f.txt',
  ]) {
    const r = s.pre('api', cmd);
    assert.equal(r.code, 0, `${cmd}: ${r.stderr}`);
    assert.deepEqual(s.checked(r), ['f.txt'], `${cmd}: the named path reaches the board check`);
    fs.rmSync(path.join(s.repo, '.git/sbxw-commit.lock'), { force: true });
  }
}));

test('a commit message from a here-document does not cut the command', withSandbox((s) => {
  const cmd = "git commit -F - -- f.txt <<'EOF'\nsubject; body && more\nEOF";
  const r = s.pre('api', cmd);
  assert.equal(r.code, 0, r.stderr);
  assert.deepEqual(s.checked(r), ['f.txt']);
}));

test('every refusal still holds, quoted or not', withSandbox((s) => {
  for (const cmd of [
    'git stash',
    'git stash pop',
    'git add -A',
    'git add .',
    'git commit -a -m x',
    'git commit -am x',
    'git commit -m x',
    'git commit -m "a; b"',
    'echo hi; git stash',
    'true && git add --all',
    'git commit -m "a; b" -a -- f.txt',
    'cat x | git commit -m x',
  ]) {
    const r = s.pre('api', cmd);
    assert.equal(r.code, 2, cmd);
    assert.match(r.stderr, /git/, cmd);
  }
}));

test('git words inside a quoted string are not git commands', withSandbox((s) => {
  for (const cmd of ['echo "run git stash; then git add -A"', "echo 'git commit -m x'", 'grep "git stash" notes.md']) {
    assert.equal(s.pre('api', cmd).code, 0, cmd);
  }
}));

test('a > inside a quoted string is not a write target', withSandbox((s) => {
  assert.deepEqual(s.checked(s.pre('api', 'echo "a > b.txt"')), []);
  assert.deepEqual(s.checked(s.pre('api', "grep -c '>>' notes.md")), []);
  assert.deepEqual(s.checked(s.pre('api', 'node -e "if (a > b) console.log(1)"')), []);
  // the real redirection beside it still counts
  assert.deepEqual(s.checked(s.pre('api', 'echo "x > y" > out.txt')), ['out.txt']);
  assert.deepEqual(s.checked(s.pre('api', 'echo hi >> "log file.txt"')), ['log file.txt']);
  assert.deepEqual(s.checked(s.pre('api', 'echo hi 2> err.log')), ['err.log']);
  assert.deepEqual(s.checked(s.pre('api', 'echo hi 2>&1')), []);
}));

test('a here-document body is not read as a command or a redirection', withSandbox((s) => {
  const cmd = "cat > out.txt <<'EOF'\nx > y.txt; git stash\nEOF";
  const r = s.pre('api', cmd);
  assert.equal(r.code, 0, r.stderr);
  assert.deepEqual(s.checked(r), ['out.txt']);
  // and the command after the body is read again
  assert.equal(s.pre('api', "cat <<EOF\nbody\nEOF\ngit stash").code, 2);
}));

test('write targets that were already found keep being found', withSandbox((s) => {
  s.write('a.txt');
  assert.deepEqual(s.checked(s.pre('api', 'tee out.txt < in')), ['out.txt']);
  assert.deepEqual(s.checked(s.pre('api', 'rm a.txt')), ['a.txt']);
  assert.deepEqual(s.checked(s.pre('api', 'cp a.txt b.txt')), ['b.txt']);
  assert.deepEqual(s.checked(s.pre('api', 'sed -i s/x/y/ a.txt')), ['a.txt']);
}));
