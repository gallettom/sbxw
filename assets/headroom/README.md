# headroom kit

Installs [Headroom](https://github.com/chopratejas/headroom) — a local
context-compression proxy — inside a sandbox and routes the Claude agent through
it to **cut token usage (the project claims 60–95% fewer tokens, same answers)**.

## How it works

On every container start, the kit's startup script:

1. Finds a Python ≥ 3.10 (Headroom's requirement).
2. Installs Headroom once via `pip install --user "headroom-ai[all]"` (→ `~/.local/bin`).
3. Starts the compression proxy: `headroom proxy --port 8787` (background).
4. **Health-checks the proxy** (TCP connect). Only if it's up does it set
   `ANTHROPIC_BASE_URL=http://localhost:8787` in the agent's env files.

The proxy uses **Authorization-header passthrough** — it forwards Claude's
existing credentials to `api.anthropic.com`, so it works with both the API-key
and OAuth auth paths. No separate key needed.

### Why the health check matters

`ANTHROPIC_BASE_URL` is only wired **when the proxy answers**. If the install or
proxy fails, the script *removes* the override, so Claude falls back to direct
API access instead of breaking. It self-heals on the next start.

## Usage

```toml
# sbxw.toml — keep above the first [[ports]] table
kits = [
  "/abs/path/to/sbxw/assets/headroom",
]
```

`ANTHROPIC_BASE_URL` is read by Claude Code **at launch**, so after the first
install you must restart the agent (Reload in the web UI, or `sbxw down <name> &&
sbxw up <name>`) for compression to take effect.

## Required network allowlist

Add to `network_allow` in `sbxw.toml` (sbxw applies the policy before the kit):

```toml
network_allow = [
  # ...defaults (pypi.org, *.pythonhosted.org already cover the pip install)...
  "huggingface.co", "cdn-lfs.huggingface.co",  # Headroom's text compressor model
]
```

`api.anthropic.com` is already allowlisted (the proxy forwards there).

## Verify / debug

```sh
sbx exec <name> -- bash -lc 'headroom --version; pgrep -af "headroom proxy"; echo $ANTHROPIC_BASE_URL'
sbx exec <name> -- tail -n 40 /tmp/headroom-proxy.log
```

## Disable

Remove the kit from `sbxw.toml` and, in the sandbox, strip the env block:

```sh
sed -i '/>>> sbxw headroom >>>/,/<<< sbxw headroom <<</d' ~/.bashrc ~/.profile
pkill -f "headroom proxy"
```
