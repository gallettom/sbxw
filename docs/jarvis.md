# Jarvis

[← Back to README](../README.md) · [Docs index](README.md)

Jarvis sits in the middle of sbxw's top bar. It is two things:

1. **The voice of sbxw.** You dictate, agents' messages are read aloud, your
   words go to the sandbox you picked. This runs on your machine, with two
   local models and no language model. See
   [the Web UI guide](web-ui.md#jarvis) for the microphone, the keys and the
   masking of names.
2. **sbxw's assistant.** A Claude session in a sandbox of its own,
   `sbxw-jarvis`, that you talk to like any agent. It can **propose** changes
   to sbxw's configuration. The daemon checks each proposal, you approve or
   refuse it, and only then does the daemon write it.

- [Starting it](#starting-it)
- [What Jarvis can change](#what-jarvis-can-change)
- [What Jarvis can never do](#what-jarvis-can-never-do)
- [How approval works](#how-approval-works)
- [Its sandbox](#its-sandbox)
- [Removing it](#removing-it)

## Starting it

Nothing is created until you ask. Pick **Jarvis** where your words go (the
name in the bar, or the list at the top of the panel; it is the first entry
and the default). While its sandbox is not running, the panel offers
**Start Jarvis**. The first start creates the sandbox, which takes a minute
or two; a later one starts the stopped sandbox and opens Jarvis's session,
with no terminal to open. The panel shows where it stands. After that, what you say or type
goes to Jarvis's Claude session, masked first like any message, and its
answers are shown and read aloud.

The panel reads as a conversation: your words on the right, Jarvis's
answers as plain text on the left, in full (only the panel scrolls). With
Jarvis picked, whatever you say or type next goes to Jarvis, so there is
nothing to arm. The small speaker on an answer reads it again; it shows once
the voice is set up. The text box and the microphone share the bottom row.

Jarvis needs the same Claude sign-in as your other sandboxes. The voice is
optional: you can type to Jarvis and read its answers, even in an sbxw built
without voice or with the voice models not installed. Press Enter to send.
If the message cannot leave, the panel says why and keeps it in the input.

## What Jarvis can change

Each proposal holds one or more changes. Jarvis can propose:

| Change | What it does | Risk |
|---|---|---|
| **Kit** (`kit_write`, `kit_remove`) | Create, update or remove a kit sbxw keeps for you under `~/.sbxw/state/kits/jarvis-<name>/`: files placed under `/home/agent/` (skills, slash commands, scripts) and optional text for the agent's `CLAUDE.md` | High |
| **Kits line** (`kits_line`) | Add or remove a kit in the `kits` list of the daemon's `sbxw.toml` or of one sandbox's. Only bundled kits (`sbxw:<name>`) and Jarvis kits | Low when it only adds bundled kits or only removes; high when it adds a Jarvis kit, or `sbxw:ssh-agent`, `sbxw:git-identity`, `sbxw:dev-ports` or `sbxw:terminal-mac` (they reach your keys, identity, ports or a Mac) |
| **Setting** (`setting`) | One key of the daemon's `sbxw.toml`, from a fixed list: the `[voice]` keys, `claude_model`, the terminal colours, `keep_awake`, `keep_awake_min_battery`, `battery_warn_below`, `cache_ttl_days`. It takes effect when the daemon restarts | Low |
| **Pronunciation** (`voice_word`) | How the voice reads one word aloud: add or replace its entry in `[voice.words]` of the daemon's `sbxw.toml`, in French, English or both, or remove that entry (a built-in word gets its own pronunciation back). Only what is read aloud changes, never dictation. Used from the next sentence read, no restart. See [Words read aloud](configuration.md#words-read-aloud) | Low |
| **Status label** (`emphasis_word`) | One family of the [status pills](configuration.md#status-pills) in `[emphasis.words]` of the daemon's `sbxw.toml`, whole: its main word, tone, other words and `use` line, or its removal. The first change copies the built-in labels into the file. The terminals redraw at once; agents read the new guidance after their sandbox's next bring-up, in a new session | Low when it only removes words or changes a tone; high when it adds a word or writes a new `use` line, since that text goes into every sandbox's `CLAUDE.md` |
| **Status pills switch** (`emphasis_switch`) | `enabled` and `guidance` under `[emphasis]`, in the daemon's `sbxw.toml` (every sandbox: the terminals follow at once, the guidance at each next bring-up) or one sandbox's own (at its next bring-up) | Low |
| **Network** (`network_allow`) | Add or remove hosts in the daemon's `network_allow`, for sandboxes created after the daemon restarts. Bare host names only: no address, no port, no wildcard, not this machine. Approving records the same consent as any new grant | High |
| **Desktop screen** (`desktop_screen`, `desktop_screen_remove`) | Replace one screen of sbxw's desktop app with your own version, `~/.sbxw/desktop/screens/<name>.luau`, or remove your version so the app's built-in one comes back. Only a screen the app has (no new screen). The app reloads it at once. See [Desktop screens](#desktop-screens) | High |
| **MCP server** (`mcp_server`, `mcp_remove`) | Add, replace or remove an MCP server, as the MCP tab of Setup does. A remote server is a URL; a **stdio** server is a command that runs on your machine, outside any sandbox (see below). Jarvis never proposes an OAuth client ID: a server that needs one (GitHub's) is added in the MCP dialog | High |

Ask Jarvis "how do you say sbxw?" or tell it a word is said badly: it reads
the dictionary (the built-in words and yours, marked which) and proposes the
fix. The tools it has for all this, in its `sbxw-config` MCP server:
`sbxw_state` (read), `propose_kit`, `remove_kit`, `set_kits`, `set_setting`,
`set_voice_word`, `set_status_label`, `switch_status_labels`, `allow_network`, `propose_mcp_server`, `remove_mcp_server`,
`read_desktop_screen`, `read_desktop_docs`, `propose_desktop_screen`,
`remove_desktop_screen` and `check_proposal`.

If the daemon was started where there is no `sbxw.toml`, a setting or pronunciation change creates one at the path the daemon was asked to read. The dialog says it creates that file, and the diff shows a new file. The folder must already exist; sbxw does not create folders, and refuses the change otherwise.

A proposal is as risky as its riskiest change. Every change is shown to you
as a diff of what would be written, before anything is.

The daemon keeps one proposal pending at a time. Jarvis waits for your
answer, and is told what came of it: applied, refused (with your reason, if
you gave one) or failed (with the error). Proposals are kept in
`~/.sbxw/state/jarvis.json`: every pending one and the last 50 settled.

### Desktop screens

The desktop app draws parts of its window (the top bar, the sandbox list)
with small Luau files called screens. Each one is built into the app, and a
file of the same name in `~/.sbxw/desktop/screens/` replaces it. Ask Jarvis
to change one ("put the stop button on the right", "show the agent's name
in the top bar"): it reads the app's documentation and the screen as it is,
then proposes the whole new file.

- **What the daemon reads.** At each start the app writes, in
  `~/.sbxw/desktop/builtin/`, its built-in screens (`screens/<name>.luau`),
  the documentation of what a screen may use (`screens.md`) and the list of
  intents a screen may ask for (`intents.json`). The daemon only reads that
  folder. If the app has never run on this machine, Jarvis says so and
  proposes nothing.
- **The diff** is against what the app uses now: your version if you have
  one ("changes your topbar screen"), else the built-in ("replaces the
  built-in topbar screen"). Removing your version shows the way back to the
  built-in.
- **What the screen can do.** Above the diff, the dialog lists the intents
  written by name in the new screen (open a terminal, stop a sandbox, ...),
  with their risk, and marks those the screen used now does not emit. It
  also names the lines where the screen builds an intent in a way that
  hides its name. The list comes from reading the text: a hint, not a proof,
  since a screen can hide an intent name. Read the diff, and check that
  each button's label says what it does. See
  [Security: Jarvis](security.md#jarvis).
- **Always high risk**: approved with a click on the diff, never by voice.
- **A broken screen** does not break the app: it keeps the last good view and
  shows the error in a red banner. Ask Jarvis to fix it, or to remove your
  version.
- **Limits:** only screens the app already has (it has no place for a new
  one), no new intent or element, at most 64 KB of text. The folder
  `~/.sbxw/desktop/screens/` is created when missing. The app also honours
  `$SBXW_DESKTOP_SCREENS` for its override folder; Jarvis always writes to
  `~/.sbxw/desktop/screens/`.

## What Jarvis can never do

The daemon builds every write itself, from data Jarvis sends. So, by
construction:

- **No path of its choosing on your machine.** A file is named by a key
  (the daemon's `sbxw.toml`, a sandbox's), and a kit always lives under
  `~/.sbxw/state/kits/jarvis-*`. Kit files go under `/home/agent/` in the
  sandbox, never into sbxw's own files or Claude Code's settings (no hooks,
  no permissions). Also refused: shell start files (`.bashrc`, `.profile`,
  `.zshrc` and the like), anything under `.ssh/` or `.claude/hooks/`,
  `.claude/CLAUDE.md` (the kit's context field is for that), and a
  `.local/bin/claude` that would replace the agent.
- **No command of its choosing in a sandbox.** sbxw writes the kit's
  `spec.yaml` itself: image content and a `chmod` for scripts. A Jarvis kit
  has no install command and no network rule.
- **No secrets.** A change that carries something shaped like a key or a
  token is refused, and so is an MCP server with a credential in a header,
  its URL or an argument. Jarvis writes a `${key}` placeholder instead, and
  you fill in the secret in the MCP tab of Setup.
- **No new screen, intent or element in the desktop app.** A screen
  Jarvis writes uses only what the app's core already offers, and the core
  checks every intent it emits.
- **No setting outside the list above**, and no value the config check
  would refuse. A pronunciation change writes one entry of
  `[voice.words]` and nothing else.
- **Nothing without you.** There is no auto-approve. A proposal you have not
  approved changes nothing.

### The one exception: a stdio MCP server

A stdio MCP server is a command that sbx runs **on your machine, outside
any sandbox**, with your rights. It is the one change Jarvis can propose that
runs code on the host. What guards it:

- the dialog shows a red banner, "This command will run on this machine,
  outside any sandbox", with the full command as plain text;
- **Approve** stays off until you tick **I have read this command**, and only
  that box tells the daemon you read it (`ack_host_command`). Without it the
  daemon refuses the approval (HTTP 409) and the dialog says so;
- it is high risk: never approved by voice. Jarvis reads the summary and
  says it must be read and approved on screen;
- the command's secrets are `${key}` placeholders you fill in Setup, as for
  any MCP server.

Read the command as you would a script someone sent you. If you don't know
what it runs, refuse it.

## How approval works

A proposal opens a dialog under the top bar. It shows:

- the summary, in Jarvis's words;
- a **Low risk** or **High risk** badge;
- each change, with its title and its diff, as plain text in a scrollable
  box (the diff is shown as text, never as a web page);
- **Approve** and **Refuse**, with an optional reason that Jarvis is told.

When you approve, the daemon plans the changes again against the files as
they are at that moment. If a diff differs from the one you read (a file
changed in between), nothing is applied and the proposal fails with that
reason. The changes are applied in order and stop at the first that fails:
the dialog then shows the result of each change, and the error.

**✕** means later. The bar's **Jarvis · N to review** brings the dialog back.

**Low risk leaves the terminals alone; high risk holds them.** A low-risk
proposal does not take the keyboard from a terminal (click into the dialog to
use it): it is answered by a spoken or typed yes, and the person may be in
the middle of typing elsewhere. A high-risk one (a kit, a network host, an
MCP server or its host command, a desktop screen, or anything else the daemon marks
high) must be read and clicked, so while it shows the terminals take nothing
from you (no keys, paste, mouse, wheel or file drop), a dimmed layer covers
them, and the focus goes to the proposal's controls. Otherwise a key meant
for the dialog, or an agent's prompt answered by habit, could land in a
terminal beside the change you are reviewing. Closing it, answering it, or a
proposal turning low risk frees the terminals and gives the focus back.
Jarvis's own panel stays usable throughout.

**By voice.** With the voice on, Jarvis reads the summary of a new proposal:

- **Low risk:** it asks whether to apply it. A spoken *oui* (or *yes*)
  approves it, *non* (or *no*) refuses it.
- **High risk:** it says that this one must be read and approved on screen.
  A spoken yes approves nothing; Jarvis only says it again. The daemon
  enforces the same rule: it refuses a voice approval of a high-risk
  proposal (HTTP 409), and the page says why rather than failing silently.

Prompts are spoken in French with a French voice (`[voice] tts_voice =
"fr_*"`), in English otherwise.

## Its sandbox

- **Name:** `sbxw-jarvis`, on this machine only. The name is reserved on
  every host: you cannot create a project sandbox under it. A sandbox of that
  name that sbxw did not create for Jarvis is never taken for it; starting
  Jarvis then says to remove it first.
- **Workspace:** an empty folder under sbxw's state, so the sandbox sees none
  of your projects and none of sbxw's files.
- **Kits:** sbxw's base kit and `sbxw:jarvis`, which installs the
  `sbxw-config` MCP server (the proposal tools) and the guidance that tells
  the session what it is. That kit is never offered in the Kits dialog, and a
  project's `kits` cannot list it.
- **Its role and its tools, at each start.** sbx 0.53 only lists a kit's
  guidance in `CLAUDE.md` and leaves the agent to read it if it seems
  relevant, so sbxw writes Jarvis's role into the sandbox's
  `~/.claude/CLAUDE.md` itself, in a block between
  `<!-- sbxw:jarvis:begin -->` and `<!-- sbxw:jarvis:end -->`, replacing the
  block, never adding a second. It also writes the `sbxw-config` server's
  script, `/home/agent/.sbxw/jarvis/config-mcp.js`, when the sandbox's copy
  is missing or differs from the one in this version of sbxw, and registers
  `sbxw-config` in `~/.claude.json`. It does this when it creates the
  sandbox and again before each Claude session starts there, in one
  command. A Jarvis made by an older sbxw is repaired and gets the new tools
  this way at its next start, without being recreated.
- **New tools need a new session.** Claude loads its MCP servers when its
  session starts. When sbxw updates the script while Jarvis's sandbox is
  already running, the session there may still have the old tools; the
  status note then says so: stop Jarvis's sandbox
  (`sbxw stop sbxw-jarvis`) and start Jarvis again. sbxw never restarts it
  by itself.
- **Checking it.** `GET /api/jarvis/status` adds `role_written`,
  `tools_registered` and `tools_updated` (what the last setup did:
  `tools_updated` is `true` when the script was rewritten, `false` when it
  was already current; all three are `null` when no session started since
  the daemon did) and a `note` in plain words when something could not be
  written or a restart is needed. A session that was already running keeps
  what it read when it started: stop the sandbox (`sbxw stop sbxw-jarvis`)
  and start Jarvis again to get a session that reads its role and has its
  tools. If the script cannot be written at all, recreating the sandbox
  repairs that: `sbxw rm sbxw-jarvis`, then start Jarvis.
- **Config:** sbxw's defaults, not your daemon's `sbxw.toml`. Only the
  listeners, the network policy and the model are taken from it. No mounts,
  no env, no ports, no shared skills, one session and no threads.
- **Who may call the proposal endpoints:** only `sbxw-jarvis`, on this
  machine. Every other sandbox is refused, whatever it sends.

## Removing it

1. Remove the sandbox: `sbxw rm sbxw-jarvis`. A pending proposal is never
   applied without your click, so nothing changes when it goes.
2. To remove the kits Jarvis wrote too, take them out of any `kits` line
   that lists them, then delete their folders, `~/.sbxw/state/kits/jarvis-*`.
   The history of proposals is `~/.sbxw/state/jarvis.json`; delete it too if
   you want it gone.

Picking **Jarvis** again later offers to start a fresh one. The voice does not
depend on the sandbox: dictation and reading aloud keep working without it.

See also: [Security: Jarvis](security.md#jarvis),
[Configuration](configuration.md), [Kits and skills](kits-and-skills.md#jarvis-kits).
