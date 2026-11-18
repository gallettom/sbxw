use super::*;

fn data() -> J {
    json!({
        "sandboxes": [{ "name": "web-app", "status": "running" }, { "name": "docs", "status": "stopped" }, { "name": "chat-1", "status": "running" }],
        "app": { "groups": { "web-app": "#e06c75" } },
    })
}

fn asked(name: &'static str, args: J) -> Asked {
    Asked { name, args: args.as_object().cloned().unwrap_or_default() }
}

fn calls(out: &[Action]) -> Vec<(String, String)> {
    out.iter()
        .filter_map(|a| match a {
            Action::Call(c) => Some((c.path.clone(), c.tag.clone())),
            _ => None,
        })
        .collect()
}

fn ui(out: &[Action]) -> J {
    out.iter()
        .rev()
        .find_map(|a| match a {
            Action::Set { key, value } if key == UI => Some(value.clone()),
            _ => None,
        })
        .expect("the view's data")
}

/// The stand-in's answers, fed back as the daemon would.
fn answer(st: &mut State, cx: &Ctx, out: &[Action]) -> Vec<Action> {
    let mut sb = Standin { data: &mut json!({}), publish: &mut |_, _| {} };
    let mut all = vec![];
    for (path, tag) in calls(out) {
        let (code, body) = standin(&mut sb, "GET", &path, &J::Null).expect("a stand-in route");
        let call = Call::get(path, "x").tag(&tag);
        all.extend(st.on_answer(cx, &call, Ok((code, body))));
    }
    all
}

#[test]
fn every_sandbox_newest_first_filters_and_pages_before_the_cap() {
    let d = data();
    let cx = Ctx { data: &d };
    let mut st = State::default();
    let out = st.act(&cx, &asked("commits_show", json!({}))).unwrap();
    assert!(out.iter().any(|a| matches!(a, Action::OpenPanel(s) if s == SCREEN)), "the overlay opens");
    let c = calls(&out);
    assert_eq!(c.len(), 3, "one read per sandbox: {c:?}");
    assert!(c.iter().all(|(p, _)| p.contains("/commits?limit=50")));
    let out = answer(&mut st, &cx, &out);
    let v = ui(&out);
    let rows = v["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 7, "web-app 5, docs 2; chat-1 has none");
    let at: Vec<i64> = rows.iter().map(|r| r["at"].as_i64().unwrap()).collect();
    assert!(at.windows(2).all(|w| w[0] >= w[1]), "newest first");
    assert_eq!(rows[0]["color"], "#e06c75", "the sandbox's colour");
    assert!(v["notes"].as_array().unwrap().iter().any(|n| n["text"].as_str().unwrap().starts_with("chat-1: not available")));
    // A thread's branch reads as the thread.
    let t = rows.iter().find(|r| r["branch"] == "sbxw/checkout-copy").unwrap();
    assert_eq!(t["branch_label"], "checkout-copy");
    // Filter by subject and author, case-insensitive; only unmerged.
    let v = ui(&st.act(&cx, &asked("commits_filter", json!({ "text": "CHECKOUT" }))).unwrap());
    assert_eq!(v["rows"].as_array().unwrap().len(), 2);
    let v = ui(&st.act(&cx, &asked("commits_filter", json!({ "text": "lee" }))).unwrap());
    assert_eq!(v["rows"].as_array().unwrap().len(), 1, "web-app's Lee Chen (data-pipeline is not listed in this data)");
    st.act(&cx, &asked("commits_filter", json!({ "text": "" }))).unwrap();
    let v = ui(&st.act(&cx, &asked("commits_unmerged", json!({ "on": true }))).unwrap());
    assert!(v["rows"].as_array().unwrap().iter().all(|r| r["merged"] == false));
    assert_eq!(v["rows"].as_array().unwrap().len(), 3);
}

#[test]
fn a_page_of_fifty_then_show_more_reads_the_next_page_where_there_is_one() {
    let mut d = data();
    let mut list = vec![];
    for i in 0..120 {
        let h = format!("{:040x}", i + 1);
        list.push(json!({ "hash": h, "short": &h[33..], "author": "A", "at": 10_000 - i, "subject": format!("c{i}"), "body": "", "branch": "main", "thread": null, "merged": true, "files": 1, "repo": "" }));
    }
    d["sandboxes"] = json!([{ "name": "web-app" }]);
    let cx = Ctx { data: &d };
    let mut st = State::default();
    let out = st.act(&cx, &asked("commits_show", json!({}))).unwrap();
    let mut fx = json!({ "commits": { "web-app": list }, "commit_details": {} });
    let feed = |st: &mut State, out: &[Action], fx: &mut J| -> Vec<Action> {
        let mut sb = Standin { data: fx, publish: &mut |_, _| {} };
        let mut all = vec![];
        for (path, tag) in calls(out) {
            let (code, body) = standin(&mut sb, "GET", &path, &J::Null).unwrap();
            all.extend(st.on_answer(&cx, &Call::get(path, "x").tag(&tag), Ok((code, body))));
        }
        all
    };
    let v = ui(&feed(&mut st, &out, &mut fx));
    assert_eq!(v["rows"].as_array().unwrap().len(), 50);
    assert_eq!((v["count"]["total"].as_u64(), v["count"]["more"].as_u64()), (Some(50), Some(0)), "the 50 read so far all show");
    let out = st.act(&cx, &asked("commits_more", json!({}))).unwrap();
    let c = calls(&out);
    assert_eq!(c.len(), 1);
    assert!(c[0].0.contains(&format!("before={:040x}", 50)), "{c:?}");
    let v = ui(&feed(&mut st, &out, &mut fx));
    assert_eq!(v["rows"].as_array().unwrap().len(), 100);
    // A filter runs over everything read before the page is cut.
    let v = ui(&st.act(&cx, &asked("commits_filter", json!({ "text": "c9" }))).unwrap());
    let n = v["rows"].as_array().unwrap().len();
    assert_eq!(n, 11, "c9, c90..c99");
}

#[test]
fn opened_from_a_pane_the_sandbox_is_preselected_and_a_branch_narrows_it() {
    let d = data();
    let cx = Ctx { data: &d };
    let mut st = State::default();
    let out = st.act(&cx, &asked("commits_show", json!({ "sandbox": "docs" }))).unwrap();
    let c = calls(&out);
    assert_eq!(c.len(), 1);
    assert!(c[0].0.starts_with("/api/sandboxes/docs/commits"));
    let v = ui(&answer(&mut st, &cx, &out));
    assert_eq!(v["sandbox"], "docs");
    assert!(v["rows"].as_array().unwrap().iter().all(|r| r["sandbox"] == "docs"));
    // From the Worktrees view: one thread's branch.
    let out = st.act(&cx, &asked("commits_show", json!({ "sandbox": "web-app", "branch": "sbxw/checkout-copy" }))).unwrap();
    assert!(calls(&out)[0].0.contains("ref=sbxw%2Fcheckout-copy"), "{:?}", calls(&out));
    assert!(st.act(&cx, &asked("commits_show", json!({ "sandbox": "nope" }))).is_err());
    assert!(st.act(&cx, &asked("commits_show", json!({ "sandbox": "web-app", "branch": "--all" }))).is_err());
    // The pane's menu item carries its sandbox.
    let x = extras(&cx);
    let pane = x.iter().find(|e| e.place == Place::Pane("docs".into())).unwrap();
    assert_eq!(pane.intent.as_ref().unwrap().1["sandbox"], "docs");
    assert!(x.iter().any(|e| e.place == Place::Rail { zone: Zone::Browse, order: 10 }));
}

#[test]
fn a_row_expands_in_place_with_its_files_and_a_file_shows_its_diff() {
    let d = data();
    let cx = Ctx { data: &d };
    let mut st = State::default();
    let out = st.act(&cx, &asked("commits_show", json!({ "sandbox": "web-app" }))).unwrap();
    let v = ui(&answer(&mut st, &cx, &out));
    let row = v["rows"].as_array().unwrap().iter().find(|r| r["files"] == 3).unwrap().clone();
    let id = row["id"].as_str().unwrap().to_string();
    let out = st.act(&cx, &asked("commits_expand", json!({ "id": id }))).unwrap();
    assert!(ui(&out)["detail"]["loading"] == true);
    let v = ui(&answer(&mut st, &cx, &out));
    let det = &v["detail"];
    let files = det["files"].as_array().unwrap();
    assert_eq!(files.len(), 3);
    assert!(files.iter().any(|f| f["binary"] == true && f["note"].as_str().unwrap().contains("Binary")));
    assert_eq!(det["truncated"], true);
    assert!(v["rows"].as_array().unwrap().iter().any(|r| r["id"] == id.as_str() && r["expanded"] == true));
    // A file's diff: groups of kinds, the header lines left out.
    let p = files[0]["path"].as_str().unwrap().to_string();
    let v = ui(&st.act(&cx, &asked("commits_file", json!({ "path": p }))).unwrap());
    let g = v["detail"]["files"][0]["groups"].as_array().unwrap();
    let kinds: Vec<&str> = g.iter().map(|x| x["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["hunk", "ctx", "del", "add", "ctx"]);
    assert!(!g.iter().any(|x| x["text"].as_str().unwrap().starts_with("+++")));
    // The big generated file: drawn up to its cap, the rest said.
    let big = files.iter().find(|f| f["path"] == "src/generated/rates.ts").unwrap()["path"].as_str().unwrap().to_string();
    let v = ui(&st.act(&cx, &asked("commits_file", json!({ "path": big }))).unwrap());
    let f = v["detail"]["files"].as_array().unwrap().iter().find(|f| f["path"] == big.as_str()).unwrap().clone();
    assert!(f["note"].as_str().unwrap().starts_with("Truncated"), "{}", f["note"]);
    // A path the commit does not list is refused.
    assert!(st.act(&cx, &asked("commits_file", json!({ "path": "/etc/passwd" }))).is_err());
    // Pressed again: folded.
    let v = ui(&st.act(&cx, &asked("commits_expand", json!({ "id": id }))).unwrap());
    assert!(v["detail"].is_null());
    // Copy: the full hash, on a press.
    let out = st.act(&cx, &asked("commits_copy", json!({ "id": id }))).unwrap();
    assert!(out.iter().any(|a| matches!(a, Action::Intent { name, args } if name == "copy_text" && args["text"] == row["hash"])));
    assert!(st.act(&cx, &asked("commits_copy", json!({ "id": "web-app:abc" }))).is_err());
}

#[test]
fn keys_move_the_selection_and_enter_opens_it() {
    let d = data();
    let cx = Ctx { data: &d };
    let mut st = State::default();
    let out = st.act(&cx, &asked("commits_show", json!({ "sandbox": "web-app" }))).unwrap();
    answer(&mut st, &cx, &out);
    st.act(&cx, &asked("commits_key", json!({ "key": "down" }))).unwrap();
    let v = ui(&st.act(&cx, &asked("commits_key", json!({ "key": "down" }))).unwrap());
    let rows = v["rows"].as_array().unwrap();
    assert_eq!(rows[1]["selected"], true);
    let out = st.act(&cx, &asked("commits_key", json!({ "key": "enter" }))).unwrap();
    assert_eq!(calls(&out).len(), 1, "the detail is read");
    let v = ui(&st.act(&cx, &asked("commits_key", json!({ "key": "up" }))).unwrap());
    assert_eq!(v["rows"][0]["selected"], true);
}

#[test]
fn a_diff_is_a_few_groups_and_capped() {
    let p: String = (0..2000).map(|i| if i % 2 == 0 { format!("+a{i}\n") } else { format!("-b{i}\n") }).collect();
    let (g, left) = diff_groups(&p);
    assert_eq!(left, 2000 - DIFF_LINES);
    assert!(g.len() <= DIFF_LINES);
    let (g, _) = diff_groups("@@ -1 +1 @@\n+one\n+two\n+three\n context\n");
    assert_eq!(g.len(), 3, "consecutive lines of a kind are one element");
}

/// The screen renders every state the model makes: the list, a row open
/// with a file's diff, filtered to nothing, nothing read yet.
#[test]
fn the_screen_renders_every_state() {
    let dir = std::env::temp_dir().join("sbxw-no-overrides");
    let d = data();
    let cx = Ctx { data: &d };
    let mut st = State::default();
    let render = |ui: J| {
        let mut data = d.clone();
        data[UI] = ui;
        let s = crate::screen::ScreenState::new(SCREEN, &dir, data);
        assert!(s.error.is_none(), "{:?}", s.error);
    };
    render(J::Null);
    let out = st.act(&cx, &asked("commits_show", json!({}))).unwrap();
    render(ui(&out));
    let v = ui(&answer(&mut st, &cx, &out));
    render(v.clone());
    let id = v["rows"].as_array().unwrap().iter().find(|r| r["files"] == 3).unwrap()["id"].as_str().unwrap().to_string();
    let out = st.act(&cx, &asked("commits_expand", json!({ "id": id }))).unwrap();
    render(ui(&out));
    answer(&mut st, &cx, &out);
    for p in ["src/checkout/Payment.tsx", "src/generated/rates.ts"] {
        st.act(&cx, &asked("commits_file", json!({ "path": p }))).unwrap();
    }
    render(ui(&st.act(&cx, &asked("commits_unmerged", json!({ "on": false }))).unwrap()));
    render(ui(&st.act(&cx, &asked("commits_filter", json!({ "text": "zzz" }))).unwrap()));
}

/// Opened on one commit (a dot of the Worktrees graph): its detail is asked
/// at once and its row is the open one; a hash needs its sandbox.
#[test]
fn opened_on_a_commit_it_reads_and_opens_it() {
    let d = data();
    let cx = Ctx { data: &d };
    let mut st = State::default();
    let h = "b1b942e56cefd1e55d21f9beebbedf0187522f81";
    let out = st.act(&cx, &asked("commits_show", json!({ "sandbox": "web-app", "branch": "sbxw/checkout-copy", "hash": h }))).unwrap();
    let c = calls(&out);
    assert!(c.iter().any(|(p, _)| p == &format!("/api/sandboxes/web-app/commits/{h}")), "{c:?}");
    let more = answer(&mut st, &cx, &out);
    let v = ui(&more);
    let row = v["rows"].as_array().unwrap().iter().find(|r| r["hash"] == h).cloned().expect("listed");
    assert_eq!(row["expanded"], true, "{row}");
    assert!(st.act(&cx, &asked("commits_show", json!({ "hash": h }))).is_err(), "a hash without its sandbox");
    assert!(st.act(&cx, &asked("commits_show", json!({ "sandbox": "web-app", "hash": "zz" }))).is_err());
}
