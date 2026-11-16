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
| `layout.panes`, `layout.split` | 1 to 4; `columns` or `rows` |
| `panes` | One entry per shown pane: `index` (from 1), `sandbox` (or nil), `mode` (`agent`, `bash`, `kit-add`), `thread` (the thread whose session it shows, or nil: the sandbox's own), `focused` (bool), `status` (`empty`, `connecting`, `connected`, `reconnecting`, `closed`, `error`) |
| `overrides` | Screens running an override file, by name: `path`, `stale` (made for an older built-in; see "Where screens live") |
| `kept` | What features asked the app to keep across runs (`Action::Remember`), by feature then key |
| `platform` | `macos`, `linux`, `windows` |

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
| `set_layout` | `panes` (1 to 4, default 1), `split` (`columns` default, `rows`) | How many panes show, and how two split |
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
`single`, `chat`, `link`, `mic`, `speaker`, `send`, `sparkle`.

## Where screens live

Built-in screens are compiled into the app (`desktop/screens/<name>.luau`).
A file with the same name in the **override folder** replaces it and reloads
on every save (within about 150 ms); deleting the file brings the built-in
back. The folder is `~/.sbxw/desktop/screens/`, or `$SBXW_DESKTOP_SCREENS`.

Screens: `topbar` (the bar at the top, right of macOS's window buttons),
`sandboxes` (the side panel) and `jarvis` (the panel docked on the right,
shown while `data.jarvis.open`). The terminals, their pane bars, the
panel's resize handle and every approval dialog are drawn by the core.

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
A sandbox's extras are looked for only when its row is drawn (a row key
`row-<name>`, or its name as a text), so a filter does not bring them back.

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
writes a PNG and the element tree. `desktop/.sbxw-artifacts/testing.md` has the rest:
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
| `on_files` | `fn(&Ctx, Incoming) -> Vec<Action>`: what the person handed it, `Incoming::Files(paths)` or `Incoming::Png(bytes)`. Files dropped on its shown panel or dialog; a picture pasted (⌘V) while its panel is shown and no terminal has the keyboard (a paste into a terminal stays the terminal's); the answers of `PickFiles` and `ReadClipboard`. The only ways a feature gets a path to read: never from a screen or a sandbox. Decode pictures with `crate::picture::prepare` (limits, no animation, scaling, PNG or JPEG out), then apply the feature's own policy |

Actions (`feat::Action`): `Call(Call)` (a daemon request off the UI thread:
`Call::post(path, body, what)` / `Call::get`, `.then(&[keys])`,
`.store(key)`, `.timeout(secs)`, `.tag(t)`, `.quiet()`), `Confirm`,
`Prepare`, `Refresh(key)`, `Fetch { key, path }`, `Set { key, value }`,
`OpenModal { screen, arg }`, `CloseModal`, `OpenPanel(screen)`,
`ClosePanel(screen)`, `Notice { text, error, then }` (`then`: an intent a
click on the notice runs), `OpenLink(url)`, `Intent { name, args }` (checked
exactly as from a screen), `Remember { key, value }` (kept across runs in the
app's state file, read back as `data.app.kept.<feature>.<key>`; null forgets).
A `Call` of method `Core` is one of the core's own operations (restoring a
screen), run only from the core's dialog. `PickFiles { title, extensions,
multiple }` opens the native open dialog (Rune's picks one file; it needs
Rune's `native-file-dialog` feature, else a notice says there is none),
`ReadClipboard` reads the clipboard for a picture, both answering through
`on_files`; `Image { key, bytes }` registers a picture for `image{ key }`
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
`modal` (`{screen, arg}` or nil), `panels` (`{open = {...}, shown}`),
`extras` (`topbar`, a list; `sandboxes` and `rows`, lists by sandbox name; each `{id,
label, icon, tone, help, count, intent = {name, args}}`), and `errors`: by
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
pressable), and `pane-<n>-x-<id>` (pane bars, drawn by the core). An extra with an
`intent` is a button, without one a badge.

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
