// ── Team board ────────────────────────────────────────────────────────────
// In a shared plan the threads work in one worktree, and the board says who
// holds which file and what they have told each other. It replaces the
// worktree graph in that dialog (see worktrees.js) and feeds the plan dialog's
// commits, open claims and distillate (see plans.js).
//
// GET /api/sandboxes/:name/board answers
//   { sandbox, plan, claims: [{file, owner, since, until, extensions, held_ms}],
//     entries: [{id, at, kind, by, to?, text?, files?, hash?}] }
// with kind one of claim | release | commit | conflict | decision | question |
// handoff | warning. Times are unix seconds (milliseconds are taken too).
// `plan` is the sandbox's shared plan at work, in full (id, variant, threads),
// or null when there is none — then the dialog draws the graph instead.

const BOARD_AMBER_MS = 90 * 60 * 1000; // a claim held this long reads amber
// What each kind of entry is called and marked; the feed filters by these.
const BOARD_KINDS = {
  decision: ['◆', 'Decision'],
  warning: ['▲', 'Warning'],
  question: ['?', 'Question'],
  commit: ['●', 'Commit'],
  conflict: ['✕', 'Conflict'],
  handoff: ['→', 'Handoff'],
  claim: ['⌖', 'Claim'],
  release: ['○', 'Release'],
};
// The filter chips: what a person scans for. The rest are under "All".
const BOARD_FILTER_KINDS = ['decision', 'warning', 'question', 'commit'];
const boardFilter = { thread: '', kind: '' }; // '' is everyone, everything

// The board of `sandbox`, or null when there is none to read (a daemon
// without the endpoint, or no shared plan).
async function fetchBoard(sandbox) {
  try {
    const res = await (await fetch(`/api/sandboxes/${encodeURIComponent(sandbox)}/board`)).json();
    return res && Array.isArray(res.claims) && Array.isArray(res.entries) ? res : null;
  } catch { return null; }
}

const boardIsShared = b => b?.plan?.variant === 'shared';

const boardSecs = t => (t > 1e12 ? t / 1000 : t);
const boardClock = t => new Date(boardSecs(t) * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
function boardSpan(ms) {
  const m = Math.max(0, Math.round(ms / 60000));
  return m < 60 ? `${m} min` : `${Math.floor(m / 60)} h ${String(m % 60).padStart(2, '0')}`;
}

// "7 files claimed · 2 open questions", for the Thread menu miniature.
// A question stays open until the thread it asks answers with a decision,
// handoff or warning after it.
function boardOpenQuestions(b) {
  return b.entries.filter(q => q.kind === 'question'
    && !b.entries.some(e => e.at > q.at && (!q.to || e.by === q.to) && ['decision', 'handoff', 'warning'].includes(e.kind))).length;
}
function boardSummary(b) {
  const n = b.claims.length, q = boardOpenQuestions(b);
  return `${wtPlural(n, 'file')} claimed · ${wtPlural(q, 'open question')}`;
}

// Files with a conflict logged since their claim began: red.
function boardConflicted(b, c) {
  return b.entries.some(e => e.kind === 'conflict' && boardSecs(e.at) >= boardSecs(c.since) && (e.files || []).includes(c.file));
}

// Each thread keeps one hue from the plan's order, as on the plan cards.
function boardHue(b, name) {
  const names = (b.plan?.threads || []).map(t => t.name);
  let i = names.indexOf(name);
  if (i < 0) i = [...new Set([...b.claims.map(c => c.owner), ...b.entries.map(e => e.by)])].sort().indexOf(name);
  return `var(${PLAN_HUES[Math.max(0, i) % PLAN_HUES.length]})`;
}

function boardClaimHtml(b, c) {
  const left = boardSecs(c.until) * 1000 - Date.now();
  const state = boardConflicted(b, c) ? 'conflict' : c.held_ms >= BOARD_AMBER_MS ? 'long' : '';
  const expiry = left > 0 ? `until ${boardClock(c.until)} · ${boardSpan(left)} left` : 'lease lapsed';
  const ext = c.extensions ? ` · extended ${c.extensions}×` : '';
  return `<li class="bd-claim${state ? ` ${state}` : ''}" style="--bd-hue: ${boardHue(b, c.owner)}">
      <code class="bd-file" title="${escHtml(c.file)}">${escHtml(c.file)}</code>
      <span class="bd-owner">${escHtml(c.owner)}</span>
      <span class="bd-when">${escHtml(expiry)}${ext}<span class="bd-held"> · held ${boardSpan(c.held_ms)}</span></span>
      ${state ? `<span class="bd-flag">${state === 'conflict' ? 'conflict' : 'held long'}</span>` : ''}
    </li>`;
}

function boardEntryHtml(b, e) {
  const [mark, word] = BOARD_KINDS[e.kind] || ['•', e.kind];
  const who = e.to ? `${escHtml(e.by)} → ${escHtml(e.to)}` : escHtml(e.by);
  const files = (e.files || []).map(f => `<code class="bd-chip">${escHtml(f)}</code>`).join('');
  return `<li class="bd-entry" data-kind="${escHtml(e.kind)}" style="--bd-hue: ${boardHue(b, e.by)}">
      <span class="bd-mark" aria-hidden="true">${mark}</span>
      <div class="bd-entry-main">
        <div class="bd-entry-head"><span class="bd-kind">${word}</span> <span class="bd-by">${who}</span>
          ${e.hash ? `<code class="bd-hash">${escHtml(wtShort(e.hash))}</code>` : ''}
          <time class="bd-at" title="${escHtml(new Date(boardSecs(e.at) * 1000).toLocaleString())}">${wtAgo(boardSecs(e.at))}</time></div>
        ${e.text ? `<p class="bd-text">${escHtml(e.text)}</p>` : ''}
        ${files ? `<div class="bd-files">${files}</div>` : ''}
      </div>
    </li>`;
}

function boardChips(group, items, current) {
  return items.map(([val, label]) =>
    `<button type="button" class="bd-chipbtn" data-bd-${group}="${escHtml(val)}" aria-pressed="${val === current}">${escHtml(label)}</button>`).join('');
}

// The whole view as html. `boardFilter` narrows the feed only: claims are
// always all shown, since they are what blocks.
function boardHtml(b) {
  const threads = [...new Set([...b.claims.map(c => c.owner), ...b.entries.map(e => e.by)])].sort();
  const feed = b.entries.filter(e => (!boardFilter.thread || e.by === boardFilter.thread || e.to === boardFilter.thread)
    && (!boardFilter.kind || e.kind === boardFilter.kind)).sort((x, y) => y.at - x.at);
  const claims = b.claims.slice().sort((x, y) => boardSecs(x.until) - boardSecs(y.until));
  return `<div class="bd">
    <p class="bd-summary">${escHtml(boardSummary(b))}</p>
    <section class="bd-sec"><h3 class="bd-h">Claims</h3>
      ${claims.length ? `<ul class="bd-claims">${claims.map(c => boardClaimHtml(b, c)).join('')}</ul>`
        : '<p class="wt-note">No file is claimed.</p>'}
    </section>
    <section class="bd-sec"><h3 class="bd-h">Feed</h3>
      <div class="bd-filters">
        <div class="bd-chips" role="group" aria-label="Filter by thread">${boardChips('thread', [['', 'Everyone'], ...threads.map(t => [t, t])], boardFilter.thread)}</div>
        <div class="bd-chips" role="group" aria-label="Filter by type">${boardChips('kind', [['', 'All'], ...BOARD_FILTER_KINDS.map(k => [k, BOARD_KINDS[k][1] + 's'])], boardFilter.kind)}</div>
      </div>
      ${feed.length ? `<ol class="bd-feed">${feed.map(e => boardEntryHtml(b, e)).join('')}</ol>`
        : '<p class="wt-note">Nothing on the board matches.</p>'}
    </section></div>`;
}

// Filter clicks, wherever the board is painted: `host` repaints through `redo`.
function boardBindFilters(host, redo) {
  host.addEventListener('click', e => {
    const t = e.target.closest('[data-bd-thread],[data-bd-kind]');
    if (!t) return;
    if (t.dataset.bdThread !== undefined) boardFilter.thread = t.dataset.bdThread;
    else boardFilter.kind = t.dataset.bdKind;
    redo();
  });
}
