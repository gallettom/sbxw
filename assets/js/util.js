// Escape text for interpolation into an HTML string, attribute values
// included. Loaded first, so every panel that builds markup from values sbxw
// does not itself constrain — filesystem paths, policy rows out of `sbx`, an
// agent's question — can reach for it.
function escHtml(s) {
  return String(s ?? '').replace(/[&<>"']/g, c =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

// The app's global shortcuts (⌥N new sandbox, ⌥M chat). On a Mac, Option
// alone is how characters are typed — ⌥N is the tilde dead key on French and
// US layouts alike, ⌥M is "µ" — so taking it breaks typing `~` in a
// terminal pane. There the shortcut is ⌃⌥ instead, which types nothing.
// Elsewhere Alt alone stays: Ctrl+Alt is AltGr on many Windows layouts.
const IS_MAC = /Mac|iPhone|iPad/.test(navigator.platform);
function isAppShortcut(e, code) {
  if (e.code !== code || !e.altKey || e.metaKey || e.shiftKey) return false;
  return IS_MAC ? e.ctrlKey : !e.ctrlKey;
}

// ── JSON over HTTP ────────────────────────────────────────────────────────
// Every mutating endpoint takes a JSON body and answers `{ok, …}`. `postJson`
// rejects the way `fetch` does, for callers with their own catch;
// `postJsonOrError` folds an unreachable daemon into the same `{ok: false,
// error}` shape the daemon itself answers with.
async function postJson(url, body) {
  const res = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body ?? {}),
  });
  return res.json();
}

async function postJsonOrError(url, body) {
  try {
    return await postJson(url, body);
  } catch (e) {
    return { ok: false, error: e.message };
  }
}

// Put `msg` in an inline note (an error line under a form) and show it, or
// hide the note when there is nothing to say.
function showNote(el, msg) {
  el.textContent = msg;
  el.classList.toggle('hidden', !msg);
}

// ── Git repos of a workspace ──────────────────────────────────────────────
// A thread works in one git repo of its sandbox's workspace: the workspace
// itself (path '') or a repo one level down. The New thread dialog and the
// plan dialog both list them, so the reading lives here once.

// `{ok, workspace, repos}` from the daemon, or `{ok: false, error}` — an
// unreachable daemon folded in, as postJsonOrError does.
async function fetchRepos(sandbox) {
  try {
    const r = await (await fetch(`/api/sandboxes/${encodeURIComponent(sandbox)}/repos`)).json();
    return r && r.ok ? { ok: true, workspace: r.workspace || '', repos: r.repos || [] }
      : { ok: false, error: r?.error || 'no answer' };
  } catch (e) {
    return { ok: false, error: e.message };
  }
}

// A repo by the name a person knows it by: its folder, or the workspace's
// own folder for the workspace itself.
function repoLabel(path, workspace) {
  if (path) return path;
  return (workspace || '').split('/').filter(Boolean).pop() || 'workspace';
}

// What a repo's checkout looks like, in a few words and a tone: 'clean' is
// green, anything a person should know before working there is amber.
function repoStatus(r) {
  const parts = [];
  if (r.detached) parts.push('detached');
  parts.push(r.dirty ? `${r.dirty} uncommitted` : 'clean');
  if (r.ahead || r.behind) parts.push(`↑${r.ahead || 0} ↓${r.behind || 0}`);
  const tone = r.dirty || r.detached ? 'warn' : 'ok';
  return { text: parts.join('  '), tone };
}

// ── Models ────────────────────────────────────────────────────────────────
// The Claude models a thread can run, from /api/models: `{ok, default,
// models: [{id, label}]}`, or `{ok: false}` when the daemon has none to offer —
// then no model control is shown and threads get the daemon's default. Asked
// once per tab: the list cannot change while the daemon runs.
let modelsAsk = null;
function fetchModels() {
  modelsAsk ??= fetch('/api/models')
    .then(r => r.json())
    .then(r => r && r.ok && Array.isArray(r.models) && r.models.length
      ? { ok: true, default: r.default, models: r.models } : { ok: false })
    .catch(() => ({ ok: false }));
  return modelsAsk;
}

// A model by its label, or by its id when the list does not know it.
function modelLabel(id, models) {
  return models?.ok ? (models.models.find(m => m.id === id)?.label ?? id) : id;
}

// The repo a thread gets without being asked: the only one there is, or the
// workspace itself when it is one. Undefined when a person has to choose.
function defaultRepo(repos) {
  if (repos.length === 1) return repos[0].path;
  return repos.some(r => r.path === '') ? '' : undefined;
}

// "3m ago" for an RFC 3339 time from sbx; '' when it does not parse, so a
// format sbx changes shows nothing rather than "NaN ago".
function timeAgo(iso) {
  const t = Date.parse(iso || '');
  if (!Number.isFinite(t)) return '';
  const s = Math.max(0, Math.round((Date.now() - t) / 1000));
  if (s < 60) return 'just now';
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

// ── Toast ─────────────────────────────────────────────────────────────────
let pasteToastTimer = null;
function showToast(msg, kind) {
  const t = document.getElementById('paste-toast');
  t.textContent = msg;
  t.className = 'paste-toast show' + (kind ? ' ' + kind : '');
  clearTimeout(pasteToastTimer);
  if (kind !== 'progress')
    pasteToastTimer = setTimeout(() => { t.className = 'paste-toast'; }, 3000);
}

// ── Background-job corner indicators ─────────────────────────────────────
// Long-running operations (sandbox creation, port publishing) run in the
// background instead of blocking the UI behind a modal overlay. Each one
// gets a small pixel-logo card stacked in the bottom-right corner — "SBXW"
// rendered as a 5×7 dot-matrix glyph grid (same blue/green pair as the
// header badge), with a fill level sweeping up through it and back down on
// a loop for as long as the job is running.
const PIXEL_FONT = {
  S: ['01111','10000','10000','01110','00001','00001','11110'],
  B: ['11110','10001','10001','11110','10001','10001','11110'],
  X: ['10001','10001','01010','00100','01010','10001','10001'],
  W: ['10001','10001','10001','10101','10101','11011','10001'],
};
const PIXEL_LETTER_COLORS = { S: '#58a6ff', B: '#58a6ff', X: '#3fb950', W: '#3fb950' };

function buildPixelGrid(container) {
  const letters = ['S', 'B', 'X', 'W'];
  const rows = 7, letterW = 5, gap = 1;
  const cols = letters.length * (letterW + gap) - gap;
  container.style.setProperty('--cols', cols);
  container.style.setProperty('--rows', rows);

  const pixels = [];
  letters.forEach((letter, li) => {
    const pattern = PIXEL_FONT[letter];
    for (let r = 0; r < rows; r++) {
      for (let c = 0; c < letterW; c++) {
        if (pattern[r][c] !== '1') continue;
        const el = document.createElement('div');
        el.className = 'px';
        el.style.gridColumn = li * (letterW + gap) + c + 1;
        el.style.gridRow = r + 1;
        el.style.setProperty('--c', PIXEL_LETTER_COLORS[letter]);
        container.appendChild(el);
        pixels.push({ el, row: r });
      }
    }
  });
  return { pixels, rows };
}

function animatePixelGrid(pixels, rows) {
  let level = 0, dir = 1, dwell = 0;
  return setInterval(() => {
    const litFromRow = rows - level;
    pixels.forEach(p => p.el.classList.toggle('lit', p.row >= litFromRow));
    if (dwell > 0) { dwell--; return; }
    level += dir;
    if (level >= rows) { level = rows; dir = -1; dwell = 3; }
    else if (level <= 0) { level = 0; dir = 1; dwell = 3; }
  }, 110);
}

const bgJobsEl = document.getElementById('bg-jobs');
const bgJobs = new Map();
let bgJobSeq = 0;

/**
 * Opens an empty corner card and hands back its body to fill.
 *
 * Most background work has one line to say and uses `startBgJob` below. Work
 * that has more — a bring-up ticking off the steps it announced — writes into
 * `body` instead, so it still arrives in the same corner, in the same card,
 * with the same logo and the same entry and exit. A long job looks like a long
 * job whatever it happens to be doing.
 *
 * Returns `{ id, card, body }`; `id` is what `finishBgJob` takes.
 */
function openBgJob(className = '') {
  const id = ++bgJobSeq;
  const card = document.createElement('div');
  card.className = 'bg-job-card' + (className ? ' ' + className : '');
  const logo = document.createElement('div');
  logo.className = 'bg-job-pixel-logo';
  const body = document.createElement('div');
  body.className = 'bg-job-body';
  card.append(logo, body);
  bgJobsEl.appendChild(card);

  const { pixels, rows } = buildPixelGrid(logo);
  const timer = animatePixelGrid(pixels, rows);
  bgJobs.set(id, { el: card, timer });
  return { id, card, body };
}

/** Adds a corner card for a long-running background operation; returns its id. */
function startBgJob(label) {
  const { id, body } = openBgJob();
  const text = document.createElement('div');
  text.className = 'bg-job-label';
  text.textContent = label;
  body.appendChild(text);
  return id;
}

/** Removes the corner card for `id`, if it's still active. */
function finishBgJob(id) {
  const job = bgJobs.get(id);
  if (!job) return;
  clearInterval(job.timer);
  job.el.classList.add('leaving');
  setTimeout(() => job.el.remove(), 200);
  bgJobs.delete(id);
}

// ── Copy-to-clipboard ─────────────────────────────────────────────────────

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

/// Copy `text` with no UI of its own — for the copies nobody pressed a button
/// for (selecting text in a terminal). Resolves to whether the clipboard took
/// it, so the caller decides whether a failure is worth saying out loud.
///
/// Falls back to `execCommand` where `navigator.clipboard` is missing or
/// refuses: that is the page reached by LAN IP rather than by localhost, and a
/// selection-copy that silently stops working there would look like the bug
/// this exists to fix.
function copyQuiet(text) {
  if (!navigator.clipboard) return Promise.resolve(copyViaExecCommand(text));
  return navigator.clipboard.writeText(text).then(() => true, () => copyViaExecCommand(text));
}

/// The pre-Clipboard-API path: `execCommand('copy')` copies the *document*
/// selection, so it needs a real element holding the text and selected in it.
/// Off-screen rather than hidden — `display:none` can't hold a selection — and
/// focus goes back where it was, or the terminal loses the keyboard to a
/// textarea that no longer exists.
function copyViaExecCommand(text) {
  const ta = document.createElement('textarea');
  ta.value = text;
  ta.setAttribute('aria-hidden', 'true');
  ta.style.cssText = 'position:fixed;top:-1000px;left:-1000px;opacity:0';
  document.body.appendChild(ta);
  const focused = document.activeElement;
  let ok = false;
  try { ta.select(); ok = document.execCommand('copy'); } catch (_) {}
  ta.remove();
  try { focused?.focus?.(); } catch (_) {}
  return ok;
}

// ── Pane-bar popovers ─────────────────────────────────────────────────────
//
// The cards that hang off a pane's top-bar buttons (SSH, Env). They share more
// than geometry: opening one, closing it on Escape, closing it on a click
// elsewhere, and re-placing it when the layout moves are the same problem every
// time, and the last three are the ones easy to get subtly wrong.

/// The popover currently open, so opening one closes the other — explicitly,
/// not as a side effect of the outside-click handler, which only fires on a
/// click.
let openPopover = null;

/// Place a popover under its button, flipping above when the viewport floor is
/// closer than the popover is tall, and clamping to the window either way.
/// Measured after the content is built and while `visibility: hidden`, since a
/// `display: none` element has no size to measure.
///
/// Returns false when the anchor has left the document — a layout change
/// rebuilds panes, which can take the button out from under a card that is
/// still up.
function positionPopover(pop, anchor) {
  if (!anchor.isConnected) return false;

  pop.style.visibility = 'hidden';
  pop.classList.remove('hidden');
  const a = anchor.getBoundingClientRect();
  const p = pop.getBoundingClientRect();
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

  pop.style.top = `${Math.round(top)}px`;
  pop.style.left = `${Math.round(left)}px`;
  pop.style.visibility = '';
  return true;
}

/// Wire up one pane-bar card. `onOpen(payload)` fills its content — it runs
/// *before* the first placement, so the card is measured at its real size — and
/// `onClose()` drops whatever state that content needed.
///
/// Returns `{ open, close, toggle, reposition, isOpen }`. The caller keeps only
/// its own content logic; the anchor, the three listeners and mutual exclusion
/// live here, once.
function bindPopover(pop, { closeButton, onOpen, onClose } = {}) {
  /// The button the card hangs off, or null when closed. Doubles as the
  /// "ignore this click" test for the outside-click handler.
  let anchor = null;

  const api = {
    isOpen: () => anchor !== null,

    open(anchorEl, payload) {
      // One card at a time, explicitly.
      if (openPopover && openPopover !== api) openPopover.close();
      anchor = anchorEl;
      openPopover = api;
      if (onOpen) onOpen(payload);
      api.reposition();
    },

    close() {
      if (!anchor) return;
      pop.classList.add('hidden');
      anchor = null;
      if (openPopover === api) openPopover = null;
      if (onClose) onClose();
    },

    toggle(anchorEl, payload) {
      if (anchor === anchorEl && !pop.classList.contains('hidden')) api.close();
      else api.open(anchorEl, payload);
    },

    /// Re-place the card against its anchor. Callers whose content changes size
    /// after opening (a fetch that fills a textarea) call this again.
    reposition() {
      if (anchor && !positionPopover(pop, anchor)) api.close();
    },
  };

  if (closeButton) {
    const btn = typeof closeButton === 'string'
      ? document.getElementById(closeButton) : closeButton;
    if (btn) btn.addEventListener('click', api.close);
  }
  // `pointerdown` on the capture phase so it still fires over the terminal,
  // which swallows its own mouse events; the anchor is excluded because its own
  // handler already toggles.
  document.addEventListener('pointerdown', e => {
    if (!anchor) return;
    if (pop.contains(e.target) || anchor.contains(e.target)) return;
    api.close();
  }, true);
  document.addEventListener('keydown', e => {
    if (e.key === 'Escape' && anchor) api.close();
  });
  // A card pinned to fixed coordinates goes stale the moment the layout moves.
  window.addEventListener('resize', api.reposition);

  return api;
}
