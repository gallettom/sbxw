// The web UI's chat reader (assets/js/chat-core.js): a session's reply is
// announced once when its turn ends, at idle or while it waits on background
// work of its own (the interim reply). The rule itself is checked against the
// case table the desktop's `reply_ready` reads too, so the two cannot drift.
// Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const src = readFileSync(new URL('../../assets/js/chat-core.js', import.meta.url), 'utf8');
const ctx = {};
vm.runInNewContext(`${src}\nthis.chatNew = chatNew; this.chatIngest = chatIngest; this.chatReplyReady = chatReplyReady;`, ctx);
const { chatNew, chatIngest, chatReplyReady } = ctx;
const cases = JSON.parse(readFileSync(new URL('../fixtures/reply-ready.json', import.meta.url), 'utf8')).cases;

test('the rule matches the shared cases', () => {
  for (const c of cases) assert.equal(chatReplyReady(c.in_turn, c.info, c.last), c.ready, c.name);
});

const bg = { shells: 1, agents: 0, other: 0, wakeups: 0 };
const replies = (c, events) => events.map(e => chatIngest(c, e)?.reply ?? null).filter(Boolean);

test('an interim reply at a busy stop, then the final one, each once', () => {
  const c = chatNew();
  const got = replies(c, [
    { state: 'working' },
    { state: 'working', reply: 'Build started.', background: bg },
    { state: 'working', reply: 'Build started.', background: bg },
    { state: 'working', reply: 'Build started.', background: { ...bg, shells: 2 } },
    { state: 'working' },
    { state: 'idle', reply: 'Build passed.' },
    { state: 'idle', reply: 'Build passed.' },
  ]);
  assert.deepEqual(got, ['Build started.', 'Build passed.']);
  assert.deepEqual([...c.log].map(m => m.text), ['Build started.', 'Build passed.']);
});

test('the wait running out is idle with the same words: not again', () => {
  const c = chatNew();
  assert.deepEqual(replies(c, [
    { state: 'working' },
    { state: 'working', reply: 'Waiting.', background: bg },
    { state: 'idle', reply: 'Waiting.' },
  ]), ['Waiting.']);
});

test('without background: as before', () => {
  const c = chatNew();
  assert.deepEqual(replies(c, [
    { state: 'working' },
    { state: 'working', reply: 'not yet' },
    { state: 'idle', reply: 'Done.' },
    { state: 'idle', reply: 'Done.' },
    { state: 'working' },
    { state: 'idle', reply: 'Done.' },
  ]), ['Done.', 'Done.'], 'a new turn with the same words is a new answer');
  assert.equal(chatIngest(c, { state: 'attention', activity: 'Allow?' })?.asked, 'Allow?');
});

test('a first look at a busy session is not news', () => {
  const c = chatNew();
  chatIngest(c, { state: 'working', last_input: 'build', reply: 'Build started.', background: bg }, true);
  assert.equal(chatIngest(c, { state: 'working', reply: 'Build started.', background: bg }), null);
  assert.equal(chatIngest(c, { state: 'idle', reply: 'Build passed.' })?.reply, 'Build passed.');
});
