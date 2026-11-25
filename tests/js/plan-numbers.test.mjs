// plan_status prints the Board's three numbers of a plan (assets/relay-mcp.js,
// between its "plan numbers" markers): approved to merged (or running time),
// test jobs run and reused and thread commands sent to the queue, and how
// many times the user was asked. Invented numbers.
// Run: node --test tests/js/plan-numbers.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const src = readFileSync(new URL('../../assets/relay-mcp.js', import.meta.url), 'utf8');
const pure = src.slice(src.indexOf('// BEGIN plan numbers'), src.indexOf('// END plan numbers'));
const ctx = vm.createContext({});
vm.runInContext(`${pure}\nglobalThis.api = { numbersLine };`, ctx);
const { numbersLine } = ctx.api;

test('a merged plan: the time from approved to merged, the jobs, the asks', () => {
  const l = numbersLine({ approved_ms: 0, merged_ms: (2 * 60 + 5) * 60000, tests: { run: 2, reused: 1, refused: 1 }, asked: 2 }, 1e12);
  assert.equal(l, 'Numbers (also on the Board): approved to merged in 2 h 05 min; test jobs: 2 run, 1 answered without running, 1 thread command(s) sent to the queue; the user was asked 2 times.');
  assert.ok(!l.includes('—'), 'no em dash');
});

test('not merged yet: the running time; no test command: no test part', () => {
  const l = numbersLine({ approved_ms: 1000, merged_ms: null, tests: null, asked: 1 }, 1000 + 42 * 60000);
  assert.equal(l, 'Numbers (also on the Board): approved 42 min ago, not merged yet; the user was asked 1 time.');
});

test('nothing sent: nothing printed', () => {
  assert.equal(numbersLine(undefined, 0), '');
  assert.equal(numbersLine(null, 0), '');
});
