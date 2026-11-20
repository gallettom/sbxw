// Thread phone-live: a leader's reply appears on the open phone page by
// itself (.sbxw-artifacts/reports/phone-live.md). Two parts:
// * remote-core.js's merge: Stop's early (provisional) reply stays until the
//   transcript's copy of the same text, or a newer reply, takes its place;
// * the page in a real browser against the stand-in: a `history` event draws
//   the reply with no tap, an item that is not the reply does not take it
//   away, coming back from a locked screen reopens a dead stream and catches
//   up, and nothing polls meanwhile.
// The browser part needs Playwright and a Chromium, as remote-page.test.mjs
// (PLAYWRIGHT_MODULE, CHROMIUM_PATH); without them it is skipped.
// Run: node --test tests/js/remote-live.test.mjs
import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import vm from 'node:vm';
import { startStandin } from './remote-standin.mjs';

const read = p => readFileSync(new URL(p, import.meta.url), 'utf8');
const FX = JSON.parse(read('./remote-fixtures.json'));
const rc = { URLSearchParams, AbortController };
vm.runInNewContext(`${read('../../assets/remote/remote-core.js')}\nthis.rcMerge = rcMerge; this.rcRows = rcRows; this.RC_AWAY_MS = RC_AWAY_MS;`, rc);
const seqs = items => Array.from(items, i => Number(i.seq));

const REPLY = 'PostgreSQL it is.';
const early = { seq: 113, at: 1, kind: 'assistant_text', text: REPLY, provisional: true };

// ── The merge ──
test('an item that is not the reply leaves the early reply in place', () => {
  const have = [{ seq: 112, kind: 'user_prompt', text: 'Which database?' }, early];
  // The transcript's tail at Stop, without the reply yet: steps, a prompt.
  for (const other of [{ seq: 114, kind: 'steps', count: 2 }, { seq: 114, kind: 'user_prompt', text: 'and the cache?' }, { seq: 114, kind: 'assistant_text', text: 'Something else.' }]) {
    const out = rc.rcMerge(have, [other]);
    assert.deepEqual(seqs(out), [112, 113, 114], JSON.stringify(other));
  }
});
test('the transcript\'s copy of the same text settles it; a newer early reply replaces it', () => {
  const have = [early];
  assert.deepEqual(seqs(rc.rcMerge(have, [{ seq: 115, kind: 'assistant_text', text: `${REPLY}\n` }])), [115]);
  assert.deepEqual(seqs(rc.rcMerge(have, [{ seq: 116, kind: 'assistant_text', text: 'Next one.', provisional: true }])), [116]);
  // An older page never settles a newer early reply.
  assert.deepEqual(seqs(rc.rcMerge(have, [{ seq: 90, kind: 'assistant_text', text: REPLY }])), [90, 113]);
  assert.equal(rc.rcRows(rc.rcMerge(have, [{ seq: 114, kind: 'steps', count: 1 }])).filter(r => r.kind === 'agent').length, 1);
});

// ── The page ──
let pw = null;
try { pw = await import(process.env.PLAYWRIGHT_MODULE || 'playwright-core'); } catch (_) {}
const exe = process.env.CHROMIUM_PATH || '/usr/bin/chromium';
const skip = !pw ? 'no Playwright (set PLAYWRIGHT_MODULE)' : !existsSync(exe) ? `no Chromium at ${exe}` : false;
const IPHONE_UA = 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1';

let sb, browser;
before(async () => {
  if (skip) return;
  sb = await startStandin(0);
  browser = await pw.chromium.launch({ executablePath: exe });
});
after(async () => {
  await browser?.close();
  await sb?.close();
});
const base = () => `http://127.0.0.1:${sb.port}`;
const ctl = body => fetch(`${base()}/__ctl`, { method: 'POST', body: JSON.stringify(body) });
const log = async () => (await fetch(`${base()}/__log`)).json();
const reads = async () => (await log()).filter(e => e.method === 'GET' && e.path === '/r/leaders/shop/history' && e.query.includes('after=')).length;
const streams = async () => (await log()).filter(e => e.path === '/r/events').length;
const page1 = items => ({ sandbox: 'shop', session_id: 's1', page: { items, latest_seq: Math.max(...items.map(i => i.seq)), truncated_before: false, partial: false, ended: false } });
const agentTexts = page => page.$$eval('#rows .rc-agent', els => els.map(e => e.textContent));

async function openShop() {
  await ctl({ reset: true, paired: true, mute: true, newerHold: false, leaders: FX.leaders, history: { shop: FX.history.shop }, newer: { shop: page1([]) } });
  const ctx = await browser.newContext({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true, userAgent: IPHONE_UA });
  const page = await ctx.newPage();
  const problems = [];
  page.on('pageerror', e => problems.push(`pageerror: ${e.message}`));
  await page.goto(`${base()}/`);
  await page.tap('.rc-leader >> text=shop');
  await page.waitForSelector('#s-chat:not([hidden])');
  await page.waitForFunction(() => document.querySelectorAll('#rows > *').length > 0);
  // The stream is open (its `open` event may read once): let it settle.
  await page.waitForTimeout(300);
  return { ctx, page, problems };
}
const setVisibility = (page, v) => page.evaluate(v => {
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => v });
  document.dispatchEvent(new Event('visibilitychange'));
}, v);

test('a history event draws the reply with no tap, and a later step does not take it away', { skip }, async () => {
  const { ctx, page, problems } = await openShop();
  const before = await agentTexts(page);
  assert.ok(!before.some(t => t.includes(REPLY)));
  await ctl({ newer: { shop: page1([early]) } });
  const t0 = Date.now();
  await ctl({ emit: { event: 'history', data: { sandbox: 'shop', seq: 113 } } });
  await page.waitForFunction(r => [...document.querySelectorAll('#rows .rc-agent')].some(e => e.textContent.includes(r)), REPLY, { timeout: 2000 });
  assert.ok(Date.now() - t0 < 2000);
  // The transcript's tail without the reply: a run of steps.
  await ctl({ newer: { shop: page1([{ seq: 114, at: 2, kind: 'steps', count: 2 }]) } });
  await ctl({ emit: { event: 'history', data: { sandbox: 'shop', seq: 114 } } });
  await page.waitForFunction(() => document.querySelector('#rows').textContent.includes('2 steps') || document.querySelectorAll('#rows > *').length > 0);
  await page.waitForTimeout(400);
  assert.equal((await agentTexts(page)).filter(t => t.includes(REPLY)).length, 1, 'the reply stays on screen');
  // Its transcript copy: still one.
  await ctl({ newer: { shop: page1([{ seq: 115, at: 3, kind: 'assistant_text', text: REPLY }]) } });
  await ctl({ emit: { event: 'history', data: { sandbox: 'shop', seq: 115 } } });
  await page.waitForTimeout(400);
  assert.equal((await agentTexts(page)).filter(t => t.includes(REPLY)).length, 1, 'no double');
  // The page never asked for a message to be sent.
  assert.equal((await log()).filter(e => e.method === 'POST').length, 0);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('no polling: an idle open conversation makes no history read', { skip }, async () => {
  const { ctx, page, problems } = await openShop();
  const n = await reads();
  await page.waitForTimeout(3000);
  assert.equal(await reads(), n, 'the stream is the trigger');
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('back from a locked screen: a dead stream is opened again and the reply drawn, with no tap', { skip }, async () => {
  const { ctx, page, problems } = await openShop();
  const s0 = await streams();
  // The phone sleeps; its stream dies without a word; the leader answers.
  await setVisibility(page, 'hidden');
  await ctl({ mute: true, newer: { shop: page1([{ ...early, provisional: false }]) } });
  await page.waitForTimeout(rc.RC_AWAY_MS + 300);
  assert.ok(!(await agentTexts(page)).some(t => t.includes(REPLY)), 'nothing came while asleep');
  await setVisibility(page, 'visible');
  await page.waitForFunction(r => [...document.querySelectorAll('#rows .rc-agent')].some(e => e.textContent.includes(r)), REPLY, { timeout: 2000 });
  assert.equal(await streams(), s0 + 1, 'one new stream');
  // A short look away (under RC_AWAY_MS) keeps the stream and reads once.
  const s1 = await streams();
  const r1 = await reads();
  await setVisibility(page, 'hidden');
  await setVisibility(page, 'visible');
  await page.waitForTimeout(300);
  assert.equal(await streams(), s1);
  assert.equal(await reads(), r1 + 1);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('a history read with no answer does not hold back the reads after it', { skip }, async () => {
  const { ctx, page, problems } = await openShop();
  const r0 = await reads();
  await ctl({ newerHold: true });
  await ctl({ emit: { event: 'history', data: { sandbox: 'shop', seq: 113 } } });
  await page.waitForFunction(async n => (await (await fetch('/__log')).json()).filter(e => e.query.includes('after=')).length > n, r0);
  // A second nudge while it hangs: it waits behind it...
  await ctl({ emit: { event: 'history', data: { sandbox: 'shop', seq: 113 } } });
  await page.waitForTimeout(300);
  assert.equal(await reads(), r0 + 1);
  // ...until the hung read is older than RC_STUCK_MS (the page's clock moves on).
  await page.evaluate(() => { const now = Date.now.bind(Date); Date.now = () => now() + 16000; });
  await ctl({ emit: { event: 'history', data: { sandbox: 'shop', seq: 113 } } });
  await page.waitForTimeout(300);
  assert.equal(await reads(), r0 + 2, 'a fresh read went out');
  await ctl({ newer: { shop: page1([early]) } });
  await ctl({ newerHold: false });
  await page.waitForFunction(r => [...document.querySelectorAll('#rows .rc-agent')].some(e => e.textContent.includes(r)), REPLY, { timeout: 2000 });
  assert.deepEqual(problems, []);
  await ctx.close();
});
