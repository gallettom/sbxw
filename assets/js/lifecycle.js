// ── Remove sandbox ────────────────────────────────────────────────────────
const rmOverlay  = document.getElementById('rm-modal-overlay');
const rmNameEl   = document.getElementById('rm-modal-name');
const rmConfirm  = document.getElementById('rm-modal-confirm');
let rmTarget = null;

function openRmModal(name) {
  rmTarget = name;
  rmNameEl.textContent = name;
  rmConfirm.disabled = false;
  rmConfirm.textContent = 'Remove';
  rmOverlay.classList.remove('hidden');
}

function closeRmModal() { rmOverlay.classList.add('hidden'); rmTarget = null; }

async function removeSandbox(name) {
  panes.forEach(p => {
    if (p.sandbox === name) {
      if (p.ws) { try { p.ws.close(); } catch(_){} p.ws = null; }
      p.sandbox = null;
    }
  });
  const s = sandboxes.find(x => x.name === name);
  if (s) s.status = 'removing…';
  renderSidebar();
  const res = await fetch(`/api/sandboxes/${encodeURIComponent(name)}/rm`, { method: 'POST' });
  const data = await res.json();
  if (!data.ok) alert(`Failed to remove '${name}': ${data.error}`);
  await loadSandboxes();
  // If pane 0 has no sandbox and there are other sandboxes, connect to first one.
  if (!panes[0]?.sandbox && sandboxes.length) connectPane(0, sandboxes[0].name);
}

document.getElementById('rm-modal-close').addEventListener('click', closeRmModal);
document.getElementById('rm-modal-cancel').addEventListener('click', closeRmModal);
rmOverlay.addEventListener('click', e => { if (e.target === rmOverlay) closeRmModal(); });
document.addEventListener('keydown', e => {
  if (e.key === 'Escape' && !rmOverlay.classList.contains('hidden')) closeRmModal();
});

rmConfirm.addEventListener('click', async () => {
  if (!rmTarget) return;
  const name = rmTarget;
  rmConfirm.disabled = true;
  rmConfirm.textContent = 'Removing…';
  closeRmModal();
  await removeSandbox(name);
});

// ── Duplicate sandbox wizard ─────────────────────────────────────────────
const dupOverlay  = document.getElementById('dup-modal-overlay');
const dupSourceEl = document.getElementById('dup-modal-source');
const dupInpName  = document.getElementById('dup-inp-name');
const dupNameErr  = document.getElementById('dup-name-error');
const dupErrEl    = document.getElementById('dup-error');
const dupConfirm  = document.getElementById('dup-modal-confirm');
let dupSource = null;

function suggestDupName(source) {
  const m = source.match(/^(.*)-copy(?:-(\d+))?$/);
  if (m) return `${m[1]}-copy-${(parseInt(m[2] || '1', 10) + 1)}`;
  return `${source}-copy`;
}

function openDupModal(name) {
  dupSource = name;
  dupSourceEl.textContent = name;
  dupInpName.value = suggestDupName(name);
  dupInpName.classList.remove('error');
  dupNameErr.textContent = '';
  dupNameErr.classList.add('hidden');
  dupErrEl.textContent = '';
  dupErrEl.classList.add('hidden');
  dupOverlay.classList.remove('hidden');
  validateDupForm();
  dupInpName.focus();
  dupInpName.select();
}

function closeDupModal() { dupOverlay.classList.add('hidden'); dupSource = null; }

function validateDupForm() {
  const nameOk = /^[a-z0-9][a-z0-9-]*$/.test(dupInpName.value.trim());
  dupConfirm.disabled = !nameOk;
}

dupInpName.addEventListener('input', () => {
  const el = dupInpName;
  const before = el.value;
  const start = el.selectionStart;
  const hadInvalidChars = /[^a-zA-Z0-9-]/.test(before) || before.startsWith('-');
  const cleaned = before.toLowerCase().replace(/[^a-z0-9-]/g, '').replace(/^-+/, '');
  if (cleaned !== before) {
    el.value = cleaned;
    const pos = Math.min(start, cleaned.length);
    el.setSelectionRange(pos, pos);
  }
  dupNameErr.textContent = hadInvalidChars
    ? 'Lowercase letters, digits, and hyphens only — must start with a letter or digit'
    : '';
  dupNameErr.classList.toggle('hidden', !hadInvalidChars);
  dupInpName.classList.remove('error');
  validateDupForm();
});

dupInpName.addEventListener('keydown', e => {
  if (e.key === 'Enter' && !dupConfirm.disabled) dupConfirm.click();
});

document.getElementById('dup-modal-close').addEventListener('click', closeDupModal);
document.getElementById('dup-modal-cancel').addEventListener('click', closeDupModal);
dupOverlay.addEventListener('click', e => { if (e.target === dupOverlay) closeDupModal(); });
document.addEventListener('keydown', e => {
  if (e.key === 'Escape' && !dupOverlay.classList.contains('hidden')) closeDupModal();
});

// Same fire-and-forget pattern as sandbox creation: close the modal right
// away and let the new row show up as "pending" while provisioning runs.
dupConfirm.addEventListener('click', () => {
  if (!dupSource || dupConfirm.disabled) return;
  const source  = dupSource;
  const newName = dupInpName.value.trim();
  closeDupModal();

  (async () => {
    addPendingSandbox(newName);
    try {
      const res = await fetch(`/api/sandboxes/${encodeURIComponent(source)}/duplicate`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ new_name: newName }),
      });
      const data = await res.json();
      if (data.ok) {
        attachToEmptyPaneOrMarkReady(newName);
        await loadSandboxes();
        showToast(`Sandbox “${newName}” duplicated from “${source}”`, 'ok');
      } else {
        showToast(`Failed to duplicate “${source}”: ${data.error || 'unknown error'}`, 'error');
      }
    } catch (e) {
      showToast(`Failed to duplicate “${source}”: network error`, 'error');
    } finally {
      removePendingSandbox(newName);
    }
  })();
});
