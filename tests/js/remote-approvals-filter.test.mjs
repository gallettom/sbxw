// The Approvals sandbox filter, the badge rule (plans and merges are the
// number, network requests a dot) and the "a plan waits" toast, in a real
// browser against the stand-in (no daemon, no token). Skipped without
// Playwright and a Chromium, like remote-page.test.mjs (same variables).
// REMOTE_SHOTS=<dir> writes the screenshots at 390x844.
// Run: node --test tests/js/remote-approvals-filter.test.mjs
import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, mkdirSync, readFileSync } from 'node:fs';
import { startStandin } from './remote-standin.mjs';

let pw = null;
try { pw = await import(process.env.PLAYWRIGHT_MODULE || 'playwright-core'); } catch (_) {}
const exe = process.env.CHROMIUM_PATH || '/usr/bin/chromium';
const skip = !pw ? 'no Playwright (set PLAYWRIGHT_MODULE)' : !existsSync(exe) ? `no Chromium at ${exe}` : false;
const SHOTS = process.env.REMOTE_SHOTS || '';
if (SHOTS) mkdirSync(SHOTS, { recursive: true });
const FX = JSON.parse(readFileSync(new URL('./remote-fixtures.json', import.meta.url), 'utf8'));
const IPHONE_UA = 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1';

let sb, browser;
before(async () => {
  if (skip) return;
  sb = await startStandin(0);
  browser = await pw.chromium.launch({ executablePath: exe });
});
after(async () => { await browser?.close(); await sb?.close(); });

const base = () => `http://127.0.0.1:${sb.port}`;
const ctl = body => fetch(`${base()}/__ctl`, { method: 'POST', body: JSON.stringify(body) });
const log = async () => (await fetch(`${base()}/__log`)).json();
const shot = async (page, name) => { if (SHOTS) await page.screenshot({ path: `${SHOTS}/${name}.png`, animations: 'disabled' }); };
async function phone() {
  const ctx = await browser.newContext({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 2, isMobile: true, hasTouch: true, userAgent: IPHONE_UA });
  const page = await ctx.newPage();
  const problems = [];
  page.on('pageerror', e => problems.push(`pageerror: ${e.message}`));
  return { ctx, page, problems };
}
const PL = (n, sandbox, summary) => ({ id: `p-${String(n).repeat(32)}`, sandbox, summary, threads: 2, created_ms: 1763650000000 });
const LISTS = (extra = {}) => ({
  plans: { plans: [PL(1, 'shop', 'Orders table'), PL(3, 'website', 'Redesign the footer')] },
  merges: { merges: [{ id: `p-${'2'.repeat(32)}`, sandbox: 'website', variant: 'split', branches: 2 }] },
  network: { requests: [
    { id: 'network:a1', sandbox: 'docs', host: 'registry.npmjs.org', options: [{ id: 'dismiss', label: 'dismiss' }] },
    { id: 'network:a2', sandbox: 'docs', host: 'github.com', options: [{ id: 'dismiss', label: 'dismiss' }] },
    { id: 'network:a3', sandbox: 'shop', host: 'pypi.org', options: [{ id: 'dismiss', label: 'dismiss' }] }] },
  stopped: { sandboxes: [{ name: 'old-experiment', agent: 'claude', chat: false, workspace: '/w/old', created_at: '2026-10-02T09:12:00Z' }] },
  ...extra,
});
const COUNTS = { plans: 2, merges: 1, network: 3, stopped: 1 };
const setup = (lists = LISTS(), counts = COUNTS) => ctl({ reset: true, paired: true, leaders: FX.leaders, l2Reset: true, l2Lists: lists, l2: { approvals: { counts } } });
const chips = page => page.$$eval('#ap-filter .rc-ap-fchip', b => b.map(x => ({ name: x.querySelector('.rc-ap-fname').textContent, n: x.querySelector('.rc-kb-count')?.textContent || '', dot: !!x.querySelector('.rc-ap-dot'), on: x.getAttribute('aria-selected') === 'true' })));
// A heading is its title and, in its own span, the count: read as "title count".
const heads = page => page.$$eval('#ap-lists .rc-ap-h', h => h.map(x => [...x.querySelectorAll('.rc-ap-ht, .rc-ap-n')].map(e => e.textContent).join(' ') || x.textContent.trim()));
async function open(page, from) {
  await page.goto(`${base()}/`);
  await page.waitForSelector('.rc-leader');
  if (from) { await page.tap(`.rc-leader >> text=${from}`); await page.waitForSelector('#s-chat:not([hidden])'); }
  await page.tap('#approvals-btn');
  await page.waitForSelector('.rc-ap-row');
}

test('filter: entries and counts, each kind filtered, headings follow, badge keeps the total', { skip }, async () => {
  await setup();
  const { ctx, page, problems } = await phone();
  await open(page);
  assert.equal(await page.textContent('#badge'), '3'); // the button is hidden here, its content is not
  assert.equal(await page.$eval('#badge-dot', e => !e.hidden), true);
  assert.deepEqual(await chips(page), [
    { name: 'All sandboxes', n: '3', dot: true, on: true },
    { name: 'shop', n: '1', dot: true, on: false },
    { name: 'docs', n: '', dot: true, on: false },
    { name: 'website', n: '2', dot: false, on: false }]);
  assert.deepEqual(await heads(page), ['Plans to approve 2', 'Merges proposed 1', 'Network requests 3'], 'no stopped sandboxes in Approvals');
  await shot(page, 'all-sandboxes');
  await page.tap('#ap-filter [data-sandbox="website"]');
  assert.deepEqual(await heads(page), ['Plans to approve 1', 'Merges proposed 1']);
  await page.tap('#ap-filter [data-sandbox="docs"]');
  assert.deepEqual(await heads(page), ['Network requests 2']);
  await page.tap('#ap-filter [data-sandbox="shop"]');
  assert.deepEqual(await heads(page), ['Plans to approve 1', 'Network requests 1']);
  await shot(page, 'one-sandbox');
  assert.equal(await page.textContent('#badge'), '3', 'the badge keeps the total');
  // nothing is sent by filtering
  assert.equal((await log()).filter(e => e.method === 'POST').length, 0);
  // a row still opens its own item
  await page.tap('.rc-ap-plans');
  await page.waitForSelector('#s-item:not([hidden])');
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('filter: starts on the leader being looked at when it has something waiting', { skip }, async () => {
  await setup();
  const { ctx, page } = await phone();
  await open(page, 'shop');
  assert.deepEqual((await chips(page)).filter(c => c.on).map(c => c.name), ['shop']);
  await page.tap('#ap-filter .rc-ap-fchip >> text=All sandboxes');
  assert.equal((await heads(page)).length, 3);
  await ctx.close();
  // a leader with nothing waiting: All
  const b = await phone();
  await open(b.page, 'payments-service-with-a-very-long-sandbox-name');
  assert.deepEqual((await chips(b.page)).filter(c => c.on).map(c => c.name), ['All sandboxes']);
  await b.ctx.close();
});

test('home: stopped sandboxes are its last group, folded past three; Waiting for you has three tiles', { skip }, async () => {
  const many = Array.from({ length: 5 }, (_, i) => ({ name: `old-${i}`, agent: 'claude', chat: false, workspace: `/Users/me/dev/a-very-long-workspace-path-for-old-${i}`, created_at: '2026-10-02T09:12:00Z' }));
  await setup(LISTS({ stopped: { sandboxes: many } }), { ...COUNTS, stopped: 5 });
  const { ctx, page, problems } = await phone();
  await page.goto(`${base()}/`);
  await page.waitForSelector('.rc-stopped');
  assert.deepEqual(await page.$$eval('.rc-wtile .rc-wl', t => t.map(e => e.textContent)), ['Plans', 'Merges', 'Network']);
  const titles = await page.$$eval('#leaders .rc-group-title', h => h.map(e => e.textContent));
  assert.equal(titles.length, 3);
  assert.equal(titles.at(-1), 'Stopped sandboxes5', 'last, with its count');
  assert.equal(await page.$eval('#leaders > li:last-child', e => e.classList.contains('rc-group-foot')), true, 'the group is last, ending with Show all');
  assert.equal(await page.$$eval('.rc-stopped', b => b.length), 3, 'folded');
  assert.equal(await page.textContent('.rc-group-more'), 'Show all (5)');
  assert.equal(await page.textContent('.rc-stopped .rc-leader-line'), 'claude · /Users/me/dev/a-very-long-workspace-path-for-old-0');
  await shot(page, 'home-stopped');
  await page.tap('.rc-group-more');
  assert.equal(await page.$$eval('.rc-stopped', b => b.length), 5, 'Show all');
  assert.equal(await page.textContent('.rc-group-more'), 'Show fewer');
  // A tap opens the detail; Back is the home again.
  await page.tap('.rc-stopped >> text=old-1');
  await page.waitForSelector('#s-item:not([hidden])');
  await page.tap('#back');
  await page.waitForSelector('#s-leaders:not([hidden])');
  // No Face ID key: the list stays, with the sentence.
  await ctl({ l2: { approvals: { passkey: false } } });
  await page.goto(`${base()}/`);
  await page.waitForSelector('.rc-group-note');
  assert.match(await page.textContent('.rc-group-note'), /needs Face ID/);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('filter: the choice is remembered for the session', { skip }, async () => {
  await setup();
  const { ctx, page } = await phone();
  await open(page);
  await page.tap('#ap-filter [data-sandbox="docs"]');
  await page.tap('#back');
  await page.waitForSelector('.rc-leader');
  await page.tap('#approvals-btn');
  await page.waitForSelector('.rc-ap-row');
  assert.deepEqual((await chips(page)).filter(c => c.on).map(c => c.name), ['docs']);
  await ctx.close();
});

test('filter: an emptied sandbox says so and one tap goes back to All; live updates keep the choice', { skip }, async () => {
  await setup();
  const { ctx, page } = await phone();
  await open(page);
  await page.tap('#ap-filter [data-sandbox="shop"]');
  // shop's plan and request are settled on the Mac; the stream says counts changed
  await ctl({ l2Lists: { plans: { plans: [PL(3, 'website', 'Redesign the footer')] }, network: { requests: LISTS().network.requests.slice(0, 2) } } });
  await ctl({ emit: { event: 'approvals', data: { plans: 1, merges: 1, network: 2, stopped: 1 } } });
  await page.waitForSelector('#ap-none:not([hidden])');
  assert.equal(await page.textContent('#ap-none-text'), 'Nothing waits in shop');
  assert.equal(await page.isVisible('#ap-empty'), false);
  assert.deepEqual((await chips(page)).find(c => c.on), { name: 'shop', n: '', dot: false, on: true });
  await shot(page, 'nothing-waits');
  await page.tap('#ap-none-all');
  assert.equal(await page.isHidden('#ap-none'), true);
  assert.equal((await chips(page)).find(c => c.on).name, 'All sandboxes');
  // live, keeping the choice: website chosen, a new plan arrives for website
  await page.tap('#ap-filter [data-sandbox="website"]');
  await ctl({ l2Lists: { plans: { plans: [PL(3, 'website', 'Redesign the footer'), PL(4, 'website', 'Another')] } } });
  await ctl({ emit: { event: 'approvals', data: { plans: 2, merges: 1, network: 2, stopped: 1 } } });
  await page.waitForFunction(() => document.querySelectorAll('.rc-ap-plans').length === 2);
  assert.deepEqual((await chips(page)).find(c => c.on), { name: 'website', n: '3', dot: false, on: true });
  await ctx.close();
});

test('filter: a name full of markup is text, never an element', { skip }, async () => {
  const evil = '<img src=x onerror=alert(1)>';
  const lists = LISTS({ plans: { plans: [PL(1, evil, '<b>bold</b>')] } });
  await setup(lists, { plans: 1, merges: 1, network: 3, stopped: 1 });
  const { ctx, page, problems } = await phone();
  await open(page);
  assert.equal(await page.$$eval('#s-approvals img, #s-approvals b', e => e.length), 0);
  assert.ok((await chips(page)).some(c => c.name === evil));
  await page.tap('#ap-filter .rc-ap-fchip:nth-child(2)');
  assert.equal(await page.$$eval('#s-approvals img, #s-approvals b', e => e.length), 0);
  assert.deepEqual(problems, []);
  await ctx.close();
});

test('badge: plans and merges are the number, network a dot, stopped nothing', { skip }, async () => {
  const { ctx, page } = await phone();
  const L = n => ({ plans: n[0], merges: n[1], network: n[2], stopped: n[3] });
  await setup(LISTS(), L([1, 1, 7, 0]));
  await page.goto(`${base()}/`);
  await page.waitForSelector('#badge:not([hidden])');
  assert.equal(await page.textContent('#badge'), '2');
  assert.equal(await page.isVisible('#badge-dot'), true);
  assert.match(await page.getAttribute('#approvals-btn', 'aria-label'), /Network requests wait/);
  await shot(page, 'badge-dot');
  await ctl({ emit: { event: 'approvals', data: L([0, 0, 3, 0]) } });
  await page.waitForSelector('#badge[hidden]', { state: 'attached' });
  assert.equal(await page.isVisible('#badge-dot'), true);
  await ctl({ emit: { event: 'approvals', data: L([0, 0, 0, 2]) } });
  await page.waitForSelector('#badge-dot[hidden]', { state: 'attached' });
  assert.equal(await page.isVisible('#badge'), false);
  await ctx.close();
});

test('toast: only when plans or merges go up, once, and a tap opens Approvals on the sandbox', { skip }, async () => {
  await setup(LISTS(), { plans: 1, merges: 0, network: 0, stopped: 0 });
  await ctl({ l2Lists: { plans: { plans: [PL(1, 'shop', 'Orders table')] }, merges: { merges: [] } } });
  const { ctx, page } = await phone();
  await page.goto(`${base()}/`);
  await page.waitForSelector('#badge:not([hidden])');
  assert.equal(await page.isVisible('#ap-toast'), false, 'first load: nothing');
  await ctl({ emit: { event: 'approvals', data: { plans: 1, merges: 0, network: 0, stopped: 0 } } });
  await ctl({ emit: { event: 'approvals', data: { plans: 1, merges: 0, network: 5, stopped: 3 } } });
  await page.waitForTimeout(400);
  assert.equal(await page.isVisible('#ap-toast'), false, 'same count, network and stopped: nothing');
  await ctl({ l2Lists: { plans: { plans: [PL(1, 'shop', 'Orders table'), PL(3, 'website', 'Secret footer title')] } } });
  await ctl({ emit: { event: 'approvals', data: { plans: 2, merges: 0, network: 5, stopped: 3 } } });
  await page.waitForSelector('#ap-toast:not([hidden])');
  assert.equal(await page.textContent('#ap-toast-text'), 'A plan waits for you');
  assert.equal(await page.textContent('#ap-toast-sb'), 'website');
  assert.ok(!(await page.textContent('#ap-toast')).includes('Secret footer title'), 'no plan text in the toast');
  await shot(page, 'toast');
  await ctl({ emit: { event: 'approvals', data: { plans: 2, merges: 0, network: 5, stopped: 3 } } });
  await page.waitForTimeout(300);
  assert.equal(await page.$$eval('#ap-toast:not([hidden])', e => e.length), 1);
  await page.tap('#ap-toast-go');
  await page.waitForSelector('#s-approvals:not([hidden])');
  await page.waitForSelector('.rc-ap-row');
  assert.equal(await page.isHidden('#ap-toast'), true);
  assert.deepEqual((await chips(page)).filter(c => c.on).map(c => c.name), ['website']);
  await ctx.close();
});

test('network detail: a request sbx issued again under a new id opens under the new id', { skip }, async () => {
  await setup();
  const { ctx, page } = await phone();
  await open(page);
  // The sandbox retried: sbx now lists pypi.org under a new id; the page still holds the old one.
  const re = LISTS().network.requests.map(r => (r.id === 'network:a3' ? { ...r, id: 'network:b9' } : r));
  await ctl({ l2Lists: { network: { requests: re } }, l2Gone: ['network:a3'] });
  await page.tap('#ap-filter [data-sandbox="shop"]');
  await page.tap('.rc-ap-network');
  await page.waitForSelector('#item-acts button');
  const gets = (await log()).filter(e => e.method === 'GET' && e.path.startsWith('/r/network/')).map(e => decodeURIComponent(e.path.slice(11)));
  assert.deepEqual(gets, ['network:a3', 'network:b9']);
  assert.notEqual(await page.textContent('#item'), 'This is not waiting any more.');
  await ctx.close();
});

test('network detail: really gone says so; a request with nothing the phone may answer says the Mac decides', { skip }, async () => {
  await setup();
  const { ctx, page } = await phone();
  await open(page);
  await ctl({ l2Gone: ['network:a3'] });
  await page.tap('#ap-filter [data-sandbox="shop"]');
  await page.tap('.rc-ap-network');
  await page.waitForFunction(() => document.querySelector('#item')?.textContent.includes('not waiting any more'));
  await ctx.close();
});

test('network detail: nothing the phone may answer says the Mac decides', { skip }, async () => {
  const one = structuredClone(FX.level2.network_one);
  one.request.options = [];
  await setup();
  await ctl({ l2Lists: { network_one: one } });
  const { ctx, page } = await phone();
  await open(page);
  await page.tap('.rc-ap-network');
  await page.waitForFunction(() => document.querySelector('#item-acts')?.textContent.includes('Only the Mac can answer this request.'));
  assert.equal(await page.$$eval('#item-acts button', b => b.length), 0);
  await ctx.close();
});
