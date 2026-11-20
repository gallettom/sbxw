//! What a phone may see of a leader: its conversation without the work.
//!
//! The history's items are filtered by an allowlist of fields: a new field on
//! [`Item`] reaches the phone only once it is added here. Tool calls and tool
//! results never do: each run of them becomes one `steps` item that says how
//! many there were, with no tool name, no input, no result, no file content.

use crate::agents::{Question, SessionState};
use crate::history::{Item, Kind, Page};
use serde::Serialize;

/// One item as the phone gets it.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct PhoneItem {
    pub seq: u64,
    pub at: u64,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<PhoneQuestion>>,
    /// For `kind: "steps"`: how many tool calls the run held.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub clipped: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub provisional: bool,
}

/// A question's step: its text and options, not the decision table.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct PhoneQuestion {
    pub text: String,
    pub options: Vec<String>,
}

pub fn phone_question(q: &Question) -> PhoneQuestion {
    PhoneQuestion { text: q.text.clone(), options: q.options.clone() }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct PhonePage {
    pub items: Vec<PhoneItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_before: Option<u64>,
    pub latest_seq: u64,
    pub truncated_before: bool,
    pub partial: bool,
    pub ended: bool,
}

fn plain(it: &Item, kind: &'static str) -> PhoneItem {
    PhoneItem { seq: it.seq, at: it.at, kind, text: String::new(), from: None, steps: None, count: None, clipped: false, provisional: false }
}

/// The phone's copy of `page`. `after`: only items past that seq.
pub fn phone_page(page: &Page, after: Option<u64>) -> PhonePage {
    let mut items: Vec<PhoneItem> = Vec::new();
    let mut run: Option<PhoneItem> = None;
    for it in page.items.iter().filter(|i| after.is_none_or(|a| i.seq > a)) {
        match it.kind {
            Kind::ToolCall | Kind::ToolResult => {
                let r = run.get_or_insert_with(|| PhoneItem { count: Some(0), ..plain(it, "steps") });
                r.seq = it.seq;
                r.at = it.at;
                if it.kind == Kind::ToolCall {
                    r.count = Some(r.count.unwrap_or(0) + 1);
                }
                continue;
            }
            _ => {}
        }
        if let Some(mut r) = run.take() {
            r.count = Some(r.count.unwrap_or(0).max(1));
            items.push(r);
        }
        let shown = match it.kind {
            Kind::UserPrompt => PhoneItem { text: it.text.clone(), clipped: it.clipped, ..plain(it, "user_prompt") },
            Kind::AssistantText => PhoneItem { text: it.text.clone(), clipped: it.clipped, provisional: it.provisional, ..plain(it, "assistant_text") },
            Kind::Injected => PhoneItem { text: it.text.clone(), clipped: it.clipped, from: it.from.clone(), ..plain(it, "injected") },
            Kind::Question => PhoneItem {
                text: it.text.clone(),
                steps: it.steps.as_ref().map(|s| s.iter().map(phone_question).collect()),
                ..plain(it, "question")
            },
            Kind::Notice => PhoneItem { text: it.text.clone(), ..plain(it, "notice") },
            Kind::CompactMarker => plain(it, "compact_marker"),
            // What tool it asks for is the tool's input: left out.
            Kind::Permission => plain(it, "permission"),
            Kind::ToolCall | Kind::ToolResult => unreachable!("folded above"),
        };
        items.push(shown);
    }
    if let Some(mut r) = run.take() {
        r.count = Some(r.count.unwrap_or(0).max(1));
        items.push(r);
    }
    PhonePage {
        items,
        next_before: if after.is_some() { None } else { page.next_before },
        latest_seq: page.latest_seq,
        truncated_before: page.truncated_before,
        partial: page.partial,
        ended: page.ended,
    }
}

/// A leader's state, in the phone's three words.
pub fn phone_state(s: SessionState) -> &'static str {
    match s {
        SessionState::Working => "working",
        SessionState::Attention => "attention",
        SessionState::Idle | SessionState::Exited => "idle",
    }
}

/// A coarse line in place of the session's `activity`, which can quote a
/// tool's input (`$ cargo test`).
pub fn phone_activity(state: SessionState, question: bool) -> &'static str {
    match (state, question) {
        (SessionState::Working, _) => "Working",
        (SessionState::Attention, true) => "Asks a question",
        (SessionState::Attention, false) => "Waits for you at the Mac",
        _ => "Waiting for a message",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{ResultStatus, ToolCall, ToolResult};

    fn item(seq: u64, kind: Kind, text: &str) -> Item {
        Item {
            seq,
            at: seq * 10,
            kind,
            text: text.into(),
            tool: None,
            result: None,
            from: None,
            steps: None,
            clipped: false,
            provisional: false,
        }
    }

    fn page(items: Vec<Item>) -> Page {
        Page { latest_seq: items.last().map_or(0, |i| i.seq), items, next_before: Some(1), truncated_before: false, partial: false, partial_reason: Some("why".into()), ended: false }
    }

    #[test]
    fn tools_are_folded_into_counts() {
        let mut call = item(2, Kind::ToolCall, "");
        call.tool = Some(ToolCall { name: "Bash".into(), summary: "cat /secret/file".into(), id: Some("t1".into()) });
        let mut res = item(3, Kind::ToolResult, "");
        res.result = Some(ToolResult { status: ResultStatus::Ok, size: 10, head: "SECRET CONTENT".into(), id: Some("t1".into()) });
        let mut call2 = item(4, Kind::ToolCall, "");
        call2.tool = Some(ToolCall { name: "Read".into(), summary: "/etc/passwd".into(), id: None });
        let mut perm = item(6, Kind::Permission, "Allow Bash: rm -rf /tmp/x?");
        perm.tool = Some(ToolCall { name: "Bash".into(), summary: "rm -rf /tmp/x".into(), id: None });
        let mut q = item(7, Kind::Question, "Which?");
        q.steps = Some(vec![Question { text: "Which?".into(), options: vec!["A".into(), "B".into()], context: vec!["A — reads ~/.ssh".into()] }]);
        let p = page(vec![item(1, Kind::UserPrompt, "hello"), call, res, call2, item(5, Kind::AssistantText, "done"), perm, q, item(8, Kind::ToolResult, "")]);
        let out = phone_page(&p, None);
        let json = serde_json::to_string(&out).unwrap();
        for leak in ["Bash", "cat /secret", "SECRET CONTENT", "/etc/passwd", "rm -rf", "t1", "Read", "reads ~/.ssh", "partial_reason", "\"tool\"", "\"result\"", "\"head\"", "\"summary\""] {
            assert!(!json.contains(leak), "{leak} in {json}");
        }
        let kinds: Vec<&str> = out.items.iter().map(|i| i.kind).collect();
        assert_eq!(kinds, ["user_prompt", "steps", "assistant_text", "permission", "question", "steps"]);
        assert_eq!(out.items[1].count, Some(2));
        assert_eq!(out.items[1].seq, 4, "a run is at its last seq");
        assert_eq!(out.items[5].count, Some(1), "a lone result still counts as a step");
        assert_eq!(out.items[4].steps.as_ref().unwrap()[0].options, ["A", "B"]);
        assert_eq!(out.next_before, Some(1));
    }

    #[test]
    fn after_keeps_only_newer_items() {
        let p = page(vec![item(1, Kind::UserPrompt, "a"), item(2, Kind::AssistantText, "b"), item(3, Kind::Injected, "c")]);
        let out = phone_page(&p, Some(1));
        assert_eq!(out.items.iter().map(|i| i.seq).collect::<Vec<_>>(), [2, 3]);
        assert_eq!(out.next_before, None);
        assert!(phone_page(&p, Some(3)).items.is_empty());
    }

    #[test]
    fn activity_never_quotes_a_tool() {
        assert_eq!(phone_activity(SessionState::Working, false), "Working");
        assert_eq!(phone_state(SessionState::Exited), "idle");
    }
}
