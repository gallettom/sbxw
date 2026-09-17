# sbxw

An ultra-light Rust wrapper around the standalone **`sbx`** (Docker Sandboxes)
CLI, tuned for running the **Claude Code** agent against a local project with a
browser terminal, host-friendly port aliases, and a restrictive dev network
policy.

It **only ever calls `sbx`** — never `docker sandbox`.

> Built against the `sbx` **0.39** CLI reference
> (docs.docker.com/reference/cli/sbx) and the **0.42/0.43** release notes, and
> it assumes 0.43 throughout — there are no fallbacks for older releases. It
> checks at startup; see below.

## Requires sbx 0.43.0 or newer

`sbxw up`, `sbxw chat`, `sbxw web`, `sbxw env run` and `sbxw prune` run
`sbx version` first and stop with a clear message if it's older.

This used to be a floor of 0.37 with everything newer behind runtime gates, so
one binary could serve three releases. That is gone. Each gate was a branch plus
a fallback at every call site, and every fallback was a second code path nobody
ran — the kind of code that rots silently and is only discovered by the person
it fails on. What sbxw now assumes, unconditionally:

| Feature | Used for |
| --- | --- |
| `create`/`run` `-e KEY=VALUE`, `--env-file` | `[env]` and `env_files` in `sbxw.toml` — baked in at creation, applied to the agent session on every attach |
| `sbx env create --name`, `--env-arg` | `sbxw env run`, which provisions from a committed `sbxenv.yaml` |
| The 0.42/0.43 environment-file format | `sbxenv.yaml` (not hidden), `${{ env.* }}` expressions, paths relative to the declaring file, `skills:` — see [Environment files](#environment-files-sbxenvyaml) |
| `create --skills=off\|readonly\|readwrite` | `skills` in `sbxw.toml` — see [Shared skills](#shared-skills) |
| `sbx prune` | `sbxw prune` |
| Kit spec **v2** (`schemaVersion: "2"`) | the OAuth credentials kit, and the four kits under `assets/` |
| `secret set --sandbox NAME` | the Anthropic secret, without the deprecation warning the old spellings printed |
| `SANDBOX_NAME` | the in-sandbox hooks naming their own sandbox |
| `create --kit` repeated | every configured kit applied at creation, which is the only moment sbx applies one whole |

One 0.39 change is what makes the rest safe to assume: **an unrecognised
command, subcommand or flag is now an error.** Before 0.39 it printed help and
exited 0, so a probe that "worked" told you nothing. Code that can assume 0.39
can trust that a command which succeeded actually ran — which is why the
speculative retries sbxw used to carry (a create retried with fewer kits, a
policy view retried unscoped because the flag might not exist) are gone.

The floor moved from 0.39 to 0.43 because environment files changed shape in
between, in ways no single file can straddle: 0.42 renamed the project file to
`sbxenv.yaml`, stopped expanding `${VAR}` and stopped mounting the file's
directory when `workspace:` is absent; 0.43 resolves relative paths against the
file that declares them and gave every `sbx env` subcommand `--name`. 0.42 also
made published ports default to `tcp4` (sbxw publishes on explicit IPv4
addresses, so nothing changes for its own ports) and refuses sandbox names over
63 characters or ending in a hyphen — sbxw checks that rule itself, in the CLI
and the web UI, before sbx has to.

Two 0.43 behaviours worth knowing: sandboxes made with `sbx create` — which is
how `sbxw up` makes them — **stop by themselves once idle** (restart with
`sbxw up`; ports are re-published on every bring-up), and **MCP OAuth client
secrets** are now stored as `mcp:<server>:client_secret` — one stored under the
old `mcp:<server>.client_secret` name is no longer read and has to be set again
with `sbx secret set mcp:<server>:client_secret`. sbxw stores no MCP secrets
itself, so that one is yours to redo.

The one thing still *probed* rather than assumed is the skills flag of `sbx
create`: it comes from release notes, not the published reference. Since an
unknown flag fails the whole `create`, sbxw reads `sbx create --help` once
before passing it — see [Shared skills](#shared-skills).

To run against an older sbx anyway:

```bash
SBXW_SKIP_SBX_VERSION_CHECK=1 sbxw up neos
```

Expect failures rather than degraded behaviour: the flags above are passed
as-is, and an sbx that doesn't know them now refuses the whole command. This is
a break-glass for one run, which is why it's an environment variable and not an
`sbxw.toml` key — not a property of your project that should be committed and
forgotten.

## What it does

`sbxw up <name> [path]` runs this pipeline (each step maps to an `sbx` call):

1. **Create** — if the sandbox doesn't exist:
   `sbx create claude <path> --name <name>` (extra `--ro DIR` mounts are appended
   as read-only workspaces, i.e. `DIR:ro`). `<path>` defaults to the current
   directory. If it already exists it's reused. Ports, kits, `skills` and
   `[env]` / `env_files` all go in here, because creation is the only moment sbx
   applies them whole. See [Environment variables](#environment-variables).
2. **Network policy** — applies a restrictive local-dev egress allowlist via
   `sbx policy allow network "<list>"` (npm, pypi, packagist, github, docker
   registries, `api.anthropic.com`). Not `**`. **Runs before kits** so a kit's
   download commands have egress.
3. **Kits** — applies each kit in `sbxw.toml`'s `kits = [...]` via `sbx kit add`.
   `kit add` **recreates the sandbox container** (state is preserved), so kits
   already listed by `sbx inspect` are skipped instead of
   blindly re-applied on every `up`. See [Kits](#kits).
4. **Bidirectional code** — the workspace is the agent's Git working tree; edits
   from the agent appear on the host instantly and vice-versa. **Only that
   directory is shared** — the sandbox is a microVM with its own filesystem,
   network and Docker daemon, so nothing else on your host is exposed.
5. **Host aliases** — writes a delimited block in `/etc/hosts` (and, in
   `ip_per_app` mode on macOS, `ifconfig lo0 alias` entries) so you reach apps at
   `http://neos.local:4200` etc. Privileged steps use `sudo` and prompt, so they
   run **before** the daemon detaches, in the terminal you typed `sbxw up` in —
   a background daemon has nowhere to show a password prompt, and sudo's cached
   password expires minutes after it was given. Aliases are *merged* into the
   block, so provisioning a second sandbox neither evicts the first one's alias
   nor needs a password to re-add it. Anything the daemon still could not write
   (an alias added from the web UI's create dialog, say) is reported as a
   warning next to the sandbox — which is created regardless — and parked for
   `sbxw hosts sync`. On macOS the daemon first tries the system's own
   authentication panel (`osascript … with administrator privileges`, shown as
   *osascript wants to make changes*), which is the one password prompt a
   process with no terminal can still put in front of you; dismissing it just
   leaves the alias parked.
6. **Ports** — a *new* sandbox is created with the mappings already attached
   (`sbx create … -p …`), so they're live from first boot. For a reused one,
   sbxw waits until it reports `running` and then (re)publishes each mapping
   with `sbx ports <name> --publish …`. Ports are **not persistent** across a
   stop/restart, which is exactly why this is automated.
   Publishing at creation is all-or-nothing — sbx 409s the whole request if one
   host port is already bound (a dev server you left running on 4200 is enough).
   sbxw won't lose the sandbox over that: it retries the create without the
   mappings and lets the per-port publishing take over, where a conflict is a
   warning naming that one port. Free the port (or change `host_port`) and run
   `sbxw ports <name>` to pick it up.
7. **Web terminal** — backgrounds a daemon serving a browser TTY (xterm.js)
   bridged over a WebSocket to a PTY. Each sandbox has two independent sessions:
   the **Claude** agent (`sbx run`) and a **Bash** shell (`sbx exec -it … bash`),
   switchable from the UI.

`sbxw up` prints the daemon pid + URL and detaches. Use `--tail` to follow its
log, or `--no-web` to attach the agent in the current terminal instead.

With `--no-web` the terminal belongs to the agent, so step 6's port publishing —
which by design finishes *after* `sbx run` has booted the sandbox — doesn't write
to it. Its reports are held and printed when the agent exits. Otherwise they
landed on top of the agent's full-screen UI, and in raw mode (no `ONLCR`) each
newline dropped a line without returning to column 0, stepping the text
diagonally across the screen.

## Commands

| Command | What it does |
|---|---|
| `sbxw up [name] [path]` | Provision + serve. **Omit `name`** to start only the web daemon (browse/create/attach from the UI) — or pass `--add-sandbox` to derive one from the workspace directory name instead (`-copy`, `-copy-1`, ... on a clash with a different path already using that name). |
| `sbxw chat [name]` | Throwaway chat sandbox: same as `up`, but on an empty workspace so the agent has none of your code. **Omit `name`** for a generated `chat-xxxxxx`. |
| `sbxw bash <name>` | Open an interactive bash shell in a sandbox (foreground). |
| `sbxw ssh [name] [-- cmd…]` | SSH into a sandbox as `<name>.sbx`, or run one command in it. `--setup` registers the SSH host block first. See [SSH](#ssh-experimental). |
| `sbxw skills import [--dry-run] [--force]` | Import your host agents' skills into the store shared by all sandboxes. See [Shared skills](#shared-skills). |
| `sbxw web <name>` | Serve the web TTY only (no provisioning). |
| `sbxw ports <name>` | Re-publish the configured ports for a running sandbox. |
| `sbxw ports-ls [name] [--all]` | Show published port mappings for one or all sandboxes. |
| `sbxw ls` | List all sandboxes with status. |
| `sbxw stop <names…> [--all]` | Stop sandboxes (state kept; restartable). |
| `sbxw rm <names…> [--all]` | Remove sandboxes permanently (passes `--force`, so removal proceeds even if a session is attached, which sbx otherwise refuses). |
| `sbxw prune [--since D] [--dry-run] [--yes]` | Remove every **stopped** sandbox — a running one is never a candidate, so this can't take out the one you're in. Lists them and asks first. `--since 168h` keeps anything stopped more recently. |
| `sbxw env run [paths…] [--env-arg K=V]` | Bring a sandbox up from an `sbxenv.yaml` (over `~/.sbxenv.yaml`): `sbx env create` creates it, sbxw adds the policy, credentials, hooks, aliases and browser terminal. Busy host ports are renegotiated **before** anything is created. |
| `sbxw env export [path]` | Write an `sbxenv.yaml` beside the workspace, with every path relative to the file so it is portable. Also available per pane from the web UI's **Env** button. See [Environment files](#environment-files-sbxenvyaml). |
| `sbxw logs <name> [-n N]` | Tail a running daemon's log. |
| `sbxw hosts [show\|sync\|clear]` | Show the `/etc/hosts` block (plus anything a daemon parked), re-apply it — the one place `sudo` can ask for a password — or remove it. |
| `sbxw down [name]` | Kill the daemon for `name`; with no name, kill all daemons **and** remove the `/etc/hosts` block. |
| `sbxw update [--check] [--no-island]` | Install the latest release in place of this binary (or just check with `--check`). On macOS it also refreshes an already-installed `SbxwIsland.app` when the release ships a newer build of it — quitting and relaunching it if it was running; `--no-island` leaves the app alone. |
| `sbxw completion [shell]` | Print `source <(sbxw completion <shell>)` material for bash/zsh/fish/elvish/powershell; see `sbxw completion --help`. |

The web-only daemon's log/pid are keyed as `web` — `sbxw logs web`, `sbxw down web`.

## Web UI

Served at `http://sbxw.localhost:<port>` (default `7681`). From the browser you can:

- **Switch sandboxes** in the sidebar; connect, **stop**, **reload**, or **remove** (✕).
- **Create** a sandbox (＋). One button for both kinds: it opens a chooser that
  spells out the difference — a **workspace sandbox** on a folder of yours, or a
  **chat sandbox** on an empty one — and hands over to that kind's own dialog.
- The workspace dialog has a folder picker and inline **port-forwarding** rows
  (sandbox→host port, optional host IP, optional `/etc/hosts` alias). This goes
  through the *same* provisioning pipeline as the CLI.
- **Star the folders you keep projects under** (☆ on each row of the picker) and
  they become one-click shortcuts above it. See below.
- **Watch the bring-up happen** — the card that appears in the bottom-right
  corner while a sandbox is being created lists the steps it will take and ticks
  them off, with `sbx`'s own output (the image pull, mostly) streaming under
  whichever one is running. See below.
- The **chat sandbox** card is the browser equivalent of `sbxw chat`, with an
  optional name (leave it empty for the generated `chat-xxxxxx`). See below.
- **Route an agent's question to another sandbox**, when one asks for something
  it can't see from its own workspace: a popup shows the question and one button
  per running sandbox, and the answer that comes back is yours to edit, release
  or refuse. See [Asking another sandbox](#asking-another-sandbox-the-relay).
- **Show an agent what it just built**, when it asks: the same popup takes
  screenshots you paste, drop, pick or capture from a window — one, or a whole
  set — previews them, and sends them to the sandbox that asked. Words instead
  of a picture, or a flat refusal, are equally valid answers. See
  [Asking *you* for a screenshot](#asking-you-for-a-screenshot).
- **View / add / remove port mappings** (⇌) per sandbox, including the host IP and alias.
- **Read the project's code map** — the **Code map** button in the **Project**
  panel (📁 beside the sandbox name in a pane's top bar) opens the linked
  markdown under `.sbxw-artifacts/codemap/` as a small vault:
  `[[wiki links]]` are clickable, every section lists what points *at* it
  (backlinks), the outline jumps within a file, search spans the whole map, and
  a **Graph** tab lays the files out as a force-directed graph you can drag.
  A dead link is drawn struck through in red rather than silently as text — a
  map that has drifted from the code says so on sight. Read-only, and served
  from the two endpoints the Project panel already uses, so it reaches nothing
  outside `.sbxw-artifacts`.

  The panel itself opens from 📁 in a pane's top bar, sat against the sandbox
  name rather than with SSH / Env / Reconnect at the other end of that bar:
  those act on the *session* in the pane, while a project is a fact about the
  **workspace** the name belongs to — two sandboxes duplicated from one project
  share the same map. The map is a *button* inside the panel rather than a row
  in its table, because a map is not a file you download: it has no size, and
  "open" is not "download". Its own files are folded out of the list because a real map is
  dozens of markdown files, and listing them buried the deliverables they sit
  beside.
- **Ask for one, when there is none** — the same button reads **Generate code
  map** on a project without one, and starts a *background session* in that
  sandbox that runs `/codemap` (from the [codemap kit](#kits)). It used to sit
  there disabled, naming the command and leaving you to go and type it: the case
  where the button has the most to offer was the one case it did nothing.

  Background, like the lens beside it: writing a map is a long uninterrupted
  read of a whole repository, and taking over the agent pane for it would cost
  you the session you were in the middle of, for output you cannot usefully
  steer — so the pane stays yours, and what you get instead is the sandbox's
  **state**. While it writes, the sandbox's sidebar row carries an amber `map…`
  badge, a corner card names the run, and the button itself becomes its status
  line; the map appearing in the panel is the end of it.

  The agent reports finishing itself, by POSTing to
  `/api/sandboxes/<name>/codemap/done` — the URL arrives in its environment as
  `$SBXW_CODEMAP_DONE`, and the kit's `/codemap` command tells it when to use it.
  That is earlier and better informed than the session's exit status, which
  knows whether a process ended and nothing about whether a map was written; the
  exit is kept as the backstop for the run that never gets that far. Three
  refusals come back before any of it starts, because each otherwise fails
  minutes later as a note on a run that never had a chance: a stopped sandbox,
  one whose agent has no `/codemap` command because the kit is not applied, and
  one already writing — a sandbox holds one codemap run at a time, map or lens,
  because a lens is written *from* the map and must not read one that is moving
  under it.
- **Have the map retold for someone who doesn't read code** — **Write a lens…**,
  in that same panel's toolbar. A code map is written for whoever reads code; a
  product owner, a new joiner or a security reviewer each need the same
  repository told a different way. Type who is reading and what they need —
  *"a product owner: what each part delivers, for whom, and what it costs to
  change"* — and the agent writes it into
  `.sbxw-artifacts/codemap-lenses/<slug>/`, beside the map and never inside it.
  Each lens then appears in the picker next to the map and is read through the
  same viewer, links, graph and all.

  The agent names it, once it has read the map: that directory is the lens's
  title in the picker, and a title cut from the brief's opening words
  (`a-product-owner-what-each`) is no title at all. Naming is a reading task, so
  it belongs to the party that did the reading.

  The browser writes nothing: it hands the brief to the daemon, which runs
  `/codemap-lens` in a background session of the sandbox's agent — the only
  party that can read the map and judge what a product owner needs out of it.
  That session used to be your agent pane, on the argument that a few minutes of
  work you may want to argue with belongs in front of you. It took over the
  session you were in the middle of, and what came back was a document rather
  than a conversation — so a lens now runs where the map does, reports through
  the same `$SBXW_CODEMAP_DONE`, and shows the same way while it writes: a
  `lens…` badge on the row, a corner card, and a picker that reloads itself the
  moment the lens lands. The command it runs comes from the
  [codemap kit](#kits), which spends most of its length on the one failure that
  matters: a business document wants revenue, users and deadlines, a repository
  has none of them, and the answer is to name them as missing rather than to
  fill them in.
- **Export the sandbox as an `sbxenv.yaml`** — the **Env** button in a pane's
  top bar, beside **SSH** and behaving the same way: a card hanging off the
  button, closed by Escape or by clicking away. Live preview, an editable
  folder (where the file goes, and what its paths are relative to), Copy, and
  Save. It reads the ports the sandbox *actually* has, so it exports
  what is running rather than what the config asks for. See
  [Environment files](#environment-files-sbxenvyaml).
- **Inspect the network policy** in that same panel. Three `sbx` calls, because
  no single one answers "what can this sandbox reach?":
  - **Rules** (`sbx policy ls <name> --wide`) — one row per rule with the
    resource it covers, i.e. the actual domains, `allow`/`deny` colour-coded.
    Comes with a filter box, since a global policy runs to a couple of hundred
    rules.
  - **Recent egress** (`sbx policy log <name>`) — the hosts that were actually
    allowed or blocked, with the rule and reason. The layer that answers "why was
    that request refused?".
  - **Policies governing this sandbox** (`sbx policy ls <name>`), folded away:
    one card per policy with its source (`local` / `kit` / `org`) and rule
    counts. A policy scoped to `all` gets a dashed border.
  - plus the **domains sbxw allows on up**, from `sbxw.toml`.

  The sandbox is a *positional* argument to `policy ls`/`log` (unlike
  `policy allow`'s `--sandbox` flag). If a call fails, only its own section goes
  — a `--wide` call that fails still gets you the policy cards. If sbx prints
  something sbxw can't parse as a table, you get its output verbatim rather than
  a misleading empty list, and rows belonging to other sandboxes are filtered out
  with a count of what was hidden.
- **Add and remove network rules** from that panel:
  - the form under the rules takes sbx's own resource syntax
    (`example.com`, `*.acme.dev`, `host:443`, comma-separated) with an
    allow/deny selector. Scoped to this sandbox by default; tick **all
    sandboxes** to write it to the host-wide policy instead — that one asks for
    confirmation, since it governs every sandbox including ones created later.
    Runs `sbx policy allow|deny network [--sandbox <name>] <resources>`.
  - **✕ on a rule** removes it via `sbx policy rm <rule-id>`, after a
    confirmation that names the rule's blast radius. The id comes from the
    listing's *rule-id* column specifically — a policy id would delete every rule
    in that policy — so the button only appears when sbx reports one. Rules from
    an `org` source show 🔒 instead: governance, which sbx won't let you remove.

  Rules changed here are **not** written back to `sbxw.toml`, so the next
  `sbxw up` re-applies the configured allowlist over the top. For a permanent
  change, edit `network_allow` / `network_deny` there.

  > `sbx policy rm`'s argument shape is inferred from its help text, not verified
  > against a live run. If it turns out to differ, the button surfaces sbx's own
  > error verbatim — it can't remove the wrong thing, since it only ever passes a
  > rule id.
- **Reconnect / ↻ (rebuild)** in the terminal bar — the first reopens the pane's
  WebSocket, the second also throws the xterm widget away and builds a new one
  (for a display that has gone bad in ways a reconnect can't clear). Both replay
  the session's last 256 KB, which is *repainted*, never re-asked: the questions
  a terminal is meant to answer — `CSI 6n` from every shell prompt, `CSI c` from
  a TUI starting up — are stripped from the replay. Left in, a fresh parser
  reads them as live requests and answers each one back into the PTY, which at a
  bash prompt means a few hundred `37;3R` typed onto your command line.
- **Toggle Claude ↔ Bash** in the terminal bar — both sessions persist server-side,
  so switching back and forth keeps each one's scrollback and running process.
  **Bash** normally attaches with `sbx exec`, which only reaches a *running*
  sandbox; on a stopped one it connects over SSH instead, since that starts the
  sandbox on the way in. (Previously it just failed, and you had to attach the
  agent first purely to boot the thing.) That fallback needs
  [SSH](#ssh-experimental) set up — the pane tells you so if it isn't.
- **SSH connection details** (SSH button in the terminal bar) for the attached
  sandbox: a popover hung off the button, with one copy button per field of a
  client's *Add SSH connection* form — **Name**, **SSH Host** (`<name>.sbx`), and
  **SSH Port** / **Identity File** shown greyed as *leave empty*, because the
  managed `Host *.sbx` block supplies the user, the key and a ProxyCommand and
  filling those two overrides the only thing that makes the connection work. A
  last row gives the whole thing as one shell command, `ssh <name>.sbx`. The
  fields are copyable separately because they go into separate boxes — pasting
  the command and re-splitting it by hand is the step this replaces. A reference
  card is not a decision, so it dims nothing: click the button again, click
  away, or press Escape.
- **Open the host monitor** (**Monitor Sandboxes**, at the foot of the sidebar)
  in the focused pane: sbx's own all-sandboxes
  dashboard, run in a PTY and streamed to the browser like any other pane, so a
  full-screen TUI works as it does in a terminal. It is *not* a sandbox session —
  it runs on the host, is shared by every viewer, and is filed under a
  pseudo-sandbox (`__host__`) that no real name can collide with, since sandbox
  names may not contain underscores. The Claude/Bash toggles and the SSH button
  hide for it: there is no sandbox behind that pane.

  Clicking it again puts the pane back on the sandbox it took over.

  What it runs is `monitor_cmd` in `sbxw.toml`, as argv — **one fixed configured
  command, deliberately not a "run anything on the host" box**. The default is
  bare `["sbx"]`: with no subcommand the CLI opens its own dashboard. Set it to
  `[]` and the button disappears.
- **See what the account is spending** — the header's right-hand corner carries
  your **Claude subscription usage**: the 5-hour and weekly window percentages,
  each as a gauge, at a size you read rather than squint at.

  The figures are the ones Claude Code's own `/usage` prints — account-wide, not
  per-sandbox, and the same numbers the [island](#dynamic-island-macos) shows.
  They are there without anyone having opened a sandbox or said a word to an
  agent, which is the point: this gauge answers "can I start something long?",
  and that is asked *before* the long thing. It goes amber at 75% and red at 90%,
  where the answer changes. Absent entirely when there is nothing to show — an
  API-key session has no subscription windows at all. And if a window has since
  reset with nothing seen since, the chip fades and says so on hover rather than
  standing behind a figure that is over.
- **Know what you are running** — the two versions in play, **`sbx`** and
  **`sbxw`**, on the last line of the sidebar, under Help. They are baked into
  the page rather than polled, since neither can change under a running daemon,
  and they sit down there rather than beside the usage because a number that
  never moves has no business next to the one you check. `sbx`'s answers *"is
  this old CLI why that failed?"* (see the [floor](#requires-sbx-0390-or-newer));
  sbxw's own is what a bug report needs, and what `sbxw update` moves. `sbx` is
  left out entirely when its `sbx version` printed nothing a version could be
  read out of — sbxw does not get to guess at that.

### Help — an empty sandbox list

The sidebar shows exactly what `sbx ls` reports, so an empty list is ambiguous:
you have no sandboxes, or the CLI can't see the ones you do. The **Help** button
at the foot of the sidebar — under the list it is a question about, and pinned
there whether that list is empty or forty rows long — opens a dialog with the
two host-side fixes, each with a copy button. (The list's own empty state
carries a second way in, since that is where the question actually gets asked.)

1. `sbx login` — an unauthenticated CLI lists nothing at all.
2. Logged in and still empty? Restart the daemon behind the CLI:
   `nohup sbx daemon start >> ~/.sbx/daemon-restart.log 2>&1 &`, then hit
   **Refresh** (the dialog has its own, so you can check without closing it).
   The log file says why if it doesn't come up.

Both run on the machine sbxw runs on — a terminal on the host, *not* a sandbox
pane, which has no `sbx` and no session of yours to log in.

### While a sandbox comes up

Creating a sandbox is one click and, on a host that has never pulled the image,
several minutes. That used to be a single sidebar row saying *Creating…* until
it either turned into a sandbox or turned into an error toast — and a slow
download looks exactly like a wedged one from there.

So it shows the work instead, from a card in the bottom-right corner — the same
stack a published port, a code map being written and a routed sandbox question
already report from, because a bring-up is the same kind of thing: work running
behind you while you carry on. The daemon announces the steps **before it starts
the first one**, and the card draws all of them, the ones still to come
included:

```
sbxw  neos                     Creating… 2/5
      ✓ Preparing the workspace
      ● Creating the sandbox
          pulling docker.io/…/sandbox-claude: 214.7MB / 512.3MB
        Waiting for the sandbox to start
        Installing the agent tooling
        Publishing ports and host aliases
```

- The steps are the pipeline's own, so the list says what the pause is *for*:
  waiting for the container to report `running` is a different wait from
  installing hooks into it, and both are different from an image arriving over
  the network.
- Two of them are conditional in the same way the pipeline is — the network
  policy step only appears when `sbxw.toml` has one, and *Applying kits* only
  for a sandbox that already existed, since a fresh one gets its kits from
  `sbx create --kit` inside the create step.
- The line underneath the running step is **`sbx`'s own output**, forwarded as
  it arrives. This is the point of the exercise: an image pull that is moving
  says so, several times a second.
- It survives a reload. The steps come over the same SSE stream as everything
  else in the UI (`/api/stream`, a `provision` event), so a tab that was opened
  or refreshed mid-bring-up picks the card back up at the next step — which is
  exactly what you do when a creation seems stuck.
- Every way in gets it: the create dialog, the chat dialog, **Duplicate**, and
  the island's ephemeral chat all go through the one pipeline, and it is the
  pipeline that reports.

The CLI is untouched by this: with nobody listening, `sbx create` keeps the
terminal and prints as it always did. It is only when a browser is watching
that its output is piped and forwarded — which also, finally, lets a failed
`sbx create` say *why* in the UI instead of just "exited with status 1".

### Favourite folders

The picker opens at `$HOME`, and most people keep their projects two or three
levels below it — the same two or three clicks before every single sandbox.
Every folder in the listing carries a **☆** on its right: click it to pin that
folder, without having to walk into it first. Starred folders show up as chips
above the picker, and clicking one drops you straight into it with its
subfolders listed. The part that actually differs each time — *which* project —
is then one more click, which is the whole point: you star the root, not the
project.

- The list lives on the host, in `~/.sbxw/state/favourites.json` (a plain JSON
  array of paths, hand-editable). Not in the browser: these name directories on
  *that machine*, so they have to survive a different browser, a cleared
  profile, or the tab being opened from another device.
- Paths are stored canonically, so `~/dev`, `~/dev/` and a symlink to it are one
  favourite — and the star lights up whenever you land there, however you got
  there.
- Two favourites ending in the same folder name (`~/work/projects` and
  `~/perso/projects` — exactly the pair someone stars) get their parent folded
  into the chip. Only the clashing ones: lengthening every label to
  disambiguate two of them makes the row harder to scan.
- A starred folder that isn't there right now — an unplugged drive, a moved
  project — is shown greyed rather than dropped, and can still be unstarred.
  The list is yours; silently editing it is how a temporarily unreachable
  favourite disappears for good.
- Twelve maximum. Past that the row stops being a shortcut and becomes a wall of
  near-identical names.

## Chat sandboxes

Sometimes you just want to talk to an agent, not point it at a codebase. A chat
sandbox is a normal sandbox whose workspace is a **fresh empty directory**
(`/tmp/sbxw-chat/<name>`) instead of one of your projects, so the only files the
agent can see are the ones sbxw puts there itself — it has none of your code to
read or edit.

```bash
sbxw chat                 # throwaway sandbox with a generated chat-xxxxxx name
sbxw chat brainstorm      # ...or name it yourself
sbxw rm brainstorm        # removes the sandbox and its empty workspace
```

The web UI's **Chat sandbox** card, under the sidebar's ＋, does the same thing
(`POST /api/sandboxes/chat`); both go through one shared code path. It opens a
small dialog where the name is optional — leave it empty and you get the same generated `chat-xxxxxx`, or type
one to get `sbxw chat brainstorm`'s result from the browser. The empty workspace
is deleted when the sandbox is removed, from either the CLI or the UI.

In the sidebar, chat sandboxes are clustered under a **💬 Chats** group, the same
way sandboxes sharing a workspace are grouped under their folder name. Each chat
has its own throwaway workspace, so path-based grouping can't catch them; the
API flags them with `chat: true` instead.

Everything else is a normal sandbox: a chat sandbox still reads your
`sbxw.toml`, so it applies the same kits and publishes the same `[[ports]]`. If
a project sandbox already holds one of those host ports, sbx's conflict recovery
gives the chat sandbox a different one.

## Asking another sandbox (the relay)

Sandboxes are isolated on purpose, which is also their limitation: the agent
working on the frontend cannot see the API repo next door, so it guesses, or it
stops and asks *you* to go and look. The relay is the third option — it asks the
other sandbox, and **you decide every hop**.

```
sandbox A ──ask──▶ sbxw ──▶ 🧑 popup ──route──▶ sandbox B
                                 ▲                  │
sandbox A ◀──approve── 🧑 review ◀──────answer──────┘
```

Nothing here happens on its own. There is no timeout that picks a recipient, and
no path where an answer reaches the asker without someone clicking a button.

**The agent's side.** The agent asks **on its own**, without being told to. Every
sandbox gets an MCP server (`sbxw-relay`, registered at user scope in
`~/.claude.json`) exposing three tools — `ask_other_sandbox`,
`ask_user_for_screenshot` and `check_sandbox_question` — plus the same thing as a
CLI at `~/.sbxw/relay.js`:

```bash
node ~/.sbxw/relay.js ask  "what shape does GET /v1/orders return in staging?"
node ~/.sbxw/relay.js shot "the settings header at mobile width, after I moved the save button"
```

It is a tool and not just a CLI for a reason worth stating, because it looks like
duplication. With only the CLI and a note in the agent's memory, a session that
had searched its workspace, established that the code it needed lived in a repo
that wasn't mounted, and was about to say so, reached instead for the nearest
thing *in its tool list* (a codebase search) and offered that to the user. The
relay never came up. A tool is weighed every turn; a paragraph competes with the
whole conversation. The memory block was also rewritten: it used to open with
"use it sparingly", which reads the same whether or not the case in front of it
is the one worth spending on. Restraint now lives in the tool's description,
where it is read while deciding to *call* it rather than while deciding whether
to consider it at all.

The call parks for ~90 s and then returns whatever the request has become — the
approved answer, a refusal, or "still open, pick it up later with
`check_sandbox_question`". It is bounded rather than blocking so an agent's tool
call never hangs on a human who has stepped away. If the answer is approved while
nobody is waiting on it, sbxw types it into the asking session instead, the same
way the island's composer does.

**Your side.** A popup opens in the web UI with the question and one button per
running sandbox. Click one and the question is typed into that sandbox's agent,
framed as untrusted data with the id it must answer with.

The popup then **gets out of the way**: the wait is on an agent now, not on you,
so the request shrinks to a card in the bottom-right corner (next to the
background-job indicators) and you carry on working in any other sandbox. Click
the card to go back to it — to re-route it elsewhere or refuse it — and **the
answer raises it again by itself**, in the foreground, when it lands. A routing
that fails to deliver comes back the same way, with the reason.

The answer arrives in the popup **editable** — trim it, cut a secret out of it,
or replace it entirely — and only *then* does the asker receive it.

- **Refuse** is a full stop: nothing is released, then or later, and the asking
  agent is told not to re-send it. (An ignored request just gets asked again; a
  refused one doesn't.)
- **Later** (or `Esc`) leaves the request open and parks it behind a header
  badge. Click the badge to come back to it. The badge counts only what is
  waiting on *you* — a request already out with a sandbox has its corner card
  instead.
- You can also **answer it yourself** without involving a second sandbox — type
  into the box under a pending question and send.
- Routing to a sandbox whose agent can't be started bounces the request back to
  you with the reason, rather than looking delivered.

**What a sandbox cannot do**, which is the point of routing everything through a
person: name its own recipient, list or read anyone else's requests, see an
answer a human has not released, or reply to a question it was not handed. The
daemon can't verify which container an HTTP call came from, so a sandbox's claim
about who it is is taken at its word — that is only safe because nothing acts on
it alone. State lives in memory (`src/relay.rs`); a settled request is kept 30
minutes for a late pickup, an unanswered one 6 hours.

### Asking *you* for a screenshot

The same queue carries a second kind of request, for the thing no other sandbox
can supply: a look at your screen.

An agent cannot see what it just built. It changed a layout, moved a button,
adjusted spacing — and everything it *can* check says yes: the code compiles,
the diff is what it intended, the tests pass. "The code is correct" and "it looks
correct" are different claims, and only one of them is available from inside a
container. So the agent can ask to be shown:

```
sandbox A ──shot──▶ sbxw ──▶ 🧑 popup ──paste / drop / capture ×N──▶ sandbox A
```

There is **no routing step**. A screenshot request is never handed to another
sandbox — nobody else is looking at your screen — so it has one live state and
two outcomes: you send something, or you refuse.

**Your side.** The popup shows what the agent wants to see, and takes images
four ways: **paste from the clipboard** (a button — where your OS screenshot key
already put it; ⌘V works too), **drop** files on it, **choose** them, or
**capture a window** right there, through the browser's own picker. The clipboard
and capture buttons appear only when the tab may use them, which means reaching
sbxw on localhost rather than a LAN address. Whatever arrives is previewed before it goes —
you send what you can see — and scaled to 1600px on the way out, since a
screenshot is read for its layout and not its pixel grid.

**One answer, as many pictures as it takes.** Every gesture *adds* rather than
replaces, up to six images and 16 MB of them: a before and an after, the same
screen at two breakpoints, the three steps of a flow. They are numbered on
screen in the order the agent will receive them, each with its own ✕, and they
settle the request together — one interruption, however many views it took. The
agent is told there are several and reads them in that order, which is what
makes "before" and "after" mean anything.

Two other answers are just as good, and the popup treats them that way:

- **Describe it in words** instead. The agent is told plainly that it was
  answered in prose rather than shown a picture, so it works from the
  description instead of hunting for a file.
- **Refuse.** An ordinary answer, not a failure — the agent is told to carry on,
  to say what it changed and what it expects, and to let you correct it. It is
  also told not to ask again for the same view.

**What the agent gets.** Through MCP, the images come back *in the tool result*,
one block each, so the agent actually looks at them — which is the whole reason
the MCP server exists next to the CLI. They are also written to
`~/.sbxw/shots/<request-id>-1.png`, `-2.png`, … inside the asking sandbox, so a
later turn can re-read them without interrupting you a second time. The daemon
never writes into a sandbox to do this: the base64 travels in the approved reply
and the sandbox's own CLI writes the files, so nothing lands on that filesystem
that did not go past you first.

The same rules as a question apply to the images: they are held until you
release them, a refusal discards them, and only the sandbox that asked can ever
receive them. The daemon never decodes them — it checks each envelope (a `data:`
URL, one of PNG/JPEG/WebP, under the size cap) and the set as a whole (at most
six, 16 MB together), then passes the bytes on.

## Dynamic Island (macOS)

An optional native companion app, **sbxw Island** (`macos/SbxwIsland`), turns
your Mac's menu bar / MacBook notch into a Dynamic-Island-style panel that keeps
track of every session — inspired by [vibeisland.app](https://vibeisland.app).
Each session shows a live state (**working** · **waiting for input** · **idle** ·
**ended**) with rich context: the last prompt you sent, the agent's current
activity, an agent tag, and elapsed time.

The notch shows a **persistent pill** hanging from it whenever something is
happening — an agent glyph, the active session's task, and a session count
(e.g. `👾 fix auth bug  3`) — and stays clean (hidden) when everything is idle.
On top of that:

- a **state change drops a toast** (full for 1 s, then a compact pill for 3 s,
  then it disappears back to the summary);
- when the agent **asks a question** (via Claude Code's `AskUserQuestion` tool),
  the notch expands into an **interactive card** showing the question, a
  decision table (each option's description), and a button per option
  (**⌘1/⌘2/…**) — picking one sends the answer straight into the session. A
  prompt with several questions is walked step by step (**1/2**, **⌘←** to go
  back) and submitted in one go once the last one is picked;
- **a sandbox asking another one gets its own card.** When an agent reaches for
  the [relay](#asking-another-sandbox-the-relay), the notch expands into the
  question and one button per running sandbox (**⌘1/⌘2/…**) — picking one sends
  it. When the answer comes back the card returns, showing what was written and
  offering to release it or refuse. In between, while the question is out with
  another agent, the notch says nothing: that wait belongs to an agent, not to
  you, and a card you cannot act on has no business over the menu bar. It shows
  up as a line at the top of the hover list instead, which is also where a card
  closed with ✕ ("later") waits.

  The island **will not release an answer it cannot show you in full.** The
  browser popup lets you edit one before it goes — trim it, cut a secret out of
  it — and the notch has no editor, so one click there would send text verbatim.
  Past ten lines the card drops the button and points at the browser instead.

  **A sandbox asking for a screenshot gets a card of its own**, with no target
  buttons on it: there is nobody to route it to. It says what the agent needs to
  see and offers the two things the notch can honestly do — hand you to the
  browser, where an image can actually be pasted, or refuse from right there.
  Declining needs no editor, and an agent that is refused stops waiting.

  That card also **puts itself away** after a few seconds, which no other card
  does. The others hold the notch because they can be answered *on* it; this one
  cannot, and it is asking for a picture of the very screen it is sitting on — a
  panel left over the menu bar while you go and capture a window ends up in the
  photograph. It announces, then retracts to the line at the top of the hover
  list, which stays the way back to it.

- **several agents in one sandbox each get a row, and the island says which is
  which.** A container can hold more than one Claude Code session — the one sbxw
  attached, plus anything started over SSH (Claude Desktop, an editor, a shell
  on `<name>.sbx`). They all read the same in-sandbox `~/.claude/settings.json`,
  so they all report to the daemon; sessions are keyed by Claude Code's own
  `session_id` so they no longer overwrite each other's state.

  Which is which is **established, not guessed**, from three signals — any one
  marker of a client is enough, and the working directory settles the rest:

  1. **The process the client runs in the container.** sbxw's own session is
     `node < claude`; Claude Desktop's is `node < 2.1.222 < server` — it runs its
     own server inside the sandbox, the way an editor's remote extension does.
     An `sshd` ancestor counts the same way, for a plain `ssh <name>.sbx`.
  2. **sshd's environment** — `SSH_CONNECTION` / `SSH_CLIENT` / `SSH_TTY`,
     conclusive when present. Claude Desktop sets none of them, so their absence
     proves nothing. (`SSH_AUTH_SOCK` is deliberately excluded: sbx forwards an
     agent into every sandbox, so it is always set and would label everything
     remote.) Names are reported, never values — `SSH_CONNECTION` carries the
     client's IP.
  3. **The working directory.** sbxw records the workspace each sandbox was
     created with and the agent it attaches starts there, while a client lands
     wherever it chose (`/home/agent/workspace` for Claude Desktop). Needs no
     hook support, so it also covers sandboxes provisioned before any of this.

  Rows are badged `tty` or `Desktop` accordingly, and **clicking a `Desktop` row
  brings the Claude client forward** rather than the browser terminal, which
  holds the *other* agent. The daemon's own value is `remote`, which is broader:
  it also covers a plain `ssh <name>.sbx` from a terminal or an editor. The
  island names the case that actually occurs and reads as the thing you would
  switch to; the wire stays accurate underneath.

  The hook is rewritten into the container at provisioning time, so a sandbox
  that is merely re-attached keeps the one it was created with. When the daemon
  sees an event from a hook too old to answer the question it says so once, and
  names the fix (`sbxw up <name>`).

  Only the `tty` session can be typed into from the island: sbxw holds one PTY
  per sandbox and that is the session it drives. An `ssh` row shows its prompt
  with the options greyed out and points you at the client. When the evidence is
  missing — an unreadable `/proc`, a hook older than this, or two sessions both
  claiming the tty — *neither* row is answerable, because answering the wrong
  terminal answers someone else's question. The daemon enforces the same rule
  (`409`), so an older island build cannot type past it either;
- **hovering** the notch reveals the full list — with each session's elapsed
  time and, at the top, your **Claude subscription usage** (5-hour and weekly
  window %) — auto-hiding 1 s after you leave (5 s with a row open, so a reply
  can be finished after the pointer wanders off). The reveal target is **the
  notch itself**, not the whole bubble: the collapsed island hangs 280 pt wide
  over the menu bar of whatever app you are in, and crossing the top edge on the
  way to *that* app's menus used to unfold it in your face. Aim at the notch and
  it opens; pass beside it and it stays put. It is also **click-through until it
  has something to click** — the collapsed pill and the toasts announce, they
  don't offer, so clicks fall through to the menu bar underneath; only the list
  and a question card take the pointer. And a **click anywhere else retracts the
  open list at once** — the timer is for a pointer that wandered, a click is a
  decision. (A question card is exempt: it stays until answered or dismissed with
  its ✕, so a stray click can't lose a prompt.) A click-through window receives no
  mouse events at all, so both the reveal and that retraction are driven by
  `NSEvent` monitors rather than by the view's own hover. Mouse monitoring needs
  no Accessibility permission — only keyboard monitoring does;
- every row carries a **chevron** that opens it: Claude's **full reply** and a
  **field to write back into that sandbox**, without a browser tab or a terminal.
  See below;
- a **＋ New chat** row at the bottom of that list starts a *fresh* throwaway
  chat agent without opening a browser or picking a workspace. See below;
- each row waiting on you carries a **✕** to dismiss it, and a **Clear all N**
  strip appears above the list once more than one is pending — dismissing is what
  takes a session off the collapsed notch, and opening a sandbox counts as
  dismissing it;
- once a turn ends, every surface that captions that session — the row, the pill
  under the notch, the mini toast — carries **what Claude actually answered**
  rather than "idle" or the prompt you sent; **opening the row** shows the reply
  in full. The prose comes from the
  `last_assistant_message` Claude Code puts on its own `Stop` event — no
  transcript reading, and it arrives even through a hook script installed before
  this feature existed. A **structured question** still outranks it — that text is
  what you have to act on — but a session nudging you *about* an answer it already
  gave shows the answer. Nothing stale can leak through: a new prompt clears the
  reply, so a session only carries one if a turn has ended since you last spoke.

### Chatting from the notch

Two gestures, and the difference between them is the point:

- **Open a row** (the chevron) to write into a sandbox that already exists. The
  drawer shows Claude's full reply and a field under it; sending keeps the field
  open, because the answer arrives right above it.
- **＋ New chat** starts a *new* throwaway sandbox — `ephemeral-chat`, then
  `ephemeral-chat-2`, `-3`, … — numbered by availability, so a name freed with
  `sbxw rm` is offered again rather than the counter climbing forever. Carrying
  on an existing conversation is the row drawer's job, one click away on that
  chat's own row.

Because every ＋ costs a container, the composer **warns from four sandboxes up**
that each one holds disk and memory, and points at the two ways out (reply in a
chat you already have, or remove what you're done with). Sandboxes are cheap to
make and not free to keep.

Both gestures are the same daemon call, `POST /api/chat/push` — the island has no
sandbox picker and no terminal, so everything between "you typed a question" and
"the agent is reading it" happens server-side: provision if missing, attach the
agent, wait for its TUI, type, submit. Three details that are easy to get wrong:

- **It waits for the terminal to go quiet before typing**, and again before
  pressing Return. Quiescence is measured on the session's output stream, not
  the replay ring buffer — that buffer is capacity-bounded, so once full its
  length stops changing and silence becomes indistinguishable from a flood.
- **Return has to arrive as its own keystroke.** Claude Code reads a burst of
  closely-spaced bytes as a *paste*, and a newline inside a paste is inserted
  into the message instead of sending it — the text lands in the box and just
  sits there.
- **A warm session is not charged for a cold start.** Typing into a sandbox whose
  agent is already attached and drawn skips the `sbx ls` existence probe (the
  live PTY is the proof) and times silence in ~180 ms rather than the 900 ms a
  first frame needs — the wait that made a second message feel as slow as the
  first. To stay honest at a short window it waits for the echo to *start* before
  timing its silence: a PTY that hasn't turned the write around yet reads as
  quiet, and Return would join the paste. The echo window itself stays looser
  (350 ms), since a long message comes back in chunks.

Creating a chat is still slow (a sandbox has to boot); the composer shows a
spinner, and keeps your text on failure so it can be retried.

**Subscription usage arrives two ways, and the header needs both.** One value
(`/api/usage`), one precedence rule — last writer wins — and two writers:

- **Each sandbox forwards what Claude Code already told it.** sbxw installs a
  `statusLine` command (`assets/usage-statusline.js`) that Claude Code invokes
  with a structured JSON payload on stdin (per its
  [statusline contract](https://code.claude.com/docs/en/statusline)); the script
  forwards the `rate_limits.{five_hour,seven_day}.used_percentage` it is given
  (`POST /api/usage`, throttled). Free, and the freshest thing going while an
  agent is actually working.
- **The daemon asks for itself, every five minutes — through a sandbox.** It runs
  `sbx exec <sandbox> -- curl https://api.anthropic.com/api/oauth/usage`, which
  costs no tokens and touches no model. This is what makes the gauges right on a
  daemon that has never run an agent: the statusLine says nothing until somebody
  sends a message, so on its own it leaves the number you check *before* starting
  work empty until you have started it.

  **Why through a sandbox, and not from the host?** Because that request needs an
  OAuth token with the `user:profile` scope, and on a Mac there isn't reliably
  one to be had: `CLAUDE_CODE_OAUTH_TOKEN` from `claude setup-token` is
  `user:inference` only and answers `403`; the keychain copy goes stale between
  Claude Code runs and answers `401`; the credentials injected into a sandbox are
  not valid outside it. A request *leaving a sandbox* is authenticated by the
  sandbox proxy with the account's own credentials at full scope — the same way
  every agent already reaches Anthropic — so it needs no token on the host at
  all. Every running sandbox is tried until one answers, and failures back off
  (doubling to half an hour) because the endpoint rate-limits.

Shown for Pro/Max only — API-key auth has no subscription windows, and the poll
does not run under `--use-api-key`. The island and the web header both read the
same `/api/usage`, so they cannot disagree.

**Session state comes from Claude Code hooks, not terminal scraping.** At
provisioning time sbxw installs a small hook (`assets/status-hook.js`) into each
sandbox that POSTs every lifecycle event to the daemon over
`host.docker.internal`. This yields *trusted, structured* state — no guessing
from the terminal:

| Hook event | State |
| --- | --- |
| `SessionStart` | `idle` |
| `UserPromptSubmit` | `working` (captures your prompt) |
| `PreToolUse` (`AskUserQuestion`) | `attention` + structured prompt (every question of the call) |
| `PreToolUse` / `PostToolUse` (other) | `working` (tool as activity) |
| `Notification` | `attention` (permission / idle nudge) |
| `Stop` | `idle` |
| `SessionEnd` | `exited` |

It's powered by these daemon endpoints:

- `GET /api/sessions` — rich snapshot of current sessions.
- `GET /api/events` — a Server-Sent Events stream of rich session updates, one
  per hook-driven transition.
- `GET /api/hook/log` — the recent raw hook events (inspection/debugging).
- `GET /api/sandboxes` — polled so running sandboxes appear right away as `idle`.
- `POST /api/answer` / `POST /api/input` — send a menu choice (or raw bytes)
  back into a session's PTY.
- `POST /api/chat/push` — `{ "text": "…", "name"?: "sandbox", "fresh"?: true }`:
  submit a message to a chat agent, creating the sandbox and attaching its
  session first if needed. `name` types into that sandbox; `fresh` mints the next
  free `ephemeral-chat[-N]`; neither falls back to the shared `ephemeral-chat`
  (what older island builds send). See
  [Chatting from the notch](#chatting-from-the-notch).

Running sandboxes appear as `idle` immediately; live state flows as soon as the
in-sandbox agent emits hook events (the daemon must be reachable from the
sandbox at `host.docker.internal:<port>`, which sbxw allows automatically).
Build and usage instructions are in [`macos/README.md`](macos/README.md).

## Installation

**Prerequisites:** the standalone [`sbx`](https://docs.docker.com/reference/cli/sbx)
CLI on your `PATH` (`sbx version` should work), and `sbx login` done once.
Building from source also needs a Rust toolchain.

### Option A — install script (release binary)

Downloads the prebuilt binary for your OS/arch into `/usr/local/bin` and the
bundled kits into `~/.local/share/sbxw/kits`. The web UI is embedded in the
binary, so that's all you need.

```bash
curl -fsSL https://raw.githubusercontent.com/gallettom/sbxw/main/install.sh | sh
# pin a version:    | sh -s v1.0.0
# custom dir:       SBXW_INSTALL_DIR=$HOME/.local/bin   ... | sh
# leave SSH alone:  SBXW_SETUP_SSH=0                    ... | sh
```

This requires a published [GitHub release](https://github.com/gallettom/sbxw/releases).
If there isn't one yet, use Option B.

### Option B — build from source

```bash
git clone https://github.com/gallettom/sbxw.git
cd sbxw
cargo build --release
# binary at ./target/release/sbxw — copy it onto your PATH if you like.
# /usr/local/bin is root-owned, so use sudo:
sudo install -m755 target/release/sbxw /usr/local/bin/sbxw
# …or install without root into ~/.local/bin (ensure it's on your PATH):
#   mkdir -p ~/.local/bin && install -m755 target/release/sbxw ~/.local/bin/sbxw
```

## Quick start

```bash
# one-time, in your project
sbx login
cp sbxw.toml.example sbxw.toml      # edit ports/aliases for your project

# from your project root (e.g. the NEOS repo)
export ANTHROPIC_API_KEY=sk-ant-...        # optional, see Auth below
sbxw up neos .                             # or: sbxw up neos /path/to/repo
# open http://sbxw.localhost:7681  → talk to Claude in the browser

# …or just start the web daemon and create sandboxes from the UI:
sbxw up
```

(If you built from source and didn't copy the binary onto your `PATH`, use
`./target/release/sbxw` instead of `sbxw`.)

Inside the sandbox, start your servers bound to **0.0.0.0** or the published
ports won't be reachable:

```bash
ng serve --host 0.0.0.0 --port 4200
symfony serve --listen-ip=0.0.0.0 --port=8000   # or php -S 0.0.0.0:8000
```

## Auth (read this — it's the gnarly bit)

`sbx run`/`create` have **no `--env`**, and there is **no "start without
attaching"** command. So an arbitrary env var (your `CLAUDE_OAUTH_TOKEN`) cannot
be injected *before* the agent launches. The wrapper offers three paths, best to
worst:

1. **API key (confirmed, recommended).** `sbxw up … --use-api-key` reads
   `ANTHROPIC_API_KEY` and stores it as a **global** `anthropic` secret (value
   piped via stdin, never in argv) — `sbx secret set anthropic`, where global
   is the default scope. The agent auto-authenticates.
2. **OAuth token.** If `CLAUDE_CODE_OAUTH_TOKEN` (or `CLAUDE_OAUTH_TOKEN`) is
   set, sbxw writes `~/.claude/.credentials.json` inside the sandbox so the
   agent is authenticated from first launch. On **create** and on existing
   **stopped** sandboxes this goes through a **mixin kit** (`--kit` /
   `sbx kit add`); on a **running** sandbox the file is refreshed directly
   via `sbx exec` instead, because `sbx kit add` recreates the container and
   would kill attached sessions. The canonical variable is
   `CLAUDE_CODE_OAUTH_TOKEN` (from `claude setup-token`); `CLAUDE_OAUTH_TOKEN`
   is accepted as an alias.

   That kit lives at `~/.sbxw/state/kits/<name>-oauth/` and **stays there for
   as long as the sandbox does** — `sbxw rm` is what deletes it. It used to be
   a temp directory removed seconds later, which is a trap:
   a container swap recomposes the sandbox from its template and
   re-resolves every kit applied before it *by its original path*, so a kit
   directory that no longer exists makes every later `sbx kit add` fail — and
   the error names the kit you were adding, not the one that actually went
   missing:

   ```text
   ERROR: re-resolve original kit 0 ("/var/folders/…/sbxw-oauth-kit-36226"):
          kit reference "…": path does not exist
   ```

   Sandboxes created by an earlier sbxw already carry such a reference. You
   don't need to recreate them: `sbxw up` recognises exactly this failure,
   restores the kit at the path sbx is asking for, and retries once. The file
   holds a live OAuth token, so it is written `0600` in a `0700` directory.
3. **Interactive.** Just run `/login` in the web terminal.

Note: host env vars are **not** auto-injected into sandboxes. An exported
`ANTHROPIC_API_KEY` does not reach the sandbox by itself — use `--use-api-key`
(which stores it via `sbx secret set`), migrate it with `sbx secret import`, or,
for non-secret values, put it in `sbxw.toml`'s `[env]`.

## Kits

Kits are `sbx`'s native, declarative extension point (tools, files, env, network,
startup commands). List them in `sbxw.toml`.

**They go in at creation.** `sbxw up` passes every configured kit to
`sbx create --kit` (repeated once per kit, credentials kit first), because
creation is the only moment sbx applies a kit *whole*. Adding one afterwards
with `sbx kit add` is refused outright if the kit declares startup
commands — the recreate flow behind it doesn't run them, so rather than apply
half a kit, sbx tells you to recreate the sandbox:

```text
ERROR: kit "md-to-pdf-tools" declares commands.startup, which the kit-add
       recreate flow does not yet apply; recreate the sandbox from scratch
       via `sbx rm` + `sbx create --kit` to use this kit
```

All four bundled kits declare startup commands — that is how they install
anything — so this is the normal case, not a corner one. **Adding a kit to
`sbxw.toml` for a sandbox that already exists therefore means recreating it:**
`sbxw rm <name>` then `sbxw up <name>`. sbxw says so and changes nothing rather
than putting the sandbox through a container swap that cannot succeed; it never
recreates on its own, since anything outside the workspace mount would be lost.

Kits *without* startup commands are still added in place on `sbxw up` (this is
how the OAuth credentials kit reaches a stopped sandbox). That path recreates
the container (state preserved) and composes the kit's own network
rules into the sandbox policy, so sbxw skips kits `sbx inspect` already lists
instead of re-applying them every `up`. To force a re-apply after editing such a
kit, run `sbx kit add <sandbox> <kit>` yourself:

```toml
kits = [
  "/abs/path/to/sbxw/assets/k8s-tools",   # relative paths resolve against sbxw.toml
]
```

A kit reference is a **directory containing `spec.yaml`** (not a single `.yaml`),
a `.zip`, an OCI ref, or a git URL. Validate one with `sbx kit validate <dir>`.

Bundled kits:

- **`assets/k8s-tools`** — installs `kubectl` + `k3d` + `skaffold` into
  `~/.local/bin` (arch-aware, idempotent).
- **`assets/headroom`** — installs [Headroom](https://github.com/chopratejas/headroom)
  (`headroom-ai[proxy]`), a local context-compression proxy, and enables its
  durable Claude Code integration (`headroom init --global claude`) to **cut token
  usage** (claimed 60–95% fewer tokens). See `assets/headroom/README.md`.
- **`assets/md-to-pdf-tools`** — ships the `/md-to-pdf` skill itself
  (user-level, works regardless of which project is mounted) plus the
  WeasyPrint + poppler-utils + Pillow stack it needs, so the skill is
  available and first invocation has no install step. See
  `assets/md-to-pdf-tools/README.md`.
- **`assets/codemap`** — ships the `/codemap` command, a format reference, an
  offline checker, and three Claude Code hooks, so an agent **reads a
  repository's code map before its source** and knows how to write one when
  there is none. The hooks are what stop that from being advice: the map's index
  is injected into the session at `SessionStart`, recalled before the session's
  first `Grep`/`Glob`, and — in a repository with no map, after a session that
  demonstrably read the tree — a `Stop` hook asks the agent to *offer* one, once
  per repository per 12h. It asks for the offer and forbids the map: writing it
  stays the user's call.
  The map is markdown in [lat.md](https://github.com/vercel-labs/lat.md) format
  — `[[wiki links]]` between sections, links into source symbols, `@lat:`
  comments tying code back to the idea it implements. `/codemap-lens` comes with
  it: the same map retold for one reader, into
  `.sbxw-artifacts/codemap-lenses/<slug>/` — what the web UI's *Write a lens…*
  button runs. Both are what the web UI's Code map panel starts, each in a
  background session, and both know to report back to `$SBXW_CODEMAP_DONE` when
  the document is written.
  Needs no network. See `assets/codemap/README.md`.

The domains a kit declares under `permissions.network.allow` are composed into
the sandbox policy when the kit is added; domains a kit does *not*
declare (e.g. apt mirrors) still need adding to `sbxw.toml`'s `network_allow` —
see each kit's README.

The four bundled kits are **spec v2** (`schemaVersion: "2"`), like the OAuth
kit sbxw generates: `permissions.network.allow`, `setup.files`, `setup.startup`.
The v2 loader rejects v1 field names outright, so a spec commits to one grammar
— if you adapt a kit written against v1, rename all three sections, not one.
Other schema gotchas worth knowing: `startup` entries are exec-style arrays
(`command: ["bash", "…"]`), and `content` fields only allow the `${WORKDIR}`
placeholder — use brace-free `$VAR` for shell variables.

## SSH (experimental)

Sandboxes can be SSH targets. Register the host block once — sbx writes a
managed `Host *.sbx` entry into your SSH config — then every sandbox answers at
`<name>.sbx`:

```bash
sbxw ssh --setup              # one-time (wraps `sbx setup ssh`)
sbxw ssh neos                 # interactive shell
sbxw ssh neos -- git status   # one-shot command
```

`install.sh` does this for you. An interactive run asks (defaulting to yes); a
piped one — `curl … | sh`, which is how most people install — has nobody to ask
and applies that same default, since otherwise the SSH button in the web UI and
`sbxw ssh` would fail for the majority of installs, with the fix buried in a line
of installer output nobody reads. What gets written is a managed, sbx-owned
`Host *.sbx` block, which matches no host you already have.

Set `SBXW_SETUP_SSH=0` to leave your SSH config untouched, or `SBXW_SETUP_SSH=1`
to configure it without being asked. `sbxw ssh --setup` is the catch-up path if
you declined or installed the binary by hand; it's idempotent, so re-running it
is harmless.

Two things this gives you that `sbxw bash` doesn't:

- **It starts things for you.** The connection brings up the sbx daemon *and* the
  target sandbox on demand, so `sbxw ssh` works against a stopped sandbox.
- **Remote development.** Any OpenSSH-compatible tool can attach — VS Code,
  Cursor, Claude Desktop, ChatGPT:

  ```bash
  code --remote ssh-remote+neos.sbx /workspace
  ```

  In **Claude Desktop**, *Add SSH connection* → put the alias in **SSH Host**
  (`neos.sbx`, which is what "or a host from `~/.ssh/config`" means) and leave
  **SSH Port** and **Identity File** empty. The managed block supplies the user,
  the key and a ProxyCommand — sandboxes are not reachable on TCP 22, so filling
  those fields in overrides the only thing that makes the connection work.

If the connection fails and no `*.sbx` entry is found in `~/.ssh/config`, sbxw
says so and points you at `--setup` rather than leaving you with a bare
`Connection refused`. SSH access is experimental and may need enabling in your
sbx installation first.

## Shared skills

sbx keeps a **persistent skill store shared across sandboxes**, separate from
this repo's kits. `sbxw skills import` fills it from the agents installed on
your host:

```bash
sbxw skills import --dry-run   # preview what would be imported
sbxw skills import             # do it
sbxw skills import --force     # ...replacing skills already in the store
```

Imported skills survive `sbxw rm`. Since sbx 0.43 new sandboxes mount the
store **read-only** by default; `skills` in `sbxw.toml` picks another mode,
passed as `sbx create --skills=…`:

```toml
skills = "off"        # only the skills this sandbox's kits provide
# skills = "readwrite"  # the agent may add to the store every sandbox mounts
```

Empty (the default) leaves it to sbx — and to its host-wide
`skills.defaultMode`, which is the better place for a preference that isn't
about one project. The old `share_skills = false` still works and means
`skills = "off"`; `skills` wins when both are set. It's read at **creation**
only — changing it does nothing to sandboxes that already exist. An exported
environment file carries it as its own `skills:` field.

The skills flag comes from release notes rather than the published `sbx create`
reference, and an unknown flag fails the whole command. So sbxw reads `sbx
create --help` once before passing it: `--skills=MODE` when listed, the
deprecated `--no-share-skills` for `off` when only that is, and otherwise the
sandbox with sbx's default and a warning that the setting couldn't be honoured,
rather than no sandbox at all.

How this relates to [kits](#kits): a kit can install *anything* (apt packages,
binaries, startup commands) but applying one recreates the container. The skill
store only carries skill files, and costs nothing to update. So `md-to-pdf-tools`
still has to be a kit — the skill needs WeasyPrint and poppler underneath it —
but a skill with no system dependencies belongs in the store instead.

## Config (`sbxw.toml`)

See `sbxw.toml.example`. Key choice: `ip_per_app`.

- `false` (default): every app binds `127.0.0.1` on a distinct host port;
  `/etc/hosts` maps the alias to `127.0.0.1`. Reach it at `alias:host_port`.
- `true`: each app gets its own `127.0.0.X` loopback IP (added on `lo0` on
  macOS), so the alias resolves to a dedicated IP and you use the app's natural
  port — `http://neos.local:4200` with no remapping.

### The default model

`claude_model` reaches the sandbox as **`ANTHROPIC_DEFAULT_MODEL`**, alongside
`[env]` — so it is baked in at creation, re-applied on every attach, and carried
into an exported `sbxenv.yaml` without a special case anywhere.

It used to be written as `model` into the sandbox's `~/.claude/settings.json`.
That looked equivalent and wasn't: in Claude Code's precedence, a settings-file
`model` sits **above** the choice `/model` saves, so every `sbxw up` quietly
undid whatever you had picked in-session. `ANTHROPIC_DEFAULT_MODEL` sits below
it — Claude Code starts on it *only* when nothing else selects a model — which
is what `claude_model` always claimed to be.

Two consequences worth knowing:

- **It needs Claude Code 2.1.236+ inside the sandbox.** That version comes from
  the sbx template, not from sbxw, so sbxw can't check it the way it checks
  `sbx` itself. On an older one the variable is ignored in silence and sessions
  start on Claude Code's own default.
- **Sandboxes created before this change carry the old `model` key**, which
  would outrank the variable forever. On the next `sbxw up`, sbxw removes that
  key — but only when it still holds exactly what `claude_model` says, i.e.
  sbxw's own leftover. A `model` that says something else is a deliberate
  `/model` and is left alone.

Spelling `ANTHROPIC_DEFAULT_MODEL` yourself in `[env]` overrides `claude_model`.

### Environment variables

```toml
env_files = [".env.sandbox"]   # root key — must come before [env]

[env]
NODE_ENV = "development"
API_URL  = "http://api.neos.local:8000"
```

Both reach the sandbox twice, and the difference matters:

- **At creation** (`sbx create -e` / `--env-file`) they are *baked into the
  sandbox*, so every process in it sees them — the agent, a Bash pane, a dev
  server you start by hand.
- **On every attach** (`sbx run -e`) they apply to the **agent session**. So
  editing `[env]` and reopening the agent pane is enough; nothing is recreated.
  A Bash pane is not the agent session, so it still shows the creation-time set
  until you `sbxw rm` and bring the sandbox back up.

sbx settles precedence, not sbxw: `[env]` beats every file, and a later file
beats an earlier one. Relative `env_files` paths resolve against `sbxw.toml`.

Two things to watch:

- `[env]` is a TOML **table**. Every root key has to come before it — a key
  written after `[env]` is read as an environment variable. Same trap as
  `[[ports]]`.
- **Not for secrets.** The value lands in `sbx create`'s argv, visible in the
  host's process list, and in a file most projects commit. Tokens go through
  `sbx secret set` — which is already how sbxw handles the Anthropic one — or an
  `sbxenv.yaml` `secrets:` entry that resolves on the host.

## Environment files (`sbxenv.yaml`)

sbx has its own committable project setup — `sbxenv.yaml`, read by
`sbx env run|create|exec|rm` — covering agent, workspace, extra mounts, kits,
env vars, secrets, MCP servers, skills, ports and registry credentials. sbxw
both **writes** one and **runs** on one.

The format as sbx 0.43 reads it, which is what sbxw follows:

- **A project's file is `sbxenv.yaml`, not hidden.** A directory means that name
  alone; a hidden `.sbxenv.yaml` there is no longer read, and `sbxw env run`
  says so and tells you to rename it. `~/.sbxenv.yaml` in your home directory is
  merged **beneath** every project file as shared defaults.
- **`${VAR}` is not expanded any more.** The file's own expressions are
  `${{ env.projectDir }}`, `${{ env.fileDir }}` and `${{ env.args.NAME }}` —
  the last declared in an `args:` block and supplied with `--env-arg NAME=VALUE`.
  sbxw warns about any `${VAR}` it finds.
- **Relative paths resolve against the file that declares them**, so
  `workspace: ./neos` means the same thing wherever you run from — and
  `~/.sbxenv.yaml` can mount each project's own directory with
  `workspace: ${{ env.projectDir }}`.
- **No `workspace:` means no mount**, not the file's directory. sbxw refuses
  such a file, because its pipeline needs a workspace to work on; write
  `workspace: .` to mount the directory holding the file.
- **`sbx env create` shows a plan and asks** before changing anything on the
  host, so `sbxw env run` needs you at the terminal for that step.

### `sbxw env run` — bring a sandbox up from the file

```bash
sbxw env run                        # sbxenv.yaml in the current directory
sbxw env run ../neos-env            # ...or in that directory
sbxw env run base.yaml local.yaml   # merged in order, later files win
sbxw env run --env-arg port=4300    # a value for ${{ env.args.port }}
```

It is not a passthrough. `sbx env create` does the **creation**, because it is
the only thing that can apply an environment file whole — secrets, bindings,
registries, MCP servers, skills, kits, mounts. Everything sbxw adds and the format has
no field for runs afterwards, through the same idempotent pipeline `sbxw up`
uses: egress policy, OAuth credentials, hooks, `/etc/hosts` aliases, the
browser terminal. That split is forced by sbx's own behaviour — `sbx env run`
re-applies only `env` and MCP to a sandbox that already exists, so anything
built on "run it again and it's fixed" has to live outside it.

**Ports are settled before sbx is called, never retried after.** That is the
whole port story, and it is not a preference:

- a port sbx can't publish doesn't cost you the port, it costs you the
  **sandbox** — creation fails and the new sandbox is removed;
- the secrets it provisioned *first* are left behind, so a naive retry loop
  accumulates credentials;
- there is no way to fix it from outside: `env create` has no port flag, and a
  second file can only *add* ports, because the merge **concatenates lists**.

So sbxw checks each host port itself, asks, and rewrites the document once:

```
$ sbxw env run
sbxw  /home/you/dev/neos-env/sbxenv.yaml → sandbox 'neos'
  ! host port 4200 (127.0.0.1) is busy — port for sandbox:4200 [4201] ⏎
sbxw  ports adjusted — running from a rewritten copy at ~/.sbxw/state/env/neos/sbxenv.yaml
sbxw  handing secrets, mcp to sbx — sbxw does not read those
sbxw  taken from sbxw.toml:
        · /etc/hosts aliases (neos.local → :4201)
        · the egress allowlist (15 rules) — an environment file has no field for it
        · claude_model (claude-sonnet-5) — passed as ANTHROPIC_DEFAULT_MODEL
```

With no terminal to ask on — a daemon, CI, a hook — it takes the next free port
and logs it instead of stopping for an answer nobody is there to give. `--yes`
forces that behaviour on a terminal too. Your committed file is never edited:
the rewrite goes to a scratch copy under `~/.sbxw/state/env/<name>/`, with paths
made absolute so it resolves from there, and every key sbxw doesn't model
(`secrets`, `bindings`, `registries`, `mcp`, `skills`, `sandboxOptions`, a kit's
`args`) travels through the round trip untouched. When nothing has to move, sbx
gets your paths exactly as you typed them.

The copy holds only your project's files. sbx merges `~/.sbxenv.yaml` beneath
it again, as it would beneath the original — so the copy must not contain it
too, or every kit and port it declares would be applied twice. The one thing
that can't be fixed this way is a busy port declared in `~/.sbxenv.yaml`
itself; sbxw stops and asks you to change it there.

The sandbox name is passed as `sbx env create --name`, so sbxw never has to
guess the `<agent>-<directory>` name sbx would otherwise pick.

**The environment file wins; `sbxw.toml` is the fallback.** Field by field,
because the two describe overlapping but unequal things — the format has ports
and env, it has no egress allowlist, no `/etc/hosts` alias and no model. A key
the environment file doesn't mention isn't unset, it is delegated downwards. The
run prints what it borrowed, which is what keeps "I edited the wrong file" from
being a silent afternoon.

One thing survives the merge on purpose: an **alias**. `neos.local` exists in no
sbx field, so when the environment file publishes 4200 and `sbxw.toml` calls
4200 `neos.local`, the alias is kept and re-pointed at whatever host port the
sandbox actually got — read back from `sbx ports`, not assumed from the file.

### `sbxw env export` — write the file

From the CLI, for a project:

```bash
sbxw env export                        # writes ../sbxenv.yaml for ./ as the workspace
sbxw env export -o ~/dev/sbxenv.yaml   # further up — workspace: ./acme/neos
sbxw env export -o -                   # print it instead
```

Or from the **web UI**, per pane: the **Env** button in the pane's top bar
opens a card (same shape as the SSH one next to it) with a preview, an editable
folder, Copy, and Save. That version is the better one when the two
differ, because it reads the ports the sandbox actually has — including one you
added from the ports dialog, or one that moved because the configured host port
was busy — rather than the ports the config asks for.

**Every path is written relative to the file**, so one committed file works on
machines that keep their repositories in different places:

```yaml
workspace: ./neos
skills: 'off'
kits:
  - ./kits/headroom
```

sbx 0.43 resolves those against the file itself, so the file moves with the
directory it sits in and needs no configuration at all. That replaces the
`${SBXW_PROJECTS_ROOT:-/Users/you/dev}/neos` sbxw used to write — sbx 0.42
stopped expanding `${VAR}`, so that line would now reach sbx verbatim. The
variable, and the `install.sh` prompt that set it, are gone; an existing
`export SBXW_PROJECTS_ROOT=…` in your shell rc is harmless and can be deleted.
A kit that shares nothing with the file's folder but the filesystem root is
written as an absolute path, since climbing all the way up is no more portable.

In the web UI, the folder is editable: any directory above the workspace will
do, and the `workspace:` line in the preview changes with it before you save.

Everything that *didn't* fit is written into the file's header, with the command
that reproduces it — the allowlist as an `sbx policy allow network` line, the
OAuth injection, the aliases, the model. An export that silently dropped the
allowlist would describe a sandbox with *broader* egress than sbxw builds, which
is the worst direction for an omission to go in.

The file lands in the workspace's **parent**, not the workspace, and that is not
tidiness: the agent can write anywhere in a direct-mounted directory, so an
environment file inside one is a file the agent can rewrite — and it is the file
that decides what the *next* sandbox gets. sbx 0.42 binds the file read-only
into the sandboxes `sbx env` creates, but a sandbox from `sbxw up` (plain `sbx
create`) mounts the same directory without that protection, so outside is still
the only place that is safe from both.

Nothing keeps the two files in step. Re-export after editing `sbxw.toml`.

## Security notes

- Workspace mount is scoped to the single project directory; use `--ro` for
  anything the agent should not modify.
- The network policy is an explicit allowlist, never `**`. Tighten/loosen in
  `sbxw.toml`. You can audit live egress with `sbx policy log`.
- **`sbxw.toml` usually sits inside the workspace**, which means the agent can
  edit it — including `network_allow`, which is read again on your next
  `sbxw up`. That is a widening the agent can propose but not perform: it takes
  effect only when you run `sbxw up` yourself, and `sbx policy ls <name>` shows
  what a sandbox actually has. If you'd rather it were out of reach, keep the
  config outside the mounted directory and point at it with `--config`. (This is
  the same hazard sbx names for `sbxenv.yaml`, which is why `sbxw env export`
  writes *beside* the workspace rather than into it.)
- When a `sbx` call fails, sbxw now folds **what sbx actually said** into the
  error instead of reporting a bare exit status — so the structured
  `Blocked by network policy` explanation (rule / origin / detail), and the
  support message an organisation attaches to a governance denial, reach you
  in the CLI and the web UI rather than dying in the daemon log.
- **Behind a corporate proxy**, a blocked download is often not sbxw's
  allowlist. `DOCKER_SANDBOXES_PROXY=system` routes sandbox egress through the
  host OS proxy configuration (macOS/Windows), PAC URL included — try that
  before widening `network_allow`. `sbx policy log` shows the real reason; an
  `origin: corporate policy` line means `sbx policy allow` won't help.
- Secrets travel via **stdin**, not argv, so they don't appear in `ps`.
- Your **OAuth token is on disk** in `~/.sbxw/state/kits/<name>-oauth/spec.yaml`
  (`0600`, in a `0700` directory) for as long as the sandbox exists — sbx
  re-resolves that path on every container swap, so it can't be deleted right
  after use. `sbxw rm <name>` removes it. Prefer `--use-api-key` if you'd rather
  the credential lived only in sbx's own keychain; the token is the same secret
  the sandbox already holds at `~/.claude/.credentials.json` either way.
- `/etc/hosts` changes are confined to a marked block and removed by `sbxw down`.
  Off a terminal, `sudo` runs with `-n`: the daemon never steals your shell's
  terminal to prompt on, it fails fast and says what to run instead. sbxw holds
  **no standing privilege** — no sudoers rule, no root helper installed; every
  privileged write is one you authorised, in a terminal or in the macOS panel.
  The content of that write is staged in a `0600` file under `~/.sbxw/state`
  (opened `O_EXCL`, so a planted path fails rather than being followed) and
  copied into place by `/bin/cp`, which keeps `/etc/hosts` root-owned `644`.
- The **relay** never moves information between two sandboxes without a person
  clicking for it, and an answer is editable before it is released — see
  [Asking another sandbox](#asking-another-sandbox-the-relay). A question is
  another agent's text arriving in your agent's prompt, so it is delivered
  quoted and labelled as untrusted input; read it before you route it.
- A **screenshot** is a picture of your screen going into an agent's context, so
  nothing goes anywhere until you attach it yourself, to one named sandbox,
  having seen the preview of every image you are sending — a set is sent whole,
  so what is on screen when you click is exactly what leaves. sbxw cannot
  capture your screen — the browser's own picker is the permission, and it is
  asked for per capture. Refusing is a first-class answer, and it discards what
  was attached.

## Unconfirmed against docs (verify locally)

- Exact column layout of `sbx ls` (used to detect existence / running state).
- Whether `sbx create` accepts the same positional `:ro` extra-workspace syntax
  as `sbx run` (documented for `run`; assumed identical for `create`).
- `sbx policy set-default` posture names (not used here; we use explicit
  `allow network`).
- Exact output format of `sbx inspect`. sbxw only does a substring
  match on it to *skip* already-applied kits; if the kit name isn't found
  (a format change), the kit is simply re-applied as before. The match
  is confined to inspect's kits section (a `Kits:` block, or a `kits` key if the
  output is JSON) because inspect also reports the sandbox's **custom secrets**,
  and a secret sharing a kit's name would otherwise skip a kit that had never
  been applied. A layout with neither — a KITS *column*, say —
  falls back to searching the whole output, as before.
- ~~The flags taken from the newer release notes but not yet checked against a
  live `sbx --help`~~ — checked against the published 0.39 CLI reference:
  `sbx create -p/--publish` ✅, `sbx skills import` ✅, `sbx setup ssh` ✅ (it
  also has a `remove` subcommand), `sbx policy ls` / `policy log` ✅.
  `sbx create --no-share-skills` is the one that did **not** turn up: the 0.39
  reference lists `--clone --cpus --deny-network -e/--env --env-file --kit
  -m/--memory --name -p/--publish -q/--quiet -t/--template` and no skills flag.
  0.43 replaced it with `--skills=MODE`, again from release notes only. Since
  an unknown flag is a hard error, sbxw probes `sbx create --help` before
  passing either — see [Shared skills](#shared-skills).
- The shape of an environment file's `args:` block. The 0.42 notes document
  `${{ env.args.NAME }}` and `--env-arg`, not the block's grammar. sbxw reads
  `NAME: value`, `NAME: {default: value}` and a list of `{name, default}` for
  its own view of the file; an argument it can't read stays in the text for sbx
  to resolve. Likewise a `kits:` entry written as a mapping (with per-kit
  `args`) is passed through untouched rather than resolved by sbxw.
- Whether `sbx ports` lists a 0.42 default publish as `tcp4` or `tcp`. sbxw
  treats both as the default on an IPv4 address, where they mean the same
  thing. The original caveat
  below still applies to everything else: each has a fallback if the flag turns
  out to be spelled differently — `create` failing means `sbxw up` fails loudly
  rather than silently mis-provisioning, and the port publishing is still done
  by the provisioning thread regardless.

(The kit schema, once flagged as unconfirmed, is now verified — see [Kits](#kits).)
