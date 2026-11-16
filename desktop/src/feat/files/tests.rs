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
        let gated = matches!(s.name, "files_save_as" | "files_open_with" | "files_delete" | "files_upload_send" | "codemap_diagram");
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
    assert!(ui["staged"][0]["problem"].as_str().unwrap().contains("more than the 128.0 MB"), "{ui}");
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
    // Dropped on a stopped sandbox's pane: refused when sent.
    let a = st.on_pane_drop(&Ctx { data: &d }, &PaneDrop { paths: vec![small.clone()], sandbox: "data-pipeline".into(), thread: None });
    assert!(a.iter().any(|x| matches!(x, Action::Intent { name, .. } if name == "files_upload_send")));
    assert!(run(&mut st, &d, "files_upload_send", json!({})).unwrap_err().contains("not running"));
    st.staged.clear();
    // Dropped on a thread's pane: that sandbox, that thread's prompt.
    st.on_pane_drop(&Ctx { data: &d }, &PaneDrop { paths: vec![small.clone()], sandbox: "api".into(), thread: Some("checkout".into()) });
    let a = run(&mut st, &d, "files_upload_send", json!({})).unwrap();
    let [Action::Confirm(c)] = a.as_slice() else { panic!() };
    assert_eq!(c.title, "Upload notes.txt into api?");
    assert!(c.lines[2].contains("api's thread checkout's prompt"));
    let a = st.on_answer(&Ctx { data: &d }, &c.call, Ok((200, json!({ "ok": true, "path": "/tmp/sbxw-uploads/2/notes.txt" }))));
    assert!(a.iter().any(|x| matches!(x, Action::Call(t) if t.body["mode"] == "thread:checkout" && t.body["sandbox"] == "api")), "{a:?}");
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
    assert!(ex.iter().any(|e| e.id == "project" && e.place == Place::Pane("web-app".into())));
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
