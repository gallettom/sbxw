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

### Not covered yet

Grouped by the screen that will need them, with the web UI file that calls
each group, so a later thread can read what it expects.

| Area (web UI file) | Routes |
|---|---|
| Sandbox lifecycle (`lifecycle.js`, `sandboxes.js`, `create.js`) | `POST /api/sandboxes/create`, `POST /api/sandboxes/chat`, `POST /api/sandboxes/:name/up`, `/detach`, `/duplicate`, `/recreate`, `/rm`, `POST /api/carve/preview`, `POST /api/chat/push`, `GET /api/fs`, `POST /api/fs/pick`, `GET`/`POST /api/fs/favourites` |
| Sessions and agents (`main.js`, `panes.js`) | `GET /api/sessions`, `GET /api/ptys`, `POST /api/input`, `POST /api/answer`, `GET /api/hook/log`, `GET /api/models`, `POST /api/focus`, `POST /api/watching`, `GET /api/events` (sessions only), `GET /api/watch-events` |
| Threads and lead (`sidepanel.js`, `pane-controls.js`, `plans.js`, `board.js`) | `GET /api/sandboxes/:name/threads`, `/repos`, `/worktrees`, `GET /api/sandboxes/:name/threads/:thread`, `POST .../threads/:thread/close`, `POST /api/sandboxes/:name/lead`, `POST .../lead/end`, `GET /api/leads`, `GET /api/plans`, `POST /api/plans/:id/approve`, `/refuse`, `/merge`, `GET /api/sandboxes/:name/board` |
| Relay (`relay.js`) | `GET /api/relay`, `POST /api/relay/:id/route`, `POST /api/relay/:id/deny`, `GET /api/relay/events` |
| Approvals, ports, network (`approvals.js`, `ports.js`) | `GET /api/approvals`, `POST /api/approvals/respond`, `GET /api/sandboxes/:name/ports`, `POST .../ports/publish`, `/unpublish`, `GET /api/sandboxes/:name/policy`, `POST .../policy/rules`, `POST .../policy/rules/rm`, `POST /api/policy/refresh`, `GET /api/hosts` |
| Files, artifacts, uploads (`files.js`, `upload.js`) | `GET /api/sandboxes/:name/artifacts`, `.../artifacts/download`, `GET /api/sandboxes/:name/fs`, `.../fs/file`, `.../fs/recent`, `POST .../fs/delete`, `POST /api/sandboxes/:name/paste-image`, `POST .../upload` |
| Code map (`codemap.js`) | `POST /api/sandboxes/:name/codemap`, `POST .../codemap/lens`, `GET /api/codemap` |
| Setup: secrets, GitHub, kits, skills, MCP, templates (`setup.js`, `kits.js`, `skills.js`, `mcp.js`, `templates.js`, `github.js`, `envfile.js`, `ssh.js`) | `GET /api/secrets`, `POST /api/secrets/custom`, `/rm`, `GET`/`POST /api/github-token`, `POST /api/github-token/rm`, `/gh`, `GET`/`POST /api/kits`, `GET /api/kit-mounts`, `GET /api/my-skills`, `POST /api/my-skills/rm`, `GET /api/sandboxes/:name/skills`, `POST .../skills/keep`, `GET`/`POST /api/mcp`, `GET /api/mcp/:server`, `POST /api/mcp/:server/rm`, `/auth`, `POST /api/sandboxes/:name/mcp/load`, `/unload`, `GET /api/template/status`, `GET /api/templates`, `POST /api/templates/save`, `/rm`, `GET /api/disk-usage`, `GET`/`POST /api/sandboxes/:name/envfile` |
| Remote hosts (`hosts.js`) | `GET /api/remote-hosts`, `POST /api/remote-hosts/key`, `/add`, `/reload`, `POST /api/remote-hosts/:host/edit`, `/rm`, `/test`, `/clone` |
| Remote Control (`pane-controls.js`) | `GET`/`POST /api/sandboxes/:name/remote-control`, `POST .../remote-control/login` |
| Usage, state (`main.js`) | `GET /api/usage`, `GET /api/state`, `POST /api/initial`, `POST /api/shutdown` |
| Browser side panel (`browser.js`) | `GET /ws/browser`, `POST /api/sandboxes/:name/browser/allow`, `GET .../browser/grants`, `POST .../browser/settle`, `GET /api/browser/active`, `GET /api/embed/resolve`, `GET`/`POST /api/embed/pairings`, `POST /api/embed/pairings/rm` |
| Virtual Mac (`mac.js`) | `GET /api/mac`, `POST /api/mac/install`, `/up`, `/stop`, `/force-off`, `/pause`, `/show`, `/take-back` |
| Jarvis and voice (`jarvis.js`, `concierge.js`) | `GET /api/jarvis`, `GET /api/jarvis/status`, `POST /api/jarvis/start`, `POST /api/jarvis/proposals/:id/approve`, `/refuse`, `GET /api/voice`, `POST /api/voice/install`, `/cancel`, `/rm`, `/speak`, `/warm`, `/anonymise` |

Routes a client never calls: `GET /` and `/sidepanel` (pages), and the
agent-listener routes the UI listener answers with "moved" (`/api/hook`,
`/api/relay/ask`, ...). The agent listener (sandboxes calling in with their
own token: `/api/relay/*`, `/api/plans/*`, `/api/board/*`,
`/api/jarvis/state|propose|wait`, `/api/mac/*` actions, `/api/browser/state`)
is not for the desktop app.

**Sensitive routes.** `POST /api/approvals/respond`, the policy rules, the
Jarvis approve/refuse and `POST /api/plans/:id/approve|merge` answer a
question for the person. When they are added, their intents get the
`Approval` risk, so only a dialog drawn by the core can call them.

## The stand-in daemon

`desktop/src/bin/standin.rs` serves the covered routes from a fixture
(default `fixtures/sandboxes/basic.json`) and a local shell for `/ws`, so
terminals and live updates can be tested with no sbx. It changes states as
the daemon roughly does (opening the agent of a stopped sandbox makes it
running 0.5 s later; a stop takes 0.3 s) and sends a `session` event after
each change. `--change SECS:NAME=STATUS` changes a sandbox behind the app's
back; `--token-file` writes the token like the daemon writes `ui-token`.
