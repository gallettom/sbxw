# Architecture

[← Back to README](../README.md)

- [Crates](#crates)
- [Tests](#tests)
- [The sbx version floor](#the-sbx-version-floor)
- [Unconfirmed against sbx docs](#unconfirmed-against-sbx-docs)

## Crates

A Cargo workspace of four crates, with dependencies pointing one way:

```
crates/core    sbxw-core    the rules: typed names and session keys, sbxw.toml,
                            the relay / plan / code-map state machines, the hook
                            state of agent sessions, every message typed into an
                            agent, the parsers for what sbx prints. No processes,
                            no sockets — most tests live here.
crates/sbx     sbxw-sbx     the only door to `sbx`: one method per command, a
                            pure argv builder and parser each, run through a
                            Runner trait (tokio::process, or a scripted fake).
crates/daemon  sbxw-daemon  one App value holding every service (no globals):
                            the store (one record per sandbox), one event bus,
                            PTY sessions, the bring-up pipeline, and the two
                            HTTP listeners (UI, agent).
crates/cli     sbxw         the binary; a client of the daemon.
assets/                     the web UI (served as-is), kits, in-sandbox scripts,
                            the browser extension.
macos/                      the Dynamic Island app (Swift).
```

State on the host lives under `~/.sbxw/`: `run/` (daemon pid and log),
`state/` (sandbox records, plans, threads, favourites, generated kits),
`kits/` (the bundled kits, written out), `cache/` (the kit cache) and
`browser-extension/`.

## Tests

```bash
cargo test --workspace   # unit tests
scripts/e2e.sh           # the built binary end to end, against scripts/fake-sbx, in a throwaway HOME
scripts/ui-smoke.sh      # the real web UI from that setup, in headless Chromium
```

`ui-smoke.sh` needs Playwright (`PLAYWRIGHT_MODULE` can point at an install) and
loads xterm from jsDelivr, or from `XTERM_VENDOR` offline.

## The sbx version floor

sbxw requires **sbx 0.50.0+** and has no fallbacks. It used to support 0.37 and
up behind runtime checks, but each check was a branch plus a fallback at every
call site, and every fallback was a code path nobody ran.

**0.39 is what makes this safe:** an unknown command or flag is now an error.
Before, it printed help and exited 0, so a probe that "worked" proved nothing.

What sbxw assumes, unconditionally:

| Feature | Used for |
| --- | --- |
| `create`/`run` `-e KEY=VALUE`, `--env-file` | `[env]` and `env_files` |
| `sbx env create --name`, `--env-arg` | `sbxw env run` |
| The 0.42/0.43 environment-file format | `sbxenv.yaml`, `${{ env.* }}`, file-relative paths, `skills:` |
| `create --skills=off\|readonly\|readwrite` | `skills` |
| `sbx prune --filter until=…` | `sbxw prune` |
| `policy rm network --id … --force` (0.45) | ✕ on a rule in the network panel |
| `run -d --name NAME` (0.47) | `keep_running` |
| `create --pull=always\|missing\|never` | `pull` |
| Kit spec v2 | the OAuth kit and the bundled kits |
| `secret set --sandbox NAME` | the Anthropic secret |
| `SANDBOX_NAME` | in-sandbox hooks naming their own sandbox |
| `create --kit` repeated | every kit applied at creation |
| `--format json` on `ls`, `ports`, `rm` (0.48) | sandbox state, live ports, per-target removal |
| `kit add` with startup commands (0.48) | kits added to an existing sandbox |
| `policy log --format json` (0.48) | blocked hosts in the network panel |
| `mcp ls --format json`, `mcp add\|rm\|inspect`, `mcp load\|unload --sandbox` (0.48) | the MCP dialog |
| `git-identity@1`, `ssh-agent@1` kit capabilities (0.48) | `sbxw:git-identity`, `sbxw:ssh-agent` |
| `template save`, `create --template` | `sbxw template save`, `template` |
| `policy refresh` (0.49) | **Re-read policy now** |
| `kit validate --offline` (0.49) | kit checks before create, `sbxw doctor` — v3 kits skipped: 0.50's validator cannot read them |
| `diagnose --format json` | `sbxw doctor` |
| `create --mount HOST:TARGET` (0.50) | `kit_mounts`, **Kit folders** in the create dialog |
| `mounts:` in environment files (0.50) | `sbxw env export` / `env run` |
| `volume@1` storage kept across `rm` (0.50) | `volumes` |
| `policy … network --method/--path/--query` (0.50) | **Only some HTTP requests** in the network panel |
| `sbx exec` exit status 125 (0.50) | "could not run" told apart from the command's own failure |

0.50 also changed what sbx provisions: subscription-OAuth Claude sandboxes
get no API-key helper. A sandbox created earlier may still carry an
`apiKeyHelper`, which outranks `/login`; the Remote Control button detects it
and says to recreate the sandbox. sbxw's own host calls follow 0.50's
hardening: host `git` reads pin off every program a repo's config can name
(`core.fsmonitor`, filters, hooks…), and host helpers (`osascript`, `sudo`,
`caffeinate`, `curl`…) are run from system directories, never looked up on
`PATH`.

Why each step of the floor moved:

- **0.43:** environment files changed shape in 0.42 and 0.43 (file renamed to
  `sbxenv.yaml`, no `${VAR}` expansion, no implicit workspace mount, paths
  relative to the declaring file, `--name` on every `sbx env` subcommand). No
  single file can straddle that. 0.42 also refuses sandbox names over 63
  characters or ending in a hyphen; sbxw checks this itself, in the CLI and the
  web UI.
- **0.47:** 0.45 made every removal ask for confirmation, and `policy rm network`
  only gained `--force` then. 0.47's `sbx run -d` is what `keep_running` uses.
- **0.50:** the security fixes (host Git, trusted helpers, MCP OAuth
  redirects, `sbx cp` symlinks) are reason enough not to run older, and
  `--mount`, `volume@1` retention, `--query` and exit 125 are used directly.
  The table fallbacks for `ls` and `ports` went with it. Note `sbx exec`
  *starts* a stopped sandbox: sbxw asks `is_running` first wherever waking
  one would be a side effect.
- **0.48:** every human-readable table changed (state marks, a creation-time
  column in `sbx ls`), so what sbxw parses it reads as `--format json`: `ls`
  (a bare array), `ports` (`host_ip`, `host_port`, `sandbox_port`,
  `protocol`) and `rm` (one result per target, failures included, so a partial
  removal forgets exactly what went). A piped answer no longer confirms a
  prompt; `secret set` refused for want of a confirmation is retried with
  `--force`. `kit add` accepts kits with startup commands.

Other behaviour sbxw follows: if `sbx ls --format json` fails or is not JSON,
the state is "could not tell", never "none"; a sandbox whose guest stopped responding is not treated as
running; sandboxes made with `sbx create` stop by themselves once idle (0.43).

MCP OAuth client secrets are stored as `mcp:<server>:client_secret` since 0.43.
One stored under the old `mcp:<server>.client_secret` name has to be set again;
sbxw stores no MCP secrets itself.

## Unconfirmed against sbx docs

Things sbxw relies on that the published reference doesn't pin down. Verify
locally if something looks off.

- **The exact column layout of `sbx ls`**, and the status field name in its
  JSON (`status`, else `state`), used to detect existence and state.
- **`sbx create --template TAG`.** The reference documents `sbx run -t TAG`;
  sbxw assumes `create` takes the same flag.
- **The shape of `sbx diagnose --format json`.** `sbxw doctor` summarises any
  object that names a check and gives it a status; if it finds none, it
  prints the document.
- **The fields of `sbx mcp ls --format json`** beyond `name` and `type`.

Confirmed on a real 0.49.2: `sbx template save` refuses `--description` (and
`--capture-mode`) without `--cloud`, so sbxw never passes it.
- **Whether `sbx create` accepts `DIR:ro`** extra workspaces like `sbx run`
  (documented for `run`, assumed for `create`).
- **`sbx inspect`'s output format.** sbxw only substring-matches it to *skip*
  already-applied kits, confined to its `Kits:` block (or a `kits` key in JSON),
  because inspect also lists custom secrets and one could share a kit's name. If
  neither is found, it searches the whole output; a miss just means a kit is
  re-applied.
- **The grammar of an environment file's `args:` block.** sbxw reads
  `NAME: value`, `NAME: {default: value}` and a list of `{name, default}`.
  Anything else stays in the text for sbx to resolve. A `kits:` entry written as
  a mapping (with per-kit `args`) is passed through untouched.
- **Whether `sbx ports` lists a default publish as `tcp4` or `tcp`.** sbxw
  treats both as the default on an IPv4 address, where they mean the same.

Confirmed against the 0.39 reference and a real 0.47.0 `--help`:
`create -p/--publish`, `skills import`, `setup ssh`, `policy ls` / `policy log`,
and `create --skills=MODE` (0.43+).
