# sbxw desktop, phase 3: Setup and settings

Thread `desktop-setup` of plan p-d562f04a5ca7a14efc3133c0b49c15dc (shared
branch, three threads). Linux (Debian trixie, aarch64) in an sbx sandbox,
no GPU, no display. Only `desktop/` was touched, and inside it only the
files listed below; the core side of features (`src/feat/mod.rs`,
`src/shell/features.rs`, the core-drawn confirmation in `approval.rs`) is
thread desktop-shell's.

## What was built

| Part | Where | What |
|---|---|---|
| Feature | `src/feat/setup/mod.rs` | `FEATURE`: 7 daemon routes read as data, the `voice` bus event, the `setup` modal screen (720 pt), 27 intents with their risk, the rules that check them and build each request and each confirmation, two extras |
| Stand-in | `src/feat/setup/standin.rs` | Every Setup route against the fixture's `daemon` object, changed as the daemon changes it; never keeps a secret value |
| Screen | `screens/setup.luau` | The web UI's Setup dialog as tabs: Secrets, Kits, Skills, MCP, Hosts, Voice |
| Fixtures | `fixtures/setup/*.json` | `full`, `empty`, `errors`, `looking`, `looked`, `voice-downloading`, `voice-failed`, `no-voice` |
| Tests | `src/feat/setup/tests.rs` | 16 tests (below) |
| Demo | `scripts/demo-setup.sh` | Part 1: one PNG per fixture state and tab. Part 2 (`window`): the whole window, scripted |

### Tabs, at parity with the web UI

| Tab | Web UI file | Ported |
|---|---|---|
| Secrets | `github.js` | Sign-in and services (`sbx secret ls` rows of kind `service`: `anthropic`, `github`, ... by name and scope, as the only place the web UI shows them); GitHub token: state, set or replace, "Use my gh login" (only when gh is found), remove; other services: list (scope, Remove for global ones), add (hosts, variable; the value in the core's dialog) |
| Kits | `kits.js`, `sandboxes.js` (✓) | Kits waiting for an answer, with "Answer in a pane" (`open_terminal` mode `kit-add`), also as an amber ✓ on the sandbox's row; the file picker (every sbxw.toml sbxw uses), who uses it, "no kits line" and "does not exist yet" notes, a file that cannot be edited; bundled kits as checkboxes with `default` badges and their descriptions; other kits with a `Jarvis kit` or `local kit` badge, Remove, an add field; Use defaults; Save through the core's dialog |
| Skills | `skills.js` | Kept skills (name, description, from which sandbox), Remove; pick a sandbox, Look (a stopped one is started), its skills with Keep or Replace, or "name not kept" |
| MCP | `mcp.js` | Servers on this machine with Load and Unload into a picked running sandbox, Sign in (remote only), Inspect (text shown in place), Remove; the form for a remote server (URL, headers, one secret per `${KEY}`) or a local command (command, arguments, directory) |
| Hosts | `hosts.js` | Hosts with link state (up with latency, down, never), trusted badge, address, sandbox count, Test, Provision (shows the command), Edit, Remove; Read hosts.toml again; add or edit: alias or address, user, port, key, agent port, sbx path, "Trust this host with secrets" with its warning |
| Voice | `concierge.js` (setup part) | Built without voice; each model with role, licence and state (installed, partial with size, or its approximate size); the speech runtime; download progress with Cancel; last failure; cancelled; Install (size and folder in the dialog); Load the models now; Remove models; the push-to-talk key |

### What goes through the core's dialog (risk Approval)

Setting, replacing or removing the GitHub token, using the gh login,
adding or removing a custom secret, saving a kits line (or using the
defaults), keeping or forgetting a skill, adding or replacing an MCP server,
unloading or removing one, adding or changing a remote host, removing one,
installing or removing the voice models. Each answers ONE `Confirm` (or a
`Prepare` for a new host) built in Rust from the daemon's data and the
intent's checked arguments:

- **Secret values** are typed in the dialog's secure fields (`token`,
  `value`, `secrets.<KEY>` for MCP headers; on an MCP edit the fields are
  optional, "unchanged if left empty", as the daemon skips empty ones). No
  intent row has an argument that could carry a value; the screen sees
  names and "set" only.
- **A kits line**: the file, each kit added (bundled, local, registry or
  Jarvis kit) and removed, who uses the file, the old and new line as a
  diff; an acknowledgement when an added kit can reach outside the sandbox
  (any kit not bundled, and `sbxw:ssh-agent`, `git-identity`, `dev-ports`,
  `terminal-mac`, Jarvis's own high-risk list). `sbxw:jarvis` is refused.
- **A local (stdio) MCP server**: the exact host line (`cd dir && command
  args`, shell-quoted) and "I have read this command" before Add works, as
  Jarvis's dialog does.
- **A new remote host**: the core reads the SSH host key itself
  (`POST /api/remote-hosts/key`) and shows the fingerprints; "These
  fingerprints are the host's" before Add works; only those fingerprints
  are sent back. Trusting a host with secrets is shown in red with what it
  sends. An edit keeping the address asks no key.
- **Removing a host with sandbox records**: an acknowledgement, then
  `force` (the web UI asks twice).

## Web features not ported, and why

| Web UI | Why |
|---|---|
| Setup → Templates, Setup → Mac | Not in this thread's area (templates and the virtual Mac are other screens) |
| MCP: Edit prefills the form from `sbx mcp ls` detail | `sbx mcp ls --format json` only documents `name` and `type`; the desktop form starts empty with "Clear". Replace is possible by typing the server again (`replace`) but no button prefills it yet |
| Provision: Copy to clipboard | No clipboard intent for screens; the command is shown as text to type or select |
| The last tab across restarts | Kept for the session only (the screen's state); the web UI keeps it in localStorage |
| Kept skill's save date | Luau has no date formatting (`os` is not in the VM); the sandbox it came from is shown |
| SSH connection card (`ssh.js`) | Per sandbox, a pane-control popover, not Setup; belongs with the sandbox actions |
| Anthropic / claude.ai login switch (Remote Control) | Per sandbox (`pane-controls.js`), not Setup |

## Evidence

Unit tests, real tree at fa226b3 plus my last commits
(`CARGO_PROFILE_DEV_DEBUG=0 cargo test -j1 --lib setup`): **17 passed**.
The whole `cargo test -j1` in desktop/: 148 passed, 3 failed, all three in
`feat::create::tests` (desktop-create's work in progress, not touched
here).

| Test | Proves |
|---|---|
| `a_secret_is_asked_by_the_core_and_never_in_the_request_built_from_the_screen` | Token and custom secret dialogs have their secure field, the request body holds no value, and an intent that tries to pass `token` or `value` is refused by the registry |
| `the_standin_never_keeps_a_secret_value` | After setting a token, a custom secret and an MCP secret, the whole fixture data has none of the three values |
| `every_fixture_renders_every_tab_without_a_value` | 8 fixtures × 6 tabs render; no `ghp_` or `sk-` in any element's text, key or help |
| `every_approval_intent_answers_one_confirm_or_prepare` | The 14 approval intents each answer exactly one Confirm or Prepare |
| `a_stdio_server_shows_its_host_command_and_needs_the_acknowledgement` | `cd /srv && npx -y @mcp/fs /srv/my data`, the ack, quoting |
| `a_remote_server_asks_one_secret_per_placeholder` | `${TOKEN}`, `${ORG}` (deduplicated) → two fields; optional on an edit; the OAuth note |
| `a_kits_line_change_lists_what_it_adds_and_asks_for_kits_that_reach_out` | Adds/removes lines, diff, ack only for kits that reach out, defaults → `null`, unchanged / unknown file / `sbxw:jarvis` refused |
| `keeping_a_skill_uses_the_cores_listing_not_the_screens_words` | `origin` and `replace` come from the core's data |
| `a_new_host_is_confirmed_on_fingerprints_the_core_read` | Prepare reads the key; build shows and sends exactly those fingerprints; known key; keyscan failure; edit without key |
| `removing_a_host_with_sandboxes_needs_the_tick_and_forces` | |
| `every_setup_intent_is_described_in_the_screen_doc` | Each of the 27 intents is in `docs/screens.md`, `### setup` |
| others | rows unique and documented, routes and refusals, voice event, ✓ extra, stand-in voice steps, tab contents |

Headless PNGs in `.sbxw-artifacts/setup/` (`demo-setup.sh`, 760x820, each
looked at):

- `full-secrets.png`: tabs with Secrets selected; "Sign-in and services 2
  stored" (anthropic, github, every sandbox); GitHub "token set" with
  Replace token…, Use my gh login, Remove; Other services: OPENAI_KEY
  (Remove) and SCOPED_KEY "only web-app" (no Remove); Hosts and Variable
  fields; "The value is typed in the next dialog, drawn by sbxw." and Add….
- `full-kits.png`: "Waiting for you": docs, 1 kit, Answer in a pane; the
  file `/Users/ana/dev/web/sbxw.toml`, used by the daemon, web-app,
  web-api; 9 bundled kits, codemap checked, four `default` badges,
  descriptions cut with an ellipsis; the Jarvis kit path with its purple
  badge and Remove; the add field; Use defaults; Save… off (nothing changed).
- `errors-kits.png`: the same file with the red "Cannot be edited: not
  valid TOML: expected `=` at line 4"; every control greyed.
- `empty-kits.png`: the one-line explanation when no sbxw.toml is in use.
- `looked-skills.png`: /release-notes kept (from web-app, Remove); web-app
  picked, its three skills: /deploy Keep…, /release-notes Replace…, /Bad
  Name "name not kept".
- `looking-skills.png`: "Reading the sandbox's skills… (a stopped sandbox
  is started)".
- `looked-mcp.png`: linear (remote: Load, Unload, Sign in, Inspect, trash)
  and playwright (stdio: no Sign in); the inspect block for linear; the
  New server form (Remote).
- `looked-hosts.png`: build-box (green dot, trusted with secrets, ssh
  build-box · 38 ms · 2 sandboxes) and spare (grey, ana@10.0.0.12:2222 ·
  never); the provision block with `/usr/local/bin/sbxw host provision
  build-box`; the Add a host form with the alias mode and the fingerprint
  note.
- `errors-secrets.png`: the red "`sbx secret ls` failed: sbx is not
  running" line; GitHub "unknown" in amber, no buttons.
- `voice-downloading-voice.png`: STT installed, TTS "partial, 1150 MB",
  runtime "needs uv"; a blue bar at 45%, "Downloading voxtral-tts-4bit ·
  model.safetensors: 45% (1150 MB / 2550 MB)", Cancel.
- `voice-failed-voice.png`: "Last download failed: HTTP 503 from
  huggingface.co" in red, Install voice (~3.2 GB)…, Remove models….
- `empty-voice.png`: "not installed", sizes, folder and terminal command,
  Install voice.
- `no-voice-voice.png`: the amber "built without voice" line, no Install.
- the rest (`full-*`, `empty-*`, `errors-mcp`, `errors-hosts`) the same
  states for the other tabs.

One early run under load (load average 10, a debug build) showed the
screen's red banner "instruction budget exceeded (1 interrupts)": the old
50 ms wall clock per call ran out. desktop-shell has since made the
instruction count the hard limit (wall clock 1 s); the shots above were
taken again.

### The whole window (`demo-setup.sh window`, fixture `full`)

One scripted run, one PNG per step (1280x860), in `.sbxw-artifacts/setup/`:

- `window-opened.png`: the top bar's gear (`extra-topbar-setup`) opened
  Setup as a modal over the dimmed window, Secrets tab. The sidebar shows
  docs with the amber "✓ 1" (kit waiting).
- `window-token-typed.png`: "Replace the GitHub token", the core's dialog
  over the modal: its two lines, the label, and the secure field showing
  **only bullets** for the 33 typed characters; Cancel, Save.
- `window-token-saved.png`: dialog gone, notice "store the GitHub token:
  done", the tab read again.
- `window-mcp-acked.png`: "Register MCP server playwright2": the host line
  `npx -y @playwright/mcp@latest`, "I have read this command" ticked, Add
  now on (it is off until the tick).
- `window-mcp-added.png`: playwright2 listed (3 servers), notice "add
  playwright2: done", and the form started again (empty, Remote).
- `window-host-dialog.png`: "Add host gpu": the core read the key first
  (Prepare), then shows two fingerprints (ED25519, RSA) for 10.0.0.9 in a
  monospace block; "Add host" greyed until "These fingerprints are the
  host's" is ticked.
- `window-host-added.png`: notice "add host gpu: done" (the stand-in log:
  "host gpu saved (fingerprints confirmed: 2)"). The list still reads "No
  remote host yet": see the conflict with desktop-create below.
- `window-kits-dialog.png`: after ticking `sbxw:ssh-agent` and Save…:
  "Change the kits of sbxw.toml", the path, "Adds sbxw:ssh-agent
  (bundled).", who uses the file, the warning, the old line in red and the
  new one in green, Save greyed until "I know what sbxw:ssh-agent brings".
- `window-voice-progress.png`: after Remove models → Remove and Install →
  Download: the bar at 50%, "Downloading … 50% (1610 MB / 3220 MB)",
  Cancel; notices from the core.
- `window-kit-pane.png` (fixture: no terminal) and `standin-kit-pane.png`
  (against the stand-in): Setup closed, the ✓ on docs pressed
  (`extra-sandbox-docs-kit-add`): a pane "docs kit-add", `terminal
  119x40 connected` and a prompt; the stand-in logged `terminal opened
  (sandbox=docs&mode=kit-add)`.

Log order (app): `OpenModal setup`, `github_token_set`, `confirmed 'Replace
the GitHub token' (Post /api/github-token)`, `mcp_save`, `confirmed
'Register MCP server playwright2'`, `host_save`, `Post
/api/remote-hosts/key`, `confirmed 'Add host gpu' (Post
/api/remote-hosts/add)`, `voice_rm`, `voice_install`, `kits_save`,
`confirmed 'Change the kits of sbxw.toml'`, `CloseModal`, `OpenTerminal
docs KitAdd Spare`. The script then checks the typed token is in no app
log and no element tree (`--tree` dump at every step): "token in no log
and no element tree: ok".

### Found in the whole-window runs, for other threads

- desktop-create's stand-in answers `GET /api/remote-hosts` itself and
  expects the fixture key `remote_hosts` to be an array; mine is the
  route's answer `{ok, hosts, sbxw}`. create comes first, so under the
  stand-in and fixtures the Hosts tab lists nothing. The real daemon is
  unaffected. Reported to the lead.
- The stand-in binary answers `GET /api/voice` in the core (Jarvis's
  route: 404 unless started with voice), before feature routes, so under
  the binary the Voice tab gets no data. Fixtures (in memory) are fine.
  Reported.

## Not verified

- **The real daemon.** Only fixtures and the stand-in routes. Routes and
  bodies were read from `crates/daemon/src/http/ui.rs` and `svc/`; the
  shapes of `GET /api/secrets` rows for custom secrets (sbx 0.51 lists
  them under which name?) are a guess: the stand-in uses the variable name.
- **macOS**: not built; the dialog, secure fields and tabs were seen only
  headless on Linux.
- The Hosts list and Voice tab under the stand-in binary (the two
  conflicts above); both seen working from fixtures alone.
- A real keyboard in the secure fields (paste of a token with ⌘V).

## macOS test procedure

Prerequisites: the daemon running (`sbxw up`), the desktop app built from
this branch (`cd desktop && cargo run --release`).

1. Press the gear in the top bar. The Setup dialog opens on Secrets.
   Check the services listed match `sbx secret ls` (names only).
2. GitHub: press Set token… (or Replace token…). The core's dialog asks
   for the token in a bulleted field. Paste a test token with ⌘V, press
   Save. The badge turns "token set". Check `sbx secret ls` lists `github`,
   and that `~/.sbxw/desktop/` and the app's log (`RUST_LOG=info`) contain
   no part of the token.
3. If gh is installed: Use my gh login → Use gh. Then Remove → Remove: the
   badge turns "no token" (put your token back afterwards).
4. Other services: hosts `api.example.com`, variable `EXAMPLE_KEY`, Add…,
   type a value, Save. The row appears ("every sandbox"). Remove it.
5. Kits: pick a project's sbxw.toml, tick `sbxw:k8s-tools`, Save…: the
   dialog lists "Adds sbxw:k8s-tools (bundled)" and the diff; Save. Open
   the file: only the `kits` line changed. Tick `sbxw:ssh-agent`: Save…
   is held until "I know what sbxw:ssh-agent brings" is ticked. Cancel,
   then Use defaults → Save: the line is gone.
6. If a sandbox shows the amber ✓ (a kit waiting): press it or "Answer in
   a pane": a pane opens on `sbx kit add`; answer there.
7. Skills: pick a running sandbox with a skill under `.claude/skills/`,
   Look, Keep… → Keep. It appears under Your skills. Remove → Remove.
8. MCP: add a remote server (name `test`, URL
   `https://example.com/mcp`, header `Authorization: Bearer ${TOKEN}`):
   the dialog asks `${TOKEN}`. Add. Check `sbx mcp ls`. Then a local
   server (command `echo`, arguments `hi`): the dialog shows `echo hi` and
   Add is off until "I have read this command" is ticked. Remove both.
9. Hosts (needs an SSH host you can reach): Add a host by alias. The
   dialog first says it reads the key, then shows the fingerprints;
   compare with `ssh-keygen -lf <(ssh-keyscan host)`; tick, Add host.
   Test: a notice with the sbx version. Remove → tick → Remove.
10. Voice: on a Mac without the models, Install voice… → Download: the bar
    moves; Cancel stops it; Install again resumes. Remove models… →
    Remove. Load the models now does nothing visible (warms them).
11. Press Escape, a click outside, or ✕: the dialog closes. Reopen it: it
    shows the tab used last (kept for the session, not across restarts as
    the web UI's localStorage does).
12. Stop sbx (`sbx daemon stop`) with Setup open on Secrets, press the
    reload icon: a red line gives the daemon's error; start sbx, reload:
    it goes.
