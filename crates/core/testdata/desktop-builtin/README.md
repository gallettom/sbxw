Copies of what the desktop app writes to `<sbxw home>/desktop/builtin/` at
each start (branch `sbxw/desktop-jarvis`: `desktop/screens/*.luau`,
`desktop/docs/screens.md`, and `intents.json` as `desktop/src/builtin.rs`
exports the registry of `desktop/src/intent.rs`). The Jarvis tests in
`crates/core/src/jarvis.rs` and `crates/daemon/src/svc/jarvis.rs` plan
against them. Refresh them when the app's built-ins change.
