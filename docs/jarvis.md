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

Jarvis needs the same Claude sign-in as your other sandboxes. The voice is
optional: you can type to Jarvis and read its answers.

## What Jarvis can change

Each proposal holds one or more changes. Jarvis can propose:

| Change | What it does | Risk |
|---|---|---|
| **Kit** (`kit_write`, `kit_remove`) | Create, update or remove a kit sbxw keeps for you under `~/.sbxw/state/kits/jarvis-<name>/`: files placed under `/home/agent/` (skills, slash commands, scripts) and optional text for the agent's `CLAUDE.md` | High |
| **Kits line** (`kits_line`) | Add or remove a kit in the `kits` list of the daemon's `sbxw.toml` or of one sandbox's. Only bundled kits (`sbxw:<name>`) and Jarvis kits | Low when it only adds bundled kits or only removes; high when it adds a Jarvis kit |
| **Setting** (`setting`) | One key of the daemon's `sbxw.toml`, from a fixed list: the `[voice]` keys, `claude_model`, the terminal colours, `keep_awake`, `keep_awake_min_battery`, `battery_warn_below`, `cache_ttl_days`. It takes effect when the daemon restarts | Low |
| **Network** (`network_allow`) | Add or remove hosts in the daemon's `network_allow`, for sandboxes created after the daemon restarts. Bare host names only: no address, no port, no wildcard, not this machine. Approving records the same consent as any new grant | High |
| **MCP server** (`mcp_server`, `mcp_remove`) | Add, replace or remove an MCP server, as the MCP tab of Setup does | High |

A proposal is as risky as its riskiest change. Every change is shown to you
as a diff of what would be written, before anything is.

The daemon keeps one proposal pending at a time. Jarvis waits for your
answer, and is told what came of it: applied, refused (with your reason, if
you gave one) or failed (with the error). Proposals are kept in
`~/.sbxw/state/jarvis.json`: every pending one and the last 50 settled.

## What Jarvis can never do

The daemon builds every write itself, from data Jarvis sends. So, by
construction:

- **No path of its choosing on your machine.** A file is named by a key
  (the daemon's `sbxw.toml`, a sandbox's), and a kit always lives under
  `~/.sbxw/state/kits/jarvis-*`. Kit files go under `/home/agent/` in the
  sandbox, never into sbxw's own files or Claude Code's settings (no hooks,
  no permissions).
- **No command of its choosing.** sbxw writes the kit's `spec.yaml` itself:
  image content and a `chmod` for scripts. A Jarvis kit has no install
  command and no network rule.
- **No secrets.** A change that carries something shaped like a key or a
  token is refused, and so is an MCP server with a credential in a header,
  its URL or an argument. Jarvis writes a `${key}` placeholder instead, and
  you fill in the secret in the MCP tab of Setup.
- **No setting outside the list above**, and no value the config check
  would refuse.
- **Nothing without you.** There is no auto-approve. A proposal you have not
  approved changes nothing.

## How approval works

A proposal opens a dialog under the top bar. It does not take the keyboard
from a terminal: click into it to use it. It shows:

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
