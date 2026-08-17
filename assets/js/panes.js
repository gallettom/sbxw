// ── Multi-pane state ──────────────────────────────────────────────────────
let panes = [];
let focusedPane = 0;
let paneCount = 1;

// Agent activity per session, painted on each pane's liseret. Declared up here
// with the rest of the pane state, and not beside the `/api/stream` subscriber
// that feeds it at the bottom of the script: `connectPane` and `setPaneMode`
// read these, and a `const` is in its temporal dead zone until the top-level
// body reaches it. See the "Agent activity" section for what fills them.
const agentStates = new Map();  // "<sandbox>::<mode>" -> "working"|"idle"|"attention"
const doneFlashes = new Map();  // sandbox -> { at: epoch ms, timer } for the green flash
const DONE_FLASH_MS = 4500;     // keep in step with the liseret-land durations

// ── Grid layout (dynamic pane count) ────────────────────────────────────
// Four panes are always offered whatever the window size; beyond that a pane
// is offered only if it stays genuinely usable. MIN_PANE_W x MIN_PANE_H is
// sized for a comfortably readable terminal rather than a technically visible
// one, and MAX_PANES_ABS stops the picker proposing a 16-up grid.
const MIN_PANE_W = 480;
const MIN_PANE_H = 320;
const BASELINE_PANES = 4;
const MAX_PANES_ABS = 12;

// Near-square arrangement: e.g. n=5 -> 3 cols x 2 rows.
function gridDimsFor(n) {
  const cols = Math.ceil(Math.sqrt(n));
  const rows = Math.ceil(n / cols);
  return { cols, rows };
}

function computeMaxPanes() {
  const mainEl = document.getElementById('main-area');
  const w = mainEl.clientWidth || window.innerWidth;
  const h = mainEl.clientHeight || window.innerHeight;
  let max = BASELINE_PANES;
  for (let n = BASELINE_PANES + 1; n <= MAX_PANES_ABS; n++) {
    const { cols, rows } = gridDimsFor(n);
    if (w / cols < MIN_PANE_W || h / rows < MIN_PANE_H) break;
    max = n;
  }
  return max;
}

// Applies the grid template for n panes, distributing an incomplete last
// row's columns as evenly as possible across its panes (e.g. 5 panes on a
// 3-column grid gives a full top row and a 2-pane bottom row split 2/1).
function applyGridTemplate(n) {
  const mainEl = document.getElementById('main-area');
  const { cols, rows } = gridDimsFor(n);
  mainEl.className = 'main';
  mainEl.style.gridTemplateColumns = `repeat(${cols}, 1fr)`;
  mainEl.style.gridTemplateRows = `repeat(${rows}, 1fr)`;

  panes.forEach(p => { p.el.style.gridColumn = ''; });

  const fullRows = Math.floor(n / cols);
  const remainder = n - fullRows * cols;
  if (remainder > 0) {
    const startIdx = fullRows * cols;
    let colCursor = 1;
    for (let i = startIdx; i < n; i++) {
      const remainingItems = n - i;
      const remainingCols = cols - (colCursor - 1);
      const span = Math.max(1, Math.round(remainingCols / remainingItems));
      panes[i].el.style.gridColumn = `${colCursor} / span ${span}`;
      colCursor += span;
    }
  }
}

function updateLayoutSwitchLabel() {
  const label = document.getElementById('layout-switch-label');
  if (label) label.textContent = String(paneCount);
}

function closeLayoutSwitchPanel() {
  const panel = document.getElementById('layout-switch-panel');
  const trigger = document.getElementById('layout-switch-btn');
  panel.hidden = true;
  trigger.classList.remove('open');
  trigger.setAttribute('aria-expanded', 'false');
}

function renderLayoutSwitch(max) {
  const panel = document.getElementById('layout-switch-panel');
  panel.innerHTML = '';
  for (let i = 1; i <= max; i++) {
    const btn = document.createElement('button');
    btn.textContent = String(i);
    btn.dataset.n = String(i);
    if (i === paneCount) btn.classList.add('active');
    btn.addEventListener('click', () => {
      setLayout(i);
      closeLayoutSwitchPanel();
    });
    panel.appendChild(btn);
  }
  updateLayoutSwitchLabel();
}

// Recompute how many panes fit, refresh the picker, and shrink the current
// layout if the window no longer has room for it.
function updateLayoutSwitch() {
  const max = computeMaxPanes();
  renderLayoutSwitch(max);
  if (paneCount > max) setLayout(max);
}

function mouseToCell(e, pane) {
  const el = pane.termEl;
  if (!el) return null;
  const cols = pane.term.cols || 80;
  const rows = pane.term.rows || 24;
  // Measure `.xterm-screen`, not the pane container. The fit addon rounds rows
  // down, so the container keeps up to a row of slack at the bottom: dividing
  // its height gives too tall a cell and the selection drifts above the pointer,
  // worsening down the pane. `.xterm-screen` is exactly cols×rows cells, and its
  // rect also excludes the scrollbar the container counts as columns.
  const screen = el.querySelector('.xterm-screen');
  const r    = (screen || el).getBoundingClientRect();
  const cw   = r.width  / cols;
  const ch   = r.height / rows;
  if (!cw || !ch) return null;
  const viewportY = pane.term.buffer?.active?.viewportY ?? 0;
  return {
    col: Math.max(0, Math.min(cols - 1, Math.floor((e.clientX - r.left) / cw))),
    row: Math.max(0, Math.min(rows - 1, Math.floor((e.clientY - r.top)  / ch))) + viewportY,
  };
}

function createPane(index) {
  const el = document.createElement('div');
  el.className = 'pane';
  el.id = `pane-${index}`;
  el.innerHTML = `
    <div class="pane-bar">
      <span class="dot term-disconnected" id="pdot-${index}" title="Terminal connection"></span>
      <span class="sandbox-label" id="plabel-${index}">—</span>
      <span class="conn-label" id="pconn-${index}"></span>
      <span class="spacer"></span>
      <div class="mode-switch" role="tablist">
        <button class="mode-btn active" data-mode="claude">✦ Claude</button>
        <button class="mode-btn" data-mode="bash">❯ Bash</button>
      </div>
      <button class="pane-btn" id="pssh-${index}" title="SSH details for this sandbox — fields for a client, and the shell command (run 'sbxw ssh --setup' once first)" disabled>SSH</button>
      <button class="pane-btn" id="preconnect-${index}">Reconnect</button>
      <button class="pane-btn" id="prefresh-${index}" title="Rebuild this pane's terminal from scratch — fixes a broken layout that Reconnect alone can't, by destroying and recreating the terminal widget (then reconnecting)">↻</button>
      <button class="pane-close-btn" id="pclose-${index}" title="Close pane" style="display:none">✕</button>
    </div>
    <div class="pane-term" id="pterm-${index}"></div>`;
  document.getElementById('main-area').appendChild(el);

  const termEl = document.getElementById(`pterm-${index}`);

  const pane = {
    index, el, termEl,
    ws: null, sandbox: null, mode: 'claude',
    // Sandbox this pane was on before the monitor took it over, so the
    // Monitor button toggles.
    beforeMonitor: null,
    lastSelection: '',
    getLastSelection: () => pane.lastSelection,
  };

  // Builds pane.term/pane.fit and wires them up. Split out of createPane so
  // recreatePaneTerminal() can call it again on a pane that already exists,
  // to throw away a terminal whose internal state (not just its content) has
  // gone bad — something reconnecting the socket into the same xterm
  // instance can never fix.
  setupTerminal(pane);

  // Focus pane on click inside it
  el.addEventListener('mousedown', () => setFocusedPane(index));

  // Mode buttons
  const [claudeBtn, bashBtn] = el.querySelectorAll('.mode-btn');
  claudeBtn.addEventListener('click', () => {
    if (pane.mode !== 'claude' && pane.sandbox) connectPane(index, pane.sandbox, 'claude');
    else setPaneMode(index, 'claude');
  });
  bashBtn.addEventListener('click', () => {
    if (pane.mode !== 'bash' && pane.sandbox) connectPane(index, pane.sandbox, 'bash');
    else setPaneMode(index, 'bash');
  });

  document.getElementById(`preconnect-${index}`).addEventListener('click', () => {
    if (pane.sandbox) connectPane(index, pane.sandbox, pane.mode);
  });
  document.getElementById(`prefresh-${index}`).addEventListener('click', () => recreatePaneTerminal(index));
  document.getElementById(`pclose-${index}`).addEventListener('click', () => closePane(index));
  document.getElementById(`pssh-${index}`).addEventListener('click', ev => {
    if (pane.sandbox) toggleSshPop(pane.sandbox, ev.currentTarget);
  });

  return pane;
}

// Builds a fresh xterm Terminal (+ addons) into `pane.termEl` and points
// pane.term/pane.fit at it. Called once from createPane, and again from
// recreatePaneTerminal() on an existing pane — so every wire-up here must
// read `pane.*` rather than close over locals that a second call would
// shadow instead of replace.
function setupTerminal(pane) {
  const term = new Terminal({
    cursorBlink: true, fontSize: 13,
    fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace',
    theme: { background: '#000000', foreground: '#c9d1d9', cursor: '#58a6ff' },
    scrollback: 10000,
  });
  const fit = new FitAddon.FitAddon();
  term.loadAddon(fit);
  if (typeof CanvasAddon !== 'undefined') term.loadAddon(new CanvasAddon.CanvasAddon());
  term.open(pane.termEl);

  term.onSelectionChange(() => { const s = term.getSelection(); if (s) pane.lastSelection = s; });
  term.onData(data => {
    if (pane.ws && pane.ws.readyState === WebSocket.OPEN)
      pane.ws.send(new TextEncoder().encode(data));
  });

  pane.term = term;
  pane.fit = fit;

  // From here on the pane keeps itself fitted to its box; nothing else has to
  // remember to re-fit it after a layout change.
  observePaneSize(pane);
}

// ── Destroy-and-recreate ─────────────────────────────────────────────────
// The "layout is broken, I can't scroll history anymore" failure lives in
// xterm's own internal render/viewport state, not in the socket or the PTY —
// Reconnect already tears down and reopens the WebSocket into the *same*
// Terminal instance, and that alone doesn't clear it. So this goes further:
// dispose the whole Terminal (DOM, renderer, addons) and build a brand new
// one in its place, exactly as if the pane had just been created. The PTY
// itself is untouched — connectPane's fresh socket then replays the
// session's last 256 KB of output (see PtySession::replay in src/web.rs) to
// repaint it, so a live agent session picks back up rather than going blank.
function recreatePaneTerminal(idx) {
  const pane = panes[idx];
  if (!pane) return;
  const { sandbox, mode } = pane;

  if (pane.ws) { try { pane.ws.close(); } catch (_) {} pane.ws = null; }
  pane.term.dispose();
  pane.termEl.innerHTML = '';
  pane.lastSelection = '';
  setupTerminal(pane);

  if (sandbox) connectPane(idx, sandbox, mode);
}

function setFocusedPane(idx) {
  panes.forEach((p, i) => p.el.classList.toggle('focused', i === idx));
  focusedPane = idx;
  panes[idx]?.term.focus();
  // Which sandbox is being read decides whether its `attention` liseret shows
  // at all — on this pane, and on its sidebar row.
  applyAgentStates();
}

function setPaneMode(idx, mode, save = true) {
  const pane = panes[idx];
  if (!pane) return;
  pane.mode = mode;
  pane.el.querySelectorAll('.mode-btn').forEach(b => b.classList.toggle('active', b.dataset.mode === mode));
  applyAgentStates();
  if (save) saveLayout();
}

const termSize = pane => `${pane.term.cols}x${pane.term.rows}`;

// Tell the PTY this pane's size, and record it *on the socket that carried it*.
// A send on a socket that is not open yet is dropped by the browser without a
// word, so a fresh socket having no record is the accurate starting state rather
// than a thing to remember to reset — and when `closePane` shifts a socket to
// another pane, what it was told travels with it.
function sendPaneResize(pane) {
  if (!pane.ws || pane.ws.readyState !== WebSocket.OPEN) {
    termLog(pane, 'resize DROPPED (socket not open)');
    return;
  }
  pane.ws.send(JSON.stringify({ type: 'resize', cols: pane.term.cols, rows: pane.term.rows }));
  pane.ws.sentSize = termSize(pane);
  termLog(pane, 'resize sent');
}

// Sizing trace, off unless asked for: `sbxwTermLog()` turns it on and remembers
// across reloads, `sbxwTermLog(false)` stops it, `sbxwSizes()` prints the current
// state once. It answers the only question a terminal drawn at the wrong width
// raises — whether the mistake is in the measurement, in xterm, or in the PTY
// (`/api/ptys` answers for that last one).
let TERM_LOG = localStorage.getItem('sbxw:termlog') === '1';
function sbxwTermLog(on = true) {
  TERM_LOG = on;
  try { on ? localStorage.setItem('sbxw:termlog', '1') : localStorage.removeItem('sbxw:termlog'); } catch (_) {}
}
function termLog(pane, what, box) {
  if (!TERM_LOG) return;
  const cell = pane.term._core?._renderService?.dimensions?.css?.cell;
  console.log(
    `[sbxw] pane ${pane.index} ${pane.sandbox || '—'} ${what}: ` +
    `term ${termSize(pane)}, sent ${pane.ws?.sentSize || 'nothing'}, ` +
    `box ${box || `${pane.termEl.offsetWidth}x${pane.termEl.offsetHeight}`}px, ` +
    `cell ${cell ? `${cell.width?.toFixed(2)}x${cell.height?.toFixed(2)}` : '?'}px, ` +
    `dpr ${window.devicePixelRatio}, hidden ${document.hidden}`
  );
}
function sbxwSizes() { panes.slice(0, paneCount).forEach(p => termLog(p, 'state')); }

// Re-measure one pane and reconcile the PTY with it.
//
// A TUI drawn into a fraction of its pane is always the two sides disagreeing:
// xterm has one width, the PTY (shared by every viewer of that sandbox) has
// another, and the TUI draws to the PTY's. The comparison is therefore against
// what the socket was told, not against the previous fit: a resize can be
// prepared and then dropped for want of an open socket, and a PTY inherited from
// an earlier page has been told nothing at all.
function fitPane(pane, why = 'fit') {
  // One read of the box, before the fit writes to it — and the only thing that
  // can say whether there is anything to measure. A hidden tab has no layout,
  // and fitting from it would propose xterm's 80×24 default as if it were real.
  const box = `${pane.termEl.offsetWidth}x${pane.termEl.offsetHeight}`;
  if (!pane.termEl.offsetWidth || !pane.termEl.offsetHeight) {
    termLog(pane, `${why} SKIPPED (no box to measure)`, box);
    return;
  }
  pane.fit.fit();
  termLog(pane, why, box);
  if (termSize(pane) !== pane.ws?.sentSize) sendPaneResize(pane);
}

function refitPanes(why = 'refit') {
  panes.slice(0, paneCount).forEach(p => fitPane(p, why));
}

// Keep a pane fitted to whatever box the layout gives it, whenever that changes:
// window resized, devtools opened, grid switched, a hidden tab finally laid out.
// One observer replaces every "fit 50ms after the thing that resized it" guess —
// including the first fit, since observing an element reports its size straight
// away. Being idempotent, it cannot make a correctly-sized terminal blink.
//
// Coalesced to one frame: the observer fires per box change, and fitting three
// panes on a window drag should not send three resizes each.
function observePaneSize(pane) {
  if (typeof ResizeObserver === 'undefined') return;
  // recreatePaneTerminal() calls setupTerminal() — and so this — again on a
  // pane whose termEl already has an observer from the first time around;
  // without disconnecting it first, a repeated rebuild would pile up one more
  // observer on every click, each firing fitPane redundantly.
  pane.resizeObserver?.disconnect();
  let queued = false;
  const ro = new ResizeObserver(() => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => { queued = false; fitPane(pane, 'box changed'); });
  });
  ro.observe(pane.termEl);
  pane.resizeObserver = ro;
}

// ── Attention notifications ────────────────────────────────────────────────
// The server sends a {"type":"attention","sandbox":"<name>"} text frame when
// that sandbox's PTY emits a BEL (the agent is waiting on the user). Permission
// is requested lazily, on the first bell, instead of nagging on every page load.
function notifyAttention(sandboxName) {
  if (typeof Notification === 'undefined' || Notification.permission === 'denied') return;
  const fire = () => {
    if (Notification.permission !== 'granted') return;
    try {
      const n = new Notification('sbxw', { body: `${sandboxName} is waiting for your approval`, tag: `sbxw-attn-${sandboxName}` });
      n.onclick = () => { window.focus(); n.close(); };
    } catch (_) {}
  };
  if (Notification.permission === 'default') Notification.requestPermission().then(fire);
  else fire();
}

// Shared onmessage handler: binary frames are PTY output, text frames are
// JSON control messages (currently only "attention").
function handlePaneData(pane, ev) {
  if (typeof ev.data === 'string') {
    try {
      const msg = JSON.parse(ev.data);
      if (msg.type === 'attention') { notifyAttention(msg.sandbox || pane.sandbox); return; }
    } catch (_) { /* not JSON — fall through and write it verbatim */ }
    pane.term.write(ev.data);
  } else {
    pane.term.write(new Uint8Array(ev.data));
  }
}

// Which mode a pane must use when connecting to `name`, given what the caller
// asked for and what the pane is showing now.
//
// A pane's mode is sticky across reconnects, which is what makes the Bash
// toggle survive switching sandboxes. `monitor` is the exception: it belongs to
// the host pane alone, and the paths that just say "connect me to this sandbox"
// pass no mode — letting it stick would reopen the monitor under a sandbox's
// name, with the Claude/Bash toggles hidden and no way back. The target decides
// the mode, not the pane's current contents.
function paneModeFor(name, requested, current) {
  if (name === MONITOR_SANDBOX) return 'monitor';
  if (requested) return requested;
  return current === 'monitor' ? 'claude' : current;
}

function connectPane(idx, name, mode) {
  if (!name || idx >= panes.length) return;
  // Refuse to duplicate a sandbox already open in a different visible pane.
  if (panes.slice(0, paneCount).some((p, i) => i !== idx && p.sandbox === name)) return;
  const pane = panes[idx];
  const monitor = name === MONITOR_SANDBOX;
  const from = { sandbox: pane.sandbox, mode: pane.mode };

  setPaneMode(idx, paneModeFor(name, mode, pane.mode), false);

  // Where the Monitor button sends you back to. Only a real sandbox is worth
  // remembering, and leaving the monitor clears it.
  pane.beforeMonitor = monitor
    ? (from.sandbox && from.sandbox !== MONITOR_SANDBOX ? from : pane.beforeMonitor)
    : null;

  if (pane.ws) { try { pane.ws.close(); } catch (_) {} pane.ws = null; }
  // Full reset, not just clear(): a previous session (e.g. a TUI that enabled
  // xterm mouse-tracking) can leave DECSET modes on, which would otherwise
  // survive into the new session and turn mouse moves into stray input.
  pane.term.reset();
  pane.sandbox = name;
  // The liseret describes whoever is in the pane *now* — repainting drops the
  // outgoing session's state and picks up the incoming one's, mid-flash included.
  applyAgentStates();
  const label = document.getElementById(`plabel-${idx}`);
  // The command is the tooltip, not the label: `monitor_cmd` is bare `sbx`,
  // which as a pane title says nothing about what the pane is.
  label.textContent = monitor ? 'monitor' : name;
  label.title = monitor ? `host monitor — ${MONITOR_CMD}` : '';
  document.getElementById(`pconn-${idx}`).textContent = 'connecting…';
  document.getElementById(`pdot-${idx}`).className = 'dot term-disconnected';
  // The monitor runs on the host: there is no sandbox to ssh into, and the
  // Claude/Bash toggles would point at the pseudo-sandbox it is filed under.
  document.getElementById(`pssh-${idx}`).disabled = monitor;
  pane.el.querySelectorAll('.mode-btn').forEach(b => { b.hidden = monitor; });
  renderSidebar();

  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  pane.ws = new WebSocket(`${proto}://${location.host}/ws?sandbox=${encodeURIComponent(name)}&mode=${encodeURIComponent(pane.mode)}`);
  pane.ws.binaryType = 'arraybuffer';

  pane.ws.onopen = () => {
    document.getElementById(`pdot-${idx}`).className = 'dot term-connected';
    document.getElementById(`pconn-${idx}`).textContent = 'connected';
    // A brand-new socket has been told nothing, so this always announces the
    // size — unless the pane has no box to measure, in which case it declines
    // and the observer does it the moment there is one.
    fitPane(pane, 'socket open');
    if (focusedPane === idx) pane.term.focus();
    loadSandboxes();
  };
  pane.ws.onmessage = ev => handlePaneData(pane, ev);
  pane.ws.onclose = () => {
    document.getElementById(`pdot-${idx}`).className = 'dot term-disconnected';
    document.getElementById(`pconn-${idx}`).textContent = 'disconnected';
    loadSandboxes();
  };
  pane.ws.onerror = () => {
    document.getElementById(`pconn-${idx}`).textContent = 'error';
  };
  saveLayout();
}
