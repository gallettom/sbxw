// ── Skills ────────────────────────────────────────────────────────────────
//
// One modal listing the commands the bundled kits put in a sandbox and what
// each does — static markup in index.html — and, above them, the person's own
// skills: ones kept from a sandbox, which every new sandbox gets (see
// svc/my_skills.rs). Keeping one is the person's act, on this page only: a
// sandbox can write a skill, never make it reach the others.
const skillsOverlay  = document.getElementById('skills-modal-overlay');
const mySkillsList   = document.getElementById('my-skills-list');
const mySkillsBox    = document.getElementById('my-skills-sandbox');
const mySkillsFound  = document.getElementById('my-skills-found');
const mySkillsError  = document.getElementById('my-skills-error');

const kb = n => (n < 1024 ? `${n} B` : `${Math.round(n / 1024)} KB`);

async function mySkillsLoad() {
  let r;
  try { r = await (await fetch('/api/my-skills')).json(); }
  catch (e) { r = { ok: false, error: e.message }; }
  if (!r.ok) { mySkillsList.innerHTML = `<p class="repo-note">Could not read them: ${escHtml(r.error)}</p>`; return; }
  mySkillsList.innerHTML = r.skills.length
    ? r.skills.map(s => `<div class="my-skill">
          <div>
            <code>/${escHtml(s.name)}</code>
            ${s.description ? `<div class="my-skill-about">${escHtml(s.description)}</div>` : ''}
            <div class="my-skill-from" title="${escHtml(s.path)}">from ${escHtml(s.sandbox || '?')}${s.saved ? `, ${new Date(s.saved * 1000).toLocaleDateString()}` : ''}</div>
          </div>
          <button class="tpl-rm" data-forget="${escHtml(s.name)}" title="New sandboxes stop getting it">Remove</button>
        </div>`).join('')
    : '<p class="repo-note">None kept yet.</p>';
}

function mySkillsSandboxes() {
  const keep = mySkillsBox.value;
  const names = (typeof sandboxes !== 'undefined' ? sandboxes : []).map(s => s.name);
  mySkillsBox.innerHTML = names.length
    ? names.map(n => `<option value="${escHtml(n)}">${escHtml(n)}</option>`).join('')
    : '<option value="">No sandbox</option>';
  if (names.includes(keep)) mySkillsBox.value = keep;
}

async function mySkillsLook() {
  const sb = mySkillsBox.value;
  if (!sb) return;
  showNote(mySkillsError, '');
  mySkillsFound.classList.remove('hidden');
  mySkillsFound.innerHTML = '<p class="repo-note">Reading the sandbox’s skills… (a stopped sandbox is started)</p>';
  let r;
  try { r = await (await fetch(`/api/sandboxes/${encodeURIComponent(sb)}/skills`)).json(); }
  catch (e) { r = { ok: false, error: e.message }; }
  if (!r.ok) { mySkillsFound.classList.add('hidden'); showNote(mySkillsError, r.error || 'Could not read the sandbox'); return; }
  mySkillsFound.innerHTML = r.skills.length
    ? r.skills.map(s => `<div class="my-skill">
          <div>
            <code>/${escHtml(s.name)}</code>
            ${s.description ? `<div class="my-skill-about">${escHtml(s.description)}</div>` : ''}
            <div class="my-skill-from" title="${escHtml(s.path)}">${escHtml(s.path)} · ${s.files} files, ${kb(s.bytes)}</div>
          </div>
          ${s.keepable
            ? `<button class="btn-cancel" data-keep="${escHtml(s.name)}" data-origin="${escHtml(s.origin)}" data-kept="${s.kept ? 1 : ''}"
                 title="Every sandbox created from now on gets it">${s.kept ? 'Replace' : 'Keep'}</button>`
            : '<span class="repo-note" title="Lowercase letters, digits, - and _ only">name not kept</span>'}
        </div>`).join('')
    : `<p class="repo-note">No skill in ${escHtml(sb)}: none under the project’s <code>.claude/skills/</code> or the sandbox’s own <code>~/.claude/skills/</code>.</p>`;
}

async function mySkillsKeep(btn) {
  const sb = mySkillsBox.value, name = btn.dataset.keep, replace = !!btn.dataset.kept;
  const ask = replace
    ? `Replace your kept /${name} with the one in ${sb}? Sandboxes created from now on get this version.`
    : `Keep /${name} from ${sb}? Every sandbox created from now on gets it — read its SKILL.md first.`;
  if (!confirm(ask)) return;
  btn.disabled = true;
  const r = await postJsonOrError(`/api/sandboxes/${encodeURIComponent(sb)}/skills/keep`, { origin: btn.dataset.origin, name, replace });
  if (!r.ok) { btn.disabled = false; showNote(mySkillsError, r.error || 'Could not keep it'); return; }
  showToast(r.warnings?.length ? `Kept /${name} — ${r.warnings.join('; ')}` : `Kept /${name}: new sandboxes get it`);
  await Promise.all([mySkillsLoad(), mySkillsLook()]);
}

async function mySkillsForget(btn) {
  const name = btn.dataset.forget;
  if (!confirm(`Stop keeping /${name}? Sandboxes created from now on will not have it; existing ones keep their copy.`)) return;
  btn.disabled = true;
  const r = await postJsonOrError('/api/my-skills/rm', { name });
  if (!r.ok) { btn.disabled = false; showNote(mySkillsError, r.error || 'Could not remove it'); return; }
  await mySkillsLoad();
}

function openSkillsModal() {
  skillsOverlay.classList.remove('hidden');
  showNote(mySkillsError, '');
  mySkillsFound.classList.add('hidden');
  mySkillsSandboxes();
  mySkillsLoad();
}
function closeSkillsModal() { skillsOverlay.classList.add('hidden'); }

document.getElementById('my-skills-look').addEventListener('click', mySkillsLook);
mySkillsFound.addEventListener('click', e => {
  const btn = e.target.closest('button[data-keep]');
  if (btn) mySkillsKeep(btn);
});
mySkillsList.addEventListener('click', e => {
  const btn = e.target.closest('button[data-forget]');
  if (btn) mySkillsForget(btn);
});

document.getElementById('btn-skills').addEventListener('click', openSkillsModal);
document.getElementById('skills-modal-close').addEventListener('click', closeSkillsModal);
document.getElementById('skills-modal-close2').addEventListener('click', closeSkillsModal);
skillsOverlay.addEventListener('click', e => { if (e.target === skillsOverlay) closeSkillsModal(); });
document.addEventListener('keydown', e => {
  if (e.key === 'Escape' && !skillsOverlay.classList.contains('hidden')) closeSkillsModal();
});
