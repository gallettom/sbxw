// The Approvals sandbox filter and the badge rule, as pure functions of
// remote-core.js (no browser): entries and counts from the contract's list
// answers, each kind filtered, the default on a current leader, the badge
// number (plans and merges) and dot (network requests).
// Run: node --test tests/js/remote-approvals-core.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const read = p => readFileSync(new URL(p, import.meta.url), 'utf8');
const fx = JSON.parse(read('./remote-fixtures.json'));
const NAMES = ['rcLeaders', 'rcApSandbox', 'rcApEntries', 'rcApFilter', 'rcApDefault', 'rcApBadge'];
const rc = {};
vm.runInNewContext(`${read('../../assets/remote/remote-core.js')}\n${NAMES.map(n => `this.${n} = ${n};`).join('\n')}`, rc);
const plain = v => JSON.parse(JSON.stringify(v));

// ── Approvals: the sandbox filter (display only) ──
const AP = () => {
  const L = structuredClone(fx.level2);
  const lists = { plans: L.plans.plans, merges: L.merges.merges, network: L.network.requests, stopped: L.stopped.sandboxes };
  // The contract's examples: shop (plan), website (merge), docs (network), old-experiment (stopped).
  lists.plans.push({ ...lists.plans[0], id: `p-${'3'.repeat(32)}`, sandbox: 'website' });
  lists.network.push({ ...lists.network[0], id: 'network:def456', sandbox: 'shop', host: 'github.com' });
  return lists;
};
const leadersOf = () => rc.rcLeaders(fx.leaders);

test('approvals filter: one entry per sandbox with something waiting, counts, leaders order', () => {
  const e = plain(rc.rcApEntries(AP(), leadersOf()));
  assert.deepEqual(e.map(x => [x.sandbox, x.count]), [['shop', 2], ['docs', 1], ['website', 2], ['old-experiment', 1]]);
  assert.equal(e[0].name, 'shop');
  // a stopped sandbox is its own sandbox (`name`), the others say `sandbox`
  assert.equal(rc.rcApSandbox('stopped', { name: 'x' }), 'x');
  assert.equal(rc.rcApSandbox('plans', { sandbox: 'y', name: 'x' }), 'y');
  assert.equal(rc.rcApSandbox('plans', {}), '');
  assert.deepEqual(plain(rc.rcApEntries({}, [])), []);
});

test('approvals filter: each kind is filtered, and All keeps everything', () => {
  const l = AP();
  assert.equal(rc.rcApFilter('plans', l.plans, 'shop').length, 1);
  assert.equal(rc.rcApFilter('plans', l.plans, 'website').length, 1);
  assert.equal(rc.rcApFilter('merges', l.merges, 'website').length, 1);
  assert.equal(rc.rcApFilter('merges', l.merges, 'shop').length, 0);
  assert.equal(rc.rcApFilter('network', l.network, 'shop').length, 1);
  assert.equal(rc.rcApFilter('network', l.network, 'docs')[0].host, 'registry.npmjs.org');
  assert.equal(rc.rcApFilter('stopped', l.stopped, 'old-experiment').length, 1);
  assert.equal(rc.rcApFilter('stopped', l.stopped, 'shop').length, 0);
  for (const k of Object.keys(l)) assert.equal(rc.rcApFilter(k, l[k], null).length, l[k].length);
  assert.deepEqual(plain(rc.rcApFilter('plans', undefined, 'shop')), []);
});

test('approvals filter: starts on the current leader only when it has something waiting', () => {
  const e = rc.rcApEntries(AP(), leadersOf());
  assert.equal(rc.rcApDefault('shop', e), 'shop');
  assert.equal(rc.rcApDefault('payments-service-with-a-very-long-sandbox-name', e), null);
  assert.equal(rc.rcApDefault(null, e), null);
  // a chosen sandbox that emptied stays in the strip, at zero, after those with items
  const gone = plain(rc.rcApEntries({ plans: [] , network: [{ sandbox: 'docs' }] }, leadersOf(), 'shop'));
  assert.deepEqual(gone.map(x => [x.sandbox, x.count]), [['docs', 1], ['shop', 0]]);
  assert.equal(rc.rcApDefault('shop', gone), null);
});

test('approvals badge: the number is plans and merges, network is a dot, stopped is nothing', () => {
  assert.deepEqual(plain(rc.rcApBadge({ plans: 1, merges: 1, network: 7, stopped: 0 })), { n: 2, dot: true });
  assert.deepEqual(plain(rc.rcApBadge({ plans: 0, merges: 0, network: 3, stopped: 0 })), { n: 0, dot: true });
  assert.deepEqual(plain(rc.rcApBadge({ plans: 0, merges: 0, network: 0, stopped: 2 })), { n: 0, dot: false });
  assert.deepEqual(plain(rc.rcApBadge(null)), { n: 0, dot: false });
  assert.deepEqual(plain(rc.rcApBadge({ plans: 'x', merges: -4, network: 1.9 })), { n: 0, dot: true });
});

test('approvals filter: per-sandbox counts follow the badge rule', () => {
  const e = plain(rc.rcApEntries(AP(), leadersOf()));
  const by = Object.fromEntries(e.map(x => [x.sandbox, x]));
  assert.deepEqual([by.shop.n, by.shop.dot, by.shop.count], [1, true, 2]); // a plan; a network request
  assert.deepEqual([by.docs.n, by.docs.dot], [0, true]);
  assert.deepEqual([by['old-experiment'].n, by['old-experiment'].dot, by['old-experiment'].count], [0, false, 1]);
});
