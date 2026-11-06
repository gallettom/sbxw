#!/usr/bin/env node
// Stop — the moment the memory block could only describe.
//
// "No map, and you have just worked out how this repo fits together? Run
// /codemap before you finish" asks the agent to notice a moment that never
// announces itself, and to notice it at the moment it is trying to stop. It
// almost never fires. This hook fires it instead: at Stop, in an unmapped
// repository, after a session that demonstrably read the source, it blocks the
// turn once and asks the agent to *offer*.
//
// Three things keep it from becoming nagging:
//
//  - It asks for an offer, never for the map. The agent says one sentence and
//    stops; writing it is the user's call and "no" ends the matter.
//  - It fires once per repository per COOLDOWN, recorded before the block is
//    emitted — so a refusal is not re-litigated the next time the session ends.
//  - It needs evidence. A session that answered a question without reading the
//    tree has no understanding worth writing down.
//
// Written without template literals: see hook-lib.mjs.

import { readFileSync, statSync } from "node:fs";
import { MAP_REL, findMap, isBackgroundRun, nudgingOff, readInput, repoRoot, since, stamp } from "./hook-lib.mjs";

// Long enough that a refusal is respected for the rest of the working day,
// short enough that tomorrow's session may ask again — the answer depends on
// what that session did, not on what yesterday's decided.
const COOLDOWN = 12 * 60 * 60 * 1000;

// How much of the session counts as "worked out how this fits together":
// distinct source files opened, plus searches (capped, since ten greps for one
// symbol is one question). MIN_FILES is the floor under it — a session that only
// searched read nothing it could map.
const MIN_SCORE = 6;
const MIN_FILES = 3;
const MAX_SEARCH_CREDIT = 5;

// Only the tail of a long transcript is scanned: evidence accumulates, so the
// last few megabytes decide the same answer as the whole file, and a hook is not
// the place to read a hundred of them.
const MAX_TRANSCRIPT = 12 * 1024 * 1024;

const SOURCE = /\.(rs|ts|tsx|js|jsx|mjs|cjs|py|go|java|kt|rb|php|swift|cs|scala|c|h|cpp|hpp|sh|sql|proto|toml|yaml|yml)$/;
const IGNORE = /(^|\/)(node_modules|target|dist|build|vendor|__pycache__|\.git|\.sbxw-artifacts)(\/|$)/;
const READERS = new Set(["Read", "Edit", "MultiEdit", "Write", "NotebookEdit"]);
const SEARCHERS = new Set(["Grep", "Glob"]);
// Paths inside a shell command. In a sandbox running with permissions bypassed
// the agent reads with `cat` and `sed -n` far more often than with Read, so a
// scan that only counted tool calls would score every such session at zero.
const IN_SHELL = /[\w./~-]*[\w-]+\.[A-Za-z0-9]+/g;
const SHELL_SEARCH = /\b(grep|rg|ag|ack|find|fd)\b/;

const collect = (files, name, input) => {
  if (!input || typeof input !== "object") return 0;
  if (READERS.has(name)) {
    for (const key of ["file_path", "notebook_path"]) {
      const p = input[key];
      if (typeof p === "string" && SOURCE.test(p) && !IGNORE.test(p)) files.add(p);
    }
    return 0;
  }
  if (SEARCHERS.has(name)) return 1;
  if (name === "Bash" && typeof input.command === "string") {
    for (const m of input.command.match(IN_SHELL) || []) {
      if (SOURCE.test(m) && !IGNORE.test(m)) files.add(m);
    }
    return SHELL_SEARCH.test(input.command) ? 1 : 0;
  }
  return 0;
};

/** What this session read, as a score against MIN_SCORE. */
function explored(path) {
  const files = new Set();
  let searches = 0;
  let text = "";
  try {
    const size = statSync(path).size;
    const fd = readFileSync(path);
    text = (size > MAX_TRANSCRIPT ? fd.subarray(size - MAX_TRANSCRIPT) : fd).toString("utf8");
  } catch (e) {
    return { files: files, score: 0 };
  }
  for (const line of text.split("\n")) {
    if (!line.includes("tool_use")) continue;
    let entry;
    try { entry = JSON.parse(line); } catch (e) { continue; } // a clipped first line
    const content = entry && entry.message && entry.message.content;
    if (!Array.isArray(content)) continue;
    for (const block of content) {
      if (block && block.type === "tool_use") searches += collect(files, block.name, block.input);
    }
  }
  return { files: files, score: files.size + Math.min(searches, MAX_SEARCH_CREDIT) };
}

const input = await readInput();
// stop_hook_active means this turn is already running because a Stop hook
// blocked it. Blocking again is how a session gets stuck in one.
if (input.stop_hook_active || isBackgroundRun() || nudgingOff()) process.exit(0);

const root = repoRoot(input.cwd);
if (findMap(root)) process.exit(0);
if (since("offered", root) < COOLDOWN) process.exit(0);

const read = explored(String(input.transcript_path || ""));
if (read.files.size < MIN_FILES || read.score < MIN_SCORE) process.exit(0);

// Recorded before the block, not after: if the agent's offer never reaches the
// user, or the user says no, the next Stop must pass in silence.
stamp("offered", root, Date.now());

process.stdout.write(JSON.stringify({
  decision: "block",
  reason: [
    "This repository has no code map (" + MAP_REL + "/) and this session read " + read.files.size
      + " of its source files. What you worked out about how it fits together is the expensive part of this session, and it dies with the session.",
    "",
    "Do not write the map now. Before you finish, add one or two sentences to what you were already going to say: name what you would map, and offer to run /codemap. Then stop, and let the user answer — writing it is their call, and \"no\" is a complete answer you must not revisit.",
    "",
    "Worth knowing when you word the offer: `/codemap <area>` maps one subsystem rather than the whole repository, which is usually the better offer after a focused session. sbxw's Project panel runs the same command in a background session, so accepting need not cost the user this pane.",
    "",
    "(codemap kit, Stop hook — once per repository per 12h. SBXW_CODEMAP_NUDGE=0 turns it off.)",
  ].join("\n"),
}));
