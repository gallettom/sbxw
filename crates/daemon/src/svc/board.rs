//! The Team-mode board around its book: one per sandbox, read from disk on
//! first use, written after every change, and cleared of a thread's claims
//! when its plan says it finished.

use crate::app::{now_ms, App};
use crate::bus::Event;
use crate::store::write_atomic;
use sbxw_core::board::{Board, ClaimOutcome, Entry, ExtendOutcome, Kind, Taken, LEAD};
use sbxw_core::session::{SessionKey, SessionKind};
use sbxw_core::SandboxName;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;

/// A refused write is told to the lead at most once per file and session in
/// this window: a thread retrying is one conflict, not ten.
const TELL_LEAD_EVERY_MS: u64 = 10 * 60 * 1000;

impl App {
    fn board_path(&self, sandbox: &str) -> PathBuf {
        self.store.paths.state().join("boards").join(format!("{sandbox}.json"))
    }

    fn load_board(&self, sandbox: &str) -> Board {
        let path = self.board_path(sandbox);
        match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
                tracing::warn!("board: starting afresh, {} unreadable: {e}", path.display());
                Board::new(sandbox)
            }),
            Err(_) => Board::new(sandbox),
        }
    }

    /// Run `f` on `sandbox`'s board, then persist and broadcast. Every change
    /// comes through here.
    pub(crate) fn with_board<T>(&self, sandbox: &str, plan: Option<&str>, f: impl FnOnce(&mut Board) -> T) -> T {
        let (out, bytes) = {
            let mut boards = self.boards.lock().unwrap();
            let board = boards.entry(sandbox.to_string()).or_insert_with(|| self.load_board(sandbox));
            board.set_plan(plan);
            let out = f(board);
            (out, serde_json::to_vec_pretty(&*board))
        };
        let path = self.board_path(sandbox);
        if let Err(e) = bytes.map_err(anyhow::Error::from).and_then(|b| write_atomic(&path, &b)) {
            tracing::warn!("board: could not save {}: {e:#}", path.display());
        }
        self.send(Event::Board(sandbox.to_string()));
        out
    }

    /// Whether `sandbox` has a board at all, without making one.
    fn has_board(&self, sandbox: &str) -> bool {
        self.boards.lock().unwrap().contains_key(sandbox) || self.board_path(sandbox).exists()
    }

    pub fn board_view(&self, sandbox: &str) -> Value {
        let now = now_ms();
        if !self.has_board(sandbox) {
            return Board::new(sandbox).view(now);
        }
        self.with_board(sandbox, None, |b| {
            b.expire(now);
            b.view(now)
        })
    }

    pub fn board_claim(&self, sandbox: &str, plan: Option<&str>, session: &str, files: &[String]) -> Result<ClaimOutcome, String> {
        self.with_board(sandbox, plan, |b| b.claim(session, files, now_ms()))
    }

    pub fn board_release(&self, sandbox: &str, plan: Option<&str>, session: &str, files: &[String]) -> Result<Vec<String>, String> {
        self.with_board(sandbox, plan, |b| b.release(session, files, None, now_ms()))
    }

    pub fn board_extend(&self, sandbox: &str, plan: Option<&str>, session: &str, files: &[String]) -> Result<ExtendOutcome, String> {
        self.with_board(sandbox, plan, |b| b.extend(session, files, now_ms()))
    }

    pub fn board_force_release(&self, sandbox: &str, files: &[String], reason: &str) -> Result<Vec<String>, String> {
        let gone = self.with_board(sandbox, None, |b| b.force_release(files, reason, now_ms()))?;
        Ok(gone.into_iter().map(|c| c.file).collect())
    }

    pub fn board_approve_extension(&self, sandbox: &str, files: &[String]) -> Result<Vec<(String, u64)>, String> {
        self.with_board(sandbox, None, |b| b.approve_extension(files, now_ms()))
    }

    pub fn board_note(&self, sandbox: &str, plan: Option<&str>, by: &str, kind: Kind, text: &str, to: Option<&str>, files: &[String]) -> Result<Entry, String> {
        self.with_board(sandbox, plan, |b| b.note(by, kind, text, to, files, now_ms()))
    }

    /// Before a write: what another session holds. A refusal is logged on
    /// the board and the lead is told, in one line typed into its session.
    pub fn board_check(self: &Arc<Self>, sandbox: &SandboxName, plan: Option<&str>, session: &str, files: &[String]) -> Result<Vec<Taken>, String> {
        let now = now_ms();
        let (taken, fresh) = self.with_board(sandbox, plan, |b| {
            let recent = |file: &str| {
                b.entries.iter().rev().take_while(|e| now.saturating_sub(e.at) < TELL_LEAD_EVERY_MS).any(|e| {
                    e.kind == Kind::Conflict && e.by == session && e.files.iter().any(|f| f == file)
                })
            };
            let normalized = sbxw_core::board::normalize_files(files).unwrap_or_default();
            let seen: Vec<String> = normalized.into_iter().filter(|f| recent(f)).collect();
            let taken = b.check(session, files, now)?;
            let fresh: Vec<Taken> = taken.iter().filter(|t| !seen.contains(&t.file)).cloned().collect();
            Ok::<_, String>((taken, fresh))
        })?;
        if !fresh.is_empty() && session != LEAD && self.sessions.is_live(&SessionKey::agent(sandbox)) {
            let list: Vec<String> = fresh
                .iter()
                .map(|t| format!("{} (held by {} for {} more min)", t.file, t.owner, t.until.saturating_sub(now).div_ceil(60_000)))
                .collect();
            let text = format!(
                "[sbxw board] Thread {session} was refused a write to {}. Arbitrate if they cannot settle it: \
                 read_board, then force_release or tell one of them.",
                list.join(", ")
            );
            self.spawn_push(sandbox.clone(), SessionKind::Agent, text, None);
        }
        Ok(taken)
    }

    /// After a tool call: record what the session read and wrote and what it
    /// committed, and answer with the writes outside its claims and the
    /// board's news for it.
    #[allow(clippy::too_many_arguments)]
    pub fn board_after(
        &self,
        sandbox: &str,
        plan: Option<&str>,
        session: &str,
        read: &[String],
        written: &[String],
        commit: Option<(&str, &str, &[String])>,
    ) -> Result<(Vec<String>, Vec<String>, Vec<Entry>), String> {
        let now = now_ms();
        self.with_board(sandbox, plan, |b| {
            b.record_reads(session, read);
            let outside = if written.is_empty() { vec![] } else { b.record_writes(session, written, now)? };
            let mut released = Vec::new();
            if let Some((hash, subject, files)) = commit {
                released = b.commit(session, hash, subject, files, now)?;
            }
            let news = b.delta(session, now);
            Ok((outside, released, news))
        })
    }

    /// Release a thread's claims once its plan says it finished.
    pub fn start_board_listener(self: &Arc<Self>) {
        let app = self.clone();
        let mut rx = self.bus.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(Event::Plan(p)) => app.board_release_finished(&p),
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => break,
                }
            }
        });
    }

    pub(crate) fn board_release_finished(&self, plan: &sbxw_core::plans::Plan) {
        if !self.has_board(&plan.sandbox) {
            return;
        }
        let now = now_ms();
        let holders: Vec<String> = {
            let mut boards = self.boards.lock().unwrap();
            let board = boards.entry(plan.sandbox.clone()).or_insert_with(|| self.load_board(&plan.sandbox));
            plan.threads
                .iter()
                .filter(|t| t.state.is_finished() && board.claims.values().any(|c| c.owner == t.name))
                .map(|t| t.name.clone())
                .collect()
        };
        if holders.is_empty() {
            return;
        }
        self.with_board(&plan.sandbox, None, |b| {
            for t in &holders {
                b.release_finished(t, now);
            }
        });
        tracing::info!("board: '{}' released the claims of {}", plan.sandbox, holders.join(", "));
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::app;
    use sbxw_core::plans::{PlanThread, ThreadState};
    use sbxw_core::SandboxName;
    use std::time::Duration;

    fn t(name: &str) -> PlanThread {
        PlanThread {
            name: name.into(),
            goal: "g".into(),
            scope: String::new(),
            repo: None,
            model: None,
            state: ThreadState::Starting,
            summary: String::new(),
            updated_ms: 0,
        }
    }

    #[tokio::test]
    async fn a_board_is_persisted_and_survives_a_restart() {
        let (dir, app) = app();
        let sb = SandboxName::parse("neos").unwrap();
        app.board_claim(&sb, Some("p-1"), "api", &["src/a.ts".into()]).unwrap();
        assert!(app.store.paths.state().join("boards/neos.json").exists());
        let again = crate::testing::app_in(dir.path());
        let taken = again.board_check(&sb, None, "front", &["src/a.ts".into()]).unwrap();
        assert_eq!(taken[0].owner, "api");
        assert_eq!(again.board_view("neos")["plan"], "p-1");
    }

    #[tokio::test]
    async fn a_thread_reporting_done_releases_its_claims() {
        let (_d, app) = app();
        app.start_board_listener();
        let sb = SandboxName::parse("neos").unwrap();
        let p = app.plan_propose(&sb, "", vec![t("api"), t("front")], None).unwrap();
        app.plans.lock().unwrap().approve(&p.id, vec![t("api"), t("front")], 1).unwrap();
        app.board_claim(&sb, Some(&p.id), "api", &["a".into()]).unwrap();
        app.board_claim(&sb, Some(&p.id), "front", &["b".into()]).unwrap();

        app.plan_report(&sb, &sbxw_core::ThreadName::parse("api").unwrap(), ThreadState::Done, "ok").unwrap();
        for _ in 0..100 {
            if app.board_view("neos")["claims"].as_array().unwrap().len() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let v = app.board_view("neos");
        let claims = v["claims"].as_array().unwrap();
        assert_eq!(claims.len(), 1, "{v}");
        assert_eq!(claims[0]["owner"], "front");
        let last = v["entries"].as_array().unwrap().last().unwrap().clone();
        assert_eq!((last["kind"].as_str(), last["text"].as_str()), (Some("release"), Some("thread finished")));
    }

    #[tokio::test]
    async fn after_a_tool_call_a_commit_releases_and_news_arrives() {
        let (_d, app) = app();
        let sb = SandboxName::parse("neos").unwrap();
        app.board_claim(&sb, None, "api", &["a".into(), "b".into()]).unwrap();
        app.board_note(&sb, None, "front", sbxw_core::board::Kind::Decision, "cents", None, &[]).unwrap();
        let files = vec!["a".to_string()];
        let (outside, released, news) = app
            .board_after(&sb, None, "api", &[], &["a".into(), "c".into()], Some(("abc1", "Cart", &files)))
            .unwrap();
        assert_eq!(outside, ["c"]);
        assert_eq!(released, ["a"]);
        let texts: Vec<_> = news.iter().filter_map(|e| e.text.as_deref()).collect();
        assert_eq!(texts, ["cents", "api wrote outside its claims"]);
    }
}
