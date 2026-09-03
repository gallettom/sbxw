// ── State ─────────────────────────────────────────────────────────────────
let sandboxes = [];
const listEl = document.getElementById('sandbox-list');

// Sandboxes still provisioning in the background (each reporting from a card
// in the bottom-right corner stack) and sandboxes that just became ready
// without the UI jumping to them (rendered as a small blue "ready" dot in the
// list until the user opens them).
const pendingCreations = new Map(); // name -> { job, steps, at, detail }
const readySandboxes = new Set();

// If a visible pane is sitting empty (no sandbox attached), a freshly
// created/duplicated sandbox attaches to it straight away instead of just
// sitting in the sidebar as "ready" — no need to manually connect it when
// there was somewhere obvious for it to go. If every pane is busy, the
// layout grows by one pane and the sandbox attaches there instead — unless
// the layout is already at the max the window can fit, in which case this
// falls back to the old "flag it ready, switch to it when you're ready"
// behaviour.
function attachToEmptyPaneOrMarkReady(name) {
  const emptyIdx = panes.slice(0, paneCount).findIndex(p => !p.sandbox);
  if (emptyIdx !== -1) {
    connectPane(emptyIdx, name, 'claude');
    return;
  }
  if (paneCount < computeMaxPanes()) {
    const newIdx = paneCount;
    setLayout(paneCount + 1);
    connectPane(newIdx, name, 'claude');
    return;
  }
  readySandboxes.add(name);
}

/**
 * Builds (once) the corner card a bring-up reports into.
 *
 * A creation is background work like publishing a port or writing a code map,
 * so it says so where the other long jobs do: a card in the bottom-right stack
 * (`#bg-jobs`, see `openBgJob`), not a row in the sandbox list. The list is for
 * sandboxes that exist — this one does not yet — and one corner for everything
 * running behind you beats a second place to look.
 *
 * Returns the card's state, so a caller that only wants to make sure a card
 * exists — the progress stream, for a bring-up this tab did not start — can
 * take the existing one instead of restarting its animation.
 */
function addPendingSandbox(name) {
  const existing = pendingCreations.get(name);
  if (existing) return existing;

  const { id, card, body } = openBgJob('bg-job-rich sbx-creating');
  body.innerHTML = `<div class="bg-job-label">${escHtml(name)}</div>`
    + `<div class="sbx-creating-status">Creating…</div>`;
  // Empty until the daemon says what the steps are, and that is the honest
  // state: a bring-up is one `sbx` call away from starting and this tab is not
  // the one that knows what it will do.
  //
  // Hung off the card rather than the body: the logo and the two header lines
  // are one row, and the list wraps under both of them at the card's left edge
  // instead of into the narrow column beside the logo.
  const steps = document.createElement('ol');
  steps.className = 'sbx-pending-steps';
  steps.hidden = true;
  card.appendChild(steps);

  const pending = {
    /// The corner card's id, for `finishBgJob` when the bring-up ends.
    job: id,
    statusEl: body.querySelector('.sbx-creating-status'),
    stepsEl: steps,
    /// The plan, as announced up front: `[{ id, label }]`.
    steps: [],
    /// Index into `steps` of the one running now; -1 before the first starts.
    at: -1,
    /// The last line `sbx` printed under it. Latest wins — a status line, not
    /// a log — and cleared when the next step starts.
    detail: '',
  };
  pendingCreations.set(name, pending);
  // Only for the sidebar's empty state, which reads differently while a
  // bring-up is running (see `renderSidebar`).
  renderSidebar();
  return pending;
}

function removePendingSandbox(name) {
  const p = pendingCreations.get(name);
  if (!p) return;
  finishBgJob(p.job);
  pendingCreations.delete(name);
  renderSidebar();
}

// ── What a bring-up is doing, while it does it ───────────────────────────
//
// A sandbox that has to pull its image takes minutes, and a card saying
// "Creating…" the whole way through cannot be told from a wedged one. The
// daemon reports each step as it starts and forwards what `sbx` prints
// underneath (see src/progress.rs); this paints it.
//
// The plan is drawn whole, unstarted steps included — a slow step then reads
// as *one of seven with five still to come* rather than as a list that has
// stopped moving.
function renderPendingSteps(p) {
  p.stepsEl.hidden = p.steps.length === 0;
  p.stepsEl.innerHTML = p.steps.map((s, i) => {
    const state = i < p.at ? 'done' : i === p.at ? 'running' : 'waiting';
    // Only under the running step: a detail is what a command is printing
    // right now, and the finished steps' last lines would be a log of a
    // bring-up nobody asked to read.
    const detail = state === 'running' && p.detail
      ? `<div class="sbx-step-detail" title="${escHtml(p.detail)}">${escHtml(p.detail)}</div>`
      : '';
    return `<li class="sbx-step ${state}">
      <span class="sbx-step-mark" aria-hidden="true"></span>
      <span class="sbx-step-label">${escHtml(s.label)}</span>${detail}
    </li>`;
  }).join('');
  // The card's own line counts what the list under it shows, so the state is
  // still readable at a glance without reading seven rows.
  p.statusEl.textContent = p.at >= 0
    ? `Creating… ${p.at + 1}/${p.steps.length}`
    : 'Creating…';
}

/**
 * Apply one `provision` event from `/api/stream`.
 *
 * A card is created on demand rather than assumed: the tab that clicked Create
 * already has one, but a tab reloaded mid-bring-up (exactly what you do when a
 * download seems stuck) has nothing, and the stream is enough to rebuild it.
 */
function applyProvisionEvent(ev) {
  if (!ev || !ev.sandbox) return;
  if (ev.kind === 'done') {
    removePendingSandbox(ev.sandbox);
    return;
  }
  const p = addPendingSandbox(ev.sandbox);
  if (ev.kind === 'plan') {
    p.steps = Array.isArray(ev.steps) ? ev.steps : [];
    p.at = -1;
    p.detail = '';
  } else if (ev.kind === 'started') {
    // An id with no row — an older page against a newer daemon — leaves the
    // list on the last step it does know, which is still true of the bring-up:
    // that step has started and has not been said to finish.
    const idx = p.steps.findIndex(s => s.id === ev.id);
    if (idx >= 0) p.at = idx;
    p.detail = '';
  } else if (ev.kind === 'detail') {
    p.detail = ev.text || '';
  } else {
    return;
  }
  renderPendingSteps(p);
}

// ── Code map runs ─────────────────────────────────────────────────────────
//
// A code map — and a lens on it — is written by the sandbox's own agent, in a
// background session with no pane to watch, and it takes minutes (see
// src/codemap.rs). The daemon holds that state per sandbox and streams every
// change over `/api/stream`'s `codemap` event; this is the tab's copy of it.
//
// One run per sandbox whatever it is writing, which is the daemon's rule and
// not this file's: a lens is written *from* the map, so the two never overlap.
// Everything here therefore keys on the sandbox and reads `run.kind` for the
// words — "map" and "lens" are different documents and calling one the other
// in a toast is how you go looking in the wrong directory.
//
// State rather than a promise, deliberately: the run belongs to the daemon and
// not to the click that started it. A tab opened halfway through seeds itself
// from `/api/codemap` and shows exactly what the tab that clicked shows — a
// badge on the sandbox's row, and one of the corner cards long jobs already
// use.
const codemapRuns = new Map();   // sandbox -> { phase, started, ended, note }
const codemapCards = new Map();  // sandbox -> corner-card id, while it writes

function codemapRun(name) { return codemapRuns.get(name) || null; }

// What a run is writing, named for the sentences the UI puts it in. `kind` is
// absent on a run from an older daemon, which only ever wrote maps.
const isLensRun = (run) => run?.kind === 'lens';
const codemapWhat = (run) => (isLensRun(run) ? 'lens' : 'code map');

/**
 * Apply one run — from the SSE event, or from a snapshot.
 *
 * `seed` marks a snapshot: those runs are already true rather than freshly
 * reached, so reloading the tab must not announce a map written an hour ago.
 *
 * An unchanged run returns early, which is what keeps the project panel from
 * chasing its own tail: it hands its `run` back here when it reloads, and it
 * reloads because of what arrives here.
 */
function applyCodemapRun(run, seed = false) {
  if (!run || !run.sandbox) return;
  const name = run.sandbox;
  const prev = codemapRuns.get(name);
  if (prev && prev.phase === run.phase && prev.started === run.started
      && prev.ended === run.ended) return;
  codemapRuns.set(name, run);

  if (run.phase === 'writing') {
    if (!codemapCards.has(name)) {
      const label = isLensRun(run) ? `Lens — ${name}` : `Code map — ${name}`;
      codemapCards.set(name, startBgJob(label));
    }
  } else {
    finishBgJob(codemapCards.get(name));
    codemapCards.delete(name);
    // Only for a run this tab watched start: a finished one seeded on load is
    // history, and the panel's own button is where it belongs.
    if (!seed && prev?.phase === 'writing') {
      const what = codemapWhat(run);
      if (run.phase === 'done') {
        showToast(isLensRun(run)
          ? `Lens written in “${name}” — it is in the Code map panel's picker`
          : `Code map written in “${name}”`, 'ok');
      } else {
        showToast(`Could not write the ${what} in “${name}”: ${run.note || 'the session ended without one'}`, 'error');
      }
    }
  }
  renderSidebar();
  refreshFilesModal(name);
  // The code map panel, when it is the one open: a lens landing is a document
  // appearing in the picker it is showing (see `applyRun` in /js/codemap.js).
  if (typeof codemapViewerRun === 'function') codemapViewerRun(run, seed);
}

// ── Sidebar ───────────────────────────────────────────────────────────────
function dotClass(status) {
  if (status === 'running') return 'running';
  if (status === 'stopped') return 'stopped';
  return 'error';
}

function workspaceBasename(p) {
  const parts = p.split(/[\\/]+/).filter(Boolean);
  return parts.length ? parts[parts.length - 1] : p;
}

// Stable hash → hue, so the same workspace path always gets the same accent
// color across renders/sessions without needing a fixed palette table.
function workspaceGroupColor(key) {
  let hash = 0;
  for (let i = 0; i < key.length; i++) hash = (hash * 31 + key.charCodeAt(i)) >>> 0;
  return `hsl(${hash % 360}, 55%, 62%)`;
}

// Chat sandboxes each get their own throwaway workspace, so path grouping can
// never bring them together — they group on the API's `chat` flag instead,
// under a sentinel key no real path can collide with, and keep a fixed accent
// (they're one recognisable kind, not one folder among many).
const CHAT_GROUP = ' chat';
const CHAT_GROUP_COLOR = 'hsl(265, 60%, 68%)';

function groupKeyOf(s) {
  if (s.chat) return CHAT_GROUP;
  return s.workspace || null;
}

function groupColorOf(key) {
  return key === CHAT_GROUP ? CHAT_GROUP_COLOR : workspaceGroupColor(key);
}

// Reorders sandboxes into three tiers, alphabetical within each: the chat
// group first (it always forms, even with a single chat sandbox, and always
// sorts to the very top so it's never buried under older workspaces), then
// every other group — sorted by its label — and finally ungrouped sandboxes
// on their own, sorted by name. Grouped sandboxes never interleave with
// ungrouped ones this way: all the group cards sit together as one block,
// with any singles trailing after rather than wedged between them.
function orderForGrouping(list, groupCounts) {
  const keyOf = s => {
    const k = groupKeyOf(s);
    if (k === CHAT_GROUP) return k;
    return (k && groupCounts.get(k) > 1) ? k : null;
  };
  const tierOf = s => {
    const k = keyOf(s);
    if (k === CHAT_GROUP) return 0;
    if (k) return 1;
    return 2;
  };
  const sortLabel = s => {
    const k = keyOf(s);
    if (k && k !== CHAT_GROUP) return workspaceBasename(k).toLowerCase();
    return s.name.toLowerCase();
  };
  return list.slice().sort((a, b) => {
    const tierCmp = tierOf(a) - tierOf(b);
    if (tierCmp !== 0) return tierCmp;
    return sortLabel(a).localeCompare(sortLabel(b)) || a.name.localeCompare(b.name);
  });
}

function sandboxItemHtml(s, groupColor) {
  // In multi-pane mode show which pane(s) are connected to this sandbox.
  const connPanes = panes.filter((p, i) => i < paneCount && p.sandbox === s.name);
  const paneTag = paneCount > 1 && connPanes.length
    ? connPanes.map(p => `<span class="sbx-pane-badge">P${p.index + 1}</span>`).join('')
    : '';
  const isActive = connPanes.length > 0;
  const isRunning = s.status === 'running';
  const readyDot = readySandboxes.has(s.name) ? '<span class="sbx-ready-dot" title="Ready"></span>' : '';
  // A map being written is the one thing an agent does *for* a sandbox that
  // its own liseret cannot speak for: the session doing it is not the one in
  // the pane, and there is no pane to light up.
  const mapRun = codemapRun(s.name);
  const mapBadge = mapRun?.phase === 'writing'
    ? `<span class="sbx-map-badge" title="Writing the ${codemapWhat(mapRun)} in a background session">`
      + `${isLensRun(mapRun) ? 'lens' : 'map'}…</span>`
    : '';
  const dupBtn = `<button class="icon-btn dup-btn" data-action="duplicate" data-name="${s.name}" title="Duplicate (new sandbox, same workspace)">⧉</button>`;
  const actions = isRunning
    ? `<button class="stop-btn" data-action="stop" data-name="${s.name}">■ Stop</button>
       <button class="icon-btn restart-btn" data-action="restart" data-name="${s.name}" title="Reload">↺</button>
       <button class="icon-btn ports-btn" data-action="ports" data-name="${s.name}" title="Ports &amp; network policy">⇌</button>
       ${dupBtn}
       <button class="icon-btn rm-btn" data-action="rm" data-name="${s.name}" title="Remove permanently">✕</button>`
    : `<button class="connect-btn" data-action="connect" data-name="${s.name}">▶ Connect</button>
       <button class="icon-btn ports-btn" data-action="ports" data-name="${s.name}" title="Ports &amp; network policy">⇌</button>
       ${dupBtn}
       <button class="icon-btn rm-btn" data-action="rm" data-name="${s.name}" title="Remove permanently">✕</button>`;
  // Active state is on border-right and the group colour on border-left, so
  // a row can carry both. Ungrouped rows still get a left border — just a
  // neutral grey (the same one buttons use), so the sidebar's left edge
  // reads consistently whether or not a row belongs to a group.
  const borderStyle = ` style="border-left-color:${groupColor || '#c9d1d9'}"`;
  return `<div class="sbx-item${isActive ? ' active' : ''}" data-name="${s.name}"${borderStyle}>
    <div class="sbx-name">${s.name}${paneTag}${readyDot}${mapBadge}</div>
    <div class="sbx-meta">
      <span class="dot ${dotClass(s.status)}"></span>
      <span class="sbx-status">${s.status}</span>
      <span class="sbx-agent">${s.agent}</span>
    </div>
    <div class="sbx-actions">${actions}</div>
  </div>`;
}

function renderSidebar() {
  if (!sandboxes.length) {
    // Nothing in the list *while a bring-up is running* is not the ambiguous
    // case below — it is the expected one, and the corner card already says
    // what is happening. Offering the "can't see my sandboxes" help here would
    // send someone troubleshooting a machine that is working.
    if (pendingCreations.size) {
      listEl.innerHTML = `<div class="sbx-empty">
        <div class="sbx-empty-msg">Creating your first sandbox…</div>
      </div>`;
      return;
    }
    // An empty list is otherwise ambiguous — you own no sandboxes, or `sbx`
    // can't see the ones you do. The second case is the common one and is fixed
    // on the host, so point at the help dialog that spells out how
    // (/js/help.js).
    listEl.innerHTML = `<div class="sbx-empty">
      <div class="sbx-empty-msg">No sandboxes</div>
      <button class="sbx-empty-help" data-help-open type="button">Expected some? →</button>
    </div>`;
    return;
  }

  // Only sandboxes sharing a group key with at least one other — an identical
  // workspace path, or being a chat sandbox — get grouped. A lone sandbox on a
  // unique workspace renders exactly as before.
  const groupCounts = new Map();
  sandboxes.forEach(s => {
    const k = groupKeyOf(s);
    if (k) groupCounts.set(k, (groupCounts.get(k) || 0) + 1);
  });
  const ordered = orderForGrouping(sandboxes, groupCounts);

  let html = '';
  let openGroup = null;
  ordered.forEach(s => {
    const key = groupKeyOf(s);
    const groupKey = (key === CHAT_GROUP || (key && groupCounts.get(key) > 1)) ? key : null;
    if (groupKey !== openGroup) {
      if (openGroup !== null) html += '</div>';
      if (groupKey !== null) {
        const isChat = groupKey === CHAT_GROUP;
        const title = isChat ? 'Chat sandboxes — each on its own empty workspace' : groupKey;
        const label = isChat ? '💬 Chats' : `📁 ${workspaceBasename(groupKey)}`;
        html += `<div class="sbx-group${isChat ? ' sbx-group-chat' : ''}" style="--group-color:${groupColorOf(groupKey)}">
          <div class="sbx-group-label" title="${title}">
            <span class="sbx-group-dot"></span>${label}
          </div>`;
      }
      openGroup = groupKey;
    }
    html += sandboxItemHtml(s, groupKey ? groupColorOf(groupKey) : null);
  });
  if (openGroup !== null) html += '</div>';

  listEl.innerHTML = html;
  // The rows above were just rebuilt from a string and carry no agent classes;
  // repaint them from the live session state rather than baking it into the
  // HTML, so an event arriving between two renders still lands.
  applyAgentStates();
}

// Clicking a sandbox item (not a button) connects it to the focused pane.
listEl.addEventListener('click', e => {
  const btn = e.target.closest('button[data-action]');
  const item = e.target.closest('.sbx-item');
  if (!item) return;
  const name = item.dataset.name;
  // Any interaction with a just-finished sandbox counts as "seen".
  if (readySandboxes.delete(name)) renderSidebar();
  if (btn) {
    e.stopPropagation();
    const action = btn.dataset.action;
    if (action === 'stop')    stopSandbox(name);
    if (action === 'restart') restartSandbox(name);
    if (action === 'connect') connectPane(focusedPane, name, 'claude');
    if (action === 'rm')        openRmModal(name);
    if (action === 'ports')     openPortsModal(name);
    if (action === 'duplicate') openDupModal(name);
  } else {
    connectPane(focusedPane, name);
  }
});

// ── API calls ─────────────────────────────────────────────────────────────
async function loadSandboxes() {
  try {
    const res = await fetch('/api/sandboxes');
    sandboxes = await res.json();
    renderSidebar();
  } catch (_) {}
}

async function stopSandbox(name) {
  const s = sandboxes.find(x => x.name === name);
  if (s) s.status = 'stopping…';
  renderSidebar();
  panes.forEach(p => { if (p.sandbox === name && p.ws) { try { p.ws.close(); } catch(_){} } });
  await fetch(`/api/sandboxes/${encodeURIComponent(name)}/stop`, { method: 'POST' });
  setTimeout(loadSandboxes, 1200);
}

async function restartSandbox(name) {
  await stopSandbox(name);
  setTimeout(() => {
    panes.forEach((p, i) => { if (i < paneCount && p.sandbox === name) connectPane(i, name, 'claude'); });
  }, 1600);
}

// ── Buttons ───────────────────────────────────────────────────────────────
document.getElementById('btn-refresh').addEventListener('click', loadSandboxes);

// Pane-count popover: toggled by its trigger button, closed on an outside
// click, an Escape press, or once a count is picked (see renderLayoutSwitch).
document.getElementById('layout-switch-btn').addEventListener('click', e => {
  e.stopPropagation();
  const panel = document.getElementById('layout-switch-panel');
  const trigger = document.getElementById('layout-switch-btn');
  const willOpen = panel.hidden;
  panel.hidden = !willOpen;
  trigger.classList.toggle('open', willOpen);
  trigger.setAttribute('aria-expanded', String(willOpen));
});
document.addEventListener('click', e => {
  if (!document.getElementById('layout-switch').contains(e.target)) closeLayoutSwitchPanel();
});
document.addEventListener('keydown', e => {
  if (e.key === 'Escape') closeLayoutSwitchPanel();
});

let resizeDebounce = null;
window.addEventListener('resize', () => {
  panes.slice(0, paneCount).forEach(p => { p.fit.fit(); sendPaneResize(p); });
  clearTimeout(resizeDebounce);
  resizeDebounce = setTimeout(updateLayoutSwitch, 150);
});

// ── Copy: Cmd+C (macOS) or Ctrl+Shift+C — copies focused pane's selection ─
document.addEventListener('keydown', e => {
  const isCopy = (e.metaKey && !e.altKey && e.key === 'c') ||
                 (e.ctrlKey && e.shiftKey && e.key === 'C');
  if (!isCopy) return;
  const fp = panes[focusedPane];
  if (!fp) return;
  const sel = fp.term.getSelection() || fp.getLastSelection() || window.getSelection()?.toString() || '';
  if (!sel) return;
  e.preventDefault();
  e.stopImmediatePropagation();
  if (!navigator.clipboard) {
    showToast('Clipboard unavailable — needs HTTPS or localhost', 'error');
    fp.term.focus();
    return;
  }
  navigator.clipboard.writeText(sel)
    .then(() => showToast('Copied', 'ok'))
    .catch(() => showToast('Copy failed', 'error'))
    .finally(() => fp.term.focus());
}, true);

// ── Paste image → upload to focused pane's sandbox ────────────────────────
async function uploadPastedImage(file, pane) {
  if (!pane.sandbox || !pane.ws || pane.ws.readyState !== WebSocket.OPEN) {
    showToast('No sandbox attached — connect one first', 'error');
    return;
  }
  showToast('Uploading image…', 'progress');
  try {
    const res = await fetch(
      `/api/sandboxes/${encodeURIComponent(pane.sandbox)}/paste-image`,
      { method: 'POST', headers: { 'Content-Type': file.type || 'image/png' }, body: file }
    );
    const data = await res.json();
    if (data.ok && data.path) {
      pane.ws.send(new TextEncoder().encode(data.path + ' '));
      pane.term.focus();
      showToast('📎 ' + data.path, 'ok');
    } else {
      showToast('Upload failed: ' + (data.error || 'unknown error'), 'error');
    }
  } catch (_) {
    showToast('Upload error — is the sandbox running?', 'error');
  }
}

document.addEventListener('paste', e => {
  const items = (e.clipboardData && e.clipboardData.items) || [];
  let file = null;
  for (const it of items) {
    if (it.kind === 'file' && it.type.startsWith('image/')) { file = it.getAsFile(); break; }
  }
  if (!file) return;
  e.preventDefault();
  e.stopPropagation();
  uploadPastedImage(file, panes[focusedPane] || panes[0]);
}, true);
