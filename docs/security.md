# Security

[← Back to README](../README.md)

sbxw runs agents with `--dangerously-skip-permissions`, so the sandbox is the
safety boundary. This page lists what that boundary is and where it has edges.

- [What a sandbox can reach](#what-a-sandbox-can-reach)
- [Two listeners](#two-listeners)
- [Human decisions](#human-decisions)
- [Secrets](#secrets)
- [Host changes](#host-changes)
- [Known edges](#known-edges)

## What a sandbox can reach

- **Files:** only the workspace folder (and any extra mounts you add). Use
  `--ro` or a **RO** repo for anything the agent should not modify. The sandbox
  is a microVM with its own filesystem, network and Docker daemon.
- **Network:** an explicit allowlist (`network_allow`), never `**`. Entries of
  `network_allow` and `network_deny` are checked when the config is loaded:
  `*`, `**`, an entry starting with `-`, and one containing whitespace or a
  comma are refused, and so is a `network_allow` entry naming the UI
  listener's port on `localhost`, `127.0.0.1`, `host.docker.internal` or
  `[::1]`. Audit live egress with `sbx policy log` or the [network panel](web-ui.md#ports-and-network-policy).
- **Behind a corporate proxy**, a blocked download is often not sbxw's
  allowlist. `DOCKER_SANDBOXES_PROXY=system` routes sandbox egress through the
  host OS proxy settings (macOS/Windows, PAC included); try it before widening
  `network_allow`. In `sbx policy log`, `origin: corporate policy` means
  `sbx policy allow` won't help.
- **Published ports bind `127.0.0.1`** on the host, not every interface, so
  a sandbox's apps are not reachable from your network.
- **sbx's own errors reach you.** When an `sbx` call fails, sbxw shows what sbx
  said (including `Blocked by network policy` details and an organisation's
  support message) in the CLI and web UI.

## Two listeners

The daemon listens twice, for two audiences:

| Listener | Default | Who | What it allows |
|---|---|---|---|
| **UI** (`web_addr`) | `127.0.0.1:7681` | Browser, island, extension | Everything, including every decision that moves information between sandboxes or starts work: routing a question, releasing an answer, approving or merging a plan, pairing a host, opening a terminal |
| **Agent** (`agent_addr`) | `127.0.0.1:7682` | Sandboxes | Hooks, usage, relay requests, plan proposals and reports. Nothing that approves anything |

Both addresses must be `IP:PORT` literals; a host name is refused.

- sbxw's own network rule for a sandbox allows **only the agent port**. It
  does not remove a UI-port rule an older sbxw may have added, and a
  `network_allow` entry that passes the load-time checks is applied as
  written: check
  `sbx policy ls <name>` if in doubt.
- **Every agent call carries the sandbox's token**: 256 bits, minted at
  bring-up, installed over stdin into `~/.sbxw/token` (mode `0600`), never on a
  command line. The daemon derives *which* sandbox is calling from it, so a
  sandbox can only act as itself.
- **The UI listener refuses other web pages.** A request must carry a `Host`
  naming the daemon (`sbxw.localhost`, `localhost`, `127.0.0.1`, `[::1]` on its
  port), which stops DNS rebinding. An `Origin`, when present, must be the
  daemon's own page or the extension, which stops a site you visit from opening
  a terminal (`/ws`: a shell in a sandbox, or `monitor_cmd` **on the host**) or
  POSTing to the API. Native clients send no `Origin`.

## Human decisions

- **Relay:** nothing moves between two sandboxes without a click, and answers
  are editable before release. A question is another agent's text arriving in
  your agent's prompt, so it's delivered quoted and labelled untrusted. Read it
  before routing. See [Relay](relay.md).
- **Text typed into a terminal:** when one agent's text is typed into another
  agent's terminal, control characters (ESC, CR, the other C0 and C1
  characters) and bidi and zero-width format characters are removed first. The
  text cannot end the paste and type keystrokes of its own.
- **Screenshots:** in the web UI, nothing leaves until you attach it, to one
  named sandbox, after seeing a preview of every image. sbxw can't capture
  your screen; the browser's own picker asks each time. Refusing discards what
  was attached. In the browser side panel, a screenshot request from the
  panel's sandbox waits for you to click **Send a screenshot of this tab**,
  which captures the tab beside the panel and sends it at once, without a
  separate preview; the transcript then shows what was sent. Only if you turn
  on automatic screenshots in that browser's panel does it answer without a
  click, at most once per sandbox every 10 seconds; further requests wait for
  the click. See [the side panel's screenshots](web-ui.md#screenshots-from-the-browser-side-panel).
- **Virtual Mac** (`sbxw:terminal-mac`, opt-in): your approval is needed to
  create or boot the VM, to resume a paused one, to lend a running VM that
  sbxw did not start itself, and to publish a port to it. Clicks, typing and
  screenshots need no approval, but only on a VM sbxw brought up or was lent.
  Ports published to the guest are removed when the sandbox's lease ends
  (release, expiry, takeover, or the sandbox stopping). When another sandbox
  asks for the Mac, the one holding it keeps it for 60 minutes at most. The
  screenshots are of a **shared, disposable guest**, never of your screen,
  and go to the sandbox holding the lease without a preview. Keep every
  personal and organisation account out of the guest. The guest has its own
  internet access through the VM's NAT, **outside the sandbox's network
  allowlist**: a sandbox holding the Mac can open any site in the guest's
  browser and read the pages back through screenshots. Enable the kit only
  where that is acceptable. terminal-mac is a pinned, SHA-256-checked
  third-party release run on the host; see
  [its trust notes](../assets/terminal-mac/README.md#trust).
- **Plans:** nothing starts until you approve. After approval, a lead and its
  threads message each other without clicks, **inside one sandbox only**. See
  [the lead's trust model](threads-and-lead.md#trust).

## Secrets

- Secrets travel over **stdin**, never argv, so they don't show in `ps`.
- sbxw writes **no OAuth token**: sbx keeps the claude.ai sign-in on the host
  itself. A Remote Control sandbox gets no auth variables at all.
- `--use-api-key` stores the key as sbx's global `anthropic` secret, over
  stdin. It is not sent to a remote host whose `trust_secrets` is false.
- `[env]` is **not** for secrets: it lands in argv and usually in a committed
  file.

## Host changes

- `/etc/hosts` changes stay in a marked block, removed by `sbxw down`.
- sbxw holds **no standing privilege**: no sudoers rule, no root helper. Every
  privileged write is one you authorised, in a terminal or the macOS admin
  prompt. Off a terminal, `sudo` runs with `-n` and fails fast with the command
  to run instead.
- The new `/etc/hosts` content is staged in a `0600` file in the system temp
  directory (`sbxw-hosts-staged.<pid>`). Any file already at that path is
  removed first and the new one is created `O_EXCL`, so a symlink planted there
  is not followed. It is copied in with `/bin/cp`, which keeps `/etc/hosts`
  root-owned `644`, and removed afterwards.

## Known edges

### The agent can edit `sbxw.toml`

`sbxw.toml` usually sits in the workspace, so the agent can edit it, including
`network_allow`, `env`, `env_files`, `kits` and `monitor_cmd`. That's a change
the agent can **propose but not perform**:

- **A sandbox's `sbxw.toml`** is read when a person brings the sandbox up
  (`sbxw up`, or Up in the web UI). The daemon records the parsed config in the
  sandbox's record under `~/.sbxw/state`, and every later terminal attach,
  thread, port release and panel uses that record, never the file. So every
  key, including `env`, `env_files`, `workdir`, `web_shell`, `network_allow`
  and `kits`, takes effect only at the next `sbxw up`. A sandbox recorded by
  an sbxw older than this has no recorded config and runs with the daemon's.
- **The daemon's own config** is read when the daemon starts. Its `kits`
  change only through the web UI's Kits dialog, so a kit added to that file by
  hand (or by an agent) reaches new sandboxes only after a daemon restart.
- `monitor_cmd` and the terminal colours are placed in the web UI as escaped
  string literals, so a value cannot run script in the UI.
- The Kits dialog refuses to write an `sbxw.toml` that is a symlink, and its
  temporary file is created under a random name that is never opened if it
  already exists.

The agent can still propose any single named host in `network_allow`; the
load-time checks refuse only wildcards, malformed entries and the UI port.
Read a changed `network_allow` before you run `sbxw up`. `sbx policy ls <name>`
shows what a sandbox really has. To keep the config out
of reach, store it outside the workspace and use `--config`. (`sbxw env export`
writes beside the workspace for the same reason.)

### The kit cache filler

The first sandbox after the cache expires can write the
[kit cache](kits-and-skills.md#the-kit-cache) for its whole life, so its agent
could leave something there for later sandboxes. The 7-day TTL and
`sbxw cache clear` bound this.

### Thread reports

A thread's status report carries no secret, so another session in the same
sandbox could forge one. It changes a label, never what runs.
