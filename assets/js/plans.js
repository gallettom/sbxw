// ── Plans ─────────────────────────────────────────────────────────────────
// The main session, made lead with ★ Lead, proposes splitting the work into threads. The plan
// arrives here over the SSE stream and opens a dialog: every thread's name,
// goal and scope can be edited, threads removed or added, and nothing starts
// until Approve. On approval the daemon starts each thread on its briefing, and
// this tab opens a pane for each while the window has room. Threads report
// where they stand; that shows on their chips (see paintGroups in panes.js).

const plans = new Map();          // id -> plan, as the daemon last sent it
const panesOpenedFor = new Set(); // approved plans whose threads this tab put on screen
let planShown = null;             // id of the plan in the dialog

const planOverlay = document.getElementById('plan-overlay');
const planRows = document.getElementById('plan-threads');
const planError = document.getElementById('plan-error');
const planApprove = document.getElementById('plan-approve');

// Where thread `thread` of `sandbox` stands, from the latest approved plan
// that holds it — null when no plan does.
function planThreadState(sandbox, thread) {
  let best = null;
  for (const p of plans.values()) {
    if (p.sandbox !== sandbox || p.state !== 'approved') continue;
    const t = p.threads.find(x => x.name === thread);
    if (t && (!best || p.created_ms > best.created)) best = { created: p.created_ms, t };
  }
  return best ? best.t : null;
}

function pendingPlanFor(sandbox) {
  for (const p of plans.values()) if (p.sandbox === sandbox && p.state === 'pending') return p;
  return null;
}

// Sandboxes whose main session was made lead with ★ Lead, per the daemon —
// which forgets one as soon as that session is replaced (see /api/leads).
const leadSessions = new Set();

function refreshLeads() {
  fetch('/api/leads')
    .then(r => r.json())
    .then(list => {
      leadSessions.clear();
      list.forEach(n => leadSessions.add(n));
      schedulePaintGroups();
    })
    .catch(() => {});
}

// Shown the moment Lead is clicked rather than when the daemon confirms, so
// the pane changes under the click; `refreshLeads` puts it right if it failed.
function markLeading(sandbox) {
  leadSessions.add(sandbox);
  schedulePaintGroups();
}

// Is the main session of `sandbox` the lead: made lead with ★ Lead, or
// leading a plan — one waiting on a person, or approved with a thread still at
// work?
function isLeading(sandbox) {
  if (leadSessions.has(sandbox)) return true;
  for (const p of plans.values()) {
    if (p.sandbox !== sandbox) continue;
    if (p.state === 'pending') return true;
    if (p.state === 'approved' && p.threads.some(t => !['done', 'failed'].includes(t.state))) return true;
  }
  return false;
}

// "Review plan" on the main pane, shown while its plan waits on a person —
// the way back to a dialog closed with Later.
function refreshPlanButtons() {
  panes.forEach((p, i) => {
    const btn = document.getElementById(`pplan-${i}`);
    if (btn) btn.hidden = !(i < paneCount && p.sandbox && !p.thread && pendingPlanFor(p.sandbox));
  });
}

function openPendingPlan(sandbox) {
  const p = pendingPlanFor(sandbox);
  if (p) showPlan(p);
}

function planRowHtml(t = { name: '', goal: '', scope: '' }) {
  return `<div class="plan-thread">
      <input class="field-input plan-name" type="text" maxlength="40" placeholder="name"
             value="${escHtml(t.name)}" autocomplete="off" autocapitalize="off" spellcheck="false" aria-label="Thread name">
      <button type="button" class="plan-remove" title="Remove this thread from the plan" aria-label="Remove thread">✕</button>
      <label class="plan-label">Goal
        <textarea class="field-input plan-goal" placeholder="What this thread must do">${escHtml(t.goal)}</textarea>
      </label>
      <label class="plan-label">Owns
        <input class="field-input plan-scope" type="text" placeholder="Files or areas only this thread edits"
               value="${escHtml(t.scope || '')}">
      </label>
    </div>`;
}

function setPlanError(msg) {
  planError.textContent = msg;
  planError.classList.toggle('hidden', !msg);
}

async function showPlan(plan) {
  planShown = plan.id;
  document.getElementById('plan-title').textContent = `Plan from the lead of ${plan.sandbox}`;
  document.getElementById('plan-summary').textContent = plan.summary || '';
  planRows.innerHTML = plan.threads.map(planRowHtml).join('');
  setPlanError('');
  planApprove.disabled = false;
  planApprove.textContent = 'Approve and start';
  planOverlay.classList.remove('hidden');
  updatePlanCount();
}

// How many sessions approving would add, against what the sandbox has open.
let planBudget = null;
async function updatePlanCount() {
  const plan = plans.get(planShown);
  if (!plan) return;
  const n = planRows.querySelectorAll('.plan-thread').length;
  if (!planBudget || planBudget.sandbox !== plan.sandbox) {
    planBudget = { sandbox: plan.sandbox, open: null, max: null };
    try {
      const r = await (await fetch(`/api/sandboxes/${encodeURIComponent(plan.sandbox)}/threads`)).json();
      if (r.ok) Object.assign(planBudget, { open: r.open, max: r.max });
    } catch (_) {}
  }
  const { open, max } = planBudget;
  document.getElementById('plan-count').textContent =
    `${n} thread${n === 1 ? '' : 's'}` + (max ? ` · ${open} of ${max} Claude sessions open now` : '');
}

function closePlan() {
  planOverlay.classList.add('hidden');
  planShown = null;
  planBudget = null;
}

// Open a pane for each thread of an approved plan, beside its sandbox's
// panes, while the window has room. A thread that does not fit still runs;
// ⑂ Thread lists it to open later.
function openPlanPanes(plan) {
  let opened = 0;
  for (const t of plan.threads) {
    const id = sessionIdentity(plan.sandbox, 'thread', t.name);
    if (panes.slice(0, paneCount).some(p => sessionIdentity(p.sandbox, p.mode, p.thread) === id)) continue;
    let target = panes.slice(0, paneCount).findIndex(p => !p.sandbox);
    if (target < 0 && paneCount < computeMaxPanes()) { setLayout(paneCount + 1); target = paneCount - 1; }
    if (target < 0) break;
    connectPane(target, plan.sandbox, 'thread', t.name);
    opened++;
  }
  if (opened) regroupPanes();
}

function ingestPlan(plan, seed = false) {
  if (!plan || !plan.id) return;
  const prev = plans.get(plan.id);
  plans.set(plan.id, plan);
  // A new proposal asks for attention at once; one found on reload too, unless
  // a dialog is already up — its pane's Review plan button leads back to it.
  if (plan.state === 'pending' && !prev && planOverlay.classList.contains('hidden')) showPlan(plan);
  if (plan.state === 'approved' && !seed && prev?.state === 'pending' && !panesOpenedFor.has(plan.id)) {
    panesOpenedFor.add(plan.id);
    openPlanPanes(plan);
  }
  if (planShown === plan.id && plan.state !== 'pending') closePlan();
  schedulePaintGroups();
}

planRows.addEventListener('click', e => {
  const rm = e.target.closest('.plan-remove');
  if (!rm) return;
  rm.closest('.plan-thread').remove();
  updatePlanCount();
});
document.getElementById('plan-add').addEventListener('click', () => {
  planRows.insertAdjacentHTML('beforeend', planRowHtml());
  planRows.lastElementChild.querySelector('.plan-name').focus();
  updatePlanCount();
});

planApprove.addEventListener('click', async () => {
  const id = planShown;
  if (!id) return;
  const threads = [...planRows.querySelectorAll('.plan-thread')].map(row => ({
    name: row.querySelector('.plan-name').value.trim(),
    goal: row.querySelector('.plan-goal').value.trim(),
    scope: row.querySelector('.plan-scope').value.trim(),
  }));
  const bad = threads.find(t => !THREAD_NAME.test(t.name));
  if (!threads.length) { setPlanError('Keep at least one thread, or refuse the plan'); return; }
  if (bad) { setPlanError(`"${bad.name}": lower-case letters, digits and inner hyphens only`); return; }
  planApprove.disabled = true;
  planApprove.textContent = 'Starting…';
  setPlanError('');
  let res;
  try {
    res = await (await fetch(`/api/plans/${encodeURIComponent(id)}/approve`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ threads }),
    })).json();
  } catch (e) {
    res = { ok: false, error: e.message };
  }
  if (!res.ok) {
    setPlanError(res.error || 'could not approve the plan');
    planApprove.disabled = false;
    planApprove.textContent = 'Approve and start';
    return;
  }
  // The `plan` event that follows opens the panes; the reply carries the same
  // plan, so a dropped event does not lose them.
  ingestPlan(res.plan);
});

document.getElementById('plan-refuse').addEventListener('click', async () => {
  const id = planShown;
  if (!id) return;
  let res;
  try {
    res = await (await fetch(`/api/plans/${encodeURIComponent(id)}/refuse`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: '{}',
    })).json();
  } catch (e) {
    res = { ok: false, error: e.message };
  }
  if (!res.ok) { setPlanError(res.error || 'could not refuse the plan'); return; }
  ingestPlan(res.plan);
});

document.getElementById('plan-later').addEventListener('click', closePlan);
document.getElementById('plan-x').addEventListener('click', closePlan);
planOverlay.addEventListener('click', e => { if (e.target === planOverlay) closePlan(); });
document.addEventListener('keydown', e => {
  if (e.key === 'Escape' && !planOverlay.classList.contains('hidden')) closePlan();
});

fetch('/api/plans')
  .then(r => r.json())
  .then(list => list.forEach(p => ingestPlan(p, true)))
  .catch(() => {});

refreshLeads();
sbxwStream.addEventListener('lead', ev => markLeading(ev.data));

sbxwStream.addEventListener('plan', ev => {
  try { ingestPlan(JSON.parse(ev.data)); } catch (_) {}
});
