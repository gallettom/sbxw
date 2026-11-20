# Security

[← Back to README](../README.md)

sbxw runs agents with `--dangerously-skip-permissions`, so the sandbox is the
safety boundary. This page lists what that boundary is and where it has edges.

- [What a sandbox can reach](#what-a-sandbox-can-reach)
- [Two listeners](#two-listeners)
- [Human decisions](#human-decisions)
- [Jarvis](#jarvis)
- [Secrets](#secrets)
- [Host changes](#host-changes)
- [Known edges](#known-edges)

## What a sandbox can reach

- **Files:** only the workspace folder (and any extra mounts you add). Use
  `--ro` or a **RO** repo for anything the agent should not modify. The sandbox
  is a microVM with its own filesystem, network and Docker daemon.
- **Files inside a mounted folder can be held back** with a
  [carve](configuration.md#carving-a-folder-out). A **strict** carve never
  puts the folder in the VM: the parent is mounted as separate folders around
  it, so even an agent with root cannot reach it, at the cost of the files
  beside the mounted folders being invisible. A **soft** carve is a mount over
  the folder; the agent has root and can unmount it, after which the folder is
  readable through its parent. Soft guards against mistakes, strict against a
  hostile agent. Removing or weakening a carve in `sbxw.toml` needs your
  approval like a new grant.
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
| **Remote** (`remote_addr`) | `127.0.0.1:7683`, only while remote access is on | A paired phone, through the `sbxw-tailnet` sidecar | Read a running team leader's conversation (no tool input or output), type or dictate to it, answer its multiple-choice question, interrupt it. Nothing that approves anything, nothing irreversible |

All addresses must be `IP:PORT` literals; a host name is refused. `remote_addr` must be
loopback.

### The remote listener

Off by default. Its own router, its own token, pairing confirmed on the Mac: see
[Remote access](#remote-access) below for the threat model in full.

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
  before routing. Only a one-line notice with the request id is typed into the
  routed-to agent's terminal; that agent fetches the question itself with
  `relay.js show`, which answers only the sandbox it was routed to, while it is
  waiting on that sandbox's answer. See [Relay](relay.md).
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

## Jarvis

[Jarvis](jarvis.md) is a Claude session in a sandbox the daemon owns,
`sbxw-jarvis`, that can propose changes to sbxw's configuration. Its trust
model has three steps: **the sandbox proposes, the daemon validates, the
person approves.**

- **The sandbox proposes.** Jarvis sends data, not files or commands:
  "add this bundled kit to that `kits` line", "set this key to that value".
  It never names a path on the host and never runs anything there.
- **The daemon validates and writes.** It turns each change into the exact
  write and a diff, or refuses it with a reason:
  - kits are written by sbxw, under `~/.sbxw/state/kits/jarvis-*` only. sbxw
    renders the `spec.yaml` itself: image content and a `chmod` for scripts,
    no install command, no network rule. Kit files land under `/home/agent/`
    in a sandbox, never in sbxw's own files or Claude Code's settings, and
    never as code that runs unasked: shell start files, `.ssh/`,
    `.claude/hooks/`, `.claude/CLAUDE.md` and `.local/bin/claude` are
    refused;
  - settings come from a fixed list and are checked as the config loader
    checks them; a pronunciation change writes one entry of `[voice.words]`
    only, a word of letters, digits, `-`, `_` or `.` and one line of text
    per language; a `network_allow` entry is a bare host name (no address,
    no port, no wildcard, not this machine), and approving it records the
    same consent as any new grant;
  - **no secrets**: a change carrying something shaped like a key or a token
    is refused, and so is an MCP server with a credential in a header, its
    URL or an argument. Jarvis writes `${key}` placeholders; the person fills
    in the secrets in Setup;
  - a desktop screen is written only as
    `~/.sbxw/desktop/screens/<name>.luau`, a path the daemon builds from the
    name alone; the name must be one of the screens the desktop app wrote in
    `~/.sbxw/desktop/builtin/screens/` (no new screen), the text is UTF-8,
    at most 64 KB, with no NUL. The daemon only reads `builtin/`. That
    folder sbxw creates when it is missing, because it is sbxw's own, under
    its home;
  - a change to the daemon's `sbxw.toml` when that file does not exist
    creates it, only at the path the daemon was started with and only in a
    folder that already exists. The title names the path and the diff shows
    a new file; the risk is that of the change, as for an existing file;
  - at approval, the daemon plans again against the files as they are. If a
    diff differs from the one shown, nothing is applied. A file that appeared
    since the proposal counts as changed. Changes are applied
    in order and stop at the first failure.
- **The person approves.** Nothing is applied without Approve, in the web UI,
  on the UI listener. There is no auto-approve.

**Only `sbxw-jarvis` may call the proposal endpoints.** On the agent listener,
the daemon answers them for one caller: the local sandbox named `sbxw-jarvis`
that it created on Jarvis's own workspace. Every other sandbox is refused,
whatever its token. The name is reserved on every host, so a project sandbox
cannot take it, and a sandbox of that name sbxw did not create is not Jarvis.
The `sbxw:jarvis` kit, which holds the proposal tools, is never offered and a
project's `kits` cannot list it.

**Jarvis's sandbox is kept small.** Its workspace is an empty folder under
sbxw's state: it sees no project and none of sbxw's files. It is built from
sbxw's defaults, not the daemon's `sbxw.toml` (only the listeners, the
network policy and the model are taken), so no mount, env, port or shared
skill store reaches it.

**Low risk and high risk.** Every change has a risk, and a proposal takes
the highest of its changes:

| Low risk | High risk |
|---|---|
| A setting from the fixed list; a word's pronunciation (`[voice.words]`); a `kits` line change that only adds bundled kits (`sbxw:<name>`) or only removes | Writing or removing a Jarvis kit; putting a Jarvis kit, or `sbxw:ssh-agent`, `sbxw:git-identity`, `sbxw:dev-ports` or `sbxw:terminal-mac`, in a `kits` line; changing `network_allow`; adding, replacing or removing an MCP server; changing or removing a desktop app screen |

A low-risk proposal may be approved by voice (a spoken *oui* after Jarvis
reads its summary). A pronunciation is low risk because its value is only
text the voice reads aloud on this machine: it reaches no sandbox, no agent
and no dictation, it runs nothing, and it is bounded (one word, one line of
at most 120 characters per language, 200 entries). The worst it does is make
a word sound wrong, which the person hears and can undo the same way. A high-risk one is approved only with a click in the
dialog, after its diff is on screen: the daemon refuses a voice approval of
it (HTTP 409). The diff is the sandbox's text: the page shows it as text,
never as HTML.

**A stdio MCP server runs on the host.** It is a command sbx runs on this
machine, outside any sandbox, with the person's rights: the one Jarvis change
that runs code on the host. The daemon marks such a proposal `needs_ack` and
refuses its approval (HTTP 409) unless the request carries
`ack_host_command: true`. The page sends that only from an **I have read this
command** box, beside a red banner that shows the full command as text, and
keeps Approve off until it is ticked. Such a proposal is high risk, so never
approved by voice.

**A desktop screen can mislead, not do more.** A screen of the desktop app
is a sandboxed Luau file: no file, network or process access, and it can
only emit intents from the closed list the app's core defines, which checks
each one and draws any approval itself. So a screen Jarvis writes cannot do
anything the app could not do before. What it can do is lie: a button
labelled "Open" that emits `stop_sandbox`. That is why every screen change
is high risk, approved only with a click on its diff, never by voice.

To help the person see it, the diff starts with the intents written by name
in the new screen, with the risk the app declares for each, and marks those
the screen used now does not emit. The daemon reads the screen's Luau for
its string literals (comments are skipped, escapes decoded) and keeps those
that are intent names from the app's `intents.json`.

A screen can emit an intent two ways: `intent("name", args)`, or by
returning any table `{ kind = "intent", name = ..., args = ... }`. The scan
flags, with the line, the forms it recognises as hiding a name:
`intent(...)` called with anything but one plain string, `intent` passed
around as a value, a table with `kind = "intent"` whose `name` is not one
plain string, a table whose `kind` is computed, a table key that is
computed (`[k] = v` in a constructor), a `kind` or `name` set field by field
(`t.kind = ...`, `t["name"] = ...`), and `rawset` or `setmetatable`.

This list is a **hint, not a proof**, and the dialog says so on every
screen change: "a screen can hide an intent name; read the diff". A text
scan cannot follow a name or the word `intent` assembled from pieces
(`"int" .. "ent"`) and stored through a computed key outside a constructor
(`t[k] = v`), which ordinary screens do for their own data. A string that is
never emitted is still listed. At approval the daemon refuses the change if
the person's version or the built-in screen moved since the proposal.

What Jarvis says is an agent's text, like any other sandbox's: its summary
may be wrong or written to persuade. The diff is what the daemon would
write; read it.

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

## Remote access

Remote access lets a phone you pair talk to your **running team leaders** while you are
away from the Mac: read their conversations, type or dictate to them, hand them a file,
answer their multiple-choice questions, interrupt them. It goes through your own private Tailscale
network (a "tailnet"); nothing passes through a server of sbxw's or anyone else's, and no
conversation leaves the Mac except to your phone.

It is **off by default**. Step 1 lets a phone do nothing irreversible. Step 2, also off
until you turn it on at the Mac, lets it approve a short list of things with Face ID at each
action: see "Approving from the phone" below.

The wire contract is `.sbxw-artifacts/remote/API.md`; this section says what it is for and
what it protects.

### Turning it on, pairing, turning it off

```
sbxw remote on          # asks first, says what it exposes
sbxw remote status      # where the set-up stands, what to do next, devices, team leaders
sbxw remote pair        # prints a one-time link to open on the phone, then asks for its digits
sbxw remote devices     # paired devices
sbxw remote revoke <id> # unpair one, at once
sbxw remote journal     # what devices did, never what they wrote
sbxw remote off         # stop everything; paired devices are kept
sbxw remote forget-node # (when off) log the tailnet machine out and delete its state
```

Set-up, the first time. **(Mac)** is a step at the Mac's terminal, **(phone)** one on the
phone. `sbxw remote status` says where you are in one line (`state:`), then what to do next.

1. **(Mac)** Install the Tailscale node: `scripts/build-tailnet.sh` in the sbxw source folder
   builds `sbxw-tailnet`. Its last lines say where sbxw looks for it (next to the `sbxw`
   binary) and give the `cp` that puts it there, or say "already in place". If it is not
   there, `sbxw remote on` and `sbxw remote status` (`state: sidecar missing`) name the
   exact path and the one command that builds it in place.
2. **(Mac)** `sbxw remote on`. It asks first, then opens the remote listener and starts
   `sbxw-tailnet`, which joins your tailnet as a machine named `sbxw`
   (`state: starting`).
3. **(Mac)** `sbxw remote status` a few seconds later. The first time it shows
   `state: waiting for login` and a `login link:`. Open it in any browser (on the Mac is
   fine) and add the machine to your tailnet. If device approval is on in your tailnet it
   shows `state: waiting for approval`: approve `sbxw` on the Machines page of the admin
   console (login.tailscale.com). If it shows `state: https unavailable`: in the admin
   console, on the DNS page, turn on MagicDNS and HTTPS Certificates, then
   `sbxw remote off` and `sbxw remote on`.
4. **(Mac)** When `sbxw remote status` says `state: running at sbxw.<your tailnet>.ts.net`,
   check its `team leaders:` line: the phone only shows running team leaders. "none" means
   no sandbox leads a team now: turn the team on in one (the Team button of its pane, or
   `/lead`).
5. **(Mac)** `sbxw remote pair` prints a link `https://sbxw.<your tailnet>.ts.net/#pair=<code>`,
   good once, for 5 minutes. **It does not open on the Mac**: only sbxw's own node is on
   the tailnet, not the Mac (unless the Mac also runs the Tailscale app). The CLI prints the link only (no QR code: the workspace has no
   QR encoder); send it to the phone whole, including the part after `#` (AirDrop, or a
   note that syncs).
6. **(phone)** With the Tailscale app on and signed in to the same account, open the link.
   The phone asks for a device name and shows a 6-digit number.
7. **(Mac)** The terminal shows the phone's name and asks you to TYPE the six digits you
   see on the phone (it never prints them): right digits pair it, Enter alone refuses,
   three wrong tries refuse it. A new link voids any request still waiting, and the
   terminal only asks about a phone that used the link it just printed.
8. **(phone)** Add the page to the home screen if you like.

**To cut access**: `sbxw remote revoke <id>` (one phone, at once: its open page loses
access immediately), or `sbxw remote off` (everything: the listener and the tailnet machine
stop). A phone unused for 30 days is unpaired by itself. "Forget this device" on the phone
does the same as a revoke.

### What a phone can and cannot do

| A paired phone can | A paired phone cannot |
|---|---|
| List the running team leaders (the main session of a running sandbox that leads a team), and see which needs you | See any other session: threads, other sandboxes, Jarvis |
| Read a leader's conversation: your prompts, its replies, messages from other sessions and from sbxw, questions. Its replies are sent as written (masked by your anonymise rules only), so a reply that quotes a file reaches the phone | See any tool's input or output, a file, a diff, a terminal, or the harness's own output (a `!` command's, a slash command's, hooks', task notifications, system reminders). These are shown as "3 steps" and nothing more |
| Send it text, typed or dictated, as your own words (masked first by your anonymise rules), when it is idle at its prompt (see "What a message can do" below) | Type raw keys, open a terminal, create a sandbox or a thread; send a line starting with `!` (a shell command), `/` (a slash command) or `#`; send a control or invisible character; write to a leader that is working or waits for an answer at the Mac |
| Answer a multiple-choice question (`AskUserQuestion`) | Allow a tool (a permission prompt): "This needs your Mac" |
| Interrupt it (one Escape, as at the keyboard; it may take a few seconds while a line is being typed into that session) | Approve or refuse a plan, a merge, a network request, or remove a sandbox, without Face ID at each action and level 2 turned on at the Mac (see "Approving from the phone") |
| Dictate (the Mac transcribes; the text comes back to the phone to correct) | Touch secrets, the GitHub token, network rules, ports, kits, MCP servers, configuration, the relay, Jarvis, the daemon |
| Hand a file to a leader (a photo, a PDF, a text): up to 25 MB each, at most 5 waiting (60 MB together) per leader; it reaches the sandbox only with a message you send, under `/tmp/sbxw-uploads/`; an unsent one is deleted after 30 minutes (see "What a file can do") | Send a program or an archive (refused by their first bytes); read a file back, list the uploads folder or learn any path; hand a file to anything but a running team leader; have a file typed or opened by the upload itself |

These limits are enforced by the daemon, not by the page: the routes for everything in
the right column do not exist on the remote listener at all.

### What a message can do

A message is typed into the leader's terminal as your words, so the daemon holds it to
what lands in the leader's prompt and nothing else:

- **Only plain text.** A message with any line starting with `!`, `/` or `#` is refused
  (Claude Code would run a `!` line as a shell command at once, and a `/` line is a slash
  command such as `/permissions` or `/mcp`). So is any control character except a line
  break (tab, Return, Escape and the like), any bidi control, and any invisible character
  (zero-width spaces and joiners, a byte order mark, a soft hyphen...): what you see is
  exactly what is typed. The text is refused, never rewritten. Every line is checked, not
  only the first: whether Claude Code could read a later line of a paste as a command was
  not verified, so none may look like one. The check runs again on the exact text typed,
  after your anonymise rules: an alias that would start a line with `/` is refused too.
- **Only into an idle leader.** A message is typed only while the leader is idle: at its
  prompt, its turn over, nothing running. A leader at work can be read and interrupted
  from the phone, not written to ("The leader is working"). A message is also refused
  ("This needs your Mac") while the leader waits for you (a permission prompt, a question,
  which the phone answers with its own button and never by typing, any other dialog) or
  when sbxw does not know its state.
- **As one paste, Return last.** The text goes as a single bracketed paste, then sbxw waits
  for the echo and 600 ms more, checks that the leader is still idle, and only then presses
  Return. If it is not, there is no Return: the phone is told "not sent" and the text waits
  in the leader's input. If the leader's terminal has bracketed paste off, nothing is typed.
  Nothing else sbxw types into that session (a composer's message, sbxw's own lines, an
  answer, Interrupt) can land between the paste and the Return.
- **Design assumption, not verified:** that a Claude Code dialog does not read a paste as
  key presses. It matters only if a dialog is on screen when the paste arrives, which the
  idle rule is there to prevent.
- **Residual window.** Claude Code opening a dialog on its own while idle, in the moment
  between sbxw's idle check and the paste, or between the second check and the Return,
  would receive that paste or that Return. An idle Claude Code waits for input and is not
  known to open dialogs by itself, so this window is small, not zero. Also, "idle" is what
  the hooks last told sbxw: if a tool call's notice were lost and the permission notice
  that follows were lost or late too, sbxw could believe the leader idle while a dialog is
  on screen, and a Return could reach it.
- **Not verified on a real Mac:** that Claude Code turns bracketed paste on at its prompt.
  If it does not, every message from the phone is refused (the safe failure).
- A message already being typed when you revoke the phone or turn remote access off
  finishes (a few seconds).
- **Interrupt may take a few seconds** while a line is being typed into that session: it
  waits behind the same typing lock (up to about 4 s behind a phone message, longer behind
  a line sbxw types into a leader that is still starting), so an Escape never lands inside
  a paste.

### What a file can do

A file from the phone is data handed to a leader, never an action:

- **Held on the Mac until you send it.** The phone streams the file to the Mac, where the
  daemon keeps it in `~/.sbxw/remote/uploads/` (folder `0700`, file `0600`, random name)
  and gives the phone an opaque handle bound to that phone and that leader. Nothing is
  typed and nothing reaches the sandbox yet: the leader learns of a file only from a message
  you send with it.
- **Sent with a message.** When a message names the handle, the daemon copies the file into
  the leader's sandbox with the web UI's own upload writer, to
  `/tmp/sbxw-uploads/<time>/<name>`, and types after your text one fixed block of its own
  paths ("Attached files (uploaded to this sandbox):" and one `- <path>` line per file), the
  same block as the web chat. Everything "What a message can do" says holds for the whole
  typed text, block included: only into an idle leader, one paste, the checks on the exact
  text typed, Return last. A file is copied only if the leader takes a message at that
  moment. The handle is spent only once the Return is sent; if the message is refused or
  not sent, the handle stays usable for a retry (a file already copied is not copied again).
- **Its name is the daemon's.** The phone's name is only a hint: the daemon keeps its last
  part, turns every character outside `A-Z a-z 0-9 . _ -` into `_` (so no space, control,
  invisible or look-alike character), drops leading dots (no hidden file, no `..`), keeps
  120 characters at most. So no line of the block can start with `!`, `/` or `#`, and the
  final check on the whole text would refuse it anyway. The daemon does not refuse a name
  that looks like a secret (`.env`, `*.pem`): the files on your phone are yours, and the
  name is never written to the journal or to the daemon's logs at info level and above.
- **Refused kinds**, by their first bytes whatever the name: native programs (ELF, Windows
  `MZ`, Mach-O) and archives (zip, tar, gzip, 7z, rar, xz, bzip2, zstd), and empty files.
  Everything else is data: pictures, PDF, text (a script included: it is text an agent
  reads, like a message), other bytes.
- **Limits.** 25 MB per file (counted while it arrives, whatever the phone declares), at
  most 5 files and 60 MB waiting per phone and leader, 200 MB for all phones, one upload at
  a time per phone, 20 uploads per 10 minutes per phone (one phone's limit never holds
  another back), the whole file within 180 s and no pause over 30 s. The Tailscale node
  takes a body of 26 MB at most and gives it 120 s (the person's decision): a 25 MB file
  needs about 1.7 Mbit/s of uplink. On a slower link the upload fails: the node cuts it at
  120 s, the phone gets an error, nothing is kept on the Mac, and you retry on a better
  connection or with a smaller file (each try counts toward the 20 per 10 minutes).
- **How long.** An unsent file is deleted from the Mac 30 minutes after its upload, or when
  you remove it on the phone, when the phone is revoked or logs out, when remote access is
  turned off, and when the daemon restarts. A file sent with a message stays in the
  sandbox, like the web chat's uploads; so does a file copied for a message that was then
  not sent. No route of the phone removes a file from a sandbox.
- **The phone never sees a path.** No answer names the uploads folder, and the
  conversation shown on the phone shows `name` where the text said
  `/tmp/sbxw-uploads/<time>/name`.
- **Logs.** The journal records `upload` with a size class (under 1 MB, 1 to 10 MB, over
  10 MB), never a name or a content. The daemon's info lines name a device and a size
  class. Known gap: the `sbx` runner's own debug line prints every command it runs, so at
  debug level the destination path (with the file's sanitised name) appears, as for every
  web UI upload; the content never does.

### Check on your Mac before relying on it

1. Pair the phone with `sbxw remote pair` and type the digits it shows.
2. Send one message from the phone to an idle leader and see it submitted in its terminal.
   While the leader works, check that the phone says "The leader is working" and types
   nothing.
3. Make a leader ask for a permission and check that a message from the phone is refused
   ("This needs your Mac") and nothing reaches the terminal.
4. From the phone, hand a photo to an idle leader with a message: the leader's terminal
   shows your text and the block with one `/tmp/sbxw-uploads/...` path, and the file is
   there in the sandbox. A large photo or video over 25 MB is refused with "too large".
5. On mains power with remote access on, check the Mac does not idle to sleep
   (`pmset -g assertions` shows `caffeinate`); on battery, that it may.

### The threat model, in plain words

**What sbxw protects against**

- *Someone on the internet.* The remote listener binds to loopback only
  (`remote_addr`, default `127.0.0.1:7683`; any other address is refused at load). Only
  `sbxw-tailnet` reaches it, and it only serves your tailnet. There is no public address.
- *Someone else on your tailnet* (a shared tailnet, a compromised device of yours). Being
  on the tailnet is not enough: every data route needs a **paired device token**. Pairing
  needs a one-time code shown on the Mac (192 bits, 5 minutes, single use) AND you typing
  on the Mac the 6 digits the phone shows. The sidecar also lets only your own devices
  through (see "The Tailscale node").
- *A stolen or lost phone.* Revoke it: access ends at once, including open pages. Unused
  for 30 days, it is dropped anyway. The phone keeps no conversation (no storage, no
  service worker).
- *A web page you visit, on the phone or the Mac.* The token is an `HttpOnly; Secure;
  SameSite=Strict` cookie named `__Host-sbxw_device`, bound to the tailnet name. Every
  write must carry the page's own `Origin`; the `Host` must be the tailnet name. No CORS,
  no framing, a strict Content-Security-Policy (`default-src 'self'`).
- *Another process on the Mac.* It can reach the loopback listener, but without a device
  token it gets only the page and the two pairing calls. Its failures (wrong codes, unknown
  tokens) are rate-limited per source and in total, and the global limits hold whatever
  source it claims; a paired phone, the valid pairing code and a request the phone was
  given are looked up first and never held back by those failures. It cannot read the
  token: the daemon stores only a SHA-256 of it, in `~/.sbxw/remote/state.json` (`0600`).
- *The UI token leaking to the phone, or the device token to the UI.* Neither listener
  accepts the other's token. The phone's token has no other power than the list above.
- *Your words in logs.* The journal records which device did which kind of thing and when,
  never the text of a message, an answer or a transcription, nor a file's name or content;
  neither do the daemon's logs (tests capture them), which at info level name only a
  device and a kind of action (and a size class for a file). No
  Tailscale user, login or machine name is read or kept.

**What it does not protect against**

- Someone who holds your **unlocked, paired phone**: they can do what you can from it (the
  left column). Revoke it from the Mac.
- Someone who controls your **Mac user account**: they control sbxw anyway.
- Someone who controls your **Tailscale account**: they can add a device to your tailnet,
  but still cannot pair without your `y` on the Mac.
- **Tailscale itself.** Traffic is end-to-end encrypted between your devices (WireGuard,
  then HTTPS terminated by `sbxw-tailnet` on the Mac); Tailscale's coordination server
  sees which devices exist, not the content. Check Tailscale's terms for your situation.
- The **machine name is public**: HTTPS certificate names go to public Certificate
  Transparency logs. Keep the neutral name `sbxw`.
- **A closed lid.** While on and on mains power, sbxw keeps the Mac from idling to sleep
  (`caffeinate`; no system setting is changed). On battery, or with the lid closed and no
  external display, macOS may sleep, and the phone then says sbxw is unreachable.

### How it fits together

```
phone (Safari, on your tailnet)
  └─ HTTPS ─▶ sbxw-tailnet (on the Mac, machine "sbxw" of your tailnet; terminates TLS)
                └─ HTTP, loopback ─▶ daemon remote listener 127.0.0.1:7683
                                       (its own router; device token)
             daemon UI listener 127.0.0.1:7681  ◀─ never reached from the phone
```

- The daemon starts `sbxw-tailnet` (found next to the `sbxw` executable, never on `PATH`)
  when remote access is turned on, and at daemon start while it is on; it stops it when it
  is turned off. The sidecar writes its state to `~/.sbxw/remote/tailnet.json`, which the
  daemon reads as untrusted input.
- State under `~/.sbxw/remote/` (directory `0700`, files `0600`): `state.json` (on/off,
  devices with hashed tokens), `journal.jsonl` (the last 1000 entries), `tailnet.json`,
  `tailnet/` (the node's keys), `uploads/` (files a phone handed over and has not sent yet;
  emptied at daemon start).
- The Mac side is driven through admin routes on the UI listener (`/api/remote*`, UI
  token as for every UI route): the CLI now, the desktop app later. Jarvis cannot turn
  remote access on or move its listener.
- The remote listener's routes are built from one explicit list; a test walks every route
  of the UI router and checks that each is absent from the remote listener, and that every
  remote data route refuses the UI token and no token.

### The phone page

One page, made for a phone and usable on a tablet, served by the remote listener at the
root of `https://<tailnet name>/`. It shares no script with the main web UI: its files are
`assets/remote/` (`index.html`, `remote-core.js`, `remote.js`, `remote.css`,
`manifest.webmanifest` and three icons made from the sbxw mark), plain JavaScript, no
framework, no build step, no font, nothing from another origin. "Add to Home Screen" gives
a standalone app named sbxw.

#### Screens

- **Pairing.** Opened from the link or QR code the Mac shows (`#pair=<code>`). The page
  reads the code from the fragment and removes it from the address bar before any request,
  so it is in no history entry, request line or `Referer`. It asks for a device name
  (prefilled from the platform: "iPhone", "iPad"...), then shows six digits in large type
  under "Type this number on your Mac": the Mac never shows them, the person types them
  there. It polls every 1.5 s until the Mac accepts or refuses. A new pairing link voids a
  pending request: "This request expired. Open the new link from your Mac." Refused (also
  after three wrong numbers), invalid and rate-limited each say so; nothing is retried.
- **Not paired.** No code and no device cookie, or a `401` anywhere, or the stream's
  `bye`: "This device is not paired" / "no longer paired" / "Remote access is off".
- **Leaders.** The running team leaders: name, state (needs you, working, idle) and the
  last activity. Those that need the person come first, then those at work. One tap opens
  one. An empty list says "No team leader is running".
- **A conversation.** Like a chat: the leader and other agents on the left (green tint),
  the person on the right (blue tint), "From sbxw" lines neutral, a run of tool steps as
  one quiet line ("3 steps"). Older entries load when scrolled to the top; new ones arrive
  on the `history` nudge of `/r/events`. A pending multiple-choice question is a card: pick
  an option, then "Send answer". A permission prompt or anything else the phone cannot
  settle says "This needs your Mac."
- **Composer.** At the bottom on its own ground: a field that grows to a few lines, the
  microphone, Send, and below them Interrupt, which asks "Interrupt <leader>?" and acts
  only on a second tap. Enter is a new line: only Send sends. A message takes 1 to 4 s
  (the daemon waits for the echo before the Return): Send reads "Sending…" and a second tap
  does nothing. A message is typed only into an idle leader: while it works, Send is
  disabled with "The leader is working: Send comes back at its prompt.", the text stays in
  the field and nothing is queued; Interrupt stays available. A line starting with `!`, `/`
  or `#`, a control character or an invisible one (zero-width space, soft hyphen...) is
  warned about in place while typing; the daemon stays the judge. Each refusal reason has its own
  sentence, with the daemon's words beside it as text; "not_submitted" says plainly that
  the text waits unsent in the leader's input on the Mac, keeps it in the field, and
  nothing is resent.
- **Dictation.** Tap the microphone to record, tap again to stop (two minutes at most).
  The audio is turned into 16 kHz samples on the phone, sent to `/r/voice/transcribe`, and
  the text is put INTO the field for the person to read and correct. It never sends.
  Without HTTPS, without a microphone or with the permission refused, the page says why.
- **Unreachable.** When sbxw does not answer: "sbxw is unreachable (is the Mac awake?)".
  The text being typed stays in the field. Nothing is queued: a message that did not go is
  reported, and the person presses Send again when sbxw is back.
- **Menu.** The device's name and "Forget this device" (second tap confirms), which calls
  `/r/logout`.

#### What the page keeps

Only the device name, in `localStorage` under `sbxw-remote-prefs`, to prefill pairing. No
message, answer, transcription or conversation is stored on the phone (no
`sessionStorage`, IndexedDB, Cache API or service worker), and nothing is written to the
console. The device token is an HttpOnly cookie the page never sees.

#### Agent text is data

Every text from the Mac (an agent's reply, a leader's or sandbox's name, a question and
its options) is set as text, except an agent's reply, which goes through the fixed
Markdown subset of the main UI (`chatMarkdown`, copied into `remote-core.js` and kept equal
by a test): escaped first, then only paragraphs, lists, code, bold, italics and links. A
link shows its address beside its label, opens only on a tap, in a new context, and only
for `http` and `https`; the page checks the elements once more after rendering. File paths
stay plain text.

#### Tests

`node --test tests/js/remote-core.test.mjs` (no browser) and
`tests/js/remote-page.test.mjs` (Chromium through Playwright, skipped without them:
`PLAYWRIGHT_MODULE`, `CHROMIUM_PATH`), which runs the page against
`tests/js/remote-standin.mjs`, a stand-in of the remote listener with the listener's CSP.

### The Kanban on the phone

From a leader's screen the phone can read that leader's sandbox Kanban and open a card. It is level 1 (reading), so any paired device can do it while remote access is on. Two routes, both `GET`, only on the remote listener: `/r/leaders/:sandbox/kanban` (the board) and `/r/leaders/:sandbox/kanban/:ticket` (one ticket), plus a `kanban` event on `/r/events` that carries the sandbox's name and a hash, so the page knows to read again. Contract: `.sbxw-artifacts/remote/API-kanban.md`.

- **Same door.** A valid device token, the Host and Origin checks, no CORS, `Cache-Control: no-store`, the per-device limits. The UI's ticket routes are not on this listener, and these two are not on the UI's. A write method on either is refused.
- **Which sandbox.** Only a running team leader's, by the rule `/r/leaders` uses. A stopped sandbox, one that leads no team, an unknown name and a malformed one all get the same 404, so the answer does not say whether a sandbox exists. The name and the ticket id are checked for length and characters before any lookup. A ticket of another sandbox is the same 404.
- **What it shows.** What the Mac's board shows for that one sandbox: columns, cards with their marks, the linked thread and its state, age, and for a ticket its title, description, history and the names, sizes and kinds of its files.
- **What it cannot do.** It cannot add, move, edit, delete or send a ticket. It cannot read a file's content: there is no file route, and no file id, no on-disk name and no path leaves the Mac. It shows no other sandbox and no "all sandboxes" view. It has no write permission field at all.
- **Text is data.** Ticket titles and descriptions are the person's words; thread titles and reports are agent text. They travel as JSON strings, bounded (title 160 characters, description 8,000, file name 120, history 60 lines of 200), and the page escapes them. A word that starts like an absolute path (`/x`, `~/x`, `C:\x`) is replaced by `[path]`, so a path of the Mac or of the sandbox does not leave in free text. This is a filter on the shape of a word, not a guarantee: a path written some other way in a ticket is the person's own text, shown to their own paired phone.
- **Not journaled, not logged.** Reads are not actions, so the journal has nothing for them, and no log line names a ticket's title or text.

### Approving from the phone

Level 2: with Face ID (or Touch ID) at **each** action, a paired phone can approve a few
things that otherwise wait for you at the Mac. It is **off by default**, turned on only at
the Mac, and turned off with remote access. Contract:
`.sbxw-artifacts/remote/API-level2.md`.

**What a phone can approve, and what it is shown first**

| Action | What the phone shows before you decide | What runs |
|---|---|---|
| Approve or refuse a plan, as it is | its sandbox, summary, justification, variant, and each thread's name, model, repo, scope, whole goal and ticket | the desktop's approve (the plan's own threads, nothing edited) or refuse (no note) |
| Run a merge a lead proposed | the sandbox, the order, the lead's note and distillate, and per branch its tip, commits ahead of the base, files changed with insertions and deletions (numbers and paths, never diff content), and whether an earlier try of this merge failed on it | the desktop's merge: same order, stops at the first conflict, threads kept, distillate as proposed |
| Answer a pending network request | the sandbox, the host and port, sbx's detail, the options the phone may use and those it may not | sbx's own answer, with one of that request's options |
| Remove a stopped sandbox | the desktop's sentences: what is lost, that the workspace folder stays | the desktop's removal |

What you see is what you approve: the daemon binds each Face ID check to a digest of exactly
what was shown. If the plan, a branch tip, the base, the request or the sandbox changed in
between (a plan edited, a branch moved, the request answered elsewhere, the sandbox started
or created again under the same name), nothing is done and the phone says it changed.

Agent text (goals, notes, the distillate, sbx's detail) reaches the phone as data, masked by
your anonymise rules, and the page shows it as text.

**What stays on the Mac**

- A leader's tool permission prompt ("allow this command"): the phone says "This needs your
  Mac", and there is no route for it.
- Editing a plan's threads, writing a note on a refusal, closing threads after a merge,
  editing a merge's distillate.
- A network option that may leave a lasting rule. The phone offers only an option that says
  it is one-time ("once") or that refuses or dismisses the request without "always" or
  "rule". sbx 0.51 offers **Allow** and **Dismiss** for a refused host, and whether its Allow
  writes a lasting rule has not been checked, so today the phone can only **dismiss**: Allow
  is answered at the Mac. A host port request (`port:`) is port publishing and is never
  listed on the phone.
- Starting or stopping a sandbox, removing a running one, removing the Jarvis sandbox.
- Everything level 3 already kept: secrets and the GitHub token, permanent network rules,
  port publishing, kits, MCP servers, configuration, relay routing, Jarvis proposals, any
  terminal, raw input, shutdown.

**Turning it on and off, setting up Face ID, revoking**

```
sbxw remote approvals on              # asks first (y/N), says what it allows; needs remote access on
sbxw remote approvals status          # on/off, and each device: Face ID set up or not
sbxw remote approvals allow <device>  # let that phone set up Face ID once: type the six digits it shows
sbxw remote approvals revoke <device> # that phone can no longer approve
sbxw remote approvals off             # nothing can be approved from a phone; Face ID set-ups are kept
```

1. **(Mac)** `sbxw remote approvals on`.
2. **(phone)** Tap "Enable Face ID". The phone shows six digits.
3. **(Mac)** `sbxw remote approvals allow <device>` (the id from `sbxw remote devices`), then
   type the six digits. That allows ONE set-up, for 5 minutes.
4. **(phone)** Finish with Face ID. The phone now has a passkey for sbxw.

A paired phone cannot set up Face ID by itself: a stolen device cookie on another phone
would show its own digits, which you would never type. Jarvis and agents cannot turn level 2
on, allow a device or set up a passkey: these are admin routes behind the UI token, level 2
is remote state (not a `sbxw.toml` setting), and Jarvis's settings allowlist refuses it.

`sbxw remote revoke <id>`, the phone's "Forget this device" and 30 days idle drop the
device's passkey with it. `sbxw remote off` turns level 2 off too.

**What a passkey is, and its limits**

- The phone keeps a private key that Face ID or the phone's passcode unlocks; the Mac keeps
  only its public key, its id and a counter (`~/.sbxw/remote/passkeys.json`, `0600`). No
  secret is stored on the Mac.
- **Anyone who can unlock the phone, by Face ID or by its passcode, can approve.** The
  passkey proves the phone and an unlock, not who is holding it. If a passkey syncs through
  iCloud Keychain, every device of that Apple account that can unlock it can approve too.
- One passkey per device, made for one tailnet name. If the tailnet name changes (a new
  tailnet, the machine renamed), the passkey stops working and the phone says to set up
  Face ID again.
- Each action needs a fresh Face ID check: a challenge from the Mac, good for 60 s, once,
  for that action on that item. No PIN, no "confirm" button, no approval by the paired
  cookie alone.
- Five failed checks in a minute (twenty in an hour) pause that phone's approvals for the
  rest of the window; other phones are not affected. A signature counter that goes back (a
  sign the key was copied) drops the passkey: allow the phone again at the Mac.
- sbxw asks the browser for the phone's built-in authenticator but cannot prove that is what
  answered (no vendor attestation is asked for): the trust comes from your typing the six
  digits at the Mac.

**The journal** (`sbxw remote journal`) records each try: the device, the time, the kind
(`plan_approve`, `plan_refuse`, `merge`, `network_respond`, `sandbox_remove`,
`passkey_registered`, `passkey_failed`, `changed`, `refused`) and what was acted on by name
only (a plan id, a sandbox name, a host). Never a goal, a note, a diff or a path; the
daemon's info log says only "device <id>: <kind>".

**Not verified on real hardware:** Face ID on a real iPhone, Safari's WebAuthn on a
`.ts.net` name, a real merge started from the phone, and whether sbx's Allow on a refused
host writes a lasting rule.

### The Tailscale node

Remote access reaches the Mac through `sbxw-tailnet`, a small program shipped beside `sbxw` and started by the daemon while remote access is on. It joins your tailnet as its own machine, named `sbxw`, and serves only HTTPS on that machine's port 443, forwarded to the daemon's remote listener on loopback. It is not the Mac's own Tailscale: nothing else on the Mac is reachable through it, and nothing needs installing besides sbxw. Details: [tailnet/README.md](../tailnet/README.md).

- **First join:** `sbxw remote status` shows a Tailscale login link; open it in any browser (the Mac's is fine) and add the machine. No auth key is asked for or stored.
- **HTTPS is required.** Turn on MagicDNS and HTTPS Certificates on the DNS page of the admin console (login.tailscale.com/admin/dns). Without them sbxw says what is missing and serves nothing: the phone's microphone needs HTTPS, so there is no plain-HTTP fallback.
- **The name is public.** Certificate names go to the public Certificate Transparency logs, so keep the neutral `sbxw`.
- **Only your tailnet.** Funnel (public Internet), SSH, Taildrop, exit node and routes are never enabled, and most are compiled out. Tailscale's log upload is off.
- **Its state** (`~/.sbxw/remote/tailnet`, private to you) holds the node's keys and certificate. "Forget this node" logs it out and deletes that folder; then remove `sbxw` from the Machines page of the admin console if it is still listed.
- **Only your own devices.** The node lets through only untagged devices logged in with the same Tailscale account that added it, and not shared in from elsewhere. Other people's devices on a shared tailnet, tagged servers and shared-in machines get a plain refusal before anything reaches the daemon.
- **Headers:** the daemon may receive the peer's tailnet address, as a hint for rate limits, never as proof of anything. No user, login or machine name is passed on. A phone is trusted only once paired.

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
  approved before are asked about; removing a setting asks nothing, except
  that a removed or weakened `[[carve]]` gives access back and is asked about
  (`access restored: ...`). The CLI
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
