// ── Remote hosts (EXPERIMENTAL) ───────────────────────────────────────────
//
// sbxw can drive `sbx` on other machines over SSH (docs/remote-hosts.md).
// Their sandboxes are named by ref, `host~name`; this machine's keep their
// bare names. With at least one host registered the sidebar becomes one
// section per host — `local` first — each with a header saying whether its
// SSH link is up, how long a round trip takes, and a ⋯ menu (Test, Provision,
// Edit). With none, nothing here shows and the sidebar is what it always was.
//
// The registry itself (`~/.sbxw/hosts.toml`) is edited in Setup → Hosts. A
// host key is trusted once, on a fingerprint the person has read: the dialog
// asks the daemon for it first, shows it, and only the second call — which
// carries the fingerprint back — writes known_hosts.
let remoteHosts = [];
// This machine's sbxw binary, for the provision command (from the daemon).
let sbxwBin = 'sbxw';
const HOST_FOLD_KEY = 'sbxw.hosts-folded';
const HOSTS_POLL_MS = 15000;

const sandboxHost = name => { const i = name.indexOf('~'); return i < 0 ? 'local' : name.slice(0, i); };
const sandboxShortName = name => { const i = name.indexOf('~'); return i < 0 ? name : name.slice(i + 1); };

// Grouped by host as soon as there is another host to group by.
function sidebarByHost() {
  return remoteHosts.length > 0 || sandboxes.some(s => s.name.includes('~'));
}

// `HH:MM` in the browser's own time: "since 14:02" is read off a wall clock.
function hostClock(unix) {
  if (!unix) return '?';
  return new Date(unix * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' });
}

// Why a sandbox cannot be acted on, or '' when it can.
function sandboxUnreachable(s) {
  if (!s || !s.unreachable_since) return '';
  return `Host “${sandboxHost(s.name)}” unreachable since ${hostClock(s.unreachable_since)}: its SSH link is down `
    + `(sbxw keeps retrying). ${s.status ? `Last seen ${s.status}. ` : ''}Its actions are off until it answers again.`;
}

async function loadRemoteHosts() {
  try {
    const r = await (await fetch('/api/remote-hosts')).json();
    if (r && r.ok) {
      remoteHosts = r.hosts || [];
      sbxwBin = r.sbxw || 'sbxw';
    }
  } catch (_) { /* the list keeps what it had */ }
}

// ── Folded sections, per browser ──────────────────────────────────────────
function foldedHosts() {
  try { return new Set(JSON.parse(localStorage.getItem(HOST_FOLD_KEY) || '[]')); } catch (_) { return new Set(); }
}
function setHostFolded(host, folded) {
  const set = foldedHosts();
  if (folded) set.add(host); else set.delete(host);
  try { localStorage.setItem(HOST_FOLD_KEY, JSON.stringify([...set])); } catch (_) {}
}

// ── The sidebar's sections ────────────────────────────────────────────────
function hostLinkLabel(h) {
  if (!h) return { cls: 'up', text: 'this machine', title: 'This machine' };
  if (h.link === 'up') {
    const ms = h.latency_ms != null ? `${h.latency_ms} ms` : 'up';
    return { cls: 'up', text: ms, title: `SSH link up${h.latency_ms != null ? `, last round trip ${h.latency_ms} ms` : ''}` };
  }
  if (h.link === 'down') {
    return { cls: 'down', text: `down ${hostClock(h.down_since)}`,
      title: `SSH link down since ${hostClock(h.down_since)}; sbxw keeps retrying` };
  }
  return { cls: 'never', text: 'never', title: 'Never connected: sbxw has not reached this host yet (it connects when the list or an action needs it)' };
}

function hostSectionHtml(name, h, list, folded) {
  const link = hostLinkLabel(h);
  const menu = h
    ? `<button class="host-menu-btn" type="button" data-host-menu="${escHtml(name)}" aria-haspopup="menu"
               title="Test, provision or edit ${escHtml(name)}" aria-label="Actions for host ${escHtml(name)}">⋯</button>`
    : '';
  const body = folded ? '' : list.length ? groupedHtml(list)
    : `<div class="host-empty">No sandboxes${h ? ' on this host' : ''}</div>`;
  return `<div class="host-section${folded ? ' folded' : ''}" data-host="${escHtml(name)}">
    <div class="host-head" data-host-fold="${escHtml(name)}" role="button" tabindex="0" aria-expanded="${!folded}"
         title="${folded ? 'Show' : 'Hide'} the sandboxes on ${escHtml(name)}">
      <span class="host-chevron" aria-hidden="true">▾</span>
      <span class="host-name">${escHtml(name)}</span>
      <span class="dot host-link ${link.cls}" title="${escHtml(link.title)}"></span>
      <span class="host-link-text" title="${escHtml(link.title)}">${escHtml(link.text)}</span>
      <span class="host-count" title="${list.length} sandbox${list.length === 1 ? '' : 'es'}">${list.length}</span>
      ${menu}
    </div>
    ${body}
  </div>`;
}

// A header whose row overflows even with the name cut to its minimum drops
// its link text (CSS .tight); the dot's title still says it. Measured after
// the list is drawn, and again when the sidebar changes size.
function fitHostHeads() {
  for (const head of listEl.querySelectorAll('.host-head')) {
    head.classList.remove('tight');
    if (head.scrollWidth > head.clientWidth) head.classList.add('tight');
  }
}
new ResizeObserver(() => fitHostHeads()).observe(listEl);

function renderHostSections() {
  requestAnimationFrame(fitHostHeads);
  const folded = foldedHosts();
  const names = ['local', ...remoteHosts.map(h => h.name)];
  // A ref whose host left hosts.toml still shows, under its own header.
  for (const s of sandboxes) if (!names.includes(sandboxHost(s.name))) names.push(sandboxHost(s.name));
  return names.map(n => hostSectionHtml(n, remoteHosts.find(h => h.name === n) || null,
    sandboxes.filter(s => sandboxHost(s.name) === n), folded.has(n))).join('');
}

listEl.addEventListener('click', e => {
  const menuBtn = e.target.closest('[data-host-menu]');
  if (menuBtn) { e.stopPropagation(); openHostMenu(menuBtn.dataset.hostMenu, menuBtn); return; }
  const head = e.target.closest('[data-host-fold]');
  if (!head) return;
  const host = head.dataset.hostFold;
  setHostFolded(host, !foldedHosts().has(host));
  renderSidebar();
});
listEl.addEventListener('keydown', e => {
  const head = e.target.closest('[data-host-fold]');
  if (!head || (e.key !== 'Enter' && e.key !== ' ')) return;
  e.preventDefault();
  head.click();
});

// A link goes up or down without the sandbox list being asked again: poll the
// hosts, and re-read the list when a link changed (that is what greys or
// un-greys its sandboxes). Only while the tab is looked at.
setInterval(async () => {
  if (document.visibilityState !== 'visible' || !remoteHosts.length) return;
  const before = JSON.stringify(remoteHosts.map(h => [h.name, h.link]));
  const shown = JSON.stringify(remoteHosts);
  await loadRemoteHosts();
  if (JSON.stringify(remoteHosts.map(h => [h.name, h.link])) !== before) loadSandboxes();
  else if (JSON.stringify(remoteHosts) !== shown) renderSidebar();
}, HOSTS_POLL_MS);

// ── The ⋯ menu ────────────────────────────────────────────────────────────
const hostMenu = document.createElement('div');
hostMenu.className = 'pane-menu';
hostMenu.id = 'host-menu';
hostMenu.setAttribute('role', 'menu');
hostMenu.hidden = true;
hostMenu.innerHTML = `
  <button type="button" role="menuitem" data-host-act="test">
    <span class="pane-menu-name">Test</span><span class="pane-menu-hint">Run sbx version over the link, time a round trip</span></button>
  <button type="button" role="menuitem" data-host-act="provision">
    <span class="pane-menu-name">Provision</span><span class="pane-menu-hint">Install Docker's sbx on the host</span></button>
  <button type="button" role="menuitem" data-host-act="edit">
    <span class="pane-menu-name">Edit</span><span class="pane-menu-hint">Address, agent port, trust, in Setup → Hosts</span></button>`;
document.body.appendChild(hostMenu);
let hostMenuFor = null;

function openHostMenu(host, anchor) {
  if (!hostMenu.hidden && hostMenuFor === host) { closeHostMenu(); return; }
  hostMenuFor = host;
  hostMenu.hidden = false;
  const r = anchor.getBoundingClientRect();
  const w = hostMenu.offsetWidth;
  hostMenu.style.left = `${Math.max(8, Math.min(r.left, window.innerWidth - w - 8))}px`;
  hostMenu.style.top = `${r.bottom + 4}px`;
  hostMenu.querySelector('button').focus();
}
function closeHostMenu() { hostMenu.hidden = true; hostMenuFor = null; }

document.addEventListener('click', e => { if (!hostMenu.hidden && !hostMenu.contains(e.target)) closeHostMenu(); });
document.addEventListener('keydown', e => { if (e.key === 'Escape' && !hostMenu.hidden) closeHostMenu(); });
hostMenu.addEventListener('click', e => {
  const btn = e.target.closest('[data-host-act]');
  if (!btn) return;
  const host = hostMenuFor;
  closeHostMenu();
  if (btn.dataset.hostAct === 'test') testHost(host);
  if (btn.dataset.hostAct === 'provision') openProvisionModal(host);
  if (btn.dataset.hostAct === 'edit') { setupOpen('hosts'); hostsEdit(host); }
});

async function testHost(host) {
  showToast(`Testing ${host}…`, 'progress');
  const r = await postJsonOrError(`/api/remote-hosts/${encodeURIComponent(host)}/test`, {});
  if (r.ok) showToast(`${host}: ${r.version || 'sbx answered'} · ${r.latency_ms} ms`, 'ok');
  else showToast(`${host}: ${r.error || 'the test failed'}`, 'error');
  await loadRemoteHosts();
  renderSidebar();
  if (hostsOverlay.open) renderHostsList();
}

// ── Provision ─────────────────────────────────────────────────────────────
// `sbxw host provision` installs sbx on the host and needs sudo there: the
// password is typed into a real terminal on this machine, never into this
// page, so the dialog hands over the command instead of running it.
const provOverlay = document.getElementById('provision-modal-overlay');
const provCmd = document.getElementById('provision-cmd');

const shellWord = w => /^[A-Za-z0-9_@%+=:,./~-]+$/.test(w) ? w : `'${w.replace(/'/g, `'\\''`)}'`;

function openProvisionModal(host) {
  provCmd.textContent = `${shellWord(sbxwBin)} host provision ${shellWord(host)}`;
  document.getElementById('provision-host').textContent = host;
  modalOpen(provOverlay);
  document.getElementById('provision-copy').focus();
}
function closeProvisionModal() { provOverlay.close(); }

document.getElementById('provision-copy').addEventListener('click', () => {
  if (!navigator.clipboard) { showToast('Clipboard unavailable — select the command and copy it', 'error'); return; }
  navigator.clipboard.writeText(provCmd.textContent)
    .then(() => showToast('Copied — paste it in a terminal on this machine', 'ok'))
    .catch(() => showToast('Copy failed — select the command and copy it', 'error'));
});
document.getElementById('provision-modal-close').addEventListener('click', closeProvisionModal);
document.getElementById('provision-modal-close2').addEventListener('click', closeProvisionModal);
wireModal(provOverlay, closeProvisionModal);

// ── Setup → Hosts ─────────────────────────────────────────────────────────
const hostsOverlay = document.getElementById('hosts-modal-overlay');
const hostsList = document.getElementById('hosts-list');
const hostsError = document.getElementById('hosts-error');
const hostsSave = document.getElementById('hosts-save');
const hostsKeyBox = document.getElementById('hosts-key');
const hf = id => document.getElementById(`hosts-f-${id}`);
// The host the form edits; null = adding one.
let hostsEditing = null;
// Fingerprints shown and waiting for "Trust and save".
let hostsPendingKey = null;

function openHostsModal() {
  showNote(hostsError, '');
  modalOpen(hostsOverlay);
  hostsReset();
  loadRemoteHosts().then(renderHostsList);
}
function closeHostsModal() { hostsOverlay.close(); }

function renderHostsList() {
  if (!remoteHosts.length) {
    hostsList.innerHTML = '<p class="repo-note">No remote host yet. Add one below.</p>';
    return;
  }
  hostsList.innerHTML = remoteHosts.map(h => {
    const link = hostLinkLabel(h);
    const target = h.ssh ? `ssh ${h.ssh}` : `${h.user ? h.user + '@' : ''}${h.host}${h.port ? ':' + h.port : ''}`;
    const trust = h.trust_secrets
      ? '<span class="skills-tag skills-tag-manual" title="Sent the Anthropic token, the OAuth kit and your skills">trusted with secrets</span>' : '';
    return `<div class="tpl-row host-row${hostsEditing === h.name ? ' editing' : ''}">
      <span class="dot host-link ${link.cls}" title="${escHtml(link.title)}"></span>
      <span class="tpl-tag" title="${escHtml(h.name)}">${escHtml(h.name)}</span>${trust}
      <span class="tpl-meta" title="${escHtml(target)}">${escHtml(target)} · ${escHtml(link.text)} · ${h.sandboxes} sandbox${h.sandboxes === 1 ? '' : 'es'}</span>
      <button class="tpl-rm host-edit" data-host-edit="${escHtml(h.name)}">Edit</button>
      <button class="tpl-rm" data-host-rm="${escHtml(h.name)}" title="Unregister this host">Remove</button>
    </div>`;
  }).join('');
}

function hostsSetMode(alias) {
  hf('mode-ssh').checked = alias;
  hf('mode-host').checked = !alias;
  document.getElementById('hosts-alias-fields').hidden = !alias;
  document.getElementById('hosts-direct-fields').hidden = alias;
}

function hostsReset() {
  hostsEditing = null;
  hostsPendingKey = null;
  hostsKeyBox.hidden = true;
  for (const id of ['name', 'ssh', 'host', 'user', 'port', 'key', 'agent-port', 'sbx-path']) hf(id).value = '';
  hf('name').disabled = false;
  hf('trust').checked = false;
  hostsSetMode(true);
  hostsTrustChanged();
  document.getElementById('hosts-form-title').textContent = 'Add a host';
  hostsSave.textContent = 'Add host';
  document.getElementById('hosts-cancel-edit').hidden = true;
  showNote(hostsError, '');
  renderHostsList();
}

function hostsEdit(name) {
  const h = remoteHosts.find(x => x.name === name);
  if (!h) { loadRemoteHosts().then(() => { if (remoteHosts.some(x => x.name === name)) hostsEdit(name); }); return; }
  hostsReset();
  hostsEditing = name;
  hf('name').value = h.name;
  hf('name').disabled = true;
  hostsSetMode(!!h.ssh);
  hf('ssh').value = h.ssh || '';
  hf('host').value = h.host || '';
  hf('user').value = h.user || '';
  hf('port').value = h.port || '';
  hf('key').value = h.key || '';
  hf('agent-port').value = h.agent_port && h.agent_port !== 7682 ? h.agent_port : '';
  hf('sbx-path').value = h.sbx_path || '';
  hf('trust').checked = !!h.trust_secrets;
  hostsTrustChanged();
  document.getElementById('hosts-form-title').textContent = `Edit ${name}`;
  hostsSave.textContent = 'Save';
  document.getElementById('hosts-cancel-edit').hidden = false;
  renderHostsList();
  hf(h.ssh ? 'ssh' : 'host').focus();
}

function hostsTrustChanged() {
  document.getElementById('hosts-trust-warn').classList.toggle('on', hf('trust').checked);
}

function hostsForm() {
  const alias = hf('mode-ssh').checked;
  const num = v => (v.trim() ? Number(v.trim()) : null);
  const text = v => v.trim() || null;
  return {
    name: hf('name').value.trim(),
    ssh: alias ? text(hf('ssh').value) : null,
    host: alias ? null : text(hf('host').value),
    user: alias ? null : text(hf('user').value),
    port: alias ? null : num(hf('port').value),
    key: alias ? null : text(hf('key').value),
    agent_port: num(hf('agent-port').value),
    sbx_path: text(hf('sbx-path').value),
    trust_secrets: hf('trust').checked,
  };
}

const sameTarget = (h, f) => (h.ssh || null) === f.ssh && (h.host || null) === f.host
  && (h.user || null) === f.user && (h.port || null) === f.port && (h.key || null) === f.key;

async function hostsSubmit(fingerprints) {
  const form = hostsForm();
  if (!form.name) { showNote(hostsError, 'Give the host a name'); hf('name').focus(); return; }
  for (const p of [form.port, form.agent_port]) {
    if (p !== null && !(Number.isInteger(p) && p >= 1 && p <= 65535)) { showNote(hostsError, 'Ports are 1 to 65535'); return; }
  }
  showNote(hostsError, '');
  hostsSave.disabled = true;
  try {
    const old = hostsEditing && remoteHosts.find(h => h.name === hostsEditing);
    // A new address needs its key trusted: ask for it first, unless already
    // confirmed. An edit that keeps the address does not.
    if (!fingerprints && !(old && sameTarget(old, form))) {
      const k = await postJsonOrError('/api/remote-hosts/key', form);
      if (!k.ok) { showNote(hostsError, k.error || 'Could not fetch the host key'); return; }
      if (!k.known) {
        hostsPendingKey = k.fingerprints || [];
        document.getElementById('hosts-key-name').textContent = k.known_as;
        document.getElementById('hosts-key-fps').textContent = hostsPendingKey.join('\n');
        hostsKeyBox.hidden = false;
        document.getElementById('hosts-key-trust').focus();
        return;
      }
    }
    const url = hostsEditing ? `/api/remote-hosts/${encodeURIComponent(hostsEditing)}/edit` : '/api/remote-hosts/add';
    const r = await postJsonOrError(url, { ...form, fingerprints: fingerprints || null });
    if (!r.ok) { showNote(hostsError, r.error || 'Could not save the host'); return; }
    showToast(hostsEditing ? `Saved ${form.name}` : `Added ${form.name} — try ⋯ → Test in the sidebar`, 'ok');
    await loadSandboxes();
    hostsReset();
  } finally {
    hostsSave.disabled = false;
  }
}

hostsSave.addEventListener('click', () => hostsSubmit(null));
document.getElementById('hosts-key-trust').addEventListener('click', () => {
  const fps = hostsPendingKey;
  hostsKeyBox.hidden = true;
  hostsPendingKey = null;
  if (fps) hostsSubmit(fps);
});
document.getElementById('hosts-key-cancel').addEventListener('click', () => {
  hostsKeyBox.hidden = true;
  hostsPendingKey = null;
  showNote(hostsError, 'Host key not trusted: nothing was saved');
});
document.getElementById('hosts-cancel-edit').addEventListener('click', hostsReset);
hf('mode-ssh').addEventListener('change', () => hostsSetMode(true));
hf('mode-host').addEventListener('change', () => hostsSetMode(false));
hf('trust').addEventListener('change', hostsTrustChanged);
// What was typed changes which key would be asked about.
document.getElementById('hosts-form').addEventListener('input', () => {
  if (hostsPendingKey) { hostsPendingKey = null; hostsKeyBox.hidden = true; }
});

hostsList.addEventListener('click', async e => {
  const edit = e.target.closest('[data-host-edit]');
  if (edit) { hostsEdit(edit.dataset.hostEdit); return; }
  const rm = e.target.closest('[data-host-rm]');
  if (!rm) return;
  const name = rm.dataset.hostRm;
  if (!confirm(`Remove host ${name} from hosts.toml?\n\nNothing on the host is touched; its known_hosts line stays.`)) return;
  rm.disabled = true;
  let r = await postJsonOrError(`/api/remote-hosts/${encodeURIComponent(name)}/rm`, { force: false });
  if (!r.ok && /still holds/.test(r.error || '')
      && confirm(`${r.error}\n\nRemove the host anyway? sbxw keeps their records on disk, and they are no longer listed.`)) {
    r = await postJsonOrError(`/api/remote-hosts/${encodeURIComponent(name)}/rm`, { force: true });
  }
  rm.disabled = false;
  if (!r.ok) { showNote(hostsError, r.error || 'Could not remove it'); return; }
  showToast(`Removed host ${name}`, 'ok');
  if (hostsEditing === name) hostsReset();
  await loadSandboxes();
  renderHostsList();
});

document.getElementById('hosts-reload').addEventListener('click', async () => {
  const r = await postJsonOrError('/api/remote-hosts/reload', {});
  if (!r.ok) { showNote(hostsError, r.error || 'Could not read hosts.toml'); return; }
  await loadSandboxes();
  renderHostsList();
  showToast('Read hosts.toml again', 'ok');
});

document.getElementById('hosts-modal-close').addEventListener('click', closeHostsModal);
document.getElementById('hosts-modal-close2').addEventListener('click', closeHostsModal);
wireModal(hostsOverlay, closeHostsModal);
