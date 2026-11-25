// The status hook as the courier of the test gate (audit D10): on a
// PreToolUse the daemon answers a deny for a thread's run of the plan's
// whole test command, and the hook prints it as Claude Code reads it. A
// daemon that does not answer in time never blocks: the hook exits 0 and
// prints nothing. The command reaches the daemon whole (not clipped).
// Run: node --test tests/js/status-hook-gate.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, mkdtempSync, mkdirSync } from 'node:fs';
import { spawn } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import http from 'node:http';

const src = readFileSync(new URL('../../assets/status-hook.js', import.meta.url), 'utf8');

function daemon(answer) {
  const seen = [];
  const server = http.createServer((req, res) => {
    let raw = '';
    req.on('data', (c) => (raw += c));
    req.on('end', () => {
      if (req.url !== '/api/hook') {
        res.writeHead(200, { 'content-type': 'application/json' });
        return res.end('{"ok":true,"next":0}');
      }
      seen.push(JSON.parse(raw));
      answer(res);
    });
  });
  return new Promise((resolve) => server.listen(0, '127.0.0.1', () => resolve({ server, seen, port: server.address().port })));
}

function run(port, event, env = {}) {
  const dir = mkdtempSync(join(tmpdir(), 'sbxw-hook-gate-'));
  mkdirSync(join(dir, '.sbxw'));
  writeFileSync(join(dir, '.sbxw', 'token'), 'sbxw_testtoken');
  const file = join(dir, 'status-hook.js');
  writeFileSync(file, src.replaceAll('__PORT__', String(port)).replaceAll('"host.docker.internal"', '"127.0.0.1"'));
  return new Promise((resolve) => {
    const started = Date.now();
    const p = spawn(process.execPath, [file], { env: { ...process.env, HOME: dir, SANDBOX_NAME: 'test-sandbox', ...env } });
    let out = '';
    let err = '';
    p.stdout.on('data', (c) => (out += c));
    p.stderr.on('data', (c) => (err += c));
    p.on('close', (status) => resolve({ status, out, err, ms: Date.now() - started }));
    p.stdin.end(JSON.stringify(event));
  });
}

const deny = {
  hookSpecificOutput: { hookEventName: 'PreToolUse', permissionDecision: 'deny', permissionDecisionReason: 'commit, then call run_tests' },
};
const long = `npm test -- ${'x'.repeat(1200)}`;
const bash = { hook_event_name: 'PreToolUse', session_id: 's-1', transcript_path: '/nonexistent/s-1.jsonl', tool_name: 'Bash', tool_input: { command: long } };

test('a deny the daemon answers is printed, and the command reached it whole', async () => {
  const d = await daemon((res) => {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify(deny));
  });
  const r = await run(d.port, bash, { SBXW_THREAD: 'api' });
  d.server.close();
  assert.equal(r.status, 0, r.err);
  assert.deepEqual(JSON.parse(r.out), deny);
  assert.equal(d.seen[0].tool_input.command, long, 'not clipped');
  assert.equal(d.seen[0].sbxw_thread, 'api');
});

test('a daemon slower than the hook limit never blocks: exit 0, nothing printed', async () => {
  const held = [];
  const d = await daemon((res) => held.push(res));
  const r = await run(d.port, bash, { SBXW_THREAD: 'api' });
  for (const res of held) res.destroy();
  d.server.close();
  assert.equal(r.status, 0, r.err);
  assert.equal(r.out, '', 'no decision printed: the command runs');
  assert.ok(r.ms < 4000, `ended in ${r.ms} ms`);
});

test('no daemon at all: exit 0, nothing printed', async () => {
  const r = await run(9, bash, { SBXW_THREAD: 'api' });
  assert.equal(r.status, 0, r.err);
  assert.equal(r.out, '');
});
