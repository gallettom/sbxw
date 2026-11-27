use super::*;

fn fx() -> J {
    serde_json::from_str(include_str!("../../fixtures/worktrees/standin.json")).unwrap()
}

fn data() -> J {
    let f = fx();
    let mut d = json!({ "sandboxes": f["daemon"]["sandboxes"], "sessions": f["daemon"]["sessions"], "plans": f["daemon"]["plans"], "app": { "groups": { "web-app": "#e06c75" } } });
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
    assert_eq!(c["tone"], "blue", "working is blue, as in the sidebar");
    assert_eq!(r("header-fix")["tone"], "amber");
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

/// Nothing ahead is not "merged" on its own: a branch just cut from main
/// has nothing ahead either. The plans and the sessions say which.
#[test]
fn merged_only_when_the_plan_merged_it_or_nothing_owns_it_and_no_session_works_in_it() {
    let d = data();
    let cx = Ctx { data: &d };
    let mut st = State::default();
    let out = st.act(&cx, &asked("worktrees_show", json!({}))).unwrap();
    let v = answer(&mut st, &cx, out);
    let rows: Vec<J> = v["blocks"].as_array().unwrap().iter().flat_map(|b| b["rows"].as_array().unwrap().clone()).collect();
    let r = |t: &str| rows.iter().find(|r| r["thread"] == t).unwrap_or_else(|| panic!("{t}")).clone();
    let says = |t: &str| (r(t)["merged"].as_bool().unwrap(), r(t)["fresh"].as_bool().unwrap());
    // Plan p-w1 merged it (its session still wants the person).
    assert_eq!(says("header-fix"), (true, false));
    // In the open plan p-w2, nothing ahead, one uncommitted file: no commit yet.
    assert_eq!(says("pricing-page"), (false, true));
    assert_eq!(r("pricing-page")["dirty"], 1);
    // No plan, its session at work: no commit yet.
    assert_eq!(says("promo-banner"), (false, true));
    // No plan, no session, nothing ahead: merged.
    assert_eq!(says("old-draft"), (true, false));
    // Commits of its own: neither, and a lane in the graph.
    assert_eq!(says("checkout-copy"), (false, false));
    // No branch: neither.
    assert_eq!(says("search-index"), (false, false));
    // The graph draws a lane exactly for the rows that are neither merged nor fresh and have commits.
    for row in &rows {
        if row["merged"] == true || row["fresh"] == true {
            assert!(row["lane_color"].is_null(), "{}: no lane", row["thread"]);
        }
    }
    // Same branch, the session idle and no plan: merged.
    let mut d2 = d.clone();
    for s in d2["sessions"].as_array_mut().unwrap() {
        if s["thread"] == "promo-banner" {
            s["state"] = json!("idle");
        }
    }
    let cx2 = Ctx { data: &d2 };
    let t = &fx()["daemon"]["worktrees_of"]["web-app"]["repos"][0]["threads"].as_array().unwrap().iter().find(|t| t["name"] == "promo-banner").unwrap().clone();
    assert_eq!(branch_word(&cx2, "web-app", "promo-banner", t, None), BranchWord::Merged);
    // A plan whose merge of the thread failed did not merge it.
    let mut p = d["plans"][0].clone();
    p["merge"]["results"][0]["ok"] = json!(false);
    assert!(!plan_merged(&p, "header-fix"));
    // The screen draws both chips.
    let mut data = d.clone();
    data[UI] = v.clone();
    let s = crate::screen::ScreenState::new(SCREEN, &std::env::temp_dir().join("sbxw-no-overrides"), data);
    assert!(s.error.is_none(), "{:?}", s.error);
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

/// The branch graph's layout: only unmerged branches get a lane; lanes go
/// by fork (newest first, so no link crosses a lane above it), then name;
/// each forks at its merge-base's column; the caps cut the dots (a "+N"
/// gap, an "earlier" edge), the same lanes in both variants.
#[test]
fn the_branch_graph_lays_out_unmerged_lanes_by_fork_with_caps() {
    let f = fx();
    let repo = &f["daemon"]["worktrees_of"]["web-app"]["repos"][0];
    let none = |_: &str| None;
    let g = graph::layout(repo, graph::WIDE, &none).expect("web-app has unmerged threads");
    let lanes = g["lanes"].as_array().unwrap();
    let names: Vec<&str> = lanes.iter().map(|l| l["thread"].as_str().unwrap()).collect();
    // header-fix (merged: nothing ahead) and search-index (no branch) are not drawn.
    assert_eq!(names, ["checkout-copy", "api-cache"], "newest fork first");
    assert_eq!(lanes.iter().map(|l| l["row"].as_u64().unwrap()).collect::<Vec<_>>(), [1, 2]);
    // The base: 10 commits, the earliest fork (api-cache at log[6], the 4th
    // oldest) with one column before it; checkout-copy forks at log[1].
    let base = g["base"].as_array().unwrap();
    let col_of = |sha: &J| base.iter().find(|c| &c["sha"] == sha).map(|c| c["col"].as_i64().unwrap());
    let log = repo["log"].as_array().unwrap();
    assert_eq!(base.len(), 8, "from just before the earliest fork to HEAD");
    assert!(g["earlier"] == true, "older commits than drawn: the earlier edge");
    assert_eq!(lanes[1]["fork"].as_i64(), col_of(&log[6]["sha"]));
    assert_eq!(lanes[0]["fork"].as_i64(), col_of(&log[1]["sha"]));
    assert!(base.last().unwrap()["head"] == true && base.last().unwrap()["sha"] == log[0]["sha"]);
    // Behind is readable from the picture: base dots after the fork.
    let after = |l: &J| base.iter().filter(|c| c["col"].as_i64().unwrap() > l["fork"].as_i64().unwrap()).count() as u64;
    assert_eq!(after(&lanes[0]), lanes[0]["behind"].as_u64().unwrap());
    assert_eq!(after(&lanes[1]), lanes[1]["behind"].as_u64().unwrap());
    // checkout-copy: its two commits after the fork, oldest first, at the tip its newest.
    let cc = &lanes[0];
    let cols: Vec<i64> = cc["commits"].as_array().unwrap().iter().map(|c| c["col"].as_i64().unwrap()).collect();
    assert_eq!(cols, [cc["fork"].as_i64().unwrap() + 1, cc["fork"].as_i64().unwrap() + 2]);
    assert_eq!(cc["commits"][1]["sha"], repo["threads"][0]["tip"], "the tip last");
    assert_eq!(cc["hidden"], 0);
    // api-cache: 14 ahead, the cap 10 drawn after a gap column, 4 counted.
    let ac = &lanes[1];
    assert_eq!((ac["commits"].as_array().unwrap().len(), ac["hidden"].as_u64()), (10, Some(4)));
    assert_eq!(ac["first"].as_i64().unwrap(), ac["fork"].as_i64().unwrap() + 2, "one column for the +N gap");
    assert_eq!(g["cols"].as_i64().unwrap(), ac["end"].as_i64().unwrap() + 1);
    // Narrow: fewer dots, the same lanes in the same rows.
    let n = graph::layout(repo, graph::NARROW, &none).unwrap();
    let nl = n["lanes"].as_array().unwrap();
    assert_eq!(nl.iter().map(|l| l["thread"].clone()).collect::<Vec<_>>(), lanes.iter().map(|l| l["thread"].clone()).collect::<Vec<_>>());
    assert_eq!((nl[1]["commits"].as_array().unwrap().len(), nl[1]["hidden"].as_u64()), (4, Some(10)));
    assert!(n["base"].as_array().unwrap().len() <= graph::NARROW.base);
    // Deterministic: the same answer, the same layout; a colour per name.
    assert_eq!(graph::layout(repo, graph::WIDE, &none), Some(g.clone()));
    assert_eq!(graph::lane_color("api-cache"), graph::lane_color("api-cache"));
    // A repository with only merged worktrees draws nothing.
    let mut merged = repo.clone();
    merged["threads"] = json!([repo["threads"][2]]);
    assert!(graph::layout(&merged, graph::WIDE, &none).is_none());
    // A fork the log does not hold (further back): drawn from the earlier edge.
    let mut far = repo.clone();
    far["threads"][0]["fork"] = json!("0000000000000000000000000000000000000000");
    let fg = graph::layout(&far, graph::WIDE, &none).unwrap();
    assert!(fg["lanes"].as_array().unwrap().iter().any(|l| l["thread"] == "checkout-copy" && l["fork"] == -1));
}

/// The view's data carries both variants per block; a lane's label selects
/// its row; a dot opens Commits on that commit.
#[test]
fn the_graph_selects_a_row_and_opens_a_commit() {
    let d = data();
    let cx = Ctx { data: &d };
    let mut st = State::default();
    let out = st.act(&cx, &asked("worktrees_show", json!({}))).unwrap();
    let v = answer(&mut st, &cx, out);
    let b = &v["blocks"][0];
    assert_eq!(b["graphs"].as_array().unwrap().len(), 1);
    assert!(b["graphs"][0]["wide"]["lanes"][0]["id"] == "web-app:checkout-copy");
    let docs = v["blocks"].as_array().unwrap().iter().find(|b| b["sandbox"] == "docs").unwrap();
    assert_eq!(docs["graphs"][0]["narrow"]["lanes"][0]["thread"], "release-notes");
    let out = st.act(&cx, &asked("worktrees_select", json!({ "id": "web-app:api-cache" }))).unwrap();
    let rows = ui(&out)["blocks"][0]["rows"].as_array().unwrap().clone();
    assert!(rows.iter().all(|r| r["selected"] == (r["id"] == "web-app:api-cache")));
    assert!(st.act(&cx, &asked("worktrees_select", json!({ "id": "web-app:nope" }))).is_err());
    // The screen: the lane's label and a dot fire those intents.
    let mut data = d.clone();
    data["worktrees_ui"] = v.clone();
    data["app"]["overlay"] = json!({ "w": 1200, "h": 700 });
    let mut h = crate::luau::LuauHost::new(crate::luau::Limits::default()).unwrap();
    h.load(SCREEN, include_str!("../../screens/worktrees.luau")).unwrap();
    let tree = h.render(&data).unwrap();
    fn find<'a>(n: &'a crate::luau::Node, k: &str) -> Option<&'a crate::luau::Node> {
        if crate::screen::key_of(n).as_deref() == Some(k) {
            return Some(n);
        }
        n.children.iter().find_map(|c| find(c, k))
    }
    let g = find(&tree, "worktrees-graph-web-app").expect("the graph");
    let p = g.paint.as_ref().expect("drawn with paint");
    assert!(p.strokes.len() >= 3 && p.dots.len() >= 8 + 2 + 10, "{} strokes {} dots", p.strokes.len(), p.dots.len());
    let lane = find(&tree, "worktrees-lane-web-app:api-cache").unwrap();
    let asks = h.act(lane.on.as_deref().unwrap(), &lane.arg, &J::Null).unwrap();
    assert_eq!(asks["name"], "worktrees_select");
    let tip = &v["blocks"][0]["graphs"][0]["wide"]["lanes"][0]["commits"][1];
    let dot = find(&tree, &format!("worktrees-dot-web-app-{}", tip["short"].as_str().unwrap())).unwrap();
    let asks = h.act(dot.on.as_deref().unwrap(), &dot.arg, &J::Null).unwrap();
    assert_eq!(asks["name"], "commits_show");
    assert_eq!(asks["args"], json!({ "sandbox": "web-app", "hash": tip["sha"], "branch": "sbxw/checkout-copy" }));
    crate::intent::parse(&json!({ "kind": "intent", "name": "commits_show", "args": asks["args"] })).unwrap();
    // Every point inside the graph's box, at 1200 and at a 720 window's overlay.
    for w in [1200.0, 420.0] {
        data["app"]["overlay"] = json!({ "w": w, "h": 400 });
        let tree = h.render(&data).unwrap();
        let g = find(&tree, "worktrees-graph-web-app").unwrap();
        let (Some(crate::luau::Size::Pt(gw)), Some(crate::luau::Size::Pt(gh))) = (g.style.w, g.style.h) else { panic!("sized") };
        assert!(gw <= w as f32 - 40.0 + 0.5, "{w}: the graph is {gw} wide");
        for (pts, _, _) in &g.paint.as_ref().unwrap().strokes {
            assert!(pts.iter().all(|[x, y]| *x >= 0.0 && *x <= gw && *y >= 0.0 && *y <= gh), "{w}: a stroke outside {gw}x{gh}: {pts:?}");
        }
        for ([x, y], r, _, _) in &g.paint.as_ref().unwrap().dots {
            assert!(x - r >= 0.0 && x + r <= gw && y - r >= 0.0 && y + r <= gh, "{w}: a dot outside");
        }
    }
}

/// The scope picker: All by default (the rail), one sandbox on a pick or
/// from a sandbox's link, All again; "none" and notes follow the scope.
#[test]
fn the_scope_narrows_to_one_sandbox_and_back_to_all() {
    let d = data();
    let cx = Ctx { data: &d };
    let mut st = State::default();
    let out = st.act(&cx, &asked("worktrees_show", json!({}))).unwrap();
    let v = answer(&mut st, &cx, out);
    let sbs = |v: &J| v["blocks"].as_array().unwrap().iter().map(|b| b["sandbox"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!((sbs(&v), v["sandbox"].clone()), (vec!["web-app".to_string(), "docs".into()], J::Null));
    let v = ui(&st.act(&cx, &asked("worktrees_sandbox", json!({ "sandbox": "docs" }))).unwrap());
    assert_eq!((sbs(&v), v["sandbox"].as_str()), (vec!["docs".to_string()], Some("docs")));
    assert!(v["none"].as_array().unwrap().is_empty(), "only docs' own: {v}");
    let v = ui(&st.act(&cx, &asked("worktrees_sandbox", json!({ "sandbox": "" }))).unwrap());
    assert_eq!(sbs(&v).len(), 2);
    // Pre-scoped from a link, then the rail again: every sandbox.
    let out = st.act(&cx, &asked("worktrees_show", json!({ "sandbox": "web-app" }))).unwrap();
    let v = answer(&mut st, &cx, out);
    assert_eq!(sbs(&v), ["web-app"]);
    let out = st.act(&cx, &asked("worktrees_show", json!({}))).unwrap();
    let v = answer(&mut st, &cx, out);
    assert_eq!(sbs(&v).len(), 2);
}
