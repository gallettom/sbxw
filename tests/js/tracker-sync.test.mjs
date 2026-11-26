// The tracker script (assets/tracker-sync.js), run as the daemon runs it
// (`node - =<request>`, the source on stdin) against a fake MCP gateway on
// loopback (tracker-sync-fake.mjs): each operation, tools listed directly
// and reached through mcp-exec, JSON and event-stream answers, every error
// kind, the candidate table falling through, fetch's size and host limits,
// hostile issue text, and no secret in any output. The GitHub tool names
// and shapes here are the script's own guesses, not the real server's.
// Run: node --test tests/js/tracker-sync.test.mjs
import { test, after } from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import { mkdtempSync, readdirSync, readFileSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fakeGateway, home, run, TOKEN } from './tracker-sync-fake.mjs';

const outputs = [];
const servers = [];
after(() => { for (const s of servers) s.close(); });

async function gw(o) {
  const g = await fakeGateway(o);
  servers.push(g.server);
  return g;
}
async function go(g, req, o = {}) {
  const r = await run(req, { home: home(g.port), ...o });
  outputs.push(r.stdout, r.stderr);
  assert.equal(r.code, 0, `exit 0 with an answer: ${r.stderr}`);
  assert.ok(r.answer, `an answer line: ${r.stdout} ${r.stderr}`);
  return r.answer;
}
const obj = props => ({ type: 'object', properties: Object.fromEntries(props.map(p => [p, {}])) });

const ISSUE = (n, extra = {}) => ({ number: n, title: `Issue ${n}`, body: `Body ${n}`, state: 'open', labels: [{ name: 'sbxw' }], html_url: `https://evil.example/${n}`, updated_at: '2026-11-20T10:00:00Z', ...extra });

// The newer server's shape: issue_read / issue_write / label_write.
function newer(calls = {}) {
  return {
    get_me: { run: () => ({ json: { login: 'octo-person', id: 1, details: { name: 'x' } } }) },
    list_issues: { schema: { type: 'object', properties: { owner: {}, repo: {}, state: { enum: ['OPEN', 'CLOSED'] }, labels: { type: 'array' }, perPage: {}, after: {}, orderBy: {}, direction: {} }, required: ['owner', 'repo'] },
      run: calls.list || (() => ({ json: { issues: [ISSUE(1), ISSUE(2)], pageInfo: { hasNextPage: true, endCursor: 'Y3Vyc29yOjI=' } } })) },
    issue_read: { schema: { type: 'object', properties: { method: {}, owner: {}, repo: {}, issue_number: { type: 'number' } }, required: ['method', 'owner', 'repo', 'issue_number'] },
      run: calls.get || (() => ({ json: { number: 7, labels: [{ name: 'bug' }, { name: 'sbxw:planned' }, { name: 'sbxw:old' }] } })) },
    add_issue_comment: { schema: obj(['owner', 'repo', 'issue_number', 'body']), run: calls.comment || (() => ({ json: { id: 99887766, html_url: 'https://github.com/a/b/issues/7#issuecomment-99887766' } })) },
    issue_write: { schema: { type: 'object', properties: { method: {}, owner: {}, repo: {}, issue_number: {}, labels: {}, state: { enum: ['open', 'closed'] }, state_reason: {} }, required: ['method', 'owner', 'repo'] },
      run: calls.write || (() => ({ json: { id: 1, url: 'https://github.com/a/b/issues/7' } })) },
    label_write: { schema: obj(['method', 'owner', 'repo', 'name', 'color', 'description']), run: calls.label || (() => ({ json: { name: 'x' } })) },
  };
}

test('probe, tools listed directly, JSON answers: tools, identity, via and the tool each operation resolved to', async () => {
  const g = await gw({ tools: newer() });
  const a = await go(g, { op: 'probe', tracker: 'github' });
  assert.equal(a.ok, true);
  assert.ok(a.tools.includes('list_issues') && a.tools.includes('mcp-exec'));
  assert.equal(a.identity, 'octo-person');
  assert.equal(a.via, 'direct');
  assert.deepEqual(a.ops, { identity: 'get_me', list: 'list_issues', get: 'issue_read', comment_create: 'add_issue_comment', comment_update: null, set_labels: 'issue_write', create_label: 'label_write', close: 'issue_write', download: null });
  const first = g.seen[0];
  assert.equal(first.body.method, 'initialize');
  assert.equal(first.auth, TOKEN);
  assert.match(first.accept, /application\/json/);
  assert.match(first.accept, /text\/event-stream/);
  assert.equal(g.seen[1].body.method, 'notifications/initialized');
  assert.ok(g.seen.slice(1).every(s => s.session === 'sess-42' && s.protocol === '2025-06-18'), 'the session id and protocol are sent back');
});

test('probe through mcp-exec, event-stream answers: via says so', async () => {
  const g = await gw({ tools: newer(), exec: true, sse: true });
  const a = await go(g, { op: 'probe', tracker: 'github' });
  assert.equal(a.ok, true, JSON.stringify(a));
  assert.equal(a.via, 'mcp-exec');
  assert.equal(a.identity, 'octo-person');
  assert.ok(!a.tools.includes('get_me'));
  const c = g.calls().find(x => x.name === 'mcp-exec');
  assert.deepEqual(c.arguments, { name: 'get_me', arguments: {} });
});

test('probe with no tracker server, and with no gateway entry: missing_server', async () => {
  const g = await gw({ tools: {} });
  const a = await go(g, { op: 'probe', tracker: 'github' });
  assert.equal(a.ok, false);
  assert.equal(a.error.kind, 'missing_server');
  assert.match(a.error.message, /mcp-exec/);
  const none = await run({ op: 'probe', tracker: 'github' }, { home: home(g.port, null) });
  assert.equal(none.answer.error.kind, 'missing_server');
  const nofile = await run({ op: 'probe', tracker: 'github' });
  assert.equal(nofile.answer.error.kind, 'missing_server');
  assert.equal(nofile.code, 0);
});

test('list: open issues with the label, pull requests and others dropped, the url built here, next_page a cursor', async () => {
  const g = await gw({ tools: newer() });
  const a = await go(g, { op: 'list', tracker: 'github', repo: 'acme/shop', label: 'sbxw', page: null });
  assert.equal(a.ok, true);
  assert.deepEqual(a.issues.map(i => i.key), ['acme/shop#1', 'acme/shop#2']);
  const i = a.issues[0];
  assert.deepEqual(Object.keys(i).sort(), ['attachments', 'body', 'key', 'labels', 'number', 'state', 'title', 'updated_at', 'url']);
  assert.equal(i.url, 'https://github.com/acme/shop/issues/1', 'never the answer\'s own url');
  assert.equal(i.state, 'open');
  assert.equal(a.next_page, 'Y3Vyc29yOjI=');
  const call = g.calls().find(c => c.name === 'list_issues').arguments;
  assert.equal(call.state, 'OPEN', 'the enum\'s own case');
  assert.deepEqual(call.labels, ['sbxw']);
  assert.ok(!('page' in call) && !('per_page' in call), 'keys the schema lacks are not sent');
  // The cursor goes back as `after`.
  const b = await go(g, { op: 'list', tracker: 'github', repo: 'acme/shop', label: 'sbxw', page: a.next_page });
  assert.equal(b.ok, true);
  assert.equal(g.calls().filter(c => c.name === 'list_issues').pop().arguments.after, 'Y3Vyc29yOjI=');
});

test('list: hostile issue text stays data, bounded; attachments only on GitHub\'s hosts', async () => {
  const long = 'é'.repeat(20000);
  const body = [
    '![screen shot](https://github.com/user-attachments/assets/1234-abcd)',
    '<img width="10" alt="../../etc/passwd" src="https://private-user-images.githubusercontent.com/1/2.png?jwt=SECRETJWT">',
    '[log](https://github.com/acme/shop/files/55/run%2F..%2Flog.txt)',
    '![x](https://evil.example/a.png) https://github.com/acme/shop/blob/main/x.png',
    '`$(rm -rf /)` <script>alert(1)</script> {{constructor}}',
    long,
  ].join('\n');
  const list = [
    ISSUE(3, { title: '"><img src=x onerror=alert(1)>\u0000', body, labels: [{ name: 'SBXW' }, { name: '__proto__' }] }),
    ISSUE(4, { pull_request: { url: 'x' } }),
    ISSUE(5, { state: 'closed' }),
    ISSUE(6, { labels: [{ name: 'other' }] }),
    ISSUE(-1), { number: '../9' }, null, 'text',
  ];
  const g = await gw({ tools: newer({ list: () => ({ json: list }) }) });
  const a = await go(g, { op: 'list', tracker: 'github', repo: 'acme/shop', label: 'sbxw', page: 1 });
  assert.equal(a.ok, true, JSON.stringify(a.error));
  assert.deepEqual(a.issues.map(i => i.number), [3]);
  const i = a.issues[0];
  assert.equal(i.title, '"><img src=x onerror=alert(1)>\u0000', 'text is returned as it was, for the daemon to treat as data');
  assert.equal([...i.body].length, 16000, 'body cut at 16000 characters');
  assert.deepEqual(i.attachments.map(x => x.url), [
    'https://github.com/user-attachments/assets/1234-abcd',
    'https://private-user-images.githubusercontent.com/1/2.png?jwt=SECRETJWT',
    'https://github.com/acme/shop/files/55/run%2F..%2Flog.txt',
  ]);
  for (const x of i.attachments) assert.ok(!/[/\\]/.test(x.name) && !x.name.startsWith('.'), `a safe name: ${x.name}`);
  assert.equal(i.attachments[0].name, 'screen shot');
  assert.equal(a.next_page, null, 'a short page is the last');
});

test('list: the table falls through to the second candidate when the first is absent, or refuses its arguments', async () => {
  const items = { json: { total_count: 120, items: [ISSUE(8)] } };
  const tools = newer();
  delete tools.list_issues;
  tools.search_issues = { schema: obj(['query', 'perPage', 'page', 'sort', 'order']), run: () => items };
  let g = await gw({ tools });
  let a = await go(g, { op: 'list', tracker: 'github', repo: 'acme/shop', label: 'sbxw', page: 2 });
  assert.equal(a.ok, true);
  assert.deepEqual(a.issues.map(i => i.key), ['acme/shop#8']);
  assert.equal(a.next_page, 3);
  assert.equal(g.calls().find(c => c.name === 'search_issues').arguments.query, 'repo:acme/shop is:issue is:open label:"sbxw"');
  // Listed, but refusing the arguments: the next one.
  const t2 = { ...newer({ list: () => ({ error: 'Invalid argument: unknown parameter orderBy' }) }), search_issues: tools.search_issues };
  g = await gw({ tools: t2 });
  a = await go(g, { op: 'list', tracker: 'github', repo: 'acme/shop', label: 'sbxw', page: 1 });
  assert.equal(a.ok, true);
  assert.deepEqual(g.calls().map(c => c.name).filter(n => n !== 'get_me'), ['list_issues', 'search_issues']);
  // Listed with a schema needing a key this script does not know: skipped.
  const t3 = { ...newer(), search_issues: tools.search_issues };
  t3.list_issues = { ...t3.list_issues, schema: { type: 'object', properties: { owner: {}, repo: {}, project: {} }, required: ['owner', 'repo', 'project'] } };
  g = await gw({ tools: t3 });
  a = await go(g, { op: 'list', tracker: 'github', repo: 'acme/shop', label: 'sbxw', page: 1 });
  assert.equal(a.ok, true);
  assert.ok(!g.calls().some(c => c.name === 'list_issues'));
});

test('comment: created with its id; an update is unsupported by this server; through mcp-exec too', async () => {
  let g = await gw({ tools: newer() });
  let a = await go(g, { op: 'comment', tracker: 'github', key: 'acme/shop#7', comment_id: null, body: 'sbxw: In progress' });
  assert.deepEqual(a, { ok: true, comment_id: '99887766' });
  assert.deepEqual(g.calls().find(c => c.name === 'add_issue_comment').arguments, { owner: 'acme', repo: 'shop', issue_number: 7, body: 'sbxw: In progress' });
  a = await go(g, { op: 'comment', tracker: 'github', key: 'acme/shop#7', comment_id: '99887766', body: 'sbxw: In test' });
  assert.equal(a.ok, false);
  assert.equal(a.error.kind, 'unsupported');
  assert.ok(!g.calls().some(c => c.name === 'add_issue_comment' && c.arguments.body === 'sbxw: In test'), 'no second comment instead');
  g = await gw({ tools: newer({ comment: () => ({ text: 'Created comment https://github.com/acme/shop/issues/7#issuecomment-555' }) }), exec: true, sse: true });
  a = await go(g, { op: 'comment', tracker: 'github', key: 'acme/shop#7', body: 'x' });
  assert.deepEqual(a, { ok: true, comment_id: '555' });
  a = await go(g, { op: 'comment', tracker: 'github', key: 'acme/shop#7', comment_id: 5, body: 'y' });
  assert.equal(a.error.kind, 'unsupported', 'via mcp-exec, a tool the server lacks is unsupported, not missing_server');
});

test('label: the other labels with the prefix go, the new one is added, created first when it must exist', async () => {
  let g = await gw({ tools: newer() });
  let a = await go(g, { op: 'label', tracker: 'github', key: 'acme/shop#7', add: 'sbxw:in-test', remove_prefix: 'sbxw:' });
  assert.deepEqual(a, { ok: true, labels: ['bug', 'sbxw:in-test'] });
  assert.deepEqual(g.calls().find(c => c.name === 'issue_write').arguments, { method: 'update', owner: 'acme', repo: 'shop', issue_number: 7, labels: ['bug', 'sbxw:in-test'] });
  // Already right: nothing written.
  g = await gw({ tools: newer({ get: () => ({ json: { labels: ['bug', 'sbxw:planned'] } }) }) });
  a = await go(g, { op: 'label', tracker: 'github', key: 'acme/shop#7', add: 'sbxw:planned', remove_prefix: 'sbxw:' });
  assert.deepEqual(a, { ok: true, labels: ['bug', 'sbxw:planned'] });
  assert.ok(!g.calls().some(c => c.name === 'issue_write'));
  // The label must exist: created, then set.
  g = await gw({ tools: newer({ write: (_a, n) => (n === 1 ? { error: "Label 'sbxw:merged' does not exist" } : { json: { id: 1 } }) }) });
  a = await go(g, { op: 'label', tracker: 'github', key: 'acme/shop#7', add: 'sbxw:merged', remove_prefix: 'sbxw:' });
  assert.equal(a.ok, true, JSON.stringify(a));
  assert.deepEqual(g.calls().map(c => c.name), ['issue_read', 'issue_write', 'label_write', 'issue_write']);
  assert.equal(g.calls()[2].arguments.name, 'sbxw:merged');
});

test('close: the issue is closed, the state in the schema\'s case', async () => {
  const g = await gw({ tools: newer() });
  const a = await go(g, { op: 'close', tracker: 'github', key: 'acme/shop#7' });
  assert.deepEqual(a, { ok: true });
  assert.deepEqual(g.calls().find(c => c.name === 'issue_write').arguments, { method: 'update', owner: 'acme', repo: 'shop', issue_number: 7, state: 'closed', state_reason: 'completed' });
});

test('the older names: get_issue and update_issue when the newer are absent', async () => {
  const tools = {
    get_me: newer().get_me,
    get_issue: { schema: obj(['owner', 'repo', 'issue_number']), run: () => ({ json: { labels: [{ name: 'sbxw:planned' }] } }) },
    update_issue: { schema: obj(['owner', 'repo', 'issue_number', 'labels', 'state']), run: () => ({ json: { id: 1 } }) },
  };
  const g = await gw({ tools });
  const a = await go(g, { op: 'label', tracker: 'github', key: 'acme/shop#7', add: 'sbxw:in-progress', remove_prefix: 'sbxw:' });
  assert.deepEqual(a, { ok: true, labels: ['sbxw:in-progress'] });
  assert.deepEqual(g.calls().map(c => c.name), ['get_issue', 'update_issue']);
  const c = await go(g, { op: 'close', tracker: 'github', key: 'acme/shop#7' });
  assert.equal(c.ok, true);
  assert.deepEqual(g.calls().pop().arguments, { owner: 'acme', repo: 'shop', issue_number: 7, state: 'closed' });
});

test('each error kind', async () => {
  const kind = async (o, req = { op: 'close', tracker: 'github', key: 'acme/shop#7' }, extra) => (await go(await gw(o), req, extra)).error;
  assert.equal((await kind({ tools: newer({ write: () => ({ error: 'PATCH https://api.github.com/repos/acme/shop/issues/7: 401 Bad credentials []' }) }) })).kind, 'auth');
  assert.equal((await kind({ tools: newer(), http: b => (b.method === 'tools/list' ? [403] : null) })).kind, 'auth');
  const rl = await kind({ tools: newer(), http: b => (b.method === 'tools/call' ? [429, { 'Retry-After': '30' }] : null) });
  assert.deepEqual([rl.kind, rl.retry_after_s], ['rate_limit', 30]);
  assert.equal((await kind({ tools: newer({ write: () => ({ error: 'API rate limit exceeded for user' }) }) })).kind, 'rate_limit');
  assert.equal((await kind({ tools: newer({ write: () => ({ error: 'PATCH https://api.github.com/repos/acme/shop/issues/7: 404 Not Found []' }) }) })).kind, 'not_found');
  assert.equal((await kind({ tools: newer({ write: () => ({ rpcError: { code: -32000, message: 'Could not resolve to an Issue with the number of 7.' } }) }) })).kind, 'not_found');
  assert.equal((await kind({ tools: newer({ write: () => 'hang' }) }, undefined, { timeoutMs: 800 })).kind, 'network');
  assert.equal((await kind({ tools: newer({ write: () => 'drop' }) })).kind, 'network');
  assert.equal((await kind({ tools: newer(), http: b => (b.method === 'tools/call' ? [502] : null) })).kind, 'network');
  assert.equal((await kind({ tools: {} })).kind, 'missing_server');
  assert.equal((await kind({ tools: newer() }, { op: 'fly', tracker: 'github' })).kind, 'unsupported');
  assert.equal((await kind({ tools: newer() }, { op: 'close', tracker: 'jira', key: 'X-1' })).kind, 'unsupported');
  assert.equal((await kind({ tools: newer() }, { op: 'close', tracker: 'github', key: '../../etc#1' })).kind, 'unsupported');
  assert.equal((await kind({ tools: newer() }, { op: 'list', tracker: 'github', repo: 'a/b', label: 'x" OR "y' })).kind, 'unsupported');
  const bad = await run('=not json', { home: home(1) });
  assert.equal(bad.answer.error.kind, 'unsupported');
  assert.equal(bad.code, 0);
});

test('a dropped connection is tried once more; an ended session is started again', async () => {
  let g = await gw({ tools: newer(), http: (b, n) => (b.method === 'tools/call' && n === 1 ? 'drop' : null) });
  let a = await go(g, { op: 'close', tracker: 'github', key: 'acme/shop#7' });
  assert.equal(a.ok, true);
  assert.equal(g.calls().length, 2);
  g = await gw({ tools: newer(), http: (b, n) => (b.method === 'tools/call' && n === 1 ? [404] : null) });
  a = await go(g, { op: 'close', tracker: 'github', key: 'acme/shop#7' });
  assert.equal(a.ok, true);
  assert.equal(g.seen.filter(s => s.body.method === 'initialize').length, 2);
});

// ── fetch ──
function files(o = {}) {
  const server = http.createServer((req, res) => {
    const u = new URL(req.url, 'http://x');
    if (u.pathname === '/user-attachments/files/1/shot.png') { res.writeHead(200, { 'Content-Type': 'image/png', 'Content-Length': '8' }); res.end('PNGBYTES'); return; }
    if (u.pathname === '/big') { res.writeHead(200, { 'Content-Length': String(5000) }); res.end('x'.repeat(5000)); return; }
    if (u.pathname === '/stream') { res.writeHead(200); let i = 0; const t = setInterval(() => { res.write('y'.repeat(1000)); if (++i === 10) { clearInterval(t); res.end(); } }, 2); return; }
    if (u.pathname === '/hop') { res.writeHead(302, { Location: '/user-attachments/files/1/shot.png' }); res.end(); return; }
    if (u.pathname === '/away') { res.writeHead(302, { Location: o.away }); res.end(); return; }
    if (u.pathname === '/evil-name/..%2F..%2F.bashrc') { res.writeHead(200); res.end('hi'); return; }
    res.writeHead(404); res.end();
  });
  servers.push(server);
  return new Promise(r => server.listen(0, '127.0.0.1', () => r(server.address().port)));
}

test('fetch: on the tracker\'s hosts only, redirects checked, a name made here, the size limit kept', async () => {
  const away = await files();
  const port = await files({ away: `http://127.0.0.1:${away}/user-attachments/files/1/shot.png` });
  const tmp = mkdtempSync(join(tmpdir(), 'sbxw-tracker-files-'));
  const o = { fetchHosts: [`127.0.0.1:${port}`], tmp };
  const f = async (url, max = 1000) => {
    const r = await run({ op: 'fetch', tracker: 'github', url, max_bytes: max }, o);
    outputs.push(r.stdout, r.stderr);
    assert.equal(r.code, 0);
    return r.answer;
  };
  const base = `http://127.0.0.1:${port}`;
  let a = await f(`${base}/user-attachments/files/1/shot.png`);
  assert.equal(a.ok, true, JSON.stringify(a));
  assert.equal(a.name, 'shot.png');
  assert.equal(a.bytes, 8);
  assert.ok(a.path.startsWith(`${tmp}/`) && a.path.endsWith('.png'));
  assert.match(a.path.slice(tmp.length + 1), /^[0-9a-f-]{36}\.png$/);
  assert.equal(readFileSync(a.path, 'utf8'), 'PNGBYTES');
  a = await f(`${base}/hop`);
  assert.equal(a.ok, true, 'a redirect on the hosts is followed');
  a = await f(`${base}/evil-name/..%2F..%2F.bashrc`);
  assert.equal(a.ok, true);
  assert.ok(!a.name.includes('/') && !a.name.startsWith('.'), a.name);
  assert.equal(a.path.split('/').length, tmp.split('/').length + 1, 'the file is in the directory itself');
  const before = readdirSync(tmp).length;
  for (const [url, why] of [[`${base}/big`, 'content-length over'], [`${base}/stream`, 'stream over']]) {
    a = await f(url, 4000);
    assert.equal(a.error?.kind, 'unsupported', why);
  }
  assert.equal(readdirSync(tmp).length, before, 'the partial file is deleted');
  a = await f(`http://127.0.0.1:${away}/user-attachments/files/1/shot.png`);
  assert.equal(a.error.kind, 'unsupported');
  assert.match(a.error.message, /not one of github's hosts/);
  a = await f(`${base}/away`);
  assert.equal(a.error.kind, 'unsupported');
  assert.match(a.error.message, /redirected off/);
  a = await f(`${base}/missing`);
  assert.equal(a.error.kind, 'unsupported', 'a refused file (private) has no download in this connector');
  a = await f('file:///etc/passwd');
  assert.equal(a.error.kind, 'unsupported');
  a = await f(`http://user:pw@127.0.0.1:${port}/user-attachments/files/1/shot.png`);
  assert.equal(a.error.kind, 'unsupported');
  // As shipped: https and GitHub's hosts only.
  const real = await run({ op: 'fetch', tracker: 'github', url: `${base}/user-attachments/files/1/shot.png`, max_bytes: 100 }, { tmp });
  assert.equal(real.answer.error.kind, 'unsupported');
  assert.ok(!existsSync('/tmp/sbxw-tracker/never'));
});

test('no secret in any output: the gateway token, and a token a tracker error echoes', async () => {
  const g = await gw({ tools: newer({ write: () => ({ error: 'denied for Authorization: Bearer ghp_abcdefghijklmnopqrstuvwx1234 and github_pat_11ABCDEFG0123456789_xyz' }) }) });
  const a = await go(g, { op: 'close', tracker: 'github', key: 'acme/shop#7' });
  assert.equal(a.ok, false);
  const all = outputs.join('\n');
  assert.ok(all.length > 1000);
  for (const s of [TOKEN, TOKEN.split(' ')[1], 'ghp_abcdefghijklmnopqrstuvwx1234', 'github_pat_11ABCDEFG0123456789_xyz']) {
    assert.ok(!all.includes(s), `never printed: ${s.slice(0, 8)}…`);
  }
  // The answer is the last line, always.
  for (const out of outputs.filter((_, i) => i % 2 === 0)) assert.match(out.trimEnd().split('\n').pop(), /^SBXW-TRACKER \{/);
});
