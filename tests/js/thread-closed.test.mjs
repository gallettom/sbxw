// The `thread-closed` event on the web page (assets/js/panes.js): a thread
// closed for good, by anyone, loses its panes and its row here, once; a pane
// closed on its own says nothing. The rules are loaded between their markers
// and run against fakes.
// Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const src = readFileSync(new URL('../../assets/js/panes.js', import.meta.url), 'utf8');
const rules = src.slice(src.indexOf('// BEGIN thread-closed rules'), src.indexOf('// END thread-closed rules'));
const main = readFileSync(new URL('../../assets/js/main.js', import.meta.url), 'utf8');

// A page with panes [{sandbox, thread}] and rows; afterThreadClosed is the real
// shape: remove the pane if there, forget the row, ask the list again.
function page(panes, rows) {
  const log = { toasts: [], fetches: [], sent: [] };
  const ctx = vm.createContext({
    panes, rows: new Set(rows), log,
    knownThreads: sb => [...new Set([...ctx.rows].filter(r => r.startsWith(`${sb}::`)).map(r => r.split('::')[1]).concat(ctx.panes.filter(p => p.sandbox === sb && p.thread).map(p => p.thread)))],
    afterThreadClosed(sb, t) {
      const at = ctx.panes.findIndex(p => p.sandbox === sb && p.thread === t);
      if (at >= 0) ctx.panes.splice(at, 1);
      ctx.rows.delete(`${sb}::${t}`);
      log.fetches.push(`${sb}/${t}`);
    },
    showToast: m => log.toasts.push(m),
  });
  vm.runInContext(`${rules}; globalThis.api = { onThreadClosed, closingHere };`, ctx);
  return ctx;
}
const ev = (sandbox, thread) => JSON.stringify({ sandbox, thread });

test('a thread closed elsewhere loses its pane and row here, with a quiet toast', () => {
  const p = page([{ sandbox: 'web', thread: 'api' }, { sandbox: 'web', thread: 'ui' }], ['web::api', 'web::ui']);
  assert.equal(p.api.onThreadClosed(ev('web', 'api')), true);
  assert.deepEqual(p.panes, [{ sandbox: 'web', thread: 'ui' }]);
  assert.ok(!p.rows.has('web::api') && p.rows.has('web::ui'));
  assert.deepEqual([...p.log.toasts], ['Thread api was closed']);
});

test('the page that asked also hears it: no second action beyond a no-op, no toast', () => {
  const p = page([{ sandbox: 'web', thread: 'api' }], ['web::api']);
  p.api.closingHere.add('web::api');
  assert.equal(p.api.onThreadClosed(ev('web', 'api')), true);
  assert.deepEqual([...p.log.toasts], []);
  // Its own afterThreadClosed then finds nothing left to remove, and an event heard again is ignored.
  assert.equal(p.api.onThreadClosed(ev('web', 'api')), false);
  assert.equal(p.panes.length, 0);
});

test('a thread this page never showed, or a bad event, is ignored', () => {
  const p = page([{ sandbox: 'web', thread: 'ui' }], ['web::ui']);
  assert.equal(p.api.onThreadClosed(ev('other', 'ui')), false);
  assert.equal(p.api.onThreadClosed(ev('web', 'nope')), false);
  for (const bad of ['', 'x', 'null', '{}', '{"sandbox":1,"thread":2}']) assert.equal(p.api.onThreadClosed(bad), false);
  assert.equal(p.panes.length, 1);
  assert.deepEqual([...p.log.toasts], []);
  assert.deepEqual([...p.log.fetches], []);
});

test('closing a pane alone sends nothing and listens to no event', () => {
  const pc = readFileSync(new URL('../../assets/js/pane-controls.js', import.meta.url), 'utf8');
  const body = pc.slice(pc.indexOf('function closePane('), pc.indexOf('\n}\n', pc.indexOf('function closePane(')));
  assert.doesNotMatch(body, /fetch\(|postJson|\/close/, 'closePane is a view change: no request');
  assert.match(main, /addEventListener\('thread-closed'/);
});
