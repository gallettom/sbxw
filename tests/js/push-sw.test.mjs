// The service worker (assets/remote/sw.js): a notification for every push,
// only ever one of three fixed French texts or one naming the sandbox; the badge set, cleared, and
// never in the way; and no way for the worker to read or send data.
// Run: node --test tests/js/push-sw.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const src = readFileSync(new URL('../../assets/remote/sw.js', import.meta.url), 'utf8');
const TEXTS = new Set(['Un plan vous attend', 'Un merge vous attend', 'Un agent a besoin de vous', 'Quelque chose vous attend']);
// A shown text: a fixed one, or one of the three that name a sandbox.
const NAMED = /^(?:Un (?:plan|merge) vous attend dans [A-Za-z0-9-]{1,63}|(?:Team leader )?[A-Za-z0-9-]{1,63} a besoin de vous)$/;
const shownOk = t => TEXTS.has(t) || NAMED.test(t);

// A worker world: records every notification, badge call and window action.
function world({ badge = 'both', showFails = false, badgeFails = false, windows = [] } = {}) {
  const w = { shown: [], badges: [], listeners: {}, opened: [], order: [] };
  const nav = {};
  if (badge === 'both' || badge === 'set') nav.setAppBadge = async n => { w.order.push('badge'); w.badges.push(['set', n]); if (badgeFails) throw new Error('no'); };
  if (badge === 'both' || badge === 'clear') nav.clearAppBadge = async () => { w.order.push('badge'); w.badges.push(['clear']); if (badgeFails) throw new Error('no'); };
  const self = {
    navigator: nav,
    registration: {
      showNotification: (title, opts) => {
        w.order.push('show');
        w.shown.push({ title, opts });
        if (showFails === 'sync') throw new Error('no');
        return showFails ? Promise.reject(new Error('no')) : Promise.resolve();
      },
    },
    clients: {
      matchAll: async () => windows,
      openWindow: async url => { w.opened.push(url); },
    },
    addEventListener: (t, f) => { (w.listeners[t] ||= []).push(f); },
  };
  vm.runInNewContext(src, { self, Number, Math, Promise }, { filename: 'sw.js' });
  w.push = async data => {
    const waits = [];
    await Promise.all(w.listeners.push.map(f => f({ data, waitUntil: p => waits.push(p) })));
    await Promise.all(waits);
    return waits.length;
  };
  w.click = async () => {
    const waits = [];
    let closed = 0;
    for (const f of w.listeners.notificationclick) f({ notification: { close: () => closed++ }, waitUntil: p => waits.push(p) });
    await Promise.all(waits);
    return closed;
  };
  return w;
}
const json = o => ({ json: () => JSON.parse(JSON.stringify(o)) });
const text = t => ({ json: () => JSON.parse(t) });

test('exactly two listeners: push and notificationclick', () => {
  const w = world();
  assert.deepEqual(Object.keys(w.listeners).sort(), ['notificationclick', 'push']);
});

test('the kind table: p, n, and the fallback', async () => {
  const w = world();
  await w.push(json({ kind: 'p', badge: 1 }));
  await w.push(json({ kind: 'n', badge: 2 }));
  await w.push(json({ kind: 'x', badge: 2 }));
  assert.deepEqual(w.shown.map(s => [s.title, s.opts.body]), [
    ['sbxw', 'Un plan vous attend'],
    ['sbxw', 'Un agent a besoin de vous'],
    ['sbxw', 'Quelque chose vous attend'],
  ]);
  for (const s of w.shown) assert.equal(s.opts.tag, 'sbxw', 'one tag: a newer one replaces the older');
});

test('showNotification is called for every push shape, inside waitUntil', async () => {
  const shapes = [
    json({ kind: 'p', badge: 3 }), json({ kind: 'n', badge: 0 }), json({}), json(null), json([]), json('p'), json(7),
    json({ kind: 5 }), json({ kind: null }), json({ kind: ['p'] }), json({ kind: '__proto__' }), json({ kind: 'constructor' }),
    json({ badge: 'x' }), json({ kind: 'p', badge: 1e9 }), json({ kind: 'p', badge: -5 }), json({ kind: 'p', badge: 1.5 }),
    text('not json'), text(''), text('{'), null, undefined,
    { json: () => { throw new Error('boom'); } },
  ];
  for (const data of shapes) {
    const w = world();
    assert.equal(await w.push(data), 1, 'one waitUntil');
    assert.equal(w.shown.length, 1, `a notification for ${JSON.stringify(data && data.json ? (() => { try { return data.json(); } catch (_) { return 'throws'; } })() : data)}`);
    assert.ok(shownOk(w.shown[0].opts.body), w.shown[0].opts.body);
    assert.equal(w.shown[0].title, 'sbxw');
  }
});

test('the text names the sandbox: a team leader, a sandbox, a plan', async () => {
  const cases = [
    [{ kind: 'n', badge: 1, name: 'sbxw-2', lead: true }, 'Team leader sbxw-2 a besoin de vous'],
    [{ kind: 'n', badge: 1, name: 'sbxw-2', lead: false }, 'sbxw-2 a besoin de vous'],
    [{ kind: 'p', badge: 1, name: 'sbxw-2', lead: false }, 'Un plan vous attend dans sbxw-2'],
    [{ kind: 'p', badge: 1, name: 'sbxw-2', lead: true }, 'Un plan vous attend dans sbxw-2'],
    [{ kind: 'n', badge: 1, name: 'x'.repeat(63), lead: false }, `${'x'.repeat(63)} a besoin de vous`],
    // No name, or one that is not a sbx name: the fixed texts.
    [{ kind: 'n', badge: 1, name: '', lead: true }, 'Un agent a besoin de vous'],
    [{ kind: 'p', badge: 1, name: '' }, 'Un plan vous attend'],
    [{ kind: 'm', badge: 1, name: 'sbxw-2', lead: false }, 'Un merge vous attend dans sbxw-2'],
    [{ kind: 'm', badge: 1, name: '' }, 'Un merge vous attend'],
    [{ kind: 'm', badge: 1 }, 'Un merge vous attend'],
    [{ kind: 'm', badge: 1, name: '<b>x</b>' }, 'Un merge vous attend'],
    [{ kind: 'n', badge: 1, lead: true }, 'Un agent a besoin de vous'],
    [{ kind: 'n', badge: 1, name: '<b>x</b>', lead: true }, 'Un agent a besoin de vous'],
    [{ kind: 'n', badge: 1, name: 'a b' }, 'Un agent a besoin de vous'],
    [{ kind: 'n', badge: 1, name: 'x'.repeat(64) }, 'Un agent a besoin de vous'],
    [{ kind: 'p', badge: 1, name: ['sbxw-2'] }, 'Un plan vous attend'],
    [{ kind: 'p', badge: 1, name: 7 }, 'Un plan vous attend'],
    [{ kind: 'n', badge: 1, name: 'sbxw-2', lead: 'true' }, 'sbxw-2 a besoin de vous'],
    [{ kind: 'x', badge: 1, name: 'sbxw-2', lead: true }, 'Quelque chose vous attend'],
  ];
  for (const [payload, want] of cases) {
    const w = world();
    await w.push(json(payload));
    assert.equal(w.shown[0].opts.body, want, JSON.stringify(payload));
    assert.equal(w.shown[0].title, 'sbxw');
  }
});

test('the notification is requested before the badge', async () => {
  const w = world();
  await w.push(json({ kind: 'p', badge: 4 }));
  assert.deepEqual(w.order, ['show', 'badge']);
});

test('hostile payloads change nothing shown: only the table texts appear', async () => {
  const evil = '<img src=x onerror=alert(1)>'.repeat(500);
  const payloads = [
    { kind: 'p', badge: 2, title: evil, body: evil, text: evil, url: 'https://evil.example/', icon: 'https://evil.example/i.png', actions: [{ action: 'a', title: evil }], data: { x: evil } },
    { kind: evil, badge: 2 },
    { kind: 'p', badge: 2, body: 'Your password is hunter2' },
    { kind: 'n‮', badge: 3 },
    { kind: 'n', badge: 3, name: evil, lead: true },
    { kind: 'p', badge: 3, name: 'sbxw-2\n<script>evil</script>' },
  ];
  for (const p of payloads) {
    const w = world();
    await w.push(json(p));
    assert.equal(w.shown.length, 1);
    const { title, opts } = w.shown[0];
    assert.equal(title, 'sbxw');
    assert.deepEqual(Object.keys(opts).sort(), ['body', 'lang', 'renotify', 'tag']);
    assert.ok(TEXTS.has(opts.body), opts.body);
    assert.equal(JSON.stringify(w.shown[0]).includes('evil'), false);
  }
  const w = world();
  await w.push(text(`{"kind":"p","badge":1,"x":"${'a'.repeat(100000)}"}`));
  assert.equal(w.shown[0].opts.body, 'Un plan vous attend');
});

test('the badge: set from the number, cleared at 0, clamped, untouched when not a number', async () => {
  const cases = [[3, [['set', 3]]], [0, [['clear']]], [99, [['set', 99]]], [500, [['set', 99]]], [-4, [['clear']]], [2.5, []], ['3', []], [null, []], [undefined, []]];
  for (const [badge, expected] of cases) {
    const w = world();
    await w.push(json({ kind: 'p', badge }));
    assert.deepEqual(w.badges, expected, `badge ${String(badge)}`);
    assert.equal(w.shown.length, 1);
  }
});

test('the badge never blocks the notification: no API, a failing API, a missing half', async () => {
  for (const opts of [{ badge: 'none' }, { badge: 'set', badgeFails: true }, { badge: 'both', badgeFails: true }, { badge: 'clear' }, { badge: 'set' }]) {
    for (const n of [0, 5]) {
      const w = world(opts);
      await w.push(json({ kind: 'n', badge: n }));
      assert.equal(w.shown.length, 1, JSON.stringify([opts, n]));
    }
  }
});

test('a failing showNotification does not stop the badge and does not throw', async () => {
  for (const showFails of [true, 'sync']) {
    const w = world({ showFails });
    await w.push(json({ kind: 'p', badge: 2 }));
    assert.deepEqual(w.badges, [['set', 2]]);
  }
});

test('notificationclick closes it and focuses an open window', async () => {
  const focused = [];
  const w = world({ windows: [{ focus: async () => { focused.push(1); } }] });
  assert.equal(await w.click(), 1);
  assert.equal(focused.length, 1);
  assert.deepEqual(w.opened, []);
});

test('notificationclick opens / when no window is open', async () => {
  const w = world({ windows: [] });
  assert.equal(await w.click(), 1);
  assert.deepEqual(w.opened, ['/']);
});

test('the worker adds no way to read data or reach the network', () => {
  assert.doesNotMatch(src, /addEventListener\(\s*['"`]fetch/);
  for (const word of ['fetch', 'caches', 'indexedDB', 'importScripts', 'XMLHttpRequest', 'WebSocket', 'EventSource', 'localStorage', 'sendBeacon', 'postMessage', 'console']) {
    assert.equal(new RegExp(`\\b${word}\\b`).test(src), false, `sw.js must not use ${word}`);
  }
  const listeners = [...src.matchAll(/addEventListener\(\s*'([^']+)'/g)].map(m => m[1]);
  assert.deepEqual(listeners, ['push', 'notificationclick']);
  assert.ok(src.length < 4000, 'as small as possible');
});
