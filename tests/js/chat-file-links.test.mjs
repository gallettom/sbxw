// File paths in an agent's reply (assets/js/chat-core.js): detection, the
// markup it produces for hostile text, and what a press calls.
// Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const src = readFileSync(new URL('../../assets/js/chat-core.js', import.meta.url), 'utf8');
const ctx = {};
vm.runInNewContext(`${src}\nthis.p = chatFilePath; this.md = chatMarkdown; this.wire = chatWireFiles;`, ctx);
const { p: filePath, md, wire } = ctx;
const render = (text) => { const files = { paths: [] }; return { html: md(text, files), files }; };
const linked = (text) => render(text).files.paths.map((f) => f.path);

test('positive cases', () => {
  assert.deepEqual(linked('the report is at `.sbxw-artifacts/foo.md`'), ['.sbxw-artifacts/foo.md']);
  assert.deepEqual(linked('the report is at .sbxw-artifacts/foo.md.'), ['.sbxw-artifacts/foo.md']);
  assert.deepEqual(linked('see src/main.rs:42, then'), ['src/main.rs']);
  assert.deepEqual(linked('see ./a/b.json and ~/notes/x.txt and /var/log/y.log'), ['./a/b.json', '~/notes/x.txt', '/var/log/y.log']);
  assert.deepEqual(linked('**docs/a.md**'), ['docs/a.md']);
  assert.equal(filePath('src/main.rs:42:7').line, 42);
  assert.deepEqual(linked('(see docs/a.md)'), ['docs/a.md']);
});

test('negative cases', () => {
  for (const t of ['foo.md', 'a/b', 'a/b.exe', 'a/../b.md', '../b.md', 'a//b.md', 'javascript:a/b.md', 'file:///etc/a.md',
    'http://x.com/a.md', 'https://x.com/a/b.md', '~a/b.md', 'a/.md', 'a/b.md/', 'a∕b.md', 'a/b\u0000.md', 'a/./b.md', 'x=y/b.md',
    'a/' + 'b'.repeat(4000) + '.md']) {
    assert.equal(filePath(t), null, t.slice(0, 40));
    assert.deepEqual(linked(t), [], t.slice(0, 40));
  }
  assert.deepEqual(linked('```\na/b.md\nc/d.md\n```'), []);
  assert.deepEqual(linked('[a/b.md](https://x.com/a/b.md)'), []);
  assert.deepEqual(linked('`cat a/b.md`'), []);
});

test('hostile text yields text or one inert button', () => {
  const hostile = ['<img src=x onerror=alert(1)> a/b.md', '`a/b.md"><script>`', 'a/"b.md', "a/'b.md", 'a/<b>.md',
    'x\na/b.md\ny', 'javascript:alert(1)//a/b.md', 'a/b c.md', 'a/b.md?x=<svg>', '`a/b`.md'];
  for (const t of hostile) {
    const { html } = render(t);
    const tags = [...html.matchAll(/<\/?([a-z0-9]+)([^>]*)>/gi)];
    for (const [whole, tag, attrs] of tags) {
      if (whole.startsWith('</')) continue;
      assert.ok(['p', 'br', 'code', 'button'].includes(tag.toLowerCase()), `${tag} in ${html}`);
      if (tag === 'button') assert.match(attrs, /^ type="button" class="chat-file" data-file="\d+"$/);
      else assert.equal(attrs, '', html);
    }
    assert.ok(!/href|onerror|<script|<img/i.test(html.replace(/&lt;[^]*?&gt;/g, '')), html);
  }
  const { html } = render('<b>a/b.md</b>');
  assert.match(html, /&lt;b&gt;/);
});

test('a 20,000 character reply stays fast', () => {
  const texts = [
    'word '.repeat(4000), 'a/'.repeat(10000), 'a'.repeat(20000), '*'.repeat(20000), 'a.'.repeat(10000),
    ('see docs/a.md. ').repeat(1300), '`'.repeat(20000), '[x]('.repeat(4000), ('x/y.md:' ).repeat(2800),
  ];
  for (const t of texts) {
    const t0 = performance.now();
    md(t, { paths: [] });
    assert.ok(performance.now() - t0 < 1500, `${performance.now() - t0}ms for ${t.slice(0, 10)}`);
  }
});

test('the person\'s own text is never linked: chatLine only renders ai through markdown', () => {
  assert.match(src, /if \(cls === 'ai'\) \{[^}]*chatMarkdownInto/);
  assert.match(src, /else \{\s*el\.appendChild\(document\.createTextNode\(text\)\)/);
});

test('a press opens once with the sandbox and the path; nothing on render', () => {
  const { files } = render('at `a/b.md` and c/d.txt');
  const calls = [];
  const btns = files.paths.map((_, i) => {
    const b = { dataset: { file: String(i) }, title: '', handlers: [], addEventListener(e, f) { this.handlers.push(f); } };
    return b;
  });
  const root = { querySelectorAll: (sel) => (assert.equal(sel, 'button.chat-file'), btns) };
  wire(root, files, 'my-box', (...a) => calls.push(a));
  assert.deepEqual(calls, []);
  assert.match(btns[0].title, /a\/b\.md.*my-box/);
  btns[1].handlers[0]();
  assert.deepEqual(calls, [['my-box', 'c/d.txt']]);
});

test('without a files context nothing is linked', () => {
  assert.ok(!md('see a/b.md', undefined).includes('<button'));
});

test('a button only reaches the paths of its own rendering', () => {
  const a = render('first x/a.md');
  const b = render('second y/b.md');
  const calls = [];
  const mk = (files) => { const bt = { dataset: { file: '0' }, h: null, addEventListener(_, f) { this.h = f; } }; wire({ querySelectorAll: () => [bt] }, files, 's', (...x) => calls.push(x)); return bt; };
  const ba = mk(a.files), bb = mk(b.files);
  const again = mk(render('re-rendered z/c.md').files); // a re-render is a new array
  bb.h(); ba.h(); again.h();
  assert.deepEqual(calls, [['s', 'y/b.md'], ['s', 'x/a.md'], ['s', 'z/c.md']]);
});

test('the tooltip carries the full resolved path and the sandbox', () => {
  const { files } = render('`docs/a.md`');
  const bt = { dataset: { file: '0' }, addEventListener() {} };
  wire({ querySelectorAll: () => [bt] }, files, 'box', () => {}, (sb, p) => `/work/${p}`);
  assert.equal(bt.title, "Open /work/docs/a.md in box's Project files");
});

test('dotfiles without an extension and bare directories are not links', () => {
  for (const t of ['a/.env', 'a/.gitignore', 'a/dir/', 'src/', '~/', './', 'a/Makefile']) assert.equal(filePath(t), null, t);
});
