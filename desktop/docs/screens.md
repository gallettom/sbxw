# Writing screens

A screen is one Luau file that describes what part of the window shows,
from data the core hands it. It cannot reach the daemon, the disk, the
network or the clipboard. It draws with the elements below and asks the core
for things by returning **intents** from its actions. The core checks every
element, every prop and every intent, and refuses anything it does not know.

This document is the whole contract: write a screen against it without
reading the Rust. The element set is **frozen** (wave 1): elements may be
added, existing ones never change (`markdown` was added in phase 2). Asking
for a new element or intent goes through the core's owner.

Contents: [The file](#the-file) · [Data](#data) · [Elements](#elements) ·
[Events and actions](#events-and-actions) · [Intents](#intents) ·
[Theme](#theme) · [Icons](#icons) · [Where screens live](#where-screens-live) ·
[Limits](#limits) · [Checking your screen](#checking-your-screen)

Beside it: [daemon-api.md](daemon-api.md) (the routes behind `data`) and
[testing.md](testing.md) (running screens and the app headless).

## The file

```lua
return {
  -- Optional. Called once, the first time the screen loads; returns `state`.
  init = function() return { filter = "" } end,

  -- Required. Called whenever the data or the state changes. Returns ONE element.
  render = function(state, data)
    return column{ text{ "Hello" }, text{ #data.sandboxes .. " sandboxes" } }
  end,

  -- Optional. Called by events (see "Events and actions").
  actions = {
    pick = function(state, arg, value) state.picked = arg end,
  },
}
```

- `state` is a table that lives in the VM. **It is kept across reloads**, and
  also when an override file replaces the built-in or the other way round. A
  new version of the file may find fields the old one set, or miss fields it
  expects: default every field you read (`state.n or 0`).
- `render` must not change `state` (it may run at any time, many times).
- A file that fails to load or render keeps the **last good view**, with the
  error in a red banner above it. Fix the file and save: the banner goes.

## Data

`render(state, data)` receives:

| Key | What |
|---|---|
| `data.sandboxes` | `GET /api/sandboxes`, an array of sandboxes (below) |
| `data.app` | The core's own state (below) |
| `data.jarvis` | Jarvis: the conversation, its sandbox's state, waiting proposals, the voice (below) |
| other keys | Later waves add their daemon data here (`threads`, `plans`, ...), by the key their fixture uses |

**When a screen renders.** Only while it is drawn (the top bar, the list,
an open panel, overlay or dialog, Jarvis's panel when open), and only when
something it read in its last render changed: the core notes which fields of
`data` (and of `data.app`) a render reads, and turns each into Lua only then.
A closed screen keeps its data and renders when it is next shown. So
`render` must be a function of `state` and `data` only (it is: nothing else
reaches it), and iterating `data` or `data.app` with `pairs` reads all of
that level.

A sandbox: `name` (string), `agent` (`claude`, `codex`, ...), `status`
(`running`, `stopped`, `stopping…`, or another word sbx uses), `workspace`
(string or nil), `chat` (bool), `created_at` (ISO time or nil), `detached`,
`remote_control`, `old_sbx` (bools), `approvals`, `kits_waiting` (numbers),
`unreachable_since` (unix seconds or nil). Fields the daemon adds later appear
unchanged.

`data.app`:

| Key | What |
|---|---|
| `connection.state` | `connecting`, `up` or `down` |
| `connection.text` | One readable line: where it is connected, or why not (`No sbxw daemon at 127.0.0.1:7681. Start it with \`sbxw up\`; retrying.`) |
| `selected` | The selected sandbox's name, or nil |
| `focused` | The focused pane's number, from 1 |
| `layout.panes`, `layout.split` | How many panes are open, on every page; `columns`, or `rows` when the current page's first split is stacked |
| `pages` | The pages of terminals: `current` (from 1), `count`, `per` (terminals per page, 1 to 6), `view` (`tabs` when `per` is 1, else `grid`), `panes` (each page's pane numbers, in reading order) |
| `panes` | One entry per pane, page by page (a pane on another page is open, not shown): `index` (from 1), `sandbox` (or nil), `mode` (`agent`, `bash`, `kit-add`), `thread` (the thread whose session it shows, or nil: the sandbox's own), `focused` (bool), `status` (`empty`, `connecting`, `connected`, `reconnecting`, `closed`, `error`) |
| `overrides` | Screens running an override file, by name: `path`, `stale` (made for an older built-in; see "Where screens live") |
| `kept` | What features asked the app to keep across runs (`Action::Remember`), by feature then key |
| `surface.release`, `surface.focused` | The key that gives the keyboard back from a picture surface (`⌘Esc`, `Ctrl+Esc`); the id of the surface that has the keyboard, or nil |
| `platform` | `macos`, `linux`, `windows` |
| `window` | `w`, `h`: the window's size in points (the plan dialog moves its header into the scrolling part on a short window) |
| `groups` | Each sandbox's group colour (`#rrggbb`), by name; absent for a sandbox in no group (a sandbox in no group and no workspace takes a hue of its own while the list draws a session block for it, several sessions: `App::own_hues`). The web UI's hue first (chats share one, a workspace folder's from its path), then made distinct: a group within 40° of a hue already taken (chats first, then workspaces by path) moves to the nearest free hue, so two groups never share a background; it can then differ from the web UI's (`theme::group_colors`) |
| `group_look` | By group colour (as `groups` gives it): `{ tint, hover, label }`, the list's block, its header hovered and the group's label, in the theme drawn now (`theme::group_tints`) |
| `theme` | The theme drawn: `dark` or `graphite` |
| `sidebar` | `w` (its width in points, which the person drags, 200 to 420) and `shown` (Cmd/Ctrl+B hides it) |
| `hover_zone` | The [hover zone](#hover-zones) the pointer rests in (this screen's), or nil |
| `sidebar_menu` | A menu of the sandbox list (a row's ⋯) is open: the list holds its order until it shuts |
| `covered` | Something sits over the view drawn in the pane area: the core's dialog, a modal, Jarvis's proposal, the palette, or an overlay drawn over the base view (read only, `App::covered`). What the BASE view reads: Sessions draws its send controls disabled then, and its Rust refuses to send whatever the screen draws |
| `covered_top` | Something sits over whatever view is on top: the core's dialog, a modal, Jarvis's proposal or the palette, never an overlay (read only, `App::covered_top`). What an OVERLAY reads for its own actions: Canvas (Ask, Send, its files) is drawn over the Sessions base in GUI and must not take the base's `covered` for its own. The core's dictation uses the same split: `covered` for the Sessions composer, `covered_top` for a field of an overlay or a panel (Requests, Jarvis) |
| `shortcuts` | Every keyboard shortcut of the app, in order: `keys` (keycaps, `{ "⌘", "K" }`) and `what` (what it does). Help draws this list |

`data.jarvis` (every screen gets it; the `jarvis` screen draws it):

| Key | What |
|---|---|
| `open` | The panel is open |
| `status` | Jarvis's sandbox: `unknown` (not asked yet), `absent`, `creating`, `starting`, `running`, `stopped`, `failed` |
| `error` | Why it failed, or nil |
| `working` | Jarvis is on a turn (thinking) |
| `sending` | A message is on its way |
| `log` | The conversation, oldest first: `id` (number), `who` (`me`, `jarvis`, `note`, `error`), `text` (Jarvis's answers are Markdown: draw them with `markdown`), `reading` (bool: being read aloud) |
| `pending` | Proposals waiting for the person: `id`, `summary`, `risk` (`low`: a typed or spoken yes approves it; `high`: only a click on the core's dialog). No diffs: the dialog is the core's |
| `voice.installed` | The daemon has the voice |
| `voice.hears`, `voice.speaks` | This machine can record and transcribe; can read aloud |
| `voice.recording`, `voice.level` | Push-to-talk is recording; its loudness, 0 to 1 |
| `voice.reading` | Something is being read aloud |
| `voice.key`, `voice.key_note` | The push-to-talk key's name (`F8`), and why there is none, when there is none |

## Elements

Every element is a function that takes a table: positional entries are its
text or its children, named entries are its props. `text "hi"` is `text{ "hi" }`.

- An unknown element, an unknown prop or a value of the wrong type is an
  error naming the element and the prop (`text: unknown prop 'colour'`).
- In a list of children, `false` means "nothing here", so
  `cond and badge{ "x" } or false` works. A plain string child is a `text`.
- Sizes are in points. Where a prop says *space*, *radius*, *size* or
  *colour*, it also takes a [theme](#theme) name.

### Common props

Every element takes these.

| Prop | Value | Effect |
|---|---|---|
| `key` | string | Names the element for scripts and tests (`tap` by key, `--tree`). Keep keys unique: the first match wins |
| `help` | string | Tooltip after the pointer rests on the element |
| `w`, `h` | number or `"full"` | Width, height (`"full"`: the parent's) |
| `min_w`, `max_w`, `min_h`, `max_h` | number | Bounds |
| `grow` | number | Takes that share of the free space along the parent's direction (CSS `flex: g 1 0`) |
| `pad`, `px`, `py` | number or space | Padding all round, left and right, top and bottom |
| `fill` | colour | Background |
| `border` | colour | 1 pt border |
| `radius` | number or radius | Rounded corners |
| `opacity` | 0 to 1 | |
| `pin` | `{ left =, top =, right =, bottom = }` (any of them; numbers or space) | Out of the parent's flow, pinned to those edges of its padding box: a toolbar over the end of a row |
| `lift` | `"raised"`, `"float"` or `"overlay"` | A shadow from the theme's elevation (`theme::elevation`): `float` for a card that floats over a screen (the explorer's saved toast), `overlay` as menus and dialogs. Any other word: no shadow |
| `reveal` | bool | Drawn only while the nearest `hover_group` above it is hovered or holds the keyboard focus. It stays pressable and reachable with Tab (reaching it lights the group). Use it for a row's secondary actions, and keep them in the row's menu too: a touch or trackpad user who never hovers reaches them there |

A key you do not set is made for you on elements that react: `<on>-<arg>`
(`stop-docs` for `button{ on = "stop", arg = "docs" }`), or the `id` for
`input`, `menu`, `select`, `split`.

### `text`

Text. Positional: the string (a number is shown as its digits).

| Prop | Value | Default |
|---|---|---|
| `color` | colour | `text` |
| `size` | number or text size | 13 (`md`) |
| `weight` | `normal`, `medium`, `semibold`, `bold`, or 100 to 900 | `normal` |
| `mono` | bool | false. The terminal's font. The UI is monospace, as the web UI is; `mono` also keeps a wrapped text monospace (a command, a path) |
| `wrap` | bool | false: one line. A wrapped text is a sentence or more: it is set in the prose face (Inter, bundled by Rune), as Markdown and the core dialogs' sentences are; `SBXW_DESKTOP_PROSE=mono` keeps everything monospace |
| `truncate` | bool | false. One line cut with an ellipsis when it does not fit (give it `grow = 1` in a row) |
| `pills` | `true` or a sandbox name | none. Keyword pills: each `[[Word]]` or `[[Word: detail]]` in the string is drawn as a calm capsule (the brackets dropped, regular weight, the word in the body text tinted toward its tone, the detail in the body text, on a faint tint of the tone), wrapping with the text, when the daemon's `[emphasis]` switch is on (`true`: the global switch; a sandbox's name: that sandbox's, and the app's own preference for it). Use it for words an agent wrote. See [Keyword pills](#keyword-pills) |

Sizes under 12 are drawn at 12 (nothing a person reads is smaller); a
`weight` of `bold` is drawn semibold (600).

```lua
text{ "Sandboxes", size = "sm", weight = "semibold", color = "text-muted" }
```

### `row`

Children side by side. Children are centred vertically.

| Prop | Value | Effect |
|---|---|---|
| `gap` | number or space | Between children |
| `align` | `start`, `center`, `end`, `stretch` | Cross axis (vertical); default `center` |
| `justify` | `start`, `center`, `end`, `between` | Main axis |
| `hover` | colour | Background under the pointer |
| `on`, `arg` | action name, any value | The whole row is pressable (a button for screen readers, named by its `help`); `hover` defaults to `surface`. Enter presses it when it has the keyboard |
| `on_space` | action name | With `on`: called with the same `arg` when Space is pressed while it has the keyboard (the sandbox list: start or stop) |
| `wrap` | bool | `row`: its children go onto as many lines as they need (a line of chips) |
| `on_left`, `on_right` | action name | With `on`: called with the same `arg` when Left or Right is pressed while it has the keyboard (a tree: fold, open in place; the explorer's folders) |
| `hover_group` | bool | Its `reveal` children show while it is hovered or the keyboard focus is in it |
| `hover_zone` | string | `row`/`column`: one hover zone with every element of this screen that names the same zone, even apart in the tree (a row and the block under it). The core tells the screen where the pointer rests in `data.app.hover_zone` ([Hover zones](#hover-zones)) |
| `group` | `list` | Its pressable children (with `on`) are one list: Up and Down move the keyboard between them |
| `selected` | bool | With `on`: the selected item (screen readers say so) |
| `keep_focus` | bool | With `on`: a press leaves the keyboard focus where it is (the rows of a completion list, under a field that keeps the keyboard) |
| `clip` | bool | What overflows it is cut at its edges (a line of badges that may not fit) |
| `shortcuts` | list of `{ what, key, ... }` | Lines under its `help` in the tooltip, each what a key does with its keycaps: `shortcuts = { { "Open its terminal", "Enter" }, { "Start or stop", "Space" } }` |
| `paint` | `{ strokes = { { points = { x1, y1, x2, y2, ... }, width = 2, color = "blue" } }, dots = { { x =, y =, r =, color =, hollow = } } }` | Vector drawing in the element's own points from its top-left, under its children (Rune's `Shape`: round-capped polylines, dots as filled discs or rings). At most 400 strokes, 600 dots, 8000 points. Give the element its `w` and `h`; put pressable parts over it as `pin`ned children. The Worktrees graph draws with it |
| `folds` | bool | `column`: its children fold open and shut smoothly. A child that grows or shrinks (a block that expands) glides to its new height over about 0.18 s, ease-out, and what it gains or loses fades; only in answer to the person's press or key, never for data that arrives; off under the system's reduced motion |

```lua
row{ gap = 8, w = "full", dot{ tone = "green" }, text{ "running" }, spacer{}, text{ "2" } }
```

A field on a row (an `input` or `select` without its own `w`, `grow` or
`min_w`) takes the room its neighbours leave, down to 160 pt, so a button
beside it never leaves the row: `row{ gap = 8, w = "full", input{...},
button{...} }` needs nothing more. A row holding a control (a button, a
checkbox, a field, a menu) wraps onto a second line when its parts do not fit
side by side, unless it holds a `truncate` text without `grow` (that text is
meant to give way instead); the top bar and the sidebar keep their own fit. A
modal whose screen has no `scroll`, `split` or grown part is as tall as its
content, up to the window's room, then scrolls.

### `column`

Children one under the other. Same props as `row`; `align` is the horizontal
axis and defaults to `stretch`.

```lua
column{ key = "row-docs", on = "open", arg = "docs", on_space = "toggle", hover_group = true, gap = 2, px = 8, py = 8, radius = "sm",
  text{ "docs", weight = "medium" }, text{ "running", size = "sm", color = "text-muted" },
  button{ icon = "terminal", variant = "ghost", on = "bash", arg = "docs", help = "Open a shell", reveal = true } }
```

### `list`

A bordered list: each child padded (12 pt left and right, `gap` above and
below, default 8), with a 1 pt line between children.

| Prop | Value |
|---|---|
| `gap` | number or space: vertical padding of each item |

```lua
list{ text"web-app", text"docs" }
```

### `scroll`

A vertical scroll area (wheel, trackpad, Page Up/Down). It fills the space
its parent leaves unless it has `h` or `grow`. **Its parent needs a bounded
height**: give the columns above it `h = "full"`.

| Prop | Value | Effect |
|---|---|---|
| `gap` | number or space | Between children |
| `stick_bottom` | bool | Opens at the end and follows it while scrolled to the end (logs, chats) |
| `fade` | number | Its top and bottom edges fade over that many points where content is hidden under them, so a cut list shows it scrolls (the plan dialog's threads) |

```lua
column{ h = "full", text"Log", scroll{ gap = 2, stick_bottom = true, text"line 1", text"line 2" } }
```

### `button`

Positional: the label (may be left out when there is an `icon`; then give it
a `help`, which is also its name for screen readers). A button is at least
28 pt square (24 with `dense`), its icon centred; it is a Tab stop and shows
the focus ring (2 pt, blue, 2 pt outside) when the keyboard reaches it.

| Prop | Value | Effect |
|---|---|---|
| `on` | action name | Required unless `disabled`. Called on press |
| `arg` | any value | Passed to the action |
| `variant` | `neutral`, `primary`, `danger`, `ghost` | `neutral` (default): grey; `primary`: green, for the one thing a dialog is for; `danger`: red label; `ghost`: no box until hovered |
| `icon` | icon name | Before the label |
| `color` | colour | The label's and icon's colour |
| `disabled` | bool | Greyed, not pressable |
| `busy` | bool | Its action is running: keeps its colours, a small spinner takes the icon's place (or leads the label), presses are ignored; reads as busy to screen readers; Reduce Motion: the ring rests. Use it, not `disabled`, while a request to the daemon runs; `disabled` wins if both are set |
| `dense` | bool | 24 pt instead of 28, 12 pt label: for bars that are dense on purpose |
| `keys` | list of strings | Its shortcut, drawn as keycaps in its tooltip after the `help` text: `keys = { "⌘", "K" }` |
| `tab_stop` | bool | `false`: left out of the Tab order, still pressable (a row's secondary actions, which its own keys and its menu reach) |
| `hover` | colour | The fill under the pointer, in place of the variant's (a stop button that turns `red-tint`) |
| `tone` | `green`, `red`, `amber`, `blue`, `purple`, `neutral` | Its colour by meaning ([Colour by meaning](#colour-by-meaning)): the icon (and a ghost button's label) in that colour, its tint under it on hover and keyboard focus |
| `compact` | bool | One text line tall: a 20 pt framed face inside a 24 pt target (a row's start, stop and +) |

```lua
button{ icon = "stop", on = "stop", arg = sb.name, color = "red", help = "Stop", keys = { "Space" } }
```

### `input`

A one-line text field. The core holds the text while it is typed; the
screen hears it through its actions.

| Prop | Value | Effect |
|---|---|---|
| `id` | string | Required. Names the field (also its key) |
| `value` | string | The text. Applied when it differs from the last `value` the screen gave, so a screen can clear the field after a submit by rendering `value = ""` |
| `placeholder` | string | Shown while empty |
| `on` | action name | Called on every edit, `value` = the text |
| `on_submit` | action name | Called on Enter, `value` = the text |
| `arg` | any value | Passed to both |
| `secure` | bool | Bullets instead of characters (no copy, no undo) |
| `disabled` | bool | Read only |
| `tone` | `compose` or `plain` | `compose`: a chat input, where the person writes to an agent (the Sessions composer, Jarvis's input, Canvas's ask and remarks, a request's answer). It gets the `compose` ground, raised above the panel, and dictation (F9 held, or the `dictate` intent) writes there. Filters, searches and forms stay `plain` (the default) |

```lua
input{ id = "filter", value = state.filter, placeholder = "Filter", on = "filter" }
-- actions.filter = function(state, _, text) state.filter = text end
```

### `textarea`

A text field of several lines: Return is a new line, Cmd+Return (Ctrl+Return
off macOS) submits. With `enter = "send"`, Return submits too and
Shift+Return is the new line. Either way a held Return submits once, and a
Return that confirms an input method's composition (IME, dead keys) never
submits. The core holds the text, as for `input`.

| Prop | Value | Effect |
|---|---|---|
| `id` | string | Required. Names the field (also its key) |
| `value` | string | The text, applied as `input`'s |
| `placeholder` | string | Shown while empty |
| `rows` | number | Visible lines, 1 to 40 (default 4); more text scrolls |
| `on` | action name | Called on every edit, `value` = the text |
| `on_submit` | action name | Called on Cmd/Ctrl+Return (and Return with `enter = "send"`), `value` = the text |
| `enter` | `newline` or `send` | What a plain Return does: a new line (the default), or `on_submit` (Shift+Return is then the new line). Only the Sessions composer sends on Return: a chat box where one message is the norm. Every other field keeps the default |
| `on_list_key` | action name | Set while the screen shows a completion list for this field: plain Up, Down, Tab, Return and Escape call it with `"up"`, `"down"`, `"tab"`, `"enter"`, `"escape"` (and the same `arg`) instead of editing or submitting; the keyboard stays in the field. Leave it unset when no list shows |
| `arg` | any value | Passed to both |
| `disabled` | bool | Read only |
| `tone` | `compose` or `plain` | `compose`: a chat input, where the person writes to an agent (the Sessions composer, Jarvis's input, Canvas's ask and remarks, a request's answer). It gets the `compose` ground, raised above the panel, and dictation (F9 held, or the `dictate` intent) writes there. Filters, searches and forms stay `plain` (the default) |

```lua
textarea{ id = "answer", value = state.answer, rows = 5, placeholder = "Your answer", on = "typed", on_submit = "send" }
```

### `image`

A picture a feature gave the core (`Action::Image { key, bytes }`): the core
decoded it, with its limits, so a screen never touches the bytes. Drawn at its
own size, scaled down to fit `max_w` and `max_h`. A key nothing registered
draws "picture not loaded".

| Prop | Value | Effect |
|---|---|---|
| `key` | string | Required. The picture's key, as the feature registered it |
| `max_w`, `max_h` | number | The largest size it is drawn at |
| `fit` | `fit` or `fill` | Letterboxed (default) or cropped to the box |

```lua
image{ key = "shot-web-app-1", max_w = 320, max_h = 200 }
```

### `surface`

A feature's live picture (a sandbox's browser): drawn and fed by the core
(`src/surface.rs`), it takes the space its parent leaves (put it last in a
`column` that fills the panel). The picture is scaled to fit, aspect kept,
letterboxed. A press gives it the keyboard; the person's pointer, keys,
input-method text and paste go to the owning feature's Rust in the picture's
own pixels, never to the screen. The app keeps its own keys: the global
shortcuts, push-to-talk, F6 (the feature may move the focus to its own
field), ⌘ or Ctrl with + - = 0, and the release key, ⌘Esc on macOS and
Ctrl+Esc elsewhere, which gives the keyboard back: say it in the chrome
(`data.app.surface.release`). The screen draws only the chrome around it; an
`id` no feature owns draws a grey line saying so.

| Prop | Value | Effect |
|---|---|---|
| `id` | string | Required. The surface's id, as its feature declares it in `surfaces` |

```lua
surface{ id = "browser" }
```

### `canvas`

A feature's canvas (Canvas's review surface): pictures, numbered pins and
note cards on an infinite surface, drawn by the core
(`src/surface_canvas.rs`). The person pans (drag the ground, the wheel, two
fingers, the middle button) and zooms (pinch, ⌘ or Ctrl with the wheel)
without the screen running; presses that mean something (select, move,
resize, place a pin, a note, an example picture) go to the owning feature's Rust as
`SurfaceEvent::Canvas`, never to the screen. It takes the space its parent
leaves. An `id` no feature declares in `surfaces` draws nothing and hears
nothing. Each picture sits on a mat (`canvas-mat`, a `canvas-mat-edge`
hairline); a selected item has an accent ring outside it, and a resize
handle on its corner only when it is drawn large enough (8 % of its smaller
side, 6 to 12 pt). Fit leaves room for the round bands above and the
captions below.

**The ground** is the scene's choice (Canvas's bar: `canvas_ground`, kept as
`features.canvas.ground`): White (the default), Dark (the theme's `bg`), or
Follow theme (Dark for the dark themes; a light theme, if one comes, picks
White). Everything on it is drawn in the canvas's own tokens (`canvas-*` in
`theme.rs`): a fixed light palette on white, the active theme's tokens on
dark, so the dark ground looks as the canvas always did. Their contrast is
`theme::tests::canvas_pairs_reach_the_level_on_both_grounds`.

**The dot grid** sits inside the world, under every item, so a live pan or
pinch moves it without a build. Dots are 24 world points apart at 100 %;
the spacing doubles as you zoom out and halves as you zoom in, so it stays
16 to 32 pt on screen. A dot is 1 pt in radius on screen (1.25 on dark,
`border` at 70 %). One build draws only the dots of the view and a 50 %
margin, as one shape (a zero-length round line per dot, one small GPU quad
each), at most 16,000: the margin shrinks first, and a view too large even
without it gets a coarser spacing. A 2560 x 1440 pt full-screen view at
16 pt is about 14,600 dots. With `SBXW_DESKTOP_PERF=1` each build logs
`perf: canvas dots: <n> at <step> pt <ms> ms`. The outgoing spacing does not
fade yet when the level changes.

| Prop | Value | Effect |
|---|---|---|
| `id` | string | Required. The canvas's id, as its feature declares it in `surfaces` |

```lua
canvas{ id = "canvas" }
```

**Its own window.** The Canvas view can open in a second window
(`canvas_window {to = "window"}`: its bar's "Open in a window", ⌃⌥O on
macOS and Alt+O elsewhere, the palette), which goes full screen with ⌃⌘F
on macOS (F11 elsewhere), the green button, or its "Full screen" button.
Asking again brings that window to the front: there is one canvas window.
The view is in one place at a time: while it has its window, the main
window's Canvas overlay says so ("Open in its window", "Bring it back
here"), and closing the window, or `canvas_window {to = "main"}`, shows it
over the panes again. The screen itself is the same file: its Luau state
moves to the window for the window's turns (`src/windows.rs`), so nothing
here changes. `canvas_ui.windowed` is true there, and `canvas_ui.win_w`
is the window's width (the screen lays out by it instead of
`app.overlay.w`). A confirmation asked for in that window (Send, Ask,
Delete) is the core's, drawn and answered in that window; one that rises
in the main window (or Jarvis's proposal, the copy offer, the palette)
holds the canvas window behind a scrim that says where to answer.

**Diagrams.** A diagram an agent added (`canvas_add_diagram`, sbxw's
schema: `crates/core/src/diagram.rs`) is one item of the world, at its
place, moved like a shot (its origin goes through the edit route's
`layout`). The core draws it (`src/surface_diagram.rs`): nodes by shape
(process a rounded box, decision a diamond, start and end round, store,
external, actor, note), their label and second line, edges routed at draw
time as orthogonal elbows with an arrowhead (solid, dashed, emphasis,
error), lanes as bands, groups as frames, a sequence as lifelines with its
messages in rows. Every text is the agent's, cleaned and cut as the core
does, drawn as text. Below 30 % zoom the labels are left out. Each node is
a button named by its label; each edge is named "A to B: label". With the
Select tool: drag a node (its edges follow; a lifeline moves sideways
only); drag from the selected node's handle to another node (a new edge)
or to empty ground (a new node, linked); drag a message up or down (a new
row); double click a node or an edge to rename it in place (Enter keeps,
Escape drops); double click the diagram's ground for a new node. A press
gives the surface the keyboard: Enter renames, Tab adds a linked successor,
Delete or Backspace removes (a node takes its edges), ⌘Z and ⇧⌘Z undo and
redo (100 steps a diagram, kept by the feature). Each edit is checked
against the core's limits before anything is sent, then the whole diagram
is saved (`POST /api/canvas/:id/diagrams/:did`, one save in flight, edits
made meanwhile going in the next); a 409 shows "Reload" and "Reapply my
change". "Lay out again" sends every node without a place, so the
daemon's own layout places them: one undo step. The screen's bar for the
diagram (`canvas_ui.diagram`, the `canvas_diagram` intent) stays while the
canvas holds diagrams, so the surface never moves when one is selected.

### `icon`

Positional: an [icon](#icons) name.

| Prop | Value | Default |
|---|---|---|
| `color` | colour | `text` |
| `size` | number | 14 |

```lua
icon{ "folder", size = 11, color = "text-muted" }
```

### `badge`

A small framed label. Positional: its text.

| Prop | Value | Default |
|---|---|---|
| `tone` | `neutral`, `blue`, `green`, `amber`, `red`, `purple` | `neutral` |
| `pill` | bool | A round bubble holding a count alone: the tone's colour filled, the number in the page's dark (the list's network count) |

The web UI's meaning: blue is focus or "a pane", amber "needs you", green
"done" or running, red an error.

```lua
badge{ "⚑ 2", tone = "blue", help = "sbx waits for you to decide something" }
```

### `dot`

A 7 pt status dot.

| Prop | Value | Default |
|---|---|---|
| `tone` | as `badge` | `neutral` (grey) |

```lua
dot{ tone = sb.status == "running" and "green" or "neutral" }
```

### `split`

Two children, the first taking `ratio` of the length, with a 6 pt handle
between them that the person drags. The core keeps the ratio from then on.

| Prop | Value | Effect |
|---|---|---|
| `id` | string | Required. The handle's key is `<id>-handle` |
| `vertical` | bool | true: side by side (a vertical handle); false (default): one above the other |
| `ratio` | 0.1 to 0.9 | The first child's share when the split first shows (default 0.5) |
| `min_a`, `min_b` | points | The least length the first, the second child keeps (default 0: only the 0.1 to 0.9 range). The handle stops there, a ratio stored before (or in a larger window) shows inside it, and the layout never gives less. When both cannot be kept, the first keeps its minimum and the second takes what is left |

```lua
split{ id = "detail", vertical = true, ratio = 0.35, column{ text"list" }, column{ text"detail" } }
```

### `spacer`

Empty. With no `w`, `h` or `grow` it grows (pushes the next children to the
end of a row); with `w` or `h` it is a fixed gap.

```lua
row{ text"left", spacer{}, text"right" }
```

### `divider`

A 1 pt line across the parent.

| Prop | Value | Default |
|---|---|---|
| `vertical` | bool | false: horizontal, full width; true: vertical, full height |
| `color` | colour | `elevated` |

```lua
column{ text"above", divider{}, text"below" }
```

### `menu`

A button that opens a list of choices drawn by the core, above everything.
Positional: the button's label.

| Prop | Value | Effect |
|---|---|---|
| `id` | string | Required. The button's key (unless `key`); the list is `<id>-list`, each item `<id>-<item id>` |
| `items` | list of items | Each a string (id and label) or `{ id = "...", label = "...", icon = "...", disabled = bool, help = "...", tone = "red", keys = { "Space" } }`, or `{ divider = true }`: a line between two groups |
| `on` | action name | Required unless `disabled`. Called with `value` = the picked item's id |
| `arg` | any value | Passed too |
| `icon` | icon name | On the button |
| `variant` | `ghost` | No frame around the button (and, with no label, no chevron: an actions menu); anything else: framed |
| `disabled` | bool | |
| `dense`, `keys`, `tab_stop` | as `button` | 24 pt; the shortcut in its tooltip (its `help`); out of the Tab order |

The core draws every menu the same way (the row menus, selects, the pane
bars' overflow, the layout picker): 6 pt padding, 28 pt rows, the icons in
one column 12 pt before the labels, as wide as its longest row up to 320 pt
(a longer label truncates; its tooltip has it whole). Write labels short,
verb first ("Open shell", "Network", "Requests (3)", "Manage…", "Stop") and
put the explanation in the item's `help`, its tooltip. A `tone` colours the
item's icon by meaning; `red` (destructive) colours its label too. `keys`
are drawn as keycaps at the row's right end. It fades in over 0.1 s (at once
under reduced motion).

The list closes on a pick, on Escape, on a second press of the button, or
on a press anywhere outside it (light dismiss, below). Up and Down move
between its items, Enter picks.

**Light dismiss, one rule.** A press outside closes, and does nothing else
(as a native menu does: it never reaches a terminal, a row or a button
under it): every menu and select, the pane bars' overflow, the command
palette, and a modal that holds nothing the person may have typed or
changed (Help; its feature declares `Slot::Modal { light_dismiss: true }`).
A modal with fields (New sandbox, the env file, templates, Setup, a plan,
New thread, the sandbox actions) does not close on a press outside: only its
cross, its Cancel and Escape close it, so a stray click loses nothing; that
is the default (`light_dismiss: false`). The core's approval dialogs keep
their own behaviour.

```lua
menu{ "Actions", id = "acts", icon = "more", on = "act", arg = sb.name,
  items = { { id = "bash", label = "Open a shell", icon = "terminal" }, { id = "stop", label = "Stop", icon = "stop" } } }
```

### `select`

A picker showing the current choice. Same list as `menu`.

| Prop | Value | Effect |
|---|---|---|
| `id` | string | Required (keys as `menu`) |
| `items` | list of items | As `menu` |
| `value` | string | The current item's id; its label shows, with a check in the list |
| `placeholder` | string | Shown when `value` matches no item |
| `on` | action name | Required unless `disabled`; `value` = the picked id |
| `arg` | any value | Passed too |
| `disabled` | bool | |
| `scope` | `"sandbox"` or a table | A scope picker, drawn by the core (below) |
| `switch` | bool | With exactly two items: a two-position switch drawn by the core (`src/switch.rs`): one track, both labels, a thumb that slides under the chosen one (the top bar's GUI \| TUI). A press on a label picks it, Space or Enter flips, Left and Right move, the thumb drags; `on` gets the picked id. Any other number of items: the plain select |

```lua
select{ id = "layout", value = "2c", on = "layout",
  items = { { id = "1", label = "1 terminal", icon = "single" }, { id = "2c", label = "2 side by side", icon = "columns" } } }
```

#### Scope pickers (`scope`)

Which sandbox a screen shows is its most important fact, so every scoped
screen draws it the same way, with one component the core draws
(`src/scope.rs`): `select{ ..., scope = "sandbox" }`.

```lua
-- In an overlay's title row ("Files in [web-app v] > thread-a x"):
row{ key = "title-slot", gap = 8,
  select{ id = "files-sandbox", value = ui.sandbox, items = ui.sandboxes, on = "sandbox",
    scope = { kind = "sandbox", narrow = thread, narrow_icon = "branch", clear = "branch_clear" } } }
```

- **The data shape**: `items` only says which sandboxes the screen offers
  (`{ id = <sandbox name> }`; labels are ignored); an item with `id = ""`
  is **All sandboxes** (its label, if any, is the text), listed first, and
  only a screen that has it offers it. Everything a row shows comes from
  the core's own data: status, group, group colour (the sidebar's), agent,
  open sessions. A name the core does not know is left out. `on` gets the
  picked id (`""` for All), as a `select`.
- **`scope` table**: `kind` (`"sandbox"`; another kind draws the plain
  select), `narrow` (a second level drawn after the capsule: a thread's
  worktree, a branch), `narrow_icon` (an icon name, default `branch`),
  `clear` (the screen's action its cross calls; no cross without it).
- **Counts and order** (Worktrees: `scope = { kind = "sandbox", all = true,
  counts = ui.counts, noun = "worktree", only = "counted", order = "count",
  counting = ui.loading }`): `counts` (`{ [sandbox] = n }`, the screen's
  number; the row's chip shows it in place of the open sessions, its
  tooltip "4 worktrees" from `noun`), `only = "counted"` (only the
  sandboxes counted above 0; the current one stays, its chip "no worktree"),
  `order = "count"` (the largest first, ties by tier, then by name; no group headings,
  each row keeps its group bar and names its group in dim text), `counting`
  (while the counts are read, an empty list says "Counting the worktrees…"
  instead of "No sandbox has a worktree"). `only` and `order` need `counts`.
  The filter field shows above 8 listed sandboxes.
- **Order** (every screen, by default): the Sandboxes sidebar's. Each
  sandbox's tier is the core's `scope::tier` (1 running with a live
  session, a session busy on background work included; 2 running without
  one, or starting; 3 stopped, stopping, failed, unreachable), the one rule
  the sidebar reads too (as `data.app.tiers`, `{ [sandbox] = 1 | 2 | 3 }`).
  A heading per sidebar block (Chats, a workspace of two or more), the block
  ranked by its best member and before an ungrouped sandbox of the same
  tier; a workspace of one is ungrouped, and ungrouped sandboxes after a
  group get a hairline (`<id>-ungrouped-<n>`). Inside a tier, the daemon's
  order (by name). The order is taken when the list opens and holds while it
  is open (a state change updates the word, never the row's
  place); the filter keeps it. `order = "name"` asks for the daemon's order
  grouped by workspace instead (no built-in screen does).
- **Closed**: a capsule in the sandbox's group colour (its fill the colour
  at 32% over `surface`, its edge the colour itself), the status dot (a
  ring when stopped), the name (clipped at 220 pt, whole in the tooltip),
  a chevron. All is neutral with a grid glyph; an empty value with no All
  shows the `placeholder`. A change of value plays one short pop of the
  name (none under reduced motion). It is a Tab stop (`combobox`).
- **In an overlay's title** (a `title-slot` row): the title reads "Files in
  [web-app]" ("in" dropped under 560 pt) and the overlay's frame takes the
  scope's colour: a 3 pt strip on its top edge (the group colour), the title
  band in a focused pane header's tint (`theme::bar_ground`), its edge.
  All, or a sandbox in no group: today's neutral frame.
- **Open**: the list under the capsule (`<id>-list`, 300 to 420 pt, never
  wider than the window; at most 420 pt tall and never past the window: its
  rows, `<id>-rows`, scroll inside under the pinned filter and All, the
  current row in view on open): All first, then each group under a heading in
  the sidebar's tint and label colour, a row per sandbox (`<id>-<name>`,
  All `<id>-`; a script's `wait:` knows `<id>-list`, then taps a row): the group's bar, name, open sessions, state
  (a word, no dot: the capsule keeps its dot, its only visible sign of the
  state), agent, a check on the current one. Above 8 sandboxes a filter field
  (`<id>-filter`) that matches names and group names. The keyboard starts
  in the field, else on the current row; Up and Down move, Enter picks
  (in the field: the first match), Escape closes only the list and gives
  the keyboard back to the capsule; a press outside closes it and reaches
  nothing else.
- **The sidebar**: while an overlay is scoped to a sandbox, that
  sandbox's row shows an eye before its buttons ("Shown in Files",
  `in-view-<name>`), from `data.app.overlay.scope` (`{sandbox, title}`,
  null for All or no scope). It follows a pick at once (the core pushes
  again when the overlay's scope moved) and goes when the overlay closes.
- **The status bar's versions** (`status-versions`, with `status-version-sbx` and
  `status-version-sbxw`): at the very left, "sbx 0.54.0" and "sbxw 4.1.3" from
  `data.daemon_state.versions`, caption size, muted, set apart by a gap. One the
  daemon does not send is left out, no placeholder. They are the first thing a
  narrow bar drops (before the bring-up's step label).

- **The status bar**: its git item (`status-git`, `src/shell/statusbar/git.rs`)
  names the repository and branch of the scope's sandbox, as
  `<repository>/<branch>` (its main checkout's branch, from the daemon's
  worktrees read). With scope All, or a screen about no sandbox, it is
  absent. Read only; no press.
- **Names are data**: text only, clipped, never a pill or a control.
- A picker a screen must draw itself (the Dashboard's two-line team rows) takes
  the same colours from `scope::look_json` (`fill`, `border`, `text`,
  `muted`, as `#rrggbb`) in its view model.

### `checkbox`

Positional: its label.

| Prop | Value | Effect |
|---|---|---|
| `value` | bool | Checked or not |
| `on` | action name | Required unless `disabled`; `value` = the new state (the opposite of `value`) |
| `arg` | any value | Passed too |
| `disabled` | bool | |

```lua
checkbox{ "Read only", value = state.ro, on = "toggle_ro" }
-- actions.toggle_ro = function(state, _, on) state.ro = on end
```

### `progress`

A 4 pt bar.

| Prop | Value | Default |
|---|---|---|
| `value` | 0 to 1 | 0 |
| `tone` | as `badge` | `blue` |

```lua
progress{ value = step / steps, tone = "green" }
```

### `markdown`

Text written in Markdown, drawn as it reads: paragraphs (blank lines between,
line breaks kept), `#` headings, `-` / `*` / `1.` lists, fenced code blocks
(```` ``` ````, monospace, wrapped), and inline `code`, `**bold**`,
`*italics*`, `[links](https://...)` and bare `https://` URLs. Code spans are
read first, so bold can hold code (`**run `ls`**`). Nothing else is
interpreted: no HTML, no images, no tables, and only `http` and `https` are
links (any other link shows its label as text). Positional: the string.

| Prop | Value | Effect |
|---|---|---|
| `color` | colour | Text colour (default `text`; headings are `text-bright`) |
| `size` | number or text size | Text size (default 13) |
| `on` | action name | Called when a link is pressed; `value` = its URL. Without `on`, links are drawn but do nothing |
| `arg` | any value | Passed too |
| `pills` | `true` or a sandbox name | Keyword pills in its prose (not in code, links, bold or italics), as `text`'s `pills` |

```lua
markdown{ "Run **`sbxw up`** first, then see [the docs](https://example.org/sbxw).", on = "link" }
-- actions.link = function(_, _, url) return intent("open_link", { url = url }) end
```

### Keyword pills

Agents write a short label in double brackets beside what it describes
(`[[Done]]`, `[[Failed: 3 tests]]`, `[[Needs you]]`); `text` and `markdown`
with `pills` draw each as a pill. The rule is emphasis-core's, the same in
the terminal, the web UI and here (`desktop/src/emphasis.rs`): a word of 1 to
32 letters, digits, spaces or hyphens (a letter first, a letter or digit
last), an optional detail after `:` of 1 to 48 characters with no bracket,
one line. The vocabulary is the daemon's (`data.emphasis.words`); a word it
does not know is a pill in the neutral blue only when it starts with a
capital (`[[summary]]`, a code map's link, stays text). Tones: green settled,
red wrong, amber needs you, blue context, purple Jarvis and the team, cyan the
kind of work, and `danger` a filled red for what cannot be undone.

**Look** (views2, the same in the terminal and here): a capsule (radius half
its height; square where a soft wrap cuts it), 1.5 pt of air above and below
in the terminal so pills on consecutive lines never touch; the fill is the
tone at 10% over whatever is under it (the terminal's ground, or a coloured
line's own background, a diff's band); the word is the body text moved 45%
toward the tone, the detail the body text, both regular weight, each moved
until it reads at 4.5:1 on the fill; `danger` stays a filled red with white
text. `emphasis::TINT`, `WORD_MIX`, `AIR`, `span_radius`.

```lua
text{ r.question, wrap = true, pills = r.from }   -- the sandbox that wrote it
markdown{ m.text, on = "link", pills = true }
```

**Text, never anything else.** A screen gets a pill only as part of one
text element: the core turns the string into characters and styles (colour,
a rounded background) of that element. No marker can make a
control, a link, a key or an intent appear, whatever it holds (markup, an
intent's spelling, a URL): the parts stay characters
(`shell::layout_tests::pills_in_a_screen_never_make_a_control`). Pills are
never drawn in tooltips (`help`): they show the marker as written.

## Events and actions

Every event calls an action of the screen: **`action(state, arg, value)`**.

| Element | Event | `arg` | `value` |
|---|---|---|---|
| `button`, `row` / `column` with `on` | press | its `arg` | nil |
| `input` | `on`: each edit; `on_submit`: Enter | its `arg` | the text |
| `menu`, `select` | pick | its `arg` | the item's id |
| `checkbox` | press | its `arg` | the new bool |
| `markdown` with `on` | a link pressed | its `arg` | the link's URL |

An action may change `state` (the screen renders again) and may **return**:
nothing, one intent, or a list of intents. Anything else is an error. A
button inside a pressable row calls only the button's action.

## Intents

`intent(name, args)` makes one. The list is closed: the core knows each
intent's arguments and their types, checks them, and refuses the **whole
batch** an action returned if one is wrong (unknown intent, unknown argument,
missing or mistyped one). The error shows in the screen's banner; nothing runs.

| Intent | Arguments | What the core does |
|---|---|---|
| `open_terminal` | `sandbox` (string, required), `mode` (`agent` default, `bash`, or `kit-add`: the pane where sbx's prompt for a waiting kit is answered), `thread` (string), `pane` | Opens that terminal in a pane and focuses it. The agent of a stopped sandbox starts it (the daemon runs `sbx run`); the list then shows it running |
| `start_sandbox` | `sandbox` (required), `pane` | Starts a stopped sandbox by opening its agent, as the web UI's Connect does |
| `stop_sandbox` | `sandbox` (required) | Closes its terminals, shows it `stopping…`, asks the daemon to stop it; the list follows |
| `select` | `sandbox` (string; none: nothing selected) | Sets `data.app.selected` |
| `sidebar` | `show`: `show`, `hide` or `toggle` | The sandboxes sidebar shown or hidden, remembered across launches (`data.app.sidebar.shown`); the pane area, terminals or Sessions, takes its width (each terminal resized once). Its header's `sidebar-hide` runs it; while it is hidden the rail's first entry `rail-sidebar` ("Show sandboxes") brings it back, its badge the sessions that want the person (amber). A `select` of a sandbox pressed elsewhere (the palette, Worktrees, the create jobs; not one a feature runs from its data) shows it first. No focus moves |
| `team_leads` | `show`: `show`, `hide` or `toggle` | Leads only on or off, the top bar's `top-lead` (`data.app.leads`: `on`, `can` (a team runs), `leaders` while on). On: the terminals show only the team leaders' panes (the layout before comes back when it goes off) and Sessions only the leaders' sessions, each followed by its threads'. One state for both base views, kept in the state file (`lead_mode`) and restored at start when a team still runs. Nothing happens while no team runs |
| `set_theme` | `theme`: `dark`, `graphite` or `toggle` (the next one) | The app's theme, remembered across launches; every surface switches at once (`data.app.theme` and `theme.mode` say which palette is drawn) |
| `set_layout` | `panes` (1 to 4, default 1), `split` (`columns` default, `rows`) | The old layout picker's: that many terminals per page, and the current page that many panes (empty ones added) in the old arrangement |
| `page_show` | `page` (from 1, required) | Shows that page of terminals |
| `page_step` | `back` (bool) | Shows the next page (`back`: the previous one), round |
| `page_new` | none | Adds an empty page at the end and shows it |
| `page_close` | `page` (from 1; none: the current one) | Closes the page: its panes' views close, the sessions go on; the only page is emptied instead |
| `pages_per` | `per` (1 to 6, required) | Terminals per page (1 is the tabbed view). Lowering it moves the overflowing terminals onto the following pages, in order; raising it moves nothing |
| `pages_view` | `view` (`tabs` or `grid`, required) | Tabs: one terminal per page; grid: the last count above one |
| `pages_gather` | none | Puts the open terminals back on as few pages as the count allows, in order; empty panes go |
| `pane_move` | `pane` (from 1; none: the focused one), `to` (`left`, `right`, `up`, `down`, required) | The pane trades places with its neighbour on that side, or, at the page's edge, docks along that whole side |
| `pane_rotate` | `pane` (as above) | The pane and its sibling switch between side by side and stacked (portrait and landscape) |
| `pane_to_page` | `pane` (as above), `page` (from 1; none: a new page) | Moves the pane to that page, which then shows; refused when the page is full |
| `panes_equalise` | none | The current page's splits share their room equally |

These rearrange views only: nothing is started, stopped or created, and a
terminal keeps its socket, history and size owner wherever its pane goes
(it fits once to its new place).
| `copy_text` | `text` (string, up to 100 KB) | Puts it on the clipboard; a notice says how much |
| `close_panes` | `sandbox` (string), `thread` (string; none: the sandbox's own session) | Empties the panes showing that session, terminals closed; with several panes shown, such a pane goes and the layout shrinks (after a thread is closed) |
| `toggle_jarvis_panel` | `open` (bool; none: toggles) | Opens or closes the Jarvis panel docked on the right; it narrows the terminals |
| `start_jarvis` | none | Creates Jarvis's sandbox the first time (a minute or two), or starts it, and opens its session; `data.jarvis.status` follows |
| `send_to_jarvis` | `input` (the `id` of this screen's input, required) | Sends what the person typed there to Jarvis, masked first. **The core reads the text itself**, from its own copy of the input: a screen cannot pass words of its own. Emptied once sent; kept, with the reason in the log, when it could not leave. A `yes` / `oui` or `no` / `non` typed and submitted with Enter answers the waiting proposal instead: a yes approves only a low-risk one, else the log says a click is needed |
| `talk_start`, `talk_stop` | none | Push-to-talk from a button: start recording; stop, transcribe, send to Jarvis (as `send_to_jarvis`, spoken) |
| `dictate` | `input` (the `id` of this screen's chat input, `tone = "compose"`, required) | The microphone beside a chat input: starts dictating into it, or stops. The words are transcribed and masked by the daemon, then put in the field at its caret as if typed (its `on` action fires): never sent. Refused in Rust under a dialog, a modal, Jarvis's proposal or the palette, while Jarvis listens, without voice, or into a disabled field; Escape cancels. The screen reads `data.app.dictation` (`recording`, `transcribing`, `notice`, each with `screen` and `input`; `key`, `available`) |
| `read_aloud` | `id` (a `log` entry's id) | Reads that message aloud |
| `stop_reading` | none | Stops reading aloud |
| `approve_proposal` | `id` (string) | **Risk: approval.** Opens the core's dialog for that proposal. It approves nothing: only the person's click on the dialog does |
| `refuse_proposal` | `id` (string), `reason` (string) | Refuses that proposal; Jarvis is told the reason |
| `open_link` | `url` (an `http://` or `https://` URL) | Opens it in the browser. Any other scheme is refused |
| `open_modal` | `screen` (required), `arg` (string) | Shows a feature's dialog screen over the window, one at a time; Escape or a press outside closes it. The screen reads `data.app.modal.screen` and `data.app.modal.arg` |
| `close_modal` | none | Closes the dialog screen |
| `open_panel` | `screen` (required) | Opens a feature's panel docked on the right, beside Jarvis's; several open show as tabs, one shown at a time |
| `close_panel` | `screen` (none: the one shown) | Closes that panel |

Features (`src/feat/`, see [Adding a feature](#adding-a-feature)) add
their own rows: each is listed, with its arguments, risk and what it does,
in `intents.json` next to the built-in screens, and in its feature's
section below.

`pane` is `"focused"` (default), `"spare"` (the first pane with nothing in
it, or a new pane while there are fewer than 4) or a number 1 to 4.

```lua
actions = {
  open = function(_, name)
    return { intent("select", { sandbox = name }), intent("open_terminal", { sandbox = name }) }
  end,
  stop = function(_, name) return intent("stop_sandbox", { sandbox = name }) end,
}
```

Each intent has a declared risk in the core. All of the above run when a
screen asks, except `approve_proposal`: an intent of risk *approval* is held,
and **the core** draws the question above every screen (for a Jarvis
proposal: its summary, risk, every change with its diff, Approve and
Refuse). A screen can neither draw that dialog, nor see it, nor press it.

The push-to-talk key (F8 by default, the daemon's `[voice]
push_to_talk_key`) and the panel's key (⌘J on macOS, F7 elsewhere) are the
core's: a terminal with the focus does not receive them.

## Keyboard

The core owns the app's chords (`src/shell/palette.rs`), and screens never
see them: Cmd/Ctrl+K the command palette (every sandbox, every top bar extra
and feature shortcut, the core's actions; a pick runs the same intents your
buttons run, through the same checks), Cmd/Ctrl+B the sandboxes sidebar
(the left rail stays and shows "Show sandboxes"; its header's button hides it too), Cmd/Ctrl+1 to 9 a pane of the current page, Ctrl+Tab and
Ctrl+Shift+Tab the next and previous page (also Cmd+Option+Right/Left on
macOS; in a terminal Ctrl+Tab was a plain Tab, so nothing is lost), Cmd/Ctrl+Shift+W closes the focused pane, Cmd/Ctrl with + − 0 zooms the
app (in a terminal: its text), Escape closes the top-most menu, palette,
dialog or modal. While a terminal's grid has the keyboard only Cmd chords
(macOS) or Ctrl+Shift chords (elsewhere) are the app's. `data.app.shortcuts`
lists them all (Help draws it). What a screen does for the keyboard:

- every `button`, `menu` and pressable `row`/`column` is a Tab stop in
  reading order, with the focus ring; leave a row's secondary actions out
  with `tab_stop = false` when the row's own keys and its menu reach them;
- a list of rows: `group = "list"` on the container (Up and Down), `on` on
  each row (Enter), `on_space` for its second action (Space);
- name every icon-only control with `help`, and give its shortcut in `keys`.

### Hover zones

Elements with the same `hover_zone` name are one zone: moving the pointer
from one to another is not leaving it. `data.app.hover_zone` names the zone
the pointer rests in, with delays the core keeps, so a screen only reads it:

- a zone becomes the one **180 ms** after the pointer *moved* inside it
  (`screen::ZONE_OPEN`). An element that slides under a still pointer,
  because something above it opened, does not count: no chain of blocks
  opening each other. A pointer sweeping down a list opens nothing;
- it stops **280 ms** after the pointer left every element of the zone
  (`screen::ZONE_CLOSE`);
- one zone at a time: resting on another makes that one the zone.

The screen draws again when it changes, and what opens or shuts on it glides
like a press (at once under reduced motion). Hover is never the only way in:
keep a press (a disclosure line) and the keyboard (the selected row) doing
the same. The sandbox list opens a sandbox's sessions on its zone (its row,
the sessions line and block, `sb:<name>`).

## Theme

The global `theme` (read only) is the app's design tokens (the dark
theme's are the web UI's, `assets/app.css`). Any colour prop takes these
names; `theme.color[name]` gives the `#rrggbb` of the theme drawn now.

**Themes.** The app has named themes, each one palette behind the same
token names: `dark` (the default, the web UI's blue-black) and `graphite`
(neutral dark greys, an orange accent). The person switches them in the
status bar; `theme.mode` (and `data.app.theme`) says which is drawn, and on a
switch every screen is loaded again and renders with the new values. So:

- **Use token names, never literal colours.** `color = "text-muted"`,
  `fill = "surface"`, `border = "accent"`. A `#rrggbb` written in a screen is
  the same in every theme and will look wrong in all but one (a dark grey
  that was a surface in Dark is a hole in Graphite). The built-in screens
  have none (a test scans them).
- Mixing your own tints from `theme.color` values is fine: they are read
  again on a switch. The core hands the list's group tints ready made
  (`data.app.group_look[colour]`: `tint`, `hover`, `label`).
- For "the current one", a focus or a link, use `accent` (`link` for link
  text); for meaning, the tones (below). `accent` is blue in Dark, orange in
  Graphite; never use it for a state.
- A customised copy of a screen (the override folder) made before themes
  existed may carry literal colours: it then looks wrong in Graphite. Ask
  Jarvis to replace them by tokens, or restore the built-in.

| Table | Names |
|---|---|
| `theme.color` | `bg`, `surface`, `surface-alt`, `elevated`, `border`, `dim`, `text`, `text-bright`, `text-muted`, `text-soft`, `blue`, `blue-tint`, `green`, `green-strong`, `green-deep`, `green-tint`, `red`, `red-border`, `red-tint`, `amber`, `amber-strong`, `amber-tint`, `purple`, `purple-strong`, `purple-alt`, `purple-tint`, `group-1` to `group-4`, `hover-subtle`, `active-bg`, `white`, `floor`, `pane-bar`, `focus`, `control-border`, `primary` (the one filled button's fill), `on-primary` (its label), `primary-hover`, `red-text` (red words on a group's tint), `status-bar`, `status-line`, `vendor-anthropic`, `vendor-openai`, `vendor-google`, `cyan`, `pill-danger`, `accent`, `accent-tint`, `link`, `tab-current`, `scrim`, `shadow`, `media` (behind a picture), `file-doc`, `file-code`, `file-image`, `file-data`, `file-archive` (the explorer's file kinds) |
| `theme.mode` | `dark` or `graphite` |
| `theme.icon` | `sm` 12 (inline with text), `md` 14 (every control), `lg` 20 (the rail), `mark` 24 (the top bar's mark) |
| `theme.radius` | `mark` 2 (8 pt squares, thin bars), `box` 4 (a checkbox, a keycap), `xs` 6, `sm` 6, `md` 10, `lg` 12, `pill` 999 (dots, chips) |
| `theme.text` | `caption` 12, `body` 13, `title` 15, `heading` 17; older names `xxs`, `xs`, `sm` 12, `md` 13, `lg` 15, `xl` 17 |
| `theme.space` | `xxs` 2, `xs` 4, `sm` 8, `md` 12, `lg` 16, `xl` 24 |
| `theme.line_height` | 1.4 (a multiple of the size) |
| `theme.layout` | `top_bar_h` 44, `gutter` 8, `sidebar_w` 264 (`sidebar_min` 200, `sidebar_max` 420), `pane_bar_h` 32, `control_h` 28, `dense_h` 24, `traffic_lights_w` |

### Colour by meaning

Actions carry a colour by what they mean, the same everywhere, so a glance
reads the window before the words do. Give a `button` (or a menu item) a
`tone` and the core draws its icon (and a ghost or neutral button's label)
in it, with its tint under the pointer and the keyboard focus.

| Tone | Means | Examples |
|---|---|---|
| `green` | start, open, allow, confirm | Start, + (open in a new pane), Open shell, Allow, Add rule, Publish, New |
| `red` | stop, remove, refuse, anything destructive | Stop, Remove, Refuse, Unpublish, Close thread |
| `amber` | needs you | a request waiting, sbx holding something, a session that wants you |
| `blue` | go to, informative | Network, Requests, Threads and plans, Project, Browser, Remote, the network count |
| `purple` | Jarvis and the team | Jarvis, a lead |
| `neutral` | everything else | Manage…, Setup, Help, Later, layout |

- The words and the icon's shape always say it too: colour is never the
  only signal.
- **One filled button per view**: the `primary` (green, filled) is the one
  thing a dialog is for (Create, Save, Send my answer, Approve). Everything
  else is tinted, not filled: in the Network panel each card's Allow is a
  green-toned button, not a filled one, so a list of ten requests is not ten
  primaries.
- Every coloured label or icon meets the contrast test on what it sits on
  (its surface, its tint, a group's tint): `theme::tests`.

**Pane bars and page tabs** (drawn by the core). A pane's bar takes its
sandbox's group colour, the sidebar's own (`App::group_colors`, one function
for both), as a 20% tint (16% unfocused) under a 2 pt line of the colour; a
sandbox with no workspace is on the neutral bar. A bar's actions are its
icon in its colour by meaning and its label in `text-bright`, grouped
left to right with a hairline between two groups (`bar::zone`,
`bar::ORDER`): identity (the grip, the state dot, the name) | the session
(the Claude / Bash switch, Esc, Redraw terminal as the reload icon alone, the microphone) | the team (Threads, a plan
to review, Merge or Retry merge) | attention (wants you, a question, the sbx
shield and its count) | tools (Browser, Project) | Remote (and Close thread)
last. The name carries team mode: on the main session of a running sandbox
it is a pill that turns it on and off (the `lead_start` / `lead_end`
intents, ending it through the same dialog); on, it is purple with the
leader icon before the name, off a quiet outlined pill with no icon. A
thread's pane or a stopped sandbox has no toggle: its name is plain text.
There is no separate Team chip. A narrow pane moves chips into the "⋯"
menu by urgency first (amber, then a count or green: the shield, Merge),
then from the bar's right end (Remote, Project, Browser, Threads, the
microphone, Esc last); the Claude / Bash switch goes only when nothing
else fits, and the name never does. Hover and focus put the meaning's tint
under it; an action whose panel shows for the focused pane is a tinted chip.
A page tab takes the group colour of its first terminal (14%, the current
one 28% with a 1 pt border and a 2 pt line of the colour, or the focus blue
when it has none), other groups on the page as small dots, the state dot
(amber wants you, blue working) ringed so it shows on any tint. Secondary
words on a tint are lifted until they read (`theme::muted_on`), icons until
they mark at 3:1 (`theme::mark_on`): `theme::tests::contrast_on_every_group_tint`
checks every hue a group can take.

**Vendor colours** (a separate axis, not colour by meaning). The sidebar's
model words (a row's line 2, a session line's model) take the colour of
whoever makes the model, so "opus 5.5" and "gpt-5-codex" are told apart at a
glance. Text colour only, regular weight, never a fill or a badge, so the
state word before it stays the strongest thing on the line.

| Token | Value | Who | Matched by (id, display name, any case; else the agent) |
|---|---|---|---|
| `vendor-anthropic` | `#e2875a` terracotta | Anthropic | claude, anthropic, opus, sonnet, haiku, fable, mythos |
| `vendor-openai` | `#e6edf3` (as `text-bright`) | OpenAI | openai, chatgpt, gpt, codex, the o-series (o3, o4-mini) |
| `vendor-google` | `#9aa8ff` periwinkle | Google | gemini, google |

Anything else stays `text-muted`. One function decides (`vendor_of` in
sandboxes.luau); the tooltip keeps the full id and adds "by <vendor>". The
terracotta's hue sits between `red-text` ("unreachable") and `amber`
("wants you"); Gemini's periwinkle (hue 232) is told from the `blue`
"working" word (hue 212) by hue, not only by lightness.
Both rules and the contrast on every ground are tested (`screen::tests`).

**The rules the built-in screens follow** (write yours the same way):

- **Spacing** on the 4 pt grid: 4, 8, 12, 16, 24 (2 only between stacked
  lines of one thing). A test fails a built-in screen that writes another
  number for `gap`, `pad`, `px` or `py`.
- **Type**: 12 pt for second lines, labels and badges (`caption`), 13 for
  body (`body`), 15 for row and panel titles (`title`), 17 for dialog titles
  (`heading`): steps of 1.125 to 1.2 above the body. Nothing under 12.
  Weights 400, 500, 600. **No capitals** for labels: sentence case; weight
  and colour make the hierarchy.
- **Colour**: every text and surface pair meets WCAG AA (4.5:1; 3:1 for
  icons, marks and focus rings): `theme::tests::contrast_of_every_pair_used`
  lists them. One secondary grey, `text-muted`, for labels, hints and
  second lines (an age, a label beside a name): it reads at 4.5:1 on every
  ground, a selected row included. `text-dim`, its former twin, still
  resolves to it for screens written against it; do not write it in a new
  screen. A state is never colour alone: say it in a word too.
- **Radii**: always a name, never a number (a test fails a built-in screen
  that writes one): 6 for controls and rows (`xs`, `sm`), 10 for cards,
  tiles and menus (`md`), 12 for dialogs (`lg`), `pill` for a dot,
  `mark` for an 8 pt square or a thin bar.
- **Motion and elevation** live in the core (`theme::motion`,
  `theme::elevation`): two durations, a fade (0.1 s in, 0.08 s out) and a
  move (0.18 s), all landing at once under the system's reduced motion;
  three shadows, for a switch's thumb, the top bar's notice and anything
  that floats over the window (menus, popovers, dialogs).

- **Targets**: anything pressable is at least 28 pt (24 in a dense bar),
  icon-only buttons have a `help` (their name for screen readers and their
  tooltip) and, when there is one, their shortcut in `keys`.

### Vocabulary

One word for one thing, everywhere (labels, tooltips, notices):

| Say | Not | Meaning |
|---|---|---|
| sandbox | box, sbx (sbx is the tool) | A Docker sandbox sbxw runs an agent in |
| terminal | tty, console, pane (for its content) | What a pane shows: the agent's session or a shell |
| pane | tile, slot | One of the 1 to 4 places a terminal shows in |
| session | conversation, chat (for an agent's) | An agent's run in a sandbox: the main one or a thread's |
| thread | branch, worker | A session working in its own worktree, planned by a lead |
| shell | bash | A plain shell in a sandbox (`Open a shell`) |
| request | approval, ask (for sbx's) | Something sbx or an agent waits on you for |
| wants you | needs attention, blocked | An agent waits for your answer |
| start, stop | run, kill, halt | A sandbox's state change |
| panel | drawer, sidebar (for the right side) | A docked view on the right (Jarvis, Network, Requests, Project, Browser) |
| sidebar | list (for the left) | The sandbox list on the left |

## Icons

`icon{ name }`, and the `icon` prop of `button`, `menu` and items:
`play`, `stop`, `reload`, `plus`, `close`, `terminal`, `chevron-down`,
`chevron-right`, `check`, `warning`, `info`, `gear`, `search`, `folder`,
`file`, `flag`, `ports`, `copy`, `trash`, `more`, `columns`, `rows`, `grid`, `eye`,
`single`, `chat`, `link`, `mic`, `speaker`, `send`, `sparkle`, `cpu`,
`coin`, `help`, `moon`, `appearance`, `branch`, `globe`, `remote`, `key`, `people`, `leader`, `threads`,
`shield`, `prompt`, `ask`, `jarvis`, `tag`, `kanban`, `detach` (a window, an arrow leaving its corner), `download` (an arrow down onto a tray), `monitor` (the virtual Mac: a desktop screen on its stand), `inbox` (Sessions: a list of sessions, the first dot filled), `history` (commits), `logo-prompt`, `logo-mark` (the sbxw mark's
">", for the top bar's logo).

One mark per meaning, everywhere (rail, pane bars, menus, panels, palette):

| Icon | Means |
|---|---|
| `sparkle` | the agent (Claude, or another vendor's) |
| `people` | the team (a session in team mode: the pane bar's Team) |
| `leader` | the team leads (a crown over one person: the rail's Team leads, the pager's mark, the palette) |
| `board` | the Dashboard, a team's conversation: two bubbles answering each other (the rail's Dashboard, its overlays, a sandbox's row menu) |
| `kanban` | the Kanban: three columns of cards hung from one line, each shorter than the one before (the top bar's Kanban button) |
| `jarvis` | Jarvis |
| `threads` | a sandbox's sessions and threads (`branch` is one worktree) |
| `globe` | the sandbox's browser |
| `shield` | network: hosts sbx asks about, rules |
| `ask` | an agent's question waiting for you (Requests) |
| `key` | the environment file (sbxenv.yaml) |
| `remote` | remote control from claude.ai |
| `prompt` | a shell (Bash) |
| `stop` | interrupt or stop |
| `tag` | keyword pills |
| `monitor` | the virtual Mac (rail, its panel, the palette) |
| `inbox` | Sessions: every session, who needs you first (the top bar's GUI) |
| `history` | commits (the Commits view, its rail entry, a pane's menu) |
| `canvas` | Canvas: shots of an agent's screens to review (rail, its overlay) |
| `pointer`, `pin`, `note`, `image` | Canvas's tools: select, a numbered pin, a note card, Add example (a picture of the person's) |
| `minus`, `fit` | zoom out; show everything (Canvas) |
| `detach` | move this page to a new window (the pager's button after its tabs) |
| `download` | download (the Files explorer: a folder's row, the listed folder, Download folder, Download) |

## Where screens live

Built-in screens are compiled into the app (`desktop/screens/<name>.luau`).
A file with the same name in the **override folder** replaces it and reloads
on every save (within about 150 ms); deleting the file brings the built-in
back. The folder is `~/.sbxw/desktop/screens/`, or `$SBXW_DESKTOP_SCREENS`.

Screens: `topbar` (the bar at the top, right of macOS's window buttons),
`sandboxes` (the side panel) and `jarvis` (the panel docked on the right,
shown while `data.jarvis.open`). The terminals, their pane bars, the
pager above them (pages of terminals: `src/shell/pager.rs`), the left rail
(Requests, Network, Team leads, Jarvis, Setup: `src/shell/rail.rs`; the
view switch, Tabs | Grid, is the pager's; Team leads shows only the team
leaders' terminals and, pressed again, the layout before: `src/shell/leads.rs`), the
panel's resize handle, the status bar at the window's foot (bring-ups,
total CPU, the sessions' cost, Help: `src/shell/statusbar.rs`) and every
approval dialog are drawn by the core.

**A page in its own window** (`src/shell/pagewin.rs`). "Move Page to New
Window" (the pager's "terminals per page" menu and its hidden tabs' menu,
the pager's button after the tabs (`pager-to-window-now`: a quiet 24 pt
square as "+", the `detach` mark in muted text, tooltip "Move this page to
a new window" with the chord; disabled at the app's disabled opacity, the
reason as its tooltip, where the menus' row is disabled: Leads only, a page
with no terminal; its room is reserved in the pager's fit, so on a narrow
pager the tabs go compact, then into the hidden tabs' menu, and the button
stays), the palette, ⌘⇧M on macOS and Ctrl+Shift+M elsewhere) moves the current
page and its terminals to a window of their own: the same terminals (no
socket reopens), each shown in one window only, its PTY size the tile that
shows it. That window has the page's tiling and pane bars (the core's
same bar: keys `pane-<n>-…` numbered within the window), a header with
"Full screen" and "Move page back", its own focus and keyboard, and no
pager, sidebar, rail or Jarvis. Its close button, ⌘W, its Window menu's
"Move Page Back to Main Window", the button or the same chord put the
page back as the main window's last page (the main window keeps its own
page and focus); a window close never closes a terminal. Its bars' presses
run in the main window's model through the window hub (`src/windows.rs`),
inside that page. A confirmation or copy offer raised by what was done in
the window is drawn and answered there; everything else stays in the main
window, and the page window then shows a scrim saying where. Its
microphone is disabled ("Dictation works in the main window"). The state
file keeps the page among the main window's pages; the window itself is
not restored at the next start.

**Terminal colours** (`src/shell/term_colors.rs`), the web UI's picker
(`assets/js/term-color.js`). The pager's button right of the "terminals per
page" switch (`pager-colors`: a tiny pane, its bar over its background, and
the `appearance` mark) and the palette's "Terminal colours" open a popover
(`term-colors`) with three slots: Background, Top bar (the pane bar) and
Text. Each has the web UI's eight presets as swatches
(`term-colors-<slot>-<n>`, radios, the current one checked, its name as
tooltip and label), a custom field (`term-colors-<slot>-custom`: `#rrggbb`,
the text also `auto`; anything else is refused with a red line, nothing
applied) with Apply, and Reset (to sbxw.toml's value, else the built-in).
Per slot the first source set wins: the pick (the state file's
`terminal_colors`), sbxw.toml's (`GET /api/state` `terminal_colors`), the
app theme's own. sbxw.toml is never written. A choice applies at once to
every terminal (its ground, text, ANSI white and bright white for a picked
text, the selection, the cursor and the keyword pills; a light background
such as Paper takes the light rules, the same `Theme::from_settings` path
as sbxw.toml's light value) and every pane bar, in the main window and in
page windows. A picked bar is the surface under the group tint: a grouped
pane's bar is the group's colour at the theme's bar tint over the picked
colour. Text or bar labels under 4.5:1 are allowed, with a quiet warning
line in the popover.

A pick (a background, or a text colour other than Auto) also reaches the
neutral colours a program draws explicitly, 24-bit or from the 256-colour
cube and greyscale ramp, which otherwise ignore the theme: Claude Code
draws its text white and its prompt and message bands grey that way
(`Remap` in `terminal/theme.rs`, applied in `render::resolve`). A grey's
lightness is read as its place between black and white and drawn at that
place between the picked ground and the picked text: a light grey (mean
channel 128 and up) is text, white the pick at full strength and a mid grey
a dimmer tone; a dark grey (96 and below) is a ground or a band, black the
ground itself and a band the ground moved the same step toward the text
(lighter on a dark ground, a light grey band on Paper), held where the text
at full strength still reads at 4.5:1. Only colours whose channels lie
within 24 of each other move: syntax highlighting, diffs, red and green stay
as the program chose. With nothing picked every colour is drawn as before.
The answer is cached per colour. Cream text on Night: Claude Code's text is
cream (`#e6d5b8`) and its band `#464345` over the `#1a1b26` ground; on Paper
with Auto text, dark text (`#24292f`) on a `#cecabc` band. The web UI does
not do this: xterm.js draws 24-bit colours as given.

The current page's tab and the pane under its left side are one shape: the
tab stands on the pane's top edge, its outline (2 pt, `JOIN_W`) in that
pane's line colour (its group's colour, else `border`) and its fill that
pane's bar tint; the pane's top line opens under the tab and carries on
from its sides. On the first tab the pane's top-left corner is square and
the two left edges are one line; further right (a later tab, the leads
view's mark or the hidden tabs' menu before it) the pane keeps its rounded
corner and the tab's sides meet its top line at a square foot. In Grid view
the tab takes the look of the pane under its left side; a pane of another
group further under it keeps its own line (test
`the_current_tab_and_its_pane_are_one_shape`).

Jarvis can propose changes to these screens (the daemon writes the file in
`~/.sbxw/desktop/screens/` once the person approves). With
`$SBXW_DESKTOP_SCREENS` set (tests), the app reads that folder instead and
says so: Jarvis's changes do not reach it.

**An override made for an older version.** The app keeps, in its state
file (`~/.sbxw/desktop/state.json`), the hash of the built-in screen that was
current when each override appeared or last changed. When a new version of
the app changes that built-in, the override is *stale*: the app says so once
("Your sandboxes screen is a customised copy made for an older version; new
features may be missing. Ask Jarvis to update it, or restore the built-in.")
and draws a small amber marker at the top of that screen for as long as it
lasts. A press on either opens the app's own dialog, which names the file;
**Delete and restore** removes it (intent `screen_restore`, risk Approval)
and the built-in comes back. An override saved again (by Jarvis, or by hand)
counts as made for the version running. An override the app has no record
of (made before this check existed) is stale when the file is older than the
app itself.

**Extras.** A screen that is the place of extras (`topbar`: `data.app.extras.topbar`;
`sandboxes`: `data.app.extras.sandboxes[name]` and `data.app.extras.rows[name]`)
must draw them, with their keys (`extra-topbar-<id>`, `extra-sandbox-<sandbox>-<id>`,
`extra-row-<sandbox>-<id>`): that is how features (network, threads, ...) reach
the person. Whatever extra a screen does not draw (an old copy that predates
it), the core draws itself in a strip under the screen (beside it for the top
bar), so an override cannot hide a feature; one it draws is never drawn twice.
An item of a `menu` counts as drawn (`<menu id>-<item id>`): the built-in top
bar puts the extras that do not fit into its menu `extra-topbar`, whose items
are then `extra-topbar-<id>`, the same keys as when they show inline.
A sandbox's extras are looked for only when its row is drawn (a row key
`row-<name>`, or its name as a text), so a filter does not bring them back.
A sandbox's session rows are accounted for by its sessions disclosure (an
element keyed `sessions-<name>`, the built-in list's "4 sessions" line),
whether the block is open or shut.

At every start the app writes what it has built in to
`~/.sbxw/desktop/builtin/` (`screens/<name>.luau`, this document as
`screens.md`, and `intents.json`), for the daemon to show Jarvis. The app
never reads that folder.

## Limits

The VM has `string`, `table`, `math`, `utf8` and `bit32`, and nothing else:
no `io`, `os`, `require`, `load`, `loadstring`, `dofile`, `getfenv`,
`setfenv`, `collectgarbage`. Libraries and `theme` are read only. Each call
into the screen (`init`, `render`, an action) gets 200,000 interrupt checks
(calls and loop turns), the hard limit, which does not depend on how busy the machine is, and 1 s of wall clock, which only catches a call that blocks; memory is capped at 16 MB; one render may
return 5,000 elements, nested 32 deep. Going over is an error in the banner
over the last good view, retried on the next change; the app keeps running.

### Long lists

A render returns 5,000 elements at most, so a long list draws its first rows
and a **Show N more** block, the filter having run over every row before the
cut. Two helpers, the same block everywhere:

- A list the feature builds in Rust: `crate::screen::paged(rows, page,
  pages, what)` returns the rows to draw and a count `{total, shown, more,
  label}`; the screen draws the block with `more_rows(count, { key = "x-more",
  on = "more", what = "commits", hint = "Filter to find one" })` (false when
  nothing is left; the box is `<key>-box`).
- A list the screen filters itself: `local rows, more = paged(items, { page
  = 200, pages = state.pages, key = "x-more", on = "more" })`, then draw
  `rows` and `more`; the action adds a page.

Used by Commits, Worktrees and Files. The screens that can still grow past
the cap with many sessions, threads or requests are listed in VIEWS.md.

### Overlay helpers

- A sandbox picker is a `select` with `scope = "sandbox"` (see `select`,
  "Scope pickers"). `sandbox_picker(data, opts)` (a colour square and a
  plain select) is kept for overrides written before it; no built-in
  screen uses it.
- A row keyed `title-slot` in an overlay's screen (its scope picker) is
  drawn by the core in the overlay's header, right after its title
  ("Files in [web-app v]"), not where it sits in the tree; in a panel or a
  modal it stays in place. Its keys and actions are the screen's as usual.
- `overlay_wide(data)`: true when the overlay over the pane area is 760 pt
  wide or more (`data.app.overlay.w`; a screen drawn alone counts as wide).
  Two columns from there (a `split` keyed `<screen>-split`), one under it.

## Checking your screen

```sh
desktop/scripts/screen.sh <screen> [fixture] [--file my.luau] [--script s.json]
```

draws the screen alone from `desktop/fixtures/<screen>/<fixture>.json` and
writes a PNG and the element tree. `.sbxw-artifacts/desktop/testing.md` (the repository root's artifacts folder, outside git) has the rest:
fixtures, scripts that press elements by key, the whole window.

## Adding a feature

A feature is one area of the web UI ported to the app (network, create,
settings, and later threads, relay, files...). Its Rust lives in
`desktop/src/feat/<area>.rs` (or `feat/<area>/`), its screens in
`desktop/screens/`, its fixtures in `desktop/fixtures/<area>/`. It is
declared in **one** place, a `pub const FEATURE: Feature`, and registered by
**one** line, `<area>,`, in the `features! { }` list at the bottom of
`src/feat/mod.rs` (the core's owner adds that line). The core does the rest:
reading its data, loading its screens, checking and running its intents,
drawing its dialog and slots.

What a feature declares:

| Field | What |
|---|---|
| `name` | Short name, the notices' prefix |
| `data` | `Data { key, path, on, every, lazy }`: a `GET` route read into `data.<key>` at connection, again 300 ms after a stream event in `on`, every `every` seconds, or when an action asks (`lazy`: only then). `{sandbox}` in the path: read for each running sandbox, `data.<key>` is then an object by sandbox name. A fixture holds the same key in its `daemon` object |
| `events` | Stream events handed to `on_event`, which answers actions |
| `screens` | `ScreenDecl { name, src: include_str!(..), slot }`; `Slot::Modal { width }` (a dialog over the window) or `Slot::Panel { title, icon, width }` (docked on the right, beside Jarvis; several open show as tabs) or `Slot::Overlay { title, icon }` (a full view over the pane area, below) or `Slot::Base { title, icon, open }` (a base view of the pane area instead of the terminals, below) |
| `intents` | Rows of the closed list (`intent::Spec`: name, typed arguments, risk, doc). Exported to `intents.json` |
| `<screen>_key` and `<screen>_ui.popup` | By name, for an overlay or base view: its `<screen>_key` intent gets Up, Down, Enter (and what its `key` list takes) while none of its controls has the keyboard; while the feature's data `<screen>_ui.popup` is true and that list takes `escape`, Escape goes to it (`{key: "escape"}`) before the view is closed (below) |
| `act` | `fn(&Ctx, &Asked) -> Result<Vec<Action>, String>`: runs one of its intents, already checked |
| `on_answer` | Optional: gets every answer of its calls, status and JSON body, non-2xx included; then the core says nothing itself |
| `extras` | Buttons and badges in others' slots: `Extra { place, id, label, icon, tone, help, count, intent }`; `place`: `Topbar`, `Sandbox(name)` (on its row), `SandboxRow(name)` (a sub-row under it: one per session), `Pane(sandbox)` (every pane bar of that sandbox), `Session { sandbox, thread }` (the pane bars of that session only; `thread: None` is the main one), `Rail { zone, order }` (an entry of the left rail, see below) |
| `standin` | Its routes for the stand-in daemon and fixture daemons: `fn(&mut Standin { data, publish }, method, path, body) -> Option<(status, body)>`; `publish_after(ms, event, data)` for events sent before the answer |
| `core_only` | Body fields only the core sets, after the person's Confirm (`Confirm::grant`); any other call carrying one is refused |
| `shortcuts` | `&[Shortcut { key, intent, args, help }]`: global keys, taken by the window before any pane, as the web UI's: Alt+<key> off macOS, Ctrl+Option+<key> on macOS (Option alone types characters). `key` is the letter by its place on the keyboard; `args` a JSON object. Create declares N (New) and M (chat) |
| `on_files` | `fn(&Ctx, Incoming) -> Vec<Action>`: what the person handed it, `Incoming::Files(paths)` or `Incoming::Png(bytes)`. Files dropped on its shown panel or dialog; a picture pasted (⌘V) while its panel is shown and no terminal has the keyboard (a paste into a terminal stays the terminal's); the answers of `PickFiles` and `ReadClipboard`. The only ways a feature gets a path to read: never from a screen or a sandbox. Decode pictures with `crate::picture::prepare` (limits, no animation, scaling, PNG or JPEG out), then apply the feature's own policy |
| `surfaces`, `on_surface` | Picture surfaces it owns, by id (`src/surface.rs`), placed by a screen with `surface{ id }`. Frames from its Rust on any thread: `surface::set_encoded(id, bytes)` (PNG, JPEG, WebP decoded with `surface::frame_limits()`) or `surface::set_rgba(id, w, h, rgba)`; Rune redraws without running Luau. `on_surface: fn(&SurfaceEvent) -> Vec<Action>` on the UI thread: `Move`, `Press`, `Release`, `Wheel` in picture pixels, `Key` (DOM key name, physical code, text), `Text` (input method), `Paste` (text; the core shows a notice with its size), `Focus`, `F6`, `Resized` (logical points, debounced). The app keeps its own keys (see the `surface` element) |
| `on_socket`, `standin_ws` | `Action::OpenSocket { id, path, opts }` opens a `/ws/...` route of the daemon with the app's auth (`src/socket.rs`); `on_socket: fn(&str, SocketEvent) -> Vec<Action>` runs on the socket's own thread (`Open`, `Text`, `Binary`, `Reconnecting`, `Closed`), its actions on the UI thread at the next poll; `socket::send(id, text)` from anywhere. `Opts { reconnect, droppable, max_in, max_out, one_line }`: reconnect is off by default; `droppable` messages (frames) keep only the newest when the handler is behind. `standin_ws: fn(target, &mut dyn WsPeer) -> bool` serves the route in the stand-in |
| `on_bytes` | `fn(&Ctx, &Call, Result<(status, bytes, content type), String>)`: the answer of a `Call::get(path).raw(max)` (at most `max + 1` bytes). Fixture and stand-in routes answer raw with `{"$raw": text, "$type": ...}` or `{"$raw_hex": ...}` |
| `on_pane_drop` | `fn(&Ctx, &PaneDrop { paths, skip_dialog, sandbox, thread, picture })`: files dropped on a terminal pane (the first feature declaring it); the paths are handed in, so an upload may read them. `skip_dialog`: the person ticked "Do not ask again for files I drop"; the core then lets ONE unconfirmed `core:uploads` through, from this drop's own actions only, of these paths only, into this pane's sandbox, when the guardrails (`feat::files::drops`) find nothing. `picture`: a PNG pasted into the pane with no text on the clipboard (⌘V on macOS, Ctrl+Shift+V or Shift+Insert elsewhere; text always wins when both are there), already checked by `crate::picture::check` (32 MB, 16384 px a side, 40 megapixels, not animated); then `paths` is empty and `skip_dialog` false (it is about dropped files). A paste is not asked: the core passes it on only for the focused pane that may take keys, never while the panes are covered (Files: uploaded at once to `paste-image`, the name `pasted-<UTC date>-<hhmmss>.png`, its path typed into the pane's session, not submitted, then the notice "Pasted <name> into <sandbox>: its path is in the prompt" or the error; a second paste while one uploads is refused with a notice). While files are dragged over a pane, that pane alone shows a 2 pt border and a line: where they would go, or why not (the panes covered, no sandbox, a sandbox not running); a docked panel or the modal does the same |
| `on_dismiss` | `fn(&Ctx, tag)`: one of its core dialogs closed without its call going out (Cancel, close, Escape, a press outside, replaced, a failed `Prepare` closed); `tag` is the dialog's call tag |

Actions (`feat::Action`): `Call(Call)` (a daemon request off the UI thread:
`Call::post(path, body, what)` / `Call::get` / `Call::delete`, `.then(&[keys])`,
`.store(key)`, `.timeout(secs)`, `.tag(t)`, `.quiet()`; a `DELETE` is a
removal: sent directly only for what undoes nothing the person made,
today Sessions' own history subscription; any other DELETE from a feature
goes through `Confirm`, like every destructive call), `Confirm`,
`Prepare`, `Refresh(key)`, `Fetch { key, path }`, `Set { key, value }`,
`OpenModal { screen, arg }`, `CloseModal`, `OpenPanel(screen)`,
`ClosePanel(screen)`, `Notice { text, error, then }` (`then`: an intent a
click on the notice runs), `OpenLink(url)`, `Intent { name, args }` (checked
exactly as from a screen), `Remember { key, value }` (kept across runs in the
app's state file, read back as `data.app.kept.<feature>.<key>`; null forgets).
`OpenSocket`, `CloseSocket(id)`, `Focus(element key)`, `ScrollIntoView(element key)`
(scrolls the nearest viewport so that element shows, once the change is
drawn; it never moves the keyboard focus, and a key not drawn does nothing:
the Dashboard's Show less on a bubble taller than its view). A `Call` of method
`Core` is one of the core's own operations: restoring a screen (from the
core's dialog only), and the file operations of `src/files.rs` built with
`feat::core_call(op, body, what, tag)`: `inspect {path}` and `reveal {path}`
(direct, files the core wrote this run), `open {path}` and `upload {route,
file, content_type, tag}` (from a `Confirm` only; `open` follows `safe.rs`'s
verdict, a `warn` file needs the dialog's ack box; `upload` reads only a file
the person handed in this run, 128 MB at most, progress in
`data.app.uploads[tag]`). `Download(feat::Download { get, name, to, after,
tag })` writes a daemon route's bytes under a sanitised new name: direct only
to the Downloads folder without opening, else as the call of a `Confirm`
(`Download::call`); `to: Dest::Ask` opens the native save dialog. `Reveal(path)`,
`ForgetFiles(paths)`. `Call::post_bytes(path, content_type, bytes, what)` posts
a raw body. `PickFiles { title, extensions, multiple }` opens the native open
dialog (Rune's `native-file-dialog` feature: one file per dialog, `multiple`
says so in a notice; where no dialog can open, headless or Linux without a
desktop portal, a notice says "Drop the file on this panel, or paste an image."),
`ReadClipboard` reads the clipboard for a picture, both answering through
`on_files` (both run only from the person's own press on a screen, never from
a feature's data, events or answers; a screen never gets the clipboard's
content, only the feature's Rust does); `Image { key, bytes }` registers a picture for `image{ key }`
(decoded by the core; empty bytes forget it). Headless scripts drive them with
Rune's `drop_files`, `paste` (`{"paste": {"png": "file.png"}}`) and
`file-dialog` events.

**Approval.** An intent of risk `Approval` may only answer one `Confirm` or
one `Prepare`; anything else is refused and nothing runs. `Confirm` is the
core's own dialog, built in Rust from the daemon's data: `Confirm::new(title,
call).line(..).preview(text, diff).ack(text).secret(label, "secrets.KEY")
.optional_secret(label, field, placeholder).grant("approved").labels(yes,
no).danger()`. Secret fields are Rune secure inputs drawn by the core:
masked, no copy, no accessibility text, absent from the tree and from
screen data; their values go into the call's body (a dotted path, never over
a field already there) on Confirm, and the inputs are wiped. `Prepare {
title, call, asked, build }` shows the dialog loading, reads `call` itself,
and `build` makes the `Confirm` from the answer (a host key's fingerprints),
so a screen cannot forge what is shown. A Direct intent may also answer a
`Confirm` (more confirmation, never less). Screens never see the dialog.

**What screens get.** The feature's keys under `data`, and in `data.app`:
`modal` (`{screen, arg}` or nil), `panels` (`{open = {...}, shown, split}`: `split` when the person shows the open panels side by side, each in its own column, with the switch at the right of the tabs),
`extras` (`topbar`, a list; `sandboxes` and `rows`, lists by sandbox name; each `{id,
label, icon, tone, help, count, intent = {name, args}}`; a top bar extra that
does not fit has `more = true`; one the core draws elsewhere has `in_sidebar`
(New, in the sidebar's header) or `in_rail` (Requests, Network, Setup, in the
left rail): the top bar skips those, and the core does not draw them for it), `topbar` (how the top bar fits: `w`, its
column's width in points; `compact`, under 720 pt or when its parts do not
fit: no title; `more`, how many extras go into its menu), and `errors`: by
data key, why its last read failed (the daemon's words), until a read works;
the key keeps its last good value meanwhile. For a plain key the reason is a
string; for a per-sandbox key (`{sandbox}` in its path) it is an object by
sandbox name (`{ "web-app" = "..." }`), each entry gone once that sandbox's
read works, each sandbox keeping its own last good value.

The built-in screens draw extras with these element keys (scripts press
them): `extra-topbar-<id>` (top bar), `extra-sandbox-<sandbox>-<id>` (a
sandbox's row; a feature extra there replaces the plain approvals flag),
`extra-row-<sandbox>-<id>` (a session sub-row: a dot of `tone`, or `icon`,
the label, a badge of `count` or else `help` in dim text, the whole row
pressable; a live session no pane shows, starting, working, wanting you or
idle, has instead of the count a green "+" `session-add-<sandbox>-<id>`
that runs the row's own `open_terminal` with `pane: "spare"`), and `pane-<n>-x-<id>` (pane bars, drawn by the core). An extra with an
`intent` is a button, without one a badge.

**What fits in a bar** (`src/shell/bar.rs`). A pane bar always shows its
state dot, the sandbox's name (and the thread's, `web-app · t2`; at least
about eight characters, its tooltip whole; key `pane-<n>-name`), the session
kind and the close button (`pane-<n>-close`) at its right end. The extras
take the room left, by priority: an amber tone first (sbx holding something,
"wants you", a plan waiting, an agent's request), then those with a count or
a green tone, then the rest, each group in its features' order. What does not
fit goes into one menu, `pane-<n>-more` ("…" and how many; a dot in amber
when it hides something amber, in blue when it hides a count), whose list is
`pane-<n>-more-list` and whose items keep the chip's key, label, icon, tone,
count and intent. So a script taps `pane-<n>-x-<id>` directly while it shows,
and `pane-<n>-more` first when it does not; `wait:pane-<n>-x-<id>` returns
for either (an item counts while its menu is open). The core's "wants you"
(`pane-<n>-attention`) and connection word (`pane-<n>-status`) go through
the same fit. Escape closes the menu. The fit follows each bar's laid-out
width, at every resize. The top bar's extras get the same order; the core
marks those that do not fit `more` (above).

**What the "⋯" menu leaves out** (`bar::same_view`). The pane's menu lists
only what the left rail and the sidebar do not already offer for that
sandbox, read from the intents' declarations, not from a list of ids. An
item is the rail's when a rail entry (drawn, or folded into the rail's
"More") runs the same intent with a part of its arguments, or shows a screen
of the feature that owns the item's intent, and the item's intent takes only
optional arguments (a view opened on any sandbox and moved to this one
there). It is the sidebar's, while the sidebar shows, when that sandbox's
row has an extra with the same intent and arguments (the row's menu lists
every one). Such an item is never a menu row; it stays inline while it fits
only when it says something there (a count, an amber or green tone), else
it leaves the pane. An action on this sandbox (Esc, Remote, Close thread,
Team, Browser, Project, Env) needs its sandbox and stays the pane's. Today:
sbx's requests (`approvals`, the rail's Network) and an agent's request
(`relay`, Requests) show inline while they fit; Commits, MCP servers…,
Skills… and Kits… (the rail's entries) are gone from the pane; Threads is
gone while the sidebar shows (its row's "threads"), back in the bar or menu
when it is hidden. `shell::tests::every_pane_action_is_reachable` walks every
pane extra and checks each has a visible entry point.

**Env and the microphone.** Env (`pane-<n>-x-envfile`, the environment file
card) is always a row of the "⋯" menu (`bar::MENU_ONLY`). In its place in the
fit order the core draws the microphone, `pane-<n>-dictate` (icon alone,
named "Dictate", its tooltip the action and the dictate key): a press gives
the pane the keyboard and starts the dictation into its terminal, the same
path as the dictate key held there (`dictate.rs`, masked by the daemon,
typed as a paste with no Enter, only while that pane keeps the keyboard and
the gate is open); a second press stops it, Escape cancels. It never takes
the keyboard from the terminal (`preserve_focus_on_press`). Recording, it is
red with `aria-pressed`; while the words are transcribed and masked, the
busy spinner; when it cannot dictate (no audio, voice not installed or not
reachable, Jarvis listening, no terminal in the pane, something over the
panes, a dictation elsewhere) it is disabled with the reason as its tooltip.
Folded into the menu, its row does the same.

### A rail entry

A feature puts an entry in the left rail with `Place::Rail { zone, order }`.
`zone` is a `feat::Zone`: `Activity` (what happens now: Requests 10,
Network 20, Team leads 30), `Browse` (views to read: Commits 10,
Worktrees 20, Files 30) or `Configure` (what is set up: MCP 10, Skills 20,
Kits 30; Settings is always its last). `order` sorts within the zone, lower
first. The rail draws the activity and browse zones at its top, the
configure zone at its bottom, Jarvis apart under it, and a hairline between
two zones (`rail-zone-<zone>`).

```rust
Extra {
    place: Place::Rail { zone: Zone::Configure, order: 10 },
    id: "mcp".into(),                 // key `rail-mcp`
    label: "MCP".into(),              // the tooltip's first line
    icon: Some("plug".into()),        // its one mark (icons.rs)
    tone: "blue".into(),              // colour by meaning
    help: "MCP servers the sandboxes reach".into(), // the tooltip's second line
    count: None,                      // a badge (amber with tone amber or red)
    intent: Some(("open_panel".into(), json!({ "screen": "mcp" }))),
}
```

A status dot on an entry (live state, such as the virtual Mac's): a second
extra, `Place::RailStatus("<the entry's id>")`, whose `tone` is the dot
(`green`, `blue`, `amber`, `red`; `muted` a hollow grey ring; none, no dot),
drawn at the icon's lower right with a dark ring, key `rail-<id>-dot`. Say
the state in the entry's `help` too: colour is never the only signal.

An entry whose intent is `open_panel {screen}` or `open_modal {screen}` is
marked on while that screen shows, and a second press closes a panel or an
overlay it opened. When the rail is too short for every entry (it holds 18
at a 720 pt window), the configure zone's entries fold first into one
**More** entry (`rail-more`, its menu `rail-more-list` opens right of it,
the folded entries keep their keys), then the browse zone's; the activity
zone, Settings and Jarvis never fold.

### Worked example

```rust
// src/feat/hello.rs
use super::*;
use crate::intent::{Arg, Risk, Spec};

pub const FEATURE: Feature = Feature {
    name: "hello",
    data: &[Data { key: "hello_rules", path: "/api/hello", on: &["hello"], every: 0, lazy: false }],
    screens: &[ScreenDecl { name: "hello", src: include_str!("../../screens/hello.luau"), slot: Slot::Panel { title: "Hello", icon: "info", width: 320.0 } }],
    intents: &[
        Spec { name: "hello_allow", risk: Risk::Approval("Allow a host"), args: &[("host", Arg::Str, true)], doc: "Asks the core's dialog to allow a host." },
        Spec { name: "hello_remove", risk: Risk::Direct, args: &[("host", Arg::Str, true)], doc: "Removes a rule." },
    ],
    act,
    ..Feature::EMPTY
};

fn act(_cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
    let host = a.req("host");
    Ok(match a.name {
        "hello_allow" => vec![Confirm::new(format!("Allow {host}?"), Call::post("/api/hello/allow", serde_json::json!({ "host": host }), format!("allow {host}")).then(&["hello_rules"]))
            .line(format!("Every sandbox may reach {host}, until you remove the rule."))
            .labels("Allow", "Cancel")
            .into_action()],
        _ => vec![Action::Call(Call::post("/api/hello/rm", serde_json::json!({ "host": host }), format!("remove {host}")).then(&["hello_rules"]))],
    })
}
```

```lua
-- screens/hello.luau: a button in a list row
button{ "Allow", action = "allow", arg = row.host }
-- actions
allow = function(_, host) return intent("hello_allow", { host = host }) end,
```

Then `hello,` in `features! { }`, a fixture `fixtures/hello/basic.json`
with `daemon.hello_rules`, and `scripts/screen.sh hello basic`.

## Features

Each feature's own section: its data keys, screens, intents and extras.
Each thread owns the text under its heading.

### network

**The counts.** `GET /api/approvals` is `sbx policy approval ls --format
json`: the whole machine's requests, unfiltered (sbx lists a refused host
again at each retry; a `network` request is a host sbx already refused,
nothing waits on it; a `port` request holds a kit and lapses after 45 s).
Only listed sandboxes' requests count. A sandbox's row (`⚑ N`) and its pane
bars (`pane-<n>-x-approvals`, "sbx asks N") count every request of it, as
the web UI's row does (it has no total). The top bar's `extra-topbar-approvals`
counts one per sandbox and host; its tooltip says the requests, the distinct
hosts and the sandboxes. Amber when a request holds something, else quiet.

**Panel.** Requests grouped by sandbox and host (`×N`), the newest's options
(`answer-<id>-<option>`; answering goes through the core's dialog); a group's
Dismiss takes all its copies; `approvals-dismiss-all` dismisses every refused
host listed. At most 25 cards are drawn; `net-req-more` ("Show 25 more")
draws the next ones, and the summary line says "Showing 25 of N".

**The filter.** A sandbox's own entries (its row's bubble and menu item
"Network requests (N)", its pane bars' "sbx asks N", its Network item, the
one-request notice) open the panel on that sandbox's requests only; the top
bar and the many-requests notice open it on every sandbox's. Under a filter
the header reads "Waiting for you N in" with the chip `net-filter-chip`
(the sandbox's name and an x) and `net-filter-all` ("Show all M"); the
summary line, the badge and the bulk button count only what is shown, and
the bulk button reads "Dismiss these N" (or "Dismiss all N of <sandbox>"
when some cards are not drawn). With nothing left: "Nothing waits in
<sandbox>." and the way back. The sandbox picker keeps the filter on (it
moves to the picked sandbox) or off. The filter is view state in data key
`network_view` (`{filter}`, or null): changing it reads and sends nothing,
and closing the panel drops it.

**`<screen>_view`.** A panel's view state that must not outlive it lives
in the data key `<screen>_view`; the core drops that key when the panel
closes (`close_panel`, the tab's x), so the next opening starts clean.

**Intents** (beside the policy and port ones): `network_show {sandbox?,
filter?}`: opens the panel on that sandbox's policy and ports, filtered to
its requests unless `filter` is false; without a sandbox, unfiltered.
`network_filter {sandbox?}`: sets or clears the filter only.
`approvals_dismiss {sandbox?, title?, expect?}`: direct; sbx's own `dismiss`
option on each `network` request of the listed sandboxes (narrowed to one
sandbox and host when given), one `POST /api/approvals/respond` after the
other, a notice at the end. `expect` is the number on the button the person
pressed: if the list now holds another number, nothing is sent and an error
says the list changed. A request that holds something is never dismissed
this way.

**sbx 0.54: an automatic host port.** Publish with the host port empty
sends no `host_port`: sbx picks a free one, and the answer says which
(`data.network_publish`: `{sandbox, sandbox_port, host_ip, host_port,
assigned, note, forwarded_port}`, or `{error}`; `{busy}` while it runs,
the Publish button `port-publish` busy). The row of that port carries
`port-picked-<spec>` ("sbx picked"), and `port-published` under the form
says "Published 5173 on 127.0.0.1:49153, the port sbx picked." An older sbx
publishes on the sandbox port and its `note` shows in amber ("automatic host
port needs sbx 0.54: ..."). A fixed host port is unchanged (the core's
dialog).

**sbx 0.54: Why?** Each allowed host of Recent egress has `why-<host>`
("Why?"): `network_why {sandbox, target}` reads `GET
/api/sandboxes/:name/policy/check?target=` and the answer shows under the
host (`why-answer`, closed by `why-close`): allowed or not, `why-rule`
("Rule: r-12ab", or "No rule named") and sbx's own words `why-raw`, all set
as text. When sbx names a rule id the panel's rules list has, `why-rm` is
that list's own remove (`network_rule_rm` for an allow rule, else
`network_unblock` and its dialog). An older sbx, or a daemon without the
route: "needs sbx 0.54", muted. `data.network_why`: `{sandbox, target,
busy, supported, allowed, rule, rule_id, raw, note, removable, error}`.
`network_why_close` hides it. Both are direct and read only.

### create

New sandbox (the web UI's `create.js`, and the bring-up cards of
`sandboxes.js`). The form is the core's: the screens show it and change it
one intent at a time, so what is sent and what the core's dialogs list is
never text from a screen.

**Screens.** `create` (modal, 640 pt): the chooser, the workspace form and
the chat form, by `data.create.dialog`. Bring-ups have no screen and no
panel: the core's status bar shows them (key `status-jobs`), and its press
lists each one in a read-only popover (`status-jobs-pop`; per sandbox
`job-<name>`, its steps `job-<name>-step-<i>`, `job-<name>-error`, and the
two actions `job-retry-<name>`, `job-dismiss-<name>`). Escape, a press
outside, or nothing left to list close it. A failure stays in the bar, red,
until retried or dismissed, and says so in an error notice. The palette's
"Show the sandboxes being created" and `open_panel {screen: "create-jobs"}`
(kept as an alias) open the same popover; with nothing listed, a notice
says "Nothing is being created."

**Corner cards** (key `corner-jobs`, the web UI's corner stack): while
anything is listed, one card per sandbox (`corner-job-<name>`), newest on
top, pinned to the pane area's bottom right (12 pt in): above the status
bar, left of the Jarvis panel and any docked panel, so the Jarvis input is
never covered; only the cards take a press. Each is the popover's card,
compact: the name, the step that runs (`corner-job-<name>-step-<i>`, with
what sbx prints under it) beside a busy ring (`corner-job-<name>-busy`); a
failure stays with sbx's words (`corner-job-<name>-error`), the denied
hosts with the same allow (`corner-job-allow-<name>-<host>`: network's
`network_allow`, the core's dialog), `corner-job-retry-<name>` and
`corner-job-dismiss-<name>`. Three at most, then `corner-jobs-more` ("and N
more: open the status bar", the popover). A ready one leaves the list and
its card fades out (the status bar says "created"). The cards give way
while the popover is open; menus, popovers, modals and the core's dialogs
draw above them. Main window only: a page window shows its page's panes,
no status bar and no cards.

**Extras.** `create` ("New", opens the chooser): drawn in the sidebar's
header (`sidebar-new`), on the rail under Files while the sidebar is hidden
(`rail-new`), and in the palette (⌃⌥N / Alt+N).

**Data.** `data.create` (set by the core, not read from a route):

| Key | What |
|---|---|
| `dialog` | `chooser`, `workspace`, `chat`, or nil |
| `hosts`, `host`, `remote` | Host items (`id`, `label`; empty with no remote host), the chosen one, whether it is remote |
| `name`, `name_note`, `name_problem` | The name as cleaned, the rule when a character was dropped, why it cannot be used (taken, rule) |
| `chat_name`, `chat_suggestion`, `chat_note`, `chat_problem` | The same for a chat; empty uses the suggestion |
| `path`, `path_tag` | The folder shown; `Selected` (it is the workspace) or `Browsing` (extra folders are added) |
| `listing` | `path`, `parent`, `can_up`, `entries` (`name`, `path`, `starred`, `added`); `listing_error` when it failed |
| `favourites` | `path`, `name`, `missing` |
| `repos`, `repo_hint`, `repo_hint_error`, `can_add_repo` | The sandbox's folders (`path`, `readonly`; the first is the workspace) |
| `carve_parents`, `carves`, `carve_pick`, `preview` | Restricted sub-folders: where one can go, each (`rel`, `access`, `enforce`, `note`, `hidden_count`, `error`), the pick in progress (`root`, `cur`, `can_use`, `typed`, `error`), the daemon's preview (`summary`, `bad`, `warnings`, `files`, `more`, `stub`) |
| `ports`, `kit_mounts` | Port rows (`sandbox`, `host`, `alias`, as typed); kit folders (`kit`, `target`, `host`) |
| `template`, `rebuild`, `remote_control`, `chrome`, `git_url` | The template offer (`note`, `help`) when `template = "auto"`; the boxes; the git URL (remote host only) |
| `can_create`, `create_problem`, `error` | Create is pressable; why not; the last refusal of the picker or the favourites |
| `jobs` | `sandbox`, `state` (`running`, `asking`, `ready`, `failed`), `headline`, `steps` (`label`, `state`: `done`, `running`, `failed`, `waiting`; `detail`), `error`, `warnings`, `can_retry` |

**Intents** (all Direct; indexes count from 1):

| Intent | Arguments | What the core does |
|---|---|---|
| `create_open` | `dialog` (`chooser` default, `workspace`, `chat`) | Opens the dialog; the workspace form starts empty and reads the folders, favourites, remote hosts, kit folders and template state |
| `create_host` | `host` (`local` or a remote host) | The folder list, kit folders and git field follow |
| `create_browse` | `path` (none: home) | Lists that folder; while picking a restricted sub-folder, a folder outside it is refused |
| `create_pick` | `kit` (number) | The system's folder picker (`POST /api/fs/pick`); browses to the folder, or fills kit folder `kit` |
| `create_star` | `path` | Stars or unstars it (`POST /api/fs/favourites`; the daemon's refusal shows in `error`) |
| `create_repo` | `action` (`toggle` with `path`, `add`, or `main`, `rw`, `ro`, `remove` with `index`) | The sandbox's folders |
| `create_carve` | `action` (`pick` with `path`; `use`, `typed`, `cancel`; `none`, `ro`, `strict`, `soft`, `remove` with `index`) | Restricted sub-folders; the preview is asked again (`POST /api/carve/preview`) |
| `create_port` | `action` (`add`, `remove` with `index`) | Port rows |
| `create_set` | `field` (`name`, `chat_name`, `git_url`, `port_sandbox`, `port_host`, `port_alias`, `kit_mount`, `carve_typed`), `value` (none: empty), `index` | A text field; give it on every edit |
| `create_toggle` | `field` (`remote_control`, `chrome`, `rebuild`), `on` | A box |
| `create_submit` | none | Creates (`POST /api/sandboxes/create`). When the sandbox reaches more of this machine than its workspace (extra folders, kit folders, ports, Chrome) the core lists what in its own dialog first |
| `create_chat` | none | Creates the chat sandbox (`POST /api/sandboxes/chat`) |
| `create_retry` | `sandbox` | Sends a failed creation again, as first sent |
| `create_dismiss` | `sandbox` | Forgets a finished or failed one |

When the daemon refuses a bring-up for want of host access its `sbxw.toml`
asks for (409 with `consent`), the core shows the daemon's lines in its own
dialog and sends them back as `approved` only on Allow (`core_only`). Once
ready, the core selects the sandbox and opens its agent in a spare pane.

### setup

The web UI's Setup dialog (`assets/js/setup.js` and its tabs): secrets and
the GitHub token (tab "Secrets and sign-in", with the dropped files
setting), remote hosts ("Remote hosts"), the voice models ("Voice"). A
modal, opened by Settings at the bottom of the rail
(`open_modal { screen = "setup", arg = <tab> }`, the tab shown last; a tab
kept from before MCP, Kits and Skills left opens on the first tab). MCP
servers, Kits and Skills were tabs here: each is now a docked panel with
its own rail entry ([mcp](#mcp), [skills](#skills), [kits](#kits)), with the
same intents, arguments and dialogs.

Files: `src/feat/setup/` (rules, stand-in, tests), `screens/setup.luau`,
`fixtures/setup/`. The stand-in still answers every route of the former
tabs too (`setup/standin.rs`, one fixture object), and `fixtures/setup/`
serves the three panels' tests. Data, each key one route's answer as the
daemon gives it (why a read failed: `data.app.errors[key]`, shown as a red
line in its tab): `data.secrets` (`GET /api/secrets`: names, scopes and
kinds, never a value), `data.github_token` (`{set, gh}`),
`data.remote_hosts`, `data.voice` (`GET /api/voice`, then each `voice`
event). Set by its intents: `data.provision` (`{host, command}`,
`host_provision`).

**No intent carries a secret value.** The value of a token, a custom secret
or an MCP header placeholder is typed in the core's dialog, sent once and
forgotten; a screen only sees names and whether a value is set.

| Intent | Arguments | Risk | What the core does |
|---|---|---|---|
| `setup_load` | `section` (`secrets`, `hosts`, `voice`) | direct | Reads that tab's data again |
| `github_token_set` | none | approval | Asks for the token in its dialog, stores it as sbx's `github` secret |
| `github_token_gh` | none | approval | Makes sbx use this machine's `gh auth token` |
| `github_token_rm` | none | approval | Removes the `github` secret |
| `secret_custom_set` | `hosts` (comma or space separated), `env` | approval | Asks for the value in its dialog; the proxy puts it in requests to those hosts |
| `secret_rm` | `name` | approval | Removes a global secret |
| `host_save` | `name`, `ssh` or `host`/`user`/`port`/`key`, `agent_port`, `sbx_path`, `trust_secrets`, `editing` | approval | For a new address, reads the host key first and shows its fingerprints; adds or edits the host |
| `host_rm` | `name` | approval | Unregisters a host (with records there: an acknowledgement, then forced) |
| `host_test` | `name` | direct | `sbx version` over the link |
| `host_provision`, `host_provision_close` | `name` / none | direct | Shows or hides the command that installs sbx there |
| `hosts_reload` | none | direct | Reads hosts.toml again |
| `voice_install` | none | approval | Downloads the voice models; progress in `data.voice.job` |
| `voice_cancel` | none | direct | Stops the download |
| `voice_rm` | none | approval | Deletes the models |
| `voice_warm` | none | direct | Loads the models now |

**SSH access to sandboxes** (`src/feat/ssh.rs`, its own small feature so
that its busy state and failure stay out of the other dialogs): a card at
the top of "Secrets and sign-in", from `data.ssh` (`GET /api/ssh`:
`{configured, refresh_due, sbx, refreshed_at}`). While sbx 0.54 asks for
it (`refresh_due`), the card says "sbx 0.54 asks to rewrite the SSH
configuration" with `ssh-setup` ("Rewrite SSH configuration"), and
Settings in the rail is amber. The press is `ssh_setup` (direct, `POST
/api/ssh/setup`, `sbx setup ssh`): busy while it runs (`data.ssh_run.busy`),
then "Rewritten with sbx 0.54.0." and the reminder is gone; a failure shows
sbx's words in red (`ssh-error`, "Not rewritten: ...") and the button
stays. An older sbx: "The one-click rewrite needs sbx 0.54.", no button; a
daemon without the route: no card.

| Intent | Arguments | Risk | What the core does |
|---|---|---|---|
| `ssh_setup` | none | direct | Rewrites the `*.sbx` block of `~/.ssh/config` with the running sbx |

### mcp, skills, kits

Three overlays over the pane area (`Slot::Overlay`, as Commits and
Worktrees: the core's header with the icon, the title, "Esc closes" and the
cross; one overlay at a time; the terminals under it untouched), each its own feature
(`src/feat/mcp.rs`, `skills.rs`, `kits.rs`, tests beside them) and its own
rail entry in the configure zone above Settings: MCP (order 10), Skills
(20), Kits (30). Their icons, one mark each wherever the view appears (rail,
panel header, palette, Manage dialog, pane menu): `plug` MCP servers (a
connector, not the network's `shield`), `book` skills (not the agent's
`sparkle`, the team's `people` or Jarvis's ring), `box` kits. Each declares its entry in one
function, `rail_entry`, an extra whose intent is `open_panel {screen}`
(the rail marks it on while the panel shows; a second press closes it).
The palette lists the same extras. They were docked panels until the
person asked for them over the panes, as Commits and Worktrees: a docked
panel was one narrow column with most of the room empty. Over the pane area
they use the width (MCP: one column at every width, the add form on top,
the registered servers under it, a server's details under its row; Skills:
one column, the picked sandbox's skills with Look and Keep on top, your kept
skills under it; Kits: from 760 pt two columns, the picker, what waits for you and the kits line | the
selected kit's description, `kits-about`, one column under it).
The `<screen>_view` convention (view state dropped on close) holds for
overlays too: closing one drops its `<screen>_view`.

Each opens for a sandbox from that sandbox's Manage dialog (`sa-mcp`,
`sa-skills`, `sa-kits`), with `<name>_show { sandbox }` (the pane extras
`mcp-view`, `skills-view`, `kits-view` are declared but left out of the pane:
the rail offers the same panel, see "What the ⋯ menu leaves out"); from the rail, on the last sandbox (or file) used, kept in the app's state
file (`data.app.kept.<feature>`). Each panel: a sentence saying what the
thing is, state by colour (green kept or listed, amber a local command or
something waiting for you, red a failed read), one filled button (Add…,
Look, Save…), at most 25 rows of a list then **Show 25 more** (a local cap;
the shared paging helper replaces it when it lands), and an empty state
saying what to do.

#### mcp

`screens/mcp.luau`. Data: `data.mcp` (`GET /api/mcp`), `data.mcp_view`
(`{sandbox}`), `data.mcp_inspect_for` and `data.mcp_inspect`. Keys:
`mcp-sandbox` (the sandbox Load and Unload act on), per server
`mcp-<name>`, `mcp-load-<name>`, `mcp-unload-<name>`, `mcp-auth-<name>`
(remote only), `mcp-inspect-<name>`, `mcp-rm-<name>`; `mcp-more`, the form
`mcp-name`, `mcp-kind`, `mcp-url`, `mcp-headers`, `mcp-command`,
`mcp-args`, `mcp-dir`, `mcp-save`, `mcp-clear`; `mcp-error`; `mcp-about`
(the quiet line under the title, its full text in the tooltip), `mcp-form`,
`mcp-sel-<name>` (the row's name: opens its details, `mcp-inspect`, under
the row). One column: the form on top (Name and Kind on one row when each
keeps 240 pt, the fields that depend on Kind under them two by two, the
hint and Add… on the last row), the servers under it, each a full-width
row with Load, Unload, Sign in (remote), Inspect and the bin. The daemon
lists no load state per sandbox (`sbx mcp ls` gives name and type), so a
row shows its kind (remote blue, local command amber), not whether it is
loaded, and the list keeps this machine's order; the list's heading says so.

**sbx 0.54: MCP mode.** `mcp-modes` ("Connected now") lists each running
sandbox (`mcp-mode-<sandbox>`): its mode badge (`mcp-mode-word-<sandbox>`:
static, dynamic, or unknown) and the servers connected now, or "no server
connected". Data: `data.mcp_mode`, by sandbox (`GET
/api/sandboxes/:name/mcp`, one `sbx inspect` each), read when the panel
opens, when it moves to a sandbox not read yet, and after a load or an
unload; never on a timer. A server connected in the sandbox picked carries
`mcp-on-<name>` ("connected in <sandbox>"). An older sbx, a remote sandbox
or a failed read: "unknown" in a neutral badge and the daemon's note
("needs sbx 0.54"), never red.

| Intent | Arguments | Risk | What the core does |
|---|---|---|---|
| `mcp_show` | `sandbox` (optional) | direct | Opens the panel and reads the servers again, on that sandbox (else the last one used) |
| `mcp_pick` | `sandbox` | direct | Puts the panel on that sandbox (view state) |
| `mcp_load` | `server`, `sandbox` | direct | Loads a server into a running sandbox |
| `mcp_unload` | `server`, `sandbox` | approval | Unloads it |
| `mcp_auth` | `server` | direct | `sbx mcp auth`, in this machine's browser |
| `mcp_inspect`, `mcp_inspect_close` | `server` / none | direct | Shows or hides `sbx mcp inspect` |
| `mcp_rm` | `server` | approval | Unregisters a server |
| `mcp_save` | `name`, `kind` (`remote`, `stdio`), `url`, `headers` (`;` or new lines), `command`, `args`, `dir`, `replace` | approval | Registers (or replaces) a server; one secret field per `${KEY}`; a `stdio` one shows its host command and needs "I have read this command" |

#### skills

`screens/skills.luau`. Data: `data.my_skills` (`GET /api/my-skills`),
`data.skills_view` (`{sandbox}`), `data.skills_for` and
`data.sandbox_skills` and `data.sandbox_commands` (`skills_look`). Keys: `kept-<name>`,
`forget-<name>`, `skills-sandbox`, `skills-look`, `found-<name>`,
`keep-<name>`, `skills-more-kept`, `skills-more-found`, `skills-installed`,
`skills-installed-count`, `skills-installed-reason`, `installed-group-<id>`,
`installed-<name>`, `skills-more-inst-<id>`. Looking stays a press: it
starts a stopped sandbox.

After Look, **Installed in <sandbox>** lists every skill and command of
`GET /api/sandboxes/:name/commands`, read only (no Keep: kits and built-ins
come back with every sandbox), with a count, grouped by origin in plain
words: `Kit <name>` (an origin `kit:<name>`), `Kits` (a home command),
`Your kept skills` (a home command named as one you keep), `sbx shared
skills` (a home skill), `Written in this sandbox` (a home skill the keepable
listing also found), `Project` (`workspace`), `Built-in`. A partial answer
(`complete: false`, a stopped sandbox) shows its `reason` as a quiet line;
nothing is started for it. A failed read or a daemon without the route
(404) leaves the panel as before. With the installed list there, an empty
keepable list says "Nothing to keep: …" instead of "No skill in …". Names
are cut to 64 characters, descriptions to 160, the reason to 200.

| Intent | Arguments | Risk | What the core does |
|---|---|---|---|
| `skills_show` | `sandbox` (optional) | direct | Opens the panel and reads the kept skills again; its picker on that sandbox (else the last one used) |
| `skills_pick` | `sandbox` | direct | Puts the picker on that sandbox (view state) |
| `skills_look` | `sandbox` | direct | Lists that sandbox's skills (`data.sandbox_skills`) and what it has installed (`data.sandbox_commands`) |
| `skill_keep` | `sandbox`, `name` | approval | Keeps a skill found by `skills_look` for every new sandbox |
| `skill_forget` | `name` | approval | Stops keeping it |

#### kits

`screens/kits.luau`. Data: `data.kits` (`GET /api/kits`), `data.kits_view`
(`{sandbox, path}`), each sandbox's `kits_waiting`. Its rail entry turns
amber with the number of kits waiting (summed over sandboxes); a sandbox
with a kit waiting keeps its amber ✓ on its row (`kit-add`,
`open_terminal` with `mode = "kit-add"`): the answer is sbx's prompt, in a
pane, never this panel. Keys: `kits-waiting`, `kit-answer-<sandbox>`,
`kits-file`, `kit-<name>` (a bundled kit's box), `kit-rm-<ref>`,
`kits-add`, `kit-push`, `kits-defaults`, `kits-discard`, `kits-save`,
`kits-more`, `kits-error`.

**sbx 0.54: a kit install that sbx denied hosts for.** The `denied`
progress event (`{sandbox, step, kit, hosts, source, exists, allow_scope,
message}`) is kept by create (`data.create.denied`, one per sandbox, until
Retry or Dismiss; a notice opens this panel). Each shows here as
`kits-denied-<sandbox>`: what failed, sbx's words (`kits-denied-msg-...`,
as text), each host with `denied-allow-<sandbox>-<host>` (network's own
`network_allow`, the core's dialog) or "allowed" once a rule was added,
`kits-denied-retry-<sandbox>` (`create_retry`: the same bring-up again; the
daemon adopts the sandbox and adds the kits not applied yet) and
`kits-denied-dismiss-<sandbox>` (`create_dismiss`). The button reads "Allow
for this sandbox" only when the rule is the sandbox's (`allow_scope`
`sandbox`); for a sandbox that does not exist it reads "Allow for all
sandboxes", and its tooltip and dialog say the rule is every sandbox's. The
status bar's bring-ups popover lists the first 3 hosts of a failed one with
the same buttons (`job-allow-<sandbox>-<host>`), then `job-denied-more-<sandbox>`
("and N more: open Kits").

| Intent | Arguments | Risk | What the core does |
|---|---|---|---|
| `kits_show` | `sandbox` (optional) | direct | Opens the panel and reads the kits lines again, on the file that sandbox uses (else the last one used) |
| `kits_pick` | `path` or `sandbox` | direct | Puts the view on one of the files, or on the file a sandbox uses (the picker at its top; view state) |
| `kits_save` | `path`, `kits` (one reference per line) or `defaults = true` | approval | Rewrites that file's `kits` line (or removes it); the dialog lists what is added and removed and asks for an acknowledgement for kits that reach outside the sandbox |

### threads

Threads, plans and the lead (the web UI's thread parts of `panes.js`,
`plans.js`, `board.js`, `worktrees.js`). Files: `src/feat/threads/`,
`screens/threads.luau`, `plans.luau`, `threads-new.luau`,
`fixtures/threads/`. Report: `.sbxw-artifacts/desktop/THREADS.md` in the main checkout.

**Screens.** `threads` (panel "Threads"): one sandbox's sessions, Team
mode, its plans, and the Worktrees and Board tabs (the Board tab is the shared plan's claims board, not the Dashboard). `plans` (modal, `arg` =
a plan id): the review while it waits, its threads' state once approved,
the merge once proposed. `threads-new` (modal, `arg` = the sandbox): New
thread, or a session on the main checkout.

`threads-new` normalizes the name instead of refusing it (ticket T-37). The
rule is `threads::model::clean_name` in Rust (the same as the web UI's
`normalizeName`, on the shared table `tests/js/name-cases.json`); the screen
calls nothing heavy: each change of the field returns the `thread_name`
intent, which keeps `{raw, typing, full}` in `data.threads_ui.draft`. Typing
shows `typing` (lower case, a trailing hyphen stands); a paste (the text grew by
more than one character) shows `full`; until the answer is back the screen
shows a light guess. A line *Will be created as `<full>`* appears when `full`
is not what the field shows, and the worktree and branch lines use it. Open
sends `thread_open` with `normalize = true`; without that flag (chips, scripts,
the plan dialog's draft check) the name stays strictly checked. The one error
left is the name taken by the other kind of session; a name that normalizes to
nothing only disables Open.

**Data** (each a route's answer; a per-sandbox read that failed is in
`data.app.errors[key][sandbox]`):

| Key | What |
|---|---|
| `sessions` | `GET /api/sessions`: every hook report (`sandbox`, `thread`, `state`: `working`, `idle`, `attention`, `exited`, ...) |
| `threads` | By sandbox, `GET /api/sandboxes/:name/threads`: `{ok, threads: [{name, open, repo, model, worktree: same\|own\|team}], open, max}` |
| `plans` | `GET /api/plans`: every plan (`id`, `sandbox`, `summary`, `threads` with `state` and `summary`, `state`, `variant`, `justification`, `proposed_tests` (`{command, timeout_minutes}`, drawn as Tests in the review), `approved_tests`, `closed`, `merge`: `{order, note, state, base, results, distillate, ci}`, and `merge_status` (below)) |
| `leads`, `models` | `GET /api/leads` (the sandboxes that really lead), `GET /api/models` (`{ok, default, models: [{id, label}]}`) |
| `lead_states` | `GET /api/lead-states`: `{ "<sandbox>": {state: waiting, activating or leading, since_ms, reason} }`, absent: off. Read again on `lead`, `lead-end`, `session` and at every connection. Before it is read (or from a daemon without the route), `leads` stands in |
| `board` | By sandbox, `GET /api/sandboxes/:name/board`: `{plan, claims, entries}` |
| `threads_view` | Set by the core: by sandbox, `{sessions: [{thread, kind, running, state, word, tone, label, summary, repo, model}], open, max, leading, lead, pending, merge, merge_line, merges}`. `merge`: the plan whose merge the bar offers (proposed, merging, failed, or Merged to CI); `merge_line`: the pane banner's line (below); `merges`: by plan id, the dialog's merge view (below). `leading`: the lead state is `leading`; `lead`: `leading`, `waiting`, `activating`, `unled` (off with a plan pending or a thread at work) or `off` |
| `threads_ui` | Set by the core: `sandbox`, `tab`, `new` (`{sandbox, kind, suggest}`), `drafts` (by plan id: `{threads, variant}`), `plan_error`, `merge_close`, `merge_note` (by plan id), `repos_for`, `worktrees_for`, `remote` |
| `thread_repos`, `worktrees` | Read on demand: `GET .../repos` (for `threads_ui.repos_for`), `GET .../worktrees` (for `worktrees_for`) |

**Merge status and CI merges.** A split plan with an approved test
command is merged through a CI branch (`sbxw/ci-<plan>`): `merge.state` is
then `ci_building`, `ci_testing`, `merged_ci` ("Merged to CI"), `ci_stale`
(main or a thread moved: the rebuild waits its turn), `ci_failed`, and
`merging`, `merged` ("Merged to main") after the click. The daemon adds
`merge_status` to every plan with a merge, `{word, detail, can_merge,
why_not, ci, no_ci_because}` (`crates/core/src/plans/ci.rs`,
`merge_status`), and `merge.ci` holds the run: `branch`, `base_branch`,
`base`, `tip`, `builds`, `rebuilt_because`, `stale_because`, `merged`
(`[{thread, tip, commit}]`), `skipped` (`[{thread, why}]`), `job`, `tests`
(`{outcome, reused, commit, attempts: [{outcome, duration_ms, tail}]}`),
`failure` (`{kind: conflict|tests|error, thread, files, text}`),
`click_refused`. Nothing on screen works the status out from the state:
the words are the daemon's.

`threads::model::merge_view` makes `threads_view[sb].merges[id]` of it:
`{state, word, detail, can_merge, why_not, ci, no_ci_because, tone, button}`,
and for a CI merge `facts` (`[{k, label, text, mono}]`: CI branch, built on
(base, short), tip (short), builds and why it was rebuilt, why it waits),
`threads` (merged at a commit, or skipped and why), `tests` and
`attempts` (`[{line, tail, tone}]`, a tail only for an attempt that did not
pass, or every attempt in `ci_failed`; its last 30 lines), `failure`
(`{kind, line, thread, files, text}`), `click_refused`, and in `merged_ci`
`ff` (Merge fast-forwards the base to the short tip of the CI branch). A
daemon that sends no `merge_status` gets no entry, and the dialog draws as
before. `model::merge_line` makes the banner's `{plan, state, word, detail,
can_merge, tone, button}` for the latest plan with a merge still to do (CI
states included); from an older daemon, the banner's earlier words.

The plan dialog draws `merge-status` (`merge-word`, `merge-detail`, and
`merge-why-not` while it cannot merge, `merge-no-ci` when the daemon says
why the merge has no CI branch). A CI merge then draws `ci-facts`
(`ci-branch`, `ci-base`, `ci-tip`, `ci-builds`, `ci-stale`), `ci-thread-<n>`,
`ci-failure` (`ci-conflict-thread`, `ci-file-<n>`), `ci-tests`,
`ci-attempt-<n>` and `ci-tail-<n>`, `ci-click-refused` and `merge-ff`, in
place of the per-thread rows; a merge without CI keeps the per-thread rows
(`mr-<thread>`) and outcome. Merge (`merge-all`, "Merge to main" for a CI
merge) is off while `can_merge` is false, its help `why_not`. Abandon merge
is offered in the states the daemon abandons (proposed, failed,
`ci_testing`, `merged_ci`, `ci_stale`, `ci_failed`). The Threads panel's
banner (`plan-merge-line`: `plan-merge-word`, `plan-merge-detail`) has
Merge (`plan-merge`) only when `can_merge`, else Details
(`plan-merge-details`), both opening the dialog. Every string of it is
drawn as text: the daemon's words, a branch name, a test's tail.

Notices: a CI proposal ("... sbxw builds and tests the CI branch first"),
and a merge reaching `merged_ci` or `ci_failed` ("<word> for <sandbox>:
<detail>", the second as an error), each opening the dialog.

Fixtures: `fixtures/threads/ci-building`, `ci-testing`, `ci-merged`,
`ci-refused` (a click refused), `ci-stale`, `ci-failed-conflict`,
`ci-failed-tests`, `ci-merged-main`, and `merge-no-ci` (a merge with no CI
branch and the daemon's reason). The stand-in runs a CI plan as the daemon
(`standin.rs`: proposal, build, tests, Merged to CI, the click; the key
`standin_ci_fail` makes a run fail by a conflict or by its tests).

**Extras.** Rows under a sandbox (`extra-row-<sandbox>-<main|thread>`)
once it has a second session; `extra-sandbox-<name>-threads` (opens the
panel; amber when a session waits); on each session's pane bar
(`pane-<n>-x-<id>`): `model`, `state`, `interrupt`, `remote`, `close`; on
the main session's: `lead`, `plan`, `merge`, `threads`; in the top bar
`extra-topbar-plan` while a plan waits.

**The lead state.** The `lead` extra is carried by the main session's
name pill (shell/bar.rs `take_team`), one look per state
(`model::team_state`):

| State | Pill | A press |
|---|---|---|
| `leading` | purple, the leader mark ("Team on") | `lead_end` (the core's dialog) |
| `waiting` | blue outline, "waits for idle" after the name: the agent is busy, lead mode starts when it is idle | `lead_cancel` |
| `activating` | blue outline, "starting": `/lead` typed, the session has not confirmed | `lead_cancel` |
| `unled` | amber outline, "Plan without lead": a plan pending or a thread at work, no lead | `lead_start` (Resume lead: the daemon gives the open plans back to the session) |
| `off` | quiet outline ("Team") | `lead_start` |

The word shows under `pane-<n>-team-word`. "Team on" never comes from a
plan alone. A state that falls back to off with a `reason` says it once
as a notice ("Team mode for X did not start: ..."). The Threads panel
draws the same states (`lead-end`, `lead-cancel`, `lead-resume`,
`lead-start`).

**Intents** (indexes from 1):

| Intent | Arguments | Risk | What the core does |
|---|---|---|---|
| `threads_show` | `sandbox`, `tab` (`sessions`, `worktrees`, `board`) | direct | Opens the panel on that sandbox and tab |
| `thread_new` | `sandbox`, `kind` (`thread`, `session`) | direct | Opens New thread and reads the repositories |
| `thread_open` | `sandbox`, `name`, `repo` (`=` then its path; `=` is the workspace), `model`, `checkout`, `pane` | direct | Opens that session (spare pane): a thread the person starts from New thread opens its pane, as they asked for it now; repo, model and checkout only for a new one. Refused: a bad name, one of the other kind, no place free under `max_agent_sessions` (said with the names) |
| `thread_bash` | `sandbox`, `thread` | direct | A shell in its worktree |
| `session_interrupt` | `sandbox`, `thread` | direct | Escape into that Claude session (`POST /api/input`) |
| `remote_control` | `sandbox`, `thread` | direct | Reads the login, then turns Remote Control on, or says why not |
| `remote_login` | `sandbox`, `thread` | approval | Stops injecting the setup token, types /login |
| `worktrees_load` | `sandbox` | direct | Reads `data.worktrees` |
| `lead_start`, `lead_end` | `sandbox` | direct | Asks for Team mode (also Resume lead); off after the core's dialog says what it leaves |
| `lead_cancel` | `sandbox` | direct | `POST /api/sandboxes/{sandbox}/lead/cancel`: a lead mode waiting or starting does not start |
| `plan_review` | `id` | direct | Opens the plan dialog (and its draft) |
| `plans_reload` | none | direct | Reads plans and boards again |
| `plan_edit` | `id`, `index`, `field` (`name`, `goal`, `scope`, `repo`, `model`), `value` (none: empty, default repo, default model) | direct | Edits the draft |
| `plan_remove`, `plan_add`, `plan_reset` | `id` (`index`) | direct | Removes, adds, resets |
| `plan_variant` | `id`, `variant` (`split`, `shared`) | direct | The draft's variant |
| `plan_no_tests` | `id`, `on` | direct | Approve the plan without the test command the lead proposed (`clear_tests` in the approval) |
| `plan_refuse` | `id`, `note` | direct | Refuses; the lead reads the note |
| `merge_close_set` | `id`, `on` | direct | Close the threads once merged (default on) |
| `plan_approve` | `id` | approval | The core reads the sessions, its dialog lists the draft, the changes and the sessions it starts; over the ceiling it says how many are open, which, how many are free, how to free one. Approved (here or elsewhere), its threads open no pane on their own and the focus stays: the daemon starts them, the sidebar (under the leader), the Board and the Threads view list them at once, and a click on a row opens its pane (unlike the web UI's `openPlanPanes`) |
| `plan_merge` | `id` | approval | The core's dialog: order, note, base, distillate, whether threads close; refused while a shared plan's files are claimed, and while `merge_status.can_merge` is false, with its `why_not`. In `merged_ci` it says the click is a fast-forward to the tested tip of the CI branch; it posts the same route and body |
| `plan_merge_abandon` | `id` | approval | The core's dialog, then the merge is withdrawn: nothing merged, closed or deleted. Refused while merging, while a CI branch is being built, once merged or abandoned |
| `thread_close` | `sandbox`, `thread` | approval | Reads what closing loses; the dialog needs uncommitted files discarded explicitly; its panes go once closed |

### relay

What agents ask the person through the relay (`assets/js/relay.js`,
`docs/relay.md`), the multiple-choice questions agents wait on
(`AskUserQuestion`), and following a sandbox other than Jarvis from the
Jarvis panel (the non-Jarvis parts of `concierge.js`). Files:
`src/feat/relay/` (`mod.rs` rules, `round.rs` rounds, `shots.rs` pictures,
`follow.rs` following, `standin.rs`, `tests.rs`), `screens/relay.luau`,
`fixtures/relay/`, `scripts/demo-relay.sh`.

**Screen.** `relay` (panel "Requests"): the requests waiting, the one shown
(a question, a screenshot request, a round of questions, a virtual Mac
request, each in its state), those out with another sandbox, and agents
waiting on a choice. It opens by itself when a request arrives. A
request the daemon reminded the person of (it waited 10 minutes for them,
`reminded_ms`) shows "10+ min" in the list (`relay-req-<id>`) and "Waiting for
you for over 10 minutes" on its card (`relay-waited-<id>`).

**Notices and the Mac's notification** (D17 of the reliability audit). A
request that starts to wait for the person (pending, or answered for review)
is a notice offering `relay_open {id}`; several at once are one notice on the
oldest. Such a notice does not go after 4 seconds: it stays while that
request waits (`relay::notice_due`, `App::notice_sticky`), until the person
opens it or closes it. A newer notice puts it aside (`held_notice`) and it
comes back when that one goes, if the request still waits. The daemon's
reminder is a notice once more ("web-app has waited 10 minutes for you: a
question"). Requests are announced once, by id AND creation time: an id a
restarted daemon gives again is a new request, announced, and nothing typed
for the old one carries over. The system notification on the Mac is the
island's (`macos/SbxwIsland`, `RelayNotifier.swift`), not this app's: no new
crate. The phone's push is the daemon's (`svc/remote_push.rs`).

**Data.** `data.relay` (`GET /api/relay`, read again on each `relay` event:
`{requests}`, each `{id, from, from_thread?, kind: question|screenshot|ask|mac,
question, to?, answer?, shot_count, state: pending|routed|answered|approved|denied,
note?, created_ms, reminded_ms?}`); `data.sessions` (threads' key) for `AskUserQuestion`;
`data.relay_ui`, set by the core: `current`, `closed`, `rounds` (by id:
`{intro, questions: [{id, title, body, rec}]}`), `forms` (by id: answers),
`own`, `review`, `caption` (by id: what was typed), `shots` (by id: `{n,
source, w, h, kb, label, image}`, `image` a key for `image{}`; never the
bytes), `max_shots`, `picks` and `sending` (by session key
`sandbox::thread`), `local` (rounds found in a followed session, shown as
requests with `local = true`), `follow` (`{target, follow, choosing}`),
`voice` (`{id, index}`: the round answered by voice), `waited` (ids of the
requests waiting whose reminder came).

**Pictures.** Dropped on the panel, pasted (⌘V on macOS while the panel is
shown and no terminal has the keyboard), or read with "Paste from
clipboard" (`Action::ReadClipboard`). Decoded by `crate::picture` (limits
before decoding), scaled to 1600 px, PNG else JPEG; six at most, 6 MB each,
16 MB together, as the daemon checks. No intent takes a path.

| Intent | Arguments | Risk | What the core does |
|---|---|---|---|
| `relay_open` | `id` | direct | Opens the panel on that request |
| `relay_later` | `id` | direct | Puts it aside (it stays in the list), or closes a settled one |
| `relay_route` | `id`, `to` | approval | The dialog shows the question and the target; sends it to that running sandbox (a round: for a draft). Never a screenshot, a Mac request, the asker, or a stopped sandbox |
| `relay_deny` | `id` | direct | Refuses; a round from a followed session is set aside |
| `relay_set` | `id`, `field` (`own`, `review`, `caption`, `answer`), `value`, `index` | direct | What the person typed; give it on every edit |
| `relay_use_rec`, `relay_clear` | `id`, `index` / `id` | direct | A recommendation into an answer; a round's answers emptied |
| `relay_send` | `id` | approval | The dialog shows exactly what leaves (the text; the pictures' count, sizes and weight) and to whom; a round is masked first (`/api/voice/anonymise`, nothing sent if it fails). A round from a followed session is typed into it (`/api/chat/push`) |
| `relay_shot_paste`, `relay_shot_choose` | `id` | direct | Reads the clipboard's picture; the file dialog (not in this build: a notice says to drop or paste) |
| `relay_shot_remove` | `id`, `index` | direct | Removes a picture (none: all) |
| `relay_voice` | `id`, `on`, `index` | direct | Answers a round by voice: each question read, the next words spoken fill it (repeat, skip, back) |
| `relay_heard` | `text` | direct | The transcript for that round (the core sends it) |
| `relay_read` | `id` | direct | Reads a round aloud |
| `agent_pick` | `sandbox`, `thread`, `step`, `option` | direct | Picks an option of an `AskUserQuestion`; once every step has one, `POST /api/answer` |
| `follow_target` | `sandbox` | direct | Where the Jarvis panel's words go (none: Jarvis); kept across runs |
| `follow_toggle` | `on` | direct | Follow: every turn of the target's agents shown and read (off: rounds and `/grill-me` only) |
| `follow_send` | `text` | approval | Words for the target: masked, then a draft in the dialog; sent on Confirm only |
| `follow_choose` | `text` | approval | Words naming an option of the target's question: the dialog says which |

**Jarvis panel.** With a sandbox running, the panel's title is a picker
(`jarvis-target`): Jarvis, or a running sandbox. With a sandbox picked, a
Follow box (`jarvis-follow`) shows; typed or spoken words become
`follow_send` (or `follow_choose`) and never answer a Jarvis proposal; the
target's turns show in the log as `who = "agent"` with `from`.

**Extras.** `extra-topbar-relay` ("Requests" with the count waiting on the
person, or "Relay" while only a sandbox is answering),
`extra-sandbox-<asker>-relay` (`?` and a count), and on the asker's pane
bars, "asks you".

### files

A sandbox's Project panel and its code map (the web UI's `files.js`,
`upload.js`, `codemap.js`; `docs/web-ui.md`, "The Project panel"). Files:
`src/feat/files/` (`mod.rs` rules, `view.rs` rows and previews, `codemap.rs`
the map's parser, `standin.rs`, `tests.rs`), `screens/files.luau`,
`screens/codemap.luau`, `fixtures/files/`, `scripts/demo-files.sh`. Report:
`.sbxw-artifacts/desktop/FILES.md` in the main checkout.

**The explorer** (thread desktop-views, `VIEWS.md`): `files` is an
**overlay** over the pane area (`Slot::Overlay`, title "Files"), opened by
the rail's Files (`files_show`: the sandbox used last, else the focused
pane's, else the selected one), a pane bar's Project (`files_open
{sandbox}`, and for a thread's pane `{sandbox, worktree}`: at that
worktree, its path from the daemon's listing) or the Worktrees view.

Its layout (views2, `VIEWS2.md`): **across the top** where the list is, as
pressable segments (`files-crumb-<i>`, the agent's home shown as `~`, a
long path folding in the middle into `files-crumbs-more`, never cut at its
end), back `files-back`, up `files-up`, copy the folder `files-copy-folder`,
pin it `files-star`, **Go to…** `files-goto` (the typed path `files-path`),
then **Upload** `files-upload` and the "…" menu `files-actions`
(read again, the code map), read again `files-refresh`. **In the title**:
the scope picker `files-sandbox` (its second level the worktree the list is
in). **Left**: the **Locations** (`files_ui.locations`, `view::locations`:
"This sandbox": Home (the agent's home, `/home/agent`), Workspace (the
project, this computer's folder mounted at the same path; Code map
`files-codemap` beside it), Temporary files (`/tmp`), File system root,
the mounts, Kit logs; "Shared with sbxw": Artifacts, Recent; "Worktrees":
`files-worktree-<thread>`; "Pinned"; keys `files-loc-<id>`, the current one
`files_ui.location` marked; under 760 pt a menu `files-locations`). **At
the foot of the Locations** (from 900 pt; they scroll above it), the
upload block `files-drop`: "Upload" with its plus, "Drop files here or on
a terminal" (wrapping), Choose… `files-upload-pick` and Paste
`files-upload-paste` as small buttons, then the files waiting, stacked
(`files-staged-<n>`: the name and its close, the size and where it goes,
the progress, Upload). It is always there; the header's Upload marks it
(an accent border). While files are dragged over the explorer
(`data.app.overlay.drop_over`) it is blue, as the explorer is. Under 900 pt
(no left column) Upload opens it at the top of the list's column (narrow:
inside the scrolled tree), as it does while files wait or are dragged.
**The toast** `files-toast`: after a download, a folder download or a
reveal, "Saved <name>" (the check, the name cut, Show in folder
`files-show-saved`) floats at the bottom centre of the explorer (12 pt
above its foot, 420 pt wide at most, pinned at its own width so it takes
no click outside it), with the surface fill, a 1 pt `border`, radius `md`
and the FLOAT shadow (`lift = "float"`). It goes after
`view::TOAST_SECS` (6 s: the feature posts `files_toast_expire` from a
timer thread, as a socket thread posts, the core having no feature timer)
unless the pointer rests on it (hover zone `files-toast`): then when it
leaves. A download that failed shows its words in red with a close
`files-toast-close` and stays until closed. Then
the list: the filter `files-filter` with **Hidden** `files-hidden` and the
sort `files-sort` (`files_sort`), and the rows as a tree (`files-rows`, one
keyboard list): a folder opens in place (`files_expand`, read from the
sandbox, its count once read; `files-into-<id>` lists it), Up and Down
move, Right opens, Left folds, Enter presses, typing filters
(`files_key` when nothing of the explorer has the keyboard, the rows'
`on_left`/`on_right` when a row has it). Empty and failed lists say what to
do. Right the reader: Markdown at about 70
characters in the prose face, paragraphs reflowed, its outline beside it
(`files-outline-<i>`, `files_outline`: that section, Whole file),
relative links and `[[wiki links]]` rewritten to
`https://files.sbxw.invalid/<path>` and followed inside (`files_link`:
its folder listed, the file selected), other links through `open_link`;
text and code with line numbers. Under 760 pt wide (`data.app.overlay.w`)
one pane at a time (`files-to-tree`). `src/feat/files/explorer.rs`.

**Screens (before the explorer, kept as written).** `files` (panel "Project", 460 pt): tabs Artifacts
(`.sbxw-artifacts`, newest first, each folder one folded row with its file
count until pressed), Files (a folder of the sandbox, the
places, Hidden files, the typed path) and Recent (written lately anywhere,
by age, Show in folder); an upload card; a filter; a preview under the list
(text, Markdown through `markdown`, a picture through `image`, HTML as its
source) with Download, Show in folder, Open, Save as, Copy path, Delete.

**A whole folder.** A folder row's ⌄ button (on hover), or the one beside the
path for the folder listed, shows that folder in the reader: "Folder", its
file count and size (`GET .../fs/folder/size`, counted in the sandbox without
reading the files), and **Download folder**, Show in folder, Save as….
Download folder saves `<name>.tar.gz` into Downloads through the core, as a
file download (the core's clean name, never over a file), with "Packing…"
until the answer, the daemon's refusal as text, then Show in folder. **The
cap here is 128 MB** (`view::FOLDER_MAX`, the core's own download limit,
under the daemon's 256 MB): a folder whose files hold more says so ("152.0 MB:
over the 128.0 MB a folder download allows here") and its buttons are off.
There is no byte-by-byte progress: the daemon packs the folder whole before
it answers (see `docs/web-ui.md`, "Downloading a folder").
`codemap` (panel "Code map", 520 pt): the map or a lens, a file or one of
its sections drawn through `markdown` (a `[[wiki link]]` is a link to
`https://sbxw-codemap.invalid/<file>#<section>`, followed in the panel; a
broken one is marked; a source reference is code, copied from the side
list), Referenced by, Outline, Code referenced, search, Generate, Write a
lens, Diagram.

**Long lists.** A list draws its first 200 rows (`PAGE` in `mod.rs`), and
200 more on each **Show N more**, under a count line ("1,240 files, showing
200. Filter by name to find one."); the filter runs over every entry before
the page is cut, and the selected row is always drawn (after the page when it is further down). The core refuses a
render past 5000 elements (`luau.rs`, a safety cap on what a screen may
build per frame), so a screen that lists the daemon's data must bound it:
this panel at about 1,700 elements for a full page.

**Extras.** On every pane bar of a sandbox, `pane-<n>-x-project` ("Project",
opens the panel); on a sandbox's row, `extra-sandbox-<name>-codemap` ("map…",
amber) while its map or a lens is being written.

**Keys** (scripts press them): rows `files-row-<id>` (`id`: an artifact's
path under `.sbxw-artifacts`, else the file's absolute path), Recent's
`files-show-<id>`, Locations `files-loc-<id>` (the tabs `files-tab-<tab>` before views2), actions `files-download`,
`files-reveal`, `files-open-with`, `files-save-as`, `files-copy-path`,
`files-delete`, a folder's `files-folder-<path>` (on its row), `files-download-listed` (beside the path), and in the reader
`files-download-folder`, `files-reveal-folder`, `files-save-as-folder`, the preview's body `files-preview-<state>` (`loading`, `text` (HTML too), `markdown`, `image`, `note`, `error`, `folder`), uploads `files-drop` (the drop target), `files-upload-send-<n>`
and `files-unstage-<n>` (by position), `files-more` (Show N more), a folder row
`files-row-<folder path>`; the map's `codemap-docs` (picker,
items by file key), `codemap-outline-<section id>`, `codemap-hit-<file key>`,
`codemap-diagram`.

**Data** (each a route's whole answer, read for the sandbox the panel shows;
a failed read is in `data.app.errors[key]`, a string):

| Key | What |
|---|---|
| `artifacts` | `GET /api/sandboxes/:name/artifacts`: `{dir, entries: [{path, name, size, modified}], codemap, lenses, run}` |
| `files_dir` | `GET /api/sandboxes/:name/fs?path=`: `{path, parent, entries: [{name, kind, link, size, modified}], offline, workspace, artifacts, home, mounts, worktrees}` |
| `files_recent` | `GET /api/sandboxes/:name/fs/recent`: `{files: [{path, size, modified}], days, workspace, home, roots}` |
| `codemap` | `GET /api/codemap`, read again on each `codemap` event: every run `{sandbox, kind: map\|lens, phase: writing\|done\|failed, note}` |
| `files_ui` | Set by the core: `sandbox`, `tab`, `rows` (`id`, `name`, `kind` (`folder` for a folded artifact folder, with `count` and `open`), `icon`, `dir`, `depth`, `size`, `when`, `fresh`, `group`, `can_delete`, `selected`), `count` (`total`, `shown`, `more`, `label`), `places`, `path`, `can_up`, `can_back`, `offline`, `selected`, `preview` (`state`: `loading`, `text`, `markdown`, `html`, `image` with `key`, `note`, `error`, `folder` with `counting`, `files`, `bytes`, `line`, `over`, `error`), `selected.packing` (a folder download under way), `staged` (`index`, `name`, `size`, `to`, `problem`, `state`, `progress`), `saved`, `toast` (`seq`, `name`, `path`, `error`, `expired`), `run`, `has_map`, `errors`, `loading` |
| `codemap_ui` | Set by the core: `sandbox`, `maps`, `root`, `loading`, `error`, `map` (`docs`, `count`, `broken`, `doc`: `{key, title, path, section, markdown, outline, backlinks, code}`), `diagram`, `run`, `writing`, `running`, `lens`, `lens_left` |

**Rules.** File content is read raw (`Call::raw`): text and Markdown are
drawn by the core's elements, pictures decoded by `crate::picture` (24 MB
before decoding, scaled to 1600 px), HTML never drawn. Nothing a sandbox
names is written on this computer by the feature: downloads, opens, reveals
and uploads are the core's (`src/files.rs`, names and verdicts by
`src/safe.rs`). No intent takes a local path: files come from a drop on the
panel or a pane, a paste, or the file dialog.

**Dropped files.** A drop on a terminal pane asks once for the whole drop.
Once the person ticked "Do not ask again for files I drop" (kept in the
app's state file, `uploads.skip_drop_dialog`; Setup, Secrets, "Dropped
files" turns it off), a plain drop uploads at once and a notice says
"Uploading brief.md to web-app…", then "Uploaded …". The dialog still asks,
saying why, for a name that looks like a secret (`.env*`, `*.pem`, `*.key`,
`id_rsa*`, `id_ed25519*`, `*.p12`, `*.pfx`, `credentials*`, `.npmrc`,
`.netrc`, `*.kdbx`), a file in a `.ssh`, `.aws` or `.gnupg` folder, more
than 10 files, more than 25 MB in all, or a file that cannot be checked
(`src/feat/files/drops.rs`, one list read by the feature and the core).
The refusals stay: empty, over 128 MB, a folder, a stopped sandbox. A drop
on the panel, Choose… and Paste are unchanged: each file waits on the
panel and its Upload button opens its own dialog.

| Intent | Arguments | Risk | What the core does |
|---|---|---|---|
| `files_open` | `sandbox`, `tab` (`artifacts`, `files`, `recent`) | direct | Opens the panel on that sandbox (the last tab, else Artifacts) and reads its lists |
| `files_open_path` | `sandbox` / `path` / `thread?` | direct | A path read from a terminal pane (⌘ or Ctrl + click): that sandbox's Files. A file: its folder listed, the file selected. A folder (trailing `/`): listed, no row selected (not even its README). An anchored path with no extension: its folder's listing decides (a folder is listed, a file selected). Re-checks the terminal-link rule (`filelink.rs`, 300 characters); relative paths resolve in the thread's worktree, else the workspace; a path not there says so. Only opens the explorer. |
| `files_tab`, `files_place`, `files_browse`, `files_up`, `files_back` | `tab` / `id` / `path` (`~` is home) / none | direct | Tabs, places, folders |
| `files_filter`, `files_hidden` | `text` / `on` | direct | The filter, dotfiles |
| `files_more` | | direct | 200 more rows of a long list |
| `files_fold` | `id` (a folder row's) | direct | Opens or folds an artifact folder |
| `files_select` | `id` (a row's) | direct | A folder is listed, a file previewed; a row not listed is refused |
| `files_show_in_folder` | `id` | direct | Recent: lists its folder, on it |
| `files_copy_path` | none | direct | The path as the agent sees it, through `copy_text` |
| `files_folder` | `id?` (a folder row's; none: the folder listed) | direct | Shows the folder in the reader, with its file count and size as the daemon counts them, to download it whole |
| `files_download`, `files_reveal` | none | direct | Into Downloads (never over a file); and shown in its folder. A folder in the reader goes whole, as `<name>.tar.gz` |
| `files_show_saved` | none | direct | Shows the last file saved in its folder |
| `files_toast_close` | none | direct | Closes the toast |
| `files_toast_expire` | `seq` | direct | The saved toast's time is up (sent by the feature's own timer); a failure, or another toast's number, is left as it is |
| `files_save_as` | none | approval | The save dialog, after the core's dialog names the file (or the folder, its count and size) and its size |
| `files_open_with` | none | approval | The core downloads it and judges it (its name and first bytes); its dialog says what it is; a program, script, installer or shortcut is never opened; a file it warns about needs the box ticked |
| `files_delete` | none | approval | The dialog shows the path and size (a folder: everything in it); only under the workspace, a read-write mount, home or /tmp, never those folders, `.sbxw-artifacts` or `.git` |
| `files_star` | `path` | direct | Pins a folder among the places (kept across runs) |
| `files_refresh` | none | direct | Reads the lists again |
| `files_upload_pick`, `files_upload_paste` | none | direct | The file dialog; the clipboard's picture |
| `files_upload_remove` | `index` (none: all) | direct | Drops a waiting file |
| `files_upload_send` | `index` | approval | Waiting file `index`: the dialog shows its local path, size, the sandbox and session; the core streams it (128 MB at most), then the daemon's path is typed into that prompt, not submitted |
| `files_upload_dropped` | none | approval | The files dropped together on a terminal pane: ONE dialog lists them (local paths, sizes, total, the sandbox and session) and says what happens; until the person chose otherwise it carries the box "Do not ask again for files I drop" (core-drawn, unticked; only it sets that choice). The core then uploads them one after the other (`core:uploads`); a failure stops the rest and the notice says what was sent and what was not; the paths the daemon made are typed together, not submitted |
| `files_upload_ask_again` | none | direct | Setup's "Ask before uploading dropped files": the dialog comes back for every drop. The only direction an intent can take |
| `codemap_open` | `sandbox`, `map` (a lens's folder) | direct | Opens the panel and reads the map's files |
| `codemap_doc` | `doc`, `anchor` (a section id) | direct | A file, or one section |
| `codemap_link` | `url` | direct | A link pressed in a map file: a wiki link followed in the panel, any other in the browser |
| `codemap_filter` | `text` | direct | Searches the map's files |
| `codemap_generate` | `sandbox` | direct | `/codemap` in a background session (the daemon refuses a stopped sandbox, no codemap kit, one already running) |
| `codemap_lens_set`, `codemap_lens` | `text` / `sandbox` | direct | The reader (400 characters at most); `/codemap-lens` in the background |
| `codemap_diagram` | none | approval | The map's Archify diagram is HTML the app does not draw: saved into Downloads, judged by the core, opened in the browser after its dialog |

#### Terminal links (file and folder paths in a pane)

Holding ⌘ (macOS) or Ctrl over a terminal pane, a file or folder path in its
text is underlined, the cursor is the pointer, and a tooltip names the path;
⌘ or Ctrl + click opens the Files screen on it in that pane's sandbox
(`files_open_path`). Rule: `src/terminal/filelink.rs`, the web's
`chatFilePath`: ASCII path characters, a slash or a leading `./` `~/` `/`, no
`..`, no scheme, 300 characters, and one of: a file name with an extension from
the web's list (an optional `:line`, ignored); a folder ending in `/`
(`src/app/`, `~/projects/`); an anchored path (`/`, `~/`, `./`) two segments
deep with no extension (`/tmp/x/y`, `~/dev/sbxw/Makefile`), a folder or a file
as the daemon's listing says. `and/or`, `TCP/IP` and URLs never link; trailing
punctuation is not part of a path. Both clients are tested on one table,
`tests/js/file-link-cases.json`, and a test reads the web's allow-list and cap
from `assets/js/chat-core.js`, so the two cannot drift silently. Detection runs only
for the row under the pointer (and the rows a soft wrap joins, 3 each way) and
only while the modifier is held. An OSC 8 link keeps priority; its text is
never a file link. With mouse reports on, ⌘ + click goes to the program as
before; ⌘ + Shift + click opens the path. Relative paths resolve in the
thread's worktree (a thread pane) or the workspace: the shell's real current
folder is unknown. The one standing rule that changed: a path read from
terminal text may reach the Files screen, only after that gesture, only as a
request to open the Files screen; nothing is executed, typed, downloaded or
opened outside the app. Demo: `scripts/demo-ttylinks.sh`.

### browser

The sandbox's own Chromium, watched and driven from a docked panel (the web
UI's `browser.js`; `docs/chat.md`, "The browser side panel"). Files:
`src/feat/browser/` (`mod.rs` the session and rules, `proto.rs` the
`/ws/browser` line protocol, `standin.rs`, `tests.rs`), `screens/browser.luau`,
`screens/browser-settle.luau`, `fixtures/browser/`, `fixtures/browser-settle/`,
`scripts/demo-browser.sh`. Report: `.sbxw-artifacts/desktop/BROWSER-MAC.md`
in the main checkout.

**Screens.** `browser` (panel "Browser", 720 pt): the sandbox's name, Hide,
Close browser; back, forward, reload, the address bar (input
`browser-url`), the device select (`browser-viewport`); the hosts the
sandbox's network policy refused (`browser-allow-<host>`,
`browser-ignore-<host>`; an organisation or deny-rule host says why it cannot
be allowed); how many hosts were allowed from here; the error; the picture,
`surface{ id = "browser" }`, last; a line with the keyboard hint and the
measured frames per second and input-to-picture latency. When the browser
could not start (an error before its first picture) or has ended: no
picture and no hint, the controls off, the error kept, and the next steps
`browser-kits` (the Kits panel on this sandbox's sbxw.toml, `kits_show`) and `browser-recreate`
(the sandbox's actions, where Recreate goes through the core's dialog).
`browser-settle`
(dialog, 440 pt): on close, the hosts allowed from the panel, Keep them
(first) or Remove them.

**Extras.** On every pane bar of a running sandbox, `pane-<n>-x-browser`
("Browser"): blue while its agent uses its browser (`browser_active`), a
press opens the panel on it, or hides it when it shows that sandbox.

**Data.** `browser_active`: `GET /api/browser/active` (`{ok, sandboxes}`),
read again on the `browser` event. `browser_ui`, set by the feature:
`sandbox`, `status` (`idle`, `starting`, `live`, `closing`, `ended`), `url`,
`title`, `viewport`, `viewports` (`{id, label, disabled}`), `blocked`
(`{host, kind, fixed}`), `allowed`, `error`, `focused`, `release_key`,
`frame` (`{w, h}`), `stats` (`frames`, `refused`, `fps`, `latency_ms`).
`browser_settle`: `{sandbox, hosts}` while the close question is open.

**Intents.** `browser_open {sandbox}`, `browser_hide`, `browser_close`,
`browser_nav {url}`, `browser_back`, `browser_forward`, `browser_reload`,
`browser_viewport {value}`, `browser_ignore {host}`, `browser_settle
{keep}`, `browser_page` (the keyboard to the picture): direct.
`browser_allow {host}`: **Approval**, the core's dialog, only for a host the
bridge reported and that is not an organisation or deny-rule host; it posts
to `POST /api/sandboxes/:name/browser/allow` (the rule for that sandbox only,
remembered as the panel's so closing can remove it).

**Rules.** Frames go from the socket's thread to the core's surface
(`surface::set_encoded`, JPEG only, base64 checked strictly): Luau never sees
a pixel or an input event, and is not re-rendered per frame (the chrome's
state is set on a change, and at most once a second for the numbers).
Everything else the sandbox sends is cleaned, bounded one-line text. An
address typed in the bar is sent to the sandbox's browser (`nav`) and never
opened on the person's machine; nothing from the sandbox reaches the
clipboard or `open_link`. A paste is the person's own ⌘V / Ctrl+V on the
picture, text only. Hiding the panel leaves the browser running; no
automatic reconnect (a new socket runs a new bridge in the sandbox).

### commits

The Commits view: an **overlay** over the pane area (`Slot::Overlay`), from
the rail's Commits entry (browse zone; a pane's `commits` extra is left out
of the pane as the rail's, see "What the ⋯ menu leaves out") or the Worktrees view (filtered to a thread's branch). Read only.

Data: `GET /api/sandboxes/:name/commits?limit=50&before=&ref=` per sandbox
(six at a time), `.../commits/:hash` for a row opened; nothing in the
background. `data.commits_ui`: `sandbox` (nil: every one), `branch`,
`filter`, `unmerged`, `sandboxes` (`{id, label, color}`), `rows` (`id`
`<sandbox>:<hash>`, `hash`, `short`, `subject`, `author`, `when`, `at`,
`sandbox`, `color`, `branch`, `branch_label` (a thread's name for
`sbxw/<thread>`), `thread`, `merged`, `files`, `expanded`, `selected`),
`count` (`more_rows`), `detail` (`loading`, `error`, `body`, `files`: `path`,
`added`, `removed`, `binary`, `open`, `groups` (`{kind, text}`: `add`, `del`,
`hunk`, `ctx`, 600 lines drawn at most), `note`; `files_more`, `truncated`),
`notes` (a sandbox not available, an error).

| Intent | Args | Effect |
|---|---|---|
| `commits_show` | `sandbox?`, `branch?`, `hash?` | Opens the view (every sandbox, one, one branch) and reads; with `hash` (and `sandbox`) that commit opened, as a dot pressed in the Worktrees graph |
| `commits_sandbox` | `sandbox?` | All, or one |
| `commits_filter` | `text` | Subject and author |
| `commits_unmerged` | `on` | Only what the main checkout's branch does not hold |
| `commits_branch_clear` | | Every branch again |
| `commits_expand` | `id` | Opens a row in place (its body, its files), or folds it |
| `commits_file` | `path` | A file's diff under it, or hides it (only a path the commit lists) |
| `commits_more` | | 50 more (the next page where needed) |
| `commits_copy` | `id` | The full hash on the clipboard |
| `commits_refresh` | | Reads again |
| `commits_key` | `key` (`up`, `down`, `enter`) | The view's keys when none of its controls has the keyboard |

The scope picker `commits-sandbox` is in the title ("Commits in [All
sandboxes v]"), a branch as its second level (`commits-sandbox-narrow`, its
cross `commits-sandbox-narrow-clear` runs `commits_branch_clear`).

Commit messages are drawn as plain text: no pills, no Markdown, no links.

### board

Shown to the person as the **Dashboard** (`board::TITLE`): the rail entry,
the overlay's title (both screens: the conversation and the Kanban), the
sandbox row's menu, the palette. The feature, its intents (`board_*`), its
element keys (`rail-board`, `board-*`) and its files keep the name `board`,
so scripts and a stored state keep working. Not the shared plan's claims
board (the Threads panel's Board tab, `read_board`), another thing.

An overlay (`src/feat/board/`, `screens/board.luau`), the rail's activity
zone (order 35, icon `board`, purple: the team; the entry "Dashboard" always
opens the conversation, the top bar's Kanban button the Kanban) and each
sandbox's row menu (opened on that sandbox's team). A **team** is a plan with its sandbox, lead
and threads, and is named by its threads' names ("api-routes + web-page",
past three "+ n more"; its sandbox when it has none), in the mono face with
its sandbox's colour mark. The title's **picker** shows those names, then the
summary when there is room; its list (drawn by the screen, not the core's
320 pt menu) gives each team its names, its summary under them on up to two
lines, its state chip and age at the right, "All teams" first, 20 teams a
page ("Show n more"). The list is at least as wide as the button, up to
560 pt, never wider than the overlay; a press outside closes it (and does
nothing else), a pick closes it, its rows are one keyboard list (Up, Down,
Enter; the keyboard lands on the team shown), Escape closes it through
`board_key` (`board_ui.popup` is true while it is open). One column. Under
the title, a **summary** (`board-summary`): the lead as a chip, each thread
as a chip (status dot, name, state; a press filters the conversation to it,
pressed again every thread; its terminal and its session in Sessions as
icons shown on hover and keyboard focus, their room kept), then counters
that are the filters below (standing decisions, standing warnings,
questions waiting in Requests, commits with their +/- totals: a press
applies that filter and marks the counter, pressed again All), the Plan
dialog, Threads and Worktrees, and Details (each thread's worktree and
model, in place). The team's state is beside the picker only. Under 760 pt
the summary is one wrapping line: counters without their words (tooltips
keep them), the links as icons, a thread's terminal and session in Details.
Under it the conversation, the overlay's whole width: the team's timeline (`GET /api/teams/:id/timeline`) as a **conversation**,
oldest at the top, opening at the newest: the person's acts on the right,
the lead and threads on the left, sbxw's own events as centred lines. Each
message has its **author line above its bubble**: the dot and the name (mono,
the thread's colour; the lead by its name when the daemon has one, purple),
the role chip "lead" only when the name does not already say it, "to" the
addressee ("the team", "you", a thread, the lead's name), the time; the
person's on the right with "You". The same author to the same addressee
within 5 minutes shares one author line, the bubbles stacked under it. A
bubble hugs its content up to a width that follows the conversation column
(62 percent of it, at least 520 pt, at most 720 pt, about 112 characters of
prose; under 760 pt, one column, 520 pt: `model::bubble_max`, mirrored in
`board.luau`; cards, notes and tinted reports share it) and holds
only the content: a small quiet kind line ("direct message", "Decision team
note", "status report" with the state chip at the bubble's top right: keys `board-kind-<id>` and `board-state-<id>`, a gap of at least 12 pt between them), the words (prose face, pills),
Show more. A run of more than 32 characters without a space gets a break
every 24 (zero-width spaces, drawing only), so a path, a hash or 200
characters without a space wrap inside the bubble. A status report with
words is tinted by its state: done green (`green-tint`, `green` border, as a
Decision), blocked amber, failed red, others neutral; one with no words (or
only spaces) is **one quiet line**, no bubble: dot, name, an arrow
(`chevron-right`), the new state's chip, the time; one agent's consecutive
bare changes fold into the last with "×n" and the trail in its tooltip. "All
teams" tags each row with its team's names (its summary in the tooltip). A
plan is a card (why, its threads as chips, Show goals); the board's decisions
(green), warnings (amber), questions (blue) and handoffs labelled notes; a
round of questions with each answer marked; a team note (`team_note`) as a
note labelled Info, Warning or Decision with "team note"; a direct message
(the agents' own `SendMessage`, as sbxw's hook saw it) as a bubble labelled
"direct message"; a thread's consecutive commits one row ("api-routes made
4 commits") that opens. A text drawn on more than 8 lines at the bubble's width folds (Show more,
6 lines shown: `model::FOLD_DRAWN`), decided by the screen at the width it
draws now (the data holds the whole text); on a wide window a few lines are not folded; day separators; relative times, the exact UTC time in the tooltip.
A filter row (All, Decisions and warnings, Questions, Messages, Commits, Status) and
words, applied by the daemon before paging (the thread: its chip). A decision
or warning is drawn once, in the conversation; one that still stands has a
"standing" mark on its bubble. On a team, the **Commits** filter shows, at
the conversation's width, a lane graph (drawn with `paint`, 260 to 960 pt),
each thread's totals and commits, a commit opened in place with the Commits
view's diff groups (`commits::diff_groups`); in All, commits stay folded rows.
**Folds hold the page**: once a fold is pressed (a long text, goals, a
thread's commits, a commit, a file's diff, Details) the conversation stops
following its end until the next team, filter or search, so the pressed
bubble's top and the rows above it stay put; folding a bubble whose top is
above the view scrolls it back into view (`ScrollIntoView`). At the very end
of the conversation a fold removes content under the view: the rows come
down together by that much.

**Merge states.** A team whose plan has a merge says its status with the
daemon's own word (`merge_status.word`, from the plans the app reads; its
`detail`, and why Merge is not offered, in the tooltip), beside the picker
and in each row of the picker's list, instead of the team's state:
"Testing the CI branch" and "Building the CI branch" blue (work under way,
as a running team), "Merged to CI" and "Merge proposed" purple (waiting for
the person's Merge, as the Merge proposed chip), "CI rebuild waits" and
"Main moved: CI rebuild waits" amber (waiting, as a blocked thread), "CI
failed" and "Merge failed" red, "Merged to main" green (`model::merge_tone`,
`model::merge_badge`). A team with no merge, or an older daemon with no
`merge_status`, shows its state as before. A long word narrows the picker
down to its least width (104 pt); past that the word is drawn first in the
summary, under the title (`board-chip-state`), never outside the overlay.
A plan in any CI state is at work (the rail's count, a sandbox's current
team: `model::plan_at_work`); Merged to main is not. The **CI run** is part
of the test rows: each `ci_step` event (built, passed, failed, stale,
rebuild, closed) is a centred tinted line like a test job, labelled with
the merge status the step led to in the daemon's words ("Merged to CI",
"Main moved: CI rebuild waits"; a rebuild "Rebuilding the CI branch"), the
daemon's line after it ("main moved from 7e83ca2 to 32f28ea: rebuilding
sbxw/ci-p"); the battery sbxw asks on the CI branch (`test_job` with asker
`sbxw`) reads "CI tests passed after a retry", "on the CI branch, asked by
sbxw · 2 attempts" (no attempt for a reused result); `merge_finished` says
the daemon's word and the CI branch ("Merged to main from sbxw/ci-p: a,
b"). All of it is data, drawn as text. Fixtures: `fixtures/board/ci.json`
(opened on the team testing after a rebuild) and its views `ci-building`,
`ci-merged-ci`, `ci-failed`, `ci-stale`, `ci-main`, one team per state,
written by `gen.py` beside `basic.json`.

Read only: every intent is Direct and reads; links go to the plan dialog,
the Requests panel, a terminal, Commits, Worktrees, the Threads panel, the
Sessions. Agent text is drawn in the prose face with pills, never as a
control. 80 rows a page drawn (Show earlier, then an older page of 100
events). Data only while it shows; the `team` event reads the newest page
again.

| Intent | Args | Effect |
|---|---|---|
| `board_show` | `sandbox?`, `team?` | Opens it on that team (`all`: every team), else on that sandbox's team, else the last shown or the selected sandbox's |
| `board_team` | `id` | Another team, or `all` |
| `board_filter` | `group?`, `thread?`, `text?` | Filters (applied by the daemon) |
| `board_more` | | Earlier rows, then an older page |
| `board_expand` | `id` | Opens or folds a row (long text, goals, commits; `summary`: the summary's details) |
| `board_commit`, `board_file` | `hash`; `path` | A team commit opened in the Commits filter; a file's diff |
| `board_open` | `what`, `thread?`, `relay?` | Where one acts: terminal, monitor, commits, worktrees, plan, requests, threads |
| `board_refresh` | | Reads again |
| `board_picker` | `open?`, `more?` | Opens or closes the team picker's list (none: toggles), or lists 20 more teams in it |
| `board_key` | `key` (`up`, `down`, `enter`, `escape`) | The picker's keys while its list is open; Escape closes the list only |

The filter row starts with the **Conversation | Kanban** switch
(`board-view`, a two-position `select`): Kanban runs `kanban_show`.

### kanban

The Dashboard's second view (`src/feat/kanban/`, `screens/kanban.luau`, the
form `screens/kanban-form.luau`, the source dialog `screens/kanban-source.luau`): an overlay titled "Dashboard" (icon `board`),
opened by the switch or by the top bar's Kanban button (`kanban_toggle`),
with the same switch (`kanban-view`) and the sandbox's scope capsule in its
title (`kanban-sandbox`, All sandboxes first). Five columns side by side
(`kanban-lane-<id>`: planned, progress, test, ready, merged), each with its
count and its cards scrolling inside it (`kanban-scroll-<id>`); each
column's title and help are the daemon's (`columns[].title`, `help`:
"Merged to main"), never the desktop's, 30 cards a
page (`kanban-more-<id>`, four pages at most), Merged's older ones behind
`kanban-older`. Under 900 pt one column at a time, picked with
`kanban-colpick`. A card (`kanban-row-<key>`, the daemon's key made safe):
its T-number (or "thread", dim, for a thread with no ticket), its age, its
title on two lines (the daemon's `title_short` under 300 pt a column, else
`title_long`: one comparison, no string work), its marks on a wrapping line
under the title, exactly as the daemon sends them (`marks[]`: its label on
the card, its help line in the tooltip, its tone; priority, proposed,
blocked, failed, merging, merge failed, plan refused, team ended, files,
validated, and the CI marks "CI testing", "CI rebuild waits", "CI failed",
"Merged to CI"; the desktop never derives a mark from a plan's state), its
thread's dot and name, the sandbox's colour square in All; a red edge for
failed or refused work and CI failed. A
press opens the ticket as a panel over the board (`kanban-panel`, at most
640 pt, pinned right): its column (the board's title for it), its marks
with the same help lines, title, description (prose face,
pills), pictures previewed in place (their bytes read by the feature,
decoded by the core: `Action::Image`), files as chips (a press saves it to
Downloads and shows it), priority and Move up / Move down / To the top for a
planned ticket with no thread (else the reason it does not move),
"Validated on my machine" on a merged one, the thread with Terminal,
Sessions, Commits, Worktrees and Plan, the history, Send to the lead
(disabled with its reason). Escape (`kanban_key`, `kanban_ui.popup`), its
cross or a press outside (`kanban-panel-dismiss`) closes it and gives the
keyboard back to its card. Keys while no control has the keyboard: arrows
between cards and columns, Enter opens, N a new ticket. The form is a modal
(no light dismiss): sandbox (new tickets), title, description (`tone =
"compose"`), priority, files (Attach… opens the file dialog, Paste picture
reads the clipboard, a drop on the dialog; each checked by
`drops::guardrails`, a secret-looking one marked "Look first" and asked
about once, with the ack box, at Save), Save (Cmd+Return).

**Ticket sources.** A daemon with ticket sources puts a **Backlog** column
first (`kanban-lane-backlog`): the issues of a tracker (GitHub for now)
imported as tickets. The columns then share the width six ways; under
1,080 pt (180 pt a column) one column at a time. An imported card (`origin`
from the daemon: tracker, key, url, upstream) shows its issue's key
(`acme/web-app#12`) as a link button (`kanban-row-<key>-issue`) that opens
the issue in the browser through `kanban_issue`: the url stays in Rust and
opens only when it is http or https (`markdown::safe_url`); any other is
shown as the key alone, disabled. A Backlog card with `can_plan` has one
action, "Move to Planned" (`kanban-row-<key>-plan`, `kanban_plan`, posts
`/plan`): the one way out of Backlog, the person's; there is no drag and no
automatic move. The mark `closed-upstream` is a quiet "closed upstream"
(text, not a pill): its issue was closed, or lost the sync label, in the
tracker, and the work here goes on. An imported ticket's title,
description and files are read-only (no edit, delete or file removal, each
refused in Rust too); its priority (in Planned) and its validation (in
Merged) stay the person's. Its panel has one line
(`kanban-imported`) saying its words are edited in the tracker, and "Open in
GitHub" (`kanban-issue`); a Backlog ticket's panel also has "Move to
Planned" (`kanban-plan`). A ticket the person creates goes to Planned.

With one sandbox shown, a line under the summary (`kanban-source`) says its
source (`GET /api/tickets/:sb/source`): "No ticket source" and Connect
(`kanban-source-open`); or the tracker, the repository, the filter label
and the state (synced so long ago, syncing, or "sync failed:" and the
daemon's hint, the tracker's own error in the tooltip), with Refresh
(`kanban-source-refresh`, posts `/source/refresh`) and "Source…". The
dialog `screens/kanban-source.luau` (a modal, 560 pt, no light dismiss, its
fields scrolling above Cancel and Save): tracker (`kanban-source-tracker`,
GitHub only, shown so the choice is visible), repository
(`kanban-source-repo`, `owner/name`, filled from `detected` when the daemon
found one), filter label (`kanban-source-label`, `sbxw` by default),
"Close the issue when its ticket is merged" (`kanban-source-close`, off by
default), one line saying what sbxw writes to the tracker (one comment and
one state label per issue, under the person's own account), Disconnect
(`kanban-source-remove`, only when a source is set), Cancel and Save
(Connect the first time). Save and Disconnect are Approvals: one Confirm
each, saying what is written outward; Disconnect's, in red, says the
imported tickets still in Backlog are removed. The `tickets` event with
`what` `source` or `synced` reads the source again. An older daemon (no
Backlog column, a 404 on the source route) leaves the Kanban as it was: no
line, five columns. Fixtures: `fixtures/kanban/backlog.json` (source on),
`source-off`, `source-syncing`, `source-failed`, written by
`fixtures/kanban/backlog.py` from `basic.json` (which stays the older
daemon's). `fixtures/kanban/ci.json`, written by `ci.py` from `basic.json`:
today's column words ("Merged to main") and a card with each CI mark (T-6
CI testing, T-13 CI failed, T-14 CI rebuild waits in In test, T-7 and T-8
Merged to CI in Ready to merge), the daemon's label and help copied from
`crates/core/src/tickets.rs`.

| Intent | Args | Effect |
|---|---|---|
| `kanban_show` | `sandbox?` | Opens it (empty: every sandbox; none: the last scope, else the selected sandbox) |
| `kanban_toggle` | | The top bar's button, ⌃⌥K / Alt+K, the palette's "Show the Kanban": opens it on the sandbox in context (TUI: the focused pane's; else the selected one; else the last scope; else every sandbox), or closes it while it shows |
| `kanban_scope` | `sandbox` | Another sandbox, or every one (`""`) |
| `kanban_open` | `id` | Opens a card's ticket (`""`: closes it) |
| `kanban_column`, `kanban_more`, `kanban_older` | `column`; `column`; | The narrow column; more cards of a column; Merged in full |
| `kanban_key` | `key` (`up`, `down`, `left`, `right`, `enter`, `escape`, `type`), `text?` | The view's keys |
| `kanban_new`, `kanban_edit`, `kanban_form`, `kanban_attach`, `kanban_paste`, `kanban_unattach`, `kanban_save`, `kanban_cancel` | | The form |
| `kanban_delete` | | **Approval**: one Confirm naming the ticket and its files' count and size |
| `kanban_remove_file` | `file` | **Approval**: one Confirm naming the file |
| `kanban_move`, `kanban_priority`, `kanban_validate` | `to`; `priority`; `on` | Planned's order (refused for a card with a thread), the priority, validated on your machine (merged only) |
| `kanban_send` | | **Approval**: one Confirm naming the sandbox's lead, the fixed line in full; refused under a dialog or a menu, a Jarvis proposal, or with no session open; a session asked something gets it once answered |
| `kanban_file` | `file`, `open?` | Saves a file to Downloads and shows it (`open`: opens it, after a Confirm and the core's check) |
| `kanban_link` | `what` | terminal, monitor, commits, worktrees, plan |
| `kanban_refresh` | | Reads again |
| `kanban_issue` | `id?` | Opens an imported card's issue in the browser (http or https only; empty: the open ticket's) |
| `kanban_plan` | `id?` | Moves a Backlog ticket with `can_plan` to Planned (posts `/plan`) |
| `kanban_source_open`, `kanban_source_field`, `kanban_source_cancel` | ; `tracker?`, `repo?`, `label?`, `auto_close?`; | The source dialog |
| `kanban_source_refresh` | | Syncs the sandbox shown now (posts `/source/refresh`) |
| `kanban_source_save` | | **Approval**: one Confirm saying what sbxw imports and writes to the tracker; posts `/source` |
| `kanban_source_remove` | | **Approval**: one Confirm, in red, saying imported Backlog tickets are removed; posts `/source/remove` |

### worktrees

An overlay (`src/feat/worktrees.rs`, `screens/worktrees.luau`): one block
per sandbox (the scope picker `worktrees-sandbox` in the title: All
sandboxes from the rail, one sandbox with `worktrees_show {sandbox}` or a
pick, `worktrees_sandbox {sandbox?}`), one row per worktree (path, thread and its state, ahead,
behind, uncommitted changes, merged, the last commit; Terminal, Commits,
Files). Above a block's rows, the **branch graph** of its unmerged
worktrees, one per repository (`src/feat/worktrees_graph.rs`, the web UI's
`wtLayout` mapping): the base branch as the top lane, each unmerged thread
(a tip with commits of its own) forking from its merge-base and running to
its tip, a dot per commit; merged worktrees and branches with nothing of
their own are not drawn (their rows keep the "merged" chip). Lanes go by
fork, the newest first (no link crosses a lane above it), then by name;
each lane's colour comes from the thread's name (`lane_color`, a stable
palette; the row's branch icon wears it), its label from the thread's
state. Ahead is the lane's dots, behind the base's dots after the fork.
Caps: 24 base commits and 10 per thread from 760 pt, 10 and 4 under it (a
"+N" chip for a thread's older commits, a faint "earlier" edge for the
base's); the same lanes at both widths. The layout is in columns and rows,
deterministic and tested (`the_branch_graph_lays_out_unmerged_lanes_by_fork_with_caps`);
the screen turns it into points and draws it with the `paint` prop
(Rune's `Shape`). A lane's label (`worktrees-lane-<id>`) selects its row
(`worktrees_select`); a dot (`worktrees-dot-<sandbox>-<short>`) opens that
commit in Commits (`commits_show {sandbox, branch, hash}`).

| Intent | Args | Effect |
|---|---|---|
| `worktrees_show` | | Opens the view and reads every sandbox's worktrees |
| `worktrees_refresh` | | Reads again |
| `worktrees_filter` | `text` | By sandbox, thread, branch or path |
| `worktrees_more` | | The next page of rows |
| `worktrees_terminal`, `worktrees_commits`, `worktrees_files` | `id` | The row's thread terminal, its branch in Commits, the explorer at it |
| `worktrees_select` | `id?` | Marks a row (a lane pressed), or none; view state |

### monitor (Sessions)

**Sessions**, the feature `monitor`: every Claude session of every
sandbox in one view, who needs the person first, the selected one's
conversation as the terminal shows it, and a composer (the "Inbox and
detail" concept of `.sbxw-artifacts/desktop/MONITOR.md`; SESSIONS.md there
for the conversation). Files: `src/feat/monitor/` (`mod.rs` intents, the
gate and the history reads, `model.rs` the cards and the view, `conv.rs`
the conversation, `attach.rs`, `tests.rs`), `screens/monitor.luau`,
`fixtures/monitor/` (`basic` with `history` and `history_events`, `empty`,
`idle`, `busy`, `gate`, `folding`), `scripts/demo-monitor.sh`. The names
code, tests and the state file use stay `monitor` (`monitor_*` intents,
`monitor-*` keys, `base_view: "monitor"`).

**Screen.** `monitor` (a base view, `Slot::Base` "Sessions": the pane
area shows it instead of the terminals, see "Base views" below; the top
bar's GUI | TUI switch, `top-view-gui` and `top-view-tui`; shortcut
Ctrl+Option+I on macOS, Alt+I elsewhere, `monitor_toggle`, "Switch between
GUI and TUI"; no rail entry). Left: the count ("8 sessions"), then the
sessions, grouped Needs you (the longest waiting first), Working, Idle,
Ended (the latest first), rows `monitor-row-<key>`, 50 a page
(`monitor-more`), `monitor-next` ("2 need you", N). With Leads only on
(`data.app.leads.on`, the top bar's `top-lead`): no group headings; each
leader's main session, then its threads' sessions one step in
(`indent`), "Team leaders and their threads" for the count, and
`monitor-leads-empty` ("No team leader is running") when none is listed.
Middle (from 760 pt;
under it, the list, and a row pressed opens its session with
`monitor-back`): the header (state, colour square, model, cost; worktree,
lines, time; `monitor-terminal`, `-commits`, `-files`, `-worktrees`), the
conversation (below), then the composer (`monitor-compose`,
the attached files' chips `monitor-attached` with `monitor-file-<id>` and its
cross `monitor-file-remove-<id>`, `monitor-target` "To ■ sandbox ·
session", `monitor-attach` ("Attach", the icon alone under 640 pt),
`monitor-send`). Keys: the
view opens with the keyboard on the shown session's row; Up and Down move
along the rows, Enter shows one, N goes to the next session that needs you;
Escape does not leave it (a base view). With nothing of the view focused, Up and Down move the
selection and Enter gives the keyboard to the list.

**The conversation** (`detail.conv`, `conv.rs`), from the daemon's
history (`GET /api/sessions/:sandbox/:session_id/history`, newest page of
100 first; docs/session-history.md), in the scroll `monitor-conv-<key>`
(`stick_bottom`: it opens at the newest and follows it while scrolled to
the end). Oldest at the top: a day separator (`monitor-day-<n>`, Today,
Yesterday, a date), the person's prompts (`monitor-you-<seq>`, "You"),
what another session or the harness typed in (`injected`,
`monitor-from-<seq>`, "From <sender>", quiet, folded past 360 characters
or 5 lines with `monitor-fold-<seq>`; never "You", never the raw tag),
Claude's text (`monitor-claude-<seq>`, the prose face with pills), a tool
call and its result as one step (`monitor-step-s<seq>`: its family's icon,
the one-line summary in mono, the result's size, a status mark; pressed,
its result's head in mono, `monitor-out-s<seq>`), consecutive steps folded
into a group (`monitor-steps-g<seq>`: "12 steps", the last one's summary,
its status, failed ones counted; pressed, its steps), a compaction, a
notice, a past question or permission as a centred quiet line
(`monitor-sys-<seq>`). Then "Now" (`monitor-now`) while it works, and the
question or permission card, the only rows with controls. A tool call of
`AskUserQuestion` is left out when the log has the question. Only the
newest 60 rows are drawn; `monitor-earlier` ("Show earlier") draws 60 more,
then reads the page before (`before`). At the start of the log:
`monitor-truncated` ("Earlier conversation is not kept") when the daemon
dropped older items, `monitor-continues` when the session continues an
earlier one (its log is not merged in). States: `loading`, `ready`,
`missing` (404: the summary, "sbxw has no conversation for this session"),
`partial` (an old hook, no transcript, catching up: the summary of the last
prompt, now and reply, and `monitor-conv-note` saying why and what to do,
"Run sbxw up <sandbox> to get it"), `error` (what is shown stays, a quiet
`monitor-retry`). Rows are built in Rust once per change of the log or of
what is open, each with a `sig`; the screen memoises a row by session, id
and sig.

**Reads and the subscription.** Only the selected session is read, and
only while Sessions is drawn or the base under an overlay: selecting a
session reads its newest page (which subscribes it for 120 s); the
`history` event (`{sandbox, session_id, seq}`, a nudge without text) for
it reads the newest page again (merged by `seq`: a provisional reply is
replaced, a reset restarts the log); a `session` event for it renews the
subscription when the last read is over 90 s old. Another session
selected, the view left (TUI, its terminal link) or an event while it is
not drawn: `DELETE .../history/subscription` of the one followed (quiet).
On quit the subscription expires by itself. A nudge for any other session
reads nothing. The selected log and three recently viewed ones stay in
memory (`conv::LRU`); nothing is logged or written to disk.

**Data.** `data.monitor`, built in Rust: `now`, `rows` (`{key, rev,
sandbox, name, thread, group, tone, line, mono, ts, ask}`), `count` (the
paging block), `counts` (`you`, `working`, `idle`, `ended`, `total`),
`sel`, `leads` (`{on, leaders, running}`), `detail` (the selected session: `strip`, `last_input`, `now`,
`reply`, `conv` (above: `state`, `rows`, `hidden`, `total`, `earlier`,
`older_loading`, `truncated`, `continues`, `partial`, `error`, `rev`),
`question {answerable, steps: [{text, options: [{label, desc}]}]}`,
`permission`, `compose {can, why, target}`, `sent`, `pane`, `files`
(`[{id, name, size, state: ok|look|refused|uploaded, why, sending,
progress}]`, the name already cut by the Rust), `sending`, `attach_max`). Started from
`data.sessions` when the view opens (`monitor_open`), then kept from each
`session` event, one card per place (a sandbox's thread, or its main
checkout, as the sidebar folds them: a new session there takes the card and
its key over, a late report or the end of an older session changes nothing,
two live sessions in one main checkout make two cards named `main · <end of
session id>`; the daemon's `continues` and `ended` are followed when it
serves them). A `last_input` the harness typed (a cross-session message, a
task notification) is `detail.from {name, text}`, never `last_input`: a card's `rev` moves only when its
report changed, the screen memoises a row by key and rev, and the view is
handed over while it shows or is the base under an overlay (`data.app.base`;
a `Set` renders only drawn screens). Also read: `data.relay_ui.picks` and
`sending` (the same picks as the Requests panel), `data.app.groups`,
`data.app.covered`.

| Intent | Arguments | Risk | What the core does |
|---|---|---|---|
| `monitor_open` | none | direct | Reads the list again and makes Sessions the base view (the sidebar's sandbox's most urgent session selected, its conversation read); an overlay over it closes. The top bar's GUI |
| `monitor_toggle` | none | direct | GUI (Sessions) or TUI (the terminals), whichever is not the base now; leaving deletes the history subscription |
| `monitor_refresh` | none | direct | Draws Sessions again (the core runs it when Leads only changes) |
| `monitor_expand` | `key` (the session shown), `id` | direct | Unfolds or folds a row of its conversation: a group of steps, a step's output, a long message |
| `monitor_earlier` | `key` | direct | 60 more rows, then the page before (`GET ...history?before=`) |
| `monitor_retry` | `key` | direct | Reads its newest page again after a failed read |
| `monitor_select`, `monitor_next`, `monitor_more` | `key` / none / none | direct | View state: a session shown, the next that needs you, the next page |
| `monitor_key` | `key` (`up`, `down`, `enter`, `type`), `text` | direct | The view's keys when none of its rows has the keyboard; N (`type`) from anywhere in it |
| `monitor_terminal`, `monitor_commits`, `monitor_files`, `monitor_worktrees` | `key` | direct | `thread_open` or `open_terminal` (spare pane; the pane that shows it, when one does), `commits_show` on its branch, `files_open` at its worktree, `worktrees_show` scoped to its sandbox (a thread: filtered to it) |
| `monitor_pick` | `key`, `step`, `option` | direct | The relay's `agent_pick`, only for a session the daemon lists as waiting and answerable |
| `monitor_attach` | `key` (the session shown) | direct | The core's file dialog (`Action::PickFiles`, from a press only); the file comes back through `on_files` and waits as a chip of that session |
| `monitor_commands` | `key` (the session shown) | direct | The composer's completion list: `GET /api/sandboxes/:name/commands` for that session's sandbox, once (the screen asks when the draft first starts with `/` and a partial name). One sandbox's list is held; the shown session in another sandbox reads that one's. Never on a timer, never from Luau. The answer's names are checked (`^[a-z0-9][a-z0-9:_-]{0,63}$`) and its descriptions cleaned (controls and invisible characters dropped, one line, 100 characters) before `detail.commands` (`{state = "loading" \| "ready" \| "none", items = { {name, desc} }}`); a 404 (a daemon without the route) or any failure is `none`: no list, nothing said |
| `monitor_attach_remove` | `key`, `id` | direct | Takes that file off the session's chips; the core forgets its path |
| `monitor_send` | `key`, `text?` | approval | Masked (`/api/voice/anonymise`), then ONE core dialog: the words in full, each attached file (local path, size, "look" reasons), the total, the session; a "look" file needs its box ticked. Send there: without files, `/api/chat/push {text, name, thread}`; with files, `core:uploads` (then each pasted picture's `paste-image`), then the push of the words followed by the daemon's paths. A failure pushes nothing |

**The composer's keys.** Return sends (the Send press: the same intent and
the same checks in Rust); Shift+Return starts a new line; Cmd+Return
(Ctrl+Return) sends too. A closed composer, or no words and no file, sends
nothing. While the draft is `/` and a partial name (no space yet), the
skills and commands of the session's sandbox whose name starts with it,
then those that hold it, show above the box (8 rows at a time, the window
following the selection): Up and Down move, Tab or Return completes the
name followed by a space (and sends nothing), Escape closes the list for
that draft and keeps it, a click picks one; the keyboard stays in the box.
No match shows nothing.

**The rules.** Only a press (or Return in the composer) sends. The
target is always drawn next to Send. The composer is disabled while the
session waits on a question or a permission (typed words would land in its
menu). `monitor_pick` and `monitor_send` refuse while `data.app.covered` is
not false (which includes another view, such as Files, drawn over
Sessions) or a Jarvis proposal waits (`data.jarvis.pending`). Agent text is
drawn as text and Markdown, never as a control; an option sends its index.
Everything from the history is data too: a step row's press only folds
it, by its own id.
A permission prompt is answered in its terminal: sbxw does not see the
command.

**Attach.** A local path only comes from the core: the file dialog, files
dropped on Sessions (any part of it, while it is drawn), a picture pasted
while it shows (`on_files`); never from Luau or agent text. Files belong to
the session they were attached to; another session shows its own, a send
takes only its own. Attaching refuses in Rust in the composer's states
(`data.app.covered`, a Jarvis proposal waiting, a session waiting on a
menu, ended, or a sandbox not running). The guardrails are
`feat::files::drops`' (a secret-looking name, a `.ssh`/`.aws`/`.gnupg`
folder, more than 25 MB in all: "look", the dialog's box), an empty file or
one over 128 MB is refused (Send off and refused). "Do not ask again for
files I drop" never applies: words to an agent always get the dialog (the
core grants it to the files feature's pane drops only). A file uploaded by a
send that stopped keeps its daemon path and goes with the next Send (the core
forgets a local path once uploaded). Names: cut in a chip, control and
direction characters as `?`; whole in the dialog, those characters written
out; the log gets counts and `what` lines without names.

### Overlays

`Slot::Overlay { title, icon }`: a full view over the pane area (the
pager and the panes); the rail, the sidebar, the top and status bars stay.
`open_panel` opens it, one at a time (another overlay replaces it);
`close_panel`, Escape (after any menu, dialog or modal), its × or its rail
entry pressed again close it. The core draws its header (icon, title,
"Esc closes", ×, key `overlay-close`); the element is `overlay-<screen>`.
The terminals keep running underneath and are not resized (it is drawn
absolute over them; while an overlay or a base view is drawn, `App::panes_held`,
a terminal's room has no layout handler, `terminal::pane_held`). When the
panes show again, each shown terminal is fitted to its rect then: one resize
for each one whose room changed meanwhile (Leads only, pages, Tabs or Grid,
the sidebar, the window), none for the others; one sized by another window
whose room did not change stays scaled, unsent. None takes a key, a paste, a press, a drag or the wheel
while it shows (`App::terminal_may_take_keys` and the panes' handlers);
files dropped on it go to its feature's `on_files`. Up, Down and Enter, when
none of its controls has the keyboard, run its feature's `<screen>_key`
intent if it declares one. A popup of its own (a menu, a picker drawn by
the screen) gets Escape first: while its feature's `<screen>_ui.popup` is
true and its `<screen>_key` takes `escape`, Escape runs `<screen>_key {key:
"escape"}` and the view stays (the core's dialog, a modal, Jarvis's
proposal and the palette still come before it; a base view too). Opened while a base view is current, it goes
over that view, and closing it (Escape, its ×, its rail entry) comes back
to it.

### Base views

`Slot::Base { title, icon, open }`: a view of the pane area equal to the
terminals (Sessions is the one). One base is current: TUI, the terminals
(the pages, in Tabs or Grid), or GUI, it. The switch is the top bar's
(`topbar.luau`): `top-view-gui` runs the base's `open` (also from under an
overlay over it), `top-view-tui` its `<name>_toggle` while it is the base;
icons alone under the compact width, the words and the shortcut in the
tooltips. The pager keeps Tabs | Grid (`pager-view-tabs`, `-grid`), the
terminals' layouts, and is not drawn under the base; the base's header has
its title only (no switch, no ×). The base never changes the pages' layout
nor any terminal's size (drawn absolute, as an overlay). `open` is the
feature's intent (it reads its data, then `open_panel`); `close_panel` of
it goes back to the terminals; Escape never leaves it. The choice is kept
in the app's state file (`base_view`) and comes back at start, its `open`
run once the first lists and `data.sessions` are in.

Leads only, beside the switch (`top-lead`, "Leads only: on" purple on its
tint, "Leads only: off" quiet, the `leader` mark; disabled while no session
leads), is the `team_leads` intent, a view filter only (it never calls the
daemon; "lead mode" is a session's own state, the pane bar's name pill):
one state for both views (TUI: only the leaders' panes, the layout before
back when it goes off; GUI: only their sessions). A leader is a session
whose lead state is `leading` (`model::leading`). Kept in the state file
under its old key (`lead_mode`, so older files still load), restored at
start when a session still leads. `data.app.leads`: `on`, `can`, `leaders`.

The Kanban, after Leads only (`top-kanban`, the same pill: height, radius,
8 pt gap; the `kanban` mark; "Kanban", its icon alone in a compact bar with
the words in its tooltip), is `kanban_toggle` (also ⌃⌥K / Alt+K and the
palette's "Show the Kanban"). Quiet at rest; lit (purple on its tint) only
while the Kanban is the screen drawn (`data.app.overlay.screen ==
"kanban"`), not while the Dashboard's conversation shows. A press opens the
Kanban overlay on the sandbox in context: in TUI the focused pane's
sandbox, else the selected sandbox (in GUI, the selected one), else the
last sandbox the Kanban showed, else All sandboxes. A press while it shows
closes it as Escape does: in TUI the focused terminal gets the keyboard
back, in GUI Sessions shows again. The width is `bar::modes_w`'s.

The gate: while it is the base, `App::panes_covered()` is true, so no
terminal takes a key, a paste, a press, a drop or the focus. `data.app.base`
is its screen name (null: the terminals). `data.app.covered` is computed in
one place (`App::covered`): the core's dialog, a modal, Jarvis's proposal,
the palette, or an overlay drawn over the base. An overlay drawn over it
reads `data.app.covered_top` instead (the same without its own overlay). Drops and pasted pictures on
it go to its feature's `on_files`, as an overlay's.

### mac

The virtual Mac (the web UI's Setup, Mac: `mac.js`;
`assets/terminal-mac/README.md`). Files: `src/feat/mac/` (`mod.rs`,
`tests.rs`), `screens/mac.luau`, `fixtures/mac/`, `scripts/demo-mac.sh`.
Report: `.sbxw-artifacts/desktop/BROWSER-MAC.md` in the main checkout.

**Screen.** `mac` (panel "Virtual Mac", 440 pt): not available on a host
that is not an Apple silicon Mac; else Install, or the VM's card (state,
guest ready, who holds the lease, release, ports) with Start / Resume /
Create and start, Open its window, Pause, Take it back, Shut down, Force off
(while stopping), Refresh (keys `mac-<action>`); the busy banner, the last
error; the requests waiting on the person, each with sbxw's sentence and, in
a block of its own labelled as the sandbox's words, its reason, and a button
to the Requests panel (`relay_open`); the guest's one-time setup while it is
not ready; the idle timers.

**Extras.** `extra-topbar-mac` ("Mac", on a host that can run it): green
running, amber when requests wait (with their count) or the guest setup is
unfinished, red after a failure, blue while busy.

**Data.** `mac`: `GET /api/mac`, the whole answer `{ok, mac}`, read again on
the `mac` event and every 10 s (the VM's state and the guest's readiness
change without an event).

**Intents.** `mac_open`, `mac_refresh`, `mac_up` (direct; creating a missing
VM goes through the core's dialog first: macOS is downloaded, several GB),
`mac_pause`, `mac_window` (terminal-mac shows the VM's own window: the live
view): direct. `mac_install`, `mac_stop`, `mac_force_off`, `mac_take_back`:
**Approval**, the core's dialog built from `data.mac`. The in-app live screen
is not ported: the daemon has no person-facing route for the guest's screen
and input (BROWSER-MAC.md says which it would need).
