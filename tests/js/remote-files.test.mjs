// Files sent with a message (assets/remote/remote-core.js): the history's
// `files` (base names) ride on the person's row and show as chips under the
// text, each name set as text, never as markup.
// Run: node --test tests/js/remote-files.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const read = p => readFileSync(new URL(p, import.meta.url), 'utf8');
const ctx = { URLSearchParams, AbortController };
vm.runInNewContext(`${read('../../assets/remote/remote-core.js')}\nthis.rcRows = rcRows; this.rcFiles = rcFiles; this.rcMeEl = rcMeEl; this.RC_FILES_MAX = RC_FILES_MAX;`, ctx);
const rc = ctx;
const plain = v => JSON.parse(JSON.stringify(v));

// A document with only what rcMeEl may use: no innerHTML at all, so a name
// that reached markup would throw.
function fakeDoc() {
  const make = tag => {
    const el = { tag, className: '', title: '', children: [], _text: '' };
    el.append = (...kids) => { el.children.push(...kids); };
    Object.defineProperty(el, 'textContent', { get: () => el._text + el.children.map(c => c.textContent).join(''), set: v => { el._text = String(v); el.children = []; } });
    Object.defineProperty(el, 'innerHTML', { get: () => { throw new Error('innerHTML read'); }, set: () => { throw new Error('innerHTML written'); } });
    return el;
  };
  return { createElement: make };
}

test('a prompt with files keeps its text and carries the names', () => {
  const rows = plain(rc.rcRows([
    { seq: 1, kind: 'user_prompt', text: 'Un peu trop gros la police', files: ['IMG_1838.png', 'notes.txt'] },
    { seq: 2, kind: 'user_prompt', files: ['only.png'] },
    { seq: 3, kind: 'user_prompt', text: 'no files' },
  ]));
  assert.deepEqual(rows.map(r => [r.kind, r.text, r.files]), [
    ['me', 'Un peu trop gros la police', ['IMG_1838.png', 'notes.txt']],
    ['me', '', ['only.png']],
    ['me', 'no files', undefined],
  ]);
});

test('file lists are never trusted: strings only, base names, capped', () => {
  assert.deepEqual(plain(rc.rcFiles(['a.png', 3, null, { toString: () => 'x' }, '/tmp/sbxw-uploads/1/b.png', 'C:\\x\\c.png', '', 'dir/'])), ['a.png', 'b.png', 'c.png']);
  assert.deepEqual(plain(rc.rcFiles('a.png')), []);
  assert.equal(rc.rcFiles(Array.from({ length: 50 }, (_, i) => `f${i}`)).length, rc.RC_FILES_MAX);
  // An empty prompt whose files are all junk is no row.
  assert.equal(rc.rcRows([{ seq: 1, kind: 'user_prompt', text: '', files: [1, ''] }]).length, 0);
});

test('chips sit under the text, each name set as text', () => {
  const hostile = '<img src=x onerror=alert(1)>".png';
  const el = rc.rcMeEl(fakeDoc(), { kind: 'me', text: '<b>look</b>', files: [hostile, 'b.txt'] });
  assert.equal(el.className, 'rc-msg rc-me');
  const [text, box] = el.children;
  assert.equal(text.className, 'rc-me-text');
  assert.equal(text.textContent, '<b>look</b>');
  assert.equal(box.className, 'rc-files');
  assert.deepEqual(box.children.map(c => [c.tag, c.className, c.textContent, c.title]), [
    ['span', 'rc-file', hostile, hostile],
    ['span', 'rc-file', 'b.txt', 'b.txt'],
  ]);
});

test('no files, no chip box; files alone, no text line', () => {
  const a = rc.rcMeEl(fakeDoc(), { kind: 'me', text: 'hi' });
  assert.deepEqual(a.children.map(c => c.className), ['rc-me-text']);
  const b = rc.rcMeEl(fakeDoc(), { kind: 'me', text: '', files: ['a.png'] });
  assert.deepEqual(b.children.map(c => c.className), ['rc-files']);
});

test('the page draws the person row with rcMeEl and styles the chips', () => {
  assert.match(read('../../assets/remote/remote.js'), /r\.kind === 'me'\) \{\s*return rcMeEl\(document, r\);/);
  const css = read('../../assets/remote/remote.css');
  for (const sel of ['.rc-files', '.rc-file']) assert.ok(css.includes(`${sel} {`), sel);
  // One rule per name: the chip keeps .rc-file, the hidden inputs have their own class.
  assert.equal(css.split('.rc-file {').length - 1, 1, 'one .rc-file rule, the chip\'s');
  assert.ok(css.includes('.rc-file-input {'));
  const html = read('../../assets/remote/index.html');
  for (const id of ['file', 'kb-file']) assert.match(html, new RegExp(`<input id="${id}" class="rc-file-input" type="file"`), id);
});
