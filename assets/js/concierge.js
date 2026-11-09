// ── Voice concierge ───────────────────────────────────────────────────────
// A local voice partner, apart from the chat dock (which talks to the
// `ephemeral-chat` sandboxes): a 🎙 button beside it opens its own panel.
// Lot 1 of `.sbxw-artifacts/specs/concierge-vocal.md` — speech becomes text
// here; the conversation, the voice and the brief come next.
//
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
// The conversation, this tab's own: [{role: 'user'|'assistant', content}].
// The daemon keeps none of it (lot 3 adds the 7-day history).
const VC_STORE = 'sbxw-concierge';
let vcTurns = [];
let vcReplying = null; // AbortController of the reply being generated

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
  vcHint.textContent = ready ? 'Speech is turned into text locally; the audio is never kept.' : '';
  vcLauncher.classList.toggle('ready', vcReady());
  vcLauncher.classList.toggle('listening', !!vcRec);
  vcLauncher.querySelector('kbd').textContent = vcKey();
}

function vcSay(who, text, meta) {
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
async function vcStart() {
  vcSpeech.stop(); // speaking over the concierge interrupts it
  vcWarm();
  if (vcRec || vcBusy || !vcReady() || !window.isSecureContext) return;
  let stream;
  try {
    stream = await navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true } });
  } catch (e) {
    vcSay('err', `No microphone: ${e.message || e}`);
    return;
  }
  const recorder = new MediaRecorder(stream);
  const rec = { recorder, chunks: [], stream, started: performance.now() };
  recorder.ondataavailable = e => { if (e.data.size) rec.chunks.push(e.data); };
  recorder.start();
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
    if (!r.ok) vcSay('err', r.error);
    else if (!r.text) vcSay('dim', '(nothing heard)');
    else {
      vcSay('me', r.text, `${r.audio_s.toFixed(1)} s · ${r.ms} ms`);
      vcHear(r.text);
    }
  } catch (e) {
    vcSay('err', `Could not transcribe: ${e.message || e}`);
  } finally {
    vcBusy = false;
    vcRender();
  }
}

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

// ── The conversation ─────────────────────────────────────────────────────
// The tab keeps the conversation (sessionStorage, for a reload) and the
// daemon keeps its text for at most a week (`/api/voice/conversations`),
// saved a moment after each change. The audio is never kept anywhere.
let vcConvId = null;
let vcPersistTimer = null;
function vcNewId() {
  const b = new Uint8Array(8);
  crypto.getRandomValues(b);
  return 'c-' + [...b].map(x => x.toString(16).padStart(2, '0')).join('');
}
function vcSave() {
  if (!vcConvId) vcConvId = vcNewId();
  try { sessionStorage.setItem(VC_STORE, JSON.stringify({ id: vcConvId, turns: vcTurns.slice(-80) })); } catch (_) {}
  clearTimeout(vcPersistTimer);
  if (!vcTurns.length) return;
  const body = { id: vcConvId, sandbox: vcTarget || undefined, turns: vcTurns.slice(-200) };
  vcPersistTimer = setTimeout(() => postJsonOrError('/api/voice/conversations', body), 800);
}
function vcShow(turns) {
  vcLog.replaceChildren();
  for (const t of turns) vcSay(t.role === 'user' ? 'me' : 'ai', t.content);
}
function vcLoad() {
  try {
    const s = JSON.parse(sessionStorage.getItem(VC_STORE) || 'null');
    // Before conversations had ids the store held the bare list.
    if (Array.isArray(s)) vcTurns = s;
    else if (s) { vcConvId = s.id || null; vcTurns = s.turns || []; }
  } catch (_) { vcTurns = []; }
  vcShow(vcTurns);
}

// One generation from /api/voice/chat, read as it streams (NDJSON): each
// piece of text to `onDelta`, then the `done` event returned.
async function vcStream(body, signal, onDelta) {
  const res = await fetch('/api/voice/chat', {
    method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body), signal,
  });
  if (!res.ok) {
    const e = await res.json().catch(() => ({}));
    throw new Error(e.error || `HTTP ${res.status}`);
  }
  const reader = res.body.getReader();
  const dec = new TextDecoder();
  let buf = '';
  let last = null;
  for (;;) {
    const { value, done } = await reader.read();
    if (done) return last;
    buf += dec.decode(value, { stream: true });
    let nl;
    while ((nl = buf.indexOf('\n')) >= 0) {
      const ev = JSON.parse(buf.slice(0, nl));
      buf = buf.slice(nl + 1);
      if (ev.kind === 'delta') onDelta(ev.text);
      else if (ev.kind === 'error') throw new Error(ev.error);
      else if (ev.kind === 'done') last = ev;
    }
  }
}

// The person's words join the conversation: appended to an unanswered turn
// (a reply that failed), else as a new one — user and assistant alternate.
function vcAddUser(text) {
  if (vcTurns.length && vcTurns[vcTurns.length - 1].role === 'user') vcTurns[vcTurns.length - 1].content += `\n${text}`;
  else vcTurns.push({ role: 'user', content: text });
  vcSave();
}

// "Prépare le brief", spoken or typed, writes the brief instead of a reply.
const VC_BRIEF_RE = /\bbrief\b/i;
const VC_BRIEF_VERB = /(pr[ée]pare|r[ée]dige|[ée]cri[st]|fais|g[ée]n[èe]re|draft|write|prepare)/i;
function vcRoute(text) {
  if (VC_BRIEF_RE.test(text) && VC_BRIEF_VERB.test(text)) vcBrief(text);
  else vcAsk(text);
}

// What the person said goes to the local model; its reply is written into
// the log as it is generated, and spoken sentence by sentence.
async function vcAsk(text) {
  if (vcReplying) vcReplying.abort();
  vcSpeech.stop();
  vcAddUser(text);
  const ctl = new AbortController();
  vcReplying = ctl;
  const row = vcSay('ai', '');
  row.classList.add('pending');
  let reply = '';
  try {
    const done = await vcStream({ turns: vcTurns, sandbox: vcTarget || undefined }, ctl.signal, piece => {
      reply += piece;
      row.firstChild.textContent = reply;
      vcLog.scrollTop = vcLog.scrollHeight;
      vcSpeech.feed(piece);
    });
    vcSpeech.flush();
    if (done) vcMeta(row, `${(done.ms / 1000).toFixed(1)} s`);
    row.classList.remove('pending');
    if (reply.trim()) { vcTurns.push({ role: 'assistant', content: reply.trim() }); vcSave(); }
    else row.remove();
  } catch (e) {
    row.remove();
    if (e.name !== 'AbortError') vcSay('err', `The concierge could not answer: ${e.message || e}`);
  } finally {
    if (vcReplying === ctl) vcReplying = null;
  }
}

// ── The brief ────────────────────────────────────────────────────────────
// The model writes it (Objectif / Contexte / Contraintes / Questions
// ouvertes) into an editable card; the person reads it, fixes it, picks the
// sandbox and presses Send. Nothing reaches a sandbox any other way: the
// click is the yes (rule 1 of the spec, the relay's own rule).
const VC_TARGET = 'sbxw-concierge-target';

// Asked of the daemon each time: the page's own list is the one it loaded
// with, and misses a sandbox started since (it took a reload to appear).
async function vcTargets() {
  let list = typeof sandboxes !== 'undefined' ? sandboxes : [];
  try {
    const fresh = await fetch('/api/sandboxes').then(r => r.json());
    if (Array.isArray(fresh)) list = fresh;
  } catch (_) {}
  const live = list.filter(s => s.status === 'running' && !s.unreachable_since).map(s => s.name);
  return [...new Set(live)].sort();
}

// ── The project: which sandbox the conversation is about ────────────────
// Picked at the top, remembered per browser. Its code map's summary.md and
// its .sbxw-artifacts/concierge/prompt.md are what the model is told (the
// daemon reads them; the page only names the sandbox), and the brief goes
// to it.
const vcPick = vcEl('vc-target');
const vcCtxInfo = vcEl('vc-ctx-info');
const vcMapBtn = vcEl('vc-map');
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
  vcLoadContext();
}

async function vcLoadContext() {
  vcMapBtn.hidden = true;
  if (!vcTarget) { vcCtxInfo.textContent = 'no project context'; return; }
  let c;
  try { c = await fetch(`/api/voice/context?sandbox=${encodeURIComponent(vcTarget)}`).then(r => r.json()); } catch (_) { c = null; }
  if (!c || !c.ok) { vcCtxInfo.textContent = 'context unavailable'; return; }
  const parts = [];
  if (c.summary_words) parts.push(`summary, ${c.summary_words} words`);
  else if (!c.local) parts.push('remote: no context');
  else parts.push(c.has_map ? 'map has no summary' : 'no code map');
  if (c.has_prompt) parts.push('your prompt');
  vcCtxInfo.textContent = parts.join(' · ');
  if (c.local && !c.summary_words) {
    vcMapBtn.hidden = false;
    vcMapBtn.textContent = c.has_map ? 'Refresh the map' : 'Map this project';
    vcMapBtn.disabled = !!c.busy;
    vcMapBtn.title = c.busy ? 'Its agent is busy: it is never interrupted' : `Types /codemap into ${vcTarget}'s agent`;
  }
}

vcPick.addEventListener('change', () => {
  vcTarget = vcPick.value || null;
  try { vcTarget ? localStorage.setItem(VC_TARGET, vcTarget) : localStorage.removeItem(VC_TARGET); } catch (_) {}
  vcLoadContext();
});
vcMapBtn.addEventListener('click', async () => {
  if (!vcTarget) return;
  vcMapBtn.disabled = true;
  const r = await postJsonOrError('/api/voice/map', { sandbox: vcTarget });
  if (!r.ok) { vcSay('err', r.error); vcMapBtn.disabled = false; return; }
  vcSay('dim', `/codemap typed into ${vcTarget}'s agent. The summary is used as soon as the map is written.`);
});

async function vcBrief(text) {
  if (!vcTurns.some(t => t.role === 'user') && !text) { vcSay('err', 'Talk the idea through first: the brief is written from the conversation.'); return; }
  if (vcReplying) vcReplying.abort();
  vcSpeech.stop();
  vcAddUser(text || 'Prépare le brief.');
  const ctl = new AbortController();
  vcReplying = ctl;

  const card = document.createElement('div');
  card.className = 'vc-brief';
  const head = document.createElement('div');
  head.className = 'vc-brief-head';
  head.textContent = 'Brief';
  const area = document.createElement('textarea');
  area.className = 'vc-brief-text';
  area.readOnly = true;
  area.rows = 12;
  area.setAttribute('aria-label', 'Brief');
  const row = document.createElement('div');
  row.className = 'vc-brief-row';
  // The sandbox the conversation is about (picked at the top) receives it.
  const target = vcTarget;
  const pick = document.createElement('span');
  pick.className = 'vc-brief-to';
  pick.textContent = target ? `to ${target}` : 'pick a sandbox at the top first';
  const send = document.createElement('button');
  send.type = 'button'; send.className = 'vc-btn vc-primary'; send.textContent = 'Send'; send.disabled = true;
  const drop = document.createElement('button');
  drop.type = 'button'; drop.className = 'vc-btn'; drop.textContent = 'Discard';
  row.append(pick, send, drop);
  card.append(head, area, row);
  vcLog.append(card);
  vcLog.scrollTop = vcLog.scrollHeight;

  drop.onclick = () => { ctl.abort(); card.remove(); };
  send.onclick = async () => {
    const name = target;
    const brief = area.value.trim();
    if (!name || !brief) return;
    send.disabled = true; drop.disabled = true; area.readOnly = true;
    const r = await postJsonOrError('/api/chat/push', { text: brief, name });
    if (!r.ok) {
      vcSay('err', `Could not send the brief to ${name}: ${r.error}`);
      send.disabled = false; drop.disabled = false; area.readOnly = false;
      return;
    }
    card.classList.add('sent');
    head.textContent = `Brief — sent to ${name}`;
    vcSay('dim', `Brief typed into ${name}'s agent.`);
  };

  try {
    let text = '';
    await vcStream({ turns: vcTurns, brief: true, sandbox: vcTarget || undefined }, ctl.signal, piece => {
      text += piece;
      area.value = text;
      area.scrollTop = area.scrollHeight;
    });
    area.value = text.trim();
    area.readOnly = false;
    send.disabled = !target || !area.value;
    vcTurns.push({ role: 'assistant', content: "J'ai préparé le brief ; il attend ta relecture et ton envoi." });
    vcSave();
    vcSpeech.feed('Le brief est prêt. Relis-le, choisis le sandbox, puis envoie-le.');
    vcSpeech.flush();
  } catch (e) {
    card.remove();
    if (e.name !== 'AbortError') vcSay('err', `Could not write the brief: ${e.message || e}`);
  } finally {
    if (vcReplying === ctl) vcReplying = null;
  }
}

function vcMeta(row, text) {
  const m = document.createElement('span');
  m.className = 'vc-meta'; m.textContent = text;
  row.append(m);
}

function vcReset() {
  if (vcReplying) vcReplying.abort();
  vcDisarm();
  vcSpeech.stop();
  vcTurns = [];
  vcConvId = null;
  vcSave();
  vcLog.replaceChildren();
}

// ── History: the week's conversations, reopened or deleted ──────────────
const vcHistory = vcEl('vc-history');
async function vcOpenHistory() {
  if (!vcHistory.hidden) { vcHistory.hidden = true; return; }
  const r = await fetch('/api/voice/conversations').then(r => r.json()).catch(() => null);
  vcHistory.replaceChildren();
  const head = document.createElement('div');
  head.className = 'vc-history-head';
  const t = document.createElement('span');
  t.textContent = `Kept ${r?.retention_days ?? 7} days at most, text only`;
  const all = vcButton('Delete all', async () => {
    const d = await postJsonOrError('/api/voice/conversations/rm', { all: true });
    vcHistory.hidden = true;
    vcReset();
    try { sessionStorage.removeItem(VC_STORE); } catch (_) {}
    vcSay('dim', d.ok ? `${d.deleted} conversation(s) deleted.` : `Not deleted: ${d.error}`);
  });
  head.append(t, all);
  vcHistory.append(head);
  const list = r?.conversations || [];
  if (!list.length) {
    const e = document.createElement('p');
    e.className = 'vc-dim'; e.textContent = 'No conversation kept.';
    vcHistory.append(e);
  }
  for (const c of list) {
    const row = document.createElement('div');
    row.className = 'vc-history-row' + (c.id === vcConvId ? ' current' : '');
    const open = document.createElement('button');
    open.type = 'button'; open.className = 'vc-history-open';
    const when = new Date(c.updated_ms).toLocaleString([], { weekday: 'short', hour: '2-digit', minute: '2-digit' });
    open.textContent = `${c.title || '(empty)'}`;
    open.title = `${when}${c.sandbox ? ' · ' + c.sandbox : ''} · ${c.turns} turns`;
    open.addEventListener('click', async () => {
      const g = await fetch(`/api/voice/conversations/${encodeURIComponent(c.id)}`).then(r => r.json()).catch(() => null);
      if (!g?.ok) { vcSay('err', g?.error || 'Could not open it'); return; }
      if (vcReplying) vcReplying.abort();
      vcSpeech.stop(); vcDisarm();
      vcConvId = g.conversation.id;
      vcTurns = g.conversation.turns.map(t => ({ role: t.role, content: t.content }));
      try { sessionStorage.setItem(VC_STORE, JSON.stringify({ id: vcConvId, turns: vcTurns })); } catch (_) {}
      vcShow(vcTurns);
      vcHistory.hidden = true;
    });
    const meta = document.createElement('span');
    meta.className = 'vc-meta'; meta.textContent = `${when}${c.sandbox ? ' · ' + c.sandbox : ''}`;
    const rm = document.createElement('button');
    rm.type = 'button'; rm.className = 'cd-close'; rm.textContent = '✕'; rm.title = 'Delete';
    rm.addEventListener('click', async () => {
      await postJsonOrError('/api/voice/conversations/rm', { id: c.id });
      if (c.id === vcConvId) vcReset();
      row.remove();
    });
    open.append(meta);
    row.append(open, rm);
    vcHistory.append(row);
  }
  vcHistory.hidden = false;
}
vcEl('vc-history-btn').addEventListener('click', vcOpenHistory);

// Typing is the same conversation, for when speaking is not an option.
vcEl('vc-type').addEventListener('submit', e => {
  e.preventDefault();
  const input = vcEl('vc-type-input');
  const text = input.value.trim();
  if (!text || !vcReady()) return;
  input.value = '';
  vcSay('me', text);
  vcHear(text);
});
vcEl('vc-brief').addEventListener('click', () => { if (vcReady()) vcBrief(''); });
vcEl('vc-new').addEventListener('click', vcReset);

// ── The voice ────────────────────────────────────────────────────────────
// The reply is spoken sentence by sentence as it streams: a sentence is sent
// to /api/voice/speak as soon as it ends, the next one is synthesised while
// this one plays, and they play in order. Muting (🔇, remembered per browser)
// keeps the text and skips the sound; talking again stops the voice at once.
const VC_MUTE = 'sbxw-concierge-mute';
const vcSpeech = (() => {
  let buf = '';
  let gen = 0;            // bumped by stop(): late audio of an old reply is dropped
  let chain = Promise.resolve();
  let ctx = null;
  let playing = null;     // the AudioBufferSourceNode playing now
  let muted = false;
  try { muted = localStorage.getItem(VC_MUTE) === '1'; } catch (_) {}

  // A sentence ends at . ! ? … or a line break, followed by space or the end
  // (so "3.5" and "e.g." mid-word do not cut).
  const END = /[.!?…\n]+["»)\]]?(\s|$)/;
  function say(text) {
    text = text.replace(/[*_#`>]/g, '').trim();
    if (!text || muted || !vcReady()) return;
    const my = gen;
    const audio = fetch('/api/voice/speak', {
      method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ text }),
    }).then(r => r.ok ? r.arrayBuffer() : r.json().then(e => { throw new Error(e.error || r.status); }));
    chain = chain.then(async () => {
      let bytes;
      try { bytes = await audio; } catch (e) { if (my === gen) vcSay('err', `Could not speak: ${e.message || e}`); return; }
      if (my !== gen) return;
      ctx = ctx || new AudioContext();
      const decoded = await ctx.decodeAudioData(bytes);
      if (my !== gen) return;
      await new Promise(done => {
        const src = ctx.createBufferSource();
        src.buffer = decoded;
        src.connect(ctx.destination);
        src.onended = done;
        playing = src;
        src.start();
      });
      playing = null;
    });
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
    },
    flush() { say(buf); buf = ''; },
    stop() {
      gen++;
      buf = '';
      chain = Promise.resolve();
      if (playing) { try { playing.stop(); } catch (_) {} playing = null; }
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

const vcMuteBtn = vcEl('vc-mute');
function vcRenderMute() {
  vcMuteBtn.textContent = vcSpeech.muted ? '🔇' : '🔊';
  vcMuteBtn.title = vcSpeech.muted ? 'Voice off — click to hear replies' : 'Voice on — click to mute';
  vcMuteBtn.setAttribute('aria-pressed', String(vcSpeech.muted));
}
vcMuteBtn.addEventListener('click', () => { vcSpeech.toggle(); vcRenderMute(); });
vcRenderMute();

// ── Wiring ───────────────────────────────────────────────────────────────
function vcSetOpen(open) {
  vcOpen = open;
  vcPanel.hidden = !open;
  vcLauncher.setAttribute('aria-expanded', String(open));
  if (open) { vcRefresh(); vcWarm(); vcFillTargets(); }
}

// Load the models before they are needed: opening the panel or pressing the
// key usually comes seconds before the first word, and that is long enough.
let vcWarmed = 0;
function vcWarm() {
  if (!vcReady() || Date.now() - vcWarmed < 60_000) return;
  vcWarmed = Date.now();
  fetch('/api/voice/warm', { method: 'POST' }).catch(() => {});
}

vcLauncher.addEventListener('click', () => vcSetOpen(!vcOpen));
vcEl('vc-close').addEventListener('click', () => vcSetOpen(false));

vcTalk.addEventListener('pointerdown', e => { e.preventDefault(); vcTalk.setPointerCapture(e.pointerId); vcStart(); });
vcTalk.addEventListener('pointerup', () => vcStop());
vcTalk.addEventListener('pointercancel', () => vcStop());

// The key works from anywhere on the page, terminals included: captured
// before xterm sees it. Holding it opens the panel, so the text lands in view.
window.addEventListener('keydown', e => {
  if (e.code !== vcKey() || e.repeat) return;
  e.preventDefault(); e.stopPropagation();
  if (!vcOpen) vcSetOpen(true);
  vcStart();
}, true);
window.addEventListener('keyup', e => {
  if (e.code !== vcKey()) return;
  e.preventDefault(); e.stopPropagation();
  vcStop();
}, true);
window.addEventListener('keydown', e => {
  if (e.key === 'Escape' && vcOpen && !vcRec) vcSetOpen(false);
});

// ── Following the sandbox's agent: the grill by voice ───────────────────
// The picked sandbox's agents are followed on the session stream (the chat
// dock's own reader, chat-core.js). When one ends its turn with a message —
// a /grill-me round, a question in prose — it shows here, its gist is read
// aloud, and the next thing said answers it: the model turns the dictation
// into the written answer, which is shown and confirmed before it is typed
// into that session. A multiple-choice question (AskUserQuestion) is read
// with its options; what is said picks one, confirmed the same way. Nothing
// reaches an agent without a yes, spoken or clicked.
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

// One task for the model, its whole output returned (speech, reply, choose).
async function vcTask(task, content) {
  let out = '';
  await vcStream({ turns: [{ role: 'user', content }], task, sandbox: vcTarget || undefined }, undefined, p => { out += p; });
  return out.trim();
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
  return { card, head, foot };
}
function vcButton(label, onClick, primary) {
  const b = document.createElement('button');
  b.type = 'button'; b.className = primary ? 'vc-btn vc-primary' : 'vc-btn'; b.textContent = label;
  b.addEventListener('click', onClick);
  return b;
}

// Speak `text` at once (a short line of the concierge's own).
function vcSpeak(text) { vcSpeech.stop(); vcSpeech.feed(text); vcSpeech.flush(); }

// An agent's message: shown, its gist read aloud, the next words answer it.
async function vcAgentMessage(sandbox, thread, text) {
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
    vcButton('Talk to the concierge', () => vcDisarm()),
    hint,
  );
  vcArm({ kind: 'reply', sandbox, thread, text, card });
  vcLog.scrollTop = vcLog.scrollHeight;
  if (!vcSpeech.muted) vcReadAgent(text);
}

async function vcReadAgent(text) {
  try { vcSpeak(await vcTask('speech', text)); } catch (e) { vcSay('err', `Could not read it: ${e.message || e}`); }
}

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
// otherwise it is a turn of the conversation (or the brief).
async function vcHear(text) {
  const p = vcPending;
  if (p?.kind === 'confirm') {
    if (VC_YES.test(text)) return p.yes();
    if (VC_NO.test(text)) return p.no();
  }
  if (p?.kind === 'reply') return vcAnswerAgent(p, text);
  if (p?.kind === 'choose') return vcChoose(p, text);
  vcRoute(text);
}

async function vcAnswerAgent(p, dictated) {
  const { card } = p;
  let answer;
  try {
    answer = await vcTask('reply', `Message de l'agent :\n<<<\n${p.text}\n>>>\n\nRéponse dictée par le développeur :\n${dictated}`);
  } catch (e) { vcSay('err', `Could not write the answer: ${e.message || e}`); return; }
  if (!answer) answer = dictated;
  const area = document.createElement('textarea');
  area.className = 'vc-brief-text';
  area.value = answer;
  area.rows = Math.min(10, answer.split('\n').length + 1);
  area.setAttribute('aria-label', 'Answer');
  card.append(area);
  vcConfirm(card, `Send this to ${vcWho(p.sandbox, p.thread)}?`, async () => {
    const r = await postJsonOrError('/api/chat/push', { text: area.value.trim(), name: p.sandbox, thread: p.thread || undefined });
    if (!r.ok) { vcSay('err', `Not sent: ${r.error}`); return; }
    area.readOnly = true;
    card.classList.add('done');
    vcSay('dim', `Answer typed into ${vcWho(p.sandbox, p.thread)}.`);
  });
  if (!vcSpeech.muted) vcSpeak("Voici ta réponse. J'envoie ?");
}

async function vcChoose(p, dictated) {
  const { step } = p;
  let n = 0;
  try {
    const out = await vcTask('choose', `Question : ${step.text}\nOptions :\n${step.options.map((o, i) => `${i + 1}. ${o}`).join('\n')}\n\nRéponse dictée : ${dictated}`);
    n = parseInt(out, 10) || 0;
  } catch (e) { vcSay('err', `Could not understand the choice: ${e.message || e}`); return; }
  if (n < 1 || n > step.options.length) {
    vcSay('dim', `“${dictated}” matches no option — say one of them, or click it.`);
    if (!vcSpeech.muted) vcSpeak("Je n'ai pas reconnu l'option. Redis-la, ou clique dessus.");
    return;
  }
  const label = step.options[n - 1];
  const btn = p.card.querySelectorAll('.cd-opts button')[n - 1];
  p.card.querySelectorAll('.cd-opts button').forEach(b => b.classList.toggle('picked', b === btn));
  vcConfirm(p.card, `Answer “${label}”?`, async () => { btn?.click(); });
  if (!vcSpeech.muted) vcSpeak(`J'envoie : ${label} ?`);
}

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
  if (!vcOpen) vcSetOpen(true);
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

vcLoad();
vcRefresh();
