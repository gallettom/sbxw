//! The team leads view: the rail's "Team leads" entry shows only the team
//! leaders' terminals, and pressed again puts back exactly the layout that
//! was there before.
//!
//! * A leader is the main session of a running sandbox that leads a team:
//!   the daemon's `GET /api/leads` (`data.leads`, a list of sandbox names),
//!   or a plan of that sandbox still under way (`feat::threads::model::leading`,
//!   the same rule as the pane bar's "Team on"). Threads are never leaders.
//! * Entering: the pages, N, tilings, current page and focus are kept; one
//!   pane per leader, in the list's order, on pages of N with the default
//!   tiling. A leader already open keeps its pane (the same terminal,
//!   moved); one not open is opened (its sandbox runs by definition). The
//!   other panes are parked: off every page, their terminals running.
//! * Leaving: the kept layout comes back exactly; panes opened only for the
//!   view close (their sessions go on); what the person changed in the view
//!   is dropped. A pane of the kept layout closed in the view is opened
//!   again when its sandbox still runs, else left out.
//! * Quitting in the view: the state file holds the kept layout
//!   (`App::save_layout`), never the view.

use super::*;

/// The view while it shows.
pub struct LeadsView {
    pub saved: Pages,
    pub focus: Option<Uid>,
    /// Panes of the kept layout the view does not show, terminals running.
    pub parked: Vec<Pane>,
    /// Panes opened for the view (closed on leaving).
    pub opened: Vec<Uid>,
    /// Each kept pane's session, to open one again that was closed meanwhile.
    pub sessions: HashMap<Uid, (String, Mode, Option<String>)>,
}

impl App {
    /// The running sandboxes that lead a team, in the list's order.
    pub fn leaders(&self) -> Vec<String> {
        let data = serde_json::json!({ "leads": self.feats.data.get("leads").cloned().unwrap_or(J::Null), "plans": self.feats.data.get("plans").cloned().unwrap_or(J::Null) });
        self.sandboxes.iter().filter(|s| s.status == "running" && crate::feat::threads::model::leading(&data, &s.name)).map(|s| s.name.clone()).collect()
    }

    pub fn leads_on(&self) -> bool {
        self.leads.is_some()
    }

    /// The rail's entry, the palette's command.
    pub fn toggle_leads(&mut self) {
        if self.leads.is_some() {
            self.leave_leads();
        } else {
            self.enter_leads();
        }
    }

    fn enter_leads(&mut self) {
        let leaders = self.leaders();
        if leaders.is_empty() {
            return self.notify("No team is running", false);
        }
        let saved = self.pages.clone();
        let focus = self.focused_uid();
        let sessions: HashMap<Uid, (String, Mode, Option<String>)> = self.panes.iter().filter_map(|p| Some((p.uid, (p.sandbox.clone()?, p.mode, p.thread.clone())))).collect();
        let mut shown = vec![];
        let mut opened = vec![];
        for l in &leaders {
            let have = self.panes.iter().find(|p| p.sandbox.as_deref() == Some(l.as_str()) && p.thread.is_none() && p.mode == Mode::Agent && !shown.contains(&p.uid)).map(|p| p.uid);
            match have {
                Some(u) => shown.push(u),
                None => {
                    let u = self.fresh();
                    self.panes.push(Pane::empty(u));
                    shown.push(u);
                    opened.push((u, l.clone()));
                }
            }
        }
        let per = saved.per.max(1);
        let list: Vec<pages::Page> = shown.chunks(per).map(|c| pages::Page::of(tiling::default(c, false))).collect();
        let (keep, parked): (Vec<Pane>, Vec<Pane>) = std::mem::take(&mut self.panes).into_iter().partition(|p| shown.contains(&p.uid));
        self.panes = keep;
        self.leads = Some(LeadsView { saved: saved.clone(), focus, parked, opened: opened.iter().map(|o| o.0).collect(), sessions });
        self.pages = Pages { list, current: 0, per, grid_per: saved.grid_per };
        self.sync_panes(shown.first().copied());
        for (u, l) in opened {
            if let Some(i) = self.index_of(u) {
                self.open_with(i, &l, Mode::Agent, None, ThreadStart::default());
            }
        }
        if let Some(i) = shown.first().and_then(|u| self.index_of(*u)) {
            self.focused = i;
        }
        self.save_layout();
    }

    fn leave_leads(&mut self) {
        let Some(v) = self.leads.take() else { return };
        // The view's own panes close; the leaders that were there before and
        // the parked panes go back to their places.
        let mut panes: Vec<Pane> = std::mem::take(&mut self.panes).into_iter().filter(|p| !v.opened.contains(&p.uid)).collect();
        panes.extend(v.parked);
        self.panes = panes;
        let mut saved = v.saved;
        let running: Vec<String> = self.sandboxes.iter().filter(|s| s.status == "running").map(|s| s.name.clone()).collect();
        let mut reopen = vec![];
        for u in saved.all() {
            if self.index_of(u).is_some() {
                continue;
            }
            match v.sessions.get(&u) {
                // Closed in the view: open it again where it was.
                Some((sb, mode, thread)) if running.contains(sb) => {
                    self.panes.push(Pane::empty(u));
                    reopen.push((u, sb.clone(), *mode, thread.clone()));
                }
                // An empty pane of the kept layout.
                None => self.panes.push(Pane::empty(u)),
                // Its sandbox stopped or went: not put back.
                Some(_) => {
                    let fresh = self.fresh();
                    if saved.remove(u, true, fresh) {
                        self.panes.push(Pane::empty(fresh));
                    }
                }
            }
        }
        self.pages = saved;
        self.sync_panes(v.focus);
        for (u, sb, mode, thread) in reopen {
            if let Some(i) = self.index_of(u) {
                self.open_with(i, &sb, mode, thread, ThreadStart::default());
            }
        }
        if let Some(i) = v.focus.and_then(|u| self.index_of(u)) {
            self.focused = i;
        }
        self.save_layout();
    }
}

#[cfg(test)]
mod tests {
    use super::super::layout_tests::Rig;
    use super::*;

    fn rig() -> Rig {
        let mut r = Rig::new(&["a", "b", "c", "d"], 1284.0, 816.0);
        r.rt.update(|s, _| {
            s.sandboxes = ["a", "b", "c", "d", "e"].iter().map(|n| crate::client::Sandbox::new(n, "claude", "running")).collect();
            s.feats.data.insert("leads".into(), serde_json::json!(["a", "c", "e"]));
        });
        r.settle();
        r
    }

    #[test]
    fn entering_and_leaving_puts_the_layout_back_exactly() {
        let mut r = rig();
        // A layout of our own: 2 per page, page 2 current, pane "d" focused, a rotation.
        r.op(PageOp::Per(2));
        r.op(PageOp::Show(1));
        r.op(PageOp::Rotate(None));
        r.rt.update(|s, _| s.focus_pane(1));
        r.settle();
        let before = r.generations();
        let pages = r.rt.state.pages.clone();
        let focus = r.rt.state.focused_uid();
        r.take();
        r.rt.update(|s, _| s.toggle_leads());
        r.settle();
        assert!(r.rt.state.leads_on());
        assert_eq!(r.names(), [vec!["a", "c"], vec!["e"]], "the leaders, on pages of N");
        // a and c are the same terminals, moved.
        let now = r.generations();
        for (u, g) in &before {
            if let Some(n) = now.get(u) {
                assert_eq!(n, g, "kept terminal {u}");
            }
        }
        let opened = r.rt.state.leads.as_ref().unwrap().opened.clone();
        assert_eq!(opened.len(), 1, "e was opened for the view");
        // Changes in the view are the view's.
        r.op(PageOp::Per(1));
        r.rt.update(|s, _| s.toggle_leads());
        r.settle();
        assert!(!r.rt.state.leads_on());
        assert_eq!(r.rt.state.pages.list.iter().map(|p| p.tile.clone()).collect::<Vec<_>>(), pages.list.iter().map(|p| p.tile.clone()).collect::<Vec<_>>());
        assert_eq!((r.rt.state.pages.per, r.rt.state.pages.current), (pages.per, pages.current));
        assert_eq!(r.rt.state.focused_uid(), focus);
        r.check_kept(&before, "leads view and back");
        assert!(opened.iter().all(|u| r.rt.state.index_of(*u).is_none()), "the view's own pane closed");
    }

    #[test]
    fn quitting_in_the_view_keeps_the_layout_before_it() {
        let mut r = rig();
        let kept = r.rt.state.store.get("layout").clone();
        r.rt.update(|s, _| s.toggle_leads());
        r.settle();
        assert!(r.rt.state.leads_on());
        assert_eq!(r.rt.state.store.get("layout")["pages"], kept["pages"], "the state file holds the layout before the view");
        assert_eq!(r.rt.state.store.get("layout")["panes"], kept["panes"]);
    }

    #[test]
    fn no_team_no_view() {
        let mut r = Rig::new(&["a", "b"], 1284.0, 816.0);
        r.rt.update(|s, _| s.toggle_leads());
        assert!(!r.rt.state.leads_on());
        let e = r.rt.state.rail_entries().0.into_iter().find(|e| e.key == "rail-leads").unwrap();
        assert!(e.disabled && e.help.contains("No team is running"));
    }
}
