// ── SSH connection details ────────────────────────────────────────────────
const sshPop = document.getElementById('ssh-pop');
/// The button the popover is currently hanging off, or null when it is closed.
/// Doubles as the "ignore this click" test for the close-on-outside handler.
let sshAnchor = null;

/// Copy `text`, reporting on the button itself rather than only in a toast:
/// with several copy buttons in one dialog, "which one did I just press?" is
/// the question a shared toast cannot answer.
function copyField(btn, text) {
  const done = ok => {
    btn.textContent = ok ? 'Copied' : 'Failed';
    btn.classList.toggle('copied', ok);
    setTimeout(() => { btn.textContent = 'Copy'; btn.classList.remove('copied'); }, 1400);
  };
  // The clipboard API needs a secure context; sbxw is served over plain HTTP on
  // a loopback name, which qualifies — but not if the page was reached by LAN
  // IP, so the failure is real and has to say what to do instead.
  if (!navigator.clipboard) {
    done(false);
    showToast('Clipboard needs localhost or HTTPS — select the value and copy it', 'error');
    return;
  }
  navigator.clipboard.writeText(text).then(() => done(true)).catch(() => {
    done(false);
    showToast('Copy failed — select the value and copy it', 'error');
  });
}

/// Place the popover under its button, flipping above when the viewport floor
/// is closer than the popover is tall, and clamping to the window either way.
/// Measured after the content is built and while `visibility: hidden`, since a
/// `display: none` element has no size to measure.
function positionSshPop(anchor) {
  // A layout change rebuilds panes, which can take the button out of the
  // document while its card is up. Nothing to hang off, so the card goes too.
  if (!anchor.isConnected) {
    closeSshPop();
    return;
  }
  sshPop.style.visibility = 'hidden';
  sshPop.classList.remove('hidden');
  const a = anchor.getBoundingClientRect();
  const p = sshPop.getBoundingClientRect();
  const gap = 6;
  const margin = 8;

  let top = a.bottom + gap;
  if (top + p.height > window.innerHeight - margin) {
    top = Math.max(margin, a.top - gap - p.height);
  }
  // Right-aligned on the button: it sits at the right end of the pane bar, so
  // growing leftwards is what keeps the card on screen.
  let left = Math.min(a.right - p.width, window.innerWidth - p.width - margin);
  left = Math.max(margin, left);

  sshPop.style.top = `${Math.round(top)}px`;
  sshPop.style.left = `${Math.round(left)}px`;
  sshPop.style.visibility = '';
}

function toggleSshPop(sandbox, anchor) {
  if (sshAnchor === anchor && !sshPop.classList.contains('hidden')) {
    closeSshPop();
    return;
  }
  openSshPop(sandbox, anchor);
}

function openSshPop(sandbox, anchor) {
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
  sshAnchor = anchor;
  positionSshPop(anchor);
}

function closeSshPop() {
  sshPop.classList.add('hidden');
  sshAnchor = null;
}

document.getElementById('ssh-pop-close').addEventListener('click', closeSshPop);
// Close on a click anywhere else. `pointerdown` on the capture phase so it
// still fires over the terminal, which swallows its own mouse events; the
// anchor is excluded because its own handler already toggles.
document.addEventListener('pointerdown', e => {
  if (!sshAnchor) return;
  if (sshPop.contains(e.target) || sshAnchor.contains(e.target)) return;
  closeSshPop();
}, true);
document.addEventListener('keydown', e => {
  if (e.key === 'Escape' && sshAnchor) closeSshPop();
});
// A popover pinned to fixed coordinates goes stale the moment the layout moves.
// Re-place it rather than leaving a card floating away from its button.
window.addEventListener('resize', () => { if (sshAnchor) positionSshPop(sshAnchor); });
