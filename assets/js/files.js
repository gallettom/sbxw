// ── Generated files ("artifacts") ─────────────────────────────────────────
const filesOverlay = document.getElementById('files-modal-overlay');
const filesNameEl  = document.getElementById('files-modal-name');
const filesDirEl   = document.getElementById('files-modal-dir');
const filesTbody   = document.getElementById('files-modal-tbody');
let filesTarget = null;

function humanSize(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB'];
  let v = bytes / 1024, i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return `${v.toFixed(1)} ${units[i]}`;
}

function humanTime(unixSecs) {
  if (!unixSecs) return '—';
  return new Date(unixSecs * 1000).toLocaleString();
}

function renderFilesTable(entries) {
  if (!entries.length) {
    filesTbody.innerHTML = '<tr><td colspan="4" class="ports-empty-msg">No generated files yet</td></tr>';
    return;
  }
  filesTbody.innerHTML = entries.map(f => {
    const url = `/api/sandboxes/${encodeURIComponent(filesTarget)}/artifacts/download?path=${encodeURIComponent(f.path)}`;
    return `<tr>
      <td>${f.path}</td>
      <td>${humanSize(f.size)}</td>
      <td>${humanTime(f.modified)}</td>
      <td><a class="dl-link" href="${url}" download="${f.name}">⬇ Download</a></td>
    </tr>`;
  }).join('');
}

async function fetchArtifacts(name) {
  filesTbody.innerHTML = '<tr><td colspan="4" class="ports-empty-msg">Loading…</td></tr>';
  try {
    const res = await fetch(`/api/sandboxes/${encodeURIComponent(name)}/artifacts`);
    const data = await res.json();
    filesDirEl.textContent = data.dir || '.sbxw-artifacts';
    renderFilesTable(data.entries || []);
  } catch (_) {
    filesTbody.innerHTML = '<tr><td colspan="4" class="ports-empty-msg" style="color:#f85149">Error fetching files</td></tr>';
  }
}

function openFilesModal(name) {
  filesTarget = name;
  filesNameEl.textContent = name;
  filesOverlay.classList.remove('hidden');
  fetchArtifacts(name);
}

function closeFilesModal() { filesOverlay.classList.add('hidden'); filesTarget = null; }

document.getElementById('files-modal-close').addEventListener('click', closeFilesModal);
document.getElementById('files-modal-close2').addEventListener('click', closeFilesModal);
document.getElementById('files-modal-refresh').addEventListener('click', () => {
  if (filesTarget) fetchArtifacts(filesTarget);
});
filesOverlay.addEventListener('click', e => { if (e.target === filesOverlay) closeFilesModal(); });
document.addEventListener('keydown', e => {
  if (e.key === 'Escape' && !filesOverlay.classList.contains('hidden')) closeFilesModal();
});
