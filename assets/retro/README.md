# retro kit

Ships the `/retro` command — written to `/home/agent/.claude/commands/retro.md`
(user-level, so it is there whatever project is mounted) — and the digest
script it reads sessions through, `/home/agent/.sbxw/retro/digest.mjs`.

## What it is for

An agent re-derives the same things every session: the same commands to cut a
release, the same correction from you, the same files read before it finds its
footing. `/retro` reads the transcripts back and sorts what it finds onto a
ladder, from most deterministic to least:

| Form | For |
|---|---|
| script | a fixed command sequence with no decision in it |
| hook | something that must happen on every event, remembered or not |
| permission rule | a safe, frequent command that prompts |
| skill | a repeatable procedure that still needs judgment |
| rule (`CLAUDE.md`, memory) | a preference or fact the agent kept missing |
| kit | something the environment lacked: a tool, a domain, a skill for every sandbox |

Each candidate comes with its evidence (sessions, step numbers, counts), a
concrete proposal, the gain and the risk. The report goes to
`.sbxw-artifacts/retro/<date>-<slug>.md`. Nothing is built, and no hook or
setting is installed, until you pick a candidate.

## Usage

```
/retro              this session
/retro last 5       the five newest sessions of this project — the best input
/retro 0cee 59d9    chosen sessions, by id prefix
```

The digest runs on its own too:

```bash
node ~/.sbxw/retro/digest.mjs --list
node ~/.sbxw/retro/digest.mjs --last 5
node ~/.sbxw/retro/digest.mjs --project /path/to/other/workspace
```

It reads `~/.claude/projects/<slug>/*.jsonl` and prints the prompts you typed,
tool counts, repeated commands, files re-read or re-edited, errors and
refusals, hooks that fired, and the tool-call timeline — with counts across
sessions when given several. Token-shaped strings are masked.

## Network

None. The kit writes two files; the transcripts are already in the sandbox.
