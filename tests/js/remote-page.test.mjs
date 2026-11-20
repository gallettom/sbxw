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
  // `window.__silent`: a muted tone; `window.__noAnalyser`: no AnalyserNode.
  spy.closed = 0;
  spy.stopped = 0;
  spy.tracks = 0;
  const close = AudioContext.prototype.close;
  AudioContext.prototype.close = function () { spy.closed++; return close.call(this); };
  const stop = MediaStreamTrack.prototype.stop;
  MediaStreamTrack.prototype.stop = function () { spy.stopped++; return stop.call(this); };
  if (localStorage.getItem('__noAnalyser')) AudioContext.prototype.createAnalyser = undefined;
  // A fake passkey: `__cred` = cancel | wrong | nosupport, `__nouv` no
  // platform authenticator. Each ceremony is recorded (no secret in it).
  spy.creds = [];
  const cred = localStorage.getItem('__cred') || 'ok';
  const buf = t => new TextEncoder().encode(t).buffer;
  const fail = () => {
    if (cred === 'cancel') throw new DOMException('The operation either timed out or was not allowed.', 'NotAllowedError');
    if (cred === 'nosupport') throw new DOMException('Not supported', 'NotSupportedError');
  };
  if (window.PublicKeyCredential) PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable = async () => !localStorage.getItem('__nouv');
  if (navigator.credentials) {
    navigator.credentials.create = async ({ publicKey: o }) => {
      spy.creds.push({ op: 'create', uv: o.authenticatorSelection.userVerification, attestation: o.attestation, challenge: [...new Uint8Array(o.challenge)], user: [...new Uint8Array(o.user.id)], rp: o.rp.id });
      fail();
      if (cred === 'wrong') return { type: 'public-key', rawId: 'not-bytes', response: {} };
      return { type: 'public-key', rawId: buf('cred-id-12345678'), response: { clientDataJSON: buf('{"type":"webauthn.create"}'), attestationObject: buf('attestation') } };
    };
    navigator.credentials.get = async ({ publicKey: o, signal }) => {
      spy.creds.push({ op: 'get', uv: o.userVerification, rpId: o.rpId, allow: o.allowCredentials.map(c => [...new Uint8Array(c.id)]), challenge: [...new Uint8Array(o.challenge)], signal: !!signal });
      fail();
      if (cred === 'wrong') return { type: 'public-key', rawId: buf('x'), response: { clientDataJSON: buf('{}') } };
      return { type: 'public-key', rawId: buf('cred-id-12345678'), response: { clientDataJSON: buf('{"type":"webauthn.get"}'), authenticatorData: buf('authdata-authdata-authdata-authdata-1'), signature: buf('sig') } };
    };
  }
  // A fake SpeechRecognition: `__sr` = ok (default) | missing | ctor | start
  // | err:<code> | dead (never says it started) | deaf (stop() never ends).
  // The test says words with window.__srLast.say([[text, final]]); endSelf()
  // ends it as iOS does after a pause. spy.srMade counts the recognisers.
  spy.sr = [];
  spy.srMade = 0;
  const sr = localStorage.getItem('__sr') || 'ok';
  delete window.SpeechRecognition;
  delete window.webkitSpeechRecognition;
  if (sr !== 'missing') {
    window.webkitSpeechRecognition = class {
      constructor() { if (sr === 'ctor') throw new Error('no'); window.__srLast = this; spy.srMade++; }
      start() {
        spy.sr.push(`start ${this.lang} ${this.interimResults} ${this.continuous}`);
        if (sr === 'start') throw new DOMException('no', 'InvalidStateError');
        if (sr !== 'dead') setTimeout(() => { this.onstart?.(); this.onaudiostart?.(); }, 10);
        if (sr.startsWith('err:')) setTimeout(() => { this.onerror?.({ error: sr.slice(4) }); this.onend?.(); }, 60);
      }
      stop() { spy.sr.push('stop'); if (sr !== 'deaf') setTimeout(() => this.onend?.(), 20); }
      endSelf() { this.onend?.(); }
      sound() { this.onsoundstart?.(); }
      abort() { spy.sr.push('abort'); setTimeout(() => this.onend?.(), 0); }
      say(list) {
        const results = list.map(([t, f]) => ({ 0: { transcript: t }, length: 1, isFinal: f }));
        results.item = i => results[i];
        this.onresult?.({ resultIndex: 0, results });
      }
      fail(code) { this.onerror?.({ error: code }); this.onend?.(); }
    };
  }
  navigator.mediaDevices.getUserMedia = async () => {
    const ctx = new AudioContext();
    const osc = ctx.createOscillator();
    const gain = ctx.createGain();
    gain.gain.value = localStorage.getItem('__silent') ? 0 : 0.8;
    const dest = ctx.createMediaStreamDestination();
    osc.connect(gain).connect(dest);
    osc.start();
    spy.tracks += dest.stream.getTracks().length;
    window.__stream = dest.stream;
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

async function phone(width = 390, height = 844, opts = {}) {
  const ctx = await browser.newContext({ viewport: { width, height }, deviceScaleFactor: 2, isMobile: true, hasTouch: true, userAgent: IPHONE_UA, reducedMotion: opts.reducedMotion || 'no-preference' });
  // Test levers for the fake microphone, set before the page's scripts (they
  // are the test's, never the page's: cleanSpy ignores keys starting __).
  if (opts.flags) await ctx.addInitScript(f => { for (const x of f) { const i = x.indexOf(':'); localStorage.setItem(i < 0 ? x : x.slice(0, i), i < 0 ? '1' : x.slice(i + 1)); } }, opts.flags);
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
const shot = async (page, name) => { if (SHOTS) await page.screenshot({ path: `${SHOTS}/${name}.png`, animations: 'disabled' }); };
const noHScroll = page => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth && document.body.scrollWidth <= window.innerWidth);
async function cleanSpy(page) {
  const spy = await page.evaluate(() => window.__spy);
  assert.deepEqual(spy.csp, [], 'no CSP violation');
  assert.deepEqual(spy.console, [], 'no console output');
  assert.equal(spy.idb + spy.caches + spy.sw, 0, 'no IndexedDB, Cache API or service worker');
  for (const [area, key, value] of spy.storage.filter(([, k]) => !k.startsWith('__'))) {
    assert.equal(`${area} ${key}`, 'local sbxw-remote-prefs', 'only the UI preferences are stored');
    const v = JSON.parse(value);
    for (const k of Object.keys(v)) assert.ok(['device_name', 'dictation'].includes(k), `stored: ${k}`);
    if ('dictation' in v) assert.equal(v.dictation, 'iphone');
    for (const w of SECRET_WORDS) assert.ok(!value.includes(w));
  }
  const stored = await page.evaluate(() => JSON.stringify(Object.fromEntries(Object.entries(localStorage).filter(([k]) => !k.startsWith('__')))) + JSON.stringify({ ...sessionStorage }));
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
  assert.equal(await page.isDisabled('#send'), true, 'nothing to send: Send rests');

  // Interrupt lives in the bar, only while the leader works.
  assert.equal(await page.isVisible('#interrupt'), false, 'an idle leader shows no Interrupt');
  const working = structuredClone(FX.leaders);
  working.leaders.find(l => l.sandbox === 'website').state = 'working';
  await ctl({ emit: { event: 'leaders', data: working } });
  await page.waitForSelector('#interrupt:visible');
  assert.equal(await page.textContent('#state'), 'working');
  await page.tap('#interrupt');
  assert.equal(await page.textContent('#confirm-text'), 'Interrupt website?');
  assert.equal((await calls('POST', '/r/leaders/website/interrupt')).length, 0, 'the first tap only asks');
  await shot(page, '07-interrupt-confirm');
  await page.tap('#interrupt-cancel');
  assert.equal(await page.isVisible('#confirm'), false);
  await page.tap('#interrupt');
  await page.tap('#interrupt-go');
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
  assert.equal((await page.textContent('#send')).trim(), 'Sending…');
  assert.equal(await page.isDisabled('#send'), true);
  await page.evaluate(() => document.getElementById('send').click());
  await page.waitForSelector('text=Sent.', { timeout: 6000 });
  assert.equal((await calls('POST', '/r/leaders/website/message')).length, 1, 'one tap, one message');
  assert.equal((await page.textContent('#send')).trim(), 'Send');
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
  await page.waitForSelector('#status.rc-t-warn');
  assert.match(await page.textContent('#status'), /NOT SENT: the text was typed into the leader's input on the Mac but not submitted.*\(sbxw: the leader changed before the Return\)/);
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
  await page.fill('#field', 'hello leader');
  assert.equal(await page.isDisabled('#send'), false);
  const working = structuredClone(FX.leaders);
  working.leaders.find(l => l.sandbox === 'website').state = 'working';
  working.leaders.find(l => l.sandbox === 'website').activity = 'Working';
  await ctl({ emit: { event: 'leaders', data: working } });
  await page.waitForSelector('#status:text("The leader is working: Send comes back at its prompt.")');
  assert.equal(await page.isDisabled('#send'), true);
  await page.evaluate(() => document.getElementById('send').click());
  assert.equal(await page.isVisible('#interrupt'), true, 'it can still be interrupted, from the bar');
  assert.equal(await page.isDisabled('#interrupt'), false);
  assert.match(await page.getAttribute('#state', 'class'), /rc-pill-working/);
  await shot(page, '16-working-send-waits');
  await ctl({ emit: { event: 'leaders', data: FX.leaders } });
  await page.waitForSelector('#interrupt', { state: 'hidden' });
  assert.equal(await page.textContent('#status'), '');
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
  await page.waitForSelector('#composer[data-rec="recording"]');
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
  assert.match(await page.textContent('#status'), /unreachable \(is the Mac awake\?\)/);
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

// ── Files ──
const MB = 1024 * 1024;
const fileOf = (name, size, mimeType = 'application/octet-stream') => ({ name, mimeType, buffer: Buffer.alloc(size, 7) });
async function pick(page, files) {
  const [chooser] = await Promise.all([page.waitForEvent('filechooser'), page.tap('#attach')]);
  assert.equal(chooser.isMultiple(), true, 'several files at once');
  await chooser.setFiles(files);
}
const uploads = async () => (await log()).filter(e => e.method === 'POST' && /\/upload$/.test(e.path));

test('files: checked before any byte, the tick, progress, failure and retry, remove, Send carries the ids', { skip }, async () => {
  await ctl({ reset: true, paired: true, leaders: FX.leaders, history: { website: FX.history.website }, uploadSlow: 0, uploadReplies: [] });
  const { ctx, page, problems } = await phone();
  await page.goto(`${base()}/`);
  await page.tap('.rc-leader >> text=website');
  await page.waitForSelector('#s-chat:not([hidden])');
  // Empty: refused in the page, nothing sent.
  await pick(page, [fileOf('empty.txt', 0)]);
  await page.waitForSelector('#status:text("Not attached: empty.txt (the file is empty).")');
  assert.equal((await uploads()).length, 0, 'no byte for an empty file');
  // A secret-looking name waits for the tick.
  await pick(page, [fileOf('.env', 64)]);
  await page.waitForSelector('#held:not([hidden])');
  assert.match(await page.textContent('#held-why'), /^\.env: its name looks like a secret/);
  await shot(page, '22-files-tick');
  await page.tap('#held-go');
  await page.waitForSelector('#status:text-matches("^Tick \\"I have looked")');
  assert.equal((await uploads()).length, 0, 'no byte before the tick');
  await page.tap('#held-cancel');
  await page.waitForSelector('#held', { state: 'hidden' });
  assert.equal((await uploads()).length, 0);
  // Progress: a slow body, Send waits.
  await ctl({ uploadSlow: 40 });
  await page.fill('#field', 'see these');
  await pick(page, [fileOf('photo.jpg', 18 * MB, 'image/jpeg'), fileOf('scan.pdf', 2 * MB, 'application/pdf')]);
  await page.waitForSelector('.rc-fchip[data-state="sending"]');
  await page.waitForFunction(() => Number(document.querySelector('.rc-fchip-bar')?.getAttribute('aria-valuenow')) > 20, null, { timeout: 15000 });
  assert.equal(await page.isDisabled('#send'), true, 'Send waits for the uploads');
  assert.match(await page.textContent('#status'), /^Uploading 2 files: Send waits until they are done\.$/);
  await shot(page, '21-files-progress');
  await ctl({ uploadSlow: 0 });
  await page.waitForFunction(() => document.querySelectorAll('.rc-fchip[data-state="done"]').length === 2, null, { timeout: 30000 });
  // A refusal on the chip, with Retry; the typed text is untouched.
  await ctl({ uploadReplies: [{ status: 400, body: { ok: false, reason: 'kind', error: 'an archive' } }] });
  await pick(page, [fileOf('bundle.zip', 100)]);
  await page.waitForSelector('.rc-fchip[data-state="failed"]');
  assert.match(await page.textContent('.rc-f-failed .rc-fchip-meta'), /Programs and archives are not accepted/);
  assert.equal(await page.inputValue('#field'), 'see these');
  assert.equal(await page.isDisabled('#send'), false, 'a failed file never blocks the rest');
  await shot(page, '23-files-refused');
  await page.tap('.rc-f-failed .rc-retry');
  await page.waitForFunction(() => document.querySelectorAll('.rc-fchip[data-state="done"]').length === 3);
  // A body cut on a slow link, in its two shapes: one sentence, Retry, never by itself.
  const SLOW = 'The connection was too slow for this file. Try again on a better connection, or send a smaller file.';
  await ctl({ uploadReplies: [{ status: 400, body: { ok: false, error: 'the upload was cut off' } }, { status: 502, raw: 'Bad Gateway' }] });
  for (const name of ['cut.mov', 'sidecar.mov']) {
    const n = (await uploads()).length;
    await pick(page, [fileOf(name, 100)]);
    await page.waitForSelector('.rc-fchip[data-state="failed"]');
    assert.equal(await page.textContent('.rc-f-failed .rc-fchip-meta'), SLOW, name);
    await page.waitForTimeout(400);
    assert.equal((await uploads()).length, n + 1, 'no automatic retry');
    await page.tap('.rc-f-failed .rc-remove');
    await page.waitForSelector('.rc-fchip[data-state="failed"]', { state: 'detached' });
  }
  // Remove one that went up: the Mac drops it.
  const before = (await uploads()).length;
  await page.tap('.rc-fchip >> nth=1 >> .rc-remove');
  await page.waitForFunction(() => document.querySelectorAll('.rc-fchip').length === 2);
  await page.waitForTimeout(200);
  assert.equal((await log()).filter(e => e.method === 'DELETE').length, 1);
  assert.equal((await uploads()).length, before, 'removing uploads nothing');
  // Uploads: raw octet-stream, the name in the query, no JSON, no path back.
  for (const u of await uploads()) {
    assert.equal(u.ct, 'application/octet-stream');
    assert.match(u.query, /^\?name=[^&]+$/);
  }
  assert.equal((await calls('POST', '/r/leaders/website/message')).length, 0, 'attaching sends no message');
  await page.tap('#send');
  await page.waitForSelector('#status:text("Sent.")');
  const msg = (await calls('POST', '/r/leaders/website/message'))[0];
  assert.deepEqual(Object.keys(msg.body).sort(), ['attachments', 'text']);
  assert.equal(msg.body.text, 'see these');
  assert.equal(msg.body.attachments.length, 2);
  for (const id of msg.body.attachments) assert.match(id, /^[0-9a-f]{32}$/, 'an id, never a path');
  assert.doesNotMatch(JSON.stringify(msg.body), /\/tmp|photo|scan|bundle/, 'no path and no name in the message');
  await page.waitForSelector('#tray', { state: 'hidden' });
  await cleanSpy(page);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('files while the leader works: attaching allowed, sending not, and only files can be sent', { skip }, async () => {
  const working = structuredClone(FX.leaders);
  working.leaders.find(l => l.sandbox === 'website').state = 'working';
  await ctl({ reset: true, paired: true, leaders: working, history: { website: FX.history.website }, uploadSlow: 0, uploadReplies: [] });
  const { ctx, page, problems } = await phone();
  await page.goto(`${base()}/`);
  await page.tap('.rc-leader >> text=website');
  await pick(page, [fileOf('notes.txt', 300, 'text/plain')]);
  await page.waitForSelector('.rc-fchip[data-state="done"]');
  assert.equal(await page.textContent('#status'), 'The leader is working: Send comes back at its prompt. Your files wait here.');
  assert.equal(await page.isDisabled('#send'), true);
  await ctl({ emit: { event: 'leaders', data: FX.leaders } });
  await page.waitForFunction(() => !document.getElementById('send').disabled);
  await page.tap('#send');
  await page.waitForSelector('#status:text("Sent.")');
  const msg = (await calls('POST', '/r/leaders/website/message'))[0];
  assert.equal(msg.body.text, '', 'files alone may go');
  assert.equal(msg.body.attachments.length, 1);
  await cleanSpy(page);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('hostile file names stay text', { skip }, async () => {
  await ctl({ reset: true, paired: true, leaders: FX.leaders, history: { website: FX.history.website }, uploadSlow: 0, uploadReplies: [] });
  const { ctx, page, problems } = await phone(320, 568);
  await page.goto(`${base()}/`);
  await page.tap('.rc-leader >> text=website');
  const evil = ['<img src=x onerror="window.__pwned=1">.png', '"><svg onload=window.__pwned=2>.txt', '\u202Egnp.exe', 'a'.repeat(400) + '.jpeg'];
  await pick(page, evil.map(n => fileOf(n, 10)));
  await page.waitForFunction(() => document.querySelectorAll('.rc-fchip[data-state="done"]').length === 4, null, { timeout: 10000 });
  const r = await page.evaluate(() => ({
    pwned: window.__pwned,
    bad: document.querySelectorAll('#tray img, #tray script, #tray iframe, #chips svg[onload], [onerror], [onload]').length,
    names: [...document.querySelectorAll('.rc-fchip-name')].map(e => e.textContent),
  }));
  assert.equal(r.pwned, undefined);
  assert.equal(r.bad, 0);
  assert.equal(r.names[0], evil[0]);
  assert.equal(r.names[2], '?gnp.exe', 'a bidi control is shown as ?');
  assert.ok(r.names[3].length <= 60, 'a long name is cut in the middle');
  assert.ok(await noHScroll(page));
  for (const u of await uploads()) assert.doesNotMatch(decodeURIComponent(u.query), /[\u0000-\u001f]/);
  await cleanSpy(page);
  assert.deepEqual(problems, []);
  await ctx.close();
});

// ── The recorder ──
async function recordOnce(page, { hold = 900 } = {}) {
  await page.tap('#mic');
  await page.waitForSelector('#composer[data-rec="recording"]');
  await page.waitForTimeout(hold);
}
test('the recorder: a live wave, a clock, stop, transcribing, filled; the context and tracks closed', { skip }, async () => {
  await ctl({ reset: true, paired: true, leaders: FX.leaders, history: { website: FX.history.website }, transcribeDelay: 800 });
  const { ctx, page, problems } = await phone();
  await page.goto(`${base()}/`);
  await page.tap('.rc-leader >> text=website');
  await recordOnce(page, { hold: 1300 });
  assert.equal(await page.getAttribute('#composer', 'data-wave'), 'bars');
  assert.ok(Number(await page.getAttribute('#rec', 'data-level')) > 0.1, 'the wave moves with the sound');
  assert.match(await page.textContent('#rec-time'), /^0:0[1-2]$/);
  assert.equal(await page.getAttribute('#mic', 'aria-label'), 'Stop dictation');
  assert.equal(await page.getAttribute('#mic', 'aria-pressed'), 'true');
  assert.equal(await page.isVisible('#field'), false, 'the wave takes the field\'s place');
  assert.equal(await page.isDisabled('#send'), true, 'nothing is sent while it records');
  assert.match(await page.textContent('#status'), /^Recording\./);
  await shot(page, '17-recording-wave');
  await page.tap('#mic');
  await page.waitForSelector('#composer[data-rec="transcribing"]');
  assert.equal(await page.textContent('#status'), 'Transcribing on your Mac…');
  await shot(page, '18-transcribing');
  await page.waitForSelector('#composer[data-rec="idle"]', { timeout: 8000 });
  assert.equal(await page.inputValue('#field'), 'please also add an index on the customer id');
  assert.equal(await page.textContent('#status'), 'Added to the field: read it, then Send.');
  await shot(page, '19-dictation-filled');
  const spy = await page.evaluate(() => window.__spy);
  assert.ok(spy.stopped >= spy.tracks && spy.tracks >= 1, `tracks stopped: ${spy.stopped} of ${spy.tracks}`);
  assert.ok(spy.closed >= 2, `the wave's AudioContext and the decoder's closed: ${spy.closed}`);
  assert.equal((await calls('POST', '/r/leaders/website/message')).length, 0, 'dictation never sends');
  await ctl({ transcribeDelay: 0 });
  await cleanSpy(page);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('the recorder: silence is flat; reduced motion draws a meter; no analyser still records', { skip }, async () => {
  await ctl({ reset: true, paired: true, leaders: FX.leaders, history: { website: FX.history.website } });
  {
    const { ctx, page } = await phone(390, 844, { flags: ['__silent'] });
    await page.goto(`${base()}/`);
    await page.tap('.rc-leader >> text=website');
    await recordOnce(page);
    assert.equal(await page.getAttribute('#rec', 'data-level'), '0.00', 'flat when silent');
    await page.tap('#mic');
    await page.waitForSelector('#composer[data-rec="idle"]', { timeout: 8000 });
    await ctx.close();
  }
  {
    const { ctx, page, problems } = await phone(390, 844, { reducedMotion: 'reduce' });
    await page.goto(`${base()}/`);
    await page.tap('.rc-leader >> text=website');
    await recordOnce(page);
    assert.equal(await page.getAttribute('#composer', 'data-wave'), 'meter');
    assert.ok(Number(await page.getAttribute('#rec', 'data-level')) > 0.1);
    await shot(page, '20-recording-reduced-motion');
    await page.tap('#mic');
    await page.waitForSelector('text=Added to the field: read it, then Send.', { timeout: 8000 });
    assert.deepEqual(problems, []);
    await ctx.close();
  }
  {
    const { ctx, page, problems } = await phone(390, 844, { flags: ['__noAnalyser'] });
    await page.goto(`${base()}/`);
    await page.tap('.rc-leader >> text=website');
    await recordOnce(page);
    assert.equal(await page.getAttribute('#composer', 'data-wave'), 'pulse');
    await page.tap('#mic');
    await page.waitForSelector('text=Added to the field: read it, then Send.', { timeout: 8000 });
    assert.match(await page.inputValue('#field'), /customer id/, 'recording never depends on the wave');
    const spy = await page.evaluate(() => window.__spy);
    assert.ok(spy.stopped >= spy.tracks);
    assert.deepEqual(problems, []);
    await ctx.close();
  }
});

test('the recorder: a hidden page stops it and sends nothing', { skip }, async () => {
  await ctl({ reset: true, paired: true, leaders: FX.leaders, history: { website: FX.history.website } });
  const { ctx, page, problems } = await phone();
  await page.goto(`${base()}/`);
  await page.tap('.rc-leader >> text=website');
  await recordOnce(page);
  await page.evaluate(() => {
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'hidden' });
    document.dispatchEvent(new Event('visibilitychange'));
  });
  await page.waitForSelector('#composer[data-rec="idle"]');
  await page.waitForTimeout(500);
  assert.equal((await calls('POST', '/r/voice/transcribe')).length, 0, 'no audio leaves the phone');
  assert.match(await page.textContent('#status'), /page was hidden\. Nothing was sent\./);
  const spy = await page.evaluate(() => window.__spy);
  assert.ok(spy.stopped >= spy.tracks && spy.closed >= 1);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('the conversation at rest, at 430x932, and the screens at three widths', { skip }, async () => {
  for (const [w, h] of [[430, 932], [390, 844], [320, 568]]) {
    await ctl({ reset: true, paired: true, leaders: FX.leaders, history: { shop: FX.history.shop, website: FX.history.website } });
    const { ctx, page, problems } = await phone(w, h);
    await page.goto(`${base()}/`);
    await page.waitForSelector('.rc-leader');
    await shot(page, `30-leaders-${w}`);
    await page.tap('.rc-leader >> text=website');
    await page.waitForSelector('.rc-agent');
    await page.evaluate(() => document.getElementById('log').scrollTo(0, 1e6));
    await page.waitForTimeout(300);
    assert.ok(await noHScroll(page));
    const box = await page.$eval('#composer', el => el.getBoundingClientRect().bottom);
    assert.ok(Math.abs(box - h) < 1, `the composer's ground runs to the bottom edge: ${box} of ${h}`);
    await shot(page, `31-conversation-${w}`);
    assert.deepEqual(problems, []);
    await ctx.close();
  }
});

// ══ Level 2: approving with Face ID ═══════════════════════════════════════
const L2 = FX.level2;
const l2calls = async () => (await log()).filter(e => /^\/r\/(approvals|passkey|plans|merges|network|stopped)/.test(e.path));
const actCalls = async () => (await log()).filter(e => e.method === 'POST' && /\/(approve|refuse|merge|respond|remove)$/.test(e.path));
const creds = page => page.evaluate(() => window.__spy.creds);
const b64 = t => Buffer.from(t).toString('base64url');
async function openApprovals(page, from = 'leaders') {
  await page.goto(`${base()}/`);
  await page.waitForSelector('.rc-leader');
  if (from !== 'leaders') { await page.tap(`.rc-leader >> text=${from}`); await page.waitForSelector('#s-chat:not([hidden])'); }
  await page.tap('#approvals-btn');
  await page.waitForSelector('#s-approvals:not([hidden])');
}

test('level 2: the Approvals entry, its badge, and one sentence when it cannot act', { skip }, async () => {
  await ctl({ reset: true, paired: true, leaders: FX.leaders, l2Reset: true });
  const { ctx, page, problems } = await phone();
  await page.goto(`${base()}/`);
  await page.waitForSelector('#badge:not([hidden])');
  assert.equal(await page.textContent('#badge'), '4');
  assert.equal(await page.getAttribute('#approvals-btn', 'aria-label'), 'Approvals, 4 waiting');
  await ctl({ emit: { event: 'approvals', data: { plans: 2, merges: 1, network: 0, stopped: 3 } } });
  await page.waitForSelector('#badge:text("6")');
  await shot(page, '40-approvals-entry');
  // From a leader's bar too.
  await page.tap('.rc-leader >> text=website');
  assert.equal(await page.isVisible('#approvals-btn'), true);
  await page.tap('#approvals-btn');
  await page.waitForSelector('.rc-ap-row');
  assert.deepEqual(await page.$$eval('.rc-ap-h', h => h.map(e => e.textContent)), ['Plans to approve 1', 'Merges proposed 1', 'Network requests 1', 'Stopped sandboxes 1']);
  await shot(page, '41-approvals-list');
  // A list sbx cannot read says so in its place; the others stay.
  await ctl({ l2Replies: { network: [{ status: 503, body: { ok: false, reason: 'unavailable' } }] } });
  await page.tap('#back');
  await page.tap('#approvals-btn');
  await page.waitForSelector('.rc-ap-fail');
  assert.equal(await page.textContent('.rc-ap-fail'), 'The Mac cannot read this right now (sbx did not answer). Try again in a moment.');
  assert.equal(await page.$$eval('.rc-ap-row', r => r.length), 3);
  await page.tap('#back');
  await page.waitForSelector('#s-chat:not([hidden])');
  assert.equal(await page.textContent('#title'), 'website', 'back to the conversation it came from');
  assert.equal((await actCalls()).length, 0);
  await ctx.close();
  // Off: one sentence, nothing else.
  await ctl({ reset: true, l2Reset: true, l2: { approvals: { level2: false } } });
  {
    const { ctx, page } = await phone();
    await openApprovals(page);
    await page.waitForSelector('#ap-block:not([hidden])');
    assert.equal(await page.textContent('#ap-block-text'), 'Approving is turned off on your Mac.');
    assert.equal(await page.isVisible('#ap-enable'), false);
    assert.equal(await page.$$eval('.rc-ap-row', r => r.length), 0);
    assert.equal(await page.isVisible('#badge'), false);
    assert.deepEqual((await l2calls()).map(e => e.path), ['/r/approvals', '/r/approvals'], 'no list is read while it is off');
    await ctx.close();
  }
  // No passkey: the lists, read only, and Enable Face ID.
  await ctl({ reset: true, l2Reset: true, l2: { approvals: { passkey: false } } });
  {
    const { ctx, page, problems: p2 } = await phone();
    await openApprovals(page);
    await page.waitForSelector('.rc-ap-row');
    assert.equal(await page.textContent('#ap-block-text'), 'Approving needs Face ID: enable it from your Mac.');
    assert.equal(await page.isVisible('#ap-enable'), true);
    await shot(page, '42-approvals-no-passkey');
    await page.tap('.rc-ap-plans');
    await page.waitForSelector('.rc-thread');
    assert.equal(await page.$$eval('#item-acts button', b => b.length), 0, 'no decision without a passkey');
    assert.equal(await page.textContent('#item-acts'), 'Approving needs Face ID: enable it from your Mac.');
    await cleanSpy(page);
    assert.deepEqual(p2, []);
    await ctx.close();
  }
  assert.deepEqual(problems, []);
});

test('level 2: enabling Face ID: the digits, the grant, create, register; and each way it stops', { skip }, async () => {
  await ctl({ reset: true, paired: true, leaders: FX.leaders, l2Reset: true, l2: { approvals: { passkey: false } }, pkStates: ['pending', 'granted'] });
  const { ctx, page, problems } = await phone();
  await openApprovals(page);
  await page.tap('#ap-enable');
  await page.waitForSelector('#pk-check:not([hidden])');
  assert.equal(await page.textContent('#pk-check'), '305 718');
  assert.equal(await page.textContent('#pk-title'), 'Type this number on your Mac');
  await shot(page, '43-faceid-digits');
  await page.waitForSelector('#pk-go:not([hidden])', { timeout: 10000 });
  assert.equal((await creds(page)).length, 0, 'the key is created on a tap only');
  await page.tap('#pk-go');
  await page.waitForSelector('#s-passkey[data-state="registered"]');
  const [c] = await creds(page);
  assert.equal(c.op, 'create');
  assert.equal(c.uv, 'required');
  assert.equal(c.attestation, 'none');
  assert.deepEqual(c.challenge, [...Array(32).keys()], 'the challenge decoded from base64url');
  assert.deepEqual(c.user, [...Buffer.from('dev-1')]);
  const reg = (await calls('POST', '/r/passkey/register'))[0].body;
  const rid = reg.request_id;
  assert.match(rid, /^[0-9a-f]{32}$/, 'the request_id of start');
  assert.deepEqual(reg, { request_id: rid, id: b64('cred-id-12345678'), client_data_json: b64('{"type":"webauthn.create"}'), attestation_object: b64('attestation') });
  for (const v of Object.values(reg)) assert.match(v, /^[A-Za-z0-9_-]+$/, 'base64url, no padding');
  // The same id on every step, in the query for status, in the body for options.
  assert.ok((await calls('GET', '/r/passkey/status')).every(e => e.query === `?request_id=${rid}`));
  assert.deepEqual((await calls('POST', '/r/passkey/options'))[0].body, { request_id: rid });
  // Set up is read from /r/approvals, not from status.
  assert.ok((await calls('GET', '/r/approvals')).length >= 2);
  // Kept nowhere but in memory.
  assert.ok(!(await page.evaluate(() => JSON.stringify({ ...localStorage }) + JSON.stringify({ ...sessionStorage }) + location.href)).includes(rid));
  await shot(page, '44-faceid-ready');
  await cleanSpy(page);
  assert.deepEqual(problems, []);
  await ctx.close();
  for (const [flags, states, want] of [
    [['__cred:cancel'], ['granted'], 'cancelled'],
    [['__nouv'], ['granted'], 'no_authenticator'],
    [['__cred:nosupport'], ['granted'], 'no_authenticator'],
    [['__cred:wrong'], ['granted'], 'failed'],
    [[], ['refused'], 'refused'],
    [['__reg:no_grant'], ['granted'], 'expired'],
    [['__reg:passkey'], ['granted'], 'rejected'],
  ]) {
    const reg = flags.find(f => f.startsWith('__reg:'))?.slice(6);
    await ctl({ reset: true, l2Reset: true, l2: { approvals: { passkey: false } }, pkStates: states, ...(reg ? { l2Replies: { register: [{ status: 403, body: { ok: false, reason: reg } }] } } : {}) });
    const { ctx, page } = await phone(390, 844, { flags });
    await openApprovals(page);
    await page.tap('#ap-enable');
    if (states[0] === 'granted') { await page.waitForSelector('#pk-go:not([hidden])', { timeout: 10000 }); await page.tap('#pk-go'); }
    await page.waitForSelector(`#s-passkey[data-state="${want}"]`, { timeout: 10000 });
    assert.equal(await page.textContent('#pk-text'), want === 'failed' ? 'The browser gave an answer this page cannot read. Nothing was saved.' : await page.evaluate(w => RC_PASSKEY_TEXT[w], want));
    assert.equal((await calls('POST', '/r/passkey/register')).length, reg ? 1 : 0, `${flags} ${states}: nothing registered`);
    assert.equal(await page.evaluate(() => window.__spy.creds.filter(c => c.op === 'create').length), states[0] === 'granted' && !flags.includes('__nouv') ? 1 : 0);
    assert.equal(await page.isVisible('#pk-again'), true);
    await ctx.close();
  }
});

async function openItemOf(page, cls) {
  await page.tap(`.rc-ap-${cls}`);
  await page.waitForSelector('#item-acts button');
}
async function viaSheet(page, button) {
  await page.tap(button);
  await page.waitForSelector('#sheet:not([hidden])');
  await page.tap('#sheet-go');
}

test('level 2: a plan folded and unfolded, the sheet first, then Face ID, then the action', { skip }, async () => {
  await ctl({ reset: true, paired: true, leaders: FX.leaders, l2Reset: true });
  const { ctx, page, problems } = await phone();
  await openApprovals(page);
  await openItemOf(page, 'plans');
  assert.equal(await page.textContent('.rc-item-title'), 'Orders table, then the checkout flow');
  assert.equal(await page.$$eval('.rc-folded', e => e.length), 1, 'the long goal is folded');
  assert.match(await page.textContent('.rc-item-warn'), /^2 uncommitted changes in the main checkout\.$/);
  await shot(page, '45-plan-folded');
  await page.tap('.rc-fold');
  assert.equal(await page.$$eval('.rc-folded', e => e.length), 0);
  assert.equal(await page.textContent('.rc-fold'), 'Fold');
  await page.evaluate(() => document.getElementById('s-item').scrollTo(0, 200));
  await shot(page, '46-plan-unfolded');
  const before = (await l2calls()).length;
  // A tap on Approve only opens the sheet: no request.
  await page.tap('[data-kind="plan_approve"]');
  await page.waitForSelector('#sheet:not([hidden])');
  assert.equal(await page.textContent('#sheet-line'), 'Approve the plan of shop as it is: it starts 2 threads.');
  assert.equal(await page.textContent('#sheet-go'), 'Approve with Face ID');
  assert.equal((await l2calls()).length, before, 'opening the sheet sends nothing');
  await shot(page, '47-approve-sheet');
  await page.tap('#sheet-cancel');
  await page.waitForSelector('#sheet', { state: 'hidden' });
  assert.equal((await l2calls()).length, before, 'cancel sends nothing');
  // Refuse is a different, red sheet.
  await page.tap('[data-kind="plan_refuse"]');
  assert.match(await page.getAttribute('#sheet-go', 'class'), /rc-danger-solid/);
  await page.tap('#sheet-cancel');
  // Approve: challenge, Face ID, the action, in that order.
  await viaSheet(page, '[data-kind="plan_approve"]');
  await page.waitForSelector('#ap-done:not([hidden])');
  assert.equal(await page.textContent('#ap-done'), 'Plan approved: its threads are starting.');
  const seq = (await l2calls()).slice(before).filter(e => e.method === 'POST').map(e => e.path);
  assert.deepEqual(seq, ['/r/approvals/challenge', `/r/plans/p-${'1'.repeat(32)}/approve`]);
  const ch = (await calls('POST', '/r/approvals/challenge')).at(-1).body;
  assert.deepEqual(ch, { kind: 'plan_approve', target: `p-${'1'.repeat(32)}`, digest: 'a'.repeat(64) });
  const g = (await creds(page)).at(-1);
  assert.equal(g.op, 'get');
  assert.equal(g.uv, 'required');
  assert.equal(g.rpId, 'sbxw.tail1234.ts.net');
  assert.deepEqual(g.allow, [[...Buffer.from('cred-id-12345678')]], 'the device\'s credential only');
  assert.equal(g.challenge.length, 32);
  const act = (await actCalls()).at(-1).body;
  assert.deepEqual(act, { assertion: { id: b64('cred-id-12345678'), client_data_json: b64('{"type":"webauthn.get"}'), authenticator_data: b64('authdata-authdata-authdata-authdata-1'), signature: b64('sig') } });
  await cleanSpy(page);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('level 2: a merge in numbers and paths, its result in the daemon\'s words', { skip }, async () => {
  await ctl({ reset: true, paired: true, leaders: FX.leaders, l2Reset: true });
  const { ctx, page, problems } = await phone();
  await openApprovals(page);
  await openItemOf(page, 'merges');
  assert.deepEqual(await page.$$eval('.rc-order li', l => l.map(e => e.textContent)), ['footer', 'contact-form']);
  assert.equal(await page.textContent('.rc-branch .rc-thread-meta'), '1a2b3c4');
  assert.match(await page.textContent('.rc-branch-stat'), /^3 commits · 3 files \+120 −8$/);
  await page.tap('.rc-files-box summary');
  assert.deepEqual(await page.$$eval('.rc-file-path', l => l.slice(0, 3).map(e => e.textContent)), ['src/components/Footer.tsx', 'src/pages/legal/index.tsx', 'public/logo.png']);
  assert.equal(await page.textContent('.rc-files li:nth-child(3) .rc-file-n'), 'binary');
  assert.deepEqual(await page.$$eval('.rc-branch .rc-item-warn', w => w.map(e => e.textContent)), ['An earlier attempt stopped at a conflict on this branch.']);
  await shot(page, '48-merge-detail');
  await viaSheet(page, '[data-kind="merge"]');
  await page.waitForSelector('#ap-done:not([hidden])');
  assert.equal(await page.textContent('#ap-done'), 'Merged: footer, contact-form.');
  await ctl({ l2Replies: { merge: [{ status: 200, body: L2.merge_conflict }] } });
  await openItemOf(page, 'merges');
  await viaSheet(page, '[data-kind="merge"]');
  await page.waitForSelector('#ap-done:text-matches("^Stopped")');
  assert.equal(await page.textContent('#ap-done'), 'Stopped at contact-form: conflict in src/pages/contact.tsx, src/components/Footer.tsx');
  await shot(page, '49-merge-conflict');
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('level 2: a network request offers only the daemon\'s options; a stopped sandbox shows its own sentences', { skip }, async () => {
  await ctl({ reset: true, paired: true, leaders: FX.leaders, l2Reset: true });
  const { ctx, page, problems } = await phone();
  await openApprovals(page);
  await openItemOf(page, 'network');
  assert.deepEqual(await page.$$eval('#item-acts button', b => b.map(e => e.textContent)), ['Dismiss'], 'dismiss only');
  assert.equal(await page.textContent('.rc-item-note'), 'Allowing this needs your Mac.');
  assert.match(await page.getAttribute('#item-acts button', 'class'), /rc-danger/);
  await shot(page, '50-network-dismiss-only');
  await viaSheet(page, '#item-acts button');
  await page.waitForSelector('#ap-done:text("Answered.")');
  assert.deepEqual((await actCalls()).at(-1).body.option, 'dismiss');
  assert.deepEqual((await calls('POST', '/r/approvals/challenge')).at(-1).body.option, 'dismiss');
  await openItemOf(page, 'stopped');
  assert.deepEqual(await page.$$eval('.rc-confirm-lines p', l => l.map(e => e.textContent)), L2.stopped_one.confirm, 'verbatim');
  await page.tap('[data-kind="sandbox_remove"]');
  await page.waitForSelector('#sheet:not([hidden])');
  assert.equal(await page.textContent('#sheet-line'), 'Remove the stopped sandbox old-experiment. This cannot be undone.');
  assert.match(await page.getAttribute('#sheet-go', 'class'), /rc-danger-solid/);
  await shot(page, '51-remove-sheet');
  await page.tap('#sheet-go');
  await page.waitForSelector('#ap-done:text("Sandbox removed.")');
  assert.equal((await actCalls()).at(-1).path, '/r/stopped/old-experiment/remove');
  await cleanSpy(page);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('level 2: "changed", every refusal, a cancelled or odd Face ID: nothing done, said plainly', { skip }, async () => {
  await ctl({ reset: true, paired: true, leaders: FX.leaders, l2Reset: true });
  const { ctx, page, problems } = await phone();
  await openApprovals(page);
  await openItemOf(page, 'plans');
  // Changed: the detail moved on since it was opened.
  await ctl({ l2Digest: { plans: 'f'.repeat(64) } });
  const details = async () => (await l2calls()).filter(e => e.method === 'GET' && e.path.startsWith('/r/plans/p-')).length;
  const d0 = await details();
  await viaSheet(page, '[data-kind="plan_approve"]');
  await page.waitForSelector('#item-status:not([hidden])');
  assert.equal(await page.textContent('#item-status'), 'This changed since you opened it: read it again.');
  assert.equal(await page.isVisible('#sheet'), false);
  await page.waitForSelector('#item-acts button');
  assert.equal(await details(), d0 + 1, 'the detail is read again');
  assert.equal((await actCalls()).length, 0);
  assert.equal((await creds(page)).length, 0, 'no Face ID for a changed item');
  await shot(page, '52-changed');
  // Now with the new digest it goes through the challenge.
  const RE = [
    ['challenge', { status: 403, body: { ok: false, reason: 'level2_off' } }, 'Approving is turned off on your Mac. Nothing was done.'],
    ['challenge', { status: 403, body: { ok: false, reason: 'no_passkey' } }, /^This device has no Face ID key/],
    ['challenge', { status: 429, body: { ok: false } }, /^Too many failed tries/],
    ['plan_approve', { status: 403, body: { ok: false, reason: 'passkey' } }, 'Face ID could not be checked by the Mac. Nothing was done.'],
    ['plan_approve', { status: 409, body: { ok: false, reason: 'refused', error: 'the plan has a thread named <b>x</b> already' } }, 'The Mac refused it: the plan has a thread named <b>x</b> already'],
  ];
  for (const [route, reply, want] of RE) {
    await ctl({ l2Replies: { [route]: [reply] } });
    await viaSheet(page, '[data-kind="plan_approve"]');
    await page.waitForFunction(() => /Nothing was done|refused/.test(document.getElementById('sheet-status').textContent));
    const t = await page.textContent('#sheet-status');
    if (typeof want === 'string') assert.equal(t, want); else assert.match(t, want);
    assert.equal(await page.$$eval('#sheet b', b => b.length), 0, 'the daemon\'s words are text');
    await page.tap('#sheet-cancel');
  }
  assert.equal((await actCalls()).filter(e => !e.path.endsWith('/approve')).length, 0);
  await cleanSpy(page);
  assert.deepEqual(problems, []);
  await ctx.close();
  // Face ID cancelled, or an answer of the wrong shape: no action route.
  for (const [mode, want] of [['cancel', 'Face ID was cancelled. Nothing was done.'], ['wrong', 'Face ID gave an answer this page cannot read. Nothing was done.']]) {
    await ctl({ reset: true, l2Reset: true });
    const { ctx, page } = await phone(390, 844, { flags: [`__cred:${mode}`] });
    await openApprovals(page);
    await openItemOf(page, 'stopped');
    await viaSheet(page, '[data-kind="sandbox_remove"]');
    await page.waitForSelector(`#sheet-status:text("${want}")`);
    assert.equal((await actCalls()).length, 0, `${mode}: nothing done`);
    assert.equal((await calls('POST', '/r/approvals/challenge')).length, 1);
    await ctx.close();
  }
});

test('level 2: hostile text in a plan and in file paths stays text; the sheet closes when the page hides', { skip }, async () => {
  const evil = '<img src=x onerror="window.__pwned=1"> [tap](javascript:window.__pwned=2) **bold** <a href="https://evil.example">x</a>';
  const L = structuredClone(L2);
  await ctl({ reset: true, paired: true, leaders: FX.leaders, l2Reset: true });
  // The stand-in reads the fixtures it was started with; hostile text goes in through its state.
  const { ctx, page, problems } = await phone(320, 568);
  await page.route('**/r/plans/p-*', async route => {
    const plan = structuredClone(L.plan);
    plan.plan.summary = evil;
    plan.plan.threads[0].goal = evil.repeat(5);
    plan.plan.threads[0].name = evil;
    plan.plan.justification = evil;
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ ...plan, digest: 'a'.repeat(64) }) });
  });
  await page.route('**/r/merges/p-*', async route => {
    const m = structuredClone(L.merge);
    m.merge.branches[0].files[0].path = '<script>window.__pwned=3</script>/../../etc/passwd';
    m.merge.note = evil;
    m.merge.distillate[0].text = evil;
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(m) });
  });
  await openApprovals(page);
  await openItemOf(page, 'plans');
  await page.tap('.rc-fold >> nth=0').catch(() => {});
  const check = () => page.evaluate(() => ({
    pwned: window.__pwned,
    bad: document.querySelectorAll('#item img, #item script, #item a, #item strong, #item iframe, #item [onerror], #item [href]').length,
  }));
  assert.deepEqual(await check(), { pwned: undefined, bad: 0 });
  assert.ok((await page.textContent('.rc-item-title')).includes('<img src=x'), 'shown as text');
  assert.ok(await noHScroll(page));
  await shot(page, '53-plan-hostile-320');
  await page.tap('#back');
  await openItemOf(page, 'merges');
  await page.tap('.rc-files-box summary >> nth=0');
  assert.deepEqual(await check(), { pwned: undefined, bad: 0 });
  assert.equal(await page.textContent('.rc-file-path'), '<script>window.__pwned=3</script>/../../etc/passwd');
  assert.ok(await noHScroll(page));
  // The sheet, then the page hides: everything of it is dropped.
  await page.tap('[data-kind="merge"]');
  await page.waitForSelector('#sheet:not([hidden])');
  await page.evaluate(() => {
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'hidden' });
    document.dispatchEvent(new Event('visibilitychange'));
  });
  await page.waitForSelector('#sheet', { state: 'hidden' });
  assert.equal((await calls('POST', '/r/approvals/challenge')).length, 0);
  await cleanSpy(page);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('level 2 screens at 320x568', { skip }, async () => {
  await ctl({ reset: true, paired: true, leaders: FX.leaders, l2Reset: true, pkStates: ['pending'] });
  const { ctx, page, problems } = await phone(320, 568);
  await page.goto(`${base()}/`);
  await page.waitForSelector('#badge:not([hidden])');
  await shot(page, '60-approvals-entry-320');
  await page.tap('#approvals-btn');
  await page.waitForSelector('.rc-ap-row');
  assert.ok(await noHScroll(page));
  await shot(page, '61-approvals-list-320');
  for (const [cls, name, btn] of [['plans', 'plan', '[data-kind="plan_approve"]'], ['merges', 'merge', '[data-kind="merge"]'], ['network', 'network', '#item-acts button'], ['stopped', 'stopped', '[data-kind="sandbox_remove"]']]) {
    await openItemOf(page, cls);
    assert.ok(await noHScroll(page), name);
    await shot(page, `62-${name}-320`);
    await page.tap(btn);
    await page.waitForSelector('#sheet:not([hidden])');
    await shot(page, `63-${name}-sheet-320`);
    await page.tap('#sheet-cancel');
    await page.tap('#back');
    await page.waitForSelector('.rc-ap-row');
  }
  await ctl({ l2: { approvals: { passkey: false } } });
  await page.tap('#back');
  await page.tap('#approvals-btn');
  await page.waitForSelector('#ap-enable:not([hidden])');
  await page.tap('#ap-enable');
  await page.waitForSelector('#pk-check:not([hidden])');
  await shot(page, '64-faceid-digits-320');
  await ctl({ l2Reset: true });
  await page.tap('#back');
  await page.waitForSelector('.rc-ap-row');
  await openItemOf(page, 'plans');
  await ctl({ l2Digest: { plans: 'f'.repeat(64) } });
  await viaSheet(page, '[data-kind="plan_approve"]');
  await page.waitForSelector('#item-status:not([hidden])');
  await shot(page, '65-changed-320');
  assert.deepEqual(problems, []);
  await ctx.close();
});

// ══ The Kanban (read only) ═══════════════════════════════════════════════
const KB = FX.kanban;
const kbGets = async (path) => (await log()).filter(e => e.method === 'GET' && e.path === path).length;
async function openBoard(page, sandbox = 'shop') {
  await page.goto(`${base()}/`);
  await page.tap(`.rc-leader >> text=${sandbox}`);
  await page.waitForSelector('#s-chat:not([hidden])');
  await page.tap('#v-kanban');
  await page.waitForSelector('.rc-kb-lane, #kb-status:not([hidden])');
}
const selected = page => page.$eval('.rc-kb-tab[aria-selected="true"]', t => t.dataset.col);
const visibleCards = page => page.$$eval('.rc-kb-lane.rc-on .rc-kb-card', c => c.map(e => e.dataset.key));
async function swipe(page, dx) {
  await page.evaluate(dx => {
    const el = document.getElementById('kb-lanes');
    const t = (x, y) => new Touch({ identifier: 1, target: el, clientX: x, clientY: y });
    el.dispatchEvent(new TouchEvent('touchstart', { touches: [t(200, 300)], changedTouches: [t(200, 300)], bubbles: true }));
    el.dispatchEvent(new TouchEvent('touchend', { touches: [], changedTouches: [t(200 + dx, 310)], bubbles: true }));
  }, dx);
}

test('kanban: the switch keeps each place; one column at a time, by picker or swipe; nothing writes', { skip }, async () => {
  await ctl({ reset: true, paired: true, leaders: FX.leaders, history: { shop: FX.history.shop }, kanbanReset: true, l2Reset: true });
  const { ctx, page, problems } = await phone();
  await page.goto(`${base()}/`);
  await page.tap('.rc-leader >> text=shop');
  await page.waitForSelector('.rc-agent');
  await page.evaluate(() => { document.getElementById('log').scrollTop = 40; });
  const logTop = await page.$eval('#log', l => l.scrollTop);
  await shot(page, '70-switch-conversation');
  await page.tap('#v-kanban');
  await page.waitForSelector('.rc-kb-lane.rc-on .rc-kb-card');
  assert.equal(await page.getAttribute('#v-kanban', 'aria-selected'), 'true');
  assert.equal(await page.isVisible('#composer'), false, 'the board has no composer');
  assert.equal(await selected(page), 'progress', 'active things first');
  assert.deepEqual(await visibleCards(page), ['t:12', 'th:checkout'], 'the daemon\'s order');
  assert.deepEqual(await page.$$eval('.rc-kb-tab', t => t.map(e => e.textContent)), ['Planned2', 'In progress2', 'In test0', 'Ready to merge1', 'Merged14']);
  assert.deepEqual(await page.$$eval('.rc-kb-lane.rc-on .rc-kb-mark', m => m.map(e => [e.textContent, e.className])), [['blocked', 'rc-data rc-kb-mark rc-tone-amber']]);
  await shot(page, '71-kanban-in-progress');
  await page.tap('.rc-kb-tab[data-col="test"]');
  assert.equal(await page.textContent('.rc-kb-lane.rc-on .rc-kb-empty'), 'Nothing in test.');
  await shot(page, '72-kanban-empty-column');
  await page.tap('.rc-kb-tab[data-col="merged"]');
  assert.equal(await page.textContent('.rc-kb-lane.rc-on .rc-kb-older'), 'and 2 older');
  await shot(page, '73-kanban-merged');
  await page.tap('.rc-kb-tab[data-col="planned"]');
  // Long titles end in an ellipsis: two lines at most, nothing overflows.
  const clamp = await page.$eval('.rc-kb-lane.rc-on .rc-kb-title', e => [getComputedStyle(e).webkitLineClamp, e.clientHeight <= 2 * parseFloat(getComputedStyle(e).lineHeight) + 1]);
  assert.deepEqual(clamp, ['2', true]);
  assert.ok(await noHScroll(page));
  await shot(page, '74-kanban-planned');
  await swipe(page, -120);
  assert.equal(await selected(page), 'progress', 'a swipe left: the next column');
  await swipe(page, 120);
  assert.equal(await selected(page), 'planned');
  await swipe(page, 120);
  assert.equal(await selected(page), 'planned', 'no column before the first');
  await swipe(page, -30);
  assert.equal(await selected(page), 'planned', 'a small move is not a swipe');
  // Read only: no control that writes, not even a disabled one.
  const labels = await page.$$eval('#board button', b => b.map(e => `${e.textContent} ${e.getAttribute('aria-label') || ''}`.toLowerCase()));
  for (const l of labels) assert.doesNotMatch(l, /\b(add|new|move|edit|delete|remove|send|drag)\b/, l);
  assert.equal(await page.$$eval('#board input, #board textarea, #board [draggable="true"], #board [contenteditable]', e => e.length), 0);
  // Back to the conversation: its place; back to the board: its column.
  await page.tap('#v-chat');
  assert.equal(await page.$eval('#log', l => l.scrollTop), logTop);
  assert.equal(await page.isVisible('#composer'), true);
  await page.tap('#v-kanban');
  assert.equal(await selected(page), 'planned');
  // Another leader has its own view; coming back to shop finds the board.
  await page.tap('#back');
  await page.tap('.rc-leader >> text=website');
  await page.waitForSelector('#v-chat[aria-selected="true"]');
  await page.tap('#back');
  await page.tap('.rc-leader >> text=shop');
  await page.waitForSelector('#v-kanban[aria-selected="true"]');
  await page.waitForSelector('.rc-kb-lane.rc-on .rc-kb-card');
  assert.equal(await selected(page), 'planned');
  assert.equal((await log()).filter(e => e.method !== 'GET' && /kanban/.test(e.path)).length, 0, 'the board only reads');
  await cleanSpy(page);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('kanban: a ticket as text, files as rows; a thread\'s own card calls no route', { skip }, async () => {
  await ctl({ reset: true, paired: true, leaders: FX.leaders, kanbanReset: true });
  const { ctx, page, problems } = await phone();
  await openBoard(page);
  await page.tap('.rc-kb-card[data-key="t:12"]');
  await page.waitForSelector('.rc-kb-desc');
  assert.equal(await page.textContent('.rc-kb-ttitle'), 'Orders table and its migration');
  assert.equal(await page.$eval('.rc-kb-desc', e => getComputedStyle(e).whiteSpace), 'pre-wrap', 'line breaks kept');
  assert.equal(await page.textContent('.rc-kb-desc'), KB.ticket.ticket.description);
  assert.deepEqual(await page.$$eval('.rc-kb-files li', l => l.map(e => e.textContent)), ['schema-sketch.png40 KB', 'notes.txt812 B']);
  assert.ok(await page.isVisible('text=Files open on your Mac.'));
  assert.equal(await page.$$eval('#kb-ticket a, #kb-ticket img, #kb-ticket [href], #kb-ticket [download]', e => e.length), 0, 'no preview, no download, no link');
  assert.deepEqual(await page.$$eval('.rc-kb-hist li', l => l.map(e => e.textContent)), ['2hCreated by you', '1hPlanned in a plan of the lead', '40mThread db-schema started']);
  assert.equal(await page.textContent('.rc-kb-tcol'), 'In progress');
  await shot(page, '75-kanban-ticket');
  await page.tap('.rc-kb-back');
  await page.waitForSelector('.rc-kb-lane.rc-on');
  assert.equal(await selected(page), 'progress');
  const before = (await log()).length;
  await page.tap('.rc-kb-card[data-key="th:checkout"]');
  await page.waitForSelector('text=A thread of the plan, with no ticket of its own.');
  await page.waitForTimeout(300);
  assert.equal((await log()).slice(before).filter(e => /kanban\//.test(e.path)).length, 0, 'no ticket route for a thread\'s card');
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('kanban: the event re-reads the board, keeping the column, the scroll and an open ticket', { skip }, async () => {
  await ctl({ reset: true, paired: true, leaders: FX.leaders, kanbanReset: true });
  const { ctx, page, problems } = await phone(320, 568);
  await openBoard(page);
  await page.tap('.rc-kb-tab[data-col="merged"]');
  await page.$eval('.rc-kb-lane.rc-on .rc-kb-list', l => { l.scrollTop = 120; });
  const top = await page.$eval('.rc-kb-lane.rc-on .rc-kb-list', l => l.scrollTop);
  assert.ok(top > 50, `the merged list scrolls: ${top}`);
  const reads = await kbGets('/r/leaders/shop/kanban');
  // Same hash: nothing to read.
  await ctl({ emit: { event: 'kanban', data: { sandbox: 'shop', latest: KB.board.latest } } });
  await page.waitForTimeout(700);
  assert.equal(await kbGets('/r/leaders/shop/kanban'), reads, 'an unchanged board is not read again');
  // A new card in Merged, a burst of three events: one read.
  const next = structuredClone(KB.board);
  next.latest = '1111111111111111';
  next.columns[4].cards.unshift({ ...next.columns[4].cards[0], key: 't:20', n: 20, num: 'T-20', title: 'Brand new merge' });
  await ctl({ kanban: { shop: next } });
  for (let i = 0; i < 3; i++) await ctl({ emit: { event: 'kanban', data: { sandbox: 'shop', latest: next.latest } } });
  await page.waitForSelector('.rc-kb-card[data-key="t:20"]');
  await page.waitForTimeout(600);
  assert.equal(await kbGets('/r/leaders/shop/kanban'), reads + 1, `debounced: ${JSON.stringify((await log()).filter(e => /kanban/.test(e.path)))}`);
  assert.equal(await selected(page), 'merged', 'the column is kept');
  assert.equal(await page.$eval('.rc-kb-lane.rc-on .rc-kb-list', l => l.scrollTop), top, 'the scroll is kept');
  // An open ticket stays open and is read again.
  await page.tap('.rc-kb-tab[data-col="progress"]');
  await page.tap('.rc-kb-card[data-key="t:12"]');
  await page.waitForSelector('.rc-kb-desc');
  const t2 = structuredClone(KB.ticket);
  t2.ticket.history.push({ when: 'now', text: 'Thread db-schema reported done', tone: 'green' });
  const n2 = structuredClone(next);
  n2.latest = '2222222222222222';
  await ctl({ kanban: { shop: n2 }, tickets: { 'shop/12': t2 } });
  await ctl({ emit: { event: 'kanban', data: { sandbox: 'shop', latest: n2.latest } } });
  await page.waitForSelector('text=Thread db-schema reported done');
  assert.equal(await page.isVisible('#kb-ticket'), true, 'the ticket stays open');
  // Another sandbox's event: nothing read.
  const r2 = await kbGets('/r/leaders/shop/kanban');
  await ctl({ emit: { event: 'kanban', data: { sandbox: 'website', latest: 'ffffffffffffffff' } } });
  await page.waitForTimeout(600);
  assert.equal(await kbGets('/r/leaders/shop/kanban'), r2);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('kanban: an empty board, a stopped leader, hostile titles and descriptions', { skip }, async () => {
  await ctl({ reset: true, paired: true, leaders: FX.leaders, kanbanReset: true });
  {
    const { ctx, page } = await phone();
    await openBoard(page, 'website');
    await page.waitForSelector('.rc-kb-lane.rc-on');
    assert.equal(await selected(page), 'planned');
    assert.equal(await page.textContent('.rc-kb-lane.rc-on .rc-kb-empty'), 'Nothing planned.');
    await shot(page, '76-kanban-empty-board');
    await ctx.close();
  }
  await ctl({ kanban: { shop: null } });
  {
    const { ctx, page } = await phone();
    await openBoard(page);
    await page.waitForSelector('#kb-status:text("This leader is no longer running.")');
    await shot(page, '77-kanban-stopped');
    await ctx.close();
  }
  const evil = '<img src=x onerror="window.__pwned=1"> [x](javascript:window.__pwned=2) **b** ' + 'W'.repeat(300);
  const b = structuredClone(KB.board);
  const c = b.columns[1].cards[0];
  Object.assign(c, { title: evil, title_long: evil, thread: '<svg onload=window.__pwned=3>', marks: [{ id: 'x', label: '<b>mark</b>', tone: '"><script>' }] });
  const t = structuredClone(KB.ticket);
  Object.assign(t.ticket, { title: evil, description: `${evil}\n<script>window.__pwned=4</script>`, card: c });
  t.ticket.files = [{ name: '<img src=x onerror=window.__pwned=5>.png', size: 3, kind: 'png' }];
  t.ticket.history = [{ when: '<i>', text: evil, tone: 'red' }];
  await ctl({ kanban: { shop: b }, tickets: { 'shop/12': t } });
  const { ctx, page, problems } = await phone(320, 568);
  await openBoard(page);
  await page.waitForSelector('.rc-kb-lane.rc-on .rc-kb-card');
  assert.ok(await noHScroll(page));
  await page.tap('.rc-kb-card[data-key="t:12"]');
  await page.waitForSelector('.rc-kb-desc');
  const r = await page.evaluate(() => ({
    pwned: window.__pwned,
    bad: document.querySelectorAll('#board img, #board script, #board svg:not([aria-hidden]), #board a, #board b, #board i, #board strong, #board [onerror], #board [onload]').length,
  }));
  assert.deepEqual(r, { pwned: undefined, bad: 0 });
  assert.ok((await page.textContent('.rc-kb-desc')).includes('<script>window.__pwned=4</script>'), 'shown as text');
  assert.ok(await noHScroll(page));
  await cleanSpy(page);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('kanban shots: 390x844, 320x568, and landscape 844x390 with the columns side by side', { skip }, async () => {
  await ctl({ reset: true, paired: true, leaders: FX.leaders, kanbanReset: true });
  for (const [w, h] of [[320, 568], [844, 390]]) {
    const { ctx, page, problems } = await phone(w, h);
    await openBoard(page);
    await page.waitForSelector('.rc-kb-lane.rc-on .rc-kb-card');
    assert.ok(await noHScroll(page), `${w}`);
    if (w > 700) {
      assert.equal(await page.$$eval('.rc-kb-lane', l => l.filter(e => getComputedStyle(e).display !== 'none').length), 5, 'all five side by side');
      assert.equal(await page.isVisible('#kb-pick'), false);
    }
    await shot(page, `78-kanban-${w}x${h}`);
    await page.tap('.rc-kb-card[data-key="t:12"]');
    await page.waitForSelector('.rc-kb-desc');
    await shot(page, `79-kanban-ticket-${w}x${h}`);
    assert.deepEqual(problems, []);
    await ctx.close();
  }
});

// ══ Dictation by the phone's own recognition (optional) ═══════════════════
const FALLBACK = 'iPhone dictation is not available here. Using your Mac.';
async function chooseIphone(page) {
  await page.tap('#menu-btn');
  await page.tap('#dict-iphone');
  await page.tap('#menu-btn');
}
test('dictation setting: Mac by default, iPhone kept as a preference, its honest line and language', { skip }, async () => {
  await ctl({ reset: true, paired: true, leaders: FX.leaders, history: { website: FX.history.website } });
  const { ctx, page, problems } = await phone();
  await page.goto(`${base()}/`);
  await page.waitForSelector('.rc-leader');
  await page.tap('#menu-btn');
  assert.equal(await page.getAttribute('#dict-mac', 'aria-checked'), 'true', 'Mac by default');
  assert.ok(await page.isVisible('text=iPhone dictation is done by Apple\'s speech recognition; your voice may be sent to Apple.'));
  assert.equal(await page.isVisible('#dict-lang'), false);
  await page.tap('#dict-iphone');
  assert.equal(await page.getAttribute('#dict-iphone', 'aria-checked'), 'true');
  assert.match(await page.textContent('#dict-lang'), /^Language: [a-z]{2}(-[A-Z]{2})? \(this browser's\)\.$/);
  await shot(page, '80-menu-dictation');
  assert.deepEqual(JSON.parse(await page.evaluate(() => localStorage.getItem('sbxw-remote-prefs'))), { dictation: 'iphone' });
  await page.reload();
  await page.waitForSelector('.rc-leader');
  await page.tap('#menu-btn');
  assert.equal(await page.getAttribute('#dict-iphone', 'aria-checked'), 'true', 'kept across a reload');
  await page.tap('#dict-mac');
  assert.deepEqual(JSON.parse(await page.evaluate(() => localStorage.getItem('sbxw-remote-prefs'))), {}, 'Mac is the default: nothing kept');
  await cleanSpy(page);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('iPhone dictation: interim then final words into the field at the caret, stop, nothing sent, no audio stream', { skip }, async () => {
  await ctl({ reset: true, paired: true, leaders: FX.leaders, history: { website: FX.history.website } });
  const { ctx, page, problems } = await phone();
  await page.goto(`${base()}/`);
  await chooseIphone(page);
  await page.tap('.rc-leader >> text=website');
  await page.fill('#field', 'Hello world');
  await page.evaluate(() => { const f = document.getElementById('field'); f.focus(); f.setSelectionRange(5, 5); });
  await page.tap('#mic');
  await page.waitForSelector('#composer[data-rec="listening"]');
  const spy = await page.evaluate(() => window.__spy);
  assert.match(spy.sr[0], /^start [a-z]{2}(-[A-Z]{2})? true true$/);
  assert.equal(spy.tracks, 0, 'the page opens no audio stream of its own');
  assert.equal(await page.isVisible('#wave'), false, 'no wave in this mode');
  assert.match(await page.textContent('#status'), /^Listening \(iPhone dictation\)\./);
  await page.evaluate(() => window.__srLast.say([['big', false]]));
  assert.equal(await page.textContent('#rec-interim'), 'big');
  assert.equal(await page.inputValue('#field'), 'Hello world', 'an interim word is not in the field');
  await page.evaluate(() => window.__srLast.say([['big and', true], ['bright', false]]));
  assert.equal(await page.textContent('#rec-final'), 'big and');
  assert.equal(await page.textContent('#rec-interim'), 'bright');
  await page.waitForTimeout(1100);
  assert.match(await page.textContent('#rec-time'), /^0:0[1-2]$/);
  await shot(page, '81-iphone-listening');
  await page.evaluate(() => window.__srLast.say([['big and', true], ['bright', true]]));
  await page.tap('#mic');
  await page.waitForSelector('#composer[data-rec="idle"]');
  assert.equal(await page.inputValue('#field'), 'Hello big and bright world', 'at the caret, spaced');
  assert.equal(await page.textContent('#status'), 'Added to the field: read it, then Send.');
  await shot(page, '82-iphone-filled');
  assert.ok((await page.evaluate(() => window.__spy.sr)).includes('stop'));
  assert.equal((await calls('POST', '/r/voice/transcribe')).length, 0, 'the Mac is not asked');
  assert.equal((await calls('POST', '/r/leaders/website/message')).length, 0, 'dictation never sends');
  // The advisory checks run on dictated words.
  await page.fill('#field', '');
  await page.tap('#mic');
  await page.waitForSelector('#composer[data-rec="listening"]');
  await page.evaluate(() => window.__srLast.say([['/clear', true]]));
  await page.tap('#mic');
  await page.waitForSelector('#status:text-matches("^Commands cannot be sent from the phone")');
  // Nothing heard.
  await page.fill('#field', '');
  await page.tap('#mic');
  await page.waitForSelector('#composer[data-rec="listening"]');
  await page.evaluate(() => window.__srLast.fail('no-speech'));
  await page.waitForSelector('#status:text("Nothing was heard. Tap the microphone and speak.")');
  assert.equal((await calls('POST', '/r/voice/transcribe')).length, 0);
  await cleanSpy(page);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('iPhone dictation falls back to the Mac, with one sentence, never a dead button', { skip }, async () => {
  for (const mode of ['missing', 'ctor', 'start', 'err:not-allowed', 'err:service-not-allowed', 'err:network', 'err:audio-capture']) {
    await ctl({ reset: true, paired: true, leaders: FX.leaders, history: { website: FX.history.website } });
    const { ctx, page, problems } = await phone(390, 844, { flags: [`__sr:${mode}`] });
    await page.goto(`${base()}/`);
    await chooseIphone(page);
    await page.tap('.rc-leader >> text=website');
    await page.tap('#mic');
    await page.waitForSelector('#composer[data-rec="recording"]', { timeout: 5000 }).catch(async e => {
      throw new Error(`${mode}: ${await page.getAttribute('#composer', 'data-rec')} / ${await page.textContent('#status')} / ${JSON.stringify(await page.evaluate(() => window.__spy.sr))}`);
    });
    assert.equal(await page.textContent('#status'), FALLBACK, mode);
    if (mode === 'missing') await shot(page, '83-iphone-fallback');
    await page.waitForTimeout(600);
    await page.tap('#mic');
    await page.waitForSelector('text=Added to the field: read it, then Send.', { timeout: 8000 });
    assert.equal((await calls('POST', '/r/voice/transcribe')).length, 1, `${mode}: the Mac transcribed`);
    assert.equal((await calls('POST', '/r/leaders/website/message')).length, 0);
    assert.deepEqual(problems, [], mode);
    await ctx.close();
  }
  // An error after words were heard is not a fallback: the words stay.
  await ctl({ reset: true, paired: true, leaders: FX.leaders, history: { website: FX.history.website } });
  const { ctx, page } = await phone();
  await page.goto(`${base()}/`);
  await chooseIphone(page);
  await page.tap('.rc-leader >> text=website');
  await page.tap('#mic');
  await page.waitForSelector('#composer[data-rec="listening"]');
  await page.evaluate(() => { window.__srLast.say([['kept words', true]]); window.__srLast.fail('network'); });
  await page.waitForSelector('#composer[data-rec="idle"]');
  assert.equal(await page.inputValue('#field'), 'kept words');
  assert.equal((await calls('POST', '/r/voice/transcribe')).length, 0);
  await ctx.close();
});

// ══ Dictation never stops or stays deaf in silence (phone-dictation) ═══════
const D = {
  added: 'Added to the field: read it, then Send.',
  nothing: 'Nothing was heard. Tap the microphone and speak.',
  stoppedSelf: 'The iPhone stopped listening: tap the microphone to go on.',
  taken: 'The microphone was taken by another app (a call?). Tap the microphone to try again.',
  network: 'The iPhone\'s speech recognition lost its connection. Tap the microphone to try again, or choose Mac in the menu.',
  noStart: 'The iPhone did not start listening. Tap the microphone to try again.',
  hidden: 'Dictation stopped: the page was hidden.',
  kept: 'The words heard are in the field.',
  quietMac: 'The microphone hears nothing. Speak closer, or check that no other app (a call) is using it.',
  quietIphone: 'Nothing heard for a few seconds. Speak, or tap stop, then the microphone again.',
  macShort: 'Too short to transcribe. Tap the microphone, speak, then tap stop.',
  macSilent: 'Nothing was heard: the recording is silent. Check the microphone and try again.',
  macNoWords: 'The Mac heard no words. Speak closer to the phone and try again.',
  macFailed: 'The Mac could not transcribe this: try again.',
};
async function iphoneListening(page) {
  await page.goto(`${base()}/`);
  await chooseIphone(page);
  await page.tap('.rc-leader >> text=website');
  await page.tap('#mic');
  await page.waitForSelector('#composer[data-rec="listening"]');
}
async function idleWith(page, text, what) {
  await page.waitForSelector('#composer[data-rec="idle"]', { timeout: 5000 });
  assert.equal(await page.textContent('#status'), text, what);
  assert.equal(await page.getAttribute('#mic', 'aria-pressed'), 'false', `${what}: not "listening"`);
  assert.equal(await page.isVisible('#rec'), false, `${what}: the bar is gone`);
}
const LONG = 'First sentence about the orders table that should scroll away. Second sentence about the index on the customer id. Third sentence asks for a migration file. Fourth and last sentence says ship it today';

test('iPhone dictation: the preview shows the END of a long text, newest words in view', { skip }, async () => {
  await ctl({ reset: true, paired: true, leaders: FX.leaders, history: { website: FX.history.website } });
  const { ctx, page, problems } = await phone();
  await iphoneListening(page);
  await page.evaluate(t => { window.__srLast.sound(); window.__srLast.say([[t, true], ['and the newest interim words', false]]); }, LONG);
  const r = await page.evaluate(() => {
    const box = document.getElementById('rec-words').getBoundingClientRect();
    const i = document.getElementById('rec-interim');
    const rr = i.getClientRects();
    const last = rr[rr.length - 1];
    const w = document.getElementById('rec-words');
    return { fits: w.scrollHeight <= w.clientHeight + 1, lastIn: last.bottom <= box.bottom + 1 && last.top >= box.top - 1, final: document.getElementById('rec-final').textContent };
  });
  assert.ok(r.fits, 'two lines hold the preview');
  assert.ok(r.lastIn, 'the newest words are in view');
  assert.ok(r.final.startsWith('…'), `cut at the start: ${r.final}`);
  assert.ok(r.final.endsWith('ship it today'), 'the last final words are shown');
  assert.ok(!r.final.includes('First sentence'), 'the first words are not');
  assert.equal(await page.inputValue('#field'), LONG, 'the field holds all of it');
  assert.equal(await page.getAttribute('#rec', 'data-hear'), 'sound', 'the dot says it hears');
  await shot(page, '84-iphone-long-text');
  await page.tap('#mic');
  await idleWith(page, D.added, 'stop');
  assert.equal(await page.inputValue('#field'), LONG);
  await cleanSpy(page);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('iPhone dictation: every end or error says a sentence and returns to idle', { skip }, async () => {
  const cases = [
    ['no-speech', p => p.evaluate(() => window.__srLast.fail('no-speech')), D.nothing, ''],
    ['ended alone, nothing heard', p => p.evaluate(() => window.__srLast.endSelf()), D.nothing, ''],
    ['ended alone after words (short round: no restart)', p => p.evaluate(() => { window.__srLast.say([['kept words', true]]); window.__srLast.endSelf(); }), `${D.stoppedSelf} ${D.kept}`, 'kept words'],
    ['audio-capture after words', p => p.evaluate(() => { window.__srLast.say([['kept words', true]]); window.__srLast.fail('audio-capture'); }), `${D.taken} ${D.kept}`, 'kept words'],
    ['network after words', p => p.evaluate(() => { window.__srLast.say([['kept words', true]]); window.__srLast.fail('network'); }), `${D.network} ${D.kept}`, 'kept words'],
    ['aborted', p => p.evaluate(() => window.__srLast.fail('aborted')), D.nothing, ''],
    ['page hidden', p => p.evaluate(() => {
      window.__srLast.say([['kept words', true]]);
      Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'hidden' });
      document.dispatchEvent(new Event('visibilitychange'));
    }), `${D.hidden} ${D.kept}`, 'kept words'],
  ];
  for (const [what, act, text, field] of cases) {
    await ctl({ reset: true, paired: true, leaders: FX.leaders, history: { website: FX.history.website } });
    const { ctx, page, problems } = await phone();
    await iphoneListening(page);
    await act(page);
    await idleWith(page, text, what);
    assert.equal(await page.inputValue('#field'), field, what);
    if (what === 'audio-capture after words') await shot(page, '85-iphone-error-sentence');
    assert.equal((await calls('POST', '/r/voice/transcribe')).length, 0, what);
    assert.deepEqual(problems, [], what);
    await ctx.close();
  }
  // A recogniser that never says it started, one that never ends after stop.
  for (const [flag, text] of [['dead', D.noStart], ['deaf', D.nothing]]) {
    await ctl({ reset: true, paired: true, leaders: FX.leaders, history: { website: FX.history.website } });
    const { ctx, page, problems } = await phone(390, 844, { flags: [`__sr:${flag}`] });
    await iphoneListening(page);
    if (flag === 'dead') {
      await page.waitForSelector(`#status:text("${D.quietIphone}")`, { timeout: 5000 });
      await shot(page, '86-iphone-nothing-heard-yet');
    } else await page.tap('#mic');
    await page.waitForSelector('#composer[data-rec="idle"]', { timeout: 6000 });
    await idleWith(page, text, flag);
    assert.deepEqual(problems, [], flag);
    await ctx.close();
  }
});

test('iPhone dictation: a recogniser gone deaf after words is said, not shown as listening in silence', { skip }, async () => {
  await ctl({ reset: true, paired: true, leaders: FX.leaders, history: { website: FX.history.website } });
  const { ctx, page, problems } = await phone();
  await iphoneListening(page);
  await page.evaluate(() => window.__srLast.say([['some words', true]]));
  assert.match(await page.textContent('#status'), /^Listening/);
  // No event at all for 8 s (WebKit bug 321436): the person is told.
  await page.waitForSelector(`#status:text("${D.quietIphone}")`, { timeout: 10000 });
  assert.equal(await page.getAttribute('#rec', 'data-quiet'), 'true');
  await page.evaluate(() => window.__srLast.say([['some words', true], ['more', false]]));
  await page.waitForSelector('#status:text-matches("^Listening")');
  await page.tap('#mic');
  await idleWith(page, D.added, 'stop');
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('iPhone dictation: an end by itself restarts at most twice, the words once each', { skip }, async () => {
  await ctl({ reset: true, paired: true, leaders: FX.leaders, history: { website: FX.history.website } });
  const { ctx, page, problems } = await phone();
  await iphoneListening(page);
  // Round 1: words, then iOS ends it after a pause.
  await page.evaluate(() => window.__srLast.say([['add an index', true]]));
  await page.waitForTimeout(2100);
  await page.evaluate(() => window.__srLast.endSelf());
  await page.waitForTimeout(100);
  assert.equal(await page.getAttribute('#composer', 'data-rec'), 'listening', 'restarted');
  assert.equal(await page.evaluate(() => window.__spy.srMade), 2);
  // Round 2 re-delivers the last words of round 1, then new ones.
  await page.evaluate(() => window.__srLast.say([['Add an index', true], ['on the id', true]]));
  assert.equal(await page.inputValue('#field'), 'add an index on the id', 'nothing twice');
  await page.waitForTimeout(2100);
  await page.evaluate(() => window.__srLast.endSelf());
  await page.waitForTimeout(100);
  assert.equal(await page.evaluate(() => window.__spy.srMade), 3, 'a second restart');
  await page.evaluate(() => window.__srLast.say([['please', true]]));
  await page.waitForTimeout(2100);
  // A third end: no more restarts. It says so, and keeps every word once.
  await page.evaluate(() => window.__srLast.endSelf());
  await idleWith(page, `${D.stoppedSelf} ${D.kept}`, 'third end');
  assert.equal(await page.evaluate(() => window.__spy.srMade), 3, 'no restart loop');
  assert.equal(await page.inputValue('#field'), 'add an index on the id please');
  assert.equal((await calls('POST', '/r/leaders/website/message')).length, 0, 'dictation never sends');
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('the Mac path: a tiny or silent clip is not sent; the Mac\'s failure or empty answer is a sentence; the mic taken away', { skip }, async () => {
  const open = async (flags = []) => {
    const o = await phone(390, 844, { flags });
    await o.page.goto(`${base()}/`);
    await o.page.tap('.rc-leader >> text=website');
    return o;
  };
  // Too short.
  await ctl({ reset: true, paired: true, leaders: FX.leaders, history: { website: FX.history.website } });
  {
    const { ctx, page, problems } = await open();
    await page.tap('#mic');
    await page.waitForSelector('#composer[data-rec="recording"]');
    await page.tap('#mic');
    await idleWith(page, D.macShort, 'short');
    assert.equal((await calls('POST', '/r/voice/transcribe')).length, 0, 'a tiny clip is not sent');
    assert.deepEqual(problems, []);
    await ctx.close();
  }
  // Silent: the meter says so while it records, and nothing is sent.
  {
    const { ctx, page, problems } = await open(['__silent']);
    await page.tap('#mic');
    await page.waitForSelector('#composer[data-rec="recording"]');
    await page.waitForSelector(`#status:text("${D.quietMac}")`, { timeout: 6000 });
    assert.equal(await page.getAttribute('#rec', 'data-quiet'), 'true');
    await shot(page, '87-mac-nothing-heard');
    await page.tap('#mic');
    await idleWith(page, D.macSilent, 'silent');
    assert.equal((await calls('POST', '/r/voice/transcribe')).length, 0, 'silence is not sent');
    assert.deepEqual(problems, []);
    await ctx.close();
  }
  // The level meter while it hears; then the Mac fails, then answers empty.
  for (const [reply, text] of [[{ status: 500, body: { ok: false, error: 'boom' } }, D.macFailed], [{ status: 200, body: { text: ' ', masked: 0 } }, D.macNoWords]]) {
    await ctl({ reset: true, transcribeReplies: [reply] });
    const { ctx, page, problems } = await open();
    await recordOnce(page, { hold: 1200 });
    assert.ok(Number(await page.getAttribute('#rec', 'data-level')) > 0.1, 'the meter moves');
    assert.notEqual(await page.getAttribute('#rec', 'data-quiet'), 'true');
    if (reply.status === 500) await shot(page, '88-mac-level-meter');
    await page.tap('#mic');
    await idleWith(page, text, `reply ${reply.status}`);
    if (reply.status === 500) await shot(page, '89-mac-failed-sentence');
    assert.equal(await page.inputValue('#field'), '');
    assert.equal((await calls('POST', '/r/voice/transcribe')).length, 1);
    assert.deepEqual(problems, []);
    await ctx.close();
  }
  // The microphone taken away mid-recording: what was recorded is kept.
  await ctl({ reset: true });
  {
    const { ctx, page, problems } = await open();
    await recordOnce(page, { hold: 1000 });
    await page.evaluate(() => window.__stream.getAudioTracks()[0].dispatchEvent(new Event('ended')));
    await idleWith(page, `${D.taken} ${D.kept}`, 'taken');
    assert.match(await page.inputValue('#field'), /customer id/);
    assert.equal((await calls('POST', '/r/voice/transcribe')).length, 1, 'transcribed once');
    const spy = await page.evaluate(() => window.__spy);
    assert.ok(spy.stopped >= spy.tracks, 'the microphone is released');
    assert.deepEqual(problems, []);
    await ctx.close();
  }
});

// ── The bar under an iPhone status bar (home-screen app) ──
// Chromium has no safe area: a 59 pt inset (a Dynamic Island iPhone) is
// emulated through the page's own --safe-t token, in a context that lets the
// test add that one rule. Each screen's bar is then checked: nothing stacked
// above its text, no filter, backdrop blur or transform on it or an ancestor,
// its text below the inset on whole device pixels; and a DPR-3 crop is kept.
test('every bar under an emulated 59 pt status bar: solid, nothing over its text, sharp', { skip }, async () => {
  await ctl({ reset: true, paired: false, pair: ['pending'], leaders: FX.leaders, history: { shop: FX.history.shop }, l2Reset: true, kanbanReset: true });
  const ctx = await browser.newContext({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 3, isMobile: true, hasTouch: true, userAgent: IPHONE_UA, bypassCSP: true });
  await ctx.addInitScript(spies);
  const page = await ctx.newPage();
  const problems = [];
  page.on('pageerror', e => problems.push(e.message));
  const inset = async () => page.addStyleTag({ content: ':root { --safe-t: 59px; }' });
  const audit = async name => {
    const r = await page.evaluate(() => {
      const bar = document.getElementById('bar');
      const out = { bad: [], over: [], top: [] };
      for (let e = bar; e; e = e.parentElement) {
        const cs = getComputedStyle(e);
        if (cs.filter !== 'none' || cs.backdropFilter !== 'none' || (cs.webkitBackdropFilter && cs.webkitBackdropFilter !== 'none') || cs.transform !== 'none' || cs.willChange !== 'auto' || Number(cs.opacity) !== 1) {
          out.bad.push(`${e.tagName}#${e.id}: ${cs.filter} ${cs.backdropFilter} ${cs.transform} ${cs.willChange} ${cs.opacity}`);
        }
      }
      const bg = getComputedStyle(bar).backgroundColor;
      out.solid = /^rgb\(/.test(bg) || /, 1\)$/.test(bg);
      for (const el of [...bar.querySelectorAll('h1, .rc-sub, .rc-pill, .rc-mark, #back svg, #menu-btn svg, #approvals-btn svg')].filter(x => x.getClientRects().length && getComputedStyle(x).display !== 'none')) {
        const b = el.getBoundingClientRect();
        const hit = document.elementFromPoint(b.left + b.width / 2, b.top + b.height / 2);
        if (hit && !(el === hit || el.contains(hit) || hit.contains(el))) out.over.push(`${(el.className?.baseVal ?? el.className) || el.tagName} under ${hit.tagName}#${hit.id}.${hit.className}`);
        out.top.push([el.tagName, b.top, Math.abs(b.top * 3 - Math.round(b.top * 3)) < 0.01]);
      }
      return out;
    });
    assert.deepEqual(r.bad, [], `${name}: no filter, backdrop, transform or opacity on the bar or an ancestor`);
    assert.deepEqual(r.over, [], `${name}: nothing stacked over the bar's text`);
    assert.ok(r.solid, `${name}: a solid ground`);
    for (const [tag, top, whole] of r.top) {
      assert.ok(top >= 59, `${name}: ${tag} below the status bar (${top})`);
      assert.ok(whole, `${name}: ${tag} on whole device pixels (${top})`);
    }
    if (SHOTS) await page.screenshot({ path: `${SHOTS}/90-bar-${name}.png`, clip: { x: 0, y: 0, width: 390, height: 140 }, animations: 'disabled' });
  };
  await page.goto(`${base()}/#pair=CODE1234abcd`);
  await inset();
  await page.waitForSelector('#pair-form:not([hidden])');
  await audit('pairing');
  await ctl({ paired: true });
  await page.goto(`${base()}/`);
  await inset();
  await page.waitForSelector('.rc-leader');
  await audit('leaders');
  await page.tap('.rc-leader >> text=shop');
  await page.waitForSelector('.rc-agent');
  // The conversation scrolled under the bar, as in the person's screenshot.
  await page.evaluate(() => { document.getElementById('log').scrollTop = 120; });
  await audit('leader');
  await page.tap('#v-kanban');
  await page.waitForSelector('.rc-kb-lane.rc-on .rc-kb-card');
  await audit('kanban');
  await page.tap('#approvals-btn');
  await page.waitForSelector('.rc-ap-row');
  await audit('approvals');
  await page.tap('.rc-ap-plans');
  await page.waitForSelector('.rc-thread');
  await page.evaluate(() => { document.getElementById('s-item').scrollTop = 200; });
  await audit('plan');
  // The composer at the bottom: nothing over its field either.
  await page.tap('#back');
  await page.tap('#back');
  await page.waitForSelector('#s-chat:not([hidden])');
  await page.tap('#v-chat');
  const over = await page.evaluate(() => {
    const f = document.getElementById('field').getBoundingClientRect();
    const hit = document.elementFromPoint(f.left + f.width / 2, f.top + f.height / 2);
    return hit?.id;
  });
  assert.equal(over, 'field');
  assert.deepEqual(problems, []);
  await ctx.close();
});

// ── Security review P6: controls shown, strings isolated, the sheet unambiguous ──
test('level 2: bidi and invisible controls in agent text and paths are shown, never honoured', { skip }, async () => {
  const CC = ['\u202A', '\u202B', '\u202C', '\u202D', '\u202E', '\u2066', '\u2067', '\u2068', '\u2069', '\u200B', '\u200C', '\u200D', '\u200E', '\u200F', '\u2060', '\uFEFF', '\u00AD'];
  const all = CC.join('');
  const L = structuredClone(FX.level2);
  const RAW = /[\p{Cf}\u2028\u2029]/u;
  await ctl({ reset: true, paired: true, leaders: FX.leaders, l2Reset: true });
  const { ctx, page, problems } = await phone();
  await page.route('**/r/plans/p-*', async route => {
    const p = structuredClone(L.plan);
    p.plan.sandbox = `sh\u202Eop`;
    p.plan.summary = `Approve ${all} this`;
    p.plan.threads[0].name = `db\u200B-schema`;
    p.plan.threads[0].goal = `Goal with \u2066isolate\u2069 and soft\u00ADhyphen`;
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ ...p, digest: 'a'.repeat(64) }) });
  });
  await page.route('**/r/merges/p-*', async route => {
    const m = structuredClone(L.merge);
    m.merge.sandbox = 'web\u202Esite';
    m.merge.order = ['foo\u202Bter', 'contact-form'];
    m.merge.branches[0].branch = 'sbxw/foo\u200Fter';
    m.merge.branches[0].files[0].path = 'src/\u202Etxt.exe';
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(m) });
  });
  await openApprovals(page);
  await openItemOf(page, 'plans');
  await page.tap('.rc-fold').catch(() => {});
  const check = scope => page.evaluate(([sel, raw]) => {
    const root = document.querySelector(sel);
    const re = new RegExp(raw, 'u');
    return {
      raw: re.test(root.textContent),
      codes: [...root.querySelectorAll('.rc-cc')].map(e => e.textContent),
      isolated: [...root.querySelectorAll('.rc-data')].every(e => e.dir === 'auto' && getComputedStyle(e).unicodeBidi === 'isolate'),
      data: root.querySelectorAll('.rc-data').length,
    };
  }, [scope, RAW.source]);
  let r = await check('#item');
  assert.equal(r.raw, false, 'no raw control in the DOM text');
  for (const c of CC) assert.ok(r.codes.includes(`U+${c.codePointAt(0).toString(16).toUpperCase().padStart(4, '0')}`), `marker for ${c.codePointAt(0).toString(16)}`);
  assert.ok(r.isolated && r.data > 3, 'every data string isolated');
  await page.tap('[data-kind="plan_approve"]');
  await page.waitForSelector('#sheet:not([hidden])');
  r = await check('#sheet');
  assert.equal(r.raw, false);
  assert.deepEqual(r.codes, ['U+202E']);
  assert.equal(await page.textContent('#sheet-line'), 'Approve the plan of shU+202Eop as it is: it starts 2 threads.');
  assert.equal(await page.$eval('#sheet-line .rc-data', e => getComputedStyle(e).fontFamily.includes('mono') || getComputedStyle(e).fontFamily.includes('Menlo')), true, 'the name in monospace');
  await shot(page, '54-sheet-controls-shown');
  await page.tap('#sheet-cancel');
  await page.tap('#back');
  await openItemOf(page, 'merges');
  await page.tap('.rc-files-box summary >> nth=0');
  r = await check('#item');
  assert.equal(r.raw, false);
  assert.equal(await page.textContent('.rc-file-path'), 'src/U+202Etxt.exe', 'the path cannot pass for another file');
  assert.ok(r.codes.includes('U+202B') && r.codes.includes('U+200F'));
  await shot(page, '55-merge-controls-shown');
  await page.tap('[data-kind="merge"]');
  assert.equal(await page.textContent('#sheet-line'), 'Merge 2 branches of webU+202Esite in the order shown, stopping at the first conflict. Order: fooU+202Bter, contact-form.');
  assert.equal((await check('#sheet')).raw, false);
  await cleanSpy(page);
  assert.deepEqual(problems, []);
  await ctx.close();
});

// ── API-level2 v1.1 and v1.2 ──
test('Face ID setup: a hidden page keeps the request and carries on; a replaced request starts again, never guessed', { skip }, async () => {
  await ctl({ reset: true, paired: true, leaders: FX.leaders, l2Reset: true, l2: { approvals: { passkey: false } }, pkStates: ['pending'] });
  const { ctx, page, problems } = await phone();
  const vis = state => page.evaluate(v => {
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => v });
    document.dispatchEvent(new Event('visibilitychange'));
  }, state);
  await openApprovals(page);
  await page.tap('#ap-enable');
  await page.waitForSelector('#pk-check:not([hidden])');
  await page.waitForFunction(async () => (await (await fetch('/__log')).json()).some(e => e.path === '/r/passkey/status'));
  // The phone locks while the person types the digits at the Mac.
  await vis('hidden');
  await page.waitForTimeout(300);
  const n = (await calls('GET', '/r/passkey/status')).length;
  await ctl({ pkStates: ['granted'] });
  await page.waitForTimeout(3000);
  assert.equal((await calls('GET', '/r/passkey/status')).length, n, 'no polling while hidden');
  assert.equal(await page.getAttribute('#s-passkey', 'data-state'), 'pending', 'the ceremony is kept');
  // Back: one look at once, with the same id, and on.
  await vis('visible');
  await page.waitForSelector('#pk-go:not([hidden])', { timeout: 1500 });
  const st = await calls('GET', '/r/passkey/status');
  assert.equal(st.length, n + 1, 'one look on return');
  assert.equal(new Set(st.map(e => e.query)).size, 1, 'the same request_id throughout');
  await page.tap('#pk-go');
  await page.waitForSelector('#s-passkey[data-state="registered"]');
  assert.equal((await calls('POST', '/r/passkey/register'))[0].body.request_id, st[0].query.slice('?request_id='.length));
  assert.equal((await calls('POST', '/r/passkey/start')).length, 1, 'no new request was needed');
  // Another request replaced this one at the daemon: status says "none".
  await ctl({ l2Reset: true, l2: { approvals: { passkey: false } } });
  await page.tap('#back');
  await page.tap('#ap-enable');
  await page.waitForSelector('#pk-check:not([hidden])');
  await fetch(`${base()}/r/passkey/start`, { method: 'POST', body: '{}' }); // a newer request
  await page.waitForSelector('#s-passkey[data-state="expired"]', { timeout: 10000 });
  assert.equal(await page.textContent('#pk-text'), 'This request expired. Start again.');
  assert.equal((await calls('POST', '/r/passkey/register')).length, 1);
  // Leaving the Face ID screen drops the id: no more looks.
  await page.tap('#pk-again');
  await page.waitForSelector('#pk-check:not([hidden])');
  await page.tap('#back');
  await page.waitForSelector('#s-approvals:not([hidden])');
  const m = (await calls('GET', '/r/passkey/status')).length;
  await page.waitForTimeout(2500);
  assert.equal((await calls('GET', '/r/passkey/status')).length, m, 'the id went with the screen');
  await cleanSpy(page);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('bye "too_many": one sentence, no reconnect loop, a tap reconnects', { skip }, async () => {
  await ctl({ reset: true, paired: true, leaders: FX.leaders, l2Reset: true });
  const { ctx, page, problems } = await phone();
  await page.goto(`${base()}/`);
  await page.waitForSelector('.rc-leader');
  await page.waitForTimeout(300);
  const streams = async () => (await calls('GET', '/r/events')).length;
  const before = await streams();
  await ctl({ emit: { event: 'bye', data: { reason: 'too_many' } } });
  await page.waitForSelector('#elsewhere:not([hidden])');
  assert.equal(await page.textContent('#elsewhere'), 'This page is open elsewhere on this device. Tap to reconnect here.');
  assert.equal(await page.isVisible('#s-leaders'), true, 'still paired, still on its screen');
  await shot(page, '84-open-elsewhere');
  await page.evaluate(() => {
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'visible' });
    document.dispatchEvent(new Event('visibilitychange'));
  });
  await page.waitForTimeout(3500);
  assert.equal(await streams(), before, 'no reconnect by itself');
  await page.tap('#elsewhere');
  await page.waitForSelector('#elsewhere', { state: 'hidden' });
  await page.waitForFunction(async n => (await (await fetch('/__log')).json()).filter(e => e.path === '/r/events').length > n, before);
  assert.equal(await streams(), before + 1, 'one tap, one stream');
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('a merge "changed": its sentence shown as data, the detail read again', { skip }, async () => {
  const why = 'Branch sbxw/contact\u202E-form moved since you opened it; already merged: footer.';
  await ctl({ reset: true, paired: true, leaders: FX.leaders, l2Reset: true, l2Replies: { merge: [{ status: 409, body: { ok: false, reason: 'changed', error: why } }] } });
  const { ctx, page, problems } = await phone();
  await openApprovals(page);
  await openItemOf(page, 'merges');
  const reads = async () => (await log()).filter(e => e.method === 'GET' && e.path.startsWith('/r/merges/p-')).length;
  const r0 = await reads();
  await viaSheet(page, '[data-kind="merge"]');
  await page.waitForSelector('#item-status:not([hidden])');
  assert.equal(await page.textContent('#item-status'), 'This changed since you opened it: read it again. Branch sbxw/contactU+202E-form moved since you opened it; already merged: footer.');
  assert.equal(await page.$eval('#item-status .rc-data', e => e.dir), 'auto');
  await page.waitForSelector('#item-acts button');
  assert.equal(await reads(), r0 + 1, 'the merge is read again');
  await shot(page, '56-merge-changed-sentence');
  assert.deepEqual(problems, []);
  await ctx.close();
});
