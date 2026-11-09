## Search the map before you search the tree

A project here may carry a **code map**: `.sbxw-artifacts/codemap/`, a few linked
markdown files saying what it does, why it is built this way, and where each idea
lives in the tree. When there is one, its index was put into this session's
context before your first turn — you did not have to go looking, and neither
should your next search.

- **A question about *where* or *why* is a question for the map.** One section
  replaces a dozen greps, and it carries the *why*, which no amount of grepping
  recovers. Read it first, then search the tree for the detail it points you at.
  Follow its `[[wiki links]]`: `[[name]]` is `.sbxw-artifacts/codemap/name.md`,
  and `@lat:` comments in source point back at the section that explains them.
- **The tree wins over the map.** The map records what was true when it was
  written — where it disagrees with the code, believe the code, and fix the map
  as you pass. Leaving a known-wrong section is worse than having no map.
- **No map, and you have just worked out how this repo fits together? Offer to
  run `/codemap` before you finish** — that understanding is the expensive part
  of the session and it dies with the session otherwise. Offer it: name what you
  would map, in a sentence, and let the user answer. Do not write a map nobody
  asked for, and take "no" for an answer. `/codemap <area>` maps one subsystem,
  which is usually the better offer after a focused session.
- **In a mapped repo, changing behaviour, architecture or tests means updating
  the map in the same session**, then re-running
  `node /home/agent/.sbxw/codemap/check-codemap.mjs` until it is clean.
- **`.sbxw-artifacts/codemap-lenses/` is not the map.** Each subdirectory there
  is the map retold for one reader — a product owner, a new joiner — by
  `/codemap-lens`. Derived, deliberately partial, and possibly stale: read it to
  learn who was asking for what, never to learn what the code does. When the map
  changes, offer to refresh the lenses; do not let them drift in silence.

`/home/agent/.sbxw/codemap/FORMAT.md` has the format in full: sections,
`[[wiki links]]`, code links and test specs.
