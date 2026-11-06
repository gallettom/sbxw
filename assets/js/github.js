// ── Setup → GitHub: the token the sandbox proxy uses for git and gh ────────
//
// Write-only. `GET /api/github-token` says whether sbx holds a global
// `github` secret; saving posts the token to the daemon, which hands it to
// `sbx secret set github` on stdin. The value is cleared from the field as
// soon as it is sent and never comes back.
const githubOverlay = document.getElementById('github-modal-overlay');
const githubStatus  = document.getElementById('github-status');
const githubToken   = document.getElementById('github-token');
const githubError   = document.getElementById('github-error');
const githubSave    = document.getElementById('github-save');
const githubRemove  = document.getElementById('github-remove');

function openGithubModal() {
  showNote(githubError, '');
  githubToken.value = '';
  githubOverlay.classList.remove('hidden');
  loadGithub();
  githubToken.focus();
}
function closeGithubModal() {
  if (githubToken.value && !confirm('Discard the token you typed?')) return;
  githubToken.value = '';
  githubOverlay.classList.add('hidden');
}

async function loadGithub() {
  let r;
  try { r = await (await fetch('/api/github-token')).json(); }
  catch (e) { r = { ok: false, error: e.message }; }
  if (!r.ok) {
    githubStatus.textContent = `Could not read the token’s state: ${r.error || 'unknown error'}`;
    githubRemove.hidden = true;
    return;
  }
  githubStatus.textContent = r.set
    ? 'A token is set. Saving a new one replaces it.'
    : 'No token set: git push from a sandbox will ask for a username.';
  githubRemove.hidden = !r.set;
}

githubSave.addEventListener('click', async () => {
  const token = githubToken.value.trim();
  if (!token) { showNote(githubError, 'Paste a token first.'); return; }
  showNote(githubError, '');
  githubSave.disabled = true;
  const r = await postJsonOrError('/api/github-token', { token });
  githubSave.disabled = false;
  if (!r.ok) { showNote(githubError, r.error || 'Could not store the token.'); return; }
  githubToken.value = '';
  loadGithub();
});
githubToken.addEventListener('keydown', e => { if (e.key === 'Enter') githubSave.click(); });

githubRemove.addEventListener('click', async () => {
  if (!confirm('Remove the GitHub token? Sandboxes lose git push over HTTPS until you set another.')) return;
  const r = await postJsonOrError('/api/github-token/rm', {});
  if (!r.ok) { showNote(githubError, r.error || 'Could not remove the token.'); return; }
  loadGithub();
});

document.getElementById('github-modal-close').addEventListener('click', closeGithubModal);
document.getElementById('github-modal-close2').addEventListener('click', closeGithubModal);
githubOverlay.addEventListener('click', e => { if (e.target === githubOverlay) closeGithubModal(); });
