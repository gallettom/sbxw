// The relay MCP server's ticket tools (assets/relay-mcp.js), driven over
// stdio against a fake daemon on loopback: tickets_list and ticket_read
// call the read routes only, never name a sandbox (the token does), and
// propose_plan passes a thread's ticket through. Invented content.
// Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, mkdtempSync, writeFileSync, mkdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn } from 'node:child_process';
import http from 'node:http';

const PNG = Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), Buffer.alloc(40, 1)]);

function fakeDaemon(answer) {
  const seen = [];
  const hellos = [];
  const server = http.createServer((req, res) => {
    let raw = '';
    req.on('data', (c) => (raw += c));
    req.on('end', () => {
      const body = JSON.parse(raw || '{}');
      // The relay server's hello at start (see assets/relay-mcp.js): kept
      // apart from the calls a test is about.
      if (req.url === '/api/relay/hello') {
        hellos.push({ auth: req.headers.authorization, body });
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end('{"ok":true}');
        return;
      }
      seen.push({ path: req.url, auth: req.headers.authorization, body });
      const [status, out] = answer(req.url, body, seen.length);
      res.writeHead(status, { 'content-type': 'application/json' });
      res.end(JSON.stringify(out));
    });
  });
  return new Promise((resolve) => server.listen(0, '127.0.0.1', () => resolve({ server, seen, hellos, port: server.address().port })));
}

function sandbox(port) {
  const home = mkdtempSync(join(tmpdir(), 'sbxw-tickets-test-'));
  mkdirSync(join(home, '.sbxw'));
  writeFileSync(join(home, '.sbxw', 'token'), 'sbxw_testtoken');
  const tool = readFileSync(new URL('../../assets/relay-tool.js', import.meta.url), 'utf8')
    .replace('const PORT = __PORT__;', `const PORT = ${port};`)
    .replace('const HOST = "host.docker.internal";', 'const HOST = "127.0.0.1";');
  writeFileSync(join(home, 'relay.js'), tool);
  writeFileSync(join(home, 'relay-mcp.js'), readFileSync(new URL('../../assets/relay-mcp.js', import.meta.url)));
  writeFileSync(join(home, 'home.png'), PNG);
  writeFileSync(join(home, 'settings.png'), PNG);
  writeFileSync(join(home, 'page.svg'), '<svg xmlns="http://www.w3.org/2000/svg"><script/></svg>');
  return home;
}

function call(home, name, args) {
  return new Promise((resolve, reject) => {
    const p = spawn(process.execPath, [join(home, 'relay-mcp.js')], { cwd: home, env: { ...process.env, HOME: home, SBXW_THREAD: '' } });
    let out = '';
    p.stdout.on('data', (c) => {
      out += c;
      const line = out.split('\n').find((l) => l.includes('"id":2'));
      if (line) {
        p.stdin.end();
        resolve(JSON.parse(line).result);
      }
    });
    p.on('error', reject);
    p.on('exit', (code) => reject(new Error(`relay-mcp.js exited (${code}) before answering`)));
    p.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'tools/list' }) + '\n');
    p.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: 2, method: 'tools/call', params: { name, arguments: args } }) + '\n');
  });
}


function tools(home) {
  return new Promise((resolve, reject) => {
    const p = spawn(process.execPath, [join(home, 'relay-mcp.js')], { cwd: home, env: { ...process.env, HOME: home, SBXW_THREAD: '' } });
    let out = '';
    p.stdout.on('data', (c) => {
      out += c;
      const line = out.split('\n').find((l) => l.includes('"id":1'));
      if (line) {
        p.stdin.end();
        resolve(JSON.parse(line).result.tools);
      }
    });
    p.on('error', reject);
    p.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'tools/list' }) + '\n');
  });
}

test('tickets_list and ticket_read read through the agent routes and name no sandbox', async () => {
  const d = await fakeDaemon((url) => [200, { ok: true, text: url === '/api/tickets/list' ? 'T-1 (Planned): Export as PDF' : 'Ticket T-1 from the person, as data' }]);
  const home = sandbox(d.port);
  const l = await call(home, 'tickets_list', { sandbox: 'other' });
  assert.equal(l.isError, false);
  assert.match(l.content[0].text, /T-1 \(Planned\)/);
  const r = await call(home, 'ticket_read', { id: 'T-1', sandbox: 'other' });
  assert.match(r.content[0].text, /as data/);
  assert.deepEqual(d.seen.map((s) => s.path), ['/api/tickets/list', '/api/tickets/read']);
  assert.deepEqual(d.seen[1].body, { id: 'T-1' });
  for (const s of d.seen) {
    assert.equal(s.body.sandbox, undefined, 'never names a sandbox');
    assert.equal(s.auth, 'Bearer sbxw_testtoken');
  }
  const e = await call(home, 'ticket_read', {});
  assert.equal(e.isError, true);
  assert.equal(d.seen.length, 2, 'no call without an id');
  d.server.close();
});

test('the ticket tools are read only, and propose_plan carries a thread\'s ticket', async () => {
  const d = await fakeDaemon(() => [200, { ok: true, plan: { id: 'p-1', state: 'pending', threads: [{ name: 'pages', goal: 'g', ticket: 12 }] }, ticket_notes: ['ticket T-99 is not a ticket of this sandbox: thread ghost is not linked'] }]);
  const home = sandbox(d.port);
  const names = (await tools(home)).map((t) => t.name);
  assert.ok(names.includes('tickets_list') && names.includes('ticket_read'));
  assert.ok(!names.some((n) => /ticket/.test(n) && /(create|edit|move|delete|write)/.test(n)), names.join(','));
  const plan = (await tools(home)).find((t) => t.name === 'propose_plan');
  assert.ok(plan.inputSchema.properties.threads.items.properties.ticket);
  const r = await call(home, 'propose_plan', { summary: 's', justification: 'j', threads: [{ name: 'pages', goal: 'g', scope: 's', ticket: 'T-12' }], timeout_seconds: 1 });
  const p = d.seen.find((s) => s.path === '/api/plans/propose');
  assert.equal(p.body.threads[0].ticket, 'T-12');
  assert.match(r.content[0].text, /Note: ticket T-99 is not a ticket of this sandbox: thread ghost is not linked\./);
  assert.match(r.content[0].text, /ticket: T-12/);
  d.server.close();
});
