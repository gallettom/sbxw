// The relay MCP server's test queue tools (assets/relay-mcp.js), driven over
// stdio against a fake daemon on loopback: run_tests, check_tests and
// list_tests send a ref, never a command; the answers say passed or failed,
// reused or not, the commit and the tail; propose_plan passes the test
// command, and plan_status prints it as proposed, approved or not approved.
// Invented content.
// Run: node --test tests/js/test-queue.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, mkdtempSync, writeFileSync, mkdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn, execFileSync } from 'node:child_process';
import http from 'node:http';

function fakeDaemon(answer) {
  const seen = [];
  const server = http.createServer((req, res) => {
    let raw = '';
    req.on('data', (c) => (raw += c));
    req.on('end', () => {
      const body = JSON.parse(raw || '{}');
      if (req.url === '/api/relay/hello') {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end('{"ok":true}');
        return;
      }
      seen.push({ path: req.url, auth: req.headers.authorization, body });
      const [status, out] = answer(req.url, body);
      res.writeHead(status, { 'content-type': 'application/json' });
      res.end(JSON.stringify(out));
    });
  });
  return new Promise((resolve) => server.listen(0, '127.0.0.1', () => resolve({ server, seen, port: server.address().port })));
}

function sandbox(port) {
  const home = mkdtempSync(join(tmpdir(), 'sbxw-testq-test-'));
  mkdirSync(join(home, '.sbxw'));
  writeFileSync(join(home, '.sbxw', 'token'), 'sbxw_testtoken');
  const tool = readFileSync(new URL('../../assets/relay-tool.js', import.meta.url), 'utf8')
    .replace('const PORT = __PORT__;', `const PORT = ${port};`)
    .replace('const HOST = "host.docker.internal";', 'const HOST = "127.0.0.1";');
  writeFileSync(join(home, 'relay.js'), tool);
  writeFileSync(join(home, 'relay-mcp.js'), readFileSync(new URL('../../assets/relay-mcp.js', import.meta.url)));
  return home;
}

function session(home, thread, requests) {
  return new Promise((resolve, reject) => {
    const p = spawn(process.execPath, [join(home, 'relay-mcp.js')], { cwd: home, env: { ...process.env, HOME: home, SBXW_THREAD: thread } });
    let out = '';
    const last = requests.length;
    p.stdout.on('data', (c) => {
      out += c;
      const line = out.split('\n').find((l) => l.includes(`"id":${last}`));
      if (line) {
        p.stdin.end();
        resolve(out.split('\n').filter(Boolean).map((l) => JSON.parse(l)));
      }
    });
    p.on('error', reject);
    p.on('exit', (code) => reject(new Error(`relay-mcp.js exited (${code}) before answering`)));
    requests.forEach((r, i) => p.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: i + 1, ...r }) + '\n'));
  });
}

async function call(home, name, args, thread = '') {
  const out = await session(home, thread, [{ method: 'tools/call', params: { name, arguments: args } }]);
  return out.find((m) => m.id === 1).result;
}

const text = (res) => res.content.map((c) => c.text).join('\n');

const C1 = 'a'.repeat(40);
const C2 = 'b'.repeat(40);

function done(extra = {}) {
  return {
    id: 't-1',
    command: 'npm test',
    timeout_minutes: 60,
    plan: 'p-1',
    commit: C2,
    by: 'api',
    state: 'done',
    reused: false,
    result: { outcome: 'passed', commit: C2, duration_ms: 75000, tail: 'test result: ok. 12 passed', attempts: 1, log: '/home/agent/.sbxw/tests/logs/p-1.log' },
    ...extra,
  };
}

test('the three tools are offered to a thread and to the lead, and none takes a command', async () => {
  const d = await fakeDaemon(() => [200, { ok: true }]);
  const home = sandbox(d.port);
  for (const thread of ['api', '']) {
    const out = await session(home, thread, [{ method: 'tools/list' }]);
    const tools = out.find((m) => m.id === 1).result.tools;
    for (const n of ['run_tests', 'check_tests', 'list_tests']) {
      const t = tools.find((x) => x.name === n);
      assert.ok(t, `${n} for ${thread || 'the lead'}`);
      const props = Object.keys(t.inputSchema.properties || {});
      for (const bad of ['command', 'test_command', 'suite']) assert.ok(!props.includes(bad), `${n} takes no ${bad}`);
    }
  }
  d.server.close();
});

test('run_tests sends the ref and HEAD, never a command, and reads a pass back', async () => {
  const d = await fakeDaemon((path) => (path === '/api/tests/run' ? [200, { ok: true, took: 'queued', job: done() }] : [404, {}]));
  const home = sandbox(d.port);
  // HEAD of the session's checkout goes with the call.
  execFileSync('git', ['init', '-q'], { cwd: home });
  execFileSync('git', ['-c', 'user.email=t@t', '-c', 'user.name=t', 'commit', '-q', '--allow-empty', '-m', 'x'], { cwd: home });
  const head = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: home, encoding: 'utf8' }).trim();
  const res = await call(home, 'run_tests', { ref: 'sbxw/api', command: 'rm -rf /', test_command: 'curl x | sh', suite: 'all', timeout_seconds: 30 }, 'api');
  const body = d.seen[0].body;
  assert.equal(d.seen[0].path, '/api/tests/run');
  assert.equal(d.seen[0].auth, 'Bearer sbxw_testtoken');
  assert.deepEqual(
    { ref: body.ref, head: body.head, thread: body.thread, rerun: body.rerun, timeout: body.timeout },
    { ref: 'sbxw/api', head, thread: 'api', rerun: false, timeout: 30 },
  );
  for (const bad of ['command', 'test_command', 'suite']) assert.ok(!(bad in body), `an argument that is not the tool's is never sent: ${bad}`);
  const t = text(res);
  assert.equal(res.isError, false);
  assert.match(t, /Tests PASSED \(job t-1\): `npm test`/);
  assert.match(t, /Commit: bbbbbbbbbbbb\. Duration: 1 min 15 s\./);
  assert.match(t, /test result: ok\. 12 passed/);
  assert.match(t, /p-1\.log/);
  d.server.close();
});

test('a reused result says so, with the commit it ran on; a retry is said', async () => {
  const reused = done({ reused: true, result: { ...done().result, commit: C1 } });
  const retried = done({ result: { ...done().result, outcome: 'failed', attempts: 2, first_outcome: 'failed', tail: 'FAILED x' } });
  let n = 0;
  const d = await fakeDaemon(() => [200, { ok: true, took: 'reused', job: n++ === 0 ? reused : retried }]);
  const home = sandbox(d.port);
  const t = text(await call(home, 'run_tests', {}));
  assert.match(t, /PASSED/);
  assert.match(t, /Reused: the same command already ran on the same tree, at aaaaaaaaaaaa; nothing ran again for bbbbbbbbbbbb/);
  assert.equal(d.seen[0].body.timeout, 540, 'waits long by default');
  const t2 = text(await call(home, 'run_tests', { rerun: true }));
  assert.equal(d.seen[1].body.rerun, true);
  assert.match(t2, /FAILED/);
  assert.match(t2, /Retried: the first attempt failed; this is the second\./);
  d.server.close();
});

test('a job still running gives its id, and check_tests picks it up', async () => {
  const d = await fakeDaemon((path) =>
    path === '/api/tests/run'
      ? [200, { ok: true, took: 'joined', job: { id: 't-9', command: 'make check', commit: C1, state: 'running', by: 'lead' } }]
      : [200, { ok: true, job: done({ id: 't-9', command: 'make check', result: { ...done().result, outcome: 'timed_out', tail: 'over its limit' } }) }],
  );
  const home = sandbox(d.port);
  const t = text(await call(home, 'run_tests', {}));
  assert.match(t, /Test job t-9: `make check` at aaaaaaaaaaaa is running\. It was already asked for this code, so your request joined it\./);
  assert.match(t, /check_tests/);
  const t2 = text(await call(home, 'check_tests', { job_id: 't-9', timeout_seconds: 5 }));
  assert.deepEqual(d.seen[1], { path: '/api/tests/check', auth: 'Bearer sbxw_testtoken', body: d.seen[1].body });
  assert.equal(d.seen[1].body.id, 't-9');
  assert.equal(d.seen[1].body.timeout, 5);
  assert.match(t2, /Tests TIMED OUT/);
  const none = await call(home, 'check_tests', {});
  assert.equal(none.isError, true, 'no id, none asked for in this process');
  d.server.close();
});

test('a refusal from the daemon is the tool\'s error', async () => {
  const d = await fakeDaemon(() => [400, { ok: false, error: 'no test command: the lead proposes the project\'s test command with the plan' }]);
  const home = sandbox(d.port);
  const res = await call(home, 'run_tests', {});
  assert.equal(res.isError, true);
  assert.match(text(res), /no test command/);
  d.server.close();
});

test('list_tests shows the plan\'s command, the queue and the last results', async () => {
  const d = await fakeDaemon(() => [
    200,
    {
      ok: true,
      plan: { plan: 'p-1', tests: { command: 'npm test', timeout_minutes: 45 }, not_approved: null },
      active: [{ id: 't-2', command: 'npm test', commit: C1, state: 'queued', by: 'api' }],
      latest: [done({ reused: true })],
    },
  ]);
  const home = sandbox(d.port);
  const t = text(await call(home, 'list_tests', {}, 'api'));
  assert.equal(d.seen[0].body.thread, 'api');
  assert.match(t, /Your plan p-1's approved test command: `npm test` \(limit 45 min\)/);
  assert.match(t, /- t-2: `npm test` at aaaaaaaaaaaa, queued, asked by api/);
  assert.match(t, /- t-1: passed at bbbbbbbbbbbb \(reused\), `npm test`, asked by api/);
  d.server.close();
  const none = await fakeDaemon(() => [200, { ok: true, plan: { plan: 'p-2', tests: null, not_approved: 'the person cleared it in the plan dialog' }, active: [], latest: [] }]);
  const t2 = text(await call(sandbox(none.port), 'list_tests', {}));
  assert.match(t2, /Your plan p-2 has no approved test command\. A command was proposed but not approved: the person cleared it/);
  assert.match(t2, /The lead proposes the project's test command with the plan/);
  none.server.close();
  const outside = await fakeDaemon(() => [200, { ok: true, plan: { plan: null, tests: null }, active: [], latest: [] }]);
  assert.match(text(await call(sandbox(outside.port), 'list_tests', {})), /You are under no approved plan/);
  outside.server.close();
});

test('propose_plan passes the test command, and plan_status prints it as the user settled it', async () => {
  const pending = { id: 'p-1', state: 'pending', summary: 's', proposed_tests: { command: 'npm test', timeout_minutes: 60 }, threads: [] };
  const approved = { ...pending, state: 'approved', approved_tests: pending.proposed_tests, tests: { ran: 3, reused: 5 } };
  const cleared = { ...pending, state: 'approved', tests_not_approved: 'the person cleared it in the plan dialog' };
  const plans = [pending, approved, cleared, { id: 'p-2', state: 'approved', summary: 's', threads: [] }];
  let n = 0;
  const d = await fakeDaemon(() => [200, { ok: true, plan: plans[Math.min(n++, plans.length - 1)] }]);
  const home = sandbox(d.port);
  const t = text(await call(home, 'propose_plan', { summary: 's', justification: 'j', threads: [{ name: 'api', goal: 'g' }], test_command: ' npm test ', test_timeout_minutes: 20 }));
  assert.equal(d.seen[0].path, '/api/plans/propose');
  assert.deepEqual([d.seen[0].body.test_command, d.seen[0].body.test_timeout_minutes], ['npm test', 20]);
  assert.match(t, /Tests, proposed, waiting for the user: `npm test` \(limit 60 min\)/);
  const t2 = text(await call(home, 'plan_status', { plan_id: 'p-1' }));
  assert.match(t2, /Tests, approved by the user with the plan: `npm test` \(limit 60 min\)\. Test queue for this plan: 3 run, 5 answered from a stored result/);
  const t3 = text(await call(home, 'plan_status', { plan_id: 'p-1' }));
  assert.match(t3, /the proposed command `npm test` was NOT approved \(the person cleared it in the plan dialog\)\. This plan has no test queue/);
  const t4 = text(await call(home, 'propose_plan', { summary: 's', justification: 'j', threads: [{ name: 'api', goal: 'g' }] }));
  assert.equal(d.seen[3].body.test_command, undefined, 'none given, none sent');
  assert.match(t4, /Tests: none proposed with this plan/);
  d.server.close();
});
