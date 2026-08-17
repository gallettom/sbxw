// ── Cross-sandbox information requests ────────────────────────────────────
//
// An agent in one sandbox asked for something it cannot see from where it is.
// This is the human's half of that: the question arrives here, you choose which
// other sandbox (if any) is asked, you read what comes back, and nothing
// reaches the asker until you say so. See `src/relay.rs` for the state machine
// and `assets/relay-tool.js` for the CLI the agents run.
//
// The popup is the *only* place any of this is visible, so it is deliberately
// interrupting: a request that nobody sees is an agent blocked on a person who
// never knew. Dismissing it is always "later", never "no" — refusing is a
// button of its own, because an agent that is refused stops asking while one
// that is ignored comes back.

const relayRequests = new Map(); // id -> request, newest state wins
const relayOverlay = document.getElementById('relay-modal-overlay');
const relayBodyEl = document.getElementById('relay-modal-body');
const relayFootEl = document.getElementById('relay-modal-foot');
const relayQueueEl = document.getElementById('relay-queue');
const relayBadgeEl = document.getElementById('relay-badge');

// Which request is on screen. Sticky: new arrivals queue behind whatever you
// are already deciding on rather than swapping the buttons under your cursor.
let relayCurrentId = null;
// Set while the popup shows the outcome of a request that just settled, so the
// tab doesn't jump straight to the next one before you've read it.
let relayLingerTimer = null;
// The human is typing an answer (or an edit of one) — re-rendering would throw
// it away, so state updates for the shown request are held back.
let relayEditing = false;

// Everything the popup shows was written by an agent or names a sandbox, so
// none of it is interpolated raw. `escHtml` comes from util.js.
const relayEsc = escHtml;

/** Requests still waiting on a human, oldest first. */
function relayOpen() {
  return [...relayRequests.values()]
    .filter(r => r.state !== 'approved' && r.state !== 'denied')
    .sort((a, b) => a.created_ms - b.created_ms);
}

/**
 * Requests that have nothing to ask you *right now* — they are out with another
 * sandbox, whose agent is thinking. These shrink to a corner card instead of
 * holding the screen, so the wait (which can run minutes) is spent in another
 * sandbox rather than in front of a modal with nothing to do in it.
 *
 * Derived from the state rather than tracked: `routed` *is* "waiting on a
 * sandbox", so nothing can drift out of sync with the server, and an answer
 * arriving un-docks the request by definition.
 */
function relayDocked() {
  return relayOpen().filter(r => r.state === 'routed');
}

/** Open requests that want a decision from you — the popup's queue. */
function relayQueue() {
  return relayOpen().filter(r => r.state !== 'routed');
}

/** The request the popup should be showing, if any. */
function relayCurrent() {
  // An explicit pick wins even if it is docked: clicking its corner card is
  // how you go back to a waiting request to re-route or refuse it.
  if (relayCurrentId && relayRequests.has(relayCurrentId)) return relayRequests.get(relayCurrentId);
  return relayQueue()[0] || null;
}

// ── Rendering ─────────────────────────────────────────────────────────────

/** Sandboxes that could answer `req`: running, and not the one that asked. */
function relayCandidates(req) {
  return sandboxes.filter(s => s.status === 'running' && s.name !== req.from);
}

function relayTargetButtons(req, label) {
  const candidates = relayCandidates(req);
  if (!candidates.length) {
    return `<p class="relay-empty">No other sandbox is running. Start one from the sidebar, or
            answer this yourself below.</p>`;
  }
  const buttons = candidates.map(s => `
    <button class="relay-target" data-relay-to="${relayEsc(s.name)}">
      <span class="relay-target-dot"${s.chat ? ' data-chat="1"' : ''}></span>
      ${relayEsc(s.name)}
    </button>`).join('');
  return `<div class="relay-section-label">${label}</div>
          <div class="relay-targets">${buttons}</div>`;
}

/** The question, always presented as someone else's words rather than as ours. */
function relayQuestionBlock(req) {
  return `
    <div class="relay-from">
      <span class="relay-badge-sbx">${relayEsc(req.from)}</span>
      <span class="relay-from-text">asks — via you — for information it can't see from its own workspace.</span>
    </div>
    <pre class="relay-quote">${relayEsc(req.question)}</pre>`;
}

/**
 * Sync the corner cards for docked requests.
 *
 * They live in the same bottom-right stack as the background-job indicators
 * (`#bg-jobs`): a routed question *is* background work, and one corner with one
 * kind of card beats two stacks fighting over the same 200 pixels. Cards are
 * reconciled rather than rebuilt, so the entry animation plays once per
 * request instead of on every SSE update.
 */
function relayRenderDocks() {
  const shown = relayOverlay.classList.contains('hidden') ? null : relayCurrentId;
  // Whatever is on screen in the popup doesn't also need a card for itself.
  const docked = relayDocked().filter(r => r.id !== shown);
  const wanted = new Set(docked.map(r => r.id));

  bgJobsEl.querySelectorAll('[data-relay-dock]').forEach(el => {
    if (!wanted.has(el.dataset.relayDock)) el.remove();
  });

  docked.forEach(req => {
    let card = bgJobsEl.querySelector(`[data-relay-dock="${CSS.escape(req.id)}"]`);
    if (!card) {
      card = document.createElement('div');
      card.className = 'bg-job-card relay-dock';
      card.dataset.relayDock = req.id;
      card.title = 'Open this request';
      bgJobsEl.appendChild(card);
    }
    card.innerHTML = `
      <span class="relay-spinner"></span>
      <div class="relay-dock-text">
        <div class="relay-dock-title">${relayEsc(req.to)} is answering</div>
        <div class="relay-dock-sub">for ${relayEsc(req.from)} · ${relayEsc(relayGist(req.question))}</div>
      </div>`;
  });
}

/** First line of a question, short enough for a corner card. */
function relayGist(question) {
  const line = String(question || '').split('\n')[0].trim();
  return line.length > 44 ? line.slice(0, 44) + '…' : line;
}

// A card is the way back into the request it stands for.
bgJobsEl.addEventListener('click', ev => {
  const id = ev.target.closest('[data-relay-dock]')?.dataset.relayDock;
  if (id) relayShow(id);
});

/**
 * The header badge: the way back in after dismissing the popup.
 *
 * Counts only what is waiting on *you* — a docked request already has a card of
 * its own down in the corner, and counting it in both places would read as two
 * requests. Separate from `relayRender` because closing the popup changes the
 * count without there being anything left to render.
 */
function relayRenderBadge() {
  const queue = relayQueue();
  relayBadgeEl.hidden = queue.length === 0;
  relayBadgeEl.textContent = queue.length === 1
    ? '1 sandbox request'
    : `${queue.length} sandbox requests`;
}

function relayRender() {
  const req = relayCurrent();
  const queue = relayQueue();

  relayRenderDocks();
  relayRenderBadge();

  if (!req) { relayClose(); return; }
  relayCurrentId = req.id;

  const others = queue.filter(r => r.id !== req.id).length;
  relayQueueEl.hidden = others === 0;
  relayQueueEl.textContent = others === 1 ? '1 more waiting' : `${others} more waiting`;

  let body = relayQuestionBlock(req);
  let foot = '';

  if (req.state === 'pending') {
    if (req.note) body += `<p class="relay-note">${relayEsc(req.note)}</p>`;
    body += relayTargetButtons(req, 'Ask one of these sandboxes');
    body += `
      <div class="relay-section-label relay-or">or answer it yourself</div>
      <textarea class="relay-answer" id="relay-own-answer" rows="3"
                placeholder="Type the answer here — it goes straight to ${relayEsc(req.from)}."></textarea>`;
    foot = `
      <button class="btn-cancel" data-relay-action="dismiss">Later</button>
      <button class="relay-deny" data-relay-action="deny">Refuse</button>
      <button class="btn-create" data-relay-action="approve-own" disabled>Send my answer</button>`;
  } else if (req.state === 'routed') {
    body += `
      <div class="relay-waiting">
        <span class="relay-spinner"></span>
        Sent to <strong>${relayEsc(req.to)}</strong> — waiting for its agent to answer.
        Nothing reaches ${relayEsc(req.from)} until you approve it.
      </div>`;
    body += relayTargetButtons(req, 'Or ask someone else instead');
    foot = `
      <button class="btn-cancel" data-relay-action="dismiss">Later</button>
      <button class="relay-deny" data-relay-action="deny">Refuse</button>`;
  } else if (req.state === 'answered') {
    body += `
      <div class="relay-section-label">
        ${relayEsc(req.to)} answered — edit freely, this is what ${relayEsc(req.from)} will receive
      </div>
      <textarea class="relay-answer" id="relay-review-answer" rows="8">${relayEsc(req.answer || '')}</textarea>`;
    foot = `
      <button class="btn-cancel" data-relay-action="dismiss">Later</button>
      <button class="relay-deny" data-relay-action="deny">Refuse</button>
      <button class="btn-create" data-relay-action="approve">✓ Send to ${relayEsc(req.from)}</button>`;
  } else {
    // Settled — held on screen for a beat so the outcome is legible.
    body += req.state === 'approved'
      ? `<div class="relay-settled ok">Sent to <strong>${relayEsc(req.from)}</strong>.</div>`
      : `<div class="relay-settled no">Refused. Nothing was shared with
         <strong>${relayEsc(req.from)}</strong>.</div>`;
    foot = `<button class="btn-cancel" data-relay-action="dismiss">Close</button>`;
  }

  relayBodyEl.innerHTML = body;
  relayFootEl.innerHTML = foot;

  // "Send my answer" only means something once there is one.
  const ownAnswer = document.getElementById('relay-own-answer');
  if (ownAnswer) {
    const sendBtn = relayFootEl.querySelector('[data-relay-action="approve-own"]');
    const sync = () => {
      relayEditing = ownAnswer.value.trim().length > 0;
      sendBtn.disabled = !relayEditing;
    };
    ownAnswer.addEventListener('input', sync);
    sync();
  }
  const review = document.getElementById('relay-review-answer');
  if (review) review.addEventListener('input', () => { relayEditing = true; });
}

// ── Open / close ──────────────────────────────────────────────────────────

function relayShow(id) {
  clearTimeout(relayLingerTimer);
  if (id) relayCurrentId = id;
  relayEditing = false;
  relayOverlay.classList.remove('hidden');
  relayRender();
  // The target buttons are built from the sandbox list, which is only refreshed
  // on explicit actions — a sandbox started since the last refresh would
  // otherwise be missing from exactly the moment it matters.
  loadSandboxes().then(relayRender).catch(() => {});
}

function relayClose() {
  relayOverlay.classList.add('hidden');
  relayCurrentId = null;
  relayEditing = false;
  // A request that was only hidden because the popup was showing it gets its
  // corner card back on the way out, and the badge re-counts what is left.
  relayRenderDocks();
  relayRenderBadge();
}

/** Done with this one: show the next request that needs a decision, or close. */
function relayAdvance() {
  relayCurrentId = null;
  relayEditing = false;
  const next = relayQueue()[0];
  if (next) relayShow(next.id);
  else relayClose();
}

// ── Actions ───────────────────────────────────────────────────────────────

async function relayPost(path, body) {
  try {
    const res = await fetch(path, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body || {}),
    });
    const data = await res.json();
    if (!data.ok) showToast(data.error || 'sbxw refused that', 'error');
    return data.ok === true;
  } catch (_) {
    showToast('Could not reach sbxw', 'error');
    return false;
  }
}

relayOverlay.addEventListener('click', async ev => {
  const req = relayCurrent();
  if (!req) return;

  const target = ev.target.closest('[data-relay-to]');
  if (target) {
    relayEditing = false;
    await relayPost(`/api/relay/${encodeURIComponent(req.id)}/route`, { to: target.dataset.relayTo });
    return;
  }

  const action = ev.target.closest('[data-relay-action]')?.dataset.relayAction;
  if (!action) return;

  if (action === 'dismiss') {
    // Explicitly *not* an answer: the request stays open, the header badge
    // keeps it findable, and the asking agent goes on waiting.
    relayClose();
    relayRender();
  } else if (action === 'deny') {
    if (await relayPost(`/api/relay/${encodeURIComponent(req.id)}/deny`, {})) relayEditing = false;
  } else if (action === 'approve' || action === 'approve-own') {
    const box = document.getElementById(
      action === 'approve' ? 'relay-review-answer' : 'relay-own-answer');
    const answer = (box?.value || '').trim();
    if (!answer) { showToast('Nothing to send', 'error'); return; }
    if (await relayPost(`/api/relay/${encodeURIComponent(req.id)}/approve`, { answer })) {
      relayEditing = false;
    }
  }
});

document.getElementById('relay-modal-close').addEventListener('click', () => {
  relayClose();
  relayRender();
});
relayBadgeEl.addEventListener('click', () => relayShow());

// Escape dismisses, like every other modal here — and, like the ✕, it means
// "later". Only when the popup is actually up, so it doesn't swallow the key
// from the terminal.
document.addEventListener('keydown', ev => {
  if (ev.key !== 'Escape' || relayOverlay.classList.contains('hidden')) return;
  relayClose();
  relayRender();
});

// ── Ingest ────────────────────────────────────────────────────────────────

// How long a settled request stays on screen before the popup moves on.
const RELAY_LINGER_MS = 2200;

function relayIngest(req, seed = false) {
  if (!req || !req.id) return;
  const prev = relayRequests.get(req.id);
  relayRequests.set(req.id, req);

  const settled = req.state === 'approved' || req.state === 'denied';
  if (settled) {
    if (relayCurrentId === req.id) {
      relayRender();
      clearTimeout(relayLingerTimer);
      relayLingerTimer = setTimeout(() => { relayRequests.delete(req.id); relayAdvance(); },
        RELAY_LINGER_MS);
    } else {
      relayRequests.delete(req.id);
      relayRender();
    }
    return;
  }

  // Just sent out to a sandbox: get out of the way. The wait is on an agent
  // now, not on you, and it can run for minutes — so the request shrinks to a
  // corner card and the screen goes back to whatever you were doing. If another
  // question is queued behind it, that one takes the popup instead of closing.
  if (req.state === 'routed' && prev?.state !== 'routed' && relayCurrentId === req.id) {
    relayAdvance();
    return;
  }

  // The other side of that: a docked request wants you again — its answer came
  // back, or delivery failed and it fell back to `pending`. Raise it, but never
  // over a decision already in progress; that one keeps the popup and this
  // shows up in the "N more waiting" count.
  const wantsYouBack = prev?.state === 'routed' && req.state !== 'routed';
  if (wantsYouBack && !seed) {
    if (relayOverlay.classList.contains('hidden')) relayShow(req.id);
    else relayRender();
    return;
  }

  // A question nobody has seen yet raises the popup. Everything else only
  // repaints — including an answer arriving in the popup you are already
  // looking at, rather than stealing focus from another one.
  const isNew = !prev && req.state === 'pending';
  if (isNew && !seed) {
    if (relayOverlay.classList.contains('hidden')) relayShow(req.id);
    else relayRender();
    return;
  }
  // Don't repaint a textarea out from under someone mid-sentence.
  if (relayEditing && relayCurrentId === req.id) return;
  relayRender();
}

fetch('/api/relay')
  .then(r => r.json())
  .then(d => {
    (d.requests || []).forEach(r => relayIngest(r, true));
    // A tab opened (or reloaded) while requests were already waiting shows them
    // straight away: the agent behind each one is blocked either way. Ones
    // already out with a sandbox only get their corner card back — the tab
    // reloading is no reason to interrupt you on their behalf.
    if (relayQueue().length) relayShow();
    else relayRenderDocks();
  })
  .catch(() => {});

sbxwStream.addEventListener('relay', ev => {
  try { relayIngest(JSON.parse(ev.data)); } catch (_) {}
});
