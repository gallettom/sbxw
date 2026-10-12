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

// The group hues, in the order panes use them: a thread keeps its colour from
// the card edge to every file it owns, so whose-is-what reads at a glance.
const PLAN_HUES = ['--c-group-1', '--c-group-2', '--c-group-3', '--c-group-4'];

// A goal's opening sentence, as the one line saying what the thread does, and
// the rest. A sentence ends at . ! or ? followed by a capital or an opening
// quote — so `web.rs` or `e.g. a file` does not cut it short.
function splitGoal(goal) {
  const text = (goal || '').trim();
  const m = text.match(/^([\s\S]+?[.!?])\s+(?=[A-Z("'`])/);
  return m ? [m[1], text.slice(m[0].length)] : [text, ''];
}

// "Owns" as one entry per file or area: the lead writes them as a list
// separated by commas, semicolons or lines.
function scopeItems(scope) {
  return (scope || '').split(/[,;\n]/).map(s => s.trim()).filter(Boolean);
}

// The reading half of a card, rebuilt from its inputs whenever editing ends.
function planViewHtml(t) {
  const [lead, rest] = splitGoal(t.goal);
  const items = scopeItems(t.scope);
  return `<h3 class="plan-card-title">${t.name ? escHtml(t.name) : '<span class="plan-unnamed">Unnamed thread</span>'}</h3>
      <p class="plan-lead">${lead ? escHtml(lead) : '<span class="plan-unnamed">No goal yet</span>'}</p>
      ${rest ? `<p class="plan-rest">${escHtml(rest)}</p>
      <button type="button" class="plan-more" aria-expanded="false" hidden>Show more</button>` : ''}
      <div class="plan-owns">
        <span class="plan-owns-label">Owns</span>
        ${items.length
          ? items.map(i => `<code class="plan-chip">${escHtml(i)}</code>`).join('')
          : '<span class="plan-unnamed">Nothing named</span>'}
      </div>`;
}

// One thread: a card to read, with the form that edits it kept beside it. The
// inputs are the card's source of truth — Approve reads them whether or not
// they were ever shown — so reading and editing cannot drift apart.
function planRowHtml(t = { name: '', goal: '', scope: '' }, i = 0) {
  return `<div class="plan-thread" style="--plan-accent: var(${PLAN_HUES[i % PLAN_HUES.length]})">
      <div class="plan-card-tools">
        <button type="button" class="plan-edit" aria-pressed="false">Edit</button>
        <button type="button" class="plan-remove" title="Remove this thread from the plan" aria-label="Remove thread">✕</button>
      </div>
      <div class="plan-view">${planViewHtml(t)}</div>
      <div class="plan-form">
        <label class="plan-label">Name
          <input class="field-input plan-name" type="text" maxlength="40" placeholder="name"
                 value="${escHtml(t.name)}" autocomplete="off" autocapitalize="off" spellcheck="false">
        </label>
        <label class="plan-label">Goal
          <textarea class="field-input plan-goal" placeholder="What this thread must do">${escHtml(t.goal)}</textarea>
        </label>
        <label class="plan-label">Owns
          <input class="field-input plan-scope" type="text" placeholder="Files or areas only this thread edits, separated by commas"
                 value="${escHtml(t.scope || '')}">
        </label>
      </div>
    </div>`;
}

function rowThread(row) {
  return {
    name: row.querySelector('.plan-name').value.trim(),
    goal: row.querySelector('.plan-goal').value.trim(),
    scope: row.querySelector('.plan-scope').value.trim(),
  };
}

// "Show more" only where the rest of a goal actually overflows its clamp,
// which can only be measured once the dialog is on screen.
function fitPlanMore(row) {
  const rest = row.querySelector('.plan-rest');
  const more = row.querySelector('.plan-more');
  if (rest && more) more.hidden = rest.scrollHeight <= rest.clientHeight + 1;
}

function setPlanEditing(row, on) {
  row.classList.toggle('editing', on);
  const btn = row.querySelector('.plan-edit');
  btn.setAttribute('aria-pressed', String(on));
  btn.textContent = on ? 'Done' : 'Edit';
  if (on) {
    row.querySelector('.plan-goal').focus();
  } else {
    row.querySelector('.plan-view').innerHTML = planViewHtml(rowThread(row));
    fitPlanMore(row);
  }
}

function setPlanError(msg) {
  planError.textContent = msg;
  planError.classList.toggle('hidden', !msg);
}

async function showPlan(plan) {
  planShown = plan.id;
  document.getElementById('plan-title').textContent = `Plan from the lead of ${plan.sandbox}`;
  document.getElementById('plan-summary').textContent = plan.summary || '';
  planRows.innerHTML = plan.threads.map((t, i) => planRowHtml(t, i)).join('');
  setPlanError('');
  planApprove.disabled = false;
  planApprove.textContent = 'Approve and start';
  planOverlay.classList.remove('hidden');
  planRows.querySelectorAll('.plan-thread').forEach(fitPlanMore);
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
  const row = e.target.closest('.plan-thread');
  if (!row) return;
  if (e.target.closest('.plan-remove')) {
    row.remove();
    updatePlanCount();
  } else if (e.target.closest('.plan-edit')) {
    setPlanEditing(row, !row.classList.contains('editing'));
  } else if (e.target.closest('.plan-more')) {
    const more = e.target.closest('.plan-more');
    const open = row.classList.toggle('expanded');
    more.setAttribute('aria-expanded', String(open));
    more.textContent = open ? 'Show less' : 'Show more';
  }
});
document.getElementById('plan-add').addEventListener('click', () => {
  const n = planRows.querySelectorAll('.plan-thread').length;
  planRows.insertAdjacentHTML('beforeend', planRowHtml(undefined, n));
  const row = planRows.lastElementChild;
  setPlanEditing(row, true);
  row.querySelector('.plan-name').focus();
  updatePlanCount();
});

planApprove.addEventListener('click', async () => {
  const id = planShown;
  if (!id) return;
  const rows = [...planRows.querySelectorAll('.plan-thread')];
  const threads = rows.map(rowThread);
  const bad = threads.findIndex(t => !THREAD_NAME.test(t.name));
  if (!threads.length) { setPlanError('Keep at least one thread, or refuse the plan'); return; }
  if (bad >= 0) {
    setPlanError(`"${threads[bad].name}": lower-case letters, digits and inner hyphens only`);
    if (!rows[bad].classList.contains('editing')) setPlanEditing(rows[bad], true);
    rows[bad].querySelector('.plan-name').focus();
    return;
  }
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
