// The event stream's watchdog (thread phone-freeze-audit): the daemon says
// `ping` after 15 s of nothing (crates/daemon/src/http/remote.rs PING_EVERY),
// so a stream silent for RC_SILENT_MS while the page shows died without a
// word (iOS kept it "open") and is opened again; each reopen that brings
// nothing waits twice as long; hidden, nothing is counted.
// In the stand-in DOM (remote-dom.mjs), on its fake clock.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { openBig, conversation } from './remote-perf.mjs';

const rc = {};
vm.runInNewContext(`${readFileSync(new URL('../../assets/remote/remote-core.js', import.meta.url), 'utf8')}\nthis.rcSilentWait = rcSilentWait; this.rcStreamDead = rcStreamDead; this.RC_SILENT_MS = RC_SILENT_MS;`, rc);
const live = dom => dom.sources.filter(e => e.readyState !== 2).length;

test('the waits: 40 s, then twice as long, at most 5 min', () => {
  assert.equal(rc.RC_SILENT_MS, 40000);
  assert.deepEqual([0, 1, 2, 3, 4, 9, 50].map(rc.rcSilentWait), [40000, 80000, 160000, 300000, 300000, 300000, 300000]);
  assert.equal(rc.rcStreamDead(1000 + 39999, 1000, 0), false);
  assert.equal(rc.rcStreamDead(1000 + 40000, 1000, 0), true);
});

test('pings keep the stream; silence opens it again, with a growing wait', async () => {
  const dom = await openBig(conversation(10));
  const opened = () => dom.sources.length;
  assert.equal(opened(), 1);
  for (let i = 0; i < 8; i += 1) { await dom.advance(15000); dom.emit('ping', ''); }
  assert.equal(opened(), 1, 'two minutes of pings: the same stream');
  const reads = dom.counters.fetches.length;
  await dom.advance(41000);
  assert.equal(opened(), 2, 'silent 40 s: opened again');
  assert.equal(live(dom), 1, 'the old one closed');
  await dom.advance(70000);
  assert.equal(opened(), 2, 'the next wait is 80 s');
  await dom.advance(20000);
  assert.equal(opened(), 3);
  await dom.advance(135000);
  assert.equal(opened(), 3, 'then 160 s');
  await dom.advance(20000);
  assert.equal(opened(), 4);
  // It answers at last: one event and the wait starts again from 40 s.
  dom.emit('open');
  dom.emit('leaders', dom.S.leaders);
  await dom.advance(35000);
  assert.equal(opened(), 4);
  await dom.advance(10000);
  assert.equal(opened(), 5);
  assert.ok(dom.counters.fetches.length - reads < 20, 'no burst of requests');
});

test('hidden, silence is not counted', async () => {
  const dom = await openBig(conversation(10));
  dom.setVisible(false);
  await dom.advance(200000);
  assert.equal(dom.sources.length, 1);
  dom.setVisible(true);
  // Back after 200 s: opened again at once, as before (RC_AWAY_MS).
  assert.equal(dom.sources.length, 2);
  dom.emit('open');
  dom.emit('leaders', dom.S.leaders);
  await dom.advance(30000);
  assert.equal(dom.sources.length, 2);
});
