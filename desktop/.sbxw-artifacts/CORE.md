# sbxw desktop, wave 1: the core

Thread `desktop-core`, branch `sbxw/desktop-core`, on the Rune spike
(`sbxw/desktop-rune`, merged fast-forward as allowed). Only `desktop/` was
touched, and not `src/terminal.rs` (thread desktop-terminal). Linux
(Debian trixie, aarch64) in an sbx sandbox, no GPU, no display.

## User decisions (recorded as given)

- Renderer: Rune, pinned at `09ab92a5`, bumped only on demand.
- Rust core; screens in sandboxed Luau, hot reloaded, state kept, a broken
  file keeps the last good view.
- The app is only a client of the existing daemon over its HTTP and
  WebSocket API, through one module (`src/client.rs`, trait `Daemon`). No
  daemon change.
- Same look as the web UI.
- Override folder for screens: `~/.sbxw/desktop/screens/`.
- A closed list of intents; sensitive approval dialogs drawn by the core.
  The user added: "Oui, mais on verra si on fait evoluer ca plus tard". So
  the list is one registry with a declared risk per intent
  (`intent::REGISTRY`), to be widened later without rewriting screens. Not
  widened now.
- No installer or signing before parity.

## What was built

| Part | Where | What |
|---|---|---|
| Shell | `src/shell.rs`, `src/main.rs` | The web UI's layout: dark floor, 48 pt top bar, 240 pt sidebar, pane area of 1 to 4 terminals (2 side by side or stacked, 3 and 4 as a grid) with draggable 6 pt handles; per pane a 32 pt bar (connection dot, sandbox, mode, status, close). Click focuses a pane; keys go to the focused terminal. A notice at the bottom right for action results |
| macOS buttons | `shell::view` | The top bar keeps `Cx::titlebar_inset()` free on its left (Rune's own measure; 0 off macOS) and is a window-drag region. `--chrome` headless: `0 overlapping elements` |
| Theme | `src/theme.rs` | Every `--c-*` and `--r-*` token of `assets/app.css` (a test compares them with the CSS), text sizes, spacing, layout sizes; the same table is Luau's read-only `theme` |
| Daemon client | `src/client.rs` | `Daemon` trait: `sandboxes`, `stop_sandbox`, `open_terminal` (`TerminalConn` unchanged), `subscribe` (`/api/stream` SSE), `describe`. `HttpDaemon` (health proof, bearer token, daemon errors shown as their text), `FixtureDaemon` (JSON). `Link`: one thread keeps the connection up, re-reads the token file on every reconnect, backs off 0.25 s to 3 s, turns stream events into list reads, runs actions off the UI thread |
| Defaults | `shell::App::new` | No arguments: `127.0.0.1:7681`, token from `~/.sbxw/run/ui-token`. Daemon absent: one line, `No sbxw daemon at 127.0.0.1:7681. Start it with \`sbxw up\`; retrying.`, retried in the background |
| Live list | `client.rs`, `shell.rs` | Re-read after a stop, 300 ms after `session`/`provision`/`lead`/`lead-end`/`codemap` events, at 1.5/3/6/12/24 s after opening the agent of a sandbox that is not running (the web UI's recheck; covers "a terminal started the sandbox"), every 15 s |
| Reconnect | `shell::on_link` | On a new connection, panes whose terminal closed open again |
| Element set (frozen) | `src/luau.rs` (`SPEC`), `docs/screens.md` | `text row column list scroll button input icon badge dot split spacer divider menu select checkbox progress`, common props, `help` tooltips, theme names in props. Unknown element, prop or value type: an error naming it |
| Icons | `src/icons.rs` | A closed set of 26 SVG icons |
| Intents | `src/intent.rs` | Registry: name, typed arguments, risk. Wave 1: `open_terminal`, `start_sandbox`, `stop_sandbox`, `select`, `set_layout`, all `Direct`. Anything else refuses the whole batch, shown in the screen's banner |
| Approvals slot | `src/approval.rs`, `App::intents` | An intent of risk `Approval` is never run from a screen: queued for a dialog the core draws in its own layer. Not built: such an intent is refused with a notice until wave 4 |
| Screens | `src/screen.rs`, `screens/` | Built-ins compiled in (`include_str!`); `<override dir>/<name>.luau` wins, hot reloaded by a watcher thread, deleting it restores the built-in; `$SBXW_DESKTOP_SCREENS` overrides the folder. Core-held element parts (input text, split ratio, open menu). Menus paint above every tile |
| Sandbox list | `screens/sandboxes.luau` | Grouped as the web UI groups (chats, workspace folders with 2+ sandboxes, the rest), name, state dot and word, agent, approvals flag, pane badge; row click = select + agent in the focused pane; start / stop / shell / open in a spare pane; filter input |
| Top bar | `screens/topbar.luau` | Layout picker (`select` → `set_layout`), connection state |
| Fixtures | `src/fixture.rs`, `fixtures/` | `{daemon, app}` JSON per screen; every screen renders from one with no daemon |
| Stand-in | `src/bin/standin.rs` | Mutable fixture list, `POST stop`, `/api/stream`, agent terminal starts a stopped sandbox, `session` event after each change, `--change`, `--token-file` |
| Harness | `scripts/`, `.sbxw-artifacts/testing.md` | `screen.sh` (named screen + named fixture → PNG + tree), `shot.sh` (whole window; `--fixture`, `--chrome`), `demo-live.sh`, `demo-reload.sh`; script marks `hold:<ms>` and `screen:<name>=<path>` |

Docs: `desktop/docs/screens.md` (the screen contract),
`desktop/.sbxw-artifacts/daemon-api.md` (routes covered and not),
`desktop/.sbxw-artifacts/testing.md` (the harness). The last two are in
`.sbxw-artifacts/` and not `docs/` because this repo's hook refuses new
non-code files under `docs/`; move them if the lead prefers.

## Evidence

Tests before the terminal merge: `cd desktop && cargo test`: **32 passed**, 1 ignored (the terminal
thread's benchmark): 28 unit tests, 3 in `tests/link.rs` against a real
stand-in process, 1 in `tests/pump.rs`. Notable: `luau::every_example_in_the_doc_renders` renders
the example of every element in `screens.md`;
`screen::override_file_wins_reloads_and_deleting_it_restores_the_builtin`
uses real files and the real watcher thread (it found a race, fixed: an edit
made before the watcher thread started was missed);
`link::a_restarted_daemon_with_a_new_token_is_found_again` kills the
stand-in and starts a new one with a new token in the token file: reconnected
in 0.2 s; `link::a_change_behind_the_app_reaches_the_list`: seen 1.3 s after
the first list for a change made at 1 s.

Images in `.sbxw-artifacts/core/`, each one looked at:

- `shell-fixture.png` (`shot.sh --fixture two-panes`, no daemon): the floor,
  top bar with `sbxw` and "2 side by side", the green dot and "fixture
  two-panes" at the right; sidebar "SANDBOXES 5": a CHATS card
  (chat-0412), a WEB card (web-app with pane badge 1, web-api stopped with a
  green ▶), then data-pipeline and docs (⚑ 2 flag, pane badge 2, selected);
  two panes, the second with a blue border (focused), each saying it has no
  terminal in a fixture.
- `chrome-two-panes.png`: the same with `--chrome`: the three macOS buttons
  painted at the top left, clear of `sbxw`; the log says `0 overlapping
  elements`.
- `term.png` (`shot.sh --type 'echo hello from the core; uname -s'`, stand-in):
  pane 1 "web-app bash", "terminal 119x36 connected", the bash prompt,
  the typed line, `hello from the core`, `Linux`, a new prompt and the block
  cursor; web-app's row has pane badge 1.
- `menu-crop.png`: the layout picker open over the sidebar and pane: five
  entries with icons, a check on "1 terminal".
- `two-terms.png` (`scripts/headless/two-terminals.json`): picked "2 side
  by side" from the menu, pressed + on docs: two live terminals, 58 columns
  each; the right one ran `echo pane two; tput cols` → `pane two`, `58`.
  Log: `SetLayout{2, Columns}`, `Select docs`, `OpenTerminal docs Spare`.
- `live-sidebars.png` (`demo-live.sh`, one run, five frames 0.5 s apart):
  (1) data-pipeline stopped, docs running, web-api stopped; (2) after a tap
  on the data-pipeline row: it is `running`, in pane 1 (the stand-in started
  it because its agent opened, and the list followed from the stream);
  (3) after Stop on docs: docs `stopped`, its button now ▶; (4) web-api
  `running`, changed by the stand-in on its own with nothing pressed;
  (5) unchanged. Log order: `list: data-pipeline is now running`, `stop
  docs: done`, `list: docs is now stopped`, `hold 6000 ms`, `list: web-api is
  now running`. `live-end.png` is the full window at the end.
- `reload-strip.png` (`demo-reload.sh`, real edits in the override folder
  while the app holds): (1) built-in, "web" typed in the filter, two rows;
  (2) the override appears: amber "SANDBOXES (edited live)", filter kept;
  (3) the file broken: red banner "screen error, showing the last good
  view: syntax error: sandboxes.luau:147: Expected <eof>, got 'return'"
  over the unchanged view; (4) fixed: banner gone; (5) file deleted: the
  built-in title is back, the filter still "web". Log: loads #1 built-in, #2
  override, the syntax error, #4 override, #5 built-in.
- `screen-sandboxes-{basic,many,down,empty}.png`, `screen-topbar-basic.png`
  (`screen.sh`): the list alone; 30 sandboxes in 4 groups cut off by the
  scroll area (a scripted 600 pt wheel moves `row-svc-01` from y 518 to
  -89); the amber banner with the "No sbxw daemon" line; "No sandboxes yet";
  the top bar alone.

## The terminal socket thread (asked by desktop-terminal through the lead)

`client::pump` no longer loops on a 5 ms read timeout. It sleeps in
`mio::Poll` (mio 1.2.4, already in the lock through tokio, same features)
on the socket (readable and writable, edge-triggered, drained to
`WouldBlock`) and a `mio::Waker`. `TerminalConn::input` keeps its type,
`mpsc::Sender<TermInput>`: on this branch `terminal.rs` stores it as that
concrete type, so a new type would not compile here. A forwarder thread
blocks on the channel and rings the waker per message. A full socket buffer
keeps a frame in tungstenite's buffer until the next writable event.

`tests/pump.rs` (release, in-process echo server with blocking reads, 200
keys one at a time): before p50 12.0 ms, p90 13.5, p99 16.6, max 16.9;
after p50 0.055 ms, p90 0.059, p99 0.113, max 0.141. The test also checks an
idle terminal wakes the UI 0 times a second (it was 0 before too: the 83
wake-ups a second measured by desktop-terminal were the socket thread's own,
which the new loop removes by construction, as it sleeps with no timeout;
not counted by this test), that a resize still goes out, and that dropping
the input closes the socket. Linux (epoll) only: macOS (kqueue) and Windows
(the duplicated socket handle registered with IOCP) are unverified.

The stand-in still polls with a 5 ms timeout on its side, so key-to-echo in
headless runs against it stays about 14 ms (p50 13.8 ms in `term2`); it is a
test tool, not changed.

## Not verified

- **The real daemon, with these changes.** Only the stand-in. The lead
  verified the spike's client (proof, list, terminal) on the user's Mac;
  `/api/stream`, `POST stop`, the token-file reconnect and the start
  recheck are unverified against the real daemon. First check:
  `SBXW_DAEMON=127.0.0.1:7681 SBXW_TOKEN=$(cat ~/.sbxw/run/ui-token) SBXW_SANDBOX=<name> desktop/scripts/shot.sh`,
  then the app with no arguments.
- **macOS and Windows** builds of this branch: not built. The traffic-light
  room is Rune's `titlebar_inset`, checked only with `--chrome` headless.
- **A real window and keyboard**: only Rune scripts. Hover, tooltips, the
  split handle drag, Escape on a menu, focus by click were not exercised in
  a window; dragging the handle was not scripted at all.
- Windows' `USERPROFILE` fallback for the token path is a guess (the daemon
  itself uses `HOME`).
- `web_addr` from `sbxw.toml` is not read; a non-default address needs `--daemon`.

## Known limits

- A press outside an open menu does not close it (Escape or the button do).
- The desktop's stream connection counts as a browser tab for the web UI's
  "several tabs" warning.
- Headless runs always reserve the macOS button room (Rune reports the macOS
  inset in headless on every OS).
- After a tap, a button's tooltip shows in headless snapshots (Rune draws
  tooltips for the pointer at rest); harmless, visible in `live-end.png`.
- Each terminal draws its own status line (size, connection) above its grid (`terminal::pane`), under the core's pane bar.
- A pane's terminal is opened on the UI thread (a loopback WebSocket
  handshake, a few ms); a slow daemon would stall a frame.

## Terminal wiring (after merging sbxw/desktop-terminal a0cf739)

Merged once, as the lead allowed; `src/terminal.rs` and `src/terminal/`
not edited. The shell uses the terminal's new API per pane:

| What | Where |
|---|---|
| `rune::App::new(spec).on_ime(..).run()`; the input method goes to the focused pane | `main.rs`, `App::on_ime` |
| `terminal::pane(t, lens(i))` in a row, one lens per pane (`shell::lens`); `TerminalState::id` = the pane number, so keys stay `grid-N`, `terminal-N` | `pane_view` |
| `poll_tx` for every pane, then `take_events()`: `Attention` puts an amber dot and a "wants you" chip on the pane bar and a notice, until a key in that pane; `Title` shows on the pane bar; `Disconnected`/`Reconnected` make the pane's status `reconnecting`; `Bell` is logged | `App::poll` |
| OSC 52 `Clipboard`: **ignored**, with a notice ("a program asked to copy N bytes to your clipboard; ignored"). My judgment: a program in a sandbox should not write the person's clipboard on its own; when wanted, it belongs behind a core-drawn approval. term-demo copies up to 4 KB; the web UI's behaviour was not checked | `App::poll` |
| `on_key_tx` to the focused pane (window `on_key`); the grid's own `on_key` when it has Rune's focus | `App::on_key`, `terminal::view` |
| Root `on_paste` (the macOS Edit menu takes Cmd+V before any key handler) and `on_window_focus` | `shell::view` |
| `set_focused(true)` only on the pane with the keyboard while the window has it, `false` on the others (hollow cursor, one IME caret) | `App::poll` |
| Which pane has the keyboard: follows Rune's focus (a click in a terminal focuses its `grid-N`, read in `poll` through `tx.focused()`), and Rune's focus follows the core when an intent opens or focuses a pane (`tx.focus`) | `App::poll` |
| `TerminalState::with_reconnect(conn, reopen)`; `reopen` asks whatever daemon the link holds *now* (a restarted daemon has a new token, so the old `HttpDaemon` would be refused). The shell's own "reopen closed panes on Up" was removed | `App::open` |
| Theme: `Theme` defaults (the web UI's); the person's `terminal_color` settings are not read yet | |
| Headless: steady cursor (`cursor_blink = false`), and `hold:` marks now poll the terminals too, so a reconnect runs during a hold | |
| Sender type: kept the forwarder thread (`TerminalConn::input` stays `mpsc::Sender<TermInput>`), so nothing of desktop-terminal changed | `client.rs` |

Evidence (looked at):

- `core/wiring-scrollback.png` (`scripts/headless/two-panes-live.json`, at
  5.5 s): two live panes. The right one (docs) ran `seq -f 'history line %g'
  1 300` and was scrolled up by Shift+PageUp and the wheel: "scrolled 40 of
  266 lines", lines 226 to 261. The left one got the keyboard by a tap on
  `grid-1` and shows `echo dead key: ê` → `dead key: ê` (a scripted
  `^` preedit then `ê` commit through `App::on_ime`), and `echo alpha bravo
  charlie`.
- `core/wiring-selection-focus.png` (same run, end): "bravo" selected by a
  double click in the left pane, which has the solid cursor; the right pane,
  wheeled back to the bottom without taking the keyboard, ends on `history
  line 300` and its prompt with a **hollow** cursor.
- `core/wiring-reconnect.png` (`scripts/demo-reconnect.sh`): top, `echo
  before the restart` against the first stand-in; bottom, after the stand-in
  was killed and a new one started on the same port with a **new token in
  the token file**: the same pane `connected`, `echo after the restart`
  answered. Log: stream ended, connection refused, connected again, a new
  terminal socket. The earlier lines are gone because the stand-in does not
  replay; the daemon replays its last 256 KB.
- Regressions rerun: `demo-live.sh` (3 list changes in order),
  `demo-reload.sh` (5 loads as before), `--chrome` (0 overlapping),
  desktop-terminal's `term-shot.sh` with the scrollback script (offset 69,
  history 269).

Tests: 59 pass (55 unit, of which the terminal thread's; 3 link; 1 pump),
2 ignored benchmarks.

Unverified for the wiring: a real Mac keyboard (dead keys, Option
characters and IME were scripted only), the clipboard (copy, paste, the
Edit menu's Paste), focus by a real click in a real window, the daemon's
`attention` frame in the shell (the stand-in does not send one; the
terminal thread tested the event itself), reconnect with the real daemon and
its replay.

## Screens that can now be built in parallel

Each needs only `docs/screens.md`, a fixture under `fixtures/<screen>/`, and
the routes listed for it in `daemon-api.md`. The core owner adds the
`Daemon` methods, the fixture keys, the registry rows for its intents and
the slot in the shell where it shows.

| Screen | Data it needs (new `Daemon` methods / fixture keys) | Intents to add |
|---|---|---|
| Threads under a sandbox (sidebar sessions, thread dialogs) | `threads`, `repos`, `worktrees`, `thread_status` | `open_terminal` with `thread` exists; `close_thread`, `start_thread` |
| Plans and lead board | `plans`, `leads`, `board` | `approve_plan`, `merge_plan` (risk Approval), `refuse_plan`, `start_lead`, `end_lead` |
| Relay requests | `relay`, `relay` stream events | `route_relay` (Approval), `deny_relay` |
| sbx approvals, ports and network | `approvals`, `ports`, `policy` | `respond_approval`, `add_policy_rule` (Approval), `publish_port`, `unpublish_port` |
| Files and artifacts (Project panel) | `artifacts`, `fs`, `fs_recent` | `open_file`, `download_artifact`, `delete_file` (Approval) |
| Create sandbox (workspace and chat) | `fs`, `favourites`, `carve_preview`, `templates` | `create_sandbox`, `create_chat` |
| Setup: secrets, GitHub, kits, skills, MCP, templates | the setup routes | one per action; secrets and tokens behind Approval |
| Usage and header extras | `usage`, `state` | none |
| Jarvis proposals | `jarvis` | the proposal dialog is the core's (`approval.rs`), not a screen |
