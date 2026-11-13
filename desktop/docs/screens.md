# Writing screens

A screen is one Luau file that describes what part of the window shows,
from data the core hands it. It cannot reach the daemon, the disk, the
network or the clipboard. It draws with the elements below and asks the core
for things by returning **intents** from its actions. The core checks every
element, every prop and every intent, and refuses anything it does not know.

This document is the whole contract: write a screen against it without
reading the Rust. The element set is **frozen** (wave 1). Asking for a new
element or intent goes through the core's owner.

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
| `layout.panes`, `layout.split` | 1 to 4; `columns` or `rows` |
| `panes` | One entry per shown pane: `index` (from 1), `sandbox` (or nil), `mode` (`agent`, `bash`), `focused` (bool), `status` (`empty`, `connecting`, `connected`, `reconnecting`, `closed`, `error`) |
| `platform` | `macos`, `linux`, `windows` |

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

```lua
text{ "SANDBOXES", size = "sm", weight = "semibold", color = "text-muted" }
```

### `row`

Children side by side. Children are centred vertically.

| Prop | Value | Effect |
|---|---|---|
| `gap` | number or space | Between children |
| `align` | `start`, `center`, `end`, `stretch` | Cross axis (vertical); default `center` |
| `justify` | `start`, `center`, `end`, `between` | Main axis |
| `hover` | colour | Background under the pointer |
| `on`, `arg` | action name, any value | The whole row is pressable (a button for screen readers); `hover` defaults to `surface` |

```lua
row{ gap = 6, w = "full", dot{ tone = "green" }, text{ "running" }, spacer{}, text{ "2" } }
```

### `column`

Children one under the other. Same props as `row`; `align` is the horizontal
axis and defaults to `stretch`.

```lua
column{ key = "row-docs", on = "open", arg = "docs", gap = 3, px = 9, py = 7, radius = "sm",
  text{ "docs", weight = "medium" }, text{ "running", size = "sm", color = "text-muted" } }
```

### `list`

A bordered list: each child padded (8 pt left and right, `gap` above and
below, default 4), with a 1 pt line between children.

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
a `help`, which is also its name for screen readers).

| Prop | Value | Effect |
|---|---|---|
| `on` | action name | Required unless `disabled`. Called on press |
| `arg` | any value | Passed to the action |
| `variant` | `neutral`, `primary`, `danger`, `ghost` | `neutral` (default): grey; `primary`: green, for the one thing a dialog is for; `danger`: red label; `ghost`: no box until hovered |
| `icon` | icon name | Before the label |
| `color` | colour | The label's and icon's colour |
| `disabled` | bool | Greyed, not pressable |

```lua
button{ icon = "stop", on = "stop", arg = sb.name, color = "red", help = "Stop" }
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
| `items` | list of items | Each a string (id and label) or `{ id = "...", label = "...", icon = "...", disabled = bool }` |
| `on` | action name | Required unless `disabled`. Called with `value` = the picked item's id |
| `arg` | any value | Passed too |
| `icon` | icon name | On the button |
| `variant` | `ghost` | No frame around the button; anything else: framed |
| `disabled` | bool | |

The list closes on a pick, on Escape, or on a second press of the button. A
press elsewhere does not close it (yet).

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

## Events and actions

Every event calls an action of the screen: **`action(state, arg, value)`**.

| Element | Event | `arg` | `value` |
|---|---|---|---|
| `button`, `row` / `column` with `on` | press | its `arg` | nil |
| `input` | `on`: each edit; `on_submit`: Enter | its `arg` | the text |
| `menu`, `select` | pick | its `arg` | the item's id |
| `checkbox` | press | its `arg` | the new bool |

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
| `open_terminal` | `sandbox` (string, required), `mode` (`agent` default, or `bash`), `thread` (string), `pane` | Opens that terminal in a pane and focuses it. The agent of a stopped sandbox starts it (the daemon runs `sbx run`); the list then shows it running |
| `start_sandbox` | `sandbox` (required), `pane` | Starts a stopped sandbox by opening its agent, as the web UI's Connect does |
| `stop_sandbox` | `sandbox` (required) | Closes its terminals, shows it `stopping…`, asks the daemon to stop it; the list follows |
| `select` | `sandbox` (string; none: nothing selected) | Sets `data.app.selected` |
| `set_layout` | `panes` (1 to 4, default 1), `split` (`columns` default, `rows`) | How many panes show, and how two split |

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
screen asks. Later intents that need the person's yes (a Jarvis proposal, a
network rule, a host command) will be held, and **the core** draws the
question above every screen; a screen can neither draw that dialog nor
answer it.

## Theme

The global `theme` (read only) is the web UI's design tokens
(`assets/app.css`). Any colour prop takes these names; `theme.color[name]`
gives the `#rrggbb`.

| Table | Names |
|---|---|
| `theme.color` | `bg`, `surface`, `surface-alt`, `elevated`, `border`, `dim`, `text`, `text-bright`, `text-muted`, `text-soft`, `text-dim`, `blue`, `blue-tint`, `green`, `green-strong`, `green-deep`, `green-tint`, `red`, `red-border`, `red-tint`, `amber`, `amber-strong`, `amber-tint`, `purple`, `purple-strong`, `purple-alt`, `purple-tint`, `group-1` to `group-4`, `hover-subtle`, `active-bg`, `white`, `floor`, `pane-bar` |
| `theme.radius` | `xs` 6, `sm` 8, `md` 12, `lg` 16, `pill` 999 |
| `theme.text` | `xxs` 9.5, `xs` 10.5, `sm` 11, `md` 13, `lg` 14, `xl` 16 |
| `theme.space` | `xxs` 2, `xs` 4, `sm` 6, `md` 8, `lg` 12, `xl` 16 |
| `theme.layout` | `top_bar_h` 48, `gutter` 6, `sidebar_w` 240, `pane_bar_h` 32, `traffic_lights_w` |

Rounder as the surface gets bigger, as in the web UI: chips `xs`, buttons and
rows `sm`, cards and menus `md`, dialogs `lg`.

## Icons

`icon{ name }`, and the `icon` prop of `button`, `menu` and items:
`play`, `stop`, `reload`, `plus`, `close`, `terminal`, `chevron-down`,
`chevron-right`, `check`, `warning`, `info`, `gear`, `search`, `folder`,
`file`, `flag`, `ports`, `copy`, `trash`, `more`, `columns`, `rows`, `grid`,
`single`, `chat`, `link`.

## Where screens live

Built-in screens are compiled into the app (`desktop/screens/<name>.luau`).
A file with the same name in the **override folder** replaces it and reloads
on every save (within about 150 ms); deleting the file brings the built-in
back. The folder is `~/.sbxw/desktop/screens/`, or `$SBXW_DESKTOP_SCREENS`.

Screens of wave 1: `topbar` (the bar at the top, right of macOS's window
buttons) and `sandboxes` (the side panel). The terminals and their pane bars
are drawn by the core.

## Limits

The VM has `string`, `table`, `math`, `utf8` and `bit32`, and nothing else:
no `io`, `os`, `require`, `load`, `loadstring`, `dofile`, `getfenv`,
`setfenv`, `collectgarbage`. Libraries and `theme` are read only. Each call
into the screen (`init`, `render`, an action) gets 200,000 interrupt checks
(calls and loop turns) and 50 ms; memory is capped at 16 MB; one render may
return 5,000 elements, nested 32 deep. Going over is an error in the banner;
the app keeps running.

## Checking your screen

```sh
desktop/scripts/screen.sh <screen> [fixture] [--file my.luau] [--script s.json]
```

draws the screen alone from `desktop/fixtures/<screen>/<fixture>.json` and
writes a PNG and the element tree. `desktop/.sbxw-artifacts/testing.md` has the rest:
fixtures, scripts that press elements by key, the whole window.
