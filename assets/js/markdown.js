// ── Markdown reader ───────────────────────────────────────────────────────
//
// Renders a Markdown file for the Project panel's Files preview (files.js):
// headings, paragraphs, emphasis, code, quotes, lists and task lists, tables,
// rules, links and images. Plain JavaScript, no dependency.
//
// A file is DATA an agent or a program wrote, shown in a page that holds the
// UI token. So:
//  - Every piece of the file reaches the HTML through `mdEsc`. Raw HTML in the
//    file is text, entities are text (`&lt;` reads as typed, never decoded).
//  - Only http and https links get an `href`, rebuilt by `URL`. A link to a
//    file or a heading gets an index into `links`, never an address: files.js
//    acts on it on a press. Any other scheme is not a link.
//  - An image is never given a source here. A relative image of this sandbox
//    gets an index into `images`; files.js decides what to load. Remote images
//    are text.
//  - Everything is linear: blocks are read line by line, inlines with the
//    CommonMark delimiter stack (linked lists, no splicing), and the regular
//    expressions are anchored and bounded. Nesting of quotes and lists stops
//    at MD_MAX_DEPTH; deeper content is shown as text.
//
// Nothing here runs at load: files.js calls `mdRender` when a preview opens.

const MD_MAX_DEPTH = 16;
const MD_SCAN_MAX = 2048;   // link destinations, titles, autolinks
const MD_LABEL_MAX = 999;
const MD_IMG_MAX = 24;      // relative images loaded per document
const MD_ESC_MAP = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };
const MD_PUNCT = new Set('!"#$%&\'()*+,-./:;<=>?@[\\]^_`{|}~');
const MD_IMG_EXT = /\.(png|jpe?g|gif|webp|svg|bmp|ico|avif)$/i;

function mdEsc(s) {
  return String(s).replace(/[&<>"']/g, (c) => MD_ESC_MAP[c]);
}

const mdIsSpace = (c) => c === ' ' || c === '\t' || c === '\n' || c === '\r' || c === '\f' || c === '\v';
const mdIsWs = (c) => c === undefined || c === '' || mdIsSpace(c) || /\s/.test(c);
const mdIsPunct = (c) => c !== undefined && c !== '' && (MD_PUNCT.has(c) || /[\p{P}\p{S}]/u.test(c));

function mdIndent(line) {
  let i = 0;
  while (i < line.length && line[i] === ' ') i++;
  return i;
}
const mdBlank = (line) => line.trim() === '';

// Leading tabs to spaces (tab stops of 4), so indentation is counted once.
function mdDetab(line) {
  if (line.indexOf('\t') === -1) return line;
  let col = 0, i = 0;
  for (; i < line.length; i++) {
    const c = line[i];
    if (c === ' ') col++;
    else if (c === '\t') col += 4 - (col % 4);
    else break;
  }
  return ' '.repeat(col) + line.slice(i);
}

// ── Block structure ──

function mdAtx(t) {
  let n = 0;
  while (n < t.length && n < 7 && t[n] === '#') n++;
  if (n < 1 || n > 6) return null;
  if (n < t.length && t[n] !== ' ' && t[n] !== '\t') return null;
  let s = t.slice(n).trim();
  // Closing #s, when separated from the text by a space.
  let j = s.length;
  while (j > 0 && s[j - 1] === '#') j--;
  if (j === 0) s = '';
  else if (j < s.length && (s[j - 1] === ' ' || s[j - 1] === '\t')) s = s.slice(0, j).trim();
  return { level: n, text: s };
}

function mdIsHr(t) {
  const c = t[0];
  if (c !== '*' && c !== '-' && c !== '_') return false;
  let n = 0;
  for (let i = 0; i < t.length; i++) {
    const d = t[i];
    if (d === c) n++;
    else if (d !== ' ' && d !== '\t') return false;
  }
  return n >= 3;
}

function mdFence(t) {
  const c = t[0];
  if (c !== '`' && c !== '~') return null;
  let n = 0;
  while (n < t.length && t[n] === c) n++;
  if (n < 3) return null;
  const info = t.slice(n).trim();
  if (c === '`' && info.includes('`')) return null;
  return { ch: c, len: n, lang: info.split(/\s/, 1)[0] || '' };
}

// `- x`, `* x`, `+ x`, `1. x`, `1) x`: the marker and where the content starts.
function mdListMarker(line) {
  const ind = mdIndent(line);
  if (ind > 3) return null;
  const t = line.slice(ind);
  let ordered = false, start = 1, mlen, delim;
  if (t[0] === '-' || t[0] === '*' || t[0] === '+') {
    mlen = 1; delim = t[0];
  } else {
    let d = 0;
    while (d < t.length && d < 10 && t[d] >= '0' && t[d] <= '9') d++;
    if (d < 1 || d > 9 || (t[d] !== '.' && t[d] !== ')')) return null;
    ordered = true; start = Number(t.slice(0, d)); mlen = d + 1; delim = t[d];
  }
  if (mlen < t.length && t[mlen] !== ' ' && t[mlen] !== '\t') return null;
  const after = t.slice(mlen);
  const rest = after.trimStart();
  let sp = after.length - rest.length;
  if (rest === '') sp = 1;
  else if (sp > 4) sp = 1;
  return { ordered, start, delim, content: ind + mlen + sp, rest: rest === '' ? '' : after.slice(sp), empty: rest === '' };
}

function mdSetext(line) {
  if (mdIndent(line) > 3) return 0;
  const t = line.trim();
  if (!t) return 0;
  const c = t[0];
  if (c !== '=' && c !== '-') return 0;
  for (let i = 0; i < t.length; i++) if (t[i] !== c) return 0;
  return c === '=' ? 1 : 2;
}

// A line that starts another block, so it ends a paragraph (and lazy text).
function mdStartsBlock(line) {
  const ind = mdIndent(line);
  if (ind > 3) return false;
  const t = line.slice(ind);
  if (t[0] === '>') return true;
  if (mdAtx(t) || mdFence(t) || mdIsHr(t)) return true;
  const m = mdListMarker(line);
  return !!(m && !m.empty && (!m.ordered || m.start === 1));
}

// Table cells: split on `|` that is not escaped.
function mdCells(line) {
  let t = line.trim();
  if (t[0] === '|') t = t.slice(1);
  if (t.endsWith('|') && !t.endsWith('\\|')) t = t.slice(0, -1);
  const out = [];
  let cur = '';
  for (let i = 0; i < t.length; i++) {
    const c = t[i];
    if (c === '\\' && t[i + 1] === '|') { cur += '|'; i++; } else if (c === '|') { out.push(cur.trim()); cur = ''; } else cur += c;
  }
  out.push(cur.trim());
  return out;
}

function mdDelimRow(line, n) {
  if (!line || mdIndent(line) > 3 || !line.includes('-')) return null;
  const cells = mdCells(line);
  if (cells.length !== n) return null;
  const align = [];
  for (const c of cells) {
    if (!/^:?-+:?$/.test(c)) return null;
    const l = c[0] === ':', r = c[c.length - 1] === ':';
    align.push(l && r ? 'center' : r ? 'right' : l ? 'left' : '');
  }
  return align;
}

function mdTableStart(line, next) {
  if (!line.includes('|') || mdIndent(line) > 3) return null;
  const head = mdCells(line);
  const align = mdDelimRow(next, head.length);
  return align ? { head, align } : null;
}

const mdLabel = (s) => s.trim().replace(/\s+/g, ' ').toLowerCase();

// `[label]: dest "title"`, read without a regular expression over the line.
function mdDef(line) {
  const t = line.trimStart();
  if (t[0] !== '[' || line.length - t.length > 3) return null;
  const close = t.indexOf(']');
  if (close < 2 || close > MD_LABEL_MAX || t[close + 1] !== ':' || t.slice(1, close).includes('[')) return null;
  const label = mdLabel(t.slice(1, close));
  if (!label) return null;
  let rest = t.slice(close + 2).trim();
  let dest;
  if (rest[0] === '<') {
    const e = rest.indexOf('>');
    if (e < 0) return null;
    dest = rest.slice(1, e); rest = rest.slice(e + 1).trim();
  } else {
    const sp = rest.search(/\s/);
    dest = sp < 0 ? rest : rest.slice(0, sp);
    rest = sp < 0 ? '' : rest.slice(sp).trim();
  }
  if (!dest) return null;
  let title = '';
  if (rest) {
    const o = rest[0], c = o === '(' ? ')' : o;
    if ((o !== '"' && o !== "'" && o !== '(') || rest.length < 2 || rest[rest.length - 1] !== c) return null;
    title = rest.slice(1, -1);
  }
  return { label, dest: mdUnescape(dest), title: mdUnescape(title) };
}

function mdUnescape(s) {
  if (s.indexOf('\\') === -1) return s;
  let out = '';
  for (let i = 0; i < s.length; i++) {
    if (s[i] === '\\' && MD_PUNCT.has(s[i + 1])) { out += s[i + 1]; i++; } else out += s[i];
  }
  return out;
}

// Lines of one container (the document, a quote, a list item) into blocks.
function mdBlocks(lines, depth, ctx) {
  const out = [];
  if (depth > MD_MAX_DEPTH) {
    const text = lines.join('\n').trim();
    if (text) out.push({ t: 'p', text });
    return out;
  }
  const n = lines.length;
  let i = 0;
  while (i < n) {
    const line = lines[i];
    if (mdBlank(line)) { i++; continue; }
    const ind = mdIndent(line);
    if (ind >= 4) {
      const code = [];
      while (i < n && (mdBlank(lines[i]) || mdIndent(lines[i]) >= 4)) { code.push(lines[i].slice(Math.min(4, mdIndent(lines[i])))); i++; }
      while (code.length && mdBlank(code[code.length - 1])) code.pop();
      out.push({ t: 'code', lang: '', text: code.join('\n') });
      continue;
    }
    const t = line.slice(ind);
    const fence = mdFence(t);
    if (fence) {
      const code = [];
      i++;
      while (i < n) {
        const l = lines[i];
        const li = mdIndent(l);
        if (li < 4) {
          const lt = l.slice(li);
          let k = 0;
          while (k < lt.length && lt[k] === fence.ch) k++;
          if (k >= fence.len && lt.slice(k).trim() === '') { i++; break; }
        }
        code.push(l.slice(Math.min(ind, li)));
        i++;
      }
      out.push({ t: 'code', lang: fence.lang, text: code.join('\n') });
      continue;
    }
    const atx = mdAtx(t);
    if (atx) { out.push({ t: 'h', level: atx.level, text: atx.text }); i++; continue; }
    if (mdIsHr(t)) { out.push({ t: 'hr' }); i++; continue; }
    if (t[0] === '>') {
      const inner = [];
      let prevText = false;
      while (i < n) {
        const l = lines[i];
        const li = mdIndent(l);
        if (li < 4 && l[li] === '>') {
          let s = l.slice(li + 1);
          if (s[0] === ' ') s = s.slice(1);
          inner.push(s);
          prevText = !mdBlank(s);
          i++;
        } else if (prevText && !mdBlank(l) && !mdStartsBlock(l)) {
          inner.push(l); i++;           // lazy continuation of the quote's paragraph
        } else break;
      }
      out.push({ t: 'quote', children: mdBlocks(inner, depth + 1, ctx) });
      continue;
    }
    const marker = mdListMarker(line);
    if (marker) { i = mdList(lines, i, marker, depth, ctx, out); continue; }
    const table = i + 1 < n ? mdTableStart(line, lines[i + 1]) : null;
    if (table) {
      i += 2;
      const rows = [];
      while (i < n && !mdBlank(lines[i]) && !mdStartsBlock(lines[i])) {
        const cells = mdCells(lines[i]);
        const row = [];
        for (let k = 0; k < table.head.length; k++) row.push(cells[k] ?? '');
        rows.push(row);
        i++;
      }
      out.push({ t: 'table', head: table.head, align: table.align, rows });
      continue;
    }
    // A paragraph, until a blank line or another block; a setext underline
    // makes it a heading.
    const para = [line];
    i++;
    let setext = 0;
    while (i < n) {
      const l = lines[i];
      if (mdBlank(l)) break;
      setext = mdSetext(l);
      if (setext) { i++; break; }
      if (mdStartsBlock(l)) break;
      if (i + 1 < n && mdTableStart(l, lines[i + 1])) break;
      para.push(l);
      i++;
    }
    // Reference definitions at the head of a paragraph are not shown.
    while (para.length) {
      const d = mdDef(para[0]);
      if (!d) break;
      if (!ctx.refs.has(d.label)) ctx.refs.set(d.label, d);
      para.shift();
    }
    if (!para.length) continue;
    const text = para.map((l) => l.trimStart()).join('\n');
    if (setext) out.push({ t: 'h', level: setext, text: text.trim() });
    else out.push({ t: 'p', text });
  }
  return out;
}

function mdList(lines, i, first, depth, ctx, out) {
  const n = lines.length;
  const list = { t: 'list', ordered: first.ordered, start: first.start, loose: false, items: [] };
  let gap = false;
  while (i < n) {
    const m = mdListMarker(lines[i]);
    if (!m || m.ordered !== first.ordered || m.delim !== first.delim) break;
    if (gap) list.loose = true;
    const item = [m.rest];
    i++;
    let prevText = !m.empty;
    while (i < n) {
      const l = lines[i];
      if (mdBlank(l)) { item.push(''); prevText = false; i++; continue; }
      if (mdIndent(l) >= m.content) { item.push(l.slice(m.content)); prevText = true; i++; continue; }
      if (prevText && !mdStartsBlock(l) && !mdListMarker(l)) { item.push(l); i++; continue; }
      break;
    }
    gap = false;
    while (item.length > 1 && mdBlank(item[item.length - 1])) { item.pop(); gap = true; }
    // A blank line between two blocks of one item makes the list loose.
    for (let k = 1; k < item.length - 1; k++) if (mdBlank(item[k])) { list.loose = true; break; }
    let task = null;
    const h = item[0];
    if ((h.startsWith('[ ]') || h.startsWith('[x]') || h.startsWith('[X]')) && (h.length === 3 || h[3] === ' ')) {
      task = h[1] !== ' ';
      item[0] = h.slice(4);
    }
    list.items.push({ task, children: mdBlocks(item, depth + 1, ctx) });
  }
  out.push(list);
  return i;
}

// ── Inlines ──
//
// Nodes form a doubly linked list; emphasis and links take a run of nodes
// as their children in O(1). Delimiters (`*`, `_`, `~`) are text nodes with a
// second link chain, the delimiter stack.

function mdNode(t, s) { return { t, s, prev: null, next: null }; }

function mdInline(src, ctx) {
  const head = mdNode('root', '');
  let tail = head;
  const append = (node) => { node.prev = tail; tail.next = node; tail = node; return node; };
  let buf = '';
  const flush = () => { if (buf) { append(mdNode('text', buf)); buf = ''; } };

  // Delimiter stack.
  let dtail = null;
  const pushDelim = (node) => { node.dprev = dtail; node.dnext = null; if (dtail) dtail.dnext = node; dtail = node; };
  const removeDelim = (d) => {
    if (d.dprev) d.dprev.dnext = d.dnext;
    if (d.dnext) d.dnext.dprev = d.dprev; else dtail = d.dprev;
    d.dprev = d.dnext = null; d.delim = false;
  };
  const removeNode = (node) => {
    if (node.prev) node.prev.next = node.next;
    if (node.next) node.next.prev = node.prev; else tail = node.prev;
  };

  const brackets = [];
  let barrier = 0;               // brackets under this index can no longer make links
  const L = src.length;
  const lastParen = src.lastIndexOf(')');
  let gtAt = 0;                  // the next `>`, found once per stretch (-1: none left)

  // Backtick runs, by length, for code spans: one pointer per length.
  const runs = new Map();
  for (let k = 0; k < L;) {
    if (src[k] !== '`') { k++; continue; }
    let e = k;
    while (e < L && src[e] === '`') e++;
    const len = e - k;
    if (!runs.has(len)) runs.set(len, { pos: [], at: 0 });
    runs.get(len).pos.push(k);
    k = e;
  }
  const closingRun = (len, after) => {
    const r = runs.get(len);
    if (!r) return -1;
    while (r.at < r.pos.length && r.pos[r.at] < after) r.at++;
    return r.at < r.pos.length ? r.pos[r.at] : -1;
  };

  function processEmphasis(bottom) {
    const floors = {};
    let closer = bottom ? bottom.dnext : null;
    if (!bottom) { let d = dtail; while (d && d.dprev) d = d.dprev; closer = d; }
    while (closer) {
      if (!closer.close) { closer = closer.dnext; continue; }
      const key = closer.ch === '~' ? '~' + closer.n : closer.ch;
      let opener = closer.dprev, found = null;
      while (opener && opener !== bottom && opener !== floors[key]) {
        if (opener.ch === closer.ch && opener.open && (closer.ch !== '~' || opener.n === closer.n)) { found = opener; break; }
        opener = opener.dprev;
      }
      if (!found) {
        floors[key] = closer.dprev;
        const nx = closer.dnext;
        if (!closer.open) removeDelim(closer);
        closer = nx;
        continue;
      }
      const use = closer.ch === '~' ? closer.n : (closer.n >= 2 && found.n >= 2 ? 2 : 1);
      const tag = closer.ch === '~' ? 'del' : use === 2 ? 'strong' : 'em';
      found.n -= use; closer.n -= use;
      found.s = found.ch.repeat(found.n); closer.s = closer.ch.repeat(closer.n);
      const wrap = mdNode(tag, '');
      if (found.next !== closer) {
        wrap.first = found.next; wrap.last = closer.prev;
        wrap.first.prev = null; wrap.last.next = null;
      }
      wrap.prev = found; wrap.next = closer; found.next = wrap; closer.prev = wrap;
      // Delimiters between the two are now plain text.
      found.dnext = closer; closer.dprev = found;
      if (found.n === 0) { removeNode(found); removeDelim(found); }
      if (closer.n === 0) { const nx = closer.dnext; removeNode(closer); removeDelim(closer); closer = nx; }
    }
    // Whatever is left above the bottom stays text.
    let d = bottom ? bottom.dnext : null;
    if (!bottom) { d = dtail; while (d && d.dprev) d = d.dprev; }
    while (d) { const nx = d.dnext; removeDelim(d); d = nx; }
    if (bottom) dtail = bottom;
  }

  // `(dest "title")` at p, or null. Scans are capped and fail fast.
  function inlineDest(p) {
    if (src[p] !== '(' || lastParen < p) return null;
    let q = p + 1;
    const end = Math.min(L, p + MD_SCAN_MAX);
    while (q < end && mdIsSpace(src[q])) q++;
    let dest = '';
    if (src[q] === '<') {
      const s = q + 1;
      while (q < end && src[q] !== '>' && src[q] !== '\n') q++;
      if (src[q] !== '>') return null;
      dest = src.slice(s, q); q++;
    } else {
      const s = q;
      let depth = 0;
      while (q < end) {
        const c = src[q];
        if (c === '\\' && MD_PUNCT.has(src[q + 1])) { q += 2; continue; }
        if (c === '(') { if (++depth > 32) return null; } else if (c === ')') { if (depth === 0) break; depth--; } else if (mdIsSpace(c) || c < ' ') break;
        q++;
      }
      if (q >= end || depth) return null;
      dest = src.slice(s, q);
    }
    const ws = q;
    while (q < end && mdIsSpace(src[q])) q++;
    let title = '';
    if (q > ws && (src[q] === '"' || src[q] === "'" || src[q] === '(')) {
      const c = src[q] === '(' ? ')' : src[q];
      const s = ++q;
      while (q < end && src[q] !== c) { if (src[q] === '\\') q++; q++; }
      if (q >= end) return null;
      title = src.slice(s, q); q++;
      while (q < end && mdIsSpace(src[q])) q++;
    }
    if (src[q] !== ')') return null;
    return { dest: mdUnescape(dest), title: mdUnescape(title), end: q + 1 };
  }

  let i = 0;
  while (i < L) {
    const c = src[i];
    if (c === '\\') {
      if (src[i + 1] === '\n') { flush(); append(mdNode('br', '')); i += 2; while (src[i] === ' ') i++; continue; }
      if (MD_PUNCT.has(src[i + 1])) { buf += src[i + 1]; i += 2; continue; }
      buf += c; i++; continue;
    }
    if (c === '`') {
      let e = i;
      while (e < L && src[e] === '`') e++;
      const len = e - i;
      const close = closingRun(len, e);
      if (close < 0) { buf += src.slice(i, e); i = e; continue; }
      let code = src.slice(e, close).replace(/\n/g, ' ');
      if (code.length > 2 && code[0] === ' ' && code[code.length - 1] === ' ' && code.trim()) code = code.slice(1, -1);
      flush(); append(mdNode('code', code));
      i = close + len;
      continue;
    }
    if (c === '*' || c === '_' || c === '~') {
      let e = i;
      while (e < L && src[e] === c) e++;
      const n = e - i;
      const before = i > 0 ? src[i - 1] : '';
      const after = e < L ? src[e] : '';
      const left = !mdIsWs(after) && (!mdIsPunct(after) || mdIsWs(before) || mdIsPunct(before));
      const right = !mdIsWs(before) && (!mdIsPunct(before) || mdIsWs(after) || mdIsPunct(after));
      let open = left, close = right;
      if (c === '_') { open = left && (!right || mdIsPunct(before)); close = right && (!left || mdIsPunct(after)); }
      flush();
      const node = append(mdNode('text', src.slice(i, e)));
      if ((open || close) && (c !== '~' || n <= 2)) {
        Object.assign(node, { delim: true, ch: c, n, open, close });
        pushDelim(node);
      }
      i = e;
      continue;
    }
    if (c === '[' || (c === '!' && src[i + 1] === '[')) {
      flush();
      const image = c === '!';
      const node = append(mdNode('text', image ? '![' : '['));
      brackets.push({ node, image, pos: i + (image ? 2 : 1), dtail });
      i += image ? 2 : 1;
      continue;
    }
    if (c === ']') {
      const op = brackets.pop();
      const inactive = op && !op.image && brackets.length < barrier;
      if (brackets.length < barrier) barrier = brackets.length;
      if (!op || inactive) { buf += ']'; i++; continue; }
      let target = null, end = i + 1;
      const d = inlineDest(i + 1);
      if (d) { target = d; end = d.end; } else {
        let label = null;
        if (src[i + 1] === '[') {
          const e = src.indexOf(']', i + 2);
          if (e > 0 && e - i <= MD_LABEL_MAX + 2 && !src.slice(i + 2, e).includes('[')) {
            label = e === i + 2 ? src.slice(op.pos, i) : src.slice(i + 2, e);
            end = e + 1;
          }
        }
        if (label === null) label = i - op.pos <= MD_LABEL_MAX ? src.slice(op.pos, i) : '';
        const ref = label ? ctx.refs.get(mdLabel(label)) : null;
        if (ref) target = ref; else end = i + 1;
      }
      if (!target) { buf += ']'; i++; continue; }
      flush();
      processEmphasis(op.dtail);
      const node = mdNode(op.image ? 'img' : 'link', '');
      node.dest = target.dest; node.title = target.title || '';
      if (op.node.next) { node.first = op.node.next; node.last = tail; node.first.prev = null; }
      tail = op.node.prev; tail.next = null;
      append(node);
      if (!op.image) barrier = brackets.length;
      i = end;
      continue;
    }
    if (c === '<') {
      if (gtAt !== -1 && gtAt <= i) gtAt = src.indexOf('>', i + 1);
      const close = gtAt;
      if (close > 0 && close - i <= MD_SCAN_MAX) {
        const inner = src.slice(i + 1, close);
        if (/^https?:\/\/[^\s<]+$/i.test(inner)) {
          flush();
          const node = mdNode('link', '');
          node.dest = inner; node.title = '';
          node.first = node.last = mdNode('text', inner);
          append(node);
          i = close + 1;
          continue;
        }
      }
      buf += c; i++; continue;
    }
    if (c === 'h' && (i === 0 || /[\s(*_~]/.test(src[i - 1])) && (src.startsWith('https://', i) || src.startsWith('http://', i))) {
      let e = i;
      const end = Math.min(L, i + MD_SCAN_MAX);
      while (e < end && !mdIsSpace(src[e]) && src[e] !== '<') e++;
      // Trailing punctuation and an unbalanced `)` are not part of it.
      let opens = 0, closes = 0;
      for (let k = i; k < e; k++) { if (src[k] === '(') opens++; else if (src[k] === ')') closes++; }
      while (e > i) {
        const t = src[e - 1];
        if ('?!.,:*_~\'"'.includes(t)) { e--; continue; }
        if (t === ')' && opens < closes) { closes--; e--; continue; }
        break;
      }
      const url = src.slice(i, e);
      if (url.length > 8) {
        flush();
        const node = mdNode('link', '');
        node.dest = url; node.title = '';
        node.first = node.last = mdNode('text', url);
        append(node);
        i = e;
        continue;
      }
    }
    if (c === '\n') {
      // Two spaces before a line end are a hard break; otherwise a soft one.
      let k = buf.length;
      while (k > 0 && buf[k - 1] === ' ') k--;
      const hard = buf.length - k >= 2;
      buf = buf.slice(0, k);
      flush();
      append(hard ? mdNode('br', '') : mdNode('text', '\n'));
      i++;
      while (src[i] === ' ') i++;
      continue;
    }
    buf += c;
    i++;
  }
  flush();
  processEmphasis(null);
  return mdInlineHtml(head.next, ctx);
}

// The nodes as HTML and as plain text, walked with an explicit stack (nesting
// is as deep as the source makes it).
function mdInlineHtml(first, ctx) {
  let html = '', text = '';
  const stack = [];
  let n = first;
  for (;;) {
    if (!n) {
      if (!stack.length) break;
      const f = stack.pop();
      html += f.close;
      n = f.next;
      continue;
    }
    switch (n.t) {
      case 'text': html += mdEsc(n.s); text += n.s; break;
      case 'code': html += `<code>${mdEsc(n.s)}</code>`; text += n.s; break;
      case 'br': html += '<br>'; text += ' '; break;
      case 'em': case 'strong': case 'del':
        html += `<${n.t}>`;
        stack.push({ close: `</${n.t}>`, next: n.next });
        n = n.first;
        continue;
      case 'img': {
        const alt = mdPlain(n.first);
        html += mdImage(n, alt, ctx);
        text += alt;
        break;
      }
      case 'link': {
        const tg = mdTarget(n.dest, ctx.base);
        const idx = tg ? ctx.links.push(tg) - 1 : -1;
        if (!tg) {
          // Not a link we follow: the text, and where it pointed, as text.
          html += '<span class="md-badlink">';
          stack.push({ close: `</span><span class="md-url"> (${mdEsc(n.dest)})</span>`, next: n.next });
        } else if (tg.kind === 'web') {
          const where = n.title ? `${n.title} (${tg.href})` : tg.href;
          html += `<a class="md-link md-ext" href="${mdEsc(tg.href)}" target="_blank" rel="noopener noreferrer" title="${mdEsc(where)}">`;
          stack.push({ close: '<span class="md-ext-mark" aria-hidden="true">↗</span></a>', next: n.next });
        } else {
          const what = tg.kind === 'anchor' ? `Go to #${tg.anchor}` : `Open ${tg.path}${tg.anchor ? '#' + tg.anchor : ''}`;
          html += `<a class="md-link md-${tg.kind}" data-md-link="${idx}" role="link" tabindex="0" title="${mdEsc(n.title ? `${n.title} (${what})` : what)}">`;
          stack.push({ close: '</a>', next: n.next });
        }
        n = n.first || null;
        if (!n) { const f = stack.pop(); html += f.close; n = f.next; }
        continue;
      }
      default: break;
    }
    n = n.next;
  }
  return { html, text };
}

function mdPlain(first) {
  let text = '';
  const stack = [];
  let n = first;
  for (;;) {
    if (!n) { if (!stack.length) break; n = stack.pop(); continue; }
    if (n.t === 'text' || n.t === 'code') text += n.s;
    else if (n.t === 'br') text += ' ';
    if (n.first) { stack.push(n.next); n = n.first; continue; }
    n = n.next;
  }
  return text;
}

function mdImage(n, alt, ctx) {
  const tg = mdTarget(n.dest, ctx.base);
  if (tg && tg.kind === 'file' && MD_IMG_EXT.test(tg.path) && ctx.images.length < MD_IMG_MAX) {
    const idx = ctx.images.push(tg.path) - 1;
    return `<img class="md-img" data-md-img="${idx}" alt="${mdEsc(alt)}" title="${mdEsc(n.title || tg.path)}">`;
  }
  return `<span class="md-img-alt" title="Image not loaded">[image: ${mdEsc(alt || 'no description')}] <span class="md-url">(${mdEsc(n.dest)})</span></span>`;
}

// What a link's destination is, or null when it must stay text.
//  - { kind: 'web', href }: http or https, rebuilt by URL.
//  - { kind: 'anchor', anchor }: a heading of this document.
//  - { kind: 'file', path, anchor, dir }: a file of this sandbox, resolved
//    against `base` (the document's folder), never above `/`.
function mdTarget(dest, base) {
  if (typeof dest !== 'string') return null;
  const d = dest.trim();
  if (!d || d.length > MD_SCAN_MAX || /[\u0000-\u001f\u007f]/.test(d)) return null;
  if (d[0] === '#') {
    const anchor = mdDecode(d.slice(1));
    return anchor ? { kind: 'anchor', anchor } : null;
  }
  const scheme = /^([A-Za-z][A-Za-z0-9+.-]{0,31}):/.exec(d);
  if (scheme) {
    const s = scheme[1].toLowerCase();
    if (s !== 'http' && s !== 'https') return null;
    try {
      const u = new URL(d);
      if (u.protocol !== 'http:' && u.protocol !== 'https:') return null;
      return { kind: 'web', href: u.href };
    } catch (_) { return null; }
  }
  // Protocol-relative, Windows drive or a colon before any slash: not a path.
  if (d.startsWith('//') || d.startsWith('\\') || /^[^/]*:/.test(d.split('#')[0])) return null;
  if (typeof base !== 'string' || !base.startsWith('/')) return null;
  const hash = d.indexOf('#');
  let p = hash < 0 ? d : d.slice(0, hash);
  const anchor = hash < 0 ? '' : mdDecode(d.slice(hash + 1));
  const q = p.indexOf('?');
  if (q >= 0) p = p.slice(0, q);
  p = mdDecode(p);
  if (!p) return null;
  const dir = p.endsWith('/');
  const parts = [];
  const all = (p.startsWith('/') ? p : base + '/' + p).split('/');
  for (const seg of all) {
    if (seg === '' || seg === '.') continue;
    if (seg === '..') { if (!parts.length) return null; parts.pop(); continue; }
    parts.push(seg);
  }
  return { kind: 'file', path: '/' + parts.join('/'), anchor, dir };
}

function mdDecode(s) {
  try { return decodeURIComponent(s); } catch (_) { return s; }
}

// GitHub's heading ids: lower case, punctuation dropped, spaces to dashes.
function mdSlug(text) {
  const s = String(text).trim().toLowerCase().replace(/[^\p{L}\p{N}\p{M}\s_-]/gu, '').replace(/\s/g, '-');
  return s || 'section';
}

// ── Rendering ──

function mdRenderBlocks(blocks, ctx, tight) {
  let html = '';
  for (const b of blocks) {
    switch (b.t) {
      case 'h': {
        const r = mdInline(b.text, ctx);
        const base = mdSlug(r.text);
        let slug = base;
        for (let k = 1; ctx.slugs.has(slug); k++) slug = `${base}-${k}`;
        ctx.slugs.add(slug);
        ctx.headings.push({ level: b.level, text: r.text, id: `md-${slug}`, slug });
        html += `<h${b.level} id="md-${mdEsc(slug)}" class="md-h">${r.html}</h${b.level}>`;
        break;
      }
      case 'p': {
        const r = mdInline(b.text, ctx);
        html += tight ? r.html : `<p>${r.html}</p>`;
        break;
      }
      case 'code':
        ctx.codeBlocks++;
        html += `<div class="md-code">${b.lang ? `<div class="md-code-lang">${mdEsc(b.lang)}</div>` : ''}<pre><code>${mdEsc(b.text)}</code></pre></div>`;
        break;
      case 'hr': html += '<hr>'; break;
      case 'quote': html += `<blockquote>${mdRenderBlocks(b.children, ctx, false)}</blockquote>`; break;
      case 'list': {
        const tag = b.ordered ? 'ol' : 'ul';
        const start = b.ordered && b.start !== 1 ? ` start="${Number(b.start) || 1}"` : '';
        html += `<${tag}${start}>`;
        for (const it of b.items) {
          const box = it.task === null ? '' : `<input type="checkbox" disabled${it.task ? ' checked' : ''}> `;
          html += `<li${it.task === null ? '' : ' class="md-task"'}>${box}${mdRenderBlocks(it.children, ctx, !b.loose)}</li>`;
        }
        html += `</${tag}>`;
        break;
      }
      case 'table': {
        ctx.tables++;
        const al = (k) => (b.align[k] ? ` class="md-al-${b.align[k]}"` : '');
        html += '<div class="md-table-wrap"><table><thead><tr>';
        b.head.forEach((c, k) => { html += `<th${al(k)}>${mdInline(c, ctx).html}</th>`; });
        html += '</tr></thead>';
        if (b.rows.length) {
          html += '<tbody>';
          for (const row of b.rows) {
            html += '<tr>';
            row.forEach((c, k) => { html += `<td${al(k)}>${mdInline(c, ctx).html}</td>`; });
            html += '</tr>';
          }
          html += '</tbody>';
        }
        html += '</table></div>';
        break;
      }
      default: break;
    }
  }
  return html;
}

/// `src` as HTML (built from escaped text only), with what files.js needs to
/// make it work: the headings for the outline, and the links and images by
/// index. `opts.base` is the document's folder, for relative links; without
/// it, links to files stay text.
function mdRender(src, opts = {}) {
  const text = String(src ?? '').replace(/\r\n?/g, '\n').replace(/\u0000/g, '�');
  const lines = text.split('\n').map(mdDetab);
  const ctx = {
    base: opts.base || null, refs: new Map(), slugs: new Set(),
    headings: [], links: [], images: [], tables: 0, codeBlocks: 0,
  };
  const blocks = mdBlocks(lines, 0, ctx);
  const html = mdRenderBlocks(blocks, ctx, false);
  return { html, headings: ctx.headings, links: ctx.links, images: ctx.images, tables: ctx.tables, codeBlocks: ctx.codeBlocks };
}

/// The outline: one button per heading, indented by level.
function mdOutlineHtml(headings) {
  const top = headings.reduce((m, h) => Math.min(m, h.level), 6);
  return headings.map((h) => `<button type="button" class="md-ol-item md-ol-l${Math.min(4, h.level - top + 1)}" data-md-goto="${mdEsc(h.id)}" title="${mdEsc(h.text)}">${mdEsc(h.text || '(untitled)')}</button>`).join('');
}

/// The heading an anchor names (`#the-chat-dock`, as written in a link), or null.
function mdFindHeading(headings, anchor) {
  if (!anchor) return null;
  const a = String(anchor).toLowerCase();
  return headings.find((h) => h.slug === a) || headings.find((h) => h.slug === mdSlug(anchor)) || null;
}
