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
- [Approving host access](#approving-host-access)
- [Changes Jarvis proposes](#changes-jarvis-proposes)

> **TOML trap:** `[env]` and `[[ports]]` are tables. Every root key must come
> **before** the first of them. A key written after `[[ports]]` belongs to that
> port; a key after `[env]` becomes an environment variable.

## All keys

### Daemon

| Key | Default | Meaning |
|---|---|---|
| `web_addr` | `127.0.0.1:7681` | The **UI listener**: browser, island, extension and the `sbxw` CLI. Every client needs the UI token ([why](security.md#two-listeners)) |
| `agent_addr` | `127.0.0.1:7682` | The **agent listener**, the only address sandboxes reach. Must differ from `web_addr` ([why](security.md#two-listeners)) |
| `remote_addr` | `127.0.0.1:7683` | The **remote listener**, for a paired phone through the tailnet sidecar ([remote access](security.md#remote-access)). Loopback only (`127.0.0.0/8` or `[::1]`): any other address is refused. Its own port, not `web_addr`'s or `agent_addr`'s. Bound only while remote access is on (`sbxw remote on`), which is state, not a key: off by default, and Jarvis cannot change either |
| `monitor_cmd` | `["sbx"]` | argv of the **Monitor Sandboxes** window, run on the host. `[]` hides the button |
| `terminal_color` | `""` | Terminal background in the web UI, `#rrggbb`. Empty is the built-in `#0d1117`. The header's colour picker overrides it per browser |
| `terminal_bar_color` | `""` | The bar above each terminal, `#rrggbb`. Empty is the built-in `darkslategrey`. Same picker, same override |
| `terminal_text_color` | `""` | Terminal text: the default foreground and ANSI white / bright white, `#rrggbb` or `auto`. Empty is `auto` (light or dark, following the background). Same picker, same override. Text printed in an explicit 24-bit colour is not affected |
| `max_agent_sessions` | `4` | Agent + [threads](threads-and-lead.md) one sandbox may hold open |
| `keep_awake` | `remote` | macOS: hold the Mac awake with `caffeinate` — `remote` while Remote Control is on in some session, `always`, or `off`. [Remote access](security.md#remote-access), while on, also holds it on mains power, whatever this says |
| `keep_awake_min_battery` | `5` | On battery at or below this percentage, the Mac is let go to sleep anyway |
| `battery_warn_below` | `20` | On battery at or below this percentage, Remote Control agents are told on their next prompt (again every 5 % lost) |
| `[voice]` | see below | Jarvis's voice: its local models, by catalog id: `stt` (`"parakeet-tdt-0.6b-v3-int8"`), `tts` (`"voxtral-tts-6bit"`, Voxtral TTS at 3.5 GB; also `"voxtral-tts-4bit"` at 2.5 GB and `"voxtral-tts-bf16"` at 8 GB), `tts_voice` (`"fr_male"`, one of Voxtral's 20 voices: `fr_female`, `casual_male`, `neutral_female`…), `push_to_talk_key` (`"F8"`, a `KeyboardEvent.code`), `dictate_key` (`"F9"`, dictation into the focused terminal), `[voice.words]` (how words are said aloud, see [below](#words-read-aloud)). Read from the daemon's config. Reading aloud needs a Mac with Apple Silicon and [uv](https://docs.astral.sh/uv/); `llm` and `retention_days` of an older sbxw are ignored. A table: write it after every root key |
| `[emphasis]` | on | Status pills: `[[Done]]` written by an agent drawn as a coloured pill in the terminals. `enabled`, `guidance` and the vocabulary, see [below](#status-pills). A table: write it after every root key |
| `[mac]` | see below | The [virtual Mac](../assets/terminal-mac/README.md#configuration): `vm` (`"default"`), `idle_release_minutes` (`10`), `pause_after_minutes` (`15`), `stop_after_minutes` (`120`), `force_stop_after_minutes` (`3`), `guest_host_ip` (`""`, the VM bridge's address). A table: write it after every root key |

### Sandbox creation

Read **at creation only**. Changing them doesn't affect existing sandboxes.

| Key | Default | Meaning |
|---|---|---|
| `kits` | the four default kits | See [Kits](kits-and-skills.md) |
| `kit_mounts` | `{}` | Sandbox path → host directory for kit-declared host mounts (`host-mount@1`, sbx 0.50+), passed as `--mount HOST:TARGET`. The target must be one a kit declares |
| `volumes` | `[]` | Sandbox paths kept on persistent storage across `sbxw rm` / `sbxw up` of the same name. See [Persistent volumes](kits-and-skills.md#persistent-volumes) |
| `workspaces` | `[]` | Other host folders mounted at creation, `PATH[:TARGET][:ro]` (sbx 0.51): `TARGET` an absolute path in the sandbox (else the host path), `:ro` read-only. `PATH` relative to this file, or `~/`. Exported as `additionalWorkspaces` |
| `[[carve]]` | none | A folder inside the workspace (or a `workspaces` folder) the sandbox cannot read, or can only read. See [Carving a folder out](#carving-a-folder-out) |
| `workdir` | `""` | Where the main agent starts, relative to the workspace or absolute (`sbx run --workdir`, 0.51) — a monorepo's package, say. Every attach, the existing sandbox's too |
| `cpus`, `memory` | `0`, `""` | At creation (`--cpus`, `--memory 8g`); 0 and empty leave sbx's defaults. Exported as `sandboxOptions` |
| `fuse`, `root_size`, `docker_size` | unset | Exported as `sandboxOptions` (`fuse`, `rootSize`, `dockerSize`) for `sbx env run` only: `sbx create` has no flag for them. FUSE is otherwise sbx's `sandbox.fuse` setting (on by default) |
| `cache` | `true` | Mount the [kit cache](kits-and-skills.md#the-kit-cache) |
| `cache_ttl_days` | `7` | Days before the kit cache's apt lists and packages are downloaded again; pinned downloads are kept four times as long, then the cache is emptied |
| `skills` | `""` (sbx decides) | `off`, `readonly` or `readwrite`. See [Shared skills](kits-and-skills.md#shared-skills) |
| `pull` | `""` (sbx decides) | Image pull policy: `always`, `missing` or `never` |
| `template` | `""` (sbx's image) | Template new sandboxes start from, with no registry pull: `"auto"` for one sbxw keeps per kit list and rebuilds in the background when sbx or the kits change, or a tag saved with `sbxw template save` |

### Agent

| Key | Default | Meaning |
|---|---|---|
| `claude_model` | `claude-sonnet-5-5` | Default model, passed as `ANTHROPIC_DEFAULT_MODEL`. Empty: don't set it. See [below](#the-default-model) |
| `models` | Sonnet 5.5, Opus 5.5, Haiku 5.5 | `{ id, label }`: the fallback and the pins of the model list. Every selector also offers the Claude models the daemon reads from Anthropic once an hour. See [below](#the-model-list) |
| `phone_skills` | `["code-review", "codemap", "retro", "grill-me"]` | Skills the [phone page](security.md#remote-access) may invoke, by name (lower-case letters, digits, hyphens, at most 64; anything else is refused at load). The phone never sends a `/` line: the daemon types `/<name> <text>` for a name of this list, at an idle prompt |
| `web_shell` | `""` | Empty attaches the Claude agent. Set to e.g. `"bash"` to open a shell in the main pane instead |
| `keep_running` | `false` | Keep the sandbox up with nobody attached (`sbx run -d`) until `sbxw stop`. Useful for a dev server |
| `chrome` | `false` | Start the agent with `--chrome`: Claude in Chrome, on tabs of your host browser (sbx 0.53). A host-access grant. Also needs `sbx settings set claude.chrome true`, the Claude extension and a subscription login. Same as **Claude in Chrome** in the create dialog or `sbxw up --chrome` |

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

### Tests

| Key | Default | Meaning |
|---|---|---|
| `[[tests]]` | none | Test suites the daemon's test queue runs inside the sandbox when an agent asks (`run_tests`): `name`, `command`, `paths`, `depends`, `timeout_minutes`, `retries`, `version_cmd`. A host-access grant per command. See [below](#test-suites) |

## Ports and host aliases

```toml
[[ports]]
alias = "myapp.local"       # optional /etc/hosts name
sandbox_port = 4200        # what the dev server listens on, bound to 0.0.0.0
host_port = 4200

[[ports]]
alias = "api.myapp.local"
sandbox_port = 8000
host_port = 8000
```

No ports are published by default. `ip_per_app` decides how aliases resolve:

| `ip_per_app` | How | You browse to |
|---|---|---|
| `false` (default) | Every app binds `127.0.0.1` on its own host port; aliases map to `127.0.0.1` | `http://alias:host_port` |
| `true` | Each app gets its own `127.0.0.X` (added on `lo0` on macOS, with `sudo`) | `http://myapp.local:4200`, the app's natural port |

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

A thread with a model runs `claude --model <id>`; without one it uses
`claude_model`.

## The model list

Every model selector offers the same list: the phone's **Model** select, the
per-thread model of a plan approved from the phone, and the Mac's **New thread**
and plan dialogs. A chosen model is checked against that list too.

- **The live part.** The daemon reads Anthropic's model list (`GET /v1/models`)
  through a running sandbox, at start and then once an hour, never when you
  open a selector. It keeps the models whose id starts with `claude-` and is a
  valid `--model` id, labelled with Anthropic's display name, newest first. A
  model released after your sbxw version (for example `claude-fable-5-2`)
  appears this way without a config change.
- **`models` is the fallback and the place to pin.** An entry of `models` keeps
  its own label and stays offered even when the live list does not have it.
  Until a live list has been read (no sandbox running yet, or the read fails),
  the list is `models` alone, as before.
- **A failed read changes nothing:** the last good list stays, and the daemon
  tries again after 5 minutes, doubling up to an hour. Its log says how many
  models were read, or why not, never the answer itself.

> Unverified on a real account: the read sends `anthropic-version` and the
> OAuth beta header, and relies on the sandbox proxy to add the credential.
> Whether a claude.ai sign-in (or, with `--use-api-key`, the proxy's API key)
> is accepted on `/v1/models` has not been confirmed yet. If it is not, the log
> says `could not read the model list` and the list stays `models`.

## Environment variables

```toml
env_files = [".env.sandbox"]   # a root key: must come before [env]

[env]
NODE_ENV = "development"
API_URL  = "http://api.myapp.local:8000"
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

## Carving a folder out

A sandbox can mount a folder read-write while one folder inside it is hidden
or read-only. Each rule is a `[[carve]]` table. Write the tables after every
bare key of the file.

```toml
[[carve]]
path = "clients/acme"   # relative to this file, or an absolute host path
access = "none"         # "none": not readable at all, or "ro": read-only
enforce = "strict"      # "strict" (the default) or "soft"
```

`path` must lie inside the workspace or inside a `workspaces` folder (not be
one), with no `..`. A relative path resolves like the file's other relative
paths. The folder must exist when the sandbox is created. A carve can also be
added in the Create dialog; the dialog's carves are your own decision and are
not asked about again.

**Strict** keeps the folder out of the sandbox altogether. sbx cannot leave a
folder out of a mount, so sbxw does not mount the parent as a whole. It mounts
every other folder on the way down, one by one, at the same place. The cost:
files and other non-folders next to the mounted folders (`README.md`, and
`sbxw.toml` itself) are not visible, and nothing can be created at those
levels, because they belong to no mount. `sbxw up` and the dialog list the
files that will be missing. If the workspace itself is split, sbx is given an
empty stub folder as its workspace, and the agent starts in the real workspace
path (or your `workdir`, made absolute). A symlink in a split folder is not
shared either. At most 200 folders can be mounted; a folder holding thousands
of children is better carved with `soft`.

**Soft** keeps the parent mounted as it is and lays a second mount over the
carve: an empty read-only folder for `none`, the folder itself read-only for
`ro`. No file goes missing. But the agent has root in its sandbox and can
unmount it, so soft protects against mistakes, not against a hostile agent.

Which one: strict for a folder of projects where one must stay private; soft
for a repository with files at its root (`package.json`, `Makefile`) where
a split would break the tooling, or when you only want a guard against slips.

Mounts are fixed when a sandbox is created. If the file's carves change later,
`sbxw up` says the sandbox is unchanged; `sbxw rm <name>` then `sbxw up <name>`
creates it with the new ones. Because the file can be edited from inside the
sandbox, removing a carve or weakening it (`none` to `ro`, `strict` to `soft`)
needs your approval like a new grant; adding or strengthening one does not.
The line reads `access restored: /path (was: no access, strict)`.

## Test suites

A `[[tests]]` table declares a suite the daemon's **test queue** may run.
Agents ask for one by name with the `run_tests` tool of the `sbxw-relay` MCP
server, see what is declared with `list_tests`, and pick up a long run with
`check_tests`. An agent can only name a suite: no tool takes a command, and
the suite's name and the commit go to the queue's script as arguments.

```toml
[[tests]]
name = "crates"                          # lower-case letters, digits, hyphens
command = "scripts/run-suite.sh crates"  # run with `bash -c` in the checkout
paths = ["crates", "Cargo.toml", "Cargo.lock"]  # relative to the repo root; "." is all of it
depends = []                             # suites whose paths this one also covers
timeout_minutes = 60                     # 1 to 240; default 30
retries = 1                              # 0 or 1: run a failed suite once more
version_cmd = "rustc --version"          # optional: its output is part of the key
```

- **A commit, not the files being edited.** The queue tests a commit of the
  workspace's repository (by default the asking session's `HEAD`): it checks
  it out, detached, in a worktree of its own, `.sbxw-worktrees/.tests`, and
  runs `command` there inside the sandbox. A ref that is not a commit of that
  repository is refused, and so is anything that is not a plain revision
  (`main`, a hash, `HEAD~1`).
- **The environment.** `SBXW_TEST_TARGET` is one build directory in the
  sandbox's home (`~/.sbxw/tests/target`), kept from job to job so builds stay
  warm; `SBXW_TEST_CHECKOUT` is the checkout (also the working directory);
  `SBXW_TEST_SUITE` and `SBXW_TEST_COMMIT` name the job. The full output stays
  in the sandbox, in `~/.sbxw/tests/logs/<suite>.log`; only its last lines
  reach the agent.
- **Reuse.** A result is stored under a key: the suite's declaration (and its
  dependencies'), the git object of every path the suite and its `depends`
  cover at that commit, and what `version_cmd` prints. Asked again with the
  same key, the queue answers the stored result at once, marked as reused,
  with the commit it first ran on: a commit that changes only paths the suite
  does not cover runs nothing. List the lock files a suite depends on among
  its `paths`. A suite that timed out or could not start is never reused;
  `run_tests` with `rerun` runs it again regardless.
- **One job at a time per sandbox.** A second request waits its turn; a
  request for the same suite and key as a job queued or running joins it.
  The job id is returned at once and kept, with the results, in
  `~/.sbxw/state/tests.json`, so a daemon restart loses neither (a job that
  was running runs again). Results older than 30 days are pruned.
- **Retries.** With `retries = 1` a failed or timed-out suite runs once more,
  and the result says so (`attempts`, the first outcome), so a flaky test
  stays visible.
- **The team suite.** A lead may name one declared suite as its plan's
  `team_suite`; `run_tests` with no suite asks for it, on the team branch's
  tip for a shared plan. The plan counts how many jobs ran and how many were
  answered without running.
- **Trust.** The file is agent-writable, so each `command` and `version_cmd`
  is a host-access line approved at `sbxw up` (`test suite crates: an agent
  may make sbxw run ...`), and the queue reads the suites only from the
  config recorded at the last bring-up, never from the file as it is now.
- **Where.** Like `[[ports]]`, the tables go after every bare key of the file.
  `sbxw.toml.example` ends with sbxw's own four suites, ready to paste.

## Approving host access

Settings that reach the host or widen the sandbox (mounts, `env_files`,
extra kits, extra `network_allow` hosts, ports and aliases, `monitor_cmd`,
the listeners, and a few more) take effect only once you have approved them
for that file. The first bring-up after you add one lists it and asks:

```
sbxw  /Users/me/myapp/sbxw.toml asks for host access you have not approved yet:
    env file: /Users/me/myapp/.env.sandbox
    mount (read-only): /Users/me/docs at /workspace/docs
    network: api.example.com
Only approve what you expect: this file can be edited from inside the sandbox.
Allow? [y/N]
```

Later bring-ups with the same lines ask nothing. In a script, pass
`--allow-host-access` to `sbxw up`, `sbxw chat` or `sbxw web`; without it, a
command run off a terminal fails with the list. The web UI asks with a dialog.

```sh
sbxw config grants            # what is approved for ./sbxw.toml, and what is not yet
sbxw config grants --revoke   # forget it; the next bring-up asks again
```

The full list of what is asked, and where approvals are kept, is in
[Security](security.md#the-agent-can-edit-sbxwtoml).

## Words read aloud

`[voice.words]` in the daemon's `sbxw.toml` says how the voice reads a word
it would say badly. It changes only what is read aloud: dictation, and what
is sent to an agent, are untouched.

```toml
[voice.words]
sbxw = { fr = "ece bé ixe double vé", en = "S B X W" }
nginx = { en = "engine X" }
"node.js" = { fr = "node jé esse" }
```

- **The word** is 1 to 40 letters, digits, `-`, `_` or `.`, starting and
  ending with a letter or digit (quote it when it holds a dot). It is
  matched as a whole word, whatever its case: `nginx` is not changed in
  `nginx2`. A longer word is tried first. The same word twice, in two cases,
  is refused.
- **How it is said**, per language: `fr` for a French voice (`tts_voice =
  "fr_*"`), `en` for every other. Each is one line of at most 120
  characters, with no control characters. A language left out: the word is
  read as written in it.
- **Built-in words.** sbxw ships one, `sbxw`, spelt letter by letter. An
  entry of the same word replaces it whole (so `sbxw = { fr = "…" }` alone
  reads `sbxw` as written in English); without one, the built-in applies.
- At most 200 entries. A table: write it after every root key.

A change by hand takes effect when the daemon restarts. A change Jarvis
proposes and you approve is used from the next sentence read.

## Status pills

An agent writes a short label in double brackets beside what it describes,
`[[Done]]` or `[[Failed: 3 tests]]`, and sbxw's terminals (web and desktop)
draw it as a coloured pill. The word sets the colour. Only the drawing
changes: copy, selection, scrollback, search and the session's bytes keep
the brackets.

```toml
[emphasis]
enabled = true    # draw the pills (default true)
guidance = true   # tell agents about them in CLAUDE.md (default true)

# Optional. When present it REPLACES the built-in vocabulary whole.
[emphasis.words]
done = { tone = "green", also = ["verified", "fixed", "passed"], use = "finished and checked" }
"needs you" = { tone = "amber", also = ["blocked"], use = "the person must act, answer or choose" }
```

- **A family** is keyed by its main word, with a `tone`, its other words
  (`also`) and one line on when to use it (`use`, at most 80 characters,
  written into the agents' guidance).
- **Tones** are names, not colours, so each UI uses its own palette:
  `green` (settled, done), `red` (wrong, failed), `amber` (needs you, look
  here), `blue` (context), `purple` (Jarvis, the team), `cyan` (the kind of
  work), `danger` (a filled red: cannot be undone, or costs money).
- **Words** are 1 to 32 letters, digits, single spaces or `-`, starting with
  a letter and ending with a letter or digit, matched whatever their case.
  A word belongs to one family. At most 12 families and 48 words in all:
  each one costs guidance tokens in every session. A bad entry is refused
  with its reason, never dropped.
- **The built-in vocabulary** (no `[emphasis.words]`): `done` (verified,
  fixed, passed) green; `failed` (error, broken) red; `needs you` (blocked,
  decide, question) amber; `irreversible` (destructive, costly) danger;
  `note` (context, fyi) blue; `working` (in progress, next, plan) cyan;
  `team` (lead, thread, jarvis) purple.
- **Per sandbox.** A project's own `sbxw.toml` may set `enabled` and
  `guidance` under `[emphasis]` for its sandboxes; they apply from its next
  bring-up. The vocabulary is read from the daemon's file only.

### The guidance

At each bring-up, when both `enabled` and `guidance` are on for the sandbox,
sbxw writes a short "Status labels" section, generated from the same table,
into the agent's `~/.claude/CLAUDE.md` (fenced, replaced in place; taken out
when switched off). About 260 tokens (977 characters) for the built-in vocabulary. A running
session read its `CLAUDE.md` when it started: it follows the new text from
its next session. Jarvis's own sandbox never gets it. Agents other than
Claude Code get no guidance: sbxw writes none for them.

### The matching rule

Both UIs implement the same rule (cases shared in
`crates/core/testdata/emphasis-cases.json`):

- A marker is `[[` WORD [`:` DETAIL] `]]` on one line, in JavaScript
  `/\[\[(\p{L}(?:[\p{L}\p{N} -]{0,30}[\p{L}\p{N}])?)(?::([^\[\]\p{Cc}]{1,48}))?\]\]/gu`:
  WORD as above (at most 32), DETAIL 1 to 48 characters with no bracket or
  control character. Scanned left to right; brackets never nest, so in
  `[[a [[Done]] b]]` only `[[Done]]` is a marker.
- WORD is looked up lower-cased, runs of spaces made one. A known word takes
  its tone. An unknown word is drawn in the `blue` (context) tone only when
  it starts with an upper-case letter, so lower-case wiki links such as
  `[[base-kit]]` stay text.
- A marker cut by the terminal's soft wrap is matched on the joined line and
  drawn as one piece per row; a hard line break ends it.
- The pill covers exactly the marker's cells (a wide character is two), so
  nothing shifts.

A change by hand takes effect when the daemon restarts (and for the guidance,
at each sandbox's next bring-up). A change Jarvis proposes and you approve
redraws the terminals at once.

## Changes Jarvis proposes

[Jarvis](jarvis.md) can propose changes to this configuration. Each one is
applied by the daemon only after you approve it, as a diff you read first:

- one of a fixed list of keys of the daemon's `sbxw.toml`: the `[voice]`
  keys, `claude_model`, `terminal_color`, `terminal_bar_color`,
  `terminal_text_color`, `keep_awake`, `keep_awake_min_battery`,
  `battery_warn_below`, `cache_ttl_days`;
- one word of `[voice.words]`: added, replaced, or its entry removed;
- the daemon's `network_allow`, bare host names only;
- the `kits` line of the daemon's `sbxw.toml` or of one sandbox's, with
  bundled kits and Jarvis kits only;
- MCP servers, as the MCP tab of Setup adds them.

Comments and key order in the file are kept. A setting or `network_allow`
change takes effect when the daemon restarts; a `[voice.words]` change is
used from the next sentence read. When the daemon was started where
there is no `sbxw.toml`, the first such change creates it, at the path the
daemon was asked to read (its folder must exist); the dialog says so and the
diff shows a new file. The Kits dialog lists that file too, and saving its
`kits` creates it. The kits Jarvis writes live
under `~/.sbxw/state/kits/jarvis-*`; see
[Jarvis kits](kits-and-skills.md#jarvis-kits).
