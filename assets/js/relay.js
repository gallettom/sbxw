// ── Requests an agent cannot settle on its own ────────────────────────────
//
// An agent in one sandbox asked for something it cannot get to from where it
// is. This is the human's half of that, and it comes in two shapes:
//
//  - a **question** about another sandbox's workspace — you choose which other
//    sandbox (if any) is asked, you read what comes back, and nothing reaches
//    the asker until you say so;
//  - a **screenshot** of what is on your screen — nobody else can supply that,
//    so there is no routing step at all: you attach one image or several,
//    describe it in words instead, or refuse.
//
// See `src/relay.rs` for the state machine and `assets/relay-tool.js` for the
// CLI the agents run.
//
// The popup is the *only* place any of this is visible, so it is deliberately
// interrupting: a request that nobody sees is an agent blocked on a person who
// never knew. Dismissing it is always "later", never "no" — refusing is a
// button of its own, because an agent that is refused stops asking while one
// that is ignored comes back.

const relayRequests = new Map(); // id -> request, newest state wins
const relayOverlay = document.getElementById('relay-modal-overlay');
const relayTitleEl = document.getElementById('relay-modal-title');
const relayBodyEl = document.getElementById('relay-modal-body');
const relayFootEl = document.getElementById('relay-modal-foot');
const relayQueueEl = document.getElementById('relay-queue');
const relayBadgeEl = document.getElementById('relay-badge');

// Images attached but not yet sent, by request id — a list per request, in the
// order they were attached, which is the order the agent receives them in.
//
// Kept here rather than on the request itself because the server does not know
// they exist: attaching is a local act, and it stays local until "Send" turns
// it into an approval. Which is also what makes building the set free — you can
// paste, look, add the after-shot, drop the one that came out blurry, and
// nothing has left the tab until you say so.
const relayShots = new Map(); // id -> [{ url, label }]

// Longest edge an attached image is scaled to before it is sent. A screenshot
// is read for its layout, not its pixel grid, and a retina capture of a 4K
// display is several megabytes of JSON to say the same thing.
const RELAY_SHOT_MAX_EDGE = 1600;

// Ceiling on the base64 that goes over the wire, mirroring `MAX_SHOT_B64` in
// `src/relay.rs`. Enforced on both sides on purpose: here so the person finds
// out while they can still pick a smaller window, there because the daemon
// cannot trust a browser to have done it.
const RELAY_SHOT_MAX_B64 = 6 * 1024 * 1024;

// How many images one reply may carry, and what they may weigh together —
// `MAX_SHOTS` and `MAX_SHOTS_B64` in `src/relay.rs`. Mirrored here for the same
// reason as the per-image cap: refused at the door is a toast you can act on
// while the popup is still open, refused at the daemon is a round trip.
const RELAY_MAX_SHOTS = 6;
const RELAY_SHOTS_MAX_B64 = 16 * 1024 * 1024;

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

/** Is this one of the requests only the person at the keyboard can settle? */
function relayIsShot(req) {
  return req.kind === 'screenshot';
}

/**
 * What was asked, always presented as someone else's words rather than as ours.
 *
 * The line above the quote is the only thing sbxw says in its own voice here,
 * and it does the work of framing: what the agent wants, and why it is you
 * being asked rather than anyone else.
 */
function relayQuestionBlock(req) {
  const why = relayIsShot(req)
    ? `can't see its own screen, and asks — via you — for a look at what it just changed.`
    : `asks — via you — for information it can't see from its own workspace.`;
  return `
    <div class="relay-from">
      <span class="relay-badge-sbx">${relayEsc(req.from)}</span>
      <span class="relay-from-text">${why}</span>
    </div>
    <pre class="relay-quote">${relayEsc(req.question)}</pre>`;
}

/** The images attached to `id` so far, oldest first. Never null. */
function relayHeld(id) {
  return relayShots.get(id) || [];
}

/**
 * The attach-an-image half of a screenshot request.
 *
 * Four ways in, because the natural gesture differs by person and by platform:
 * take the image the OS screenshot key already put on the clipboard, drop a
 * file (or several), pick one, or let the browser capture a window here and
 * now. Each is a button or a target you can see — nothing here depends on
 * knowing that a keystroke would have worked. They all end at the same place
 * (`relayAttach`), and the previews are the confirmation: you send what you can
 * see, never what you hope you copied.
 *
 * Each gesture *adds*, because the answer to "how does it look?" is often more
 * than one picture — a before and an after, two breakpoints, three steps of a
 * flow. The set is numbered on screen in the order the agent will receive it,
 * and each thumbnail carries its own ✕ so a bad capture costs one click rather
 * than starting the set again.
 */
function relayShotSection(req) {
  const held = relayHeld(req.id);
  const room = RELAY_MAX_SHOTS - held.length;
  const grid = held.length
    ? `<div class="relay-shot-grid" data-count="${held.length}">${held.map((shot, i) => `
        <figure class="relay-shot-item">
          <img class="relay-shot-img" src="${shot.url}" alt="">
          <button class="relay-shot-remove" data-relay-action="drop-shot" data-shot="${i}"
                  title="Remove this one">✕</button>
          <figcaption class="relay-shot-meta">${i + 1}. ${relayEsc(shot.label)}</figcaption>
        </figure>`).join('')}</div>`
    : '';
  // The zone stays on screen with images already attached — that is where the
  // next one goes — but shrinks to a strip, since by then the previews are what
  // the space is for.
  const hint = held.length
    ? `<div class="relay-shot-hint">
         <strong>Add another</strong> — drop, paste or click${held.length > 1 ? ` · ${held.length} attached` : ''}
       </div>`
    : `<div class="relay-shot-hint">
         <strong>Drop images here</strong> — or click to choose files
       </div>`;
  const zone = room > 0
    ? `<div class="relay-shot-zone${held.length ? ' compact' : ''}" id="relay-shot-zone"
            title="Drop images, or click to choose files">${hint}</div>`
    : `<p class="relay-note">That is ${RELAY_MAX_SHOTS} images — the most one reply carries.
       Remove one to swap it for another.</p>`;
  return `
    ${grid}
    ${zone}
    <div class="relay-shot-actions">
      ${room > 0 && relayCanPaste() ? `<button class="relay-target" data-relay-action="paste">Paste from clipboard</button>` : ''}
      ${room > 0 && relayCanCapture() ? `<button class="relay-target" data-relay-action="capture">Capture a window…</button>` : ''}
      ${room > 0 ? `<button class="relay-target" data-relay-action="choose">Choose a file…</button>` : ''}
      ${held.length ? `<button class="relay-target" data-relay-action="drop-shots">Remove all</button>` : ''}
    </div>
    <input type="file" id="relay-shot-file" accept="image/png,image/jpeg,image/webp" multiple hidden>`;
}

// Both of the browser-side capture routes are gated on a *secure context*,
// which localhost satisfies and a LAN address (a `web_addr` opened up to the
// network) does not — so their availability is a property of how you reached
// sbxw, not of your browser. Either button is left out rather than shown and
// refused: an offer that cannot be taken up is worse than no offer, and
// dropping a file is always on screen.

/** Whether this tab may read the clipboard on a click. */
function relayCanPaste() {
  return !!(window.isSecureContext && navigator.clipboard && navigator.clipboard.read);
}

/** Whether this tab may capture a window. */
function relayCanCapture() {
  return !!(window.isSecureContext && navigator.mediaDevices
            && navigator.mediaDevices.getDisplayMedia);
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
  relayTitleEl.textContent = relayIsShot(req) ? 'Screenshot request' : 'Information request';

  let body = relayQuestionBlock(req);
  let foot = '';

  const settled = req.state === 'approved' || req.state === 'denied';

  if (relayIsShot(req) && !settled) {
    // `pending` is the only live state a screenshot request has: the server
    // refuses to route one, so there is nothing to wait on and nothing to
    // review — just you, an image, and two buttons.
    if (req.note) body += `<p class="relay-note">${relayEsc(req.note)}</p>`;
    body += relayShotSection(req);
    body += `
      <div class="relay-section-label relay-or">or describe it instead</div>
      <textarea class="relay-answer" id="relay-own-answer" rows="2"
                placeholder="Words work too — say what it looks like, and ${relayEsc(req.from)} carries on with that."></textarea>`;
    foot = `
      <button class="btn-cancel" data-relay-action="dismiss">Later</button>
      <button class="relay-deny" data-relay-action="deny">Refuse</button>
      <button class="btn-create" data-relay-action="approve-own" disabled>Send to ${relayEsc(req.from)}</button>`;
  } else if (req.state === 'pending') {
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

  // "Send" only means something once there is something to send — which, on a
  // screenshot request, an attached image already satisfies with the textarea
  // left empty.
  const ownAnswer = document.getElementById('relay-own-answer');
  if (ownAnswer) {
    const sendBtn = relayFootEl.querySelector('[data-relay-action="approve-own"]');
    const sync = () => {
      const typed = ownAnswer.value.trim().length > 0;
      relayEditing = typed;
      sendBtn.disabled = !typed && !relayHeld(req.id).length;
    };
    ownAnswer.addEventListener('input', sync);
    sync();
  }
  const review = document.getElementById('relay-review-answer');
  if (review) review.addEventListener('input', () => { relayEditing = true; });

  // A file input cannot be delegated: the change fires on the element itself.
  const file = document.getElementById('relay-shot-file');
  if (file) {
    file.addEventListener('change', () => {
      if (!file.files || !file.files.length) return;
      const chosen = [...file.files];
      // Cleared so the same file can be picked again — after removing it, say.
      // `change` does not fire on an input whose value has not changed.
      file.value = '';
      relayAttachAll(req.id, chosen);
    });
  }
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

// ── Attaching an image ────────────────────────────────────────────────────

/**
 * Scale an image down and turn it into the `data:` URL that gets sent.
 *
 * PNG first, because a screenshot is flat colour and text where PNG is both
 * smaller and sharper than JPEG. The fallback exists for the case PNG is worst
 * at — a photograph, or a screen full of gradients and video — where it can run
 * several times over the cap that the same picture as JPEG sits comfortably
 * under. Choosing by *result* rather than by guessing at the content is the
 * only rule that gets both right.
 */
async function relayShrink(source) {
  const bitmap = await createImageBitmap(source);
  const scale = Math.min(1, RELAY_SHOT_MAX_EDGE / Math.max(bitmap.width, bitmap.height));
  const w = Math.max(1, Math.round(bitmap.width * scale));
  const h = Math.max(1, Math.round(bitmap.height * scale));
  const canvas = document.createElement('canvas');
  canvas.width = w;
  canvas.height = h;
  canvas.getContext('2d').drawImage(bitmap, 0, 0, w, h);
  if (bitmap.close) bitmap.close();

  let url = canvas.toDataURL('image/png');
  if (url.length > RELAY_SHOT_MAX_B64) url = canvas.toDataURL('image/jpeg', 0.85);
  return { url, w, h };
}

/**
 * Repaint the popup without losing a half-typed caption.
 *
 * Attaching an image rebuilds the body, and the textarea goes with it. Someone
 * who wrote "this is the 375px breakpoint" and *then* pasted the screenshot
 * would watch their sentence disappear — so the text is carried across, and the
 * synthetic `input` re-runs the enable/disable pass that was listening on it.
 */
function relayRenderKeepingCaption() {
  const caption = document.getElementById('relay-own-answer')?.value || '';
  relayRender();
  const box = document.getElementById('relay-own-answer');
  if (box && caption) {
    box.value = caption;
    box.dispatchEvent(new Event('input'));
  }
}

/**
 * Hold an image against `id`, ready to send. The preview is the receipt.
 *
 * Adds to whatever is already attached, and refuses at the two limits the
 * daemon would refuse at anyway — one image too heavy, or the set as a whole
 * past what a reply carries. Refusing here is worth the duplicated constants:
 * the person is looking at the popup with the previews in front of them, and
 * can drop the one that was making the same point twice.
 *
 * `render` is false while a batch is being attached — dropping four files
 * repaints once at the end rather than four times.
 */
async function relayAttach(id, source, label, render = true) {
  const held = relayHeld(id);
  if (held.length >= RELAY_MAX_SHOTS) {
    showToast(`${RELAY_MAX_SHOTS} images is the most one reply carries`, 'error');
    return false;
  }
  try {
    const { url, w, h } = await relayShrink(source);
    if (url.length > RELAY_SHOT_MAX_B64) {
      showToast('That image is too large to send — try a single window', 'error');
      return false;
    }
    const weight = held.reduce((sum, shot) => sum + shot.url.length, url.length);
    if (weight > RELAY_SHOTS_MAX_B64) {
      showToast('That is more than one reply carries — remove one first', 'error');
      return false;
    }
    relayShots.set(id, held.concat({ url, label: `${label || 'image'} · ${w}×${h}` }));
    if (render) relayRenderKeepingCaption();
    return true;
  } catch (_) {
    showToast('That did not come through as an image', 'error');
    return false;
  }
}

/**
 * Attach a whole batch — a multi-file drop, or a picker several files were
 * chosen in — in the order they were given, then repaint once.
 *
 * Sequential rather than `Promise.all`: the order the images arrive in is what
 * the agent is told they are in, and a race would shuffle a before and an
 * after.
 */
async function relayAttachAll(id, files) {
  for (const file of files) await relayAttach(id, file, file.name, false);
  relayRenderKeepingCaption();
}

/**
 * Read the clipboard on a click, rather than waiting to be pasted into.
 *
 * ⌘V still works (the listener below), but it is not something to *rely* on
 * here: the popup shares a tab with a live terminal, the keystroke belongs to
 * whatever holds focus, and a person looking at a modal with a screenshot
 * already on their clipboard should not have to guess which of the two is
 * listening. A button is the version of that gesture with no ambiguity in it —
 * and the click is also the user activation the API insists on.
 */
async function relayPasteFromClipboard(id) {
  if (!relayCanPaste()) {
    showToast('Reading the clipboard needs sbxw opened on localhost', 'error');
    return;
  }
  let items;
  try {
    items = await navigator.clipboard.read();
  } catch (_) {
    // Permission refused, or a browser that only allows the keystroke. Both
    // leave ⌘V working, so that is what to say.
    showToast('The browser would not hand over the clipboard — press ⌘V instead', 'error');
    return;
  }
  let attached = 0;
  for (const item of items) {
    const type = item.types.find(t => t.startsWith('image/'));
    if (!type) continue;
    try {
      if (await relayAttach(id, await item.getType(type), 'from clipboard', false)) attached++;
    } catch (_) {
      // Unreadable item; another one on the clipboard may still be an image.
    }
  }
  if (!attached) {
    showToast('Nothing on the clipboard is an image', 'error');
    return;
  }
  relayRenderKeepingCaption();
}

/**
 * Capture a window (or a screen) through the browser itself.
 *
 * Worth the extra path because it removes the step where the whole thing goes
 * wrong: an agent is waiting *now*, and "go and take a screenshot, then come
 * back and paste it" is where a person leaves the popup and forgets. The picker
 * the browser puts up is also the permission — sbxw never chooses what is
 * captured, and cannot see the screen the rest of the time.
 */
async function relayCapture(id) {
  if (!relayCanCapture()) {
    showToast('Capture needs sbxw opened on localhost — paste or drop an image instead', 'error');
    return;
  }
  let stream;
  try {
    stream = await navigator.mediaDevices.getDisplayMedia({ video: true, audio: false });
  } catch (_) {
    return; // Picker dismissed. Changing your mind is not an error.
  }
  try {
    const video = document.createElement('video');
    video.srcObject = stream;
    video.muted = true;
    await video.play();
    // A frame has to have actually arrived before the canvas has anything to
    // copy; `play()` resolving only means the pipeline started. Two frames, so
    // what is captured is a painted one rather than the first blank buffer.
    await new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)));
    await relayAttach(id, video, 'captured');
  } finally {
    stream.getTracks().forEach(t => t.stop());
  }
}

/** Every image on a DataTransfer or clipboard, in the order it offers them. */
function relayImagesFrom(data) {
  return [...((data && data.items) || [])]
    .filter(i => i.kind === 'file' && i.type.startsWith('image/'))
    .map(i => i.getAsFile())
    .filter(Boolean);
}

// ⌘V as well as the button, for the muscle memory of whoever just pressed the
// OS screenshot key: anywhere in the popup, with no field to focus first.
// Scoped to a screenshot request that is actually on screen, so it never
// swallows a paste meant for a terminal or a textarea.
document.addEventListener('paste', ev => {
  if (relayOverlay.classList.contains('hidden')) return;
  const req = relayCurrent();
  if (!req || !relayIsShot(req) || req.state !== 'pending') return;
  const files = relayImagesFrom(ev.clipboardData);
  if (!files.length) return;
  ev.preventDefault();
  relayAttachAll(req.id, files);
});

relayOverlay.addEventListener('dragover', ev => {
  const req = relayCurrent();
  if (!req || !relayIsShot(req)) return;
  ev.preventDefault();
  const zone = ev.target.closest('#relay-shot-zone');
  if (zone) zone.classList.add('over');
});
relayOverlay.addEventListener('dragleave', ev => {
  const zone = ev.target.closest('#relay-shot-zone');
  if (zone) zone.classList.remove('over');
});
relayOverlay.addEventListener('drop', ev => {
  const req = relayCurrent();
  if (!req || !relayIsShot(req) || req.state !== 'pending') return;
  ev.preventDefault();
  const files = relayImagesFrom(ev.dataTransfer);
  if (files.length) relayAttachAll(req.id, files);
});

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

  // The drop zone is a button in everything but markup — clicking it (or the
  // preview already in it) picks a file, so the three ways in are one target.
  if (ev.target.closest('#relay-shot-zone')) {
    document.getElementById('relay-shot-file')?.click();
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
    if (await relayPost(`/api/relay/${encodeURIComponent(req.id)}/deny`, {})) {
      relayEditing = false;
      // Refusing has to take the image with it, or the next request to reach
      // this popup would find a screenshot from the one you just declined.
      relayShots.delete(req.id);
    }
  } else if (action === 'paste') {
    relayPasteFromClipboard(req.id);
  } else if (action === 'capture') {
    relayCapture(req.id);
  } else if (action === 'choose') {
    document.getElementById('relay-shot-file')?.click();
  } else if (action === 'drop-shot') {
    const at = Number(ev.target.closest('[data-shot]')?.dataset.shot);
    relayShots.set(req.id, relayHeld(req.id).filter((_, i) => i !== at));
    relayRenderKeepingCaption();
  } else if (action === 'drop-shots') {
    relayShots.delete(req.id);
    relayRenderKeepingCaption();
  } else if (action === 'approve' || action === 'approve-own') {
    const box = document.getElementById(
      action === 'approve' ? 'relay-review-answer' : 'relay-own-answer');
    const answer = (box?.value || '').trim();
    const images = relayHeld(req.id).map(shot => shot.url);
    // Either one is enough. A screenshot needs no caption, and a person who
    // would rather describe what they see than show it has answered too.
    if (!answer && !images.length) { showToast('Nothing to send', 'error'); return; }
    if (await relayPost(`/api/relay/${encodeURIComponent(req.id)}/approve`, { answer, images })) {
      relayEditing = false;
      relayShots.delete(req.id);
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
    // Whatever was attached has either been sent or refused; either way this
    // tab has no further use for a copy of someone's screen.
    relayShots.delete(req.id);
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
