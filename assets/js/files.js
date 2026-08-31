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
// What the last `/artifacts` fetch said this project has. Held rather than
// passed straight to the renderer because the button repaints on its own
// schedule too — a run starting or ending elsewhere changes what it should say
// without changing a single file (see `applyCodemapRun`).
let filesCodemap = { map: null, lenses: [], run: null };

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

/// Paint the footer's map button for what this project has, or is getting.
///
/// Four states, because the button answers four different questions:
///
///  - **Code map** — there is one; open it.
///  - **Writing the map…** — a background session is writing one right now
///    (see `applyCodemapRun`), so there is nothing to open and nothing to ask
///    for. Disabled, and the only state that is.
///  - **Writing a lens…** — same session, different document, and the map it is
///    written from is still there to read. So it says what is happening and
///    stays clickable: a lens being written is no reason to lock the map away.
///  - **Generate code map** — there is none. This used to be the disabled state
///    with a tooltip naming the command that writes one, which told you what
///    you were missing and left you to go and type it in a pane. A project
///    without a map is the case where the button has the most to do, not the
///    least.
///
/// A lens counts as something to open. A project whose map was deleted but
/// whose lenses survive still has documents in that panel, and a button that
/// refuses to open them because the *source* is gone would be hiding them.
function renderCodemapButton() {
  const btn = document.getElementById('files-modal-codemap');
  const { map, lenses } = filesCodemap;
  const n = (lenses || []).length;
  const run = filesTarget ? codemapRun(filesTarget) : null;
  const writing = run?.phase === 'writing';
  const lens = isLensRun(run);
  const readable = !!(map || n);

  btn.disabled = writing && !readable;
  btn.classList.toggle('working', writing);
  document.getElementById('files-modal-codemap-label').textContent =
    writing ? (lens ? 'Writing a lens…' : 'Writing the map…')
      : readable ? 'Code map' : 'Generate code map';

  const lensNote = n ? ` · ${n} lens${n === 1 ? '' : 'es'}` : '';
  btn.title = writing
    ? `${filesTarget} is writing ${lens ? 'a lens' : 'its map'} in a background session, started `
      + `${humanTime(Math.round(run.started / 1000))} — it takes a few minutes`
      + (lens && readable ? ' · the map is readable meanwhile' : '')
    : map
      ? `Open the code map — ${map.files} file${map.files === 1 ? '' : 's'} under `
        + `.sbxw-artifacts/${map.dir}/, updated ${humanTime(map.modified)}${lensNote}`
      : n
        ? `Open ${n} lens${n === 1 ? '' : 'es'} under .sbxw-artifacts/codemap-lenses/ `
          + '— the map they were written from is gone'
        : run?.phase === 'failed'
          ? `No code map yet — the last attempt ended: ${run.note || 'without one'}. `
            + 'Click to try again.'
          : 'No code map yet — write one with this sandbox\'s agent, in the background';
}

/// Ask this sandbox to write its map, and let the daemon paint the rest.
///
/// Nothing is drawn from the response beyond a failure: the run is the daemon's
/// state, it reaches every open tab over the `codemap` event, and a tab that
/// painted itself from its own click would be the one tab showing something the
/// others do not.
async function generateCodemap(name) {
  try {
    const res = await fetch(`/api/sandboxes/${encodeURIComponent(name)}/codemap`, { method: 'POST' });
    const data = await res.json();
    if (!data.ok) { showToast(data.error || 'sbxw could not start that', 'error'); return; }
    showToast(`“${name}” is writing its code map — this takes a few minutes`);
  } catch (_) {
    showToast('Could not reach sbxw', 'error');
  }
}

/// Reload the project panel if it is the one showing `name`.
///
/// What this project *has* changes from outside the panel — a map finishing is
/// files appearing under `.sbxw-artifacts/` with nobody in this tab involved —
/// so the panel re-reads rather than being told what changed.
function refreshFilesModal(name) {
  if (filesTarget !== name || filesOverlay.classList.contains('hidden')) return;
  fetchArtifacts(name);
}

async function fetchArtifacts(name) {
  filesTbody.innerHTML = '<tr><td colspan="4" class="ports-empty-msg">Loading…</td></tr>';
  try {
    const res = await fetch(`/api/sandboxes/${encodeURIComponent(name)}/artifacts`);
    const data = await res.json();
    filesDirEl.textContent = data.dir || '.sbxw-artifacts';
    renderFilesTable(data.entries || []);
    filesCodemap = { map: data.codemap || null, lenses: data.lenses || [], run: data.run || null };
    // The panel may be the first thing in this tab to hear about a run — it is
    // opened by name, while the stream only reports changes. Seeded, so a map
    // that finished before this tab existed is not announced as news.
    if (data.run) applyCodemapRun(data.run, true);
    renderCodemapButton();
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
  // Nothing to read yet: ask for one instead, and leave the panel open — the
  // button becomes the run's own status line a moment later.
  if (!filesCodemap.map && !filesCodemap.lenses.length) { generateCodemap(sandbox); return; }
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
