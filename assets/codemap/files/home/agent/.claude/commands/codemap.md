Write or refresh this project's code map — a small knowledge graph in markdown that tells the next agent what this codebase does, why it is built this way, and where each idea lives.

Scope for this run: $ARGUMENTS  (empty means the whole repository)

The map lives in `.sbxw-artifacts/codemap/`, so sbxw's web UI lists it and the
next session finds it without being told. It is written in the **lat.md format**
— `/home/agent/.sbxw/codemap/FORMAT.md` is the full reference; the essentials are
repeated below.

## 0. Situate yourself

- `ls .sbxw-artifacts/codemap/ 2>/dev/null` — if it exists this is a **refresh**:
  read `codemap.md` (the index) and every file you are about to touch *before*
  changing anything. Never rewrite a map you have not read.
- If the repo instead has a `lat.md/` directory at its root, it already uses the
  real tool. Work there, keep its conventions, and validate with `lat check`
  instead of this kit's checker. Do not create a second map.
- Confirm the repository root — every path you write is relative to it.

## 1. Reconnaissance, before writing a word

Spend your first calls on the sources with the highest information density.
Roughly in order:

- `README`, then the build manifest (`Cargo.toml`, `package.json`, `pyproject.toml`,
  `go.mod`) — they name the moving parts and the commands that drive them.
- `git log --oneline -30`, then
  `git log --format= --name-only -300 | sort | uniq -c | sort -rn | head -25` —
  what actually changes is what actually matters. Frozen files rarely need a map.
- The tree: top-level `ls`, then file counts per directory. Find the entry points
  (`main`, `index`, `lib`, `cli`, `server`) and the test directories.
- Prose that already exists: `docs/`, `CLAUDE.md`, `AGENTS.md`, ADRs, and module
  doc comments (Rust `//!` headers, Python module docstrings, JSDoc file blocks).
  These are usually the only place the *why* was ever written down. Long doc
  comments at the top of a file are the single best source in most repos.
- Then read the two or three files that everything else depends on.

For each area you intend to map, be able to answer: what is it for, what does it
guarantee, and what would surprise someone who only read the code? The third
question is the one worth the map.

## 2. Decide the shape

- **One file per domain or subsystem, never one per source file.** A first pass
  is usually 4–8 files. A map nobody can hold in their head is a second codebase.
- `codemap.md` is the index: one bullet per file, `- [[name]] — one sentence`.
- Add subdirectories only when a topic grows past three files; each subdirectory
  gets its own index, **named after the directory** (`api/api.md`).
- Name files after concepts (`auth.md`, `scheduling.md`), not after paths.
- `diagram/` is reserved for step 6's Archify diagram. It holds no markdown and
  needs no index; do not name a map subdirectory after it.
- If the project has tests worth describing as behaviour, give them `tests.md`
  with `require-code-mention: true` in the frontmatter (see step 4).

## 3. Write the sections

Format rules the checker enforces. Write to them from the first line — they are
cheap to satisfy while writing and expensive to retrofit across a finished map:

- **Every section opens with a leading paragraph** — one or two sentences
  immediately after the heading, before any child heading, list or code block,
  **≤250 characters, a hard limit** (wiki-link text doesn't count). Write that
  sentence *first*, then the detail underneath it. It is the section's identity:
  what a future agent reads in search results and previews, and often all it
  reads. If the overview needs three clauses, the section is really two sections.
- **Source paths are relative to the repository root.** Not to the map file, not
  a bare file name — the same path `git ls-files` would print.

  ```
  wrong:  [[../../src/lib/models/models.ts#FieldConfiguration]]
  wrong:  [[models.ts#FieldConfiguration]]
  right:  [[src/lib/models/models.ts#FieldConfiguration]]
  ```

- **Wiki links tie the graph together**: `[[file#Heading#Subheading]]` for a
  section, `[[file#Heading|nicer words]]` to rename the anchor text, and
  `[[src/auth.ts#validateToken]]`, `[[src/main.rs#Server#run]]`,
  `[[lib/utils.py#parse_args]]` to point at real symbols in source.
- The file's `#` H1 may be left implicit in a link: `[[cli#mcp]]` reaches
  `cli.md` → `# CLI` → `## mcp`.
- **A heading is an address, so it is matched verbatim** — backticks and
  punctuation included. `## The `childEvent` contract` can only be reached by
  `[[components#The `childEvent` contract]]`, which nobody writes correctly
  twice. Prefer plain heading text and put the code in the paragraph below it.
  Links to markdown outside the map (`[[../../README.md]]`) don't resolve at
  all: say it in prose, or link the section that covers it.

What belongs in a map:

- Decisions and the reasoning behind them — especially the ones a reader would
  otherwise undo by accident.
- Invariants, constraints, and the failure modes the code defends against.
- Domain vocabulary: the words this team uses, and what they mean here.
- Contracts and protocols between parts, including who owns what state.
- Anything that surprised *you* while reading. That surprise is the map's value.

What does not:

- A walkthrough of what the code plainly says. The code is the walkthrough.
- Generated API documentation, or an inventory of every function.
- TODOs, session notes, plans, or anything that goes stale in a week.

Write only what you verified in the tree, and anchor claims with code links. If
you are guessing, either check it or leave it out — a confident wrong map costs
more than no map. When a question stays open, say so in a sentence and name what
you could not confirm.

## 4. Anchor the code back to the map (where it pays)

Add `// @lat: [[section-id]]` (or `# @lat:` in Python) directly above the
definition a section explains — one comment per section, at the code that
carries the meaning, not at the top of the file.

Do this where a reader would otherwise be lost, not everywhere: a handful of
well-placed anchors beats a carpet of them. **Ask the user before adding these
if the repo is not yours to edit** — they change source files, unlike the map.

For test specs under `require-code-mention: true`, each leaf section must be
referenced by exactly one `@lat:` comment next to the test that covers it; the
checker reports any spec no test claims.

## 5. Validate — early, then again at the end

```bash
node /home/agent/.sbxw/codemap/check-codemap.mjs
```

**Run it as soon as the index and the first content file exist**, not after the
last one. Every rule it enforces is one you are about to repeat in every
remaining file, and a habit caught on file one costs a minute — the same habit
caught on file six costs a rewrite of all six. Then run it again at the end, and
fix everything it reports until it is clean.

It checks that every directory has its index, that every section opens with an
overview, and that every wiki link, code link and `@lat:` reference still points
at something that exists. When a link is close to something real it names the
fix: the exact heading spelling, or the repo-root path you meant. Its closing
"never mentioned in the map" list is a hint, not an error — map what carries
meaning, skip what doesn't.

The three ways a first pass usually fails, all avoidable:

1. **Overviews written as summaries.** 900-character opening paragraphs. Lead
   with the sentence, follow with the detail.
2. **Source paths copied from the wrong vantage point** — `../../src/…` or a bare
   file name. They are repo-root-relative.
3. **Links to headings that contain code**, quoted without their backticks.

## 6. Draw the diagram (Archify)

The markdown is for the next agent; the picture of the whole is for the people
reading over its shoulder. Draw it with **Archify**, from the reading you have
just done — not a second exploration — as a repository-backed `architecture`
diagram:

- Archify lives at `/home/agent/.sbxw/agent-skills/archify` (installed by sbxw's
  agent-skills kit). **If that directory is missing, skip this step** and say so
  in the report: the map is complete without it.
- Read its `SKILL.md`, then follow its fast authoring path and its
  `references/repository-authoring.md`. `bin/archify.mjs` and every relative path
  in them resolve against `/home/agent/.sbxw/agent-skills/archify/`; never
  install or update Archify yourself.
- The location is fixed, so the web UI and the checker find it:
  `.sbxw-artifacts/codemap/diagram/architecture.json` for the candidate,
  `meta.output` set to `.sbxw-artifacts/codemap/diagram/architecture.html`.
  On a refresh, edit the candidate that is there instead of starting over.
- **One component per map section that names a runtime part**, labelled with that
  section's heading text, verbatim — it is how a reader crosses from the picture
  to the prose, and the checker warns about a label no heading carries. Its
  `sources` cite the same code the section links to. Relationships come from call
  sites you read, not from the prose.
- Render and check it in one command, from the repository root:

  ```bash
  node /home/agent/.sbxw/agent-skills/archify/bin/archify.mjs finalize architecture \
    .sbxw-artifacts/codemap/diagram/architecture.json \
    .sbxw-artifacts/codemap/diagram/architecture.html \
    --repo-root . --quality showcase --json
  ```

  Repair what it reports within Archify's own repair limit. A browser gate
  reported `skipped` means this sandbox has no Chromium: say so, do not call it
  passed. A repository with no commit or no `origin` cannot pin
  `meta.repository`; draw it without `--repo-root` and say the sources are
  unverified.
- Run the map checker again: it also checks that the diagram was rendered after
  its last edit and that every file it cites still exists.

A diagram that will not pass is not worth holding the map back for: stop at
Archify's repair limit, leave no half-written `diagram/` behind, and report it.

## 7. Report back

Tell the user, briefly: what you mapped, what you deliberately left out and why,
and anything you could not verify. If you added `@lat:` comments, say which files
you touched. Give the diagram's path and the gates it passed — or why there is
none.

**If `$SBXW_CODEMAP_DONE` is set**, this run was started from sbxw's web UI: there
is no pane anyone is reading, and that URL is where the person who clicked is
waiting. POST to it the moment the map is written, the diagram drawn (or given
up on) and the checker clean — before anything optional you still mean to do,
because until it arrives the UI can only say "still writing".

```bash
curl -fsS -X POST "$SBXW_CODEMAP_DONE" -H 'content-type: application/json' \
  -d '{"ok":true,"note":"7 files under .sbxw-artifacts/codemap/"}'
```

Send `{"ok":false,"note":"…"}` instead if you are stopping *without* a map, and
say in one line why. Reporting either way is what turns the run into an outcome
someone can read; saying nothing leaves sbxw to infer one from your exit status,
which knows whether the process ended and nothing whatsoever about the map.

## Refreshing a map that already exists

- Read the index and the affected files first; edit in place, keep the sections
  that are still true, and delete what has become false. Do not append a second
  account of the same thing.
- When code moved, fix the links rather than dropping them — a broken link is a
  signal the checker will surface; a deleted one is silence.
- After changing behaviour, architecture or tests in a mapped repo, update the
  map in the same session and re-run the checker. A map nobody maintains is worse
  than no map, because it is still believed.
- When the change moved a component or a relationship, edit
  `diagram/architecture.json` to match and run step 6's `finalize` again. The
  checker says when the diagram cites a file that is gone, or was drawn at an
  older commit than `HEAD`; the second is a prompt to look, not a failure.
