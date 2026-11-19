//! The memory report (`mem:<label>` marks in a headless script, and
//! `scripts/memory.sh`): resident memory, the heap's live bytes when
//! counting (`SBXW_DESKTOP_MEM=1`), and the bytes of each owner the app
//! holds. One block of `mem:` lines in the log per mark; `memory-summary.py`
//! turns them into the table of MEMORY.md.

use super::App;
use crate::mem;
use std::rc::Rc;

impl App {
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
        let fmt = |b: Option<u64>| b.map_or("-".to_string(), mem::mb);
        log::info!("mem: rss {} MB (peak {} MB), heap live {} MB, heap peak {} MB", fmt(rss), fmt(hwm), fmt(live), fmt(mem::peak().map(|p| p.0)));
        for (k, b, note) in lines {
            log::info!("mem: | {k} | {} | {note}", mem::mb(b));
        }
    }
}
