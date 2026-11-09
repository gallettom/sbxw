// ── Approvals: what sbx waits for a person to decide ─────────────────────
//
// `sbx policy approval` (0.51) keeps one list per machine: a host a sandbox
// reached that no rule allows, a kit's request for a host port (which lapses
// after 45 s)… Each offers its own options; this dialog shows one sandbox's
// and sends back the option clicked. The daemon checks the request is still
// pending and offers that option before answering.
const approvalsOverlay = document.getElementById('approvals-modal-overlay');
const approvalsBody    = document.getElementById('approvals-body');
const approvalsError   = document.getElementById('approvals-error');
let approvalsFor = null;
let approvalsTimer = null;

const APPROVAL_KINDS = { network: 'Network', port: 'Host port' };

function openApprovalsModal(name) {
  approvalsFor = name;
  document.getElementById('approvals-modal-title').textContent = `Waiting for you — ${name}`;
  showNote(approvalsError, '');
  approvalsOverlay.classList.remove('hidden');
  loadApprovals();
  // A port request lapses in 45 s: keep the list honest while it is open.
  clearInterval(approvalsTimer);
  approvalsTimer = setInterval(loadApprovals, 5000);
}
function closeApprovalsModal() {
  approvalsOverlay.classList.add('hidden');
  clearInterval(approvalsTimer);
  approvalsFor = null;
  loadSandboxes();
}

async function loadApprovals() {
  if (!approvalsFor) return;
  let r;
  try { r = await (await fetch('/api/approvals')).json(); }
  catch (e) { r = { ok: false, error: e.message }; }
  if (!r.ok) { approvalsBody.innerHTML = `<p class="repo-note">${escHtml(r.error || 'Could not read what sbx is waiting for')}</p>`; return; }
  const mine = (r.approvals || []).filter(a => a.sandbox === approvalsFor);
  approvalsBody.innerHTML = mine.length ? mine.map(a => `<div class="mac-card">
      <div class="mac-card-head">
        <code class="mac-card-title">${escHtml(a.title)}</code>
        <span class="mac-pills"><span class="mac-pill mac-pill-warn"><span class="mac-dot"></span>${escHtml(APPROVAL_KINDS[a.kind] || a.kind || 'request')}</span></span>
      </div>
      <p class="mac-card-note">${escHtml(a.detail).replace(/\n/g, ' · ')}${a.created_at ? ` · ${escHtml(timeAgo(a.created_at) || '')}` : ''}</p>
      <div class="mac-actions">${a.options.map((o, i) => `<button class="${o.id === 'allow' ? 'btn-create' : 'mac-btn'}" data-approval="${escHtml(a.id)}" data-option="${escHtml(o.id)}">${escHtml(o.label || o.id)}</button>`).join('')}</div>
    </div>`).join('')
    : '<p class="repo-note">Nothing waiting — sbx has no pending request for this sandbox.</p>';
}

approvalsBody.addEventListener('click', async e => {
  const btn = e.target.closest('[data-approval]');
  if (!btn) return;
  showNote(approvalsError, '');
  approvalsBody.querySelectorAll('button').forEach(b => { b.disabled = true; });
  const r = await postJsonOrError('/api/approvals/respond', { id: btn.dataset.approval, option: btn.dataset.option });
  if (!r.ok) showNote(approvalsError, r.error || 'sbx did not take the answer');
  loadApprovals();
});

document.getElementById('approvals-modal-close').addEventListener('click', closeApprovalsModal);
document.getElementById('approvals-modal-close2').addEventListener('click', closeApprovalsModal);
approvalsOverlay.addEventListener('click', e => { if (e.target === approvalsOverlay) closeApprovalsModal(); });
