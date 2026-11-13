# sbxw desktop: spike report

A native client of the sbxw daemon in Rust with GPUI, screens in Luau. This
spike checks four points before the full build is split into parallel
threads. Verified on Linux (Debian trixie, aarch64, 10 cores) inside an sbx
sandbox, with no GPU and no display.

| # | Proof | Verdict |
|---|-------|---------|
| 1 | GPUI window builds and opens on Linux | PASSED (one GPUI bug to work around, under Xvfb only) |
| 2 | Real terminal fed by the daemon's terminal WebSocket | PARTIAL: works end to end against a **stand-in** daemon; the real daemon was not exercised |
| 3 | Luau screen, hot reload that keeps state, broken file keeps last good view, sandbox limits | PASSED |
| 4 | Automatic screenshot without a display, one command | PASSED |

## Layout

```
desktop/                     own [workspace]; the root build never sees it
  Cargo.toml, Cargo.lock     exact pins for gpui, alacritty_terminal, mlua, tungstenite
  src/client.rs              the ONLY module that talks to the daemon: trait Daemon + HttpDaemon
  src/terminal.rs            TerminalView: alacritty_terminal grid + parser, drawn with GPUI, keys out
  src/luau.rs                LuauHost: sandboxed VM, memory cap, instruction budget, Node tree
  src/screen.rs              ScreenView: draws a Node tree, watches the file, reloads, keeps state
  src/main.rs                the window: screen panel on the left, terminal on the right
  src/bin/standin.rs         stand-in daemon (same routes and protocol, local shell)
  screens/sandboxes.luau     the sandbox list screen
  scripts/shot.sh            proof 4: one command, build + run headless + PNG
  scripts/demo-reload.sh     proof 3 in the running app, four PNGs
  scripts/lib.sh             shared by the two scripts
  .sbxw-artifacts/           this report and the screenshots it cites
```

The rest of the app sees the daemon only through `client::Daemon`
(`sandboxes()`, `open_terminal()`), so a later transport (a Unix socket, a
different protocol) replaces one file.

## Proof 1: a GPUI window on Linux. PASSED

- **Crate:** the published `gpui = "=0.2.2"` from crates.io (2025-10-22,
  Apache-2.0). No git dependency was needed.
- **Snag 1, build:** `gpui 0.2.2` pulls `xattr 0.2.3` (through
  `gpui_http_client` and `zed-async-tar`), which does not compile against
  `libc >= 0.2.187` (`ENOATTR` was removed for Linux). Fixed by pinning
  `libc = 0.2.186` in `Cargo.lock` (`cargo update -p libc --precise 0.2.186`).
  Any `cargo update` must keep that pin until gpui moves on.
- **Snag 2, Xvfb only:** with no window manager, the window stays black after
  the first frame. GPUI 0.2.2 maps its X11 window before registering it, so
  the one `MapNotify` is dropped and its refresh loop never starts (found with
  a patched local copy of gpui that logged `update_refresh_loop`; the copy was
  removed). With a window manager, later events start the loop. The scripts
  unmap and remap the window once it exists (`xdotool windowunmap/windowmap`),
  after which it renders continuously (471 frames in 10 s in the test).
  To report upstream. Not seen on a real desktop here, because none exists.
- **Rendering:** Vulkan through `blade-graphics`, on Mesa's software
  `llvmpipe` here. With a real GPU it would use that GPU's Vulkan driver.
- **Rust:** builds with rustc 1.100.0. `mlua 0.12` needs >= 1.88; gpui sets
  no MSRV.
- **System packages installed** (`sudo apt-get install`):
  `build-essential pkg-config cmake clang libxkbcommon-dev libxkbcommon-x11-dev
  libwayland-dev libvulkan-dev libfontconfig-dev libfreetype-dev libx11-xcb-dev
  libxcb1-dev libxcb-xkb-dev libx11-dev libzstd-dev libssl-dev` (to build) and
  `mesa-vulkan-drivers vulkan-tools xvfb x11-apps imagemagick xdotool
  fonts-dejavu-core gdb` (to run headless, capture, debug). `cmake`, `clang`,
  `libssl-dev`, `vulkan-tools`, `x11-apps` and `gdb` were precautions or
  debugging aids; a minimal list for CI should be confirmed by a clean
  container build.
- **Build times and size:** see "Numbers" below.

## Proof 2: a real terminal. PARTIAL (stand-in daemon, not the real one)

**The real sbxw daemon was not exercised.** This sandbox has no `sbx` or
`sbxw`, the host daemon port (`localhost:7300`) answers "Approval required"
through the sandbox firewall, and its UI token lives on the host. I did not
ask for that approval: the spike rules say not to work around the network.

What was built and checked:

- `src/bin/standin.rs` speaks the daemon's protocol, read from:
  - `crates/daemon/src/http/mod.rs:42`: route `/ws`.
  - `crates/daemon/src/http/ws.rs:19-28`: query `sandbox`, `mode`
    (`bash`, `monitor`, `thread`, `kit-add`, default agent), `thread`, `repo`,
    `model`, `checkout`.
  - `ws.rs:85-111`: daemon to client: binary frames carry PTY bytes (a
    replay first); text frames carry an error line or
    `{"type":"attention","sandbox":...}`.
  - `ws.rs:112-131`: client to daemon: binary frames are keys; text frames
    are JSON `{"type":"resize","cols":N,"rows":N}`.
  - `crates/daemon/src/http/guard.rs:1-17,154-171`: native clients send no
    `Origin`, and send `Authorization: Bearer <ui token>`. `token.rs:66-73`:
    `/api/health?nonce=` answers `sha256(token || nonce)`, which
    `HttpDaemon::connect` checks before it sends the token anywhere.
  - `crates/daemon/src/http/ui.rs:55,279` and
    `crates/daemon/src/svc/sandboxes.rs:30`: `GET /api/sandboxes` returns an
    array of `{name, agent, status, workspace, ...}`.
- The stand-in does not keep the PTY across reconnects, replay the last
  256 KB, check Host/Origin, or support threads/monitor. A local `bash` replaces
  `sbx exec`.
- `src/terminal.rs`: `alacritty_terminal 0.26` holds the grid and parses
  (colours, cursor, application cursor mode, answers to terminal queries go
  back as `PtyWrite`). GPUI draws one shaped line per row with colour runs.
  Keys map to xterm bytes (`key_bytes`, unit tested). The window size gives
  cols/rows, and a change sends a `resize` frame.

Evidence: `scripts/shot.sh .sbxw-artifacts/p2-terminal.png --type 'echo hello from the desktop client; uname -sm; ls /usr | head -n 3'`
gives `.sbxw-artifacts/p2-terminal.png`. Stand-in log: `standin: terminal opened
(sandbox=web-app&mode=bash)`. See "What the screenshots show".

**Input latency** (key sent on the socket to the first output byte back, over
loopback through the stand-in and bash echo, logged by the app): over the
33 keys of the screenshot run, min 0.0 ms, median 4.3 ms, max 6.5 ms. Part of that is the 5 ms read-timeout polling
in the client's socket thread (see Risks). Time to pixels was not measured.

**Not verified:** shifted characters. Typed through `xdotool` (XTEST) on
Xvfb, every shifted key arrives unshifted (`A` gives `a`, `(` gives `9`).
GPUI reads modifiers from its xkb state, and that state never sees the XTEST
Shift on Xvfb. Zed uses the same X11 code on real keyboards, so I expect a
real keyboard to work, but I could not check it here. The test scripts avoid
shifted characters.

## Proof 3: a Luau screen with hot reload. PASSED

- `mlua =0.12.2`, feature `luau` (Luau is vendored and compiled from source,
  so no system Lua is needed).
- Libraries opened: `string table math utf8 bit32` only (no `io`, no `os`,
  no `package`, no `debug`, no `coroutine`, no `buffer`). `require`,
  `loadstring`, `load`, `dofile`, `loadfile`, `getfenv`, `setfenv` and
  `collectgarbage` are removed, then `lua.sandbox(true)` makes the globals
  read-only.
- Memory cap: `set_memory_limit` (16 MB by default).
- Instruction budget: Luau's interrupt hook (fired on calls and loop
  back-edges) counts interrupts per call into the script (200,000) and checks
  a wall deadline (50 ms). Over budget, the call fails with an error, and the
  VM stays usable.
- Elements: `text`, `row`, `column`, `button`, `list`, built by a small
  prelude as plain tables and turned into a Rust `Node` enum (an unknown kind
  is an error; depth is capped at 32). A screen returns
  `{ init, render(state, data), actions }`. `state` lives in the VM and
  survives reloads; buttons call `actions[on](state, arg)`. Screens cannot
  call the daemon: the core passes `data` in.
- The view polls the file's mtime every 150 ms (no `notify` dependency). On
  change it reloads; if the load or the render fails, the last good tree stays
  and a red banner shows the error.

Tests (`cargo test --lib`, 8 passed), quoted from `-- --nocapture`:

```
while true do end
  -> runtime error: instruction budget exceeded (200001 interrupts) (after 11.958875ms)
string.rep 8 MB under a 4 MB cap
  -> memory error: not enough memory
return io.open('/etc/passwd')      -> runtime error: eval:1: attempt to index nil with 'open'
return os.execute('id')            -> runtime error: eval:1: attempt to index nil with 'execute'
return os.getenv('HOME')           -> runtime error: eval:1: attempt to index nil with 'getenv'
return require('x')                -> runtime error: eval:1: attempt to call a nil value
return loadstring('return 1')      -> runtime error: eval:1: attempt to call a nil value
return getfenv(1)                  -> runtime error: eval:1: attempt to call a nil value
return dofile('/etc/passwd')       -> runtime error: eval:1: attempt to call a nil value
test luau::tests::an_endless_render_is_stopped_and_the_host_survives ... ok
test luau::tests::globals_are_read_only ... ok
test luau::tests::renders_and_keeps_state_across_reload ... ok
test luau::tests::a_broken_file_keeps_the_last_good_version ... ok
```

In the running app: `scripts/demo-reload.sh` clicks "select" on `docs`
twice, edits the file, breaks it, then fixes it. It works on a temporary copy
of the screen file. Images `.sbxw-artifacts/reload-1-clicked.png` to
`reload-4-fixed.png`, side by side in `.sbxw-artifacts/reload-strip.png`. App log:
`loaded (reload #1)`, `#2`, then `#4` (#3 was the broken file).

## Proof 4: automatic screenshot. PASSED

```
desktop/scripts/shot.sh [out.png] [--type "text"] [--wait s]
```

It builds, starts Xvfb on a free display, starts the stand-in (or uses
`SBXW_DAEMON` + `SBXW_TOKEN`), starts the app with `WAYLAND_DISPLAY` unset
(GPUI prefers Wayland when it is set), remaps the window (snag 2 above),
optionally types a line, and saves the X screen with ImageMagick `import`.
It prints the PNG path (default `desktop/shots/shot.png`, git-ignored). Logs
go next to the image (`app.log`, `standin.log`). It takes about 25 s with a
warm build, mostly the fixed waits plus GPUI start-up and shader compilation
on llvmpipe.

GPUI 0.2.2 has no public offscreen renderer, so Xvfb is the route. A headless
Wayland compositor (for example sway with `WLR_BACKENDS=headless` and `grim`)
was not tried.

## What the screenshots show (I looked at each one)

- `.sbxw-artifacts/p2-terminal.png`: dark window, 1280x800. Left: a panel
  titled "Sandboxes", "from daemon at 127.0.0.1:74xx", a bordered list of
  three rows (`web-app running`, `data-pipeline stopped`, `docs running`,
  statuses in green or grey, each with a grey "select" button), "selected:
  none, clicks: 0", and the screen file path with "(loads: 1)". Right: the
  terminal, status line "terminal 111x42 connected", a bash prompt with a
  green check mark and a bold coloured `agent@sbxw-2:~$`, then the typed
  line as `echo hello from the desktop client; uname -sm; ls /usr \ head -n 3`.
  The `|` (Shift+`\`) arrived as `\`: that is the XTEST Shift limitation
  described above. The output follows: `hello from the desktop client`,
  `Linux aarch64`, two `ls: cannot access` errors (for ` head` and `3`), the
  `ls -l` style listing of `/usr` (bin, games, include, lib, ...), then a red
  `✗ 2` prompt with a block cursor. So: keys out, PTY bytes in, colours, bold,
  cursor and resize all work; shifted keys from XTEST do not.
- `.sbxw-artifacts/reload-strip.png`, four crops of the left panel, left to
  right: (1) `> docs` marked and "selected: docs, clicks: 2" after two
  clicks; (2) the yellow title "Sandboxes (edited live)" and the new line "a
  line added while running", still "selected: docs, clicks: 2", "(loads:
  2)"; (3) a red banner "screen error, showing the last good view: syntax
  error: tmp...luau:37: Expected <eof>, got 'return'" above the unchanged
  edited view; (4) the banner gone, same state, "(loads: 4)".

## Numbers

Measured on this sandbox (10 aarch64 cores, 11 GB RAM, target directory on
the host mount, registry already downloaded), `cargo build --bins` in
`desktop/`:

| Build | Time |
|-------|------|
| Cold debug (empty target dir) | 64 s |
| Warm debug (one source file touched) | 15 s |
| Cold release (`opt-level = 3`, no LTO, `strip`) | 96 s |
| Warm release (one source file changed) | 3.6 s |

The very first debug build (gpui only, before the other dependencies were
added) took 88 s while the disk was nearly full; treat build times as roughly
1 to 1.5 minutes cold.

| Artefact | Size |
|----------|------|
| `sbxw` (then `sbxw-desktop`) release, stripped | 20.2 MB |
| `sbxw` (then `sbxw-desktop`) debug | 537 MB |
| `standin` release | 1.1 MB |
| Target directory after debug + release | 6.0 GB |

Release binary's shared libraries (`ldd`): libstdc++, libfreetype, libxcb,
libxcb-xkb, libxkbcommon, libxkbcommon-x11, libz, libbz2, libpng16,
libbrotli, libXau, libXdmcp. Vulkan, Wayland and fontconfig are opened at
run time.

Root build: unchanged. `cargo metadata --no-deps` at the root still lists
only `sbxw-core`, `sbxw-sbx`, `sbxw-daemon` and `sbxw`. `desktop/Cargo.toml`
has its own `[workspace]` table, so the root `Cargo.toml` was not edited
(no `exclude` was needed).

Frame time was not measured. Under llvmpipe the first frame takes about a
second (shader compilation); afterwards the window keeps up with typing.

## Dependencies and licences

Direct: `gpui 0.2.2` (Apache-2.0), `alacritty_terminal 0.26.0` (Apache-2.0),
`mlua 0.12.2` (MIT; Luau itself MIT), `tungstenite 0.30.0` (MIT OR Apache-2.0),
`ureq 2` (MIT OR Apache-2.0), `portable-pty 0.8` (MIT, stand-in only),
`serde`, `serde_json`, `sha2`, `getrandom`, `anyhow`, `futures`, `log`,
`env_logger` (all MIT and/or Apache-2.0).

All 577 crates resolved for Linux (`cargo metadata`), by licence: mostly
MIT and/or Apache-2.0 (about 480), then Unicode-3.0 (18), BSD-2/3 (about 16),
Zlib, ISC, CC0, Unlicense OR MIT. Two to note: `option-ext 0.2.0` is
**MPL-2.0** (file-level copyleft, fine to use unmodified, through `dirs-sys`)
and one crate is `Apache-2.0 OR GPL-2.0-only` (take Apache-2.0). Run a
`cargo deny` licence check in the real build; these numbers came from a
script over `cargo metadata`.

## What is unverified

- **The real sbxw daemon** (see proof 2). First thing to do on a machine with
  sbx: `SBXW_DAEMON=127.0.0.1:<web port> SBXW_TOKEN=$(cat <sbxw home>/run/ui-token) SBXW_SANDBOX=<name> desktop/scripts/shot.sh`.
  Points to check: the replay on connect, the agent mode (the default), the
  thread modes, the `Host` check (`127.0.0.1:<port>` is in the allowed list
  in `guard.rs:38-55`).
- **macOS and Windows**: not built or run. GPUI supports both (Metal on macOS;
  the Windows backend is newer). `portable-pty` (stand-in only) and
  `alacritty_terminal` are cross-platform. The `libc` pin is Unix-only and
  has no effect there.
- **A real X11 or Wayland desktop with a GPU**: only Xvfb with llvmpipe here.
  The Wayland backend was not run.
- **Shifted keys** from a real keyboard, IME, dead keys, paste, mouse in the
  terminal, scrollback, selection, the bell. None of these is done.
- **Time to pixels** and frame times under load (a full-screen `htop`, a
  large `cat`).

## Risks found

1. **gpui on crates.io is a snapshot.** `0.2.2` is from 2025-10. It already
   needs a `libc` pin to build, and it has an X11 start-up bug. Zed develops
   gpui in its monorepo with no stability promise. Plan to move to a pinned
   git commit when a fix is needed. github.com was not tried from here; it may
   need firewall approval.
2. **Heavy dependency tree:** 577 crates and 6 GB of build output (see
   Numbers). This sandbox's 20 GB disk ran out once during the first build.
   CI needs caching.
3. **Terminal drawing is naive:** one shaped text line per row, rebuilt each
   frame, cursor drawn as a colour swap. Fine for a prompt; heavy output will
   need a custom element that paints cell backgrounds and glyph runs, and
   only for damaged rows (alacritty exposes damage). Expect a few days of work.
4. **Socket thread polls:** one thread per terminal reads with a 5 ms timeout,
   then sends what is queued. It is simple and works, but it adds up to 5 ms
   to key latency and wakes the thread 200 times a second. Use a split async
   socket (tokio-tungstenite) or a blocking reader plus a separate writer in
   the real build.
5. **Headless testing has limits:** it needs the remap workaround, the XTEST
   Shift does not reach GPUI, and llvmpipe is slow to start. Good enough for an
   agent to see a layout; not a substitute for a check on a real desktop.
6. **Luau budget is per call, not per frame.** A screen that renders a very
   large list could hit 50 ms on a slow machine and show an error. Tune the
   limits once real screens exist. `render` runs on every data or state
   change, on the UI thread.

## Recommendation: splitting the full build

Shared core first (one thread, about a week), everything else on top of it
in parallel:

1. **Core (serial, first):** window and app shell, the `Daemon` trait with
   the whole HTTP surface the web UI uses (sandboxes, threads, plans,
   approvals, stream events), the Luau host and a frozen element set (add
   `input`, `scroll`, `icon`, `badge`, `split`, styles), the screen loader
   with reload, a pattern for screens to emit intents (open a terminal, call a
   daemon action) that the core carries out, the headless screenshot command,
   the `libc` pin and CI.
2. **Terminal element (parallel):** custom painting, damage tracking,
   scrollback, selection and copy/paste, mouse modes, IME, reconnect with
   replay, the agent and thread modes. Needs only the `TerminalConn` interface.
3. **Screens (parallel, one thread each, Luau files plus fixtures):** sandbox
   list and actions (up/down/create); threads and plans board; approvals;
   artifacts and files; settings. Each one only needs the core's element set
   and a JSON fixture of its daemon data, so they can start as soon as the
   element set is frozen.
4. **Platform (parallel):** macOS build, signing and notarisation; Windows
   build and installer; the Wayland path on Linux. One owner per OS with that
   machine.
5. **Later:** audio (the voice concierge), tray and notifications.

Gate before step 3 starts: run this spike's `shot.sh` against a real daemon
(proof 2 for real) and on one real desktop per OS.
