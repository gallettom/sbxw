# Environment files (`sbxenv.yaml`)

[← Back to README](../README.md)

sbx has its own committable project format, `sbxenv.yaml`, read by
`sbx env run|create|exec|rm`. It covers the agent, workspace, extra mounts,
kits, env vars, secrets, MCP servers, skills, ports and registry credentials.
sbxw both **runs** on one and **writes** one.

- [The format, as sbx 0.43+ reads it](#the-format-as-sbx-043-reads-it)
- [`sbxw env run`: bring a sandbox up from the file](#sbxw-env-run-bring-a-sandbox-up-from-the-file)
- [`sbxw env export`: write the file](#sbxw-env-export-write-the-file)

## The format, as sbx 0.43+ reads it

- **The file is `sbxenv.yaml`, not hidden.** A hidden `.sbxenv.yaml` in a
  project is no longer read; `sbxw env run` tells you to rename it.
  `~/.sbxenv.yaml` in your home is merged **beneath** every project file as
  shared defaults.
- **`${VAR}` is not expanded.** Use `${{ env.projectDir }}`,
  `${{ env.fileDir }}` and `${{ env.args.NAME }}` (declared in an `args:` block,
  supplied with `--env-arg NAME=VALUE`). sbxw warns about any `${VAR}` it finds.
- **Relative paths resolve against the file that declares them.**
  `workspace: ./neos` means the same thing wherever you run from.
- **No `workspace:` means no mount.** sbxw refuses such a file, since its
  pipeline needs a workspace. Write `workspace: .` to mount the file's folder.
- **`sbx env create` shows a plan and asks** before changing the host, so
  `sbxw env run` needs you at the terminal for that step.
- **`secrets.<name>.command` runs from a fresh temporary directory** (sbx
  0.46+). sbxw warns about one using `./helper` or `../token`: use an absolute
  path outside the workspace, or a program on `PATH`.

## `sbxw env run`: bring a sandbox up from the file

```bash
sbxw env run                        # sbxenv.yaml in the current directory
sbxw env run ../neos-env            # ...or in that directory
sbxw env run base.yaml local.yaml   # merged in order, later files win
sbxw env run --env-arg port=4300    # a value for ${{ env.args.port }}
```

**The split:** `sbx env create` does the **creation**, because only it applies
an environment file whole (secrets, bindings, registries, MCP servers, skills,
kits, mounts). Then sbxw's usual pipeline adds what the format has no field
for: egress policy, OAuth credentials, hooks, `/etc/hosts` aliases, and the
browser terminal. (`sbx env run` re-applies only `env` and MCP to an existing
sandbox, so anything that relies on "run it again" has to live outside it.)

The sandbox name goes in as `sbx env create --name`.

### Ports are settled before sbx is called

A port sbx can't publish costs you the **whole sandbox**: creation fails, the
sandbox is removed, and secrets it provisioned first are left behind. There's
no way to fix it from outside either (`env create` has no port flag, and a
second file can only *add* ports). So sbxw checks every host port first, asks,
and rewrites the document once:

```
$ sbxw env run
sbxw  /home/you/dev/neos-env/sbxenv.yaml → sandbox 'neos'
  ! host port 4200 (127.0.0.1) is busy — port for sandbox:4200 [4201] ⏎
sbxw  ports adjusted — running from a rewritten copy at ~/.sbxw/state/env/neos/sbxenv.yaml
sbxw  handing secrets, mcp to sbx — sbxw does not read those
sbxw  taken from sbxw.toml:
        · /etc/hosts aliases (neos.local → :4201)
        · the egress allowlist (15 rules) — an environment file has no field for it
        · claude_model (claude-sonnet-5-5) — passed as ANTHROPIC_DEFAULT_MODEL
```

- With no terminal to ask on (a daemon, CI), it takes the next free port and
  logs it. `--yes` does the same on a terminal.
- **Your committed file is never edited.** The rewrite goes to
  `~/.sbxw/state/env/<name>/`, with absolute paths. Keys sbxw doesn't model
  (`secrets`, `bindings`, `registries`, `mcp`, `skills`, `sandboxOptions`, kit
  `args`) pass through untouched. When nothing has to move, sbx gets your file
  as is.
- The copy holds only your project's files; sbx merges `~/.sbxenv.yaml`
  beneath it again. A busy port declared in `~/.sbxenv.yaml` itself can't be
  fixed this way: sbxw stops and asks you to change it there.

### The file wins; `sbxw.toml` fills the gaps

Field by field, the environment file wins. `sbxw.toml` supplies what the format
lacks (egress allowlist, aliases, model), and the run prints what it borrowed.

**Aliases survive the merge.** When the file publishes 4200 and `sbxw.toml`
names 4200 `neos.local`, the alias is kept and pointed at whatever host port the
sandbox actually got (read back from `sbx ports`).

## `sbxw env export`: write the file

From the CLI:

```bash
sbxw env export                        # writes ../sbxenv.yaml for ./ as the workspace
sbxw env export -o ~/dev/sbxenv.yaml   # further up: workspace: ./acme/neos
sbxw env export -o -                   # print it instead
```

From the web UI: the **Env** button in a pane's bar
([screenshot](web-ui.md#environment-file-and-ssh-cards)). Prefer it when the two
differ: it reads the ports the sandbox **actually has**, including ones added in
the UI or moved because a host port was busy.

**Every path is relative to the file**, so one committed file works wherever
people keep their repos:

```yaml
workspace: ./neos
skills: 'off'
kits:
  - ./kits/codemap
```

A kit that shares only `/` with the file's folder is written as an absolute
path. (sbxw used to write `${SBXW_PROJECTS_ROOT:-…}`; sbx 0.42 stopped expanding
`${VAR}`, so that's gone. An old `export SBXW_PROJECTS_ROOT=…` in your shell rc
is harmless.)

**What doesn't fit goes in the header**, with the command that reproduces it:
the allowlist as an `sbx policy allow network` line, the OAuth injection, the
aliases, the model. An export that silently dropped the allowlist would describe
a sandbox with *broader* egress than sbxw builds.

**The file lands in the workspace's parent.** The agent can write anywhere in
its workspace, and this file decides what the *next* sandbox gets, so it lives
outside. (sbx binds the file read-only into sandboxes `sbx env` creates, but a
sandbox from `sbxw up` mounts the workspace without that protection.)

Nothing keeps `sbxw.toml` and the export in step. Re-export after editing
`sbxw.toml`.
