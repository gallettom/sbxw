# Relay: asking another sandbox

[← Back to README](../README.md)

- [Why](#why)
- [Questions between sandboxes](#questions-between-sandboxes)
- [Screenshot requests](#screenshot-requests)
- [Canvas reviews](#canvas-reviews)
- [Virtual Mac requests](#virtual-mac-requests)
- [What a sandbox cannot do](#what-a-sandbox-cannot-do)

## Why

Sandboxes are isolated on purpose, which is also their limit: the agent on the
frontend can't see the API repo next door. So it guesses, or stops and asks you
to go look. The relay is a third option: it asks the other sandbox, and **you
decide every hop**.

```
sandbox A ──ask──▶ sbxw ──▶ 🧑 popup ──route──▶ sandbox B
                                 ▲                  │
sandbox A ◀──approve── 🧑 review ◀──────answer──────┘
```

Nothing happens on its own. No timeout picks a recipient, and no answer reaches
the asker without someone clicking.

## Questions between sandboxes

### The agent's side

Every sandbox gets an MCP server, `sbxw-relay`, with these tools:
`ask_other_sandbox`, `ask_user_for_screenshot`, `ask_user_questions` and `check_sandbox_question`.
The same is available as a CLI:

```bash
node ~/.sbxw/relay.js ask  "what shape does GET /v1/orders return in staging?"
node ~/.sbxw/relay.js shot "the settings header at mobile width, after I moved the save button"
```

Agents ask **on their own**, without being told to. It's a tool and not only a
CLI because a tool is weighed every turn, while a note in memory competes with
the whole conversation. In practice, an agent with only the note reached for a
codebase search instead of asking.

The call waits about 90 seconds, then returns the answer, a refusal, or "still
open, check later with `check_sandbox_question`". An agent's tool call never
hangs on a human who stepped away. If the answer is approved after the call
returned, sbxw types it into the asking session (the thread, if a thread
asked).

### Your side

A popup shows the question and one button per running sandbox:

<img src="images/relay.png" alt="An information request from api: the question, buttons for myapp and shop, and a box to answer it yourself" width="600">

1. **Route it:** click a sandbox. A one-line notice is typed into that agent,
   and it fetches the question with `node ~/.sbxw/relay.js show <id>`, framed
   as untrusted data, with the id it must answer with. A sandbox brought up
   before this version has no `show` until you run `sbxw up` for it.
2. **Carry on:** the popup shrinks to a corner card while the other agent
   works. It comes back by itself when the answer lands.
3. **Review the answer:** it arrives **editable**. Trim it, cut a secret out,
   or rewrite it. Only then does the asker receive it.

Other choices:

- **Answer it yourself:** type in the box and send, no second sandbox needed.
- **Refuse:** a full stop. Nothing is released, and the agent is told not to
  ask again.
- **Later** (or Esc): parks it behind a header badge. The badge counts only what
  waits on *you*.

Routing to a sandbox whose agent can't start bounces the request back with the
reason.

## Screenshot requests

An agent can't see what it just built. The code compiles and the diff is right,
but "the code is correct" and "it looks correct" are different claims. So it can
ask to be shown:

```
sandbox A ──shot──▶ sbxw ──▶ 🧑 popup ──paste / drop / capture ×N──▶ sandbox A
```

There's no routing step: nobody else is looking at your screen.

<img src="images/screenshot-request.png" alt="A screenshot request from api: the view it wants, a drop zone, Paste from clipboard, Capture a window, Choose a file, and a box to describe it in words" width="600">

- **Add images four ways:** paste from the clipboard (⌘V works too), drop
  files, choose files, or **capture a window** through the browser's own picker.
  Clipboard and capture need the page on `localhost`, not a LAN address.
- **Several at once:** up to six images and 16 MB, numbered in the order the
  agent receives them. One interruption for a before/after or two breakpoints.
- **Preview first:** you see exactly what is sent. Images are scaled to 1600px.
- **Words work too:** describe it instead, and the agent works from the
  description.
- **Refuse** is an ordinary answer: the agent carries on, says what it changed
  and what it expects, and won't ask again for the same view.

Through MCP, the images come back **in the tool result**, so the agent actually
looks at them. They're also saved in the asking sandbox as
`~/.sbxw/shots/<request-id>-1.png`, `-2.png`… for later turns. The daemon never
writes into a sandbox: the image data travels in the approved reply and the
sandbox's own CLI writes the files.

The [browser side panel](chat.md#the-browser-side-panel) can answer a
screenshot request with a capture of the app's tab.

## Canvas reviews

A canvas is one review session of one sandbox. The agent adds screenshots
of the app it works on; you zoom and pan over them in the desktop app's
**Canvas** view, pin numbered remarks on points of a shot, place notes and
example pictures, then press **Send remarks**. The agent reads the round
with a tool and works on it. Everything stays on this computer's disk until
you delete it.

| Tool | What it does |
|---|---|
| `canvas_add_shots {canvas?, title?, shots: [{file, title, note?, width?, screen?}]}` | Reads each file inside the sandbox and posts it; returns the canvas id, how many were kept, and why any was refused. One call is one round. |
| `canvas_read {canvas?}` | The remarks of the review rounds not read yet: pins with their number, the shot's title, the position (percentages and a hint such as `top-left`) and the text; notes; each example picture's path inside the sandbox. Each round is handed over once. |
| `canvas_list` | The sandbox's canvases: id, title, shots, rounds, unread review rounds. |

An agent reaches only its own sandbox's canvases (its token says which
sandbox it is; a body field never does). It cannot delete a canvas, edit a
remark or read another sandbox's canvas.

**Layout.** Each round is a grid below the earlier ones (12 desktop shots
make 4 columns of 3 rows, 9 make 3 x 3, phones share a cell three by
three), its title under each shot. Give the same `screen` key to the same
screen in every round: it takes the same cell as before, so before and after
line up in one column; a new screen goes after the earlier ones. A shot the
person moved keeps its place. Canvases made before grids are laid out again
once, when the daemon reads them, unless the person had arranged them.
In `canvas_read`, a picture the person added is "Example N".

**Limits.** PNG, JPEG or WebP, checked by content, never by name (animated
pictures refused); 20 MB and 8192 px a side per picture; 24 shots a round;
500 MB of pictures a canvas; titles 120 characters, notes and remarks 2000.

**On disk.** `~/.sbxw/state/canvas/<id>/`: `canvas.json` and the pictures,
each under a name sbxw generates (`s-<12 hex>.png`). Nothing an agent or a
person types is used as a path. Folders 0700, files 0600, atomic writes.
The daemon's log has ids, counts and sizes, never a title, a remark or a
picture. Canvases are never deleted on their own, not even with their
sandbox: the Canvas view shows each one's size and the total, and deletes
one canvas or every canvas older than a date, each after one confirmation.

**Sending.** One confirmation lists the target session, every remark in
full and each example picture (local path, size, a box to tick for a file
that looks secret). On Send, sbxw copies the pictures into the sandbox
(`/tmp/sbxw-uploads/...`, as dropped files), records the round and types
one fixed line: `[sbxw canvas · c-…] review round 1 is ready: run
canvas_read to read the person's remarks`. A session started before this
version has no `canvas_read` until `sbxw up <sandbox>`.

## Virtual Mac requests

With the [`sbxw:terminal-mac`](../assets/terminal-mac/README.md) kit, a
sandbox may ask to **boot the virtual Mac** or to **publish a port** to it.
Each is a relay request of its own kind: never routed, a yes or a no. The
card says what will happen in sbxw's words, then quotes the agent's reason.
**Approve** runs it, and the request is settled with what came of it (the URL
to open, or the error), so the agent reads the outcome rather than "approved".

## Questions for you

`ask_user_questions` puts a set of questions to **you** (a `/grill-me`
round, or any batch of decisions that are yours) and blocks until you have
answered (10 minutes, then a request id for `check_sandbox_question`). Each
question carries a title, the question and the agent's recommendation.
`/grill-me` asks its rounds this way in sbxw.

The round opens this popup like any other request, on **one form**: who
asks (sandbox and thread), the agent's intro if it wrote one, then every
question in order with its id, its title, the question, the agent's
recommendation on its own line, and an answer field. **Use the
recommendation** fills a field with it. A question you leave empty is
skipped. **Send answers** (or Ctrl+Enter, ⌘+Enter on a Mac) releases them
all at once, one line per answered question (`Q1 : …`), which is what the
tool returns. Personal data is masked first, as the
[Jarvis](web-ui.md#jarvis) masks what it sends; if the
masking is not available or fails, nothing is sent. Tab moves from one
answer field to the next. The popup does not take the keyboard from a
terminal until you click into it.

You can also **route the round** to another sandbox, with the same buttons
as a question: its agent gets the round as untrusted text from another
agent and is asked to draft the answers in the `Q1 : …` format. Its draft
comes back to the popup, shown as it was written and filled into the form,
for you to edit, discard with **Clear the draft** and answer yourself, or send. Nothing reaches the
asker until you click **Send answers**. **Refuse** sets the round aside and
tells the agent so.

**Read aloud** (shown when Jarvis's voice is set up) reads the round
with Jarvis's voice: the intro, then each question with its
recommendation. Click again to stop. Nothing is read in full on its own;
with the voice on, a short spoken cue says a round has arrived.

## Team notes and the relay server's hello

A lead and its threads leave each other team notes (`team_note`,
`read_team_notes`, see [threads-and-lead.md](threads-and-lead.md)). sbxw
types one fixed line into the addressee's session saying a note waits, never
the note's words. A session reads a note only if its relay MCP server has
`read_team_notes`; one started before the relay had it does not, and the
line would leave the note unread.

So each relay server says hello: at start and every 2 minutes it posts
`/api/relay/hello` on the agent listener with `{thread, agent_proc, tools}`,
where `agent_proc` is `<pid>:<start>` of the Claude process it serves (found
as the status hook finds it) and `tools` the tool *names* it offers. The
sandbox is the token's, never the body's; the body is capped at 16 KB and 200
names. The daemon keeps, in memory only, whether each process offers
`read_team_notes`, and, from the status hook's events, which process each
session (`lead` or a thread) runs in. An addressee is a sure reader only when
its current process said hello with the tool within 6 minutes. Any other
(an old session never says hello, and after a daemon restart nobody has
until the next hello and hook event) gets the cautious line, which tells it
to ask the sender to resend by `SendMessage`, and the sender's answer says
to send it that way too. A hello is logged as counts, never its content; a
failed hello is silent.

Why this signal: the hook version says what is on disk, not what a running
session loaded; the daemon does not record when it wrote the relay into a
sandbox; a session's own relay server is the one thing that knows its tools.

## What a sandbox cannot do

- name its own recipient;
- list or read anyone else's requests;
- see an answer you haven't released;
- reply to a question it wasn't handed;
- claim to be another sandbox. Each sandbox presents the token sbxw installed
  (`~/.sbxw/token`), and the daemon derives the sender from it. A `from` field
  in the request is ignored.

Routing, approving and refusing exist only on the UI listener, which sandboxes
can't reach. Requests live in memory (`crates/core/src/relay.rs`): a settled
one is kept 30 minutes for a late pickup, an unanswered one 6 hours. A
request opened by a thread or lead of a plan is also noted on that team's
timeline (`docs/threads-and-lead.md`): who asked whom, the outcome, the
times, the image count and the texts (2000 characters at most), kept with the
plan; never an image.

The daemon never decodes images. It checks each one is a `data:` URL of PNG,
JPEG or WebP under the size cap, and the set is at most six and 16 MB.
