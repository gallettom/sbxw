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
  // minmax(0, …), not a bare 1fr (which is minmax(auto, 1fr)): a terminal's
  // canvas has a fixed width, and with an `auto` floor it widened its own
  // column and squeezed an empty neighbour down to a sliver.
  mainEl.style.gridTemplateColumns = `repeat(${cols}, minmax(0, 1fr))`;
  mainEl.style.gridTemplateRows = `repeat(${rows}, minmax(0, 1fr))`;

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
  schedulePaintGroups();
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
      <span class="role-chip" id="prole-${index}" hidden></span>
      <span class="conn-label" id="pconn-${index}"></span>
      <!-- Beside the name, not over with SSH/Env/Reconnect on the right: those
           act on the *session* in this pane, while a project is a fact about
           the sandbox the name belongs to — the same workspace, whichever pane
           happens to be showing it. -->
      <button class="pane-btn pane-files-btn" id="pfiles-${index}" disabled
              title="Project — this sandbox's code map and the deliverables under .sbxw-artifacts/">
        <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"><path d="M1.5 3.5h4l1.2 1.6h7.3a.5.5 0 0 1 .5.5v7a.5.5 0 0 1-.5.5h-12a.5.5 0 0 1-.5-.5v-9a.5.5 0 0 1 .5-.5z"/></svg><span class="files-word">Project</span>
      </button>
      <button class="pane-btn pane-thread-close" id="pthreadclose-${index}" hidden
              title="Close this thread — stop it, remove its worktree, and its branch">Close thread</button>
      <span class="spacer"></span>
      <button class="pane-btn pane-session-btn" id="pssh-${index}" title="SSH details for this sandbox — fields for a client, and the shell command (run 'sbxw ssh --setup' once first)" disabled>SSH</button>
      <button class="pane-btn pane-session-btn" id="penv-${index}" title="Environment file (.sbxenv.yaml) for this sandbox — its workspace and published ports in sbx's own format, for a colleague to bring the same sandbox up" disabled>Env</button>
      <button class="pane-btn pane-session-btn pane-reconnect-btn" id="preconnect-${index}">Reconnect</button>
      <button class="pane-btn pane-session-btn" id="prefresh-${index}" title="Rebuild this pane's terminal from scratch — fixes a broken layout that Reconnect alone can't, by destroying and recreating the terminal widget (then reconnecting)">↻</button>
      <div class="mode-switch" role="tablist">
        <!-- The word is wrapped so a narrow pane can drop it and keep the
             glyph, which carries the mode on its own (see app.css). -->
        <button class="mode-btn active" data-mode="claude" title="Claude" aria-label="Claude">✦<span class="mode-word"> Claude</span></button>
        <button class="mode-btn" data-mode="bash" title="Bash" aria-label="Bash">❯<span class="mode-word"> Bash</span></button>
        <button class="mode-btn" data-mode="thread" title="Thread — another Claude session in this sandbox, in a git worktree and branch of its own" aria-label="Thread">⑂<span class="mode-word"> Thread</span></button>
      </div>
      <button class="pane-close-btn" id="pclose-${index}" title="Close pane" style="display:none">✕</button>
    </div>
    <div class="pane-term" id="pterm-${index}"></div>`;
  document.getElementById('main-area').appendChild(el);

  const termEl = document.getElementById(`pterm-${index}`);

  const pane = {
    index, el, termEl,
    ws: null, sandbox: null, mode: 'claude',
    // Name of the thread shown, when `mode` is 'thread'.
    thread: null,
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
  const [claudeBtn, bashBtn, threadBtn] = el.querySelectorAll('.mode-btn');
  claudeBtn.addEventListener('click', () => {
    if (pane.mode !== 'claude' && pane.sandbox) connectPane(index, pane.sandbox, 'claude');
    else setPaneMode(index, 'claude');
  });
  bashBtn.addEventListener('click', () => {
    if (pane.mode !== 'bash' && pane.sandbox) connectPane(index, pane.sandbox, 'bash');
    else setPaneMode(index, 'bash');
  });
  threadBtn.addEventListener('click', () => {
    if (pane.sandbox && pane.sandbox !== MONITOR_SANDBOX) openThread(index);
  });

  document.getElementById(`preconnect-${index}`).addEventListener('click', () => {
    if (pane.sandbox) connectPane(index, pane.sandbox, pane.mode);
  });
  document.getElementById(`prefresh-${index}`).addEventListener('click', () => recreatePaneTerminal(index));
  document.getElementById(`pthreadclose-${index}`).addEventListener('click', () => closeThread(index));
  document.getElementById(`pclose-${index}`).addEventListener('click', () => closePane(index));
  document.getElementById(`pssh-${index}`).addEventListener('click', ev => {
    if (pane.sandbox) toggleSshPop(pane.sandbox, ev.currentTarget);
  });
  document.getElementById(`penv-${index}`).addEventListener('click', ev => {
    if (pane.sandbox) toggleEnvPop(pane.sandbox, ev.currentTarget);
  });
  document.getElementById(`pfiles-${index}`).addEventListener('click', () => {
    if (pane.sandbox) openFilesModal(pane.sandbox);
  });

  return pane;
}

let oscCopyFailed = false;

// Builds a fresh xterm Terminal (+ addons) into `pane.termEl` and points
// pane.term/pane.fit at it. Called once from createPane, and again from
// recreatePaneTerminal() on an existing pane — so every wire-up here must
// read `pane.*` rather than close over locals that a second call would
// shadow instead of replace.
function setupTerminal(pane) {
  const term = new Terminal({
    cursorBlink: true, fontSize: 13,
    fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace',
    // --c-bg, so the terminal and the chrome around it are one black, not two.
    theme: { background: '#0d1117', foreground: '#c9d1d9', cursor: '#58a6ff' },
    scrollback: 10000,
  });
  const fit = new FitAddon.FitAddon();
  term.loadAddon(fit);
  if (typeof CanvasAddon !== 'undefined') term.loadAddon(new CanvasAddon.CanvasAddon());
  term.open(pane.termEl);

  term.onSelectionChange(() => { const s = term.getSelection(); if (s) pane.lastSelection = s; });

  // ── Clipboard writes from inside the sandbox (OSC 52) ───────────────────
  // A program in the PTY has no clipboard within reach: the sandbox's `xclip`
  // and `wl-paste` are shims that only *read* images back out (`sbx-clipboard`),
  // and a write to them exits 0 having copied nothing — which is how an agent
  // reports "copied 7 chars to clipboard" while the user's clipboard never
  // changes. OSC 52 is the sequence that asks the *terminal* to do the copy
  // instead, and through this browser it is the one path out of the sandbox
  // that ends on the user's own clipboard. Claude Code, vim's `"+y` and tmux
  // all emit it; nothing was listening.
  //
  // It also covers the case the pane's own copy-on-select cannot: while a TUI
  // holds mouse tracking, xterm makes no selection to copy, so the double-click
  // is the app's and the clipboard write has to come from the app too.
  //
  // Writes only. The read form (`52;c;?`) replies down the PTY, handing
  // whatever the user is carrying — a password out of a manager, say — to any
  // program that asks for it, or to any file that happens to be `cat`ed.
  // ── A repaint must not take the selection away ──────────────────────────
  // xterm clears the selection every time the program re-declares its mouse
  // mode: `CoreMouseService`'s `activeProtocol` setter fires `onProtocolChange`
  // unconditionally, and `Terminal` answers that by calling
  // `SelectionService.disable()`, whose first line is `clearSelection()`. An
  // agent's TUI re-sends `?1000;1002;1003;1006h` on essentially every repaint —
  // 115 times in one 256 KB replay of a live session — so a selection could not
  // outlive the next line of output arriving behind it.
  //
  // A real terminal resets nothing when a program re-declares a mode it already
  // has, so neither does this one: the setter is made idempotent, and a genuine
  // change still goes through untouched. Reaching into `_core` is the price —
  // the guard below keeps a future xterm that renames this from throwing.
  //
  // This must stay *after* `term.open()`: xterm ends `_bindMouse` with a
  // deliberate self-assignment ("force initial onProtocolChange so we dont miss
  // early mouse requests"), and that one has to fire to wire up the mouse
  // listeners. Patched before open, it would be the one assignment we skip.
  const mouse = term._core?.coreMouseService;
  const proto = mouse && Object.getOwnPropertyDescriptor(Object.getPrototypeOf(mouse), 'activeProtocol');
  if (proto?.get && proto?.set) {
    Object.defineProperty(mouse, 'activeProtocol', {
      configurable: true,
      get: () => proto.get.call(mouse),
      set: v => { if (v !== proto.get.call(mouse)) proto.set.call(mouse, v); },
    });
  }

  term.parser.registerOscHandler(52, payload => {
    const b64 = payload.slice(payload.indexOf(';') + 1);
    if (!b64 || b64 === '?') return true;
    let text = '';
    try {
      // base64 → bytes → UTF-8. `atob` alone stops at Latin-1 and would turn
      // every accented character into two.
      text = new TextDecoder().decode(Uint8Array.from(atob(b64), c => c.charCodeAt(0)));
    } catch (_) {
      return true; // Malformed payload: handled, and nothing to copy.
    }
    if (text) copyQuiet(text).then(ok => {
      // Once per tab: the reason is the address the page was opened on or the
      // browser's rules, and neither changes between one copy and the next.
      if (ok || oscCopyFailed) return;
      oscCopyFailed = true;
      showToast('The browser refused a clipboard copy from the sandbox', 'error');
    });
    return true;
  });
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
  const focusedSandbox = panes[idx]?.sandbox;
  listEl.querySelectorAll('.sbx-item').forEach(row =>
    row.classList.toggle('focused', !!focusedSandbox && row.dataset.name === focusedSandbox));
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
  schedulePaintGroups();
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
//
// `thread` does not stick either: a thread belongs to one sandbox, and "connect
// this pane to that other sandbox" means its agent, not a thread of the same
// name over there.
function paneModeFor(name, requested, current) {
  if (name === MONITOR_SANDBOX) return 'monitor';
  if (requested) return requested;
  return current === 'monitor' || current === 'thread' ? 'claude' : current;
}

// Which session a pane holds, as one comparable string. A sandbox's agent and
// its shell are one slot — showing both in two panes was never offered — while
// each thread is a session of its own, so it can sit beside the agent.
function sessionIdentity(sandbox, mode, thread) {
  if (!sandbox) return null;
  return mode === 'thread' ? `${sandbox}::thread:${thread}` : `${sandbox}::main`;
}

// The pane bar as it depends on *what* a pane holds rather than on which
// sandbox: its title, and the buttons that need a sandbox behind them. An empty
// pane has nothing to ssh into, describe, or open a project for; the monitor
// runs on the host, so it has no sandbox to ssh into, no env file, and no
// workspace — neither a project of its own nor anything for the Claude/Bash
// toggles (which would otherwise point at the pseudo-sandbox it is filed under)
// to read a map from. One function because three paths set this: connecting a
// pane, emptying one, and shifting a live session into one when a pane closes.
function applyPaneChrome(idx, name) {
  const monitor = name === MONITOR_SANDBOX;
  const label = document.getElementById(`plabel-${idx}`);
  // The command is the tooltip, not the label: `monitor_cmd` is bare `sbx`,
  // which as a pane title says nothing about what the pane is.
  const thread = panes[idx].mode === 'thread' ? panes[idx].thread : null;
  // The thread's name is its role chip, painted by paintGroups().
  label.textContent = monitor ? 'monitor' : (name || '—');
  label.title = monitor ? `host monitor — ${MONITOR_CMD}`
    : thread ? `Thread "${thread}" — worktree .sbxw-worktrees/${thread}, branch sbxw/${thread}` : '';
  document.getElementById(`pssh-${idx}`).disabled = !name || monitor;
  document.getElementById(`penv-${idx}`).disabled = !name || monitor;
  // The monitor runs on the host and has no workspace, so it has no project
  // either — the same reason SSH and Env go dark on it.
  document.getElementById(`pfiles-${idx}`).disabled = !name || monitor;
  panes[idx].el.querySelectorAll('.mode-btn').forEach(b => { b.hidden = monitor; });
  document.getElementById(`pthreadclose-${idx}`).hidden = !(name && thread);
}

/**
 * Put a pane back to the state a fresh one is in: no socket, no session, no
 * name in the bar, nothing left on screen.
 *
 * Closing the socket is not the same thing, and the difference is what a
 * removed sandbox used to look like: the pane kept the dead session's last
 * screen under its name, which reads exactly like a session that is still
 * there and merely quiet. The terminal is reset rather than cleared, so a TUI
 * that left mouse-tracking on cannot leak DECSET modes into whatever is
 * connected here next.
 *
 * Leaves the layout alone — how many panes there are is the user's choice, not
 * a consequence of one of them emptying. Callers that also want the slot gone
 * use `closePane`.
 */
function emptyPane(idx) {
  const pane = panes[idx];
  if (!pane) return;
  if (pane.ws) { try { pane.ws.close(); } catch (_) {} pane.ws = null; }
  pane.sandbox = null;
  pane.thread = null;
  pane.beforeMonitor = null;
  pane.term.reset();
  pane.lastSelection = '';
  document.getElementById(`pconn-${idx}`).textContent = '';
  document.getElementById(`pdot-${idx}`).className = 'dot term-disconnected';
  applyPaneChrome(idx, null);
  // The liseret speaks for whoever is in the pane, and now nobody is.
  applyAgentStates();
  schedulePaintGroups();
}

function connectPane(idx, name, mode, thread) {
  if (!name || idx >= panes.length) return;
  const pane = panes[idx];
  const nextMode = paneModeFor(name, mode, pane.mode);
  // Reconnect and ↻ pass the mode alone; the thread is the one already here.
  const nextThread = nextMode === 'thread'
    ? (thread || (name === pane.sandbox ? pane.thread : null))
    : null;
  if (nextMode === 'thread' && !nextThread) return;
  // Refuse to duplicate a session already open in a different visible pane.
  const id = sessionIdentity(name, nextMode, nextThread);
  if (panes.slice(0, paneCount).some((p, i) => i !== idx && sessionIdentity(p.sandbox, p.mode, p.thread) === id)) return;
  const monitor = name === MONITOR_SANDBOX;
  const from = { sandbox: pane.sandbox, mode: pane.mode, thread: pane.thread };

  pane.thread = nextThread;
  setPaneMode(idx, nextMode, false);

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
  document.getElementById(`pconn-${idx}`).textContent = 'connecting…';
  document.getElementById(`pdot-${idx}`).className = 'dot term-disconnected';
  applyPaneChrome(idx, name);
  renderSidebar();

  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const threadParam = pane.thread ? `&thread=${encodeURIComponent(pane.thread)}` : '';
  pane.ws = new WebSocket(`${proto}://${location.host}/ws?sandbox=${encodeURIComponent(name)}&mode=${encodeURIComponent(pane.mode)}${threadParam}`);
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

// ── Threads ───────────────────────────────────────────────────────────────
// A thread is another Claude session in the same sandbox, working in a git
// worktree and branch of its own (`.sbxw-worktrees/<name>`, `sbxw/<name>`), so
// two agents never edit one checkout. The daemon creates the worktree the first
// time a name is opened and reattaches to it after that; nothing here removes
// one, since it can hold work nobody has committed.

const THREAD_NAME = /^[a-z0-9](?:[a-z0-9-]{0,38}[a-z0-9])?$/;

const threadOverlay = document.getElementById('thread-modal-overlay');
const threadInput = document.getElementById('thread-name');
const threadOpenBtn = document.getElementById('thread-modal-open');
const threadError = document.getElementById('thread-error');
// The pane that asked, and the sandbox it showed then.
let threadAsk = null;

// Ask for a thread of the sandbox in pane `idx`. The existing ones are offered
// as chips — reopening one is the common case after a reload.
async function openThread(idx) {
  const sandbox = panes[idx]?.sandbox;
  if (!sandbox) return;
  let info = null;
  try { info = await (await fetch(`/api/sandboxes/${encodeURIComponent(sandbox)}/threads`)).json(); } catch (_) {}
  const known = info?.ok ? info.threads : [];
  const used = new Set(known.map(t => t.name));
  let n = 1;
  while (used.has(`t${n}`)) n++;

  threadAsk = { idx, sandbox, known };
  document.getElementById('thread-modal-title').textContent = `New thread in ${sandbox}`;
  document.getElementById('thread-hint').textContent =
    'Lower-case letters, digits and hyphens. It gets the worktree .sbxw-worktrees/<name> and the branch sbxw/<name>.';
  document.getElementById('thread-count').textContent =
    info?.ok ? `${info.open} of ${info.max} Claude sessions open` : '';
  const chips = document.getElementById('thread-existing');
  chips.innerHTML = known.map(t =>
    `<button type="button" class="thread-chip${t.open ? ' open' : ''}" data-name="${escHtml(t.name)}"
       title="${t.open ? 'Open — go to it' : 'Worktree kept — reopen it'}">${escHtml(t.name)}</button>`).join('');
  document.getElementById('thread-existing-wrap').hidden = !known.length;
  threadInput.value = `t${n}`;
  validateThreadName();
  threadOverlay.classList.remove('hidden');
  threadInput.focus();
  threadInput.select();
}

function closeThreadModal() { threadOverlay.classList.add('hidden'); threadAsk = null; }

function validateThreadName() {
  const name = threadInput.value.trim();
  const ok = THREAD_NAME.test(name);
  threadInput.classList.toggle('error', !!name && !ok);
  threadError.textContent = name && !ok ? 'Lower-case letters, digits and inner hyphens only' : '';
  threadError.classList.toggle('hidden', !threadError.textContent);
  const existing = threadAsk?.known.find(t => t.name === name);
  threadOpenBtn.textContent = existing ? (existing.open ? 'Go to thread' : 'Reopen thread') : 'Open thread';
  threadOpenBtn.disabled = !ok;
}

function submitThread(name) {
  if (!threadAsk || !THREAD_NAME.test(name)) return;
  const { idx, sandbox } = threadAsk;
  closeThreadModal();
  showThread(idx, sandbox, name);
}

// Put thread `name` of `sandbox` on screen, beside its sandbox's panes: in the
// first empty pane, else a new one while the window has room, else in pane
// `idx` (the agent's session keeps running behind it).
function showThread(idx, sandbox, name) {
  const id = sessionIdentity(sandbox, 'thread', name);
  const shown = panes.slice(0, paneCount).findIndex(p => sessionIdentity(p.sandbox, p.mode, p.thread) === id);
  if (shown >= 0) { setFocusedPane(shown); return; }

  let target = panes.slice(0, paneCount).findIndex(p => !p.sandbox);
  if (target < 0 && paneCount < computeMaxPanes()) { setLayout(paneCount + 1); target = paneCount - 1; }
  if (target < 0) target = idx;
  connectPane(target, sandbox, 'thread', name);
  regroupPanes();
  const at = panes.slice(0, paneCount).findIndex(p => sessionIdentity(p.sandbox, p.mode, p.thread) === id);
  setFocusedPane(at >= 0 ? at : target);
}

threadInput.addEventListener('input', validateThreadName);
threadInput.addEventListener('keydown', e => {
  if (e.key === 'Enter' && !threadOpenBtn.disabled) { e.preventDefault(); submitThread(threadInput.value.trim()); }
});
threadOpenBtn.addEventListener('click', () => submitThread(threadInput.value.trim()));
document.getElementById('thread-existing').addEventListener('click', e => {
  const chip = e.target.closest('.thread-chip');
  if (chip) submitThread(chip.dataset.name);
});
document.getElementById('thread-modal-close').addEventListener('click', closeThreadModal);
document.getElementById('thread-modal-cancel').addEventListener('click', closeThreadModal);
threadOverlay.addEventListener('click', e => { if (e.target === threadOverlay) closeThreadModal(); });

// ── Close a thread ──
// What would be lost is read first and shown, and the dialog cannot discard
// uncommitted files until that is ticked. The branch goes by default only when
// it holds nothing its base lacks; with unmerged commits it is kept unless
// ticked, since those commits are work.
const closeOverlay = document.getElementById('thread-close-overlay');
const closeConfirm = document.getElementById('thread-close-confirm');
const closeError = document.getElementById('thread-close-error');
const discardBox = document.getElementById('thread-close-discard');
const branchBox = document.getElementById('thread-close-branch');
let closeAsk = null;

async function closeThread(idx) {
  const { sandbox, thread } = panes[idx] || {};
  if (!sandbox || !thread) return;
  const base = `/api/sandboxes/${encodeURIComponent(sandbox)}/threads/${encodeURIComponent(thread)}`;
  closeAsk = { sandbox, thread, base, st: null };
  document.getElementById('thread-close-title').textContent = `Close thread ${thread}`;
  document.getElementById('thread-close-path').textContent = 'Reading the worktree…';
  document.getElementById('thread-close-dirty-row').hidden = true;
  document.getElementById('thread-close-branch-row').hidden = true;
  setCloseError('');
  closeConfirm.disabled = true;
  closeConfirm.textContent = 'Close thread';
  closeOverlay.classList.remove('hidden');

  let st;
  try {
    const r = await (await fetch(base)).json();
    if (!r.ok) throw new Error(r.error || 'unknown error');
    st = r.status;
  } catch (e) {
    document.getElementById('thread-close-path').textContent = '—';
    setCloseError(`Could not read the thread: ${e.message}`);
    return;
  }
  if (closeAsk?.thread !== thread) return;
  closeAsk.st = st;
  document.getElementById('thread-close-path').textContent = st.worktree;
  discardBox.checked = false;
  document.getElementById('thread-close-dirty').textContent =
    `Discard ${st.dirty} uncommitted file${st.dirty === 1 ? '' : 's'} in it`;
  document.getElementById('thread-close-dirty-row').hidden = !st.dirty;
  branchBox.checked = st.has_branch && !st.ahead;
  document.getElementById('thread-close-branch-text').textContent = st.ahead
    ? `Delete branch ${st.branch} too, with its ${st.ahead} commit${st.ahead === 1 ? '' : 's'} not in ${st.base}`
    : `Delete branch ${st.branch} too (nothing in it that ${st.base} lacks)`;
  document.getElementById('thread-close-branch-row').hidden = !st.has_branch;
  updateCloseConfirm();
}

function setCloseError(msg) {
  closeError.textContent = msg;
  closeError.classList.toggle('hidden', !msg);
}

function updateCloseConfirm() {
  const st = closeAsk?.st;
  closeConfirm.disabled = !st || (st.dirty > 0 && !discardBox.checked);
}

function closeCloseModal() { closeOverlay.classList.add('hidden'); closeAsk = null; }

discardBox.addEventListener('change', updateCloseConfirm);
closeConfirm.addEventListener('click', async () => {
  const ask = closeAsk;
  if (!ask?.st) return;
  closeConfirm.disabled = true;
  closeConfirm.textContent = 'Closing…';
  setCloseError('');
  let res;
  try {
    res = await (await fetch(`${ask.base}/close`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ discard: discardBox.checked, delete_branch: branchBox.checked }),
    })).json();
  } catch (e) {
    res = { ok: false, error: e.message };
  }
  if (!res.ok) {
    setCloseError(res.error || 'could not close the thread');
    closeConfirm.textContent = 'Close thread';
    updateCloseConfirm();
    return;
  }
  closeCloseModal();
  // The pane may have moved while the dialog was open.
  const at = panes.slice(0, paneCount)
    .findIndex(p => p.sandbox === ask.sandbox && p.mode === 'thread' && p.thread === ask.thread);
  if (at >= 0) { if (paneCount > 1) closePane(at); else emptyPane(at); }
});
document.getElementById('thread-close-x').addEventListener('click', closeCloseModal);
document.getElementById('thread-close-cancel').addEventListener('click', closeCloseModal);
closeOverlay.addEventListener('click', e => { if (e.target === closeOverlay) closeCloseModal(); });

document.addEventListener('keydown', e => {
  if (e.key !== 'Escape') return;
  if (!threadOverlay.classList.contains('hidden')) closeThreadModal();
  if (!closeOverlay.classList.contains('hidden')) closeCloseModal();
});

// ── Groups ────────────────────────────────────────────────────────────────
// A sandbox shown in more than one pane — its agent and its threads — is one
// piece of work, so its panes sit side by side and share a thin outline. The
// outline is drawn per edge, only where a pane does not touch another of its
// group, so a group reads as one block rather than as boxes in a row.

const GROUP_COLORS = ['var(--c-group-1)', 'var(--c-group-2)', 'var(--c-group-3)', 'var(--c-group-4)'];

// Reorder the visible panes so each sandbox's panes are contiguous, in the
// order the sandboxes first appear, the agent ahead of its threads, and empty
// panes last — a gap inside the grid would split the groups apart. Only the
// panes whose content changes are reconnected — a reconnect replays the
// session, so it costs a repaint, never the session.
function regroupPanes() {
  const vis = panes.slice(0, paneCount);
  const now = vis.map(p => ({ sandbox: p.sandbox, mode: p.mode, thread: p.thread }));
  const first = new Map();
  now.forEach((d, i) => { if (d.sandbox && !first.has(d.sandbox)) first.set(d.sandbox, i); });
  const want = now
    .map((d, i) => ({ d, i, k: d.sandbox ? first.get(d.sandbox) : Infinity, sub: d.mode === 'thread' ? 1 : 0 }))
    .sort((a, b) => a.k - b.k || a.sub - b.sub || a.i - b.i)
    .map(x => x.d);
  const same = (a, b) => a.sandbox === b.sandbox && a.mode === b.mode && a.thread === b.thread;
  const moved = want.map((d, i) => (same(d, now[i]) ? -1 : i)).filter(i => i >= 0);
  if (!moved.length) return;
  moved.forEach(i => emptyPane(i));
  moved.forEach(i => { if (want[i].sandbox) connectPane(i, want[i].sandbox, want[i].mode, want[i].thread); });
}

let groupsQueued = false;
function schedulePaintGroups() {
  if (groupsQueued) return;
  groupsQueued = true;
  requestAnimationFrame(() => { groupsQueued = false; paintGroups(); });
}
window.addEventListener('resize', schedulePaintGroups);

function paintGroups() {
  const vis = panes.slice(0, paneCount);
  const count = new Map();
  vis.forEach(p => {
    if (p.sandbox && p.sandbox !== MONITOR_SANDBOX) count.set(p.sandbox, (count.get(p.sandbox) || 0) + 1);
  });
  const colorOf = new Map();
  vis.forEach(p => {
    if ((count.get(p.sandbox) || 0) > 1 && !colorOf.has(p.sandbox))
      colorOf.set(p.sandbox, GROUP_COLORS[colorOf.size % GROUP_COLORS.length]);
  });
  const rects = vis.map(p => p.el.getBoundingClientRect());
  const near = (a, b) => Math.abs(a - b) <= 3;
  panes.forEach((p, i) => {
    p.el.classList.remove('grouped', 'edge-t', 'edge-r', 'edge-b', 'edge-l');
    const color = i < paneCount ? colorOf.get(p.sandbox) : null;
    // A thread always names itself; the agent says "main" only when it has a
    // thread on screen to be told apart from.
    const chip = document.getElementById(`prole-${i}`);
    const thread = p.sandbox && p.mode === 'thread' ? p.thread : null;
    chip.textContent = thread ? `⑂ ${thread}` : 'main';
    chip.classList.toggle('thread', !!thread);
    chip.hidden = !(i < paneCount && p.sandbox && (thread || color));
    if (!color) { p.el.style.removeProperty('--group-color'); return; }
    p.el.style.setProperty('--group-color', color);
    p.el.classList.add('grouped');
    const r = rects[i];
    const mates = vis.filter((q, j) => j !== i && q.sandbox === p.sandbox).map(q => rects[q.index]);
    const acrossX = q => Math.min(r.right, q.right) - Math.max(r.left, q.left) > 4;
    const acrossY = q => Math.min(r.bottom, q.bottom) - Math.max(r.top, q.top) > 4;
    if (!mates.some(q => near(q.bottom, r.top) && acrossX(q))) p.el.classList.add('edge-t');
    if (!mates.some(q => near(q.top, r.bottom) && acrossX(q))) p.el.classList.add('edge-b');
    if (!mates.some(q => near(q.right, r.left) && acrossY(q))) p.el.classList.add('edge-l');
    if (!mates.some(q => near(q.left, r.right) && acrossY(q))) p.el.classList.add('edge-r');
  });
}
