# Code map format

A code map is a small set of cross-linked markdown files describing **what** a
project does and **why** — domain concepts, design decisions, invariants, test
specs. It is not a copy of the code; it is the layer the code cannot express.
It lives in `.sbxw-artifacts/codemap/` and follows the
[lat.md](https://github.com/vercel-labs/lat.md) format. Beside the markdown, in
`diagram/`, sits the one picture of the whole: an
[Archify](https://github.com/tt-a1i/archify) architecture diagram (see
[The diagram](#the-diagram)).

## Reading a map

Start at `.sbxw-artifacts/codemap/codemap.md` — the index, one line per file.
Follow `[[wiki links]]` to the section you need instead of grepping: the map is
where intent lives, grep is for finding code. In source files, an
`// @lat: [[section]]` comment points back at the section explaining that code.

The map states what was true when it was written. If the tree disagrees with it,
the tree wins — and the map is now wrong where you looked, so fix it as you pass.

## Section structure

Every section opens with a **leading paragraph**: one or two sentences right
after the heading, before any child heading, list or code block, at most **250
characters** (text inside `[[…]]` doesn't count). It is the section's identity —
what search results, previews and other agents read first.

```markdown
# Good Section

Brief overview of what this section documents and why it matters.

More detail can follow in later paragraphs, lists or code blocks.

## Child heading

Details about this child topic.
```

A section that starts straight into a child heading is invalid — the parent has
said nothing, and a link to it resolves to an empty promise.

## Section ids

A section is addressed by file path plus heading chain:

- Full: `.sbxw-artifacts/codemap/api/auth#OAuth Flow#Refresh`
- Short: `auth#OAuth Flow#Refresh` — the bare file stem works when it is unique
- The file's H1 may be left implicit: `[[cli#mcp]]` reaches `cli.md` → `# CLI` → `## mcp`

Headings are addresses, and they are matched **verbatim** — backticks and
punctuation included. A heading like ``## The `childEvent` contract`` is only
reachable as ``[[components#The `childEvent` contract]]``. That works, but it is
a link nobody writes correctly twice: prefer plain heading text and put the code
in the paragraph below.

## Wiki links

`[[target]]` links a section; `[[target|alias]]` renames the anchor text.

```markdown
See [[cli#init]] for setup.
The parser validates [[markdown#Wiki Links|link syntax]].
```

Links can point straight at source symbols, which is what keeps a map honest.
**Paths are relative to the repository root** — never to the map file, never a
bare file name: `[[src/lib/models/models.ts#FieldConfiguration]]`, not
`[[../../src/lib/models/models.ts#…]]` and not `[[models.ts#…]]`.

```markdown
[[src/config.ts#getConfigDir]]     — function
[[src/server.ts#App#listen]]       — class method
[[lib/utils.py#parse_args]]        — Python function
[[src/lib.rs#Greeter#greet]]       — Rust impl method
[[src/app.go#Greeter#Greet]]       — Go method
[[src/app.h#Greeter]]              — C struct
[[src/config.ts]]                  — the file itself
```

## Code refs

Tie a source definition back to the section that explains it:

```typescript
// @lat: [[auth#OAuth Flow]]
export function validateToken(token: string) { … }
```

```python
# @lat: [[auth#OAuth Flow]]
def validate_token(token): ...
```

One comment per section, placed at the code that carries the meaning — not at the
top of the file — and only where a reader would otherwise be lost.

## Indexes

Every directory in the map has an index file **named after the directory**:
`.sbxw-artifacts/codemap/codemap.md` for the root, `…/api/api.md` for `api/`. It
lists every file and subdirectory beside it, one bullet each:

```markdown
Short paragraph saying what this part of the map covers.

- [[auth]] — authentication and session rules
- [[api]] — HTTP surface, versioning and error contracts
```

## The summary

`.sbxw-artifacts/codemap/summary.md` is the project told in one breath: **one
to three paragraphs of plain prose, at most 300 words, no headings**. Write it
for someone who has to *talk* about the project without reading the code —
what it does and for whom, its stack, its main parts and how they fit, and
where it stands (what works, what is in progress). Link sections with
`[[wiki links]]` where it helps; the links do not count as words.

It is listed in the index like any file (`- [[summary]] — the project in 300
words`) and refreshed with the map. sbxw's voice concierge reads it, as is, as
its picture of the project, so a stale summary is a concierge that talks about
the wrong project. The checker fails a map without one, and one that is too
long or has headings.

## Test specs

A file whose frontmatter sets `require-code-mention: true` becomes a contract:
every leaf section must be claimed by exactly one `@lat:` comment in test code.

```markdown
---
lat:
  require-code-mention: true
---
# Tests

Behaviour specifications for the authentication surface.

## User login

Credential validation and error handling for the login endpoint.

### Rejects expired tokens

Tokens past their expiry are rejected with 401, even if otherwise well formed.
```

```python
# @lat: [[tests#User login#Rejects expired tokens]]
def test_rejects_expired_tokens(): ...
```

Every spec section needs a real description — one sentence on what it verifies
and why. A bare heading is not a spec.

## What belongs, what doesn't

Write: decisions and their reasons, invariants and constraints, domain
vocabulary, contracts between parts, failure modes, and whatever surprised you
while reading the code.

Don't write: walkthroughs of what the code plainly says, generated API listings,
TODOs, or session notes. And never write a claim you have not checked against
the tree.

## The diagram

`diagram/` holds the map's picture, drawn with Archify (installed by sbxw's
agent-skills kit at `/home/agent/.sbxw/agent-skills/archify`):

| File | What it is |
| --- | --- |
| `diagram/architecture.json` | the typed candidate — the diagram's source, edited by hand |
| `diagram/architecture.html` | the standalone, interactive render; what people open |
| `diagram/architecture.*.json` | Archify's receipts from the last `finalize` |

It is repository-backed: `meta.repository` pins the commit it was drawn at, and
every component cites the code it stands for in `sources`. Each component is
labelled with the heading of the map section that explains it, so the picture
and the prose can be read against each other. The markdown stays the record —
the diagram never says anything the map does not.

Edit the JSON, never the HTML, then render and check again from the repository
root:

```bash
node /home/agent/.sbxw/agent-skills/archify/bin/archify.mjs finalize architecture \
  .sbxw-artifacts/codemap/diagram/architecture.json \
  .sbxw-artifacts/codemap/diagram/architecture.html \
  --repo-root . --quality showcase --json
```

A map without `diagram/` is still a valid map: the diagram is drawn when Archify
is available, and the checker only mentions its absence.

## Validating

```bash
node /home/agent/.sbxw/codemap/check-codemap.mjs            # whole repo
node /home/agent/.sbxw/codemap/check-codemap.mjs --dir lat.md   # a lat.md/ vault
```

Checks indexes, leading paragraphs, wiki links, code links, `@lat:` refs and
`require-code-mention` coverage. Symbol lookup is regex-based: it can miss an
exotic declaration, but it never invents one. Run it after every edit to the map,
and treat a failure as work that is not finished.

When `diagram/` exists it is checked too: the HTML must be newer than the JSON
(a render older than its source is stale), every cited source file must still
exist and be long enough for its line range, and a component label no map
heading carries is a warning. So is a diagram drawn at another commit than
`HEAD` — look whether the architecture moved, and redraw if it did.

A repository that already keeps its map in a `lat.md/` vault at its root
validates it with that project's own `lat check`; this kit works in it rather
than beside it.

`/codemap` walks through building or refreshing a map end to end; this file is
the reference it leans on.
