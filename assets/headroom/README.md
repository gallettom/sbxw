# headroom kit

Installs [Headroom](https://github.com/chopratejas/headroom) — a local
context-compression proxy — inside a sandbox and routes the Claude agent through
it to **cut token usage** (the project claims 60–95% fewer tokens, same answers).

## How it works

On every container start, the kit's startup script:

1. Finds a Python ≥ 3.10.
2. Installs `headroom-ai[proxy]` once via `pip install --user
   --break-system-packages` (→ `~/.local/bin/headroom`). Lightweight — the
   `[proxy]` extra is just the proxy server, no torch/ML/OCR.
3. Runs Headroom's own durable integration: **`headroom init --global claude`**.

`headroom init` writes `~/.claude/settings.json` with:

- `env.ANTHROPIC_BASE_URL = http://127.0.0.1:8787` — Claude Code routes through
  the proxy.
- `SessionStart` / `PreToolUse` hooks that **auto-start the proxy** — so there's
  no daemon, env-file, or health-check wiring to maintain on our side.

The proxy uses **Authorization-header passthrough**: it forwards Claude's
existing credentials to `api.anthropic.com`, so it works with both the API-key
and OAuth auth paths. No separate key needed.

## Usage

```toml
# sbxw.toml — keep above the first [[ports]] table
kits = [
  "/abs/path/to/sbxw/assets/headroom",
]
```

No extra `network_allow` entries are needed: the install hits PyPI and the
`headroom init` plugin clone hits GitHub, both already in the defaults.

`ANTHROPIC_BASE_URL` is applied from `settings.json` **at agent launch**, so after
the first install **Reload the agent** (web UI) or `sbxw down <name> && sbxw up
<name>` for compression to take effect.

## Side effects (worth knowing)

`headroom init --global claude` **overwrites `~/.claude/settings.json`** and, among
other things, sets `defaultMode: bypassPermissions` and registers a Headroom
plugin marketplace. In an isolated sandbox that's usually fine, but it does change
Claude Code's permission posture — be aware.

## Verify / debug

```sh
sbx exec <name> -- bash -lc 'headroom --version; grep ANTHROPIC_BASE_URL ~/.claude/settings.json'
# proxy listening?
sbx exec <name> -- bash -lc 'exec 3<>/dev/tcp/127.0.0.1/8787 && echo up || echo down'
# force the proxy to (re)start the way the SessionStart hook does:
sbx exec <name> -- ~/.local/bin/headroom init hook ensure --profile init-user --marker headroom-init-claude
```

## Disable

Remove the kit from `sbxw.toml`, then in the sandbox:

```sh
headroom unwrap claude 2>/dev/null || true
pkill -f "headroom proxy" 2>/dev/null || true
```
