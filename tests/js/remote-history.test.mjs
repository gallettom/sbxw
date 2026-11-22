// The phone conversation's history reading (assets/remote/remote-core.js):
// the page's own copy of a message until the history has it, the order of a
// turn whose reply came before its prompt, the epoch that says the numbering
// changed, a small first page, and older rows that keep the place.
// Every text is invented. Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

// RC_CORE=<file> runs the same tests on another copy of the page's core
// (an older one, to see a test fail before its fix).
const coreSrc = readFileSync(process.env.RC_CORE || new URL('../../assets/remote/remote-core.js', import.meta.url), 'utf8');
const NAMES = ['rcWithPending', 'rcRows', 'rcMerge', 'rcPage', 'rcEpochChanged', 'rcRowKeys', 'rcAnchorPick', 'rcLogPlace', 'RC_FIRST_ROWS', 'RC_OLDER_ROWS',
  'rcPendingSent', 'rcPendingRenumber', 'rcPendingAge', 'rcSameText', 'RC_CATCH_UP_MS', 'RC_PENDING_WAIT_MS', 'RC_PENDING_TEXT'];
const ctx = { URLSearchParams, AbortController };
vm.runInNewContext(`${coreSrc}\n${NAMES.map(n => `try { this.${n} = ${n}; } catch (_) {}`).join('\n')}`, ctx);
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
  assert.deepEqual(view(out.rows), ['me:Merge the threads', 'agent:done', 'me(sent):Merge the threads']);
});

test('a pending message stays before what arrived after it; a failed one says so and never matches', () => {
  const pending = [{ id: 7, text: 'Status?', after: 2, state: 'sent' }];
  const out = rc.rcWithPending(rc.rcRows([prompt(1, 'a'), reply(2, 'b'), reply(3, 'An early reply', true)]), pending);
  assert.deepEqual(view(out.rows), ['me:a', 'agent:b', 'me(sent):Status?', 'agent:An early reply']);
  const failed = rc.rcWithPending(rc.rcRows([prompt(1, 'a'), prompt(3, 'Status?')]), [{ id: 8, text: 'Status?', after: 2, state: 'failed' }]);
  assert.deepEqual(view(failed.rows), ['me:a', 'me(failed):Status?', 'me:Status?']);
  // After the numbering changed (`after` -1): its own text anywhere, else at the end.
  const lost = rc.rcWithPending(rc.rcRows([prompt(1, 'Status?'), reply(2, 'b')]), [{ id: 9, text: 'Status?', after: -1, state: 'sent' }]);
  assert.deepEqual(view(lost.rows), ['me:Status?', 'agent:b']);
  const waiting = rc.rcWithPending(rc.rcRows([prompt(1, 'x'), reply(2, 'b')]), [{ id: 9, text: 'Status?', after: -1, state: 'sent' }]);
  assert.deepEqual(view(waiting.rows), ['me:x', 'agent:b', 'me(sent):Status?']);
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

// ── The bubble that stayed on "Sending…" ──────────────────────────────────
// The page after a Send, as remote.js runs it: the bubble at once, the
// daemon's answer, then a history read at each nudge of the event stream and
// at each RC_CATCH_UP_MS after the answer (none on a core without it). The
// daemon's log is a list of steps `{ t, items, epoch }` (a new epoch drops
// the rows: a restart, the hook's backfill). Returns the rows at the end.
function simulate({ send, answer, steps, nudges, epoch0 = 'e1', held = [prompt(1, 'earlier'), reply(2, 'earlier reply')] }) {
  const log = { epoch: epoch0, items: held.slice() };
  const page = { epoch: epoch0, items: held.slice(), latestSeq: Math.max(...held.map(i => i.seq)), pending: [] };
  const mine = { id: 1, text: send.text, files: send.files || [], after: page.latestSeq, epoch: page.epoch, state: 'sending' };
  page.pending.push(mine);
  const ANSWERED = 1500;
  const reads = new Set(nudges);
  for (const ms of rc.RC_CATCH_UP_MS || []) reads.add(ANSWERED + ms);
  const times = [...new Set([...steps.map(s => s.t), ...reads, ANSWERED, 200000])].sort((a, b) => a - b);
  const render = () => {
    const shown = rc.rcWithPending(rc.rcRows(page.items), page.pending);
    page.pending = page.pending.filter(p => shown.left.includes(p.id));
    return shown.rows;
  };
  let rows = render();
  for (const t of times) {
    for (const s of steps.filter(x => x.t === t)) {
      if (s.epoch && s.epoch !== log.epoch) { log.epoch = s.epoch; log.items = []; }
      log.items.push(...s.items);
    }
    if (t === ANSWERED) {
      // The daemon's answer: its key is the log's newest seq before Return.
      const data = { ok: true, masked: answer.masked || 0, text: answer.text, after: answer.after, epoch: log.epoch };
      if (rc.rcPendingSent) rc.rcPendingSent(mine, data, page.epoch, t); else mine.state = 'sent';
    }
    if (reads.has(t)) {
      if (rc.rcPendingAge) rc.rcPendingAge(page.pending, t);
      if (log.epoch !== page.epoch) {
        // loadLast: the last page of the new numbering.
        page.items = log.items.slice();
        page.epoch = log.epoch;
        if (rc.rcPendingRenumber) rc.rcPendingRenumber(page.pending, page.epoch); else for (const p of page.pending) p.after = -1;
      } else {
        page.items = rc.rcMerge(page.items, log.items.filter(i => i.seq > page.latestSeq));
      }
      page.latestSeq = Math.max(0, ...page.items.map(i => i.seq));
    }
    rows = render();
  }
  return view(rows);
}

// The four kinds of message from the screenshot's report: what the page
// shows in its bubble, what the daemon typed (and answers), what the
// history's row holds.
const KINDS = {
  'plain text': { send: { text: 'Status?' }, answer: { text: 'Status?' }, row: { text: 'Status?' } },
  'dictated text': { send: { text: 'Ship it now,\nthen tell me' }, answer: { text: 'Ship it now,\nthen tell me' }, row: { text: 'Ship it now,\nthen tell me' } },
  'text with attachments': { send: { text: 'See the plan', files: ['plan.md'] }, answer: { text: 'See the plan' }, row: { text: 'See the plan', files: ['plan.md'] } },
  'a masked word': { send: { text: 'Write to Jeanne Martin' }, answer: { text: 'Write to [person-1]', masked: 1 }, row: { text: 'Write to [person-1]' } },
};
const userRow = (seq, k) => ({ seq, at: seq, kind: 'user_prompt', text: k.row.text, ...(k.row.files ? { files: k.row.files } : {}) });
const expectRow = k => `me:${k.row.text}`;

for (const [name, k] of Object.entries(KINDS)) {
  test(`${name}: the stream's nudges never come, the page reads anyway and the bubble gives way`, () => {
    const rows = simulate({
      send: k.send,
      answer: { ...k.answer, after: 2 },
      steps: [{ t: 2000, items: [userRow(3, k), { seq: 4, at: 4, kind: 'tool_call' }] }, { t: 2600, items: [reply(5, 'Done.')] }],
      nudges: [],
    });
    assert.deepEqual(rows, ['me:earlier', 'agent:earlier reply', expectRow(k), 'steps:1', 'agent:Done.']);
  });

  test(`${name}: the numbering changes between Send and the row, the bubble still gives way`, () => {
    // The hook's backfill reset the log after the Return: new seqs, new epoch.
    const rows = simulate({
      send: k.send,
      answer: { ...k.answer, after: 2 },
      steps: [{ t: 2000, epoch: 'e2', items: [prompt(1, 'earlier'), reply(2, 'earlier reply'), userRow(3, k), reply(4, 'Done.')] }],
      nudges: [2100],
    });
    assert.deepEqual(rows, ['me:earlier', 'agent:earlier reply', expectRow(k), 'agent:Done.']);
  });
}

test('the daemon\'s key: a prompt typed at the Mac in between is not the phone\'s', () => {
  // The page held up to 2; the Mac's prompt (3) and its reply (4) came in
  // before the phone's Return (the daemon's key: 4); the phone's prompt is 5.
  const p = { id: 1, text: 'Status?', after: 2, epoch: 'e1', state: 'sending' };
  rc.rcPendingSent(p, { ok: true, masked: 0, text: 'Status?', after: 4, epoch: 'e1' }, 'e1', 1000);
  const items = [prompt(1, 'a'), reply(2, 'b'), prompt(3, 'From the Mac'), reply(4, 'ok')];
  let out = rc.rcWithPending(rc.rcRows(items), [p]);
  assert.deepEqual(view(out.rows), ['me:a', 'agent:b', 'me:From the Mac', 'agent:ok', 'me(sent):Status?']);
  out = rc.rcWithPending(rc.rcRows([...items, prompt(5, 'Status?')]), [p]);
  assert.deepEqual(view(out.rows), ['me:a', 'agent:b', 'me:From the Mac', 'agent:ok', 'me:Status?']);
  assert.deepEqual(plain(out.left), []);
  // A key of another numbering than the rows held waits for that numbering.
  const q = { id: 2, text: 'x', after: 2, epoch: 'e1', state: 'sending' };
  rc.rcPendingSent(q, { ok: true, text: 'x', after: 9, epoch: 'e2' }, 'e1', 1000);
  assert.equal(q.after, 2);
  rc.rcPendingRenumber([q], 'e2');
  assert.equal(q.after, 9);
  rc.rcPendingRenumber([q], 'e3');
  assert.equal(q.after, -1);
  // A daemon that gives no key leaves the page's own anchor.
  const r = { id: 3, text: 'y', after: 2, state: 'sending' };
  rc.rcPendingSent(r, { ok: true, masked: 0 }, 'e1', 1000);
  assert.equal(r.after, 2);
  assert.equal(r.state, 'sent');
});

test('a bubble with no row after RC_PENDING_WAIT_MS says it was sent and waits; it is still replaced later', () => {
  assert.equal(rc.RC_PENDING_TEXT.sending, 'Sending…');
  assert.equal(rc.RC_PENDING_TEXT.waiting, 'Sent, waiting for the agent');
  assert.equal(rc.RC_CATCH_UP_MS.at(-1) >= rc.RC_PENDING_WAIT_MS, true, 'a last read at the bound');
  const p = { id: 1, text: 'Status?', after: 2, state: 'sending' };
  rc.rcPendingSent(p, { ok: true, text: 'Status?', after: 2, epoch: 'e1' }, 'e1', 1000);
  assert.equal(rc.rcPendingAge([p], 1000 + rc.RC_PENDING_WAIT_MS - 1), false);
  assert.equal(rc.rcPendingAge([p], 1000 + rc.RC_PENDING_WAIT_MS), true);
  let out = rc.rcWithPending(rc.rcRows([prompt(1, 'a'), reply(2, 'b')]), [p]);
  assert.deepEqual(view(out.rows), ['me:a', 'agent:b', 'me(waiting):Status?']);
  out = rc.rcWithPending(rc.rcRows([prompt(1, 'a'), reply(2, 'b'), prompt(3, 'Status?')]), [p]);
  assert.deepEqual(view(out.rows), ['me:a', 'agent:b', 'me:Status?']);
  // A failed one never ages into waiting.
  const f = { id: 2, text: 'x', state: 'failed', sentAt: 1 };
  assert.equal(rc.rcPendingAge([f], 10 ** 9), false);
});

test('the same words: dictation\'s no-break spaces, new lines, composed accents', () => {
  assert.equal(rc.rcSameText('Ship it now,\r\nthen', 'Ship it now, then'), true);
  assert.equal(rc.rcSameText('Café ouvert', 'Café ouvert'), true);
  assert.equal(rc.rcSameText('a b', 'a c'), false);
});

test('after the numbering changed, the newest row with its words is the one; else the same files', () => {
  const p = { id: 1, text: 'yes', after: -1, state: 'sent' };
  const out = rc.rcWithPending(rc.rcRows([prompt(1, 'yes'), reply(2, 'ok'), prompt(3, 'yes')]), [p]);
  assert.deepEqual(view(out.rows), ['me:yes', 'agent:ok', 'me:yes']);
  assert.deepEqual(plain(out.left), []);
  const f = { id: 2, text: '', files: ['shot.png'], after: -1, state: 'sent', match: '' };
  const rows = rc.rcRows([{ seq: 1, at: 1, kind: 'user_prompt', text: 'Look', files: ['shot.png'] }]);
  assert.deepEqual(plain(rc.rcWithPending(rows, [f]).left), []);
});
