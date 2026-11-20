//! The New thread dialog fixes the name instead of refusing it (T-37): typed
//! text keeps a trailing hyphen, a paste is normalized whole, and a line says
//! what will be created. Driven through the real screen and the Rust rule.
//!
//! `cd desktop && cargo test --test name_dialog`

use rune::{Key, KeyEvent, Runtime};
use sbxw_desktop::shell::{view, App, Args};
use serde_json::json;

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..12 {
        rt.frame(0.05);
        rt.update(|s, tx| {
            s.poll(0.0, tx);
        });
    }
}

fn dialog() -> Runtime<App> {
    let path = format!("{}/fixtures/threads/basic.json", env!("CARGO_MANIFEST_DIR"));
    let mut a = App::new(Args { fixture: Some(path), ..Args::default() });
    a.store = sbxw_desktop::shell::store::Store::memory();
    let mut rt = Runtime::new(a, view, (1284.0, 816.0), rune::Env::new(true, 1.0));
    settle(&mut rt);
    let v = json!({ "kind": "intent", "name": "thread_new", "args": { "sandbox": "web-app" } });
    for i in sbxw_desktop::intent::parse(&v).unwrap() {
        rt.update(|s, _| s.run_intent("test", i));
    }
    settle(&mut rt);
    let (x, y) = rt.find("tn-name").expect("the name field is drawn");
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(&mut rt);
    rt
}

fn texts(rt: &Runtime<App>) -> Vec<String> {
    rt.elements().into_iter().filter(|e| !e.exiting).filter_map(|e| e.text).collect()
}

fn type_str(rt: &mut Runtime<App>, s: &str) {
    for c in s.chars() {
        rt.key(KeyEvent { key: Key::Text(c.to_string()), mods: rune::Mods::default() });
        settle(rt);
    }
}

fn clear(rt: &mut Runtime<App>) {
    for _ in 0..50 {
        rt.key(KeyEvent { key: Key::Backspace, mods: rune::Mods::default() });
    }
    settle(rt);
}

fn has(rt: &Runtime<App>, t: &str) -> bool {
    texts(rt).iter().any(|x| x == t)
}

#[test]
fn typing_lower_cases_keeps_a_trailing_hyphen_and_never_shows_the_old_error() {
    let mut rt = dialog();
    clear(&mut rt);
    type_str(&mut rt, "Fix ");
    assert!(has(&mut rt, "fix-"), "{:?}", texts(&rt));
    assert!(!texts(&rt).iter().any(|t| t.contains("inner hyphens")));
    type_str(&mut rt, "Login");
    assert!(has(&rt, "fix-login"), "{:?}", texts(&rt));
}

#[test]
fn a_paste_is_normalized_whole_and_the_dialog_says_what_will_be_created() {
    let mut rt = dialog();
    clear(&mut rt);
    rt.paste("  API_auth v2 ");
    settle(&mut rt);
    let t = texts(&rt);
    assert!(t.iter().any(|x| x == "api-auth-v2"), "{t:?}");
    assert!(t.iter().any(|x| x.contains(".sbxw-worktrees/api-auth-v2 and the branch sbxw/api-auth-v2")), "{t:?}");
    assert!(!t.iter().any(|x| x.contains("inner hyphens")), "{t:?}");
    clear(&mut rt);
    rt.paste("!!!");
    settle(&mut rt);
    assert!(!texts(&rt).iter().any(|x| x.contains("inner hyphens")));
}
