use super::*;

fn fx() -> J {
    serde_json::from_str(include_str!("../../fixtures/worktrees/standin.json")).unwrap()
}

fn data() -> J {
    let f = fx();
    let mut d = json!({ "sandboxes": f["daemon"]["sandboxes"], "sessions": f["daemon"]["sessions"], "app": { "groups": { "web-app": "#e06c75" } } });
    d["threads"] = json!({});
    d
}

fn asked(name: &'static str, args: J) -> Asked {
    Asked { name, args: args.as_object().cloned().unwrap_or_default() }
}

fn ui(out: &[Action]) -> J {
    out.iter().rev().find_map(|a| match a {
        Action::Set { key, value } if key == UI => Some(value.clone()),
        _ => None,
    }).expect("the view's data")
}

/// Answers every read the way the daemon would (the fixture's graph).
fn answer(st: &mut State, cx: &Ctx, mut out: Vec<Action>) -> J {
    let f = fx();
    let mut last = J::Null;
    let mut n = 0;
    while let Some(i) = out.iter().position(|a| matches!(a, Action::Call(_))) {
        let Action::Call(c) = out.remove(i) else { unreachable!() };
        let sb = c.path.trim_start_matches("/api/sandboxes/").trim_end_matches("/worktrees").to_string();
        let more = st.on_answer(cx, &c, Ok((200, f["daemon"]["worktrees_of"][&sb].clone())));
        last = ui(&more);
        out.extend(more);
        n += 1;
    }
    assert_eq!(n, 5, "one read per sandbox");
    last
}

#[test]
fn every_sandbox_s_worktrees_with_their_numbers_and_thread_state() {
    let d = data();
    let cx = Ctx { data: &d };
    let mut st = State::default();
    let out = st.act(&cx, &asked("worktrees_show", json!({}))).unwrap();
    assert!(out.iter().any(|a| matches!(a, Action::OpenPanel(s) if s == SCREEN)));
    let calls = out.iter().filter(|a| matches!(a, Action::Call(_))).count();
    assert_eq!(calls, 5, "five sandboxes, under the cap of {PARALLEL}");
    let v = answer(&mut st, &cx, out);
    let blocks = v["blocks"].as_array().unwrap();
    assert_eq!(blocks.iter().map(|b| b["sandbox"].as_str().unwrap()).collect::<Vec<_>>(), ["web-app", "docs"]);
    assert_eq!(blocks[0]["color"], "#e06c75");
    let rows = blocks[0]["rows"].as_array().unwrap();
    let r = |t: &str| rows.iter().find(|r| r["thread"] == t).unwrap().clone();
    let c = r("checkout-copy");
    assert_eq!((c["path"].as_str(), c["ahead"].as_u64(), c["behind"].as_u64(), c["dirty"].as_u64(), c["merged"].as_bool(), c["word"].as_str()), (Some(".sbxw-worktrees/checkout-copy"), Some(2), Some(1), Some(3), Some(false), Some("working")));
    assert_eq!(c["subject"], "Checkout: copy for the payment step");
    assert_eq!(r("header-fix")["merged"], true);
    assert_eq!(r("header-fix")["word"], "wants you");
    assert_eq!(r("search-index")["has_branch"], false);
    assert!(v["none"].as_array().unwrap().iter().any(|n| n == "chat-0412"));
    assert!(v["notes"].as_array().unwrap().iter().any(|n| n["text"].as_str().unwrap().contains("not in a git repository")));
    // Filter.
    let v = ui(&st.act(&cx, &asked("worktrees_filter", json!({ "text": "release" }))).unwrap());
    assert_eq!(v["blocks"].as_array().unwrap().len(), 1);
    assert_eq!(v["blocks"][0]["sandbox"], "docs");
}

#[test]
fn a_row_links_to_its_terminal_commits_and_files_by_id_only() {
    let d = data();
    let cx = Ctx { data: &d };
    let mut st = State::default();
    let out = st.act(&cx, &asked("worktrees_show", json!({}))).unwrap();
    answer(&mut st, &cx, out);
    let intent = |out: &[Action]| out.iter().find_map(|a| match a {
        Action::Intent { name, args } => Some((name.clone(), args.clone())),
        _ => None,
    }).unwrap();
    let out = st.act(&cx, &asked("worktrees_terminal", json!({ "id": "web-app:checkout-copy" }))).unwrap();
    assert_eq!(intent(&out), ("thread_open".into(), json!({ "sandbox": "web-app", "name": "checkout-copy" })));
    assert!(out.iter().any(|a| matches!(a, Action::ClosePanel(s) if s == SCREEN)), "the terminal shows");
    let out = st.act(&cx, &asked("worktrees_commits", json!({ "id": "web-app:checkout-copy" }))).unwrap();
    assert_eq!(intent(&out), ("commits_show".into(), json!({ "sandbox": "web-app", "branch": "sbxw/checkout-copy" })));
    let out = st.act(&cx, &asked("worktrees_files", json!({ "id": "docs:release-notes" }))).unwrap();
    assert_eq!(intent(&out), ("files_open".into(), json!({ "sandbox": "docs", "worktree": "release-notes" })));
    assert!(st.act(&cx, &asked("worktrees_files", json!({ "id": "docs:/etc" }))).is_err());
}

#[test]
fn paths_are_short_as_the_sidebar_shows_them() {
    assert_eq!(short_path("/Users/ana/dev/web/.sbxw-worktrees/x", Some("/Users/ana/dev/web")), ".sbxw-worktrees/x");
    assert_eq!(short_path("/Users/ana/other/.sbxw-worktrees/x", Some("/Users/ana/dev/web")), "~/other/.sbxw-worktrees/x");
    assert_eq!(short_path("/srv/x", None), "/srv/x");
}

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
    let out = st.act(&cx, &asked("worktrees_show", json!({}))).unwrap();
    render(ui(&out));
    render(answer(&mut st, &cx, out));
    render(ui(&st.act(&cx, &asked("worktrees_filter", json!({ "text": "zzz" }))).unwrap()));
}
