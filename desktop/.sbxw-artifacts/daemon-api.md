# The daemon's API, as the desktop app uses it

The app talks to the sbxw daemon only through `desktop/src/client.rs` (the
`Daemon` trait), over the HTTP and WebSocket API the web UI uses, on the UI
listener (default `127.0.0.1:7681`). Nothing in the daemon was changed for it.
Routes were read from `crates/daemon/src/http/` (`ui.rs`, `mod.rs`,
`stream.rs`, `ws.rs`, `board.rs`, `jarvis.rs`, `mac.rs`, `browser.rs`) on the
commit this branch started from.

## Connecting

| Step | How |
|---|---|
| Address | `--daemon`, else `$SBXW_DAEMON`, else `127.0.0.1:7681` (`DEFAULT_WEB_ADDR`, `crates/core/src/config.rs`). A `web_addr` set in `sbxw.toml` is not read: pass `--daemon` |
| Token | `--token`, `--token-file`, else `~/.sbxw/run/ui-token` (`$HOME/.sbxw`, as `crates/daemon/src/store.rs`). The daemon mints a new one at every start, so the file is read again on every reconnect |
| Proof | `GET /api/health?nonce=<32 hex>` must answer `proof = sha256(token ‖ nonce)` before any request carries the token (`guard.rs`, `token.rs`). A process on the port that cannot prove it never sees the token |
| Requests | `Authorization: Bearer <token>`, no `Origin`, `Host` = the address (allowed by `allowed_hosts`) |
| Down | Retries every 0.25 s, doubling to 3 s; the window shows one line (`No sbxw daemon at 127.0.0.1:7681. Start it with \`sbxw up\`; retrying.`) |
| Restart | The event stream ends; the link reconnects (new token from the file), reads the list again, and panes whose terminal closed open again (the daemon keeps the PTY and replays its last 256 KB) |

## Live updates

There is no "sandbox list changed" event on the bus (`crates/daemon/src/bus.rs`).
The web UI reads `/api/sandboxes` again after its own actions and after some
events. The app does the same:

- after its own stop (`Link::run`, then a read);
- 300 ms after any `session`, `provision`, `lead`, `lead-end` or `codemap`
  event on `/api/stream` (a burst makes one read; `client::REFRESH_ON`);
- after opening the agent of a sandbox that is not running: at 1.5, 3, 6, 12
  and 24 s until it shows as running (`RUNNING_RECHECK_MS` in
  `assets/js/sandboxes.js`);
- every 15 s while connected, which the web UI does not do: a change `sbx`
  makes behind the daemon (a CLI `sbxw stop`) otherwise shows nowhere.

**Side effect:** each `/api/stream` connection counts as one open tab in the
daemon (`bus.clients`). With the desktop app and one browser tab open, the web
UI shows its "several tabs" warning. Not changed (no daemon change in wave 1).

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
| `GET /ws?sandbox=&mode=&thread=` | `open_terminal()` | binary PTY frames, JSON `resize`, text error lines, `{"type":"attention"}`; `mode` absent is the agent (starts a stopped sandbox) |

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
| Shell extras (`feat/shell_extras.rs`, NETWORK.md) | `POST /api/sandboxes/:name/rm`, `/recreate`, `/duplicate`, `/detach`, `GET /api/usage` (every 30 s), `GET /api/state` (versions, at connection) | remove, recreate |
| Create (`feat/create/`, CREATE.md) | `POST /api/sandboxes/create` (409 consent answered with `approved` lines through `Confirm::grant_lines`), `POST /api/sandboxes/chat`, `POST /api/carve/preview`, `GET /api/fs`, `POST /api/fs/pick`, `GET`/`POST /api/fs/favourites`, `GET /api/kit-mounts`, `GET /api/template/status`, `GET /api/remote-hosts`, `POST /api/remote-hosts/:host/clone`; bus event `provision` | grants a sandbox would get beyond its workspace |
| Setup (`feat/setup/`, SETUP.md) | `GET /api/secrets`, `POST /api/secrets/custom`, `/rm`, `GET /api/github-token`, `POST /api/github-token`, `/rm`, `/gh`, `GET`/`POST /api/kits`, `GET /api/my-skills`, `POST /api/my-skills/rm`, `GET /api/sandboxes/:name/skills`, `POST .../skills/keep`, `GET`/`POST /api/mcp`, `GET /api/mcp/:server`, `POST /api/mcp/:server/rm`, `/auth`, `POST /api/sandboxes/:name/mcp/load`, `/unload`, `GET /api/remote-hosts`, `POST /api/remote-hosts/key`, `/add`, `/reload`, `POST /api/remote-hosts/:host/edit`, `/rm`, `/test`, `GET /api/voice`, `POST /api/voice/install`, `/rm`, `/warm`, `/cancel`; bus event `voice` | secrets and tokens (core-drawn secure fields), kits that reach out, a stdio MCP server (acknowledged command), a new remote host (fingerprints read by the core) |
| Core slots | none (open/close modal and panels, extras) | |

### Not covered yet

Grouped by the area that will need them, with the web UI file that calls
each group, so a later thread can read what it expects.

| Area (web UI file) | Routes |
|---|---|
| Sessions and agents (`main.js`, `panes.js`) | `GET /api/ptys`, `POST /api/input`, `POST /api/answer`, `GET /api/hook/log`, `GET /api/models`, `POST /api/focus`, `POST /api/watching`, `GET /api/events` (sessions only), `GET /api/watch-events`, `POST /api/sandboxes/:name/up` |
| Threads and lead (`sidepanel.js`, `pane-controls.js`, `plans.js`, `board.js`) | `GET /api/sandboxes/:name/threads`, `/repos`, `/worktrees`, `GET /api/sandboxes/:name/threads/:thread`, `POST .../threads/:thread/close`, `POST /api/sandboxes/:name/lead`, `POST .../lead/end`, `GET /api/leads`, `GET /api/plans`, `POST /api/plans/:id/approve`, `/refuse`, `/merge`, `GET /api/sandboxes/:name/board` |
| Relay (`relay.js`) | `GET /api/relay`, `POST /api/relay/:id/route`, `POST /api/relay/:id/deny`, `GET /api/relay/events` |
| Files, artifacts, uploads (`files.js`, `upload.js`) | `GET /api/sandboxes/:name/artifacts`, `.../artifacts/download`, `GET /api/sandboxes/:name/fs`, `.../fs/file`, `.../fs/recent`, `POST .../fs/delete`, `POST /api/sandboxes/:name/paste-image`, `POST .../upload` |
| Code map (`codemap.js`) | `POST /api/sandboxes/:name/codemap`, `POST .../codemap/lens`, `GET /api/codemap` |
| Templates, env file, SSH (`templates.js`, `envfile.js`, `ssh.js`) | `GET /api/templates`, `POST /api/templates/save`, `/rm`, `GET /api/disk-usage`, `GET`/`POST /api/sandboxes/:name/envfile` |
| Remote Control (`pane-controls.js`) | `GET`/`POST /api/sandboxes/:name/remote-control`, `POST .../remote-control/login` |
| Header leftovers (`main.js`) | `POST /api/initial`, `POST /api/shutdown` |
| Browser side panel (`browser.js`) | `GET /ws/browser`, `POST /api/sandboxes/:name/browser/allow`, `GET .../browser/grants`, `POST .../browser/settle`, `GET /api/browser/active`, `GET /api/embed/resolve`, `GET`/`POST /api/embed/pairings`, `POST /api/embed/pairings/rm` |
| Virtual Mac (`mac.js`) | `GET /api/mac`, `POST /api/mac/install`, `/up`, `/stop`, `/force-off`, `/pause`, `/show`, `/take-back` |

Routes a client never calls: `GET /` and `/sidepanel` (pages), and the
agent-listener routes the UI listener answers with "moved" (`/api/hook`,
`/api/relay/ask`, ...). The agent listener (sandboxes calling in with their
own token: `/api/relay/*`, `/api/plans/*`, `/api/board/*`,
`/api/jarvis/state|propose|wait`, `/api/mac/*` actions, `/api/browser/state`)
is not for the desktop app.

**Sensitive routes.** Every route that answers a question for the person
or widens access is called only from a `feat::Confirm` the core draws
(table above). Still to come with the same rule: `POST
/api/plans/:id/approve|merge`, `POST /api/relay/:id/route`, `POST
.../fs/delete`, the browser allow.

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
