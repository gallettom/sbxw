//! A two-position switch: `select{ ..., switch = true }` with exactly two
//! items (the top bar's GUI | TUI). One pill track holds both labels; one
//! thumb, the width of a label, slides under the chosen one. A press on a
//! label picks it (on the chosen one: picked again, which is how GUI brings
//! Sessions back from under an overlay); Space or Enter flips it; Left and
//! Right move it; the thumb can be dragged past the middle. The select's
//! `on` action gets the picked item's id, as a pick in its list would.
//!
//! The thumb is shifted with Rune's `offset`, which springs in an animated
//! commit (a press, a key) and lands at once in one that is not (data that
//! arrives, a restore at start) and under Reduce Motion. The action runs
//! on the press: the view switches as the thumb starts to travel.

use crate::luau::{Item, Kind, Node};
use crate::theme::{self, c, hex};
use rune::{el, row, text, Cursor, El};
use std::rc::Rc;

/// The track's inner padding, the thumb's travel time.
pub const PAD: f32 = 2.0;
pub const TRAVEL: rune::Spring = rune::Spring::new(0.16, 0.0);
/// A label slot: px 10, icon 14, gap 6, the word at 13/500; icons alone 28.
pub const SLOT_PX: f32 = 10.0;
pub const ICON: f32 = theme::icon::MEDIUM;
pub const GAP: f32 = 6.0;
pub const TEXT: f32 = 13.0;

/// Whether a node is drawn as a switch: a select with `switch = true` and
/// exactly two items (anything else is the plain select).
pub fn is_switch(n: &Node) -> bool {
    n.kind == Kind::Select && n.switch && n.items.iter().filter(|i| !i.divider).count() == 2
}

/// One slot's width: the wider of the two labels, so the thumb fits both.
pub fn slot_w(items: &[&Item]) -> f32 {
    items.iter().map(|i| {
        let words = if i.label.is_empty() { 0.0 } else { rune::text::measure(&i.label, &[], rune::TextStyle::new(TEXT, 500.0), None).0 };
        if i.label.is_empty() { 28.0 } else { SLOT_PX * 2.0 + if i.icon.is_some() { ICON + GAP } else { 0.0 } + words }
    }).fold(28.0, f32::max).ceil()
}

/// The whole switch's width (shell/bar.rs counts it in the top bar's fit).
pub fn width(items: &[&Item]) -> f32 {
    slot_w(items) * 2.0 + PAD * 2.0
}

/// The switch. `flip(s, id)` runs the select's action with that item's id.
pub fn view<S: 'static>(n: &Node, key: String, flip: Rc<dyn Fn(&mut S, &str)>) -> El {
    let items: Vec<&Item> = n.items.iter().filter(|i| !i.divider).collect();
    let value = n.value.as_str().unwrap_or("");
    let on = items.iter().position(|i| i.id == value).unwrap_or(0);
    let w = slot_w(&items);
    let h = theme::layout::CONTROL_H;
    let ids: [String; 2] = [items[0].id.clone(), items[1].id.clone()];
    let other = ids[1 - on].clone();
    let label = format!("{}: {}", items.iter().map(|i| i.label.as_str()).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" | "), items[on].label);
    // The thumb: the raised surface (today's lit segment), under the chosen label.
    let thumb = el()
        .key(format!("{key}-thumb"))
        .pin(Some(PAD), Some(PAD), None, None)
        .w(w)
        .h(h - PAD * 2.0)
        .radius(theme::radius("pill").unwrap())
        .fill(c("bg"))
        .border(1.0, c("border"))
        .offset(on as f32 * w, 0.0)
        .offset_motion(TRAVEL);
    let mut track = row()
        .key(key.clone())
        .role("switch")
        .label(label)
        .h(h)
        .padding(PAD)
        .gap(0.0)
        .items_center()
        .radius(theme::radius("pill").unwrap())
        .fill(c("surface-alt"))
        .focus_ring(2.0, 2.0, c("focus"))
        .child(thumb);
    if let Some(help) = &n.help {
        track = track.help(help.clone());
    }
    for (i, it) in items.iter().enumerate() {
        let lit = i == on;
        let fg = if lit { c("text-bright") } else { c("text-muted") };
        let mut slot = row().key(format!("{key}-{}", it.id)).w(w).h(h - PAD * 2.0).gap(GAP).items_center().justify_center();
        if !n.disabled && !lit {
            let (f, id) = (flip.clone(), it.id.clone());
            slot = slot.cursor(Cursor::Pointer).on_press(move |s: &mut S| f(s, &id));
        } else if !n.disabled {
            // The chosen label (the thumb under it): dragged past the middle,
            // the other side on release; pressed without moving, picked again.
            let (f, to, me) = (flip.clone(), other.clone(), ids[on].clone());
            let dir = if on == 0 { 1.0 } else { -1.0 };
            slot = slot.cursor(Cursor::Pointer).on_drag(move |s: &mut S, d: &mut rune::Drag, _tx| {
                if matches!(d.phase, rune::Phase::Up) {
                    if d.dx * dir > w / 2.0 {
                        f(s, &to);
                    } else if d.dx.abs() < 3.0 {
                        f(s, &me);
                    }
                }
            });
        }
        if let Some(ic) = it.icon.as_deref().and_then(crate::icons::get) {
            slot = slot.child(rune::icon(ic, ICON).color(fg));
        }
        if !it.label.is_empty() {
            slot = slot.child(text(it.label.clone()).font(crate::screen::body(TEXT, 500.0)).color(fg));
        }
        track = track.child(slot);
    }
    if n.disabled {
        return track.opacity(theme::DISABLED_OPACITY).disabled(true);
    }
    let (f1, f2, o1) = (flip.clone(), flip, other);
    let ids2 = ids.clone();
    track
        .focusable(true)
        .cursor(Cursor::Pointer)
        .hover(hex(theme::over(theme::color_value("text").unwrap(), 0.04, theme::color_value("surface-alt").unwrap())))
        .on_press(move |s: &mut S| f1(s, &o1))
        .on_key(move |s: &mut S, ev: &rune::KeyEvent, _tx: &mut rune::Tx| {
            if ev.mods.cmd || ev.mods.ctrl || ev.mods.alt || ev.mods.logo {
                return false;
            }
            let to = match &ev.key {
                rune::Key::Left if on == 1 => Some(&ids2[0]),
                rune::Key::Right if on == 0 => Some(&ids2[1]),
                rune::Key::Left | rune::Key::Right => return true,
                rune::Key::Enter => Some(&ids2[1 - on]),
                rune::Key::Text(t) if t == " " => Some(&ids2[1 - on]),
                _ => None,
            };
            match to {
                Some(id) => {
                    f2(s, id);
                    true
                }
                None => false,
            }
        })
}

#[cfg(test)]
mod tests {
    use crate::shell::{view, App, Args};
    use rune::{Key, KeyEvent, Runtime};
    use serde_json::json;

    fn settle(rt: &mut Runtime<App>) {
        for _ in 0..12 {
            rt.frame(0.05);
            rt.update(|s, tx| {
                s.poll(0.0, tx);
            });
        }
    }

    fn window(w: f32, h: f32) -> Runtime<App> {
        let path = format!("{}/fixtures/shell/two-panes.json", env!("CARGO_MANIFEST_DIR"));
        let mut a = App::new(Args { fixture: Some(path), ..Args::default() });
        a.store = crate::shell::store::Store::memory();
        let mut rt = Runtime::new(a, view, (w, h), rune::Env::new(true, 1.0));
        settle(&mut rt);
        rt
    }

    fn press(rt: &mut Runtime<App>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} is not drawn"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        settle(rt);
    }

    /// The thumb's drawn x against each label's.
    fn under(rt: &Runtime<App>) -> &'static str {
        let x = |k: &str| rt.presented(k).unwrap_or_else(|| panic!("{k}")).0[0];
        let t = x("top-view-thumb");
        if (t - x("top-view-gui")).abs() < 1.0 {
            "gui"
        } else if (t - x("top-view-tui")).abs() < 1.0 {
            "tui"
        } else {
            "between"
        }
    }

    fn gui(rt: &Runtime<App>) -> bool {
        rt.state.feats.base.is_some()
    }

    fn intent(rt: &mut Runtime<App>, name: &str) {
        let v = json!({ "kind": "intent", "name": name, "args": {} });
        for i in crate::intent::parse(&v).unwrap() {
            rt.update(|s, _| crate::screen::Host::intents(s, "test", vec![i]));
        }
        settle(rt);
    }

    #[test]
    fn one_thumb_under_the_chosen_label_by_press_key_and_intent() {
        let mut rt = window(1284.0, 816.0);
        let thumbs = |rt: &Runtime<App>| rt.elements().iter().filter(|e| !e.exiting && e.key.as_deref() == Some("top-view-thumb")).count();
        assert_eq!((under(&rt), gui(&rt), thumbs(&rt)), ("tui", false, 1));
        let role = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("top-view")).map(|e| e.role);
        assert_eq!(role, Some("switch"));
        // A press anywhere on the track flips it; the view switches with it.
        press(&mut rt, "top-view-gui");
        assert_eq!((under(&rt), gui(&rt), thumbs(&rt)), ("gui", true, 1));
        press(&mut rt, "top-view-gui");
        assert_eq!((under(&rt), gui(&rt)), ("gui", true), "a press on the chosen label keeps it");
        press(&mut rt, "top-view-tui");
        assert_eq!((under(&rt), gui(&rt)), ("tui", false));
        // The keyboard (GUI is the left side): Left to GUI, Left again stays, Right back, Space and Enter flip.
        rt.update(|_, tx| tx.focus("top-view", true));
        settle(&mut rt);
        for (k, want) in [(Key::Left, "gui"), (Key::Left, "gui"), (Key::Right, "tui"), (Key::Text(" ".into()), "gui"), (Key::Enter, "tui")] {
            rt.key(KeyEvent { key: k.clone(), mods: rune::Mods::default() });
            settle(&mut rt);
            assert_eq!(under(&rt), want, "{k:?}");
        }
        // As the shortcut and the palette do: the intents.
        intent(&mut rt, "monitor_toggle");
        assert_eq!((under(&rt), gui(&rt)), ("gui", true));
        intent(&mut rt, "monitor_toggle");
        assert_eq!((under(&rt), gui(&rt)), ("tui", false));
        // The thumb (under TUI, the right side) dragged left past the middle: it switches on release.
        let (x, y) = rt.find("top-view-tui").unwrap();
        rt.pointer_down(x, y);
        for i in 1..=6 {
            rt.pointer_move(x - i as f32 * 12.0, y);
            rt.frame(0.02);
        }
        rt.pointer_up(x - 72.0, y);
        settle(&mut rt);
        assert_eq!((under(&rt), gui(&rt)), ("gui", true), "a drag past the middle");
        assert_eq!(thumbs(&rt), 1);
    }

    #[test]
    fn a_restored_gui_starts_with_the_thumb_in_place() {
        let path = format!("{}/fixtures/shell/two-panes.json", env!("CARGO_MANIFEST_DIR"));
        let mut a = App::new(Args { fixture: Some(path), ..Args::default() });
        a.store = crate::shell::store::Store::memory();
        // The state a run that quit in GUI leaves (features.rs `BASE_KEY`).
        a.store.set("base_view", json!("monitor"));
        a.kept_base();
        let mut rt = Runtime::new(a, view, (1284.0, 816.0), rune::Env::new(true, 1.0));
        settle(&mut rt);
        assert!(gui(&rt), "restored in GUI");
        assert_eq!(under(&rt), "gui");
        // Not travelling: the restore lands at once (no animated commit).
        assert!(!rt.elements().iter().any(|e| e.key.as_deref() == Some("top-view-thumb") && e.moving));
    }

    #[test]
    fn a_switch_without_exactly_two_items_is_the_plain_select() {
        let dir = std::env::temp_dir().join(format!("sbxw-switch-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("topbar.luau");
        std::fs::write(&path, r#"
return {
  render = function()
    return row{ select{ id = "three", switch = true, value = "a", on = "x", items = { { id = "a", label = "A" }, { id = "b", label = "B" }, { id = "c", label = "C" } } },
                select{ id = "two", switch = true, value = "a", on = "x", items = { { id = "a", label = "A" }, { id = "b", label = "B" } } } }
  end,
  actions = { x = function() end },
}
"#).unwrap();
        let mut rt = window(1284.0, 816.0);
        rt.update(|s, _| s.screens.iter_mut().find(|x| x.name == "topbar").unwrap().load_file(path.clone()));
        settle(&mut rt);
        std::fs::remove_dir_all(&dir).ok();
        let role = |k: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k)).map(|e| e.role);
        assert_eq!(role("three"), Some("combobox"));
        assert_eq!(role("two"), Some("switch"));
    }
}
