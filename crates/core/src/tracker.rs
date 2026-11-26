//! Ticket sync with an external tracker (plan A: GitHub issues): the source
//! a person sets per sandbox, its status, the requests the sync script
//! takes and the answers it gives, the issues as data, and the fixed texts
//! sbxw writes back.
//!
//! The daemon holds no token and has no HTTPS client. Every call to a
//! tracker is made by one script ([`SCRIPT`]), with no agent, run with
//! `node` in the daemon's own sync sandbox, through sbx's MCP gateway where
//! the tracker's MCP server is loaded. This module is pure: it builds the
//! requests, reads the answers (as data), and writes the texts.
//!
//! What goes to the tracker is fixed here and nowhere else: one comment
//! starting with `[sbxw]` and one `sbxw:` label per column, written from
//! the column, a branch name and a commit hash. Never a ticket's words, never
//! an agent's.
//!
//! Tracker-neutral but for [`Tracker`]: GitLab and Jira come later through
//! the same requests.

use crate::tickets::{self, Column, Mark};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The sync script, piped to `node -` in the sync sandbox on each run.
pub const SCRIPT: &str = include_str!("../../../assets/tracker-sync.js");

/// The marker the script's answer line starts with (its last line).
pub const ANSWER: &str = "SBXW-TRACKER ";

/// The filter label an issue carries to be imported, by default.
pub const DEFAULT_LABEL: &str = "sbxw";
/// What every state label sbxw writes starts with; the script removes the
/// others of that prefix when it sets one.
pub const LABEL_PREFIX: &str = "sbxw:";
/// What every comment sbxw writes starts with.
pub const COMMENT_PREFIX: &str = "[sbxw]";

/// How often a source is polled.
pub const POLL_MS: u64 = 5 * 60 * 1000;
/// Pages of issues one sync reads at most (the script's pages).
pub const MAX_PAGES: u32 = 20;
/// An attachment, at most.
pub const MAX_ATTACHMENT: u64 = 8 << 20;
/// Attachments one issue brings at most.
pub const MAX_ATTACHMENTS: usize = 10;
/// Labels kept of one issue.
pub const MAX_LABELS: usize = 20;
/// Characters of one label.
pub const MAX_LABEL: usize = 50;
/// Characters of a repository (`owner/repo`).
pub const MAX_REPO: usize = 140;
/// Characters of a link.
pub const MAX_URL: usize = 400;
/// Write-backs one sandbox queues at most; past it the oldest of a ticket
/// that has a newer one go first (a newer one says all the older did).
pub const MAX_QUEUE: usize = 2000;

/// The trackers sbxw knows. The only tracker-specific value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Tracker {
    #[default]
    Github,
}

impl Tracker {
    pub fn id(self) -> &'static str {
        match self {
            Tracker::Github => "github",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Tracker::Github => "GitHub",
        }
    }
    /// The name of the MCP server the person registers in Setup, MCP, and
    /// which the sync sandbox loads.
    pub fn mcp_server(self) -> &'static str {
        match self {
            Tracker::Github => "github",
        }
    }
    /// What the sync sandbox may reach besides the gateway: where the
    /// issues' attachments are served: the script's own download hosts.
    /// Unverified against a real sbx.
    pub fn hosts(self) -> &'static [&'static str] {
        match self {
            Tracker::Github => &["github.com", "api.github.com", "objects.githubusercontent.com", "private-user-images.githubusercontent.com", "user-images.githubusercontent.com", "raw.githubusercontent.com"],
        }
    }
    /// An issue's key: `owner/repo#12`.
    pub fn key(self, repo: &str, number: u64) -> String {
        match self {
            Tracker::Github => format!("{repo}#{number}"),
        }
    }
    /// Whether `url` is a link of this tracker (an issue's, an attachment's).
    pub fn url_ok(self, url: &str) -> bool {
        if url.len() > MAX_URL || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return false;
        }
        let Some(rest) = url.strip_prefix("https://") else { return false };
        let host = rest.split(['/', '?', '#']).next().unwrap_or("").to_ascii_lowercase();
        match self {
            Tracker::Github => host == "github.com" || host.ends_with(".githubusercontent.com"),
        }
    }
    /// A repository as the person types it, checked: `owner/repo`.
    pub fn parse_repo(self, s: &str) -> Result<String, String> {
        let s = s.trim().trim_end_matches(".git").trim_matches('/');
        // The sync script's own rules, so a source it would refuse is refused
        // here, when it is set: an owner of 1 to 39 letters, digits and dashes
        // starting with a letter or digit; a name of 1 to 100 letters, digits,
        // dots, dashes and underscores, not starting with a dot.
        let ok_owner = |p: &str| !p.is_empty() && p.len() <= 39 && !p.starts_with('-') && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        let ok_name = |p: &str| !p.is_empty() && p.len() <= 100 && !p.starts_with('.') && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        match self {
            Tracker::Github => match s.split_once('/') {
                Some((o, r)) if ok_owner(o) && ok_name(r) && s.len() <= MAX_REPO => Ok(format!("{o}/{r}")),
                _ => Err("a GitHub repository is owner/name, for example acme/web".into()),
            },
        }
    }
}

/// The GitHub repository `origin` points at, from `git remote get-url
/// origin`: `https://github.com/o/r(.git)`, `git@github.com:o/r(.git)`,
/// `ssh://git@github.com/o/r(.git)`. Only pre-fills the source.
pub fn detect_github(url: &str) -> Option<String> {
    let u = url.trim();
    let rest = u
        .strip_prefix("https://github.com/")
        .or_else(|| u.strip_prefix("http://github.com/"))
        .or_else(|| u.strip_prefix("git@github.com:"))
        .or_else(|| u.strip_prefix("ssh://git@github.com/"))
        .or_else(|| u.strip_prefix("ssh://git@github.com:22/"))
        .or_else(|| u.strip_prefix("git://github.com/"))
        .or_else(|| {
            // https://user@github.com/o/r
            let r = u.strip_prefix("https://")?;
            let (cred, path) = r.split_once('@')?;
            (!cred.contains('/')).then_some(())?;
            path.strip_prefix("github.com/")
        })?;
    let parts: Vec<&str> = rest.trim_end_matches('/').splitn(3, '/').collect();
    if parts.len() != 2 {
        return None;
    }
    Tracker::Github.parse_repo(&format!("{}/{}", parts[0], parts[1])).ok()
}

/// A source as the person sets it for one sandbox.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Source {
    pub tracker: Tracker,
    pub repo: String,
    pub label: String,
    pub auto_close: bool,
}

impl Default for Source {
    fn default() -> Self {
        Source { tracker: Tracker::Github, repo: String::new(), label: DEFAULT_LABEL.into(), auto_close: false }
    }
}

/// What `POST /api/tickets/:sandbox/source` takes.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct SourceDraft {
    pub tracker: Option<Tracker>,
    pub repo: Option<String>,
    pub label: Option<String>,
    pub auto_close: Option<bool>,
}

impl SourceDraft {
    pub fn check(self) -> Result<Source, String> {
        let tracker = self.tracker.unwrap_or_default();
        let repo = tracker.parse_repo(self.repo.as_deref().unwrap_or(""))?;
        let label = self.label.as_deref().map(str::trim).filter(|l| !l.is_empty()).unwrap_or(DEFAULT_LABEL).to_string();
        if label.chars().count() > MAX_LABEL || label.chars().any(|c| c.is_control() || matches!(c, ',' | '"')) {
            return Err(format!("a filter label is at most {MAX_LABEL} characters, with no comma or quote"));
        }
        if label.starts_with(LABEL_PREFIX) {
            return Err(format!("labels starting with {LABEL_PREFIX} are sbxw's own: pick another filter label"));
        }
        Ok(Source { tracker, repo, label, auto_close: self.auto_close.unwrap_or(false) })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SyncState {
    #[default]
    Off,
    Ok,
    Syncing,
    Failed,
}

/// Why a run failed. The first six are the script's; the rest the daemon's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Auth,
    MissingServer,
    RateLimit,
    NotFound,
    Network,
    Unsupported,
    /// The run took longer than its bound.
    Timeout,
    /// No answer line, or one that does not parse.
    BadAnswer,
    /// The sync sandbox could not be created, started or reached.
    Sandbox,
}

impl ErrorKind {
    fn parse(s: &str) -> ErrorKind {
        match s {
            "auth" => ErrorKind::Auth,
            "missing_server" => ErrorKind::MissingServer,
            "rate_limit" => ErrorKind::RateLimit,
            "not_found" => ErrorKind::NotFound,
            "network" => ErrorKind::Network,
            "unsupported" => ErrorKind::Unsupported,
            _ => ErrorKind::BadAnswer,
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            ErrorKind::Auth => "auth",
            ErrorKind::MissingServer => "missing_server",
            ErrorKind::RateLimit => "rate_limit",
            ErrorKind::NotFound => "not_found",
            ErrorKind::Network => "network",
            ErrorKind::Unsupported => "unsupported",
            ErrorKind::Timeout => "timeout",
            ErrorKind::BadAnswer => "bad_answer",
            ErrorKind::Sandbox => "sandbox",
        }
    }
    /// What the person can do, one sentence. `until`: the rate limit's end
    /// as people read it.
    pub fn hint(self, t: Tracker, until: Option<&str>) -> String {
        let s = t.mcp_server();
        let name = t.title();
        match self {
            ErrorKind::Auth => format!("Sign in to {name} again in Setup, MCP (server '{s}')."),
            ErrorKind::MissingServer => format!("Add the {name} MCP server in Setup, MCP, named '{s}', and sign in to it."),
            ErrorKind::RateLimit => match until {
                Some(u) => format!("{name} limits the requests: sbxw tries again after {u}."),
                None => format!("{name} limits the requests: sbxw tries again later."),
            },
            ErrorKind::NotFound => format!("{name} does not show this repository to your account: check its name, and that your sign-in can read it."),
            ErrorKind::Network => format!("{name} could not be reached: sbxw tries again at the next sync."),
            ErrorKind::Unsupported => format!("The {name} MCP server lacks a tool sbxw needs: update it in Setup, MCP."),
            ErrorKind::Timeout => "The sync took too long: sbxw tries again at the next sync.".into(),
            ErrorKind::BadAnswer => "The sync script gave no answer sbxw can read: update sbxw, or tell its maintainers.".into(),
            ErrorKind::Sandbox => "sbxw's sync sandbox (sbxw-sync) could not run: check sbx, then press Refresh.".into(),
        }
    }
    /// The Board warning posted once per failure kind.
    pub fn warning(self, t: Tracker, repo: &str) -> String {
        format!("Ticket sync with {} ({repo}) failed: {}", t.title(), self.hint(t, None))
    }
}

/// Where a source stands.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Status {
    pub state: SyncState,
    pub last_ok_ms: Option<u64>,
    pub error: Option<ErrorKind>,
    pub hint: Option<String>,
    /// A rate limit's end: nothing is asked before it.
    pub retry_at_ms: Option<u64>,
    /// The failure kinds already posted on the Board since the last success.
    pub warned: Vec<ErrorKind>,
    /// What the last probe said: how the script reaches the tools
    /// (`direct`, `mcp-exec`), and each operation's tool. Data, shown only.
    pub via: Option<String>,
    pub ops: std::collections::BTreeMap<String, String>,
    pub probed_ms: Option<u64>,
}

impl Status {
    /// Keeps what a probe answer says, cleaned and bounded.
    pub fn set_probe(&mut self, v: &Value, now: u64) {
        self.via = v.get("via").and_then(Value::as_str).map(|s| tickets::clean(s, 40, false)).filter(|s| !s.is_empty());
        self.ops = v
            .get("ops")
            .and_then(Value::as_object)
            .map(|m| m.iter().take(20).map(|(k, v)| (tickets::clean(k, 40, false), tickets::clean(v.as_str().unwrap_or(""), 80, false))).collect())
            .unwrap_or_default();
        self.probed_ms = Some(now);
    }
}

/// A run that failed, as data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub kind: ErrorKind,
    /// For the log only, cleaned and short; never a ticket's words.
    pub message: String,
    pub retry_after_s: Option<u64>,
}

impl Failure {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Failure {
        Failure { kind, message: tickets::clean(&message.into(), 300, false), retry_after_s: None }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind.id(), self.message)
    }
}

/// The script's answer from its whole stdout: the last line that starts with
/// [`ANSWER`]. `{ok: true, ...}` is the answer; `{ok: false, error}` a failure.
pub fn parse_answer(stdout: &str) -> Result<Value, Failure> {
    let line = stdout.lines().rev().find_map(|l| l.trim_end().strip_prefix(ANSWER));
    let Some(line) = line else { return Err(Failure::new(ErrorKind::BadAnswer, "no answer line")) };
    let v: Value = serde_json::from_str(line).map_err(|_| Failure::new(ErrorKind::BadAnswer, "the answer line is not JSON"))?;
    match v.get("ok").and_then(Value::as_bool) {
        Some(true) => Ok(v),
        Some(false) => {
            let e = v.get("error").cloned().unwrap_or(Value::Null);
            let kind = ErrorKind::parse(e.get("kind").and_then(Value::as_str).unwrap_or(""));
            let mut f = Failure::new(kind, e.get("message").and_then(Value::as_str).unwrap_or(""));
            f.retry_after_s = e.get("retry_after_s").and_then(Value::as_u64).map(|s| s.min(24 * 3600));
            Err(f)
        }
        None => Err(Failure::new(ErrorKind::BadAnswer, "the answer has no ok")),
    }
}

// ── Requests ──────────────────────────────────────────────────────────────

pub fn probe(t: Tracker) -> Value {
    json!({ "op": "probe", "tracker": t })
}
pub fn list(s: &Source, page: Value) -> Value {
    json!({ "op": "list", "tracker": s.tracker, "repo": s.repo, "label": s.label, "page": page })
}
pub fn comment(t: Tracker, key: &str, comment_id: Option<&str>, body: &str) -> Value {
    json!({ "op": "comment", "tracker": t, "key": key, "comment_id": comment_id, "body": body })
}
pub fn label(t: Tracker, key: &str, add: &str) -> Value {
    json!({ "op": "label", "tracker": t, "key": key, "add": add, "remove_prefix": LABEL_PREFIX })
}
pub fn close(t: Tracker, key: &str) -> Value {
    json!({ "op": "close", "tracker": t, "key": key })
}
pub fn fetch(t: Tracker, url: &str) -> Value {
    json!({ "op": "fetch", "tracker": t, "url": url, "max_bytes": MAX_ATTACHMENT })
}

/// Where the script writes a fetched file: a path under it, checked before
/// it is read out or removed.
pub const FETCH_DIR: &str = "/tmp/sbxw-tracker/";

/// A comment id as the script gives it (a number or a string), checked.
pub fn comment_id(v: &Value) -> Option<String> {
    let s = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    (!s.is_empty() && s.len() <= 100 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '=' | ':'))).then_some(s)
}

pub fn fetch_path_ok(p: &str) -> bool {
    p.starts_with(FETCH_DIR) && p.len() < 300 && !p.contains("..") && p[FETCH_DIR.len()..].chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '/'))
}

// ── Issues, as data ───────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Upstream {
    #[default]
    Open,
    Closed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Attachment {
    pub name: String,
    pub url: String,
}

/// One issue as the script lists it, cleaned by [`Issue::cleaned`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Issue {
    pub key: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub state: Upstream,
    pub url: String,
    pub updated_at: String,
    pub attachments: Vec<Attachment>,
}

impl Issue {
    /// The issue as sbxw keeps it, or `None` when it is not one of `s`'s
    /// (its key must be the source's repository and its number). Texts are
    /// cleaned and clipped by the ticket limits; links are the tracker's or
    /// dropped.
    pub fn cleaned(v: &Value, s: &Source) -> Option<Issue> {
        let raw: Issue = serde_json::from_value(v.clone()).ok()?;
        if raw.number == 0 || raw.key != s.tracker.key(&s.repo, raw.number) {
            return None;
        }
        let title = tickets::clean(&raw.title, tickets::MAX_TITLE, false);
        let mut labels: Vec<String> = raw.labels.iter().map(|l| tickets::clean(l, MAX_LABEL, false)).filter(|l| !l.is_empty()).collect();
        labels.dedup();
        labels.truncate(MAX_LABELS);
        let attachments = raw
            .attachments
            .iter()
            .filter(|a| s.tracker.url_ok(&a.url))
            .take(MAX_ATTACHMENTS)
            .map(|a| Attachment { name: tickets::shown_name(&a.name), url: a.url.clone() })
            .collect();
        Some(Issue {
            key: raw.key,
            number: raw.number,
            title: if title.is_empty() { format!("Issue #{}", raw.number) } else { title },
            body: tickets::clean(&raw.body, tickets::MAX_DESCRIPTION, true),
            labels,
            state: raw.state,
            url: if s.tracker.url_ok(&raw.url) { raw.url } else { String::new() },
            updated_at: tickets::clean(&raw.updated_at, 40, false),
            attachments,
        })
    }

    pub fn has_label(&self, l: &str) -> bool {
        self.labels.iter().any(|x| x.eq_ignore_ascii_case(l))
    }
}

// ── Write-backs ───────────────────────────────────────────────────────────

/// The state label for a column; Backlog has none (nothing to say yet).
pub fn state_label(c: Column) -> Option<&'static str> {
    match c {
        Column::Backlog => None,
        Column::Planned => Some("sbxw:planned"),
        Column::Progress => Some("sbxw:in-progress"),
        Column::Test => Some("sbxw:in-test"),
        Column::Ready => Some("sbxw:ready"),
        Column::Merged => Some("sbxw:merged"),
    }
}

/// A branch name as written back: `sbxw/<thread>` when the thread's name is
/// one sbxw makes (lower-case letters, digits, dashes), else nothing.
fn branch(thread: Option<&str>) -> Option<String> {
    let t = thread?;
    (!t.is_empty() && t.len() <= 64 && t.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')).then(|| format!("sbxw/{t}"))
}

/// A commit as written back: its first 9 hex digits, or nothing.
fn short(commit: Option<&str>) -> Option<String> {
    let c = commit?;
    (c.len() >= 7 && c.chars().all(|x| x.is_ascii_hexdigit())).then(|| c[..c.len().min(9)].to_ascii_lowercase())
}

/// The one comment sbxw keeps on an issue, for a column. Fixed English,
/// from the column, the marks, a branch name and a commit only.
pub fn comment_text(c: Column, marks: &[Mark], thread: Option<&str>, commit: Option<&str>) -> Option<String> {
    let on = branch(thread).map(|b| format!(" on branch {b}")).unwrap_or_default();
    let text = match c {
        Column::Backlog => return None,
        Column::Planned => "planned in sbxw".to_string(),
        Column::Progress if marks.contains(&Mark::Blocked) => format!("in progress{on}, waiting for an answer"),
        Column::Progress if marks.contains(&Mark::Failed) => format!("in progress{on}, the work stopped on an error"),
        Column::Progress => format!("in progress{on}"),
        Column::Test => format!("in test{on}"),
        Column::Ready => format!("ready to merge{on}"),
        Column::Merged if marks.contains(&Mark::Dismissed) => "closed in sbxw, not merged".to_string(),
        Column::Merged if marks.contains(&Mark::BranchGone) => format!("done{on}, the branch no longer exists"),
        Column::Merged if marks.contains(&Mark::MergedOutside) => format!("merged outside sbxw{on}"),
        Column::Merged => match short(commit) {
            Some(s) => format!("merged in {s}"),
            None => "merged".to_string(),
        },
    };
    Some(format!("{COMMENT_PREFIX} {text}"))
}

/// Whether a column change closes the issue, with auto-close on: a real
/// merge, by sbxw or outside it; never a dismissed plan nor a gone branch.
pub fn closes(c: Column, marks: &[Mark]) -> bool {
    c == Column::Merged && !marks.iter().any(|m| matches!(m, Mark::Dismissed | Mark::BranchGone))
}

/// One write-back waiting: what to set on one issue.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WriteBack {
    pub n: u32,
    pub key: String,
    pub column: Column,
    pub label: String,
    pub comment: String,
    pub close: bool,
    pub queued_ms: u64,
    pub attempts: u32,
    /// Not tried before this.
    pub next_ms: u64,
    pub error: Option<ErrorKind>,
    /// The steps already done (`comment`, `label`), so a retry does not do
    /// them twice.
    pub done: Vec<String>,
}

/// The wait before the next try after `attempts` failures: 30 s doubling to
/// 30 min.
pub fn backoff_ms(attempts: u32) -> u64 {
    (30_000u64 << attempts.min(6)).min(30 * 60 * 1000)
}

/// One sandbox's sync: `state/tickets/<sandbox>/tracker.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sync {
    pub source: Option<Source>,
    pub status: Status,
    pub last_poll_ms: u64,
    pub queue: Vec<WriteBack>,
    /// sbxw's comment on each issue (key to id), kept here too so a ticket
    /// deleted meanwhile does not make a second comment.
    pub comments: std::collections::BTreeMap<String, String>,
}

impl Sync {
    /// Queues a write-back; a full queue drops the oldest item of a ticket
    /// that has a newer one queued, never a ticket's last.
    pub fn push(&mut self, w: WriteBack) {
        self.queue.push(w);
        while self.queue.len() > MAX_QUEUE {
            let Some(i) = (0..self.queue.len()).find(|&i| self.queue[i + 1..].iter().any(|x| x.n == self.queue[i].n)) else { break };
            self.queue.remove(i);
        }
    }
}

#[cfg(test)]
mod tests;
