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
// What the dialog shows of that plan: 'review' while it waits for approval,
// 'merge' once its lead has proposed merging the threads' branches.
let planView = 'review';
// The models a thread can run, from /api/models (see fetchModels in util.js);
// null until they arrive.
let planModels = null;

const planOverlay = document.getElementById('plan-overlay');
const planRows = document.getElementById('plan-threads');
const planError = document.getElementById('plan-error');
const planApprove = document.getElementById('plan-approve');

// Where thread `thread` of `sandbox` stands, from the latest approved plan
// that holds it; null when no plan does.
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

// Each sandbox's lead as the daemon vouches for it (/api/lead-states):
// `waiting` (Team pressed, the agent busy), `activating` (`/lead` typed, the
// session has not confirmed), `leading`, or `off` with the reason of a recent
// fall back. A sandbox that is off is absent.
const leadStates = new Map();
// The sandboxes that lead: those `leading` above, as /api/leads lists them.
const leadSessions = new Set();

function refreshLeads() {
  fetch('/api/lead-states')
    .then(r => r.json())
    .then(states => {
      leadStates.clear();
      leadSessions.clear();
      for (const [n, st] of Object.entries(states || {})) {
        leadStates.set(n, st);
        if (st.state === 'leading') leadSessions.add(n);
      }
      schedulePaintGroups();
    })
    .catch(() => {});
}

// BEGIN plans lead pure
// Is the main session of `sandbox` the lead? Only the daemon's word counts:
// a plan under way says nothing of whether a session leads it (see
// `planWithoutLead`).
function leadingIn(leads, sandbox) {
  return leads.has(sandbox);
}

// The lead's state of `sandbox` other than leading: 'waiting',
// 'activating', or null.
function leadPendingIn(states, sandbox) {
  const st = states.get(sandbox)?.state;
  return st === 'waiting' || st === 'activating' ? st : null;
}

// A plan of `sandbox` is under way (one waiting on a person, or approved
// with a thread still at work) while no session leads it.
function planWithoutLeadIn(plans, leads, states, sandbox) {
  if (leadingIn(leads, sandbox) || leadPendingIn(states, sandbox)) return false;
  for (const p of plans) {
    if (p.sandbox !== sandbox || p.closed) continue;
    if (p.state === 'pending') return true;
    if (p.state === 'approved' && p.threads.some(t => !['done', 'failed'].includes(t.state))) return true;
  }
  return false;
}

// What the pane's Team toggle says, and its hover text.
function leadToggleText(leading, pending, orphan, reason) {
  if (leading) return { label: 'On', title: 'Team mode is on. Click to end it: this session stops leading; its threads keep running' };
  if (pending === 'waiting') return { label: 'Waiting', title: 'Team mode waits for the agent to be idle before sbxw types /lead. Click to cancel' };
  if (pending === 'activating') return { label: 'Starting', title: 'sbxw typed /lead and waits for the session to confirm it leads. Click to cancel' };
  if (orphan) return { label: 'No lead', title: 'A plan is under way and no session leads it. Click to resume the lead' };
  return { label: 'Off', title: reason ? `Team mode is off: ${reason}` : null };
}
// END plans lead pure

function isLeading(sandbox) {
  return leadingIn(leadSessions, sandbox);
}

function leadPending(sandbox) {
  return leadPendingIn(leadStates, sandbox);
}

function planWithoutLead(sandbox) {
  return planWithoutLeadIn(plans.values(), leadSessions, leadStates, sandbox);
}

function leadReason(sandbox) {
  return leadStates.get(sandbox)?.reason || '';
}

// Lead mode ended, here or in another tab: the daemon has closed its plans
// (they come back with `closed: true`), and the states are read again.
function forgetLeading(sandbox) {
  leadSessions.delete(sandbox);
  refreshLeads();
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
    lines.push(`${running} thread${running === 1 ? ' is' : 's are'} still at work. ${running === 1 ? 'It keeps' : 'They keep'} running (close ${running === 1 ? 'it' : 'them'} with Close thread), but ${running === 1 ? 'its reports' : 'their reports'} will no longer reach the main session.`);
  }
  if (pending) lines.push('The plan waiting for your approval will be refused.');
  document.getElementById('lead-end-body').innerHTML =
    lines.map((l, i) => `<p class="${i ? 'lead-end-warn' : 'thread-close-lead'}">${escHtml(l)}</p>`).join('');
  showNote(leadEndError, '');
  leadEndConfirm.disabled = false;
  leadEndConfirm.textContent = 'End lead mode';
  modalOpen(leadEndOverlay);
  leadEndConfirm.focus();
}

function closeLeadEnd() { leadEndOverlay.close(); leadEndAsk = null; }

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
wireModal(leadEndOverlay, closeLeadEnd);

// "Review plan" on the main pane, shown while its plan waits on a person:
// the way back to a dialog closed with Later.
function refreshPlanButtons() {
  panes.forEach((p, i) => {
    const btn = document.getElementById(`pplan-${i}`);
    if (btn) btn.hidden = !(i < paneCount && p.sandbox && !p.thread && pendingPlanFor(p.sandbox));
    const merge = document.getElementById(`pmerge-${i}`);
    if (!merge) return;
    const mp = i < paneCount && p.sandbox && !p.thread ? mergePlanFor(p.sandbox) : null;
    merge.hidden = !mp;
    if (mp) merge.textContent = mp.merge.state === 'merging' ? 'Merging…' : mp.merge.state === 'failed' ? 'Retry merge' : 'Merge';
  });
}

function openPendingPlan(sandbox) {
  const p = pendingPlanFor(sandbox);
  if (p) showPlan(p);
}

// ── Merge ──
// Once a lead has reviewed its threads it proposes merging their branches, in
// an order of its choosing; the Merge button on its pane opens that proposal.
// One click merges them all, one after another, and each row follows along
// from the plan's SSE updates: pending, then merged at a commit or failed with
// the reason. A failure stops the run, so what follows it was never merged.
const mergeList = document.getElementById('merge-list');
const mergeAll = document.getElementById('merge-all');
const mergeLater = document.getElementById('merge-later');
const mergeError = document.getElementById('merge-error');
const mergeClose = document.getElementById('merge-close');
const mergeAbandon = document.getElementById('merge-abandon');
const mergeAbandonAsk = document.getElementById('merge-abandon-ask');
const mergeAbandonYes = document.getElementById('merge-abandon-yes');
// The plan id a Merge all is on its way for, until the daemon answers.
let mergeAsk = null;

// BEGIN plans merge pure
// A merge to do: proposed, under way, or failed (and can be retried). One the
// person abandoned is none: the pane's Merge button goes, the dialog shows
// the record. Abandon merge is offered while it is proposed or failed only.
function mergeToDo(m) {
  return !!m && ['proposed', 'merging', 'failed'].includes(m.state);
}
function mergeAbandonable(m) {
  return !!m && (m.state === 'proposed' || m.state === 'failed');
}
const MERGE_ABANDON_ASK = 'Abandon the proposed merge? Nothing is merged, and the threads, their branches and worktrees stay as they are. The lead is told.';
// The outcome line of an abandoned merge: what merged before stays merged.
function mergeAbandonedText(m) {
  const done = (m.results || []).filter(r => r.ok).map(r => r.thread);
  return done.length
    ? `You abandoned this merge. Merged before that: ${done.join(', ')}. The rest was not merged; the lead may propose it again.`
    : 'You abandoned this merge: nothing was merged. The lead may propose it again.';
}
// END plans merge pure

// The plan of `sandbox` whose merge waits on a person, is under way or failed
// (and can be retried): the latest, if several. Plans whose lead mode was ended are over.
function mergePlanFor(sandbox) {
  let best = null;
  for (const p of plans.values()) {
    if (p.sandbox !== sandbox || p.closed || !p.merge) continue;
    if (!mergeToDo(p.merge)) continue;
    if (!best || p.created_ms > best.created_ms) best = p;
  }
  return best;
}

function openMergePlan(sandbox) {
  const p = mergePlanFor(sandbox);
  if (p) showMerge(p);
}

// ── Shared plans at the merge ──
// Commits by thread, open claims (they block Merge until released) and the
// distillate the lead proposes, all from the board, read again every few
// seconds while the merge is open. `mergeBoard` is null until the first read
// and for a split plan; `distillateEdits` holds what the person changed.
let mergeBoard = null;
let mergeBoardTimer = null;
let distillateEdits = null; // [{kind, text, keep}] once the list is first painted
// Indexes of the notes shown open; every note starts closed, one line each.
// Kept here so a repaint (which rebuilds the rows) leaves them open.
let distillateOpen = new Set();
const mergeTeam = document.getElementById('merge-team');

function stopMergeBoard() {
  clearInterval(mergeBoardTimer);
  mergeBoardTimer = null;
  mergeBoard = null;
  distillateEdits = null;
  distillateOpen = new Set();
  mergeTeam.innerHTML = '';
  delete mergeTeam.dataset.key;
}

function startMergeBoard(plan) {
  clearInterval(mergeBoardTimer);
  mergeTeam.innerHTML = '';
  delete mergeTeam.dataset.key;
  if (plan.variant !== 'shared') return;
  const read = async () => {
    const b = await fetchBoard(plan.sandbox);
    if (planShown !== plan.id || planView !== 'merge') return;
    // A board that cannot be read does not hold Merge for ever: the daemon
    // refuses a merge with claims open anyway.
    if (b) mergeBoard = b; else if (!mergeBoard) mergeBoard = { claims: [], entries: [], unread: true };
    const now = plans.get(plan.id);
    if (now?.merge) paintMerge(now);
  };
  read();
  mergeBoardTimer = setInterval(() => { if (document.visibilityState === 'visible') read(); }, 4000);
}

const mergeClaimsOpen = plan => plan.variant === 'shared' ? (mergeBoard ? mergeBoard.claims.length : null) : 0;

function distillateOf(plan) {
  if (!distillateEdits) distillateEdits = (plan.merge.distillate || []).map(d => ({ ...d, keep: true }));
  return distillateEdits;
}

// The distillate the merge sends: what the person kept, as edited.
const keptDistillate = () => (distillateEdits || []).filter(d => d.keep && d.text.trim())
  .map(d => ({ kind: d.kind, text: d.text.trim() }));

function paintMergeTeam(plan) {
  if (plan.variant !== 'shared') { mergeTeam.innerHTML = ''; return; }
  const over = plan.merge.state === 'merged';
  const claims = mergeBoard?.claims || [];
  const commits = new Map();
  (mergeBoard?.entries || []).filter(e => e.kind === 'commit').sort((a, b) => a.at - b.at)
    .forEach(e => { if (!commits.has(e.by)) commits.set(e.by, []); commits.get(e.by).push(e); });
  const hue = name => Math.max(0, plan.threads.findIndex(t => t.name === name));
  let html = '';
  if (claims.length) {
    html += `<div class="merge-claims" role="alert"><strong>${wtPlural(claims.length, 'file')} still claimed</strong>. Merge waits until they are released.
      <ul>${claims.map(c => `<li><code>${escHtml(c.file)}</code> · ${escHtml(c.owner)}</li>`).join('')}</ul></div>`;
  }
  html += `<h3 class="merge-h">Commits by thread</h3>` + (commits.size
    ? [...commits].map(([name, list]) => `<div class="merge-commits" style="--plan-accent: var(${PLAN_HUES[hue(name) % PLAN_HUES.length]})">
        <div class="merge-commits-name">${escHtml(name)} <span class="merge-commits-n">${wtPlural(list.length, 'commit')}</span></div>
        <ul>${list.map(e => `<li>${e.hash ? `<code>${escHtml(wtShort(e.hash))}</code> ` : ''}${escHtml(e.text || '')}</li>`).join('')}</ul></div>`).join('')
    : `<p class="wt-note">${mergeBoard ? mergeBoard.unread ? 'The board could not be read.' : 'No commit on the board.' : 'Reading the board…'}</p>`);
  const dist = distillateOf(plan);
  if (dist.length || !over) {
    html += `<h3 class="merge-h">Distillate</h3>
      <p class="wt-note">Decisions and warnings that still hold. What you keep goes to auto memory with the merge.</p>
      <ul class="merge-distillate">${dist.map((d, i) => {
        const open = distillateOpen.has(i);
        const label = d.kind === 'warning' ? '▲ Warning' : '◆ Decision';
        return `<li class="merge-dist${d.keep ? '' : ' dropped'}${open ? ' open' : ''}" data-i="${i}">
        <div class="merge-dist-head">
          <input type="checkbox" class="merge-dist-keep" aria-label="Keep this ${escHtml(d.kind)}"${d.keep ? ' checked' : ''}${over ? ' disabled' : ''}>
          <button type="button" class="merge-dist-toggle" aria-expanded="${open}" aria-controls="merge-dist-body-${i}">
            <span class="merge-dist-kind" data-kind="${escHtml(d.kind)}">${label}</span>
            <span class="merge-dist-preview">${escHtml(d.text)}</span>
            <span class="merge-dist-chevron" aria-hidden="true">›</span>
          </button>
        </div>
        <div class="merge-dist-body" id="merge-dist-body-${i}" role="region" aria-label="${label.slice(2)} ${i + 1}"${open ? '' : ' hidden'}>
          <textarea class="field-input merge-dist-text" rows="1"${over ? ' disabled' : ''}>${escHtml(d.text)}</textarea>
        </div></li>`;
      }).join('')
        || '<li class="wt-note">The lead proposed nothing to keep.</li>'}</ul>`;
  }
  // Repainting while typing would drop the caret: only rebuild when the
  // structure changed, otherwise leave the inputs alone.
  const key = JSON.stringify([claims.map(c => c.file + c.owner), [...commits].map(([n, l]) => [n, l.map(e => e.hash || e.id)]),
    dist.length, over, !!mergeBoard]);
  if (mergeTeam.dataset.key === key) return;
  mergeTeam.dataset.key = key;
  mergeTeam.innerHTML = html;
  mergeTeam.querySelectorAll('.merge-dist.open .merge-dist-text').forEach(growDistText);
}

// A note's text area is as tall as its text: no inner scrollbar. Only
// measurable while its row is open (a hidden one has no scrollHeight).
function growDistText(t) {
  t.style.height = 'auto';
  t.style.height = `${t.scrollHeight + t.offsetHeight - t.clientHeight}px`;
}

mergeTeam.addEventListener('input', e => {
  const t = e.target.closest('.merge-dist-text');
  if (!t) return;
  const li = t.closest('.merge-dist');
  distillateEdits[+li.dataset.i].text = t.value;
  li.querySelector('.merge-dist-preview').textContent = t.value;
  growDistText(t);
});
// Clicking a note's header, anywhere but its checkbox, opens or closes it.
// The toggle is a button, so Enter and Space arrive here as clicks too.
mergeTeam.addEventListener('click', e => {
  const head = e.target.closest('.merge-dist-head');
  if (!head || e.target.closest('.merge-dist-keep')) return;
  const li = head.closest('.merge-dist');
  const i = +li.dataset.i;
  const open = !distillateOpen.has(i);
  if (open) distillateOpen.add(i); else distillateOpen.delete(i);
  li.classList.toggle('open', open);
  li.querySelector('.merge-dist-toggle').setAttribute('aria-expanded', String(open));
  li.querySelector('.merge-dist-body').hidden = !open;
  if (open) growDistText(li.querySelector('.merge-dist-text'));
});
mergeTeam.addEventListener('change', e => {
  const k = e.target.closest('.merge-dist-keep');
  if (!k) return;
  const li = k.closest('.merge-dist');
  distillateEdits[+li.dataset.i].keep = k.checked;
  li.classList.toggle('dropped', !k.checked);
});

function setPlanView(view) {
  planView = view;
  planOverlay.classList.toggle('merge-view', view === 'merge');
}

async function showMerge(plan) {
  planShown = plan.id;
  setPlanView('merge');
  mergeBoard = null;
  distillateEdits = null;
  distillateOpen = new Set();
  startMergeBoard(plan);
  if (planRepos?.sandbox !== plan.sandbox) planRepos = null;
  document.getElementById('plan-title').textContent = mergeToDo(plan.merge) ? `Merge the threads of ${plan.sandbox}` : `Plan of ${plan.sandbox}`;
  mergeClose.checked = true;
  mergeAbandonAsk.hidden = true;
  showNote(mergeError, '');
  paintMerge(plan);
  modalOpen(planOverlay);
  (mergeAll.hidden || mergeAll.disabled ? mergeLater : mergeAll).focus();
  if (!planRepos) {
    const r = await fetchRepos(plan.sandbox);
    if (planShown !== plan.id) return;
    planRepos = { sandbox: plan.sandbox, ...r };
    const now = plans.get(plan.id);
    if (now?.merge && planView === 'merge') paintMerge(now);
  }
}

// One row of the merge: where the thread stands in it, as a mark and a word.
function mergeRowHtml(plan, name, i) {
  const m = plan.merge;
  const t = plan.threads.find(x => x.name === name);
  const r = (m.results || []).find(x => x.thread === name);
  const running = m.state === 'merging' || mergeAsk === plan.id;
  let state = '', status = '';
  if (r?.ok) {
    state = 'ok';
    status = `<span class="merge-mark" aria-hidden="true">✓</span>${r.commit
      ? `<code class="merge-commit" title="${escHtml(r.commit)}">${escHtml(r.commit.slice(0, 7))}</code>` : 'merged'}`;
  } else if (r) {
    state = 'failed';
    status = `<span class="merge-mark" aria-hidden="true">✕</span><span>${escHtml(r.error || 'failed')}</span>`;
  } else if (running) {
    state = 'pending';
    status = '<span>pending</span>';
  } else if (t?.set_by === 'lead' && !(m.state === 'failed' || m.state === 'merged')) {
    status = `<span>marked done by the lead${t.summary ? `: ${escHtml(t.summary)}` : ''}</span>`;
  } else if (m.state === 'failed' || m.state === 'merged' || m.state === 'abandoned') {
    state = 'skipped';
    status = '<span>not merged</span>';
  }
  const repo = t?.repo != null
    ? `<code class="plan-repo-chip">${escHtml(repoLabel(t.repo, planRepos?.workspace))}</code>` : '';
  return `<li class="merge-row${state ? ` ${state}` : ''}" style="--plan-accent: var(${PLAN_HUES[i % PLAN_HUES.length]})">
      <span class="merge-name">${escHtml(name)}</span>${repo}
      ${status ? `<span class="merge-status">${status}</span>` : ''}
    </li>`;
}

function paintMerge(plan) {
  const m = plan.merge;
  const order = m.order?.length ? m.order : plan.threads.map(t => t.name);
  // The hue each thread had on its plan card, so a row is recognised at once.
  const hue = name => Math.max(0, plan.threads.findIndex(t => t.name === name));
  document.getElementById('merge-note').textContent = m.note || '';
  mergeList.innerHTML = order.map(n => mergeRowHtml(plan, n, hue(n))).join('');

  const outcome = document.getElementById('merge-outcome');
  outcome.className = 'merge-outcome';
  if (m.state === 'merged') {
    outcome.classList.add('ok');
    const notes = (m.results || []).filter(r => r.ok && r.note).map(r => `${r.thread}: ${r.note}.`);
    outcome.textContent = (m.base ? `Merged into ${m.base}.` : 'Merged.') + (notes.length ? ` ${notes.join(' ')}` : '');
  } else if (m.state === 'failed') {
    outcome.classList.add('failed');
    const bad = (m.results || []).find(r => !r.ok);
    const done = new Set((m.results || []).filter(r => r.ok).map(r => r.thread));
    const rest = order.filter(n => !done.has(n) && n !== bad?.thread);
    outcome.textContent = (bad ? `${bad.thread} could not be merged: ${bad.error || 'no reason given'}.` : 'The merge failed.')
      + (rest.length ? ` The remaining thread${rest.length === 1 ? ' was' : 's were'} not merged: ${rest.join(', ')}.` : '');
  } else if (m.state === 'abandoned') {
    outcome.textContent = mergeAbandonedText(m);
  } else {
    outcome.textContent = '';
  }
  outcome.hidden = !outcome.textContent;

  // 'merged' is final, and so is 'abandoned' (the lead may propose afresh);
  // 'failed' stays open for a retry.
  const over = !mergeToDo(m);
  if (over) document.getElementById('plan-title').textContent = `Plan of ${plan.sandbox}`;
  const failed = m.state === 'failed';
  const running = !over && (m.state === 'merging' || mergeAsk === plan.id);
  document.getElementById('merge-close-row').hidden = over;
  mergeClose.disabled = running;
  paintMergeTeam(plan);
  const open = mergeClaimsOpen(plan);
  mergeAll.hidden = over;
  mergeAll.disabled = running || open !== 0;
  mergeAll.title = open ? `${wtPlural(open, 'file')} still claimed: release them first` : open === null ? 'Reading the board…' : '';
  mergeAll.textContent = running ? 'Merging…' : failed ? 'Retry merge' : 'Merge all';
  // Closing never stops a merge under way: the pane's Merging… button leads back.
  mergeLater.textContent = over ? 'Close' : running ? 'Hide' : failed ? 'Close' : 'Not now';
  // Quiet, beside Not now: shown while the merge is proposed or failed.
  mergeAbandon.hidden = running || !mergeAbandonable(m);
  if (mergeAbandon.hidden) mergeAbandonAsk.hidden = true;
}

// Abandon merge: asked in the dialog first, then the daemon withdraws the
// merge. Nothing is merged, closed or deleted; the lead is told.
mergeAbandon.addEventListener('click', () => {
  document.getElementById('merge-abandon-text').textContent = MERGE_ABANDON_ASK;
  mergeAbandonYes.disabled = false;
  mergeAbandonYes.textContent = 'Abandon merge';
  mergeAbandonAsk.hidden = false;
  showNote(mergeError, '');
  mergeAbandonYes.focus();
});
document.getElementById('merge-abandon-no').addEventListener('click', () => {
  mergeAbandonAsk.hidden = true;
  mergeAbandon.focus();
});
mergeAbandonYes.addEventListener('click', async () => {
  const id = planShown;
  if (!mergeAbandonable(plans.get(id)?.merge) || mergeAbandonYes.disabled) return;
  mergeAbandonYes.disabled = true;
  mergeAbandonYes.textContent = 'Abandoning…';
  const res = await postJsonOrError(`/api/plans/${encodeURIComponent(id)}/merge/abandon`, {});
  mergeAbandonYes.disabled = false;
  mergeAbandonYes.textContent = 'Abandon merge';
  if (res.ok && res.plan) plans.set(id, res.plan);
  if (planShown === id && planView === 'merge') {
    if (res.ok) mergeAbandonAsk.hidden = true;
    else showNote(mergeError, `Abandon merge refused: ${res.error || 'no reason given'}`);
    const now = plans.get(id);
    if (now?.merge) paintMerge(now);
  }
  schedulePaintGroups();
});

mergeAll.addEventListener('click', async () => {
  const id = planShown;
  const plan = plans.get(id);
  if (!plan?.merge || mergeAsk || mergeClaimsOpen(plan) !== 0) return;
  mergeAsk = id;
  showNote(mergeError, '');
  paintMerge(plan);
  const body = { close_threads: mergeClose.checked };
  if (plan.variant === 'shared') body.distillate = keptDistillate();
  const res = await postJsonOrError(`/api/plans/${encodeURIComponent(id)}/merge`, body);
  mergeAsk = null;
  const now = plans.get(id);
  if (res.ok && res.merge && now) plans.set(id, { ...now, merge: res.merge });
  if (planShown === id && planView === 'merge' && plans.get(id)?.merge) {
    paintMerge(plans.get(id));
    if (!res.ok) showNote(mergeError, res.error || 'could not merge');
  }
  schedulePaintGroups();
});
mergeLater.addEventListener('click', closePlan);

// The group hues, in the order panes use them: a thread keeps its colour from
// the card edge to every file it owns, so whose-is-what reads at a glance.
const PLAN_HUES = ['--c-group-1', '--c-group-2', '--c-group-3', '--c-group-4'];

// A goal's opening sentence, as the one line saying what the thread does, and
// the rest. A sentence ends at . ! or ? followed by a capital or an opening
// quote, so `web.rs` or `e.g. a file` does not cut it short.
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
// The model has its own select below, shown whether or not the card is edited.
function planRepoHtml(t) {
  if (t.repo != null) {
    return `<div class="plan-repo"><span class="plan-owns-label">In</span>
        <code class="plan-repo-chip">${escHtml(repoLabel(t.repo, planRepos?.workspace))}</code></div>`;
  }
  if (planNeedsRepo()) {
    return `<div class="plan-repo"><span class="plan-repo-flag">No repository yet: choose one with Edit</span></div>`;
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
// inputs are the card's source of truth (Approve reads them whether or not
// they were ever shown), so reading and editing cannot drift apart. The repo
// is kept in `data-repo`, absent while the thread has none: '' is a repo too,
// the workspace itself. The model is kept the same way, in `data-model`.
function planRowHtml(t = { name: '', goal: '', scope: '' }, i = 0) {
  const repo = typeof t.repo === 'string' ? ` data-repo="${escHtml(t.repo)}"` : '';
  const model = t.model ? ` data-model="${escHtml(t.model)}"` : '';
  return `<div class="plan-thread" style="--plan-accent: var(${PLAN_HUES[i % PLAN_HUES.length]})"${repo}${model}>
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
      <label class="plan-model-field"><span class="plan-owns-label">Model</span>
        <select class="field-input plan-model-select"></select>
      </label>
    </div>`;
}

// `repo` and `model` are added only when the thread has one, so a plan that
// names neither approves exactly as it did before either was a thing.
function rowThread(row) {
  const t = {
    name: row.querySelector('.plan-name').value.trim(),
    goal: row.querySelector('.plan-goal').value.trim(),
    scope: row.querySelector('.plan-scope').value.trim(),
  };
  if (row.dataset.repo !== undefined) t.repo = row.dataset.repo;
  if (row.dataset.model) t.model = row.dataset.model;
  return t;
}

// The Model select of a card: the default first, as "none", then each model.
// Hidden when the daemon offers no list; the card keeps whatever model it has.
function fillModelSelect(row) {
  const field = row.querySelector('.plan-model-field');
  field.hidden = !planModels?.ok;
  if (field.hidden) return;
  const cur = row.dataset.model || '';
  const list = planModels.models.slice();
  if (cur && !list.some(m => m.id === cur)) list.push({ id: cur, label: cur });
  row.querySelector('.plan-model-select').innerHTML =
    `<option value="">Default (${escHtml(modelLabel(planModels.default, planModels))})</option>`
    + list.map(m => `<option value="${escHtml(m.id)}"${m.id === cur ? ' selected' : ''}>${escHtml(m.label)}</option>`).join('');
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
  fillModelSelect(row);
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

// ── Variant ──
// Split: a worktree and a branch per thread. Shared: one worktree, one branch,
// file claims on a board. The lead proposes one with its justification; the
// choice made here is what Approve sends.
const planVariant = document.getElementById('plan-variant');
function paintPlanVariant(plan) {
  const v = plan.variant === 'shared' ? 'shared' : 'split';
  const opt = (val, name, hint) => `<label class="plan-variant-opt"><input type="radio" name="plan-variant" value="${val}"${val === v ? ' checked' : ''}>
      <span><strong>${name}</strong> <span class="plan-variant-hint">${hint}</span></span></label>`;
  planVariant.innerHTML = `<div class="plan-variant-opts" role="radiogroup" aria-label="Variant">
      ${opt('split', 'Split', 'a worktree and a branch per thread')}
      ${opt('shared', 'Shared', 'one worktree, one branch, files claimed on a board')}
    </div>
    ${plan.justification ? `<p class="plan-justification"><span class="plan-owns-label">Why</span> ${escHtml(plan.justification)}</p>` : ''}
    ${planTestsHtml(plan.proposed_tests)}`;
}

// BEGIN plans tests pure
// The test command the lead proposed with the plan: approving the plan
// approves it, and sbxw then runs it on the threads' commits. So it is drawn
// in full, as text (an agent's words are data), with its time limit, and
// the person may approve the plan without it.
function planTestsHtml(tests) {
  if (!tests || !tests.command) return '';
  const limit = Number(tests.timeout_minutes) || 60;
  return `<div class="plan-tests"><p class="plan-justification"><span class="plan-owns-label">Tests</span> `
    + `<code class="plan-test-command">${escHtml(tests.command)}</code> `
    + `<span class="plan-variant-hint">time limit ${limit} min, run by sbxw on the threads' commits</span></p>`
    + `<label class="plan-variant-opt"><input type="checkbox" id="plan-no-tests"> `
    + `<span>Approve without tests <span class="plan-variant-hint">the plan then has no test queue</span></span></label></div>`;
}
// END plans tests pure
const planClearTests = () => !!planVariant.querySelector('#plan-no-tests')?.checked;
const planVariantChoice = () => planVariant.querySelector('input:checked')?.value || 'split';

function setPlanError(msg) {
  showNote(planError, msg);
}

// The main checkout's uncommitted work, counted on the host when the plan was
// proposed (main_uncommitted): the threads' worktrees are cut from HEAD and
// will not have it.
function paintPlanUncommitted(plan) {
  const summary = document.getElementById('plan-summary');
  let note = document.getElementById('plan-uncommitted');
  if (!note) {
    note = document.createElement('p');
    note.id = 'plan-uncommitted';
    note.className = 'plan-summary';
    note.setAttribute('role', 'note');
    summary.after(note);
  }
  const n = plan.main_uncommitted || 0;
  note.textContent = n
    ? `⚠ ${n} uncommitted change${n === 1 ? '' : 's'} in the main checkout will not be in the threads' worktrees: commit ${n === 1 ? 'it' : 'them'} first if the plan depends on ${n === 1 ? 'it' : 'them'}.`
    : '';
  note.hidden = !n;
}

async function showPlan(plan) {
  planShown = plan.id;
  setPlanView('review');
  if (planRepos?.sandbox !== plan.sandbox) planRepos = null;
  document.getElementById('plan-title').textContent = `Plan from the lead of ${plan.sandbox}`;
  document.getElementById('plan-summary').textContent = plan.summary || '';
  paintPlanUncommitted(plan);
  paintPlanVariant(plan);
  planRows.innerHTML = plan.threads.map((t, i) => planRowHtml(t, i)).join('');
  setPlanError('');
  planApprove.disabled = false;
  planApprove.textContent = 'Approve and start';
  modalOpen(planOverlay);
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
  stopMergeBoard();
  planOverlay.close();
  planShown = null;
  mergeAsk = null;
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
  // a dialog is already up; its pane's Review plan button leads back to it.
  if (plan.state === 'pending' && !prev && !planOverlay.open) showPlan(plan);
  if (plan.state === 'approved' && !seed && prev?.state === 'pending' && !panesOpenedFor.has(plan.id)) {
    panesOpenedFor.add(plan.id);
    openPlanPanes(plan);
  }
  if (planShown === plan.id) {
    if (planView === 'merge') { if (plan.merge) paintMerge(plan); else closePlan(); }
    else if (plan.state !== 'pending' || plan.closed) closePlan();
  }
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
  const msel = e.target.closest('.plan-model-select');
  if (msel) {
    const row = msel.closest('.plan-thread');
    if (msel.value) row.dataset.model = msel.value;
    else delete row.dataset.model;
    return;
  }
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
  const res = await postJsonOrError(`/api/plans/${encodeURIComponent(id)}/approve`, { threads, variant: planVariantChoice(), clear_tests: planClearTests() });
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
wireModal(planOverlay, closePlan);

fetchModels().then(m => {
  planModels = m;
  if (planShown && planView === 'review') planRows.querySelectorAll('.plan-thread').forEach(paintPlanRow);
});

fetch('/api/plans')
  .then(r => r.json())
  .then(list => list.forEach(p => ingestPlan(p, true)))
  .catch(() => {});

refreshLeads();
// Every change of a lead's state sends `lead`, every fall back to off
// `lead-end`: the states are read again on either.
sbxwStream.addEventListener('lead', () => refreshLeads());
sbxwStream.addEventListener('lead-end', ev => forgetLeading(ev.data));

sbxwStream.addEventListener('plan', ev => {
  try { ingestPlan(JSON.parse(ev.data)); } catch (_) {}
});
