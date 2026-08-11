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

  // Disconnect the closing pane.
  const closing = panes[idx];
  if (closing.ws) { try { closing.ws.close(); } catch (_) {} closing.ws = null; }
  closing.sandbox = null;
  closing.term.clear();
  document.getElementById(`plabel-${idx}`).textContent = '—';
  document.getElementById(`pconn-${idx}`).textContent = '';
  document.getElementById(`pdot-${idx}`).className = 'dot term-disconnected';
  document.getElementById(`pssh-${idx}`).disabled = true;
  // A closed monitor pane must not leave its hidden mode buttons behind for
  // whatever gets connected here next.
  closing.el.querySelectorAll('.mode-btn').forEach(b => { b.hidden = false; });

  // Shift panes[idx+1..paneCount-1] one slot to the left.
  // After each step the source slot is nulled so it can safely be overwritten
  // in the next step without closing a still-live connection.
  for (let k = idx; k < paneCount - 1; k++) {
    const src = panes[k + 1];
    const dst = panes[k];

    dst.sandbox = src.sandbox;
    dst.mode    = src.mode;
    dst.ws      = src.ws;

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
    document.getElementById(`plabel-${dst.index}`).textContent = src.sandbox || '—';
    document.getElementById(`pdot-${dst.index}`).className = 'dot ' +
      (dst.ws?.readyState === WebSocket.OPEN ? 'term-connected' : 'term-disconnected');
    document.getElementById(`pconn-${dst.index}`).textContent =
      dst.ws?.readyState === WebSocket.OPEN ? 'connected' : (src.sandbox ? 'disconnected' : '');
    document.getElementById(`pssh-${dst.index}`).disabled = !src.sandbox;
    setPaneMode(dst.index, src.mode, false);
    // Full reset, not just clear(): dst now displays a different live session,
    // and any mouse-tracking mode left on from dst's previous content must not
    // leak into it. PTY resize will trigger a redraw.
    dst.term.reset();

    // Null out the source so it becomes a clean empty slot.
    src.sandbox = null;
    src.ws      = null;
    src.term.clear();
    document.getElementById(`plabel-${src.index}`).textContent = '—';
    document.getElementById(`pconn-${src.index}`).textContent = '';
    document.getElementById(`pdot-${src.index}`).className = 'dot term-disconnected';
    document.getElementById(`pssh-${src.index}`).disabled = true;
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
  if (!dragSel || !(e.buttons & 1)) { dragSel = null; return; }
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
  try { dragSel.pane.term.select(a.col, a.row, Math.max(1, (b.row - a.row) * cols + (b.col - a.col))); }
  catch (_) { dragSel = null; }
}, true);

document.addEventListener('mouseup', e => {
  if (!dragSel?.dragging) { dragSel = null; return; }
  dragSel = null;
}, true);

// ── Address badge (copy URL) ──────────────────────────────────────────────
const addrBadge = document.getElementById('addr-badge');
addrBadge.textContent = location.host;
addrBadge.addEventListener('click', () => {
  navigator.clipboard?.writeText(location.href).then(() => {
    addrBadge.textContent = 'copied!';
    addrBadge.classList.add('copied');
    setTimeout(() => {
      addrBadge.textContent = location.host;
      addrBadge.classList.remove('copied');
    }, 1500);
  });
});
