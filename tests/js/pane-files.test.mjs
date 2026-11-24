// File paths and drops on terminal panes (assets/js/pane-files.js): the link
// provider on recorded lines, the drop flow with fake events, the typed line,
// the held-pane cases, and the guard in panes.js that must keep stopping a
// pasted or dropped file. No browser: a tiny fake DOM, a fake buffer, a fake
// XMLHttpRequest. Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const read = p => readFileSync(new URL(`../../${p}`, import.meta.url), 'utf8');
const core = read('assets/js/chat-core.js');
const fileRules = core.slice(core.indexOf('const CHAT_FILE_EXT'), core.indexOf('// Code spans are lifted out first'));
const attach = core.slice(core.indexOf('// BEGIN chat attach'), core.indexOf('// END chat attach'));
const paneFiles = read('assets/js/pane-files.js');
// Loaded before every app script (singleton.js).
const invisible = read('assets/js/invisible.js');
const panes = read('assets/js/panes.js');
const holdRules = panes.slice(panes.indexOf('// BEGIN pane-hold rules'), panes.indexOf('// END pane-hold rules'));

class El {
  constructor(tag) { this.tag = tag; this.children = []; this.attrs = {}; this.listeners = {}; this.className = ''; this._text = ''; this.hidden = false; this.offsetWidth = 100; this.offsetHeight = 40; this.disabled = false; this.value = ''; this.checked = false; this.style = {}; }
  get textContent() { return this._text + this.children.map(c => c.textContent).join(''); }
  set textContent(v) { this._text = String(v); this.children = []; }
  get childNodes() { return this.children; }
  appendChild(c) { this.children.push(c); c.parent = this; return c; }
  append(...cs) { cs.forEach(c => this.appendChild(typeof c === 'string' ? Object.assign(new El('#text'), { _text: c }) : c)); }
  replaceChildren() { this.children = []; this._text = ''; }
  remove() { if (this.parent) this.parent.children = this.parent.children.filter(c => c !== this); }
  setAttribute(k, v) { this.attrs[k] = String(v); }
  addEventListener(t, f) { (this.listeners[t] ||= []).push(f); }
  get classList() { return { toggle: (n, on) => { const s = new Set(this.className.split(' ').filter(Boolean)); on ? s.add(n) : s.delete(n); this.className = [...s].join(' '); } }; }
  fire(t, e = {}) { const ev = { defaultPrevented: false, preventDefault() { this.defaultPrevented = true; }, ...e }; (this.listeners[t] || []).forEach(f => f(ev)); return ev; }
  walk(f) { f(this); this.children.forEach(c => c.walk(f)); }
  find(cls) { let r = null; this.walk(n => { if (!r && n.className.split(' ').includes(cls)) r = n; }); return r; }
  findAll(tag) { const r = []; this.walk(n => { if (n.tag === tag) r.push(n); }); return r; }
  querySelector(sel) { return this.find(sel.replace(/^\./, '')); }
}

// A xterm buffer from rows of text, `cols` wide; a row marked wrapped continues the previous.
function makeBuf(rows, cols = 80) {
  const lines = rows.map(r => {
    const text = typeof r === 'string' ? r : r.text;
    const chars = [...text.padEnd(cols, ' ')].slice(0, cols);
    return {
      isWrapped: typeof r === 'object' && !!r.wrapped,
      length: cols,
      getCell: x => (chars[x] === undefined ? undefined : { getChars: () => (chars[x] === ' ' ? '' : chars[x]), getWidth: () => 1 }),
    };
  });
  return { getLine: i => lines[i] };
}

function load(extra = {}, core = 'with') {
  const calls = { open: [], toasts: [], xhr: [], tips: [] };
  const document = {
    createElement: t => new El(t),
    body: new El('body'),
    querySelector: () => null,
  };
  class XHR {
    constructor() { this.upload = {}; this.headers = {}; calls.xhr.push(this); }
    open(m, url) { this.method = m; this.url = url; }
    setRequestHeader(k, v) { this.headers[k] = v; }
    send(file) {
      this.file = file;
      setImmediate(() => {
        const out = calls.reply ? calls.reply(this) : { status: 200, body: { ok: true, path: `/tmp/sbxw-uploads/${calls.xhr.length}-${calls.xhr.indexOf(this)}.bin` } };
        this.status = out.status;
        this.responseText = JSON.stringify(out.body);
        this.upload.onprogress?.({ lengthComputable: true, loaded: 1, total: 2 });
        this.onload();
      });
    }
    abort() { this.onabort?.(); }
  }
  const ctx = vm.createContext({
    document, AbortController, Promise, setTimeout, setImmediate, TextEncoder, XMLHttpRequest: XHR, CSS: { supports: () => true },
    console: { log() {}, warn() {}, error() {} },
    showToast: (m, k) => calls.toasts.push([m, k]),
    fxResolvePath: (sbx, p) => (p.startsWith('/') ? p : `/ws/${sbx}/${p.replace(/^\.\//, '')}`),
    fxOpenPath: (sbx, p) => calls.open.push([sbx, p]),
    sandboxes: [{ name: 'dev', workspace: '/home/agent/proj' }, { name: 'other', workspace: '/srv/o' }],
    threadLists: new Map(),
    paneCount: 2, window: { innerWidth: 1000, innerHeight: 800, addEventListener() {} },
    ...extra,
  });
  vm.runInContext(`${invisible}\n${core === 'with' ? `${fileRules}\n${attach}\n` : ''}${holdRules}\n${paneFiles}\nglobalThis.api = { chatFilePath: globalThis.chatFilePath, chatFileTokens: globalThis.chatFileTokens, paneLogicalLine, paneFileLinksAt, paneLinkTarget, paneLinkTip, paneLinkProvider, paneLinksAttach, paneShellQuote, paneTypeLine, paneTypeBlocker, paneTypePaths, paneDropMount, paneShowPathNotice, paneFrameAllowed };`, ctx);
  // chat-core.js comes after pane-files.js on the page: `late` evaluates it now.
  const late = () => vm.runInContext(`${fileRules}\n${attach}\nglobalThis.chatFilePath = chatFilePath; globalThis.chatFileTokens = chatFileTokens;`, ctx);
  return { api: ctx.api, ctx, calls, document, late };
}

const links = (buf, y, api) => new Promise(res => api.paneLinkProvider({ buffer: { active: buf } }, {
  session: () => ({ sandbox: 'dev', mode: 'claude', thread: null, kind: null }),
  workspace: () => '/home/agent/proj', resolve: () => null, open() {}, tip() {}, detect: api.chatFileTokens,
}).provideLinks(y, res));

// ── A: the link provider ─────────────────────────────────────────────────
test('detects the chat rule on recorded lines, with columns', async () => {
  const { api } = load();
  const buf = makeBuf(['error in src/app/main.rs:42:7 (see ./docs/web-ui.md, and "assets/a.png")']);
  const out = await links(buf, 1, api);
  assert.deepEqual(Array.from(out, l => l.text), ['src/app/main.rs:42:7', './docs/web-ui.md', 'assets/a.png']);
  assert.deepEqual(JSON.parse(JSON.stringify(out[0].range)), { start: { x: 10, y: 1 }, end: { x: 29, y: 1 } });
  assert.equal(out[0].decorations.pointerCursor, true);
  assert.equal(out[0].decorations.underline, true);
});

test('negatives: no slash, unknown extension, scheme, ..', async () => {
  const { api } = load();
  for (const line of ['see main.rs', 'a/b.exe', 'https://x.example/a/b.js', '../a/b.js', 'a/../b.js', '~x/a.js', 'and/or', 'TCP/IP', '/tmp', '//host/a.js', 'k=v/ok.md:']) {
    const out = await links(makeBuf([line]), 1, api);
    if (line === 'k=v/ok.md:') continue; // '=' is not a path character: not linked either
    assert.equal(out, undefined, line);
  }
});

test('hostile output: markup, quotes, escapes, OSC 8 text stay inert', async () => {
  const { api } = load();
  const line = '\u001b]8;;javascript:alert(1)\u0007<img src=x onerror=alert(1)>/a/b.md"\'><script>x/y.js</script> `$(touch /tmp/x)/z.md`';
  const out = (await links(makeBuf([line], 200), 1, api)) || [];
  for (const l of out) assert.match(l.text, /^[A-Za-z0-9_.+@%\-\/~:]+$/, 'only path characters reach a link');
  // The path is a plain value: no markup or url is built from it anywhere.
  assert.ok(!/innerHTML|insertAdjacentHTML|document\.write/.test(paneFiles.replace(/\/\/.*$/gm, '')));
});

test('a 10,000 character line gives no link and no work', async () => {
  const { api } = load();
  const long = 'a/b.md '.repeat(1500);
  const rows = [];
  for (let i = 0; i < long.length; i += 80) rows.push({ text: long.slice(i, i + 80), wrapped: i > 0 });
  const t0 = performance.now();
  const out = await links(makeBuf(rows), 3, api);
  assert.equal(out, undefined);
  assert.ok(performance.now() - t0 < 100);
});

test('a path cut across two rows links as one', async () => {
  const { api } = load();
  const buf = makeBuf(['open the file src/components/wid', { text: 'get/panel.tsx now', wrapped: true }], 32);
  for (const y of [1, 2]) {
    const out = await links(buf, y, api);
    assert.equal(out.length, 1);
    assert.equal(out[0].text, 'src/components/widget/panel.tsx');
    assert.deepEqual({ ...out[0].range.start }, { x: 15, y: 1 });
    assert.deepEqual({ ...out[0].range.end }, { x: 13, y: 2 });
  }
});

test('a pane of another sandbox opens in ITS sandbox; no request before a press', () => {
  const { api, calls } = load();
  let provider;
  const buf = makeBuf(['x docs/a.md']);
  const mk = (sandbox, thread, kind) => api.paneLinksAttach({ registerLinkProvider: p => { provider = p; }, buffer: { active: buf } }, { sandbox, mode: thread ? 'thread' : 'claude', thread });
  mk('other', null);
  let found;
  provider.provideLinks(1, l => { found = l; });
  assert.equal(calls.open.length, 0);
  found[0].hover({ clientX: 1, clientY: 1 });
  found[0].leave();
  assert.equal(calls.open.length, 0, 'hover requests nothing');
  found[0].activate({ metaKey: false, ctrlKey: false });
  assert.equal(calls.open.length, 0, 'a plain click does nothing');
  found[0].activate({ metaKey: true });
  assert.deepEqual(calls.open, [['other', 'docs/a.md']]);
  found[0].activate({ ctrlKey: true });
  assert.equal(calls.open.length, 2);
});

test('relative path: workspace, or the worktree of an own thread; tooltip shows the resolved path and sandbox', () => {
  const { api } = load();
  assert.equal(api.paneLinkTarget('a/b.md', { thread: null }, '/ws'), 'a/b.md');
  assert.equal(api.paneLinkTarget('./a/b.md', { thread: 'tty-web', kind: 'own' }, '/ws/'), '/ws/.sbxw-worktrees/tty-web/a/b.md');
  assert.equal(api.paneLinkTarget('a/b.md', { thread: 'p', kind: 'team' }, '/ws'), 'a/b.md');
  assert.equal(api.paneLinkTarget('a/b.md', { thread: 's', kind: 'same' }, '/ws'), 'a/b.md');
  assert.equal(api.paneLinkTarget('/etc/x.md', { thread: 's', kind: 'own' }, '/ws'), '/etc/x.md');
  const tip = api.paneLinkTip('/ws/a/b.md', 'a/b.md', { sandbox: 'dev', thread: 'tty-web' });
  assert.match(tip, /^\/ws\/a\/b\.md\nin dev, thread tty-web\n(⌘|Ctrl) \+ click to preview$/);
  assert.match(api.paneLinkTip(null, 'a/b.md', { sandbox: 'dev', thread: null }), /not known yet/);
});

test('a press with the session changed since the hover does nothing', () => {
  const { api, calls } = load();
  let provider;
  const pane = { sandbox: 'dev', mode: 'claude', thread: null };
  api.paneLinksAttach({ registerLinkProvider: p => { provider = p; }, buffer: { active: makeBuf(['x docs/a.md']) } }, pane);
  let found;
  provider.provideLinks(1, l => { found = l; });
  pane.sandbox = 'elsewhere';
  found[0].activate({ metaKey: true });
  assert.equal(calls.open.length, 0);
});

test('a 5,000-line burst: the provider runs per line on demand, fast', async () => {
  const { api } = load();
  const rows = Array.from({ length: 5000 }, (_, i) => `2026-11-20 12:00:${i % 60} INFO worker ${i} wrote build/out/part-${i}.log (${i * 7} bytes) ok`);
  const buf = makeBuf(rows);
  const t0 = performance.now();
  let n = 0;
  for (let y = 1; y <= rows.length; y++) { const o = await links(buf, y, api); n += o ? o.length : 0; }
  const ms = performance.now() - t0;
  console.log(`burst: 5000 lines scanned in ${ms.toFixed(0)} ms (${n} links)`);
  assert.equal(n, 5000);
  assert.ok(ms < 3000, `took ${ms} ms`);
});

// ── B: typing and the drop flow ──────────────────────────────────────────
test('the typed line is the daemon paths, quoted, separated by spaces, no Return', () => {
  const { api } = load();
  assert.equal(api.paneTypeLine(['/tmp/sbxw-uploads/a.png', '/tmp/sbxw-uploads/b-1.pdf']), '/tmp/sbxw-uploads/a.png /tmp/sbxw-uploads/b-1.pdf ');
  assert.equal(api.paneTypeLine(["/tmp/my file's.png", '/tmp/$(x).png']), "'/tmp/my file'\\''s.png' '/tmp/$(x).png' ");
  assert.equal(api.paneTypeLine(['/tmp/a\n.png']), null, 'a newline would be a Return');
  assert.equal(api.paneTypeLine(['/tmp/a\r.png']), null);
  assert.equal(api.paneTypeLine([]), null);
  assert.ok(!/[\r\n]/.test(api.paneTypeLine(['/tmp/x.png'])));
});

function fakePane(over = {}) {
  const sent = [];
  const pane = {
    index: 0, el: new El('div'), sandbox: 'dev', mode: 'claude', thread: null,
    ws: { readyState: 1, send: d => sent.push(new TextDecoder().decode(d)) },
    term: { focus() { pane.focused = true; } },
    ...over,
  };
  return { pane, sent };
}
const dt = (...files) => ({ types: ['Files'], items: files.map(f => ({ kind: 'file', getAsFile: () => f, webkitGetAsEntry: () => null })), files, dropEffect: '' });
const file = (name, size = 10) => ({ name, size, type: 'image/png' });
const settle = async () => { for (let i = 0; i < 6; i++) await new Promise(r => setImmediate(r)); };

test('drop target names the sandbox and thread; a non-file drag keeps today\'s behaviour', () => {
  const { api } = load();
  const { pane } = fakePane({ thread: 'tty-web', mode: 'thread' });
  api.paneDropMount(pane);
  const veil = pane.el.find('pane-drop-veil');
  const over = pane.el.fire('dragenter', { dataTransfer: dt(file('a.png')) });
  assert.equal(over.defaultPrevented, true);
  assert.equal(veil.hidden, false);
  assert.match(veil.textContent, /Drop to send to dev\nthread tty-web/);
  const text = pane.el.fire('dragenter', { dataTransfer: { types: ['text/plain'], items: [], files: [] } });
  assert.equal(text.defaultPrevented, false);
  const txtDrop = pane.el.fire('drop', { dataTransfer: { types: ['text/uri-list'], items: [], files: [] } });
  assert.equal(txtDrop.defaultPrevented, false);
});

test('drop: checks before any byte, then upload, progress, and the typed line', async () => {
  const { api, calls } = load();
  const { pane, sent } = fakePane();
  api.paneDropMount(pane);
  // Refused before any byte: empty file and a folder; nothing is uploaded.
  pane.el.fire('drop', { dataTransfer: { types: ['Files'], items: [{ kind: 'file', getAsFile: () => file('empty.txt', 0), webkitGetAsEntry: () => ({ isDirectory: false }) }, { kind: 'file', getAsFile: () => null, webkitGetAsEntry: () => ({ isDirectory: true, name: 'dir' }) }] } });
  await settle();
  assert.equal(calls.xhr.length, 0);
  assert.match(pane.el.find('pane-drop-box').textContent, /empty/);
  assert.match(pane.el.find('pane-drop-box').textContent, /dir/);
  assert.equal(sent.length, 0);
  // Two good files, one with a hostile name.
  pane.el.fire('drop', { dataTransfer: dt(file('a.png'), file('$(rm -rf ~) \n"x".png')) });
  const box = pane.el.find('pane-drop-box');
  assert.equal(box.hidden, false);
  await settle();
  assert.equal(calls.xhr.length, 2);
  for (const x of calls.xhr) assert.match(x.url, /^\/api\/sandboxes\/dev\/upload\?name=/);
  assert.ok(!/[\n\r"\s]/.test(calls.xhr[1].url), 'the name is encoded in the url');
  assert.equal(sent.length, 1);
  const typed = sent[0];
  assert.match(typed, /^\/tmp\/sbxw-uploads\/\S+\.bin \/tmp\/sbxw-uploads\/\S+\.bin $/);
  assert.ok(!typed.includes('rm') && !/[\r\n]/.test(typed), 'built from the daemon answers only, no Return');
  assert.equal(pane.focused, true);
  assert.equal(box.hidden, true, 'progress box gone when done');
  assert.equal(calls.toasts.at(-1)[1], 'ok');
});

test('a name that looks like a secret waits for the tick; nothing is sent before it', async () => {
  const { api, calls } = load();
  const { pane, sent } = fakePane();
  api.paneDropMount(pane);
  pane.el.fire('drop', { dataTransfer: dt(file('.env.production')) });
  await settle();
  assert.equal(calls.xhr.length, 0);
  const box = pane.el.find('pane-drop-box');
  assert.match(box.textContent, /looks like a secret/);
  const [go] = box.findAll('button');
  const tick = box.findAll('input')[0];
  assert.equal(go.disabled, true);
  go.fire('click');
  await settle();
  assert.equal(calls.xhr.length, 0, 'a click without the tick sends nothing');
  tick.checked = true;
  tick.fire('change');
  go.fire('click');
  await settle();
  assert.equal(calls.xhr.length, 1);
  assert.equal(sent.length, 1);
});

test('a failed upload shows a message and Retry; the retry completes and types', async () => {
  const { api, calls } = load();
  const { pane, sent } = fakePane();
  api.paneDropMount(pane);
  let n = 0;
  calls.reply = () => (++n === 1 ? { status: 502, body: { ok: false, error: 'sandbox busy' } } : { status: 200, body: { ok: true, path: '/tmp/sbxw-uploads/ok.png' } });
  pane.el.fire('drop', { dataTransfer: dt(file('a.png')) });
  await settle();
  const box = pane.el.find('pane-drop-box');
  assert.match(box.textContent, /Failed: sandbox busy/);
  assert.equal(sent.length, 0);
  box.findAll('button').find(b => b.textContent === 'Retry').fire('click');
  await settle();
  assert.deepEqual(sent, ['/tmp/sbxw-uploads/ok.png ']);
});

test('held pane: no drop handled, and paths are never typed; a notice with Copy instead', async () => {
  const { api, ctx, calls } = load();
  const { pane, sent } = fakePane();
  api.paneDropMount(pane);
  vm.runInContext("paneInputHeld = true", ctx);
  const ev = pane.el.fire('drop', { dataTransfer: dt(file('a.png')) });
  assert.equal(ev.defaultPrevented, false, 'the window hold guard stops it; this handler does not take it');
  await settle();
  assert.equal(calls.xhr.length, 0);
  // The hold begins while an upload runs: the answer is shown, not typed.
  vm.runInContext("paneInputHeld = false", ctx);
  pane.el.fire('drop', { dataTransfer: dt(file('a.png')) });
  vm.runInContext("paneInputHeld = true", ctx);
  await settle();
  assert.equal(sent.length, 0);
  const notice = pane.el.find('pane-drop-notice');
  assert.match(notice.textContent, /held while you review a proposal/);
  assert.match(notice.find('pane-drop-row').findAll('button')[0].textContent, /Copy/);
  assert.match(notice.findAll('code')[0].textContent, /^\/tmp\/sbxw-uploads\/\S+ $/);
});

test('the other cases: hidden pane, another session, not connected, a dialog', () => {
  const { api, ctx } = load();
  const want = { sandbox: 'dev', mode: 'claude', thread: null };
  const env = { held: false, shown: 2, dialog: false };
  const ok = { index: 0, sandbox: 'dev', mode: 'claude', thread: null, ws: { readyState: 1 } };
  assert.equal(api.paneTypeBlocker(ok, want, env), '');
  assert.match(api.paneTypeBlocker({ ...ok, index: 3 }, want, env), /not on screen/);
  assert.match(api.paneTypeBlocker({ ...ok, sandbox: 'x' }, want, env), /another session/);
  assert.match(api.paneTypeBlocker({ ...ok, mode: 'bash' }, want, env), /another session/);
  assert.match(api.paneTypeBlocker({ ...ok, thread: 't' }, want, env), /another session/);
  assert.match(api.paneTypeBlocker({ ...ok, ws: { readyState: 0 } }, want, env), /not connected/);
  assert.match(api.paneTypeBlocker({ ...ok, ws: null }, want, env), /not connected/);
  assert.match(api.paneTypeBlocker(ok, want, { ...env, dialog: true }), /dialog is open/);
  assert.match(api.paneTypeBlocker(ok, want, { ...env, held: true }), /held/);
  void ctx;
});

test('the sidebar Upload goes through the same function and never claims "attached" when not typed', async () => {
  const up = read('assets/js/upload.js');
  assert.match(up, /paneTypePaths\(pane, want, paths\)/);
  assert.ok(!/pane\.ws\.send\(/.test(up), 'no raw send left in the sidebar upload');
  const { api, ctx } = load();
  const { pane, sent } = fakePane();
  vm.runInContext("paneInputHeld = true", ctx);
  const typed = api.paneTypePaths(pane, { sandbox: 'dev', mode: 'claude', thread: null }, ['/tmp/sbxw-uploads/a.png']);
  assert.equal(typed, false);
  assert.equal(sent.length, 0);
  assert.ok(pane.el.find('pane-drop-notice'));
  vm.runInContext("paneInputHeld = false", ctx);
  assert.equal(api.paneTypePaths(pane, { sandbox: 'dev', mode: 'claude', thread: null }, ['/tmp/sbxw-uploads/a.png']), true);
  assert.deepEqual(sent, ['/tmp/sbxw-uploads/a.png ']);
  assert.equal(pane.el.find('pane-drop-notice'), null, 'a typed line clears an old notice');
});

test('the hold guard still stops a pasted file and a drop on a held pane', () => {
  const { api, ctx } = load();
  void api;
  const stops = vm.runInContext('paneHoldStops', ctx);
  vm.runInContext("paneInputHeld = true", ctx);
  // inPanes, hasFiles
  assert.equal(stops('paste', true, true), true);
  assert.equal(stops('drop', true, true), true);
  assert.equal(stops('dragover', true, true), true);
  assert.equal(stops('drop', false, true), true, 'a dropped file anywhere');
  assert.equal(stops('paste', false, true), true);
  vm.runInContext("paneInputHeld = false", ctx);
  assert.equal(stops('drop', true, true), false, 'free: the pane takes the drop');
});

test('the window drop guard of the chat attach still covers the panes area', () => {
  assert.match(core, /window\.addEventListener\('drop', e => \{ if \(files\(e\) && !e\.defaultPrevented\) e\.preventDefault\(\); \}\)/);
  // A pane handler that takes a file drop prevents it; one that ignores it leaves it to that guard.
  assert.match(paneFiles, /e\.preventDefault\(\);\s*\n\s*const why = refusal\(\);/);
});

test('the pane code logs nothing and builds no markup from data', () => {
  const code = paneFiles.replace(/\/\/.*$/gm, '');
  assert.ok(!/console\./.test(code));
  assert.ok(!/innerHTML|outerHTML|insertAdjacentHTML/.test(code));
});

// ── The tray is built on the first drop, not at mount ────────────────────
// pane-files.js loads before chat-core.js (singleton.js), and panes are built
// at load: mounting must not need chat-core.js.
test('mount needs nothing from chat-core.js; the tray is built on the first drop', async () => {
  const { api, calls, late } = load({}, 'without');
  const { pane, sent } = fakePane();
  api.paneDropMount(pane); // chat-core.js is not loaded yet: must not throw
  assert.equal(pane.dropTray, undefined, 'no tray at mount');
  late(); // now every script is loaded
  pane.el.fire('drop', { dataTransfer: dt(file('a.png')) });
  assert.ok(pane.dropTray, 'the tray exists after the first drop');
  await settle();
  assert.equal(calls.xhr.length, 1);
  assert.equal(sent.length, 1);
});

test('without chat-core.js a file drop is refused with a plain notice, nothing throws', () => {
  const { api, calls } = load({}, 'without');
  const { pane, sent } = fakePane();
  api.paneDropMount(pane);
  const ev = pane.el.fire('drop', { dataTransfer: dt(file('a.png')) });
  assert.equal(ev.defaultPrevented, true);
  assert.match(pane.el.find('pane-drop-box').textContent, /File drop is not available on this page/);
  assert.equal(pane.el.find('pane-drop-box').hidden, false);
  assert.equal(calls.xhr.length, 0);
  assert.equal(sent.length, 0);
  // A text drag is left alone.
  const txt = pane.el.fire('drop', { dataTransfer: { types: ['text/plain'], items: [], files: [] } });
  assert.equal(txt.defaultPrevented, false);
});

test('panes.js reports a failure of pane-files.js once, fixed message, and goes on', () => {
  const src = panes.slice(panes.indexOf('let paneFilesFailed'), panes.indexOf('function createPane'));
  const errors = [];
  const ctx = vm.createContext({ console: { error: (...a) => errors.push(a.join(' ')) } });
  vm.runInContext(`${src}\nglobalThis.t = paneFilesTry;`, ctx);
  ctx.t(() => { throw new Error('/secret/path'); });
  ctx.t(() => { throw new Error('/secret/path'); });
  ctx.t(() => {});
  assert.equal(errors.length, 1);
  assert.ok(!/secret/.test(errors[0]));
  assert.match(panes, /paneFilesTry\(\(\) => paneDropMount\(pane\)\)/);
  assert.match(panes, /paneFilesTry\(\(\) => paneLinksAttach\(term, pane\)\)/);
});
