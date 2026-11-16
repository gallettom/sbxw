# sbxw desktop: threads, plans and the lead

Thread desktop-threads of phase 4, first wave. Built and tested on Linux
(aarch64, no GPU, headless Rune renders) against fixtures and the stand-in
daemon. **Nothing was run on a Mac or against the real daemon.**

Files: `src/feat/threads/` (`mod.rs` the feature, `model.rs` the pure
readings, `standin.rs` the stand-in routes, `tests.rs`), `screens/threads.luau`
(panel), `screens/plans.luau` and `screens/threads-new.luau` (modals),
`fixtures/threads/` (18 fixtures), `scripts/demo-threads.sh`, the shots in
`.sbxw-artifacts/threads/`. The contract for screens is the `### threads`
section of `docs/screens.md`.

## What was built

| Web UI (file) | In the app |
|---|---|
| Sessions folded under each sandbox (`panes.js`, `sidebarSessionsHtml`) | Rows under the sandbox in the sidebar (`Place::SandboxRow`): `✦ main`, `✦ s1` (main checkout), `⑂ t1` (own worktree), each with its state word (`working`, `wants you`, `done`, `starting`, `failed`, `stopped`) and tone, the pane it is in as a badge; a press shows it (`open_terminal` with the thread). Shown once a sandbox has a second session, as the web UI does |
| Sessions menu (`panes.js`) | The **Threads** panel (docked), per sandbox: every session with its kind, worktree, repo, model, plan report, and Go to / Show / Start, Bash, Interrupt, Remote control, Close |
| ⑂ Thread dialog (`panes.js`, `openThread`) | `threads-new` modal: name (suggested `t<n>` / `s<n>`), existing ones as chips, the repository list with branch and state (from `GET .../repos`), the model, a session on the main checkout (`kind = session`), the session count; Open stays off while the name is bad, of the other kind, a repo must be picked, or no place is free |
| Close thread (`panes.js`, `closeThread`) | `thread_close`, risk Approval: the core reads `GET .../threads/:t` first (Prepare) and its dialog says what goes: the worktree, uncommitted files (Confirm needs "Discard N uncommitted files" ticked), the branch deleted only when nothing in it is missing from its base. A session on the main checkout and a shared plan's thread say their own words. A refusal shows the daemon's reason in a notice; once closed, its panes go (`close_panes`) |
| Pane bar (`pane-controls.js`, ⋯ menu) | Extras on the bar of each session (`Place::Session`): the model, the plan state with the report as tooltip, **Esc** (interrupt: Escape typed through `POST /api/input`), **Remote** (Remote Control, `GET`/`POST .../remote-control`, a setup token offers the login switch, which is risk Approval), **Close thread**; on the main session: **Team** / **Team on**, **Review plan**, **Merge** / **Merging…** / **Retry merge**, **Threads** |
| ★ Team mode (`panes.js`, `plans.js`) | `lead_start` (direct, as the web UI), `lead_end` through a dialog that says what it leaves (threads at work, the waiting plan refused) |
| The plan dialog (`plans.js`) | `plans` modal: summary, variant (split, shared) with the lead's reason, one card per thread (name, repository, goal's first sentence and Show more, Owns chips, model), Edit (name, goal, owns, repository), remove, add, Undo my changes, Refuse with a note, Later. The repository a thread still needs is asked on its card. Opens by itself when a plan arrives |
| The draft | Held by the core in `data.threads_ui.drafts`, changed one intent at a time (`plan_edit`, `plan_remove`, `plan_add`, `plan_variant`, `plan_reset`); the screen never sends the threads |
| Approve and start | `plan_approve`, risk Approval: the core reads the sandbox's sessions (`GET .../threads`), then its dialog lists the draft as the core holds it, the person's changes against the proposal, and "Starts N new Claude sessions: X of Y open now (main, t1), Z free" |
| The ceiling (`max_agent_sessions`) | Said before and after: the plan card count ("4 of 5 Claude sessions open now") and an amber line when it does not fit; the approval dialog refuses to open with the full explanation (how many it needs, how many are open and which, how many places are free, what to remove or close, the setting); a refusal from the daemon anyway (`svc/plans.rs`'s text, parsed) is said the same way in the dialog and a notice. New thread says it too |
| Approved plan | Panes open for its threads while the window has room (empty panes, then up to 4), from this app's approval or one seen elsewhere; the dialog shows each thread's state, goal and report, and the session stopped |
| Merge (`plans.js`) | Same dialog once the lead proposes: the order, the lead's note, a row per thread (✓ commit, ✕ the error, pending, not merged), the outcome ("Merged into main", "Stopped at web-orders: CONFLICT …. Not merged: docs."), "Close each thread once merged". `plan_merge`, risk Approval: the core's dialog shows the order, the note, the base, the distillate, whether threads close. A shared plan: open claims are listed and hold Merge. After a merge that closed threads the core reads the threads again and says which were **not closed** |
| Board (`board.js`) | Panel tab: summary (files claimed, open questions), claims (owner, held, extended, conflict / held long), the feed newest first, filtered by thread and kind |
| Worktree graph (`worktrees.js`) | Panel tab, as text: per repo its base branch, head, uncommitted files; per thread its branch, worktree, ahead / behind, uncommitted, last three commits |

Data keys (each one route's answer, the shape the daemon sends):
`sessions`, `threads` (per sandbox), `plans`, `leads`, `models`, `board`
(per sandbox); set by the feature: `threads_view` (sessions computed per
sandbox), `threads_ui`, `thread_repos`, `worktrees`.

## Web features not ported, and why

| Web UI | Why |
|---|---|
| The graph drawing of `worktrees.js` (lanes, forks) | No drawing element; the same facts are listed as text |
| Board times ("until 10:42", "3 min ago") | Screens have no clock (no `os`); held time and extensions are shown |
| Editing the distillate at merge (keep or reword each item) | Sent as the lead proposed it (the daemon's default); the dialog lists it. Could be another draft like the plan's |
| Group colours chosen per pane, pane drag and drop regrouping (`paintGroups`) | Core pane layout; not in this thread's area |
| "Show more" measured by overflow | Shown whenever the goal has more than one sentence |
| A multi-line goal field | `input` is one line; long goals edit fine but on one line |
| Sessions fold remembered per sandbox | No per-feature UI memory used yet (`Action::Remember` came late); the rows always show |
| Agent activity strips (liserets), `/api/answer` from the UI | Not in this thread's scope |
| Changing a running thread's model | The web UI has none either (model is chosen at the first open) |

## Evidence

- `cd desktop && CARGO_PROFILE_DEV_DEBUG=0 cargo test -j1`: all green at
  c7cc82c's tree (215 lib tests, 3 ignored, integration tests pass). The
  feature's own: `cargo test --lib threads` (21 pass), including: every Approval
  intent answers only one Confirm or Prepare and no Direct intent calls the
  approve, merge or close routes; the plan screen's Approve and Merge return
  only those intents; the approval dialog lists the draft and the sessions;
  the ceiling text (numbers, names, free places, the way out) from the
  dialog and from the daemon's refusal; a dirty close needs discarding and
  a refusal shows the daemon's words; New thread's repo / model / kind; a
  whole plan against the stand-in routes (Team mode, plan, refusal over the
  ceiling, approval, steps, merge stopped at a conflict, retry, a thread not
  closed said, panes closed).
- Headless shots, solo (`scripts/demo-threads.sh`), each looked at:
  - `threads-basic`: main working (P1), s1 in the main checkout, old-fix
    stopped (Start), t1 working with its model, t2 amber "wants you".
  - `threads-empty`: main only, New thread / New session.
  - `threads-errors`: the red line "Could not read the threads of web-app:
    sbxw has no workspace recorded for 'web-app'".
  - `threads-board`: 2 files claimed, 1 open question; a claim in conflict
    (red) and one held 22 min; feed with conflict, warning, commit,
    question, decision; filters.
  - `threads-worktrees`: repo web-app on main, 1 uncommitted (a merge
    refuses it), t1 3 ahead 1 behind with two commits, t2 4 uncommitted.
  - `threads-plan-pending`: Team mode on, amber "The lead's plan waits for
    you" with Review plan.
  - `plans-plan-pending`, `plans-plan-edited` (shared, two threads, docs
    asks its repository on the card, Approve off), `plans-plan-ceiling`
    (amber count line and the red explanation), `plans-plan-approved`
    (working, wants you, done with reports), `plans-merge-proposed`,
    `plans-merge-running` (✓ then pending), `plans-merge-failed` (✕ conflict,
    "Stopped at web-orders … Not merged: docs.", Retry merge),
    `plans-merge-merged` ("Merged into main." and the amber "Not closed:
    docs."), `plans-merge-shared` (claims listed, commits by thread,
    distillate, Merge all off).
  - `threads-new-new-thread` (three repos, pick one, Open off until then),
    `threads-new-new-session` (main checkout wording, no repo),
    `threads-new-new-full` ("5 of 5 … no place is free", Open off).
- Whole window against the stand-in (`scripts/demo-threads.sh window`),
  `window-*.png`: `main` (web-app's agent, Team on its pane bar),
  `plan-arrived` (the plan dialog opened by itself, "Plan waiting" in the top
  bar), `edited` (docs given its repository), `approve-dialog` (the core's
  dialog: summary, split, "Your changes: docs: repository docs.", "Starts 3
  new Claude sessions: web-app has 1 of 5 open now (main), 4 free.", the
  list), `approved` (4 panes: main and the three threads, each bar with
  model or state, Esc, Remote, Close thread; rows under web-app in the
  sidebar, all working), `panel`, `status` (all done with reports),
  `merge-proposed`, `merge-dialog` (the core's dialog with the order and
  note), `merge-failed` (✕ CONFLICT on web-orders, notice), `merged` (all ✓,
  "Merged into main.", "Every merged thread was closed."), `end` (the thread
  panes are gone, one terminal, the sidebar rows gone, the panel lists main
  only).

## Not verified

- **The real daemon.** Routes and shapes were read from
  `crates/daemon/src/http/ui.rs`, `svc/plans.rs`, `svc/sandboxes.rs`,
  `svc/board.rs`, `crates/core/src/plans.rs`; the stand-in follows them, the
  refusals word for word. Not run against `sbxw up`: the order of `plan`
  events and answers, a real merge's duration (the dialog shows Merging…
  while the call runs, up to 600 s), `GET /api/sessions` volume on a busy
  sandbox.
- **Remote Control** against claude.ai, and Esc reaching Claude Code
  through `/api/input` (`mode: thread:<name>`).
- **macOS**: nothing was run there (fonts, the panel width, keyboard).
- Stand-in shortcuts: plans move one step per read at most every 1.2 s;
  the merge conflict happens once; `/ws` thread opens are recorded through
  the core's `OPEN` hook.

## macOS test procedure

Prerequisites: on the Mac, this branch; `sbxw up` running; a running
sandbox `web-app` whose workspace holds two git repos (say `api`, `web`) or
is one; `max_agent_sessions = 4` in its `sbxw.toml` for step 9; the app:
`cd desktop && cargo run --release`; the web UI beside it (`sbxw web`) to
compare.

1. Click web-app in the sidebar: its agent opens. Its pane bar shows Team,
   Esc, Remote, Threads.
2. Press **Threads** on the bar: the Threads panel docks on the right with
   web-app: "Team mode off", one session (main, P1), "1 of 4 Claude sessions
   open".
3. Press **New thread**: the dialog suggests `t1`, lists the repositories
   with branch and state, Open is off until one is picked (if several). Pick
   one, choose Sonnet, Open: a new pane opens on the thread. In the web UI's
   ⑂ Sessions menu the same thread shows. Its pane bar shows the model,
   Esc, Remote, Close thread; the sidebar lists `✦ main` and `⑂ t1` under
   web-app.
4. Type a long task in t1, press **Esc** on its bar: Claude Code stops.
5. In t1's worktree (Bash on its panel row) create an uncommitted file,
   press **Close thread**: the app's red dialog lists the worktree and
   "It has 1 uncommitted file"; Close stays off until "Discard 1
   uncommitted file in it" is ticked. Cancel. Commit the file on the
   thread's branch, Close thread again: "Branch sbxw/t1 is kept: its 1
   commit is not in main". Confirm: the pane goes, the row goes.
6. **New session** (main checkout): name `s1`, Open: it works on the main
   checkout; Close session says the checkout stays as it is.
7. Press **Team** on web-app's main pane; ask the agent to split a small
   task into 3 threads. Within seconds of its proposal the plan dialog
   opens by itself (top bar: Plan waiting).
8. Edit a thread's goal, remove one thread, set the variant to Shared then
   back to Split, press **Approve and start**: the app's dialog lists the
   remaining threads, "Your changes: …", and "Starts 2 new Claude
   sessions: web-app has 1 of 4 open now (main), 3 free." Cancel, then
   Approve again and confirm: panes open for each thread; the dialog closes.
9. Ask the lead for a plan of 4 threads while 2 sessions are open: the
   dialog's count line turns amber; Approve shows, instead of a
   confirmation, "Not started: this plan needs 4 new Claude sessions and
   web-app allows 4 at once (max_agent_sessions = 4). 2 sessions are open
   now (main, …), so 2 places are free. To make room: remove 2 threads …".
   Refuse it with a note: the lead's session receives the note.
10. When the lead proposes the merge, a notice says so; press **Merge** on
    the main pane: the dialog shows the order and the lead's note. Press
    Merge all, confirm in the app's dialog: each row turns ✓ with its
    commit, "Merged into main". With "Close each thread once merged"
    ticked, the thread panes go; a thread that could not be closed is
    named in amber.
11. Make a merge conflict (two threads editing one line) and merge: the row
    shows ✕ with git's message, "Stopped at … Not merged: …", Retry merge.
12. A shared plan: the panel's **Board** tab lists claims and the feed;
    while a file is claimed, Merge all is off and the dialog lists it.
13. **Worktrees** tab: each thread's branch, ahead / behind, uncommitted;
    compare with the web UI's worktree graph.
14. **Remote** on a thread's bar: with a claude.ai login it turns on
    (Remote on after Claude connects); with a setup token a notice offers
    the switch, which opens the app's dialog.
15. Press **Team on**: the dialog says the threads keep running and a
    waiting plan is refused; confirm: the bar shows Team again.

If a step fails, report its number, a screenshot and the app's log
(`RUST_LOG=info cargo run --release 2>&1 | tee app.log`).
