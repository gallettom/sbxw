#!/usr/bin/env node
// SessionStart: inject the i-have-adhd ruleset when the flag file exists.
// Never blocks a session start: any failure exits 0 and says nothing.
import { existsSync, readFileSync } from "node:fs";

const FLAG = "/home/agent/.claude/.i-have-adhd-always";
const SKILL = "/home/agent/.sbxw/agent-skills/i-have-adhd/SKILL.md";

try {
  if (!existsSync(FLAG) || !existsSync(SKILL)) process.exit(0);
  const body = readFileSync(SKILL, "utf8")
    .replace(/^---[^\S\r\n]*\r?\n[\s\S]*?\r?\n---[^\S\r\n]*(?:\r?\n|$)/, "")
    .replace(/(?:\r?\n)+$/, "");
  process.stdout.write(
    "ADHD MODE ACTIVE (always-on). The ruleset below applies to every response. " +
      '"stop adhd mode" turns it off for this session; delete ' + FLAG +
      " to turn always-on off for good.\n\n" + body + "\n",
  );
} catch {
  process.exit(0);
}
