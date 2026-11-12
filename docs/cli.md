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
| `sbxw ls` | List sandboxes, their status, when each was created, and which are kept up with nobody attached. |
| `sbxw stop <names…> [--all]` | Stop sandboxes. State is kept; `sbxw up` restarts them. |
| `sbxw rm <names…> [--all]` | Remove sandboxes for good, and everything sbxw keeps about them. Passes `--force`, so it works even with a session attached. If one fails, the others are still removed and forgotten, and the failure is named. |
| `sbxw prune [--since D] [--dry-run] [-y]` | Remove every **stopped** sandbox, after listing them and asking. A running sandbox is never a candidate. `--since 168h` (alias `--until`) keeps anything stopped more recently. |
| `sbxw config grants [--config F] [--revoke]` | Show the host access approved for an `sbxw.toml`, and what it asks for that is not approved yet. `--revoke` forgets the approvals, so the next bring-up asks again. See [Approving host access](configuration.md#approving-host-access). |

`sbxw up` flags:

| Flag | Effect |
|---|---|
| `--ro DIR` | Mount an extra directory read-only (repeatable). |
| `--add-sandbox` | With no name, derive one from the workspace folder's name (`-copy`, `-copy-1`… on a clash with another path). |
| `--use-api-key` | Store `ANTHROPIC_API_KEY` as the global `anthropic` secret. See [Authentication](getting-started.md#authentication). |
| `--allow-host-access` | Approve the host access the config asks for (mounts, env files, kits, network hosts, ports, the Monitor pane's command, the listeners) as shown, without asking. For scripts: off a terminal, `sbxw up` otherwise fails with the list. See [Approving host access](configuration.md#approving-host-access). |
| `--no-web` | Run the pipeline in this process and attach the agent in this terminal. No daemon. |
| `--tail` | Follow the daemon log afterwards. |
| `--rebuild-template` | With `template = "auto"`: once this new sandbox is up and its kits have finished installing, save it as the template for its kits and remove the previous one. Without it, a missing or out-of-date template is rebuilt by the daemon in the background, in a sandbox of its own, while this one is created from sbx's image. On a sandbox that already exists it does nothing and says so: a template is only taken from a sandbox nobody has worked in yet. |

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

### Remote hosts (experimental)

See [Remote hosts](remote-hosts.md) for requirements, `trust_secrets` and what is untested. The registry of machines sbxw will drive over SSH, kept in `~/.sbxw/hosts.toml`. Not to be confused with `sbxw hosts` above (the `/etc/hosts` block). A host is either an alias from `~/.ssh/config` (`ssh`) or spelled out (`host`, with optional `user`, `port`, `key`). `key` is a path to an identity file, never a key or password. `local` is reserved; other names follow the sandbox name rules.

| Command | What it does |
|---|---|
| `sbxw host add <name> (--ssh <alias> \| --host <h> [--user U] [--port N] [--key PATH])` | Register a host. Flags: `--agent-port N` (default 7682), `--trust-secrets` (allow secrets to be sent there; default off), `--sbx-path PATH` (the `sbx` to run there; by default sbxw finds it with `sh -lc 'command -v sbx'` when it connects, and caches the answer for that connection). Fetches the host key with `ssh-keyscan`, shows its SHA256 fingerprint and asks; only on yes is it appended to `~/.ssh/known_hosts` (skipped if already known). `-y/--yes` skips the question for scripts. |
| `sbxw host ls` | Table of registered hosts. |
| `sbxw host test <name>` | Run `ssh … sbx version` on the host (or the `--sbx-path` given) (BatchMode, strict host key checking). Prints `OK` and the version, or the ssh error. |
| `sbxw host check <name> [--keep]` | Exercise ssh quoting, `sbx` detection, the ControlMaster link, `-L` and `-R`, the host's disk, kit copies and git on the host, in a temp dir that is removed (`--keep` leaves it). **Creates no sandbox**; needs no KVM. One line per step (`✓`, `✗` with the reason, or `-` skipped with the reason), a verdict, and what it cannot cover. Exit 0 only if nothing failed. `scripts/remote-smoke.sh <host>` runs `host test` then this. |
| `sbxw host provision <name> [--yes] [--skip-install] [--dry-run]` | Interactive, in your terminal (EXPERIMENTAL). Checks the host (Ubuntu 24.04+, x86-64/arm64, `/dev/kvm`, `kvm` group), installs `sbx` over `ssh -t` (sudo asks for its password there), signs in with a Docker PAT typed without echo and sent on stdin (never kept), sets the network policy with `sbx policy init`, then runs `host check`. `--yes` takes the safe defaults and never supplies the token; `--skip-install` requires sbx to be there; `--dry-run` runs only read-only probes and prints every command that would change the host. See [Provisioning](remote-hosts.md#provisioning). |
| `sbxw host edit <name> [flags]` | Change a host; takes the `add` flags plus `--no-trust-secrets`. Only the flags given change; `--ssh` and `--host` replace each other. A changed address asks about its host key again. |
| `sbxw host rm <name> [--force]` | Unregister a host. Refuses while `~/.sbxw/state/hosts/<name>/` is non-empty; `--force` unregisters anyway and leaves that state on disk. |

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
| `sbxw template save <name> <tag>` | Snapshot a sandbox that is already set up as a local template. Its agent token and Claude credentials are taken out first and put back afterwards. Set `template = "<tag>"` in `sbxw.toml` and new sandboxes start from it, with no registry pull and no kit reinstall. sbx may ask to stop the sandbox first. |
| `sbxw template ls` | List saved templates (`sbx template ls`). |
| `sbxw voice [status\|install\|cancel\|rm]` | The voice concierge's local models (`[voice]`): show which are downloaded, download them (~4 GB by default, asks first unless `-y`; resumable; sizes and sha256 from Hugging Face's listing) into `~/.sbxw/models`, stop a running download, or delete them. On Apple Silicon the install also has `uvx` fetch the speech runtime (`mlx-audio`), so [uv](https://docs.astral.sh/uv/) must be installed. The daemon downloads, so the web UI's Concierge panel shows the same progress. Dictation needs a build with `--features voice`. |
| `sbxw mac install\|up\|status\|pause\|stop\|force-off\|show\|release` | The [virtual Mac](../assets/terminal-mac/README.md): install the pinned terminal-mac (SHA-256 checked), start the VM (created and installing first if needed), show where it stands, pause it, shut it down normally (forced if it hangs), power it off at once (`force-off`), open its window, or take the lease back from a sandbox. |

### The daemon and sbxw itself

| Command | What it does |
|---|---|
| `sbxw doctor [--json]` | Check sbxw: the sbx version, `sbxw.toml`, the daemon, every local kit (v3 only since sbx 0.51) and `clipboard.textCopy`. Then summarise `sbx diagnose`: CLI and shim processes, the global network policy, the ZFS setting that slows image builds. Exits non-zero when a check fails. `--json` prints sbx's full report. |
| `sbxw logs [name] [-n N]` | Tail the daemon log (default 40 lines). With a name, only lines about that sandbox. |
| `sbxw down [name]` | Stop the daemon (closing every terminal it holds) **and** remove the `/etc/hosts` block. With a name, only close that sandbox's terminals. |
| `sbxw update [--check] [--no-island]` | Install the latest release over this binary. See [Updating](getting-started.md#updating). |
| `sbxw extension` | Write the Chrome/Edge extension folder, print how to load it, and print the sign-in address the panel needs after each daemon start. See [the side panel](chat.md#the-browser-side-panel). |
| `sbxw completion [shell]` | Print a completion script for bash, zsh, fish, elvish or powershell. |

The daemon's pid and log live in `~/.sbxw/run/`.

## Release signatures

A release is made by pushing a tag like `v3.4.0`. `.github/workflows/release.yml`
builds the binaries and the island, writes `sha256sums.txt` with one line per
file, and publishes them as a GitHub release. `install.sh` and `sbxw update`
download `sha256sums.txt` and refuse to install a file whose SHA-256 does not
match its line.

### What the checksums do not cover

The sums come from the same release as the files. Anyone who can replace a
release asset can replace `sha256sums.txt` to match. A signature over
`sha256sums.txt`, made with a key that never leaves the maintainer's control
and checked against a public key carried by the installer and the updater,
closes that gap.

### How signing works

- Scheme: OpenSSH signatures (`ssh-keygen -Y sign` and `ssh-keygen -Y verify`),
  Ed25519 key, namespace `sbxw-release`. `ssh-keygen` ships with macOS and
  with every Linux sbxw supports, so nothing new has to be installed.
- The release workflow signs `sha256sums.txt` into `sha256sums.txt.sig` when
  the repository secret `RELEASE_SIGNING_KEY` is set. Without it, the release
  still publishes, unsigned, and the run shows a notice.
- `install.sh` and `sbxw update` each carry the public key in a constant named
  `RELEASE_PUBLIC_KEY`. While it is empty they check sha256 only and print
  that release signatures are not enabled in this build. Once it is set, they
  download `sha256sums.txt.sig` and verify it before trusting any sum. A
  missing signature, a missing `ssh-keygen`, or a signature that does not
  verify stops the install or update before anything is written.
- `ssh-keygen -Y verify` needs OpenSSH 8.1 or newer (released in 2019). macOS 12
  and later, Ubuntu 20.04 and later, and Debian 11 and later all ship a newer
  version. Check with `ssh -V`.

Signing is off today: no key has been created, and both constants are empty.

### Turn signing on

Run these on your own machine, not in a sandbox: the private key must never
be on a machine an agent can read.

1. Generate the key pair (no passphrase, since the workflow must use it
   unattended):

   ```sh
   ssh-keygen -t ed25519 -N '' -C sbxw-release -f sbxw-release
   ```

   This writes `sbxw-release` (private) and `sbxw-release.pub` (public).

2. Store the private key as the repository secret `RELEASE_SIGNING_KEY`:

   ```sh
   gh secret set RELEASE_SIGNING_KEY --repo gallettom/sbxw < sbxw-release
   ```

   Or paste the whole file, including the `BEGIN` and `END` lines, under
   Settings, Secrets and variables, Actions.

3. Keep an offline copy of `sbxw-release` (for example on an encrypted USB key
   or in a password manager), then delete it from the disk:

   ```sh
   rm sbxw-release
   ```

4. Paste the single line from `sbxw-release.pub` (it looks like
   `ssh-ed25519 AAAAC3Nza... sbxw-release`) into both constants, exactly the
   same:

   - `install.sh`: `RELEASE_PUBLIC_KEY="ssh-ed25519 AAAAC3Nza... sbxw-release"`
   - `crates/cli/src/update.rs`: `const RELEASE_PUBLIC_KEY: &str = "ssh-ed25519 AAAAC3Nza... sbxw-release";`

   `cargo test -p sbxw` fails if the two differ.

5. Commit, merge to `main`, and release as usual (push a `v*` tag). In the
   run, the "Sign checksums" step prints `Signed with:` followed by the public
   key: it must match the line you pasted. The release then carries
   `sha256sums.txt.sig`.

Check a release by hand:

```sh
printf 'sbxw-release %s\n' "$(cut -d' ' -f1,2 sbxw-release.pub)" > allowed_signers
ssh-keygen -Y verify -f allowed_signers -I sbxw-release -n sbxw-release \
  -s sha256sums.txt.sig < sha256sums.txt
```

### Who verifies, and from when

- `install.sh` is fetched from `main`, so every install verifies from the
  moment the key is on `main` and the latest release is signed. Set the
  secret before you merge the key, and release right after: until a signed
  release exists, an installer with a key refuses the latest (unsigned) one.
- `sbxw update` verifies only in builds that carry the key. Installs made from
  a build with the empty constant keep checking sha256 only, until they update
  once to a build that has the key. Their next update after that is verified.

### What it protects against, and what it does not

It protects against a release, or one of its assets, being replaced by
someone who does not have the signing key: for example a GitHub account or
token that can edit releases, or a swapped download along the way.

It does not protect against:

- a compromise of the repository's secrets or of the release workflow, since
  whoever can run the workflow with the secret can sign anything;
- a compromise of the build itself (the source, a dependency, or the runner),
  since the signature only says the release workflow produced the sums;
- a change to `install.sh` on `main`, since the installer carries the key and
  is fetched from there. Protect `main` with branch rules and reviews.

### Rotate the key

To replace a key that is still safe:

1. Generate a new key pair as above.
2. Put the new public key line in both constants, and release that version
   while `RELEASE_SIGNING_KEY` still holds the old key. Updaters with the old
   key accept it, and from it they carry the new key.
3. Then set `RELEASE_SIGNING_KEY` to the new private key for every later
   release.

`sbxw update` always goes to the latest release. An install that skips the
transition release (step 2) has the old key and refuses releases signed with
the new one. It fails closed with a message, and re-running `install.sh`
fixes it. Leave time between steps 2 and 3 so most installs update through
the transition release.

There is a cost on the other side: between steps 2 and 3, `install.sh` on
`main` already carries the new key while the latest release is signed with
the old one, so fresh installs fail until the first release signed with the
new key. Pick the length of that window knowing both effects, and say it in
the release notes.

If the old key leaked, rotating this way is not enough, because whoever holds
it can also sign. Replace the secret, ship the new key, and tell users to
reinstall with `install.sh` from `main`.

## What `sbxw up` does

`sbxw up <name> [path]` runs this pipeline. Each step is one or more `sbx`
calls, and every step is safe to run again.

First, before the daemon is started and before any step, the host access
`sbxw.toml` asks for is checked against what you approved for that file. New
lines are printed and sbxw asks `Allow? [y/N]`; a no stops here with nothing
changed. The daemon checks again, so a file edited in between is asked about
too. See [Approving host access](configuration.md#approving-host-access).

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
   every kit goes through `sbx kit add` (this recreates the container, state
   kept; kits sbxw already applied, or that `sbx inspect` lists, are skipped). A kit sbx refuses
   to add needs a recreate, and sbxw says so. See
   [Kits](kits-and-skills.md).
5. **Shared code.** The workspace is mounted read-write: the agent's edits
   appear on the host instantly, and yours in the sandbox. **Only that folder is
   shared.** The sandbox is a microVM with its own filesystem, network and
   Docker daemon.
6. **Host aliases.** A marked block in `/etc/hosts` (plus `ifconfig lo0 alias`
   entries with `ip_per_app` on macOS) so `http://myapp.local:4200` works. These
   steps need `sudo`, so they run **in your terminal, before the daemon takes
   over**: a background process has nowhere to show a password prompt. Aliases
   are merged into the block, so a second sandbox doesn't evict the first.
   Anything the daemon can't write later (an alias added from the web UI) is
   shown as a warning and parked for `sbxw hosts sync`. On macOS the daemon
   first tries the system's own admin prompt (*osascript wants to make
   changes*).
7. **Ports.** A new sandbox is created with its mappings (`sbx create … -p …`),
   live from first boot. For a reused one, sbxw waits for `running` and
   publishes each with `sbx ports <name> --publish …`. Ports published later
   don't survive a stop, which is why this is automated. Mappings made at
   creation do, and a busy host port among them would stop the sandbox from
   starting: sbxw unpublishes that one before starting it, with a warning. If one host port is busy, sbx refuses
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
