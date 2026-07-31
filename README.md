# sbxw

An ultra-light Rust wrapper around the standalone **`sbx`** (Docker Sandboxes)
CLI, tuned for running the **Claude Code** agent against a local project with a
browser terminal, host-friendly port aliases, and a restrictive dev network
policy.

It **only ever calls `sbx`** — never `docker sandbox`.

> The core pipeline was built against the `sbx` 0.35 CLI reference
> (docs.docker.com/reference/cli/sbx); everything sbxw has grown since needs
> **0.37 or newer**, which it checks at startup — see below. A few behaviours
> could not be confirmed from the docs and are flagged in the text — check them
> with `sbx … --help` on your machine before depending on them.

## Requires sbx 0.37.0 or newer

`sbxw up`, `sbxw chat` and `sbxw web` run `sbx version` first and stop with a
clear message if it's older. Two different requirements sit behind that number,
and the higher one wins because the check is there to prevent *surprises*, not
just crashes.

**0.35, or sbxw misprovisions.** These are the behaviours the pipeline is built
on:

- `sbx kit add` **recreates** the container, which is why sbxw skips kits
  `sbx inspect` already lists instead of re-applying them on every `up`;
- `sbx rm` refuses a sandbox with an attached session unless `--force` is passed,
  which sbxw always does — the web daemon holds a session;
- `sbx run --name` re-attaches a sandbox created with a custom kit (sbxw's OAuth
  kit) without re-passing it.

Older than that, these don't fail cleanly, they *misbehave*: a kit re-applied on
every `up`, an `rm` that refuses with no explanation.

**0.37, or half of sbxw quietly isn't there.** Ports published at creation
(`create -p`), `--no-share-skills`, `sbxw skills import`, `sbxw ssh --setup`, and
the network-policy panel's `policy ls <name> --wide` / `policy log`. Every one of
these degrades or is opt-in — sbxw even retries `create` without the mappings
when `-p` is rejected — which is exactly why the floor is here. Between 0.35 and
0.37 you get a working sandbox and a tool missing features it says it has,
explained only by scattered warnings.

There is deliberately **no upper bound**: newer sbx releases have added surface
rather than moved it, and the parts that did move (the policy panel's `ls
--wide` and `log`) already degrade view by view. A version sbxw can't parse is a
warning, not a refusal — the format of `sbx version` isn't sbxw's to veto.

To run against an older sbx anyway:

```bash
SBXW_SKIP_SBX_VERSION_CHECK=1 sbxw up neos
```

An environment variable rather than an `sbxw.toml` key on purpose: this is a
break-glass for one run, not a property of your project that should be committed
and forgotten.

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
6. **Ports** — a *new* sandbox is created with the mappings already attached
   (`sbx create … -p …`), so they're live from first boot. For a reused one,
   sbxw waits until it reports `running` and then (re)publishes each mapping
   with `sbx ports <name> --publish …`. Ports are **not persistent** across a
   stop/restart, which is exactly why this is automated.
   Publishing at creation is all-or-nothing — sbx 409s the whole request if one
   host port is already bound (a dev server you left running on 4200 is enough).
   sbxw won't lose the sandbox over that: it retries the create without the
   mappings and lets the per-port publishing take over, where a conflict is a
   warning naming that one port. Free the port (or change `host_port`) and run
   `sbxw ports <name>` to pick it up.
7. **Web terminal** — backgrounds a daemon serving a browser TTY (xterm.js)
   bridged over a WebSocket to a PTY. Each sandbox has two independent sessions:
   the **Claude** agent (`sbx run`) and a **Bash** shell (`sbx exec -it … bash`),
   switchable from the UI.

`sbxw up` prints the daemon pid + URL and detaches. Use `--tail` to follow its
log, or `--no-web` to attach the agent in the current terminal instead.

With `--no-web` the terminal belongs to the agent, so step 6's port publishing —
which by design finishes *after* `sbx run` has booted the sandbox — doesn't write
to it. Its reports are held and printed when the agent exits. Otherwise they
landed on top of the agent's full-screen UI, and in raw mode (no `ONLCR`) each
newline dropped a line without returning to column 0, stepping the text
diagonally across the screen.

## Commands

| Command | What it does |
|---|---|
| `sbxw up [name] [path]` | Provision + serve. **Omit `name`** to start only the web daemon (browse/create/attach from the UI). |
| `sbxw chat [name]` | Throwaway chat sandbox: same as `up`, but on an empty workspace so the agent has none of your code. **Omit `name`** for a generated `chat-xxxxxx`. |
| `sbxw bash <name>` | Open an interactive bash shell in a sandbox (foreground). |
| `sbxw ssh [name] [-- cmd…]` | SSH into a sandbox as `<name>.sbx`, or run one command in it. `--setup` registers the SSH host block first. See [SSH](#ssh-experimental). |
| `sbxw skills import [--dry-run] [--force]` | Import your host agents' skills into the store shared by all sandboxes. See [Shared skills](#shared-skills). |
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
- **Inspect the network policy** in that same panel. Three `sbx` calls, because
  no single one answers "what can this sandbox reach?":
  - **Rules** (`sbx policy ls <name> --wide`) — one row per rule with the
    resource it covers, i.e. the actual domains, `allow`/`deny` colour-coded.
    Comes with a filter box, since a global policy runs to a couple of hundred
    rules.
  - **Recent egress** (`sbx policy log <name>`) — the hosts that were actually
    allowed or blocked, with the rule and reason. The layer that answers "why was
    that request refused?".
  - **Policies governing this sandbox** (`sbx policy ls <name>`), folded away:
    one card per policy with its source (`local` / `kit` / `org`) and rule
    counts. A policy scoped to `all` gets a dashed border.
  - plus the **domains sbxw allows on up**, from `sbxw.toml`.

  The sandbox is a *positional* argument to `policy ls`/`log` (unlike
  `policy allow`'s `--sandbox` flag). If a call fails, only its own section goes
  — an older sbx without `--wide` still gets you the policy cards. If sbx prints
  something sbxw can't parse as a table, you get its output verbatim rather than
  a misleading empty list, and rows belonging to other sandboxes are filtered out
  with a count of what was hidden.
- **Add and remove network rules** from that panel:
  - the form under the rules takes sbx's own resource syntax
    (`example.com`, `*.acme.dev`, `host:443`, comma-separated) with an
    allow/deny selector. Scoped to this sandbox by default; tick **all
    sandboxes** to write it to the host-wide policy instead — that one asks for
    confirmation, since it governs every sandbox including ones created later.
    Runs `sbx policy allow|deny network [--sandbox <name>] <resources>`.
  - **✕ on a rule** removes it via `sbx policy rm <rule-id>`, after a
    confirmation that names the rule's blast radius. The id comes from the
    listing's *rule-id* column specifically — a policy id would delete every rule
    in that policy — so the button only appears when sbx reports one. Rules from
    an `org` source show 🔒 instead: governance, which sbx won't let you remove.

  Rules changed here are **not** written back to `sbxw.toml`, so the next
  `sbxw up` re-applies the configured allowlist over the top. For a permanent
  change, edit `network_allow` / `network_deny` there.

  > `sbx policy rm`'s argument shape is inferred from its help text, not verified
  > against a live run. If it turns out to differ, the button surfaces sbx's own
  > error verbatim — it can't remove the wrong thing, since it only ever passes a
  > rule id.
- **Toggle Claude ↔ Bash** in the terminal bar — both sessions persist server-side,
  so switching back and forth keeps each one's scrollback and running process.
  **Bash** normally attaches with `sbx exec`, which only reaches a *running*
  sandbox; on a stopped one it connects over SSH instead, since that starts the
  sandbox on the way in. (Previously it just failed, and you had to attach the
  agent first purely to boot the thing.) That fallback needs
  [SSH](#ssh-experimental) set up — the pane tells you so if it isn't.
- **Copy the SSH command** (SSH button in the terminal bar) for the attached
  sandbox — `ssh <name>.sbx`, ready to paste into a terminal or a remote-dev tool.
- **Open the host monitor** (the screen icon in the sidebar header) in the
  focused pane: sbx's own all-sandboxes
  dashboard, run in a PTY and streamed to the browser like any other pane, so a
  full-screen TUI works as it does in a terminal. It is *not* a sandbox session —
  it runs on the host, is shared by every viewer, and is filed under a
  pseudo-sandbox (`__host__`) that no real name can collide with, since sandbox
  names may not contain underscores. The Claude/Bash toggles and the SSH button
  hide for it: there is no sandbox behind that pane.

  Clicking it again puts the pane back on the sandbox it took over.

  What it runs is `monitor_cmd` in `sbxw.toml`, as argv — **one fixed configured
  command, deliberately not a "run anything on the host" box**. The default is
  bare `["sbx"]`: with no subcommand the CLI opens its own dashboard. Set it to
  `[]` and the button disappears.

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
  window %) — auto-hiding 1 s after you leave (5 s with a row open, so a reply
  can be finished after the pointer wanders off). The reveal target is **the
  notch itself**, not the whole bubble: the collapsed island hangs 280 pt wide
  over the menu bar of whatever app you are in, and crossing the top edge on the
  way to *that* app's menus used to unfold it in your face. Aim at the notch and
  it opens; pass beside it and it stays put. It is also **click-through until it
  has something to click** — the collapsed pill and the toasts announce, they
  don't offer, so clicks fall through to the menu bar underneath; only the list
  and a question card take the pointer. And a **click anywhere else retracts the
  open list at once** — the timer is for a pointer that wandered, a click is a
  decision. (A question card is exempt: it stays until answered or dismissed with
  its ✕, so a stray click can't lose a prompt.) A click-through window receives no
  mouse events at all, so both the reveal and that retraction are driven by
  `NSEvent` monitors rather than by the view's own hover. Mouse monitoring needs
  no Accessibility permission — only keyboard monitoring does;
- every row carries a **chevron** that opens it: Claude's **full reply** and a
  **field to write back into that sandbox**, without a browser tab or a terminal.
  See below;
- a **＋ New chat** row at the bottom of that list starts a *fresh* throwaway
  chat agent without opening a browser or picking a workspace. See below;
- each row waiting on you carries a **✕** to dismiss it, and a **Clear all N**
  strip appears above the list once more than one is pending — dismissing is what
  takes a session off the collapsed notch, and opening a sandbox counts as
  dismissing it;
- once a turn ends, every surface that captions that session — the row, the pill
  under the notch, the mini toast — carries **what Claude actually answered**
  rather than "idle" or the prompt you sent; **opening the row** shows the reply
  in full. The prose comes from the
  `last_assistant_message` Claude Code puts on its own `Stop` event — no
  transcript reading, and it arrives even through a hook script installed before
  this feature existed. A **structured question** still outranks it — that text is
  what you have to act on — but a session nudging you *about* an answer it already
  gave shows the answer. Nothing stale can leak through: a new prompt clears the
  reply, so a session only carries one if a turn has ended since you last spoke.

### Chatting from the notch

Two gestures, and the difference between them is the point:

- **Open a row** (the chevron) to write into a sandbox that already exists. The
  drawer shows Claude's full reply and a field under it; sending keeps the field
  open, because the answer arrives right above it.
- **＋ New chat** starts a *new* throwaway sandbox — `ephemeral-chat`, then
  `ephemeral-chat-2`, `-3`, … — numbered by availability, so a name freed with
  `sbxw rm` is offered again rather than the counter climbing forever. Carrying
  on an existing conversation is the row drawer's job, one click away on that
  chat's own row.

Because every ＋ costs a container, the composer **warns from four sandboxes up**
that each one holds disk and memory, and points at the two ways out (reply in a
chat you already have, or remove what you're done with). Sandboxes are cheap to
make and not free to keep.

Both gestures are the same daemon call, `POST /api/chat/push` — the island has no
sandbox picker and no terminal, so everything between "you typed a question" and
"the agent is reading it" happens server-side: provision if missing, attach the
agent, wait for its TUI, type, submit. Three details that are easy to get wrong:

- **It waits for the terminal to go quiet before typing**, and again before
  pressing Return. Quiescence is measured on the session's output stream, not
  the replay ring buffer — that buffer is capacity-bounded, so once full its
  length stops changing and silence becomes indistinguishable from a flood.
- **Return has to arrive as its own keystroke.** Claude Code reads a burst of
  closely-spaced bytes as a *paste*, and a newline inside a paste is inserted
  into the message instead of sending it — the text lands in the box and just
  sits there.
- **A warm session is not charged for a cold start.** Typing into a sandbox whose
  agent is already attached and drawn skips the `sbx ls` existence probe (the
  live PTY is the proof) and times silence in ~180 ms rather than the 900 ms a
  first frame needs — the wait that made a second message feel as slow as the
  first. To stay honest at a short window it waits for the echo to *start* before
  timing its silence: a PTY that hasn't turned the write around yet reads as
  quiet, and Return would join the paste. The echo window itself stays looser
  (350 ms), since a long message comes back in chunks.

Creating a chat is still slow (a sandbox has to boot); the composer shows a
spinner, and keeps your text on failure so it can be retried.

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
- `POST /api/chat/push` — `{ "text": "…", "name"?: "sandbox", "fresh"?: true }`:
  submit a message to a chat agent, creating the sandbox and attaching its
  session first if needed. `name` types into that sandbox; `fresh` mints the next
  free `ephemeral-chat[-N]`; neither falls back to the shared `ephemeral-chat`
  (what older island builds send). See
  [Chatting from the notch](#chatting-from-the-notch).

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
# leave SSH alone:  SBXW_SETUP_SSH=0                    ... | sh
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

## SSH (experimental)

Sandboxes can be SSH targets. Register the host block once — sbx writes a
managed `Host *.sbx` entry into your SSH config — then every sandbox answers at
`<name>.sbx`:

```bash
sbxw ssh --setup              # one-time (wraps `sbx setup ssh`)
sbxw ssh neos                 # interactive shell
sbxw ssh neos -- git status   # one-shot command
```

`install.sh` does this for you. An interactive run asks (defaulting to yes); a
piped one — `curl … | sh`, which is how most people install — has nobody to ask
and applies that same default, since otherwise the SSH button in the web UI and
`sbxw ssh` would fail for the majority of installs, with the fix buried in a line
of installer output nobody reads. What gets written is a managed, sbx-owned
`Host *.sbx` block, which matches no host you already have.

Set `SBXW_SETUP_SSH=0` to leave your SSH config untouched, or `SBXW_SETUP_SSH=1`
to configure it without being asked. `sbxw ssh --setup` is the catch-up path if
you declined or installed the binary by hand; it's idempotent, so re-running it
is harmless.

Two things this gives you that `sbxw bash` doesn't:

- **It starts things for you.** The connection brings up the sbx daemon *and* the
  target sandbox on demand, so `sbxw ssh` works against a stopped sandbox.
- **Remote development.** Any OpenSSH-compatible tool can attach — VS Code,
  Cursor, Claude Desktop, ChatGPT:

  ```bash
  code --remote ssh-remote+neos.sbx /workspace
  ```

If the connection fails and no `*.sbx` entry is found in `~/.ssh/config`, sbxw
says so and points you at `--setup` rather than leaving you with a bare
`Connection refused`. SSH access is experimental and may need enabling in your
sbx installation first.

## Shared skills

sbx keeps a **persistent skill store shared across sandboxes**, separate from
this repo's kits. `sbxw skills import` fills it from the agents installed on
your host:

```bash
sbxw skills import --dry-run   # preview what would be imported
sbxw skills import             # do it
sbxw skills import --force     # ...replacing skills already in the store
```

Imported skills survive `sbxw rm` and are mounted read-write into new sandboxes.
Set `share_skills = false` in `sbxw.toml` to create sandboxes without the store
(passes `--no-share-skills`), e.g. for a sandbox that should only ever see the
skills its own kits provide. It's read at **creation** only — flipping it does
nothing to sandboxes that already exist.

How this relates to [kits](#kits): a kit can install *anything* (apt packages,
binaries, startup commands) but applying one recreates the container. The skill
store only carries skill files, and costs nothing to update. So `md-to-pdf-tools`
still has to be a kit — the skill needs WeasyPrint and poppler underneath it —
but a skill with no system dependencies belongs in the store instead.

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
- When a `sbx` call fails, sbxw now folds **what sbx actually said** into the
  error instead of reporting a bare exit status — so the structured
  `Blocked by network policy` explanation (rule / origin / detail), and the
  support message an organisation attaches to a governance denial, reach you
  in the CLI and the web UI rather than dying in the daemon log.
- **Behind a corporate proxy**, a blocked download is often not sbxw's
  allowlist. `DOCKER_SANDBOXES_PROXY=system` routes sandbox egress through the
  host OS proxy configuration (macOS/Windows), PAC URL included — try that
  before widening `network_allow`. `sbx policy log` shows the real reason; an
  `origin: corporate policy` line means `sbx policy allow` won't help.
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
- The flags taken from the newer release notes but not yet checked against a
  live `sbx --help`: `sbx create -p/--publish`, `sbx create --no-share-skills`,
  `sbx skills import [--dry-run|--force]`, and `sbx setup ssh`. Each has a
  fallback if the flag turns out to be spelled differently — `create` failing
  means `sbxw up` fails loudly rather than silently mis-provisioning, and the
  port publishing is still done by the provisioning thread regardless.

(The kit schema, once flagged as unconfirmed, is now verified — see [Kits](#kits).)
