// ── SSH connection details ────────────────────────────────────────────────
//
// Content only: the card's lifecycle (anchor, placement, Escape, outside-click,
// resize, mutual exclusion) comes from `bindPopover` in util.js.
const sshPop = document.getElementById('ssh-pop');

const sshCard = bindPopover(sshPop, {
  closeButton: 'ssh-pop-close',
  onOpen: sandbox => fillSshFields(sandbox),
});

function toggleSshPop(sandbox, anchor) { sshCard.toggle(anchor, sandbox); }
function closeSshPop() { sshCard.close(); }

function fillSshFields(sandbox) {
  const host = `${sandbox}.sbx`;
  // sbx publishes each sandbox as `<name>.sbx` through the managed block that
  // `sbx setup ssh` (i.e. `sbxw ssh --setup`) writes into ~/.ssh/config — it
  // owns the user, the key and the ProxyCommand, so we must not invent any of
  // them, and the client must not be given a port or an identity file either.
  const rows = [
    { key: 'Name',          value: sandbox, hint: 'Any label; the sandbox name keeps them straight.' },
    { key: 'SSH Host',      value: host,    hint: 'The alias from ~/.ssh/config — no user@ needed.' },
    { key: 'SSH Port',      empty: 'leave empty', hint: 'Not port 22: the managed block dials a ProxyCommand.' },
    { key: 'Identity File', empty: 'leave empty', hint: 'The managed block supplies the key.' },
    { key: 'Terminal',      value: `ssh ${host}`, hint: 'The same connection, from a shell.' },
  ];

  document.getElementById('ssh-pop-title').textContent = `SSH — ${sandbox}`;
  const box = document.getElementById('ssh-fields');
  box.replaceChildren();
  for (const row of rows) {
    const el = document.createElement('div');
    el.className = 'ssh-row' + (row.empty ? ' ssh-empty' : '');
    el.title = row.hint;

    const key = document.createElement('span');
    key.className = 'ssh-key';
    key.textContent = row.key;

    const val = document.createElement('div');
    val.className = 'ssh-val';
    // textContent, never innerHTML: a sandbox name is validated elsewhere, but
    // this dialog must not be the one place where that stops being true.
    val.textContent = row.empty || row.value;

    el.append(key, val);
    if (!row.empty) {
      const btn = document.createElement('button');
      btn.className = 'ssh-copy';
      btn.type = 'button';
      btn.textContent = 'Copy';
      btn.setAttribute('aria-label', `Copy ${row.key}`);
      btn.addEventListener('click', () => copyField(btn, row.value));
      el.append(btn);
    }
    box.append(el);
  }
}
