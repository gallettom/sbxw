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
// A reconnect (the pane's ↻) forces one full TUI repaint: a resize to one column
// narrower, then straight back to the real size — see nudgePaneRepaint.
const resizes = await page.evaluate(async () => {
  const sent = [];
  const send = WebSocket.prototype.send;
  WebSocket.prototype.send = function (d) {
    try { const m = JSON.parse(d); if (m.type === 'resize') sent.push(`${m.cols}x${m.rows}`); } catch (_) {}
    return send.call(this, d);
  };
  recreatePaneTerminal(0);
  await new Promise(r => setTimeout(r, 800));
  WebSocket.prototype.send = send;
  return { sent, now: `${panes[0].term.cols}x${panes[0].term.rows}`, tracked: panes[0].ws?.sentSize };
});
const [wide, narrow, last] = [resizes.now, resizes.sent.at(-2), resizes.sent.at(-1)];
const [c, r] = (wide || '0x0').split('x').map(Number);
if (narrow !== `${c - 1}x${r}` || last !== wide || resizes.tracked !== wide) {
  problems.push(`reconnect repaint: sent resizes [${resizes.sent}], want ...,${c - 1}x${r},${wide} and sentSize ${wide} (got ${resizes.tracked})`);
}
// A failed plan merge can be retried: with a faked plan in that state, the
// dialog keeps the error and offers Retry merge, and the pane's button leads there.
const retry = await page.evaluate((sbx) => {
  const id = 'smoke-failed-merge';
  plans.set(id, { id, sandbox: sbx, variant: 'split', state: 'approved', closed: false, created_ms: Date.now(),
    threads: [{ name: 'a', state: 'done' }, { name: 'b', state: 'done' }],
    merge: { state: 'failed', order: ['a', 'b'], note: '', results: [{ thread: 'a', ok: true, commit: 'abc1234' }, { thread: 'b', ok: false, error: 'conflict' }] } });
  refreshPlanButtons();
  const pane = document.getElementById('pmerge-0');
  const out = { pane: pane && !pane.hidden ? pane.textContent : null };
  openMergePlan(sbx);
  const all = document.getElementById('merge-all');
  out.retry = all.hidden ? null : `${all.textContent}${all.disabled ? ' (disabled)' : ''}`;
  out.error = document.getElementById('merge-outcome').textContent;
  closePlan();
  plans.delete(id);
  refreshPlanButtons();
  return out;
}, sandbox);
if (retry.pane !== 'Retry merge' || !/^Retry merge/.test(retry.retry || '') || !/conflict/.test(retry.error)) {
  problems.push(`failed merge: ${JSON.stringify(retry)}, want a Retry merge pane button and dialog button, error shown`);
}
// Remote hosts (ui-smoke.sh registers `fake`, which answers, and `gone`,
// which does not): the sidebar is one section per host, local first, and
// gone's sandbox is greyed with its actions off.
const sidebar = await page.evaluate(() => {
  const sections = [...document.querySelectorAll('#sandbox-list .host-section')].map(e => e.dataset.host);
  const gone = document.querySelector('.sbx-item[data-name="gone~old"]');
  return {
    sections,
    fake: !!document.querySelector('.host-section[data-host="fake"] .sbx-item[data-name="fake~api"]'),
    fakeLink: document.querySelector('.host-section[data-host="fake"] .host-link-text')?.textContent || '',
    gone: gone ? { greyed: gone.classList.contains('unreachable'), note: gone.querySelector('.sbx-status')?.textContent || '',
      enabled: [...gone.querySelectorAll('.sbx-actions button')].filter(b => !b.disabled).length } : null,
  };
});
if (sidebar.sections.join(',') !== 'local,fake,gone' || !sidebar.fake) {
  problems.push(`grouped sidebar: ${JSON.stringify(sidebar)}, want sections local,fake,gone with fake~api under fake`);
}
if (!sidebar.gone || !sidebar.gone.greyed || !/^unreachable since \d\d:\d\d/.test(sidebar.gone.note) || sidebar.gone.enabled) {
  problems.push(`unreachable sandbox: ${JSON.stringify(sidebar.gone)}, want greyed, "unreachable since HH:MM", no action enabled`);
}
// The host headers at the default sidebar width: the name is what truncates.
// fake's latency is shown whole, and so is gone's short state: "never", and
// "down HH:MM" put in a copy of its header to try the longest one. A header
// too narrow for its link text would drop it rather than clip it, the dot
// keeping it in its title.
const heads = await page.evaluate(() => {
  const rows = [...document.querySelectorAll('.host-head')];
  const gone = document.querySelector('.host-head[data-host-fold="gone"]');
  let probe = null;
  if (gone) {
    probe = gone.cloneNode(true);
    probe.dataset.hostFold = 'gone (down HH:MM)';
    probe.querySelector('.host-link-text').textContent = 'down 14:05';
    probe.classList.remove('tight');
    gone.after(probe);
    rows.push(probe);
  }
  const out = rows.map(h => {
  const t = h.querySelector('.host-link-text');
  return { host: h.dataset.hostFold, overflow: h.scrollWidth > h.clientWidth,
    shown: t && t.offsetParent !== null, clipped: t && t.scrollWidth > t.clientWidth,
    dotTitle: h.querySelector('.dot.host-link')?.title || '' };
  });
  probe?.remove();
  return out;
});
for (const h of heads) {
  if (h.overflow || (h.shown && h.clipped)) problems.push(`host header ${h.host} clipped: ${JSON.stringify(h)}`);
  if (!h.shown && !h.dotTitle) problems.push(`host header ${h.host} dropped its link text with no dot title`);
}
for (const host of ['fake', 'gone', 'gone (down HH:MM)']) {
  if (!heads.find(h => h.host === host)?.shown) problems.push(`${host}: link text not shown at the default sidebar width: ${JSON.stringify(heads)}`);
}
await page.screenshot({ path: png.replace(/\.png$/, '-hosts-sidebar.png') });
await visit('Fold a host', async () => {
  await page.locator('.host-head[data-host-fold="fake"]').click();
  await page.waitForTimeout(200);
  const folded = await page.evaluate(() => ({
    rows: document.querySelectorAll('.host-section[data-host="fake"] .sbx-item').length,
    kept: JSON.parse(localStorage.getItem('sbxw.hosts-folded') || '[]'),
  }));
  if (folded.rows || !folded.kept.includes('fake')) problems.push(`fold: ${JSON.stringify(folded)}`);
  await page.locator('.host-head[data-host-fold="fake"]').click();
});
await visit('Host menu: Test', async () => {
  await page.locator('[data-host-menu="fake"]').click();
  await page.locator('#host-menu [data-host-act="test"]').click();
  await page.waitForTimeout(2500);
}, async () => {
  const toast = await page.locator('#paste-toast').textContent();
  if (!/^fake: .* · \d+ ms$/.test(toast || '')) problems.push(`host test toast: ${JSON.stringify(toast)}`);
});
await visit('Host menu: Provision', async () => {
  await page.locator('[data-host-menu="fake"]').click();
  await page.locator('#host-menu [data-host-act="provision"]').click();
}, async () => {
  const cmd = await page.locator('#provision-cmd').textContent();
  if (!/ host provision fake$/.test(cmd || '')) problems.push(`provision command: ${JSON.stringify(cmd)}`);
  await page.screenshot({ path: png.replace(/\.png$/, '-provision.png') });
});
// Setup → Hosts: lists both, and adds a third, confirming its key first.
await visit('Setup: Hosts', async () => { await button(/^Setup$/).click(); await page.locator('.modal-overlay:not(.hidden) .setup-tab[data-tab="hosts"]').click(); }, async () => {
  const rows = await page.locator('#hosts-list .host-row .tpl-tag').allTextContents();
  if (rows.join(',') !== 'fake,gone') problems.push(`hosts tab lists [${rows}], want fake,gone`);
  await page.locator('#hosts-f-name').fill('newbox');
  await page.locator('#hosts-f-mode-host').check();
  await page.locator('#hosts-f-host').fill('newhost');
  await page.locator('#hosts-f-trust').check();
  await page.locator('#hosts-save').click();
  await page.waitForTimeout(1200);
  const fps = await page.locator('#hosts-key-fps').textContent();
  if (!/SHA256:/.test(fps || '') || await page.locator('#hosts-key').isHidden()) problems.push(`host key not shown: ${JSON.stringify(fps)}`);
  await page.screenshot({ path: png.replace(/\.png$/, '-hosts.png') });
  await page.locator('#hosts-key-trust').click();
  await page.waitForTimeout(1500);
  const after = await page.locator('#hosts-list .host-row .tpl-tag').allTextContents();
  const saved = await page.evaluate(async () => (await (await fetch('/api/remote-hosts')).json()).hosts.find(h => h.name === 'newbox'));
  if (after.join(',') !== 'fake,gone,newbox' || !saved || saved.host !== 'newhost' || !saved.trust_secrets) {
    problems.push(`add host: list [${after}], saved ${JSON.stringify(saved)}`);
  }
});
// ＋ on a remote host: the Host field, that host's folders, a git URL field.
await visit('Create on a remote host', async () => {
  await page.evaluate(() => { openModal(); });
  await page.waitForTimeout(500);
  await page.locator('#inp-host').selectOption('fake');
  await page.waitForTimeout(1500);
}, async () => {
  const c = await page.evaluate(() => ({
    hostShown: !document.getElementById('host-field').classList.contains('hidden'),
    git: !document.getElementById('git-url-row').classList.contains('hidden'),
    folders: [...document.querySelectorAll('#fs-list .fs-name')].map(e => e.textContent),
  }));
  if (!c.hostShown || !c.git || !c.folders.includes('srv')) problems.push(`create on fake: ${JSON.stringify(c)}`);
  await page.screenshot({ path: png.replace(/\.png$/, '-create-remote.png') });
  await page.locator('#inp-host').selectOption('local');
  await page.evaluate(() => { try { localStorage.removeItem('sbxw.create-host'); } catch (_) {} });
});
// Headless Chromium has been seen to crash at or after the Monitor step: say
// where things stand first, so a crash there does not hide what came before.
console.error(JSON.stringify({ before_monitor: { tour, problems } }, null, 2));
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
// Setup → Mac: on a host that cannot run terminal-mac it says so; with the
// fake (SBXW_TERMINAL_MAC) it shows the VM. Either way it must draw.
await visit('Setup: Mac', async () => { await button(/^Setup$/).click(); await tab(/^Mac$/).click(); }, async () => {
  const text = await page.locator('#mac-body').innerText();
  if (/Loading…|Could not read/.test(text)) problems.push('mac panel: ' + text.slice(0, 120));
  await page.screenshot({ path: png.replace(/\.png$/, '-mac.png') });
});
const terminal = await page.evaluate(() => [...document.querySelectorAll('.xterm-rows')].map((e) => e.innerText).join('\n'));
await page.screenshot({ path: png });
await browser.close();
console.log(JSON.stringify({ listed, terminal: terminal.slice(0, 300), tour, problems }, null, 2));
process.exit(listed && problems.length === 0 ? 0 : 1);
