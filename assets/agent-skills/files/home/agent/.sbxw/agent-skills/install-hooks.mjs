#!/usr/bin/env node
// Register hook-adhd.mjs in Claude Code's user-level settings, the file it
// reads for every session in this sandbox whatever project is open.
// Merges rather than overwrites and replaces its own previous entry where it
// stands (moving it to the end would reorder other kits' hooks on every
// start), so re-running on every container start is a no-op; a corrupt settings file is
// left exactly as found. Same arrangement as the codemap kit's hooks.
import { existsSync, readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname } from "node:path";

const SETTINGS = "/home/agent/.claude/settings.json";
const HOOK = "/home/agent/.sbxw/agent-skills/hook-adhd.mjs";
const FLAG = "/home/agent/.claude/.i-have-adhd-always";
// Seeded once: the flag is created on the first start only, so a user who
// deleted it to turn always-on off is not overruled by the next start.
const SEEDED = "/home/agent/.sbxw/agent-skills/.adhd-flag-seeded";

let raw = "";
let settings = {};
try {
  raw = readFileSync(SETTINGS, "utf8");
  settings = JSON.parse(raw);
} catch (e) {
  if (e.code !== "ENOENT") {
    console.log("  [agent-skills] " + SETTINGS + " is not readable JSON — i-have-adhd hook not installed, settings left alone");
    process.exit(0);
  }
}

const ours = (entry) =>
  (entry.hooks || []).some((h) => (h.args || []).includes(HOOK) || String(h.command || "").includes(HOOK));

settings.hooks = settings.hooks || {};
const list = settings.hooks.SessionStart || [];
const at = list.findIndex(ours);
settings.hooks.SessionStart = list.filter((e) => !ours(e));
settings.hooks.SessionStart.splice(at < 0 ? settings.hooks.SessionStart.length : at, 0, {
  matcher: "startup|resume|clear|compact",
  hooks: [{ type: "command", command: "node", args: [HOOK], timeout: 30 }],
});

// The shared browser's guard (hook-browser.mjs), on Bash, the same way.
const BROWSER_HOOK = "/home/agent/.sbxw/agent-skills/hook-browser.mjs";
const oursBrowser = (entry) =>
  (entry.hooks || []).some((h) => (h.args || []).includes(BROWSER_HOOK) || String(h.command || "").includes(BROWSER_HOOK));
const pre = settings.hooks.PreToolUse || [];
const preAt = pre.findIndex(oursBrowser);
settings.hooks.PreToolUse = pre.filter((e) => !oursBrowser(e));
settings.hooks.PreToolUse.splice(preAt < 0 ? settings.hooks.PreToolUse.length : preAt, 0, {
  matcher: "Bash",
  hooks: [{ type: "command", command: "node", args: [BROWSER_HOOK], timeout: 10 }],
});

// Written only when it changes, so a restart leaves the file untouched.
const next = JSON.stringify(settings, null, 2) + "\n";
if (next !== raw) {
  mkdirSync(dirname(SETTINGS), { recursive: true });
  writeFileSync(SETTINGS, next);
}

if (!existsSync(SEEDED)) {
  writeFileSync(FLAG, "");
  writeFileSync(SEEDED, "");
}
console.log("  [agent-skills] shared-browser guard installed; i-have-adhd hook installed (always-on " + (existsSync(FLAG) ? "on" : "off: " + FLAG + " was deleted") + ")");
