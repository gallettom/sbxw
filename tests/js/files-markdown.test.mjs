// The Files preview's Markdown view (assets/js/files.js): the Rendered |
// Source switch, kept for the session; Source is the plain text view; a page
// without the renderer falls back to the source with a notice; other files
// are not Markdown. The functions are taken from files.js itself and run
// against a small fake of the few DOM pieces they touch.
// Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const read = (p) => readFileSync(new URL(`../../${p}`, import.meta.url), 'utf8');
const files = read('assets/js/files.js');

// One top-level declaration of files.js, by name, up to the next one.
function decl(name) {
  const m = new RegExp(`^(?:async\\s+)?(?:function|const|let)\\s+${name}\\b`, 'm').exec(files);
  assert.ok(m, `files.js declares ${name}`);
  const rest = files.slice(m.index + 1);
  const next = rest.search(/^(?:\/\/\/|\/\*|(?:async\s+)?function |const |let |document\.)/m);
  return files.slice(m.index, m.index + 1 + (next < 0 ? rest.length : next));
}

function fakeBody() {
  const classes = new Set();
  const body = {
    html: '', pre: null, notices: [],
    classList: { add: (c) => classes.add(c), remove: (c) => classes.delete(c), contains: (c) => classes.has(c) },
    set innerHTML(v) { this.html = v; this.pre = v.includes('<pre class="fx-prev-text">') ? { textContent: '' } : null; },
    get innerHTML() { return this.html; },
    querySelector(sel) { return sel === 'pre' ? this.pre : null; },
    querySelectorAll() { return []; },
    insertAdjacentHTML(_, h) { this.notices.push(h); },
    scrollTop: 5,
  };
  return body;
}

function load({ renderer = true, storage = 'ok' } = {}) {
  const store = new Map();
  const reading = new Set();
  const sandbox = {
    URL,
    fx: { md: null, mdAnchor: null },
    humanSize: (n) => `${n} B`,
    fxNotice() {}, fxOpen() {}, fxFileUrl: (p) => `/f?path=${encodeURIComponent(p)}`,
    FX_IMAGE_MAX: 1,
    sessionStorage: {
      getItem: (k) => { if (storage === 'throws') throw new Error('denied'); return store.get(k) ?? null; },
      setItem: (k, v) => { if (storage === 'throws') throw new Error('denied'); store.set(k, v); },
    },
    document: {
      querySelector: () => ({ classList: { toggle: (c, on) => (on ? reading.add(c) : reading.delete(c)) } }),
      getElementById: () => null,
    },
  };
  const ctx = vm.createContext(sandbox);
  if (renderer) vm.runInContext(read('assets/js/markdown.js'), ctx);
  const names = ['FX_MD', 'FX_MD_VIEW_KEY', 'fxMdViewMem', 'fxMdView', 'fxSetMdView', 'fxReading', 'fxPaintSource', 'fxPaintMd'];
  vm.runInContext(names.map(decl).join('\n') + '\nthis.api = { FX_MD, fxMdView, fxSetMdView, fxPaintMd };', ctx);
  return { api: sandbox.api, sandbox, store, reading };
}

const doc = (text, extra = {}) => ({ key: 'a.md', path: '/w/docs/a.md', text, truncated: false, size: text.length, max: 1048576, render: null, ...extra });

test('only .md and .markdown files are Markdown', () => {
  const { api } = load();
  for (const n of ['a.md', 'README.MD', 'notes.markdown']) assert.ok(api.FX_MD.test(n), n);
  for (const n of ['a.mdx', 'a.txt', 'md', 'a.md.html', 'a.html', 'Cargo.toml']) assert.ok(!api.FX_MD.test(n), n);
});

test('Rendered by default; the choice is kept for the session; Source is the plain text view', () => {
  const { api, sandbox, store, reading } = load();
  assert.equal(api.fxMdView(), 'rendered');
  const body = fakeBody();
  sandbox.fx.md = doc('# Title\n\n<b>x</b> [a](https://x.org)');
  api.fxPaintMd(body);
  assert.match(body.html, /<article class="md-body"><h1 id="md-title" class="md-h">Title<\/h1>/);
  assert.ok(body.html.includes('&lt;b&gt;x&lt;/b&gt;'));
  assert.ok(body.classList.contains('fx-prev-md') && reading.has('md-reading'));
  assert.equal(sandbox.fx.md.render.headings.length, 1);

  api.fxSetMdView('source');
  assert.equal(store.get('sbxw.files.mdView'), 'source');
  api.fxPaintMd(body);
  assert.equal(body.html, '<pre class="fx-prev-text"></pre>');
  assert.equal(body.pre.textContent, '# Title\n\n<b>x</b> [a](https://x.org)', 'the source, as text');
  assert.ok(!body.classList.contains('fx-prev-md') && !reading.has('md-reading'));

  // A fresh page in the same session reads the stored choice.
  const again = load();
  again.store.set('sbxw.files.mdView', 'source');
  vm.runInContext('fxMdViewMem = null', vm.createContext(again.sandbox));
  assert.equal(again.api.fxMdView(), 'source');
});

test('a cut document says so at its end, in both views', () => {
  const { api, sandbox } = load();
  const body = fakeBody();
  sandbox.fx.md = doc('# A\n\ntext', { truncated: true, size: 3000000, max: 1048576 });
  api.fxPaintMd(body);
  assert.match(body.html, /<p class="md-cut">The file is 3000000 B; this is its first 1048576 B\. Download it for the rest\.<\/p><\/article>/);
  api.fxSetMdView('source');
  api.fxPaintMd(body);
  assert.match(body.html, /First 1048576 B of 3000000 B/);
});

test('without the renderer, the source with a plain notice', () => {
  const { api, sandbox } = load({ renderer: false });
  const body = fakeBody();
  sandbox.fx.md = doc('# T');
  api.fxPaintMd(body);
  assert.equal(body.pre.textContent, '# T');
  assert.deepEqual(body.notices, ['<p class="cm-empty-small">The Markdown reader did not load: showing the source.</p>']);
});

test('storage that throws: rendered by default, the switch still works in memory', () => {
  const { api } = load({ storage: 'throws' });
  assert.equal(api.fxMdView(), 'rendered');
  api.fxSetMdView('source');
  assert.equal(api.fxMdView(), 'source');
});

test('the HTML preview path is unchanged: a sandboxed iframe from srcdoc', () => {
  assert.ok(files.includes('sandbox="allow-scripts allow-downloads allow-popups"></iframe>`;\n    body.querySelector(\'iframe\').srcdoc = text;'));
  assert.ok(!/console\.(log|info|warn|error|debug)/.test(decl('fxPaintMd') + decl('fxMdFollow') + decl('fxMdScroll')), 'no console output of a file');
});
