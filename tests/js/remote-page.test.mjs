// The phone page in a real browser, against the stand-in of the remote
// listener (tests/js/remote-standin.mjs), under the listener's CSP: the
// pairing flow, the fragment removed, the leaders, a conversation, a
// question answered, Send / answer / Interrupt each calling their route once
// and only on a tap, dictation filling the field without sending, the
// unreachable state keeping the text and sending nothing later, a 401, hostile
// text, and nothing written to storage but the device name.
//
// Needs Playwright and a Chromium: PLAYWRIGHT_MODULE (a playwright-core
// install, else `playwright-core` must resolve) and CHROMIUM_PATH (default
// /usr/bin/chromium). Without them the test is skipped, and says so.
// REMOTE_SHOTS=<dir> also writes the screenshots at 390x844 and 320x568.
// Run: node --test tests/js/remote-page.test.mjs
import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, mkdirSync, readFileSync } from 'node:fs';
import { startStandin } from './remote-standin.mjs';

let pw = null;
try { pw = await import(process.env.PLAYWRIGHT_MODULE || 'playwright-core'); } catch (_) {}
const exe = process.env.CHROMIUM_PATH || '/usr/bin/chromium';
const skip = !pw ? 'no Playwright (set PLAYWRIGHT_MODULE)' : !existsSync(exe) ? `no Chromium at ${exe}` : false;
const SHOTS = process.env.REMOTE_SHOTS || '';
const FX = JSON.parse(readFileSync(new URL('./remote-fixtures.json', import.meta.url), 'utf8'));
if (SHOTS) mkdirSync(SHOTS, { recursive: true });

const IPHONE_UA = 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1';
const SECRET_WORDS = ['Plan the orders table', 'hello leader', 'customer id', 'PostgreSQL', 'kept while down'];

// Spies, installed before any page script: every storage write, every
// console call, every CSP violation, every EventSource and fetch.
function spies() {
  const spy = { storage: [], console: [], csp: [], idb: 0, caches: 0, sw: 0 };
  window.__spy = spy;
  const set = Storage.prototype.setItem;
  Storage.prototype.setItem = function (k, v) {
    spy.storage.push([this === window.sessionStorage ? 'session' : 'local', k, String(v)]);
    return set.call(this, k, v);
  };
  try { const open = indexedDB.open.bind(indexedDB); indexedDB.open = (...a) => { spy.idb++; return open(...a); }; } catch (_) {}
  try { if (window.caches) { const o = caches.open.bind(caches); caches.open = (...a) => { spy.caches++; return o(...a); }; } } catch (_) {}
  try { if (navigator.serviceWorker) navigator.serviceWorker.register = () => { spy.sw++; return Promise.reject(new Error('no')); }; } catch (_) {}
  for (const k of ['log', 'info', 'warn', 'error', 'debug']) {
    const f = console[k].bind(console);
    console[k] = (...a) => { spy.console.push(a.map(String).join(' ')); f(...a); };
  }
  document.addEventListener('securitypolicyviolation', e => spy.csp.push(`${e.violatedDirective} ${e.blockedURI}`));
  // A fake microphone: a tone through a real MediaRecorder, so the page's
  // decoding and resampling run as on a phone.
  navigator.mediaDevices.getUserMedia = async () => {
    const ctx = new AudioContext();
    const osc = ctx.createOscillator();
    const dest = ctx.createMediaStreamDestination();
    osc.connect(dest);
    osc.start();
    return dest.stream;
  };
}

let sb, browser;
before(async () => {
  if (skip) return;
  sb = await startStandin(0);
  browser = await pw.chromium.launch({ executablePath: exe, args: ['--autoplay-policy=no-user-gesture-required'] });
});
after(async () => {
  await browser?.close();
  await sb?.close();
});

async function phone(width = 390, height = 844) {
  const ctx = await browser.newContext({ viewport: { width, height }, deviceScaleFactor: 2, isMobile: true, hasTouch: true, userAgent: IPHONE_UA });
  await ctx.addInitScript(spies);
  const page = await ctx.newPage();
  const problems = [];
  page.on('pageerror', e => problems.push(`pageerror: ${e.message}`));
  page.on('console', m => { if (m.type() === 'error' && !/net::ERR_|Failed to load resource/.test(m.text())) problems.push(`console: ${m.text()}`); });
  return { ctx, page, problems };
}
const base = () => `http://127.0.0.1:${sb.port}`;
const ctl = body => fetch(`${base()}/__ctl`, { method: 'POST', body: JSON.stringify(body) });
const log = async () => (await fetch(`${base()}/__log`)).json();
const calls = async (method, path) => (await log()).filter(e => e.method === method && e.path === path);
const shot = async (page, name) => { if (SHOTS) await page.screenshot({ path: `${SHOTS}/${name}.png` }); };
const noHScroll = page => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth && document.body.scrollWidth <= window.innerWidth);
async function cleanSpy(page) {
  const spy = await page.evaluate(() => window.__spy);
  assert.deepEqual(spy.csp, [], 'no CSP violation');
  assert.deepEqual(spy.console, [], 'no console output');
  assert.equal(spy.idb + spy.caches + spy.sw, 0, 'no IndexedDB, Cache API or service worker');
  for (const [area, key, value] of spy.storage) {
    assert.equal(`${area} ${key}`, 'local sbxw-remote-prefs', 'only the device name is stored');
    assert.deepEqual(Object.keys(JSON.parse(value)), ['device_name']);
    for (const w of SECRET_WORDS) assert.ok(!value.includes(w));
  }
  const stored = await page.evaluate(() => JSON.stringify({ ...localStorage }) + JSON.stringify({ ...sessionStorage }));
  for (const w of SECRET_WORDS) assert.ok(!stored.includes(w), `"${w}" stored`);
}

test('pairing: the fragment goes at once, the check number shows, then the leaders', { skip }, async () => {
  await ctl({ reset: true, paired: false, pair: ['pending', 'pending', 'paired'] });
  const { ctx, page, problems } = await phone();
  await page.goto(`${base()}/#pair=CODE1234abcd`);
  await page.waitForSelector('#pair-form:not([hidden])');
  assert.equal(await page.evaluate(() => location.href), `${base()}/`, 'the fragment is removed');
  assert.equal(await page.inputValue('#pair-name'), 'iPhone', 'prefilled from the platform');
  assert.equal((await log()).length, 1, 'only GET /r/me before the person taps Pair');
  for (const e of await log()) assert.ok(!(e.path + e.query).includes('CODE1234abcd'), 'the code is in no URL');
  await shot(page, '01-pairing-name');
  await page.fill('#pair-name', 'Test phone');
  await page.tap('#pair-go');
  await page.waitForSelector('#pair-wait:not([hidden])');
  assert.equal(await page.textContent('#pair-check'), '482 913');
  assert.match(await page.textContent('#pair-text'), /type these six digits/);
  assert.equal(await page.textContent('#pair-wait h2'), 'Type this number on your Mac');
  await shot(page, '02-pairing-check');
  const start = await calls('POST', '/r/pair/start');
  assert.equal(start.length, 1);
  assert.deepEqual(start[0].body, { code: 'CODE1234abcd', device_name: 'Test phone' });
  await page.waitForSelector('#s-leaders:not([hidden])', { timeout: 10000 });
  assert.ok((await calls('GET', '/r/pair/status')).every(e => /^\?request_id=[0-9a-f]{32}$/.test(e.query)));
  await cleanSpy(page);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('pairing refused, expired and rate limited each say so', { skip }, async () => {
  for (const [state, words] of [['refused', /refused on the Mac/], ['expired', /^This request expired\. Open the new link from your Mac\.$/]]) {
    await ctl({ reset: true, paired: false, pair: [state] });
    const { ctx, page } = await phone();
    await page.goto(`${base()}/#pair=CODE1234abcd`);
    await page.tap('#pair-go');
    await page.waitForSelector('#pair-end:not([hidden])', { timeout: 6000 });
    assert.match(await page.textContent('#pair-end-text'), words);
    if (state === 'refused') await shot(page, '03-pairing-refused');
    await ctx.close();
  }
  for (const [status, words] of [[429, /Too many attempts/], [403, /not valid any more/]]) {
    await ctl({ reset: true, paired: false, pairStart: status });
    const { ctx, page } = await phone();
    await page.goto(`${base()}/#pair=CODE1234abcd`);
    await page.tap('#pair-go');
    await page.waitForSelector('#pair-end:not([hidden])');
    assert.match(await page.textContent('#pair-end-text'), words);
    assert.equal((await calls('POST', '/r/pair/start')).length, 1, 'no retry');
    await ctx.close();
  }
  await ctl({ pairStart: 200 });
  // Without a code: the not-paired screen.
  await ctl({ reset: true, paired: false });
  const { ctx, page } = await phone();
  await page.goto(`${base()}/`);
  await page.waitForSelector('#s-unpaired:not([hidden])');
  assert.match(await page.textContent('#unpaired-title'), /not paired/);
  await ctx.close();
});

test('leaders, a conversation, a question answered once on a tap', { skip }, async () => {
  await ctl({ reset: true, paired: true });
  const { ctx, page, problems } = await phone();
  await page.goto(`${base()}/`);
  await page.waitForSelector('.rc-leader');
  const names = await page.$$eval('.rc-leader-name', els => els.map(e => e.textContent));
  assert.deepEqual(names, ['shop', 'docs', 'payments-service-with-a-very-long-sandbox-name', 'website']);
  assert.ok(await noHScroll(page), 'no horizontal scroll');
  await shot(page, '04-leaders');
  await page.tap('.rc-leader >> nth=0');
  await page.waitForSelector('.rc-card-q .rc-opt');
  await page.waitForSelector('.rc-steps');
  assert.equal(await page.textContent('#title'), 'shop');
  assert.equal(await page.$$eval('.rc-steps', els => els.map(e => e.textContent).join('|')), '3 steps|1 step');
  assert.equal(await page.textContent('.rc-sbxw b'), 'From sbxw');
  assert.equal(await page.textContent('.rc-from'), 'From db-schema');
  const link = await page.$eval('.rc-link', a => [a.getAttribute('href'), a.target, a.rel, a.textContent]);
  assert.deepEqual(link, ['https://example.org/guide?a=1&b=2', '_blank', 'noopener noreferrer', 'the guide https://example.org/guide?a=1&b=2']);
  await shot(page, '05-conversation');
  assert.equal((await calls('POST', '/r/leaders/shop/answer')).length, 0);
  await page.tap('.rc-opt >> nth=0');
  assert.equal((await calls('POST', '/r/leaders/shop/answer')).length, 0, 'picking is not answering');
  await page.evaluate(() => document.getElementById('log').scrollTo(0, 1e6));
  await shot(page, '06-question-card');
  await page.tap('.rc-card-q .rc-primary');
  await page.waitForSelector('text=Answer sent.');
  const answers = await calls('POST', '/r/leaders/shop/answer');
  assert.equal(answers.length, 1);
  assert.deepEqual(answers[0].body, { indices: [1] });
  assert.equal(answers[0].ct, 'application/json');
  assert.equal(answers[0].origin, base(), 'the browser sends Origin on a POST');
  // A leader waiting on a permission prompt: no answer from here.
  await page.tap('#back');
  await page.waitForSelector('#s-leaders:not([hidden])');
  await page.tap('.rc-leader >> nth=1');
  await page.waitForSelector('text=This needs your Mac.');
  assert.equal(await page.$$eval('.rc-card-q button', b => b.length), 0);
  await cleanSpy(page);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('Send calls message once and only on a tap; Interrupt asks for a second tap', { skip }, async () => {
  await ctl({ reset: true, paired: true });
  const { ctx, page, problems } = await phone();
  await page.goto(`${base()}/`);
  await page.tap('.rc-leader >> text=website');
  await page.waitForSelector('#s-chat:not([hidden])');
  await page.fill('#field', 'hello leader');
  await page.press('#field', 'Enter');
  await page.waitForTimeout(300);
  assert.equal((await calls('POST', '/r/leaders/website/message')).length, 0, 'typing and Enter never send');
  assert.match(await page.inputValue('#field'), /hello leader/);
  await page.fill('#field', 'hello leader');
  await page.tap('#send');
  await page.waitForSelector('text=Sent.');
  const msgs = await calls('POST', '/r/leaders/website/message');
  assert.equal(msgs.length, 1);
  assert.deepEqual(msgs[0].body, { text: 'hello leader' });
  assert.equal(await page.inputValue('#field'), '');

  await page.tap('#interrupt');
  assert.equal(await page.textContent('#interrupt'), 'Interrupt website?');
  assert.equal((await calls('POST', '/r/leaders/website/interrupt')).length, 0, 'the first tap only asks');
  await shot(page, '07-interrupt-confirm');
  await page.tap('#interrupt-cancel');
  assert.equal(await page.textContent('#interrupt'), 'Interrupt');
  await page.tap('#interrupt');
  await page.tap('#interrupt');
  await page.waitForSelector('text=Interrupt sent.');
  assert.equal((await calls('POST', '/r/leaders/website/interrupt')).length, 1);
  await cleanSpy(page);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('a message takes seconds: Send is busy, a second tap sends nothing more', { skip }, async () => {
  await ctl({ reset: true, paired: true, messageDelay: 1500 });
  const { ctx, page, problems } = await phone();
  await page.goto(`${base()}/`);
  await page.tap('.rc-leader >> text=website');
  await page.fill('#field', 'hello leader');
  await page.tap('#send');
  assert.equal(await page.textContent('#send'), 'Sending…');
  assert.equal(await page.isDisabled('#send'), true);
  await page.evaluate(() => document.getElementById('send').click());
  await page.waitForSelector('text=Sent.', { timeout: 6000 });
  assert.equal((await calls('POST', '/r/leaders/website/message')).length, 1, 'one tap, one message');
  assert.equal(await page.textContent('#send'), 'Send');
  await ctl({ messageDelay: 0 });
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('refusals: a sentence per reason, the text kept, nothing resent', { skip }, async () => {
  await ctl({
    reset: true, paired: true,
    messageReplies: [
      { status: 409, body: { ok: false, reason: 'not_submitted', error: 'the leader changed before the Return' } },
      { status: 409, body: { ok: false, reason: 'busy', error: 'a tool call is under way' } },
      { status: 400, body: { ok: false, reason: 'command', error: 'a line starts with /' } },
    ],
  });
  const { ctx, page, problems } = await phone();
  await page.goto(`${base()}/`);
  await page.tap('.rc-leader >> text=website');
  await page.fill('#field', 'hello leader');
  await page.tap('#send');
  await page.waitForSelector('.rc-composer-note.rc-warn');
  assert.match(await page.textContent('#composer-note'), /NOT SENT: the text was typed into the leader's input on the Mac but not submitted.*\(sbxw: the leader changed before the Return\)/);
  assert.equal(await page.inputValue('#field'), 'hello leader', 'kept in the composer');
  await shot(page, '14-not-submitted');
  await page.waitForTimeout(1500);
  assert.equal((await calls('POST', '/r/leaders/website/message')).length, 1, 'never resent by itself');
  await page.tap('#send');
  await page.waitForSelector('text=/once it is back at its prompt/');
  assert.equal(await page.inputValue('#field'), 'hello leader');
  // A command: warned in place while typing, still the person's to send; the daemon refuses it.
  await page.fill('#field', '/model opus');
  await page.waitForSelector('text=/Commands cannot be sent from the phone/');
  assert.equal(await page.isDisabled('#send'), false, 'the daemon stays the judge');
  await shot(page, '15-command-warning');
  assert.equal((await calls('POST', '/r/leaders/website/message')).length, 2, 'the warning sent nothing');
  await page.tap('#send');
  await page.waitForSelector('text=/a line starts with \\//');
  assert.equal(await page.inputValue('#field'), '/model opus');
  assert.equal((await calls('POST', '/r/leaders/website/message')).length, 3);
  await cleanSpy(page);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('while the leader works Send waits, the text stays, nothing is queued', { skip }, async () => {
  await ctl({ reset: true, paired: true, leaders: FX.leaders, history: { website: FX.history.website } });
  const { ctx, page, problems } = await phone();
  await page.goto(`${base()}/`);
  await page.tap('.rc-leader >> text=website');
  await page.waitForSelector('#s-chat:not([hidden])');
  assert.equal(await page.isDisabled('#send'), false);
  const working = structuredClone(FX.leaders);
  working.leaders.find(l => l.sandbox === 'website').state = 'working';
  working.leaders.find(l => l.sandbox === 'website').activity = 'Working';
  await ctl({ emit: { event: 'leaders', data: working } });
  await page.waitForSelector('#send-gate:not([hidden])');
  assert.equal(await page.textContent('#send-gate'), 'The leader is working: Send comes back at its prompt.');
  assert.equal(await page.isDisabled('#send'), true);
  await page.fill('#field', 'hello leader');
  await page.evaluate(() => document.getElementById('send').click());
  assert.equal(await page.isDisabled('#interrupt'), false, 'it can still be interrupted');
  await shot(page, '16-working-send-waits');
  await ctl({ emit: { event: 'leaders', data: FX.leaders } });
  await page.waitForSelector('#send-gate', { state: 'hidden' });
  assert.equal(await page.isDisabled('#send'), false);
  await page.waitForTimeout(800);
  assert.equal((await calls('POST', '/r/leaders/website/message')).length, 0, 'nothing queued while it worked');
  assert.equal(await page.inputValue('#field'), 'hello leader', 'the text stayed');
  // A daemon `busy` (it started working meanwhile): its sentence, the text kept.
  await ctl({ messageReplies: [{ status: 409, body: { ok: false, reason: 'busy', error: 'the leader is working' } }] });
  await page.tap('#send');
  await page.waitForSelector('text=/meanwhile you can read it and interrupt it/');
  assert.equal(await page.inputValue('#field'), 'hello leader');
  // An invisible character: warned in place, with what to do.
  await page.fill('#field', 'hello\u200bleader');
  await page.waitForSelector('text=/Remove the invisible characters, or retype the text/');
  assert.equal((await calls('POST', '/r/leaders/website/message')).length, 1);
  await cleanSpy(page);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('dictation fills the field and never sends', { skip }, async () => {
  await ctl({ reset: true, paired: true });
  const { ctx, page, problems } = await phone();
  await page.goto(`${base()}/`);
  await page.tap('.rc-leader >> text=website');
  await page.fill('#field', 'Please');
  await page.tap('#mic');
  await page.waitForSelector('.rc-mic.rc-recording');
  await page.waitForTimeout(900);
  await shot(page, '08-dictation-recording');
  await page.tap('#mic');
  await page.waitForSelector('text=Added to the field: read it, then Send.', { timeout: 8000 });
  assert.equal(await page.inputValue('#field'), 'Please please also add an index on the customer id');
  const t = await calls('POST', '/r/voice/transcribe');
  assert.equal(t.length, 1);
  assert.equal(t[0].ct, 'application/octet-stream');
  assert.ok(t[0].bytes > 0 && t[0].bytes % 4 === 0, 'f32 samples');
  await page.waitForTimeout(500);
  assert.equal((await calls('POST', '/r/leaders/website/message')).length, 0, 'dictation never sends');
  await shot(page, '09-dictation-filled');
  await cleanSpy(page);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('unreachable: a banner, the text kept, nothing sent later', { skip }, async () => {
  await ctl({ reset: true, paired: true, down: false });
  const { ctx, page, problems } = await phone();
  await page.goto(`${base()}/`);
  await page.tap('.rc-leader >> text=website');
  await page.waitForSelector('#s-chat:not([hidden])');
  await ctl({ down: true });
  await page.fill('#field', 'kept while down');
  await page.tap('#send');
  await page.waitForSelector('#unreachable:not([hidden])');
  assert.match(await page.textContent('#composer-note'), /unreachable \(is the Mac awake\?\)/);
  assert.equal(await page.inputValue('#field'), 'kept while down');
  await shot(page, '10-unreachable');
  await ctl({ down: false, reset: true });
  // The page looks again by itself (a read), and the banner goes...
  await page.waitForSelector('#unreachable', { state: 'hidden', timeout: 20000 });
  await page.waitForTimeout(1500);
  // ...but nothing it failed to send is sent now.
  assert.equal((await calls('POST', '/r/leaders/website/message')).length, 0, 'nothing queued');
  assert.equal(await page.inputValue('#field'), 'kept while down');
  await cleanSpy(page);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('a 401 or a bye returns to "no longer paired"; Forget asks twice', { skip }, async () => {
  await ctl({ reset: true, paired: true });
  const { ctx, page } = await phone();
  await page.goto(`${base()}/`);
  await page.waitForSelector('.rc-leader');
  await ctl({ emit: { event: 'bye', data: { reason: 'revoked' } } });
  await page.waitForSelector('#s-unpaired:not([hidden])');
  assert.match(await page.textContent('#unpaired-title'), /no longer paired/);
  await ctx.close();

  await ctl({ reset: true, paired: true });
  const b = await phone();
  await b.page.goto(`${base()}/`);
  await b.page.waitForSelector('.rc-leader');
  await ctl({ paired: false });
  await b.page.tap('.rc-leader >> nth=0');
  await b.page.waitForSelector('#s-unpaired:not([hidden])');
  await b.ctx.close();

  await ctl({ reset: true, paired: true });
  const c = await phone();
  await c.page.goto(`${base()}/`);
  await c.page.waitForSelector('.rc-leader');
  await c.page.tap('#menu-btn');
  assert.equal(await c.page.textContent('#menu-device'), 'Test iPhone');
  await shot(c.page, '11-menu');
  await c.page.tap('#forget');
  assert.equal((await calls('POST', '/r/logout')).length, 0, 'the first tap only asks');
  await c.page.tap('#forget');
  await c.page.waitForSelector('#s-unpaired:not([hidden])');
  assert.equal((await calls('POST', '/r/logout')).length, 1);
  await c.ctx.close();
});

test('hostile names and text stay text', { skip }, async () => {
  const evil = '<img src=x onerror="window.__pwned=1">"\'><script>window.__pwned=2</script>';
  const long = 'A'.repeat(20000);
  await ctl({
    reset: true, paired: true,
    leaders: { leaders: [{ sandbox: 'evil', name: evil, state: 'attention', activity: evil, last_at: Date.now(), asks: true, answerable: true, steps: [{ text: evil, options: [evil, '[x](javascript:window.__pwned=3)'] }] }] },
    history: { evil: { page: { items: [
      { seq: 1, kind: 'assistant_text', text: `${evil}\n\n[tap](javascript:window.__pwned=4) [ok](https://example.org/"onclick="window.__pwned=5)\n\n${long}` },
      { seq: 2, kind: 'user_prompt', text: evil },
      { seq: 3, kind: 'injected', from: evil, text: evil },
      { seq: 4, kind: 'injected', text: evil },
      { seq: 5, kind: 'notice', text: evil },
    ], latest_seq: 5 } } },
  });
  const { ctx, page, problems } = await phone(320, 568);
  await page.goto(`${base()}/`);
  await page.waitForSelector('.rc-leader');
  assert.ok(await noHScroll(page));
  await page.tap('.rc-leader');
  await page.waitForSelector('.rc-card-q .rc-opt');
  await page.evaluate(() => document.querySelectorAll('a').forEach(a => a.getAttribute('href')?.startsWith('https:') || a.click()));
  const r = await page.evaluate(() => ({
    pwned: window.__pwned,
    bad: document.querySelectorAll('#rows *:is(img,script,svg,iframe), #tail *:is(img,script,svg,iframe), #leaders *:is(img,script,svg,iframe), [onerror], [onclick]').length,
    hrefs: [...document.querySelectorAll('#rows a')].map(a => a.getAttribute('href')),
  }));
  assert.equal(r.pwned, undefined);
  assert.equal(r.bad, 0, await page.evaluate(() => [...document.querySelectorAll('#rows *:is(img,script,svg,iframe), #tail *:is(img,script,svg,iframe), #leaders *:is(img,script,svg,iframe), [onerror], [onclick]')].map(e => e.outerHTML.slice(0, 200)).join('\n')));
  assert.ok(r.hrefs.every(h => /^https?:\/\//.test(h)), JSON.stringify(r.hrefs));
  assert.ok(await noHScroll(page), 'a 20,000-character message does not widen the page');
  await cleanSpy(page);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('every screen at 320x568 and 390x844: no horizontal scroll, the composer in view', { skip }, async () => {
  for (const [w, h] of [[390, 844], [320, 568]]) {
    await ctl({ reset: true, paired: true, down: false, leaders: FX.leaders, history: { shop: FX.history.shop, website: FX.history.website } });
    const { ctx, page, problems } = await phone(w, h);
    await page.goto(`${base()}/`);
    await page.waitForSelector('.rc-leader');
    assert.ok(await noHScroll(page), `leaders at ${w}`);
    if (w === 320) await shot(page, '12-leaders-320');
    await page.tap('.rc-leader >> text=shop');
    await page.waitForSelector('.rc-card-q');
    assert.ok(await noHScroll(page), `chat at ${w}`);
    const box = await page.$eval('#composer', el => { const r = el.getBoundingClientRect(); return [r.top, r.bottom]; });
    assert.ok(box[1] <= h + 0.5 && box[0] > h / 2, `composer at the bottom: ${box}`);
    if (w === 320) {
      await page.evaluate(() => document.getElementById('log').scrollTo(0, 1e6));
      await shot(page, '13-conversation-320');
    }
    assert.deepEqual(problems, []);
    await ctx.close();
  }
});
