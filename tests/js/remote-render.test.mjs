// The phone conversation's drawing, in the stand-in DOM (remote-dom.mjs):
// a render of N rows gives the same DOM as a page opened on them from
// scratch, and a new row, a steps fold or a leaders event does not touch
// the rows that did not change (thread phone-freeze-audit).
// Every text is invented. Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { openBig, conversation, reply } from './remote-perf.mjs';

const rowsOf = dom => dom.$('rows').children;
// The DOM of the rows, without the rise-in class (it marks what arrived
// since the last look, so a page opened fresh has none).
const html = dom => dom.serialize(dom.$('rows')).replace(/ rc-new\b/g, '');
const newRow = (seq, text) => ({ seq, at: 1763650000000 + seq, kind: 'assistant_text', text });

test('a render of N rows: the same DOM as a page opened on them', async () => {
  for (const n of [50, 300]) {
    const items = conversation(n);
    const last = items[items.length - 1].seq;
    const extra = [newRow(last + 1, reply(5001)), { seq: last + 2, at: 1, kind: 'user_prompt', text: 'and the tests?', files: ['a.png'], images: ['ab'.repeat(16)] }, { seq: last + 3, at: 2, kind: 'steps', count: 2 }];
    const live = await openBig(items);
    live.S.newer = extra;
    live.emit('history', { sandbox: 'big' });
    await live.advance(0);
    const fresh = await openBig([...items, ...extra]);
    assert.equal(rowsOf(live).length, rowsOf(fresh).length);
    assert.equal(html(live), html(fresh), `${n} rows`);
    assert.ok(live.$('rows').textContent.includes('and the tests?'));
  }
});

test('a new row builds one row and touches none of the others', async () => {
  const items = conversation(200);
  const dom = await openBig(items);
  const before = [...rowsOf(dom)];
  const last = items[items.length - 1].seq;
  dom.S.newer = [newRow(last + 1, reply(42))];
  dom.counters.innerHTML = 0;
  dom.emit('history', { sandbox: 'big' });
  await dom.advance(0);
  const after = [...rowsOf(dom)];
  assert.equal(after.length, before.length + 1);
  before.forEach((el, i) => assert.equal(after[i], el, `row ${i} kept`));
  assert.equal(dom.counters.innerHTML, 1, 'one Markdown parse: the new reply');
  assert.ok(after[after.length - 1].classList.contains('rc-new'), 'the new row rises in');
  assert.ok(!after[0].classList.contains('rc-new'));
});

test('a steps row folding replaces that row only', async () => {
  const items = conversation(120);
  const dom = await openBig(items);
  const before = [...rowsOf(dom)];
  const tail = before[before.length - 1];
  assert.match(tail.textContent, /steps/);
  const last = items[items.length - 1].seq;
  dom.S.newer = [{ seq: last + 1, at: 1, kind: 'steps', count: 4 }];
  dom.emit('history', { sandbox: 'big' });
  await dom.advance(0);
  const after = [...rowsOf(dom)];
  assert.equal(after.length, before.length);
  before.slice(0, -1).forEach((el, i) => assert.equal(after[i], el, `row ${i} kept`));
  assert.notEqual(after[after.length - 1], tail);
  assert.equal(after[after.length - 1].textContent, `${Number(tail.textContent.split(' ')[0]) + 4} steps`);
});

test('a sent message: the bubble, then the history\'s copy in its place; the rest kept', async () => {
  const items = conversation(60);
  const dom = await openBig(items);
  const before = [...rowsOf(dom)];
  const f = dom.$('field');
  f.value = 'Add the index';
  f.dispatchEvent(new dom.Event('input'));
  dom.$('send').click();
  await dom.advance(0);
  let after = [...rowsOf(dom)];
  assert.equal(after.length, before.length + 1);
  before.forEach((el, i) => assert.equal(after[i], el));
  assert.match(after[after.length - 1].className, /rc-pending/);
  const last = items[items.length - 1].seq;
  dom.S.newer = [{ seq: last + 1, at: 1, kind: 'user_prompt', text: 'Add the index' }];
  dom.emit('history', { sandbox: 'big' });
  await dom.advance(0);
  after = [...rowsOf(dom)];
  assert.equal(after.length, before.length + 1);
  before.forEach((el, i) => assert.equal(after[i], el));
  assert.doesNotMatch(after[after.length - 1].className, /rc-pending/);
  assert.equal(dom.$('rows').querySelectorAll('.rc-me').filter(e => e.textContent === 'Add the index').length, 1, 'one bubble, never two');
});

test('a leaders event leaves the rows alone', async () => {
  const dom = await openBig(conversation(80));
  const before = [...rowsOf(dom)];
  dom.counters.created = 0;
  dom.emit('leaders', { leaders: [{ sandbox: 'big', name: 'big', kind: 'leader', state: 'idle', activity: 'Waiting', last_at: 1, asks: false }] });
  await dom.advance(0);
  [...rowsOf(dom)].forEach((el, i) => assert.equal(el, before[i]));
});

// The row at the top of the view, and how far below the view's top it starts.
function topRow(dom) {
  const log = dom.$('log');
  const lt = log.getBoundingClientRect().top;
  const el = rowsOf(dom).find(r => r.getBoundingClientRect().bottom > lt);
  return { el, offset: el.getBoundingClientRect().top - lt };
}

test('scrolled up, older rows come in above and the row in view stays put', async () => {
  const items = conversation(300, 100000);
  const older = conversation(30, 1);
  const dom = await openBig(items, { older });
  const log = dom.$('log');
  log.scrollTop = 110; // near the top: the scroll reads the older page
  await dom.advance(0);
  await dom.advance(0);
  assert.ok(rowsOf(dom).length > 300, 'older rows drawn');
  const was = topRow(dom);
  // The same row, measured against a page drawn with the older rows from the start.
  const before = await openBig(items, { older });
  const blog = before.$('log');
  blog.scrollTop = 110;
  before.frame();
  const b = topRow(before);
  assert.equal(was.el.dataset.key, b.el.dataset.key);
  assert.equal(Math.round(was.offset), Math.round(b.offset));
});

test('scrolled up, a new row costs a few box reads, not one per row', async () => {
  for (const n of [200, 2000]) {
    const items = conversation(n);
    const dom = await openBig(items);
    const log = dom.$('log');
    log.scrollTop = log.scrollHeight / 2;
    await dom.advance(0);
    const keep = topRow(dom);
    const last = items[items.length - 1].seq;
    dom.S.newer = [newRow(last + 1, reply(7))];
    dom.counters.layoutReads = 0;
    dom.emit('history', { sandbox: 'big' });
    await dom.advance(0);
    assert.ok(dom.counters.layoutReads < 120, `${n} rows: ${dom.counters.layoutReads} reads`);
    const now = topRow(dom);
    assert.equal(now.el, keep.el, 'the same row at the top');
    assert.equal(now.offset, keep.offset);
    assert.ok(!dom.$('to-end').hidden, 'the round button shows, away from the end');
  }
});
