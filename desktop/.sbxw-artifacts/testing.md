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
| Two live terminals: scrollback, selection, focus, a dead key | `desktop/scripts/shot.sh shots/x.png --script desktop/scripts/headless/two-panes-live.json` |
| Run the unit and link tests | `cd desktop && cargo test` |

Outputs go to `desktop/shots/` (git-ignored). Keep `CARGO_TARGET_DIR` on the
host mount (the scripts default it to `desktop/target`).

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

Fixtures now: `sandboxes/{basic,empty,many,down}`, `topbar/{basic,down}`,
`shell/two-panes`.

## Scripts: pressing things

Rune replays a JSON script on the virtual clock (`--script s.json`):

```json
{"events": [
  {"t": 0.3, "type": "tap", "key": "row-docs"},
  {"t": 0.6, "type": "tap", "key": "layout"},
  {"t": 0.8, "type": "tap", "key": "layout-2c"},
  {"t": 1.0, "type": "type", "text": "echo hi", "every": 0.02},
  {"t": 1.4, "type": "key", "name": "Enter"},
  {"t": 1.5, "type": "scroll", "x": 180, "y": 400, "dy": -600},
  {"t": 2.0, "type": "mark", "name": "end"}
]}
```

- `tap` by **key**: the keys in `--tree` (`elements[].key`). Screens' keys are
  their `key` prop or `<on>-<arg>` (`stop-docs`, `row-docs`); menus are
  `<id>`, `<id>-list`, `<id>-<item>` (see `docs/screens.md`). The core's own:
  `top-bar`, `sidebar`, `panes`, `pane-N`, `pane-N-bar`, `pane-N-close`,
  `terminal-N`, `grid-N`, `notice`, `panes-handle` (and `panes-left-handle`,
  `panes-right-handle`).
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
app covers (`.sbxw-artifacts/daemon-api.md`) from
`fixtures/sandboxes/basic.json`, with a local bash for every terminal.
`--change SECS:NAME=STATUS` (on `start_daemon` in a script) changes a
sandbox behind the app's back.

Against the real daemon on a machine with sbx:

```sh
SBXW_DAEMON=127.0.0.1:7681 SBXW_TOKEN=$(cat ~/.sbxw/run/ui-token) SBXW_SANDBOX=<name> desktop/scripts/shot.sh
```

## Tests

`cd desktop && cargo test`:

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
- `theme`: every colour token equals `assets/app.css`'s.
- `client`: SSE parsing, unknown sandbox fields kept, readable reasons.
- `tests/link.rs` (real stand-in process): a change behind the app reaches
  the list; a restarted daemon with a new token in the token file is found
  again; no daemon gives the one-line message.
- `tests/pump.rs`: key-to-echo through the terminal socket thread against an
  in-process echo server, idle wake-ups, close on drop (`cargo test --release
  --test pump -- --nocapture` prints the numbers).
- `terminal` (thread desktop-terminal): key bytes; `full_screen_redraw_cost`
  is `--ignored` (a benchmark).
