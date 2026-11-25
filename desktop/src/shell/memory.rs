//! The memory report (`mem:<label>` marks in a headless script, and
//! `scripts/memory.sh`): resident memory, the heap's live bytes when
//! counting (`SBXW_DESKTOP_MEM=1`), and the bytes of each owner the app
//! holds. One block of `mem:` lines in the log per mark; `memory-summary.py`
//! turns them into the table of MEMORY.md.

use super::App;
use crate::mem;
use std::rc::Rc;

/// `SBXW_DESKTOP_MEM_EVERY=<seconds>`: the report every so often in a real
/// run (with `SBXW_DESKTOP_MEM=1` for the heap's numbers), labelled by the
/// seconds since start; `memory-summary.py` reads the log as a script's.
fn every() -> Option<std::time::Duration> {
    static EVERY: std::sync::OnceLock<Option<std::time::Duration>> = std::sync::OnceLock::new();
    *EVERY.get_or_init(|| std::env::var("SBXW_DESKTOP_MEM_EVERY").ok().and_then(|v| v.parse::<u64>().ok()).filter(|s| *s > 0).map(std::time::Duration::from_secs))
}

thread_local! {
    static NEXT: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
    static START: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
}

impl App {
    /// The periodic report when asked for ([`every`]); nothing otherwise
    /// (one cached check).
    pub(crate) fn memory_tick(&self) {
        let Some(every) = every() else { return };
        let now = std::time::Instant::now();
        let start = START.with(|s| *s.get().get_or_insert(now));
        START.with(|s| s.set(Some(start)));
        if NEXT.with(|n| n.get().is_some_and(|t| now < t)) {
            return;
        }
        NEXT.with(|n| n.set(Some(now + every)));
        self.memory_report(&format!("{} s", now.duration_since(start).as_secs()));
    }

    /// What the memory audit's estimates depend on, one line: the app's
    /// version, the pane area, the panes, their pages and columns.
    pub(crate) fn layout_line(&self) -> String {
        let area = self.rects.area.map_or("unknown".to_string(), |r| format!("{:.0}x{:.0} pt", r[2], r[3]));
        let shown = self.shown();
        let sizes: Vec<String> = self.panes.iter().enumerate().filter_map(|(i, p)| p.term.as_ref().map(|t| format!("{}x{}{}", t.size.0, t.size.1, if shown.contains(&i) { "" } else { " hidden" }))).collect();
        format!(
            "perf: layout version {} pane area {area}, {} panes on {} pages (page {} shown), {} page windows, terminals (cols x rows) [{}]",
            env!("CARGO_PKG_VERSION"),
            self.panes.iter().filter(|p| p.term.is_some()).count(),
            self.pages.list.len(),
            self.pages.current + 1,
            self.windowed.len(),
            sizes.join(", ")
        )
    }

    /// Logs the report under `label`.
    pub(crate) fn memory_report(&self, label: &str) {
        let (rss, hwm) = mem::rss();
        let r = self.memory_lines(mem::live());
        log::info!("mem: ==== {label}");
        log::info!("{}", self.layout_line());
        // MEM_TRIM=1: the resident size once the allocator gave back what it keeps.
        if std::env::var_os("MEM_TRIM").is_some() {
            mem::trim();
            log::info!("mem: rss after trim {} MB", mem::rss().0.map_or("-".into(), mem::mb));
        }
        let fmt = |b: Option<u64>| b.map_or("-".to_string(), mem::mb);
        log::info!("mem: rss {} MB (peak {} MB), heap live {} MB, heap peak {} MB", fmt(rss), fmt(hwm), fmt(r.live), fmt(mem::peak().map(|p| p.0)));
        for l in &r.lines {
            log::info!("mem: | {}{} | {} | {}", if l.part { "  " } else { "" }, l.name, fmt(l.bytes), l.note);
        }
        // The owners' sum against the heap's total: the top-level lines (not
        // the indented parts of one) and the rest add up to heap live.
        log::info!("mem: sum of owners {} MB, rest {} MB, heap live {} MB", mem::mb(r.known), fmt(r.rest), fmt(r.live));
    }

    /// The report's lines. `live`: the heap's live bytes (counting on),
    /// against which the rest is computed.
    pub(crate) fn memory_lines(&self, live: Option<u64>) -> Report {
        let mut lines: Vec<Line> = vec![];
        let line = |name: &str, bytes: u64, note: String| Line { name: name.to_string(), bytes: Some(bytes), note, part: false };
        let part = |name: String, bytes: u64, note: String| Line { name, bytes: Some(bytes), note, part: true };
        // Terminals: one line each, then their sums.
        let mut terms = 0u64;
        let mut drawn = 0u64;
        let (mut on_page, mut hidden, mut parked_b, mut windowed_b) = (0u64, 0u64, 0u64, 0u64);
        let (mut queued_n, mut queued_b) = (0usize, 0u64);
        // The panes, then those the Team leads view parked (off every page).
        let parked = self.leads.as_ref().map_or(&[][..], |l| l.parked.as_slice());
        let mut per_term = vec![];
        for (i, p) in self.panes.iter().chain(parked).enumerate() {
            if let Some(t) = &p.term {
                let m = t.mem();
                terms += m.grid_bytes;
                drawn += m.drawn_bytes;
                let (qn, qb) = t.queued_events();
                queued_n += qn;
                queued_b += qb;
                let shown = i < self.panes.len() && self.pages.page_of(p.uid) == Some(self.pages.current);
                let where_ = if i >= self.panes.len() {
                    parked_b += m.grid_bytes;
                    ", parked by the leads view"
                } else if shown {
                    on_page += m.grid_bytes;
                    ", current page"
                } else {
                    hidden += m.grid_bytes;
                    ", hidden page"
                };
                per_term.push(part(format!("terminal {} ({})", i + 1, p.sandbox.as_deref().unwrap_or("-")), m.grid_bytes, format!("{} lines x {} cols, measured {} MB, {} MB of output{where_}", m.lines, m.cols, mem::mb(m.measured), mem::mb(m.bytes_in))));
            }
        }
        // The page windows' terminals (pagewin.rs).
        for wp in &self.windowed {
            for p in &wp.panes {
                if let Some(t) = &p.term {
                    let m = t.mem();
                    terms += m.grid_bytes;
                    drawn += m.drawn_bytes;
                    windowed_b += m.grid_bytes;
                    let (qn, qb) = t.queued_events();
                    queued_n += qn;
                    queued_b += qb;
                    per_term.push(part(format!("terminal in page window {} ({})", wp.id, p.sandbox.as_deref().unwrap_or("-")), m.grid_bytes, format!("{} lines x {} cols, measured {} MB, {} MB of output, page window", m.lines, m.cols, mem::mb(m.measured), mem::mb(m.bytes_in))));
                }
            }
        }
        // Screens: Luau VMs, element trees, the data they point at (shared:
        // each distinct one once).
        let mut lua = 0u64;
        let mut trees = 0u64;
        let mut nodes = 0usize;
        let mut datas: Vec<Rc<serde_json::Value>> = vec![];
        let mut per_lua: Vec<(String, u64)> = vec![];
        for s in &self.screens {
            let m = s.mem();
            lua += m.lua;
            trees += m.tree;
            nodes += m.nodes;
            per_lua.push((s.name.clone(), m.lua));
            if !datas.iter().any(|d| Rc::ptr_eq(d, &m.data)) {
                datas.push(m.data);
            }
        }
        let size = |v: &serde_json::Value| mem::heap_of(|| v.clone()).unwrap_or_else(|| mem::json_bytes(v));
        let data: u64 = datas.iter().map(|d| size(d)).sum();
        // Older snapshots something still holds (a Luau render's lazy tables
        // until Luau collects them: the memory audit's finding 4).
        let pinned: Vec<Rc<serde_json::Value>> = alive_snapshots().into_iter().filter(|d| !datas.iter().any(|x| Rc::ptr_eq(x, d))).collect();
        let pinned_b: u64 = pinned.iter().map(|d| size(d)).sum();
        let feats: u64 = self.feats.data.values().map(size).sum();
        let raw = size(&self.raw);
        let pics = crate::picture::mem();
        let kanban = crate::feat::kanban::previews_mem();
        let canvas = crate::surface_canvas::mem();
        let handed = crate::feat::canvas::handed_mem();
        let surfaces = crate::surface::mem();
        let conv = crate::feat::monitor::conv_mem(&size);
        let jarvis: u64 = self.jarvis.log.iter().map(|m| m.text.capacity() as u64 + 64).sum();
        let nterms = self.panes.iter().chain(parked).chain(self.windowed.iter().flat_map(|w| w.panes.iter())).filter(|p| p.term.is_some()).count();
        lines.extend(per_term);
        lines.push(line("terminals: grids, all", terms, format!("{nterms} terminals")));
        lines.push(part("on the current page".into(), on_page, String::new()));
        lines.push(part("on hidden pages".into(), hidden, String::new()));
        lines.push(part("parked by the leads view".into(), parked_b, String::new()));
        lines.push(part("in page windows".into(), windowed_b, String::new()));
        lines.push(line("terminals: rows drawn", drawn, "estimate".into()));
        lines.push(line("terminals: queued events", queued_b, format!("{queued_n} waiting (titles, copies: a parked pane's are never taken)")));
        lines.push(line("luau: all VMs", lua, format!("{} screens", self.screens.len())));
        per_lua.sort_by(|a, b| b.1.cmp(&a.1));
        for (n, b) in per_lua.iter().take(6) {
            lines.push(part(format!("luau: {n}"), *b, String::new()));
        }
        lines.push(line("element trees (last good)", trees, format!("{nodes} nodes")));
        lines.push(line("screens' data", data, format!("{} distinct", datas.len())));
        lines.push(line("screens' data: older snapshots still held", pinned_b, format!("{} snapshots (a render's lazy tables keep theirs until Luau collects)", pinned.len())));
        lines.push(line("features' data", feats, format!("{} keys", self.feats.data.len())));
        lines.push(line("daemon's raw data", raw, String::new()));
        lines.push(line("pictures shown", pics.0, format!("{} kept", pics.1)));
        lines.push(part("of which the Kanban's previews".into(), kanban.0, format!("{} kept", kanban.1)));
        lines.push(line("canvas pictures", canvas.0, format!("{} pictures (thumbs, previews, full levels)", canvas.1)));
        lines.push(line("canvas: example pictures handed in", handed.0, format!("{} kept (until another canvas shows)", handed.1)));
        lines.push(line("surfaces", surfaces.0, format!("{} surfaces", surfaces.1)));
        lines.push(line("sessions conversations", conv.0, format!("{} logs, {} items", conv.1, conv.2)));
        lines.push(line("jarvis log", jarvis, format!("{} messages", self.jarvis.log.len())));
        let known: u64 = lines.iter().filter(|l| !l.part).filter_map(|l| l.bytes).sum();
        // Not readable from here: said, not left out.
        lines.push(Line { name: "Rune's image atlas".into(), bytes: None, note: "not readable from the app at the pinned Rune: a CPU copy of each picture drawn (pages of 4 MB, or the picture's size over 1024 px), in the rest".into(), part: false });
        lines.push(Line { name: "GPU memory".into(), bytes: None, note: "not readable from the app (textures, swap chain): the system's footprint has it on macOS, scripts/measure.sh records it".into(), part: false });
        // MEM_GC=1: what a full collection of every VM would leave (it runs
        // it), and the heap it frees outside Luau (what the lazy tables pinned).
        if std::env::var_os("MEM_GC").is_some() {
            let before = mem::live();
            for s in &self.screens {
                s.collect();
            }
            let after: u64 = self.screens.iter().map(|s| s.mem().lua).sum();
            lines.push(Line { name: "luau: all VMs after a full collection".into(), bytes: Some(after), note: String::new(), part: true });
            let still = alive_snapshots().into_iter().filter(|d| !datas.iter().any(|x| Rc::ptr_eq(x, d))).count();
            let freed = before.zip(mem::live()).map(|(b, a)| b.saturating_sub(a));
            lines.push(Line { name: "heap freed by that collection".into(), bytes: freed, note: format!("older snapshots still held after it: {still}"), part: true });
        }
        let rest = live.map(|l| l.saturating_sub(known));
        if let Some(r) = rest {
            lines.push(line("heap: rest (Rune's trees and caches, image atlas, features' state, fonts)", r, String::new()));
        }
        Report { lines, known, rest, live }
    }
}

/// One line of the report. `part`: a part of the line above it (drawn
/// indented), left out of the sum. `bytes`: None when not readable.
pub(crate) struct Line {
    pub name: String,
    pub bytes: Option<u64>,
    pub note: String,
    pub part: bool,
}

pub(crate) struct Report {
    pub lines: Vec<Line>,
    /// The sum of the top-level lines.
    pub known: u64,
    /// The heap's live bytes the lines do not explain (counting on).
    pub rest: Option<u64>,
    pub live: Option<u64>,
}

thread_local! {
    /// Every data snapshot pushed to the screens, weakly (counting on), to
    /// tell how many older ones something still holds.
    static SNAPSHOTS: std::cell::RefCell<Vec<std::rc::Weak<serde_json::Value>>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Notes a pushed snapshot (shell.rs's `push_data_once`, counting on).
pub(crate) fn keep_snapshot(d: &Rc<serde_json::Value>) {
    SNAPSHOTS.with(|s| {
        let mut s = s.borrow_mut();
        s.retain(|w| w.strong_count() > 0);
        s.push(Rc::downgrade(d));
    });
}

/// The pushed snapshots still alive.
fn alive_snapshots() -> Vec<Rc<serde_json::Value>> {
    SNAPSHOTS.with(|s| s.borrow().iter().filter_map(|w| w.upgrade()).collect())
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::keep_snapshot;
    use crate::client::{TermEvent, TerminalConn};
    use std::rc::Rc;
    use std::sync::mpsc;

    /// A pane the Team leads view parked keeps taking its output in: parsed
    /// into its history as it comes, not left waiting in the socket's
    /// channel (unbounded) until the view closes.
    #[test]
    fn a_parked_pane_takes_its_output_in() {
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/shell/two-panes.json");
        let mut a = App::new(Args { fixture: Some(fixture.into()), ..Args::default() });
        let (ev_tx, ev_rx) = mpsc::channel();
        let (in_tx, _in_rx) = mpsc::channel();
        let mut t = TerminalState::new(TerminalConn { events: ev_rx, input: in_tx });
        ev_tx.send(TermEvent::Size { cols: 80, rows: 24, owner: true }).unwrap();
        t.poll(None);
        let mut p = Pane::empty(a.fresh());
        p.sandbox = Some("docs".into());
        p.term = Some(t);
        a.leads = Some(leads::LeadsView { saved: a.pages.clone(), focus: None, parked: vec![p], opened: vec![], sessions: HashMap::new() });
        let mut rt = rune::Runtime::new(a, view, (1284.0, 816.0), rune::Env::new(true, 1.0));
        let lines: String = (0..500).map(|i| format!("line {i}\r\n")).collect();
        ev_tx.send(TermEvent::Output(lines.into_bytes())).unwrap();
        rt.update(|s, tx| {
            s.poll(0.0, tx);
        });
        let t = rt.state.leads.as_ref().unwrap().parked[0].term.as_ref().unwrap();
        assert!(t.history_len() >= 470, "the output is in its history: {}", t.history_len());
        assert!(t.visible_text().contains("line 499"));
    }

    /// The report has a line for every owner the memory audit named, says
    /// what it cannot read instead of leaving it out, and its top-level
    /// lines plus the rest add up to the heap's total.
    #[test]
    fn the_report_names_every_owner_and_adds_up() {
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/shell/two-panes.json");
        let mut a = App::new(Args { fixture: Some(fixture.into()), ..Args::default() });
        super::super::tests::set_panes(&mut a, &[Some("web-app"), Some("docs")]);
        for i in 0..2 {
            let (ev_tx, ev_rx) = mpsc::channel();
            let (in_tx, _in_rx) = mpsc::channel();
            let mut t = TerminalState::new(TerminalConn { events: ev_rx, input: in_tx });
            ev_tx.send(TermEvent::Size { cols: 80, rows: 24, owner: true }).unwrap();
            t.poll(None);
            a.panes[i].term = Some(t);
        }
        a.push_data();
        let r = a.memory_lines(None);
        let names: Vec<&str> = r.lines.iter().map(|l| l.name.as_str()).collect();
        for n in [
            "terminals: grids, all",
            "on the current page",
            "on hidden pages",
            "parked by the leads view",
            "in page windows",
            "terminals: queued events",
            "screens' data: older snapshots still held",
            "pictures shown",
            "of which the Kanban's previews",
            "canvas pictures",
            "canvas: example pictures handed in",
            "sessions conversations",
            "Rune's image atlas",
            "GPU memory",
        ] {
            assert!(names.contains(&n), "no line {n:?}: {names:?}");
        }
        assert!(r.lines.iter().any(|l| l.name.starts_with("terminal 1 (web-app)") && l.note.contains("80 cols")), "each terminal with its width");
        for n in ["Rune's image atlas", "GPU memory"] {
            let l = r.lines.iter().find(|l| l.name == n).unwrap();
            assert!(l.bytes.is_none() && l.note.contains("not readable"), "{n}: said, with no number");
        }
        let top: u64 = r.lines.iter().filter(|l| !l.part).filter_map(|l| l.bytes).sum();
        assert_eq!(r.known, top, "the sum is of the top-level lines");
        assert!(r.rest.is_none(), "no rest without counting");
        // Counting on, the rest is what the lines leave of the heap's total.
        let live = r.known + 12_345;
        let r = a.memory_lines(Some(live));
        let rest = r.lines.iter().find(|l| l.name.starts_with("heap: rest")).and_then(|l| l.bytes).unwrap();
        let top: u64 = r.lines.iter().filter(|l| !l.part && !l.name.starts_with("heap: rest")).filter_map(|l| l.bytes).sum();
        assert_eq!(top + rest, live, "top-level lines and the rest add up to heap live");
        // A part never exceeds its parent's total.
        let get = |n: &str| r.lines.iter().find(|l| l.name == n).and_then(|l| l.bytes).unwrap();
        assert!(get("on the current page") + get("on hidden pages") + get("parked by the leads view") + get("in page windows") == get("terminals: grids, all"));
        assert!(get("of which the Kanban's previews") <= get("pictures shown"));
    }

    /// A pushed snapshot a render still holds is counted as held; once
    /// dropped, it is not.
    #[test]
    fn older_snapshots_still_held_are_counted() {
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/shell/two-panes.json");
        let a = App::new(Args { fixture: Some(fixture.into()), ..Args::default() });
        let held = Rc::new(serde_json::json!({ "big": "x".repeat(10_000) }));
        keep_snapshot(&held);
        let line = |a: &App| a.memory_lines(None).lines.into_iter().find(|l| l.name == "screens' data: older snapshots still held").unwrap();
        let l = line(&a);
        assert!(l.note.starts_with("1 snapshots") && l.bytes.unwrap() >= 10_000, "{} {:?}", l.note, l.bytes);
        drop(held);
        let l = line(&a);
        assert!(l.note.starts_with("0 snapshots") && l.bytes == Some(0), "{}", l.note);
    }

    /// A feature's screen has no Luau VM until it is first drawn; opened,
    /// it renders from the latest data; the shell's own screens load at start.
    #[test]
    fn a_screen_s_vm_is_made_when_it_is_first_drawn() {
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/shell/two-panes.json");
        let mut a = App::new(Args { fixture: Some(fixture.into()), ..Args::default() });
        let vm = |a: &App, n: &str| a.screens.iter().find(|s| s.name == n).map(|s| (s.has_vm(), s.last_good.is_some())).unwrap();
        assert_eq!(vm(&a, "sandboxes"), (true, true));
        assert_eq!(vm(&a, "topbar"), (true, true));
        assert_eq!(vm(&a, "commits"), (false, false), "never drawn: no VM");
        assert_eq!(vm(&a, "network"), (false, false));
        a.push_data();
        assert_eq!(vm(&a, "commits"), (false, false), "new data does not make it");
        a.run_intent("test", crate::intent::Intent::OpenPanel { screen: "commits".into() });
        a.push_data();
        assert_eq!(vm(&a, "commits"), (true, true), "opened: made and rendered");
        let sc = a.screens.iter().find(|s| s.name == "commits").unwrap();
        assert!(sc.error.is_none(), "{:?}", sc.error);
        assert_eq!(vm(&a, "network"), (false, false));
    }
}
