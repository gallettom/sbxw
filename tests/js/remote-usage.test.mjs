// The plan usage helper of the phone page (rcUsage in remote-core.js).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const src = readFileSync(new URL('../../assets/remote/remote-core.js', import.meta.url), 'utf8');
const ctx = {};
vm.runInNewContext(`${src}\nthis.rcUsage = rcUsage; this.rcUsageReset = rcUsageReset;`, ctx);
const now = Date.UTC(2026, 10, 22, 12, 0, 0);
const plain = v => JSON.parse(JSON.stringify(v));
const sec = ms => Math.round(ms / 1000);

test('percent, rounded and clamped, with the tone at 75 and 90', () => {
  const u = ctx.rcUsage({ five_hour_pct: 74.6, seven_day_pct: 120, updated_ms: now }, now);
  assert.deepEqual(plain(u.rows.map(r => [r.key, r.pct, r.tone])), [['five', 75, 'amber'], ['seven', 100, 'red']]);
  assert.equal(ctx.rcUsage({ five_hour_pct: 74.4 }, now).rows[0].tone, 'ok');
  assert.equal(ctx.rcUsage({ five_hour_pct: 90 }, now).rows[0].tone, 'red');
  assert.equal(u.compact, '5 h 75% · 7 d 100%');
});
test('reset countdown under a day', () => {
  assert.equal(ctx.rcUsageReset(sec(now + (2 * 60 + 14) * 60e3), now), 'resets in 2 h 14 min');
  assert.equal(ctx.rcUsageReset(sec(now + 5 * 60e3), now), 'resets in 5 min');
  assert.equal(ctx.rcUsageReset(sec(now + 10e3), now), 'resets in 1 min');
});
test('reset a day or more away: weekday and local clock', () => {
  const at = sec(now + 3 * 86400e3);
  const d = new Date(at * 1000);
  const days = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
  const hm = `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
  assert.equal(ctx.rcUsageReset(at, now), `resets ${days[d.getDay()]} ${hm}`);
});
test('a past reset', () => {
  assert.equal(ctx.rcUsageReset(sec(now - 60e3), now), 'reset passed');
  assert.equal(ctx.rcUsageReset(sec(now), now), 'reset passed');
});
test('missing values', () => {
  for (const v of [null, undefined, {}, { updated_ms: now }, 'x', { five_hour_pct: 'a' }, { five_hour_pct: NaN }]) {
    const u = ctx.rcUsage(v, now);
    assert.equal(u.has, false);
    assert.deepEqual(plain(u.rows), []);
    assert.equal(u.updated, '');
    assert.equal(u.compact, '');
  }
  const one = ctx.rcUsage({ seven_day_pct: 10 }, now);
  assert.deepEqual(plain(one.rows.map(r => [r.key, r.reset])), [['seven', '']]);
  assert.equal(ctx.rcUsageReset(null, now), '');
  assert.equal(ctx.rcUsageReset(0, now), '');
});
test('updated ago', () => {
  assert.equal(ctx.rcUsage({ five_hour_pct: 1, updated_ms: now - 3 * 60e3 }, now).updated, 'updated 3 min ago');
  assert.equal(ctx.rcUsage({ five_hour_pct: 1, updated_ms: now - 5e3 }, now).updated, 'updated just now');
  assert.equal(ctx.rcUsage({ five_hour_pct: 1 }, now).updated, '');
});
