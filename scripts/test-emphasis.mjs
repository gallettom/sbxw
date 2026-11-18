#!/usr/bin/env node
// The web terminal's status pill matcher (assets/js/emphasis.js) against the
// shared cases (crates/core/testdata/emphasis-cases.json) and a few rows of
// cells: wide characters, a marker split by the terminal's soft wrap.
//
//   node scripts/test-emphasis.mjs CONTRACT.json
//
// CONTRACT.json is `GET /api/emphasis`'s answer for the built-in vocabulary;
// `cargo test -p sbxw-core` writes one and runs this script.
import { createRequire } from "node:module";
import fs from "node:fs";
import path from "node:path";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const require = createRequire(import.meta.url);
const E = require(path.join(root, "assets/js/emphasis.js"));
const contractFile = process.argv[2];
if (!contractFile) {
  console.error("usage: node scripts/test-emphasis.mjs CONTRACT.json");
  process.exit(2);
}
const contract = JSON.parse(fs.readFileSync(contractFile, "utf8"));
const { cases } = JSON.parse(fs.readFileSync(path.join(root, "crates/core/testdata/emphasis-cases.json"), "utf8"));

let n = 0;
for (const c of cases) {
  assert.deepEqual(E.findCodePoints(c.text, contract), c.markers, JSON.stringify(c.text));
  n++;
}

// Rows of cells as xterm gives them: a wide character is one cell of width 2
// (its trailing cell is skipped).
const wide = (ch) => /[ᄀ-ᅟ⺀-꓏가-힣豈-﫿︰-﹏＀-｠￠-￦]|\p{Extended_Pictographic}/u.test(ch);
function row(y, text) {
  const cells = [];
  let x = 0;
  for (const ch of text) {
    const w = wide(ch) ? 2 : 1;
    cells.push({ x, w, ch });
    x += w;
  }
  return { y, cells };
}
const brief = (ps) => ps.map((p) => [p.y, p.x, p.width, p.tone, p.left, p.right, p.chars.map((c) => c.ch).join("")]);

// A wide character before the marker moves its cells, not its characters.
assert.deepEqual(brief(E.pieces([row(5, "日本 [[Done]]")], contract)), [[5, 5, 8, "green", true, true, "[[Done]]"]]);
// A wide character inside the detail widens the pill by its cells.
assert.deepEqual(brief(E.pieces([row(0, "[[Failed: 日]]")], contract)), [[0, 0, 14, "red", true, true, "[[Failed: 日]]"]]);
// Split by the soft wrap: one piece per row, square where it was cut.
assert.deepEqual(brief(E.pieces([row(3, "a long line [[Nee"), row(4, "ds you]] tail")], contract)), [
  [3, 12, 5, "amber", true, false, "[[Nee"],
  [4, 0, 8, "amber", false, true, "ds you]]"],
]);
// Two markers on one row, an unknown upper-case word, a wiki link left alone.
assert.deepEqual(brief(E.pieces([row(0, "[[Done]] [[Shipped]] [[base-kit]]")], contract)), [
  [0, 0, 8, "green", true, true, "[[Done]]"],
  [0, 9, 11, "blue", true, true, "[[Shipped]]"],
]);
// Unknown tones never leak: a configured vocabulary drives the colours.
const custom = { words: { shipped: "purple" }, unknown_tone: "blue" };
assert.deepEqual(E.findCodePoints("[[Shipped]] [[done]] [[Done]]", custom).map((m) => [m.word, m.tone]), [["Shipped", "purple"], ["Done", "blue"]]);
// No marker, no work.
assert.deepEqual(E.pieces([row(0, "plain text, nothing [here]")], contract), []);

// ── Pills while scrolling (attachWith, against a fake xterm) ────────────
// A buffer of `lines` (absolute rows), a viewport of `rows`; markers record
// the line they were put on; timers only run when flushed.
function fakeTerm(lines, rows = 24, cols = 80) {
  const on = { render: [], resize: [], buffer: [], sel: [] };
  const sub = (list) => (f) => { list.push(f); return { dispose() {} }; };
  const markers = [];
  const term = {
    rows, cols,
    options: { fontSize: 13, fontFamily: "mono", theme: {} },
    buffer: {
      active: {
        viewportY: 0, cursorY: 0,
        get baseY() { return Math.max(0, lines.length - rows); },
        get length() { return lines.length; },
        getNullCell: () => ({}),
        getLine(y) {
          const text = lines[y];
          if (text === undefined) return undefined;
          return {
            isWrapped: false,
            translateToString: () => text,
            getCell: (x) => (x < [...text].length ? { getWidth: () => 1, getChars: () => [...text][x] } : undefined),
          };
        },
      },
      onBufferChange: sub(on.buffer),
    },
    registerMarker(offset) {
      const b = term.buffer.active;
      const m = { line: b.baseY + b.cursorY + offset, isDisposed: false, subs: [], onDispose(f) { m.subs.push(f); }, dispose() { if (!m.isDisposed) { m.isDisposed = true; m.line = -1; m.subs.forEach((f) => f()); } } };
      markers.push(m);
      return m;
    },
    registerDecoration({ marker }) { return { marker, onRender() {}, dispose() {} }; },
    onRender: sub(on.render),
    onResize: sub(on.resize),
    onSelectionChange: sub(on.sel),
    hasSelection: () => false,
  };
  const timers = [];
  const env = {
    enabled: () => true,
    contract: () => contract,
    setTimeout: (f) => { timers.push(f); return timers.length; },
    clearTimeout: (t) => { timers[t - 1] = null; },
    paint() {},
  };
  const render = (start = 0, end = rows - 1) => on.render.forEach((f) => f({ start, end }));
  const flush = () => { const fs = timers.splice(0); fs.forEach((f) => f && f()); };
  const pending = () => timers.some(Boolean);
  const live = () => markers.filter((m) => !m.isDisposed).map((m) => m.line).sort((a, b) => a - b);
  return { term, env, render, flush, pending, markers, live };
}

// 5,000 lines, a pill every 50th; the cursor at the bottom.
const big = Array.from({ length: 5000 }, (_, i) => (i % 50 === 7 ? `line ${i} [[Done]] ok` : `line ${i} plain`));
{
  const f = fakeTerm(big);
  f.term.buffer.active.cursorY = 23;
  f.term.buffer.active.viewportY = 4976;
  const h = E.attachWith(f.term, f.env);
  f.render();
  assert.deepEqual(f.live(), [4957].filter((y) => y >= 4976), "the first render is read at once (no pill in the last 24 rows here)");
  // Scroll up a page: the rows brought into view get their pills in that
  // render, before any timer runs.
  f.term.buffer.active.viewportY = 4940;
  const readBefore = h.stats.rowsRead;
  f.render();
  assert.deepEqual(f.live(), [4957], "the pill on line 4957 is there in the frame that shows it");
  assert.equal(h.stats.rowsRead - readBefore, 24, "one viewport read, not the scrollback");
  // A fast scroll, several pages at once: same.
  f.term.buffer.active.viewportY = 4800;
  f.render();
  assert.deepEqual(f.live(), [4807, 4957], "a jump of several pages: the rows it lands on");
  assert.equal(h.stats.debounced, 0, "a scroll never waits for the timer");
  // Back down through lines read before: their pills are still on their
  // markers, nothing new is made.
  const made = h.stats.markers;
  f.term.buffer.active.viewportY = 4940;
  f.render();
  assert.equal(h.stats.markers, made, "scrolling back reuses the pills");
  assert.deepEqual(f.live(), [4807, 4957], "kept in the scrollback");
  // Reading the whole 5,000 lines by scrolling costs one viewport per render.
  let renders = 0;
  const read0 = h.stats.rowsRead;
  for (let y = 0; y <= 4976; y += 24) { f.term.buffer.active.viewportY = y; f.render(); renders++; }
  assert.ok(h.stats.rowsRead - read0 <= renders * 24, "never more than a viewport per frame");
  assert.equal(f.live().length, 100, "every pill seen is kept (under the cap)");
  assert.ok(!f.pending(), "no timer was needed");

  // Output in place: a few rows repainted are read at once (a pill whose
  // text went goes in that frame); a burst repainting the screen waits for
  // the burst to settle.
  f.term.buffer.active.viewportY = 4940;
  f.render();
  big[4957] = "line 4957 repainted, no pill";
  f.render(17, 17);
  assert.deepEqual(f.live().filter((y) => y >= 4940), [], "gone in the same frame");
  big[4958] = "now [[Failed: x]] here";
  f.render(0, 23);
  assert.ok(f.pending(), "a full in-place repaint is debounced");
  assert.ok(!f.live().includes(4958));
  f.flush();
  assert.ok(f.live().includes(4958), "and read once it settles");

  // A line trimmed off the scrollback takes its marker, and the pill.
  const first = f.markers.find((m) => !m.isDisposed);
  const before = h.count;
  first.dispose();
  assert.equal(h.count, before - 1);
  h.dispose();
}

// The cap keeps the pills nearest the viewport.
{
  const many = Array.from({ length: 3000 }, (_, i) => `[[Done]] ${i}`);
  const f = fakeTerm(many);
  f.term.buffer.active.cursorY = 23;
  const h = E.attachWith(f.term, f.env);
  for (let y = 0; y <= 2976; y += 24) { f.term.buffer.active.viewportY = y; f.render(); }
  assert.ok(h.count <= E.MAX_DECOS, `${h.count} pills, capped at ${E.MAX_DECOS}`);
  const kept = f.live();
  assert.ok(kept.includes(2999) && !kept.includes(0), "the oldest, farthest from the viewport, went first");
  h.dispose();
}

// ── Pill colours: the desktop's rule lands on its values ────────────────
// The web theme's tones (assets/app.css) on a dark terminal, the light tones
// on a light one, against the desktop's final values (each channel within 3).
{
  const near = (a, b) => {
    const p = (h) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16));
    return p(a).every((v, i) => Math.abs(v - p(b)[i]) <= 3);
  };
  const web = { green: "#3fb950", red: "#f85149", amber: "#d29922", blue: "#58a6ff", purple: "#d2a8ff", cyan: "#39c5cf", danger: "#b62324" };
  const dark = { green: ["#12221d", "#8bc69b"], red: ["#25171c", "#de9798"], amber: ["#211f18", "#cdb887"], blue: ["#15202e", "#96beea"], purple: ["#21202e", "#cdbfea"], cyan: ["#112329", "#88ccd5"], danger: ["#b62324", "#ffffff"] };
  const light = { green: ["#e8f2eb", "#205033"], red: ["#fae9ea", "#71262f"], amber: ["#f5f0e6", "#59451a"], blue: ["#e6f0fb", "#18467c"], purple: ["#f3eefc", "#4e3b7e"], cyan: ["#e8f2f3", "#204e55"], danger: ["#cf222e", "#ffffff"] };
  for (const [tone, [fill, word]] of Object.entries(dark)) {
    const c = E.pillColors(web[tone], "#0d1117", "#c9d1d9", tone === "danger");
    assert.ok(near(c.fill, fill) && near(c.word, word), `dark ${tone}: ${JSON.stringify(c)} vs ${fill} ${word}`);
    if (tone !== "danger") assert.equal(c.detail, "#c9d1d9");
    assert.ok(E.contrast(E.parseColor(c.word), E.parseColor(c.fill)) >= 4.5);
  }
  for (const [tone, [fill, word]] of Object.entries(light)) {
    const c = E.pillColors(E.LIGHT_TONES[tone], "#ffffff", "#24292f", tone === "danger");
    assert.ok(near(c.fill, fill) && near(c.word, word), `light ${tone}: ${JSON.stringify(c)} vs ${fill} ${word}`);
    if (tone !== "danger") assert.equal(c.detail, "#24292f");
  }
  // Over a diff line's green band, the fill is the tone over the band.
  const band = E.pillColors(web.blue, "#12261e", "#c9d1d9");
  assert.notEqual(band.fill, E.pillColors(web.blue, "#0d1117", "#c9d1d9").fill);
  // The detail starts at the colon, and leaves the closing brackets out.
  const [piece] = E.pieces([row(0, "[[Failed: 3]]")], contract);
  assert.equal(piece.chars.map((c) => (c.detail ? "d" : ".")).join(""), "........ddd..", "the colon and the detail");
}

console.log(`emphasis matcher: ${n} shared cases and 6 cell checks passed; scrolling and colour checks passed`);
