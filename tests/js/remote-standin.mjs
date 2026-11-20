// A stand-in for the daemon's remote listener, for the phone page's browser
// test and its screenshots: no daemon, no token, no network. It serves
// assets/remote/ at the root under the headers of API.md section 4.7, and
// answers the /r/ routes from tests/js/remote-fixtures.json. It records
// every request so a test can say which route was called, and how often.
//
// `/__ctl` (POST JSON) is the test's lever, never part of the contract:
// {down: true} drops every /r/ request, {paired: false} answers 401,
// {pair: [...states]} the next pair/status answers, {pairStart: 429} the
// status of pair/start, {messageDelay: ms, messageReplies: [{status, body}]}
// how the next messages are answered, {emit: {event, data}}
// pushes one server-sent event, {leaders}, {history: {sandbox: page}}.
// Usage: node tests/js/remote-standin.mjs [port]
import http from 'node:http';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const root = new URL('../../assets/remote/', import.meta.url);
const fx = JSON.parse(readFileSync(new URL('./remote-fixtures.json', import.meta.url), 'utf8'));
const STATIC = {
  '/': ['index.html', 'text/html; charset=utf-8'],
  '/index.html': ['index.html', 'text/html; charset=utf-8'],
  '/remote-core.js': ['remote-core.js', 'text/javascript; charset=utf-8'],
  '/remote.js': ['remote.js', 'text/javascript; charset=utf-8'],
  '/remote.css': ['remote.css', 'text/css; charset=utf-8'],
  '/manifest.webmanifest': ['manifest.webmanifest', 'application/manifest+json'],
  '/icon-192.png': ['icon-192.png', 'image/png'],
  '/icon-512.png': ['icon-512.png', 'image/png'],
  '/apple-touch-icon.png': ['apple-touch-icon.png', 'image/png'],
};
export const CSP = "default-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";
const HEADERS = {
  'Content-Security-Policy': CSP,
  'X-Content-Type-Options': 'nosniff',
  'Referrer-Policy': 'no-referrer',
  'X-Frame-Options': 'DENY',
};

// The recorded times, moved so the newest is a minute ago (rows say "1 min").
function recent(leaders) {
  const l = structuredClone(leaders);
  const newest = Math.max(...l.leaders.map(x => x.last_at || 0));
  for (const x of l.leaders) if (x.last_at) x.last_at += Date.now() - 60000 - newest;
  return l;
}

export function startStandin(port = 0) {
  const s = {
    log: [],
    down: false,
    paired: true,
    pair: ['pending', 'pending', 'paired'],
    leaders: recent(fx.leaders),
    history: { shop: fx.history.shop, website: fx.history.website },
    pairStart: 200,
    messageDelay: 0,
    messageReplies: [],
    older: { shop: fx.history.shop_older },
    newer: { shop: fx.history.shop_newer },
    streams: new Set(),
    transcript: 'please also add an index on the customer id',
  };
  const send = (res, status, body, extra = {}) => {
    res.writeHead(status, { ...HEADERS, 'Content-Type': 'application/json', 'Cache-Control': 'no-store', ...extra });
    res.end(body === undefined ? '' : JSON.stringify(body));
  };
  const emit = (event, data) => {
    for (const r of s.streams) r.write(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`);
  };
  const server = http.createServer(async (req, res) => {
    const url = new URL(req.url, 'http://x');
    const chunks = [];
    for await (const c of req) chunks.push(c);
    const raw = Buffer.concat(chunks);
    if (url.pathname === '/__ctl') {
      const c = JSON.parse(raw.toString() || '{}');
      if ('down' in c) {
        s.down = c.down;
        if (c.down) for (const r of s.streams) r.destroy();
      }
      if ('paired' in c) s.paired = c.paired;
      if (c.pair) s.pair = c.pair;
      if (c.leaders) s.leaders = recent(c.leaders);
      if (c.pairStart) s.pairStart = c.pairStart;
      if ('messageDelay' in c) s.messageDelay = c.messageDelay;
      if (c.messageReplies) s.messageReplies = c.messageReplies;
      if (c.history) Object.assign(s.history, c.history);
      if (c.emit) emit(c.emit.event, c.emit.data);
      if (c.reset) s.log.length = 0;
      return send(res, 200, { ok: true });
    }
    if (url.pathname === '/__log') return send(res, 200, s.log);
    const file = STATIC[url.pathname];
    if (file && req.method === 'GET') {
      res.writeHead(200, { ...HEADERS, 'Content-Type': file[1] });
      return res.end(readFileSync(new URL(file[0], root)));
    }
    if (!url.pathname.startsWith('/r/')) return send(res, 404, { ok: false, error: 'not found' });
    const ct = req.headers['content-type'] || '';
    const entry = { method: req.method, path: url.pathname, query: url.search, ct, origin: req.headers.origin || null, cookie: !!req.headers.cookie };
    if (ct.startsWith('application/json')) { try { entry.body = JSON.parse(raw.toString()); } catch (_) { entry.body = raw.toString(); } }
    else if (raw.length) entry.bytes = raw.length;
    s.log.push(entry);
    if (s.down) return req.socket.destroy();
    const p = url.pathname;
    if (p === '/r/pair/start') {
      if (s.pairStart !== 200) return send(res, s.pairStart, { ok: false, error: 'refused by the stand-in' }, s.pairStart === 429 ? { 'Retry-After': '60' } : {});
      return send(res, 200, { ...fx.pair_start, expires_ms: Date.now() + 300000 });
    }
    if (p === '/r/pair/status') {
      const state = s.pair.length > 1 ? s.pair.shift() : s.pair[0];
      if (state === 'paired') { s.paired = true; return send(res, 200, { state, device: { id: fx.me.device.id, name: 'iPhone' } }); }
      return send(res, 200, { state });
    }
    if (!s.paired) return send(res, 401, { ok: false, error: 'not paired' });
    if (p === '/r/me') return send(res, 200, fx.me);
    if (p === '/r/logout') { s.paired = false; return send(res, 200, { ok: true }); }
    if (p === '/r/leaders') return send(res, 200, s.leaders);
    if (p === '/r/events') {
      res.writeHead(200, { ...HEADERS, 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-store' });
      res.write(`event: leaders\ndata: ${JSON.stringify(s.leaders)}\n\n`);
      s.streams.add(res);
      req.on('close', () => s.streams.delete(res));
      return;
    }
    const m = /^\/r\/leaders\/([^/]+)\/(history|message|answer|interrupt)$/.exec(p);
    if (m) {
      const sb = decodeURIComponent(m[1]);
      if (m[2] === 'history') {
        if (url.searchParams.has('before')) return send(res, 200, s.older[sb] || { page: { items: [], latest_seq: 0 } });
        if (url.searchParams.has('after')) return send(res, 200, s.newer[sb] || { page: { items: [], latest_seq: 0 } });
        return s.history[sb] ? send(res, 200, s.history[sb]) : send(res, 404, { ok: false, error: 'not a leader' });
      }
      if (m[2] === 'message') {
        // A message waits for the echo before its Return: 1 to 4 s.
        await new Promise(r => setTimeout(r, s.messageDelay));
        const reply = s.messageReplies.length ? s.messageReplies.shift() : { status: 200, body: { ok: true, masked: 0 } };
        return send(res, reply.status, reply.body);
      }
      return send(res, 200, { ok: true, masked: 0 });
    }
    if (p === '/r/voice/transcribe') return send(res, 200, { text: s.transcript, masked: 0 });
    return send(res, 404, { ok: false, error: 'not found' });
  });
  return new Promise(resolve => server.listen(port, '127.0.0.1', () => {
    s.port = server.address().port;
    s.close = () => { for (const r of s.streams) r.destroy(); server.closeAllConnections?.(); return new Promise(r => server.close(r)); };
    s.emit = emit;
    resolve(s);
  }));
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const s = await startStandin(Number(process.argv[2]) || 8099);
  process.stdout.write(`remote stand-in on http://127.0.0.1:${s.port}/\n`);
}
