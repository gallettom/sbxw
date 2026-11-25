// The relay MCP server's request tools (assets/relay-mcp.js), driven over
// stdio against a fake daemon on loopback: a request is opened (its id at
// once, with a client key), then waited on, and acknowledged only once the
// tool result is written; a retried open keeps its key; a failed wait still
// names the request; a long answer says how to read its next part; a
// cancelled call answers nothing and acknowledges nothing. Invented content.
// Run: node --test tests/js/relay-ask.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, mkdtempSync, writeFileSync, mkdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn } from 'node:child_process';
import http from 'node:http';

/// `answer(url, body, n)` returns [status, json], or 'drop' to cut the
/// connection, or 'hang' to never answer.
function fakeDaemon(answer) {
  const seen = [];
  const closed = [];
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
      const n = seen.filter((s) => s.path === req.url).length;
      const out = answer(req.url, body, n);
      if (out === 'drop') {
        req.socket.destroy();
        return;
      }
      if (out === 'hang') {
        res.on('close', () => closed.push(req.url));
        return;
      }
      const [status, json] = out;
      res.writeHead(status, { 'content-type': 'application/json' });
      res.end(JSON.stringify(json));
    });
  });
  return new Promise((resolve) => server.listen(0, '127.0.0.1', () => resolve({ server, seen, closed, port: server.address().port })));
}

function sandbox(port) {
  const home = mkdtempSync(join(tmpdir(), 'sbxw-ask-test-'));
  mkdirSync(join(home, '.sbxw'));
  writeFileSync(join(home, '.sbxw', 'token'), 'sbxw_testtoken');
  const tool = readFileSync(new URL('../../assets/relay-tool.js', import.meta.url), 'utf8')
    .replace('const PORT = __PORT__;', `const PORT = ${port};`)
    .replace('const HOST = "host.docker.internal";', 'const HOST = "127.0.0.1";');
  writeFileSync(join(home, 'relay.js'), tool);
  writeFileSync(join(home, 'relay-mcp.js'), readFileSync(new URL('../../assets/relay-mcp.js', import.meta.url)));
  return home;
}

/// A running server: `call` sends a tools/call and resolves with its result.
function server(home, thread = '') {
  const p = spawn(process.execPath, [join(home, 'relay-mcp.js')], { cwd: home, env: { ...process.env, HOME: home, SBXW_THREAD: thread } });
  const lines = [];
  const waiting = new Map();
  let out = '';
  let next = 10;
  p.stdout.on('data', (c) => {
    out += c;
    let nl;
    while ((nl = out.indexOf('\n')) >= 0) {
      const line = out.slice(0, nl);
      out = out.slice(nl + 1);
      if (!line.trim()) continue;
      const msg = JSON.parse(line);
      lines.push(msg);
      const w = waiting.get(msg.id);
      if (w) {
        waiting.delete(msg.id);
        w(msg.result);
      }
    }
  });
  const write = (m) => p.stdin.write(JSON.stringify(m) + '\n');
  return {
    lines,
    write,
    start(name, args) {
      const id = next++;
      const done = new Promise((resolve) => waiting.set(id, resolve));
      write({ jsonrpc: '2.0', id, method: 'tools/call', params: { name, arguments: args } });
      return { id, done };
    },
    call(name, args) {
      return this.start(name, args).done;
    },
    end() {
      p.stdin.end();
      p.kill();
    },
  };
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const paths = (d) => d.seen.map((s) => s.path);

/// Waits until the fake has seen `n` calls to `path`, or fails.
async function until(d, path, n = 1) {
  for (let i = 0; i < 100; i++) {
    if (d.seen.filter((s) => s.path === path).length >= n) return;
    await sleep(20);
  }
  assert.fail(`never saw ${n} call(s) to ${path}: ${paths(d).join(', ')}`);
}

test('ask_other_sandbox opens with a client key, waits, and acknowledges once the answer is written', async () => {
  const d = await fakeDaemon((url) => {
    if (url === '/api/relay/open') return [200, { id: 'r-7', state: 'pending' }];
    if (url === '/api/relay/wait') return [200, { id: 'r-7', kind: 'question', state: 'approved', to: 'docs', answer: 'The guide is in docs/style.md', answer_part: 1, answer_parts: 1, images: [] }];
    return [200, { ok: true, delivered: true }];
  });
  const s = server(sandbox(d.port));
  const r = await s.call('ask_other_sandbox', { question: 'Where is the style guide?' });
  assert.equal(r.isError, false);
  assert.match(r.content.at(-1).text, /docs\/style\.md/);
  await until(d, '/api/relay/ack');
  assert.deepEqual(paths(d), ['/api/relay/open', '/api/relay/wait', '/api/relay/ack']);
  const [open, wait, ack] = d.seen;
  assert.equal(open.body.kind, 'question');
  assert.equal(open.body.thread, '');
  assert.match(open.body.client_key, /\S{8,}/);
  assert.equal(open.body.from, undefined, 'never names a sandbox');
  assert.deepEqual([wait.body.id, wait.body.ack, wait.body.thread], ['r-7', true, '']);
  assert.deepEqual(ack.body, { id: 'r-7', thread: '' });
  assert.equal(open.auth, 'Bearer sbxw_testtoken');
  s.end();
  d.server.close();
});

test('a retried open keeps its key, so the daemon sees one request', async () => {
  const d = await fakeDaemon((url, _b, n) => {
    if (url === '/api/relay/open') return n === 1 ? 'drop' : [200, { id: 'r-8', state: 'pending' }];
    if (url === '/api/relay/wait') return [200, { id: 'r-8', kind: 'question', state: 'pending', images: [] }];
    return [200, { ok: true }];
  });
  const s = server(sandbox(d.port));
  const r = await s.call('ask_other_sandbox', { question: 'q?', timeout_seconds: 1 });
  const opens = d.seen.filter((x) => x.path === '/api/relay/open');
  assert.equal(opens.length, 2);
  assert.equal(opens[0].body.client_key, opens[1].body.client_key);
  assert.match(r.content.at(-1).text, /r-8/);
  assert.match(r.content.at(-1).text, /check_sandbox_question/);
  await sleep(100);
  assert.ok(!paths(d).includes('/api/relay/ack'), 'nothing final: nothing to acknowledge');
  s.end();
  d.server.close();
});

test('a wait that fails still names the request and says how to pick it up', async () => {
  const d = await fakeDaemon((url) => {
    if (url === '/api/relay/open') return [200, { id: 'r-9', state: 'pending' }];
    if (url === '/api/relay/wait') return 'drop';
    return [200, { ok: true }];
  });
  const s = server(sandbox(d.port));
  const r = await s.call('ask_user_for_screenshot', { reason: 'how does the header look?' });
  assert.equal(r.isError, true);
  const text = r.content.at(-1).text;
  assert.match(text, /Request r-9 is open/);
  assert.match(text, /check_sandbox_question` with request_id "r-9"/);
  assert.match(text, /Do not ask again/);
  assert.equal(d.seen[0].body.kind, 'screenshot');
  s.end();
  d.server.close();
});

test('ask_user_questions opens a round for the person, in its thread', async () => {
  const d = await fakeDaemon((url) => {
    if (url === '/api/relay/open') return [200, { id: 'r-10', state: 'pending' }];
    if (url === '/api/relay/wait') return [200, { id: 'r-10', kind: 'ask', state: 'approved', answer: 'Q1 : yes\nQ2 : 8081', answer_part: 1, answer_parts: 1, images: [] }];
    return [200, { ok: true }];
  });
  const s = server(sandbox(d.port), 'api');
  const r = await s.call('ask_user_questions', {
    questions: [
      { title: 'Port', question: 'Which port?', recommendation: '8080' },
      { title: 'Name', question: 'Keep the name?' },
    ],
  });
  assert.match(r.content.at(-1).text, /Q2 : 8081/);
  await until(d, '/api/relay/ack');
  const open = d.seen[0].body;
  assert.equal(open.kind, 'ask');
  assert.equal(open.thread, 'api');
  assert.match(open.question, /❓ \*\*Q1 - Port\*\* : Which port\?/);
  assert.equal(d.seen[1].body.timeout, 600, 'a round waits 10 minutes by default');
  assert.deepEqual(d.seen.at(-1).body, { id: 'r-10', thread: 'api' });
  s.end();
  d.server.close();
});

test('check_sandbox_question reads a part of a long answer and says how to read the next', async () => {
  const d = await fakeDaemon((url) => {
    if (url === '/api/relay/wait') return [200, { id: 'r-11', kind: 'ask', state: 'approved', answer: 'Q5 : the middle part', answer_part: 2, answer_parts: 3, images: [] }];
    return [200, { ok: true }];
  });
  const s = server(sandbox(d.port));
  const r = await s.call('check_sandbox_question', { request_id: 'r-11', part: 2 });
  const text = r.content.at(-1).text;
  assert.match(text, /part 2 of 3/);
  assert.match(text, /Q5 : the middle part/);
  assert.match(text, /request_id "r-11" and part 3/);
  assert.equal(d.seen[0].body.part, 2);
  const unknown = await fakeDaemon(() => [400, { ok: false, error: "no request 'r-99' — it may have expired, or never existed" }]);
  const s2 = server(sandbox(unknown.port));
  const e = await s2.call('check_sandbox_question', { request_id: 'r-99' });
  assert.equal(e.isError, true);
  assert.match(e.content[0].text, /no request 'r-99'/, 'a refusal is said as it is, not as a failed wait');
  s.end();
  s2.end();
  d.server.close();
  unknown.server.close();
});

test('a cancelled call aborts its wait, answers nothing and acknowledges nothing', async () => {
  const d = await fakeDaemon((url) => {
    if (url === '/api/relay/open') return [200, { id: 'r-12', state: 'pending' }];
    if (url === '/api/relay/wait') return 'hang';
    return [200, { ok: true }];
  });
  const s = server(sandbox(d.port));
  const { id } = s.start('ask_other_sandbox', { question: 'q?' });
  await until(d, '/api/relay/wait');
  s.write({ jsonrpc: '2.0', method: 'notifications/cancelled', params: { requestId: id, reason: 'user pressed Esc' } });
  for (let i = 0; i < 50 && !d.closed.length; i++) await sleep(20);
  assert.deepEqual(d.closed, ['/api/relay/wait'], 'the wait on the daemon is abandoned');
  await sleep(200);
  assert.ok(!s.lines.some((m) => m.id === id), 'no result for a cancelled call');
  assert.ok(!paths(d).includes('/api/relay/ack'), 'not acknowledged: sbxw hands the outcome on');
  // The server is still serving.
  const tools = await new Promise((resolve) => {
    const t = setInterval(() => {
      const m = s.lines.find((l) => l.id === 99);
      if (m) {
        clearInterval(t);
        resolve(m.result.tools);
      }
    }, 20);
    s.write({ jsonrpc: '2.0', id: 99, method: 'tools/list' });
  });
  assert.ok(tools.some((t) => t.name === 'check_sandbox_question'));
  s.end();
  d.server.close();
});
