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
// The git repos of the shown plan's workspace, from /repos: `{sandbox, ok,
// workspace, repos}`, or null until they arrive.
let planRepos = null;

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

// A plan whose lead mode was ended is over, whatever state it was left in.
function pendingPlanFor(sandbox) {
  for (const p of plans.values()) if (p.sandbox === sandbox && p.state === 'pending' && !p.closed) return p;
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
    if (p.sandbox !== sandbox || p.closed) continue;
    if (p.state === 'pending') return true;
    if (p.state === 'approved' && p.threads.some(t => !['done', 'failed'].includes(t.state))) return true;
  }
  return false;
}

// Lead mode ended, here or in another tab: the daemon has closed its plans
// (they come back with `closed: true`), and this forgets the ★ Lead click.
function forgetLeading(sandbox) {
  leadSessions.delete(sandbox);
  schedulePaintGroups();
}

// ── End lead mode ──
// The lit ★ Lead button ends it, once a person has read what that leaves
// behind: threads at work keep running, but nobody reads their reports.
const leadEndOverlay = document.getElementById('lead-end-overlay');
const leadEndConfirm = document.getElementById('lead-end-confirm');
const leadEndError = document.getElementById('lead-end-error');
let leadEndAsk = null; // { idx, sandbox }

function askEndLead(idx) {
  const sandbox = panes[idx]?.sandbox;
  if (!sandbox) return;
  leadEndAsk = { idx, sandbox };
  let running = 0;
  for (const p of plans.values()) {
    if (p.sandbox !== sandbox || p.closed || p.state !== 'approved') continue;
    running += p.threads.filter(t => !['done', 'failed'].includes(t.state)).length;
  }
  const pending = pendingPlanFor(sandbox);
  const lines = [`The main session of ${sandbox} stops leading and goes back to being an ordinary session.`];
  if (running) {
    lines.push(`${running} thread${running === 1 ? ' is' : 's are'} still at work. ${running === 1 ? 'It keeps' : 'They keep'} running — close ${running === 1 ? 'it' : 'them'} with Close thread — but ${running === 1 ? 'its reports' : 'their reports'} will no longer reach the main session.`);
  }
  if (pending) lines.push('The plan waiting for your approval will be refused.');
  document.getElementById('lead-end-body').innerHTML =
    lines.map((l, i) => `<p class="${i ? 'lead-end-warn' : 'thread-close-lead'}">${escHtml(l)}</p>`).join('');
  showNote(leadEndError, '');
  leadEndConfirm.disabled = false;
  leadEndConfirm.textContent = 'End lead mode';
  leadEndOverlay.classList.remove('hidden');
  leadEndConfirm.focus();
}

function closeLeadEnd() { leadEndOverlay.classList.add('hidden'); leadEndAsk = null; }

leadEndConfirm.addEventListener('click', async () => {
  const ask = leadEndAsk;
  if (!ask) return;
  leadEndConfirm.disabled = true;
  leadEndConfirm.textContent = 'Ending…';
  const res = await postJsonOrError(`/api/sandboxes/${encodeURIComponent(ask.sandbox)}/lead/end`, {});
  if (leadEndAsk !== ask) return;
  if (!res.ok) {
    // Said in the dialog, and in the pane the way a failed Lead is.
    showNote(leadEndError, res.error || 'could not end lead mode');
    leadEndConfirm.disabled = false;
    leadEndConfirm.textContent = 'End lead mode';
    const pane = panes[ask.idx];
    if (pane?.sandbox === ask.sandbox) pane.term.write(`\r\n\x1b[31m[sbxw] End lead mode: ${res.error}\x1b[0m\r\n`);
    return;
  }
  closeLeadEnd();
  forgetLeading(ask.sandbox);
});
document.getElementById('lead-end-cancel').addEventListener('click', closeLeadEnd);
document.getElementById('lead-end-x').addEventListener('click', closeLeadEnd);
leadEndOverlay.addEventListener('click', e => { if (e.target === leadEndOverlay) closeLeadEnd(); });
document.addEventListener('keydown', e => {
  if (e.key === 'Escape' && !leadEndOverlay.classList.contains('hidden')) closeLeadEnd();
});

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

// Does a thread without a repo have to be given one before approval? Only
// when the workspace holds several and none of them is the obvious default.
function planNeedsRepo() {
  return !!planRepos?.ok && planRepos.repos.length > 1 && defaultRepo(planRepos.repos) === undefined;
}

// The card's repo line: the repo as a chip, or the flag saying one is needed.
function planRepoHtml(t) {
  if (t.repo != null) {
    return `<div class="plan-repo"><span class="plan-owns-label">In</span>
        <code class="plan-repo-chip">${escHtml(repoLabel(t.repo, planRepos?.workspace))}</code></div>`;
  }
  if (planNeedsRepo()) {
    return `<div class="plan-repo flagged">No repository yet: choose one with Edit</div>`;
  }
  return '';
}

// The reading half of a card, rebuilt from its inputs whenever editing ends.
function planViewHtml(t) {
  const [lead, rest] = splitGoal(t.goal);
  const items = scopeItems(t.scope);
  return `<h3 class="plan-card-title">${t.name ? escHtml(t.name) : '<span class="plan-unnamed">Unnamed thread</span>'}</h3>
      ${planRepoHtml(t)}
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
// they were ever shown — so reading and editing cannot drift apart. The repo
// is kept in `data-repo`, absent while the thread has none: '' is a repo too,
// the workspace itself.
function planRowHtml(t = { name: '', goal: '', scope: '' }, i = 0) {
  const repo = typeof t.repo === 'string' ? ` data-repo="${escHtml(t.repo)}"` : '';
  return `<div class="plan-thread" style="--plan-accent: var(${PLAN_HUES[i % PLAN_HUES.length]})"${repo}>
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
        <label class="plan-label">Repository
          <select class="field-input plan-repo-select"></select>
        </label>
        <label class="plan-label">Owns
          <input class="field-input plan-scope" type="text" placeholder="Files or areas only this thread edits, separated by commas"
                 value="${escHtml(t.scope || '')}">
        </label>
      </div>
    </div>`;
}

// `repo` is added only when the thread has one, so a plan in a workspace of
// one repo approves exactly as it did before repos were a thing.
function rowThread(row) {
  const t = {
    name: row.querySelector('.plan-name').value.trim(),
    goal: row.querySelector('.plan-goal').value.trim(),
    scope: row.querySelector('.plan-scope').value.trim(),
  };
  if (row.dataset.repo !== undefined) t.repo = row.dataset.repo;
  return t;
}

// The Repository select of a card, from the workspace's repos. Values are
// '=' + path, so the workspace itself ('=') is never mistaken for "none" ('').
function fillRepoSelect(row) {
  const sel = row.querySelector('.plan-repo-select');
  const cur = row.dataset.repo;
  const ws = planRepos?.workspace;
  const list = planRepos?.ok ? planRepos.repos.map(r => r.path) : [];
  if (cur !== undefined && !list.includes(cur)) list.push(cur);
  let none = '';
  if (cur === undefined) {
    const def = planRepos?.ok ? defaultRepo(planRepos.repos) : undefined;
    none = `<option value="">${planNeedsRepo() ? 'Choose a repository…'
      : def !== undefined ? `Default: ${escHtml(repoLabel(def, ws))}` : 'Default'}</option>`;
  }
  sel.innerHTML = none + list.map(p => {
    const r = planRepos?.ok ? planRepos.repos.find(x => x.path === p) : null;
    const st = r ? `  (${r.detached ? '' : `${r.branch}, `}${repoStatus(r).text})` : '';
    return `<option value="=${escHtml(p)}"${p === cur ? ' selected' : ''}>${escHtml(repoLabel(p, ws) + st)}</option>`;
  }).join('');
  sel.disabled = !list.length;
}

function paintPlanRow(row) {
  row.classList.toggle('needs-repo', planNeedsRepo() && row.dataset.repo === undefined);
  fillRepoSelect(row);
  if (!row.classList.contains('editing')) {
    row.querySelector('.plan-view').innerHTML = planViewHtml(rowThread(row));
    fitPlanMore(row);
  }
}

// Approve waits until every thread has somewhere to work. Not while a
// request is out: that one is already on its way.
function updatePlanReady() {
  if (planApprove.textContent !== 'Approve and start') return;
  const rows = [...planRows.querySelectorAll('.plan-thread')];
  const empty = !!planRepos?.ok && !planRepos.repos.length;
  const missing = rows.filter(r => r.classList.contains('needs-repo')).length;
  planApprove.disabled = empty || missing > 0;
  planApprove.title = empty ? 'The workspace holds no git repository'
    : missing ? `Choose a repository for ${missing} thread${missing === 1 ? '' : 's'}` : '';
  setPlanError(empty
    ? `${repoLabel('', planRepos.workspace)} holds no git repository, and every thread needs one: run git init there, or clone one into it.`
    : '');
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
  showNote(planError, msg);
}

async function showPlan(plan) {
  planShown = plan.id;
  if (planRepos?.sandbox !== plan.sandbox) planRepos = null;
  document.getElementById('plan-title').textContent = `Plan from the lead of ${plan.sandbox}`;
  document.getElementById('plan-summary').textContent = plan.summary || '';
  planRows.innerHTML = plan.threads.map((t, i) => planRowHtml(t, i)).join('');
  setPlanError('');
  planApprove.disabled = false;
  planApprove.textContent = 'Approve and start';
  planOverlay.classList.remove('hidden');
  planRows.querySelectorAll('.plan-thread').forEach(paintPlanRow);
  updatePlanReady();
  updatePlanCount();
  if (!planRepos) {
    const r = await fetchRepos(plan.sandbox);
    if (planShown !== plan.id) return;
    planRepos = { sandbox: plan.sandbox, ...r };
    planRows.querySelectorAll('.plan-thread').forEach(paintPlanRow);
    updatePlanReady();
  }
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
  planRepos = null;
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
  if (planShown === plan.id && (plan.state !== 'pending' || plan.closed)) closePlan();
  schedulePaintGroups();
}

planRows.addEventListener('click', e => {
  const row = e.target.closest('.plan-thread');
  if (!row) return;
  if (e.target.closest('.plan-remove')) {
    row.remove();
    updatePlanReady();
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
  paintPlanRow(row);
  setPlanEditing(row, true);
  updatePlanReady();
  row.querySelector('.plan-name').focus();
  updatePlanCount();
});

planRows.addEventListener('change', e => {
  const sel = e.target.closest('.plan-repo-select');
  if (!sel) return;
  const row = sel.closest('.plan-thread');
  if (sel.value) row.dataset.repo = sel.value.slice(1);
  else delete row.dataset.repo;
  paintPlanRow(row);
  updatePlanReady();
});

planApprove.addEventListener('click', async () => {
  const id = planShown;
  if (!id) return;
  const rows = [...planRows.querySelectorAll('.plan-thread')];
  const threads = rows.map(rowThread);
  const bad = threads.findIndex(t => !THREAD_NAME.test(t.name));
  if (!threads.length) { setPlanError('Keep at least one thread, or refuse the plan'); return; }
  const unplaced = rows.findIndex(r => r.classList.contains('needs-repo'));
  if (unplaced >= 0) {
    setPlanError(`"${threads[unplaced].name || 'Unnamed thread'}" needs a repository`);
    if (!rows[unplaced].classList.contains('editing')) setPlanEditing(rows[unplaced], true);
    rows[unplaced].querySelector('.plan-repo-select').focus();
    return;
  }
  if (bad >= 0) {
    setPlanError(`"${threads[bad].name}": lower-case letters, digits and inner hyphens only`);
    if (!rows[bad].classList.contains('editing')) setPlanEditing(rows[bad], true);
    rows[bad].querySelector('.plan-name').focus();
    return;
  }
  planApprove.disabled = true;
  planApprove.textContent = 'Starting…';
  setPlanError('');
  const res = await postJsonOrError(`/api/plans/${encodeURIComponent(id)}/approve`, { threads });
  if (!res.ok) {
    planApprove.textContent = 'Approve and start';
    updatePlanReady();
    setPlanError(res.error || 'could not approve the plan');
    return;
  }
  // The `plan` event that follows opens the panes; the reply carries the same
  // plan, so a dropped event does not lose them.
  ingestPlan(res.plan);
});

document.getElementById('plan-refuse').addEventListener('click', async () => {
  const id = planShown;
  if (!id) return;
  const res = await postJsonOrError(`/api/plans/${encodeURIComponent(id)}/refuse`, {});
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
sbxwStream.addEventListener('lead-end', ev => forgetLeading(ev.data));

sbxwStream.addEventListener('plan', ev => {
  try { ingestPlan(JSON.parse(ev.data)); } catch (_) {}
});
