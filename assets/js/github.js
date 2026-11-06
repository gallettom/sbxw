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

const secretList  = document.getElementById('secrets-custom-list');
const secretHosts = document.getElementById('secret-hosts');
const secretEnv   = document.getElementById('secret-env');
const secretValue = document.getElementById('secret-value');

function openGithubModal() {
  showNote(githubError, '');
  githubToken.value = '';
  secretValue.value = '';
  githubOverlay.classList.remove('hidden');
  loadGithub();
  loadCustomSecrets();
  githubToken.focus();
}
function closeGithubModal() {
  if ((githubToken.value || secretValue.value) && !confirm('Discard the secret you typed?')) return;
  githubToken.value = '';
  secretValue.value = '';
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

// ── Other services: sbx's custom secrets, names and scopes only ────────────
async function loadCustomSecrets() {
  let r;
  try { r = await (await fetch('/api/secrets')).json(); }
  catch (e) { r = { ok: false, error: e.message }; }
  if (!r.ok) { secretList.innerHTML = `<p class="repo-note">${escHtml(r.error || 'Could not list the secrets')}</p>`; return; }
  const custom = (r.secrets || []).filter(x => x.kind !== 'service');
  secretList.innerHTML = custom.length
    ? custom.map(x => `<div class="tpl-row">
        <span class="tpl-tag" title="${escHtml(x.scope)}">${escHtml(x.name)}${x.scope !== 'global' ? ` · ${escHtml(x.scope)}` : ''}</span>
        ${x.scope === 'global' ? `<button class="tpl-rm" data-secret="${escHtml(x.name)}">Remove</button>` : ''}
      </div>`).join('')
    : '<p class="repo-note">None yet.</p>';
}

document.getElementById('secret-save').addEventListener('click', async () => {
  const hosts = secretHosts.value.split(/[\s,]+/).filter(Boolean);
  const env = secretEnv.value.trim();
  const value = secretValue.value;
  if (!hosts.length || !env || !value) { showNote(githubError, 'Hosts, variable and value are all needed.'); return; }
  showNote(githubError, '');
  const r = await postJsonOrError('/api/secrets/custom', { hosts, env, value });
  if (!r.ok) { showNote(githubError, r.error || 'Could not store the secret.'); return; }
  secretValue.value = '';
  secretHosts.value = '';
  secretEnv.value = '';
  loadCustomSecrets();
});

secretList.addEventListener('click', async e => {
  const btn = e.target.closest('[data-secret]');
  if (!btn) return;
  if (!confirm(`Remove the secret “${btn.dataset.secret}”? Sandboxes then send its placeholder unchanged.`)) return;
  const r = await postJsonOrError('/api/secrets/rm', { name: btn.dataset.secret });
  if (!r.ok) { showNote(githubError, r.error || 'Could not remove it.'); return; }
  loadCustomSecrets();
});
