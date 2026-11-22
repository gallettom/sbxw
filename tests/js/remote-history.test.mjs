// The phone conversation's history reading (assets/remote/remote-core.js):
// the page's own copy of a message until the history has it, the order of a
// turn whose reply came before its prompt, the epoch that says the numbering
// changed, a small first page, and older rows that keep the place.
// Every text is invented. Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const coreSrc = readFileSync(new URL('../../assets/remote/remote-core.js', import.meta.url), 'utf8');
const NAMES = ['rcWithPending', 'rcRows', 'rcMerge', 'rcPage', 'rcEpochChanged', 'rcRowKeys', 'rcAnchorPick', 'rcLogPlace', 'RC_FIRST_ROWS', 'RC_OLDER_ROWS'];
const ctx = { URLSearchParams, AbortController };
vm.runInNewContext(`${coreSrc}\n${NAMES.map(n => `this.${n} = ${n};`).join('\n')}`, ctx);
const rc = ctx;
const plain = v => JSON.parse(JSON.stringify(v));
const view = rows => plain(rows).map(r => `${r.kind}${r.pending ? `(${r.pending})` : ''}:${r.text ?? r.count}`);

const prompt = (seq, text) => ({ seq, at: seq, kind: 'user_prompt', text });
const reply = (seq, text, provisional) => ({ seq, at: seq, kind: 'assistant_text', text, ...(provisional ? { provisional: true } : {}) });

test('a sent message shows at once, and the history\'s copy replaces it, never twice', () => {
  const items = [prompt(1, 'earlier'), reply(2, 'earlier reply')];
  const pending = [{ id: 1, text: 'Merge the threads', files: ['plan.md'], after: 2, state: 'sending' }];
  let out = rc.rcWithPending(rc.rcRows(items), pending);
  assert.deepEqual(view(out.rows), ['me:earlier', 'agent:earlier reply', 'me(sending):Merge the threads']);
  assert.deepEqual(plain(out.rows[2].files), ['plan.md']);
  assert.deepEqual(plain(out.left), [1]);
  assert.equal(plain(rc.rcRowKeys(out.rows))[2], 'p1');

  // The history brings it (whitespace aside): one bubble, the real one.
  pending[0].state = 'sent';
  out = rc.rcWithPending(rc.rcRows([...items, prompt(3, 'Merge  the threads\n')]), pending);
  assert.deepEqual(view(out.rows), ['me:earlier', 'agent:earlier reply', 'me:Merge  the threads\n']);
  assert.deepEqual(plain(out.left), []);

  // Masked by the daemon (a name became its alias): still one bubble.
  out = rc.rcWithPending(rc.rcRows([...items, prompt(3, 'Merge the [alias] threads')]), pending);
  assert.deepEqual(view(out.rows), ['me:earlier', 'agent:earlier reply', 'me:Merge the [alias] threads']);
  assert.deepEqual(plain(out.left), []);

  // An older prompt with the same words is not it.
  out = rc.rcWithPending(rc.rcRows([prompt(1, 'Merge the threads'), reply(2, 'done')]), pending);
  assert.deepEqual(view(out.rows), ['me:Merge the threads', 'agent:done', 'me(sending):Merge the threads']);
});

test('a pending message stays before what arrived after it; a failed one says so and never matches', () => {
  const pending = [{ id: 7, text: 'Status?', after: 2, state: 'sent' }];
  const out = rc.rcWithPending(rc.rcRows([prompt(1, 'a'), reply(2, 'b'), reply(3, 'An early reply', true)]), pending);
  assert.deepEqual(view(out.rows), ['me:a', 'agent:b', 'me(sending):Status?', 'agent:An early reply']);
  const failed = rc.rcWithPending(rc.rcRows([prompt(1, 'a'), prompt(3, 'Status?')]), [{ id: 8, text: 'Status?', after: 2, state: 'failed' }]);
  assert.deepEqual(view(failed.rows), ['me:a', 'me(failed):Status?', 'me:Status?']);
  // After the numbering changed (`after` -1): its own text anywhere, else at the end.
  const lost = rc.rcWithPending(rc.rcRows([prompt(1, 'Status?'), reply(2, 'b')]), [{ id: 9, text: 'Status?', after: -1, state: 'sent' }]);
  assert.deepEqual(view(lost.rows), ['me:Status?', 'agent:b']);
  const waiting = rc.rcWithPending(rc.rcRows([prompt(1, 'x'), reply(2, 'b')]), [{ id: 9, text: 'Status?', after: -1, state: 'sent' }]);
  assert.deepEqual(view(waiting.rows), ['me:x', 'agent:b', 'me(sending):Status?']);
});

test('an early reply, then the transcript with the prompt: the prompt is drawn first', () => {
  // Stop's early copy came first (seq 3). The transcript then brought the
  // prompt (4) and the daemon moved the early copy to the end (5, see
  // crates/core/src/history.rs `provisional_last`); the old 3 is gone.
  let items = rc.rcMerge([prompt(1, 'a'), reply(2, 'b')], [reply(3, 'All merged.', true)]);
  items = rc.rcMerge(items, [prompt(4, 'Are the threads merged?'), reply(5, 'All merged.', true)]);
  assert.deepEqual(view(rc.rcRows(items)), ['me:a', 'agent:b', 'me:Are the threads merged?', 'agent:All merged.']);
  items = rc.rcMerge(items, [reply(6, 'All merged.')]);
  assert.deepEqual(view(rc.rcRows(items)), ['me:a', 'agent:b', 'me:Are the threads merged?', 'agent:All merged.']);
  // Rows come in seq order whatever order the pages came in.
  items = rc.rcMerge([reply(9, 'late')], [prompt(8, 'older page')]);
  assert.deepEqual(view(rc.rcRows(items)), ['me:older page', 'agent:late']);
});

test('the epoch: another numbering means start again; a daemon that does not say, never', () => {
  const p = plain(rc.rcPage({ page: { items: [], latest_seq: 4, epoch: 'a1', more: true } }));
  assert.equal(p.epoch, 'a1');
  assert.equal(p.more, true);
  assert.equal(plain(rc.rcPage({ page: { items: [] } })).more, false);
  assert.equal(rc.rcEpochChanged('a1', { epoch: 'b2' }), true);
  assert.equal(rc.rcEpochChanged('a1', { epoch: 'a1' }), false);
  assert.equal(rc.rcEpochChanged('a1', { epoch: '' }), false);
  assert.equal(rc.rcEpochChanged('', { epoch: 'a1' }), false);
});

test('the first page is small: about the last five exchanges', () => {
  assert.ok(rc.RC_FIRST_ROWS >= 10 && rc.RC_FIRST_ROWS <= 20, `${rc.RC_FIRST_ROWS}`);
  assert.ok(rc.RC_OLDER_ROWS > rc.RC_FIRST_ROWS);
  const src = readFileSync(new URL('../../assets/remote/remote.js', import.meta.url), 'utf8');
  assert.match(src, /api\.history\(sandbox, \{ limit: RC_FIRST_ROWS \}\)/, 'the first read asks for the small page');
  assert.match(src, /api\.history\(sandbox, \{ before: st\.nextBefore, limit: RC_OLDER_ROWS \}\)/);
  assert.doesNotMatch(src, /await api\.history\(sandbox\);/, 'never the default page of 100');
});

test('older rows prepended keep the row in view where it was', () => {
  // Scrolled up to 25 px: row s40 is the first in view, 5 px below its top.
  const before = [{ key: 's39', top: 0, bottom: 20 }, { key: 's40', top: 30, bottom: 150 }, { key: 's41', top: 160, bottom: 400 }];
  const anchor = plain(rc.rcAnchorPick(before, 25));
  assert.deepEqual(anchor, { key: 's40', offset: 5 });
  // 900 px of older rows come in above everything.
  const after = before.map(r => ({ ...r, top: r.top + 900, bottom: r.bottom + 900 }));
  after.unshift({ key: 's10', top: 0, bottom: 890 });
  const top = rc.rcLogPlace(false, anchor, after, 25, 2400, 600);
  assert.equal(top, 925, 's40 is 5 px below the view\'s top again');
});
