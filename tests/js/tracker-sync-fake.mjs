// A fake sbx MCP gateway for tests/js/tracker-sync.test.mjs: JSON-RPC over
// HTTP on loopback, answering as JSON or as an event stream, with a session
// id, the gateway's own tools, and a tracker server whose tools are either
// listed directly or reached only through `mcp-exec`. Invented content.
import http from 'node:http';
import { readFileSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn } from 'node:child_process';

export const TOKEN = 'Bearer sbx_gw_SECRET_7f3a9c1d2e';
export const SESSION = 'sess-42';
const GATEWAY_TOOLS = ['code-mode', 'mcp-add', 'mcp-config-set', 'mcp-exec', 'mcp-find'].map(name => ({ name, inputSchema: { type: 'object', properties: {} } }));

/// `o.tools`: {name: {schema?, run(args, n) -> answer}}; `o.exec`: reach
/// them only through mcp-exec; `o.sse`: answer as an event stream;
/// `o.http(body, n)`: an early answer ([status, headers] | 'drop' | 'hang').
/// An answer: {json} | {text} | {error: text} | {rpcError: {code, message}} | 'drop' | 'hang'.
export function fakeGateway(o = {}) {
  const seen = [];
  const counts = new Map();
  const server = http.createServer((req, res) => {
    let raw = '';
    req.on('data', c => (raw += c));
    req.on('end', () => {
      let body = {};
      try { body = JSON.parse(raw || '{}'); } catch { /* answered below */ }
      seen.push({ auth: req.headers.authorization, session: req.headers['mcp-session-id'], protocol: req.headers['mcp-protocol-version'], accept: req.headers.accept, body });
      const key = body.method === 'tools/call' ? `call:${body.params?.name === 'mcp-exec' ? body.params.arguments?.name : body.params?.name}` : body.method;
      const n = (counts.get(key) || 0) + 1;
      counts.set(key, n);
      if (req.headers.authorization !== TOKEN) { res.writeHead(401); res.end(); return; }
      const early = o.http && o.http(body, n);
      if (early === 'drop') { req.socket.destroy(); return; }
      if (early === 'hang') return;
      if (early) { res.writeHead(early[0], early[1] || {}); res.end(early[2] || ''); return; }
      if (body.method === 'initialize') return send(res, body.id, { protocolVersion: '2025-06-18', capabilities: { tools: {}, logging: {}, prompts: {}, resources: {} }, serverInfo: { name: 'fake-gateway', version: '0' } }, { 'Mcp-Session-Id': SESSION });
      if (req.headers['mcp-session-id'] !== SESSION) { res.writeHead(400); res.end('no session'); return; }
      if (!body.id && body.method?.startsWith('notifications/')) { res.writeHead(202); res.end(); return; }
      if (body.method === 'tools/list') {
        const server = Object.entries(o.tools || {}).map(([name, t]) => ({ name, inputSchema: t.schema || { type: 'object', properties: {} } }));
        return send(res, body.id, { tools: o.exec ? GATEWAY_TOOLS : [...GATEWAY_TOOLS, ...server] });
      }
      if (body.method === 'tools/call') {
        let name = body.params?.name;
        let args = body.params?.arguments || {};
        if (name === 'mcp-exec') { name = args.name; args = args.arguments || {}; } else if (o.exec && o.tools?.[name]) name = '';
        const t = o.tools?.[name];
        if (!t) return send(res, body.id, { content: [{ type: 'text', text: `Tool "${String(name)}" not found` }], isError: true });
        const out = t.run(args, n);
        if (out === 'drop') { req.socket.destroy(); return; }
        if (out === 'hang') return;
        if (out.rpcError) return send(res, body.id, null, {}, out.rpcError);
        if (out.error) return send(res, body.id, { content: [{ type: 'text', text: out.error }], isError: true });
        return send(res, body.id, { content: [{ type: 'text', text: out.text ?? JSON.stringify(out.json) }] });
      }
      send(res, body.id, null, {}, { code: -32601, message: 'Method not found' });
    });
  });
  function send(res, id, result, headers = {}, error) {
    const msg = error ? { jsonrpc: '2.0', id, error } : { jsonrpc: '2.0', id, result };
    if (o.sse) {
      res.writeHead(200, { 'Content-Type': 'text/event-stream', ...headers });
      // A notification first, then the answer split across writes.
      res.write('event: message\r\ndata: {"jsonrpc":"2.0","method":"notifications/message","params":{"level":"info","data":"hi"}}\r\n\r\n');
      const s = `event: message\ndata: ${JSON.stringify(msg)}\n\n`;
      res.write(s.slice(0, 7));
      setTimeout(() => res.end(s.slice(7)), 5);
    } else {
      res.writeHead(200, { 'Content-Type': 'application/json', ...headers });
      res.end(JSON.stringify(msg));
    }
  }
  return new Promise(resolve => server.listen(0, '127.0.0.1', () => resolve({ server, seen, port: server.address().port, calls: () => seen.filter(s => s.body.method === 'tools/call').map(s => s.body.params) })));
}

/// A HOME with the gateway's entry, as sbx writes it.
export function home(port, entry) {
  const dir = mkdtempSync(join(tmpdir(), 'sbxw-tracker-home-'));
  const e = entry === undefined ? { type: 'http', url: `http://127.0.0.1:${port}/mcp`, headers: { Authorization: TOKEN } } : entry;
  writeFileSync(join(dir, '.claude.json'), JSON.stringify(e ? { mcpServers: { 'mcp-gateway': e } } : { mcpServers: {} }));
  return dir;
}

const SRC = readFileSync(new URL('../../assets/tracker-sync.js', import.meta.url), 'utf8');

/// The script as the daemon runs it (`node - =<json>`, the source on stdin),
/// with test constants put in: `fetchHosts` (host:port list), `tmp`,
/// `timeoutMs`. Resolves {answer, stdout, stderr, code}.
export function run(req, o = {}) {
  let src = SRC;
  const swap = (from, to) => {
    if (!src.includes(from)) throw new Error(`constant not found: ${from}`);
    src = src.replace(from, to);
  };
  if (o.fetchHosts) {
    swap(/^const GITHUB_FETCH_HOSTS = .*$/m.exec(src)[0], `const GITHUB_FETCH_HOSTS = ${JSON.stringify(o.fetchHosts)};`);
    swap("const SCHEMES = ['https:'];", "const SCHEMES = ['http:', 'https:'];");
  }
  if (o.tmp) swap("const TMP_DIR = '/tmp/sbxw-tracker';", `const TMP_DIR = ${JSON.stringify(o.tmp)};`);
  if (o.timeoutMs) swap('const CALL_TIMEOUT_MS = 30000;', `const CALL_TIMEOUT_MS = ${o.timeoutMs};`);
  const env = { ...process.env, HOME: o.home || mkdtempSync(join(tmpdir(), 'sbxw-tracker-empty-')), NO_PROXY: '127.0.0.1,localhost', no_proxy: '127.0.0.1,localhost' };
  for (const k of ['HTTP_PROXY', 'HTTPS_PROXY', 'http_proxy', 'https_proxy']) delete env[k];
  const arg = typeof req === 'string' ? req : `=${JSON.stringify(req)}`;
  return new Promise(resolve => {
    const p = spawn(process.execPath, ['-', arg], { env });
    let stdout = '', stderr = '';
    p.stdout.on('data', c => (stdout += c));
    p.stderr.on('data', c => (stderr += c));
    p.on('close', code => {
      const last = stdout.trimEnd().split('\n').pop() || '';
      let answer = null;
      if (last.startsWith('SBXW-TRACKER ')) answer = JSON.parse(last.slice('SBXW-TRACKER '.length));
      resolve({ answer, stdout, stderr, code });
    });
    p.stdin.end(src);
  });
}
