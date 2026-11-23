// The sbx 0.54 features' rules in the Mac web UI, without a browser: each
// file's rules are loaded on their own, between their BEGIN and END markers.
// Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

function rules(file, tag) {
  const src = readFileSync(new URL(`../../assets/js/${file}`, import.meta.url), 'utf8');
  const a = src.indexOf(`// BEGIN sbx-054 ${tag} rules`);
  const b = src.indexOf(`// END sbx-054 ${tag} rules`);
  assert.ok(a >= 0 && b > a, `${file}: markers`);
  return src.slice(a, b);
}

function load(file, tag, names) {
  const ctx = vm.createContext({});
  vm.runInContext(`${rules(file, tag)}\nglobalThis.api = { ${names.join(', ')} };`, ctx);
  return ctx.api;
}

test('a sandbox-scoped allow never sends global, and a global one says so', () => {
  const { deniedAllow } = load('create.js', 'denied', ['deniedAllow', 'deniedHeadline']);
  const here = deniedAllow({ sandbox: 'web', allow_scope: 'sandbox' }, 'registry.npmjs.org');
  assert.equal(here.label, 'Allow for this sandbox');
  assert.deepEqual(JSON.parse(JSON.stringify(here.body)), { resources: 'registry.npmjs.org', decision: 'allow' });
  assert.ok(!('global' in here.body), 'the sandbox button never carries global');
  assert.equal(here.line, '');

  for (const scope of ['global', undefined, 'other']) {
    const all = deniedAllow({ sandbox: 'web', allow_scope: scope }, 'github.com');
    assert.equal(all.label, 'Allow for all sandboxes', String(scope));
    assert.equal(all.body.global, true);
    assert.match(all.line, /every sandbox/);
    assert.match(all.confirm, /EVERY sandbox/);
    assert.notEqual(all.label, 'Allow for this sandbox');
  }
});

test('the denied headline names the step and where the hosts came from', () => {
  const { deniedHeadline } = load('create.js', 'denied', ['deniedHeadline']);
  assert.equal(deniedHeadline({ step: 'kits', kit: 'k8s', source: 'sbx' }), 'sbx refused these hosts while installing kit k8s:');
  assert.equal(deniedHeadline({ step: 'create', kit: null, source: 'policy_log' }), 'sbx refused these hosts while creating the sandbox (from the policy log):');
});

test('the publish result line says the port and why it is not automatic', () => {
  const { publishResultLine, policyWhyTarget } = load('ports.js', 'ports', ['publishResultLine', 'policyWhyTarget']);
  assert.equal(publishResultLine({ sandbox_port: 3000, host_ip: '127.0.0.1', host_port: 49153, assigned: true, note: null }),
    'Published sandbox port 3000 on 127.0.0.1:49153 (picked by sbx)');
  assert.equal(publishResultLine({ sandbox_port: 3000, host_ip: '127.0.0.1', host_port: 3000, assigned: false, note: 'automatic host port needs sbx 0.54: published on the sandbox port' }),
    'Published sandbox port 3000 on 127.0.0.1:3000. automatic host port needs sbx 0.54: published on the sandbox port');
  assert.match(publishResultLine({ sandbox_port: 80, host_ip: '127.0.0.1', host_port: 8080, forwarded_port: 51000 }), /reached here on port 51000/);
  assert.equal(publishResultLine({ ok: true }), 'Published');

  const { publishRefusal } = load('ports.js', 'ports', ['publishRefusal']);
  assert.match(publishRefusal('127.0.0.2', undefined), /only on the loopback address 127\.0\.0\.1: name the host port/);
  assert.match(publishRefusal('0.0.0.0', 0), /name the host port/);
  for (const [ip, hp] of [[undefined, undefined], ['', undefined], ['127.0.0.1', undefined], [' 127.0.0.1 ', undefined], ['127.0.0.2', 4200]]) {
    assert.equal(publishRefusal(ip, hp), null, `${ip} ${hp}`);
  }

  assert.equal(policyWhyTarget(' PyPI.org:443 '), 'pypi.org:443');
  assert.equal(policyWhyTarget('github.com'), 'github.com');
  for (const bad of ['', '*.github.com', 'https://a.com/x', 'a.com;rm', 'localhost', '-x.com', '<b>.com']) {
    assert.equal(policyWhyTarget(bad), null, bad);
  }
});

test('the MCP line is sbx\'s words or plainly unknown', () => {
  const { mcpModeLine } = load('mcp.js', 'mcp', ['mcpModeLine']);
  assert.equal(mcpModeLine('web', { ok: true, supported: true, mode: 'dynamic', servers: ['github', 'notion'] }), 'web: MCP mode dynamic; connected: github, notion');
  assert.equal(mcpModeLine('web', { ok: true, supported: true, mode: 'weird', servers: [] }), 'web: MCP mode unknown; connected: none');
  assert.equal(mcpModeLine('web', { ok: true, supported: false, mode: 'unknown', servers: [], note: 'needs sbx 0.54' }), 'web: MCP mode and connected servers needs sbx 0.54');
  assert.match(mcpModeLine('web', { ok: false, error: 'boom' }), /not read \(boom\)/);
});
