//! The app's keyboard: its shortcuts and the command palette (Cmd/Ctrl+K).
//!
//! The palette is the core's own list over a scrim: a filter field, then
//! every sandbox to open, every top bar extra (New, the requests, Setup,
//! Help, the Mac, ...), the features' shortcuts and the core's own actions
//! (sidebar, Jarvis, panes, layout, zoom), each with its shortcut. A pick
//! runs the same intents the buttons run, through the same checks: an
//! intent that needs the person's approval still opens the core's dialog.
//!
//! The shortcuts never reach a terminal, and none takes what a terminal
//! needs: while a terminal's grid has the keyboard, only Cmd chords (macOS)
//! or Ctrl+Shift chords (elsewhere) are the app's; elsewhere plain Ctrl
//! chords are too. Push-to-talk and F7 stay Jarvis's (jarvis.rs).

use super::*;
use crate::feat;
use rune::TextEdit;

/// The palette while it is open: its filter and the highlighted row.
pub struct Palette {
    pub edit: TextEdit,
    pub sel: usize,
}

/// What a palette row does.
#[derive(Clone, Debug, PartialEq)]
pub enum Run {
    /// Intents, checked as any (an approval opens the core's dialog).
    Intents(Vec<(String, J)>),
    Sidebar,
    Jarvis,
    FocusPane(usize),
    ClosePane,
    Layout(usize, Split),
    Zoom(i32),
}

/// One row of the palette.
#[derive(Clone, Debug)]
pub struct Cmd {
    /// Stable: the row's key is `palette-<id>`.
    pub id: String,
    pub label: String,
    pub hint: String,
    pub icon: Option<String>,
    pub keys: Vec<String>,
    pub run: Run,
}

fn mac() -> bool {
    cfg!(target_os = "macos")
}

/// A chord's keycaps on this platform: `["⌘", "K"]` or `["Ctrl", "K"]`.
pub fn chord_keys(k: &str) -> Vec<String> {
    if mac() { vec!["⌘".into(), k.into()] } else { vec!["Ctrl".into(), k.into()] }
}

/// The app's chords, for the palette, Help (`data.app.shortcuts`) and the
/// tooltips: (keys, what it does).
pub fn shortcuts(ptt: &str) -> Vec<(Vec<String>, String)> {
    let mut v: Vec<(Vec<String>, String)> = vec![
        (chord_keys("K"), "Open the command palette".into()),
        (chord_keys("B"), "Show or hide the sidebar".into()),
        (if mac() { chord_keys("J") } else { vec!["F7".into()] }, "Open or close Jarvis".into()),
        (chord_keys("1…9"), "Give pane 1 to 9 the keyboard".into()),
        (chord_keys("W"), "Close the focused pane (the sandbox and its session go on)".into()),
        (chord_keys("+ / − / 0"), "Zoom the app in, out, back to 100% (in a terminal: its text size)".into()),
        (vec![ptt.to_string()], "Push to talk to Jarvis (hold)".into()),
        (vec!["Escape".into()], "Close the menu, dialog or palette on top".into()),
        (vec!["↑".into(), "↓".into()], "Move in the sandbox list".into()),
        (vec!["Enter".into()], "Open the sandbox's terminal".into()),
        (vec!["Space".into()], "Start or stop the sandbox".into()),
        (vec!["Tab".into()], "Go to the next control".into()),
    ];
    for sc in feat::FEATURES.iter().flat_map(|f| f.shortcuts.iter()) {
        let k = sc.key.to_ascii_uppercase().to_string();
        v.push((if mac() { vec!["⌃".into(), "⌥".into(), k] } else { vec!["Alt".into(), k] }, sc.help.to_string()));
    }
    v.push((if mac() { vec!["⌘".into(), "V".into()] } else { vec!["Ctrl".into(), "Shift".into(), "V".into()] }, "Paste into the focused terminal".into()));
    v.push((vec!["Shift".into(), "PageUp".into()], "Scroll a terminal's history".into()));
    if !mac() {
        v.push((vec!["Ctrl".into(), "Shift".into(), "K".into()], "The same chords with Shift while a terminal has the keyboard (Ctrl+Shift+K, +B, +1…9, +W)".into()));
    }
    v
}

/// The app's chord in `ev`, if it is one it may take now: the letter or
/// digit, lowercased. `terminal`: a terminal's grid has the keyboard.
pub fn chord(ev: &rune::KeyEvent, terminal: bool) -> Option<char> {
    let m = ev.mods;
    let rune::Key::Command(c) = ev.key else { return None };
    let c = match c {
        '!' => '1',
        '@' => '2',
        '#' => '3',
        '$' => '4',
        '%' => '5',
        '^' => '6',
        '&' => '7',
        '*' => '8',
        '(' => '9',
        ')' => '0',
        '_' => '-',
        '+' => '=',
        c => c.to_ascii_lowercase(),
    };
    if mac() {
        (m.logo && !m.ctrl && !m.alt).then_some(c)
    } else {
        (m.ctrl && !m.alt && !m.logo && (m.shift || !terminal)).then_some(c)
    }
}

/// Whether every word of `q` is in `text` (case-insensitive).
pub fn matches(q: &str, text: &str) -> bool {
    let t = text.to_lowercase();
    q.split_whitespace().all(|w| t.contains(&w.to_lowercase()))
}

pub const ZOOM_STEPS: &[f32] = &[0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

impl App {
    /// The app's zoom (remembered; `SBXW_DESKTOP_ZOOM` for shots).
    pub fn zoom(&self) -> f32 {
        let env = std::env::var("SBXW_DESKTOP_ZOOM").ok().and_then(|z| z.parse::<f32>().ok());
        env.or_else(|| self.store.get("zoom").as_f64().map(|z| z as f32)).unwrap_or(1.0).clamp(0.8, 2.0)
    }

    fn zoom_by(&mut self, step: i32) {
        let z = self.zoom();
        let next = match step {
            0 => 1.0,
            s if s > 0 => ZOOM_STEPS.iter().copied().find(|x| *x > z + 0.01).unwrap_or(2.0),
            _ => ZOOM_STEPS.iter().rev().copied().find(|x| *x < z - 0.01).unwrap_or(0.8),
        };
        self.store.set("zoom", if (next - 1.0).abs() < 0.01 { J::Null } else { serde_json::json!(next) });
        self.notify(format!("Zoom {}%", (next * 100.0).round()), false);
    }

    /// Every row the palette can show, in its order: sandboxes (what wants
    /// you first), then the top bar's and the features' actions, then the core's.
    pub fn palette_cmds(&self) -> Vec<Cmd> {
        let mut v = vec![];
        let mut sbs: Vec<&crate::client::Sandbox> = self.sandboxes.iter().collect();
        let wants = |name: &str| self.raw["sessions"].as_array().is_some_and(|a| a.iter().any(|s| s["sandbox"] == name && s["state"] == "attention"));
        sbs.sort_by_key(|s| (!wants(&s.name), s.status != "running"));
        for sb in sbs {
            let word = if sb.unreachable_since.is_some() { "unreachable".to_string() } else if wants(&sb.name) { "wants you".to_string() } else { sb.status.clone() };
            v.push(Cmd {
                id: format!("open-{}", sb.name),
                label: format!("Open {}", sb.name),
                hint: format!("{word} · {}", sb.agent),
                icon: Some("terminal".into()),
                keys: vec![],
                run: Run::Intents(vec![("select".into(), serde_json::json!({ "sandbox": sb.name })), ("open_terminal".into(), serde_json::json!({ "sandbox": sb.name }))]),
            });
        }
        for sb in &self.sandboxes {
            let (verb, intent) = match sb.status.as_str() {
                "running" => ("Stop", "stop_sandbox"),
                "stopped" => ("Start", "start_sandbox"),
                _ => continue,
            };
            v.push(Cmd { id: format!("{}-{}", verb.to_lowercase(), sb.name), label: format!("{verb} {}", sb.name), hint: sb.status.clone(), icon: Some(if verb == "Stop" { "stop" } else { "play" }.into()), keys: vec![], run: Run::Intents(vec![(intent.into(), serde_json::json!({ "sandbox": sb.name }))]) });
        }
        // The top bar's extras, as their buttons run them.
        let data = self.screen_data();
        for e in data["app"]["extras"]["topbar"].as_array().into_iter().flatten() {
            let Some(name) = e["intent"]["name"].as_str() else { continue };
            let label = match e["label"].as_str().filter(|l| !l.is_empty()) {
                Some(l) => l.to_string(),
                None => e["help"].as_str().unwrap_or("").split(':').next().unwrap_or("").to_string(),
            };
            let label = match e["count"].as_u64() {
                Some(n) => format!("{label} ({n})"),
                None => label,
            };
            let keys = if e["id"] == "help" { vec!["?".to_string()] } else { vec![] };
            v.push(Cmd { id: format!("top-{}", e["id"].as_str().unwrap_or("")), label, hint: e["help"].as_str().unwrap_or("").to_string(), icon: e["icon"].as_str().map(str::to_string), keys, run: Run::Intents(vec![(name.to_string(), e["intent"]["args"].clone())]) });
        }
        for sc in feat::FEATURES.iter().flat_map(|f| f.shortcuts.iter()) {
            let k = sc.key.to_ascii_uppercase().to_string();
            let keys = if mac() { vec!["⌃".into(), "⌥".into(), k] } else { vec!["Alt".into(), k] };
            v.push(Cmd { id: format!("key-{}", sc.key), label: sc.help.to_string(), hint: String::new(), icon: None, keys, run: Run::Intents(vec![(sc.intent.to_string(), serde_json::from_str(sc.args).unwrap_or(J::Null))]) });
        }
        let core = |id: &str, label: &str, hint: &str, icon: &str, keys: Vec<String>, run: Run| Cmd { id: id.into(), label: label.into(), hint: hint.into(), icon: Some(icon.into()), keys, run };
        v.push(core("sidebar", if self.sidebar_shown() { "Hide the sidebar" } else { "Show the sidebar" }, "", "columns", chord_keys("B"), Run::Sidebar));
        v.push(core("jarvis", if self.jarvis.open { "Close Jarvis" } else { "Open Jarvis" }, "", "sparkle", if mac() { chord_keys("J") } else { vec!["F7".into()] }, Run::Jarvis));
        for i in 0..self.layout.0 {
            let what = self.panes.get(i).and_then(|p| p.sandbox.clone()).unwrap_or_else(|| "empty".into());
            v.push(core(&format!("pane-{}", i + 1), &format!("Go to pane {}", i + 1), &what, "single", chord_keys(&(i + 1).to_string()), Run::FocusPane(i)));
        }
        v.push(core("close-pane", "Close the focused pane", "The sandbox and its session go on", "close", chord_keys("W"), Run::ClosePane));
        for (n, split, label, icon) in [(1, Split::Columns, "1 terminal", "single"), (2, Split::Columns, "2 side by side", "columns"), (2, Split::Rows, "2 stacked", "rows"), (3, Split::Columns, "3 terminals", "grid"), (4, Split::Columns, "4 terminals", "grid")] {
            v.push(core(&format!("layout-{n}{}", if split == Split::Rows { "r" } else { "" }), &format!("Layout: {label}"), "", icon, vec![], Run::Layout(n, split)));
        }
        v.push(core("zoom-in", "Zoom in", &format!("now {}%", (self.zoom() * 100.0).round()), "plus", chord_keys("+"), Run::Zoom(1)));
        v.push(core("zoom-out", "Zoom out", "", "search", chord_keys("−"), Run::Zoom(-1)));
        v.push(core("zoom-reset", "Actual size", "100%", "search", chord_keys("0"), Run::Zoom(0)));
        v
    }

    /// The rows the filter keeps.
    pub fn palette_shown(&self) -> Vec<Cmd> {
        let q = self.palette.as_ref().map(|p| p.edit.text.clone()).unwrap_or_default();
        self.palette_cmds().into_iter().filter(|c| matches(&q, &format!("{} {}", c.label, c.hint))).collect()
    }

    pub fn palette_open(&mut self) {
        self.palette = Some(Palette { edit: TextEdit::new(""), sel: 0 });
        self.bar_menu = None;
        for s in &mut self.screens {
            s.close_menus();
        }
    }

    pub fn palette_close(&mut self) {
        if self.palette.take().is_some() {
            self.refocus = true;
        }
    }

    /// Runs the highlighted row (or row `i`), then closes the palette.
    pub fn palette_run(&mut self, i: Option<usize>) {
        let shown = self.palette_shown();
        let Some(p) = &self.palette else { return };
        let Some(c) = shown.get(i.unwrap_or(p.sel)).cloned() else { return };
        self.palette = None;
        self.refocus = true;
        self.run_cmd(c.run);
    }

    /// What a palette row or a shortcut does.
    pub fn run_cmd(&mut self, run: Run) {
        match run {
            Run::Intents(list) => {
                for (name, args) in list {
                    self.press_extra("palette", &name, &args);
                }
            }
            Run::Sidebar => self.toggle_sidebar(),
            Run::Jarvis => self.toggle_panel(None),
            Run::FocusPane(i) if i < self.layout.0 => {
                self.focused = i;
                self.touch(i);
                self.refocus = true;
            }
            Run::FocusPane(_) => {}
            Run::ClosePane => {
                let i = self.focused;
                self.remove_pane(i);
            }
            Run::Layout(n, split) => self.press_extra("palette", "set_layout", &serde_json::json!({ "panes": n, "split": if split == Split::Rows { "rows" } else { "columns" } })),
            Run::Zoom(step) => self.zoom_by(step),
        }
        self.push_data();
    }

    /// The app's own chords (see the module's comment). True when taken.
    /// `focused`: the key of what has the keyboard (Rune's `Tx::focused`).
    pub fn app_key(&mut self, ev: &rune::KeyEvent, focused: Option<&str>) -> bool {
        let terminal = focused.is_some_and(|k| k.starts_with("grid"));
        let Some(c) = chord(ev, terminal) else { return false };
        let run = match c {
            'k' => {
                if self.palette.is_some() {
                    self.palette_close();
                } else {
                    self.palette_open();
                }
                self.push_data();
                return true;
            }
            'b' => Run::Sidebar,
            'j' if !mac() => Run::Jarvis,
            'w' => Run::ClosePane,
            '1'..='9' => Run::FocusPane(c as usize - '1' as usize),
            // In a terminal these size its text (terminal/keys.rs).
            '=' | '-' | '0' if !terminal => Run::Zoom(match c {
                '=' => 1,
                '-' => -1,
                _ => 0,
            }),
            _ => return false,
        };
        self.run_cmd(run);
        true
    }
}

/// The palette over a scrim, when open.
pub fn view(s: &App) -> Option<El> {
    let p = s.palette.as_ref()?;
    let shown = s.palette_shown();
    let sel = p.sel.min(shown.len().saturating_sub(1));
    let w = 560.0f32.min(s.window.0 - 32.0);
    let side = ((s.window.0 - w) / 2.0).max(16.0);
    let input = rune::text_input(&p.edit, crate::screen::body(14.0, 400.0))
        .single_line()
        .key("palette-input")
        .label("Command palette: type to filter")
        .placeholder("Type a command or a sandbox name", c("text-dim"))
        .color(c("text-bright"))
        .autofocus()
        .grow(1.0)
        .min_w(0.0)
        .on_edit(|s: &mut App| &mut s.palette.as_mut().expect("palette open").edit)
        .on_input_change(|s: &mut App, _tx| {
            if let Some(p) = s.palette.as_mut() {
                p.sel = 0;
            }
        })
        .on_input_key(|s: &mut App, ev: &rune::KeyEvent, _tx| {
            let n = s.palette_shown().len();
            match ev.key {
                Key::Down | Key::Up if n > 0 => {
                    if let Some(p) = s.palette.as_mut() {
                        let cur = p.sel.min(n - 1);
                        p.sel = if ev.key == Key::Down { (cur + 1) % n } else { (cur + n - 1) % n };
                    }
                    true
                }
                Key::Enter => {
                    s.palette_run(None);
                    true
                }
                Key::Escape => {
                    s.palette_close();
                    true
                }
                _ => false,
            }
        });
    let field = row()
        .w_full()
        .gap(8.0)
        .px(16.0)
        .h(48.0)
        .items_center()
        .child(rune::icon(crate::icons::get("search").unwrap(), 16.0).color(c("text-muted")))
        .child(input);
    let mut list = col().key("palette-list").role("listbox").w_full().gap(2.0).padding(8.0);
    if shown.is_empty() {
        list = list.child(text(format!("Nothing matches “{}”.", p.edit.text)).font(crate::screen::body(13.0, 400.0)).color(c("text-muted")).wrap());
    }
    for (i, cmd) in shown.iter().enumerate() {
        let on = i == sel;
        let mut r = row()
            .key(format!("palette-{}", cmd.id))
            .role("option")
            .label(format!("{} {}", cmd.label, cmd.hint))
            .h(36.0)
            .w_full()
            .px(8.0)
            .gap(12.0)
            .items_center()
            .radius(theme::radius("xs").unwrap())
            .cursor(Cursor::Pointer)
            .hover(c("hover-subtle"))
            .on_press(move |s: &mut App| s.palette_run(Some(i)));
        if on {
            r = r.fill(c("elevated"));
        }
        r = r.child(match cmd.icon.as_deref().and_then(crate::icons::get) {
            Some(ic) => rune::icon(ic, 16.0).color(c(if on { "text-bright" } else { "text-muted" })),
            None => el().w(16.0),
        });
        r = r.child(text(cmd.label.clone()).font(crate::screen::body(13.0, 500.0)).color(c("text-bright")).truncate().min_w(0.0));
        r = r.child(text(cmd.hint.clone()).font(crate::screen::body(12.0, 400.0)).color(c("text-muted")).truncate().grow(1.0).min_w(0.0));
        let mut caps = row().gap(4.0).items_center();
        for k in &cmd.keys {
            caps = caps.child(el().px(6.0).h(20.0).items_center().radius(4.0).fill(c("surface-alt")).border(1.0, c("border")).child(text(k.clone()).font(crate::screen::body(12.0, 500.0)).color(c("text"))));
        }
        list = list.child(r.child(caps));
    }
    let foot = row()
        .w_full()
        .px(16.0)
        .h(32.0)
        .items_center()
        .child(text("↑ ↓ to move · Enter to run · Esc to close").font(crate::screen::body(12.0, 400.0)).color(c("text-muted")));
    let card = col()
        .key("palette")
        .role("dialog")
        .label("Command palette")
        .pin(Some(side), Some(80.0), Some(side), None)
        .max_h((s.window.1 - 120.0).max(200.0))
        .fill(c("surface"))
        .border(1.0, c("border"))
        .radius(theme::radius("lg").unwrap())
        .shadow(8.0, 24.0, rune::color::hex("#00000073").unwrap())
        .on_press(|_s: &mut App| {})
        .child(field)
        .child(el().h(1.0).w_full().fill(c("elevated")))
        .child(col().w_full().max_h((s.window.1 - 240.0).clamp(120.0, 440.0)).scroll(rune::ScrollSpec { stick_bottom: false }).child(list))
        .child(el().h(1.0).w_full().fill(c("elevated")))
        .child(foot);
    Some(super::scrim("palette-scrim", card, |s: &mut App| {
        s.palette_close();
        s.push_data();
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(c: char, cmd: bool, ctrl: bool, shift: bool) -> rune::KeyEvent {
        rune::KeyEvent { key: rune::Key::Command(c), mods: rune::Mods { cmd, ctrl, logo: cmd && mac(), shift, alt: false } }
    }

    /// In a terminal only Cmd (macOS) or Ctrl+Shift (elsewhere) chords are
    /// the app's: plain Ctrl+K, Ctrl+B, Ctrl+W stay the shell's.
    #[test]
    fn chords_leave_a_terminal_its_keys() {
        if mac() {
            assert_eq!(chord(&key('k', true, false, false), true), Some('k'));
            assert_eq!(chord(&key('k', false, true, false), true), None, "Ctrl+K is the shell's");
        } else {
            assert_eq!(chord(&key('k', true, true, false), false), Some('k'), "outside a terminal Ctrl+K opens the palette");
            assert_eq!(chord(&key('k', true, true, false), true), None, "in a terminal Ctrl+K is readline's");
            assert_eq!(chord(&key('w', true, true, false), true), None, "Ctrl+W deletes a word in a shell");
            assert_eq!(chord(&key('b', true, true, false), true), None, "Ctrl+B is tmux's");
            assert_eq!(chord(&key('k', true, true, true), true), Some('k'), "Ctrl+Shift+K in a terminal");
            assert_eq!(chord(&key('!', true, true, true), true), Some('1'));
        }
        let plain = rune::KeyEvent { key: rune::Key::Text("k".into()), mods: rune::Mods::default() };
        assert_eq!(chord(&plain, false), None);
    }

    #[test]
    fn the_filter_keeps_rows_with_every_word() {
        assert!(matches("open check", "Open better-checkout wants you"));
        assert!(!matches("open docs", "Open better-checkout"));
        assert!(matches("", "anything"));
    }
}
