// The session and thread name rule (assets/js/util.js: normalizeName,
// fixNameField), on the table shared with the desktop's Rust tests
// (tests/js/name-cases.json). Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const read = p => readFileSync(new URL(`../../${p}`, import.meta.url), 'utf8');
const util = read('assets/js/util.js');
const src = util.slice(util.indexOf('const NAME_MAX'));
const ctx = vm.createContext({});
vm.runInContext(`${src}\nglobalThis.api = { normalizeName, fixNameField };`, ctx);
const { normalizeName, fixNameField } = ctx.api;
const cases = JSON.parse(read('tests/js/name-cases.json'));
const NAME = /^[a-z0-9](?:[a-z0-9-]{0,38}[a-z0-9])?$/; // panes.js THREAD_NAME

// A name <input>: value and caret, as the browser keeps them.
function field(value, caret = value.length) {
  return { value, selectionStart: caret, selectionEnd: caret,
    setSelectionRange(a, b) { this.selectionStart = a; this.selectionEnd = b; } };
}

test('the shared table: full rule', () => {
  for (const [input, want] of cases.full) assert.equal(normalizeName(input), want, JSON.stringify(input));
});

test('the shared table: typing', () => {
  for (const [input, want] of cases.typing) assert.equal(normalizeName(input, true), want, JSON.stringify(input));
});

test('a 200-character paste is cut to 40 without a trailing hyphen', () => {
  const r = normalizeName('word '.repeat(40));
  assert.equal(r.length, 39);
  assert.ok(NAME.test(r));
  assert.equal(normalizeName('a'.repeat(200)).length, 40);
});

test('hostile input gives a valid name or nothing', () => {
  const hostile = ['<script>alert(1)</script>', '"><img src=x onerror=1>', "'; rm -rf / #", '$(reboot)', '\u0000\u0007\u001b[31m',
    '😀😀😀', 'x'.repeat(4000), '<b>'.repeat(2000), 'é'.repeat(4000), '‮evil', 'á'.repeat(500)];
  for (const h of hostile) {
    const r = normalizeName(h);
    assert.ok(r === '' || NAME.test(r), JSON.stringify(h.slice(0, 20)));
    assert.ok(!/[<>"'&]/.test(r));
  }
});

test('normalizing is idempotent', () => {
  for (const [input] of cases.full) { const r = normalizeName(input); assert.equal(normalizeName(r), r); }
});

test('paste normalizes the whole field at once', () => {
  const f = field('  Fix Login/Bug ');
  assert.equal(fixNameField(f, 'insertFromPaste'), 'fix-login-bug');
  assert.equal(f.value, 'fix-login-bug');
  assert.equal(f.selectionEnd, 'fix-login-bug'.length);
});

test('typing keeps a trailing hyphen and lower-cases as it goes', () => {
  const f = field('fix ');
  fixNameField(f, 'insertText');
  assert.equal(f.value, 'fix-');
  f.value = 'fix-L'; f.selectionStart = f.selectionEnd = 5;
  fixNameField(f, 'insertText');
  assert.equal(f.value, 'fix-l');
});

test('typing in the middle keeps the caret after the typed letter', () => {
  const f = field('fixbug', 3);
  f.value = 'fix Bug'; f.selectionStart = f.selectionEnd = 4; // a space typed at 3
  fixNameField(f, 'insertText');
  assert.equal(f.value, 'fix-bug');
  assert.equal(f.selectionEnd, 4);
  const g = field('abcdef', 2);
  g.value = 'abXcdef'; g.selectionStart = g.selectionEnd = 3;
  fixNameField(g, 'insertText');
  assert.equal(g.value, 'abxcdef');
  assert.equal(g.selectionEnd, 3);
  // A leading space typed first: nothing stays, the caret stays at 0.
  const h = field(' a', 1);
  fixNameField(h, 'insertText');
  assert.equal(h.value, 'a');
  assert.equal(h.selectionEnd, 0);
});

test('typing a name that needs no change leaves the field and caret alone', () => {
  const f = field('abc', 1);
  fixNameField(f, 'insertText');
  assert.equal(f.value, 'abc');
  assert.equal(f.selectionEnd, 1);
});

test('blur and submit finish the job', () => {
  assert.equal(normalizeName('fix-'), 'fix');
  assert.equal(normalizeName(' Fix- '), 'fix');
});

test('the old error text is gone from the dialog', () => {
  const panes = read('assets/js/panes.js');
  const dialog = panes.slice(panes.indexOf('function validateThreadName'), panes.indexOf('function submitThread'));
  assert.ok(!dialog.includes('inner hyphens'));
  assert.ok(dialog.includes('Will be created as'));
});
