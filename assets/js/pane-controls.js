// ── Layout persistence ────────────────────────────────────────────────────
const LAYOUT_KEY = `sbxw:layout:${location.host}`;

function saveLayout() {
  try {
    localStorage.setItem(LAYOUT_KEY, JSON.stringify({
      n: paneCount,
      panes: panes.slice(0, paneCount).map(p => ({ sandbox: p.sandbox || null, mode: p.mode })),
    }));
  } catch (_) {}
}

function loadLayout() {
  try {
    const s = JSON.parse(localStorage.getItem(LAYOUT_KEY) || 'null');
    if (s?.n >= 1 && s.n <= MAX_PANES_ABS && Array.isArray(s.panes)) return s;
  } catch (_) {}
  return null;
}

function setLayout(n) {
  paneCount = n;

  while (panes.length < n) panes.push(createPane(panes.length));
  panes.forEach((p, i) => { p.el.style.display = i < n ? '' : 'none'; });
  applyGridTemplate(n);

  // No refit here: the grid template just changed every pane's box, which is
  // what `observePaneSize` watches for.
  setFocusedPane(focusedPane < n ? focusedPane : 0);

  document.querySelectorAll('#layout-switch-panel button').forEach(btn => {
    btn.classList.toggle('active', parseInt(btn.dataset.n) === n);
  });
  updateLayoutSwitchLabel();
  updateCloseButtons();
  saveLayout();
}

// ── Close a pane ─────────────────────────────────────────────────────────
function updateCloseButtons() {
  panes.forEach((p, i) => {
    const btn = document.getElementById(`pclose-${i}`);
    if (btn) btn.style.display = paneCount > 1 ? '' : 'none';
  });
}

// Close pane at `idx`, shifting subsequent panes left to fill the gap.
// The WebSocket of each shifted pane is rewired to write to its new term element.
function closePane(idx) {
  if (paneCount <= 1) return;
  if (dragSel?.pane?.index === idx) dragSel = null;

  // Disconnect the closing pane. `emptyPane` is also what puts a closed
  // monitor pane's hidden mode buttons back, so they don't stay hidden for
  // whatever gets connected here next.
  emptyPane(idx);

  // Shift panes[idx+1..paneCount-1] one slot to the left.
  // After each step the source slot is nulled so it can safely be overwritten
  // in the next step without closing a still-live connection.
  for (let k = idx; k < paneCount - 1; k++) {
    const src = panes[k + 1];
    const dst = panes[k];

    dst.sandbox = src.sandbox;
    dst.mode    = src.mode;
    dst.ws      = src.ws;
    // Travels with the session: it says where *this* session's pane goes back
    // to when the monitor button is pressed again, not where dst used to be.
    dst.beforeMonitor = src.beforeMonitor;

    if (dst.ws) {
      // Rewire incoming data to the destination terminal element.
      dst.ws.onmessage = ev => handlePaneData(dst, ev);
      dst.ws.onclose = () => {
        document.getElementById(`pdot-${dst.index}`).className = 'dot term-disconnected';
        document.getElementById(`pconn-${dst.index}`).textContent = 'disconnected';
        loadSandboxes();
      };
      dst.ws.onerror = () => {
        document.getElementById(`pconn-${dst.index}`).textContent = 'error';
      };
    }

    // Update the destination pane bar.
    document.getElementById(`pdot-${dst.index}`).className = 'dot ' +
      (dst.ws?.readyState === WebSocket.OPEN ? 'term-connected' : 'term-disconnected');
    document.getElementById(`pconn-${dst.index}`).textContent =
      dst.ws?.readyState === WebSocket.OPEN ? 'connected' : (src.sandbox ? 'disconnected' : '');
    applyPaneChrome(dst.index, src.sandbox);
    setPaneMode(dst.index, src.mode, false);
    // Full reset, not just clear(): dst now displays a different live session,
    // and any mouse-tracking mode left on from dst's previous content must not
    // leak into it. PTY resize will trigger a redraw.
    dst.term.reset();

    // Null out the source so it becomes a clean empty slot. The socket is
    // dropped *before* emptying it — it now belongs to `dst`, and `emptyPane`
    // would otherwise close the session that just moved.
    src.ws = null;
    emptyPane(src.index);
  }

  // Hide the now-empty last slot and decrement the count.
  panes[paneCount - 1].el.style.display = 'none';
  paneCount--;

  applyGridTemplate(paneCount);
  document.querySelectorAll('#layout-switch-panel button').forEach(btn => {
    btn.classList.toggle('active', parseInt(btn.dataset.n) === paneCount);
  });
  updateLayoutSwitchLabel();
  updateCloseButtons();

  if (focusedPane >= paneCount) setFocusedPane(paneCount - 1);

  // The one refit the observer cannot infer: the loop above moved *sockets*
  // between panes, and on an uneven grid a socket can land on a pane of a
  // different cell count without any box changing. Each moved socket carries the
  // size it was told, so this reconciles exactly those and stays silent for the
  // rest — no delay needed, the grid template above is already applied.
  refitPanes('pane closed');

  renderSidebar();
  saveLayout();
}

// ── Custom drag selection ─────────────────────────────────────────────────
// Intercepts mouse drag inside terminal areas so text can be selected even
// when the process has mouse-tracking mode active (which normally steals drags).
let dragSel = null;

function paneFromEl(el) {
  return panes.find(p => p.termEl && p.termEl.contains(el)) || null;
}

document.addEventListener('mousedown', e => {
  if (e.button !== 0) return;
  const pane = paneFromEl(e.target);
  if (!pane || pane.el.style.display === 'none') return;
  // xterm-viewport carries the native scrollbar — let it scroll freely.
  if (e.target.classList.contains('xterm-viewport')) return;
  const start = mouseToCell(e, pane);
  if (!start) return;
  dragSel = { pane, start, dragging: false };
}, true);

document.addEventListener('mousemove', e => {
  if (!dragSel || !(e.buttons & 1)) { dragSel = null; holdSelectionAgainstMotion(e); return; }
  const cur = mouseToCell(e, dragSel.pane);
  if (!cur) { dragSel = null; return; }
  if (!dragSel.dragging) {
    if (Math.abs(cur.col - dragSel.start.col) < 1 &&
        Math.abs(cur.row - dragSel.start.row) < 1) return;
    dragSel.dragging = true;
  }
  e.stopImmediatePropagation(); e.preventDefault();
  let [a, b] = [dragSel.start, cur];
  if (b.row < a.row || (b.row === a.row && b.col < a.col)) [a, b] = [b, a];
  const cols = dragSel.pane.term.cols || 1;
  const len = Math.max(1, (b.row - a.row) * cols + (b.col - a.col));
  // Kept for the restore below. Buffer coordinates, not viewport ones (see
  // `mouseToCell`), so they stay valid even if the pane scrolls afterwards.
  try { dragSel.pane.term.select(a.col, a.row, len); dragSel.range = { col: a.col, row: a.row, len }; }
  catch (_) { dragSel = null; }
}, true);

document.addEventListener('mouseup', e => {
  // A drag that ends outside the pane it started in still belongs to that
  // pane, so the pane comes from `dragSel` first and from the element under
  // the cursor only for a click that never became a drag.
  //
  // The scrollbar is excluded the same way it is on mousedown: dragging it
  // leaves whatever was selected selected, and re-copying an old selection
  // the user has moved on from is worse than copying nothing.
  const onScrollbar = e.target.classList?.contains('xterm-viewport');
  const pane = dragSel?.pane || (onScrollbar ? null : paneFromEl(e.target));
  const range = dragSel?.range;
  dragSel = null;
  if (e.button !== 0 || !pane) return;
  // Copy first: this handler is on the capture phase, so the selection is
  // still there — by the time xterm is done with this same event it may not be.
  copySelectionOnRelease(pane);
  if (range) keepSelectionVisible(pane, range);
}, true);

/// Put the drag's selection back after the release wipes it.
///
/// While a TUI holds mouse tracking, xterm reports the mouse to the program as
/// *user input* (`CoreMouseService` → `triggerDataEvent(report, true)`), and
/// its selection service clears the selection on any user input — the same
/// rule that makes a keystroke drop the highlight, which is right for a
/// keystroke and wrong for the mouse-up that just finished drawing it. So the
/// text stayed selected for exactly as long as the button was held, which read
/// as "selecting with the mouse doesn't work" even though the copy landed.
///
/// The release still reaches the program — swallowing it would leave the TUI
/// believing the button is down — and the selection goes back on top of it,
/// before the frame it would have been missing from.
function keepSelectionVisible(pane, range) {
  requestAnimationFrame(() => {
    // Nothing cleared it (a pane with no mouse tracking, say): leave it alone
    // rather than re-firing a selection change for the same range.
    if (!pane.term || pane.term.hasSelection()) return;
    try { pane.term.select(range.col, range.row, range.len); } catch (_) {}
  });
}

/// Keep the pointer's idle motion away from a program that is tracking it,
/// for as long as a selection is on screen.
///
/// An agent's TUI asks for any-event tracking (`?1003h`), so every move over
/// the pane is reported to it — and xterm hands the program's mouse reports to
/// `triggerDataEvent(report, true)`, i.e. as *user input*, which its selection
/// service answers by clearing the selection. The selected text therefore
/// survived only until the pointer twitched over the terminal: move the mouse
/// away and it stayed, leave it there and it vanished.
///
/// Motion only, and only over a pane that has something selected. Buttons and
/// keys still reach the program untouched, and the first of either drops the
/// selection and hands the pointer straight back — so a TUI's hover states are
/// frozen exactly while the user is reading a selection, and never after it.
function holdSelectionAgainstMotion(e) {
  if (e.target.classList?.contains('xterm-viewport')) return;
  const pane = paneFromEl(e.target);
  if (!pane || pane.el.style.display === 'none') return;
  // No preventDefault: the browser's own cursor and hover work is not the
  // program's, and stopping it would only make the pane feel dead.
  if (pane.term?.hasSelection()) e.stopImmediatePropagation();
}

// ── Copy on select ───────────────────────────────────────────────────────
// A selection in a terminal is only ever made in order to paste it somewhere,
// and a pane has no Copy button to press — xterm just leaves the selection lit
// up waiting for a ⌘C nobody types after double-clicking a word. So every
// selection the mouse makes (double-click word, triple-click line, or a drag
// through the custom path above) goes to the clipboard the moment the button
// comes up. A click that selects nothing copies nothing: it must not wipe the
// clipboard of whatever the user was carrying.
//
// This is the copy for text the *browser* selected. While a TUI holds mouse
// tracking, xterm makes no selection at all and the double-click belongs to
// the program in the PTY — there the copy arrives as OSC 52 instead, handled
// in `setupTerminal`.
let copySelectionFailed = false;

function copySelectionOnRelease(pane) {
  // Read and write synchronously, inside the mouseup. Both selection paths are
  // settled by now — xterm selects a word on the second *mousedown* of a
  // double-click, and a drag updates on every mousemove — and deferring the
  // clipboard write out of the event, even by `setTimeout(…, 0)`, leaves the
  // user gesture behind, which Safari will not write without.
  const text = pane.term?.getSelection?.();
  if (!text) return;
  copyQuiet(text).then(ok => {
    // Said once per tab: the failure is a property of the address the page was
    // opened on, so it will fail on every selection after this one too.
    if (ok || copySelectionFailed) return;
    copySelectionFailed = true;
    showToast('Copy-on-select needs localhost or HTTPS — press ⌘C instead', 'error');
  });
}
