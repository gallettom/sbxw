// The Files preview's Markdown reader (assets/js/markdown.js): each construct,
// the repository's own docs, hostile input, the link rules and the speed.
// Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const read = (p) => readFileSync(new URL(`../../${p}`, import.meta.url), 'utf8');
const ctx = vm.createContext({ URL });
vm.runInContext(read('assets/js/markdown.js'), ctx, { filename: 'markdown.js' });
const mdRender = vm.runInContext('mdRender', ctx);
const mdTarget = vm.runInContext('mdTarget', ctx);
const mdSlug = vm.runInContext('mdSlug', ctx);
const mdFindHeading = vm.runInContext('mdFindHeading', ctx);
const mdOutlineHtml = vm.runInContext('mdOutlineHtml', ctx);
const html = (src, base = '/w/docs') => mdRender(src, { base }).html;

// ── The output's vocabulary ──
// Every tag and attribute the renderer may write. A hostile document must
// produce nothing beyond these, and no attribute value may carry a script.
const TAGS = new Set(['h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'p', 'br', 'strong', 'em', 'del', 'code', 'pre', 'div',
  'blockquote', 'ul', 'ol', 'li', 'input', 'table', 'thead', 'tbody', 'tr', 'th', 'td', 'hr', 'a', 'img', 'span']);
const ATTRS = new Set(['id', 'class', 'href', 'target', 'rel', 'title', 'data-md-link', 'data-md-img', 'role',
  'tabindex', 'alt', 'type', 'disabled', 'checked', 'start', 'aria-hidden']);

// A strict tokenizer for what mdRender writes: `<tag attr="v" bare>` and
// `</tag>`, with text between that holds no `<` or `>` at all.
function audit(out) {
  const found = { tags: new Set(), attrs: new Set(), hrefs: [] };
  let i = 0;
  while (i < out.length) {
    const lt = out.indexOf('<', i);
    const text = lt < 0 ? out.slice(i) : out.slice(i, lt);
    assert.ok(!text.includes('>'), `a bare > in text: ${text.slice(0, 80)}`);
    if (lt < 0) break;
    const gt = out.indexOf('>', lt);
    assert.ok(gt > 0, 'an unclosed tag');
    const tag = out.slice(lt + 1, gt);
    const m = /^\/?([a-z0-9]+)((?:\s+[a-z-]+(?:="[^"<>]*")?)*)$/.exec(tag);
    assert.ok(m, `not a tag the renderer writes: <${tag.slice(0, 120)}>`);
    found.tags.add(m[1]);
    for (const a of m[2].matchAll(/([a-z-]+)(?:="([^"]*)")?/g)) {
      found.attrs.add(a[1]);
      if (a[1] === 'href') found.hrefs.push(a[2]);
      assert.ok(!/^on/i.test(a[1]), `a handler attribute: ${a[1]}`);
    }
    i = gt + 1;
  }
  for (const t of found.tags) assert.ok(TAGS.has(t), `unexpected tag <${t}>`);
  for (const a of found.attrs) assert.ok(ATTRS.has(a), `unexpected attribute ${a}`);
  for (const h of found.hrefs) assert.match(h, /^https?:\/\//, `an href that is not http(s): ${h}`);
  return found;
}

// ── Constructs ──

test('headings 1 to 6, ATX and setext, with stable ids', () => {
  const r = mdRender('# One\n## Two ##\n###### Six\n####### seven\n\nTitle\n=====\n\nSub\n---\n\n## Two');
  assert.deepEqual(JSON.parse(JSON.stringify(r.headings.map((h) => [h.level, h.text, h.id]))), [
    [1, 'One', 'md-one'], [2, 'Two', 'md-two'], [6, 'Six', 'md-six'], [1, 'Title', 'md-title'], [2, 'Sub', 'md-sub'], [2, 'Two', 'md-two-1'],
  ]);
  assert.match(r.html, /<p>####### seven<\/p>/);
  assert.equal(mdSlug('The chat dock: `sbxw` & you!'), 'the-chat-dock-sbxw--you');
});

test('paragraphs, soft and hard line breaks', () => {
  assert.equal(html('a\nb\n\nc'), '<p>a\nb</p><p>c</p>');
  assert.equal(html('a  \nb\\\nc'), '<p>a<br>b<br>c</p>');
});

test('bold, italic, strikethrough, inline code, backslash escapes', () => {
  assert.equal(html('**b** *i* _i_ ~~s~~ `c` \\*not\\*'), '<p><strong>b</strong> <em>i</em> <em>i</em> <del>s</del> <code>c</code> *not*</p>');
  assert.equal(html('***both*** snake_case_word'), '<p><em><strong>both</strong></em> snake_case_word</p>');
  assert.equal(html('`` a ` b `` and `**no**`'), '<p><code>a ` b</code> and <code>**no**</code></p>');
  assert.equal(html('**bold `code` bold**'), '<p><strong>bold <code>code</code> bold</strong></p>');
});

test('fenced code with its language, tildes, indented code', () => {
  assert.equal(html('```rust\nfn x() {}\n**no**\n```'), '<div class="md-code"><div class="md-code-lang">rust</div><pre><code>fn x() {}\n**no**</code></pre></div>');
  assert.equal(html('~~~\n<b>\n~~~'), '<div class="md-code"><pre><code>&lt;b&gt;</code></pre></div>');
  assert.equal(html('    let a = 1;\n    let b;'), '<div class="md-code"><pre><code>let a = 1;\nlet b;</code></pre></div>');
  assert.equal(html('```\nnever closed'), '<div class="md-code"><pre><code>never closed</code></pre></div>');
});

test('block quotes, nested, with lazy lines', () => {
  assert.equal(html('> a\nlazy\n> > b'), '<blockquote><p>a\nlazy</p><blockquote><p>b</p></blockquote></blockquote>');
});

test('lists: unordered, ordered with a start, nested, loose, tasks', () => {
  assert.equal(html('- a\n- b\n  - c\n    1. d'), '<ul><li>a</li><li>b<ul><li>c<ol><li>d</li></ol></li></ul></li></ul>');
  assert.equal(html('3. x\n4. y'), '<ol start="3"><li>x</li><li>y</li></ol>');
  assert.equal(html('- a\n\n- b'), '<ul><li><p>a</p></li><li><p>b</p></li></ul>');
  assert.equal(html('- [ ] todo\n- [x] done'), '<ul><li class="md-task"><input type="checkbox" disabled> todo</li><li class="md-task"><input type="checkbox" disabled checked> done</li></ul>');
  assert.equal(html('- one\n  more of one\n* other list'), '<ul><li>one\nmore of one</li></ul><ul><li>other list</li></ul>');
  assert.equal(html('- a\n\n  ```\n  code\n  ```'), '<ul><li><p>a</p><div class="md-code"><pre><code>code</code></pre></div></li></ul>');
});

test('tables with alignment, escaped pipes, short and long rows', () => {
  const r = mdRender('| L | C | R | N |\n|:--|:-:|--:|---|\n| a | `b` | c \\| d | e | extra |\n| short |');
  assert.equal(r.tables, 1);
  assert.equal(r.html, '<div class="md-table-wrap"><table><thead><tr><th class="md-al-left">L</th><th class="md-al-center">C</th><th class="md-al-right">R</th><th>N</th></tr></thead>'
    + '<tbody><tr><td class="md-al-left">a</td><td class="md-al-center"><code>b</code></td><td class="md-al-right">c | d</td><td>e</td></tr>'
    + '<tr><td class="md-al-left">short</td><td class="md-al-center"></td><td class="md-al-right"></td><td></td></tr></tbody></table></div>');
  assert.equal(html('a | b\n--|--'), '<div class="md-table-wrap"><table><thead><tr><th>a</th><th>b</th></tr></thead></table></div>');
  assert.equal(html('| a | b |\n|---|'), '<p>| a | b |\n|---|</p>', 'a delimiter row that does not match is text');
});

test('rules, raw HTML and entities as text', () => {
  assert.equal(html('***\n- - -\n___'), '<hr><hr><hr>');
  assert.equal(html('<div onclick="x">hi</div> &amp; &lt;b&gt; &#60;'), '<p>&lt;div onclick=&quot;x&quot;&gt;hi&lt;/div&gt; &amp;amp; &amp;lt;b&amp;gt; &amp;#60;</p>');
});

test('links: web, autolinks, references, titles', () => {
  const r = mdRender('[site](https://example.com/a?b=1 "Home") <https://x.org> see https://y.org/p). [ref][r] [r]\n\n[r]: http://z.net', { base: '/w' });
  assert.equal(r.html, '<p><a class="md-link md-ext" href="https://example.com/a?b=1" target="_blank" rel="noopener noreferrer" title="Home (https://example.com/a?b=1)">site<span class="md-ext-mark" aria-hidden="true">↗</span></a> '
    + '<a class="md-link md-ext" href="https://x.org/" target="_blank" rel="noopener noreferrer" title="https://x.org/">https://x.org<span class="md-ext-mark" aria-hidden="true">↗</span></a> '
    + 'see <a class="md-link md-ext" href="https://y.org/p" target="_blank" rel="noopener noreferrer" title="https://y.org/p">https://y.org/p<span class="md-ext-mark" aria-hidden="true">↗</span></a>). '
    + '<a class="md-link md-ext" href="http://z.net/" target="_blank" rel="noopener noreferrer" title="http://z.net/">ref<span class="md-ext-mark" aria-hidden="true">↗</span></a> '
    + '<a class="md-link md-ext" href="http://z.net/" target="_blank" rel="noopener noreferrer" title="http://z.net/">r<span class="md-ext-mark" aria-hidden="true">↗</span></a></p>');
  // No link inside a link: the outer brackets stay text (its bare URL is still an autolink).
  assert.match(html('[a [b](https://x.org) c](https://y.org)'), /^<p>\[a <a [^>]*href="https:\/\/x.org\/"[^>]*>b<span[^>]*>↗<\/span><\/a> c\]\(<a /);
});

test('link rules: relative files, anchors, .., bad schemes', () => {
  const base = '/w/docs';
  assert.deepEqual({ ...mdTarget('web-ui.md', base) }, { kind: 'file', path: '/w/docs/web-ui.md', anchor: '', dir: false });
  assert.deepEqual({ ...mdTarget('./chat.md#the-chat-dock', base) }, { kind: 'file', path: '/w/docs/chat.md', anchor: 'the-chat-dock', dir: false });
  assert.deepEqual({ ...mdTarget('../README.md', base) }, { kind: 'file', path: '/w/README.md', anchor: '', dir: false });
  assert.deepEqual({ ...mdTarget('../src/', base) }, { kind: 'file', path: '/w/src', anchor: '', dir: true });
  assert.deepEqual({ ...mdTarget('my%20notes.md?x=1', base) }, { kind: 'file', path: '/w/docs/my notes.md', anchor: '', dir: false });
  assert.deepEqual({ ...mdTarget('#two-listeners', base) }, { kind: 'anchor', anchor: 'two-listeners' });
  assert.equal(mdTarget('../../../etc/passwd', base), null, '.. may not climb above /');
  assert.equal(mdTarget('../../etc/x.md', base)?.path, '/etc/x.md');
  assert.equal(mdTarget('web-ui.md', null), null, 'no folder known: no file link');
  for (const bad of ['javascript:alert(1)', 'JaVaScRiPt:alert(1)', 'data:text/html,<script>', 'file:///etc/passwd', 'vbscript:x',
    'mailto:a@b.c', 'ftp://x', 'unknown:thing', '//evil.example/x', 'java\tscript:alert(1)', 'java\nscript:x', '\u0001javascript:x',
    'c:\\windows', 'blob:https://x/y']) {
    assert.equal(mdTarget(bad, base), null, bad);
  }
  // An entity is never decoded: this is a file named `&` with an odd anchor.
  assert.equal(mdTarget('&#106;avascript:alert(1)', base).kind, 'file');
  for (const u of ['https://x.org', 'HTTP://x.org/a b']) assert.equal(mdTarget(u, base)?.kind, 'web', u);
});

test('images: relative ones by index, remote and bad ones as text', () => {
  const r = mdRender('![logo](img/a.png "Logo") ![remote](https://x.org/a.png) ![x](x onerror=alert(1)) ![notimg](a.txt)', { base: '/w' });
  assert.deepEqual([...r.images], ['/w/img/a.png']);
  assert.equal(r.html, '<p><img class="md-img" data-md-img="0" alt="logo" title="Logo"> '
    + '<span class="md-img-alt" title="Image not loaded">[image: remote] <span class="md-url">(https://x.org/a.png)</span></span> '
    + '![x](x onerror=alert(1)) '
    + '<span class="md-img-alt" title="Image not loaded">[image: notimg] <span class="md-url">(a.txt)</span></span></p>');
  const many = mdRender(Array.from({ length: 40 }, (_, k) => `![i](p${k}.png)`).join(' '), { base: '/w' });
  assert.equal(many.images.length, 24, 'at most 24 images are loaded per document');
});

test('outline: buttons for the headings, found again from an anchor', () => {
  const r = mdRender('## A b\n### C\n## A b');
  assert.equal(mdOutlineHtml(r.headings), '<button type="button" class="md-ol-item md-ol-l1" data-md-goto="md-a-b" title="A b">A b</button>'
    + '<button type="button" class="md-ol-item md-ol-l2" data-md-goto="md-c" title="C">C</button>'
    + '<button type="button" class="md-ol-item md-ol-l1" data-md-goto="md-a-b-1" title="A b">A b</button>');
  assert.equal(mdFindHeading(r.headings, 'a-b-1').id, 'md-a-b-1');
  assert.equal(mdFindHeading(r.headings, 'C').id, 'md-c');
  assert.equal(mdFindHeading(r.headings, 'nope'), null);
});

// ── The repository's own docs ──

test('docs/security.md and docs/web-ui.md render whole', () => {
  for (const [file, headings, tables] of [['docs/security.md', 27, 3], ['docs/web-ui.md', 22, null]]) {
    const src = read(file);
    const r = mdRender(src, { base: '/work/docs' });
    const atx = src.split('\n').filter((l, i, all) => /^#{1,6} /.test(l)
      && all.slice(0, i).filter((x) => /^```/.test(x)).length % 2 === 0).length;
    assert.equal(r.headings.length, atx, `${file}: one heading per # line outside code`);
    assert.equal(r.headings.length, headings, `${file}: heading count`);
    const delims = src.split('\n').filter((l) => /^\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)+\|?\s*$/.test(l)).length;
    assert.equal(r.tables, tables ?? delims, `${file}: table count`);
    audit(r.html);
    // Every in-page link finds its heading.
    for (const l of r.links) if (l.kind === 'anchor') assert.ok(mdFindHeading(r.headings, l.anchor), `${file}: #${l.anchor}`);
    assert.ok(r.links.some((l) => l.kind === 'file' && l.path === '/work/README.md'), `${file}: the README link resolves`);
  }
});

// ── Hostile input ──

test('hostile Markdown produces only the expected elements and attributes', () => {
  const src = [
    '<script>alert(1)</script>', '<iframe src="https://evil"></iframe>', '<img src=x onerror=alert(1)>',
    '<a href="javascript:alert(1)" onclick="x()">raw</a>',
    '[x](javascript:alert(1))', '[x](JAVASCRIPT:alert(1) "t")', '[q](https://ok.example "a\\" onmouseover=\\"alert(1)")',
    '[q2](https://ok.example \'x" autofocus onfocus="alert(1)\')', '![x](x onerror=alert(1))', '![x](javascript:alert(1))',
    '<javascript:alert(1)>', '<https://x.org" onclick="alert(1)>', '<data:text/html;base64,PHNjcmlwdD4=>',
    'https://x.org/"><script>alert(1)</script>', '&lt;script&gt;alert(1)&lt;/script&gt; &#x3C;img src=x onerror=alert(1)&#x3E;',
    '[bad][r]', '', '[r]: javascript:alert(1)', '',
    '| <b onclick=x> | `<i>` |', '|---|---|', '| <script>x</script> | [y](vbscript:z) |', '',
    '```"><script>alert(1)</script>', 'x', '```', '',
    '# <img src=x onerror=alert(1)>', '- [x] <svg onload=alert(1)>', '> <style>*{}</style>',
    '[a](<javascript:alert(1)>) [b](https://x.org/\u0000) [c](https://x.org/"onmouseover="x)',
  ].join('\n');
  const r = mdRender(src, { base: '/w' });
  const found = audit(r.html);
  assert.ok(!/<(script|iframe|style|svg|object|embed|form|meta|link|base)\b/i.test(r.html));
  for (const l of r.links) assert.ok(['web', 'file', 'anchor'].includes(l.kind));
  for (const l of r.links.filter((x) => x.kind === 'web')) assert.match(l.href, /^https?:\/\/[^"\s<>]*$/);
  assert.ok(found.hrefs.every((h) => !h.includes('"')));
  assert.ok(r.html.includes('&lt;script&gt;alert(1)&lt;/script&gt;'), 'raw HTML is shown as text');
  assert.ok(r.html.includes('&amp;lt;script&amp;gt;'), 'entities are shown as typed, never decoded');
  assert.ok(r.html.includes('<span class="md-badlink">bad</span><span class="md-url"> (javascript:alert(1))</span>'), 'a reference to a bad scheme is text');
  assert.ok(r.html.includes('<span class="md-badlink">x</span><span class="md-url"> (javascript:alert(1))</span>'));
});

const time = (fn) => { const t = performance.now(); const v = fn(); return [performance.now() - t, v]; };
const report = [];

test('hostile nesting and long lines stay fast', () => {
  const cases = {
    '10,000 [': '['.repeat(10000),
    '10,000 [ then ](': '['.repeat(10000) + 'x' + '](a'.repeat(10000),
    '10,000 ![': '!['.repeat(10000) + ']'.repeat(10000),
    '[a](b repeated, never closed': '[a](bbbbbbbbbb '.repeat(7000),
    '5,000 nested quotes, one line': '>'.repeat(5000) + ' x',
    '5,000 nested quotes, staircase': Array.from({ length: 1400 }, (_, k) => '>'.repeat(k + 1) + ' q').join('\n'),
    '2,000 nested lists': Array.from({ length: 2000 }, (_, k) => ' '.repeat(2 * k) + '- i').join('\n'),
    '100,000-character line': 'word '.repeat(20000),
    '100,000 *': '*'.repeat(100000),
    '100,000 alternating *_': '*_'.repeat(50000),
    '50,000 **a**': '**a** '.repeat(16000),
    '100,000 backticks runs': '`a``b```c'.repeat(11000),
    '100,000 <': '<'.repeat(100000),
    '100,000 )': 'https://x.org/' + ')'.repeat(100000),
    'heading of spaces': '# ' + ' '.repeat(100000) + 'x',
    'nested emphasis': '*'.repeat(20000) + 'a' + '*'.repeat(20000),
    'link titles never closed': '[a](b "'.repeat(15000),
  };
  for (const [name, src] of Object.entries(cases)) {
    const [ms, r] = time(() => mdRender(src, { base: '/w' }));
    report.push(`${name}: ${ms.toFixed(0)} ms (${src.length} chars)`);
    assert.ok(ms < 1500, `${name} took ${ms.toFixed(0)} ms`);
    audit(r.html);
  }
});

test('a 200 KB document and a 5,000-line table render in well under a second', () => {
  const doc = read('docs/security.md');
  const big = doc.repeat(Math.ceil(200 * 1024 / doc.length)).slice(0, 200 * 1024);
  const [ms1, r1] = time(() => mdRender(big, { base: '/w/docs' }));
  const rows = Array.from({ length: 5000 }, (_, k) => `| ${k} | **row ${k}** | \`code\` | [link](https://x.org/${k}) |`);
  const table = ['| n | name | c | l |', '|--:|---|:-:|---|', ...rows].join('\n');
  const [ms2, r2] = time(() => mdRender(table));
  report.push(`200 KB document: ${ms1.toFixed(0)} ms (${r1.headings.length} headings)`, `5,000-line table: ${ms2.toFixed(0)} ms (${table.length} chars)`);
  assert.equal(r2.tables, 1);
  assert.equal(r2.html.match(/<tr>/g).length, 5001);
  assert.ok(ms1 < 1000 && ms2 < 1000, `${ms1} ${ms2}`);
  console.log('markdown speed:\n  ' + report.join('\n  '));
});
