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
const githubUseGh   = document.getElementById('github-use-gh');
const githubGhNote  = document.getElementById('github-gh-note');

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
  // sbx runs `gh auth token` itself and refreshes it: nothing pasted, nothing
  // to rotate by hand.
  githubUseGh.hidden = !r.gh;
  githubGhNote.innerHTML = r.gh
    ? `sbx runs <code>${escHtml(r.gh)} auth token</code> on this machine when it needs the token, and refreshes it — your <code>gh auth login</code>, nothing pasted.`
    : 'No <code>gh</code> found (Homebrew or /usr/bin). Install it and run <code>gh auth login</code> to use it here, or paste a token below.';
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
githubUseGh.addEventListener('click', async () => {
  showNote(githubError, '');
  githubUseGh.disabled = true;
  const r = await postJsonOrError('/api/github-token/gh', {});
  githubUseGh.disabled = false;
  if (!r.ok) { showNote(githubError, r.error || 'sbx could not read a token from gh — is `gh auth login` done?'); return; }
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
