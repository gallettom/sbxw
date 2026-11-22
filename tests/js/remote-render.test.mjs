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

test('the Working line: drawn again only when it changes', async () => {
  const dom = await openBig(conversation(30));
  const tail = dom.$('tail');
  const line = tail.firstChild;
  assert.match(tail.textContent, /Working, Running tests/);
  const same = { leaders: [{ sandbox: 'big', name: 'big', kind: 'leader', state: 'working', activity: 'Running tests', last_at: 2, asks: false }] };
  dom.emit('leaders', same);
  await dom.advance(0);
  assert.equal(tail.firstChild, line, 'the same line kept');
  same.leaders[0].activity = 'Editing src/db/orders.rs';
  dom.emit('leaders', same);
  await dom.advance(0);
  assert.notEqual(tail.firstChild, line);
  assert.match(tail.textContent, /Working, Editing src\/db\/orders\.rs/);
  same.leaders[0].state = 'idle';
  dom.emit('leaders', same);
  await dom.advance(0);
  assert.equal(tail.childNodes.length, 0, 'idle: no line');
  same.leaders[0].state = 'working';
  dom.emit('leaders', same);
  await dom.advance(0);
  assert.match(tail.textContent, /Working, Editing/);
});

test('a leaders event during a conversation does not rebuild the hidden home list; back home it is current', async () => {
  const dom = await openBig(conversation(20));
  const list = dom.$('leaders');
  const was = list.firstChild;
  const l = { leaders: [{ sandbox: 'big', name: 'big', kind: 'leader', state: 'idle', activity: '', last_at: 3, asks: false }, { sandbox: 'new-one', name: 'new-one', kind: 'leader', state: 'working', activity: 'Starting', last_at: 4, asks: false }] };
  dom.emit('leaders', l);
  await dom.advance(0);
  assert.equal(list.firstChild, was, 'hidden list untouched');
  dom.$('back').click();
  await dom.advance(0);
  assert.ok(list.textContent.includes('new-one'), 'drawn when it shows');
});

test('at the end, a long conversation keeps its newest rows; scrolling up reads the rest again', async () => {
  const items = conversation(600, 1000);
  const older = conversation(30, 1);
  const dom = await openBig(items, { older });
  const n0 = rowsOf(dom).length;
  const last = items[items.length - 1].seq;
  dom.S.newer = [newRow(last + 1, reply(3))];
  dom.emit('history', { sandbox: 'big' });
  await dom.advance(0);
  const after = rowsOf(dom);
  assert.ok(after.length < n0, `${after.length} rows, from ${n0}`);
  assert.ok(after.length >= 300, 'still a long scroll back');
  assert.match(after[after.length - 1].textContent, /Step 3: what changed/);
  // Up to the top: the rows dropped are read again, from the first kept.
  dom.counters.fetches.length = 0;
  dom.$('log').scrollTop = 0;
  await dom.advance(0);
  const want = items[items.length + 1 - 400].seq;
  assert.ok(dom.counters.fetches.some(f => f.includes(`before=${want}&`)), dom.counters.fetches.join());
});

test('scrolled up, nothing is dropped', async () => {
  const items = conversation(600, 1000);
  const dom = await openBig(items);
  const log = dom.$('log');
  log.scrollTop = log.scrollHeight / 2;
  await dom.advance(0);
  const n0 = rowsOf(dom).length;
  const last = items[items.length - 1].seq;
  dom.S.newer = [newRow(last + 1, reply(3))];
  dom.emit('history', { sandbox: 'big' });
  await dom.advance(0);
  assert.equal(rowsOf(dom).length, n0 + 1);
});

test('the keyboard fit writes the root style only when the viewport changed', async () => {
  const dom = await openBig(conversation(5));
  const vv = dom.window.visualViewport;
  const root = dom.document.documentElement;
  let writes = 0;
  const set = root.style.setProperty;
  root.style.setProperty = (k, v) => { writes++; set(k, v); };
  for (let i = 0; i < 20; i += 1) vv.dispatchEvent(new dom.Event('scroll'));
  assert.equal(writes, 0, 'twenty scrolls, nothing moved: no write');
  vv.height = 500;
  vv.dispatchEvent(new dom.Event('resize'));
  assert.equal(root.style.getPropertyValue('--vvh'), '500px');
  assert.ok(root.classList.contains('rc-kb'), 'the keyboard is up');
  vv.height = 844;
  vv.dispatchEvent(new dom.Event('resize'));
  assert.ok(!root.classList.contains('rc-kb'));
});
