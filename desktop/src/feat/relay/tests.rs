use super::*;
use crate::feat::Standin;

fn fixture(name: &str) -> J {
    let raw = match name {
        "basic" => include_str!("../../../fixtures/relay/basic.json"),
        _ => include_str!("../../../fixtures/relay/answered.json"),
    };
    let v: J = serde_json::from_str(raw).unwrap();
    v["daemon"].clone()
}

fn ask(name: &'static str, args: J) -> Asked {
    Asked { name, args: args.as_object().unwrap().clone() }
}

fn run(st: &mut State, d: &J, name: &'static str, args: J) -> Result<Vec<Action>, String> {
    st.act(&Ctx { data: d }, &ask(name, args))
}

/// The core's rule (`shell::features::admit`): an approval answers with its dialog only.
fn admit(risk: Risk, name: &str, a: Vec<Action>) -> Result<(), String> {
    match (risk, a.as_slice()) {
        (Risk::Direct, _) | (Risk::Approval(_), [Action::Confirm(_)] | [Action::Prepare(_)]) => Ok(()),
        _ => Err(format!("{name}: not the core's dialog")),
    }
}

fn spec(name: &str) -> &'static Spec {
    INTENTS.iter().find(|s| s.name == name).unwrap()
}

/// What the screen sees, after the actions ran.
fn ui(acts: &[Action]) -> J {
    acts.iter().rev().find_map(|a| match a {
        Action::Set { key, value } if key == UI => Some(value.clone()),
        _ => None,
    }).expect("relay_ui set")
}

#[test]
fn routing_and_releasing_are_approvals_and_refusing_is_direct() {
    for s in INTENTS {
        let gated = matches!(s.name, "relay_route" | "relay_send");
        assert_eq!(matches!(s.risk, Risk::Approval(_)), gated, "{}", s.name);
        // No intent takes a file or an image.
        assert!(s.args.iter().all(|(k, _, _)| !matches!(*k, "path" | "paths" | "file" | "files" | "data" | "url")), "{}", s.name);
    }
}

#[test]
fn routing_shows_the_question_and_where_it_goes_and_checks_the_target() {
    let d = fixture("basic");
    let mut st = State::default();
    let a = run(&mut st, &d, "relay_route", json!({ "id": "r-1", "to": "api" })).unwrap();
    admit(spec("relay_route").risk, "relay_route", a.clone()).unwrap();
    let [Action::Confirm(c)] = a.as_slice() else { panic!("{a:?}") };
    assert_eq!(c.title, "Send web-app's question to api?");
    assert_eq!(c.call.path, "/api/relay/r-1/route");
    assert_eq!(c.call.body, json!({ "to": "api" }));
    assert!(c.preview.as_ref().unwrap().text.contains("GET /v1/orders"));
    assert!(c.lines.iter().any(|l| l.contains("nothing reaches web-app until you release it")), "{:?}", c.lines);
    // The asker, a stopped sandbox, an unknown one, an unknown request, a screenshot: refused.
    assert!(run(&mut st, &d, "relay_route", json!({ "id": "r-1", "to": "web-app" })).unwrap_err().contains("asked"));
    assert!(run(&mut st, &d, "relay_route", json!({ "id": "r-1", "to": "data-pipeline" })).unwrap_err().contains("not running"));
    assert!(run(&mut st, &d, "relay_route", json!({ "id": "r-1", "to": "nowhere" })).is_err());
    assert!(run(&mut st, &d, "relay_route", json!({ "id": "r-99", "to": "api" })).unwrap_err().contains("not in the daemon's list"));
    assert!(run(&mut st, &d, "relay_route", json!({ "id": "r-2", "to": "api" })).unwrap_err().contains("screenshot"));
    // A round goes for a draft.
    let a = run(&mut st, &d, "relay_route", json!({ "id": "r-3", "to": "api" })).unwrap();
    let [Action::Confirm(c)] = a.as_slice() else { panic!() };
    assert!(c.title.starts_with("Ask api to draft"), "{}", c.title);
    // Refusing: direct.
    let a = run(&mut st, &d, "relay_deny", json!({ "id": "r-1" })).unwrap();
    assert!(matches!(&a[0], Action::Call(c) if c.path == "/api/relay/r-1/deny"));
}

#[test]
fn the_answer_sent_is_the_one_typed_and_shown() {
    let d = fixture("basic");
    let mut st = State::default();
    assert!(run(&mut st, &d, "relay_send", json!({ "id": "r-1" })).unwrap_err().contains("Nothing to send"));
    run(&mut st, &d, "relay_set", json!({ "id": "r-1", "field": "own", "value": "It returns {orders, next}." })).unwrap();
    let a = run(&mut st, &d, "relay_send", json!({ "id": "r-1" })).unwrap();
    admit(spec("relay_send").risk, "relay_send", a.clone()).unwrap();
    let [Action::Confirm(c)] = a.as_slice() else { panic!() };
    assert_eq!(c.call.body, json!({ "answer": "It returns {orders, next}." }));
    assert_eq!(c.preview.as_ref().unwrap().text, "It returns {orders, next}.");
    assert!(c.lines.iter().any(|l| l == "Written by: you."));

    // A routed answer: as the other agent wrote it, or as edited.
    let d = fixture("answered");
    let mut st = State::default();
    let a = run(&mut st, &d, "relay_send", json!({ "id": "r-1" })).unwrap();
    let [Action::Confirm(c)] = a.as_slice() else { panic!() };
    assert!(c.call.body["answer"].as_str().unwrap().starts_with("It returns"));
    assert!(c.lines.iter().any(|l| l == "Written by: api."), "{:?}", c.lines);
    run(&mut st, &d, "relay_set", json!({ "id": "r-1", "field": "review", "value": "Trimmed: {orders, next}." })).unwrap();
    let a = run(&mut st, &d, "relay_send", json!({ "id": "r-1" })).unwrap();
    let [Action::Confirm(c)] = a.as_slice() else { panic!() };
    assert_eq!(c.call.body["answer"], "Trimmed: {orders, next}.");
    assert!(c.lines.iter().any(|l| l == "Written by: api, as you edited it."));
}

#[test]
fn a_rounds_answers_are_masked_before_the_dialog_shows_them() {
    let d = fixture("basic");
    let mut st = State::default();
    assert!(run(&mut st, &d, "relay_send", json!({ "id": "r-3" })).unwrap_err().contains("No question answered"));
    run(&mut st, &d, "relay_set", json!({ "id": "r-3", "field": "answer", "index": 2, "value": "ask Jeanne Martin" })).unwrap();
    let a = run(&mut st, &d, "relay_use_rec", json!({ "id": "r-3", "index": 1 })).unwrap();
    assert_eq!(ui(&a)["forms"]["r-3"], json!(["8080, as in staging.", "ask Jeanne Martin"]));
    let a = run(&mut st, &d, "relay_send", json!({ "id": "r-3" })).unwrap();
    admit(spec("relay_send").risk, "relay_send", a.clone()).unwrap();
    let [Action::Prepare(p)] = a.as_slice() else { panic!("{a:?}") };
    assert_eq!(p.call.path, "/api/voice/anonymise");
    assert_eq!(p.call.body["text"], "Q1 : 8080, as in staging.\nQ2 : ask Jeanne Martin");
    // The daemon's masking: the dialog shows and sends the masked text.
    let mut raw = d.clone();
    let (_, masked) = crate::feat::answer_from(&mut raw, "POST", "/api/voice/anonymise", &p.call.body, &mut |_, _| {});
    let c = (p.build)(&Ctx { data: &d }, &p.asked, &masked).unwrap();
    assert_eq!(c.call.body["answer"], "Q1 : 8080, as in staging.\nQ2 : ask Personne A");
    assert_eq!(c.preview.as_ref().unwrap().text, "Q1 : 8080, as in staging.\nQ2 : ask Personne A");
    assert!(c.lines.iter().any(|l| l.contains("1 piece of personal data masked")));
    // A masking that failed sends nothing.
    assert!((p.build)(&Ctx { data: &d }, &p.asked, &json!({ "ok": false, "error": "rules file" })).unwrap_err().contains("masking"));
}

#[test]
fn a_draft_fills_the_form_and_can_be_cleared() {
    let d = fixture("answered");
    let mut st = State::default();
    let a = run(&mut st, &d, "relay_open", json!({ "id": "r-3" })).unwrap();
    assert_eq!(ui(&a)["forms"]["r-3"], json!(["8081", "Keep the cookie"]));
    let a = run(&mut st, &d, "relay_clear", json!({ "id": "r-3" })).unwrap();
    assert_eq!(ui(&a)["forms"]["r-3"], json!(["", ""]));
}

#[test]
fn images_come_only_from_the_core_and_the_dialog_counts_them() {
    let d = fixture("basic");
    let mut st = State::default();
    // A path from a screen (or anyone else) is never read.
    for args in [json!({ "id": "r-2", "path": "/etc/passwd" }), json!({ "id": "r-2", "files": ["/home/me/.ssh/id_ed25519"] })] {
        assert!(run(&mut st, &d, "relay_shot_choose", args).unwrap_err().contains("never an argument"));
    }
    assert!(run(&mut st, &d, "relay_shot_paste", json!({ "id": "r-1" })).unwrap_err().contains("does not ask for a screenshot"));
    // The core hands images over; the screen sees labels only.
    let png = shots::tests::png(800, 600);
    let a = st.take_images(&Ctx { data: &d }, vec![(png.clone(), "from clipboard".into()), (png.clone(), "after.png".into()), (b"junk".to_vec(), "notes.txt".into())]);
    assert!(a.iter().any(|x| matches!(x, Action::Notice { error: true, text, .. } if text.contains("notes.txt"))));
    let u = ui(&a);
    assert_eq!(u["shots"]["r-2"].as_array().unwrap().len(), 2);
    assert!(!u.to_string().contains("iVBOR"), "no image bytes reach the screen");
    let a = run(&mut st, &d, "relay_send", json!({ "id": "r-2" })).unwrap();
    let [Action::Confirm(c)] = a.as_slice() else { panic!() };
    assert_eq!(c.title, "Send 2 images to web-app?");
    assert!(c.lines.iter().any(|l| l.starts_with("1. from clipboard · 800×600")), "{:?}", c.lines);
    assert!(c.lines.iter().any(|l| l.starts_with("Total:")));
    assert_eq!(c.call.body["images"].as_array().unwrap().len(), 2);
    assert!(c.call.body["images"][0].as_str().unwrap().starts_with("data:image/png;base64,"));
    run(&mut st, &d, "relay_shot_remove", json!({ "id": "r-2", "index": 1 })).unwrap();
    run(&mut st, &d, "relay_set", json!({ "id": "r-2", "field": "caption", "value": "the 375px breakpoint" })).unwrap();
    let a = run(&mut st, &d, "relay_send", json!({ "id": "r-2" })).unwrap();
    let [Action::Confirm(c)] = a.as_slice() else { panic!() };
    assert_eq!(c.title, "Send 1 image and your words to web-app?");
    assert_eq!(c.call.body["answer"], "the 375px breakpoint");
}

#[test]
fn an_agents_choice_is_sent_once_every_question_has_a_pick() {
    let d = fixture("basic");
    let mut st = State::default();
    let a = run(&mut st, &d, "agent_pick", json!({ "sandbox": "shop", "step": 1, "option": 2 })).unwrap();
    let [Action::Call(c), _] = a.as_slice() else { panic!("{a:?}") };
    assert_eq!(c.path, "/api/answer");
    assert_eq!(c.body, json!({ "sandbox": "shop", "mode": "claude", "index": 2, "indices": [2] }));
    // Two questions in a thread: nothing goes until both are picked.
    let a = run(&mut st, &d, "agent_pick", json!({ "sandbox": "api", "thread": "auth", "step": 1, "option": 1 })).unwrap();
    assert!(!a.iter().any(|x| matches!(x, Action::Call(_))));
    assert_eq!(ui(&a)["asks"].as_array().unwrap().iter().find(|x| x["key"] == "api::auth").unwrap()["picks"], json!([1, 0]));
    let a = run(&mut st, &d, "agent_pick", json!({ "sandbox": "api", "thread": "auth", "step": 2, "option": 2 })).unwrap();
    let Some(Action::Call(c)) = a.first() else { panic!() };
    assert_eq!(c.body, json!({ "sandbox": "api", "mode": "thread:auth", "index": 1, "indices": [1, 2] }));
    // Out of range, unknown session, Jarvis's own (its panel answers it): refused.
    assert!(run(&mut st, &d, "agent_pick", json!({ "sandbox": "shop", "step": 1, "option": 9 })).is_err());
    assert!(run(&mut st, &d, "agent_pick", json!({ "sandbox": "nowhere", "step": 1, "option": 1 })).is_err());
    assert!(run(&mut st, &d, "agent_pick", json!({ "sandbox": "sbxw-jarvis", "step": 1, "option": 1 })).is_err());
}

#[test]
fn a_new_request_is_a_notice_that_opens_the_panel() {
    let d = fixture("basic");
    let mut st = State::default();
    let one = json!({ "requests": [d["relay"]["requests"][0]] });
    let a = st.on_data(&Ctx { data: &d }, "relay", &json!({ "requests": [] }), &one);
    assert!(a.iter().any(|x| matches!(x, Action::Notice { text, then: Some((n, _)), .. } if text.starts_with("web-app asks: What shape") && n == "relay_open")), "{a:?}");
    assert!(a.contains(&Action::OpenPanel(SCREEN.into())));
    // Seen once only.
    assert!(!st.on_data(&Ctx { data: &d }, "relay", &one, &one).iter().any(|x| matches!(x, Action::Notice { .. })));
    let x = extras(&Ctx { data: &d });
    assert!(x.iter().any(|e| e.place == Place::Topbar && e.count == Some(5)), "{x:?}");
    assert!(x.iter().any(|e| e.place == Place::Sandbox("web-app".into()) && e.count == Some(3)));
}

#[test]
fn the_stand_in_follows_the_daemons_rules() {
    let mut d = fixture("basic");
    let mut events = vec![];
    let mut publish = |n: &str, v: J| events.push((n.to_string(), v));
    let mut st = Standin { data: &mut d, publish: &mut publish };
    assert_eq!(standin::route(&mut st, "POST", "/api/relay/r-2/route", &json!({ "to": "api" })).unwrap().0, 400);
    assert_eq!(standin::route(&mut st, "POST", "/api/relay/r-1/route", &json!({ "to": "web-app" })).unwrap().0, 400);
    assert_eq!(standin::route(&mut st, "POST", "/api/relay/r-1/route", &json!({ "to": "api" })).unwrap().0, 200);
    assert_eq!(standin::route(&mut st, "POST", "/api/relay/r-2/approve", &json!({})).unwrap().0, 400, "nothing to send");
    assert_eq!(standin::route(&mut st, "POST", "/api/relay/r-2/approve", &json!({ "images": ["data:image/png;base64,AAAA"] })).unwrap().0, 200);
    assert_eq!(standin::route(&mut st, "POST", "/api/relay/r-2/deny", &json!({})).unwrap().0, 400, "settled");
    assert_eq!(d["relay"]["requests"][0]["state"], "answered");
    assert_eq!(d["relay"]["requests"][1]["shot_count"], 1);
    assert!(events.iter().any(|(n, v)| n == "relay" && v["state"] == "routed"));
}
