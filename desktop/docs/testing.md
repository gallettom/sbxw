# Testing the desktop app (for agents and people)

Everything here runs headless on Linux: Rune draws offscreen with wgpu on a
**virtual clock** (60 frames per virtual second, as fast as the CPU allows).
No display, no Xvfb, no xdotool. With no GPU, Mesa's software Vulkan driver
does it (`mesa-vulkan-drivers`; to build: `build-essential pkg-config
libxkbcommon-dev libwayland-dev`). All commands are run from the repository
root; the first build takes about a minute, later ones a few seconds.

| I want to | Run |
|---|---|
| See one screen from one fixture, and its element tree | `desktop/scripts/screen.sh sandboxes basic` |
| Try a screen file I am writing | `desktop/scripts/screen.sh myscreen basic --file path/to/myscreen.luau` |
| See the whole window from a fixture (no daemon) | `desktop/scripts/shot.sh shots/x.png --fixture two-panes` |
| See the whole window with a live terminal | `desktop/scripts/shot.sh shots/x.png --type 'ls -la'` |
| Check nothing sits under macOS's window buttons | add `--chrome` to `shot.sh` (prints `0 overlapping elements` or the culprits) |
| Show live list updates | `desktop/scripts/demo-live.sh` |
| Show hot reload through the override folder | `desktop/scripts/demo-reload.sh` |
| Show a daemon restart (new token) reconnecting the list and a terminal | `desktop/scripts/demo-reconnect.sh` |
| Show another window (the web UI) taking the PTY's size, and typing taking it back | `desktop/scripts/demo-shared-size.sh` (`OTHER=200x60` for a larger one) |
| Two live terminals: scrollback, selection, focus, a dead key | `desktop/scripts/shot.sh shots/x.png --script desktop/scripts/headless/two-panes-live.json` |
| Run the unit and link tests | `cd desktop && CARGO_PROFILE_DEV_DEBUG=0 cargo test -j1` |

Shots and logs go to the checkout's root artifacts folder
(`.sbxw-artifacts/desktop/shots/`, git-ignored), or `$SBXW_SHOTS`; never into
`desktop/`. Keep `CARGO_TARGET_DIR` on the host mount (the scripts default it
to `desktop/target`; a thread uses `desktop/target-<name>`, git-ignored, and
deletes it when done).

Every demo (each writes its shots and says what they show in its header):

| Script | Shows |
|---|---|
| `demo-browser.sh` | the sandbox's browser panel |
| `demo-commits.sh`, `demo-worktrees.sh` | the Commits and Worktrees views |
| `demo-configviews.sh` | the MCP servers, Skills and Kits views out of Setup |
| `demo-create.sh` | a whole creation, chooser to running |
| `demo-explorer.sh`, `demo-files.sh` | the file explorer; the files and the code map |
| `demo-jarvis.sh`, `demo-jarvis-first-run.sh` | Jarvis end to end; the first run on a Mac, replayed |
| `demo-live.sh` | list updates made behind the app |
| `demo-mac.sh` | the virtual Mac panel |
| `demo-netfilter.sh`, `demo-network.sh` | the Network panel's filter; the network area and the core's dialog |
| `demo-pages.sh`, `demo-tiling.sh` | pages and the pager; tiling by drag and drop |
| `demo-panebar.sh`, `demo-rail.sh`, `demo-statusbar.sh` | the pane bars and top bar fit; the left rail; the status bar |
| `demo-pills.sh` | keyword pills in a terminal (each tone, a selection, the switch off live, a light theme, the scaled view) |
| `demo-reconnect.sh`, `demo-reload.sh`, `demo-shared-size.sh` | a daemon restart; hot reload; one PTY and two clients |
| `demo-relay.sh`, `demo-setup.sh`, `demo-threads.sh` | the relay; the Setup dialog; threads, plans and the lead |

## `screen.sh`: one screen, one fixture

```sh
desktop/scripts/screen.sh SCREEN [FIXTURE] [--out f.png] [--tree f.json] [--script s.json] [--at T] [--size WxH] [--file my.luau]
```

- `SCREEN`: `sandboxes`, `topbar`, or a new name with `--file`.
- `FIXTURE`: a name under `desktop/fixtures/SCREEN/` (default `basic`) or a path.
- Draws the screen alone (`--solo`), 360x720 by default, at 0.5 s (or half
  a second after the script's `end` mark).
- Writes `shots/screen-SCREEN-FIXTURE.png`, `.json` (the tree) and `.log`,
  and prints the screen's load line or its error (`screen x: render: ...`).
- Your own override folder is never used: the scripts point
  `SBXW_DESKTOP_SCREENS` at an empty folder unless you set it.

## Fixtures

`desktop/fixtures/<screen>/<name>.json`:

```json
{
  "daemon": { "sandboxes": [ { "name": "docs", "agent": "codex", "status": "running" } ] },
  "app": { "selected": "docs", "panes": 2, "split": "columns", "open": ["web-app", "docs"], "focused": 2,
           "connection": { "state": "down", "text": "No sbxw daemon at 127.0.0.1:7681. Start it with `sbxw up`; retrying." } }
}
```

- `daemon`: what each route answers, by the key the screen reads. Today
  `sandboxes` (`GET /api/sandboxes`). A later screen adds its key (`threads`,
  `plans`, ...); it reaches the screen as `data.<key>` unchanged.
- `app` (all optional): the core's state at start. `open` puts sandboxes in
  panes (no terminal: a fixture has none); `connection` shows another state
  than "up".
- A fixture is also a fake daemon: pressing Stop in a fixture run changes the
  fixture's copy in memory, so scripts can press buttons.
- `cargo test` checks every fixture parses and every built-in screen renders
  against each of its fixtures without an error.

Fixtures: one folder per screen or feature under `desktop/fixtures/`
(`shell/` for whole-window states). `app` also takes `modal`, `panels`,
`jarvis_open`, `attention` (panes that want you) and `tabs` (start in the
Tabs view); see `src/fixture.rs`, `AppInit`. Data beside fixtures (a
terminal sample, shared test cases ending in `-cases.json`) is not one.

## Scripts: pressing things

Rune replays a JSON script on the virtual clock (`--script s.json`):

```json
{"events": [
  {"t": 0.3, "type": "tap", "key": "row-docs"},
  {"t": 0.6, "type": "tap", "key": "pager-per"},
  {"t": 0.8, "type": "tap", "key": "pager-per-2"},
  {"t": 1.0, "type": "type", "text": "echo hi", "every": 0.02},
  {"t": 1.4, "type": "key", "name": "Enter"},
  {"t": 1.5, "type": "scroll", "x": 180, "y": 400, "dy": -600},
  {"t": 2.0, "type": "mark", "name": "end"}
]}
```

- `tap` by **key**: the keys in `--tree` (`elements[].key`). Screens' keys are
  their `key` prop or `<on>-<arg>` (`stop-docs`, `row-docs`); menus are
  `<id>`, `<id>-list`, `<id>-<item>` (see `docs/screens.md`). The core's own:
  `top-bar`, `sidebar`, `rail` and its entries (`rail-requests`,
  `rail-network`, `rail-leads`, `jarvis-toggle`, `rail-setup`), `pager`,
  `page-N`, `pager-view-tabs`, `pager-view-grid`, `pager-per`, `pane-N`,
  `pane-N-bar`, `pane-N-more`, `pane-N-close`, `pane-N-mode-agent`,
  `pane-N-mode-bash`, `terminal-N`, `grid-N`, `notice`, `modal`.
- Every key a script names must exist on the **first frame**. In a run
  against a daemon the app waits up to 1.5 s for the first list before that
  frame, so rows are there.
- `type` sends each character as typed. A `key` event with `"shift": true`
  only sets the modifier (`"a"` with shift still types `a`): name the
  character (`"A"`) instead.
- `ime` (`preedit` with `cursor`, then `commit`) is a dead key or an input method: it goes to the focused pane.
- Other events: `down`, `up`, `drag`, `double-click`, `key` with `ctrl`,
  `alt`, `cmd`, `paste`, `fling`, `resize` (`dw`, `dh`, `dur`). Rune's
  README ("Agent tooling") has the full list.
- Marks the app itself acts on:
  - `hold:<ms>`: wait that long in **real** time, taking in what arrives
    from the daemon and the screen watcher. The virtual clock does not wait
    for a stop to finish or a file to be edited; a hold does.
    `demo-reload.sh` watches the app log for `hold <ms> ms` to know when to
    edit the file.
  - `screen:<name>=<path>`: load that file as that screen now, through the
    same reload code as an edit.
  - `wait:<key>[:<ms>]` and `wait-gone:<key>[:<ms>]`: wait in real time until
    an element of that key is drawn (or gone), at most 10 s by default; prefer
    them to a fixed hold before tapping what the daemon's answer draws.
  - Any other name (`end`) is just a time: `--at mark:end+0.5`.
- Headless runs also wait (real time) for what the app started itself: a
  terminal's echo (up to 1.5 s a frame, then 100 ms of quiet), and a stop or
  other background job (up to 1.5 s).

## What to look at

- `--snap out.png --at T`: one PNG (`--size WxH`, `--scale S`).
- `--tree tree.json`: every element with key, role, label, text, rect,
  colours, hover state, plus the accessibility tree and `warnings`. The
  terminal's lines are not in it: add `--dump-text file` (the focused
  terminal's size and text, rewritten when it changes).
- `--strip strip.png --start T --frames N --every K --cols C --crop x,y,w,h`:
  frames side by side from one run (how the demos show a sequence).
- `--chrome`: paints macOS's three window buttons on the snapshot and prints
  `rune: chrome: traffic lights at [...], N overlapping elements`. The
  headless run always reserves their room, on every OS; a Linux or Windows
  window reserves none.
- The app's log (beside the PNG): `screen X loaded from ...`, `intent from X:
  ...`, `list: NAME is now STATUS`, `hold ...`, daemon connection lines.

## The stand-in daemon

`shot.sh` and the demos start `desktop/src/bin/standin.rs` on a free port
unless `SBXW_DAEMON` and `SBXW_TOKEN` name a real daemon (then
`SBXW_SANDBOX=<name>` picks the terminal's sandbox). It serves the routes the
app covers (`desktop/docs/daemon-api.md`) from
`fixtures/sandboxes/basic.json`, with a local bash for every terminal.
`--change SECS:NAME=STATUS` (on `start_daemon` in a script) changes a
sandbox behind the app's back.

Against the real daemon on a machine with sbx:

```sh
SBXW_DAEMON=127.0.0.1:7681 SBXW_TOKEN=$(cat ~/.sbxw/run/ui-token) SBXW_SANDBOX=<name> desktop/scripts/shot.sh
```

## Tests

### Running them

| Suite | Command (from the repository root) | Tests |
|---|---|---|
| Desktop | `cd desktop && CARGO_PROFILE_DEV_DEBUG=0 cargo test -j1` | about 450, plus 9 against a real stand-in process |
| Crates | `cargo test -p sbxw-core -p sbxw-daemon -p sbxw-sbx -p sbxw` | about 740 |
| Web UI matcher | run by `crates/core`'s `the_web_matcher_passes_the_shared_cases` (needs `node`) | the shared pill cases |
| Jarvis's MCP server | `node scripts/test-config-mcp.mjs` (by hand: nothing runs it yet) | 8 checks |
| Web UI smoke | `scripts/ui-smoke.sh` (by hand, needs Playwright) | about 30 steps |

A thread keeps `CARGO_TARGET_DIR` in its own worktree (`desktop/target-<name>`,
`target/<name>`, both git-ignored) and sets it on every command. A scratch copy
of the sources (a mutation check) gets a target dir of its own: two copies of
one package in one target dir share artifact names, and cargo then runs the
other copy's binary.

Measured 2026-11-18 in the sandbox (10 cores, 11 GB, other sessions building: load
average 4 to 9), `CARGO_PROFILE_DEV_DEBUG=0` for the desktop, the default for
the crates; tests only, the build excluded:

| Suite | Wall time (parallel) | Serial sum | Slowest |
|---|---|---|---|
| Desktop lib | 41 to 44 s (was 72 to 82 s) | about 210 s | the modal rule's three tests (60 s together), the configure panels (19 s), the gate matrix (11 s), the usage-bar fit (10 s) |
| Desktop `tests/` | 7 s | 7 s | `tests/link.rs` (real restarts) |
| Daemon | 11.6 s (was 90 s) | 40 s (was 225 s) | `no_stats_are_read_while_nobody_listens` (11.5 s: the stats ticker on the real clock) |
| Core, sbx, cli | 0.5 s, 0.1 s, 1 s | | |
| Every demo script, one after the other | about 36 min | | `demo-files`, `demo-setup.sh window`, `demo-relay`, `demo-browser` (each mark is its own run, replaying the script from the start) |

### Writing one

- **A new test names the behaviour or the rule it protects**, in its name and
  in a line above it ("Rule (docs/security.md): ...", "Design rule (the
  user's): ..."), so the next cleanup can tell what it guards.
- Check that it fails when the rule breaks: break the code by hand in a scratch
  copy and run it. A test no sensible break can fail protects nothing.
- No real time: no `sleep` to wait for something, no wall-clock bound. Wait
  for the event or the mark (`wait:` in scripts, a channel, a polled condition
  with a generous deadline that only stops a hang); move a clock the test
  owns (the hover zones' `screen::advance_zone_clock`); set file times
  (`File::set_modified`) instead of waiting for a second to pass; let a fake
  `sbx` answer what the code waits for (`testing::sbx_answer_booting`).
- Process-wide state (`surface::FOCUSED`, the network panel's `DISMISS`
  queue) is shared by tests running in parallel: hold a test lock, or retry
  a set-and-check, and say why.
- Not a second copy: when a shared case list exists (`emphasis-cases.json`,
  `host/exec_fs.rs`'s `both()`), add the case there.

### What they cover

- `luau`: sandbox limits (no io/os/require, read-only globals and theme,
  memory cap, instruction budget on load, render and actions, element count),
  every prop checked, every element, prop and example of `docs/screens.md`
  present and rendering.
- `intent`: the registry parses known intents and refuses the whole batch on
  anything else; every intent documented.
- `screen`: the override file wins, reloads with state kept, a broken edit
  keeps the last good view, deleting it restores the built-in (a real
  watcher thread on real files); refused intents are shown, not run;
  built-ins render against every fixture.
- `shell::gate_tests`: the keyboard gate, layer by layer. Under the core
  dialog, a modal, an overlay, Jarvis's proposal and the palette, no key,
  chord, input-method text, paste, Ctrl/Cmd+V, wheel or press reaches a
  terminal (one predicate, `App::panes_covered`, for the window's path and
  the grid's); a menu and a focused surface keep the keyboard; the OSC 52
  card is never accepted by a key; Jarvis's scrim holds the panes and gives
  the keyboard back; the typed yes still works; a drop on a covered pane
  starts nothing; an overlay under the proposal takes no key or file for a
  terminal or an upload.
- `theme`: every colour token equals `assets/app.css`'s.
- `client`: SSE parsing, unknown sandbox fields kept, readable reasons.
- `tests/link.rs` (real stand-in process): a change behind the app reaches
  the list; a restarted daemon with a new token in the token file is found
  again; no daemon gives the one-line message.
- `tests/pump.rs`: key-to-echo through the terminal socket thread against an
  in-process echo server, idle wake-ups, close on drop; the latency budget is
  checked in a release build (`cargo test --release --test pump --
  --nocapture` prints the numbers).
- `terminal`: key bytes, mouse reports, OSC 8 and OSC 52, bracketed paste,
  reconnects. Keyword pills: the matcher, emphasis-core's shared cases
  (`fixtures/emphasis/emphasis-cases.json`, a copy of
  `crates/core/testdata/`), pills over exactly their cells, copy and
  selection unchanged, the switch, contrast; `tests/pills_alloc.rs` proves a
  frame without markers allocates nothing.
- `shell::layout_tests`: pages, tiling, Tabs then Grid, the team leads view,
  the mode switch; `modal_content_stays_inside_the_modal_in_every_*` (modal
  fixtures, Setup sections, create dialogs) checks every modal at 420 and
  1284 pt.

## Performance

`desktop/scripts/perf.sh` builds the release binary and runs one script
headless against the stand-in with realistic sizes
(`scripts/perf-fixture.py`, generated on each run: 11 sandboxes, 200
commits with their details, a 3,000-entry folder, 113 requests). It prints,
per step (open Commits, expand a commit, its diff, a filter keystroke,
Files, a folder, hovering):

- the app's own probes (`SBXW_DESKTOP_PERF=1`, also usable on a real run:
  each Luau render, the data pushed to the screens and how many rendered,
  the core's view, each daemon request, off the UI thread);
- Rune's split of each input, input to present (`RUNE_PERF=1`, through
  `scripts/perf-collector.py`, a local OTLP/JSON collector): handler, view,
  commit, layout, text, paint, apply, and the node count.

It exits 1 when an input takes over `PERF_BUDGET_MS` (160 ms: ten frames, a
regression check on a loaded machine, not a benchmark) or a data push
renders more than the screens drawn. `src/perf.rs` says what the probes
cost when off (one cached flag). Numbers and causes: the checkout's
`.sbxw-artifacts/desktop/PERF.md`.

## Jarvis

- `scripts/screen.sh jarvis <fixture>`: the panel alone (`fixtures/jarvis/`).
- `scripts/shot.sh out.png --fixture fixtures/jarvis/proposal-high.json [--script scripts/headless/jarvis-yes-then-click.json]`: the whole window with the core's dialog.
- `scripts/demo-jarvis.sh`: end to end against the stand-in's scripted Jarvis (start, message, typed yes on low risk, yes then click on high risk, a screen change hot reloaded). Shots in `shots/jarvis/`.
- Push-to-talk: `SBXW_DESKTOP_FAKE_AUDIO=1`, stand-in `--voice "text"`, `scripts/headless/jarvis-ptt.json` (F8 `key` then `key-up`).
- See `JARVIS.md`.
