# Session history

What a session said and did, in order, for a UI that shows the conversation
as the terminal does: the person's prompts, the agent's replies, the steps
(tool calls and their results) between them, the pending question at the
end. The daemon holds it; the desktop app's Sessions view draws it.

Code: `crates/core/src/history.rs` (kinds, bounds, masking, classification),
`crates/daemon/src/svc/history.rs`, the routes in `http/ui.rs` and
`http/agent.rs`, the reader in `assets/status-hook.js` (between `BEGIN
history` and `END history`), tests in `crates/core/src/history/tests.rs`,
`http/tests.rs` and `tests/js/status-hook-history.test.mjs`.

## Where it comes from

Two sources, one log per session (sandbox, `session_id`):

- **Hook events.** Claude Code's hooks (`SessionStart`, `SessionEnd`,
  `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Notification`, `Stop`)
  already reach the daemon. They give what is live: a question
  (`AskUserQuestion`), a permission prompt (with the tool it is about, from
  the `PreToolUse` before it), the start and end, and the final reply
  (`Stop`'s `last_assistant_message`). With a hook older than v3 they also
  give everything else the log has: prompts, tool calls and results, the
  final reply. Such a log is `partial`: no assistant text between tool
  calls, nothing from before the daemon saw the session.
- **The transcript's tail** (hook v3). Claude Code hands every hook the path
  of its session's transcript (a JSONL file in the sandbox). The hook reads
  **only that file**, and only when its name is `<session_id>.jsonl`, from
  the byte offset it reached last time, at most 256 KB per event, complete
  lines only. It posts what it found, **summarised** (see below), to `POST
  /api/hook/history`, in parallel with the event itself. It keeps its
  offset in `/tmp/sbxw-history/<session_id>.json` (offsets only, never
  text, mode 0600). A first read starts at most 2 MB from the end, on a
  line boundary, and says `reset`. A line longer than a read (a huge tool
  result) is reported from its head as a result with no head, then skipped.
  Unknown entry types and blocks are skipped; any error means nothing is
  posted this time, and the hook still exits 0.

The daemon checks that offsets line up: a post must start where the last
one ended (`from` = the daemon's `next`), else it answers `409 {next}` and
the hook continues from there. After a daemon restart the daemon knows
nothing (`409 {next: 0}`), and the hook backfills from the tail with
`reset` at the next events. A transcript that shrank or was replaced is read
again from its tail, with `reset`.

`Stop`'s reply arrives before the transcript has it: it is kept as a
`provisional` item, removed when the transcript's next post arrives (which
carries the same text as an `assistant_text`).

## Items

Each item: `seq` (per session, increasing, never reused), `at` (ms since the
epoch: the transcript's timestamp, else arrival), `kind`, and what the kind
carries. Absent fields are left out.

| `kind` | Carries | From |
|---|---|---|
| `user_prompt` | `text`: the person's words; a slash command or `!` command as typed (`/model opus`, `! ls`) | transcript; `UserPromptSubmit` (old hook) |
| `assistant_text` | `text`; `provisional: true` for `Stop`'s early copy | transcript; `Stop` |
| `tool_call` | `tool {name, summary, id}` | transcript; `PreToolUse` (old hook) |
| `tool_result` | `result {status: ok\|error, size, head, id}` | transcript; `PostToolUse` (old hook) |
| `question` | `text` (first question), `steps` (every question with `options` and `context`) | `PreToolUse AskUserQuestion` |
| `permission` | `text` (Claude Code's message), `tool` (the call it is about) | `Notification` `permission_prompt` |
| `notice` | `text`: a start or end, an interruption, "An image was here (not kept)", an API error | both |
| `compact_marker` | `text`: the compaction summary, when the transcript has it | transcript (`compact_boundary`, `isCompactSummary`); `SessionStart source=compact` (old hook) |
| `injected` | `text`, `from` | transcript; `UserPromptSubmit` (old hook) |

`clipped: true` says the text was cut at its cap.

**`injected`** is a user turn the harness wrote, never the person: a message
from another session (`<cross-session-message from-name=…>`: `from` is the
sender's name), a task notification (`from: task`), a system reminder
(`system`, also when folded into a prompt: it is split off), a local
command's output or the caveat before it (`command`), a `!` command's output
(`shell`), a hook's output (`hook`), a skill's or command's expansion
(`isMeta`: `meta`), or any other tag-shaped wrapper around the whole text
(`from` is the tag's name). The same classification sets `last_input_from`
on the session.

**Tool summaries**, one line, at most 160 characters: Bash `$ <command>`
(first line, `…` when there are more); Read the path and the line range;
Edit the path and `-old +new lines`; Write the path and its line count;
Grep the pattern and path; Glob the pattern; Task/Agent the type and
description; AskUserQuestion the first question; MCP tools `MCP <server>:
<tool>` (no arguments); any other tool its name only. The hook sends only
the fields a summary needs (an edit's line counts, never its strings).

**Never stored:** thinking blocks, images and other binary content (a
`notice` says one was there), a tool result past its 2000-character head,
system prompts, subagent (sidechain) entries.

## Bounds

All in `sbxw_core::history::limits`, with tests:

| Bound | Value |
|---|---|
| Prompt, assistant text, injected text, compaction summary | 16 KB (cut on a character boundary, `clipped`) |
| Tool result head | 2000 characters |
| Tool summary | 160 characters |
| `from` | 80 characters |
| Items per session | 400 (older dropped, `truncated_before`) |
| All sessions | 64 MB; past it, whole logs go: ended ones first (longest ended), then the longest idle; the log being written last, and then only its oldest items |
| After a session ended | kept 1 hour |
| Its sandbox removed | dropped at once |
| A page | 100 by default, 200 at most |
| A hook post | 512 KB of body, 400 items (`413` past either) |
| Hook side | 256 KB read per event, 2 MB backfill, oversized lines skipped up to 8 MB, posts kept under 400 KB and 380 items |
| Subscription | 120 s after the last read |

Memory only: the daemon never writes a history to disk, and logs counts and
sizes at most (`tracing::debug!`), never a text.

## Masking

Every text field (`text`, the tool `summary`, the result `head`, `from`,
question steps) is masked **on ingest, before it is stored**, with the same
rules as dictation and the desktop Sessions view's sends (`sbxw_core::anonymise`): the
names the person listed in `anonymise.toml` become their aliases, and email
addresses, phone numbers, IBANs and key-shaped strings become `[email]`,
`[phone]`, `[iban]`, `[secret]`. A file that does not parse falls back to
the shapes alone, never to nothing.

What masking does **not** catch:

- short or plain secrets: a password like `hunter2`, a key under 24
  characters without a known prefix, an all-lowercase or all-hex token
  (a hex key looks like a commit hash and is left alone);
- secrets with characters outside letters, digits, `_` and `-` (base64
  with `+`, `/`, `=`; a URL's credentials `user:pass@`);
- a private key block (`-----BEGIN … KEY-----`): its lines are base64;
- names nobody listed, and any personal data that is not one of the shapes;
- the tool summary of an MCP call holds no arguments, but a Bash command
  may hold a secret passed inline (masked only if shaped);
- what reaches the daemon elsewhere: `GET /api/hook/log` keeps the last 100
  raw hook events, unmasked (prompt, tool input, `Stop`'s reply), on the UI
  listener; `reply` and `last_input` on sessions are not masked either.

A result head is still a file's or a command's output: masking reduces, but
does not remove, what a person did not ask to see.

## Routes and event

UI listener only, with the UI token like every other route:

```
GET /api/sessions/:sandbox/:session_id/history?before=<seq>&limit=<n>
```

The newest page first: up to `limit` items older than `before` (all when
absent), oldest first within the page. Answer (content invented):

```json
{
  "ok": true,
  "sandbox": "shop",
  "session_id": "0f9e8d7c-0000-0000-0000-000000000001",
  "page": {
    "items": [
      {"seq": 41, "at": 1763546400000, "kind": "user_prompt", "text": "Fix the basket total"},
      {"seq": 42, "at": 1763546401000, "kind": "assistant_text", "text": "Reading the basket first."},
      {"seq": 43, "at": 1763546402000, "kind": "tool_call", "tool": {"name": "Read", "summary": "Read /w/src/basket.rs (lines 10-50)", "id": "toolu_01"}},
      {"seq": 44, "at": 1763546402500, "kind": "tool_result", "result": {"status": "ok", "size": 1200, "head": "fn total() …", "id": "toolu_01"}},
      {"seq": 45, "at": 1763546403000, "kind": "injected", "text": "Please rebase on main", "from": "lead"},
      {"seq": 46, "at": 1763546404000, "kind": "question", "text": "Round half up?", "steps": [{"text": "Round half up?", "options": ["Yes", "No"]}]}
    ],
    "next_before": 41,
    "latest_seq": 46,
    "truncated_before": false,
    "partial": false,
    "ended": false
  },
  "limits": {"page_default": 100, "page_max": 200, "ring": 400, "text": 16384, "result_head": 2000, "subscription_ms": 120000}
}
```

`404` when the daemon holds nothing for that session. `partial: true` comes
with `partial_reason`: `old_hook: …` (the sandbox's hook is older than v3;
`sbxw up <sandbox>` installs it), `no_transcript: …` (a v3 hook could not
read the transcript: events only), `catching_up: …` (a v3 hook has not
posted yet, for instance right after a daemon restart).

```
DELETE /api/sessions/:sandbox/:session_id/history/subscription
```

Stops the nudges for that session.

**The nudge.** `/api/stream` carries `history` with `{"sandbox", "session_id",
"seq"}` (no text) when that session's log grew, **only while a UI is
subscribed** to it. Reading a page subscribes for 120 s, and every read
renews it: a UI that re-reads on each nudge and on each `session` event of
the session it shows stays subscribed. Chosen over a stream parameter
because the desktop keeps one `/api/stream` open for everything and changes
its selected session often: a parameter would mean reconnecting the stream.
The subscription is the daemon's (any UI reading it makes the nudge go out
on the bus), so two windows showing different sessions both get theirs.

Agent listener: `POST /api/hook/history`, the hook's post, accepted for the
sandbox the token proves (a `sandbox` field in the body is ignored), body
`{session_id, from, to, reset, items, prior_session?}`; `200 {next}`,
`409 {next}`, `413`, `400` (no session id).

## Sessions: ended, continues, who wrote the last input

Additive fields on `SessionInfo` (`GET /api/sessions`, `session` events):

- `ended: true` and `end_reason`, only on a session's last payload:
  `session_end:<Claude Code's reason>` (`clear`, `logout`,
  `prompt_input_exit`, `other`), `replaced` (the same agent process started
  another session), `process_gone` (its process no longer runs),
  `sandbox_stopped`, `thread_closed`.
- `continues`: the session this one follows: the one the same agent process
  ran before (`/clear`, `/resume` inside the running claude), or the one its
  transcript names (a `claude --resume` in a new process, when the
  transcript's first entries carry the old `sessionId`: format-dependent).
- `last_input_from`: `person` or `injected`; `last_input_sender` for an
  injected one when known.

How records are retired: a v3 hook reports `agent_proc` (`<pid>:<start
time>` of its nearest non-shell ancestor, the claude process) on every
event, and on `SessionStart` `live_procs`: the same key for every process of
the agent's name alive in the sandbox, from `/proc`. A v4 hook lists every
Claude-like process (`claude`, and the versioned binary `2.1.328` that
Claude Code's background sessions run as) and says so with `live_scope:
"claude"`. On a `SessionStart`, the daemon retires, **in the same sandbox
only**, the records whose process is known (reported by a v3 hook) and is
either this one (`replaced`) or not in `live_procs` (`process_gone`), and
the second **only when the list covers that record's process name**: the
record's agent name is the name the list was made for, or, from a v4 hook,
any Claude-like name. A background session's start (its process named
`2.1.328`, its v3 list holding only that name) therefore never retires the
terminals' `claude` sessions; before this rule it retired all of them. A
record whose process or name is not known is never retired; when `/proc`
cannot be read there is no list and nothing is retired; no time-out. A
second live terminal on the same checkout keeps its row. Retiring a dead
`tty` record is what makes a restarted main session answerable again:
nothing becomes answerable otherwise.

Claude Code's background sessions (`CLAUDE_CODE_SESSION_KIND=bg`, forwarded
by hook v4 as `session_kind`; from a v3 hook, an agent process named like a
version) are listed with `kind: "background"`. They have no terminal: never
answerable, not counted among the main checkout's sessions, and not the
sandbox's model. Their conversation is read like any other.
