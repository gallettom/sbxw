// The web UI's one invisible-character list (assets/js/invisible.js) is the
// list of crates/core/src/invisible.rs, and each place of the web UI that asks
// it handles every character of it with its own action, leaves the characters
// around each range alone, and lets ❤️ (U+FE0F) and ❤︎ (U+FE0E) through:
// oscPreview (panes.js), paneTypeLine and paneTypePaths (pane-files.js),
// chatShortName (chat-core.js, and its copy on the phone page).
// Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const read = p => readFileSync(new URL(`../../${p}`, import.meta.url), 'utf8');
const invisibleSrc = read('assets/js/invisible.js');

// One top-level declaration of `src`, from its first line to the closing
// line at column 0.
function decl(src, head) {
  const a = src.indexOf(`\n${head}`) + 1;
  assert.ok(a > 0, `${head} is defined`);
  const end = src.indexOf(head.startsWith('function') ? '\n}\n' : ';\n', a);
  return src.slice(a, end + 2);
}

const rs = read('crates/core/src/invisible.rs');
const body = /pub const INVISIBLE[^=]*=\s*&\[([\s\S]*?)\n\];/.exec(rs)[1];
const WANT = [...body.matchAll(/\('\\u\{([0-9a-f]+)\}',\s*'\\u\{([0-9a-f]+)\}'\)/g)].map(m => [parseInt(m[1], 16), parseInt(m[2], 16)]);
const inList = n => WANT.some(([a, b]) => n >= a && n <= b);
const ALL = WANT.flatMap(([a, b]) => Array.from({ length: b - a + 1 }, (_, i) => a + i));
const NEIGHBOURS = WANT.flatMap(([a, b]) => [a - 1, b + 1]).filter(n => n >= 0 && !inList(n));
const EMOJI = ['\u2764\uFE0F ok', '\u2764\uFE0E ok', 'a\uFE0Fb', 'a\uFE0Eb'];

const ctx = {};
vm.runInNewContext([
  invisibleSrc,
  decl(read('assets/js/panes.js'), 'const OSC_PREVIEW_CHARS'),
  decl(read('assets/js/panes.js'), 'function oscPreview('),
  decl(read('assets/js/pane-files.js'), 'function paneShellQuote('),
  decl(read('assets/js/pane-files.js'), 'const PANE_PATH_BAD'),
  decl(read('assets/js/pane-files.js'), 'function paneTypeLine('),
  decl(read('assets/js/chat-core.js'), 'function chatShortName('),
  'this.api = { INVISIBLE_RANGES, invisibleCode, oscPreview, paneTypeLine, chatShortName };',
].join('\n'), ctx);
const { api } = ctx;

test('assets/js/invisible.js holds the list of crates/core/src/invisible.rs', () => {
  assert.ok(WANT.length >= 20);
  assert.deepEqual(JSON.parse(JSON.stringify(api.INVISIBLE_RANGES)), WANT);
  assert.ok(!api.invisibleCode(0xFE0E) && !api.invisibleCode(0xFE0F));
  for (const n of ALL) assert.ok(api.invisibleCode(n), n.toString(16));
  for (const n of NEIGHBOURS) assert.ok(!api.invisibleCode(n), n.toString(16));
});

test('the phone page holds the same list, and the same chatShortName', () => {
  const core = read('assets/remote/remote-core.js');
  const c = {};
  vm.runInNewContext(`${decl(core, 'const RC_INVISIBLE_RANGES')}\n${decl(core, 'const INVISIBLE_CLASS')}\n${decl(core, 'function chatShortName(')}\nthis.f = chatShortName;`, c);
  assert.equal(decl(core, 'function chatShortName('), decl(read('assets/js/chat-core.js'), 'function chatShortName('));
  for (const n of ALL) assert.equal(c.f(`a${String.fromCodePoint(n)}b`), 'a?b', n.toString(16));
  for (const s of EMOJI) assert.equal(c.f(s), s);
});

test('oscPreview marks every character of the list', () => {
  for (const n of ALL) assert.equal(api.oscPreview(`a${String.fromCodePoint(n)}b`), 'a\uFFFDb', n.toString(16));
  for (const s of EMOJI) assert.equal(api.oscPreview(s), s);
  for (const n of NEIGHBOURS) {
    const s = `a${String.fromCodePoint(n)}b`;
    assert.equal(api.oscPreview(s), s, n.toString(16));
  }
  // Controls keep their own marks.
  assert.equal(api.oscPreview('a\n\t\x1b\x7f\x9b'), 'a↵⇥␛␡\uFFFD');
});

test('paneTypeLine refuses a path with any character of the list', () => {
  for (const n of ALL) assert.equal(api.paneTypeLine([`/tmp/a${String.fromCodePoint(n)}b.png`]), null, n.toString(16));
  for (const s of EMOJI) assert.equal(api.paneTypeLine([`/tmp/${s}.png`]), `'/tmp/${s}.png' `);
  for (const n of NEIGHBOURS) assert.notEqual(api.paneTypeLine([`/tmp/a${String.fromCodePoint(n)}b.png`]), null, n.toString(16));
  assert.equal(api.paneTypeLine(['/tmp/a\x85b.png']), null, 'a C1 control');
});

test('chatShortName shows every character of the list as ?', () => {
  for (const n of ALL) assert.equal(api.chatShortName(`a${String.fromCodePoint(n)}b`), 'a?b', n.toString(16));
  for (const s of EMOJI) assert.equal(api.chatShortName(s), s);
  for (const n of NEIGHBOURS) {
    const s = `a${String.fromCodePoint(n)}b`;
    assert.equal(api.chatShortName(s), s, n.toString(16));
  }
  assert.equal(api.chatShortName('a\nb\x9bc'), 'a?b?c', 'controls');
});
