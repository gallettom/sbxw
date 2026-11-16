# sbxw desktop, phase 4, first wave: fixes, threads and plans, relay

Three threads on one branch (`sbxw/team-p-6bfb2cd194f22ac57c7c28a2292f6ee9`):
desktop-integ (the phase 3 bugs, integration, core slots, Templates, env
file, shortcuts, the app's state), desktop-threads (threads, plans, the
lead, the board), desktop-relay (relay requests, screenshots, following
another sandbox from Jarvis). Built and tested on Linux (aarch64, no GPU,
headless Rune renders) against fixtures and the stand-in daemon. **Nothing
was run on a Mac, against the real daemon or against real sbx.**

Reports: [THREADS.md](THREADS.md) (threads, plans, lead, board),
[RELAY.md](RELAY.md) (relay, screenshots, following), this file for the rest.
Routes: [daemon-api.md](daemon-api.md). Screen contract and "Adding a
feature": `desktop/docs/screens.md`. Shots of this thread: `integ/`.

## What each area delivers

| Area | Delivers | Through the core's own dialog |
|---|---|---|
| Phase 3 fixes (desktop-integ) | A new sandbox never replaces a live terminal while there is room; sbx's requests counted as the web UI counts them, quiet when nothing waits; stale screen overrides said and restorable; extras an old screen does not draw drawn by the core; the gear is a cog | Restoring a built-in screen (deletes the override, names the file) |
| Core slots (desktop-integ) | Docked panels as tabs or side by side; session sub-rows under a sandbox (`Place::SandboxRow`) and per-session pane-bar extras (`Place::Session`); files dropped and pictures pasted handed to a feature (`Feature::on_files`), `PickFiles`, `ReadClipboard`; one picture decoder with limits (`picture.rs`) and an `image` element; a `textarea` element; `copy_text`, `close_panes`; global shortcuts declared by features; the app's state file (`Action::Remember`, `data.app.kept`); per-sandbox read errors | Clipboard and files only from the person's own press |
| Templates (desktop-integ) | The Templates dialog: saved templates (in use, automatic), save a sandbox as one, disk usage (`sbx df`, every item) | Removing a template |
| Env file (desktop-integ) | The environment file card (`sbxenv.yaml`): folder, preview, copy, save, overwrite | none (as the web UI) |
| Create and Setup leftovers (desktop-integ) | Alt+N (New) and Alt+M (chat), Ctrl+Option on macOS; the remote host remembered across runs; Setup opens on the last tab; copy the provision command | |
| Threads and plans (desktop-threads) | See [THREADS.md](THREADS.md): sessions under each sandbox, the Threads panel, new thread or session, close, Team mode, the plan dialog with edits, approve and merge, the board, worktrees as text | Closing a thread, approving a plan, merging, ending Team mode, Remote Control login |
| Relay (desktop-relay) | See [RELAY.md](RELAY.md): the Requests panel (questions from one sandbox to another, routed or answered yourself; screenshot requests with pictures dropped, pasted or from the clipboard button; rounds as one form with recommendations, drafts and Answer by voice; virtual Mac requests); agents waiting on AskUserQuestion answered from the panel; following another sandbox from the Jarvis panel (picker, Follow, words as a draft) | Routing a request and every release (the text that leaves, masked, and where it goes; pictures with their size and source); words to a followed sandbox; a choice for it |

## The phase 3 bugs: cause and fix

**A.1 "quand la sandbox démarre elle prend le tty de l'autre au lieu de
prendre un nouveau slot".**
Cause: `src/shell.rs:815` (phase 3, `pane_for`, `PaneTarget::Spare`). When
no shown pane was empty it grew the layout by one and opened the sandbox in
pane `layout`, whatever that pane held. A pane hidden by a smaller layout
(two terminals, then the switch set back to "1 terminal") keeps its live
terminal, and that terminal was replaced. That is the only path in phase 3's
code where a spare open replaces a live terminal while the layout has room. I
could not reproduce the person's exact sequence; if they never had more than
one pane, there is another cause, and step 3 of the procedure below will
show it.
Fix (`spare_pane`): the first empty pane on screen; else grow by one, and if
the pane revealed still holds a terminal, swap it with an empty hidden pane
first, so it is kept; only with four busy panes is one replaced: the least
recently used (`Pane::used`, from opens and focus), never the focused one,
and a notice names what it closed. Every "spare" path uses it: create's
ready sandbox, the row's +, Bash, a kit-add pane, threads' panes. Test:
`shell::tests::a_spare_pane_never_replaces_a_live_terminal_while_there_is_room`
(on phase 3 the hidden `web-api` terminal is replaced). Shot:
`integ/a1-spare-pane.png` (layout 1 with a hidden terminal on `docs`; + on
data-pipeline grows to two panes; docs is kept).

**A.2 "sbx asks 85" in the top bar, "sbx asks 2" on a new pane.**
Cause: `src/feat/network.rs:381-392` (phase 3, `extras`) counted every row
of `GET /api/approvals`. That route is `sbx policy approval ls --format
json` with no filter (`crates/daemon/src/http/ui.rs:819`,
`crates/sbx/src/lib.rs`, `approvals`): the whole machine's requests,
including those of sandboxes sbxw does not list (removed ones, Jarvis's
`sbxw-jarvis`, which reaches many hosts). The web UI never shows a total: it
counts per listed sandbox (`crates/daemon/src/svc/sandboxes.rs:134-147`,
`pending_for`; `assets/js/approvals.js` lists one sandbox's). Every request
was also amber: a `network` request is a connection sbx already refused
(nothing waits; Allow helps the next try), unlike a kit's `port` request,
which holds the kit and lapses after 45 s.
Fix: `network::waiting` keeps only listed sandboxes' requests (top bar, rows,
pane bars, the panel, notices); `holds` tells refused hosts (quiet, a neutral
button) from waiting ones (amber); the first read after start makes no notice
for refused hosts. Fixture `fixtures/network/machine-wide.json`: rows in the
shape of a real sbx 0.51 row (`crates/core/src/approvals.rs` test), 81 for
unlisted sandboxes, 2 refused hosts of `web-app`. Test:
`network::tests::only_listed_sandboxes_requests_count_and_refused_hosts_are_quiet`
(phase 3 shows 83, amber). "sbx asks 2" on a new sandbox is real, as the web
UI's ⚑ 2: hosts it reached while provisioning that sbx refused; now quiet.
Unverified: the user's 85 rows themselves (sbx cannot run here).

**A.3 An old override hid the new row extras, without a word.**
Cause: `src/screen.rs:263` (`read_source`): an override file wins over the
built-in with no check, and extras exist only if the screen draws them.
Fix: the app's state file (`~/.sbxw/desktop/state.json`, `shell/store.rs`)
keeps for each override the hash of the built-in compiled in when the
override appeared or last changed (never read from `builtin/`). An override
whose built-in has since changed is stale: one notice ("Your sandboxes screen
is a customised copy made for an older version; new features may be missing.
Ask Jarvis to update it, or restore the built-in."; a click opens the
dialog), and an amber marker at the top of the screen for as long as it
lasts. Its press opens the core's dialog, which names the file;
**Delete and restore** removes it (`screen_restore`, risk Approval, a core
operation run only from the dialog). An override with no record (made before
this check, the person's case) is stale when its file is older than the
running app. Extras: `docs/screens.md` now says a screen that hosts extras
must draw them with their keys; the core draws any extra the screen did not
draw in a strip under it (beside it for the top bar), found by element key,
so nothing is drawn twice; a sandbox's are looked for only if its row is
drawn (a filter does not bring them back). Tests:
`shell::tests::an_override_older_than_its_builtin_is_flagged_and_restorable`,
`screen::tests::the_sandbox_list_draws_session_rows`. Shot:
`integ/a2-a3-stale-override-and-quiet-badge.png` (an old copy of the list:
the marker, the fallback strip with ⚑ 2, ⇄ and …, the notice; "sbx asks 2"
quiet in the top bar).

**A.4 The gear looked like a sun.** Cause: `src/icons.rs:34`, spokes round a
circle. Fix: a cog outline with eight square teeth and a hole. Visible in
every shot's top bar.

## Integrator notes

- Notices: phase 3 pinned them over the bottom right of the window for 4 s,
  over the Jarvis input and its talk and send buttons, so a click meant for
  the input ran the notice's action (found by desktop-relay). A notice now
  sits **in the top bar's layout**, at its right end: one line (its full text
  as a tooltip), what a click does, a × to dismiss. It covers nothing,
  takes only its own clicks, and stays under any modal and core dialog drawn
  over the window. Checked headless with the Jarvis panel and the Network
  panel open and a notice showing: a tap on the Jarvis input focuses it and
  typing goes there (`integ/notice-in-top-bar.png`; the notice's box is
  inside the top bar's, the input's far below). Scripts no longer need to
  wait notices out.

- Registered: `envfile`, `relay`, `templates`, `threads` (and phase 3's).
- Stand-in: `GET /api/sessions` adds the fixture's `sessions` (one key for
  threads, relay and Jarvis's seed); `POST /api/chat/push` to another sandbox
  goes to features; a `/ws` open with `mode=thread` is handed to features as
  `OPEN /ws?<query>`. desktop-threads' stand-in moves a plan one step per
  `GET /api/plans`, at most every 1.2 s, and its merge conflict happens once.
- `tests/pump.rs` was flaky under load (three builds at once): it counted the
  last echo's wake-up as idle traffic. It now settles 100 ms first.
- Builds broke once at integration: `a9df0b3` registered `threads` while its
  module was still untracked (a clean checkout would not build);
  desktop-threads committed it. `861ba98` (mine) went in with a test red,
  fixed by `659c4c5`.
- Ignored tests (3), each with its reason: `terminal::tests::full_screen_redraw_cost`
  and `big_cat_throughput` (benchmarks, `--release --ignored`),
  `create::tests::write_create_fixtures` (regenerates `fixtures/create`).

## Decision for the user: native file dialog

`Action::PickFiles` opens Rune's native open dialog only with Rune's
`native-file-dialog` feature, which this plan does **not** enable. Until then
a feature asking for it shows "Drop the file on this panel, or paste an
image." Drop and paste cover today's uses (screenshot requests).

What enabling it costs (resolved with `cargo metadata` in a scratch copy, not
compiled):

- **One new package in the lock: `rfd` 0.17.2, MIT.** Every dependency of it
  is already in the lock (`objc2`, `objc2-app-kit`, `objc2-foundation`,
  `objc2-core-foundation`, `block2`, `dispatch2` on macOS; `windows-sys` on
  Windows; `libc`, `percent-encoding` on Linux; `raw-window-handle`, `log`).
- **macOS, Windows:** nothing to install (AppKit's panel, as a sheet over the
  window; the Windows common dialog).
- **Linux:** no new build-time library: Rune shows the dialog through the XDG
  desktop portal over zbus, already compiled for its `dbus` feature. At run
  time a portal with a FileChooser backend must be installed
  (`xdg-desktop-portal` and `-gtk`, `-gnome` or `-kde`); without one the
  dialog fails and the same notice shows.
- **Rune's dialog picks one file** (`pick_file`); `PickFiles { multiple }` is
  kept for when it can pick several.

Recommendation: enable it (`rune = { ..., features = ["tooling",
"native-file-dialog"] }`): one small MIT crate, nothing new on macOS, and
the Project/files panel of the next plan will want it.

## What of the web UI is still not ported

| Web UI | Why |
|---|---|
| Files and artifacts, uploads (`files.js`, `upload.js`) | Next plan (the core's intake, decoder and `image` element are ready for it) |
| Browser side panel, embed pairings (`browser.js`) | Next plan; its needs are below |
| Virtual Mac (`mac.js`), Setup's Mac tab | Next plan; its needs are below |
| Code map runs (`codemap.js`), SSH card (`ssh.js`) | Not in this plan |
| Agent activity strips, `/api/ptys`, `/api/hook/log` | Not in this plan (threads show each session's state word instead) |
| Threads: the worktree graph drawn (shown as text), the board's clock times, editing the distillate, a multi-line goal field | No drawing surface for graphs and no clock in Luau; the `textarea` element now exists, so the goal field can use it next ([THREADS.md](THREADS.md)) |
| Relay: capture a window (`getDisplayMedia`) | The app has no screen capture; ⌘⇧⌃4 (to the clipboard) then paste does the same ([RELAY.md](RELAY.md)) |
| Relay: Choose a file… | Waits on the native file dialog decision (above); drop and paste work |
| `chat-dock.js` (the ephemeral-chat dock) | Chats open in a pane; their AskUserQuestion is answered from the Requests panel; the dock's own transcript and tabs are a feature of their own |
| The grill one question at a time (`vcGrill`) | Replaced by one form per round, with Answer by voice (the same commands) |
| A spoken "oui" sending a draft to a followed sandbox | The core's dialog is answered by a click only |
| Setup: MCP edit prefill | `sbx mcp ls` gives no detail of a server; needs the daemon to answer one |
| Setup: a kept skill's date | No date formatting in Luau; the daemon would send a readable date |
| Network: the policy log's sort by decision, `sbx policy ls` raw fold | Unchanged since phase 3 |
| The native file dialog | A decision for the user (above) |
| Image paste off macOS | Off macOS the window takes no paste (Ctrl+V is the terminal's); a drop works wherever Rune has file drops (not Wayland) |
| The "several tabs" warning | Unchanged since phase 1 |

## For the next plan: sandbox browser and virtual Mac

Both show a remote picture and send input back. What the core needs, not
built now:

1. **A picture surface drawn by the core**, in a pane (like a terminal) or a
   docked panel (like Network): a `Slot::Surface { title, icon }` or a pane
   mode whose content is a `rune::Pixels` the feature's Rust updates
   (`Pixels::set(w, h, rgba)`; Rune redraws on change, no screen re-render
   per frame). `picture.rs` decodes PNG/JPEG/WebP with limits; frames of a
   stream want a cheaper path (decode off the UI thread, keep the last frame,
   drop late ones).
2. **Pointer and key events handed to the feature's Rust**, never to Luau:
   pointer move, press, release and wheel in the picture's own pixels (the
   surface knows its scale and letterbox), and keys with their physical codes
   and modifiers while the surface has the focus. A new
   `Feature::on_surface(&mut SurfaceState, SurfaceEvent) -> Vec<Action>`;
   the shell's global shortcuts and Escape stay the shell's.
3. **A long-lived feature WebSocket through the link**: `Link::socket(path,
   on_message)` for `/ws/browser` and the Mac's stream, authenticated like
   `/ws` (token, proof), reconnecting as terminals do, frames delivered to the
   feature's Rust on the link's thread, never through Luau.
4. **Risk Approval** for what widens access: a browser allow
   (`POST .../browser/allow`), pairing an embed, taking the Mac back from
   another client; through `Confirm` with the daemon's words, as network's.
5. **Clipboard and files**: `copy_text`, `on_files` and the decoder exist; a
   download needs a native Save dialog (the same Rune feature as above).

## One macOS test procedure, every area

Prerequisites: on the Mac, this branch checked out; `sbxw up` running; two
running sandboxes with a workspace (`web-app` and `docs` below); the app
built and started with no arguments, keeping the log:
`cd desktop && RUST_LOG=info cargo run --release 2>&1 | tee app.log`. The web
UI open beside it helps to compare.

**Phase 3 fixes (this file)**

1. Look at the top bar: the gear is a cog. If sbx has requests, "sbx asks N"
   counts only those of sandboxes in the list (compare with the web UI's ⚑ on
   each row: same numbers; the web UI shows no total). With only refused hosts
   it is grey, not green. On a machine with Jarvis or removed sandboxes, the
   old 85 should be gone.
2. Your old `~/.sbxw/desktop/screens/sandboxes.luau` (if it is still there):
   at start a notice says the sandboxes screen is a customised copy made for
   an older version; an amber line sits at the top of the list; the "…", ⇄
   and ⚑ of each row show in a strip under your copy. Press the amber line:
   the app's red dialog names the file. **Keep my copy**: nothing changes.
   Again, then **Delete and restore**: the file is gone
   (`ls ~/.sbxw/desktop/screens`), the built-in list is back with its row
   buttons, and the marker is gone.
3. Layout "2 side by side", open `web-app` in pane 1 and `docs` in pane 2,
   then choose "1 terminal". Press + on a sandbox in no pane: the layout grows
   to two panes and it takes pane 2. Choose "3 terminals": `docs` is still
   there, live. With four panes all busy, press + on another sandbox: the
   least recently used pane (not the focused one) takes it, and a notice
   names what it closed.

**Core slots, Templates, env file, shortcuts**

4. Press ⇄ on web-app (Network), then open a second panel (Threads or
   Requests): tabs. Press the columns icon at the right of the tabs: both show
   side by side; press it again: tabs.
5. Press … on web-app, then **Save as a template…**: the Templates dialog
   lists `sbx template ls` (in use, automatic) and disk usage as `sbx df`
   prints it (tick Every item). Type a tag, Save: "Saving…", then a notice;
   the tag is listed. **Remove** on it: the app's red dialog names the tag;
   Cancel, then Remove: it leaves the list.
6. Press **Env** on web-app's pane bar: the card shows `sbxenv.yaml` and its
   folder. Type a folder that does not contain the workspace, Preview: the
   line under the field says the file goes in the parent. **Copy**: a notice
   shows the size and the first characters; paste in an editor: the same
   text. **Save file**: a notice names the file; Save again: the card says it
   exists; tick Overwrite and Save: written.
7. Press Ctrl+Option+N (⌃⌥N): New opens. Ctrl+Option+M: the chat form. In a
   terminal, Option+N still types `~` (dead key) as before.
8. New, Workspace sandbox, pick a remote host (if you have one), Cancel; quit
   the app and start it again; New, Workspace: that host is preselected. Open
   Setup on the Hosts tab, close it, restart the app, open Setup: Hosts.

**Threads and plans** ([THREADS.md](THREADS.md), its 15 steps in full)

9. Create a chat sandbox (New, Chat): it opens in a new pane, never in a busy
   one (step 3's rule).
10. On web-app's pane bar press **Threads**: the panel lists the main
    session. **New thread** `t1` on a repository: a pane opens for it, and a
    sub-row `⑂ t1` appears under web-app in the sidebar with its state. Close
    the thread from its bar: the app's dialog lists what goes; confirm: its
    pane closes and the layout shrinks.
11. Press **Team** on the main pane and ask for a small plan of two threads:
    the plan dialog opens by itself. Edit a goal, Approve: the app's dialog
    lists the threads as you left them and the session count; they start in
    their own panes. Follow THREADS.md steps 10 to 15 for the merge.

**Relay** ([RELAY.md](RELAY.md), its procedure in full)

12. Follow RELAY.md's 20 steps. In short: ask a question from web-app with
    `ask_other_sandbox`; the Requests panel opens; route it (the app's dialog
    shows the question and the target) or answer it yourself; Refuse is
    direct. For a screenshot request, drop a PNG on the panel, or take a
    screenshot to the clipboard (⌘⇧⌃4) and press ⌘V while the panel shows
    and no terminal has the focus: the picture is attached; Send lists each
    picture with its size. A 10 MB PDF or an animated picture is refused with
    a notice. `/grill-me` in a sandbox: the round opens as one form. In
    Jarvis, pick web-app in the title: the panel shows web-app's state (no
    "Jarvis absent", no Start Jarvis), and your words become a draft for
    web-app in the app's dialog, never a yes to a Jarvis proposal. Stop
    web-app from the sidebar: the panel goes back to Jarvis and says nothing
    was sent.
13. A notice (for example after step 6's Copy) sits at the right end of the
    top bar. While it shows, click the Jarvis input and type: the text goes
    into the input; the notice only acts when clicked itself.

**Across areas**

14. Stop the daemon with panels and a dialog open: the top bar says so;
    panels keep their last values. `sbxw up`: the app reconnects within
    seconds, the lists refresh, and the panes' terminals come back.
15. Quit and restart: panels start closed; Setup's last tab, the create host
    and the overrides' records are kept (`~/.sbxw/desktop/state.json`).

If a step fails, report the step number, a screenshot and `app.log`.
