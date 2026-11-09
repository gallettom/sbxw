Retell this project's code map for one reader, as a separate document beside it.

Brief for this run: $ARGUMENTS

A code map is written for whoever reads code. A product owner, a new joiner, a
security reviewer or an auditor each need the same repository told a different
way, and none of them needs `src/auth.rs#validate_token`. A **lens** is that
retelling: same facts, one audience, its own directory.

sbxw's Code map panel runs this command with the brief you were given, in a
background session nobody is watching, and waits to hear that the lens is
written (see the last step). It can also be run by hand in a pane.

## 0. Name the lens — this is yours to get right

The brief is prose: who is reading, and what they need out of it. Nothing in it
is a directory name. **You choose the name**, and it is not a formality — the
lens's directory *is* its title everywhere it is read: the Code map panel lists
`codemap-lenses/product-owner/` as "Lens · product owner", and that line is all a
reader has to go on when they pick between four of them.

So do not slice it off the front of the brief. "A product owner: what each part
delivers…" is a brief; `a-product-owner-what-each-part` is not a title. Name it
after you have read the brief and the map, from what the document turned out to
be:

- two or three kebab-case words, ASCII, no trailing dash — the reader and their
  purpose: `product-owner`, `security-review`, `new-joiner-backend`,
  `incident-response`, `data-retention`;
- name the *audience or the question*, never the format: not `lens`, `overview`,
  `document`, `codemap`, and not the project's own name — every lens here is a
  lens of this project;
- already have one for that audience? Refresh it under the same name rather than
  adding a near-duplicate beside it (see the last section).

`--into <slug>`, when it is present, overrides all of that: a human naming the
directory by hand, usually to land on top of a lens that already exists. Use it
verbatim.

Say which name you chose, and why that one, when you report back.

## 1. Read the map. All of it.

- `.sbxw-artifacts/codemap/codemap.md` is the index; read it and every file it
  lists. A lens is written **from the map**, not from a fresh reading of the
  source — the map is where the reasoning was written down, and re-deriving it
  wastes the session the map exists to save.
- No map? Stop and say so: `/codemap` writes one, and there is nothing to retell
  until it exists. Do not silently write a lens from the source instead.
- Where the map is thin on something the brief needs, go and check the tree for
  that one thing. Note in your report that you did.

## 2. Write it where it belongs

Everything goes under `.sbxw-artifacts/codemap-lenses/<slug>/`, and **nothing
else is touched**. The map is the source: a lens never edits it, never adds to
it, and never moves a file out of it. Two more rules the viewer and the checker
both depend on:

- the index file is named after its directory — `<slug>/<slug>.md` — and lists
  every other file beside it, one bullet each;
- three to six files is a lens. It is a shorter document than the map, not a
  longer one: a reader who wanted the full account would read the map.

Open the index with a frontmatter block recording what this is, so nobody a
month from now has to guess which brief produced it:

```
---
lens: product-owner
brief: what a product owner needs — value delivered, for whom, cost to change
source: .sbxw-artifacts/codemap/
written: 2026-03-04
---
```

## 3. Rewrite, do not summarise

The temptation is to shorten every section by a third and call it a lens. That
produces a worse map, not a different document. Reorganise instead:

- **Lead with what this reader came for.** For a product owner that is what the
  system delivers and to whom; the architecture is background, if it appears at
  all. For a new joiner it is where to start and what will surprise them. Let
  the brief decide the running order, not the map's.
- **Speak their vocabulary.** The map's domain terms are usually the right ones
  — they came from the team. Module names, symbol names and file paths usually
  are not.
- **Drop what this reader cannot act on.** Code links (`[[src/auth.ts#login]]`)
  belong in a lens only when the reader reads code. Keep `[[wiki links]]`
  *between the lens's own files*: they are what makes it navigable in the panel.
- **Keep the section shape.** Every section still opens with a leading paragraph
  of at most 250 characters, because the checker and the viewer both read it.

## 4. Never invent the part the map does not have

This is the failure mode that matters, and a business-facing lens is where it
bites: a map records what the code does, and a product owner's document wants
revenue, users, priorities and deadlines — none of which are in the repository.

- Every claim traces to the map or to something you verified in the tree.
- What the reader will want and the project does not record, **name as missing**
  rather than filling in: a short "What this document cannot tell you" section
  at the end of the index costs you three lines and is the most honest part of
  the document.
- No invented metrics, customer names, dates, roadmaps or dollar figures. Not
  even as an illustration — an example figure in a business document is quoted
  back as a real one within the week.
- Translating a technical guarantee into its business consequence is the job
  ("a request is retried three times before it fails" → "a transient network
  fault does not lose the order"). Inventing the consequence is not.

## 5. Validate, then report

```bash
node /home/agent/.sbxw/codemap/check-codemap.mjs --dir .sbxw-artifacts/codemap-lenses/<slug>
```

Same checker, pointed at the lens: it holds the index rule, the overview limit
and the links to account. Its "never mentioned" list means less here than for a
map — a lens leaves things out on purpose — but a broken link still is one.

Then tell the user, in a few lines: which brief you wrote to, the name you gave
the lens and why, what you led with, what you deliberately left out, and what you
had to mark as unknown. Say that it is in the Code map panel's lens picker under
that name, beside the map.

**If `$SBXW_CODEMAP_DONE` is set**, this run was started from sbxw's web UI:
there is no pane anyone is reading, and that URL is where the person who clicked
is waiting. POST to it the moment the lens is written and the checker is clean —
before anything optional you still mean to do, because until it arrives the UI
can only say "still writing".

```bash
curl -fsS -X POST "$SBXW_CODEMAP_DONE" -H 'content-type: application/json' \
  -d '{"ok":true,"note":"4 files under .sbxw-artifacts/codemap-lenses/product-owner/"}'
```

Name the directory you chose in the note: it is the lens's title, the panel
reloads its picker on this call, and the note is what the person reads to know
which of the entries is the one they just asked for. Send
`{"ok":false,"note":"…"}` instead if you are stopping *without* a lens — no map
to retell is the usual reason — and say so in one line, because saying nothing
leaves sbxw to infer an outcome from your exit status, which knows whether a
process ended and nothing about whether a document was written.

## Refreshing a lens that already exists

Read it first, then edit in place — same slug, same directory. A second lens for
the same audience is how a project ends up with three documents that disagree.
If the brief has genuinely changed, say so in the frontmatter rather than
leaving the old one describing itself wrongly.
