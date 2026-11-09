---
description: A relentless interview to sharpen a plan or design.
disable-model-invocation: true
---

Subject: $ARGUMENTS

/grill-me is installed in this sandbox by sbxw's agent-skills kit. Its upstream
SKILL.md only hands over to the `grilling` skill, which is installed at
`/home/agent/.sbxw/agent-skills/grilling` (the commit is in `.sbxw-rev` there).
**Read `/home/agent/.sbxw/agent-skills/grilling/SKILL.md` now and follow it**
on the subject above, or on the plan this conversation is about if it is empty.

**In sbxw, put each round to the person with the `ask_user_questions` tool**
(MCP server `sbxw-relay`) instead of writing it out and ending your turn: one
call per round, each question with a short title, the question and your
recommendation. The person answers in sbxw's Concierge, often by voice, one
question at a time, and every answer comes back as the tool's result — then
compute the next round from them. If the tool is not available, write the
round out as the skill says.
