// The relay MCP server's test queue tools (assets/relay-mcp.js), driven over
// stdio against a fake daemon on loopback: run_tests, check_tests and
// list_tests send a suite's NAME and a ref, never a command; the answers say
// passed or failed, reused or not, the commit and the tail; propose_plan
// passes the team suite and plan_status prints it. Invented content.
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
    suite: 'crates',
    commit: C2,
    by: 'api',
    state: 'done',
    reused: false,
    result: { outcome: 'passed', commit: C2, duration_ms: 75000, tail: 'test result: ok. 12 passed', attempts: 1, log: '/home/agent/.sbxw/tests/logs/crates.log' },
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
      assert.ok(!('command' in (t.inputSchema.properties || {})), n);
    }
  }
  d.server.close();
});

test('run_tests sends the suite, the ref and HEAD, and reads a pass back', async () => {
  const d = await fakeDaemon((path) => (path === '/api/tests/run' ? [200, { ok: true, took: 'queued', job: done() }] : [404, {}]));
  const home = sandbox(d.port);
  // HEAD of the session's checkout goes with the call.
  execFileSync('git', ['init', '-q'], { cwd: home });
  execFileSync('git', ['-c', 'user.email=t@t', '-c', 'user.name=t', 'commit', '-q', '--allow-empty', '-m', 'x'], { cwd: home });
  const head = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: home, encoding: 'utf8' }).trim();
  const res = await call(home, 'run_tests', { suite: 'crates', ref: 'sbxw/api', command: 'rm -rf /', timeout_seconds: 30 }, 'api');
  const body = d.seen[0].body;
  assert.equal(d.seen[0].path, '/api/tests/run');
  assert.equal(d.seen[0].auth, 'Bearer sbxw_testtoken');
  assert.deepEqual(
    { suite: body.suite, ref: body.ref, head: body.head, thread: body.thread, rerun: body.rerun, timeout: body.timeout },
    { suite: 'crates', ref: 'sbxw/api', head, thread: 'api', rerun: false, timeout: 30 },
  );
  assert.ok(!('command' in body), 'an argument that is not the tool\'s is never sent');
  const t = text(res);
  assert.equal(res.isError, false);
  assert.match(t, /Suite crates PASSED \(job t-1\)/);
  assert.match(t, /Commit: bbbbbbbbbbbb\. Duration: 1 min 15 s\./);
  assert.match(t, /test result: ok\. 12 passed/);
  assert.match(t, /crates\.log/);
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
  assert.match(t, /Reused: the code this suite covers is the same as at aaaaaaaaaaaa, where it ran; nothing ran again for bbbbbbbbbbbb/);
  assert.equal(d.seen[0].body.suite, undefined, 'no suite: the team suite');
  assert.equal(d.seen[0].body.timeout, 540, 'waits long by default');
  const t2 = text(await call(home, 'run_tests', { suite: 'crates', rerun: true }));
  assert.equal(d.seen[1].body.rerun, true);
  assert.match(t2, /FAILED/);
  assert.match(t2, /Retried: the first attempt failed; this is the second\./);
  d.server.close();
});

test('a job still running gives its id, and check_tests picks it up', async () => {
  const d = await fakeDaemon((path) =>
    path === '/api/tests/run'
      ? [200, { ok: true, took: 'joined', job: { id: 't-9', suite: 'all', commit: C1, state: 'running', by: 'lead' } }]
      : [200, { ok: true, job: done({ id: 't-9', suite: 'all', result: { ...done().result, outcome: 'timed_out', tail: 'over its limit' } }) }],
  );
  const home = sandbox(d.port);
  const t = text(await call(home, 'run_tests', { suite: 'all' }));
  assert.match(t, /Test job t-9: suite all at aaaaaaaaaaaa is running\. It was already asked for this code, so your request joined it\./);
  assert.match(t, /check_tests/);
  const t2 = text(await call(home, 'check_tests', { job_id: 't-9', timeout_seconds: 5 }));
  assert.deepEqual(d.seen[1], { path: '/api/tests/check', auth: 'Bearer sbxw_testtoken', body: d.seen[1].body });
  assert.equal(d.seen[1].body.id, 't-9');
  assert.equal(d.seen[1].body.timeout, 5);
  assert.match(t2, /Suite all TIMED OUT/);
  const none = await call(home, 'check_tests', {});
  assert.equal(none.isError, true, 'no id, none asked for in this process');
  d.server.close();
});

test('a refusal from the daemon is the tool\'s error', async () => {
  const d = await fakeDaemon(() => [400, { ok: false, error: '"nope" is not a declared test suite (declared: crates, js)' }]);
  const home = sandbox(d.port);
  const res = await call(home, 'run_tests', { suite: 'nope' });
  assert.equal(res.isError, true);
  assert.match(text(res), /not a declared test suite/);
  d.server.close();
});

test('list_tests shows the suites, the queue and the latest results', async () => {
  const d = await fakeDaemon(() => [
    200,
    {
      ok: true,
      suites: [
        { name: 'crates', paths: ['crates', 'Cargo.lock'], depends: [], timeout_minutes: 30, retries: 1 },
        { name: 'all', paths: ['.'], depends: ['crates'], timeout_minutes: 90, retries: 0 },
      ],
      active: [{ id: 't-2', suite: 'all', commit: C1, state: 'queued', by: 'api' }],
      latest: [done({ reused: true })],
    },
  ]);
  const home = sandbox(d.port);
  const t = text(await call(home, 'list_tests', {}));
  assert.match(t, /- crates: covers crates, Cargo\.lock; limit 30 min; retried once on failure/);
  assert.match(t, /- all: covers \.; also covers crates; limit 90 min/);
  assert.match(t, /- t-2: all at aaaaaaaaaaaa, queued, asked by api/);
  assert.match(t, /- crates: passed at bbbbbbbbbbbb \(reused\)/);
  d.server.close();
  const empty = await fakeDaemon(() => [200, { ok: true, suites: [], active: [], latest: [] }]);
  const t2 = text(await call(sandbox(empty.port), 'list_tests', {}));
  assert.match(t2, /declares no test suites/);
  empty.server.close();
});

test('propose_plan passes the team suite, and plan_status prints it with the counts', async () => {
  const plan = { id: 'p-1', state: 'approved', summary: 's', team_suite: 'all', tests: { ran: 3, reused: 5 }, threads: [] };
  const d = await fakeDaemon(() => [200, { ok: true, plan }]);
  const home = sandbox(d.port);
  const t = text(await call(home, 'propose_plan', { summary: 's', justification: 'j', threads: [{ name: 'api', goal: 'g' }], team_suite: ' all ' }));
  assert.equal(d.seen[0].path, '/api/plans/propose');
  assert.equal(d.seen[0].body.team_suite, 'all');
  assert.match(t, /Team suite: all \(test queue for this plan: 3 run, 5 answered from a stored result\)/);
  const t2 = text(await call(home, 'plan_status', { plan_id: 'p-1' }));
  assert.match(t2, /Team suite: all/);
  await call(home, 'propose_plan', { summary: 's', justification: 'j', threads: [{ name: 'api', goal: 'g' }] });
  assert.equal(d.seen[2].body.team_suite, undefined, 'none named, none sent');
  d.server.close();
});
