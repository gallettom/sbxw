// The phone conversation's place (assets/remote/remote-core.js): one
// "stick" state that only a scroll sets; stuck, every change pins the log to
// its end, however much the content or the tail grew; scrolled up, the row
// at the top of the view stays where it was. Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const coreSrc = readFileSync(new URL('../../assets/remote/remote-core.js', import.meta.url), 'utf8');
const NAMES = ['rcStickAfterScroll', 'rcRowKeys', 'rcAnchorPick', 'rcLogPlace', 'rcAwayFromEnd'];
const ctx = { URLSearchParams, AbortController };
vm.runInNewContext(`${coreSrc}\n${NAMES.map(n => `this.${n} = ${n};`).join('\n')}`, ctx);
const rc = ctx;
const plain = v => JSON.parse(JSON.stringify(v));

test('only a scroll decides "stuck": at the end within 80 px, or while gliding there', () => {
  assert.equal(rc.rcStickAfterScroll(2000, 1200, 800), true, 'at the end');
  assert.equal(rc.rcStickAfterScroll(2000, 1130, 800), true, 'within 80 px');
  assert.equal(rc.rcStickAfterScroll(2000, 900, 800), false, 'scrolled up on purpose');
  assert.equal(rc.rcStickAfterScroll(2000, 300, 800, true), true, 'the round button\'s glide is on its way down');
});

test('the old bug: content growth no longer unsticks the log', () => {
  // At the end, then the tail grew by a question card (200 px) and a reply
  // arrived (400 px): no scroll happened, so the state is still "stuck",
  // and the place is the new end, not the old scrollTop.
  let stick = rc.rcStickAfterScroll(2000, 1200, 800);
  const after = rc.rcLogPlace(stick, null, [], 1200, 2600, 800);
  assert.equal(after, 1800);
  // The keyboard opens: the log is 300 px shorter, still pinned.
  assert.equal(rc.rcLogPlace(stick, null, [], 1800, 2600, 500), 2100);
  // A short log never scrolls below 0.
  assert.equal(rc.rcLogPlace(true, null, [], 0, 300, 800), 0);
});

test('scrolled up: the anchor row stays at the same offset when rows arrive or older ones load', () => {
  const rows = [
    { key: 's1', top: 0, bottom: 100 },
    { key: 's2', top: 110, bottom: 300 },
    { key: 'after2', top: 310, bottom: 340 },
    { key: 's5', top: 350, bottom: 600 },
  ];
  // The view starts at 150: s2 is the first row still in view, 40 px above the top.
  const anchor = rc.rcAnchorPick(rows, 150);
  assert.deepEqual(plain(anchor), { key: 's2', offset: -40 });
  // Older messages loaded above (+900 px): s2 is now at 1010.
  const older = [{ key: 's0', top: 0, bottom: 890 }, ...rows.map(r => ({ ...r, top: r.top + 900, bottom: r.bottom + 900 }))];
  assert.equal(rc.rcLogPlace(false, anchor, older, 150, 2000, 800), 1050);
  // New rows below: nothing above moved, the place is the same.
  assert.equal(rc.rcLogPlace(false, anchor, [...rows, { key: 's9', top: 610, bottom: 1500 }], 150, 1500, 800), 150);
  // The anchor row is gone (a provisional reply replaced): the old scrollTop.
  assert.equal(rc.rcLogPlace(false, { key: 'gone', offset: 0 }, rows, 150, 2000, 800), 150);
  // Never past the end.
  assert.equal(rc.rcLogPlace(false, anchor, older, 150, 1200, 800), 400);
  assert.equal(rc.rcAnchorPick([], 0), null);
});

test('row keys are stable across re-renders, a folded steps row keyed by the row before it', () => {
  assert.deepEqual(plain(rc.rcRowKeys([{ seq: 3 }, { kind: 'steps' }, { seq: 7 }, { kind: 'steps' }])), ['s3', 'after3', 's7', 'after7']);
  assert.deepEqual(plain(rc.rcRowKeys([{ kind: 'steps' }, { seq: 2 }])), ['after0', 's2']);
  assert.deepEqual(plain(rc.rcRowKeys(null)), []);
});
