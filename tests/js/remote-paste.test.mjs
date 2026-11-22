// Pasting files into the phone's message field (rcPasteFiles in remote-core.js).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const src = readFileSync(new URL('../../assets/remote/remote-core.js', import.meta.url), 'utf8');
const ctx = {};
vm.runInNewContext(`${src}\nthis.rcPasteFiles = rcPasteFiles; this.rcCheckAttach = rcCheckAttach;`, ctx);
const now = Date.UTC(2026, 10, 22, 9, 5, 7);
const mk = (f, name) => ({ ...f, name });
const file = (name, type, size = 100) => ({ name, type, size });
const paste = (cd) => ctx.rcPasteFiles(cd, now, mk);

test('a pasted image with the generic or no name gets a generated one', () => {
  const [a] = paste({ files: [file('image.png', 'image/png')] });
  assert.equal(a.name, 'pasted-20261122090507.png');
  const [b] = paste({ files: [file('', 'image/jpeg')] });
  assert.equal(b.name, 'pasted-20261122090507.jpg');
});
test('a real name is kept; several images are numbered', () => {
  assert.equal(paste({ files: [file('shot.png', 'image/png')] })[0].name, 'shot.png');
  assert.deepEqual(Array.from(paste({ files: [file('', 'image/png'), file('', 'image/png')] }).map(f => f.name)), ['pasted-20261122090507-1.png', 'pasted-20261122090507-2.png']);
});
test('items of kind file are the fallback', () => {
  const f = file('', 'image/png');
  const r = paste({ files: [], items: [{ kind: 'string', type: 'text/plain' }, { kind: 'file', getAsFile: () => f }] });
  assert.equal(r.length, 1);
  assert.match(r[0].name, /^pasted-/);
});
test('a text paste takes nothing', () => {
  assert.deepEqual(JSON.parse(JSON.stringify(paste({ files: [], items: [{ kind: 'string', type: 'text/plain' }] }))), []);
  assert.deepEqual(JSON.parse(JSON.stringify(paste(null))), []);
  assert.deepEqual(JSON.parse(JSON.stringify(paste({}))), []);
});
test('the existing check still decides: a program is not an image, more than 5 are refused', () => {
  const six = Array.from({ length: 6 }, () => file('', 'image/png'));
  const r = ctx.rcCheckAttach(0, paste({ files: six }));
  assert.equal(r.take.length, 5);
  assert.equal(r.refused.length, 1);
  const big = ctx.rcCheckAttach(0, paste({ files: [file('', 'image/png', 1e9)] }));
  assert.equal(big.take.length, 0);
  // A non-image file is passed through unchanged, for the same check as a picked file.
  const doc = file('notes.zip', 'application/zip');
  assert.equal(paste({ files: [doc] })[0].name, 'notes.zip');
});
