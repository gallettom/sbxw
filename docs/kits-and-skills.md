# Kits and skills

[← Back to README](../README.md)

- [Kits](#kits)
- [Bundled kits](#bundled-kits)
- [The kit cache](#the-kit-cache)
- [Shared skills](#shared-skills)
- [Keeping a skill you made](#keeping-a-skill-you-made)
- [Writing your own kit](#writing-your-own-kit)

## Kits

Kits are `sbx`'s declarative extension point: tools, files, environment,
network rules and startup commands. List them in `sbxw.toml`:

```toml
kits = [
  "sbxw:codemap", "sbxw:agent-skills", "sbxw:md-to-pdf-tools",  # the defaults
  "sbxw:retro",                                                 # default too
  "sbxw:k8s-tools",                                             # opt-in
  "./my-kit",                   # relative paths resolve against sbxw.toml
]
```

**Without a `kits` line**, a sandbox gets `sbxw:codemap`, `sbxw:agent-skills`,
`sbxw:md-to-pdf-tools` and `sbxw:retro`. Writing `kits` replaces the defaults; `kits = []`
means none.

A kit reference is one of:

- `sbxw:<name>`, a kit built into the sbxw binary;
- a **directory containing `spec.yaml`** (not a single `.yaml` file);
- a `.zip`, an OCI ref (`ghcr.io/org/kit:tag`), or a git URL.

**Every kit must be kit spec v3** (`schemaVersion: "3"`). Since sbx 0.51,
`claude` is the published v3 workload `docker.io/sbx/claude:latest` (Docker
Hardened Images, Debian 13), and a v1 or v2 kit cannot be combined with it.
`sbxw up` refuses a local v1/v2 kit before anything is created, naming it; see
[Writing your own kit](#writing-your-own-kit) to convert one.

### sbxw's base kit

Every sandbox sbxw creates also gets `sbxw:base`, first, whatever `kits` says.
It installs Node.js (`nodejs-24` from the image's Docker Hardened Images
repository, else Debian's `nodejs`) in an install hook, once, before the
agent starts: sbx 0.51's image ships none, and sbxw's relay, session hooks,
status line and the bundled kits' scripts all run on it. It opens
`deb.debian.org`, `security.debian.org` and `dhi.io` for that hook only. It is
never listed in `kits` and the Kits dialog does not offer it. It also carries, as `agent-context@2`, the agent's
instructions for the relay (asking another sandbox, asking you for a
screenshot): sbx writes them into `~/.claude/CLAUDE.md` before the agent
starts, and sbxw's tooling writes the same text afterwards only if sbx did not.

The web UI's **Kits** button edits the same line without opening the file:
see [the sidebar footer](web-ui.md#the-sidebar-footer).

### Kits go in at creation

`sbxw up` passes every kit to `sbx create --kit` (sbxw's base kit first, then
the credentials kit), because creation applies a kit in one step, with no
container swap.

A kit added to `kits` later reaches an existing sandbox on the next `sbxw up`,
via `sbx kit add`. That recreates the container (state kept), so sbxw skips
kits it already applied: it records them itself, and also recognises the paths
`sbx inspect` lists. sbx 0.51 adds a v3 kit's image content, install and
startup hooks and network allow rules this way, which covers every bundled
kit. It refuses a kit that declares credentials, published ports, volumes,
generated lifecycle files or host-level deny rules, and any kit for a sandbox
created by an sbx older than 0.51. Then sbxw tells you to recreate
(`sbxw rm <name>`, then `sbxw up <name>`) and changes nothing — it never
recreates on its own, since anything outside the workspace would be lost. To
force a re-apply, run `sbx kit add <sandbox> <kit>` yourself.

A kit that adds network rules makes sbx ask you to approve them, and sbx has
no flag for that answer. sbxw then keeps the kit waiting and puts a **✓** on
the sandbox's row: it opens a pane running `sbx kit add` where you answer
sbx's prompt, one kit per ✓.

A kit's `network-policy@1` allow rules join the sandbox policy when it's
applied. The bundled kits declare everything they fetch, so none of them needs
a `network_allow` entry.

`sbxw:` kits are written to `~/.sbxw/kits/<name>/` and rewritten only when a new
sbxw ships a different spec. That path has to outlive the sandbox, because sbx
re-resolves a kit by path on every container swap. `sbxw env export` writes the
path, since sbx doesn't know `sbxw:` names.

## Bundled kits

| Kit | Default | What it adds |
|---|---|---|
| `sbxw:codemap` | ✓ | `/codemap` and `/codemap-lens`: agents read a repo's code map before its source |
| `sbxw:agent-skills` | ✓ | `/archify`, `/brag`, `/brag-slim`, `/i-have-adhd`, `/handoff`, `/grill-me`, `/grilling`, `/frontend-design`, plus headless Chromium and FFmpeg |
| `sbxw:md-to-pdf-tools` | ✓ | `/md-to-pdf`, with WeasyPrint, poppler-utils and Pillow |
| `sbxw:retro` | ✓ | `/retro`: reads past sessions and proposes what should become a script, hook, skill, rule or kit |
| `sbxw:k8s-tools` | | `kubectl`, `k3d` and `skaffold` in `~/.local/bin` |
| `sbxw:git-identity` | | Your host's Git `user.name` and `user.email` in the sandbox |
| `sbxw:ssh-agent` | | Your host's SSH agent, restricted to signing commits and the common Git hosts |
| `sbxw:terminal-mac` | | A virtual Mac the agent drives (Safari, Mac apps) through terminal-mac on an Apple silicon host |

The web UI's **Skills** button lists each command and whether it runs on its own
(*default*) or only when typed (*manual*).

### `sbxw:codemap`

A **code map** is a few linked Markdown files in `.sbxw-artifacts/codemap/`
saying what a repo does, why it's built that way, and where each idea lives. It
uses the [lat.md](https://github.com/vercel-labs/lat.md) format: `[[wiki
links]]` between sections, links into source symbols, and `@lat:` comments in
code pointing back. It includes an [Archify](https://github.com/tt-a1i/archify)
architecture diagram in `diagram/`, so apply `sbxw:agent-skills` too.

The kit ships `/codemap`, a format reference, an offline checker, and three
hooks that make agents use the map:

- `SessionStart` injects the map's index into the session;
- the first `Grep`/`Glob` recalls it;
- in a repo **without** a map, after a session that clearly read the tree, a
  `Stop` hook asks the agent to *offer* to write one (once per repo per 12h).
  Writing it stays your call.

`/codemap-lens` retells the map for one reader into
`.sbxw-artifacts/codemap-lenses/<slug>/`. Both run from the web UI's
[Project panel](web-ui.md#code-map) in background sessions. Needs no network.
See [`assets/codemap/README.md`](../assets/codemap/README.md).

### `sbxw:agent-skills`

Each skill is pinned to a commit:

| Command | Source | What it does |
|---|---|---|
| `/archify` | [tt-a1i/archify](https://github.com/tt-a1i/archify) | Validated, interactive HTML architecture, workflow, sequence, data-flow and lifecycle diagrams |
| `/brag`, `/brag-slim` | [latent-spaces/brag](https://github.com/latent-spaces/brag) | A short launch video of the project |
| `/i-have-adhd` | [ayghri/i-have-adhd](https://github.com/ayghri/i-have-adhd) | Answers shaped for ADHD readers. On in every session via a `SessionStart` hook |
| `/handoff`, `/grill-me`, `/grilling` | [mattpocock/skills](https://github.com/mattpocock/skills) | Handoff documents; relentless plan interviews |
| `/frontend-design` | [anthropics/skills](https://github.com/anthropics/skills) | Guidance for distinctive UI design |

Plus Playwright's headless Chromium (which has linux-arm64 builds) and FFmpeg.
Skills land in `~/.sbxw/agent-skills/` with thin user-level commands pointing at
them, never in the shared skill store. See
[`assets/agent-skills/README.md`](../assets/agent-skills/README.md).

### `sbxw:md-to-pdf-tools`

The `/md-to-pdf` skill (user-level, so it works whatever project is mounted)
and the WeasyPrint stack it needs, so the first use has no install step. See
[`assets/md-to-pdf-tools/README.md`](../assets/md-to-pdf-tools/README.md).

### `sbxw:retro`

`/retro` reads back over Claude Code sessions (`/retro` for this one, `/retro
last 5` for the five newest) and finds what should stop depending on the agent
remembering: a fixed command sequence that wants to be a script, a step that
must happen on every event and wants to be a hook, a correction given twice
that wants to be a `CLAUDE.md` rule, a tool installed by hand that wants to be
a kit. A digest script does the counting over the transcripts. The report, with
evidence for each candidate, goes to `.sbxw-artifacts/retro/`; nothing is built
or installed until you pick a candidate. Needs no network. See
[`assets/retro/README.md`](../assets/retro/README.md).

### `sbxw:k8s-tools`

`kubectl`, `k3d` and `skaffold`, arch-aware and idempotent. Opt-in. See
[`assets/k8s-tools/README.md`](../assets/k8s-tools/README.md).

### `sbxw:git-identity` and `sbxw:ssh-agent`

Two kit spec v3 capability kits, opt-in because each hands the
sandbox something of yours:

- **git-identity** (`git-identity@1`): sbx copies `user.name` and `user.email`
  from your host Git config into the agent's global Git config, so commits are
  yours. Nothing else from the host config comes along.
- **ssh-agent** (`ssh-agent@1`): your host SSH agent, served on a socket in the
  sandbox (`SSH_AUTH_SOCK`). Your keys never enter it. Restricted to signing
  Git objects and authenticating to `git@github.com`, `git@gitlab.com` and
  `git@bitbucket.org`; for other hosts, write your own kit from
  [`assets/ssh-agent/spec.yaml`](../assets/ssh-agent/spec.yaml).

Both are optional capabilities: on a host without a Git identity or a running
agent, the sandbox is created without them rather than failing. Neither works
in cloud sandboxes.

### `sbxw:terminal-mac`

A real macOS guest on the host, run by
[terminal-mac](https://github.com/govijr/terminal-mac), that the agent drives
with `mac_*` tools: Safari/WebKit, Mac apps, native windows. One VM shared by
every sandbox, one holder at a time; a boot or a published port needs your
approval, clicks and typing do not. Opt-in, Apple silicon hosts only; sbxw
installs a pinned, SHA-256-checked release. See
[`assets/terminal-mac/README.md`](../assets/terminal-mac/README.md).

## Kits are checked before anything is created

On every `sbxw up`, each of your own local kits (a directory, not a bundled
one, not a registry reference) has its `schemaVersion` read before the sandbox
is created or changed: a v1 or v2 kit fails the bring-up in a second, naming
the kit, not after the image pull. A v3 kit is built and checked by sbx at
creation (`sbx kit validate` reads only v1/v2 specs). `sbxw doctor` runs the
same check.

## Persistent volumes

```toml
volumes = ["/home/agent/.npm", "/home/agent/.cache/pip", "/home/agent/.cargo/registry"]
```

Paths inside the sandbox kept on storage that outlives it (sbx 0.50,
`volume@1`): `sbxw rm` then `sbxw up` under the same name — switching to Remote
Control, say — finds them as they were. sbxw writes them as a small kit at
`~/.sbxw/state/kits/<name>-volumes/` and keeps that folder when the sandbox is
removed, because sbx reattaches the storage only to the same kit. They apply
at creation: changing the list on an existing sandbox warns and changes
nothing. Your workspace (and its `node_modules`) is your own folder already;
this is for what lives outside it.

## Kit folders (host mounts)

A kit can declare a folder it shares with your machine (`host-mount@1`, sbx
0.50). By default sbx picks that folder; `kit_mounts` picks it yourself:

```toml
kit_mounts = { "/home/agent/history" = "~/agent-history" }
```

The create dialog shows the same thing as **Kit folders**, one row per path
your configured kits declare, with a folder picker. Only declared paths are
accepted, and only at creation.

## The kit cache

`agent-skills` and `md-to-pdf-tools` spend their startup downloading: apt
packages, a Playwright Chromium, skill tarballs, Python wheels. With
`cache = true` (the default), sbxw mounts `~/.sbxw/cache` into every new sandbox
and sets `SBXW_CACHE`. The kits copy from it and fall back to the network.

- **The first sandbox fills it.** When the cache is empty or expired, the
  sandbox being created gets it **writable** (`SBXW_CACHE_FILL=1`). Its kits
  download into it and mark each kit done. Every other sandbox gets it
  **read-only**. One filler at a time; a fill not done in 30 minutes counts as
  failed and the next sandbox takes over.
- **It expires** after `cache_ttl_days` (default 7): the next sandbox empties
  and refills it. This bounds how long anything the filler's agent might have
  left there survives.
- **Per image flavour:** files go under `<arch>-<codename>/` (e.g.
  `arm64-bookworm`), so a `.deb` never reaches the wrong distribution.

```bash
sbxw cache          # each flavour, its size, when it expires
sbxw cache clear    # empty it now
```

`cache = false` turns the mount off. Existing sandboxes keep what they were
created with.

## Shared skills

sbx keeps a **skill store shared across sandboxes**, separate from kits.
`sbxw skills import` fills it from the agents installed on your host:

```bash
sbxw skills import --dry-run   # preview
sbxw skills import             # do it
sbxw skills import --force     # replace skills already in the store
```

Imported skills survive `sbxw rm`. New sandboxes mount the store **read-only**
by default. `skills` in `sbxw.toml` picks another mode, passed as
`sbx create --skills=…`:

```toml
skills = "off"          # only the skills this sandbox's kits provide
# skills = "readwrite"  # the agent may add skills every sandbox then sees
```

Empty (the default) leaves it to sbx and its host-wide `skills.defaultMode`.
It's read at **creation** only.

**Kit or store?** A kit can install anything (packages, binaries) but applying
one recreates the container. The store carries only skill files and costs
nothing to update. A skill with system dependencies (like `/md-to-pdf`, which
needs WeasyPrint) must be a kit; one without belongs in the store.

## Keeping a skill you made

A skill an agent wrote in one sandbox — under the project's
`.claude/skills/<name>/` or the sandbox's own `~/.claude/skills/<name>/` — can
be **kept**, so every sandbox created afterwards has it:

1. Open **Skills** in the web UI's sidebar footer.
2. Under **Your skills**, pick the sandbox and press **Show its skills**.
3. Press **Keep** on the one you want (**Replace** if you kept one of that name before).

sbxw copies the folder to `~/.sbxw/skills/<name>/` on your machine and writes
one kit, `~/.sbxw/state/kits/sbxw-my-skills/`, that every new sandbox gets
whatever its `kits` line says. In the sandbox the files land in
`~/.sbxw/my-skills/<name>/`, with a `/name` command pointing at its `SKILL.md`
— the same arrangement as the bundled kits. **Remove** stops new sandboxes
getting it.

- **Only you keep a skill.** It happens on the web UI's own port, never from a
  sandbox. An agent wrote it and every later agent will follow it: read the
  `SKILL.md` before pressing Keep.
- **Your host's Claude Code never sees it.** The copy is data: it is never
  put in your own `~/.claude/skills`, and nothing in it is executable on your
  machine. Files executable in the sandbox are executable again in the next ones.
- **Text only, small.** At most 200 files and 512 KB; links, binaries,
  `node_modules/` and `.git/` are left out (the dialog says which). A skill
  needing packages belongs in a [kit](#writing-your-own-kit).
- **New sandboxes.** An existing sandbox gets the change only when sbx swaps
  its container. A sandbox made from an [automatic template](configuration.md)
  saved before a Remove still carries the removed skill: rebuild the template.
- Skills in sbx's [shared store](#shared-skills) are not offered: they reach
  every sandbox already.

## Writing your own kit

Every kit sbxw ships or writes is **kit spec v3**
([specification](https://github.com/docker/sandbox-kit-spec/blob/main/docs/spec/SPEC-v3.md)),
and yours must be too. The bundled ones share one layout, worth copying:

```
my-kit/
├── spec.yaml          # the descriptor
└── files/home/agent/  # what lands in the agent's home, as image content
```

```yaml
# syntax=docker/sandbox-kit:3
schemaVersion: "3"
kind: mixin
displayName: my-kit
build: |
  FROM scratch
  COPY --chown=1000:1000 files/home/agent/ /home/agent/
capabilities:
  - type: com.docker.sandbox/network-policy@1
    config:
      runtime:
        allow: [api.example.com]
  - type: com.docker.sandbox/lifecycle@1
    config:
      startup:                          # every boot, alongside the agent
        - command: ["bash", "/home/agent/.local/bin/my-kit-install"]
```

Converting a v2 kit:

| v2 | v3 |
|---|---|
| `name:` | `displayName:` |
| `permissions.network.allow` | `network-policy@1` → `runtime.allow` (or `install.allow` for install hooks) |
| `setup.files` | files under `files/`, copied by `build:` (works with `sbx kit add`), or `lifecycle@1` → `files` (creation only) |
| `setup.startup` | `lifecycle@1` → `startup` |
| `setup.install` | `lifecycle@1` → `install` (runs once, as root by default) |
| `${WORKDIR}` in file content | `${{ kit.env.WORKSPACE_DIR }}` in `lifecycle@1` files |

- Image content keeps its bytes as they are: nothing is expanded. In a
  `lifecycle@1` command or file, sbx expands `${{ … }}`; `$VAR` stays for the
  shell.
- An install hook sees only the environment it names in `env:`; a startup
  hook sees the sandbox's.
- A kit may use `node`: sbxw's base kit installs it before any startup hook runs.
