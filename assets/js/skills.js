// ── Skills ────────────────────────────────────────────────────────────────
//
// One modal listing the commands the bundled kits put in a sandbox and what
// each does (static markup in index.html) and, above them, the person's own
// skills: ones kept from a sandbox, which every new sandbox gets (see
// svc/my_skills.rs). Showing a sandbox's skills also lists what it has
// installed (GET /api/sandboxes/:name/commands), read only. Keeping one is the person's act, on this page only: a
// sandbox can write a skill, never make it reach the others.
const skillsOverlay  = document.getElementById('skills-modal-overlay');
const mySkillsList   = document.getElementById('my-skills-list');
const mySkillsBox    = document.getElementById('my-skills-sandbox');
const mySkillsFound  = document.getElementById('my-skills-found');
const mySkillsError  = document.getElementById('my-skills-error');

// BEGIN skills pure
// "Installed in <sandbox>": every skill and command of
// GET /api/sandboxes/:name/commands, grouped by origin in plain words, read
// only (kits and built-ins come back with every sandbox: nothing to keep).
// The desktop panel's screens/skills.luau does the same.
const SKILLS_GROUP_ORDER = { kits: 1, kept: 2, store: 3, own: 4, project: 5, 'built-in': 7 };

function skillsClip(t, n) {
  const a = Array.from(String(t ?? ''));
  return a.length <= n ? a.join('') : a.slice(0, n - 1).join('') + '…';
}

// { id, title, where } for one entry. `kept`: names of the person's kept
// skills; `ownHome`: names the keepable listing found under ~/.claude/skills.
function skillsOriginOf(c, kept, ownHome) {
  const o = String(c.origin ?? '');
  if (o === 'built-in') return { id: 'built-in', title: 'Built-in', where: 'Claude Code’s own commands' };
  if (o === 'workspace') return { id: 'project', title: 'Project', where: 'the project’s .claude/skills/ and .claude/commands/' };
  const kit = /^kit:(.+)$/.exec(o);
  if (kit) return { id: `kit-${skillsClip(kit[1], 40)}`, title: `Kit ${skillsClip(kit[1], 40)}`, where: 'installed by that kit' };
  if (o === 'home' && c.kind === 'command') {
    if (kept.has(c.name)) return { id: 'kept', title: 'Your kept skills', where: 'the sbxw-my-skills kit' };
    return { id: 'kits', title: 'Kits', where: '~/.claude/commands/, written by the sandbox’s kits' };
  }
  if (o === 'home') {
    if (ownHome.has(c.name)) return { id: 'own', title: 'Written in this sandbox', where: '~/.claude/skills/: you can keep it above' };
    return { id: 'store', title: 'sbx shared skills', where: '~/.claude/skills/, sbx’s shared skills store' };
  }
  return { id: `other-${skillsClip(o, 24)}`, title: skillsClip(o, 24), where: '' };
}

// The groups, in order: [{ o, items: [{ name, description, kind }] }].
function skillsInstalledGroups(answer, kept, ownHome) {
  const by = new Map();
  for (const c of Array.isArray(answer?.commands) ? answer.commands : []) {
    if (!c || typeof c.name !== 'string') continue;
    const o = skillsOriginOf(c, kept, ownHome);
    if (!by.has(o.id)) by.set(o.id, { o, items: [] });
    by.get(o.id).items.push({
      name: skillsClip(c.name, 64),
      description: typeof c.description === 'string' ? skillsClip(c.description, 160) : '',
      kind: c.kind === 'command' ? 'command' : 'skill',
    });
  }
  const rank = id => SKILLS_GROUP_ORDER[id] ?? (id.startsWith('kit-') ? 0 : 6);
  return [...by.values()].sort((a, b) => rank(a.o.id) - rank(b.o.id) || (a.o.id < b.o.id ? -1 : a.o.id > b.o.id ? 1 : 0));
}

// The section's markup, every text escaped; '' when the daemon has no such
// route or the read failed (the dialog is then as before).
function skillsInstalledHtml(answer, sb, kept, ownHome) {
  if (!answer || !Array.isArray(answer.commands)) return '';
  const groups = skillsInstalledGroups(answer, kept, ownHome);
  const total = groups.reduce((n, g) => n + g.items.length, 0);
  return `<div class="my-skills-installed" data-installed>
      <div class="help-step-head">Installed in ${escHtml(sb)} <span class="skills-kit">${total} installed</span></div>
      <p class="repo-note">What every sandbox gets already, read only: you type them after /. Nothing here needs keeping.</p>
      ${answer.complete === false ? `<p class="repo-note" data-installed-reason>${escHtml(skillsClip(answer.reason || 'The list is partial.', 200))}</p>` : ''}
      ${groups.map(g => `<div class="my-skills-group" data-group="${escHtml(g.o.id)}">
          <div class="my-skill-from"><strong>${escHtml(g.o.title)}</strong> (${g.items.length})${g.o.where ? ` · ${escHtml(g.o.where)}` : ''}</div>
          ${g.items.map(x => `<div class="my-skill">
              <div>
                <code>/${escHtml(x.name)}</code>
                ${x.description ? `<div class="my-skill-about">${escHtml(x.description)}</div>` : ''}
              </div>
              <span class="repo-note">${x.kind}</span>
            </div>`).join('')}
        </div>`).join('')}
    </div>`;
}

// The keepable list's empty text: "No skill" only when nothing else is known.
function skillsNothingToKeep(sb, haveInstalled) {
  return haveInstalled
    ? `<p class="repo-note">Nothing to keep: no skill the agent wrote under the project’s <code>.claude/skills/</code> or the sandbox’s own <code>~/.claude/skills/</code>. What ${escHtml(sb)} has already is listed below.</p>`
    : `<p class="repo-note">No skill in ${escHtml(sb)}: none under the project’s <code>.claude/skills/</code> or the sandbox’s own <code>~/.claude/skills/</code>.</p>`;
}
// END skills pure

// Names of the person's kept skills, as last read (the installed list names
// their commands so).
let mySkillsKept = new Set();

// GET /commands, or null for a daemon without the route (404) or any failed
// read: the dialog is then as before. Never starts a stopped sandbox.
async function mySkillsInstalled(sb) {
  try {
    const res = await fetch(`/api/sandboxes/${encodeURIComponent(sb)}/commands`);
    if (!res.ok) return null;
    const r = await res.json();
    return r && Array.isArray(r.commands) ? r : null;
  } catch { return null; }
}

const kb = v => { const n = Number(v) || 0; return n < 1024 ? `${n} B` : `${Math.round(n / 1024)} KB`; };

async function mySkillsLoad() {
  let r;
  try { r = await (await fetch('/api/my-skills')).json(); }
  catch (e) { r = { ok: false, error: e.message }; }
  if (!r.ok) { mySkillsList.innerHTML = `<p class="repo-note">Could not read them: ${escHtml(r.error)}</p>`; return; }
  mySkillsKept = new Set(r.skills.map(s => s.name));
  mySkillsList.innerHTML = r.skills.length
    ? r.skills.map(s => `<div class="my-skill">
          <div>
            <code>/${escHtml(s.name)}</code>
            ${s.description ? `<div class="my-skill-about">${escHtml(s.description)}</div>` : ''}
            <div class="my-skill-from" title="${escHtml(s.path)}">from ${escHtml(s.sandbox || '?')}${s.saved ? `, ${escHtml(new Date(Number(s.saved) * 1000).toLocaleDateString())}` : ''}</div>
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
  const installed = mySkillsInstalled(sb);
  let r;
  try { r = await (await fetch(`/api/sandboxes/${encodeURIComponent(sb)}/skills`)).json(); }
  catch (e) { r = { ok: false, error: e.message }; }
  const inst = await installed;
  const ownHome = new Set(r.ok ? r.skills.filter(s => s.origin === 'home').map(s => s.name) : []);
  const instHtml = skillsInstalledHtml(inst, sb, mySkillsKept, ownHome);
  if (!r.ok) {
    showNote(mySkillsError, r.error || 'Could not read the sandbox');
    if (instHtml) mySkillsFound.innerHTML = instHtml; else mySkillsFound.classList.add('hidden');
    return;
  }
  mySkillsFound.innerHTML = (r.skills.length
    ? r.skills.map(s => `<div class="my-skill">
          <div>
            <code>/${escHtml(s.name)}</code>
            ${s.description ? `<div class="my-skill-about">${escHtml(s.description)}</div>` : ''}
            <div class="my-skill-from" title="${escHtml(s.path)}">${escHtml(s.path)} · ${Number(s.files) || 0} files, ${kb(s.bytes)}</div>
          </div>
          ${s.keepable
            ? `<button class="btn-cancel" data-keep="${escHtml(s.name)}" data-origin="${escHtml(s.origin)}" data-kept="${s.kept ? 1 : ''}"
                 title="Every sandbox created from now on gets it">${s.kept ? 'Replace' : 'Keep'}</button>`
            : '<span class="repo-note" title="Lowercase letters, digits, - and _ only">name not kept</span>'}
        </div>`).join('')
    : skillsNothingToKeep(sb, !!instHtml)) + instHtml;
}

async function mySkillsKeep(btn) {
  const sb = mySkillsBox.value, name = btn.dataset.keep, replace = !!btn.dataset.kept;
  const ask = replace
    ? `Replace your kept /${name} with the one in ${sb}? Sandboxes created from now on get this version.`
    : `Keep /${name} from ${sb}? Every sandbox created from now on gets it. Read its SKILL.md first.`;
  if (!confirm(ask)) return;
  btn.disabled = true;
  const r = await postJsonOrError(`/api/sandboxes/${encodeURIComponent(sb)}/skills/keep`, { origin: btn.dataset.origin, name, replace });
  if (!r.ok) { btn.disabled = false; showNote(mySkillsError, r.error || 'Could not keep it'); return; }
  showToast(r.warnings?.length ? `Kept /${name}: ${r.warnings.join('; ')}` : `Kept /${name}: new sandboxes get it`);
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
  modalOpen(skillsOverlay);
  showNote(mySkillsError, '');
  mySkillsFound.classList.add('hidden');
  mySkillsSandboxes();
  mySkillsLoad();
}
function closeSkillsModal() { skillsOverlay.close(); }

document.getElementById('my-skills-look').addEventListener('click', mySkillsLook);
mySkillsFound.addEventListener('click', e => {
  const btn = e.target.closest('button[data-keep]');
  if (btn) mySkillsKeep(btn);
});
mySkillsList.addEventListener('click', e => {
  const btn = e.target.closest('button[data-forget]');
  if (btn) mySkillsForget(btn);
});

document.getElementById('skills-modal-close').addEventListener('click', closeSkillsModal);
document.getElementById('skills-modal-close2').addEventListener('click', closeSkillsModal);
wireModal(skillsOverlay, closeSkillsModal);
