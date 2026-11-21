// The notifications switch's logic (remote-core.js: rcPush, rcPushSupport,
// rcPushFail, rcClearBadge) against the stand-in's /r/push routes, with a
// fake service worker container and Notification. Also checks how remote.js
// wires it: Forget this device and a `bye` both end in unpaired(), which
// drops the browser's subscription. Run: node --test tests/js/push-page.test.mjs
import { test, after } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { startStandin } from './remote-standin.mjs';

const read = p => readFileSync(new URL(p, import.meta.url), 'utf8');
const coreSrc = read('../../assets/remote/remote-core.js');
const pageSrc = read('../../assets/remote/remote.js');
const htmlSrc = read('../../assets/remote/index.html');
const ctx = { URLSearchParams, AbortController, Uint8Array };
vm.runInNewContext(`${coreSrc}\n${['rcPush', 'rcPushSupport', 'rcPushFail', 'rcClearBadge', 'rcClient', 'RC_PUSH_TEXT', 'rcB64u'].map(n => `this.${n} = ${n};`).join('\n')}`, ctx);
const rc = ctx;

const so = await startStandin(0);
after(() => so.close());
const base = `http://127.0.0.1:${so.port}`;
const ctl = body => fetch(`${base}/__ctl`, { method: 'POST', body: JSON.stringify(body) });
const log = async () => (await fetch(`${base}/__log`)).json();
const pushLog = async () => (await log()).filter(e => e.path.startsWith('/r/push/') && e.method === 'POST');
const api = () => rc.rcClient((path, init) => fetch(base + path, init));

// A browser: a worker container and a Notification, recording what the page asks.
function browser({ permission = 'default', answer = 'granted', subscribeFails = false, hasSub = false, registerFails = false } = {}) {
  const b = { registered: [], asked: 0, subscribed: [], unsubscribed: 0, order: [], sub: null, Notif: { permission } };
  const mk = (key) => ({
    endpoint: 'https://web.push.apple.com/SECRETENDPOINT',
    getKey: n => new Uint8Array(n === 'auth' ? 16 : 65).fill(n === 'auth' ? 7 : 4),
    unsubscribe: async () => { b.unsubscribed++; b.sub = null; return true; },
    key,
  });
  if (hasSub) b.sub = mk('old');
  const pm = {
    getSubscription: async () => b.sub,
    subscribe: async o => {
      b.order.push('subscribe');
      b.subscribed.push(o);
      if (subscribeFails) throw new Error('no');
      b.sub = mk(o.applicationServerKey);
      return b.sub;
    },
  };
  const registration = { pushManager: pm };
  b.sw = {
    register: (url, o) => { b.order.push('register'); b.registered.push([url, o]); return registerFails ? Promise.reject(new Error('no')) : Promise.resolve(registration); },
    ready: Promise.resolve(registration),
    getRegistration: async () => registration,
  };
  b.Notif.requestPermission = async () => { b.order.push('permission'); b.asked++; b.Notif.permission = answer; return answer; };
  b.make = () => rc.rcPush({ api: api(), sw: b.sw, Notif: b.Notif });
  return b;
}
const reset = async (push = {}) => { await ctl({ reset: true, push: { on: false, subs: [], keyReply: null, subscribeReplies: [], ...push } }); };

test('support: needs service workers, PushManager and Notification; the iPhone line says Home Screen', () => {
  const all = { serviceWorker: true, PushManager: true, Notification: true };
  const IPHONE = 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 Version/18.0 Mobile/15E148 Safari/604.1';
  const MAC = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Version/18.0 Safari/605.1.15';
  assert.equal(rc.rcPushSupport(all, IPHONE), null);
  for (const miss of ['serviceWorker', 'PushManager', 'Notification']) {
    assert.equal(rc.rcPushSupport({ ...all, [miss]: false }, IPHONE), 'home', miss);
    assert.equal(rc.rcPushSupport({ ...all, [miss]: false }, MAC), 'unsupported', miss);
  }
  assert.match(rc.RC_PUSH_TEXT.home, /Home Screen/);
  assert.match(rc.RC_PUSH_TEXT.unsupported, /not supported/);
});

test('the page shows the switch only where supported, else one line', () => {
  assert.match(pageSrc, /rcPushSupport\(\{ serviceWorker: 'serviceWorker' in navigator, PushManager: 'PushManager' in window, Notification: 'Notification' in window \}/);
  assert.match(pageSrc, /if \(pushNo\) \{\s*\$\('push'\)\.hidden = true;[\s\S]*?RC_PUSH_TEXT\[pushNo\]/);
  assert.match(htmlSrc, /id="push"[^>]*role="switch"[^>]*aria-checked="false"[^>]*hidden/);
});

test('state: off when the Mac says off and nothing is subscribed', async () => {
  await reset();
  const b = browser();
  assert.deepEqual(plain(await b.make().state()), { state: 'off', text: '' });
});

test('state: on only when the Mac and the browser agree', async () => {
  await reset({ on: true });
  const b = browser({ permission: 'granted', hasSub: true });
  assert.equal((await b.make().state()).state, 'on');
});

test('state: disagreement shows off (the Mac says on, the browser has none; and the reverse)', async () => {
  await reset({ on: true });
  assert.equal((await browser({ permission: 'granted', hasSub: false }).make().state()).state, 'off');
  await reset({ on: false });
  assert.equal((await browser({ permission: 'granted', hasSub: true }).make().state()).state, 'off');
  await reset({ on: true });
  assert.equal((await browser({ permission: 'default', hasSub: true }).make().state()).state, 'off', 'permission gone');
});

test('state: refused permission says how to allow it again; an unreachable Mac says so', async () => {
  await reset();
  const r = await browser({ permission: 'denied' }).make().state();
  assert.equal(r.state, 'denied');
  assert.match(r.text, /Settings/);
  assert.match(r.text, /Notifications/);
  await ctl({ down: true });
  const u = await browser({ permission: 'granted' }).make().state();
  await ctl({ down: false });
  assert.equal(u.state, 'off');
  assert.match(u.text, /did not answer/);
});

test('enable: registers /sw.js, asks permission from the tap, subscribes userVisibleOnly with the key, posts exactly three fields', async () => {
  await reset();
  const b = browser();
  const push = b.make();
  await push.state(); // the key is read at load
  const r = await push.enable();
  assert.equal(r.ok, true);
  assert.deepEqual(plain(b.registered), [['/sw.js', { scope: '/' }]]);
  assert.deepEqual(b.order, ['register', 'permission', 'subscribe'], 'the prompt is not behind any network wait');
  assert.equal(b.subscribed.length, 1);
  assert.equal(b.subscribed[0].userVisibleOnly, true);
  assert.equal(b.subscribed[0].applicationServerKey.length, 65);
  const posts = (await pushLog()).filter(e => e.path === '/r/push/subscribe');
  assert.equal(posts.length, 1);
  assert.deepEqual(Object.keys(posts[0].body).sort(), ['auth', 'endpoint', 'p256dh']);
  assert.match(posts[0].body.p256dh, /^[A-Za-z0-9_-]{87}$/);
  assert.match(posts[0].body.auth, /^[A-Za-z0-9_-]{22}$/);
  assert.equal(posts[0].body.endpoint, 'https://web.push.apple.com/SECRETENDPOINT');
  assert.equal((await push.state()).state, 'on');
});

test('enable: with no key read yet it still works (reads it after the prompt)', async () => {
  await reset();
  const r = await browser().make().enable();
  assert.equal(r.ok, true);
});

test('enable: an old subscription is replaced (a new key cannot reuse it)', async () => {
  await reset();
  const b = browser({ hasSub: true });
  assert.equal((await b.make().enable()).ok, true);
  assert.equal(b.unsubscribed, 1);
  assert.equal(b.subscribed.length, 1);
});

test('enable: permission refused says how to allow it again, posts nothing', async () => {
  await reset();
  const b = browser({ answer: 'denied' });
  const r = await b.make().enable();
  assert.equal(r.ok, false);
  assert.equal(r.why, 'denied');
  assert.match(r.text, /Settings, then Notifications, then sbxw/);
  assert.equal(b.subscribed.length, 0);
  assert.equal((await pushLog()).length, 0);
  // Already refused earlier: no prompt at all.
  const b2 = browser({ permission: 'denied' });
  assert.equal((await b2.make().enable()).why, 'denied');
  assert.equal(b2.asked, 0);
});

test('enable: a dismissed prompt (default) is not a grant', async () => {
  await reset();
  const b = browser({ answer: 'default' });
  assert.equal((await b.make().enable()).why, 'denied');
  assert.equal(b.subscribed.length, 0);
});

test('enable: the browser failing to register or subscribe says so, posts nothing', async () => {
  await reset();
  const a = await browser({ registerFails: true }).make().enable();
  assert.equal(a.why, 'browser');
  const b = await browser({ subscribeFails: true }).make().enable();
  assert.equal(b.why, 'browser');
  assert.equal((await pushLog()).length, 0);
});

test('enable: the Mac refusing says so, with its plain reason word, and the browser side is undone', async () => {
  await reset({ subscribeReplies: [{ status: 400, body: { ok: false, error: 'bad endpoint', reason: 'endpoint' } }] });
  const b = browser();
  const r = await b.make().enable();
  assert.equal(r.ok, false);
  assert.equal(r.why, 'mac');
  assert.match(r.text, /^The Mac refused/);
  assert.match(r.text, /\(endpoint\)/);
  assert.equal(b.sub, null, 'rolled back');
  // A hostile reason is never shown.
  await reset({ subscribeReplies: [{ status: 400, body: { reason: '<b>x</b> '.repeat(50) } }] });
  const h = await browser().make().enable();
  assert.doesNotMatch(h.text, /</);
});

test('enable: an unreachable Mac says so and undoes the subscription', async () => {
  await reset();
  const b = browser();
  const push = b.make();
  await push.state();
  await ctl({ down: true });
  const r = await push.enable();
  await ctl({ down: false });
  assert.equal(r.why, 'unreachable');
  assert.equal(b.sub, null);
});

test('enable: a Mac with no key (answers 404) is refused without subscribing', async () => {
  await reset({ keyReply: { status: 404, body: { ok: false, error: 'not found' } } });
  const b = browser();
  const r = await b.make().enable();
  assert.equal(r.why, 'mac');
  assert.equal(b.subscribed.length, 0);
});

test('disable: unsubscribes in the browser and posts unsubscribe', async () => {
  await reset({ on: true });
  const b = browser({ permission: 'granted', hasSub: true });
  const r = await b.make().disable();
  assert.equal(r.ok, true);
  assert.equal(b.unsubscribed, 1);
  const calls = await pushLog();
  assert.deepEqual(calls.map(c => c.path), ['/r/push/unsubscribe']);
  assert.deepEqual(calls[0].body, {});
});

test('disable: the Mac not answering still leaves the browser unsubscribed, and says so', async () => {
  await reset({ on: true });
  const b = browser({ permission: 'granted', hasSub: true });
  await ctl({ down: true });
  const r = await b.make().disable();
  await ctl({ down: false });
  assert.equal(r.ok, false);
  assert.equal(b.unsubscribed, 1);
  assert.match(r.text, /off on this phone/);
});

test('dropBrowser unsubscribes in the browser only and never throws', async () => {
  await reset({ on: true });
  const b = browser({ permission: 'granted', hasSub: true });
  await b.make().dropBrowser();
  assert.equal(b.unsubscribed, 1);
  assert.equal((await pushLog()).length, 0, 'no request');
  await b.make().dropBrowser(); // none left
  const broken = rc.rcPush({ api: api(), sw: { getRegistration: async () => { throw new Error('x'); } }, Notif: {} });
  await broken.dropBrowser();
});

test('Forget this device and a bye both end in unpaired(), which drops the browser subscription', () => {
  const unp = pageSrc.slice(pageSrc.indexOf('function unpaired('), pageSrc.indexOf('// ── Pairing ──'));
  assert.match(unp, /if \(push\) push\.dropBrowser\(\)/);
  const bye = pageSrc.slice(pageSrc.indexOf("es.addEventListener('bye'"), pageSrc.indexOf("es.addEventListener('error'"));
  assert.match(bye, /unpaired\('Remote access is off'/);
  assert.match(bye, /unpaired\('This device is no longer paired'/);
  assert.match(bye, /too_many[^\n]*elsewhere\(true\)/, 'a second view does not unsubscribe this device');
  const forget = pageSrc.slice(pageSrc.indexOf("$('forget').addEventListener"), pageSrc.indexOf('// ── Text that is not the page'));
  assert.match(forget, /unpaired\('This device was forgotten'/);
});

test('the page clears the app badge when visible, only via rcClearBadge', async () => {
  assert.match(pageSrc, /const clearBadge = \(\) => \{ if \(document\.visibilityState === 'visible'\) rcClearBadge\(navigator\); \};/);
  assert.match(pageSrc, /document\.addEventListener\('visibilitychange', clearBadge\);\s*\n\s*clearBadge\(\);/);
  let n = 0;
  await rc.rcClearBadge({ clearAppBadge: async () => { n++; } });
  assert.equal(n, 1);
  await rc.rcClearBadge({});
  await rc.rcClearBadge({ clearAppBadge: async () => { throw new Error('x'); } });
  await rc.rcClearBadge({ clearAppBadge() { throw new Error('x'); } });
  await rc.rcClearBadge(undefined);
});

test('no subscription detail reaches the console or storage', () => {
  const core = coreSrc.slice(coreSrc.indexOf('// ── Notifications (Web Push'));
  assert.doesNotMatch(core, /console\./);
  assert.doesNotMatch(core, /localStorage|sessionStorage|indexedDB|caches\./);
  const wiring = pageSrc.slice(pageSrc.indexOf('// ── Notifications: a switch'), pageSrc.indexOf("$('menu-btn').addEventListener('click'"));
  assert.doesNotMatch(wiring, /console\.|innerHTML|localStorage/);
});

test('the stand-in serves /sw.js from the root', async () => {
  const res = await fetch(`${base}/sw.js`);
  assert.equal(res.status, 200);
  assert.match(res.headers.get('content-type'), /javascript/);
  assert.equal(await res.text(), readFileSync(new URL('../../assets/remote/sw.js', import.meta.url), 'utf8'));
});

function plain(v) { return JSON.parse(JSON.stringify(v)); }
