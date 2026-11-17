//! The left rail: a fixed column (44 pt) at the far left, under the top
//! bar, always shown (Cmd/Ctrl+B hides the sidebar, not the rail).
//!
//! Top: Requests and Network, with their counts as small badges (amber when
//! something holds on the person), then Team leads (the leads view,
//! `leads.rs`; disabled when no team runs). The view switch (Tabs | Grid)
//! is the pager's alone. Bottom: Jarvis and
//! Settings (Setup). Each entry runs the intent its old top bar button ran
//! (`relay_open`, `network_show`, `toggle_jarvis_panel`, Setup's
//! `open_modal`); nothing new is asked of the daemon.
//!
//! 28 pt targets, icons coloured by meaning (requests and network blue,
//! Jarvis purple, settings neutral), a tooltip with the shortcut, the
//! focus ring, and Up/Down (Home/End) move the keyboard within the rail.
//!
//! What moved here leaves the top bar (shell.rs, `top_layout`, marks those
//! extras `in_rail`): no entry is shown twice.
//!
//! Keys: `rail`, `rail-requests`, `rail-network`, `rail-leads`,
//! `jarvis-toggle` (Jarvis's button keeps its key), `rail-setup`; a badge is
//! `<key>-badge`.

use super::*;

/// The rail's width.
pub const W: f32 = layout::RAIL_W;
/// Its buttons.
pub const TARGET: f32 = 28.0;

/// The top bar extras the rail draws instead.
pub const IN_RAIL: &[&str] = &["relay", "approvals", "setup"];

/// One rail entry.
pub struct Entry {
    pub key: &'static str,
    pub icon: String,
    pub help: String,
    pub keys: Vec<String>,
    /// The icon's colour by meaning.
    pub tone: &'static str,
    /// Marked (the current view; Jarvis open).
    pub on: bool,
    pub count: Option<u64>,
    /// The badge's tone: amber when something waits on the person.
    pub badge: &'static str,
    pub run: RailRun,
    /// Nothing to do now (no team running): drawn dim, no press.
    pub disabled: bool,
    /// Its zone: 0 activity (Requests, Network, Team leads), 1 browse (for
    /// later: commits, worktrees, files), 2 configure (Jarvis, Settings). A
    /// hairline parts two zones of the same end of the rail.
    pub zone: u8,
}

/// An entry's colour by meaning (0xRRGGBB), always shown: its tone, Jarvis's
/// own violet (`purple-alt`, so Team leads' lilac beside it is still told
/// apart, as are their shapes), Requests a quiet blue-grey while nothing
/// waits, Settings the bright text.
pub fn color_of(tone: &str) -> u32 {
    let v = |n: &str| theme::color_value(n).unwrap();
    match tone {
        "jarvis" => v("purple-alt"),
        "quiet-blue" => theme::over(v("blue"), 0.55, v("text-muted")),
        "neutral" => v("text-bright"),
        t => crate::screen::action_tone(t).0,
    }
}

/// The tint under an entry on hover, focus and while active.
pub fn tint_of(tone: &str) -> u32 {
    match tone {
        "jarvis" => theme::color_value("purple-tint").unwrap(),
        "quiet-blue" => crate::screen::action_tone("blue").1,
        "neutral" => theme::color_value("elevated").unwrap(),
        t => crate::screen::action_tone(t).1,
    }
}

#[derive(Clone)]
pub enum RailRun {
    Intent(String, J),
    /// The team leads view on and off (leads.rs).
    Leads,
}

impl App {
    /// The top bar's extra `id`, as the features made it (counts, tones).
    fn top_extra(&self, id: &str) -> Option<J> {
        let data = self.screens.iter().find(|x| x.name == "topbar").map(|t| t.data().clone())?;
        data["app"]["extras"]["topbar"].as_array()?.iter().find(|e| e["id"] == id).cloned()
    }

    /// A feature's shortcut that runs `intent`, as keycaps.
    fn shortcut_for(intent: &str) -> Vec<String> {
        crate::feat::FEATURES
            .iter()
            .flat_map(|f| f.shortcuts.iter())
            .find(|s| s.intent == intent)
            .map(|s| {
                let k = s.key.to_ascii_uppercase().to_string();
                if cfg!(target_os = "macos") { vec!["⌃".into(), "⌥".into(), k] } else { vec!["Alt".into(), k] }
            })
            .unwrap_or_default()
    }

    /// The rail's entries, top group then bottom group.
    pub fn rail_entries(&self) -> (Vec<Entry>, Vec<Entry>) {
        let mut top = vec![];
        let badge_of = |e: &Option<J>| -> (Option<u64>, &'static str) {
            match e {
                Some(e) => (e["count"].as_u64().filter(|n| *n > 0), if e["tone"] == "amber" || e["tone"] == "red" { "amber" } else { "blue" }),
                None => (None, "blue"),
            }
        };
        let relay = self.top_extra("relay");
        let (count, badge) = badge_of(&relay);
        let help = relay.as_ref().and_then(|e| e["help"].as_str().map(str::to_string)).unwrap_or_else(|| "Requests: what agents ask you, and answers for other sandboxes".into());
        let tone = if count.is_some() && badge == "amber" { "amber" } else if count.is_some() { "blue" } else { "quiet-blue" };
        top.push(Entry { key: "rail-requests", icon: "ask".into(), help: format!("Requests\nAgents waiting for your answer. {help}"), keys: App::shortcut_for("relay_open"), tone, on: self.feats.panel.as_deref() == Some("relay"), count, badge, run: RailRun::Intent("relay_open".into(), serde_json::json!({})), disabled: false, zone: 0 });
        let net = self.top_extra("approvals");
        let (count, badge) = badge_of(&net);
        let help = net.as_ref().and_then(|e| e["help"].as_str().map(str::to_string)).unwrap_or_else(|| "Network: what sbx asks you to allow, the hosts sandboxes reach".into());
        top.push(Entry { key: "rail-network", icon: "shield".into(), help: format!("Network\nWhat sbx asks you to allow, the hosts sandboxes reach. {help}"), keys: App::shortcut_for("network_show"), tone: "blue", on: self.feats.panel.as_deref() == Some("network"), count, badge, run: RailRun::Intent("network_show".into(), serde_json::json!({})), disabled: false, zone: 0 });
        // The team leads view (leads.rs): only the leaders' terminals.
        let leaders = self.leaders();
        let help = if self.leads_on() {
            "Team leads\nShowing only the team leaders' terminals. Press for the layout before".to_string()
        } else if leaders.is_empty() {
            "Team leads\nNo team is running".to_string()
        } else {
            format!("Team leads\nOnly the {} team leader{}' terminals; press again for this layout", leaders.len(), if leaders.len() == 1 { "" } else { "s" })
        };
        top.push(Entry { key: "rail-leads", icon: "people".into(), help, keys: vec![], tone: "purple", on: self.leads_on(), count: None, badge: "", run: RailRun::Leads, disabled: leaders.is_empty() && !self.leads_on(), zone: 0 });
        let pending = self.jarvis.pending_list().len();
        let mut bottom = vec![Entry {
            key: "jarvis-toggle",
            icon: if self.jarvis.recording { "mic" } else { "jarvis" }.into(),
            help: if self.jarvis.open { "Close Jarvis\nYour voice and chat concierge".into() } else if pending > 0 { format!("Open Jarvis\n{pending} to review") } else { "Open Jarvis\nYour voice and chat concierge".into() },
            keys: if cfg!(target_os = "macos") { palette::chord_keys("J") } else { vec!["F7".into()] },
            tone: "jarvis",
            on: self.jarvis.open,
            count: (pending > 0).then_some(pending as u64),
            badge: "amber",
            run: RailRun::Intent("toggle_jarvis_panel".into(), serde_json::json!({})),
            disabled: false,
            zone: 2,
        }];
        if let Some(e) = self.top_extra("setup") {
            if let Some(name) = e["intent"]["name"].as_str() {
                bottom.push(Entry { key: "rail-setup", icon: "gear".into(), help: format!("Settings\n{}", e["help"].as_str().unwrap_or("Setup")), keys: App::shortcut_for(name), tone: "neutral", on: self.feats.modal.as_ref().is_some_and(|m| m.0 == "setup"), count: None, badge: "", run: RailRun::Intent(name.to_string(), e["intent"]["args"].clone()), disabled: false, zone: 2 });
            }
        }
        (top, bottom)
    }

    /// Whether the rail draws an element of that key (`wait:` marks).
    pub(super) fn rail_drawn(&self, key: &str) -> bool {
        if key == "rail" {
            return true;
        }
        let (a, b) = self.rail_entries();
        a.iter().chain(&b).any(|e| key == e.key || (e.count.is_some() && key == format!("{}-badge", e.key)))
    }
}

/// One 28 pt button.
/// The icon's size in its 28 pt target.
pub const ICON: f32 = 20.0;

fn button(e: &Entry, prev: Option<&'static str>, next: Option<&'static str>) -> El {
    let (fg, tint) = (color_of(e.tone), tint_of(e.tone));
    let run = e.run.clone();
    let disabled = e.disabled;
    let (label, line2) = e.help.split_once('\n').unwrap_or((e.help.as_str(), ""));
    let mut b = el()
        .key(e.key)
        .role("button")
        .label(if line2.is_empty() { label.to_string() } else { format!("{label}: {line2}") })
        .help(e.help.clone())
        .help_keys(e.keys.clone())
        .w(TARGET)
        .h(TARGET)
        .items_center()
        .justify_center()
        .radius(theme::radius("sm").unwrap())
        .hover(theme::hex(tint))
        .focus_ring(2.0, 1.0, c("focus"))
        .cursor(Cursor::Pointer)
        .on_press(move |s: &mut App| {
            if disabled {
                return;
            }
            match run.clone() {
                RailRun::Intent(name, args) => s.press_extra("rail", &name, &args),
                RailRun::Leads => s.toggle_leads(),
            }
            s.push_data();
        })
        .on_key(move |_s: &mut App, ev: &KeyEvent, tx: &mut Tx| {
            let to = match ev.key {
                Key::Up | Key::Left => prev,
                Key::Down | Key::Right => next,
                _ => return false,
            };
            if let Some(k) = to {
                tx.focus(k, true);
            }
            true
        });
    if e.disabled {
        b = b.opacity(theme::DISABLED_OPACITY).cursor(Cursor::NotAllowed);
    }
    if e.on {
        // Active: a filled tile of its tint and a 2 pt bar on the rail's edge.
        b = b.fill(theme::hex(tint)).checked(true).child(el().key(format!("{}-on", e.key)).absolute(-(W - TARGET) / 2.0 + 1.0, 4.0).w(2.0).h(TARGET - 8.0).radius(1.0).fill(theme::hex(fg)));
    }
    b = b.child(rune::icon(crate::icons::get(&e.icon).unwrap_or_else(|| crate::icons::get("single").unwrap()), ICON).color(theme::hex(fg)));
    if let Some(n) = e.count {
        // The badge: the tone filled, its number dark, a dark ring so it
        // reads over the coloured icon.
        let (bf, _) = crate::screen::tone(e.badge);
        let label = if n > 99 { "99+".to_string() } else { n.to_string() };
        b = b.child(
            el().key(format!("{}-badge", e.key))
                .pin(None, Some(-6.0), Some(-8.0), None)
                .h(16.0)
                .min_w(16.0)
                .px(3.0)
                .items_center()
                .justify_center()
                .radius(8.0)
                .fill(theme::hex(bf))
                .border(2.0, c("bg"))
                .child(text(label).font(small(10.0, 700.0)).color(c("bg"))),
        );
    }
    b
}

/// The rail.
pub fn view(s: &App) -> El {
    let (top, bottom) = s.rail_entries();
    let keys: Vec<&'static str> = top.iter().chain(&bottom).map(|e| e.key).collect();
    let around = |k: &'static str| {
        let i = keys.iter().position(|x| *x == k).unwrap_or(0);
        (i.checked_sub(1).map(|j| keys[j]), keys.get(i + 1).copied())
    };
    let mut col_ = col().key("rail").role("toolbar").label("Views and panels").w(W).h_full().min_h(0.0).py(8.0).gap(6.0).items_center().fill(c("bg")).border(1.0, c("elevated")).radius(theme::radius("md").unwrap());
    // Zones, a hairline between two (activity, then browse when it comes).
    let mut zone = None;
    for e in top.iter() {
        if zone.is_some_and(|z| z != e.zone) {
            col_ = col_.child(el().key(format!("rail-zone-{}", e.zone)).w(20.0).h(1.0).fill(c("elevated")));
        }
        zone = Some(e.zone);
        let (p, n) = around(e.key);
        col_ = col_.child(button(e, p, n));
    }
    col_ = col_.child(el().grow(1.0));
    for e in &bottom {
        let (p, n) = around(e.key);
        col_ = col_.child(button(e, p, n));
    }
    col_
}

#[cfg(test)]
mod tests {
    use super::super::layout_tests::Rig;
    use super::*;

    /// Each entry's colour marks at 3:1 on the rail and on its active tile;
    /// a badge's number reads on its fill.
    #[test]
    fn rail_colours_read_at_rest_and_active() {
        let bg = theme::color_value("bg").unwrap();
        for t in ["amber", "blue", "quiet-blue", "purple", "jarvis", "neutral"] {
            for (what, ground) in [("rail", bg), ("active", tint_of(t))] {
                let r = theme::contrast(color_of(t), ground);
                assert!(r >= 3.0, "{t} on {what}: {r:.2}");
            }
        }
        for b in ["amber", "blue"] {
            let r = theme::contrast(bg, crate::screen::tone(b).0);
            assert!(r >= 4.5, "badge {b}: {r:.2}");
        }
        assert_ne!(color_of("purple"), color_of("jarvis"), "Team leads and Jarvis are told apart");
    }

    /// How many 28 pt entries the rail holds at a 720 pt high window before
    /// it must scroll (printed for the report).
    #[test]
    fn entries_that_fit_at_720() {
        let r = Rig::new(&["a"], 1100.0, 720.0);
        let (rail, _) = r.rt.presented("rail").unwrap();
        let fit = ((rail[3] - 16.0 + 6.0) / (TARGET + 6.0)).floor() as usize;
        eprintln!("rail at 720: {} pt high, {fit} entries of 28 pt (6 pt apart), minus one per hairline", rail[3]);
        assert!(fit >= 10);
    }

    #[test]
    fn the_rail_switches_the_view_and_opens_what_the_top_bar_opened() {
        for (w, h) in [(720.0, 480.0), (1284.0, 816.0), (1800.0, 1000.0)] {
            let r = Rig::new(&["a", "b", "c"], w, h);
            let (rail, _) = r.rt.presented("rail").unwrap();
            assert!((rail[2] - W).abs() < 0.5, "{w}: 44 pt wide");
            for k in ["rail-requests", "rail-network", "jarvis-toggle", "rail-setup"] {
                let (b, _) = r.rt.presented(k).unwrap_or_else(|| panic!("{w}: {k}"));
                assert!(b[2] >= TARGET - 0.5 && b[3] >= TARGET - 0.5, "{w}: {k} a 28 pt target");
                assert!(b[0] >= rail[0] && b[0] + b[2] <= rail[0] + rail[2] && b[1] + b[3] <= rail[1] + rail[3] + 0.5, "{w}: {k} {b:?} inside the rail {rail:?}");
                assert!(r.rt.state.drawn(k));
            }
            assert!(r.rt.presented("rail-grid").is_none() && r.rt.presented("rail-tabs").is_none(), "{w}: the view switch is the pager's alone");
            // Under the top bar, left of the sidebar.
            let (top, _) = r.rt.presented("top-bar").unwrap();
            let (side, _) = r.rt.presented("sidebar").unwrap();
            assert!(rail[1] >= top[1] + top[3] && rail[0] + rail[2] <= side[0], "{w}: {rail:?} {top:?} {side:?}");
            // Nothing twice: the rail's entries are not in the top bar.
            for k in ["extra-topbar-relay", "extra-topbar-approvals", "extra-topbar-setup"] {
                assert!(r.rt.presented(k).is_none(), "{w}: {k} is in the rail now");
            }
        }
        let mut r = Rig::new(&["a", "b", "c"], 1284.0, 816.0);
        super::super::tests::tap(&mut r.rt, "jarvis-toggle");
        assert!(r.rt.state.jarvis.open);
        super::super::tests::tap(&mut r.rt, "jarvis-toggle");
        assert!(!r.rt.state.jarvis.open);
        // Sidebar hidden: the rail stays.
        r.rt.update(|s, _| s.toggle_sidebar());
        r.settle();
        assert!(r.rt.presented("rail").is_some() && r.rt.presented("sidebar").is_none());
        // With requests waiting: the counts are the rail's badges, and the top
        // bar draws neither them nor Setup (not even as its fallback strip).
        let a = App::new(Args { fixture: Some(format!("{}/fixtures/shell/panebar-4.json", env!("CARGO_MANIFEST_DIR"))), ..Args::default() });
        let mut rt = rune::Runtime::new(a, super::super::view, (1284.0, 816.0), rune::Env::new(true, 1.0));
        for _ in 0..10 {
            rt.frame(0.016);
        }
        assert!(rt.presented("rail-requests-badge").is_some() && rt.presented("rail-network-badge").is_some());
        for id in IN_RAIL {
            assert!(rt.presented(&format!("extra-topbar-{id}")).is_none() && rt.presented(&format!("screen-extras-topbar--{id}")).is_none(), "{id} drawn twice");
        }
        let req = rt.state.rail_entries().0.into_iter().find(|e| e.key == "rail-requests").unwrap();
        assert_eq!((req.count, req.badge), (Some(3), "amber"), "3 agents' requests wait");
        super::super::tests::tap(&mut rt, "rail-requests");
        assert_eq!(rt.state.feats.panel.as_deref(), Some("relay"));
        super::super::tests::tap(&mut rt, "rail-setup");
        assert_eq!(rt.state.feats.modal.as_ref().map(|m| m.0.as_str()), Some("setup"));
        // Arrow keys move within the rail.
        r.rt.update(|_, tx| tx.focus("rail-requests", true));
        r.rt.frame(0.016);
        r.rt.key(KeyEvent { key: Key::Down, mods: rune::Mods::default() });
        r.rt.frame(0.016);
        r.rt.update(|_, tx| assert_eq!(tx.focused(), Some("rail-network")));
    }
}
