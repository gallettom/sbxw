# sbxw

An ultra-light Rust wrapper around the standalone **`sbx`** (Docker Sandboxes)
CLI, tuned for running the **Claude Code** agent against a local project with a
browser terminal, host-friendly port aliases, and a restrictive dev network
policy.

It **only ever calls `sbx`** — never `docker sandbox`.

> Built and verified against the `sbx` 0.30 CLI reference
> (docs.docker.com/reference/cli/sbx). A few behaviours could not be confirmed
> from the docs and are flagged below — check them with `sbx … --help` on your
> machine before depending on them.

## What it does

`sbxw up <name> [path]` runs this pipeline (each step maps to an `sbx` call):

1. **Create** — if the sandbox doesn't exist:
   `sbx create claude <path> --name <name>` (extra `--ro DIR` mounts are appended
   as read-only workspaces, i.e. `DIR:ro`). `<path>` defaults to the current
   directory. If it already exists it's reused.
2. **Network policy** — applies a restrictive local-dev egress allowlist via
   `sbx policy allow network "<list>"` (npm, pypi, packagist, github, docker
   registries, `api.anthropic.com`). Not `**`. **Runs before kits** so a kit's
   download commands have egress.
3. **Kits** — applies each kit in `sbxw.toml`'s `kits = [...]` via `sbx kit add`
   (idempotent). See [Kits](#kits).
4. **Bidirectional code** — the workspace is the agent's Git working tree; edits
   from the agent appear on the host instantly and vice-versa. **Only that
   directory is shared** — the sandbox is a microVM with its own filesystem,
   network and Docker daemon, so nothing else on your host is exposed.
5. **Host aliases** — writes a delimited block in `/etc/hosts` (and, in
   `ip_per_app` mode on macOS, `ifconfig lo0 alias` entries) so you reach apps at
   `http://neos.local:4200` etc. Privileged steps use `sudo` and prompt.
6. **Ports** — once the sandbox is `running`, (re)publishes each mapping with
   `sbx ports <name> --publish …`. Ports are **not persistent** across a
   stop/restart, which is exactly why this is automated.
7. **Web terminal** — backgrounds a daemon serving a browser TTY (xterm.js)
   bridged over a WebSocket to a PTY. Each sandbox has two independent sessions:
   the **Claude** agent (`sbx run`) and a **Bash** shell (`sbx exec -it … bash`),
   switchable from the UI.

`sbxw up` prints the daemon pid + URL and detaches. Use `--tail` to follow its
log, or `--no-web` to attach the agent in the current terminal instead.

## Commands

| Command | What it does |
|---|---|
| `sbxw up [name] [path]` | Provision + serve. **Omit `name`** to start only the web daemon (browse/create/attach from the UI). |
| `sbxw bash <name>` | Open an interactive bash shell in a sandbox (foreground). |
| `sbxw web <name>` | Serve the web TTY only (no provisioning). |
| `sbxw ports <name>` | Re-publish the configured ports for a running sandbox. |
| `sbxw ports-ls [name] [--all]` | Show published port mappings for one or all sandboxes. |
| `sbxw ls` | List all sandboxes with status. |
| `sbxw stop <names…> [--all]` | Stop sandboxes (state kept; restartable). |
| `sbxw rm <names…> [--all]` | Remove sandboxes permanently. |
| `sbxw logs <name> [-n N]` | Tail a running daemon's log. |
| `sbxw down [name]` | Kill the daemon for `name`; with no name, kill all daemons **and** remove the `/etc/hosts` block. |

The web-only daemon's log/pid are keyed as `web` — `sbxw logs web`, `sbxw down web`.

## Web UI

Served at `http://sbxw.localhost:<port>` (default `7681`). From the browser you can:

- **Switch sandboxes** in the sidebar; connect, **stop**, **reload**, or **remove** (✕).
- **Create** a sandbox (＋) with a folder picker and inline **port-forwarding** rows
  (sandbox→host port, optional host IP, optional `/etc/hosts` alias). This goes
  through the *same* provisioning pipeline as the CLI.
- **View / add / remove port mappings** (⇌) per sandbox, including the host IP and alias.
- **Toggle Claude ↔ Bash** in the terminal bar — both sessions persist server-side,
  so switching back and forth keeps each one's scrollback and running process.

## Quick start

```bash
# one-time
sbx login
cp sbxw.toml.example sbxw.toml      # edit ports/aliases for your project
cargo build --release

# from your project root (e.g. the NEOS repo)
export ANTHROPIC_API_KEY=sk-ant-...        # optional, see Auth below
./target/release/sbxw up neos .            # or: sbxw up neos /path/to/repo
# open http://sbxw.localhost:7681  → talk to Claude in the browser

# …or just start the web daemon and create sandboxes from the UI:
./target/release/sbxw up
```

Install system-wide with `install.sh` (downloads a release binary + bundled
kits); set `REPO` in it first. The web UI is embedded in the binary.

Inside the sandbox, start your servers bound to **0.0.0.0** or the published
ports won't be reachable:

```bash
ng serve --host 0.0.0.0 --port 4200
symfony serve --listen-ip=0.0.0.0 --port=8000   # or php -S 0.0.0.0:8000
```

## Auth (read this — it's the gnarly bit)

`sbx run`/`create` have **no `--env`**, and there is **no "start without
attaching"** command. So an arbitrary env var (your `CLAUDE_OAUTH_TOKEN`) cannot
be injected *before* the agent launches. The wrapper offers three paths, best to
worst:

1. **API key (confirmed, recommended).** `sbxw up … --use-api-key` reads
   `ANTHROPIC_API_KEY` and stores it with `sbx secret set -g anthropic` (value
   piped via stdin, never in argv). The agent auto-authenticates.
2. **OAuth token (kit-based).** If `CLAUDE_CODE_OAUTH_TOKEN` (or
   `CLAUDE_OAUTH_TOKEN`) is set, sbxw generates an ephemeral **mixin kit** whose
   `initFiles` writes `~/.claude/.credentials.json` inside the sandbox, so the
   agent is authenticated from first launch (applied via `--kit` at create time,
   or `sbx kit add` on an existing sandbox). The canonical variable is
   `CLAUDE_CODE_OAUTH_TOKEN` (from `claude setup-token`); `CLAUDE_OAUTH_TOKEN` is
   accepted as an alias.
3. **Interactive.** Just run `/login` in the web terminal.

## Kits

Kits are `sbx`'s native, declarative extension point (tools, files, env, network,
startup commands). List them in `sbxw.toml`; they're applied **after** the
network policy on every `sbxw up`:

```toml
kits = [
  "/abs/path/to/sbxw/assets/k8s-tools",   # relative paths resolve against sbxw.toml
]
```

A kit reference is a **directory containing `spec.yaml`** (not a single `.yaml`),
a `.zip`, an OCI ref, or a git URL. Validate one with `sbx kit validate <dir>`.

Bundled: **`assets/k8s-tools`** installs `kubectl` + `k3d` + `skaffold` into
`~/.local/bin` (arch-aware, idempotent). It needs extra egress domains — see
`assets/k8s-tools/README.md`. Schema gotchas worth knowing: `startup` entries are
exec-style arrays (`command: ["bash", "…"]`), and `content` fields only allow the
`${WORKDIR}` placeholder — use brace-free `$VAR` for shell variables.

## Config (`sbxw.toml`)

See `sbxw.toml.example`. Key choice: `ip_per_app`.

- `false` (default): every app binds `127.0.0.1` on a distinct host port;
  `/etc/hosts` maps the alias to `127.0.0.1`. Reach it at `alias:host_port`.
- `true`: each app gets its own `127.0.0.X` loopback IP (added on `lo0` on
  macOS), so the alias resolves to a dedicated IP and you use the app's natural
  port — `http://neos.local:4200` with no remapping.

## Security notes

- Workspace mount is scoped to the single project directory; use `--ro` for
  anything the agent should not modify.
- The network policy is an explicit allowlist, never `**`. Tighten/loosen in
  `sbxw.toml`. You can audit live egress with `sbx policy log`.
- Secrets travel via **stdin**, not argv, so they don't appear in `ps`.
- `/etc/hosts` changes are confined to a marked block and removed by `sbxw down`.

## Unconfirmed against docs (verify locally)

- Exact column layout of `sbx ls` (used to detect existence / running state).
- Whether `sbx create` accepts the same positional `:ro` extra-workspace syntax
  as `sbx run` (documented for `run`; assumed identical for `create`).
- `sbx policy set-default` posture names (not used here; we use explicit
  `allow network`).

(The kit schema, once flagged as unconfirmed, is now verified — see [Kits](#kits).)
