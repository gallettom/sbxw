// The Browser panel: the sandbox's own Chromium, live.
//
// One WebSocket to the daemon (/ws/browser), which runs `browser.mjs serve`
// in the sandbox and passes its lines on: pictures, the address, the hosts
// the network policy refused. Clicks, keys and navigation go back the same
// way. Nothing here trusts what comes out of the sandbox: a picture is only
// ever base64 in an <img>, an address and a host only ever text.
//
// Hiding the panel leaves the browser running for the agent. Closing the
// browser asks what to do with the hosts allowed from here, which were
// allowed for this sandbox alone.

const browserOverlay = document.getElementById('browser-overlay');
const browserNameEl = document.getElementById('browser-name');
const browserUrl = document.getElementById('browser-url');
const browserStage = document.getElementById('browser-stage');
const browserScreen = document.getElementById('browser-screen');
const browserStatus = document.getElementById('browser-status');
const browserBlocked = document.getElementById('browser-blocked');
const browserGrantNote = document.getElementById('browser-grant-note');
const browserSettleOverlay = document.getElementById('browser-settle-overlay');
const browserSettleList = document.getElementById('browser-settle-list');
const browserSettleName = document.getElementById('browser-settle-name');
const browserBox = browserOverlay.querySelector('.browser-modal');

let browserTarget = null;   // the sandbox the panel shows
let browserWs = null;
let browserFrame = { w: 1280, h: 800 };
let browserPageUrl = '';
let browserQuitting = false;
const browserRefused = new Map(); // host → kind, as the proxy said
const browserIgnored = new Set();
let browserAllowed = 0;

function browserSend(msg) {
  if (browserWs && browserWs.readyState === WebSocket.OPEN) browserWs.send(JSON.stringify(msg));
}

// An error is announced (role=alert); "Starting…" is not: a screen reader
// has no use for a progress line that the picture replaces a moment later.
function browserSay(msg, isError) {
  if (isError) browserStatus.setAttribute('role', 'alert'); else browserStatus.removeAttribute('role');
  browserStatus.textContent = msg || '';
  browserStatus.classList.toggle('hidden', !msg);
  browserStatus.classList.toggle('browser-status-error', !!isError);
}

// Something to tell the person about the panel. A toast lives outside the
// dialog, which makes the rest of the page inert: while the panel is the
// modal dialog the line goes in the panel itself, and is a toast only when
// the panel sits beside the terminal.
const browserNote = document.getElementById('browser-note');
let browserNoteTimer = null;
function browserTell(msg, kind) {
  if (!browserOverlay.open) { showToast(msg, kind); return; }
  browserNote.textContent = msg;
  browserNote.classList.toggle('browser-note-error', kind === 'error');
  browserNote.classList.remove('hidden');
  clearTimeout(browserNoteTimer);
  browserNoteTimer = setTimeout(() => { browserNote.classList.add('hidden'); browserNote.textContent = ''; }, 8000);
}

// Never under the person's own typing: a focused field follows the page
// until something is typed in it, and again once it is left.
let browserUrlTyped = false;
function browserShowUrl() {
  if (document.activeElement === browserUrl && browserUrlTyped) return;
  browserUrl.value = browserPageUrl;
  if (document.activeElement === browserUrl) browserUrl.select();
}

// The page is laid out at the size the panel gives it, so a click lands where
// it was aimed, but never narrower than a desktop: beside a terminal the
// panel can be narrow, and many sites would switch to their phone layout
// there. A narrow panel shows the desktop page scaled down instead.
const BROWSER_MIN_LAYOUT_W = 1024;
function browserFit() {
  const r = browserStage.getBoundingClientRect();
  if (r.width < 50 || r.height < 50) return;
  const w = Math.max(BROWSER_MIN_LAYOUT_W, r.width);
  const h = r.height * (w / r.width);
  browserSend({ t: 'size', w: Math.round(Math.min(1600, w)), h: Math.round(Math.min(1600, Math.max(320, h))) });
}

// The row is rebuilt whole, which would drop the keyboard's place when the
// focused button is one of its own: focus goes back to the same host's
// button, else the next chip's, else the page.
function renderBrowserBlocked() {
  const held = browserBlocked.contains(document.activeElement) ? document.activeElement : null;
  const heldHost = held?.dataset.host;
  const heldClass = held?.classList.contains('browser-allow') ? '.browser-allow' : '.browser-ignore';
  renderBrowserBlockedRow();
  if (!held) return;
  const same = [...browserBlocked.querySelectorAll(heldClass)].find(b => b.dataset.host === heldHost);
  (same || browserBlocked.querySelector('button') || browserStage).focus();
}

function renderBrowserBlockedRow() {
  const hosts = [...browserRefused.keys()].filter(h => !browserIgnored.has(h));
  browserBlocked.classList.toggle('hidden', hosts.length === 0);
  if (!hosts.length) { browserBlocked.innerHTML = ''; return; }
  const chip = host => {
    const kind = browserRefused.get(host);
    const fixed = kind === 'org' ? 'organisation policy, cannot be allowed' : kind === 'rule' ? 'a deny rule, which an allow does not override' : '';
    return `<span class="browser-chip" data-host="${escHtml(host)}">` +
      `<code>${escHtml(host)}</code>` +
      (fixed
        ? `<span class="browser-chip-why">${fixed}</span>`
        : `<button class="browser-allow" data-host="${escHtml(host)}" aria-label="Allow ${escHtml(host)} for this sandbox only">Allow</button>`) +
      `<button class="browser-ignore" data-host="${escHtml(host)}" aria-label="Leave ${escHtml(host)} blocked">✕</button>` +
      `</span>`;
  };
  browserBlocked.innerHTML =
    `<span class="browser-blocked-label">Blocked by this sandbox's network policy</span>` + hosts.map(chip).join('');
}

function renderBrowserGrantNote() {
  browserGrantNote.textContent = browserAllowed
    ? `${browserAllowed} host${browserAllowed === 1 ? '' : 's'} allowed from here, for this sandbox only. Closing the browser asks whether to keep ${browserAllowed === 1 ? 'it' : 'them'}.`
    : 'Runs inside the sandbox, under its network policy.';
}

browserBlocked.addEventListener('click', async e => {
  const ignore = e.target.closest('.browser-ignore');
  if (ignore) { browserIgnored.add(ignore.dataset.host); renderBrowserBlocked(); return; }
  const allow = e.target.closest('.browser-allow');
  if (!allow || !browserTarget) return;
  const host = allow.dataset.host;
  // Busy, not `disabled`: a disabled button drops the keyboard's focus,
  // and the row is about to be rebuilt around it.
  if (allow.getAttribute('aria-disabled') === 'true') return;
  allow.setAttribute('aria-disabled', 'true');
  const data = await postJsonOrError(`/api/sandboxes/${encodeURIComponent(browserTarget)}/browser/allow`, { host });
  if (!data.ok) {
    allow.removeAttribute('aria-disabled');
    browserTell(data.error || `Could not allow ${host}`, 'error');
    return;
  }
  browserRefused.delete(host);
  browserAllowed += 1;
  renderBrowserBlocked();
  renderBrowserGrantNote();
  browserTell(`${host} allowed for ${browserTarget} only`);
  browserSend({ t: 'reload' });
});

function browserOnMessage(ev) {
  let m;
  try { m = JSON.parse(ev.data); } catch (_) { return; }
  switch (m.t) {
    case 'frame':
      if (typeof m.d !== 'string' || !/^[A-Za-z0-9+/=]+$/.test(m.d)) return;
      browserFrame = { w: Number(m.w) || browserFrame.w, h: Number(m.h) || browserFrame.h };
      // The picture's own size, so its box is known before it decodes.
      if (browserScreen.width !== browserFrame.w) browserScreen.width = browserFrame.w;
      if (browserScreen.height !== browserFrame.h) browserScreen.height = browserFrame.h;
      browserScreen.src = 'data:image/jpeg;base64,' + m.d;
      browserScreen.classList.remove('hidden');
      browserSay('');
      break;
    case 'url':
      browserPageUrl = String(m.url || '');
      browserShowUrl();
      break;
    case 'blocked': {
      const host = String(m.host || '');
      const kind = String(m.kind || '');
      // Said again every few seconds while the page retries: nothing new.
      if (!host || browserIgnored.has(host) || browserRefused.get(host) === kind) return;
      browserRefused.set(host, kind);
      renderBrowserBlocked();
      break;
    }
    case 'viewport': {
      const sel = document.getElementById('browser-viewport');
      const name = String(m.name || 'auto');
      if (![...sel.options].some(o => o.value === name)) {
        // A size the agent chose: shown as it is, until another is picked.
        sel.querySelectorAll('option[data-custom]').forEach(o => o.remove());
        const o = document.createElement('option');
        o.value = name; o.textContent = name; o.dataset.custom = '1';
        sel.append(o);
      }
      sel.value = name;
      break;
    }
    case 'ready':
      browserSay('');
      browserFit();
      break;
    case 'note':
      browserTell(String(m.msg || ''), 'error');
      break;
    case 'error':
      browserSay(String(m.msg || 'The browser could not start.'), true);
      break;
    case 'closed':
    case 'gone':
      browserEnded(m.t === 'closed');
      break;
  }
}

function openBrowserPanel(name) {
  if (browserWs) { try { browserWs.close(); } catch (_) { /* already gone */ } }
  browserTarget = name;
  browserQuitting = false;
  browserNameEl.textContent = name;
  browserPageUrl = '';
  browserUrl.value = '';
  browserRefused.clear();
  browserIgnored.clear();
  browserAllowed = 0;
  renderBrowserBlocked();
  renderBrowserGrantNote();
  browserScreen.classList.add('hidden');
  browserScreen.removeAttribute('src');
  browserSay('Starting the browser in the sandbox…');
  browserPlace(name);
  // What was allowed earlier and not settled yet still counts.
  fetch(`/api/sandboxes/${encodeURIComponent(name)}/browser/grants`).then(r => r.json()).then(d => {
    if (browserTarget === name && d.ok) { browserAllowed = (d.grants || []).length; renderBrowserGrantNote(); }
  }).catch(() => {});
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const ws = new WebSocket(`${proto}://${location.host}/ws/browser?sandbox=${encodeURIComponent(name)}`);
  browserWs = ws;
  ws.addEventListener('message', ev => { if (browserWs === ws) browserOnMessage(ev); });
  ws.addEventListener('close', () => { if (browserWs === ws) browserEnded(false); });
}

// The pipe ended. `closed` is Chromium itself going away (asked from here, or
// by the agent): that is when the grants are settled.
function browserEnded(browserClosed) {
  const name = browserTarget;
  const ws = browserWs;
  browserWs = null;
  if (ws) { try { ws.close(); } catch (_) { /* already gone */ } }
  if (!name) return;
  if (browserClosed || browserQuitting) {
    hideBrowserPanel();
    settleBrowserGrants(name);
  } else if (browserShowing() && browserStatus.classList.contains('hidden')) {
    browserSay('The connection to the sandbox browser ended. Close and open the panel to reconnect.', true);
  }
}

// Leave the browser running: the agent may be in the middle of something.
function hideBrowserPanel() {
  // A dialog gives focus back to what opened it; the docked panel is
  // hidden by hand, so its focus is handed back by hand: to the tab that
  // shows it again, else to the terminal.
  const name = browserTarget;
  const hadFocus = !browserOverlay.open && browserBox.contains(document.activeElement);
  browserUndock();
  if (browserOverlay.open) browserOverlay.close();
  if (hadFocus) {
    const pane = browserPaneOf(name);
    const tab = pane?.el.querySelector(':scope > .browser-tab');
    if (tab) tab.focus(); else (pane || panes[focusedPane])?.term?.focus();
  }
  const ws = browserWs;
  browserWs = null;
  browserTarget = null;
  if (ws) { try { ws.close(); } catch (_) { /* already gone */ } }
}

function closeBrowser() {
  if (!browserTarget) return;
  const name = browserTarget;
  if (!browserWs || browserWs.readyState !== WebSocket.OPEN) {
    hideBrowserPanel();
    settleBrowserGrants(name);
    return;
  }
  browserQuitting = true;
  browserSend({ t: 'quit' });
  // Chromium says `closed` on its way out; do not wait on a hung one.
  setTimeout(() => { if (browserQuitting && browserTarget === name) browserEnded(true); }, 2500);
}

async function settleBrowserGrants(name) {
  browserQuitting = false;
  let grants = [];
  try {
    const d = await (await fetch(`/api/sandboxes/${encodeURIComponent(name)}/browser/grants`)).json();
    grants = d.grants || [];
  } catch (_) { return; }
  // Nothing was allowed: nothing to ask, the browser just closes.
  if (!grants.length) return;
  browserSettleName.textContent = name;
  browserSettleList.innerHTML = grants.map(g => `<li><code>${escHtml(g.host)}</code></li>`).join('');
  browserSettleOverlay.dataset.sandbox = name;
  // Opens on "Keep them" (autofocus in the markup): the answer that removes
  // nothing is the one a stray Enter gives.
  modalOpen(browserSettleOverlay);
}

async function answerBrowserSettle(keep) {
  const name = browserSettleOverlay.dataset.sandbox;
  browserSettleOverlay.close();
  if (!name) return;
  const data = await postJsonOrError(`/api/sandboxes/${encodeURIComponent(name)}/browser/settle`, { keep });
  if (!data.ok) { showToast(data.error || 'Could not settle the network rules', 'error'); return; }
  const s = data.settled || {};
  const failed = s.failed || [];
  if (failed.length) {
    showToast(`Not removed: ${failed.map(f => `${f.host} (${f.why})`).join('; ')}`, 'error');
  } else if (keep) {
    showToast(`${(s.kept || []).length} network rule${(s.kept || []).length === 1 ? '' : 's'} kept for ${name}`);
  } else {
    showToast(`${(s.removed || []).length} network rule${(s.removed || []).length === 1 ? '' : 's'} removed from ${name}`);
  }
}

// Escape leaves the question unanswered: the hosts stay until the next close.
wireModal(browserSettleOverlay, () => browserSettleOverlay.close());
// Escape belongs to the page in the sandbox browser (browserStage forwards it
// and prevents it), so the panel closes by its own buttons only:
// `closedby="none"` in the markup, and `cancel` prevented here for a browser
// without `closedby`. A second Escape with no click between cannot be
// prevented everywhere; if the dialog closes that way, the panel's own
// clean-up still runs, or its socket would keep feeding a closed panel.
browserOverlay.addEventListener('cancel', e => e.preventDefault());
browserOverlay.addEventListener('close', () => {
  if (browserTarget && !browserShowing()) hideBrowserPanel();
});
document.getElementById('browser-settle-keep').addEventListener('click', () => answerBrowserSettle(true));
document.getElementById('browser-settle-remove').addEventListener('click', () => answerBrowserSettle(false));
document.getElementById('browser-hide').addEventListener('click', hideBrowserPanel);
document.getElementById('browser-close').addEventListener('click', closeBrowser);
document.getElementById('browser-back').addEventListener('click', () => browserSend({ t: 'back' }));
document.getElementById('browser-forward').addEventListener('click', () => browserSend({ t: 'forward' }));
document.getElementById('browser-reload').addEventListener('click', () => browserSend({ t: 'reload' }));

browserUrl.addEventListener('keydown', e => {
  if (isEnterKey(e)) {
    e.preventDefault();
    browserSend({ t: 'nav', url: browserUrl.value });
    browserUrl.blur();
    browserStage.focus();
  } else if (e.key === 'Escape') {
    e.preventDefault(); // leave the field for the page, do not close the panel
    browserStage.focus();
  }
});
document.getElementById('browser-viewport').addEventListener('change', e => {
  // Focus stays on the select: arrowing through it fires `change` at each
  // step, and a control that moves focus by itself is a surprise.
  browserSend({ t: 'viewport', value: e.target.value });
});
browserUrl.addEventListener('focus', () => browserUrl.select());
browserUrl.addEventListener('input', () => { browserUrlTyped = true; });
browserUrl.addEventListener('blur', () => { browserUrlTyped = false; browserShowUrl(); });

// ── The person's hands ──────────────────────────────────────────────────────

function browserPoint(e) {
  const r = browserScreen.getBoundingClientRect();
  return {
    x: Math.round((e.clientX - r.left) * (browserFrame.w / Math.max(1, r.width))),
    y: Math.round((e.clientY - r.top) * (browserFrame.h / Math.max(1, r.height))),
  };
}
const browserMods = e => (e.altKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.metaKey ? 4 : 0) | (e.shiftKey ? 8 : 0);
const BROWSER_BUTTONS = ['left', 'middle', 'right'];

browserScreen.addEventListener('mousedown', e => {
  e.preventDefault();
  browserStage.focus();
  browserSend({ t: 'mouse', type: 'mousePressed', ...browserPoint(e), button: BROWSER_BUTTONS[e.button] || 'left', clickCount: e.detail || 1, modifiers: browserMods(e) });
});
browserScreen.addEventListener('mouseup', e => {
  e.preventDefault();
  browserSend({ t: 'mouse', type: 'mouseReleased', ...browserPoint(e), button: BROWSER_BUTTONS[e.button] || 'left', clickCount: e.detail || 1, modifiers: browserMods(e) });
});
let browserMoveAt = 0;
browserScreen.addEventListener('mousemove', e => {
  const now = Date.now();
  if (now - browserMoveAt < 40) return;
  browserMoveAt = now;
  browserSend({ t: 'mouse', type: 'mouseMoved', ...browserPoint(e), button: e.buttons & 1 ? 'left' : 'none', modifiers: browserMods(e) });
});
browserScreen.addEventListener('wheel', e => {
  e.preventDefault();
  browserSend({ t: 'mouse', type: 'mouseWheel', ...browserPoint(e), deltaX: e.deltaX, deltaY: e.deltaY, modifiers: browserMods(e) });
}, { passive: false });
browserScreen.addEventListener('contextmenu', e => e.preventDefault());
browserScreen.addEventListener('dragstart', e => e.preventDefault());

// Every key goes to the page, Tab and Escape included, so the stage would
// hold the keyboard for good. One key is kept back: F6, which browsers use
// to move between a page and its address bar, moves to this panel's.
// Zoom (Ctrl or Cmd with + - 0) stays the person's own browser's.
const BROWSER_KEY_TEXT = { Enter: '\r', Tab: '\t' };
const browserKeptKey = e => e.key === 'F6' || ((e.ctrlKey || e.metaKey) && ['+', '-', '=', '0'].includes(e.key));
browserStage.addEventListener('keydown', e => {
  if (e.key === 'F6') { e.preventDefault(); browserUrl.focus(); return; }
  if (browserKeptKey(e)) return;
  // Paste arrives as its own event, with the text.
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'v') return;
  e.preventDefault();
  const plain = !e.ctrlKey && !e.metaKey && !e.altKey;
  const text = e.key.length === 1 && plain ? e.key : (plain ? BROWSER_KEY_TEXT[e.key] : undefined);
  browserSend({ t: 'key', type: text ? 'keyDown' : 'rawKeyDown', key: e.key, code: e.code, text, vk: e.keyCode, modifiers: browserMods(e) });
});
browserStage.addEventListener('keyup', e => {
  if (browserKeptKey(e)) return;
  e.preventDefault();
  browserSend({ t: 'key', type: 'keyUp', key: e.key, code: e.code, vk: e.keyCode, modifiers: browserMods(e) });
});
browserStage.addEventListener('paste', e => {
  e.preventDefault();
  const text = (e.clipboardData || window.clipboardData).getData('text');
  if (text) browserSend({ t: 'paste', text });
});

// Refit whenever the stage changes size: the window, or the blocked-hosts
// row appearing above it.
let browserFitTimer = null;
new ResizeObserver(() => {
  if (!browserTarget) return;
  clearTimeout(browserFitTimer);
  browserFitTimer = setTimeout(browserFit, 200);
}).observe(browserStage);


// ── Beside the terminal, or over everything ────────────────────────────────
// The panel opens beside the pane showing that sandbox, on the side away
// from it (a terminal on the left of the screen gets the browser on its
// right), and pushes the panes over rather than covering them, so the agent's
// terminal stays in view. Without room for both, it is the dialog over
// everything, as before. The same element moves between the two.
const BROWSER_DOCK_MIN = 560;   // narrower and many sites switch to their phone layout
const BROWSER_DOCK_MAX = 960;
const BROWSER_TERM_MIN = 480;   // what the panes must keep beside it
const browserMain = document.getElementById('main-area');
const browserDock = document.createElement('aside');
browserDock.className = 'browser-dock hidden';
browserDock.setAttribute('aria-labelledby', 'browser-title');

// Move a connected node without resetting it (focus, the picture, the
// select's state) where the browser can; insert it as before where not.
function browserMoveInto(parent, node, before = null) {
  if ('moveBefore' in Element.prototype && node.isConnected && parent.isConnected) parent.moveBefore(node, before);
  else parent.insertBefore(node, before);
}

function browserShowing() {
  return browserOverlay.open || !browserDock.classList.contains('hidden');
}

// The pane on screen showing `name`: the focused one if it does.
function browserPaneOf(name) {
  const shown = panes.map((p, i) => ({ p, i })).filter(({ p, i }) => i < paneCount && p.sandbox === name);
  return (shown.find(({ i }) => i === focusedPane) || shown[0] || {}).p || null;
}

// Which side of the panes the browser goes, for a pane: away from it.
function browserSideOf(pane) {
  if (!pane) return 'right';
  const m = browserMain.getBoundingClientRect();
  const r = pane.el.getBoundingClientRect();
  return (r.left + r.width / 2) > (m.left + m.width / 2) ? 'left' : 'right';
}

function browserDockWidth(total) {
  const w = Math.round(Math.min(BROWSER_DOCK_MAX, Math.max(BROWSER_DOCK_MIN, total * 0.45)));
  return total - w >= BROWSER_TERM_MIN ? w : 0;
}

function browserPlace(name) {
  const pane = browserPaneOf(name);
  // What the panes have, the dock's own width included when it is open.
  const total = browserMain.getBoundingClientRect().width +
    (browserDock.classList.contains('hidden') ? 0 : browserDock.getBoundingClientRect().width);
  const width = pane ? browserDockWidth(total) : 0;
  if (!width) {
    browserUndock();
    modalOpen(browserOverlay);
    return;
  }
  const side = browserSideOf(pane);
  // The dock goes in the page first, and only when it is not already where
  // it belongs: a resize re-places the panel, and re-inserting it would
  // drop the focus from the address field or the page.
  const before = side === 'left' ? browserMain : browserMain.nextSibling;
  const placed = browserDock.parentNode === browserMain.parentNode && browserDock.dataset.side === side &&
    (side === 'left' ? browserDock.nextSibling === browserMain : browserMain.nextSibling === browserDock);
  browserDock.style.width = `${width}px`;
  browserDock.dataset.side = side;
  if (!placed) browserMoveInto(browserMain.parentNode, browserDock, before === browserDock ? browserDock.nextSibling : before);
  browserDock.classList.remove('hidden');
  if (browserBox.parentNode !== browserDock) browserMoveInto(browserDock, browserBox);
  browserBox.classList.add('docked');
  if (browserOverlay.open) browserOverlay.close();
  renderBrowserTabs();
}

function browserUndock() {
  if (browserBox.parentNode === browserDock) {
    browserBox.classList.remove('docked');
    browserMoveInto(browserOverlay, browserBox);
  }
  browserDock.classList.add('hidden');
  renderBrowserTabs();
}

// A window made narrow while the panel is beside the terminal: it becomes
// the dialog, and goes back beside it when there is room again.
let browserPlaceTimer = null;
window.addEventListener('resize', () => {
  if (!browserTarget || !browserShowing()) return;
  clearTimeout(browserPlaceTimer);
  browserPlaceTimer = setTimeout(() => { if (browserTarget && browserShowing()) browserPlace(browserTarget); }, 200);
});

// ── The tab on a terminal ──────────────────────────────────────────────────
// While a sandbox's agent uses its browser, the pane showing that sandbox
// carries a tab on its edge, on the side the panel would open. One click
// shows the browser; another hides it again.
const browserActive = new Set();

function renderBrowserTabs() {
  panes.forEach((p, i) => {
    let tab = p.el.querySelector(':scope > .browser-tab');
    const want = i < paneCount && p.sandbox && browserActive.has(p.sandbox);
    if (!want) { if (tab) tab.remove(); return; }
    if (!tab) {
      tab = document.createElement('button');
      tab.type = 'button';
      tab.className = 'browser-tab';
      tab.innerHTML = '<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" aria-hidden="true"><circle cx="8" cy="8" r="6.3"/><path d="M1.7 8h12.6M8 1.7c2.2 2 2.2 10.600 0 12.6M8 1.7c-2.2 2-2.2 10.600 0 12.6"/></svg>';
      p.el.append(tab);
    }
    const open = browserTarget === p.sandbox && browserShowing();
    tab.dataset.sandbox = p.sandbox;
    tab.dataset.side = open && !browserDock.classList.contains('hidden') ? browserDock.dataset.side : browserSideOf(p);
    tab.classList.toggle('open', open);
    // One name whatever the state; the state is `aria-expanded`.
    tab.setAttribute('aria-label', `Browser of ${p.sandbox}: its agent is using it`);
    tab.setAttribute('aria-expanded', String(open));
    tab.title = open ? 'Hide the browser — it keeps running' : 'Watch what the agent does in its browser';
  });
}

document.addEventListener('click', e => {
  const tab = e.target.closest('.browser-tab');
  if (!tab) return;
  e.preventDefault();
  e.stopPropagation();
  const name = tab.dataset.sandbox;
  if (browserTarget === name && browserShowing()) hideBrowserPanel();
  else openBrowserPanel(name);
});

function browserSetActive(name, active) {
  if (active) browserActive.add(name); else browserActive.delete(name);
  renderBrowserTabs();
}

fetch('/api/browser/active').then(r => r.json()).then(d => {
  (d.sandboxes || []).forEach(n => browserActive.add(n));
  renderBrowserTabs();
}).catch(() => {});
// main.js opens the stream, and loads after this file.
(function listenBrowserEvents() {
  if (typeof sbxwStream === 'undefined') { setTimeout(listenBrowserEvents, 200); return; }
  sbxwStream.addEventListener('browser', ev => {
    let d;
    try { d = JSON.parse(ev.data); } catch (_) { return; }
    if (d && typeof d.sandbox === 'string') browserSetActive(d.sandbox, !!d.active);
  });
})();
// A pane connected to another sandbox, or the layout changed: the tabs
// follow. Cheap, and only while some agent is browsing.
setInterval(() => { if (browserActive.size) renderBrowserTabs(); }, 1500);
