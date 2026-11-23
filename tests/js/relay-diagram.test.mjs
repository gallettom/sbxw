// The relay MCP server's canvas_add_diagram tool and canvas_read's diagram
// read-back (assets/relay-mcp.js), over stdio against a fake daemon on
// loopback: the file's checks before anything is posted, the body posted
// (never a sandbox), and the whole diagram saved under generated names in
// /tmp/sbxw-canvas, 0600, from checked ids only. Every diagram is invented.
// Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, mkdtempSync, writeFileSync, mkdirSync, statSync, existsSync, rmSync, symlinkSync, readdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import http from 'node:http';

const FLOW = JSON.stringify({ version: 1, kind: 'workflow', title: 'Release', nodes: [{ id: 'push', label: 'Push' }, { id: 'test', label: 'Test' }], edges: [{ from: 'push', to: 'test' }] });

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
      const [status, out] = answer(req.url, body, seen.length);
      res.writeHead(status, { 'content-type': 'application/json' });
      res.end(JSON.stringify(out));
    });
  });
  return new Promise((resolve) => server.listen(0, '127.0.0.1', () => resolve({ server, seen, port: server.address().port })));
}

function sandbox(port) {
  const home = mkdtempSync(join(tmpdir(), 'sbxw-diagram-test-'));
  mkdirSync(join(home, '.sbxw'));
  writeFileSync(join(home, '.sbxw', 'token'), 'sbxw_testtoken');
  const tool = readFileSync(new URL('../../assets/relay-tool.js', import.meta.url), 'utf8')
    .replace('const PORT = __PORT__;', `const PORT = ${port};`)
    .replace('const HOST = "host.docker.internal";', 'const HOST = "127.0.0.1";');
  writeFileSync(join(home, 'relay.js'), tool);
  writeFileSync(join(home, 'relay-mcp.js'), readFileSync(new URL('../../assets/relay-mcp.js', import.meta.url)));
  writeFileSync(join(home, 'flow.json'), FLOW);
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

const added = { ok: true, canvas: 'c-0123456789abcdef', canvas_title: 'Review', diagram: 'd-0123456789ab', title: 'Release', kind: 'workflow', rev: 1, round: 2, nodes: 2, edges: 1, replaced: false };

test('canvas_add_diagram checks its arguments and the file before posting anything', async () => {
  const d = await fakeDaemon(() => [200, added]);
  const home = sandbox(d.port);
  mkdirSync(join(home, '.claude'));
  writeFileSync(join(home, '.sbxw', 'stolen.json'), '{"token":"x"}');
  writeFileSync(join(home, '.claude', 'creds.json'), '{"token":"x"}');
  writeFileSync(join(home, 'flow.txt'), FLOW);
  writeFileSync(join(home, 'broken.json'), '{ not json');
  writeFileSync(join(home, 'huge.json'), JSON.stringify({ pad: 'x'.repeat(300 * 1024) }));
  symlinkSync('/etc/durable-startup.d/manifest.json', join(home, 'outside.json'));
  symlinkSync(join(home, '.sbxw', 'stolen.json'), join(home, 'link.json'));
  const cases = [
    [{}, /give the diagram's JSON file/],
    [{ file: 'missing.json' }, /no such file/],
    [{ file: 'flow.txt' }, /not a \.json file/],
    [{ file: 'broken.json' }, /not JSON/],
    [{ file: 'huge.json' }, /more than 256 KB/],
    [{ file: '.sbxw/stolen.json' }, /private folder/],
    [{ file: 'link.json' }, /private folder/],
    [{ file: join(home, '.claude', 'creds.json') }, /private folder/],
    [{ file: 'flow.json', canvas: '../c-0123' }, /not a canvas id/],
    [{ file: 'flow.json', replace: 'd-../../x' }, /not a diagram id/],
    [{ file: 'flow.json', kind: 'architecture' }, /kind: one of workflow, sequence, dataflow, lifecycle/],
    [{ file: 'flow.json', title: 7 }, /title: a string/],
  ];
  if (existsSync('/etc/durable-startup.d/manifest.json')) cases.push([{ file: 'outside.json' }, /inside the workspace, the home or \/tmp/]);
  for (const [args, re] of cases) {
    const r = await call(home, 'canvas_add_diagram', args);
    assert.equal(r.isError, true, JSON.stringify(args));
    assert.match(r.content[0].text, re, JSON.stringify(args));
  }
  d.server.close();
  assert.equal(d.seen.length, 0, 'nothing was posted');
});

test('canvas_add_diagram posts the file once, never names a sandbox, and gives the canvas id', async () => {
  const d = await fakeDaemon(() => [200, added]);
  const home = sandbox(d.port);
  const r = await call(home, 'canvas_add_diagram', { file: 'flow.json', title: 'Release', kind: 'workflow', canvas: 'c-0123456789abcdef' });
  d.server.close();
  assert.equal(r.isError, false);
  assert.match(r.content[0].text, /Diagram d-0123456789ab \("Release", workflow, 2 node\(s\), 1 edge\(s\)\) added to canvas c-0123456789abcdef \("Review"\), round 2\./);
  assert.match(r.content[0].text, /by its id, c-0123456789abcdef/);
  assert.equal(d.seen.length, 1);
  const s = d.seen[0];
  assert.equal(s.path, '/api/canvas/diagrams');
  assert.equal(s.auth, 'Bearer sbxw_testtoken');
  assert.deepEqual(Object.keys(s.body).sort(), ['batch', 'canvas', 'diagram']);
  assert.deepEqual(s.body.diagram, { json: FLOW, title: 'Release', kind: 'workflow' });
});

test('a refusal from the daemon is reported as an error', async () => {
  const d = await fakeDaemon(() => [400, { ok: false, error: 'a canvas holds at most 50 diagrams; start a new canvas' }]);
  const home = sandbox(d.port);
  const r = await call(home, 'canvas_add_diagram', { file: 'flow.json' });
  d.server.close();
  assert.equal(r.isError, true);
  assert.match(r.content[0].text, /at most 50 diagrams/);
});

test('canvas_read saves each changed diagram under generated names, 0600, and refuses a forged id', async () => {
  const canvas = `c-${randomBytes(8).toString('hex')}`;
  const id = `d-${randomBytes(6).toString('hex')}`;
  const json = JSON.stringify({ version: 1, id, kind: 'workflow', title: 'Release', nodes: [{ id: 'a', label: 'Lead review' }] }, null, 2);
  const d = await fakeDaemon(() => [200, {
    ok: true,
    text: 'The person\'s remarks...\nReview round 1: 0 pin(s), 1 changed diagram(s) (below).',
    diagrams: [
      { canvas, id, title: 'Release‮', rev: 2, lines: ['Renamed node a from "A" to "Lead review".', 'line\nwith a break'], json },
      { canvas: '../../etc', id: 'passwd', title: 'Forged', rev: 1, lines: ['x'], json: '{}' },
      { canvas, id: `d-${randomBytes(6).toString('hex')}`, title: 'Not JSON', rev: 1, lines: [], json: 'not json' },
    ],
  }]);
  const home = sandbox(d.port);
  try {
    const r = await call(home, 'canvas_read', {});
    const text = r.content[0].text;
    const file = join('/tmp/sbxw-canvas', canvas, `${id}.json`);
    assert.match(text, new RegExp(`Diagram ${id} "Release" on canvas ${canvas}`));
    assert.match(text, /- Renamed node a from "A" to "Lead review"\./);
    assert.match(text, /- linewith a break/);
    assert.ok(text.includes(`The whole diagram (sbxw's schema): ${file}.`), text);
    assert.match(text, new RegExp(`replace: "${id}"`));
    assert.equal(readFileSync(file, 'utf8').trim(), json);
    assert.equal(statSync(file).mode & 0o777, 0o600);
    assert.equal(statSync(join('/tmp/sbxw-canvas', canvas)).mode & 0o777, 0o700);
    // The forged and the broken ones: said, not written.
    assert.equal((text.match(/could not be saved/g) || []).length, 2, text);
    assert.ok(!existsSync('/tmp/sbxw-canvas/../../etc/passwd.json') && !existsSync('/etc/passwd.json'));
    assert.deepEqual(readdirSync(join('/tmp/sbxw-canvas', canvas)), [`${id}.json`], 'no temporary file left, nothing else');
  } finally {
    d.server.close();
    rmSync(join('/tmp/sbxw-canvas', canvas), { recursive: true, force: true });
  }
});

test('canvas_read without diagrams reads as before', async () => {
  const d = await fakeDaemon(() => [200, { ok: true, text: 'No new review round: the person has not sent remarks since you last read them.', diagrams: [] }]);
  const home = sandbox(d.port);
  const r = await call(home, 'canvas_read', {});
  d.server.close();
  assert.equal(r.content[0].text, 'No new review round: the person has not sent remarks since you last read them.');
});
