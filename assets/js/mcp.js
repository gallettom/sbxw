// ── MCP servers ───────────────────────────────────────────────────────────
//
// One modal over `sbx mcp`: the servers registered on this host, a form to
// add one or edit one, and Load / Unload into the sandbox picked at the top.
// Registration is the host's; a sandbox only gets a server once it is loaded
// there (`sbx mcp load`, the sandbox running).
//
// sbx has no edit: saving an edit asks the daemon to remove the server and
// add it again, which also drops its OAuth token. And sbx does not say which
// servers a sandbox has loaded, so Load and Unload are both always offered.
// `sbx mcp ls --format json` documents only `name` and `type`; an edit
// prefills whatever else the row carries that looks like a URL or a command.
const mcpOverlay   = document.getElementById('mcp-modal-overlay');
const mcpListEl    = document.getElementById('mcp-list');
const mcpSandbox   = document.getElementById('mcp-sandbox');
const mcpInspect   = document.getElementById('mcp-inspect');
const mcpError     = document.getElementById('mcp-error');
const mcpFormTitle = document.getElementById('mcp-form-title');
const mcpName      = document.getElementById('mcp-name');
const mcpKind      = document.getElementById('mcp-kind');
const mcpUrl       = document.getElementById('mcp-url');
const mcpHeaders   = document.getElementById('mcp-headers');
const mcpSecrets   = document.getElementById('mcp-secrets');
const mcpCommand   = document.getElementById('mcp-command');
const mcpArgs      = document.getElementById('mcp-args');
const mcpDir       = document.getElementById('mcp-dir');
const mcpSave      = document.getElementById('mcp-save');
const mcpCancel    = document.getElementById('mcp-cancel-edit');

let mcpServers = [];
let mcpEditing = null;   // the name being edited, or null for a new server

// What an `mcp ls` row says about itself, under whichever key it uses.
function mcpField(detail, ...keys) {
  for (const k of keys) {
    const v = k.split('.').reduce((o, p) => (o && typeof o === 'object' ? o[p] : undefined), detail);
    if (v != null && v !== '') return v;
  }
  return null;
}
const mcpIsStdio = s => /stdio|local|command/i.test(s.type || '') || !!mcpField(s.detail, 'command');

function mcpRenderSandboxes() {
  const keep = mcpSandbox.value;
  const running = sandboxes.filter(s => s.status === 'running');
  mcpSandbox.innerHTML = running.length
    ? running.map(s => `<option value="${escHtml(s.name)}">${escHtml(s.name)}</option>`).join('')
    : '<option value="">no running sandbox</option>';
  if (running.some(s => s.name === keep)) mcpSandbox.value = keep;
}

function mcpRenderList() {
  const target = mcpSandbox.value;
  mcpListEl.innerHTML = mcpServers.length
    ? mcpServers.map(s => `<div class="tpl-row" data-server="${escHtml(s.name)}">
        <span class="tpl-tag" title="${escHtml(s.name)}">${escHtml(s.name)}</span>
        <span class="tpl-meta">${escHtml(s.type || '')}</span>
        <span class="mcp-actions">
          <button class="mcp-btn" data-act="load" ${target ? '' : 'disabled'} title="Attach to ${escHtml(target || 'a running sandbox')} — its agent picks up the tools without a restart">Load</button>
          <button class="mcp-btn" data-act="unload" ${target ? '' : 'disabled'} title="Detach from ${escHtml(target || 'a running sandbox')} — its agent keeps running">Unload</button>
          <button class="mcp-btn" data-act="inspect" title="sbx mcp inspect — header templates and whether their secrets are set">Inspect</button>
          <button class="mcp-btn" data-act="edit" title="Edit — saved as remove then add">Edit</button>
          <button class="tpl-rm" data-act="rm" title="Unregister from this host">Remove</button>
        </span>
      </div>`).join('')
    : '<p class="mcp-empty">No servers registered yet. Add the first one below.</p>';
}

async function mcpLoad() {
  let r;
  try { r = await (await fetch('/api/mcp')).json(); }
  catch (e) { r = { ok: false, error: e.message }; }
  if (!r.ok) {
    mcpServers = [];
    mcpListEl.innerHTML = `<p class="repo-note">${escHtml(r.error || 'Could not list MCP servers')}</p>`;
    return;
  }
  mcpServers = r.servers || [];
  mcpRenderList();
}

// The secret fields follow the `${key}` placeholders typed in the headers. On
// an edit an empty field keeps the stored secret.
function mcpRenderSecrets() {
  const keys = [];
  for (const m of mcpHeaders.value.matchAll(/\$\{([A-Za-z0-9_-]+)\}/g)) if (!keys.includes(m[1])) keys.push(m[1]);
  const kept = Object.fromEntries([...mcpSecrets.querySelectorAll('input')].map(i => [i.dataset.key, i.value]));
  mcpSecrets.innerHTML = keys.map(k => `<label class="mcp-secret">
      <code>\${${escHtml(k)}}</code>
      <input class="field-input" type="password" autocomplete="off" data-key="${escHtml(k)}"
             placeholder="${mcpEditing ? 'unchanged if left empty' : 'secret value'}" aria-label="Secret ${escHtml(k)}">
    </label>`).join('');
  for (const i of mcpSecrets.querySelectorAll('input')) i.value = kept[i.dataset.key] || '';
}

function mcpSyncKind() {
  const stdio = mcpKind.value === 'stdio';
  mcpOverlay.querySelector('.mcp-remote').classList.toggle('hidden', stdio);
  mcpOverlay.querySelector('.mcp-stdio').classList.toggle('hidden', !stdio);
}

function mcpResetForm() {
  mcpEditing = null;
  mcpFormTitle.textContent = 'New server';
  mcpSave.textContent = 'Add';
  mcpCancel.classList.add('hidden');
  for (const el of [mcpName, mcpUrl, mcpHeaders, mcpCommand, mcpArgs, mcpDir]) el.value = '';
  mcpKind.value = 'remote';
  mcpSecrets.innerHTML = '';
  mcpSyncKind();
}

function mcpStartEdit(s) {
  mcpEditing = s.name;
  mcpFormTitle.textContent = `Edit ${s.name}`;
  mcpSave.textContent = 'Save';
  mcpCancel.classList.remove('hidden');
  const d = s.detail || {};
  mcpName.value = s.name;
  mcpKind.value = mcpIsStdio(s) ? 'stdio' : 'remote';
  mcpUrl.value = mcpField(d, 'url', 'endpoint', 'remote.url', 'config.url') || '';
  const headers = mcpField(d, 'headers', 'config.headers');
  mcpHeaders.value = Array.isArray(headers) ? headers.join('\n')
    : headers && typeof headers === 'object' ? Object.entries(headers).map(([k, v]) => `${k}: ${v}`).join('\n') : '';
  mcpCommand.value = mcpField(d, 'command', 'config.command') || '';
  const args = mcpField(d, 'args', 'config.args');
  mcpArgs.value = Array.isArray(args) ? args.join(' ') : (args || '');
  mcpDir.value = mcpField(d, 'dir', 'cwd', 'config.dir') || '';
  mcpSyncKind();
  mcpRenderSecrets();
  showNote(mcpError, (mcpUrl.value || mcpCommand.value) ? '' : 'sbx did not list this server\'s settings — fill them in again (Inspect shows its headers).');
  mcpName.focus();
}

async function mcpSubmit() {
  const spec = {
    name: mcpName.value.trim(),
    kind: mcpKind.value,
    url: mcpUrl.value.trim(),
    headers: mcpHeaders.value.split('\n').map(h => h.trim()).filter(Boolean),
    command: mcpCommand.value.trim(),
    args: mcpArgs.value.trim() ? mcpArgs.value.trim().split(/\s+/) : [],
    dir: mcpDir.value.trim(),
  };
  if (!spec.name) { showNote(mcpError, 'A server needs a name'); return; }
  if (mcpEditing && !confirm(
    `Save '${mcpEditing}'?\n\nsbx has no edit: the server is removed and added again, ` +
    `which drops its OAuth token (run sbx mcp auth again if it uses OAuth).`)) return;
  const secrets = Object.fromEntries([...mcpSecrets.querySelectorAll('input')].map(i => [i.dataset.key, i.value]));
  mcpSave.disabled = true;
  showNote(mcpError, '');
  const r = await postJsonOrError('/api/mcp', { spec, replace: mcpEditing, secrets });
  mcpSave.disabled = false;
  if (!r.ok) { showNote(mcpError, r.error || 'Could not save'); await mcpLoad(); return; }
  showToast(mcpEditing ? `${spec.name} saved` : `${spec.name} added`);
  mcpResetForm();
  await mcpLoad();
}

async function mcpRowAction(btn) {
  const server = btn.closest('.tpl-row').dataset.server;
  const act = btn.dataset.act;
  const target = mcpSandbox.value;
  showNote(mcpError, '');
  if (act === 'edit') { mcpStartEdit(mcpServers.find(s => s.name === server)); return; }
  if (act === 'inspect') {
    let r;
    try { r = await (await fetch(`/api/mcp/${encodeURIComponent(server)}`)).json(); }
    catch (e) { r = { ok: false, error: e.message }; }
    if (!r.ok) { showNote(mcpError, r.error || 'Could not inspect'); return; }
    mcpInspect.textContent = r.text.trim() || '(no output)';
    mcpInspect.classList.remove('hidden');
    return;
  }
  if (act === 'rm' && !confirm(
    `Unregister MCP server '${server}' from this host?\n\n` +
    `Sandboxes that loaded it keep it until it is unloaded there.`)) return;
  if (act === 'unload' && !confirm(`Unload MCP server '${server}' from '${target}'?`)) return;
  btn.disabled = true;
  const url = act === 'rm'
    ? `/api/mcp/${encodeURIComponent(server)}/rm`
    : `/api/sandboxes/${encodeURIComponent(target)}/mcp/${act}`;
  const r = await postJsonOrError(url, act === 'rm' ? {} : { server });
  btn.disabled = false;
  if (!r.ok) { showNote(mcpError, r.error || `Could not ${act} ${server}`); return; }
  showToast({ rm: `${server} removed`, load: `${server} loaded into ${target}`, unload: `${server} unloaded from ${target}` }[act]);
  if (act === 'rm') {
    if (mcpEditing === server) mcpResetForm();
    await mcpLoad();
  }
}

async function openMcpModal() {
  showNote(mcpError, '');
  mcpInspect.classList.add('hidden');
  mcpOverlay.classList.remove('hidden');
  mcpRenderSandboxes();
  await mcpLoad();
}

function mcpFormDirty() {
  return [mcpName, mcpUrl, mcpHeaders, mcpCommand, mcpArgs, mcpDir].some(el => el.value.trim());
}

function closeMcpModal() {
  if (mcpFormDirty() && !confirm('Close without saving the server?')) return;
  mcpResetForm();
  mcpOverlay.classList.add('hidden');
}

mcpListEl.addEventListener('click', e => {
  const btn = e.target.closest('button[data-act]');
  if (btn) mcpRowAction(btn);
});
mcpSandbox.addEventListener('change', mcpRenderList);
mcpKind.addEventListener('change', mcpSyncKind);
mcpHeaders.addEventListener('input', mcpRenderSecrets);
mcpSave.addEventListener('click', mcpSubmit);
mcpCancel.addEventListener('click', () => { mcpResetForm(); showNote(mcpError, ''); });

document.getElementById('mcp-modal-close').addEventListener('click', closeMcpModal);
document.getElementById('mcp-modal-close2').addEventListener('click', closeMcpModal);
mcpOverlay.addEventListener('click', e => { if (e.target === mcpOverlay) closeMcpModal(); });
document.addEventListener('keydown', e => {
  if (e.key === 'Escape' && !mcpOverlay.classList.contains('hidden')) closeMcpModal();
});
