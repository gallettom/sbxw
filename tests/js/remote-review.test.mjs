// Independent security review of remote access (thread remote-review): the
// agent-text renderer of the phone page, fed hostile Markdown. Only the fixed
// subset's tags may come out, with only their attributes, and every href must
// be an http(s) URL that cannot leave its quotes.
// Run: node --test tests/js/remote-review.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const src = readFileSync(new URL('../../assets/remote/remote-core.js', import.meta.url), 'utf8');
const ctx = {};
vm.runInNewContext(`${src}\nthis.rcAgentHtml = rcAgentHtml;`, ctx);

const HOSTILE = [
  '<img src=x onerror=alert(1)>',
  '<script>alert(1)</script>',
  '[x](javascript:alert(1))',
  '[x](https://a.example/"onmouseover="alert(1))',
  "[x](https://a.example/'><img src=x onerror=alert(1)>)",
  '[**x**](https://a.example/)',
  '[`<b>`](https://a.example/)',
  '`</code><img src=x onerror=1>`',
  '```\n</pre><svg onload=alert(1)>\n```',
  '' + '0' + ' [a](https://x.example)',
  '1. <iframe src=//evil>',
  '# <a href="javascript:1">h</a>',
  '[a](https://x.example/ onload=1)',
  'data:text/html,<script>1</script>',
];
const ALLOWED = { p: [], br: [], ul: [], ol: ['start'], li: [], strong: [], em: [], code: [], pre: [], a: ['class', 'href', 'target', 'rel'], span: ['class'] };

test('hostile agent text yields only the subset, with safe links', () => {
  for (const t of HOSTILE) {
    const html = ctx.rcAgentHtml(t);
    for (const m of html.matchAll(/<\/?([a-zA-Z0-9]+)([^>]*)>/g)) {
      const tag = m[1].toLowerCase();
      assert.ok(tag in ALLOWED, `tag <${tag}> from ${JSON.stringify(t)}: ${html}`);
      for (const a of m[2].matchAll(/([a-zA-Z-]+)\s*=\s*"([^"]*)"/g)) {
        assert.ok(ALLOWED[tag].includes(a[1]), `attribute ${a[1]} on <${tag}> from ${JSON.stringify(t)}: ${html}`);
        if (a[1] === 'href') assert.match(a[2], /^https?:\/\/[^\s"'<>]+$/, `href from ${JSON.stringify(t)}: ${html}`);
      }
      const leftover = m[2].replace(/([a-zA-Z-]+)\s*=\s*"[^"]*"/g, '').trim();
      assert.equal(leftover, '', `unquoted attribute text on <${tag}> from ${JSON.stringify(t)}: ${html}`);
    }
  }
});
