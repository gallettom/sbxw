// The MCP dialog's OAuth client rules (assets/js/mcp.js), without a browser:
// loaded on their own, between their BEGIN and END markers.
// Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

function load() {
  const src = readFileSync(new URL('../../assets/js/mcp.js', import.meta.url), 'utf8');
  const a = src.indexOf('// BEGIN mcp oauth rules');
  const b = src.indexOf('// END mcp oauth rules');
  assert.ok(a >= 0 && b > a, 'markers');
  const ctx = vm.createContext({});
  vm.runInContext(`${src.slice(a, b)}\nglobalThis.api = { mcpSecretKeys, mcpClientIdOf };`, ctx);
  return ctx.api;
}

test('the client secret is asked only once a client ID is filled, after the header secrets', () => {
  const { mcpSecretKeys } = load();
  const plain = v => JSON.parse(JSON.stringify(v));
  assert.deepEqual(plain(mcpSecretKeys('Authorization: Bearer ${pat}\nX: ${pat}', '')), ['pat']);
  assert.deepEqual(plain(mcpSecretKeys('Authorization: Bearer ${pat}', ' Iv1.abc ')), ['pat', 'client_secret']);
  assert.deepEqual(plain(mcpSecretKeys('', '   ')), []);
  assert.deepEqual(plain(mcpSecretKeys('X: ${client_secret}', 'abc')), ['client_secret'], 'one field, never two of one key');
});

test('Edit pre-fills the client ID sbx lists, and nothing else under that name', () => {
  const { mcpClientIdOf } = load();
  assert.equal(mcpClientIdOf({ client_id: 'abc' }), 'abc');
  assert.equal(mcpClientIdOf({ oauth: { clientId: ' Iv1.x ' } }), 'Iv1.x');
  assert.equal(mcpClientIdOf({ oauth: { client_secret: 'nope' } }), '');
  assert.equal(mcpClientIdOf({ client_id: 42 }), '');
  assert.equal(mcpClientIdOf(null), '');
});
