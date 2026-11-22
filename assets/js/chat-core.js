// ── Chat core ─────────────────────────────────────────────────────────────
// What the chat dock (chat-dock.js) and the browser side panel (sidepanel.js)
// have in common: how a `session` event moves a conversation along, and the
// pieces of a transcript (a message, the "working" line, a pending question)
// built the same way in both, on the same `cd-*` styles.
//
// A chat here is `{ log, state, steps, answerable, activity }`: the tab's own
// transcript (the daemon keeps only the last reply per session) plus what the
// latest event said about the agent. Nothing in this file knows which page it
// is on; each caller decides what to re-render and whom to tell.

// Is this event about a session sbxw drives? `/api/chat/push` types into
// sbxw's own terminal, so a session an editor, Claude Desktop or SSH started
// in the same container (`origin: 'remote'`) is not the conversation shown,
// and its events, re-emitted alongside every sibling's (see `emit_info`),
// would otherwise overwrite this one's state with theirs.
//
// A thread's agent is sbxw's too, but the daemon reads it as `remote`: origin
// is decided by "started in the workspace", and a thread starts in its
// worktree. It stays `remote` there on purpose: the main agent must remain the
// one tty session that typing and answering pick (`is_answerable`), so the
// worktree is recognised here instead.
function chatOwnSession(info) {
  return info.origin !== 'remote' || /\/\.sbxw-worktrees\/[a-z0-9-]+\/?$/.test(info.cwd || '');
}

function chatNew() {
  return { log: [], state: 'idle', steps: null, answerable: false, activity: null, handled: null };
}

// The one rule for "this event brings a reply to announce", the same as the
// desktop's (`reply_ready`, desktop/src/jarvis.rs); both are checked against
// tests/fixtures/reply-ready.json. `inTurn`: the reader last saw the session
// `working` or `attention`; `last`: the reply it last handled. A turn ends
// with a reply at `idle`, or while the session waits on background work of
// its own (`working` with `background` set): the interim reply. Once each.
function chatReplyReady(inTurn, info, last) {
  const reply = typeof info.reply === 'string' && info.reply.trim() ? info.reply : null;
  if (!reply || !inTurn || reply === last) return null;
  const ended = info.state === 'idle' || info.state === 'exited'
    || (info.state === 'working' && !!info.background && typeof info.background === 'object');
  return ended ? reply : null;
}

// Fold a `session` event into `c`. Returns what the reader may want to hear
// about: `{ reply }` when a turn just ended with an answer, `{ asked }` when a
// question just appeared, or null.
//
// `seed` is the first look, from `GET /api/sessions`: a chat with no
// transcript of its own starts from what the daemon knows, the last prompt
// and, once the turn ended, its answer.
function chatIngest(c, info, seed = false) {
  const prev = c.state;
  c.state = info.state === 'exited' ? 'idle' : info.state;
  c.activity = info.activity || null;
  c.steps = info.steps || (info.question ? [info.question] : null);
  c.answerable = !!info.answerable;

  if (seed) {
    if (!c.log.length && info.last_input) {
      c.log.push({ who: 'me', text: info.last_input });
      if (info.reply) c.log.push({ who: 'ai', text: info.reply });
    }
    // Known already: a later event of the same busy stop is not news.
    if (info.reply) c.handled = info.reply;
    return null;
  }
  // A new turn (not a busy stop): the same words later are a new answer.
  if (info.state === 'working' && !info.background) c.handled = null;
  const reply = chatReplyReady(prev === 'working' || prev === 'attention', info, c.handled ?? null);
  if (reply) {
    c.handled = reply;
    const last = c.log[c.log.length - 1];
    if (!(last?.who === 'ai' && last.text === reply)) c.log.push({ who: 'ai', text: reply });
    return { reply };
  }
  if (info.state === 'attention' && prev !== 'attention') {
    return { asked: c.steps?.[0]?.text || info.activity || 'Claude is waiting for you.' };
  }
  return null;
}

// Claude answers in Markdown. Just enough of it to read well in a narrow
// column (paragraphs, lists, code blocks, inline code, bold, italics, links,
// headings) and nothing that could carry markup of its own: the text is
// escaped first, and only these fixed tags are added to it.
function chatEscape(s) {
  return s.replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

// ── File paths in an agent's reply ─────────────────────────────────────────
// A token that looks like a path to a previewable file becomes a button that
// opens it in the Project panel's Files tab (see `fxOpenPath` in files.js).
// Detection is deliberately boring: ASCII path characters only, a slash (or a
// leading ./ ~/ /), no `..`, no scheme, a file name with an extension from the
// list below, an optional :line. The scan is linear: tokens are cut at
// whitespace and quotes, and anything over CHAT_FILE_MAX is dropped before any
// pattern runs on it. The path never enters the markup: the HTML carries a
// button with an integer, and the path waits in the caller's `files.paths`.
// Extensions the Files preview handles: images and video it draws, HTML in its
// sandboxed frame, any other non-binary file as text.
const CHAT_FILE_EXT = new Set([
  'md', 'markdown', 'txt', 'html', 'htm', 'json', 'csv', 'log', 'pdf',
  'png', 'jpg', 'jpeg', 'gif', 'webp', 'svg',
  'js', 'mjs', 'ts', 'tsx', 'jsx', 'py', 'rs', 'go', 'toml', 'yaml', 'yml', 'sh', 'css', 'xml', 'sql', 'rb', 'java', 'c', 'h', 'cpp',
]);
const CHAT_FILE_MAX = 300;
const CHAT_FILE_NAME = /^[A-Za-z0-9_+@%-][A-Za-z0-9_.+@%-]*$/;
const CHAT_FILE_CHARS = /^[A-Za-z0-9_.+@%\-\/~]+$/;

// `token` as `{ path, line }` when it is a file path worth linking, else null.
function chatFilePath(token) {
  if (typeof token !== 'string' || token.length === 0 || token.length > CHAT_FILE_MAX) return null;
  let t = token;
  let line = null;
  const m = /:(\d{1,7})(?::\d{1,7})?$/.exec(t);
  if (m) { line = Number(m[1]); t = t.slice(0, m.index); }
  if (!CHAT_FILE_CHARS.test(t)) return null;
  const slash = t.indexOf('/');
  if (slash < 0) return null;
  if (t.startsWith('~') && !t.startsWith('~/')) return null;
  if (t.endsWith('/')) return null;
  const parts = t.split('/');
  parts.forEach((seg, i) => { if (i === 0 && (seg === '' || seg === '~' || seg === '.')) parts[i] = null; });
  const segs = parts.filter(x => x !== null);
  for (const seg of segs) if (seg === '' || seg === '.' || seg === '..') return null;
  const name = segs[segs.length - 1];
  const dot = name.lastIndexOf('.');
  if (dot < 1) return null;
  if (!CHAT_FILE_NAME.test(name) || !CHAT_FILE_EXT.has(name.slice(dot + 1).toLowerCase())) return null;
  return { path: t, line };
}

// The path inside `s` between whitespace and quotes: trailing punctuation and
// emphasis stars are not part of it. `{ start, end, found }` or null.
function chatFileTokens(s) {
  const out = [];
  const re = /[^\s"'`<>()\[\]{}|\uE000-\uE003]+/g;
  let m;
  while ((m = re.exec(s))) {
    if (m[0].length > CHAT_FILE_MAX + 40) continue;
    let a = m.index, b = m.index + m[0].length;
    while (a < b && s[a] === '*') a++;
    while (b > a && /[*.,;:!?]/.test(s[b - 1])) b--;
    if (s.startsWith('](', b)) continue; // the label of a markdown link
    const found = chatFilePath(s.slice(a, b));
    if (found) out.push({ start: a, end: b, found });
  }
  return out;
}

// Code spans are lifted out first and replaced by a placeholder (private-use
// characters, stripped from the input beforehand so the text cannot forge
// one), so bold, italics and link labels can wrap a code span while its
// content is never read as markdown. Placeholders are excluded from link URLs.
// With `files` ({ paths: [] }), a code span that is only a file path, and a
// bare path in the text, become file buttons; the path goes to `files.paths`
// and the button holds its index.
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

// `text` as chat markdown into `el`; its file paths open in `sandbox`'s Files
// tab where this page has one (the extension's side panel has none: there
// the path stays plain text).
function chatMarkdownInto(el, text, sandbox) {
  const canOpen = sandbox && typeof fxOpenPath === 'function';
  const files = canOpen ? { paths: [] } : null;
  el.innerHTML = chatMarkdown(text, files);
  if (canOpen) chatWireFiles(el, files, sandbox, fxOpenPath, typeof fxResolvePath === 'function' ? fxResolvePath : null);
}

// Give the file buttons `root` holds their behaviour: a press calls
// `open(sandbox, path)` once, with the path as the agent wrote it. Titles say
// the full path that will open and the sandbox. `files` belongs to this one
// rendering: a button can only reach the paths of its own message. Nothing
// runs before a press.
function chatWireFiles(root, files, sandbox, open, resolve) {
  root.querySelectorAll('button.chat-file').forEach(btn => {
    const found = files.paths[Number(btn.dataset.file)];
    if (!found) return;
    const abs = resolve ? resolve(sandbox, found.path) : null;
    btn.title = `Open ${abs || `${found.path} (relative to its workspace)`} in ${sandbox}'s Project files`;
    btn.addEventListener('click', () => open(sandbox, found.path));
  });
}

// One blank-line-separated block: a run of `- ` or `1. ` lines is a list, a
// `#` line a heading, anything else a paragraph that keeps its line breaks.
// Lists are real lists so a long item wraps under its own text, not under
// the bullet.
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

// ── The `>_` avatar ───────────────────────────────────────────────────────
// sbxw's prompt in the 5×7 dot-matrix of the corner job cards (util.js): the
// chat dock's launcher, the side panel's header and the extension's icon are
// one glyph. The cursor blinks at rest (CSS); while the agent works the grid
// fills and drains like a running job, unless the reader asked for less
// motion, when it simply stays lit.
const CHAT_GLYPH = [
  [['10000', '01000', '00100', '00010', '00100', '01000', '10000'], '#3fb950', 0, false],
  [['000', '000', '000', '000', '000', '000', '111'], '#58a6ff', 6, true],
];

// Draw the glyph into `grid`; `host` gets the `working` class while it sweeps.
function chatGlyph(grid, host) {
  const pixels = [];
  grid.style.setProperty('--cols', 9);
  CHAT_GLYPH.forEach(([pattern, color, offset, cursor]) => {
    for (let r = 0; r < 7; r++) for (let c = 0; c < pattern[r].length; c++) {
      if (pattern[r][c] !== '1') continue;
      const px = document.createElement('div');
      px.className = 'px lit' + (cursor ? ' cd-cursor' : '');
      px.style.gridColumn = offset + c + 1;
      px.style.gridRow = r + 1;
      px.style.setProperty('--c', color);
      grid.appendChild(px);
      pixels.push({ el: px, row: r });
    }
  });
  const still = window.matchMedia?.('(prefers-reduced-motion: reduce)').matches;
  let sweep = null;
  let on = false;
  return {
    setWorking(working) {
      if (working === on) return;
      on = working;
      host.classList.toggle('working', working);
      clearInterval(sweep);
      sweep = null;
      pixels.forEach(p => p.el.classList.add('lit'));
      if (!working || still) return;
      let level = 0, dir = 1;
      sweep = setInterval(() => {
        pixels.forEach(p => p.el.classList.toggle('lit', p.row >= 7 - level));
        level += dir;
        if (level >= 7 || level <= 0) dir = -dir;
      }, 110);
    },
  };
}

// `sandbox`, when given and the Project panel exists in this page, makes the
// file paths in an agent's reply open in that sandbox's Files tab.
function chatLine(cls, text, meta, sandbox) {
  const el = document.createElement('div');
  el.className = 'cd-msg ' + cls;
  if (meta) {
    const m = document.createElement('span');
    m.className = 'cd-meta';
    m.textContent = meta;
    el.appendChild(m);
  }
  if (cls === 'ai') {
    const body = document.createElement('div');
    chatMarkdownInto(body, text, sandbox);
    el.appendChild(body);
  } else {
    el.appendChild(document.createTextNode(text));
  }
  return el;
}

function chatStatus(text) {
  const s = document.createElement('div');
  s.className = 'cd-status';
  const bar = document.createElement('span');
  bar.className = 'cd-bar';
  s.append(bar, text);
  return s;
}

// The "working" line, or the pending question, under a transcript.
function chatTail(c, questionOpts) {
  if (c.state === 'working') return chatStatus(c.activity ? `Claude is working · ${c.activity}` : 'Claude is working…');
  if (c.state === 'attention') return chatQuestion(c, questionOpts);
  return null;
}

// A pending `AskUserQuestion`: one row of options per step, submitted at once
// like the terminal does. A prompt sbxw cannot type into (see `is_answerable`)
// or one with no structured steps (a permission request) is answered in the
// session itself, which `openLabel`/`onOpen` lead to.
//
// `sandbox`/`mode` address the session for `POST /api/answer`; `onChange` runs
// after an answer is sent or refused, for the caller to save and re-render.
function chatQuestion(c, { sandbox, mode = 'claude', openLabel, onOpen, onChange }) {
  const box = document.createElement('div');
  box.className = 'cd-question';
  if (!c.steps?.length || !c.answerable) {
    const p = document.createElement('p');
    p.textContent = c.steps?.[0]?.text || c.activity || 'Claude is waiting for you.';
    const open = document.createElement('button');
    open.type = 'button';
    open.textContent = openLabel;
    open.addEventListener('click', onOpen);
    box.append(p, open);
    return box;
  }
  const picked = c.steps.map(() => null);
  const submit = document.createElement('button');
  submit.type = 'button';
  submit.className = 'cd-answer';
  submit.textContent = 'Send answer';
  submit.hidden = c.steps.length === 1;
  const send = async () => {
    const indices = picked.map(i => i + 1);
    box.querySelectorAll('button').forEach(b => { b.disabled = true; });
    const labels = c.steps.map((s, i) => s.options[picked[i]]).join(' · ');
    try {
      const res = await fetch('/api/answer', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ sandbox, mode, index: indices[0], indices }),
      });
      if (!res.ok) throw new Error(`the daemon answered ${res.status}`);
      c.log.push({ who: 'me', text: labels });
      c.steps = null;
      c.state = 'working';
    } catch (e) {
      c.log.push({ who: 'err', text: `Answer not sent: ${e.message}. ${openLabel} instead.` });
    }
    onChange();
  };
  c.steps.forEach((step, si) => {
    const p = document.createElement('p');
    p.textContent = step.text;
    const opts = document.createElement('div');
    opts.className = 'cd-opts';
    step.options.forEach((label, oi) => {
      const b = document.createElement('button');
      b.type = 'button';
      b.textContent = label;
      if (step.context?.[oi]) b.title = step.context[oi];
      b.addEventListener('click', () => {
        picked[si] = oi;
        opts.querySelectorAll('button').forEach((x, xi) => x.classList.toggle('picked', xi === oi));
        if (c.steps.length === 1) send();
        else submit.disabled = picked.includes(null);
      });
      opts.appendChild(b);
    });
    box.append(p, opts);
  });
  submit.disabled = true;
  submit.addEventListener('click', send);
  box.appendChild(submit);
  return box;
}

// Enter sends, Shift+Enter is a new line, and the box grows with its text up
// to its max-height. `onSend` gets the trimmed text; the box is already cleared.
function chatComposer(form, input, onSend, { gate, allowEmpty } = {}) {
  form.addEventListener('submit', e => {
    e.preventDefault();
    const text = input.value.trim();
    // `gate()` is the reason a message cannot go yet ('' when it can): the
    // typed text stays. `allowEmpty()`: files alone make a message.
    if (gate && gate()) return;
    if (!text && !(allowEmpty && allowEmpty())) return;
    input.value = '';
    input.style.height = '';
    onSend(text);
  });
  input.addEventListener('keydown', e => {
    if (e.key !== 'Enter' || e.shiftKey) return;
    e.preventDefault();
    // Not the Enter that confirms an IME composition; 229 is Safari's, which
    // has already cleared isComposing (util.js's isEnterKey is not loaded in
    // the side panel).
    if (!e.isComposing && e.keyCode !== 229) form.requestSubmit();
  });
  // CSS grows the box (`field-sizing: content`, capped by max-height in
  // app.css); measure it by hand only where that is not supported.
  if (!CSS.supports('field-sizing', 'content')) {
    input.addEventListener('input', () => {
      input.style.height = 'auto';
      input.style.height = Math.min(input.scrollHeight, 120) + 'px';
    });
  }
}

// ── A round of questions ──────────────────────────────────────────────────
// /grill-me, and the `ask_user_questions` tool (relay-mcp.js, `askRound`),
// write a round as an optional intro, then `❓ **Q1 - Title** : body` and
// `➡️ the recommendation` per question, questions separated by `---`. Cut
// there, so no question is lost or invented. Shared by the Concierge, which
// asks a round found in an agent's session one question at a time, and the
// relay dialog, which puts a round asked with the tool on one form.
function chatParseRound(text) {
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

// ── Files dropped on a chat ───────────────────────────────────────────────
// BEGIN chat attach: the pure part (no DOM, no fetch of its own) is what
// tests/js/chat-drop.test.mjs runs; the DOM part below it is a thin skin.
//
// A file dropped on the chat dock or the side panel (or picked with its
// Attach button) is checked in the page, uploaded to the chat's own sandbox
// with the existing `POST /api/sandboxes/:name/upload` (the sidebar Upload
// box's route: same folder, same 128 MB ceiling, same refusal of an empty
// file) and shown as a chip in the composer. NOTHING is typed to the agent by
// the drop: on Send the message carries the paths the daemon answered, in one
// fixed block after the person's own words. A removed chip leaves its file in
// the sandbox: no route deletes one.
//
// File names are data: they reach the page only through textContent and
// setAttribute, and the daemon alone decides the name on disk. File contents
// are never read here, only handed to the upload.
const CHAT_ATTACH_MAX_FILES = 10;                 // per message (a page cap; the daemon has none)
const CHAT_ATTACH_MAX_BYTES = 128 * 1024 * 1024;  // UPLOAD_MAX in http/ui.rs
const CHAT_ATTACH_STAYS = '/tmp/sbxw-uploads';    // where a removed chip's file stays
// File names that look like a secret: kanban.js's KB_SECRET_NAMES, the
// desktop's SECRET_NAMES (the side panel loads neither, so the list is here too;
// tests/js/chat-drop.test.mjs keeps it equal to kanban.js's).
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

// A name for a message to the person: no control or invisible characters (the
// one list, invisible.js; each becomes ?), at most 60
// characters, a long one cut in the middle so its extension stays visible.
function chatShortName(name) {
  const s = [...String(name ?? '').replace(new RegExp(`[\\u0000-\\u001f\\u007f-\\u009f${INVISIBLE_CLASS}]`, 'gu'), '?')];
  return s.length <= 60 ? s.join('') : `${s.slice(0, 40).join('')}…${s.slice(-16).join('')}`;
}

// The name sent as `?name=`: the last path component, no control characters,
// kept short with its extension (the daemon cuts at 120 and keeps only
// [A-Za-z0-9._-]; it alone decides the name on disk).
function chatUploadName(name) {
  const base = String(name ?? '').split(/[\/\\]/).pop().replace(/[\u0000-\u001f\u007f]/g, '');
  const s = [...base];
  if (s.length <= 100) return s.join('');
  const ext = /\.[A-Za-z0-9]{1,10}$/.exec(base)?.[0] || '';
  return s.slice(0, 90).join('') + ext;
}

const chatUploadUrl = (sandbox, name) =>
  `/api/sandboxes/${encodeURIComponent(sandbox)}/upload?name=${encodeURIComponent(chatUploadName(name))}`;

// The block added after the person's text when files go with it; paths are
// the daemon's answers, never rebuilt from names.
function chatAttachBlock(paths) {
  if (!paths.length) return '';
  return `Attached files (uploaded to this sandbox):\n${paths.map(p => `- ${p}`).join('\n')}`;
}
function chatWithAttachments(text, paths) {
  const block = chatAttachBlock(paths);
  return block ? (text ? `${text}\n\n${block}` : block) : text;
}

// Files of a drop, read synchronously (the browser empties a DataTransfer once
// the handler returns). `null`: not files at all (a text selection, a link, an
// image from a page): left to the browser as today. A folder is named, never
// read. Nothing is fetched.
function chatDroppedFiles(dt) {
  if (!dt || ![...(dt.types || [])].includes('Files')) return null;
  const files = [], folders = [];
  if (dt.items && dt.items.length) {
    for (const it of dt.items) {
      if (it.kind !== 'file') continue;
      const entry = typeof it.webkitGetAsEntry === 'function' ? it.webkitGetAsEntry() : null;
      if (entry && entry.isDirectory) { folders.push(entry.name); continue; }
      const f = it.getAsFile();
      if (f) files.push(f);
    }
  } else {
    files.push(...(dt.files || []));
  }
  return { files, folders };
}

// The checks, before any byte is sent. `held`: files already in the tray.
// → { take, refused: [{name, why}], ask: [reason] }
function chatCheckAttach(held, incoming, folders = []) {
  const take = [], refused = [];
  for (const name of folders) refused.push({ name, why: 'a folder is not attached; drop the files in it instead' });
  for (const f of incoming) {
    let why = '';
    if (!f.size) why = 'the file is empty';
    else if (f.size > CHAT_ATTACH_MAX_BYTES) why = `over ${chatSize(CHAT_ATTACH_MAX_BYTES)}, the most one upload takes`;
    else if (held + take.length >= CHAT_ATTACH_MAX_FILES) why = `a message takes ${CHAT_ATTACH_MAX_FILES} files at most`;
    if (why) refused.push({ name: f.name, why }); else take.push(f);
  }
  const ask = take.filter(f => chatSecretName(f.name)).map(f => `${chatShortName(f.name)}: its name looks like a secret (keys, tokens, credentials)`);
  return { take, refused, ask };
}

// XMLHttpRequest, not fetch: it reports how much of the body has gone.
function chatUploadXhr(sandbox, file, onProgress, signal) {
  return new Promise((resolve, reject) => {
    const x = new XMLHttpRequest();
    x.open('POST', chatUploadUrl(sandbox, file.name));
    x.setRequestHeader('Content-Type', file.type || 'application/octet-stream');
    x.upload.onprogress = e => { if (e.lengthComputable) onProgress(e.loaded / e.total); };
    x.onload = () => {
      let d = null;
      try { d = JSON.parse(x.responseText); } catch (_) {}
      if (x.status === 401) reject(new Error('signed out of sbxw: reload the page'));
      else if (d && d.ok && typeof d.path === 'string') resolve(d.path);
      else reject(new Error((d && d.error) || `HTTP ${x.status}`));
    };
    x.onerror = () => reject(new Error('sbxw did not answer'));
    x.onabort = () => reject(new Error('cancelled'));
    signal.addEventListener('abort', () => x.abort());
    x.send(file);
  });
}

// One chat's attachments: chips (queued → sending → done | failed), at most
// one upload at a time, and the drop held back while a ticked review is due.
// `sandbox()` names the chat's sandbox; `send(sandbox, file, onProgress, signal)`
// resolves the path the daemon answered; `onChange` redraws.
function chatAttachTray({ sandbox, send = chatUploadXhr, onChange = () => {} }) {
  const t = {
    chips: [], held: null, note: '', seq: 0, active: null,
    offer(files, folders = []) {
      const { take, refused, ask } = chatCheckAttach(t.chips.length + (t.held ? t.held.files.length : 0), files, folders);
      t.note = refused.map(r => `Not attached: ${chatShortName(r.name)} (${r.why}).`).join(' ');
      if (!take.length) { onChange(); return; }
      // More files held means a new look: the tick starts again.
      if (ask.length) t.held = { files: [...(t.held ? t.held.files : []), ...take], reasons: [...(t.held ? t.held.reasons : []), ...ask], ticked: false };
      else t.start(take);
      onChange();
    },
    // Only an explicit tick sends what was held.
    confirmHeld(ticked) {
      if (!t.held) return false;
      if (ticked !== true) { t.note = 'Tick "I have looked at these files and want to attach them" first: nothing is sent before.'; onChange(); return false; }
      const files = t.held.files;
      t.held = null;
      t.note = '';
      t.start(files);
      onChange();
      return true;
    },
    discardHeld() { t.held = null; onChange(); },
    start(files) {
      for (const file of files) t.chips.push({ id: ++t.seq, file, name: file.name, size: file.size, state: 'queued', pct: 0, path: null, error: '' });
      t.pump();
    },
    pump() {
      if (t.active || !sandbox()) return;
      const chip = t.chips.find(c => c.state === 'queued');
      if (!chip) return;
      const target = sandbox();
      const ctl = new AbortController();
      chip.state = 'sending';
      chip.pct = 0;
      t.active = { chip, ctl };
      Promise.resolve().then(() => send(target, chip.file, p => { chip.pct = p; onChange(); }, ctl.signal)).then(path => {
        if (t.active?.chip !== chip) return;
        chip.state = 'done';
        chip.pct = 1;
        chip.path = path;
      }, e => {
        if (t.active?.chip !== chip) return;
        chip.state = 'failed';
        chip.error = String(e && e.message || 'upload failed');
      }).then(() => {
        if (t.active?.chip === chip) t.active = null;
        t.pump();
        onChange();
      });
    },
    remove(id) {
      const i = t.chips.findIndex(c => c.id === id);
      if (i < 0) return;
      const [chip] = t.chips.splice(i, 1);
      if (t.active?.chip === chip) { const a = t.active; t.active = null; a.ctl.abort(); t.pump(); }
      onChange();
    },
    retry(id) {
      const chip = t.chips.find(c => c.id === id);
      if (!chip || chip.state !== 'failed') return;
      chip.state = 'queued';
      chip.error = '';
      t.pump();
      onChange();
    },
    // What stops Send, or ''.
    problem() {
      if (t.held) return 'Review the files waiting for your tick first, or cancel them.';
      if (t.chips.some(c => c.state === 'queued' || c.state === 'sending')) return 'Files are still uploading. Wait, or remove them.';
      if (t.chips.some(c => c.state === 'failed')) return 'An upload failed: retry it or remove it.';
      return '';
    },
    count() { return t.chips.length; },
    // Take the done chips for a message; `restore` puts them back if it fails.
    take() { const done = t.chips.splice(0); onChange(); return done; },
    restore(chips) { t.chips.unshift(...chips); onChange(); },
    clear() { t.chips.forEach(c => { if (t.active?.chip === c) t.active.ctl.abort(); }); t.chips = []; t.held = null; t.note = ''; t.active = null; onChange(); },
  };
  return t;
}

// ── The DOM part ──────────────────────────────────────────────────────────
function chatEl(tag, cls, text) {
  const el = document.createElement(tag);
  if (cls) el.className = cls;
  if (text != null) el.textContent = text;
  return el;
}

// Draw a tray into `box` (replaced whole). Every name goes through
// textContent or setAttribute.
function chatRenderTray(box, tray, flash) {
  box.replaceChildren();
  const say = (text, cls) => { if (text) { const p = chatEl('p', `cd-attach-note ${cls || ''}`, text); box.appendChild(p); } };
  say(flash, 'err');
  say(tray && tray.note, 'err');
  if (!tray) return;
  if (tray.held) {
    const held = chatEl('div', 'cd-attach-held');
    held.appendChild(chatEl('p', null, 'Check these before they are sent to the sandbox:'));
    const ul = chatEl('ul');
    tray.held.reasons.forEach(r => ul.appendChild(chatEl('li', null, r)));
    held.appendChild(ul);
    const label = chatEl('label', 'cd-attach-tick');
    const tick = chatEl('input');
    tick.type = 'checkbox';
    label.append(tick, chatEl('span', null, 'I have looked at these files and want to attach them'));
    const row = chatEl('div', 'cd-attach-row');
    const go = chatEl('button', null, 'Attach');
    go.type = 'button';
    // The tick lives on the tray: a redraw (a session event) must not lose it.
    tick.checked = !!tray.held.ticked;
    go.disabled = !tick.checked;
    tick.addEventListener('change', () => { tray.held.ticked = tick.checked; go.disabled = !tick.checked; });
    go.addEventListener('click', () => tray.confirmHeld(tick.checked));
    const no = chatEl('button', null, 'Cancel');
    no.type = 'button';
    no.addEventListener('click', () => tray.discardHeld());
    row.append(go, no);
    held.append(label, row);
    box.appendChild(held);
  }
  if (!tray.chips.length) return;
  const list = chatEl('ul', 'cd-chips');
  tray.chips.forEach(c => {
    const li = chatEl('li', `cd-chip ${c.state}`);
    li.appendChild(chatEl('span', 'cd-chip-icon', '📎'));
    const name = chatEl('span', 'cd-chip-name', chatShortName(c.name));
    name.setAttribute('title', chatShortName(c.name));
    const meta = chatEl('span', 'cd-chip-meta', c.state === 'failed' ? `Failed: ${c.error}`
      : c.state === 'done' ? `${chatSize(c.size)} · attached`
      : c.state === 'sending' ? `${chatSize(c.size)} · ${Math.round(c.pct * 100)}%` : `${chatSize(c.size)} · waiting`);
    const info = chatEl('span', 'cd-chip-info');
    info.append(name, meta);
    li.appendChild(info);
    if (c.state === 'sending' || c.state === 'queued') {
      const bar = chatEl('progress');
      bar.max = 100;
      bar.value = Math.round(c.pct * 100);
      bar.setAttribute('aria-label', 'Upload progress');
      li.appendChild(bar);
    }
    if (c.state === 'failed') {
      const r = chatEl('button', 'cd-chip-btn', 'Retry');
      r.type = 'button';
      r.addEventListener('click', () => tray.retry(c.id));
      li.appendChild(r);
    }
    const x = chatEl('button', 'cd-chip-btn', '✕');
    x.type = 'button';
    x.setAttribute('aria-label', `Remove ${chatShortName(c.name)}`);
    x.setAttribute('title', c.state === 'done' ? `Remove from the message; the file stays in the sandbox under ${CHAT_ATTACH_STAYS}` : 'Remove');
    x.addEventListener('click', () => tray.remove(c.id));
    li.appendChild(x);
    list.appendChild(li);
  });
  box.appendChild(list);
  if (tray.chips.some(c => c.state === 'done')) box.appendChild(chatEl('p', 'cd-attach-stays', `A removed file stays in the sandbox under ${CHAT_ATTACH_STAYS}.`));
}

// Wire a chat's composer: the chips and notes above its text field, an Attach
// button (`button`, made by the caller so it matches its surroundings), and a
// drop target over the whole `panel`. `current()` → { tray, sandbox, why }:
// the tray of the chat on screen, and when it has none (no sandbox known) the
// reason a drop is refused. Returns { render, say }.
function chatAttachMount({ form, panel, button, current, focus = () => {} }) {
  const box = chatEl('div', 'cd-attach-box');
  box.setAttribute('aria-live', 'polite');
  form.insertBefore(box, form.firstChild);
  const input = chatEl('input');
  input.type = 'file';
  input.multiple = true;
  input.hidden = true;
  button.after(input);
  let flash = '';
  const render = () => { chatRenderTray(box, current().tray, flash); box.hidden = !box.childNodes.length; };
  const take = (files, folders) => {
    const cur = current();
    flash = '';
    if (!cur.tray) { flash = cur.why || 'There is no sandbox to attach to yet.'; render(); return; }
    cur.tray.offer(files, folders);
    render();
  };
  button.addEventListener('click', () => input.click());
  input.addEventListener('change', () => { take([...input.files], []); input.value = ''; focus(); });

  const veil = chatEl('div', 'cd-drop');
  veil.setAttribute('aria-hidden', 'true');
  veil.hidden = true;
  panel.appendChild(veil);
  let depth = 0;
  const hide = () => { depth = 0; veil.hidden = true; };
  const hasFiles = e => [...(e.dataTransfer?.types || [])].includes('Files');
  panel.addEventListener('dragenter', e => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    depth++;
    const cur = current();
    veil.textContent = cur.tray ? `Drop to attach to ${cur.sandbox}` : `Cannot attach here: ${cur.why}`;
    veil.classList.toggle('refuse', !cur.tray);
    veil.hidden = false;
  });
  panel.addEventListener('dragover', e => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = current().tray ? 'copy' : 'none';
  });
  panel.addEventListener('dragleave', e => { if (hasFiles(e) && --depth <= 0) hide(); });
  panel.addEventListener('drop', e => {
    const got = chatDroppedFiles(e.dataTransfer);
    hide();
    if (!got) return; // text, a link, an image from a page: as before
    e.preventDefault();
    take(got.files, got.folders);
    focus();
  });
  return { render, say(msg) { flash = msg || ''; render(); } };
}

// A file dropped just outside a chat must not make the browser open it and
// leave the app. Once per page; a target that took the drop has already
// prevented it.
if (typeof window !== 'undefined' && !window.__chatDropGuard) {
  window.__chatDropGuard = true;
  const files = e => [...(e.dataTransfer?.types || [])].includes('Files');
  window.addEventListener('dragover', e => { if (files(e) && !e.defaultPrevented) { e.preventDefault(); e.dataTransfer.dropEffect = 'none'; } });
  window.addEventListener('drop', e => { if (files(e) && !e.defaultPrevented) e.preventDefault(); });
}
// END chat attach
