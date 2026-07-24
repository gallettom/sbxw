# sbxw

An ultra-light Rust wrapper around the standalone **`sbx`** (Docker Sandboxes)
CLI, tuned for running the **Claude Code** agent against a local project with a
browser terminal, host-friendly port aliases, and a restrictive dev network
policy.

It **only ever calls `sbx`** — never `docker sandbox`.

> Built and verified against the `sbx` 0.35 CLI reference
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
3. **Kits** — applies each kit in `sbxw.toml`'s `kits = [...]` via `sbx kit add`.
   Since sbx 0.35 `kit add` **recreates the sandbox container** (state is
   preserved), so kits already listed by `sbx inspect` are skipped instead of
   blindly re-applied on every `up`. See [Kits](#kits).
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
| `sbxw chat [name]` | Throwaway chat sandbox: same as `up`, but on an empty workspace so the agent has none of your code. **Omit `name`** for a generated `chat-xxxxxx`. |
| `sbxw bash <name>` | Open an interactive bash shell in a sandbox (foreground). |
| `sbxw web <name>` | Serve the web TTY only (no provisioning). |
| `sbxw ports <name>` | Re-publish the configured ports for a running sandbox. |
| `sbxw ports-ls [name] [--all]` | Show published port mappings for one or all sandboxes. |
| `sbxw ls` | List all sandboxes with status. |
| `sbxw stop <names…> [--all]` | Stop sandboxes (state kept; restartable). |
| `sbxw rm <names…> [--all]` | Remove sandboxes permanently (passes `--force`, so removal proceeds even if a session is attached — sbx 0.35 refuses otherwise). |
| `sbxw logs <name> [-n N]` | Tail a running daemon's log. |
| `sbxw down [name]` | Kill the daemon for `name`; with no name, kill all daemons **and** remove the `/etc/hosts` block. |
| `sbxw update [--check] [--no-island]` | Install the latest release in place of this binary (or just check with `--check`). On macOS it also refreshes an already-installed `SbxwIsland.app` when the release ships a newer build of it — quitting and relaunching it if it was running; `--no-island` leaves the app alone. |
| `sbxw completion [shell]` | Print `source <(sbxw completion <shell>)` material for bash/zsh/fish/elvish/powershell; see `sbxw completion --help`. |

The web-only daemon's log/pid are keyed as `web` — `sbxw logs web`, `sbxw down web`.

## Web UI

Served at `http://sbxw.localhost:<port>` (default `7681`). From the browser you can:

- **Switch sandboxes** in the sidebar; connect, **stop**, **reload**, or **remove** (✕).
- **Create** a sandbox (＋) with a folder picker and inline **port-forwarding** rows
  (sandbox→host port, optional host IP, optional `/etc/hosts` alias). This goes
  through the *same* provisioning pipeline as the CLI.
- **Start a chat sandbox** (💬) — the browser equivalent of `sbxw chat`, with an
  optional name (leave it empty for the generated `chat-xxxxxx`). See below.
- **View / add / remove port mappings** (⇌) per sandbox, including the host IP and alias.
- **Toggle Claude ↔ Bash** in the terminal bar — both sessions persist server-side,
  so switching back and forth keeps each one's scrollback and running process.

## Chat sandboxes

Sometimes you just want to talk to an agent, not point it at a codebase. A chat
sandbox is a normal sandbox whose workspace is a **fresh empty directory**
(`/tmp/sbxw-chat/<name>`) instead of one of your projects, so the only files the
agent can see are the ones sbxw puts there itself — it has none of your code to
read or edit.

```bash
sbxw chat                 # throwaway sandbox with a generated chat-xxxxxx name
sbxw chat brainstorm      # ...or name it yourself
sbxw rm brainstorm        # removes the sandbox and its empty workspace
```

The web UI's 💬 button does the same thing (`POST /api/sandboxes/chat`); both go
through one shared code path. It opens a small dialog where the name is
optional — leave it empty and you get the same generated `chat-xxxxxx`, or type
one to get `sbxw chat brainstorm`'s result from the browser. The empty workspace
is deleted when the sandbox is removed, from either the CLI or the UI.

In the sidebar, chat sandboxes are clustered under a **💬 Chats** group, the same
way sandboxes sharing a workspace are grouped under their folder name. Each chat
has its own throwaway workspace, so path-based grouping can't catch them; the
API flags them with `chat: true` instead.

Everything else is a normal sandbox: a chat sandbox still reads your
`sbxw.toml`, so it applies the same kits and publishes the same `[[ports]]`. If
a project sandbox already holds one of those host ports, sbx's conflict recovery
gives the chat sandbox a different one.

## Dynamic Island (macOS)

An optional native companion app, **sbxw Island** (`macos/SbxwIsland`), turns
your Mac's menu bar / MacBook notch into a Dynamic-Island-style panel that keeps
track of every session — inspired by [vibeisland.app](https://vibeisland.app).
Each session shows a live state (**working** · **waiting for input** · **idle** ·
**ended**) with rich context: the last prompt you sent, the agent's current
activity, an agent tag, and elapsed time.

The notch shows a **persistent pill** hanging from it whenever something is
happening — an agent glyph, the active session's task, and a session count
(e.g. `👾 fix auth bug  3`) — and stays clean (hidden) when everything is idle.
On top of that:

- a **state change drops a toast** (full for 1 s, then a compact pill for 3 s,
  then it disappears back to the summary);
- when the agent **asks a question** (via Claude Code's `AskUserQuestion` tool),
  the notch expands into an **interactive card** showing the question, a
  decision table (each option's description), and a button per option
  (**⌘1/⌘2/…**) — picking one sends the answer straight into the session. A
  prompt with several questions is walked step by step (**1/2**, **⌘←** to go
  back) and submitted in one go once the last one is picked;
- **hovering** the notch reveals the full list — with each session's elapsed
  time and, at the top, your **Claude subscription usage** (5-hour and weekly
  window %) — auto-hiding 1 s after you leave.

**Subscription usage comes from Claude Code's own `statusLine`, not the OAuth
API.** sbxw installs a `statusLine` command (`assets/usage-statusline.js`) that
Claude Code invokes with a structured JSON payload on stdin (per its
[statusline contract](https://code.claude.com/docs/en/statusline)). Claude Code
fetches the `/usage` numbers itself; the script just forwards the
`rate_limits.{five_hour,seven_day}.used_percentage` it receives to the daemon
(`POST /api/usage`, throttled) — no OAuth token is reused out-of-band. Shown
only for Pro/Max sessions (API-key auth has no `rate_limits`), and only after a
session's first API response.

**Session state comes from Claude Code hooks, not terminal scraping.** At
provisioning time sbxw installs a small hook (`assets/status-hook.js`) into each
sandbox that POSTs every lifecycle event to the daemon over
`host.docker.internal`. This yields *trusted, structured* state — no guessing
from the terminal:

| Hook event | State |
| --- | --- |
| `SessionStart` | `idle` |
| `UserPromptSubmit` | `working` (captures your prompt) |
| `PreToolUse` (`AskUserQuestion`) | `attention` + structured prompt (every question of the call) |
| `PreToolUse` / `PostToolUse` (other) | `working` (tool as activity) |
| `Notification` | `attention` (permission / idle nudge) |
| `Stop` | `idle` |
| `SessionEnd` | `exited` |

It's powered by these daemon endpoints:

- `GET /api/sessions` — rich snapshot of current sessions.
- `GET /api/events` — a Server-Sent Events stream of rich session updates, one
  per hook-driven transition.
- `GET /api/hook/log` — the recent raw hook events (inspection/debugging).
- `GET /api/sandboxes` — polled so running sandboxes appear right away as `idle`.
- `POST /api/answer` / `POST /api/input` — send a menu choice (or raw bytes)
  back into a session's PTY.

Running sandboxes appear as `idle` immediately; live state flows as soon as the
in-sandbox agent emits hook events (the daemon must be reachable from the
sandbox at `host.docker.internal:<port>`, which sbxw allows automatically).
Build and usage instructions are in [`macos/README.md`](macos/README.md).

## Installation

**Prerequisites:** the standalone [`sbx`](https://docs.docker.com/reference/cli/sbx)
CLI on your `PATH` (`sbx version` should work), and `sbx login` done once.
Building from source also needs a Rust toolchain.

### Option A — install script (release binary)

Downloads the prebuilt binary for your OS/arch into `/usr/local/bin` and the
bundled kits into `~/.local/share/sbxw/kits`. The web UI is embedded in the
binary, so that's all you need.

```bash
curl -fsSL https://raw.githubusercontent.com/gallettom/sbxw/main/install.sh | sh
# pin a version:    | sh -s v1.0.0
# custom dir:       SBXW_INSTALL_DIR=$HOME/.local/bin   ... | sh
```

This requires a published [GitHub release](https://github.com/gallettom/sbxw/releases).
If there isn't one yet, use Option B.

### Option B — build from source

```bash
git clone https://github.com/gallettom/sbxw.git
cd sbxw
cargo build --release
# binary at ./target/release/sbxw — copy it onto your PATH if you like.
# /usr/local/bin is root-owned, so use sudo:
sudo install -m755 target/release/sbxw /usr/local/bin/sbxw
# …or install without root into ~/.local/bin (ensure it's on your PATH):
#   mkdir -p ~/.local/bin && install -m755 target/release/sbxw ~/.local/bin/sbxw
```

## Quick start

```bash
# one-time, in your project
sbx login
cp sbxw.toml.example sbxw.toml      # edit ports/aliases for your project

# from your project root (e.g. the NEOS repo)
export ANTHROPIC_API_KEY=sk-ant-...        # optional, see Auth below
sbxw up neos .                             # or: sbxw up neos /path/to/repo
# open http://sbxw.localhost:7681  → talk to Claude in the browser

# …or just start the web daemon and create sandboxes from the UI:
sbxw up
```

(If you built from source and didn't copy the binary onto your `PATH`, use
`./target/release/sbxw` instead of `sbxw`.)

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
2. **OAuth token.** If `CLAUDE_CODE_OAUTH_TOKEN` (or `CLAUDE_OAUTH_TOKEN`) is
   set, sbxw writes `~/.claude/.credentials.json` inside the sandbox so the
   agent is authenticated from first launch. On **create** and on existing
   **stopped** sandboxes this goes through an ephemeral **mixin kit** (`--kit`
   / `sbx kit add`); on a **running** sandbox the file is refreshed directly
   via `sbx exec` instead, because `sbx kit add` (0.35+) recreates the
   container and would kill attached sessions. The canonical variable is
   `CLAUDE_CODE_OAUTH_TOKEN` (from `claude setup-token`); `CLAUDE_OAUTH_TOKEN`
   is accepted as an alias.
3. **Interactive.** Just run `/login` in the web terminal.

Note: since sbx 0.35, host env vars are **no longer auto-injected** into
sandboxes at runtime. If you relied on an exported `ANTHROPIC_API_KEY` reaching
the sandbox by itself, that stopped working — use `--use-api-key` (which stores
it via `sbx secret set`) or migrate it with `sbx secret import`.

## Kits

Kits are `sbx`'s native, declarative extension point (tools, files, env, network,
startup commands). List them in `sbxw.toml`; they're applied **after** the
network policy on `sbxw up`. Since sbx 0.35, `sbx kit add` **recreates the
sandbox container** with the augmented kit set (state is preserved) and composes
the kit's own network allow/deny rules into the sandbox policy — so sbxw skips
kits that `sbx inspect` already lists, rather than re-applying them on every
`up`. To force a re-apply (e.g. after editing a kit), run
`sbx kit add <sandbox> <kit>` yourself:

```toml
kits = [
  "/abs/path/to/sbxw/assets/k8s-tools",   # relative paths resolve against sbxw.toml
]
```

A kit reference is a **directory containing `spec.yaml`** (not a single `.yaml`),
a `.zip`, an OCI ref, or a git URL. Validate one with `sbx kit validate <dir>`.

Bundled kits:

- **`assets/k8s-tools`** — installs `kubectl` + `k3d` + `skaffold` into
  `~/.local/bin` (arch-aware, idempotent).
- **`assets/headroom`** — installs [Headroom](https://github.com/chopratejas/headroom)
  (`headroom-ai[proxy]`), a local context-compression proxy, and enables its
  durable Claude Code integration (`headroom init --global claude`) to **cut token
  usage** (claimed 60–95% fewer tokens). See `assets/headroom/README.md`.
- **`assets/md-to-pdf-tools`** — ships the `/md-to-pdf` skill itself
  (user-level, works regardless of which project is mounted) plus the
  WeasyPrint + poppler-utils + Pillow stack it needs, so the skill is
  available and first invocation has no install step. See
  `assets/md-to-pdf-tools/README.md`.

Since sbx 0.35 the domains a kit declares under `network.allowedDomains` are
composed into the sandbox policy when the kit is added; domains a kit does *not*
declare (e.g. apt mirrors) still need adding to `sbxw.toml`'s `network_allow` —
see each kit's README. Schema gotchas worth
knowing: `startup` entries are exec-style arrays (`command: ["bash", "…"]`), and
`content` fields only allow the `${WORKDIR}` placeholder — use brace-free `$VAR`
for shell variables.

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
- Exact output format of `sbx inspect` (0.35+). sbxw only does a substring
  match on it to *skip* already-applied kits; if the kit name isn't found
  (older sbx, format change), the kit is simply re-applied as before.

(The kit schema, once flagged as unconfirmed, is now verified — see [Kits](#kits).)
