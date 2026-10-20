# Configuration (`sbxw.toml`)

[← Back to README](../README.md)

sbxw reads `sbxw.toml` from the current directory (or `--config <file>`). Every
key is optional. Start from [`sbxw.toml.example`](../sbxw.toml.example), which
lists every key with its default.

- [All keys](#all-keys)
- [Ports and host aliases](#ports-and-host-aliases)
- [The default model](#the-default-model)
- [Environment variables](#environment-variables)
- [Which config a sandbox uses](#which-config-a-sandbox-uses)

> **TOML trap:** `[env]` and `[[ports]]` are tables. Every root key must come
> **before** the first of them. A key written after `[[ports]]` belongs to that
> port; a key after `[env]` becomes an environment variable.

## All keys

### Daemon

| Key | Default | Meaning |
|---|---|---|
| `web_addr` | `127.0.0.1:7681` | The **UI listener**: browser, island, extension |
| `agent_addr` | `127.0.0.1:7682` | The **agent listener**, the only address sandboxes reach. Must differ from `web_addr` ([why](security.md#two-listeners)) |
| `monitor_cmd` | `["sbx"]` | argv of the **Monitor Sandboxes** pane, run on the host. `[]` hides the button |
| `max_agent_sessions` | `4` | Agent + [threads](threads-and-lead.md) one sandbox may hold open |

### Sandbox creation

Read **at creation only**. Changing them doesn't affect existing sandboxes.

| Key | Default | Meaning |
|---|---|---|
| `kits` | the three default kits | See [Kits](kits-and-skills.md) |
| `cache` | `true` | Mount the [kit cache](kits-and-skills.md#the-kit-cache) |
| `cache_ttl_days` | `7` | Days before the kit cache is emptied and refilled |
| `skills` | `""` (sbx decides) | `off`, `readonly` or `readwrite`. See [Shared skills](kits-and-skills.md#shared-skills) |
| `share_skills` | `true` | Old spelling: `false` means `skills = "off"` |
| `pull` | `""` (sbx decides) | Image pull policy: `always`, `missing` or `never` |

### Agent

| Key | Default | Meaning |
|---|---|---|
| `claude_model` | `claude-sonnet-5-5` | Default model, passed as `ANTHROPIC_DEFAULT_MODEL`. Empty: don't set it. See [below](#the-default-model) |
| `models` | Sonnet 5.5, Opus 5.5, Haiku 5.5 | `{ id, label }` choices for new threads and a lead's plans |
| `claude_subscription` | `pro` | Plan written into injected OAuth credentials: `pro`, `max`, `team`, `enterprise` or `free` |
| `web_shell` | `""` | Empty attaches the Claude agent. Set to e.g. `"bash"` to open a shell in the main pane instead |
| `keep_running` | `false` | Keep the sandbox up with nobody attached (`sbx run -d`) until `sbxw stop`. Useful for a dev server |

### Network

| Key | Default | Meaning |
|---|---|---|
| `network_allow` | npm, Yarn, PyPI, Packagist, Composer, GitHub, Docker Hub, GHCR, `api.anthropic.com` | Egress allowlist, applied with `sbx policy allow network`. Never `**` |
| `network_deny` | `[]` | Explicit denylist, applied with `sbx policy deny network` |
| `ip_per_app` | `false` | One loopback IP per app. See [below](#ports-and-host-aliases) |

### Environment and ports

| Key | Default | Meaning |
|---|---|---|
| `env_files` | `[]` | `KEY=VALUE` files passed as `--env-file`, in order |
| `[env]` | empty | Variables for the sandbox. See [below](#environment-variables) |
| `[[ports]]` | none | `alias`, `sandbox_port`, `host_port`. See [below](#ports-and-host-aliases) |

## Ports and host aliases

```toml
[[ports]]
alias = "neos.local"       # optional /etc/hosts name
sandbox_port = 4200        # what the dev server listens on, bound to 0.0.0.0
host_port = 4200

[[ports]]
alias = "api.neos.local"
sandbox_port = 8000
host_port = 8000
```

No ports are published by default. `ip_per_app` decides how aliases resolve:

| `ip_per_app` | How | You browse to |
|---|---|---|
| `false` (default) | Every app binds `127.0.0.1` on its own host port; aliases map to `127.0.0.1` | `http://alias:host_port` |
| `true` | Each app gets its own `127.0.0.X` (added on `lo0` on macOS, with `sudo`) | `http://neos.local:4200`, the app's natural port |

Writing `/etc/hosts` needs `sudo`. See [what `sbxw up` does](cli.md#what-sbxw-up-does)
and `sbxw hosts`.

## The default model

`claude_model` reaches the sandbox as `ANTHROPIC_DEFAULT_MODEL`, alongside
`[env]`. It's a real default: Claude Code uses it **only when nothing else picks
a model**, so a `/model` you choose in-session survives the next `sbxw up`.
Setting `ANTHROPIC_DEFAULT_MODEL` in `[env]` yourself overrides it.

- **It needs Claude Code 2.1.236+ inside the sandbox.** That version comes from
  the sbx template, not from sbxw. On an older one the variable is silently
  ignored.
- **Older sandboxes** carry a `model` key that sbxw used to write into
  `~/.claude/settings.json`, which would outrank the variable. On the next
  `sbxw up`, sbxw removes that key, but only if it still equals `claude_model`
  (sbxw's own leftover). A different value is your `/model` and is kept.

`models` lists what a thread can start on. A thread with a model runs
`claude --model <id>`; without one it uses `claude_model`.

## Environment variables

```toml
env_files = [".env.sandbox"]   # a root key: must come before [env]

[env]
NODE_ENV = "development"
API_URL  = "http://api.neos.local:8000"
```

Both reach the sandbox twice:

- **At creation** (`sbx create -e` / `--env-file`): baked in, so every process
  sees them: the agent, a Bash pane, a dev server.
- **On every attach** (`sbx run -e`): applied to the **agent session**. Editing
  `[env]` and reopening the agent pane is enough. A Bash pane still sees the
  creation-time values until you recreate the sandbox.

sbx settles precedence: `[env]` beats every file, and a later file beats an
earlier one. Relative `env_files` paths resolve against `sbxw.toml`.

**Not for secrets.** Values land in `sbx create`'s argv (visible in the host's
process list) and in a file most projects commit. Use `sbx secret set`, or an
`sbxenv.yaml` `secrets:` entry, for tokens.

## Which config a sandbox uses

- The daemon takes its `web_addr` / `agent_addr` from the `sbxw.toml` it was
  started with.
- Each sandbox keeps the `sbxw.toml` it was brought up with (its env, model,
  ports and policy), whoever started the daemon.

`sbxw.toml` usually sits inside the workspace, where the agent can edit it. See
[Security](security.md#the-agent-can-edit-sbxwtoml).
