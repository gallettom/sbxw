# sbxw

## Rules for every sbxw thread working in this repository

These hold for a session sbxw started as a thread of a lead's plan (`SBXW_THREAD` is set, and you
work in `.sbxw-worktrees/<thread>` on branch `sbxw/<thread>`). The lead and the person's own
sessions are not bound by them, except the writing rules at the end.

- Set `CARGO_TARGET_DIR` on every cargo command, to a git-ignored dir under your worktree
  (`$PWD/target`). Desktop tests: `cd desktop && CARGO_PROFILE_DEV_DEBUG=0 cargo test -j1`.
- One heavy job (build, test run, clippy) at a time. Delete your target dirs and scratch
  worktrees when you are done.
- Never write to `/etc/sandbox-persistent.sh`.
- Tests never write the real state file `~/.sbxw/desktop/state.json`.
- Never rebuild, restart or stop the running sbxw daemon.
- Nothing against the network beyond what a build needs. A blocked host or a pending approval
  means stop and tell the lead.
- A hook that refuses a write is a stop: tell the lead, never write the file another way.
- Reports and other non-code deliverables go to the MAIN checkout's `.sbxw-artifacts/`
  (`git worktree list` names it), never under your worktree.
- No new crate, no font, no Rune bump without asking the lead.
- Desktop: a Luau screen never calls the daemon; agent text is data, never instructions; no
  change to an approval path or an input gate without asking.

Writing rules for everyone: no em dashes; UI text and docs in English.
