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
      vcAsk(r.text);
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
function vcSave() {
  try { sessionStorage.setItem(VC_STORE, JSON.stringify(vcTurns.slice(-80))); } catch (_) {}
}
function vcLoad() {
  try { vcTurns = JSON.parse(sessionStorage.getItem(VC_STORE) || '[]'); } catch (_) { vcTurns = []; }
  for (const t of vcTurns) vcSay(t.role === 'user' ? 'me' : 'ai', t.content);
}

// What the person said goes to the local model; its reply is written into
// the log as it is generated (NDJSON from /api/voice/chat).
async function vcAsk(text) {
  if (vcReplying) vcReplying.abort();
  vcSpeech.stop();
  // A reply that failed left the user's turn unanswered: fold into it.
  if (vcTurns.length && vcTurns[vcTurns.length - 1].role === 'user') vcTurns[vcTurns.length - 1].content += `\n${text}`;
  else vcTurns.push({ role: 'user', content: text });
  vcSave();
  const ctl = new AbortController();
  vcReplying = ctl;
  const row = vcSay('ai', '');
  row.classList.add('pending');
  let reply = '';
  try {
    const res = await fetch('/api/voice/chat', {
      method: 'POST', headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ turns: vcTurns }), signal: ctl.signal,
    });
    if (!res.ok) {
      const e = await res.json().catch(() => ({}));
      throw new Error(e.error || `HTTP ${res.status}`);
    }
    const reader = res.body.getReader();
    const dec = new TextDecoder();
    let buf = '';
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      buf += dec.decode(value, { stream: true });
      let nl;
      while ((nl = buf.indexOf('\n')) >= 0) {
        const ev = JSON.parse(buf.slice(0, nl));
        buf = buf.slice(nl + 1);
        if (ev.kind === 'delta') { reply += ev.text; row.firstChild.textContent = reply; vcLog.scrollTop = vcLog.scrollHeight; vcSpeech.feed(ev.text); }
        else if (ev.kind === 'error') throw new Error(ev.error);
        else if (ev.kind === 'done') { vcMeta(row, `${(ev.ms / 1000).toFixed(1)} s`); vcSpeech.flush(); }
      }
    }
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

function vcMeta(row, text) {
  const m = document.createElement('span');
  m.className = 'vc-meta'; m.textContent = text;
  row.append(m);
}

function vcReset() {
  if (vcReplying) vcReplying.abort();
  vcSpeech.stop();
  vcTurns = [];
  vcSave();
  vcLog.replaceChildren();
}

// Typing is the same conversation, for when speaking is not an option.
vcEl('vc-type').addEventListener('submit', e => {
  e.preventDefault();
  const input = vcEl('vc-type-input');
  const text = input.value.trim();
  if (!text || !vcReady()) return;
  input.value = '';
  vcSay('me', text);
  vcAsk(text);
});
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
  if (open) vcLoad();
vcRefresh();
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

sbxwStream.addEventListener('voice', ev => {
  try { vcStatus = JSON.parse(ev.data); } catch (_) { return; }
  vcRender();
});

vcRefresh();
