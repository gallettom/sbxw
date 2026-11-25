// The status hook as the courier of the inbox (audit D15): the daemon
// answers an event with context and an `sbxw_inbox` part; the hook prints
// the answer without that part, writes a long message's file first, and once
// printed acknowledges the ids (`/api/hook/inbox_ack`). A daemon that does
// not answer in time never blocks: exit 0, nothing printed, nothing acked.
// Run: node --test tests/js/status-hook-inbox.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, mkdtempSync, mkdirSync, existsSync, statSync } from 'node:fs';
import { spawn } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import http from 'node:http';

const src = readFileSync(new URL('../../assets/status-hook.js', import.meta.url), 'utf8');

// A fake daemon: `/api/hook` is answered by `answer`, acks are recorded.
function daemon(answer) {
  const hooks = [];
  const acks = [];
  const server = http.createServer((req, res) => {
    let raw = '';
    req.on('data', (c) => (raw += c));
    req.on('end', () => {
      if (req.url === '/api/hook') {
        hooks.push(JSON.parse(raw));
        return answer(res);
      }
      if (req.url === '/api/hook/inbox_ack') acks.push({ body: JSON.parse(raw), auth: req.headers.authorization });
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end('{"ok":true,"next":0}');
    });
  });
  return new Promise((resolve) => server.listen(0, '127.0.0.1', () => resolve({ server, hooks, acks, port: server.address().port })));
}

function run(port, event, env = {}) {
  const dir = mkdtempSync(join(tmpdir(), 'sbxw-hook-inbox-'));
  mkdirSync(join(dir, '.sbxw'));
  writeFileSync(join(dir, '.sbxw', 'token'), 'sbxw_testtoken');
  const inboxDir = join(dir, 'inbox');
  const file = join(dir, 'status-hook.js');
  writeFileSync(
    file,
    src
      .replaceAll('__PORT__', String(port))
      .replaceAll('"host.docker.internal"', '"127.0.0.1"')
      .replace('const INBOX_DIR = "/tmp/sbxw-inbox";', `const INBOX_DIR = ${JSON.stringify(inboxDir)};`),
  );
  return new Promise((resolve) => {
    const started = Date.now();
    const p = spawn(process.execPath, [file], { env: { ...process.env, HOME: dir, SANDBOX_NAME: 'test-sandbox', ...env } });
    let out = '';
    let err = '';
    p.stdout.on('data', (c) => (out += c));
    p.stderr.on('data', (c) => (err += c));
    p.on('close', (status) => resolve({ status, out, err, ms: Date.now() - started, inboxDir }));
    p.stdin.end(JSON.stringify(event));
  });
}

const event = (name) => ({ hook_event_name: name, session_id: 's-1', transcript_path: '/nonexistent/s-1.jsonl', tool_name: 'Bash' });

function answerFor(name, files = []) {
  const inbox = { session_id: 's-1', ids: ['m-1', 'm-2'], files };
  if (name === 'Stop') return { decision: 'block', reason: '[sbxw inbox] two', sbxw_inbox: inbox };
  return { hookSpecificOutput: { hookEventName: name, additionalContext: '[sbxw inbox] two' }, sbxw_inbox: inbox };
}

for (const name of ['PostToolUse', 'UserPromptSubmit', 'SessionStart', 'Stop']) {
  test(`${name}: the inbox is printed without its ack part, then acknowledged`, async () => {
    const d = await daemon((res) => {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify(answerFor(name)));
    });
    const r = await run(d.port, event(name), { SBXW_THREAD: 'api' });
    d.server.close();
    assert.equal(r.status, 0, r.err);
    const printed = JSON.parse(r.out);
    assert.equal(printed.sbxw_inbox, undefined, 'the ack part is never printed');
    const want = answerFor(name);
    delete want.sbxw_inbox;
    assert.deepEqual(printed, want);
    assert.equal(d.hooks[0].hook_version, 5);
    assert.deepEqual(d.acks.map((a) => a.body), [{ session_id: 's-1', ids: ['m-1', 'm-2'] }]);
    assert.equal(d.acks[0].auth, 'Bearer sbxw_testtoken');
  });
}

test('a long message is written whole before the answer is printed and acked', async () => {
  const whole = 'x'.repeat(30000);
  const d = await daemon((res) => {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify(answerFor('PostToolUse', [{ name: 'm-1.txt', text: whole }])));
  });
  const r = await run(d.port, event('PostToolUse'));
  d.server.close();
  assert.equal(r.status, 0, r.err);
  const f = join(r.inboxDir, 's-1', 'm-1.txt');
  assert.equal(readFileSync(f, 'utf8'), whole);
  assert.equal(statSync(f).mode & 0o777, 0o600);
  assert.equal(d.acks.length, 1);
});

test('a file it cannot write: printed, never acknowledged, so it comes again', async () => {
  const d = await daemon((res) => {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify(answerFor('PostToolUse', [{ name: '../escape.txt', text: 'no' }])));
  });
  const r = await run(d.port, event('PostToolUse'));
  d.server.close();
  assert.equal(r.status, 0, r.err);
  assert.ok(r.out.includes('[sbxw inbox]'));
  assert.equal(d.acks.length, 0);
  assert.ok(!existsSync(join(r.inboxDir, 'escape.txt')));
});

test('an answer with nothing to print is not printed and the inbox part alone is ignored', async () => {
  const d = await daemon((res) => {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ sbxw_inbox: { session_id: 's-1', ids: ['m-1'], files: [] } }));
  });
  const r = await run(d.port, event('PostToolUse'));
  d.server.close();
  assert.equal(r.status, 0, r.err);
  assert.equal(r.out, '');
  assert.equal(d.acks.length, 0);
});

test('a daemon slower than the hook limit: exit 0 fast, nothing printed, nothing acked', async () => {
  const held = [];
  const d = await daemon((res) => held.push(res));
  const r = await run(d.port, event('PostToolUse'));
  for (const res of held) res.destroy();
  d.server.close();
  assert.equal(r.status, 0, r.err);
  assert.equal(r.out, '');
  assert.equal(d.acks.length, 0);
  assert.ok(r.ms < 4000, `ended in ${r.ms} ms`);
});
