// ── Skills ────────────────────────────────────────────────────────────────
//
// One modal listing the commands the bundled kits put in a sandbox and what
// each does. Its content is static markup in index.html; this file only opens
// and closes it, the same way /js/help.js does for Help.
const skillsOverlay = document.getElementById('skills-modal-overlay');

function openSkillsModal()  { skillsOverlay.classList.remove('hidden'); }
function closeSkillsModal() { skillsOverlay.classList.add('hidden'); }

document.getElementById('btn-skills').addEventListener('click', openSkillsModal);
document.getElementById('skills-modal-close').addEventListener('click', closeSkillsModal);
document.getElementById('skills-modal-close2').addEventListener('click', closeSkillsModal);
skillsOverlay.addEventListener('click', e => { if (e.target === skillsOverlay) closeSkillsModal(); });
document.addEventListener('keydown', e => {
  if (e.key === 'Escape' && !skillsOverlay.classList.contains('hidden')) closeSkillsModal();
});
