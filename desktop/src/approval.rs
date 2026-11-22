//! Sensitive approvals, drawn by the core: Jarvis's proposals (a diff to
//! approve or refuse) and OSC 52 clipboard offers.
//!
//! The rule this module exists for: **a screen never draws or answers an
//! approval.** An intent whose registered risk is
//! [`Risk::Approval`](crate::intent::Risk::Approval) is not run when a
//! screen returns it: `App::intents` (shell.rs) turns `approve_proposal`
//! into "show that proposal's dialog", and [`proposal_view`] draws the dialog
//! in the core's layer, above every screen, from Rust only; only the
//! person's press on that dialog approves. Luau cannot see the dialog,
//! cannot press its buttons, and cannot draw over it (screens are laid out
//! under it).
//!
//! The Jarvis dialog follows `assets/js/jarvis.js`: the summary and the
//! diffs are the sandbox's words and are shown as plain text only; the
//! risk badge; a note when it is high risk (a spoken or typed yes does not
//! approve it); per change its kind, title, the command a stdio MCP server
//! would run on this machine (red banner), for a desktop screen the intents
//! it can emit, and the unified diff (monospace, coloured, scrollable);
//! Approve, off until "I have read this command" is ticked when the daemon
//! says `needs_ack`; Refuse with an optional reason; the daemon's refusal
//! (409) shown beside the buttons; once answered, the result per change.

use crate::intent::Intent;
use rune::{El, TextEdit};
use serde_json::Value as J;

/// An intent waiting for the person's answer.
pub struct Pending {
    /// The screen that asked.
    pub screen: String,
    pub intent: Intent,
    /// What the dialog asks (the registry's text).
    pub asks: &'static str,
}

/// What the proposal dialog's buttons do, in the app.
pub trait ProposalAnswers: Sized + 'static {
    /// Approve, by a click: `via: "click"`, with the acknowledgement when ticked.
    fn approve_click(&mut self);
    fn refuse_click(&mut self);
    /// "I have read this command".
    fn toggle_ack(&mut self);
    /// ✕: later. The proposal stays pending; the panel can bring it back.
    fn close_dialog(&mut self);
    /// After an answer: the next pending proposal, or close.
    fn next_dialog(&mut self);
    /// The refusal reason's text.
    fn reason_edit(&mut self) -> &mut TextEdit;
}

/// The change kinds, as the person reads them (jarvis.js, `JV_KINDS`).
pub fn kind_label(kind: &str) -> &str {
    match kind {
        "kit_write" => "kit",
        "kit_remove" => "kit removed",
        "kits_line" => "kits line",
        "setting" => "setting",
        "voice_word" => "pronunciation",
        "network_allow" => "network",
        "mcp_server" => "MCP server",
        "mcp_remove" => "MCP server removed",
        "desktop_screen" => "desktop screen",
        "desktop_screen_remove" => "desktop screen removed",
        k => k,
    }
}

/// What a desktop screen change lets the screen do, in plain words: the
/// daemon's `intents: [{name, risk, new}]`. `None` for other kinds.
pub fn screen_intents(change: &J) -> Option<Vec<(String, bool, bool)>> {
    let list = change["intents"].as_array()?;
    Some(
        list.iter()
            .map(|i| {
                let name = i["name"].as_str().unwrap_or("?").to_string();
                let risk = i["risk"].as_str().unwrap_or("");
                let words = crate::intent::spec(&name).map(|s| s.doc.split(['.', '(']).next().unwrap_or(s.doc).trim().to_string()).unwrap_or_else(|| name.replace('_', " "));
                (format!("{words} (`{name}`)"), i["new"] == true, risk.starts_with("approval"))
            })
            .collect(),
    )
}

/// What one change came to, once the proposal is answered (jarvis.js, `jvChangeResult`).
pub fn change_result(p: &J, i: usize) -> Option<(Option<bool>, String, String)> {
    let state = p["state"].as_str().unwrap_or("");
    if state == "pending" {
        return None;
    }
    if let Some(r) = p["results"].as_array().and_then(|r| r.get(i)) {
        let ok = r["ok"] == true;
        let text = if ok {
            if r["restart"] == true {
                "Applied, takes effect when the daemon restarts"
            } else {
                "Applied"
            }
        } else {
            "Failed"
        };
        return Some((Some(ok), text.into(), r["message"].as_str().unwrap_or("").into()));
    }
    Some(match state {
        "applied" => (Some(true), "Applied".into(), String::new()),
        "refused" => (None, "Not applied (refused)".into(), String::new()),
        _ => (Some(false), "Not applied".into(), String::new()),
    })
}

/// What the dialog needs from the app's state.
pub struct DialogState<'a> {
    pub proposal: &'a J,
    /// Other pending proposals.
    pub waiting: usize,
    pub acked: bool,
    /// The daemon's refusal of the last answer (409: changed since proposal...).
    pub error: Option<&'a str>,
    pub busy: bool,
    pub reason: &'a TextEdit,
    /// Insets from the window's edges: left, top, right; and the height left.
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub max_h: f32,
}

/// The proposal dialog (key `jv-dialog`; buttons `jv-approve`, `jv-refuse`,
/// `jv-ack`, `jv-close`, `jv-next`; the reason `jv-reason`).
pub fn proposal_view<S: ProposalAnswers>(d: &DialogState) -> El {
    use crate::theme::{c, radius};
    use rune::{col, row, text, Cursor, ScrollSpec, Span};
    let p = d.proposal;
    let pending = crate::jarvis::pending(p);
    let high = crate::jarvis::screen_only(p);
    let needs_ack = crate::jarvis::needs_ack(p);
    let small = |t: String, size: f32, weight: f32, color: &str| text(t).font(crate::screen::body(size, weight)).color(c(color)).wrap_anywhere().w_full().min_w(0.0);
    let badge = |t: &str, tone: &str| {
        let (fg, tint) = crate::screen::tone(tone);
        rune::el().px(crate::theme::spacing::CHIP_X).py(crate::theme::spacing::CHIP_Y).radius(radius("xs").unwrap()).fill(crate::theme::hex(tint)).child(text(t.to_string()).font(crate::screen::body(crate::theme::font_size::CAPTION, 500.0)).color(crate::theme::hex(fg)))
    };
    let button = |key: &str, label: &str, primary: bool, enabled: bool| {
        let b = rune::el()
            .key(key)
            .role("button")
            .label(label)
            .px(crate::theme::spacing::MD)
            .py(crate::theme::spacing::XS).min_h(crate::theme::layout::CONTROL_H).items_center()
            .radius(radius("sm").unwrap())
            .fill(c(if primary { "primary" } else { "elevated" }))
            .border(1.0, c(if primary { "primary" } else { "border" }))
            .child(text(label.to_string()).font(crate::screen::body(crate::theme::font_size::BODY, 500.0)).color(c(if primary { "on-primary" } else { "text-bright" })));
        if enabled {
            b.hover(c(if primary { "primary-hover" } else { "border" })).cursor(Cursor::Pointer)
        } else {
            b.opacity(crate::theme::DISABLED_OPACITY).disabled(true)
        }
    };

    let mut head = row().w_full().gap(crate::theme::spacing::SM).items_center().child(text("Jarvis proposes").font(crate::screen::body(crate::theme::font_size::HEADING, 600.0)).color(c("text-bright"))).child(badge(if high { "High risk" } else { "Low risk" }, if high { "red" } else { "green" }));
    if d.waiting > 0 {
        head = head.child(text(format!("{} more waiting", d.waiting)).font(crate::screen::body(crate::theme::font_size::CAPTION, 400.0)).color(c("text-muted")));
    }
    head = head.child(rune::el().grow(1.0)).child(
        rune::el()
            .key("jv-close")
            .role("button")
            .label("Later")
            .help("Later: the proposal waits, the Jarvis panel brings it back")
            .padding(crate::theme::spacing::XS)
            .radius(radius("xs").unwrap())
            .hover(c("border"))
            .cursor(Cursor::Pointer)
            .on_press(|s: &mut S| s.close_dialog())
            .child(rune::icon(crate::icons::get("close").unwrap(), 14.0).color(c("text-muted"))),
    );

    let mut body = col().w_full().min_w(0.0).gap(crate::theme::spacing::MD);
    body = body.child(small(p["summary"].as_str().filter(|s| !s.is_empty()).unwrap_or("(no summary)").to_string(), 13.0, 400.0, "text-bright").key("jv-summary"));
    if high && pending {
        body = body.child(small("High risk: read every change below before you approve. A spoken or typed yes does not approve this one: click Approve.".into(), 11.5, 400.0, "amber").key("jv-high-note"));
    }
    let changes = p["changes"].as_array().cloned().unwrap_or_default();
    for (i, ch) in changes.iter().enumerate() {
        let mut item = col().key(format!("jv-change-{i}")).w_full().min_w(0.0).gap(crate::theme::spacing::INLINE);
        let mut h = row().w_full().gap(crate::theme::spacing::INLINE).items_center().child(badge(kind_label(ch["kind"].as_str().unwrap_or("change")), "neutral")).child(text(ch["title"].as_str().unwrap_or("").to_string()).font(crate::screen::body(crate::theme::font_size::CAPTION, 500.0)).color(c("text-bright")).wrap_anywhere().grow(1.0).min_w(0.0));
        if ch["risk"] == "high" && pending {
            h = h.child(badge("high risk", "red"));
        }
        let res = change_result(p, i);
        if let Some((ok, t, _)) = &res {
            h = h.child(badge(t, match ok {
                Some(true) => "green",
                Some(false) => "red",
                None => "neutral",
            }));
        }
        item = item.child(h);
        if let Some((ok, _, msg)) = &res {
            if !msg.is_empty() {
                item = item.child(small(msg.clone(), 11.0, 400.0, if *ok == Some(false) { "red" } else { "text-muted" }));
            }
        }
        if let Some(cmd) = ch["host_command"].as_str() {
            item = item.child(
                col()
                    .key(format!("jv-host-{i}"))
                    .role("alert")
                    .w_full()
                    .padding(crate::theme::spacing::SM)
                    .gap(crate::theme::spacing::XS)
                    .radius(radius("sm").unwrap())
                    .fill(c("red-tint"))
                    .border(1.0, c("red"))
                    .child(small("This command will run on this machine, outside any sandbox:".into(), 11.5, 600.0, "text-bright"))
                    .child(text(cmd.to_string()).font(crate::screen::body(crate::theme::font_size::CAPTION, 400.0).mono()).color(c("text-bright")).wrap_anywhere().w_full().min_w(0.0)),
            );
        }
        if let Some(list) = screen_intents(ch) {
            let mut box_ = col().key(format!("jv-intents-{i}")).w_full().gap(crate::theme::spacing::XXS).padding(crate::theme::spacing::SM).radius(radius("sm").unwrap()).fill(c("surface-alt")).border(1.0, c("elevated"));
            box_ = box_.child(small(if list.is_empty() { "This screen asks the app for nothing.".into() } else { "This screen can ask the app to:".into() }, 11.5, 600.0, "text-bright"));
            for (words, new, approval) in list {
                let mut line = words;
                if new {
                    line = format!("{line}  NEW: the current screen does not");
                }
                if approval {
                    line = format!("{line}  (needs your approval in a core dialog)");
                }
                box_ = box_.child(small(format!("• {line}"), 11.5, 400.0, if new { "amber" } else { "text" }));
            }
            item = item.child(box_);
        }
        if let Some(diff) = ch["diff"].as_str().filter(|d| !d.is_empty()) {
            // The sandbox's text, as text, coloured by line.
            let mut spans = vec![];
            let mut at = 0;
            for line in diff.split_inclusive('\n') {
                let colour = if line.starts_with("+++") || line.starts_with("---") {
                    Some("text-bright")
                } else if line.starts_with('+') {
                    Some("green")
                } else if line.starts_with('-') {
                    Some("red")
                } else if line.starts_with("@@") {
                    Some("blue")
                } else if line.starts_with('#') {
                    Some("amber")
                } else {
                    None
                };
                if let Some(col_) = colour {
                    spans.push(Span::new(at, at + line.len()).color(c(col_)));
                }
                at += line.len();
            }
            item = item.child(
                col()
                    .key(format!("jv-diff-{i}"))
                    .role("document")
                    .label(format!("Diff of {}", ch["title"].as_str().unwrap_or("this change")))
                    .scroll(ScrollSpec { stick_bottom: false })
                    .w_full()
                    .max_h(280.0)
                    .min_h(0.0)
                    .padding(crate::theme::spacing::SM)
                    .radius(radius("sm").unwrap())
                    .fill(c("bg"))
                    .border(1.0, c("elevated"))
                    .child(rune::rich(diff.to_string(), spans).font(crate::screen::body(crate::theme::font_size::CAPTION, 400.0).mono()).color(c("text-muted")).wrap_anywhere().w_full().min_w(0.0))
                    .on_at_bottom(move |_: &mut S, at: bool| set_more(&format!("jv-diff-{i}"), !at)),
            );
            if let Some(e) = more_below(&format!("jv-diff-{i}")) {
                item = item.child(e);
            }
        }
        body = body.child(item);
    }
    if changes.is_empty() {
        body = body.child(small("No change listed.".into(), 12.0, 400.0, "text-muted"));
    }
    if !pending {
        let state = p["state"].as_str().unwrap_or("");
        let (t, col_) = match state {
            "applied" => ("Applied.".to_string(), "green"),
            "refused" => ("Refused: nothing was changed.".to_string(), "text-muted"),
            _ => ("Failed.".to_string(), "red"),
        };
        let t = match p["error"].as_str() {
            Some(e) => format!("{t} {e}"),
            None => t,
        };
        body = body.child(small(t, 12.0, 500.0, col_).key("jv-outcome"));
    }
    if let Some(e) = d.error {
        body = body.child(small(e.to_string(), 12.0, 500.0, "red").key("jv-error"));
    }

    let mut foot = row().w_full().gap(crate::theme::spacing::SM).items_center();
    let mut ack_row = None;
    if pending {
        if needs_ack {
            let on = d.acked;
            let mut b = rune::el().w(16.0).h(16.0).radius(crate::theme::corner::BOX).border(1.0, c(if on { "blue" } else { "control-border" })).items_center().justify_center();
            if on {
                b = b.fill(c("blue")).child(rune::icon(crate::icons::get("check").unwrap(), 12.0).color(c("white")));
            }
            ack_row = Some(
                row()
                    .key("jv-ack")
                    .w_full()
                    .role("checkbox")
                    .checked(on)
                    .label("I have read this command")
                    .gap(crate::theme::spacing::INLINE)
                    .items_center()
                    .cursor(Cursor::Pointer)
                    .on_press(|s: &mut S| s.toggle_ack())
                    .child(b)
                    .child(text("I have read this command").font(crate::screen::body(crate::theme::font_size::BODY, 500.0)).color(c("text-bright")).wrap_anywhere().min_w(0.0)),
            );
        }
        let reason = rune::text_input(d.reason, crate::screen::body(crate::theme::font_size::CAPTION, 400.0))
            .single_line()
            .key("jv-reason")
            .color(c("text-bright"))
            .placeholder("Why not (optional, told to Jarvis)", c("text-dim"))
            .w_full()
            .on_edit(|s: &mut S| s.reason_edit());
        foot = foot
            .child(rune::el().grow(1.0).min_w(80.0).h(crate::theme::layout::CONTROL_H).px(crate::theme::spacing::SM).items_center().fill(c("bg")).border(1.0, c("control-border")).radius(radius("sm").unwrap()).child(reason))
            .child(if d.busy { button("jv-refuse", "Refuse", false, false) } else { button("jv-refuse", "Refuse", false, true).on_press(|s: &mut S| s.refuse_click()) });
        let can = !d.busy && (!needs_ack || d.acked);
        foot = foot.child(if can { button("jv-approve", "Approve", true, true).on_press(|s: &mut S| s.approve_click()) } else { button("jv-approve", "Approve", true, false) });
    } else {
        foot = foot.child(rune::el().grow(1.0)).child(button("jv-next", if d.waiting > 0 { "Next proposal" } else { "Close" }, false, true).on_press(|s: &mut S| s.next_dialog()));
    }

    col()
        .key("jv-dialog")
        .role("dialog")
        .label("Jarvis proposes a change")
        .pin(Some(d.left), Some(d.top), Some(d.right), None)
        .max_h(d.max_h.max(160.0))
        .padding(crate::theme::spacing::LG)
        .gap(crate::theme::spacing::MD)
        .radius(radius("lg").unwrap())
        .fill(c("surface"))
        .border(1.0, c(if high { "red-border" } else { "border" }))
        .shadow(crate::theme::elevation::OVERLAY.0, crate::theme::elevation::OVERLAY.1, crate::theme::shadow())
        .child(head)
        .child(watch::<S>(col().key("jv-body").scroll(ScrollSpec { stick_bottom: false }).w_full().min_h(0.0).max_h((d.max_h - 130.0).max(80.0)).child(body), "jv-body"))
        .children(more_below("jv-body"))
        .children(ack_row)
        .child(foot)
}

// ---- The generic confirmation, for every feature intent of risk Approval ----
//
// A feature (`src/feat/`) answers an intent of risk Approval with a
// `feat::Confirm` built in Rust from the daemon's data: a title, plain lines,
// an optional block (a command, a diff, fingerprints), an optional
// acknowledgement to tick, optional secret fields drawn here (Rune's secure
// inputs: masked, no copy, no accessibility text, wiped on drop), and the
// two labels. Nothing here comes from a screen, and no screen can see or
// press it: it is drawn in the core's layer above every screen.

/// What the confirmation's buttons do, in the app.
pub trait ConfirmAnswers: Sized + 'static {
    fn confirm_click(&mut self);
    fn cancel_click(&mut self);
    fn toggle_confirm_ack(&mut self);
    /// The optional "do not ask again" box ([`crate::feat::Confirm::dont_ask`]).
    fn toggle_confirm_dont_ask(&mut self);
    /// Secret field `i`'s edit.
    fn secret_edit(&mut self, i: usize) -> &mut TextEdit;
}

/// What the confirmation needs from the app's state.
pub struct ConfirmState<'a> {
    /// `None` while it loads (`feat::Action::Prepare`) or when preparing failed.
    pub confirm: Option<&'a crate::feat::Confirm>,
    /// The title while there is no `confirm` yet.
    pub title: &'a str,
    pub loading: bool,
    /// Preparing failed, or the daemon refused the call: shown beside the buttons.
    pub error: Option<&'a str>,
    pub busy: bool,
    pub acked: bool,
    /// The "do not ask again" box, ticked (never needed to confirm).
    pub dont_ask: bool,
    pub secrets: &'a [TextEdit],
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub max_h: f32,
}

impl ConfirmState<'_> {
    /// Confirm works: loaded, not busy, ticked if asked, every required secret filled.
    pub fn can_confirm(&self) -> bool {
        let Some(c) = self.confirm else { return false };
        !self.busy && (c.ack.is_none() || self.acked) && c.secrets.iter().enumerate().all(|(i, s)| s.optional || self.secrets.get(i).is_some_and(|e| !e.text.is_empty()))
    }
}

// ---- Nothing hidden: what scrolls says so ----
//
// Every text in these dialogs wraps anywhere (a path, a host, a command with
// no space is never cut at the edge). A block that is taller than its box
// scrolls, and while its end is out of view a line under it says so: the
// person is never left approving something whose end they cannot see.

thread_local! {
    static MORE: std::cell::RefCell<std::collections::HashSet<String>> = std::cell::RefCell::new(std::collections::HashSet::new());
}

fn set_more(key: &str, more: bool) {
    MORE.with(|m| {
        let mut m = m.borrow_mut();
        if more {
            m.insert(key.to_string());
        } else {
            m.remove(key);
        }
    });
}

/// Whether scroll box `key` has text below what it shows (as last laid out).
pub fn has_more(key: &str) -> bool {
    MORE.with(|m| m.borrow().contains(key))
}

fn watch<S: 'static>(e: El, key: &'static str) -> El {
    e.on_at_bottom(move |_: &mut S, at: bool| set_more(key, !at))
}

/// The line under a scroll box whose end is out of view.
fn more_below(key: &str) -> Option<El> {
    has_more(key).then(|| {
        rune::el()
            .key(format!("{key}-more"))
            .w_full()
            .child(rune::text("More below: scroll to read it all before you answer".to_string()).font(crate::screen::body(crate::theme::font_size::CAPTION, 600.0)).color(crate::theme::c("amber")).wrap_anywhere().w_full().min_w(0.0))
    })
}

/// The lines the dialog lists as granted (`Confirm::grant_lines`), in
/// order: exactly the strings Confirm sends.
pub fn granted_lines(c: &crate::feat::Confirm) -> Vec<String> {
    c.grants.iter().filter_map(|g| g.lines.clone()).flatten().collect()
}

/// The confirmation (key `cf-dialog`; buttons `cf-confirm`, `cf-cancel`,
/// `cf-ack`; secret fields `cf-secret-<i>`; lines `cf-line-<i>`; the block `cf-preview`).
pub fn confirm_view<S: ConfirmAnswers>(d: &ConfirmState) -> El {
    use crate::theme::{c, radius};
    use rune::{ScrollSpec, Span};
    let small = |t: String, size: f32, weight: f32, color: &str| text(t).font(crate::screen::body(size, weight)).color(c(color)).wrap_anywhere().w_full().min_w(0.0);
    let danger = d.confirm.is_some_and(|x| x.danger);
    let button = |key: &str, label: &str, primary: bool, enabled: bool| {
        let (fill, edge, hover) = match (primary, danger) {
            (true, true) => ("red-tint", "red", "red-border"),
            (true, false) => ("primary", "primary", "primary-hover"),
            _ => ("elevated", "border", "border"),
        };
        // Confirm while the call runs: its own colours and a spinner, no press.
        let working = d.busy && key == "cf-confirm" && !enabled;
        let ink = c(if primary && !danger { "on-primary" } else { "text-bright" });
        let mut b = rune::el()
            .key(key)
            .role("button")
            .label(label)
            .px(crate::theme::spacing::MD)
            .py(crate::theme::spacing::XS).min_h(crate::theme::layout::CONTROL_H).items_center()
            .radius(radius("sm").unwrap())
            .fill(c(fill))
            .border(1.0, c(edge))
            .min_w(0.0);
        if working {
            b = b.gap(crate::theme::spacing::INLINE).justify_center().child(crate::screen::busy_spinner(14.0, ink));
        }
        let b = b.child(text(label.to_string()).font(crate::screen::body(crate::theme::font_size::BODY, 500.0)).color(ink).wrap_anywhere().min_w(0.0));
        if working {
            return b.busy(true);
        }
        if enabled {
            b.hover(c(hover)).cursor(Cursor::Pointer)
        } else {
            b.opacity(crate::theme::DISABLED_OPACITY).disabled(true)
        }
    };
    let title = d.confirm.map(|x| x.title.as_str()).unwrap_or(d.title);
    let head = row()
        .w_full()
        .gap(crate::theme::spacing::SM)
        .items_center()
        .child(text(title.to_string()).font(crate::screen::body(crate::theme::font_size::HEADING, 600.0)).color(c("text-bright")).wrap_anywhere().grow(1.0).min_w(0.0).key("cf-title"))
        .child(
            rune::el()
                .key("cf-close")
                .role("button")
                .label("Cancel")
                .w(crate::theme::layout::DENSE_H)
                .h(crate::theme::layout::DENSE_H)
                .justify_center()
                .items_center()
                .radius(radius("xs").unwrap())
                .hover(c("border"))
                .cursor(Cursor::Pointer)
                .on_press(|s: &mut S| s.cancel_click())
                .child(rune::icon(crate::icons::get("close").unwrap(), 14.0).color(c("text-muted"))),
        );
    // The title scrolls with the body: a long one (a path, a host) never
    // pushes Cancel and Confirm out of the window.
    let mut body = col().w_full().min_w(0.0).gap(crate::theme::spacing::SM).child(head);
    if d.loading {
        body = body.child(small("Reading from the daemon…".into(), 12.0, 400.0, "text-muted").key("cf-loading"));
    }
    if let Some(x) = d.confirm {
        for (i, l) in x.lines.iter().enumerate() {
            body = body.child(text(l.clone()).font(crate::theme::prose(crate::theme::font_size::BODY, 400.0)).color(c("text-bright")).wrap_anywhere().w_full().min_w(0.0).key(format!("cf-line-{i}")));
        }
        let granted = granted_lines(x);
        if !granted.is_empty() {
            let mut list = col().key("cf-grants").w_full().gap(crate::theme::spacing::XXS).padding(crate::theme::spacing::SM).radius(radius("sm").unwrap()).fill(c("surface-alt")).border(1.0, c("elevated"));
            list = list.child(small("You grant:".into(), 11.5, 600.0, "text-bright"));
            for (i, g) in granted.iter().enumerate() {
                list = list.child(small(format!("• {g}"), 12.0, 400.0, "text").key(format!("cf-grant-{i}")));
            }
            body = body.child(list);
        }
        if let Some(p) = &x.preview {
            let mut spans = vec![];
            if p.diff {
                let mut at = 0;
                for line in p.text.split_inclusive('\n') {
                    let colour = if line.starts_with("+++") || line.starts_with("---") {
                        Some("text-bright")
                    } else if line.starts_with('+') {
                        Some("green")
                    } else if line.starts_with('-') {
                        Some("red")
                    } else if line.starts_with("@@") {
                        Some("blue")
                    } else {
                        None
                    };
                    if let Some(col_) = colour {
                        spans.push(Span::new(at, at + line.len()).color(c(col_)));
                    }
                    at += line.len();
                }
            }
            body = body.child(
                col()
                    .key("cf-preview")
                    .role("document")
                    .scroll(ScrollSpec { stick_bottom: false })
                    .w_full()
                    .max_h(260.0)
                    .min_h(0.0)
                    .padding(crate::theme::spacing::SM)
                    .radius(radius("sm").unwrap())
                    .fill(c("bg"))
                    .border(1.0, c("elevated"))
                    .child(rune::rich(p.text.clone(), spans).font(crate::screen::body(crate::theme::font_size::CAPTION, 400.0).mono()).color(c("text")).wrap_anywhere().w_full().min_w(0.0))
                    .on_at_bottom(|_: &mut S, at: bool| set_more("cf-preview", !at)),
            );
            if let Some(e) = more_below("cf-preview") {
                body = body.child(e);
            }
        }
        for (i, sec) in x.secrets.iter().enumerate() {
            let Some(edit) = d.secrets.get(i) else { continue };
            let label = if sec.optional { format!("{} (optional)", sec.label) } else { sec.label.clone() };
            let mut input = rune::text_input(edit, crate::screen::body(crate::theme::font_size::CAPTION, 400.0)).key(format!("cf-secret-{i}")).label(sec.label.clone()).color(c("text-bright")).w_full().on_edit(move |s: &mut S| s.secret_edit(i));
            if !sec.multiline {
                input = input.single_line();
            }
            if !sec.placeholder.is_empty() {
                input = input.placeholder(sec.placeholder.clone(), c("text-dim"));
            }
            let input = input.secure();
            body = body.child(small(label, 11.5, 500.0, "text-muted")).child(
                rune::el().w_full().min_h(crate::theme::layout::CONTROL_H).px(crate::theme::spacing::SM).py(crate::theme::spacing::XS).items_center().fill(c("bg")).border(1.0, c("control-border")).radius(radius("sm").unwrap()).child(input),
            );
        }
    }
    if let Some(e) = d.error {
        body = body.child(small(e.to_string(), 12.0, 500.0, "red").key("cf-error"));
    }
    // The acknowledgement box on its own line, above the buttons: it wraps,
    // and it never pushes Confirm past the dialog's edge.
    let checkbox = |key: &str, label: String, on: bool| {
        let mut b = rune::el().w(16.0).h(16.0).radius(crate::theme::corner::BOX).border(1.0, c(if on { "blue" } else { "control-border" })).items_center().justify_center();
        if on {
            b = b.fill(c("blue")).child(rune::icon(crate::icons::get("check").unwrap(), 12.0).color(c("white")));
        }
        row()
            .key(key)
            .w_full()
            .role("checkbox")
            .checked(on)
            .label(label.clone())
            .gap(crate::theme::spacing::INLINE)
            .items_center()
            .min_w(0.0)
            .cursor(Cursor::Pointer)
            .child(b)
            .child(text(label).font(crate::screen::body(crate::theme::font_size::BODY, 500.0)).color(c("text-bright")).wrap_anywhere().grow(1.0).min_w(0.0))
    };
    let ack_row = d.confirm.and_then(|x| x.ack.clone()).map(|ack| checkbox("cf-ack", ack, d.acked).on_press(|s: &mut S| s.toggle_confirm_ack()));
    // Unticked by default, and Confirm works either way.
    let dont_ask_row = d.confirm.and_then(|x| x.dont_ask.clone()).map(|l| checkbox("cf-dont-ask", l, d.dont_ask).on_press(|s: &mut S| s.toggle_confirm_dont_ask()));
    let (yes, no) = d.confirm.map(|x| (x.confirm.as_str(), x.cancel.as_str())).unwrap_or(("Confirm", "Cancel"));
    let cancel = button("cf-cancel", no, false, true).on_press(|s: &mut S| s.cancel_click());
    let confirm = if d.can_confirm() { button("cf-confirm", yes, true, true).on_press(|s: &mut S| s.confirm_click()) } else { button("cf-confirm", yes, true, false) };
    // Long labels: one button per line, full width, so both always fit.
    let foot = if yes.chars().count() + no.chars().count() > 36 {
        col().w_full().gap(crate::theme::spacing::INLINE).child(confirm.w_full()).child(cancel.w_full())
    } else {
        row().w_full().gap(crate::theme::spacing::SM).items_center().child(rune::el().grow(1.0)).child(cancel).child(confirm)
    };
    col()
        .key("cf-dialog")
        .role("dialog")
        // A press inside stays inside (the scrim under it cancels).
        .on_press(|_s: &mut S| {})
        .label(title.to_string())
        .pin(Some(d.left), Some(d.top), Some(d.right), None)
        .max_h(d.max_h.max(160.0))
        .padding(crate::theme::spacing::LG)
        .gap(crate::theme::spacing::MD)
        .radius(radius("lg").unwrap())
        .fill(c("surface"))
        .border(1.0, c(if danger { "red-border" } else { "border" }))
        .shadow(crate::theme::elevation::OVERLAY.0, crate::theme::elevation::OVERLAY.1, crate::theme::shadow())
        .child(watch::<S>(col().key("cf-body").scroll(ScrollSpec { stick_bottom: false }).w_full().min_h(0.0).max_h((d.max_h - 110.0).max(80.0)).child(body), "cf-body"))
        .children(more_below("cf-body"))
        .children(ack_row)
        .children(dont_ask_row)
        .child(foot)
}

// ---- OSC 52: a program in the sandbox asking to write the clipboard ----
//
// The web UI's policy (assets/js/panes.js, "An OSC 52 copy: done, offered or
// dropped, by size"), kept the same so both clients behave alike. The person
// copies out of the agent's terminal all day (Claude Code's own copy is OSC
// 52, and while a TUI holds the mouse the terminal makes no selection of its
// own), so a click on every copy would be too much. By decoded size:
// - up to [`OSC_COPY_QUIET_MAX`]: copied at once, with a one-line notice so a
//   copy nobody asked for is still noticed;
// - up to [`OSC_COPY_MAX`]: offered by a card the core draws (this module:
//   it is an approval surface, a screen cannot draw or press it), which
//   names the sandbox and the size, shows the start of the text with its
//   control and invisible characters made visible, and copies only on a
//   click on Copy. One offer at a time; a newer one replaces it;
// - above: dropped, with a notice.
// Writes only: the read form never reaches here (terminal.rs, `new_term`).

use rune::{col, row, text, Cursor, Tx};
use std::time::Instant;

pub const OSC_COPY_QUIET_MAX: usize = 4 * 1024;
pub const OSC_COPY_MAX: usize = 100 * 1024;
pub const OSC_PREVIEW_CHARS: usize = 80;
/// An offer nobody answers goes away after this long (as in the web UI).
pub const OSC_OFFER_SECS: u64 = 30;

/// What happens to an OSC 52 copy of `bytes` bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OscTier {
    Copy,
    Offer,
    Drop,
}

pub fn osc_tier(bytes: usize) -> OscTier {
    if bytes <= OSC_COPY_QUIET_MAX {
        OscTier::Copy
    } else if bytes <= OSC_COPY_MAX {
        OscTier::Offer
    } else {
        OscTier::Drop
    }
}

/// `512 B`, `4.0 KB` (the web UI's `oscSize`).
pub fn size_label(n: usize) -> String {
    if n < 1024 {
        format!("{n} B")
    } else {
        format!("{:.1} KB", n as f64 / 1024.0)
    }
}

/// The first [`OSC_PREVIEW_CHARS`] characters, with line breaks, tabs, C0
/// and C1 controls, DEL and the invisible characters ([`crate::safe::is_invisible`]) shown as visible
/// marks, so the preview is what will be pasted (the web UI's `oscPreview`).
pub fn osc_preview(text: &str) -> String {
    let mut out: String = text
        .chars()
        .take(OSC_PREVIEW_CHARS)
        .map(|c| match c as u32 {
            0x0a => '↵',
            0x0d => '␍',
            0x09 => '⇥',
            n @ 0..=0x1f => char::from_u32(0x2400 + n).unwrap_or('�'),
            0x7f => '␡',
            0x80..=0x9f => '�',
            _ if crate::safe::is_invisible(c) => '�',
            _ => c,
        })
        .collect();
    if text.chars().count() > OSC_PREVIEW_CHARS {
        out.push('…');
    }
    out
}

/// A copy waiting for the person's click.
pub struct CopyOffer {
    pub sandbox: String,
    pub text: String,
    pub bytes: usize,
    pub until: Instant,
}

impl CopyOffer {
    pub fn new(sandbox: &str, text: String, now: Instant) -> CopyOffer {
        let bytes = text.len();
        CopyOffer { sandbox: sandbox.to_string(), text, bytes, until: now + std::time::Duration::from_secs(OSC_OFFER_SECS) }
    }

    /// "web-app wants to copy 6.0 KB to your clipboard:"
    pub fn headline(&self) -> String {
        let who = if self.sandbox.is_empty() { "A sandbox" } else { &self.sandbox };
        format!("{who} wants to copy {} to your clipboard:", size_label(self.bytes))
    }
}

/// The card (key `copy-offer`; buttons `copy-offer-copy`,
/// `copy-offer-dismiss`), drawn by the core above every screen.
pub fn copy_offer_view<S: 'static>(o: &CopyOffer, copy: impl Fn(&mut S, &mut Tx) + 'static, dismiss: impl Fn(&mut S) + 'static) -> El {
    use crate::theme::{c, radius};
    let button = |key: &str, label: &str, primary: bool| {
        rune::el()
            .key(key)
            .role("button")
            .label(label)
            .px(crate::theme::spacing::MD)
            .py(crate::theme::spacing::XS)
            .min_h(crate::theme::layout::CONTROL_H)
            .items_center()
            .radius(radius("sm").unwrap())
            .fill(c(if primary { "primary" } else { "elevated" }))
            .border(1.0, c(if primary { "primary" } else { "border" }))
            .hover(c(if primary { "primary-hover" } else { "border" }))
            .cursor(Cursor::Pointer)
            .child(text(label.to_string()).font(crate::screen::body(crate::theme::font_size::BODY, 500.0)).color(c(if primary { "on-primary" } else { "text-bright" })))
    };
    col()
        .key("copy-offer")
        .role("alertdialog")
        .label("Clipboard copy from a sandbox")
        .pin(None, None, Some(16.0), Some(64.0))
        .w(460.0)
        .padding(crate::theme::spacing::MD)
        .gap(crate::theme::spacing::SM)
        .radius(radius("md").unwrap())
        .fill(c("surface"))
        .border(1.0, c("amber"))
        .child(text(o.headline()).font(crate::screen::body(crate::theme::font_size::CAPTION, 500.0)).color(c("text-bright")).wrap_anywhere())
        .child(
            col()
                .w_full()
                .min_w(0.0)
                .padding(crate::theme::spacing::SM)
                .radius(radius("sm").unwrap())
                .fill(c("bg"))
                .child(text(osc_preview(&o.text)).font(crate::screen::body(crate::theme::font_size::CAPTION, 400.0)).color(c("text")).wrap_anywhere().w_full().min_w(0.0)),
        )
        .child(
            row()
                .w_full()
                .gap(crate::theme::spacing::SM)
                .child(rune::el().grow(1.0))
                .child(button("copy-offer-dismiss", "Dismiss", false).on_press(dismiss))
                .child(button("copy-offer-copy", "Copy", true).on_press_tx(copy)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc_copies_by_size() {
        assert_eq!(osc_tier(40), OscTier::Copy);
        assert_eq!(osc_tier(4096), OscTier::Copy);
        assert_eq!(osc_tier(4097), OscTier::Offer);
        assert_eq!(osc_tier(100 * 1024), OscTier::Offer);
        assert_eq!(osc_tier(100 * 1024 + 1), OscTier::Drop);
        assert_eq!(size_label(40), "40 B");
        assert_eq!(size_label(6144), "6.0 KB");
    }

    #[test]
    fn preview_shows_what_would_be_pasted() {
        assert_eq!(osc_preview("ls\n\trm\r"), "ls↵⇥rm␍");
        assert_eq!(osc_preview("a\x1bb\x7fc\u{9b}"), "a␛b␡c�");
        // Bidi overrides, zero-width space and joiner, isolates, BOM.
        assert_eq!(osc_preview("x\u{202e}y\u{200b}z\u{200d}\u{2066}\u{feff}"), "x�y�z���");
        assert_eq!(osc_preview("é ü 日本"), "é ü 日本");
        let long = "x".repeat(81);
        assert_eq!(osc_preview(&long), format!("{}…", "x".repeat(80)));
        assert_eq!(osc_preview(&"x".repeat(80)), "x".repeat(80));
    }

    #[test]
    fn preview_marks_every_invisible_character() {
        use crate::safe::{invisible_chars, invisible_neighbours};
        for c in invisible_chars() {
            assert_eq!(osc_preview(&format!("a{c}b")), "a�b", "{c:?}");
        }
        for s in ["\u{2764}\u{fe0f} ok", "\u{2764}\u{fe0e} ok", "a\u{fe0f}b", "a\u{fe0e}b"] {
            assert_eq!(osc_preview(s), s, "{s:?} kept");
        }
        for c in invisible_neighbours() {
            let s = format!("a{c}b");
            assert_eq!(osc_preview(&s), s, "{c:?} kept");
        }
    }
}
