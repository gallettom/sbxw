//! The approval dialogs' element trees, held against a snapshot taken
//! before their styling moved onto the theme's tokens: the same elements,
//! in the same nesting, with the same keys, roles, names, texts, Tab stops
//! and pressable controls. Only sizes, spacing and corners may differ.
//!
//! `SBXW_WRITE_APPROVAL_TREE=1` writes the snapshot again (only on purpose:
//! a change to what the dialogs show is an approval-path change).

use crate::approval::{self, ConfirmAnswers, ConfirmState, CopyOffer, DialogState, ProposalAnswers};
use crate::feat::{Call, Confirm, Grant, Preview, Secret};
use rune::{Cx, El, TextEdit};
use serde_json::{json, Value as J};

#[derive(Default)]
struct S {
    which: usize,
    proposal: J,
    confirm: Option<Confirm>,
    reason: Option<TextEdit>,
    secrets: Vec<TextEdit>,
    acked: bool,
    offer: Option<CopyOffer>,
}

impl ProposalAnswers for S {
    fn approve_click(&mut self) {}
    fn refuse_click(&mut self) {}
    fn toggle_ack(&mut self) {}
    fn close_dialog(&mut self) {}
    fn next_dialog(&mut self) {}
    fn reason_edit(&mut self) -> &mut TextEdit {
        self.reason.get_or_insert_with(|| TextEdit::new(String::new()))
    }
}

impl ConfirmAnswers for S {
    fn confirm_click(&mut self) {}
    fn cancel_click(&mut self) {}
    fn toggle_confirm_ack(&mut self) {}
    fn toggle_confirm_dont_ask(&mut self) {}
    fn secret_edit(&mut self, i: usize) -> &mut TextEdit {
        &mut self.secrets[i]
    }
}

fn view(s: &S, _cx: &mut Cx<S>) -> El {
    let empty = TextEdit::new(String::new());
    match s.which {
        0 | 1 => approval::proposal_view::<S>(&DialogState {
            proposal: &s.proposal,
            waiting: s.which,
            acked: s.acked,
            error: (s.which == 1).then_some("Changed since it was proposed"),
            busy: false,
            reason: s.reason.as_ref().unwrap_or(&empty),
            left: 64.0,
            top: 48.0,
            right: 16.0,
            max_h: 700.0,
        }),
        2 | 3 => approval::confirm_view::<S>(&ConfirmState {
            confirm: s.confirm.as_ref(),
            title: "Allow the network",
            loading: false,
            error: (s.which == 3).then_some("The daemon answered 409"),
            busy: s.which == 3,
            acked: s.acked,
            dont_ask: false,
            secrets: &s.secrets,
            left: 64.0,
            top: 48.0,
            right: 16.0,
            max_h: 700.0,
        }),
        _ => approval::copy_offer_view::<S>(s.offer.as_ref().unwrap(), |_: &mut S, _| {}, |_: &mut S| {}),
    }
}

fn proposal(fixture: &str) -> J {
    let path = format!("{}/fixtures/jarvis/{fixture}.json", env!("CARGO_MANIFEST_DIR"));
    let v: J = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    v["daemon"]["jarvis_proposals"][0].clone()
}

fn confirm() -> Confirm {
    let mut c = Confirm::new("Allow api.github.com", Call::post("/api/network/allow", json!({ "host": "api.github.com" }), "allow api.github.com"))
        .line("web-app may reach api.github.com:443")
        .line("Until you remove it");
    c.preview = Some(Preview { text: "+allow api.github.com\n-deny *".into(), diff: true });
    c.ack = Some("I read what this allows".into());
    c.dont_ask = Some("Do not ask again for files I drop".into());
    c.secrets = vec![Secret { label: "API key".into(), placeholder: "sk-...".into(), field: "secrets.API_KEY".into(), multiline: false, optional: true }];
    c.grants = vec![Grant { field: "hosts".into(), lines: Some(vec!["api.github.com".into(), "uploads.github.com".into()]) }];
    c.danger = true;
    c
}

/// Each dialog's elements, one line each: nesting depth, key, role, name,
/// text, whether it is a Tab stop. No geometry, colour or size.
fn tree(s: S) -> String {
    let mut rt = rune::Runtime::new(s, view, (1280.0, 800.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(0.5);
    let els = rt.elements();
    let depth = |mut i: usize| {
        let mut d = 0;
        while let Some(p) = els[i].parent {
            d += 1;
            i = p;
        }
        d
    };
    let mut out = String::new();
    for (i, e) in els.iter().enumerate() {
        if e.exiting {
            continue;
        }
        let text = e.text.as_deref().unwrap_or("").replace('\n', "\\n");
        out.push_str(&format!("{} {} [{}] label={:?} text={:?} tab={}\n", "  ".repeat(depth(i)), e.key.as_deref().unwrap_or("-"), e.role, e.label.as_deref().unwrap_or(""), text, e.focusable));
    }
    out
}

#[test]
fn the_approval_dialogs_keep_their_elements() {
    let cases: Vec<(&str, S)> = vec![
        ("proposal, high risk", S { which: 0, proposal: proposal("proposal-high"), ..S::default() }),
        ("proposal, low risk, refused once", S { which: 1, proposal: proposal("proposal-low"), acked: true, ..S::default() }),
        ("confirm", S { which: 2, confirm: Some(confirm()), secrets: vec![TextEdit::new(String::new())], ..S::default() }),
        ("confirm, busy with an error", S { which: 3, confirm: Some(confirm()), secrets: vec![TextEdit::new(String::new())], acked: true, ..S::default() }),
        ("copy offer", S { which: 4, offer: Some(CopyOffer::new("web-app", "secret text\nline two".into(), std::time::Instant::now())), ..S::default() }),
    ];
    let mut got = String::new();
    for (name, s) in cases {
        got.push_str(&format!("== {name}\n{}", tree(s)));
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/approval/trees.txt");
    if std::env::var("SBXW_WRITE_APPROVAL_TREE").is_ok_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &got).unwrap();
    }
    let want = std::fs::read_to_string(&path).expect("the snapshot (SBXW_WRITE_APPROVAL_TREE=1 writes it)");
    if got != want {
        let first = got.lines().zip(want.lines()).position(|(a, b)| a != b).unwrap_or(got.lines().count().min(want.lines().count()));
        panic!("the approval dialogs' elements changed at line {}:\n got: {:?}\nwant: {:?}", first + 1, got.lines().nth(first), want.lines().nth(first));
    }
}
