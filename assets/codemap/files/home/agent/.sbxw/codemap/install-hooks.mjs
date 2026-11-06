#!/usr/bin/env node
// Register the kit's three hooks in Claude Code's user-level settings
// (/home/agent/.claude/settings.json), the file it reads for every session in
// this sandbox whatever project is open.
//
// The commands and the memory block are documentation: they work when the agent
// reaches for them. The hooks are the part that does not depend on reaching —
// they put the map in front of a session that is about to search without it, and
// ask for an offer from a session about to end without one.
//
// Merges rather than overwrites, and replaces its own previous entries where
// they stand (other kits' hooks share these events, and moving to the end
// would reorder them on every start), so re-running on every container start
// is a no-op. That file also holds the
// model, the statusLine and sbxw's own status hooks; a corrupt one is left
// exactly as found rather than replaced with a fresh object, since these three
// hooks are not worth the rest of a user's configuration.

import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname } from "node:path";

const SETTINGS = "/home/agent/.claude/settings.json";
const KIT = "/home/agent/.sbxw/codemap/";

// Event, script, and the tools it applies to (null = every call of the event).
const HOOKS = [
  ["SessionStart", KIT + "hook-session-start.mjs", null],
  ["PreToolUse", KIT + "hook-search.mjs", "Bash|Grep|Glob"],
  ["Stop", KIT + "hook-stop.mjs", null],
];

let raw = "";
let settings = {};
try {
  raw = readFileSync(SETTINGS, "utf8");
  settings = JSON.parse(raw);
} catch (e) {
  if (e.code !== "ENOENT") {
    console.log("  [codemap] " + SETTINGS + " is not readable JSON — hooks not installed, settings left alone");
    process.exit(0);
  }
}

const ours = (entry, path) =>
  (entry.hooks || []).some((h) => (h.args || []).includes(path) || String(h.command || "").includes(path));

settings.hooks = settings.hooks || {};
for (const [event, path, matcher] of HOOKS) {
  const entry = { hooks: [{ type: "command", command: "node", args: [path] }] };
  if (matcher) entry.matcher = matcher;
  const list = settings.hooks[event] || [];
  const at = list.findIndex((e) => ours(e, path));
  settings.hooks[event] = list.filter((e) => !ours(e, path));
  settings.hooks[event].splice(at < 0 ? settings.hooks[event].length : at, 0, entry);
}

// Written only when it changes, so a restart leaves the file untouched.
const next = JSON.stringify(settings, null, 2) + "\n";
if (next !== raw) {
  mkdirSync(dirname(SETTINGS), { recursive: true });
  writeFileSync(SETTINGS, next);
}
console.log("  [codemap] hooks installed: the map is injected at session start, recalled on first search, and offered at the end of an unmapped session");
