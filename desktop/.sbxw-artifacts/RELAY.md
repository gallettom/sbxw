# The relay in the desktop client (phase 4, thread desktop-relay)

Branch `sbxw/team-p-6bfb2cd194f22ac57c7c28a2292f6ee9`, shared with
desktop-integ and desktop-threads. Built and tested on Linux (aarch64, no
GPU, headless Rune renders, no sound card). **Nothing was run on a Mac or
against the real daemon.** Shots in `desktop/.sbxw-artifacts/relay/`. The
screen contract is the `### relay` section of `desktop/docs/screens.md`.

## What was built

| Part | Where | What |
|---|---|---|
| Feature | `src/feat/relay/mod.rs` | `relay` in `features!`. Data `relay` (`GET /api/relay`, again on each `relay` event); `data.sessions` (desktop-threads' key) for `AskUserQuestion`; the `session` event for notices. Intents `relay_*`, `agent_pick`, `follow_*` (table in screens.md). What the person types, attaches and picks lives in the core (`State`), shown as `data.relay_ui`; pictures never reach Luau |
| Requests panel | `screens/relay.luau` | Panel "Requests", opened by itself when a request arrives. The waiting list, the request shown, those "with another sandbox", agents asking. Each kind and state of relay.js: a question (route to a running sandbox, or answer it yourself; routed: waiting, re-route; answered: the answer in an editable field, released), a screenshot request (previews, drop zone, Paste from clipboard, words instead), a round (one form, every question with its recommendation, "Use it", an answer field each; route for a draft; a draft fills the form, "Clear the draft"; Answer by voice; Read aloud), a virtual Mac request (Approve or Refuse), a settled one (outcome, Close). Agent text is drawn with `text` or `markdown`, never run |
| Approvals | `mod.rs` (`route`, `send`, `build_ask`, `follow_send`, `build_follow`, `follow_choose`) | Routing and every release are intents of risk Approval: the core's dialog shows the text that leaves (the question routed, the answer, the round's answers after masking, a caption) and where it goes; for pictures the count, each one's source, size and weight, the total. A round's answers and words for a followed sandbox go through `Prepare`: masked by `POST /api/voice/anonymise` first, the dialog shows the masked text; a masking that fails sends nothing. Refusing is direct. A request id the daemon did not list, a stopped or unknown target, the asker as target, a screenshot or Mac request routed: refused |
| Rounds | `src/feat/relay/round.rs` | `chatParseRound` of chat-core.js (intro, `❓ **Qn - Title** :`, `➡️` recommendation, `---`), text with no marker is one question; a draft's `Qn : …` lines fill the form (`relayAskDraft`); answers go back one line each (`relayAskAnswer`); what is read aloud |
| Pictures | `src/feat/relay/shots.rs` | Through the core's `picture::prepare` (limits before decoding, no animation): longest edge 1600 px, PNG, else JPEG 85 when the PNG passes 6 MB of base64; six pictures, 16 MB together. Each gets a preview (`Action::Image`, drawn with `image{}`), forgotten when sent, refused or removed. Files are read only from the core's `on_files` paths (drop, file dialog), size-checked first; no intent takes a path or image data (tested) |
| AskUserQuestion | `mod.rs` (`asking`, `pick`), the panel | Every sbxw-driven session waiting with steps (not Jarvis's own, which its panel asks): one row of options per step, sent at once with `POST /api/answer {sandbox, mode: claude or thread:<t>, index, indices}` once each step has a pick; "Answer in its terminal" when not answerable; a notice when one starts waiting |
| Following another sandbox | `src/feat/relay/follow.rs`, `src/shell/jarvis.rs`, `src/jarvis.rs`, `screens/jarvis.luau` | The Jarvis panel's title becomes a picker (Jarvis, or a running sandbox, kept across runs with `Action::Remember`); with a sandbox picked, a Follow box. Words typed or spoken then go to `follow_send` (a draft in the core's dialog) or, when the target waits on a one-step choice, `follow_choose` (the dialog names the option, `vcPickOption`); they never reach Jarvis's yes/no, so another sandbox's turn can never approve a proposal. The target's turns are ingested as chat-core.js does (`chatIngest`): shown once, as `who = "agent"` with `from`, **only** when Follow is on, the prompt was `/grill-me`, or the reply is a round (`vcEngages`); read aloud through the Jarvis reader (never while the microphone is open; talk stops it for good). A round in a turn goes to the Requests panel's form (answers typed into that session, `/api/chat/push`) |
| Voice for rounds | `relay_voice`, `relay_heard`, `relay_read`, `src/shell/jarvis.rs` (`hear`) | Answer by voice: each question read, the next words spoken (push-to-talk) fill it, "repeat", "skip", "back" (concierge.js `VC_REPEAT`, `VC_SKIP`, `VC_BACK`); Read aloud reads the round; a short cue when a round arrives. What is read is first a line of the Jarvis log |
| Stand-in | `src/feat/relay/standin.rs`, `fixtures/relay/standin.json` | `GET /api/relay`, route, approve, deny with the daemon's rules; a routed request answered by a scripted agent 0.9 s later (a round: its recommendations as the draft); `POST /api/answer`; `POST /api/chat/push` to a sandbox other than Jarvis: a turn and a reply (a round when the words say "grill") |
| Fixtures | `fixtures/relay/*.json` | `basic`, `answered`, `ask`, `ask-draft`, `ask-routed`, `screenshot`, `mac`, `bounced`, `errors` (the read fails: the daemon's words in red), `unanswerable`, `empty`, `standin` |
| Demo | `scripts/demo-relay.sh` | Below |

Core pieces asked of desktop-integ and used here: `Feature::on_files` with
`Incoming`, `Action::ReadClipboard`, `Action::PickFiles`, `Action::Image` and
the `image` element, the `textarea` element, `picture.rs`, `Action::Remember`,
`data.sessions` in the stand-in, `/api/chat/push` for other sandboxes handed
to features in the stand-in.

## Web features not ported, and why

| Web UI | Here | Why |
|---|---|---|
| Capture a window (`getDisplayMedia`) | Not offered | The app has no screen capture. On macOS, ⌘⇧4 then Ctrl (to the clipboard) and Paste does the same |
| Choose a file… | Not offered | Rune's native file dialog needs a feature that adds packages (lead's decision: not in this plan). `relay_shot_choose` exists and answers with the core's notice. Drop and paste work |
| Paste with ⌘V on Linux and Windows | The "Paste from clipboard" button | Off macOS the root takes no paste (Ctrl+V stays the terminal's) |
| The popup over everything, Escape = later, corner cards for routed requests | A docked panel; "Later"; a "With another sandbox" list | The app's slots are panels; nothing steals the terminal's keyboard |
| `chat-dock.js` (the ephemeral-chat dock, its tabs, "New chat") | Not ported | Chat sandboxes open in a pane (create); their `AskUserQuestion` is answered from the Requests panel. Its own transcript store and tabs are a feature of their own |
| The grill one question at a time in the concierge (`vcGrill`) | The one form, with Answer by voice | One form for every round, as the plan asks; the same voice commands |
| "No sandbox" as a target, the draft card that grows as you talk | Jarvis is the default target; the core's dialog | Every message to another sandbox is a draft confirmed in the core's dialog (plan rule), not sent at once as `vcDraft` does; Cancel and say it again to change it |
| A spoken "oui" sending a draft | A click on the dialog | The core's dialog is answered by a click only |
| Seeding the Jarvis log from `data.sessions` | Unchanged (its own read at connection) | Kept as it was to leave the Jarvis rules and tests untouched |

## Evidence

Tests: `cd desktop && CARGO_PROFILE_DEV_DEBUG=0 cargo test -j1`: 217 passed,
3 ignored (others' benchmarks), plus the integration tests, all green at
commit `6b605d5`. Mine (23):

- `feat::relay::tests`: routing and releasing are approvals, refusing is
  direct, no intent takes a path or image data; routing shows the question
  and the target and refuses the asker, a stopped or unknown sandbox, an
  unknown request, a screenshot; the answer sent is the one typed or edited
  (and who wrote it); a round's answers are masked before the dialog shows
  them ("Jeanne Martin" becomes "Personne A" in the dialog and the body),
  a failed masking sends nothing; a draft fills the form, Clear empties it;
  a path from a screen is refused, pictures from the core are counted in
  the dialog and never reach the screen; an agent's choice goes once every
  step is picked (`thread:auth` mode), Jarvis's own question is refused; a
  new request is a notice that opens the panel, once; the stand-in keeps the
  daemon's rules.
- `round`, `shots`, `follow`: the parser on a grill round and edge cases,
  drafts, answers, words read; pictures scaled to 1600 px by the core's
  helper, the set's limits, base64; a followed turn shown once and only when
  engaged, a grill engaging with Follow off, options picked by words.
- `shell::jarvis`: with another sandbox picked, a typed "yes" is a draft in
  the core's dialog and does not approve Jarvis's armed low-risk proposal,
  nothing is pushed; back to Jarvis the yes answers it. A followed agent's
  turn is shown once, read once, never while recording; a round goes to the
  panel.

Solo shots (`screen-*.png`, 420x900, each looked at): `basic` (three waiting,
the question shown with api, docs, shop to route to, one with another
sandbox, shop and api·auth asking, two steps); `answered` (api's answer in an
editable eight-line field, Send to web-app lit); `ask` (intro, Q1 and Q2 with
their recommendations, two-line answer fields, Use it, sandboxes to draft);
`ask-draft` (api's draft quoted, the form filled with 8081 and "Keep the
cookie", Clear the draft); `ask-routed` (the round under "With another
sandbox"); `screenshot` (drop zone, Paste from clipboard, words field);
`mac` (the agent's reason quoted, Approve…); `bounced` (the daemon's reason
in amber above the question); `errors` (the read error in red, shop's
question still there); `unanswerable` ("Answer in its terminal"); `empty`.

Whole window against the stand-in (`window-*.png`, from
`scripts/demo-relay.sh`, each its own run, each looked at):

- `window-panel`: the Requests panel opened by itself, "3 requests from
  agents wait for you", "Requests 4" in the top bar, `?` badges on web-app
  (3) and shop (1).
- `window-route-confirm`: the core's dialog "Send web-app's question to
  api?", the question in a box, "nothing reaches web-app until you release
  it", Send to api.
- `window-answered`, `window-send-confirm`: api's scripted answer in the
  field; the dialog "Send this answer to web-app?", written by api, the
  character count, the text.
- `window-shot-typed`, `window-shot-confirm`: two files dropped on the panel
  (1280x800, and 3000x1800 shown as 1600x960) and a pasted 375x812 picture,
  three numbered previews, words typed; the dialog "Send 3 images and your
  words to web-app?" lists 1. before.png · 1280×800 · 17 KB, 2. after.png ·
  1600×960 · 28 KB, 3. from clipboard · 375×812 · 8 KB, the total, where the
  agent keeps them, the words.
- `window-round-confirm`: "Send 2 answers to web-app · checkout?", "1 piece
  of personal data masked before sending", `Q2 : Keep the cookie, ask
  Personne A about the mobile app`.
- `window-picked`: "Answered shop: One column", shop's question gone.
- `window-follow-draft`, `window-follow-reply`, `window-follow-round`: the
  Jarvis panel with web-app picked and Follow ticked; the words in the core's
  dialog "Send this to web-app?" (a draft, Cancel keeps nothing); after
  Confirm, web-app's scripted reply in the log under a `web-app` badge;
  "grill me on the migration" sent, web-app's round on the Requests panel's
  form (Set aside instead of Refuse, no routing) and one line in the log.

With a sandbox picked, the Jarvis panel shows that sandbox's state, not
Jarvis's (fixture `fixtures/jarvis/follow.json`): `jarvis-follow-before.png`
(Jarvis picked: "absent", Start Jarvis, "1 to review"); `jarvis-follow-picked.png`
(web-app picked: an amber "wants you" badge on a line under the title with
Follow and "rounds only", no Jarvis status, no Start Jarvis, "1 to review"
still there); `window-follow-fallback.png` (docs picked, then stopped from the
sidebar: the panel is back on Jarvis, its log and a notice say "docs is
stopped: your words go to Jarvis again. Nothing was sent."). Test
`a_followed_sandbox_that_stops_falls_back_to_jarvis` (stopped, then gone).

Logs (`shots/relay/window-*.log`): every routing and release reads
`relay: confirmed '…'` before its POST; no `/api/relay/*/approve` and no
`/api/chat/push` without one.

Jarvis demos rerun after the change, both pass and look as before:
`scripts/demo-jarvis.sh` (answered, low risk approved by "yes", high risk
told to click then clicked, the screen change applied: "MY SANDBOXES (by
Jarvis)") and `scripts/demo-jarvis-first-run.sh` (refused send, start and
send, the options question, "2", the answer). The panel's title now reads
"✦ Jarvis ▾" (the picker) when sandboxes run.

## Not verified

- **The real daemon.** Shapes read from `crates/core/src/relay.rs`,
  `crates/daemon/src/svc/relay.rs`, `http/ui.rs` (`/api/relay`, route,
  approve, deny, `/api/answer`, `/api/chat/push`), `crates/core/src/agents.rs`
  (`SessionInfo`). Not run against `sbxw up`: delivery into a real agent,
  `relay.js show`, a real `ask_user_questions` round, the 409/404 of
  `/api/answer`, a real draft's format.
- **macOS.** Not built here: ⌘V paste of a picture into the panel, a drop
  from Finder, the picker and checkbox in the Jarvis header, the core's
  dialog over a live terminal.
- **Voice.** No microphone or speaker here: Answer by voice, Read aloud, the
  cue, following with Follow read aloud, `follow_choose` by voice were run
  with typed words and the fake audio only.
- **Pictures from a real clipboard** (macOS NSPasteboard to PNG) and large
  Retina screenshots (decode time on the UI thread).
- The native file dialog does not exist in this build.

## macOS test procedure

Prerequisites: this branch on the Mac; `sbxw up` running; two running
sandboxes with a workspace (called `web-app` and `api` below), each with the
relay kit (any sandbox brought up by this sbxw); the voice installed (Setup,
Voice) for steps 15 to 18; the app started with `cd desktop && cargo run
--release`. The web UI open beside it (`sbxw web`) helps to compare.

1. In web-app's agent, ask: "use ask_other_sandbox to ask which env var holds
   the API base URL". Within seconds: a notice "web-app asks: …", the
   Requests panel opens on the right, "Requests 1" in the top bar, a `?`
   badge on web-app's row and "asks you" on its pane bar.
2. Press **api** under "Ask one of these sandboxes". The app's own dialog
   names web-app and api and shows the question. Cancel: nothing happens
   (the web UI still shows it pending). Press api again, confirm: the
   request moves under "With another sandbox"; api's terminal shows the
   one-line notice.
3. When api answers, a notice "api answered web-app's question: review it";
   the answer is in an editable field. Edit a word, press **Send to
   web-app…**: the dialog says "as you edited it" and shows your text.
   Confirm: web-app's agent receives exactly that text.
4. Ask another question from web-app; type an answer yourself under "Or
   answer it yourself" (Return makes a new line); Send my answer…, confirm.
5. Ask a third; press **Refuse**: the agent is told; no dialog.
6. In web-app, ask: "use ask_user_for_screenshot to see the settings
   page". Take a screenshot to the clipboard (⌘⇧4, Ctrl while selecting),
   click the panel (not a terminal) and press ⌘V: a preview appears,
   numbered 1. Drag a PNG from Finder onto the panel: preview 2. Press
   **Paste from clipboard**: preview 3. Remove one with ✕.
7. Press **Send to web-app…**: the dialog lists each picture with its size
   and weight and the total. Confirm: the agent's tool result has the
   images; `~/.sbxw/shots/<id>-1.png` exists in web-app.
8. Drop a 10 MB PDF on the panel: a red notice, nothing attached. Attach
   six pictures: the seventh is refused.
9. In web-app run `/grill-me` on any plan. The round opens on one form:
   intro, every question with its recommendation. Press **Use it** on Q1,
   type Q2 with a name in it ("ask Jeanne Martin"), leave Q3 empty. Send
   answers… : the dialog shows `Q1 : …` and `Q2 : …` with the name masked,
   no Q3. Confirm: the agent gets those lines.
10. Start another round; press **api** to have it drafted. api's draft comes
    back quoted and fills the form; Clear the draft empties it; Send
    answers…, confirm.
11. Make an agent ask a multiple-choice question (AskUserQuestion), for
    example "ask me with AskUserQuestion which colour to use": a notice;
    the Requests panel shows its options under "Agents asking". Press one:
    the session goes on; the terminal shows the choice taken.
12. Open Jarvis (⌘J). The title is a picker: choose **web-app**. A Follow
    box appears; leave it off. Type "list the files you changed" and press
    Return: the app's dialog "Send this to web-app?" shows the text. Cancel:
    nothing reaches web-app. Again, Confirm: web-app's agent receives it.
    Its answer does **not** show in the Jarvis panel (Follow off).
13. Tick Follow and send another message: web-app's answer shows once in
    the Jarvis log under a `web-app` badge.
14. With web-app picked, have Jarvis propose something low risk first (pick
    Jarvis, "use the english voice", then pick web-app again) and type
    "yes": the dialog for web-app opens; Jarvis's proposal is **not**
    approved (its review button stays).
15. Voice: with Follow on, hold F8 and say "run the tests": the dialog for
    web-app shows your words. Web-app's reply is read aloud. Press F8 while
    it reads: the reading stops and does not come back.
16. While recording (F8 held), let web-app answer: the answer is shown, not
    read.
17. A round in the Requests panel: press **Answer by voice**: Q1 is read.
    Hold F8, say an answer: it fills Q1 and Q2 is read. Say "skip", then
    "back", then "repeat". Press **Read aloud**: the whole round is read.
18. Make web-app ask an AskUserQuestion with Follow on: its options are
    read; say "the second": the dialog names option 2; confirm.
19. Quit and restart the app: the Jarvis panel's picker is still on
    web-app, Follow as left; requests still pending come back in the panel.
20. Stop the daemon with the panel open: the panel keeps its last view;
    start it again: the requests are read again.

If a step fails, report the step number, a screenshot and the app's log
(`RUST_LOG=info cargo run --release 2>&1 | tee app.log`).
