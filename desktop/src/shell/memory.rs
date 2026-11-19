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
/// minutes since start; `memory-summary.py` reads the log as a script's.
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
        self.memory_report(&format!("{} min", now.duration_since(start).as_secs() / 60));
    }

    /// Logs the report under `label`.
    pub(crate) fn memory_report(&self, label: &str) {
        let (rss, hwm) = mem::rss();
        let live = mem::live();
        let mut lines: Vec<(String, u64, String)> = vec![];
        // Terminals: one line each, then their sum.
        let mut terms = 0u64;
        let mut drawn = 0u64;
        // The panes, then those the Team leads view parked (off every page).
        let parked = self.leads.as_ref().map_or(&[][..], |l| l.parked.as_slice());
        for (i, p) in self.panes.iter().chain(parked).enumerate() {
            if let Some(t) = &p.term {
                let m = t.mem();
                terms += m.grid_bytes;
                drawn += m.drawn_bytes;
                let shown = i < self.panes.len() && self.pages.page_of(p.uid) == Some(self.pages.current);
                lines.push((format!("terminal {} ({})", i + 1, p.sandbox.as_deref().unwrap_or("-")), m.grid_bytes, format!("{} lines x {} cols, measured {} MB, {} MB of output{}", m.lines, m.cols, mem::mb(m.measured), mem::mb(m.bytes_in), if i >= self.panes.len() { ", parked by the leads view" } else if shown { "" } else { ", hidden page" })));
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
        let feats: u64 = self.feats.data.values().map(size).sum();
        let raw = size(&self.raw);
        let pics = crate::picture::mem();
        let surfaces = crate::surface::mem();
        let jarvis: u64 = self.jarvis.log.iter().map(|m| m.text.capacity() as u64 + 64).sum();
        lines.push(("terminals: grids, all".into(), terms, format!("{} terminals", self.panes.iter().chain(parked).filter(|p| p.term.is_some()).count())));
        lines.push(("terminals: rows drawn".into(), drawn, "estimate".into()));
        lines.push(("luau: all VMs".into(), lua, format!("{} screens", self.screens.len())));
        // MEM_GC=1: what a full collection of every VM would leave (it runs it).
        if std::env::var_os("MEM_GC").is_some() {
            for s in &self.screens {
                s.collect();
            }
            let after: u64 = self.screens.iter().map(|s| s.mem().lua).sum();
            lines.push(("luau: all VMs after a full collection".into(), after, String::new()));
        }
        per_lua.sort_by(|a, b| b.1.cmp(&a.1));
        for (n, b) in per_lua.iter().take(6) {
            lines.push((format!("luau: {n}"), *b, String::new()));
        }
        lines.push(("element trees (last good)".into(), trees, format!("{nodes} nodes")));
        lines.push(("screens' data".into(), data, format!("{} distinct", datas.len())));
        lines.push(("features' data".into(), feats, format!("{} keys", self.feats.data.len())));
        lines.push(("daemon's raw data".into(), raw, String::new()));
        lines.push(("pictures shown".into(), pics.0, format!("{} kept", pics.1)));
        lines.push(("surfaces".into(), surfaces.0, format!("{} surfaces", surfaces.1)));
        lines.push(("jarvis log".into(), jarvis, format!("{} messages", self.jarvis.log.len())));
        let known: u64 = terms + drawn + lua + trees + data + feats + raw + pics.0 + surfaces.0 + jarvis;
        if let Some(l) = live {
            lines.push(("heap: rest (Rune's trees and caches, features' state, fonts)".into(), l.saturating_sub(known), String::new()));
        }
        log::info!("mem: ==== {label}");
        // MEM_TRIM=1: the resident size once the allocator gave back what it keeps.
        if std::env::var_os("MEM_TRIM").is_some() {
            mem::trim();
            log::info!("mem: rss after trim {} MB", mem::rss().0.map_or("-".into(), mem::mb));
        }
        let fmt = |b: Option<u64>| b.map_or("-".to_string(), mem::mb);
        log::info!("mem: rss {} MB (peak {} MB), heap live {} MB, heap peak {} MB", fmt(rss), fmt(hwm), fmt(live), fmt(mem::peak().map(|p| p.0)));
        for (k, b, note) in lines {
            log::info!("mem: | {k} | {} | {note}", mem::mb(b));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::client::{TermEvent, TerminalConn};
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
