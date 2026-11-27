// Which sidebar row and which pane are highlighted, given the visible panes
// and the focused one (assets/js/panes.js, between its "pane highlight pure"
// markers), and where a click on a row lands. Invented sandboxes.
// Run: node --test tests/js/pane-highlight.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const src = readFileSync(new URL('../../assets/js/panes.js', import.meta.url), 'utf8');
const pure = src.slice(src.indexOf('// BEGIN pane highlight pure'), src.indexOf('// END pane highlight pure'));
const ctx = vm.createContext({});
vm.runInContext(`const MONITOR_SANDBOX = '__host__';\n${pure}\nglobalThis.api = { focusHighlight, rowClickTarget, connectTarget };`, ctx);
const { focusHighlight, rowClickTarget, connectTarget } = ctx.api;
// Objects made in the vm compare by shape here, not by prototype.
const plain = v => JSON.parse(JSON.stringify(v));

const pane = (sandbox, mode = 'claude', thread = null, groupColor = null) => ({ sandbox, mode, thread, groupColor });
const FOCUS = 'var(--c-focus)';

test('the focused pane lights its own sandbox row, in the focus blue when ungrouped', () => {
  const vis = [pane('alpha'), pane('beta'), pane('gamma')];
  assert.deepEqual(plain(focusHighlight(vis, 1)), { pane: 1, row: 'beta', thread: null, color: FOCUS });
  assert.deepEqual(plain(focusHighlight(vis, 2)), { pane: 2, row: 'gamma', thread: null, color: FOCUS });
});

test('a click on a row whose sandbox is in another pane focuses that pane and opens nothing', () => {
  const vis = [pane('alpha'), pane('beta'), pane('gamma')];
  assert.deepEqual(plain(rowClickTarget(vis, 0, 'gamma')), { pane: 2, connect: false });
  // Then that pane is the focused one, and its row the lit one.
  assert.equal(focusHighlight(vis, 2).row, 'gamma');
});

test('a click on a row in no pane opens it in the focused pane, which is marked', () => {
  const vis = [pane('alpha'), pane('beta')];
  assert.deepEqual(plain(rowClickTarget(vis, 1, 'delta')), { pane: 1, connect: true });
});

test('a click on the focused pane\'s own row stays on it (it reconnects, as before)', () => {
  const vis = [pane('alpha'), pane('beta')];
  assert.deepEqual(plain(rowClickTarget(vis, 0, 'alpha')), { pane: 0, connect: true });
});

test('Bash and Claude of one sandbox are one session: a row click finds the Bash pane', () => {
  const vis = [pane('alpha'), pane('beta', 'bash')];
  assert.deepEqual(plain(rowClickTarget(vis, 0, 'beta')), { pane: 1, connect: false });
});

test('a sandbox in two panes: the focused one is lit, in its group colour, and the row is the sandbox\'s', () => {
  const vis = [pane('alpha', 'claude', null, 'var(--c-group-1)'), pane('alpha', 'thread', 'fix-a', 'var(--c-group-1)'), pane('beta')];
  assert.deepEqual(plain(focusHighlight(vis, 0)), { pane: 0, row: 'alpha', thread: null, color: 'var(--c-group-1)' });
  assert.deepEqual(plain(focusHighlight(vis, 1)), { pane: 1, row: 'alpha', thread: 'fix-a', color: 'var(--c-group-1)' });
  // The row opens the main session: from pane 2 it leads to pane 0, not to the thread's pane.
  assert.deepEqual(plain(rowClickTarget(vis, 2, 'alpha')), { pane: 0, connect: false });
});

test('a thread alone on screen: a click on its sandbox row opens the main session in the focused pane', () => {
  const vis = [pane('beta'), pane('alpha', 'thread', 'fix-a')];
  assert.deepEqual(plain(rowClickTarget(vis, 0, 'alpha')), { pane: 0, connect: true });
});

test('a thread\'s Bash names its thread: its session row is the one lit', () => {
  const vis = [pane('alpha', 'bash', 'fix-a')];
  assert.equal(focusHighlight(vis, 0).thread, 'fix-a');
});

test('a chat sandbox is a row like any other', () => {
  const vis = [pane('chat-0412'), pane('alpha')];
  assert.deepEqual(plain(focusHighlight(vis, 0)), { pane: 0, row: 'chat-0412', thread: null, color: FOCUS });
  assert.deepEqual(plain(rowClickTarget(vis, 1, 'chat-0412')), { pane: 0, connect: false });
});

test('an empty pane, the monitor, no pane: the pane is marked or not, no row is', () => {
  assert.deepEqual(plain(focusHighlight([pane(null, 'claude')], 0)), { pane: 0, row: null, thread: null, color: FOCUS });
  assert.deepEqual(plain(focusHighlight([pane('__host__', 'monitor')], 0)), { pane: 0, row: null, thread: null, color: FOCUS });
  assert.deepEqual(plain(focusHighlight([], 0)), { pane: -1, row: null, thread: null, color: FOCUS });
  assert.deepEqual(plain(focusHighlight([pane('alpha')], 3)), { pane: -1, row: null, thread: null, color: FOCUS });
});

test('connectTarget keeps what connectPane did: thread sticks to Bash, not to another sandbox', () => {
  assert.deepEqual(plain(connectTarget(pane('alpha', 'bash', 'fix-a'), 'alpha')), { mode: 'bash', thread: 'fix-a' });
  assert.deepEqual(plain(connectTarget(pane('alpha', 'bash', 'fix-a'), 'beta')), { mode: 'bash', thread: null });
  assert.deepEqual(plain(connectTarget(pane('alpha', 'thread', 'fix-a'), 'beta')), { mode: 'claude', thread: null });
  assert.deepEqual(plain(connectTarget(pane('alpha'), 'alpha', 'thread', 'fix-b')), { mode: 'thread', thread: 'fix-b' });
  assert.equal(connectTarget(pane('alpha'), 'alpha', 'thread'), null);
  assert.deepEqual(plain(connectTarget(pane('alpha'), '__host__')), { mode: 'monitor', thread: null });
});
