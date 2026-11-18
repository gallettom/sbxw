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
// Cost: no full-buffer scan; see "Pills on a terminal" for when rows are
// read. A row without "[[" costs one translateToString; a row with one is
// read cell by cell. A decoration already right is left alone, so a redraw
// that changes nothing adds and removes nothing (no flicker).
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

  // ── Pills on a terminal ────────────────────────────────────────────────
  // Each pill is a decoration on a MARKER of its buffer line: xterm moves it
  // with the line, in the frame that draws the text (its decoration renderer
  // runs on every viewport render, scroll included), and keeps it while the
  // line is in the scrollback. So a pill is made once per marker, and lines
  // are read only when they may hold something new:
  //
  // * on a render that moved the viewport (a scroll, or output that scrolls),
  //   the rows it drew are read AT ONCE, inside that render: a decoration
  //   registered there is created before the frame ends, so a row brought
  //   into view never shows its raw "[[Done]]" first. Lines read before still
  //   have their pills (on their markers), and reading them again changes
  //   nothing.
  // * on a render in place (output, a TUI repainting its rows) of at most
  //   SYNC_ROWS rows, the same: a pill whose text was repainted away goes in
  //   that frame, not after a timer.
  // * a larger render in place (a burst of output repainting the screen) is
  //   read once the burst settles, at most every SCAN_MS.
  //
  // Cost: never more than the rows of one render per frame (one
  // translateToString per row; cells read only on a row holding "[["),
  // whatever the scrollback: 5,000 lines of it are not read when scrolled
  // through, only the rows each frame shows. Pills are capped at MAX_DECOS
  // per terminal; over it, those farthest from the viewport go first, so the
  // lines around what is being read keep theirs. A line trimmed off the
  // scrollback takes its marker, and the pill with it.
  const SCAN_MS = 60;
  const SYNC_ROWS = 8;
  const MAX_WRAP_ROWS = 8;     // a logical line longer than this is cut
  const MAX_DECOS = 1000;      // per terminal

  // `env`: { enabled(): bool, contract(): object, setTimeout, clearTimeout,
  // paint(el, piece, term) }. Returns { refresh, dispose, stats }.
  function attachWith(term, env) {
    let decos = [];            // { marker, deco, key }
    const dirty = new Set();   // viewport rows
    let timer = 0;
    let all = false;
    let lastY = null;          // the viewport's top line at the last render
    let syncNext = true;       // the next render is read at once (first, resize, buffer switch)
    let scanning = false;
    const stats = { scans: 0, rowsRead: 0, sync: 0, debounced: 0, markers: 0 };

    const drop = d => { d.deco?.dispose(); d.marker.dispose(); };
    const clear = () => { for (const d of decos) drop(d); decos = []; };
    const schedule = () => { if (!timer) timer = env.setTimeout(scan, SCAN_MS); };
    const scanNow = () => { if (timer) { env.clearTimeout(timer); timer = 0; } scan(); };

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
      stats.scans++;
      const buf = term.buffer.active;
      if (!env.enabled()) { clear(); dirty.clear(); all = false; return; }
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
      const scratch = buf.getNullCell();
      scanning = true;
      decos = decos.filter(d => {
        if (!d.gone && !d.marker.isDisposed && d.marker.line >= 0) return true;
        d.deco?.dispose();
        return false;
      });
      for (const s of starts) {
        const ys = [s];
        while (ys.length < MAX_WRAP_ROWS && buf.getLine(ys[ys.length - 1] + 1)?.isWrapped) ys.push(ys[ys.length - 1] + 1);
        const lines = ys.map(y => buf.getLine(y));
        stats.rowsRead += lines.length;
        const has = lines.map(l => l?.translateToString(true) || '').join('').includes('[[');
        const want = new Map();
        if (has) {
          const rowsIn = ys.map((y, i) => ({ y, cells: rowCells(lines[i], term.cols, scratch) }));
          for (const p of pieces(rowsIn, env.contract())) {
            const k = `${p.y}|${p.x}|${p.width}|${p.tone}|${p.left ? 1 : 0}${p.right ? 1 : 0}|${p.chars.map(c => c.ch).join('')}`;
            want.set(k, p);
          }
        }
        const onRows = new Set(ys);
        const keep = [];
        for (const d of decos) {
          if (d.gone) continue;
          const line = d.marker.line;
          if (!onRows.has(line)) { keep.push(d); continue; }
          const k = line + d.key.slice(d.key.indexOf('|'));
          if (want.has(k)) { want.delete(k); keep.push(d); } else drop(d);
        }
        decos = keep;
        for (const [k, p] of want) add(p, k);
      }
      scanning = false;
      if (decos.length > MAX_DECOS) {
        // Farthest from the viewport first.
        const mid = buf.viewportY + term.rows / 2;
        decos.sort((a, b) => Math.abs(a.marker.line - mid) - Math.abs(b.marker.line - mid));
        for (const d of decos.splice(MAX_DECOS)) drop(d);
      }
    }

    function add(p, k) {
      const buf = term.buffer.active;
      const marker = term.registerMarker(p.y - (buf.baseY + buf.cursorY));
      if (!marker) return;
      const deco = term.registerDecoration({ marker, x: p.x, width: p.width, layer: 'top' });
      if (!deco) { marker.dispose(); return; }
      stats.markers++;
      deco.onRender(el => env.paint(el, p, term));
      const d = { marker, deco, key: k };
      // Trimmed off the scrollback (or cleared): gone from the list at once.
      // (During a scan the list is being walked: marked, and left to it.)
      marker.onDispose?.(() => {
        d.gone = true;
        if (scanning) return;
        const i = decos.indexOf(d);
        if (i >= 0) decos.splice(i, 1);
      });
      decos.push(d);
    }

    function onRender(e) {
      const y = term.buffer.active.viewportY;
      const moved = lastY !== null && y !== lastY;
      lastY = y;
      for (let r = e.start; r <= e.end; r++) dirty.add(r);
      if (syncNext || moved || e.end - e.start + 1 <= SYNC_ROWS) {
        syncNext = false;
        stats.sync++;
        scanNow();
      } else {
        stats.debounced++;
        schedule();
      }
    }
    const reset = () => { clear(); all = true; syncNext = true; schedule(); };

    const subs = [
      term.onRender(onRender),
      term.onResize(reset),
      term.buffer.onBufferChange(reset),
      // The pill hides the selection drawn under it: while there is one,
      // the pills step aside.
      term.onSelectionChange(() => term.element?.classList.toggle('emph-selecting', term.hasSelection())),
    ];
    const h = {
      refresh: reset,
      dispose() { if (timer) env.clearTimeout(timer); timer = 0; subs.forEach(s => s.dispose()); clear(); },
      stats,
      get count() { return decos.length; },
    };
    all = true;
    schedule();
    return h;
  }

  const api = { find, findCodePoints, pieces, key, attachWith, SYNC_ROWS, SCAN_MS, MAX_DECOS };

  // ── Browser half ───────────────────────────────────────────────────────
  if (typeof window === 'undefined') {
    if (typeof module !== 'undefined') module.exports = api;
    return;
  }

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

  // The row's own background under a pill, as CSS: the pill's fill is
  // blended over it (a diff line's green band stays green around the pill),
  // and the cell-high frame around the pill is painted with it, which hides
  // the canvas glyphs it covers.
  const XTERM_16 = ['black', 'red', 'green', 'yellow', 'blue', 'magenta', 'cyan', 'white',
    'brightBlack', 'brightRed', 'brightGreen', 'brightYellow', 'brightBlue', 'brightMagenta', 'brightCyan', 'brightWhite'];
  function paletteColor(theme, i) {
    if (i < 16) return theme?.[XTERM_16[i]] || null;
    if (i < 232) {
      const n = i - 16, v = c => (c ? 55 + c * 40 : 0);
      return `rgb(${v(Math.floor(n / 36))} ${v(Math.floor(n / 6) % 6)} ${v(n % 6)})`;
    }
    const g = 8 + (i - 232) * 10;
    return `rgb(${g} ${g} ${g})`;
  }
  function rowBackground(term, p) {
    const theme = term.options.theme || {};
    const line = term.buffer.active.getLine(p.y);
    const cell = line?.getCell(p.x);
    let bg = null;
    if (cell && !cell.isBgDefault()) {
      if (cell.isBgRGB()) {
        const c = cell.getBgColor();
        bg = `rgb(${(c >> 16) & 255} ${(c >> 8) & 255} ${c & 255})`;
      } else if (cell.isBgPalette()) bg = paletteColor(theme, cell.getBgColor());
    }
    return bg || theme.background || null;
  }

  // The pill draws the label itself, one span per cell (widths in % of the
  // pill, so a scaled font keeps every glyph on its cell). A bracket of the
  // marker's own start or end keeps its cell but draws nothing (.emph-br): a
  // wrapped marker's pieces each hide the brackets they hold. The element
  // is the cells' frame, in the row's background; the pill inside it is a
  // little shorter than the cell. Called on every viewport render: only the
  // font size is looked at again.
  function paint(el, p, term) {
    const size = `${term.options.fontSize}px`;
    if (el.style.fontSize !== size) el.style.fontSize = size;
    if (el.dataset.emph) return;
    el.dataset.emph = '1';
    el.className += ` emph-cell emph-${p.tone}${p.left ? ' emph-l' : ''}${p.right ? ' emph-r' : ''}`;
    el.style.fontFamily = term.options.fontFamily;
    const row = rowBackground(term, p);
    if (row) el.style.setProperty('--emph-row', row);
    const fg = term.options.theme?.foreground;
    if (fg) el.style.setProperty('--emph-term-fg', fg);
    const pill = document.createElement('span');
    pill.className = 'emph-pill';
    const n = p.chars.length;
    p.chars.forEach((c, i) => {
      const s = document.createElement('span');
      s.textContent = c.ch;
      s.style.width = `${(c.w / p.width) * 100}%`;
      const bracket = (p.left && i < 2 && c.ch === '[') || (p.right && i >= n - 2 && c.ch === ']');
      if (bracket) s.className = 'emph-br';
      pill.append(s);
    });
    el.append(pill);
  }

  // Draw pills on `term`; `sandboxOf()` names the sandbox it shows now (it
  // changes when a pane is pointed elsewhere). Returns { refresh, dispose }.
  function attach(term, sandboxOf) {
    const inner = attachWith(term, {
      enabled: () => enabledFor(sandboxOf()),
      contract: () => contract,
      setTimeout: (f, ms) => setTimeout(f, ms),
      clearTimeout: t => clearTimeout(t),
      paint,
    });
    const h = {
      refresh: inner.refresh,
      dispose() { inner.dispose(); attached.delete(h); },
    };
    attached.add(h);
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
