// ── sbxw remote: the phone page's core ─────────────────────────────────────
// Everything of the phone page that does not touch the document: how an
// agent's words become safe markup, how a history page becomes chat rows,
// the order of the leaders, the pairing states, and the one client every
// request goes through. remote.js wires it to the page; the tests in
// tests/js/remote-*.test.mjs load this file alone.
//
// This page is served by the daemon's REMOTE listener, to paired devices
// only, and shares no script with the main web UI at runtime. The four
// functions between the COPY markers are copied verbatim from
// assets/js/chat-core.js (a test keeps them equal): agent text is escaped
// first, then only a fixed Markdown subset adds markup of its own.
// `files` is always null here: there is no Files tab on the phone, so a
// path stays plain text.

// ── COPY from assets/js/chat-core.js: begin ──
function chatEscape(s) {
  return s.replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

function chatInline(s, files) {
  const codes = [];
  const links = [];
  const fileButton = (found, label) => {
    const i = files.paths.push(found) - 1;
    return `<button type="button" class="chat-file" data-file="${i}">${chatEscape(label)}</button>`;
  };
  let text = s
    .replace(/[\uE000-\uE003]/g, '')
    .replace(/`([^`\n]+)`/g, (_, code) => {
      const found = files && chatFilePath(code.trim());
      if (found) return `\uE002${links.push(fileButton(found, code.trim())) - 1}\uE003`;
      return `\uE000${codes.push(code) - 1}\uE001`;
    });
  if (files) {
    let out = '';
    let at = 0;
    for (const t of chatFileTokens(text)) {
      out += text.slice(at, t.start) + `\uE002${links.push(fileButton(t.found, text.slice(t.start, t.end))) - 1}\uE003`;
      at = t.end;
    }
    text = out + text.slice(at);
  }
  const plain = h => h.replace(/<button[^>]*>([^<]*)<\/button>/, '$1');
  return chatEscape(text)
    .replace(/\*\*([^*\n]+)\*\*/g, '<strong>$1</strong>')
    .replace(/(^|[^*\w])\*([^*\s][^*\n]*?)\*(?![\w*])/g, '$1<em>$2</em>')
    .replace(/\[([^\]\n]+)\]\((https?:\/\/[^\s)\uE000-\uE003]+)\)/g, (_, label, url) =>
      `<a href="${url}" target="_blank" rel="noopener noreferrer">${label.replace(/\uE002(\d+)\uE003/g, (_m, n) => plain(links[n]))}</a>`)
    .replace(/\uE000(\d+)\uE001/g, (_, n) => `<code>${chatEscape(codes[n])}</code>`)
    .replace(/\uE002(\d+)\uE003/g, (_, n) => links[n]);
}

function chatMarkdown(text, files) {
  return text.split(/```[^\n]*\n([\s\S]*?)```/).map((part, i) => {
    if (i % 2) return `<pre><code>${chatEscape(part.replace(/\n$/, ''))}</code></pre>`;
    return part.split(/\n\s*\n/).map(b => chatBlock(b, files)).join('');
  }).join('');
}

function chatBlock(block, files) {
  let html = '';
  let para = [];
  let list = null; // { tag, start, items }
  const flushPara = () => {
    if (para.length) html += `<p>${para.join('<br>')}</p>`;
    para = [];
  };
  const flushList = () => {
    if (list) {
      const start = list.start > 1 ? ` start="${list.start}"` : '';
      html += `<${list.tag}${start}>${list.items.map(i => `<li>${i}</li>`).join('')}</${list.tag}>`;
    }
    list = null;
  };
  for (const line of block.split('\n')) {
    if (!line.trim()) continue;
    const heading = /^#{1,6}\s+(.*)$/.exec(line);
    const bullet = /^\s*[-*+]\s+(.*)$/.exec(line);
    const numbered = /^\s*(\d+)[.)]\s+(.*)$/.exec(line);
    if (heading) {
      flushPara();
      flushList();
      html += `<p><strong>${chatInline(heading[1], files)}</strong></p>`;
    } else if (bullet || numbered) {
      flushPara();
      const tag = bullet ? 'ul' : 'ol';
      if (!list || list.tag !== tag) {
        flushList();
        list = { tag, start: numbered ? Number(numbered[1]) : 1, items: [] };
      }
      list.items.push(chatInline(bullet ? bullet[1] : numbered[2], files));
    } else if (list && /^\s{2,}\S/.test(line)) {
      // An indented line under an item carries on that item.
      list.items[list.items.length - 1] += ' ' + chatInline(line.trim(), files);
    } else {
      flushList();
      para.push(chatInline(line, files));
    }
  }
  flushPara();
  flushList();
  return html;
}

// ── COPY from assets/js/chat-core.js: end ──

// An agent's text as markup: the fixed subset above, then each link shows its
// own address beside its label, so a tap never goes somewhere the words hid.
// chatInline only ever makes a link of an http(s) address; remote.js checks
// the scheme once more on the element before the link is live.
const RC_LINK = /<a href="([^"]*)" target="_blank" rel="noopener noreferrer">([\s\S]*?)<\/a>/g;
function rcAgentHtml(text) {
  const src = String(text ?? '').replace(/\r\n?/g, '\n');
  return chatMarkdown(src, null).replace(RC_LINK, (_, url, label) =>
    `<a class="rc-link" href="${url}" target="_blank" rel="noopener noreferrer">${label} <span class="rc-url">${url}</span></a>`);
}

// ── History → chat rows ───────────────────────────────────────────────────
// A page of a leader's history (oldest first) as the rows the conversation
// shows. The remote history carries no tool input and no tool result, only
// a count of steps: a run of steps is one quiet line with its total. A
// message another session wrote (`injected` with a `from`) sits on the
// agent's side under its sender's name; one from sbxw itself is neutral.
function rcRows(items) {
  const rows = [];
  let steps = null;
  for (const it of Array.isArray(items) ? items : []) {
    if (!it || typeof it !== 'object') continue;
    const kind = it.kind;
    if (kind === 'steps' || kind === 'tool_call') {
      if (!steps) { steps = { kind: 'steps', count: 0 }; rows.push(steps); }
      steps.count += kind === 'steps' ? Math.max(1, Math.floor(Number(it.count)) || 1) : 1;
      continue;
    }
    if (kind === 'tool_result') continue;
    const text = typeof it.text === 'string' ? it.text : '';
    const at = Number(it.at) || 0;
    let row = null;
    if (kind === 'user_prompt' && text) row = { kind: 'me', text };
    else if (kind === 'assistant_text' && text) row = { kind: 'agent', text };
    else if (kind === 'injected' && text) {
      const from = typeof it.from === 'string' && it.from && it.from !== 'sbxw' ? it.from : null;
      row = from ? { kind: 'agent', from, text } : { kind: 'sbxw', text };
    } else if (kind === 'question') {
      const q = Array.isArray(it.steps) && it.steps[0] && typeof it.steps[0].text === 'string' ? it.steps[0].text : text;
      row = { kind: 'asked', text: q || 'The leader asked a question.' };
    } else if (kind === 'permission') row = { kind: 'mac', text: text || 'The leader asked to allow a tool.' };
    else if (kind === 'notice' && text) row = { kind: 'note', text };
    else if (kind === 'compact_marker') row = { kind: 'note', text: 'Conversation compacted' };
    if (!row) continue;
    steps = null;
    row.at = at;
    row.seq = Number(it.seq) || 0;
    rows.push(row);
  }
  return rows;
}

// `have` and a newer or older page, by seq, oldest first. A provisional
// reply (the Stop hook's copy) gives way once the transcript's items arrive.
function rcMerge(have, incoming) {
  const fresh = (Array.isArray(incoming) ? incoming : []).filter(i => i && Number.isFinite(Number(i.seq)));
  const settles = fresh.some(i => !i.provisional);
  const bySeq = new Map();
  for (const i of have) if (!(settles && i.provisional)) bySeq.set(Number(i.seq), i);
  for (const i of fresh) bySeq.set(Number(i.seq), i);
  return [...bySeq.values()].sort((a, b) => Number(a.seq) - Number(b.seq));
}

// A history answer, as `{ items, next_before, latest_seq }`: the page may
// come bare or under `page`.
function rcPage(data) {
  const p = data && typeof data === 'object' ? (data.page && typeof data.page === 'object' ? data.page : data) : {};
  return {
    items: Array.isArray(p.items) ? p.items : [],
    next_before: Number.isFinite(Number(p.next_before)) && p.next_before !== null ? Number(p.next_before) : null,
    latest_seq: Number(p.latest_seq) || 0,
  };
}

// ── Leaders ───────────────────────────────────────────────────────────────
// Those that need the person first, then those at work, then the rest; the
// most recent first within each.
const RC_RANK = { attention: 0, working: 1, idle: 2 };
const RC_STATE = { attention: 'needs you', working: 'working', idle: 'idle' };
function rcLeaders(data) {
  const list = Array.isArray(data) ? data : Array.isArray(data?.leaders) ? data.leaders : [];
  return list
    .filter(l => l && typeof l.sandbox === 'string' && l.sandbox)
    .map(l => ({ ...l, name: typeof l.name === 'string' && l.name ? l.name : l.sandbox }))
    .sort((a, b) => rcRank(a) - rcRank(b)
      || (Number(b.last_at) || 0) - (Number(a.last_at) || 0)
      || a.name.localeCompare(b.name));
}
function rcRank(l) {
  return l.asks ? 0 : RC_RANK[l.state] ?? 3;
}
function rcStateLabel(state) {
  return RC_STATE[state] || 'stopped';
}

// "now", "4 min", "2 h", "3 d": the last activity, short enough for a row.
function rcAgo(ms, now) {
  const t = Number(ms);
  if (!t) return '';
  const s = Math.max(0, Math.round((now - t) / 1000));
  if (s < 60) return 'now';
  if (s < 3600) return `${Math.floor(s / 60)} min`;
  if (s < 86400) return `${Math.floor(s / 3600)} h`;
  return `${Math.floor(s / 86400)} d`;
}

// What a leader waits on, for the card under its conversation: a question
// the phone can answer (structured steps, typed by sbxw), or something only
// the Mac can settle (a permission prompt, a plan to approve), or nothing.
function rcPending(leader) {
  if (!leader || !(leader.asks || leader.state === 'attention')) return null;
  const steps = Array.isArray(leader.steps) ? leader.steps.filter(s => s && typeof s.text === 'string' && Array.isArray(s.options) && s.options.length) : [];
  if (steps.length && leader.answerable) return { kind: 'question', steps };
  return { kind: 'mac', text: steps[0]?.text || 'The leader waits for something only the Mac can settle, such as allowing a tool.' };
}

// A leader's activity: one of the daemon's fixed phrases ("Working", "Asks a
// question", "Waits for you at the Mac", "Waiting for a message"), never the
// session's own line. Left out where it only repeats the state shown beside it.
function rcActivity(leader) {
  const a = leader && typeof leader.activity === 'string' ? leader.activity.trim() : '';
  if (!a) return '';
  const said = leader.asks ? 'needs you' : rcStateLabel(leader.state);
  return a.toLowerCase() === said || (leader.state === 'working' && a.toLowerCase() === 'working') ? '' : a;
}

// ── Pairing ───────────────────────────────────────────────────────────────
// The pairing code arrives in the URL's fragment, never its query, so it is
// in no request line and no Referer: `#pair=<code>` (API.md 2.2).
function rcPairCode(hash) {
  const m = /^#?(?:.*&)?pair=([A-Za-z0-9_-]{4,128})(?:&|$)/.exec(String(hash || ''));
  return m ? m[1] : null;
}

// The words for each pairing state.
const RC_PAIR_TEXT = {
  pending: 'In sbxw on your Mac, type these six digits to pair this device.',
  paired: 'Paired.',
  refused: 'Pairing was refused on the Mac (or the number was typed wrong three times).',
  expired: 'This request expired. Open the new link from your Mac.',
  limited: 'Too many attempts. Wait a minute, then show a new code on the Mac.',
  invalid: 'This pairing link is not valid any more. Show a new one on the Mac.',
  unreachable: 'sbxw is unreachable (is the Mac awake?)',
  failed: 'Pairing failed. Show a new code on the Mac and try again.',
};

// A pair/start or pair/status answer as one state name.
function rcPairState(res, step) {
  if (res.unreachable || res.status === 0) return 'unreachable';
  if (res.status === 429) return 'limited';
  if (step === 'start') {
    if (res.ok && res.data && typeof res.data.request_id === 'string') return 'pending';
    if (res.status === 403 || res.status === 410) return 'invalid';
    return res.status >= 400 && res.status < 500 ? 'invalid' : 'failed';
  }
  const s = res.data && (res.data.status || res.data.state);
  if (res.ok && ['pending', 'refused', 'expired', 'paired'].includes(s)) return s;
  if (res.status === 404 || res.status === 410) return 'expired';
  return 'failed';
}

// A name for this device to offer at pairing; the person can change it.
function rcDeviceName(ua, platform) {
  const s = `${ua || ''} ${platform || ''}`;
  if (/iPhone/.test(s)) return 'iPhone';
  if (/iPad/.test(s) || (/Macintosh/.test(s) && /Mobile/.test(s))) return 'iPad';
  if (/Android/.test(s)) return /Mobile/.test(s) ? 'Android phone' : 'Android tablet';
  return 'Phone';
}
const RC_NAME_MAX = 40;
function rcCleanName(s) {
  return String(s || '').replace(/\s+/g, ' ').replace(/[\u0000-\u001f\u007f]/g, '').trim().slice(0, RC_NAME_MAX);
}

// ── The client ────────────────────────────────────────────────────────────
// Every request of the page: same origin, under /r/, JSON (or raw audio for
// dictation), the device cookie the only credential, nothing in a URL but
// a sandbox name and a cursor. Never retried and never queued: a request the
// Mac did not answer is reported, and the person decides again.
//
// `hooks.reach(bool)` hears whether sbxw answered; `hooks.unpaired()` a 401
// outside pairing (the device was forgotten or revoked).
function rcClient(fetchImpl, hooks = {}) {
  const call = async (method, path, body, raw) => {
    if (!path.startsWith('/r/')) throw new Error('not a remote route');
    const init = { method, credentials: 'same-origin', cache: 'no-store', redirect: 'error', headers: {} };
    if (raw !== undefined) {
      init.headers['Content-Type'] = 'application/octet-stream';
      init.body = raw;
    } else if (body !== undefined) {
      init.headers['Content-Type'] = 'application/json';
      init.body = JSON.stringify(body);
    }
    let res;
    try {
      res = await fetchImpl(path, init);
    } catch (_) {
      hooks.reach?.(false);
      return { ok: false, status: 0, unreachable: true, data: null };
    }
    // The sidecar answers 502/504 when the daemon behind it is gone.
    const unreachable = res.status === 502 || res.status === 504;
    hooks.reach?.(!unreachable);
    let data = null;
    try { data = await res.json(); } catch (_) {}
    if (res.status === 401 && !path.startsWith('/r/pair/')) hooks.unpaired?.();
    return { ok: res.ok, status: res.status, unreachable, data };
  };
  const at = s => `/r/leaders/${encodeURIComponent(s)}`;
  return {
    me: () => call('GET', '/r/me'),
    logout: () => call('POST', '/r/logout', {}),
    pairStart: (code, deviceName) => call('POST', '/r/pair/start', { code, device_name: deviceName }),
    pairStatus: id => call('GET', `/r/pair/status?request_id=${encodeURIComponent(id)}`),
    leaders: () => call('GET', '/r/leaders'),
    history: (s, q = {}) => {
      const p = new URLSearchParams();
      if (q.before != null) p.set('before', String(q.before));
      if (q.after != null) p.set('after', String(q.after));
      if (q.limit != null) p.set('limit', String(q.limit));
      const qs = p.toString();
      return call('GET', `${at(s)}/history${qs ? `?${qs}` : ''}`);
    },
    // Attachments are the daemon's upload ids, never a path: the daemon adds
    // the paths itself. Without attachments the body is exactly { text }.
    message: (s, text, attachments) => call('POST', `${at(s)}/message`,
      Array.isArray(attachments) && attachments.length ? { text, attachments: attachments.map(String) } : { text }),
    dropUpload: (s, id) => call('DELETE', `${at(s)}/upload/${encodeURIComponent(id)}`),
    answer: (s, indices) => call('POST', `${at(s)}/answer`, { indices }),
    interrupt: s => call('POST', `${at(s)}/interrupt`, {}),
    transcribe: pcm => call('POST', '/r/voice/transcribe', undefined, pcm),
    // For the level-2 routes (rcL2Client): the same call, nothing more.
    get: path => call('GET', path),
    post: (path, body) => call('POST', path, body),
  };
}

// A message's refusal reasons (API.md 2.6), each as a plain sentence. The
// daemon's own `error` is shown beside it, as text.
const RC_MSG_REASON = {
  command: 'Commands cannot be sent from the phone: no line may start with !, / or #.',
  control: 'The message holds a control or invisible character (a tab, a zero-width space, a soft hyphen...). Remove the invisible characters, or retype the text.',
  empty: 'The message is empty.',
  needs_mac: 'This needs your Mac: the leader is waiting for you there.',
  busy: 'The leader is working. A message can be sent once it is back at its prompt; meanwhile you can read it and interrupt it.',
  no_paste: 'Send it from your Mac: this leader\'s terminal cannot take a message from the phone.',
  attachment: 'Not sent: a file of this message is no longer on the Mac (it expires 30 minutes after its upload). Remove it, choose it again, then Send.',
  too_many: 'Not sent: a message takes 5 files at most. Remove some, then Send.',
  not_submitted: 'NOT SENT: the text was typed into the leader\'s input on the Mac but not submitted, because the leader changed meanwhile. It waits there unsent; it is also still in this field. Check on your Mac before sending again.',
};

// What the daemon will refuse, checked before sending so the person can fix
// it in place (the daemon stays the judge): a line whose first non-blank
// character is !, / or #, or any control character but a line feed (tab, CR,
// ESC, DEL, C1), a format character (bidi controls, zero-width space and
// joiners, BOM, soft hyphen, word joiner, tags), a line or paragraph separator.
const RC_CONTROL = /(?!\n)[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u;
function rcMessageCheck(text) {
  const s = String(text ?? '');
  if (!s.trim()) return 'empty';
  if (s.split('\n').some(line => /^\s*[!/#]/.test(line))) return 'command';
  if (RC_CONTROL.test(s)) return 'control';
  return null;
}

// Why a request did not go through, in the person's words. `what`: "Message".
function rcFailText(what, res) {
  if (res.unreachable || res.status === 0) return `${what} not sent: sbxw is unreachable (is the Mac awake?).`;
  const reason = res.data && typeof res.data.reason === 'string' && Object.hasOwn(RC_MSG_REASON, res.data.reason) ? res.data.reason : null;
  if (reason) {
    const err = typeof res.data.error === 'string' && res.data.error.trim() ? ` (sbxw: ${res.data.error.trim().slice(0, 200)})` : '';
    return `${RC_MSG_REASON[reason]}${err}`;
  }
  if (res.status === 409) return `${what} not sent: the leader is not waiting for that any more.`;
  if (res.status === 413) return `${what} not sent: it is too long.`;
  if (res.status === 429) return `${what} not sent: too many at once. Try again in a moment.`;
  const err = res.data && typeof res.data.error === 'string' ? res.data.error.slice(0, 200) : `the Mac answered ${res.status}`;
  return `${what} not sent: ${err}.`;
}

// Why Send waits, or '' when it can go: a message is typed only while the
// leader is idle at its prompt (API.md v1.3). Never a queue: the text stays
// in the field until the person presses Send again.
function rcSendGate(leader) {
  if (!leader) return 'This leader is not running.';
  if (leader.state === 'working') return 'The leader is working: Send comes back at its prompt.';
  return '';
}

// Dictation: the longest recording the daemon accepts (MAX_SAMPLES at
// 16 kHz in crates/daemon/src/svc/voice.rs), and the rate it wants.
const RC_VOICE_RATE = 16000;
const RC_VOICE_MAX_S = 120;

// Dictated words go INTO the field, after what is there, for the person to
// read and correct. Nothing is sent.
function rcWithDictation(field, words) {
  const w = String(words || '').trim();
  if (!w) return field;
  return field && !/\s$/.test(field) ? `${field} ${w}` : `${field}${w}`;
}

// ── Files: the attach rules ──────────────────────────────────────────────
// The list of secret-looking names and the helpers between the COPY markers
// are copied verbatim from assets/js/chat-core.js's "chat attach" block (a
// test keeps them equal). The page checks before any byte is sent; the
// daemon stays the judge. A file's CONTENT is never read here, only handed
// to the upload; a file's NAME is data, shown with textContent only.
// ── COPY from assets/js/chat-core.js (chat attach): begin ──
const CHAT_SECRET_NAMES = ['.env*', '*.pem', '*.key', 'id_rsa*', 'id_ed25519*', '*.p12', '*.pfx', 'credentials*', '.npmrc', '.netrc', '*.kdbx'];

function chatSecretName(name) {
  const n = String(name || '').toLowerCase();
  return CHAT_SECRET_NAMES.some(p => (p.startsWith('*') ? n.endsWith(p.slice(1))
    : p.endsWith('*') ? n.startsWith(p.slice(0, -1)) : n === p));
}

function chatSize(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB'];
  let v = bytes / 1024, i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return `${v.toFixed(v < 10 ? 1 : 0)} ${units[i]}`;
}

function chatShortName(name) {
  const s = [...String(name ?? '').replace(/[\u0000-\u001f\u007f-\u009f\u202a-\u202e\u2066-\u2069]/g, '?')];
  return s.length <= 60 ? s.join('') : `${s.slice(0, 40).join('')}…${s.slice(-16).join('')}`;
}

function chatUploadName(name) {
  const base = String(name ?? '').split(/[\/\\]/).pop().replace(/[\u0000-\u001f\u007f]/g, '');
  const s = [...base];
  if (s.length <= 100) return s.join('');
  const ext = /\.[A-Za-z0-9]{1,10}$/.exec(base)?.[0] || '';
  return s.slice(0, 90).join('') + ext;
}
// ── COPY from assets/js/chat-core.js (chat attach): end ──

// The limits the page checks, API.md 2.6.1 (the daemon enforces them too):
// 25 MiB a file, 5 files waiting per device and leader, 60 MiB together.
const RC_UPLOAD_MAX_FILES = 5;
const RC_UPLOAD_MAX_BYTES = 25 * 1024 * 1024;
const RC_UPLOAD_MAX_TOTAL = 60 * 1024 * 1024;
const RC_TICK = 'I have looked at these files and want to attach them';

// The checks, before any byte is sent. `held`: how many files are already
// attached or waiting, `heldBytes` their size together.
// → { take, refused: [{name, why}], ask: [sentence] }
function rcCheckAttach(held, incoming, heldBytes = 0) {
  const take = [], refused = [];
  let bytes = Number(heldBytes) || 0;
  for (const f of Array.isArray(incoming) ? incoming : []) {
    if (!f || typeof f !== 'object') continue;
    const size = Number(f.size);
    let why = '';
    if (!(size > 0)) why = 'the file is empty';
    else if (size > RC_UPLOAD_MAX_BYTES) why = `over ${chatSize(RC_UPLOAD_MAX_BYTES)}, the most one file takes`;
    else if (held + take.length >= RC_UPLOAD_MAX_FILES) why = `a message takes ${RC_UPLOAD_MAX_FILES} files at most`;
    else if (bytes + size > RC_UPLOAD_MAX_TOTAL) why = `the files of one message take ${chatSize(RC_UPLOAD_MAX_TOTAL)} together at most`;
    if (why) refused.push({ name: String(f.name ?? ''), why }); else { take.push(f); bytes += size; }
  }
  const ask = take.filter(f => chatSecretName(f.name)).map(f => `${chatShortName(f.name)}: its name looks like a secret (keys, tokens, credentials)`);
  return { take, refused, ask };
}

// An upload's refusal reasons (API.md 2.6.1), each as a short sentence for
// its chip.
const RC_UPLOAD_REASON = {
  too_large: `Too large: one file takes ${chatSize(RC_UPLOAD_MAX_BYTES)} at most.`,
  too_many: `Too many files wait on the Mac (${RC_UPLOAD_MAX_FILES} at most, ${chatSize(RC_UPLOAD_MAX_TOTAL)} together). Remove one, or send them first.`,
  empty: 'The file is empty.',
  kind: 'Programs and archives are not accepted (zip, gzip, executables...).',
  name: 'The Mac could not read this file\'s name. Rename it, then choose it again.',
  busy: 'Another upload from this phone is under way. Retry once it is done.',
  timeout: 'The connection was too slow for this file. Try again on a better connection, or send a smaller file.',
};
// A body the network did not carry in time (the Tailscale node gives a whole
// body 120 s: a 25 MiB file needs about 1.7 Mbit/s): no answer at all, the
// sidecar's error (no JSON, no reason), the daemon's 400 "cut off" or its
// `timeout`. One sentence for all, and Retry stays the person's: each try
// counts toward the daemon's 20 uploads per 10 minutes.
const RC_UPLOAD_SLOW = 'The connection was too slow for this file. Try again on a better connection, or send a smaller file.';
function rcUploadFail(res) {
  if (!res || res.unreachable || res.status === 0) return RC_UPLOAD_SLOW;
  const reason = res.data && typeof res.data.reason === 'string' && Object.hasOwn(RC_UPLOAD_REASON, res.data.reason) ? res.data.reason : null;
  if (reason) return RC_UPLOAD_REASON[reason];
  if (!res.ok && (!res.data || typeof res.data !== 'object')) return RC_UPLOAD_SLOW;
  if (res.status === 400 && /cut off/i.test(String(res.data.error || ''))) return RC_UPLOAD_SLOW;
  if (res.status === 413) return RC_UPLOAD_REASON.too_large;
  if (res.status === 429) return 'Not uploaded: too many uploads in a short time. Retry in a few minutes.';
  if (res.status === 404) return 'Not uploaded: this leader does not take files from the phone.';
  return `Not uploaded: the Mac answered ${res.status}.`;
}
// The daemon's answer, checked: an id (no path, ever), its own name, a size.
const RC_UPLOAD_ID = /^[0-9a-f]{32}$/;
function rcUploadDone(data) {
  if (!data || typeof data.id !== 'string' || !RC_UPLOAD_ID.test(data.id)) return null;
  return { id: data.id, name: typeof data.name === 'string' ? data.name : '', size: Number(data.size) || 0 };
}

// One file as the raw body, by XMLHttpRequest (it reports how much of the
// body has gone; fetch does not). Same origin, the cookie, nothing in the URL
// but the sandbox and the name. Resolves like the client: { ok, status,
// unreachable, data }; never rejects, never retried.
function rcUploadXhr(Xhr, sandbox, file, onProgress, signal) {
  return new Promise(resolve => {
    const x = new Xhr();
    x.open('POST', `/r/leaders/${encodeURIComponent(sandbox)}/upload?name=${encodeURIComponent(chatUploadName(file.name))}`);
    x.setRequestHeader('Content-Type', 'application/octet-stream');
    if (x.upload) x.upload.onprogress = e => { if (e.lengthComputable && e.total) onProgress(Math.min(1, e.loaded / e.total)); };
    x.onload = () => {
      let data = null;
      try { data = JSON.parse(x.responseText); } catch (_) {}
      const unreachable = x.status === 502 || x.status === 504;
      resolve({ ok: x.status >= 200 && x.status < 300, status: x.status, unreachable, data });
    };
    x.onerror = () => resolve({ ok: false, status: 0, unreachable: true, data: null });
    x.onabort = () => resolve({ ok: false, status: 0, aborted: true, data: null });
    signal?.addEventListener('abort', () => x.abort());
    x.send(file);
  });
}

// The composer's files: chips (queued → sending → done | failed), one upload
// at a time, and secret-looking names held back until an explicit tick.
// `upload(file, onProgress, signal)` resolves like rcUploadXhr; `drop(id)`
// tells the daemon a done file is not wanted any more; `onChange` redraws.
// A message carries the ids of the done chips only.
function rcTray({ upload, drop = () => {}, onChange = () => {} }) {
  const t = {
    chips: [], held: null, note: '', seq: 0, active: null,
    offer(files) {
      const waiting = [...t.chips.filter(c => c.state !== 'failed'), ...(t.held ? t.held.files : [])];
      const { take, refused, ask } = rcCheckAttach(waiting.length, files, waiting.reduce((n, c) => n + (Number(c.size) || 0), 0));
      t.note = refused.map(r => `Not attached: ${chatShortName(r.name)} (${r.why}).`).join(' ');
      if (take.length && ask.length) {
        // More files held means a new look: the tick starts again.
        t.held = { files: [...(t.held ? t.held.files : []), ...take], reasons: [...(t.held ? t.held.reasons : []), ...ask] };
      } else if (take.length) t.start(take);
      onChange();
    },
    // Only an explicit tick sends what was held.
    confirmHeld(ticked) {
      if (!t.held) return false;
      if (ticked !== true) { t.note = `Tick "${RC_TICK}" first: nothing is sent before.`; onChange(); return false; }
      const files = t.held.files;
      t.held = null;
      t.note = '';
      t.start(files);
      onChange();
      return true;
    },
    discardHeld() { t.held = null; onChange(); },
    start(files) {
      for (const file of files) t.chips.push({ key: ++t.seq, file, name: String(file.name ?? ''), size: Number(file.size) || 0, state: 'queued', pct: 0, id: null, error: '' });
      t.pump();
    },
    pump() {
      if (t.active) return;
      const chip = t.chips.find(c => c.state === 'queued');
      if (!chip) return;
      const ctl = new AbortController();
      chip.state = 'sending';
      chip.pct = 0;
      t.active = { chip, ctl };
      Promise.resolve()
        .then(() => upload(chip.file, p => { if (t.active?.chip === chip) { chip.pct = p; onChange(); } }, ctl.signal))
        .then(res => {
          if (t.active?.chip !== chip) return;
          const done = res && res.ok ? rcUploadDone(res.data) : null;
          if (done) Object.assign(chip, { state: 'done', pct: 1, id: done.id, size: done.size || chip.size });
          else Object.assign(chip, { state: 'failed', error: res && res.ok ? 'Not uploaded: the Mac gave no file id.' : rcUploadFail(res) });
        }, () => {
          if (t.active?.chip === chip) Object.assign(chip, { state: 'failed', error: rcUploadFail(null) });
        })
        .then(() => {
          if (t.active?.chip === chip) t.active = null;
          t.pump();
          onChange();
        });
    },
    remove(key) {
      const i = t.chips.findIndex(c => c.key === key);
      if (i < 0) return;
      const [chip] = t.chips.splice(i, 1);
      if (t.active?.chip === chip) { const a = t.active; t.active = null; a.ctl.abort(); t.pump(); }
      if (chip.state === 'done' && chip.id) drop(chip.id);
      onChange();
    },
    retry(key) {
      const chip = t.chips.find(c => c.key === key);
      if (!chip || chip.state !== 'failed') return;
      chip.state = 'queued';
      chip.error = '';
      t.pump();
      onChange();
    },
    pending() { return t.chips.filter(c => c.state === 'queued' || c.state === 'sending').length; },
    failed() { return t.chips.filter(c => c.state === 'failed').length; },
    ids() { return t.chips.filter(c => c.state === 'done' && c.id).map(c => c.id); },
    // After a message went: the files it carried leave the tray.
    sent(ids) { t.chips = t.chips.filter(c => !(c.state === 'done' && ids.includes(c.id))); onChange(); },
    // Leaving the leader: drop everything not sent (aborting an upload).
    clear() {
      if (t.active) { t.active.ctl.abort(); t.active = null; }
      for (const c of t.chips) if (c.state === 'done' && c.id) drop(c.id);
      t.chips = []; t.held = null; t.note = '';
      onChange();
    },
  };
  return t;
}

// ── The status line under the field ───────────────────────────────────────
// One line says what the composer waits on, the most urgent first: the
// outcome of the last action (a refusal, "Sent."), then the recorder, the
// files, why Send waits, and last a hint. → { text, tone }: tone is err,
// warn, ok, busy or info.
function rcStatus({ note = null, rec = 'idle', gate = '', held = 0, pending = 0, failed = 0, files = 0, hint = '' } = {}) {
  if (note && note.text) return { text: note.text, tone: note.tone || 'info' };
  if (rec === 'recording') return { text: 'Recording. Tap stop when you are done: the words go into the field, nothing is sent.', tone: 'busy' };
  if (rec === 'transcribing') return { text: 'Transcribing on your Mac…', tone: 'busy' };
  if (held) return { text: `Tick the box to attach ${held === 1 ? 'the file whose name looks' : `the ${held} files whose names look`} like a secret, or cancel.`, tone: 'info' };
  if (pending) return { text: `Uploading ${pending === 1 ? '1 file' : `${pending} files`}: Send waits until ${pending === 1 ? 'it is' : 'they are'} done.`, tone: 'busy' };
  if (gate) return { text: files ? `${gate.replace(/\.$/, '')}. Your files wait here.` : gate, tone: 'info' };
  if (failed) return { text: `${failed === 1 ? 'An upload' : `${failed} uploads`} failed: retry or remove. The rest can be sent.`, tone: 'err' };
  return { text: hint || '', tone: 'info' };
}

// ── The recorder's helpers ────────────────────────────────────────────────
// How loud a frame of time-domain samples is (an AnalyserNode's bytes, 128 is
// silence), as 0 to 1 for the wave. Levels only: the samples are not kept.
function rcLevel(bytes) {
  const n = bytes && bytes.length ? bytes.length : 0;
  if (!n) return 0;
  let sum = 0;
  for (let i = 0; i < n; i++) { const v = (bytes[i] - 128) / 128; sum += v * v; }
  const rms = Math.sqrt(sum / n);
  return rms < 0.01 ? 0 : Math.min(1, Math.sqrt(rms * 3.5));
}
// "0:07", "1:42": the running time of a recording.
function rcClock(seconds) {
  const s = Math.max(0, Math.floor(Number(seconds) || 0));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}
// The wave a recorder shows: bars that move with the voice, a plain level
// meter with reduced motion, a steady pulse without an analyser.
function rcWaveMode(hasAnalyser, reducedMotion) {
  if (!hasAnalyser) return 'pulse';
  return reducedMotion ? 'meter' : 'bars';
}

// ══ Level 2: approving from the phone with a passkey ═════════════════════
// The contract is .sbxw-artifacts/remote/API-level2.md. Everything below is
// pure: the page wires it to navigator.credentials and the document.
// What it never does: keep a challenge, an assertion or a credential id
// beyond the one request (they live in the caller's locals), or turn
// agent-written text into markup.

// ── base64url without padding (API-level2.md section 2) ──
const RC_B64U = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';
function rcB64u(buf) {
  const b = buf instanceof Uint8Array ? buf : ArrayBuffer.isView(buf) ? new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength) : new Uint8Array(buf);
  let out = '';
  for (let i = 0; i < b.length; i += 3) {
    const n = (b[i] << 16) | ((b[i + 1] ?? 0) << 8) | (b[i + 2] ?? 0);
    out += RC_B64U[(n >> 18) & 63] + RC_B64U[(n >> 12) & 63];
    if (i + 1 < b.length) out += RC_B64U[(n >> 6) & 63];
    if (i + 2 < b.length) out += RC_B64U[n & 63];
  }
  return out;
}
// Strict: only the alphabet, no padding, a valid length; else null.
function rcFromB64u(s) {
  if (typeof s !== 'string' || !/^[A-Za-z0-9_-]*$/.test(s) || s.length % 4 === 1) return null;
  const out = new Uint8Array(Math.floor(s.length * 3 / 4));
  let o = 0;
  for (let i = 0; i < s.length; i += 4) {
    const c = [0, 1, 2, 3].map(k => (i + k < s.length ? RC_B64U.indexOf(s[i + k]) : 0));
    const n = (c[0] << 18) | (c[1] << 12) | (c[2] << 6) | c[3];
    out[o++] = (n >> 16) & 255;
    if (i + 2 < s.length) out[o++] = (n >> 8) & 255;
    if (i + 3 < s.length) out[o++] = n & 255;
  }
  return out;
}

// ── The two ceremonies' options, from the daemon's JSON ──
// Only the fields the contract names pass, the binary ones decoded; the
// page always asks for user verification, whatever came.
function rcCreateOptions(pk) {
  if (!pk || typeof pk !== 'object') return null;
  const challenge = rcFromB64u(pk.challenge);
  const userId = rcFromB64u(pk.user?.id);
  if (!challenge || challenge.length < 16 || !userId || !userId.length) return null;
  if (typeof pk.rp?.id !== 'string' || !pk.rp.id) return null;
  const exclude = Array.isArray(pk.excludeCredentials) ? pk.excludeCredentials.map(c => rcFromB64u(c?.id)) : [];
  if (exclude.some(x => !x)) return null;
  const params = (Array.isArray(pk.pubKeyCredParams) ? pk.pubKeyCredParams : []).filter(p => p && p.type === 'public-key' && p.alg === -7);
  if (!params.length) return null;
  return {
    rp: { id: pk.rp.id, name: typeof pk.rp.name === 'string' ? pk.rp.name : 'sbxw' },
    user: { id: userId, name: String(pk.user.name ?? ''), displayName: String(pk.user.displayName ?? pk.user.name ?? '') },
    challenge,
    pubKeyCredParams: [{ type: 'public-key', alg: -7 }],
    authenticatorSelection: { authenticatorAttachment: 'platform', residentKey: 'preferred', userVerification: 'required' },
    attestation: 'none',
    timeout: Math.min(Number(pk.timeout) || 120000, 300000),
    excludeCredentials: exclude.map(id => ({ type: 'public-key', id })),
  };
}
function rcGetOptions(pk) {
  if (!pk || typeof pk !== 'object') return null;
  const challenge = rcFromB64u(pk.challenge);
  if (!challenge || challenge.length < 16 || typeof pk.rpId !== 'string' || !pk.rpId) return null;
  const allow = Array.isArray(pk.allowCredentials) ? pk.allowCredentials.map(c => rcFromB64u(c?.id)) : [];
  // The device's credential only: an empty list would let any passkey answer.
  if (!allow.length || allow.some(x => !x)) return null;
  return {
    challenge,
    rpId: pk.rpId,
    allowCredentials: allow.map(id => ({ type: 'public-key', id })),
    userVerification: 'required',
    timeout: Math.min(Number(pk.timeout) || 60000, 120000),
  };
}
// The browser's answers as the daemon's JSON; null for any other shape.
const rcIsBuf = v => v instanceof ArrayBuffer || ArrayBuffer.isView(v) || Object.prototype.toString.call(v) === '[object ArrayBuffer]';
function rcRegistrationBody(cred) {
  const r = cred && cred.response;
  if (!cred || cred.type !== 'public-key' || !rcIsBuf(cred.rawId) || !r || !rcIsBuf(r.clientDataJSON) || !rcIsBuf(r.attestationObject)) return null;
  return { id: rcB64u(cred.rawId), client_data_json: rcB64u(r.clientDataJSON), attestation_object: rcB64u(r.attestationObject) };
}
function rcAssertionBody(cred) {
  const r = cred && cred.response;
  if (!cred || cred.type !== 'public-key' || !rcIsBuf(cred.rawId) || !r || !rcIsBuf(r.clientDataJSON) || !rcIsBuf(r.authenticatorData) || !rcIsBuf(r.signature)) return null;
  return { id: rcB64u(cred.rawId), client_data_json: rcB64u(r.clientDataJSON), authenticator_data: rcB64u(r.authenticatorData), signature: rcB64u(r.signature) };
}
// A ceremony that did not finish, as one state: the person cancelled (or the
// sheet timed out), no platform authenticator, or something else.
function rcCeremonyError(e) {
  const n = e && e.name;
  if (n === 'NotAllowedError' || n === 'AbortError') return 'cancelled';
  if (n === 'NotSupportedError' || n === 'SecurityError') return 'no_authenticator';
  if (n === 'InvalidStateError') return 'exists';
  return 'failed';
}

// ── Sentences ──
// The Approvals entry, when it cannot act: one sentence, nothing else.
function rcApprovalsBlock(a) {
  if (!a || a.level2 !== true) return 'Approving is turned off on your Mac.';
  if (a.passkey !== true) return 'Approving needs Face ID: enable it from your Mac.';
  if (a.rp_ok === false) return 'The tailnet name changed: enable Face ID again from your Mac.';
  return '';
}
const RC_L2_REASON = {
  level2_off: 'Approving is turned off on your Mac. Nothing was done.',
  no_passkey: 'This device has no Face ID key for sbxw (or the tailnet name changed): enable Face ID from your Mac. Nothing was done.',
  no_grant: 'The Mac has not allowed this device yet, or the permission expired. Start again.',
  passkey: 'Face ID could not be checked by the Mac. Nothing was done.',
  changed: 'This changed since you opened it: read it again.',
  refused: 'The Mac refused it.',
};
function rcL2Fail(res) {
  if (!res || res.unreachable || res.status === 0) return 'sbxw is unreachable (is the Mac awake?). Nothing was done.';
  const reason = res.data && typeof res.data.reason === 'string' && Object.hasOwn(RC_L2_REASON, res.data.reason) ? res.data.reason : null;
  const err = res.data && typeof res.data.error === 'string' && res.data.error.trim() ? res.data.error.trim().slice(0, 300) : '';
  if (reason === 'refused') return err ? `The Mac refused it: ${err}` : RC_L2_REASON.refused;
  if (reason) return RC_L2_REASON[reason];
  if (res.status === 429) return 'Too many failed tries. Wait a minute, then try again. Nothing was done.';
  if (res.status === 404) return 'This is not waiting any more. Nothing was done.';
  return `Nothing was done: the Mac answered ${res.status}${err ? ` (${err})` : ''}.`;
}
// The states of enabling Face ID on this device, each in a sentence.
const RC_PASSKEY_TEXT = {
  pending: 'In sbxw on your Mac, run "sbxw remote approvals allow" for this device and type these six digits.',
  granted: 'Allowed on the Mac. Create the Face ID key now.',
  refused: 'Refused on the Mac (or the number was typed wrong three times).',
  expired: 'This request expired. Start again.',
  registered: 'Face ID is ready on this device.',
  no_authenticator: 'This device cannot create a passkey (no Face ID or Touch ID for this site).',
  cancelled: 'Face ID was cancelled. Nothing was saved.',
  exists: 'This device already has a passkey for sbxw.',
  failed: 'The Face ID key could not be created. Start again.',
  rejected: 'The Mac did not accept this Face ID key. Start again from the Mac.',
  off: 'Approving is turned off on your Mac.',
};

// ── The actions ──
const RC_KINDS = ['plan_approve', 'plan_refuse', 'merge', 'network_respond', 'sandbox_remove'];
// The route of each action, the target encoded.
function rcActPath(kind, target) {
  const t = encodeURIComponent(String(target));
  switch (kind) {
    case 'plan_approve': return `/r/plans/${t}/approve`;
    case 'plan_refuse': return `/r/plans/${t}/refuse`;
    case 'merge': return `/r/merges/${t}/merge`;
    case 'network_respond': return `/r/network/${t}/respond`;
    case 'sandbox_remove': return `/r/stopped/${t}/remove`;
    default: return null;
  }
}
// One line: WHAT will happen, to WHAT. Names are the daemon's data, as text.
function rcActLine(kind, item, option) {
  const sb = String(item?.sandbox ?? '');
  switch (kind) {
    case 'plan_approve': {
      const n = Array.isArray(item?.threads) ? item.threads.length : 0;
      return `Approve the plan of ${sb} as it is: it starts ${n === 1 ? '1 thread' : `${n} threads`}.`;
    }
    case 'plan_refuse': return `Refuse the plan of ${sb}: no thread starts.`;
    case 'merge': {
      const n = Array.isArray(item?.branches) ? item.branches.length : 0;
      return `Merge ${n === 1 ? '1 branch' : `${n} branches`} of ${sb} in the order shown, stopping at the first conflict.`;
    }
    case 'network_respond': return `Answer "${String(option?.label ?? option?.id ?? '')}" to ${sb}'s request for ${String(item?.host ?? '')}.`;
    case 'sandbox_remove': return `Remove the stopped sandbox ${String(item?.name ?? '')}. This cannot be undone.`;
    default: return '';
  }
}
// What was done, in the daemon's words.
function rcActDone(kind, data) {
  if (kind === 'merge') {
    const m = data && typeof data.merge === 'object' && data.merge ? data.merge : null;
    const results = Array.isArray(m?.results) ? m.results : [];
    const failed = results.find(r => r && r.ok === false);
    if (failed || m?.state === 'failed') {
      const why = failed && typeof failed.error === 'string' ? failed.error : 'the merge stopped';
      return `Stopped at ${String(failed?.thread ?? 'a branch')}: ${why}`;
    }
    const done = results.filter(r => r && r.ok).map(r => String(r.thread));
    const notes = results.filter(r => r && r.ok && typeof r.note === 'string' && r.note).map(r => `${r.thread}: ${r.note}`);
    return `Merged${done.length ? `: ${done.join(', ')}` : ''}.${notes.length ? ` ${notes.join(' ')}` : ''}`;
  }
  return { plan_approve: 'Plan approved: its threads are starting.', plan_refuse: 'Plan refused.', network_respond: 'Answered.', sandbox_remove: 'Sandbox removed.' }[kind] || 'Done.';
}

// The level-2 routes, on the same client rules (same origin, JSON, once).
function rcL2Client(api) {
  return {
    approvals: () => api.get('/r/approvals'),
    passkeyStart: () => api.post('/r/passkey/start', {}),
    passkeyStatus: () => api.get('/r/passkey/status'),
    passkeyOptions: () => api.post('/r/passkey/options', {}),
    passkeyRegister: body => api.post('/r/passkey/register', body),
    list: what => api.get(`/r/${what}`),
    detail: (what, id) => api.get(`/r/${what}/${encodeURIComponent(id)}`),
    challenge: (kind, target, digest, option) => api.post('/r/approvals/challenge', option ? { kind, target, digest, option } : { kind, target, digest }),
    act: (kind, target, assertion, option) => {
      const path = rcActPath(kind, target);
      if (!path) throw new Error('not an action');
      return api.post(path, kind === 'network_respond' ? { option, assertion } : { assertion });
    },
  };
}
