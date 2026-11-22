// The phone page's design screenshots, one state per shot, at 390x844,
// through the stand-in (fixture data only). The file names are the canvas
// screen keys, so a BEFORE and an AFTER round line up.
// Run: PLAYWRIGHT_MODULE=<playwright-core/index.mjs> CHROMIUM_PATH=/usr/bin/chromium \
//      node tests/js/remote-shots.mjs <outdir>
import { readFileSync, mkdirSync } from 'node:fs';
import { startStandin } from './remote-standin.mjs';
const pw = await import(process.env.PLAYWRIGHT_MODULE || 'playwright-core');
const OUT = process.argv[2];
if (!OUT) { console.error('usage: node tests/js/remote-shots.mjs <outdir>'); process.exit(2); }
mkdirSync(OUT, { recursive: true });
const sb = await startStandin(0);
const base = `http://127.0.0.1:${sb.port}`;
const ctl = b => fetch(`${base}/__ctl`, { method: 'POST', body: JSON.stringify(b) });
const br = await pw.chromium.launch({ executablePath: process.env.CHROMIUM_PATH || '/usr/bin/chromium' });
const IPHONE_UA = 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1';
async function phone(w, h) {
  const ctx = await br.newContext({ viewport: { width: w, height: h }, deviceScaleFactor: 2, isMobile: true, hasTouch: true, userAgent: IPHONE_UA });
  const page = await ctx.newPage();
  page.on('pageerror', e => console.error('pageerror', e.message));
  return { ctx, page };
}
const sleep = ms => new Promise(r => setTimeout(r, ms));
const FX = JSON.parse(readFileSync(new URL('./remote-fixtures.json', import.meta.url), 'utf8'));
// The fixture leaders as a v1.5 daemon says them: two team leaders, two sandboxes.
const leaders = structuredClone(FX.leaders);
for (const l of leaders.leaders) l.kind = ['shop', 'website'].includes(l.sandbox) ? 'leader' : 'sandbox';
leaders.leaders.find(l => l.sandbox === 'website').model = 'sonnet';
leaders.leaders.find(l => l.sandbox === 'shop').model = 'opus';
const now = Date.now() / 1000;
const SETUP = {
  reset: true, paired: true, leaders, l2Reset: true, kanbanReset: true, canvasReset: true,
  history: { shop: FX.history.shop, website: FX.history.website },
  models: { models: [{ id: 'sonnet', label: 'Sonnet' }, { id: 'opus', label: 'Opus' }, { id: 'haiku', label: 'Haiku' }] },
  usage: { five_hour_pct: 42, five_hour_resets_at: now + 2 * 3600 + 900, seven_day_pct: 78, seven_day_resets_at: now + 3 * 86400, updated_ms: Date.now() - 60000 },
};
const made = [];
async function snap(page, name) {
  await sleep(350);
  await page.screenshot({ path: `${OUT}/${name}.png`, animations: 'disabled' });
  made.push(name);
}
async function fresh(extra = {}) {
  await ctl({ ...SETUP, ...extra });
  const o = await phone(390, 844);
  return o;
}
async function home(page) { await page.goto(`${base}/`); await page.waitForSelector('.rc-leader'); }
async function chat(page, sb) { await home(page); await page.tap(`.rc-leader >> text=${sb}`); await page.waitForSelector('#s-chat:not([hidden])'); }

// 1. Home
{ const { ctx, page } = await fresh(); await home(page); await page.waitForSelector('.rc-wtile'); await page.waitForSelector('.rc-stopped'); await snap(page, 'phone-home'); await ctx.close(); }
// 2. Conversation idle
{ const { ctx, page } = await fresh(); await chat(page, 'website'); await page.waitForSelector('#ctl:not([hidden])'); await page.waitForSelector('#ctl-usage:not([hidden])').catch(() => {}); await snap(page, 'phone-chat-idle'); await ctx.close(); }
// 3. Conversation working
{
  const { ctx, page } = await fresh(); await chat(page, 'website');
  const w = structuredClone(leaders); w.leaders.find(l => l.sandbox === 'website').state = 'working';
  await ctl({ emit: { event: 'leaders', data: w } });
  await page.waitForSelector('#interrupt:visible');
  await snap(page, 'phone-chat-working'); await ctx.close();
}
// 4. A question card
{ const { ctx, page } = await fresh(); await chat(page, 'shop'); await page.waitForSelector('.rc-card-q .rc-opt'); await page.evaluate(() => document.getElementById('log').scrollTo(0, 1e6)); await snap(page, 'phone-question'); await ctx.close(); }
// 5. Approvals, 6. a plan, 7. the merge sheet
{
  const { ctx, page } = await fresh(); await home(page);
  await page.tap('#approvals-btn'); await page.waitForSelector('.rc-ap-row'); await snap(page, 'phone-approvals');
  await page.tap('.rc-ap-plans'); await page.waitForSelector('#item-acts button'); await snap(page, 'phone-plan');
  await page.tap('#back'); await page.waitForSelector('.rc-ap-row');
  await page.tap('.rc-ap-merges'); await page.waitForSelector('#item-acts button');
  await page.tap('[data-kind="merge"]'); await page.waitForSelector('#sheet:not([hidden])'); await snap(page, 'phone-merge-sheet');
  await ctx.close();
}
// 8. Kanban, 9. a ticket form
{
  const { ctx, page } = await fresh(); await chat(page, 'shop');
  await page.tap('#v-kanban'); await page.waitForSelector('.rc-kb-lane.rc-on .rc-kb-card'); await snap(page, 'phone-kanban');
  await page.tap('#kb-new'); await page.waitForSelector('#kb-form:not([hidden])');
  await page.fill('#kb-title', 'Add a search box to the orders list');
  await page.fill('#kb-desc', 'Filter by order number and status.');
  await page.evaluate(() => document.activeElement?.blur());
  await snap(page, 'phone-ticket-form'); await ctx.close();
}
// 10. Canvas list, 11. a shot
{
  const { ctx, page } = await fresh(); await chat(page, 'shop');
  await page.tap('#v-canvas'); await page.waitForSelector('#cv-listview button'); await snap(page, 'phone-canvas-list');
  await page.tap('#cv-listview button >> nth=0'); await page.waitForSelector('#cv-review:not([hidden]) img');
  await page.waitForFunction(() => [...document.querySelectorAll('#cv-review img')].some(i => i.complete && i.naturalWidth));
  await snap(page, 'phone-canvas-shot'); await ctx.close();
}
// 12. The ••• menu
{ const { ctx, page } = await fresh(); await home(page); await page.tap('#menu-btn'); await page.waitForSelector('#dict-mac'); await snap(page, 'phone-menu'); await ctx.close(); }
// 13. Pairing
{
  await ctl({ reset: true, paired: false, pair: ['pending', 'pending', 'pending', 'pending', 'pending', 'pending'] });
  const { ctx, page } = await phone(390, 844);
  await page.goto(`${base}/#pair=CODE1234abcd`); await page.waitForSelector('#pair-form:not([hidden])');
  await snap(page, 'phone-pairing'); await ctx.close();
}
console.log(made.join('\n'));
await br.close();
await sb.close();
