// ── Chat dock ─────────────────────────────────────────────────────────────
// A chat with the throwaway `ephemeral-chat[-N]` sandboxes that needs no
// terminal: a button in the bottom-right corner, and a panel over the panes.
// Asking a question and reading the answer is all most of these chats are, and
// giving one a whole pane for that was the wrong trade.
//
// It is the island's composer brought into the browser, on the same daemon
// calls: `POST /api/chat/push` types the message (provisioning first when the
// daemon picks the name), the `session` events on the stream main.js opens say
// what the agent is doing, the `reply` the `Stop` hook carries is the answer,
// and `POST /api/answer` picks an option of a pending question. Nothing here is
// chat-specific on the server side.
//
// The daemon keeps one reply per session — the last turn's — so the
// transcript is this tab's own, kept in sessionStorage so a reload doesn't
// lose it. That fits what these sandboxes are; "Open in a pane" is the way to
// the real, complete session when it matters.
//
// Loaded after main.js: it listens on `sbxwStream` and reads `sandboxes`. The
// conversation itself — events, transcript pieces, the composer — is
// chat-core.js, shared with the browser side panel.

const CHAT_DOCK_RE = /^ephemeral-chat(-\d+)?$/;
const CHAT_DOCK_STORE = 'sbxw-chat-dock';
// Same threshold as the island's composer: every ＋ costs a container.
const CHAT_DOCK_WARN_AT = 4;
const CHAT_DOCK_PEEK_MS = 15000;

const cdEl = id => document.getElementById(id);
const cdLauncher = cdEl('cd-launcher');
const cdPanel = cdEl('cd-panel');
const cdTabs = cdEl('cd-tabs');
const cdLog = cdEl('cd-log');
const cdInput = cdEl('cd-input');
const cdForm = cdEl('cd-composer');
const cdHint = cdEl('cd-hint');
const cdPeek = cdEl('cd-peek');
const cdBadge = cdEl('cd-badge');

// name → { log: [{who: 'me'|'ai'|'err', text}], state, steps, answerable, activity }
// `active === null` is the "＋ New chat" tab: its first message mints a sandbox.
const cdChats = new Map();
let cdActive = null;
let cdOpen = false;
let cdUnread = 0;
let cdSendingNew = null; // text of a fresh chat being provisioned, if any
let cdPeekTimer = null;

function cdChat(name) {
  if (!cdChats.has(name)) cdChats.set(name, chatNew());
  return cdChats.get(name);
}

function cdSave() {
  try {
    const chats = [...cdChats].map(([name, c]) => [name, c.log.slice(-60)]);
    sessionStorage.setItem(CHAT_DOCK_STORE, JSON.stringify({ chats, active: cdActive }));
  } catch (_) {}
}
function cdLoad() {
  try {
    const s = JSON.parse(sessionStorage.getItem(CHAT_DOCK_STORE) || 'null');
    if (!s) return;
    (s.chats || []).forEach(([name, log]) => { cdChat(name).log = log || []; });
    cdActive = s.active ?? null;
  } catch (_) {}
}

// Chats are the sandboxes that exist under a chat name, plus one being
// provisioned. A chat whose sandbox was removed leaves the dock with it.
function cdNames() {
  const live = sandboxes.map(s => s.name).filter(n => CHAT_DOCK_RE.test(n));
  return live.sort((a, b) => (+(a.split('-')[2] || 1)) - (+(b.split('-')[2] || 1)));
}

// ── Avatar ────────────────────────────────────────────────────────────────
// The `>_` of chat-core.js, on the launcher: it sweeps while any chat's agent
// works.
const cdGlyph = chatGlyph(cdEl('cd-glyph'), cdLauncher);
function cdSetWorking(on) {
  cdGlyph.setWorking(on);
}

// ── Rendering ─────────────────────────────────────────────────────────────
function cdRenderTabs() {
  cdTabs.replaceChildren();
  cdNames().forEach(name => {
    const c = cdChat(name);
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'cd-tab';
    b.setAttribute('role', 'tab');
    b.setAttribute('aria-selected', String(name === cdActive));
    const dot = document.createElement('span');
    dot.className = 'cd-dot ' + c.state;
    b.append(dot, name);
    b.addEventListener('click', () => { cdActive = name; cdRender(); cdInput.focus(); });
    cdTabs.appendChild(b);
  });
  const add = document.createElement('button');
  add.type = 'button';
  add.className = 'cd-tab cd-new';
  add.setAttribute('role', 'tab');
  add.setAttribute('aria-selected', String(cdActive === null));
  add.title = 'Start a new throwaway sandbox';
  add.textContent = cdSendingNew ? 'starting…' : '＋ New chat';
  add.addEventListener('click', () => { cdActive = null; cdRender(); cdInput.focus(); });
  cdTabs.appendChild(add);
}

function cdRenderLog() {
  cdLog.replaceChildren();
  if (cdActive === null) {
    if (cdSendingNew) {
      cdLog.append(chatLine('me', cdSendingNew), chatStatus('Starting a sandbox. The first answer takes a little longer.'));
    } else {
      cdLog.appendChild(chatLine('ai', 'Ask anything. Your first message starts a throwaway sandbox with its own agent, and every next message answers as fast as Claude does.', 'new chat'));
    }
    return;
  }
  const c = cdChat(cdActive);
  if (!c.log.length) cdLog.appendChild(chatLine('ai', 'No messages in this tab yet. Earlier turns of this chat are in its pane.', cdActive));
  c.log.forEach(m => cdLog.appendChild(chatLine(m.who, m.text, m.who === 'ai' ? 'claude' : null)));
  const name = cdActive;
  const tail = chatTail(c, {
    sandbox: name,
    openLabel: 'Answer in a pane',
    onOpen: () => cdOpenInPane(name),
    onChange: () => { cdSave(); cdRender(); },
  });
  if (tail) cdLog.appendChild(tail);
  cdLog.scrollTop = cdLog.scrollHeight;
}

function cdRenderHint() {
  if (cdActive === null && sandboxes.length >= CHAT_DOCK_WARN_AT) {
    cdHint.textContent = `${sandboxes.length} sandboxes already hold disk and memory. Reply in a chat you have, or remove the ones you're done with.`;
    cdHint.classList.add('warn');
  } else {
    cdHint.textContent = 'Enter to send · Shift+Enter for a new line';
    cdHint.classList.remove('warn');
  }
  cdEl('cd-to-pane').hidden = cdActive === null;
}

function cdRenderLauncher() {
  const states = cdNames().map(n => cdChat(n).state);
  cdSetWorking(states.includes('working') || !!cdSendingNew);
  cdLauncher.classList.toggle('attention', states.includes('attention'));
  cdLauncher.setAttribute('aria-expanded', String(cdOpen));
  cdBadge.hidden = !cdUnread || cdOpen;
  cdBadge.textContent = cdUnread;
  cdPanel.hidden = !cdOpen;
  if (cdOpen) cdPeek.hidden = true;
}

function cdRender() {
  cdRenderLauncher();
  if (!cdOpen) return;
  // Pruned only while open: the list is refreshed on every open, and before
  // main.js's first load it is empty, which would drop a restored tab.
  if (cdActive !== null && !cdNames().includes(cdActive)) cdActive = cdNames()[0] ?? null;
  cdRenderTabs();
  cdRenderLog();
  cdRenderHint();
}

function cdToggle(open = !cdOpen) {
  cdOpen = open;
  if (open) {
    cdUnread = 0;
    // The list may be older than the last chat someone made elsewhere.
    loadSandboxes().then(cdRender);
  }
  cdRender();
  if (open) cdInput.focus();
  else cdLauncher.focus();
}

function cdOpenInPane(name) {
  cdToggle(false);
  focusSandbox(name);
}

// ── Sending ───────────────────────────────────────────────────────────────
async function cdSend(text) {
  const target = cdActive;
  if (target === null) {
    if (cdSendingNew) return; // one sandbox at a time
    cdSendingNew = text;
  } else {
    const c = cdChat(target);
    c.log.push({ who: 'me', text });
    c.state = 'working';
    c.activity = null;
  }
  cdRender();
  const res = await postJsonOrError('/api/chat/push', target === null ? { text, fresh: true } : { text, name: target });
  if (target === null) {
    cdSendingNew = null;
    if (res.ok && res.name) {
      const c = cdChat(res.name);
      c.log.push({ who: 'me', text });
      if (c.state !== 'idle' || !c.log.some(m => m.who === 'ai')) c.state = 'working';
      // Only follow it if nobody switched tabs while it booted.
      if (cdActive === null) cdActive = res.name;
      await loadSandboxes();
    } else {
      cdInput.value = cdInput.value || text;
    }
  } else if (!res.ok) {
    const c = cdChat(target);
    c.state = 'idle';
    c.log.push({ who: 'err', text: `Not sent: ${res.error || 'no answer from the daemon'}` });
    if (!cdInput.value) cdInput.value = text;
  }
  cdSave();
  cdRender();
  if (target === null && !res.ok && cdActive === null && cdOpen) cdLog.appendChild(chatLine('err', `Chat not started: ${res.error || 'no answer from the daemon'}`));
}

chatComposer(cdForm, cdInput, cdSend);

// ── Session events ────────────────────────────────────────────────────────
// Only a chat's own agent: its threads (a worktree cwd) and a bash pane are
// not the conversation the dock shows.
function cdIngest(info, seed = false) {
  if (!info || !CHAT_DOCK_RE.test(info.sandbox || '')) return;
  if ((info.mode || 'claude') !== 'claude' || threadOfCwd(info.cwd) || !chatOwnSession(info)) return;
  const news = chatIngest(cdChat(info.sandbox), info, seed);
  if (news?.reply && (!cdOpen || cdActive !== info.sandbox)) cdNotify(info.sandbox, news.reply);
  else if (news?.asked && !cdOpen) cdNotify(info.sandbox, news.asked);
  cdSave();
  cdRender();
}

function cdNotify(name, text) {
  cdUnread++;
  if (cdOpen) return;
  cdEl('cd-peek-who').textContent = `${name} replied`;
  cdEl('cd-peek-text').textContent = text;
  cdPeek.dataset.name = name;
  cdPeek.hidden = false;
  clearTimeout(cdPeekTimer);
  cdPeekTimer = setTimeout(() => { cdPeek.hidden = true; }, CHAT_DOCK_PEEK_MS);
}

sbxwStream.addEventListener('session', ev => {
  try { cdIngest(JSON.parse(ev.data)); } catch (_) {}
});

// ── Wiring ────────────────────────────────────────────────────────────────
cdLauncher.addEventListener('click', () => cdToggle());
cdEl('cd-close').addEventListener('click', () => cdToggle(false));
cdEl('cd-to-pane').addEventListener('click', () => { if (cdActive) cdOpenInPane(cdActive); });
cdPeek.addEventListener('click', () => {
  if (cdPeek.dataset.name) cdActive = cdPeek.dataset.name;
  cdToggle(true);
});
// Esc closes the panel only from inside it: everywhere else Esc belongs to the
// terminal (Claude Code reads it as "interrupt").
cdPanel.addEventListener('keydown', e => {
  if (e.key === 'Escape') { e.stopPropagation(); cdToggle(false); }
});
// Alt+C (⌥C), on the same terms as create.js's ⌥N: matched on `code` (⌥C is
// "ç" on a Mac), in the capture phase because xterm swallows what it handles,
// and not over a modal. Ctrl/⌘+K would have been the obvious pick, but Ctrl+K
// is kill-line in every shell a pane runs.
window.addEventListener('keydown', e => {
  if (e.code !== 'KeyC' || !e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
  if (document.querySelector('.modal-overlay:not(.hidden)')) return;
  e.preventDefault();
  e.stopPropagation();
  cdToggle();
}, true);

cdLoad();
fetch('/api/sessions')
  .then(r => r.json())
  .then(list => list.forEach(i => cdIngest(i, true)))
  .catch(() => {})
  .finally(cdRender);
