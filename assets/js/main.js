// "Jump to this sandbox" — the one thing an island click asks of the tab.
//
// Deliberately the smallest gesture that answers it: a sandbox already on
// screen only needs its pane focused. Reconnecting it would close a healthy
// WebSocket and `term.reset()` a terminal the user is reading, which is a blink
// of the session at best and a mid-flight refit at worst. Only a sandbox that
// is nowhere gets attached, to the focused pane.
function focusSandbox(name) {
  if (!name) return;
  const open = panes.slice(0, paneCount).findIndex(p => p.sandbox === name);
  if (open < 0) { connectPane(focusedPane, name); return; }
  setFocusedPane(open);
  // The one reconnect worth doing: a pane whose socket has died has no live
  // terminal to preserve, and landing on it would show a frozen screen.
  const pane = panes[open];
  if (!pane.ws || pane.ws.readyState > WebSocket.OPEN) connectPane(open, name, pane.mode);
}

// Deep-link: opening `…/#sandbox=<name>` focuses that session. Only used to
// cold-start a tab — an island click with a tab already open goes over the SSE
// stream below instead, precisely so that no navigation (and no reload) is
// involved.
function applySandboxHash() {
  const m = location.hash.match(/sandbox=([^&]+)/);
  if (!m) return;
  // Drop the `#sandbox=` fragment so the tab's URL stays the bare base URL.
  // That lets a later island click re-open the same URL and have the browser
  // focus *this* tab instead of treating a changed hash as a new page.
  history.replaceState(null, '', location.pathname + location.search);
  focusSandbox(decodeURIComponent(m[1]));
}
window.addEventListener('hashchange', applySandboxHash);

// The macOS notch companion (island) asks an already-open tab to switch
// sandboxes over this stream — so clicking a session focuses this tab rather
// than spawning a new page. See `/api/focus` in src/web.rs.
new EventSource('/api/focus-events').onmessage = ev => {
  if (!ev.data) return;
  window.focus();
  focusSandbox(ev.data);
};

// ── Agent activity on the liserets ────────────────────────────────────────
// `/api/events` carries one SessionInfo per state change, folded from the hooks
// the sandbox POSTs to `/api/hook` — the same feed the macOS island reads.
// Painting the latest state on both strips turns the window into a status
// board: the panes answer for what you have open, the sidebar for everything
// else, which is where an agent working in a sandbox you closed shows up.
//
// Sessions are keyed `<sandbox>::claude`, so a sidebar row keys off its sandbox
// alone while a pane is painted only in `claude` mode — animating a bash pane
// would claim an agent works in a terminal that has none.
//
// Both strips are one animation on a different edge, so the classes below are
// host-agnostic and the stylesheet decides where the line goes.

// Paint one liseret host — a `.pane` or a `.sbx-item` — for the session of
// `sandbox`. Both carry the same three classes and run the same animations;
// only where the strip is anchored differs (under the pane bar, along the top
// of the row).
function paintLiseret(el, sandbox, watchedSandbox = null) {
  const st = sandbox ? agentStates.get(`${sandbox}::claude`) : null;
  el.classList.toggle('agent-working', st === 'working');
  // A summons is pointless while you are reading the terminal it points at, so
  // it is suppressed on every strip speaking for that sandbox, sidebar row
  // included. Suppressed *while* watched rather than acknowledged once: leave
  // the pane or the tab and the amber returns, since the session still wants an
  // answer. A bash pane has no sandbox here (`null`) and must not read as
  // watched just because nothing else is.
  const watched = !!sandbox && sandbox === watchedSandbox;
  el.classList.toggle('agent-attention', st === 'attention' && !watched);

  const flash = sandbox ? doneFlashes.get(sandbox) : null;
  if (!flash) {
    el.classList.remove('agent-done');
    el.style.removeProperty('--liseret-delay');
  } else if (!el.classList.contains('agent-done')) {
    // renderSidebar() rebuilds every row from an HTML string, so a row created
    // during a flash starts with a fresh animation and would replay the sweep
    // from the top on each re-render. A negative delay drops it straight into
    // the point the others are at. Only on the way in: re-assigning the delay
    // to an element already animating would restart it.
    el.style.setProperty('--liseret-delay', `${flash.at - Date.now()}ms`);
    el.classList.add('agent-done');
  }
}

function applyAgentStates() {
  // The sandbox whose terminal is actually being read: the focused pane's, in
  // `claude` mode, in a focused window. The window condition matters — a
  // background tab still has a focused pane and nobody reading it, and a
  // summons raised then is exactly the one that must survive until you return.
  // A sandbox rather than a pane, since reading it settles every strip that
  // speaks for it, and it is what `announceWatching` tells the daemon.
  const focused = panes[focusedPane];
  const watchedSandbox = document.hasFocus() && focused?.mode === 'claude'
    ? focused.sandbox || null
    : null;
  // A bash pane is a shell you drive yourself — no agent, so nothing to show.
  panes.forEach(p => paintLiseret(p.el, p.mode === 'claude' ? p.sandbox : null, watchedSandbox));
  listEl.querySelectorAll('.sbx-item')
    .forEach(row => paintLiseret(row, row.dataset.name, watchedSandbox));

  announceWatching(watchedSandbox);
}

// Tell the daemon which terminal is being read, so the island can retire a
// notification for a session already looked at (`/api/watching`). Only on
// change — the signal is an arrival, not a state — and never for "none", which
// has no consumer and would make a tab that merely lost focus chatter.
let announcedWatch = null;
function announceWatching(sandbox) {
  if (sandbox === announcedWatch) return;
  announcedWatch = sandbox;
  if (!sandbox) return;
  fetch('/api/watching', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ sandbox }),
  }).catch(() => {});
}

// Every element currently standing in for `sandbox`: the panes showing its
// agent, and its sidebar row.
function liseretElements(sandbox) {
  const els = panes.filter(p => p.sandbox === sandbox && p.mode === 'claude').map(p => p.el);
  const row = listEl.querySelector(`.sbx-item[data-name="${CSS.escape(sandbox)}"]`);
  if (row) els.push(row);
  return els;
}

function flashDone(sandbox) {
  clearTimeout(doneFlashes.get(sandbox)?.timer);
  // Re-adding a class an element already carries does not restart its
  // animation, so back-to-back turns would land only the first sweep. Clearing
  // it and reading a layout property forces the reflow that arms the next one.
  liseretElements(sandbox).forEach(el => {
    el.classList.remove('agent-done');
    el.style.removeProperty('--liseret-delay');
    void el.offsetWidth;
  });
  doneFlashes.set(sandbox, {
    at: Date.now(),
    timer: setTimeout(() => { doneFlashes.delete(sandbox); applyAgentStates(); }, DONE_FLASH_MS),
  });
  applyAgentStates();
}

// `seed` marks the initial snapshot: those states are already true rather than
// freshly reached, so a session that has been idle since yesterday must not
// flash green just because you reloaded the tab.
function ingestSession(info, seed = false) {
  if (!info || !info.sandbox) return;
  const key = `${info.sandbox}::${info.mode || 'claude'}`;
  const prev = agentStates.get(key);
  if (info.state === 'exited') agentStates.delete(key);
  else agentStates.set(key, info.state);
  // "Finished thinking" is a transition, not a state — `idle` is also where a
  // session sits between prompts, and that is not news.
  if (!seed && info.state === 'idle' && (prev === 'working' || prev === 'attention'))
    flashDone(info.sandbox);
  applyAgentStates();
}

fetch('/api/sessions')
  .then(r => r.json())
  .then(list => list.forEach(i => ingestSession(i, true)))
  .catch(() => {});

new EventSource('/api/events').onmessage = ev => {
  try { ingestSession(JSON.parse(ev.data)); } catch (_) {}
};

// Leaving the tab un-watches the focused pane, coming back watches it again —
// so a session that raised `attention` while you were elsewhere is still
// flagged when you return, and stops being flagged the moment you read it.
window.addEventListener('focus', applyAgentStates);
window.addEventListener('blur', applyAgentStates);

// Coming back to the tab reconciles every pane's size against the live PTY
// instead of blindly re-announcing it. A PTY is shared by everyone watching
// that sandbox, so the one thing this tab cannot notice while it is away is
// somebody else resizing it — no box changes here, and the observer has
// nothing to fire on. But forgetting what every socket was told (nulling
// `sentSize` outright) made *every* pane re-announce on *every* return,
// whether or not its size actually moved — and after a long sleep or a
// reconnect, several panes come back at once. Each unnecessary resize is a
// real SIGWINCH to that sandbox's PTY, and a CLI agent (Claude Code among
// them) redraws its whole screen off the back of one — a wave of full-screen
// redraws across every open pane for no reason. `/api/ptys` reads the PTY's
// actual size, the same truth `sentSize` is meant to track, so reconcile
// against that instead of discarding it: only a pane whose size truly drifted
// while this tab was away ends up sending anything.
document.addEventListener('visibilitychange', async () => {
  if (document.hidden) return;
  let live = new Map();
  try {
    const res = await fetch('/api/ptys');
    live = new Map((await res.json()).map(e => [e.key, e]));
  } catch (_) { /* fall back to each pane's last-known sentSize below */ }
  panes.slice(0, paneCount).forEach(p => {
    if (!p.ws) return;
    const entry = p.sandbox ? live.get(`${p.sandbox}::${p.mode}`) : null;
    if (entry && entry.cols != null && entry.rows != null)
      p.ws.sentSize = `${entry.cols}x${entry.rows}`;
  });
  refitPanes('tab visible');
});

// ── Init ──────────────────────────────────────────────────────────────────
// Last in the last script, and it has to stay there: laying out the panes
// paints the liserets, which reads the module-level state above (`announcedWatch`
// and friends). Booting from the top of this file would run that before those
// `let`s are initialised — a TDZ ReferenceError, not an `undefined` to shrug
// off. Function declarations hoist, top-level bindings do not.
(async () => {
  const maxPanes = computeMaxPanes();
  renderLayoutSwitch(maxPanes);
  const saved = loadLayout();
  const n = (saved?.n >= 1 && saved?.n <= maxPanes) ? saved.n : 1;
  setLayout(n);
  await loadSandboxes();
  // The monitor pane has no sandbox to still exist, so it restores on the
  // command being configured instead.
  const exists = name =>
    name === MONITOR_SANDBOX ? !!MONITOR_CMD : sandboxes.some(s => s.name === name);

  if (saved?.panes?.length) {
    // Restore saved pane connections for sandboxes that still exist.
    let connected = false;
    saved.panes.slice(0, n).forEach((state, i) => {
      if (state.sandbox && exists(state.sandbox)) {
        connectPane(i, state.sandbox, state.mode || 'claude');
        connected = true;
      }
    });
    if (!connected) {
      // No saved sandbox exists anymore — fall back to default.
      const initial = (INITIAL_SANDBOX && exists(INITIAL_SANDBOX)) ? INITIAL_SANDBOX : sandboxes[0]?.name;
      if (initial) {
        connectPane(0, initial);
      } else {
        document.getElementById('pconn-0').textContent =
          sandboxes.length ? 'select a sandbox →' : 'create a sandbox with ＋';
        panes[0].term.write('\r\n  \x1b[90mNo sandbox attached. Pick one from the sidebar, or create a new one with ＋.\x1b[0m\r\n');
      }
    }
  } else {
    // No saved layout — use default behaviour.
    const initial = (INITIAL_SANDBOX && exists(INITIAL_SANDBOX)) ? INITIAL_SANDBOX : sandboxes[0]?.name;
    if (initial) {
      connectPane(0, initial);
    } else {
      document.getElementById('pconn-0').textContent =
        sandboxes.length ? 'select a sandbox →' : 'create a sandbox with ＋';
      panes[0].term.write('\r\n  \x1b[90mNo sandbox attached. Pick one from the sidebar, or create a new one with ＋.\x1b[0m\r\n');
    }
  }
  // A `#sandbox=<name>` fragment (e.g. opened from the macOS notch companion)
  // wins over the restored layout: attach that sandbox to the focused pane.
  applySandboxHash();
})();
