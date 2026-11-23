//! Canvas: the feature driven as the core drives it, against the fixture
//! daemon's routes (`model::standin`), and the window with the overlay.

use super::*;
use crate::feat::Method;
use crate::luau::{LuauHost, Limits, Node};
use serde_json::json;

fn fixture(name: &str) -> J {
    let p = format!("{}/fixtures/canvas/{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// The app's data as a feature sees it: the fixture's daemon, the overlay shown, nothing over it.
struct World {
    data: J,
    /// The fixture daemon's state (its routes change it).
    daemon: J,
    /// Every call that went out, in order.
    calls: Vec<Call>,
    /// Calls the test answers itself (not the stand-in), by tag prefix.
    hold: Vec<&'static str>,
    held: Vec<Call>,
}

impl World {
    fn new(name: &str) -> World {
        *STATE().lock().unwrap() = State::default();
        crate::surface_canvas::close(SURF);
        let fx = fixture(name);
        let mut data = fx["daemon"].clone();
        data["app"] = json!({ "covered": false, "covered_top": false, "overlay": { "screen": "canvas", "w": 1200, "h": 700 }, "selected": "web-app", "groups": {} });
        data["jarvis"] = json!({ "pending": [] });
        World { data, daemon: fx["daemon"].clone(), calls: vec![], hold: vec![], held: vec![] }
    }
    fn cx(&self) -> Ctx<'_> {
        Ctx { data: &self.data }
    }
    fn intent(&mut self, name: &'static str, args: J) -> Result<Vec<Action>, String> {
        let a = Asked { name, args: args.as_object().cloned().unwrap_or_default() };
        let r = act(&self.cx(), &a);
        // The core's rule: an approval answers with its one dialog.
        let risk = INTENTS.iter().find(|s| s.name == name).unwrap().risk;
        if let (Risk::Approval(_), Ok(acts)) = (risk, &r) {
            assert!(matches!(acts.as_slice(), [Action::Confirm(_)]), "{name}: an approval answers with one Confirm: {acts:?}");
        }
        r
    }
    /// Runs the actions as the core would: calls answered by the fixture
    /// daemon (pictures as bytes), until nothing is left; a Confirm is left
    /// for the test. Returns the Sets of the view and the other actions.
    fn run(&mut self, acts: Vec<Action>) -> Vec<Action> {
        let mut todo: std::collections::VecDeque<Action> = acts.into();
        let mut rest = vec![];
        while let Some(a) = todo.pop_front() {
            match a {
                Action::Call(c) if c.method != Method::Core => {
                    self.calls.push(c.clone());
                    if self.hold.iter().any(|h| c.tag.starts_with(h)) {
                        self.held.push(c);
                        continue;
                    }
                    let body = if let Some((ct, b)) = &c.bytes { json!({ "$bytes": b.len(), "$type": ct }) } else { c.body.clone() };
                    let method = if matches!(c.method, Method::Get) { "GET" } else { "POST" };
                    let (code, v) = crate::feat::answer_from(&mut self.daemon, method, &c.path, &body, &mut |_, _| {});
                    let cx = Ctx { data: &self.data };
                    let more = if c.raw.is_some() {
                        let (b, ct) = crate::client::raw_of(&v);
                        on_bytes(&cx, &c, Ok((code, b, ct)))
                    } else if c.path.starts_with("/api/chat/push") || c.path.contains("/upload?") {
                        // Routes of other features: a plain answer.
                        let ans = if c.path.contains("/upload?") { json!({ "ok": true, "path": format!("/tmp/sbxw-uploads/1/{}", c.path.rsplit('=').next().unwrap()) }) } else { json!({ "ok": true }) };
                        on_answer(&cx, &c, Ok((200, ans)))
                    } else {
                        on_answer(&cx, &c, Ok((code, v)))
                    };
                    todo.extend(more);
                }
                Action::Set { key, value } => {
                    self.data[key.as_str()] = value.clone();
                    rest.push(Action::Set { key, value });
                }
                other => rest.push(other),
            }
        }
        rest
    }
    fn ui(&self) -> &J {
        &self.data[UI]
    }
    /// Opens the view; its pictures decode on worker threads.
    fn open(&mut self) {
        let a = self.intent("canvas_show", json!({})).unwrap();
        self.run(a);
    }
    /// Waits until every picture of the canvas shown is decoded.
    fn decoded(&self) {
        let want = STATE().lock().unwrap().canvas.as_ref().map_or(0, |c| c.shots.len() + c.assets.len());
        for _ in 0..1000 {
            if crate::surface_canvas::mem().1 >= want {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("{want} pictures not decoded: {:?}", crate::surface_canvas::mem());
    }
    fn surface(&mut self, ev: CanvasEvent) -> Vec<Action> {
        let a = on_surface(&crate::surface::SurfaceEvent::Canvas { surface: SURF.into(), ev });
        self.run(a)
    }
    fn confirm(&mut self, acts: &[Action]) -> Confirm {
        acts.iter().find_map(|a| match a {
            Action::Confirm(c) => Some((**c).clone()),
            _ => None,
        }).expect("a Confirm")
    }
    /// The person pressed the dialog's Confirm: its call goes out.
    fn press(&mut self, c: &Confirm) -> Vec<Action> {
        self.run(vec![Action::Call(c.call.clone())])
    }
    fn pushed(&self) -> Vec<String> {
        self.calls.iter().filter(|c| c.path == "/api/chat/push").map(|c| c.body["text"].as_str().unwrap_or("").to_string()).collect()
    }
}

#[test]
fn it_opens_on_the_sandboxes_latest_canvas_and_draws_every_round() {
    let mut w = World::new("basic");
    w.open();
    let ui = w.ui().clone();
    assert_eq!(ui["current"]["title"], "Checkout review");
    assert_eq!(ui["current"]["shots"], 11);
    let scene = crate::surface_canvas::scene(SURF);
    let bands: Vec<&str> = scene.items.iter().filter_map(|i| match &i.body { crate::surface_canvas::Body::Band { label } => Some(label.as_str()), _ => None }).collect();
    assert_eq!(bands.len(), 2, "{bands:?}");
    // The newer round is below the older one, the same screen in the same column.
    let at = |id: &str| scene.items.iter().find(|i| i.id == id).map(|i| (i.x, i.y)).unwrap();
    assert_eq!(at("s-2-0").0, at("s-1-3").0);
    assert!(at("s-2-0").1 > at("s-1-3").1);
    // Sent pins muted, draft pins live; the side list holds the draft, the sent round apart.
    assert_eq!(scene.pins.iter().filter(|p| p.sent).count(), 3);
    assert_eq!(scene.pins.iter().filter(|p| !p.sent).count(), 2);
    assert_eq!(ui["draft"].as_array().unwrap().len(), 4);
    assert_eq!(ui["history"][0]["n"], 1);
    // The picker: the sandbox's canvases, newest first, then New; the removed sandbox listed by name.
    let labels: Vec<&str> = ui["canvases"].as_array().unwrap().iter().filter_map(|c| c["label"].as_str()).collect();
    assert!(labels[0].starts_with("Checkout review ·") && labels[1].starts_with("Settings pages ·") && labels.contains(&"New canvas") && labels.last().unwrap().starts_with("Storage: This canvas"), "{labels:?}");
    assert!(ui["sandboxes"].as_array().unwrap().iter().any(|s| s["label"] == "old-shop (removed)"));
    assert!(ui["storage"].as_str().unwrap().contains("all canvases"));
}

#[test]
fn pins_are_numbered_per_canvas_and_linked_to_the_list_both_ways() {
    let mut w = World::new("basic");
    w.open();
    let acts = w.surface(CanvasEvent::PinAt { item: "s-2-1".into(), fx: 0.25, fy: 0.75 });
    // After every pin, sent or not (1..5): 6; selected, its field focused, saved.
    assert!(acts.iter().any(|a| matches!(a, Action::Focus(k) if k == "canvas-text-pin-6")));
    assert!(w.calls.iter().any(|c| c.tag.starts_with("cv-save:") && c.body["pins"].as_array().unwrap().iter().any(|p| p["n"] == 6)));
    let row = w.ui()["draft"].as_array().unwrap().iter().find(|d| d["key"] == "pin-6").cloned().unwrap();
    assert_eq!(row["selected"], true);
    assert!(row["label"].as_str().unwrap().contains("Checkout: address (after)") && row["label"].as_str().unwrap().contains("bottom-left"));
    // The list's row: the surface selects it and brings the pin into view.
    let a = w.intent("canvas_select", json!({ "target": "pin-2" })).unwrap();
    w.run(a);
    assert_eq!(crate::surface_canvas::scene(SURF).selected.as_deref(), Some("pin-2"));
    let cam = crate::surface_canvas::camera(SURF);
    let shot = crate::surface_canvas::scene(SURF).items.into_iter().find(|i| i.id == "s-1-4").unwrap();
    let p = cam.screen([shot.x + 0.3 * shot.w, shot.y + 0.55 * shot.h]);
    assert!(p[0] >= 0.0 && p[1] >= 0.0 && p[0] <= cam.view[0] && p[1] <= cam.view[1], "pin 2 in view: {p:?}");
    // The surface's press on a pin selects its row.
    w.surface(CanvasEvent::Select { target: Some("pin-4".into()) });
    assert!(w.ui()["draft"].as_array().unwrap().iter().any(|d| d["key"] == "pin-4" && d["selected"] == true));
}

#[test]
fn a_pin_keeps_its_point_on_its_shot_at_any_zoom_and_pan() {
    use crate::surface_canvas::{Camera, ZoomCmd};
    let mut w = World::new("basic");
    w.open();
    let shot = crate::surface_canvas::scene(SURF).items.into_iter().find(|i| i.id == "s-2-2").unwrap();
    let pin = |cam: &Camera| cam.screen([shot.x + 0.68 * shot.w, shot.y + 0.62 * shot.h]);
    let corner = |cam: &Camera| cam.screen([shot.x, shot.y]);
    for cmd in [ZoomCmd::In, ZoomCmd::In, ZoomCmd::Actual, ZoomCmd::Out, ZoomCmd::Fit] {
        crate::surface_canvas::zoom(SURF, cmd);
        let cam = crate::surface_canvas::camera(SURF);
        let (p, c) = (pin(&cam), corner(&cam));
        // On screen, the pin is at 68 % and 62 % of the shot as drawn.
        let fx = (p[0] - c[0]) / (shot.w * cam.zoom);
        let fy = (p[1] - c[1]) / (shot.h * cam.zoom);
        assert!((fx - 0.68).abs() < 1e-3 && (fy - 0.62).abs() < 1e-3, "{cmd:?}: {fx} {fy}");
    }
}

#[test]
fn edits_are_saved_one_request_at_a_time_and_nothing_is_lost() {
    let mut w = World::new("basic");
    w.open();
    w.hold = vec!["cv-save:"];
    let a = w.intent("canvas_text", json!({ "target": "pin-4", "text": "One" })).unwrap();
    w.run(a);
    let a = w.intent("canvas_text", json!({ "target": "pin-4", "text": "One two" })).unwrap();
    w.run(a);
    assert_eq!(w.held.len(), 1, "one save in flight");
    // Its answer sends the rest: the newest text.
    w.hold.clear();
    let first = w.held.remove(0);
    let cx = Ctx { data: &w.data };
    let more = on_answer(&cx, &first, Ok((200, json!({ "ok": true }))));
    w.run(more);
    let last = w.calls.iter().rev().find(|c| c.tag.starts_with("cv-save:")).unwrap();
    assert!(last.body["pins"].as_array().unwrap().iter().any(|p| p["n"] == 4 && p["text"] == "One two"));
    // A sent pin does not change.
    assert!(w.intent("canvas_text", json!({ "target": "pin-1", "text": "x" })).is_err());
    assert!(w.intent("canvas_remove", json!({ "target": "pin-1" })).is_err());
    // A note placed with the Note tool, then moved: saved with its place.
    w.surface(CanvasEvent::NoteAt { x: 100.0, y: 200.0 });
    let id = w.ui()["selected"].as_str().unwrap().to_string();
    w.surface(CanvasEvent::Moved { item: id.clone(), x: 300.0, y: 400.0 });
    let last = w.calls.iter().rev().find(|c| c.tag.starts_with("cv-save:")).unwrap();
    assert!(last.body["notes"].as_array().unwrap().iter().any(|n| n["id"] == id.as_str() && n["x"] == 300.0));
}

#[test]
fn an_asset_comes_only_from_the_core_and_no_intent_takes_a_path() {
    for s in INTENTS {
        for (n, _, _) in s.args {
            assert!(!["path", "file", "files", "local"].contains(n), "{} takes {n}", s.name);
        }
    }
    let mut w = World::new("basic");
    w.open();
    let png = super::model::mock_png(80, 60, 1);
    let a = on_files(&w.cx(), Incoming::Png(png));
    w.run(a);
    let c = w.calls.iter().find(|c| c.tag.starts_with("cv-asset:")).expect("the picture posted");
    assert!(c.path.starts_with("/api/canvas/c-00000000000000c1/assets?name=pasted-"));
    // A file that is not a picture: refused, nothing posted.
    let dir = std::env::temp_dir().join(format!("sbxw-canvas-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("notes.svg");
    std::fs::write(&f, "<svg xmlns='http://www.w3.org/2000/svg'><script/></svg>").unwrap();
    let n = w.calls.len();
    let a = on_files(&w.cx(), Incoming::Files(vec![f]));
    assert!(a.iter().any(|x| matches!(x, Action::Notice { error: true, text, .. } if text.contains("not a PNG"))));
    assert_eq!(w.calls.len(), n);
    let _ = std::fs::remove_dir_all(dir);
    // Under a dialog nothing is added.
    w.data["app"]["covered_top"] = true.into();
    let a = on_files(&w.cx(), Incoming::Png(super::model::mock_png(8, 8, 1)));
    assert!(a.iter().all(|x| !matches!(x, Action::Call(_))));
}

#[test]
fn a_send_is_one_dialog_then_the_uploads_then_the_round_then_one_fixed_line() {
    let mut w = World::new("basic");
    w.open();
    w.decoded();
    let a = w.intent("canvas_send", json!({})).unwrap();
    let c = w.confirm(&a);
    assert_eq!(c.title, "Send 4 remarks to web-app · main?");
    let all = format!("{}\n{}", c.lines.join("\n"), c.preview.as_ref().unwrap().text);
    for t in ["Better. Make the button full width on narrow screens.", "Use the example below for the empty cart.", "Round 2 looks closer.", "Example 1", "empty-cart-example.png"] {
        assert!(all.contains(t), "the dialog shows {t}: {all}");
    }
    // Nothing went yet.
    assert!(w.pushed().is_empty() && !w.calls.iter().any(|c| c.tag.starts_with("cv-up:") || c.tag.starts_with("cv-submit:")));
    // The first call is the asset's upload; then the round; then the line.
    assert!(c.call.tag.starts_with("cv-up:") && c.call.path.starts_with("/api/sandboxes/web-app/upload?name=canvas-asset-1.png"));
    w.press(&c);
    let order: Vec<&str> = w.calls.iter().map(|c| c.tag.as_str()).filter(|t| t.starts_with("cv-up") || t.starts_with("cv-submit") || t.starts_with("cv-push")).collect();
    assert_eq!(order.len(), 3, "{order:?}");
    assert!(order[0].starts_with("cv-up:") && order[1].starts_with("cv-submit:") && order[2].starts_with("cv-push:"));
    let submit = w.calls.iter().find(|c| c.tag.starts_with("cv-submit:")).unwrap();
    assert_eq!(submit.body["assets"]["a-1"], "/tmp/sbxw-uploads/1/canvas-asset-1.png");
    // ONE line, sbxw's own: the remarks travel by canvas_read (the main session started after the shots).
    let p = w.pushed();
    assert_eq!(p.len(), 1);
    assert_eq!(p[0], "[sbxw canvas · c-00000000000000c1] review round 2 is ready: run canvas_read to read the person's remarks");
    // The round is history now; nothing new to send.
    assert_eq!(w.ui()["history"].as_array().unwrap().len(), 2);
    assert_eq!(w.ui()["send"]["ok"], false);
    assert!(w.intent("canvas_send", json!({})).is_err());
}

#[test]
fn a_session_without_canvas_read_gets_the_plain_list_after_the_line() {
    let mut w = World::new("basic");
    // The main session started before the first shot: it may lack the tool.
    w.data["sessions"][0]["started_ms"] = 1.into();
    w.open();
    let a = w.intent("canvas_send", json!({})).unwrap();
    let c = w.confirm(&a);
    assert!(c.lines.iter().any(|l| l.contains("plain numbered list")));
    w.press(&c);
    let p = w.pushed();
    assert_eq!(p.len(), 1);
    assert!(p[0].starts_with("[sbxw canvas · ") && p[0].contains("\n1. Pin 4 on shot \"Checkout: payment (after)\""), "{}", p[0]);
}

#[test]
fn a_failed_upload_records_and_types_nothing() {
    let mut w = World::new("basic");
    w.open();
    let a = w.intent("canvas_send", json!({})).unwrap();
    let c = w.confirm(&a);
    let cx = Ctx { data: &w.data };
    let more = on_answer(&cx, &c.call, Ok((500, json!({ "ok": false, "error": "disk full" }))));
    assert!(more.iter().any(|x| matches!(x, Action::Notice { error: true, text, .. } if text.contains("Not sent") && text.contains("disk full"))));
    let more = w.run(more);
    assert!(more.iter().all(|x| !matches!(x, Action::Call(_))));
    assert!(w.pushed().is_empty() && !w.calls.iter().any(|c| c.tag.starts_with("cv-submit:")));
    // The remarks are still the draft.
    assert_eq!(w.ui()["draft"].as_array().unwrap().len(), 4);
}

#[test]
fn every_refusal_holds_in_rust() {
    let cases: Vec<(&str, Box<dyn Fn(&mut J)>, &str)> = vec![
        ("covered", Box::new(|d: &mut J| d["app"]["covered_top"] = true.into()), "open over the canvas"),
        ("jarvis", Box::new(|d: &mut J| d["jarvis"]["pending"] = json!([{ "id": "j1" }])), "Jarvis proposal"),
        ("question", Box::new(|d: &mut J| d["sessions"][0]["state"] = "attention".into()), "question or a permission"),
        ("ended", Box::new(|d: &mut J| d["sessions"] = json!([{ "sandbox": "web-app", "state": "exited", "answerable": true }])), "no open agent session"),
        ("stopped", Box::new(|d: &mut J| d["sandboxes"][0]["status"] = "stopped".into()), "not running"),
        ("none", Box::new(|d: &mut J| d["sessions"] = json!([])), "no open agent session"),
    ];
    for (what, f, why) in cases {
        let mut w = World::new("basic");
        w.open();
        f(&mut w.data);
        let n = w.calls.len();
        let e = w.intent("canvas_send", json!({})).unwrap_err();
        assert!(e.contains(why), "{what}: {e}");
        let e = w.intent("canvas_ask", json!({})).unwrap_err();
        assert!(e.contains(why), "{what} (ask): {e}");
        assert_eq!(w.calls.len(), n, "{what}: nothing went");
        // The button says why too.
        let a = w.intent("canvas_tool", json!({ "tool": "select" })).unwrap();
        w.run(a);
        assert_eq!(w.ui()["send"]["ok"], false, "{what}");
    }
}

#[test]
fn ask_for_screenshots_is_one_dialog_with_the_prompt_in_full() {
    let mut w = World::new("empty");
    let a = w.intent("canvas_show", json!({})).unwrap();
    w.run(a);
    assert_eq!(w.ui()["empty"], "none");
    let a = w.intent("canvas_ask_text", json!({ "text": "Shots of the login page, please." })).unwrap();
    w.run(a);
    let a = w.intent("canvas_ask", json!({})).unwrap();
    let c = w.confirm(&a);
    assert_eq!(c.preview.as_ref().unwrap().text, "Shots of the login page, please.");
    assert!(w.pushed().is_empty());
    w.press(&c);
    assert_eq!(w.pushed(), ["Shots of the login page, please."]);
}

#[test]
fn delete_goes_through_one_dialog_naming_what_goes() {
    let mut w = World::new("basic");
    // The canvas shown was changed today; the two others long ago.
    w.daemon["canvas"]["canvases"]["c-00000000000000c1"]["updated_ms"] = (crate::feat::files::view::now_secs() * 1000).into();
    w.open();
    let a = w.intent("canvas_delete", json!({})).unwrap();
    let c = w.confirm(&a);
    assert!(c.danger && c.title.contains("Checkout review"));
    assert!(c.lines[0].contains("11 shot(s)"), "{:?}", c.lines);
    assert_eq!(c.call.body["ids"], json!(["c-00000000000000c1"]));
    // Older than 30 days: the two old ones, named, with their size.
    let a = w.intent("canvas_delete", json!({ "older": "30" })).unwrap();
    let c = w.confirm(&a);
    assert!(c.title.contains("2 canvas(es)"), "{}", c.title);
    assert!(c.lines.iter().any(|l| l.contains("Settings pages")) && c.lines.iter().any(|l| l.contains("old-shop")));
    w.press(&c);
    assert_eq!(w.daemon["canvas"]["canvases"].as_object().unwrap().len(), 1);
}

#[test]
fn closing_the_view_gives_the_pictures_back() {
    let mut w = World::new("big");
    w.open();
    w.decoded();
    let (bytes, n) = crate::surface_canvas::mem();
    assert_eq!(n, 15, "15 shots shown");
    assert!(bytes > 0 && bytes < 64 << 20, "{bytes}");
    // The overlay closed: the next data push sees it.
    w.data["app"]["overlay"] = J::Null;
    let _ = extras(&w.cx());
    assert_eq!(crate::surface_canvas::mem(), (0, 0));
}

fn walk<'a>(n: &'a Node, out: &mut Vec<&'a Node>) {
    out.push(n);
    for c in &n.children {
        walk(c, out);
    }
}

#[test]
fn hostile_agent_text_and_file_names_make_no_control() {
    let mut w = World::new("basic");
    let evil = "Home\u{1b}]8;;https://x\u{7}\u{202e}gnp.exe\n\"; return intent('canvas_delete')";
    w.daemon["canvas"]["canvases"]["c-00000000000000c1"]["title"] = evil.into();
    w.daemon["canvas"]["canvases"]["c-00000000000000c1"]["shots"][0]["title"] = evil.into();
    w.daemon["canvas"]["canvases"]["c-00000000000000c1"]["assets"][0]["name"] = evil.into();
    w.open();
    let ui = w.ui().clone();
    let all = ui.to_string();
    assert!(!all.contains('\u{1b}') && !all.contains('\u{202e}'), "no control or direction mark reaches the screen");
    let mut h = LuauHost::new(Limits::default()).unwrap();
    h.load("canvas", include_str!("../../../screens/canvas.luau")).unwrap();
    let n = h.render(&w.data).unwrap();
    let mut nodes = vec![];
    walk(&n, &mut nodes);
    for x in nodes.iter().filter(|x| x.on.is_some()) {
        let k = x.key.clone().unwrap_or_default();
        assert!(!k.contains("Home") && !k.contains("exe"), "a control keyed by agent text: {k}");
        assert!(!x.arg.to_string().contains("exe"), "a control's argument from agent text: {}", x.arg);
    }
    // Shot captions on the surface are text.
    let scene = crate::surface_canvas::scene(SURF);
    let cap = scene.items.iter().find_map(|i| match &i.body { crate::surface_canvas::Body::Picture { caption, asset: false, .. } => Some(caption[0].clone()), _ => None }).unwrap();
    assert!(!cap.contains('\u{1b}') && !cap.contains('\n'));
}

// ---------------------------------------------------------------------------
// The window

mod window {
    use crate::shell::{view, App, Args};
    use rune::{KeyEvent, Tx};

    pub fn runtime(w: f32, h: f32) -> rune::Runtime<App> {
        runtime_of("basic", w, h)
    }

    pub fn runtime_of(fixture: &str, w: f32, h: f32) -> rune::Runtime<App> {
        // A name under fixtures/canvas, or a path (a fixture a test changed).
        let f = if fixture.contains('/') { fixture.to_string() } else { format!("{}/fixtures/canvas/{fixture}.json", env!("CARGO_MANIFEST_DIR")) };
        let a = App::new(Args { fixture: Some(f.into()), ..Args::default() });
        let mut rt = rune::Runtime::new(a, view, (w, h), rune::Env::new(true, 1.0));
        rt.on_key = Some(|s: &mut App, ev: &KeyEvent, tx: &mut Tx| s.on_key(ev, tx));
        settle(&mut rt, 6);
        rt
    }

    pub fn settle(rt: &mut rune::Runtime<App>, frames: usize) {
        for _ in 0..frames {
            rt.frame(0.05);
            rt.update(|s, tx| {
                s.poll(0.0, tx);
            });
        }
    }

    pub fn tap(rt: &mut rune::Runtime<App>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} drawn"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        settle(rt, 4);
    }

    /// An intent as a press runs it (checked as any).
    pub fn intent(rt: &mut rune::Runtime<App>, name: &str) {
        let list = crate::intent::parse(&serde_json::json!({ "kind": "intent", "name": name, "args": {} })).unwrap();
        rt.update(|s, _| {
            for i in list {
                s.run_intent("test", i);
            }
        });
        settle(rt, 4);
    }

    pub fn rect(rt: &rune::Runtime<App>, key: &str) -> Option<[f32; 4]> {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).map(|e| e.rect)
    }

    /// Opened from the rail, the canvas read and laid out.
    pub fn opened(w: f32, h: f32) -> rune::Runtime<App> {
        opened_of("basic", "s-1-0", w, h)
    }

    /// [`opened`] on another fixture, until shot `first` is drawn.
    pub fn opened_of(fixture: &str, first: &str, w: f32, h: f32) -> rune::Runtime<App> {
        *super::STATE().lock().unwrap() = super::State::default();
        crate::surface_canvas::close(super::SURF);
        let mut rt = runtime_of(fixture, w, h);
        intent(&mut rt, "canvas_show");
        let key = format!("canvas-{first}");
        for _ in 0..100 {
            settle(&mut rt, 2);
            if rect(&rt, &key).is_some() {
                break;
            }
        }
        assert!(rect(&rt, &key).is_some(), "the shots are drawn");
        rt
    }
}

#[test]
fn under_a_dialog_the_canvas_neither_pans_nor_zooms_and_nothing_is_drawn_outside_the_overlay() {
    for (w, h) in [(1284.0, 816.0), (720.0, 480.0)] {
        let mut rt = window::opened(w, h);
        // The audit: every element the view draws (its visible part, what the
        // surface's clip leaves) inside the overlay's box.
        let ov = window::rect(&rt, "overlay-canvas").expect("the overlay");
        for e in rt.elements() {
            let k = e.key.clone().unwrap_or_default();
            if !k.starts_with("canvas") {
                continue;
            }
            let r = e.visible;
            if r[2] <= 0.5 || r[3] <= 0.5 {
                continue;
            }
            assert!(r[0] >= ov[0] - 0.5 && r[1] >= ov[1] - 0.5 && r[0] + r[2] <= ov[0] + ov[2] + 0.5 && r[1] + r[3] <= ov[1] + ov[3] + 0.5, "{w}x{h}: {k} {r:?} outside {ov:?}");
        }
        // A pan by the wheel moves the camera...
        let c = window::rect(&rt, "canvas-canvas").unwrap();
        let (x, y) = (c[0] + c[2] / 2.0, c[1] + c[3] / 2.0);
        let before = crate::surface_canvas::camera(super::SURF);
        rt.wheel_event(rune::WheelEvent::new(-30.0, -20.0).at(x, y).precise(true));
        window::settle(&mut rt, 2);
        let panned = crate::surface_canvas::camera(super::SURF);
        assert_ne!(before.pan, panned.pan, "the wheel pans");
        // ...not under the core's dialog (Delete this canvas).
        window::intent(&mut rt, "canvas_delete");
        assert!(rt.elements().iter().any(|e| e.role == "dialog"), "the dialog is up");
        rt.wheel_event(rune::WheelEvent::new(-30.0, -20.0).at(x, y).precise(true));
        rt.magnify_event(rune::MagnifyEvent::new(0.5).at(x, y));
        rt.pointer_down(x, y);
        rt.pointer_move(x - 60.0, y - 40.0);
        rt.pointer_up(x - 60.0, y - 40.0);
        window::settle(&mut rt, 2);
        assert_eq!(crate::surface_canvas::camera(super::SURF), panned, "{w}x{h}: nothing moved under the dialog");
    }
}

// ---------------------------------------------------------------------------
// Who the Ask and the send go to: one verdict for the button and the press.

/// An empty canvas: the Ask button's reason is its own (not the send bar's
/// "No new pin"), and every refusal reads the same on the button and on the press.
#[test]
fn ask_says_its_own_reason_and_the_button_and_the_press_agree() {
    let mut w = World::new("empty");
    let a = w.intent("canvas_show", json!({})).unwrap();
    w.run(a);
    assert_eq!(w.ui()["send"]["ask_ok"], true, "{}", w.ui()["send"]);
    assert!(w.ui()["send"]["why"].as_str().unwrap().contains("No new pin"), "the send bar's reason stays the send bar's");
    assert!(w.ui()["send"]["ask_why"].is_null());
    let cases: Vec<(&str, Box<dyn Fn(&mut J)>, &str)> = vec![
        ("jarvis", Box::new(|d: &mut J| d["jarvis"]["pending"] = json!([{ "id": "j1" }])), "A Jarvis proposal waits"),
        ("covered", Box::new(|d: &mut J| d["app"]["covered_top"] = true.into()), "open over the canvas"),
        ("question", Box::new(|d: &mut J| d["sessions"][0]["state"] = "attention".into()), "waits on a question or a permission"),
        ("unanswerable", Box::new(|d: &mut J| d["sessions"][0]["answerable"] = false.into()), "cannot tell which terminal"),
        ("several", Box::new(|d: &mut J| {
            d["sessions"][0]["answerable"] = false.into();
            d["sessions"][0]["unanswerable"] = json!({ "reason": "several_sessions", "sessions": 3, "tty": 3, "hook_version": 3 });
        }), "3 Claude sessions are open in"),
        ("no terminal", Box::new(|d: &mut J| {
            d["sessions"][0]["answerable"] = false.into();
            d["sessions"][0]["unanswerable"] = json!({ "reason": "no_terminal", "sessions": 1, "tty": 1, "hook_version": 3 });
        }), "sbxw holds no terminal for this session"),
        ("old hook", Box::new(|d: &mut J| {
            d["sessions"][0]["answerable"] = false.into();
            d["sessions"][0]["unanswerable"] = json!({ "reason": "old_hook", "sessions": 2, "tty": 2, "hook_version": 2 });
        }), "run sbxw up"),
        ("background", Box::new(|d: &mut J| {
            d["sessions"][0]["kind"] = "background".into();
            d["sessions"][0]["answerable"] = false.into();
        }), "background session of Claude Code"),
        ("stopped", Box::new(|d: &mut J| d["sandboxes"][0]["status"] = "stopped".into()), "is not running"),
        ("none", Box::new(|d: &mut J| d["sessions"] = json!([])), "no open agent session"),
    ];
    for (what, f, why) in cases {
        let mut w = World::new("empty");
        let a = w.intent("canvas_show", json!({})).unwrap();
        w.run(a);
        f(&mut w.data);
        let a = w.intent("canvas_tool", json!({ "tool": "select" })).unwrap();
        w.run(a);
        let shown = w.ui()["send"]["ask_why"].as_str().unwrap_or_else(|| panic!("{what}: no reason shown")).to_string();
        assert!(shown.contains(why), "{what}: {shown}");
        assert_eq!(w.ui()["send"]["ask_ok"], false, "{what}");
        let e = w.intent("canvas_ask", json!({})).unwrap_err();
        // The press says what the button said (the held checks are the
        // press's own first words, the same rule).
        if what == "jarvis" || what == "covered" {
            assert!(e.contains(if what == "jarvis" { "Jarvis proposal" } else { "open over the canvas" }), "{what}: {e}");
        } else {
            assert_eq!(e, format!("Not asked: {shown}"), "{what}");
        }
        // A session's own refusal names who can be asked instead.
        if what == "question" || what == "unanswerable" {
            assert!(shown.contains("Or pick web-app · checkout-ui instead"), "{what}: {shown}");
            let a = w.intent("canvas_session", json!({ "key": "web-app::checkout-ui" })).unwrap();
            w.run(a);
            assert_eq!(w.ui()["send"]["ask_ok"], true, "{what}: the thread picked can be asked");
            assert!(w.intent("canvas_ask", json!({})).is_ok());
        }
    }
}

/// A sandbox's main checkout with an old session waiting on a question and
/// the live one working: the live one is asked; the label and the push are
/// that session's.
#[test]
fn a_stale_question_beside_the_live_main_session_does_not_hold_the_ask() {
    let mut w = World::new("empty");
    let a = w.intent("canvas_show", json!({})).unwrap();
    w.run(a);
    w.data["sessions"] = json!([
        { "sandbox": "web-app", "mode": "claude", "state": "attention", "started_ms": 1, "ts": 100, "answerable": false, "session_id": "old-aaaa" },
        { "sandbox": "web-app", "mode": "claude", "state": "working", "started_ms": 2, "ts": 200, "answerable": true, "session_id": "live-bbbb" },
    ]);
    let a = w.intent("canvas_tool", json!({ "tool": "select" })).unwrap();
    w.run(a);
    let s = w.ui()["send"].clone();
    assert_eq!(s["ask_ok"], true, "{s}");
    assert_eq!(s["sessions"].as_array().unwrap().len(), 2, "two live sessions in one checkout: two to pick from");
    assert_eq!(s["to"], "web-app · main · bbbb", "{s}");
    let a = w.intent("canvas_ask", json!({})).unwrap();
    let c = w.confirm(&a);
    w.press(&c);
    let push = w.calls.iter().find(|c| c.path == "/api/chat/push").unwrap();
    assert_eq!(push.body["name"], "web-app");
    assert!(push.body["thread"].is_null());
}

/// The scope picker in the title: the capsule names the sandbox, the overlay
/// takes its frame, a pick reads the list again and shows that sandbox's
/// canvas, a pre-scoped open selects its sandbox; on the empty view a
/// disabled Ask says why on screen and the session picker is there.
#[test]
fn the_title_carries_the_scope_capsule_and_a_disabled_ask_says_why_on_screen() {
    let mut rt = window::opened(1284.0, 816.0);
    let label = |rt: &rune::Runtime<crate::shell::App>| rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("canvas-sandbox")).and_then(|e| e.label).unwrap_or_default();
    assert_eq!(label(&rt), "Scope: web-app (running)");
    assert!(window::rect(&rt, "overlay-strip-canvas").is_some(), "the overlay's coloured frame");
    // A pick: the list read again, the other sandbox's canvas (none: empty view).
    window::tap(&mut rt, "canvas-sandbox");
    window::tap(&mut rt, "canvas-sandbox-docs");
    window::settle(&mut rt, 8);
    assert_eq!(STATE().lock().unwrap().sandbox.as_deref(), Some("docs"));
    assert!(label(&rt).starts_with("Scope: docs"), "{}", label(&rt));
    assert!(window::rect(&rt, "canvas-empty").is_some(), "docs has no canvas");
    // docs has no session: the Ask is disabled and says why, visibly.
    let why = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("canvas-ask-why")).expect("the reason on screen");
    assert!(why.visible[2] > 1.0 && why.visible[3] > 1.0, "visible: {:?}", why.visible);
    // A pre-scoped open (a pane's menu, Sessions) selects its sandbox.
    let list = crate::intent::parse(&json!({ "kind": "intent", "name": "canvas_show", "args": { "sandbox": "web-app" } })).unwrap();
    rt.update(|s, _| list.into_iter().for_each(|i| s.run_intent("test", i)));
    window::settle(&mut rt, 8);
    assert_eq!(STATE().lock().unwrap().sandbox.as_deref(), Some("web-app"), "the open's sandbox");
    assert!(window::rect(&rt, "overlay-canvas").is_some(), "still open");
    for _ in 0..50 {
        if label(&rt) == "Scope: web-app (running)" {
            break;
        }
        window::settle(&mut rt, 2);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(label(&rt), "Scope: web-app (running)");
}

/// The title row at 1284 and 720: the capsule, the canvas menu, the storage
/// line, Rename and Delete side by side, none over another, all in the overlay.
#[test]
fn the_title_row_does_not_overlap_at_1284_or_720() {
    for (w, h) in [(1284.0, 816.0), (720.0, 480.0)] {
        let rt = window::opened(w, h);
        let ov = window::rect(&rt, "overlay-canvas").expect("the overlay");
        let keys = ["canvas-sandbox", "canvas-pick", "canvas-storage", "canvas-rename-open", "canvas-delete"];
        let rects: Vec<(&str, [f32; 4])> = keys.iter().filter_map(|k| window::rect(&rt, k).map(|r| (*k, r))).collect();
        assert!(rects.iter().any(|(k, _)| *k == "canvas-sandbox") && rects.iter().any(|(k, _)| *k == "canvas-pick"), "{w}: {rects:?}");
        for (i, (a, r)) in rects.iter().enumerate() {
            assert!(r[0] >= ov[0] - 0.5 && r[0] + r[2] <= ov[0] + ov[2] + 0.5, "{w}: {a} {r:?} leaves {ov:?}");
            for (b, q) in &rects[i + 1..] {
                let apart = r[0] + r[2] <= q[0] + 0.5 || q[0] + q[2] <= r[0] + 0.5 || r[1] + r[3] <= q[1] + 0.5 || q[1] + q[3] <= r[1] + 0.5;
                assert!(apart, "{w}: {a} {r:?} overlaps {b} {q:?}");
            }
        }
    }
}

/// Canvas drawn over the Sessions base view (GUI): the base is covered
/// (`covered`), Canvas is not (`covered_top`): its Ask and Send are its
/// own to refuse; a dialog, a modal, a proposal or the palette (all
/// `covered_top`) still holds them.
#[test]
fn over_the_sessions_view_the_canvas_is_not_covered() {
    let mut w = World::new("basic");
    w.open();
    w.data["app"]["covered"] = true.into();
    w.data["app"]["base"] = "monitor".into();
    let a = w.intent("canvas_ask_text", json!({ "text": "Shots of the login page, please." })).unwrap();
    w.run(a);
    let a = w.intent("canvas_ask", json!({})).unwrap();
    let c = w.confirm(&a);
    assert_eq!(c.preview.as_ref().unwrap().text, "Shots of the login page, please.");
    assert!(w.intent("canvas_send", json!({})).map_or_else(|e| !e.contains("open over the canvas"), |_| true), "Send is not held by the base under it");
    w.data["app"]["covered_top"] = true.into();
    let e = w.intent("canvas_ask", json!({})).unwrap_err();
    assert!(e.contains("open over the canvas"), "{e}");
}

/// The window in GUI (Sessions as the base), Canvas opened over it: Ask
/// gives the core's Confirm; the data says the base is covered and the top
/// is not; under the palette Ask is refused.
#[test]
fn in_gui_mode_canvas_ask_reaches_its_confirm() {
    *STATE().lock().unwrap() = State::default();
    crate::surface_canvas::close(SURF);
    let mut rt = window::runtime(1284.0, 816.0);
    window::tap(&mut rt, "top-view-gui");
    assert_eq!(rt.state.feats.base.as_deref(), Some("monitor"));
    window::intent(&mut rt, "canvas_show");
    window::settle(&mut rt, 6);
    let app = rt.state.screen_data()["app"].clone();
    assert_eq!((app["covered"].clone(), app["covered_top"].clone()), (json!(true), json!(false)), "{app}");
    let run = |rt: &mut rune::Runtime<crate::shell::App>, name: &str, args: J| {
        let list = crate::intent::parse(&json!({ "kind": "intent", "name": name, "args": args })).unwrap();
        rt.update(move |s, _| {
            for i in list {
                s.run_intent("canvas", i);
            }
        });
        window::settle(rt, 6);
    };
    run(&mut rt, "canvas_ask_text", json!({ "text": "Shots of the login page, please." }));
    rt.update(|s, _| s.palette_open());
    run(&mut rt, "canvas_ask", json!({}));
    assert!(rt.state.feats.dialog.is_none(), "refused under the palette");
    rt.update(|s, _| s.palette_close());
    run(&mut rt, "canvas_ask", json!({}));
    let d = rt.state.feats.dialog.as_ref().expect("the core's Confirm");
    assert!(d.error.is_none(), "{:?}", d.error);
    assert!(d.confirm.as_ref().is_some_and(|c| c.preview.as_ref().is_some_and(|p| p.text == "Shots of the login page, please.")), "the prompt in full");
}

/// Canvas's sandbox picker is the scope list: open it, narrow it by its
/// filter (the list is bounded and scrolls, a row below its fold is not
/// pressable), press web-app's row: its canvas's surface is drawn.
#[test]
fn picking_a_sandbox_in_the_scope_list_shows_its_canvas() {
    *STATE().lock().unwrap() = State::default();
    crate::surface_canvas::close(SURF);
    let mut rt = window::runtime(1284.0, 816.0);
    window::tap(&mut rt, "rail-canvas");
    window::tap(&mut rt, "canvas-sandbox");
    assert!(rt.find("canvas-sandbox-list").is_some(), "the scope list is open");
    for c in "web-app".chars() {
        rt.key(rune::KeyEvent { key: rune::Key::Text(c.to_string()), mods: rune::Mods::default() });
        rt.frame(0.02);
    }
    window::settle(&mut rt, 2);
    let row = window::rect(&rt, "canvas-sandbox-web-app").expect("web-app's row");
    let list = window::rect(&rt, "canvas-sandbox-list").expect("the list");
    assert!(row[1] >= list[1] - 0.5 && row[1] + row[3] <= list[1] + list[3] + 0.5, "the row is inside the list: {row:?} {list:?}");
    window::tap(&mut rt, "canvas-sandbox-web-app");
    for _ in 0..100 {
        window::settle(&mut rt, 2);
        if window::rect(&rt, "canvas-s-1-0").is_some() {
            break;
        }
    }
    let keys: Vec<String> = rt.elements().iter().filter_map(|e| e.key.clone()).filter(|k| k.starts_with("canvas")).take(40).collect();
    assert!(window::rect(&rt, "canvas-s-1-0").is_some(), "web-app's canvas is drawn: {keys:?} ui {}", rt.state.screen_data()["canvas"]);
}

// ---------------------------------------------------------------------------
// Rounds as a grid: fit, the handle, the Add example tool

/// A round of `n` shots 1284 x 816 laid out as the core lays it (4 x 3 for 12...).
fn grid_scene(n: usize) -> crate::surface_canvas::Scene {
    use crate::surface_canvas::{Body, Item, Scene};
    let cols = super::model::grid_cols(n, 1284.0, 816.0);
    let mut items = vec![Item { id: "band-1".into(), x: 0.0, y: 0.0, w: 1284.0, h: 0.0, body: Body::Band { label: "Round 1".into() }, movable: false }];
    for k in 0..n {
        let (c, r) = ((k % cols) as f32, (k / cols) as f32);
        items.push(Item { id: format!("s-g-{k}"), x: c * 1404.0, y: r * 1016.0, w: 1284.0, h: 816.0, body: Body::Picture { image: format!("none-{k}"), caption: vec![format!("Screen {k}")], asset: false }, movable: true });
    }
    Scene { items, ..Scene::default() }
}

#[test]
fn the_stand_ins_columns_are_the_cores() {
    // The same table as crates/core/src/canvas.rs (a_round_is_a_grid_whose_columns_follow_its_shots).
    for (n, cols) in [(1, 1), (2, 2), (4, 2), (9, 3), (12, 4), (16, 4), (24, 5)] {
        assert_eq!(super::model::grid_cols(n, 1284.0, 816.0), cols, "{n}");
    }
}

/// Fit fills the surface with the content (the side list open where the
/// window is wide), with room for the band above and the captions below.
#[test]
fn fit_fills_the_surface_at_every_size_and_count() {
    use crate::surface_canvas::{camera, set_scene, zoom, ZoomCmd, FIT_PAD};
    for (w, h) in [(1284.0, 816.0), (720.0, 480.0), (1900.0, 950.0)] {
        let mut rt = window::opened_of("grid12", "s-1-80", w, h);
        if w >= 1000.0 {
            assert!(window::rect(&rt, "canvas-sendbar").is_some(), "{w}: the side list is open beside the surface");
        }
        let view = window::rect(&rt, "canvas-canvas").unwrap();
        for n in [1, 4, 12, 24] {
            set_scene(super::SURF, grid_scene(n));
            zoom(super::SURF, ZoomCmd::Fit);
            window::settle(&mut rt, 2);
            let cam = camera(super::SURF);
            assert_eq!(cam.view, [view[2], view[3]], "{w}x{h}");
            let cols = super::model::grid_cols(n, 1284.0, 816.0) as f32;
            let rows = (n as f32 / cols).ceil();
            let (bw, bh) = (cols * 1284.0 + (cols - 1.0) * 120.0, rows * 816.0 + (rows - 1.0) * 200.0);
            let (aw, ah) = (cam.view[0] - FIT_PAD[1] - FIT_PAD[3], cam.view[1] - FIT_PAD[0] - FIT_PAD[2]);
            let (dw, dh) = (bw * cam.zoom, bh * cam.zoom);
            assert!(dw <= aw + 0.5 && dh <= ah + 0.5, "{w}x{h}, {n} shots: {dw}x{dh} in {aw}x{ah}");
            assert!(dw >= aw - 0.5 || dh >= ah - 0.5 || cam.zoom >= 1.0, "{w}x{h}, {n} shots: {dw}x{dh} does not fill {aw}x{ah}");
            // The block, its band and its captions inside the view.
            let tl = cam.screen([0.0, 0.0]);
            let br = cam.screen([bw, bh]);
            assert!(tl[1] >= FIT_PAD[0] - 0.5 && br[1] <= cam.view[1] - FIT_PAD[2] + 0.5 && tl[0] >= FIT_PAD[3] - 0.5 && br[0] <= cam.view[0] - FIT_PAD[1] + 0.5, "{w}x{h}, {n}: {tl:?} {br:?}");
            eprintln!("fit {w}x{h} (surface {:.0}x{:.0}), {n} shots: zoom {:.1}%, a shot {:.0} pt wide", view[2], view[3], cam.zoom * 100.0, 1284.0 * cam.zoom);
        }
    }
}

/// The user's round of 12 shots at Fit: a 4 x 3 grid. In one row a shot
/// was about 45 pt wide at 1284 x 816 and 90 at 1900 x 950; now about 126
/// and 270 (the surface beside the side list is 588 x 591 and 1204 x 725).
#[test]
fn a_round_of_twelve_reads_at_fit() {
    for (w, h, least) in [(1284.0, 816.0, 120.0), (1900.0, 950.0, 260.0)] {
        let rt = window::opened_of("grid12", "s-1-80", w, h);
        let first = window::rect(&rt, "canvas-s-1-80").unwrap();
        let fifth = window::rect(&rt, "canvas-s-1-84").unwrap();
        assert!(first[2] >= least, "{w}: a shot is {} pt wide", first[2]);
        assert!((fifth[0] - first[0]).abs() < 0.5 && fifth[1] > first[1] + first[3], "{w}: the fifth shot starts the second row");
        // Each shot on its mat, framed by a hairline.
        assert!(window::rect(&rt, "canvas-mat-s-1-80").is_some());
    }
}

#[test]
fn the_handle_is_a_small_share_of_the_item_and_hidden_when_it_is_small() {
    use crate::surface_canvas::{handle_side, HANDLE_MAX, HANDLE_SHARE};
    // The user's case: a 1284 x 816 shot at 9 %: no handle (it was a 14 pt square on a 115 pt shot).
    assert_eq!(handle_side(1284.0 * 0.09, 816.0 * 0.09), None);
    for z in [0.15, 0.3, 0.5, 1.0, 3.0] {
        let (dw, dh) = (1284.0 * z, 816.0 * z);
        let s = handle_side(dw, dh).unwrap();
        assert!(s <= HANDLE_MAX && s <= dh * HANDLE_SHARE + 0.01, "{z}: {s}");
    }
}

#[test]
fn a_selected_shot_at_low_zoom_has_a_ring_and_no_handle_and_at_100_a_small_handle_on_its_corner() {
    let mut rt = window::opened_of("grid12", "s-1-80", 1284.0, 816.0);
    crate::surface_canvas::zoom(super::SURF, crate::surface_canvas::ZoomCmd::Out);
    crate::surface_canvas::zoom(super::SURF, crate::surface_canvas::ZoomCmd::Out);
    crate::surface_canvas::zoom(super::SURF, crate::surface_canvas::ZoomCmd::Out);
    window::settle(&mut rt, 2);
    let r = window::rect(&rt, "canvas-s-1-84").unwrap();
    rt.pointer_down(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    rt.pointer_up(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    window::settle(&mut rt, 4);
    assert_eq!(STATE().lock().unwrap().selected.as_deref(), Some("s-1-84"));
    let r = window::rect(&rt, "canvas-s-1-84").unwrap();
    assert!(r[2] < 80.0, "drawn small: {r:?}");
    assert!(window::rect(&rt, "canvas-handle-s-1-84").is_none(), "no handle on a shot drawn {} pt wide", r[2]);
    let ring = window::rect(&rt, "canvas-ring-s-1-84").expect("the selection ring");
    assert!(ring[0] < r[0] && ring[1] < r[1] && ring[0] + ring[2] > r[0] + r[2] && ring[1] + ring[3] > r[1] + r[3], "the ring around the shot");
    // At 100 %: a handle, on the corner, at most 12 pt.
    crate::surface_canvas::zoom(super::SURF, crate::surface_canvas::ZoomCmd::Actual);
    crate::surface_canvas::show(super::SURF, [1284.0 - 300.0, 1016.0 + 816.0 - 200.0, 300.0, 200.0], 1.0);
    window::settle(&mut rt, 2);
    let r = window::rect(&rt, "canvas-s-1-84").unwrap();
    let hd = window::rect(&rt, "canvas-handle-s-1-84").expect("a handle at 100%");
    assert!(hd[2] <= crate::surface_canvas::HANDLE_MAX + 0.5, "{hd:?}");
    let (cx, cy) = (hd[0] + hd[2] / 2.0, hd[1] + hd[3] / 2.0);
    assert!((cx - (r[0] + r[2])).abs() < 1.0 && (cy - (r[1] + r[3])).abs() < 1.0, "on the corner: {hd:?} {r:?}");
}

#[test]
fn the_add_example_tool_says_what_it_does() {
    let mut w = World::new("basic");
    w.open();
    let mut h = LuauHost::new(Limits::default()).unwrap();
    h.load("canvas", include_str!("../../../screens/canvas.luau")).unwrap();
    let find = |n: &Node, key: &str| -> Option<Node> {
        let mut all = vec![];
        walk(n, &mut all);
        all.into_iter().find(|x| x.key.as_deref() == Some(key)).cloned()
    };
    let n = h.render(&w.data).unwrap();
    let b = find(&n, "canvas-tool-asset").expect("the tool");
    assert_eq!(b.text, "Add example");
    assert_eq!(b.icon.as_deref(), Some("image"));
    assert_eq!(b.help.as_deref(), Some("Add a picture of yours as an example for the agent: pick a file, drop one, or paste"));
    for (k, says) in [("canvas-tool-pin", "a numbered remark on a point of a shot"), ("canvas-tool-note", "a remark placed anywhere")] {
        let help = find(&n, k).unwrap().help.unwrap();
        assert!(help.contains(says), "{k}: {help}");
    }
    assert!(find(&n, "canvas-asset-hint").is_none(), "no hint until the tool is chosen");
    // Chosen (A): the hint above the surface, with its own Pick a file.
    let a = w.intent("canvas_key", json!({ "key": "type", "text": "a" })).unwrap();
    w.run(a);
    assert_eq!(w.ui()["tool"], "asset");
    let n = h.render(&w.data).unwrap();
    let hint = find(&n, "canvas-asset-hint").expect("the hint");
    let mut all = vec![];
    walk(&hint, &mut all);
    assert!(all.iter().any(|x| x.text.contains("as an example for the agent") && x.text.contains("drop one here, or paste it")), "the hint's words");
    assert_eq!(find(&n, "canvas-asset-pick").unwrap().on.as_deref(), Some("asset"));
    // The list and the send name it Example 1.
    let ui = w.ui().to_string();
    assert!(ui.contains("Example 1 · empty-cart-example.png") && !ui.contains("Asset 1"), "{ui}");
}

/// The title band at 1284, 950, 760 and 720 with a long canvas title: the
/// capsule and the canvas menu centred in the band under its strip, nothing
/// of the title row outside the band or over another; the storage line on
/// one line, in the band only on a wide overlay (else beside Rename and Delete).
#[test]
fn the_title_band_holds_its_row_at_every_width_with_a_long_title() {
    use crate::scope::{BAND_H, HEAD_TOP, STRIP_H};
    let long = "sbxw desktop: main screens after the second round of remarks";
    let mut fx: J = serde_json::from_str(&std::fs::read_to_string(format!("{}/fixtures/canvas/basic.json", env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap();
    fx["daemon"]["canvas"]["canvases"]["c-00000000000000c1"]["title"] = long.into();
    let path = std::env::temp_dir().join(format!("sbxw-canvas-long-title-{}.json", std::process::id()));
    std::fs::write(&path, fx.to_string()).unwrap();
    let mut bad = vec![];
    for (w, h) in [(1284.0, 816.0), (950.0, 700.0), (760.0, 560.0), (720.0, 480.0)] {
        let mut rt = window::opened_of(path.to_str().unwrap(), "s-1-0", w, h);
        // Motion settled (a control that moved slides to its place).
        window::settle(&mut rt, 30);
        let at = format!("{w}");
        let (o, _) = rt.presented("overlay-canvas").expect("the overlay");
        let band = rt.presented("overlay-wash-canvas").map(|(r, _)| r).unwrap_or([o[0], o[1], o[2], BAND_H]);
        let (top, bottom) = (band[1] + STRIP_H, band[1] + band[3]);
        let (cap, _) = rt.presented("canvas-sandbox").expect("the capsule");
        let (above, below) = (cap[1] - top, bottom - (cap[1] + cap[3]));
        if (above - below).abs() > 1.0 || (cap[1] - (o[1] + HEAD_TOP)).abs() > 1.0 {
            bad.push(format!("{at}: the capsule has {above} over it and {below} under it"));
        }
        let mid = cap[1] + cap[3] / 2.0;
        let mut row: Vec<(&str, [f32; 4])> = vec![("canvas-sandbox", cap)];
        for k in ["canvas-pick", "canvas-storage", "overlay-close"] {
            if let Some((r, _)) = rt.presented(k) {
                row.push((k, r));
            }
        }
        assert!(row.iter().any(|(k, _)| *k == "canvas-pick"), "{at}: the canvas menu");
        for (k, r) in &row {
            if r[1] < top - 0.5 || r[1] + r[3] > bottom + 0.5 || r[0] < o[0] - 0.5 || r[0] + r[2] > o[0] + o[2] + 0.5 {
                bad.push(format!("{at}: {k} {r:?} leaves the band {band:?}"));
            }
            if (r[1] + r[3] / 2.0 - mid).abs() > 1.0 {
                bad.push(format!("{at}: {k} {r:?} is off the row's centre {mid}"));
            }
        }
        for (i, (a, r)) in row.iter().enumerate() {
            for (b, q) in &row[i + 1..] {
                let apart = r[0] + r[2] <= q[0] + 0.5 || q[0] + q[2] <= r[0] + 0.5;
                if !apart {
                    bad.push(format!("{at}: {a} {r:?} overlaps {b} {q:?}"));
                }
            }
        }
        // The storage line: one line, in the band on a wide overlay, else under it.
        let line = rt.presented("canvas-storage").or_else(|| rt.presented("canvas-storage-line"));
        match line {
            Some((r, _)) if r[3] > 20.0 => bad.push(format!("{at}: the storage line wraps ({r:?})")),
            Some(_) => {}
            None => bad.push(format!("{at}: no storage line")),
        }
        if o[2] < 1100.0 && rt.presented("canvas-storage").is_some() {
            bad.push(format!("{at}: the storage line is in the title of a {} pt overlay", o[2]));
        }
        // The long title: cut, its full text in the menu's tooltip.
        let menu = rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("canvas-pick")).unwrap();
        if menu.help.as_deref() != Some(long) {
            bad.push(format!("{at}: the menu's tooltip is {:?}", menu.help));
        }
    }
    let _ = std::fs::remove_file(&path);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// The audit on the grid: a round of 12 at Fit, a shot selected, the Add
/// example hint up: everything the view draws inside the overlay, at 1284 and 720.
#[test]
fn the_grid_view_draws_nothing_outside_the_overlay() {
    for (w, h) in [(1284.0, 816.0), (720.0, 480.0)] {
        let mut rt = window::opened_of("grid12", "s-1-80", w, h);
        window::settle(&mut rt, 30);
        let r = window::rect(&rt, "canvas-s-1-85").unwrap();
        rt.pointer_down(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.pointer_up(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        window::settle(&mut rt, 4);
        window::tap(&mut rt, "canvas-tool-asset");
        window::settle(&mut rt, 20);
        assert!(window::rect(&rt, "canvas-asset-hint").is_some(), "{w}: the hint");
        assert!(window::rect(&rt, "canvas-ring-s-1-85").is_some(), "{w}: the ring (selected {:?}, shot {r:?})", STATE().lock().unwrap().selected);
        let ov = window::rect(&rt, "overlay-canvas").unwrap();
        for e in rt.elements() {
            let k = e.key.clone().unwrap_or_default();
            let r = e.visible;
            if !k.starts_with("canvas") || r[2] <= 0.5 || r[3] <= 0.5 {
                continue;
            }
            assert!(r[0] >= ov[0] - 0.5 && r[1] >= ov[1] - 0.5 && r[0] + r[2] <= ov[0] + ov[2] + 0.5 && r[1] + r[3] <= ov[1] + ov[3] + 0.5, "{w}x{h}: {k} {r:?} outside {ov:?}");
        }
    }
}

// ---------------------------------------------------------------------------
// The ground and its dots

/// White when nothing is kept, or something no ground is called; the kept
/// choice when the view opens; a choice from the bar is kept (Remember) and
/// reaches the surface's scene.
#[test]
fn the_ground_defaults_to_white_and_the_choice_is_remembered() {
    let mut w = World::new("basic");
    w.open();
    assert_eq!(w.ui()["ground"], "white");
    assert_eq!(crate::surface_canvas::scene(SURF).ground, Ground::White);
    let ids: Vec<&str> = w.ui()["grounds"].as_array().unwrap().iter().filter_map(|g| g["id"].as_str()).collect();
    assert_eq!(ids, ["white", "dark", "follow"], "three options");
    let checked: Vec<&str> = w.ui()["grounds"].as_array().unwrap().iter().filter(|g| g["icon"] == "check").filter_map(|g| g["id"].as_str()).collect();
    assert_eq!(checked, ["white"], "the current one marked");
    let acts = w.intent("canvas_ground", json!({ "ground": "dark" })).unwrap();
    assert!(acts.contains(&Action::Remember { key: "ground".into(), value: json!("dark") }), "{acts:?}");
    w.run(acts);
    assert_eq!(w.ui()["ground"], "dark");
    assert_eq!(crate::surface_canvas::scene(SURF).ground, Ground::Dark);
    assert!(crate::intent::parse(&json!({ "kind": "intent", "name": "canvas_ground", "args": { "ground": "pink" } })).is_err(), "a ground no choice has is refused");
    // Opened again in a later run: the kept choice.
    for (kept, want) in [(json!("follow"), Ground::Follow), (json!("dark"), Ground::Dark), (json!("sepia"), Ground::White), (J::Null, Ground::White)] {
        let mut w = World::new("basic");
        w.data["app"]["kept"] = json!({ "canvas": { "ground": kept } });
        w.open();
        assert_eq!(crate::surface_canvas::scene(SURF).ground, want, "{kept}");
        assert_eq!(w.ui()["ground"], want.name());
    }
    // Follow theme is the dark ground in every theme the app has.
    for m in crate::theme::Mode::ALL {
        assert!(!crate::theme::with_mode(m, || Ground::Follow.light()), "{m:?}");
    }
}

/// Each choice draws its ground under the items and the dot grid inside the
/// world; the captions take the ground's text colour; the choice is kept in
/// the state file's `features.canvas.ground` (in memory here).
#[test]
fn the_element_tree_has_the_ground_and_the_grid_for_each_choice() {
    use crate::theme::canvas_c;
    let mut rt = window::opened_of("grid12", "s-1-80", 1284.0, 816.0);
    window::settle(&mut rt, 4);
    let info = |rt: &rune::Runtime<crate::shell::App>, key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key));
    assert!(info(&rt, "canvas-ground").is_some(), "the ground's menu in the bar");
    for (g, light) in [("white", true), ("dark", false), ("follow", false), ("white", true)] {
        let list = crate::intent::parse(&json!({ "kind": "intent", "name": "canvas_ground", "args": { "ground": g } })).unwrap();
        rt.update(|s, _| list.into_iter().for_each(|i| s.run_intent("test", i)));
        // Long enough for the ground's fill to finish its transition.
        window::settle(&mut rt, 30);
        let ground = info(&rt, "canvas-canvas").expect("the surface");
        assert_eq!(ground.fill, canvas_c(light, "canvas-ground"), "{g}");
        let dots = info(&rt, "canvas-dots-canvas").unwrap_or_else(|| panic!("{g}: the dots"));
        let world = info(&rt, "canvas-world-canvas").unwrap();
        assert!(dots.path.starts_with(&world.path), "{g}: the dots move with the world: {} under {}", dots.path, world.path);
        // Drawn first in the world: under every item.
        let first = rt.elements().into_iter().filter(|e| e.path.starts_with(&world.path) && e.path != world.path).map(|e| e.key.unwrap_or_default()).next();
        assert_eq!(first.as_deref(), Some("canvas-dots-canvas"), "{g}");
        let cap = rt.elements().into_iter().find(|e| e.text.as_deref().is_some_and(|t| t == "Dashboard" || t == "Inbox") && e.path.starts_with(&world.path)).expect("a caption");
        assert_eq!(cap.text_color, Some(canvas_c(light, "canvas-text")), "{g}: a caption's title in the ground's text colour");
        let mut kept = J::Null;
        rt.update(|s, _| kept = s.store.get("features")["canvas"]["ground"].clone());
        assert_eq!(kept, g, "kept");
    }
}

// ---------------------------------------------------------------------------
// The canvas window (crate::windows): the main window and the canvas window
// as two runtimes, each run inside its turn scope as Rune runs them.

mod in_window {
    use super::window::{runtime, settle, tap};
    use super::*;
    use crate::screen::Host;
    use crate::shell::App;
    use crate::windows::{self as w, CanvasWin, Native, CANVAS};
    use rune::{Key, KeyEvent, Mods, Runtime, Tx};

    /// A turn of the main window: the screen (and a confirmation) back, then out.
    fn main<R>(rt: &mut Runtime<App>, f: impl FnOnce(&mut Runtime<App>) -> R) -> R {
        rt.update(|s, _| w::primary_enter(s));
        let r = f(rt);
        rt.update(|s, _| w::primary_leave(s));
        r
    }

    /// A turn of the canvas window.
    fn cwin<R>(ct: &mut Runtime<CanvasWin>, f: impl FnOnce(&mut Runtime<CanvasWin>) -> R) -> R {
        ct.update(|s, _| w::canvas_enter(s));
        let r = f(ct);
        ct.update(|s, _| w::canvas_leave(s));
        r
    }

    fn canvas_runtime() -> Runtime<CanvasWin> {
        let mut ct = Runtime::new(CanvasWin::default(), w::canvas_view, (1200.0, 800.0), rune::Env::new(true, 1.0));
        ct.on_key = Some(|s: &mut CanvasWin, ev: &KeyEvent, tx: &mut Tx| s.on_key(ev, tx));
        ct
    }

    fn ctap(ct: &mut Runtime<CanvasWin>, key: &str) {
        cwin(ct, |ct| {
            ct.frame(0.05);
            let (x, y) = ct.find(key).unwrap_or_else(|| panic!("{key} drawn in the canvas window"));
            ct.pointer_down(x, y);
            ct.pointer_up(x, y);
            ct.frame(0.05);
        });
    }

    fn cframe(ct: &mut Runtime<CanvasWin>) {
        cwin(ct, |ct| {
            ct.frame(0.05);
            ct.frame(0.05);
        });
    }

    fn key(k: Key) -> KeyEvent {
        KeyEvent { key: k, mods: Mods::default() }
    }

    /// Canvas over the panes, then in its own window from its bar's control.
    fn opened() -> (Runtime<App>, Runtime<CanvasWin>) {
        opened_on("basic")
    }

    /// [`opened`] on another fixture.
    pub(super) fn opened_on(fixture: &str) -> (Runtime<App>, Runtime<CanvasWin>) {
        *STATE().lock().unwrap() = State::default();
        crate::surface_canvas::close(SURF);
        w::reset();
        let mut rt = super::window::runtime_of(fixture, 1284.0, 816.0);
        main(&mut rt, |rt| tap(rt, "rail-canvas"));
        main(&mut rt, |rt| settle(rt, 6));
        main(&mut rt, |rt| tap(rt, "canvas-open-window"));
        assert!(w::canvas_open());
        assert_eq!(w::take_done(), vec![Native::Open(CANVAS)]);
        let mut ct = canvas_runtime();
        cframe(&mut ct);
        main(&mut rt, |rt| settle(rt, 4));
        cframe(&mut ct);
        (rt, ct)
    }

    #[test]
    fn open_in_a_window_moves_the_view_and_keeps_one_place() {
        let (mut rt, mut ct) = opened();
        // The main window's overlay says where it is, never a second live copy.
        main(&mut rt, |rt| {
            settle(rt, 2);
            assert!(rt.find("canvas-in-window").is_some(), "the overlay's placeholder");
            assert!(rt.find("canvas-tool-pin").is_none() && rt.find("canvas-surface-col").is_none(), "no second copy");
        });
        // The window draws the Canvas view, without the open control.
        cwin(&mut ct, |ct| {
            ct.frame(0.05);
            assert!(ct.state.has_screen());
            assert!(ct.find("canvas-tool-pin").is_some() && ct.find("cwin-back").is_some() && ct.find("cwin-full-screen").is_some());
            assert!(ct.find("canvas-open-window").is_none());
        });
        // A press there runs in the main window's feature, as from the overlay.
        ctap(&mut ct, "canvas-tool-pin");
        main(&mut rt, |rt| settle(rt, 2));
        assert_eq!(STATE().lock().unwrap().tool, Tool::Pin);
        // Asked again (the palette's shortcut, the placeholder): focused, not a second window.
        main(&mut rt, |rt| {
            let list = crate::intent::parse(&json!({ "kind": "intent", "name": "canvas_window", "args": { "to": "window" } })).unwrap();
            rt.update(|s, _| s.intents("canvas", list));
            settle(rt, 2);
        });
        assert_eq!(w::take_done(), vec![Native::Focus(CANVAS)]);
        main(&mut rt, |rt| tap(rt, "canvas-in-window-show"));
        assert_eq!(w::take_done(), vec![Native::Focus(CANVAS)]);
        // Its keys (P picks Pin's tool, as in the overlay) go to the feature too.
        cwin(&mut ct, |ct| {
            ct.key(key(Key::Escape));
            ct.frame(0.05);
        });
        main(&mut rt, |rt| settle(rt, 2));
        assert_eq!(STATE().lock().unwrap().tool, Tool::Select, "Escape left the tool");
    }

    #[test]
    fn closing_the_window_puts_the_canvas_back_over_the_panes() {
        let (mut rt, mut ct) = opened();
        ct.update(|s, tx| w::canvas_closed(s, tx));
        cframe(&mut ct);
        main(&mut rt, |rt| settle(rt, 4));
        assert!(!w::canvas_open());
        main(&mut rt, |rt| {
            settle(rt, 2);
            assert!(rt.find("canvas-in-window").is_none());
            assert!(rt.find("canvas-tool-pin").is_some(), "the live Canvas, over the panes");
            assert!(rt.find("canvas-open-window").is_some());
        });
        assert!(STATE().lock().unwrap().open, "the surface kept its canvas");
    }

    #[test]
    fn bring_it_back_from_the_main_window() {
        let (mut rt, mut ct) = opened();
        main(&mut rt, |rt| tap(rt, "canvas-in-window-back"));
        assert!(!w::canvas_open());
        assert_eq!(w::take_done(), vec![Native::Close(CANVAS)]);
        // Rune then runs its on_close: nothing more is said.
        ct.update(|s, tx| w::canvas_closed(s, tx));
        main(&mut rt, |rt| {
            settle(rt, 2);
            assert!(rt.find("canvas-tool-pin").is_some());
        });
    }

    /// Ask in the canvas window: the core's Confirm, drawn there; the main
    /// window says where it is, and no key or press there answers it.
    fn asked() -> (Runtime<App>, Runtime<CanvasWin>) {
        let (mut rt, mut ct) = opened();
        cwin(&mut ct, |ct| {
            for (name, args) in [("canvas_ask_text", json!({ "text": "Shots of the login page, please." })), ("canvas_ask", json!({}))] {
                let list = crate::intent::parse(&json!({ "kind": "intent", "name": name, "args": args })).unwrap();
                ct.update(|s, _| s.intents("canvas", list));
            }
        });
        main(&mut rt, |rt| settle(rt, 4));
        assert_eq!(w::with(|h| h.dialog_owner()), Some(CANVAS), "the Confirm belongs to the canvas window");
        cframe(&mut ct);
        (rt, ct)
    }

    #[test]
    fn a_confirmation_raised_in_the_canvas_window_is_drawn_there_alone() {
        let (mut rt, mut ct) = asked();
        cwin(&mut ct, |ct| {
            ct.frame(0.05);
            assert!(ct.find("cf-dialog").is_some() && ct.find("cf-confirm").is_some(), "the core's Confirm, in the canvas window");
            assert!(ct.find("cwin-gate").is_none());
            let d = ct.state.dialog().expect("held for this turn");
            assert!(d.confirm.as_ref().is_some_and(|c| c.preview.as_ref().is_some_and(|p| p.text == "Shots of the login page, please.")), "the prompt in full");
        });
        main(&mut rt, |rt| {
            settle(rt, 2);
            assert!(rt.find("cwin-elsewhere-scrim").is_some(), "the main window says where it is");
            assert!(rt.find("cf-dialog").is_none() && rt.find("cf-confirm").is_none(), "drawn in one window only");
        });
    }

    #[test]
    fn no_key_or_press_in_the_main_window_answers_the_canvas_windows_confirmation() {
        let (mut rt, mut ct) = asked();
        main(&mut rt, |rt| {
            for k in [Key::Escape, Key::Enter, Key::Text(" ".into()), Key::Tab] {
                rt.key(key(k));
                settle(rt, 1);
            }
            let (x, y) = rt.find("cwin-elsewhere-scrim").unwrap();
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
            settle(rt, 2);
            let d = rt.state.feats.dialog.as_ref().expect("still waiting");
            assert!(d.running.is_none(), "not confirmed");
        });
        assert_eq!(w::with(|h| h.dialog_owner()), Some(CANVAS));
        // Nor a forged answer from the main window.
        let id = cwin(&mut ct, |ct| ct.state.dialog().map(|d| d.identity()).unwrap());
        assert!(!w::with(|h| h.post(w::PRIMARY, w::ToPrimary::Answer { confirm: true, dialog: id })));
        main(&mut rt, |rt| assert!(rt.state.feats.dialog.as_ref().is_some_and(|d| d.running.is_none())));
    }

    #[test]
    fn the_canvas_windows_confirmation_is_answered_there() {
        // Cancel there: closed, nothing sent.
        let (mut rt, mut ct) = asked();
        ctap(&mut ct, "cf-cancel");
        main(&mut rt, |rt| settle(rt, 2));
        assert!(rt.state.feats.dialog.is_none() && STATE().lock().unwrap().job.is_none());
        cframe(&mut ct);
        cwin(&mut ct, |ct| {
            ct.frame(0.05);
            assert!(ct.find("cf-dialog").is_none() && ct.find("canvas-tool-pin").is_some());
        });
        // Confirm there: the main window's confirm_click runs it.
        let (mut rt, mut ct) = asked();
        ctap(&mut ct, "cf-confirm");
        main(&mut rt, |rt| settle(rt, 4));
        assert!(rt.state.feats.dialog.as_ref().is_none_or(|d| d.running.is_some() || d.error.is_some()), "confirmed");
    }

    #[test]
    fn closing_the_canvas_window_cancels_its_confirmation() {
        let (mut rt, mut ct) = asked();
        ct.update(|s, tx| w::canvas_closed(s, tx));
        cframe(&mut ct);
        main(&mut rt, |rt| settle(rt, 4));
        assert!(rt.state.feats.dialog.is_none(), "cancelled, never confirmed");
        assert!(!w::canvas_open());
    }

    /// A gate of the main window's own (here the palette) holds the canvas
    /// window: a scrim says where, and its presses and keys reach nothing.
    #[test]
    fn a_gate_in_the_main_window_holds_the_canvas_window() {
        let (mut rt, mut ct) = opened();
        main(&mut rt, |rt| {
            rt.update(|s, _| s.palette_open());
            settle(rt, 2);
        });
        cwin(&mut ct, |ct| {
            ct.frame(0.05);
            assert!(ct.find("cwin-gate").is_some() && ct.find("cwin-gate-show").is_some());
            assert!(ct.state.shown_gate().is_some_and(|g| g.owner == w::PRIMARY));
        });
        let refused = w::with(|h| h.refused);
        cwin(&mut ct, |ct| {
            let (x, y) = ct.find("canvas-tool-pin").unwrap();
            ct.pointer_down(x, y);
            ct.pointer_up(x, y);
            ct.key(key(Key::Enter));
            ct.key(key(Key::Text("p".into())));
            ct.frame(0.05);
        });
        main(&mut rt, |rt| settle(rt, 2));
        assert_eq!(STATE().lock().unwrap().tool, Tool::Select, "nothing reached the view");
        assert!(w::with(|h| h.refused) > refused);
        assert!(!w::with(|h| h.pending_for_primary()));
        ctap(&mut ct, "cwin-gate-show");
        assert_eq!(w::take_done(), vec![Native::FocusPrimary]);
        main(&mut rt, |rt| {
            rt.update(|s, _| s.palette_close());
            settle(rt, 2);
        });
        cwin(&mut ct, |ct| {
            ct.frame(0.05);
            assert!(ct.find("cwin-gate").is_none());
        });
    }

    /// A typed change (an input's per-keystroke action) stays a typed change
    /// across windows: the main window's `pressed` rule sees `editing`.
    #[test]
    fn a_typed_change_is_never_taken_for_a_press() {
        let (mut rt, mut ct) = opened();
        cwin(&mut ct, |ct| {
            let list = crate::intent::parse(&json!({ "kind": "intent", "name": "canvas_asset", "args": {} })).unwrap();
            ct.update(|s, _| {
                s.screen("canvas").ui.editing = true;
                s.intents("canvas", list);
                s.screen("canvas").ui.editing = false;
            });
        });
        main(&mut rt, |rt| {
            settle(rt, 2);
            let n = rt.state.notice.as_ref().map(|n| n.0.clone()).unwrap_or_default();
            assert!(n.contains("runs only from your own press"), "{n}");
        });
    }

    /// The canvas window draws a diagram and takes its gestures and keys as
    /// the overlay does: a node dragged, Tab for a successor, ⌘Z; saved
    /// through the main window's feature.
    #[test]
    fn a_diagram_is_edited_the_same_in_the_canvas_window() {
        use crate::surface_canvas::diagram as dg;
        let flow = super::diagram::FLOW;
        let (mut rt, mut ct) = opened_on("diagrams");
        // The workflow in view there, nothing selected.
        main(&mut rt, |rt| {
            for t in [flow, ""] {
                let list = crate::intent::parse(&json!({ "kind": "intent", "name": "canvas_select", "args": { "target": t } })).unwrap();
                rt.update(|s, _| list.into_iter().for_each(|i| s.run_intent("test", i)));
                settle(rt, 3);
            }
        });
        cframe(&mut ct);
        let node = format!("canvas-dg-{flow}-n-plan");
        let (x, y) = cwin(&mut ct, |ct| {
            ct.frame(0.05);
            ct.find(&node).unwrap_or_else(|| panic!("{node} drawn in the canvas window"))
        });
        cwin(&mut ct, |ct| {
            ct.pointer_down(x, y);
            ct.frame(0.02);
            for k in 1..=5 {
                ct.pointer_move(x + 12.0 * k as f32, y);
                ct.frame(0.02);
            }
            ct.pointer_up(x + 60.0, y);
            ct.frame(0.05);
        });
        main(&mut rt, |rt| settle(rt, 4));
        let moved = STATE().lock().unwrap().diagrams[flow].cur.node("plan").unwrap().clone();
        assert!(moved.x > 336.0 && moved.placed == dg::Placed::Person, "{moved:?}");
        assert_eq!(STATE().lock().unwrap().selected.as_deref(), Some(format!("{flow}:n:plan").as_str()));
        // Its keys: Tab adds a successor, ⌘Z takes it back.
        let n = STATE().lock().unwrap().diagrams[flow].cur.nodes.len();
        cwin(&mut ct, |ct| {
            ct.key(key(Key::Tab));
            ct.frame(0.05);
        });
        main(&mut rt, |rt| settle(rt, 4));
        assert_eq!(STATE().lock().unwrap().diagrams[flow].cur.nodes.len(), n + 1);
        cwin(&mut ct, |ct| {
            ct.key(key(Key::Escape));
            ct.frame(0.05);
        });
        main(&mut rt, |rt| settle(rt, 2));
        cwin(&mut ct, |ct| {
            ct.key(KeyEvent { key: Key::Command('z'), mods: Mods { cmd: true, ..Mods::default() } });
            ct.frame(0.05);
        });
        main(&mut rt, |rt| settle(rt, 6));
        assert_eq!(STATE().lock().unwrap().diagrams[flow].cur.nodes.len(), n);
        let st = STATE().lock().unwrap().diagrams[flow].clone();
        assert!(!st.saving && !st.dirty && st.rev >= 3, "saved through the main window: rev {}", st.rev);
    }
}

// ---------------------------------------------------------------------------
// Diagrams (surface_canvas::diagram): drawn, then edited.

#[path = "diagram_tests.rs"]
mod diagram;
