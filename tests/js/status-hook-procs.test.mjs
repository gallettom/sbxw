// The status hook's process reading (assets/status-hook.js, between its
// BEGIN procs and END procs markers), against a scripted /proc in a
// temporary directory. The case from sbxw-2: a background session's
// process is named `2.1.328`, the terminals' `claude`.
// Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, mkdtempSync, mkdirSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import vm from 'node:vm';

const src = readFileSync(new URL('../../assets/status-hook.js', import.meta.url), 'utf8');
const block = src.slice(src.indexOf('// BEGIN procs'), src.indexOf('// END procs'));
const require = createRequire(import.meta.url);

function load() {
  const ctx = vm.createContext({ require, fs: require('fs') });
  vm.runInContext(`${block}
    globalThis.api = { agentProcess, liveProcs, sessionKind, claudeLike };`, ctx);
  const api = {};
  for (const [k, f] of Object.entries(ctx.api)) {
    api[k] = (...a) => {
      const r = f(...a);
      return r === undefined ? r : JSON.parse(JSON.stringify(r));
    };
  }
  return api;
}

// A /proc with these processes: [pid, comm, start].
function fakeProc(procs) {
  const root = mkdtempSync(join(tmpdir(), 'sbxw-proc-test-'));
  for (const [pid, comm, start] of procs) {
    mkdirSync(join(root, String(pid)));
    // pid (comm) state ppid ... field 22 is the start time.
    const rest = ['S', '1', ...Array.from({ length: 17 }, () => '0'), String(start)];
    writeFileSync(join(root, String(pid), 'stat'), `${pid} (${comm}) ${rest.join(' ')}`);
  }
  mkdirSync(join(root, 'self'));
  return root;
}

test('a background session\'s start lists the terminals\' claude too', () => {
  const api = load();
  const root = fakeProc([[100, 'claude', 5], [300, '2.1.328', 7], [301, '2.1.328', 7], [400, 'node', 9], [500, 'bash', 9]]);
  const agent = api.agentProcess([{ name: 'MainThread', pid: 1 }, { name: '2.1.328', pid: 300, start: '7' }]);
  assert.equal(agent.name, '2.1.328');
  assert.deepEqual(api.liveProcs(agent.name, root).sort(), ['100:5', '300:7', '301:7']);
  // A terminal's start lists the background ones as well.
  assert.deepEqual(api.liveProcs('claude', root).sort(), ['100:5', '300:7', '301:7']);
  // An agent of another name (an npm claude run as node) is still listed.
  assert.deepEqual(api.liveProcs('node', root).sort(), ['100:5', '300:7', '301:7', '400:9']);
  assert.equal(api.liveProcs('claude', join(root, 'missing')), undefined, 'no /proc: no list');
});

test('the session kind is forwarded only as a plain short word', () => {
  const api = load();
  assert.equal(api.sessionKind({ CLAUDE_CODE_SESSION_KIND: 'bg' }), 'bg');
  assert.equal(api.sessionKind({}), undefined);
  assert.equal(api.sessionKind({ CLAUDE_CODE_SESSION_KIND: 'x'.repeat(30) }), undefined);
  assert.equal(api.sessionKind({ CLAUDE_CODE_SESSION_KIND: 'bg; rm' }), undefined);
  assert.ok(api.claudeLike('2.1.328') && api.claudeLike('claude') && !api.claudeLike('node') && !api.claudeLike('2'));
});
