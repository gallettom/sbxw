#!/usr/bin/env node
// SessionStart — hand the agent the map's index instead of hoping it looks.
//
// The memory block asks every session to check `.sbxw-artifacts/codemap/` before
// reading source. That is prose competing with an eager first grep, and prose
// loses: the moment to check does not announce itself. So this hook does the
// check — one `existsSync` — and, when the answer is yes, puts the index in the
// session's context before the first turn. The agent no longer has to remember
// to look; it has already read it.
//
// Silent when there is no map. Offering to write one is the Stop hook's job,
// which fires when the session has proved it needed one.
//
// Written without template literals: see hook-lib.mjs.

import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { CHECKER, LENS_REL, MAP_REL, clip, findMap, lenses, readInput, repoRoot, sections } from "./hook-lib.mjs";

// Enough for an index of a large map, small enough that it never dominates the
// session it is meant to shorten.
const MAX_INDEX = 6000;

const input = await readInput();
const root = repoRoot(input.cwd);
const map = findMap(root);
if (!map) process.exit(0);

const lines = [];
if (map.kind === "lat") {
  lines.push(
    "This repository keeps a lat.md vault at lat.md/ — the same knowledge-graph format as a code map, run by the real tool.",
    "Read it before searching the tree for what a project does or why, and validate any change you make to it with `lat check`.",
  );
} else {
  lines.push(
    "This repository has a code map at " + MAP_REL + "/ — the written record of what each part does and *why*. The why is the half no amount of grepping recovers, so:",
    "",
    "- **Read the map before you search the tree.** For where an idea lives, or why something is built this way, one section beats a dozen greps. Follow its links: [[name]] is " + MAP_REL + "/name.md, and `@lat:` comments in source point back at the section that explains them.",
    "- **The tree wins where they disagree.** The map records what was true when it was written. Believe the code, and fix the section as you pass — a known-wrong map is worse than no map.",
    "- **A change to behaviour, architecture or tests is a change to the map**, in the same session, checked with: node " + CHECKER,
    "",
  );
  const named = sections(root);
  if (named.length) lines.push("Sections beside the index: " + named.join(", "), "");
  if (existsSync(join(root, MAP_REL, "diagram", "architecture.json"))) {
    lines.push(
      "The map's picture is an Archify architecture diagram: " + MAP_REL + "/diagram/architecture.json (source) rendered to architecture.html. Each component is labelled with the heading of the section that explains it. When a change moves a component or a relationship, edit the JSON and render it again — /home/agent/.sbxw/codemap/FORMAT.md, The diagram.",
      "",
    );
  }
  lines.push("The index, verbatim:", "");
  try {
    lines.push(clip(readFileSync(map.index, "utf8").trim(), MAX_INDEX));
  } catch (e) {
    lines.push("(" + MAP_REL + "/codemap.md could not be read — open it yourself.)");
  }
}

const names = lenses(root);
if (names.length) {
  lines.push(
    "",
    "Lenses in " + LENS_REL + "/: " + names.join(", ") + ". Each is the map retold for one reader — derived, partial, possibly stale. Read one to learn who was asking for what, never to learn what the code does.",
  );
}

process.stdout.write(JSON.stringify({
  hookSpecificOutput: { hookEventName: "SessionStart", additionalContext: lines.join("\n") },
}));
