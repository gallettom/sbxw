// Escape text for interpolation into an HTML string, attribute values
// included. Loaded first, so every panel that builds markup from values sbxw
// does not itself constrain — filesystem paths, policy rows out of `sbx`, an
// agent's question — can reach for it.
function escHtml(s) {
  return String(s ?? '').replace(/[&<>"']/g, c =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
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

// ── Pane-bar popovers ─────────────────────────────────────────────────────
//
// The cards that hang off a pane's top-bar buttons (SSH, Env). They share more
// than geometry: opening one, closing it on Escape, closing it on a click
// elsewhere, and re-placing it when the layout moves are the same problem every
// time, and the last three are the ones easy to get subtly wrong.
//
// This lives in util.js rather than in whichever feature file happened to need
// it first. When `positionPopover` sat in ssh.js, every later card had to be
// loaded after it — a load-order rule recorded in a comment instead of in the
// dependency.

/// The popover currently open, so opening one closes the other. That used to
/// happen by accident: clicking the second button tripped the first card's own
/// outside-click handler. Accidental exclusion works until two cards are opened
/// from somewhere that isn't a click.
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
