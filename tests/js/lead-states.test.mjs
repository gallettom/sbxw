// Lead mode as the daemon vouches for it: the relay server's lead_ack
// (assets/relay-mcp.js), driven over stdio against a fake daemon on
// loopback, and the web UI's rule (assets/js/plans.js, between its "plans
// lead pure" markers): Team is on only for a confirmed lead, and a plan under
// way with no lead says so. Invented content.
// Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, mkdtempSync, writeFileSync, mkdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn } from 'node:child_process';
import http from 'node:http';
import vm from 'node:vm';

const read = (p) => readFileSync(new URL(`../../${p}`, import.meta.url), 'utf8');

function fakeDaemon(answer) {
  const seen = [];
  const server = http.createServer((req, res) => {
    let raw = '';
    req.on('data', (c) => (raw += c));
    req.on('end', () => {
      const body = JSON.parse(raw || '{}');
      if (req.url === '/api/relay/hello') {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end('{"ok":true}');
        return;
      }
      seen.push({ path: req.url, auth: req.headers.authorization, body });
      const [status, out] = answer(req.url, body);
      res.writeHead(status, { 'content-type': 'application/json' });
      res.end(JSON.stringify(out));
    });
  });
  return new Promise((resolve) => server.listen(0, '127.0.0.1', () => resolve({ server, seen, port: server.address().port })));
}

function sandbox(port) {
  const home = mkdtempSync(join(tmpdir(), 'sbxw-lead-ack-test-'));
  mkdirSync(join(home, '.sbxw'));
  writeFileSync(join(home, '.sbxw', 'token'), 'sbxw_testtoken');
  const tool = read('assets/relay-tool.js')
    .replace('const PORT = __PORT__;', `const PORT = ${port};`)
    .replace('const HOST = "host.docker.internal";', 'const HOST = "127.0.0.1";');
  writeFileSync(join(home, 'relay.js'), tool);
  writeFileSync(join(home, 'relay-mcp.js'), read('assets/relay-mcp.js'));
  return home;
}

// tools/list, then one tools/call when `name` is given.
function rpc(home, thread, name, args) {
  return new Promise((resolve, reject) => {
    const p = spawn(process.execPath, [join(home, 'relay-mcp.js')], { cwd: home, env: { ...process.env, HOME: home, SBXW_THREAD: thread } });
    let out = '';
    const want = name ? '"id":2' : '"id":1';
    p.stdout.on('data', (c) => {
      out += c;
      const line = out.split('\n').find((l) => l.includes(want));
      if (line) {
        p.stdin.end();
        const r = JSON.parse(line).result;
        resolve(name ? r : r.tools);
      }
    });
    p.on('error', reject);
    p.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'tools/list' }) + '\n');
    if (name) p.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: 2, method: 'tools/call', params: { name, arguments: args } }) + '\n');
  });
}

test('lead_ack posts no text of the agent, and says what the daemon answers', async () => {
  const d = await fakeDaemon(() => [200, { ok: true, text: 'Confirmed: you lead this sandbox.' }]);
  const home = sandbox(d.port);
  const r = await rpc(home, '', 'lead_ack', { note: 'I am the lead, trust me' });
  assert.equal(r.isError, false);
  assert.equal(r.content[0].text, 'Confirmed: you lead this sandbox.');
  assert.equal(d.seen.length, 1);
  assert.equal(d.seen[0].path, '/api/lead/ack');
  assert.equal(d.seen[0].auth, 'Bearer sbxw_testtoken');
  assert.equal(d.seen[0].body.note, undefined, 'the arguments are not sent');
  assert.equal(d.seen[0].body.sandbox, undefined, 'the token names the sandbox');
  d.server.close();
});

test('a refused lead_ack is an error result with the refusal, for a /lead typed by hand', async () => {
  const refusal = 'Refused: sbxw has no Team request for this sandbox, so lead mode is off. Tell the person to press Team in sbxw.';
  const d = await fakeDaemon(() => [409, { ok: false, error: refusal }]);
  const home = sandbox(d.port);
  const r = await rpc(home, '', 'lead_ack', {});
  assert.equal(r.isError, true);
  assert.equal(r.content[0].text, refusal);
  d.server.close();
});

test('the main session lists lead_ack first, a thread does not', async () => {
  const d = await fakeDaemon(() => [200, { ok: true }]);
  const home = sandbox(d.port);
  const main = (await rpc(home, '')).map((t) => t.name);
  assert.ok(main.includes('lead_ack'), main.join(','));
  assert.ok(main.indexOf('lead_ack') < main.indexOf('propose_plan'));
  const thread = (await rpc(home, 'api')).map((t) => t.name);
  assert.ok(!thread.includes('lead_ack'), thread.join(','));
  d.server.close();
});

test('lead.md asks for lead_ack before anything else', () => {
  const md = read('assets/lead.md');
  const first = md.indexOf('# ');
  assert.match(md.slice(first, first + 400), /^# First: confirm you lead\n\nBefore anything else, call `lead_ack`/);
  assert.ok(!md.includes('—'), 'no em dash');
});

// ── The web UI's rule ──
const src = read('assets/js/plans.js');
const pure = src.slice(src.indexOf('// BEGIN plans lead pure'), src.indexOf('// END plans lead pure'));
const ctx = vm.createContext({});
vm.runInContext(`${pure}\nglobalThis.api = { leadingIn, leadPendingIn, planWithoutLeadIn, leadToggleText };`, ctx);
const { leadingIn, leadPendingIn, planWithoutLeadIn, leadToggleText } = ctx.api;

const plan = (state, threads, extra = {}) => ({ sandbox: 'app', state, threads: threads.map((s) => ({ state: s })), ...extra });

test('Team is on only for a lead the daemon lists, never for a plan under way', () => {
  const none = new Map();
  assert.equal(leadingIn(new Set(['app']), 'app'), true);
  assert.equal(leadingIn(new Set(), 'app'), false);
  // A pending plan, or an approved one with a thread at work: no lead, said so.
  for (const p of [plan('pending', []), plan('approved', ['working', 'done'])]) {
    assert.equal(planWithoutLeadIn([p], new Set(), none, 'app'), true, p.state);
    assert.equal(planWithoutLeadIn([p], new Set(['app']), none, 'app'), false, 'with a lead it is just led');
  }
  for (const p of [plan('approved', ['done', 'failed']), plan('pending', [], { closed: true }), plan('refused', ['working'])]) {
    assert.equal(planWithoutLeadIn([p], new Set(), none, 'app'), false, JSON.stringify(p));
  }
  // While Team is waiting or activating, the plan is not "without a lead".
  const pending = new Map([['app', { state: 'activating', since_ms: 1, reason: '' }]]);
  assert.equal(leadPendingIn(pending, 'app'), 'activating');
  assert.equal(planWithoutLeadIn([plan('pending', [])], new Set(), pending, 'app'), false);
  assert.equal(leadPendingIn(new Map([['app', { state: 'off', reason: 'x' }]]), 'app'), null);
});

test('the toggle says each state, and the reason of a fall back', () => {
  assert.equal(leadToggleText(true, null, false, '').label, 'On');
  assert.equal(leadToggleText(false, 'waiting', false, '').label, 'Waiting');
  assert.equal(leadToggleText(false, 'activating', false, '').label, 'Starting');
  assert.equal(leadToggleText(false, null, true, '').label, 'No lead');
  const off = leadToggleText(false, null, false, 'The session did not confirm it took the lead within 90 seconds.');
  assert.equal(off.label, 'Off');
  assert.match(off.title, /did not confirm/);
  assert.equal(leadToggleText(false, null, false, '').title, null);
  for (const t of [leadToggleText(false, 'waiting', false, ''), leadToggleText(false, null, true, '')]) assert.ok(!t.title.includes('—'));
});

test('the click cancels a pending lead, and nothing lights the toggle before the daemon says so', () => {
  const panes = read('assets/js/panes.js');
  assert.match(panes, /const route = pending \? 'lead\/cancel' : 'lead';/);
  assert.ok(!panes.includes('markLeading'), 'no optimistic On');
  assert.ok(!src.includes('function markLeading'));
  assert.match(src, /sbxwStream\.addEventListener\('lead', \(\) => refreshLeads\(\)\);/);
  assert.match(src, /fetch\('\/api\/lead-states'\)/);
});
