# The daemon's API, as the desktop app uses it

The app talks to the sbxw daemon only through `desktop/src/client.rs` (the
`Daemon` trait), over the HTTP and WebSocket API the web UI uses, on the UI
listener (default `127.0.0.1:7681`). Routes are read from
`crates/daemon/src/http/` (`ui.rs`, `mod.rs`, `stream.rs`, `ws.rs`,
`board.rs`, `jarvis.rs`, `mac.rs`, `browser.rs`). The daemon changed for the
app in a few places, each additive and opt-in or harmless to the web UI: the
terminal's `size=1` frames and one PTY size for all clients, real bells only
(`attention` no longer raised by an OSC's BEL), `GET /api/stats`, and the
`emphasis` configuration. This file is a reference kept in git
(`desktop/docs/`); correct it when a route changes.

## Connecting

| Step | How |
|---|---|
| Address | `--daemon`, else `$SBXW_DAEMON`, else `127.0.0.1:7681` (`DEFAULT_WEB_ADDR`, `crates/core/src/config.rs`). A `web_addr` set in `sbxw.toml` is not read: pass `--daemon` |
| Token | `--token`, `--token-file`, else `~/.sbxw/run/ui-token` (`$HOME/.sbxw`, as `crates/daemon/src/store.rs`). The daemon mints a new one at every start, so the file is read again on every reconnect |
| Proof | `GET /api/health?nonce=<32 hex>` must answer `proof = sha256(token ‖ nonce)` before any request carries the token (`guard.rs`, `token.rs`). A process on the port that cannot prove it never sees the token |
| Requests | `Authorization: Bearer <token>`, no `Origin`, `Host` = the address (allowed by `allowed_hosts`) |
| Methods | `GET` and `POST` (`Daemon::request`), and `DELETE` for one route only: Sessions' `DELETE /api/sessions/:sandbox/:session_id/history/subscription`, sent directly because it removes nothing the person made. Any other `DELETE` a feature sends goes through the core's dialog (`Confirm`), as every destructive call |
| Down | Retries every 0.25 s, doubling to 3 s; the window shows one line (`No sbxw daemon at 127.0.0.1:7681. Start it with \`sbxw up\`; retrying.`) |
| Restart | The event stream ends; the link reconnects (new token from the file), reads the list again, and panes whose terminal closed open again (the daemon keeps the PTY and replays its last 256 KB) |

## Live updates

There is no "sandbox list changed" event on the bus (`crates/daemon/src/bus.rs`).
The web UI reads `/api/sandboxes` again after its own actions and after some
events. The app does the same:

- after its own stop (`Link::run`, then a read);
- 300 ms after any `session`, `provision`, `lead`, `lead-end` or `codemap`
  event on `/api/stream` (a burst makes one read; `client::REFRESH_ON`);
  features read their own keys again on the events they name (`feat::Data::on`);
- on `thread-closed` `{sandbox, thread}` (a thread closed for good, by either
  client, a plan merge, or the chat dock): the threads feature closes that
  thread's panes (on every page, and the ones Lead Mode parks), reads
  `threads` and `sessions` again, and says so in a notice unless this app
  asked for the close. A thread the app never showed is ignored; a pane
  closed alone sends nothing;
- after opening the agent of a sandbox that is not running: at 1.5, 3, 6, 12
  and 24 s until it shows as running (`RUNNING_RECHECK_MS` in
  `assets/js/sandboxes.js`);
- every 15 s while connected, which the web UI does not do: a change `sbx`
  makes behind the daemon (a CLI `sbxw stop`) otherwise shows nowhere.

**Side effect:** each `/api/stream` connection counts as one open tab in the
daemon (`bus.clients`). With the desktop app and one browser tab open, the web
UI shows its "several tabs" warning.

## Routes

"Covered" means a `Daemon` method calls it. Adding one: a method on the
trait, its body on `HttpDaemon` with `self.get(path)` or
`self.post(path, &body)`, a body on `FixtureDaemon` reading the fixture's key,
and its row here.

### Covered in wave 1

| Route | `Daemon` method | Notes |
|---|---|---|
| `GET /api/health?nonce=` | `HttpDaemon::connect` | the proof |
| `GET /api/sandboxes` | `sandboxes()` | array of `SandboxItem`; unknown fields kept |
| `POST /api/sandboxes/:name/stop` | `stop_sandbox()` | |
| `GET /api/stream` | `subscribe()` | SSE; every event passed on as `LinkEvent::Bus` (wave 1 uses them only to refresh the list) |
| `GET /ws?sandbox=&mode=&thread=` | `open_terminal()` | binary PTY frames, JSON `resize` (the size this client wants: it becomes the owner of the PTY's one size), text error lines, `{"type":"attention"}` (a real BEL, not an OSC terminator); with `size=1`, `{"type":"size","cols","rows","owner"}` before the replay and on every change (TERMINAL.md); `mode` absent is the agent (starts a stopped sandbox). A thread's agent is `mode=thread&thread=<t>` (`SessionKind::from_query`, `crates/core/src/session.rs`: without `mode=thread` the daemon opens the MAIN agent); its first open also sends `repo=` (may be empty: the workspace), `model=`, `checkout=1`, never sent again on a reconnect (phase 4). Its shell is `mode=bash&thread=<t>` |

### Covered in phase 2 (Jarvis, thread desktop-jarvis)

| Route | `Daemon` method | Notes |
|---|---|---|
| `GET /api/jarvis/status` | `jarvis_status()` | `{state: absent\|creating\|starting\|running\|stopped\|failed, error?}` (`svc/jarvis_sandbox.rs`, `Status`). Read on connect, then every 1.5 s while `creating` or `starting`, and after a `session` or `provision` event while not running |
| `POST /api/jarvis/start` | `jarvis_start()` | same shape, answered at once; the work runs in the daemon's background |
| `GET /api/jarvis` | `jarvis_proposals()` | `{ok, proposals: [ui_view, newest first]}`; `ui_view` = `{id, summary, risk: low\|high, state: pending\|applied\|refused\|failed, created, changes: [{kind, title, diff, risk, host_command?, intents?}], needs_ack?, results?, error?, via?, reason?, settled?}` (`crates/core/src/jarvis.rs`, `Proposal::ui_view`) |
| `POST /api/jarvis/proposals/:id/approve` | `jarvis_approve(id, via, ack)` | body `{via: "click"\|"voice", ack_host_command}`; answer `{ok, proposal}`. 409 (`client::HttpError`, code and message): voice on a high-risk one, a host command not acknowledged, already answered. A file changed since the proposal is `200` with `state: failed` and `error` |
| `POST /api/jarvis/proposals/:id/refuse` | `jarvis_refuse(id, reason)` | body `{reason?}`; answer `{ok, proposal}` |
| bus event `jarvis` | `LinkEvent::Bus` | data: one proposal's `ui_view` (`bus.rs`). The app also takes `{proposals: [...]}` or nothing (then reads the list), as jarvis.js does |
| `GET /api/sessions` | `sessions()` | every `SessionInfo`; the app reads Jarvis's (`sandbox: "sbxw-jarvis"`, no `thread`) `last_input` and `reply` as the conversation's first look |
| bus event `session` | `LinkEvent::Bus` | a `SessionInfo`. Jarvis's answer = `reply` when its `state` goes from `working` or `attention` to `idle` (chat-core.js, `chatIngest`) |
| `POST /api/voice/anonymise` | `anonymise(text)` | `{text}` → `{ok, text, masked}`; every message is masked before it leaves, as the web UI does |
| `POST /api/chat/push` | `chat_push(name, text)` | `{text, name: "sbxw-jarvis"}`: types the masked text into Jarvis's agent session (concierge.js, `vcDraft`) |
| `GET /api/voice` | `voice_status()` | `{compiled, ready, speech: {supported, ...}, push_to_talk_key, ...}`; 404 or an error = no voice. Bus event `voice` carries the same |
| `POST /api/voice/transcribe` | `transcribe(pcm)` | body: 16 kHz mono f32 LE PCM, at most 120 s, `application/octet-stream`; answer `{ok, text, masked, audio_s, ms}` (text already masked) |
| `POST /api/voice/speak` | `speak(text)` | `{text}`, 1 to 1000 characters; answer: s16 LE mono PCM streamed, rate in `x-sample-rate` (24000). Longer answers are cut at sentences and chained in one reader (`voice::Chain`) |
| `POST /api/voice/cancel` | `voice_cancel()` | stops a model download; not called by the panel yet |

Gone on the daemon: `/api/voice/chat` and `/api/voice/conversations` answer
404 (no language model in the daemon; `http/tests.rs`). Words go to Jarvis
through `/api/chat/push` like any agent's message.

### Covered in phase 3 (features, `src/feat/`)

Features call the daemon through one generic method, `Daemon::request(method,
path, body, timeout)` (`HttpDaemon`: the JSON body of any status, non-2xx
included; `FixtureDaemon`: the features' stand-in routes, then their data
keys), run off the UI thread by `Link::call` and `Link::fetch`. A feature
declares its reads (`feat::Data`) and its calls (`feat::Call`); the core does
the rest (`shell/features.rs`). A POST through a feature also reads the
sandbox list again, as a stop does.

| Area (feature, report) | Routes | Risk Approval (core dialog) |
|---|---|---|
| Network (`feat/network.rs`, NETWORK.md) | `GET /api/approvals` (every 5 s, after `session`), `POST /api/approvals/respond`, `GET /api/sandboxes/:name/policy`, `POST .../policy/rules`, `POST .../policy/rules/rm`, `POST /api/policy/refresh`, `GET /api/sandboxes/:name/ports`, `POST .../ports/publish`, `POST .../ports/unpublish`, `GET /api/hosts` | every answer to a request, an allow rule, removing a deny rule, a fixed host port |
| Shell extras (`feat/shell_extras.rs`, NETWORK.md) | `POST /api/sandboxes/:name/rm`, `/recreate`, `/duplicate`, `/detach`, `GET /api/usage` (every 30 s), `GET /api/state` (versions, at connection; also `terminal_colors: {background, bar, text}`, sbxw.toml's `terminal_color`, `terminal_bar_color` and `terminal_text_color` as written, empty when unset, which the terminal colours picker falls back to: `shell/term_colors.rs`; a daemon without the field gives the built-in) | remove, recreate |
| Create (`feat/create/`, CREATE.md) | `POST /api/sandboxes/create` (409 consent answered with `approved` lines through `Confirm::grant_lines`), `POST /api/sandboxes/chat`, `POST /api/carve/preview`, `GET /api/fs`, `POST /api/fs/pick`, `GET`/`POST /api/fs/favourites`, `GET /api/kit-mounts`, `GET /api/template/status`, `GET /api/remote-hosts`, `POST /api/remote-hosts/:host/clone`; bus event `provision` | grants a sandbox would get beyond its workspace |
| Setup (`feat/setup/`, SETUP.md) | `GET /api/secrets`, `POST /api/secrets/custom`, `/rm`, `GET /api/github-token`, `POST /api/github-token`, `/rm`, `/gh`, `GET`/`POST /api/kits`, `GET /api/my-skills`, `POST /api/my-skills/rm`, `GET /api/sandboxes/:name/skills`, `POST .../skills/keep`, `GET`/`POST /api/mcp`, `GET /api/mcp/:server`, `POST /api/mcp/:server/rm`, `/auth`, `POST /api/sandboxes/:name/mcp/load`, `/unload`, `GET /api/remote-hosts`, `POST /api/remote-hosts/key`, `/add`, `/reload`, `POST /api/remote-hosts/:host/edit`, `/rm`, `/test`, `GET /api/voice`, `POST /api/voice/install`, `/rm`, `/warm`, `/cancel`; bus event `voice` | secrets and tokens (core-drawn secure fields), kits that reach out, a stdio MCP server (acknowledged command), a new remote host (fingerprints read by the core) |
| Core slots | none (open/close modal and panels, extras) | |

### Covered in phase 4 (threads, relay, integrator)

| Area (feature, report) | Routes | Risk Approval (core dialog) |
|---|---|---|
| Dashboard, its conversation (`feat/board/`, BOARD.md) | `GET /api/teams`, `GET /api/teams/:id/timeline?limit=&before=&group=&thread=&q=`, `GET /api/sandboxes/:name/commits/:hash` (a commit's diff); the `team` stream event | read only |
| Threads and plans (`feat/threads/`, its report) | `GET /api/sessions` (data key `sessions`, after `session`; also read by relay and once by Jarvis at connection), `GET /api/sandboxes/:name/threads`, `/repos`, `/worktrees`, `/board`, `/commits`, `/commits/:hash` (the Commits and Worktrees views, read-only), `GET .../threads/:thread`, `POST .../threads/:thread/close`, `POST /api/sandboxes/:name/lead`, `/lead/end`, `GET /api/leads`, `GET /api/plans`, `POST /api/plans/:id/approve`, `/refuse`, `/merge`, `/merge/abandon`, `GET /api/models`, `POST /api/input`, `GET`/`POST /api/sandboxes/:name/remote-control`, `.../remote-control/login`; `/ws` with `mode=thread` | see its report (approving and merging a plan) |
| Relay (`feat/relay/`, its report) | `GET /api/relay` (after `relay`), `POST /api/relay/:id/route`, `/deny`, `/approve`, `POST /api/answer`, `POST /api/chat/push` to another sandbox, `POST /api/voice/anonymise` | routing and releasing a request (see its report) |
| Templates (`feat/templates.rs`) | `GET /api/templates` (`{ok, templates: [{tag, size, created, configured, auto}], configured}`), `POST /api/templates/save` (`{sandbox, tag}`, up to 30 min), `POST /api/templates/rm` (`{tag}`), `GET /api/disk-usage[?verbose=true]` (`{ok, text}`: `sbx df` as printed; an older sbx: "unknown command") | removing a template |
| Env file (`feat/envfile.rs`) | `POST /api/sandboxes/:name/envfile` (`{dir, save, force}` → `{ok, yaml, dest, dir, written}`; 409 when the file exists and `force` is false) | none (as the web UI) |
| Core | none: `copy_text`, `close_panes`, global shortcuts, the app's state file (`~/.sbxw/desktop/state.json`), stale override detection, `screen_restore` (deletes `~/.sbxw/desktop/screens/<name>.luau`, a core operation behind the dialog) | restoring a built-in screen |

A per-sandbox data key's failed read is `data.app.errors[key][sandbox]` (the
daemon's words), the sandbox keeping its last good value; a plain key's is a
string (phase 4).

`GET /api/approvals` is `sbx policy approval ls --format json` unfiltered:
the whole machine's requests, those of sandboxes sbxw does not list too
(removed ones, `sbxw-jarvis`). The app counts and shows only those of listed
sandboxes, as the web UI does (`svc/sandboxes.rs`, `pending_for`); a
`network` request is a connection already refused (quiet), a `port` one a kit
waiting (amber).

### Covered in phase 4, second wave (files, browser, Mac, integrator)

| Area (feature, report) | Routes | Risk Approval (core dialog) |
|---|---|---|
| Core: feature sockets (`src/socket.rs`) | any `GET /ws/<route>?<query>` a feature opens (`Action::OpenSocket`), `Authorization: Bearer` as `/ws`; text and binary frames both ways; reconnect optional (new token after a daemon restart) | none (a feature's own) |
| Core: raw reads and uploads (`Call::raw`, `Call::post_bytes`, `src/files.rs`) | a feature's `GET` read as bytes (at most `max + 1`); `POST` with a raw body of known length (`Content-Length`, streamed from a file the person handed in, 128 MB at most) | uploads: always from a `Confirm` |
| Core: downloads, open, reveal (`src/files.rs`, `src/safe.rs`) | a feature's `GET` route written to the Downloads folder or a place the person picks (save dialog) under a sanitised name, never over a file; opened with the system's default app by the core's verdict | choosing the place, opening: from a `Confirm` |
| Files: a whole folder (`feat/files/`) | `GET /api/sandboxes/:name/fs/folder/size?path=` (`{path, files, bytes, max}`, counted without reading), `GET /api/sandboxes/:name/fs/folder?path=&max=` (`<folder>.tar.gz`, `max` 128 MB from the app, 413 past it with the size and the cap), saved through the core's download | Save as: from a `Confirm` |

### Covered since (status bar, keyword pills)

| Area (where) | Routes | Notes |
|---|---|---|
| Status bar (`src/shell/statusbar.rs`, STATUSBAR.md) | `GET /api/stats`, bus event `stats` (every 5 s) | each running sandbox's CPU and memory (nulls where no recent report), the daemon's own, the total; `GET /api/sandboxes` items carry `cost_usd` (the sessions' cost so far), and `GET /api/sessions` items a `model` |
| Top bar usage | `GET /api/usage` (every 30 s) | `five_hour_pct`, `seven_day_pct` |
| Keyword pills (`src/feat/emphasis.rs`, EMPHASIS.md) | `GET /api/emphasis`, bus event `emphasis` (on every change and after every sandbox bring-up) | `{enabled, guidance, unknown_tone, tones, max_word, max_detail, families: [{word, tone, also, use}], words: {word: tone}, sandboxes: {ref: bool}}`; `sandboxes` is the effective switch per sandbox, a sandbox not listed follows `enabled`. Read only: the switch changes through Jarvis |

The terminal (`/ws`, above) with `size=1`: `{"type":"size","cols","rows","owner"}`
before the replay and on every change; the grid takes the PTY's size and is
drawn smaller when another window owns a larger one (TERMINAL.md).

### Message input autocompletion (thread gui-composer)

| Route | Notes |
|---|---|
| `GET /api/sandboxes/:name/commands` | `{ok, commands: [{name, description, kind: "skill"\|"command", origin: "workspace"\|"home"\|"built-in"}], complete, reason?}`. Read-only, UI listener only. `name` matches `^[a-z0-9][a-z0-9:_-]{0,63}$`, without the `/`; `description` is one line of visible text, at most 160 characters, empty when the file has none. Sorted by name, one entry per name: workspace over home over built-in, a skill over a command. At most 300 entries. **Never starts a sandbox**: a stopped one (or one whose read failed or took over 5 s) answers from its workspace on the host, `complete: false` and a plain `reason` to show muted. `built-in` is a short fixed list (`clear`, `compact`, `context`, `model`), always present |

### Covered for sbx 0.54 (features 1, 2, 3, 5, 6)

The contract is `.sbxw-artifacts/reports/sbx-054-features-api.md`. An sbx
older than 0.54 answers `supported: false` and a `note`; the app shows that
note muted ("needs sbx 0.54"), never an error. A daemon without a route
(404) is treated the same way, or the control is absent. Every string from
sbx (`raw`, `message`, `rule`, server names) is set as text.

| Route | `Daemon` method (feature) | Notes |
|---|---|---|
| `POST /api/sandboxes/:name/ports/publish` (changed) | `request` (network, `port_publish`) | `host_port` left out: sbx 0.54 picks, on 127.0.0.1 only (a `host_ip` other than 127.0.0.1 without `host_port` is refused, nothing run: the Network screen says so as text and asks nothing). Answer adds `sandbox_port, host_ip, host_port, assigned, note, forwarded_port`; kept as `data.network_publish`, the row of that port says "sbx picked" |
| `GET /api/sandboxes/:name/policy/check?target=<host>[:<port>]` | `request` (network, `network_why`) | `{supported, target, allowed (true/false/null), rule, rule_id, raw, note}`; the target is checked in the app first (host and optional port). Removing the rule named is the existing `.../policy/rules/rm`, from the rules list's own button |
| `GET /api/sandboxes/:name/mcp` | data key `mcp_mode` (mcp, lazy, by sandbox) | `{supported, mode: static\|dynamic\|unknown, servers, note}`; read when the panel opens, when it moves to a sandbox not read yet, after a load or unload; never on a timer |
| `GET /api/ssh` | data key `ssh` (ssh) | `{configured, refresh_due, sbx, refreshed_at}`; Setup's card. `GET /api/state` also carries `ssh_refresh_due` (`daemon_state`), which turns Settings amber |
| `POST /api/ssh/setup` | `request` (ssh, `ssh_setup`) | `{}` → the same fields as `GET /api/ssh`; 502 with sbx's words. A person's press only |
| bus event `provision`, `kind: "denied"` | create (`on_event`) | `{sandbox, step: create\|kits, kit, hosts, source: sbx\|policy_log, exists, allow_scope: sandbox\|global, message}`, just before the failure it explains. Kept in `data.create.denied` until Retry (`create_retry`: the same `POST /api/sandboxes/create`, which adopts the existing sandbox) or Dismiss. Allowing is the existing `POST .../policy/rules` through `network_allow` (`global: true` when `allow_scope` is `global`) |

### Remote access (admin, not covered yet by a screen)

The Mac's side of [remote access](../../docs/security.md#remote-access), on the UI listener with the
UI token. The contract is `.sbxw-artifacts/remote/API.md` section 5; the CLI
(`sbxw remote`) uses these today.

| Route | Answer |
|---|---|
| `GET /api/remote` | `{enabled, remote_addr, listening, tailnet: {state, name, serving, login_url, detail} \| null, devices, pending_requests, keep_awake: "holding"\|"on_battery"\|"not_macos"\|"off", sidecar_missing: <path> \| null, leaders: [<sandbox>]}`. `sidecar_missing` is the path the daemon looked at when `sbxw-tailnet` is not there (enable then fails with a sentence naming it and the build command); `leaders` are the names of the team leaders a paired phone sees now (the same list as `GET /r/leaders`, names only) |
| `POST /api/remote/enable` / `disable` `{}` | `{ok, status}`; both idempotent. Enable is `409` with a sentence when the listener or the sidecar cannot start. A screen must ask the person first, saying what it exposes |
| `POST /api/remote/pairing` `{}` | `{ok, code, code_id, url, expires_ms}`; `409` when off or the tailnet is not serving. Show `url` as a QR code and a link. A new code voids every pending request |
| `GET /api/remote/pairing/requests` | `[{id, code_id, device_name, created_ms, expires_ms}]`: show only the request whose `code_id` is the code on screen, with its `device_name`. No digits: the person reads them on the phone |
| `POST /api/remote/pairing/requests/:id/approve` `{check}` | `{ok}` when `check` is the six digits the person typed from the phone; `403` "N tries left", the third wrong try refuses; `404` unknown, expired or voided. This is the confirmation on the Mac: a core-drawn field, never from agent text |
| `POST /api/remote/pairing/requests/:id/refuse` `{}` | `{ok}`; `404` unknown |
| `GET /api/remote/devices` | `[{id, name, created_ms, last_seen_ms}]` |
| `DELETE /api/remote/devices/:id` | `{ok}`; `404` unknown. Immediate |
| `GET /api/remote/journal?limit=` | `[{at_ms, device_id, device_name, kind, size?}]`, newest last; never text. `kind` `upload` (a file a phone handed to a leader) carries `size`: `under_1mb`, `1_to_10mb` or `over_10mb`; never a file name |
| `POST /api/remote/tailnet/forget` `{}` | `{ok}`; `409` while on |

No bus event yet: poll `GET /api/remote` (every few seconds while a pairing is shown).

### Canvas (`src/feat/canvas/`, CANVAS.md)

UI listener, UI token. Ids are `c-` and 16 hex digits; picture files are
names the daemon generated (`s-<12 hex>.png`, `a-<12 hex>.jpg`), served only
when the manifest lists them. Every answer is `{ok, ...}`.

| Route | Body or query | Answer |
|---|---|---|
| `GET /api/canvas` | | `canvases` (newest first: `id, sandbox, title, created_ms, updated_ms, shots, rounds, reviews, bytes, draft, diagrams, draft_diagrams`), `total_bytes`, `max_canvas_bytes` |
| `POST /api/canvas` | `{sandbox, title}` | `canvas` (a new manifest) |
| `GET /api/canvas/:id` | | `canvas` (the manifest: `shots, assets, pins, notes, rounds, reviews, diagrams`), `bytes`, `diagram_bytes`. A shot has `moved` (placed by the person) and `cell` (`[row, column, slot]` in its round's grid); a round `cols`, `cell_w`, `slot_w`, `diagrams`; a review `diagrams` (how many changed diagrams it sent). `diagrams` is absent when there is none; each is `{id, title, kind, round, added_ms, x, y, w, h, moved, bytes, diagram}`: a node at `(n.x, n.y)` sits at `(x + n.x, y + n.y)` on the surface, `w, h` are the content's far corner. `diagram` is the current version (sbxw's schema, with `rev`). Manifest `version` 2 with diagrams, 1 without |
| `GET /api/canvas/:id/diagrams/:did` | | `diagram` (one item as in the manifest); 404 for an id the canvas does not hold |
| `POST /api/canvas/:id/diagrams/:did` | `{rev, diagram}`: `rev` the revision the editor started from, `diagram` the whole new version in sbxw's schema (unknown fields refused; nodes the person placed carry `placed: "person"`; nodes without `x, y` are laid out) | `diagram` (the item saved), `rev` (the new revision). 409 when the diagram moved past `rev` (read it again), 400 with the reason when it fails a check |
| `GET /api/canvas/:id/images/:file` | | the picture's bytes, its type (PNG, JPEG, WebP), `nosniff`; 404 for a name the manifest does not list |
| `POST /api/canvas/:id/rename` | `{title}` | `title` |
| `POST /api/canvas/:id/edit` | `{pins?, notes?, remove_assets, layout: [{id, x, y, w, h}]}`: the unsent pins and notes whole; sent ones never change. A diagram's id in `layout` moves it (`x, y` only) | `updated_ms, draft` |
| `POST /api/canvas/:id/assets?name=&x=&y=` | the picture's bytes (20 MB at most; checked by content) | `asset` |
| `POST /api/canvas/:id/submit` | `{session, assets: {asset id: path in the sandbox}}` (paths under `/tmp/sbxw-uploads/` only). Takes the diagrams the person changed too; a round of diagram changes only is allowed | `review` (its number), `line` (the fixed line the app types) |
| `POST /api/canvas/delete` | `{ids}` and/or `{before_ms}` | `deleted, bytes` |
| bus event `canvas` | | `{id, sandbox, what}`: `created`, `shots` (the agent added some), `diagram` (the agent added or replaced one), `asset`, `edit`, `review`, `read`, `renamed`, `deleted` |

Agent listener (the sandbox is the token's): `POST /api/canvas/shots`
(`{canvas?, title?, batch, shot: {b64, title, note?, width?, screen?}}`),
`POST /api/canvas/list`, `POST /api/canvas/read` (`{canvas?}`: `text`,
`diagrams: [{canvas, id, title, rev, lines, json}]`), `POST /api/canvas/diagrams`
(`{canvas?, batch, diagram: {json, title?, kind?, replace?}}`: `canvas,
canvas_title, diagram, title, kind, rev, round, nodes, edges, replaced`).

### Tickets and the Kanban (`src/feat/kanban/`, KANBAN.md)

UI listener, UI token. A ticket is `T-<n>` per sandbox (`:n` takes `12` or
`T-12`); file ids are `a-` and 8 hex digits, the files on disk names the
daemon generated. The column of every card, its labels, marks and counts are
the daemon's (`sbxw_core::tickets`): the app lays them out. Every answer is
`{ok, ...}`; an error is `{ok: false, error}` with 400, 404 or 409.

| Route | Body or query | Answer |
|---|---|---|
| `GET /api/tickets` | `?sandbox=<name>` (`all` or none: every sandbox), `&older=1` (Merged in full) | `board`: `scope, now, columns: [{id (planned, progress, test, ready, merged), title, tone, help, empty, count, cards, older}], counts_line, tickets, bytes, limits` |
| `GET /api/tickets/:sandbox/:n` | | `ticket`: `card, n, num, title, description, priority, source, created_ms, updated_ms, validated_ms, link {plan, thread}, files [{id, name, bytes, size, kind, picture, icon, added_ms}], files_label, bytes, history [{at, when, text, tone}]` |
| `POST /api/tickets/:sandbox` | `{title, description?, priority? (none, p1, p2, p3)}` | `ticket` |
| `POST /api/tickets/:sandbox/:n/edit` | `{title?, description?, priority?, validated?}` (over a cap: refused) | `ticket` |
| `POST /api/tickets/:sandbox/:n/files?name=` | the file's bytes (20 MB; archives and programs refused by content) | `ticket` |
| `GET /api/tickets/:sandbox/:n/files/:file` | | the bytes, the type sniffed (text as `text/plain`), `nosniff`, `Content-Security-Policy: sandbox`, `attachment` |
| `POST /api/tickets/:sandbox/:n/files/:file/remove` | | `ticket` |
| `POST /api/tickets/:sandbox/:n/move` | `{to: up, down, top, bottom}` (Planned, no thread) | `{}` |
| `POST /api/tickets/:sandbox/:n/delete` | | `deleted, files, bytes`; 400 while its thread works |
| `POST /api/tickets/:sandbox/:n/send` | (nothing read from the body) | `sent` (`typed` or `held`), `line`; 409 when no session is open or the ticket is in a plan |
| bus event `tickets` | | `{sandbox, n?, what}`: `created`, `edited`, `file`, `moved`, `deleted`, `followed` (a plan changed), `sent` |

A card: `key` (`t:<sandbox>:<n>` or `p:<plan>:<thread>`), `sandbox`, `n`
(null for a thread with no ticket), `num` (`T-12` or `thread`), `column`,
`title`, `title_short` (64 characters), `title_long` (110), `source`
(`person`, `thread`), `priority`, `thread`, `thread_state`, `thread_tone`,
`plan`, `marks [{id, label, tone, help}]`, `files`, `bytes`, `validated`,
`created_ms`, `updated_ms`, `age`, `can {reorder, validate, edit, delete,
send, reorder_why, delete_why, send_why}`.

Agent listener (the sandbox is the token's, read only): `POST
/api/tickets/list` (`text`), `POST /api/tickets/read {id}` (`text`; the
files copied into the sandbox on this read). `POST /api/plans/propose` and
`/api/plans/wait` answer `ticket_notes` (each ticket a thread names that is
not linked, and why).

### Not covered yet

Grouped by the area that will need them, with the web UI file that calls
each group, so a later thread can read what it expects.

| Area (web UI file) | Routes |
|---|---|
| Session history, for Sessions (the feature `monitor`; no web UI caller; docs/session-history.md) | `GET /api/sessions/:sandbox/:session_id/history?before=&limit=` (newest page first; reading subscribes for 120 s), `DELETE /api/sessions/:sandbox/:session_id/history/subscription`, bus event `history` `{sandbox, session_id, seq}` (a nudge without text, only while subscribed); `GET /api/sessions` items gain `ended`, `end_reason`, `continues`, `last_input_from`, `last_input_sender` |
| Sessions and agents (`main.js`, `panes.js`) | `GET /api/ptys`, `GET /api/hook/log`, `POST /api/focus`, `POST /api/watching`, `GET /api/events` (sessions only), `GET /api/watch-events`, `POST /api/sandboxes/:name/up` |
| Relay events (`relay.js`) | `GET /api/relay/events` (the app reads `GET /api/relay` after the bus event `relay` instead) |
| SSH card (`ssh.js`) | its connection details popover (no route; `GET /api/ssh` and `POST /api/ssh/setup` are covered above) |
| Header leftovers (`main.js`) | `POST /api/initial`, `POST /api/shutdown` |
| Browser pairing (`browser.js`) | `GET /api/embed/resolve`, `GET`/`POST /api/embed/pairings`, `POST /api/embed/pairings/rm` |

Covered since this table was first written (phase 4, second wave): the files,
artifacts and uploads routes, the code map, the browser panel (`/ws/browser`,
`browser/allow`, `grants`, `settle`, `GET /api/browser/active`) and the virtual
Mac (`GET /api/mac` and its actions); `POST /api/input`, `POST /api/answer` and
`GET /api/models` (threads, relay).

Routes a client never calls: `GET /` and `/sidepanel` (pages), and the
agent-listener routes the UI listener answers with "moved" (`/api/hook`,
`/api/relay/ask`, ...). The agent listener (sandboxes calling in with their
own token: `/api/relay/*`, `/api/plans/*`, `/api/board/*`,
`/api/jarvis/state|propose|wait`, `/api/mac/*` actions, `/api/browser/state`)
is not for the desktop app.

**Sensitive routes.** Every route that answers a question for the person
or widens access is called only from a `feat::Confirm` the core draws
(tables above): approving and merging a plan, routing and releasing a relay
request, deleting a file, the browser's allow, uploads.

## The stand-in daemon

`desktop/src/bin/standin.rs` serves the covered routes from a fixture
(default `fixtures/sandboxes/basic.json`) and a local shell for `/ws`, so
terminals and live updates can be tested with no sbx. It changes states as
the daemon roughly does (opening the agent of a stopped sandbox makes it
running 0.5 s later; a stop takes 0.3 s) and sends a `session` event after
each change. `--change SECS:NAME=STATUS` changes a sandbox behind the app's
back; `--token-file` writes the token like the daemon writes `ui-token`.

Phase 3: every other `/api/` route goes to the features' stand-in routes
(`feat::answer_later`: each `Feature::standin`, else a `GET` of a declared
data path answers the fixture's key), with the fixture's whole `daemon`
object as their data. Events a route publishes go out with the lock
released; `publish_after` events are played before the answer. A route that
changes the list (a removal, a creation) makes the list follow and sends a
`session` event. `--approval SECS` makes sbx ask (web-app wants
api.github.com) that long after the first stream client; `GET /api/voice`
answers the fixture's `voice` key when the stand-in has no voice of its own.

Simulations for headless demos: `POST /api/standin/other-client` with
`{cols, rows, type}` makes "another window" take every open terminal's size
(and type into it); `POST /api/standin/emphasis` with `{"variant": "custom" |
"quiet" | "off"}` or a whole payload replaces `GET /api/emphasis` and
publishes its `emphasis` event; `--stats full|partial|none` serves
`GET /api/stats` and its event; `--approval SECS`, `--jarvis`, `--voice`,
`--clients N` as the file's header says.

sbx 0.54 fixture keys: `standin_sbx_old` (true: answer as an sbx before
0.54: publish on the sandbox port with a note, `policy/check` and the MCP
mode unsupported), `policy_check[sandbox][target]` (a fixed check answer),
`mcp_mode_of[sandbox]` (the MCP mode; a load into a dynamic sandbox
connects the server), `ssh` and `standin_ssh_fail` (the SSH card),
`create_denied[name]` (`{hosts, step, kit}`: sbx denies those hosts at the
kits step until an allow rule of the stand-in's policy lets each through).

Phase 4, second wave: a `GET /ws/<route>` upgrade goes to the stand-in's own
`/ws/echo` (tests of `src/socket.rs`: a hello with the connection's number,
every text message back, `frames K`, `bye`) or to the first feature whose
`standin_ws` takes the target, on the connection's own thread. A feature
route may answer raw bytes with `{"$raw": text, "$type": ...}` or
`{"$raw_hex": ..., "$type": ...}`; a raw request body (an upload) reaches the
feature's stand-in as `{"$bytes": length, "$type": content type}`.
