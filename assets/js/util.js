// ── Toast ─────────────────────────────────────────────────────────────────
let pasteToastTimer = null;
function showToast(msg, kind) {
  const t = document.getElementById('paste-toast');
  t.textContent = msg;
  t.className = 'paste-toast show' + (kind ? ' ' + kind : '');
  clearTimeout(pasteToastTimer);
  if (kind !== 'progress')
    pasteToastTimer = setTimeout(() => { t.className = 'paste-toast'; }, 3000);
}

// ── Background-job corner indicators ─────────────────────────────────────
// Long-running operations (sandbox creation, port publishing) run in the
// background instead of blocking the UI behind a modal overlay. Each one
// gets a small pixel-logo card stacked in the bottom-right corner — "SBXW"
// rendered as a 5×7 dot-matrix glyph grid (same blue/green pair as the
// header badge), with a fill level sweeping up through it and back down on
// a loop for as long as the job is running.
const PIXEL_FONT = {
  S: ['01111','10000','10000','01110','00001','00001','11110'],
  B: ['11110','10001','10001','11110','10001','10001','11110'],
  X: ['10001','10001','01010','00100','01010','10001','10001'],
  W: ['10001','10001','10001','10101','10101','11011','10001'],
};
const PIXEL_LETTER_COLORS = { S: '#58a6ff', B: '#58a6ff', X: '#3fb950', W: '#3fb950' };

function buildPixelGrid(container) {
  const letters = ['S', 'B', 'X', 'W'];
  const rows = 7, letterW = 5, gap = 1;
  const cols = letters.length * (letterW + gap) - gap;
  container.style.setProperty('--cols', cols);
  container.style.setProperty('--rows', rows);

  const pixels = [];
  letters.forEach((letter, li) => {
    const pattern = PIXEL_FONT[letter];
    for (let r = 0; r < rows; r++) {
      for (let c = 0; c < letterW; c++) {
        if (pattern[r][c] !== '1') continue;
        const el = document.createElement('div');
        el.className = 'px';
        el.style.gridColumn = li * (letterW + gap) + c + 1;
        el.style.gridRow = r + 1;
        el.style.setProperty('--c', PIXEL_LETTER_COLORS[letter]);
        container.appendChild(el);
        pixels.push({ el, row: r });
      }
    }
  });
  return { pixels, rows };
}

function animatePixelGrid(pixels, rows) {
  let level = 0, dir = 1, dwell = 0;
  return setInterval(() => {
    const litFromRow = rows - level;
    pixels.forEach(p => p.el.classList.toggle('lit', p.row >= litFromRow));
    if (dwell > 0) { dwell--; return; }
    level += dir;
    if (level >= rows) { level = rows; dir = -1; dwell = 3; }
    else if (level <= 0) { level = 0; dir = 1; dwell = 3; }
  }, 110);
}

const bgJobsEl = document.getElementById('bg-jobs');
const bgJobs = new Map();
let bgJobSeq = 0;

/** Adds a corner card for a long-running background operation; returns its id. */
function startBgJob(label) {
  const id = ++bgJobSeq;
  const card = document.createElement('div');
  card.className = 'bg-job-card';
  const logo = document.createElement('div');
  logo.className = 'bg-job-pixel-logo';
  const text = document.createElement('div');
  text.className = 'bg-job-label';
  text.textContent = label;
  card.append(logo, text);
  bgJobsEl.appendChild(card);

  const { pixels, rows } = buildPixelGrid(logo);
  const timer = animatePixelGrid(pixels, rows);
  bgJobs.set(id, { el: card, timer });
  return id;
}

/** Removes the corner card for `id`, if it's still active. */
function finishBgJob(id) {
  const job = bgJobs.get(id);
  if (!job) return;
  clearInterval(job.timer);
  job.el.classList.add('leaving');
  setTimeout(() => job.el.remove(), 200);
  bgJobs.delete(id);
}
