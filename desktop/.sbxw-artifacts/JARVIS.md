# Jarvis in the desktop client (phase 2)

Thread `desktop-jarvis`, branch `sbxw/desktop-jarvis`, with
`sbxw/desktop-audio` merged once (as the lead allowed). Only `desktop/` was
touched. Linux (Debian trixie, aarch64) in an sbx sandbox, no GPU, no
display, no sound card. No daemon change. Shots in
`desktop/.sbxw-artifacts/jarvis/`.

## User decisions (recorded as given)

- Jarvis is a panel **docked on the right**, opened and closed by a top-bar
  button and a key; it narrows the terminal panes; its width is resizable
  and remembered.
- The panel stays **light**: a plain conversation (the person's words in a
  small bubble on the right, Jarvis's answers as plain Markdown, no card
  frame, no nested scrollbar), one input row with a compact talk button, no
  status lines repeating the conversation, errors shown and the typed text
  kept on failure.
- Approval rule (daemon-enforced, mirrored): a **low-risk** proposal can be
  approved by a spoken or typed yes; anything else needs a click on the
  diff; a stdio MCP server's host command needs the extra acknowledgement or
  the daemon answers 409. The dialog is drawn by the **core** in Rust
  (`approval.rs`); Luau can neither see nor press it.
- Audio is native, by thread desktop-audio (`src/audio.rs`).

## What was built

| Part | Where | What |
|---|---|---|
| Daemon routes | `src/client.rs` (`Daemon`) | `jarvis_status`, `jarvis_start`, `jarvis_proposals`, `jarvis_approve(id, via, ack)`, `jarvis_refuse`, `sessions`, `chat_push`, `voice_status`, `anonymise`, `transcribe`, `speak`, `voice_cancel`. A failed call is a typed `HttpError {path, code, message}` (`client::reason`, `client::status_of`), so a 409 is told apart. `FixtureDaemon` answers them from fixture keys and applies the daemon's rule (`client::fixture_rule`). Table in `daemon-api.md` |
| How words reach Jarvis | `src/shell/jarvis.rs` (`send`) | As the web UI (`concierge.js`, `vcDraft`): masked with `POST /api/voice/anonymise`, then `POST /api/chat/push {text, name: "sbxw-jarvis"}`. Answers: the `session` bus event of `sbxw-jarvis` (no thread), whose `reply` is taken when `state` goes from `working`/`attention` to `idle` (chat-core.js `chatIngest`); first look from `GET /api/sessions`. The grill rounds of concierge.js follow other sandboxes' agents and are not ported (Jarvis is talked to, not watched) |
| Core state and rule | `src/jarvis.rs` | The conversation (200 lines kept), status, proposals, the armed proposal, yes/no words (`VC_YES`, `VC_NO` of concierge.js), `hear()`: a yes approves the armed proposal only when low risk without host command, else "click needed"; a no refuses; anything else disarms and is a message (jarvis.js). Speech cut at sentences into 1000-character pieces (the daemon's limit) |
| `markdown` element | `src/markdown.rs`, `src/luau.rs`, `src/screen.rs`, `docs/screens.md` | Added to the frozen set (no existing element changed): paragraphs with line breaks, headings, `-`/`*`/`1.` lists, fenced code (monospace, wrapped, no scrollbar), inline code, bold, italics, links, bare URLs. Code spans read first, so **bold holding `code` renders** (test `bold_holding_inline_code_renders_both`). Only `http`/`https` are links; a link press calls the screen's `on` action with the URL, which returns `open_link` |
| Panel slot | `src/shell.rs`, `src/shell/jarvis.rs` (`panel_view`) | A tile right of the panes, in the same row: the panes narrow. A 6 pt handle (`jarvis-handle`) drags the width (280 to 720 pt, and leaves the panes 320 pt); width and open state kept in `~/.sbxw/desktop/window.json` (not in headless runs; `$SBXW_DESKTOP_PREFS` overrides) |
| Jarvis screen | `screens/jarvis.luau`, fixtures `fixtures/jarvis/*.json` | Header (sparkle, "Jarvis", the state word only when not running, "N to review", close), the log (`scroll` with `stick_bottom`, the only scroll), Start Jarvis when absent/stopped/failed, a progress bar while creating/starting, the input row (`jarvis-input`, talk button `mic` when the voice hears, send). Fixtures: `absent`, `starting`, `running-short`, `running-long`, `no-voice`, `proposal-low`, `proposal-high`, `proposal-ack`, `proposal-refused`, `proposal-screen`; `fixtures/shell/jarvis-two-panes.json` |
| Top bar | `screens/topbar.luau` | Button `jarvis-toggle`: "Jarvis", or "Jarvis · N to review" in green while proposals wait |
| Intents | `src/intent.rs` | `start_jarvis`, `send_to_jarvis {input}`, `toggle_jarvis_panel {open?}`, `talk_start`, `talk_stop`, `read_aloud {id}`, `stop_reading`, `approve_proposal {id}` (**risk Approval**: only opens the core's dialog), `refuse_proposal {id, reason?}`, `open_link {url}` (http/https only, checked in the registry). New argument types `Bool`, `Url` |
| Typed text belongs to the person | `src/screen.rs` (`Ui::typed`, `Ui::editing`) | `send_to_jarvis` carries no text: the core reads its own copy of the input. A yes answers a proposal only if the person typed it (the screen did not set it through `value`) and it was sent by Enter or a press, not from a per-keystroke action. Test `only_the_persons_own_yes_approves_and_only_low_risk` |
| Proposal dialog | `src/approval.rs` (`proposal_view`, trait `ProposalAnswers`) | Keys `jv-dialog`, `jv-approve`, `jv-refuse`, `jv-ack`, `jv-close`, `jv-next`, `jv-reason`. Title, summary, Low/High risk badge, the high-risk note, per change: kind, title, high-risk badge, result once answered, the host command in a red banner, for `desktop_screen`/`desktop_screen_remove` the daemon's `intents` list in plain words above the diff (from the registry's docs; new ones in amber with "NEW: the current screen does not"; approval-risk ones marked), the unified diff as text (monospace, coloured: `+` green, `-` red, `@@` blue, `#` amber, headers bright), each diff scrollable (max 280 pt), the whole body scrollable within the window. Approve is off until "I have read this command" is ticked when `needs_ack`. Refuse takes an optional reason. A refused approval (409) is shown in red beside the buttons and in the log. Not modal (as the web UI): the terminals keep the keyboard; ✕ is "later" |
| Push-to-talk and reading | `src/shell/jarvis.rs` | F8 (the daemon's `push_to_talk_key`, F1 to F24) held: `start_capture`; released: `stop_capture`, under 0.3 s dropped ("Too short"), else `transcribe`, then the words go through the same `hear()` as typed ones (a spoken yes approves only low risk). The talk button toggles (click to start, click to send). The window losing focus stops a recording. Jarvis's answer is read aloud when the person's last message was spoken; the speaker under an answer reads it again (shown when the daemon can speak). A recording shows a level bar from `CaptureLevel`. `CaptureFailed` text goes into the log |
| Keys a terminal leaves to the shell | `src/terminal/keys.rs` (`KeyOpts::reserved`, `is_reserved`), `src/terminal.rs` (`on_key_tx`) | Granted by the lead for this change only. A reserved key pressed without Cmd, Ctrl or Alt is not sent and `on_key_tx` returns false, so it bubbles to the shell. The shell sets F7 (the panel's key off macOS) and the push-to-talk key on every pane, and updates them when `/api/voice` changes. A key the shell cannot map leaves only F7 reserved and the talk button's tooltip says why. `KeyOpts` lost its `Eq` derive (`rune::Key` is not `Eq`); nothing used it. Test `reserved_keys_stay_with_the_shell` |
| Panel key | `src/shell/jarvis.rs` (`jarvis_key`) | ⌘J on macOS (⌘ keys are never sent to a program), F7 everywhere |
| Built-in export | `src/builtin.rs` | Below |
| Override folder notice | `src/shell.rs` | With `$SBXW_DESKTOP_SCREENS` (or `--screens-dir`) set, a notice (logged in headless runs): screens are read from that folder; Jarvis's changes go to `~/.sbxw/desktop/screens` |
| Stand-in | `src/bin/standin.rs` | `--jarvis absent\|running`, `--screens-dir`, `--voice TEXT`; the Jarvis routes, `sessions`, `chat/push`, `anonymise`, `transcribe`; a scripted Jarvis (words "english", "review", "command", "screen" make it propose low risk, high risk, a host command, a screen change written to `--screens-dir` on approval) |
| Scripts | `scripts/demo-jarvis.sh`, `scripts/headless/jarvis-*.json` | The end-to-end run; fixture flows; push-to-talk with a terminal focused; panel resize |

## The audio seam and its wiring

`src/voice.rs` (unchanged since it was sent to the lead, commit a346ed8;
`FakeAudio::queued` made public and `voice::Chain` added):

```rust
pub const CAPTURE_RATE: u32 = 16_000;
pub const MAX_CAPTURE_SECS: u32 = 120;
pub enum AudioEvent { CaptureStarted, CaptureLevel(f32), CaptureEnded, CaptureFailed(String),
                      PlaybackStarted, PlaybackLevel(f32), PlaybackEnded, PlaybackFailed(String) }
pub type Wake = Arc<dyn Fn() + Send + Sync>;
pub trait AudioIo: Send {
    fn available(&self) -> bool;
    fn start_capture(&mut self) -> Result<(), String>;      // stops playback first
    fn stop_capture(&mut self) -> Result<Vec<u8>, String>;  // 16 kHz mono f32 LE, <= 120 s
    fn cancel_capture(&mut self);
    fn play(&mut self, rate: u32, pcm: Box<dyn Read + Send>) -> Result<(), String>; // s16 LE mono
    fn stop_playback(&mut self);
    fn events(&mut self) -> Vec<AudioEvent>;
}
pub type AudioFactory = fn(Wake) -> Box<dyn AudioIo>;
```

After the merge: the seam mirror in `audio.rs` was replaced by a
re-export of `crate::voice`'s names, and `impl crate::voice::AudioIo for
Audio` forwards each method (the only edit in `audio.rs`; `src/audio/` not
touched). The app (`App::setup`) uses `Audio::boxed` as the factory in a
real window on a real daemon; headless runs and fixtures keep `NullAudio`,
scripts can ask for `FakeAudio` (`"fake_audio": true` in a fixture, or
`SBXW_DESKTOP_FAKE_AUDIO=1`); `SBXW_DESKTOP_NO_AUDIO=1` turns native audio
off. Per desktop-audio's report: device errors arrive as events, shown in
the log; taps under 300 ms are dropped here; capture always stops playback
first (the seam's rule), and `talk_start` clears the reading state; speech
longer than 1000 characters is one `voice::Chain` reader that fetches the
next `/api/voice/speak` when the previous body is drained, so audio sees one
clip.

## `builtin/` export (for the daemon thread jarvis-desktop-screens)

At every start of a real run (not headless, not on a fixture;
`$SBXW_DESKTOP_BUILTIN=<dir>` forces a folder), the app writes
`$HOME/.sbxw/desktop/builtin/`:

- `screens/<name>.luau`: each built-in screen as compiled in
  (`sandboxes`, `topbar`, `jarvis`; names match `[a-z0-9-]`, a test checks);
- `screens.md`: `docs/screens.md` verbatim (documents `markdown`, `data.jarvis`
  and every intent);
- `intents.json`: `{"intents": [{"name", "args": {"<arg>": "<type>[, required]"}, "risk": "direct"|"approval", "asks"?, "doc"}]}`.

Written to `.builtin.tmp-<pid>` beside it, then the old folder is renamed
away and the new one renamed in, then the old one deleted: a reader sees a
whole set. Nothing is ever read back from it. Overrides stay in
`~/.sbxw/desktop/screens/`.

**How a screen emits an intent** (for the daemon's scanner): the prelude
global `intent("name", { arg = value })`, returned from an action (one, or a
list). The core accepts any table `{ kind = "intent", name = "...", args = {...} }`,
which is what `intent()` builds, so a screen can also write that table by
hand, and its `name` need not be a literal there either. The scanner should
therefore flag, besides `intent(` with a non-literal name, any `kind =
"intent"` table (or the string `"intent"` used as a value). Whatever the
scan says, an intent of risk approval never runs from a screen.

## Evidence

Tests: `cd desktop && cargo test --release`: **114 passed**, 2 ignored
benchmarks (110 unit, 3 link, 1 pump). Before: 67 here, 99 on
desktop-audio. New here: markdown (5), jarvis rules (6), intents (in the
existing two), builtin export, reserved keys, the approval rule through the
app (`only_the_persons_own_yes_approves_and_only_low_risk`: a screen-set
"yes" approves nothing, a typed one approves the low-risk proposal with
`via: voice` and empties the input, "oui" on a high-risk one is "click
needed", a forced voice approval of it gets the daemon's 409, the screen's
`approve_proposal` only shows the dialog, the host command refuses the click
until ticked, a 409's reason is kept for the dialog), `markdown` in the
element set and its doc example.

Shots, each looked at (`desktop/.sbxw-artifacts/jarvis/`):

- `closed.png`: two panes side by side, each half the area; "Jarvis" button
  in the middle of the top bar; no panel.
- `open.png` (`fixtures/shell/jarvis-two-panes`): the same two panes
  narrowed (about 312 pt each), the panel on the right: header "Jarvis",
  the seeded question in a bubble on the right, the answer as Markdown with
  its bold sentence, the input row at the bottom.
- `resized.png` (`jarvis-resize.json`, a 160 pt drag on the handle): the
  panel 540 pt wide, the panes 232 pt each (tree bounds).
- `strip-states.png`, the panel alone per fixture: absent (explanation and
  Start Jarvis), starting (progress bar), running with a short answer (the
  speaker under it, the mic in the input row), running with a long answer
  (scrolled to its end inside the log: lists, inline code, a code block, a
  blue underlined link, headings; the input row stays at the bottom, no
  other scrollbar), daemon without voice (no mic, no speaker).
- `screen-running-long.png`: the long answer alone.
- `win-proposal-low.png`, `win-proposal-high.png`, `win-proposal-screen.png`,
  `win-proposal-ack.png`, `strip-dialogs.png`: low risk (green badge, one
  setting diff in colour); high risk (red badge, amber note, two diffs);
  a screen change (the intents box: "Opens a terminal...", "Marks a sandbox
  as selected", "Stops a sandbox (`stop_sandbox`) NEW: the current screen
  does not" in amber, then the diff starting with the daemon's `#` line);
  the host command (red banner with `npx -y @playwright/mcp@latest
  --headless`, the "I have read this command" box unticked, Approve dimmed).
- `ack-ticked.png`, `ack-applied.png` (`jarvis-ack.json`): ticked, Approve
  lit; after the click, "Applied" on the change, "Applied." and Close, the
  log says "Applied: Add the Playwright MCP server".
- `refused.png` (`jarvis-refused.json`, fixture answering 409): red line
  "Not applied: the file /Users/ana/dev/web/sbxw.toml changed since the
  proposal..." above the buttons, and the same in the log; still pending.
- `yes-low.png` (`jarvis-yes.json`): "yes" typed and Enter: the proposal is
  applied (log "Applied: Read answers with the English voice").
- `yes-high-told.png`, `yes-high-clicked.png`
  (`jarvis-yes-then-click.json`): "yes" on the high-risk one: the log says
  "High risk: approve it on screen, with the Approve button. A yes does not
  approve this one." and nothing is approved; the click on Approve applies
  it. `strip-flows.png` puts these six side by side.
- Against the stand-in, `scripts/demo-jarvis.sh` (one script, a shot per
  step, `demo-grid.png` four of them): Start Jarvis (absent, creating,
  running in 1.5 s); "hello Jarvis" answered by the scripted Jarvis
  (`demo-answered.png`); "use the english voice" proposes, "yes" applies it
  (`demo-low-approved.png`); "add a review command" proposes high risk,
  "yes" is told to click (`demo-high-told.png`), the click applies it
  (`demo-high-approved.png`); "change the sandboxes screen", click Approve:
  the stand-in writes the override, the watcher reloads it and the sidebar
  title reads "MY SANDBOXES (by Jarvis)" (`demo-screen-applied.png`). Log:
  `approve p-2 via voice`, `approve p-3 via click`, `approve p-4 via click`,
  `screen sandboxes loaded from /tmp/.../sandboxes.luau (load #2)`; the
  stand-in received no approval for the high-risk yes.
- `ptt.png` (`jarvis-ptt.json`, stand-in with `--voice`, `FakeAudio`, a
  bash terminal with Rune's focus on `grid-1`): F8 pressed (and a repeat),
  released 0.6 s later: "1 s recorded", the stand-in transcribed it, Jarvis
  heard "use the english voice" and proposed. The terminal's text dump shows
  no byte from F8 (`\e[19~` would print `~`), and `echo typed-after-F8` typed
  next reached bash.

## Not verified

- **The real daemon and a real Jarvis session.** Only the stand-in and
  fixtures. The shapes were read from `crates/daemon/src/http/jarvis.rs`,
  `ui.rs`, `svc/jarvis.rs`, `svc/jarvis_sandbox.rs` and
  `crates/core/src/jarvis.rs`; the session-turn rule from chat-core.js.
- **macOS**: not built here. ⌘J, F8 on a Mac keyboard (where F8 may be a
  media key: Fn+F8, or "Use F1, F2... as standard function keys"), the
  window, the open-link path.
- **Voice**: real microphone and speakers through `audio::Audio` were not
  run from the app here (no window, no sound card); headless runs use
  `NullAudio` or `FakeAudio`. desktop-audio's own tests and `audio-demo`
  cover the device side. Reading aloud with the real speech stream, its
  chaining over 1000 characters and its timing are unverified.
- The approval dialog over a live terminal with a real pointer (it is not
  modal; the terminal keeps the keyboard).
- The daemon's `voice` bus event changing the push-to-talk key at run time
  (code path exercised only on connect).

## Known limits

- The talk button toggles (Luau buttons have no hold); F8 is the hold.
- Jarvis's answers are read aloud only after a spoken message (there is no
  mute switch in the panel yet); the speaker reads any answer on demand.
- The web UI's draft cards for spoken words with no target are not ported:
  spoken words that cannot leave (Jarvis not running) are put in the input.
- One proposal dialog at a time, as the daemon keeps one pending.

## First real run: two fixes

Thread `desktop-jarvis-fixes`, branch `sbxw/desktop-jarvis-fixes`. The
user's first run on a Mac against the real daemon showed two defects, then
a third (the microphone did not silence Jarvis). Line numbers below are
those of the code before the fix (commit c7a3a18).

### What the code shows about the run

- The desktop cannot read aloud a reply it did not log: before the fix,
  `read()` was only called right after `say()` (`src/shell/jarvis.rs:178`),
  and only when the person's last message was spoken
  (`Wiring::spoken`). A typed message is never read back.
- The web UI reads every Jarvis answer and every question aloud on its own
  (`assets/js/concierge.js:1089`, `vcAgentQuestion` at 1250, voice unmuted
  by default). The user confirmed a browser tab was probably open at the
  same time. So the voice heard for the missing first answer most likely came
  from that tab, while the panel had dropped the answer.
- The daemon sends one `session` event **per session of the sandbox** on
  every hook (`crates/core/src/agents.rs:442`, `infos_for(sandbox)`), and a
  session it first hears of mid-turn starts as `working` (`SessionState`
  default). A Claude Code question (`AskUserQuestion`) puts the session in
  `attention` with `steps`, and the next turn starts when it is answered.

### Defect 1: the first answer was not in the panel (and the stuck "…")

Cause, two parts, both in `src/jarvis.rs`:

1. `ingest_session` (`src/jarvis.rs:230-241`) only took a reply on `idle`
   after `working`/`attention`, and ignored `attention` entirely. A first
   answer that is a question with numbered options (`AskUserQuestion`, which
   the person then answered with "2") was never shown; the answer after "2"
   was an ordinary reply and was shown. That matches the screenshot.
2. It kept **one** previous state for the whole sandbox
   (`session_state`, line 230) and set `working` from whichever event came
   last (line 231). With a second session in the sandbox (for example an
   older one left as `working`), the events alternate, so a turn's
   `working` to `idle` is seen across two sessions (a reply dropped), and
   `working` ends `true` after every hook: the lone "…" drawn by
   `screens/jarvis.luau:68`. The seed (`seed_sessions`, line 247) could also
   overwrite the state with a stale snapshot.

Fix: each session is followed by `session_id`; a reply is new when that
session's own turn ended, or when a session never seen before is idle with a
reply while our message waits for its answer (`awaiting`); a question
(`attention` with `steps`) is shown as Jarvis's message with its options
numbered and "Answer with the number of your choice." (a typed number answers
it, as the person did); each reply and question is shown once. "…" means:
our message is not answered yet, or the newest session is working; it clears
on a reply, a question, a failed send, Jarvis leaving `running`, and a
reconnect.

Rule, enforced in code: anything read aloud is a message of the log, read
by id (`read(msg)` takes no text); a message is read by itself at most once
(`Jarvis::auto_read`), again only with its speaker button. The proposal
announcement that used to be read without a log line now has one.

### Defect 2: "Not sent: Jarvis is not running..." stayed at the top

Cause: `send()` wrote the refusal into the conversation as an error line
(`src/shell/jarvis.rs:458`); the log keeps every line, so it stayed above
everything after Jarvis started.

Fix: a refused send is not a conversation line. It is shown in red beside
the input row (`data.jarvis.send`), the text stays in the input, and one
button says what it does: **Start Jarvis and send** (absent, stopped,
failed) or **Send once it runs** (creating, starting). That button returns
`start_jarvis` and `send_to_jarvis` together; only that pair queues the
text, which leaves once Jarvis runs (`send_kept`). A plain Start, or Enter
while Jarvis starts, never sends later. The line goes when Jarvis runs, when
a send succeeds, or when the person edits the input (the screen's
`edited` action); with the line shown, the log's own Start button is hidden,
so there is one Start button. Emptying the input cancels a queued send.

### Defect 3: the talk button did not silence Jarvis

Cause: `read()` asks the daemon for speech on a thread
(`src/shell/jarvis.rs:577-588`); its answer, `JEvent::Speech`, was played
unconditionally (line 381). Pressing talk while that speech was still being
made stopped nothing (nothing played yet) and the speech then played into
the open microphone, which recorded Jarvis. Nothing in `src/audio/` is at
fault: `start_capture` does stop what plays.

Fix: every reading has a generation; `stop_reading()` (called by talk, F8,
the speaker button and "stop reading") bumps it, stops playback, clears the
panel's reading state at once, and sets the stop flag of the reading's
`voice::Chain` so no further piece is fetched. A `Speech` of an older
generation, or arriving while recording, is dropped. Nothing is read while
the microphone is open: an answer arriving then is shown, not read; nothing
resumes after the capture.

The doubled reading the user heard is most likely the browser tab reading
the same answer. The desktop now says so: when the daemon's `clients` event
counts another `/api/stream` client, one muted line under the header reads
"sbxw is also open in another window or browser tab: Jarvis may be read
aloud there too." (The island uses `/api/events` and is not counted.)

### Evidence

Tests (`src/shell/jarvis.rs`, module `tests`, the app with a fixture daemon
and a shared `FakeAudio`, events shaped as the daemon sends them):
`first_answer_with_options_is_shown_and_working_clears`,
`a_reply_from_a_session_never_seen_is_the_answer_once`,
`talk_stops_the_reading_for_good_and_nothing_is_read_twice`,
`a_refused_send_offers_start_and_send_and_clears`. Run against the code
before the fix (a detached worktree of c7a3a18 with the same tests), all
four fail: the question is missing (`left: 0, right: 1`), the never-seen
session's reply is missing, the speech that arrived after talk was played
(`left: 2, right: 1`), the refusal is in the conversation. After the fix:
`cargo test --release`: 118 passed (114 unit, 3 link, 1 pump), 2 ignored.

Shots (`scripts/demo-jarvis-first-run.sh`, stand-in with
`--jarvis absent --stale-session yes`; `strip-first-run.png` puts the first
four side by side):

- `first-refused.png`: "changez la voix" typed, Enter: no conversation line;
  above the input, in red, "Not sent: Jarvis is not running." with the green
  **Start Jarvis and send** button; the text still in the input; the panel's
  explanation without a second Start button.
- `first-edited.png`: one Backspace later: the red line and its button are
  gone, the log's **Start Jarvis** is back, "changez la voi" in the input.
- `first-asked.png`: after the button: Jarvis started, the kept text left by
  itself (log "running, sending the kept message as asked"), the input is
  empty, the error gone, and the first answer shown: "Which voice should
  read my answers?", the list 1. fr_female 2. en_female 3. de_male, "Answer
  with the number of your choice."; no "…" although the stand-in's older
  session stays `working`.
- `first-answered.png` and `first-end.png` (2 s later): "2" in a bubble,
  then the second answer "You chose **en_female**..."; no "…" under it.
- `first-others.png` (`--clients 2`): the muted line about another window or
  tab under the header.
- `scripts/demo-jarvis.sh` rerun: same steps and outcomes as before
  (`demo-high-told.png` refreshed: no stray "…").

### Not verified

- The real daemon and a real Jarvis session: the event shapes come from
  `crates/core/src/agents.rs`; whether the Mac's sandbox really had a second
  session, and whether the first answer was an `AskUserQuestion`, cannot be
  seen from here. Both cases are handled.
- macOS, the real microphone and speakers (`audio::Audio`): the reading race
  is covered with `FakeAudio`; the timing on a real device is not.
- `docs/screens.md` (outside this thread) does not yet list
  `data.jarvis.send` and `data.jarvis.others`.

### Check on the Mac

1. Close every browser tab of sbxw first (then once more with one open, for
   step 6). `cd desktop && git pull && cargo run --release`.
2. If Jarvis runs, stop it (`sbx stop sbxw-jarvis`). Open the panel (⌘J),
   type `changez la voix`, Enter. Expect: no line in the conversation, the
   red "Not sent: Jarvis is not running." beside the input with **Start
   Jarvis and send**, your text still in the input. Type one letter: the
   line goes. Press Enter again: it comes back.
3. Click **Start Jarvis and send**. Expect: Jarvis starts, then your text
   leaves by itself, the input empties, the red line is gone.
4. Jarvis's first answer, whatever its form (a question with options or
   text), shows in the panel. If it lists options, type `2`, Enter: the
   next answer shows. After each answer, no "…" stays under it.
5. Hold F8 (or click the mic) and ask something; while Jarvis's answer is
   being read, click the mic: the reading stops at once and does not
   resume; release or click again: what you said is sent, not Jarvis's
   words. Ask again and wait: each answer is read once.
6. Open the web UI in a browser tab: the panel shows "sbxw is also open in
   another window or browser tab: Jarvis may be read aloud there too."
7. If something differs, send the log: `RUST_LOG=info cargo run --release
   2>~/sbxw-jarvis.log`, the lines starting with `jarvis`.

## Test on the Mac

1. Audio alone first (isolates device problems), from `AUDIO.md`:
   ```sh
   cd desktop && git pull
   cargo build --release --bin audio-demo
   target/release/audio-demo list      # built-in mic [default input], speakers [default output]
   target/release/audio-demo tone      # a clean 1 s beep
   target/release/audio-demo record 4  # allow the terminal to use the microphone, speak, hear it back
   target/release/audio-demo roundtrip --seconds 4   # with the daemon and the voice installed
   ```
2. The app: `sbxw up` (daemon running), then `cargo run --release`. Expect
   the top bar's **Jarvis** button. Press ⌘J (or click it): the panel opens
   on the right and the terminals get narrower. Drag the panel's left edge:
   it resizes. Quit and run again: same width, same open state.
3. If Jarvis was never started: the panel explains it and offers **Start
   Jarvis**; click it. A progress bar, then (a minute or two the first time)
   the input says "Ask Jarvis".
4. Type `What can you change for me?`, Enter. The input empties, "…" shows,
   then the answer in Markdown. Turn the daemon off and send again: the
   panel says why and the text stays in the input.
5. Type `Read your answers in English`. When the dialog opens with a
   **Low risk** badge, type `yes` and Enter: "Applied: ..." in the log.
6. Ask for something high risk (`Add a /review command to my web project`).
   The dialog shows **High risk** and the diff. Type `yes`: the log says a
   click is needed and nothing is applied. Click **Approve**: applied.
7. Ask for an MCP server that runs a command (`Add the Playwright MCP
   server`). The red banner shows the command; Approve stays dimmed until
   **I have read this command** is ticked.
8. Click into a terminal, then hold **F8** (Fn+F8 if the Mac maps F8 to
   media) and say "what can you do?"; release. The level bar moves while
   you speak; the words appear in the log and Jarvis answers aloud. Check
   the terminal received nothing from F8. Say "oui" to a low-risk proposal:
   applied; to a high-risk one: Jarvis says to click.
9. Ask Jarvis to change a screen (`Add a Stop all button to the sandbox
   list`): the dialog lists what the screen can ask the app to do, new ones
   marked. Approve: the sidebar changes within a second.
10. If something fails, paste the app's log lines starting with `jarvis` or
    `audio` (`RUST_LOG=info cargo run --release 2>~/sbxw-jarvis.log`), and
    say which step.
