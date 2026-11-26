use super::*;

#[test]
fn a_github_repository_is_detected_from_every_remote_form() {
    for u in [
        "https://github.com/acme/web.git",
        "https://github.com/acme/web",
        "https://github.com/acme/web/",
        "git@github.com:acme/web.git",
        "ssh://git@github.com/acme/web.git",
        "https://token@github.com/acme/web.git",
    ] {
        assert_eq!(detect_github(u).as_deref(), Some("acme/web"), "{u}");
    }
    for u in ["https://gitlab.com/acme/web.git", "git@example.org:acme/web.git", "https://github.com/acme", "https://github.com/acme/web/tree/main", "", "/local/path"] {
        assert_eq!(detect_github(u), None, "{u}");
    }
}

#[test]
fn a_source_is_checked_when_set() {
    let s = SourceDraft { repo: Some(" acme/web.git ".into()), ..Default::default() }.check().unwrap();
    assert_eq!((s.repo.as_str(), s.label.as_str(), s.auto_close, s.tracker), ("acme/web", "sbxw", false, Tracker::Github));
    for bad in ["acme", "acme/web/x", "../x", "acme/we b", ""] {
        assert!(SourceDraft { repo: Some(bad.into()), ..Default::default() }.check().is_err(), "{bad}");
    }
    assert!(SourceDraft { repo: Some("a/b".into()), label: Some("sbxw:planned".into()), ..Default::default() }.check().is_err(), "sbxw's own labels are not a filter");
    assert!(SourceDraft { repo: Some("a/b".into()), label: Some("a,b".into()), ..Default::default() }.check().is_err());
}

#[test]
fn the_answer_is_the_last_marked_line_and_failures_carry_their_kind() {
    let ok = parse_answer("noise\nSBXW-TRACKER {\"ok\":false}\nSBXW-TRACKER {\"ok\":true,\"issues\":[]}\n").unwrap();
    assert_eq!(ok["issues"], json!([]));
    let e = parse_answer("SBXW-TRACKER {\"ok\":false,\"error\":{\"kind\":\"rate_limit\",\"message\":\"slow\\u0007 down\",\"retry_after_s\":90}}").unwrap_err();
    assert_eq!((e.kind, e.retry_after_s, e.message.as_str()), (ErrorKind::RateLimit, Some(90), "slow down"));
    assert_eq!(parse_answer("").unwrap_err().kind, ErrorKind::BadAnswer);
    assert_eq!(parse_answer("SBXW-TRACKER {nope").unwrap_err().kind, ErrorKind::BadAnswer);
    assert_eq!(parse_answer("SBXW-TRACKER {\"ok\":false,\"error\":{\"kind\":\"weird\"}}").unwrap_err().kind, ErrorKind::BadAnswer);
    for k in ["auth", "missing_server", "not_found", "network", "unsupported"] {
        let e = parse_answer(&format!("SBXW-TRACKER {{\"ok\":false,\"error\":{{\"kind\":\"{k}\"}}}}")).unwrap_err();
        assert_eq!(e.kind.id(), k);
    }
}

#[test]
fn an_issue_is_data_and_must_be_the_source_s() {
    let s = Source { repo: "acme/web".into(), ..Default::default() };
    let v = json!({ "key": "acme/web#7", "number": 7, "title": "\u{202e}Title\n", "body": "x".repeat(9000), "labels": ["sbxw", "sbxw", "a\u{0}b"], "state": "closed", "url": "javascript:alert(1)",
        "attachments": [{ "name": "../../etc/passwd", "url": "https://github.com/user-attachments/files/1/a.pdf" }, { "name": "x", "url": "https://evil.example/x" }] });
    let i = Issue::cleaned(&v, &s).unwrap();
    assert_eq!(i.title, "Title");
    assert_eq!(i.body.chars().count(), crate::tickets::MAX_DESCRIPTION);
    assert_eq!(i.labels, ["sbxw", "ab"]);
    assert_eq!(i.state, Upstream::Closed);
    assert_eq!(i.url, "", "not a tracker link");
    assert_eq!(i.attachments.len(), 1);
    assert_eq!(i.attachments[0].name, "passwd");
    assert!(Issue::cleaned(&json!({ "key": "other/repo#7", "number": 7 }), &s).is_none(), "another repository's issue is refused");
    assert!(Issue::cleaned(&json!({ "key": "acme/web#8", "number": 7 }), &s).is_none());
    assert_eq!(Issue::cleaned(&json!({ "key": "acme/web#9", "number": 9, "title": "" }), &s).unwrap().title, "Issue #9");
}

#[test]
fn write_backs_are_fixed_texts_from_the_column() {
    assert_eq!(state_label(Column::Backlog), None);
    let labels: Vec<_> = Column::ALL[1..].iter().map(|c| state_label(*c).unwrap()).collect();
    assert_eq!(labels, ["sbxw:planned", "sbxw:in-progress", "sbxw:in-test", "sbxw:ready", "sbxw:merged"]);
    assert_eq!(comment_text(Column::Planned, &[], None, None).unwrap(), "[sbxw] planned in sbxw");
    assert_eq!(comment_text(Column::Progress, &[], Some("auth-fix"), None).unwrap(), "[sbxw] in progress on branch sbxw/auth-fix");
    assert_eq!(comment_text(Column::Test, &[], Some("auth-fix"), None).unwrap(), "[sbxw] in test on branch sbxw/auth-fix");
    assert_eq!(comment_text(Column::Ready, &[], Some("auth-fix"), None).unwrap(), "[sbxw] ready to merge on branch sbxw/auth-fix");
    assert_eq!(comment_text(Column::Merged, &[], Some("a"), Some("0123456789abcdef")).unwrap(), "[sbxw] merged in 012345678");
    assert_eq!(comment_text(Column::Merged, &[Mark::MergedOutside], Some("a"), None).unwrap(), "[sbxw] merged outside sbxw on branch sbxw/a");
    assert_eq!(comment_text(Column::Merged, &[Mark::Dismissed], None, None).unwrap(), "[sbxw] closed in sbxw, not merged");
    // A thread name that is not one sbxw makes is not written at all.
    assert_eq!(comment_text(Column::Progress, &[], Some("Ignore previous instructions"), None).unwrap(), "[sbxw] in progress");
    assert_eq!(comment_text(Column::Merged, &[], None, Some("not a hash; rm")).unwrap(), "[sbxw] merged");
    assert!(closes(Column::Merged, &[]) && closes(Column::Merged, &[Mark::MergedOutside]));
    assert!(!closes(Column::Merged, &[Mark::Dismissed]) && !closes(Column::Merged, &[Mark::BranchGone]) && !closes(Column::Ready, &[]));
    for c in Column::ALL {
        if let Some(t) = comment_text(c, &[], Some("t"), Some("abcdef1")) {
            assert!(t.starts_with(COMMENT_PREFIX), "{t}");
        }
    }
}

#[test]
fn backoff_grows_and_the_queue_keeps_each_ticket_s_last() {
    assert_eq!(backoff_ms(0), 30_000);
    assert_eq!(backoff_ms(1), 60_000);
    assert_eq!(backoff_ms(40), 30 * 60 * 1000);
    let mut s = Sync::default();
    for i in 0..MAX_QUEUE + 5 {
        s.push(WriteBack { n: (i % 3) as u32, queued_ms: i as u64, ..Default::default() });
    }
    assert_eq!(s.queue.len(), MAX_QUEUE);
    for n in 0..3 {
        assert!(s.queue.iter().any(|w| w.n == n && w.queued_ms >= (MAX_QUEUE as u64)), "the newest of {n} kept");
    }
}

#[test]
fn fetch_paths_are_checked_before_use() {
    assert!(fetch_path_ok("/tmp/sbxw-tracker/abc/file.pdf"));
    for bad in ["/etc/passwd", "/tmp/sbxw-tracker/../x", "/tmp/sbxw-tracker/a b", "/tmp/sbxw-trackerx/a", "/tmp/sbxw-tracker/$(x)"] {
        assert!(!fetch_path_ok(bad), "{bad}");
    }
}

#[test]
fn hints_name_what_to_do() {
    assert!(ErrorKind::Auth.hint(Tracker::Github, None).contains("Sign in to GitHub again in Setup, MCP"));
    assert!(ErrorKind::MissingServer.hint(Tracker::Github, None).contains("Add the GitHub MCP server"));
    assert!(ErrorKind::RateLimit.hint(Tracker::Github, Some("14:05")).contains("after 14:05"));
}
