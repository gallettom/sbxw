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
      // The detail starts at its colon: "[[" WORD ":" ...
      const detailAt = m.detail === null ? Infinity : m.index + 2 + m.word.length;
      for (let i = m.index; i < m.end; i++) {
        const pos = at[i];
        if (!pos) continue;
        const [ri, ci] = pos;
        const c = rows[ri].cells[ci];
        if (!cur || cur.row !== ri) {
          if (cur) out.push(cur);
          cur = { row: ri, y: rows[ri].y, x: c.x, width: 0, chars: [], tone: m.tone, left: i === m.index, right: false };
        }
        cur.chars.push({ ch: c.ch, w: c.w, detail: i >= detailAt && i < m.end - 2 });
        cur.width = c.x + c.w - cur.x;
      }
      if (cur) { cur.right = true; out.push(cur); }
    }
    return out.map(({ row, ...p }) => p);
  }

  // ── Pill colours (the desktop's rule) ──────────────────────────────────
  // Fill: the tone at 10% over the background under the pill (the cell's
  // own, so a diff band shows through). Word: the terminal's text mixed 45%
  // toward the tone, then moved toward white on a dark ground (black on a
  // light one) until it reads at 4.5:1 on the fill. Detail: the terminal's
  // text, same contrast rule. Danger: the tone darkened until white reads at
  // 4.5:1 on it, white text. The contrast step is why this is code and not
  // color-mix() alone: CSS cannot test a contrast ratio.
  function parseColor(s) {
    if (!s) return null;
    s = String(s).trim();
    let m = /^#([0-9a-f]{3,8})$/i.exec(s);
    if (m) {
      let h = m[1];
      if (h.length === 3 || h.length === 4) h = [...h.slice(0, 3)].map(c => c + c).join('');
      return [0, 2, 4].map(i => parseInt(h.slice(i, i + 2), 16));
    }
    m = /^rgba?\(\s*([\d.]+)[\s,]+([\d.]+)[\s,]+([\d.]+)/i.exec(s);
    return m ? [m[1], m[2], m[3]].map(Number) : null;
  }
  const hex = c => '#' + c.map(v => Math.round(Math.max(0, Math.min(255, v))).toString(16).padStart(2, '0')).join('');
  const mix = (a, b, t) => a.map((v, i) => v + (b[i] - v) * t);   // t of the way from a to b
  const lum = c => {
    const [r, g, b] = c.map(v => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; });
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  };
  const contrast = (a, b) => { const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p); return (x + 0.05) / (y + 0.05); };
  const WHITE = [255, 255, 255], BLACK = [0, 0, 0];
  // `c` moved toward `toward` in steps of 2%, until it reads at 4.5:1 on `on`.
  const readable = (c, on, toward) => {
    for (let k = 0; k <= 50; k++) {
      const d = mix(c, toward, k / 50);
      if (contrast(d, on) >= 4.5) return d;
    }
    return toward;
  };
  // `tone`, `bg`, `text`: colours as CSS strings. Returns { fill, word,
  // detail } as hex, or null when one cannot be read.
  function pillColors(tone, bg, text, danger) {
    const [t, b, x] = [tone, bg, text].map(parseColor);
    if (!t || !b || !x) return null;
    const dark = contrast(b, WHITE) > contrast(b, BLACK);
    if (danger) {
      let f = t;
      for (let k = 0; k < 100 && contrast(WHITE, f) < 4.5; k++) f = mix(f, BLACK, 0.02);
      return { fill: hex(f), word: '#ffffff', detail: '#ffffff' };
    }
    const fill = mix(b, t, 0.10);
    const toward = dark ? WHITE : BLACK;
    return {
      fill: hex(fill),
      word: hex(readable(mix(x, t, 0.45), fill, toward)),
      detail: hex(readable(x, fill, toward)),
    };
  }
  // The tones on a light ground: the web theme's tone variables are a dark
  // ground's, so a light terminal takes these (GitHub's light palette, the
  // desktop's light tones).
  const LIGHT_TONES = { green: '#1a7f37', red: '#cf222e', amber: '#9a6700', blue: '#0969da', purple: '#8250df', cyan: '#197d87', danger: '#cf222e' };

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

  const api = { find, findCodePoints, pieces, key, attachWith, pillColors, contrast, parseColor, LIGHT_TONES, SYNC_ROWS, SCAN_MS, MAX_DECOS };

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

  // A tone's colours on a ground and text, computed once per combination.
  // The web theme's tones, as the desktop's dark ground uses them. Danger's
  // base is the one colour hard-coded on a dark ground: GitHub's danger
  // emphasis (#b62324), which the desktop fills with; the theme's --c-red
  // is a text colour, too light under white.
  const TONE_VARS = { green: '--c-green', red: '--c-red', amber: '--c-amber', blue: '--c-blue', purple: '--c-purple', cyan: '--c-group-2' };
  const DARK_DANGER = '#b62324';
  const colorCache = new Map();
  function colorsFor(tone, bg, fg) {
    const k = `${tone}|${bg}|${fg}`;
    if (colorCache.has(k)) return colorCache.get(k);
    const b = parseColor(bg);
    const lightGround = b && contrast(b, [0, 0, 0]) > contrast(b, [255, 255, 255]);
    const css = tone === 'danger' ? DARK_DANGER : getComputedStyle(document.documentElement).getPropertyValue(TONE_VARS[tone] || '--c-blue');
    const out = pillColors(lightGround ? LIGHT_TONES[tone] || LIGHT_TONES.blue : css, bg, fg, tone === 'danger');
    if (colorCache.size > 200) colorCache.clear();
    colorCache.set(k, out);
    return out;
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
    const colors = row && fg ? colorsFor(p.tone, row, fg) : null;
    if (colors) {
      el.style.setProperty('--emph-fill-c', colors.fill);
      el.style.setProperty('--emph-word-c', colors.word);
      el.style.setProperty('--emph-detail-c', colors.detail);
    }
    const pill = document.createElement('span');
    pill.className = 'emph-pill';
    const n = p.chars.length;
    p.chars.forEach((c, i) => {
      const s = document.createElement('span');
      s.textContent = c.ch;
      s.style.width = `${(c.w / p.width) * 100}%`;
      const bracket = (p.left && i < 2 && c.ch === '[') || (p.right && i >= n - 2 && c.ch === ']');
      if (bracket) s.className = 'emph-br';
      else if (c.detail) s.className = 'emph-detail';
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
