# CLI reference

[← Back to README](../README.md)

- [Commands](#commands)
- [What `sbxw up` does](#what-sbxw-up-does)
- [The daemon](#the-daemon)

Every command takes `--help`. Commands that read the project config take
`--config <file>` (default `sbxw.toml` in the current directory).

## Commands

### Sandboxes

| Command | What it does |
|---|---|
| `sbxw up [name] [path]` | Create the sandbox if needed, provision it, and serve it from the daemon. `path` defaults to the current directory. **Without a name**, only start the daemon (create from the web UI). |
| `sbxw chat [name]` | A throwaway [chat sandbox](chat.md) on an empty folder. Without a name you get `chat-xxxxxx`. |
| `sbxw ls` | List sandboxes and their status. |
| `sbxw stop <names…> [--all]` | Stop sandboxes. State is kept; `sbxw up` restarts them. |
| `sbxw rm <names…> [--all]` | Remove sandboxes for good, and everything sbxw keeps about them. Passes `--force`, so it works even with a session attached. |
| `sbxw prune [--since D] [--dry-run] [-y]` | Remove every **stopped** sandbox, after listing them and asking. A running sandbox is never a candidate. `--since 168h` (alias `--until`) keeps anything stopped more recently. |

`sbxw up` flags:

| Flag | Effect |
|---|---|
| `--ro DIR` | Mount an extra directory read-only (repeatable). |
| `--add-sandbox` | With no name, derive one from the workspace folder's name (`-copy`, `-copy-1`… on a clash with another path). |
| `--use-api-key` | Store `ANTHROPIC_API_KEY` as the global `anthropic` secret. See [Authentication](getting-started.md#authentication). |
| `--no-web` | Run the pipeline in this process and attach the agent in this terminal. No daemon. |
| `--tail` | Follow the daemon log afterwards. |

`sbxw chat` takes `--no-web`, `--use-api-key` and `--tail` too.

### Terminals and access

| Command | What it does |
|---|---|
| `sbxw web <name>` | Open the web UI on that sandbox. Starts the daemon if needed; no provisioning. |
| `sbxw bash <name>` | An interactive bash shell in a running sandbox, in this terminal. |
| `sbxw ssh [name] [-- cmd…]` | SSH into `<name>.sbx`, or run one command there. `--setup` registers the SSH host block first. See [SSH](ssh.md). |

### Ports and hosts

| Command | What it does |
|---|---|
| `sbxw ports <name>` | Re-publish the configured ports of a running sandbox. |
| `sbxw ports-ls [name] [--all]` | Show published port mappings for one or all sandboxes. |
| `sbxw hosts [show\|sync\|clear]` | Show the `/etc/hosts` block (and anything the daemon parked), re-apply it (the one place `sudo` can ask for a password), or remove it. |

### Environment files

| Command | What it does |
|---|---|
| `sbxw env run [paths…]` | Bring a sandbox up from `sbxenv.yaml` files (merged in order, over `~/.sbxenv.yaml`). Flags: `--env-arg NAME=VALUE`, `-y/--yes` (pick free ports without asking), `--no-web`, `--use-api-key`, `--tail`. |
| `sbxw env export [path]` | Write an `sbxenv.yaml` beside the workspace, every path relative to the file. Flags: `-o FILE` (`-o -` prints it), `--name`, `--force`. |

See [Environment files](environment-files.md).

### Kits, skills and cache

| Command | What it does |
|---|---|
| `sbxw skills import [--dry-run] [--force]` | Import your host agents' skills into the store all sandboxes share. See [Shared skills](kits-and-skills.md#shared-skills). |
| `sbxw cache [show\|clear]` | Show or empty the bundled kits' download cache. See [The kit cache](kits-and-skills.md#the-kit-cache). |

### The daemon and sbxw itself

| Command | What it does |
|---|---|
| `sbxw logs [name] [-n N]` | Tail the daemon log (default 40 lines). With a name, only lines about that sandbox. |
| `sbxw down [name]` | Stop the daemon (closing every terminal it holds) **and** remove the `/etc/hosts` block. With a name, only close that sandbox's terminals. |
| `sbxw update [--check] [--no-island]` | Install the latest release over this binary. See [Updating](getting-started.md#updating). |
| `sbxw extension` | Write the Chrome/Edge extension folder and print how to load it. See [the side panel](chat.md#the-browser-side-panel). |
| `sbxw completion [shell]` | Print a completion script for bash, zsh, fish, elvish or powershell. |

The daemon's pid and log live in `~/.sbxw/run/`.

## What `sbxw up` does

`sbxw up <name> [path]` runs this pipeline. Each step is one or more `sbx`
calls, and every step is safe to run again.

1. **Create.** If the sandbox doesn't exist:
   `sbx create claude <path> --name <name>`, with `--ro` mounts appended as
   `DIR:ro`. Ports, kits, `skills`, `pull`, `[env]` and `env_files` all go in
   here, because creation is the only moment sbx applies them whole. An
   existing sandbox is reused.
2. **Agent tooling.** Once the sandbox reports `running`, one `sbx exec` pipes a
   Node script that: pre-trusts the workspace; installs the status and
   artifacts hooks, the usage `statusLine` (yours is kept if you set one), and
   the [relay](relay.md) MCP server with its `~/.claude/CLAUDE.md` section;
   removes a stale sbxw-written `model` key. `settings.json` and `.claude.json`
   are merged, not overwritten. A stopped sandbox is not waited for; its tooling
   is refreshed on the next `up`.
3. **Network policy.** `sbx policy allow network "<list>"` with the
   `network_allow` allowlist (npm, PyPI, Packagist, GitHub, Docker registries,
   `api.anthropic.com` by default; never `**`). sbxw's own rule,
   `localhost:<agent port>`, goes in the same call so hooks and the relay can
   reach the daemon's **agent listener** (never the UI port, see
   [Security](security.md#two-listeners)). An old `localhost:<UI port>` rule is
   revoked. This runs **before kits**, so a kit's downloads have egress.
4. **Kits.** A new sandbox got them at `sbx create --kit`. For an existing one,
   kits without startup commands go through `sbx kit add` (this recreates the
   container, state kept; kits already listed by `sbx inspect` are skipped).
   Kits with startup commands need a recreate, and sbxw says so. See
   [Kits](kits-and-skills.md).
5. **Shared code.** The workspace is mounted read-write: the agent's edits
   appear on the host instantly, and yours in the sandbox. **Only that folder is
   shared.** The sandbox is a microVM with its own filesystem, network and
   Docker daemon.
6. **Host aliases.** A marked block in `/etc/hosts` (plus `ifconfig lo0 alias`
   entries with `ip_per_app` on macOS) so `http://neos.local:4200` works. These
   steps need `sudo`, so they run **in your terminal, before the daemon takes
   over**: a background process has nowhere to show a password prompt. Aliases
   are merged into the block, so a second sandbox doesn't evict the first.
   Anything the daemon can't write later (an alias added from the web UI) is
   shown as a warning and parked for `sbxw hosts sync`. On macOS the daemon
   first tries the system's own admin prompt (*osascript wants to make
   changes*).
7. **Ports.** A new sandbox is created with its mappings (`sbx create … -p …`),
   live from first boot. For a reused one, sbxw waits for `running` and
   publishes each with `sbx ports <name> --publish …`. Ports don't survive a
   stop, which is why this is automated. If one host port is busy, sbx refuses
   the whole create; sbxw retries without the mappings and publishes them one by
   one, so only the busy port fails, with a warning. Free it (or change
   `host_port`) and run `sbxw ports <name>`.
8. **Web terminal.** The daemon serves xterm.js over a WebSocket to a PTY. Each
   sandbox has two sessions: the **Claude** agent (`sbx run`) and a **Bash**
   shell (`sbx exec -it … bash`).

## The daemon

There is **one daemon** per host. `sbxw up` is a client of it:

1. writes the `/etc/hosts` aliases in your terminal (where `sudo` can ask),
2. starts the daemon if nothing answers on `web_addr` (detached, logging to
   `~/.sbxw/run/daemon.log`),
3. hands it the bring-up and prints each step as the daemon reports it (the
   same steps the browser's progress card shows),
4. exits. The daemon owns the terminals and the port re-publishing from then on.

A second `sbxw up` for another sandbox talks to the same daemon.

With `--no-web`, the terminal belongs to the agent. Port-publishing reports,
which finish after `sbx run` has booted the sandbox, are held and printed when
the agent exits, so they don't scribble over its full-screen UI.
