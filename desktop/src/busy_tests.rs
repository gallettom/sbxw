//! A busy button (`button{ busy = true }`, screen.rs): its own colours, a
//! spinner, no press (pointer or keyboard), frames asked for only while one
//! is drawn, a resting ring under Reduce Motion.

use crate::shell::{view, App, Args};
use rune::{Key, KeyEvent, Runtime};

const SCREEN: &str = r#"
return {
  render = function(state)
    return row{ gap = 8,
      text{ "N" .. (state.n or 0), key = "count" },
      button{ "Work", variant = "primary", on = "bump", busy = state.busy ~= false, key = "work" },
      button{ "Off", variant = "primary", on = "bump", disabled = true, key = "off" },
      button{ "Done", on = "stop", key = "stop" } }
  end,
  actions = { bump = function(state) state.n = (state.n or 0) + 1 end,
              stop = function(state) state.busy = false end },
}
"#;

/// Frames until the fixture's link has answered everything asked (its calls
/// and list reads run on threads of their own, and a loaded machine answers
/// after any fixed count of frames), then a few more for the drawing to
/// follow. The wait is bounded and fails saying what it waited for.
fn settle(rt: &mut Runtime<App>) {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut quiet = 0;
    while quiet < 12 {
        rt.frame(0.05);
        rt.update(|s, tx| {
            s.poll(0.0, tx);
        });
        let busy = !rt.state.link_quiet();
        quiet = if busy { 0 } else { quiet + 1 };
        if busy {
            assert!(std::time::Instant::now() < end, "the fixture's link never settled (it still has answers to give)");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

/// The two-panes fixture with the status bar's CPU figures fixed. Without a
/// `stats` the bar shows this process's own CPU share, measured on the wall
/// clock (the first figure comes 0.5 s in, later ones every 2 s): the figure
/// arrives and changes the room the git item has, which then glides, whenever
/// the run is slow enough to have a clock tick after the test settled.
fn pinned_fixture(dir: &std::path::Path) -> String {
    let src = format!("{}/fixtures/shell/two-panes.json", env!("CARGO_MANIFEST_DIR"));
    let mut fx: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(src).unwrap()).unwrap();
    let daemon = fx["daemon"].as_object_mut().expect("the fixture's daemon object");
    daemon.insert("stats".into(), serde_json::json!({ "sandboxes": [], "daemon_cpu_pct": 0.0, "total_cpu_pct": 0.0, "sandboxes_counted": 0, "sandboxes_unknown": 0, "ts": 1763380000000u64, "app_cpu_pct": 0.0 }));
    let path = dir.join("two-panes.json");
    std::fs::write(&path, serde_json::to_string(&fx).unwrap()).unwrap();
    path.to_string_lossy().into_owned()
}

/// The status bar reads the git item of the pane's sandbox once the sandbox
/// has been on screen for a debounce of wall-clock time (shell/statusbar/
/// git.rs); the fixture answers inside that poll, and the item then glides
/// (frames). Frames alone never pass wall-clock time: on a slow run the read
/// landed after the test had settled, and the next frame was due. So: poll
/// until the debounce has certainly run out since `since`, then once more
/// (the read), then settle for the glide.
fn let_the_bar_rest(rt: &mut Runtime<App>, since: std::time::Instant) {
    while since.elapsed() < crate::shell::statusbar::git::DEBOUNCE {
        rt.frame(0.05);
        rt.update(|s, tx| {
            s.poll(0.0, tx);
        });
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    rt.update(|s, tx| {
        s.poll(0.0, tx);
    });
    settle(rt);
}

fn window(reduced: bool) -> Runtime<App> {
    let since = std::time::Instant::now();
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("sbxw-busy-test-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("topbar.luau");
    std::fs::write(&path, SCREEN).unwrap();
    let path = pinned_fixture(&dir);
    let mut a = App::new(Args { fixture: Some(path), ..Args::default() });
    a.store = crate::shell::store::Store::memory();
    let mut rt = Runtime::new(a, view, (1284.0, 816.0), rune::Env::new(true, 1.0));
    settle(&mut rt);
    rt.update(|s, _| s.screens.iter_mut().find(|x| x.name == "topbar").unwrap().load_file(dir.join("topbar.luau")));
    rt.set_system_reduced_motion(reduced);
    settle(&mut rt);
    let_the_bar_rest(&mut rt, since);
    std::fs::remove_dir_all(&dir).ok();
    rt
}

fn info(rt: &Runtime<App>, key: &str) -> rune::ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key) && !e.exiting).unwrap_or_else(|| panic!("{key} is not drawn"))
}

fn count(rt: &Runtime<App>) -> String {
    info(rt, "count").text.unwrap()
}

fn spinning(rt: &Runtime<App>) -> usize {
    rt.elements().iter().filter(|e| !e.exiting && e.loop_phase.is_some()).count()
}

fn press(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} is not drawn"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

#[test]
fn a_busy_button_keeps_its_colours_and_a_disabled_one_is_dimmed() {
    let rt = window(false);
    assert!(info(&rt, "work").opacity > 0.99, "busy is not dimmed");
    assert!(info(&rt, "off").opacity < 1.0, "disabled is dimmed");
    assert_eq!(spinning(&rt), 1, "one spinner, on the busy button");
}

#[test]
fn a_busy_button_ignores_the_pointer_and_the_keyboard() {
    let mut rt = window(false);
    press(&mut rt, "work");
    assert_eq!(count(&rt), "N0");
    rt.update(|_, tx| tx.focus("work", true));
    settle(&mut rt);
    for k in [Key::Enter, Key::Text(" ".into())] {
        rt.key(KeyEvent { key: k, mods: rune::Mods::default() });
        settle(&mut rt);
    }
    assert_eq!(count(&rt), "N0", "no press reached the action");
}

#[test]
fn frames_are_asked_for_only_while_a_busy_button_is_drawn() {
    let mut rt = window(false);
    assert!(rt.frame(0.05), "the spinner turns: another frame is due");
    press(&mut rt, "stop");
    assert_eq!(spinning(&rt), 0);
    assert!(!rt.frame(0.05), "idle again: nothing asks for a frame");
}

#[test]
fn under_reduced_motion_the_ring_rests() {
    let mut rt = window(true);
    assert!(!rt.frame(0.05), "no frames are asked for");
    assert!(info(&rt, "work").opacity > 0.99);
    assert_eq!(spinning(&rt), 1);
    assert!(rt.elements().iter().filter(|e| e.loop_phase.is_some()).all(|e| e.loop_phase == Some(0.0)));
}

#[test]
fn a_button_that_stops_being_busy_presses_again() {
    let mut rt = window(false);
    press(&mut rt, "stop");
    press(&mut rt, "work");
    assert_eq!(count(&rt), "N1");
}
