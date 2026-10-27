// ── Create sandbox wizard ─────────────────────────────────────────────────
const overlay   = document.getElementById('modal-overlay');
const inpName   = document.getElementById('inp-name');
const fsList    = document.getElementById('fs-list');
const fsPath    = document.getElementById('fs-path');
const btnCreate = document.getElementById('modal-create');
const errEl     = document.getElementById('create-error');
const nameErrEl = document.getElementById('name-error');

const fsFavsEl  = document.getElementById('fs-favs');

let selectedPath = '';

// ── Favourite folders ─────────────────────────────────────────────────────
//
// Developers keep their projects under one or two roots, so the picker opening
// at $HOME every time costs the same three clicks to reach a folder they will
// pick again tomorrow. Starring a root turns that into one click and leaves the
// browser doing the part that genuinely differs — choosing the subfolder.
//
// The list lives on the host (`~/.sbxw/state/favourites.json`), not in this
// browser: it names directories on *that machine*, so it has to survive a
// different browser or a cleared profile. Every mutation answers with the whole
// list, so this never has to predict what the server made of it.
let favourites = [];

function isFavourite(path) {
  return favourites.some(f => f.path === path);
}

function renderFavourites() {
  fsFavsEl.hidden = favourites.length === 0;
  fsFavsEl.innerHTML = favourites.map(f => `
    <button type="button" class="fs-fav${f.missing ? ' missing' : ''}"
            data-fav-path="${escHtml(f.path)}"
            title="${escHtml(f.missing ? f.path + ' — not found right now' : f.path)}">
      <span class="fs-fav-star">★</span>${escHtml(f.name)}
    </button>`).join('');
}

async function loadFavourites() {
  try {
    const res = await fetch('/api/fs/favourites');
    favourites = (await res.json()).favourites || [];
  } catch (_) {
    favourites = [];
  }
  renderFavourites();
  renderFsList();
}

/**
 * Star or unstar `path`, from the row it belongs to.
 *
 * Starring is on the *rows* rather than on the path bar because you star a root
 * you are about to browse *into* — `~/dev`, not the project inside it. On the
 * path bar that meant entering a folder just to pin it, then going back out;
 * from the row it is one click on something already on screen.
 */
async function toggleFavourite(path) {
  const on = isFavourite(path);
  try {
    const data = await postJson('/api/fs/favourites', { path, favourite: !on });
    if (!data.ok) { showToast(data.error || 'Could not update favourites', 'error'); return; }
    favourites = data.favourites || [];
    renderFavourites();
    renderFsList();
  } catch (_) {
    showToast('Could not update favourites', 'error');
  }
}

// A chip jumps the browser into that folder — the subfolder list below is the
// point, so this browses *into* it rather than selecting it outright.
fsFavsEl.addEventListener('click', ev => {
  const path = ev.target.closest('[data-fav-path]')?.dataset.favPath;
  if (path) browseTo(path);
});

// ── Repos ─────────────────────────────────────────────────────────────────
//
// A workspace is often one repo of several (`~/dev/shop/{backend,frontend}`),
// and the agent needs the others beside it: to read a contract, or to change
// both sides at once. The first repo is the workspace — always read-write,
// threads put their worktrees in it; each other one is mounted beside it at
// the same path, read-only unless switched. Nothing added means the folder
// browsed to, alone, as before.
const repoRowsEl = document.getElementById('repo-rows');
const repoHintEl = document.getElementById('repo-hint');
const btnAddRepo = document.getElementById('btn-add-repo');
let repos = [];

function hasRepo(path) {
  return repos.some(r => r.path === path);
}

function toggleRepo(path) {
  if (hasRepo(path)) repos = repos.filter(r => r.path !== path);
  else repos.push({ path, readonly: true });
  renderRepos();
  renderFsList();
}

// sbx shares a directory once: a repo inside another is reached through the
// outer mount, with its mode — so an RO one inside the workspace is writable.
// The server refuses the pair; this says so before anyone presses Create.
function nestedRepos() {
  const inside = (a, b) => a.startsWith(b.endsWith('/') ? b : b + '/');
  for (const a of repos) for (const b of repos) {
    if (a !== b && inside(a.path, b.path)) return [a.path, b.path];
  }
  return null;
}

// The folder on screen is the sandbox's folder until extra ones are added;
// after that the first of those is, and this one is only where you are.
function paintPath() {
  if (!selectedPath) { fsPath.textContent = '—'; return; }
  const tag = repos.length ? 'Browsing' : 'Selected';
  fsPath.innerHTML = `<span class="fs-path-tag${repos.length ? ' browsing' : ''}">${tag}</span><bdi>${escHtml(selectedPath)}</bdi>`;
}

function renderRepos() {
  const nested = nestedRepos();
  repoHintEl.classList.toggle('error', !!nested);
  repoHintEl.textContent = nested
    ? `${nested[0]} is inside ${nested[1]}. Pick folders side by side, not one inside another.`
    : repos.length
    ? 'The agent works in the first one. The others sit beside it, at the same path.'
    : 'Other repos the agent can read, or change, next to the project folder.';
  repoRowsEl.innerHTML = repos.map((r, i) => {
    const mode = i === 0
      ? `<span class="repo-badge">workspace</span>
         <span class="repo-mode" title="The workspace is always read-write: threads write their worktrees into it">
           <button type="button" data-mode="rw" class="on" disabled>RW</button></span>`
      : `<button type="button" class="repo-main" data-main="${i}" title="Make this the workspace">⤒ workspace</button>
         <span class="repo-mode">
           <button type="button" data-mode="rw" data-i="${i}" class="${r.readonly ? '' : 'on'}" title="The agent may change it">RW</button><button
                   type="button" data-mode="ro" data-i="${i}" class="${r.readonly ? 'on' : ''}" title="The agent can only read it">RO</button></span>`;
    return `<div class="repo-row">
       <span class="repo-path" title="${escHtml(r.path)}"><bdi>${escHtml(r.path)}</bdi></span>
       ${mode}
       <button class="btn-rm-port" type="button" data-rm="${i}" title="Remove">✕</button>
     </div>`;
  }).join('');
  btnAddRepo.disabled = !selectedPath || hasRepo(selectedPath);
  paintPath();
  validateForm();
}

repoRowsEl.addEventListener('click', ev => {
  const b = ev.target.closest('button');
  if (!b || b.disabled) return;
  if (b.dataset.rm !== undefined) {
    repos.splice(Number(b.dataset.rm), 1);
  } else if (b.dataset.main !== undefined) {
    const [r] = repos.splice(Number(b.dataset.main), 1);
    repos.unshift(r);
  } else if (b.dataset.i !== undefined) {
    repos[Number(b.dataset.i)].readonly = b.dataset.mode === 'ro';
  } else {
    return;
  }
  renderRepos();
  renderFsList();
});

btnAddRepo.addEventListener('click', () => {
  if (selectedPath && !hasRepo(selectedPath)) toggleRepo(selectedPath);
});

const portRowsEl = document.getElementById('port-rows');
let portRowId = 0;

function addPortRow(sandboxPort = '', hostPort = '', alias = '') {
  const id = portRowId++;
  const row = document.createElement('div');
  row.className = 'port-row';
  row.dataset.id = id;
  row.innerHTML =
    `<input class="port-input" type="number" min="1" max="65535" placeholder="3000" value="${sandboxPort}" data-field="sbx">` +
    `<span class="port-arrow">→</span>` +
    `<input class="port-input" type="number" min="1" max="65535" placeholder="same" value="${hostPort}" data-field="host">` +
    `<input class="port-input" type="text" placeholder="app.local" value="${alias}" data-field="alias">` +
    `<button class="btn-rm-port" type="button" title="Remove">✕</button>`;
  row.querySelector('.btn-rm-port').addEventListener('click', () => row.remove());
  const sbxInput  = row.querySelector('[data-field="sbx"]');
  const hostInput = row.querySelector('[data-field="host"]');
  sbxInput.addEventListener('input', () => {
    if (!hostInput.value) hostInput.placeholder = sbxInput.value || 'same';
  });
  portRowsEl.appendChild(row);
}

function collectPorts() {
  return Array.from(portRowsEl.querySelectorAll('.port-row')).map(row => {
    const sbx   = parseInt(row.querySelector('[data-field="sbx"]').value, 10);
    const host  = parseInt(row.querySelector('[data-field="host"]').value, 10);
    const alias = row.querySelector('[data-field="alias"]').value.trim();
    if (!sbx) return null;
    const entry = { sandbox_port: sbx };
    if (host) entry.host_port = host;
    if (alias) entry.alias = alias;
    return entry;
  }).filter(Boolean);
}

document.getElementById('btn-add-port').addEventListener('click', () => addPortRow());

const btnBrowseNative = document.getElementById('btn-browse-native');
btnBrowseNative.addEventListener('click', async () => {
  btnBrowseNative.disabled = true;
  const prevLabel = btnBrowseNative.textContent;
  btnBrowseNative.textContent = 'Waiting…';
  try {
    const res = await fetch('/api/fs/pick', { method: 'POST' });
    const data = await res.json();
    if (data.ok) {
      await browseTo(data.path);
    } else if (data.error) {
      errEl.textContent = data.error;
      errEl.classList.remove('hidden');
    }
    // Cancelling the native dialog (data.cancelled) is a silent no-op.
  } catch (e) {
    showNote(errEl, 'Failed to open native folder picker');
  } finally {
    btnBrowseNative.disabled = false;
    btnBrowseNative.textContent = prevLabel;
  }
});

function openModal() {
  overlay.classList.remove('hidden');
  inpName.value = '';
  inpName.classList.remove('error');
  nameErrEl.textContent = '';
  nameErrEl.classList.add('hidden');
  errEl.textContent = '';
  errEl.classList.add('hidden');
  btnCreate.disabled = true;
  selectedPath = '';
  fsPath.textContent = '—';
  portRowsEl.innerHTML = '';
  repos = [];
  renderRepos();
  // Re-read every time the modal opens: another tab (or a hand-edited state
  // file) may have changed the list since this one was last used.
  loadFavourites();
  browseTo(null);
  showTemplateForce();
  showKitMounts();
  document.getElementById('rc-box').checked = false;
  inpName.focus();
}

// The "rebuild" box, offered only when the automatic template is up to date:
// otherwise Create asks anyway (askTemplate).
const tplForce = document.getElementById('tpl-force');
const tplForceBox = document.getElementById('tpl-force-box');

async function showTemplateForce() {
  tplForceBox.checked = false;
  tplForce.classList.add('hidden');
  let status;
  try { status = await (await fetch('/api/template/status')).json(); } catch (_) { return; }
  const t = status && status.auto && status.template;
  if (!t || t.state !== 'fresh') return;
  const note = document.getElementById('tpl-force-note');
  note.textContent = 'Save this sandbox as the new starting point. Takes several extra minutes.';
  note.title = `Replaces ${t.record.tag}. The sandbox opens once its kits have installed and it is saved.`;
  tplForce.classList.remove('hidden');
}

// The host mounts the configured kits declare, one row each: a folder typed or
// picked here is passed as `--mount HOST:TARGET`; one left empty keeps what
// `kit_mounts` says, or sbx's own folder for the kit.
const kmRows = document.getElementById('km-rows');

async function showKitMounts() {
  const opt = document.getElementById('km-option');
  opt.classList.add('hidden');
  kmRows.innerHTML = '';
  let data;
  try { data = await (await fetch('/api/kit-mounts')).json(); } catch (_) { return; }
  const mounts = (data && data.mounts) || [];
  if (!mounts.length) return;
  kmRows.innerHTML = mounts.map(m => `<div class="km-row">
      <code class="km-target" title="Declared by ${escHtml(m.kit)}">${escHtml(m.target)}</code>
      <input class="port-input km-host" data-target="${escHtml(m.target)}" value="${escHtml(m.host || '')}"
             placeholder="sbx's own folder" spellcheck="false">
      <button class="btn-add-port km-pick" type="button">Choose…</button>
    </div>`).join('');
  opt.classList.remove('hidden');
}

kmRows.addEventListener('click', async e => {
  const btn = e.target.closest('.km-pick');
  if (!btn) return;
  btn.disabled = true;
  try {
    const data = await (await fetch('/api/fs/pick', { method: 'POST' })).json();
    if (data.ok) btn.parentElement.querySelector('.km-host').value = data.path;
    else if (data.error) showNote(errEl, data.error);
  } catch (_) {
    showNote(errEl, 'Failed to open native folder picker');
  } finally {
    btn.disabled = false;
  }
});

function collectKitMounts() {
  const out = {};
  for (const inp of kmRows.querySelectorAll('.km-host')) {
    if (inp.value.trim()) out[inp.dataset.target] = inp.value.trim();
  }
  return out;
}

function closeModal() { overlay.classList.add('hidden'); }

// The listing currently on screen. Kept so starring a row can repaint the list
// from memory: the folders haven't changed, only which of them are starred, and
// re-reading the directory to learn that would flicker the list under the click.
let fsListing = null;

function renderFsList() {
  if (!fsListing) return;
  let html = '';
  if (fsListing.parent) {
    html += `<div class="fs-item up" data-path="${escHtml(fsListing.parent)}">
               <span class="fs-icon">↑</span> ..
             </div>`;
  }
  if (fsListing.entries.length === 0 && !fsListing.parent) {
    html += '<div class="fs-empty">No subdirectories</div>';
  }
  // The star is a button *inside* the row: the row itself browses, the star
  // pins. Rendered on every row rather than on hover only — a control nobody
  // can see is a feature nobody finds — but dimmed until starred or hovered.
  html += fsListing.entries.map(e => {
    const on = isFavourite(e.path);
    const added = hasRepo(e.path);
    return `<div class="fs-item" data-path="${escHtml(e.path)}">
       <span class="fs-icon">📁</span>
       <span class="fs-name">${escHtml(e.name)}</span>
       <button type="button" class="fs-row-add${added ? ' on' : ''}"
               data-add-path="${escHtml(e.path)}" aria-pressed="${added}"
               title="${added ? 'Remove from this sandbox' : 'Add to this sandbox’s repos'}"
       >${added ? '✓' : '＋'}</button>
       <button type="button" class="fs-row-star${on ? ' on' : ''}"
               data-star-path="${escHtml(e.path)}" aria-pressed="${on}"
               title="${on ? 'Unstar this folder' : 'Star this folder — pins it as a shortcut above'}"
       >${on ? '★' : '☆'}</button>
     </div>`;
  }).join('');

  fsList.innerHTML = html || '<div class="fs-empty">Empty directory</div>';
}

// Which navigation is the current one. Clicking from one favourite to another
// starts a second read before the first has answered, and the responses can
// come back in either order — without this, the slower one wins and you land
// somewhere you didn't click last.
let browseSeq = 0;

/// How long a read may take before the list says anything about it. A local
/// directory answers in a millisecond or two, so showing a state for that is
/// pure flicker; a slow mount is where the feedback is actually wanted.
const FS_SLOW_MS = 200;

async function browseTo(path) {
  const seq = ++browseSeq;
  // Deliberately *not* blanking the list: replacing it with a one-line
  // "Loading…" collapsed the box from its full height to a single row and back
  // on every navigation, which the modal followed — the jump you see switching
  // between two favourites. The folders already on screen stay put until the
  // new ones are ready to take their place.
  const slow = setTimeout(() => {
    if (seq === browseSeq) fsList.classList.add('loading');
  }, FS_SLOW_MS);
  const url = path ? `/api/fs?path=${encodeURIComponent(path)}` : '/api/fs';
  try {
    const res = await fetch(url);
    const data = await res.json();
    if (seq !== browseSeq) return; // a later click already won
    selectedPath = data.path;
    renderRepos();
    fsListing = data;
    renderFsList();
  } catch (e) {
    if (seq !== browseSeq) return;
    fsListing = null;
    fsList.innerHTML = '<div class="fs-empty">Error loading directory</div>';
  } finally {
    clearTimeout(slow);
    if (seq === browseSeq) fsList.classList.remove('loading');
  }
}

// One listener for the whole list rather than one per row, so a repaint after
// starring doesn't have to re-bind anything. The star wins over the row: a
// click on it must pin the folder, not walk into it.
fsList.addEventListener('click', ev => {
  const add = ev.target.closest('[data-add-path]');
  if (add) {
    ev.stopPropagation();
    toggleRepo(add.dataset.addPath);
    return;
  }
  const star = ev.target.closest('[data-star-path]');
  if (star) {
    ev.stopPropagation();
    toggleFavourite(star.dataset.starPath);
    return;
  }
  const row = ev.target.closest('.fs-item[data-path]');
  if (row) browseTo(row.dataset.path);
});

function validateForm() {
  const name = inpName.value.trim();
  // Creating under a taken name would reuse that sandbox, its folders and all
  // — never what this dialog's repos asked for.
  const taken = sandboxes.some(s => s.name === name);
  if (taken) showNote(nameErrEl, `“${name}” already exists — pick another name`);
  const nameOk = !taken && /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(name);
  const pathOk = repos.length > 0 || selectedPath.length > 0;
  btnCreate.disabled = !(nameOk && pathOk) || !!nestedRepos();
}

function normalizeNameInput() {
  const el = inpName;
  const before = el.value;
  const start = el.selectionStart;
  const hadInvalidChars = /[^a-zA-Z0-9-]/.test(before) || before.startsWith('-');
  const cleaned = before.toLowerCase().replace(/[^a-z0-9-]/g, '').replace(/^-+/, '');
  if (cleaned !== before) {
    el.value = cleaned;
    const pos = Math.min(start, cleaned.length);
    el.setSelectionRange(pos, pos);
  }
  nameErrEl.textContent = hadInvalidChars
    ? 'Lowercase letters, digits, and hyphens only — must start and end with a letter or digit'
    : '';
  nameErrEl.classList.toggle('hidden', !hadInvalidChars);
  inpName.classList.remove('error');
  validateForm();
}

inpName.addEventListener('input', normalizeNameInput);

// ── Which kind of sandbox? ────────────────────────────────────────────────
// The sidebar's ＋ is the single way in: it opens the chooser, and the chooser
// opens one of the two wizards. Two buttons sitting side by side made the
// choice before it was explained — an emoji is a poor place to learn that one
// of them mounts your code and the other deliberately does not.
const kindOverlay = document.getElementById('kind-modal-overlay');

function openKindModal() { kindOverlay.classList.remove('hidden'); }
function closeKindModal() { kindOverlay.classList.add('hidden'); }

document.getElementById('btn-new-sandbox').addEventListener('click', openKindModal);
document.getElementById('kind-modal-close').addEventListener('click', closeKindModal);
kindOverlay.addEventListener('click', e => { if (e.target === kindOverlay) closeKindModal(); });
document.addEventListener('keydown', e => {
  if (e.key === 'Escape' && !kindOverlay.classList.contains('hidden')) closeKindModal();
});

// Alt+N (⌃⌥N on a Mac, see isAppShortcut) opens the chooser from anywhere —
// Ctrl/⌘+N belongs to the browser and cannot be taken. Listened for on window in the capture phase because xterm
// stops the keydowns it handles, and the point is to work from a terminal pane.
// Not while another dialog is open: one modal stacked on another is the bug.
window.addEventListener('keydown', e => {
  if (!isAppShortcut(e, 'KeyN')) return;
  if (document.querySelector('.modal-overlay:not(.hidden)')) return;
  e.preventDefault();
  e.stopPropagation();
  openKindModal();
}, true);

// Each card hands straight over to that kind's own modal — the chooser is a
// signpost, not a step you have to confirm your way out of.
document.getElementById('kind-workspace').addEventListener('click', () => {
  closeKindModal();
  openModal();
});
document.getElementById('kind-chat').addEventListener('click', () => {
  closeKindModal();
  openChatModal();
});

// …and back the other way, from either wizard.
document.getElementById('modal-back').addEventListener('click', () => {
  closeModal();
  openKindModal();
});
document.getElementById('chat-modal-back').addEventListener('click', () => {
  closeChatModal();
  openKindModal();
});

// ── Host monitor ──────────────────────────────────────────────────────────
// One PTY on the host running `monitor_cmd`, shared by every viewer — not a
// sandbox session, and deliberately not a general "run something on the host"
// pane: the daemon binds to localhost, and one fixed configured command is a
// far smaller thing to expose than a shell.
//
// The button sits at the foot of the sidebar, under the list: it shows every
// sandbox at once, which is the whole of what that column is about — and it
// stays put there while the list above it scrolls.
//
// It opens in a modal nearly the size of the window, not in a pane: a TUI
// listing every sandbox is unreadable in a quarter of the grid, and the panes
// stay as they were underneath. The modal's terminal is built by the same
// setupTerminal as a pane's, so copy, OSC 52 and fitting behave the same.
const btnMonitor = document.getElementById('btn-monitor');
const monitorOverlay = document.getElementById('monitor-overlay');
const monitorDot = document.getElementById('monitor-dot');
let monitorPane = null;

function openMonitor() {
  monitorOverlay.classList.remove('hidden');
  if (!monitorPane) {
    monitorPane = {
      index: 'monitor', termEl: document.getElementById('monitor-term'),
      ws: null, sandbox: MONITOR_SANDBOX, lastSelection: '',
      getLastSelection: () => monitorPane.lastSelection,
    };
    setupTerminal(monitorPane);
  }
  const pane = monitorPane;
  if (pane.ws) { pane.term.focus(); return; }
  // The PTY outlives the modal; a fresh socket replays its screen.
  pane.term.reset();
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const ws = new WebSocket(`${proto}://${location.host}/ws?sandbox=${encodeURIComponent(MONITOR_SANDBOX)}&mode=monitor`);
  ws.binaryType = 'arraybuffer';
  ws.onopen = () => {
    monitorDot.className = 'dot term-connected';
    fitPane(pane, 'socket open');
    pane.term.focus();
  };
  ws.onmessage = ev => handlePaneData(pane, ev);
  ws.onclose = () => {
    monitorDot.className = 'dot term-disconnected';
    if (pane.ws === ws) pane.ws = null;
  };
  pane.ws = ws;
}

function closeMonitor() {
  monitorOverlay.classList.add('hidden');
  if (monitorPane?.ws) { try { monitorPane.ws.close(); } catch (_) {} monitorPane.ws = null; }
  monitorDot.className = 'dot term-disconnected';
  panes[focusedPane]?.term.focus();
}

if (MONITOR_CMD) {
  btnMonitor.hidden = false;
  btnMonitor.title = `Host monitor — runs \`${MONITOR_CMD}\` in a large window`;
  document.getElementById('monitor-cmd').textContent = MONITOR_CMD;
  btnMonitor.addEventListener('click', () =>
    monitorOverlay.classList.contains('hidden') ? openMonitor() : closeMonitor());
  document.getElementById('monitor-close').addEventListener('click', closeMonitor);
  monitorOverlay.addEventListener('mousedown', e => { if (e.target === monitorOverlay) closeMonitor(); });
}

// ── Chat sandbox: an agent on an empty temp folder (no code access) ────────
// Spins up a throwaway sandbox whose workspace is a fresh temp dir, so the
// agent has nothing to read or edit — a pure chat. The name is optional: the
// modal pre-computes a `chat-xxxxxx` one and uses it when the field is left
// empty, so the pending card always matches the sandbox the server creates.
// Provisioned in the background like a normal create (corner card +
// auto-attach on success).
const chatOverlay   = document.getElementById('chat-modal-overlay');
const chatInpName   = document.getElementById('chat-inp-name');
const chatNameErr   = document.getElementById('chat-name-error');
const chatSuggestEl = document.getElementById('chat-name-suggestion');
const chatConfirm   = document.getElementById('chat-modal-confirm');
let chatSuggestion = '';

function openChatModal() {
  chatSuggestion = 'chat-' + Math.random().toString(16).slice(2, 8);
  chatSuggestEl.textContent = chatSuggestion;
  chatInpName.value = '';
  chatInpName.placeholder = chatSuggestion;
  chatInpName.classList.remove('error');
  chatNameErr.textContent = '';
  chatNameErr.classList.add('hidden');
  chatOverlay.classList.remove('hidden');
  validateChatForm();
  chatInpName.focus();
}

function closeChatModal() { chatOverlay.classList.add('hidden'); }

// Empty is valid (the suggestion is used); a typed name has to be well-formed
// and free — provisioning an existing name would adopt that sandbox instead of
// starting a chat.
function validateChatForm() {
  const name = chatInpName.value.trim();
  if (!name) { chatNameErr.classList.add('hidden'); chatConfirm.disabled = false; return; }
  const taken = sandboxes.some(s => s.name === name);
  if (taken) {
    showNote(chatNameErr, `“${name}” already exists — pick another name`);
  }
  chatConfirm.disabled = taken || !/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(name);
}

chatInpName.addEventListener('input', () => {
  const el = chatInpName;
  const before = el.value;
  const start = el.selectionStart;
  const hadInvalidChars = /[^a-zA-Z0-9-]/.test(before) || before.startsWith('-');
  const cleaned = before.toLowerCase().replace(/[^a-z0-9-]/g, '').replace(/^-+/, '');
  if (cleaned !== before) {
    el.value = cleaned;
    const pos = Math.min(start, cleaned.length);
    el.setSelectionRange(pos, pos);
  }
  chatNameErr.textContent = hadInvalidChars
    ? 'Lowercase letters, digits, and hyphens only — must start and end with a letter or digit'
    : '';
  chatNameErr.classList.toggle('hidden', !hadInvalidChars);
  chatInpName.classList.remove('error');
  validateChatForm();
});

chatInpName.addEventListener('keydown', e => {
  if (e.key === 'Enter' && !chatConfirm.disabled) chatConfirm.click();
});

document.getElementById('chat-modal-close').addEventListener('click', closeChatModal);
document.getElementById('chat-modal-cancel').addEventListener('click', closeChatModal);
chatOverlay.addEventListener('click', e => { if (e.target === chatOverlay) closeChatModal(); });
document.addEventListener('keydown', e => {
  if (e.key === 'Escape' && !chatOverlay.classList.contains('hidden')) closeChatModal();
});

chatConfirm.addEventListener('click', () => {
  if (chatConfirm.disabled) return;
  const name = chatInpName.value.trim() || chatSuggestion;
  closeChatModal();
  addPendingSandbox(name);
  (async () => {
    try {
      const data = await postJson('/api/sandboxes/chat', { name });
      if (data.ok) {
        attachToEmptyPaneOrMarkReady(data.name || name);
        await loadSandboxes();
        showToast(`Chat sandbox “${data.name || name}” is ready`, 'ok');
      } else {
        showToast(`Failed to start chat: ${data.error || 'unknown error'}`, 'error');
      }
    } catch (e) {
      showToast('Failed to start chat: network error', 'error');
    } finally {
      removePendingSandbox(name);
    }
  })();
});
document.getElementById('modal-close').addEventListener('click', closeModal);
document.getElementById('modal-cancel').addEventListener('click', closeModal);

overlay.addEventListener('click', e => { if (e.target === overlay) closeModal(); });
document.addEventListener('keydown', e => {
  if (e.key === 'Escape' && !overlay.classList.contains('hidden')) closeModal();
});

// ── The automatic template ────────────────────────────────────────────────
// With `template = "auto"`, a create first asks whether the template for these
// kits is still the one this sbx and sbxw would make. If it is, nothing is
// asked. If it was saved by another version — or never — the person chooses:
// rebuild it from the sandbox being created (which then opens only once its
// kits have finished installing and it is saved), use the old one, or go
// without. Resolves to the choice, or null when the dialog is closed.
const tplOverlay = document.getElementById('tpl-modal-overlay');

async function askTemplate() {
  let status;
  try { status = await (await fetch('/api/template/status')).json(); }
  catch (_) { return 'default'; }
  if (!status || !status.auto) return 'default';
  const t = status.template || {};
  if (t.state === 'fresh') return 'default';

  const cur = status.current || {};
  const rec = t.record;
  const row = (label, was, now) =>
    `<tr><td>${label}</td><td>${escHtml(was)}</td><td>→</td>` +
    `<td class="${was === now ? '' : 'changed'}">${escHtml(now)}</td></tr>`;
  document.getElementById('tpl-modal-title').textContent = rec ? 'Template out of date' : 'No template yet';
  // What made it stale: sbx itself, or the kits' specs (an sbxw release that
  // leaves the kits alone does not count).
  const changed = (t.changed || []).filter(c => !c.startsWith('sbx '));
  document.getElementById('tpl-versions').innerHTML = rec
    ? `<tr><td>Template</td><td class="tpl-tag" colspan="3">${escHtml(rec.tag)}</td></tr>` +
      row('sbx', rec.sbx, cur.sbx) +
      changed.map(c => `<tr><td>Kits</td><td class="changed" colspan="3">${escHtml(c)}</td></tr>`).join('')
    : `<tr><td>sbx</td><td>${escHtml(cur.sbx)}</td></tr><tr><td>sbxw</td><td>${escHtml(cur.sbxw)}</td></tr>`;
  document.getElementById('tpl-explain').textContent =
    (rec ? 'What it holds was made by another sbx, or from kits that have changed since. ' : 'No template has been saved for these kits. ') +
    'Rebuild creates this sandbox from sbx\'s image, waits for its kits to finish installing, and saves it as the template ' +
    '(its token and credentials taken out) before it opens — several minutes this once. ' +
    (rec ? 'The old template is removed once the new one is saved.' : 'Later sandboxes start from it.');
  document.getElementById('tpl-keep').classList.toggle('hidden', !rec);
  tplOverlay.classList.remove('hidden');
  document.getElementById('tpl-rebuild').focus();

  return new Promise(resolve => {
    const done = choice => {
      tplOverlay.classList.add('hidden');
      for (const [id, fn] of handlers) document.getElementById(id).removeEventListener('click', fn);
      document.removeEventListener('keydown', onKey, true);
      resolve(choice);
    };
    const handlers = [
      ['tpl-rebuild', () => done('rebuild')],
      ['tpl-keep', () => done('keep')],
      ['tpl-skip', () => done('skip')],
      ['tpl-modal-close', () => done(null)],
    ];
    // Escape closes this dialog only, not the create dialog under it.
    const onKey = e => { if (e.key === 'Escape') { e.stopImmediatePropagation(); done(null); } };
    for (const [id, fn] of handlers) document.getElementById(id).addEventListener('click', fn);
    document.addEventListener('keydown', onKey, true);
  });
}

// Provisioning (network policy, kits, ports…) can take a while, so this
// doesn't block the UI: the modal closes right away and the request runs
// in the background behind a corner card reporting its steps, leaving every
// other sandbox/pane fully interactive in the meantime.
document.getElementById('modal-create').addEventListener('click', async () => {
  const name = inpName.value.trim();
  if (!name || btnCreate.disabled) { inpName.classList.add('error'); return; }

  const path   = repos.length ? repos[0].path : selectedPath;
  const mounts = repos.slice(1).map(r => ({ path: r.path, readonly: r.readonly }));
  const ports  = collectPorts();
  const kit_mounts = collectKitMounts();
  // Asked over the create dialog, which stays open if this one is closed —
  // unless the person already asked for a rebuild with the box.
  const forced = !tplForce.classList.contains('hidden') && tplForceBox.checked;
  const remote_control = document.getElementById('rc-box').checked;
  const template = forced ? 'rebuild' : await askTemplate();
  if (template === null) return;
  closeModal();

  (async () => {
    // Shown as a corner card, where every other job running behind you is —
    // the sandbox list is for sandboxes that exist, and this one does not yet.
    addPendingSandbox(name);
    try {
      const data = await postJson('/api/sandboxes/create', { name, path, mounts, ports, template, remote_control, kit_mounts });
      if (data.ok) {
        // Attaches to an empty pane if one is sitting idle; otherwise just
        // flags it ready in the list with a blue dot for a manual connect.
        attachToEmptyPaneOrMarkReady(name);
        await loadSandboxes();
        // The sandbox is up even when something beside it did not work — an
        // /etc/hosts alias needing a password nobody could type is the usual
        // one. The toast holds a single message, so the warning replaces the
        // success line rather than being pushed off by it; the last warning is
        // the one that says what to run.
        const warnings = Array.isArray(data.warnings) ? data.warnings : [];
        showToast(
          warnings.length
            ? `Sandbox “${name}” is ready — ⚠ ${warnings[warnings.length - 1]}`
            : remote_control
              ? `Sandbox “${name}” is ready — log in with /login in its pane, then press Remote`
              : `Sandbox “${name}” is ready`,
          warnings.length ? 'error' : 'ok');
      } else {
        showToast(`Failed to create “${name}”: ${data.error || 'unknown error'}`, 'error');
      }
    } catch (e) {
      showToast(`Failed to create “${name}”: network error`, 'error');
    } finally {
      removePendingSandbox(name);
    }
  })();
});
