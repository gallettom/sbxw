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
| `groups` | Each sandbox's group colour (`#rrggbb`), by name; absent for a sandbox in no group. The web UI's hue first (chats share one, a workspace folder's from its path), then made distinct: a group within 40° of a hue already taken (chats first, then workspaces by path) moves to the nearest free hue, so two groups never share a background; it can then differ from the web UI's (`theme::group_colors`) |
| `sidebar` | `w` (its width in points, which the person drags, 200 to 420) and `shown` (Cmd/Ctrl+B hides it) |
| `hover_zone` | The [hover zone](#hover-zones) the pointer rests in (this screen's), or nil |
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
| `mono` | bool | false. The terminal's font. The whole UI is already monospace, as the web UI is |
| `wrap` | bool | false: one line |
| `truncate` | bool | false. One line cut with an ellipsis when it does not fit (give it `grow = 1` in a row) |

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
| `hover_group` | bool | Its `reveal` children show while it is hovered or the keyboard focus is in it |
| `hover_zone` | string | `row`/`column`: one hover zone with every element of this screen that names the same zone, even apart in the tree (a row and the block under it). The core tells the screen where the pointer rests in `data.app.hover_zone` ([Hover zones](#hover-zones)) |
| `group` | `list` | Its pressable children (with `on`) are one list: Up and Down move the keyboard between them |
| `selected` | bool | With `on`: the selected item (screen readers say so) |
| `clip` | bool | What overflows it is cut at its edges (a line of badges that may not fit) |
| `shortcuts` | list of `{ what, key, ... }` | Lines under its `help` in the tooltip, each what a key does with its keycaps: `shortcuts = { { "Open its terminal", "Enter" }, { "Start or stop", "Space" } }` |
| `folds` | bool | `column`: its children fold open and shut smoothly. A child that grows or shrinks (a block that expands) glides to its new height over about 0.18 s, ease-out, and what it gains or loses fades; only in answer to the person's press or key, never for data that arrives; off under the system's reduced motion |

```lua
row{ gap = 8, w = "full", dot{ tone = "green" }, text{ "running" }, spacer{}, text{ "2" } }
```

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

```lua
input{ id = "filter", value = state.filter, placeholder = "Filter", on = "filter" }
-- actions.filter = function(state, _, text) state.filter = text end
```

### `textarea`

A text field of several lines: Return is a new line, Cmd+Return (Ctrl+Return
off macOS) submits. The core holds the text, as for `input`.

| Prop | Value | Effect |
|---|---|---|
| `id` | string | Required. Names the field (also its key) |
| `value` | string | The text, applied as `input`'s |
| `placeholder` | string | Shown while empty |
| `rows` | number | Visible lines, 1 to 40 (default 4); more text scrolls |
| `on` | action name | Called on every edit, `value` = the text |
| `on_submit` | action name | Called on Cmd/Ctrl+Return, `value` = the text |
| `arg` | any value | Passed to both |
| `disabled` | bool | Read only |

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

```lua
select{ id = "layout", value = "2c", on = "layout",
  items = { { id = "1", label = "1 terminal", icon = "single" }, { id = "2c", label = "2 side by side", icon = "columns" } } }
```

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

```lua
markdown{ "Run **`sbxw up`** first, then see [the docs](https://example.org/sbxw).", on = "link" }
-- actions.link = function(_, _, url) return intent("open_link", { url = url }) end
```

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
buttons run, through the same checks), Cmd/Ctrl+B the sidebar (the left
rail stays), Cmd/Ctrl+1 to 9 a pane of the current page, Ctrl+Tab and
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

The global `theme` (read only) is the web UI's design tokens
(`assets/app.css`). Any colour prop takes these names; `theme.color[name]`
gives the `#rrggbb`.

| Table | Names |
|---|---|
| `theme.color` | `bg`, `surface`, `surface-alt`, `elevated`, `border`, `dim`, `text`, `text-bright`, `text-muted`, `text-soft`, `text-dim`, `blue`, `blue-tint`, `green`, `green-strong`, `green-deep`, `green-tint`, `red`, `red-border`, `red-tint`, `amber`, `amber-strong`, `amber-tint`, `purple`, `purple-strong`, `purple-alt`, `purple-tint`, `group-1` to `group-4`, `hover-subtle`, `active-bg`, `white`, `floor`, `pane-bar`, `focus`, `control-border`, `primary-hover`, `red-text` (red words on a group's tint) |
| `theme.radius` | `xs` 6, `sm` 6, `md` 10, `lg` 12, `pill` 999 |
| `theme.text` | `caption` 12, `body` 13, `title` 14, `heading` 16; older names `xxs`, `xs`, `sm` 12, `md` 13, `lg` 14, `xl` 16 |
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

**Vendor colours** (a separate axis, not colour by meaning). The sidebar's
model words (a row's line 2, a session line's model) take the colour of
whoever makes the model, so "opus 5.5" and "gpt-5-codex" are told apart at a
glance. Text colour only, regular weight, never a fill or a badge, so the
state word before it stays the strongest thing on the line.

| Token | Value | Who | Matched by (id, display name, any case; else the agent) |
|---|---|---|---|
| `vendor-anthropic` | `#e2875a` terracotta | Anthropic | claude, anthropic, opus, sonnet, haiku, fable, mythos |
| `vendor-openai` | `#e6edf3` (as `text-bright`) | OpenAI | openai, chatgpt, gpt, codex, the o-series (o3, o4-mini) |
| `vendor-google` | `#8ab4f8` | Google | gemini, google |

Anything else stays `text-muted`. One function decides (`vendor_of` in
sandboxes.luau); the tooltip keeps the full id and adds "by <vendor>". The
terracotta's hue sits between `red-text` ("unreachable") and `amber`
("wants you"); Gemini's blue is lighter than the `blue` "working" word.
Both rules and the contrast on every ground are tested (`screen::tests`).

**The rules the built-in screens follow** (write yours the same way):

- **Spacing** on the 4 pt grid: 4, 8, 12, 16, 24 (2 only between stacked
  lines of one thing). A test fails a built-in screen that writes another
  number for `gap`, `pad`, `px` or `py`.
- **Type**: 12 pt for second lines, labels and badges (`caption`), 13 for
  body (`body`), 14 for row and panel titles (`title`), 16 for dialog titles
  (`heading`). Nothing under 12. Weights 400, 500, 600. **No capitals** for
  labels: sentence case; weight and colour make the hierarchy.
- **Colour**: every text and surface pair meets WCAG AA (4.5:1; 3:1 for
  icons, marks and focus rings): `theme::tests::contrast_of_every_pair_used`
  lists them. `text-dim` is for placeholders and hints only, never for
  something a person needs. A state is never colour alone: say it in a word
  too.
- **Radii**: 6 for controls and rows (`xs`, `sm`), 10 for cards, tiles and
  menus (`md`), 12 for dialogs (`lg`).
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
`file`, `flag`, `ports`, `copy`, `trash`, `more`, `columns`, `rows`, `grid`,
`single`, `chat`, `link`, `mic`, `speaker`, `send`, `sparkle`, `cpu`,
`coin`, `help`, `branch`.

## Where screens live

Built-in screens are compiled into the app (`desktop/screens/<name>.luau`).
A file with the same name in the **override folder** replaces it and reloads
on every save (within about 150 ms); deleting the file brings the built-in
back. The folder is `~/.sbxw/desktop/screens/`, or `$SBXW_DESKTOP_SCREENS`.

Screens: `topbar` (the bar at the top, right of macOS's window buttons),
`sandboxes` (the side panel) and `jarvis` (the panel docked on the right,
shown while `data.jarvis.open`). The terminals, their pane bars, the
pager above them (pages of terminals: `src/shell/pager.rs`), the left rail
(Grid and Tabs, Requests, Network, Jarvis, Setup: `src/shell/rail.rs`), the
panel's resize handle, the status bar at the window's foot (bring-ups,
total CPU, the sessions' cost, Help: `src/shell/statusbar.rs`) and every
approval dialog are drawn by the core.

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
| `screens` | `ScreenDecl { name, src: include_str!(..), slot }`; `Slot::Modal { width }` (a dialog over the window) or `Slot::Panel { title, icon, width }` (docked on the right, beside Jarvis; several open show as tabs) |
| `intents` | Rows of the closed list (`intent::Spec`: name, typed arguments, risk, doc). Exported to `intents.json` |
| `act` | `fn(&Ctx, &Asked) -> Result<Vec<Action>, String>`: runs one of its intents, already checked |
| `on_answer` | Optional: gets every answer of its calls, status and JSON body, non-2xx included; then the core says nothing itself |
| `extras` | Buttons and badges in others' slots: `Extra { place, id, label, icon, tone, help, count, intent }`; `place`: `Topbar`, `Sandbox(name)` (on its row), `SandboxRow(name)` (a sub-row under it: one per session), `Pane(sandbox)` (every pane bar of that sandbox), `Session { sandbox, thread }` (the pane bars of that session only; `thread: None` is the main one) |
| `standin` | Its routes for the stand-in daemon and fixture daemons: `fn(&mut Standin { data, publish }, method, path, body) -> Option<(status, body)>`; `publish_after(ms, event, data)` for events sent before the answer |
| `core_only` | Body fields only the core sets, after the person's Confirm (`Confirm::grant`); any other call carrying one is refused |
| `shortcuts` | `&[Shortcut { key, intent, args, help }]`: global keys, taken by the window before any pane, as the web UI's: Alt+<key> off macOS, Ctrl+Option+<key> on macOS (Option alone types characters). `key` is the letter by its place on the keyboard; `args` a JSON object. Create declares N (New) and M (chat) |
| `on_files` | `fn(&Ctx, Incoming) -> Vec<Action>`: what the person handed it, `Incoming::Files(paths)` or `Incoming::Png(bytes)`. Files dropped on its shown panel or dialog; a picture pasted (⌘V) while its panel is shown and no terminal has the keyboard (a paste into a terminal stays the terminal's); the answers of `PickFiles` and `ReadClipboard`. The only ways a feature gets a path to read: never from a screen or a sandbox. Decode pictures with `crate::picture::prepare` (limits, no animation, scaling, PNG or JPEG out), then apply the feature's own policy |
| `surfaces`, `on_surface` | Picture surfaces it owns, by id (`src/surface.rs`), placed by a screen with `surface{ id }`. Frames from its Rust on any thread: `surface::set_encoded(id, bytes)` (PNG, JPEG, WebP decoded with `surface::frame_limits()`) or `surface::set_rgba(id, w, h, rgba)`; Rune redraws without running Luau. `on_surface: fn(&SurfaceEvent) -> Vec<Action>` on the UI thread: `Move`, `Press`, `Release`, `Wheel` in picture pixels, `Key` (DOM key name, physical code, text), `Text` (input method), `Paste` (text; the core shows a notice with its size), `Focus`, `F6`, `Resized` (logical points, debounced). The app keeps its own keys (see the `surface` element) |
| `on_socket`, `standin_ws` | `Action::OpenSocket { id, path, opts }` opens a `/ws/...` route of the daemon with the app's auth (`src/socket.rs`); `on_socket: fn(&str, SocketEvent) -> Vec<Action>` runs on the socket's own thread (`Open`, `Text`, `Binary`, `Reconnecting`, `Closed`), its actions on the UI thread at the next poll; `socket::send(id, text)` from anywhere. `Opts { reconnect, droppable, max_in, max_out, one_line }`: reconnect is off by default; `droppable` messages (frames) keep only the newest when the handler is behind. `standin_ws: fn(target, &mut dyn WsPeer) -> bool` serves the route in the stand-in |
| `on_bytes` | `fn(&Ctx, &Call, Result<(status, bytes, content type), String>)`: the answer of a `Call::get(path).raw(max)` (at most `max + 1` bytes). Fixture and stand-in routes answer raw with `{"$raw": text, "$type": ...}` or `{"$raw_hex": ...}` |
| `on_pane_drop` | `fn(&Ctx, &PaneDrop { paths, sandbox, thread })`: files dropped on a terminal pane (the first feature declaring it); the paths are handed in, so an upload may read them |
| `on_dismiss` | `fn(&Ctx, tag)`: one of its core dialogs closed without its call going out (Cancel, close, Escape, a press outside, replaced, a failed `Prepare` closed); `tag` is the dialog's call tag |

Actions (`feat::Action`): `Call(Call)` (a daemon request off the UI thread:
`Call::post(path, body, what)` / `Call::get`, `.then(&[keys])`,
`.store(key)`, `.timeout(secs)`, `.tag(t)`, `.quiet()`), `Confirm`,
`Prepare`, `Refresh(key)`, `Fetch { key, path }`, `Set { key, value }`,
`OpenModal { screen, arg }`, `CloseModal`, `OpenPanel(screen)`,
`ClosePanel(screen)`, `Notice { text, error, then }` (`then`: an intent a
click on the notice runs), `OpenLink(url)`, `Intent { name, args }` (checked
exactly as from a screen), `Remember { key, value }` (kept across runs in the
app's state file, read back as `data.app.kept.<feature>.<key>`; null forgets).
`OpenSocket`, `CloseSocket(id)`, `Focus(element key)`. A `Call` of method
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

### create

New sandbox (the web UI's `create.js`, and the bring-up cards of
`sandboxes.js`). The form is the core's: the screens show it and change it
one intent at a time, so what is sent and what the core's dialogs list is
never text from a screen.

**Screens.** `create` (modal, 640 pt): the chooser, the workspace form and
the chat form, by `data.create.dialog`. `create-jobs` (panel "New
sandboxes"): every bring-up in progress, failed or waiting for an answer.

**Extras.** In the top bar: `extra-topbar-create` ("New", opens the
chooser), and `extra-topbar-create-jobs` ("Creating", with a count) while a
bring-up runs.

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
the GitHub token, kits lines, kept skills, MCP servers, remote hosts, the
voice models. A modal, opened by the gear in the top bar
(`open_modal { screen = "setup", arg = <tab> }`). A sandbox with a kit
waiting gets an amber ✓ on its row (`open_terminal` with `mode = "kit-add"`).

Files: `src/feat/setup/` (rules, stand-in, tests), `screens/setup.luau`,
`fixtures/setup/`. Data, each key one route's answer as the daemon gives
it (why a read failed: `data.app.errors[key]`, shown as a red line in its
tab): `data.secrets` (`GET /api/secrets`:
names, scopes and kinds, never a value), `data.github_token` (`{set, gh}`),
`data.kits` (`GET /api/kits`), `data.my_skills`, `data.mcp`,
`data.remote_hosts`, `data.voice` (`GET /api/voice`, then each `voice`
event). Set by its intents: `data.skills_for` and `data.sandbox_skills`
(`skills_look`), `data.mcp_inspect_for` and `data.mcp_inspect`
(`mcp_inspect`), `data.provision` (`{host, command}`, `host_provision`).

**No intent carries a secret value.** The value of a token, a custom secret
or an MCP header placeholder is typed in the core's dialog, sent once and
forgotten; a screen only sees names and whether a value is set.

| Intent | Arguments | Risk | What the core does |
|---|---|---|---|
| `setup_load` | `section` (`secrets`, `kits`, `skills`, `mcp`, `hosts`, `voice`) | direct | Reads that tab's data again |
| `github_token_set` | none | approval | Asks for the token in its dialog, stores it as sbx's `github` secret |
| `github_token_gh` | none | approval | Makes sbx use this machine's `gh auth token` |
| `github_token_rm` | none | approval | Removes the `github` secret |
| `secret_custom_set` | `hosts` (comma or space separated), `env` | approval | Asks for the value in its dialog; the proxy puts it in requests to those hosts |
| `secret_rm` | `name` | approval | Removes a global secret |
| `kits_save` | `path`, `kits` (one reference per line) or `defaults = true` | approval | Rewrites that file's `kits` line (or removes it); the dialog lists what is added and removed and asks for an acknowledgement for kits that reach outside the sandbox |
| `skills_look` | `sandbox` | direct | Lists that sandbox's skills (`data.sandbox_skills`) |
| `skill_keep` | `sandbox`, `name` | approval | Keeps a skill found by `skills_look` for every new sandbox |
| `skill_forget` | `name` | approval | Stops keeping it |
| `mcp_load` | `server`, `sandbox` | direct | Loads a server into a running sandbox |
| `mcp_unload` | `server`, `sandbox` | approval | Unloads it |
| `mcp_auth` | `server` | direct | `sbx mcp auth`, in this machine's browser |
| `mcp_inspect`, `mcp_inspect_close` | `server` / none | direct | Shows or hides `sbx mcp inspect` |
| `mcp_rm` | `server` | approval | Unregisters a server |
| `mcp_save` | `name`, `kind` (`remote`, `stdio`), `url`, `headers` (`;` or new lines), `command`, `args`, `dir`, `replace` | approval | Registers (or replaces) a server; one secret field per `${KEY}`; a `stdio` one shows its host command and needs "I have read this command" |
| `host_save` | `name`, `ssh` or `host`/`user`/`port`/`key`, `agent_port`, `sbx_path`, `trust_secrets`, `editing` | approval | For a new address, reads the host key first and shows its fingerprints; adds or edits the host |
| `host_rm` | `name` | approval | Unregisters a host (with records there: an acknowledgement, then forced) |
| `host_test` | `name` | direct | `sbx version` over the link |
| `host_provision`, `host_provision_close` | `name` / none | direct | Shows or hides the command that installs sbx there |
| `hosts_reload` | none | direct | Reads hosts.toml again |
| `voice_install` | none | approval | Downloads the voice models; progress in `data.voice.job` |
| `voice_cancel` | none | direct | Stops the download |
| `voice_rm` | none | approval | Deletes the models |
| `voice_warm` | none | direct | Loads the models now |

### threads

Threads, plans and the lead (the web UI's thread parts of `panes.js`,
`plans.js`, `board.js`, `worktrees.js`). Files: `src/feat/threads/`,
`screens/threads.luau`, `plans.luau`, `threads-new.luau`,
`fixtures/threads/`. Report: `.sbxw-artifacts/desktop/THREADS.md` in the main checkout.

**Screens.** `threads` (panel "Threads"): one sandbox's sessions, Team
mode, its plans, and the Worktrees and Board tabs. `plans` (modal, `arg` =
a plan id): the review while it waits, its threads' state once approved,
the merge once proposed. `threads-new` (modal, `arg` = the sandbox): New
thread, or a session on the main checkout.

**Data** (each a route's answer; a per-sandbox read that failed is in
`data.app.errors[key][sandbox]`):

| Key | What |
|---|---|
| `sessions` | `GET /api/sessions`: every hook report (`sandbox`, `thread`, `state`: `working`, `idle`, `attention`, `exited`, ...) |
| `threads` | By sandbox, `GET /api/sandboxes/:name/threads`: `{ok, threads: [{name, open, repo, model, worktree: same\|own\|team}], open, max}` |
| `plans` | `GET /api/plans`: every plan (`id`, `sandbox`, `summary`, `threads` with `state` and `summary`, `state`, `variant`, `justification`, `closed`, `merge`: `{order, note, state, base, results, distillate}`) |
| `leads`, `models` | `GET /api/leads`, `GET /api/models` (`{ok, default, models: [{id, label}]}`) |
| `board` | By sandbox, `GET /api/sandboxes/:name/board`: `{plan, claims, entries}` |
| `threads_view` | Set by the core: by sandbox, `{sessions: [{thread, kind, running, state, word, tone, label, summary, repo, model}], open, max, leading, pending, merge}` |
| `threads_ui` | Set by the core: `sandbox`, `tab`, `new` (`{sandbox, kind, suggest}`), `drafts` (by plan id: `{threads, variant}`), `plan_error`, `merge_close`, `merge_note` (by plan id), `repos_for`, `worktrees_for`, `remote` |
| `thread_repos`, `worktrees` | Read on demand: `GET .../repos` (for `threads_ui.repos_for`), `GET .../worktrees` (for `worktrees_for`) |

**Extras.** Rows under a sandbox (`extra-row-<sandbox>-<main|thread>`)
once it has a second session; `extra-sandbox-<name>-threads` (opens the
panel; amber when a session waits); on each session's pane bar
(`pane-<n>-x-<id>`): `model`, `state`, `interrupt`, `remote`, `close`; on
the main session's: `lead`, `plan`, `merge`, `threads`; in the top bar
`extra-topbar-plan` while a plan waits.

**Intents** (indexes from 1):

| Intent | Arguments | Risk | What the core does |
|---|---|---|---|
| `threads_show` | `sandbox`, `tab` (`sessions`, `worktrees`, `board`) | direct | Opens the panel on that sandbox and tab |
| `thread_new` | `sandbox`, `kind` (`thread`, `session`) | direct | Opens New thread and reads the repositories |
| `thread_open` | `sandbox`, `name`, `repo` (`=` then its path; `=` is the workspace), `model`, `checkout`, `pane` | direct | Opens that session (spare pane); repo, model and checkout only for a new one. Refused: a bad name, one of the other kind, no place free under `max_agent_sessions` (said with the names) |
| `thread_bash` | `sandbox`, `thread` | direct | A shell in its worktree |
| `session_interrupt` | `sandbox`, `thread` | direct | Escape into that Claude session (`POST /api/input`) |
| `remote_control` | `sandbox`, `thread` | direct | Reads the login, then turns Remote Control on, or says why not |
| `remote_login` | `sandbox`, `thread` | approval | Stops injecting the setup token, types /login |
| `worktrees_load` | `sandbox` | direct | Reads `data.worktrees` |
| `lead_start`, `lead_end` | `sandbox` | direct | Team mode on; off after the core's dialog says what it leaves |
| `plan_review` | `id` | direct | Opens the plan dialog (and its draft) |
| `plans_reload` | none | direct | Reads plans and boards again |
| `plan_edit` | `id`, `index`, `field` (`name`, `goal`, `scope`, `repo`, `model`), `value` (none: empty, default repo, default model) | direct | Edits the draft |
| `plan_remove`, `plan_add`, `plan_reset` | `id` (`index`) | direct | Removes, adds, resets |
| `plan_variant` | `id`, `variant` (`split`, `shared`) | direct | The draft's variant |
| `plan_refuse` | `id`, `note` | direct | Refuses; the lead reads the note |
| `merge_close_set` | `id`, `on` | direct | Close the threads once merged (default on) |
| `plan_approve` | `id` | approval | The core reads the sessions, its dialog lists the draft, the changes and the sessions it starts; over the ceiling it says how many are open, which, how many are free, how to free one |
| `plan_merge` | `id` | approval | The core's dialog: order, note, base, distillate, whether threads close; refused while a shared plan's files are claimed |
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
waiting on a choice. It opens by itself when a request arrives.

**Data.** `data.relay` (`GET /api/relay`, read again on each `relay` event:
`{requests}`, each `{id, from, from_thread?, kind: question|screenshot|ask|mac,
question, to?, answer?, shot_count, state: pending|routed|answered|approved|denied,
note?, created_ms}`); `data.sessions` (threads' key) for `AskUserQuestion`;
`data.relay_ui`, set by the core: `current`, `closed`, `rounds` (by id:
`{intro, questions: [{id, title, body, rec}]}`), `forms` (by id: answers),
`own`, `review`, `caption` (by id: what was typed), `shots` (by id: `{n,
source, w, h, kb, label, image}`, `image` a key for `image{}`; never the
bytes), `max_shots`, `picks` and `sending` (by session key
`sandbox::thread`), `local` (rounds found in a followed session, shown as
requests with `local = true`), `follow` (`{target, follow, choosing}`),
`voice` (`{id, index}`: the round answered by voice).

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

**Screens.** `files` (panel "Project", 460 pt): tabs Artifacts
(`.sbxw-artifacts`, newest first, each folder one folded row with its file
count until pressed), Files (a folder of the sandbox, the
places, Hidden files, the typed path) and Recent (written lately anywhere,
by age, Show in folder); an upload card; a filter; a preview under the list
(text, Markdown through `markdown`, a picture through `image`, HTML as its
source) with Download, Show in folder, Open, Save as, Copy path, Delete.
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
`files-show-<id>`, tabs `files-tab-<tab>`, actions `files-download`,
`files-reveal`, `files-open-with`, `files-save-as`, `files-copy-path`,
`files-delete`, the preview's body `files-preview-<state>` (`loading`, `text` (HTML too), `markdown`, `image`, `note`, `error`), uploads `files-drop` (the drop target), `files-upload-send-<n>`
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
| `files_ui` | Set by the core: `sandbox`, `tab`, `rows` (`id`, `name`, `kind` (`folder` for a folded artifact folder, with `count` and `open`), `icon`, `dir`, `depth`, `size`, `when`, `fresh`, `group`, `can_delete`, `selected`), `count` (`total`, `shown`, `more`, `label`), `places`, `path`, `can_up`, `can_back`, `offline`, `selected`, `preview` (`state`: `loading`, `text`, `markdown`, `html`, `image` with `key`, `note`, `error`), `staged` (`index`, `name`, `size`, `to`, `problem`, `state`, `progress`), `saved`, `run`, `has_map`, `errors`, `loading` |
| `codemap_ui` | Set by the core: `sandbox`, `maps`, `root`, `loading`, `error`, `map` (`docs`, `count`, `broken`, `doc`: `{key, title, path, section, markdown, outline, backlinks, code}`), `diagram`, `run`, `writing`, `running`, `lens`, `lens_left` |

**Rules.** File content is read raw (`Call::raw`): text and Markdown are
drawn by the core's elements, pictures decoded by `crate::picture` (24 MB
before decoding, scaled to 1600 px), HTML never drawn. Nothing a sandbox
names is written on this computer by the feature: downloads, opens, reveals
and uploads are the core's (`src/files.rs`, names and verdicts by
`src/safe.rs`). No intent takes a local path: files come from a drop on the
panel or a pane, a paste, or the file dialog.

| Intent | Arguments | Risk | What the core does |
|---|---|---|---|
| `files_open` | `sandbox`, `tab` (`artifacts`, `files`, `recent`) | direct | Opens the panel on that sandbox (the last tab, else Artifacts) and reads its lists |
| `files_tab`, `files_place`, `files_browse`, `files_up`, `files_back` | `tab` / `id` / `path` (`~` is home) / none | direct | Tabs, places, folders |
| `files_filter`, `files_hidden` | `text` / `on` | direct | The filter, dotfiles |
| `files_more` | | direct | 200 more rows of a long list |
| `files_fold` | `id` (a folder row's) | direct | Opens or folds an artifact folder |
| `files_select` | `id` (a row's) | direct | A folder is listed, a file previewed; a row not listed is refused |
| `files_show_in_folder` | `id` | direct | Recent: lists its folder, on it |
| `files_copy_path` | none | direct | The path as the agent sees it, through `copy_text` |
| `files_download`, `files_reveal` | none | direct | Into Downloads (never over a file); and shown in its folder |
| `files_show_saved` | none | direct | Shows the last file saved in its folder |
| `files_save_as` | none | approval | The save dialog, after the core's dialog names the file and its size |
| `files_open_with` | none | approval | The core downloads it and judges it (its name and first bytes); its dialog says what it is; a program, script, installer or shortcut is never opened; a file it warns about needs the box ticked |
| `files_delete` | none | approval | The dialog shows the path and size (a folder: everything in it); only under the workspace, a read-write mount, home or /tmp, never those folders, `.sbxw-artifacts` or `.git` |
| `files_star` | `path` | direct | Pins a folder among the places (kept across runs) |
| `files_refresh` | none | direct | Reads the lists again |
| `files_upload_pick`, `files_upload_paste` | none | direct | The file dialog; the clipboard's picture |
| `files_upload_remove` | `index` (none: all) | direct | Drops a waiting file |
| `files_upload_send` | `index` | approval | Waiting file `index`: the dialog shows its local path, size, the sandbox and session; the core streams it (128 MB at most), then the daemon's path is typed into that prompt, not submitted |
| `files_upload_dropped` | none | approval | The next file dropped on a terminal pane, the same dialog at once; several are asked one after the other, Cancel stops |
| `codemap_open` | `sandbox`, `map` (a lens's folder) | direct | Opens the panel and reads the map's files |
| `codemap_doc` | `doc`, `anchor` (a section id) | direct | A file, or one section |
| `codemap_link` | `url` | direct | A link pressed in a map file: a wiki link followed in the panel, any other in the browser |
| `codemap_filter` | `text` | direct | Searches the map's files |
| `codemap_generate` | `sandbox` | direct | `/codemap` in a background session (the daemon refuses a stopped sandbox, no codemap kit, one already running) |
| `codemap_lens_set`, `codemap_lens` | `text` / `sandbox` | direct | The reader (400 characters at most); `/codemap-lens` in the background |
| `codemap_diagram` | none | approval | The map's Archify diagram is HTML the app does not draw: saved into Downloads, judged by the core, opened in the browser after its dialog |

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
`browser-kits` (Setup on its Kits tab, `open_modal`) and `browser-recreate`
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
