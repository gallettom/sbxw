// Files dropped on a chat (assets/js/chat-core.js, between "BEGIN chat attach"
// and "END chat attach"): the checks, the tick, hostile names as text,
// what Send carries. No browser: a tiny fake DOM, a fake upload.
// Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const read = p => readFileSync(new URL(`../../${p}`, import.meta.url), 'utf8');
const core = read('assets/js/chat-core.js');
// Loaded before chat-core.js on every page (singleton.js, the side panel).
const invisible = read('assets/js/invisible.js');
const block = core.slice(core.indexOf('// BEGIN chat attach'), core.indexOf('// END chat attach'));
const composerSrc = core.slice(core.indexOf('function chatComposer'), core.indexOf('// ── A round of questions'));

class El {
  constructor(tag) { this.tag = tag; this.children = []; this.attrs = {}; this.listeners = {}; this.className = ''; this._text = ''; this.hidden = false; this.disabled = false; this.value = ''; this.checked = false; this.style = {}; }
  get textContent() { return this._text + this.children.map(c => c.textContent).join(''); }
  set textContent(v) { this._text = String(v); this.children = []; }
  get childNodes() { return this.children; }
  appendChild(c) { this.children.push(c); c.parent = this; return c; }
  append(...cs) { cs.forEach(c => this.appendChild(typeof c === 'string' ? Object.assign(new El('#text'), { _text: c }) : c)); }
  replaceChildren() { this.children = []; this._text = ''; }
  setAttribute(k, v) { this.attrs[k] = String(v); }
  addEventListener(t, f) { (this.listeners[t] ||= []).push(f); }
  insertBefore(c, ref) { this.children.splice(Math.max(0, this.children.indexOf(ref)), 0, c); c.parent = this; }
  after(c) { const p = this.parent; if (p) p.children.splice(p.children.indexOf(this) + 1, 0, c); }
  get classList() { return { toggle: (n, on) => { const s = new Set(this.className.split(' ').filter(Boolean)); on ? s.add(n) : s.delete(n); this.className = [...s].join(' '); } }; }
  fire(t, e = {}) { const ev = { defaultPrevented: false, preventDefault() { this.defaultPrevented = true; }, ...e }; (this.listeners[t] || []).forEach(f => f(ev)); return ev; }
  walk(f) { f(this); this.children.forEach(c => c.walk(f)); }
}
const document = { createElement: t => new El(t) };

function load(extra = '') {
  const ctx = vm.createContext({ document, AbortController, Promise, setTimeout, CSS: { supports: () => true } });
  vm.runInContext(`${invisible}\n${block}\n${composerSrc}\n${extra}\nglobalThis.api = { CHAT_SECRET_NAMES, chatSecretName, chatCheckAttach, chatAttachTray, chatRenderTray, chatDroppedFiles, chatUploadUrl, chatUploadName, chatWithAttachments, chatAttachMount, chatComposer, chatShortName };`, ctx);
  return ctx.api;
}
const api = load();
const file = (name, size = 10, type = '') => ({ name, size, type });
const tick = () => new Promise(r => setImmediate(r));
// A fake upload the test settles by hand.
function fakeSend() {
  const calls = [];
  const send = (sandbox, f, onProgress, signal) => new Promise((resolve, reject) => {
    const c = { sandbox, f, onProgress, resolve, reject, aborted: false };
    signal.addEventListener('abort', () => { c.aborted = true; reject(new Error('cancelled')); });
    calls.push(c);
  });
  return { send, calls };
}

test('the secret-name list equals kanban.js\'s, and so the desktop\'s', () => {
  const kb = read('assets/js/kanban.js');
  const m = /const KB_SECRET_NAMES = (\[[^\]]*\])/.exec(kb);
  assert.deepEqual(JSON.parse(m[1].replaceAll("'", '"')), JSON.parse(JSON.stringify(api.CHAT_SECRET_NAMES)));
  for (const n of ['.env', '.ENV.local', 'a.pem', 'id_rsa.pub', 'credentials.json', '.npmrc', 'x.KDBX']) assert.ok(api.chatSecretName(n), n);
  for (const n of ['notes.txt', 'environment.md', 'key.txt', 'my.npmrc']) assert.ok(!api.chatSecretName(n), n);
});

test('checks: empty, too big, folders, and the count cap are refused before any byte', () => {
  const r = api.chatCheckAttach(0, [file('a.txt'), file('empty', 0), file('huge', 128 * 1024 * 1024 + 1)], ['dir']);
  assert.deepEqual([...r.take.map(f => f.name)], ['a.txt']);
  assert.deepEqual([...r.refused.map(x => x.name)], ['dir', 'empty', 'huge']);
  assert.match(r.refused[0].why, /folder/);
  assert.match(r.refused[1].why, /empty/);
  assert.match(r.refused[2].why, /128 MB/);
  const many = Array.from({ length: 12 }, (_, i) => file(`f${i}`));
  const c = api.chatCheckAttach(3, many);
  assert.equal(c.take.length, 7);
  assert.equal(c.refused.length, 5);
  assert.match(c.refused[0].why, /10 files at most/);
  // archives and programs are not the upload route's business
  assert.equal(api.chatCheckAttach(0, [file('a.zip'), file('prog.exe')]).take.length, 2);
});

test('a secret-looking name needs the tick: nothing is uploaded before it', async () => {
  const { send, calls } = fakeSend();
  const t = api.chatAttachTray({ sandbox: () => 'ephemeral-chat', send });
  t.offer([file('notes.txt'), file('.env')]);
  await tick();
  assert.equal(calls.length, 0, 'nothing sent');
  assert.equal(t.chips.length, 0);
  assert.ok(t.held && t.held.reasons[0].includes('.env'));
  assert.equal(t.confirmHeld(false), false);
  assert.equal(t.confirmHeld(undefined), false);
  await tick();
  assert.equal(calls.length, 0, 'still nothing');
  assert.match(t.note, /Tick/);
  assert.match(t.problem(), /tick/);
  assert.equal(t.confirmHeld(true), true);
  await tick();
  assert.equal(calls.length, 1);
  assert.equal(calls[0].sandbox, 'ephemeral-chat');
  // a second look starts again unticked
  t.offer([file('a.pem')]);
  assert.equal(t.held.ticked, false);
  t.discardHeld();
  assert.equal(t.held, null);
});

test('uploads one at a time with progress; a failure stays with its reason and a retry', async () => {
  const { send, calls } = fakeSend();
  let changes = 0;
  const t = api.chatAttachTray({ sandbox: () => 's', send, onChange: () => changes++ });
  t.offer([file('a.txt', 100), file('b.txt', 200)]);
  await tick();
  assert.equal(calls.length, 1, 'one at a time');
  calls[0].onProgress(0.5);
  assert.equal(t.chips[0].pct, 0.5);
  calls[0].reject(new Error('sandbox is stopped'));
  await tick();
  assert.equal(t.chips[0].state, 'failed');
  assert.equal(t.chips[0].error, 'sandbox is stopped');
  assert.equal(calls.length, 2, 'the next one goes on');
  calls[1].resolve('/tmp/sbxw-uploads/1/b.txt');
  await tick();
  assert.equal(t.chips.length, 2, 'the failed chip does not disappear');
  assert.match(t.problem(), /failed/);
  t.retry(t.chips[0].id);
  await tick();
  assert.equal(calls.length, 3);
  calls[2].resolve('/tmp/sbxw-uploads/2/a.txt');
  await tick();
  assert.equal(t.problem(), '');
  assert.deepEqual([...t.chips.map(c => c.path)], ['/tmp/sbxw-uploads/2/a.txt', '/tmp/sbxw-uploads/1/b.txt']);
  assert.ok(changes > 3);
});

test('remove drops the chip and cancels a running upload; the next one starts', async () => {
  const { send, calls } = fakeSend();
  const t = api.chatAttachTray({ sandbox: () => 's', send });
  t.offer([file('a'), file('b')]);
  await tick();
  t.remove(t.chips[0].id);
  await tick();
  assert.equal(calls[0].aborted, true);
  assert.deepEqual([...t.chips.map(c => c.name)], ['b']);
  assert.equal(calls.length, 2);
  calls[1].resolve('/tmp/sbxw-uploads/1/b');
  await tick();
  assert.deepEqual([...t.take().map(c => c.path)], ['/tmp/sbxw-uploads/1/b']);
  assert.equal(t.count(), 0);
});

test('Send carries exactly the attachments left, the daemon\'s paths, after the typed text', async () => {
  const { send, calls } = fakeSend();
  const t = api.chatAttachTray({ sandbox: () => 's', send });
  t.offer([file('../../etc/passwd'), file('keep me.txt')]);
  await tick();
  calls[0].resolve('/tmp/sbxw-uploads/1/passwd');
  await tick();
  calls[1].resolve('/tmp/sbxw-uploads/1/keep_me.txt');
  await tick();
  t.remove(t.chips[0].id);
  const chips = t.take();
  assert.equal(api.chatWithAttachments('look at this', chips.map(c => c.path)),
    'look at this\n\nAttached files (uploaded to this sandbox):\n- /tmp/sbxw-uploads/1/keep_me.txt');
  assert.equal(api.chatWithAttachments('hi', []), 'hi');
  t.restore(chips);
  assert.equal(t.count(), 1, 'a failed send puts them back');
});

test('hostile names: chips are text only and the request is encoded', async () => {
  const names = ['<img src=x onerror=alert(1)>.png', '"quote\'s".txt', '../../etc/passwd', '..\\..\\win.ini', 'a\u0000b\nc‮.txt', 'x'.repeat(4000) + '.pdf', '<script>'];
  const { send, calls } = fakeSend();
  const t = api.chatAttachTray({ sandbox: () => 's', send });
  t.offer(names.map(n => file(n)));
  await tick();
  const box = new El('div');
  api.chatRenderTray(box, t, '');
  box.walk(el => {
    assert.ok(!('innerHTML' in el), 'no innerHTML was used');
    for (const v of Object.values(el.attrs)) assert.ok(v.length <= 130, 'attributes stay short');
  });
  const text = box.textContent;
  assert.ok(text.includes('<img src=x onerror=alert(1)>.png'), 'shown as text');
  assert.ok(!text.includes('x'.repeat(100)), 'a 4,000 character name is cut');
  assert.ok(!/[\u0000-\u001f‮]/.test(text));
  for (const n of names) {
    const url = api.chatUploadUrl('ephemeral-chat', n);
    assert.match(url, /^\/api\/sandboxes\/ephemeral-chat\/upload\?name=[A-Za-z0-9%._~!*'()-]*$/);
    assert.ok(!url.includes('..%2F') && !url.includes('%2F'), 'no separator in the name');
    assert.ok(url.length < 700);
  }
  assert.equal(api.chatUploadName('../../etc/passwd'), 'passwd');
  assert.equal(api.chatUploadName('..\\..\\win.ini'), 'win.ini');
  assert.ok(api.chatUploadName('x'.repeat(4000) + '.pdf').endsWith('.pdf'));
  assert.equal(calls.length, 1);
  // the source of the DOM part never builds HTML
  assert.ok(!/innerHTML|insertAdjacentHTML|outerHTML/.test(block), 'no HTML string building in the attach block');
});

test('a drop: files only; text, links and folders are told apart; nothing is read', () => {
  const textDrag = { types: ['text/plain', 'text/uri-list'], items: [{ kind: 'string' }], files: [] };
  assert.equal(api.chatDroppedFiles(textDrag), null);
  const f = file('a.txt');
  const dir = { kind: 'file', webkitGetAsEntry: () => ({ isDirectory: true, name: 'src' }), getAsFile: () => file('src') };
  const real = { kind: 'file', webkitGetAsEntry: () => ({ isDirectory: false, name: 'a.txt' }), getAsFile: () => f };
  const got = api.chatDroppedFiles({ types: ['Files'], items: [dir, real] });
  assert.deepEqual(JSON.parse(JSON.stringify(got.folders)), ['src']);
  assert.equal(got.files.length, 1);
  assert.equal(api.chatDroppedFiles({ types: ['Files'], files: [f] }).files[0], f);
});

function mount({ sandbox = 'ephemeral-chat' } = {}) {
  const { send, calls } = fakeSend();
  const form = new El('form'), panel = new El('div'), button = new El('button'), input = new El('textarea');
  form.appendChild(input);
  form.appendChild(button);
  let tray = null;
  const ui = api.chatAttachMount({
    form, panel, button,
    current: () => (sandbox ? (tray ||= api.chatAttachTray({ sandbox: () => sandbox, send, onChange: () => ui.render() }), { tray, sandbox })
      : { tray: null, why: 'a new chat has no sandbox yet.' }),
  });
  return { ui, form, panel, calls, input, get tray() { return tray; } };
}
const dropEvent = dt => ({ dataTransfer: dt, defaultPrevented: false });

test('the drop target names the sandbox, restores on leave, keeps typed text, never navigates', async () => {
  const m = mount();
  m.input.value = 'half a sentence';
  const veil = m.panel.children[m.panel.children.length - 1];
  const over = m.panel.fire('dragenter', { dataTransfer: { types: ['Files'] } });
  assert.ok(over.defaultPrevented);
  assert.equal(veil.hidden, false);
  assert.equal(veil.textContent, 'Drop to attach to ephemeral-chat');
  m.panel.fire('dragleave', { dataTransfer: { types: ['Files'] } });
  assert.equal(veil.hidden, true);
  // a text drag is left alone
  const txt = m.panel.fire('dragenter', { dataTransfer: { types: ['text/plain'] } });
  assert.equal(txt.defaultPrevented, false);
  assert.equal(veil.hidden, true);
  m.panel.fire('dragenter', { dataTransfer: { types: ['Files'] } });
  const ev = m.panel.fire('drop', { dataTransfer: { types: ['Files'], items: [{ kind: 'file', webkitGetAsEntry: () => null, getAsFile: () => file('a.txt', 5) }] } });
  assert.ok(ev.defaultPrevented, 'the browser does not open the file');
  assert.equal(veil.hidden, true);
  await tick();
  assert.equal(m.calls.length, 1);
  assert.equal(m.calls[0].sandbox, 'ephemeral-chat');
  assert.equal(m.input.value, 'half a sentence', 'typed text kept');
  assert.match(m.form.children[0].textContent, /a\.txt/);
  m.calls[0].resolve('/tmp/sbxw-uploads/1/a.txt');
  await tick();
  assert.match(m.form.children[0].textContent, /attached/);
  assert.match(m.form.children[0].textContent, /stays in the sandbox under \/tmp\/sbxw-uploads/);
});

test('a text drop is not intercepted on drop either', () => {
  const m = mount();
  const ev = m.panel.fire('drop', { dataTransfer: { types: ['text/uri-list'], items: [{ kind: 'string' }] } });
  assert.equal(ev.defaultPrevented, false);
  assert.equal(m.calls.length, 0);
});

test('with no sandbox the drop is refused with the reason, and nothing is sent', async () => {
  const m = mount({ sandbox: null });
  const veil = m.panel.children[m.panel.children.length - 1];
  m.panel.fire('dragenter', { dataTransfer: { types: ['Files'] } });
  assert.match(veil.textContent, /Cannot attach here: a new chat has no sandbox/);
  const over = m.panel.fire('dragover', { dataTransfer: { types: ['Files'] } });
  assert.equal(over.dataTransfer.dropEffect, 'none');
  m.panel.fire('drop', { dataTransfer: { types: ['Files'], items: [{ kind: 'file', webkitGetAsEntry: () => null, getAsFile: () => file('a.txt') }] } });
  await tick();
  assert.equal(m.calls.length, 0);
  assert.match(m.form.children[0].textContent, /no sandbox/);
});

test('the composer gate keeps the typed text; files alone make a message', () => {
  const form = new El('form'), input = new El('textarea');
  input.value = 'hello';
  const sent = [];
  let why = 'An upload failed: retry it or remove it.', files = 0;
  api.chatComposer(form, input, t => sent.push(t), { gate: () => why, allowEmpty: () => files > 0 });
  form.fire('submit');
  assert.deepEqual([...sent], []);
  assert.equal(input.value, 'hello');
  why = '';
  form.fire('submit');
  assert.deepEqual([...sent], ['hello']);
  assert.equal(input.value, '');
  form.fire('submit');
  assert.deepEqual([...sent], ['hello'], 'empty with no files sends nothing');
  files = 1;
  form.fire('submit');
  assert.deepEqual([...sent], ['hello', '']);
});
