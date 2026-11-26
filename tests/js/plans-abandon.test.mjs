// The Mac web UI's Abandon merge (assets/js/plans.js, between its "plans
// merge pure" markers, and its markup in assets/index.html): when it is
// offered, what the dialog asks, what an abandoned merge says, and that the
// pane's Merge button goes once the merge is abandoned.
// Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const read = p => readFileSync(new URL(`../../${p}`, import.meta.url), 'utf8');
const src = read('assets/js/plans.js');
const pure = src.slice(src.indexOf('// BEGIN plans merge pure'), src.indexOf('// END plans merge pure'));
const html = read('assets/index.html');

const ctx = vm.createContext({});
vm.runInContext(`${pure}\nglobalThis.api = { mergeToDo, mergeAbandonable, mergeAbandonedText, MERGE_ABANDON_ASK };`, ctx);
const { mergeToDo, mergeAbandonable, mergeAbandonedText, MERGE_ABANDON_ASK } = ctx.api;

test('Abandon merge is offered while the merge is proposed or failed, never while it runs or after', () => {
  const by = Object.fromEntries(['proposed', 'failed', 'merging', 'merged', 'abandoned'].map(s => [s, mergeAbandonable({ state: s })]));
  assert.deepEqual({ ...by }, { proposed: true, failed: true, merging: false, merged: false, abandoned: false });
  assert.equal(mergeAbandonable(undefined), false);
});

test('an abandoned merge is no merge to do: the pane offers no Merge or Retry merge', () => {
  for (const s of ['proposed', 'merging', 'failed']) assert.equal(mergeToDo({ state: s }), true, s);
  for (const s of ['merged', 'abandoned']) assert.equal(mergeToDo({ state: s }), false, s);
  // mergePlanFor, which the pane button reads, goes through mergeToDo.
  const body = src.slice(src.indexOf('function mergePlanFor'), src.indexOf('function openMergePlan'));
  assert.match(body, /if \(!mergeToDo\(p\.merge\)\) continue;/);
});

test('the dialog asks before calling, in the words of the plan, and says what an abandoned merge left', () => {
  assert.equal(MERGE_ABANDON_ASK, 'Abandon the proposed merge? Nothing is merged, and the threads, their branches and worktrees stay as they are. The lead is told.');
  assert.ok(!MERGE_ABANDON_ASK.includes('—'), 'no em dash');
  assert.equal(mergeAbandonedText({ state: 'abandoned', results: [] }), 'You abandoned this merge: nothing was merged. The lead may propose it again.');
  assert.equal(mergeAbandonedText({ state: 'abandoned', results: [{ thread: 'a', ok: true }, { thread: 'b', ok: false }] }),
    'You abandoned this merge. Merged before that: a. The rest was not merged; the lead may propose it again.');
  // The button, quiet beside Not now; the question with its own confirm.
  assert.match(html, /<button class="btn-cancel" id="merge-abandon" hidden>Abandon merge<\/button>\s*<button class="btn-cancel" id="merge-later">/);
  for (const id of ['merge-abandon-ask', 'merge-abandon-text', 'merge-abandon-no', 'merge-abandon-yes']) assert.ok(html.includes(`id="${id}"`), id);
  // Only the confirm calls the route, and its refusal is said as text.
  const route = src.indexOf('/merge/abandon');
  assert.ok(route > src.indexOf("mergeAbandonYes.addEventListener('click'"), 'the call is behind the confirm');
  assert.equal(src.indexOf('/merge/abandon', route + 1), -1, 'called from one place');
  assert.match(src, /showNote\(mergeError, `Abandon merge refused: /);
  assert.match(src, /mergeAbandonYes\.textContent = 'Abandoning…'/);
});

test('a CI merge: Merge and Abandon merge are offered once it is Merged to CI, never while it builds, tests or failed', () => {
  const states = ['merged_ci', 'ci_building', 'ci_testing', 'ci_stale', 'ci_failed'];
  const todo = Object.fromEntries(states.map(s => [s, mergeToDo({ state: s })]));
  assert.deepEqual({ ...todo }, { merged_ci: true, ci_building: false, ci_testing: false, ci_stale: false, ci_failed: false });
  assert.equal(mergeAbandonable({ state: 'merged_ci' }), true);
});
