# sbxw Island (macOS notch companion)

A tiny native macOS app that turns your Mac's menu bar / MacBook notch into a
**Dynamic-Island-style panel for your sbxw sessions** — inspired by
[vibeisland.app](https://vibeisland.app). It shows every live session and its
state at a glance, and lets you jump straight to the right one.

It is a **companion to the sbxw web UI**, not a replacement: it reads the
session event stream the daemon exposes and opens the browser when you click a
session.

## What it shows

Each session (one per sandbox × mode) has a state, shown with a colored dot:

| State | Label | Dot | Meaning |
|---|---|---|---|
| `working` | working… | blue | the agent is running a turn (a tool, or thinking) |
| `attention` | waiting for input | orange | the agent needs you — it asked a question (`AskUserQuestion`) or hit a permission / idle prompt |
| `idle` (talked to) | waiting for your reply | teal | Claude finished its turn and it's your move — the only cue for a **free-text question asked inline** (e.g. "Question 1: …?"), which fires no `AskUserQuestion` hook |
| `idle` (untouched) | idle | gray | a sandbox that has never been prompted this session |
| `exited` | ended | — | the session ended (row disappears) |

Each session row is rich: the **sandbox name**, the **last prompt you sent**
("You: …"), the agent's **current activity** (last output line), an **agent tag**
(from `sbx ls`), and **elapsed time**.

## Two surfaces

**Menu bar** — a status item whose icon reflects the most urgent state:
bell-badge when any session is *waiting for input*, a filled glyph when any is
*working*, otherwise a neutral grid. Click it for a dark popover with a
`N working · N waiting · N idle` tally and the full list; **click a session** to
open `…/#sandbox=<name>` in the browser (focuses that sandbox in the sbxw UI).

**The notch (Dynamic Island)** — a panel hugging the MacBook notch that stays
hidden until something happens:

- **State change** → a compact toast drops from the notch, auto-collapsing after
  **5 s**.
- **A question** → when the agent asks something with a numbered menu (an
  approval, a choice), the notch expands into an **interactive card**: the
  question plus one button per option (**⌘1 / ⌘2 / …**). Picking one sends the
  answer straight into the session's terminal — no context switch. The card
  stays until you answer.

  When the agent asks **several questions at once**, the card walks them one at
  a time (the **1/2** badge, **⌘←** to go back) and keeps your earlier picks
  visible. Nothing is written to the terminal until the last question is
  answered, so abandoning the card leaves the session untouched.
- **Hover** → the full session list, auto-collapsing **5 s** after the pointer
  leaves.

You can also force the list open from the menu-bar popover
("Show statuses on the notch").

## How it talks to sbxw

Everything is local HTTP to your own sbxw daemon — no cloud, no accounts. The
daemon (v1.0.16+) exposes:

- `GET /api/sessions` — rich snapshot of current sessions (seeded on launch).
- `GET /api/events` — a Server-Sent Events stream of rich session updates.
- `GET /api/sandboxes` — polled so running sandboxes appear as `idle` baselines.
- `POST /api/answer` — `{sandbox, mode, index}` selects a numbered menu option.
- `POST /api/input` — `{sandbox, mode, data}` writes raw bytes to the PTY.

Question detection is **structured, not scraped**: when the agent invokes Claude
Code's `AskUserQuestion` tool, a `PreToolUse` hook forwards the question and its
options to the daemon, and the card renders them verbatim. Other waiting states
(permission prompts, idle nudges) arrive as `Notification` hooks and show as
*waiting for input* — click through to the browser to answer those.

## Requirements

- macOS 14 (Sonoma) or later.
- A running sbxw daemon (`sbxw up` or `sbxw web <name>`).
- To build it yourself: Xcode 15+, or a standalone Swift 5.9+ toolchain.

## Install (prebuilt)

The `sbxw` installer offers it on macOS — answer **y** at the "Install the sbxw
Island menu-bar app?" prompt and it drops `SbxwIsland.app` into `~/Applications`.

To install it after the fact, grab `SbxwIsland-macos.zip` from the
[releases page](https://github.com/gallettom/sbxw/releases), unzip into
`~/Applications`, and clear the download quarantine so Gatekeeper runs the
ad-hoc-signed bundle:

```bash
xattr -dr com.apple.quarantine ~/Applications/SbxwIsland.app
open ~/Applications/SbxwIsland.app
```

The app launches as a menu-bar accessory (no Dock icon) and, on notched Macs,
also shows the floating notch panel. Use the menu-bar item to **toggle the notch
panel**, open **Settings…**, or **quit**. The first time you click a session to
jump to it, macOS asks permission to control your browser — allow it (that's how
the island brings the right tab forward). Add it under **System Settings ›
General › Login Items** if you want it running at startup.

## Build & run (from source)

Quick iteration:

```bash
cd SbxwIsland
swift run          # builds and launches the app
```

or open it in Xcode (`cd SbxwIsland && open Package.swift`, then ⌘R).

To produce the same distributable `.app` + zip the release ships (universal,
ad-hoc-signed), run the packaging script from this directory:

```bash
./build-app.sh dist
#   → dist/SbxwIsland.app, dist/SbxwIsland-macos.zip, dist/island-version.txt
```

The app is versioned independently of the `sbxw` CLI: the version baked into
`Info.plist` comes from `ISLAND_VERSION` in `build-app.sh` (currently `1.0.0`),
which is what the release workflow ships too. Bump it there when the island
itself changes; `SBXW_ISLAND_VERSION=…` overrides it for a one-off build.

That version is also written to `island-version.txt` and published next to the
zip, which is how `sbxw update` decides whether an installed bundle is stale:
it compares the file against the installed app's `CFBundleShortVersionString`,
and only then downloads the zip, quits the app, replaces it, and relaunches it
if it was running. Users who never installed the app are left alone, and
`sbxw update --no-island` skips the whole step.

## Configuration

If your daemon is not on the default `http://sbxw.localhost:7681`, open
**Settings…** from the menu-bar popover and set the URL/port. It is stored in
`UserDefaults` and applied immediately (the stream reconnects).

## Notes & limitations

- **Running sandboxes show as `idle`; live state needs an open terminal.** The
  app polls `GET /api/sandboxes`, so every running sandbox appears in the island
  right away as an `idle` baseline. Its real `working` / `needs you` state only
  starts flowing once its terminal has been opened once in the browser (sbxw
  spawns each PTY lazily, on first connect), which upgrades the entry in place.
- **Jump = open in browser.** sbxw sessions run inside Docker sandboxes served
  over a web terminal, so "jump to a session" focuses the browser tab rather
  than a local terminal app.
- This app is **read-only** toward the agent: it monitors and links, it does not
  send approve/deny back into the session (a possible future addition).
