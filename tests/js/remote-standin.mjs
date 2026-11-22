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
// pushes one server-sent event, {leaders}, {history: {sandbox: page}},
// {uploadReplies: [{status, body}]} how the next uploads are answered,
// {uploadSlow: ms} a pause after each 64 KiB read (so progress shows),
// {uploadHold: true} uploads wait until {uploadHold: false},
// {transcribeDelay: ms} how long a transcription takes, {transcribeReplies:
// [{status, body}]} how the next transcriptions are answered.
// Level 2 (API-level2.md): {l2: {approvals: {...}}} the state GET /r/approvals
// answers, {l2Digest: {plans|merges|network|stopped: hex}} a detail's new
// digest (a challenge with the old one is "changed"), {l2Replies: {kind:
// [{status, body}]}} how the next challenge or action is answered,
// {pkStates: [...]} the next passkey/status answers, {l2Reset: true},
// {l2Gone: [ids]} whose detail answers 404, {l2Lists: {plans|merges|network|stopped: <the list's whole answer>}}.
// Kanban (API-kanban.md): {kanban: {sandbox: board | null}} a board (null:
// 404, the leader stopped), {tickets: {"sandbox/n": detail}}.
// Uploads follow API.md 2.6.1: the raw body, an id of 32 hex back, no path.
// Live replies (thread phone-live): {newer: {sandbox: page}} what a
// `?after=` read answers, {newerHold: true} such reads wait until
// {newerHold: false}, {mute: true} open streams stay open but get nothing
// more (a stream that died without a word, as after a phone's sleep).
// Push (API-push.md, thread push-daemon): GET /sw.js, GET /r/push/key,
// POST /r/push/subscribe and /unsubscribe. {push: {on, keyReply: {status,
// body}, subscribeReplies: [{status, body}], tag}} sets what they answer
// (`tag`, else the one of the last subscription posted); the
// subscriptions posted are in `state.push.subs`, and the log has every call.
// Usage: node tests/js/remote-standin.mjs [port]
import http from 'node:http';
import { createHash, randomBytes } from 'node:crypto';
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
  '/sw.js': ['sw.js', 'text/javascript; charset=utf-8'],
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
    uploads: new Map(),  // id -> { sandbox, name, size }
    uploadReplies: [],
    uploadSlow: 0,
    uploadHold: false,
    transcribeDelay: 0,
    transcribeReplies: [],
    l2: null,
    kanban: { shop: fx.kanban.board, website: fx.kanban.empty },
    tickets: { 'shop/12': fx.kanban.ticket },
    holds: [],
    push: { on: false, key: Buffer.concat([Buffer.from([4]), randomBytes(64)]).toString('base64url'), keyReply: null, subscribeReplies: [], subs: [] },
  };
  const l2Fresh = () => {
    const L = structuredClone(fx.level2);
    return {
      approvals: L.approvals, L,
      digest: { plans: L.plan.digest, merges: L.merge.digest, network: L.network_one.digest, stopped: L.stopped_one.digest },
      replies: {}, pkStates: [], pk: 'none', rid: null, chal: null, done: [],
    };
  };
  s.l2 = l2Fresh();
  const B64U = /^[A-Za-z0-9_-]+$/;
  const L2_ACT = /^\/r\/(plans|merges|network|stopped)\/([^/]+)\/(approve|refuse|merge|respond|remove)$/;
  const L2_DETAIL = /^\/r\/(plans|merges|network|stopped)\/([^/]+)$/;
  const KIND = { approve: 'plan_approve', refuse: 'plan_refuse', merge: 'merge', respond: 'network_respond', remove: 'sandbox_remove' };
  const WHAT = { plan_approve: 'plans', plan_refuse: 'plans', merge: 'merges', network_respond: 'network', sandbox_remove: 'stopped' };
  function level2(req, p, body, res) {
    const L = s.l2.L;
    const A = s.l2.approvals;
    const off = () => send(res, 403, { ok: false, error: 'level 2 is off', reason: 'level2_off' });
    if (p === '/r/approvals' && req.method === 'GET') return send(res, 200, A.level2 ? A : { level2: false, passkey: A.passkey, rp_ok: A.rp_ok, counts: null });
    if (!A.level2) return off();
    // v1.1: the ceremony is bound to the request_id of start; any other id
    // is "none" / no_grant; status never says "registered".
    const ridOk = id => typeof id === 'string' && id === s.l2.rid;
    if (p === '/r/passkey/start') {
      s.l2.pk = 'pending';
      s.l2.rid = randomBytes(16).toString('hex');
      return send(res, 200, { ...L.passkey_start, request_id: s.l2.rid, expires_ms: Date.now() + 300000 });
    }
    if (p === '/r/passkey/status') {
      if (!ridOk(new URL(req.url, 'http://x').searchParams.get('request_id'))) return send(res, 200, { state: 'none', expires_ms: null });
      if (s.l2.pkStates.length) s.l2.pk = s.l2.pkStates.shift();
      return send(res, 200, { state: s.l2.pk, expires_ms: Date.now() + 200000 });
    }
    if (p === '/r/passkey/options') {
      if (!ridOk(body?.request_id) || s.l2.pk !== 'granted') return send(res, 403, { ok: false, error: 'no grant', reason: 'no_grant' });
      return send(res, 200, { ...L.passkey_options, expires_ms: Date.now() + 120000 });
    }
    if (p === '/r/passkey/register') {
      const q = s.l2.replies.register?.shift();
      if (q) { s.l2.pk = 'none'; return send(res, q.status, q.body); }
      if (!ridOk(body?.request_id) || s.l2.pk !== 'granted') return send(res, 403, { ok: false, error: 'no grant', reason: 'no_grant' });
      const ok = body && ['id', 'client_data_json', 'attestation_object'].every(k => typeof body[k] === 'string' && B64U.test(body[k])) && Object.keys(body).length === 4;
      s.l2.pk = 'none';
      if (!ok) return send(res, 403, { ok: false, error: 'passkey', reason: 'passkey' });
      s.l2.pk = 'none';
      s.l2.rid = null;
      A.passkey = true;
      A.rp_ok = true;
      return send(res, 200, { ok: true });
    }
    if (p === '/r/plans') return send(res, 200, L.plans);
    if (p === '/r/merges') return send(res, 200, L.merges);
    if (p === '/r/network') {
      const q = s.l2.replies.network?.shift();
      return q ? send(res, q.status, q.body) : send(res, 200, L.network);
    }
    if (p === '/r/stopped') return send(res, 200, L.stopped);
    if (p === '/r/approvals/challenge') {
      if (!A.passkey || A.rp_ok === false) return send(res, 403, { ok: false, error: 'no passkey', reason: 'no_passkey' });
      const q = s.l2.replies.challenge?.shift();
      if (q) return send(res, q.status, q.body);
      const what = WHAT[body?.kind];
      if (!what) return send(res, 400, { ok: false, error: 'kind' });
      if (body.digest !== s.l2.digest[what]) return send(res, 409, { ok: false, error: 'changed', reason: 'changed' });
      s.l2.chal = { kind: body.kind, target: body.target, option: body.option || null };
      return send(res, 200, { ...L.challenge, expires_ms: Date.now() + 60000 });
    }
    const act = L2_ACT.exec(p);
    if (act && req.method === 'POST') {
      const kind = KIND[act[3]];
      const target = decodeURIComponent(act[2]);
      const chal = s.l2.chal;
      s.l2.chal = null; // taken, success or failure
      const a = body?.assertion;
      const shaped = a && ['id', 'client_data_json', 'authenticator_data', 'signature'].every(k => typeof a[k] === 'string' && B64U.test(a[k]));
      if (!chal || chal.kind !== kind || chal.target !== target || (kind === 'network_respond' && chal.option !== body.option) || !shaped) {
        return send(res, 403, { ok: false, error: 'passkey', reason: 'passkey' });
      }
      const q = s.l2.replies[kind]?.shift();
      if (q) return send(res, q.status, q.body);
      s.l2.done.push({ kind, target, option: body.option || null });
      if (kind === 'merge') return send(res, 200, L.merge_done);
      return send(res, 200, { ok: true });
    }
    const det = L2_DETAIL.exec(p);
    if (det && req.method === 'GET') {
      const what = det[1];
      if (s.l2.gone?.includes(decodeURIComponent(det[2]))) return send(res, 404, { ok: false, error: 'not found', reason: 'not_found' });
      const one = { plans: L.plan, merges: L.merge, network: L.network_one, stopped: L.stopped_one }[what];
      return send(res, 200, { ...one, digest: s.l2.digest[what] });
    }
    return null;
  }
  const UPLOAD = /^\/r\/leaders\/([^/]+)\/upload$/;
  const UPLOAD_ID = /^\/r\/leaders\/([^/]+)\/upload\/([^/]+)$/;
  // The daemon's name rule (upload_name): last component, [A-Za-z0-9._-],
  // no leading dot, 120 at most, `upload` when nothing is left.
  const cleanName = n => (String(n).split(/[\/\\]/).pop().replace(/[^A-Za-z0-9._-]/g, '_').replace(/^\.+/, '').slice(0, 120)) || 'upload';
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
    const slow = req.method === 'POST' && UPLOAD.test(url.pathname) && s.uploadSlow;
    for await (const c of req) {
      chunks.push(c);
      if (slow) { req.pause(); await new Promise(r => setTimeout(r, s.uploadSlow)); req.resume(); }
    }
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
      if (c.uploadReplies) s.uploadReplies = c.uploadReplies;
      if ('transcribeDelay' in c) s.transcribeDelay = c.transcribeDelay;
      if (c.reset) s.transcribeReplies = [];
      if (c.transcribeReplies) s.transcribeReplies = c.transcribeReplies;
      if (c.l2Reset || !s.l2) s.l2 = l2Fresh();
      if (c.l2) Object.assign(s.l2.approvals, c.l2.approvals || {});
      if (c.l2Gone) s.l2.gone = c.l2Gone;
      if (c.l2Lists) Object.assign(s.l2.L, c.l2Lists);
      if (c.l2Digest) Object.assign(s.l2.digest, c.l2Digest);
      if (c.l2Replies) Object.assign(s.l2.replies, c.l2Replies);
      if (c.pkStates) s.l2.pkStates = c.pkStates;
      if (c.kanban) Object.assign(s.kanban, c.kanban);
      if (c.tickets) Object.assign(s.tickets, c.tickets);
      if (c.kanbanReset) { s.kanban = { shop: fx.kanban.board, website: fx.kanban.empty }; s.tickets = { 'shop/12': fx.kanban.ticket }; }
      if ('uploadSlow' in c) s.uploadSlow = c.uploadSlow;
      if ('uploadHold' in c) { s.uploadHold = c.uploadHold; if (!c.uploadHold) s.holds.splice(0).forEach(r => r()); }
      if (c.push) Object.assign(s.push, c.push);
      if (c.emit) emit(c.emit.event, c.emit.data);
      if (c.newer) Object.assign(s.newer, c.newer);
      if ('newerHold' in c) { s.newerHold = c.newerHold; if (!c.newerHold) (s.newerHolds || []).splice(0).forEach(r => r()); }
      if (c.mute) s.streams.clear();
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
    if (p === '/r/push/key' && req.method === 'GET') {
      return s.push.keyReply ? send(res, s.push.keyReply.status, s.push.keyReply.body) : send(res, 200, { key: s.push.key, on: s.push.on, tag: s.push.on ? (s.push.tag ?? (s.push.subs[0] ? createHash('sha256').update(s.push.subs[0].endpoint).digest('hex').slice(0, 16) : null)) : null });
    }
    if (p === '/r/push/subscribe' && req.method === 'POST') {
      const q = s.push.subscribeReplies.shift();
      if (q) return send(res, q.status, q.body);
      const b = entry.body;
      const ok = b && typeof b === 'object' && Object.keys(b).length === 3 && ['endpoint', 'p256dh', 'auth'].every(k => typeof b[k] === 'string' && b[k]);
      if (!ok) return send(res, 400, { ok: false, error: 'bad subscription', reason: 'bad_request' });
      s.push.subs = [b];
      s.push.on = true;
      return send(res, 200, { ok: true });
    }
    if (p === '/r/push/unsubscribe' && req.method === 'POST') {
      s.push.subs = [];
      s.push.on = false;
      return send(res, 200, { ok: true });
    }
    if (p === '/r/logout') { s.paired = false; return send(res, 200, { ok: true }); }
    if (p === '/r/leaders') return send(res, 200, s.leaders);
    if (p === '/r/events') {
      res.writeHead(200, { ...HEADERS, 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-store' });
      res.write(`event: leaders\ndata: ${JSON.stringify(s.leaders)}\n\n`);
      s.streams.add(res);
      req.on('close', () => s.streams.delete(res));
      return;
    }
    const kb = /^\/r\/leaders\/([^/]+)\/kanban(?:\/(?:T-)?([0-9]{1,9}))?$/.exec(p);
    if (kb && req.method === 'GET') {
      const sb = decodeURIComponent(kb[1]);
      if (!s.kanban[sb]) return send(res, 404, { error: 'not found' });
      if (!kb[2]) return send(res, 200, s.kanban[sb]);
      const t = s.tickets[`${sb}/${Number(kb[2])}`];
      return t ? send(res, 200, t) : send(res, 404, { error: 'not found' });
    }
    const m = /^\/r\/leaders\/([^/]+)\/(history|message|answer|interrupt)$/.exec(p);
    if (m) {
      const sb = decodeURIComponent(m[1]);
      if (m[2] === 'history') {
        if (url.searchParams.has('before')) return send(res, 200, s.older[sb] || { page: { items: [], latest_seq: 0 } });
        if (url.searchParams.has('after')) {
          if (s.newerHold) await new Promise(r => (s.newerHolds ||= []).push(r));
          return send(res, 200, s.newer[sb] || { page: { items: [], latest_seq: 0 } });
        }
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
    if (/^\/r\/(approvals|passkey|plans|merges|network|stopped)(\/|$)/.test(p)) {
      if (level2(req, p, entry.body, res) !== null) return;
    }
    const up = UPLOAD.exec(p);
    if (up && req.method === 'POST') {
      if (s.uploadHold) await new Promise(r => s.holds.push(r));
      if (ct !== 'application/octet-stream') return send(res, 415, { ok: false, error: 'not octet-stream', reason: 'kind' });
      if (s.uploadReplies.length) {
        const r = s.uploadReplies.shift();
        // `raw`: a body that is not JSON (the sidecar's own error page).
        if (r.raw !== undefined) { res.writeHead(r.status, { ...HEADERS, 'Content-Type': 'text/plain' }); return res.end(r.raw); }
        return send(res, r.status, r.body);
      }
      if (!raw.length) return send(res, 400, { ok: false, error: 'empty', reason: 'empty' });
      const id = randomBytes(16).toString('hex');
      const name = cleanName(url.searchParams.get('name') || '');
      s.uploads.set(id, { sandbox: decodeURIComponent(up[1]), name, size: raw.length });
      return send(res, 200, { id, name, size: raw.length, expires_ms: Date.now() + 30 * 60000 });
    }
    const upId = UPLOAD_ID.exec(p);
    if (upId && req.method === 'DELETE') {
      return s.uploads.delete(decodeURIComponent(upId[2])) ? send(res, 200, { ok: true }) : send(res, 404, { ok: false, error: 'not found' });
    }
    if (p === '/r/voice/transcribe') {
      await new Promise(r => setTimeout(r, s.transcribeDelay));
      const r = s.transcribeReplies.shift();
      if (r) return send(res, r.status, r.body);
      return send(res, 200, { text: s.transcript, masked: 0 });
    }
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
