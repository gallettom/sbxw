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

// ── Two threads, one worktree ─────────────────────────────────────────────

const written = (r) => r.sent.find((x) => x.route === '/api/board/after').body.written;
const commitOf = (r) => r.sent.find((x) => x.route === '/api/board/after').body.commit;

test('a long command is not credited with the other thread\'s edits', withSandbox((s) => {
  const cmd = 'npm test';
  assert.equal(s.pre('api', cmd, 'a1').code, 0);
  // meanwhile the other thread works in the same worktree
  s.write('theirs.txt');
  s.write('seed.txt', 'edited by them\n');
  const r = s.post('api', cmd, 'a1');
  assert.deepEqual(written(r), [], 'nothing here can be tied to the caller');
  assert.equal(r.stdout, '', 'so no warning against the caller');
}));

test('what the command names, and the caller\'s own claims, are still reported', withSandbox((s) => {
  const cmd = 'echo built > out.txt && ./gen.sh';
  assert.equal(s.pre('api', cmd, 'a2').code, 0);
  s.write('out.txt'); // named as a write target
  s.write('mine.txt'); // not named, but the caller's claim
  s.write('theirs.txt'); // the other thread's
  const answers = { '/api/board/read': { claims: [{ file: 'mine.txt', owner: 'api' }, { file: 'theirs.txt', owner: 'front' }] } };
  const r = s.post('api', cmd, 'a2', answers);
  assert.deepEqual(written(r).sort(), ['mine.txt', 'out.txt']);
}));

test('a directory the command names covers the files under it', withSandbox((s) => {
  fs.mkdirSync(path.join(s.repo, 'gen'));
  assert.equal(s.pre('api', 'touch gen', 'a3').code, 0);
  s.write('gen/one.txt');
  s.write('other/two.txt'.replace('other/', 'o-'));
  const r = s.post('api', 'touch gen', 'a3');
  assert.deepEqual(written(r), ['gen/one.txt']);
}));

test('a HEAD the other thread moved is not this thread\'s commit', withSandbox((s) => {
  const cmd = 'npm test';
  assert.equal(s.pre('api', cmd, 'a4').code, 0);
  s.write('theirs.txt');
  s.git('add', 'theirs.txt');
  s.git('commit', '-q', '-m', 'their commit', '--', 'theirs.txt');
  const r = s.post('api', cmd, 'a4');
  assert.equal(commitOf(r), undefined);
  assert.deepEqual(written(r), []);
}));

test('the caller\'s own commit is reported, with the files it named', withSandbox((s) => {
  s.write('mine.txt');
  s.write('theirs.txt');
  const cmd = 'git add mine.txt && git commit -q -m "mine; ok" -- mine.txt';
  assert.equal(s.pre('api', cmd, 'a5').code, 0);
  execFileSync('sh', ['-c', cmd], { cwd: s.repo });
  const r = s.post('api', cmd, 'a5');
  const c = commitOf(r);
  assert.equal(c.subject, 'mine; ok');
  assert.deepEqual(c.files, ['mine.txt']);
  assert.deepEqual(written(r), ['mine.txt'], 'theirs.txt, still untracked, is not mine');
  assert.ok(!fs.existsSync(path.join(s.repo, '.git/sbxw-commit.lock')), 'the lock is dropped');
}));

test('two threads commit one after the other, each credited with its own', withSandbox((s) => {
  s.write('a.txt');
  s.write('b.txt');
  const ca = 'git add a.txt && git commit -q -m "from api" -- a.txt';
  const cb = 'git add b.txt && git commit -q -m "from front" -- b.txt';
  assert.equal(s.pre('api', ca, 'x1').code, 0);
  execFileSync('sh', ['-c', ca], { cwd: s.repo });
  const ra = s.post('api', ca, 'x1');
  assert.equal(s.pre('front', cb, 'x2').code, 0);
  execFileSync('sh', ['-c', cb], { cwd: s.repo });
  const rb = s.post('front', cb, 'x2');
  assert.deepEqual([commitOf(ra).subject, commitOf(ra).files], ['from api', ['a.txt']]);
  assert.deepEqual([commitOf(rb).subject, commitOf(rb).files], ['from front', ['b.txt']]);
  assert.equal(ra.sent[0].body.thread, 'api');
  assert.equal(rb.sent[0].body.thread, 'front');
}));
