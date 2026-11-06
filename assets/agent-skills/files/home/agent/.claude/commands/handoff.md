---
description: Compact the current conversation into a handoff document for another agent to pick up.
argument-hint: "What will the next session be used for?"
disable-model-invocation: true
---

Next session's focus: $ARGUMENTS

/handoff is installed in this sandbox by sbxw's agent-skills kit, at
`/home/agent/.sbxw/agent-skills/handoff` (the commit is in `.sbxw-rev` there).
**Read `/home/agent/.sbxw/agent-skills/handoff/SKILL.md` now and follow it.**
End by giving the user the absolute path of the document you wrote: this sandbox
has no display, and that path is how the next agent is pointed at it.
