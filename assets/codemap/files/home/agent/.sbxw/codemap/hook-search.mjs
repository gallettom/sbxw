#!/usr/bin/env node
// PreToolUse on Grep/Glob/Bash — the one moment when "check the map first" is
// actionable, because the agent is about to do the thing the map replaces.
//
// It runs on Grep and Glob, and on Bash when the command is a search
// (grep/rg/ag/ack/find/fd) — in a bypass-permissions sandbox the agent greps
// and finds through the shell far more than through the two tools, so a hook
// that only watched the tools missed most sessions. Same command shapes the
// Stop hook counts. What it does with the answer to "is this repo mapped?":
//
//  - Mapped: it points at the map. The SessionStart hook already put the index
//    in context, so this is a second line — it earns its place in a long
//    session where that injection is many turns back or was dropped by a
//    compaction, and in the subagent spawned to search that starts by searching.
//  - Unmapped: at the first search of the session it asks the agent to offer
//    /codemap to the user — the point where the session has shown it is going
//    to need one. Whatever the user then answers, nothing here fires again: the
//    offer is stamped against the repo, which also keeps the Stop hook from
//    making the same offer a second time in the same session.
//
// Deliberately toothless throughout. It returns context and never a permission
// decision, so the search always runs, and it acts at most once per session.
//
// Written without template literals: see hook-lib.mjs.

import { MAP_REL, findMap, isBackgroundRun, nudgingOff, readInput, repoRoot, sections, since, stamp } from "./hook-lib.mjs";

// Shared with the Stop hook's "offered" bucket: long enough that a refusal is
// respected for the rest of the working day, short enough that tomorrow's
// session may ask again.
const OFFER_COOLDOWN = 12 * 60 * 60 * 1000;

// A Bash command that is a tree search — the same set the Stop hook scores.
const SHELL_SEARCH = /\b(grep|rg|ag|ack|find|fd)\b/;

const ok = (extra) => {
  if (extra) {
    process.stdout.write(JSON.stringify({
      hookSpecificOutput: { hookEventName: "PreToolUse", additionalContext: extra },
    }));
  }
  process.exit(0);
};

const input = await readInput();
if (isBackgroundRun() || nudgingOff()) ok(null);

// On Bash, only a search command counts. A nudge on every `ls` and `cat` is one
// the agent learns to skip, and this hook has one shot per session to spend.
if (String(input.tool_name || "") === "Bash") {
  const cmd = input.tool_input && typeof input.tool_input.command === "string" ? input.tool_input.command : "";
  if (!SHELL_SEARCH.test(cmd)) ok(null);
}

const session = String(input.session_id || "");
if (!session || since("searched", session) !== Infinity) ok(null);
stamp("searched", session, Date.now());

const root = repoRoot(input.cwd);
const map = findMap(root);

// Unmapped: ask for an offer, once per repository per cooldown. Never a block —
// whatever the user decides, this is the last the session hears of it.
if (!map) {
  if (since("offered", root) < OFFER_COOLDOWN) ok(null);
  stamp("offered", root, Date.now());
  ok([
    "This repository has no code map (" + MAP_REL + "/) and you are about to search it. A map answers where an idea lives and why it is built that way in one read; a search recovers the where slowly and the why not at all.",
    "",
    "Do not write it now, and do not block on this. Before you finish, add a sentence to what you were going to say: name what you would map, and offer to run /codemap — or /codemap <area> for a single subsystem, usually the better offer after a focused session. Then let the user answer: writing it is their call and \"no\" ends the matter.",
    "",
    "(codemap kit, search hook — once per repository per 12h. SBXW_CODEMAP_NUDGE=0 turns it off.)",
  ].join("\n"));
}

// A real lat.md vault: defer to it.
if (map.kind === "lat") ok("This repository keeps a lat.md vault at lat.md/. If this search is for where something lives or why it works this way, read the vault first — it answers that in one read.");

// Mapped by the kit: point at the map.
const named = sections(root);
ok([
  "Before this search: this repository is mapped. " + MAP_REL + "/codemap.md is the index"
    + (named.length ? ", beside " + named.join(", ") : "") + ".",
  "If what you are after is where an idea lives, or why something is built this way, the map answers it in one read and a search does not. Read the relevant section, then search the tree for the detail it points you at — and believe the tree wherever the two disagree.",
].join("\n"));
