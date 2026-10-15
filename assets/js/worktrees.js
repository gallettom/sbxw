// ── Worktrees ─────────────────────────────────────────────────────────────
// Where each thread's branch stands against the branch the lead works on: the
// one its repo's main checkout is on, which is also where a merge lands. Drawn
// as lanes off that branch, one row per thread, and said in words beside each
// lane, so the drawing needs no legend. Opened from the Thread menu and read
// again every few seconds while open (GET /api/sandboxes/:name/worktrees, see
// src/worktrees.rs).

const wtOverlay = document.getElementById('wt-overlay');
const wtBody = document.getElementById('wt-body');
const wtUpdated = document.getElementById('wt-updated');
// The sandbox shown, the pane whose Thread menu opened it (where a thread
// clicked here opens, as from the menu), the poll timer, whether a read is on
// its way, and the last answer.
const wtView = { sandbox: null, idx: null, timer: null, busy: false, data: null, lanes: 360 };

const WT_POLL_MS = 4000;
const WT_ROW = 40;         // px per lane: the base first, then a thread per row
const WT_STEP = [22, 44];  // px per commit, least and most: as wide as the lanes allow
const WT_PAD = 32;         // px before the first column: room for a fork drawn as "earlier"
const WT_EARLIER = 8;      // x of a fork further back than the base drawn
const WT_BASE_COLS = 24;   // commits of the base drawn at most
const WT_THREAD_COLS = 10; // commits of one thread drawn at most

function openWorktrees(sandbox, idx) {
  Object.assign(wtView, { sandbox, idx, data: null });
  document.getElementById('wt-title').textContent = `Worktrees of ${sandbox}`;
  setHtml(wtBody, '<p class="wt-note">Reading the branches…</p>');
  wtUpdated.textContent = '';
  wtOverlay.classList.remove('hidden');
  document.getElementById('wt-close').focus();
  readWorktrees();
  clearInterval(wtView.timer);
  wtView.timer = setInterval(() => {
    if (document.visibilityState === 'visible') readWorktrees();
  }, WT_POLL_MS);
}

function closeWorktrees() {
  wtOverlay.classList.add('hidden');
  clearInterval(wtView.timer);
  Object.assign(wtView, { sandbox: null, idx: null, timer: null, data: null });
}

async function readWorktrees() {
  const sandbox = wtView.sandbox;
  if (!sandbox || wtView.busy) return;
  wtView.busy = true;
  let res;
  try {
    res = await (await fetch(`/api/sandboxes/${encodeURIComponent(sandbox)}/worktrees`)).json();
  } catch (e) {
    res = { ok: false, error: e.message };
  }
  wtView.busy = false;
  if (wtView.sandbox !== sandbox) return;
  if (!res?.ok) {
    // The last drawing stays: one failed read says nothing about the branches.
    const why = res?.error || 'no answer';
    if (!wtView.data) setHtml(wtBody, `<p class="wt-note wt-warn">${escHtml(why)}</p>`);
    wtUpdated.textContent = `Could not read them again: ${why}`;
    return;
  }
  wtView.data = res;
  paintWorktrees();
  wtUpdated.textContent = `Read at ${new Date().toLocaleTimeString()}, again every ${WT_POLL_MS / 1000} s`;
}

function paintWorktrees() {
  const { sandbox, data } = wtView;
  if (!sandbox || !data) return;
  // A lane strip keeps its scroll across repaints; a new one starts with HEAD
  // in view, at its right edge, and the forks before it.
  const scroll = new Map([...wtBody.querySelectorAll('.wt-lanes')].map(el => [el.dataset.repo, el.scrollLeft]));
  const paint = () => setHtml(wtBody, data.repos.length
    ? data.repos.map(r => wtRepoHtml(sandbox, r, data.workspace)).join('')
    : '<p class="wt-note">No thread has a worktree yet. Open one with ⑂ Thread, or let the lead plan them.</p>');
  paint();
  // The commits spread over the width the lanes got; known only once laid
  // out, so a first paint that guessed it wrong is redone before it shows.
  const w = wtBody.querySelector('.wt-lanes')?.clientWidth;
  if (w && Math.abs(w - wtView.lanes) > 4) { wtView.lanes = w; paint(); }
  wtBody.querySelectorAll('.wt-lanes').forEach(el => {
    const head = +el.querySelector('.wt-head')?.getAttribute('cx') || 0;
    el.scrollLeft = scroll.has(el.dataset.repo) ? scroll.get(el.dataset.repo) : head + WT_PAD - el.clientWidth;
  });
}

function wtAgo(at) {
  const s = Math.max(0, Date.now() / 1000 - at);
  if (s < 60) return 'just now';
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  return `${Math.floor(s / 86400)} d ago`;
}

const wtShort = sha => (sha || '').slice(0, 7);
const wtPlural = (n, one) => `${n} ${one}${n === 1 ? '' : 's'}`;

// Where each lane goes. Columns are the base's first-parent commits, oldest
// first, from just before the earliest fork to HEAD; a thread's commits follow
// its fork on its own row. A column is a commit's place in the history, not
// its time, so lanes side by side are not "at the same moment".
//
// Rows go by fork, the newest first: a thread's link down from the base then
// never crosses the lane of a thread above it, which forked at or after it.
function wtLayout(r) {
  const fp = r.log.slice().reverse();
  const col = new Map(fp.map((c, i) => [c.sha, i]));
  // A thread merged with --no-ff has its tip as the second parent of the
  // base's merge commit: it is placed at that merge.
  const mergedAt = new Map();
  fp.forEach((c, i) => c.parents.slice(1).forEach(p => { if (!mergedAt.has(p)) mergedAt.set(p, i); }));
  const at = sha => (!sha ? -1 : col.has(sha) ? col.get(sha) : mergedAt.has(sha) ? mergedAt.get(sha) : -1);
  const last = fp.length - 1;
  const forks = r.threads.map(t => at(t.fork)).filter(i => i >= 0);
  let lo = forks.length ? Math.min(...forks) - 1 : last - 4;
  lo = Math.max(0, lo, last - WT_BASE_COLS + 1);
  let cols = last - lo + 1;
  const lanes = r.threads.map(t => {
    const f = at(t.fork);
    const l = {
      t,
      fk: f >= lo ? f - lo : -1,
      merged: !!t.tip && t.ahead === 0 && !col.has(t.tip) && mergedAt.has(t.tip),
      shown: t.commits.slice(0, WT_THREAD_COLS).reverse(),
    };
    // The columns it takes: its commits after the fork, one more for "⋯"
    // when not all are drawn, and one for its uncommitted work.
    l.gap = t.ahead > l.shown.length;
    l.first = l.fk + 1 + (l.gap ? 1 : 0);
    l.end = l.merged || !t.tip ? l.fk : !t.ahead ? l.fk + 1 : l.first + l.shown.length - 1;
    if (t.dirty) l.end += 1;
    cols = Math.max(cols, l.end + 1);
    return l;
  });
  lanes.sort((a, b) => b.fk - a.fk || a.t.name.localeCompare(b.t.name));
  return { fp, lo, last, cols, lanes, mergeOf: sha => fp[mergedAt.get(sha)] };
}

function wtRepoHtml(sandbox, r, workspace) {
  const name = repoLabel(r.path, workspace);
  const head = `<div class="wt-repo-head"><span class="wt-repo-name">${escHtml(name)}</span>
    <span class="wt-repo-path">${escHtml(r.path ? `${workspace}/${r.path}` : workspace)}</span></div>`;
  if (r.error) return `<section class="wt-repo">${head}<p class="wt-note wt-warn">${escHtml(r.error)}</p></section>`;
  const g = wtLayout(r);
  const base = r.base || 'HEAD';
  const rows = 1 + g.lanes.length;
  const labels = [wtBaseLabel(r)].concat(g.lanes.map(({ t }) => wtThreadLabel(sandbox, t)));
  const facts = [wtBaseFacts(r)].concat(g.lanes.map(l => wtThreadFacts(l, base, g)));
  return `<section class="wt-repo">${head}
    <div class="wt-graph" style="--wt-rows:${rows}; --wt-row:${WT_ROW}px">
      ${labels.map((h, i) => `<div class="wt-label" style="grid-row:${i + 1}">${h}</div>`).join('')}
      <div class="wt-lanes" data-repo="${escHtml(r.path)}" style="grid-row:1 / span ${rows + 1}">${wtSvg(g, rows)}</div>
      ${facts.map((h, i) => `<div class="wt-facts" style="grid-row:${i + 1}">${h}</div>`).join('')}
    </div></section>`;
}

function wtBaseLabel(r) {
  return `<span class="wt-name wt-base-name" title="The branch the lead's checkout is on — the threads merge into it">${escHtml(r.base || 'HEAD')}</span>
    <span class="wt-sub">${r.base ? "lead's branch" : 'detached HEAD'}</span>`;
}

function wtThreadLabel(sandbox, t) {
  const { mark, state } = threadMark(sandbox, t.name);
  return `<button type="button" class="wt-name wt-go" data-thread="${escHtml(t.name)}"
      title="${escHtml(threadVerb(sandbox, t.name))}">⑂ ${escHtml(t.name)}</button>
    <span class="wt-sub"><span class="thread-mark" data-state="${state}">${mark || '•'}</span> ${THREAD_STATE_WORDS[state] ?? 'Idle'}</span>`;
}

function wtBaseFacts(r) {
  const c = r.log[0];
  const dirty = r.dirty ? ` · <span class="wt-warn" title="${wtPlural(r.dirty, 'uncommitted file')}: a merge refuses a main checkout with uncommitted changes to tracked files">${r.dirty} uncommitted</span>` : '';
  return `<span class="wt-line">HEAD <code>${escHtml(wtShort(r.head))}</code>${c ? ` · ${wtAgo(c.at)}` : ''}${dirty}</span>
    <span class="wt-sub" title="${escHtml(c?.subject || '')}">${escHtml(c?.subject || '')}</span>`;
}

// What the lane says, in words: what a merge would bring, what it may
// conflict with, and what it would leave out. The tooltips say it in full.
function wtThreadFacts({ t, merged }, base, g) {
  const dirty = t.dirty ? `<span class="wt-warn" title="${wtPlural(t.dirty, 'uncommitted file')} in its worktree: a merge takes commits only, so it would leave them out">${t.dirty} uncommitted</span>` : '';
  const behind = t.behind
    ? `<span class="wt-warn" title="${wtPlural(t.behind, 'commit')} ${escHtml(base)} gained since this thread forked: what a merge may conflict with">${t.behind} behind ${escHtml(base)}</span>` : '';
  const join = parts => parts.filter(Boolean).join(' · ');
  let line, sub;
  if (!t.tip) {
    line = join([`<span class="wt-muted">no branch ${escHtml(t.branch)}</span>`, dirty]);
    sub = 'its worktree is there, its branch is not';
  } else if (merged) {
    const m = g.mergeOf(t.tip);
    line = join([`<span class="wt-ok">✓ merged into ${escHtml(base)}</span>`, dirty]);
    sub = m ? `at <code>${escHtml(wtShort(m.sha))}</code> · ${wtAgo(m.at)}` : '';
  } else if (!t.ahead) {
    line = join(['<span class="wt-muted">no commits yet</span>', behind, dirty]);
    sub = `forked at <code>${escHtml(wtShort(t.fork))}</code>`;
  } else {
    const c = t.commits[0];
    line = join([`<span class="wt-ahead" title="${wtPlural(t.ahead, 'commit')} a merge would bring into ${escHtml(base)}">${t.ahead} to merge</span>`, behind, dirty]);
    sub = c ? `<span title="${escHtml(c.subject)}">${escHtml(c.subject)}</span> · ${wtAgo(c.at)}` : '';
  }
  return `<span class="wt-line">${line}</span><span class="wt-sub">${sub}</span>`;
}

// The lanes, as one SVG over every row so a fork can run from the base down
// to its thread. Every commit has a tooltip; the words beside each lane carry
// what the drawing says.
function wtSvg(g, rows) {
  const room = wtView.lanes - WT_PAD - 16;
  const step = Math.max(WT_STEP[0], Math.min(WT_STEP[1], Math.floor(room / Math.max(1, g.cols - 1))));
  const X = k => (k < 0 ? WT_EARLIER : WT_PAD + k * step);
  const Y = row => row * WT_ROW + WT_ROW / 2;
  const y0 = Y(0);
  const tip = c => `<title>${escHtml(`${wtShort(c.sha)} ${c.subject} — ${wtAgo(c.at)}`)}</title>`;
  // From the base at column k down to row y, turning into the lane.
  const drop = (k, y) => `M${X(k)} ${y0} L${X(k)} ${y - 8} Q${X(k)} ${y} ${X(k) + 8} ${y}`;
  const out = [];

  g.lanes.forEach(({ t, fk, merged, shown, gap, first }, i) => {
    if (!t.tip) return;
    const y = Y(i + 1);
    let end = fk;
    if (merged) {
      // Into the merge commit, from just before it.
      if (fk < 0) return;
      const x = X(fk);
      out.push(`<path class="wt-edge wt-merged" d="M${x - step / 2} ${y} L${x - 8} ${y} Q${x} ${y} ${x} ${y - 8} L${x} ${y0}"/>`,
        `<circle class="wt-dot wt-merged" cx="${x - step / 2}" cy="${y}" r="4"><title>${escHtml(`${wtShort(t.tip)} merged`)}</title></circle>`);
    } else if (!t.ahead) {
      // A branch with nothing on it yet: a link to an empty mark.
      end = fk + 1;
      out.push(`<path class="wt-edge wt-faint" d="${fk >= 0 ? drop(fk, y) : `M${X(fk)} ${y}`} L${X(end) - 6} ${y}"/>`,
        `<circle class="wt-hollow" cx="${X(end)}" cy="${y}" r="4"><title>${escHtml(`${wtShort(t.tip)} — no commits of its own yet`)}</title></circle>`);
    } else {
      end = first + shown.length - 1;
      out.push(fk >= 0
        ? `<path class="wt-edge wt-thread" d="${drop(fk, y)} L${X(end)} ${y}"/>`
        : `<path class="wt-edge wt-faint" d="M${X(fk)} ${y} L${X(0)} ${y}"/><path class="wt-edge wt-thread" d="M${X(0)} ${y} L${X(end)} ${y}"/>`);
      if (gap) {
        out.push(`<g class="wt-gap"><rect x="${X(fk + 1) - 8}" y="${y - 7}" width="16" height="14"/><text x="${X(fk + 1)}" y="${y + 4}">⋯</text>`
          + `<title>${wtPlural(t.ahead - shown.length, 'older commit')} of this thread</title></g>`);
      }
      shown.forEach((c, j) => out.push(`<circle class="wt-dot wt-thread" cx="${X(first + j)}" cy="${y}" r="4">${tip(c)}</circle>`));
    }
    if (t.dirty) {
      const x = X(end + 1);
      out.push(`<path class="wt-edge wt-dirty" d="M${X(end) + 5} ${y} L${x - 6} ${y}"/>`,
        `<circle class="wt-hollow wt-dirty" cx="${x}" cy="${y}" r="4.5"><title>${wtPlural(t.dirty, 'uncommitted file')}</title></circle>`);
    }
  });

  // The base last, so its dots sit over the links drawn from them.
  const older = g.lo > 0 || (g.fp[0]?.parents.length ?? 0) > 0;
  if (older && g.fp.length) out.push(`<path class="wt-edge wt-faint" d="M${X(-1)} ${y0} L${X(0)} ${y0}"/>`);
  if (g.last >= g.lo) out.push(`<path class="wt-edge wt-base" d="M${X(0)} ${y0} L${X(g.last - g.lo)} ${y0}"/>`);
  for (let k = 0; k <= g.last - g.lo; k++) {
    const isHead = k === g.last - g.lo;
    out.push(`<circle class="wt-dot wt-base${isHead ? ' wt-head' : ''}" cx="${X(k)}" cy="${y0}" r="${isHead ? 5.5 : 4}">${tip(g.fp[g.lo + k])}</circle>`);
  }
  const width = X(Math.max(g.cols, 1) - 1) + WT_PAD / 2;
  return `<svg class="wt-svg" width="${width}" height="${rows * WT_ROW}" aria-hidden="true">${out.join('')}</svg>`;
}

wtBody.addEventListener('click', e => {
  const b = e.target.closest('[data-thread]');
  if (!b || !wtView.sandbox) return;
  const { sandbox, idx } = wtView;
  closeWorktrees();
  const from = idx != null && panes[idx]?.sandbox === sandbox ? idx : focusedPane;
  reachThread(sandbox, b.dataset.thread, from);
});
document.getElementById('wt-x').addEventListener('click', closeWorktrees);
document.getElementById('wt-close').addEventListener('click', closeWorktrees);
wtOverlay.addEventListener('click', e => { if (e.target === wtOverlay) closeWorktrees(); });
document.addEventListener('keydown', e => {
  if (e.key === 'Escape' && !wtOverlay.classList.contains('hidden')) closeWorktrees();
});
