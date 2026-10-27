// ── sbxw extension: the side panel's shell ────────────────────────────────
// A thin frame around the daemon's own page (`<daemon>/sidepanel`), which is
// where the chat lives: same-origin with the daemon, updated with sbxw, never
// with the extension. This shell does only what that page cannot do from
// inside an iframe:
//
//   * say which URL the tab beside the panel shows (`sbxw:tab`), and follow
//     it as the developer switches tabs or navigates;
//   * screenshot that tab (`sbxw:capture` → `sbxw:capture-result`);
//   * reload it (`sbxw:reload`);
//   * bring the sbxw web UI's tab forward (`sbxw:show-app`).
//
// Messages are exchanged with the daemon's origin only, and only with the
// frame this shell created.
//
// Seeing a tab takes a permission. `activeTab`, granted by clicking the sbxw
// icon on that tab, gives its URL and screenshots until it navigates to
// another origin. To keep following an app across reloads and sign-ins, the
// panel offers to allow that one site for good (`optional_host_permissions`),
// asked once per site, never "all sites".

const frame = document.getElementById('frame');
const banner = document.getElementById('banner');
const down = document.getElementById('down');

let daemon = SBXW_DAEMON_DEFAULT; // from defaults.js, written by `sbxw extension`
let daemonOrigin = null;
let frameReady = false;
let tab = null; // the active tab of this window: { id, windowId, url? }

// ── The daemon's page ─────────────────────────────────────────────────────
async function start() {
  try {
    const { daemon: saved } = await chrome.storage.local.get('daemon');
    if (saved) daemon = saved;
  } catch (_) {}
  daemonOrigin = new URL(daemon).origin;
  frameReady = false;
  // An opaque answer is enough to know sbxw is listening; an unreachable
  // daemon would otherwise be a blank iframe with no explanation.
  try {
    await fetch(`${daemonOrigin}/api/sessions`, { mode: 'no-cors', cache: 'no-store' });
  } catch (_) {
    showDown();
    return;
  }
  down.hidden = true;
  frame.hidden = false;
  frame.src = `${daemonOrigin}/sidepanel`;
}

function showDown() {
  frame.hidden = true;
  down.hidden = false;
  document.getElementById('down-addr').textContent = daemonOrigin;
}

function post(msg) {
  if (frameReady) frame.contentWindow.postMessage(msg, daemonOrigin);
}

window.addEventListener('message', e => {
  if (e.source !== frame.contentWindow || e.origin !== daemonOrigin) return;
  const m = e.data || {};
  if (m.type === 'sbxw:ready') {
    frameReady = true;
    sendTab();
  } else if (m.type === 'sbxw:capture') {
    capture(m.id);
  } else if (m.type === 'sbxw:reload') {
    if (tab) chrome.tabs.reload(tab.id);
  } else if (m.type === 'sbxw:show-app') {
    showApp();
  }
});

// ── The sbxw tab ──────────────────────────────────────────────────────────
// "Answer in sbxw" points the web UI at the sandbox over `/api/focus`, but a
// page cannot bring its own background tab forward (Chrome ignores its
// `window.focus()`): the extension does. Finding the tab by URL takes host
// permission on the daemon's origin; the manifest's loopback ones cover the
// address `sbxw extension` writes by default.
async function showApp() {
  let tabs = [];
  try { tabs = await chrome.tabs.query({ url: `${daemonOrigin}/*` }); } catch (_) {}
  const app = tabs.find(t => t.url && new URL(t.url).pathname !== '/sidepanel');
  if (!app) return;
  await chrome.tabs.update(app.id, { active: true });
  await chrome.windows.update(app.windowId, { focused: true });
}

// ── The tab beside the panel ──────────────────────────────────────────────
async function refreshTab() {
  const [t] = await chrome.tabs.query({ active: true, currentWindow: true });
  tab = t || null;
  sendTab();
  renderBanner();
}

// `blocked`: there is a tab, but no permission to read it yet — the banner
// above says how to give one.
function sendTab() {
  post({ type: 'sbxw:tab', url: tab?.url || null, blocked: !!tab && !tab.url });
}

function originOf(url) {
  try {
    const u = new URL(url);
    return /^https?:$/.test(u.protocol) ? u.origin : null;
  } catch (_) {
    return null;
  }
}

async function capture(id) {
  try {
    if (!tab) throw new Error('no tab to capture');
    const dataUrl = await chrome.tabs.captureVisibleTab(tab.windowId, { format: 'png' });
    post({ type: 'sbxw:capture-result', id, dataUrl });
  } catch (e) {
    const msg = String(e?.message || e);
    post({
      type: 'sbxw:capture-result',
      id,
      error: /activeTab|all_urls|permission/i.test(msg)
        ? 'click the sbxw icon in the toolbar on this tab first, then 📷 again'
        : msg,
    });
  }
}

// ── What the panel may see ────────────────────────────────────────────────
// One line above the chat, only when something is missing: the tab can't be
// read at all, or it can, but only until it leaves this origin.
async function renderBanner() {
  banner.replaceChildren();
  banner.hidden = true;
  if (!tab) return;
  if (!tab.url) {
    banner.append('The chat can’t see this tab yet. Click the sbxw icon in the toolbar (Alt+Shift+S) on the app’s tab.');
    banner.hidden = false;
    return;
  }
  const origin = originOf(tab.url);
  if (!origin) return;
  const pattern = `${origin}/*`;
  const [allowed, dismissed] = await Promise.all([
    chrome.permissions.contains({ origins: [pattern] }),
    chrome.storage.local.get('dismissed').then(r => (r.dismissed || []).includes(origin)).catch(() => false),
  ]);
  if (allowed || dismissed) return;
  const text = document.createElement('span');
  text.textContent = `Keep following ${new URL(origin).host} after reloads and sign-ins?`;
  const allow = document.createElement('button');
  allow.type = 'button';
  allow.textContent = 'Allow this site';
  allow.addEventListener('click', async () => {
    // Straight from the click: Chrome asks only inside a user gesture.
    try { await chrome.permissions.request({ origins: [pattern] }); } catch (_) {}
    refreshTab();
  });
  const later = document.createElement('button');
  later.type = 'button';
  later.className = 'quiet';
  later.textContent = '✕';
  later.title = 'Not now';
  later.setAttribute('aria-label', 'Not now');
  later.addEventListener('click', async () => {
    try {
      const { dismissed = [] } = await chrome.storage.local.get('dismissed');
      await chrome.storage.local.set({ dismissed: [...new Set([...dismissed, origin])].slice(-50) });
    } catch (_) {}
    renderBanner();
  });
  banner.append(text, allow, later);
  banner.hidden = false;
}

// ── Wiring ────────────────────────────────────────────────────────────────
chrome.tabs.onActivated.addListener(refreshTab);
chrome.tabs.onUpdated.addListener((id, change) => {
  if (tab && id === tab.id && (change.url || change.status === 'complete')) refreshTab();
});
chrome.permissions.onAdded.addListener(refreshTab);
chrome.permissions.onRemoved.addListener(refreshTab);
chrome.runtime.onMessage.addListener(m => {
  if (m?.type === 'sbxw:granted') refreshTab();
});
chrome.storage.onChanged.addListener(changes => {
  if (changes.daemon) start();
});
document.getElementById('retry').addEventListener('click', start);
document.getElementById('options').addEventListener('click', () => chrome.runtime.openOptionsPage());

// ── After an sbxw update ──────────────────────────────────────────────────
// install.sh and `sbxw update` rewrite this extension's folder. Chrome reads
// the panel's own files from it afresh each time the panel opens, but keeps the
// manifest and service worker it loaded until the extension reloads. When
// defaults.js names another version than the one running, reload it: that
// closes the panel, so say so first.
if (typeof SBXW_EXTENSION_VERSION === 'string'
    && SBXW_EXTENSION_VERSION !== chrome.runtime.getManifest().version) {
  banner.textContent = 'sbxw updated its extension: reloading it. Click the sbxw icon again.';
  banner.hidden = false;
  setTimeout(() => chrome.runtime.reload(), 1500);
} else {
  start();
  refreshTab();
}
