# agent-skills kit

Puts third-party agent skills in every sandbox that applies it, pinned to a
commit, with what they run on:

- **[Archify](https://github.com/tt-a1i/archify)** (`/archify`) — architecture,
  workflow, sequence, data-flow and lifecycle diagrams as validated, interactive
  standalone HTML, from a description, pasted Mermaid, or a repository traced
  from source. The [codemap kit](../codemap/README.md) draws its map's diagram
  with it.
- **[/brag](https://github.com/latent-spaces/brag)** (`/brag`, `/brag-slim`) — a
  short launch video of the project, with music, motion and share copy.
- **[i-have-adhd](https://github.com/ayghri/i-have-adhd)** (`/i-have-adhd`) —
  shapes every answer for a reader with ADHD (next action first, numbered steps,
  state restated each turn) until "stop adhd mode". **Always on**: a
  `SessionStart` hook injects the rules into every session (see below).
- **[/handoff](https://github.com/mattpocock/skills/tree/main/skills/productivity/handoff)**
  — compacts the conversation into a handoff document for a fresh agent.
- **[/grill-me](https://github.com/mattpocock/skills/tree/main/skills/productivity/grill-me)**
  — a relentless interview to sharpen a plan. Upstream it only hands over to the
  `grilling` skill, so that one is installed too (as a command the agent may
  also reach for on its own, on a "grill me").
- **[frontend-design](https://github.com/anthropics/skills/tree/main/skills/frontend-design)**
  (`/frontend-design`) — Anthropic's guidance for distinctive, intentional UI
  design; the agent reaches for it on its own when building UI.

These five are plain markdown: no runtime, no extra network. `/i-have-adhd`,
`/handoff` and `/grill-me` keep their upstream `disable-model-invocation`, so
they run only when typed.

## i-have-adhd, always on

Upstream ships always-on mode as a plugin hook that finds `SKILL.md` through the
plugin's own tree. The kit carries the same hook, `hook-adhd.mjs`, reading the
pinned copy instead, and `install-hooks.mjs` registers it in
`~/.claude/settings.json` under `SessionStart` (merged, never duplicated, a
corrupt file left alone — as the codemap kit does). Like upstream it fires only
while `~/.claude/.i-have-adhd-always` exists. The kit creates that file on the
sandbox's **first** start only, so it is on by default and
`rm ~/.claude/.i-have-adhd-always` turns it off for good in that sandbox — a
later container start does not turn it back on. "stop adhd mode" turns it off
for the current session.

## What it installs

| Path in the sandbox | What it is |
| --- | --- |
| `~/.sbxw/agent-skills/archify/` | Archify's skill package (`SKILL.md`, `bin/archify.mjs`, schemas, references) |
| `~/.sbxw/agent-skills/brag/`, `…/brag-slim/` | the two /brag skills, bundled music and SFX included |
| `~/.sbxw/agent-skills/i-have-adhd/`, `…/handoff/`, `…/grill-me/`, `…/grilling/`, `…/frontend-design/` | the markdown skills |
| `~/.sbxw/agent-skills/hook-adhd.mjs`, `install-hooks.mjs` | i-have-adhd's always-on hook, and what registers it in `~/.claude/settings.json` |
| `~/.claude/.i-have-adhd-always` | the always-on flag, created on the first start |
| `~/.sbxw/agent-skills/*/.sbxw-rev` | the commit each was installed from |
| `~/.claude/commands/archify.md`, `brag.md`, `brag-slim.md`, `i-have-adhd.md`, `handoff.md`, `grill-me.md`, `grilling.md`, `frontend-design.md` | the commands: each points the agent at the pinned `SKILL.md` and says what is specific to a sandbox |
| `~/.sbxw/agent-skills/install.sh` | the installer, run at every container start |
| `~/.cache/ms-playwright/chromium-*` + `~/.local/bin/chromium` | headless Chromium, and a wrapper that adds `--no-sandbox` |
| `ffmpeg` (apt) | /brag's poster frame and final mux |
| a fenced block in `/etc/sandbox-persistent.sh` | `ARCHIFY_CHROME` and `HYPERFRAMES_BROWSER_PATH`, pointing at the wrapper |

## Why commands that point at a copy, and not skills

`~/.claude/skills` inside a sandbox is a read-write mount of sbx's **shared**
skill store (see the main README's Shared skills section): a kit writing a skill
there writes onto the host and into every sandbox that mounts the store, and the
file outlives the sandbox. So the packages land in `~/.sbxw/agent-skills/`, which
belongs to this sandbox alone, and a user-level command in `~/.claude/commands/`
carries each skill's description — which is what lets the agent reach for
`/archify` on "draw me the architecture" as well as on the slash command — and
sends it to the pinned `SKILL.md`. Same arrangement as `/codemap` and `/md-to-pdf`.

Both skills are more than a markdown file anyway: Archify needs a browser for
its last gate, /brag needs FFmpeg and a renderer. That is what makes them a kit
rather than a `sbxw skills import`.

## Why pinned

`install.sh` fetches each repository as the tarball of one commit
(`ARCHIFY_REV`, `BRAG_REV`, `ADHD_REV`, `POCOCK_REV`, `ANTHROPIC_REV` at the top of the script) and records it in
`.sbxw-rev`. A container start with the same pin downloads nothing; moving a pin
is how a skill is updated, and the next start swaps the new copy in whole —
a failed download leaves the previous one in place. The commands tell the agent
never to install or update a skill itself (Archify's own instructions say the
same): an update is a change to this kit, reviewed like any other.

| Skill | Pinned at |
| --- | --- |
| Archify | `73aaa06` — v3.0.1 |
| /brag, /brag-slim | `7079945` — v0.4.0 |
| i-have-adhd | `1ecea19` — main, 2026-10-11 |
| /handoff, /grill-me, grilling | `49dd158` — main, 2026-10-09 |
| frontend-design | `dbd4588` — main, 2026-10-09 |

## Why Playwright's Chromium

Archify's `finalize` ends with a browser gate (four viewports, both themes), and
Hyperframes renders /brag's video in a browser. Chrome for Testing — what
Hyperframes would download by itself — has no linux-arm64 build, which is what a
sandbox on an Apple Silicon host runs; Ubuntu's `chromium` is a snap, which does
not run in a container. Playwright publishes Chromium for both architectures, so
the kit installs that (`npx playwright@1.63.0 install chromium`), adds the shared
libraries `ldd` says are missing, and checks that it actually starts headless
before pointing anything at it.

Chrome's own sandbox needs user namespaces the container does not give a
non-root user, hence the wrapper with `--no-sandbox`. Without a working browser
the environment block is removed rather than left pointing at nothing: Archify
reports an absent browser as `skipped`, and the commands tell the agent to say
so instead of claiming the gate passed.

## Usage

Reference the kit *directory* from your `sbxw.toml`, before the codemap kit:

```toml
kits = [
  "path/to/sbxw/assets/agent-skills",
  "path/to/sbxw/assets/codemap",
]
```

Like every bundled kit it declares startup commands, so it is applied at
creation: a sandbox that already exists needs `sbxw rm <name>` then
`sbxw up <name>` to get it. Then, in the sandbox:

```
/archify the request path from the web UI to the sandbox, through the daemon
/brag
/brag --tone "fake Series A launch from 2016"
```

## Required network allowlist

The installers `403` unless these are in `sbxw.toml`'s `network_allow` (sbxw
applies the policy before the kit runs). `github.com`, `codeload.github.com` and
`registry.npmjs.org` are in sbxw's defaults already:

```toml
network_allow = [
  # ...defaults...
  "cdn.playwright.dev", "playwright.download.prss.microsoft.com",   # Chromium
  "deb.debian.org", "security.debian.org",                         # ffmpeg, Chromium's libraries
  "archive.ubuntu.com", "security.ubuntu.com", "ports.ubuntu.com",
]
```

At use time, `/brag` runs `npx hyperframes` (npm) and Archify's receipts carry a
bounded update check that simply reports `unavailable` when it cannot reach
GitHub.

## Notes

- Every step is best-effort: a skill whose download failed keeps its previous
  copy, a missing ffmpeg or browser is reported in the container's start log and
  by the skill when it needs it, and the container start itself never fails.
- `uv` is not installed, so /brag's richer music-cue analysis
  (`analyze_music_cues.py`) is unavailable; it falls back to the bundled presets
  or `npx hyperframes beats`, which its own instructions allow.
- `spec.yaml` `content` fields reject dollar-brace: the installer is written
  without it. Keep it that way when editing.

## Manual apply (existing sandbox)

```sh
sbx kit validate path/to/sbxw/assets/agent-skills   # sanity check the spec
```

`sbx kit add` refuses a kit with startup commands; recreate the sandbox instead.
