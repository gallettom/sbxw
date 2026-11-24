// The Mac web UI's Skills dialog (assets/js/skills.js, between its "skills
// pure" markers): after "Show its skills", "Installed in <sandbox>" lists
// every skill and command of GET /api/sandboxes/:name/commands, grouped by
// origin, read only; the keepable list's empty text no longer says "No
// skill" when only that list is empty; a daemon without the route (404)
// leaves the dialog as before.
// Run: node --test tests/js/*.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const read = p => readFileSync(new URL(`../../${p}`, import.meta.url), 'utf8');
const src = read('assets/js/skills.js');
const util = read('assets/js/util.js');
const esc = util.slice(util.indexOf('function escHtml'), util.indexOf('\n}\n', util.indexOf('function escHtml')) + 2);
const pure = src.slice(src.indexOf('// BEGIN skills pure'), src.indexOf('// END skills pure'));

const ctx = vm.createContext({});
vm.runInContext(`${esc}\n${pure}\nglobalThis.api = { skillsInstalledGroups, skillsInstalledHtml, skillsNothingToKeep, skillsOriginOf };`, ctx);
const { skillsInstalledGroups, skillsInstalledHtml, skillsNothingToKeep } = ctx.api;

const answer = () => ({ ok: true, complete: true, commands: [
  { name: 'archify', description: 'Draw a diagram.', kind: 'command', origin: 'kit:agent-skills' },
  { name: 'clear', description: 'Clear the conversation history', kind: 'command', origin: 'built-in' },
  { name: 'deploy', description: 'Deploy it.', kind: 'skill', origin: 'workspace' },
  { name: 'frontend-design', description: 'Visual design.', kind: 'skill', origin: 'home' },
  { name: 'handoff', description: 'Hand the work over.', kind: 'command', origin: 'home' },
  { name: 'notes', description: '', kind: 'skill', origin: 'home' },
  { name: 'release-notes', description: 'Write release notes.', kind: 'command', origin: 'home' },
] });
const kept = new Set(['release-notes']);
const own = new Set(['notes']);

test('every entry is listed, grouped by origin in plain words, kits first and built-ins last', () => {
  const g = skillsInstalledGroups(answer(), kept, own);
  assert.deepEqual(JSON.parse(JSON.stringify(g.map(x => [x.o.title, x.items.map(i => i.name).join(',')]))),
    [['Kit agent-skills', 'archify'], ['Kits', 'handoff'], ['Your kept skills', 'release-notes'],
     ['sbx shared skills', 'frontend-design'], ['Written in this sandbox', 'notes'], ['Project', 'deploy'], ['Built-in', 'clear']]);
  const html = skillsInstalledHtml(answer(), 'sbxw-2', kept, own);
  assert.match(html, /Installed in sbxw-2 <span class="skills-kit">7 installed<\/span>/);
  assert.match(html, /<code>\/handoff<\/code>\s*<div class="my-skill-about">Hand the work over\.<\/div>/);
});

test('read only: no Keep, no button at all, in the installed section', () => {
  const html = skillsInstalledHtml(answer(), 'sbxw-2', kept, own);
  assert.ok(!/<button/.test(html), html);
  assert.ok(!/data-keep/.test(html));
  // The keepable list keeps its Keep button, unchanged.
  assert.match(src, /data-keep="\$\{escHtml\(s\.name\)\}" data-origin="\$\{escHtml\(s\.origin\)\}"/);
});

test('sandbox texts are escaped and cut', () => {
  const a = { ok: true, complete: false, reason: '<img src=x onerror=alert(1)>', commands: [
    { name: 'x', description: `<b>${'y'.repeat(400)}`, kind: '<i>', origin: '<script>' },
  ] };
  const html = skillsInstalledHtml(a, '<sb>', kept, own);
  assert.ok(!/<img|<script|<b>|<i>|<sb>/.test(html), html);
  assert.match(html, /&lt;img src=x/);
  const d = /<div class="my-skill-about">([^<]*)<\/div>/.exec(html)[1];
  assert.ok(d.endsWith('…') && d.length < 200, d);
});

test('a stopped sandbox: what the route read, with its reason as a quiet line', () => {
  const a = { ok: true, complete: false, reason: 'The sandbox is stopped: only the project entries and the built-in ones.', commands: [
    { name: 'deploy', description: 'Deploy it.', kind: 'skill', origin: 'workspace' },
    { name: 'compact', description: 'Free up context', kind: 'command', origin: 'built-in' },
  ] };
  const html = skillsInstalledHtml(a, 'web-api', kept, own);
  assert.match(html, /<p class="repo-note" data-installed-reason>The sandbox is stopped: only the project entries/);
  assert.match(html, /2 installed/);
  // The route is read without starting anything: one GET, no start call.
  assert.ok(!/\/start/.test(src));
});

test('a daemon without the route (404) leaves the dialog as before', () => {
  assert.equal(skillsInstalledHtml(null, 'sbxw-2', kept, own), '');
  assert.equal(skillsInstalledHtml({ ok: false, error: 'not found' }, 'sbxw-2', kept, own), '');
  assert.match(src, /if \(!res\.ok\) return null;/);
  assert.match(skillsNothingToKeep('sbxw-2', false), /^<p class="repo-note">No skill in sbxw-2: none under/);
});

test('an empty keepable list no longer says "No skill" when the installed list is there', () => {
  const t = skillsNothingToKeep('sbxw-2', true);
  assert.match(t, /^<p class="repo-note">Nothing to keep: no skill the agent wrote under the project’s <code>\.claude\/skills\/<\/code>/);
  assert.ok(!t.includes('No skill'));
  assert.ok(!pure.includes('—'), 'no em dash');
});
