//! Every text here is invented, and every "secret" a fake made for the test.

use super::*;
use serde_json::json;

const SB: &str = "shop";
const ID: &str = "s1";

fn rules() -> Rules {
    Rules::default()
}

fn ev(book: &mut HistoryBook, body: Value, now: u64) -> Option<u64> {
    book.ingest_event(SB, &HookEvent::new(&body), &rules(), now)
}

fn post(book: &mut HistoryBook, from: u64, to: u64, items: Value, now: u64) -> Posted {
    book.ingest_transcript(SB, &json!({"session_id": ID, "from": from, "to": to, "items": items}), &rules(), now)
}

fn page(book: &mut HistoryBook) -> Page {
    book.page(SB, ID, None, Some(200), 0).unwrap()
}

fn kinds(p: &Page) -> Vec<Kind> {
    p.items.iter().map(|i| i.kind).collect()
}

fn v3(event: &str, extra: Value) -> Value {
    let mut b = json!({"hook_event_name": event, "session_id": ID, "hook_version": 3, "history": "transcript"});
    for (k, v) in extra.as_object().unwrap() {
        b[k] = v.clone();
    }
    b
}

#[test]
fn a_scripted_session_comes_in_order_with_its_kinds() {
    let mut b = HistoryBook::new();
    ev(&mut b, v3("SessionStart", json!({"source": "startup"})), 1);
    ev(&mut b, v3("UserPromptSubmit", json!({"prompt": "Fix the basket total"})), 2);
    let items = json!([
        {"t": "user", "text": "Fix the basket total", "at": 2},
        {"t": "thinking", "text": "never kept"},
        {"t": "assistant", "text": "Reading the basket first.", "at": 3},
        {"t": "tool_use", "id": "tu1", "name": "Read", "input": {"file_path": "/w/src/basket.rs", "offset": 10, "limit": 40}, "at": 4},
        {"t": "tool_result", "id": "tu1", "size": 1200, "head": "fn total() {}", "at": 5},
        {"t": "tool_use", "id": "tu2", "name": "Bash", "input": {"command": "cargo test basket"}, "at": 6},
        {"t": "tool_result", "id": "tu2", "is_error": true, "size": 30, "head": "1 failed", "at": 7},
        {"t": "a-new-kind-of-entry", "text": "skipped"},
    ]);
    assert!(matches!(post(&mut b, 0, 900, items, 8), Posted::Accepted { next: 900, .. }));
    ev(&mut b, v3("PreToolUse", json!({"tool_name": "AskUserQuestion", "tool_input": {"questions": [
        {"question": "Round half up?", "options": [{"label": "Yes"}, {"label": "No"}]}]}})), 9);
    ev(&mut b, v3("PreToolUse", json!({"tool_name": "Bash", "tool_input": {"command": "rm -rf build"}})), 10);
    ev(&mut b, v3("Notification", json!({"notification_type": "permission_prompt", "message": "Claude needs your permission to use Bash"})), 11);
    post(&mut b, 900, 1000, json!([{"t": "compact"}, {"t": "user", "compact_summary": true, "text": "Summary of the work so far"}]), 12);
    ev(&mut b, v3("Stop", json!({"last_assistant_message": "Done: the total rounds half up."})), 13);
    let p = page(&mut b);
    assert_eq!(
        kinds(&p),
        vec![
            Kind::Notice,
            Kind::UserPrompt,
            Kind::AssistantText,
            Kind::ToolCall,
            Kind::ToolResult,
            Kind::ToolCall,
            Kind::ToolResult,
            Kind::Question,
            Kind::Permission,
            Kind::CompactMarker,
            Kind::CompactMarker,
            Kind::AssistantText,
        ]
    );
    let seqs: Vec<u64> = p.items.iter().map(|i| i.seq).collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(p.items[3].tool.as_ref().unwrap().summary, "Read /w/src/basket.rs (lines 10-50)");
    assert_eq!(p.items[6].result.as_ref().unwrap().status, ResultStatus::Error);
    assert_eq!(p.items[7].steps.as_ref().unwrap()[0].options, vec!["Yes", "No"]);
    assert_eq!(p.items[8].tool.as_ref().unwrap().summary, "$ rm -rf build", "the permission says which command");
    assert!(p.items[11].provisional, "Stop's reply waits for the transcript");
    assert!(!p.partial);

    // The transcript catches up: the provisional reply makes way for its own.
    post(&mut b, 1000, 1100, json!([{"t": "assistant", "text": "Done: the total rounds half up."}]), 14);
    let p = page(&mut b);
    assert_eq!(p.items.iter().filter(|i| i.kind == Kind::AssistantText).count(), 2);
    assert!(p.items.iter().all(|i| !i.provisional));

    ev(&mut b, v3("SessionEnd", json!({"reason": "prompt_input_exit"})), 15);
    let p = page(&mut b);
    assert!(p.ended);
    assert_eq!(p.items.last().unwrap().text, "Session ended (prompt_input_exit)");
}

#[test]
fn a_resumed_session_is_a_log_of_its_own() {
    let mut b = HistoryBook::new();
    post(&mut b, 0, 10, json!([{"t": "user", "text": "first"}]), 1);
    let resumed = json!({"session_id": "s2", "from": 0, "to": 50, "items": [{"t": "user", "text": "again"}]});
    b.ingest_transcript(SB, &resumed, &rules(), 2);
    assert_eq!(page(&mut b).items.len(), 1);
    assert_eq!(b.page(SB, "s2", None, None, 2).unwrap().items[0].text, "again");
}

#[test]
fn injected_turns_are_never_the_persons_words() {
    let cases = [
        ("<cross-session-message from=\"uds:/x.sock\" from-name=\"lead\">Please rebase</cross-session-message>", Kind::Injected, Some("lead")),
        ("<task-notification><task-id>b1</task-id>done</task-notification>", Kind::Injected, Some("task")),
        ("<system-reminder>The date changed.</system-reminder>", Kind::Injected, Some("system")),
        ("<local-command-stdout>Set model to Opus</local-command-stdout>", Kind::Injected, Some("command")),
        ("<user-prompt-submit-hook>battery low</user-prompt-submit-hook>", Kind::Injected, Some("hook")),
        ("<some-new-wrapper>harness text</some-new-wrapper>", Kind::Injected, Some("some-new-wrapper")),
        ("Caveat: The messages below were generated by the user while running local commands. DO NOT respond", Kind::Injected, Some("command")),
        ("<command-name>/model</command-name>\n<command-message>model</command-message>\n<command-args>opus</command-args>", Kind::UserPrompt, None),
        ("<bash-input>ls -la</bash-input>", Kind::UserPrompt, None),
        ("[Request interrupted by user for tool use]", Kind::Notice, None),
        ("Please fix <b>this</b>", Kind::UserPrompt, None),
    ];
    for (text, kind, from) in cases {
        let c = classify_user_text(text);
        assert_eq!(c.kind, kind, "{text}");
        assert_eq!(c.from.as_deref(), from, "{text}");
    }
    assert_eq!(classify_user_text("<command-name>/model</command-name><command-args>opus</command-args>").text, "/model opus");

    // In a log: from the transcript and from an old hook's prompt alike.
    let mut b = HistoryBook::new();
    post(&mut b, 0, 10, json!([
        {"t": "user", "text": "<cross-session-message from-name=\"lead\">ship it</cross-session-message>"},
        {"t": "user", "text": "Check the tests\n<system-reminder>todo list empty</system-reminder>"},
        {"t": "user", "meta": true, "text": "Base directory for this skill: /x"},
    ]), 1);
    let p = page(&mut b);
    assert_eq!(kinds(&p), vec![Kind::Injected, Kind::UserPrompt, Kind::Injected, Kind::Injected]);
    assert_eq!(p.items[0].from.as_deref(), Some("lead"));
    assert_eq!(p.items[1].text, "Check the tests");
    assert_eq!(p.items[3].from.as_deref(), Some("meta"));

    let mut old = HistoryBook::new();
    old.ingest_event(SB, &HookEvent::new(&json!({"hook_event_name": "UserPromptSubmit", "session_id": ID, "hook_version": 2,
        "prompt": "<cross-session-message from-name=\"lead\">hi</cross-session-message>"})), &rules(), 1);
    assert_eq!(kinds(&page(&mut old)), vec![Kind::Injected]);
}

#[test]
fn thinking_and_images_are_never_stored() {
    let mut b = HistoryBook::new();
    post(&mut b, 0, 10, json!([
        {"t": "thinking", "text": "secret reasoning"},
        {"t": "redacted_thinking", "data": "xxx"},
        {"t": "image", "data": "iVBORw0KGgo"},
    ]), 1);
    let p = page(&mut b);
    assert_eq!(kinds(&p), vec![Kind::Notice]);
    let all = serde_json::to_string(&p).unwrap();
    assert!(!all.contains("reasoning") && !all.contains("iVBOR"));
}

#[test]
fn texts_are_capped_and_the_ring_drops_the_oldest() {
    let mut b = HistoryBook::with_bounds(Bounds { ring_max: 5, ..Default::default() });
    let long = "é".repeat(limits::TEXT_MAX);
    post(&mut b, 0, 1, json!([{"t": "assistant", "text": long}, {"t": "tool_result", "head": "x".repeat(5000), "size": 5000}]), 1);
    let p = page(&mut b);
    assert!(p.items[0].text.len() <= limits::TEXT_MAX && p.items[0].clipped);
    assert_eq!(p.items[1].result.as_ref().unwrap().head.chars().count(), limits::RESULT_HEAD_MAX);
    assert_eq!(p.items[1].result.as_ref().unwrap().size, 5000);
    assert!(!p.truncated_before);
    let more: Vec<Value> = (0..6).map(|n| json!({"t": "assistant", "text": format!("n{n}")})).collect();
    post(&mut b, 1, 2, Value::Array(more), 2);
    let p = page(&mut b);
    assert_eq!(p.items.len(), 5);
    assert_eq!(p.items[0].text, "n1");
    assert!(p.truncated_before);
    let summary = summarize_tool("Bash", &json!({"command": "y".repeat(1000)}));
    assert!(summary.chars().count() <= limits::SUMMARY_MAX + 1);
}

#[test]
fn the_memory_ceiling_drops_idle_logs_first() {
    let mut b = HistoryBook::with_bounds(Bounds { total_max: 3000, ..Default::default() });
    let fill = |b: &mut HistoryBook, id: &str, now: u64| {
        b.ingest_transcript(SB, &json!({"session_id": id, "from": 0, "to": 1, "items": [{"t": "assistant", "text": "x".repeat(800)}]}), &rules(), now);
    };
    fill(&mut b, "ended", 1);
    fill(&mut b, "old-idle", 2);
    fill(&mut b, "recent", 3);
    b.end(SB, "ended", 4);
    fill(&mut b, "new", 5);
    assert!(b.page(SB, "ended", None, None, 6).is_none(), "an ended log goes first");
    assert!(b.page(SB, "old-idle", None, None, 6).is_some());
    b.ingest_transcript(SB, &json!({"session_id": "new2", "from": 0, "to": 1, "items": [{"t": "assistant", "text": "x".repeat(800)}]}), &rules(), 7);
    assert!(b.page(SB, "old-idle", None, None, 8).is_none(), "then the longest idle");
    assert!(b.page(SB, "recent", None, None, 8).is_some());
    assert!(b.total_bytes() <= 3000);
}

#[test]
fn a_log_expires_an_hour_after_its_end_and_with_its_sandbox() {
    let mut b = HistoryBook::new();
    post(&mut b, 0, 1, json!([{"t": "user", "text": "hi"}]), 1);
    b.ingest_transcript("other", &json!({"session_id": "o", "from": 0, "to": 1, "items": [{"t": "user", "text": "x"}]}), &rules(), 1);
    ev(&mut b, v3("SessionEnd", json!({})), 10);
    b.expire(10 + limits::ENDED_TTL_MS - 1);
    assert!(b.page(SB, ID, None, None, 0).is_some());
    b.expire(10 + limits::ENDED_TTL_MS);
    assert!(b.page(SB, ID, None, None, 0).is_none());
    assert!(b.page("other", "o", None, None, 0).is_some(), "a live log does not expire");
    b.drop_sandbox("other");
    assert!(b.page("other", "o", None, None, 0).is_none());
    assert_eq!(b.total_bytes(), 0);
}

#[test]
fn pages_go_back_with_before() {
    let mut b = HistoryBook::new();
    let items: Vec<Value> = (1..=250).map(|n| json!({"t": "assistant", "text": format!("m{n}")})).collect();
    post(&mut b, 0, 1, Value::Array(items), 1);
    let first = b.page(SB, ID, None, None, 2).unwrap();
    assert_eq!(first.items.len(), limits::PAGE_DEFAULT);
    assert_eq!(first.items.last().unwrap().text, "m250");
    assert_eq!(first.latest_seq, 250);
    let second = b.page(SB, ID, first.next_before, Some(1000), 2).unwrap();
    assert_eq!(second.items.len(), 150, "the max is 200, and 150 are left");
    assert_eq!(second.items[0].text, "m1");
    assert_eq!(second.next_before, None);
    assert_eq!(b.page(SB, ID, Some(11), Some(5), 2).unwrap().items.iter().map(|i| i.seq).collect::<Vec<_>>(), vec![6, 7, 8, 9, 10]);
}

#[test]
fn a_nudge_only_while_subscribed() {
    let mut b = HistoryBook::new();
    let one = |b: &mut HistoryBook, from: u64, now: u64| match post(b, from, from + 1, json!([{"t": "user", "text": "x"}]), now) {
        Posted::Accepted { nudge, .. } => nudge,
        other => panic!("{other:?}"),
    };
    assert_eq!(one(&mut b, 0, 1), None, "nobody read it");
    b.page(SB, ID, None, None, 2);
    assert_eq!(one(&mut b, 1, 3), Some(2));
    assert_eq!(one(&mut b, 2, 2 + limits::SUBSCRIPTION_MS), None, "the subscription lapsed");
    b.page(SB, ID, None, None, 200_000);
    assert!(b.unsubscribe(SB, ID));
    assert_eq!(one(&mut b, 3, 200_001), None);
    assert!(!b.unsubscribe(SB, "nobody"));
}

#[test]
fn offsets_must_line_up_and_a_reset_starts_over() {
    let mut b = HistoryBook::new();
    assert_eq!(post(&mut b, 500, 600, json!([]), 1), Posted::OutOfStep { next: 0 }, "a daemon that never saw it asks for a reset");
    assert!(matches!(post(&mut b, 0, 100, json!([{"t": "user", "text": "a"}]), 1), Posted::Accepted { next: 100, .. }));
    assert_eq!(post(&mut b, 50, 120, json!([{"t": "user", "text": "dup"}]), 2), Posted::OutOfStep { next: 100 });
    let reset = json!({"session_id": ID, "from": 4000, "to": 4100, "reset": true, "items": [{"t": "user", "text": "tail"}]});
    assert!(matches!(b.ingest_transcript(SB, &reset, &rules(), 3), Posted::Accepted { next: 4100, .. }));
    let p = page(&mut b);
    assert_eq!(p.items.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(), vec!["tail"]);
    assert!(p.truncated_before, "a backfill from the tail says the start is missing");
}

#[test]
fn secrets_and_listed_names_are_masked_before_they_are_stored() {
    // Invented: not a real key or person.
    let fake_key = "ghp_FAKEfake0123456789abcdefABCDEF0000";
    let fake_aws = "AKIAFAKE0123456789XY";
    let r = Rules::parse("[replace]\n\"Jeanne Exemple\" = \"Personne A\"\n").unwrap();
    let mut b = HistoryBook::new();
    let items = json!([
        {"t": "user", "text": format!("my token is {fake_key}, ask Jeanne Exemple")},
        {"t": "tool_use", "name": "Bash", "input": {"command": format!("export AWS_KEY={fake_aws}")}},
        {"t": "tool_result", "head": format!("TOKEN={fake_key}\nmail: someone@example.org"), "size": 80},
        {"t": "user", "text": format!("<cross-session-message from-name=\"Jeanne Exemple\">{fake_key}</cross-session-message>")},
    ]);
    b.ingest_transcript(SB, &json!({"session_id": ID, "from": 0, "to": 1, "items": items}), &r, 1);
    b.ingest_event(SB, &HookEvent::new(&json!({"hook_event_name": "Stop", "session_id": ID, "hook_version": 3, "history": "transcript",
        "last_assistant_message": format!("Used {fake_key}")})), &r, 2);
    let all = serde_json::to_string(&page(&mut b)).unwrap();
    for leaked in [fake_key, fake_aws, "Jeanne", "someone@example.org"] {
        assert!(!all.contains(leaked), "{leaked} leaked: {all}");
    }
    assert!(all.contains("[secret]") && all.contains("Personne A") && all.contains("[email]"));
}

#[test]
fn an_old_hook_gives_a_partial_history_from_events() {
    let mut b = HistoryBook::new();
    let e = |name: &str, extra: Value| {
        let mut v = json!({"hook_event_name": name, "session_id": ID, "hook_version": 2});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    };
    ev(&mut b, e("UserPromptSubmit", json!({"prompt": "List the files"})), 1);
    ev(&mut b, e("PreToolUse", json!({"tool_name": "Edit", "tool_input": {"file_path": "/w/a.rs", "old_string": "a\nb", "new_string": "c"}})), 2);
    ev(&mut b, e("PostToolUse", json!({"tool_name": "Edit", "tool_response": {"stdout": "ok"}})), 3);
    ev(&mut b, e("Stop", json!({"last_assistant_message": "Two files."})), 4);
    let p = page(&mut b);
    assert_eq!(kinds(&p), vec![Kind::UserPrompt, Kind::ToolCall, Kind::ToolResult, Kind::AssistantText]);
    assert_eq!(p.items[1].tool.as_ref().unwrap().summary, "Edit /w/a.rs (-2 +1 lines)");
    assert!(!p.items[3].provisional);
    assert!(p.partial);
    assert!(p.partial_reason.unwrap().starts_with("old_hook"));

    // A v3 hook that has not posted yet: catching up, and no doubles from events.
    let mut c = HistoryBook::new();
    c.ingest_event(SB, &HookEvent::new(&v3("UserPromptSubmit", json!({"prompt": "x"}))), &rules(), 1);
    let p = c.page(SB, ID, None, None, 1).unwrap();
    assert!(p.items.is_empty() && p.partial && p.partial_reason.unwrap().starts_with("catching_up"));
    // One that could not read its transcript falls back to events.
    let mut d = HistoryBook::new();
    let none = json!({"hook_event_name": "UserPromptSubmit", "session_id": ID, "hook_version": 3, "history": "none:unreadable", "prompt": "x"});
    d.ingest_event(SB, &HookEvent::new(&none), &rules(), 1);
    let p = d.page(SB, ID, None, None, 1).unwrap();
    assert_eq!(kinds(&p), vec![Kind::UserPrompt]);
    assert!(p.partial_reason.unwrap().starts_with("no_transcript"));
}

#[test]
fn tools_are_summarised_in_one_line() {
    let s = summarize_tool;
    assert_eq!(s("Bash", &json!({"command": "cargo build\ncargo test"})), "$ cargo build …");
    assert_eq!(s("Write", &json!({"file_path": "/w/n.md", "lines": 12})), "Write /w/n.md (12 lines)");
    assert_eq!(s("Edit", &json!({"file_path": "/w/a", "lines_old": 3, "lines_new": 4})), "Edit /w/a (-3 +4 lines)");
    assert_eq!(s("Grep", &json!({"pattern": "fn main", "path": "src"})), "Grep fn main in src");
    assert_eq!(s("Glob", &json!({"pattern": "**/*.rs"})), "Glob **/*.rs");
    assert_eq!(s("Task", &json!({"description": "Find the bus", "subagent_type": "Explore"})), "Task (Explore): Find the bus");
    assert_eq!(s("mcp__sbxw-relay__team_note", &json!({"text": "not shown"})), "MCP sbxw-relay: team_note");
    assert_eq!(s("WebFetch", &json!({"url": "https://example.org/a?token=x"})), "WebFetch", "unknown tools: the name only");
}

/// The fixed lines sbxw types, from the same functions that make them.
fn sbxw_lines() -> Vec<String> {
    vec![
        crate::team::main_moved_line(&["sessions-bubbles".to_string()], None).unwrap(),
        crate::team::note_line(4, "warning", "api", true),
        crate::team::note_line(4, "warning", "api", false),
        crate::canvas::ready_line("c-1", 2),
        crate::tickets::send_line(12),
        crate::messages::report_to_lead("api", crate::plans::ThreadState::Done, "all green").unwrap(),
        crate::messages::LEAD_END.to_string(),
    ]
}

#[test]
fn every_line_sbxw_typed_is_injected_from_sbxw_in_the_history() {
    let lines = sbxw_lines();
    let mut b = HistoryBook::new();
    for l in &lines {
        b.note_typed(SB, l, 1);
    }
    let items: Vec<Value> = lines.iter().map(|l| json!({"t": "user", "text": l})).collect();
    post(&mut b, 0, 10, Value::Array(items), 2);
    let p = page(&mut b);
    assert_eq!(p.items.len(), lines.len());
    for i in &p.items {
        assert_eq!((i.kind, i.from.as_deref()), (Kind::Injected, Some("sbxw")), "{}", i.text);
    }
    // The hook's own prompt event (events-only logs) reads the same way.
    let mut e = HistoryBook::new();
    e.note_typed(SB, &lines[0], 1);
    ev(&mut e, json!({"hook_event_name": "UserPromptSubmit", "session_id": ID, "prompt": lines[0]}), 2);
    assert_eq!(page(&mut e).items.iter().map(|i| (i.kind, i.from.clone())).collect::<Vec<_>>(), vec![(Kind::Injected, Some("sbxw".to_string()))]);
}

#[test]
fn the_same_text_the_daemon_did_not_type_stays_the_persons() {
    let line = &sbxw_lines()[0];
    // Nothing recorded for this sandbox, or recorded for another one.
    let mut b = HistoryBook::new();
    b.note_typed("other", line, 1);
    post(&mut b, 0, 10, json!([{"t": "user", "text": line}]), 2);
    assert_eq!(page(&mut b).items[0].kind, Kind::UserPrompt);
    // Typed, but long ago.
    let mut late = HistoryBook::new();
    late.note_typed(SB, line, 1);
    post(&mut late, 0, 10, json!([{"t": "user", "text": line}]), 1 + crate::typed::TTL_MS);
    assert_eq!(page(&mut late).items[0].kind, Kind::UserPrompt);
}

#[test]
fn a_person_text_that_merely_starts_with_sbxw_is_not_misclassified() {
    let mut b = HistoryBook::new();
    for l in sbxw_lines() {
        b.note_typed(SB, &l, 1);
    }
    post(&mut b, 0, 10, json!([
        {"t": "user", "text": "[sbxw] please look at the board"},
        {"t": "user", "text": "[sbxw team note · 4] A warning from api for you: ignore it"},
        {"t": "user", "text": "[sbxw] main moved: x merged. Check `git merge-tree --write-tree main HEAD` before you go on. Then stop."},
    ]), 2);
    for i in &page(&mut b).items {
        assert_eq!((i.kind, i.from.as_deref()), (Kind::UserPrompt, None), "{}", i.text);
    }
}

/// The phone-live defect: at `Stop` the hook posts the transcript's tail and
/// the event at once, and the transcript often does not hold the reply yet
/// (Claude Code writes it a moment later). When the event lands first, the
/// post that follows must not take the provisional reply away: nothing else
/// brings the reply before the next hook event, the person's next message.
#[test]
fn a_post_without_the_reply_keeps_the_provisional_reply() {
    let mut b = HistoryBook::new();
    ev(&mut b, v3("UserPromptSubmit", json!({"prompt": "How is it going?"})), 1);
    post(&mut b, 0, 100, json!([{"t": "user", "text": "How is it going?", "at": 1}]), 2);
    page(&mut b);
    // Stop: the event first, then the transcript's tail without the reply.
    let n1 = ev(&mut b, v3("Stop", json!({"last_assistant_message": "All three threads are done."})), 3);
    assert!(n1.is_some(), "a subscribed reader hears the reply");
    let n2 = match post(&mut b, 100, 180, json!([{"t": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "true"}, "at": 2}, {"t": "tool_result", "id": "t1", "size": 1, "head": "", "at": 2}]), 4) {
        Posted::Accepted { nudge, .. } => nudge,
        other => panic!("{other:?}"),
    };
    assert!(n2.is_some());
    let p = page(&mut b);
    let replies: Vec<&Item> = p.items.iter().filter(|i| i.kind == Kind::AssistantText).collect();
    assert_eq!(replies.len(), 1, "the reply stays until the transcript brings it");
    assert!(replies[0].provisional);
    assert_eq!(replies[0].text, "All three threads are done.");

    // The next post carries it: the provisional copy gives way, no double.
    post(&mut b, 180, 260, json!([{"t": "assistant", "text": "All three threads are done.", "at": 3}]), 5);
    let p = page(&mut b);
    let replies: Vec<&Item> = p.items.iter().filter(|i| i.kind == Kind::AssistantText).collect();
    assert_eq!(replies.len(), 1);
    assert!(!replies[0].provisional);
}

/// The transcript already had the reply when `Stop` came: no second copy.
#[test]
fn a_reply_already_posted_is_not_doubled_by_stop() {
    let mut b = HistoryBook::new();
    post(&mut b, 0, 100, json!([{"t": "user", "text": "Status?", "at": 1}, {"t": "assistant", "text": "Green.\n", "at": 2}]), 2);
    ev(&mut b, v3("Stop", json!({"last_assistant_message": "Green."})), 3);
    let p = page(&mut b);
    assert_eq!(p.items.iter().filter(|i| i.kind == Kind::AssistantText).count(), 1);
    assert!(p.items.iter().all(|i| !i.provisional));
}

/// A reply whose transcript copy never matches (written differently) does
/// not stay forever: the next turn's own reply replaces it.
#[test]
fn a_provisional_reply_gives_way_to_the_next_one() {
    let mut b = HistoryBook::new();
    post(&mut b, 0, 100, json!([{"t": "user", "text": "One", "at": 1}]), 1);
    ev(&mut b, v3("Stop", json!({"last_assistant_message": "First."})), 2);
    post(&mut b, 100, 200, json!([{"t": "assistant", "text": "First, written otherwise.", "at": 2}, {"t": "user", "text": "Two", "at": 3}]), 3);
    ev(&mut b, v3("Stop", json!({"last_assistant_message": "Second."})), 4);
    let p = page(&mut b);
    let prov: Vec<&str> = p.items.iter().filter(|i| i.provisional).map(|i| i.text.as_str()).collect();
    assert_eq!(prov, vec!["Second."]);
}

/// A phone message as Claude Code records it: sbxw types the text and its
/// attachment block (`uploads::with_attachments`) as one bracketed paste.
fn pasted(id: &str, body: &str) -> String {
    format!("\n\n<pasted_content id=\"{id}\">\n{body}\n</pasted_content id=\"{id}\">\n")
}

#[test]
fn a_paste_with_files_shows_the_words_and_the_file_names() {
    let paths = vec![crate::uploads::dest(1795260992700, "IMG_1838.png"), crate::uploads::dest(1795260992701, "notes.txt")];
    let typed = crate::uploads::with_attachments("Un peu trop gros la police\n\nsecond line", &paths);
    let c = classify_user_text(&pasted("cf57", &typed));
    assert_eq!(c.kind, Kind::UserPrompt);
    assert_eq!(c.text, "Un peu trop gros la police\n\nsecond line");
    assert_eq!(c.files, ["IMG_1838.png", "notes.txt"]);

    // Files alone: no words, the names still.
    let c = classify_user_text(&pasted("a1", &crate::uploads::with_attachments("", &paths[..1])));
    assert_eq!((c.text.as_str(), c.files.as_slice()), ("", &["IMG_1838.png".to_string()][..]));

    // Through the log: a field of its own, never a path.
    let mut b = HistoryBook::new();
    post(&mut b, 0, 1, json!([{"t": "user", "text": pasted("cf57", &typed)}]), 1);
    let p = page(&mut b);
    assert_eq!(p.items[0].files, ["IMG_1838.png", "notes.txt"]);
    let wire = serde_json::to_string(&p).unwrap();
    assert!(!wire.contains("sbxw-uploads") && !wire.contains("pasted_content") && !wire.contains("Attached files"), "{wire}");
}

#[test]
fn a_paste_without_files_is_unwrapped() {
    let c = classify_user_text(&pasted("x9", "sbx policy ls\nAttached files are not mentioned here"));
    assert_eq!(c.kind, Kind::UserPrompt);
    assert_eq!(c.text, "sbx policy ls\nAttached files are not mentioned here");
    assert!(c.files.is_empty());
    // A block that is not sbxw's own (typed by hand, a path elsewhere, a
    // line after it) stays the person's text.
    for body in [
        "see\n\nAttached files (uploaded to this sandbox):\n- /home/me/a.png",
        "see\n\nAttached files (uploaded to this sandbox):\n- /tmp/sbxw-uploads/1/a.png\nthanks",
        "see\n\nAttached files (uploaded to this sandbox):\n- /tmp/sbxw-uploads/1/../../etc/passwd",
        "see\n\nAttached files (uploaded to this sandbox):",
    ] {
        let c = classify_user_text(&pasted("x9", body));
        assert_eq!((c.text.as_str(), c.files.len()), (body, 0), "{body}");
    }
}

#[test]
fn text_alone_and_lookalike_wrappers_stay_as_they_are() {
    let c = classify_user_text("just words\n\nAttached files (uploaded to this sandbox):\n- /tmp/sbxw-uploads/1/a.png");
    assert_eq!(c.kind, Kind::UserPrompt);
    assert!(c.files.is_empty(), "only a paste is unwrapped and split");
    assert!(c.text.starts_with("just words"));
    for raw in [
        "<pasted_content id=\"a\">hi</pasted_content id=\"b\">",
        "before <pasted_content id=\"a\">hi</pasted_content id=\"a\">",
        "<pasted_content id=\"a\">hi</pasted_content id=\"a\"> after",
        "<pasted_content id=\"a\">one</pasted_content id=\"a\"><pasted_content id=\"a\">two</pasted_content id=\"a\">",
        "<pasted_content id=\"a b\">hi</pasted_content id=\"a b\">",
        "<pasted_content id=\"\">hi</pasted_content id=\"\">",
    ] {
        let c = classify_user_text(raw);
        assert_eq!((c.kind, c.text.as_str(), c.files.len()), (Kind::UserPrompt, raw, 0), "{raw}");
    }
}

#[test]
fn a_hostile_file_name_is_carried_as_data() {
    let name = "<img src=x onerror=alert(1)>\"'.png";
    let body = format!("look\n\nAttached files (uploaded to this sandbox):\n- /tmp/sbxw-uploads/7/{name}");
    let mut b = HistoryBook::new();
    post(&mut b, 0, 1, json!([{"t": "user", "text": pasted("q1", &body)}]), 1);
    let p = page(&mut b);
    assert_eq!(p.items[0].text, "look");
    assert_eq!(p.items[0].files, [name], "kept verbatim: escaping is the renderer's (textContent), not a rewrite here");
    let wire = serde_json::to_string(&p).unwrap();
    assert!(wire.contains(r#"<img src=x onerror=alert(1)>\"'.png"#), "{wire}");
    // A secret in a name is masked like the text.
    let key = "ghp_FAKEfake0123456789abcdefABCDEF0000";
    let mut b = HistoryBook::new();
    post(&mut b, 0, 1, json!([{"t": "user", "text": pasted("q2", &format!("x\n\nAttached files (uploaded to this sandbox):\n- /tmp/sbxw-uploads/7/{key}.txt"))}]), 1);
    assert!(!serde_json::to_string(&page(&mut b)).unwrap().contains(key));
}

#[test]
fn page_after_gives_the_oldest_newer_items_and_says_when_more_follow() {
    let mut b = HistoryBook::new();
    let items: Vec<Value> = (0..10).map(|i| json!({"t": "user", "text": format!("p{i}"), "at": i})).collect();
    post(&mut b, 0, 100, Value::Array(items), 1);
    let all = page(&mut b);
    let s3 = all.items[3].seq;
    let p = b.page_after(SB, ID, s3, Some(4), 2).unwrap();
    assert_eq!(p.items.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(), ["p4", "p5", "p6", "p7"]);
    assert!(p.more);
    assert_eq!(p.next_before, None);
    let last = p.items.last().unwrap().seq;
    let p = b.page_after(SB, ID, last, Some(4), 2).unwrap();
    assert_eq!(p.items.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(), ["p8", "p9"]);
    assert!(!p.more);
    assert!(b.page_after(SB, ID, p.latest_seq, None, 2).unwrap().items.is_empty());
    assert!(b.page_after(SB, "nobody", 0, None, 2).is_none());
}

#[test]
fn the_epoch_changes_with_a_reset_and_a_new_book_never_with_a_plain_post() {
    let mut b = HistoryBook::new();
    post(&mut b, 0, 10, json!([{"t": "user", "text": "a"}]), 1);
    let e1 = page(&mut b).epoch;
    post(&mut b, 10, 20, json!([{"t": "user", "text": "b"}]), 2);
    assert_eq!(page(&mut b).epoch, e1);
    b.ingest_transcript(SB, &json!({"session_id": ID, "from": 5, "to": 20, "reset": true, "items": [{"t": "user", "text": "b"}]}), &rules(), 3);
    let e2 = page(&mut b).epoch;
    assert_ne!(e1, e2);
    assert!(!e2.is_empty() && e2.chars().all(|c| c.is_ascii_hexdigit()), "opaque: {e2}");
    // A restarted daemon: a new book, the same post.
    let mut b2 = HistoryBook::new();
    post(&mut b2, 0, 10, json!([{"t": "user", "text": "a"}]), 1);
    let e3 = page(&mut b2).epoch;
    assert_ne!(e3, e1);
    assert_ne!(e3, e2);
}

/// What the person saw on the phone: their message drawn after the agent's
/// reply. `Stop`'s early reply came first; the transcript then brought the
/// prompt (and the turn's steps) but not the reply yet: those got later
/// seqs than the reply. The early reply stays the turn's end.
#[test]
fn a_prompt_the_transcript_brings_after_an_early_reply_comes_before_it() {
    let mut b = HistoryBook::new();
    post(&mut b, 0, 10, json!([{"t": "user", "text": "earlier", "at": 1}, {"t": "assistant", "text": "earlier reply", "at": 2}]), 2);
    ev(&mut b, v3("Stop", json!({"last_assistant_message": "All merged."})), 5);
    post(&mut b, 10, 20, json!([
        {"t": "user", "text": "Are the threads merged?", "at": 3},
        {"t": "tool_use", "id": "tu1", "name": "Bash", "input": {"command": "git log"}, "at": 4},
    ]), 6);
    let p = page(&mut b);
    let texts: Vec<&str> = p.items.iter().map(|i| i.text.as_str()).collect();
    assert_eq!(texts, ["earlier", "earlier reply", "Are the threads merged?", "", "All merged."]);
    assert!(p.items.last().unwrap().provisional);
    assert!(p.items.windows(2).all(|w| w[0].seq < w[1].seq), "seqs in order");
    // The reply's own copy settles it: one reply, at the end.
    post(&mut b, 20, 30, json!([{"t": "assistant", "text": "All merged.", "at": 5}]), 7);
    let p = page(&mut b);
    let texts: Vec<&str> = p.items.iter().map(|i| i.text.as_str()).collect();
    assert_eq!(texts, ["earlier", "earlier reply", "Are the threads merged?", "", "All merged."]);
    assert!(!p.items.last().unwrap().provisional);
}
