// ── Project panel ─────────────────────────────────────────────────────────
//
// What this repository has produced: its code map first, then the deliverables
// under `.sbxw-artifacts/`. Named after its subject rather than its mechanism —
// it used to be "Generated files", which described how the list is built rather
// than what anyone opens it for.
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

/// Enable the footer's map button for what this project actually has.
///
/// Disabled rather than hidden when there is no map: a button that vanishes
/// teaches nothing, while a disabled one with a tooltip says the map is a thing
/// this project could have and names the command that writes it.
function renderCodemapButton(map) {
  const btn = document.getElementById('files-modal-codemap');
  btn.disabled = !map;
  btn.title = map
    ? `Open the code map — ${map.files} file${map.files === 1 ? '' : 's'} under `
      + `.sbxw-artifacts/${map.dir}/, updated ${humanTime(map.modified)}`
    : 'No code map yet — run /codemap in this sandbox to write one';
}

async function fetchArtifacts(name) {
  filesTbody.innerHTML = '<tr><td colspan="4" class="ports-empty-msg">Loading…</td></tr>';
  try {
    const res = await fetch(`/api/sandboxes/${encodeURIComponent(name)}/artifacts`);
    const data = await res.json();
    filesDirEl.textContent = data.dir || '.sbxw-artifacts';
    renderFilesTable(data.entries || []);
    renderCodemapButton(data.codemap || null);
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

document.getElementById('files-modal-codemap').addEventListener('click', () => {
  if (!filesTarget) return;
  const sandbox = filesTarget;
  closeFilesModal();
  openCodemapModal(sandbox);
});

document.getElementById('files-modal-close').addEventListener('click', closeFilesModal);
document.getElementById('files-modal-close2').addEventListener('click', closeFilesModal);
document.getElementById('files-modal-refresh').addEventListener('click', () => {
  if (filesTarget) fetchArtifacts(filesTarget);
});
filesOverlay.addEventListener('click', e => { if (e.target === filesOverlay) closeFilesModal(); });
document.addEventListener('keydown', e => {
  if (e.key === 'Escape' && !filesOverlay.classList.contains('hidden')) closeFilesModal();
});
