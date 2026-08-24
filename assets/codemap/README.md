# codemap kit

Gives every sandbox agent two things it doesn't have by default: the reflex to
**look for a repository's code map before reading its source**, and the means to
**write one when there is none**.

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
| `~/.sbxw/codemap/FORMAT.md` | the format reference the command and the memory block point at |
| `~/.sbxw/codemap/check-codemap.mjs` | offline checker (no dependencies, no network) |
| `~/.sbxw/codemap/memory-block.md` | the paragraph spliced into user memory |
| `~/.sbxw/codemap/install-memory.mjs` | the splice, run at every container start |

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
session in the sandbox, that names the moment: check
`.sbxw-artifacts/codemap/codemap.md` first, believe the tree over the map, and if
you just reconstructed how a repo fits together and there is no map, run
`/codemap` before you finish.

It is spliced between `<!-- sbxw:codemap:begin -->` / `<!-- sbxw:codemap:end -->`
fences and rewrites only what sits between them — the same discipline sbxw's
relay uses on that file, which also holds the user's own instructions and the
agent's `#` memories. Re-running is safe.

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
  user's call; the agent should ask rather than assume.
- `spec.yaml` `content` fields reject dollar-brace, so the shipped JavaScript is
  written without template literals. Keep it that way when editing.
- `startup` entries are exec-style arrays (`command: ["node", "…"]`), not shell
  strings.

## Manual apply (existing sandbox)

```sh
sbx kit add <sandbox> path/to/sbxw/assets/codemap
sbx kit validate path/to/sbxw/assets/codemap   # sanity check the spec
```
