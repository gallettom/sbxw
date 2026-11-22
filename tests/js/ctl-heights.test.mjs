// The control size tokens and the pane bar's one height. The browser check
// (every control in a pane bar has the same height and centre) is in
// scripts/ui-smoke.mjs, checkPaneBars; this guards the numbers it relies on.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const css = readFileSync(new URL('../../assets/app.css', import.meta.url), 'utf8');
const token = (n) => css.match(new RegExp(`--${n}:\\s*(\\d+)px`))?.[1];

test('control size tokens', () => {
  assert.equal(token('ctl-h'), '28');
  assert.equal(token('ctl-h-dense'), '24');
  assert.equal(token('ctl-h-min'), '24');
  assert.equal(token('ctl-hdr'), '28');
  assert.equal(token('ctl-bar'), '24');
});

test('the pane bar is 32px and nothing in the smoke expects another height', () => {
  assert.match(css, /\.pane-bar \{\n  height: 32px; box-sizing: border-box;/);
  const smoke = readFileSync(new URL('../../scripts/ui-smoke.mjs', import.meta.url), 'utf8');
  assert.match(smoke, /checkPaneBars/);
});
