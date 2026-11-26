// scripts/run-suite.sh in a throwaway checkout: a copy of the script, a
// quarantine list, two tiny js tests (one failing) and a fake `cargo` that
// records its arguments. Nothing here runs the repository's real suites.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const root = path.resolve(import.meta.dirname, '../..');

function checkout({ quarantine = '', failing = false } = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'run-suite-'));
  fs.mkdirSync(path.join(dir, 'scripts'));
  fs.mkdirSync(path.join(dir, 'tests/js'), { recursive: true });
  fs.mkdirSync(path.join(dir, 'desktop'));
  fs.mkdirSync(path.join(dir, 'bin'));
  for (const f of ['run-suite.sh', 'quarantine.mjs']) fs.copyFileSync(path.join(root, 'scripts', f), path.join(dir, 'scripts', f));
  fs.chmodSync(path.join(dir, 'scripts/run-suite.sh'), 0o755);
  fs.writeFileSync(path.join(dir, 'tests/quarantine.txt'), quarantine);
  fs.writeFileSync(
    path.join(dir, 'tests/js/a.test.mjs'),
    `import { test } from 'node:test';
     test('good one', () => {});
     test('flaky (sometimes) one', () => { throw new Error('boom'); });
     ${failing ? "test('really broken', () => { throw new Error('no'); });" : ''}`,
  );
  // the fake cargo: log "<cwd> <target dir> <args>", print a libtest summary, fail on CARGO_FAIL
  fs.writeFileSync(
    path.join(dir, 'bin/cargo'),
    `#!/bin/sh
echo "$PWD|$CARGO_TARGET_DIR|$CARGO_PROFILE_DEV_DEBUG|$*" >> "$CARGO_LOG"
echo "test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"
[ -z "$CARGO_FAIL" ] || { echo "test result: FAILED. 3 passed; 2 failed; 0 ignored"; exit 101; }
`,
  );
  fs.chmodSync(path.join(dir, 'bin/cargo'), 0o755);
  const run = (suite, env = {}) => {
    const r = spawnSync(path.join(dir, 'scripts/run-suite.sh'), suite === undefined ? [] : [suite], {
      cwd: dir,
      encoding: 'utf8',
      env: { PATH: `${path.join(dir, 'bin')}:${process.env.PATH}`, HOME: dir, CARGO_LOG: path.join(dir, 'cargo.log'), ...env },
    });
    const cargo = fs.existsSync(path.join(dir, 'cargo.log')) ? fs.readFileSync(path.join(dir, 'cargo.log'), 'utf8').trim().split('\n') : [];
    return { ...r, cargo, summary: r.stdout.split('\n').filter((l) => l.startsWith('suite ') && l.includes(':')) };
  };
  return { dir, run, done: () => fs.rmSync(dir, { recursive: true, force: true }) };
}

const withCheckout = (opts, fn) => () => {
  const c = checkout(opts);
  try {
    return fn(c);
  } finally {
    c.done();
  }
};

test('an unknown suite or no argument is a usage error', withCheckout({}, (c) => {
  assert.equal(c.run(undefined).status, 2);
  assert.equal(c.run('web').status, 2);
}));

test('crates runs the four packages, in the target directory SBXW_TEST_TARGET names', withCheckout({}, (c) => {
  const r = c.run('crates', { SBXW_TEST_TARGET: path.join(c.dir, 'tt') });
  assert.equal(r.status, 0, r.stdout + r.stderr);
  assert.equal(r.cargo.length, 1);
  const [cwd, target, , args] = r.cargo[0].split('|');
  assert.equal(fs.realpathSync(cwd), fs.realpathSync(c.dir));
  assert.equal(target, path.join(c.dir, 'tt', 'crates'));
  assert.equal(args, 'test -p sbxw-core -p sbxw-daemon -p sbxw-sbx -p sbxw --');
  // The seconds are whatever the run took: a loaded machine takes one.
  assert.equal(r.summary.length, 1);
  assert.match(r.summary[0], /^suite crates: PASS \(7 passed, 0 failed, 0 quarantined\) \d+s$/);
}));

test('without SBXW_TEST_TARGET the target is under the checkout', withCheckout({}, (c) => {
  const r = c.run('crates');
  assert.equal(r.cargo[0].split('|')[1], path.join(fs.realpathSync(c.dir), 'target/run-suite/crates'));
}));

test('desktop runs in desktop/ with no-fail-fast, -j1 and no debug info', withCheckout({}, (c) => {
  const r = c.run('desktop');
  assert.equal(r.status, 0, r.stdout + r.stderr);
  const [cwd, target, debug, args] = r.cargo[0].split('|');
  assert.equal(fs.realpathSync(cwd), fs.realpathSync(path.join(c.dir, 'desktop')));
  assert.match(target, /\/target\/run-suite\/desktop$/);
  assert.equal(debug, '0');
  assert.equal(args, 'test -j1 --no-fail-fast --');
}));

test('js runs the node tests and fails on a failing one', withCheckout({}, (c) => {
  const r = c.run('js');
  assert.equal(r.status, 1);
  assert.match(r.summary[0], /^suite js: FAIL \(1 passed, 1 failed, 0 quarantined\)/);
}));

test('a failing cargo run fails the suite, and all runs every suite anyway', withCheckout({}, (c) => {
  const r = c.run('all', { CARGO_FAIL: '1' });
  assert.equal(r.status, 1);
  assert.equal(r.cargo.length, 2, 'desktop still ran after crates failed');
  assert.deepEqual(r.summary.map((l) => l.split(' ').slice(0, 3).join(' ')), ['suite crates: FAIL', 'suite js: FAIL', 'suite desktop: FAIL']);
}));

test('quarantined tests are skipped and named; the rest still blocks', withCheckout(
  { quarantine: 'js | flaky (sometimes) one | SBXW-9 | 2026-11-25\ncrates | core::a::b | #3 | 2026-11-25\n' },
  (c) => {
    const js = c.run('js');
    assert.equal(js.status, 0, js.stdout + js.stderr);
    assert.match(js.stdout, /quarantined, skipped: flaky \(sometimes\) one/);
    assert.match(js.summary[0], /^suite js: PASS \(1 passed, 0 failed, 1 quarantined\)/);
    const crates = c.run('crates');
    assert.match(crates.stdout, /quarantined, skipped: core::a::b/);
    assert.match(crates.cargo[0], /-- --skip core::a::b$/);
  },
));

test('a quarantined line without a ticket stops the run before anything starts', withCheckout(
  { quarantine: 'js | flaky (sometimes) one | | 2026-11-25\n' },
  (c) => {
    const r = c.run('all');
    assert.equal(r.status, 1);
    assert.match(r.stderr, /no ticket reference/);
    assert.deepEqual(r.cargo, []);
    assert.match(r.summary.join('\n'), /FAIL/);
  },
));
