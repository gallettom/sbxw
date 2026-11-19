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
use crate::feat::{Place, Zone};

/// The rail's width.
pub const W: f32 = layout::RAIL_W;
/// Its buttons.
pub const TARGET: f32 = 28.0;

/// The top bar extras the rail draws instead.
pub const IN_RAIL: &[&str] = &["relay", "approvals", "setup"];

/// One rail entry.
pub struct Entry {
    pub key: String,
    pub icon: String,
    pub help: String,
    pub keys: Vec<String>,
    /// The icon's colour by meaning.
    pub tone: String,
    /// Marked (the current view; Jarvis open).
    pub on: bool,
    pub count: Option<u64>,
    /// The badge's tone: amber when something waits on the person.
    pub badge: &'static str,
    pub run: RailRun,
    /// Nothing to do now (no team running): drawn dim, no press.
    pub disabled: bool,
    /// Its zone (`feat::Zone`); `None`: Jarvis, apart at the very bottom.
    pub zone: Option<Zone>,
    /// Its place within the zone, lower first (Settings: `u16::MAX`).
    pub order: u16,
    /// A status dot's tone (`Place::RailStatus`): `muted` is a hollow ring.
    pub dot: Option<String>,
}

/// The rail as drawn: the top block (activity, then browse), the bottom
/// block (configure, Settings last, then Jarvis apart), and what folded into
/// the "More" entry because the rail is too short.
pub struct RailLayout {
    pub top: Vec<Entry>,
    pub bottom: Vec<Entry>,
    pub folded: Vec<Entry>,
}

/// The height `n` entries and `lines` hairlines take in the rail (its
/// padding included), as `view` lays them out.
pub fn height_for(n: usize, lines: usize) -> f32 {
    let items = n + lines;
    16.0 + n as f32 * TARGET + lines as f32 + items.saturating_sub(1) as f32 * 6.0
}

/// How many hairlines a list of zones draws (one between two zones).
fn lines_in(zones: &[Option<Zone>]) -> usize {
    zones.windows(2).filter(|w| w[0] != w[1]).count()
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
    /// A feature's entry: its intent, or, while its screen shows (a panel,
    /// an overlay, a modal), closing that screen.
    Toggle { name: String, args: J, screen: String },
    /// The team leads view on and off (leads.rs).
    Leads,
    /// The "More" entry: the folded entries' menu.
    More,
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

    /// Every rail entry: the core's own (Requests, Network, Team leads,
    /// Settings, Jarvis) and the features' `Place::Rail` extras, each with
    /// its zone and order. Top block (activity, browse) then bottom block
    /// (configure, Settings last, then Jarvis), each sorted.
    pub fn rail_entries(&self) -> (Vec<Entry>, Vec<Entry>) {
        let mut all = vec![];
        let badge_of = |e: &Option<J>| -> (Option<u64>, &'static str) {
            match e {
                Some(e) => (e["count"].as_u64().filter(|n| *n > 0), if e["tone"] == "amber" || e["tone"] == "red" { "amber" } else { "blue" }),
                None => (None, "blue"),
            }
        };
        let relay = self.top_extra("relay");
        let (count, badge) = badge_of(&relay);
        let help = relay.as_ref().and_then(|e| e["help"].as_str().map(str::to_string)).filter(|h| !h.is_empty()).unwrap_or_else(|| "Agents waiting for your answer: questions, screenshots, choices".into());
        // The feature's own sentence ("Agents wait for you: ...") is the line; it is not said twice.
        let help = if help.starts_with("Agents wait for you") { "Agents waiting for your answer: questions, screenshots, choices".to_string() } else { help };
        let tone = if count.is_some() && badge == "amber" { "amber" } else if count.is_some() { "blue" } else { "quiet-blue" };
        all.push(Entry { key: "rail-requests".into(), icon: "ask".into(), help: format!("Requests\n{help}"), keys: App::shortcut_for("relay_open"), tone: tone.into(), on: self.feats.panel.as_deref() == Some("relay"), count, badge, run: RailRun::Intent("relay_open".into(), serde_json::json!({})), disabled: false, zone: Some(Zone::Activity), order: 10, dot: None });
        let net = self.top_extra("approvals");
        let (count, badge) = badge_of(&net);
        let help = net.as_ref().and_then(|e| e["help"].as_str().map(str::to_string)).filter(|h| !h.is_empty()).unwrap_or_else(|| "What sbx asks you to allow, the hosts sandboxes reach".into());
        all.push(Entry { key: "rail-network".into(), icon: "shield".into(), help: format!("Network\n{help}"), keys: App::shortcut_for("network_show"), tone: "blue".into(), on: self.feats.panel.as_deref() == Some("network"), count, badge, run: RailRun::Intent("network_show".into(), serde_json::json!({})), disabled: false, zone: Some(Zone::Activity), order: 20, dot: None });
        // The team leads view (leads.rs): only the leaders' terminals.
        let leaders = self.leaders();
        let help = if self.leads_on() {
            "Team leads\nShowing only the team leaders' terminals. Press for the layout before".to_string()
        } else if leaders.is_empty() {
            "Team leads\nNo team is running".to_string()
        } else {
            format!("Team leads\nOnly the {} team leader{}' terminals; press again for this layout", leaders.len(), if leaders.len() == 1 { "" } else { "s" })
        };
        all.push(Entry { key: "rail-leads".into(), icon: "leader".into(), help, keys: vec![], tone: "purple".into(), on: self.leads_on(), count: None, badge: "", run: RailRun::Leads, disabled: leaders.is_empty() && !self.leads_on(), zone: Some(Zone::Activity), order: 30, dot: None });
        // The features' entries (`Place::Rail`).
        for e in &self.feats.pane_extras {
            let Place::Rail { zone, order } = e.place else { continue };
            let Some((name, args)) = e.intent.clone() else { continue };
            // The screen it shows: its `open_panel`/`open_modal` argument, else
            // the screen named as the entry (a feature's own opening intent).
            let screen = args["screen"].as_str().unwrap_or(&e.id).to_string();
            // Lit while it shows, and while it is the base view under an overlay.
            let on = self.screen_shown(&screen) || self.feats.base.as_deref() == Some(screen.as_str());
            let help = if e.help.is_empty() { e.label.clone() } else { format!("{}\n{}", e.label, e.help) };
            let badge = if e.tone == "amber" || e.tone == "red" { "amber" } else { "blue" };
            let shortcut = App::shortcut_for(&name);
            let dot = self.feats.pane_extras.iter().find(|d| d.place == Place::RailStatus(e.id.clone())).map(|d| d.tone.clone());
            all.push(Entry { key: format!("rail-{}", e.id), icon: e.icon.clone().unwrap_or_else(|| "single".into()), help, keys: shortcut, tone: if e.tone.is_empty() { "neutral".into() } else { e.tone.clone() }, on, count: e.count.filter(|n| *n > 0), badge, run: RailRun::Toggle { name, args, screen }, disabled: false, zone: Some(zone), order, dot });
        }
        if let Some(e) = self.top_extra("setup") {
            if let Some(name) = e["intent"]["name"].as_str() {
                all.push(Entry { key: "rail-setup".into(), icon: "gear".into(), help: format!("Settings\n{}", e["help"].as_str().unwrap_or("Setup")), keys: App::shortcut_for(name), tone: "neutral".into(), on: self.feats.modal.as_ref().is_some_and(|m| m.0 == "setup"), count: None, badge: "", run: RailRun::Intent(name.to_string(), e["intent"]["args"].clone()), disabled: false, zone: Some(Zone::Configure), order: u16::MAX, dot: None });
            }
        }
        let pending = self.jarvis.pending_list().len();
        all.push(Entry {
            key: "jarvis-toggle".into(),
            icon: if self.jarvis.recording { "mic" } else { "jarvis" }.into(),
            help: if self.jarvis.open { "Close Jarvis\nYour voice and chat concierge".into() } else if pending > 0 { format!("Open Jarvis\n{pending} to review") } else { "Open Jarvis\nYour voice and chat concierge".into() },
            keys: if cfg!(target_os = "macos") { palette::chord_keys("J") } else { vec!["F7".into()] },
            tone: "jarvis".into(),
            on: self.jarvis.open,
            count: (pending > 0).then_some(pending as u64),
            badge: "amber",
            run: RailRun::Intent("toggle_jarvis_panel".into(), serde_json::json!({})),
            disabled: false,
            zone: None,
            order: 0,
            dot: None,
        });
        // Jarvis (no zone) sorts last: `None` is mapped after every zone.
        all.sort_by_key(|e| (e.zone.map_or(9, |z| z as u8), e.order));
        let (top, bottom): (Vec<Entry>, Vec<Entry>) = all.into_iter().partition(|e| matches!(e.zone, Some(Zone::Activity | Zone::Browse)));
        (top, bottom)
    }

    /// The rail as it fits its height: when every entry does not fit, the
    /// configure zone's entries (not Settings) fold into "More", then the
    /// browse zone's. The activity zone, Settings and Jarvis never fold.
    pub fn rail_layout(&self) -> RailLayout {
        let (mut top, mut bottom) = self.rail_entries();
        let room = self.rects.rail.map(|r| r[3]).unwrap_or(f32::MAX);
        let need = |top: &[Entry], bottom: &[Entry], more: bool| {
            let mut zones: Vec<Option<Zone>> = top.iter().map(|e| e.zone).collect();
            let lines_top = lines_in(&zones);
            zones = bottom.iter().map(|e| e.zone).collect();
            if more {
                zones.insert(0, Some(Zone::Configure));
            }
            let lines_bottom = lines_in(&zones);
            // The spacer between the blocks is one more gap.
            height_for(top.len() + bottom.len() + more as usize, lines_top + lines_bottom) + 6.0
        };
        let mut folded = vec![];
        if need(&top, &bottom, false) > room {
            let (f, keep): (Vec<Entry>, Vec<Entry>) = bottom.into_iter().partition(|e| e.zone == Some(Zone::Configure) && e.key != "rail-setup");
            folded = f;
            bottom = keep;
            if need(&top, &bottom, true) > room {
                let (f, keep): (Vec<Entry>, Vec<Entry>) = top.into_iter().partition(|e| e.zone == Some(Zone::Browse));
                folded.splice(0..0, f);
                top = keep;
            }
        }
        if !folded.is_empty() {
            let n: u64 = folded.iter().filter_map(|e| e.count).sum();
            let badge = if folded.iter().any(|e| e.count.is_some() && e.badge == "amber") { "amber" } else { "blue" };
            let names: Vec<&str> = folded.iter().map(|e| e.help.split('\n').next().unwrap_or("")).collect();
            let more = Entry { key: "rail-more".into(), icon: "more".into(), help: format!("More\n{}", names.join(", ")), keys: vec![], tone: "neutral".into(), on: self.rail_menu || folded.iter().any(|e| e.on), count: (n > 0).then_some(n), badge, run: RailRun::More, disabled: false, zone: Some(Zone::Configure), order: 0, dot: None };
            bottom.insert(0, more);
        }
        RailLayout { top, bottom, folded }
    }

    /// Whether the rail draws an element of that key (`wait:` marks).
    pub(super) fn rail_drawn(&self, key: &str) -> bool {
        if key == "rail" {
            return true;
        }
        let l = self.rail_layout();
        let badge = |e: &Entry| (e.count.is_some() && key == format!("{}-badge", e.key)) || (e.dot.is_some() && key == format!("{}-dot", e.key));
        if l.top.iter().chain(&l.bottom).any(|e| key == e.key || badge(e)) {
            return true;
        }
        self.rail_menu && (key == "rail-more-list" || l.folded.iter().any(|e| key == e.key))
    }

    /// The "More" entry's menu: the folded entries, anchored right of it.
    pub(super) fn rail_menu_view(&self) -> Option<El> {
        if !self.rail_menu {
            return None;
        }
        let l = self.rail_layout();
        if l.folded.is_empty() {
            return None;
        }
        let labels: Vec<String> = l.folded.iter().map(|e| e.help.split('\n').next().unwrap_or("").to_string()).collect();
        let rows: Vec<screen::MenuRow> = l
            .folded
            .iter()
            .zip(&labels)
            .map(|(e, label)| screen::MenuRow { key: e.key.clone(), label, icon: Some(e.icon.as_str()), tone: Some(e.tone.as_str()), help: e.help.split_once('\n').map(|x| x.1), keys: &e.keys, count: e.count, chosen: e.on, disabled: e.disabled })
            .collect();
        let mut list = screen::menu_surface("rail-more-list".into(), "rail-more".into(), rune::Align::End, screen::menu_width(&rows)).anchored("rail-more", rune::Side::Right, rune::Align::End, 6.0);
        let mut zone = l.folded.first().and_then(|e| e.zone);
        for (e, r) in l.folded.iter().zip(&rows) {
            if e.zone != zone {
                list = list.child(screen::menu_divider());
                zone = e.zone;
            }
            let run = e.run.clone();
            list = list.child(screen::menu_row(r).on_press(move |s: &mut App| {
                s.rail_menu = false;
                s.rail_run(run.clone());
                s.push_data();
            }));
        }
        Some(list)
    }

    /// Whether that screen shows: the docked panel shown, the overlay, the modal.
    pub fn screen_shown(&self, screen: &str) -> bool {
        self.feats.panel.as_deref() == Some(screen) || self.feats.overlay.as_ref().is_some_and(|o| o.0 == screen) || self.feats.modal.as_ref().is_some_and(|m| m.0 == screen)
    }

    /// A rail entry's press. An entry whose screen shows closes it.
    pub(super) fn rail_run(&mut self, run: RailRun) {
        match run {
            RailRun::Intent(name, args) => self.press_extra("rail", &name, &args),
            RailRun::Toggle { name, args, screen } => {
                if self.feats.modal.as_ref().is_some_and(|m| m.0 == screen) {
                    self.press_extra("rail", "close_modal", &serde_json::json!({}));
                } else if self.screen_shown(&screen) {
                    self.press_extra("rail", "close_panel", &serde_json::json!({ "screen": screen }));
                } else {
                    self.press_extra("rail", &name, &args);
                }
            }
            RailRun::Leads => self.toggle_leads(),
            RailRun::More => self.rail_menu = !self.rail_menu,
        }
    }
}

/// One 28 pt button.
/// The icon's size in its 28 pt target.
pub const ICON: f32 = theme::icon::LARGE;

fn button(e: &Entry, prev: Option<String>, next: Option<String>) -> El {
    let (fg, tint) = (color_of(&e.tone), tint_of(&e.tone));
    let run = e.run.clone();
    let disabled = e.disabled;
    let (label, line2) = e.help.split_once('\n').unwrap_or((e.help.as_str(), ""));
    let mut b = el()
        .key(e.key.clone())
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
            s.rail_run(run.clone());
            s.push_data();
        })
        .on_key(move |_s: &mut App, ev: &KeyEvent, tx: &mut Tx| {
            let to = match ev.key {
                Key::Up | Key::Left => prev.clone(),
                Key::Down | Key::Right => next.clone(),
                _ => return false,
            };
            if let Some(k) = &to {
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
    if let Some(t) = &e.dot {
        // The status dot: lower right, a dark ring so it reads on any tile;
        // `muted` is a hollow grey ring (off, not set up).
        let d = el().key(format!("{}-dot", e.key)).pin(None, None, Some(-3.0), Some(-3.0)).w(10.0).h(10.0).radius(5.0);
        b = b.child(if t == "muted" { d.fill(c("bg")).border(2.0, c("text-muted")) } else { d.fill(c(t)).border(2.0, c("bg")) });
    }
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

/// The rail: the top block, a spacer, the bottom block; a hairline
/// between two zones; the column reports its height so the layout knows
/// what folds.
pub fn view(s: &App) -> El {
    let l = s.rail_layout();
    let keys: Vec<String> = l.top.iter().chain(&l.bottom).map(|e| e.key.clone()).collect();
    let around = |k: &str| {
        let i = keys.iter().position(|x| x == k).unwrap_or(0);
        (i.checked_sub(1).map(|j| keys[j].clone()), keys.get(i + 1).cloned())
    };
    let mut col_ = col()
        .key("rail")
        .role("toolbar")
        .label("Views and panels")
        .w(W)
        .h_full()
        .min_h(0.0)
        .py(8.0)
        .gap(6.0)
        .items_center()
        .clip_rounded(theme::radius("md").unwrap())
        .fill(c("bg"))
        .border(1.0, c("elevated"))
        .radius(theme::radius("md").unwrap())
        .on_layout(|s: &mut App, r: [f32; 4]| {
            if s.rects.rail.is_none_or(|o| (o[3] - r[3]).abs() > 0.5) {
                s.rects.rail = Some(r);
                s.push_data();
            }
        });
    let zone_name = |z: Option<Zone>| match z {
        Some(Zone::Activity) => "activity",
        Some(Zone::Browse) => "browse",
        Some(Zone::Configure) => "configure",
        None => "jarvis",
    };
    let block = |mut col_: El, list: &[Entry]| {
        let mut zone: Option<Option<Zone>> = None;
        for e in list {
            if zone.is_some_and(|z| z != e.zone) {
                col_ = col_.child(el().key(format!("rail-zone-{}", zone_name(e.zone))).w(20.0).h(1.0).fill(c("elevated")));
            }
            zone = Some(e.zone);
            let (p, n) = around(&e.key);
            col_ = col_.child(button(e, p, n));
        }
        col_
    };
    col_ = block(col_, &l.top);
    col_ = col_.child(el().grow(1.0));
    block(col_, &l.bottom)
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

    /// The sbxw mark: without window buttons (full screen, Linux, Windows)
    /// it is centred on the rail's icon column; in a macOS window it sits
    /// right after the buttons, as before. Wide and narrow (points: the
    /// scale does not move it; the 2x shots in views2/logo show it).
    #[test]
    fn the_mark_sits_on_the_rail_axis_without_window_buttons() {
        for (w, h) in [(1284.0, 816.0), (900.0, 600.0)] {
            {
                let scale = 1.0;
                let mut r = Rig::new(&["a"], w, h);
                let mark = r.rt.presented("app-mark").expect("the mark").0;
                let entry = r.rt.presented("rail-requests").expect("a rail icon").0;
                let rail = r.rt.presented("rail").expect("the rail").0;
                let (mc, ec, rc) = (mark[0] + mark[2] / 2.0, entry[0] + entry[2] / 2.0, rail[0] + rail[2] / 2.0);
                assert!((mark[2] - crate::shell::MARK_W).abs() < 0.5, "the mark's box is {} wide", mark[2]);
                assert!((mc - ec).abs() <= 0.5 && (mc - rc).abs() <= 0.5, "{w}x{h} @{scale}: mark centre {mc}, rail icon {ec}, rail {rc}");
                // In a macOS window (buttons up to x 76 from the window's edge): after them.
                r.rt.set_titlebar_inset(76.0);
                r.settle();
                let mark = r.rt.presented("app-mark").expect("the mark").0;
                assert!((mark[0] - (76.0 + 12.0)).abs() <= 1.5, "after the window buttons, 12 pt clear: x {}", mark[0]);
            }
        }
    }

    /// Team leads wears the `leader` mark (a crown over one person) in the
    /// rail and the palette, never the team's `people` the pane bar's Team
    /// keeps; its tooltip's second line stays in words.
    #[test]
    fn team_leads_has_its_own_mark() {
        let r = Rig::new(&["a"], 1284.0, 816.0);
        let s = &r.rt.state;
        let l = s.rail_layout();
        let e = l.top.iter().find(|e| e.key == "rail-leads").expect("the leads entry");
        assert_eq!(e.icon, "leader");
        assert!(crate::icons::exists("leader") && crate::icons::exists("people"));
        assert_ne!(crate::icons::get("leader").unwrap().body, crate::icons::get("people").unwrap().body);
        assert!(e.help.chars().any(|c| c.is_alphabetic()), "line 2 in words: {:?}", e.help);
        let src = include_str!("palette.rs");
        assert!(src.contains("hint, \"leader\""), "the palette's Team leads uses the same mark");
        assert!(include_str!("pager.rs").contains("small_icon(\"leader\""), "the pager's Team leads mark too");
    }

    /// Zones in order (activity, browse; then configure with Settings last,
    /// then Jarvis apart), a hairline between two; features' entries by
    /// their `order`; too short a rail folds the configure zone first, then
    /// the browse zone, into "More", whose menu runs the folded entry.
    #[test]
    fn zones_order_hairlines_and_the_configure_zone_folds_first() {
        use crate::feat::Extra;
        let mut r = Rig::new(&["a"], 1284.0, 816.0);
        let x = |id: &str, zone, order| Extra { place: Place::Rail { zone, order }, id: id.into(), label: id.to_uppercase(), icon: Some("plus".into()), tone: "blue".into(), help: "line".into(), count: None, intent: Some(("open_panel".into(), serde_json::json!({ "screen": id }))) };
        let s = &mut r.rt.state;
        s.feats.pane_extras.retain(|e| !matches!(e.place, Place::Rail { .. } | Place::RailStatus(_)));
        s.feats.pane_extras.extend([x("kits", Zone::Configure, 30), x("mcp", Zone::Configure, 10), x("files", Zone::Browse, 30), x("commits", Zone::Browse, 10)]);
        let keys = |v: &[Entry]| v.iter().map(|e| e.key.clone()).collect::<Vec<_>>();
        s.rects.rail = Some([0.0, 0.0, W, 2000.0]);
        let l = s.rail_layout();
        assert_eq!(keys(&l.top), ["rail-requests", "rail-network", "rail-leads", "rail-commits", "rail-files"]);
        assert_eq!(keys(&l.bottom), ["rail-mcp", "rail-kits", "rail-setup", "jarvis-toggle"]);
        assert!(l.folded.is_empty());
        // Exactly the room for all of it (9 entries, 2 hairlines, the spacer's gap): nothing folds.
        let all = height_for(9, 2) + 6.0;
        s.rects.rail = Some([0.0, 0.0, W, all]);
        assert!(s.rail_layout().folded.is_empty());
        // A point less: the configure zone folds (not Settings).
        s.rects.rail = Some([0.0, 0.0, W, all - 1.0]);
        let l = s.rail_layout();
        assert_eq!(keys(&l.folded), ["rail-mcp", "rail-kits"]);
        assert_eq!(keys(&l.bottom), ["rail-more", "rail-setup", "jarvis-toggle"]);
        assert_eq!(keys(&l.top).len(), 5, "browse stays while it fits");
        // Much shorter: browse folds too; activity, Settings and Jarvis never.
        s.rects.rail = Some([0.0, 0.0, W, 200.0]);
        let l = s.rail_layout();
        assert_eq!(keys(&l.folded), ["rail-commits", "rail-files", "rail-mcp", "rail-kits"]);
        assert_eq!(keys(&l.top), ["rail-requests", "rail-network", "rail-leads"]);
        assert_eq!(keys(&l.bottom), ["rail-more", "rail-setup", "jarvis-toggle"]);
        // "More" opens its menu, a folded entry in it opens its panel's intent.
        s.rects.rail = Some([0.0, 0.0, W, 200.0]);
        s.rail_run(RailRun::More);
        assert!(s.rail_menu && s.rail_drawn("rail-more-list") && s.rail_drawn("rail-mcp"));
        assert!(s.rail_menu_view().is_some());
        assert!(s.escape() && !s.rail_menu, "Escape closes the menu");
        // Drawn: the hairline between Settings' zone and Jarvis.
        let r = Rig::new(&["a"], 1284.0, 816.0);
        assert!(r.rt.presented("rail-zone-jarvis").is_some());
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
