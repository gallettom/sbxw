// Load the real web UI from a running daemon in headless Chromium: no page
// error, the sandbox listed, a pane attached to it, the usage chip filled —
// then a screenshot. Usage: node ui-smoke.mjs <port> <sandbox> <png>
// (PLAYWRIGHT_MODULE points at a playwright install when it is not resolvable,
// CHROMIUM_PATH at a browser when Playwright has none of its own.)
const [port, sandbox, png] = process.argv.slice(2);
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const browser = await chromium.launch(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {});
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
// The 409 a voice approval of a high-risk proposal gets is expected (see below).
const expected = new Set();
page.on('response', (r) => { if (r.url().includes('/api/') && r.status() >= 400 && !expected.has(`${r.status()} ${new URL(r.url()).pathname}`)) problems.push(`HTTP ${r.status()} ${r.url()}`); });
// Jarvis's endpoints, stubbed: the smoke has no `sbxw-jarvis` sandbox to
// propose anything, and nothing may be created by loading the page. The stub
// keeps what was posted, so the checks below can say what reached the daemon.
const jv = { status: { state: 'absent' }, proposals: [], posts: [] };
await page.route('**/api/jarvis/status', (route) => route.fulfill({ json: jv.status }));
await page.route('**/api/jarvis/start', (route) => { jv.posts.push({ url: 'start' }); jv.status = { state: 'creating' }; route.fulfill({ json: { ok: true, state: 'creating' } }); });
await page.route('**/api/jarvis', (route) => route.fulfill({ json: { proposals: jv.proposals } }));
await page.route('**/api/jarvis/proposals/*/*', (route) => {
  const [, id, act] = new URL(route.request().url()).pathname.match(/proposals\/([^/]+)\/(\w+)$/);
  const body = JSON.parse(route.request().postData() || '{}');
  jv.posts.push({ id, act, ...body });
  const p = jv.proposals.find(x => x.id === id);
  if (!p) return route.fulfill({ status: 404, json: { ok: false, error: 'no such proposal' } });
  if (act === 'approve' && body.via === 'voice' && p.risk === 'high') {
    return route.fulfill({ status: 409, json: { ok: false, error: 'a high-risk proposal is approved on screen' } });
  }
  if (act === 'approve' && p.needs_ack && body.ack_host_command !== true) {
    return route.fulfill({ status: 409, json: { ok: false, error: 'acknowledge the host command first' } });
  }
  p.state = act === 'approve' ? 'applied' : 'refused';
  if (act === 'approve') p.results = p.changes.map(() => ({ ok: true, message: 'written', restart: false }));
  route.fulfill({ json: { ok: true, proposal: p } });
});
expected.add('409 /api/jarvis/proposals/smoke-high/approve');
const signIn = process.env.SBXW_UI_TOKEN ? `?t=${encodeURIComponent(process.env.SBXW_UI_TOKEN)}` : '';
await page.goto(`http://127.0.0.1:${port}/${signIn}#sandbox=${sandbox}`, { waitUntil: 'domcontentloaded' });
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
// The voice concierge: nothing installed in the throwaway home, so its panel
// says which command installs the models and offers the button — and the
// talk button stays off.
await visit('Jarvis (voice setup)', () => page.locator('#vc-launcher').click(), async () => {
  await page.screenshot({ path: png.replace(/\.png$/, '-concierge.png') });
  const vc = await page.evaluate(() => ({
    setup: document.getElementById('vc-setup').innerText,
    talk: document.getElementById('vc-talk').disabled,
  }));
  if (!vc.setup.includes('sbxw voice install') || !/Install voice/.test(vc.setup) || !vc.talk) {
    problems.push(`concierge: ${JSON.stringify(vc)}, want the install command, the install button and a disabled talk button`);
  }
});
// Jarvis as a destination: first in the bar's menu, its state beside it;
// picking it while absent offers to start it, and starts nothing.
await visit('Jarvis in the picker', () => page.locator('#vc-l-target').click(), async () => {
  const rows = await page.locator('.vc-target-menu button').allInnerTexts();
  if (!/^✓?\s*Jarvis\s*not created yet/.test(rows[0] || '')) problems.push(`picker: first row ${JSON.stringify(rows[0])}, want Jarvis, not created yet`);
  await page.screenshot({ path: png.replace(/\.png$/, '-jarvis-picker.png') });
  await page.locator('.vc-target-menu button[data-name="sbxw-jarvis"]').click();
  await page.waitForTimeout(600);
  const offer = await page.evaluate(() => ({
    bar: document.getElementById('vc-l-name').textContent,
    start: !!document.querySelector('#vc-log .vc-jarvis-go:not([hidden])'),
    open: !document.getElementById('vc-panel').hidden,
  }));
  if (offer.bar !== 'Jarvis' || !offer.start || !offer.open) problems.push(`jarvis offer: ${JSON.stringify(offer)}, want Jarvis in the bar and a Start button in the open panel`);
  if (jv.posts.some(p => p.url === 'start')) problems.push('jarvis: started without a click on Start');
  await page.screenshot({ path: png.replace(/\.png$/, '-jarvis-start.png') });
  await page.evaluate(() => vcSetOpen(false));
});
// A low-risk proposal: shown beside the page with its risk and its diff as
// text (markup in it stays text), and approved by a click.
await visit('Jarvis: low risk, approved by click', async () => {
  const low = { id: 'smoke-low', summary: 'Add the k8s kit to the default kits', risk: 'low', state: 'pending', created: 1794045600000,
    changes: [{ kind: 'kits', title: 'sbxw.toml: kits', diff: '-kits = ["sbxw:codemap"]\n+kits = ["sbxw:codemap", "sbxw:k8s-tools"]\n <img src=x onerror="window.jvPwned=1">' }] };
  jv.proposals.push(low);
  await page.evaluate((p) => jvIngest(structuredClone(p)), low);
  await page.waitForTimeout(300);
}, async () => {
  const shown = await page.evaluate(() => ({
    open: document.getElementById('jv-dialog').open,
    modal: document.getElementById('jv-dialog').matches(':modal'),
    risk: document.getElementById('jv-risk').textContent,
    diff: document.querySelector('#jv-body .jv-diff')?.textContent || '',
    img: !!document.querySelector('#jv-body img'),
    badge: document.getElementById('jv-badge').textContent,
  }));
  if (!shown.open || shown.modal || shown.risk !== 'Low risk' || !shown.diff.includes('<img src=x') || shown.img || !/1 to review/.test(shown.badge)) {
    problems.push(`jarvis low: ${JSON.stringify(shown)}, want a non-modal dialog, Low risk, the diff as text, the badge`);
  }
  await page.screenshot({ path: png.replace(/\.png$/, '-jarvis-low.png') });
  await page.locator('#jv-foot [data-jv-action="approve"]').click();
  await page.waitForTimeout(500);
  const after = await page.evaluate(() => ({
    outcome: document.querySelector('#jv-body .jv-outcome')?.textContent || '',
    result: document.querySelector('#jv-body .jv-result')?.textContent || '',
  }));
  const post = jv.posts.find(p => p.id === 'smoke-low');
  if (!post || post.act !== 'approve' || post.via !== 'click' || after.outcome !== 'Applied.' || after.result !== 'Applied') {
    problems.push(`jarvis low approve: posted ${JSON.stringify(post)}, shown ${JSON.stringify(after)}, want via click and Applied`);
  }
  await page.evaluate(() => jvClose());
});
// A high-risk proposal: a spoken yes approves nothing and says why; a voice
// approval that reaches the daemon anyway gets 409, said the same way.
await visit('Jarvis: high risk, voice approval refused', async () => {
  const high = { id: 'smoke-high', summary: 'Allow all network traffic', risk: 'high', state: 'pending', created: 1794045660000,
    changes: [{ kind: 'network', title: 'network_allow', diff: '+network_allow = ["**"]' }] };
  jv.proposals.push(high);
  await page.evaluate((p) => jvIngest(structuredClone(p)), high);
  await page.waitForTimeout(300);
}, async () => {
  const before = jv.posts.length;
  await page.evaluate(() => vcHear('oui'));
  await page.waitForTimeout(300);
  const spoken = jv.posts.slice(before);
  // As if the page still held it as low risk and the daemon knew better.
  await page.evaluate(async () => {
    const p = jvProposals.get('smoke-high');
    p.risk = 'low';
    await jvApprove('smoke-high', 'voice');
    p.risk = 'high';
    jvRender();
  });
  await page.waitForTimeout(500);
  const forced = jv.posts.slice(before + spoken.length);
  const st = await page.evaluate(() => ({
    risk: document.getElementById('jv-risk').textContent,
    approve: !!document.querySelector('#jv-foot [data-jv-action="approve"]'),
    said: [...document.querySelectorAll('#vc-log .vc-msg')].filter(e => /approve it on screen/.test(e.textContent)).length,
    state: jvProposals.get('smoke-high')?.state,
  }));
  if (spoken.length || forced.length !== 1 || forced[0].via !== 'voice' || st.risk !== 'High risk' || !st.approve || st.said < 2 || st.state !== 'pending') {
    problems.push(`jarvis high: spoken yes posted ${JSON.stringify(spoken)}, voice approval ${JSON.stringify(forced)}, shown ${JSON.stringify(st)}; want nothing, one 409, the message twice, still pending`);
  }
  await page.screenshot({ path: png.replace(/\.png$/, '-jarvis-high.png') });
  await page.locator('#jv-foot [data-jv-action="refuse"]').click();
  await page.waitForTimeout(300);
  await page.evaluate(() => jvClose());
});
// A stdio MCP server runs a command on this machine: the dialog shows it in
// full, and Approve stays off until "I have read this command" is ticked; the
// acknowledgement is sent from that box only.
await visit('Jarvis: host command acknowledged', async () => {
  const host = { id: 'smoke-host', summary: 'Add a local MCP server', risk: 'high', state: 'pending', needs_ack: true, created: 1794045720000,
    changes: [{ kind: 'mcp_server', title: 'MCP server notes (stdio)', risk: 'high', host_command: 'npx -y notes-mcp --root ~/notes', diff: '+ sbx mcp add notes -- npx -y notes-mcp --root ~/notes' }] };
  jv.proposals.push(host);
  await page.evaluate((p) => jvIngest(structuredClone(p)), host);
  await page.waitForTimeout(300);
}, async () => {
  const approve = page.locator('#jv-foot [data-jv-action="approve"]');
  const before = await page.evaluate(() => ({
    banner: document.querySelector('#jv-body .jv-host-cmd')?.textContent || '',
    disabled: document.querySelector('#jv-foot [data-jv-action="approve"]')?.disabled,
    box: !!document.querySelector('#jv-foot [data-jv-action="ack"]'),
  }));
  await page.screenshot({ path: png.replace(/\.png$/, '-jarvis-host.png') });
  await page.locator('#jv-foot [data-jv-action="ack"]').check();
  const enabled = await approve.isEnabled();
  await approve.click();
  await page.waitForTimeout(500);
  const post = jv.posts.find(p => p.id === 'smoke-host');
  if (before.banner !== 'npx -y notes-mcp --root ~/notes' || before.disabled !== true || !before.box || !enabled || !post || post.ack_host_command !== true || post.via !== 'click') {
    problems.push(`jarvis host command: ${JSON.stringify(before)}, enabled after tick ${enabled}, posted ${JSON.stringify(post)}; want the command shown, Approve off until ticked, ack_host_command sent`);
  }
  await page.evaluate(() => jvClose());
});
// Typing to Jarvis needs no voice: with /api/voice saying voice was not
// compiled in, Enter in the panel's input masks the text, sends it once, and
// empties the input; a push that fails says why and keeps the text.
const chat = { posts: [], fail: false };
await visit('Jarvis: typed without voice', async () => {
  await page.route('**/api/voice', (route) => route.fulfill({ json: { compiled: false, ready: false, push_to_talk_key: 'F8' } }));
  await page.route('**/api/voice/anonymise', (route) => {
    const { text } = JSON.parse(route.request().postData() || '{}');
    const safe = text.replace(/\S+@\S+/g, '[e-mail]');
    route.fulfill({ json: { ok: true, text: safe, masked: safe === text ? 0 : 1 } });
  });
  await page.route('**/api/chat/push', (route) => {
    chat.posts.push(JSON.parse(route.request().postData() || '{}'));
    // 200 with ok: false, as postJsonOrError reads it: a 5xx would log a console error.
    if (chat.fail) return route.fulfill({ json: { ok: false, error: 'smoke: push refused' } });
    route.fulfill({ json: { ok: true } });
  });
  jv.status = { state: 'running' };
  await page.evaluate(async () => { vcDisarm(); vcSetOpen(true); await vcRefresh(); vcSetTarget('sbxw-jarvis'); await vcJarvisStatus(); });
  await page.waitForTimeout(300);
}, async () => {
  const input = page.locator('#vc-type-input');
  const noVoice = await page.evaluate(() => ({ ready: vcReady(), talk: document.getElementById('vc-talk').disabled }));
  await input.fill('change the voice, mail me at a.b@example.org');
  await input.press('Enter');
  await page.waitForTimeout(600);
  const sent = { posts: chat.posts.slice(), left: await input.inputValue() };
  await page.screenshot({ path: png.replace(/\.png$/, '-jarvis-typed.png') });
  chat.fail = true;
  await input.fill('change the voice again');
  await input.press('Enter');
  await page.waitForTimeout(600);
  const failed = await page.evaluate(() => ({
    left: document.getElementById('vc-type-input').value,
    err: [...document.querySelectorAll('#vc-log .vc-err')].some(e => /smoke: push refused/.test(e.textContent)),
  }));
  if (noVoice.ready || !noVoice.talk || sent.posts.length !== 1 || sent.posts[0].name !== 'sbxw-jarvis'
    || sent.posts[0].text !== 'change the voice, mail me at [e-mail]' || sent.left !== ''
    || chat.posts.length !== 2 || failed.left !== 'change the voice again' || !failed.err) {
    problems.push(`jarvis typed: voice ${JSON.stringify(noVoice)}, sent ${JSON.stringify(sent)}, after a failed push ${JSON.stringify(failed)} (${chat.posts.length} posts); want no voice, one masked push to sbxw-jarvis, the input emptied, then an error and the text kept`);
  }
  await page.unroute('**/api/voice');
  await page.unroute('**/api/voice/anonymise');
  await page.unroute('**/api/chat/push');
  jv.status = { state: 'absent' };
  await input.fill('');
  await page.evaluate(async () => { await vcRefresh(); vcSetOpen(false); });
});
// The panel is a plain conversation: with Jarvis picked, its answer is prose
// on the left of the log (no card, no inner scrollbar, nothing armed, no
// "Answer it"), markup in it stays text, and Follow is hidden. The footer is
// one row: the input and the talk glyph side by side, the talk button named.
// Another sandbox's message stays a card the next words answer.
await visit('Jarvis: a plain conversation', async () => {
  jv.status = { state: 'running' };
  await page.evaluate(async () => { vcReset(); vcSetOpen(true); vcSetTarget('sbxw-jarvis'); await vcJarvisStatus(); });
  await page.waitForTimeout(300);
}, async () => {
  const long = Array.from({ length: 40 }, (_, i) => `Line ${i + 1} of a **long** answer with \`code\`.`).join('\n\n');
  const got = await page.evaluate(async (long) => {
    await vcAgentMessage('sbxw-jarvis', null, 'Short answer. <img src=x onerror=alert(1)>');
    await vcAgentMessage('sbxw-jarvis', null, long);
    const rows = [...document.querySelectorAll('#vc-log .vc-jarvis')];
    const inner = rows.some(r => [r, ...r.querySelectorAll('*')].some(e => e.tagName !== 'PRE' && e.scrollHeight > e.clientHeight + 2 && /auto|scroll/.test(getComputedStyle(e).overflowY)));
    const input = document.getElementById('vc-type-input').getBoundingClientRect();
    const talk = document.getElementById('vc-talk').getBoundingClientRect();
    return {
      rows: rows.length,
      cards: document.querySelectorAll('#vc-log .vc-agent').length,
      img: !!document.querySelector('#vc-log img'),
      buttons: [...document.querySelectorAll('#vc-log button')].map(b => b.getAttribute('aria-label') || b.textContent),
      armed: !!vcPending,
      inner,
      left: rows.every(r => r.getBoundingClientRect().left < document.getElementById('vc-log').getBoundingClientRect().left + 20),
      follow: document.getElementById('vc-follow').hidden,
      hint: !!document.getElementById('vc-hint'),
      oneRow: Math.abs(input.top - talk.top) < 6 && input.width > talk.width * 4,
      talkName: document.getElementById('vc-talk').getAttribute('aria-label'),
      lines: [...document.querySelectorAll('#vc-log .vc-dim')].map(e => e.textContent),
    };
  }, long);
  await page.screenshot({ path: png.replace(/\.png$/, '-jarvis-conversation.png') });
  const other = await page.evaluate(async () => {
    vcSetTarget('smoke-other');
    await vcAgentMessage('smoke-other', null, 'A question from another agent?');
    const card = [...document.querySelectorAll('#vc-log .vc-agent')].pop();
    return {
      follow: document.getElementById('vc-follow').hidden,
      armed: vcPending?.kind,
      answer: !!card && [...card.querySelectorAll('button')].some(b => b.textContent === 'Answer it'),
      frame: card ? getComputedStyle(card).borderTopWidth : null,
    };
  });
  if (got.rows !== 2 || got.cards || got.img || got.buttons.some(b => b !== 'Read aloud') || got.armed || got.inner || !got.left
    || !got.follow || got.hint || !got.oneRow || !/^Hold to talk \(F8\)|Talking is off/.test(got.talkName || '') || got.lines.length
    || other.follow || other.armed !== 'reply' || !other.answer || other.frame !== '0px') {
    problems.push(`jarvis conversation: ${JSON.stringify(got)}, another sandbox ${JSON.stringify(other)}; want two plain answers on the left, markup as text, only Read aloud buttons, nothing armed, no inner scrollbar, Follow hidden, no hint line, a one-row footer; and for another sandbox Follow shown and an armed card with a left rule`);
  }
  jv.status = { state: 'absent' };
  await page.evaluate(() => { vcReset(); vcSetTarget('sbxw-jarvis'); vcSetOpen(false); });
});
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
await visit('Setup: Hosts', async () => { await button(/^Setup$/).click(); await page.locator('dialog[open] .setup-tab[data-tab="hosts"]').click(); }, async () => {
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
const tab = (re) => page.locator('dialog[open] .setup-tab').filter({ hasText: re });
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
