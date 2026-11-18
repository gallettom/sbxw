// The input hold of assets/js/panes.js (Jarvis's high-risk proposals), without
// a browser: its rules are loaded on their own, between their BEGIN and END
// markers, and run against a fake pane socket.
// Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const src = readFileSync(new URL('../../assets/js/panes.js', import.meta.url), 'utf8');
const rules = src.slice(src.indexOf('// BEGIN pane-hold rules'), src.indexOf('// END pane-hold rules'));

function load() {
  const ctx = vm.createContext({});
  vm.runInContext(`${rules}
    globalThis.hold = on => { paneInputHeld = on; };
    globalThis.api = { paneFrameAllowed, paneHoldStops, holdPaneSocket, PANE_HOLD_EVENTS };`, ctx);
  return ctx;
}

function fakeSocket() {
  const sent = [];
  return { sent, send(d) { sent.push(d); } };
}

test('a held pane socket carries no input, only its own control messages', () => {
  const ctx = load();
  const ws = ctx.api.holdPaneSocket(fakeSocket());
  const key = new Uint8Array([0x61]);
  ws.send(key);
  ctx.hold(true);
  ws.send(new Uint8Array([0x62]));                 // a key, a paste, a mouse report
  ws.send(new Uint8Array([0x1b, 0x5b, 0x4d]));     // mouse reporting
  ws.send(JSON.stringify({ type: 'resize', cols: 80, rows: 24 }));
  ctx.hold(false);
  ws.send(new Uint8Array([0x63]));
  assert.equal(ws.sent.length, 3);
  assert.equal(ws.sent[0], key);
  assert.equal(typeof ws.sent[1], 'string', 'the resize went through the hold');
  assert.deepEqual([...ws.sent[2]], [0x63], 'freed, input flows again');
});

test('events aimed at the terminals are stopped while held, and files anywhere', () => {
  const ctx = load();
  const { paneHoldStops } = ctx.api;
  for (const t of ['keydown', 'paste', 'mousedown', 'wheel', 'drop', 'contextmenu', 'click']) {
    assert.equal(paneHoldStops(t, true, false), false, `${t}: not held, untouched`);
  }
  ctx.hold(true);
  for (const t of ['keydown', 'keypress', 'beforeinput', 'paste', 'mousedown', 'mouseup', 'click', 'wheel', 'contextmenu', 'dragover', 'drop', 'pointerdown', 'touchstart']) {
    assert.equal(paneHoldStops(t, true, false), true, `${t} on the terminals' area or its scrim`);
  }
  // Jarvis's dialog and input stay usable...
  for (const t of ['keydown', 'paste', 'click', 'input']) assert.equal(paneHoldStops(t, false, false), false, t);
  // ...but a pasted or dropped file is stopped wherever it lands: an image
  // paste uploads to the focused pane (sandboxes.js).
  assert.equal(paneHoldStops('paste', false, true), true);
  assert.equal(paneHoldStops('drop', false, true), true);
  assert.equal(paneHoldStops('mousemove', true, false), false, 'a move is no input; its mouse report is held at the socket');
});

test('every pane socket goes through the hold', () => {
  const made = [...src.matchAll(/pane\.ws = new WebSocket\(/g)];
  assert.equal(made.length, 1, 'one place makes a pane socket');
  const at = made[0].index;
  assert.match(src.slice(at, at + 400), /holdPaneSocket\(pane\.ws\)/);
});
