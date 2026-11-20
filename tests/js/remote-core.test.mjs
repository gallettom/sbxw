// The phone page's core (assets/remote/remote-core.js): agent text is data
// (escaped, then a fixed Markdown subset), history pages become chat rows,
// leaders sort with those that need the person first, the pairing states,
// and the one client: right route, once, same origin, never retried.
// Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const read = p => readFileSync(new URL(p, import.meta.url), 'utf8');
const coreSrc = read('../../assets/remote/remote-core.js');
const pageSrc = read('../../assets/remote/remote.js');
const chatSrc = read('../../assets/js/chat-core.js');
const fx = JSON.parse(read('./remote-fixtures.json'));

const NAMES = ['rcAgentHtml', 'rcRows', 'rcMerge', 'rcPage', 'rcLeaders', 'rcStateLabel', 'rcAgo', 'rcPending', 'rcActivity', 'rcPairCode',
  'RC_PAIR_TEXT', 'rcPairState', 'rcDeviceName', 'rcCleanName', 'rcClient', 'rcFailText', 'rcWithDictation', 'chatMarkdown'];
const ctx = { URLSearchParams };
vm.runInNewContext(`${coreSrc}\n${NAMES.map(n => `this.${n} = ${n};`).join('\n')}`, ctx);
const rc = ctx;
// Values made in the vm's realm compare by shape, not by prototype.
const plain = v => JSON.parse(JSON.stringify(v));

// ── The copy ──
const fnSource = (src, name) => {
  const a = src.indexOf(`function ${name}(`);
  assert.ok(a >= 0, `${name} is defined`);
  return src.slice(a, src.indexOf('\n}\n', a) + 2);
};
test('the escaping and Markdown code equals its source in assets/js/chat-core.js', () => {
  for (const name of ['chatEscape', 'chatInline', 'chatMarkdown', 'chatBlock']) {
    assert.equal(fnSource(coreSrc, name), fnSource(chatSrc, name), `${name} drifted: copy it again from chat-core.js`);
  }
});

// ── Hostile strings ──
const ALLOWED = { p: [], br: [], ul: [], ol: ['start'], li: [], strong: [], em: [], code: [], pre: [], a: ['class', 'href', 'target', 'rel'], span: ['class'] };
function assertSafe(html, label) {
  for (const m of html.matchAll(/<\/?([A-Za-z][A-Za-z0-9-]*)([^>]*)>/g)) {
    const tag = m[1].toLowerCase();
    assert.ok(tag in ALLOWED, `${label}: tag <${tag}> in ${html.slice(0, 200)}`);
    for (const a of m[2].matchAll(/([A-Za-z-]+)\s*=\s*"([^"]*)"/g)) {
      assert.ok(ALLOWED[tag].includes(a[1]), `${label}: attribute ${a[1]} on <${tag}>`);
      if (a[1] === 'href') assert.match(a[2], /^https?:\/\//, `${label}: href ${a[2]}`);
    }
    // Every attribute is a quoted pair: nothing left over that a browser
    // could read as one more attribute.
    const rest = m[2].replace(/\s*[A-Za-z-]+\s*=\s*"[^"]*"/g, '').trim();
    assert.equal(rest, '', `${label}: stray attribute text "${rest}"`);
  }
  // In the markup itself (text between tags is escaped and may say anything).
  const tags = [...html.matchAll(/<[^>]*>/g)].join('');
  assert.doesNotMatch(tags, /<script|<img|<iframe|<svg|<style|\son[a-z]+\s*=|javascript:|data:/i, label);
  assert.doesNotMatch(html, /<(?![\/a-z])/i, `${label}: a raw < left`);
}
const HOSTILE = [
  '<script>alert(1)</script>',
  '<img src=x onerror=alert(1)>',
  '"><svg onload=alert(1)>',
  "' onmouseover='alert(1)",
  '[click](javascript:alert(1))',
  '[click](JaVaScRiPt:alert(1))',
  '[x](data:text/html,<script>alert(1)</script>)',
  '[x](https://ok.example/"onmouseover="alert(1))',
  '[x](https://ok.example/a) [y](http://b.example/<b>)',
  '`<b>code</b>` **<i>bold</i>** *<u>em</u>*',
  '```\n</code></pre><script>alert(1)</script>\n```',
  '# <h1>heading</h1>\n- <li>item</li>\n1. <ol>',
  ' 0   0  placeholders forged',
  '&lt;already&gt; &amp; &#39; &quot;',
  '<a href="javascript:alert(1)">a</a>',
  '[**bold label**](https://x.example/?q="1")',
];
test('hostile agent text never becomes markup', () => {
  for (const s of HOSTILE) assertSafe(rc.rcAgentHtml(s), JSON.stringify(s));
});
test('a link shows its address and opens only http(s), in a new context', () => {
  const html = rc.rcAgentHtml('See [the guide](https://example.org/guide?a=1&b=2).');
  assert.match(html, /<a class="rc-link" href="https:\/\/example.org\/guide\?a=1&amp;b=2" target="_blank" rel="noopener noreferrer">the guide <span class="rc-url">https:\/\/example.org\/guide\?a=1&amp;b=2<\/span><\/a>/);
  assert.doesNotMatch(rc.rcAgentHtml('[x](javascript:alert(1))'), /<a /);
  assert.doesNotMatch(rc.rcAgentHtml('a bare https://example.org/x stays text'), /<a /);
});
test('a file path stays plain text (no Files tab on the phone)', () => {
  const html = rc.rcAgentHtml('Edit `src/main.rs:12` and docs/README.md');
  assert.doesNotMatch(html, /<button|data-file/);
  assert.match(html, /<code>src\/main.rs:12<\/code>/);
});
test('20,000 characters render quickly and stay safe', () => {
  for (const s of ['x'.repeat(20000), '*'.repeat(20000), '<'.repeat(20000), '[a]('.repeat(5000), '`*'.repeat(10000), ('**a* '.repeat(4000)), '`'.repeat(20000), '['.repeat(20000)]) {
    const t = Date.now();
    const html = rc.rcAgentHtml(s);
    assert.ok(Date.now() - t < 1500, `took ${Date.now() - t} ms`);
    assertSafe(html, `long ${s.slice(0, 6)}`);
  }
});

// ── Rows from a recorded history ──
test('a recorded history becomes chat rows', () => {
  const rows = plain(rc.rcRows(rc.rcPage(fx.history.shop).items));
  assert.deepEqual(rows.map(r => r.kind), ['note', 'me', 'steps', 'agent', 'sbxw', 'agent', 'steps', 'mac', 'asked']);
  assert.equal(rows[2].count, 3);
  assert.equal(rows[5].from, 'db-schema');
  assert.equal(rows[4].text, 'Thread db-schema reported done.');
  assert.equal(rows[8].text, 'Which database should the shop use for orders?');
});
test('runs of steps fold into one line with their total', () => {
  const rows = plain(rc.rcRows([{ seq: 1, kind: 'steps', count: 2 }, { seq: 2, kind: 'steps', count: 5 }, { seq: 3, kind: 'assistant_text', text: 'ok' }, { seq: 4, kind: 'steps', count: 1 }]));
  assert.deepEqual(rows.map(r => [r.kind, r.count]), [['steps', 7], ['agent', undefined], ['steps', 1]]);
});
test('rows ignore what is not an item, and never trust a field type', () => {
  const rows = rc.rcRows([null, 3, 'x', { kind: 'assistant_text', text: { toString: () => '<b>' } }, { kind: 'unknown', text: 'hi' }, { kind: 'user_prompt', text: '' }]);
  assert.equal(rows.length, 0);
  assert.equal(rc.rcRows('nope').length, 0);
});
test('older and newer pages merge by seq; a provisional reply gives way', () => {
  let items = rc.rcPage(fx.history.shop).items;
  items = rc.rcMerge(items, rc.rcPage(fx.history.shop_older).items);
  assert.equal(items[0].seq, 90);
  items = rc.rcMerge(items, rc.rcPage(fx.history.shop_newer).items);
  assert.equal(items.at(-1).seq, 113);
  items = rc.rcMerge(items, [{ seq: 114, kind: 'assistant_text', text: 'PostgreSQL it is.' }]);
  assert.deepEqual(plain(items.slice(-2).map(i => i.seq)), [112, 114]);
  assert.equal(rc.rcPage(fx.history.shop_newer).next_before, null);
  assert.equal(rc.rcPage(fx.history.shop).next_before, 101);
});

// ── Leaders ──
test('leaders: those that need you first, then working, then idle', () => {
  const l = plain(rc.rcLeaders(fx.leaders));
  assert.deepEqual(l.map(x => x.sandbox), ['shop', 'docs', 'payments-service-with-a-very-long-sandbox-name', 'website']);
  assert.deepEqual(plain(rc.rcLeaders({ leaders: [] })), []);
  assert.deepEqual(plain(rc.rcLeaders({ leaders: [{ name: 'no sandbox' }, null] })), []);
});
test('a question the phone can answer, and one that needs the Mac', () => {
  const [shop, docs] = rc.rcLeaders(fx.leaders);
  assert.equal(rc.rcPending(shop).kind, 'question');
  assert.equal(rc.rcPending(docs).kind, 'mac');
  assert.doesNotMatch(rc.rcPending(docs).text, /Waits for you/, 'the fixed phrase is not repeated in the card');
  assert.equal(rc.rcPending({ state: 'idle', asks: false }), null);
  // Steps without `answerable` are not answered from the phone.
  assert.equal(rc.rcPending({ ...shop, answerable: false }).kind, 'mac');
});
test('the fixed activity phrase, left out where it repeats the state', () => {
  const [shop, docs, pay, web] = rc.rcLeaders(fx.leaders);
  assert.deepEqual([shop, docs, pay, web].map(rc.rcActivity), ['Asks a question', 'Waits for you at the Mac', '', 'Waiting for a message']);
  assert.equal(rc.rcActivity({ state: 'idle', activity: 42 }), '');
});
test('last activity in short words', () => {
  const now = 1_000_000_000;
  assert.deepEqual([0, now - 5e3, now - 4 * 60e3, now - 3 * 3600e3, now - 2 * 86400e3].map(t => rc.rcAgo(t, now)), ['', 'now', '4 min', '3 h', '2 d']);
});

// ── Pairing ──
test('the pairing code comes from the fragment only', () => {
  assert.equal(rc.rcPairCode('#pair=AbC-12_xyz'), 'AbC-12_xyz');
  assert.equal(rc.rcPairCode('#x=1&pair=abcd1234'), 'abcd1234');
  for (const bad of ['', '#', '#abcd1234', '#pair=', '#pair=<script>', '#pair=ab', '#pair=a b c d', `#pair=${'a'.repeat(129)}`]) {
    assert.equal(rc.rcPairCode(bad), null, bad);
  }
});
test('every pairing answer maps to one plain state', () => {
  const r = (status, data) => ({ ok: status >= 200 && status < 300, status, data, unreachable: status === 0 || status === 502 });
  assert.equal(rc.rcPairState(r(200, fx.pair_start), 'start'), 'pending');
  assert.equal(rc.rcPairState(r(403, { error: 'this pairing code is not valid' }), 'start'), 'invalid');
  assert.equal(rc.rcPairState(r(429, null), 'start'), 'limited');
  assert.equal(rc.rcPairState(r(0, null), 'start'), 'unreachable');
  assert.equal(rc.rcPairState(r(502, null), 'start'), 'unreachable');
  for (const s of ['pending', 'refused', 'expired', 'paired']) assert.equal(rc.rcPairState(r(200, { state: s }), 'status'), s);
  assert.equal(rc.rcPairState(r(200, { state: 'weird' }), 'status'), 'failed');
  assert.equal(rc.rcPairState(r(429, null), 'status'), 'limited');
  for (const s of ['refused', 'expired', 'limited', 'invalid', 'unreachable']) assert.ok(rc.RC_PAIR_TEXT[s].length > 10);
  assert.match(rc.RC_PAIR_TEXT.pending, /Compare with the number on your Mac, then confirm there/);
});
test('a device name: prefilled from the platform, cleaned, at most 40 characters', () => {
  assert.equal(rc.rcDeviceName('Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)', 'iPhone'), 'iPhone');
  assert.equal(rc.rcDeviceName('Mozilla/5.0 (Linux; Android 15; Pixel) Mobile', 'Linux'), 'Android phone');
  assert.equal(rc.rcCleanName(`  a\u0000b\n  c ${'x'.repeat(80)}`).length, 40);
  assert.equal(rc.rcCleanName(' my\tphone '), 'my phone');
});

// ── The client ──
function fakeFetch(answer) {
  const calls = [];
  const f = async (path, init) => {
    calls.push({ path, ...init });
    const a = typeof answer === 'function' ? answer(path, init) : answer;
    if (a instanceof Error) throw a;
    return { ok: a.status < 300, status: a.status, json: async () => a.body ?? null };
  };
  return { f, calls };
}
test('each action calls its route once, same origin, JSON, no credential in the URL', async () => {
  const { f, calls } = fakeFetch({ status: 200, body: { ok: true } });
  const api = rc.rcClient(f);
  await api.message('shop', 'hello');
  await api.answer('shop', [2]);
  await api.interrupt('shop');
  await api.leaders();
  await api.history('a b/../c', { before: 101 });
  await api.history('shop', { after: 112, limit: 200 });
  await api.pairStart('CODE1234', 'iPhone');
  await api.pairStatus('0123456789abcdef0123456789abcdef');
  await api.logout();
  await api.me();
  await api.transcribe(new Float32Array(4));
  assert.deepEqual(calls.map(c => `${c.method} ${c.path}`), [
    'POST /r/leaders/shop/message',
    'POST /r/leaders/shop/answer',
    'POST /r/leaders/shop/interrupt',
    'GET /r/leaders',
    'GET /r/leaders/a%20b%2F..%2Fc/history?before=101',
    'GET /r/leaders/shop/history?after=112&limit=200',
    'POST /r/pair/start',
    'GET /r/pair/status?request_id=0123456789abcdef0123456789abcdef',
    'POST /r/logout',
    'GET /r/me',
    'POST /r/voice/transcribe',
  ]);
  for (const c of calls) {
    assert.equal(c.credentials, 'same-origin');
    assert.equal(c.cache, 'no-store');
    assert.ok(c.path.startsWith('/r/'), 'relative to the page origin');
    assert.doesNotMatch(c.path, /token|code=|CODE1234|hello/i);
    if (c.method === 'POST' && c.path !== '/r/voice/transcribe') assert.equal(c.headers['Content-Type'], 'application/json');
  }
  assert.deepEqual(JSON.parse(calls[0].body), { text: 'hello' });
  assert.deepEqual(JSON.parse(calls[1].body), { indices: [2] });
  assert.deepEqual(JSON.parse(calls[6].body), { code: 'CODE1234', device_name: 'iPhone' });
  assert.equal(calls.at(-1).headers['Content-Type'], 'application/octet-stream');
});
test('unreachable: reported once, never retried', async () => {
  const { f, calls } = fakeFetch(new TypeError('Load failed'));
  const seen = [];
  const api = rc.rcClient(f, { reach: ok => seen.push(ok) });
  const res = await api.message('shop', 'keep me');
  assert.equal(res.unreachable, true);
  assert.equal(calls.length, 1);
  assert.deepEqual(seen, [false]);
  assert.match(rc.rcFailText('Message', res), /unreachable \(is the Mac awake\?\)/);
  await new Promise(r => setTimeout(r, 50));
  assert.equal(calls.length, 1, 'nothing queued for later');
  const gw = rc.rcClient(fakeFetch({ status: 502 }).f, { reach: ok => seen.push(ok) });
  assert.equal((await gw.leaders()).unreachable, true);
});
test('a 401 outside pairing means the device is no longer paired', async () => {
  let unpaired = 0;
  const api = rc.rcClient(fakeFetch({ status: 401, body: { error: 'not paired' } }).f, { unpaired: () => unpaired++ });
  await api.leaders();
  assert.equal(unpaired, 1);
  await api.pairStatus('x');
  await api.pairStart('c', 'n');
  assert.equal(unpaired, 1, 'pairing routes do not count');
});
test('only /r/ routes can be called', async () => {
  const api = rc.rcClient(fakeFetch({ status: 200 }).f);
  assert.equal(typeof api.raw, 'undefined');
});
test('dictated words go into the field, never sent', () => {
  assert.equal(rc.rcWithDictation('', ' hello '), 'hello');
  assert.equal(rc.rcWithDictation('Please', 'check'), 'Please check');
  assert.equal(rc.rcWithDictation('Line\n', 'two'), 'Line\ntwo');
  assert.equal(rc.rcWithDictation('kept', '  '), 'kept');
});

// ── What the page may keep ──
test('no storage but the device name, no console, no eval, no innerHTML of raw text', () => {
  // Code only: the comments may name what is not used.
  const code = s => s.replace(/^\s*\/\/.*$/gm, '');
  for (const [name, src] of [['remote-core.js', code(coreSrc)], ['remote.js', code(pageSrc)]]) {
    assert.doesNotMatch(src, /sessionStorage|indexedDB|caches\.|serviceWorker|console\.|eval\(|new Function|document\.write|outerHTML|insertAdjacentHTML/, name);
  }
  assert.doesNotMatch(code(coreSrc), /localStorage/);
  // localStorage: one key, read and written by `prefs` only.
  assert.deepEqual([...pageSrc.matchAll(/localStorage\.(\w+)\(([^,)]*)/g)].map(m => `${m[1]} ${m[2]}`), ['getItem PREFS', 'setItem PREFS']);
  // innerHTML: once, with the core's escaped Markdown.
  assert.deepEqual([...pageSrc.matchAll(/\.innerHTML\s*=\s*([^;]+);/g)].map(m => m[1]), ['rcAgentHtml(text)']);
});
