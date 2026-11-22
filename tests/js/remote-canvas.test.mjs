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
const NAMES = ['rcCvPage', 'rcCvCounter', 'rcCvSwipe', 'rcCvClamp', 'rcCvPinch', 'rcCvToFrac', 'rcCvFromFrac', 'RC_CV_MAX_ZOOM',
  'rcCvId', 'rcCvImage', 'rcCvList', 'rcCvBadge', 'rcCvRemarks', 'rcCvManifest', 'rcCvShotLine', 'rcCvListLine', 'rcCvRowEl',
  'rcCvConfirm', 'rcCvConfirmText', 'rcCvSubmitResult', 'rcCvRemarkResult', 'rcClient', 'rcCvHold',
  'rcCvMode', 'rcCvTapAction', 'rcCvDoubleTap', 'rcCvIsDouble', 'rcCvToggleMode', 'rcCvHint', 'rcCvFsOpen', 'rcCvFsGo', 'rcCvFsClose', 'RC_CV_DOUBLE_ZOOM'];
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

// ── The manifest and the list, read defensively ──
const ID = 'c-0123456789abcdef';
const MANIFEST = {
  id: ID, sandbox: 'myproject', title: 'Mobile review', updated_ms: 1763800000000,
  rounds: [{ n: 1, at_ms: 1, shots: 2 }, { n: 2, at_ms: 2, shots: 1 }],
  shots: [
    { id: 's-1', round: 1, title: 'Home', note: 'logged out', width: 390, screen: 'home', file: 's-0123456789ab.png', px_w: 780, px_h: 1688, new: false },
    { id: 's-2', round: 1, title: 'Bad file', note: '', width: null, file: '../../etc/passwd', px_w: 1, px_h: 1, new: false },
    { id: 's-3', round: 2, title: 'Login', note: '', width: null, screen: null, file: 's-aaaaaaaaaaaa.webp', px_w: 1280, px_h: 800, new: true },
  ],
  pins: [
    { n: 4, shot: 's-1', x: 42.5, y: 10, text: 'Bigger tap target' },
    { n: 6, shot: 's-3', x: 150, y: -3, text: 'Line one\nline two' },
    { n: 'x', shot: 's-3', x: 1, y: 1, text: 'dropped' },
  ],
  notes: [{ n: 2, shot: 's-1', text: 'The empty state needs a call to action' }, { n: 3, shot: null, text: 'Overall too dark' }],
  draft: 5, draft_examples: 0, limits: { text: 2000 },
};

test('the manifest: bad files are dropped, coordinates kept in 0..100, junk remarks left out', () => {
  const m = plain(rc.rcCvManifest(MANIFEST));
  assert.deepEqual(m.shots.map(s => s.id), ['s-1', 's-3']);
  assert.deepEqual(m.pins.map(p => [p.n, p.x, p.y]), [[4, 42.5, 10], [6, 100, 0]]);
  assert.equal(m.notes.length, 2);
  assert.equal(m.draft, 5);
  assert.equal(rc.rcCvManifest({ ...MANIFEST, id: 'c-../x' }), null);
  assert.equal(rc.rcCvShotLine(m.shots[1], 2), 'Round 2 · new');
  assert.equal(rc.rcCvShotLine(m.shots[0], 2), 'Round 1 · 390 px wide');
});

test('pictures load only from /r/canvas/<id>/images/<file>, both checked', () => {
  assert.equal(rc.rcCvImage(ID, 's-0123456789ab.png'), `/r/canvas/${ID}/images/s-0123456789ab.png`);
  for (const f of ['../x.png', 's-0123456789ab.svg', 's-0123456789AB.png', 'https://evil/x.png', '%2e%2e', 's-0123456789ab.png?x', null]) {
    assert.equal(rc.rcCvImage(ID, f), null, String(f));
  }
  assert.equal(rc.rcCvImage('c-xyz', 's-0123456789ab.png'), null);
  assert.equal(rc.rcCvImage(`${ID}/../`, 's-0123456789ab.png'), null);
});

test('the client: right routes, JSON bodies, a bad id never fetched', async () => {
  const seen = [];
  const api = rc.rcClient(async (path, init) => { seen.push([init.method, path, init.body]); return { ok: true, status: 200, json: async () => ({ ok: true }) }; });
  await api.canvasList('my proj');
  await api.canvas(ID);
  await api.canvasRemark(ID, { op: 'remove', kind: 'pin', n: 4 });
  await api.canvasSubmit(ID);
  const bad = await api.canvas('../r/me');
  assert.equal(bad.status, 404);
  assert.deepEqual(seen, [
    ['GET', '/r/leaders/my%20proj/canvas', undefined],
    ['GET', `/r/canvas/${ID}`, undefined],
    ['POST', `/r/canvas/${ID}/remarks`, '{"op":"remove","kind":"pin","n":4}'],
    ['POST', `/r/canvas/${ID}/submit`, '{}'],
  ]);
});

test('the list and the tab badge', () => {
  const list = plain(rc.rcCvList({ canvases: [
    { id: ID, title: 'A', shots: 12, rounds: 2, new_shots: 5, draft: 3, updated_ms: 1000 },
    { id: 'c-1111111111111111', title: '', shots: 1, rounds: 1, new_shots: 0, draft: 0 },
    { id: 'nope', title: 'x' },
  ] }));
  assert.equal(list.length, 2);
  assert.equal(list[1].title, 'Untitled canvas');
  assert.equal(rc.rcCvBadge(list), 1);
  assert.equal(rc.rcCvBadge(null), 0);
  assert.equal(rc.rcCvListLine(list[0], 1000 + 5 * 60000), '12 shots · 2 rounds · 3 remarks not sent · updated 5 min ago');
  assert.equal(rc.rcCvListLine(list[1], 0), '1 shot');
});

// A document with only what the row may use: no innerHTML at all, so a
// title that reached markup would throw.
function fakeDoc() {
  const make = tag => {
    const el = { tag, className: '', type: '', children: [], _text: '' };
    el.append = (...kids) => { el.children.push(...kids); };
    Object.defineProperty(el, 'textContent', { get: () => el._text + el.children.map(c => c.textContent).join(''), set: v => { el._text = String(v); el.children = []; } });
    Object.defineProperty(el, 'innerHTML', { get: () => { throw new Error('innerHTML read'); }, set: () => { throw new Error('innerHTML written'); } });
    return el;
  };
  return { createElement: make };
}

test('hostile titles stay text: in a list row and in the confirmation', () => {
  const hostile = '<img src=x onerror=alert(1)>‮"</button><script>';
  const [c] = rc.rcCvList({ canvases: [{ id: ID, title: hostile, shots: 2, new_shots: 2 }] });
  const row = rc.rcCvRowEl(fakeDoc(), c, 0);
  assert.equal(row.tag, 'button');
  assert.deepEqual(row.children.map(k => [k.className, k.textContent]), [
    ['rc-cv-row-title', hostile], ['rc-cv-row-line', '2 shots'], ['rc-cv-new', '2 new'],
  ]);
  const m = rc.rcCvManifest({ ...MANIFEST, title: hostile, shots: [{ ...MANIFEST.shots[0], title: hostile }], pins: [{ n: 1, shot: 's-1', x: 1, y: 1, text: hostile }], notes: [] });
  const text = rc.rcCvConfirmText(rc.rcCvConfirm(m));
  assert.ok(text.includes(`Shot 1 · ${hostile}`));
  assert.ok(text.includes(`Pin 1 (1%, 1%): ${hostile}`));
});

// ── Send remarks ──
test('the confirmation lists every remark in full, by shot, and counts what the phone cannot show', () => {
  const c = rc.rcCvConfirm(rc.rcCvManifest(MANIFEST));
  assert.equal(c.empty, false);
  assert.equal(c.blocked, false);
  assert.equal(rc.rcCvConfirmText(c), [
    'Send 5 remarks?',
    'One round of remarks on "Mobile review", for the session of this sandbox. sbxw types one line asking it to read them; once sent, they cannot be changed.',
    'Shot 1 · Home',
    '  Pin 4 (43%, 10%): Bigger tap target',
    '  Note 2: The empty state needs a call to action',
    'Shot 2 · Login',
    '  Pin 6 (100%, 0%): Line one\nline two',
    'Not on a shot shown here',
    '  Note 3: Overall too dark',
    '1 more remark is on an example picture (shown on the Mac only).',
  ].join('\n'));
  const one = rc.rcCvConfirm(rc.rcCvManifest({ ...MANIFEST, pins: [], notes: [MANIFEST.notes[0]], draft: 1 }));
  assert.equal(one.title, 'Send 1 remark?');
  const none = rc.rcCvConfirm(rc.rcCvManifest({ ...MANIFEST, pins: [], notes: [], draft: 0 }));
  assert.equal(none.empty, true);
  const ex = rc.rcCvConfirm(rc.rcCvManifest({ ...MANIFEST, draft_examples: 1 }));
  assert.equal(ex.blocked, true);
  assert.ok(rc.rcCvConfirmText(ex).includes('can only be sent from the Mac'));
});

test('the send\'s answer: sent, or why not, in one sentence', () => {
  const r = (ok, status, data, unreachable = false) => plain(rc.rcCvSubmitResult({ ok, status, data, unreachable }));
  assert.deepEqual(r(true, 200, { ok: true, review: 3 }), { tone: 'ok', text: 'Sent: round 3 of remarks. The session was asked to read them.', sent: true });
  assert.match(r(false, 409, { reason: 'needs_mac', error: 'waits' }).text, /^Not sent\. This needs your Mac: the session is waiting for you there\. \(sbxw: waits\)$/);
  assert.match(r(false, 409, { reason: 'busy' }).text, /^Not sent\. The session is busy/);
  assert.match(r(false, 409, { reason: 'no_paste' }).text, /^Not sent\. Send it from your Mac/);
  assert.match(r(false, 409, { error: "'x' is read only from the phone (several_sessions)" }).text, /read only from the phone/);
  assert.match(r(false, 409, { reason: 'examples' }).text, /example pictures/);
  const ns = r(false, 409, { reason: 'not_submitted', recorded: true, review: 3 });
  assert.equal(ns.sent, true);
  assert.equal(ns.tone, 'warn');
  assert.match(ns.text, /^Round 3 is recorded, but .*NOT submitted/);
  assert.equal(r(false, 400, { error: 'there is no new remark to send' }).text, 'Not sent: there is no new remark to send.');
  assert.match(r(false, 0, null, true).text, /unreachable/);
  assert.match(r(false, 404, { error: 'not found' }).text, /no longer reachable/);
});

test('a remark\'s save: saved (with masking said), or not saved and why', () => {
  const r = (ok, status, data, unreachable = false) => plain(rc.rcCvRemarkResult({ ok, status, data, unreachable }, 'Pin 4'));
  assert.deepEqual(r(true, 200, { ok: true, n: 4, masked: 0 }), { tone: 'ok', text: 'Pin 4: saved.' });
  assert.equal(r(true, 200, { ok: true, n: 4, masked: 2 }).text, 'Pin 4: saved (2 words masked by your anonymise rules).');
  assert.equal(r(false, 404, { error: 'not found' }).reload, true);
  assert.match(r(false, 409, { error: 'too many pins' }).text, /canvas is full \(too many pins\)/);
  assert.equal(r(false, 400, { error: 'text holds a control character' }).text, 'Pin 4: not saved (text holds a control character).');
  assert.match(r(false, 0, null, true).text, /unreachable\. Your text is still in the field/);
  // The answer's remarks redraw the page without a second read.
  assert.deepEqual(plain(rc.rcCvRemarks({ ok: true, pins: [{ n: 7, shot: 's-1', x: 5, y: 6, text: 'a' }], notes: [], draft: 1 })), { pins: [{ n: 7, shot: 's-1', x: 5, y: 6, text: 'a' }], notes: [], draft: 1 });
});

test('a remark being typed holds the page: leaving does nothing and says why', () => {
  assert.equal(rc.rcCvHold(null, 'anything'), '', 'no editor open');
  assert.equal(rc.rcCvHold({ kind: 'pin', n: null, orig: '' }, '   '), '', 'a new pin with nothing typed');
  assert.equal(rc.rcCvHold({ kind: 'pin', n: 4, orig: 'Bigger' }, 'Bigger '), '', 'an existing pin, unchanged');
  assert.equal(rc.rcCvHold({ kind: 'pin', n: null, orig: '' }, 'Too small'), 'Save or Cancel the new pin first: its text is not saved yet.');
  assert.equal(rc.rcCvHold({ kind: 'pin', n: 4, orig: 'Bigger' }, 'Bigger, and blue'), 'Save or Cancel pin 4 first: its text is not saved yet.');
  assert.equal(rc.rcCvHold({ kind: 'note', n: 2, orig: 'x' }, ''), 'Save or Cancel note 2 first: its text is not saved yet.', 'emptied is a change too');
  assert.equal(rc.rcCvHold({ kind: 'note', n: null, orig: '' }, 'A note'), 'Save or Cancel the new note first: its text is not saved yet.');
});

// ── View mode and pin mode ──
const V1 = { s: 1, tx: 0, ty: 0 };
test('mode: a tap in view mode places nothing; in pin mode it places a pin', () => {
  assert.equal(rc.rcCvTapAction('view', false, V1, 150, 50, 300, 200).kind, 'none');
  assert.equal(rc.rcCvTapAction(undefined, false, V1, 150, 50, 300, 200).kind, 'none', 'view is the default');
  assert.deepEqual(plain(rc.rcCvTapAction('pin', false, V1, 150, 50, 300, 200)), { kind: 'new-pin', fx: 0.5, fy: 0.25 });
  assert.equal(rc.rcCvTapAction('pin', false, V1, 400, 50, 300, 200).kind, 'none', 'off the picture');
  const z = { s: 2, tx: -100, ty: 0 };
  assert.deepEqual(plain(rc.rcCvTapAction('pin', false, z, 100, 100, 300, 200)), { kind: 'new-pin', fx: 0.3333, fy: 0.25 }, 'at any zoom');
});
test('mode: an existing pin opens for editing in both modes', () => {
  assert.equal(rc.rcCvTapAction('view', true, V1, 1, 1, 300, 200).kind, 'edit-pin');
  assert.equal(rc.rcCvTapAction('pin', true, V1, 1, 1, 300, 200).kind, 'edit-pin');
});
test('mode: toggling on is free; leaving follows rcCvHold', () => {
  assert.deepEqual(plain(rc.rcCvToggleMode('view', null, '')), { mode: 'pin', hold: '' });
  assert.deepEqual(plain(rc.rcCvToggleMode('pin', null, '')), { mode: 'view', hold: '' });
  const fresh = { kind: 'pin', n: null, orig: '' };
  assert.equal(rc.rcCvToggleMode('pin', fresh, '').mode, 'view', 'a new pin with nothing typed does not hold');
  const held = rc.rcCvToggleMode('pin', fresh, 'wrong colour');
  assert.equal(held.mode, 'pin');
  assert.match(held.hold, /Save or Cancel the new pin first/);
  const edited = rc.rcCvToggleMode('pin', { kind: 'pin', n: 3, orig: 'a' }, 'b');
  assert.equal(edited.mode, 'pin');
  assert.match(edited.hold, /pin 3/);
  assert.equal(rc.rcCvToggleMode('pin', { kind: 'pin', n: 3, orig: 'a' }, ' a ').mode, 'view', 'unchanged text lets go');
  assert.equal(rc.rcCvToggleMode('pin', { kind: 'note', n: null, orig: '' }, 'typed').mode, 'view', 'a note is not a pin');
});
test('mode: the hint names the mode', () => {
  assert.match(rc.rcCvHint('pin'), /^Tap to place a pin/);
  assert.doesNotMatch(rc.rcCvHint('view'), /^Tap to place a pin/);
  assert.equal(rc.rcCvMode('junk'), 'view');
});

// ── Double tap ──
test('double tap: 1x goes to 2.5x about the tapped point; zoomed goes back to 1x', () => {
  const v = rc.rcCvDoubleTap(V1, 100, 50, 300, 200);
  near(v.s, 2.5, 's');
  near(v.tx, -150, 'tx');
  near(v.ty, -75, 'ty');
  // the tapped point stays under the finger
  near(100 * v.s + v.tx, 100, 'the point is fixed');
  near(50 * v.s + v.ty, 50, 'the point is fixed (y)');
  assert.deepEqual(plain(rc.rcCvDoubleTap({ s: 2.5, tx: -150, ty: -75 }, 10, 10, 300, 200)), V1);
  assert.deepEqual(plain(rc.rcCvDoubleTap({ s: 1.005, tx: 0, ty: 0 }, 100, 50, 300, 200)).s, 2.5, 'about 1x counts as 1x');
});
test('double tap: near an edge the picture stays covering its box', () => {
  const v = rc.rcCvDoubleTap(V1, 300, 200, 300, 200);
  near(v.tx, 300 - 300 * 2.5, 'tx at the limit');
  near(v.ty, 200 - 200 * 2.5, 'ty at the limit');
  const o = rc.rcCvDoubleTap(V1, 0, 0, 300, 200);
  near(o.tx, 0, 'tx');
  near(o.ty, 0, 'ty');
});
test('double tap: two close, quick taps only', () => {
  assert.equal(rc.rcCvIsDouble(null, 100, 0, 0), false);
  assert.equal(rc.rcCvIsDouble({ t: 0, x: 10, y: 10 }, 200, 14, 12), true);
  assert.equal(rc.rcCvIsDouble({ t: 0, x: 10, y: 10 }, 500, 10, 10), false, 'too slow');
  assert.equal(rc.rcCvIsDouble({ t: 0, x: 10, y: 10 }, 200, 100, 10), false, 'too far');
});

// ── Full screen ──
test('full screen: opens on the current shot at 1x; no shot, no overlay', () => {
  assert.deepEqual(plain(rc.rcCvFsOpen(2, 5)), { open: true, idx: 2, view: V1 });
  assert.equal(rc.rcCvFsOpen(0, 0).open, false);
  assert.equal(rc.rcCvFsOpen(9, 5).idx, 4, 'clamped');
});
test('full screen: next and previous stay in range and reset the zoom', () => {
  const s = { open: true, idx: 1, view: { s: 3, tx: -10, ty: -10 } };
  assert.deepEqual(plain(rc.rcCvFsGo(s, 1, 5)), { open: true, idx: 2, view: V1 });
  assert.equal(rc.rcCvFsGo({ ...s, idx: 4 }, 1, 5).idx, 4, 'no wrap');
  assert.equal(rc.rcCvFsGo({ ...s, idx: 0 }, -1, 5).idx, 0);
  assert.equal(rc.rcCvFsGo({ open: false, idx: 1, view: V1 }, 1, 5).open, false, 'closed stays closed');
});
test('full screen: closing keeps the shot and gives 1x', () => {
  assert.deepEqual(plain(rc.rcCvFsClose({ open: true, idx: 3, view: { s: 2, tx: -1, ty: -1 } })), { open: false, idx: 3, view: V1 });
  assert.deepEqual(plain(rc.rcCvFsClose(null)), { open: false, idx: 0, view: V1 });
});
test('full screen: swipe pages only at 1x, as on the page', () => {
  assert.equal(rc.rcCvSwipe(-120, 5, 1), 1);
  assert.equal(rc.rcCvSwipe(-120, 5, 2.5), 0);
});
