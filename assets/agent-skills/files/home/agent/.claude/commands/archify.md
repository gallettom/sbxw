Create polished, validated architecture, workflow, sequence, data-flow and lifecycle/state diagrams as explorable standalone HTML (Archify) — from a description, pasted Mermaid, or a real repository traced from source. Not for numeric charts or dashboards.

Request: $ARGUMENTS

Archify is installed in this sandbox by sbxw's agent-skills kit, at
`/home/agent/.sbxw/agent-skills/archify` (the commit is in `.sbxw-rev` there).
**Read `/home/agent/.sbxw/agent-skills/archify/SKILL.md` now and follow it** for
this request. What is specific to this sandbox:

- **Paths.** `bin/archify.mjs` is `/home/agent/.sbxw/agent-skills/archify/bin/archify.mjs`,
  and every relative path in SKILL.md — `references/`, `schemas/`, `examples/` —
  resolves against `/home/agent/.sbxw/agent-skills/archify/`. Your working directory
  stays the project's: input and output paths resolve from there.
- **Output.** Unless the user names another location, put each new diagram in
  `.sbxw-artifacts/archify/<type>-<slug>-<YYYYMMDD-HHMMSS>/` rather than
  `.archify/…` — same rules as SKILL.md's folder, but sbxw's web UI lists
  `.sbxw-artifacts/`, so the person can open the HTML from there.
- **Browser gate.** The kit installs a headless Chromium as `chromium` on `PATH`
  and points `ARCHIFY_CHROME` at it. If `finalize` reports the browser gate as
  `skipped`, Chromium is not installed in this sandbox: report browser evidence as
  skipped, never as passed.
- **Updates.** Never install or update Archify yourself — the kit pins it. If a
  receipt says an update is available, mention it once and say it arrives by
  moving the pin in sbxw's `assets/agent-skills` kit.
