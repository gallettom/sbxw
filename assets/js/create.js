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
    const res = await fetch('/api/fs/favourites', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path, favourite: !on }),
    });
    const data = await res.json();
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
    errEl.textContent = 'Failed to open native folder picker';
    errEl.classList.remove('hidden');
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
  // Re-read every time the modal opens: another tab (or a hand-edited state
  // file) may have changed the list since this one was last used.
  loadFavourites();
  browseTo(null);
  inpName.focus();
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
    return `<div class="fs-item" data-path="${escHtml(e.path)}">
       <span class="fs-icon">📁</span>
       <span class="fs-name">${escHtml(e.name)}</span>
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
    fsPath.textContent = data.path;
    validateForm();
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
  const nameOk = /^[a-z0-9][a-z0-9-]*$/.test(inpName.value.trim());
  const pathOk = selectedPath.length > 0;
  btnCreate.disabled = !(nameOk && pathOk);
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
    ? 'Lowercase letters, digits, and hyphens only — must start with a letter or digit'
    : '';
  nameErrEl.classList.toggle('hidden', !hadInvalidChars);
  inpName.classList.remove('error');
  validateForm();
}

inpName.addEventListener('input', normalizeNameInput);

document.getElementById('btn-new-sandbox').addEventListener('click', openModal);

// ── Host monitor ──────────────────────────────────────────────────────────
// One PTY on the host running `monitor_cmd`, shared by every viewer — not a
// sandbox session, and deliberately not a general "run something on the host"
// pane: the daemon binds to localhost, and one fixed configured command is a
// far smaller thing to expose than a shell.
const btnMonitor = document.getElementById('btn-monitor');
if (MONITOR_CMD) {
  btnMonitor.hidden = false;
  btnMonitor.title = `Host monitor — runs \`${MONITOR_CMD}\` in the focused pane`;
  btnMonitor.addEventListener('click', () => {
    // A toggle: from the monitor it puts the pane back on the sandbox it took
    // over, which is the way out most people reach for first.
    const pane = panes[focusedPane];
    const back = pane?.mode === 'monitor' && pane.beforeMonitor;
    if (back) connectPane(focusedPane, back.sandbox, back.mode);
    else connectPane(focusedPane, MONITOR_SANDBOX, 'monitor');
  });
}

// ── Chat sandbox: an agent on an empty temp folder (no code access) ────────
// Spins up a throwaway sandbox whose workspace is a fresh temp dir, so the
// agent has nothing to read or edit — a pure chat. The name is optional: the
// modal pre-computes a `chat-xxxxxx` one and uses it when the field is left
// empty, so the pending row always matches the sandbox the server creates.
// Provisioned in the background like a normal create (pending row +
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
    chatNameErr.textContent = `“${name}” already exists — pick another name`;
    chatNameErr.classList.remove('hidden');
  }
  chatConfirm.disabled = taken || !/^[a-z0-9][a-z0-9-]*$/.test(name);
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
    ? 'Lowercase letters, digits, and hyphens only — must start with a letter or digit'
    : '';
  chatNameErr.classList.toggle('hidden', !hadInvalidChars);
  chatInpName.classList.remove('error');
  validateChatForm();
});

chatInpName.addEventListener('keydown', e => {
  if (e.key === 'Enter' && !chatConfirm.disabled) chatConfirm.click();
});

document.getElementById('btn-new-chat').addEventListener('click', openChatModal);
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
      const res = await fetch('/api/sandboxes/chat', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name }),
      });
      const data = await res.json();
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

// Provisioning (network policy, kits, ports…) can take a while, so this
// doesn't block the UI: the modal closes right away and the request runs
// in the background behind a corner indicator, leaving every other
// sandbox/pane fully interactive in the meantime.
document.getElementById('modal-create').addEventListener('click', () => {
  const name = inpName.value.trim();
  if (!name || btnCreate.disabled) { inpName.classList.add('error'); return; }

  const path  = selectedPath;
  const ports = collectPorts();
  closeModal();

  (async () => {
    // Shown as the last row of the sandbox list, not a corner toast — this
    // is the one operation whose outcome literally *is* a new list entry.
    addPendingSandbox(name);
    try {
      const res = await fetch('/api/sandboxes/create', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name, path, ports }),
      });
      const data = await res.json();
      if (data.ok) {
        // Attaches to an empty pane if one is sitting idle; otherwise just
        // flags it ready in the list with a blue dot for a manual connect.
        attachToEmptyPaneOrMarkReady(name);
        await loadSandboxes();
        showToast(`Sandbox “${name}” is ready`, 'ok');
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
