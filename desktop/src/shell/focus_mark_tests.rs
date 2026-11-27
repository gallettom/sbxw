//! The focused pane and its sidebar row, marked together in one colour
//! (sandboxes.luau `lit_of`, `App::pane_focus_color`), from the one record
//! of focus (`App.focused`, `focused` on one of `data.app.panes`); and a
//! press on a row or a session line whose session is already in a pane of
//! the window focuses that pane and opens nothing (run_intent's
//! OpenTerminal). Through the App in Rune's runtime, the list pressed as a
//! person does. Fixture: fixtures/shell/order.json (invented names).

use super::*;
use crate::client::{BusEvent, TermEvent, TermInput, TerminalConn, Wake, WakeRoute};
use crate::luau::Node;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::mpsc;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/shell/order.json");

/// A daemon that counts the terminals asked of it, each one live.
struct Counting(Arc<AtomicUsize>);

impl Daemon for Counting {
    fn describe(&self) -> String {
        "counting".into()
    }
    fn sandboxes(&self) -> anyhow::Result<Vec<Sandbox>> {
        Ok(vec![])
    }
    fn stop_sandbox(&self, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
    fn open_terminal(&self, _: &TerminalRequest, _: Wake) -> anyhow::Result<TerminalConn> {
        self.0.fetch_add(1, SeqCst);
        let (ev_tx, events) = mpsc::channel::<TermEvent>();
        let (input, in_rx) = mpsc::channel::<TermInput>();
        std::mem::forget((ev_tx, in_rx));
        Ok(TerminalConn { events, input })
    }
    fn subscribe(&self, _: &mut dyn FnMut(BusEvent) -> bool) -> anyhow::Result<()> {
        Ok(())
    }
}

/// A pane: its sandbox, its thread, Bash or the agent.
type P<'a> = (&'a str, Option<&'a str>, Mode);

struct Rig {
    rt: rune::Runtime<App>,
    opened: Arc<AtomicUsize>,
}

/// One page of live terminals as `panes` say (`per` to a page), pane 1
/// focused, the list as wide as `sidebar`.
fn rig(panes: &[P], per: usize, sidebar: Option<f32>) -> Rig {
    let mut a = App::new(Args { fixture: Some(FIXTURE.into()), ..Args::default() });
    a.store = store::Store::memory();
    super::tests::set_panes(&mut a, &panes.iter().map(|p| Some(p.0)).collect::<Vec<_>>());
    let wake: Wake = Arc::new(|| {});
    a.waker = Some(wake.clone());
    let opened = Arc::new(AtomicUsize::new(0));
    a.daemon = Some(Arc::new(Counting(opened.clone())));
    for (i, (_, thread, mode)) in panes.iter().enumerate() {
        let (ev_tx, events) = mpsc::channel::<TermEvent>();
        let (input, in_rx) = mpsc::channel::<TermInput>();
        std::mem::forget((ev_tx, in_rx));
        let mut t = TerminalState::new(TerminalConn { events, input });
        t.id = (i + 1).to_string();
        a.panes[i].thread = thread.map(str::to_string);
        a.panes[i].mode = *mode;
        a.panes[i].term = Some(t);
        a.panes[i].wake = Some(WakeRoute::new(wake.clone()));
    }
    a.sidebar_live = sidebar;
    a.run_intent("test", Intent::Pages(PageOp::Per(per)));
    a.focused = 0;
    let rt = rune::Runtime::new(a, view, (1284.0, 816.0), rune::Env::new(true, 1.0));
    let mut r = Rig { rt, opened };
    r.settle();
    r
}

impl Rig {
    fn settle(&mut self) {
        for _ in 0..30 {
            self.rt.update(|s, tx| {
                s.poll(0.0, tx);
            });
            self.rt.frame(1.0 / 60.0);
        }
    }

    /// A press in the middle of what `key` names, as a person's.
    fn press(&mut self, key: &str) {
        let (r, _) = self.rt.presented(key).unwrap_or_else(|| panic!("{key} drawn"));
        let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        self.rt.pointer_down(x, y);
        self.rt.pointer_up(x, y);
        self.rt.frame(1.0 / 60.0);
        self.settle();
    }

    fn focus(&mut self, i: usize) {
        self.rt.update(|s, _| s.focus_pane(i));
        self.settle();
    }

    fn list(&self) -> Node {
        self.rt.state.screens.iter().find(|s| s.name == "sandboxes").unwrap().last_good.clone().expect("the list drawn")
    }

    /// The marks the list draws, by key.
    fn lit(&self) -> Vec<String> {
        let mut v = vec![];
        keys(&self.list(), "lit-", &mut v);
        v
    }

    /// The sandbox rows marked.
    fn lit_rows(&self) -> Vec<String> {
        self.lit().into_iter().filter(|k| k.starts_with("lit-row-")).collect()
    }

    fn opened(&self) -> usize {
        self.opened.load(SeqCst)
    }

    fn shows(&self, i: usize) -> (Option<String>, Option<String>) {
        let p = &self.rt.state.panes[i];
        (p.sandbox.clone(), p.thread.clone())
    }
}

fn keys(n: &Node, prefix: &str, out: &mut Vec<String>) {
    if let Some(k) = crate::screen::key_of(n).filter(|k| k.starts_with(prefix)) {
        out.push(k);
    }
    for c in &n.children {
        keys(c, prefix, out);
    }
}

fn find<'a>(n: &'a Node, k: &str) -> Option<&'a Node> {
    if crate::screen::key_of(n).as_deref() == Some(k) {
        return Some(n);
    }
    n.children.iter().find_map(|c| find(c, k))
}

const A: Mode = Mode::Agent;

#[test]
fn focusing_a_pane_marks_its_row_and_only_it_in_the_pane_colour() {
    let mut r = rig(&[("cobalt", None, A), ("ember", None, A), ("ivy", None, A)], 4, None);
    assert_eq!(r.lit(), ["lit-row-cobalt"], "pane 1 focused at the start");
    // cobalt has no workspace: the accent, as the pane's border.
    let accent = theme::color_value("accent").unwrap();
    assert_eq!(r.rt.state.pane_focus_color(0), accent);
    assert_eq!(find(&r.list(), "lit-row-cobalt").unwrap().style.fill, Some(accent));
    r.focus(1);
    assert_eq!(r.lit(), ["lit-row-ember"], "pane 2 focused: its row, and only it");
    // ember's workspace gives it a group colour: the pane's border and the mark take it.
    let c = r.rt.state.group_colors()["ember"];
    assert_eq!(r.rt.state.pane_focus_color(1), c);
    assert_eq!(find(&r.list(), "lit-row-ember").unwrap().style.fill, Some(c));
    // A mark only: no control, nothing a press would run.
    let n = find(&r.list(), "lit-row-ember").unwrap();
    assert!(n.on.is_none() && n.children.is_empty());
    // A press in pane 3 (the core's focus follows it): the mark follows too.
    r.press("pane-3");
    assert_eq!(r.rt.state.focused, 2);
    assert_eq!(r.lit(), ["lit-row-ivy", "lit-line-ivy-main"], "ivy has two sessions: its row and the main one's line");
}

#[test]
fn a_row_press_on_a_sandbox_in_pane_3_focuses_pane_3_and_opens_nothing() {
    let mut r = rig(&[("cobalt", None, A), ("ember", None, A), ("fjord", None, A)], 4, None);
    let (n0, panes0) = (r.opened(), r.rt.state.panes.len());
    r.press("name-fjord");
    assert_eq!(r.rt.state.focused, 2, "pane 3 has the focus");
    assert_eq!(r.opened(), n0, "no terminal asked of the daemon");
    assert_eq!(r.rt.state.panes.len(), panes0, "no pane added");
    assert_eq!(r.shows(0).0.as_deref(), Some("cobalt"), "pane 1 kept what it showed");
    assert_eq!(r.lit_rows(), ["lit-row-fjord"]);
}

#[test]
fn a_row_press_on_a_sandbox_in_no_pane_opens_it_in_the_focused_pane_as_before() {
    let mut r = rig(&[("cobalt", None, A), ("ember", None, A)], 4, None);
    r.focus(1);
    let n0 = r.opened();
    r.press("name-ivy");
    assert_eq!(r.opened(), n0 + 1, "one terminal opened");
    assert_eq!(r.rt.state.focused, 1);
    assert_eq!(r.shows(1).0.as_deref(), Some("ivy"), "in the focused pane");
    assert_eq!(r.lit(), ["lit-row-ivy", "lit-line-ivy-main"], "ivy has two sessions: its row and the main one's line");
}

#[test]
fn the_same_sandbox_in_another_mode_is_not_the_same_session() {
    // fjord's shell in pane 2: a press on its row opens its agent in the
    // focused pane, as before.
    let mut r = rig(&[("cobalt", None, A), ("fjord", None, Mode::Bash)], 4, None);
    let n0 = r.opened();
    r.press("name-fjord");
    assert_eq!(r.opened(), n0 + 1);
    assert_eq!(r.rt.state.focused, 0);
    assert_eq!(r.shows(0).0.as_deref(), Some("fjord"));
}

#[test]
fn a_sandbox_in_two_panes_marks_its_row_and_the_session_line_of_the_focused_one() {
    let mut r = rig(&[("atlas-web", None, A), ("atlas-web", Some("search"), A), ("cobalt", None, A)], 4, None);
    let c = r.rt.state.group_colors()["atlas-web"];
    // The main session's pane focused: the row, and the line "main".
    assert_eq!(r.lit(), ["lit-row-atlas-web", "lit-line-atlas-web-main"]);
    assert_eq!(find(&r.list(), "lit-line-atlas-web-main").unwrap().style.fill, Some(c));
    // The thread's pane focused: the same row, the thread's line instead.
    r.focus(1);
    assert_eq!(r.lit(), ["lit-row-atlas-web", "lit-line-atlas-web-search"]);
    assert_eq!(r.rt.state.pane_focus_color(1), c, "both panes of the group in its colour");
    // From pane 3, the row leads to the pane of its main session.
    r.focus(2);
    let n0 = r.opened();
    r.press("name-atlas-web");
    assert_eq!((r.rt.state.focused, r.opened()), (0, n0));
}

#[test]
fn a_session_line_focuses_the_pane_that_shows_that_thread() {
    let mut r = rig(&[("atlas-web", None, A), ("atlas-web", Some("search"), A), ("cobalt", None, A)], 4, None);
    r.focus(2);
    let n0 = r.opened();
    r.press("extra-row-atlas-web-search");
    assert_eq!(r.rt.state.focused, 1, "the thread's pane");
    assert_eq!(r.opened(), n0, "nothing opened");
    assert_eq!(r.shows(2).0.as_deref(), Some("cobalt"));
    assert_eq!(r.lit(), ["lit-row-atlas-web", "lit-line-atlas-web-search"]);
}

#[test]
fn a_pane_on_another_page_of_the_window_is_shown_and_focused() {
    let mut r = rig(&[("cobalt", None, A), ("ember", None, A), ("fjord", None, A)], 2, None);
    let fjord = r.rt.state.panes.iter().find(|p| p.sandbox.as_deref() == Some("fjord")).unwrap().uid;
    let page = r.rt.state.pages.page_of(fjord).unwrap();
    assert_ne!(r.rt.state.pages.current, page, "fjord starts on the other page");
    let n0 = r.opened();
    r.press("name-fjord");
    assert_eq!(r.rt.state.pages.current, page, "its page shows");
    assert_eq!(r.rt.state.panes[r.rt.state.focused].uid, fjord, "its pane has the focus");
    assert_eq!(r.opened(), n0);
    assert_eq!(r.lit_rows(), ["lit-row-fjord"]);
}

#[test]
fn at_the_narrowest_list_the_mark_stays_inside_its_row() {
    let r = rig(&[("ember", None, A), ("atlas-web", Some("search"), A)], 4, Some(layout::SIDEBAR_MIN));
    let (row, _) = r.rt.presented("row-ember").expect("the row");
    let (bar, _) = r.rt.presented("lit-row-ember").expect("the mark");
    assert!(bar[2] > 0.0 && bar[3] > 0.0, "drawn: {bar:?}");
    assert!(bar[0] >= row[0] && bar[0] + bar[2] <= row[0] + row[2] && bar[1] >= row[1] && bar[1] + bar[3] <= row[1] + row[3] + 0.5, "{bar:?} in {row:?}");
}

#[test]
fn the_mark_beside_the_name_does_not_move_the_name() {
    let mut r = rig(&[("cobalt", None, A), ("fjord", None, A)], 4, None);
    let x = |r: &Rig, k: &str| r.rt.presented(k).unwrap().0[0];
    let (cobalt, fjord) = (x(&r, "name-cobalt"), x(&r, "name-fjord"));
    r.focus(1);
    assert!((x(&r, "name-cobalt") - cobalt).abs() < 0.5 && (x(&r, "name-fjord") - fjord).abs() < 0.5, "the names keep their x");
}

#[test]
fn a_hostile_sandbox_name_in_the_focused_pane_makes_no_control() {
    let hostile = "x\" on=\"remove\" arg=\"cobalt\nrow-cobalt";
    let mut a = App::new(Args { fixture: Some(FIXTURE.into()), ..Args::default() });
    a.store = store::Store::memory();
    let mut sb = a.sandboxes.iter().find(|s| s.name == "cobalt").unwrap().clone();
    sb.name = hostile.into();
    a.sandboxes.push(sb);
    super::tests::set_panes(&mut a, &[Some(hostile), Some("cobalt")]);
    a.focused = 0;
    a.push_data();
    let list = a.screens.iter().find(|s| s.name == "sandboxes").unwrap().last_good.clone().unwrap();
    let mark = find(&list, &format!("lit-row-{hostile}")).expect("its mark, under its own key");
    assert!(mark.on.is_none() && mark.children.is_empty(), "a mark, no control");
    let mut lit = vec![];
    keys(&list, "lit-", &mut lit);
    assert_eq!(lit, [format!("lit-row-{hostile}")], "cobalt's row is not marked");
}
