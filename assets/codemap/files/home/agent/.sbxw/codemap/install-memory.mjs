#!/usr/bin/env node
// Splice the codemap section into the agent's user-level memory
// (/home/agent/.claude/CLAUDE.md), the file Claude Code reads at the start of
// every session in this sandbox whatever project is open.
//
// Rewrites only what sits between its own fences: that file also belongs to the
// user and to the agent (`#` memories land there), and sbxw's relay keeps its
// own fenced section in it. Idempotent — safe on every container start.

import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname } from "node:path";

const MEMORY = "/home/agent/.claude/CLAUDE.md";
const BLOCK = "/home/agent/.sbxw/codemap/memory-block.md";
const BEGIN = "<!-- sbxw:codemap:begin -->";
const END = "<!-- sbxw:codemap:end -->";

const doc = readFileSync(BLOCK, "utf8").trim();

let current = "";
try { current = readFileSync(MEMORY, "utf8"); } catch (e) { /* first run */ }

const from = current.indexOf(BEGIN);
const to = current.indexOf(END);
let rest = from >= 0 && to > from ? current.slice(0, from) + current.slice(to + END.length) : current;
rest = rest.replace(/\n{3,}/g, "\n\n").trim();

// Written only when it changes, so a restart leaves the file untouched.
const next = (rest ? rest + "\n\n" : "") + BEGIN + "\n" + doc + "\n" + END + "\n";
if (next !== current) {
  mkdirSync(dirname(MEMORY), { recursive: true });
  writeFileSync(MEMORY, next);
}

console.log("  [codemap] /codemap + /codemap-lens installed; user memory points at .sbxw-artifacts/codemap/");
