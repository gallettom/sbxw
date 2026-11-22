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

const NAMES = ['rcAgentHtml', 'RC_PILL_TONES', 'rcPills', 'rcRows', 'rcMerge', 'rcPage', 'rcLeaders', 'rcStateLabel', 'rcAgo', 'rcPending', 'rcAnswerPick', 'rcAnswers', 'rcAnswerFail', 'rcActivity', 'rcPairCode',
  'RC_PAIR_TEXT', 'rcPairState', 'rcDeviceName', 'rcCleanName', 'rcClient', 'rcFailText', 'rcMessageCheck', 'RC_INVISIBLE_RANGES', 'rcAwayFromEnd', 'rcSkillMatches', 'rcSkillInsert', 'rcSkillLine', 'RC_MSG_REASON', 'rcSendGate', 'rcGroups', 'rcStopped', 'rcFold', 'rcWaiting', 'rcToastChoice', 'rcLeaderInfo', 'rcReadOnly', 'rcReadOnlyShort', 'rcWithDictation', 'chatMarkdown',
  'CHAT_SECRET_NAMES', 'chatSecretName', 'chatShortName', 'chatUploadName', 'rcCheckAttach', 'RC_UPLOAD_MAX_FILES', 'RC_UPLOAD_MAX_BYTES', 'RC_UPLOAD_MAX_TOTAL',
  'RC_UPLOAD_REASON', 'rcUploadFail', 'rcUploadDone', 'rcUploadXhr', 'rcTray', 'rcStatus', 'rcLevel', 'rcClock', 'rcWaveMode',
  'rcB64u', 'rcFromB64u', 'rcCreateOptions', 'rcGetOptions', 'rcRegistrationBody', 'rcAssertionBody', 'rcCeremonyError', 'rcApprovalsBlock',
  'rcL2Fail', 'RC_L2_REASON', 'rcActPath', 'rcActLine', 'rcActDone', 'rcL2Client',
  'rcModelChanges', 'rcKbBoard', 'rcKbFirst', 'rcKbTicket', 'rcKbPaths', 'rcInsertAt', 'rcSpeechText', 'rcSpeechLang', 'rcSpeechFallback', 'rcMarked', 'rcActParts',
  'RC_DICT_TEXT', 'rcSpeechEnd', 'rcSpeechRestart', 'RC_SPEECH_RESTARTS', 'rcSpeechRound', 'rcFinalSegments', 'rcTail', 'rcRecMime', 'rcClipCheck', 'rcSilentPcm', 'rcTranscribed', 'rcSendNote', 'RC_QUEUED_TEXT',
  'rcBusy', 'rcMicGate', 'rcMicQuiet', 'RC_MIC_SETTLE_MS', 'RC_MIC_WAIT_MS'];
const ctx = { URLSearchParams, AbortController };
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
test('a multi-select step takes several options, a single-select step one', () => {
  const one = { text: 'Which?', options: ['A', 'B', 'C'] };
  const many = { text: 'Which ones?', options: ['A', 'B', 'C'], multi: true };
  assert.deepEqual(plain(rc.rcAnswerPick([], one, 1)), [1]);
  assert.deepEqual(plain(rc.rcAnswerPick([1], one, 2)), [2], 'a new tap replaces');
  assert.deepEqual(plain(rc.rcAnswerPick([2], many, 0)), [0, 2], 'sorted');
  assert.deepEqual(plain(rc.rcAnswerPick([0, 2], many, 2)), [0], 'a second tap unticks');
  assert.deepEqual(plain(rc.rcAnswers([[0], [1, 2], [1]])), [[1], [2, 3], [2]], '1-based on the wire');
  const leader = { asks: true, answerable: true, steps: [one, many] };
  assert.equal(rc.rcPending(leader).steps[1].multi, true, 'the flag reaches the card');
});
test('an answer typed but not taken says to answer on the Mac', () => {
  const notTaken = { ok: false, status: 409, data: { error: 'not taken: the question is still open at the Mac, answer it there' } };
  assert.match(rc.rcAnswerFail(notTaken), /^Answer not taken: .*Answer it on the Mac\.$/);
  assert.match(rc.rcAnswerFail({ ok: false, status: 409, data: { error: 'two sessions' } }), /^Answer not sent: the session is not waiting/);
  assert.match(rc.rcAnswerFail({ ok: false, status: 0, unreachable: true }), /unreachable/);
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
  assert.match(rc.RC_PAIR_TEXT.pending, /type these six digits/);
  assert.equal(rc.RC_PAIR_TEXT.expired, 'This request expired. Open the new link from your Mac.');
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
  await api.answer('shop', [[2]]);
  await api.interrupt('shop');
  await api.leaders();
  await api.history('a b/../c', { before: 101 });
  await api.history('shop', { after: 112, limit: 200 });
  await api.pairStart('CODE1234', 'iPhone');
  await api.pairStatus('0123456789abcdef0123456789abcdef');
  await api.logout();
  await api.me();
  await api.transcribe(new Float32Array(4));
  await api.message('shop', '', ['0123456789abcdef0123456789abcdef']);
  await api.dropUpload('shop', '0123456789abcdef0123456789abcdef');
  assert.deepEqual(calls.slice(-2).map(c => `${c.method} ${c.path}`), ['POST /r/leaders/shop/message', 'DELETE /r/leaders/shop/upload/0123456789abcdef0123456789abcdef']);
  assert.deepEqual(JSON.parse(calls.at(-2).body), { text: '', attachments: ['0123456789abcdef0123456789abcdef'] }, 'ids only, never a path');
  calls.splice(-2);
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
  assert.deepEqual(JSON.parse(calls[1].body), { answers: [[2]] }, 'per step, the options picked');
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
    assert.doesNotMatch(src, /sessionStorage|indexedDB|caches\.|console\.|eval\(|new Function|document\.write|outerHTML|insertAdjacentHTML/, name);
  }
  assert.doesNotMatch(code(coreSrc), /localStorage/);
  // The one worker (push-page thread): registered only as /sw.js, only from the switch's tap.
  assert.deepEqual([...code(coreSrc).matchAll(/\.register\(([^,)]*)/g)].map(m => m[1]), ["'/sw.js'"]);
  assert.doesNotMatch(code(pageSrc), /\.register\(/);
  // localStorage: one key, read and written by `prefs` only.
  assert.deepEqual([...pageSrc.matchAll(/localStorage\.(\w+)\(([^,)]*)/g)].map(m => `${m[1]} ${m[2]}`), ['getItem PREFS', 'setItem PREFS']);
  // innerHTML: once, with the core's escaped Markdown.
  assert.deepEqual([...pageSrc.matchAll(/\.innerHTML\s*=\s*([^;]+);/g)].map(m => m[1]), ['rcAgentHtml(text)']);
});

// ── Message refusals (API.md v1.2, 2.6) ──
test('each refusal reason has its own sentence, with the daemon\'s words as text', () => {
  const r = (status, reason, error = 'the daemon said <b>so</b>') => ({ ok: false, status, unreachable: false, data: { ok: false, reason, error } });
  const want = {
    command: [400, /Commands cannot be sent from the phone/],
    control: [400, /control or invisible character/],
    empty: [400, /empty/],
    needs_mac: [409, /This needs your Mac/],
    busy: [409, /The session is busy right now: try again in a few seconds/],
    no_paste: [409, /Send it from your Mac/],
    not_submitted: [409, /NOT SENT: the text was typed into the session's input on the Mac but not submitted/],
    attachment: [400, /a file of this message is no longer on the Mac.*choose it again/],
    too_many: [400, /a message takes 5 files at most/],
  };
  for (const [reason, [status, words]] of Object.entries(want)) {
    const s = rc.rcFailText('Message', r(status, reason));
    assert.match(s, words, reason);
    assert.match(s, /\(sbxw: the daemon said <b>so<\/b>\)$/, 'the error is kept, as text (set with textContent)');
  }
  assert.deepEqual(Object.keys(rc.RC_MSG_REASON).sort(), Object.keys(want).sort());
  // An unknown reason falls back to the generic sentence, never to its own name.
  assert.match(rc.rcFailText('Message', r(409, '__proto__')), /^Message not sent:/);
  assert.match(rc.rcFailText('Message', r(409, 'toString')), /^Message not sent:/);
});
test('the check before sending matches the daemon\'s rule', () => {
  for (const s of ['/model', '!ls', '# remember', 'ok\n  /clear', 'a\n\t!x']) assert.equal(rc.rcMessageCheck(s), 'command', JSON.stringify(s));
  for (const s of ['a\tb', 'a\rb', 'a\u001b[2J', 'a\u007f', 'a\u0085', 'a\u202eb', 'a\u2066b',
    'a\u200bb', 'a\u200cb', 'a\u200db', '\ufeffa', 'soft\u00adhyphen', 'a\u2060b', 'a\u2028b', 'a\u2029b', 'tag\u{E0041}']) {
    assert.equal(rc.rcMessageCheck(s), 'control', JSON.stringify(s));
  }
  assert.match(rc.RC_MSG_REASON.control, /Remove the invisible characters, or retype the text/);
  assert.equal(rc.RC_MSG_REASON.busy, 'The session is busy right now: try again in a few seconds. Meanwhile you can read it and interrupt it.');
  for (const s of ['', '   ', '\n\n']) assert.equal(rc.rcMessageCheck(s), 'empty');
  for (const s of ['hello', 'café ünïcödé 日本語 😀', 'a/b and c#d', 'line one\nline two', 'see https://x.example/#a', 'Fix it! Now.']) assert.equal(rc.rcMessageCheck(s), null, JSON.stringify(s));
});

test('the phone refuses the one invisible list of crates/core/src/invisible.rs', () => {
  const rs = read('../../crates/core/src/invisible.rs');
  const body = /pub const INVISIBLE[^=]*=\s*&\[([\s\S]*?)\n\];/.exec(rs)[1];
  const want = [...body.matchAll(/\('\\u\{([0-9a-f]+)\}',\s*'\\u\{([0-9a-f]+)\}'\)/g)].map(m => [parseInt(m[1], 16), parseInt(m[2], 16)]);
  assert.ok(want.length >= 20);
  assert.deepEqual(plain(rc.RC_INVISIBLE_RANGES), want);
  const inList = n => want.some(([a, b]) => n >= a && n <= b);
  const cf = /[\p{Cf}\p{Zl}\p{Zp}]/u;
  for (let n = 0; n <= 0x10FFFF; n++) {
    if (n >= 0xD800 && n <= 0xDFFF) continue;
    const ch = String.fromCodePoint(n);
    // Never looser than before: every format character, separator and control.
    if (cf.test(ch)) assert.ok(inList(n), `U+${n.toString(16)} is a format character missing from the list`);
    const control = /\p{Cc}/u.test(ch) && ch !== '\n';
    assert.equal(rc.rcMessageCheck(`a${ch}b`), inList(n) || control ? 'control' : null, `U+${n.toString(16)}`);
  }
});

test('the jump button shows only when the log is away from its end', () => {
  assert.equal(rc.rcAwayFromEnd(2000, 1200, 800), false, 'at the end');
  assert.equal(rc.rcAwayFromEnd(2000, 1121, 800), false, 'within 80 px');
  assert.equal(rc.rcAwayFromEnd(2000, 1100, 800), true);
  assert.equal(rc.rcAwayFromEnd(500, 0, 800), false, 'a short log');
  assert.equal(rc.rcAwayFromEnd(2000, 0, 800, 200), true);
  assert.equal(rc.rcAwayFromEnd(undefined, 0, 0), false);
});

test('the "/" autocomplete filters, inserts, and routes a known skill apart from an unknown line', () => {
  const sk = ['code-review', 'codemap', 'retro', 'grill-me'];
  assert.deepEqual(plain(rc.rcSkillMatches('/', sk)), sk);
  assert.deepEqual(plain(rc.rcSkillMatches('/co', sk)), ['code-review', 'codemap']);
  assert.deepEqual(plain(rc.rcSkillMatches('/ME', sk)), ['grill-me']);
  assert.deepEqual(plain(rc.rcSkillMatches('/map', sk)), ['codemap'], 'a name that holds it comes after prefixes');
  for (const t of ['', 'co', 'hi /co', '/retro ', '/retro x', '/x y', '/a\nb', null]) assert.deepEqual(plain(rc.rcSkillMatches(t, sk)), [], JSON.stringify(t));
  assert.equal(rc.rcSkillInsert('retro'), '/retro ');
  assert.deepEqual(plain(rc.rcSkillLine('/retro', sk)), { name: 'retro', args: '' });
  assert.deepEqual(plain(rc.rcSkillLine('  /retro  last week ', sk)), { name: 'retro', args: 'last week' });
  for (const t of ['/x', '/x args', '/retro\nmore', '/Retro', 'retro', '//retro', '/', '/retro-x']) assert.equal(rc.rcSkillLine(t, sk), null, JSON.stringify(t));
  // The typing check: a known skill is no command; an unknown line still is.
  assert.equal(rc.rcMessageCheck('/retro now', sk), null);
  assert.equal(rc.rcMessageCheck('/retro', sk), null);
  assert.equal(rc.rcMessageCheck('/x now', sk), 'command');
  assert.equal(rc.rcMessageCheck('/retro now', []), 'command');
  assert.equal(rc.rcMessageCheck('/retro now'), 'command');
  assert.equal(rc.rcMessageCheck('/retro a\tb', sk), 'control');
  assert.equal(rc.rcMessageCheck('/retro\n/clear', sk), 'command');
});

test('Send goes while the session works, and the status line says Claude Code queues it', () => {
  assert.match(rc.rcSendGate(null), /not running/);
  for (const l of [{ state: 'idle' }, { state: 'working' }, { state: 'attention', asks: true }]) assert.equal(rc.rcSendGate(l), '', JSON.stringify(l));
  assert.equal(rc.rcSendNote({ state: 'working' }), rc.RC_QUEUED_TEXT);
  assert.match(rc.RC_QUEUED_TEXT, /queued by Claude Code/);
  for (const l of [null, { state: 'idle' }, { state: 'attention', asks: true }, { state: 'working', read_only: 'no_terminal' }]) assert.equal(rc.rcSendNote(l), '', JSON.stringify(l));
  const s = o => plain(rc.rcStatus(o));
  assert.deepEqual(s({ queued: rc.RC_QUEUED_TEXT }), { text: rc.RC_QUEUED_TEXT, tone: 'info' });
  assert.equal(s({ queued: rc.RC_QUEUED_TEXT, note: { text: 'Sent.', tone: 'ok' } }).text, 'Sent.', 'a note wins');
  assert.equal(s({ queued: rc.RC_QUEUED_TEXT, gate: 'Read only.' }).text, 'Read only.', 'a gate wins');
});

// ── Files (API.md 2.6.1) ──
const chatOnly = n => fnSource(chatSrc, n);
test('the attach rules equal their source in assets/js/chat-core.js', () => {
  for (const name of ['chatSecretName', 'chatSize', 'chatShortName', 'chatUploadName']) {
    assert.equal(fnSource(coreSrc, name), chatOnly(name), `${name} drifted: copy it again from chat-core.js`);
  }
  const list = src => /const CHAT_SECRET_NAMES = (\[[^\]]*\]);/.exec(src)[1];
  assert.equal(list(coreSrc), list(chatSrc), 'the secret-looking names drifted: copy the list again');
});
const F = (name, size) => ({ name, size });
test('the checks before any byte: empty, size, count, total, secret-looking names', () => {
  const MB = 1024 * 1024;
  const r = rc.rcCheckAttach(0, [F('a.png', 10), F('empty.txt', 0), F('big.mov', 26 * MB), F('.env.local', 5), F('key.PEM', 9)]);
  assert.deepEqual(plain(r.take.map(f => f.name)), ['a.png', '.env.local', 'key.PEM']);
  assert.deepEqual(plain(r.refused), [{ name: 'empty.txt', why: 'the file is empty' }, { name: 'big.mov', why: 'over 25 MB, the most one file takes' }]);
  assert.equal(r.ask.length, 2);
  assert.match(r.ask[0], /^\.env\.local: its name looks like a secret/);
  // Five at most, with what is already held.
  const five = rc.rcCheckAttach(3, [F('1', 1), F('2', 1), F('3', 1)]);
  assert.equal(five.take.length, 2);
  assert.match(five.refused[0].why, /5 files at most/);
  // 60 MB together.
  const total = rc.rcCheckAttach(0, [F('a', 24 * MB), F('b', 24 * MB), F('c', 24 * MB)]);
  assert.equal(total.take.length, 2);
  assert.match(total.refused[0].why, /60 MB together/);
  assert.equal(rc.rcCheckAttach(1, [F('d', 30 * MB)], 40 * MB).refused.length, 1);
  for (const n of ['.env', 'id_rsa.pub', 'credentials.json', '.netrc', 'x.kdbx', 'y.p12']) assert.ok(rc.chatSecretName(n), n);
  for (const n of ['photo.jpg', 'environment.md', 'notes.txt']) assert.ok(!rc.chatSecretName(n), n);
  assert.equal(rc.rcCheckAttach(0, [null, 3, 'x']).take.length, 0, 'not a file: ignored');
});

test('each upload refusal has its own sentence', () => {
  const r = (status, reason) => ({ ok: false, status, data: { ok: false, reason, error: '<b>x</b>' } });
  const want = { too_large: [413, /25 MB at most/], too_many: [409, /5 at most, 60 MB together/], empty: [400, /empty/], kind: [400, /Programs and archives/],
    name: [400, /could not read this file's name/], busy: [409, /Another upload from this phone/], timeout: [408, /^The connection was too slow for this file\./] };
  for (const [reason, [status, words]] of Object.entries(want)) assert.match(rc.rcUploadFail(r(status, reason)), words, reason);
  assert.deepEqual(Object.keys(rc.RC_UPLOAD_REASON).sort(), Object.keys(want).sort());
  // A body cut on a slow link: no answer, the sidecar's error (no JSON), the daemon's 400.
  const SLOW = 'The connection was too slow for this file. Try again on a better connection, or send a smaller file.';
  assert.equal(rc.rcUploadFail({ ok: false, status: 0, unreachable: true, data: null }), SLOW);
  assert.equal(rc.rcUploadFail({ ok: false, status: 502, unreachable: true, data: null }), SLOW);
  assert.equal(rc.rcUploadFail({ ok: false, status: 500, data: null }), SLOW);
  assert.equal(rc.rcUploadFail({ ok: false, status: 400, data: { ok: false, error: 'the upload was cut off' } }), SLOW);
  assert.equal(rc.rcUploadFail(null), SLOW);
  assert.match(rc.rcUploadFail(r(413, 'nope')), /25 MB at most/);
  assert.match(rc.rcUploadFail(r(429)), /too many uploads/);
  assert.match(rc.rcUploadFail(r(500, '__proto__')), /^Not uploaded: the Mac answered 500\.$/);
  // The answer: an id of 32 hex, never a path; anything else is no upload.
  assert.deepEqual(plain(rc.rcUploadDone({ id: '0123456789abcdef0123456789abcdef', name: 'photo.jpg', size: 3, path: '/tmp/x' })), { id: '0123456789abcdef0123456789abcdef', name: 'photo.jpg', size: 3 });
  for (const d of [null, {}, { id: '../../etc' }, { id: 7 }, { id: '0123456789ABCDEF0123456789ABCDEF' }, { id: '/tmp/sbxw-uploads/1/a' }]) assert.equal(rc.rcUploadDone(d), null, JSON.stringify(d));
});

function fakeXhr(reply) {
  const made = [];
  class X {
    constructor() { this.headers = {}; this.upload = {}; made.push(this); }
    open(m, u) { this.method = m; this.url = u; }
    setRequestHeader(k, v) { this.headers[k] = v; }
    abort() { this.aborted = true; this.onabort?.(); }
    send(body) {
      this.body = body;
      if (reply === 'hang') return;
      queueMicrotask(() => {
        this.upload.onprogress?.({ lengthComputable: true, loaded: 1, total: 2 });
        if (reply === 'error') return this.onerror();
        this.status = reply.status;
        this.responseText = JSON.stringify(reply.body);
        this.onload();
      });
    }
  }
  return { X, made };
}
test('an upload: the raw file, same origin, the name encoded, progress, no path', async () => {
  const { X, made } = fakeXhr({ status: 200, body: { id: '0123456789abcdef0123456789abcdef', name: 'a_b.png', size: 2 } });
  const seen = [];
  const file = { name: '../../a b<img>.png', size: 2 };
  const res = await rc.rcUploadXhr(X, 'my shop', file, p => seen.push(p));
  assert.equal(made[0].method, 'POST');
  assert.equal(made[0].url, '/r/leaders/my%20shop/upload?name=a%20b%3Cimg%3E.png', 'the last component only, encoded');
  assert.equal(made[0].headers['Content-Type'], 'application/octet-stream');
  assert.equal(made[0].body, file, 'the file itself is the body; its content is never read');
  assert.deepEqual(seen, [0.5]);
  assert.equal(res.ok, true);
  for (const name of ['\u202egnp.exe', 'x\u0000y\nz', 'a'.repeat(300) + '.jpeg', '..', 'C:\\Users\\me\\secret.txt']) {
    const { X: X2, made: m2 } = fakeXhr({ status: 200, body: {} });
    await rc.rcUploadXhr(X2, 's', { name, size: 1 }, () => {});
    const q = new URL(m2[0].url, 'https://x.example').searchParams.get('name');
    assert.doesNotMatch(q, /[\u0000-\u001f\/\\]/, JSON.stringify(name));
    assert.ok(q.length <= 100, 'kept short');
  }
  const { X: X3 } = fakeXhr('error');
  assert.equal((await rc.rcUploadXhr(X3, 's', file, () => {})).unreachable, true);
});

test('the tray: one upload at a time, the tick, failure and retry, remove, the ids', async () => {
  const ID = n => String(n).padStart(32, '0');
  const answers = [];
  let n = 0;
  const started = [];
  const dropped = [];
  const upload = (file, onProgress) => {
    started.push(file.name);
    return new Promise(r => answers.push(v => { onProgress(1); r(v); }));
  };
  const t = rc.rcTray({ upload, drop: id => dropped.push(id) });
  const flush = () => new Promise(r => setTimeout(r, 0));
  t.offer([F('a.png', 1), F('b.pdf', 2), F('empty', 0)]);
  await flush();
  assert.match(t.note, /^Not attached: empty \(the file is empty\)\.$/);
  assert.deepEqual(plain(started), ['a.png'], 'one at a time');
  assert.equal(t.pending(), 2);
  answers.shift()({ ok: true, status: 200, data: { id: ID(++n), name: 'a.png', size: 1 } });
  await flush(); await flush();
  assert.deepEqual(plain(started), ['a.png', 'b.pdf']);
  answers.shift()({ ok: false, status: 400, data: { reason: 'kind' } });
  await flush(); await flush();
  assert.equal(t.failed(), 1);
  assert.match(t.chips[1].error, /Programs and archives/);
  assert.deepEqual(plain(t.ids()), [ID(1)], 'a failed file is not sent, and blocks nothing');
  t.retry(t.chips[1].key);
  await flush();
  answers.shift()({ ok: true, status: 200, data: { id: ID(++n), name: 'b.pdf', size: 2 } });
  await flush(); await flush();
  assert.deepEqual(plain(t.ids()), [ID(1), ID(2)]);
  // A secret-looking name waits for the tick: nothing is uploaded before.
  t.offer([F('.env', 4)]);
  assert.ok(t.held);
  assert.equal(started.length, 3);
  assert.equal(t.confirmHeld(false), false);
  assert.match(t.note, /^Tick "I have looked at these files and want to attach them" first/);
  assert.equal(started.length, 3);
  assert.equal(t.confirmHeld(true), true);
  await flush();
  assert.equal(started.at(-1), '.env');
  // Removing an uploading file aborts it; removing a done one tells the Mac.
  t.remove(t.chips.at(-1).key);
  t.remove(t.chips[0].key);
  assert.deepEqual(plain(dropped), [ID(1)]);
  assert.deepEqual(plain(t.ids()), [ID(2)]);
  t.sent([ID(2)]);
  assert.equal(t.chips.length, 0);
  // A message's worth: five waiting at most.
  t.offer([1, 2, 3, 4, 5, 6].map(i => F(`f${i}`, 1)));
  assert.equal(t.chips.length, 5);
  assert.match(t.note, /f6 \(a message takes 5 files at most\)/);
  t.clear();
  assert.equal(t.chips.length, 0);
});

test('the status line: one sentence, the most urgent first', () => {
  const s = o => plain(rc.rcStatus(o));
  assert.deepEqual(s({}), { text: '', tone: 'info' });
  assert.deepEqual(s({ hint: 'Added to the field: read it, then Send.' }), { text: 'Added to the field: read it, then Send.', tone: 'info' });
  assert.deepEqual(s({ gate: 'The leader is working: Send comes back at its prompt.', hint: 'x' }), { text: 'The leader is working: Send comes back at its prompt.', tone: 'info' });
  assert.equal(s({ gate: 'The leader is working: Send comes back at its prompt.', files: 2 }).text, 'The leader is working: Send comes back at its prompt. Your files wait here.');
  assert.deepEqual(s({ pending: 2, gate: 'g' }), { text: 'Uploading 2 files: Send waits until they are done.', tone: 'busy' });
  assert.equal(s({ pending: 1 }).text, 'Uploading 1 file: Send waits until it is done.');
  assert.match(s({ held: 1, pending: 1 }).text, /^Tick the box/);
  assert.equal(s({ failed: 1 }).text, 'An upload failed: retry or remove. The rest can be sent.');
  assert.match(s({ rec: 'recording', held: 1 }).text, /^Recording\. .*nothing is sent\.$/);
  assert.equal(s({ rec: 'transcribing' }).text, 'Transcribing on your Mac…');
  // The last action's outcome wins over everything.
  const busy = rc.rcFailText('Message', { ok: false, status: 409, data: { reason: 'busy' } });
  assert.deepEqual(s({ note: { text: busy, tone: 'err' }, rec: 'recording', pending: 1 }), { text: busy, tone: 'err' });
  for (const reason of Object.keys(rc.RC_MSG_REASON)) {
    const text = rc.rcFailText('Message', { ok: false, status: 409, data: { reason } });
    assert.equal(s({ note: { text, tone: 'err' } }).text, rc.RC_MSG_REASON[reason], reason);
  }
});

test('the recorder: a level from samples, a clock, a wave that degrades', () => {
  const frame = f => Uint8Array.from({ length: 512 }, (_, i) => f(i));
  assert.equal(rc.rcLevel(frame(() => 128)), 0, 'silence is flat');
  assert.equal(rc.rcLevel(frame(i => 128 + (i % 2 ? 1 : 0))), 0, 'noise floor is flat');
  const quiet = rc.rcLevel(frame(i => 128 + Math.round(10 * Math.sin(i / 5))));
  const loud = rc.rcLevel(frame(i => 128 + Math.round(90 * Math.sin(i / 5))));
  assert.ok(quiet > 0 && loud > quiet && loud <= 1, `${quiet} ${loud}`);
  assert.equal(rc.rcLevel(frame(i => (i % 2 ? 255 : 0))), 1);
  assert.equal(rc.rcLevel(null), 0);
  assert.deepEqual([0, 7, 59.9, 60, 102, -3, 'x'].map(rc.rcClock), ['0:00', '0:07', '0:59', '1:00', '1:42', '0:00', '0:00']);
  assert.equal(rc.rcWaveMode(true, false), 'bars');
  assert.equal(rc.rcWaveMode(true, true), 'meter');
  assert.equal(rc.rcWaveMode(false, false), 'pulse');
  assert.equal(rc.rcWaveMode(false, true), 'pulse');
});

// ── Level 2 (API-level2.md) ──
test('base64url without padding, strict both ways', () => {
  for (let n = 0; n < 40; n++) {
    const b = Uint8Array.from({ length: n }, (_, i) => (i * 37 + n) & 255);
    const e = rc.rcB64u(b);
    assert.equal(e, Buffer.from(b).toString('base64url'), `length ${n}`);
    assert.deepEqual([...rc.rcFromB64u(e)], [...b]);
  }
  assert.equal(rc.rcB64u(new Uint8Array([251, 255]).buffer), '-_8');
  for (const bad of ['a', 'ab==', 'a+b/', 'ab cd', 'é', 7, null, 'abcde']) assert.equal(rc.rcFromB64u(bad), null, JSON.stringify(bad));
});
test('ceremony options: only the contract\'s fields, user verification always required', () => {
  const fx2 = fx.level2;
  const c = rc.rcCreateOptions({ ...fx2.passkey_options.public_key, authenticatorSelection: { userVerification: 'discouraged' }, attestation: 'direct', extensions: { x: 1 } });
  assert.equal(c.authenticatorSelection.userVerification, 'required');
  assert.equal(c.attestation, 'none');
  assert.equal(c.extensions, undefined);
  assert.deepEqual([...c.challenge], [...Array(32).keys()]);
  assert.deepEqual(plain(c.pubKeyCredParams), [{ type: 'public-key', alg: -7 }]);
  for (const bad of [null, {}, { ...fx2.passkey_options.public_key, challenge: 'ab==' }, { ...fx2.passkey_options.public_key, pubKeyCredParams: [{ type: 'public-key', alg: -257 }] }, { ...fx2.passkey_options.public_key, rp: {} }]) {
    assert.equal(rc.rcCreateOptions(bad), null);
  }
  const g = rc.rcGetOptions({ ...fx2.challenge.public_key, userVerification: 'preferred' });
  assert.equal(g.userVerification, 'required');
  assert.equal(g.allowCredentials.length, 1);
  assert.equal(rc.rcGetOptions({ ...fx2.challenge.public_key, allowCredentials: [] }), null, 'never "any passkey"');
  assert.equal(rc.rcGetOptions({ ...fx2.challenge.public_key, challenge: 'AA' }), null, 'a short challenge');
  // The browser's answers: exact shapes, else nothing is sent.
  const B = t => new TextEncoder().encode(t).buffer;
  assert.deepEqual(plain(rc.rcAssertionBody({ type: 'public-key', rawId: B('id'), response: { clientDataJSON: B('c'), authenticatorData: B('a'), signature: B('s'), userHandle: B('u') } })),
    { id: 'aWQ', client_data_json: 'Yw', authenticator_data: 'YQ', signature: 'cw' });
  for (const bad of [null, {}, { type: 'x', rawId: B('i'), response: {} }, { type: 'public-key', rawId: 'id', response: { clientDataJSON: B('c'), authenticatorData: B('a'), signature: B('s') } }, { type: 'public-key', rawId: B('i'), response: { clientDataJSON: B('c'), authenticatorData: B('a') } }]) {
    assert.equal(rc.rcAssertionBody(bad), null);
  }
  assert.equal(rc.rcRegistrationBody({ type: 'public-key', rawId: B('i'), response: { clientDataJSON: B('c') } }), null);
  assert.equal(rc.rcCeremonyError({ name: 'NotAllowedError' }), 'cancelled');
  assert.equal(rc.rcCeremonyError({ name: 'AbortError' }), 'cancelled');
  assert.equal(rc.rcCeremonyError({ name: 'NotSupportedError' }), 'no_authenticator');
  assert.equal(rc.rcCeremonyError(new Error('x')), 'failed');
});
test('level 2: the entry\'s sentence, each refusal, the routes, the sheet\'s line, the result', async () => {
  assert.equal(rc.rcApprovalsBlock(null), 'Approving is turned off on your Mac.');
  assert.equal(rc.rcApprovalsBlock({ level2: false }), 'Approving is turned off on your Mac.');
  assert.equal(rc.rcApprovalsBlock({ level2: true, passkey: false }), 'Approving needs Face ID: enable it from your Mac.');
  assert.match(rc.rcApprovalsBlock({ level2: true, passkey: true, rp_ok: false }), /tailnet name changed/);
  assert.equal(rc.rcApprovalsBlock({ level2: true, passkey: true, rp_ok: true }), '');
  const r = (status, reason, error) => ({ ok: false, status, data: { ok: false, reason, error } });
  assert.equal(rc.rcL2Fail(r(409, 'changed')), 'This changed since you opened it: read it again.');
  assert.match(rc.rcL2Fail(r(409, 'refused', 'a <b>conflict</b>')), /^The Mac refused it: a <b>conflict<\/b>$/);
  for (const k of ['level2_off', 'no_passkey', 'passkey', 'no_grant', 'not_found', 'unavailable', 'bad', 'limited']) assert.equal(rc.rcL2Fail(r(403, k)), rc.RC_L2_REASON[k]);
  assert.deepEqual(Object.keys(rc.RC_L2_REASON).sort(), ['bad', 'changed', 'level2_off', 'limited', 'no_grant', 'no_passkey', 'not_found', 'passkey', 'refused', 'unavailable']);
  assert.match(rc.rcL2Fail(r(429)), /Too many failed tries/);
  assert.match(rc.rcL2Fail({ ok: false, status: 0, unreachable: true }), /unreachable/);
  assert.match(rc.rcL2Fail(r(500, '__proto__')), /^Nothing was done: the Mac answered 500/);
  assert.equal(rc.rcActPath('plan_approve', 'p-1'), '/r/plans/p-1/approve');
  assert.equal(rc.rcActPath('network_respond', 'network:a/b'), '/r/network/network%3Aa%2Fb/respond');
  assert.equal(rc.rcActPath('sandbox_remove', '../x'), '/r/stopped/..%2Fx/remove');
  assert.equal(rc.rcActPath('shutdown', 'x'), null);
  assert.equal(rc.rcActLine('sandbox_remove', { name: 'old' }), 'Remove the stopped sandbox old. This cannot be undone.');
  assert.equal(rc.rcActLine('network_respond', { sandbox: 'docs', host: 'x.org' }, { id: 'dismiss', label: 'dismiss' }), 'Answer dismiss to the request of docs for x.org.');
  assert.equal(rc.rcActDone('merge', fx.level2.merge_done), 'Merged: footer, contact-form.');
  assert.equal(rc.rcActDone('merge', fx.level2.merge_conflict), 'Stopped at contact-form: conflict in src/pages/contact.tsx, src/components/Footer.tsx');
  assert.equal(rc.rcActDone('merge', null), 'Merged.');
  // The client: same origin, JSON, the option only for a network answer.
  const { f, calls } = fakeFetch({ status: 200, body: { ok: true } });
  const l2 = rc.rcL2Client(rc.rcClient(f));
  await l2.challenge('plan_approve', 'p-1', 'a'.repeat(64));
  await l2.challenge('network_respond', 'network:x', 'b'.repeat(64), 'dismiss');
  await l2.act('network_respond', 'network:x', { id: 'i' }, 'dismiss');
  await l2.act('merge', 'p-2', { id: 'i' }, 'ignored');
  await l2.challenge('plan_approve', 'p-3', 'c'.repeat(64), undefined, { alpha: 'haiku' });
  await l2.act('plan_approve', 'p-3', { id: 'i' }, undefined, { alpha: 'haiku' });
  await l2.challenge('plan_refuse', 'p-3', 'c'.repeat(64), undefined, { alpha: 'haiku' });
  await l2.act('plan_approve', 'p-3', { id: 'i' }, undefined, {});
  const RID = '0123456789abcdef0123456789abcdef';
  await l2.passkeyStatus(RID);
  await l2.passkeyOptions(RID);
  await l2.passkeyRegister(RID, { id: 'a', client_data_json: 'b', attestation_object: 'c' });
  const pk = calls.splice(-3);
  assert.equal(pk[0].path, `/r/passkey/status?request_id=${RID}`);
  assert.deepEqual(JSON.parse(pk[1].body), { request_id: RID });
  assert.deepEqual(JSON.parse(pk[2].body), { request_id: RID, id: 'a', client_data_json: 'b', attestation_object: 'c' });
  assert.deepEqual(calls.map(c => [c.path, JSON.parse(c.body)]), [
    ['/r/approvals/challenge', { kind: 'plan_approve', target: 'p-1', digest: 'a'.repeat(64) }],
    ['/r/approvals/challenge', { kind: 'network_respond', target: 'network:x', digest: 'b'.repeat(64), option: 'dismiss' }],
    ['/r/network/network%3Ax/respond', { option: 'dismiss', assertion: { id: 'i' } }],
    ['/r/merges/p-2/merge', { assertion: { id: 'i' } }],
    ['/r/approvals/challenge', { kind: 'plan_approve', target: 'p-3', digest: 'c'.repeat(64), models: { alpha: 'haiku' } }],
    ['/r/plans/p-3/approve', { assertion: { id: 'i' }, models: { alpha: 'haiku' } }],
    ['/r/approvals/challenge', { kind: 'plan_refuse', target: 'p-3', digest: 'c'.repeat(64) }],
    ['/r/plans/p-3/approve', { assertion: { id: 'i' } }],
  ]);
  assert.throws(() => l2.act('nope', 'x', {}));
});

// ── The Kanban (API-kanban.md) ──
test('the board is drawn as it comes: the daemon\'s order, fields checked, active first', () => {
  const b = plain(rc.rcKbBoard(fx.kanban.board));
  assert.deepEqual(b.columns.map(c => c.id), ['planned', 'progress', 'test', 'ready', 'merged']);
  assert.deepEqual(b.columns[1].cards.map(c => c.key), ['t:12', 'th:checkout']);
  assert.equal(b.columns[4].older, 2);
  assert.equal(b.columns[1].cards[1].n, null, 'a thread\'s own card');
  assert.equal(b.columns[1].cards[1].num, 'thread');
  assert.equal(rc.rcKbFirst(rc.rcKbBoard(fx.kanban.board)), 'progress');
  assert.equal(rc.rcKbFirst(rc.rcKbBoard(fx.kanban.empty)), 'planned');
  const odd = plain(rc.rcKbBoard({ columns: [null, { id: 'planned', cards: [null, { key: 'k', n: '12', title: { x: 1 }, marks: [{ label: 'x', tone: 'javascript' }, 3], priority: 'p0', thread_tone: '"><' }] }] }));
  assert.deepEqual(odd.columns[0].cards[0], { key: 'k', n: null, num: 'thread', title: '', priority: 'none', thread: null, thread_state: null, thread_tone: 'neutral', marks: [{ label: 'x', tone: 'neutral' }], files: 0, age: '' });
  assert.deepEqual(plain(rc.rcKbBoard(null)), { latest: '', summary: '', columns: [] });
  const t = plain(rc.rcKbTicket(fx.kanban.ticket));
  assert.match(t.description, /\n\nThe migration must be reversible; see \[path\]/);
  assert.deepEqual(t.files, [{ name: 'schema-sketch.png', size: 41234, kind: 'png' }, { name: 'notes.txt', size: 812, kind: 'text' }]);
  assert.equal(rc.rcKbTicket({}), null);
  assert.equal(rc.rcKbPaths('my shop'), '/r/leaders/my%20shop/kanban');
  assert.equal(rc.rcKbPaths('shop', 12), '/r/leaders/shop/kanban/T-12');
  for (const n of [null, 0, -1, 1.5, '12', 1e9]) assert.equal(rc.rcKbPaths('shop', n), null, String(n));
});

// ── iPhone dictation ──
test('iPhone dictation: words at the caret, final and interim apart, the language, which errors fall back', () => {
  const at = (t, c, w) => plain(rc.rcInsertAt(t, c, w));
  assert.deepEqual(at('', 0, 'hello'), { value: 'hello', caret: 5 });
  assert.deepEqual(at('Hello world', 5, 'big'), { value: 'Hello big world', caret: 9 });
  assert.deepEqual(at('Hello ', 6, ' there '), { value: 'Hello there', caret: 11 });
  assert.deepEqual(at('ab', 99, 'c'), { value: 'ab c', caret: 4 }, 'a caret out of range: the end');
  assert.deepEqual(at('ab', 1, '  '), { value: 'ab', caret: 1 });
  const R = list => Object.assign(list.map(([t, f]) => ({ 0: { transcript: t }, isFinal: f })), {});
  assert.deepEqual(plain(rc.rcSpeechText(R([['one ', true], [' two', true], ['thr', false]]))), { final: 'one two', interim: 'thr' });
  assert.deepEqual(plain(rc.rcSpeechText(null)), { final: '', interim: '' });
  assert.deepEqual(plain(rc.rcSpeechText([{ 0: { transcript: 7 }, isFinal: true }])), { final: '', interim: '' });
  assert.equal(rc.rcSpeechLang('fr-FR', 'en'), 'fr-FR');
  assert.equal(rc.rcSpeechLang('', 'en'), 'en');
  assert.equal(rc.rcSpeechLang('"><x', '!!'), 'en-US');
  for (const e of ['not-allowed', 'service-not-allowed', 'network', 'audio-capture']) assert.equal(rc.rcSpeechFallback(e), true, e);
  for (const e of ['no-speech', 'aborted', 'language-not-supported', '', undefined]) assert.equal(rc.rcSpeechFallback(e), false, String(e));
  assert.match(plain(rc.rcStatus({ rec: 'listening' })).text, /^Listening \(iPhone dictation\)\. .*nothing is sent\.$/);
});

// ── Dictation that never stops or stays deaf in silence ──
const SR = list => list.map(([t, f]) => ({ 0: { transcript: t }, isFinal: f }));
test('dictation preview: the END of a long text, older words cut at the start with an ellipsis', () => {
  const long = Array.from({ length: 60 }, (_, i) => `word${i}`).join(' ');
  const t = plain(rc.rcTail(long, 'and the newest interim', 80));
  assert.equal(t.cut, true);
  assert.equal(t.interim, 'and the newest interim', 'the interim, newest, is whole');
  assert.ok(t.final.startsWith('…'), t.final);
  assert.ok(t.final.endsWith('word59'), 'the last final word is shown');
  assert.ok(`${t.final} ${t.interim}`.length <= 80);
  assert.ok(!t.final.includes('word0 '), 'the first words are cut');
  assert.match(t.final, /^…word\d+ /, 'cut on a word');
  // Only interim, very long: its end.
  const i = plain(rc.rcTail('', long, 50));
  assert.equal(i.final, '…');
  assert.ok(i.interim.endsWith('word59') && i.interim.length <= 49);
  // Short: untouched.
  assert.deepEqual(plain(rc.rcTail('one two', 'three', 140)), { final: 'one two', interim: 'three', cut: false });
});

test('dictation ends: every way says a sentence; words kept are said kept', () => {
  const T = rc.RC_DICT_TEXT;
  const e = o => plain(rc.rcSpeechEnd(o));
  assert.deepEqual(e({ reason: 'person', words: true }), { text: T.added, tone: 'ok' });
  assert.deepEqual(e({ reason: 'person', words: false }), { text: T.nothing, tone: 'warn' });
  assert.equal(e({ error: 'no-speech', reason: 'self' }).text, T.nothing);
  assert.equal(e({ reason: 'self', words: true }).text, `${T.stoppedSelf} ${T.kept}`);
  assert.equal(e({ reason: 'self', words: false }).text, T.nothing);
  assert.equal(e({ error: 'audio-capture', words: false }).text, T.taken);
  assert.equal(e({ error: 'audio-capture', words: true }).text, `${T.taken} ${T.kept}`);
  assert.equal(e({ error: 'network', words: true }).text, `${T.network} ${T.kept}`);
  assert.equal(e({ error: 'not-allowed' }).text, T.refused);
  assert.equal(e({ error: 'service-not-allowed' }).text, T.refused);
  assert.equal(e({ error: 'language-not-supported' }).text, T.language);
  assert.equal(e({ error: 'aborted', reason: 'nostart' }).text, T.noStart);
  assert.equal(e({ reason: 'hidden', words: true }).text, `${T.hidden} ${T.kept}`);
  assert.equal(e({ reason: 'time', words: true }).text, `${T.time} ${T.kept}`);
  for (const error of ['', 'no-speech', 'aborted', 'audio-capture', 'network', 'not-allowed', 'service-not-allowed', 'language-not-supported', 'bad-grammar', 'other']) {
    for (const reason of ['person', 'self', 'hidden', 'time', 'nostart']) {
      for (const words of [true, false]) {
        const o = e({ error, reason, words });
        assert.ok(o.text && o.text.length > 10, `${error}/${reason}/${words}: a sentence`);
        assert.ok(['ok', 'warn'].includes(o.tone));
      }
    }
  }
  // The status line while it records or listens, and when it hears nothing.
  assert.equal(plain(rc.rcStatus({ rec: 'recording', quiet: true })).text, T.quietMac);
  assert.equal(plain(rc.rcStatus({ rec: 'listening', quiet: true })).text, T.quietIphone);
  assert.deepEqual(plain(rc.rcStatus({ hint: { text: 'x', tone: 'warn' } })), { text: 'x', tone: 'warn' });
  assert.deepEqual(plain(rc.rcStatus({ hint: 'y' })), { text: 'y', tone: 'info' });
});

test('dictation restarts: only after a clean end by itself, with words, visible, at most twice', () => {
  const ok = { error: '', reason: 'self', hidden: false, restarts: 0, heard: true, ms: 5000, total: 5000 };
  assert.equal(rc.rcSpeechRestart(ok), true);
  assert.equal(rc.rcSpeechRestart({ ...ok, restarts: rc.RC_SPEECH_RESTARTS }), false, 'at most twice');
  for (const error of ['no-speech', 'aborted', 'audio-capture', 'network', 'not-allowed']) assert.equal(rc.rcSpeechRestart({ ...ok, error }), false, error);
  for (const reason of ['person', 'hidden', 'time', 'nostart']) assert.equal(rc.rcSpeechRestart({ ...ok, reason }), false, reason);
  assert.equal(rc.rcSpeechRestart({ ...ok, hidden: true }), false);
  assert.equal(rc.rcSpeechRestart({ ...ok, heard: false }), false, 'silence: no restart');
  assert.equal(rc.rcSpeechRestart({ ...ok, ms: 300 }), false, 'a round that ends at once: no restart');
  assert.equal(rc.rcSpeechRestart({ ...ok, total: 120000 }), false);
  // A recogniser that ends after every round: the restarts stop.
  let n = 0, restarts = 0;
  while (rc.rcSpeechRestart({ ...ok, restarts }) && n < 100) { restarts++; n++; }
  assert.equal(n, rc.RC_SPEECH_RESTARTS);
});

// The microphone, used again quickly (an iPhone keeps the audio session a
// moment after a recogniser ends: a take started in that time hears nothing).
// A small model of the page: users of the microphone held until they end.
function micModel() {
  const users = new Set();
  let endedAt = 0, now = 0;
  return {
    hold: u => users.add(u),
    end: u => { if (users.delete(u) && !users.size) endedAt = now; },
    at: t => { now = t; },
    gate: (waited = 0) => rc.rcMicGate({ live: users.size, endedAt, now, waited }),
  };
}

test('dictation: a take started while the previous one is still ending waits', () => {
  const m = micModel();
  m.at(1000); m.hold('take1');
  // Stop tapped: the recogniser has not said `end` yet.
  m.at(1200);
  assert.equal(m.gate(0), 'wait', 'never two recognisers at once');
  assert.equal(m.gate(rc.RC_MIC_WAIT_MS - 1), 'wait');
  // Still not ended past the bounded wait: busy, said, nothing starts.
  assert.equal(m.gate(rc.RC_MIC_WAIT_MS), 'busy');
  assert.equal(rc.RC_DICT_TEXT.busy, 'Microphone busy, try again in a second.');
  // Ended: a short settle, then it starts.
  m.at(1500); m.end('take1');
  assert.equal(m.gate(300), 'wait', 'just ended: settle');
  m.at(1500 + rc.RC_MIC_SETTLE_MS);
  assert.equal(m.gate(300 + rc.RC_MIC_SETTLE_MS), 'go');
  // The settle never turns into "busy" and never runs past the bound.
  m.hold('x'); m.at(5000); m.end('x');
  assert.equal(m.gate(rc.RC_MIC_WAIT_MS), 'go');
  assert.ok(rc.RC_MIC_SETTLE_MS < rc.RC_MIC_WAIT_MS);
});

test('dictation: a second take after the first really ended starts', () => {
  const m = micModel();
  assert.equal(m.gate(), 'go', 'first take: at once, in the tap');
  m.at(0); m.hold('take1'); m.at(3000); m.end('take1');
  m.at(3000 + rc.RC_MIC_SETTLE_MS);
  assert.equal(m.gate(), 'go');
  // Mac mode: a stream and two contexts (wave, decode) all let go.
  m.hold('stream'); m.hold('wave'); m.hold('decode');
  m.at(9000); m.end('stream'); m.end('wave');
  assert.equal(m.gate(), 'wait', 'a context still closing');
  m.end('decode'); m.at(9000 + rc.RC_MIC_SETTLE_MS);
  assert.equal(m.gate(), 'go');
  // Five quick takes in a row: each starts once the one before ended.
  let t = 20000;
  for (let i = 0; i < 5; i++) {
    let waited = 0;
    while (m.gate(waited) === 'wait') { waited += 100; m.at(t + waited); }
    assert.equal(m.gate(waited), 'go', `take ${i + 1}`);
    t += waited; m.at(t); m.hold(i); t += 800; m.at(t); m.end(i);
  }
});

test('dictation: "nothing heard" only after audio actually started', () => {
  const q = o => rc.rcMicQuiet({ quietMs: 3000, heardMs: 8000, ...o });
  // No audio yet (a recogniser that has not said audiostart, a context
  // that is suspended): never "nothing heard", however long.
  assert.equal(q({ now: 60000, audioAt: 0 }), false);
  // Audio for less than the quiet time: not yet.
  assert.equal(q({ now: 5000, audioAt: 4000 }), false, 'a slow start is not counted');
  assert.equal(q({ now: 7000, audioAt: 4000 }), true);
  // Words heard: the longer pause counts from the last one.
  assert.equal(q({ now: 10000, audioAt: 4000, heardAt: 5000 }), false);
  assert.equal(q({ now: 13000, audioAt: 4000, heardAt: 5000 }), true);
  // Words of an earlier round do not count for this one.
  assert.equal(q({ now: 21000, audioAt: 20000, heardAt: 15000 }), false);
  assert.equal(q({ now: 23000, audioAt: 20000, heardAt: 15000 }), true);
  // The Mac's level meter: one quiet time.
  assert.equal(rc.rcMicQuiet({ now: 4000, audioAt: 500, heardAt: 1000, quietMs: 3000 }), true);
  assert.equal(rc.rcMicQuiet({ now: 3900, audioAt: 500, heardAt: 1000, quietMs: 3000 }), false);
});

test('dictation: the page holds the microphone until each user really ended', () => {
  const page = pageSrc;
  // A recogniser is held until its own `end`, registered before onend.
  assert.match(page, /micHold\(r\);\s*\n\s*try \{ r\.addEventListener\('end', \(\) => micRelease\(r\)\); \} catch/);
  // Ended in the UI first: aborted, released on `end` or after the bound.
  assert.match(page, /if \(micUsers\.has\(r\)\) \{\s*try \{ r\.abort\(\); \} catch \(_\) \{\}\s*setTimeout\(\(\) => micRelease\(r\), RC_MIC_WAIT_MS \+ 1000\);/);
  // A tap starts at once only when the gate says go; else it waits or says busy.
  assert.match(page, /if \(micGateNow\(\) === 'go'\) \{ go\(\); return; \}/);
  assert.match(page, /if \(g === 'busy'\) \{ say\(RC_DICT_TEXT\.busy, 'warn'\); return; \}/);
  // A restart waits too.
  assert.match(page, /speech\.waiting = true;\s*\n\s*speech\.audioAt = 0;\s*\n\s*micReady\(\)\.then/);
  // Streams and contexts: released on every path.
  assert.match(page, /const release = \(\) => \{ stream\.getTracks\(\)\.forEach\(t => t\.stop\(\)\); micRelease\(stream\); \};/);
  assert.match(page, /rec\.stream\?\.getTracks\(\)\.forEach\(t => t\.stop\(\)\);\s*\n\s*micRelease\(rec\.stream\);/);
  assert.equal((page.match(/closeCtx\(/g) || []).length, 3, 'defined, the wave, the decoder');
  // The quiet line goes through rcMicQuiet in both modes; no other verdict.
  assert.equal((page.match(/setQuiet\(rcMicQuiet\(/g) || []).length, 2);
  assert.doesNotMatch(page, /Date\.now\(\) - speech\.roundAt >= QUIET_MS/);
});

test('dictation: a final text is inserted once', () => {
  // Re-delivered as the same, or as cumulative, within one round.
  assert.equal(plain(rc.rcSpeechRound(SR([['Hello there', true], ['hello there.', true]]))).final, 'hello there.');
  assert.equal(plain(rc.rcSpeechRound(SR([['Plan the table', true], ['Plan the table and the index', true]]))).final, 'Plan the table and the index');
  // Each final carrying the whole text so far, after separate finals.
  assert.equal(plain(rc.rcSpeechRound(SR([['add an', true], ['index', true], ['Add an index on the id', true]]))).final, 'Add an index on the id');
  assert.equal(plain(rc.rcSpeechRound(SR([['add an index', true], ['add an index', true], ['on the id', true]]))).final, 'add an index on the id', 'A A B gives A B');
  // Distinct sentences stay; a one-word repeat is a person's.
  assert.equal(plain(rc.rcSpeechRound(SR([['one two', true], ['three four', true]]))).final, 'one two three four');
  assert.equal(plain(rc.rcSpeechRound(SR([['yes', true], ['yes', true]]))).final, 'yes yes');
  // Recomputed from the whole list: the same event twice gives the same text.
  const list = SR([['a b c', true], ['d', false]]);
  assert.deepEqual(plain(rc.rcSpeechRound(list)), plain(rc.rcSpeechRound(list)));
  // After a restart: the first result repeats the last of the round before.
  const r2 = plain(rc.rcSpeechRound(SR([['add an index', true], ['then', false]]), 'add an index'));
  assert.equal(r2.final, '');
  assert.equal(r2.interim, 'then');
  assert.equal(plain(rc.rcSpeechRound(SR([['Add an index on the id', true]]), 'add an index')).final, 'on the id');
  assert.equal(plain(rc.rcSpeechRound(SR([['and more', true]]), 'add an index')).final, 'and more');
  assert.equal(plain(rc.rcSpeechRound(SR([['and more', true]]), 'add an index')).last, 'and more');
});

test('the Mac path: a format the phone records, no empty or tiny clip, the answer as a sentence', () => {
  const T = rc.RC_DICT_TEXT;
  const iphone = t => t === 'audio/mp4';
  const chromium = t => t.startsWith('audio/webm');
  assert.equal(rc.rcRecMime(iphone), 'audio/mp4');
  assert.equal(rc.rcRecMime(chromium), 'audio/webm;codecs=opus');
  assert.equal(rc.rcRecMime(() => false), '');
  assert.equal(rc.rcRecMime(() => { throw new Error('x'); }), '');
  assert.equal(rc.rcRecMime(undefined), '');
  assert.equal(rc.rcClipCheck(0, 3000), T.macEmpty);
  assert.equal(rc.rcClipCheck(0, 100), T.macShort, 'stopped before the first chunk');
  assert.equal(rc.rcClipCheck(1200, 300), T.macShort);
  assert.equal(rc.rcClipCheck(1200, 500), '');
  assert.equal(rc.rcSilentPcm(new Float32Array(1600)), true);
  assert.equal(rc.rcSilentPcm(Float32Array.from([0, 0.001, -0.3])), false);
  const tr = r => plain(rc.rcTranscribed(r));
  assert.deepEqual(tr({ ok: true, status: 200, data: { text: ' hi there ' } }), { words: 'hi there' });
  assert.deepEqual(tr({ ok: true, status: 200, data: { text: '  ' } }), { text: T.macNoWords });
  assert.deepEqual(tr({ ok: true, status: 200, data: {} }), { text: T.macNoWords });
  assert.deepEqual(tr({ ok: false, status: 503 }), { text: T.macMissing });
  assert.deepEqual(tr({ ok: false, status: 0, unreachable: true }), { text: T.macUnreachable });
  assert.deepEqual(tr({ ok: false, status: 502, unreachable: true }), { text: T.macUnreachable });
  assert.deepEqual(tr({ ok: false, status: 413 }), { text: T.macTooLong });
  assert.deepEqual(tr({ ok: false, status: 500 }), { text: T.macFailed });
  assert.deepEqual(tr({ ok: false, timeout: true }), { text: T.macSlow });
  assert.deepEqual(tr(null), { text: T.unreadable });
});

// ── Security review P6: invisible and bidi controls made visible ──
const CONTROLS = [0x202A, 0x202B, 0x202C, 0x202D, 0x202E, 0x2066, 0x2067, 0x2068, 0x2069, 0x200B, 0x200C, 0x200D, 0x200E, 0x200F, 0x2060, 0xFEFF, 0x00AD,
  0x2028, 0x2029, 0x061C, 0x034F, 0x115F, 0x1160, 0x3164, 0xFE0F, 0xFFA0, 0xFFF9, 0xE0001, 0xE0041, 0x001B, 0x007F, 0x0085];
test('every invisible or bidi control becomes a visible code, nothing else does', () => {
  for (const cp of CONTROLS) {
    const parts = plain(rc.rcMarked(`a${String.fromCodePoint(cp)}b`));
    assert.deepEqual(parts, ['a', { code: `U+${cp.toString(16).toUpperCase().padStart(4, '0')}` }, 'b'], cp.toString(16));
  }
  for (const ok of ['plain', 'é à ü', '日本語', 'עברית', 'العربية', '😀👍🏽', 'a\tb', 'line\nline', 'src/x.rs']) {
    assert.deepEqual(plain(rc.rcMarked(ok)), [ok], ok);
  }
  assert.deepEqual(plain(rc.rcMarked('')), []);
  assert.deepEqual(plain(rc.rcMarked(null)), []);
  // A path that would read as another file: the reordering control is shown.
  const trick = `src/\u202Etxt.exe`;
  assert.deepEqual(plain(rc.rcMarked(trick)), ['src/', { code: 'U+202E' }, 'txt.exe']);
});
test('a sheet\'s line: the page\'s words and the daemon\'s data apart', () => {
  const parts = plain(rc.rcActParts('merge', { sandbox: 'web\u202Esite', branches: [{}, {}], order: ['a', 'b\u200B'] }));
  assert.deepEqual(parts, ['Merge 2 branches of ', { data: 'web\u202Esite' }, ' in the order shown, stopping at the first conflict.', ' Order: ', { data: 'a' }, ', ', { data: 'b\u200B' }, '.']);
  for (const kind of ['plan_approve', 'plan_refuse', 'network_respond', 'sandbox_remove']) {
    const p = plain(rc.rcActParts(kind, { sandbox: 's', name: 'n', host: 'h', threads: [] }, { id: 'dismiss', label: 'dismiss' }));
    assert.ok(p.some(x => typeof x === 'object'), kind);
    assert.ok(p.filter(x => typeof x === 'string').every(x => !/[\p{Cf}]/u.test(x)), 'the page\'s own words carry no control');
  }
  assert.deepEqual(plain(rc.rcActParts('nope', {})), []);
  assert.deepEqual(plain(rc.rcActParts('plan_approve', { sandbox: 's', threads: [{}, {}] }, null, { b: 'haiku' })), ['Approve the plan of ', { data: 's' }, ': it starts 2 threads, as proposed except the model of ', { data: 'b' }, ' (now ', { data: 'haiku' }, ')', '.']);
  assert.equal(rc.rcActLine('plan_approve', { sandbox: 's', threads: [{}] }), 'Approve the plan of s as it is: it starts 1 thread.');
});
test('a plan\'s model changes: only threads of the plan, models of the list, and only real changes', () => {
  const threads = [{ name: 'a', model: 'opus' }, { name: 'b', model: null }, { name: 'c', model: 'sonnet' }];
  const list = [{ id: 'opus', label: 'Opus' }, { id: 'haiku', label: 'Haiku' }, { id: 'sonnet', label: 'Sonnet' }];
  assert.deepEqual(plain(rc.rcModelChanges(threads, { a: 'opus', b: 'haiku', c: 'gpt', d: 'haiku' }, list)), { b: 'haiku' });
  assert.deepEqual(plain(rc.rcModelChanges(threads, {}, list)), {});
  assert.deepEqual(plain(rc.rcModelChanges(threads, { a: 'haiku' }, [])), {}, 'no list, no change');
  assert.deepEqual(plain(rc.rcModelChanges(null, { a: 'haiku' }, list)), {});
  const odd = rc.rcModelChanges([{ name: '__proto__', model: 'opus' }], JSON.parse('{"__proto__":"haiku"}'), list);
  assert.deepEqual(Object.keys(odd), ['__proto__'], 'an own key, never a prototype');
  assert.equal(Object.getPrototypeOf(odd), Object.getPrototypeOf(rc.rcModelChanges([], {}, list)));
});

// ── Plan phone-sandboxes (API.md 2.4, v1.5) ──
test('the picker: team leaders first, then sandboxes, those that need you first in each', () => {
  const g = rc.rcGroups({ leaders: [
    { sandbox: 'calm', kind: 'sandbox', state: 'idle', last_at: 9 },
    { sandbox: 'web', kind: 'leader', state: 'idle', last_at: 5 },
    { sandbox: 'asks', kind: 'sandbox', state: 'attention', asks: true, last_at: 1 },
    { sandbox: 'old', state: 'working', last_at: 2 },
  ] });
  assert.deepEqual(plain(g.map(x => [x.title, x.items.map(i => i.sandbox)])), [['Team leaders', ['old', 'web']], ['Sandboxes', ['asks', 'calm']]],
    'an entry with no kind is a leader (a daemon before v1.5)');
  const one = plain(rc.rcGroups({ leaders: [{ sandbox: 'a', kind: 'sandbox', state: 'idle' }] }));
  assert.deepEqual(one.map(x => [x.title, x.count]), [['Team leaders', 0], ['Sandboxes', 1]], 'both groups stay, with their counts');
  assert.equal(one[0].empty, 'No team leader running');
  const none = plain(rc.rcGroups({ leaders: [] }));
  assert.deepEqual(none.map(x => [x.kind, x.count, x.icon, x.empty]), [['leader', 0, 'home-i-team', 'No team leader running'], ['sandbox', 0, 'home-i-box', 'No other sandbox running']]);
});
test('the Waiting for you tiles: the Approvals kinds, colours and icons, 0 dimmed', () => {
  const t = plain(rc.rcWaiting({ plans: 2, merges: 0, network: '3', stopped: 4 }));
  assert.deepEqual(t.map(x => [x.what, x.n, x.zero]), [['plans', 2, false], ['merges', 0, true], ['network', 3, false]], 'stopped sandboxes are no tile');
  assert.deepEqual(t.map(x => [x.cls, x.icon]), [['rc-k-plans', 'ap-i-plans'], ['rc-k-merges', 'ap-i-merges'], ['rc-k-network', 'ap-i-network']]);
  assert.ok(plain(rc.rcWaiting(null)).every(x => x.n === 0 && x.zero), 'no counts: all zero');
});
test('the home: stopped sandboxes are a third group, last, only when there are any', () => {
  const leaders = { leaders: [{ sandbox: 'web', kind: 'leader', state: 'idle' }, { sandbox: 'a', kind: 'sandbox', state: 'idle' }] };
  const stopped = [
    { name: 'old', agent: 'claude', chat: false, workspace: '/Users/me/dev/old', created_at: 'x' },
    { name: 'notes', agent: '', chat: true, workspace: null },
    { name: '', agent: 'claude' }, null, { agent: 'claude' },
  ];
  const g = plain(rc.rcGroups(leaders, stopped));
  assert.deepEqual(g.map(x => [x.kind, x.title, x.count]), [['leader', 'Team leaders', 1], ['sandbox', 'Sandboxes', 1], ['stopped', 'Stopped sandboxes', 2]]);
  assert.equal(g[2].icon, 'ap-i-stopped');
  assert.deepEqual(g[2].items, [{ name: 'old', line: 'claude · /Users/me/dev/old' }, { name: 'notes', line: 'chat' }], 'a name, then the agent (or chat) and the path; no name, no card');
  for (const none of [undefined, null, [], 'x', [{ agent: 'claude' }]]) {
    assert.deepEqual(plain(rc.rcGroups(leaders, none)).map(x => x.kind), ['leader', 'sandbox'], `no stopped group for ${JSON.stringify(none)}`);
  }
  assert.deepEqual(plain(rc.rcStopped([{ name: 'x', agent: 7, workspace: { a: 1 } }])), [{ name: 'x', line: '' }], 'odd fields are dropped');
});
test('the stopped group folds past three entries, until Show all', () => {
  const n = k => Array.from({ length: k }, (_, i) => `s${i}`);
  for (const k of [0, 1, 3]) assert.deepEqual(plain(rc.rcFold(n(k), false)), { shown: n(k), rest: 0, open: false }, `${k}: never folded`);
  assert.deepEqual(plain(rc.rcFold(n(4), false)), { shown: n(3), rest: 1, open: false });
  assert.deepEqual(plain(rc.rcFold(n(9), false)), { shown: n(3), rest: 6, open: false });
  assert.deepEqual(plain(rc.rcFold(n(9), true)), { shown: n(9), rest: 6, open: true }, 'opened: all, with Show fewer');
  assert.deepEqual(plain(rc.rcFold(n(2), true)), { shown: n(2), rest: 0, open: false }, 'nothing to fold');
  assert.deepEqual(plain(rc.rcFold(null, false)), { shown: [], rest: 0, open: false });
});
test('the sprite: kind icons are outlines (a circle in the sprite is never a filled dot)', () => {
  const css = read('../../assets/remote/remote.css');
  const html = read('../../assets/remote/index.html');
  // The page draws every svg circle filled; a <use> clone matches as its
  // symbol is placed, in the sprite, so the sprite must undo that.
  assert.match(css, /svg circle \{ fill: currentColor; stroke: none; \}/);
  assert.match(css, /\.rc-sprite symbol circle \{ fill: none; stroke: currentColor; \}/);
  const sprite = html.slice(html.indexOf('<svg class="rc-sprite"'), html.indexOf('</svg>', html.indexOf('<svg class="rc-sprite"')));
  assert.match(sprite, /id="ap-i-network"[^]*?<circle cx="12" cy="12" r="9"\/>/, 'the globe is a circle, outlined by the rule above');
  assert.doesNotMatch(sprite, /fill=|style=/, 'no symbol sets its own fill');
});
test('a card info line: only what is known', () => {
  const models = [{ id: 'claude-opus-5-5', label: 'Opus 5.5' }];
  assert.deepEqual(plain(rc.rcLeaderInfo({ model: 'claude-opus-5-5', threads_working: 2, canvas_new: 1, tickets_planned: 3 }, models)), { parts: ['Opus 5.5', '2 threads working', '3 planned'], canvas: 1 });
  assert.deepEqual(plain(rc.rcLeaderInfo({ model: 'odd-id', threads_working: 1 }, models)), { parts: ['odd-id', '1 thread working'], canvas: 0 }, 'the id when the list lacks it');
  assert.deepEqual(plain(rc.rcLeaderInfo({}, [])), { parts: [], canvas: 0 });
  assert.deepEqual(plain(rc.rcLeaderInfo({ threads_working: 0, canvas_new: 'x', tickets_planned: -2 }, null)), { parts: [], canvas: 0 });
});
test('a read-only entry says why, and Send says it too', () => {
  const two = { sandbox: 'a', kind: 'sandbox', state: 'idle', read_only: 'several_sessions' };
  const bare = { sandbox: 'b', kind: 'sandbox', state: 'idle', read_only: 'no_terminal' };
  assert.match(rc.rcReadOnly(two), /several sessions/);
  assert.match(rc.rcReadOnly(bare), /no terminal/);
  assert.equal(rc.rcReadOnlyShort(two), 'Read only: several sessions open');
  assert.equal(rc.rcSendGate(two), rc.rcReadOnly(two), 'Send waits with the reason');
  assert.equal(rc.rcSendGate({ ...two, state: 'working' }), rc.rcReadOnly(two), 'read only before working');
  for (const odd of ['__proto__', 'toString', 'something new']) assert.match(rc.rcReadOnly({ read_only: odd }), /^Read only/);
  assert.equal(rc.rcReadOnly({ state: 'idle' }), '');
  assert.equal(rc.rcSendGate({ state: 'idle', kind: 'sandbox' }), '');
});
test('the page no longer says "leader" where it means either kind', () => {
  const html = read('../../assets/remote/index.html');
  assert.doesNotMatch(html, /No sandbox is running/, "the empty lines are per group now (rcGroups)");
  assert.match(html, /placeholder="Write a message"/);
  assert.doesNotMatch(html, /No team leader is running|Message the leader|Back to the leaders/);
  for (const v of Object.values(rc.RC_MSG_REASON)) assert.doesNotMatch(v, /leader/i);
});

// ── Model and lead controls ──
// Loaded the same way as the cases above: the page's own source in a vm.
test('the model list keeps only {id, label} and the select follows the reported model', () => {
  const src = readFileSync(new URL('../../assets/remote/remote-core.js', import.meta.url), 'utf8');
  const ctx = {};
  vm.createContext(ctx);
  vm.runInContext(src + '\n;globalThis.__t = { rcModels, rcModelValue, rcCtlGate };', ctx);
  const { rcModels, rcModelValue, rcCtlGate } = ctx.__t;
  const list = rcModels({ models: [{ id: 'a', label: 'A', extra: 1 }, { id: '' }, null, { id: 'b' }] });
  assert.deepEqual(JSON.parse(JSON.stringify(list)), [{ id: 'a', label: 'A' }, { id: 'b', label: 'b' }]);
  assert.equal(rcModelValue({ model: 'a' }, list), 'a');
  assert.equal(rcModelValue({ model: 'zzz' }, list), '');
  assert.equal(rcModelValue(null, list), '');
  assert.match(rcCtlGate(null, 'model'), /not running/);
  assert.match(rcCtlGate({ state: 'working', kind: 'sandbox' }, 'model'), /working/);
  assert.equal(rcCtlGate({ state: 'working', kind: 'leader' }, 'lead'), '', 'ending lead needs no idle prompt');
  assert.equal(rcCtlGate({ state: 'idle', kind: 'sandbox' }, 'lead'), '');
});

// ── Status labels ──
test('the pill word table equals the built-in one of crates/core/src/emphasis.rs', () => {
  const rs = read('../../crates/core/src/emphasis.rs');
  const body = /const BUILT_IN[^=]*=\s*&\[([\s\S]*?)\n\];/.exec(rs)[1];
  const want = {};
  for (const m of body.matchAll(/\("([^"]+)",\s*"([^"]+)",\s*&\[([^\]]*)\]/g)) {
    want[m[1]] = m[2];
    for (const a of m[3].matchAll(/"([^"]+)"/g)) want[a[1]] = m[2];
  }
  assert.ok(Object.keys(want).length >= 20);
  assert.deepEqual(plain(rc.RC_PILL_TONES), want);
});
test('a status label is a pill with its tone and detail; unknown words and code stay plain', () => {
  assert.match(rc.rcAgentHtml('Build [[Done]] ok'), /<span class="rc-pill rc-pill-green">Done<\/span>/);
  assert.match(rc.rcAgentHtml('[[Failed: 3 tests]]'), /rc-pill-red">Failed<span class="rc-pill-detail"> ?3 tests<\/span>/);
  assert.match(rc.rcAgentHtml('[[needs  you]]'), /rc-pill-amber/);
  assert.doesNotMatch(rc.rcAgentHtml('[[Banana]] and [[x]]'), /rc-pill/);
  assert.doesNotMatch(rc.rcAgentHtml('`[[Done]]`'), /rc-pill/);
  assert.doesNotMatch(rc.rcAgentHtml('```\n[[Done]]\n```'), /rc-pill/);
  const hostile = rc.rcAgentHtml('[[Done: <img src=x onerror=alert(1)>]]');
  assertSafe(hostile, 'pill detail');
});

test('the toast names what is fresh and never guesses a plan', () => {
  const t = (f, d) => plain(rc.rcToastChoice(f, d));
  assert.deepEqual(t([{ what: 'merges', sandbox: 'a' }], 1), { what: 'merges', text: 'A merge waits for you' });
  assert.deepEqual(t([{ what: 'plans' }, { what: 'plans' }], 2), { what: 'plans', text: '2 plans wait for you' });
  assert.deepEqual(t([], 1), { what: 'neutral', text: 'Something waits for you' }, 'a merge of a plan already seen');
  assert.deepEqual(t([{ what: 'plans' }, { what: 'merges' }], 2), { what: 'neutral', text: '2 things wait for you' });
});
