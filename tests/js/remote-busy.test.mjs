// rcBusy: the one busy state of a button whose request is running.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const src = readFileSync(new URL('../../assets/remote/remote-core.js', import.meta.url), 'utf8');
const ctx = {};
vm.runInNewContext(`${src}\nthis.rcBusy = rcBusy;`, ctx);

// A tiny DOM: just what rcBusy touches.
function node(tag, text = '') {
  const n = { tag, text, className: '', attrs: {}, childNodes: [], parent: null };
  n.setAttribute = (k, v) => { n.attrs[k] = String(v); };
  n.removeAttribute = k => { delete n.attrs[k]; };
  n.remove = () => { if (n.parent) n.parent.childNodes = n.parent.childNodes.filter(c => c !== n); n.parent = null; };
  return n;
}
function button(label) {
  const b = node('button');
  const classes = new Set();
  b.classList = { add: c => classes.add(c), remove: c => classes.delete(c), contains: c => classes.has(c) };
  b.ownerDocument = { createElement: t => node(t), createTextNode: t => node('#text', t) };
  b.disabled = false;
  b.replaceChildren = (...k) => { b.childNodes.forEach(c => { c.parent = null; }); b.childNodes = k; k.forEach(c => { c.parent = b; }); };
  b.prepend = c => { b.childNodes.unshift(c); c.parent = b; };
  b.replaceChildren(node('#text', label));
  return b;
}
const labels = b => b.childNodes.map(c => c.text || c.className).join('|');

test('on: aria-busy, spinner before the label, label kept, inert', () => {
  const b = button('Merge with Face ID');
  ctx.rcBusy(b, true);
  assert.equal(b.attrs['aria-busy'], 'true');
  assert.ok(b.classList.contains('rc-busy'));
  assert.equal(b.disabled, true);
  assert.equal(b.childNodes[0].className, 'rc-spin');
  assert.equal(b.childNodes[0].attrs['aria-hidden'], 'true');
  assert.equal(labels(b), 'rc-spin|Merge with Face ID');
});

test('on with a label: the label is replaced; off restores it exactly', () => {
  const b = button('Merge');
  ctx.rcBusy(b, true, 'Merging…');
  assert.equal(labels(b), 'rc-spin|Merging…');
  ctx.rcBusy(b, false);
  assert.equal(labels(b), 'Merge');
  assert.equal(b.attrs['aria-busy'], undefined);
  assert.ok(!b.classList.contains('rc-busy'));
  assert.equal(b.disabled, false);
});

test('off without a label removes the spinner only; a disabled button stays disabled', () => {
  const b = button('Send');
  b.disabled = true;
  ctx.rcBusy(b, true);
  ctx.rcBusy(b, false);
  assert.equal(labels(b), 'Send');
  assert.equal(b.disabled, true);
});

test('double on and double off are safe', () => {
  const b = button('Create');
  ctx.rcBusy(b, true, 'Creating…');
  ctx.rcBusy(b, true, 'Other…');
  assert.equal(b.childNodes.length, 2);
  ctx.rcBusy(b, false);
  ctx.rcBusy(b, false);
  assert.equal(labels(b), 'Create');
  assert.equal(b.disabled, false);
  ctx.rcBusy(b, false);
  ctx.rcBusy(null, true);
});
