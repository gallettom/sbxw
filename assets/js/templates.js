// ── Templates ─────────────────────────────────────────────────────────────
//
// One modal over `sbx template ls`: each saved template with a remove button,
// and a form to save a sandbox as a new one. The list is fetched each time the
// dialog opens — a template saved or removed from a terminal shows up then.
// Saving runs in the daemon (it takes the sandbox's secrets out first), and
// can take minutes; the Save button says so while it does.
const templatesOverlay = document.getElementById('templates-modal-overlay');
const templatesList    = document.getElementById('templates-list');
const templatesSandbox = document.getElementById('templates-sandbox');
const templatesTag     = document.getElementById('templates-tag');
const templatesSave    = document.getElementById('templates-save');
const templatesError   = document.getElementById('templates-error');

function openTemplatesModal() {
  showNote(templatesError, '');
  templatesSandbox.innerHTML = sandboxes.length
    ? sandboxes.map(s => `<option value="${escHtml(s.name)}">${escHtml(s.name)}</option>`).join('')
    : '<option value="">No sandbox</option>';
  const focused = panes[focusedPane]?.sandbox;
  if (focused && sandboxes.some(s => s.name === focused)) templatesSandbox.value = focused;
  templatesOverlay.classList.remove('hidden');
  loadTemplates();
}
function closeTemplatesModal() { templatesOverlay.classList.add('hidden'); }

async function loadTemplates() {
  let r;
  try { r = await (await fetch('/api/templates')).json(); }
  catch (e) { r = { ok: false, error: e.message }; }
  if (!r.ok) {
    templatesList.innerHTML = `<p class="repo-note">Could not list templates: ${escHtml(r.error || 'unknown error')}</p>`;
    return;
  }
  const rows = r.templates || [];
  if (!rows.length) {
    templatesList.innerHTML = '<p class="repo-note">No template saved yet.</p>';
    return;
  }
  templatesList.innerHTML = rows.map(t => {
    const tags = [
      t.configured ? '<span class="skills-tag skills-tag-default" title="sbxw.toml’s template: new sandboxes start from it">in use</span>' : '',
      t.auto ? `<span class="skills-tag skills-tag-manual" title="Kept by sbxw for template = &quot;auto&quot; — kits: ${escHtml((t.auto.kits || []).join(', ') || 'none')}">automatic</span>` : '',
    ].join('');
    const meta = [t.size, t.created].filter(Boolean).map(escHtml).join(' · ');
    return `<div class="tpl-row">
      <span class="tpl-tag" title="${escHtml(t.tag)}">${escHtml(t.tag)}</span>${tags}
      <span class="tpl-meta">${meta}</span>
      <button class="tpl-rm" data-tag="${escHtml(t.tag)}" title="Remove this template">Remove</button>
    </div>`;
  }).join('');
}

templatesList.addEventListener('click', async e => {
  const btn = e.target.closest('.tpl-rm');
  if (!btn) return;
  const tag = btn.dataset.tag;
  if (!confirm(`Remove template ${tag}?\n\nSandboxes already made from it keep working; new ones can no longer start from it.`)) return;
  btn.disabled = true;
  const r = await postJsonOrError('/api/templates/rm', { tag });
  if (!r.ok) { btn.disabled = false; showNote(templatesError, r.error || 'Could not remove it'); return; }
  showToast(`Removed template ${tag}`);
  loadTemplates();
});

templatesSave.addEventListener('click', async () => {
  const sandbox = templatesSandbox.value;
  const tag = templatesTag.value.trim();
  if (!sandbox) { showNote(templatesError, 'Pick a sandbox'); return; }
  if (!tag) { showNote(templatesError, 'Give the template a tag'); templatesTag.focus(); return; }
  showNote(templatesError, '');
  templatesSave.disabled = true;
  templatesSave.textContent = 'Saving…';
  const r = await postJsonOrError('/api/templates/save', { sandbox, tag });
  templatesSave.disabled = false;
  templatesSave.textContent = 'Save';
  if (!r.ok) { showNote(templatesError, r.error || 'Could not save it'); return; }
  templatesTag.value = '';
  showToast(r.warnings?.length ? `Saved ${tag} — ${r.warnings.join('; ')}` : `Saved ${sandbox} as ${tag}`);
  loadTemplates();
});
templatesTag.addEventListener('keydown', e => { if (e.key === 'Enter') templatesSave.click(); });

document.getElementById('templates-modal-close').addEventListener('click', closeTemplatesModal);
document.getElementById('templates-modal-close2').addEventListener('click', closeTemplatesModal);
templatesOverlay.addEventListener('click', e => { if (e.target === templatesOverlay) closeTemplatesModal(); });
document.addEventListener('keydown', e => {
  if (e.key === 'Escape' && !templatesOverlay.classList.contains('hidden')) closeTemplatesModal();
});
