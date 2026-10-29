//! The board: what the threads of a shared Team-mode plan know of each other.
//!
//! In the shared variant every thread edits one worktree, so a merge conflict
//! becomes a live file conflict. The board prevents it with **claims**: a
//! thread claims the files it will change, holds each on a 20-minute lease
//! renewed by every edit, and nobody else may write them meanwhile (the hook
//! in the sandbox asks [`Board::check`] before an Edit, Write or writing Bash
//! command). A lease runs out on its own, is released by the thread, by a
//! commit of the file, by the thread finishing, or by the lead.
//!
//! Beside the claims runs the **feed**: what sbxw saw (claims, releases,
//! commits, refused conflicts) and what agents posted (decisions, questions,
//! handoffs, warnings). Each session reads it as a filtered delta after each
//! tool call, so a decision taken in one thread reaches the others without
//! anyone re-reading everything.
//!
//! One board per sandbox. The book is pure: the daemon persists it and serves
//! it; time is passed in.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// How long a claim lasts without an edit, and what one extension adds.
pub const LEASE_MS: u64 = 20 * 60 * 1000;
/// Past this much held in total, an extension needs the lead's approval.
pub const MAX_HELD_MS: u64 = 2 * 3600 * 1000;
/// The feed keeps this many entries; older ones are dropped.
pub const MAX_ENTRIES: usize = 1000;
/// Files in one call, and characters in one note.
pub const MAX_FILES: usize = 200;
pub const MAX_TEXT: usize = 2000;
/// Entries one delta carries at most; the rest are on `read_board`.
pub const MAX_DELTA: usize = 20;
/// Who the lead is on the board, and who sbxw is.
pub const LEAD: &str = "lead";
pub const SBXW: &str = "sbxw";

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Claim,
    Release,
    Commit,
    Conflict,
    Decision,
    Question,
    Handoff,
    Warning,
}

impl Kind {
    /// The four an agent may post with `post_note`; the rest are sbxw's.
    pub fn is_note(self) -> bool {
        matches!(self, Kind::Decision | Kind::Question | Kind::Handoff | Kind::Warning)
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Claim {
    pub file: String,
    pub owner: String,
    pub since: u64,
    pub until: u64,
    #[serde(default)]
    pub extensions: u32,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Entry {
    pub id: u64,
    pub at: u64,
    pub kind: Kind,
    pub by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

/// A file someone else holds.
#[derive(Clone, Serialize, Debug, PartialEq)]
pub struct Taken {
    pub file: String,
    pub owner: String,
    pub until: u64,
}

#[derive(Clone, Serialize, Debug, PartialEq, Default)]
pub struct ClaimOutcome {
    pub granted: Vec<String>,
    pub taken: Vec<Taken>,
}

#[derive(Clone, Serialize, Debug, PartialEq, Default)]
pub struct ExtendOutcome {
    /// `(file, new expiry)`.
    pub extended: Vec<(String, u64)>,
    /// Past the 2 h cap: the lead was asked.
    pub needs_lead: Vec<String>,
    /// Not held by the caller.
    pub not_held: Vec<String>,
}

fn clip(s: &str) -> String {
    let s = s.trim();
    match s.char_indices().nth(MAX_TEXT) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// A file as the board names it: relative to the repository root, `/`
/// separated, no `.` or `..` components. The hook and the MCP server resolve
/// paths to that form; anything still absolute or escaping is refused.
pub fn normalize_file(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("an empty file name".into());
    }
    if raw.starts_with('/') || raw.contains('\\') || raw.chars().any(char::is_control) {
        return Err(format!("{raw:?}: name files relative to the repository root"));
    }
    let mut parts = Vec::new();
    for p in raw.split('/') {
        match p {
            "" | "." => {}
            ".." => return Err(format!("{raw:?}: a file outside the repository")),
            p => parts.push(p),
        }
    }
    if parts.is_empty() {
        return Err(format!("{raw:?}: name a file, not the repository"));
    }
    Ok(parts.join("/"))
}

/// The files of one call: normalized, each once, at most [`MAX_FILES`].
pub fn normalize_files(raw: &[String]) -> Result<Vec<String>, String> {
    if raw.len() > MAX_FILES {
        return Err(format!("at most {MAX_FILES} files at a time"));
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for f in raw {
        let f = normalize_file(f)?;
        if seen.insert(f.clone()) {
            out.push(f);
        }
    }
    Ok(out)
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct Board {
    pub sandbox: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(default)]
    pub claims: BTreeMap<String, Claim>,
    #[serde(default)]
    pub entries: Vec<Entry>,
    #[serde(default)]
    next_id: u64,
    /// The last entry each session was shown.
    #[serde(default)]
    cursors: HashMap<String, u64>,
    /// The files each session has read: claims on them reach its delta.
    #[serde(default)]
    reads: HashMap<String, BTreeSet<String>>,
}

impl Board {
    pub fn new(sandbox: &str) -> Self {
        Self { sandbox: sandbox.to_string(), ..Self::default() }
    }

    /// The plan the board is for, as the latest caller said.
    pub fn set_plan(&mut self, plan: Option<&str>) {
        if let Some(p) = plan.map(str::trim).filter(|p| !p.is_empty()) {
            self.plan = Some(p.to_string());
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn log(&mut self, now: u64, kind: Kind, by: &str, to: Option<&str>, text: Option<String>, files: Vec<String>, hash: Option<String>) -> Entry {
        self.next_id += 1;
        let e = Entry {
            id: self.next_id,
            at: now,
            kind,
            by: by.to_string(),
            to: to.map(str::to_string),
            text: text.filter(|t| !t.is_empty()),
            files,
            hash,
        };
        self.entries.push(e.clone());
        if self.entries.len() > MAX_ENTRIES {
            let drop = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(..drop);
        }
        e
    }

    /// Drop claims whose lease ran out. True when any did.
    pub fn expire(&mut self, now: u64) -> bool {
        let gone: Vec<Claim> = self.claims.values().filter(|c| c.until <= now).cloned().collect();
        let mut by_owner: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for c in gone {
            self.claims.remove(&c.file);
            by_owner.entry(c.owner).or_default().push(c.file);
        }
        let changed = !by_owner.is_empty();
        for (owner, files) in by_owner {
            self.log(now, Kind::Release, SBXW, Some(&owner), Some("lease expired".into()), files, None);
        }
        changed
    }

    pub fn holder(&self, file: &str, now: u64) -> Option<&Claim> {
        self.claims.get(file).filter(|c| c.until > now)
    }

    /// Claim what is free; what another holds is reported, not taken. A file
    /// already one's own is renewed.
    pub fn claim(&mut self, owner: &str, files: &[String], now: u64) -> Result<ClaimOutcome, String> {
        let files = normalize_files(files)?;
        self.expire(now);
        let mut out = ClaimOutcome::default();
        for f in files {
            match self.holder(&f, now) {
                Some(c) if c.owner != owner => out.taken.push(Taken { file: f, owner: c.owner.clone(), until: c.until }),
                Some(_) => {
                    let c = self.claims.get_mut(&f).expect("held");
                    c.until = c.until.max(now + LEASE_MS);
                    out.granted.push(f);
                }
                None => {
                    self.claims.insert(
                        f.clone(),
                        Claim { file: f.clone(), owner: owner.to_string(), since: now, until: now + LEASE_MS, extensions: 0 },
                    );
                    out.granted.push(f);
                }
            }
        }
        if !out.granted.is_empty() {
            self.log(now, Kind::Claim, owner, None, None, out.granted.clone(), None);
        }
        for t in &out.taken {
            self.log(now, Kind::Conflict, owner, Some(&t.owner), Some("claim refused".into()), vec![t.file.clone()], None);
        }
        Ok(out)
    }

    /// Release the caller's own claims among `files`, or all of them when
    /// `files` is empty. Returns what was released.
    pub fn release(&mut self, owner: &str, files: &[String], why: Option<&str>, now: u64) -> Result<Vec<String>, String> {
        let files = normalize_files(files)?;
        let mine: Vec<String> = self
            .claims
            .values()
            .filter(|c| c.owner == owner && (files.is_empty() || files.contains(&c.file)))
            .map(|c| c.file.clone())
            .collect();
        for f in &mine {
            self.claims.remove(f);
        }
        if !mine.is_empty() {
            self.log(now, Kind::Release, owner, None, why.map(str::to_string), mine.clone(), None);
        }
        Ok(mine)
    }

    /// The lead takes files back from whoever holds them.
    pub fn force_release(&mut self, files: &[String], reason: &str, now: u64) -> Result<Vec<Claim>, String> {
        let files = normalize_files(files)?;
        let mut out = Vec::new();
        for f in files {
            if let Some(c) = self.claims.remove(&f) {
                out.push(c);
            }
        }
        let mut by_owner: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for c in &out {
            by_owner.entry(c.owner.clone()).or_default().push(c.file.clone());
        }
        let text = format!("released by the lead{}", if reason.trim().is_empty() { String::new() } else { format!(": {}", clip(reason)) });
        for (owner, files) in by_owner {
            self.log(now, Kind::Release, LEAD, Some(&owner), Some(text.clone()), files, None);
        }
        Ok(out)
    }

    /// `+20 min` on each of the caller's files, refused for a file that
    /// would then have been held past [`MAX_HELD_MS`]: a question goes to the
    /// lead instead, who may [`approve_extension`](Self::approve_extension).
    pub fn extend(&mut self, owner: &str, files: &[String], now: u64) -> Result<ExtendOutcome, String> {
        let files = normalize_files(files)?;
        self.expire(now);
        let mut out = ExtendOutcome::default();
        let all: Vec<String> = if files.is_empty() {
            self.claims.values().filter(|c| c.owner == owner).map(|c| c.file.clone()).collect()
        } else {
            files
        };
        for f in all {
            match self.claims.get_mut(&f) {
                Some(c) if c.owner == owner => {
                    let until = c.until.max(now) + LEASE_MS;
                    if until - c.since > MAX_HELD_MS {
                        out.needs_lead.push(f);
                    } else {
                        c.until = until;
                        c.extensions += 1;
                        out.extended.push((f, until));
                    }
                }
                _ => out.not_held.push(f),
            }
        }
        if !out.needs_lead.is_empty() {
            self.log(
                now,
                Kind::Question,
                owner,
                Some(LEAD),
                Some("asks to hold these past 2 h: approve with approve_extension, or release them".into()),
                out.needs_lead.clone(),
                None,
            );
        }
        Ok(out)
    }

    /// The lead lets a claim run past the cap: `+20 min` on each file.
    pub fn approve_extension(&mut self, files: &[String], now: u64) -> Result<Vec<(String, u64)>, String> {
        let files = normalize_files(files)?;
        self.expire(now);
        let mut out = Vec::new();
        let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for f in files {
            if let Some(c) = self.claims.get_mut(&f) {
                c.until = c.until.max(now) + LEASE_MS;
                c.extensions += 1;
                owners.entry(c.owner.clone()).or_default().push(f.clone());
                out.push((f, c.until));
            }
        }
        for (owner, files) in owners {
            self.log(now, Kind::Decision, LEAD, Some(&owner), Some("extension approved past 2 h".into()), files, None);
        }
        Ok(out)
    }

    /// Before a write: the files another session holds, each logged as a
    /// conflict addressed to its owner. The caller's own are renewed — an
    /// edit is what keeps a lease alive.
    pub fn check(&mut self, session: &str, files: &[String], now: u64) -> Result<Vec<Taken>, String> {
        let files = normalize_files(files)?;
        self.expire(now);
        let mut taken = Vec::new();
        for f in files {
            match self.claims.get_mut(&f) {
                Some(c) if c.owner == session => c.until = c.until.max(now + LEASE_MS),
                Some(c) => taken.push(Taken { file: f, owner: c.owner.clone(), until: c.until }),
                None => {}
            }
        }
        for t in &taken {
            self.log(now, Kind::Conflict, session, Some(&t.owner), Some("write refused".into()), vec![t.file.clone()], None);
        }
        Ok(taken)
    }

    /// Files the session read: claims on them will reach its delta.
    pub fn record_reads(&mut self, session: &str, files: &[String]) {
        let Ok(files) = normalize_files(files) else { return };
        if files.is_empty() {
            return;
        }
        self.reads.entry(session.to_string()).or_default().extend(files);
    }

    /// Files a command changed: the caller's own claims are renewed, and
    /// those outside them are returned. Each is logged: a warning to the
    /// caller, and a conflict to the owner of a file another session held.
    pub fn record_writes(&mut self, session: &str, files: &[String], now: u64) -> Result<Vec<String>, String> {
        let files = normalize_files(files)?;
        self.expire(now);
        let mut outside = Vec::new();
        let mut trespass: Vec<(String, String)> = Vec::new();
        for f in files {
            match self.claims.get_mut(&f) {
                Some(c) if c.owner == session => c.until = c.until.max(now + LEASE_MS),
                Some(c) => {
                    trespass.push((c.owner.clone(), f.clone()));
                    outside.push(f);
                }
                None => outside.push(f),
            }
        }
        if !outside.is_empty() {
            self.log(now, Kind::Warning, SBXW, Some(session), Some(format!("{session} wrote outside its claims")), outside.clone(), None);
        }
        for (owner, f) in trespass {
            self.log(now, Kind::Conflict, session, Some(&owner), Some("written without the claim".into()), vec![f], None);
        }
        Ok(outside)
    }

    /// A commit: logged with its subject line, and the committer's claims on
    /// its files released.
    pub fn commit(&mut self, session: &str, hash: &str, subject: &str, files: &[String], now: u64) -> Result<Vec<String>, String> {
        let files = normalize_files(files)?;
        let hash = hash.trim();
        if hash.is_empty() || hash.len() > 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("{hash:?} is not a commit hash"));
        }
        let subject = clip(subject.lines().next().unwrap_or(""));
        self.log(now, Kind::Commit, session, None, Some(subject), files.clone(), Some(hash.to_string()));
        let released: Vec<String> = files
            .into_iter()
            .filter(|f| self.claims.get(f).is_some_and(|c| c.owner == session))
            .collect();
        for f in &released {
            self.claims.remove(f);
        }
        if !released.is_empty() {
            self.log(now, Kind::Release, session, None, Some("committed".into()), released.clone(), None);
        }
        Ok(released)
    }

    /// An agent's note: decision, question, handoff or warning.
    pub fn note(&mut self, by: &str, kind: Kind, text: &str, to: Option<&str>, files: &[String], now: u64) -> Result<Entry, String> {
        if !kind.is_note() {
            return Err("a note is a decision, question, handoff or warning".into());
        }
        let text = clip(text);
        if text.is_empty() {
            return Err("a note needs text".into());
        }
        let to = to.map(str::trim).filter(|t| !t.is_empty());
        if kind == Kind::Question && to.is_none() {
            return Err("a question is addressed to a thread or to the lead".into());
        }
        let files = normalize_files(files)?;
        Ok(self.log(now, kind, by, to, Some(text), files, None))
    }

    /// A finished thread holds nothing.
    pub fn release_finished(&mut self, owner: &str, now: u64) -> bool {
        self.release(owner, &[], Some("thread finished"), now).map(|r| !r.is_empty()).unwrap_or(false)
    }

    /// What `session` has not seen yet, filtered to what concerns it:
    /// decisions and warnings for everyone, entries addressed to it, and
    /// claims on files it has read. Its own entries are skipped. Advances its cursor.
    pub fn delta(&mut self, session: &str, now: u64) -> Vec<Entry> {
        self.expire(now);
        let from = self.cursors.get(session).copied().unwrap_or(0);
        let read = self.reads.get(session);
        let mut out: Vec<Entry> = self
            .entries
            .iter()
            .filter(|e| e.id > from && e.by != session)
            .filter(|e| match e.kind {
                Kind::Decision | Kind::Warning => e.to.as_deref().is_none_or(|t| t == session),
                Kind::Claim => read.is_some_and(|r| e.files.iter().any(|f| r.contains(f))),
                _ => e.to.as_deref() == Some(session),
            })
            .cloned()
            .collect();
        if let Some(last) = self.entries.last() {
            self.cursors.insert(session.to_string(), last.id);
        }
        if out.len() > MAX_DELTA {
            out.drain(..out.len() - MAX_DELTA);
        }
        out
    }

    /// The board as the UI and `read_board` see it. Times are unix seconds
    /// there (the book keeps milliseconds); `held_ms` stays milliseconds.
    pub fn view(&self, now: u64) -> Value {
        let s = |ms: u64| ms / 1000;
        let claims: Vec<Value> = self
            .claims
            .values()
            .filter(|c| c.until > now)
            .map(|c| {
                json!({
                    "file": c.file, "owner": c.owner, "since": s(c.since), "until": s(c.until),
                    "extensions": c.extensions, "held_ms": now.saturating_sub(c.since),
                })
            })
            .collect();
        let entries: Vec<Value> = self
            .entries
            .iter()
            .map(|e| {
                let mut v = serde_json::to_value(e).unwrap_or_default();
                v["at"] = s(e.at).into();
                v
            })
            .collect();
        json!({ "sandbox": self.sandbox, "plan": self.plan, "claims": claims, "entries": entries })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60 * 1000;

    fn f(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn files_are_named_from_the_repository_root() {
        assert_eq!(normalize_file("./src//cart.ts").unwrap(), "src/cart.ts");
        assert!(normalize_file("/etc/passwd").is_err());
        assert!(normalize_file("src/../../x").is_err());
        assert!(normalize_file(" ").is_err());
        assert!(normalize_file(".").is_err());
        assert_eq!(normalize_files(&f(&["a", "./a", "b"])).unwrap(), f(&["a", "b"]));
    }

    #[test]
    fn a_claimed_file_is_refused_to_others_and_renewed_for_its_owner() {
        let mut b = Board::new("n");
        let got = b.claim("api", &f(&["src/cart.ts"]), 0).unwrap();
        assert_eq!(got.granted, f(&["src/cart.ts"]));
        let other = b.claim("front", &f(&["src/cart.ts", "src/ui.ts"]), MIN).unwrap();
        assert_eq!(other.granted, f(&["src/ui.ts"]));
        assert_eq!(other.taken[0].owner, "api");

        let taken = b.check("front", &f(&["src/cart.ts"]), 2 * MIN).unwrap();
        assert_eq!(taken, vec![Taken { file: "src/cart.ts".into(), owner: "api".into(), until: LEASE_MS }]);
        let last = b.entries.last().unwrap();
        assert_eq!((last.kind, last.to.as_deref()), (Kind::Conflict, Some("api")));

        assert!(b.check("api", &f(&["src/cart.ts"]), 10 * MIN).unwrap().is_empty());
        assert_eq!(b.claims["src/cart.ts"].until, 10 * MIN + LEASE_MS, "an edit renews the lease");
    }

    #[test]
    fn a_lease_expires_without_an_edit() {
        let mut b = Board::new("n");
        b.claim("api", &f(&["a"]), 0).unwrap();
        assert!(b.holder("a", LEASE_MS - 1).is_some());
        assert!(b.check("front", &f(&["a"]), LEASE_MS).unwrap().is_empty(), "free once the lease ran out");
        assert!(b.claims.is_empty());
        let rel = b.entries.iter().find(|e| e.kind == Kind::Release).unwrap();
        assert_eq!((rel.by.as_str(), rel.to.as_deref()), (SBXW, Some("api")));
        assert!(b.claim("front", &f(&["a"]), LEASE_MS).unwrap().taken.is_empty());
    }

    #[test]
    fn extensions_stop_at_two_hours_unless_the_lead_approves() {
        let mut b = Board::new("n");
        b.claim("api", &f(&["a"]), 0).unwrap();
        // 20 min claimed + 5 extensions = 2 h exactly.
        for i in 0..5 {
            let out = b.extend("api", &f(&["a"]), i * MIN).unwrap();
            assert_eq!(out.extended.len(), 1, "extension {i}");
        }
        assert_eq!(b.claims["a"].until, MAX_HELD_MS);
        let out = b.extend("api", &f(&["a"]), 6 * MIN).unwrap();
        assert_eq!(out.needs_lead, f(&["a"]));
        let q = b.entries.last().unwrap();
        assert_eq!((q.kind, q.to.as_deref()), (Kind::Question, Some(LEAD)));
        assert_eq!(b.extend("front", &f(&["a"]), 6 * MIN).unwrap().not_held, f(&["a"]));

        let ok = b.approve_extension(&f(&["a"]), 7 * MIN).unwrap();
        assert_eq!(ok, vec![("a".to_string(), MAX_HELD_MS + LEASE_MS)]);
        assert_eq!(b.claims["a"].extensions, 6);
    }

    #[test]
    fn releases_by_owner_commit_lead_and_finish() {
        let mut b = Board::new("n");
        b.claim("api", &f(&["a", "b", "c", "d"]), 0).unwrap();
        b.claim("front", &f(&["x"]), 0).unwrap();
        assert!(b.release("front", &f(&["a"]), None, 1).unwrap().is_empty(), "not its own");
        assert_eq!(b.release("api", &f(&["a"]), None, 1).unwrap(), f(&["a"]));
        assert_eq!(b.commit("api", "abc123", "Cart: totals\nbody", &f(&["b", "x"]), 2).unwrap(), f(&["b"]));
        assert!(b.commit("api", "nope!", "", &f(&["c"]), 2).is_err());
        assert_eq!(b.force_release(&f(&["c"]), "stuck", 3).unwrap()[0].owner, "api");
        let c = b.entries.iter().find(|e| e.kind == Kind::Commit).unwrap();
        assert_eq!((c.by.as_str(), c.hash.as_deref(), c.text.as_deref()), ("api", Some("abc123"), Some("Cart: totals")));
        assert!(b.release_finished("api", 4));
        assert!(!b.release_finished("api", 5), "nothing left");
        assert_eq!(b.claims.keys().collect::<Vec<_>>(), ["x"]);
    }

    #[test]
    fn notes_are_the_four_kinds_and_questions_are_addressed() {
        let mut b = Board::new("n");
        assert!(b.note("api", Kind::Claim, "x", None, &[], 0).is_err());
        assert!(b.note("api", Kind::Question, "why?", None, &[], 0).is_err());
        assert!(b.note("api", Kind::Decision, "  ", None, &[], 0).is_err());
        let e = b.note("api", Kind::Question, "which field?", Some("front"), &[], 0).unwrap();
        assert_eq!(e.to.as_deref(), Some("front"));
    }

    #[test]
    fn a_delta_is_filtered_per_session_and_read_once() {
        let mut b = Board::new("n");
        b.record_reads("front", &f(&["src/cart.ts"]));
        b.claim("api", &f(&["src/cart.ts"]), 0).unwrap();
        b.claim("api", &f(&["src/other.ts"]), 0).unwrap();
        b.note("api", Kind::Decision, "prices in cents", None, &[], 1).unwrap();
        b.note("api", Kind::Question, "for db", Some("db"), &[], 2).unwrap();
        b.note("api", Kind::Question, "for front", Some("front"), &[], 3).unwrap();
        b.note("front", Kind::Warning, "mine", None, &[], 4).unwrap();

        let d = b.delta("front", 5);
        let texts: Vec<_> = d.iter().map(|e| (e.kind, e.files.first().cloned(), e.text.clone())).collect();
        assert_eq!(
            texts,
            vec![
                (Kind::Claim, Some("src/cart.ts".into()), None),
                (Kind::Decision, None, Some("prices in cents".into())),
                (Kind::Question, None, Some("for front".into())),
            ]
        );
        assert!(b.delta("front", 6).is_empty(), "each entry once");
        assert_eq!(b.delta("db", 6).len(), 3, "decision, its question, front's warning");
    }

    #[test]
    fn writes_outside_claims_are_flagged_to_the_writer_and_the_owner() {
        let mut b = Board::new("n");
        b.claim("api", &f(&["a"]), 0).unwrap();
        b.claim("front", &f(&["c"]), 0).unwrap();
        assert_eq!(b.record_writes("api", &f(&["a", "b", "c"]), 5 * MIN).unwrap(), f(&["b", "c"]));
        assert_eq!(b.claims["a"].until, 5 * MIN + LEASE_MS);
        let api: Vec<_> = b.delta("api", 6 * MIN).into_iter().map(|e| e.kind).collect();
        assert_eq!(api, [Kind::Warning]);
        let front: Vec<_> = b.delta("front", 6 * MIN).into_iter().map(|e| (e.kind, e.files)).collect();
        assert_eq!(front, [(Kind::Conflict, f(&["c"]))]);
        assert!(b.delta("db", 6 * MIN).is_empty(), "nobody else hears of it");
    }

    #[test]
    fn the_view_has_the_ui_shape_and_the_board_round_trips() {
        let mut b = Board::new("n");
        b.set_plan(Some("p-1"));
        b.claim("api", &f(&["a"]), 0).unwrap();
        b.extend("api", &f(&["a"]), 0).unwrap();
        let v = b.view(10 * MIN);
        assert_eq!(v["plan"], "p-1");
        assert_eq!(v["claims"][0]["until"], 2 * LEASE_MS / 1000, "seconds");
        assert_eq!(v["claims"][0]["held_ms"], 10 * MIN);
        assert_eq!(v["claims"][0]["extensions"], 1);
        assert_eq!(v["entries"][0]["kind"], "claim");
        let again: Board = serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
        assert_eq!(again.claims, b.claims);
        assert_eq!(again.next_id, b.next_id);
    }
}
