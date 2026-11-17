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

console.log(`emphasis matcher: ${n} shared cases and 6 cell checks passed`);
