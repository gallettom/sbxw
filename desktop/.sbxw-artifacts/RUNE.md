# sbxw desktop on Rune: spike report

Second try of the native client: the GPUI layer of the first spike
(`SPIKE.md`, commit 1416916) is replaced by
[Rune](https://github.com/anomalyco/rune). The Luau host (`src/luau.rs`),
the daemon client (`src/client.rs`, still the only module that talks to the
daemon) and the stand-in daemon (`src/bin/standin.rs`) are kept. Verified on
Linux (Debian trixie, aarch64, 10 cores, 11 GB) inside an sbx sandbox, with no
GPU and no display.

| Proof | Verdict |
|-------|---------|
| A. Window and headless: builds on Linux, PNG, element tree and a scripted click through Rune's own headless mode, no Xvfb, no xdotool | **PASSED** |
| B. Luau screen drawn with Rune elements, hot reload keeps state, broken file keeps the last good view with the error | **PASSED** (headless reload is driven by script marks; the real file watcher is covered by a unit test, see below) |
| C. Terminal: monospace grid, per-cell colours, cursor, keys with modifiers, resize, against the stand-in daemon | **PASSED** for everything listed; **PARTIAL** as a whole terminal, same gaps as the GPUI spike (no scrollback, selection, mouse, IME) and the real daemon not exercised |

The open question of the plan (can Rune draw a terminal?) has a clear answer:
**yes, and it has a purpose-built element for it**. `rune::grid` and
`rune::GridRow` (`crates/rune/src/el/grid.rs`) draw a terminal's screen as one
element: per row, the text with colour, weight, italic and underline spans,
background runs, wide cells pinned to their columns, an optional vector layer
(box drawing, curly underlines), and only the rows that changed are shaped and
painted again. It is not in the README yet: it landed on 2026-11-12 (#813,
#835). Moonframe's Rune build (`crates/moonframe-rune/src/terminal.rs`, read
at ed3253f) predates it and draws one `rich` text per row; it gets its PTYs
from `moonframe-mux` (herdr-core) and its emulator from `moonframe-terminal`
(alacritty). We need neither: our PTY is the daemon's WebSocket and our
emulator is `alacritty_terminal`, as in the GPUI spike.

## Rune: what was pinned and what it needs

- **Rev:** `09ab92a5c83b906adb6fbf92637323b49e86d7ac` on `dev`
  (2026-11-13, "Declare the wasm32 target in rust-toolchain.toml (r420)").
  Pinned in `desktop/Cargo.toml` for `rune` and for the four crates Rune
  patches (`winit`, `parley_engine`, `wgpu-hal`, `accesskit_windows`): a
  dependency's `[patch]` does not reach the app, so the app must repeat it from
  the same rev (crates/rune/README.md, "Depending on Rune"). A bump must move
  all five together and copy any new entry of Rune's root `[patch.crates-io]`.
- **Feature:** `tooling`, so release builds keep the headless flags (debug
  builds always have them). Default features stay on (accessibility, dbus with
  tokio and zbus on Linux, embedded fonts, Metal backend on macOS, ogg audio,
  svg); trimming them is a later size optimisation.
- **Rust:** Rune pins `1.99.1` in its `rust-toolchain.toml`; `desktop/` now
  pins the same (installed with `rustup toolchain install 1.99.1 --profile
  minimal`). It also builds with our 1.100.0 (one measurement run used
  `+stable` by mistake and built fine, in the same time).
- **System packages (Linux):** to build, `libxkbcommon-dev libwayland-dev`
  (plus `build-essential pkg-config`; Luau compiles from source through mlua).
  To render headless, a Vulkan driver: with no GPU, `mesa-vulkan-drivers`
  (llvmpipe). Windowed runs also need `libxkbcommon-x11-0`. The release binary
  links only `libstdc++`, `libxkbcommon`, `libgcc_s`, `libm`, `libc`
  (GPUI's linked 12 libraries).
- **No CPU fallback in headless:** with the Vulkan ICD hidden
  (`VK_ICD_FILENAMES=/nonexistent.json`) the run prints `no GPU adapter: No
  suitable graphics adapter found; ... vulkan drivers/libraries could not be
  loaded, ... gl support not compiled in` and writes no PNG. CI needs Mesa's
  llvmpipe (or a GPU).
- **Fonts:** UI text is Rune's embedded Inter. The terminal uses Rune's mono
  slot set to `ui-monospace` (fontconfig `monospace`, here DejaVu Sans Mono;
  SF Mono on macOS; Cascadia Mono or Consolas on Windows), followed by CJK
  faces by name (`terminal::MONO_FAMILIES`). Without that list, wide CJK cells
  drew nothing on Linux: Rune did not find `Droid Sans Fallback` on its own.

### How Rune's headless mode works

Every Rune app takes these flags (crates/rune/README.md, "Agent tooling"): the
same renderer draws offscreen through wgpu, on a **virtual clock** (60 fps by
default), with no window and no display server.

- `--snap out.png [--at T] [--size WxH] [--scale S]`: one PNG at time `T`.
- `--tree tree.json [--at T]`: every element with its key, path, role, label,
  text, presented rect, colours, hover/press state, plus the accessibility tree.
- `--script s.json`: timed input: `tap` / `down` / `up` / `drag` at `x,y` or
  **at an element by key**, `key` (with `shift`, `ctrl`, `alt`, `cmd`),
  `type` (one key per character, every 35 ms by default, `every` to change),
  `paste`, `wheel`, window `resize`, and `mark` (a named time; `--at
  mark:end+0.5`).
- `--out frames/`, `--strip strip.png --crop x,y,w,h`, `--video out.mp4`
  (ffmpeg), `--trace`, `--first-pixels`.
- The app reads the run with `rune::tooling::headless_run()` (script events,
  marks). Flags Rune does not know end up in `rune::extra_args()`, which is
  how `--daemon`, `--token`, `--screen` still work.

Two consequences shaped the code:

1. **The virtual clock does not wait for the shell.** Frames run as fast as
   the CPU allows, so a PNG would show the terminal before bash answered. In a
   headless run (`headless_run().is_some()`), `TerminalState::poll` waits (real
   time) for output when a key is waiting for its echo, then until the output
   has been quiet for 100 ms, capped at 1.5 s per frame (`src/terminal.rs`).
   Rune warns about it in debug builds (`poll blocked 102 ms`), which is
   expected here. Windowed runs never block.
2. **The virtual clock cannot wait for a file edit either.** For proof B the
   app takes script marks named `screen:<path>`: at that time it loads `<path>`
   as the screen, through the same reload code a real edit goes through
   (`ScreenState::load_file`, then `load`). Real edits are picked up by a
   watcher thread (no file I/O in the frame loop, as Rune asks) and tested by
   `screen::tests::the_watcher_reloads_a_real_edit_and_keeps_state`.

## Proof A: window and headless. PASSED

```
$ desktop/scripts/shot.sh shots/shot.png --type 'echo hello from Rune; uname -sm | tr a-z A-Z' --tree shots/tree.json
shots/shot.png
shots/tree.json
```

`scripts/shot.sh` builds, starts the stand-in daemon on a free port (or uses
`SBXW_DAEMON` + `SBXW_TOKEN`), and runs the app once with
`--snap ... --at ... [--script ...] [--tree ...]`. `--type` writes a small
Rune script (type the text, then Enter); `--script` replays any script.
No Xvfb, no xdotool, no `DISPLAY` (the script unsets it). `SBXW_PROFILE=release`
uses the release build.

- `.sbxw-artifacts/rune/a-shot.png` (1280x800 at scale 1; I looked at it):
  dark window. Left, the "Sandboxes" panel: "from daemon at 127.0.0.1:59591",
  a bordered list of three rows separated by lines (`web-app running`,
  `data-pipeline stopped`, `docs running`, status in green or grey, a grey
  "select" button each), "selected: none, clicks: 0", the screen file path and
  "(loads: 1)". Right, the terminal: status "terminal 111x42 connected", a
  green check mark and the bold teal `agent@sbxw-2:~$` prompt, the typed line,
  `hello from Rune`, `LINUX AARCH64`, a new prompt and a block cursor.
- **Element tree:** `shots/tree.json`, 32 elements; excerpt of the keyed ones
  in `.sbxw-artifacts/rune/a-tree-excerpt.json`, for example
  `{"key": "btn-select-docs", "role": "button", "label": "select", "rect":
  [128.0, 112.0, 56.0, 17.5]}` and `{"key": "grid", "role": "terminal",
  "rect": [328.0, 28.0, 935.59, 756.0]}`. Screen buttons get keys from their
  action and argument (`btn-<action>-<arg>`), so a script can press them by
  name.
- **Scripted click:** `scripts/headless/click-and-type.json` taps
  `btn-select-docs` twice; the PNG shows `> docs` in bold and "selected: docs,
  clicks: 2" (see `c-terminal-typed.png` below and every proof B image).
- **One gap for agents:** the tree names the terminal element and its rect,
  and the accessibility tree has a `terminal` node, but neither carries the
  terminal's lines. The app adds `--dump-text <file>`, which writes the
  terminal's size and text whenever it changes, for an agent that wants to
  check output without reading pixels
  (`.sbxw-artifacts/rune/c-terminal-resized.txt` is one).
- **A real window also works** (not required, checked for comparison): under
  Xvfb the Rune window draws on its own, without GPUI's unmap/remap
  workaround. `.sbxw-artifacts/rune/c-window-xvfb-shifted-keys.png`.

## Proof B: the Luau screen. PASSED

```
$ desktop/scripts/demo-reload.sh
.../shots/reload-1-clicked.png ... reload-4-fixed.png, reload-strip.png
screen sandboxes.luau loaded (reload #1)
screen edited.luau loaded (reload #2)
screen broken.luau: syntax error: broken.luau:37: Expected <eof>, got 'return'
screen fixed.luau loaded (reload #4)
```

(7.7 s for the five headless runs, warm debug build.)

The screen file and the element set are unchanged from the GPUI spike
(`text`, `row`, `column`, `list`, `button`); `src/screen.rs` maps them to Rune
elements (`text`, `row`, `col`, a bordered `col` with 1 pt separators, a keyed
element with `role("button")`, hover colour and `on_press`). The sandbox
limits are unchanged (`cargo test --lib`: 9 passed, the 8 GPUI-spike tests
plus the watcher test; `luau.rs` was not touched).

`.sbxw-artifacts/rune/b-reload-strip.png` is Rune's own `--strip`: one run,
four frames 0.5 s apart, the left panel cropped. I looked at it, left to
right: (1) `> docs` marked, "selected: docs, clicks: 2", "(loads: 1)";
(2) the yellow title "Sandboxes (edited live)" and the new line "a line added
while running", still "selected: docs, clicks: 2", "(loads: 2)"; (3) a red
banner "screen error, showing the last good view: syntax error:
broken.luau:37: Expected <eof>, got 'return'" (wrapped on three lines) above
the unchanged edited view, "(loads: 3)"; (4) the banner gone, same state,
"(loads: 4)". The same four states are in `b-reload-1-clicked.png` to
`b-reload-4-fixed.png` (full window).

## Proof C: the terminal. PASSED (with the gaps listed)

`src/terminal.rs`: `alacritty_terminal 0.26` parses and keeps the grid, as
before. After each batch of output, every screen row is hashed (characters,
flags, colours, cursor) and only changed rows become a new `Rc<GridRow>`; an
unchanged row keeps its `Rc`, so Rune neither shapes nor paints it again. Text
runs become `Span`s (colour, bold as weight 700, italic, underline),
backgrounds become `fill` runs, wide characters become pinned `cell`s, the
block cursor is the cell drawn in reverse, and `grid_caret` gives screen
readers the cursor. The element's `on_layout` gives its size, from which
cols/rows are computed; a change resizes the grid and sends the daemon's
`resize` frame. Keys come through the terminal element's `on_key` (focused)
and the window's `on_key` (nothing focused), so Tab and Escape reach the shell.
`key_bytes` maps Rune keys to xterm bytes: Ctrl+letter and Ctrl+[\]^_ to
control bytes, Alt as an ESC prefix, arrows/Home/End with xterm modifier
parameters (`\e[1;5C` for Ctrl+Right) and application cursor mode, Shift+Tab,
Insert/Delete/PageUp/PageDown, F1 to F12; ⌘ shortcuts on macOS are left to
the app (unit tested).

Evidence, all against the stand-in daemon:

- `scripts/headless/click-and-type.json` (headless, `--snap ... --at
  mark:end+0.3`), image `.sbxw-artifacts/rune/c-terminal-typed.png`. I looked
  at it: `echo 'Hello (World) | <ok> {x} ~@#$%^&*_+' | tr a-z A-Z` typed
  exactly, answered `HELLO (WORLD) | <OK> {X} ~@#$%^&*_+`; a `printf` line
  wrapped correctly at column 111 onto the next row; its output `bold red`
  (bold, red), `green bg` (black on a green background), `underline`
  (underlined), `256-colour` (orange, colour 208), `wide:日本語|` with each
  ideograph on two columns and the `|` after them in place; `echo shift
  keys:a9A` (see shifted keys below); `sleep 30` stopped by a scripted Ctrl+C
  (`^C`, then the red `✗ 130` prompt); the block cursor after the last prompt.
- `scripts/headless/resize.json` (a scripted window resize by -480 x -300
  points over 0.3 s), image `.sbxw-artifacts/rune/c-terminal-resized.png` and
  text `c-terminal-resized.txt`: `stty size` says `42 111` before and `25 55`
  after, `seq 1 40 | paste -sd ' '` wraps at column 55, the status line reads
  "terminal 55x25". The window shrinks into the top-left corner of the fixed
  canvas, as Rune documents. The app log shows 17 intermediate sizes (one per
  frame of the drag, each sent to the daemon; a real build should debounce).
- **Shifted keys.** In a real Rune window under Xvfb, typed through xdotool
  (XTEST), shifted characters arrive shifted: `Text("H")`, `Text("<")`,
  `Text("(")` with `shift: true`, and the PNG shows `echo Hello World | <x>
  (y) {z}` and `echo shiftA(` exactly (the bash syntax errors come from the
  test text itself). The GPUI spike lost Shift in exactly this setup. Through
  **Rune's scripted input**, `type` sends each character as typed (all of
  `(`, `|`, `<`, `{`, `~`, `@`, `#`, `$`, `%`, `^`, `&`, `*`, `_`, `+`, `A`
  reached bash), but a `key` event with `"shift": true` only sets the modifier:
  `{"type": "key", "name": "a", "shift": true}` arrives as `Text("a")` with
  `shift: true`, so it types `a`, and `"9"` types `9` (hence "shift keys:a9A").
  That is Rune's design (a script has no keyboard layout; a real window gets
  the shifted text from the OS), not a terminal bug. Scripts that test shifted
  characters must name the character (`"name": "A"`, or `type`).
- **Speed.** `cargo test --release --lib full_screen -- --ignored --nocapture`
  redraws a 111x42 screen where every cell changes colour and character every
  frame, 120 frames at scale 2: parse and row rebuild p50 0.82 ms (p95 0.90),
  Rune layout and paint p50 0.23 ms (p95 0.34), 4,663 instances. That is CPU
  only (no GPU raster, upload or presentation). In the headless runs, frames at
  rest cost about 4 ms p50 of CPU in debug builds.

Still missing (none of it blocked by Rune; the GPUI spike lacked the same):
scrollback and its wheel scrolling, selection and copy, mouse reporting, IME
and dead keys, paste, bracketed paste, bell, cursor shapes other than the
block, the line-drawing vector layer (`GridRow::shape`), reconnect with the
daemon's replay. Rune has the pieces for these (`on_wheel`, `on_drag` with
click counts, `Tx::copy`, `App::on_ime`, `on_physical_input`).

## Comparison with the GPUI spike, same machine

Both measured in this sandbox in the same session: the GPUI spike rebuilt from
commit 1416916 in a throwaway copy (`desktop/.gpui-spike/`, git-ignored, now
deleted), with its own `Cargo.lock`. Build times are `cargo build --bins`
(both binaries) with the registry already downloaded, target directory on the
host mount; "warm" means `src/terminal.rs` touched.

| | GPUI 0.2.2 (spike) | Rune 09ab92a5 |
|---|---|---|
| Cold debug build | 69 s | 70 s |
| Warm debug build | 17 s | 16 s |
| Cold release build | 128 s | 124 s |
| Warm release build | 4.3 s | 14 s |
| Release binary, stripped | 20.2 MB | 33.8 MB (with `tooling`; Rune puts that at about 3.5 MB) |
| Crates built for Linux (`cargo tree -e normal,build`, unique) | 553 | 356 |
| Target dir after debug + release | 6.0 GB | 5.7 GB |
| Shared libraries of the release binary | 12 | 5 |
| Key to first echo byte, window under Xvfb, xdotool | median 4.2 and 4.9 ms (two debug runs, 23 keys each, 20 ms per key) | median 8.4 ms (release, 98 keys, 40 ms per key; min 4.1, max 30.7) |
| One screenshot, warm build, a typed line included | 6.2 s (`shot.sh`, Xvfb + xdotool, debug) | 7.5 s debug, 6.8 s release (`shot.sh`, headless; about 4.6 s of it is the 100 ms quiet wait after each of the 46 keys) |
| One screenshot, nothing typed | not measured | 2.5 s (`--snap`, debug, daemon connection included) |
| Needs a display server for screenshots | yes (Xvfb, plus an unmap/remap workaround) | no |
| Element tree for agents | no | yes (`--tree`, without the terminal's text) |
| Scripted input | xdotool (XTEST) | Rune scripts: taps by element key, keys, typing, resize, on a virtual clock |
| Shifted keys from XTEST in a window | lost | correct |
| Terminal drawing | one shaped text line per row, rebuilt every frame | `rune::grid`, only changed rows shaped and painted |

Latency notes: both numbers run from the key leaving the UI to the first
output byte reaching the UI thread, over loopback through the stand-in and
bash. To compare like with like, the GPUI copy was patched to time from the
first unanswered key (the spike's code reset the timer on every key, which
gave a misleading 0.5 ms median). Both include up to 10 ms of socket polling
(5 ms read timeout in the client thread and 5 ms in the stand-in), so the gap
is within that noise; time to pixels was not measured in either. Rune's warm
release build is slower because `src/terminal.rs` lives in the library that
both binaries link, and Rune's generic code is monomorphised in our crate.

What each renderer lacks for this app, as seen in these spikes:

- **GPUI:** no headless renderer (Xvfb only, with a window-mapping bug), no
  element tree, XTEST Shift lost, a `libc` pin to build, the crates.io release
  is a snapshot of Zed's monorepo. Its strengths: Zed proves it on real
  terminals and editors, and it is a published crate.
- **Rune:** pre-release with a fast-moving API (see risks); the terminal
  element is one day old and undocumented; `--tree` does not carry the
  terminal's text; headless needs a Vulkan driver; scripted `key` events do
  not apply Shift to the text; the release binary is larger with `tooling`.

## What is unverified

- **The real sbxw daemon.** As in the GPUI spike, only the stand-in was used
  (no `sbx` or `sbxw` here, and the host daemon port needs a firewall
  approval that was not asked for). `client.rs` is unchanged apart from the
  wake callback, so the first check on a machine with sbx is still
  `SBXW_DAEMON=127.0.0.1:<port> SBXW_TOKEN=$(cat <sbxw home>/run/ui-token) SBXW_SANDBOX=<name> desktop/scripts/shot.sh`.
- **macOS and Windows:** not built or run. Rune targets both (Metal by
  default on macOS, wgpu on Windows; its README notes the Windows GNU
  toolchain needs MinGW binutils). The CJK font names in `MONO_FAMILIES` for
  those systems are guesses.
- **Wayland, and a real X11 desktop with a GPU:** only Xvfb with llvmpipe.
- **A real keyboard:** only XTEST and Rune scripts. IME, dead keys and AltGr
  were not tested.
- **Time to pixels**, and frame times on a real GPU under heavy output.

## Risks of depending on Rune

1. **Pre-release, git only.** No crates.io release, no semver. We pin a `dev`
   rev (never rewritten, so builds stay reproducible) in five git entries.
   Every bump is a manual review.
2. **Very fast moving.** The public history starts on 2026-10-22: 2,320
   commits, 737 in the last 7 days, 647 unreleased changelog entries. Expect
   breaking API changes between any two revs; plan bumps as small, deliberate
   tasks with the headless shots as the regression check.
3. **No pull requests.** "We don't take pull requests": bugs go in as issues,
   which their agents usually turn around fast. We cannot upstream a fix
   ourselves; a blocking bug means an issue and waiting, or a fork.
4. **The terminal element is new.** `grid` landed on 2026-11-12 and is not in
   the README. It may change shape soon; it is also exactly what we need.
5. **Patched dependencies.** Rune patches winit, parley_engine, wgpu-hal and
   accesskit_windows; the app must mirror that table on every bump or it
   silently builds against stock crates.
6. **Toolchain pin.** Rune pins Rust 1.99.1 (ours is 1.100.0); `desktop/`
   pins 1.99.1 to match. If Rune moves ahead of the root workspace's
   toolchain, `desktop/` follows Rune, not the root.
7. **Headless needs Vulkan.** CI images need `mesa-vulkan-drivers`; there is
   no CPU raster fallback compiled in.

## Recommendation: splitting the full build

Same shape as the GPUI recommendation, with Rune's tooling as the shared test
harness. Every thread proves its work with `scripts/shot.sh` (or a Rune script
under `scripts/headless/`) plus `--tree` and `--dump-text`, so the lead can
review PNGs and JSON without running the app.

1. **Core (serial, first, about a week):** app shell and window, the `Daemon`
   trait with the whole HTTP surface the web UI uses, the Luau element set
   frozen and extended (`input`, `scroll`, `icon`, `badge`, `split`, styles),
   screen intents (open a terminal, call a daemon action), the headless
   harness (shot.sh, scripts, `screen:` marks, fixtures instead of the
   stand-in where possible), CI with llvmpipe, a Rune bump procedure (five
   pins, the patch table, rerun all scripted shots and compare).
2. **Terminal (parallel):** scrollback and wheel, selection and copy, mouse
   modes, IME and paste, bracketed paste, cursor shapes, box drawing through
   `GridRow::shape`, resize debounce, reconnect with replay, the agent and
   thread modes. Needs only `TerminalConn`. File a Rune issue asking for the
   grid's lines in `--tree` (we cannot send a pull request).
3. **Screens (parallel, one thread each):** sandbox list and actions; threads
   and plans board; approvals; artifacts and files; settings. Each needs only
   the frozen element set, a JSON fixture of its daemon data and a Rune script.
4. **Platform (parallel, one owner per OS with that machine):** macOS build,
   signing and notarisation, checking Metal and SF Mono; Windows build and
   installer, Cascadia Mono, the MSVC toolchain; Wayland on Linux.
5. **Later:** audio (the voice concierge), tray and notifications (Rune has
   `Tray` and `rune-notify`).

Gate before step 3: proof C against a real daemon, and `shot.sh` plus one
windowed run on a real desktop per OS.

## Layout

```
desktop/                     own [workspace]; the root build never sees it
  Cargo.toml, Cargo.lock     exact pins: rune (git rev) and its 4 patches, alacritty_terminal, mlua, tungstenite
  rust-toolchain.toml        1.99.1, as Rune
  src/client.rs              the ONLY module that talks to the daemon (now wakes the UI through a callback)
  src/terminal.rs            TerminalState: alacritty_terminal grid to rune::GridRow rows, keys to xterm bytes
  src/luau.rs                LuauHost, unchanged
  src/screen.rs              ScreenState: watcher thread, reload, Node tree to Rune elements
  src/main.rs                the window: AppSpec (init, setup, poll, on_key, view), args after Rune's flags
  src/bin/standin.rs         stand-in daemon (now gives the shell a UTF-8 locale when none is set)
  scripts/shot.sh            proof A: one command, build + headless PNG (+ tree)
  scripts/demo-reload.sh     proof B: four headless PNGs and a strip
  scripts/headless/*.json    Rune scripts: click-and-type, resize
  .sbxw-artifacts/RUNE.md    this report; rune/ holds the images it cites
  .sbxw-artifacts/SPIKE.md   the GPUI report, kept as history (its images beside it)
```

One fix outside the renderer: the stand-in started bash with no locale
(`LANG` empty, so POSIX), and bash counted the prompt's `✓` as three columns
and redrew long lines in the wrong place. It now sets `LANG=C.UTF-8` when
`LANG` is unset; a real sandbox sets its own.
