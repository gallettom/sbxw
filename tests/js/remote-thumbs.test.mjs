// Sent images on the phone (assets/remote/remote-core.js): a chip for a kept
// image becomes a lazy thumbnail that opens full screen; no copy, a file that
// is not an image, or a copy that fails to load keeps the name chip.
// Run: node --test tests/js/remote-thumbs.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const read = p => readFileSync(new URL(p, import.meta.url), 'utf8');
const ctx = { URLSearchParams, AbortController };
vm.runInNewContext(`${read('../../assets/remote/remote-core.js')}\nthis.rcRows = rcRows; this.rcThumbs = rcThumbs; this.rcMeEl = rcMeEl; this.rcSentSrc = rcSentSrc;`, ctx);
const rc = ctx;
const plain = v => JSON.parse(JSON.stringify(v));
const ID = 'a'.repeat(32);

function fakeDoc() {
  const make = tag => {
    const el = { tag, className: '', title: '', children: [], _text: '', handlers: {} };
    el.append = (...kids) => { el.children.push(...kids); };
    el.addEventListener = (n, f) => { el.handlers[n] = f; };
    el.replaceWith = other => { el.replacedBy = other; };
    Object.defineProperty(el, 'textContent', { get: () => el._text + el.children.map(c => c.textContent).join(''), set: v => { el._text = String(v); el.children = []; } });
    return el;
  };
  return { createElement: make };
}

test('rows carry the kept image id of each file, in step with the names', () => {
  const rows = plain(rc.rcRows([
    { seq: 1, kind: 'user_prompt', text: 'look', files: ['a.png', 'n.txt'], images: [ID, ''] },
    { seq: 2, kind: 'user_prompt', text: 'old', files: ['b.png'] },
    { seq: 3, kind: 'user_prompt', text: 'junk', files: ['c.png'], images: ['../../etc/passwd'] },
    { seq: 4, kind: 'user_prompt', text: 'skew', files: [3, 'd.png'], images: ['x', ID] },
  ]));
  assert.deepEqual(rows[0].images, [ID, '']);
  assert.equal(rows[1].images, undefined);
  assert.equal(rows[2].images, undefined, 'only 32 hex is an id');
  assert.deepEqual([rows[3].files, rows[3].images], [['d.png'], [ID]], 'a dropped file drops its id too');
});

test('the thumbnail choice: a src only for a valid id', () => {
  assert.deepEqual(plain(rc.rcThumbs({ files: ['a.png', 'n.txt', 'z.png'], images: [ID, '', 'nope'] })), [
    { name: 'a.png', src: `/r/sent/${ID}` }, { name: 'n.txt', src: null }, { name: 'z.png', src: null },
  ]);
  assert.deepEqual(plain(rc.rcThumbs({ files: ['a.png'] })), [{ name: 'a.png', src: null }]);
  assert.ok(rc.rcSentSrc(`/r/sent/${ID}`) && !rc.rcSentSrc('/r/sent/x') && !rc.rcSentSrc('https://e.x/r/sent/' + ID) && !rc.rcSentSrc(null));
});

test('a kept image is a lazy thumbnail button, the rest stay chips', () => {
  const el = rc.rcMeEl(fakeDoc(), { kind: 'me', text: 'hi', files: ['a.png', 'n.txt'], images: [ID, ''] });
  const box = el.children[1];
  const [thumb, chip] = box.children;
  assert.deepEqual([thumb.tag, thumb.className, thumb.type, thumb.dataset.src, thumb.dataset.name], ['button', 'rc-thumb', 'button', `/r/sent/${ID}`, 'a.png']);
  const img = thumb.children[0];
  assert.deepEqual([img.tag, img.loading, img.src, img.alt], ['img', 'lazy', `/r/sent/${ID}`, 'a.png']);
  assert.deepEqual([chip.tag, chip.className, chip.textContent], ['span', 'rc-file', 'n.txt']);
});

test('an image that fails to load (expired) falls back to the name chip', () => {
  const el = rc.rcMeEl(fakeDoc(), { kind: 'me', text: '', files: ['a.png'], images: [ID] });
  const thumb = el.children[0].children[0];
  thumb.children[0].handlers.error();
  assert.deepEqual([thumb.replacedBy.className, thumb.replacedBy.textContent], ['rc-file', 'a.png']);
});

test('the page opens a thumbnail full screen and styles it at 120px', () => {
  const js = read('../../assets/remote/remote.js');
  assert.match(js, /closest\('\.rc-thumb'\)/);
  assert.match(js, /rcCvPinch\(g\.v0/);
  const css = read('../../assets/remote/remote.css');
  assert.match(css, /\.rc-thumb-img \{[^}]*max-width: 120px; max-height: 120px/);
  assert.ok(css.indexOf('.rc-thumb {') < css.indexOf('/* ── Canvas: the list'), 'with the file chips, not at the end');
});
