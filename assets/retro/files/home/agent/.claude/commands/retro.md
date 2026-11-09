Look back over Claude Code sessions and find what should stop depending on the agent remembering: what can become a script, a hook, a skill, a permission, a rule or a kit. Report it; change nothing unasked.

Arguments: $ARGUMENTS
- empty → this session (the newest transcript of the current project)
- a session id or prefix, or several → those sessions
- `last N` → the N newest sessions; the best input, since a habit seen once is an anecdote
- `--project <dir>` may follow any of these, for another project's sessions

## Why this exists

An agent re-derives the same things every session: the same five commands to cut a release, the same correction from the person ("commit and tag", "don't close on Escape"), the same file read three times because nothing pointed at it. Each is cheap once and expensive forever. The cure is to move it down the ladder below, toward something the harness or a script does every time without being asked — and to leave alone what genuinely needs judgment.

## 1. Read the history

```bash
node /home/agent/.sbxw/retro/digest.mjs --list            # what there is
node /home/agent/.sbxw/retro/digest.mjs                   # this session
node /home/agent/.sbxw/retro/digest.mjs --last 5          # adds counts across sessions
node /home/agent/.sbxw/retro/digest.mjs 0cee 59d9         # chosen sessions
```

The digest gives the person's prompts (with the tool step they came at), tool counts, repeated commands, re-read and re-edited files, errors and refusals, the hooks that fired, and the timeline of tool calls. It masks token-shaped strings; still never copy a secret from a transcript into anything you write.

When it is not enough, open the raw transcript selectively — `~/.claude/projects/<slug>/<id>.jsonl`, one JSON object per line — with `grep -n` or `node -e` on the entries you need. Never read it whole: request blobs make it megabytes.

On this session, ignore everything from the `/retro` prompt onward: that is this skill working, not a habit.

## 2. Know what is already there

Before proposing anything, look at what already exists, so you do not propose a hook that is installed or a skill that ships:

- `~/.claude/settings.json` and the project's `.claude/settings*.json` — hooks and permissions;
- `~/.claude/commands/`, `~/.claude/skills/`, `.claude/commands/`, `.claude/skills/` — commands and skills;
- `CLAUDE.md` files (global and project) and the memory directory;
- the repo's `scripts/`, `Makefile`, `package.json` scripts, `justfile`;
- `sbxw.toml` (kits, `network_allow`) when the project has one.

## 3. Find the candidates

Read for these signals, most valuable first:

1. **The person corrected the agent.** A prompt that undoes, restates or says "no, …", "again", "I told you". The same correction in two sessions is the strongest signal there is.
2. **The same sequence of tool calls**, in one session or across several: release steps, build-then-test, regenerate-then-check. Note where the sequence varies — that is where judgment lives, if any.
3. **The same command failing the same way**, and the same recovery after it: a missing tool, a blocked domain, a wrong flag.
4. **Work done by hand that a check could have caught**: a lint or format run forgotten until the person noticed, a version bumped in one file of three.
5. **Exploration that ended in the same place**: the same files read at the start of every session on a topic — a pointer, a map section or a CLAUDE.md line would replace it.
6. **Permission prompts and refusals** on commands that are safe and frequent.

Count, do not guess: every candidate cites its evidence — session id prefix, step numbers (`#12`, `[@15]`), how many times.

## 4. Place each one on the ladder

Pick the most deterministic form that still fits. A step that needs judgment is not made deterministic by writing it as a script; a step that needs none should not cost a model call.

| Form | When | Lives in |
|---|---|---|
| **Script** | A fixed sequence of commands with no decision in it (release, regenerate, bump versions) | the repo: `scripts/`, a `Makefile` target, a `package.json` script |
| **Hook** | Something that must happen *every time* an event occurs, whatever the agent remembers: format after `Edit`, refuse a dangerous command (`PreToolUse` exit 2), inject context at `SessionStart`, a check at `Stop` | `.claude/settings.json` (project) or `~/.claude/settings.json` |
| **Permission rule** | A safe, frequent command that prompts | `permissions.allow` in `.claude/settings.json` |
| **Skill / command** | A repeatable procedure that still needs judgment (which tests, which wording, which files) | `.claude/commands/<name>.md` or `.claude/skills/<name>/SKILL.md`; for every sandbox, a kit |
| **Rule** | A preference or a fact the agent kept missing ("tags are annotated", "answer in French") | project `CLAUDE.md`, or memory |
| **Kit** | The environment itself was missing something: a tool installed by hand, a domain allowed by hand, the same skill wanted in every sandbox | an sbx kit (`spec.yaml`), listed in `sbxw.toml` `kits` |
| **Nothing** | It happened once, or the cost of the machinery is above the cost of the habit | — |

A candidate can split: the deterministic core becomes a script, and a thin skill or hook calls it.

## 5. Write the report

Write `.sbxw-artifacts/retro/<YYYY-MM-DD>-<slug>.md` when `.sbxw-artifacts/` exists, else print it. The slug names the sessions' subject in two or three words.

```markdown
# Retro — <subject> (<date>)

Sessions: <id prefixes>, <n> prompts, <n> tool calls.

## Candidates

### 1. <name> → <form>
- **Evidence:** <session> #12–#18, and <session> #4–#9: <what repeated>, <n> times.
- **Proposal:** <the script / hook / skill / rule, concretely — a settings.json fragment, a script outline, a SKILL.md description line>.
- **Gain:** <what stops happening: calls, minutes, corrections>.
- **Risk:** <what could go wrong if it is automated; why it is still worth it, or the guard>.

### 2. …

## Left alone
- <pattern> — <why it stays a judgment call, or why once is not enough>.
```

Rank candidates by (times seen × cost each time) ÷ effort to build. Keep the list short: five strong candidates beat fifteen weak ones; say what you dropped under *Left alone*.

## 6. Hand it over

In the conversation: one line per candidate (name → form, the evidence count), then offer to build the first one. Build nothing, install no hook and edit no settings until the person picks — a hook changes every later session, and that is their call.

When they pick one, build it in the place the table names, test it (run the script; for a hook, trigger its event once and show it firing), and note in the report which candidates were done.
