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
away from the Mac: read their conversations, type or dictate to them, answer their
multiple-choice questions, interrupt them. It goes through your own private Tailscale
network (a "tailnet"); nothing passes through a server of sbxw's or anyone else's, and no
conversation leaves the Mac except to your phone.

It is **off by default**. This is step 1 of three: nothing irreversible can be done from a
phone yet (no approval of a plan, a merge or a network request, no removal).

The wire contract is `.sbxw-artifacts/remote/API.md`; this section says what it is for and
what it protects.

### Turning it on, pairing, turning it off

```
sbxw remote on          # asks first, says what it exposes
sbxw remote status      # on/off, the tailnet machine, devices, keeping awake
sbxw remote pair        # prints a one-time link, then asks for the digits the phone shows
sbxw remote devices     # paired devices
sbxw remote revoke <id> # unpair one, at once
sbxw remote journal     # what devices did, never what they wrote
sbxw remote off         # stop everything; paired devices are kept
sbxw remote forget-node # (when off) log the tailnet machine out and delete its state
```

1. `sbxw remote on`. The daemon opens its remote listener and starts `sbxw-tailnet`, which
   joins your tailnet as a machine named `sbxw` (the first time, `sbxw remote status`
   shows a Tailscale login link).
2. `sbxw remote pair` prints a link `https://sbxw.<your tailnet>.ts.net/#pair=<code>`. Open
   it on the phone (the phone must be on the same tailnet, with the Tailscale app). The
   code is good once, for 5 minutes. The CLI prints the link only: drawing a QR code in
   the terminal would need a new library.
3. The phone asks for a device name and shows a 6-digit number. The terminal shows the
   name and asks you to TYPE the six digits you see on the phone (it never prints them):
   right digits pair it, Enter alone refuses, three wrong tries refuse it. A new link
   voids any request still waiting, and the terminal only asks about a phone that used the
   link it just printed.
4. Add the page to the phone's home screen if you like.

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
| Interrupt it (one Escape, as at the keyboard) | Approve or refuse a plan, a merge, a network request; remove anything |
| Dictate (the Mac transcribes; the text comes back to the phone to correct) | Touch secrets, the GitHub token, network rules, ports, kits, MCP servers, configuration, the relay, Jarvis, the daemon |

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
  known to open dialogs by itself, so this window is small, not zero.
- **Not verified on a real Mac:** that Claude Code turns bracketed paste on at its prompt.
  If it does not, every message from the phone is refused (the safe failure).
- A message already being typed when you revoke the phone or turn remote access off
  finishes (a few seconds).

### Check on your Mac before relying on it

1. Pair the phone with `sbxw remote pair` and type the digits it shows.
2. Send one message from the phone to an idle leader and see it submitted in its terminal.
   While the leader works, check that the phone says "The leader is working" and types
   nothing.
3. Make a leader ask for a permission and check that a message from the phone is refused
   ("This needs your Mac") and nothing reaches the terminal.
4. On mains power with remote access on, check the Mac does not idle to sleep
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
  never the text of a message, an answer or a transcription; neither do the daemon's logs
  (tests capture them), which at info level name only a device and a kind of action. No
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
  `tailnet/` (the node's keys).
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

### The Tailscale node

Remote access reaches the Mac through `sbxw-tailnet`, a small program shipped beside `sbxw` and started by the daemon while remote access is on. It joins your tailnet as its own machine, named `sbxw`, and serves only HTTPS on that machine's port 443, forwarded to the daemon's remote listener on loopback. It is not the Mac's own Tailscale: nothing else on the Mac is reachable through it, and nothing needs installing besides sbxw. Details: [tailnet/README.md](../tailnet/README.md).

- **First join:** sbxw shows a Tailscale login link; open it and add the machine. No auth key is asked for or stored.
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
