// scripts/quarantine.mjs: the parsing of tests/quarantine.txt, and the file
// as committed.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { parseQuarantine } from '../../scripts/quarantine.mjs';

const root = path.resolve(import.meta.dirname, '../..');

test('a list of comments and blank lines quarantines nothing', () => {
  assert.deepEqual(parseQuarantine('# a\n\n   \n# js | x | SBXW-1 | 2026-01-01\n'), { entries: [], errors: [] });
});

test('a line has a suite, a name (spaces allowed), a ticket and a date', () => {
  const { entries, errors } = parseQuarantine([
    'js | docs render whole | SBXW-12 | 2026-11-25',
    'crates | core::board::tests::expiry | #34 | 2026-11-25',
    'desktop|theme::tests::grid|https://example.invalid/t/5|2026-11-25',
  ].join('\n'));
  assert.deepEqual(errors, []);
  assert.deepEqual(entries.map((e) => [e.suite, e.name, e.ticket, e.line]), [
    ['js', 'docs render whole', 'SBXW-12', 1],
    ['crates', 'core::board::tests::expiry', '#34', 2],
    ['desktop', 'theme::tests::grid', 'https://example.invalid/t/5', 3],
  ]);
});

test('a line without a ticket reference is an error, and not an entry', () => {
  for (const line of ['js | a test | | 2026-11-25', 'js | a test | soon | 2026-11-25', 'js | a test | 2026-11-25']) {
    const { entries, errors } = parseQuarantine(line);
    assert.equal(entries.length, 0, line);
    assert.equal(errors.length, 1, line);
    assert.match(errors[0], /^line 1: /, line);
  }
  assert.match(parseQuarantine('js | a test | | 2026-11-25').errors[0], /no ticket reference/);
});

test('an unknown suite, a missing name and a bad date are errors with their line number', () => {
  const { entries, errors } = parseQuarantine('js | ok | SBXW-1 | 2026-01-01\nweb | a | SBXW-1 | 2026-01-01\njs |  | SBXW-1 | 2026-01-01\njs | a | SBXW-1 | yesterday');
  assert.equal(entries.length, 1);
  assert.equal(errors.length, 3);
  assert.match(errors[0], /^line 2: unknown suite "web"/);
  assert.match(errors[1], /^line 3: no test name/);
  assert.match(errors[2], /^line 4: "yesterday" is not a date/);
});

test('the command prints the names of one suite, and refuses a bad list', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'quarantine-'));
  try {
    const run = (text, suite) => {
      const file = path.join(dir, 'q.txt');
      fs.writeFileSync(file, text);
      return spawnSync('node', [path.join(root, 'scripts/quarantine.mjs'), suite, '--file', file], { encoding: 'utf8' });
    };
    const ok = run('js | one | SBXW-1 | 2026-01-01\ncrates | two | SBXW-2 | 2026-01-01\njs | three | SBXW-3 | 2026-01-01\n', 'js');
    assert.equal(ok.status, 0);
    assert.equal(ok.stdout, 'one\nthree\n');
    const bad = run('js | one | | 2026-01-01\n', 'js');
    assert.equal(bad.status, 1);
    assert.equal(bad.stdout, '');
    assert.match(bad.stderr, /no ticket reference/);
    assert.equal(spawnSync('node', [path.join(root, 'scripts/quarantine.mjs'), 'js', '--file', path.join(dir, 'missing.txt')], { encoding: 'utf8' }).status, 0, 'no file, no quarantine');
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('the committed list parses and is empty unless every line has its ticket', () => {
  const { entries, errors } = parseQuarantine(fs.readFileSync(path.join(root, 'tests/quarantine.txt'), 'utf8'));
  assert.deepEqual(errors, []);
  assert.ok(entries.every((e) => e.ticket && e.date));
});
