// A link from a chat opens the web UI's file explorer (assets/js/files.js,
// fxOpenPath): a file in its folder, selected; a folder (ending in /) listed
// with no row selected; an anchored path with no extension listed as a folder
// when it is one, else selected as a file; a missing one says so. The
// functions are taken from files.js itself and run against a fake listing.
// Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const files = readFileSync(new URL('../../assets/js/files.js', import.meta.url), 'utf8');

function decl(name) {
  const m = new RegExp(`^(?:async\\s+)?(?:function|const|let)\\s+${name}\\b`, 'm').exec(files);
  assert.ok(m, `files.js declares ${name}`);
  const rest = files.slice(m.index + 1);
  const next = rest.search(/^(?:\/\/\/|\/\*|(?:async\s+)?function |const |let |document\.)/m);
  return files.slice(m.index, m.index + 1 + (next < 0 ? rest.length : next));
}

// The sandbox's tree: folder -> entries.
const TREE = {
  '/tmp/sbxw-chat/test': [{ name: 'graph-lookup', kind: 'dir' }],
  '/tmp/sbxw-chat/test/graph-lookup': [{ name: 'README.md', kind: 'file' }, { name: '.env.example', kind: 'file' }],
  '/home/agent/dev/sbxw': [{ name: 'Makefile', kind: 'file' }, { name: 'src', kind: 'dir' }],
  '/home/agent/dev/sbxw/src': [{ name: 'main.rs', kind: 'file' }],
  '/ws': [{ name: 'docs', kind: 'dir' }],
  '/ws/docs': [{ name: 'a.md', kind: 'file' }, { name: 'README.md', kind: 'file' }],
};

function load() {
  const listed = [], shown = [], notices = [];
  const el = () => ({ innerHTML: '', value: '', disabled: false, classList: { remove() {}, contains: () => false }, focus() {} });
  const sb = {
    filesTarget: 'box',
    filesOverlay: { open: true },
    fx: { seq: 0, path: '', history: [], places: { workspace: '/ws', home: '/home/agent' }, entries: [] },
    FX_RECENT: {},
    FX_README: /^readme\.md$/i,
    fxTbody: el(),
    sandboxes: [{ name: 'box', workspace: '/ws' }],
    document: { getElementById: el, querySelector: el },
    openFilesModal: async () => {},
    fxApi: (_, q) => `/api/sandboxes/box/fs${q}`,
    fetch: async (url) => {
      const path = decodeURIComponent(url.split('?path=')[1]);
      listed.push(path);
      const entries = TREE[path];
      return { json: async () => (entries ? { ok: true, path, parent: path.replace(/\/[^/]*$/, '') || '/', entries, workspace: '/ws', home: '/home/agent' } : { ok: false, error: `${path}: not found` }) };
    },
    fxNotice: (m, k) => { if (m) notices.push([m, k]); },
    fxWorktreeOf: () => null,
    renderCrumbs() {}, fxRenderFavourites() {}, renderList() {},
    fxShow: (e) => shown.push(e.name),
    fxPreviewEmpty() {},
  };
  const ctx = vm.createContext(sb);
  vm.runInContext(['fxJoin', 'fxOpen', 'fxResolvePath', 'fxOpenPath'].map(decl).join('\n') + '\nthis.open = fxOpenPath;', ctx);
  return { sb, listed, shown, notices };
}

test('a folder link lists that folder, no row selected (not even its README)', async () => {
  const t = load();
  await t.sb.open('box', '/tmp/sbxw-chat/test/graph-lookup/');
  assert.deepEqual(t.listed, ['/tmp/sbxw-chat/test/graph-lookup']);
  assert.equal(t.sb.fx.path, '/tmp/sbxw-chat/test/graph-lookup');
  assert.equal(t.sb.fx.selected, null);
  assert.deepEqual(t.shown, []);
});

test('an anchored path with no extension: a folder is entered, a file selected', async () => {
  let t = load();
  await t.sb.open('box', '/tmp/sbxw-chat/test/graph-lookup');
  assert.equal(t.sb.fx.path, '/tmp/sbxw-chat/test/graph-lookup');
  assert.equal(t.sb.fx.selected, null);
  assert.deepEqual(t.shown, []);
  t = load();
  await t.sb.open('box', '~/dev/sbxw/Makefile');
  assert.equal(t.sb.fx.path, '/home/agent/dev/sbxw');
  assert.deepEqual(t.shown, ['Makefile']);
});

test('a file link is as before; a relative folder resolves in the workspace; a missing one says so', async () => {
  let t = load();
  await t.sb.open('box', 'docs/a.md');
  assert.deepEqual([t.sb.fx.path, t.shown], ['/ws/docs', ['a.md']]);
  t = load();
  await t.sb.open('box', 'docs/');
  assert.deepEqual([t.sb.fx.path, t.sb.fx.selected, t.shown], ['/ws/docs', null, []]);
  t = load();
  await t.sb.open('box', '/tmp/nope/');
  assert.match(t.notices.at(-1)[0], /not found/);
  t = load();
  await t.sb.open('box', '~/dev/sbxw/nothing');
  assert.match(t.notices.at(-1)[0], /Not found in box: \/home\/agent\/dev\/sbxw\/nothing/);
});
