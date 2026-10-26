// ── Terminal colours ────────────────────────────────────────────────────────
// Two colours, picked the same way: the background every pane's terminal is
// drawn on, and the bar above it. For each, the first source set wins: what
// this browser picked (localStorage), sbxw.toml (`terminal_color`,
// `terminal_bar_color`, baked into the page), the built-in value. Terminal
// text and cursor follow the background's lightness, so a light pick stays
// readable without a third setting.
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
};

function isHexColor(c) { return /^#[0-9a-f]{6}$/i.test(c || ''); }

function storedColor(slot) {
  try { const c = localStorage.getItem(TERM_COLOR_SLOTS[slot].key); return isHexColor(c) ? c.toLowerCase() : ''; }
  catch (_) { return ''; }
}

function defaultColor(slot) {
  const s = TERM_COLOR_SLOTS[slot];
  return isHexColor(s.config) ? s.config.toLowerCase() : s.builtin;
}

function currentColor(slot) { return storedColor(slot) || defaultColor(slot); }

// WCAG relative luminance; above ~0.4 dark text reads better than light.
function isLightColor(hex) {
  const lin = v => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; };
  const [r, g, b] = [1, 3, 5].map(i => lin(parseInt(hex.slice(i, i + 2), 16)));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b > 0.4;
}

function termTextColor(bg) { return isLightColor(bg) ? '#24292f' : '#c9d1d9'; }

function termTheme(bg = currentColor('term')) {
  const light = isLightColor(bg);
  return {
    background: bg,
    foreground: termTextColor(bg),
    cursor: light ? '#0969da' : '#58a6ff',
    selectionBackground: light ? '#0969da40' : '#58a6ff4d',
  };
}

// A tiny pane: `el` is the background with `>_` in it, its <i> the bar.
function paintPane(el, bg, bar) {
  el.style.background = bg;
  el.style.color = termTextColor(bg);
  el.querySelector('i').style.background = bar;
}

function applyTermColors() {
  const bg = currentColor('term'), bar = currentColor('bar');
  const root = document.documentElement.style;
  root.setProperty('--c-term', bg);
  root.setProperty('--c-pane-bar', bar);
  if (typeof panes !== 'undefined') {
    const theme = termTheme(bg);
    panes.forEach(p => { if (p && p.term) p.term.options.theme = theme; });
  }
  paintPane(document.getElementById('term-color-swatch'), bg, bar);

  for (const [slot, s] of Object.entries(TERM_COLOR_SLOTS)) {
    const section = document.querySelector(`.term-color-section[data-slot="${slot}"]`);
    const cur = currentColor(slot);
    // Each preset previews itself against the other slot's current colour.
    section.querySelectorAll('.term-color-presets button').forEach(b => {
      const c = b.dataset.color;
      slot === 'term' ? paintPane(b, c, bar) : paintPane(b, bg, c);
      b.classList.toggle('active', c === cur);
    });
    section.querySelector('input[type=color]').value = cur;
    const reset = section.querySelector('.term-color-reset');
    const from = isHexColor(s.config) ? 'sbxw.toml' : 'built-in';
    reset.disabled = !storedColor(slot);
    reset.textContent = reset.disabled ? `Default: ${from}` : `Reset to ${from}`;
  }
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
  for (const [slot, s] of Object.entries(TERM_COLOR_SLOTS)) {
    const section = document.createElement('div');
    section.className = 'term-color-section';
    section.dataset.slot = slot;
    section.innerHTML = `
      <span class="term-color-heading">${s.heading}</span>
      <div class="term-color-presets"></div>
      <label class="term-color-custom"><input type="color"><span>Custom colour</span></label>
      <button class="term-color-reset"></button>`;
    const presets = section.querySelector('.term-color-presets');
    for (const [color, name] of s.presets) {
      const b = document.createElement('button');
      b.dataset.color = color;
      b.title = name;
      b.setAttribute('aria-label', `${s.heading}: ${name}`);
      b.innerHTML = '<i></i>&gt;_';
      b.addEventListener('click', () => setColor(slot, color));
      presets.appendChild(b);
    }
    // `input` fires while the native picker is dragged: the panes follow live.
    section.querySelector('input').addEventListener('input', e => setColor(slot, e.target.value));
    section.querySelector('.term-color-reset').addEventListener('click', () => setColor(slot, ''));
    panel.appendChild(section);
  }

  const trigger = document.getElementById('term-color-btn');
  trigger.addEventListener('click', e => {
    e.stopPropagation();
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
