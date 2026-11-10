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
| **UI** (`web_addr`) | `127.0.0.1:7681` | Browser, island, extension, `sbxw` CLI | Everything, including every decision that moves information between sandboxes or starts work: routing a question, releasing an answer, approving or merging a plan, pairing a host, opening a terminal |
| **Agent** (`agent_addr`) | `127.0.0.1:7682` | Sandboxes | Hooks, usage, relay requests, plan proposals and reports. Nothing that approves anything |

Both addresses must be `IP:PORT` literals; a host name is refused.

### The agent listener

- sbxw's own network rule for a sandbox allows **only the agent port**. It
  does not remove a UI-port rule an older sbxw may have added, and a
  `network_allow` entry that passes the load-time checks is applied as
  written: check
  `sbx policy ls <name>` if in doubt.
- **Every agent call carries the sandbox's token**: 256 bits, minted at
  bring-up, installed over stdin into `~/.sbxw/token` (mode `0600`), never on a
  command line. The daemon derives *which* sandbox is calling from it, so a
  sandbox can only act as itself.

### The UI listener

Every request must pass two checks, in this order.

1. **Host and Origin.** A request must carry a `Host` naming the daemon
   (`sbxw.localhost`, `localhost`, `127.0.0.1`, `[::1]` on its port), which
   stops DNS rebinding. An `Origin`, when present, must be the daemon's own
   page or the extension, which stops a site you visit from opening a
   terminal (`/ws`: a shell in a sandbox, or `monitor_cmd` **on the host**) or
   POSTing to the API.
2. **The UI token.** At every start the daemon mints a new 256-bit token and
   writes it to `~/.sbxw/run/ui-token` (file `0600`, directory `0700`); it
   removes the file when it stops. Every request must carry it, compared in
   constant time:
   - native clients (the `sbxw` CLI, the island) read the file and send
     `Authorization: Bearer <token>`;
   - a browser signs in once per daemon start through the address `sbxw web`
     prints, `/?t=<token>`. The daemon answers with an `HttpOnly`,
     `SameSite=Strict` cookie and a redirect to the same address without the
     token (`Cache-Control: no-store`), so the token stays out of the address
     bar and the history. The page, its scripts, `/api/events` and the
     terminal WebSocket all ride on that cookie.
   - **The cookie is set for `sbxw.localhost` only**, when the daemon is on
     loopback (`127.*`, the default). A browser keeps a cookie per host name,
     not per port, and sandbox apps are published on `127.0.0.1` and
     `localhost`: a cookie there would go to every dev server on those names,
     and an agent could read the token from its own server's request headers.
     So a sign-in at `127.0.0.1` or `localhost` sets nothing and redirects to
     the same sign-in at `sbxw.localhost`, and a cookie arriving at any other
     name is ignored. **Residual:** a server on another port of
     `sbxw.localhost` would still receive the cookie, so do not open sandbox
     apps under that name.
   - With `web_addr` deliberately off loopback (a LAN address, or `0.0.0.0`
     for every interface), there is no name of sbxw's own to pin the cookie
     to: it is set for whatever name the browser used. The `sbxw` CLI and the
     extension still use `sbxw.localhost` for a `0.0.0.0` bind, but a browser
     signed in at the machine's IP shares that cookie with any app published
     on the same IP.

   Without a valid token the daemon answers `401`, with three exceptions that
   reveal nothing: `/` shows a page saying to run `sbxw web`; `/sidepanel`
   shows a page saying how to sign the browser in for the extension; and
   `/api/health` answers only the proof below (no version, no addresses).

**Port squatting.** Before the CLI sends the token, or anything secret such as
the API key of `sbxw up --use-api-key`, it checks that what answers on the
port knows the token, without revealing it: it sends a random nonce to
`/api/health?nonce=`, and the daemon answers `SHA-256(token || nonce)`. On a
mismatch the CLI sends nothing more and names the port: another process is
listening there (an sbxw daemon older than the token answers the same way;
`sbxw down` stops it). It also refuses to start a second daemon on a port that
answers without the proof.

**What the token protects against:**

- other users of this machine: they cannot read `~/.sbxw/run/`;
- other processes and web pages that can reach the port but do not have the
  file;
- a sandbox whose network policy happens to reach the UI port;
- a process that took the port before the daemon (the proof above).

**What it does not protect against:** a process running as **your** user. It
can read the token file (and your browser's cookies) and act as you. The
token also appears in the terminal where `sbxw web` printed the sign-in
address.

**The extension's side panel** frames `http://sbxw.localhost:<port>/sidepanel`
(an address saved on `127.0.0.1` or `localhost` is moved to that name), so it
uses the same sign-in as the web UI; `sbxw extension` also prints the address.
Whether Chrome sends a `SameSite=Strict` cookie to a frame inside an extension
page depends on its rules for extensions with host permissions and on
third-party cookie settings; when it does not, the panel shows the sign-in
page instead of the chat.

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

`sbxw.toml` usually sits in the workspace, so the agent can edit it, and so
can a repository you have just cloned. The part of it that reaches the host
or widens the sandbox takes effect only after you have seen it and said yes,
as with `direnv allow`:

- **What is asked.** Each such setting becomes one line: a mount
  (`workspaces`, `kit_mounts`), an env file, a kit beyond the default ones
  (local path, registry or git), a `network_allow` host beyond the default
  list, a published port, a `/etc/hosts` alias, `ip_per_app`, a
  `monitor_cmd` (it runs **on the host**), a moved `web_addr` or
  `agent_addr`, `skills = "readwrite"`, a named `template`, a longer
  `cache_ttl_days`, and the virtual Mac's `vm` and `guest_host_ip`. A value
  equal to the built-in default is not a line, so a file that only changes
  the model, the colours, `env` or the sizes asks nothing. (`env` stays
  inside the sandbox; `[env]` is still not for secrets.)
- **When.** At a bring-up from that file: `sbxw up`, `sbxw chat`, and Create
  or Chat in the web UI (which use the daemon's own config). Only lines not
  approved before are asked about; removing a setting asks nothing. The CLI
  also checks `web_addr`, `agent_addr`, `monitor_cmd` and `[mac]` before it
  starts the daemon with the file (`sbxw up`, `sbxw web`). The first
  `sbxw up` after upgrading asks once for whatever the file grants beyond
  the defaults.
- **How.** In a terminal, the lines are printed and sbxw asks
  `Allow? [y/N]`. Off a terminal the command fails with the lines;
  `--allow-host-access` approves what is shown without asking, for scripts.
  In the web UI a dialog lists the lines as text, with Allow and Cancel.
  The approval names the exact lines shown, so a file edited between
  showing and approving is asked about again.
- **Enforced by the daemon**, so the CLI and the web UI cannot skip it. A
  remote host's `sbxw.toml` is checked the same way, filed under the host's
  name and path.
- **Where approvals live.** `~/.sbxw/state/approved-grants.json` (mode
  `0600`), keyed by the config file's absolute path, never in the
  workspace. A line no longer in the file is dropped, so a setting removed
  and later put back is asked about again.
- **Revoking.** `sbxw config grants` shows what is approved for a file and
  what it asks for beyond that; `sbxw config grants --revoke` forgets the
  approvals, so the next bring-up asks again.

What is not asked about:

- **Duplicate and Recreate** reuse the sandbox's recorded config, which was
  approved when the sandbox was brought up. A sandbox recorded by an older
  sbxw has no recorded config and runs with the daemon's.
- **Folders and ports you pick in the create dialog** are your own choice.
- **Everything else the file sets** still takes effect at the next bring-up,
  as before: the daemon records the parsed config in the sandbox's record
  under `~/.sbxw/state`, and every later terminal attach, thread, port
  release and panel uses that record, never the file.

Other safeguards:

- **The daemon's own config** is read when the daemon starts. Its `kits`
  change only through the web UI's Kits dialog, so a kit added to that file
  by hand (or by an agent) reaches new sandboxes only after a daemon
  restart, and is then asked about.
- `monitor_cmd` and the terminal colours are placed in the web UI as escaped
  string literals, so a value cannot run script in the UI.
- The Kits dialog refuses to write an `sbxw.toml` that is a symlink, and its
  temporary file is created under a random name that is never opened if it
  already exists.
- `network_allow` entries are checked at load: wildcards, malformed entries
  and the UI port are refused. `sbx policy ls <name>` shows what a sandbox
  really has.

To keep the config out of the agent's reach entirely, store it outside the
workspace and use `--config`. (`sbxw env export` writes beside the
workspace for the same reason.)

### The kit cache filler

The first sandbox after the cache expires can write the
[kit cache](kits-and-skills.md#the-kit-cache) for its whole life, so its agent
could leave something there for later sandboxes. The 7-day TTL and
`sbxw cache clear` bound this.

### Thread reports

A thread's status report carries no secret, so another session in the same
sandbox could forge one. It changes a label, never what runs.
