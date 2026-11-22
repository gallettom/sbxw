// The phone page's Canvas mode, its pure part (assets/remote/remote-core.js):
// the pager's index, the swipe that only pages when not zoomed, zoom and pan
// kept on the picture, a tap turned into a pin's fraction at any zoom, the
// manifest read defensively, the confirmation text, and hostile titles kept
// as text. Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const read = p => readFileSync(new URL(p, import.meta.url), 'utf8');
const coreSrc = read('../../assets/remote/remote-core.js');
const NAMES = ['rcCvPage', 'rcCvCounter', 'rcCvSwipe', 'rcCvClamp', 'rcCvPinch', 'rcCvToggle', 'rcCvToFrac', 'rcCvFromFrac', 'RC_CV_MAX_ZOOM'];
const ctx = { URLSearchParams, AbortController };
vm.runInNewContext(`${coreSrc}\n${NAMES.map(n => `this.${n} = ${n};`).join('\n')}`, ctx);
const rc = ctx;
const plain = v => JSON.parse(JSON.stringify(v));
const near = (a, b, msg) => assert.ok(Math.abs(a - b) < 1e-6, `${msg}: ${a} != ${b}`);

// ── The pager ──
test('pager: the index stays in [0, n - 1]; the counter is 1-based', () => {
  assert.equal(rc.rcCvPage(0, 12, 1), 1);
  assert.equal(rc.rcCvPage(11, 12, 1), 11, 'no wrap past the last');
  assert.equal(rc.rcCvPage(0, 12, -1), 0, 'no wrap before the first');
  assert.equal(rc.rcCvPage(40, 12), 11, 'an index beyond a shrunk list');
  assert.equal(rc.rcCvPage(3, 0, 1), 0, 'no page at all');
  assert.equal(rc.rcCvPage('x', 5, 2), 2, 'junk counts as 0');
  assert.equal(rc.rcCvPage(2.7, 5), 2);
  assert.equal(rc.rcCvCounter(2, 12), '3 / 12');
  assert.equal(rc.rcCvCounter(99, 12), '12 / 12');
  assert.equal(rc.rcCvCounter(0, 0), '0 / 0');
});

test('swipe: left is next, right is previous; short, vertical or zoomed moves do not page', () => {
  assert.equal(rc.rcCvSwipe(-80, 5), 1);
  assert.equal(rc.rcCvSwipe(80, -5), -1);
  assert.equal(rc.rcCvSwipe(-30, 0), 0, 'too short');
  assert.equal(rc.rcCvSwipe(-80, 70), 0, 'mostly vertical');
  assert.equal(rc.rcCvSwipe(-200, 0, 2), 0, 'zoomed in: the finger pans');
  assert.equal(rc.rcCvSwipe(-200, 0, 1.005), 1, 'rounding at 1x still pages');
});

// ── Zoom and pan ──
test('clamp: scale in [1, max], the picture always covers its box', () => {
  assert.deepEqual(plain(rc.rcCvClamp({ s: 0.3, tx: 50, ty: -50 }, 300, 200)), { s: 1, tx: 0, ty: 0 });
  assert.deepEqual(plain(rc.rcCvClamp({ s: 9, tx: 0, ty: 0 }, 300, 200)), { s: rc.RC_CV_MAX_ZOOM, tx: 0, ty: 0 });
  // At 2x the picture is 600x400: it may move by up to -300, -200.
  assert.deepEqual(plain(rc.rcCvClamp({ s: 2, tx: -1000, ty: 10 }, 300, 200)), { s: 2, tx: -300, ty: 0 });
  assert.deepEqual(plain(rc.rcCvClamp({ s: 2, tx: -120, ty: -80 }, 300, 200)), { s: 2, tx: -120, ty: -80 });
  assert.deepEqual(plain(rc.rcCvClamp(null, 300, 200)), { s: 1, tx: 0, ty: 0 });
});

test('pinch: the point under the fingers stays under them', () => {
  const v0 = { s: 1, tx: 0, ty: 0 };
  const v = rc.rcCvPinch(v0, { x: 150, y: 100 }, 100, { x: 150, y: 100 }, 200, 300, 200);
  near(v.s, 2, 'twice the distance, twice the scale');
  // The middle of the picture (0.5, 0.5) is still under (150, 100).
  const p = rc.rcCvFromFrac(0.5, 0.5, v, 300, 200);
  near(p.x, 150, 'x'); near(p.y, 100, 'y');
  // Pinching in below 1x comes back to the whole picture.
  assert.deepEqual(plain(rc.rcCvPinch(v, { x: 10, y: 10 }, 200, { x: 10, y: 10 }, 20, 300, 200)), { s: 1, tx: 0, ty: 0 });
  const t = rc.rcCvToggle(v0, { x: 0, y: 0 }, 300, 200);
  near(t.s, 2, 'double tap zooms'); assert.deepEqual(plain(rc.rcCvToggle(t, { x: 0, y: 0 }, 300, 200)), { s: 1, tx: 0, ty: 0 });
});

test('a tap becomes a fraction of the picture at any zoom, and back', () => {
  assert.deepEqual(plain(rc.rcCvToFrac(150, 50, { s: 1, tx: 0, ty: 0 }, 300, 200)), { fx: 0.5, fy: 0.25 });
  // Zoomed 2x, moved so the picture's centre is at the box's top left.
  const v = { s: 2, tx: -300, ty: -200 };
  assert.deepEqual(plain(rc.rcCvToFrac(0, 0, v, 300, 200)), { fx: 0.5, fy: 0.5 });
  assert.deepEqual(plain(rc.rcCvToFrac(300, 200, v, 300, 200)), { fx: 1, fy: 1 });
  assert.deepEqual(plain(rc.rcCvToFrac(75, 50, v, 300, 200)), { fx: 0.625, fy: 0.625 });
  for (const [fx, fy] of [[0.1, 0.9], [0.3333, 0.6667], [0, 0], [1, 1]]) {
    const p = rc.rcCvFromFrac(fx, fy, { s: 3, tx: -250, ty: -90 }, 300, 200);
    const f = rc.rcCvToFrac(p.x, p.y, { s: 3, tx: -250, ty: -90 }, 300, 200);
    near(f.fx, fx, 'round trip x'); near(f.fy, fy, 'round trip y');
  }
  assert.equal(rc.rcCvToFrac(-1, 10, { s: 1, tx: 0, ty: 0 }, 300, 200), null, 'off the picture');
  assert.equal(rc.rcCvToFrac(10, 10, { s: 1, tx: 0, ty: 0 }, 0, 200), null, 'no box yet');
  // Four decimals at most.
  assert.deepEqual(plain(rc.rcCvToFrac(100, 100, { s: 1, tx: 0, ty: 0 }, 300, 300)), { fx: 0.3333, fy: 0.3333 });
});
