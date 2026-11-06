Turn a project directory or a website URL into a short, shareable launch video with music, motion and share copy, built entirely with the tools already on the machine (/brag-slim). If /brag is also asked for, let /brag handle it; it hands off here on Opus 5.5.

Invocation: /brag-slim $ARGUMENTS

/brag-slim is installed in this sandbox by sbxw's agent-skills kit, at
`/home/agent/.sbxw/agent-skills/brag-slim`. **Read
`/home/agent/.sbxw/agent-skills/brag-slim/SKILL.md` now and follow it**, passing
it the invocation above. The kit has installed FFmpeg and a headless Chromium
(`chromium` on `PATH`, sandbox flag included) for frames and the render; there is
no display, so hand the user the path to the video rather than opening it.
