// The phone page's cost, measured in node (thread phone-freeze-audit):
// remote-core.js and remote.js run in the stand-in DOM of remote-dom.mjs,
// against a conversation of 50, 500 and 2000 rows of realistic text (long
// Markdown replies with code blocks, steps rows, status labels, file chips,
// thumbnails), and each step a person or the Mac causes is timed:
// the first render, one new row (at the end, and scrolled up), a steps row
// folding, the Working line toggling, a leaders event, 30 older rows, and
// Conversation / Kanban / Canvas. Every text is invented.
//
// For each: the time (ms, median of REPS runs), elements created, nodes
// inserted and removed, Markdown parses (innerHTML), forced layouts (a
// layout read after a write), ResizeObserver calls, requests made.
// Not iPhone times: compare one version of the page with another.
//
// Run: node tests/js/remote-perf.mjs [--json] [--sizes 50,500,2000] [--reps 5]
//      RC_CORE=<file> RC_PAGE=<file> to measure another copy of the page.
import { readFileSync } from 'node:fs';
import { performance } from 'node:perf_hooks';
import { bootPage } from './remote-dom.mjs';

const FX = JSON.parse(readFileSync(new URL('./remote-fixtures.json', import.meta.url), 'utf8'));
const args = process.argv.slice(2);
const opt = (k, d) => { const i = args.indexOf(k); return i >= 0 ? args[i + 1] : d; };
const SIZES = opt('--sizes', '50,500,2000').split(',').map(Number);
const REPS = Number(opt('--reps', '5'));
const JSON_OUT = args.includes('--json');

// ── Realistic text ──
const WORDS = 'the orders table needs an index on the customer id before the migration runs so that reads stay fast when the shop grows and the threads can merge'.split(' ');
const words = (n, k) => Array.from({ length: n }, (_, i) => WORDS[(i * 7 + k) % WORDS.length]).join(' ');
export function reply(k) {
  const code = Array.from({ length: 18 }, (_, i) => `  const row${i} = await db.query('select * from orders where id = $1', [id${i}]);`).join('\n');
  return [
    `## Step ${k}: what changed`,
    '',
    `${words(60, k)} **${words(4, k + 1)}** and \`orders_idx_${k}\` with *care*.`,
    '',
    `- ${words(14, k + 2)} [[Done]]`,
    `- ${words(12, k + 3)} \`src/db/orders.rs\``,
    `- ${words(10, k + 4)} [[Failed: 3 tests]]`,
    '',
    '```ts',
    code,
    '```',
    '',
    `1. ${words(16, k + 5)}`,
    `2. ${words(9, k + 6)} [the guide](https://example.org/guide?n=${k})`,
    '',
    `${words(40, k + 7)} [[Note: check the plan]]`,
  ].join('\n');
}
// About `rows` chat rows: prompts (some with files and kept images), steps,
// replies, a note now and then; the newest seq last, ending on a steps row
// so the next steps item folds into it.
export function conversation(rows, from = 1) {
  const items = [];
  let seq = from;
  let k = 0;
  let n = 0;
  while (n < rows) {
    const files = k % 5 === 0 ? ['plan.md', 'screen.png'] : null;
    const it = { seq: seq++, at: 1763600000000 + seq * 1000, kind: 'user_prompt', text: `${words(25, k)}?` };
    if (files) { it.files = files; it.images = ['', (k.toString(16).padStart(2, '0')).repeat(16)]; }
    items.push(it); n++;
    if (n >= rows) break;
    items.push({ seq: seq++, at: 1763600000000 + seq * 1000, kind: 'steps', count: 3 + (k % 4) }); n++;
    if (n >= rows) break;
    items.push({ seq: seq++, at: 1763600000000 + seq * 1000, kind: 'assistant_text', text: reply(k) }); n++;
    if (k % 7 === 3 && n < rows) { items.push({ seq: seq++, at: 1763600000000 + seq * 1000, kind: 'injected', from: 'sbxw', text: `Thread t${k} reported done.` }); n++; }
    k++;
  }
  if (items[items.length - 1].kind !== 'steps') items.push({ seq: seq++, at: 1763600000000 + seq * 1000, kind: 'steps', count: 2 });
  return items;
}

const leaders = state => ({ leaders: [{ sandbox: 'big', name: 'big', kind: 'leader', state, activity: state === 'working' ? 'Running tests' : 'Waiting for a message', last_at: 1763649900000, asks: false }] });

// A page booted, paired, with the leader `big` open on `items`.
export async function openBig(items, o = {}) {
  const S = { items, older: o.older || [], newer: [], leaders: leaders('working'), hang: o.hang || (() => false) };
  const route = (path, init) => {
    if (S.hang(path, init)) return undefined;
    const u = new URL(path, 'https://mac.local');
    const p = u.pathname;
    if (p === '/r/me') return { status: 200, body: FX.me };
    if (p === '/r/leaders') return { status: 200, body: S.leaders };
    if (p === '/r/models') return { status: 200, body: { models: [{ id: 'opus', label: 'Opus' }] } };
    if (p === '/r/skills') return { status: 200, body: { skills: [] } };
    if (p === '/r/usage') return { status: 200, body: {} };
    if (p === '/r/approvals') return { status: 404, body: {} };
    if (p === '/r/leaders/big/canvas') return { status: 200, body: { canvases: [] } };
    if (p === '/r/leaders/big/kanban') return { status: 200, body: { ...FX.kanban.board, sandbox: 'big' } };
    if (p === '/r/leaders/big/message') return { status: 200, body: { ok: true, masked: 0 } };
    if (p === '/r/leaders/big/history') {
      const all = S.items;
      const latest = all.length ? all[all.length - 1].seq : 0;
      if (u.searchParams.has('before')) return { status: 200, body: { page: { items: S.older, next_before: S.older.length ? S.older[0].seq : null, latest_seq: latest, epoch: 'e1' } } };
      if (u.searchParams.has('after')) {
        const after = Number(u.searchParams.get('after'));
        const got = S.newer.filter(i => i.seq > after);
        return { status: 200, body: { page: { items: got, latest_seq: Math.max(latest, ...got.map(i => i.seq)), epoch: 'e1' } } };
      }
      return { status: 200, body: { page: { items: all, next_before: S.older.length ? all[0].seq : null, latest_seq: latest, epoch: 'e1' } } };
    }
    return { status: 404, body: {} };
  };
  const dom = await bootPage(route, { core: process.env.RC_CORE, page: process.env.RC_PAGE });
  dom.S = S;
  const $ = id => dom.document.getElementById(id);
  dom.$ = $;
  const btn = $('leaders').querySelectorAll('button').find(b => b.textContent.includes('big'));
  dom.open = async () => { btn.click(); await dom.advance(0); };
  if (!o.closed) await dom.open();
  return dom;
}

function reset(c) {
  Object.assign(c, { created: 0, inserted: 0, removed: 0, innerHTML: 0, layouts: 0, layoutReads: 0, roCalls: 0, styleSets: 0 });
  c.fetches.length = 0;
}
async function measure(dom, act) {
  reset(dom.counters);
  const t0 = performance.now();
  await act();
  await dom.advance(0);
  const ms = performance.now() - t0;
  const c = dom.counters;
  return { ms, created: c.created, inserted: c.inserted, removed: c.removed, md: c.innerHTML, layouts: c.layouts, reads: c.layoutReads, ro: c.roCalls, fetches: c.fetches.length };
}

// The steps, each from a fresh page of `n` rows (setup not timed).
export const STEPS = {
  async 'first render'(n) {
    const dom = await openBig(conversation(n), { closed: true });
    return measure(dom, () => dom.open());
  },
  async 'new row (at end)'(n) {
    const items = conversation(n);
    const dom = await openBig(items);
    const s = items[items.length - 1].seq;
    dom.S.newer = [{ seq: s + 1, at: 1763650000000, kind: 'assistant_text', text: reply(9999) }];
    return measure(dom, () => dom.emit('history', { sandbox: 'big' }));
  },
  async 'new row (scrolled up)'(n) {
    const items = conversation(n);
    const dom = await openBig(items);
    const log = dom.$('log');
    log.scrollTop = Math.max(0, log.scrollHeight / 2);
    await dom.advance(0);
    const s = items[items.length - 1].seq;
    dom.S.newer = [{ seq: s + 1, at: 1763650000000, kind: 'assistant_text', text: reply(9999) }];
    return measure(dom, () => dom.emit('history', { sandbox: 'big' }));
  },
  async 'steps row folds'(n) {
    const items = conversation(n);
    const dom = await openBig(items);
    const s = items[items.length - 1].seq;
    dom.S.newer = [{ seq: s + 1, at: 1763650000000, kind: 'steps', count: 1 }];
    return measure(dom, () => dom.emit('history', { sandbox: 'big' }));
  },
  async 'working line toggles'(n) {
    const dom = await openBig(conversation(n));
    return measure(dom, () => dom.emit('leaders', leaders('idle')));
  },
  async 'leaders event'(n) {
    const dom = await openBig(conversation(n));
    const l = leaders('working');
    l.leaders[0].activity = 'Editing src/db/orders.rs';
    return measure(dom, () => dom.emit('leaders', l));
  },
  async '30 older rows'(n) {
    const items = conversation(n, 100000);
    const older = conversation(30, 1).filter(i => i.seq < 100000);
    const dom = await openBig(items, { older });
    const log = dom.$('log');
    return measure(dom, async () => { log.scrollTop = 0; });
  },
  async 'to Kanban and back'(n) {
    const dom = await openBig(conversation(n));
    return measure(dom, async () => { dom.$('v-kanban').click(); await dom.advance(0); dom.$('v-chat').click(); });
  },
  async 'to Canvas and back'(n) {
    const dom = await openBig(conversation(n));
    return measure(dom, async () => { dom.$('v-canvas').click(); await dom.advance(0); dom.$('v-chat').click(); });
  },
  async 'send a message'(n) {
    const dom = await openBig(conversation(n));
    const f = dom.$('field');
    f.value = 'Please add the index now';
    f.dispatchEvent(new dom.Event('input'));
    return measure(dom, async () => { dom.$('send').click(); await dom.advance(0); });
  },
};

const median = xs => { const s = [...xs].sort((a, b) => a - b); return s[Math.floor(s.length / 2)]; };
export async function run(sizes = SIZES, reps = REPS, only) {
  const out = [];
  for (const [name, fn] of Object.entries(STEPS)) {
    if (only && !only.includes(name)) continue;
    for (const n of sizes) {
      const runs = [];
      for (let i = 0; i < reps; i += 1) runs.push(await fn(n));
      const last = runs[runs.length - 1];
      out.push({ step: name, rows: n, ...last, ms: Number(median(runs.map(r => r.ms)).toFixed(2)) });
    }
  }
  return out;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const res = await run();
  if (JSON_OUT) process.stdout.write(`${JSON.stringify(res, null, 1)}\n`);
  else {
    const cols = ['step', 'rows', 'ms', 'created', 'inserted', 'removed', 'md', 'layouts', 'reads', 'ro', 'fetches'];
    console.log(`| ${cols.join(' | ')} |`);
    console.log(`|${cols.map(() => '---').join('|')}|`);
    for (const r of res) console.log(`| ${cols.map(c => r[c]).join(' | ')} |`);
  }
}
