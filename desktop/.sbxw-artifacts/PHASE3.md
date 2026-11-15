# sbxw desktop, phase 3: shared structure, network, creation, settings

Three threads on one branch (`sbxw/team-p-d562f04a5ca7a14efc3133c0b49c15dc`):
desktop-shell (structure, integration, network, shell extras),
desktop-create (creating a sandbox), desktop-setup (secrets, kits, skills,
MCP, hosts, voice). Built and tested on Linux (aarch64, no GPU, headless
Rune renders); **nothing was run on a Mac or against the real daemon**.

Reports: [NETWORK.md](NETWORK.md) (structure, network, shell extras),
[CREATE.md](CREATE.md), [SETUP.md](SETUP.md). Routes:
[daemon-api.md](daemon-api.md). Screen contract and "Adding a feature":
`desktop/docs/screens.md`.

## What each area delivers

| Area | Delivers | Through the core's own dialog |
|---|---|---|
| Structure (desktop-shell) | `src/feat/`: a feature is one `const` and one line in `features! { }`; data reads, actions, answers, read errors (`data.app.errors`), slots (modal, docked panels with tabs, extras on the top bar, sandbox rows and pane bars, notices that open something); one generic confirmation (lines, granted lines, a block or diff, acknowledgement, secure secret fields, Prepare); `core_only` body fields; the stand-in and fixtures answer features' routes | The rule itself: an Approval intent can only open the dialog (`features::admit`) |
| Network (desktop-shell) | sbx's pending requests with their options, shown as soon as they arrive (notice, badges on the row, top bar and pane bar); the Network panel: blocked hosts, rules, add or remove a rule, recent egress, re-read policy, published ports (open, publish, unpublish), host aliases | Every answer to a request, every allow, removing a deny, a fixed host port |
| Shell extras (desktop-shell) | Per-sandbox actions dialog (duplicate, network, close terminals, recreate, remove), usage gauge, Help with keyboard shortcuts and versions; panes of a removed sandbox empty | Remove, recreate |
| Create (desktop-create) | Chooser, workspace form (folders, extra folders RO/RW, restricted sub-folders with preview, ports, kit folders, template rebuild, remote host and git clone), chat form; bring-ups with live steps, failure, Retry, Dismiss; the new sandbox opens in a spare pane | What the sandbox reaches beyond its workspace; the daemon's host-access question (exact lines sent back) |
| Setup (desktop-setup) | Secrets and GitHub token, kits lines and waiting kits (`kit-add` pane), kept skills, MCP servers (remote with secrets, local with its command), remote hosts (fingerprints read by the core), voice models | Every secret (typed only in the core's secure fields), kits lines, skills, MCP add/unload/remove, hosts, voice install/remove |

Tests at the end of the phase: `cd desktop && CARGO_PROFILE_DEV_DEBUG=0 cargo test -j1`
(the link step is killed for memory when several builds run at once with
debug info). Scripted runs: `scripts/demo-network.sh`, `demo-create.sh`,
`demo-setup.sh window`, and the older `demo-live.sh`, `demo-reload.sh`,
`demo-reconnect.sh` (run with `bash`), `demo-jarvis.sh`, all rerun.

## What of the web UI is still not ported

| Web UI | Why |
|---|---|
| Threads under a sandbox, lead and plans, the board (`sidepanel.js`, `pane-controls.js`, `plans.js`, `board.js`) | The next plan's threads (not in this one) |
| Relay requests (`relay.js`) | Next plan |
| Files and artifacts, uploads, paste image (`files.js`, `upload.js`) | Next plan |
| Browser side panel, embed pairings (`browser.js`) | Next plan |
| Virtual Mac (`mac.js`), Setup's Mac tab | Next plan |
| Templates dialog, Setup's Templates tab, env file card (`templates.js`, `envfile.js`) | Not in any thread of this plan |
| Code map runs (`codemap.js`), Remote Control (`pane-controls.js`), SSH card (`ssh.js`) | Not in this plan |
| Agent activity strips (liserets), `/api/ptys`, answering a session from the UI (`/api/answer`) | Not in this plan |
| Create: Alt+N, the host remembered across restarts | No global shortcut registry for features yet; app state not persisted (CREATE.md) |
| Setup: MCP edit prefill, copy the provision command, last tab across restarts, kept skill's date | `sbx mcp ls` gives no detail; no clipboard intent for screens; no persisted screen state; no date formatting in Luau (SETUP.md) |
| Network: the policy log's sort by decision, the raw `sbx policy ls` output fold | Left out of the panel; rows are shown in sbx's order |
| The "several tabs" warning (the app's stream counts as a browser tab) | Unchanged since phase 1 |

## For the next plan (threads and plans, relay, files, sandbox browser, virtual Mac, templates)

Still to build: each of those areas as a feature (`src/feat/<area>`), with
its routes from the "Not covered yet" table of daemon-api.md. What a new
feature thread needs to know that "Adding a feature" does not say yet:

- **Register through the integrator.** Never edit `src/feat/mod.rs`
  yourself: send the line (`<area>,`) to the lead.
- **Build with `CARGO_PROFILE_DEV_DEBUG=0 cargo test -j1`** and your own
  `CARGO_TARGET_DIR`; three links at once with debug info get killed.
- **Panels are one at a time with tabs.** The relay, files and browser
  panels will be tabs of the same right column, left of Jarvis.
- **Sensitive actions still to come** (Approval, a `Confirm` built from
  the daemon's data): approving and merging a plan, routing a relay
  request, deleting a file in a sandbox, a browser allow. Copy
  `feat/network.rs`: the dialog's words come from the data the core read,
  a request id the daemon did not list is refused, narrowing stays direct.
- **No clipboard, download or file-picker intents exist yet.** Files
  (download an artifact) and Setup (copy a command) need new core intents:
  ask the integrator; they belong in `intent.rs`, not in a feature.
- **No persisted state.** A feature that must remember across restarts
  (last tab, last host) needs a core store; none exists.
- **The terminal panes are the core's.** A feature opens one only through
  the `open_terminal` intent (`Action::Intent`); threads will need `thread`
  (already an argument) and possibly a new mode from the core.
- **Stream events** reach a feature only if named in `events`; data keys
  can also follow events (`Data::on`). There is no event for sbx approvals,
  so network polls every 5 s.
- **Fixture keys are route answers**: a fixture's `daemon` object holds
  each route's answer under the feature's data key, in the shape the daemon
  sends, so the stand-in, fixtures and the real daemon agree.

## One macOS test procedure, every area

Prerequisites: on the Mac, this branch checked out; `sbxw up` running (the
daemon); at least one running sandbox with a workspace (called `web-app`
below); the app built and started with no arguments:
`cd desktop && cargo run --release`. The web UI open beside it
(`sbxw web`) helps to compare. Each area's own report has more steps.

**Structure and shell extras** ([NETWORK.md](NETWORK.md))

1. The window opens connected (green dot, "daemon at 127.0.0.1:7681"). The
   top bar shows **New**, a gear, the usage gauge (`5h N% · 7d N%`, if you
   use a subscription) and ⓘ. Each running sandbox's row has a ⇄ (network)
   and a … button.
2. Press ⓘ: Help opens (empty-list steps, keyboard shortcuts with ⌘J, F8,
   ⌘V, versions of sbxw and sbx). Press Escape: it closes. Open it again
   and click outside it: it closes.
3. Press … on `web-app`: the actions dialog shows its workspace. Type
   `web-app-copy` under Duplicate and press Duplicate: a notice says done
   and the copy appears in the list. On the copy, press … then
   **Remove…**: the app's own red dialog "Remove web-app-copy?" names it
   and its workspace. Cancel: nothing happens. Again, then **Remove**: it
   leaves the list; a pane that showed it is emptied.
4. Press … then **Close its terminals** on a sandbox with a pane open: the
   pane shows the terminal closed; press the row to open it again.

**Network** ([NETWORK.md](NETWORK.md))

5. Press ⇄ on `web-app`: the **Network** panel docks on the right. It
   shows "Waiting for you", web-app's blocked hosts, rules, recent egress
   and ports. Compare with the web UI's Ports dialog for web-app: same
   rules, same ports.
6. In web-app's terminal, run `curl -sI https://example.org`. Within about
   5 seconds: if sbx asks, a notice "web-app asks: example.org:443"
   appears, an amber ⚑ 1 on web-app's row, "sbx asks 1" in the top bar and
   on web-app's pane bar; the request shows in the panel with sbx's
   options. If sbx simply refuses, `example.org:443` shows under **Blocked
   recently** after ↻.
7. Press an option (or **Allow** on the blocked host): the app's own dialog
   shows who asks, the host, sbx's detail, your answer and how long. Press
   **Cancel**: nothing is answered (`sbx policy approval ls` still lists
   it). Press it again, then confirm: the request leaves the list, the
   badges go, and `example.org` shows as an allow rule of web-app. Run the
   curl again: it goes through.
8. Remove that rule with its ✕ (an allow: no dialog, it narrows). Add a
   **deny** rule for `example.net` (no dialog), then press ✕ on it: the
   app's red dialog says removing it may make example.net reachable again;
   confirm.
9. Type `example.com` in the rule form, tick **all sandboxes**, press Add
   rule: the dialog is red and says every sandbox, including ones created
   later. Cancel.
10. Ports: in web-app's terminal start `python3 -m http.server 8000`. Type
    `8000` as the sandbox port, leave the host port empty, Publish (no
    dialog: sbx picks a port on 127.0.0.1). The port shows; press **Open**:
    the browser shows the directory listing. Unpublish it. Publish again
    with host port `8000`: the app's dialog names 127.0.0.1:8000; with host
    address `0.0.0.0` it is red ("reachable from other machines"). Cancel.
11. Press **Re-read policy now**: a notice says done.
12. Open Jarvis (⌘J): the Network panel and Jarvis show side by side. Open
    Setup (gear): the modal covers both; close it; both are still there.

**Create** ([CREATE.md](CREATE.md), its "macOS test procedure" for details)

13. Press **New** → **Workspace sandbox**, browse to a project folder,
    name it, press **Create** with no extra folder: no question; the **New
    sandboxes** panel shows the steps, then the agent opens in a spare
    pane.
14. Create again with a second folder in read-write and a port: the app's
    dialog lists both; Cancel leaves the job "Waiting for you"; Retry and
    confirm: it is created.
15. In a project whose `sbxw.toml` asks for host access not approved yet:
    the dialog "Allow host access?" lists the daemon's exact lines; Allow
    creates it.
16. **Chat sandbox** with an empty name: created and opened.

**Setup** ([SETUP.md](SETUP.md), its "macOS test procedure" for details)

17. Press the gear: Setup opens on Secrets. Set the GitHub token: it is
    typed in the app's own bulleted field (⌘V works), never in the screen.
    `sbx secret ls` lists `github`; the app's log and `~/.sbxw/desktop/`
    contain no part of it.
18. Kits: tick a kit, Save…: the dialog shows the diff of the `kits` line;
    a kit that reaches outside the sandbox needs its box ticked first.
19. MCP: add a local server (`echo hi`): Add stays off until "I have read
    this command" is ticked. Remove it.
20. Hosts (if you have an SSH host): add it; the dialog reads and shows the
    host key fingerprints before anything is saved.
21. Voice: the tab shows the models' state; Install, Cancel, Remove work as
    in the web UI.

**Across areas**

22. Stop the daemon with the Network panel and Setup open: the top bar says
    there is no daemon; the panels keep their last values; Setup's reload
    shows the daemon's error. Start it again (`sbxw up`): the app
    reconnects within seconds and the lists refresh.
23. Quit and restart the app: panels and dialogs start closed (not
    persisted); the sandbox list and badges come back.

If a step fails, report: the step number, a screenshot, and the app's log
(`RUST_LOG=info cargo run --release 2>&1 | tee app.log`).
