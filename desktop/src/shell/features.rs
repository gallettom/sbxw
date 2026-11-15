//! The core side of features (`src/feat/`): reading their data, running
//! their intents and actions, the generic confirmation dialog, and the
//! slots their screens show in (a modal, docked panels, extras on others'
//! slots).

use super::App;
use crate::approval::{self, ConfirmAnswers};
use crate::client::LinkEvent;
use crate::feat::{self, Action, Asked, Call, Confirm, Ctx, Extra, Feature, Place, Slot};
use crate::intent::{Intent, Risk};
use crate::theme::{c, layout, radius};
use rune::{col, el, row, text, Cursor, El, TextEdit, TextStyle};
use serde_json::{json, Map, Value as J};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// The confirmation showing.
pub struct Dialog {
    pub feature: &'static str,
    /// The screen (or `feature:<name>`) whose intent asked.
    pub from: String,
    pub title: String,
    pub confirm: Option<Confirm>,
    /// Waiting for the call a `Prepare` makes (its id).
    pub preparing: Option<u64>,
    pub prepare: Option<Box<feat::Prepare>>,
    /// The confirmed call is running (its id).
    pub running: Option<u64>,
    pub error: Option<String>,
    pub acked: bool,
    /// One secure edit per `confirm.secrets`.
    pub secrets: Vec<TextEdit>,
}

/// Features' state in the app.
#[derive(Default)]
pub struct Feats {
    /// Data read for features, by key (over the fixture's keys).
    pub data: Map<String, J>,
    /// Why a key's last read failed (the daemon's words), until the next
    /// read works: `data.app.errors[key]`. The key keeps its last good value.
    pub errors: Map<String, J>,
    /// The modal screen showing and its argument.
    pub modal: Option<(String, J)>,
    /// Open panels, in the order opened; `panel` the one shown.
    pub panels: Vec<String>,
    pub panel: Option<String>,
    pub dialog: Option<Dialog>,
    /// Data keys to read again at that time (bursts of events merged).
    due: HashMap<String, Instant>,
    /// Data keys read every N seconds: next time.
    every: HashMap<String, Instant>,
    /// An intent offered on the notice showing.
    pub notice_then: Option<(String, J)>,
    /// Extras on pane bars, from the last data.
    pub pane_extras: Vec<Extra>,
    next_id: u64,
    /// The running sandboxes per-sandbox data was last read for.
    running: Vec<String>,
}

/// The whole rule for an intent of risk Approval: its feature may answer only
/// ONE `Confirm` or ONE `Prepare` (the core draws that dialog and runs the
/// call on the person's click). Anything else is refused, nothing runs.
pub fn admit(risk: Risk, name: &str, actions: Vec<Action>) -> Result<Vec<Action>, String> {
    match risk {
        Risk::Direct => Ok(actions),
        Risk::Approval(_) => match actions.as_slice() {
            [Action::Confirm(_)] | [Action::Prepare(_)] => Ok(actions),
            _ => Err(format!("{name} needs the person's approval, but its feature answered without the core's dialog: refused")),
        },
    }
}

impl App {
    fn feat_ctx_data(&self) -> J {
        self.screen_data()
    }

    pub(super) fn feature_of(name: &str) -> Option<&'static Feature> {
        feat::FEATURES.iter().copied().find(|f| f.name == name)
    }

    /// Slots and features' intents.
    pub(super) fn run_feature_intent(&mut self, from: &str, intent: Intent) {
        match intent {
            Intent::OpenModal { screen, arg } => self.open_modal(&screen, arg.map(J::String).unwrap_or(J::Null)),
            Intent::CloseModal => self.feats.modal = None,
            Intent::OpenPanel { screen } => self.open_panel(&screen),
            Intent::ClosePanel { screen } => {
                let s = screen.or_else(|| self.feats.panel.clone());
                if let Some(s) = s {
                    self.close_panel(&s);
                }
            }
            Intent::Feature(a) => self.feature_intent(from, a),
            other => self.notify(format!("{from}: {} is not a feature intent", other.name()), true),
        }
    }

    fn feature_intent(&mut self, from: &str, a: Asked) {
        let Some((f, spec)) = feat::intent(a.name) else {
            return self.notify(format!("{from}: unknown intent {}", a.name), true);
        };
        let data = self.feat_ctx_data();
        let answer = (f.act)(&Ctx { data: &data }, &a).and_then(|acts| admit(spec.risk, a.name, acts));
        match answer {
            Ok(acts) => self.feature_actions(f, from, acts),
            Err(e) => self.notify(format!("{}: {e}", f.name), true),
        }
    }

    /// Runs what a feature asked.
    pub(super) fn feature_actions(&mut self, f: &'static Feature, from: &str, acts: Vec<Action>) {
        for a in acts {
            match a {
                Action::Call(call) => {
                    self.feature_call(f, call, false);
                }
                Action::Confirm(c) => self.open_dialog(f, from, Some(*c), None),
                Action::Prepare(p) => self.open_dialog(f, from, None, Some(p)),
                Action::Refresh(key) => self.refresh_key(key),
                Action::Fetch { key, path } => {
                    if let (Some(d), Some(l)) = (self.daemon.clone(), &self.link) {
                        l.fetch(d, key, vec![(None, path)]);
                    }
                }
                Action::Set { key, value } => {
                    self.feats.data.insert(key, value);
                }
                Action::OpenModal { screen, arg } => self.open_modal(&screen, arg),
                Action::CloseModal => self.feats.modal = None,
                Action::OpenPanel(s) => self.open_panel(&s),
                Action::ClosePanel(s) => self.close_panel(&s),
                Action::Notice { text, error, then } => {
                    self.notify(text, error);
                    self.feats.notice_then = then;
                }
                Action::OpenLink(url) => self.run_intent(from, Intent::OpenLink { url }),
                Action::Intent { name, args } => match crate::intent::parse(&json!({ "kind": "intent", "name": name, "args": args })) {
                    Ok(list) => crate::screen::Host::intents(self, &format!("feature:{}", f.name), list),
                    Err(e) => self.notify(format!("{}: {e}", f.name), true),
                },
            }
        }
    }

    fn next_id(&mut self) -> u64 {
        self.feats.next_id += 1;
        self.feats.next_id
    }

    /// Sends a call; returns its id (0: not connected, said in a notice).
    /// `confirmed`: sent by the person's Confirm on the core's dialog; only
    /// then may the body carry the feature's `core_only` fields.
    fn feature_call(&mut self, f: &'static Feature, call: Call, confirmed: bool) -> u64 {
        if let Err(e) = core_only_check(f, &call, confirmed) {
            self.notify(format!("{}: {e}", f.name), true);
            return 0;
        }
        let (Some(d), Some(_)) = (self.daemon.clone(), &self.link) else {
            self.notify(format!("{}: not connected: {}", call.what, self.connection.text), true);
            return 0;
        };
        let id = self.next_id();
        log::info!("{}: {:?} {} ({})", f.name, call.method, call.path, call.what);
        self.link.as_ref().unwrap().call(d, id, f.name, call);
        id
    }

    fn open_modal(&mut self, screen: &str, arg: J) {
        match feat::screen(screen) {
            Some((_, s)) if matches!(s.slot, Slot::Modal { .. }) => self.feats.modal = Some((screen.to_string(), arg)),
            _ => self.notify(format!("open_modal: no dialog screen named {screen}"), true),
        }
    }

    fn open_panel(&mut self, screen: &str) {
        match feat::screen(screen) {
            Some((_, s)) if matches!(s.slot, Slot::Panel { .. }) => {
                if !self.feats.panels.iter().any(|p| p == screen) {
                    self.feats.panels.push(screen.to_string());
                }
                self.feats.panel = Some(screen.to_string());
            }
            _ => self.notify(format!("open_panel: no panel screen named {screen}"), true),
        }
    }

    fn close_panel(&mut self, screen: &str) {
        self.feats.panels.retain(|p| p != screen);
        if self.feats.panel.as_deref() == Some(screen) {
            self.feats.panel = self.feats.panels.last().cloned();
        }
    }

    fn open_dialog(&mut self, f: &'static Feature, from: &str, confirm: Option<Confirm>, prepare: Option<Box<feat::Prepare>>) {
        let title = confirm.as_ref().map(|c| c.title.clone()).or_else(|| prepare.as_ref().map(|p| p.title.clone())).unwrap_or_default();
        let secrets = confirm.as_ref().map(secret_edits).unwrap_or_default();
        let mut d = Dialog { feature: f.name, from: from.to_string(), title, confirm, preparing: None, prepare: None, running: None, error: None, acked: false, secrets };
        if let Some(p) = prepare {
            let id = self.feature_call(f, p.call.clone().quiet(), false);
            d.preparing = Some(id);
            if id == 0 {
                d.error = Some(format!("Not connected: {}", self.connection.text));
            }
            d.prepare = Some(p);
        }
        // A newer one replaces the one showing (the person answers one at a time).
        self.feats.dialog = Some(d);
    }

    // ---- data ----

    /// Reads a declared data key now (per sandbox: for every running sandbox).
    pub(super) fn refresh_key(&mut self, key: &str) {
        let Some(d) = feat::FEATURES.iter().flat_map(|f| f.data.iter()).find(|d| d.key == key) else {
            return log::warn!("refresh: no data key {key}");
        };
        let (Some(daemon), Some(l)) = (self.daemon.clone(), &self.link) else { return };
        let paths = if d.per_sandbox() {
            self.sandboxes.iter().filter(|s| s.status == "running").map(|s| (Some(s.name.clone()), d.path.replace("{sandbox}", &crate::client::enc(&s.name)))).collect::<Vec<_>>()
        } else {
            vec![(None, d.path.to_string())]
        };
        if paths.is_empty() {
            self.feats.data.insert(key.to_string(), json!({}));
            return;
        }
        l.fetch(daemon, key.to_string(), paths);
    }

    /// At (re)connection: every non-lazy key.
    pub(super) fn feats_connected(&mut self) {
        let now = Instant::now();
        for d in feat::FEATURES.iter().flat_map(|f| f.data.iter()) {
            if !d.lazy {
                self.feats.due.insert(d.key.to_string(), now);
            }
            if d.every > 0 {
                self.feats.every.insert(d.key.to_string(), now + Duration::from_secs(d.every));
            }
        }
    }

    /// The list changed: per-sandbox keys follow the running set.
    pub(super) fn feats_sandboxes(&mut self) {
        let running: Vec<String> = self.sandboxes.iter().filter(|s| s.status == "running").map(|s| s.name.clone()).collect();
        if running != self.feats.running {
            self.feats.running = running;
            let now = Instant::now();
            for d in feat::FEATURES.iter().flat_map(|f| f.data.iter()) {
                if d.per_sandbox() && !d.lazy {
                    self.feats.due.insert(d.key.to_string(), now);
                }
            }
        }
    }

    /// A stream event: data keys read again 300 ms later; features told.
    pub(super) fn feats_bus(&mut self, name: &str, raw: &str) -> bool {
        let soon = Instant::now() + Duration::from_millis(300);
        for d in feat::FEATURES.iter().flat_map(|f| f.data.iter()) {
            if d.on.contains(&name) {
                self.feats.due.entry(d.key.to_string()).or_insert(soon);
            }
        }
        let mut changed = false;
        let body: J = serde_json::from_str(raw).unwrap_or(J::String(raw.to_string()));
        for f in feat::FEATURES {
            if let (true, Some(on)) = (f.events.contains(&name), f.on_event) {
                let data = self.feat_ctx_data();
                let acts = on(&Ctx { data: &data }, name, &body);
                if !acts.is_empty() {
                    changed = true;
                    self.feature_actions(f, &format!("feature:{}", f.name), acts);
                }
            }
        }
        changed
    }

    /// Due reads; returns when to wake next.
    pub(super) fn feats_poll(&mut self) -> Option<Instant> {
        let now = Instant::now();
        let due: Vec<String> = self.feats.due.iter().filter(|(_, t)| **t <= now).map(|(k, _)| k.clone()).collect();
        for k in &due {
            self.feats.due.remove(k);
        }
        let mut every = vec![];
        for (k, t) in self.feats.every.iter_mut() {
            if *t <= now {
                every.push(k.clone());
                let secs = feat::FEATURES.iter().flat_map(|f| f.data.iter()).find(|d| d.key == k).map_or(60, |d| d.every);
                *t = now + Duration::from_secs(secs);
            }
        }
        if self.daemon.is_some() {
            for k in due.iter().chain(every.iter()) {
                self.refresh_key(k);
            }
        }
        self.feats.due.values().chain(self.feats.every.values()).min().copied()
    }

    /// What the link answered for features.
    pub(super) fn feats_link(&mut self, ev: LinkEvent) -> bool {
        match ev {
            LinkEvent::Data { key, result } => match result {
                Ok(v) => {
                    let had_error = self.feats.errors.remove(&key).is_some();
                    if self.feats.data.get(&key) == Some(&v) && !had_error {
                        return false;
                    }
                    self.feats.data.insert(key, v);
                }
                Err(e) => {
                    log::warn!("{key}: {e}");
                    if self.feats.errors.get(&key).and_then(J::as_str) == Some(e.as_str()) {
                        return false;
                    }
                    self.feats.errors.insert(key, J::String(e));
                }
            },
            LinkEvent::Answer { id, feature, call, result } => {
                let Some(f) = Self::feature_of(feature) else { return false };
                for k in &call.then {
                    self.refresh_key(k);
                }
                // A prepared dialog's read.
                if self.feats.dialog.as_ref().is_some_and(|d| d.preparing == Some(id)) {
                    let data = self.feat_ctx_data();
                    let d = self.feats.dialog.as_mut().unwrap();
                    d.preparing = None;
                    let p = d.prepare.take().expect("prepare");
                    let built = match result {
                        Ok((code, v)) if (200..300).contains(&code) => (p.build)(&Ctx { data: &data }, &p.asked, &v),
                        Ok((code, v)) => Err(format!("The daemon answered {code}: {}", v["error"].as_str().unwrap_or("no reason given"))),
                        Err(e) => Err(e),
                    };
                    match built {
                        Ok(c) => {
                            d.secrets = secret_edits(&c);
                            d.title = c.title.clone();
                            d.confirm = Some(c);
                        }
                        Err(e) => d.error = Some(e),
                    }
                    return true;
                }
                let from_dialog = self.feats.dialog.as_ref().is_some_and(|d| d.running == Some(id));
                if let Some(on) = f.on_answer {
                    if from_dialog {
                        self.feats.dialog = None;
                    }
                    let data = self.feat_ctx_data();
                    let acts = on(&Ctx { data: &data }, &call, result);
                    self.feature_actions(f, &format!("feature:{}", f.name), acts);
                    return true;
                }
                match result {
                    Ok((code, v)) if (200..300).contains(&code) => {
                        if from_dialog {
                            self.feats.dialog = None;
                        }
                        if let Some(k) = &call.store {
                            self.feats.data.insert(k.clone(), v);
                        }
                        if call.notify {
                            self.notify(format!("{}: done", call.what), false);
                        }
                    }
                    Ok((code, v)) => {
                        let why = v["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {code}"));
                        self.fail(from_dialog, &call.what, why);
                    }
                    Err(e) => self.fail(from_dialog, &call.what, e),
                }
            }
            _ => return false,
        }
        true
    }

    fn fail(&mut self, from_dialog: bool, what: &str, why: String) {
        if from_dialog {
            if let Some(d) = self.feats.dialog.as_mut() {
                d.running = None;
                d.error = Some(why);
                return;
            }
        }
        self.notify(format!("{what}: {why}"), true);
    }

    // ---- what screens see ----

    /// `data.app.modal`, `data.app.panels`, `data.app.extras`, from `data`.
    pub(super) fn feats_app(&self, data: &J) -> (J, J, J, Vec<Extra>) {
        let modal = match &self.feats.modal {
            Some((s, a)) => json!({ "screen": s, "arg": a }),
            None => J::Null,
        };
        let panels = json!({ "open": self.feats.panels, "shown": self.feats.panel });
        let mut topbar = vec![];
        let mut sandboxes = Map::new();
        let mut panes = vec![];
        for f in feat::FEATURES {
            let Some(x) = f.extras else { continue };
            for e in x(&Ctx { data }) {
                match &e.place {
                    Place::Topbar => topbar.push(e.to_json()),
                    Place::Sandbox(n) => sandboxes.entry(n.clone()).or_insert_with(|| J::Array(vec![])).as_array_mut().unwrap().push(e.to_json()),
                    Place::Pane(_) => panes.push(e),
                }
            }
        }
        let _ = &self.feats.errors;
        (modal, panels, json!({ "topbar": topbar, "sandboxes": sandboxes }), panes)
    }

    /// A press on an extra the core draws (pane bars) or on a notice's offer.
    pub(super) fn press_extra(&mut self, from: &str, name: &str, args: &J) {
        match crate::intent::parse(&json!({ "kind": "intent", "name": name, "args": args })) {
            Ok(list) => crate::screen::Host::intents(self, from, list),
            Err(e) => self.notify(format!("{from}: {e}"), true),
        }
    }

    // ---- drawing ----

    /// The confirmation, centred over the window.
    pub(super) fn confirm_view(&self) -> Option<El> {
        let d = self.feats.dialog.as_ref()?;
        let w = 520.0f32.min(self.window.0 - 32.0);
        let side = ((self.window.0 - w) / 2.0).max(16.0);
        let top = layout::GUTTER * 2.0 + layout::TOP_BAR_H + 24.0;
        let st = approval::ConfirmState {
            confirm: d.confirm.as_ref(),
            title: &d.title,
            loading: d.preparing.is_some() && d.error.is_none(),
            error: d.error.as_deref(),
            busy: d.running.is_some(),
            acked: d.acked,
            secrets: &d.secrets,
            left: side,
            top,
            right: side,
            max_h: self.window.1 - top - 24.0,
        };
        Some(approval::confirm_view::<App>(&st))
    }

    /// The modal screen over a dimmed window.
    pub(super) fn modal_view(&self, screen: El, width: f32) -> El {
        let w = width.min(self.window.0 - 32.0);
        let side = ((self.window.0 - w) / 2.0).max(16.0);
        let top = layout::GUTTER * 2.0 + layout::TOP_BAR_H + 16.0;
        let card = col()
            .key("modal")
            .role("dialog")
            .pin(Some(side), Some(top), Some(side), None)
            .max_h((self.window.1 - top - 24.0).max(160.0))
            .padding(14.0)
            .gap(8.0)
            .radius(radius("lg").unwrap())
            .fill(c("surface"))
            .border(1.0, c("border"))
            .shadow(8.0, 24.0, rune::color::hex("#00000073").unwrap())
            // A press inside stays inside (it does not reach the scrim).
            .on_press(|_s: &mut App| {})
            .child(
                row().w_full().child(el().grow(1.0)).child(
                    el().key("modal-close")
                        .role("button")
                        .label("Close")
                        .help("Close (Escape)")
                        .padding(3.0)
                        .radius(radius("xs").unwrap())
                        .hover(c("border"))
                        .cursor(Cursor::Pointer)
                        .on_press(|s: &mut App| {
                            s.feats.modal = None;
                            s.push_data();
                        })
                        .child(rune::icon(crate::icons::get("close").unwrap(), 12.0).color(c("text-muted"))),
                ),
            )
            .child(col().w_full().min_h(0.0).grow(1.0).child(screen));
        el().key("modal-scrim")
            .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .fill(rune::color::hex("#00000080").unwrap())
            .on_press(|s: &mut App| {
                s.feats.modal = None;
                s.push_data();
            })
            .child(card)
    }

    /// The docked feature panel: tabs when more than one is open, the screen below.
    pub(super) fn feat_panel_view(&self, screen: El, width: f32) -> El {
        let mut tabs = row().key("panel-tabs").w_full().gap(4.0).items_center();
        for name in &self.feats.panels {
            let Some((_, decl)) = feat::screen(name) else { continue };
            let Slot::Panel { title, icon, .. } = decl.slot else { continue };
            let shown = self.feats.panel.as_deref() == Some(name.as_str());
            let n1 = name.clone();
            let n2 = name.clone();
            let mut tab = row()
                .key(format!("panel-tab-{name}"))
                .role("tab")
                .label(title)
                .gap(5.0)
                .px(8.0)
                .py(4.0)
                .items_center()
                .radius(radius("sm").unwrap())
                .fill(c(if shown { "elevated" } else { "bg" }))
                .cursor(Cursor::Pointer)
                .on_press(move |s: &mut App| {
                    s.feats.panel = Some(n1.clone());
                    s.push_data();
                });
            if let Some(i) = crate::icons::get(icon) {
                tab = tab.child(rune::icon(i, 12.0).color(c(if shown { "text-bright" } else { "text-muted" })));
            }
            tab = tab.child(text(title).font(TextStyle::new(12.0, 500.0)).color(c(if shown { "text-bright" } else { "text-muted" }))).child(
                el().key(format!("panel-close-{name}"))
                    .role("button")
                    .label(format!("Close {title}"))
                    .padding(2.0)
                    .radius(radius("xs").unwrap())
                    .hover(c("border"))
                    .on_press(move |s: &mut App| {
                        s.close_panel(&n2);
                        s.push_data();
                    })
                    .child(rune::icon(crate::icons::get("close").unwrap(), 10.0).color(c("text-muted"))),
            );
            tabs = tabs.child(tab);
        }
        col()
            .key("feature-panel")
            .role("complementary")
            .w(width)
            .h_full()
            .min_h(0.0)
            .padding(10.0)
            .gap(8.0)
            .fill(c("bg"))
            .border(1.0, c("elevated"))
            .radius(radius("md").unwrap())
            .child(tabs)
            .child(col().w_full().grow(1.0).min_h(0.0).child(screen))
    }

    /// Feature buttons and badges on the bar of pane `i`.
    pub(super) fn pane_extras(&self, i: usize, sandbox: &str) -> Vec<El> {
        self.feats
            .pane_extras
            .iter()
            .filter(|e| matches!(&e.place, Place::Pane(s) if s == sandbox))
            .map(|e| {
                let (fg, tint) = crate::screen::tone(&e.tone);
                let mut b = row()
                    .key(format!("pane-{}-x-{}", i + 1, e.id))
                    .gap(4.0)
                    .px(5.0)
                    .py(1.0)
                    .items_center()
                    .radius(radius("xs").unwrap())
                    .fill(crate::theme::hex(tint))
                    .border(1.0, crate::theme::hex(fg));
                if !e.help.is_empty() {
                    b = b.help(e.help.clone());
                }
                if let Some(i) = e.icon.as_deref().and_then(crate::icons::get) {
                    b = b.child(rune::icon(i, 11.0).color(crate::theme::hex(fg)));
                }
                let label = match e.count {
                    Some(n) if e.label.is_empty() => n.to_string(),
                    Some(n) => format!("{} {n}", e.label),
                    None => e.label.clone(),
                };
                if !label.is_empty() {
                    b = b.child(text(label).font(TextStyle::new(10.5, 500.0)).color(crate::theme::hex(fg)));
                }
                if let Some((name, args)) = e.intent.clone() {
                    b = b.role("button").cursor(Cursor::Pointer).on_press(move |s: &mut App| {
                        s.press_extra("pane", &name, &args);
                        s.push_data();
                    });
                }
                b
            })
            .collect()
    }
}

/// A call's body may hold a feature's `core_only` fields only when the
/// person confirmed it on the core's dialog.
pub fn core_only_check(f: &Feature, call: &Call, confirmed: bool) -> Result<(), String> {
    if confirmed {
        return Ok(());
    }
    match f.core_only.iter().find(|k| call.body.get(**k).is_some()) {
        Some(k) => Err(format!("{}: '{k}' is set only by your Confirm on the app's own dialog: refused", call.what)),
        None => Ok(()),
    }
}

fn secret_edits(c: &Confirm) -> Vec<TextEdit> {
    c.secrets.iter().map(|_| TextEdit::new("").secure()).collect()
}

impl ConfirmAnswers for App {
    fn confirm_click(&mut self) {
        let Some(d) = self.feats.dialog.as_ref() else { return };
        let st = approval::ConfirmState { confirm: d.confirm.as_ref(), title: "", loading: false, error: None, busy: d.running.is_some(), acked: d.acked, secrets: &d.secrets, left: 0.0, top: 0.0, right: 0.0, max_h: 0.0 };
        if !st.can_confirm() {
            return;
        }
        let Some(f) = Self::feature_of(d.feature) else { return };
        let confirm = d.confirm.clone().expect("loaded");
        let mut call = confirm.call.clone();
        // The secrets go into the body here, once; the edits are wiped below.
        for (i, s) in confirm.secrets.iter().enumerate() {
            let v = d.secrets[i].text.clone();
            if v.is_empty() && s.optional {
                continue;
            }
            if let Err(e) = feat::put_at(&mut call.body, &s.field, J::String(v)) {
                if let Some(d) = self.feats.dialog.as_mut() {
                    d.error = Some(e);
                }
                return;
            }
        }
        for g in &confirm.grants {
            if let Err(e) = feat::put_at(&mut call.body, g, J::Bool(true)) {
                if let Some(d) = self.feats.dialog.as_mut() {
                    d.error = Some(e);
                }
                return;
            }
        }
        log::info!("{}: confirmed '{}' ({:?} {})", f.name, confirm.title, call.method, call.path);
        let id = self.feature_call(f, call, true);
        if let Some(d) = self.feats.dialog.as_mut() {
            for e in &mut d.secrets {
                e.replace_text(String::new());
            }
            if id == 0 {
                d.error = Some(format!("Not connected: {}", self.connection.text));
            } else {
                d.running = Some(id);
                d.error = None;
            }
        }
        self.push_data();
    }

    fn cancel_click(&mut self) {
        self.feats.dialog = None;
        self.push_data();
    }

    fn toggle_confirm_ack(&mut self) {
        if let Some(d) = self.feats.dialog.as_mut() {
            d.acked = !d.acked;
        }
    }

    fn secret_edit(&mut self, i: usize) -> &mut TextEdit {
        let d = self.feats.dialog.as_mut().expect("dialog open");
        &mut d.secrets[i]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_approval_intent_only_ever_opens_the_core_dialog() {
        let call = Call::post("/api/x", json!({}), "x");
        let risk = Risk::Approval("Allow");
        // Straight to the daemon: refused.
        assert!(admit(risk, "allow", vec![Action::Call(call.clone())]).is_err());
        // A dialog plus a call beside it: refused.
        assert!(admit(risk, "allow", vec![Confirm::new("t", call.clone()).into_action(), Action::Call(call.clone())]).is_err());
        // An intent run on its behalf: refused (it could be anything).
        assert!(admit(risk, "allow", vec![Action::Intent { name: "x".into(), args: json!({}) }]).is_err());
        assert!(admit(risk, "allow", vec![]).is_err());
        assert!(admit(risk, "allow", vec![Confirm::new("t", call.clone()).into_action()]).is_ok());
        assert!(admit(Risk::Direct, "deny", vec![Action::Call(call)]).is_ok());
    }

    #[test]
    fn core_only_fields_go_out_only_after_a_confirm() {
        const F: Feature = Feature { name: "t", core_only: &["approved"], ..Feature::EMPTY };
        let sneaky = Call::post("/api/sandboxes/create", json!({ "name": "x", "approved": true }), "create x");
        assert!(core_only_check(&F, &sneaky, false).is_err(), "a feature (or a screen's args copied into the body) cannot send it");
        assert!(core_only_check(&F, &Call::post("/api/sandboxes/create", json!({ "name": "x" }), "create x"), false).is_ok());
        // On Confirm the core adds it itself; the feature cannot preset it.
        let c = Confirm::new("Create x?", Call::post("/api/sandboxes/create", json!({ "name": "x" }), "create x")).grant("approved");
        let mut body = c.call.body.clone();
        for g in &c.grants {
            feat::put_at(&mut body, g, J::Bool(true)).unwrap();
        }
        assert_eq!(body["approved"], true);
        let preset = Confirm::new("t", sneaky.clone()).grant("approved");
        let mut b2 = preset.call.body.clone();
        assert!(feat::put_at(&mut b2, "approved", J::Bool(true)).is_err());
    }
}
