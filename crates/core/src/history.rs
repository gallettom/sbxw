//! A session's conversation, as a terminal shows it: the person's prompts, the
//! agent's replies, the steps between them, the pending question at the end.
//!
//! Two sources feed one log per session (see `docs/session-history.md`):
//!
//! * **Hook events** ([`HistoryBook::ingest_event`]): what is live (a
//!   question, a permission prompt, the end), and for a hook too old to read
//!   the transcript, everything it can say (prompts, tool calls and results,
//!   the final reply). Such a log is `partial`.
//! * **The transcript's tail** ([`HistoryBook::ingest_transcript`]): the
//!   status hook (v3) reads its own session's transcript incrementally and
//!   posts summarised items; the daemon checks the byte offsets line up.
//!
//! **In memory only.** Never written to disk, never logged (counts and sizes
//! at most), never in fixtures. Every text is masked on ingest, before it is
//! stored, with the person's anonymise rules (names they listed, and the
//! email, phone, IBAN and key shapes). Excluded always: thinking, images
//! (a placeholder says one was there), tool results past their clipped head,
//! system prompts. Every bound is below, in [`Bounds`].

use crate::agents::{questions_from_ask, HookEvent, Question};
use crate::anonymise::{mask, Rules};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};

/// The status hook version that reads the transcript.
pub const HOOK_VERSION_TRANSCRIPT: u64 = 3;

/// Every bound of the history, in one place.
pub mod limits {
    /// A prompt, an assistant text, an injected turn, a compaction summary:
    /// at most this many bytes (cut at a character boundary).
    pub const TEXT_MAX: usize = 16 * 1024;
    /// A tool result's head: this many characters, nothing past it.
    pub const RESULT_HEAD_MAX: usize = 2000;
    /// A tool call's one-line summary.
    pub const SUMMARY_MAX: usize = 160;
    /// Who an injected turn is from.
    pub const FROM_MAX: usize = 80;
    /// Files named on one prompt, and each name's length in characters.
    pub const FILES_MAX: usize = 32;
    pub const FILE_NAME_MAX: usize = crate::uploads::NAME_MAX;
    /// Items kept per session; older ones are dropped and the log says so.
    pub const RING_MAX: usize = 400;
    /// All sessions' logs together; past it, the oldest idle logs go first.
    pub const TOTAL_MAX: usize = 64 * 1024 * 1024;
    /// A session's log is kept this long after it ended.
    pub const ENDED_TTL_MS: u64 = 60 * 60 * 1000;
    /// A page of the route: default and maximum.
    pub const PAGE_DEFAULT: usize = 100;
    pub const PAGE_MAX: usize = 200;
    /// One transcript post from a hook: bytes of body and items.
    pub const POST_MAX_BYTES: usize = 512 * 1024;
    pub const POST_ITEMS_MAX: usize = 400;
    /// A UI reading a session's history hears its `history` nudges this long.
    pub const SUBSCRIPTION_MS: u64 = 120_000;
}

/// The bounds a book runs with: [`limits`], smaller in tests.
#[derive(Clone, Debug)]
pub struct Bounds {
    pub text_max: usize,
    pub ring_max: usize,
    pub total_max: usize,
    pub ended_ttl_ms: u64,
}

impl Default for Bounds {
    fn default() -> Self {
        Bounds {
            text_max: limits::TEXT_MAX,
            ring_max: limits::RING_MAX,
            total_max: limits::TOTAL_MAX,
            ended_ttl_ms: limits::ENDED_TTL_MS,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// The person's words (or a command they typed: `/model`, `! ls`).
    UserPrompt,
    AssistantText,
    ToolCall,
    ToolResult,
    /// A multiple-choice question (`AskUserQuestion`), with its steps.
    Question,
    /// Claude Code asks the person to allow a tool.
    Permission,
    /// Something happened that is nobody's words: a start, an end, an
    /// interruption, an image left out, an API error.
    Notice,
    CompactMarker,
    /// A user turn the harness wrote: a message from another session, a task
    /// notification, a system reminder, a command's output, a hook's output.
    /// Never the person's words; `from` says who when it can be read.
    Injected,
}

#[derive(Clone, PartialEq, Serialize, Debug)]
pub struct ToolCall {
    pub name: String,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum ResultStatus {
    Ok,
    Error,
}

#[derive(Clone, PartialEq, Serialize, Debug)]
pub struct ToolResult {
    pub status: ResultStatus,
    /// The whole result's size, in characters, as the hook measured it.
    pub size: u64,
    /// Its first [`limits::RESULT_HEAD_MAX`] characters; empty when it was
    /// too large for the hook to read or held only an image.
    pub head: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Clone, PartialEq, Serialize, Debug)]
pub struct Item {
    pub seq: u64,
    /// Milliseconds since the epoch: the transcript's timestamp, else arrival.
    pub at: u64,
    pub kind: Kind,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<ToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<ToolResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<Question>>,
    /// Files the person sent with a prompt: base names only, from sbxw's own
    /// attachment block (`uploads::with_attachments`), split off the text.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// The text was cut at the cap.
    #[serde(skip_serializing_if = "is_false")]
    pub clipped: bool,
    /// The final reply as `Stop` gave it, before the transcript has it: it
    /// is replaced by the transcript's own items when they arrive.
    #[serde(skip_serializing_if = "is_false")]
    pub provisional: bool,
    /// An injected turn that is a message: from another session (a lead, a
    /// thread, an agent: `<cross-session-message>`) or a line sbxw typed. Every
    /// other injected turn is the harness's own output (a `!` command's, a
    /// slash command's, a hook's, a task notification, a system reminder).
    /// Not on the wire: the remote view keeps messages and folds the rest.
    #[serde(skip)]
    pub message: bool,
}

impl Item {
    fn new(kind: Kind, at: u64) -> Self {
        Item { seq: 0, at, kind, text: String::new(), tool: None, result: None, from: None, steps: None, files: Vec::new(), clipped: false, provisional: false, message: false }
    }
    /// Roughly what it holds, for the memory ceiling.
    fn weight(&self) -> usize {
        let mut n = 96 + self.text.len() + self.files.iter().map(|f| 24 + f.len()).sum::<usize>();
        if let Some(t) = &self.tool {
            n += t.name.len() + t.summary.len() + t.id.as_ref().map_or(0, String::len);
        }
        if let Some(r) = &self.result {
            n += r.head.len() + r.id.as_ref().map_or(0, String::len);
        }
        n += self.from.as_ref().map_or(0, String::len);
        if let Some(steps) = &self.steps {
            n += steps.iter().map(|q| q.text.len() + q.options.iter().chain(&q.context).map(String::len).sum::<usize>()).sum::<usize>();
        }
        n
    }
}

/// Where a log's conversation comes from.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Source {
    /// Hook events only: a hook older than v3, or one that could not read
    /// its transcript.
    #[default]
    Events,
    /// The transcript's tail (hook v3).
    Transcript,
}

#[derive(Debug, Default)]
struct Log {
    items: VecDeque<Item>,
    next_seq: u64,
    truncated_before: bool,
    bytes: usize,
    source: Source,
    hook_version: u64,
    /// Why a v3 hook could not read its transcript.
    no_transcript: Option<String>,
    /// The transcript byte offset ingested up to.
    transcript_next: Option<u64>,
    ended_ms: Option<u64>,
    last_ms: u64,
    subscribed_until: u64,
    /// The last tool a `PreToolUse` announced: what a permission prompt is about.
    pending_tool: Option<ToolCall>,
    /// Which numbering the items carry: new when the log is made (so after a
    /// daemon restart too) and at each reset. A reader holding items of
    /// another epoch drops them and reads the last page again.
    epoch: u64,
}

/// One page of a session's log, newest page first.
#[derive(Serialize, Debug, PartialEq)]
pub struct Page {
    /// Oldest first within the page.
    pub items: Vec<Item>,
    /// Pass as `before` for the page older than this one; absent at the start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_before: Option<u64>,
    /// The newest seq of the log (0 when empty).
    pub latest_seq: u64,
    /// Items older than the oldest kept were dropped (the ring, the memory
    /// ceiling, a backfill that started from the transcript's tail).
    pub truncated_before: bool,
    /// The log is not the whole conversation; `partial_reason` says why.
    pub partial: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partial_reason: Option<String>,
    pub ended: bool,
    /// The log's numbering (see [`HistoryBook::page_after`]): opaque, it
    /// changes when the log is made again (a daemon restart, a log that
    /// expired) or reset (the hook's backfill). Seqs of two epochs are not
    /// comparable: a reader whose items carry another epoch reads afresh.
    pub epoch: String,
    /// [`HistoryBook::page_after`] only: more items past this page.
    #[serde(skip_serializing_if = "is_false")]
    pub more: bool,
}

/// What a transcript post was answered: accepted up to `next`, or out of
/// step (the hook starts again from `next`; 0 means from the tail, reset).
#[derive(Debug, PartialEq, Eq)]
pub enum Posted {
    Accepted { next: u64, nudge: Option<u64> },
    OutOfStep { next: u64 },
}

/// Every session's log, keyed by (sandbox, session_id).
#[derive(Debug, Default)]
pub struct HistoryBook {
    logs: BTreeMap<(String, String), Log>,
    total: usize,
    bounds: Bounds,
    /// What sbxw typed, so its lines are not shown as the person's.
    typed: crate::typed::TypedLog,
    /// Epochs handed out: a random start per daemon run, then one more each.
    epochs: Epochs,
}

/// Where epochs come from: random per book (so a restarted daemon never
/// hands out an epoch a reader saw before it), then counting.
#[derive(Debug)]
struct Epochs(u64);

impl Default for Epochs {
    fn default() -> Self {
        let mut b = [0u8; 8];
        getrandom::getrandom(&mut b).expect("the OS has no randomness to give");
        Epochs(u64::from_le_bytes(b))
    }
}

impl Epochs {
    fn mint(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(1);
        self.0
    }
}

impl HistoryBook {
    /// Sbxw is about to type `text` into one of `sandbox`'s sessions: a user
    /// turn with exactly that text is sbxw's, not the person's.
    pub fn note_typed(&mut self, sandbox: &str, text: &str, now: u64) {
        self.typed.note(sandbox, text, now);
    }

    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_bounds(bounds: Bounds) -> Self {
        HistoryBook { bounds, ..Default::default() }
    }

    /// Bytes held, for the ceiling (and a count in the logs).
    pub fn total_bytes(&self) -> usize {
        self.total
    }

    pub fn sessions(&self) -> usize {
        self.logs.len()
    }

    fn log(&mut self, sandbox: &str, session_id: &str, now: u64) -> &mut Log {
        let epochs = &mut self.epochs;
        let log = self.logs.entry((sandbox.to_string(), session_id.to_string())).or_insert_with(|| Log { epoch: epochs.mint(), ..Log::default() });
        log.last_ms = now;
        log
    }

    /// Fold one hook event in. Returns the newest seq to nudge with, when the
    /// log grew and a UI subscribed to it.
    pub fn ingest_event(&mut self, sandbox: &str, ev: &HookEvent<'_>, rules: &Rules, now: u64) -> Option<u64> {
        let session_id = ev.session_id();
        let body = ev.body;
        let log = self.log(sandbox, session_id, now);
        log.hook_version = log.hook_version.max(ev.hook_version());
        match body.get("history").and_then(Value::as_str) {
            Some("transcript") => {
                log.source = Source::Transcript;
                log.no_transcript = None;
            }
            Some(other) if other.starts_with("none") && log.source == Source::Events => {
                log.no_transcript = Some(crate::agents::clip(other, 80));
            }
            _ => {}
        }
        // A v3 hook that reads its transcript says so; until its first post
        // the log is catching up, not filled from events (that would double).
        let events_only = log.source == Source::Events && (log.no_transcript.is_some() || ev.hook_version() < HOOK_VERSION_TRANSCRIPT);
        let str_of = |k: &str| body.get(k).and_then(Value::as_str).unwrap_or("");
        let mut new: Vec<Item> = Vec::new();
        match ev.event() {
            "SessionStart" => {
                let source = str_of("source");
                if source == "compact" && events_only {
                    new.push(Item::new(Kind::CompactMarker, now));
                } else {
                    let mut it = Item::new(Kind::Notice, now);
                    it.text = match source {
                        "" | "startup" => "Session started".into(),
                        s => format!("Session started ({})", crate::agents::clip(s, 20)),
                    };
                    new.push(it);
                }
            }
            "UserPromptSubmit" if events_only => {
                new.extend(user_items(str_of("prompt"), false, now, &|t| self.typed.typed(sandbox, t, now)));
            }
            "PreToolUse" if ev.tool() == "AskUserQuestion" => {
                let steps = questions_from_ask(body);
                if let Some(first) = steps.first() {
                    let mut it = Item::new(Kind::Question, now);
                    it.text = first.text.clone();
                    it.steps = Some(steps);
                    new.push(it);
                }
            }
            "PreToolUse" => {
                let call = ToolCall {
                    name: crate::agents::clip(ev.tool(), 80),
                    summary: summarize_tool(ev.tool(), body.get("tool_input").unwrap_or(&Value::Null)),
                    id: body.get("tool_use_id").and_then(Value::as_str).map(|s| crate::agents::clip(s, 80)),
                };
                if events_only {
                    let mut it = Item::new(Kind::ToolCall, now);
                    it.tool = Some(call.clone());
                    new.push(it);
                }
                self.log(sandbox, session_id, now).pending_tool = Some(call);
            }
            "PostToolUse" if events_only && ev.tool() != "AskUserQuestion" => {
                let mut it = Item::new(Kind::ToolResult, now);
                it.result = Some(result_of_response(body.get("tool_response").unwrap_or(&Value::Null), body.get("tool_use_id")));
                new.push(it);
            }
            "Notification" => match str_of("notification_type") {
                "idle_prompt" | "auth_success" => {}
                kind => {
                    let msg = str_of("message");
                    let permission = kind == "permission_prompt" || (kind.is_empty() && msg.contains("permission"));
                    let mut it = Item::new(if permission { Kind::Permission } else { Kind::Notice }, now);
                    it.text = crate::agents::clip(msg, 300);
                    if permission {
                        it.tool = self.log(sandbox, session_id, now).pending_tool.clone();
                    }
                    if !it.text.is_empty() || it.tool.is_some() {
                        new.push(it);
                    }
                }
            },
            "Stop" => {
                if let Some(text) = body.get("last_assistant_message").and_then(Value::as_str).filter(|t| !t.trim().is_empty()) {
                    let mut it = Item::new(Kind::AssistantText, now);
                    it.text = text.to_string();
                    it.provisional = !events_only;
                    new.push(it);
                }
            }
            "SessionEnd" => {
                let mut it = Item::new(Kind::Notice, now);
                it.text = match str_of("reason") {
                    "" => "Session ended".into(),
                    r => format!("Session ended ({})", crate::agents::clip(r, 40)),
                };
                new.push(it);
                self.log(sandbox, session_id, now).ended_ms = Some(now);
            }
            _ => {}
        }
        self.push(sandbox, session_id, new, rules, now)
    }

    /// One transcript post from a session's hook (identity already proven:
    /// `sandbox` comes from its token). `post` is `{session_id, from, to,
    /// reset, items}`; see `docs/session-history.md`.
    pub fn ingest_transcript(&mut self, sandbox: &str, post: &Value, rules: &Rules, now: u64) -> Posted {
        let session_id = post.get("session_id").and_then(Value::as_str).unwrap_or("");
        let from = post.get("from").and_then(Value::as_u64).unwrap_or(0);
        let to = post.get("to").and_then(Value::as_u64).unwrap_or(from).max(from);
        let reset = post.get("reset").and_then(Value::as_bool).unwrap_or(false);
        let fresh = self.epochs.mint();
        let log = self.log(sandbox, session_id, now);
        if reset {
            let dropped: usize = log.items.iter().map(Item::weight).sum();
            log.items.clear();
            log.bytes = 0;
            log.truncated_before = from > 0;
            log.epoch = fresh;
            self.total = self.total.saturating_sub(dropped);
        } else {
            match log.transcript_next {
                Some(n) if n == from => {}
                None if from == 0 => {}
                Some(n) => return Posted::OutOfStep { next: n },
                None => return Posted::OutOfStep { next: 0 },
            }
        }
        let log = self.log(sandbox, session_id, now);
        log.source = Source::Transcript;
        log.no_transcript = None;
        log.transcript_next = Some(to);
        // `Stop`'s early reply stays until this transcript brings the same
        // text (see `push`): a post without it (the reply not written yet
        // when the hook read the file) must not take it away, or nothing
        // shows the reply before the next hook event.
        let mut new = Vec::new();
        for raw in post.get("items").and_then(Value::as_array).into_iter().flatten().take(limits::POST_ITEMS_MAX) {
            new.extend(transcript_items(raw, now, &|t| self.typed.typed(sandbox, t, now)));
        }
        let mut nudge = self.push(sandbox, session_id, new, rules, now);
        // `Stop`'s early reply is the end of its turn: what the transcript
        // brings before the reply itself (the prompt the person sent, the
        // turn's steps) goes before it. It is numbered again, at the end.
        if self.provisional_last(sandbox, session_id) {
            nudge = nudge.and_then(|_| self.subscribed_seq(sandbox, session_id, now));
        }
        if nudge.is_none() && reset {
            nudge = self.subscribed_seq(sandbox, session_id, now);
        }
        Posted::Accepted { next: to, nudge }
    }

    /// Move a provisional reply that is no longer the last item to the end,
    /// with a new seq (seqs are never reused). `true` when it moved.
    fn provisional_last(&mut self, sandbox: &str, session_id: &str) -> bool {
        let Some(log) = self.logs.get_mut(&(sandbox.to_string(), session_id.to_string())) else { return false };
        let Some(at) = log.items.iter().position(|i| i.provisional) else { return false };
        if at + 1 == log.items.len() {
            return false;
        }
        let Some(mut it) = log.items.remove(at) else { return false };
        log.next_seq += 1;
        it.seq = log.next_seq;
        log.items.push_back(it);
        true
    }

    fn subscribed_seq(&self, sandbox: &str, session_id: &str, now: u64) -> Option<u64> {
        let log = self.logs.get(&(sandbox.to_string(), session_id.to_string()))?;
        (log.subscribed_until > now).then_some(log.next_seq)
    }

    /// Mask, cap, number and append; then the ring and the ceiling.
    fn push(&mut self, sandbox: &str, session_id: &str, new: Vec<Item>, rules: &Rules, now: u64) -> Option<u64> {
        if new.is_empty() {
            return None;
        }
        let text_max = self.bounds.text_max;
        let ring_max = self.bounds.ring_max;
        let key = (sandbox.to_string(), session_id.to_string());
        let log = self.logs.entry(key.clone()).or_default();
        let mut added = 0;
        let mut freed = 0;
        let mut grew = false;
        for mut it in new {
            sanitize(&mut it, rules, text_max);
            if it.kind == Kind::AssistantText {
                if it.provisional {
                    // The transcript already gave this turn's reply: no copy.
                    let turn = log.items.iter().rev().take_while(|i| i.kind != Kind::UserPrompt);
                    if turn.filter(|i| i.kind == Kind::AssistantText && !i.provisional).any(|i| same_reply(&i.text, &it.text)) {
                        continue;
                    }
                }
                // A newer `Stop` replaces an older early reply; the
                // transcript's own copy settles the one it repeats.
                let settles = |i: &Item| i.provisional && (it.provisional || same_reply(&i.text, &it.text));
                log.items.retain(|i| {
                    let gone = settles(i);
                    if gone {
                        freed += i.weight();
                    }
                    !gone
                });
            }
            grew = true;
            log.next_seq += 1;
            it.seq = log.next_seq;
            let w = it.weight();
            log.bytes += w;
            added += w;
            log.items.push_back(it);
        }
        if !grew {
            log.bytes -= freed;
            self.total -= freed;
            return None;
        }
        while log.items.len() > ring_max {
            if let Some(old) = log.items.pop_front() {
                freed += old.weight();
                log.truncated_before = true;
            }
        }
        log.bytes -= freed;
        let subscribed = log.subscribed_until > now;
        let latest = log.next_seq;
        self.total = self.total + added - freed;
        self.enforce_ceiling(&key);
        subscribed.then_some(latest)
    }

    /// Past the ceiling, whole logs go, ended ones first (the longest ended),
    /// then the longest idle; the one just written to last, and then only
    /// its oldest items.
    fn enforce_ceiling(&mut self, keep: &(String, String)) {
        while self.total > self.bounds.total_max {
            let victim = self
                .logs
                .iter()
                .filter(|(k, _)| *k != keep)
                .min_by_key(|(_, l)| (l.ended_ms.is_none(), l.ended_ms.unwrap_or(0), l.last_ms))
                .map(|(k, _)| k.clone());
            match victim {
                Some(k) => {
                    if let Some(l) = self.logs.remove(&k) {
                        self.total -= l.bytes;
                    }
                }
                None => {
                    let Some(log) = self.logs.get_mut(keep) else { return };
                    let Some(old) = log.items.pop_front() else { return };
                    let w = old.weight();
                    log.bytes -= w;
                    log.truncated_before = true;
                    self.total -= w;
                }
            }
        }
    }

    /// A session ended without its own `SessionEnd` (retired, sandbox
    /// stopped, thread closed): its log expires from now.
    pub fn end(&mut self, sandbox: &str, session_id: &str, now: u64) {
        if let Some(l) = self.logs.get_mut(&(sandbox.to_string(), session_id.to_string())) {
            l.ended_ms.get_or_insert(now);
        }
    }

    /// Logs of sessions ended an hour ago go.
    pub fn expire(&mut self, now: u64) {
        let ttl = self.bounds.ended_ttl_ms;
        let mut freed = 0;
        self.logs.retain(|_, l| {
            let keep = l.ended_ms.is_none_or(|e| now.saturating_sub(e) < ttl);
            if !keep {
                freed += l.bytes;
            }
            keep
        });
        self.total -= freed;
    }

    /// A removed sandbox's logs go at once.
    pub fn drop_sandbox(&mut self, sandbox: &str) {
        self.typed.drop_sandbox(sandbox);
        let mut freed = 0;
        self.logs.retain(|(sb, _), l| {
            let keep = sb != sandbox;
            if !keep {
                freed += l.bytes;
            }
            keep
        });
        self.total -= freed;
    }

    /// A page, newest first: up to `limit` items older than `before` (all
    /// when absent). Reading subscribes the reader to the session's nudges
    /// for [`limits::SUBSCRIPTION_MS`]. `None`: no such session.
    pub fn page(&mut self, sandbox: &str, session_id: &str, before: Option<u64>, limit: Option<usize>, now: u64) -> Option<Page> {
        let log = self.logs.get_mut(&(sandbox.to_string(), session_id.to_string()))?;
        log.subscribed_until = now + limits::SUBSCRIPTION_MS;
        let limit = limit.unwrap_or(limits::PAGE_DEFAULT).clamp(1, limits::PAGE_MAX);
        let older: Vec<&Item> = log.items.iter().filter(|i| before.is_none_or(|b| i.seq < b)).collect();
        let start = older.len().saturating_sub(limit);
        let items: Vec<Item> = older[start..].iter().map(|i| (*i).clone()).collect();
        let next_before = (start > 0).then(|| items.first().map(|i| i.seq)).flatten();
        let (partial, partial_reason) = partial_of(log);
        Some(Page {
            items,
            next_before,
            latest_seq: log.next_seq,
            truncated_before: log.truncated_before,
            partial,
            partial_reason,
            ended: log.ended_ms.is_some(),
            epoch: format!("{:016x}", log.epoch),
            more: false,
        })
    }

    /// The items newer than `after`, oldest first: the `limit` oldest of
    /// them, `more` when there are others after those. Subscribes like
    /// [`Self::page`]. `after` is a seq of the log's current epoch; one of
    /// another epoch means nothing here (the reader compares epochs).
    pub fn page_after(&mut self, sandbox: &str, session_id: &str, after: u64, limit: Option<usize>, now: u64) -> Option<Page> {
        let log = self.logs.get_mut(&(sandbox.to_string(), session_id.to_string()))?;
        log.subscribed_until = now + limits::SUBSCRIPTION_MS;
        let limit = limit.unwrap_or(limits::PAGE_DEFAULT).clamp(1, limits::PAGE_MAX);
        let mut newer = log.items.iter().filter(|i| i.seq > after);
        let items: Vec<Item> = newer.by_ref().take(limit).cloned().collect();
        let more = newer.next().is_some();
        let (partial, partial_reason) = partial_of(log);
        Some(Page {
            items,
            next_before: None,
            latest_seq: log.next_seq,
            truncated_before: log.truncated_before,
            partial,
            partial_reason,
            ended: log.ended_ms.is_some(),
            epoch: format!("{:016x}", log.epoch),
            more,
        })
    }

    /// The UI no longer shows this session. `false`: no such session.
    pub fn unsubscribe(&mut self, sandbox: &str, session_id: &str) -> bool {
        match self.logs.get_mut(&(sandbox.to_string(), session_id.to_string())) {
            Some(l) => {
                l.subscribed_until = 0;
                true
            }
            None => false,
        }
    }
}

/// Whether a log is the whole conversation, and why not.
fn partial_of(log: &Log) -> (bool, Option<String>) {
    match (log.source, log.transcript_next) {
        (Source::Transcript, Some(_)) => (false, None),
        (Source::Transcript, None) => (true, Some("catching_up: the hook has not posted this session's transcript yet".to_string())),
        (Source::Events, _) => match &log.no_transcript {
            Some(why) => (true, Some(format!("no_transcript: the hook could not read this session's transcript ({why}); events only"))),
            None if log.hook_version < HOOK_VERSION_TRANSCRIPT => (
                true,
                Some(format!(
                    "old_hook: this sandbox's status hook (v{}) does not read the transcript: events only, no assistant text \
                     between tool calls, nothing from before the daemon saw the session; `sbxw up <sandbox>` installs v{}",
                    log.hook_version, HOOK_VERSION_TRANSCRIPT
                )),
            ),
            None => (true, Some("catching_up: the hook has not posted this session's transcript yet".to_string())),
        },
    }
}

/// Cap at `max` bytes on a character boundary. `true` when cut.
fn cap_bytes(s: &mut String, max: usize) -> bool {
    if s.len() <= max {
        return false;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
    true
}

fn cap_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

/// Every text an item carries, capped then masked (masked again after the
/// cap would be the same: the cap only shortens).
/// Whether two replies are the same text, whitespace aside (`Stop`'s copy
/// and the transcript's can differ in a trailing line feed).
fn same_reply(a: &str, b: &str) -> bool {
    a.split_whitespace().eq(b.split_whitespace())
}

fn sanitize(it: &mut Item, rules: &Rules, text_max: usize) {
    it.clipped |= cap_bytes(&mut it.text, text_max);
    it.text = mask(&it.text, rules).text;
    if let Some(t) = &mut it.tool {
        t.summary = mask(&crate::agents::clip(&t.summary, limits::SUMMARY_MAX), rules).text;
        t.name = crate::agents::clip(&t.name, 80);
    }
    if let Some(r) = &mut it.result {
        r.head = mask(&cap_chars(&r.head, limits::RESULT_HEAD_MAX), rules).text;
    }
    if let Some(f) = &mut it.from {
        *f = mask(&crate::agents::clip(f, limits::FROM_MAX), rules).text;
    }
    it.files.truncate(limits::FILES_MAX);
    for f in &mut it.files {
        *f = mask(&cap_chars(f, limits::FILE_NAME_MAX), rules).text;
    }
    if let Some(steps) = &mut it.steps {
        for q in steps {
            q.text = mask(&q.text, rules).text;
            for o in q.options.iter_mut().chain(q.context.iter_mut()) {
                *o = mask(o, rules).text;
            }
        }
    }
}

/// A tool call in one line, by tool. Unknown tools: the name only.
pub fn summarize_tool(name: &str, input: &Value) -> String {
    let field = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("");
    let num = |k: &str| input.get(k).and_then(Value::as_u64);
    let lines = |k: &str, n: &str| -> Option<u64> {
        input.get(k).and_then(Value::as_str).map(|s| s.lines().count() as u64).or_else(|| num(n))
    };
    let one = |s: &str, max: usize| crate::agents::clip(s, max);
    let s = match name {
        "Bash" => match field("command") {
            "" => "Bash".to_string(),
            c => {
                let more = if c.trim().lines().count() > 1 { " …" } else { "" };
                format!("$ {}{more}", one(c, 140))
            }
        },
        "Read" => {
            let p = field("file_path");
            match (num("offset"), num("limit")) {
                (Some(o), Some(l)) => format!("Read {} (lines {}-{})", one(p, 120), o, o + l),
                (None, Some(l)) => format!("Read {} (first {} lines)", one(p, 120), l),
                _ => format!("Read {}", one(p, 120)),
            }
        }
        "Edit" => {
            let p = field("file_path");
            match (lines("old_string", "lines_old"), lines("new_string", "lines_new")) {
                (Some(o), Some(n)) => format!("Edit {} (-{o} +{n} lines)", one(p, 120)),
                _ => format!("Edit {}", one(p, 120)),
            }
        }
        "MultiEdit" => {
            let n = input.get("edits").and_then(Value::as_array).map(|a| a.len() as u64).or_else(|| num("edits_count"));
            match n {
                Some(n) => format!("Edit {} ({n} edits)", one(field("file_path"), 120)),
                None => format!("Edit {}", one(field("file_path"), 120)),
            }
        }
        "Write" => match lines("content", "lines") {
            Some(n) => format!("Write {} ({n} lines)", one(field("file_path"), 120)),
            None => format!("Write {}", one(field("file_path"), 120)),
        },
        "NotebookEdit" => format!("NotebookEdit {}", one(field("notebook_path"), 120)),
        "Grep" => match field("path") {
            "" => format!("Grep {}", one(field("pattern"), 100)),
            p => format!("Grep {} in {}", one(field("pattern"), 80), one(p, 60)),
        },
        "Glob" => format!("Glob {}", one(field("pattern"), 120)),
        "Task" | "Agent" => match (field("description"), field("subagent_type")) {
            ("", _) => name.to_string(),
            (d, "") => format!("{name}: {}", one(d, 120)),
            (d, t) => format!("{name} ({}): {}", one(t, 30), one(d, 100)),
        },
        "AskUserQuestion" => {
            let q = input
                .get("questions")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(|q| q.get("question"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if q.is_empty() {
                name.to_string()
            } else {
                format!("Question: {}", one(q, 120))
            }
        }
        n if n.starts_with("mcp__") => {
            let rest = &n[5..];
            match rest.split_once("__") {
                Some((server, tool)) => format!("MCP {}: {}", one(server, 60), one(tool, 60)),
                None => format!("MCP {}", one(rest, 100)),
            }
        }
        "" => "Tool".to_string(),
        other => one(other, 80),
    };
    one(&s, limits::SUMMARY_MAX)
}

/// A `PostToolUse` event's `tool_response` as a result: its status, size and head.
fn result_of_response(resp: &Value, id: Option<&Value>) -> ToolResult {
    let error = resp.get("is_error").and_then(Value::as_bool).unwrap_or(false)
        || resp.get("success").and_then(Value::as_bool) == Some(false)
        || resp.get("error").is_some_and(|e| !e.is_null() && e != "");
    let text = match resp {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        _ => ["stdout", "content", "output", "result", "error"]
            .iter()
            .find_map(|k| resp.get(*k).and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| resp.to_string()),
    };
    ToolResult {
        status: if error { ResultStatus::Error } else { ResultStatus::Ok },
        size: text.chars().count() as u64,
        head: cap_chars(&text, limits::RESULT_HEAD_MAX),
        id: id.and_then(Value::as_str).map(|s| crate::agents::clip(s, 80)),
    }
}

/// One item the hook posted, as the daemon's items. Unknown types: none.
fn transcript_items(raw: &Value, now: u64, typed: &dyn Fn(&str) -> bool) -> Vec<Item> {
    let s = |k: &str| raw.get(k).and_then(Value::as_str).unwrap_or("");
    let at = raw.get("at").and_then(Value::as_u64).filter(|&a| a > 0).unwrap_or(now);
    let it = |kind: Kind| Item::new(kind, at);
    match s("t") {
        "user" => {
            if raw.get("compact_summary").and_then(Value::as_bool) == Some(true) {
                let mut c = it(Kind::CompactMarker);
                c.text = s("text").to_string();
                return vec![c];
            }
            let meta = raw.get("meta").and_then(Value::as_bool) == Some(true);
            user_items(s("text"), meta, at, typed)
        }
        "assistant" => {
            let text = s("text");
            if text.trim().is_empty() {
                return Vec::new();
            }
            let mut a = it(Kind::AssistantText);
            a.text = text.to_string();
            vec![a]
        }
        "tool_use" => {
            let name = s("name");
            let mut c = it(Kind::ToolCall);
            c.tool = Some(ToolCall {
                name: name.to_string(),
                summary: summarize_tool(name, raw.get("input").unwrap_or(&Value::Null)),
                id: Some(s("id")).filter(|i| !i.is_empty()).map(str::to_string),
            });
            vec![c]
        }
        "tool_result" => {
            let mut r = it(Kind::ToolResult);
            let head = s("head").to_string();
            r.result = Some(ToolResult {
                status: if raw.get("is_error").and_then(Value::as_bool) == Some(true) { ResultStatus::Error } else { ResultStatus::Ok },
                size: raw.get("size").and_then(Value::as_u64).unwrap_or(head.chars().count() as u64),
                head,
                id: Some(s("id")).filter(|i| !i.is_empty()).map(str::to_string),
            });
            vec![r]
        }
        "image" => {
            let mut n = it(Kind::Notice);
            n.text = "An image was here (not kept)".into();
            vec![n]
        }
        "compact" => vec![it(Kind::CompactMarker)],
        "api_error" => {
            let mut n = it(Kind::Notice);
            n.text = crate::agents::clip(s("text"), 300);
            vec![n]
        }
        _ => Vec::new(),
    }
}

/// A user turn's text as items: the person's words, or what the harness
/// wrote in their place. `meta` is the transcript's `isMeta`.
fn user_items(text: &str, meta: bool, at: u64, typed: &dyn Fn(&str) -> bool) -> Vec<Item> {
    let mut out = Vec::new();
    // System reminders folded into a prompt are the harness's, not the person's.
    let (rest, reminders) = take_reminders(text);
    for r in reminders {
        let mut i = Item::new(Kind::Injected, at);
        i.text = r;
        i.from = Some("system".into());
        out.push(i);
    }
    if rest.trim().is_empty() {
        return out;
    }
    let mut c = classify_user_text(&rest);
    let cross_session = c.kind == Kind::Injected && leading_tag(rest.trim()).is_some_and(|t| t.name == "cross-session-message");
    let mut by_sbxw = false;
    if c.kind == Kind::UserPrompt && c.files.is_empty() && typed(&c.text) {
        c = UserText { kind: Kind::Injected, text: c.text, from: Some(crate::typed::SENDER.to_string()), files: Vec::new() };
        by_sbxw = true;
    }
    let mut i = Item::new(c.kind, at);
    i.text = c.text;
    i.from = c.from;
    i.files = c.files;
    i.message = cross_session || by_sbxw;
    if meta && i.kind == Kind::UserPrompt {
        i.kind = Kind::Injected;
        i.from = Some("meta".into());
    }
    out.insert(0, i);
    out
}

/// `text` without its `<system-reminder>` spans, and those spans' contents.
fn take_reminders(text: &str) -> (String, Vec<String>) {
    const OPEN: &str = "<system-reminder>";
    const CLOSE: &str = "</system-reminder>";
    let mut rest = String::new();
    let mut found = Vec::new();
    let mut s = text;
    while let Some(a) = s.find(OPEN) {
        let Some(b) = s[a..].find(CLOSE) else { break };
        rest.push_str(&s[..a]);
        found.push(s[a + OPEN.len()..a + b].trim().to_string());
        s = &s[a + b + CLOSE.len()..];
    }
    rest.push_str(s);
    (rest, found)
}

/// What a user turn's text is.
#[derive(Debug, PartialEq)]
pub struct UserText {
    pub kind: Kind,
    pub text: String,
    /// Who wrote it, for an injected turn: a cross-session message's sender,
    /// `task`, `system`, `command`, `shell`, `hook`, or the wrapping tag.
    pub from: Option<String>,
    /// For the person's prompt: the files sent with it (base names).
    pub files: Vec<String>,
}

/// Is this user turn the person's words? The harness writes its own turns
/// in tags (`<cross-session-message from=…>`, `<task-notification>`,
/// `<system-reminder>`, a command's output…): those are `injected`, so a UI
/// never shows them as the person's. A slash command or a `!` command the
/// person typed is theirs, rewritten as they typed it.
pub fn classify_user_text(text: &str) -> UserText {
    let t = text.trim();
    if let Some(inner) = pasted_inner(t) {
        // A paste (a phone message is one): the person's words, with the
        // files sbxw named in its own block split off.
        let (words, files) = split_attachments(inner);
        return UserText { kind: Kind::UserPrompt, text: words.to_string(), from: None, files };
    }
    let injected = |from: &str, body: String| UserText { kind: Kind::Injected, text: body, from: Some(from.to_string()), files: Vec::new() };
    if t.starts_with("[Request interrupted by user") {
        return UserText { kind: Kind::Notice, text: "Interrupted by the person".into(), from: None, files: Vec::new() };
    }
    if t.starts_with("Caveat: The messages below were generated by the user while running local commands") {
        return injected("command", t.to_string());
    }
    if let Some(tag) = leading_tag(t) {
        let inner = |name: &str| tag_inner(t, name).unwrap_or_else(|| t.to_string());
        match tag.name.as_str() {
            "cross-session-message" => {
                let from = tag.attr("from-name").or_else(|| tag.attr("from")).unwrap_or_else(|| "another session".into());
                return injected(&from, inner("cross-session-message"));
            }
            "task-notification" => return injected("task", inner("task-notification")),
            "system-reminder" => return injected("system", inner("system-reminder")),
            "local-command-stdout" | "local-command-stderr" | "local-command-caveat" => return injected("command", inner(&tag.name)),
            "bash-stdout" | "bash-stderr" => return injected("shell", t.to_string()),
            "user-prompt-submit-hook" => return injected("hook", inner("user-prompt-submit-hook")),
            "command-name" | "command-message" | "command-args" => {
                let name = tag_inner(t, "command-name").unwrap_or_default();
                let args = tag_inner(t, "command-args").unwrap_or_default();
                let typed = format!("{} {}", name.trim(), args.trim()).trim().to_string();
                if !typed.is_empty() {
                    return UserText { kind: Kind::UserPrompt, text: typed, from: None, files: Vec::new() };
                }
                return injected("command", t.to_string());
            }
            "bash-input" => {
                return UserText { kind: Kind::UserPrompt, text: format!("! {}", inner("bash-input").trim()), from: None, files: Vec::new() };
            }
            // Any other harness-shaped wrapper around the whole text
            // (`<some-thing>…</some-thing>`): not the person's.
            name if (name.contains('-') || name.contains('_')) && t.ends_with(&format!("</{name}>")) => {
                return injected(name, inner(name));
            }
            _ => {}
        }
    }
    UserText { kind: Kind::UserPrompt, text: t.to_string(), from: None, files: Vec::new() }
}

/// What a text that is exactly one paste holds: Claude Code records a
/// bracketed paste as `<pasted_content id="X">` … `</pasted_content id="X">`.
/// Both ids must match, the id is plain (`[A-Za-z0-9_-]`), and no other
/// wrapper may open inside: a text that merely contains the tag stays whole.
fn pasted_inner(t: &str) -> Option<&str> {
    const OPEN: &str = "<pasted_content id=\"";
    let rest = t.strip_prefix(OPEN)?;
    let q = rest.find('"')?;
    let id = &rest[..q];
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return None;
    }
    let body = rest[q..].strip_prefix("\">")?;
    let close = format!("</pasted_content id=\"{id}\">");
    let inner = body.strip_suffix(close.as_str())?;
    if inner.contains("<pasted_content") || inner.contains("</pasted_content") {
        return None;
    }
    Some(inner.trim())
}

/// `text` without sbxw's trailing attachment block, and the files it names
/// (base names). The block is exactly what `uploads::with_attachments`
/// writes: the text, a blank line (or nothing, for files alone), the header,
/// then one `- <UPLOAD_ROOT>/<ms>/<name>` line per file, nothing after.
/// Anything else (a header typed by hand, a path elsewhere) stays text.
fn split_attachments(text: &str) -> (&str, Vec<String>) {
    let header = crate::uploads::ATTACH_HEADER;
    let (words, block) = if let Some(b) = text.strip_prefix(header) {
        ("", b)
    } else {
        match text.rfind(&format!("\n\n{header}")) {
            Some(i) => (&text[..i], &text[i + 2 + header.len()..]),
            None => return (text, Vec::new()),
        }
    };
    let mut files = Vec::new();
    let mut lines = block.split('\n');
    if lines.next() != Some("") {
        return (text, Vec::new());
    }
    for line in lines {
        match upload_base_name(line.trim_end_matches('\r')) {
            Some(n) => files.push(n.to_string()),
            None => return (text, Vec::new()),
        }
    }
    if files.is_empty() {
        return (text, Vec::new());
    }
    (words.trim_end(), files)
}

/// `name` of a block line `- <UPLOAD_ROOT>/<digits>/<name>`.
fn upload_base_name(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("- ")?.strip_prefix(crate::uploads::UPLOAD_ROOT)?.strip_prefix('/')?;
    let (ms, name) = rest.split_once('/')?;
    if ms.is_empty() || !ms.bytes().all(|b| b.is_ascii_digit()) || name.is_empty() || name.contains('/') {
        return None;
    }
    Some(name)
}

struct Tag {
    name: String,
    head: String,
}

impl Tag {
    fn attr(&self, k: &str) -> Option<String> {
        let pat = format!("{k}=\"");
        let i = self.head.find(&pat)? + pat.len();
        let j = self.head[i..].find('"')?;
        Some(self.head[i..i + j].to_string()).filter(|s| !s.is_empty())
    }
}

/// `<name attrs…>` at the start of `t`, when `name` is a tag's.
fn leading_tag(t: &str) -> Option<Tag> {
    let rest = t.strip_prefix('<')?;
    let end = rest.find('>')?;
    let head = &rest[..end];
    let name: String = head.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    if name.is_empty() || !name.chars().next().is_some_and(|c| c.is_ascii_lowercase()) {
        return None;
    }
    Some(Tag { name, head: head.to_string() })
}

/// What `<name …>` … `</name>` holds in `t`.
fn tag_inner(t: &str, name: &str) -> Option<String> {
    let open = t.find(&format!("<{name}"))?;
    let start = open + t[open..].find('>')? + 1;
    let close = t[start..].find(&format!("</{name}>")).map(|c| start + c).unwrap_or(t.len());
    Some(t[start..close].trim().to_string())
}

#[cfg(test)]
mod tests;
