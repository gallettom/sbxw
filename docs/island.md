# Dynamic Island (macOS)

[← Back to README](../README.md)

**sbxw Island** (`macos/SbxwIsland`) is an optional native companion app. It
turns your Mac's notch and menu bar into a Dynamic-Island-style panel that
tracks every session, inspired by [vibeisland.app](https://vibeisland.app).

Install, build and the full behaviour reference are in
[`macos/README.md`](../macos/README.md). The `sbxw` installer offers it on
macOS, and `sbxw update` keeps it current.

- [What it shows](#what-it-shows)
- [Answering from the notch](#answering-from-the-notch)
- [Chatting from the notch](#chatting-from-the-notch)
- [Several agents in one sandbox](#several-agents-in-one-sandbox)
- [How session state works](#how-session-state-works)
- [Subscription usage](#subscription-usage)

## What it shows

Each session has a live state: **working**, **waiting for input**, **idle** or
**ended**, with the last prompt you sent, the agent's current activity, and
elapsed time.

- **A pill** hangs from the notch while something is happening (e.g.
  `👾 fix auth bug  3`), and hides when everything is idle.
- **A state change** drops a short toast.
- **Hovering the notch** shows the full list, with your **subscription usage**
  (5-hour and weekly) at the top. Aim at the notch itself; passing beside it on
  the way to another app's menus doesn't open it.
- **Click-through** until there's something to click: the pill and toasts let
  clicks fall through to the menu bar. A click elsewhere closes the open list.
- **After a turn ends**, rows and toasts show **what Claude answered**, from
  the `last_assistant_message` of Claude Code's `Stop` event, not "idle".
- Rows waiting on you have a **✕** to dismiss, and **Clear all N** appears when
  several are pending.

## Answering from the notch

- **`AskUserQuestion`:** the notch expands into a card with the question, each
  option's description, and a button per option (**⌘1**, **⌘2**, …). Several
  questions are walked step by step (**⌘←** goes back) and sent together.
- **A sandbox asking another one** ([relay](relay.md)): a card with the question
  and one button per running sandbox. When the answer comes back, the card
  offers to release or refuse it. The island **won't release an answer longer
  than ten lines**, since it has no editor; it sends you to the browser instead.
- **A screenshot request:** a card that can hand you to the browser (where you
  can paste an image) or refuse. It puts itself away after a few seconds so it
  doesn't end up in your own screenshot.

## Chatting from the notch

- **Open a row** (the chevron) to read Claude's full reply and write back into
  that sandbox.
- **＋ New chat** starts a throwaway sandbox: `ephemeral-chat`, then
  `ephemeral-chat-2`, … A freed name is reused. From four sandboxes up, it warns
  that each one holds disk and memory.

Both go through `POST /api/chat/push`, like the
[chat dock](chat.md#the-chat-dock). Creating a chat takes a while (a sandbox has
to boot); your text is kept if it fails.

## Several agents in one sandbox

A sandbox can hold more than one Claude Code session: the one sbxw attached,
plus anything over SSH (Claude Desktop, an editor, a shell on `<name>.sbx`).
Each gets its own row, keyed by Claude Code's `session_id`, badged `tty` or
`Desktop`. Clicking a `Desktop` row brings the Claude client forward instead of
the browser.

Which is which is established from evidence, not guessed:

1. **The process tree.** sbxw's session is `node < claude`; Claude Desktop runs
   its own server in the sandbox; an `sshd` ancestor marks a plain SSH session.
2. **sshd's environment** (`SSH_CONNECTION`, `SSH_CLIENT`, `SSH_TTY`): names
   only, never values. `SSH_AUTH_SOCK` is ignored, since sbx sets it everywhere.
3. **The working directory:** sbxw's agent starts in the recorded workspace.

Only the `tty` session can be typed into from the island. When the evidence is
missing, **neither** row is answerable, because answering the wrong terminal
answers someone else's question. The daemon enforces the same rule (`409`).

## How session state works

State comes from **Claude Code hooks**, not terminal scraping. Provisioning
installs `assets/status-hook.js` in each sandbox. It POSTs every lifecycle
event to the daemon's agent listener over `host.docker.internal`, with the
sandbox's token.

| Hook event | State |
|---|---|
| `SessionStart` | `idle` |
| `UserPromptSubmit` | `working` (captures your prompt) |
| `PreToolUse` (`AskUserQuestion`) | `attention`, with every question of the call |
| `PreToolUse` / `PostToolUse` (other) | `working` (the tool as activity) |
| `Notification` | `attention` (permission or idle nudge) |
| `Stop` | `idle` (with Claude's reply) |
| `SessionEnd` | `exited` |

Daemon endpoints the island uses:

| Endpoint | Purpose |
|---|---|
| `GET /api/sessions` | Snapshot of current sessions |
| `GET /api/events` | Server-Sent Events, one per hook-driven change |
| `GET /api/hook/log` | Recent raw hook events, for debugging |
| `GET /api/sandboxes` | Running sandboxes, shown as `idle` right away |
| `POST /api/answer`, `POST /api/input` | Send a menu choice, or raw bytes, into a session |
| `POST /api/chat/push` | `{ "text", "name"?, "fresh"? }`: send a message, creating the sandbox if needed. `fresh` takes the next free `ephemeral-chat[-N]` |

A sandbox whose hooks predate the agent listener gets a `410` naming the fix:
`sbxw up <name>`.

## Subscription usage

The island and the web UI header read the same `/api/usage`, so they always
agree. It's filled two ways (last writer wins):

- **From each sandbox, while an agent works.** sbxw installs a `statusLine`
  script (`assets/usage-statusline.js`). Claude Code passes it
  `rate_limits.{five_hour,seven_day}.used_percentage`, and the script forwards
  it, throttled.
- **From the daemon, every five minutes.** It runs
  `sbx exec <sandbox> -- curl https://api.anthropic.com/api/oauth/usage`. This
  costs no tokens, and fills the gauges before anyone has sent a message.

It goes **through a sandbox** because that request needs an OAuth token with
the `user:profile` scope, and the host often doesn't have one
(`claude setup-token` tokens are `user:inference` only; the keychain copy goes
stale). Requests leaving a sandbox are authenticated by sbx's proxy with full
scope. Every running sandbox is tried; failures back off up to 30 minutes.

Shown for Pro/Max only. API-key auth has no subscription windows, and the poll
doesn't run under `--use-api-key`.
