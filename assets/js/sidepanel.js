// ── Browser side panel ────────────────────────────────────────────────────
// The chat beside the app being tested. The sbxw extension opens Chrome's side
// panel next to the app's tab and frames this page in it (`/sidepanel`); the
// app stays in its own top-level tab, so HTTPS, its real hostname, its cookies
// and its SSO all keep working.
//
// The extension is a thin shell: it tells this page which URL its tab shows,
// takes screenshots and reloads the tab, all over `postMessage`. Everything
// else happens here, same-origin with the daemon:
//
//   * `GET /api/embed/resolve` turns the tab's URL into the sandbox serving it
//     (a hand-made pairing, else the published ports — see src/embed.rs);
//     with no single answer the developer picks one, and the pick becomes a
//     pairing (`POST /api/embed/pairings`);
//   * messages go to that sandbox's main agent, or one of its open threads,
//     through `POST /api/chat/push`, prefixed with the page's URL;
//   * `/api/events` (the island's feed, which does not count as a tab of the
//     web UI) says what the agent is doing, and the turn's reply;
//   * when that agent calls `ask_user_for_screenshot`, the panel answers the
//     request itself with a capture of the tab (see "Screenshots Claude asks
//     for" below), so the agent can look at its own change.
//
// The transcript is chat-core.js's, on the chat dock's styles. Opened outside
// the extension the page still works for a `?url=` given by hand, minus the
// screenshot and the reload.

const SP_STORE = 'sbxw-sidepanel';
const SP_AUTORELOAD = 'sbxw-sidepanel-autoreload';
const SP_AUTOSHOT = 'sbxw-sidepanel-autoshot';
const SP_CAPTURE_TIMEOUT_MS = 8000;
// Same numbers and rule as relay.js's `relayShrink`: a screenshot is read for
// its layout, and the relay refuses one past `MAX_SHOT_B64`.
const SP_SHOT_MAX_EDGE = 1600;
const SP_SHOT_MAX_B64 = 6 * 1024 * 1024;
// A request older than this was opened before anyone could have meant this
// tab; it is left to the popup in sbxw.
const SP_SHOT_FRESH_MS = 30 * 60 * 1000;
const spFramed = window.parent !== window;

// Line icons, drawn in the text colour so hover and disabled states follow it.
const SP_ICON = {
  camera: '<svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" aria-hidden="true"><path d="M1.75 5.25h2.5l1.5-2h4.5l1.5 2h2.5v7.5H1.75z"/><circle cx="8" cy="8.75" r="2.4"/></svg>',
  send: '<svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M8 13.25V2.75M3.5 7.25 8 2.75l4.5 4.5"/></svg>',
  reload: '<svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M13.25 8A5.25 5.25 0 1 1 11.7 4.3"/><path d="M13.25 2.25v3.5h-3.5"/></svg>',
  more: '<svg viewBox="0 0 16 16" width="16" height="16" fill="currentColor" aria-hidden="true"><circle cx="3.25" cy="8" r="1.35"/><circle cx="8" cy="8" r="1.35"/><circle cx="12.75" cy="8" r="1.35"/></svg>',
};

document.body.insertAdjacentHTML('afterbegin', `
<div class="sp-head" id="sp-head" hidden>
  <span class="sp-avatar" id="sp-avatar"><span class="cd-glyph" id="sp-glyph" aria-hidden="true"></span></span>
  <div class="sp-who">
    <div class="sp-row">
      <span class="sp-name" id="sp-name"></span>
      <select id="sp-target" aria-label="Who receives your messages" title="Who receives your messages"></select>
    </div>
    <span class="sp-page" id="sp-page"></span>
  </div>
  <button type="button" class="sp-icon" id="sp-reload" title="Reload the app's tab" aria-label="Reload the app's tab">${SP_ICON.reload}</button>
  <details class="sp-menu" id="sp-menu">
    <summary class="sp-icon" title="More" aria-label="More">${SP_ICON.more}</summary>
    <div class="sp-menu-body">
      <label class="sp-check"><input type="checkbox" id="sp-autoshot"><span>Let Claude take screenshots of this tab</span></label>
      <label class="sp-check"><input type="checkbox" id="sp-autoreload"><span>Reload the tab after each reply</span></label>
      <hr>
      <button type="button" id="sp-open">Open the full session in sbxw</button>
      <button type="button" id="sp-change">Change sandbox</button>
    </div>
  </details>
</div>
<div class="sp-notice" id="sp-notice" hidden></div>
<div class="sp-chat" id="sp-chat" hidden>
  <div class="cd-log sp-log" id="sp-log" aria-live="polite"></div>
  <p class="sp-hint" id="sp-hint" hidden></p>
  <form class="sp-composer" id="sp-composer">
    <div class="sp-shot" id="sp-shot" hidden>
      <img id="sp-shot-img" alt="">
      <span>Screenshot attached</span>
      <button type="button" class="sp-icon" id="sp-shot-drop" title="Remove the screenshot" aria-label="Remove the screenshot">✕</button>
    </div>
    <textarea id="sp-input" rows="1" placeholder="Describe what you see…" aria-label="Message"></textarea>
    <div class="sp-tools">
      <button type="button" class="sp-icon" id="sp-capture" title="Attach a screenshot of this tab" aria-label="Attach a screenshot of this tab">${SP_ICON.camera}</button>
      <span class="sp-spacer"></span>
      <button type="submit" class="sp-send" title="Send (Enter)" aria-label="Send">${SP_ICON.send}</button>
    </div>
  </form>
</div>`);

const spEl = id => document.getElementById(id);
const spLog = spEl('sp-log');
const spInput = spEl('sp-input');
const spTarget = spEl('sp-target');
const spNotice = spEl('sp-notice');
const spMenu = spEl('sp-menu');
const spGlyph = chatGlyph(spEl('sp-glyph'), spEl('sp-avatar'));

let spUrl = null;          // the tab's URL
let spResolved = null;     // last `/api/embed/resolve` answer
let spSandbox = null;      // who the chat talks to…
let spThread = null;       // …and which of its threads; null is the main agent
let spThreads = [];        // its open threads, for the picker
let spPicking = false;     // the developer asked to change sandbox
let spShot = null;         // a screenshot waiting for the next message (data URL)
let spResolveSeq = 0;
let spCaptureSeq = 0;
const spCaptureWaits = new Map();
const spChats = new Map(); // "<sandbox>::<thread or ''>" → chat-core chat
const spShotsAnswered = new Set(); // relay request ids this panel has taken on

const spKey = () => `${spSandbox}::${spThread || ''}`;
const spMode = () => (spThread ? `thread:${spThread}` : 'claude');
function spChat(key = spKey()) {
  if (!spChats.has(key)) spChats.set(key, chatNew());
  return spChats.get(key);
}

// Same rule as main.js's `threadOfCwd`: a thread's agent runs in its worktree.
function spThreadOf(cwd) {
  const m = /\/\.sbxw-worktrees\/([a-z0-9-]+)(?:\/|$)/.exec(cwd || '');
  return m ? m[1] : null;
}

async function spJson(url, body) {
  try {
    const res = await fetch(url, body === undefined ? {} : {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });
    return await res.json();
  } catch (e) {
    return { ok: false, error: `sbxw did not answer (${e.message})` };
  }
}

function spSave() {
  try {
    const chats = [...spChats].map(([key, c]) => [key, c.log.slice(-60)]);
    sessionStorage.setItem(SP_STORE, JSON.stringify({ chats }));
  } catch (_) {}
}
function spLoad() {
  try {
    const s = JSON.parse(sessionStorage.getItem(SP_STORE) || 'null');
    (s?.chats || []).forEach(([key, log]) => { spChat(key).log = log || []; });
  } catch (_) {}
}

function spAutoReload() {
  try { return localStorage.getItem(SP_AUTORELOAD) === '1'; } catch (_) { return false; }
}

// On unless turned off: it is what this panel is for.
function spAutoShot() {
  try { return localStorage.getItem(SP_AUTOSHOT) !== '0'; } catch (_) { return true; }
}

// What Claude saw, as the transcript shows it: a line and a thumbnail of the
// very image it was sent. Its request is the tooltip — it was written for a
// person who would have had to take the screenshot, and read in the
// transcript it says "please send a capture" about a capture already taken.
function spShotNote(m) {
  const el = chatLine('note', m.text.replace(/^📷\s*/, ''));
  el.classList.add('sp-shot-note');
  el.insertAdjacentHTML('afterbegin', SP_ICON.camera);
  if (m.title) el.title = m.title;
  const img = document.createElement('img');
  img.alt = 'What Claude saw';
  // It decodes after the transcript is laid out, and grows it: stay at the end.
  img.addEventListener('load', () => { spLog.scrollTop = spLog.scrollHeight; }, { once: true });
  img.src = m.thumb;
  el.appendChild(img);
  return el;
}

// A small JPEG of a capture for the transcript, which sessionStorage holds.
async function spThumb(dataUrl) {
  const bitmap = await createImageBitmap(await (await fetch(dataUrl)).blob());
  const scale = Math.min(1, 320 / bitmap.width);
  const canvas = document.createElement('canvas');
  canvas.width = Math.max(1, Math.round(bitmap.width * scale));
  canvas.height = Math.max(1, Math.round(bitmap.height * scale));
  canvas.getContext('2d').drawImage(bitmap, 0, 0, canvas.width, canvas.height);
  if (bitmap.close) bitmap.close();
  return canvas.toDataURL('image/jpeg', 0.7);
}

// Scale a capture down to what the relay and the agent's context want: PNG
// first, JPEG when PNG is too big (see relay.js's `relayShrink`).
async function spShrink(dataUrl) {
  const bitmap = await createImageBitmap(await (await fetch(dataUrl)).blob());
  const scale = Math.min(1, SP_SHOT_MAX_EDGE / Math.max(bitmap.width, bitmap.height));
  const canvas = document.createElement('canvas');
  canvas.width = Math.max(1, Math.round(bitmap.width * scale));
  canvas.height = Math.max(1, Math.round(bitmap.height * scale));
  canvas.getContext('2d').drawImage(bitmap, 0, 0, canvas.width, canvas.height);
  if (bitmap.close) bitmap.close();
  let url = canvas.toDataURL('image/png');
  if (url.length > SP_SHOT_MAX_B64) url = canvas.toDataURL('image/jpeg', 0.85);
  return url;
}

// ── The extension ─────────────────────────────────────────────────────────
// Orders come from the extension's own origin only, and only from the frame
// that holds this page; replies go to that origin and nowhere else.
function spToParent(msg) {
  if (spFramed) window.parent.postMessage(msg, SP_EXTENSION_ORIGIN);
}

window.addEventListener('message', e => {
  if (e.source !== window.parent || e.origin !== SP_EXTENSION_ORIGIN) return;
  const m = e.data || {};
  if (m.type === 'sbxw:tab') {
    if (m.blocked) {
      spUrl = null;
      spResolved = null;
      spSandbox = null;
      spNote('The chat can’t see the tab beside this panel yet: see the note above.');
    } else spSetTab(m.url || null);
  }
  else if (m.type === 'sbxw:capture-result') {
    const done = spCaptureWaits.get(m.id);
    if (done) { spCaptureWaits.delete(m.id); done(m); }
  }
});

function spCapture() {
  const id = ++spCaptureSeq;
  return new Promise(resolve => {
    spCaptureWaits.set(id, resolve);
    spToParent({ type: 'sbxw:capture', id });
    setTimeout(() => {
      if (spCaptureWaits.delete(id)) resolve({ error: 'the extension did not answer' });
    }, SP_CAPTURE_TIMEOUT_MS);
  });
}

// ── Which sandbox ─────────────────────────────────────────────────────────
function spSetTab(url) {
  const before = spUrl;
  spUrl = url;
  let host = null;
  try { host = url && /^https?:$/.test(new URL(url).protocol) ? new URL(url).host : null; } catch (_) {}
  // Moving around inside the app is the common case: same host, same sandbox.
  let beforeHost = null;
  try { beforeHost = new URL(before).host; } catch (_) {}
  if (spResolved && spSandbox && host && host === beforeHost) {
    spRender();
    return;
  }
  spResolved = null;
  spSandbox = null;
  spPicking = false;
  if (!url) return spNote('The chat follows the tab beside this panel. Open the app you are testing there.');
  if (!host) return spNote('This tab is not a web page. Open the app you are testing here, and the chat with its sandbox appears beside it.');
  spResolve();
}

async function spResolve() {
  const seq = ++spResolveSeq;
  spNote('Finding the sandbox that serves this page…');
  const r = await spJson(`/api/embed/resolve?url=${encodeURIComponent(spUrl)}`);
  if (seq !== spResolveSeq) return;
  if (!r.ok) return spNote(r.error || 'sbxw could not tell which sandbox serves this page.');
  spResolved = r;
  if (r.sandbox && spIsRunning(r.sandbox)) spSetSandbox(r.sandbox);
  else spRender();
}

// Only running sandboxes are offered. An older daemon sends no `running`:
// then every sandbox counts.
function spIsRunning(name) {
  return !spResolved?.running || spResolved.running.includes(name);
}

async function spSetSandbox(name) {
  if (name !== spSandbox) spThread = null;
  spSandbox = name;
  spPicking = false;
  spRender();
  spLoadThreads();
  const list = await spJson('/api/sessions');
  if (Array.isArray(list)) list.forEach(info => spIngest(info, true));
  spRender();
  spAnswerPendingShots();
}

async function spLoadThreads() {
  const name = spSandbox;
  const r = await spJson(`/api/sandboxes/${encodeURIComponent(name)}/threads`);
  if (name !== spSandbox) return;
  spThreads = (r.threads || []).filter(t => t.open).map(t => t.name);
  if (spThread && !spThreads.includes(spThread)) spThread = null;
  spRenderHead();
}

async function spPair(name) {
  const r = await spJson('/api/embed/pairings', { host: spResolved.host, sandbox: name });
  if (!r.ok) return spNote(`Could not remember this choice: ${r.error}`);
  spResolved = { ...spResolved, sandbox: name, via: 'pairing', stale_pairing: null };
  spSetSandbox(name);
}

async function spUnpair() {
  const r = await spJson('/api/embed/pairings/rm', { host: spResolved.host });
  if (!r.ok) return spNote(`Could not forget the pairing: ${r.error}`);
  spSandbox = null;
  spResolve();
}

// ── Rendering ─────────────────────────────────────────────────────────────
function spNote(text) {
  spNotice.replaceChildren();
  const p = document.createElement('p');
  p.textContent = text;
  spNotice.appendChild(p);
  spShow('notice');
}

function spShow(which) {
  spNotice.hidden = which !== 'notice';
  spEl('sp-chat').hidden = which !== 'chat';
  spEl('sp-head').hidden = which !== 'chat';
}

function spRender() {
  if (!spResolved) return;
  if (!spSandbox || spPicking) return spRenderPicker();
  spShow('chat');
  spRenderHead();
  spRenderLog();
}

function spRenderPicker() {
  const r = spResolved;
  spNotice.replaceChildren();
  const ask = document.createElement('p');
  ask.textContent = `Which sandbox serves ${r.host}?`;
  spNotice.appendChild(ask);
  const why = document.createElement('p');
  why.className = 'sp-dim';
  why.textContent = r.stale_pairing
    ? `It was paired with ${r.stale_pairing}, which no longer exists. Your choice is remembered for ${r.host}.`
    : r.sandbox && !spIsRunning(r.sandbox)
      ? `It is paired with ${r.sandbox}, which is stopped. Start it in sbxw, or pick a running one.`
    : r.candidates.length > 1
      ? `${r.candidates.length} sandboxes publish this address. Your choice is remembered for ${r.host}.`
      : `sbxw does not route ${r.host} itself, so it can't tell. Your choice is remembered for ${r.host}.`;
  spNotice.appendChild(why);

  const live = r.sandboxes.filter(spIsRunning);
  const likely = r.candidates.filter(spIsRunning);
  const others = live.filter(n => !likely.includes(n));
  const list = document.createElement('div');
  list.className = 'sp-choices';
  [...likely, ...others].forEach(name => {
    const b = document.createElement('button');
    b.type = 'button';
    b.textContent = name;
    if (r.candidates.includes(name)) {
      b.classList.add('sp-likely');
      const hint = document.createElement('small');
      hint.textContent = 'publishes this address';
      b.appendChild(hint);
    }
    if (spPicking && name === spSandbox) {
      const now = document.createElement('small');
      now.textContent = 'current';
      b.appendChild(now);
    }
    b.addEventListener('click', () => spPair(name));
    list.appendChild(b);
  });
  if (!live.length) {
    const none = document.createElement('p');
    none.className = 'sp-dim';
    none.textContent = r.sandboxes.length
      ? 'No sandbox is running. Start or create one in sbxw, then come back to this tab.'
      : 'No sandbox exists yet. Create one in sbxw, then come back to this tab.';
    const go = document.createElement('button');
    go.type = 'button';
    go.className = 'sp-likely';
    go.textContent = 'Open sbxw';
    go.addEventListener('click', () => window.open(`${location.origin}/`, '_blank'));
    list.append(none, go);
  }
  spNotice.appendChild(list);

  if (spPicking) {
    const row = document.createElement('div');
    row.className = 'sp-row';
    if (r.via === 'pairing') {
      const forget = document.createElement('button');
      forget.type = 'button';
      forget.className = 'sp-link';
      forget.textContent = `forget the pairing for ${r.host}`;
      forget.addEventListener('click', spUnpair);
      row.appendChild(forget);
    }
    const spacer = document.createElement('span');
    spacer.className = 'sp-spacer';
    const cancel = document.createElement('button');
    cancel.type = 'button';
    cancel.className = 'sp-link';
    cancel.textContent = 'cancel';
    cancel.addEventListener('click', () => { spPicking = false; spRender(); });
    row.append(spacer, cancel);
    spNotice.appendChild(row);
  }
  spShow('notice');
}

// The page as a path: the host is the app's, and the app is the sandbox's.
function spPathOf(url) {
  try {
    const u = new URL(url);
    return u.pathname + u.search + u.hash;
  } catch (_) {
    return url || '';
  }
}

function spRenderHead() {
  const state = spChat().state;
  const avatar = spEl('sp-avatar');
  spGlyph.setWorking(state === 'working');
  avatar.classList.toggle('attention', state === 'attention');
  avatar.title = state === 'working' ? `${spSandbox} is working`
    : state === 'attention' ? `${spSandbox} is waiting for you`
    : `${spSandbox} is ready`;
  spEl('sp-name').textContent = spSandbox;
  spEl('sp-page').textContent = spPathOf(spUrl);
  spEl('sp-page').title = spUrl || '';
  spTarget.replaceChildren();
  const main = new Option('main agent', '');
  spTarget.appendChild(main);
  spThreads.forEach(t => spTarget.appendChild(new Option(`thread ${t}`, t)));
  spTarget.value = spThread || '';
  spTarget.hidden = !spThreads.length;
  spEl('sp-reload').disabled = !spFramed;
  spEl('sp-capture').disabled = !spFramed;
  spEl('sp-autoshot').disabled = !spFramed;
}

// What an empty conversation says: who is listening, and what goes along.
function spEmpty() {
  const box = document.createElement('div');
  box.className = 'sp-empty';
  const lead = document.createElement('p');
  lead.className = 'sp-empty-lead';
  lead.textContent = spThread
    ? `Tell the ${spThread} thread of ${spSandbox} what you see on this page.`
    : `Tell ${spSandbox} what you see on this page.`;
  const more = document.createElement('p');
  more.textContent = spFramed && spAutoShot()
    ? 'The page address goes with every message, and Claude can take a screenshot of this tab whenever it needs to look.'
    : 'The page address goes with every message. The camera attaches a screenshot.';
  box.append(lead, more);
  return box;
}

function spRenderLog() {
  const c = spChat();
  spLog.replaceChildren();
  if (!c.log.length) spLog.appendChild(spEmpty());
  // Who said what is the layout's job here: your messages on the right,
  // Claude's as plain prose. No name over every reply.
  c.log.forEach(m => spLog.appendChild(m.thumb ? spShotNote(m) : chatLine(m.who, m.text, m.who === 'ai' ? null : m.meta)));
  const tail = c.state === 'working' ? chatStatus(c.activity || 'Working…') : chatTail(c, {
    sandbox: spSandbox,
    mode: spMode(),
    openLabel: 'Answer in sbxw',
    onOpen: spOpenInSbxw,
    onChange: () => { spSave(); spRender(); },
  });
  if (tail) spLog.appendChild(tail);
  spLog.scrollTop = spLog.scrollHeight;
  spRenderShot();
}

function spRenderShot() {
  spEl('sp-shot').hidden = !spShot;
  if (spShot) spEl('sp-shot-img').src = spShot;
}

function spHint(text, warn = false) {
  const h = spEl('sp-hint');
  h.textContent = text || '';
  h.hidden = !text;
  h.classList.toggle('warn', warn);
}

// The full session lives in the web UI: point the sbxw tab at this sandbox,
// or open one when there is none (`#sandbox=` is the island's deep link).
async function spOpenInSbxw() {
  const r = await spJson('/api/focus', { sandbox: spSandbox });
  if (!r || !r.clients) window.open(`${location.origin}/#sandbox=${encodeURIComponent(spSandbox)}`, '_blank');
  else spHint(`${spSandbox} is open in your sbxw tab.`);
}

// ── Sending ───────────────────────────────────────────────────────────────
// What the agent reads: where the developer is, what they said, and the
// screenshot's path inside the sandbox when there is one.
//
// When the panel answers screenshot requests, the header says so: an agent
// told only a URL replies "I can't see your screen" before it thinks of
// asking, and the developer reads that while the panel stands ready to show it.
function spCompose(text, url, shotPath) {
  const sight = spFramed && spAutoShot() ? ' · you can see this tab: ask_user_for_screenshot captures it instantly' : '';
  let message = `[sbxw side panel · ${url}${sight}]\n${text}`;
  if (shotPath) message += `\n\nScreenshot of the page: ${shotPath}`;
  return message;
}

async function spUploadShot(dataUrl) {
  const blob = await (await fetch(dataUrl)).blob();
  const res = await fetch(`/api/sandboxes/${encodeURIComponent(spSandbox)}/paste-image`, {
    method: 'POST',
    headers: { 'Content-Type': blob.type || 'image/png' },
    body: blob,
  });
  const r = await res.json();
  if (!r.ok || !r.path) throw new Error(r.error || 'no path returned');
  return r.path;
}

async function spSend(text) {
  const sandbox = spSandbox;
  const thread = spThread;
  const c = spChat();
  const url = spUrl;
  const shot = spShot;
  spShot = null;
  // A message sent mid-turn waits in Claude Code's queue for the next one.
  const queued = c.state === 'working';
  // The page, when it says more than the header's URL already does.
  let path = '';
  try { path = new URL(url).pathname + new URL(url).search; } catch (_) {}
  if (path === '/') path = '';
  const meta = [path ? `on ${path}` : null, shot ? 'with a screenshot' : null, queued ? 'queued' : null];
  c.log.push({ who: 'me', text, meta: meta.filter(Boolean).join(', ') });
  c.state = 'working';
  c.activity = null;
  spSave();
  spRender();

  let shotPath = null;
  if (shot) {
    try {
      shotPath = await spUploadShot(shot);
    } catch (e) {
      c.log.push({ who: 'err', text: `Screenshot not attached: ${e.message}. The message goes without it.` });
    }
  }
  const body = { text: spCompose(text, url, shotPath), name: sandbox };
  if (thread) body.thread = thread;
  const res = await spJson('/api/chat/push', body);
  if (!res.ok) {
    c.state = 'idle';
    c.log.push({ who: 'err', text: `Not sent: ${res.error || 'no answer from sbxw'}` });
    if (!spInput.value) {
      spInput.value = text;
      spInput.dispatchEvent(new Event('input')); // grow the box to fit it again
    }
  }
  spSave();
  spRender();
}

// ── Screenshots Claude asks for ───────────────────────────────────────────
// `ask_user_for_screenshot` opens a screenshot request on the daemon's relay
// and waits for a person to answer it in sbxw's popup. When this panel sits
// beside the app of the sandbox that asked, it answers instead: a capture of
// the tab, released with `POST /api/relay/:id/approve`, as the popup would.
//
// The consent the relay asks of a person per request is given here once, by
// the "Claude screenshots" box — on by default, since an agent able to look
// at its own change is why this panel exists. It stays narrow: screenshot
// requests only, from this panel's sandbox only, of the tab the developer put
// beside the panel, and every one is written into the transcript. A capture
// that fails leaves the request open for the popup; nothing is ever refused
// on the developer's behalf.
async function spAnswerShot(req) {
  if (!req || req.kind !== 'screenshot' || req.state !== 'pending') return;
  if (!spFramed || !spAutoShot() || !spSandbox || req.from !== spSandbox) return;
  if (spShotsAnswered.has(req.id) || Date.now() - (req.created_ms || 0) > SP_SHOT_FRESH_MS) return;
  spShotsAnswered.add(req.id);
  const c = spChat(`${req.from}::${req.from_thread || ''}`);
  const url = spUrl;
  const r = await spCapture();
  if (!r.dataUrl) {
    c.log.push({ who: 'err', text: `Claude asked to see the page, and no screenshot could be taken: ${r.error}. Its request waits in sbxw.` });
  } else {
    const image = await spShrink(r.dataUrl);
    const res = await spJson(`/api/relay/${encodeURIComponent(req.id)}/approve`, {
      answer: `Captured by the sbxw side panel from the browser tab beside it, ${url}, without asking the person: the page exactly as it was on screen, not resized, scrolled or navigated. If you need another state, say in your reply what to bring on screen.`,
      images: [image],
    });
    c.log.push(res.ok
      ? { who: 'note', text: 'Claude took a screenshot of this tab', title: req.question, thumb: await spThumb(image) }
      : { who: 'err', text: `Screenshot not sent to Claude: ${res.error || 'no answer from sbxw'}` });
  }
  spSave();
  spRender();
}

function spAnswerPendingShots() {
  spJson('/api/relay').then(r => (r.requests || []).forEach(spAnswerShot));
}

new EventSource('/api/relay/events').onmessage = ev => {
  try { spAnswerShot(JSON.parse(ev.data)); } catch (_) {}
};

// ── Session events ────────────────────────────────────────────────────────
// Only the conversation this panel shows: its sandbox's main agent and its
// threads, never a bash pane.
function spIngest(info, seed = false) {
  if (!info || info.sandbox !== spSandbox || (info.mode || 'claude') !== 'claude' || !chatOwnSession(info)) return;
  const thread = spThreadOf(info.cwd);
  const c = spChat(`${info.sandbox}::${thread || ''}`);
  const news = chatIngest(c, info, seed);
  // Answering types keystrokes into a PTY, and sbxw only knows it is typing
  // into the right one for a sandbox's main agent (see `is_answerable`).
  if (thread) c.answerable = false;
  if (news?.reply && thread === spThread && spAutoReload()) spToParent({ type: 'sbxw:reload' });
  spSave();
  if (!seed) spRender();
}

new EventSource('/api/events').onmessage = ev => {
  try { spIngest(JSON.parse(ev.data)); } catch (_) {}
};

// ── Wiring ────────────────────────────────────────────────────────────────
chatComposer(spEl('sp-composer'), spInput, spSend);
spTarget.addEventListener('change', () => { spThread = spTarget.value || null; spRender(); spInput.focus(); });
spTarget.addEventListener('focus', spLoadThreads);
// Back from sbxw, where a sandbox may just have been started: ask again.
window.addEventListener('focus', () => {
  if (spUrl && spResolved && !spSandbox && !spPicking) spResolve();
});
spEl('sp-change').addEventListener('click', async () => {
  spMenu.open = false;
  spPicking = true;
  // The list of sandboxes may be older than the last one created.
  const r = await spJson(`/api/embed/resolve?url=${encodeURIComponent(spUrl)}`);
  if (r.ok) spResolved = { ...r, sandbox: spSandbox };
  spRender();
});
spEl('sp-reload').addEventListener('click', () => spToParent({ type: 'sbxw:reload' }));
spEl('sp-open').addEventListener('click', () => { spMenu.open = false; spOpenInSbxw(); });
// The menu closes like a menu: on a click elsewhere, or Escape.
document.addEventListener('click', e => { if (spMenu.open && !spMenu.contains(e.target)) spMenu.open = false; });
spMenu.addEventListener('keydown', e => {
  if (e.key === 'Escape') { spMenu.open = false; spMenu.querySelector('summary').focus(); }
});
spEl('sp-capture').addEventListener('click', async () => {
  const b = spEl('sp-capture');
  b.disabled = true;
  const r = await spCapture();
  b.disabled = !spFramed;
  if (r.dataUrl) { spShot = await spShrink(r.dataUrl); spHint(); }
  else spHint(`No screenshot: ${r.error || 'the extension could not take one'}`, true);
  spRenderShot();
  spInput.focus();
});
spEl('sp-shot-drop').addEventListener('click', () => { spShot = null; spRenderShot(); });
const spShotBox = spEl('sp-autoshot');
spShotBox.checked = spAutoShot();
spShotBox.disabled = !spFramed;
spShotBox.addEventListener('change', () => {
  try { localStorage.setItem(SP_AUTOSHOT, spShotBox.checked ? '1' : '0'); } catch (_) {}
  if (spShotBox.checked) spAnswerPendingShots();
  spRender();
});
const spAuto = spEl('sp-autoreload');
spAuto.checked = spAutoReload();
spAuto.addEventListener('change', () => {
  try { localStorage.setItem(SP_AUTORELOAD, spAuto.checked ? '1' : '0'); } catch (_) {}
});

spLoad();
spHint();
if (spFramed) {
  spNote('Waiting for the tab beside this panel…');
  spToParent({ type: 'sbxw:ready' });
} else {
  // Opened on its own: the URL to chat about is given by hand.
  const url = new URLSearchParams(location.search).get('url');
  if (url) spSetTab(url);
  else spNote('This is the chat the sbxw browser extension shows beside the app you are testing. Open it from the extension, or add ?url=<the app\'s URL> to this address.');
}
