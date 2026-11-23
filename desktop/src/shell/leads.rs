//! Lead Mode (the team leads view): the top bar's "Lead Mode" shows only
//! the team leaders' terminals, and pressed again puts back exactly the
//! layout that was there before. Sessions, as the base view, shows only the
//! leaders' sessions under the same state.
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
//!   (`App::save_layout`), never the view's, and `lead_mode: true`; at the
//!   next start Lead Mode comes back over that layout once the teams are
//!   read (and only if a team runs then).
//! * The top bar's Lead Mode (topbar.luau, the `team_leads` intent) and the
//!   palette turn it on and off; Sessions filters its list by the same state.

use super::*;

/// The state file's key: Lead Mode was on when the app quit (comes back at start).
pub const LEAD_KEY: &str = "lead_mode";

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
        self.leads_restore = false;
        self.store.set(LEAD_KEY, if self.leads_on() { J::Bool(true) } else { J::Null });
        // The base view drawn instead of the terminals (Sessions) filters by
        // the same state: it reads `data.app.leads` again (`<base>_refresh`).
        if let Some(b) = self.feats.base.clone() {
            let refresh = format!("{b}_refresh");
            if crate::intent::all().any(|s| s.name == refresh) {
                self.press_extra("leads", &refresh, &serde_json::json!({}));
            }
        }
    }

    fn enter_leads(&mut self) {
        let leaders = self.leaders();
        if leaders.is_empty() {
            return self.notify("No team is running", false);
        }
        // A leader shown in a page window stays there (one view per terminal, pagewin.rs).
        let leaders: Vec<String> = leaders.into_iter().filter(|l| self.shown_elsewhere(l, Mode::Agent, None).is_none()).collect();
        if leaders.is_empty() {
            return self.notify("The team leaders show in their page windows", false);
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
                Some((sb, mode, thread)) if running.contains(sb) && self.shown_elsewhere(sb, *mode, thread.as_deref()).is_none() => {
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

    /// The fixture's link answers on threads of its own (the first list, the
    /// first reads of every data key). Wait, on those threads and not on a
    /// clock, until they all landed, then detach the link: nothing arrives
    /// after the data a test sets, however loaded the machine (an `Up` or a
    /// list landing late used to undo `listed` and `sandboxes`).
    fn quiet(r: &mut Rig) {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let clock = r.clock;
            r.rt.update(|s, tx| {
                s.poll(clock, tx);
            });
            if r.rt.state.link_quiet() {
                break;
            }
            assert!(std::time::Instant::now() < end, "the fixture's link never settled");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        r.rt.update(|s, _| {
            s.drain(None, false);
            s.link = None;
        });
    }

    fn rig() -> Rig {
        let mut r = Rig::new(&["a", "b", "c", "d"], 1284.0, 816.0);
        quiet(&mut r);
        r.rt.update(|s, _| {
            s.sandboxes = ["a", "b", "c", "d", "e"].iter().map(|n| crate::client::Sandbox::new(n, "claude", "running")).collect();
            s.feats.data.insert("leads".into(), serde_json::json!(["a", "c", "e"]));
            s.feats.data.insert("plans".into(), serde_json::json!([]));
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
        assert_eq!(r.rt.state.screen_data()["app"]["leads"], serde_json::json!({ "on": false, "can": false }), "the top bar draws Lead Mode disabled");
        r.rt.update(|s, _| s.run_intent("x", Intent::TeamLeads { show: Some(true) }));
        assert!(!r.rt.state.leads_on(), "nothing to show: it stays off");
    }

    fn sess(sb: &str, thread: Option<&str>, id: &str, ts: u64) -> J {
        serde_json::json!({ "sandbox": sb, "thread": thread, "session_id": id, "state": "idle", "started_ms": 1, "ts": ts, "mode": "claude", "reply": "Done." })
    }

    fn rows(r: &Rig) -> Vec<String> {
        r.rt.state.screen_data()["monitor"]["rows"].as_array().map(|v| v.iter().map(|x| x["key"].as_str().unwrap_or("").to_string()).collect()).unwrap_or_default()
    }

    /// A thread closed elsewhere while Lead Mode parks its pane: the pane is
    /// dropped from the parked ones and from the layout to come back to, so
    /// leaving the view opens no dead terminal for it.
    #[test]
    fn a_thread_closed_elsewhere_leaves_no_parked_pane_behind() {
        let mut r = rig();
        r.rt.update(|s, _| {
            let i = s.panes.iter().position(|p| p.sandbox.as_deref() == Some("b")).unwrap();
            s.panes[i].thread = Some("t1".into());
            s.feats.data.insert("threads".into(), serde_json::json!({ "b": { "threads": [{ "name": "t1" }] } }));
            s.push_data();
        });
        r.settle();
        r.rt.update(|s, _| s.toggle_leads());
        r.settle();
        assert!(r.rt.state.leads_on());
        assert!(r.rt.state.leads.as_ref().unwrap().parked.iter().any(|p| p.thread.as_deref() == Some("t1")), "b's thread pane is parked");
        r.rt.update(|s, _| {
            s.feats_bus("thread-closed", r#"{"sandbox":"b","thread":"t1"}"#);
        });
        r.settle();
        assert!(r.rt.state.leads.as_ref().unwrap().parked.iter().all(|p| p.thread.as_deref() != Some("t1")), "dropped from the parked panes");
        r.rt.update(|s, _| s.toggle_leads());
        r.settle();
        assert!(!r.rt.state.leads_on());
        assert!(r.rt.state.panes.iter().all(|p| p.thread.as_deref() != Some("t1")), "no pane for the closed thread once back");
        let in_pages = r.rt.state.pages.all().len();
        assert_eq!(in_pages, r.rt.state.panes.len(), "the layout and the panes agree");
    }

    /// Lead Mode is one state for both base views: on from the top bar in
    /// Sessions, the list keeps the leaders, each with its threads under it
    /// (indented), and a selection filtered out moves to the first leader;
    /// the terminals behind show only the leaders' panes; TUI and back keep
    /// it; off, the full list and the selection come back. Kept in the state
    /// file while on. The leaders gone while it is on: the empty state.
    #[test]
    fn lead_mode_is_one_state_for_the_terminals_and_sessions() {
        use super::super::tests::tap;
        let mut r = rig();
        r.rt.update(|s, _| {
            s.feats.data.insert("sessions".into(), serde_json::json!([sess("a", None, "sa", 5), sess("a", Some("t1"), "sat", 4), sess("b", None, "sb", 9), sess("c", None, "sc", 3), sess("d", None, "sd", 8)]));
            s.push_data();
        });
        r.settle();
        tap(&mut r.rt, "top-view-gui");
        r.settle();
        assert_eq!(r.rt.state.feats.base.as_deref(), Some("monitor"));
        r.rt.update(|s, _| s.press_extra("test", "monitor_select", &serde_json::json!({ "key": "b:sb" })));
        r.settle();
        let all = rows(&r);
        assert_eq!(all.len(), 5);
        assert_eq!(r.rt.state.screen_data()["monitor"]["sel"], "b:sb");
        tap(&mut r.rt, "top-lead");
        r.settle();
        assert!(r.rt.state.leads_on(), "Lead Mode on from the top bar");
        assert_eq!(rows(&r), ["a:sa", "a::t1", "c:sc"], "each leader (the core's order), then its threads");
        let v = r.rt.state.screen_data()["monitor"].clone();
        assert_eq!(v["rows"][1]["indent"], true, "a thread under its leader");
        assert!(v["rows"][0].get("indent").is_none());
        assert_eq!(v["sel"], "a:sa", "b is not a leader: the first leader is selected");
        assert_eq!(r.rt.state.store.get(LEAD_KEY), &J::Bool(true), "kept for the next start");
        // The terminals behind: only the leaders'.
        let shown: std::collections::BTreeSet<String> = r.names().concat().into_iter().collect();
        assert_eq!(shown, ["a", "c", "e"].iter().map(|s| s.to_string()).collect(), "the TUI shows the leaders' panes");
        tap(&mut r.rt, "top-view-tui");
        r.settle();
        assert!(r.rt.state.leads_on() && r.rt.state.feats.base.is_none(), "TUI keeps Lead Mode");
        tap(&mut r.rt, "top-view-gui");
        r.settle();
        assert_eq!(rows(&r), ["a:sa", "a::t1", "c:sc"], "back in GUI: still filtered");
        tap(&mut r.rt, "top-lead");
        r.settle();
        assert!(!r.rt.state.leads_on());
        assert_eq!(rows(&r), all, "the full list again");
        assert_eq!(r.rt.state.screen_data()["monitor"]["sel"], "b:sb", "the selection from before");
        assert_eq!(r.rt.state.store.get(LEAD_KEY), &J::Null);
        // On again, then no team runs any more: the empty state, the filter still on.
        tap(&mut r.rt, "top-lead");
        r.settle();
        r.rt.update(|s, _| {
            s.feats.data.insert("leads".into(), serde_json::json!([]));
            s.press_extra("test", "monitor_refresh", &serde_json::json!({}));
        });
        r.settle();
        assert!(r.rt.state.leads_on());
        assert!(rows(&r).is_empty());
        assert!(r.rt.find("monitor-leads-empty").is_some(), "No team leader is running");
        assert!(r.rt.find("monitor-empty").is_none(), "not the no-session state");
    }

    /// Lead Mode on when the app quit comes back at the next start, once the
    /// teams are read, over the layout kept; with no team running then it
    /// stays off (and does not wait for one).
    #[test]
    fn lead_mode_comes_back_at_start_when_a_team_still_runs() {
        let mut r = rig();
        // The fixture's first lists in, then the running sandboxes again.
        r.rt.update(|s, tx| {
            s.poll(0.5, tx);
            s.sandboxes = ["a", "b", "c", "d", "e"].iter().map(|n| crate::client::Sandbox::new(n, "claude", "running")).collect();
        });
        r.rt.update(|s, tx| {
            s.store.set(LEAD_KEY, J::Bool(true));
            s.leads_restore = true;
            s.listed = true;
            s.poll(1.0, tx);
        });
        assert!(r.rt.state.leads_on() && !r.rt.state.leads_restore);
        let mut r = Rig::new(&["a", "b"], 1284.0, 816.0);
        quiet(&mut r);
        r.rt.update(|s, tx| {
            s.feats.data.remove("leads");
            s.feats.data.remove("plans");
            s.leads_restore = true;
            s.listed = true;
            s.poll(1.0, tx);
        });
        assert!(r.rt.state.leads_restore, "the teams are not read yet: it waits");
        r.rt.update(|s, tx| {
            s.feats.data.insert("leads".into(), serde_json::json!([]));
            s.feats.data.insert("plans".into(), serde_json::json!([]));
            s.poll(2.0, tx);
        });
        assert!(!r.rt.state.leads_on() && !r.rt.state.leads_restore, "no team: off, and it waits no more");
    }

    /// A team known only by its plan (a thread still at work, the sandbox
    /// not made lead): the made-lead list read first must not decide alone.
    #[test]
    fn lead_mode_waits_for_the_plans_as_well_as_the_leads() {
        let mut r = Rig::new(&["a", "b"], 1284.0, 816.0);
        quiet(&mut r);
        r.rt.update(|s, tx| {
            s.sandboxes = ["a", "b"].iter().map(|n| crate::client::Sandbox::new(n, "claude", "running")).collect();
            s.feats.data.remove("plans");
            s.feats.data.insert("leads".into(), serde_json::json!([]));
            s.store.set(LEAD_KEY, J::Bool(true));
            s.leads_restore = true;
            s.listed = true;
            s.poll(1.0, tx);
        });
        assert!(r.rt.state.leads_restore && !r.rt.state.leads_on(), "the plans are not read yet: it waits");
        r.rt.update(|s, tx| {
            s.feats.data.insert("plans".into(), serde_json::json!([{ "id": "p", "sandbox": "a", "state": "approved", "threads": [{ "state": "working" }] }]));
            s.poll(2.0, tx);
        });
        assert!(r.rt.state.leads_on() && !r.rt.state.leads_restore, "a team still runs by its plan: Lead Mode is back");
    }

    /// A lead's plan approved (here or elsewhere): its threads open no pane
    /// on their own and the focus stays; the thread is listed at once under
    /// its leader in the sidebar, and a click on that row opens its pane.
    #[test]
    fn a_plans_thread_opens_no_pane_until_its_row_is_clicked() {
        let mut r = rig();
        r.rt.update(|s, _| s.focus_pane(1));
        r.settle();
        let panes = r.rt.state.panes.iter().map(|p| p.uid).collect::<Vec<_>>();
        let focus = r.rt.state.focused_uid();
        let pending = serde_json::json!([{ "id": "p-1", "sandbox": "a", "state": "pending", "threads": [{ "name": "t1", "goal": "x" }] }]);
        let mut approved = pending.clone();
        approved[0]["state"] = serde_json::json!("approved");
        approved[0]["threads"][0]["state"] = serde_json::json!("starting");
        r.rt.update(|s, _| {
            s.feats.data.insert("plans".into(), pending.clone());
            s.feats.modal = None;
            s.feats.data.insert("plans".into(), approved.clone());
            s.feats.data.insert("threads".into(), serde_json::json!({ "a": { "threads": [{ "name": "t1", "open": true }] } }));
            s.feats_data_changed("plans", &pending, &approved);
            s.push_data();
        });
        r.settle();
        assert_eq!(r.rt.state.panes.iter().map(|p| p.uid).collect::<Vec<_>>(), panes, "no pane opened for the plan's thread");
        assert!(r.rt.state.panes.iter().all(|p| p.thread.is_none()));
        assert_eq!(r.rt.state.focused_uid(), focus, "the focus did not move");
        // The sidebar lists it under its leader, with the click that opens it.
        let rows = r.rt.state.screen_data()["app"]["extras"]["rows"]["a"].clone();
        let row = rows.as_array().and_then(|l| l.iter().find(|x| x["id"] == "t1").cloned()).unwrap_or_else(|| panic!("a row for t1: {rows}"));
        assert_eq!(row["intent"]["name"], "open_terminal");
        r.rt.update(|s, _| s.press_extra("sandboxes", row["intent"]["name"].as_str().unwrap(), &row["intent"]["args"]));
        r.settle();
        let i = r.rt.state.panes.iter().position(|p| p.sandbox.as_deref() == Some("a") && p.thread.as_deref() == Some("t1")).expect("the click opened t1's pane");
        assert_eq!(r.rt.state.focused, i, "and focused it");
    }
}
