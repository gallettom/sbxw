// The Mac web UI's plan dialog draws the test command the lead proposed
// (assets/js/plans.js, between its "plans tests pure" markers): labelled,
// escaped (an agent's text is data), with its time limit, and a way to
// approve the plan without it, which the Approve call sends.
// Run: node --test tests/js/plans-tests.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const read = p => readFileSync(new URL(`../../${p}`, import.meta.url), 'utf8');
const src = read('assets/js/plans.js');
const util = read('assets/js/util.js');
const pure = src.slice(src.indexOf('// BEGIN plans tests pure'), src.indexOf('// END plans tests pure'));
const esc = util.slice(util.indexOf('function escHtml'), util.indexOf('\n}\n', util.indexOf('function escHtml')) + 2);

const ctx = vm.createContext({});
vm.runInContext(`${esc}\n${pure}\nglobalThis.api = { planTestsHtml };`, ctx);
const { planTestsHtml } = ctx.api;

test('the dialog draws the command, labelled, with its time limit', () => {
  const h = planTestsHtml({ command: 'npm test -- --ci', timeout_minutes: 45 });
  assert.match(h, /<span class="plan-owns-label">Tests<\/span>/);
  assert.match(h, /<code class="plan-test-command">npm test -- --ci<\/code>/);
  assert.match(h, /time limit 45 min/);
  assert.match(h, /<input type="checkbox" id="plan-no-tests">/);
  assert.match(h, /Approve without tests/);
  assert.ok(!h.includes('—'), 'no em dash');
});

test('the command is escaped: an agent\'s text is never markup', () => {
  const h = planTestsHtml({ command: '<img src=x onerror=alert(1)> && echo "a" \'b\'', timeout_minutes: '<b>9</b>' });
  assert.ok(!h.includes('<img'), h);
  assert.match(h, /&lt;img src=x onerror=alert\(1\)&gt; &amp;&amp; echo/);
  assert.match(h, /time limit 60 min/, 'a limit that is not a number is not drawn as given');
});

test('no proposed command: nothing drawn', () => {
  assert.equal(planTestsHtml(undefined), '');
  assert.equal(planTestsHtml(null), '');
  assert.equal(planTestsHtml({ command: '' }), '');
});

test('the dialog draws it beside the justification and Approve sends the choice', () => {
  const paint = src.slice(src.indexOf('function paintPlanVariant'), src.indexOf('// BEGIN plans tests pure'));
  assert.match(paint, /\$\{planTestsHtml\(plan\.proposed_tests\)\}/);
  assert.match(src, /\{ threads, variant: planVariantChoice\(\), clear_tests: planClearTests\(\) \}/);
  assert.match(src, /const planClearTests = \(\) => !!planVariant\.querySelector\('#plan-no-tests'\)\?\.checked;/);
});
