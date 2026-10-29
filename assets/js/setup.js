// ── Setup: Skills, Templates, Kits and MCP as tabs of one dialog ──────────
//
// The sidebar used to give each its own button; four rows for what is one
// question — what does a sandbox start with. They stay four dialogs (each
// file keeps its own open, close and dirty check), and this draws the same
// tab strip across the head of each, so switching is closing one and opening
// the next in the same frame. A tab whose dialog refuses to close (an unsaved
// form, and the person said Cancel) stays where it is.
//
// The button opens the tab used last, kept in localStorage.
const SETUP_TABS = [
  { id: 'skills',    label: 'Skills',    open: openSkillsModal,    close: closeSkillsModal },
  { id: 'templates', label: 'Templates', open: openTemplatesModal, close: closeTemplatesModal },
  { id: 'kits',      label: 'Kits',      open: openKitsModal,      close: closeKitsModal },
  { id: 'mcp',       label: 'MCP',       open: openMcpModal,       close: closeMcpModal },
];
const SETUP_KEY = 'sbxw.setup-tab';

const setupOverlay = id => document.getElementById(`${id}-modal-overlay`);
const setupIsOpen  = id => !setupOverlay(id).classList.contains('hidden');

function setupOpen(id) {
  const tab = SETUP_TABS.find(t => t.id === id) || SETUP_TABS[0];
  const current = SETUP_TABS.find(t => setupIsOpen(t.id));
  if (current === tab) return;
  if (current) {
    current.close();
    if (setupIsOpen(current.id)) return;
  }
  try { localStorage.setItem(SETUP_KEY, tab.id); } catch {}
  tab.open();
  setupOverlay(tab.id).querySelector(`.setup-tab[data-tab="${tab.id}"]`)?.focus();
}

for (const modal of document.querySelectorAll('.setup-modal')) {
  const here = modal.dataset.setupTab;
  const nav = document.createElement('div');
  nav.className = 'setup-tabs';
  nav.setAttribute('role', 'tablist');
  nav.setAttribute('aria-label', 'Setup');
  nav.innerHTML = SETUP_TABS.map(t => `<button type="button" class="setup-tab" role="tab"
      data-tab="${t.id}" aria-selected="${t.id === here}" tabindex="${t.id === here ? 0 : -1}">${t.label}</button>`).join('');
  const head = modal.querySelector('.modal-head');
  head.querySelector('.modal-title').classList.add('sr-only');
  head.prepend(nav);
  nav.addEventListener('click', e => {
    const btn = e.target.closest('.setup-tab');
    if (btn) setupOpen(btn.dataset.tab);
  });
  // Arrow keys move between tabs, as in any tab strip.
  nav.addEventListener('keydown', e => {
    const step = { ArrowRight: 1, ArrowLeft: -1 }[e.key];
    if (!step) return;
    e.preventDefault();
    const i = SETUP_TABS.findIndex(t => t.id === here);
    setupOpen(SETUP_TABS[(i + step + SETUP_TABS.length) % SETUP_TABS.length].id);
  });
}

document.getElementById('btn-setup').addEventListener('click', () => {
  let last = null;
  try { last = localStorage.getItem(SETUP_KEY); } catch {}
  setupOpen(last);
});
