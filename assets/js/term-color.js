// ── Terminal colours ────────────────────────────────────────────────────────
// Three colours, picked the same way: the background every pane's terminal is
// drawn on, the bar above it, and the text. For each, the first source set
// wins: what this browser picked (localStorage), sbxw.toml (`terminal_color`,
// `terminal_bar_color`, `terminal_text_color`, baked into the page), the
// built-in value. Text defaults to `auto`: it follows the background's
// lightness, so a light pick stays readable. A picked text colour replaces the
// default foreground and ANSI white / bright white; programs that print an
// explicit 24-bit colour keep it, the theme cannot reach those.
//
// Loaded before panes.js: setupTerminal() reads termTheme() for every new
// terminal, and applyTermColors() repaints the ones already open.

const TERM_COLOR_SLOTS = {
  term: {
    heading: 'Terminal',
    key: 'sbxw:termcolor',
    config: TERM_COLOR_CONFIG,
    builtin: '#0d1117',
    cssVar: '--c-term',
    presets: [
      ['#0d1117', 'Midnight (built-in)'],
      ['#000000', 'Black'],
      ['#1a1b26', 'Night'],
      ['#002b36', 'Solarized'],
      ['#0e1f17', 'Phosphor'],
      ['#1f1530', 'Aubergine'],
      ['#2a1d10', 'Amber'],
      ['#fdf6e3', 'Paper'],
    ],
  },
  bar: {
    heading: 'Top bar',
    key: 'sbxw:termbarcolor',
    config: TERM_BAR_COLOR_CONFIG,
    builtin: '#2f4f4f',  // darkslategrey, the bar's colour before it had a picker
    cssVar: '--c-pane-bar',
    // Dark on purpose: the bar's labels and buttons are light text.
    presets: [
      ['#2f4f4f', 'Slate (built-in)'],
      ['#21262d', 'Graphite'],
      ['#1c2f4a', 'Navy'],
      ['#163a3a', 'Teal'],
      ['#2b3a1f', 'Moss'],
      ['#3a2b4f', 'Plum'],
      ['#4a2a2a', 'Oxblood'],
      ['#4a3a1a', 'Ochre'],
    ],
  },
  text: {
    heading: 'Text',
    key: 'sbxw:termtextcolor',
    config: TERM_TEXT_COLOR_CONFIG,
    builtin: 'auto',
    presets: [
      ['auto', 'Auto (follows the background)'],
      ['#ffffff', 'White'],
      ['#c9d1d9', 'Silver'],
      ['#8b949e', 'Grey'],
      ['#e6d5b8', 'Cream'],
      ['#ffb000', 'Amber'],
      ['#33ff66', 'Phosphor'],
      ['#7dcfff', 'Ice'],
    ],
  },
};

function isHexColor(c) { return /^#[0-9a-f]{6}$/i.test(c || ''); }

// `auto` is a value only the text slot takes: it can be stored, so that
// picking Auto in the browser wins over a colour set in sbxw.toml.
function isSlotColor(slot, c) { return isHexColor(c) || (slot === 'text' && c === 'auto'); }

function storedColor(slot) {
  try { const c = (localStorage.getItem(TERM_COLOR_SLOTS[slot].key) || '').toLowerCase(); return isSlotColor(slot, c) ? c : ''; }
  catch (_) { return ''; }
}

function defaultColor(slot) {
  const s = TERM_COLOR_SLOTS[slot];
  return isSlotColor(slot, (s.config || '').toLowerCase()) ? s.config.toLowerCase() : s.builtin;
}

function currentColor(slot) { return storedColor(slot) || defaultColor(slot); }

// WCAG relative luminance; above ~0.4 dark text reads better than light.
function isLightColor(hex) {
  const lin = v => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; };
  const [r, g, b] = [1, 3, 5].map(i => lin(parseInt(hex.slice(i, i + 2), 16)));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b > 0.4;
}

function termTextColor(bg, text = 'auto') {
  if (isHexColor(text)) return text;
  return isLightColor(bg) ? '#24292f' : '#c9d1d9';
}

function termTheme(bg = currentColor('term'), text = currentColor('text')) {
  const light = isLightColor(bg);
  const theme = {
    background: bg,
    foreground: termTextColor(bg, text),
    cursor: light ? '#0969da' : '#58a6ff',
    selectionBackground: light ? '#0969da40' : '#58a6ff4d',
  };
  // Auto leaves xterm's own ANSI whites alone. A picked colour takes them
  // too: most "white" text in a TUI is ANSI 7/15, not the default foreground.
  if (isHexColor(text)) theme.white = theme.brightWhite = text;
  return theme;
}

// The trigger's swatch: `el` is the background, its <i> the bar.
function paintPane(el, bg, bar, text) {
  el.style.background = bg;
  el.style.color = termTextColor(bg, text);
  el.querySelector('i').style.background = bar;
}

// The panel is one terminal drawn small: click its bar, its background or its
// text to say what to recolour, then pick below. Hovering a colour shows it
// in the preview before it is picked.
let termColorSlot = 'term';
const TERM_COLOR_ZONES = { term: 'Background', bar: 'Top bar', text: 'Text' };

function paintPreview(over = {}) {
  const c = slot => over[slot] || currentColor(slot);
  const bg = c('term'), bar = c('bar'), text = c('text');
  const preview = document.querySelector('.tc-preview');
  preview.style.background = bg;
  preview.querySelector('.tc-bar').style.background = bar;
  preview.querySelector('.tc-text').style.color = termTextColor(bg, text);
}

function applyTermColors() {
  const bg = currentColor('term'), bar = currentColor('bar'), text = currentColor('text');
  const root = document.documentElement.style;
  root.setProperty('--c-term', bg);
  root.setProperty('--c-pane-bar', bar);
  if (typeof panes !== 'undefined') {
    const theme = termTheme(bg, text);
    panes.forEach(p => { if (p && p.term) p.term.options.theme = theme; });
    if (typeof monitorPane !== 'undefined' && monitorPane) monitorPane.term.options.theme = theme;
  }
  paintPane(document.getElementById('term-color-swatch'), bg, bar, text);
  paintPreview();
  renderTermColorPicker();
}

function renderTermColorPicker() {
  const slot = termColorSlot, s = TERM_COLOR_SLOTS[slot], cur = currentColor(slot);
  const panel = document.getElementById('term-color-panel');
  panel.querySelectorAll('.tc-zone').forEach(z => {
    const on = z.dataset.slot === slot;
    z.classList.toggle('active', on);
    z.setAttribute('aria-pressed', String(on));
  });
  panel.querySelector('.tc-heading').textContent = TERM_COLOR_ZONES[slot];

  const presets = panel.querySelector('.term-color-presets');
  if (presets.dataset.slot !== slot) {
    presets.dataset.slot = slot;
    presets.replaceChildren(...s.presets.map(([color, name]) => {
      const b = document.createElement('button');
      b.dataset.color = color;
      b.title = name;
      b.setAttribute('aria-label', `${TERM_COLOR_ZONES[slot]}: ${name}`);
      if (slot === 'text') b.textContent = color === 'auto' ? 'auto' : 'Aa';
      b.addEventListener('click', () => setColor(slot, color));
      b.addEventListener('pointerenter', () => paintPreview({ [slot]: color }));
      b.addEventListener('focus', () => paintPreview({ [slot]: color }));
      b.addEventListener('pointerleave', () => paintPreview());
      b.addEventListener('blur', () => paintPreview());
      return b;
    }));
  }
  const bg = currentColor('term');
  presets.querySelectorAll('button').forEach(b => {
    const c = b.dataset.color;
    // Text is shown as text on the current background; the others as colour.
    b.style.background = slot === 'text' ? bg : c;
    b.style.color = slot === 'text' ? termTextColor(bg, c) : '';
    b.classList.toggle('active', c === cur);
  });

  // The native picker only holds a hex: on Auto, show the colour Auto gives.
  panel.querySelector('input[type=color]').value = slot === 'text' ? termTextColor(bg, cur) : cur;
  const reset = panel.querySelector('.term-color-reset');
  const from = isSlotColor(slot, (s.config || '').toLowerCase()) ? 'sbxw.toml' : 'built-in';
  reset.disabled = !storedColor(slot);
  reset.textContent = reset.disabled ? `Default: ${from}` : `Reset to ${from}`;
}

function setColor(slot, c) {
  try {
    c && c.toLowerCase() !== defaultColor(slot)
      ? localStorage.setItem(TERM_COLOR_SLOTS[slot].key, c.toLowerCase())
      : localStorage.removeItem(TERM_COLOR_SLOTS[slot].key);
  } catch (_) {}
  applyTermColors();
}

function closeTermColorPanel() {
  document.getElementById('term-color-panel').hidden = true;
  const trigger = document.getElementById('term-color-btn');
  trigger.classList.remove('open');
  trigger.setAttribute('aria-expanded', 'false');
}

(function initTermColors() {
  const panel = document.getElementById('term-color-panel');
  // Three sibling buttons, not nested ones: the text sits over the background.
  panel.innerHTML = `
    <div class="tc-preview">
      <button class="tc-zone tc-bar" data-slot="bar" title="Top bar"><span>my-sandbox</span><span>claude</span></button>
      <div class="tc-body">
        <button class="tc-zone tc-bg" data-slot="term" title="Background" aria-label="Background"></button>
        <button class="tc-zone tc-text" data-slot="text" title="Text">$ claude<br>&#x273B; Welcome back<br>&gt; <span class="tc-cursor">_</span></button>
      </div>
    </div>
    <p class="tc-hint">Click the bar, the background or the text to recolour it.</p>
    <span class="tc-heading"></span>
    <div class="term-color-presets"></div>
    <div class="tc-actions">
      <label class="term-color-custom"><input type="color"><span>Custom</span></label>
      <button class="term-color-reset"></button>
    </div>`;
  panel.querySelectorAll('.tc-zone').forEach(z => z.addEventListener('click', () => {
    termColorSlot = z.dataset.slot;
    renderTermColorPicker();
  }));
  // `input` fires while the native picker is dragged: the panes follow live.
  panel.querySelector('input').addEventListener('input', e => setColor(termColorSlot, e.target.value));
  panel.querySelector('.term-color-reset').addEventListener('click', () => setColor(termColorSlot, ''));

  const trigger = document.getElementById('term-color-btn');
  trigger.addEventListener('click', e => {
    e.stopPropagation();
    // The stopped click never reaches the pane-count picker's outside-click
    // close, so close it here: one popover at a time.
    closeLayoutSwitchPanel();
    const willOpen = panel.hidden;
    panel.hidden = !willOpen;
    trigger.classList.toggle('open', willOpen);
    trigger.setAttribute('aria-expanded', String(willOpen));
  });
  document.addEventListener('click', e => {
    if (!document.getElementById('term-color').contains(e.target)) closeTermColorPanel();
  });
  document.addEventListener('keydown', e => { if (e.key === 'Escape') closeTermColorPanel(); });

  applyTermColors();
})();
