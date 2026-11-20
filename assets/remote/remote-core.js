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
    message: (s, text) => call('POST', `${at(s)}/message`, { text }),
    answer: (s, indices) => call('POST', `${at(s)}/answer`, { indices }),
    interrupt: s => call('POST', `${at(s)}/interrupt`, {}),
    transcribe: pcm => call('POST', '/r/voice/transcribe', undefined, pcm),
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
