// ── Setup → Mac: the virtual Mac sandboxes borrow (sbxw:terminal-mac) ──────
//
// One panel over `GET /api/mac`: whether terminal-mac is installed (the
// pinned release, SHA-256 checked), the VM's state, the guest's one-time
// setup as a checklist, who holds the lease, and the buttons the CLI's
// `sbxw mac` has. Install and boot take minutes — a first boot downloads
// macOS — so they start in the daemon and this follows `busy` on the `mac`
// event until they finish.
const macOverlay = document.getElementById('mac-modal-overlay');
const macBody    = document.getElementById('mac-body');
const macError   = document.getElementById('mac-error');

// The guest's setup, which terminal-mac cannot automate (same as `sbxw mac status`).
const MAC_STEPS = [
  'In the VM window, finish macOS Setup Assistant and create the guest’s user account. Sign in to no personal or work account: every sandbox shares this guest.',
  'In the guest, open Finder → <code>/Volumes/My Shared Files/tools</code> → <code>TerminalMacGuest.app</code>.',
  'Use its two buttons to grant <strong>Screen Recording</strong> and <strong>Accessibility</strong> in the guest’s System Settings (reopen it if macOS asks).',
  'Approve its login item if macOS asks, and keep the guest logged in.',
  'Turn on <strong>automatic login</strong> for that account (System Settings → Users &amp; Groups; FileVault must be off in the guest), so a VM sbxw boots reaches the desktop and its helper without you.',
  'Stop the guest from sleeping or locking: in System Settings → Lock Screen, set <em>Turn display off when inactive</em> and <em>Require password after the screen saver begins or display is turned off</em> to <strong>Never</strong>. A guest whose display sleeps comes back on its lock screen, where the agent cannot go on.',
];

let macState = null;
let macSubscribed = false;

function openMacModal() {
  showNote(macError, '');
  // main.js opens the stream after setup.js has read this file's functions,
  // so the subscription is taken on first open, not at load.
  if (!macSubscribed && typeof sbxwStream !== 'undefined') {
    macSubscribed = true;
    sbxwStream.addEventListener('mac', () => { if (!macOverlay.classList.contains('hidden')) loadMac(); });
  }
  macOverlay.classList.remove('hidden');
  loadMac();
}
function closeMacModal() { macOverlay.classList.add('hidden'); }

async function loadMac() {
  let r;
  try { r = await (await fetch('/api/mac')).json(); }
  catch (e) { r = { ok: false, error: e.message }; }
  if (!r.ok) { macBody.innerHTML = `<p class="repo-note">Could not read the Mac’s state: ${escHtml(r.error || 'unknown error')}</p>`; return; }
  macState = r.mac;
  renderMac();
}

function macButton(action, label, kind) {
  const busy = !!macState?.busy;
  const cls = { primary: 'btn-create', danger: 'mac-btn mac-btn-danger' }[kind] || 'mac-btn';
  return `<button class="${cls}" data-mac="${action}"${busy ? ' disabled' : ''}>${label}</button>`;
}

// One pill per fact: a coloured dot and a word. Tones map to the app's own
// tokens (see .mac-pill in app.css), so light and dark follow the rest.
function macPill(tone, text, title) {
  return `<span class="mac-pill mac-pill-${tone}"${title ? ` title="${escHtml(title)}"` : ''}><span class="mac-dot"></span>${text}</span>`;
}

const MAC_VM_TONE = { running: 'ok', paused: 'warn', starting: 'busy', stopping: 'busy', stopped: 'off', missing: 'off', unknown: 'off' };

function renderMac() {
  const m = macState;
  if (!m) return;
  const lead = `<p class="help-lead">A real macOS guest, run on this Mac by
    <a href="https://github.com/govijr/terminal-mac" target="_blank" rel="noopener">terminal-mac</a>,
    that sandboxes with the <code>sbxw:terminal-mac</code> kit can drive: Safari, Mac apps, native windows.
    One sandbox at a time, for an hour at most when another asks. Starting, resuming,
    lending a VM sbxw did not start, and publishing a port need your yes; clicks and typing do not.</p>`;
  if (!m.supported) {
    macBody.innerHTML = `<div class="mac-card">${macPill('off', 'not available')}
      <p class="mac-card-note">The virtual Mac needs an Apple silicon Mac (macOS 14 or later). This host is not one, so the <code>mac_*</code> tools say so and do nothing.</p></div>`;
    return;
  }
  const rows = [lead];
  if (m.busy) rows.push(`<div class="mac-banner mac-banner-busy"><span class="relay-spinner"></span>${escHtml(m.busy)}…</div>`);
  if (m.error) rows.push(`<div class="mac-banner mac-banner-error">${escHtml(m.error)}</div>`);

  if (!m.installed) {
    rows.push(`<div class="mac-card">
      <div class="mac-card-head"><span class="mac-card-title">terminal-mac</span>${macPill('off', 'not installed')}</div>
      <p class="mac-card-note">sbxw installs release <code>${escHtml(m.pinned)}</code> under <code>~/.sbxw/terminal-mac</code>, its SHA-256 checked.
        It is an early release candidate from one author, self-signed and not notarised by Apple.</p>
      <div class="mac-actions">${macButton('install', 'Install terminal-mac', 'primary')}</div>
    </div>`);
    macBody.innerHTML = rows.join('');
    return;
  }

  const running = m.state === 'running';
  const pills = [macPill(MAC_VM_TONE[m.state] || 'off', escHtml(m.state))];
  if (running) pills.push(m.guest_ready ? macPill('ok', 'guest ready') : macPill('warn', 'guest not ready', 'Finish the one-time setup below'));
  pills.push(m.holder ? macPill('held', `held by ${escHtml(m.holder)}`) : macPill('off', 'free'));

  const facts = [
    ['Release', `<code>${escHtml(m.pinned)}</code>`],
    ['Ports', m.ports?.length ? m.ports.map(p => `<code>${escHtml(p)}</code>`).join(' ') : '<span class="mac-none">none published</span>'],
  ];

  const acts = [];
  const stopping = m.state === 'stopping';
  if (!running && !stopping) acts.push(macButton('up', m.state === 'paused' ? 'Resume' : m.state === 'missing' ? 'Create and start' : 'Start', 'primary'));
  // A shutdown that hangs is usually a dialog in the guest: show it.
  if (running || m.state === 'paused' || stopping) acts.push(macButton('show', 'Open its window'));
  if (running) acts.push(macButton('pause', 'Pause'));
  if (m.holder) acts.push(macButton('take-back', `Take it back from ${escHtml(m.holder)}`));
  const end = [macButton('refresh', 'Refresh')];
  if (running || m.state === 'paused') end.unshift(macButton('stop', 'Shut down', 'danger'));
  // A shutdown that hangs: sbxw forces its own after force_stop_after_minutes;
  // this is the person's way to do it sooner, or for one sbxw did not ask for.
  const stoppingFor = m.stop_requested_ms ? Date.now() - m.stop_requested_ms : Infinity;
  if (stopping && stoppingFor >= 2 * 60_000) end.unshift(macButton('force-off', 'Force off', 'danger'));

  rows.push(`<div class="mac-card">
    <div class="mac-card-head">
      <span class="mac-card-title">VM <code>${escHtml(m.vm)}</code></span>
      <span class="mac-pills">${pills.join('')}</span>
    </div>
    <dl class="mac-facts">${facts.map(([k, v]) => `<dt>${k}</dt><dd>${v}</dd>`).join('')}</dl>
    <div class="mac-actions">${acts.join('')}<span class="mac-actions-end">${end.join('')}</span></div>
  </div>`);

  // Cards waiting on you. sbxw's sentence and the sandbox's reason arrive as
  // separate fields and are shown apart, so a reason cannot pass for sbxw's words.
  if (m.pending?.length) {
    rows.push(`<div class="mac-card mac-card-warn">
      <div class="mac-card-head"><span class="mac-card-title">Waiting on you</span></div>
      ${m.pending.map(p => `
        <p class="mac-card-note"><strong>${escHtml(p.action)}</strong></p>
        ${p.reason
          ? `<div class="relay-section-label">Reason given by ${escHtml(p.sandbox)} (its own words, not checked by sbxw)</div>
             <pre class="relay-quote">${escHtml(p.reason)}</pre>`
          : `<p class="mac-card-note">${escHtml(p.sandbox)} gave no reason.</p>`}`).join('')}
      <p class="mac-card-note">Approve or refuse each on its <em>Virtual Mac request</em> card.</p></div>`);
  }
  if (m.state === 'missing') {
    rows.push(`<p class="repo-note">Starting creates the VM: macOS is downloaded from Apple (several GB) and installed, then you finish its setup once.</p>`);
  }
  if (running && !m.guest_ready) {
    rows.push(`<div class="mac-card mac-card-warn">
      <div class="mac-card-head"><span class="mac-card-title">One-time guest setup</span></div>
      <ol class="mac-steps">${MAC_STEPS.map(st => `<li>${st}</li>`).join('')}</ol>
      <p class="mac-card-note">This panel notices on its own when the guest is ready.</p></div>`);
  }
  const st = m.settings || {};
  rows.push(`<p class="mac-foot">A lease lapses after <strong>${st.idle_release_minutes} min</strong> idle.
    A VM sbxw started is paused after <strong>${st.pause_after_minutes} min</strong> unheld and shut down after
    <strong>${st.stop_after_minutes} min</strong>, forced if the shutdown still hangs <strong>${st.force_stop_after_minutes} min</strong> later. Set these under <code>[mac]</code> in <code>sbxw.toml</code>.</p>`);
  macBody.innerHTML = rows.join('');
}

macBody.addEventListener('click', async e => {
  const btn = e.target.closest('[data-mac]');
  if (!btn) return;
  const action = btn.dataset.mac;
  if (action === 'refresh') { loadMac(); return; }
  if (action === 'force-off' && !confirm('Force the virtual Mac off now?\n\nIt is powered off at once: anything unsaved in the guest is lost.')) return;
  if (action === 'stop' && !confirm('Ask the virtual Mac to shut down?\n\nA sandbox using it loses it, and the ports opened for it are closed.')) return;
  showNote(macError, '');
  btn.disabled = true;
  const r = await postJsonOrError(`/api/mac/${action}`, {});
  if (!r.ok) showNote(macError, r.error || 'sbxw refused that');
  else if (r.text) showToast(r.text);
  loadMac();
});

// The `mac` event says when the lease or a job moved. While the guest is
// being set up nothing fires, so poll then.
setInterval(() => {
  if (macOverlay.classList.contains('hidden') || !macState) return;
  if (macState.busy || (macState.state === 'running' && !macState.guest_ready) || macState.state === 'starting') loadMac();
}, 5000);

document.getElementById('mac-modal-close').addEventListener('click', closeMacModal);
document.getElementById('mac-modal-close2').addEventListener('click', closeMacModal);
macOverlay.addEventListener('click', e => { if (e.target === macOverlay) closeMacModal(); });
document.addEventListener('keydown', e => {
  if (e.key === 'Escape' && !macOverlay.classList.contains('hidden')) closeMacModal();
});
