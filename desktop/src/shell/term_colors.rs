//! Terminal colours: the background every terminal is drawn on, the pane
//! bar above it and the text, picked as in the web UI
//! (`assets/js/term-color.js`). Three slots, each the web UI's presets
//! ([`crate::terminal::theme::Slot`]) and a custom colour.
//!
//! * **Where**: the pager's palette button (`pager-colors`), right of the
//!   "terminals per page" switch, the second half of one "terms" control as
//!   in the web UI; and the palette's "Terminal colours". It opens a
//!   popover (`term-colors`): per slot a row of swatches
//!   (`term-colors-<slot>-<n>`, the current one checked, its name as tooltip
//!   and label), a custom field (`term-colors-<slot>-custom`, `#rrggbb`, and
//!   `auto` for the text; anything else is refused with a line saying so)
//!   and Reset (`term-colors-<slot>-reset`), back to sbxw.toml or the
//!   built-in.
//! * **Sources**, per slot, the first set wins: the person's pick (kept in
//!   the state file, `terminal_colors`, store.rs), sbxw.toml's
//!   (`terminal_color`, `terminal_bar_color`, `terminal_text_color`, read
//!   from the daemon's `GET /api/state` `terminal_colors`), the app theme's
//!   own. sbxw.toml is never written.
//! * **Applied** through [`crate::terminal::theme::set_chosen`]: every
//!   terminal follows on its next poll (its colours, selection, cursor and
//!   keyword pills; a light background takes the light rules, the same path
//!   as sbxw.toml's light value), in the main window and in page windows
//!   (woken here; their bars are drawn again since the fingerprint holds
//!   the colours' generation). The pane bar's ground is [`bar_ground`]: the
//!   picked bar under the group tint.
//! * A pair under 4.5:1 (text on background, the bar's labels on the bar)
//!   is allowed and shows a quiet warning line.

use super::*;
use crate::terminal::theme::{self as tt, Chosen, Pick, Slot};
use rune::{Side, TextEdit};

/// The state file's key (store.rs).
pub const KEY: &str = "terminal_colors";

/// What reads easily (text under 18 pt).
pub const NEED: f64 = 4.5;

const W: f32 = 320.0;
/// The pager's button: its padding, the tiny pane, the gap and the mark
/// (the pager's fit counts it, pager.rs `controls_w`).
pub const TRIGGER_W: f32 = 6.0 * 2.0 + 16.0 + 4.0 + theme::icon::SMALL;
const SWATCH_W: f32 = 30.0;
const SWATCH_H: f32 = 22.0;

/// The popover's own state while open: each slot's custom field and the
/// refusal under it.
pub struct Panel {
    pub edits: [TextEdit; 3],
    pub refused: [Option<String>; 3],
}

impl Default for Panel {
    fn default() -> Self {
        Panel { edits: [TextEdit::new(""), TextEdit::new(""), TextEdit::new("")], refused: [None, None, None] }
    }
}

fn idx(slot: Slot) -> usize {
    match slot {
        Slot::Background => 0,
        Slot::Bar => 1,
        Slot::Text => 2,
    }
}

/// Where a slot's value comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Picked,
    Toml,
    Builtin,
}

/// The kept picks (`terminal_colors` in the state file), each validated.
pub fn kept(store: &store::Store, slot: Slot) -> Option<Pick> {
    store.get(KEY)[slot.key()].as_str().and_then(|v| slot.parse(v))
}

/// sbxw.toml's value for `slot`, as the daemon's `GET /api/state` gives it
/// (`terminal_colors`); none on a daemon without the field.
pub fn toml(daemon_state: Option<&J>, slot: Slot) -> Option<Pick> {
    daemon_state.and_then(|d| d["terminal_colors"][slot.key()].as_str()).and_then(|v| slot.parse(v))
}

/// The colours in force from the two sources (the built-in: `None`).
pub fn resolve(store: &store::Store, daemon_state: Option<&J>) -> Chosen {
    let get = |slot| kept(store, slot).or_else(|| toml(daemon_state, slot));
    let color = |p: Option<Pick>| match p {
        Some(Pick::Color(c)) => Some(c),
        _ => None,
    };
    Chosen { background: color(get(Slot::Background)), bar: color(get(Slot::Bar)), text: get(Slot::Text) }
}

/// A pane bar's ground: the picked bar colour (a little toward the theme's
/// quieter surface when the pane is not focused) under its group's tint,
/// as the theme's own bar; the theme's ([`theme::bar_ground`]) when no bar
/// colour is in force.
pub fn bar_ground(group: Option<u32>, focused: bool) -> u32 {
    let Some(b) = tt::chosen().bar else { return theme::bar_ground(group, focused) };
    let surface = if focused { b } else { theme::over(theme::color_value("surface-alt").unwrap(), 0.25, b) };
    let look = theme::group_look();
    match group {
        Some(g) => theme::over(g, if focused { look.bar } else { look.bar_unfocused }, surface),
        None => surface,
    }
}

/// The contrast warnings for the colours in force: text on the background,
/// the bar's labels on the bar.
pub fn warnings() -> Vec<String> {
    let t = tt::for_mode(theme::mode());
    let mut v = vec![];
    let r = theme::contrast(t.foreground, t.background);
    if r < NEED {
        v.push(format!("Text on this background is {r:.1}:1, under the 4.5:1 that reads easily."));
    }
    let bar = bar_ground(None, true);
    let r = theme::contrast(theme::color_value("text-bright").unwrap(), bar);
    if r < NEED {
        v.push(format!("The bar's labels on this bar are {r:.1}:1, under the 4.5:1 that reads easily."));
    }
    v
}

impl App {
    /// Where `slot`'s value comes from now, and that value (`None`: the app theme's own).
    pub fn term_color_source(&self, slot: Slot) -> (Source, Option<Pick>) {
        if let Some(p) = kept(&self.store, slot) {
            return (Source::Picked, Some(p));
        }
        match toml(self.feats.data.get("daemon_state"), slot) {
            Some(p) => (Source::Toml, Some(p)),
            None => (Source::Builtin, None),
        }
    }

    /// Puts the sources' colours in force when they changed; true then.
    /// The main window's terminals follow at once, the page windows are
    /// woken for theirs.
    pub fn sync_term_colors(&mut self) -> bool {
        if !tt::set_chosen(resolve(&self.store, self.feats.data.get("daemon_state"))) {
            return false;
        }
        for p in &mut self.panes {
            if let Some(term) = &mut p.term {
                term.poll(None);
            }
        }
        crate::windows::with(|h| h.mark_all());
        for n in self.page_windows() {
            crate::windows::wake(crate::windows::Win::Page(n));
        }
        true
    }

    /// The person's pick for `slot` (`None`: Reset), kept and applied. A
    /// pick equal to sbxw.toml's is not kept (as the web UI: the file's
    /// value then follows the file).
    pub fn set_term_color(&mut self, slot: Slot, pick: Option<Pick>) {
        let pick = pick.filter(|p| toml(self.feats.data.get("daemon_state"), slot) != Some(*p));
        self.store.set_in(KEY, slot.key(), pick.map_or(J::Null, |p| J::String(p.spelled())));
        self.term_colors.refused[idx(slot)] = None;
        self.sync_term_colors();
    }

    /// The custom field of `slot` applied: a colour (or `auto` for the
    /// text) is picked; anything else is refused, with a line saying so.
    pub fn apply_custom_term_color(&mut self, slot: Slot) {
        let typed = self.term_colors.edits[idx(slot)].text.trim().to_string();
        match slot.parse(&typed) {
            Some(p) => {
                self.term_colors.edits[idx(slot)] = TextEdit::new("");
                self.set_term_color(slot, Some(p));
            }
            None => {
                let want = if slot == Slot::Text { "#rrggbb or auto" } else { "#rrggbb" };
                self.term_colors.refused[idx(slot)] = Some(if typed.is_empty() { format!("Type a colour: {want}.") } else { format!("“{typed}” is not a colour: type {want}.") });
            }
        }
    }

    /// Opens the popover (the pager's button, the palette).
    pub fn open_term_colors(&mut self) {
        self.term_colors = Panel::default();
        self.pager_menu = Some("colors".into());
    }

    /// Whether the popover draws an element of that key (`wait:` marks).
    pub(super) fn term_colors_drawn(&self, key: &str) -> bool {
        if key == "pager-colors" {
            return true;
        }
        self.pager_menu.as_deref() == Some("colors") && (key == "term-colors" || key.starts_with("term-colors-"))
    }
}

/// The pager's button: a tiny pane (its bar over its background) and the
/// palette mark, as the web UI's.
pub fn trigger(s: &App) -> El {
    let open = s.pager_menu.as_deref() == Some("colors");
    let t = tt::for_mode(theme::mode());
    let swatch = col()
        .w(16.0)
        .h(12.0)
        .radius(theme::radius("xs").unwrap())
        .border(1.0, c("border"))
        .fill(theme::hex(t.background))
        .clip_rounded(theme::radius("xs").unwrap())
        .child(el().w_full().h(4.0).fill(theme::hex(bar_ground(None, true))))
        .child(el().ml(2.0).mt(2.0).w(7.0).h(2.0).fill(theme::hex(t.foreground)));
    row()
        .key("pager-colors")
        .role("button")
        .label("Terminal colours")
        .expanded(open)
        .help("Terminal colours: background, bar and text")
        .h(pager::H - 6.0)
        .w(TRIGGER_W)
        .px(6.0)
        .gap(4.0)
        .items_center()
        .radius(theme::radius("xs").unwrap())
        .fill(c(if open { "elevated" } else { "surface-alt" }))
        .hover(c("elevated"))
        .cursor(Cursor::Pointer)
        .on_press(|s: &mut App| {
            if s.pager_menu.as_deref() == Some("colors") {
                s.pager_menu = None;
            } else {
                s.open_term_colors();
            }
        })
        .child(swatch)
        .child(rune::icon(crate::icons::get("appearance").unwrap(), theme::icon::SMALL).color(c("text-muted")))
}

fn caption(t: impl Into<String>, tone: &str) -> El {
    text(t.into()).font(small(theme::font_size::CAPTION, 400.0)).color(c(tone)).wrap()
}

fn swatch(slot: Slot, n: usize, value: &str, name: &str, on: bool, ground: u32) -> El {
    let pick = slot.parse(value).expect("a preset is a colour");
    let mut e = el()
        .key(format!("term-colors-{}-{n}", slot.key()))
        .role("radio")
        .label(format!("{}: {name}", slot.heading()))
        .help(name.to_string())
        .checked(on)
        .w(SWATCH_W)
        .h(SWATCH_H)
        .items_center()
        .justify_center()
        .radius(theme::radius("xs").unwrap())
        .cursor(Cursor::Pointer)
        .focusable(true)
        .focus_ring(theme::focus::W, theme::focus::OUT, c("focus"))
        .on_press(move |s: &mut App| s.set_term_color(slot, Some(pick)));
    e = if on { e.border(2.0, c("focus")) } else { e.border(1.0, c("border")) };
    match (slot, pick) {
        // The text is shown as text on the background in force.
        (Slot::Text, p) => {
            let fg = match p {
                Pick::Color(t) => t,
                Pick::Auto => tt::Theme::from_settings(ground, None).foreground,
            };
            e.fill(theme::hex(ground)).child(text(if p == Pick::Auto { "auto" } else { "Aa" }).font(small(theme::font_size::CAPTION, 600.0)).color(theme::hex(fg)))
        }
        (_, Pick::Color(v)) => e.fill(theme::hex(v)),
        (_, Pick::Auto) => e,
    }
}

/// The value a slot's swatches show as current (the one checked): the one
/// in force, else the app theme's own.
pub fn shown(s: &App, slot: Slot) -> Pick {
    let t = tt::for_mode(theme::mode());
    s.term_color_source(slot).1.unwrap_or(match slot {
        Slot::Background => Pick::Color(t.background),
        Slot::Bar => Pick::Color(bar_ground(None, true)),
        Slot::Text => Pick::Auto,
    })
}

fn slot_view(s: &App, slot: Slot) -> El {
    let (source, _) = s.term_color_source(slot);
    let t = tt::for_mode(theme::mode());
    let shown = shown(s, slot);
    let from = match source {
        Source::Picked => "picked here",
        Source::Toml => "from sbxw.toml",
        Source::Builtin => "built-in",
    };
    let name = slot.preset_name(shown).map(str::to_string).unwrap_or_else(|| shown.spelled());
    let mut swatches = row().key(format!("term-colors-{}", slot.key())).role("radiogroup").label(slot.heading().to_string()).gap(4.0).flex_wrap();
    for (n, (v, nm)) in slot.presets().iter().enumerate() {
        swatches = swatches.child(swatch(slot, n, v, nm, slot.parse(v) == Some(shown), t.background));
    }
    let k = idx(slot);
    let field_key = format!("term-colors-{}-custom", slot.key());
    let field = rune::text_input(&s.term_colors.edits[k], small(theme::font_size::BODY, 400.0))
        .single_line()
        .key(field_key)
        .label(format!("{}: custom colour", slot.heading()))
        .placeholder(if slot == Slot::Text { "#rrggbb or auto" } else { "#rrggbb" }, c("text-muted"))
        .color(c("text-bright"))
        .grow(1.0)
        .min_w(0.0)
        .on_edit(move |s: &mut App| &mut s.term_colors.edits[k])
        .on_input_change(move |s: &mut App, _tx| s.term_colors.refused[k] = None)
        .on_input_key(move |s: &mut App, ev: &rune::KeyEvent, _tx| match ev.key {
            Key::Enter => {
                s.apply_custom_term_color(slot);
                true
            }
            _ => false,
        });
    let field = row().h(26.0).px(6.0).items_center().grow(1.0).min_w(0.0).radius(theme::radius("xs").unwrap()).fill(c("bg")).border(1.0, c(if s.term_colors.refused[k].is_some() { "red" } else { "border" })).child(field);
    let button = |key: String, label: String, help: String, disabled: bool| {
        let e = row().key(key).role("button").label(label.clone()).help(help).h(26.0).px(8.0).items_center().radius(theme::radius("xs").unwrap()).fill(c("surface-alt")).child(text(label).font(small(theme::font_size::CAPTION, 500.0)).color(c(if disabled { "text-muted" } else { "text" })));
        if disabled {
            e.opacity(theme::DISABLED_OPACITY).disabled(true)
        } else {
            e.hover(c("elevated")).cursor(Cursor::Pointer).focusable(true).focus_ring(theme::focus::W, theme::focus::OUT, c("focus"))
        }
    };
    let apply = button(format!("term-colors-{}-apply", slot.key()), "Apply".into(), "Use the colour typed".into(), false).on_press(move |s: &mut App| s.apply_custom_term_color(slot));
    let fallback = if toml(s.feats.data.get("daemon_state"), slot).is_some() { "sbxw.toml" } else { "built-in" };
    let picked = source == Source::Picked;
    let reset_label = if picked { format!("Reset to {fallback}") } else { format!("Default: {fallback}") };
    let mut reset = button(format!("term-colors-{}-reset", slot.key()), reset_label, format!("Forget the colour picked here: {fallback} again"), !picked);
    if picked {
        reset = reset.on_press(move |s: &mut App| s.set_term_color(slot, None));
    }
    let mut out = col()
        .gap(6.0)
        .w_full()
        .child(row().gap(6.0).items_center().child(text(slot.heading()).font(small(theme::font_size::CAPTION, 600.0)).color(c("text-bright"))).child(text(format!("{name}, {from}")).font(small(theme::font_size::CAPTION, 400.0)).color(c("text-muted")).truncate().min_w(0.0)))
        .child(swatches)
        .child(row().gap(4.0).items_center().w_full().child(field).child(apply).child(reset));
    if let Some(why) = &s.term_colors.refused[k] {
        out = out.child(el().key(format!("term-colors-{}-refused", slot.key())).child(caption(why.clone(), "red")));
    }
    out
}

/// The popover, anchored under the pager's button.
pub fn view(s: &App) -> El {
    let mut p = col()
        .key("term-colors")
        .role("dialog")
        .label("Terminal colours")
        .focus_group()
        .anchored("pager-colors".to_string(), Side::Bottom, rune::Align::End, 4.0)
        .w(W)
        .padding(12.0)
        .gap(12.0)
        .fill(c("surface"))
        .border(1.0, c("border"))
        .radius(theme::radius("md").unwrap())
        .child(text("Terminal colours").font(small(theme::font_size::BODY, 600.0)).color(c("text-bright")));
    for slot in Slot::ALL {
        p = p.child(slot_view(s, slot));
    }
    for (n, w) in warnings().into_iter().enumerate() {
        p = p.child(
            row()
                .key(format!("term-colors-warning-{n}"))
                .gap(6.0)
                .items_start()
                .child(rune::icon(crate::icons::get("warning").unwrap(), theme::icon::SMALL).color(c("text-muted")))
                .child(caption(w, "text-muted").grow(1.0).min_w(0.0)),
        );
    }
    theme::lift(p, theme::elevation::OVERLAY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::TerminalConn;
    use crate::emphasis::Tone;
    use crate::terminal::theme::Theme;
    use rune::Runtime;
    use std::sync::mpsc;

    /// A window of two live terminals (their sockets held by the test).
    fn window() -> (Runtime<App>, Vec<(mpsc::Sender<crate::client::TermEvent>, mpsc::Receiver<crate::client::TermInput>)>) {
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/files/standin.json");
        let mut a = App::new(Args { fixture: Some(fixture.into()), ..Args::default() });
        // Never the person's state file.
        a.store = store::Store::memory();
        super::super::tests::set_panes(&mut a, &[Some("web-app"), Some("docs")]);
        let mut socks = vec![];
        for i in 0..2 {
            let (ev_tx, ev_rx) = mpsc::channel();
            let (in_tx, in_rx) = mpsc::channel();
            let mut t = TerminalState::new(TerminalConn { events: ev_rx, input: in_tx });
            t.id = (i + 1).to_string();
            a.panes[i].term = Some(t);
            socks.push((ev_tx, in_rx));
        }
        a.run_intent("test", Intent::Pages(PageOp::Per(2)));
        let mut rt = Runtime::new(a, super::super::view, (1284.0, 816.0), rune::Env::new(true, 1.0));
        settle(&mut rt);
        (rt, socks)
    }

    fn settle(rt: &mut Runtime<App>) {
        for _ in 0..8 {
            rt.frame(0.05);
            rt.update(|s, tx| {
                s.poll(0.0, tx);
            });
        }
    }

    fn press(rt: &mut Runtime<App>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} is not drawn"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        settle(rt);
    }

    fn fill_of(rt: &Runtime<App>, key: &str) -> Option<rune::Color> {
        rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).map(|e| e.fill)
    }

    fn element(rt: &Runtime<App>, key: &str) -> Option<rune::ElementInfo> {
        rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key))
    }

    fn backgrounds(rt: &Runtime<App>) -> Vec<u32> {
        rt.state.panes.iter().filter_map(|p| p.term.as_ref()).map(|t| t.theme().background).collect()
    }

    /// The trigger sits beside the pane-count switch; a swatch repaints
    /// every open terminal and bar at once; the current one is checked,
    /// named in its label.
    #[test]
    fn a_pick_repaints_the_open_terminals_and_bars() {
        let (mut rt, _socks) = window();
        let per = element(&rt, "pager-per").expect("the pane-count switch");
        let btn = element(&rt, "pager-colors").expect("the trigger");
        assert!(btn.rect[0] >= per.rect[0] + per.rect[2] - 0.5 && btn.rect[0] - (per.rect[0] + per.rect[2]) < 12.0, "right beside it: {per:?} {btn:?}");
        assert_eq!(backgrounds(&rt), [WEB_BG; 2]);
        let bar0 = fill_of(&rt, "pane-1-bar");
        press(&mut rt, "pager-colors");
        assert!(element(&rt, "term-colors").is_some(), "the popover opens");
        let night = element(&rt, "term-colors-background-2").unwrap();
        assert_eq!(night.label.as_deref(), Some("Background: Night"));
        assert_eq!(night.help.as_deref(), Some("Night"), "its name as tooltip");
        assert_eq!(shown(&rt.state, Slot::Background), Pick::Color(WEB_BG), "Midnight checked");
        press(&mut rt, "term-colors-background-2");
        assert_eq!(backgrounds(&rt), [0x1a1b26; 2], "every terminal at once");
        assert_eq!(shown(&rt.state, Slot::Background), Pick::Color(0x1a1b26), "checked now");
        // The bar: Navy, under the group tint where there is one.
        press(&mut rt, "term-colors-bar-2");
        let bar1 = fill_of(&rt, "pane-1-bar");
        assert_ne!(bar0, bar1, "the bar follows");
        assert_eq!(bar_ground(None, true), 0x1c2f4a);
        assert_eq!(rt.state.store.get(KEY), &serde_json::json!({ "background": "#1a1b26", "bar": "#1c2f4a" }), "kept");
        // The palette opens the same popover.
        rt.update(|s, _| {
            s.pager_menu = None;
            s.run_cmd(palette::Run::TermColors);
        });
        settle(&mut rt);
        assert!(element(&rt, "term-colors").is_some());
        assert!(rt.state.palette_cmds().iter().any(|c| c.id == "term-colors" && c.label == "Terminal colours"));
    }

    const WEB_BG: u32 = 0x0d1117;

    /// Paper: the light rules, the very theme sbxw.toml's light value gives
    /// (cursor, selection, the pills' light-ground tones).
    #[test]
    fn paper_flips_the_light_rules() {
        let (mut rt, _socks) = window();
        rt.update(|s, _| s.set_term_color(Slot::Background, Slot::Background.parse("#fdf6e3")));
        settle(&mut rt);
        let t = rt.state.panes[0].term.as_ref().unwrap();
        assert_eq!(t.theme(), &Theme::from_setting_strings("#fdf6e3", "auto"), "sbxw.toml's path");
        assert_eq!((t.theme().foreground, t.theme().cursor), (0x24292f, 0x0969da));
        let green = crate::theme::ON_LIGHT_GROUND.iter().find(|(n, _)| *n == "green").unwrap().1;
        assert_eq!(t.pills.colors(Tone::Green), crate::emphasis::palette(Tone::Green, 0xfdf6e3, 0x24292f));
        assert_eq!(t.pills.colors(Tone::Green).bg, crate::theme::over(green, crate::emphasis::TINT, 0xfdf6e3), "the light ground's green");
        assert!(warnings().is_empty(), "Auto reads on Paper");
    }

    /// The custom field takes `#rrggbb` (and `auto` for the text) and
    /// refuses anything else as text; a pair under 4.5:1 is kept with a
    /// quiet warning.
    #[test]
    fn the_custom_field_refuses_what_is_not_a_colour() {
        let (mut rt, _socks) = window();
        press(&mut rt, "pager-colors");
        for bad in ["red", "#12345", "auto", "#12345g"] {
            rt.update(|s, _| {
                s.term_colors.edits[0] = TextEdit::new(bad);
                s.apply_custom_term_color(Slot::Background);
            });
            settle(&mut rt);
            assert!(element(&rt, "term-colors-background-refused").is_some(), "{bad} refused");
            assert!(rt.state.store.get(KEY).is_null(), "{bad} not kept");
            assert_eq!(backgrounds(&rt), [WEB_BG; 2]);
        }
        rt.update(|s, _| {
            s.term_colors.edits[2] = TextEdit::new("AUTO");
            s.apply_custom_term_color(Slot::Text);
            s.term_colors.edits[0] = TextEdit::new("#3A3A3A");
        });
        press(&mut rt, "term-colors-background-apply");
        assert!(element(&rt, "term-colors-background-refused").is_none());
        assert_eq!(backgrounds(&rt), [0x3a3a3a; 2]);
        assert!(element(&rt, "term-colors-warning-0").is_none(), "Auto reads on it");
        rt.update(|s, _| s.set_term_color(Slot::Text, Slot::Text.parse("#5a5a5a")));
        settle(&mut rt);
        assert_eq!(rt.state.panes[0].term.as_ref().unwrap().theme().foreground, 0x5a5a5a, "allowed");
        assert!(element(&rt, "term-colors-warning-0").is_some(), "with a warning");
    }

    /// The pick wins over sbxw.toml, Reset falls back to it, then to the
    /// built-in; an invalid sbxw.toml value is ignored; sbxw.toml is never
    /// written (only the state file's key moves).
    #[test]
    fn the_kept_choice_wins_over_sbxw_toml_and_reset_falls_back() {
        let (mut rt, _socks) = window();
        rt.update(|s, _| {
            s.feats.data.insert("daemon_state".into(), serde_json::json!({ "terminal_colors": { "background": "#002B36", "bar": "nope", "text": "auto" } }));
        });
        settle(&mut rt);
        assert_eq!(backgrounds(&rt), [0x002b36; 2], "sbxw.toml's");
        assert_eq!(tt::chosen().bar, None, "an invalid value is ignored");
        assert_eq!(rt.state.term_color_source(Slot::Background).0, Source::Toml);
        press(&mut rt, "pager-colors");
        assert_eq!(shown(&rt.state, Slot::Background), Pick::Color(0x002b36), "Solarized checked, from sbxw.toml");
        assert_eq!(element(&rt, "term-colors-background-reset").unwrap().label.as_deref(), Some("Default: sbxw.toml"), "nothing to reset");
        press(&mut rt, "term-colors-background-7");
        assert_eq!(backgrounds(&rt), [0xfdf6e3; 2], "the pick wins");
        assert_eq!(element(&rt, "term-colors-background-reset").unwrap().label.as_deref(), Some("Reset to sbxw.toml"));
        press(&mut rt, "term-colors-background-reset");
        assert_eq!(backgrounds(&rt), [0x002b36; 2], "back to sbxw.toml");
        assert!(rt.state.store.get(KEY).is_null());
        // A daemon without the field: the built-in.
        rt.update(|s, _| {
            s.feats.data.insert("daemon_state".into(), serde_json::json!({ "versions": {} }));
        });
        settle(&mut rt);
        assert_eq!(backgrounds(&rt), [WEB_BG; 2]);
        assert_eq!(element(&rt, "term-colors-background-reset").unwrap().label.as_deref(), Some("Default: built-in"));
    }

    /// The kept choice survives a restart (a state file of the test's own).
    #[test]
    fn the_choice_is_kept_across_a_restart() {
        let p = std::env::temp_dir().join(format!("sbxw-term-colors-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let mut s = store::Store::at(Some(p.clone()));
        s.set_in(KEY, "text", J::String("#ffb000".into()));
        let again = store::Store::at(Some(p.clone()));
        assert_eq!(kept(&again, Slot::Text), Some(Pick::Color(0xffb000)));
        assert_eq!(resolve(&again, Some(&serde_json::json!({ "terminal_colors": { "text": "#ffffff" } }))).text, Some(Pick::Color(0xffb000)), "over sbxw.toml");
        std::fs::remove_file(&p).ok();
    }
}
