# Kits and skills

[← Back to README](../README.md)

- [Kits](#kits)
- [Bundled kits](#bundled-kits)
- [The kit cache](#the-kit-cache)
- [Shared skills](#shared-skills)
- [Writing your own kit](#writing-your-own-kit)

## Kits

Kits are `sbx`'s declarative extension point: tools, files, environment,
network rules and startup commands. List them in `sbxw.toml`:

```toml
kits = [
  "sbxw:codemap", "sbxw:agent-skills", "sbxw:md-to-pdf-tools",  # the defaults
  "sbxw:k8s-tools",                                             # opt-in
  "./my-kit",                   # relative paths resolve against sbxw.toml
]
```

**Without a `kits` line**, a sandbox gets `sbxw:codemap`, `sbxw:agent-skills`
and `sbxw:md-to-pdf-tools`. Writing `kits` replaces the defaults; `kits = []`
means none.

A kit reference is one of:

- `sbxw:<name>`, a kit built into the sbxw binary;
- a **directory containing `spec.yaml`** (not a single `.yaml` file);
- a `.zip`, an OCI ref (`ghcr.io/org/kit:tag`), or a git URL.

Validate one with `sbx kit validate <dir>`.

### Kits go in at creation

`sbxw up` passes every kit to `sbx create --kit` (credentials kit first),
because creation applies a kit in one step, with no container swap.

A kit added to `kits` later reaches an existing sandbox on the next `sbxw up`,
via `sbx kit add`. That recreates the container (state kept), so sbxw skips
kits it already applied: it records them itself, and also recognises the paths
`sbx inspect` lists. Since sbx 0.48 this includes kits with
startup commands, like all the bundled ones: their commands run after the add
and on every later start. If sbx still refuses a kit, sbxw tells you to
recreate (`sbxw rm <name>`, then `sbxw up <name>`) and changes nothing — it
never recreates on its own, since anything outside the workspace would be
lost. To force a re-apply, run `sbx kit add <sandbox> <kit>` yourself.

A kit's `permissions.network.allow` domains join the sandbox policy when it's
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
| `sbxw:k8s-tools` | | `kubectl`, `k3d` and `skaffold` in `~/.local/bin` |
| `sbxw:git-identity` | | Your host's Git `user.name` and `user.email` in the sandbox |
| `sbxw:ssh-agent` | | Your host's SSH agent, restricted to signing commits and the common Git hosts |

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

### `sbxw:k8s-tools`

`kubectl`, `k3d` and `skaffold`, arch-aware and idempotent. Opt-in. See
[`assets/k8s-tools/README.md`](../assets/k8s-tools/README.md).

### `sbxw:git-identity` and `sbxw:ssh-agent`

Two kit spec v3 capability kits (sbx 0.48+), opt-in because each hands the
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

## Kits are checked before anything is created

On every `sbxw up`, each of your own local kits (a directory or ZIP, not a
bundled one, not a registry reference) is checked with
`sbx kit validate --offline` (sbx 0.49) before the sandbox is created or
changed. A broken kit fails the bring-up in a second with sbx's message, not
after the image pull. With sbx 0.48, which lacks `--offline`, the check is
skipped. `sbxw doctor` runs the same check.

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

Empty (the default) leaves it to sbx and its host-wide `skills.defaultMode`. The
old `share_skills = false` still works and means `"off"`. It's read at
**creation** only.

**Kit or store?** A kit can install anything (packages, binaries) but applying
one recreates the container. The store carries only skill files and costs
nothing to update. A skill with system dependencies (like `/md-to-pdf`, which
needs WeasyPrint) must be a kit; one without belongs in the store.

## Writing your own kit

The bundled kits use **kit spec v2** (`schemaVersion: "2"`), like the OAuth kit
sbxw generates, except `git-identity` and `ssh-agent`, which use v3
capabilities: `permissions.network.allow`, `setup.files`, `setup.startup`.

- The v2 loader rejects v1 field names. If you adapt a v1 kit, rename all three
  sections.
- `startup` entries are exec-style arrays: `command: ["bash", "…"]`.
- `content` fields allow only the `${WORKDIR}` placeholder. Use brace-free
  `$VAR` for shell variables.
