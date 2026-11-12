// ── Kits ──────────────────────────────────────────────────────────────────
//
// One modal over the `kits` line of the sbxw.toml files sbxw uses: the
// daemon's own, and the one each sandbox was created from. The daemon writes
// only those files and only that line, comments and all (see svc/kits.rs);
// this page never names a path of its own.
//
// Bundled kits are checkboxes, everything else a list with an add field. A
// bundled kit listed by its path in a checkout (`…/assets/codemap`) is shown on
// its checkbox row, not again under Other kits: it is the same kit, read from
// the working tree instead of the binary — which is why it is kept as written
// unless "Use sbxw: names" is pressed. A file without a `kits` line gets the
// defaults, shown checked: saving writes them out, and "Use defaults" takes the
// line away again.
const kitsOverlay  = document.getElementById('kits-modal-overlay');
const kitsFile     = document.getElementById('kits-file');
const kitsFileNote = document.getElementById('kits-file-note');
const kitsBundled  = document.getElementById('kits-bundled');
const kitsOther    = document.getElementById('kits-other');
const kitsAddInput = document.getElementById('kits-add-input');
const kitsSave     = document.getElementById('kits-save');
const kitsDefaults = document.getElementById('kits-defaults');
const kitsError    = document.getElementById('kits-error');

let kitsPanel = null;   // { files, bundled, defaults } from the daemon
let kitsList = [];      // the line being edited, in order
let kitsExplicit = false;
let kitsDirty = false;

const kitsIsBundled = (ref, name) => ref === `sbxw:${name}`;

function kitsCurrentFile() {
  return kitsPanel?.files.find(f => f.path === kitsFile.value) || null;
}

function kitsMarkDirty(d) {
  kitsDirty = d;
  kitsSave.disabled = !d;
}

function kitsLoadFile() {
  const f = kitsCurrentFile();
  showNote(kitsError, '');
  if (!f) { kitsList = []; kitsExplicit = false; kitsRender(); return; }
  kitsExplicit = Array.isArray(f.kits);
  kitsList = kitsExplicit ? [...f.kits] : [...kitsPanel.defaults];
  const users = [f.daemon ? 'the daemon (new sandboxes from this page)' : '', ...(f.sandboxes || [])].filter(Boolean);
  const notes = [
    users.length ? `Used by ${users.join(', ')}.` : '',
    f.exists ? '' : 'The file does not exist yet — saving creates it.',
    kitsExplicit ? '' : 'No kits line: the default kits apply. Saving writes them out.',
    f.error ? `Cannot be edited: ${f.error}` : '',
  ].filter(Boolean);
  kitsFileNote.textContent = notes.join(' ');
  kitsFileNote.classList.toggle('warn', !!f.error);
  kitsDefaults.disabled = !kitsExplicit;
  kitsMarkDirty(false);
  kitsRender();
}

function kitsRender() {
  const broken = !!kitsCurrentFile()?.error;
  kitsBundled.innerHTML = (kitsPanel?.bundled || []).map(b => {
    const on = kitsList.some(k => kitsIsBundled(k, b.name));
    return `<label class="kit-row">
      <input type="checkbox" data-name="${escHtml(b.name)}" ${on ? 'checked' : ''} ${broken ? 'disabled' : ''}>
      <span class="kit-ref">sbxw:${escHtml(b.name)}</span>
      ${b.default ? '<span class="skills-tag skills-tag-default" title="What a file without a kits line gets">default</span>' : ''}
      <span class="kit-about">${escHtml(b.about)}</span>
    </label>`;
  }).join('');
  const others = kitsList.filter(k => !k.startsWith('sbxw:'));
  kitsOther.innerHTML = others.length
    ? others.map(k => `<div class="tpl-row">
          <span class="tpl-tag" title="${escHtml(k)}">${escHtml(k)}</span>
          <button class="tpl-rm" data-ref="${escHtml(k)}" ${broken ? 'disabled' : ''} title="Take it out of the list">Remove</button>
        </div>`).join('')
    : '<p class="repo-note">None — only bundled kits.</p>';
}

async function openKitsModal() {
  showNote(kitsError, '');
  modalOpen(kitsOverlay);
  let r;
  try { r = await (await fetch('/api/kits')).json(); }
  catch (e) { r = { ok: false, error: e.message }; }
  if (!r.ok) { showNote(kitsError, `Could not read the configs: ${r.error || 'unknown error'}`); return; }
  kitsPanel = r;
  const keep = kitsFile.value;
  kitsFile.innerHTML = r.files.length
    ? r.files.map(f => `<option value="${escHtml(f.path)}">${escHtml(f.path)}</option>`).join('')
    : '<option value="">No sbxw.toml in use</option>';
  if (r.files.some(f => f.path === keep)) kitsFile.value = keep;
  else {
    const daemon = r.files.find(f => f.daemon);
    if (daemon) kitsFile.value = daemon.path;
  }
  if (!r.files.length) {
    kitsFileNote.textContent = 'The daemon runs without an sbxw.toml and no sandbox records one: start it from a project with `sbxw web`, or create a sandbox from one.';
  }
  kitsLoadFile();
}

function closeKitsModal() {
  if (kitsDirty && !confirm('Close without saving the kits?')) return;
  kitsOverlay.close();
}

async function kitsWrite(kits) {
  const path = kitsFile.value;
  if (!path) return;
  kitsSave.disabled = kitsDefaults.disabled = true;
  const r = await postJsonOrError('/api/kits', { path, kits });
  if (!r.ok) {
    kitsMarkDirty(true);
    kitsDefaults.disabled = !kitsExplicit;
    showNote(kitsError, r.error || 'Could not save');
    return;
  }
  showToast(r.warnings?.length ? `Saved — ${r.warnings.join('; ')}` : `Saved ${path.split('/').pop()}`);
  kitsDirty = false;
  await openKitsModal();
  if (r.warnings?.length) showNote(kitsError, r.warnings.join(' · '));
}

kitsFile.addEventListener('change', () => {
  if (kitsDirty && !confirm('Discard the unsaved kits of the other file?')) return;
  kitsLoadFile();
});

kitsBundled.addEventListener('change', e => {
  const box = e.target.closest('input[type=checkbox]');
  if (!box) return;
  const name = box.dataset.name;
  kitsList = box.checked
    ? [...kitsList, `sbxw:${name}`]
    : kitsList.filter(k => !kitsIsBundled(k, name));
  kitsMarkDirty(true);
  kitsRender();
});

kitsOther.addEventListener('click', e => {
  const btn = e.target.closest('.tpl-rm');
  if (!btn) return;
  kitsList = kitsList.filter(k => k !== btn.dataset.ref);
  kitsMarkDirty(true);
  kitsRender();
});

function kitsAdd() {
  const ref = kitsAddInput.value.trim();
  if (!ref) return;
  if (kitsList.includes(ref)) { showNote(kitsError, `${ref} is already in the list`); return; }
  showNote(kitsError, '');
  kitsList.push(ref);
  kitsAddInput.value = '';
  kitsMarkDirty(true);
  kitsRender();
}
document.getElementById('kits-add').addEventListener('click', kitsAdd);
kitsAddInput.addEventListener('keydown', e => { if (isEnterKey(e)) kitsAdd(); });

kitsSave.addEventListener('click', () => kitsWrite(kitsList));
kitsDefaults.addEventListener('click', () => {
  if (!confirm('Remove the kits line from this file? Its sandboxes then get the default kits.')) return;
  kitsWrite(null);
});

document.getElementById('kits-modal-close').addEventListener('click', closeKitsModal);
document.getElementById('kits-modal-close2').addEventListener('click', closeKitsModal);
wireModal(kitsOverlay, closeKitsModal);
