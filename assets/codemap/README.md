# codemap kit

Gives every sandbox agent two things it doesn't have by default: the reflex to
**look for a repository's code map before reading its source**, and the means to
**write one when there is none** — both wired to Claude Code hooks, so neither
depends on the agent remembering at the right moment.

A code map is a small set of cross-linked markdown files — architecture, design
decisions, domain vocabulary, test specs — describing what a project does and
*why*. It lives in `.sbxw-artifacts/codemap/`, so it lands in the folder sbxw's
web UI already lists and serves, and it is written in the
[lat.md](https://github.com/vercel-labs/lat.md) format: `[[wiki links]]` between
sections, links straight into source symbols (`[[src/auth.ts#validateToken]]`),
and `// @lat: [[section]]` comments tying code back to the idea it implements.

The format matters more than the tooling here. A repo that later runs
`npm i -g lat.md` gets `lat check`, `lat search` and `lat section` over exactly
what this kit's agents have already written — nothing to convert.

## What it installs

| Path in the sandbox | What it is |
| --- | --- |
| `~/.claude/commands/codemap.md` | the `/codemap` command — reconnaissance, shape, writing rules, validation, reporting |
| `~/.claude/commands/codemap-lens.md` | the `/codemap-lens` command — the map retold for one reader (see [Lenses](#lenses)) |
| `~/.sbxw/codemap/FORMAT.md` | the format reference the command and the memory block point at |
| `~/.sbxw/codemap/check-codemap.mjs` | offline checker (no dependencies, no network) |
| `~/.sbxw/codemap/memory-block.md` | the paragraph spliced into user memory |
| `~/.sbxw/codemap/install-memory.mjs` | the splice, run at every container start |
| `~/.sbxw/codemap/hook-lib.mjs` | what the three hooks share: is this repo mapped, and has this fired already |
| `~/.sbxw/codemap/hook-session-start.mjs` | `SessionStart` — puts the map's index in the session's context |
| `~/.sbxw/codemap/hook-search.mjs` | `PreToolUse` on Grep/Glob + Bash searches — recalls the map before the session's first search, or asks an unmapped session to *offer* one |
| `~/.sbxw/codemap/hook-stop.mjs` | `Stop` — asks an unmapped session to *offer* a map before it ends |
| `~/.sbxw/codemap/install-hooks.mjs` | merges those three into `~/.claude/settings.json`, at every container start |

User-level, not project-level: all of it lands in the sandbox regardless of which
project is mounted, so nothing has to be committed into target repos.

## Why the reference is a file and not a skill

It was a skill, briefly, and that was a mistake twice over.

A skill named `codemap` collides with the command named `codemap`: Claude Code's
picker lists both, identically, under one `/codemap`. Worse, `~/.claude/skills`
inside a sandbox is a **read-write mount of sbx's shared skill store** — so a kit
writing a skill there does not write into one sandbox, it writes onto the host
and into every other sandbox that mounts the store, and the file outlives the
sandbox that installed it. Kit files are supposed to be sandbox-local; that path
is not.

So the reference lives at `~/.sbxw/codemap/FORMAT.md`, beside the checker, and
`/codemap` and the memory block both point at it. This matches sbxw's own rule
of thumb (see the README's Shared skills section): a skill with no system
dependencies belongs in the shared store via `sbxw skills import`, not in a kit.

## Why the memory block

The command and the reference are useless if nobody reaches for them, and the
moment to reach for them doesn't announce itself — it looks like an ordinary "let me go
read the source". So the kit writes a short section into
`/home/agent/.claude/CLAUDE.md`, the file Claude Code reads at the start of every
session in the sandbox, that names the moment: a question about *where* or *why*
is a question for the map, believe the tree over the map, and if you just
reconstructed how a repo fits together and there is none, **offer** to run
`/codemap` before you finish.

It is spliced between `<!-- sbxw:codemap:begin -->` / `<!-- sbxw:codemap:end -->`
fences and rewrites only what sits between them — the same discipline sbxw's
relay uses on that file, which also holds the user's own instructions and the
agent's `#` memories. Re-running is safe.

Naming the moment is where the block stops. What makes an agent act on it at that
moment is the three hooks below — the block is their vocabulary, not their
enforcement, which is why the two say the same thing in the same words.

## Why hooks, and not only the memory block

The memory block names the two moments. Naming them turned out not to be enough,
because neither announces itself:

- *"Check for a map before you read source"* competes with an eager first grep,
  at the one moment the session knows least about the repository.
- *"No map, and you have just worked out how this repo fits together? Run
  `/codemap`"* asks the agent to recognise an instant that arrives by
  sedimentation, and to recognise it while it is trying to stop. It also asks for
  4–8 unrequested files at the point where every agent is told to stop widening
  scope — so the instruction loses even when it is read.

Three hooks move each moment from something the agent must notice to something it
is handed. All three are registered in `~/.claude/settings.json` by
`install-hooks.mjs`, which merges into that file (it also holds the model, the
statusLine and sbxw's own status hooks) and removes its own previous entries
first, so every container start is a no-op.

**`SessionStart` — the map arrives before the first turn.** When
`.sbxw-artifacts/codemap/codemap.md` exists, its text becomes the session's
`additionalContext`, with the section list beside it, the lens directories named,
and the three rules that matter: read the map before searching, believe the tree
where they differ, update the map in the same session as the code. The agent no
longer has to remember to look — it has already read it. Silent when there is no
map, and it defers to a real `lat.md/` vault when the repo has graduated to one.

**`PreToolUse` on `Grep|Glob|Bash` — a second line, at the moment it bites.** It
runs on the Grep and Glob tools, and on a Bash command that is a search
(`grep`/`rg`/`ag`/`ack`/`find`/`fd`) — in a bypass-permissions sandbox the agent
searches through the shell far more than through the two tools, and a hook that
only watched the tools sat out most sessions. On the first such search it
re-checks whether the repo has a usable map, and acts:

- **Mapped** — recalls the index. The `SessionStart` injection is many turns
  back in a long session, gone after a compaction, and absent from a subagent
  spawned to search; this puts it back.
- **Unmapped** — asks the agent to *offer* `/codemap`: name what it would map,
  in a sentence, and let the user answer. This is the offer moved to the moment
  the session first shows it needs a map, rather than waiting for `Stop`.

It returns context and **no permission decision** — the search runs either way,
whatever the user then decides — and acts **once per session**. The unmapped
offer also honours the `Stop` hook's per-repo 12-hour cooldown and stamps it, so
the two never both offer in one session and a refusal is respected until
tomorrow. It defers to a real `lat.md/` vault.

**`Stop` — the offer.** In an unmapped repository, at the end of a session that
demonstrably read the tree, the hook blocks the turn once and asks the agent to
*offer* the map: name what it would map, in a sentence, and let the user answer.
It asks for the offer and forbids the map, so accepting or refusing stays the
user's call. Three things keep it from nagging:

- **Evidence.** It scans the transcript for distinct source files opened plus
  searches run, and stays silent below a floor. Reads through `Bash` (`cat`,
  `sed -n`) count as much as `Read` calls — in a sandbox running with permissions
  bypassed that is how most files are read, and a scan that counted only tool
  calls would score those sessions at zero.
- **A cooldown**, recorded *before* the block is emitted: once per repository per
  12 hours, shared with the search hook's unmapped offer. A refusal is not
  re-litigated at the next Stop, and an offer that never reached the user does
  not repeat either.
- **`stop_hook_active`**, which is how a Stop hook avoids trapping a session in
  its own loop.

`SBXW_CODEMAP_NUDGE=0` turns off the search reminder and the offer (the
`SessionStart` injection stays: reading a map that exists costs nothing to be
wrong about). sbxw's own background runs set `SBXW_CODEMAP_DONE`, and the two
skip themselves there — a session writing the map has no use for being asked to.

## Reading the map

Three ways, in descending order of comfort:

- **sbxw's web UI** — the **Map** button in a pane's top bar. Wiki links resolve,
  backlinks are listed, search spans the map, and the **Graph** tab shows the
  files as a force-directed graph. Nothing to install, works on a remote sandbox.
- **Obsidian** — `.sbxw-artifacts/codemap/` is a valid vault as it stands, since
  the format is Obsidian's. Best for long editing sessions.
- **Any editor** — the links are plain text. WebStorm and GitHub render `[[…]]`
  as literal text unless a plugin teaches them otherwise, and no plugin resolves
  the `[[src/foo.rs#bar]]` half, which points at code rather than notes.

## Writing the map from the web UI

`/codemap` is also what sbxw's **Generate code map** button runs — the Project
panel's map button, on a project that has none yet. It starts a *background*
session in the sandbox (`claude -p /codemap`, no pane), because a map is a long
uninterrupted read of a whole repository: there is nothing to steer while it
happens, and taking over the agent pane would cost you the session you were in
the middle of. sbxw shows the run instead — a badge on the sandbox's row, a
corner card, and the button as its own status line. *Write a lens…* works the
same way (`claude -p "/codemap-lens <brief>"`), and a sandbox runs one of the
two at a time: a lens is written from the map, so it never reads one that is
being rewritten under it.

That background session is why the command's step 6 ends with an HTTP call. The
run has to end somewhere, and the two candidates know different things: the
agent knows the map is written the moment the checker comes back clean, while
the session's exit status knows only that a process stopped. So the daemon puts
the URL in the environment as **`$SBXW_CODEMAP_DONE`** and the command POSTs to
it — `{"ok":true,"note":"…"}`, or `ok:false` with a line about what stopped it.
The exit is kept as the backstop for a run that never gets that far (a session
refused its permissions, a sandbox stopped underneath it). Nothing is set when a
human types `/codemap` (or `/codemap-lens`) in a pane, and the step is a no-op
then.

## Lenses

A code map is written for whoever reads code. A product owner, a new joiner, a
security reviewer each need the same repository told a different way, and none
of them needs `[[src/auth.rs#validate_token]]`. `/codemap-lens` writes that
retelling into `.sbxw-artifacts/codemap-lenses/<slug>/` — same facts, one
audience, its own directory:

```
/codemap-lens a product owner: what each part delivers, for whom, and what it costs to change
```

Beside the map, never inside it. A lens drops sections, merges others and speaks
a different vocabulary, so filing it under `codemap/` would put prose the
checker cannot validate — and links it cannot resolve — inside the map it
paraphrases. It is still a map in every mechanical sense (an index named after
its directory, overviews, wiki links between its own files), so the same checker
validates it and the same viewer reads it:

```bash
node /home/agent/.sbxw/codemap/check-codemap.mjs --dir .sbxw-artifacts/codemap-lenses/product-owner
```

The command is mostly a list of things not to do, because the failure mode is
specific and expensive: a business-facing document wants revenue, users,
priorities and deadlines, and a repository contains none of them. So every claim
has to trace back to the map or to the tree, what the reader will want and the
project does not record has to be **named as missing** rather than filled in,
and an illustrative figure is forbidden outright — an example number in a
business document is quoted back as a real one within the week.

sbxw's web UI drives the same command: the **Code map** panel has a *Write a
lens…* button in its toolbar that takes the brief, runs `/codemap-lens <brief>`
in a background session of the sandbox's agent — the pane stays yours — and
lists what comes back in a picker beside the map. That session has no terminal
anyone is reading, so the command reports the lens written by POSTing to
`$SBXW_CODEMAP_DONE`, exactly as `/codemap` does.
The brief goes over unnamed on purpose. The daemon used to derive the directory
from its opening words and pass it as `--into`, which is how a lens ended up
titled `a-product-owner-who-needs-to`: that directory is what the picker shows.
Naming is a reading task, so the agent does it once it has read the map. Pass
`--into <slug>` by hand when you want a specific directory — refreshing an
existing lens, mostly.

## Usage

Reference the kit *directory* (not a single file) from your `sbxw.toml`:

```toml
kits = [
  "path/to/sbxw/assets/codemap",
]
```

Then, in any sandbox:

```
/codemap                      # map the whole repository
/codemap the auth subsystem   # map one area
/codemap-lens a product owner: value delivered, for whom, cost to change
```

and to check a map by hand at any time:

```bash
node /home/agent/.sbxw/codemap/check-codemap.mjs
node /home/agent/.sbxw/codemap/check-codemap.mjs --dir lat.md   # a real lat.md/ vault
```

## What the checker enforces

- every directory in the map has an index file named after it, listing everything beside it, with nothing stale
- every section opens with a leading paragraph of at most 250 characters
- every `[[wiki link]]` resolves — to a section, or to a real symbol in a real source file
- every `// @lat:` / `# @lat:` back reference in source points at a section that exists
- every leaf section under `require-code-mention: true` is claimed by exactly one test

When a link is close to something real, the error names the fix rather than the
failure: the repo-root path you meant (`did you mean [[src/lib/models/models.ts…`)
or the exact heading spelling, backticks included. Headings are read verbatim —
fenced blocks are masked, inline code never is, because blanking a `code span`
in a heading silently renames the section and every link that addresses it.

Symbol lookup in source is regex-based rather than tree-sitter-based, so it can
miss an exotic declaration — it will never invent one. Exit code is 1 on errors,
0 when clean; the closing "never mentioned in the map" list is a hint, not a
failure.

## Notes

- **No network needed.** The kit only writes files. The npm domains in
  `permissions.network.allow` are there for the optional upgrade path — a repo
  that wants semantic search runs `npm i -g lat.md`, keeps its map in `lat.md/`
  at the repo root instead, and uses `lat check`. The `/codemap` command detects
  that case and defers to it.
- **`.sbxw-artifacts/` is not `.gitignore`d by sbxw**, so a map is committable
  and travels with the repo — which is the point. Whether to commit it is the
  user's call; the agent should ask rather than assume. The same goes for
  lenses, with one extra thought: a lens goes stale the moment the map moves,
  and a stale business document is read by people who have no way to tell.
- `spec.yaml` `content` fields reject dollar-brace, so the shipped JavaScript is
  written without template literals. Keep it that way when editing.
- **The hooks keep one state file**, `~/.sbxw/codemap/state.json`: when the offer
  last fired per repository (written by both the `Stop` hook and the search
  hook), and which sessions have had their search reminder.
  Entries older than a week are dropped on write. Delete it to make the kit
  forget a refusal.
- **`install-hooks.mjs` merges, never overwrites**, and leaves an unparseable
  `~/.claude/settings.json` exactly as it found it — three hooks are not worth
  the rest of a user's configuration.
- `startup` entries are exec-style arrays (`command: ["node", "…"]`), not shell
  strings.

## Manual apply (existing sandbox)

```sh
sbx kit add <sandbox> path/to/sbxw/assets/codemap
sbx kit validate path/to/sbxw/assets/codemap   # sanity check the spec
```
