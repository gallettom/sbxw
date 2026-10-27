// ── Project panel ─────────────────────────────────────────────────────────
//
// A file explorer over the sandbox's filesystem, opening on `.sbxw-artifacts/`
// — what this repository has produced — with the code map button beside it,
// and a Recent view above the folders for the files an agent left elsewhere.
// The whole filesystem is reachable, not just the deliverables: `/home/agent`,
// a tool's cache, a log under `/tmp`. While the sandbox runs it is read through
// `sbx exec`; stopped, the daemon falls back to the workspace on the host and
// says so (see src/sandbox_fs.rs). Browse, preview, download — and delete,
// for the stray file, under the places an agent writes to (see `fxDelete`).
const filesOverlay = document.getElementById('files-modal-overlay');
const filesNameEl  = document.getElementById('files-modal-name');
const fxTbody      = document.getElementById('fx-tbody');
let filesTarget = null;
// What the last `/artifacts` fetch said this project has. Held rather than
// passed straight to the renderer because the button repaints on its own
// schedule too — a run starting or ending elsewhere changes what it should say
// without changing a single file (see `applyCodemapRun`).
let filesCodemap = { map: null, lenses: [], run: null };

// The explorer's own state. `places` is what the daemon says this sandbox's
// fixed favourites are — only it knows where the workspace is mounted.
const fx = {
  path: '',
  parent: null,
  entries: [],
  history: [],
  selected: null,
  offline: false,
  // The Recent view, when it is the one on screen: the daemon's newest files
  // across the workspace, the RW mounts, home and /tmp. `null` while browsing a
  // folder. `fresh` is what the favourite's badge counts — kept apart so the
  // badge survives leaving the view.
  recent: null,
  fresh: null,
  places: { workspace: null, artifacts: null, home: '/home/agent', mounts: [], worktrees: [] },
  seq: 0,              // the newest listing request; older answers are dropped
};

function humanSize(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB'];
  let v = bytes / 1024, i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return `${v.toFixed(1)} ${units[i]}`;
}

/// Short and in the browser's own locale: day and month, plus the time for
/// this year or the year for older ones. Seconds told nobody anything, and a
/// column of "8/17/2026, 3:12:16 PM" was mostly noise around the date.
function humanTime(unixSecs) {
  if (!unixSecs) return '—';
  const d = new Date(unixSecs * 1000);
  return d.toLocaleString(undefined, d.getFullYear() === new Date().getFullYear()
    ? { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' }
    : { day: 'numeric', month: 'short', year: 'numeric' });
}

/// The full timestamp, for a tooltip on the short one.
function fullTime(unixSecs) {
  return unixSecs ? new Date(unixSecs * 1000).toLocaleString() : '';
}

/// "4 min ago", "3 h ago", "yesterday" — for the Recent view, where how long
/// ago is the whole question and a date makes you do the subtraction.
function agoTime(unixSecs) {
  if (!unixSecs) return '—';
  const s = Math.max(0, Date.now() / 1000 - unixSecs);
  if (s < 60) return 'just now';
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  if (s < 2 * 86400) return 'yesterday';
  return `${Math.floor(s / 86400)} days ago`;
}

/// Written in the last hour: the dot a listing puts on a file or folder, so a
/// stray file shows up in its folder too, not only in Recent.
const FX_FRESH_SECS = 3600;
const isFresh = (e) => e.modified && Date.now() / 1000 - e.modified < FX_FRESH_SECS;

const fxApi = (path, extra = '') =>
  `/api/sandboxes/${encodeURIComponent(filesTarget)}/fs${path}${extra}`;
const fxFileUrl = (path, extra = '') =>
  fxApi('/file', `?path=${encodeURIComponent(path)}${extra}`);
const fxJoin = (dir, name) => (dir === '/' ? '' : dir) + '/' + name;
// A row's identity and its absolute path. Recent rows come from all over the
// filesystem — two `notes.md` are two rows — so they are told apart by path.
const fxKey = (e) => e.path || e.name;
const fxPathOf = (e) => e.path || fxJoin(fx.path, e.name);

/* ── Favourites ──
   Four are built in and always there — the deliverables folder first, since
   that is what this panel opened onto before it was an explorer — and any
   folder can be starred on top of them. Starred folders are a per-browser
   convenience, kept in localStorage per sandbox; storage that throws (a
   private window) just means nothing is remembered. */
const FX_FAV_KEY = (name) => `sbxw.fx.favourites.${name}`;

function fxUserFavourites() {
  try { return JSON.parse(localStorage.getItem(FX_FAV_KEY(filesTarget)) || '[]'); }
  catch (_) { return []; }
}

function fxSaveFavourites(list) {
  try { localStorage.setItem(FX_FAV_KEY(filesTarget), JSON.stringify(list)); } catch (_) {}
}

/// One line icon per kind of place, drawn at 16px and coloured by kind (see
/// `.fx-fav-icon.<kind>` in app.css): the glyphs they replace were a few pixels
/// of dim text, and a worktree, a mount and home could not be told apart.
const FX_FAV_SVG = {
  recent: '<circle cx="8" cy="8" r="6"/><path d="M8 4.5V8l2.5 1.5"/>',
  artifacts: '<path d="M8 1.8l1.9 3.9 4.3.6-3.1 3 .7 4.3L8 11.6l-3.8 2 .7-4.3-3.1-3 4.3-.6z"/>',
  workspace: '<path d="M1.5 3.5h4l1.2 1.6h7.3a.5.5 0 0 1 .5.5v7a.5.5 0 0 1-.5.5h-12a.5.5 0 0 1-.5-.5v-9a.5.5 0 0 1 .5-.5z"/>',
  mount: '<ellipse cx="8" cy="3.8" rx="5.5" ry="2"/><path d="M2.5 3.8v8.4c0 1.1 2.5 2 5.5 2s5.5-.9 5.5-2V3.8M2.5 8c0 1.1 2.5 2 5.5 2s5.5-.9 5.5-2"/>',
  thread: '<circle cx="4.5" cy="3.5" r="1.6"/><circle cx="4.5" cy="12.5" r="1.6"/><circle cx="11.5" cy="5.5" r="1.6"/><path d="M4.5 5.1v5.8M11.5 7.1c0 2.6-7 2-7 3.8"/>',
  home: '<path d="M2 7.5L8 2.5l6 5M3.5 6.5V13.5h3.2V10h2.6v3.5h3.2V6.5"/>',
  root: '<rect x="1.5" y="4" width="13" height="8" rx="1.2"/><path d="M4 8h.01M7 8h5"/>',
  starred: '<path d="M8 1.8l1.9 3.9 4.3.6-3.1 3 .7 4.3L8 11.6l-3.8 2 .7-4.3-3.1-3 4.3-.6z"/>',
};
const fxFavIcon = (kind) => `<svg class="fx-fav-icon ${kind}" width="16" height="16" viewBox="0 0 16 16"
  fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"
  aria-hidden="true">${FX_FAV_SVG[kind]}</svg>`;

function fxBuiltins() {
  const p = fx.places;
  return [
    // Not a folder: the newest files wherever they were written. First, because
    // "what did the agent just make, and where did it put it" is the question
    // this panel is opened with most.
    { path: FX_RECENT, label: 'Recent', icon: 'recent', recent: true },
    p.artifacts && { path: p.artifacts, label: '.sbxw-artifacts', icon: 'artifacts', main: true },
    p.workspace && { path: p.workspace, label: 'Workspace', icon: 'workspace' },
    // The other repos mounted beside it, each saying how: an RO one is there
    // to read, and the agent is refused any write to it.
    ...p.mounts.map((m) => ({
      path: m.path,
      label: m.path.split('/').pop() || m.path,
      icon: 'mount',
      mode: m.readonly ? 'ro' : 'rw',
    })),
    // Each thread's worktree, on its deliverables when it has any: what a
    // thread made stays on its branch, under .sbxw-worktrees/, until merged.
    ...p.worktrees.map((w) => ({
      path: w.artifacts || w.path,
      label: w.thread,
      icon: 'thread',
      thread: w,
    })),
    { path: p.home, label: 'Home', icon: 'home' },
    { path: '/', label: 'Root', icon: 'root' },
  ].filter(Boolean);
}

/// The thread whose worktree `path` is in, if any.
function fxWorktreeOf(path) {
  return fx.places.worktrees.find((w) => path === w.path || path.startsWith(w.path + '/')) || null;
}

function fxRenderFavourites() {
  const builtin = fxBuiltins();
  const builtinPaths = new Set(builtin.map((f) => f.path));
  const user = fxUserFavourites().filter((p) => !builtinPaths.has(p));
  const here = fx.recent ? FX_RECENT : fx.path;
  const row = (f, removable) => `
    <div class="fx-fav${f.path === here ? ' active' : ''}${f.main ? ' main' : ''}${f.recent ? ' recent' : ''}"
         data-path="${escHtml(f.path)}"
         title="${escHtml(f.recent ? 'Files written in the last few days — workspace, mounts, home and /tmp'
           : f.thread ? `Thread "${f.thread.thread}" — ${f.path}, branch sbxw/${f.thread.thread}`
           : f.path + (f.mode === 'ro' ? ' — read-only: the agent can read it, not change it'
                                  : f.mode === 'rw' ? ' — read-write' : ''))}">
      ${fxFavIcon(f.icon)}
      <span class="fx-fav-name">${escHtml(f.label)}</span>
      ${f.mode ? `<span class="fx-fav-mode ${f.mode}">${f.mode === 'ro' ? 'RO' : 'RW'}</span>` : ''}
      ${f.recent && fx.fresh ? `<span class="fx-fav-count" title="${fx.fresh} file${
        fx.fresh === 1 ? '' : 's'} written in the last hour">${fx.fresh}</span>` : ''}
      ${removable ? `<button class="fx-fav-rm" data-unfav="${escHtml(f.path)}"
        title="Remove from favourites" aria-label="Remove from favourites">✕</button>` : ''}
    </div>`;
  // The threads' worktrees under a label of their own, after the places every
  // sandbox has: a list of bare thread names read as more folders.
  const places = builtin.filter((f) => !f.thread);
  const threads = builtin.filter((f) => f.thread);
  document.getElementById('fx-favourites').innerHTML =
    places.map((f) => row(f, false)).join('')
    + (threads.length ? `<div class="cm-side-label fx-fav-group">Threads</div>${
      threads.map((f) => row(f, false)).join('')}` : '')
    + (user.length ? '<div class="fx-fav-sep"></div>' : '')
    + user.map((p) => row({ path: p, label: p.split('/').pop() || '/', icon: 'starred' }, true)).join('');
  syncStar();
}

function syncStar() {
  const star = document.getElementById('fx-star');
  const builtin = !!fx.recent || fxBuiltins().some((f) => f.path === fx.path);
  const starred = builtin || fxUserFavourites().includes(fx.path);
  star.classList.toggle('on', starred);
  star.setAttribute('aria-pressed', String(starred));
  star.disabled = builtin || !fx.path;
  star.title = builtin ? 'Built-in favourite'
    : starred ? 'Remove this folder from favourites' : 'Add this folder to favourites';
}

function fxToggleFavourite(path) {
  if (!path || fxBuiltins().some((f) => f.path === path)) return;
  const list = fxUserFavourites();
  const i = list.indexOf(path);
  if (i === -1) list.push(path); else list.splice(i, 1);
  fxSaveFavourites(list);
  fxRenderFavourites();
}

/* ── Listing ── */

function fxNotice(text, kind) {
  const el = document.getElementById('fx-notice');
  el.textContent = text || '';
  el.className = `fx-notice${text ? '' : ' hidden'}${kind ? ` ${kind}` : ''}`;
}

async function fxOpen(path, { push = true, select = null } = {}) {
  if (!filesTarget) return;
  const seq = ++fx.seq;
  fxTbody.innerHTML = '<tr><td colspan="4" class="ports-empty-msg">Loading…</td></tr>';
  let data;
  try {
    const res = await fetch(fxApi('', `?path=${encodeURIComponent(path || '')}`));
    data = await res.json();
  } catch (_) {
    data = { ok: false, error: 'could not reach sbxw' };
  }
  if (seq !== fx.seq) return;            // a newer click won
  if (!data.ok) {
    fxNotice(data.error || 'could not list that folder', 'error');
    // Nothing on screen yet (the first open failed): show something rather
    // than a spinner forever.
    if (!fx.path) fxTbody.innerHTML = '<tr><td colspan="4" class="ports-empty-msg">Nothing to show</td></tr>';
    else renderList();
    return;
  }
  if (push && fx.recent) fx.history.push(FX_RECENT);
  else if (push && fx.path && fx.path !== data.path) fx.history.push(fx.path);
  fx.recent = null;
  document.querySelector('.fx-list').classList.remove('recent');
  fx.path = data.path;
  fx.parent = data.parent;
  fx.entries = data.entries || [];
  fx.offline = !!data.offline;
  fx.places = {
    workspace: data.workspace || null,
    artifacts: data.artifacts || null,
    home: data.home || '/home/agent',
    mounts: Array.isArray(data.mounts) ? data.mounts : [],
    worktrees: Array.isArray(data.worktrees) ? data.worktrees : [],
  };
  // A folder with a README opens on it, the way a repository page does: the
  // preview has something to say before anything is clicked.
  if (!select) select = fx.entries.find((e) => e.kind === 'file' && FX_README.test(e.name))?.name || null;
  fx.selected = select;
  const ro = fx.places.mounts.find((m) => m.readonly && (fx.path === m.path || fx.path.startsWith(m.path + '/')));
  const wt = fxWorktreeOf(fx.path);
  fxNotice(fx.offline
    ? 'This sandbox is stopped — showing its workspace from the host. Start it to browse the rest of its filesystem.'
    : ro ? `Read-only — ${ro.path.split('/').pop()} is mounted RO: the agent can read it, not change it.`
    : wt ? `Thread "${wt.thread}"'s worktree — branch sbxw/${wt.thread}, not in the main checkout until it is merged.`
    : '');
  document.getElementById('fx-filter').value = '';
  renderCrumbs();
  fxRenderFavourites();
  renderList();
  if (select) {
    const entry = fx.entries.find((e) => e.name === select);
    if (entry) fxShow(entry); else fxPreviewEmpty();
  } else {
    fxPreviewEmpty();
  }
  document.getElementById('fx-back').disabled = !fx.history.length;
  document.getElementById('fx-up').disabled = !fx.parent;
}

function renderCrumbs() {
  if (fx.recent) {
    const el = document.getElementById('fx-crumbs');
    const n = fx.entries.length;
    el.innerHTML = `<span class="fx-crumb last">◷ Recent</span><span class="fx-crumb-sep">—</span>
      <span class="fx-crumb-note">${n} file${n === 1 ? '' : 's'} written in the last ${fx.recent.days || 7} days,
      newest first</span>`;
    return;
  }
  const parts = fx.path.split('/').filter(Boolean);
  let acc = '';
  const crumbs = [`<button class="fx-crumb" data-path="/">/</button>`];
  parts.forEach((p, i) => {
    acc += '/' + p;
    crumbs.push(`<button class="fx-crumb${i === parts.length - 1 ? ' last' : ''}"
      data-path="${escHtml(acc)}">${escHtml(p)}</button>`);
    if (i < parts.length - 1) crumbs.push('<span class="fx-crumb-sep">/</span>');
  });
  const el = document.getElementById('fx-crumbs');
  el.innerHTML = crumbs.join('');
  el.scrollLeft = el.scrollWidth;        // the deep end is the part you need
}

const FX_ICONS = { dir: '📁', file: '📄', other: '·' };
const FX_README = /^readme(\.(md|markdown|txt))?$/i;

/// The rows on screen, in order: what the filter and the dotfile toggle leave.
function fxVisible() {
  const q = document.getElementById('fx-filter').value.trim().toLowerCase();
  const hidden = document.getElementById('fx-show-hidden').checked;
  // Recent filters on the whole path — "which of these went under /tmp" — and
  // ignores the dotfile toggle: a file hidden in a dot-folder is exactly the
  // one this view is for.
  if (fx.recent) return fx.entries.filter((e) => !q || e.path.toLowerCase().includes(q));
  return fx.entries.filter((e) =>
    // Dotfiles are hidden by default — except the deliverables, and the threads'
    // worktrees, where a thread's deliverables are until it is merged.
    (hidden || !e.name.startsWith('.') || e.name === '.sbxw-artifacts' || e.name === '.sbxw-worktrees')
    && (!q || e.name.toLowerCase().includes(q)));
}

function renderList() {
  if (fx.recent) { renderRecentList(); return; }
  const rows = fxVisible();
  if (!rows.length) {
    fxTbody.innerHTML = `<tr><td colspan="4" class="ports-empty-msg">${
      fx.entries.length ? 'Nothing matches' : 'Empty folder'}</td></tr>`;
    return;
  }
  fxTbody.innerHTML = rows.map((e) => `
    <tr class="fx-row${e.kind === 'dir' ? ' dir' : ''}${e.name === fx.selected ? ' selected' : ''}"
        data-key="${escHtml(e.name)}">
      <td class="fx-name"><span class="fx-icon">${FX_ICONS[e.kind] || '·'}</span>
        <span class="fx-label">${escHtml(e.name)}</span>${e.link ? '<span class="fx-link" title="Symbolic link">↗</span>' : ''}${
        isFresh(e) ? `<span class="fx-fresh" title="${e.kind === 'dir' ? 'Changed' : 'Written'} in the last hour"></span>` : ''}</td>
      <td class="fx-num">${e.kind === 'file' ? humanSize(e.size) : '—'}</td>
      <td class="fx-time" title="${escHtml(fullTime(e.modified))}">${humanTime(e.modified)}</td>
      <td class="fx-act">${e.kind === 'file'
        ? `<button class="fx-dl" type="button" tabindex="-1" title="Download"
             aria-label="Download ${escHtml(e.name)}">⬇</button>` : ''}${fxDeleteButton(e)}</td>
    </tr>`).join('');
  fxTbody.querySelector('.fx-row.selected')?.scrollIntoView({ block: 'nearest' });
}

/* ── Recent ──
   Not a folder but a question: what was written lately, and where. The daemon
   scans the workspace, the RW mounts, home and /tmp, skipping .git, dependency
   and cache folders (see RECENT_PRUNE in src/sandbox_fs.rs), and answers with
   the newest files first. Shown in the same table, grouped by how long ago,
   each with the folder it landed in — the part you could not have guessed. */
const FX_RECENT = 'recent:';

/// Where a file is, said the short way: `./` for the workspace, `~/` for home.
function fxShortDir(path) {
  const dir = path.slice(0, path.lastIndexOf('/')) || '/';
  const wt = fxWorktreeOf(dir);
  if (wt) return `⑂ ${wt.thread}` + dir.slice(wt.path.length);
  const ws = fx.places.workspace;
  if (ws && (dir === ws || dir.startsWith(ws + '/'))) return '.' + dir.slice(ws.length);
  const home = fx.places.home;
  if (home && (dir === home || dir.startsWith(home + '/'))) return '~' + dir.slice(home.length);
  return dir;
}

function fxRecentEntries(files) {
  return (files || []).map((f) => ({
    name: f.path.slice(f.path.lastIndexOf('/') + 1),
    path: f.path,
    kind: 'file',
    size: f.size,
    modified: f.modified,
  }));
}

/// Fetch the recent files. Returns the daemon's answer, or `null` when it had
/// none to give; the favourite's badge is refreshed either way.
async function fxFetchRecent(name) {
  try {
    const res = await fetch(`/api/sandboxes/${encodeURIComponent(name)}/fs/recent`);
    const data = await res.json();
    if (filesTarget !== name) return null;
    if (!data.ok) return data;
    const now = Date.now() / 1000;
    fx.fresh = (data.files || []).filter((f) => now - f.modified < FX_FRESH_SECS).length;
    fxRenderFavourites();
    return data;
  } catch (_) {
    return { ok: false, error: 'could not reach sbxw' };
  }
}

async function fxOpenRecent({ push = true, select = null } = {}) {
  if (!filesTarget) return;
  const name = filesTarget;
  const seq = ++fx.seq;
  fxTbody.innerHTML = '<tr><td colspan="4" class="ports-empty-msg">Looking for recent files…</td></tr>';
  const data = await fxFetchRecent(name);
  if (seq !== fx.seq || !data) return;
  if (!data.ok) { fxNotice(data.error || 'could not list recent files', 'error'); renderList(); return; }
  if (push && !fx.recent && fx.path) fx.history.push(fx.path);
  fx.recent = data;
  fx.places.workspace ||= data.workspace || null;
  fx.entries = fxRecentEntries(data.files);
  fx.parent = null;
  fx.selected = select && fx.entries.some((e) => e.path === select) ? select : null;
  fxNotice(data.offline
    ? 'This sandbox is stopped — only its workspace was searched. Start it to include home and /tmp.' : '');
  document.getElementById('fx-filter').value = '';
  document.querySelector('.fx-list').classList.add('recent');
  renderCrumbs();
  fxRenderFavourites();
  renderList();
  const entry = fx.selected && fx.entries.find((e) => e.path === fx.selected);
  if (entry) fxPreview(entry); else fxPreviewEmpty();
  document.getElementById('fx-back').disabled = !fx.history.length;
  document.getElementById('fx-up').disabled = true;
}

/// Bucket by age, newest first — the list is already in that order.
function fxAgeGroup(unixSecs) {
  const s = Date.now() / 1000 - unixSecs;
  if (s < 3600) return 'Last hour';
  const today = new Date(); today.setHours(0, 0, 0, 0);
  if (unixSecs * 1000 >= today.getTime()) return 'Today';
  if (unixSecs * 1000 >= today.getTime() - 86400000) return 'Yesterday';
  return 'Earlier this week';
}

function renderRecentList() {
  const rows = fxVisible();
  if (!rows.length) {
    fxTbody.innerHTML = `<tr><td colspan="4" class="ports-empty-msg">${fx.entries.length
      ? 'Nothing matches' : `Nothing written in the last ${fx.recent.days || 7} days`}</td></tr>`;
    return;
  }
  let group = null;
  fxTbody.innerHTML = rows.map((e) => {
    const g = fxAgeGroup(e.modified);
    const head = g !== group ? `<tr class="fx-group"><td colspan="4">${escHtml(g)}</td></tr>` : '';
    group = g;
    const dir = fxShortDir(e.path);
    return `${head}
    <tr class="fx-row${e.path === fx.selected ? ' selected' : ''}" data-key="${escHtml(e.path)}">
      <td class="fx-name" title="${escHtml(e.path)}"><span class="fx-icon">${FX_ICONS.file}</span>
        <span class="fx-label">${escHtml(e.name)}</span>
        <span class="fx-where">${escHtml(dir)}</span></td>
      <td class="fx-num">${humanSize(e.size)}</td>
      <td class="fx-time" title="${escHtml(fullTime(e.modified))}">${agoTime(e.modified)}</td>
      <td class="fx-act"><button class="fx-dl" type="button" tabindex="-1" title="Download"
             aria-label="Download ${escHtml(e.name)}">⬇</button>${fxDeleteButton(e)}</td>
    </tr>`;
  }).join('');
  fxTbody.querySelector('.fx-row.selected')?.scrollIntoView({ block: 'nearest' });
}

/* ── Delete ──
   The one write this panel makes: removing what an agent left where it should
   not have. Offered only under the places an agent writes to — the workspace,
   the read-write mounts, home and /tmp — and never on one of those itself, the
   deliverables folder or the repository's .git. The daemon checks the same
   again, on the resolved path (see refuse_delete in src/sandbox_fs.rs); this
   copy only decides whether to show the button. Always confirmed. */
function fxCanDelete(path) {
  const p = fx.places;
  const ws = p.workspace;
  const roots = [ws, ...p.mounts.filter((m) => !m.readonly).map((m) => m.path), p.home, '/tmp']
    .filter(Boolean).map((r) => r.replace(/\/+$/, ''));
  // A worktree goes with its thread (Close thread), never from here.
  const kept = [...(ws ? [`${ws}/.sbxw-artifacts`, `${ws}/.git`, `${ws}/.sbxw-worktrees`] : []),
    ...p.worktrees.map((w) => w.path)];
  if (roots.includes(path) || kept.includes(path)) return false;
  if (p.mounts.some((m) => m.readonly && path.startsWith(m.path + '/'))) return false;
  return roots.some((r) => path.startsWith(r + '/'));
}

function fxDeleteButton(e) {
  if (e.kind === 'other' && !e.link) return '';
  return fxCanDelete(fxPathOf(e)) ? `<button class="fx-dl fx-rm" type="button" tabindex="-1"
    title="Delete" aria-label="Delete ${escHtml(e.name)}">🗑</button>` : '';
}

async function fxDelete(entry) {
  if (!filesTarget) return;
  const name = filesTarget;
  const path = fxPathOf(entry);
  if (!fxCanDelete(path)) return;
  const what = entry.kind === 'dir' && !entry.link ? 'the folder' : entry.link ? 'the link' : 'the file';
  if (!confirm(`Delete ${what} ${path}?${entry.kind === 'dir' && !entry.link
    ? '\n\nEverything in it goes too.' : ''}\n\nThis cannot be undone.`)) return;
  let data;
  try {
    const res = await fetch(`/api/sandboxes/${encodeURIComponent(name)}/fs/delete`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path }),
    });
    data = await res.json();
  } catch (_) {
    data = { ok: false, error: 'could not reach sbxw' };
  }
  if (!data.ok) { showToast(data.error || 'Could not delete it', 'error'); return; }
  showToast(`Deleted ${entry.name}`);
  if (filesTarget !== name) return;
  // The neighbour takes the selection, so ⌫ ⌫ ⌫ walks down a list of strays.
  const rows = fxVisible();
  const i = rows.findIndex((r) => fxKey(r) === fxKey(entry));
  const next = rows[i + 1] || rows[i - 1];
  const select = next ? fxKey(next) : null;
  if (fx.recent) { fxOpenRecent({ push: false, select }); return; }
  fxOpen(fx.path, { push: false, select });
  fxFetchRecent(name);
}

function fxDownload(entry) {
  const a = document.createElement('a');
  a.href = fxFileUrl(fxPathOf(entry), '&download=1');
  a.download = entry.name;
  a.click();
}

/// Select a row and show it on the right: a file is previewed, a folder says
/// what Return will do. Used by the keyboard, which walks folders without
/// opening them, as well as by the README a folder opens on.
function fxShow(entry) {
  if (entry.kind === 'file') { fxPreview(entry); return; }
  fx.selected = fxKey(entry);
  renderList();
  setPreviewOpen(true);
  document.getElementById('fx-preview').innerHTML = `
    <div class="fx-prev-head">
      <div class="fx-prev-name">${escHtml(entry.name)}</div>
      <div class="fx-prev-meta">${entry.kind === 'dir' ? 'Folder — Return to open it' : 'Not a regular file'}</div>
      ${fxCanDelete(fxPathOf(entry)) ? `<div class="fx-prev-actions">
        <button class="fx-copy fx-del" data-delete title="Delete it and everything in it (Del)">Delete</button></div>` : ''}
    </div>`;
}

/* ── Preview ──
   Read through the same endpoint as Download, capped so a click on a big log
   does not stream it into a <pre>. HTML is shown from its text in an iframe
   with scripts and no same-origin — the file was written in the sandbox, and a
   page with this tab's origin could call the daemon's API. Images go straight
   into an <img>, which runs nothing. */
const FX_IMAGE = /\.(png|jpe?g|gif|webp|svg|bmp|ico)$/i;
const FX_VIDEO = /\.(mp4|m4v|webm|mov)$/i;
const FX_HTML = /\.html?$/i;
const FX_TEXT_MAX = 1024 * 1024;
const FX_HTML_MAX = 8 * 1024 * 1024;
const FX_IMAGE_MAX = 24 * 1024 * 1024;

/// Nothing selected: the preview column folds away rather than standing empty
/// beside the list — it was a third of the panel holding one grey sentence and
/// a third copy of the path. It comes back with the first selection.
function setPreviewOpen(open) {
  document.querySelector('.fx-body').classList.toggle('no-preview', !open);
}

function fxPreviewEmpty() {
  document.getElementById('fx-preview').innerHTML = '';
  setPreviewOpen(false);
}

async function fxPreview(entry) {
  fx.selected = fxKey(entry);
  renderList();
  setPreviewOpen(true);
  const path = fxPathOf(entry);
  const host = document.getElementById('fx-preview');
  host.innerHTML = `
    <div class="fx-prev-head">
      <div class="fx-prev-name" title="${escHtml(path)}">${escHtml(entry.name)}</div>
      <div class="fx-prev-meta">${humanSize(entry.size)} · ${escHtml(fullTime(entry.modified))}</div>
      <div class="fx-prev-actions">
        <a class="dl-link" href="${fxFileUrl(path, '&download=1')}" download="${escHtml(entry.name)}">⬇ Download</a>
        <button class="fx-copy" data-copy="${escHtml(path)}" title="Copy the path">Copy path</button>
        ${fx.recent ? `<button class="fx-copy" data-reveal="${escHtml(path)}"
          title="Open the folder it was written to">Show in folder</button>` : ''}
        ${fxCanDelete(path) ? '<button class="fx-copy fx-del" data-delete title="Delete it (Del)">Delete</button>' : ''}
      </div>
      ${fx.recent ? `<div class="fx-prev-where" title="${escHtml(path)}">${escHtml(path)}</div>` : ''}
    </div>
    <div class="fx-prev-body" id="fx-prev-body"><p class="cm-empty-small">Loading…</p></div>`;
  const body = document.getElementById('fx-prev-body');
  const stillHere = () => fx.selected === fxKey(entry) && document.getElementById('fx-prev-body') === body;

  if (entry.kind !== 'file') { body.innerHTML = '<p class="cm-empty-small">Not a regular file.</p>'; return; }
  if (FX_IMAGE.test(entry.name) && entry.size <= FX_IMAGE_MAX) {
    body.innerHTML = `<img class="fx-prev-img" alt="${escHtml(entry.name)}" src="${fxFileUrl(path)}">`;
    return;
  }
  // Played by the browser from the same capped endpoint; only the metadata is
  // read until Play.
  if (FX_VIDEO.test(entry.name) && entry.size <= FX_IMAGE_MAX * 4) {
    body.innerHTML = `<video class="fx-prev-img" controls preload="metadata" src="${fxFileUrl(path)}"></video>`;
    return;
  }
  const html = FX_HTML.test(entry.name);
  const max = html ? FX_HTML_MAX : FX_TEXT_MAX;
  if (html && entry.size > max) {
    body.innerHTML = '<p class="cm-empty-small">Too large to preview — download it.</p>';
    return;
  }
  // Text past the cap is previewed from its head: the daemon reads only that.
  const truncated = entry.size > max;
  let text;
  try {
    const res = await fetch(fxFileUrl(path, `&max=${max}${truncated ? '&head=1' : ''}`));
    if (!res.ok) throw new Error(await res.text());
    const head = new Uint8Array(await res.arrayBuffer());
    if (head.subarray(0, 8192).includes(0)) {
      if (stillHere()) body.innerHTML = '<p class="cm-empty-small">Binary file — no preview.</p>';
      return;
    }
    text = new TextDecoder('utf-8', { fatal: false }).decode(head);
  } catch (e) {
    if (stillHere()) body.innerHTML = `<p class="cm-empty-small cm-error">${escHtml(e.message || 'Could not read it')}</p>`;
    return;
  }
  if (!stillHere()) return;
  if (html) {
    body.innerHTML = `<iframe class="fx-prev-frame" title="${escHtml(entry.name)}"
      sandbox="allow-scripts allow-downloads allow-popups"></iframe>`;
    body.querySelector('iframe').srcdoc = text;
    return;
  }
  body.innerHTML = `<pre class="fx-prev-text"></pre>${truncated
    ? `<p class="cm-empty-small">First ${humanSize(max)} of ${humanSize(entry.size)} — download it for the rest.</p>` : ''}`;
  body.querySelector('pre').textContent = text;
}

/// Paint the footer's map button for what this project has, or is getting.
///
/// Four states, because the button answers four different questions:
///
///  - **Code map** — there is one; open it.
///  - **Writing the map…** — a background session is writing one right now
///    (see `applyCodemapRun`), so there is nothing to open and nothing to ask
///    for. Disabled, and the only state that is.
///  - **Writing a lens…** — same session, different document, and the map it is
///    written from is still there to read. So it says what is happening and
///    stays clickable: a lens being written is no reason to lock the map away.
///  - **Generate code map** — there is none. Enabled: a project without a map
///    is the case where the button has the most to do.
///
/// A lens counts as something to open. A project whose map was deleted but
/// whose lenses survive still has documents in that panel, and a button that
/// refuses to open them because the *source* is gone would be hiding them.
function renderCodemapButton() {
  const btn = document.getElementById('files-modal-codemap');
  const { map, lenses } = filesCodemap;
  const n = (lenses || []).length;
  const run = filesTarget ? codemapRun(filesTarget) : null;
  const writing = run?.phase === 'writing';
  const lens = isLensRun(run);
  const readable = !!(map || n);

  btn.disabled = writing && !readable;
  btn.classList.toggle('working', writing);
  document.getElementById('files-modal-codemap-label').textContent =
    writing ? (lens ? 'Writing a lens…' : 'Writing the map…')
      : readable ? 'Code map' : 'Generate code map';

  const lensNote = n ? ` · ${n} lens${n === 1 ? '' : 'es'}` : '';
  btn.title = writing
    ? `${filesTarget} is writing ${lens ? 'a lens' : 'its map'} in a background session, started `
      + `${humanTime(Math.round(run.started / 1000))} — it takes a few minutes`
      + (lens && readable ? ' · the map is readable meanwhile' : '')
    : map
      ? `Open the code map — ${map.files} file${map.files === 1 ? '' : 's'} under `
        + `.sbxw-artifacts/${map.dir}/, updated ${humanTime(map.modified)}${lensNote}`
      : n
        ? `Open ${n} lens${n === 1 ? '' : 'es'} under .sbxw-artifacts/codemap-lenses/ `
          + '— the map they were written from is gone'
        : run?.phase === 'failed'
          ? `No code map yet — the last attempt ended: ${run.note || 'without one'}. `
            + 'Click to try again.'
          : 'No code map yet — write one with this sandbox\'s agent, in the background';
}

/// Ask this sandbox to write its map, and let the daemon paint the rest.
///
/// Nothing is drawn from the response beyond a failure: the run is the daemon's
/// state, it reaches every open tab over the `codemap` event, and a tab that
/// painted itself from its own click would be the one tab showing something the
/// others do not.
async function generateCodemap(name) {
  try {
    const res = await fetch(`/api/sandboxes/${encodeURIComponent(name)}/codemap`, { method: 'POST' });
    const data = await res.json();
    if (!data.ok) { showToast(data.error || 'sbxw could not start that', 'error'); return; }
    showToast(`“${name}” is writing its code map — this takes a few minutes`);
  } catch (_) {
    showToast('Could not reach sbxw', 'error');
  }
}

/// Reload the project panel if it is the one showing `name`.
///
/// What this project *has* changes from outside the panel — a map finishing is
/// files appearing under `.sbxw-artifacts/` with nobody in this tab involved —
/// so the panel re-reads rather than being told what changed.
function refreshFilesModal(name) {
  if (filesTarget !== name || filesOverlay.classList.contains('hidden')) return;
  fetchCodemapState(name);
  if (fx.recent) { fxOpenRecent({ push: false, select: fx.selected }); return; }
  fxOpen(fx.path, { push: false, select: fx.selected });
  fxFetchRecent(name);
}

/// What the code map button needs to know: is there a map, are there lenses,
/// is one being written. Read from the artifacts listing, which folds the map
/// out of `.sbxw-artifacts/` for exactly this.
async function fetchCodemapState(name) {
  try {
    const res = await fetch(`/api/sandboxes/${encodeURIComponent(name)}/artifacts`);
    const data = await res.json();
    if (filesTarget !== name) return;
    filesCodemap = { map: data.codemap || null, lenses: data.lenses || [], run: data.run || null };
    // The panel may be the first thing in this tab to hear about a run — it is
    // opened by name, while the stream only reports changes. Seeded, so a map
    // that finished before this tab existed is not announced as news.
    if (data.run) applyCodemapRun(data.run, true);
    renderCodemapButton();
  } catch (_) { /* the button keeps its last state */ }
}

/// Open onto `.sbxw-artifacts/`, as the panel always has: the first listing
/// (the workspace) tells us where that is, and whether it exists yet.
async function openFilesModal(name) {
  filesTarget = name;
  filesNameEl.textContent = name;
  filesOverlay.classList.remove('hidden');
  Object.assign(fx, { path: '', parent: null, entries: [], history: [], selected: null, recent: null, fresh: null });
  document.getElementById('fx-filter').value = '';
  fxNotice('');
  fetchCodemapState(name);
  fxFetchRecent(name);
  document.getElementById('fx-list-wrap').focus({ preventScroll: true });
  await fxOpen('', { push: false });
  if (filesTarget !== name) return;
  const artifacts = fx.entries.find((e) => e.name === '.sbxw-artifacts' && e.kind === 'dir');
  if (artifacts && fx.places.artifacts) fxOpen(fx.places.artifacts, { push: true });
}

function closeFilesModal() { filesOverlay.classList.add('hidden'); filesTarget = null; fx.seq++; }

document.getElementById('files-modal-codemap').addEventListener('click', () => {
  if (!filesTarget) return;
  const sandbox = filesTarget;
  // Nothing to read yet: ask for one instead, and leave the panel open — the
  // button becomes the run's own status line a moment later.
  if (!filesCodemap.map && !filesCodemap.lenses.length) { generateCodemap(sandbox); return; }
  closeFilesModal();
  openCodemapModal(sandbox);
});

document.getElementById('files-modal-close').addEventListener('click', closeFilesModal);
document.getElementById('files-modal-close2').addEventListener('click', closeFilesModal);
document.getElementById('files-modal-refresh').addEventListener('click', () => {
  if (filesTarget) refreshFilesModal(filesTarget);
});
filesOverlay.addEventListener('click', e => { if (e.target === filesOverlay) closeFilesModal(); });
document.addEventListener('keydown', e => {
  if (filesOverlay.classList.contains('hidden')) return;
  if (e.key === 'Escape') {
    // Escape leaves the path field first, the panel second.
    if (!document.getElementById('fx-path-input').classList.contains('hidden')) { closePathInput(); return; }
    closeFilesModal();
  } else if (e.key === 'Backspace' && e.altKey && fx.parent) {
    fxOpen(fx.parent);
  }
});

/* ── Explorer wiring ── */

document.getElementById('fx-back').addEventListener('click', () => {
  const prev = fx.history.pop();
  if (prev === FX_RECENT) fxOpenRecent({ push: false });
  else if (prev) fxOpen(prev, { push: false });
});
document.getElementById('fx-up').addEventListener('click', () => {
  if (fx.parent) fxOpen(fx.parent, { select: fx.path.split('/').pop() });
});
document.getElementById('fx-star').addEventListener('click', () => fxToggleFavourite(fx.path));
document.getElementById('fx-filter').addEventListener('input', renderList);
document.getElementById('fx-show-hidden').addEventListener('change', renderList);

// Clicking the breadcrumb's empty space turns it into a path field.
function openPathInput() {
  const input = document.getElementById('fx-path-input');
  document.getElementById('fx-crumbs').classList.add('hidden');
  input.classList.remove('hidden');
  input.value = fx.path;
  input.focus();
  input.select();
}
function closePathInput() {
  document.getElementById('fx-path-input').classList.add('hidden');
  document.getElementById('fx-crumbs').classList.remove('hidden');
}
document.getElementById('fx-crumbs').addEventListener('click', (e) => {
  const crumb = e.target.closest('.fx-crumb');
  if (crumb) { if (crumb.dataset.path !== fx.path) fxOpen(crumb.dataset.path); return; }
  openPathInput();
});
document.getElementById('fx-path-input').addEventListener('keydown', (e) => {
  if (e.key === 'Enter') {
    const v = e.target.value.trim();
    closePathInput();
    if (v && v !== fx.path) fxOpen(v.replace(/^~(?=\/|$)/, fx.places.home));
  }
});
document.getElementById('fx-path-input').addEventListener('blur', closePathInput);

document.getElementById('fx-favourites').addEventListener('click', (e) => {
  const rm = e.target.closest('[data-unfav]');
  if (rm) { e.stopPropagation(); fxToggleFavourite(rm.dataset.unfav); return; }
  const fav = e.target.closest('.fx-fav');
  if (fav?.dataset.path === FX_RECENT) { if (!fx.recent) fxOpenRecent(); return; }
  if (fav && (fx.recent || fav.dataset.path !== fx.path)) fxOpen(fav.dataset.path);
});

fxTbody.addEventListener('click', (e) => {
  const row = e.target.closest('.fx-row');
  if (!row) return;
  const entry = fx.entries.find((x) => fxKey(x) === row.dataset.key);
  if (!entry) return;
  // Keep the keyboard where the pointer just was, so ↑/↓ carry on from here.
  document.getElementById('fx-list-wrap').focus({ preventScroll: true });
  if (e.target.closest('.fx-rm')) { fxDelete(entry); return; }
  if (e.target.closest('.fx-dl')) { fxDownload(entry); return; }
  if (entry.kind === 'dir') fxOpen(fxJoin(fx.path, entry.name));
  else fxPreview(entry);
});
fxTbody.addEventListener('dblclick', (e) => {
  const row = e.target.closest('.fx-row');
  const entry = row && fx.entries.find((x) => fxKey(x) === row.dataset.key);
  if (!entry || entry.kind !== 'file' || e.target.closest('.fx-dl')) return;
  fxDownload(entry);
});

// The list as one keyboard widget. Arrows walk the visible rows — previewing
// files as they go, so reading through a folder is a matter of holding ↓ —
// Return acts on the selection, Backspace goes up a level.
document.getElementById('fx-list-wrap').addEventListener('keydown', (e) => {
  if (e.metaKey || e.ctrlKey || e.altKey) return;
  const rows = fxVisible();
  const i = rows.findIndex((r) => fxKey(r) === fx.selected);
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    e.preventDefault();
    if (!rows.length) return;
    const next = i === -1 ? 0
      : Math.max(0, Math.min(rows.length - 1, i + (e.key === 'ArrowDown' ? 1 : -1)));
    if (next !== i) fxShow(rows[next]);
  } else if (e.key === 'Enter' && i !== -1) {
    e.preventDefault();
    const entry = rows[i];
    if (entry.kind === 'dir') fxOpen(fxJoin(fx.path, entry.name));
    else if (entry.kind === 'file') fxDownload(entry);
  } else if (e.key === 'Delete' && i !== -1) {
    e.preventDefault();
    fxDelete(rows[i]);
  } else if (e.key === 'Backspace' && fx.recent && fx.history.length) {
    e.preventDefault();
    document.getElementById('fx-back').click();
  } else if (e.key === 'Backspace' && fx.parent) {
    e.preventDefault();
    fxOpen(fx.parent, { select: fx.path.split('/').pop() });
  }
});

document.getElementById('fx-preview').addEventListener('click', (e) => {
  if (e.target.closest('[data-delete]')) {
    const entry = fx.entries.find((x) => fxKey(x) === fx.selected);
    if (entry) fxDelete(entry);
    return;
  }
  const reveal = e.target.closest('[data-reveal]');
  if (reveal) {
    const p = reveal.dataset.reveal;
    fxOpen(p.slice(0, p.lastIndexOf('/')) || '/', { select: p.slice(p.lastIndexOf('/') + 1) });
    return;
  }
  const btn = e.target.closest('.fx-copy');
  if (!btn) return;
  if (navigator.clipboard) {
    navigator.clipboard.writeText(btn.dataset.copy).then(
      () => showToast('Path copied'),
      () => showToast('Could not copy the path', 'error'));
  } else {
    showToast('Clipboard needs localhost or HTTPS — select the path and copy it', 'error');
  }
});
