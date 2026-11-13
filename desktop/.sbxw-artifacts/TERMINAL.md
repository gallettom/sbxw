# sbxw desktop: the terminal

Thread `desktop-terminal`, wave 1 of the native client. The spike's terminal
(RUNE.md, proof C: grid, colours, block cursor, keys, resize) is now a
terminal to work in all day. Everything here is in `desktop/src/terminal.rs`
and `desktop/src/terminal/`; it is tested through its own binary
`desktop/src/bin/term-demo.rs` against the stand-in daemon. Verified on Linux
(Debian trixie, aarch64) in an sbx sandbox, headless through Rune, plus one
windowed run under Xvfb for latency.

## What works

| Feature | State | Evidence |
|---|---|---|
| Scrollback: 10,000 lines (as the web UI), wheel, Shift+PageUp/PageDown/Home/End, back to the bottom on input; in a full-screen program those keys go to the program | done | unit tests, `terminal/scrollback.png` |
| Selection: drag, double click (word), triple click (line), select all, into the history, wrapped lines rejoined, auto-scroll at the edges | done | unit tests, `terminal/select.png` |
| Copy and paste: ⌘C/⌘V/⌘A on macOS, Ctrl+Shift+C/V/A and Shift+Insert elsewhere (plain Ctrl+C stays the interrupt), through Rune's clipboard (`Tx::copy`, `Tx::read_clipboard`) and the window's `on_paste`; no new crate | done, partly unverified (see below) | unit tests, `select.png`, log `copy: 5 bytes` |
| Bracketed paste (DECSET 2004), line breaks as `\r`, end marker stripped from pasted text | done | unit test, `select.png` (`cat -v` shows `^[[200~...^[[201~`) |
| Mouse reports: 1000, 1002, 1003, SGR 1006, UTF-8 1005, X10; wheel as buttons 64/65; Shift selects even then; xterm alternate scroll (arrows) in a full-screen program without mouse mode | done | unit tests, `terminal/htop.png` |
| Focus reports (DECSET 1004), hollow cursor when the window loses focus | done | unit test, `cursor.png`, `cursor-blurred.png` |
| Alternate screen | alacritty, unchanged | `htop.png` |
| Cursor shapes (block, beam, underline, hollow) from DECSCUSR, blink (on by default as in the web UI, steady for a period after activity, wake-ups scheduled with `Tx::wake_after`) | done | unit tests, `cursor.png` |
| Bold (and bold ANSI 0-7 as bright, as xterm.js), dim, italic, underline, double, curly, dotted, dashed, coloured underline (SGR 58), strikethrough, inverse, hidden, true colour, 256 colours | done | `terminal/glyphs.png`, `glyphs@2.png` |
| Box drawing U+2500-257F (light, heavy, double, rounded, diagonals), block elements U+2580-259F, Powerline triangles: drawn as vectors on whole points, so lines meet across cells and rows | done | `glyphs@2.png` |
| Wide characters pinned to their two columns, a wide character that does not fit wraps whole, combining marks | done | unit test, `glyphs.png`, `resize.png` |
| OSC 8 hyperlinks: underlined on hover, pointer cursor, ⌘-click (macOS) or Ctrl-click opens web links only (as the web UI) | done (opening unverified) | unit test, `glyphs.png` (hovered "the docs") |
| Keys: Option-composed characters on macOS sent as typed (`\|` on Swiss and French layouts), AltGr on Windows, Alt as Meta elsewhere, `KeyOpts::option_as_meta` for macOS | done; `\|` **verified on the user's Mac with a real keyboard** (commit 9f2543f); other Option characters not individually confirmed | unit tests |
| IME and dead keys: `TerminalState::on_ime` (commit typed, composition drawn at the cursor), an `.ime_cursor()` caret at the cursor so Rune turns the input method on and puts the candidate window there | done in term-demo; main.rs must register `App::on_ime` (requested) | unit test, `terminal/ime.png` |
| Resize debounce: the grid follows at once, the daemon hears once the size held 80 ms | done | unit test, `resize.png`: 1 resize frame for a 0.4 s drag (spike: 17) |
| Reconnect with the daemon's replay: retry with backoff (0.5 s to 10 s), a fresh grid before the replay (as the web UI's `term.reset()`), so nothing is duplicated; the size is sent on the new socket | done | unit test, `terminal/reconnect.png` |
| The daemon's `attention` frame, bell, title, OSC 52 copy request, disconnect, reconnect as `TerminalEvent`s the shell shows | done | unit tests, `reconnect.png` (banner) |
| Damage: only rows alacritty reports changed, or whose cursor, selection or hovered link changed, are rebuilt | done | unit test, measurements |
| Theme: `Theme` struct passed in by the caller, the web UI's defaults (`#0d1117`, text `auto`, cursor `#58a6ff`, selection `#58a6ff4d`, xterm.js's ANSI palette), `Theme::from_setting_strings(terminal_color, terminal_text_color)` with the same rules as `term-color.js` | done | unit test, `terminal/theme.png` |
| Colour and size queries (OSC 10/11/4, CSI 14/18 t) answered from the theme; the sandbox never reads the clipboard (OSC 52 read ignored) | done | unit test |

## Public API

Documented at the top of `src/terminal.rs`. The spike's API still works
unchanged: `TerminalState::new`, `poll`, `send`, `fit`, `on_key`,
`screen_text`, the fields `size`, `status`, `rows`, `cursor`, `echo_ms`,
`rebuilt_rows`, `build_time`, and `FONT_SIZE`, `LINE_HEIGHT`, `PAD`,
`STATUS_H`, `MONO_FAMILIES`, `style`, `cell_width`, `key_bytes`, `rgb`, `FG`,
`BG` (`BG` is now the web UI's `#0d1117`, so main.rs's fill matches). New:

- `TerminalState::with_reconnect(conn, reopen)`, `poll_tx(settle, clock, tx)`,
  `take_events()`, `set_theme`, `on_key_tx`, `on_ime`, `paste`,
  `set_focused`, `fit_at`, `resize`, scrollback and selection methods,
  `on_drag` / `drag`, `on_wheel`, `on_pointer_move`, `on_secondary_click`.
- `terminal::pane(t, lens)` (status line, grid, padding, background, layout)
  and `terminal::view(t, lens)` (the grid alone), with every input wired.
- `Theme`, `KeyOpts`, `TerminalEvent`, `Reconnect`, `DragInput`,
  `keys::{key_bytes_with, shortcut, paste_bytes}`, `mouse::report`.

**Several terminals per window** (desktop-core's multi-pane shell): nothing
assumes one terminal per app. `lens` is a closure, so each pane passes its own
(`move |s: &mut App| s.panes.get_mut(i)?.term.as_mut()`); element keys take
`TerminalState::id` as a suffix (`grid-<id>`, `terminal-<id>`); events,
focus, IME and paste are per `TerminalState`. Two things are the shell's:
call `set_focused(true)` only on the pane that has the keyboard (and `false`
on the others: the IME caret is drawn only on a focused terminal, and Rune
wants one), and route the app-level `on_ime`, the root's `on_paste` and the
window's `on_key` (when nothing is focused) to that pane.

**client.rs's sender type**: the module calls only `input.send(..)` on
`TerminalConn::input`, inside a closure built where the connection is taken
apart, so desktop-core can change the sender's type (mio waker) without
touching this module. Code that builds a `TerminalConn` from an
`mpsc::Sender` (my tests in `src/terminal/tests.rs`, term-demo's `Proxy` and
`bench_echo`) needs a one-word change then (`input: in_tx.into()` if the new
type has `From<mpsc::Sender<TermInput>>`).

**main.rs wiring** (sent to the lead for desktop-core; term-demo is the
reference): `rune::App::new(spec).on_ime(..).run()` instead of `rune::run`,
`terminal::pane` instead of the hand-built grid, `poll_tx` and
`take_events`, `on_key_tx`, root `on_paste` (on macOS the Edit menu's Paste
takes ⌘V before any key handler) and `on_window_focus`, `with_reconnect`.

## How to run it

```
cd desktop
cargo test --lib                                   # 36 tests (keys, mouse, selection, scrollback, reconnect, ...)
cargo test --release --lib -- --ignored --nocapture full_screen big_cat
bash .sbxw-artifacts/terminal/term-shot.sh shots/term/htop.png --script scripts/headless/term-htop.json
bash .sbxw-artifacts/terminal/term-shot.sh shots/term/reconnect.png --script scripts/headless/term-reconnect.json --simulate-drop
target/release/term-demo --bench-echo 300          # socket thread latency and idle cost
bash .sbxw-artifacts/terminal/xvfb-latency.sh 100 40   # windowed key-to-echo under Xvfb
```

`term-shot.sh` builds, starts the stand-in (`scripts/lib.sh`), runs
term-demo headless, and writes `<out>.txt` beside the PNG: size, scroll
offset, history, status, selection, events, bytes in, rows rebuilt, and the
visible text. Shots use a steady cursor (`--no-blink`) unless `BLINK=1`.
term-demo script marks: `drop` (with `--simulate-drop`, a proxy that keeps the
stand-in's shell running and replays what it saw, as the daemon's 256 KB
ring does), `attention`, `theme:<bg>:<text>`, `scroll:<n>`. The glyph and
theme scripts `cat` `samples/glyphs.ans` by its absolute path in this
worktree; regenerate the path if the checkout moves.

## Evidence (each image looked at)

All in `desktop/.sbxw-artifacts/terminal/`, 960x640 at scale 1 unless said,
with the `.txt` dump beside each.

- `scrollback.png` (`term-scrollback.json`): `seq` printed 300 lines, then
  Shift+PageUp twice and a wheel step: the view shows `history line 200` to
  `232`, the status line reads "scrolled 69 of 269 lines (Shift+End:
  bottom)" (2 pages of 32 lines plus 5 lines of wheel).
- `select.png` (`term-select.json`): a selection from "bravo" on row 1 to
  "fo" of "foxtrot" on row 2 drawn in the selection blue over the text; the
  dump says `selection: Some("bravo charlie\ndelta echo fo")`. Before it, a
  double click selected "bravo" and Ctrl+Shift+C copied it (`copy: 5 bytes`
  in the log); a scripted paste into `cat -v` after `printf '\e[?2004h'`
  shows `^[[200~pasted one` / `pasted two^[[201~` (tty echo, then cat's
  output).
- `htop.png` (`term-htop.json`): htop full screen in the alternate screen,
  colours, meters, the F-key bar. A scripted click on the `MEM%` header
  sorted by memory (the header highlight moved to `MEM%▽`), a click on a row
  selected it and a wheel step scrolled the list: all three through SGR mouse
  reports.
- `glyphs.png`, `glyphs@2.png` (scale 2) (`term-glyphs.json`, sample in
  `samples/glyphs.ans`): every style on one row (bold, dim, italic,
  underline, double, curly, dotted, dashed, strike, inverse, red curly
  underline); a 24-bit gradient; 256 colours; ANSI 16 with bold red bright;
  light, heavy and double boxes and a rounded box closed with no gaps between
  cells or rows; block elements and quadrants, two rows of full blocks with no
  seams; `日本語テキスト|` with the bar in its column; Korean as boxes (this
  machine has no Hangul font: `fc-list :lang=ko` is empty); 🎉✅ and `é`; the
  OSC 8 link "the docs" underlined under the pointer; Powerline segments.
- `resize.png` (`term-resize.json`): `stty size` says `33 111`, then after a
  scripted window drag `19 62`; `seq` wraps at 62 columns; a `日` that did not
  fit on the prompt line moved whole to the next one. The log shows one
  resize frame for the drag.
- `reconnect.png` (`term-reconnect.json --simulate-drop`): 40 lines printed,
  the connection dropped and came back, `echo after reconnect` ran; the
  banner reads "disconnected: simulated drop | reconnected, replay follows |
  attention: the session wants you" (amber: attention). Each `before N` line
  is there once; the history is 12 lines for 44 lines on a 32-row screen
  (the banner took a row), so the replay replaced the screen rather than
  adding to it.
- `cursor.png` (`term-cursor.json`): `printf '\e[6 q\e[?1004h'; cat -v`, a
  scripted window blur and focus: `cat -v` shows `^[[O^[[I` (focus out, in),
  then typed `beam`, with a beam cursor. `cursor-blurred.png`: the same run
  while blurred, a hollow cursor.
- `ime.png` (`term-ime.json`): scripted input-method events: a dead-key
  composition `^` committed as `ê` (typed into the shell: `echo dead key: ê`),
  then a Japanese composition `にほん` drawn at the cursor in a bordered box,
  not sent.
- `theme.png` (`term-theme.json`): the glyph sample after the mark
  `theme:#fdf6e3:auto`: the web UI's "Paper" background with dark text, as
  `termTheme()` computes it; every row redrawn.

## Measurements

Same machine (10 cores, llvmpipe, other threads building in the sandbox at
the same time, so absolute numbers are noisy).

- **Full-screen redraw** (`full_screen_redraw_cost`, release, 111x42, every
  cell changes colour and character each frame, scale 2, CPU only): parse and
  row rebuild p50 **0.21 ms** (p95 0.28), spike 0.82 ms; Rune layout and
  paint p50 0.23 ms (p95 0.33); 4,663 instances.
- **Large `cat`**, unit bench (`big_cat_throughput`, release): 20 MB of
  coloured log lines in 64 KB chunks, one frame per MB: parse and rebuild
  88 ms (**226 MB/s**), with Rune's layout and paint 115 ms (175 MB/s),
  5.45 ms per frame, 954 rows rebuilt over 23 rebuilds, history capped at
  10,000.
- **Large `cat`, the real path** (`term-bigcat.json`, release, headless):
  `cat` of a 10 MB log through the stand-in's PTY, its WebSocket and
  client.rs: 10,594,435 bytes in, 6 rebuilds, 100 rows rebuilt, 1.7 ms of row
  building in total; Rune's report for the run: 205 frames, frame CPU p50
  3.54 ms, p95 4.12 ms. (The 10 MB file is generated into `shots/term/`,
  not committed.)
- **Damage**: one typed character rebuilds at most 2 rows of 24; a frame
  with nothing new at most 1 (alacritty always reports the cursor's row)
  (`only_damaged_rows_are_rebuilt`).
- **Resize**: one resize frame per window drag (17 in the spike).
- **Key to echo, socket thread only** (`term-demo --bench-echo 300`, release,
  in-process echo server with blocking reads): client.rs's pump with its 5 ms
  read timeout p50 **5.63 ms**, p90 11.30, p99 13.94, max 23.89; at rest
  **2.0 ms CPU per second and 83 wake-ups per second** per open terminal. A
  blocking-read client against the same server: p50 0.15 ms, p99 0.36 ms.
  Sent to the lead as a request for desktop-core (a `mio` poll with a waker;
  `mio` 1.2.4 is already in Cargo.lock); being done there.
- **Key to echo, whole path in a window** (`xvfb-latency.sh`, Xvfb,
  xdotool, 100 keys 40 ms apart, release): p50 21.1 ms, p90 28.1. The spike's
  terminal.rs built in the same session measures the same (p50 21.5 ms), so
  this is the environment (frame pacing on llvmpipe under load, the stand-in's
  5 ms polling and client.rs's), not the terminal code. RUNE.md's 8.4 ms was
  measured on a quieter machine. Time to pixels was not measured.

## What Rune lacks (text for an upstream issue)

The project takes no pull requests; this is written to be pasted as an issue
against rev `09ab92a5`.

```
Title: Gaps met building a terminal on rune::grid (rev 09ab92a5)

We draw a full terminal emulator with `rune::grid` / `GridRow` and it works
well (per-row reuse, pinned wide cells, the vector layer). Gaps we had to
work around:

1. Headless clipboard. In a headless run (`--script`), `Tx::copy`,
   `Tx::read_clipboard` and `Tx::request_paste` do nothing (no clipboard is
   wired into the headless host), so a script cannot test copy then paste.
   Only a script `paste` event reaches the root `on_paste`. A memory
   clipboard in headless runs (and its content in `--tree`) would make
   copy/paste testable.
2. No focus/blur callback for a focusable element. Only window activation
   (`on_window_focus` on the root) is observable. A terminal cannot show a
   hollow cursor or send focus reports (DECSET 1004) when another element in
   the same window takes the keyboard. `El::on_focus_change(bool)` would do.
3. Secondary button. The right button only reaches `on_secondary_press`, on
   release; `pointer_buttons` cannot opt it into `on_drag`. Terminal mouse
   reporting needs its press, drag and release separately; we send press and
   release together on release.
4. IME registration is app-level only: `App::on_ime` exists on the `App`
   builder but not on `AppSpec` (apps started with `rune::run` cannot get
   it) nor on an element, so a reusable editor component cannot own its
   input method; with several editors the app routes events itself.
5. Font metrics. There is no API for a `TextStyle`'s ascent, baseline,
   underline position and thickness, and `Span` has only a single underline
   (no strikethrough, no double/curly/dotted/dashed). Drawing those in
   `GridRow::shape` means guessing where the baseline is.
6. Device scale. The view cannot read the device scale, so vector strokes in
   `GridRow::shape` cannot be snapped to device pixels; we snap to whole
   points, which is exact at 1x and 2x only.
7. `--tree` names the grid element but not its rows' text (the accessibility
   tree has it), so agents need an app-side text dump to check a terminal.
8. Scripted `key` events with `"shift": true` keep the unshifted text
   (`"a"` stays `a`), unlike a window; scripts must name the shifted
   character.
```

Not a Rune gap but worth knowing: with nothing marked `.ime_cursor()`, the
window's input method stays off, so dead keys do nothing; that is why the
terminal now draws one at its cursor.

## What is not verified

- **A real keyboard and mouse**: only the Option+7 `|` fix was checked by the
  user on their Mac. Everything else ran through Rune scripts (headless) or
  XTEST (latency only): real selection by mouse, double and triple click
  timing, trackpad and notched-wheel scrolling feel, right click, middle
  click, hover, ⌘/Ctrl-click opening a link (`Tx::open_url` is never called
  headless).
- **The real clipboard**: copy was seen leaving the terminal (`Tx::copy`
  called with the selection), but nothing reads a system clipboard headless;
  `Tx::read_clipboard` (Ctrl+Shift+V, ⌘V when the menu does not take it) has
  never run. The root `on_paste` path ran with scripted text.
- **IME in a window**: only scripted `Ime` events. Dead keys on macOS
  (`^` then `e`) need main.rs's `App::on_ime` first; IBus/XIM on Linux and the
  Windows IME were not tried. Option as Meta (`KeyOpts::option_as_meta`) has
  no setting yet and was not run.
- **macOS and Windows** for this code (the spike's rendering was checked on
  macOS by the user); fractional HiDPI scales; Wayland.
- **The real daemon**: reconnect and replay were tested with a unit test and
  with term-demo's proxy, not with the daemon's own 256 KB replay
  (`crates/daemon/src/http/ws.rs`); the `attention` frame was injected, not
  sent by the daemon.
- **GPU frame times** and time to pixels: all frame numbers are CPU only, on
  llvmpipe.
- Korean and other scripts without a font on this machine.
