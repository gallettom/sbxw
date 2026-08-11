// ── Create sandbox wizard ─────────────────────────────────────────────────
const overlay   = document.getElementById('modal-overlay');
const inpName   = document.getElementById('inp-name');
const fsList    = document.getElementById('fs-list');
const fsPath    = document.getElementById('fs-path');
const btnCreate = document.getElementById('modal-create');
const errEl     = document.getElementById('create-error');
const nameErrEl = document.getElementById('name-error');

let selectedPath = '';

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
  browseTo(null);
  inpName.focus();
}

function closeModal() { overlay.classList.add('hidden'); }

async function browseTo(path) {
  fsList.innerHTML = '<div class="fs-loading">Loading…</div>';
  const url = path ? `/api/fs?path=${encodeURIComponent(path)}` : '/api/fs';
  try {
    const res = await fetch(url);
    const data = await res.json();
    selectedPath = data.path;
    fsPath.textContent = data.path;
    validateForm();

    let html = '';
    if (data.parent) {
      html += `<div class="fs-item up" data-path="${data.parent}">
                 <span class="fs-icon">↑</span> ..
               </div>`;
    }
    if (data.entries.length === 0 && !data.parent) {
      html += '<div class="fs-empty">No subdirectories</div>';
    }
    html += data.entries.map(e =>
      `<div class="fs-item" data-path="${e.path}">
         <span class="fs-icon">📁</span>${e.name}
       </div>`
    ).join('');

    fsList.innerHTML = html || '<div class="fs-empty">Empty directory</div>';
    fsList.querySelectorAll('.fs-item[data-path]').forEach(el => {
      el.addEventListener('click', () => browseTo(el.dataset.path));
    });
  } catch (e) {
    fsList.innerHTML = '<div class="fs-empty">Error loading directory</div>';
  }
}

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
