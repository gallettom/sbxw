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
| `sbxw:agent-skills` | ✓ | `/archify`, `/brag`, `/brag-slim`, `/i-have-adhd`, `/handoff`, `/grill-me`, `/grilling`, `/frontend-design`, `/modern-web-guidance`, `/chrome-extensions`, plus headless Chromium and FFmpeg |
| `sbxw:md-to-pdf-tools` | ✓ | `/md-to-pdf`, with WeasyPrint, poppler-utils and Pillow |
| `sbxw:retro` | ✓ | `/retro`: reads past sessions and proposes what should become a script, hook, skill, rule or kit |
| `sbxw:k8s-tools` | | `kubectl`, `k3d` and `skaffold` in `~/.local/bin` |
| `sbxw:git-identity` | | Your host's Git `user.name` and `user.email` in the sandbox |
| `sbxw:ssh-agent` | | Your host's SSH agent, restricted to signing commits and the common Git hosts |
| `sbxw:dev-ports` | | The agent publishes a dev server on a loopback port sbx picks, without a prompt (sbx 0.53) |
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

Each skill is pinned to a commit (modern-web-guidance to an npm version and
its sha512):

| Command | Source | What it does |
|---|---|---|
| `/archify` | [tt-a1i/archify](https://github.com/tt-a1i/archify) | Validated, interactive HTML architecture, workflow, sequence, data-flow and lifecycle diagrams |
| `/brag`, `/brag-slim` | [latent-spaces/brag](https://github.com/latent-spaces/brag) | A short launch video of the project |
| `/i-have-adhd` | [ayghri/i-have-adhd](https://github.com/ayghri/i-have-adhd) | Answers shaped for ADHD readers. On in every session via a `SessionStart` hook |
| `/handoff`, `/grill-me`, `/grilling` | [mattpocock/skills](https://github.com/mattpocock/skills) | Handoff documents; relentless plan interviews |
| `/frontend-design` | [anthropics/skills](https://github.com/anthropics/skills) | Guidance for distinctive UI design |
| `/modern-web-guidance`, `/chrome-extensions` | [GoogleChrome/modern-web-guidance](https://github.com/GoogleChrome/modern-web-guidance) | Current web platform practice (HTML, CSS, client-side JS, accessibility, performance), searched offline before writing web code; Manifest V3 extensions. **Telemetry removed** |

Plus Playwright's headless Chromium (which has linux-arm64 builds) and FFmpeg.
Skills land in `~/.sbxw/agent-skills/` with thin user-level commands pointing at
them, never in the shared skill store. See
[`assets/agent-skills/README.md`](../assets/agent-skills/README.md).

#### The sandbox browser

That Chromium is also the sandbox's own browser. While the agent uses it, a
globe tab shows on the edge of that sandbox's terminal; click it to watch. The
panel opens beside the terminal, on the side away from it, and pushes the
panes over; without room for both (the panes must keep 480 px beside at least
560 px for the browser) it opens as a dialog instead. A click on the tab hides
it again. The globe button of a running sandbox opens the same **Browser
panel**: the page it is on, live, with its address
always shown at the top. You can click, scroll and type in it, or type an
address yourself. The agent drives the same browser with
`node ~/.sbxw/browser/browser.mjs open <url>` (also `url`, `text`, `eval`,
`click`, `type`, `shot`, `stop`), or with Playwright over
`http://127.0.0.1:9222`.

The picker beside the address lays the page out as a phone (390 px, touch,
a phone's user agent), a tablet (820 px) or a desktop (1280 px); **Auto** fits
the panel. The agent has the same with `browser.mjs viewport mobile` (also
`tablet`, `desktop`, `<w>x<h> [mobile]`, `auto`), and whoever picks a device,
the other sees it: it is kept in the sandbox until changed.

It runs inside the sandbox and goes through its proxy, so the network policy
applies to it, unlike Claude in Chrome, which browses from your own machine.
When the policy refuses a host, the panel lists it with an **Allow** button:
one click writes an allow rule for **that sandbox only**. **Close browser**
quits Chromium and, if you allowed hosts from the panel, asks once whether to
keep those rules or remove them. The browser reopens on the page it was on
when it was closed. ✕ only hides the panel; the browser keeps
running for the agent.

A Chromium the agent launches for itself would be invisible to you, so the
kit adds a Claude Code hook that refuses such a command once (a one-off
`playwright screenshot`, `chromium.launch()` inline or in a script under
`/tmp`) and tells the agent to use the shared browser instead. A project's own
test suite is not affected, and `SBXW_OWN_BROWSER=1` before a command lets the
agent use its own browser when it says why.

Every key typed in the page goes to it, Tab and Escape included; **F6** moves
to the address bar. The panel offers a bare host name only: `localhost`,
`host.docker.internal`, a `.local` name or an IP address is this machine or
your network, and is allowed with its port in Ports & network instead. A rule
is removed at closing only when sbx's listing identified it with certainty;
otherwise the panel says so and leaves it to you.

Chromium's command port (9222) has no password, so it listens on the
sandbox's loopback only and is never published. The panel reaches it through
`sbx exec`, behind the UI token. Sandboxes on this machine only, for now.

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

`kubectl`, `k3d` and `skaffold`, each pinned to a version and checked against
its hash, arch-aware and idempotent, downloaded once per host with the kit
cache. Opt-in. See
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

### `sbxw:dev-ports`

Needs sbx 0.53. The kit carries one grant (`api@1`, `ports.publish` with
`approval: none`) and one script. The agent runs
`node ~/.sbxw/dev-ports/publish.mjs 3000` and gets back an address such as
`http://127.0.0.1:49213` to give you: a host port on the loopback interface
only, chosen by sbx, with no prompt. The port shows in the ports panel like
any other.

- The binding lasts until the sandbox stops and is not restored at the next
  start; the agent is told to publish again.
- A fixed host port still needs you: `[[ports]]` in `sbxw.toml`, or the ports
  panel.
- Opt-in, and only when the sandbox is created: on an existing one, recreate it.
  An sbx older than 0.53 does not know the grant.

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

`agent-skills`, `md-to-pdf-tools` and `k8s-tools` spend their startup
downloading: apt packages, a Playwright Chromium, skill tarballs, Python wheels,
binaries. With `cache = true` (the default), sbxw mounts `~/.sbxw/cache` into
every new sandbox and sets `SBXW_CACHE`. The kits copy from it and fall back to
the network.

- **The daemon fills it, from a sandbox of its own.** When a kit of the
  sandbox being created has not filled the cache yet, or its fill expired, sbxw
  creates `sbxw-cache-fill` in the background: the caching kits, an empty
  workspace, the cache **writable** (`SBXW_CACHE_FILL=1`). Its kits download
  into the cache and mark each kit done, then it is removed. It shows in the
  sandbox list while it runs.
- **Every sandbox you work in gets it read-only**, the one whose creation
  started the fill included: that one installs from the network, the next ones
  from the cache. What one sandbox's agent could write, every later sandbox
  would copy and run.
- One fill at a time; one not done in 30 minutes counts as failed and the next
  sandbox created starts another.
- **apt expires** after `cache_ttl_days` (default 7): the next sandbox drops
  apt's lists and packages and downloads them again.
- **What a kit pins is kept** across that refresh: skill tarballs (by commit),
  Chromium (by Playwright version), wheels, the k8s binaries (checked against
  their hash on every copy). They are downloaded once, not once a week.
- **Everything goes** once the cache is four times `cache_ttl_days` old (28
  days by default), which bounds how long anything written there survives.
- **Only a complete Chromium is copied**: a sandbox created while another is
  still filling the cache downloads its own instead of copying half of one.
- **Per image flavour:** files go under `<arch>-<codename>/` (e.g.
  `arm64-bookworm`), so a `.deb` never reaches the wrong distribution.

```bash
sbxw cache          # each flavour, its size, when apt is refreshed
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
