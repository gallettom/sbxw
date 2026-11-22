// The plan usage helper of the phone page (rcUsage in remote-core.js).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const src = readFileSync(new URL('../../assets/remote/remote-core.js', import.meta.url), 'utf8');
const ctx = {};
vm.runInNewContext(`${src}\nthis.rcUsage = rcUsage; this.rcUsageReset = rcUsageReset; this.rcUsageLine = rcUsageLine;`, ctx);
const now = Date.UTC(2026, 10, 22, 12, 0, 0);
const plain = v => JSON.parse(JSON.stringify(v));
const sec = ms => Math.round(ms / 1000);

test('percent, rounded and clamped, with the tone at 75 and 90', () => {
  const u = ctx.rcUsage({ five_hour_pct: 74.6, seven_day_pct: 120, updated_ms: now }, now);
  assert.deepEqual(plain(u.rows.map(r => [r.key, r.pct, r.tone])), [['five', 75, 'amber'], ['seven', 100, 'red']]);
  assert.equal(ctx.rcUsage({ five_hour_pct: 74.4 }, now).rows[0].tone, 'ok');
  assert.equal(ctx.rcUsage({ five_hour_pct: 90 }, now).rows[0].tone, 'red');
  assert.deepEqual([...u.compact], ['5 h 75%', '7 d 100%']);
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
    assert.deepEqual([...u.compact], []);
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
test('the conversation line: each window with its tone, then the first reset known', () => {
  const at = ms => sec(now + ms);
  const l = ctx.rcUsageLine(ctx.rcUsage({ five_hour_pct: 42, five_hour_resets_at: at(134 * 60e3), seven_day_pct: 80, seven_day_resets_at: at(3 * 864e5) }, now));
  assert.deepEqual(plain(l), { has: true, parts: [{ text: '5 h 42%', tone: 'ok' }, { text: '7 d 80%', tone: 'amber' }], reset: 'resets in 2 h 14 min' });
  const seven = ctx.rcUsageLine(ctx.rcUsage({ five_hour_pct: 95, seven_day_pct: 12, seven_day_resets_at: at(30 * 60e3) }, now));
  assert.deepEqual(plain(seven), { has: true, parts: [{ text: '5 h 95%', tone: 'red' }, { text: '7 d 12%', tone: 'ok' }], reset: 'resets in 30 min' }, 'no 5-hour reset: the 7-day one');
  for (const v of [null, undefined, {}, ctx.rcUsage(null, now)]) assert.deepEqual(plain(ctx.rcUsageLine(v)), { has: false, parts: [], reset: '' });
});
test('the usage moved from the menu to the conversation', () => {
  const html = readFileSync(new URL('../../assets/remote/index.html', import.meta.url), 'utf8');
  assert.doesNotMatch(html, /id="usage-block"|>Plan usage</, 'no usage block in the menu');
  const composer = html.slice(html.indexOf('id="ctl"'), html.indexOf('id="status"'));
  assert.match(composer, /<p id="ctl-usage" class="rc-ctl-usage" hidden><\/p>/, 'one line right under Model and Lead');
  assert.match(html, /id="usage-line"/, 'the home keeps its line');
});
