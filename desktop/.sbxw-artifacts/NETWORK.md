# Phase 3, thread desktop-shell: the structure, network and shell extras

Linux (Debian trixie, aarch64) in an sbx sandbox, no GPU, no display; every
image below is a headless Rune render, each one looked at. The real daemon,
real sbx and macOS are **not verified** (see the end).

## 1. The structure every feature uses

| Part | Where | What |
|---|---|---|
| Registration | `src/feat/mod.rs` | A feature is `pub const FEATURE: Feature` in `src/feat/<area>.rs` (or `<area>/mod.rs`) and ONE line in `features! { }`. It declares its data (`Data`), stream events, screens and their slot, intents with risk, `act`, `on_answer`, `on_data`, `on_event`, extras, stand-in routes and `core_only` body fields. `docs/screens.md`, "Adding a feature", has a worked example |
| Intents | `src/intent.rs` | Feature rows join the closed list (`intent::all()`), checked the same way, exported to `intents.json`. Core slot intents: `open_modal`, `close_modal`, `open_panel`, `close_panel`. `open_terminal` takes `mode = "kit-add"` |
| Core side | `src/shell/features.rs` | Reads data (at connection, after events, every N s, on demand, per running sandbox), runs actions off the UI thread (`Link::call`, `Link::fetch`), passes answers (status and body, non-2xx included) to `on_answer`, read failures to `data.app.errors[key]` |
| The approval rule | `features::admit`, `features::core_only_check` | An intent of risk Approval may answer only ONE `Confirm` or `Prepare`; anything else is refused and nothing runs. Body fields in `core_only` go out only from a Confirm the person pressed (`Confirm::grant`, `grant_lines`: what the dialog lists is exactly what is sent) |
| Generic confirmation | `src/approval.rs`, `confirm_view` | Title, plain lines, granted lines, an optional block (a diff in colour), an acknowledgement checkbox, secret fields (Rune secure inputs: masked, no copy, no accessibility text, wiped after Confirm; values go into the body at a validated dotted path, never over a field the feature set), Confirm and Cancel labels, red when it removes or widens. Prepare: the dialog loads while the core reads, then builds from the daemon's answer. Jarvis's proposal dialog and the OSC 52 card are unchanged |
| Slots | `shell.rs`, `shell/features.rs` | A modal (one at a time, fixed height up to 720, dimmed window, Escape or a press outside closes it), docked right panels beside Jarvis (several open: tabs, one shown), extras on others' slots: top bar (`extra-topbar-<id>`), sandbox rows (`extra-sandbox-<sandbox>-<id>`), pane bars (`pane-<n>-x-<id>`, core-drawn), a notice that can carry an intent ("Click to open") |
| Fixtures | `src/fixture.rs`, `client.rs` | A fixture's whole `daemon` object answers features' routes through their stand-in functions; `app.panels` and `app.modal` open slots at start |
| Stand-in | `src/bin/standin.rs` | Any `/api/` route it does not know goes to the features' stand-in routes; a route that changes the list makes the list follow |
| Luau budget | `src/luau.rs` | The instruction count stays the hard limit; the wall clock is 1 s (only a blocking call); a failed render keeps the last good view |

No new element was needed: features built tables, tabs and forms from
`row`, `column`, `list`, `button`, `input`, `select`, `checkbox`, `badge`.

## 2. Network (`src/feat/network.rs`, `screens/network.luau`)

Parity with `assets/js/approvals.js` and `ports.js`.

| What | How |
|---|---|
| sbx's pending requests | `GET /api/approvals` every 5 s and after `session` events (sbx has no event of its own). A new one: a notice `web-app asks: api.github.com:443`, click to open; amber `⚑ N` on the sandbox row, `sbx asks N` in the top bar, `sbx asks` on the bar of each pane showing it |
| The Network panel | Docked right (440 pt): requests with their options; a running-sandbox picker; blocked hosts with Allow (none for an organisation block); rules (decision, host, scope, source, remove; filter above 8 rules; organisation rules locked); add a rule (resources, allow or deny, all sandboxes, HTTP methods / path / query); recent egress; `sbxw.toml` allow list; re-read policy; ports (open in the browser, unpublish, publish with an optional host port, address, alias); `/etc/hosts` aliases |
| Through the core dialog (risk Approval) | `approval_answer` (every answer: who, what, sbx's detail lines, the option's label, how long), `network_allow_blocked` (the resource from the daemon's blocked list, this sandbox only), `network_allow` (scope said; every sandbox in red), `network_unblock` (removing a deny or a rule whose decision is unknown; scope from the listing), `port_publish_fixed` (address and port; 0.0.0.0 in red, "reachable from other machines") |
| Direct (narrows, or sbx picks) | `network_deny`, `network_rule_rm` (an allow rule only: the feature refuses a deny), `port_publish` (sbx picks the host port, loopback), `port_unpublish`, `port_open` (URL from the daemon's list), `policy_refresh`, `network_show`, `network_refresh` |
| Text source | The dialog's words come from the daemon's data held by the core (the request, the blocked list, the rule view), never from the screen's arguments; a request or option the daemon did not list is refused. A rule typed in the form is shown exactly as it will be sent |

## 3. Shell extras (`src/feat/shell_extras.rs`, `screens/help.luau`, `sandbox-actions.luau`)

| What | How |
|---|---|
| Sandbox actions | `…` on every row opens a dialog: duplicate (name suggested as the web UI does), network, close its terminals (`detach`), recreate (sandboxes an older sbx made), remove |
| Remove, recreate | Risk Approval, red, with the sandbox's agent and workspace from the list ("its workspace folder stays on this machine") |
| After a removal | The list follows at once (feature POSTs re-read it) and panes showing a sandbox that left the list empty themselves |
| Usage | `5h 42% · 7d 12%` in the top bar from `GET /api/usage` every 30 s; amber at 80 %; hidden with no subscription data |
| Help | ⓘ in the top bar: why the list may be empty (sbx login, restart sbx's daemon), keyboard shortcuts (per platform, the push-to-talk key from the daemon), versions from `GET /api/state` |

## Evidence

Tests: `CARGO_PROFILE_DEV_DEBUG=0 cargo test -j1` in `desktop/`: 159 lib
tests, 3 link, 1 pump, all passing at commit 903c2fd. Those of this thread:

- `shell::features::tests::a_screen_emitted_allow_never_runs_without_the_core_dialog`:
  through the whole App on `fixtures/network/basic.json`, a screen's
  `approval_answer` and a global `network_allow` open the core's dialog and
  send nothing; Cancel sends nothing; only `confirm_click` sends, and the
  list empties.
- `an_approval_intent_only_ever_opens_the_core_dialog`, `core_only_fields_go_out_only_after_a_confirm`
  (shown lines == sent list), `feat::tests::secrets_go_at_their_path_and_never_over_a_field`,
  `paths_match_their_patterns`, `names_are_unique_and_well_formed`.
- `feat::network::tests::*`: every widening is a Confirm with the daemon's
  words, forged requests or options refused, narrowing direct, a deny
  removal refused on the direct intent, the notice and badges, the stand-in
  answer empties the list and adds the rule.
- `feat::shell_extras::tests::*`: removal dialog words, risk per intent, usage label and tone.

Images (`desktop/.sbxw-artifacts/network/`, `shell-extras/`):

- `net-basic.png` (fixture `network/basic`): top bar `sbx asks 1`; web-app's
  row `⚑ 1` and the network button; the panel: the request
  `api.github.com:443` with Allow always / Allow once / Deny; blocked
  `registry.npmjs.org:443 ×5 Allow`, `pastebin.com:443 org`; four rules with
  the org one locked; the add-rule form; recent egress; the notice "web-app
  asks: api.github.com:443 / Click to open".
- `net-many.png`: three requests over two sandboxes, a pane open on web-app.
- `network-strip.png` and `network-end.png` (`scripts/demo-network.sh`,
  against the stand-in): (1) panel open on web-app, nothing waiting;
  (2) the stand-in makes sbx ask: notice, badges, the request in the panel;
  (3)-(4) Allow always → the core's dialog "Allow always: web-app asks to
  reach a host" with Sandbox, Asks for, detail, Your answer, How long;
  (5)-(6) after Confirm: "Nothing waiting", `api.github.com` listed as an
  allow rule of web-app, "answer web-app's request (Allow always): done".
  Log: `intent ... approval_answer`, `network: confirmed ...`, stand-in
  `POST /api/approvals/respond -> 200`.
- `sx-remove-dialog.png`: "Remove web-app?" in red over the actions dialog,
  with its workspace; `sx-remove-end.png`: web-app gone from the list, the
  pane empty, "remove web-app: done".
- `sx-help.png`, `sx-actions.png` (docs, made by an older sbx: Recreate
  shown), `sx-usage-high.png` (amber gauge).
- `setup-modal.png`, `create-modal.png`, `setup-full.png`: the other
  threads' modals opened from their top-bar extras in the whole window.
- `integration-jarvis-setup-voice.png`: one stand-in run with Jarvis open
  and Setup's Voice tab (the stand-in now answers `/api/voice` from the
  fixture).

Older demos rerun after the changes: `demo-live.sh` (three list changes in
order), `demo-reload.sh` (five loads, the syntax error banner),
`demo-reconnect.sh` (reconnected with a new token), `demo-jarvis.sh`
(proposals applied, the screen file written). `demo-reconnect.sh` is now
executable in the tree.

## Not verified

- The real daemon and real sbx: the shape of `GET /api/approvals` options
  (ids and labels sbx 0.53 really gives: the "how long" words are derived
  from the label, and say "as sbx's option says" when unsure), the rule view
  columns of `sbx policy ls --wide`, a port request lapsing after 45 s.
- macOS: none of it was built or run on a Mac.
- A real window: modal press-outside, panel tabs, secure fields typed with
  a real keyboard.
- `GET /api/approvals` every 5 s shells out to sbx on the host; the web UI
  polls only while its dialog is open. Lower it if it costs.

## Known limits

- The modal has a fixed height (up to 720 pt), so a short dialog leaves
  empty room.
- Extras are buttons or badges only (no menus).
- The approvals poll is per app, not per sandbox; sbx has no event.
