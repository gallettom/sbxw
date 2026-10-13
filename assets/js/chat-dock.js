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
// Loaded after main.js: it listens on `sbxwStream` and reads `sandboxes`.

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
  if (!cdChats.has(name)) cdChats.set(name, { log: [], state: 'idle', steps: null, answerable: false, activity: null });
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
// The header's ">_" in the 5×7 dot-matrix of the corner job cards (util.js),
// so the dock reads as one of the family. The cursor blinks at rest; while an
// agent works the grid fills and drains like a running job.
const CD_GLYPH = [
  [['10000', '01000', '00100', '00010', '00100', '01000', '10000'], '#3fb950', 0, false],
  [['000', '000', '000', '000', '000', '000', '111'], '#58a6ff', 6, true],
];
const cdPixels = [];
(() => {
  const grid = cdEl('cd-glyph');
  grid.style.setProperty('--cols', 9);
  CD_GLYPH.forEach(([pattern, color, offset, cursor]) => {
    for (let r = 0; r < 7; r++) for (let c = 0; c < pattern[r].length; c++) {
      if (pattern[r][c] !== '1') continue;
      const px = document.createElement('div');
      px.className = 'px' + (cursor ? ' cd-cursor' : '');
      px.style.gridColumn = offset + c + 1;
      px.style.gridRow = r + 1;
      px.style.setProperty('--c', color);
      grid.appendChild(px);
      cdPixels.push({ el: px, row: r });
    }
  });
})();

let cdSweep = null;
function cdSetWorking(on) {
  if (on === !!cdSweep) return;
  cdLauncher.classList.toggle('working', on);
  clearInterval(cdSweep);
  cdSweep = null;
  cdPixels.forEach(p => p.el.classList.add('lit'));
  if (!on) return;
  let level = 0, dir = 1;
  cdSweep = setInterval(() => {
    cdPixels.forEach(p => p.el.classList.toggle('lit', p.row >= 7 - level));
    level += dir;
    if (level >= 7 || level <= 0) dir = -dir;
  }, 110);
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

function cdLine(cls, text, meta) {
  const el = document.createElement('div');
  el.className = 'cd-msg ' + cls;
  if (meta) {
    const m = document.createElement('span');
    m.className = 'cd-meta';
    m.textContent = meta;
    el.appendChild(m);
  }
  el.appendChild(document.createTextNode(text));
  return el;
}

function cdStatus(text) {
  const s = document.createElement('div');
  s.className = 'cd-status';
  const bar = document.createElement('span');
  bar.className = 'cd-bar';
  s.append(bar, text);
  return s;
}

// A pending `AskUserQuestion`: one row of options per step, submitted at once
// like the terminal does. A prompt sbxw cannot type into (see `is_answerable`)
// or one with no structured steps (a permission request) is answered in the
// session itself.
function cdQuestion(name, c) {
  const box = document.createElement('div');
  box.className = 'cd-question';
  if (!c.steps?.length || !c.answerable) {
    const p = document.createElement('p');
    p.textContent = c.steps?.[0]?.text || c.activity || 'Claude is waiting for you.';
    const open = document.createElement('button');
    open.type = 'button';
    open.textContent = 'Answer in a pane';
    open.addEventListener('click', () => cdOpenInPane(name));
    box.append(p, open);
    return box;
  }
  const picked = c.steps.map(() => null);
  const submit = document.createElement('button');
  submit.type = 'button';
  submit.className = 'cd-answer';
  submit.textContent = 'Send answer';
  submit.hidden = c.steps.length === 1;
  const send = async () => {
    const indices = picked.map(i => i + 1);
    box.querySelectorAll('button').forEach(b => { b.disabled = true; });
    const labels = c.steps.map((s, i) => s.options[picked[i]]).join(' · ');
    try {
      const res = await fetch('/api/answer', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ sandbox: name, mode: 'claude', index: indices[0], indices }),
      });
      if (!res.ok) throw new Error(`the daemon answered ${res.status}`);
      c.log.push({ who: 'me', text: labels });
      c.steps = null;
      c.state = 'working';
    } catch (e) {
      c.log.push({ who: 'err', text: `Answer not sent: ${e.message}. Answer in a pane instead.` });
    }
    cdSave();
    cdRender();
  };
  c.steps.forEach((step, si) => {
    const p = document.createElement('p');
    p.textContent = step.text;
    const opts = document.createElement('div');
    opts.className = 'cd-opts';
    step.options.forEach((label, oi) => {
      const b = document.createElement('button');
      b.type = 'button';
      b.textContent = label;
      if (step.context?.[oi]) b.title = step.context[oi];
      b.addEventListener('click', () => {
        picked[si] = oi;
        opts.querySelectorAll('button').forEach((x, xi) => x.classList.toggle('picked', xi === oi));
        if (c.steps.length === 1) send();
        else submit.disabled = picked.includes(null);
      });
      opts.appendChild(b);
    });
    box.append(p, opts);
  });
  submit.disabled = true;
  submit.addEventListener('click', send);
  box.appendChild(submit);
  return box;
}

function cdRenderLog() {
  cdLog.replaceChildren();
  if (cdActive === null) {
    if (cdSendingNew) {
      cdLog.append(cdLine('me', cdSendingNew), cdStatus('Starting a sandbox. The first answer takes a little longer.'));
    } else {
      cdLog.appendChild(cdLine('ai', 'Ask anything. Your first message starts a throwaway sandbox with its own agent, and every next message answers as fast as Claude does.', 'new chat'));
    }
    return;
  }
  const c = cdChat(cdActive);
  if (!c.log.length) cdLog.appendChild(cdLine('ai', 'No messages in this tab yet. Earlier turns of this chat are in its pane.', cdActive));
  c.log.forEach(m => cdLog.appendChild(cdLine(m.who, m.text, m.who === 'ai' ? 'claude' : null)));
  if (c.state === 'working') cdLog.appendChild(cdStatus(c.activity ? `Claude is working · ${c.activity}` : 'Claude is working…'));
  if (c.state === 'attention') cdLog.appendChild(cdQuestion(cdActive, c));
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
  if (target === null && !res.ok && cdActive === null && cdOpen) cdLog.appendChild(cdLine('err', `Chat not started: ${res.error || 'no answer from the daemon'}`));
}

cdForm.addEventListener('submit', e => {
  e.preventDefault();
  const text = cdInput.value.trim();
  if (!text) return;
  cdInput.value = '';
  cdInput.style.height = '';
  cdSend(text);
});
cdInput.addEventListener('keydown', e => {
  if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) { e.preventDefault(); cdForm.requestSubmit(); }
});
cdInput.addEventListener('input', () => {
  cdInput.style.height = 'auto';
  cdInput.style.height = Math.min(cdInput.scrollHeight, 120) + 'px';
});

// ── Session events ────────────────────────────────────────────────────────
// Only a chat's own agent: its threads (a worktree cwd) and a bash pane are
// not the conversation the dock shows.
function cdIngest(info, seed = false) {
  if (!info || !CHAT_DOCK_RE.test(info.sandbox || '')) return;
  if ((info.mode || 'claude') !== 'claude' || threadOfCwd(info.cwd)) return;
  const c = cdChat(info.sandbox);
  const prev = c.state;
  c.state = info.state === 'exited' ? 'idle' : info.state;
  c.activity = info.activity || null;
  c.steps = info.steps || (info.question ? [info.question] : null);
  c.answerable = !!info.answerable;

  if (seed) {
    // A tab with no transcript of its own starts from what the daemon knows:
    // the last prompt and, once the turn ended, its answer.
    if (!c.log.length && info.last_input) {
      c.log.push({ who: 'me', text: info.last_input });
      if (info.reply) c.log.push({ who: 'ai', text: info.reply });
    }
  } else if (info.state === 'idle' && (prev === 'working' || prev === 'attention') && info.reply) {
    const last = c.log[c.log.length - 1];
    if (!(last?.who === 'ai' && last.text === info.reply)) c.log.push({ who: 'ai', text: info.reply });
    if (!cdOpen || cdActive !== info.sandbox) cdNotify(info.sandbox, info.reply);
  } else if (info.state === 'attention' && prev !== 'attention' && !cdOpen) {
    cdNotify(info.sandbox, c.steps?.[0]?.text || info.activity || 'Claude is waiting for you.');
  }
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
cdPixels.forEach(p => p.el.classList.add('lit'));
fetch('/api/sessions')
  .then(r => r.json())
  .then(list => list.forEach(i => cdIngest(i, true)))
  .catch(() => {})
  .finally(cdRender);
