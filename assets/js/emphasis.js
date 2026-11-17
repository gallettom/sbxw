// ── Status pills ─────────────────────────────────────────────────────────
// An agent writes `[[Done]]` or `[[Failed: 3 tests]]`; each terminal draws the
// label as a pill, an xterm decoration laid over exactly the cells that hold
// it. Nothing in the buffer changes: copy, selection, scrollback, search and
// the PTY bytes keep the brackets.
//
// The vocabulary and the switches are the daemon's (`GET /api/emphasis`, the
// `emphasis` event on `/api/stream`); the matching rule is the one in
// docs/configuration.md, shared with the desktop app through
// crates/core/testdata/emphasis-cases.json (scripts/test-emphasis.mjs).
//
// Cost: no full-buffer scan. xterm's onRender says which viewport rows were
// drawn; those rows are marked dirty and rescanned at most every SCAN_MS. A
// row without "[[" costs one translateToString; a row with one is read cell
// by cell. A decoration already right is left alone, so a redraw that
// changes nothing adds and removes nothing (no flicker).
(function (root) {
  'use strict';

  // "[[" WORD [":" DETAIL] "]]": WORD 1 to 32 letters, digits, spaces or '-',
  // starting with a letter and ending with a letter or digit; DETAIL 1 to 48
  // characters, no bracket or control character.
  const MARKER = /\[\[(\p{L}(?:[\p{L}\p{N} -]{0,30}[\p{L}\p{N}])?)(?::([^\[\]\p{Cc}]{1,48}))?\]\]/gu;
  const UPPER = /^\p{Lu}/u;

  const key = w => w.split(' ').filter(Boolean).join(' ').toLowerCase();

  // The markers in one line of text: { index, end } in UTF-16 units of `text`,
  // the word and detail as written, and the tone. A known word in any case
  // takes its tone; an unknown one is a marker only when it starts with an
  // upper-case letter (so wiki links such as [[base-kit]] stay text).
  function find(text, contract) {
    const out = [];
    if (!text || text.indexOf('[[') < 0) return out;
    const words = (contract && contract.words) || {};
    const unknown = (contract && contract.unknown_tone) || 'blue';
    MARKER.lastIndex = 0;
    let m;
    while ((m = MARKER.exec(text))) {
      const word = m[1];
      const k = key(word);
      let tone = Object.prototype.hasOwnProperty.call(words, k) ? words[k] : null;
      if (!tone && UPPER.test(word)) tone = unknown;
      if (!tone) {
        // Not a marker: the next one may start inside it ("[[a [[Done]]").
        MARKER.lastIndex = m.index + 2;
        continue;
      }
      out.push({ index: m.index, end: m.index + m[0].length, word, detail: m[2] === undefined ? null : m[2], tone });
    }
    return out;
  }

  // `find` with positions counted in code points, as the shared cases are.
  function findCodePoints(text, contract) {
    const cp = i => Array.from(text.slice(0, i)).length;
    return find(text, contract).map(m => ({ at: cp(m.index), len: cp(m.end) - cp(m.index), word: m.word, detail: m.detail, tone: m.tone }));
  }

  // One logical line as cells: `rows` is a list of { y, cells: [{ x, w, ch }] }
  // (a wide character is one entry of w = 2; its trailing cell is left out).
  // Returns the pieces to draw, one per row a marker covers:
  // { y, x, width (cells), chars: [{ ch, w }], tone, left, right } where
  // left/right say whether the piece holds the marker's start/end.
  function pieces(rows, contract) {
    let text = '';
    const at = []; // UTF-16 index -> [row, cell] for the first unit of each cell
    rows.forEach((r, ri) => r.cells.forEach((c, ci) => { at[text.length] = [ri, ci]; text += c.ch; }));
    const out = [];
    for (const m of find(text, contract)) {
      let cur = null;
      for (let i = m.index; i < m.end; i++) {
        const pos = at[i];
        if (!pos) continue;
        const [ri, ci] = pos;
        const c = rows[ri].cells[ci];
        if (!cur || cur.row !== ri) {
          if (cur) out.push(cur);
          cur = { row: ri, y: rows[ri].y, x: c.x, width: 0, chars: [], tone: m.tone, left: i === m.index, right: false };
        }
        cur.chars.push({ ch: c.ch, w: c.w });
        cur.width = c.x + c.w - cur.x;
      }
      if (cur) { cur.right = true; out.push(cur); }
    }
    return out.map(({ row, ...p }) => p);
  }

  const api = { find, findCodePoints, pieces, key };

  // ── Browser half ───────────────────────────────────────────────────────
  if (typeof window === 'undefined') {
    if (typeof module !== 'undefined') module.exports = api;
    return;
  }

  const SCAN_MS = 60;
  const MAX_WRAP_ROWS = 8;     // a logical line longer than this is cut
  const MAX_DECOS = 600;       // per terminal, oldest dropped first
  let contract = null;
  const attached = new Set();

  function enabledFor(sandbox) {
    if (!contract) return false;
    if (sandbox && Object.prototype.hasOwnProperty.call(contract.sandboxes || {}, sandbox)) return !!contract.sandboxes[sandbox];
    return !!contract.enabled;
  }

  function setContract(c) {
    contract = c;
    for (const h of attached) h.refresh();
  }

  function load() {
    fetch('/api/emphasis').then(r => (r.ok ? r.json() : null)).then(c => { if (c) setContract(c); }).catch(() => {});
  }

  // Draw pills on `term`; `sandboxOf()` names the sandbox it shows now (it
  // changes when a pane is pointed elsewhere). Returns { refresh, dispose }.
  function attach(term, sandboxOf) {
    let decos = [];            // { marker, deco, key }
    const dirty = new Set();   // viewport rows
    let timer = 0;
    let all = false;
    const cell = () => term.buffer.active.getNullCell();

    const clear = () => { for (const d of decos) { d.deco?.dispose(); d.marker.dispose(); } decos = []; };
    const schedule = () => { if (!timer) timer = setTimeout(scan, SCAN_MS); };
    const markRows = (a, b) => { for (let r = a; r <= b; r++) dirty.add(r); schedule(); };
    const markAll = () => { all = true; schedule(); };

    function rowCells(line, cols, scratch) {
      const cells = [];
      for (let x = 0; x < cols; x++) {
        const c = line.getCell(x, scratch);
        if (!c) break;
        const w = c.getWidth();
        if (w === 0) continue;
        cells.push({ x, w, ch: c.getChars() || ' ' });
      }
      return cells;
    }

    function scan() {
      timer = 0;
      const buf = term.buffer.active;
      if (!enabledFor(sandboxOf())) { clear(); dirty.clear(); all = false; return; }
      const rows = all ? [...Array(term.rows).keys()] : [...dirty];
      dirty.clear(); all = false;
      const starts = new Set();
      for (const r of rows) {
        let y = buf.viewportY + r;
        if (y >= buf.length) continue;
        let n = 0;
        while (y > 0 && n++ < MAX_WRAP_ROWS && buf.getLine(y)?.isWrapped) y--;
        starts.add(y);
      }
      const scratch = cell();
      const live = decos.filter(d => d.marker.line >= 0 && !d.marker.isDisposed);
      if (live.length !== decos.length) {
        for (const d of decos) if (!live.includes(d)) d.deco?.dispose();
        decos = live;
      }
      for (const s of starts) {
        const ys = [s];
        while (ys.length < MAX_WRAP_ROWS && buf.getLine(ys[ys.length - 1] + 1)?.isWrapped) ys.push(ys[ys.length - 1] + 1);
        const lines = ys.map(y => buf.getLine(y));
        const has = lines.map(l => l?.translateToString(true) || '').join('').includes('[[');
        const want = new Map();
        if (has) {
          const rowsIn = ys.map((y, i) => ({ y, cells: rowCells(lines[i], term.cols, scratch) }));
          for (const p of pieces(rowsIn, contract)) {
            const k = `${p.y}|${p.x}|${p.width}|${p.tone}|${p.left ? 1 : 0}${p.right ? 1 : 0}|${p.chars.map(c => c.ch).join('')}`;
            want.set(k, p);
          }
        }
        const onRows = new Set(ys);
        const keep = [];
        for (const d of decos) {
          const line = d.marker.line;
          if (!onRows.has(line)) { keep.push(d); continue; }
          const k = line + d.key.slice(d.key.indexOf('|'));
          if (want.has(k)) { want.delete(k); keep.push(d); } else { d.deco?.dispose(); d.marker.dispose(); }
        }
        decos = keep;
        for (const [k, p] of want) add(p, k);
      }
      while (decos.length > MAX_DECOS) { const d = decos.shift(); d.deco?.dispose(); d.marker.dispose(); }
    }

    function add(p, k) {
      const buf = term.buffer.active;
      const marker = term.registerMarker(p.y - (buf.baseY + buf.cursorY));
      if (!marker) return;
      const deco = term.registerDecoration({ marker, x: p.x, width: p.width, layer: 'top' });
      if (!deco) { marker.dispose(); return; }
      deco.onRender(el => paint(el, p));
      decos.push({ marker, deco, key: k });
    }

    // The pill is opaque, so the canvas glyphs under it are hidden; it draws
    // the label itself, one span per cell (widths in % of the pill, so a
    // scaled font keeps every glyph on its cell). A bracket of the marker's
    // own start or end keeps its cell but draws nothing (.emph-br): a
    // wrapped marker's pieces each hide the brackets they hold.
    function paint(el, p) {
      el.style.fontSize = `${term.options.fontSize}px`;
      if (el.dataset.emph) return;
      el.dataset.emph = '1';
      el.className += ` emph-pill emph-${p.tone}${p.left ? ' emph-l' : ''}${p.right ? ' emph-r' : ''}`;
      el.style.fontFamily = term.options.fontFamily;
      const n = p.chars.length;
      p.chars.forEach((c, i) => {
        const s = document.createElement('span');
        s.textContent = c.ch;
        s.style.width = `${(c.w / p.width) * 100}%`;
        const bracket = (p.left && i < 2 && c.ch === '[') || (p.right && i >= n - 2 && c.ch === ']');
        if (bracket) s.className = 'emph-br';
        el.append(s);
      });
    }

    const subs = [
      term.onRender(e => markRows(e.start, e.end)),
      term.onResize(() => { clear(); markAll(); }),
      term.buffer.onBufferChange(() => { clear(); markAll(); }),
      // The pill hides the selection drawn under it: while there is one,
      // the pills step aside.
      term.onSelectionChange(() => term.element?.classList.toggle('emph-selecting', term.hasSelection())),
    ];
    const h = {
      refresh() { clear(); markAll(); },
      dispose() { clearTimeout(timer); subs.forEach(s => s.dispose()); clear(); attached.delete(h); },
    };
    attached.add(h);
    markAll();
    return h;
  }

  // Hear changes on the page's `/api/stream` (main.js calls this once it
  // has opened it); every (re)connect reads the contract again, since an
  // event sent while the stream was down is lost.
  function listen(stream) {
    stream.addEventListener('emphasis', ev => { try { setContract(JSON.parse(ev.data)); } catch (_) {} });
    stream.addEventListener('open', load);
  }

  root.sbxwEmphasis = { ...api, attach, setContract, enabledFor, load, listen, get contract() { return contract; } };
  load();
})(typeof window !== 'undefined' ? window : globalThis);
