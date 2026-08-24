// ── Help ──────────────────────────────────────────────────────────────────
//
// One modal, one question: "why is the sandbox list empty?". sbxw renders
// whatever `sbx ls` reports and nothing else, so both cures live on the host
// (`sbx login`, then restarting the daemon) and there is nothing the UI can
// press on the user's behalf — it can only show the commands and let them be
// copied. Reachable from the header's "?" and from the sidebar's own empty
// state, which is where the question actually gets asked.
const helpOverlay = document.getElementById('help-modal-overlay');

function openHelpModal()  { helpOverlay.classList.remove('hidden'); }
function closeHelpModal() { helpOverlay.classList.add('hidden'); }

document.getElementById('btn-help').addEventListener('click', openHelpModal);
document.getElementById('help-modal-close').addEventListener('click', closeHelpModal);
document.getElementById('help-modal-close2').addEventListener('click', closeHelpModal);
helpOverlay.addEventListener('click', e => { if (e.target === helpOverlay) closeHelpModal(); });
document.addEventListener('keydown', e => {
  if (e.key === 'Escape' && !helpOverlay.classList.contains('hidden')) closeHelpModal();
});

// The list is rebuilt from an HTML string on every render, so the empty
// state's "Why is this empty?" button is bound by delegation rather than
// per-node (see renderSidebar in /js/sandboxes.js).
document.addEventListener('click', e => {
  if (e.target.closest('[data-help-open]')) openHelpModal();
});

// Copy buttons carry their command verbatim in `data-help-copy` — the visible
// <code> is HTML-escaped for the redirection operators, so reading it back out
// of the DOM would be the long way round to the same string.
helpOverlay.addEventListener('click', e => {
  const btn = e.target.closest('[data-help-copy]');
  if (btn) copyField(btn, btn.dataset.helpCopy);
});

// Same refresh the header button does, so step 2 can be checked without
// closing the dialog first.
document.getElementById('help-modal-refresh').addEventListener('click', loadSandboxes);
