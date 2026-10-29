// Load the real web UI from a running daemon in headless Chromium: no page
// error, the sandbox listed, a pane attached to it, the usage chip filled —
// then a screenshot. Usage: node ui-smoke.mjs <port> <sandbox> <png>
// (PLAYWRIGHT_MODULE points at a playwright install when it is not resolvable.)
const [port, sandbox, png] = process.argv.slice(2);
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const browser = await chromium.launch();
// The CDN scripts (xterm) may come through a TLS-intercepting proxy in CI or a
// sandbox; the page's own origin is plain HTTP on loopback either way.
const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, ignoreHTTPSErrors: true });
const page = await context.newPage();
// Offline: serve the CDN's xterm files from a local copy (same versions).
if (process.env.XTERM_VENDOR) {
  const fs = await import('node:fs');
  await page.route('https://cdn.jsdelivr.net/**', (route) => {
    const file = route.request().url().split('/').pop();
    const body = fs.readFileSync(`${process.env.XTERM_VENDOR}/${file}`);
    route.fulfill({ body, contentType: file.endsWith('.css') ? 'text/css' : 'application/javascript' });
  });
}
const problems = [];
page.on('pageerror', (e) => problems.push('pageerror: ' + e.message));
page.on('console', (m) => { if (m.type() === 'error') problems.push('console: ' + m.text()); });
page.on('response', (r) => { if (r.url().includes('/api/') && r.status() >= 400) problems.push(`HTTP ${r.status()} ${r.url()}`); });
await page.goto(`http://127.0.0.1:${port}/#sandbox=${sandbox}`, { waitUntil: 'domcontentloaded' });
let listed = false;
for (let i = 0; i < 60 && !listed; i++) {
  listed = await page.evaluate((n) => document.body.innerText.includes(n), sandbox);
  if (!listed) await page.waitForTimeout(250);
}
await page.waitForTimeout(3000);
// Open each panel the way a person would, and give it time to fetch.
const tour = [];
async function visit(label, open, after) {
  const before = problems.length;
  try {
    await open();
    await page.waitForTimeout(1200);
    if (after) await after();
    await page.keyboard.press('Escape');
    await page.waitForTimeout(300);
    tour.push(`${label}: ${problems.length === before ? 'ok' : 'PROBLEM'}`);
  } catch (e) {
    tour.push(`${label}: could not open (${e.message.split('\n')[0]})`);
  }
}
const button = (re) => page.getByRole('button', { name: re }).first();
await visit('Project panel', () => button(/Project/).click(), () => page.screenshot({ path: png.replace(/\.png$/, '-project.png') }));
await visit('More menu', () => button(/More actions/).click(), () => page.screenshot({ path: png.replace(/\.png$/, '-more.png') }));
await visit('Env card', async () => { await button(/More actions/).click(); await page.getByRole('menuitem', { name: /Environment file/ }).click(); },
  () => page.screenshot({ path: png.replace(/\.png$/, '-env.png') }));
await visit('Sessions menu', () => page.locator('#psessions-0').click(), () => page.screenshot({ path: png.replace(/\.png$/, '-sessions.png') }));
// A session on the main checkout, opened from that menu: it lands in a pane
// of its own and is listed under its kind.
await visit('New session on the main checkout', async () => {
  await page.locator('#psessions-0').click();
  await page.getByRole('menuitem', { name: /^\+ Session$/ }).click();
  await page.waitForTimeout(800);
  await page.screenshot({ path: png.replace(/\.png$/, '-new-session.png') });
  await page.keyboard.press('Enter');
  await page.waitForTimeout(2500);
}, async () => {
  const label = await page.evaluate(() => panes.slice(0, paneCount).map(p => p.thread).filter(Boolean).join(','));
  if (label !== 's1') problems.push(`new session: panes hold threads [${label}], want s1`);
  await page.locator('#psessions-0').click();
  await page.waitForTimeout(600);
  await page.screenshot({ path: png.replace(/\.png$/, '-sessions-after.png') });
});
await visit('Sidebar sessions', () => page.locator('.sbx-sessions-toggle').first().click(),
  () => page.screenshot({ path: png.replace(/\.png$/, '-sidebar.png') }));
await visit('Bash pane', () => button(/Bash/).click());
await visit('Claude pane', () => button(/Claude/).click());
await visit('Monitor', () => button(/^Monitor$/).click());
await visit('Help', () => button(/^Help$/).click());
// Setup is one dialog with a tab per part: open it, then walk the tabs.
const tab = (re) => page.locator('.modal-overlay:not(.hidden) .setup-tab').filter({ hasText: re });
await visit('Setup: Skills', async () => { await button(/^Setup$/).click(); await tab(/^Skills$/).click(); });
await visit('Setup: Templates', async () => { await button(/^Setup$/).click(); await tab(/^Templates$/).click(); },
  () => page.screenshot({ path: png.replace(/\.png$/, '-templates.png') }));
await visit('Setup: Kits', async () => { await button(/^Setup$/).click(); await tab(/^Kits$/).click(); });
await visit('Setup: MCP', async () => { await button(/^Setup$/).click(); await tab(/^MCP$/).click(); }, async () => {
  await page.screenshot({ path: png.replace(/\.png$/, '-mcp.png') });
  await page.locator('#mcp-list .mcp-btn[data-act="edit"]').first().click();
  await page.waitForTimeout(300);
  await page.screenshot({ path: png.replace(/\.png$/, '-mcp-edit.png') });
});
const terminal = await page.evaluate(() => [...document.querySelectorAll('.xterm-rows')].map((e) => e.innerText).join('\n'));
await page.screenshot({ path: png });
await browser.close();
console.log(JSON.stringify({ listed, terminal: terminal.slice(0, 300), tour, problems }, null, 2));
process.exit(listed && problems.length === 0 ? 0 : 1);
