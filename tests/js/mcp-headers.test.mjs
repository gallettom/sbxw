// The MCP dialog's credential header rule (assets/js/mcp.js, its BEGIN and
// END markers), on the table the core and the desktop are tested on too
// (tests/js/mcp-header-cases.json). Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const read = p => readFileSync(new URL(`../../${p}`, import.meta.url), 'utf8');
const cases = JSON.parse(read('tests/js/mcp-header-cases.json'));

function load() {
  const src = read('assets/js/mcp.js');
  const a = src.indexOf('// BEGIN mcp header rules');
  const b = src.indexOf('// END mcp header rules');
  assert.ok(a >= 0 && b > a, 'markers');
  const ctx = vm.createContext({});
  vm.runInContext(`${src.slice(a, b)}\nglobalThis.api = { MCP_CREDENTIAL_WORDS, mcpCredentialHeader, mcpHeaderRefusal, mcpHeadersRefusal };`, ctx);
  return ctx.api;
}

const message = name => cases.message.replace('NAME', name)
  .replace('EXAMPLE', cases.example[name.toLowerCase()] || cases.example.other);

test('the shared table: credential names need a well-formed placeholder, other names stay free', () => {
  const { MCP_CREDENTIAL_WORDS, mcpCredentialHeader, mcpHeaderRefusal } = load();
  assert.deepEqual([...MCP_CREDENTIAL_WORDS], cases.words);
  for (const name of cases.credential) {
    assert.ok(mcpCredentialHeader(name), name);
    for (const v of cases.values.ok) assert.equal(mcpHeaderRefusal(`${name}: ${v}`), '', `${name}: ${v}`);
    for (const v of cases.values.refused) assert.equal(mcpHeaderRefusal(`${name}: ${v}`), message(name), `${name}: ${v}`);
  }
  for (const name of cases.free) {
    assert.ok(!mcpCredentialHeader(name), name);
    for (const v of cases.values.refused) assert.equal(mcpHeaderRefusal(`${name}: ${v}`), '', `${name}: ${v}`);
  }
});

test('the form says the first refusal and never a part of the value', () => {
  const { mcpHeadersRefusal } = load();
  assert.equal(mcpHeadersRefusal('Accept: application/json\nX-MCP-Toolsets: repos'), '');
  assert.equal(mcpHeadersRefusal(''), '');
  const why = mcpHeadersRefusal('Accept: application/json\n  Authorization: Bearer ghp_Sup3rS3cret  \nCookie: x');
  assert.equal(why, message('Authorization'));
  assert.ok(!why.includes('ghp_') && !why.includes('Sup3r'));
});
