# headroom kit

Installs [Headroom](https://github.com/chopratejas/headroom) — a local
context-compression proxy — inside a sandbox and routes the Claude agent through
it to **cut token usage** (the project claims 60–95% fewer tokens, same answers).

## How it works

On every container start, the kit's startup script:

1. Finds a Python ≥ 3.10.
2. Installs `headroom-ai[proxy,code]` via `pip install --user
   --break-system-packages` (→ `~/.local/bin/headroom`). `[proxy]` is the proxy
   server; `[code]` adds tree-sitter for AST-aware code compression. No torch/ML/OCR.
3. Runs Headroom's durable integration: **`headroom init --global claude`**, which
   writes `~/.claude/settings.json` (`ANTHROPIC_BASE_URL=http://127.0.0.1:8787` +
   `SessionStart`/`PreToolUse` hooks that **auto-start the proxy** — no daemon or
   env-file wiring on our side).
4. Patches Headroom's deployment manifest
   (`~/.headroom/deploy/init-user/manifest.json`) — the real source of the proxy's
   command line — to force:
   - **`--host 0.0.0.0`** — so the proxy is reachable via a published port (its
     web dashboard / stats), not just sandbox-loopback.
   - **`--mode token`** — maximise token savings.
   - **`--code-aware`** — AST-based code compression (uses `[code]`/tree-sitter).
   - **`--intercept-tool-results`** — compress tool outputs (file reads, command
     output) — the biggest token sink for an agent.

The proxy uses **Authorization-header passthrough**: it forwards Claude's
existing credentials to `api.anthropic.com`, so it works with both the API-key
and OAuth auth paths. No separate key needed.

### Mode note

`token` mode maximises compression (rewrites prior turns). If `headroom perf`
reports "cache prefix unstable" / low reduction (the `token` mode fighting Claude
Code's prompt cache), set `--mode cache` in the manifest's `proxy_args` instead to
preserve cache hits.

### Web dashboard

With `--host 0.0.0.0` and a published port, open the proxy's stats in a browser:

```toml
# sbxw.toml — publish 8787 to reach the dashboard from the host
[[ports]]
alias = "headroom.local"
sandbox_port = 8787
host_port = 8787
```

Then visit `http://127.0.0.1:8787/dashboard` (also `/stats`, `/metrics` for JSON).
The proxy forwards your Anthropic credentials, so don't expose 8787 beyond your host.

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
# proxy command line (should show --host 0.0.0.0 --code-aware --intercept-tool-results):
sbx exec <name> -- bash -lc 'p=$(lsof -t -i :8787|head -1); tr "\0" " " </proc/$p/cmdline; echo'
# stats (compression / rtk / cli_filtering counters):
sbx exec <name> -- bash -lc 'curl -s http://127.0.0.1:8787/stats'
# force the proxy to (re)start the way the SessionStart hook does:
sbx exec <name> -- ~/.local/bin/headroom init hook ensure --profile init-user --marker headroom-init-claude
```

## Disable

Remove the kit from `sbxw.toml`, then in the sandbox:

```sh
headroom unwrap claude 2>/dev/null || true
pkill -f "headroom proxy" 2>/dev/null || true
```
