# Getting started

[← Back to README](../README.md)

- [Requirements](#requirements)
- [Install](#install)
- [Your first sandbox](#your-first-sandbox)
- [Authentication](#authentication)
- [Optional extras](#optional-extras)
- [Updating](#updating)

## Requirements

- The standalone [`sbx`](https://docs.docker.com/reference/cli/sbx) CLI on your
  `PATH`, **version 0.48.0 or newer**. Check with `sbx version`.
- `sbx login` done once.
- To build from source: a Rust toolchain.

`sbxw up`, `chat`, `web`, `env run` and `prune` run `sbx version` first. They
stop with a clear message if `sbx` is too old. sbxw has no fallbacks for older
releases ([why](architecture.md#the-sbx-version-floor)). To try an older `sbx`
anyway, for one run:

```bash
SBXW_SKIP_SBX_VERSION_CHECK=1 sbxw up neos
```

Expect hard failures, not degraded behaviour: since sbx 0.39, an unknown flag
fails the whole command.

## Install

### Option A: install script (release binary)

```bash
curl -fsSL https://raw.githubusercontent.com/gallettom/sbxw/main/install.sh | sh
```

Options, passed to the same command:

| Option | Example |
|---|---|
| Pin a version | `… \| sh -s v2.1.2` |
| Custom directory | `SBXW_INSTALL_DIR=$HOME/.local/bin … \| sh` |
| Leave your SSH config alone | `SBXW_SETUP_SSH=0 … \| sh` ([SSH](ssh.md)) |

The script installs the binary (default `/usr/local/bin`) and writes the
[browser extension](chat.md#the-browser-side-panel) folder. On macOS it offers
the [Dynamic Island](island.md) app. The web UI and the
[bundled kits](kits-and-skills.md) are inside the binary.

It needs a published [GitHub release](https://github.com/gallettom/sbxw/releases).
If there is none yet, use Option B.

### Option B: build from source

```bash
git clone https://github.com/gallettom/sbxw.git
cd sbxw
cargo build --release
sudo install -m755 target/release/sbxw /usr/local/bin/sbxw
# or, without root:
# mkdir -p ~/.local/bin && install -m755 target/release/sbxw ~/.local/bin/sbxw
```

## Your first sandbox

From your project's root:

```bash
sbxw up neos .        # or: sbxw up neos /path/to/repo
```

Then open <http://sbxw.localhost:7681>. The Claude agent is in the pane.

What just happened: sbxw created the sandbox, installed its hooks and tools,
applied the network allowlist, wrote any `/etc/hosts` aliases (asking for
`sudo` in your terminal if needed), published ports, and handed everything to a
background daemon that serves the browser terminal. The full pipeline is in the
[CLI reference](cli.md#what-sbxw-up-does).

**No name?** `sbxw up` alone starts the daemon only. Create sandboxes from the
browser with **＋**.

**Dev servers** inside the sandbox must listen on `0.0.0.0`:

```bash
ng serve --host 0.0.0.0 --port 4200
symfony serve --listen-ip=0.0.0.0 --port=8000   # or php -S 0.0.0.0:8000
```

**Ports and hostnames** come from an `sbxw.toml` at the project root. Start from
[`sbxw.toml.example`](../sbxw.toml.example):

```toml
[[ports]]
alias = "neos.local"
sandbox_port = 4200
host_port = 4200
```

Now `http://neos.local:4200` reaches the dev server. See
[Configuration](configuration.md).

**Stopping:**

```bash
sbxw stop neos    # stop the sandbox, state kept
sbxw rm neos      # remove it for good
sbxw down         # stop the daemon and remove the /etc/hosts block
```

A sandbox also stops by itself once nobody is attached. Set
`keep_running = true` to keep it up for a dev server.

## Authentication

Host environment variables do **not** reach sandboxes by themselves. Pick one:

1. **API key (recommended).** `sbxw up … --use-api-key` reads
   `ANTHROPIC_API_KEY` and stores it as sbx's global `anthropic` secret
   (`sbx secret set anthropic`, value piped over stdin, never on a command
   line). The agent signs in on its own.
2. **OAuth token.** Set `CLAUDE_CODE_OAUTH_TOKEN` (from `claude setup-token`;
   `CLAUDE_OAUTH_TOKEN` also works) before `sbxw up`. sbxw writes
   `~/.claude/.credentials.json` into the sandbox so the agent is signed in from
   its first launch. Set `claude_subscription` in `sbxw.toml` to your plan so
   the in-session label is right.
3. **Interactive.** Run `/login` in the agent's pane.
4. **Remote Control.** Tick *Remote control* in the create dialog (or
   `sbxw up … --remote-control`). Remote Control refuses the setup token and an
   API key, so sbxw gives this sandbox neither — no OAuth kit, no auth variable
   from `env` — and allows the login hosts. Run `/login` in its pane once, then
   press **⇄ Remote** in the pane bar: sbxw types `/remote-control` and Claude
   prints the claude.ai/code link. The button reads **On** while Remote Control
   is connected in that pane's session (read from Claude Code's session
   registry in the sandbox, every 15 s); it is then only an indicator — turn
   Remote Control off with `/remote-control` in the session, or from claude.ai. On a sandbox already signed in with the
   token, Remote offers to switch it: sbxw rewrites its OAuth kit without the
   token (sbx re-reads the kit on every container swap, so the old one would
   put the token back), stops injecting it, and types `/login`. A global
   `anthropic` secret (`--use-api-key`) is sbx's, for every sandbox; remove it
   if the agent signs in with it instead.

   **On a Mac, sbxw keeps the host awake** while Remote Control is on in at
   least one session (`caffeinate -i -s`, tied to the daemon). On battery at or
   below `keep_awake_min_battery` (5 %) it lets go, so a forgotten session never
   drains the Mac flat. Below `battery_warn_below` (20 %) the agent is told on
   your next prompt, and passes it on, since claude.ai does not show the Mac's
   battery. A closed lid on battery still sleeps whatever is held; for that,
   plug in, or see `pmset disablesleep`. `keep_awake = "always"` or `"off"`
   changes the rule.

<details>
<summary>How the OAuth token gets in</summary>

On creation, and on a stopped sandbox, the credentials file goes in through a
generated **mixin kit** (`sbx create --kit` / `sbx kit add`). On a running
sandbox it is written directly with `sbx exec`, because `sbx kit add` recreates
the container and would kill attached sessions.

That kit lives at `~/.sbxw/state/kits/<name>-oauth/` (`0600` file, `0700`
directory) for as long as the sandbox exists. `sbxw rm` deletes it. It cannot be
a temporary directory: sbx re-resolves every kit by its original path on each
container swap, so a missing kit directory breaks every later `sbx kit add`,
with an error naming the wrong kit:

```text
ERROR: re-resolve original kit 0 ("/var/folders/…/sbxw-oauth-kit-36226"):
       kit reference "…": path does not exist
```

Sandboxes created by an older sbxw may still carry such a reference. `sbxw up`
recognises this exact failure, restores the kit at the path sbx asks for, and
retries once.

</details>

For non-secret variables, use `[env]` in `sbxw.toml`
([Configuration](configuration.md#environment-variables)). To move existing
secrets into sbx, use `sbx secret import`.

## Optional extras

| Extra | How |
|---|---|
| Shell completion | `source <(sbxw completion zsh)` (bash, zsh, fish, elvish, powershell) |
| SSH and remote editors | `sbxw ssh --setup` once. See [SSH](ssh.md) |
| Chrome / Edge side panel | `sbxw extension`, then *Load unpacked*. See [Chat](chat.md#the-browser-side-panel) |
| macOS notch companion | Offered by the installer. See [Dynamic Island](island.md) |
| Your host's skills in every sandbox | `sbxw skills import`. See [Kits and skills](kits-and-skills.md#shared-skills) |

## Updating

```bash
sbxw update            # install the latest release over this binary
sbxw update --check    # only check
```

`sbxw update` also refreshes the browser extension folder. On macOS it updates
an installed `SbxwIsland.app` when the release has a newer build, and relaunches
it if it was running. `--no-island` leaves the app alone.

The web UI shows both versions, `sbx` and `sbxw`, at the foot of the sidebar.
