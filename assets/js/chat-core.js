// ── Chat core ─────────────────────────────────────────────────────────────
// What the chat dock (chat-dock.js) and the browser side panel (sidepanel.js)
// have in common: how a `session` event moves a conversation along, and the
// pieces of a transcript — a message, the "working" line, a pending question —
// built the same way in both, on the same `cd-*` styles.
//
// A chat here is `{ log, state, steps, answerable, activity }`: the tab's own
// transcript (the daemon keeps only the last reply per session) plus what the
// latest event said about the agent. Nothing in this file knows which page it
// is on; each caller decides what to re-render and whom to tell.

// Is this event about a session sbxw drives? `/api/chat/push` types into
// sbxw's own terminal, so a session an editor, Claude Desktop or SSH started
// in the same container (`origin: 'remote'`) is not the conversation shown —
// and its events, re-emitted alongside every sibling's (see `emit_info`),
// would otherwise overwrite this one's state with theirs.
function chatOwnSession(info) {
  return info.origin !== 'remote';
}

function chatNew() {
  return { log: [], state: 'idle', steps: null, answerable: false, activity: null };
}

// Fold a `session` event into `c`. Returns what the reader may want to hear
// about — `{ reply }` when a turn just ended with an answer, `{ asked }` when a
// question just appeared — or null.
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
    return null;
  }
  if (info.state === 'idle' && (prev === 'working' || prev === 'attention') && info.reply) {
    const last = c.log[c.log.length - 1];
    if (!(last?.who === 'ai' && last.text === info.reply)) c.log.push({ who: 'ai', text: info.reply });
    return { reply: info.reply };
  }
  if (info.state === 'attention' && prev !== 'attention') {
    return { asked: c.steps?.[0]?.text || info.activity || 'Claude is waiting for you.' };
  }
  return null;
}

// Claude answers in Markdown. Just enough of it to read well in a narrow
// column — paragraphs, lists, code blocks, inline code, bold, italics, links,
// headings — and nothing that could carry markup of its own: the text is
// escaped first, and only these fixed tags are added to it.
function chatEscape(s) {
  return s.replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

function chatInline(s) {
  return s.split(/(`[^`\n]+`)/).map((part, i) => {
    if (i % 2) return `<code>${chatEscape(part.slice(1, -1))}</code>`;
    return chatEscape(part)
      .replace(/\*\*([^*\n]+)\*\*/g, '<strong>$1</strong>')
      .replace(/(^|[^*\w])\*([^*\s][^*\n]*?)\*(?![\w*])/g, '$1<em>$2</em>')
      .replace(/\[([^\]\n]+)\]\((https?:\/\/[^\s)]+)\)/g, '<a href="$2" target="_blank" rel="noopener noreferrer">$1</a>');
  }).join('');
}

function chatMarkdown(text) {
  return text.split(/```[^\n]*\n([\s\S]*?)```/).map((part, i) => {
    if (i % 2) return `<pre><code>${chatEscape(part.replace(/\n$/, ''))}</code></pre>`;
    return part.split(/\n\s*\n/).map(chatBlock).join('');
  }).join('');
}

// One blank-line-separated block: a run of `- ` or `1. ` lines is a list, a
// `#` line a heading, anything else a paragraph that keeps its line breaks.
// Lists are real lists so a long item wraps under its own text, not under
// the bullet.
function chatBlock(block) {
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
      html += `<p><strong>${chatInline(heading[1])}</strong></p>`;
    } else if (bullet || numbered) {
      flushPara();
      const tag = bullet ? 'ul' : 'ol';
      if (!list || list.tag !== tag) {
        flushList();
        list = { tag, start: numbered ? Number(numbered[1]) : 1, items: [] };
      }
      list.items.push(chatInline(bullet ? bullet[1] : numbered[2]));
    } else if (list && /^\s{2,}\S/.test(line)) {
      // An indented line under an item carries on that item.
      list.items[list.items.length - 1] += ' ' + chatInline(line.trim());
    } else {
      flushList();
      para.push(chatInline(line));
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
// fills and drains like a running job — unless the reader asked for less
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

function chatLine(cls, text, meta) {
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
    body.innerHTML = chatMarkdown(text);
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
// to 120px. `onSend` gets the trimmed text; the box is already cleared.
function chatComposer(form, input, onSend) {
  form.addEventListener('submit', e => {
    e.preventDefault();
    const text = input.value.trim();
    if (!text) return;
    input.value = '';
    input.style.height = '';
    onSend(text);
  });
  input.addEventListener('keydown', e => {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) { e.preventDefault(); form.requestSubmit(); }
  });
  input.addEventListener('input', () => {
    input.style.height = 'auto';
    input.style.height = Math.min(input.scrollHeight, 120) + 'px';
  });
}
