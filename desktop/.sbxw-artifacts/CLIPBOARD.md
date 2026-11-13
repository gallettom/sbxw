# Copy and paste in the desktop client

Branch `sbxw/desktop-clipboard`. Bug reported on a real Mac: "copié collé
marche pas". Rune pinned at `09ab92a5c83b906adb6fbf92637323b49e86d7ac`, not
bumped. Rune paths below are under `crates/rune/src/` at that rev; desktop
paths under `desktop/`. Shots in `desktop/.sbxw-artifacts/clipboard/`.

## Cause

1. **Copy**: Claude Code's TUI holds the mouse, makes its own selection and
   copies through OSC 52. The shell dropped every OSC 52 write on purpose
   (the user's screenshot showed "mus: a program asked to copy 40 bytes to
   your clipboard; ignored"). The highlighted text was the TUI's, not the
   terminal's, so ⌘C had nothing of its own to copy.
2. **Paste**: nothing found broken on macOS in the source (facts below);
   not verified on a real Mac. A real defect found off macOS: the root
   `on_paste` made Rune swallow plain Ctrl+V on Linux and Windows.

## Facts from Rune's source

| Question | Answer | Where |
|---|---|---|
| What ⌘C, ⌘V, ⌘A deliver | `Key::Command('c')` (letter lowered) with `cmd = true` and `logo = true` (both from Super on macOS), `ctrl = false`. On Linux and Windows Ctrl sets `cmd` and `ctrl`. A non-Latin layout: macOS switches to the Latin layout under ⌘, so still `'c'` | `window/keys.rs:7` (`map_mods`), `:63` (`map_key`), `:79` (`map_key_event`) |
| Is there an Edit menu by default | Yes, when the menu bar is still winit's default (a lone app menu): Rune adds Undo, Redo, Cut, Copy, Paste, Select All, Minimize, Close, each sent to the first responder | `native_menu/standard.rs:31`, `native_menu/standard_macos.rs:29` (`install`), `window.rs:540` |
| What those items call | Not the app: Rune adds `copy:`, `paste:`, `selectAll:` to winit's view and replays the key into it (the real event when it was the item's own equivalent, else a made-up ⌘-key press). So ⌘C, ⌘V, ⌘A reach Rune as keys, once, typed or picked in the menu | `native_menu/standard_macos.rs:72` (`hook_view_class`), `:94` (`perform`), `:154` (`press`) |
| ⌘C path | An ordinary key: the focused element's `on_key` (the grid's, `grid-N`) first, then the window's `on_key`. There is no `on_copy` hook. `Tx::copy` needs no focus nor gesture; the write goes to the clipboard worker after the frame | `runtime/input/keys.rs:23` (`element_key`), `:87`; `clipboard/tx.rs:19`; `runtime/frame.rs:201`; `clipboard/native.rs:15` |
| ⌘V path | **Not a key.** When the focused field or the root has a paste handler (`accepts_paste`), the primary modifier + V alone (`edit_command() == Some('v')`) becomes a clipboard read, then the root's `on_paste`; no `on_key` sees it, so it cannot be typed as well | `runtime/input/physical.rs:163`, `window/events.rs:426`, `edit/keys.rs:146`, `runtime/input/keys.rs:313`, `runtime/input/action.rs:477`, `:506` |
| Same rule off macOS | `edit_command` is "primary modifier alone", which is Ctrl there: **plain Ctrl+V** became a paste and never reached the terminal (Bash's quoted insert, Vim's block select). Ctrl+Shift+V is not an edit command, so it reached `on_key` | `edit/keys.rs:146` |
| Focus after a mouse selection | A press on the grid focuses it (`.focusable(true)`), `tx.focused()` reads `grid-N`, the shell's `poll` follows it. Seen headless: every key line says `focus grid-1` after a click in the grid | `desktop/src/terminal/view.rs:26`, `desktop/src/shell.rs` (`poll`) |
| OSC 52 read form (`52;c;?`) | alacritty_terminal 0.26 `Osc52::OnlyCopy` (its default) denies the load before any event; the shell also ignored `ClipboardLoad`. Now stated in the config | alacritty `term/mod.rs:1726`; `desktop/src/terminal.rs` (`new_term`) |
| A local selection under a repainting TUI | alacritty drops a selection when a line it covers is erased (EL, ED). Claude Code erases and redraws its live region, so a Shift+drag selection there can vanish on the next repaint; older output above it stays | alacritty `term/mod.rs:1657`, `:1773`, `:1786` |

The headless host has no OS clipboard, but `rune::clipboard::memory()` with
`Runtime::use_clipboard` (`clipboard.rs:217`) gives tests a real one, so copy
and paste are now tested through Rune's own routing.

## The fix

- **OSC 52 copies** (`desktop/src/approval.rs`, `App::osc_copy`): the web
  UI's policy (`assets/js/panes.js`, lines 262 to 330 and 422 to 445), by
  decoded size: up to 4 KB copied at once with "Copied N B from <sandbox>";
  up to 100 KB offered by a card the core draws (key `copy-offer`, buttons
  `copy-offer-copy` and `copy-offer-dismiss`) naming the sandbox and the
  size, showing the first 80 characters with line breaks, tabs, controls,
  DEL, bidi and zero-width marks made visible, copying only on Copy, gone
  after 30 s; a newer offer replaces it; above 100 KB dropped with a notice.
  Writes only.
- **Ctrl+V off macOS**: the root `on_paste` is set only on macOS (and in
  headless runs, for scripted pastes), `shell::window_paste`. Same in
  term-demo.
- **Debug switch**: `SBXW_DESKTOP_DEBUG_KEYS=1` prints to stderr, prefix
  `sbxw keys:`, one line per key reaching a terminal (key name only for
  chords or keys that type nothing, otherwise `text`; mods; pane; Rune's
  focus; what it became), per paste (bytes, bracketed or not), per copy
  (bytes, and why 0), per OSC 52 copy (bytes, tier). Never any content.
- Unchanged: Option characters, dead keys, IME, reconnect, ⌘C and ⌘A on the
  terminal's own selection, Ctrl+Shift+C/V/A and Shift+Insert, Ctrl+C.

Local selection while a program holds the mouse: Shift+drag (or Shift+double
click), as in xterm and iTerm2; ⌘C then copies it. Tested.

## Verified headless (Linux, llvmpipe)

- `cargo test`: 67 passed, 2 ignored (59 before). New tests:
  `real_event_shapes`,
  `mac_cmd_c_copies_the_selection_held_while_the_program_has_the_mouse`,
  `mac_paste_through_the_root_reaches_the_terminal_bracketed_or_not`,
  `pc_ctrl_shift_c_and_v_copy_and_paste_while_ctrl_c_and_ctrl_v_stay_the_programs`,
  `osc52_read_form_is_never_answered`,
  `osc52_copies_offers_or_drops_by_size_and_a_newer_offer_replaces`,
  `osc_copies_by_size`, `preview_shows_what_would_be_pasted`.
- `desktop/scripts/headless/clipboard.json` (`desktop/.sbxw-artifacts/clipboard/copy-paste.png`,
  looked at): a double click selects "bravo", Ctrl+Shift+C logs `copy (5
  bytes)` with `focus grid-1`, Ctrl+C reaches bash (`^C`), Ctrl+V reaches
  the tty (its literal-next turns the Enter after it into `^M`), a window
  paste into `cat -v` after `\e[?2004h` shows `^[[200~pasted one` / `pasted
  two^[[201~`, after `\e[?2004l` shows `plain paste`, an OSC 52 write of
  "hello" shows "Copied 5 B from web-app".
- `desktop/scripts/headless/osc52-offer.json` (`desktop/.sbxw-artifacts/clipboard/osc52-offer.png`,
  looked at): a 6028-byte OSC 52 write shows "web-app wants to copy 5.9 KB
  to your clipboard:" over `echo hi↵⇥second �evil␛[0m xxx…`, wrapped; a tap
  on Copy logs `osc52 offer copied 6028 bytes` and "Copied 5.9 KB from
  web-app", and the card is gone. The control pictures (␛) need a font that
  has them: this sandbox had none until `fonts-noto-core`; macOS has Apple
  Symbols.
- term-demo's `term-select.json` as before (`copy: 5 bytes`, bracketed
  paste); `scripts/demo-reconnect.sh` connects again.

Not verified (no Mac here): the real ⌘ keys, the Edit menu, the system
clipboard, the menu bar and Dock name.

## Test on the Mac

1. `cd desktop && git pull && SBXW_DESKTOP_DEBUG_KEYS=1 cargo run --release 2>~/sbxw-keys.log`
2. The menu bar shows **sbxw** (not sbxw-desktop), and so do the Dock and
   ⌘Tab. The app menu reads About sbxw, Hide sbxw, Quit sbxw.
3. In a pane running Claude Code, select text with the mouse as before.
   Expected: a notice at the bottom right "Copied N B from <sandbox>". Paste
   it in TextEdit: it is there.
4. In a bash pane:
   `printf '\e]52;c;%s\a' "$(seq 1 2000 | tr '\n' ' ' | base64 | tr -d '\n')"`.
   Expected: a card "<sandbox> wants to copy 8.7 KB to your clipboard:" with
   Copy and Dismiss. Copy puts the numbers on the clipboard; Dismiss does
   not.
5. In bash: `echo alpha bravo`, double click "bravo", ⌘C, then ⌘V: `bravo`
   is typed once.
6. In Claude Code's pane: Shift+drag over older output (a local selection),
   ⌘C, then ⌘V in TextEdit.
7. Copy a line in another app, ⌘V in bash: it appears once. Also try Edit >
   Paste in the menu bar.
8. Ctrl+C still interrupts (`sleep 100`, then Ctrl+C).
9. If something fails: quit the app and paste the output of
   `grep 'sbxw keys' ~/sbxw-keys.log | grep -v 'key text' | tail -40`. It
   holds key names for shortcuts only and byte counts, never what was typed
   or copied.

Reading the lines: `key Command('c') mods=[cmd,logo] pane 1 focus grid-1 ->
copy (5 bytes)` is a working ⌘C. No line at all for a ⌘C means the key never
reached the app (a menu or system shortcut took it). `paste (window
on_paste) N bytes -> pane 1` is a working ⌘V. `focus none` means no
terminal had Rune's focus (click in the pane first). `osc52 copy pane 1 N
bytes -> Copy` is the TUI's own copy.

## Upstream issue (Rune)

No Rune gap found for macOS at this rev. One rule of Rune's is wrong for a
terminal; worked around here. Text for an issue:

> **A root `on_paste` swallows plain Ctrl+V on Linux and Windows.**
> `Runtime::dispatch_physical` (runtime/input/physical.rs:163) and the
> window host (window/events.rs:426) turn any `edit_command() == Some('v')`
> into a clipboard read whenever `accepts_paste()` is true, and
> `accepts_paste` is true for any root `on_paste`. Off macOS the primary
> modifier is Ctrl, so an app embedding a terminal, which wants ⌘V to paste
> on macOS, loses Ctrl+V (^V) everywhere else, and its focused element's
> `on_key` never sees it. Proposal: give the focused element's `on_key`
> first refusal before the paste read, or let an element opt out
> (`El::paste_chord(false)`). Workaround today: set `on_paste` only on
> macOS, and read the clipboard from Ctrl+Shift+V with `Tx::read_clipboard`.

## The app's name

The binary is `sbxw` (`[[bin]] name = "sbxw"`, `default-run = "sbxw"` in
`desktop/Cargo.toml`), so `cargo run --release` still needs no argument; the
package stays `sbxw-desktop` (cargo metadata only). On macOS an app outside
a bundle is named after its process (`NSProcessInfo.processName`): menu bar,
Dock, app switcher, and Rune's About, Hide and Quit items
(`native_menu/default_macos.rs:102`). Rune has no hook to set that name. The
window title and the top bar already said "sbxw". Nothing installs this
binary on PATH, where the CLI's `sbxw` lives (separate target directories).
A future `.app` bundle sets `CFBundleName` (and `CFBundleDisplayName`) to
`sbxw`. Scripts updated: `desktop/scripts/lib.sh`, `demo-reconnect.sh`.
