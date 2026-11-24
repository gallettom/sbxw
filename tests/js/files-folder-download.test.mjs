// A whole folder downloads from the web UI's file explorer (assets/js/files.js):
// sized first through `fs/folder/size`, refused with its size and the cap
// when over it, else saved as `<name>.tar.gz` from `fs/folder`. The functions
// are taken from files.js itself and run against a small fake of the DOM.
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

function load(size) {
  const clicked = [], toasts = [], fetched = [];
  const sandbox = {
    filesTarget: 'web one',
    fx: { path: '/home/agent', entries: [], selected: null },
    toasts, clicked, fetched,
    showToast: (m, k) => toasts.push([m, k || 'ok']),
    escHtml: (s) => String(s),
    fetch: async (url) => { fetched.push(url); return { json: async () => size }; },
    document: { createElement: () => { const a = { click() { clicked.push({ href: a.href, download: a.download }); } }; return a; } },
  };
  const ctx = vm.createContext(sandbox);
  const names = ['humanSize', 'fxApi', 'fxFileUrl', 'fxFolderUrl', 'fxJoin', 'fxKey', 'fxPathOf', 'fxFolderSize', 'fxFolderOverCap', 'fxDownloadFolder', 'fxDownload'];
  vm.runInContext(names.map(decl).join('\n') + '\nthis.api = { fxDownload, fxDownloadFolder, fxFolderOverCap };', ctx);
  return sandbox;
}

const dir = { name: 'out dir', kind: 'dir' };
const MAX = 256 * 1024 * 1024;

test('a folder under the cap downloads as <name>.tar.gz from fs/folder, sized first', async () => {
  const sb = load({ ok: true, files: 3, bytes: 1200, max: MAX });
  await sb.api.fxDownloadFolder(dir);
  assert.deepEqual(sb.fetched, ['/api/sandboxes/web%20one/fs/folder/size?path=%2Fhome%2Fagent%2Fout%20dir']);
  assert.deepEqual(sb.clicked, [{ href: '/api/sandboxes/web%20one/fs/folder?path=%2Fhome%2Fagent%2Fout%20dir', download: 'out dir.tar.gz' }]);
});

test('a folder over the cap is refused with its size and the cap, nothing saved', async () => {
  const sb = load({ ok: true, files: 9, bytes: 300 * 1024 * 1024, max: MAX });
  await sb.api.fxDownloadFolder(dir);
  assert.deepEqual(sb.clicked, []);
  assert.equal(sb.toasts.length, 1);
  assert.equal(sb.toasts[0][1], 'error');
  assert.match(sb.toasts[0][0], /300\.0 MB: over the 256\.0 MB a folder download allows here/);
});

test('the row button of a folder downloads the folder, of a file the file', async () => {
  const sb = load({ ok: true, files: 1, bytes: 1, max: MAX });
  sb.api.fxDownload({ name: 'a.txt', kind: 'file' });
  assert.match(sb.clicked[0].href, /\/fs\/file\?path=%2Fhome%2Fagent%2Fa\.txt&download=1$/);
  sb.api.fxDownload(dir);
  await new Promise((r) => setTimeout(r, 0));
  assert.match(sb.clicked[1].href, /\/fs\/folder\?path=/);
  assert.equal(sb.api.fxFolderOverCap(null), null, 'a size the daemon could not give does not block');
});

test('folder rows carry a download button, as file rows do', () => {
  assert.match(files, /e\.kind === 'dir' \? `<button class="fx-dl"[^`]*title="Download the folder as/);
  assert.match(files, /data-folder-dl[^>]*>⬇ Download folder</);
});
