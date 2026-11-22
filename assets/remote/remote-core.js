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
  return rcPills(chatMarkdown(src, null).replace(RC_LINK, (_, url, label) =>
    `<a class="rc-link" href="${url}" target="_blank" rel="noopener noreferrer">${label} <span class="rc-url">${url}</span></a>`));
}

// ── Status labels ─────────────────────────────────────────────────────────
// `[[Done]]` or `[[Failed: 3 tests]]` as a pill, as the Mac terminal draws
// them (assets/js/emphasis.js, crates/core/src/emphasis.rs). The word -> tone
// table is the built-in one of emphasis.rs, copied: a test keeps the two
// equal. A word outside it stays plain text (the Mac's colouring of unknown
// capitalised words is not copied). Runs on the ESCAPED markup only, and
// never inside <code> or <pre>.
const RC_PILL_TONES = {
  done: 'green', verified: 'green', fixed: 'green', passed: 'green',
  failed: 'red', error: 'red', broken: 'red',
  'needs you': 'amber', blocked: 'amber', decide: 'amber', question: 'amber',
  irreversible: 'danger', destructive: 'danger', costly: 'danger',
  note: 'blue', context: 'blue', fyi: 'blue',
  working: 'cyan', 'in progress': 'cyan', next: 'cyan', plan: 'cyan',
  team: 'purple', lead: 'purple', thread: 'purple', jarvis: 'purple',
};
const RC_PILL = /\[\[(\p{L}(?:[\p{L}\p{N} -]{0,30}[\p{L}\p{N}])?)(?::([^\[\]\p{Cc}]{1,48}))?\]\]/gu;
function rcPills(html) {
  if (html.indexOf('[[') < 0) return html;
  return html
    .split(/(<pre>[\s\S]*?<\/pre>|<code>[\s\S]*?<\/code>)/)
    .map((part, i) => {
      if (i % 2) return part;
      return part.replace(RC_PILL, (all, word, detail) => {
        const k = word.split(' ').filter(Boolean).join(' ').toLowerCase();
        if (!Object.prototype.hasOwnProperty.call(RC_PILL_TONES, k)) return all;
        return `<span class="rc-pill rc-pill-${RC_PILL_TONES[k]}">${word}${detail === undefined ? '' : `<span class="rc-pill-detail">${detail}</span>`}</span>`;
      });
    })
    .join('');
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
    if (kind === 'user_prompt') {
      const files = rcFiles(it.files);
      if (text || files.length) row = files.length ? { kind: 'me', text, files } : { kind: 'me', text };
    }
    else if (kind === 'assistant_text' && text) row = { kind: 'agent', text };
    else if (kind === 'injected' && text) {
      const from = typeof it.from === 'string' && it.from && it.from !== 'sbxw' ? it.from : null;
      row = from ? { kind: 'agent', from, text } : { kind: 'sbxw', text };
    } else if (kind === 'question') {
      const q = Array.isArray(it.steps) && it.steps[0] && typeof it.steps[0].text === 'string' ? it.steps[0].text : text;
      row = { kind: 'asked', text: q || 'It asked a question.' };
    } else if (kind === 'permission') row = { kind: 'mac', text: text || 'It asked to allow a tool.' };
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

// The files sent with a prompt (the history's `files`: base names): strings
// only, a path cut to its last part all the same, at most RC_FILES_MAX.
const RC_FILES_MAX = 32;
function rcFiles(list) {
  if (!Array.isArray(list)) return [];
  return list
    .filter(f => typeof f === 'string')
    .map(f => f.split(/[\\/]/).pop())
    .filter(Boolean)
    .slice(0, RC_FILES_MAX);
}

// The person's bubble: the text, then one small chip per file. Every name
// is set as text (textContent), never as markup.
function rcMeEl(doc, row) {
  const d = doc.createElement('div');
  d.className = 'rc-msg rc-me';
  if (row.text) {
    const t = doc.createElement('div');
    t.className = 'rc-me-text';
    t.textContent = row.text;
    d.append(t);
  }
  const files = rcFiles(row.files);
  if (files.length) {
    const box = doc.createElement('div');
    box.className = 'rc-files';
    for (const name of files) {
      const chip = doc.createElement('span');
      chip.className = 'rc-file';
      chip.textContent = name;
      chip.title = name;
      box.append(chip);
    }
    d.append(box);
  }
  return d;
}

// `have` and a newer or older page, by seq, oldest first. A provisional
// reply (the Stop hook's copy) gives way to the transcript's copy of the same
// text, or to a newer provisional reply: the daemon's own rule
// (sbxw_core::history). Any other item leaves it: the transcript often does
// not hold the reply yet when the turn ends, and the reply must stay shown.
function rcMerge(have, incoming) {
  const fresh = (Array.isArray(incoming) ? incoming : []).filter(i => i && Number.isFinite(Number(i.seq)));
  const replies = fresh.filter(i => i.kind === 'assistant_text');
  const settled = p => replies.some(i => Number(i.seq) > Number(p.seq) && (i.provisional || rcSameReply(i.text, p.text)));
  const bySeq = new Map();
  for (const i of have) if (!(i.provisional && settled(i))) bySeq.set(Number(i.seq), i);
  for (const i of fresh) bySeq.set(Number(i.seq), i);
  return [...bySeq.values()].sort((a, b) => Number(a.seq) - Number(b.seq));
}
// The page catching up (remote.js): back after this long hidden, the event
// stream is opened again (iOS may have dropped it without a word); a history
// read with no answer after RC_STUCK_MS no longer holds the next one back.
const RC_AWAY_MS = 5000;
const RC_STUCK_MS = 15000;

// Two replies are the same text, whitespace aside.
function rcSameReply(a, b) {
  const words = t => String(t ?? '').split(/\s+/).filter(Boolean).join(' ');
  return words(a) === words(b);
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

// The picker's two groups (API.md 2.4, v1.5): the team leaders first, then
// the running sandboxes with no team, each in rcLeaders' order. An entry
// without a kind is a leader (a daemon from before v1.5). Empty groups are
// left out.
function rcGroups(data) {
  const all = rcLeaders(data);
  return [
    { kind: 'leader', title: 'Team leaders', items: all.filter(l => l.kind !== 'sandbox') },
    { kind: 'sandbox', title: 'Sandboxes', items: all.filter(l => l.kind === 'sandbox') },
  ].filter(g => g.items.length);
}

// Why the phone can only read this one, when the daemon says so; '' else.
// The daemon still refuses by its own rules: this only says it before the
// person writes.
const RC_READ_ONLY = {
  several_sessions: 'Read only: several sessions are open in this sandbox and sbxw cannot tell which one its terminal shows. Write from the Mac.',
  no_terminal: 'Read only: sbxw has no terminal open on this sandbox. Write from the Mac.',
};
// The picker's short form of the same reason.
const RC_READ_ONLY_SHORT = {
  several_sessions: 'Read only: several sessions open',
  no_terminal: 'Read only: no sbxw terminal on it',
};
function rcReadOnlyShort(l) {
  if (!l || typeof l.read_only !== 'string' || !l.read_only) return '';
  return Object.hasOwn(RC_READ_ONLY_SHORT, l.read_only) ? RC_READ_ONLY_SHORT[l.read_only] : 'Read only';
}
function rcReadOnly(l) {
  if (!l || typeof l.read_only !== 'string' || !l.read_only) return '';
  return Object.hasOwn(RC_READ_ONLY, l.read_only) ? RC_READ_ONLY[l.read_only] : 'Read only: write from the Mac.';
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
  return { kind: 'mac', text: steps[0]?.text || 'It waits for something only the Mac can settle, such as allowing a tool.' };
}

// A tap on option `oi` (0-based) of a question's step: a single-select step
// keeps that one option, a multi-select step (`step.multi`) toggles it. A
// step's picks are a sorted array, empty while nothing is chosen.
function rcAnswerPick(picked, step, oi) {
  if (!step || !step.multi) return [oi];
  const rest = picked.filter(x => x !== oi);
  return (rest.length === picked.length ? [...picked, oi] : rest).sort((a, b) => a - b);
}

// The answer route's `answers`: per step, the 1-based options picked.
function rcAnswers(picks) {
  return picks.map(p => p.map(i => i + 1));
}

// Why an answer did not go through. A 409 "not taken": sbxw typed it, but
// the session still asks, so the person answers at the Mac.
function rcAnswerFail(res) {
  const err = res && res.data && typeof res.data.error === 'string' ? res.data.error : '';
  if (res && res.status === 409 && err.startsWith('not taken')) return 'Answer not taken: the question is still open. Answer it on the Mac.';
  return rcFailText('Answer', res);
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
    usage: () => call('GET', '/r/usage'),
    pushKey: () => call('GET', '/r/push/key'),
    pushSubscribe: sub => call('POST', '/r/push/subscribe', { endpoint: sub.endpoint, p256dh: sub.p256dh, auth: sub.auth }),
    pushUnsubscribe: () => call('POST', '/r/push/unsubscribe', {}),
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
    // Per step, the 1-based options chosen (several for a multi-select step).
    answer: (s, answers) => call('POST', `${at(s)}/answer`, { answers }),
    interrupt: s => call('POST', `${at(s)}/interrupt`, {}),
    models: () => call('GET', '/r/models'),
    setModel: (s, id) => call('POST', `${at(s)}/model`, { id }),
    skills: () => call('GET', '/r/skills'),
    runSkill: (s, name, args) => call('POST', `${at(s)}/skill`, { name, args }),
    setLead: (s, on) => call('POST', `${at(s)}/lead`, { on: !!on }),
    // A new ticket on a leader's board: text and up to 5 upload ids, it lands in Planned.
    createTicket: (s, title, description, attachments = []) => call('POST', `${at(s)}/kanban`, attachments.length ? { title, description, attachments } : { title, description }),
    transcribe: pcm => call('POST', '/r/voice/transcribe', undefined, pcm),
    // The canvas (API-canvas.md): read the list and a manifest, change one
    // remark of the draft, send the round. An id that is not a canvas id
    // never reaches the network.
    canvasList: s => call('GET', `${at(s)}/canvas`),
    canvas: id => (rcCvId(id) ? call('GET', `/r/canvas/${id}`) : Promise.resolve({ ok: false, status: 404, unreachable: false, data: null })),
    canvasRemark: (id, body) => (rcCvId(id) ? call('POST', `/r/canvas/${id}/remarks`, body) : Promise.resolve({ ok: false, status: 404, unreachable: false, data: null })),
    canvasSubmit: id => (rcCvId(id) ? call('POST', `/r/canvas/${id}/submit`, {}) : Promise.resolve({ ok: false, status: 404, unreachable: false, data: null })),
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
  needs_mac: 'This needs your Mac: the session is waiting for you there.',
  busy: 'The session is busy right now: try again in a few seconds. Meanwhile you can read it and interrupt it.',
  no_paste: 'Send it from your Mac: this session\'s terminal cannot take a message from the phone.',
  attachment: 'Not sent: a file of this message is no longer on the Mac (it expires 30 minutes after its upload). Remove it, choose it again, then Send.',
  too_many: 'Not sent: a message takes 5 files at most. Remove some, then Send.',
  not_submitted: 'NOT SENT: the text was typed into the session\'s input on the Mac but not submitted, because the session changed meanwhile. It waits there unsent; it is also still in this field. Check on your Mac before sending again.',
};

// The model choices the page offers: the daemon's closed list, as
// `{id, label}` pairs. Anything else in the answer is dropped.
function rcModels(data) {
  const list = Array.isArray(data?.models) ? data.models : [];
  return list
    .filter(m => m && typeof m.id === 'string' && m.id)
    .map(m => ({ id: m.id, label: typeof m.label === 'string' && m.label ? m.label : m.id }));
}

// The skill names the page offers: the daemon's closed list, names only.
function rcSkills(data) {
  const list = Array.isArray(data?.skills) ? data.skills : [];
  return list.filter(n => typeof n === 'string' && /^[a-z0-9][a-z0-9-]{0,63}$/.test(n));
}

// The id the select shows for a session: the one it reported, when the list
// has it; '' (no choice made) otherwise.
function rcModelValue(leader, models) {
  const id = leader && typeof leader.model === 'string' ? leader.model : '';
  return models.some(m => m.id === id) ? id : '';
}

// Whether the model and lead controls can act now: the same gate as Send
// (idle, not read only). Lead OFF needs only that the sandbox is reachable.
function rcCtlGate(leader, which) {
  if (!leader) return 'This sandbox is not running.';
  const ro = rcReadOnly(leader);
  if (ro) return ro;
  if (which === 'lead' && leader.kind === 'leader') return '';
  if (leader.state === 'working') return 'The session is working: this comes back at its prompt.';
  if (leader.asks) return 'This needs your Mac: the session is waiting for you there.';
  return '';
}

// What the daemon will refuse, checked before sending so the person can fix
// it in place (the daemon stays the judge): a line whose first non-blank
// character is !, / or #, or any control character but a line feed (tab, CR,
// ESC, DEL, C1), a format character (bidi controls, zero-width space and
// joiners, BOM, soft hyphen, word joiner, tags), a line or paragraph separator.
const RC_CONTROL = /(?!\n)[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u;
function rcMessageCheck(text, skills) {
  const s = String(text ?? '');
  if (!s.trim()) return 'empty';
  // A known skill line goes to the skill route, not as a message.
  if (skills && rcSkillLine(s, skills)) return RC_CONTROL.test(s) ? 'control' : null;
  if (s.split('\n').some(line => /^\s*[!/#]/.test(line))) return 'command';
  if (RC_CONTROL.test(s)) return 'control';
  return null;
}

// The skill autocomplete. While the field holds `/` and a partial name (no
// space yet), the skills whose name starts with it, then those that hold it.
// Anything else (no `/`, a space typed, a line break): none.
function rcSkillMatches(text, skills) {
  const m = /^\/([a-z0-9-]*)$/i.exec(String(text ?? ''));
  if (!m || !Array.isArray(skills)) return [];
  const q = m[1].toLowerCase();
  return [...skills.filter(n => n.startsWith(q)), ...skills.filter(n => !n.startsWith(q) && n.includes(q))];
}
// What tapping a suggestion puts in the field.
function rcSkillInsert(name) { return `/${name} `; }
// The whole text as a known skill's line: `/<name>` or `/<name> <args>`, one
// line, the name in the list. {name, args}, or null (a message, or a line the
// daemon refuses).
function rcSkillLine(text, skills) {
  const s = String(text ?? '').trim();
  const m = /^\/([a-z0-9][a-z0-9-]{0,63})(?:[ \t]+([^\n]*))?$/.exec(s);
  if (!m || !Array.isArray(skills) || !skills.includes(m[1])) return null;
  return { name: m[1], args: (m[2] || '').trim() };
}

// Why a request did not go through, in the person's words. `what`: "Message".
function rcFailText(what, res) {
  if (res.unreachable || res.status === 0) return `${what} not sent: sbxw is unreachable (is the Mac awake?).`;
  const reason = res.data && typeof res.data.reason === 'string' && Object.hasOwn(RC_MSG_REASON, res.data.reason) ? res.data.reason : null;
  if (reason) {
    const err = typeof res.data.error === 'string' && res.data.error.trim() ? ` (sbxw: ${res.data.error.trim().slice(0, 200)})` : '';
    return `${RC_MSG_REASON[reason]}${err}`;
  }
  if (res.status === 409) return `${what} not sent: the session is not waiting for that any more.`;
  if (res.status === 413) return `${what} not sent: it is too long.`;
  if (res.status === 429) return `${what} not sent: too many at once. Try again in a moment.`;
  const err = res.data && typeof res.data.error === 'string' ? res.data.error.slice(0, 200) : `the Mac answered ${res.status}`;
  return `${what} not sent: ${err}.`;
}

// Why Send waits, or '' when it can go. A message goes while the session is
// idle at its prompt and while it works (Claude Code queues it, as when typed
// at the Mac); the daemon refuses it while the session waits for the person.
// Never a queue of the page's: a refused text stays in the field until the
// person presses Send again.
function rcSendGate(leader) {
  if (!leader) return 'This sandbox is not running.';
  const ro = rcReadOnly(leader);
  if (ro) return ro;
  return '';
}

// What the status line says while Send can go to a working session: the
// message is queued by Claude Code, not read at once. '' otherwise.
const RC_QUEUED_TEXT = 'The session is working: a message you send now is queued by Claude Code and read when it can, as when typed at the Mac.';
function rcSendNote(leader) {
  if (!leader || rcSendGate(leader)) return '';
  return leader.state === 'working' ? RC_QUEUED_TEXT : '';
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
  if (res.status === 404) return 'Not uploaded: this sandbox is not running any more.';
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
// `quiet`: the recorder or the recogniser has heard nothing for a while.
// `hint`: a string (tone info) or { text, tone }.
function rcStatus({ note = null, rec = 'idle', gate = '', held = 0, pending = 0, failed = 0, files = 0, hint = '', quiet = false, queued = '' } = {}) {
  if (note && note.text) return { text: note.text, tone: note.tone || 'info' };
  if (rec === 'recording' && quiet) return { text: RC_DICT_TEXT.quietMac, tone: 'warn' };
  if (rec === 'recording') return { text: 'Recording. Tap stop when you are done: the words go into the field, nothing is sent.', tone: 'busy' };
  if (rec === 'transcribing') return { text: 'Transcribing on your Mac…', tone: 'busy' };
  if (rec === 'listening' && quiet) return { text: RC_DICT_TEXT.quietIphone, tone: 'warn' };
  if (rec === 'listening') return { text: 'Listening (iPhone dictation). Tap stop when you are done: the words go into the field, nothing is sent.', tone: 'busy' };
  if (held) return { text: `Tick the box to attach ${held === 1 ? 'the file whose name looks' : `the ${held} files whose names look`} like a secret, or cancel.`, tone: 'info' };
  if (pending) return { text: `Uploading ${pending === 1 ? '1 file' : `${pending} files`}: Send waits until ${pending === 1 ? 'it is' : 'they are'} done.`, tone: 'busy' };
  if (gate) return { text: files ? `${gate.replace(/\.$/, '')}. Your files wait here.` : gate, tone: 'info' };
  if (failed) return { text: `${failed === 1 ? 'An upload' : `${failed} uploads`} failed: retry or remove. The rest can be sent.`, tone: 'err' };
  if (hint && typeof hint === 'object') return { text: String(hint.text || ''), tone: hint.tone || 'info' };
  return { text: hint || queued || '', tone: 'info' };
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
  not_found: 'This is not waiting any more. Nothing was done.',
  unavailable: 'The Mac cannot read this right now (sbx did not answer). Try again in a moment.',
  bad: 'The Mac could not read this request. Nothing was done.',
  limited: 'Too many tries. Wait a few minutes, then start again.',
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
// ── The Approvals sandbox filter: a display filter, nothing more ──
// Works on the lists the page already read. Every list item names its
// sandbox (`sandbox`; a stopped sandbox is its own, so `name`). Nothing here
// decides what the device may see or approve: it only hides rows.
function rcApSandbox(what, it) {
  const v = what === 'stopped' ? it?.name : it?.sandbox;
  return typeof v === 'string' ? v : '';
}
// What the badge says (API-level2.md, `counts`): the number is plan decisions
// only (plans and merges waiting); network requests never add to it, they
// raise a dot; stopped sandboxes are a list, not a request, and count for
// neither.
function rcApBadge(counts) {
  const n = k => (counts && typeof counts === 'object' ? Math.max(0, Math.floor(Number(counts[k]))) || 0 : 0);
  return { n: n('plans') + n('merges'), dot: n('network') > 0 };
}
// `lists`: { plans: [...], merges: [...], network: [...], stopped: [...] }.
// One entry per sandbox that has something waiting, with its count, in the
// order the app uses for leaders (rcLeaders); the rest after, by name.
function rcApEntries(lists, leaders, keep) {
  const counts = new Map();
  const per = new Map(); // sandbox -> { plans, merges, network, stopped }
  for (const what of ['plans', 'merges', 'network', 'stopped']) {
    for (const it of Array.isArray(lists?.[what]) ? lists[what] : []) {
      const s = rcApSandbox(what, it);
      if (!s) continue;
      counts.set(s, (counts.get(s) || 0) + 1);
      per.set(s, { ...per.get(s), [what]: (per.get(s)?.[what] || 0) + 1 });
    }
  }
  // A chosen sandbox with nothing left stays in the strip, at zero.
  if (typeof keep === 'string' && keep && !counts.has(keep)) counts.set(keep, 0);
  const order = new Map((Array.isArray(leaders) ? leaders : []).map((l, i) => [l.sandbox, i]));
  const name = s => (Array.isArray(leaders) ? leaders.find(l => l.sandbox === s)?.name : '') || s;
  return [...counts]
    .map(([sandbox, count]) => ({ sandbox, name: name(sandbox), count, ...rcApBadge(per.get(sandbox)) }))
    .sort((a, b) => (b.count > 0) - (a.count > 0)
      || (order.has(a.sandbox) ? order.get(a.sandbox) : Infinity) - (order.has(b.sandbox) ? order.get(b.sandbox) : Infinity)
      || a.name.localeCompare(b.name));
}
// The items of one kind that pass the filter (null: all of them).
function rcApFilter(what, items, sandbox) {
  const list = Array.isArray(items) ? items : [];
  return sandbox ? list.filter(it => rcApSandbox(what, it) === sandbox) : list;
}
// Where Approvals starts: on the leader the phone is looking at when it has
// something waiting, else on All (null).
function rcApDefault(current, entries) {
  return current && (entries || []).some(e => e.sandbox === current && e.count > 0) ? current : null;
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
// One line: WHAT will happen, to WHAT, as plain text (rcActParts draws it).
function rcActLine(kind, item, option) {
  return rcActParts(kind, item, option).map(p => (typeof p === 'string' ? p : p.data)).join('');
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

const rcHasModels = m => !!m && typeof m === 'object' && Object.keys(m).length > 0;
// A plan's model changes, as the challenge takes them: for each thread whose
// picked model is an id of the configured list and differs from the one
// proposed, thread name to id. Nothing else of a thread changes.
function rcModelChanges(threads, picked, list) {
  const ids = new Set((Array.isArray(list) ? list : []).map(m => (m && typeof m.id === 'string' ? m.id : '')).filter(Boolean));
  const out = [];
  for (const t of Array.isArray(threads) ? threads : []) {
    const name = typeof t?.name === 'string' ? t.name : '';
    const want = picked && Object.prototype.hasOwnProperty.call(picked, name) ? picked[name] : undefined;
    if (!name || typeof want !== 'string' || !ids.has(want) || want === (t.model ?? '')) continue;
    out.push([name, want]);
  }
  // Own data properties only: a thread named `__proto__` sets no prototype.
  return Object.fromEntries(out);
}

// The level-2 routes, on the same client rules (same origin, JSON, once).
function rcL2Client(api) {
  return {
    approvals: () => api.get('/r/approvals'),
    passkeyStart: () => api.post('/r/passkey/start', {}),
    // The request_id of /r/passkey/start binds the ceremony (v1.1): sent on
    // status, options and register; the caller keeps it in memory only.
    passkeyStatus: rid => api.get(`/r/passkey/status?request_id=${encodeURIComponent(rid)}`),
    passkeyOptions: rid => api.post('/r/passkey/options', { request_id: rid }),
    passkeyRegister: (rid, body) => api.post('/r/passkey/register', { request_id: rid, ...body }),
    list: what => api.get(`/r/${what}`),
    detail: (what, id) => api.get(`/r/${what}/${encodeURIComponent(id)}`),
    // `models` (a plan approval's model changes, thread name to model id)
    // is sent with the challenge and again with the act, only when not empty.
    challenge: (kind, target, digest, option, models) => {
      const body = option ? { kind, target, digest, option } : { kind, target, digest };
      if (kind === 'plan_approve' && rcHasModels(models)) body.models = models;
      return api.post('/r/approvals/challenge', body);
    },
    act: (kind, target, assertion, option, models) => {
      const path = rcActPath(kind, target);
      if (!path) throw new Error('not an action');
      if (kind === 'network_respond') return api.post(path, { option, assertion });
      if (kind === 'plan_approve' && rcHasModels(models)) return api.post(path, { assertion, models });
      return api.post(path, { assertion });
    },
  };
}

// ══ The Kanban (read only) ═══════════════════════════════════════════════
// The contract is .sbxw-artifacts/remote/API-kanban.md. The page draws the
// route's answer in the daemon's order and computes nothing: no counting,
// no sorting, no moving. Every string is data, set as text.
const RC_KB_COLS = ['planned', 'progress', 'test', 'ready', 'merged'];
const RC_KB_TONES = ['neutral', 'blue', 'green', 'amber', 'red', 'purple', 'gray', 'grey'];
const rcKbTone = t => (RC_KB_TONES.includes(t) ? (t === 'grey' ? 'gray' : t) : 'neutral');
const rcStr = v => (typeof v === 'string' ? v : '');
// A card, its fields checked; anything unexpected becomes empty.
function rcKbCard(c) {
  if (!c || typeof c !== 'object' || typeof c.key !== 'string' || !c.key) return null;
  const n = Number.isInteger(c.n) && c.n > 0 ? c.n : null;
  return {
    key: c.key,
    n,
    num: rcStr(c.num) || (n ? `T-${n}` : 'thread'),
    title: rcStr(c.title_long) || rcStr(c.title) || rcStr(c.title_short),
    priority: ['p1', 'p2', 'p3'].includes(c.priority) ? c.priority : 'none',
    thread: rcStr(c.thread) || null,
    thread_state: rcStr(c.thread_state) || null,
    thread_tone: rcKbTone(c.thread_tone),
    marks: (Array.isArray(c.marks) ? c.marks : []).filter(m => m && typeof m.label === 'string' && m.label).map(m => ({ label: m.label, tone: rcKbTone(m.tone) })),
    files: Math.max(0, Math.floor(Number(c.files)) || 0),
    age: rcStr(c.age),
  };
}
// The board: the five columns in the daemon's order, with what it says.
function rcKbBoard(data) {
  const cols = Array.isArray(data?.columns) ? data.columns : [];
  return {
    latest: rcStr(data?.latest),
    summary: rcStr(data?.summary),
    columns: cols.filter(c => c && typeof c.id === 'string' && c.id).map(c => {
      const cards = (Array.isArray(c.cards) ? c.cards : []).map(rcKbCard).filter(Boolean);
      return {
        id: c.id,
        title: rcStr(c.title) || c.id,
        tone: rcKbTone(c.tone),
        empty: rcStr(c.empty) || 'Nothing here.',
        count: Number.isFinite(Number(c.count)) ? Math.max(0, Math.floor(Number(c.count))) : cards.length,
        older: Math.max(0, Math.floor(Number(c.older)) || 0),
        cards,
      };
    }),
  };
}
// The column to open first: active things first (in progress, in test,
// ready to merge), then planned, then merged; the first if all are empty.
function rcKbFirst(board) {
  const cols = board?.columns || [];
  for (const id of ['progress', 'test', 'ready', 'planned', 'merged']) {
    const c = cols.find(x => x.id === id);
    if (c && c.cards.length) return c.id;
  }
  return cols[0]?.id || null;
}
// A ticket's detail, its fields checked.
function rcKbTicket(data) {
  const t = data?.ticket;
  if (!t || typeof t !== 'object') return null;
  return {
    num: rcStr(t.num) || (Number.isInteger(t.n) ? `T-${t.n}` : ''),
    title: rcStr(t.title),
    description: rcStr(t.description),
    priority: ['p1', 'p2', 'p3'].includes(t.priority) ? t.priority : 'none',
    column: rcStr(t.column),
    card: rcKbCard(t.card),
    files: (Array.isArray(t.files) ? t.files : []).filter(f => f && typeof f.name === 'string').map(f => ({ name: f.name, size: Math.max(0, Number(f.size) || 0), kind: rcStr(f.kind) || 'other' })),
    history: (Array.isArray(t.history) ? t.history : []).filter(h => h && typeof h.text === 'string').map(h => ({ when: rcStr(h.when), text: h.text, tone: rcKbTone(h.tone) })),
  };
}
const RC_KB_PRIORITY = { p1: 'Priority 1', p2: 'Priority 2', p3: 'Priority 3', none: 'No priority' };
const RC_KB_THREAD = { starting: 'starting', working: 'working', blocked: 'blocked', done: 'done', failed: 'failed' };
function rcKbPaths(sandbox, n) {
  const at = `/r/leaders/${encodeURIComponent(sandbox)}/kanban`;
  if (n === undefined) return at;
  return Number.isInteger(n) && n > 0 && n < 1e9 ? `${at}/T-${n}` : null;
}

// ══ Dictation by the phone's own speech recognition (optional) ═══════════
// Off by default: the Mac transcribes (audio to /r/voice/transcribe only).
// With "iPhone" chosen in the menu, the Web Speech API listens instead; the
// voice may then be processed by Apple. The words still only go INTO the
// field; nothing is sent.
const RC_DICTATION = { mac: 'mac', iphone: 'iphone' };
const RC_SPEECH_NOTE = 'iPhone dictation is done by Apple\'s speech recognition; your voice may be sent to Apple.';
const RC_SPEECH_FALLBACK_TEXT = 'iPhone dictation is not available here. Using your Mac.';
// Errors that mean "this browser cannot do it": the Mac takes over.
const RC_SPEECH_FALLBACK = ['not-allowed', 'service-not-allowed', 'network', 'audio-capture'];
function rcSpeechFallback(error) {
  return RC_SPEECH_FALLBACK.includes(String(error || ''));
}
// The recognition language: the browser's, else the page's, else English.
function rcSpeechLang(browser, page) {
  const ok = s => typeof s === 'string' && /^[A-Za-z]{2,3}(-[A-Za-z0-9]{2,8})*$/.test(s);
  return ok(browser) ? browser : ok(page) ? page : 'en-US';
}
// Words put into the field at the caret, with a space on each side where
// the text there needs one.
function rcInsertAt(text, caret, words) {
  const t = String(text ?? '');
  const w = String(words ?? '').trim();
  if (!w) return { value: t, caret: Math.min(Math.max(0, caret | 0), t.length) };
  const at = Number.isInteger(caret) && caret >= 0 && caret <= t.length ? caret : t.length;
  const before = t.slice(0, at), after = t.slice(at);
  const lead = before && !/\s$/.test(before) ? ' ' : '';
  const tail = after && !/^\s/.test(after) ? ' ' : '';
  const value = `${before}${lead}${w}${tail}${after}`;
  return { value, caret: before.length + lead.length + w.length };
}
// The final and the interim text of a SpeechRecognition result list.
function rcSpeechText(results) {
  let final = '', interim = '';
  for (let i = 0; i < (results?.length || 0); i++) {
    const r = results[i];
    const t = r && r[0] && typeof r[0].transcript === 'string' ? r[0].transcript : '';
    if (r && r.isFinal) final += ` ${t}`; else interim += ` ${t}`;
  }
  return { final: final.replace(/\s+/g, ' ').trim(), interim: interim.replace(/\s+/g, ' ').trim() };
}

// ── Dictation that never ends or fails in silence ─────────────────────────
// Every end of a dictation says what happened and what to do, in one
// sentence; the control is idle again. These sentences are the page's own.
const RC_DICT_TEXT = {
  added: 'Added to the field: read it, then Send.',
  nothing: 'Nothing was heard. Tap the microphone and speak.',
  stoppedSelf: 'The iPhone stopped listening: tap the microphone to go on.',
  taken: 'The microphone was taken by another app (a call?). Tap the microphone to try again.',
  network: 'The iPhone\'s speech recognition lost its connection. Tap the microphone to try again, or choose Mac in the menu.',
  refused: 'Speech recognition is not allowed here. Allow it in Settings, or choose Mac in the menu.',
  language: 'The iPhone cannot recognise this language here. Choose Mac in the menu.',
  noStart: 'The iPhone did not start listening. Tap the microphone to try again.',
  hidden: 'Dictation stopped: the page was hidden.',
  time: 'Two minutes is the longest dictation. Tap the microphone to go on.',
  kept: 'The words heard are in the field.',
  quietMac: 'The microphone hears nothing. Speak closer, or check that no other app (a call) is using it.',
  quietIphone: 'Nothing heard for a few seconds. Speak, or tap stop, then the microphone again.',
  macEmpty: 'Nothing was recorded. Tap the microphone and try again.',
  macShort: 'Too short to transcribe. Tap the microphone, speak, then tap stop.',
  macSilent: 'Nothing was heard: the recording is silent. Check the microphone and try again.',
  macNoWords: 'The Mac heard no words. Speak closer to the phone and try again.',
  macFailed: 'The Mac could not transcribe this: try again.',
  macUnreachable: 'The Mac could not transcribe this: sbxw is unreachable (is the Mac awake?).',
  macMissing: 'Dictation is not installed on the Mac.',
  macTooLong: 'The Mac could not transcribe this: the recording is too long.',
  macSlow: 'The Mac did not answer in time. Try again.',
  unreadable: 'This recording could not be read. Try again.',
  recFailed: 'The recording stopped by itself. Tap the microphone to try again.',
  busy: 'Microphone busy, try again in a second.',
  waiting: 'Waiting for the microphone…',
};
// How a recognition ended → { text, tone }. `error`: the last SpeechRecognition
// error code, or ''. `reason`: 'person' (tapped stop), 'self' (ended alone),
// 'hidden', 'time', 'nostart'. `words`: something is in the field.
function rcSpeechEnd({ error = '', reason = 'self', words = false } = {}) {
  const e = String(error || '');
  let t = '';
  if (e === 'audio-capture') t = RC_DICT_TEXT.taken;
  else if (e === 'network') t = RC_DICT_TEXT.network;
  else if (e === 'not-allowed' || e === 'service-not-allowed') t = RC_DICT_TEXT.refused;
  else if (e === 'language-not-supported') t = RC_DICT_TEXT.language;
  else if (reason === 'nostart') t = RC_DICT_TEXT.noStart;
  else if (reason === 'hidden') t = RC_DICT_TEXT.hidden;
  else if (reason === 'time') t = RC_DICT_TEXT.time;
  else if (reason === 'person' || e === 'aborted') t = words ? '' : RC_DICT_TEXT.nothing;
  else if (e === 'no-speech' && !words) t = RC_DICT_TEXT.nothing;
  else t = words ? RC_DICT_TEXT.stoppedSelf : RC_DICT_TEXT.nothing;
  if (!t) return { text: RC_DICT_TEXT.added, tone: 'ok' };
  return { text: words ? `${t} ${RC_DICT_TEXT.kept}` : t, tone: 'warn' };
}
// Whether a recognition that ended BY ITSELF starts again. Only after a
// clean end (no error), while visible, with words heard in that round and a
// round of at least 2 s, and at most 2 times per tap: it cannot loop. An
// error that needs the person never restarts.
const RC_SPEECH_RESTARTS = 2;
function rcSpeechRestart({ error = '', reason = 'self', hidden = false, restarts = 0, heard = false, ms = 0, total = 0 } = {}) {
  return !error && reason === 'self' && !hidden && heard && restarts < RC_SPEECH_RESTARTS && ms >= 2000 && total < RC_VOICE_MAX_S * 1000;
}
// ── One user of the microphone at a time ──────────────────────────────────
// On an iPhone a recogniser that was told to stop (or abort) keeps the audio
// session until its `end` event, and a little after: a new recogniser, or a
// new getUserMedia stream, started in that time gets no audio at all and
// the page says it hears nothing. So a new take waits: until every earlier
// user of the microphone (a recogniser, a stream, an AudioContext) has
// really ended, then a short settle. The wait is bounded: past `max` ms the
// take does not start and the person is told the microphone is busy.
// `live`: earlier users not yet ended. `endedAt`: when the last one ended
// (0: never). `waited`: ms this take has waited. → 'go', 'wait' or 'busy'.
const RC_MIC_SETTLE_MS = 350;
const RC_MIC_WAIT_MS = 2000;
function rcMicGate({ live = 0, endedAt = 0, now = 0, waited = 0, settle = RC_MIC_SETTLE_MS, max = RC_MIC_WAIT_MS } = {}) {
  if (live > 0) return waited >= max ? 'busy' : 'wait';
  if (endedAt > 0 && now - endedAt < settle && waited < max) return 'wait';
  return 'go';
}
// Whether "nothing heard" is said, during a take. Only once audio has
// really started (`audioAt`: the recogniser's audiostart, or the first level
// read from a running AudioContext; 0: not yet, or not known now): before
// that, a quiet line would blame the person for a microphone that is not
// delivering yet. `heardAt`: the last sound or word, of this take (one
// before `audioAt` belongs to an earlier round and does not count).
function rcMicQuiet({ now = 0, audioAt = 0, heardAt = 0, quietMs = 3000, heardMs = quietMs } = {}) {
  if (!(audioAt > 0)) return false;
  if (heardAt >= audioAt) return now - heardAt >= heardMs;
  return now - audioAt >= quietMs;
}
// A final text is inserted once. Some recognisers deliver a final result
// that repeats the one before it (the same words, or the same words and
// more: a cumulative transcript), within one round or as the first result
// of a restarted round. Two results whose first words (two or more) are the
// same, compared without case and punctuation: the later one replaces the
// earlier. → the joined text.
const rcWordKeys = s => String(s || '').split(/\s+/).filter(Boolean).map(w => w.toLowerCase().replace(/[^\p{L}\p{N}]/gu, ''));
function rcRepeats(prev, cur) {
  const p = rcWordKeys(prev), c = rcWordKeys(cur);
  return p.length >= 2 && c.length >= p.length && p.every((k, i) => k === c[i]);
}
function rcFinalSegments(results) {
  const out = [];
  for (let i = 0; i < (results?.length || 0); i++) {
    const r = results[i];
    if (!r || !r.isFinal) continue;
    const t = r[0] && typeof r[0].transcript === 'string' ? r[0].transcript.replace(/\s+/g, ' ').trim() : '';
    if (!t) continue;
    // The whole text so far, carried again: it replaces all of it.
    if (out.length > 1 && rcRepeats(out.join(' '), t)) out.splice(0, out.length, t);
    else if (out.length && rcRepeats(out[out.length - 1], t)) out[out.length - 1] = t;
    else out.push(t);
  }
  return out;
}
// One round's text after the rounds before it. `carry`: the last final
// segment of the round before; a first segment that repeats it adds only
// its new words. → { final, interim, last }
function rcSpeechRound(results, carry = '') {
  const segs = rcFinalSegments(results);
  if (carry && segs.length && rcRepeats(carry, segs[0])) {
    const n = String(carry).split(/\s+/).filter(Boolean).length;
    const rest = segs[0].split(/\s+/).filter(Boolean).slice(n).join(' ');
    if (rest) segs[0] = rest; else segs.shift();
  }
  const t = rcSpeechText(results);
  return { final: segs.join(' '), interim: t.interim, last: segs.length ? segs[segs.length - 1] : carry };
}
// The "jump to the latest" button shows when the log is scrolled away from
// its end by more than `slack` px (the same 80 px the page uses for "at the end").
function rcAwayFromEnd(scrollHeight, scrollTop, clientHeight, slack = 80) {
  const gap = Number(scrollHeight) - Number(scrollTop) - Number(clientHeight);
  return Number.isFinite(gap) && gap > slack;
}
// ── The conversation's place (remote.js keeps it in one place) ──
// One state, `stick`: true while the person is at the end of the log. Only
// a scroll changes it, never a change of content, so rows that arrive, a
// tail that grows or a keyboard that opens never decide it. While the page
// itself glides to the end (the round button), it stays true.
function rcStickAfterScroll(scrollHeight, scrollTop, clientHeight, gliding = false, slack = 80) {
  return !!gliding || !rcAwayFromEnd(scrollHeight, scrollTop, clientHeight, slack);
}
// A row's key, stable across re-renders: its seq, or for a folded "steps"
// row the seq of the row before it.
function rcRowKeys(rows) {
  let last = 0;
  return (Array.isArray(rows) ? rows : []).map(r => {
    if (r && Number.isFinite(Number(r.seq)) && Number(r.seq) > 0) { last = Number(r.seq); return `s${last}`; }
    return `after${last}`;
  });
}
// Away from the end, the place is a row, not a scrollTop: the first row
// still in view at the top ({ key, top, bottom } relative to the log's
// content), and how far below the view's top it starts.
function rcAnchorPick(rows, viewTop) {
  for (const r of Array.isArray(rows) ? rows : []) {
    if (r && r.key && r.bottom > viewTop) return { key: r.key, offset: r.top - viewTop };
  }
  return null;
}
// After a change: where to put scrollTop. Stuck: the end. Away: the anchor
// row back at the same offset; with no anchor found, the old scrollTop.
function rcLogPlace(stick, anchor, rows, scrollTop, scrollHeight, clientHeight) {
  const end = Math.max(0, scrollHeight - clientHeight);
  if (stick) return end;
  const r = anchor && (Array.isArray(rows) ? rows : []).find(x => x && x.key === anchor.key);
  const top = r ? r.top - anchor.offset : scrollTop;
  return Math.min(end, Math.max(0, top));
}
// The live preview shows the END of what is said: at most `max` characters
// of final then interim words, older words cut at the start with "…".
function rcTail(final, interim, max = 140) {
  const f = String(final || ''), i = String(interim || '');
  const sep = f && i ? 1 : 0;
  const all = `${f}${sep ? ' ' : ''}${i}`;
  const m = Math.max(8, max | 0);
  if (all.length <= m) return { final: f, interim: i, cut: false };
  let start = all.length - (m - 1);
  // Start on a word when one begins close by.
  const sp = all.indexOf(' ', start - 1);
  if (sp >= 0 && sp - start < 16 && sp + 1 < all.length) start = sp + 1;
  if (start < f.length) return { final: `…${f.slice(start).trimStart()}`, interim: i, cut: true };
  return { final: '…', interim: i.slice(Math.max(0, start - f.length - sep)).trimStart(), cut: true };
}
// The recorder's format: the first the browser says it can record (an
// iPhone records audio/mp4; Chromium webm). '' lets the browser choose.
// The phone decodes it to 16 kHz PCM itself: the daemon reads no format.
const RC_REC_TYPES = ['audio/mp4', 'audio/webm;codecs=opus', 'audio/webm', 'audio/aac', 'audio/ogg;codecs=opus'];
function rcRecMime(isSupported) {
  for (const t of RC_REC_TYPES) {
    try { if (isSupported && isSupported(t)) return t; } catch (_) {}
  }
  return '';
}
// A clip worth sending: not empty, at least half a second. → '' or why not.
function rcClipCheck(bytes, ms) {
  if (!(ms >= 500)) return RC_DICT_TEXT.macShort;
  if (!(bytes > 0)) return RC_DICT_TEXT.macEmpty;
  return '';
}
// Samples that are all silence (peak under -54 dBFS): not sent.
function rcSilentPcm(samples) {
  const n = samples?.length || 0;
  for (let i = 0; i < n; i++) if (Math.abs(samples[i]) >= 0.002) return false;
  return true;
}
// The Mac's answer to a transcription → { words } or { text } (why not).
function rcTranscribed(res) {
  if (!res) return { text: RC_DICT_TEXT.unreadable };
  if (res.timeout) return { text: RC_DICT_TEXT.macSlow };
  if (!res.ok) {
    if (res.status === 503) return { text: RC_DICT_TEXT.macMissing };
    if (res.unreachable || res.status === 0) return { text: RC_DICT_TEXT.macUnreachable };
    if (res.status === 413) return { text: RC_DICT_TEXT.macTooLong };
    return { text: RC_DICT_TEXT.macFailed };
  }
  const words = typeof res.data?.text === 'string' ? res.data.text.trim() : '';
  return words ? { words } : { text: RC_DICT_TEXT.macNoWords };
}

// ══ Text that is not the page's own, made unambiguous ════════════════════
// (Security review P6.) A name, a path, a goal or a ticket written by an
// agent may carry bidirectional controls or invisible characters that
// reorder or hide what the person reads before approving. They are never
// honoured: each becomes a visible code ("U+202E") the page draws in a box,
// and the page isolates every such string (dir="auto", unicode-bidi:
// isolate) so one cannot reorder its neighbours. Line feeds and tabs stay.
// Format characters (bidi controls and isolates, zero-width space and
// joiners, BOM, soft hyphen, word joiner, tags...), line and paragraph
// separators, C0 and C1 controls but tab and line feed, and the characters
// that draw nothing (combining grapheme joiner, Hangul fillers, Mongolian
// selectors, variation selectors, interlinear annotation).
const RC_INVISIBLE = /[\p{Cf}\p{Zl}\p{Zp}\u0000-\u0008\u000B-\u001F\u007F-\u009F\u034F\u115F\u1160\u17B4\u17B5\u180B-\u180F\u3164\uFE00-\uFE0F\uFFA0\uFFF9-\uFFFB]/u;
// → [string | { code: 'U+202E' }]
function rcMarked(s) {
  const out = [];
  let run = '';
  for (const ch of String(s ?? '')) {
    if (RC_INVISIBLE.test(ch)) {
      if (run) { out.push(run); run = ''; }
      out.push({ code: `U+${ch.codePointAt(0).toString(16).toUpperCase().padStart(4, '0')}` });
    } else run += ch;
  }
  if (run) out.push(run);
  return out;
}
// A sheet's one line as parts: the page's words as strings, the daemon's
// data (names, hosts, labels, branches) as { data } to draw isolated, in
// monospace, with the codes visible.
function rcActParts(kind, item, option, models) {
  const d = v => ({ data: String(v ?? '') });
  switch (kind) {
    case 'plan_approve': {
      const n = Array.isArray(item?.threads) ? item.threads.length : 0;
      const changed = rcHasModels(models) ? Object.entries(models) : [];
      if (!changed.length) return ['Approve the plan of ', d(item?.sandbox), ` as it is: it starts ${n === 1 ? '1 thread' : `${n} threads`}.`];
      const parts = ['Approve the plan of ', d(item?.sandbox), `: it starts ${n === 1 ? '1 thread' : `${n} threads`}, as proposed except the model of `];
      changed.forEach(([t, m], i) => { if (i) parts.push(', '); parts.push(d(t), ' (now ', d(m), ')'); });
      parts.push('.');
      return parts;
    }
    case 'plan_refuse': return ['Refuse the plan of ', d(item?.sandbox), ': no thread starts.'];
    case 'merge': {
      const n = Array.isArray(item?.branches) ? item.branches.length : 0;
      const order = Array.isArray(item?.order) ? item.order : [];
      const parts = [`Merge ${n === 1 ? '1 branch' : `${n} branches`} of `, d(item?.sandbox), ' in the order shown, stopping at the first conflict.'];
      if (order.length) {
        parts.push(' Order: ');
        order.forEach((o, i) => { if (i) parts.push(', '); parts.push(d(o)); });
        parts.push('.');
      }
      return parts;
    }
    case 'network_respond': return ['Answer ', d(option?.label ?? option?.id), ' to the request of ', d(item?.sandbox), ' for ', d(item?.host), '.'];
    case 'sandbox_remove': return ['Remove the stopped sandbox ', d(item?.name), '. This cannot be undone.'];
    default: return [];
  }
}

// ── Notifications (Web Push, API-push.md) ─────────────────────────────────
// The page's own wording is English; the two notification texts are French
// and live only in sw.js. A switch the person turns on by a tap, per device.
// Nothing here logs a subscription, an endpoint or a key.
const RC_PUSH_TEXT = {
  unsupported: 'Notifications are not supported by this browser.',
  home: 'To get notifications, first add sbxw to your Home Screen (Share, then Add to Home Screen) and open it from there.',
  denied: 'Notifications are blocked for sbxw. To allow them again, open Settings, then Notifications, then sbxw on this iPhone, and turn Allow Notifications on.',
  mac: 'The Mac refused to turn notifications on.',
  unreachable: 'The Mac did not answer (is it awake?). Notifications stay off.',
  browser: 'This browser could not turn notifications on. Try again.',
  mac_off: 'Notifications are off on this phone, but the Mac did not answer, so it may still try to send one.',
  install: 'Notifications are off on this installation: turn them on again.',
};

// null when this browser can do Web Push, else the key of the one line to
// show. `env`: { serviceWorker, PushManager, Notification } as booleans.
function rcPushSupport(env, ua) {
  if (env && env.serviceWorker && env.PushManager && env.Notification) return null;
  return /\b(iPhone|iPad|iPod)\b/.test(String(ua || '')) ? 'home' : 'unsupported';
}

// The sentence for a failure; `reason` is the Mac's word, shown only when
// it is a plain lower-case word.
function rcPushFail(why, reason) {
  const base = RC_PUSH_TEXT[Object.hasOwn(RC_PUSH_TEXT, why) ? why : 'browser'];
  if (why === 'mac' && typeof reason === 'string' && /^[a-z_]{1,24}$/.test(reason)) return `${base.replace(/\.$/, '')} (${reason.replace(/_/g, ' ')}).`;
  return base;
}

// The tag the Mac gives for this device's subscription (API.md 2.8, GET
// /r/push/key): the first 16 hex digits of the SHA-256 of the endpoint.
// null when it cannot be computed (no crypto.subtle).
async function rcEndpointTag(subtle, endpoint) {
  try {
    const d = new Uint8Array(await subtle.digest('SHA-256', new TextEncoder().encode(endpoint)));
    return Array.from(d.slice(0, 8), b => b.toString(16).padStart(2, '0')).join('');
  } catch (_) { return null; }
}

// The switch's logic, with its world passed in: `api` (rcClient), `sw`
// (navigator.serviceWorker), `Notif` (Notification), `subtle`
// (crypto.subtle, optional). Every method answers, none throws.
function rcPush({ api, sw, Notif, subtle }) {
  let key = null;
  const sub = async () => {
    try { const r = await sw.getRegistration(); return r ? await r.pushManager.getSubscription() : null; } catch (_) { return null; }
  };
  const fail = (why, reason) => ({ ok: false, why, text: rcPushFail(why, reason) });
  const readKey = async () => {
    const k = await api.pushKey();
    if (k.ok && typeof k.data?.key === 'string' && rcFromB64u(k.data.key)) key = k.data.key;
    return k;
  };
  return {
    // 'on' only when the Mac and this browser both hold the subscription and
    // the permission stands; any disagreement shows 'off' (a tap repairs it).
    // The Mac on but this installation without it, or with another one (the
    // tags differ: the app was removed and added again), says so: the Mac
    // would send where nothing is shown.
    async state() {
      const k = await readKey();
      if (Notif.permission === 'denied') return { state: 'denied', text: RC_PUSH_TEXT.denied };
      if (!k.ok) return { state: 'off', text: k.unreachable ? RC_PUSH_TEXT.unreachable : '' };
      const mine = await sub();
      if (k.data.on !== true) return { state: 'off', text: '' };
      let same = !!mine && Notif.permission === 'granted';
      if (same && subtle && typeof k.data.tag === 'string') {
        const t = await rcEndpointTag(subtle, mine.endpoint);
        if (t !== null && t !== k.data.tag) same = false;
      }
      return same ? { state: 'on', text: '' } : { state: 'off', text: RC_PUSH_TEXT.install };
    },
    // From the tap itself: the worker registers while the permission is
    // asked (the prompt needs the tap, so nothing is awaited before it).
    async enable() {
      if (Notif.permission === 'denied') return fail('denied');
      let reg;
      try { reg = sw.register('/sw.js', { scope: '/' }); reg.catch(() => {}); } catch (_) { return fail('browser'); }
      let perm;
      try { perm = await Notif.requestPermission(); } catch (_) { return fail('browser'); }
      if (perm !== 'granted') return fail('denied');
      let s;
      try {
        await reg;
        const ready = await sw.ready;
        if (!key) await readKey();
        const k = key && rcFromB64u(key);
        if (!k) return fail('mac');
        const old = await ready.pushManager.getSubscription();
        if (old) await old.unsubscribe();
        s = await ready.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: k });
      } catch (_) { return fail('browser'); }
      const res = await api.pushSubscribe({ endpoint: s.endpoint, p256dh: rcB64u(s.getKey('p256dh')), auth: rcB64u(s.getKey('auth')) });
      if (res.ok && res.data?.ok === true) return { ok: true };
      try { await s.unsubscribe(); } catch (_) {}
      return res.unreachable || res.status === 0 ? fail('unreachable') : fail('mac', res.data?.reason ?? res.data?.error);
    },
    async disable() {
      await this.dropBrowser();
      const res = await api.pushUnsubscribe();
      return res.ok ? { ok: true } : fail('mac_off');
    },
    // Browser side only (Forget this device, a `bye`): the Mac already
    // dropped its copy. Never throws.
    async dropBrowser() {
      try { const s = await sub(); if (s) await s.unsubscribe(); } catch (_) {}
    },
  };
}

// The person is looking: clear the icon's badge, when the API exists.
function rcClearBadge(nav) {
  try {
    if (nav && typeof nav.clearAppBadge === 'function') return Promise.resolve(nav.clearAppBadge()).catch(() => {});
  } catch (_) {}
  return Promise.resolve();
}

// ══ Canvas: one shot per page, zoom, pins ═════════════════════════════════
// The contract is .sbxw-artifacts/remote/API-canvas.md. The geometry below
// is pure: the page measures, these compute.
const RC_CV_MAX_ZOOM = 4;
// The pager: an index kept inside [0, n - 1] (0 when there is no page).
function rcCvPage(i, n, delta = 0) {
  const count = Math.max(0, Math.floor(Number(n)) || 0);
  if (!count) return 0;
  const j = (Math.trunc(Number(i)) || 0) + (Math.trunc(Number(delta)) || 0);
  return Math.min(count - 1, Math.max(0, j));
}
function rcCvCounter(i, n) {
  const count = Math.max(0, Math.floor(Number(n)) || 0);
  return count ? `${rcCvPage(i, count) + 1} / ${count}` : '0 / 0';
}
// A finished one-finger move: +1 (next), -1 (previous) or 0. Only a
// mostly horizontal move, and never while the picture is zoomed in (then
// the finger pans).
function rcCvSwipe(dx, dy, scale = 1, min = 50) {
  if (!(scale <= 1.01)) return 0;
  if (Math.abs(dx) < min || Math.abs(dx) < Math.abs(dy) * 1.5) return 0;
  return dx < 0 ? 1 : -1;
}
// The view of the picture: scale s about the box's top left, then a move
// (tx, ty), kept so the picture always covers its box (no empty edge) and
// s stays in [1, RC_CV_MAX_ZOOM].
function rcCvClamp(view, w, h) {
  const s = Math.min(RC_CV_MAX_ZOOM, Math.max(1, Number(view?.s) || 1));
  const clamp = (v, lo) => Math.min(0, Math.max(lo, Number(v) || 0));
  return { s, tx: clamp(view?.tx, w - w * s), ty: clamp(view?.ty, h - h * s) };
}
// Two fingers: the point of the picture under the first midpoint (m0) stays
// under the current one (m1), scaled by the change of distance (d1 / d0).
// Points are relative to the box's top left.
function rcCvPinch(v0, m0, d0, m1, d1, w, h) {
  const s = Math.min(RC_CV_MAX_ZOOM, Math.max(1, v0.s * (d0 > 0 ? d1 / d0 : 1)));
  const px = (m0.x - v0.tx) / v0.s;
  const py = (m0.y - v0.ty) / v0.s;
  return rcCvClamp({ s, tx: m1.x - px * s, ty: m1.y - py * s }, w, h);
}
// A tap at (x, y), relative to the box's top left, as a fraction of the
// picture (0..1 each way, 4 decimals), or null when it is off the picture.
function rcCvToFrac(x, y, view, w, h) {
  if (!(w > 0 && h > 0)) return null;
  const fx = (x - view.tx) / view.s / w;
  const fy = (y - view.ty) / view.s / h;
  if (!(fx >= 0 && fx <= 1 && fy >= 0 && fy <= 1)) return null;
  const r = v => Math.round(v * 1e4) / 1e4;
  return { fx: r(fx), fy: r(fy) };
}
// Where a fraction of the picture is on screen, relative to the box.
function rcCvFromFrac(fx, fy, view, w, h) {
  return { x: view.tx + fx * w * view.s, y: view.ty + fy * h * view.s };
}

// ── View mode and pin mode ──
// View mode (the default): a tap on the picture places nothing; a double tap
// zooms. Pin mode: a tap on the picture places a pin. A pin already there
// opens for editing in both modes.
const RC_CV_DOUBLE_ZOOM = 2.5;
const rcCvMode = m => (m === 'pin' ? 'pin' : 'view');
// What a finished still tap does: { kind: 'edit-pin' | 'new-pin' | 'none', fx, fy }.
function rcCvTapAction(mode, onPin, view, x, y, w, h) {
  if (onPin) return { kind: 'edit-pin' };
  if (rcCvMode(mode) !== 'pin') return { kind: 'none' };
  const f = rcCvToFrac(x, y, view, w, h);
  return f ? { kind: 'new-pin', fx: f.fx, fy: f.fy } : { kind: 'none' };
}
// A double tap at (x, y) (relative to the box's top left): zoomed in, back to
// 1x; at 1x, to RC_CV_DOUBLE_ZOOM about that point (it stays under the finger).
function rcCvDoubleTap(view, x, y, w, h) {
  if ((Number(view?.s) || 1) > 1.01) return { s: 1, tx: 0, ty: 0 };
  const s = Math.min(RC_CV_MAX_ZOOM, RC_CV_DOUBLE_ZOOM);
  return rcCvClamp({ s, tx: x - x * s, ty: y - y * s }, w, h);
}
// Two quick, close taps make a double tap.
function rcCvIsDouble(prev, now, x, y, gap = 320, dist = 32) {
  return !!prev && now - prev.t >= 0 && now - prev.t < gap && Math.hypot(x - prev.x, y - prev.y) < dist;
}
// Switching mode. Leaving pin mode with an unsaved remark follows rcCvHold:
// the mode stays and the sentence says why. `edit` is the open editor's remark
// (or null), `text` its field.
function rcCvToggleMode(mode, edit, text) {
  const cur = rcCvMode(mode);
  if (cur === 'view') return { mode: 'pin', hold: '' };
  const hold = edit && edit.kind === 'pin' ? rcCvHold(edit, text) : '';
  return hold ? { mode: 'pin', hold } : { mode: 'view', hold: '' };
}
function rcCvHint(mode) {
  return rcCvMode(mode) === 'pin'
    ? 'Tap to place a pin. Tap a pin to edit it. Pinch to zoom.'
    : 'Double-tap or pinch to zoom; swipe to change shot. Turn on Remark to place pins.';
}
// ── Full screen: the current shot alone, in a fixed overlay ──
// State { open, idx, view }. Opening and moving to another shot start at 1x;
// closing gives back the same shot at 1x.
const RC_CV_VIEW1 = () => ({ s: 1, tx: 0, ty: 0 });
function rcCvFsOpen(idx, n) {
  const count = Math.max(0, Math.floor(Number(n)) || 0);
  return count ? { open: true, idx: rcCvPage(idx, count), view: RC_CV_VIEW1() } : { open: false, idx: 0, view: RC_CV_VIEW1() };
}
function rcCvFsGo(state, delta, n) {
  if (!state?.open) return state;
  return { open: true, idx: rcCvPage(state.idx, n, delta), view: RC_CV_VIEW1() };
}
function rcCvFsClose(state) {
  return { open: false, idx: Math.max(0, Math.trunc(Number(state?.idx)) || 0), view: RC_CV_VIEW1() };
}

// ── The canvas's data, read defensively ──
// Ids and file names have one shape each; anything else is not used, so a
// picture only ever loads from /r/canvas/<id>/images/<file> as the manifest
// names it.
const rcCvId = id => typeof id === 'string' && /^c-[0-9a-f]{16}$/.test(id);
const rcCvFile = f => typeof f === 'string' && /^s-[0-9a-f]{12}\.(?:png|jpg|webp|gif)$/.test(f);
function rcCvImage(id, file) {
  return rcCvId(id) && rcCvFile(file) ? `/r/canvas/${id}/images/${file}` : null;
}
const rcCvNum = (v, d = 0) => (Number.isFinite(Number(v)) && v !== null && v !== '' ? Number(v) : d);
const rcCvInt = v => Math.max(0, Math.floor(rcCvNum(v)));
// The list of one sandbox's canvases, in the daemon's order.
function rcCvList(data) {
  return (Array.isArray(data?.canvases) ? data.canvases : []).filter(c => c && rcCvId(c.id)).map(c => ({
    id: c.id,
    title: rcStr(c.title) || 'Untitled canvas',
    shots: rcCvInt(c.shots),
    rounds: rcCvInt(c.rounds),
    new_shots: rcCvInt(c.new_shots),
    draft: rcCvInt(c.draft),
    draft_examples: rcCvInt(c.draft_examples),
    updated_ms: rcCvNum(c.updated_ms),
  }));
}
// How many canvases have new shots: the Canvas tab's badge.
function rcCvBadge(list) {
  return (Array.isArray(list) ? list : []).filter(c => c.new_shots > 0).length;
}
// The pins and notes of the draft, checked; for the manifest and for a
// remarks answer alike.
function rcCvRemarks(data) {
  const pins = (Array.isArray(data?.pins) ? data.pins : []).filter(p => p && Number.isInteger(p.n) && p.n > 0 && typeof p.text === 'string')
    .map(p => ({ n: p.n, shot: rcStr(p.shot) || null, x: Math.min(100, Math.max(0, rcCvNum(p.x))), y: Math.min(100, Math.max(0, rcCvNum(p.y))), text: p.text }));
  const notes = (Array.isArray(data?.notes) ? data.notes : []).filter(t => t && Number.isInteger(t.n) && t.n > 0 && typeof t.text === 'string')
    .map(t => ({ n: t.n, shot: rcStr(t.shot) || null, text: t.text }));
  return { pins, notes, draft: rcCvInt(data?.draft) };
}
// The manifest: shots in the daemon's order, those without a usable picture
// left out.
function rcCvManifest(data) {
  if (!data || !rcCvId(data.id)) return null;
  const limits = data.limits && typeof data.limits === 'object' ? data.limits : {};
  return {
    id: data.id,
    title: rcStr(data.title) || 'Untitled canvas',
    updated_ms: rcCvNum(data.updated_ms),
    rounds: (Array.isArray(data.rounds) ? data.rounds : []).filter(r => r && Number.isInteger(r.n)).map(r => ({ n: r.n, at_ms: rcCvNum(r.at_ms), shots: rcCvInt(r.shots) })),
    shots: (Array.isArray(data.shots) ? data.shots : []).filter(s => s && typeof s.id === 'string' && s.id && rcCvFile(s.file)).map(s => ({
      id: s.id,
      round: rcCvInt(s.round),
      title: rcStr(s.title),
      note: rcStr(s.note),
      width: Number.isFinite(s.width) && s.width > 0 ? Math.floor(s.width) : null,
      file: s.file,
      px_w: rcCvInt(s.px_w) || 1,
      px_h: rcCvInt(s.px_h) || 1,
      new: s.new === true,
    })),
    ...rcCvRemarks(data),
    draft_examples: rcCvInt(data.draft_examples),
    text_max: rcCvInt(limits.text) || 2000,
  };
}
// The line under a shot's title: its round, whether it is new, the width.
function rcCvShotLine(shot, rounds = 0) {
  const parts = [rounds > 1 || shot.round ? `Round ${shot.round}` : ''];
  if (shot.new) parts.push('new');
  if (shot.width) parts.push(`${shot.width} px wide`);
  return parts.filter(Boolean).join(' · ');
}
// "3 shots · 2 rounds", a canvas row's line.
function rcCvListLine(c, now) {
  const p = [c.shots === 1 ? '1 shot' : `${c.shots} shots`];
  if (c.rounds > 1) p.push(`${c.rounds} rounds`);
  if (c.draft) p.push(c.draft === 1 ? '1 remark not sent' : `${c.draft} remarks not sent`);
  const ago = rcAgo(c.updated_ms, now);
  if (ago) p.push(ago === 'now' ? 'updated now' : `updated ${ago} ago`);
  return p.join(' · ');
}
// A canvas row of the list, every string set as text.
function rcCvRowEl(doc, c, now) {
  const b = doc.createElement('button');
  b.className = 'rc-cv-row';
  b.type = 'button';
  const t = doc.createElement('span');
  t.className = 'rc-cv-row-title';
  t.textContent = c.title;
  const line = doc.createElement('span');
  line.className = 'rc-cv-row-line';
  line.textContent = rcCvListLine(c, now);
  b.append(t, line);
  if (c.new_shots > 0) {
    const n = doc.createElement('span');
    n.className = 'rc-cv-new';
    n.textContent = `${c.new_shots} new`;
    b.append(n);
  }
  return b;
}
// What "Send remarks" asks before it sends: every remark in full, grouped
// by shot in the shots' order; what the phone cannot show is counted.
function rcCvConfirm(m) {
  const pins = m?.pins || [], notes = m?.notes || [], shots = m?.shots || [];
  const groups = [];
  const at = (n, i) => `Shot ${i + 1}${n ? ` · ${n}` : ''}`;
  const items = id => [
    ...pins.filter(p => p.shot === id).map(p => `Pin ${p.n} (${Math.round(p.x)}%, ${Math.round(p.y)}%): ${p.text}`),
    ...notes.filter(t => t.shot === id).map(t => `Note ${t.n}: ${t.text}`),
  ];
  shots.forEach((s, i) => { const it = items(s.id); if (it.length) groups.push({ head: at(s.title, i), items: it }); });
  const known = new Set(shots.map(s => s.id));
  const loose = [...pins.filter(p => !known.has(p.shot)).map(p => `Pin ${p.n}: ${p.text}`), ...notes.filter(t => !known.has(t.shot)).map(t => `Note ${t.n}: ${t.text}`)];
  if (loose.length) groups.push({ head: 'Not on a shot shown here', items: loose });
  const listed = pins.length + notes.length;
  const total = Math.max(listed, rcCvInt(m?.draft));
  const notes_ = [];
  if (total > listed) notes_.push(total - listed === 1 ? '1 more remark is on an example picture (shown on the Mac only).' : `${total - listed} more remarks are on example pictures (shown on the Mac only).`);
  if (m?.draft_examples) notes_.push('This round holds example pictures added on the Mac: it can only be sent from the Mac.');
  return {
    title: total === 1 ? 'Send 1 remark?' : `Send ${total} remarks?`,
    line: `One round of remarks on "${m?.title || 'this canvas'}", for the session of this sandbox. sbxw types one line asking it to read them; once sent, they cannot be changed.`,
    groups,
    notes: notes_,
    empty: total === 0,
    blocked: !!m?.draft_examples,
  };
}
// A remark being typed is never lost: while the editor is open with text
// that differs from what is saved, leaving the shot (swipe, Previous/Next,
// back to the list, Send remarks) does nothing and says why. Returns the
// sentence, or '' when leaving is fine (no editor, nothing typed, unchanged).
function rcCvHold(edit, text) {
  if (!edit) return '';
  const now = String(text ?? '').trim();
  const was = String(edit.orig ?? '').trim();
  if (now === was) return '';
  const what = edit.n == null ? `the new ${edit.kind === 'pin' ? 'pin' : 'note'}` : `${edit.kind === 'pin' ? 'pin' : 'note'} ${edit.n}`;
  return `Save or Cancel ${what} first: its text is not saved yet.`;
}
// The confirmation as plain text (one line per remark), as tests read it.
function rcCvConfirmText(c) {
  return [c.title, c.line, ...c.groups.flatMap(g => [g.head, ...g.items.map(i => `  ${i}`)]), ...c.notes].join('\n');
}
// The send's answer, as one sentence and a tone.
function rcCvSubmitResult(res) {
  if (res.ok && res.data?.ok) return { tone: 'ok', text: `Sent: round ${rcCvInt(res.data.review)} of remarks. The session was asked to read them.`, sent: true };
  if (res.unreachable || res.status === 0) return { tone: 'err', text: 'Not sent: sbxw is unreachable (is the Mac awake?). Your remarks are kept on the Mac.' };
  const d = res.data || {};
  const err = typeof d.error === 'string' && d.error.trim() ? ` (sbxw: ${d.error.trim().slice(0, 200)})` : '';
  if (d.reason === 'not_submitted') return { tone: 'warn', sent: true, text: `Round ${rcCvInt(d.review)} is recorded, but the line asking the session to read it was NOT submitted: the session started asking you something at the Mac. The line waits unsent in its input; check on your Mac.` };
  if (d.reason === 'examples') return { tone: 'err', text: 'Not sent: this round holds example pictures added on the Mac. Send it from the Mac.' };
  if (Object.hasOwn(RC_MSG_REASON, d.reason || '')) return { tone: 'err', text: `Not sent. ${RC_MSG_REASON[d.reason]}${err}` };
  if (res.status === 409) return { tone: 'err', text: `Not sent: this sandbox is read only from the phone. Send from the Mac.${err}` };
  if (res.status === 400) return { tone: 'err', text: 'Not sent: there is no new remark to send.' };
  if (res.status === 404) return { tone: 'err', text: 'Not sent: this canvas is no longer reachable (the sandbox stopped?).' };
  if (res.status === 429) return { tone: 'err', text: 'Not sent: too many requests at once. Try again in a moment.' };
  return { tone: 'err', text: `Not sent: the Mac answered ${res.status}${err}.` };
}
// One remark's save, as one sentence and a tone.
function rcCvRemarkResult(res, what) {
  if (res.ok && res.data?.ok) {
    const m = rcCvInt(res.data.masked);
    return { tone: 'ok', text: `${what}: saved${m ? ` (${m === 1 ? '1 word' : `${m} words`} masked by your anonymise rules)` : ''}.` };
  }
  if (res.unreachable || res.status === 0) return { tone: 'err', text: `${what}: not saved, sbxw is unreachable. Your text is still in the field.` };
  const err = typeof res.data?.error === 'string' && res.data.error.trim() ? res.data.error.trim().slice(0, 200) : `the Mac answered ${res.status}`;
  if (res.status === 404) return { tone: 'err', reload: true, text: `${what}: not saved, it was already sent or removed. The canvas was read again.` };
  if (res.status === 409) return { tone: 'err', text: `${what}: not saved, the canvas is full (${err}).` };
  return { tone: 'err', text: `${what}: not saved (${err}).` };
}

// The plan usage (GET /r/usage): the two windows as rows for the menu block
// and the compact line of the picker. Pure: `now` in ms, local time for the
// reset's clock. A window with no percentage is left out; none at all means
// `has: false` ("No reading yet").
const RC_USAGE_DAYS = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
function rcUsageTone(pct) { return pct >= 90 ? 'red' : pct >= 75 ? 'amber' : 'ok'; }
function rcUsageReset(at, now) {
  const t = Number(at);
  if (at == null || !Number.isFinite(t) || t <= 0) return '';
  const left = Math.round(t - now / 1000);
  if (left <= 0) return 'reset passed';
  if (left < 86400) {
    const m = Math.max(1, Math.round(left / 60));
    const h = Math.floor(m / 60);
    return h ? `resets in ${h} h ${m % 60} min` : `resets in ${m} min`;
  }
  const d = new Date(t * 1000);
  const two = n => String(n).padStart(2, '0');
  return `resets ${RC_USAGE_DAYS[d.getDay()]} ${two(d.getHours())}:${two(d.getMinutes())}`;
}
function rcUsage(u, now) {
  const rows = [];
  const o = u && typeof u === 'object' ? u : {};
  for (const [key, label, pk, rk] of [['five', '5 hours', 'five_hour_pct', 'five_hour_resets_at'], ['seven', '7 days', 'seven_day_pct', 'seven_day_resets_at']]) {
    const v = o[pk];
    if (typeof v !== 'number' || !Number.isFinite(v)) continue;
    const pct = Math.min(100, Math.max(0, Math.round(v)));
    rows.push({ key, label, pct, tone: rcUsageTone(pct), reset: rcUsageReset(o[rk], now) });
  }
  const upd = Number(o.updated_ms);
  const ago = upd > 0 && rows.length ? rcAgo(upd, now) : '';
  return {
    has: rows.length > 0,
    rows,
    updated: !ago ? '' : ago === 'now' ? 'updated just now' : `updated ${ago} ago`,
    compact: rows.map(r => `${r.key === 'five' ? '5 h' : '7 d'} ${r.pct}%`).join(' · '),
  };
}

// Files pasted from the clipboard (the composer and the Kanban form): the
// event's `files`, else its `items` of kind 'file'. An image without a useful
// name (none, or the generic "image.png" iOS gives) is named by its type and
// the time. Returns [] for a plain-text paste, so the caller leaves the event
// alone. `mk(file, name)` makes the renamed copy (the page's File).
const RC_PASTE_EXT = { 'image/png': 'png', 'image/jpeg': 'jpg', 'image/gif': 'gif', 'image/webp': 'webp', 'image/heic': 'heic', 'image/heif': 'heif', 'image/tiff': 'tiff', 'image/bmp': 'bmp' };
function rcPasteFiles(cd, now, mk) {
  let files = Array.from((cd && cd.files) || []);
  if (!files.length) {
    files = Array.from((cd && cd.items) || []).filter(i => i && i.kind === 'file' && typeof i.getAsFile === 'function').map(i => i.getAsFile()).filter(Boolean);
  }
  const stamp = new Date(Number(now) || 0).toISOString().replace(/\D/g, '').slice(0, 14);
  const make = typeof mk === 'function' ? mk : (f, name) => new File([f], name, { type: f.type });
  return files.map((f, i) => {
    const type = String(f.type || '');
    if (!type.startsWith('image/')) return f;
    const name = String(f.name || '');
    if (name && !/^image\.\w+$/i.test(name)) return f;
    const ext = RC_PASTE_EXT[type] || (/^image\/([a-z0-9]+)/.exec(type) || [])[1] || 'png';
    return make(f, `pasted-${stamp}${files.length > 1 ? `-${i + 1}` : ''}.${ext}`);
  });
}
// The Approvals kinds: one class (accent colour) and one icon id per kind.
// An unknown kind is neutral, so a new kind from the Mac never breaks a row.
const RC_AP_KINDS = {
  plans: { cls: 'rc-k-plans', icon: 'ap-i-plans', label: 'Plan' },
  merges: { cls: 'rc-k-merges', icon: 'ap-i-merges', label: 'Merge' },
  network: { cls: 'rc-k-network', icon: 'ap-i-network', label: 'Network request' },
  stopped: { cls: 'rc-k-stopped', icon: 'ap-i-stopped', label: 'Stopped sandbox' },
};
function rcApKind(what) {
  const k = Object.prototype.hasOwnProperty.call(RC_AP_KINDS, what) ? RC_AP_KINDS[what] : null;
  return k ? { ...k } : { cls: 'rc-k-neutral', icon: 'ap-i-neutral', label: '' };
}
// The one busy state of a button whose request is running: it keeps its
// colour, shows a small ring before its label (or `label` instead of the
// label), says aria-busy, and takes no tap. Off puts back what was there.
// On twice or off twice does nothing more. The ring is CSS (.rc-spin).
function rcBusy(button, on, label) {
  if (!button) return;
  const was = button._rcBusy;
  if (on) {
    if (was) return;
    const doc = button.ownerDocument;
    const spin = doc.createElement('span');
    spin.className = 'rc-spin';
    spin.setAttribute('aria-hidden', 'true');
    const saved = { disabled: !!button.disabled, nodes: null };
    if (typeof label === 'string') {
      saved.nodes = Array.from(button.childNodes);
      button.replaceChildren(spin, doc.createTextNode(label));
    } else {
      button.prepend(spin);
    }
    saved.spin = spin;
    button._rcBusy = saved;
    button.classList.add('rc-busy');
    button.setAttribute('aria-busy', 'true');
    button.disabled = true;
  } else {
    if (!was) return;
    button._rcBusy = null;
    if (was.nodes) button.replaceChildren(...was.nodes);
    else was.spin.remove();
    button.classList.remove('rc-busy');
    button.removeAttribute('aria-busy');
    button.disabled = was.disabled;
  }
}
