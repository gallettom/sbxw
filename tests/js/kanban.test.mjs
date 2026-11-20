// The web Kanban's pure part (assets/js/kanban.js, between its BEGIN and END
// markers), without a browser: the board and the ticket drawn from real
// answers of the daemon's routes (desktop/fixtures/kanban/*.json, written by
// sbxw_core::tickets itself, see `write_kanban_fixtures`), the form's checks,
// and text kept as text.
// Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const read = p => readFileSync(new URL(`../../${p}`, import.meta.url), 'utf8');
const src = read('assets/js/kanban.js');
const pure = src.slice(src.indexOf('// BEGIN kanban pure'), src.indexOf('// END kanban pure'));
const util = read('assets/js/util.js');
const esc = util.slice(util.indexOf('function escHtml'), util.indexOf('// True for an Enter'));
const basic = JSON.parse(read('desktop/fixtures/kanban/basic.json')).daemon;
const big = JSON.parse(read('desktop/fixtures/kanban/big.json')).daemon;

function load() {
  const ctx = vm.createContext({});
  vm.runInContext(`${esc}\n${pure}\nglobalThis.api = { escHtml, kbCardHtml, kbColumnHtml, kbBoardHtml, kbPanelHtml, kbCards,
    kbColumnTitle, kbSniff, kbSecretName, kbCheckFiles, kbAskReasons, kbFormProblem, kbFormFilesHtml, kbSaveReport,
    kbFileUrl, kbBoardUrl, kbTicketUrl, KB_SECRET_NAMES };`, ctx);
  return ctx.api;
}
const ctx = load();
// Values from the script's own realm, as plain data for deepEqual.
const plain = x => JSON.parse(JSON.stringify(x));
const kb = new Proxy(ctx, { get: (o, k) => (typeof o[k] === 'function' ? (...a) => { const r = o[k](...a); return r && typeof r === 'object' && r.constructor?.name !== 'Map' ? plain(r) : r; } : o[k]) });

// The page's markup as a list of tags, each with its attribute names, and the
// text between them. Throws if any text could open or close a tag: escHtml
// leaves no `<`, `>` or quote in text, and every attribute value is quoted.
function tags(html) {
  const re = /<(\/?)([a-zA-Z][a-zA-Z0-9]*)((?:\s+[a-zA-Z-]+(?:="[^"<>]*")?)*)\s*>/g;
  const out = [];
  let last = 0;
  for (const m of html.matchAll(re)) {
    const text = html.slice(last, m.index);
    assert.ok(!/[<>"]/.test(text), `raw markup in text: ${JSON.stringify(text.slice(0, 80))}`);
    out.push(`${m[1]}${m[2].toLowerCase()}(${[...m[3].matchAll(/\s([a-zA-Z-]+)(?:="[^"]*")?/g)].map(a => a[1]).join(',')})`);
    last = m.index + m[0].length;
  }
  assert.ok(!/[<>"]/.test(html.slice(last)), 'raw markup after the last tag');
  return out;
}
const names = list => list.map(t => t.replace(/\(.*$/, ''));

test('the board is the daemon\'s five columns, in its order, with its counts and empty sentences', () => {
  for (const [key, board] of Object.entries(basic.tickets_boards)) {
    const html = kb.kbBoardHtml(board, { all: key.startsWith('all') });
    tags(html);
    const ids = [...html.matchAll(/data-kb-col="([^"]+)"/g)].map(m => m[1]);
    assert.deepEqual(ids, board.columns.map(c => c.id), key);
    assert.deepEqual(ids, ['planned', 'progress', 'test', 'ready', 'merged']);
    const counts = [...html.matchAll(/class="kb-count">(\d+)</g)].map(m => +m[1]);
    assert.deepEqual(counts, board.columns.map(c => c.count), `${key}: counts`);
    for (const c of board.columns) if (!c.cards.length) assert.ok(html.includes(ctx.escHtml(c.empty)), `${key}: ${c.id} says it is empty`);
  }
});

test('every card is drawn in the column the daemon put it in, in its order', () => {
  const board = basic.tickets_boards['web-app'];
  const html = kb.kbBoardHtml(board, {});
  for (const col of board.columns) {
    const part = html.slice(html.indexOf(`data-kb-col="${col.id}"`)).split('<section')[0];
    const keys = [...part.matchAll(/data-kb-card="([^"]+)"/g)].map(m => m[1]);
    assert.deepEqual(keys, col.cards.map(c => c.key), col.id);
  }
  assert.equal([...html.matchAll(/data-kb-card=/g)].length, board.columns.reduce((s, c) => s + c.cards.length, 0));
});

test('a card shows its number or "thread", its title, marks, thread and age; the sandbox only in All', () => {
  const board = basic.tickets_boards['web-app'];
  const cards = kb.kbCards(board);
  const failed = [...cards.values()].find(c => c.n == null);
  const html = kb.kbCardHtml(failed, {});
  assert.ok(html.includes('>thread<') && html.includes('kb-num-thread'));
  assert.ok(html.includes(failed.title_long) && html.includes(failed.title_short));
  for (const m of failed.marks) {
    assert.ok(html.includes(`>${m.label}<`), m.label);
    assert.ok(html.includes(`title="${m.help}"`), 'its help as a tooltip');
  }
  assert.ok(html.includes('kb-red'), 'a failed thread has the red edge');
  assert.ok(html.includes(`kb-dot kb-tone-${failed.thread_tone}`) && html.includes(failed.thread));
  assert.ok(html.includes(`>${failed.age}<`));
  assert.ok(!html.includes('kb-sb'), 'no sandbox name on a sandbox\'s own board');
  const all = kb.kbCardHtml(failed, { all: true });
  assert.ok(all.includes(`<span class="kb-sb">${failed.sandbox}</span>`));
  // Only failed, merge_failed and refused give the red edge.
  for (const c of cards.values()) {
    const red = c.marks.some(m => ['failed', 'merge_failed', 'refused'].includes(m.id));
    assert.equal(kb.kbCardHtml(c).includes('kb-red'), red, c.key);
  }
});

test('a card carries the daemon\'s short and long titles, as it cut them; the column\'s width picks', () => {
  for (const c of kb.kbCards(big.tickets_boards.all).values()) {
    const html = kb.kbCardHtml(c, {});
    const shown = cls => html.match(new RegExp(`class="kb-card-title ${cls}">([^<]*)<`))[1];
    assert.equal(shown('kb-t-short'), ctx.escHtml(c.title_short));
    assert.equal(shown('kb-t-long'), ctx.escHtml(c.title_long));
  }
  const css = read('assets/app.css');
  assert.match(css, /@container \(min-width: 250px\) \{\s*\.kb-t-short \{ display: none; \}/);
});

test('Merged at real size: 12 drawn, the count says all, and "Show 16 older"', () => {
  const board = big.tickets_boards.all;
  const merged = board.columns.find(c => c.id === 'merged');
  const html = kb.kbColumnHtml(merged, {});
  assert.equal([...html.matchAll(/data-kb-card=/g)].length, 12);
  assert.ok(html.includes(`class="kb-count">${merged.count}<`));
  assert.ok(html.includes(`Show ${merged.older} older`) && html.includes('data-kb-older'));
  const older = big.tickets_boards['all|older'].columns.find(c => c.id === 'merged');
  assert.ok(!kb.kbColumnHtml(older, {}).includes('data-kb-older'));
});

test('an open ticket shows its text, files as chips with name and size, pictures by id, and its history', () => {
  const t = basic.tickets_views['web-app/5'];
  const html = kb.kbPanelHtml(t.card, t, { column: 'In progress', links: { terminal: true, plan: true, worktrees: true } });
  tags(html);
  assert.ok(html.includes(t.title));
  assert.ok(html.includes(`class="kb-p-desc">${t.description.replace(/'/g, '&#39;')}<`), 'line breaks kept as they are');
  for (const f of t.files) {
    assert.ok(html.includes(`href="/api/tickets/web-app/5/files/${f.id}"`), `${f.name}: linked by its id`);
    assert.ok(html.includes(`>${f.name}<`) && html.includes(`>${f.size}<`));
  }
  const pics = [...html.matchAll(/data-kb-pic="([^"]+)"/g)].map(m => m[1]);
  assert.deepEqual(pics, t.files.filter(f => f.picture).map(f => f.id));
  assert.ok(!/<img[^>]*\ssrc=/.test(html), 'a picture gets its source from a blob, later');
  for (const h of t.history) assert.ok(html.includes(h.text));
  assert.ok(html.includes('data-kb-go="terminal"') && html.includes('data-kb-go="plan"') && html.includes('data-kb-go="worktrees"'));
  assert.ok(html.includes('copied into web-app when an agent reads this ticket'));
});

test('what the web view does not do is absent, not disabled; Send is pointed at the desktop', () => {
  for (const [key, t] of Object.entries(basic.tickets_views)) {
    const html = kb.kbPanelHtml(t.card, t, {});
    for (const word of ['Delete', 'Move up', 'Move down', 'To the top', 'Validated on my machine', 'Edit']) {
      assert.ok(!new RegExp(`<button[^>]*>[^<]*${word}`).test(html), `${key}: no ${word} button`);
    }
    assert.ok(!/disabled/.test(html), `${key}: no disabled control`);
    assert.ok(!/data-kb-(send|delete|move|edit|remove)/.test(html));
    assert.equal(html.includes('Send it to the lead from the desktop app'), !!t.card.can.send, key);
  }
});

test('a thread with no ticket opens its card only', () => {
  const card = [...kb.kbCards(basic.tickets_boards['web-app']).values()].find(c => c.n == null);
  const html = kb.kbPanelHtml(card, null, { links: { terminal: true } });
  tags(html);
  assert.ok(html.includes(card.title) && html.includes(card.can.send_why));
  assert.ok(!html.includes('kb-p-desc') && !html.includes('kb-file'));
});

test('a ticket not read yet (or whose read failed) is never called a thread', () => {
  const t = basic.tickets_views['web-app/5'];
  const html = kb.kbPanelHtml(t.card, null, {});
  assert.ok(html.includes('Reading the ticket') && !html.includes('A thread, not a ticket'));
});

// ── Text stays text ──
const HOSTILE = `<script>alert(1)</script><img src=x onerror=alert(2)>"'><a href="javascript:alert(3)">x</a>&amp;`;

function hostileCard(card) {
  return { ...card, key: `${card.key}"><b>`, num: HOSTILE, sandbox: HOSTILE, title: HOSTILE, title_short: HOSTILE, title_long: HOSTILE,
    age: HOSTILE, thread: card.thread ? HOSTILE : null, thread_tone: '"><x', marks: card.marks.map(m => ({ ...m, label: HOSTILE, help: HOSTILE, tone: 'red" onclick="x' })) };
}

test('a title, a description and a file name full of markup add no element and no attribute', () => {
  const board = basic.tickets_boards.all;
  for (const c of kb.kbCards(board).values()) {
    for (const o of [{ all: true }, {}]) {
      assert.deepEqual(tags(kb.kbCardHtml(hostileCard(c), o)), tags(kb.kbCardHtml(c, o)), c.key);
    }
  }
  const t = basic.tickets_views['web-app/5'];
  const bad = { ...t, num: HOSTILE, title: HOSTILE, description: `${HOSTILE}\n\n${HOSTILE}`, files_label: HOSTILE,
    files: t.files.map(f => ({ ...f, name: HOSTILE, size: HOSTILE, id: `${f.id}"><script>` })),
    history: t.history.map(h => ({ ...h, text: HOSTILE, when: HOSTILE, tone: '"x' })), card: hostileCard(t.card) };
  const o = { column: HOSTILE, all: true, links: { terminal: true, plan: true, worktrees: true } };
  const good = kb.kbPanelHtml({ ...t.card }, t, o);
  const evil = kb.kbPanelHtml(hostileCard(t.card), bad, o);
  assert.deepEqual(names(tags(evil)), names(tags(good)));
  assert.deepEqual(tags(evil), tags(good));
  // Only escaped: no tag of the text's, no attribute the text named.
  assert.ok(!/<script|<img src=x|<a href|<b>|href="javascript/i.test(evil));
  // The file's link is built from its id, encoded: no name, no markup.
  assert.ok(evil.includes(`href="/api/tickets/${encodeURIComponent(HOSTILE).replace(/'/g, '&#39;')}/5/files/a-00000003%22%3E%3Cscript%3E"`));
  const files = kb.kbFormFilesHtml([{ name: HOSTILE, size: 3, state: 'failed', error: HOSTILE }], false);
  assert.deepEqual(names(tags(files)), names(tags(kb.kbFormFilesHtml([{ name: 'a', size: 3, state: 'failed', error: 'b' }], false))));
  assert.ok(!/<script|<img/i.test(files));
});

test('the routes are built from encoded names and numbers', () => {
  assert.equal(kb.kbBoardUrl('all', false), '/api/tickets?sandbox=all');
  assert.equal(kb.kbBoardUrl('web-app', true), '/api/tickets?sandbox=web-app&older=1');
  assert.equal(kb.kbBoardUrl('a&b=c', false), '/api/tickets?sandbox=a%26b%3Dc');
  assert.equal(kb.kbFileUrl('web/app', 5, 'a-1/../x'), '/api/tickets/web%2Fapp/5/files/a-1%2F..%2Fx');
});

// ── The form's checks ──
const LIMITS = basic.tickets_boards['web-app'].limits;
const bytes = (...b) => new Uint8Array(b);
const ascii = s => new Uint8Array([...s].map(c => c.charCodeAt(0)));
const file = (name, size, head = ascii('hello')) => ({ name, size, head });

test('archives, programs and empty files are refused by their first bytes, as the daemon does', () => {
  const tar = new Uint8Array(300); tar.set(ascii('ustar'), 257);
  for (const [what, head] of [['zip', bytes(0x50, 0x4b, 3, 4)], ['empty zip', bytes(0x50, 0x4b, 5, 6)], ['gzip', bytes(0x1f, 0x8b, 8)],
    ['7z', bytes(0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c)], ['rar', bytes(...ascii('Rar!'), 0x1a, 7, 0)], ['xz', bytes(0xfd, ...ascii('7zXZ'), 0)],
    ['bzip2', ascii('BZh91')], ['zstd', bytes(0x28, 0xb5, 0x2f, 0xfd)], ['tar', tar]]) {
    assert.match(kb.kbSniff(head), /^archives/, what);
  }
  for (const [what, head] of [['elf', bytes(0x7f, ...ascii('ELF'))], ['pe', ascii('MZ\x90')], ['mach-o 64', bytes(0xcf, 0xfa, 0xed, 0xfe)],
    ['mach-o 32', bytes(0xce, 0xfa, 0xed, 0xfe)], ['fat', bytes(0xca, 0xfe, 0xba, 0xbe)]]) {
    assert.match(kb.kbSniff(head), /^programs/, what);
  }
  assert.equal(kb.kbSniff(bytes()), 'the file is empty');
  for (const head of [bytes(0x89, 0x50, 0x4e, 0x47, 13, 10, 26, 10), ascii('%PDF-1.7'), ascii('plain text'), bytes(0xff, 0xd8, 0xff), new Uint8Array(300)]) {
    assert.equal(kb.kbSniff(head), '');
  }
  // The daemon's own words, so the page and the daemon say the same.
  const core = read('crates/core/src/tickets.rs');
  assert.ok(core.includes(kb.kbSniff(bytes(0x1f, 0x8b))) && core.includes(kb.kbSniff(bytes(0x7f, ...ascii('ELF')))));
});

test('sizes and counts are checked against the daemon\'s limits before anything is sent', () => {
  const MB = 1 << 20;
  let r = kb.kbCheckFiles([], [file('big.png', LIMITS.file_bytes + 1), file('ok.png', MB)], LIMITS);
  assert.deepEqual(r.take.map(f => f.name), ['ok.png']);
  assert.match(r.refused[0].why, /over 20 MB/);
  const held = Array.from({ length: LIMITS.files - 1 }, (_, i) => ({ name: `f${i}`, size: 1 }));
  r = kb.kbCheckFiles(held, [file('a', 1), file('b', 1)], LIMITS);
  assert.deepEqual(r.take.map(f => f.name), ['a']);
  assert.match(r.refused[0].why, /20 files at most/);
  const heavy = Array.from({ length: 5 }, (_, i) => ({ name: `h${i}`, size: 19 * MB }));
  r = kb.kbCheckFiles(heavy, [file('c', 6 * MB), file('d', 4 * MB)], LIMITS);
  assert.deepEqual(r.take.map(f => f.name), ['d']);
  assert.match(r.refused[0].why, /100 MB in all/);
  r = kb.kbCheckFiles([], [file('x.zip', 10, bytes(0x50, 0x4b, 3, 4)), file('notes.txt', 10)], LIMITS);
  assert.deepEqual(r.take.map(f => f.name), ['notes.txt']);
  assert.equal(r.refused[0].name, 'x.zip');
});

test('a file named like a secret, many files or many bytes need the tick; the list is the desktop\'s', () => {
  const desktop = read('desktop/src/feat/files/drops.rs').match(/SECRET_NAMES: &\[&str\] = &\[([^\]]*)\]/)[1];
  assert.deepEqual([...kb.KB_SECRET_NAMES], [...desktop.matchAll(/"([^"]+)"/g)].map(m => m[1]));
  for (const n of ['.env', '.env.local', 'server.PEM', 'tls.key', 'id_rsa', 'id_rsa.pub', 'id_ed25519', 'cert.p12', 'a.pfx', 'credentials.json', '.npmrc', '.netrc', 'vault.kdbx']) {
    assert.ok(kb.kbSecretName(n), n);
  }
  for (const n of ['notes.txt', 'keyboard.png', 'my.env.txt', 'environment.md', 'pem.txt']) assert.ok(!kb.kbSecretName(n), n);
  assert.deepEqual([...kb.kbAskReasons([{ name: 'a.png', size: 10 }])], []);
  assert.match(kb.kbAskReasons([{ name: 'a.png', size: 1 }, { name: 'id_rsa', size: 10 }])[0], /^id_rsa: its name looks like a secret/);
  assert.match(kb.kbAskReasons(Array.from({ length: 11 }, (_, i) => ({ name: `f${i}`, size: 1 })))[0], /11 files/);
  assert.match(kb.kbAskReasons([{ name: 'a', size: 26 << 20 }])[0], /in all \(more than 25/);
  // The flagged file is marked in the list.
  assert.ok(kb.kbFormFilesHtml([{ name: '.env', size: 3 }], false).includes('look first'));
});

test('the form refuses an empty title or text over the caps, and says what a half-made save left', () => {
  const ok = { sandbox: 'web-app', title: 'x', description: '' };
  assert.equal(kb.kbFormProblem(ok, LIMITS), '');
  assert.match(kb.kbFormProblem({ ...ok, title: '   ' }, LIMITS), /needs a title/);
  assert.match(kb.kbFormProblem({ ...ok, sandbox: '' }, LIMITS), /sandbox/);
  assert.match(kb.kbFormProblem({ ...ok, title: 'x'.repeat(LIMITS.title + 1) }, LIMITS), /at most 160/);
  assert.equal(kb.kbFormProblem({ ...ok, title: 'é'.repeat(LIMITS.title) }, LIMITS), '');
  assert.match(kb.kbFormProblem({ ...ok, description: 'x'.repeat(LIMITS.description + 1) }, LIMITS), /at most 8000/);
  const said = kb.kbSaveReport('T-13', [{ name: 'a.png', state: 'done' }, { name: 'b.bin', state: 'failed', error: 'programs are refused' }, { name: 'c.pdf', state: 'wait' }]);
  assert.match(said, /^T-13 was created\. Attached: a\.png\. Not attached: b\.bin \(programs are refused\)\. Not sent: c\.pdf\./);
  assert.match(said, /Attach the rest/);
});

test('kanban.js declares no global another page script already declares', () => {
  // The page's scripts are classic scripts sharing one global scope: a
  // `const` declared twice stops the second file whole.
  const top = s => new Set([...s.matchAll(/^(?:const|let|var|function|async function|class) ([A-Za-z_$][\w$]*)/gm)].map(m => m[1]));
  const mine = top(src);
  const shell = read('assets/js/singleton.js');
  for (const f of [...shell.matchAll(/'\/js\/([\w-]+\.js)'/g)].map(m => m[1]).filter(f => f !== 'kanban.js')) {
    for (const n of top(read(`assets/js/${f}`))) assert.ok(!mine.has(n), `${n} is declared in ${f} too`);
  }
});
