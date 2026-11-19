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
| `Notification`: `permission_prompt`, `worker_permission_prompt`, `elicitation_dialog`, `elicitation_url_dialog`, `agent_needs_input`, or no type | `attention` |
| `Notification`: `idle_prompt` | `idle` |
| `Notification`: any other type | unchanged |
| a subagent's `PreToolUse` / `PostToolUse` (`agent_id` set) | unchanged, except that it ends an `attention` with no question pending |
| `Stop` | `idle` (with Claude's reply) |
| `SessionEnd` | `exited` |

`attention` ("wants you") means the session cannot go on without the
person: a permission prompt, a question, an elicitation, a dialog. Claude
Code sends `Notification` for news too (a teammate or background agent
finished, an elicitation answered, a push the model sent, a usage limit
reset, a login), with the type saying which; those change nothing, and a
type the daemon does not know is read as news. A `Notification` with no
type comes from a Claude Code older than `notification_type`, or from one of
today's dialogs that names none ("Claude needs your permission", a plan to
approve), so it still reads as `attention`. The idle reminder (a minute
into an open prompt) is `idle`, never `attention`: it also settles a turn
cut short with Esc, which fires no `Stop`. Any evidence of work or of an
answer ends `attention`: a prompt (the person's or an injected message), a
tool call or result of the session, `Stop`, `SessionEnd`.

A session's last payload says `ended: true` with an `end_reason`, and a new
session says which one it `continues` when the daemon can tell (the same
claude process after `/clear`, or a resumed transcript). From hook v3, a
`SessionStart` also retires the records of the same sandbox whose claude
process is gone, so a killed terminal no longer leaves a row behind (and no
longer makes the restarted main session unanswerable). `last_input_from`
says whether the last prompt was the person's or one the harness injected
(a message from another session, a task notification). The whole
conversation, for the Monitor, is in [session-history.md](session-history.md).

### The model a session runs

Each session in `GET /api/sessions` (and each `session` event) carries
`model`, the model id (`claude-opus-5-5`), and `model_name`, its display name
(`Opus 5.5`). Each item of `GET /api/sandboxes` carries the same two fields
for its main agent session: the one without a thread that did not come in
over SSH or Claude Desktop. Both are optional and left out when unknown.

```json
{ "sandbox": "myapp", "session_id": "s1", "state": "idle", "agent": "claude",
  "origin": "tty", "model": "claude-opus-5-5", "model_name": "Opus 5.5", "...": "..." }
{ "name": "myapp", "agent": "claude", "status": "running",
  "model": "claude-opus-5-5", "model_name": "Opus 5.5", "...": "..." }
```

Where the value comes from, best first:

1. **The session itself.** Claude Code tells the `statusLine` script
   (`assets/usage-statusline.js`) its `model.id` and `model.display_name` at
   every render. The script sends them with the session's `session_id` to
   `POST /api/usage`, at once when they differ from what it last sent for that
   session (the first render, or a `/model` switch), otherwise with the
   throttled usage report. A hook event that names a model (`SessionStart`
   gives a `model` id) counts too; a bare id keeps the display name already
   known for it.
2. **What sbxw started it with**, until the session has said: the model a
   thread was opened on, else the sandbox's `claude_model`, else the daemon's,
   else the first entry of `models`. The display name is that model's `label`
   in `models`, absent for an id the list does not hold. A model the person
   saved with `/model` in an earlier session shows only once the status line
   reports it.
3. **Nothing**, for an agent other than Claude: no hook or status line of
   sbxw's runs there, so both fields are left out.

The payload does not say which of these a value came from.

The status line script is written by the tooling batch at each `sbxw up` (and
each bring-up from the UI), so an existing sandbox starts reporting its model
after its next bring-up. Until then it shows the fallback; an older script's
reports, which name no model, change nothing.

### What a session has cost

Each session also carries, once its status line has reported them:
`cost_usd` (dollars), `duration_ms` (its wall-clock time so far),
`lines_added` and `lines_removed`. Each `GET /api/sandboxes` item carries
`cost_usd`, the sum over that sandbox's sessions. All are optional and left
out until something is known; never `0` for unknown.

```json
{ "sandbox": "myapp", "session_id": "s1", "cost_usd": 1.37, "duration_ms": 842000,
  "lines_added": 120, "lines_removed": 14, "...": "..." }
{ "name": "myapp", "status": "running", "cost_usd": 4.12, "...": "..." }
```

**What the number is.** Claude Code's own estimate of what the session's
tokens would cost at API prices (the status line's `cost.total_cost_usd`). It
is not an invoice: under a Pro or Max subscription it is not what the person
pays, and under an API key the console's bill is the reference.

- **Where it comes from.** The status line script sends the session's `cost`
  object (`total_cost_usd`, `total_duration_ms`, `total_lines_added`,
  `total_lines_removed`, whichever are numbers) with its throttled report to
  `POST /api/usage`, at most every 10 s per sandbox. A change of cost does
  not bypass the throttle (a model change does).
- **Events.** A session's first cost goes out at once in a `session` event.
  After that a change of cost alone goes out at most once per 30 s per
  session; it goes out at once when the event is sent for another reason (a
  hook, a model change), and a change held back is sent by the daemon's 15 s
  tick once its 30 s are up. `GET /api/sessions` always has the latest.
- **The sandbox's total** counts every session of that sandbox the daemon has
  seen since it last saw the sandbox running: the main session, threads and
  sessions that came in over SSH, live ones and ones that ended (`SessionEnd`,
  a closed thread, `/clear`). An ended session stays in the total until the
  sandbox stops, so the number never goes backwards under the person's eyes.
  A session id whose cost goes down (Claude Code restarted under the same id
  without a `SessionEnd`) keeps its old run in the total too. The total
  starts again from nothing when the sandbox stops, and when the daemon
  restarts (it is kept in memory only).

An existing sandbox reports costs after one `sbxw up <name>` (the script is
rewritten at each bring-up); until then, its older script's reports change
nothing.

### CPU and memory

`GET /api/stats` (UI token, like every `/api` route) answers what the daemon
and each sandbox use; the same body goes out as a `stats` event on
`/api/stream` every 5 s while at least one `/api/stream` is open, and not at
all otherwise.

```json
{ "daemon_cpu_pct": 0.8,
  "sandboxes": [
    { "name": "myapp", "cpu_pct": 143.5, "cpu_age_s": 4, "cpu_cores": 4,
      "mem_bytes": 2147483648, "mem_total_bytes": 8589934592 },
    { "name": "cx", "cpu_pct": null, "cpu_age_s": null, "cpu_cores": null,
      "mem_bytes": null, "mem_total_bytes": null } ],
  "total_cpu_pct": 144.3, "sandboxes_counted": 1, "sandboxes_unknown": 1,
  "ts": 1763380000000 }
```

- **Percent of one core**, so it exceeds 100: four busy cores read 400.
- **Unknown is `null`, never `0`.** `total_cpu_pct` is the daemon plus every
  sandbox with a `cpu_pct`, `null` only when none is known; it is partial
  when `sandboxes_unknown > 0` or `daemon_cpu_pct` is `null`, and a bar
  should say so.
- **`sandboxes`** lists every sandbox the daemon's 15 s `sbx ls` last saw
  running, plus any that reported since; a stopped one drops out.

**Where a sandbox's numbers come from.** sbx exposes no CPU figure, and a
sandbox is a microVM the host's Docker does not list. So each sandbox
measures itself: the status line script reads the first `cpu` line of the
VM's `/proc/stat`, its number of `cpuN` lines and `MemTotal` /
`MemAvailable` from `/proc/meminfo`, and sends them raw with its throttled
report (`proc_stat_cpu`, `cpu_cores`, `proc_meminfo`). The daemon turns two
successive reports of the same sandbox into `cpu_pct`; `mem_bytes` is total
minus available. Nothing is polled and no sandbox is woken for it. What
follows from that:

- It covers the whole microVM (the agent, its tools, a build, Docker inside
  it), not one process.
- A sandbox reports only while a Claude session renders its status line:
  about every 10 s while an agent works. Claude Code renders it when the
  conversation changes, so a long single tool call (a two-minute build)
  sends nothing until it ends. A sandbox with no Claude session (another
  agent, a plain shell) never has a value.
- `cpu_pct` is `null` after the first report (one sample is no rate), when
  the last sample is older than 30 s, and when two samples are more than
  60 s apart. `cpu_age_s` is the age of the last sample (`null` if none), so
  a client can say "no recent report" rather than show a frozen number.

**The daemon's own CPU** is read from the OS when asked (the `stats` tick or
a request, at most once a second): `/proc/self/schedstat` on Linux, `ps -o
cputime= -p <pid>` on macOS with a 2 s timeout. A failed or late reading
makes it `null`; the first reading after the daemon starts is `null` too
(it takes two).

An existing sandbox sends these after one `sbxw up <name>`, like the cost.

Every request the island makes carries the daemon's UI token
(`Authorization: Bearer`, read from `~/.sbxw/run/ui-token` before each
request, since the daemon writes a new one each time it starts). Opening a
sandbox in the browser when no tab shows sbxw uses the sign-in address
(`/?t=<token>#sandbox=<name>`), so the new tab is signed in. See
[Security: two listeners](security.md#two-listeners).

Daemon endpoints the island uses:

| Endpoint | Purpose |
|---|---|
| `GET /api/sessions` | Snapshot of current sessions |
| `GET /api/events` | Server-Sent Events, one per hook-driven change |
| `GET /api/hook/log` | Recent raw hook events, for debugging |
| `GET /api/sandboxes` | Running sandboxes, shown as `idle` right away |
| `GET /api/stats` | CPU and memory, the daemon's and each sandbox's ([above](#cpu-and-memory)) |
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
