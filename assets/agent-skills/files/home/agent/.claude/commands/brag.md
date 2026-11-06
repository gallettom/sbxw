Turn the current project into a short, polished, shareable launch video — music, motion and share copy included (/brag, powered by Hyperframes). Use when someone says "/brag", "let's brag about this", "make a launch video" or wants to show off what they built.

Invocation: /brag $ARGUMENTS

/brag is installed in this sandbox by sbxw's agent-skills kit, at
`/home/agent/.sbxw/agent-skills/brag` (the commit is in `.sbxw-rev` there).
**Read `/home/agent/.sbxw/agent-skills/brag/SKILL.md` now and follow it**, passing
it the invocation above. What is specific to this sandbox:

- **`<skill-dir>`** in SKILL.md and its references is
  `/home/agent/.sbxw/agent-skills/brag`; `references/`, `scripts/`, `slim.md`
  and the bundled audio resolve against it.
- **Tools.** FFmpeg is installed by the kit. Hyperframes is fetched by `npx
  hyperframes` on first use (registry.npmjs.org must be allowed); it renders with
  the kit's Chromium through `HYPERFRAMES_BROWSER_PATH`, because the
  chrome-headless-shell it would otherwise download has no linux-arm64 build.
  `uv` is not installed: for music cues use a bundled preset or
  `npx hyperframes beats`, as SKILL.md allows.
- **Previews.** This sandbox has no display and its ports are not published by
  default: prefer `npx hyperframes snapshot` over `preview`, and give the user
  the path to `brag.mp4` rather than trying to open it.
