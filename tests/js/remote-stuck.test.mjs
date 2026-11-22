// The phone page never waits for ever (thread phone-freeze-audit): a
// request iOS never settles (the phone slept with it in the air) ends after
// RC_CALL_MS, the busy state it held is cleared, a write that got no answer
// says so without claiming "not sent", and nothing is retried by itself.
// In the stand-in DOM (remote-dom.mjs), on its fake clock.
// Every text is invented. Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { openBig, conversation } from './remote-perf.mjs';

const core = readFileSync(new URL('../../assets/remote/remote-core.js', import.meta.url), 'utf8');
const rc = {};
vm.runInNewContext(`${core}\nthis.rcFailText = rcFailText; this.rcL2Fail = rcL2Fail; this.rcCvSubmitResult = rcCvSubmitResult; this.rcCvRemarkResult = rcCvRemarkResult; this.RC_CALL_MS = RC_CALL_MS; this.rcClient = rcClient;`, Object.assign(rc, { setTimeout, clearTimeout, AbortController }));

test('a write with no answer never says "not sent"', () => {
  const res = { ok: false, status: 0, timeout: true, data: null };
  for (const what of ['Message', 'Answer', 'Interrupt', 'Model change']) {
    const t = rc.rcFailText(what, res);
    assert.doesNotMatch(t, /not sent/i);
    assert.match(t, /no answer from the Mac/);
    assert.match(t, /check before sending again/);
  }
  assert.doesNotMatch(rc.rcL2Fail(res), /Nothing was done/);
  assert.doesNotMatch(rc.rcCvSubmitResult(res).text, /Not sent/);
  assert.doesNotMatch(rc.rcCvRemarkResult(res, 'Pin 1').text, /not saved/);
  // An unreachable Mac (the request failed at once) is still "not sent".
  assert.match(rc.rcFailText('Message', { ok: false, status: 0, unreachable: true }), /not sent/);
});

test('the client: a hung request or body ends as a timeout, and says nothing about reachability', async () => {
  // A fake clock: the bound's timer runs when the test says.
  const timers = [];
  const ctx = { AbortController, setTimeout: fn => { timers.push(fn); return timers.length; }, clearTimeout: () => {} };
  vm.runInNewContext(`${core}\nthis.rcClient = rcClient; this.RC_CALL_MS = RC_CALL_MS;`, ctx);
  assert.equal(ctx.RC_CALL_MS.read, 20000);
  assert.ok(ctx.RC_CALL_MS.message >= 10000 && ctx.RC_CALL_MS.message <= 60000);
  const reach = [];
  let aborted = 0;
  const hung = ctx.rcClient((p, init) => new Promise((_, no) => init.signal.addEventListener('abort', () => { aborted++; no(new Error('aborted')); })), { reach: ok => reach.push(ok) });
  const p1 = hung.me();
  timers.shift()();
  const r1 = await p1;
  assert.equal(r1.timeout, true);
  assert.equal(r1.ok, false);
  assert.equal(aborted, 1, 'the request is aborted');
  const slowBody = ctx.rcClient(async () => ({ ok: true, status: 200, json: () => new Promise(() => {}) }), { reach: ok => reach.push(ok) });
  const p2 = slowBody.leaders();
  await new Promise(r => setImmediate(r));
  timers.shift()();
  assert.equal((await p2).timeout, true, 'a body that never ends too');
  assert.deepEqual(reach, [true], 'only the answer that came says the Mac is there');
  // Transcription keeps its own bound: no timer from the client.
  const before = timers.length;
  const t = ctx.rcClient(() => new Promise(() => {}), {});
  t.transcribe(new Uint8Array(4));
  assert.equal(timers.length, before);
});

test('a Send the Mac never answers: the composer is free again, the text kept, nothing resent', async () => {
  const items = conversation(40);
  let posts = 0;
  const dom = await openBig(items, { hang: (p, init) => { if (p.endsWith('/message')) { posts++; return true; } return false; } });
  const f = dom.$('field');
  f.value = 'Add the index';
  f.dispatchEvent(new dom.Event('input'));
  dom.$('send').click();
  await dom.advance(1000);
  assert.equal(dom.$('send').getAttribute('aria-busy'), 'true', 'still sending after 1 s');
  await dom.advance(30000);
  assert.equal(dom.$('send').getAttribute('aria-busy'), null, 'no longer busy');
  assert.equal(dom.$('send-label').textContent, 'Send');
  assert.equal(dom.$('send').disabled, false, 'Send can be tapped again');
  assert.equal(f.value, 'Add the index', 'the text stays');
  assert.match(dom.$('status').textContent, /no answer from the Mac/);
  assert.doesNotMatch(dom.$('status').textContent, /not sent/i);
  const bubble = dom.$('rows').querySelectorAll('.rc-pending')[0];
  assert.match(bubble.className, /rc-pending-unknown/);
  assert.match(bubble.textContent, /No answer from the Mac/);
  // It had arrived after all: the history's copy takes the bubble's place.
  const last = items[items.length - 1].seq;
  dom.S.newer = [{ seq: last + 1, at: 1, kind: 'user_prompt', text: 'Add the index' }];
  await dom.advance(70000);
  assert.equal(posts, 1, 'never sent again by itself');
  assert.equal(dom.$('rows').querySelectorAll('.rc-pending').length, 0);
  assert.equal(dom.$('rows').querySelectorAll('.rc-me').filter(e => e.textContent === 'Add the index').length, 1);
});

test('an older page that never comes does not stop the next one', async () => {
  const items = conversation(40, 1000);
  const older = conversation(30, 1);
  let hang = true;
  let reads = 0;
  const dom = await openBig(items, { older, hang: p => { if (p.includes('before=')) { reads++; return hang; } return false; } });
  const log = dom.$('log');
  log.scrollTop = 0;
  await dom.advance(0);
  assert.equal(reads, 1);
  log.scrollTop = 50;
  await dom.advance(5000);
  log.scrollTop = 0;
  await dom.advance(0);
  assert.equal(reads, 1, 'one read at a time while it waits');
  await dom.advance(20000);
  hang = false;
  log.scrollTop = 50;
  await dom.advance(0);
  log.scrollTop = 0;
  await dom.advance(0);
  assert.ok(reads >= 2, 'read again once the first gave up');
  assert.ok(dom.$('rows').children.length > 40);
});

test('a model change the Mac never answers: the controls come back', async () => {
  const dom = await openBig(conversation(10), { hang: p => p.endsWith('/model') });
  const idle = { leaders: [{ sandbox: 'big', name: 'big', kind: 'leader', state: 'idle', activity: '', last_at: 1, asks: false }] };
  dom.S.leaders = idle;
  dom.emit('leaders', idle);
  await dom.advance(0);
  assert.equal(dom.$('model').disabled, false);
  const sel = dom.$('model');
  sel.value = 'opus';
  sel.dispatchEvent(new dom.Event('change'));
  await dom.advance(1000);
  assert.equal(sel.disabled, true);
  await dom.advance(61000);
  assert.equal(sel.disabled, false);
  assert.match(dom.$('status').textContent, /no answer from the Mac/);
});
