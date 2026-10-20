// ── Upload ────────────────────────────────────────────────────────────────
//
// The drop box above Skills. A file dropped on it (or picked through it) opens
// a modal asking which sandbox it is for; picking one copies the file in and
// types its path into that sandbox's Claude prompt — not submitted, so there is
// room to say what the file is for before Return.
//
// A sandbox no pane shows gets one first, the way a freshly created sandbox
// does. Its path then waits for the terminal to settle: typed into a TUI still
// drawing its first frame, it would be dropped (the rule typing.rs follows
// server-side, here measured on the pane's own output).
const uploadBox = document.getElementById('upload-box');
const uploadInput = document.getElementById('upload-input');
const uploadOverlay = document.getElementById('upload-modal-overlay');
const uploadList = document.getElementById('upload-sandboxes');
const uploadNote = document.getElementById('upload-note');
const uploadSendBtn = document.getElementById('upload-modal-send');
const uploadError = document.getElementById('upload-error');

// The daemon's own ceiling (UPLOAD_MAX in http/ui.rs), checked here so a large
// file is refused before it is sent rather than after.
const UPLOAD_MAX = 128 * 1024 * 1024;
// Silence after which a terminal counts as drawn: a cold TUI's first frame,
// and a warm one's redraw — the same two numbers as typing.rs.
const UPLOAD_SETTLE_COLD = 900;
const UPLOAD_SETTLE_WARM = 180;
// A stopped sandbox starts when its pane attaches, which can take a while.
const UPLOAD_READY_TIMEOUT = 90000;

let uploadFiles = [];

function setUploadError(msg) {
  uploadError.textContent = msg || '';
  uploadError.classList.toggle('hidden', !msg);
}

// Object URLs of the thumbnails on show, revoked when the modal closes.
let uploadThumbs = [];

function uploadFileHtml(f) {
  let thumb;
  if (f.type.startsWith('image/')) {
    const url = URL.createObjectURL(f);
    uploadThumbs.push(url);
    thumb = `<img src="${url}" alt="">`;
  } else {
    const ext = f.name.includes('.') ? f.name.split('.').pop().slice(0, 4).toLowerCase() : '';
    thumb = escHtml(ext ? '.' + ext : 'file');
  }
  return `<div class="upload-file">
      <span class="upload-thumb">${thumb}</span>
      <span class="upload-file-name" title="${escHtml(f.name)}">${escHtml(f.name)}
        <span class="upload-file-size">${humanSize(f.size)}</span></span>
    </div>`;
}

function openUploadModal(files) {
  uploadFiles = [...files];
  if (!uploadFiles.length) return;
  document.getElementById('upload-files').innerHTML = uploadFiles.map(uploadFileHtml).join('');
  const big = uploadFiles.find(f => f.size > UPLOAD_MAX);
  setUploadError(big ? `${big.name} is over ${humanSize(UPLOAD_MAX)}, the most one upload takes.` : '');
  renderUploadSandboxes(big);
  uploadOverlay.classList.remove('hidden');
  uploadList.querySelector('input:checked')?.focus();
}

function closeUploadModal() {
  uploadOverlay.classList.add('hidden');
  uploadFiles = [];
  uploadInput.value = '';
  uploadThumbs.forEach(u => URL.revokeObjectURL(u));
  uploadThumbs = [];
}

// What sending does to that sandbox, in the box's last line.
function uploadWhat(s, shown) {
  if (shown) return 'On screen';
  return s.status === 'running' ? 'Opens in a pane' : 'Starts, then opens';
}

// The button names its target, so the pick is confirmed where it is acted on.
function syncUploadSend(blocked) {
  const name = uploadList.querySelector('input:checked')?.value;
  uploadSendBtn.disabled = !!blocked || !name;
  uploadSendBtn.textContent = name ? `Send to ${name}` : 'Send';
}

// One box per sandbox in the sidebar's list, the focused pane's preselected:
// a file dropped while reading a session is most likely meant for it.
function renderUploadSandboxes(blocked) {
  const here = panes[focusedPane]?.sandbox;
  const pick = sandboxes.some(s => s.name === here) ? here : sandboxes[0]?.name;
  uploadList.innerHTML = sandboxes.map(s => {
    const shown = panes.slice(0, paneCount).some(p => p.sandbox === s.name);
    const cls = `upload-sbx ${dotClass(s.status)}${shown ? ' shown' : ''}`;
    return `<label class="${cls}">
        <input type="radio" name="upload-sandbox" value="${escHtml(s.name)}"${s.name === pick ? ' checked' : ''}>
        <span class="upload-sbx-check" aria-hidden="true"></span>
        <span class="upload-sbx-head"><span class="dot ${dotClass(s.status)}" title="${escHtml(s.status)}"></span>
          <span class="upload-sbx-name" title="${escHtml(s.name)}">${escHtml(s.name)}</span></span>
        ${s.agent ? `<span class="upload-sbx-agent">${escHtml(s.agent)}</span>` : ''}
        <span class="upload-sbx-what">${uploadWhat(s, shown)}</span>
      </label>`;
  }).join('');
  uploadNote.textContent = sandboxes.length ? '' : 'There is no sandbox to send to yet. Create one with ＋ at the top of the list.';
  uploadNote.hidden = !uploadNote.textContent;
  uploadList.onchange = () => syncUploadSend(blocked);
  syncUploadSend(blocked);
}

// The pane whose prompt the path goes into, attaching one when none shows the
// sandbox: the focused pane when it is already on it (agent or thread), else
// its agent's pane, else a fresh one — an empty pane, a new one when the
// layout has room, the focused one as a last resort. -1 when connectPane
// refused (that session is in another pane in a mode we did not look for).
function uploadPane(name) {
  const shown = panes.slice(0, paneCount);
  const focused = shown[focusedPane];
  if (focused?.sandbox === name && (focused.mode === 'claude' || focused.mode === 'thread')) return focusedPane;
  const agent = shown.findIndex(p => p.sandbox === name && p.mode === 'claude');
  if (agent >= 0) return agent;
  // Its shell is the agent's slot: turn that pane back to Claude.
  let idx = shown.findIndex(p => p.sandbox === name && p.mode === 'bash' && !p.thread);
  if (idx < 0) idx = shown.findIndex(p => !p.sandbox);
  if (idx < 0 && paneCount < computeMaxPanes()) { idx = paneCount; setLayout(paneCount + 1); }
  if (idx < 0) idx = focusedPane;
  connectPane(idx, name, 'claude');
  return panes[idx].sandbox === name && panes[idx].mode === 'claude' ? idx : -1;
}

// Resolves once the pane's socket is open and its output has been quiet for
// `quiet` ms. `cold` also waits for a first byte: a pane just attached has
// drawn nothing yet, and nothing is not the same as quiet. A warm pane whose
// agent is busy never goes quiet; it is typed into after a second anyway —
// Claude Code queues what is typed mid-turn.
function paneSettled(pane, name, cold) {
  const quiet = cold ? UPLOAD_SETTLE_COLD : UPLOAD_SETTLE_WARM;
  const t0 = Date.now();
  return new Promise((resolve, reject) => {
    const tick = () => {
      if (pane.sandbox !== name) return reject(new Error(`its pane moved off ${name}`));
      const open = pane.ws && pane.ws.readyState === WebSocket.OPEN;
      const drawn = pane.lastOutput > 0 || !cold;
      if (open && drawn && Date.now() - (pane.lastOutput || 0) >= quiet) return resolve();
      if (open && !cold && Date.now() - t0 > 1000) return resolve();
      if (Date.now() - t0 > UPLOAD_READY_TIMEOUT) return reject(new Error(`${name} did not come up within ${UPLOAD_READY_TIMEOUT / 1000} s`));
      setTimeout(tick, 100);
    };
    tick();
  });
}

async function uploadOne(name, file) {
  const res = await fetch(
    `/api/sandboxes/${encodeURIComponent(name)}/upload?name=${encodeURIComponent(file.name)}`,
    { method: 'POST', headers: { 'Content-Type': file.type || 'application/octet-stream' }, body: file }
  );
  const data = await res.json().catch(() => ({ error: `HTTP ${res.status}` }));
  if (!data.ok || !data.path) throw Object.assign(new Error(data.error || 'no path returned'), { status: res.status });
  return data.path;
}

// A sandbox starting from stopped can go quiet mid-boot, before `sbx exec`
// reaches it — which the daemon answers 502. Retried until it is up; every
// other refusal is the answer.
async function uploadWhenUp(pane, name, file, cold) {
  const t0 = Date.now();
  for (;;) {
    try {
      return await uploadOne(name, file);
    } catch (e) {
      if (!cold || e.status !== 502 || Date.now() - t0 > UPLOAD_READY_TIMEOUT) throw e;
      await new Promise(r => setTimeout(r, 2000));
      await paneSettled(pane, name, true);
    }
  }
}

async function sendUpload() {
  const name = uploadList.querySelector('input:checked')?.value;
  const files = uploadFiles;
  if (!name || !files.length) return;
  closeUploadModal();
  const cold = !panes.slice(0, paneCount).some(p => p.sandbox === name && p.ws?.readyState === WebSocket.OPEN);
  const idx = uploadPane(name);
  if (idx < 0) { showToast(`Could not open a pane for ${name}`, 'error'); return; }
  const pane = panes[idx];
  setFocusedPane(idx);
  showToast(cold ? `Opening ${name}…` : `Uploading to ${name}…`, 'progress');
  try {
    // The sandbox runs once its pane has drawn, and `sbx exec` needs it running.
    await paneSettled(pane, name, cold);
    if (cold) showToast(`Uploading to ${name}…`, 'progress');
    const paths = [];
    for (const f of files) paths.push(await uploadWhenUp(pane, name, f, cold));
    await paneSettled(pane, name, false);
    if (pane.ws?.readyState !== WebSocket.OPEN) throw new Error(`${name}'s terminal disconnected`);
    // Trailing space, no Return: the cursor waits after the path for context.
    pane.ws.send(new TextEncoder().encode(paths.join(' ') + ' '));
    pane.term.focus();
    showToast('📎 ' + paths.join('  '), 'ok');
  } catch (e) {
    showToast(`Upload failed: ${e.message}`, 'error');
  }
}

uploadInput.addEventListener('change', () => openUploadModal(uploadInput.files));
// A counter, not a flag: dragenter/dragleave fire for every child crossed.
let uploadDrag = 0;
uploadBox.addEventListener('dragenter', e => { e.preventDefault(); uploadDrag++; uploadBox.classList.add('drag'); });
uploadBox.addEventListener('dragover', e => { e.preventDefault(); e.dataTransfer.dropEffect = 'copy'; });
uploadBox.addEventListener('dragleave', () => { if (--uploadDrag <= 0) { uploadDrag = 0; uploadBox.classList.remove('drag'); } });
uploadBox.addEventListener('drop', e => {
  e.preventDefault();
  uploadDrag = 0;
  uploadBox.classList.remove('drag');
  if (e.dataTransfer.files.length) openUploadModal(e.dataTransfer.files);
});

uploadSendBtn.addEventListener('click', sendUpload);
// A double-click on a box picks it and sends.
uploadList.addEventListener('dblclick', e => { if (e.target.closest('.upload-sbx') && !uploadSendBtn.disabled) sendUpload(); });
document.getElementById('upload-modal-close').addEventListener('click', closeUploadModal);
document.getElementById('upload-modal-cancel').addEventListener('click', closeUploadModal);
uploadOverlay.addEventListener('click', e => { if (e.target === uploadOverlay) closeUploadModal(); });
document.addEventListener('keydown', e => {
  if (uploadOverlay.classList.contains('hidden')) return;
  if (e.key === 'Escape') closeUploadModal();
  else if (e.key === 'Enter' && !uploadSendBtn.disabled) { e.preventDefault(); sendUpload(); }
});
