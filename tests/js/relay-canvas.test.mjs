// The relay MCP server's canvas tools (assets/relay-mcp.js), driven over
// stdio against a fake daemon on loopback: the shots are read inside the
// "sandbox" (a temporary home), checked by content, posted one per request
// with one batch id; refusals are named; nothing names the sandbox (the
// token does). Every picture is a few invented bytes.
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
  const server = http.createServer((req, res) => {
    let raw = '';
    req.on('data', (c) => (raw += c));
    req.on('end', () => {
      const body = JSON.parse(raw || '{}');
      seen.push({ path: req.url, auth: req.headers.authorization, body });
      const [status, out] = answer(req.url, body, seen.length);
      res.writeHead(status, { 'content-type': 'application/json' });
      res.end(JSON.stringify(out));
    });
  });
  return new Promise((resolve) => server.listen(0, '127.0.0.1', () => resolve({ server, seen, port: server.address().port })));
}

function sandbox(port) {
  const home = mkdtempSync(join(tmpdir(), 'sbxw-canvas-test-'));
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

test('canvas_add_shots reads the files, posts one shot a request in one batch, and names refusals', async () => {
  const d = await fakeDaemon((url, body, n) => [200, { ok: true, canvas: 'c-0123456789abcdef', title: 'Review', round: 1, shot: `s-${n}` }]);
  const home = sandbox(d.port);
  const r = await call(home, 'canvas_add_shots', {
    title: 'Checkout',
    shots: [
      { file: 'home.png', title: 'Home', width: 1440, screen: 'home' },
      { file: 'page.svg', title: 'Not a picture' },
      { file: 'missing.png', title: 'Gone' },
      { file: 'settings.png', title: 'Settings', note: 'dark theme' },
    ],
  });
  d.server.close();
  const text = r.content[0].text;
  assert.match(text, /2 shot\(s\) added to canvas c-0123456789abcdef/);
  assert.match(text, /page\.svg: not a PNG, JPEG or WebP picture/);
  assert.match(text, /missing\.png: /);
  assert.equal(d.seen.length, 2);
  const [a, b] = d.seen;
  assert.equal(a.path, '/api/canvas/shots');
  assert.equal(a.auth, 'Bearer sbxw_testtoken');
  assert.equal(a.body.batch, b.body.batch, 'one round');
  assert.equal(a.body.title, 'Checkout');
  assert.equal(b.body.canvas, 'c-0123456789abcdef', 'the next shots go to the canvas the first made');
  assert.equal(Buffer.from(a.body.shot.b64, 'base64').equals(PNG), true);
  assert.equal(a.body.shot.width, 1440);
  assert.equal(a.body.shot.screen, 'home');
  for (const s of d.seen) assert.equal(s.body.sandbox, undefined, 'never names a sandbox');
});

test('a refusal from the daemon is reported, and more than 24 shots are cut', async () => {
  const d = await fakeDaemon((url, body, n) => (n === 1 ? [400, { ok: false, error: 'the picture is 9000x10 pixels, more than 8192 a side' }] : [200, { ok: true, canvas: 'c-0123456789abcdef', title: 'R', round: 2 }]));
  const home = sandbox(d.port);
  const shots = Array.from({ length: 26 }, (_, i) => ({ file: 'home.png', title: `S${i}` }));
  const r = await call(home, 'canvas_add_shots', { canvas: 'c-0123456789abcdef', shots });
  d.server.close();
  const text = r.content[0].text;
  assert.match(text, /23 shot\(s\) added/);
  assert.match(text, /more than 8192 a side/);
  assert.equal((text.match(/at most 24 shots/g) || []).length, 2);
  assert.equal(d.seen.length, 24);
});

test('canvas_read and canvas_list hand back the daemon text', async () => {
  const d = await fakeDaemon((url) => (url === '/api/canvas/read' ? [200, { ok: true, text: 'The person\'s remarks...\nPin 1 on shot "Home"' }] : [200, { ok: true, canvases: [{ id: 'c-0123456789abcdef', title: 'R', shots: 3, rounds: 1, reviews: 1, unread_reviews: 1 }] }]));
  const home = sandbox(d.port);
  const r = await call(home, 'canvas_read', {});
  const l = await call(home, 'canvas_list', {});
  d.server.close();
  assert.match(r.content[0].text, /Pin 1 on shot "Home"/);
  assert.match(l.content[0].text, /c-0123456789abcdef: "R", 3 shot\(s\) in 1 round\(s\).*1 not read yet/);
});
