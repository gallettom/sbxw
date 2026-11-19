// The status hook's transcript reader (assets/status-hook.js, between its
// BEGIN history and END history markers), against scripted transcripts in a
// temporary directory: one that grows, one truncated, unknown entries, a
// bounded read that catches up, a backfill from the tail, an oversized line.
// Every text is invented.
// Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, mkdtempSync, writeFileSync, appendFileSync, readdirSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import vm from 'node:vm';

const src = readFileSync(new URL('../../assets/status-hook.js', import.meta.url), 'utf8');
const block = src.slice(src.indexOf('// BEGIN history'), src.indexOf('// END history'));
const require = createRequire(import.meta.url);

function load() {
  const ctx = vm.createContext({ require, fs: require('fs'), Buffer });
  vm.runInContext(`${block}
    globalThis.api = { readTail, entryItems, stateAfter, saveState, loadState, transcriptProblem, toolInput };`, ctx);
  // Values come back through JSON: the context's arrays are another realm's.
  const api = {};
  for (const [k, f] of Object.entries(ctx.api)) {
    api[k] = (...a) => {
      const r = f(...a);
      return r === undefined ? r : JSON.parse(JSON.stringify(r));
    };
  }
  return api;
}

const SID = '0f9e8d7c-aaaa-bbbb-cccc-000000000001';
const line = (o) => JSON.stringify({ sessionId: SID, timestamp: '2026-11-19T10:00:00Z', ...o }) + '\n';
const user = (text) => line({ type: 'user', message: { role: 'user', content: text } });
const said = (...blocks) => line({ type: 'assistant', message: { role: 'assistant', content: blocks } });

function transcript() {
  const dir = mkdtempSync(join(tmpdir(), 'sbxw-hook-test-'));
  return { dir, path: join(dir, SID + '.jsonl') };
}

// What the hook keeps after a 200: the daemon accepted up to `to`.
const accept = (api, piece) => api.stateAfter(200, { next: piece.to }, piece);

test('a growing transcript is read incrementally, complete lines only', () => {
  const api = load();
  const t = transcript();
  writeFileSync(t.path, user('Fix the basket') + said({ type: 'thinking', thinking: 'hidden' }, { type: 'text', text: 'On it.' }));
  let piece = api.readTail(t.path, {}, SID);
  assert.equal(piece.from, 0);
  assert.equal(piece.reset, false);
  assert.deepEqual(piece.items.map((i) => i.t), ['user', 'assistant']);
  assert.ok(!JSON.stringify(piece.items).includes('hidden'), 'thinking is never read out');
  let st = accept(api, piece);

  // Nothing new: nothing to post.
  piece = api.readTail(t.path, st, SID);
  assert.equal(piece.to, piece.from);

  // A tool call, its result, and a line still being written.
  const half = said({ type: 'text', text: 'Next step' });
  appendFileSync(t.path,
    said({ type: 'tool_use', id: 'tu1', name: 'Edit', input: { file_path: '/w/a.rs', old_string: 'a\nb', new_string: 'c' } }) +
    line({ type: 'user', message: { content: [{ type: 'tool_result', tool_use_id: 'tu1', content: 'x'.repeat(5000) }] } }) +
    half.slice(0, 10));
  piece = api.readTail(t.path, st, SID);
  assert.equal(piece.from, st.off);
  assert.deepEqual(piece.items.map((i) => i.t), ['tool_use', 'tool_result']);
  assert.deepEqual(piece.items[0].input, { file_path: '/w/a.rs', lines_old: 2, lines_new: 1 }, 'the edit\'s text stays in the sandbox');
  assert.equal(piece.items[1].head.length, 2000);
  assert.equal(piece.items[1].size, 5000);
  st = accept(api, piece);
  appendFileSync(t.path, half.slice(10));
  piece = api.readTail(t.path, st, SID);
  assert.deepEqual(piece.items.map((i) => i.text), ['Next step']);
});

test('a truncated or replaced transcript starts again with a reset', () => {
  const api = load();
  const t = transcript();
  writeFileSync(t.path, user('one') + user('two'));
  const st = accept(api, api.readTail(t.path, {}, SID));
  writeFileSync(t.path, user('new'));
  const piece = api.readTail(t.path, st, SID);
  assert.equal(piece.reset, true);
  assert.equal(piece.from, 0);
  assert.deepEqual(piece.items.map((i) => i.text), ['new']);
});

test('unknown entries, broken lines and sidechains are skipped, never fatal', () => {
  const api = load();
  const t = transcript();
  writeFileSync(t.path,
    line({ type: 'file-history-snapshot', snapshot: {} }) +
    '{not json\n' +
    line({ type: 'a-type-from-the-future', message: { content: 'x' } }) +
    said({ type: 'a-block-from-the-future', data: 1 }, { type: 'redacted_thinking', data: 'zz' }) +
    line({ type: 'assistant', isSidechain: true, message: { content: [{ type: 'text', text: 'subagent' }] } }) +
    line({ type: 'system', subtype: 'compact_boundary' }) +
    line({ type: 'user', isCompactSummary: true, message: { content: 'Summary so far' } }) +
    line({ type: 'user', message: { content: [{ type: 'image', source: { data: 'iVBORw0KGgo' } }, { type: 'text', text: 'see this' }] } }));
  const piece = api.readTail(t.path, {}, SID);
  assert.deepEqual(piece.items.map((i) => i.t), ['compact', 'user', 'image', 'user']);
  assert.equal(piece.items[1].compact_summary, true);
  assert.ok(!JSON.stringify(piece.items).includes('iVBOR'), 'no image data');
  assert.equal(api.readTail(join(t.dir, 'missing.jsonl'), {}, SID), null, 'an unreadable file: nothing, no throw');
});

test('a read is bounded and catches up over the next events', () => {
  const api = load();
  const t = transcript();
  let body = '';
  for (let n = 0; n < 50; n++) body += user('message ' + n + ' ' + 'y'.repeat(100));
  writeFileSync(t.path, body);
  let st = {};
  const seen = [];
  for (let calls = 0; calls < 30 && seen.length < 50; calls++) {
    const piece = api.readTail(t.path, st, SID, { read: 1024 });
    seen.push(...piece.items.map((i) => i.text.split(' ').slice(0, 2).join(' ')));
    st = accept(api, piece);
  }
  assert.equal(seen.length, 50);
  assert.equal(seen[49], 'message 49');
  // Bounded by items too.
  const few = api.readTail(t.path, {}, SID, { items: 5 });
  assert.equal(few.items.length, 5);
  assert.ok(few.to < body.length);
});

test('a first read starts from the tail, on a line boundary, and says so', () => {
  const api = load();
  const t = transcript();
  let body = '';
  for (let n = 0; n < 100; n++) body += user('m' + n);
  writeFileSync(t.path, body);
  const piece = api.readTail(t.path, {}, SID, { backfill: 300 });
  assert.equal(piece.reset, true);
  assert.ok(piece.from > 0);
  assert.equal(piece.items.at(-1).text, 'm99');
  assert.ok(piece.items.every((i) => /^m\d+$/.test(i.text)), 'no half line');
  // The daemon asked for a reset (409, next 0): the next read is from the tail.
  assert.deepEqual(api.stateAfter(409, { next: 0 }, piece), { reset: true });
  assert.equal(api.stateAfter(500, {}, piece), null, 'a failure keeps the state: tried again');
});

test('a line longer than a read is reported from its head and skipped', () => {
  const api = load();
  const t = transcript();
  const big = line({ type: 'user', message: { content: [{ type: 'tool_result', tool_use_id: 'tu9', is_error: true, content: 'z'.repeat(20000) }] } });
  writeFileSync(t.path, big + user('after'));
  let piece = api.readTail(t.path, {}, SID, { read: 4096 });
  assert.equal(piece.items.length, 1);
  assert.equal(piece.items[0].t, 'tool_result');
  assert.equal(piece.items[0].is_error, true);
  assert.equal(piece.items[0].head, '');
  assert.equal(piece.to, Buffer.byteLength(big));
  piece = api.readTail(t.path, accept(api, piece), SID, { read: 4096 });
  assert.deepEqual(piece.items.map((i) => i.text), ['after']);
});

test('only its own session\'s transcript, and the state file holds offsets only', () => {
  const api = load();
  assert.equal(api.transcriptProblem({ session_id: SID, transcript_path: '/home/agent/.claude/projects/x/' + SID + '.jsonl' }), '');
  assert.equal(api.transcriptProblem({ session_id: SID, transcript_path: '/home/agent/.env' }), 'not_this_sessions');
  assert.equal(api.transcriptProblem({ session_id: '../etc', transcript_path: '/x/../etc.jsonl' }), 'no_session');
  assert.equal(api.transcriptProblem({ session_id: SID }), 'no_path');
  const dir = mkdtempSync(join(tmpdir(), 'sbxw-hook-state-'));
  api.saveState(dir, SID, { off: 10, ino: 5, text: 'never stored', items: [{ t: 'user', text: 'no' }] });
  assert.deepEqual(readdirSync(dir), [SID + '.json']);
  assert.deepEqual(JSON.parse(readFileSync(join(dir, SID + '.json'), 'utf8')), { off: 10, ino: 5 });
  assert.deepEqual(api.loadState(dir, 'nobody'), {});
});

test('a resumed transcript names the session it came from', () => {
  const api = load();
  const t = transcript();
  writeFileSync(t.path, JSON.stringify({ type: 'user', sessionId: 'aaaa-old', message: { content: 'before' } }) + '\n' + user('after'));
  assert.equal(api.readTail(t.path, {}, SID).prior, 'aaaa-old');
});

test('reading a full chunk stays well under the hook\'s budget', () => {
  const api = load();
  const t = transcript();
  let body = '';
  while (body.length < 256 * 1024) body += said({ type: 'text', text: 'w'.repeat(500) }, { type: 'tool_use', id: 't', name: 'Bash', input: { command: 'ls' } });
  writeFileSync(t.path, body);
  const t0 = process.hrtime.bigint();
  const piece = api.readTail(t.path, {}, SID);
  const ms = Number(process.hrtime.bigint() - t0) / 1e6;
  assert.ok(piece.items.length > 100);
  assert.ok(ms < 100, `took ${ms} ms`);
});
