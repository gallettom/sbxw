// The status hook run whole, as Claude Code runs it: an event on stdin, the
// daemon out of reach. It must end with exit 0 and nothing on stderr. The
// other status-hook tests load only marked sections of the file, so a name
// the main flow uses but nobody defines (`settle`, once) passed them all.
// Run: node --test tests/js/status-hook-run.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, mkdtempSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const src = readFileSync(new URL('../../assets/status-hook.js', import.meta.url), 'utf8');

function run(event) {
  const dir = mkdtempSync(join(tmpdir(), 'sbxw-hook-run-'));
  const file = join(dir, 'status-hook.js');
  // Port 9 (discard): nothing answers, so both requests fail and settle.
  writeFileSync(file, src.replaceAll('__PORT__', '9'));
  return spawnSync(process.execPath, [file], {
    input: JSON.stringify(event),
    encoding: 'utf8',
    timeout: 20000,
    env: { ...process.env, HOME: dir, SANDBOX_NAME: 'test-sandbox' },
  });
}

for (const name of ['SessionStart', 'UserPromptSubmit', 'Stop']) {
  test(`the hook ends cleanly on ${name} with no daemon`, () => {
    const r = run({ hook_event_name: name, session_id: 's-1', transcript_path: '/nonexistent/s-1.jsonl' });
    assert.equal(r.status, 0, `exit ${r.status}, signal ${r.signal}: ${r.stderr}`);
    assert.equal(r.stderr, '', 'nothing on stderr');
  });
}
