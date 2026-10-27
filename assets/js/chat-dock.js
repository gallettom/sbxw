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
// "＋ New chat" is another session in `ephemeral-chat` — a `chat-N` thread,
// in a worktree of the chat's throwaway repo — not another sandbox: one
// container answers every conversation. The sandbox is made only when there
// is none. Older `ephemeral-chat-N` sandboxes still show while they exist.
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
const cdDock = cdEl('cd-dock');
const cdTuck = cdEl('cd-tuck');

// key → { log: [{who: 'me'|'ai'|'err', text}], state, steps, answerable, activity }
// A key is a sandbox (its own agent) or `<sandbox>::<thread>` (a chat session).
// `active === null` is the "＋ New chat" tab: its first message opens a session.
const cdChats = new Map();
let cdActive = null;
let cdOpen = false;
let cdUnread = 0;
let cdSendingNew = null; // text of a new chat being opened, if any
let cdPeekTimer = null;
let cdTucked = false;

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

function cdKey(sandbox, thread) {
  return thread ? `${sandbox}::${thread}` : sandbox;
}
function cdSplit(key) {
  const [sandbox, thread = null] = key.split('::');
  return { sandbox, thread };
}
// The tab's label: a session goes by its thread, `chat-2`.
function cdLabel(key) {
  const { sandbox, thread } = cdSplit(key);
  return thread || sandbox;
}

// Chats are the sandboxes that exist under a chat name, and the sessions seen
// in them. A chat whose sandbox was removed leaves the dock with it.
function cdNames() {
  const live = sandboxes.map(s => s.name).filter(n => CHAT_DOCK_RE.test(n));
  const sessions = [...cdChats.keys()].filter(k => k.includes('::') && live.includes(cdSplit(k).sandbox));
  const num = s => +(s.split('-').pop()) || 1;
  return [...live, ...sessions].sort((a, b) => {
    const x = cdSplit(a), y = cdSplit(b);
    return num(x.sandbox) - num(y.sandbox) || (x.thread ? num(x.thread) : 0) - (y.thread ? num(y.thread) : 0);
  });
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
    b.append(dot, cdLabel(name));
    b.addEventListener('click', () => { cdActive = name; cdRender(); cdInput.focus(); });
    // A session can be closed; the sandbox's own agent is the chat itself and
    // goes with `sbxw rm`. A sibling of the tab, not inside it: a button
    // cannot hold another.
    if (!cdSplit(name).thread) { cdTabs.appendChild(b); return; }
    const wrap = document.createElement('span');
    wrap.className = 'cd-tabwrap';
    const x = document.createElement('button');
    x.type = 'button';
    x.className = 'cd-tab-x';
    x.title = `Close ${cdLabel(name)}`;
    x.setAttribute('aria-label', `Close ${cdLabel(name)}`);
    x.textContent = '×';
    x.addEventListener('click', () => cdCloseSession(name));
    wrap.append(b, x);
    cdTabs.appendChild(wrap);
  });
  const add = document.createElement('button');
  add.type = 'button';
  add.className = 'cd-tab cd-new';
  add.setAttribute('role', 'tab');
  add.setAttribute('aria-selected', String(cdActive === null));
  add.title = 'Start another conversation in ephemeral-chat';
  add.textContent = cdSendingNew ? 'starting…' : '＋ New chat';
  add.addEventListener('click', () => { cdActive = null; cdRender(); cdInput.focus(); });
  cdTabs.appendChild(add);
}

function cdRenderLog() {
  cdLog.replaceChildren();
  if (cdActive === null) {
    if (cdSendingNew) {
      cdLog.append(chatLine('me', cdSendingNew), chatStatus('Starting a session. The first answer takes a little longer.'));
    } else {
      cdLog.appendChild(chatLine('ai', 'Ask anything. Your first message starts a new session in ephemeral-chat, and every next message answers as fast as Claude does.', 'new chat'));
    }
    return;
  }
  const c = cdChat(cdActive);
  if (!c.log.length) cdLog.appendChild(chatLine('ai', 'No messages in this tab yet. Earlier turns of this chat are in its pane.', cdLabel(cdActive)));
  c.log.forEach(m => cdLog.appendChild(chatLine(m.who, m.text, m.who === 'ai' ? 'claude' : null)));
  const name = cdActive;
  const { sandbox, thread } = cdSplit(name);
  const tail = chatTail(c, {
    sandbox,
    mode: thread ? `thread:${thread}` : 'claude',
    openLabel: 'Answer in a pane',
    onOpen: () => cdOpenInPane(name),
    onChange: () => { cdSave(); cdRender(); },
  });
  if (tail) cdLog.appendChild(tail);
  cdLog.scrollTop = cdLog.scrollHeight;
}

function cdRenderHint() {
  cdHint.textContent = 'Enter to send · Shift+Enter for a new line';
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
  if (cdOpen) { cdPeek.hidden = true; cdSetTucked(false); }
  // Tucked, the tab carries what the launcher would have said.
  cdTuck.classList.toggle('attention', states.includes('attention'));
  cdTuck.classList.toggle('unread', !!cdUnread);
}

// ── Out of the way ────────────────────────────────────────────────────────
// The launcher sits over the bottom-right corner of whichever pane is there,
// on the last lines of its terminal. While that terminal has focus, the
// launcher slides off the edge and leaves a tab; the tab brings it back, and
// so does focus going anywhere else. Focus, not the pointer: typing is when
// the corner matters, and a launcher that fled the pointer could not be hit.
const CD_CORNER = 80; // px from the window's corner the launcher covers, gap included
function cdCoversPane(pane) {
  const r = pane.getBoundingClientRect();
  return r.right > window.innerWidth - CD_CORNER && r.bottom > window.innerHeight - CD_CORNER;
}
function cdSetTucked(on) {
  cdTucked = on && !cdOpen;
  cdDock.classList.toggle('tucked', cdTucked);
  cdTuck.hidden = !cdTucked;
}
document.addEventListener('focusin', e => {
  if (cdDock.contains(e.target)) return;
  const term = e.target.closest?.('.pane-term');
  cdSetTucked(!!term && cdCoversPane(term));
});

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

function cdOpenInPane(key) {
  cdToggle(false);
  const { sandbox, thread } = cdSplit(key);
  if (!thread) { focusSandbox(sandbox); return; }
  const open = panes.slice(0, paneCount).findIndex(p => p.sandbox === sandbox && p.mode === 'thread' && p.thread === thread);
  if (open >= 0) setFocusedPane(open);
  else connectPane(focusedPane, sandbox, 'thread', thread);
}

// Closing a session ends its agent and removes its worktree and branch: a
// chat worktree holds nothing (the repo is one empty commit), so there is
// nothing to keep. Asked only while it is mid-answer.
async function cdCloseSession(key) {
  const { sandbox, thread } = cdSplit(key);
  if (cdChat(key).state === 'working' && !confirm(`${thread} is still answering. Close it anyway?`)) return;
  const res = await postJsonOrError(`/api/sandboxes/${encodeURIComponent(sandbox)}/threads/${encodeURIComponent(thread)}/close`, { discard: true, delete_branch: true });
  if (!res.ok) {
    cdChat(key).log.push({ who: 'err', text: `Not closed: ${res.error || 'no answer from the daemon'}` });
  } else {
    cdChats.delete(key);
    if (cdActive === key) cdActive = sandbox;
    afterThreadClosed(sandbox, thread);
  }
  cdSave();
  cdRender();
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
  const to = target === null ? null : cdSplit(target);
  const res = await postJsonOrError('/api/chat/push', to === null ? { text, session: true } : { text, name: to.sandbox, thread: to.thread });
  if (target === null) {
    cdSendingNew = null;
    if (res.ok && res.name) {
      const key = cdKey(res.name, res.thread);
      const c = cdChat(key);
      c.log.push({ who: 'me', text });
      if (c.state !== 'idle' || !c.log.some(m => m.who === 'ai')) c.state = 'working';
      // Only follow it if nobody switched tabs while it booted.
      if (cdActive === null) cdActive = key;
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
// A chat's agents — its own and its sessions (a worktree cwd) — but not a
// bash pane.
function cdIngest(info, seed = false) {
  if (!info || !CHAT_DOCK_RE.test(info.sandbox || '')) return;
  if ((info.mode || 'claude') !== 'claude' || !chatOwnSession(info)) return;
  const key = cdKey(info.sandbox, threadOfCwd(info.cwd));
  // The end of a session closed here would otherwise bring its tab back.
  if (info.state === 'exited' && !cdChats.has(key)) return;
  const news = chatIngest(cdChat(key), info, seed);
  if (news?.reply && (!cdOpen || cdActive !== key)) cdNotify(key, news.reply);
  else if (news?.asked && !cdOpen) cdNotify(key, news.asked);
  cdSave();
  cdRender();
}

function cdNotify(name, text) {
  cdUnread++;
  if (cdOpen) return;
  cdEl('cd-peek-who').textContent = `${cdLabel(name)} replied`;
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
cdTuck.addEventListener('click', () => { cdSetTucked(false); cdLauncher.focus(); });
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
// Alt+M (⌃⌥M on a Mac), on the same terms as create.js's Alt+N. Not C: window
// managers (Rectangle, Magnet) take ⌃⌥C for "center window" before the page
// ever sees it. In the capture phase because xterm swallows what it handles,
// and not over a modal. Ctrl/⌘+K would have been the obvious pick, but Ctrl+K
// is kill-line in every shell a pane runs.
window.addEventListener('keydown', e => {
  if (!isAppShortcut(e, 'KeyM')) return;
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
