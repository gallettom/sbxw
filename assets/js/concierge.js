// ── Voice concierge ───────────────────────────────────────────────────────
// The voice side of sbxw, apart from the chat dock (which talks to the
// `ephemeral-chat` sandboxes): a 🎙 button beside it opens its own panel.
// See `.sbxw-artifacts/specs/concierge-vocal.md`.
//
// * **No language model.** What is said becomes text (Parakeet, in the
//   daemon) and goes to an agent as it was said, once read and confirmed;
//   what an agent writes is read aloud as it was written (Voxtral TTS, a
//   local server the daemon starts). Nothing is reworded on the way.
// * **Push-to-talk only**: hold the button, or the key `[voice]` names (F8 by
//   default — no terminal binds it). Nothing listens while nothing is held.
// * **The page resamples**: MediaRecorder's own container (webm, mp4) is
//   decoded here and sent as 16 kHz mono f32, so the daemon parses no audio
//   format. The audio never leaves the tab otherwise and is never stored.
// * **The models are a decision**: until they are installed the panel says
//   which command installs them, and offers a button that asks first.
//
// Loaded after main.js: it listens on `sbxwStream` for `voice` events.

const vcEl = id => document.getElementById(id);
const vcLauncher = vcEl('vc-launcher');
const vcMic = vcEl('vc-l-mic');
const vcStopBtn = vcEl('vc-l-stop');
const vcOpenBtn = vcEl('vc-l-open');
const vcPanel = vcEl('vc-panel');
const vcLog = vcEl('vc-log');
const vcSetup = vcEl('vc-setup');
const vcTalk = vcEl('vc-talk');
const vcHint = vcEl('vc-hint');

const VC_RATE = 16000;
let vcStatus = null;
let vcOpen = false;
let vcRec = null; // { recorder, chunks, stream, started }
let vcBusy = false;
let vcStarting = false;
function vcMb(n) { return `${Math.round(n / 1e6)} MB`; }
function vcGb(mb) { return `${(mb / 1000).toFixed(1)} GB`; }

function vcKey() { return (vcStatus && vcStatus.push_to_talk_key) || 'F8'; }
function vcReady() { return !!(vcStatus && vcStatus.ready && vcStatus.compiled); }

// ── Setup: what is missing, and how to get it ────────────────────────────
function vcRenderSetup() {
  const s = vcStatus;
  vcSetup.replaceChildren();
  if (!s) return;
  const p = (cls, text) => { const e = document.createElement('p'); e.className = cls; e.textContent = text; return e; };
  const code = text => { const c = document.createElement('code'); c.textContent = text; return c; };

  if (!window.isSecureContext) {
    vcSetup.append(p('vc-warn', 'The browser only lends the microphone to a page served on 127.0.0.1 or over HTTPS. Open sbxw at http://127.0.0.1:7681 to talk.'));
  }
  if (!s.compiled) {
    vcSetup.append(p('vc-warn', 'This sbxw was built without voice. Rebuild it with:'), code('cargo build --release --features voice'));
  }
  const job = s.job;
  if (job && (job.state === 'running' || job.state === 'verifying')) {
    const pct = job.total ? Math.floor(job.done * 100 / job.total) : 0;
    const row = document.createElement('div');
    row.className = 'vc-progress';
    const bar = document.createElement('progress');
    bar.max = 100; bar.value = pct;
    const label = p('vc-dim', `${job.state === 'verifying' ? 'Checking' : 'Downloading'} ${job.model} · ${job.file} — ${pct}% (${vcMb(job.done)} / ${vcMb(job.total)})`);
    const cancel = document.createElement('button');
    cancel.type = 'button'; cancel.className = 'vc-btn'; cancel.textContent = 'Cancel';
    cancel.onclick = () => fetch('/api/voice/cancel', { method: 'POST' });
    row.append(bar, label, cancel);
    vcSetup.append(row);
    return;
  }
  if (!s.ready) {
    const list = document.createElement('ul');
    list.className = 'vc-models';
    for (const m of s.models || []) {
      const li = document.createElement('li');
      const st = m.installed.state === 'ready' ? '✓' : m.installed.state === 'partial' ? `partial, ${vcMb(m.installed.have)}` : `~${m.approx_mb} MB`;
      li.textContent = `${m.role.toUpperCase()} · ${m.label} (${m.license}) — ${st}`;
      list.append(li);
    }
    if (s.speech?.supported) {
      const li = document.createElement('li');
      li.textContent = `RUNTIME · ${s.speech.runtime}, fetched by uvx — ${s.speech.runtime_ready ? '✓' : 'needs uv installed'}`;
      list.append(li);
    }
    vcSetup.append(p('vc-text', `Voice needs local models, about ${vcGb(s.approx_mb)}. Install them from a terminal:`), code(s.hint || 'sbxw voice install'), p('vc-dim', 'or here:'), list);
    if (job && job.state === 'failed') vcSetup.append(p('vc-warn', `Last download failed: ${job.error || 'unknown error'}`));
    if (job && job.state === 'cancelled') vcSetup.append(p('vc-dim', 'Download cancelled — installing resumes where it stopped.'));
    const go = document.createElement('button');
    go.type = 'button'; go.className = 'vc-btn vc-primary';
    go.textContent = `Install voice (~${vcGb(s.approx_mb)})`;
    go.onclick = async () => {
      if (!confirm(`Download about ${vcGb(s.approx_mb)} of models into ${s.dir}?\nIt can be cancelled and resumed.`)) return;
      const r = await fetch('/api/voice/install', { method: 'POST' }).then(r => r.json()).catch(() => null);
      if (r && !r.ok) vcSay('err', r.error);
    };
    vcSetup.append(go);
  }
}

function vcRender() {
  vcRenderSetup();
  const ready = vcReady() && window.isSecureContext;
  vcTalk.disabled = !ready || vcBusy;
  vcTalk.classList.toggle('recording', !!vcRec);
  vcTalk.textContent = vcRec ? '● Listening… release to send' : vcBusy ? 'Transcribing…' : `Hold to talk (${vcKey()})`;
  vcHint.textContent = !ready ? '' : vcStatus.speech?.supported
    ? 'Speech is turned into text locally; the audio is never kept.'
    : 'Speech is turned into text locally. Reading aloud needs a Mac with Apple Silicon.';
  vcLauncher.classList.toggle('ready', ready);
  vcLauncher.classList.toggle('listening', !!vcRec && !vcRec.dest);
  vcLauncher.classList.toggle('busy', vcBusy && !vcRec);
  vcMic.setAttribute('aria-pressed', String(!!vcRec && !vcRec.dest));
  vcMic.title = !ready ? 'Voice is not set up: click to see why'
    : vcRec ? 'Listening: click to send'
      : vcBusy ? 'Transcribing…' : `Click to talk, click again to send (or hold ${vcKey()})`;
  document.querySelectorAll('.pane-dictate-btn').forEach(b => {
    b.classList.toggle('recording', !!vcRec?.dest && vcRec.dest.pane === +b.dataset.pane);
    b.classList.toggle('busy', vcBusy && !vcRec);
  });
  // On <body>, so a pane opened later shows its button too.
  document.body.classList.toggle('voice-ready', vcReady());
  vcLauncher.querySelector('kbd').textContent = vcKey();
}

// Something was added to a closed panel: a dot on the bar's Open says so
// until the panel is opened.
function vcNews() {
  if (vcOpen) return;
  vcLauncher.classList.add('news');
  vcOpenBtn.title = 'Open the Concierge panel: something new is in it';
}

function vcSay(who, text, meta) {
  // The panel stays closed: an error is a toast, and Open in the bar shows
  // that something new is in there.
  if (who === 'err' && !vcOpen) showToast(text, 'error');
  vcNews();
  const row = document.createElement('div');
  row.className = `vc-msg vc-${who}`;
  row.append(document.createTextNode(text));
  if (meta) {
    const m = document.createElement('span');
    m.className = 'vc-meta'; m.textContent = meta;
    row.append(m);
  }
  vcLog.append(row);
  vcLog.scrollTop = vcLog.scrollHeight;
  return row;
}

async function vcRefresh() {
  try { vcStatus = await fetch('/api/voice').then(r => r.json()); } catch (_) { vcStatus = null; }
  vcRender();
}

// ── Recording ────────────────────────────────────────────────────────────
// `dest`: null for the concierge, `{ pane }` to dictate into a terminal.
async function vcStart(dest = null) {
  vcSpeech.stop(); // speaking over the voice interrupts it
  vcWarm(false);
  if (dest && (!vcReady() || !window.isSecureContext)) {
    showToast(!window.isSecureContext ? 'The microphone needs sbxw on 127.0.0.1' : `Voice is not installed — run ${vcStatus?.hint || 'sbxw voice install'}`, 'error');
    return;
  }
  if (vcRec || vcBusy || vcStarting || !vcReady() || !window.isSecureContext) return;
  let stream;
  // The microphone takes a moment to open; a second click meanwhile is not a second start.
  vcStarting = true;
  try {
    stream = await navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true } });
  } catch (e) {
    vcStarting = false;
    if (dest) showToast(`No microphone: ${e.message || e}`, 'error');
    else vcSay('err', `No microphone: ${e.message || e}`);
    return;
  }
  const recorder = new MediaRecorder(stream);
  const rec = { recorder, chunks: [], stream, started: performance.now(), dest };
  recorder.ondataavailable = e => { if (e.data.size) rec.chunks.push(e.data); };
  recorder.start();
  vcStarting = false;
  vcRec = rec;
  vcRender();
}

async function vcStop() {
  const rec = vcRec;
  if (!rec) return;
  vcRec = null;
  const stopped = new Promise(res => { rec.recorder.onstop = res; });
  rec.recorder.stop();
  await stopped;
  rec.stream.getTracks().forEach(t => t.stop());
  if (performance.now() - rec.started < 300) { vcRender(); return; } // a tap, not speech
  vcBusy = true;
  vcRender();
  try {
    const pcm = await vcToPcm(new Blob(rec.chunks, { type: rec.recorder.mimeType }));
    const r = await fetch('/api/voice/transcribe', {
      method: 'POST', headers: { 'content-type': 'application/octet-stream' }, body: pcm.buffer,
    }).then(r => r.json());
    if (rec.dest) {
      if (!r.ok) showToast(`Dictation: ${r.error}`, 'error');
      else if (r.text) { vcDictate(rec.dest.pane, r.text); if (r.masked) showToast(vcMaskedNote(r.masked)); }
    } else if (!r.ok) vcSay('err', r.error);
    else if (!r.text) vcSay('dim', '(nothing heard)');
    else {
      vcHear(r.text);
      if (r.masked) vcSay('dim', vcMaskedNote(r.masked));
    }
  } catch (e) {
    if (rec.dest) showToast(`Dictation failed: ${e.message || e}`, 'error');
    else vcSay('err', `Could not transcribe: ${e.message || e}`);
  } finally {
    vcBusy = false;
    vcRender();
  }
}

// ── Anonymisation ────────────────────────────────────────────────────────
// What is dictated comes back from the daemon already masked: the names the
// person listed in ~/.sbxw/anonymise.toml, and what looks like an e-mail, a
// phone number, an IBAN or a key. A message can be typed or edited here too,
// so it is masked once more just before it is sent: the text area then shows
// what really left. A rules file that does not parse sends nothing.
function vcMaskedNote(n) { return `${n} thing${n > 1 ? 's' : ''} masked before sending`; }
async function vcMask(area) {
  const r = await postJsonOrError('/api/voice/anonymise', { text: area.value.trim() });
  if (!r.ok) { vcSay('err', `Not sent, the anonymisation failed: ${r.error}`); return null; }
  area.value = r.text;
  if (r.masked) vcSay('dim', vcMaskedNote(r.masked));
  return r.text;
}

// ── Dictation into a terminal ────────────────────────────────────────────
// What is said is typed into the pane's terminal, as the keyboard would:
// into Claude's prompt, or a shell. No Enter, so it can be read and fixed
// first — unless the dictation ends with "entrée" (or "envoie", "enter"),
// which is dropped and pressed as Return, its own keystroke once the text
// has landed (a Return inside the burst would be pasted as a newline).
const VC_ENTER = /[\s,.;:!?]*\b(entr[ée]e?|entrer|envoie|envoyer|enter|send)[\s.!]*$/i;
function vcDictate(idx, text) {
  const pane = panes[idx];
  if (!pane?.ws || pane.ws.readyState !== WebSocket.OPEN) { showToast('That terminal is not connected', 'error'); return; }
  let enter = false;
  if (VC_ENTER.test(text)) { enter = true; text = text.replace(VC_ENTER, ''); }
  text = text.trim();
  if (text) pane.ws.send(new TextEncoder().encode(text));
  if (enter) setTimeout(() => { if (pane.ws?.readyState === WebSocket.OPEN) pane.ws.send(new TextEncoder().encode('\r')); }, 400);
  pane.term?.focus();
}

function vcDictateKey() { return (vcStatus && vcStatus.dictate_key) || 'F9'; }

// The pane's button toggles: one click starts listening, the next stops and
// types — easier than holding a button through a long sentence. F9 stays
// push-to-talk.
document.addEventListener('click', e => {
  const b = e.target.closest('.pane-dictate-btn');
  if (!b) return;
  e.preventDefault();
  const pane = +b.dataset.pane;
  if (vcRec?.dest) {
    const same = vcRec.dest.pane === pane;
    vcStop();
    if (same) return;
  }
  if (!vcRec) vcStart({ pane });
});
window.addEventListener('keydown', e => {
  if (e.code !== vcDictateKey() || e.repeat) return;
  e.preventDefault(); e.stopPropagation();
  if (typeof focusedPane === 'number') vcStart({ pane: focusedPane });
}, true);
window.addEventListener('keyup', e => {
  if (e.code !== vcDictateKey()) return;
  e.preventDefault(); e.stopPropagation();
  if (vcRec?.dest) vcStop();
}, true);

// Whatever the recorder produced, as 16 kHz mono f32.
async function vcToPcm(blob) {
  const ctx = new AudioContext();
  const decoded = await ctx.decodeAudioData(await blob.arrayBuffer());
  ctx.close();
  const frames = Math.ceil(decoded.duration * VC_RATE);
  const off = new OfflineAudioContext(1, Math.max(frames, 1), VC_RATE);
  const src = off.createBufferSource();
  src.buffer = decoded;
  src.connect(off.destination);
  src.start();
  return (await off.startRendering()).getChannelData(0);
}

// ── The sandbox ──────────────────────────────────────────────────────────
// Asked of the daemon each time: the page's own list is the one it loaded
// with, and misses a sandbox started since (it took a reload to appear).
const VC_TARGET = 'sbxw-concierge-target';
async function vcTargets() {
  let list = typeof sandboxes !== 'undefined' ? sandboxes : [];
  try {
    const fresh = await fetch('/api/sandboxes').then(r => r.json());
    if (Array.isArray(fresh)) list = fresh;
  } catch (_) {}
  const live = list.filter(s => s.status === 'running' && !s.unreachable_since).map(s => s.name);
  return [...new Set(live)].sort();
}

// Picked at the top, remembered per browser: the sandbox whose agents are
// followed, and where a message said with nothing to answer goes.
const vcPick = vcEl('vc-target');
const vcBarTarget = vcEl('vc-l-target');
const vcBarName = vcEl('vc-l-name');
let vcTarget = null;
try { vcTarget = localStorage.getItem(VC_TARGET) || null; } catch (_) {}

async function vcFillTargets() {
  const targets = await vcTargets();
  if (vcTarget && !targets.includes(vcTarget)) targets.unshift(vcTarget); // stopped, but still the choice
  vcPick.replaceChildren();
  const none = document.createElement('option');
  none.value = ''; none.textContent = targets.length ? '— no project —' : 'no running sandbox';
  vcPick.append(none);
  for (const name of targets) {
    const o = document.createElement('option');
    o.value = o.textContent = name;
    vcPick.append(o);
  }
  vcPick.value = vcTarget || '';
  vcRenderTarget();
}
function vcRenderTarget() {
  vcBarName.textContent = vcTarget || 'pick a sandbox';
  vcBarTarget.classList.toggle('none', !vcTarget);
}
function vcSetTarget(name) {
  vcTarget = name || null;
  try { vcTarget ? localStorage.setItem(VC_TARGET, vcTarget) : localStorage.removeItem(VC_TARGET); } catch (_) {}
  if (![...vcPick.options].some(o => o.value === (vcTarget || ''))) vcFillTargets();
  vcPick.value = vcTarget || '';
  vcRenderTarget();
}
vcPick.addEventListener('change', () => vcSetTarget(vcPick.value));
vcFillTargets();

// The name in the bar opens a menu of the running sandboxes, the app's own
// (the pane menu's shell): one row each, the current one ticked, and a last
// row for none. Arrows move, Enter picks, Escape or a click elsewhere closes.
let vcMenu = null;
function vcCloseMenu(refocus) {
  if (!vcMenu || vcMenu.hidden) return;
  vcMenu.hidden = true;
  vcBarTarget.setAttribute('aria-expanded', 'false');
  if (refocus) vcBarTarget.focus();
}
async function vcOpenMenu() {
  if (!vcMenu) {
    vcMenu = document.createElement('div');
    vcMenu.className = 'pane-menu vc-target-menu';
    vcMenu.setAttribute('role', 'menu');
    vcMenu.setAttribute('aria-label', 'The sandbox your words go to');
    vcMenu.hidden = true;
    vcMenu.addEventListener('click', e => {
      const row = e.target.closest('button[data-name]');
      if (!row) return;
      vcSetTarget(row.dataset.name);
      vcCloseMenu(true);
    });
    vcMenu.addEventListener('keydown', e => {
      const rows = [...vcMenu.querySelectorAll('button')];
      const at = rows.indexOf(document.activeElement);
      if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); vcCloseMenu(true); }
      else if (e.key === 'ArrowDown') { e.preventDefault(); rows[(at + 1) % rows.length]?.focus(); }
      else if (e.key === 'ArrowUp') { e.preventDefault(); rows[(at - 1 + rows.length) % rows.length]?.focus(); }
      else if (e.key === 'Tab') vcCloseMenu(false);
    });
    document.addEventListener('pointerdown', e => {
      if (!vcMenu.contains(e.target) && !vcBarTarget.contains(e.target)) vcCloseMenu(false);
    });
    document.body.appendChild(vcMenu);
  }
  const targets = await vcTargets();
  if (vcTarget && !targets.includes(vcTarget)) targets.unshift(vcTarget); // stopped, but still the choice
  const row = (name, label, cls) => {
    const b = document.createElement('button');
    b.type = 'button';
    b.dataset.name = name;
    b.setAttribute('role', 'menuitemradio');
    b.setAttribute('aria-checked', String((vcTarget || '') === name));
    const tick = document.createElement('span');
    tick.className = 'vc-tm-tick';
    tick.textContent = (vcTarget || '') === name ? '✓' : '';
    const text = document.createElement('span');
    text.className = `vc-tm-name ${cls || ''}`;
    text.textContent = label;
    b.append(tick, text);
    return b;
  };
  vcMenu.replaceChildren();
  if (!targets.length) {
    const none = document.createElement('div');
    none.className = 'vc-tm-empty';
    none.textContent = 'No running sandbox';
    vcMenu.append(none);
  }
  for (const name of targets) vcMenu.append(row(name, name));
  if (vcTarget || targets.length) vcMenu.append(row('', 'No sandbox', 'vc-tm-none'));
  vcMenu.hidden = false;
  vcBarTarget.setAttribute('aria-expanded', 'true');
  const r = vcBarTarget.getBoundingClientRect();
  const w = vcMenu.offsetWidth;
  vcMenu.style.top = `${r.bottom + 8}px`;
  vcMenu.style.left = `${Math.max(8, Math.min(r.left + r.width / 2 - w / 2, window.innerWidth - w - 8))}px`;
  (vcMenu.querySelector('[aria-checked="true"]') || vcMenu.querySelector('button'))?.focus();
}
vcBarTarget.addEventListener('click', () => (vcMenu && !vcMenu.hidden ? vcCloseMenu(true) : vcOpenMenu()));

// ── A message to the sandbox ─────────────────────────────────────────────
// Words said (or typed) with nothing to answer are a message for the picked
// sandbox's agent: masked, then typed into it at once, with no card to
// confirm. What left is shown when the masking changed it.
async function vcDraft(text) {
  const name = vcTarget;
  if (!name) return vcDraftCard(text);
  const safe = await postJsonOrError('/api/voice/anonymise', { text: text.trim() });
  if (!safe.ok) { vcSay('err', `Not sent, the anonymisation failed: ${safe.error}`); return; }
  const r = await postJsonOrError('/api/chat/push', { text: safe.text, name });
  // Not lost: the words wait in a card, to send again or drop.
  if (!r.ok) { vcSay('err', `Not sent to ${name}: ${r.error}`); return vcDraftCard(safe.text); }
  vcSay('dim', safe.masked ? `Sent to ${name}, ${vcMaskedNote(safe.masked).replace(' before sending', '')}: ${safe.text}` : `Sent to ${name}.`);
  if (!vcSpeech.muted) vcSpeak('Envoyé.');
}

// With no sandbox picked, or when sending failed, the words are kept in an
// editable card: more words add to it; "oui" or Send types it into the
// agent, "non" or Discard drops it.
function vcDraftCard(text) {
  const target = vcTarget;
  const { card, head, foot } = vcCard('vc-draft', target ? `To ${target}` : 'Pick a sandbox at the top first');
  const area = document.createElement('textarea');
  area.className = 'vc-brief-text';
  area.value = text;
  area.setAttribute('aria-label', 'Message');
  const fit = () => { area.rows = Math.min(12, Math.max(2, area.value.split('\n').length + 1)); };
  fit();
  card.insertBefore(area, foot);
  const done = note => { vcDisarm(); card.classList.add('done'); area.readOnly = true; foot.replaceChildren(); if (note) vcSay('dim', note); };
  const send = vcButton('Send', async () => {
    const name = vcTarget || target;
    const body = area.value.trim();
    if (!name) { vcSay('err', 'Pick a sandbox at the top first.'); return; }
    if (!body) return;
    send.disabled = true;
    const safe = await vcMask(area);
    if (safe === null) { send.disabled = false; return; }
    const r = await postJsonOrError('/api/chat/push', { text: safe, name });
    if (!r.ok) { send.disabled = false; vcSay('err', `Not sent to ${name}: ${r.error}`); return; }
    head.textContent = `Sent to ${name}`;
    done(`Typed into ${name}'s agent.`);
  }, true);
  const drop = vcButton('Discard', () => { card.remove(); vcDisarm(); });
  const hint = document.createElement('span');
  hint.className = 'vc-armed-hint';
  hint.textContent = 'Keep talking to add to it; say “oui” to send.';
  foot.append(send, drop, hint);
  vcArm({
    kind: 'draft', card,
    add: more => { area.value = `${area.value.trimEnd()} ${more}`.trim(); fit(); vcLog.scrollTop = vcLog.scrollHeight; },
    yes: () => send.click(), no: () => drop.click(),
  });
  if (!vcSpeech.muted) vcSpeak(target ? "C'est noté. J'envoie ?" : 'Choisis un sandbox en haut.');
}

// Start over: the log emptied, nothing armed, the voice quiet.
function vcReset() {
  vcDisarm();
  vcSpeech.stop();
  vcLog.replaceChildren();
}

// Typing does what speaking does, for when speaking is not an option.
vcEl('vc-type').addEventListener('submit', e => {
  e.preventDefault();
  const input = vcEl('vc-type-input');
  const text = input.value.trim();
  if (!text || !vcReady()) return;
  input.value = '';
  vcHear(text);
});
vcEl('vc-new').addEventListener('click', vcReset);

// ── The voice ────────────────────────────────────────────────────────────
// A message is cut into sentences, grouped into batches, and each batch is
// one request to /api/voice/speak. The answer is raw sound that arrives as it
// is made (16-bit mono PCM): each piece is scheduled right behind the last
// one, so the voice starts before the batch is finished and batches follow
// each other without a gap. Muting (the speaker, remembered per browser) keeps the
// text and skips the sound; talking again stops the voice at once.
const VC_MUTE = 'sbxw-concierge-mute';
const vcSpeech = (() => {
  let buf = '';
  let gen = 0;            // bumped by stop(): late audio of an old reply is dropped
  let ctx = null;
  let nextAt = 0;         // when, on ctx's clock, the next piece of sound starts
  const live = new Set(); // sources scheduled or playing, for stop()
  const VC_LEAD = 0.4;    // seconds of sound kept ready before the voice starts
  let lead = VC_LEAD;
  let gaps = 0;           // times the voice caught up, for the console
  let muted = false;
  try { muted = localStorage.getItem(VC_MUTE) === '1'; } catch (_) {}

  // A sentence ends at . ! ? … or a line break, followed by space or the end
  // (so "3.5" and "e.g." mid-word do not cut).
  const END = /[.!?…\n]+["»)\]]?(\s|$)/;
  const VC_BATCH = 5;         // sentences per request, at most
  const VC_BATCH_CHARS = 900; // the daemon refuses more than 1000
  const VC_AHEAD = 30;        // seconds of sound made ahead of the voice, at most
  let pending = [];       // sentences of the batch being gathered
  let pendingChars = 0;
  let timer = 0;
  let queue = [];         // batches not yet sent
  let fetching = false;   // one batch is made at a time, in order
  let abort = null;       // cancels the request in flight on stop()
  let wait = 0;
  // Told to the bar: something is being made or played.
  let was = false;
  function tell() {
    const on = fetching || queue.length > 0 || pending.length > 0 || live.size > 0;
    if (on !== was) { was = on; vcSpeaking(on); }
  }
  function say(text) {
    text = text.trim();
    if (!text || muted || !vcReady() || !vcStatus.speech?.supported) return;
    // Sentences are grouped: a batch leaves 100 ms after its last sentence,
    // or at once when it holds VC_BATCH of them or nears the request limit.
    if (pending.length && pendingChars + text.length + 1 > VC_BATCH_CHARS) batch();
    pending.push(text);
    pendingChars += text.length + 1;
    clearTimeout(timer);
    if (pending.length >= VC_BATCH) batch();
    else timer = setTimeout(batch, 100);
    tell();
  }
  function batch() {
    clearTimeout(timer);
    if (!pending.length) return;
    queue.push(pending.join(' '));
    pending = [];
    pendingChars = 0;
    pump();
  }
  function drop() { clearTimeout(timer); clearTimeout(wait); pending = []; pendingChars = 0; queue = []; }
  // Schedule one piece of PCM right behind what is already scheduled.
  function play(samples, rate) {
    if (!ctx || ctx.sampleRate !== rate) {
      // The context runs at the voice's own rate: pieces join without resampling.
      try { ctx = new AudioContext({ sampleRate: rate }); } catch (_) { ctx = ctx || new AudioContext(); }
      nextAt = 0;
    }
    const audio = ctx.createBuffer(1, samples.length, rate);
    const out = audio.getChannelData(0);
    for (let i = 0; i < samples.length; i++) out[i] = samples[i] / 32768;
    const src = ctx.createBufferSource();
    src.buffer = audio;
    src.connect(ctx.destination);
    src.onended = () => { live.delete(src); tell(); };
    live.add(src);
    // The voice caught up with what is made (or is starting): it waits for a
    // little sound to be ready, longer each time, so that a slow moment is
    // one pause and not a stutter.
    if (nextAt < ctx.currentTime + 0.02) {
      if (nextAt) { gaps++; lead = Math.min(lead * 2, 3); }
      nextAt = ctx.currentTime + lead;
    }
    src.start(nextAt);
    nextAt += audio.duration;
  }
  async function pump() {
    if (fetching || !queue.length) return;
    // Enough is ready: wait for the voice to catch up before making more.
    const lead = ctx ? nextAt - ctx.currentTime : 0;
    if (lead > VC_AHEAD) { clearTimeout(wait); wait = setTimeout(pump, (lead - VC_AHEAD + 1) * 1000); return; }
    const my = gen;
    const text = queue.shift();
    const sent = performance.now();
    fetching = true;
    abort = new AbortController();
    try {
      const r = await fetch('/api/voice/speak', {
        method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ text }), signal: abort.signal,
      });
      if (!r.ok) throw new Error((await r.json().catch(() => ({}))).error || r.status);
      const rate = Number(r.headers.get('x-sample-rate')) || 24000;
      const reader = r.body.getReader();
      let odd = null;       // the first half of a sample cut between two pieces
      let seconds = 0;
      let firstAfter = 0;
      for (;;) {
        const { done, value } = await reader.read();
        if (my !== gen) { reader.cancel().catch(() => {}); return; }
        if (done) break;
        let bytes = value;
        if (odd !== null) { bytes = new Uint8Array(value.length + 1); bytes[0] = odd; bytes.set(value, 1); odd = null; }
        if (bytes.length % 2) { odd = bytes[bytes.length - 1]; bytes = bytes.subarray(0, bytes.length - 1); }
        if (!bytes.length) continue;
        const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.length);
        const samples = new Int16Array(bytes.length / 2);
        for (let i = 0; i < samples.length; i++) samples[i] = view.getInt16(i * 2, true);
        if (!firstAfter) firstAfter = (performance.now() - sent) / 1000;
        seconds += samples.length / rate;
        play(samples, rate);
      }
      console.debug(`voice: ${seconds.toFixed(1)} s of audio for ${text.length} characters, first sound after ${firstAfter.toFixed(1)} s, all of it after ${((performance.now() - sent) / 1000).toFixed(1)} s, ${gaps} pause(s) so far`);
    } catch (e) {
      // One failure ends the reading: the rest would fail the same way.
      if (my === gen) { drop(); vcSay('err', `Could not speak: ${e.message || e}`); }
    } finally {
      if (my === gen) { fetching = false; pump(); tell(); }
    }
  }
  return {
    feed(piece) {
      buf += piece;
      let m;
      while ((m = END.exec(buf))) {
        const cut = m.index + m[0].length;
        say(buf.slice(0, cut));
        buf = buf.slice(cut);
      }
      // A long sentence is not waited for: past ~70 characters it is cut at
      // its last comma, semicolon or colon, so the voice starts sooner.
      if (buf.length > 70) {
        const at = Math.max(buf.lastIndexOf(', '), buf.lastIndexOf('; '), buf.lastIndexOf(': '));
        if (at > 30) { say(buf.slice(0, at + 1)); buf = buf.slice(at + 2); }
      }
      // Text with no punctuation at all (a pasted log) is cut at a space
      // before it outgrows what one request may carry.
      while (buf.length > 400) {
        const at = buf.lastIndexOf(' ', 400);
        const cut = at > 100 ? at : 400;
        say(buf.slice(0, cut)); buf = buf.slice(cut);
      }
    },
    flush() { say(buf); buf = ''; },
    stop() {
      gen++;
      buf = '';
      drop();
      fetching = false;
      if (abort) { abort.abort(); abort = null; }
      for (const src of live) { src.onended = null; try { src.stop(); } catch (_) {} }
      live.clear();
      tell();
      nextAt = 0;
      lead = VC_LEAD;
      gaps = 0;
    },
    get muted() { return muted; },
    toggle() {
      muted = !muted;
      try { localStorage.setItem(VC_MUTE, muted ? '1' : '0'); } catch (_) {}
      if (muted) this.stop();
      return muted;
    },
  };
})();

// The voice switch: the speaker in the bar and the one in the panel are two
// controls for one state (`vcSpeech.muted`, remembered per browser), drawn
// by one function. Off: nothing is read until it is switched back on, and
// what was missed meanwhile is not replayed. On: the next message is read.
const vcMuteBtn = vcEl('vc-mute');
const VC_SVG = inner => `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M11 5 6 9H3v6h3l5 4z"/>${inner}</svg>`;
const VC_ICON_ON = VC_SVG('<path d="M15.5 8.5a5 5 0 0 1 0 7M18.5 5.5a9 9 0 0 1 0 13"/>');
const VC_ICON_OFF = VC_SVG('<path d="m16 9 6 6M22 9l-6 6"/>');
function vcRenderVoice() {
  const off = vcSpeech.muted;
  const reading = !off && vcLauncher.classList.contains('speaking');
  const label = off ? 'Voice off: click to hear replies'
    : reading ? 'Reading aloud: click to stop and mute'
    : 'Voice on: click to mute';
  for (const b of [vcStopBtn, vcMuteBtn]) {
    const icon = off ? 'off' : 'on';
    if (b.dataset.icon !== icon) { b.innerHTML = off ? VC_ICON_OFF : VC_ICON_ON; b.dataset.icon = icon; }
    b.classList.toggle('vc-voice-off', off);
    b.classList.toggle('vc-voice-reading', reading);
    b.title = label;
    b.setAttribute('aria-label', label);
    b.setAttribute('aria-pressed', String(!off));
  }
}
function vcToggleVoice() {
  // Muting stops the reading (toggle calls stop); unmuting loads the voice,
  // as opening the panel does, so the next message starts sooner.
  if (!vcSpeech.toggle()) vcWarm(true);
  vcRenderVoice();
}
vcMuteBtn.addEventListener('click', vcToggleVoice);
vcStopBtn.addEventListener('click', vcToggleVoice);
vcRenderVoice();

// ── Wiring ───────────────────────────────────────────────────────────────
function vcSetOpen(open) {
  vcOpen = open;
  vcPanel.hidden = !open;
  vcOpenBtn.setAttribute('aria-expanded', String(open));
  vcLauncher.classList.toggle('open', open);
  if (open) { vcLauncher.classList.remove('news'); vcLog.scrollTop = vcLog.scrollHeight; }
  vcOpenBtn.title = open ? 'Close the Concierge panel' : 'Open the Concierge panel';
  vcOpenBtn.setAttribute('aria-label', vcOpenBtn.title);
  vcEl('vc-l-open-label').textContent = open ? 'Close' : 'Open';
  if (open) { vcRefresh(); vcWarm(!vcSpeech.muted); vcFillTargets(); }
}

// Load the models before they are needed: opening the panel or pressing a
// key usually comes seconds before the first word, and that is long enough.
// `speech` loads the voice too: not for a dictation, which reads nothing.
const vcWarmed = { false: 0, true: 0 };
function vcWarm(speech) {
  speech = !!speech && !!vcStatus?.speech?.supported;
  if (!vcReady() || Date.now() - vcWarmed[speech] < 60_000) return;
  vcWarmed[speech] = Date.now();
  fetch('/api/voice/warm', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ speech }) }).catch(() => {});
}

vcOpenBtn.addEventListener('click', () => vcSetOpen(!vcOpen));
// The microphone in the bar: a click starts listening, the next one sends.
// Not set up yet: the panel says what is missing.
vcMic.addEventListener('click', () => {
  if (!(vcReady() && window.isSecureContext)) return vcSetOpen(true);
  if (vcRec && !vcRec.dest) vcStop();
  else if (!vcRec && !vcBusy) vcStart();
});
// Something is read aloud: rings on the bar, the speaker lit.
function vcSpeaking(on) {
  vcLauncher.classList.toggle('speaking', on);
  vcRenderVoice();
}
vcEl('vc-close').addEventListener('click', () => vcSetOpen(false));

vcTalk.addEventListener('pointerdown', e => { e.preventDefault(); vcTalk.setPointerCapture(e.pointerId); vcStart(); });
vcTalk.addEventListener('pointerup', () => { if (vcRec && !vcRec.dest) vcStop(); });
vcTalk.addEventListener('pointercancel', () => { if (vcRec && !vcRec.dest) vcStop(); });

// The key works from anywhere on the page, terminals included: captured
// before xterm sees it. The panel stays as it is: the bar shows the listening.
window.addEventListener('keydown', e => {
  if (e.code !== vcKey() || e.repeat) return;
  e.preventDefault(); e.stopPropagation();
  vcWarm(!vcSpeech.muted);
  vcStart();
}, true);
window.addEventListener('keyup', e => {
  if (e.code !== vcKey()) return;
  e.preventDefault(); e.stopPropagation();
  if (vcRec && !vcRec.dest) vcStop();
}, true);
window.addEventListener('keydown', e => {
  if (e.key === 'Escape' && vcOpen && !vcRec) vcSetOpen(false);
});

// ── Following the sandbox's agent: the grill by voice ───────────────────
// The picked sandbox's agents are followed on the session stream (the chat
// dock's own reader, chat-core.js). When one ends its turn with a message —
// a /grill-me round, a question in prose — it shows here and is read aloud
// as written (markdown and code blocks left out), and the next thing said
// answers it: the words are shown, editable, and confirmed before they are
// typed into that session. A multiple-choice question (AskUserQuestion) is
// read with its options; what is said picks one, confirmed the same way.
// Nothing reaches an agent without a yes, spoken or clicked.
const VC_FOLLOW = 'sbxw-concierge-follow';
const vcAgents = new Map(); // `${sandbox}::${thread}` → chat-core state
let vcPending = null;        // what the next words are for (see vcHear)
// Follow is off by default: on, every turn the agent ends is read aloud,
// which is too much for ordinary work. A /grill-me is followed regardless.
let vcFollow = false;
try { vcFollow = localStorage.getItem(VC_FOLLOW) === '1'; } catch (_) {}

// A grill is what the voice is for: a prompt starting /grill-me (or
// /grilling), or a reply in its round format, engages the concierge even
// with Follow off — and opens the panel, since what arrives must be seen.
const VC_GRILL_PROMPT = /^\s*\/(grill-me|grilling)\b/i;
const VC_GRILL_REPLY = /❓\s*\*\*Q\d+/;
function vcEngages(info, reply) {
  return vcFollow || VC_GRILL_PROMPT.test(info.last_input || '') || VC_GRILL_REPLY.test(reply || '');
}

function vcModeOf(thread) { return thread ? `thread:${thread}` : 'claude'; }
function vcWho(sandbox, thread) { return thread ? `${sandbox} · ${thread}` : sandbox; }

// Arm the next words for `p`, and say so on the card.
function vcArm(p) {
  if (vcPending?.card && vcPending.card !== p.card) vcPending.card.classList.remove('armed');
  vcPending = p;
  if (p?.card) p.card.classList.add('armed');
}
function vcDisarm() {
  if (vcPending?.card) vcPending.card.classList.remove('armed');
  vcPending = null;
}

function vcCard(cls, title) {
  const card = document.createElement('div');
  card.className = `vc-agent ${cls}`;
  const head = document.createElement('div');
  head.className = 'vc-agent-head';
  head.textContent = title;
  const foot = document.createElement('div');
  foot.className = 'vc-agent-foot';
  card.append(head, foot);
  vcLog.append(card);
  vcLog.scrollTop = vcLog.scrollHeight;
  vcNews();
  return { card, head, foot };
}
function vcButton(label, onClick, primary) {
  const b = document.createElement('button');
  b.type = 'button'; b.className = primary ? 'vc-btn vc-primary' : 'vc-btn'; b.textContent = label;
  b.addEventListener('click', onClick);
  return b;
}

// Speak `text` at once, in place of whatever was being read.
function vcSpeak(text) { vcSpeech.stop(); vcSpeech.feed(text); vcSpeech.flush(); }

// An agent's message: shown, read aloud, the next words answer it.
async function vcAgentMessage(sandbox, thread, text) {
  const grill = vcParseGrill(text);
  if (grill.questions.length) return vcGrill(sandbox, thread, text, grill);
  const { card, foot } = vcCard('vc-msg-agent', `${vcWho(sandbox, thread)} says`);
  const body = document.createElement('div');
  body.className = 'vc-agent-body';
  body.innerHTML = chatMarkdown(text);
  card.insertBefore(body, foot);
  const hint = document.createElement('span');
  hint.className = 'vc-armed-hint';
  hint.textContent = 'Your next words answer it.';
  foot.append(
    vcButton('Read aloud', () => vcReadAgent(text)),
    vcButton('Answer it', () => vcArm({ kind: 'reply', sandbox, thread, text, card })),
    vcButton('Not an answer', () => vcDisarm()),
    hint,
  );
  vcArm({ kind: 'reply', sandbox, thread, text, card });
  vcLog.scrollTop = vcLog.scrollHeight;
  if (!vcSpeech.muted) vcReadAgent(text);
}

// ── A grill round, one question at a time ───────────────────────────────
// /grill-me writes its round as `❓ **Q1 - Title** : body` then `➡️ the
// recommendation`, questions separated by `---`. The round is cut there, so
// no question is lost or invented, and asked one
// question at a time: shown, read aloud, answered; then the next. Once every
// one is answered (or skipped), the answers are listed as one message
// ("Q1 : …", as they were said), shown, confirmed, and typed into the
// session at once.
function vcParseGrill(text) {
  const parts = text.split(/^\s*❓\s*/m);
  const intro = parts.shift().trim();
  const questions = [];
  for (const raw of parts) {
    const block = raw.replace(/\n\s*---+\s*$/s, '').trim();
    const [ask, ...rec] = block.split(/^\s*➡️\s*/m);
    const head = ask.match(/^\**\s*(Q\d+)\**\s*[-–—:]?\s*\**\s*([^*\n:]*)\**\s*:?\s*/);
    const id = head ? head[1] : `Q${questions.length + 1}`;
    const title = head ? head[2].trim() : '';
    questions.push({ id, title, ask: ask.trim(), body: head ? ask.slice(head[0].length).trim() : ask.trim(), rec: rec.join(' ').replace(/\n\s*---+[\s\S]*$/, '').trim(), answer: null });
  }
  return { intro, questions };
}

// Markdown without its structure. The symbols left (`_`, `/`, `>=`…) are
// turned into words by the daemon, which holds the dictionary.
function vcPlain(md) {
  return md
    .replace(/```[\s\S]*?```/g, ' ')
    .replace(/`([^`]*)`/g, '$1')
    .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/^\s*\|?[-:| ]+\|\s*$/gm, ' ')   // a table's rule line
    .replace(/^\s*(#+|>|[-•])\s+/gm, '')
    .replace(/^\s*\||\|\s*$/gm, '')          // a table row's outer bars
    .replace(/\*+(?=\S)|(?<=\S)\*+/g, '')    // emphasis; `3 * 4` keeps its star
    .replace(/\s+/g, ' ')
    .trim();
}

const VC_REPEAT = /^\s*(r[ée]p[èe]te|encore|repeat|again)\b/i;
const VC_SKIP = /^\s*(passe|suivante|skip|next|je ne sais pas)\b/i;
const VC_BACK = /^\s*(pr[ée]c[ée]dente|reviens|back|previous)\b/i;

// `relay`: the request id when the round came through `ask_user_questions`
// — the answers then go back as that tool's result, not into a terminal.
function vcGrill(sandbox, thread, text, grill, relay = null) {
  const n = grill.questions.length;
  const { card, head, foot } = vcCard('vc-msg-agent vc-grill', `${vcWho(sandbox, thread)} · ${n} question${n > 1 ? 's' : ''}`);
  if (grill.intro) {
    const ctx = document.createElement('details');
    ctx.className = 'vc-grill-intro';
    const sum = document.createElement('summary');
    sum.textContent = 'Context';
    const b = document.createElement('div');
    b.className = 'vc-agent-body';
    b.innerHTML = chatMarkdown(grill.intro);
    ctx.append(sum, b);
    card.insertBefore(ctx, foot);
  }
  const now = document.createElement('div');
  now.className = 'vc-grill-now';
  const answers = document.createElement('ol');
  answers.className = 'vc-grill-answers';
  card.insertBefore(now, foot);
  card.insertBefore(answers, foot);
  const st = { sandbox, thread, text, grill, i: 0, card, head, relay };

  const show = () => {
    const q = grill.questions[st.i];
    head.textContent = `${vcWho(sandbox, thread)} · ${q.id} (${st.i + 1}/${n})${q.title ? ' — ' + q.title : ''}`;
    now.innerHTML = chatMarkdown(q.body || q.ask) + (q.rec ? `<p class="vc-grill-rec">➡️ ${chatInline(q.rec)}</p>` : '');
    answers.replaceChildren(...grill.questions.slice(0, st.i).map(x => {
      const li = document.createElement('li');
      li.textContent = `${x.id}${x.title ? ' — ' + x.title : ''} : ${x.answer ?? '(skipped)'}`;
      return li;
    }));
    vcArm({ kind: 'grill', st, card });
    vcLog.scrollTop = vcLog.scrollHeight;
    if (!vcSpeech.muted) {
      const said = `${q.id}${q.title ? ', ' + vcPlain(q.title) : ''}. ${vcPlain(q.body)}${q.rec ? ' Recommandation : ' + vcPlain(q.rec) : ''}`;
      vcSpeak(said);
    }
  };
  st.show = show;
  st.next = () => { st.i += 1; st.i < n ? show() : vcGrillDone(st); };
  foot.append(
    vcButton('Repeat', () => show()),
    vcButton('Skip', () => { grill.questions[st.i].answer = null; st.next(); }),
    vcButton('Not an answer', () => vcDisarm()),
  );
  if (relay) {
    // The agent is blocked on its tool call: setting the round aside tells it.
    foot.append(vcButton('Decline', async () => {
      await postJsonOrError(`/api/relay/${encodeURIComponent(relay)}/deny`, { note: 'set aside in the Concierge' });
      vcDisarm(); card.classList.add('done');
      vcSay('dim', `${vcWho(sandbox, thread)} was told you set its questions aside.`);
    }));
  }
  show();
}

// One answer heard for the current question (or a command about it).
function vcGrillHear(st, said) {
  if (st.i >= st.grill.questions.length) return false;
  if (VC_REPEAT.test(said)) { st.show(); return true; }
  if (VC_BACK.test(said)) { st.i = Math.max(0, st.i - 1); st.show(); return true; }
  const q = st.grill.questions[st.i];
  q.answer = VC_SKIP.test(said) ? null : said;
  st.next();
  return true;
}

// Every question answered: one message, confirmed, sent at once.
async function vcGrillDone(st) {
  const { card, grill, sandbox, thread } = st;
  card.querySelector('.vc-grill-now').replaceChildren();
  st.head.textContent = `${vcWho(sandbox, thread)} · ${grill.questions.length} answered`;
  vcDisarm();
  const answered = grill.questions.filter(q => q.answer);
  if (!answered.length) { vcSay('dim', 'No question answered: nothing to send.'); return; }
  const answer = answered.map(q => `${q.id} : ${q.answer}`).join('\n');
  const area = document.createElement('textarea');
  area.className = 'vc-brief-text';
  area.value = answer;
  area.rows = Math.min(12, answer.split('\n').length + 1);
  area.setAttribute('aria-label', 'Answer');
  card.append(area);
  vcConfirm(card, `Send these answers to ${vcWho(sandbox, thread)}?`, async () => {
    // Through the relay when the agent asked with its tool: the answers are
    // that call's result. Otherwise typed into its session.
    const safe = await vcMask(area);
    if (safe === null) return;
    const r = st.relay
      ? await postJsonOrError(`/api/relay/${encodeURIComponent(st.relay)}/approve`, { answer: safe })
      : await postJsonOrError('/api/chat/push', { text: safe, name: sandbox, thread: thread || undefined });
    if (!r.ok) { vcSay('err', `Not sent: ${r.error}`); return; }
    area.readOnly = true;
    card.classList.add('done');
    vcSay('dim', `Answers typed into ${vcWho(sandbox, thread)}.`);
  });
  if (!vcSpeech.muted) vcSpeak("J'ai toutes tes réponses. Relis-les. J'envoie ?");
}

// Read an agent's message as it was written: no summary, so nothing is
// added or lost; code blocks and markdown signs are left out.
function vcReadAgent(text) { vcSpeak(vcPlain(text)); }

// A multiple-choice question: chat-core's buttons, read aloud, answerable by voice.
function vcAgentQuestion(sandbox, thread, c) {
  const { card, foot } = vcCard('vc-ask-agent', `${vcWho(sandbox, thread)} asks`);
  const box = chatQuestion(c, {
    sandbox, mode: vcModeOf(thread), openLabel: 'Answer in its terminal',
    onOpen: () => {}, onChange: () => { vcDisarm(); card.classList.add('done'); },
  });
  card.insertBefore(box, foot);
  const steps = c.steps || [];
  if (steps.length !== 1) {
    foot.textContent = 'Several questions: answer with the buttons.';
    if (!vcSpeech.muted) vcSpeak(`${vcWho(sandbox, thread)} pose ${steps.length} questions ; réponds avec les boutons.`);
    return;
  }
  const step = steps[0];
  const hint = document.createElement('span');
  hint.className = 'vc-armed-hint';
  hint.textContent = 'Say your choice.';
  foot.append(hint);
  vcArm({ kind: 'choose', sandbox, thread, step, card });
  if (!vcSpeech.muted) vcSpeak(`${step.text} ${step.options.map((o, i) => `Option ${i + 1} : ${o}.`).join(' ')}`);
}

// Ask for a yes before `send` runs; a spoken "oui" or the button does it.
function vcConfirm(card, label, send) {
  const bar = document.createElement('div');
  bar.className = 'vc-confirm';
  const q = document.createElement('span');
  q.textContent = label;
  const finish = () => { bar.remove(); vcDisarm(); };
  const yes = vcButton('Send', async () => { finish(); await send(); }, true);
  const no = vcButton('Cancel', () => { finish(); vcSay('dim', 'Not sent.'); });
  bar.append(q, yes, no);
  card.append(bar);
  vcLog.scrollTop = vcLog.scrollHeight;
  vcArm({ kind: 'confirm', card, yes: () => yes.click(), no: () => no.click() });
}

const VC_YES = /^\s*(oui|ouais|ok|okay|d'accord|vas-y|envoie|confirme|yes|send|go)\b/i;
const VC_NO = /^\s*(non|annule|stop|laisse|no|cancel)\b/i;

// Everything heard or typed comes here first: an armed card takes it,
// otherwise it starts a message to the picked sandbox.
function vcHear(text) {
  const p = vcPending;
  if (p?.kind === 'confirm' || p?.kind === 'draft') {
    if (VC_YES.test(text)) return p.yes();
    if (VC_NO.test(text)) return p.no();
  }
  if (p?.kind === 'grill' && vcGrillHear(p.st, text)) return;
  if (p?.kind !== 'grill') vcSay('me', text);
  if (p?.kind === 'draft') return p.add(text);
  if (p?.kind === 'reply') return vcAnswerAgent(p, text);
  if (p?.kind === 'choose') return vcChoose(p, text);
  vcDraft(text);
}

// The words said are the answer: masked, sent at once, and shown as they left.
async function vcAnswerAgent(p, dictated) {
  const { card } = p;
  vcDisarm();
  const area = document.createElement('textarea');
  area.className = 'vc-brief-text';
  area.value = dictated;
  area.rows = Math.min(10, dictated.split('\n').length + 1);
  area.readOnly = true;
  area.setAttribute('aria-label', 'Answer');
  card.append(area);
  const safe = await vcMask(area);
  const r = safe === null ? null : await postJsonOrError('/api/chat/push', { text: safe, name: p.sandbox, thread: p.thread || undefined });
  if (!r?.ok) {
    // The question stays open: say the answer again.
    if (r) vcSay('err', `Not sent: ${r.error}`);
    area.remove();
    vcArm(p);
    return;
  }
  card.classList.add('done');
  vcSay('dim', `Answer typed into ${vcWho(p.sandbox, p.thread)}.`);
  if (!vcSpeech.muted) vcSpeak('Envoyé.');
}

// Which option the words name, 1-based, or 0: a number ("la deux", "option
// 3", "2"), a rank ("la première", "the last one"), or the one option whose
// label shares the most of its words with what was said. No model: a wrong guess
// is still shown and confirmed before anything is sent.
const VC_NUMBERS = [
  ['1', 'un', 'une', 'one', 'premier', 'premiere', 'first'], ['2', 'deux', 'two', 'deuxieme', 'second', 'seconde'],
  ['3', 'trois', 'three', 'troisieme', 'third'], ['4', 'quatre', 'four', 'quatrieme', 'fourth'],
  ['5', 'cinq', 'five', 'cinquieme', 'fifth'], ['6', 'six', 'sixieme', 'sixth'],
];
function vcWords(text) {
  return text.toLowerCase().normalize('NFD').replace(/[\u0300-\u036f]/g, '').split(/[^a-z0-9]+/).filter(Boolean);
}
function vcPickOption(said, options) {
  const words = vcWords(said);
  if (!words.length) return 0;
  if (words.includes('derniere') || words.includes('dernier') || words.includes('last')) return options.length;
  // A label wins over a number when it is all there ("deux colonnes").
  const scores = options.map(o => {
    const label = vcWords(o).filter(w => w.length > 2); // not "a", "de", "to"
    return label.length ? label.filter(w => words.includes(w)).length / label.length : 0;
  });
  const best = Math.max(...scores);
  if (best === 1 && scores.filter(x => x === 1).length === 1) return scores.indexOf(1) + 1;
  const n = VC_NUMBERS.findIndex(names => words.some(w => names.includes(w))) + 1;
  if (n >= 1 && n <= options.length) return n;
  if (best > 0 && scores.filter(x => x === best).length === 1) return scores.indexOf(best) + 1;
  return 0;
}

function vcChoose(p, dictated) {
  const { step } = p;
  const n = vcPickOption(dictated, step.options);
  if (!n) {
    vcSay('dim', `“${dictated}” matches no option — say its number, or click it.`);
    if (!vcSpeech.muted) vcSpeak("Je n'ai pas reconnu l'option. Dis son numéro, ou clique dessus.");
    return;
  }
  const label = step.options[n - 1];
  const btn = p.card.querySelectorAll('.cd-opts button')[n - 1];
  p.card.querySelectorAll('.cd-opts button').forEach(b => b.classList.toggle('picked', b === btn));
  vcConfirm(p.card, `Answer “${label}”?`, async () => { btn?.click(); });
  if (!vcSpeech.muted) vcSpeak(`J'envoie : ${label} ?`);
}

// ── Questions asked through the relay (`ask_user_questions`) ────────────
// Any sandbox may ask: these are put to the person, whichever project the
// panel is on. Each round opens the panel on its card, voice on or off: the
// agent is blocked until the person answers, so the round must be seen, not
// only heard. Opening moves no keyboard focus, so a terminal being typed in
// keeps it. Other agent messages (the `session` stream) leave the panel shut.
const vcAsks = new Map(); // relay id → its card
function vcAskRelay(req) {
  if (req.kind !== 'ask') return;
  if (req.state === 'approved' || req.state === 'denied') {
    const card = vcAsks.get(req.id);
    if (card && !card.classList.contains('done')) {
      card.classList.add('done');
      if (vcPending?.card === card) vcDisarm();
    }
    return;
  }
  if (req.state !== 'pending' || vcAsks.has(req.id)) return;
  const grill = vcParseGrill(req.question);
  if (!grill.questions.length) grill.questions.push({ id: 'Q1', title: '', ask: req.question, body: req.question, rec: '', answer: null });
  // Open first: `vcSetOpen` warms the voice and calls no `focus()`.
  if (!vcOpen) vcSetOpen(true); else vcWarm(!vcSpeech.muted);
  vcGrill(req.from, req.from_thread || null, req.question, grill, req.id);
  const card = vcPending?.card || vcLog.lastElementChild;
  vcAsks.set(req.id, card);
  // Its card in view and marked (`armed`, set by vcGrill): within the log
  // only, so the page itself does not scroll.
  if (card) vcLog.scrollTop += card.getBoundingClientRect().top - vcLog.getBoundingClientRect().top;
}
sbxwStream.addEventListener('relay', ev => {
  try { vcAskRelay(JSON.parse(ev.data)); } catch (_) {}
});
fetch('/api/relay').then(r => r.json()).then(d => (d.requests || []).forEach(vcAskRelay)).catch(() => {});

sbxwStream.addEventListener('session', ev => {
  let info;
  try { info = JSON.parse(ev.data); } catch (_) { return; }
  // Every session sbxw drives in that sandbox: its main agent, and the
  // threads and sessions it named (SBXW_THREAD, which read as `remote`
  // since they are not the main agent). A client of its own — Claude
  // Desktop, an ssh shell — has no thread and stays out.
  if (!vcTarget || info.sandbox !== vcTarget) return;
  if (info.origin === 'remote' && !info.thread) return;
  const key = `${info.sandbox}::${info.thread || ''}`;
  if (!vcAgents.has(key)) vcAgents.set(key, chatNew());
  const c = vcAgents.get(key);
  const seen = chatIngest(c, info);
  if (!seen || !vcEngages(info, seen.reply)) return;
  vcWarm(!vcSpeech.muted);
  if (seen.reply) vcAgentMessage(info.sandbox, info.thread || null, seen.reply);
  else if (seen.asked && c.steps?.length && c.answerable) vcAgentQuestion(info.sandbox, info.thread || null, c);
});

const vcFollowBtn = vcEl('vc-follow');
function vcRenderFollow() {
  vcFollowBtn.classList.toggle('on', vcFollow);
  vcFollowBtn.title = vcFollow
    ? "Following this sandbox's agents: every message and question shows here and is read aloud"
    : 'Only a /grill-me is followed — click to follow every turn';
  vcFollowBtn.setAttribute('aria-pressed', String(vcFollow));
}
vcFollowBtn.addEventListener('click', () => {
  vcFollow = !vcFollow;
  try { localStorage.setItem(VC_FOLLOW, vcFollow ? '1' : '0'); } catch (_) {}
  if (!vcFollow) vcDisarm();
  vcRenderFollow();
});
vcRenderFollow();

sbxwStream.addEventListener('voice', ev => {
  try { vcStatus = JSON.parse(ev.data); } catch (_) { return; }
  vcRender();
});

vcRefresh();
