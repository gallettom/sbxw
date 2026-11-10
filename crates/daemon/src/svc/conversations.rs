//! The concierge's conversations, kept as text for a week at most (Q19).
//!
//! One JSON file per conversation under `~/.sbxw/state/concierge/`, written by
//! the page after each turn; audio never reaches here. One not updated within
//! `[voice] retention_days` (max 7) is deleted whenever the list is read or a
//! conversation saved: no timer, no background task.

use crate::app::{now_ms, App};
use crate::svc::voice::Turn;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Conversation {
    pub id: String,
    #[serde(default)]
    pub sandbox: Option<String>,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub turns: Vec<TurnRecord>,
}

/// A turn as stored (the page's `{role, content}`).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TurnRecord {
    pub role: String,
    pub content: String,
}

impl From<&Turn> for TurnRecord {
    fn from(t: &Turn) -> Self {
        TurnRecord { role: t.role.clone(), content: t.content.clone() }
    }
}

/// What the history list shows.
#[derive(Serialize, Debug)]
pub struct Listed {
    pub id: String,
    pub sandbox: Option<String>,
    pub updated_ms: u64,
    pub title: String,
    pub turns: usize,
}

const DAY_MS: u64 = 24 * 60 * 60 * 1000;

/// An id the page made (`c-<hex>`), safe as a file name.
pub fn valid_id(id: &str) -> bool {
    id.len() <= 40 && id.starts_with("c-") && id[2..].chars().all(|c| c.is_ascii_alphanumeric()) && id.len() > 2
}

fn title_of(c: &Conversation) -> String {
    let first = c.turns.iter().find(|t| t.role == "user").map(|t| t.content.as_str()).unwrap_or("");
    let one_line = first.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > 70 {
        format!("{}…", one_line.chars().take(70).collect::<String>())
    } else {
        one_line
    }
}

/// Delete what is older than `days`; return what remains, newest first.
pub fn prune_and_list(dir: &Path, days: u32, now: u64) -> Vec<Conversation> {
    let cutoff = now.saturating_sub(days as u64 * DAY_MS);
    let mut out = Vec::new();
    for p in std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()) {
        if p.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let c = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<Conversation>(&t).ok());
        match c {
            Some(c) if c.updated_ms >= cutoff => out.push(c),
            // Expired, or unreadable: either way not something to keep.
            _ => {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    out.sort_by_key(|c| std::cmp::Reverse(c.updated_ms));
    out
}

impl App {
    fn conversations_dir(&self) -> PathBuf {
        self.store.paths.state().join("concierge")
    }

    fn retention_days(&self) -> u32 {
        self.cfg.voice.retention_days.clamp(1, sbxw_core::voice::MAX_RETENTION_DAYS)
    }

    pub fn conversations(&self) -> Vec<Listed> {
        prune_and_list(&self.conversations_dir(), self.retention_days(), now_ms())
            .into_iter()
            .map(|c| Listed { title: title_of(&c), turns: c.turns.len(), id: c.id, sandbox: c.sandbox, updated_ms: c.updated_ms })
            .collect()
    }

    pub fn conversation(&self, id: &str) -> Result<Conversation, String> {
        if !valid_id(id) {
            return Err("bad conversation id".into());
        }
        prune_and_list(&self.conversations_dir(), self.retention_days(), now_ms())
            .into_iter()
            .find(|c| c.id == id)
            .ok_or_else(|| "no such conversation (older than the retention?)".into())
    }

    /// Save the page's transcript of `id` (created on first save).
    pub fn conversation_save(&self, id: &str, sandbox: Option<String>, turns: &[Turn]) -> Result<(), String> {
        if !valid_id(id) {
            return Err("bad conversation id".into());
        }
        if turns.len() > crate::svc::voice::MAX_TURNS || turns.iter().map(|t| t.content.len()).sum::<usize>() > crate::svc::voice::MAX_CHARS {
            return Err("the conversation is too long to keep".into());
        }
        let dir = self.conversations_dir();
        let path = dir.join(format!("{id}.json"));
        let now = now_ms();
        let created_ms = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<Conversation>(&t).ok())
            .map(|c| c.created_ms)
            .unwrap_or(now);
        let c = Conversation { id: id.into(), sandbox, created_ms, updated_ms: now, turns: turns.iter().map(TurnRecord::from).collect() };
        let json = serde_json::to_vec_pretty(&c).map_err(|e| e.to_string())?;
        crate::store::write_atomic(&path, &json).map_err(|e| format!("{e:#}"))?;
        prune_and_list(&dir, self.retention_days(), now);
        Ok(())
    }

    /// Delete one conversation, or (`None`) every one.
    pub fn conversation_remove(&self, id: Option<&str>) -> Result<usize, String> {
        let dir = self.conversations_dir();
        match id {
            Some(id) if !valid_id(id) => Err("bad conversation id".into()),
            Some(id) => {
                let p = dir.join(format!("{id}.json"));
                Ok(std::fs::remove_file(&p).map(|_| 1).unwrap_or(0))
            }
            None => {
                let mut n = 0;
                for p in std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()) {
                    if p.extension().is_some_and(|x| x == "json") && std::fs::remove_file(&p).is_ok() {
                        n += 1;
                    }
                }
                Ok(n)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv(id: &str, updated_ms: u64) -> Conversation {
        Conversation {
            id: id.into(),
            sandbox: None,
            created_ms: updated_ms,
            updated_ms,
            turns: vec![TurnRecord { role: "user".into(), content: "  une   idée\nde mode clair ".into() }],
        }
    }

    #[test]
    fn a_week_old_conversation_is_deleted_when_the_list_is_read() {
        let d = tempfile::tempdir().unwrap();
        let now = 100 * DAY_MS;
        for c in [conv("c-new", now - DAY_MS), conv("c-old", now - 8 * DAY_MS)] {
            std::fs::write(d.path().join(format!("{}.json", c.id)), serde_json::to_vec(&c).unwrap()).unwrap();
        }
        std::fs::write(d.path().join("c-junk.json"), b"{").unwrap();
        let left = prune_and_list(d.path(), 7, now);
        assert_eq!(left.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["c-new"]);
        assert!(!d.path().join("c-old.json").exists(), "expired files are gone, not hidden");
        assert!(!d.path().join("c-junk.json").exists());
        assert_eq!(title_of(&left[0]), "une idée de mode clair");
    }

    #[test]
    fn ids_are_file_name_safe() {
        assert!(valid_id("c-1a2b3c"));
        assert!(!valid_id("c-"));
        assert!(!valid_id("../x"));
        assert!(!valid_id("c-a/b"));
        assert!(!valid_id("x-123"));
    }
}
