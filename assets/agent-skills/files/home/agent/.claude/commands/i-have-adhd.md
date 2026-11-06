---
description: 'Shape output for a reader with ADHD: lead with the next action, number multi-step work, restate state across turns, suppress tangents, give specific time estimates, make wins visible. Stays on until "stop adhd mode".'
disable-model-invocation: true
---

/i-have-adhd is installed in this sandbox by sbxw's agent-skills kit, at
`/home/agent/.sbxw/agent-skills/i-have-adhd` (the commit is in `.sbxw-rev` there).
**Read `/home/agent/.sbxw/agent-skills/i-have-adhd/SKILL.md` now and follow it**
for the rest of this session, until the user says "stop adhd mode" or "normal mode".

If the session already opened with "ADHD MODE ACTIVE (always-on)", the rules are
in force: confirm in one line and carry on. Always-on is the kit's `SessionStart`
hook, which fires while `/home/agent/.claude/.i-have-adhd-always` exists; deleting
that file turns it off for good in this sandbox.

$ARGUMENTS
