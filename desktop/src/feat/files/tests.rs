use super::*;

fn fixture(name: &str) -> J {
    let raw = match name {
        "nomap" => include_str!("../../../fixtures/files/standin-nomap.json"),
        _ => include_str!("../../../fixtures/files/standin.json"),
    };
    let v: J = serde_json::from_str(raw).unwrap();
    // The app's data (what screens and the feature see) and, apart, the
    // fixture daemon's own (`__daemon`), as the core keeps them apart.
    let mut d = json!({ "sandboxes": v["daemon"]["sandboxes"], "app": { "kept": {}, "errors": {} } });
    d["__daemon"] = v["daemon"].clone();
    d
}

fn ask(name: &'static str, args: J) -> Asked {
    Asked { name, args: args.as_object().unwrap().clone() }
}

fn run(st: &mut State, d: &J, name: &'static str, args: J) -> Result<Vec<Action>, String> {
    st.act(&Ctx { data: d }, &ask(name, args))
}

fn spec(name: &str) -> &'static Spec {
    INTENTS.iter().find(|s| s.name == name).unwrap()
}

/// The core's rule (`shell::features::admit`).
fn admit(name: &str, a: &[Action]) -> Result<(), String> {
    match (&spec(name).risk, a) {
        (Risk::Direct, _) | (Risk::Approval(_), [Action::Confirm(_)] | [Action::Prepare(_)]) => Ok(()),
        _ => Err(format!("{name}: not the core's dialog")),
    }
}

/// An open panel on web-app, its lists read (as the fixture daemon answers them).
fn opened(d: &J) -> State {
    let mut st = State::default();
    let a = run(&mut st, d, "files_open", json!({ "sandbox": "web-app" })).unwrap();
    assert!(a.iter().any(|x| matches!(x, Action::OpenPanel(s) if s == "files")));
    for k in [ARTIFACTS, DIR, RECENT] {
        assert!(a.iter().any(|x| matches!(x, Action::Fetch { key, path } if key == k && path.starts_with("/api/sandboxes/web-app/"))), "{k}");
    }
    st
}

#[test]
fn sensitive_intents_are_approvals_and_none_takes_a_local_path() {
    for s in INTENTS {
        let gated = matches!(s.name, "files_save_as" | "files_open_with" | "files_delete" | "files_upload_send" | "files_upload_dropped" | "codemap_diagram");
        assert_eq!(matches!(s.risk, Risk::Approval(_)), gated, "{}", s.name);
        // No intent carries a file of this computer, nor bytes.
        assert!(s.args.iter().all(|(k, _, _)| !matches!(*k, "file" | "files" | "local" | "dest" | "data" | "bytes")), "{}", s.name);
    }
}

/// What the fixture daemon answers for a route (the stand-in's rules).
fn daemon(d: &mut J, method: &str, path: &str, body: J) -> (u16, J) {
    let mut sink = |_: &str, _: J| {};
    crate::feat::answer_from(&mut d["__daemon"], method, path, &body, &mut sink)
}

/// Runs the Fetches an action list asked for against the fixture daemon,
/// as the core would: the key set, then `on_data`.
fn pump(st: &mut State, d: &mut J, acts: Vec<Action>) -> Vec<Action> {
    let mut out = vec![];
    let mut todo = acts;
    for _ in 0..8 {
        let mut next = vec![];
        for a in todo {
            match &a {
                Action::Fetch { key, path } => {
                    let (c, v) = daemon(d, "GET", path, J::Null);
                    assert!((200..300).contains(&c), "{path}: {c} {v}");
                    let old = d[key.as_str()].clone();
                    d[key.as_str()] = v.clone();
                    next.extend(st.on_data(&Ctx { data: d }, key, &old, &v));
                }
                Action::Call(c) if c.method == crate::feat::Method::Get && c.raw.is_some() => {
                    let (code, v) = daemon(d, "GET", &c.path, J::Null);
                    let (b, ty) = crate::client::raw_of(&v);
                    next.extend(st.on_bytes(&Ctx { data: d }, c, Ok((code, b, ty))));
                }
                Action::Call(c) if c.method == crate::feat::Method::Get => {
                    let r = daemon(d, "GET", &c.path, J::Null);
                    next.extend(st.on_answer(&Ctx { data: d }, c, Ok(r)));
                }
                Action::Set { key, value } => d[key.as_str()] = value.clone(),
                _ => {}
            }
            out.push(a);
        }
        if next.is_empty() {
            break;
        }
        todo = next;
    }
    out
}

fn opened_pumped(d: &mut J) -> State {
    let mut st = State::default();
    let a = run(&mut st, d, "files_open", json!({ "sandbox": "web-app" })).unwrap();
    pump(&mut st, d, a);
    st
}

#[test]
fn the_panel_opens_on_the_deliverables_and_a_folder_opens_on_its_readme() {
    let mut d = fixture("standin");
    let _ = opened(&d);
    let st = opened_pumped(&mut d);
    let ui = &d[UI];
    assert_eq!(ui["tab"], "artifacts");
    assert_eq!(ui["rows"][0]["name"], "report.md", "newest first, as the daemon lists them");
    // The workspace listing went on to .sbxw-artifacts, as the web UI's panel.
    assert_eq!(d[DIR]["path"], "/Users/me/web/.sbxw-artifacts");
    assert_eq!(ui["places"][0]["id"], "recent");
    assert!(ui["places"].as_array().unwrap().iter().any(|p| p["label"] == "shared-lib (read only)"));
    // A folder with a README opens on it.
    let mut st = st;
    let a = run(&mut st, &d, "files_browse", json!({ "path": "/Users/me/web" })).unwrap();
    pump(&mut st, &mut d, a);
    assert_eq!(d[UI]["tab"], "files");
    assert_eq!(d[UI]["selected"]["name"], "README.md");
    assert_eq!(d[UI]["preview"]["state"], "markdown");
    assert!(d[UI]["preview"]["text"].as_str().unwrap().contains("npm start"));
    // Dotfiles hidden until asked, folders first.
    let names: Vec<&str> = d[UI]["rows"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["src", "install.sh", "package.json", "README.md"]);
    let a = run(&mut st, &d, "files_hidden", json!({ "on": true })).unwrap();
    pump(&mut st, &mut d, a);
    assert!(d[UI]["rows"].as_array().unwrap().iter().any(|r| r["name"] == ".git"));
}

#[test]
fn paths_stay_where_the_daemon_listed_them() {
    let mut d = fixture("standin");
    let mut st = opened_pumped(&mut d);
    // A row that was not listed is refused: a screen cannot make the panel read elsewhere.
    for id in ["../../etc/passwd", "/etc/passwd", "report.md/../../x"] {
        assert!(run(&mut st, &d, "files_select", json!({ "id": id })).is_err(), "{id}");
    }
    // A relative or odd path to browse is refused; `~` is the agent's home.
    assert!(run(&mut st, &d, "files_browse", json!({ "path": "../.." })).unwrap_err().contains("from /"));
    let a = run(&mut st, &d, "files_browse", json!({ "path": "~" })).unwrap();
    assert!(a.iter().any(|x| matches!(x, Action::Fetch { path, .. } if path.ends_with("/fs?path=%2Fhome%2Fagent"))), "{a:?}");
    // The daemon's own check (the stand-in has it): artifacts never leave their folder.
    for p in ["..%2F.env", "%2Fetc%2Fpasswd", "notes%2F..%2F..%2F.env", "."] {
        let (c, _) = daemon(&mut d, "GET", &format!("/api/sandboxes/web-app/artifacts/download?path={p}"), J::Null);
        assert_eq!(c, 403, "{p}");
    }
    // An artifact is read through the artifacts route, by its relative path.
    run(&mut st, &d, "files_tab", json!({ "tab": "artifacts" })).unwrap();
    let a = run(&mut st, &d, "files_select", json!({ "id": "notes/meeting.md" })).unwrap();
    assert!(a.iter().any(|x| matches!(x, Action::Call(c) if c.path == "/api/sandboxes/web-app/artifacts/download?path=notes%2Fmeeting.md")), "{a:?}");
}

#[test]
fn deleting_shows_the_exact_path_and_size_and_only_where_the_daemon_allows() {
    let mut d = fixture("standin");
    let mut st = opened_pumped(&mut d);
    let a = run(&mut st, &d, "files_browse", json!({ "path": "/Users/me/web" })).unwrap();
    pump(&mut st, &mut d, a);
    let a = run(&mut st, &d, "files_select", json!({ "id": "/Users/me/web/package.json" })).unwrap();
    pump(&mut st, &mut d, a);
    let a = run(&mut st, &d, "files_delete", json!({})).unwrap();
    admit("files_delete", &a).unwrap();
    let [Action::Confirm(c)] = a.as_slice() else { panic!("{a:?}") };
    assert_eq!(c.title, "Delete the file package.json?");
    assert!(c.danger);
    assert_eq!(c.lines[0], "In web-app: /Users/me/web/package.json");
    assert!(c.lines[1].starts_with("812 B · modified "), "{:?}", c.lines);
    assert_eq!((c.call.path.as_str(), &c.call.body), ("/api/sandboxes/web-app/fs/delete", &json!({ "path": "/Users/me/web/package.json" })));
    // The rule, as the web UI's and the daemon's.
    let l = &d[DIR];
    for (p, ok) in [
        ("/Users/me/web/src/app.ts", true),
        ("/home/agent/notes.txt", true),
        ("/tmp/build.log", true),
        ("/Users/me/web", false),
        ("/Users/me/web/.git", false),
        ("/Users/me/web/.sbxw-artifacts", false),
        ("/Users/me/web/.sbxw-artifacts/report.md", true),
        ("/Users/me/web/.sbxw-worktrees/checkout", false),
        ("/Users/me/shared-lib/x.rs", false),
        ("/etc/passwd", false),
        ("/Users/me/web/../../etc/passwd", false),
        ("relative/x", false),
        ("/tmp", false),
    ] {
        assert_eq!(view::can_delete(p, l), ok, "{p}");
    }
    // The stand-in refuses what the daemon refuses, and deletes the rest.
    let (code, v) = daemon(&mut d, "POST", "/api/sandboxes/web-app/fs/delete", json!({ "path": "/Users/me/web/.git" }));
    assert_eq!(code, 403, "{v}");
    let r = daemon(&mut d, "POST", &c_path(), json!({ "path": "/Users/me/web/package.json" }));
    let acts = st.on_answer(&Ctx { data: &d }, &c.call, Ok(r));
    assert!(acts.iter().any(|a| matches!(a, Action::Notice { text, error: false, .. } if text == "Deleted /Users/me/web/package.json")), "{acts:?}");
    pump(&mut st, &mut d, acts);
    assert!(!d[DIR]["entries"].as_array().unwrap().iter().any(|e| e["name"] == "package.json"));
}

fn c_path() -> String {
    "/api/sandboxes/web-app/fs/delete".into()
}

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sbxw-files-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn uploads_show_the_local_path_and_size_respect_the_limits_and_type_the_path() {
    let d = fixture("standin");
    let mut st = opened(&d);
    let dir = tmp("up");
    let small = dir.join("notes.txt");
    std::fs::write(&small, b"hello").unwrap();
    let empty = dir.join("empty.txt");
    std::fs::write(&empty, b"").unwrap();
    let big = dir.join("huge.iso");
    std::fs::File::create(&big).unwrap().set_len(view::UPLOAD_MAX + 1).unwrap();
    // Nothing waiting: refused before any dialog.
    assert!(run(&mut st, &d, "files_upload_send", json!({})).unwrap_err().contains("nothing to upload"));
    let a = st.on_files(&Ctx { data: &d }, Incoming::Files(vec![small.clone(), dir.clone()]));
    assert!(a.iter().any(|x| matches!(x, Action::Notice { text, error: true, .. } if text.contains("a folder"))), "{a:?}");
    assert!(a.iter().any(|x| matches!(x, Action::OpenPanel(s) if s == "files")));
    let a = run(&mut st, &d, "files_upload_send", json!({})).unwrap();
    admit("files_upload_send", &a).unwrap();
    let [Action::Confirm(c)] = a.as_slice() else { panic!() };
    assert_eq!(c.title, "Upload notes.txt into web-app?");
    assert_eq!(c.lines[0], format!("{} · 5 B", small.display()));
    assert!(c.lines.iter().any(|l| l.contains("typed into web-app's agent's prompt, not submitted")), "{:?}", c.lines);
    assert_eq!((c.call.method, c.call.path.as_str()), (crate::feat::Method::Core, "core:upload"));
    assert_eq!(c.call.body["file"], json!(small));
    assert_eq!(c.call.body["route"], "/api/sandboxes/web-app/upload?name=notes.txt");
    // The daemon's answer: the path typed into the prompt, not submitted.
    let up = c.call.clone();
    let a = st.on_answer(&Ctx { data: &d }, &up, Ok((200, json!({ "ok": true, "path": "/tmp/sbxw-uploads/1/notes.txt" }))));
    let Some(Action::Call(t)) = a.iter().find(|x| matches!(x, Action::Call(_))) else { panic!("{a:?}") };
    assert_eq!((t.path.as_str(), &t.body), ("/api/input", &json!({ "sandbox": "web-app", "data": "/tmp/sbxw-uploads/1/notes.txt " })));
    assert!(st.staged.is_empty());
    // No session took it: said, with the path to copy.
    let a = st.on_answer(&Ctx { data: &d }, t, Ok((404, J::Null)));
    assert!(a.iter().any(|x| matches!(x, Action::Notice { then: Some((n, args)), .. } if n == "copy_text" && args["text"] == "/tmp/sbxw-uploads/1/notes.txt")), "{a:?}");
    // The daemon's refusal, in its words.
    st.on_files(&Ctx { data: &d }, Incoming::Files(vec![small.clone()]));
    let a = run(&mut st, &d, "files_upload_send", json!({})).unwrap();
    let [Action::Confirm(c)] = a.as_slice() else { panic!() };
    let a = st.on_answer(&Ctx { data: &d }, &c.call, Ok((413, json!({ "error": "length limit exceeded" }))));
    assert!(a.iter().any(|x| matches!(x, Action::Notice { text, error: true, .. } if text == "notes.txt was not uploaded: length limit exceeded")), "{a:?}");
    st.staged.clear();
    // Too big, empty: said on the row, and the upload refused.
    st.on_files(&Ctx { data: &d }, Incoming::Files(vec![big, empty]));
    let ui = st.ui(&Ctx { data: &d });
    assert!(ui["staged"][0]["problem"].as_str().unwrap().contains("too large: 134217729 bytes, the daemon takes 128.0 MB at most"), "{ui}");
    assert!(ui["staged"][1]["problem"].as_str().unwrap().contains("empty"));
    assert!(run(&mut st, &d, "files_upload_send", json!({ "index": 1 })).unwrap_err().contains("huge.iso cannot go"));
    assert!(run(&mut st, &d, "files_upload_send", json!({ "index": 2 })).unwrap_err().contains("empty"));
    assert!(run(&mut st, &d, "files_upload_send", json!({ "index": 3 })).unwrap_err().contains("no file 3"));
    assert!(run(&mut st, &d, "files_upload_remove", json!({ "index": 5 })).is_err());
    run(&mut st, &d, "files_upload_remove", json!({})).unwrap();
    // A pasted picture: named by the app, sent as bytes to paste-image.
    st.on_files(&Ctx { data: &d }, Incoming::Png(vec![1; 2048]));
    let a = run(&mut st, &d, "files_upload_send", json!({})).unwrap();
    let [Action::Confirm(c)] = a.as_slice() else { panic!() };
    assert_eq!(c.title, "Upload pasted-1.png into web-app?");
    assert_eq!(c.lines[0], "a picture from the clipboard · 2.0 KB");
    assert_eq!(c.call.path, "/api/sandboxes/web-app/paste-image");
    assert_eq!(c.call.bytes.as_ref().map(|(t, b)| (t.as_str(), b.len())), Some(("image/png", 2048)));
    st.staged.clear();
    // Dropped on a stopped sandbox's pane: the dialog is asked for, and refused.
    let a = st.on_pane_drop(&Ctx { data: &d }, &PaneDrop { paths: vec![small.clone()], skip_dialog: false, sandbox: "data-pipeline".into(), thread: None, picture: None });
    assert!(matches!(a.as_slice(), [Action::Intent { name, .. }] if name == "files_upload_dropped"), "{a:?}");
    assert!(run(&mut st, &d, "files_upload_dropped", json!({})).unwrap_err().contains("not running"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn files_dropped_together_go_through_one_dialog_that_explains_it_and_offers_to_stop_asking() {
    let mut d = fixture("standin");
    let mut st = State::default();
    let dir = tmp("pane");
    let (a1, a2) = (dir.join("spec one.md"), dir.join("logo.png"));
    std::fs::write(&a1, b"# spec").unwrap();
    std::fs::write(&a2, vec![0u8; 3000]).unwrap();
    let drop = |paths: Vec<std::path::PathBuf>, skip: bool| PaneDrop { paths, skip_dialog: skip, sandbox: "api".into(), thread: Some("checkout".into()), picture: None };
    // The drop itself uploads nothing and types nothing: it asks for the dialog.
    let a = st.on_pane_drop(&Ctx { data: &d }, &drop(vec![a1.clone(), a2.clone()], false));
    assert!(matches!(a.as_slice(), [Action::Intent { name, args }] if name == "files_upload_dropped" && args == &json!({})), "{a:?}");
    assert!(st.staged.is_empty(), "never shown as waiting in the panel");
    let a = run(&mut st, &d, "files_upload_dropped", json!({})).unwrap();
    admit("files_upload_dropped", &a).unwrap();
    let [Action::Confirm(c)] = a.as_slice() else { panic!() };
    // ONE dialog for both: each file, the total, what happens, the box.
    assert_eq!(c.title, "Upload 2 files into api?");
    assert_eq!(c.lines[0], format!("{} · 6 B", a1.display()));
    assert_eq!(c.lines[1], format!("{} · 2.9 KB", a2.display()));
    assert_eq!(c.lines[2], "2 files, 2.9 KB in all.");
    assert_eq!(c.lines[3], "They are copied into api under /tmp/sbxw-uploads/, where its agent can read them.");
    assert_eq!(c.lines[4], "Their paths are then typed into api's thread checkout's prompt, not sent: say what they are for, then press Return.");
    assert_eq!(c.dont_ask.as_deref(), Some("Do not ask again for files I drop"));
    assert_eq!(c.call.path, "core:uploads", "the core streams them, on Confirm only");
    assert_eq!(c.call.body["items"][0]["route"], "/api/sandboxes/api/upload?name=spec%20one.md");
    assert_eq!(c.call.body["items"][1]["file"], json!(a2));
    // Answered: both paths typed in one go (not submitted), a notice, the drop forgotten.
    let ids: Vec<u64> = st.dropped.iter().map(|s| s.id).collect();
    let ok = |id: u64, p: &str| json!({ "tag": format!("upload-{id}"), "code": 200, "body": { "ok": true, "path": p } });
    let a = st.on_answer(&Ctx { data: &d }, &c.call, Ok((200, json!({ "results": [ok(ids[0], "/tmp/sbxw-uploads/17/spec_one.md"), ok(ids[1], "/tmp/sbxw-uploads/18/logo.png")] }))));
    assert!(a.iter().any(|x| matches!(x, Action::Call(t) if t.body == json!({ "sandbox": "api", "mode": "thread:checkout", "data": "/tmp/sbxw-uploads/17/spec_one.md /tmp/sbxw-uploads/18/logo.png " }))), "{a:?}");
    assert!(a.iter().any(|x| matches!(x, Action::Notice { text, error: false, .. } if text == "Uploaded 2 files (spec one.md, logo.png) to api")), "{a:?}");
    assert!(a.iter().any(|x| matches!(x, Action::ForgetFiles(f) if f.len() == 2)));
    assert!(st.dropped.is_empty());
    // A path that is not one the daemon makes is never typed.
    for bad in ["/tmp/sbxw-uploads/1/a b.md", "/tmp/sbxw-uploads/1/a\nrm -rf ~", "/tmp/sbxw-uploads/1/\"x", "/etc/passwd", "/tmp/sbxw-uploads/../x", "/tmp/sbxw-uploads/1/-rf"] {
        assert!(!typable(bad), "{bad}");
    }
    assert!(typable("/tmp/sbxw-pastes/paste-1731744000000.png"));
    // A failure stops the rest and says what was sent and what was not.
    let a3 = dir.join("c.txt");
    std::fs::write(&a3, b"c").unwrap();
    st.on_pane_drop(&Ctx { data: &d }, &drop(vec![a1.clone(), a2.clone(), a3.clone()], false));
    let a = run(&mut st, &d, "files_upload_dropped", json!({})).unwrap();
    let [Action::Confirm(c)] = a.as_slice() else { panic!() };
    let ids: Vec<u64> = st.dropped.iter().map(|s| s.id).collect();
    let bad = json!({ "tag": format!("upload-{}", ids[1]), "code": 502, "body": { "error": "sbx exec: sandbox not ready" } });
    let a = st.on_answer(&Ctx { data: &d }, &c.call, Ok((200, json!({ "results": [ok(ids[0], "/tmp/sbxw-uploads/1/a\nsubmit"), bad] }))));
    assert!(!a.iter().any(|x| matches!(x, Action::Call(_))), "an untypable path is never typed: {a:?}");
    assert!(a.iter().any(|x| matches!(x, Action::Notice { text, error: true, .. } if text.starts_with("logo.png was not uploaded: ") && text.ends_with(". Sent: spec one.md. Not sent: c.txt"))), "{a:?}");
    assert!(st.dropped.is_empty());
    // A stopped sandbox, an empty file: refused before any dialog, nothing sent.
    let empty = dir.join("empty.txt");
    std::fs::write(&empty, b"").unwrap();
    st.on_pane_drop(&Ctx { data: &d }, &drop(vec![a1.clone(), empty.clone()], false));
    assert!(run(&mut st, &d, "files_upload_dropped", json!({})).unwrap_err().contains("Nothing was uploaded: empty.txt: empty"));
    d["sandboxes"] = json!([]);
    st.on_pane_drop(&Ctx { data: &d }, &drop(vec![a1.clone()], true));
    assert!(st.dropped.is_empty(), "a sandbox sbxw does not know: refused at once");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn chosen_not_to_be_asked_a_plain_drop_goes_at_once_and_a_guardrail_still_asks() {
    let d = fixture("standin");
    let mut st = State::default();
    let dir = tmp("skip");
    let brief = dir.join("brief.md");
    std::fs::write(&brief, b"# Brief").unwrap();
    let drop = |paths: Vec<std::path::PathBuf>| PaneDrop { paths, skip_dialog: true, sandbox: "web-app".into(), thread: None, picture: None };
    // Plain: the batch at once, said by a notice; no dialog asked for.
    let a = st.on_pane_drop(&Ctx { data: &d }, &drop(vec![brief.clone()]));
    assert!(a.iter().any(|x| matches!(x, Action::Notice { text, .. } if text == "Uploading brief.md to web-app…")), "{a:?}");
    let Some(Action::Call(c)) = a.iter().find(|x| matches!(x, Action::Call(_))) else { panic!("{a:?}") };
    assert_eq!(c.path, "core:uploads");
    assert!(!a.iter().any(|x| matches!(x, Action::Intent { .. } | Action::Confirm(_))));
    let id = st.dropped[0].id;
    let a = st.on_answer(&Ctx { data: &d }, c, Ok((200, json!({ "results": [{ "tag": format!("upload-{id}"), "code": 200, "body": { "ok": true, "path": "/tmp/sbxw-uploads/3/brief.md" } }] }))));
    assert!(a.iter().any(|x| matches!(x, Action::Notice { text, .. } if text == "Uploaded brief.md to web-app")), "{a:?}");
    // Each guardrail: the dialog all the same, saying why, without the box.
    let env = dir.join(".env.local");
    std::fs::write(&env, b"KEY=x").unwrap();
    let ssh = dir.join(".ssh");
    std::fs::create_dir_all(&ssh).unwrap();
    let known = ssh.join("known_hosts");
    std::fs::write(&known, b"x").unwrap();
    let big = dir.join("big.bin");
    std::fs::File::create(&big).unwrap().set_len(drops::ASK_ABOVE_BYTES + 1).unwrap();
    let many: Vec<_> = (0..11).map(|i| { let p = dir.join(format!("f{i}.txt")); std::fs::write(&p, b"x").unwrap(); p }).collect();
    for (paths, why) in [
        (vec![brief.clone(), env.clone()], ".env.local: its name looks like a secret"),
        (vec![known.clone()], "known_hosts: it is in a .ssh folder"),
        (vec![big.clone()], "in all (more than 25.0 MB)"),
        (many.clone(), "11 files dropped at once (more than 10)"),
    ] {
        let a = st.on_pane_drop(&Ctx { data: &d }, &drop(paths));
        assert!(a.iter().any(|x| matches!(x, Action::Intent { name, .. } if name == "files_upload_dropped")), "{why}: {a:?}");
        assert!(!a.iter().any(|x| matches!(x, Action::Call(_))), "{why}");
        let a = run(&mut st, &d, "files_upload_dropped", json!({})).unwrap();
        let [Action::Confirm(c)] = a.as_slice() else { panic!() };
        assert!(c.lines.iter().any(|l| l.starts_with("Asked even though you chose not to be asked, because: ") && l.contains(why)), "{why}: {:?}", c.lines);
        assert!(c.dont_ask.is_none(), "already chosen");
        st.on_dismiss(&Ctx { data: &d }, &c.call.tag);
        assert!(st.dropped.is_empty());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_guardrails_read_one_list_and_fail_closed() {
    for n in [".env", ".ENV.production", "server.pem", "tls.key", "id_rsa", "id_rsa.pub", "id_ed25519", "cert.p12", "cert.pfx", "credentials", "credentials.json", ".npmrc", ".netrc", "vault.kdbx"] {
        assert!(drops::secret_name(n), "{n}");
    }
    for n in ["brief.md", "environment.md", "keys.txt", "key.md", "npmrc.md", "my.env.md"] {
        assert!(!drops::secret_name(n), "{n}");
    }
    let dir = tmp("guard");
    let gone = dir.join("gone.txt");
    let why = drops::guardrails(std::slice::from_ref(&gone), 1);
    assert!(why.iter().any(|w| w.starts_with("gone.txt: could not be checked")), "fail closed: {why:?}");
    for d in [".aws", ".gnupg"] {
        std::fs::create_dir_all(dir.join(d)).unwrap();
        let p = dir.join(d).join("config");
        std::fs::write(&p, b"x").unwrap();
        assert!(drops::guardrails(&[p], 1).iter().any(|w| w.contains(&format!("in a {d} folder"))), "{d}");
    }
    let plain = dir.join("notes.md");
    std::fs::write(&plain, b"x").unwrap();
    assert!(drops::guardrails(&[plain], 1).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ten_files_at_a_time() {
    let d = fixture("standin");
    let mut st = opened(&d);
    let dir = tmp("ten");
    // Ten at a time at most.
    let many: Vec<_> = (0..12).map(|i| { let p = dir.join(format!("f{i}.txt")); std::fs::write(&p, b"x").unwrap(); p }).collect();
    let a = st.on_files(&Ctx { data: &d }, Incoming::Files(many));
    assert_eq!(st.staged.len(), view::STAGED_MAX);
    assert!(a.iter().any(|x| matches!(x, Action::Notice { text, .. } if text.contains("10 files at a time"))));
    let _ = std::fs::remove_dir_all(&dir);
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 30, 30, 255]));
    let mut out = std::io::Cursor::new(vec![]);
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn with_selection(name: &str, size: u64) -> State {
    let mut st = State { sandbox: Some("web-app".into()), ..State::default() };
    st.seq = 7;
    st.sel = Some(Sel { src: Src::Artifact, path: name.into(), name: name.into(), size, modified: 0, kind: "file".into() });
    st
}

/// A picture previewed, then another tab, place or folder: the picture is
/// forgotten (its decoded pixels stayed registered otherwise: MEMORY.md).
#[test]
fn leaving_a_picture_forgets_it() {
    let d = fixture("standin");
    for (name, args) in [("files_tab", json!({ "tab": "recent" })), ("files_place", json!({ "id": "artifacts" })), ("files_back", json!({}))] {
        let mut st = with_selection("wireframe.png", 0);
        st.history = vec!["/Users/me/web".into()];
        st.got_preview(7, &png(40, 30));
        let a = run(&mut st, &d, name, args).unwrap();
        assert!(a.iter().any(|x| matches!(x, Action::Image { key, bytes } if key == "files-preview-7" && bytes.is_empty())), "{name}: {a:?}");
        assert!(st.preview.is_null());
    }
}

#[test]
fn pictures_go_through_the_cores_decoder_and_a_bad_one_is_said_not_drawn() {
    // A good one: decoded, scaled, re-encoded, registered under a key.
    let mut st = with_selection("wireframe.png", 0);
    let a = st.got_preview(7, &png(40, 30));
    assert!(matches!(a.as_slice(), [Action::Image { key, bytes }] if key == "files-preview-7" && bytes.starts_with(b"\x89PNG")), "{a:?}");
    assert_eq!((st.preview["state"].as_str(), st.preview["w"].as_u64()), (Some("image"), Some(40)));
    // A late answer for an older selection is dropped.
    assert!(st.got_preview(6, &png(4, 4)).is_empty());
    // Malformed, truncated, not a picture, a bomb's header: a sentence, no picture.
    let mut bomb = png(1, 1);
    // IHDR says 100000 x 100000.
    bomb[16..20].copy_from_slice(&100_000u32.to_be_bytes());
    bomb[20..24].copy_from_slice(&100_000u32.to_be_bytes());
    for bad in [b"\x89PNG\r\n\x1a\nGARBAGE".to_vec(), png(40, 30)[..60].to_vec(), b"<svg onload=alert(1)>".to_vec(), bomb] {
        let mut st = with_selection("wireframe.png", 0);
        assert!(st.got_preview(7, &bad).is_empty());
        assert_eq!(st.preview["state"], "note");
        assert!(st.preview["text"].as_str().unwrap().starts_with("This picture cannot be shown"), "{}", st.preview);
    }
    // Over the panel's own cap: not even read.
    let p = view::preview_without_reading("big.png", view::IMAGE_MAX + 1).unwrap();
    assert!(p["text"].as_str().unwrap().starts_with("Too large to preview"));
    let mut st = with_selection("big.png", view::IMAGE_MAX + 1);
    assert!(st.got_preview(7, &vec![0u8; (view::IMAGE_MAX + 1) as usize]).is_empty());
    assert_eq!(st.preview["state"], "note");
}

#[test]
fn text_previews_are_capped_binary_is_said_and_html_is_never_drawn() {
    let p = view::text_preview("a.bin", b"ELF\0\0\x01", 6);
    assert_eq!(p["text"], "Binary file: no preview.");
    let p = view::text_preview("log.txt", b"line\n", 5_000_000);
    assert_eq!(p["state"], "text");
    assert_eq!(p["note"], "First 5 B of 4.8 MB: download it for the rest.");
    let long: String = (0..40_000).map(|i| format!("{i}\n")).collect();
    let p = view::text_preview("long.log", long.as_bytes(), long.len() as u64);
    assert!(p["text"].as_str().unwrap().len() <= view::TEXT_SHOWN);
    assert!(p["text"].as_str().unwrap().ends_with(|c: char| c.is_ascii_digit()), "cut at a line");
    assert!(p["note"].as_str().unwrap().starts_with("First 128.0 KB of"));
    let p = view::text_preview("page.html", b"<script>alert(1)</script>", 25);
    assert_eq!(p["state"], "html", "shown as its source, never drawn");
    assert_eq!(view::text_preview("r.md", b"# hi", 4)["state"], "markdown");
    assert!(view::preview_without_reading("deck.pptx", 10).unwrap()["text"].as_str().unwrap().contains("no preview here"));
    // Invalid UTF-8 is drawn with replacement marks, not refused.
    assert_eq!(view::text_preview("x.txt", &[0x66, 0xff, 0x6f], 3)["text"], "f\u{fffd}o");
}

#[test]
fn downloads_and_opening_go_through_the_core_and_its_verdict_makes_the_dialog() {
    let mut d = fixture("standin");
    let mut st = opened_pumped(&mut d);
    let a = run(&mut st, &d, "files_select", json!({ "id": "report.md" })).unwrap();
    pump(&mut st, &mut d, a);
    // Downloads: direct, written by the core under the name it cleans.
    let a = run(&mut st, &d, "files_download", json!({})).unwrap();
    let Action::Download(dl) = &a[0] else { panic!("{a:?}") };
    assert!(dl.direct());
    assert_eq!((dl.get.as_str(), dl.name.as_str(), dl.to, dl.after), ("/api/sandboxes/web-app/artifacts/download?path=report.md", "report.md", Dest::Downloads, After::Nothing));
    let a = run(&mut st, &d, "files_reveal", json!({})).unwrap();
    assert!(matches!(&a[0], Action::Download(x) if x.after == After::Reveal && x.direct()));
    let a = st.on_answer(&Ctx { data: &d }, &dl.call("save report.md"), Ok((200, json!({ "saved": "/Users/me/Downloads/report (1).md", "name": "report (1).md", "bytes": 10, "verdict": "plain" }))));
    assert!(a.iter().any(|x| matches!(x, Action::Notice { text, .. } if text == "Saved report (1).md: /Users/me/Downloads/report (1).md")));
    let a = run(&mut st, &d, "files_show_saved", json!({})).unwrap();
    assert!(matches!(&a[0], Action::Reveal(p) if p.to_str() == Some("/Users/me/Downloads/report (1).md")));
    // A cancelled save dialog says nothing.
    let a = st.on_answer(&Ctx { data: &d }, &dl.call("save report.md"), Ok((499, json!({ "cancelled": true }))));
    assert!(!a.iter().any(|x| matches!(x, Action::Notice { .. })));
    // Elsewhere: the save dialog, behind the core's Confirm.
    let a = run(&mut st, &d, "files_save_as", json!({})).unwrap();
    admit("files_save_as", &a).unwrap();
    let [Action::Confirm(c)] = a.as_slice() else { panic!() };
    assert_eq!((c.call.path.as_str(), c.call.body["to"].as_str()), ("core:download", Some("ask")));
    // Open: the core downloads and judges it first; the dialog is built from its answer.
    let a = run(&mut st, &d, "files_open_with", json!({})).unwrap();
    admit("files_open_with", &a).unwrap();
    let [Action::Prepare(p)] = a.as_slice() else { panic!("{a:?}") };
    assert_eq!((p.call.path.as_str(), p.call.body["after"].as_str(), p.call.body["to"].as_str()), ("core:download", Some("nothing"), Some("downloads")));
    let cx = Ctx { data: &d };
    let plain = (p.build)(&cx, &p.asked, &json!({ "saved": "/D/report.md", "name": "report.md", "bytes": 300, "verdict": "plain", "kind": "Markdown document", "reason": "" })).unwrap();
    assert_eq!(plain.call.path, "core:open");
    assert_eq!(plain.call.body, json!({ "path": "/D/report.md" }));
    assert!(plain.ack.is_none());
    assert_eq!(plain.lines, ["From web-app: /Users/me/web/.sbxw-artifacts/report.md", "Saved in Downloads: /D/report.md", "300 B · Markdown document"]);
    let warn = (p.build)(&cx, &p.asked, &json!({ "saved": "/D/page.html", "name": "page.html", "bytes": 3, "verdict": "warn", "kind": "HTML page", "reason": "Your browser runs its scripts." })).unwrap();
    assert!(warn.ack.is_some());
    assert!(warn.lines.contains(&"Your browser runs its scripts.".to_string()));
    let refused = (p.build)(&cx, &p.asked, &json!({ "saved": "/D/report.pdf", "name": "report.pdf", "bytes": 3, "verdict": "refuse", "kind": "a program", "reason": "Its first bytes are a Windows program." })).unwrap_err();
    assert!(refused.contains("never opens it") && refused.contains("/D/report.pdf"), "{refused}");
    assert!((p.build)(&cx, &p.asked, &json!({ "saved": "/D/x", "name": "x" })).is_err(), "no verdict: not opened");
    // Copy path: the path as the agent sees it, through the core's copy_text.
    let a = run(&mut st, &d, "files_copy_path", json!({})).unwrap();
    assert!(matches!(&a[0], Action::Intent { name, args } if name == "copy_text" && args["text"] == "/Users/me/web/.sbxw-artifacts/report.md"));
    // No selection: nothing to save.
    let mut st = opened(&d);
    assert!(run(&mut st, &d, "files_open_with", json!({})).is_err());
}

#[test]
fn a_picture_artifact_is_read_raw_and_shown_through_the_core() {
    let mut d = fixture("standin");
    let mut st = opened_pumped(&mut d);
    let a = run(&mut st, &d, "files_select", json!({ "id": "wireframe.png" })).unwrap();
    let out = pump(&mut st, &mut d, a);
    assert!(out.iter().any(|x| matches!(x, Action::Image { key, bytes } if key.starts_with("files-preview-") && !bytes.is_empty())), "{out:?}");
    assert_eq!(d[UI]["preview"]["state"], "image");
    assert_eq!(d[UI]["preview"]["w"], 320);
}

#[test]
fn recent_rows_say_where_and_show_in_folder_lists_it() {
    let mut d = fixture("standin");
    let mut st = opened_pumped(&mut d);
    let a = run(&mut st, &d, "files_tab", json!({ "tab": "recent" })).unwrap();
    pump(&mut st, &mut d, a);
    let rows = d[UI]["rows"].as_array().unwrap().clone();
    let r = rows.iter().find(|r| r["name"] == "app.ts").unwrap();
    assert_eq!(r["dir"], "workspace/src");
    assert_eq!(rows.iter().find(|r| r["name"] == "checkout.md").unwrap()["dir"], "~/.claude/plans");
    let a = run(&mut st, &d, "files_show_in_folder", json!({ "id": "/Users/me/web/src/app.ts" })).unwrap();
    pump(&mut st, &mut d, a);
    assert_eq!(d[UI]["tab"], "files");
    assert_eq!(d[UI]["path"], "/Users/me/web/src");
    assert_eq!(d[UI]["selected"]["name"], "app.ts");
    assert!(run(&mut st, &d, "files_show_in_folder", json!({ "id": "/etc/shadow" })).is_err());
}

// ---------------------------------------------------------------------------
// The code map

fn docs(d: &J) -> std::collections::BTreeMap<String, Doc> {
    let c = &d["__daemon"]["files_content"];
    ["codemap", "cli", "net"].iter().map(|k| {
        let p = format!("codemap/{k}.md");
        (k.to_string(), codemap::parse_doc(k, &p, c[format!("/Users/me/web/.sbxw-artifacts/{p}")].as_str().unwrap()))
    }).collect()
}

#[test]
fn wiki_links_resolve_as_the_web_uis_and_code_spans_and_fences_hold_none() {
    let d = fixture("standin");
    let m = docs(&d);
    let net = &m["net"];
    // `[[not-a-link]]` in a code span and `[[inside-fence]]` in a fence are not links.
    let targets: Vec<&str> = net.links.iter().map(|l| l.target.as_str()).collect();
    assert_eq!(targets, ["src/net/proxy.ts", "codemap"]);
    assert!(net.links[0].code);
    assert_eq!(net.sections.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["net", "net-proxy", "net-retries"]);
    // Full key, unique stem, case-insensitive heading, implicit H1.
    assert!(matches!(codemap::resolve("net#proxy", &m), codemap::Resolved::Doc(d, Some(s)) if d.key == "net" && s.id == "net-proxy"));
    assert!(matches!(codemap::resolve("net#Net#Proxy", &m), codemap::Resolved::Doc(_, Some(_))));
    assert!(matches!(codemap::resolve("./cli.md", &m), codemap::Resolved::Doc(d, None) if d.key == "cli"));
    assert!(matches!(codemap::resolve("billing", &m), codemap::Resolved::Broken(_)));
    assert!(matches!(codemap::resolve("net#nowhere", &m), codemap::Resolved::Broken(w) if w == "no such section"));
    assert_eq!(codemap::broken_count(&m), 1);
    let back = codemap::backlinks(&m);
    assert_eq!(back["net"].len(), 3, "{:?}", back["net"]);
    // Rewritten for the markdown element: panel links, the broken one marked, code as code.
    let md = codemap::rewrite(&m["codemap"].text, &m);
    assert!(md.contains("[cli](https://sbxw-codemap.invalid/cli)"), "{md}");
    assert!(md.contains("[net#Proxy](https://sbxw-codemap.invalid/net#net-proxy)"), "{md}");
    assert!(md.contains("[the network layer](https://sbxw-codemap.invalid/net)"));
    assert!(md.contains("billing *(broken link: no such file)*"));
    assert!(md.contains("`src/main.ts`"));
    let md = codemap::rewrite(&net.text, &m);
    assert!(md.contains("`[[not-a-link]]`") && md.contains("// [[inside-fence]] is not a link"), "{md}");
    // Round trip of the panel's own links, odd names included.
    for (doc, anchor) in [("areas/net ops", Some("a--b")), ("cli", None)] {
        assert_eq!(codemap::parse_panel_url(&codemap::panel_url(doc, anchor)), Some((doc.to_string(), anchor.map(str::to_string))));
    }
    assert_eq!(codemap::parse_panel_url("https://example.org/cli"), None);
    assert_eq!(codemap::section_text(net, "net-proxy").unwrap().lines().next(), Some("## Proxy"));
    assert!(!codemap::section_text(net, "net-proxy").unwrap().contains("Retries"));
}

#[test]
fn the_map_panel_reads_every_file_follows_links_inside_and_opens_others_in_the_browser() {
    let mut d = fixture("standin");
    let mut st = State::default();
    let a = run(&mut st, &d, "codemap_open", json!({ "sandbox": "web-app" })).unwrap();
    assert!(a.iter().any(|x| matches!(x, Action::OpenPanel(s) if s == "codemap")));
    pump(&mut st, &mut d, a);
    let ui = &d[MAP_UI];
    assert_eq!(ui["loading"], false);
    assert_eq!(ui["map"]["count"], 3);
    assert_eq!(ui["map"]["doc"]["key"], "codemap", "the index first");
    assert_eq!(ui["maps"].as_array().unwrap().len(), 2, "the map and its lens");
    assert_eq!(ui["diagram"]["path"], "codemap/diagram/architecture.html");
    // A wiki link pressed: followed in the panel, to its section.
    let a = run(&mut st, &d, "codemap_link", json!({ "url": "https://sbxw-codemap.invalid/net#net-proxy" })).unwrap();
    pump(&mut st, &mut d, a);
    assert_eq!(d[MAP_UI]["map"]["doc"]["section"]["title"], "Net › Proxy");
    assert!(d[MAP_UI]["map"]["doc"]["markdown"].as_str().unwrap().starts_with("## Proxy"));
    // Any other link: the browser.
    let a = run(&mut st, &d, "codemap_link", json!({ "url": "https://example.org/web-app" })).unwrap();
    assert!(matches!(&a[0], Action::OpenLink(u) if u == "https://example.org/web-app"));
    // A file or a section the map does not have: refused.
    assert!(run(&mut st, &d, "codemap_link", json!({ "url": "https://sbxw-codemap.invalid/billing" })).is_err());
    assert!(run(&mut st, &d, "codemap_doc", json!({ "doc": "cli", "anchor": "nope" })).is_err());
    // The lens.
    let a = run(&mut st, &d, "codemap_open", json!({ "sandbox": "web-app", "map": "codemap-lenses/product-owner" })).unwrap();
    pump(&mut st, &mut d, a);
    assert_eq!(d[MAP_UI]["map"]["doc"]["key"], "product-owner");
    // The diagram: an approval that says it is a page an agent wrote.
    let a = run(&mut st, &d, "codemap_open", json!({ "sandbox": "web-app" })).unwrap();
    pump(&mut st, &mut d, a);
    let a = run(&mut st, &d, "codemap_diagram", json!({})).unwrap();
    admit("codemap_diagram", &a).unwrap();
    let [Action::Prepare(p)] = a.as_slice() else { panic!() };
    assert_eq!(p.call.body["get"], "/api/sandboxes/web-app/artifacts/download?path=codemap%2Fdiagram%2Farchitecture.html");
    let c = (p.build)(&Ctx { data: &d }, &p.asked, &json!({ "saved": "/D/architecture.html", "name": "architecture.html", "bytes": 120, "verdict": "warn", "kind": "HTML page", "reason": "A page an agent wrote: your browser runs its scripts." })).unwrap();
    assert_eq!(c.ack.as_deref(), Some("I understand my browser runs this page's scripts"));
    assert_eq!(c.lines[0], "From web-app: .sbxw-artifacts/codemap/diagram/architecture.html");
}

#[test]
fn generating_a_map_and_a_lens_follow_the_daemons_rules() {
    let mut d = fixture("nomap");
    let mut st = State::default();
    let a = run(&mut st, &d, "codemap_open", json!({ "sandbox": "web-app" })).unwrap();
    pump(&mut st, &mut d, a);
    assert_eq!(d[MAP_UI]["map"]["count"], 0);
    let a = run(&mut st, &d, "codemap_generate", json!({ "sandbox": "web-app" })).unwrap();
    let Action::Call(c) = &a[0] else { panic!() };
    assert_eq!(c.path, "/api/sandboxes/web-app/codemap");
    let r = daemon(&mut d, "POST", &c.path, json!({}));
    assert_eq!(r.0, 200);
    let n = st.on_answer(&Ctx { data: &d }, c, Ok(r));
    assert!(n.iter().any(|x| matches!(x, Action::Notice { text, .. } if text.contains("is writing its code map"))));
    // One at a time; a stopped sandbox is refused, in the daemon's words.
    let r = daemon(&mut d, "POST", "/api/sandboxes/web-app/codemap", json!({}));
    assert_eq!(r.0, 409);
    let n = st.on_answer(&Ctx { data: &d }, c, Ok(r));
    assert!(n.iter().any(|x| matches!(x, Action::Notice { text, error: true, .. } if text.contains("already writing"))), "{n:?}");
    assert_eq!(daemon(&mut d, "POST", "/api/sandboxes/data-pipeline/codemap", json!({})).0, 409);
    // The badge while it writes (the core reads the runs again after the call).
    d[RUNS] = daemon(&mut d, "GET", "/api/codemap", J::Null).1;
    let ex = extras(&Ctx { data: &d });
    assert!(ex.iter().any(|e| e.id == "codemap" && e.place == Place::Sandbox("web-app".into())));
    assert!(ex.iter().any(|e| e.id == "project" && e.place == Place::Session { sandbox: "web-app".into(), thread: None }));
    // Done: a notice that opens it, the panel reads again.
    let n = st.on_event(&Ctx { data: &d }, "codemap", &json!({ "sandbox": "web-app", "kind": "map", "phase": "done" }));
    assert!(n.iter().any(|x| matches!(x, Action::Notice { then: Some((i, _)), .. } if i == "codemap_open")));
    // A lens: a reader, 400 characters at most.
    assert!(run(&mut st, &d, "codemap_lens", json!({ "sandbox": "web-app" })).unwrap_err().contains("who the lens is for"));
    run(&mut st, &d, "codemap_lens_set", json!({ "text": "x".repeat(401) })).unwrap();
    assert!(run(&mut st, &d, "codemap_lens", json!({ "sandbox": "web-app" })).unwrap_err().contains("400 at most"));
    run(&mut st, &d, "codemap_lens_set", json!({ "text": "a product owner" })).unwrap();
    let a = run(&mut st, &d, "codemap_lens", json!({ "sandbox": "web-app" })).unwrap();
    assert!(matches!(&a[0], Action::Call(c) if c.body == json!({ "objective": "a product owner" })));
}

#[test]
fn the_screens_render_against_every_fixture() {
    let dir = std::env::temp_dir().join("sbxw-no-overrides");
    let fx = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/files");
    let mut n = 0;
    for f in std::fs::read_dir(&fx).unwrap() {
        let f = f.unwrap().path();
        let x = crate::fixture::Fixture::load(&f).unwrap();
        for screen in [SCREEN, MAP_SCREEN] {
            let s = crate::screen::ScreenState::new(screen, &dir, x.screen_data(&crate::fixture::AppView::default()));
            assert!(s.error.is_none(), "{screen} with {}: {:?}", f.display(), s.error);
            n += 1;
        }
    }
    assert!(n >= 8, "{n}");
}

/// Writes the state fixtures (`files-*.json`, `codemap-*.json`) from the
/// stand-in's data, as the panel's own code builds them:
/// `cargo test write_files_fixtures -- --ignored`.
#[test]
#[ignore]
fn write_files_fixtures() {
    let save = |name: &str, d: &J, panels: &[&str], about: &str| {
        let mut daemon = serde_json::Map::new();
        for k in ["sandboxes", ARTIFACTS, DIR, RECENT, RUNS, UI, MAP_UI] {
            if !d[k].is_null() {
                daemon.insert(k.into(), d[k].clone());
            }
        }
        let f = json!({ "about": about, "daemon": daemon, "app": { "open": ["web-app"], "panels": panels } });
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/files").join(format!("{name}.json"));
        std::fs::write(p, serde_json::to_string_pretty(&f).unwrap() + "\n").unwrap();
    };
    let step = |st: &mut State, d: &mut J, name: &'static str, args: J| {
        let a = run(st, d, name, args).unwrap();
        pump(st, d, a);
    };
    let mut d = fixture("standin");
    let mut st = opened_pumped(&mut d);
    save("files-artifacts", &d, &["files"], "Artifacts tab: web-app's deliverables, newest first.");
    step(&mut st, &mut d, "files_select", json!({ "id": "report.md" }));
    save("files-preview-markdown", &d, &["files"], "An artifact previewed through the markdown element.");
    step(&mut st, &mut d, "files_select", json!({ "id": "summary.html" }));
    save("files-preview-html", &d, &["files"], "An HTML page an agent wrote: shown as its source, never drawn.");
    step(&mut st, &mut d, "files_select", json!({ "id": "wireframe.png" }));
    save("files-preview-picture", &d, &["files"], "A picture, read raw and decoded by the core (alone, a fixture has no picture registered: picture not loaded).");
    step(&mut st, &mut d, "files_browse", json!({ "path": "/Users/me/web" }));
    save("files-browse", &d, &["files"], "Files tab: the workspace, folders first, its README previewed, the places.");
    step(&mut st, &mut d, "files_select", json!({ "id": "/Users/me/web/install.sh" }));
    save("files-preview-script", &d, &["files"], "A script selected: its text previewed; Open goes through the core's dialog, which refuses scripts.");
    step(&mut st, &mut d, "files_tab", json!({ "tab": "recent" }));
    save("files-recent", &d, &["files"], "Recent: what was written lately anywhere, grouped by age, with Show in folder.");
    step(&mut st, &mut d, "files_select", json!({ "id": "/tmp/build.log" }));
    save("files-preview-log", &d, &["files"], "A log in /tmp previewed as text.");
    let dir = tmp("fx");
    let a = dir.join("brief.pdf");
    std::fs::write(&a, vec![1u8; 30_000]).unwrap();
    let b = dir.join("dump.iso");
    std::fs::File::create(&b).unwrap().set_len(view::UPLOAD_MAX + 5).unwrap();
    let acts = st.on_files(&Ctx { data: &d }, Incoming::Files(vec![a, b]));
    pump(&mut st, &mut d, acts);
    let mut ui = d[UI].clone();
    for s in ui["staged"].as_array_mut().unwrap() {
        s["from"] = J::String(format!("/Users/me/Desktop/{}", s["name"].as_str().unwrap()));
    }
    d[UI] = ui;
    save("files-upload", &d, &["files"], "Two files dropped on the panel: one too large for the daemon, said on its row; Upload stays off.");
    let _ = std::fs::remove_dir_all(&dir);
    let mut e = d.clone();
    e[UI]["errors"]["recent"] = "web-app is not reachable: sbx exec timed out".into();
    e[UI]["rows"] = json!([]);
    save("files-error", &e, &["files"], "Recent could not be read: the daemon's words in red.");
    let mut st = State::default();
    let a = run(&mut st, &d, "codemap_open", json!({ "sandbox": "web-app" })).unwrap();
    pump(&mut st, &mut d, a);
    save("codemap-index", &d, &["codemap"], "The code map's index: wiki links, a broken one, backlinks, outline, the source it names.");
    step(&mut st, &mut d, "codemap_link", json!({ "url": "https://sbxw-codemap.invalid/net#net-proxy" }));
    save("codemap-section", &d, &["codemap"], "A wiki link followed to its section.");
    step(&mut st, &mut d, "codemap_filter", json!({ "text": "retry" }));
    save("codemap-search", &d, &["codemap"], "Searching the map.");
    let mut d = fixture("nomap");
    let mut st = State::default();
    let a = run(&mut st, &d, "codemap_open", json!({ "sandbox": "web-app" })).unwrap();
    pump(&mut st, &mut d, a);
    save("codemap-empty", &d, &["codemap"], "No map yet: Generate code map.");
    d[RUNS] = json!([{ "sandbox": "web-app", "kind": "map", "phase": "writing", "started": 1 }]);
    d["__daemon"][RUNS] = d[RUNS].clone();
    let a = run(&mut st, &d, "codemap_filter", json!({ "text": "" })).unwrap();
    pump(&mut st, &mut d, a);
    save("codemap-writing", &d, &["codemap"], "A map being written in a background session.");
}

#[test]
fn a_cancelled_pane_drop_leaves_no_file_waiting_nor_uploadable() {
    let d = fixture("standin");
    let mut st = State::default();
    let dir = tmp("cancel");
    let paths: Vec<_> = ["a.txt", "b.txt", "c.txt"].iter().map(|n| { let p = dir.join(n); std::fs::write(&p, b"x").unwrap(); p }).collect();
    // As the core does on a drop: handed in, then the feature.
    crate::files::hand_in(&paths);
    st.on_pane_drop(&Ctx { data: &d }, &PaneDrop { paths: paths.clone(), skip_dialog: false, sandbox: "web-app".into(), thread: None, picture: None });
    assert!(paths.iter().all(|p| crate::files::was_handed(p)));
    let a = run(&mut st, &d, "files_upload_dropped", json!({})).unwrap();
    let [Action::Confirm(c)] = a.as_slice() else { panic!() };
    // Cancel on the first: the queue empties, the core forgets all three.
    let a = st.on_dismiss(&Ctx { data: &d }, &c.call.tag);
    assert!(st.dropped.is_empty());
    let Some(Action::ForgetFiles(f)) = a.iter().find(|x| matches!(x, Action::ForgetFiles(_))) else { panic!("{a:?}") };
    assert_eq!(f, &paths);
    crate::files::forget(f);
    assert!(paths.iter().all(|p| !crate::files::was_handed(p)));
    assert!(run(&mut st, &d, "files_upload_dropped", json!({})).is_err(), "nothing left to ask");
    // A tag that is not an upload's: nothing.
    assert!(st.on_dismiss(&Ctx { data: &d }, "open-read").iter().all(|x| !matches!(x, Action::ForgetFiles(_))));
    // The panel's waiting files: forgotten when removed or cleared.
    st.sandbox = Some("web-app".into());
    st.on_files(&Ctx { data: &d }, Incoming::Files(paths.clone()));
    let a = run(&mut st, &d, "files_upload_remove", json!({ "index": 1 })).unwrap();
    assert!(a.iter().any(|x| matches!(x, Action::ForgetFiles(f) if f == &paths[..1])));
    let a = run(&mut st, &d, "files_upload_remove", json!({})).unwrap();
    assert!(a.iter().any(|x| matches!(x, Action::ForgetFiles(f) if f == &paths[1..])));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_map_opens_from_the_project_panel_without_a_new_answer() {
    // The Project panel already read the listing: the same answer again
    // would not come back as new data, so the map is read from what is there.
    let mut d = fixture("standin");
    let mut st = opened_pumped(&mut d);
    let a = run(&mut st, &d, "codemap_open", json!({ "sandbox": "web-app" })).unwrap();
    assert!(!a.iter().any(|x| matches!(x, Action::Fetch { .. })), "{a:?}");
    pump(&mut st, &mut d, a);
    assert_eq!(d[MAP_UI]["map"]["count"], 3);
    assert_eq!(d[MAP_UI]["loading"], false);
}

/// A sandbox with thousands of deliverables (a desktop thread's shots and
/// logs): the panel draws a page and folded folders, never past the core's
/// element cap, and the filter still finds a file beyond the first page.
#[test]
fn thousands_of_files_render_under_the_cap_and_the_filter_reaches_them_all() {
    let mut d = fixture("standin");
    let mut st = opened_pumped(&mut d);
    // 3,000 files: 2,700 in 30 nested folders, 300 at the top.
    let mut entries = vec![];
    for i in 0..3000u64 {
        let name = format!("shot-{i:04}.png");
        let path = if i < 2700 { format!("desktop/run-{:02}/{}/{name}", i % 30, if i % 2 == 0 { "solo" } else { "window" }) } else { name.clone() };
        entries.push(json!({ "path": path, "name": name, "size": 1000 + i, "modified": 1794199000 - i }));
    }
    d[ARTIFACTS]["entries"] = J::Array(entries);
    let dir = std::env::temp_dir().join("sbxw-no-overrides");
    let render = |st: &State, d: &J| {
        let ui = st.ui(&Ctx { data: d });
        let mut data = d.clone();
        data[UI] = ui.clone();
        let s = crate::screen::ScreenState::new(SCREEN, &dir, crate::fixture::screen_data(&data, &crate::fixture::AppView::default()));
        assert!(s.error.is_none(), "{:?} with {} rows", s.error, ui["rows"].as_array().map_or(0, |r| r.len()));
        ui
    };
    // Folded: the folder first (it holds the newest file), then the top files.
    let ui = render(&st, &d);
    assert_eq!(ui["rows"][0]["kind"], "folder");
    assert_eq!((ui["rows"][0]["name"].as_str(), ui["rows"][0]["count"].as_u64()), (Some("desktop"), Some(2700)));
    assert_eq!(ui["rows"].as_array().unwrap().len(), PAGE, "1 folder and 300 files: the first 200 rows");
    assert_eq!(ui["count"]["label"], "3,000 files, showing 200");
    assert_eq!(ui["loading"]["artifacts"], false);
    // Show more grows by a page.
    run(&mut st, &d, "files_more", json!({})).unwrap();
    let ui = render(&st, &d);
    assert_eq!(ui["rows"].as_array().unwrap().len(), 301);
    assert_eq!(ui["count"]["more"], 0);
    // Opening the folder: its 30 run folders, folded.
    run(&mut st, &d, "files_fold", json!({ "id": "desktop" })).unwrap();
    let ui = render(&st, &d);
    let rows = ui["rows"].as_array().unwrap();
    assert_eq!(rows.iter().filter(|r| r["depth"] == 1 && r["kind"] == "folder").count(), 30);
    assert!(run(&mut st, &d, "files_fold", json!({ "id": "nowhere" })).is_err(), "only a listed folder");
    // Every folder open at once: still one page, and it renders.
    for i in 0..30 {
        for sub in ["solo", "window"] {
            st.open.insert(format!("desktop/run-{i:02}"));
            st.open.insert(format!("desktop/run-{i:02}/{sub}"));
        }
    }
    run(&mut st, &d, "files_tab", json!({ "tab": "artifacts" })).unwrap();
    let ui = render(&st, &d);
    assert_eq!(ui["rows"].as_array().unwrap().len(), PAGE);
    // The filter runs over all 3,000 before the page: a file deep and late.
    run(&mut st, &d, "files_filter", json!({ "text": "shot-2699" })).unwrap();
    let ui = render(&st, &d);
    assert_eq!(ui["rows"].as_array().unwrap().len(), 1);
    assert_eq!(ui["rows"][0]["id"], "desktop/run-29/window/shot-2699.png");
    assert_eq!(ui["count"]["label"], "", "nothing cut");
    // Selected there, then the filter cleared: its folders open and its row
    // is drawn even past the first page.
    run(&mut st, &d, "files_select", json!({ "id": "desktop/run-29/window/shot-2699.png" })).unwrap();
    run(&mut st, &d, "files_filter", json!({ "text": "" })).unwrap();
    let ui = render(&st, &d);
    assert!(ui["rows"].as_array().unwrap().iter().any(|r| r["selected"] == true), "the selected row is drawn");
    // The Files tab on a 3,000-entry folder: one page too.
    let names: Vec<J> = (0..3000).map(|i| json!({ "name": format!("f{i:04}.log"), "kind": "file", "size": 10, "modified": 1794199000 })).collect();
    d[DIR]["entries"] = J::Array(names);
    run(&mut st, &d, "files_tab", json!({ "tab": "files" })).unwrap();
    let ui = render(&st, &d);
    assert_eq!(ui["rows"].as_array().unwrap().len(), PAGE);
    assert!(ui["count"]["label"].as_str().unwrap().starts_with("3,0"), "{}", ui["count"]);
}

fn ui_of(acts: &[Action]) -> J {
    acts.iter().rev().find_map(|a| match a {
        Action::Set { key, value } if key == UI => Some(value.clone()),
        _ => None,
    }).expect("files_ui")
}

/// The explorer: opened from the rail on the focused pane's sandbox (else
/// the one used last); from a thread pane at its worktree, as the daemon
/// lists it (before or after the listing arrives); a Markdown file read
/// with its outline, a section, its relative and wiki links followed in
/// the explorer and the others opened through the safe path; a code file
/// with line numbers.
#[test]
fn the_explorer_opens_where_it_is_asked_and_reads_markdown_and_code() {
    let mut d = fixture("standin");
    d["app"]["focused"] = json!(1);
    d["app"]["panes"] = json!([{ "index": 1, "sandbox": "web-app" }]);
    let mut st = State::default();
    let a = run(&mut st, &d, "files_show", json!({})).unwrap();
    assert!(a.iter().any(|x| matches!(x, Action::OpenPanel(s) if s == SCREEN)));
    assert!(a.iter().any(|x| matches!(x, Action::Remember { key, value } if key == "last" && value == "web-app")));
    // A thread pane: at its worktree once the places are read.
    d["__daemon"]["files_tree"]["/Users/me/web/.sbxw-worktrees/checkout"] = json!({ "ok": true, "path": "/Users/me/web/.sbxw-worktrees/checkout", "parent": "/Users/me/web/.sbxw-worktrees", "entries": [{ "name": "README.md", "kind": "file", "size": 10, "modified": 1 }], "workspace": "/Users/me/web", "home": "/home/agent", "worktrees": d["__daemon"]["files_dir"]["worktrees"].clone() });
    let mut st = State::default();
    let a = run(&mut st, &d, "files_open", json!({ "sandbox": "web-app", "worktree": "checkout" })).unwrap();
    let a = pump(&mut st, &mut d, a);
    let u = ui_of(&a);
    assert_eq!(u["path"], "/Users/me/web/.sbxw-worktrees/checkout");
    assert_eq!(st.path, "/Users/me/web/.sbxw-worktrees/checkout");
    assert!(u["worktrees"].as_array().unwrap().iter().any(|w| w["thread"] == "checkout"));
    // A thread unknown to the daemon: nothing browsed.
    let mut st2 = opened(&d);
    run(&mut st2, &d, "files_open", json!({ "sandbox": "web-app", "worktree": "nope" })).unwrap();
    assert_ne!(st2.path, "nope");
    // Markdown: outline, heading, links.
    let mut st = opened(&d);
    let a = run(&mut st, &d, "files_browse", json!({ "path": "/Users/me/web" })).unwrap();
    pump(&mut st, &mut d, a);
    let a = run(&mut st, &d, "files_select", json!({ "id": "/Users/me/web/README.md" })).unwrap();
    let a = pump(&mut st, &mut d, a);
    st.preview = json!({ "state": "markdown", "text": "# Web\nSee [the notes](docs/notes.md) and [[CHANGELOG]] and [site](https://example.org).\n## Run\nnpm start\n## Test\nnpm test\n" });
    let u = st.ui(&Ctx { data: &d });
    let _ = a;
    let p = &u["preview"];
    assert_eq!(p["outline"].as_array().unwrap().len(), 3);
    let t = p["text"].as_str().unwrap();
    assert!(t.contains(&explorer::url("/Users/me/web/docs/notes.md")) && t.contains(&explorer::url("/Users/me/web/CHANGELOG.md")) && t.contains("(https://example.org)"), "{t}");
    // An Outline entry scrolls to its heading (the whole text stays) and is the one marked.
    let a = run(&mut st, &d, "files_outline", json!({ "index": 1 })).unwrap();
    assert!(a.iter().any(|x| matches!(x, Action::ScrollIntoView(k) if k == "files-text-b2")), "{a:?}");
    let p = st.ui(&Ctx { data: &d })["preview"].clone();
    assert!(p["text"].as_str().unwrap().starts_with("# Web") && p["text"].as_str().unwrap().contains("npm test"));
    assert_eq!(p["outline"][1]["on"], true);
    assert_eq!(p["outline"][0]["on"], false);
    assert!(run(&mut st, &d, "files_outline", json!({ "index": 9 })).is_err());
    run(&mut st, &d, "files_outline", json!({})).unwrap();
    // A relative link: its folder listed, the file selected when it comes.
    let a = run(&mut st, &d, "files_link", json!({ "url": explorer::url("/Users/me/web/src/app.ts") })).unwrap();
    assert!(a.iter().any(|x| matches!(x, Action::Fetch { key, path } if key == DIR && path.contains("path=%2FUsers%2Fme%2Fweb%2Fsrc"))), "{a:?}");
    let a = pump(&mut st, &mut d, a);
    assert_eq!(st.sel.as_ref().map(|s| s.path.as_str()), Some("/Users/me/web/src/app.ts"));
    // A code file: line numbers.
    let u = ui_of(&a);
    assert!(u["preview"]["numbered"].as_str().is_some_and(|t| t.starts_with('1') || t.starts_with(' ')), "{}", u["preview"]);
    // Another link: the browser, through the app's safe path.
    let a = run(&mut st, &d, "files_link", json!({ "url": "https://example.org/x" })).unwrap();
    assert!(a.iter().any(|x| matches!(x, Action::Intent { name, args } if name == "open_link" && args["url"] == "https://example.org/x")));
}

/// Everything the docked Project panel did is still reachable in the
/// explorer: the same intents, and the screen draws each action's control
/// (downloads, reveal, open, save as, copy path, delete, uploads by card,
/// Choose and Paste, the code map's entry, Generate code map) over every
/// state fixture.
#[test]
fn the_explorer_keeps_every_action_of_the_project_panel() {
    for n in ["files_download", "files_reveal", "files_open_with", "files_save_as", "files_copy_path", "files_delete", "files_upload_pick", "files_upload_paste", "files_upload_send", "files_upload_dropped", "files_upload_ask_again", "files_star", "files_hidden", "files_back", "files_up", "codemap_open", "codemap_generate", "codemap_lens"] {
        assert!(INTENTS.iter().any(|s| s.name == n), "{n}");
    }
    let dir = std::env::temp_dir().join("sbxw-no-overrides");
    let fx = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/files");
    let has = |n: &crate::luau::Node, k: &str| {
        fn walk(n: &crate::luau::Node, k: &str) -> bool {
            n.key.as_deref() == Some(k) || n.id.as_deref() == Some(k) || n.children.iter().any(|c| walk(c, k))
        }
        walk(n, k)
    };
    let mut seen = std::collections::BTreeSet::new();
    for f in std::fs::read_dir(&fx).unwrap() {
        let f = f.unwrap().path();
        if !f.file_name().unwrap().to_string_lossy().starts_with("files-") {
            continue;
        }
        let x = crate::fixture::Fixture::load(&f).unwrap();
        let s = crate::screen::ScreenState::new(SCREEN, &dir, x.screen_data(&crate::fixture::AppView::default()));
        assert!(s.error.is_none(), "{}: {:?}", f.display(), s.error);
        let n = s.last_good.as_ref().unwrap();
        // views2: the tabs became Locations (Workspace, Artifacts, Recent), the
        // path field a breadcrumb (with Go to… for a typed path), the upload card
        // a popover behind Upload, Code map beside the Workspace location.
        for k in KEYS {
            if has(n, k) {
                seen.insert(k);
            }
        }
    }
    for k in KEYS {
        assert!(seen.contains(k), "{k} is drawn in no state of the explorer");
    }
}

const KEYS: &[&str] = &[
    "files-download", "files-reveal", "files-open-with", "files-save-as", "files-copy-path", "files-delete", "files-drop", "files-upload-pick", "files-upload-paste",
    "files-codemap", "files-sandbox", "files-loc-/Users/me/web", "files-loc-artifacts", "files-loc-recent", "files-loc-/home/agent", "files-loc-/tmp", "files-loc-/",
    "files-worktree-checkout", "files-filter", "files-back", "files-up", "files-hidden", "files-star", "files-upload", "files-actions", "files-crumbs", "files-goto",
    "files-copy-folder", "files-sort", "files-refresh",
];

/// views2: the Locations are a visible list, grouped; each is reachable and
/// marked when it is where the list is; the shared ones are the former tabs.
#[test]
fn every_location_is_reachable_and_the_current_one_is_marked() {
    let mut d = fixture("standin");
    let mut st = opened_pumped(&mut d);
    let u = st.ui(&Ctx { data: &d });
    let groups: Vec<&str> = u["locations"].as_array().unwrap().iter().map(|g| g["group"].as_str().unwrap()).collect();
    assert_eq!(&groups[..2], ["This sandbox", "Shared with sbxw"]);
    assert!(groups.contains(&"Worktrees"), "{groups:?}");
    let items: Vec<J> = u["locations"].as_array().unwrap().iter().flat_map(|g| g["items"].as_array().unwrap().clone()).collect();
    let labels: Vec<&str> = items.iter().map(|x| x["label"].as_str().unwrap()).collect();
    for l in ["Home", "Workspace", "Temporary files", "File system root", "Artifacts", "Recent", "checkout"] {
        assert!(labels.contains(&l), "{l} in {labels:?}");
    }
    let ws = items.iter().find(|x| x["label"] == "Workspace").unwrap();
    assert!(ws["help"].as_str().unwrap().contains("mounted"), "the workspace says it is this computer's folder: {ws}");
    // Each path location lists its folder and is then the one marked.
    for (id, path) in [("/home/agent", "/home/agent"), ("/tmp", "/tmp"), ("/", "/"), ("/Users/me/web", "/Users/me/web")] {
        let a = run(&mut st, &d, "files_place", json!({ "id": id })).unwrap();
        let a = pump(&mut st, &mut d, a);
        let u = ui_of(&a);
        assert_eq!((u["tab"].as_str(), u["path"].as_str()), (Some("files"), Some(path)), "{id}");
        assert_eq!(u["location"], id, "{id} is marked");
    }
    // Inside a worktree: the worktree is marked, not the workspace.
    let a = run(&mut st, &d, "files_browse", json!({ "path": "/Users/me/web/.sbxw-worktrees/checkout" })).unwrap();
    let u = ui_of(&pump(&mut st, &mut d, a));
    assert_eq!(u["location"], "/Users/me/web/.sbxw-worktrees/checkout");
    // The shared ones: their lists.
    for t in ["artifacts", "recent"] {
        let a = run(&mut st, &d, "files_place", json!({ "id": t })).unwrap();
        let u = ui_of(&a);
        assert_eq!((u["tab"].as_str(), u["location"].as_str()), (Some(t), Some(t)));
    }
    assert!(run(&mut st, &d, "files_place", json!({ "id": "/etc" })).is_err(), "only a location");
}

/// The breadcrumb: the home as ~, every segment a path the list can go to.
#[test]
fn the_breadcrumb_says_where_the_list_is() {
    assert_eq!(view::crumbs("/home/agent/a/b", "/home/agent"), vec![json!({ "label": "~", "path": "/home/agent" }), json!({ "label": "a", "path": "/home/agent/a" }), json!({ "label": "b", "path": "/home/agent/a/b" })]);
    assert_eq!(view::crumbs("/", "/home/agent"), vec![json!({ "label": "/", "path": "/" })]);
    assert_eq!(view::crumbs("/Users/me/web", "/home/agent").iter().map(|c| c["label"].as_str().unwrap()).collect::<Vec<_>>(), ["/", "Users", "me", "web"]);
    let mut d = fixture("standin");
    let mut st = opened_pumped(&mut d);
    let a = run(&mut st, &d, "files_browse", json!({ "path": "/Users/me/web/src" })).unwrap();
    let u = ui_of(&pump(&mut st, &mut d, a));
    let c = u["crumbs"].as_array().unwrap();
    assert_eq!(c.last().unwrap()["path"], "/Users/me/web/src");
    // The screen: a long path folds in the middle, never at its end; a segment goes there.
    let mut data = d.clone();
    data["files_ui"] = u.clone();
    data["files_ui"]["crumbs"] = json!((0..12).map(|i| json!({ "label": format!("d{i}"), "path": format!("/{}", (0..=i).map(|j| format!("d{j}")).collect::<Vec<_>>().join("/")) })).collect::<Vec<_>>());
    data["app"]["overlay"] = json!({ "w": 1200, "h": 700 });
    let mut h = crate::luau::LuauHost::new(crate::luau::Limits::default()).unwrap();
    h.load(SCREEN, include_str!("../../../screens/files.luau")).unwrap();
    let tree = h.render(&data).unwrap();
    fn find<'a>(n: &'a crate::luau::Node, k: &str) -> Option<&'a crate::luau::Node> {
        if crate::screen::key_of(n).as_deref() == Some(k) {
            return Some(n);
        }
        n.children.iter().find_map(|c| find(c, k))
    }
    assert!(find(&tree, "files-crumb-12").is_some() && find(&tree, "files-crumb-1").is_some(), "first and last kept");
    assert!(find(&tree, "files-crumb-5").is_none() && find(&tree, "files-crumbs-more").is_some(), "the middle folds into a menu");
    let seg = find(&tree, "files-crumb-11").unwrap();
    let asks = h.act(seg.on.as_deref().unwrap(), &seg.arg, &J::Null).unwrap();
    assert_eq!(asks, json!({ "kind": "intent", "name": "files_browse", "args": { "path": "/d0/d1/d2/d3/d4/d5/d6/d7/d8/d9/d10" } }));
    let copy = find(&tree, "files-copy-folder").unwrap();
    assert_eq!(h.act(copy.on.as_deref().unwrap(), &copy.arg, &J::Null).unwrap()["name"], "copy_text");
}

/// The list is a tree: a folder opens in place (read from the sandbox, its
/// count), folds back; the keys move, open, fold, read; typing filters.
#[test]
fn the_tree_opens_folders_in_place_and_takes_the_keys() {
    let mut d = fixture("standin");
    let mut st = opened_pumped(&mut d);
    let a = run(&mut st, &d, "files_place", json!({ "id": "/home/agent" })).unwrap();
    let u = ui_of(&pump(&mut st, &mut d, a));
    let names = |u: &J| u["rows"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(names(&u), ["scratch", "notes.md"], "folders first, dotfiles hidden");
    // Open scratch in place: one read, its rows under it, one deeper.
    let a = run(&mut st, &d, "files_expand", json!({ "id": "/home/agent/scratch" })).unwrap();
    assert!(a.iter().any(|x| matches!(x, Action::Call(c) if c.path == "/api/sandboxes/web-app/fs?path=%2Fhome%2Fagent%2Fscratch")), "{a:?}");
    let u = ui_of(&pump(&mut st, &mut d, a));
    assert_eq!(names(&u), ["scratch", "draft.txt", "notes.md"]);
    let r = &u["rows"][0];
    assert_eq!((r["open"].as_bool(), r["count"].as_u64(), u["rows"][1]["depth"].as_u64()), (Some(true), Some(1), Some(1)));
    // A file opened in place is read from its listing.
    let a = run(&mut st, &d, "files_select", json!({ "id": "/home/agent/scratch/draft.txt" })).unwrap();
    pump(&mut st, &mut d, a);
    assert_eq!(st.sel.as_ref().map(|s| s.path.as_str()), Some("/home/agent/scratch/draft.txt"));
    // Keys: Down to the first row, Left folds it, Right opens it, Down, Enter reads.
    let key = |st: &mut State, d: &mut J, k: &str| {
        let a = run(st, d, "files_key", json!({ "key": k })).unwrap();
        pump(st, d, a);
    };
    st.cursor = None;
    key(&mut st, &mut d, "down");
    assert_eq!(st.cursor.as_deref(), Some("/home/agent/scratch"));
    key(&mut st, &mut d, "left");
    assert!(!st.expanded.contains("/home/agent/scratch"), "Left folds");
    key(&mut st, &mut d, "right");
    assert!(st.expanded.contains("/home/agent/scratch"), "Right opens");
    key(&mut st, &mut d, "down");
    assert_eq!(st.cursor.as_deref(), Some("/home/agent/scratch/draft.txt"));
    key(&mut st, &mut d, "left");
    assert_eq!(st.cursor.as_deref(), Some("/home/agent/scratch"), "Left on a file: its folder's row");
    key(&mut st, &mut d, "down");
    key(&mut st, &mut d, "down");
    key(&mut st, &mut d, "enter");
    assert_eq!(st.sel.as_ref().map(|s| s.path.as_str()), Some("/home/agent/notes.md"), "Enter reads the file");
    // Enter on a folder lists it.
    st.cursor = Some("/home/agent/scratch".into());
    key(&mut st, &mut d, "enter");
    assert_eq!(st.path, "/home/agent/scratch");
    // Typing filters; Backspace takes a letter back.
    run(&mut st, &d, "files_key", json!({ "key": "type", "text": "dr" })).unwrap();
    assert_eq!(st.filter, "dr");
    run(&mut st, &d, "files_key", json!({ "key": "backspace" })).unwrap();
    assert_eq!(st.filter, "d");
    // Sort by date: the newest first, folders still first.
    let a = run(&mut st, &d, "files_place", json!({ "id": "/tmp" })).unwrap();
    pump(&mut st, &mut d, a);
    let u = ui_of(&run(&mut st, &d, "files_sort", json!({ "by": "date" })).unwrap());
    assert_eq!(names(&u), ["upload-report.pdf", "paste-1794199000.png"]);
    assert!(run(&mut st, &d, "files_sort", json!({ "by": "size" })).is_err() || crate::intent::parse(&json!({ "kind": "intent", "name": "files_sort", "args": { "by": "size" } })).is_err());
    for n in ["files_expand", "files_sort", "files_key"] {
        assert!(matches!(spec(n).risk, Risk::Direct), "{n}");
    }
}

#[test]
fn a_path_from_a_terminal_opens_the_explorer_on_it_and_nothing_else() {
    let mut d = fixture("standin");
    let mut st = State::default();
    let a = run(&mut st, &d, "files_open_path", json!({ "sandbox": "web-app", "path": "src/main.ts" })).unwrap();
    admit("files_open_path", &a).unwrap();
    assert!(a.iter().any(|x| matches!(x, Action::OpenPanel(s) if s == "files")));
    // Only the explorer: no download, no system open, no dialog, no delete, no link.
    let quiet = |acts: &[Action]| acts.iter().all(|x| !matches!(x, Action::Download(_) | Action::Reveal(_) | Action::Confirm(_) | Action::Prepare(_) | Action::OpenLink(_) | Action::PickFiles { .. } | Action::Intent { .. }));
    assert!(quiet(&a), "{a:?}");
    let all = pump(&mut st, &mut d, a);
    assert!(quiet(&all), "{all:?}");
    // A relative path is read from the workspace: the folder listed, the file selected and previewed.
    assert_eq!(d[UI]["tab"], "files");
    assert_eq!(d[DIR]["path"], "/Users/me/web/src");
    assert_eq!(d[UI]["selected"]["name"], "main.ts");
    // An absolute path, and one under ~, the same.
    let a = run(&mut st, &d, "files_open_path", json!({ "sandbox": "web-app", "path": "/Users/me/web/src/app.ts" })).unwrap();
    pump(&mut st, &mut d, a);
    assert_eq!(d[UI]["selected"]["name"], "app.ts");
    let a = run(&mut st, &d, "files_open_path", json!({ "sandbox": "web-app", "path": "~/scratch/notes.md" })).unwrap();
    assert!(a.iter().any(|x| matches!(x, Action::Fetch { path, .. } if path.ends_with("/fs?path=%2Fhome%2Fagent%2Fscratch"))), "{a:?}");
}

#[test]
fn a_file_that_is_not_there_says_so_and_selects_no_readme() {
    let mut d = fixture("standin");
    let mut st = State::default();
    let a = run(&mut st, &d, "files_open_path", json!({ "sandbox": "web-app", "path": "./nope.md" })).unwrap();
    let all = pump(&mut st, &mut d, a);
    assert!(all.iter().any(|x| matches!(x, Action::Notice { text, .. } if text.contains("not in this folder"))), "{all:?}");
    assert_eq!(d[UI]["selected"], J::Null, "the folder's README is not put in its place");
}

#[test]
fn the_intent_refuses_a_path_outside_the_terminal_link_rule() {
    let d = fixture("standin");
    for p in [
        "../x.md", "a/../b.md", "/etc/../etc/passwd.md", "http://x.com/a.md", "file:///etc/a.md", "javascript:a/b.md", "a/b.exe", "src/main.ts:12", "~root/a.md", "a/b c.md", "a//b.md", "foo.md",
        "a/.env", "a/b.md\n", "a/\u{430}.md", "a/b.md/", "", "/",
    ] {
        let mut st = State::default();
        let r = run(&mut st, &d, "files_open_path", json!({ "sandbox": "web-app", "path": p }));
        assert!(r.is_err(), "{p:?} was accepted");
        assert_eq!(st.sandbox, None, "{p:?} opened the explorer");
    }
    assert!(run(&mut State::default(), &d, "files_open_path", json!({ "sandbox": "nobody", "path": "a/b.md" })).is_err());
    assert!(matches!(spec("files_open_path").risk, Risk::Direct));
}

#[test]
fn a_thread_pane_resolves_a_relative_path_in_its_worktree() {
    let mut d = fixture("standin");
    // The places are not read yet: the path waits for them, and nothing is listed before.
    let mut st = State::default();
    let a = run(&mut st, &d, "files_open_path", json!({ "sandbox": "web-app", "path": "./README.md", "thread": "checkout" })).unwrap();
    assert!(!a.iter().any(|x| matches!(x, Action::Fetch { path, .. } if path.contains("checkout"))), "{a:?}");
    let listing = d["__daemon"]["files_dir"].clone();
    let b = st.on_data(&Ctx { data: &d }, DIR, &J::Null, &listing);
    assert!(b.iter().any(|x| matches!(x, Action::Fetch { path, .. } if path.ends_with("/fs?path=%2FUsers%2Fme%2Fweb%2F.sbxw-worktrees%2Fcheckout"))), "{b:?}");
    // Places already read: at once; an unknown thread falls back to the workspace.
    let mut st = opened_pumped(&mut d);
    let a = run(&mut st, &d, "files_open_path", json!({ "sandbox": "web-app", "path": "./README.md", "thread": "checkout" })).unwrap();
    assert!(a.iter().any(|x| matches!(x, Action::Fetch { path, .. } if path.ends_with("%2F.sbxw-worktrees%2Fcheckout"))), "{a:?}");
    let a = run(&mut st, &d, "files_open_path", json!({ "sandbox": "web-app", "path": "src/main.ts", "thread": "unknown" })).unwrap();
    assert!(a.iter().any(|x| matches!(x, Action::Fetch { path, .. } if path.ends_with("%2FUsers%2Fme%2Fweb%2Fsrc"))), "{a:?}");
}

/// A location's row is 24 pt (the 4 pt steps; was 22): 13 pt text and its
/// 12 pt glyph inside, a pointer target no smaller than before.
#[test]
fn a_location_row_is_24_pt() {
    let dir = std::env::temp_dir().join("sbxw-no-overrides");
    let f = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/files/files-browse.json");
    let x = crate::fixture::Fixture::load(&f).unwrap();
    let s = crate::screen::ScreenState::new(SCREEN, &dir, x.screen_data(&crate::fixture::AppView::default()));
    fn walk(n: &crate::luau::Node, out: &mut Vec<(String, Option<crate::luau::Size>)>) {
        if let Some(k) = n.key.as_deref().filter(|k| k.starts_with("files-loc-") || k.starts_with("files-worktree-")) {
            out.push((k.to_string(), n.style.h));
        }
        n.children.iter().for_each(|c| walk(c, out));
    }
    let mut rows = vec![];
    walk(s.last_good.as_ref().unwrap(), &mut rows);
    assert!(rows.len() >= 3, "{rows:?}");
    for (k, h) in rows {
        assert_eq!(h, Some(crate::luau::Size::Pt(24.0)), "{k}");
    }
}

/// A picture pasted into a pane: its one dialog at once (paste-image, the
/// name the app made), and on the daemon's answer its path typed into that
/// pane's session with a space, never a Return; Cancel leaves nothing; a
/// stopped sandbox is refused with no dialog.
#[test]
fn a_picture_pasted_into_a_pane_asks_once_then_types_its_path() {
    let d = fixture("standin");
    let mut st = State::default();
    let paste = |sb: &str| PaneDrop { paths: vec![], skip_dialog: false, sandbox: sb.into(), thread: Some("checkout".into()), picture: Some(vec![7; 3000]) };
    let a = st.on_pane_drop(&Ctx { data: &d }, &paste("web-app"));
    let [Action::Confirm(c)] = a.as_slice() else { panic!("{a:?}") };
    assert!(st.staged.is_empty(), "never shown as waiting in the panel");
    assert!(c.title.starts_with("Upload pasted-20") && c.title.ends_with(".png into web-app?"), "{}", c.title);
    assert_eq!(c.lines[0], "a picture from the clipboard · 2.9 KB");
    assert_eq!(c.call.path, "/api/sandboxes/web-app/paste-image");
    let a = st.on_answer(&Ctx { data: &d }, &c.call, Ok((200, json!({ "ok": true, "path": "/tmp/sbxw-pastes/paste-1.png" }))));
    let calls: Vec<&Call> = a.iter().filter_map(|x| if let Action::Call(c) = x { Some(c) } else { None }).collect();
    let [t] = calls.as_slice() else { panic!("{a:?}") };
    assert_eq!((t.path.as_str(), &t.body), ("/api/input", &json!({ "sandbox": "web-app", "mode": "thread:checkout", "data": "/tmp/sbxw-pastes/paste-1.png " })));
    assert!(st.dropped.is_empty());
    // A path the daemon does not make is never typed.
    let a = st.on_pane_drop(&Ctx { data: &d }, &paste("web-app"));
    let [Action::Confirm(c)] = a.as_slice() else { panic!() };
    let a = st.on_answer(&Ctx { data: &d }, &c.call, Ok((200, json!({ "ok": true, "path": "/tmp/x; rm -rf ~" }))));
    assert!(!a.iter().any(|x| matches!(x, Action::Call(_))), "{a:?}");
    // The daemon unreachable: said, nothing typed.
    let a = st.on_pane_drop(&Ctx { data: &d }, &paste("web-app"));
    let [Action::Confirm(c)] = a.as_slice() else { panic!() };
    let a = st.on_answer(&Ctx { data: &d }, &c.call, Err("not connected to the daemon".into()));
    assert!(a.iter().any(|x| matches!(x, Action::Notice { text, error: true, .. } if text.ends_with("was not uploaded: not connected to the daemon"))), "{a:?}");
    assert!(!a.iter().any(|x| matches!(x, Action::Call(_))));
    // Cancel: nothing left.
    let a = st.on_pane_drop(&Ctx { data: &d }, &paste("web-app"));
    let [Action::Confirm(c)] = a.as_slice() else { panic!() };
    st.on_dismiss(&Ctx { data: &d }, &c.call.tag);
    assert!(st.dropped.is_empty());
    // A stopped sandbox: refused, no dialog.
    let a = st.on_pane_drop(&Ctx { data: &d }, &paste("data-pipeline"));
    assert!(matches!(a.as_slice(), [Action::Notice { text, error: true, .. }] if text.contains("not running")), "{a:?}");
    assert!(st.dropped.is_empty());
}

#[test]
fn a_pasted_picture_is_named_by_the_time_only() {
    assert_eq!(pasted_name(1_795_444_205_000), "pasted-2026-11-23-143005.png");
}
