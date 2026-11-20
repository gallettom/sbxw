// ── Kanban ────────────────────────────────────────────────────────────────
// The tickets of a sandbox (or of every sandbox) in five columns: Planned,
// In progress, In test, Ready to merge, Merged. The daemon decides everything
// on the board: which column a card is in, its labels, marks and counts, and
// what the person may do (`sbxw_core::tickets`). This page draws its answer
// and computes none of it. See docs/web-ui.md#the-kanban and
// docs/threads-and-lead.md#tickets-and-the-kanban.
//
//   GET  /api/tickets?sandbox=<name>|all&older=1   {ok, board}
//   GET  /api/tickets/:sandbox/:n                  {ok, ticket}
//   POST /api/tickets/:sandbox                     {title, description, priority} → {ok, ticket}
//   POST /api/tickets/:sandbox/:n/files?name=      the file's bytes → {ok, ticket}
//   GET  /api/tickets/:sandbox/:n/files/:id        the bytes, as an attachment
//   stream event `tickets` {sandbox, n?, what}, and `plan`
//
// The web view sees the board, opens a ticket and adds one with files. Moving,
// editing, deleting a ticket, removing a file and "Send to the lead" are the
// desktop app's: they are absent here, not disabled.
//
// Text from a ticket, a thread or an agent is data: it reaches the page only
// through escHtml (as text, or in a quoted attribute), never as markup, and is
// never logged. A link to a file is built from the ids the daemon returned,
// never from a name.

// BEGIN kanban pure: no DOM, no fetch; tests/js/kanban.test.mjs runs it.

// The tones the daemon sends, each a class in app.css; anything else is drawn
// neutral rather than put in a class name.
const KB_TONES = ['neutral', 'blue', 'purple', 'amber', 'green', 'red', 'dim'];
const kbTone = t => (KB_TONES.includes(t) ? t : 'neutral');
// A red edge for failed or refused work only (the contract's three marks).
const KB_RED_MARKS = ['failed', 'merge_failed', 'refused'];

// File names that look like a secret: the desktop's list
// (desktop/src/feat/files/drops.rs SECRET_NAMES), matched case-insensitively,
// `x*` a prefix, `*x` a suffix, else the whole name. The web upload has none.
const KB_SECRET_NAMES = ['.env*', '*.pem', '*.key', 'id_rsa*', 'id_ed25519*', '*.p12', '*.pfx', 'credentials*', '.npmrc', '.netrc', '*.kdbx'];
// More files, or more bytes in all, than this: asked, as the desktop does.
const KB_ASK_ABOVE_FILES = 10;
const KB_ASK_ABOVE_BYTES = 25 * 1024 * 1024;
// Bytes read from each file to know its kind: tar's mark is at 257.
const KB_SNIFF_BYTES = 512;
// Pictures previewed in an open ticket, at most (as the desktop).
const KB_PICTURES = 4;

function kbSecretName(name) {
  const n = String(name || '').toLowerCase();
  return KB_SECRET_NAMES.some(p => (p.startsWith('*') ? n.endsWith(p.slice(1))
    : p.endsWith('*') ? n.startsWith(p.slice(0, -1)) : n === p));
}

// Why the daemon would refuse these first bytes, or '' when it would not:
// the same magic bytes as `sbxw_core::tickets::sniff`, and its words.
function kbSniff(b) {
  const starts = (...m) => b.length >= m.length && m.every((x, i) => b[i] === x);
  const ascii = s => [...s].map(c => c.charCodeAt(0));
  if (!b.length) return 'the file is empty';
  const archive = starts(0x50, 0x4b, 0x03, 0x04) || starts(0x50, 0x4b, 0x05, 0x06) || starts(0x1f, 0x8b)
    || starts(0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c) || starts(...ascii('Rar!'), 0x1a, 0x07)
    || starts(0xfd, ...ascii('7zXZ'), 0x00) || starts(...ascii('BZh')) || starts(0x28, 0xb5, 0x2f, 0xfd)
    || (b.length > 262 && ascii('ustar').every((x, i) => b[257 + i] === x));
  if (archive) return 'archives (zip, tar, gzip, 7z, rar, xz, bzip2, zstd) are refused: attach the files themselves';
  if (starts(0x7f, ...ascii('ELF')) || starts(...ascii('MZ')) || starts(0xcf, 0xfa, 0xed, 0xfe)
    || starts(0xce, 0xfa, 0xed, 0xfe) || starts(0xca, 0xfe, 0xba, 0xbe)) {
    return 'programs are refused: a ticket\'s files are reference material';
  }
  return '';
}

function kbSize(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB'];
  let v = bytes / 1024, i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return `${v.toFixed(v < 10 ? 1 : 0)} ${units[i]}`;
}

// Files handed in, checked against what the form already holds and the
// daemon's limits (`board.limits`). `incoming`: [{name, size, head}] with
// `head` the first KB_SNIFF_BYTES bytes. Answers which go in and which are
// refused, each refusal with its reason, in the order handed in.
function kbCheckFiles(held, incoming, limits) {
  const take = [], refused = [];
  let count = held.length;
  let bytes = held.reduce((s, f) => s + f.size, 0);
  for (const f of incoming) {
    const kind = kbSniff(f.head);
    let why = '';
    if (kind) why = kind;
    else if (f.size > limits.file_bytes) why = `over ${kbSize(limits.file_bytes)}, the most one file can be`;
    else if (count + 1 > limits.files) why = `a ticket holds ${limits.files} files at most`;
    else if (bytes + f.size > limits.ticket_bytes) why = `the ticket's files would pass ${kbSize(limits.ticket_bytes)} in all`;
    if (why) { refused.push({ name: f.name, why }); continue; }
    count++;
    bytes += f.size;
    take.push(f);
  }
  return { take, refused };
}

// Why Save must ask before sending these files ([] when it need not): a name
// that looks like a secret, more than ten files, more than 25 MB in all. The
// desktop also asks for a file in a .ssh, .aws or .gnupg folder; a page is
// never told a file's folder, so that one cannot be checked here.
function kbAskReasons(files) {
  const why = [];
  for (const f of files) if (kbSecretName(f.name)) why.push(`${f.name}: its name looks like a secret (keys, tokens, credentials)`);
  if (files.length > KB_ASK_ABOVE_FILES) why.push(`${files.length} files (more than ${KB_ASK_ABOVE_FILES})`);
  const total = files.reduce((s, f) => s + f.size, 0);
  if (total > KB_ASK_ABOVE_BYTES) why.push(`${kbSize(total)} in all (more than ${kbSize(KB_ASK_ABOVE_BYTES)})`);
  return why;
}

// What stops the form before anything is sent, or ''.
function kbFormProblem(f, limits) {
  if (!f.sandbox) return 'Pick the sandbox the ticket is for.';
  const title = String(f.title || '').trim();
  if (!title) return 'A ticket needs a title.';
  if ([...title].length > limits.title) return `A title is at most ${limits.title} characters.`;
  if ([...String(f.description || '')].length > limits.description) return `A description is at most ${limits.description} characters.`;
  return '';
}

// A ticket's routes, from the sandbox name and the ids the daemon gave.
const kbTicketUrl = (sandbox, n) => `/api/tickets/${encodeURIComponent(sandbox)}/${encodeURIComponent(String(n))}`;
const kbFileUrl = (sandbox, n, id) => `${kbTicketUrl(sandbox, n)}/files/${encodeURIComponent(id)}`;
const kbBoardUrl = (scope, older) => `/api/tickets?sandbox=${encodeURIComponent(scope || 'all')}${older ? '&older=1' : ''}`;

function kbMarksHtml(marks) {
  const list = Array.isArray(marks) ? marks : [];
  if (!list.length) return '';
  return `<span class="kb-marks">${list.map(m =>
    `<span class="kb-mark kb-tone-${kbTone(m.tone)}" title="${escHtml(m.help)}">${escHtml(m.label)}</span>`).join('')}</span>`;
}

function kbThreadHtml(card) {
  if (!card.thread) return '';
  return `<span class="kb-thread"><span class="kb-dot kb-tone-${kbTone(card.thread_tone)}" aria-hidden="true"></span>${escHtml(card.thread)}</span>`;
}

// One card; `o.all` adds the sandbox's name. Both of the daemon's titles go
// in, and the column's width picks one (a container query in app.css):
// `title_short` under 250 px, `title_long` above.
function kbCardHtml(card, o = {}) {
  const red = (card.marks || []).some(m => KB_RED_MARKS.includes(m.id));
  return `<button type="button" class="kb-card${red ? ' kb-red' : ''}" data-kb-card="${escHtml(card.key)}" title="${escHtml(card.title)}">
      <span class="kb-card-head"><span class="kb-num${card.n == null ? ' kb-num-thread' : ''}">${escHtml(card.num)}</span>${
        o.all ? `<span class="kb-sb">${escHtml(card.sandbox)}</span>` : ''}<span class="kb-age">${escHtml(card.age)}</span></span>
      <span class="kb-card-title kb-t-short">${escHtml(card.title_short)}</span><span class="kb-card-title kb-t-long">${escHtml(card.title_long)}</span>
      ${kbMarksHtml(card.marks)}
      ${kbThreadHtml(card)}
    </button>`;
}

function kbColumnHtml(col, o = {}) {
  const cards = Array.isArray(col.cards) ? col.cards : [];
  const older = Number(col.older) || 0;
  return `<section class="kb-col" data-kb-col="${escHtml(col.id)}" aria-label="${escHtml(col.title)}">
      <div class="kb-col-head" title="${escHtml(col.help)}">
        <span class="kb-dot kb-tone-${kbTone(col.tone)}" aria-hidden="true"></span>
        <h3 class="kb-col-title">${escHtml(col.title)}</h3>
        <span class="kb-count">${Number(col.count) || 0}</span>
      </div>
      <div class="kb-col-cards" data-kb-scroll="${escHtml(col.id)}">
        ${cards.length ? cards.map(c => kbCardHtml(c, o)).join('') : `<p class="kb-empty">${escHtml(col.empty)}</p>`}
        ${older ? `<button type="button" class="kb-older" data-kb-older>Show ${older} older</button>` : ''}
      </div>
    </section>`;
}

// The five columns, in the daemon's order.
function kbBoardHtml(board, o = {}) {
  const cols = Array.isArray(board?.columns) ? board.columns : [];
  return `<div class="kb-cols">${cols.map(c => kbColumnHtml(c, o)).join('')}</div>`;
}

// Every card of the board by its key.
function kbCards(board) {
  const all = new Map();
  for (const col of board?.columns || []) for (const c of col.cards || []) all.set(c.key, c);
  return all;
}

function kbColumnTitle(board, id) {
  return (board?.columns || []).find(c => c.id === id)?.title || '';
}

// The thread's line and the ways to it this page has: `o.links` says which
// (`terminal`, `plan`, `worktrees`), since they depend on what is open.
function kbThreadLinksHtml(card, o) {
  if (!card.thread) return '';
  const l = o.links || {};
  const state = card.thread_state ? `<span class="kb-thread-state">${escHtml(String(card.thread_state).replace(/_/g, ' '))}</span>` : '';
  return `<div class="kb-p-thread">
      ${kbThreadHtml(card)}${state}
      <span class="kb-p-links">
        ${l.terminal ? '<button type="button" class="kb-link" data-kb-go="terminal" title="Open the thread\'s terminal">Terminal</button>' : ''}
        ${l.plan ? '<button type="button" class="kb-link" data-kb-go="plan" title="Open its plan">Plan</button>' : ''}
        ${l.worktrees ? '<button type="button" class="kb-link" data-kb-go="worktrees" title="Its branch in the worktree graph">Worktrees</button>' : ''}
      </span>
    </div>`;
}

// The open ticket's panel. `t` is the ticket route's answer, or null for a
// thread with no ticket (then `card` is all there is). `o.column` is the
// column's title, `o.links` as above.
function kbPanelHtml(card, t, o = {}) {
  const num = t?.num || card.num;
  const title = t ? t.title : card.title;
  const marks = (t?.card || card).marks;
  const files = Array.isArray(t?.files) ? t.files : [];
  const pics = files.filter(f => f.picture).slice(0, KB_PICTURES);
  const sandbox = card.sandbox;
  const history = Array.isArray(t?.history) ? t.history : [];
  const can = (t?.card || card).can || {};
  const parts = [];
  parts.push(`<div class="kb-p-head">
      <span class="kb-num${card.n == null ? ' kb-num-thread' : ''}">${escHtml(num)}</span>
      ${o.column ? `<span class="kb-p-col">${escHtml(o.column)}</span>` : ''}
      ${o.all ? `<span class="kb-sb">${escHtml(sandbox)}</span>` : ''}
      <button type="button" class="modal-close kb-p-close" data-kb-close aria-label="Close the ticket" title="Close the ticket (Esc)">✕</button>
    </div>`);
  parts.push(kbMarksHtml(marks));
  parts.push(`<h3 class="kb-p-title" id="kb-p-title">${escHtml(title)}</h3>`);
  if (t?.description) parts.push(`<p class="kb-p-desc">${escHtml(t.description)}</p>`);
  if (files.length) {
    parts.push(`<h4 class="kb-p-h">${escHtml(t.files_label || 'Files')}</h4>`);
    if (pics.length) {
      parts.push(`<div class="kb-p-pics">${pics.map(f =>
        `<img class="kb-p-pic" data-kb-pic="${escHtml(f.id)}" alt="${escHtml(f.name)}" title="${escHtml(f.name)}">`).join('')}</div>`);
    }
    parts.push(`<div class="kb-p-files">${files.map(f =>
      `<a class="kb-file" href="${escHtml(kbFileUrl(sandbox, t.n, f.id))}" download title="Download ${escHtml(f.name)}">
        <span class="kb-file-name">${escHtml(f.name)}</span><span class="kb-file-size">${escHtml(f.size)}</span></a>`).join('')}</div>`);
    parts.push(`<p class="kb-p-note">These files are copied into ${escHtml(sandbox)} when an agent reads this ticket (ticket_read), as the lead does when it takes it.</p>`);
  }
  parts.push(kbThreadLinksHtml(card, o));
  if (history.length) {
    parts.push(`<h4 class="kb-p-h">History</h4><ol class="kb-p-history">${history.map(h =>
      `<li><span class="kb-dot kb-tone-${kbTone(h.tone)}" aria-hidden="true"></span><span class="kb-h-text">${escHtml(h.text)}</span><span class="kb-age">${escHtml(h.when)}</span></li>`).join('')}</ol>`);
  }
  if (t) {
    const lines = ['Moving, editing and deleting a ticket are in the desktop app.'];
    if (can.send) lines.push('Send it to the lead from the desktop app, or ask the lead to read the planned tickets.');
    parts.push(`<p class="kb-p-note">${lines.map(escHtml).join(' ')}</p>`);
  } else if (card.n == null) {
    parts.push(`<p class="kb-p-note">${escHtml(can.send_why || 'A thread, not a ticket')}: it follows its plan.</p>`);
  } else {
    parts.push('<p class="kb-p-note">Reading the ticket…</p>');
  }
  return parts.filter(Boolean).join('\n');
}

// The form's list of files. `f.state`: wait, sending, done, failed.
function kbFormFilesHtml(files, locked) {
  return files.map((f, i) => {
    const flag = kbSecretName(f.name);
    const state = f.state === 'done' ? 'attached' : f.state === 'sending' ? 'sending…' : f.state === 'failed' ? 'refused' : '';
    return `<li class="kb-f-file${flag ? ' kb-flag' : ''} kb-f-${escHtml(f.state || 'wait')}">
        <span class="kb-file-name" title="${escHtml(f.name)}">${escHtml(f.name)}</span>
        ${flag ? '<span class="kb-mark kb-tone-amber" title="Its name looks like a secret: look at it before attaching">look first</span>' : ''}
        <span class="kb-file-size">${escHtml(kbSize(f.size))}</span>
        ${state ? `<span class="kb-f-state">${escHtml(state)}</span>` : ''}
        ${f.state === 'done' || locked ? '' : `<button type="button" class="kb-f-rm" data-kb-rm="${i}" aria-label="Remove ${escHtml(f.name)}" title="Remove">✕</button>`}
        ${f.error ? `<span class="kb-f-why">${escHtml(f.error)}</span>` : ''}
      </li>`;
  }).join('');
}

// After a Save that stopped part way: what went and what did not, said once.
function kbSaveReport(num, files) {
  const done = files.filter(f => f.state === 'done').map(f => f.name);
  const failed = files.filter(f => f.state === 'failed');
  const left = files.filter(f => f.state !== 'done' && f.state !== 'failed').map(f => f.name);
  const out = [`${num} was created.`];
  out.push(done.length ? `Attached: ${done.join(', ')}.` : 'No file was attached.');
  for (const f of failed) out.push(`Not attached: ${f.name} (${f.error}).`);
  if (left.length) out.push(`Not sent: ${left.join(', ')}.`);
  out.push(`Remove the refused file and press Attach the rest, or Close to keep ${num} as it is (the desktop app can remove its files or delete it).`);
  return out.join(' ');
}
// END kanban pure

const kbOverlay = document.getElementById('kb-overlay');
const kbBody = document.getElementById('kb-body');
const kbPanel = document.getElementById('kb-panel');
const kbScope = document.getElementById('kb-scope');
const kbCounts = document.getElementById('kb-counts');
const kbNote = document.getElementById('kb-note');
// scope: a sandbox's name or 'all'; board: the last answer; ticket: the open
// one ({key, card, t}) or null; back: the card to give the keyboard back to.
const kbv = { scope: 'all', older: false, board: null, open: null, busy: false, again: false, timer: null, pics: new Map(), seq: 0 };

function openKanban(scope) {
  kbv.scope = scope || panes[focusedPane]?.sandbox || 'all';
  kbv.older = false;
  kbv.board = null;
  kbCloseTicket(false);
  kbFillScope();
  setHtml(kbBody, '<p class="wt-note">Reading the tickets…</p>');
  kbCounts.textContent = '';
  showNote(kbNote, '');
  modalOpen(kbOverlay);
  document.getElementById('kb-new').focus();
  kbRead();
}

function closeKanban() {
  kbCloseTicket(false);
  kbOverlay.close();
  clearTimeout(kbv.timer);
}

function kbFillScope() {
  const names = sandboxes.map(s => s.name);
  if (kbv.scope !== 'all' && !names.includes(kbv.scope)) names.unshift(kbv.scope);
  kbScope.innerHTML = ['<option value="all">All sandboxes</option>',
    ...names.map(n => `<option value="${escHtml(n)}">${escHtml(n)}</option>`)].join('');
  kbScope.value = kbv.scope;
}

async function kbRead() {
  if (!kbOverlay.open) return;
  if (kbv.busy) { kbv.again = true; return; }
  kbv.busy = true;
  const scope = kbv.scope;
  let res;
  try {
    const r = await fetch(kbBoardUrl(scope, kbv.older));
    res = await r.json().catch(() => ({ ok: false, error: r.status === 404 ? 'this daemon has no ticket routes: rebuild sbxw and run sbxw up' : `HTTP ${r.status}` }));
  } catch (e) {
    res = { ok: false, error: e.message };
  }
  kbv.busy = false;
  if (kbv.again) { kbv.again = false; kbRead(); return; }
  if (scope !== kbv.scope || !kbOverlay.open) return;
  if (!res?.ok || !res.board) {
    const why = `Could not read the tickets: ${res?.error || 'no answer'}`;
    if (kbv.board) showNote(kbNote, why); else setHtml(kbBody, `<p class="wt-note wt-warn">${escHtml(why)}</p>`);
    return;
  }
  showNote(kbNote, '');
  kbv.board = res.board;
  kbPaint();
  if (kbv.open) kbReadTicket();
}

// A repaint keeps every column's scroll, the board's sideways scroll and the
// focused card.
function kbPaint() {
  const b = kbv.board;
  if (!b) return;
  const tops = new Map([...kbBody.querySelectorAll('[data-kb-scroll]')].map(el => [el.dataset.kbScroll, el.scrollTop]));
  const left = kbBody.querySelector('.kb-cols')?.scrollLeft || 0;
  const focused = document.activeElement?.closest?.('[data-kb-card]')?.dataset.kbCard;
  setHtml(kbBody, kbBoardHtml(b, { all: kbv.scope === 'all' }));
  kbBody.querySelectorAll('[data-kb-scroll]').forEach(el => { el.scrollTop = tops.get(el.dataset.kbScroll) || 0; });
  const cols = kbBody.querySelector('.kb-cols');
  if (cols) cols.scrollLeft = left;
  if (focused) kbCardEl(focused)?.focus({ preventScroll: true });
  kbCounts.textContent = b.counts_line || '';
  kbBody.querySelectorAll('.kb-card').forEach(el => el.classList.toggle('kb-on', el.dataset.kbCard === kbv.open?.key));
}

const kbCardEl = key => [...kbBody.querySelectorAll('[data-kb-card]')].find(el => el.dataset.kbCard === key) || null;

// ── The open ticket ──
function kbOpenTicket(key) {
  const card = kbCards(kbv.board).get(key);
  if (!card) return;
  if (kbv.open?.key !== key) kbRevokePics();
  kbv.open = { key, card, t: null };
  kbPaintPanel();
  kbPanel.hidden = false;
  kbBody.querySelectorAll('.kb-card').forEach(el => el.classList.toggle('kb-on', el.dataset.kbCard === key));
  kbPanel.querySelector('[data-kb-close]')?.focus();
  if (card.n != null) kbReadTicket();
}

async function kbReadTicket() {
  const open = kbv.open;
  if (!open) return;
  const card = kbCards(kbv.board).get(open.key) || open.card;
  open.card = card;
  if (card.n == null) { kbPaintPanel(); return; }
  const seq = ++kbv.seq;
  let res;
  try { res = await (await fetch(kbTicketUrl(card.sandbox, card.n))).json(); } catch (e) { res = { ok: false, error: e.message }; }
  if (kbv.open !== open || seq !== kbv.seq) return;
  if (!res?.ok || !res.ticket) {
    showNote(kbNote, `Could not read ${card.num}: ${res?.error || 'no answer'}`);
    kbPaintPanel();
    return;
  }
  open.t = res.ticket;
  kbPaintPanel();
}

function kbLinks(card) {
  if (!card.thread) return {};
  const plan = typeof pendingPlanFor === 'function'
    && ((pendingPlanFor(card.sandbox)?.id === card.plan) || (mergePlanFor(card.sandbox)?.id === card.plan));
  return { terminal: true, plan: !!plan, worktrees: true };
}

function kbPaintPanel() {
  const open = kbv.open;
  if (!open) return;
  const top = kbPanel.scrollTop;
  setHtml(kbPanel, kbPanelHtml(open.card, open.t, {
    column: kbColumnTitle(kbv.board, open.card.column), all: kbv.scope === 'all', links: kbLinks(open.card),
  }));
  kbPanel.scrollTop = top;
  kbPanel.setAttribute('aria-label', `Ticket ${open.t?.num || open.card.num}`);
  kbLoadPics();
}

// A picture is read as a blob and shown from an object URL: never navigated
// to, and the daemon serves it as an attachment with a sandboxing CSP anyway.
function kbLoadPics() {
  const open = kbv.open;
  kbPanel.querySelectorAll('img[data-kb-pic]').forEach(async img => {
    const id = img.dataset.kbPic;
    if (kbv.pics.has(id)) { img.src = kbv.pics.get(id); return; }
    kbv.pics.set(id, '');
    try {
      const r = await fetch(kbFileUrl(open.card.sandbox, open.card.n, id));
      if (!r.ok) throw new Error(String(r.status));
      const blob = await r.blob();
      if (!blob.type.startsWith('image/')) throw new Error('not a picture');
      if (kbv.open !== open) return;
      const url = URL.createObjectURL(blob);
      kbv.pics.set(id, url);
      kbPanel.querySelectorAll('img[data-kb-pic]').forEach(i => { if (i.dataset.kbPic === id) i.src = url; });
    } catch {
      kbv.pics.delete(id);
      img.hidden = true;
    }
  });
}

function kbRevokePics() {
  for (const u of kbv.pics.values()) if (u) URL.revokeObjectURL(u);
  kbv.pics.clear();
}

// Closes the panel and gives the keyboard back to its card.
function kbCloseTicket(refocus = true) {
  const key = kbv.open?.key;
  kbv.open = null;
  kbRevokePics();
  kbPanel.hidden = true;
  setHtml(kbPanel, '');
  kbBody.querySelectorAll('.kb-card.kb-on').forEach(el => el.classList.remove('kb-on'));
  if (refocus && key) kbCardEl(key)?.focus();
}

function kbGo(what) {
  const card = kbv.open?.card;
  if (!card?.thread) return;
  closeKanban();
  if (what === 'terminal') reachThread(card.sandbox, card.thread, focusedPane);
  else if (what === 'plan') {
    const p = pendingPlanFor(card.sandbox);
    if (p?.id === card.plan) showPlan(p); else openMergePlan(card.sandbox);
  } else if (what === 'worktrees') openWorktrees(card.sandbox, focusedPane);
}

// Escape and a press outside: the ticket first, then the Kanban.
wireModal(kbOverlay, () => { if (kbv.open) kbCloseTicket(); else closeKanban(); });
kbOverlay.addEventListener('pointerdown', e => {
  if (!kbv.open || kbPanel.contains(e.target) || e.target.closest('[data-kb-card]')) return;
  if (!kbOverlay.contains(e.target) || e.target === kbOverlay) return; // the backdrop: wireModal's
  kbCloseTicket();
});
kbBody.addEventListener('click', e => {
  const card = e.target.closest('[data-kb-card]');
  if (card) { kbOpenTicket(card.dataset.kbCard); return; }
  if (e.target.closest('[data-kb-older]')) { kbv.older = true; kbRead(); }
});
kbPanel.addEventListener('click', e => {
  if (e.target.closest('[data-kb-close]')) { kbCloseTicket(); return; }
  const go = e.target.closest('[data-kb-go]');
  if (go) kbGo(go.dataset.kbGo);
});
kbScope.addEventListener('change', () => {
  kbv.scope = kbScope.value;
  kbv.older = false;
  kbv.board = null;
  kbCloseTicket(false);
  setHtml(kbBody, '<p class="wt-note">Reading the tickets…</p>');
  kbRead();
});
document.getElementById('kb-x').addEventListener('click', closeKanban);
document.getElementById('kb-new').addEventListener('click', () => openKanbanForm());
document.getElementById('btn-kanban')?.addEventListener('click', () => openKanban());
// From the worktree graph (or the shared plan's board) of a sandbox.
document.getElementById('wt-kanban')?.addEventListener('click', () => {
  const sandbox = wtView.sandbox;
  closeWorktrees();
  openKanban(sandbox);
});

// Live: a ticket changed, or a plan its cards follow. Read again once the
// burst settles; another sandbox's tickets do not concern a sandbox's board.
function kbSoon() {
  if (!kbOverlay.open) return;
  clearTimeout(kbv.timer);
  kbv.timer = setTimeout(kbRead, 300);
}
sbxwStream.addEventListener('tickets', ev => {
  let d = {};
  try { d = JSON.parse(ev.data) || {}; } catch { /* a nudge all the same */ }
  if (kbv.scope !== 'all' && d.sandbox && d.sandbox !== kbv.scope) return;
  kbSoon();
});
sbxwStream.addEventListener('plan', kbSoon);

// ── New ticket ──
const kbForm = document.getElementById('kb-form');
const kbfTitle = document.getElementById('kbf-title');
const kbfDesc = document.getElementById('kbf-desc');
const kbfPrio = document.getElementById('kbf-prio');
const kbfSandbox = document.getElementById('kbf-sandbox');
const kbfFiles = document.getElementById('kbf-files');
const kbfInput = document.getElementById('kbf-input');
const kbfAck = document.getElementById('kbf-ack');
const kbfAckRow = document.getElementById('kbf-ack-row');
const kbfAsk = document.getElementById('kbf-ask');
const kbfNotice = document.getElementById('kbf-notice');
const kbfError = document.getElementById('kbf-error');
const kbfSave = document.getElementById('kbf-save');
const kbfCancel = document.getElementById('kbf-cancel');
const kbfLimits = document.getElementById('kbf-limits');
// files: [{file, name, size, state, error}]; made: the ticket once created
// ({sandbox, n, num}), so a retry attaches to it and never makes a second.
const kbf = { files: [], made: null, busy: false };

const kbLimits = () => kbv.board?.limits || { title: 160, description: 8000, file_bytes: 20 << 20, files: 20, ticket_bytes: 100 << 20 };

function openKanbanForm() {
  const l = kbLimits();
  Object.assign(kbf, { files: [], made: null, busy: false });
  kbfTitle.value = '';
  kbfDesc.value = '';
  kbfPrio.value = 'none';
  kbfTitle.maxLength = l.title;
  kbfDesc.maxLength = l.description;
  kbfSandbox.innerHTML = sandboxes.map(s => `<option value="${escHtml(s.name)}">${escHtml(s.name)}</option>`).join('');
  kbfSandbox.value = kbv.scope !== 'all' ? kbv.scope : (panes[focusedPane]?.sandbox || sandboxes[0]?.name || '');
  kbfLimits.textContent = `Title up to ${l.title} characters, description up to ${l.description}. Files: ${l.files} at most, ${kbSize(l.file_bytes)} each, ${kbSize(l.ticket_bytes)} in all; archives and programs are refused.`;
  kbfAck.checked = false;
  showNote(kbfNotice, '');
  showNote(kbfError, '');
  kbFormPaint();
  modalOpen(kbForm);
  kbfTitle.focus();
}

function kbFormPaint() {
  const locked = !!kbf.made;
  [kbfTitle, kbfDesc, kbfPrio, kbfSandbox].forEach(el => { el.disabled = locked || kbf.busy; });
  kbfInput.disabled = kbf.busy;
  setHtml(kbfFiles, kbFormFilesHtml(kbf.files, kbf.busy));
  document.getElementById('kbf-count').textContent = kbf.files.length
    ? `${kbf.files.length} file${kbf.files.length === 1 ? '' : 's'} · ${kbSize(kbf.files.reduce((s, f) => s + f.size, 0))}` : '';
  const ask = kbAskReasons(kbf.files.filter(f => f.state !== 'done'));
  kbfAckRow.hidden = !ask.length;
  kbfAsk.textContent = ask.length ? `Look first: ${ask.join('; ')}.` : '';
  kbfSave.disabled = kbf.busy;
  kbfSave.textContent = kbf.busy ? 'Saving…' : kbf.made ? 'Attach the rest' : 'Save';
  kbfCancel.textContent = kbf.made ? `Close, keep ${kbf.made.num}` : 'Cancel';
  kbfCancel.disabled = kbf.busy;
}

async function kbAddFiles(list) {
  if (kbf.busy) return;
  const incoming = [];
  for (const file of list) {
    let head = new Uint8Array(0);
    try { head = new Uint8Array(await file.slice(0, KB_SNIFF_BYTES).arrayBuffer()); } catch { /* unreadable: said by the check */ }
    incoming.push({ file, name: file.name, size: file.size, head });
  }
  const held = kbf.files.filter(f => f.state !== 'failed');
  const { take, refused } = kbCheckFiles(held, incoming, kbLimits());
  kbf.files.push(...take.map(f => ({ file: f.file, name: f.name, size: f.size, state: 'wait', error: '' })));
  kbfAck.checked = false; // a new list is a new look
  showNote(kbfNotice, refused.length ? `Not added: ${refused.map(r => `${r.name} (${r.why})`).join('; ')}.` : '');
  kbFormPaint();
}

async function kbSave() {
  if (kbf.busy) return;
  const l = kbLimits();
  const draft = { sandbox: kbf.made?.sandbox || kbfSandbox.value, title: kbfTitle.value, description: kbfDesc.value };
  const problem = kbf.made ? '' : kbFormProblem(draft, l);
  if (problem) { showNote(kbfError, problem); return; }
  const pending = kbf.files.filter(f => f.state !== 'done');
  if (kbAskReasons(pending).length && !kbfAck.checked) {
    showNote(kbfError, 'Tick "I have looked at these files and want to attach them" first: nothing is sent before.');
    kbfAck.focus();
    return;
  }
  showNote(kbfError, '');
  kbf.busy = true;
  kbFormPaint();
  if (!kbf.made) {
    const res = await postJsonOrError(`/api/tickets/${encodeURIComponent(draft.sandbox)}`,
      { title: draft.title.trim(), description: draft.description, priority: kbfPrio.value });
    if (!res.ok || !res.ticket) {
      kbf.busy = false;
      kbFormPaint();
      showNote(kbfError, `The ticket was not created: ${res.error || 'no answer'}.`);
      return;
    }
    kbf.made = { sandbox: draft.sandbox, n: res.ticket.n, num: res.ticket.num || `T-${res.ticket.n}` };
  }
  const todo = kbf.files.filter(f => f.state !== 'done');
  let stopped = false;
  for (let i = 0; i < todo.length; i++) {
    const f = todo[i];
    if (stopped) { f.state = 'wait'; f.error = ''; continue; }
    f.state = 'sending';
    f.error = '';
    showNote(kbfNotice, `Attaching ${i + 1} of ${todo.length}…`);
    kbFormPaint();
    let res;
    try {
      const r = await fetch(`${kbTicketUrl(kbf.made.sandbox, kbf.made.n)}/files?name=${encodeURIComponent(f.name)}`,
        { method: 'POST', headers: { 'Content-Type': 'application/octet-stream' }, body: f.file });
      res = await r.json().catch(() => ({ ok: false, error: r.status === 413 ? 'too large for the daemon' : `HTTP ${r.status}` }));
    } catch (e) {
      res = { ok: false, error: e.message };
    }
    if (res.ok) f.state = 'done';
    else { f.state = 'failed'; f.error = res.error || 'refused'; stopped = true; }
  }
  kbf.busy = false;
  showNote(kbfNotice, '');
  kbSoon();
  if (!stopped) {
    const n = kbf.files.length;
    kbForm.close();
    showToast(`${kbf.made.num} created${n ? ` with ${n} file${n === 1 ? '' : 's'}` : ''}`, 'ok');
    kbf.made = null;
    kbf.files = [];
    return;
  }
  kbFormPaint();
  showNote(kbfError, kbSaveReport(kbf.made.num, kbf.files));
}

// Escape closes an empty form; one with text in it closes only by its
// buttons, so a stray key does not drop what was typed.
function kbFormCancel(byButton) {
  if (kbf.busy) return;
  const typed = !kbf.made && (kbfTitle.value.trim() || kbfDesc.value.trim() || kbf.files.length);
  if (typed && !byButton) { showNote(kbfError, 'Press Cancel to drop this ticket.'); return; }
  kbForm.close();
  kbf.files = [];
  kbf.made = null;
  kbfInput.value = '';
}
kbForm.addEventListener('cancel', e => { e.preventDefault(); kbFormCancel(false); });
kbfCancel.addEventListener('click', () => kbFormCancel(true));
document.getElementById('kbf-x').addEventListener('click', () => kbFormCancel(true));
kbfSave.addEventListener('click', kbSave);
kbfInput.addEventListener('change', () => { kbAddFiles([...kbfInput.files]); kbfInput.value = ''; });
kbfFiles.addEventListener('click', e => {
  const rm = e.target.closest('[data-kb-rm]');
  if (!rm || kbf.busy) return;
  kbf.files.splice(Number(rm.dataset.kbRm), 1);
  kbfAck.checked = false;
  kbFormPaint();
});
kbfAck.addEventListener('change', () => { if (kbfAck.checked) showNote(kbfError, ''); });
// Files dropped anywhere on the form.
let kbDrag = 0;
kbForm.addEventListener('dragenter', e => { e.preventDefault(); kbDrag++; kbForm.classList.add('kb-drag'); });
kbForm.addEventListener('dragover', e => { e.preventDefault(); e.dataTransfer.dropEffect = 'copy'; });
kbForm.addEventListener('dragleave', () => { if (--kbDrag <= 0) { kbDrag = 0; kbForm.classList.remove('kb-drag'); } });
kbForm.addEventListener('drop', e => {
  e.preventDefault();
  kbDrag = 0;
  kbForm.classList.remove('kb-drag');
  if (e.dataTransfer.files.length) kbAddFiles([...e.dataTransfer.files]);
});
