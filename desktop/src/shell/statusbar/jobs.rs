//! The bring-ups' popover over the status bar (key `status-jobs-pop`):
//! what the "New sandboxes" panel used to list, opened by a press on the
//! bar's bring-up entry. Each sandbox being created: its name and how it
//! stands, the plan's steps (the running one with what sbx prints under
//! it), the failure and its reason, the warnings, and the two actions that
//! existed (Retry, Dismiss: create's own intents). Nothing else is
//! pressable. Escape and a press outside close it, and it closes by itself
//! when nothing is left to show. Bounded: a long list scrolls inside.

use super::{App, Pop, POP_W, TEXT};
use crate::theme::Lift;
use crate::feat::{create, Asked};
use crate::intent::Intent;
use crate::screen;
use crate::theme::{self, c};
use rune::{el, row, text, Cursor, El};
use serde_json::Value as J;

/// The sandboxes being created (the popover's lines): create's list.
pub fn listed() -> Vec<J> {
    create::jobs_view()
}

impl App {
    /// Opens the popover when there is something to list (the bar's press,
    /// the palette, the old `open_panel create-jobs`); else says so.
    pub(in crate::shell) fn statusbar_open_jobs(&mut self) {
        if listed().is_empty() {
            self.status.pop = None;
            return self.notify("Nothing is being created.", false);
        }
        self.status.pop = Some(Pop::Jobs);
        self.bar_menu = None;
        for s in &mut self.screens {
            s.close_menus();
        }
    }

    fn statusbar_job_intent(&mut self, name: &'static str, sandbox: &str) {
        let args = serde_json::json!({ "sandbox": sandbox }).as_object().cloned().unwrap_or_default();
        self.run_intent("statusbar", Intent::Feature(Asked { name, args }));
        if listed().is_empty() {
            self.status.pop = None;
        }
        self.push_data();
    }
}

fn tone_of(state: &str) -> (&'static str, &'static str) {
    match state {
        "failed" => ("close", "red"),
        "asking" => ("warning", "amber"),
        "ready" => ("check", "green"),
        _ => ("reload", "blue"),
    }
}

fn small(t: &str, color: &str) -> El {
    text(t.to_string()).font(screen::body(TEXT, 400.0)).color(c(color))
}

/// A small action button (Retry, Dismiss): its key, label, colour by meaning.
fn action(key: String, label: &str, icon: Option<&str>, tone: &str, press: impl Fn(&mut App) + 'static) -> El {
    let (fg, tint) = screen::action_tone(tone);
    let (fg, tint) = (theme::hex(fg), theme::hex(tint));
    let mut b = row()
        .key(key)
        .role("button")
        .label(label.to_string())
        .h(24.0)
        .px(8.0)
        .gap(crate::theme::spacing::INLINE)
        .items_center()
        .radius(theme::radius("xs").unwrap())
        .hover(tint)
        .focus_ring(crate::theme::focus::W, crate::theme::focus::OUT, c("focus"))
        .cursor(Cursor::Pointer)
        .on_press(press);
    if let Some(i) = icon {
        b = b.child(rune::icon(crate::icons::get(i).unwrap(), theme::icon::SMALL).color(fg));
    }
    b.child(text(label.to_string()).font(screen::body(TEXT, 500.0)).color(fg))
}

/// One sandbox being created (key `job-<name>`).
fn card(j: &J) -> El {
    let name = j["sandbox"].as_str().unwrap_or("").to_string();
    let state = j["state"].as_str().unwrap_or("running");
    let (icon, tone) = tone_of(state);
    let head_c = match state {
        "failed" => "red-text",
        "asking" => "amber",
        _ => "text-muted",
    };
    let mut col = rune::col()
        .key(format!("job-{name}"))
        .w_full()
        .gap(4.0)
        .padding(8.0)
        .radius(theme::radius("sm").unwrap())
        .border(1.0, c(if state == "failed" { "red-border" } else { "border" }))
        .child(
            row()
                .w_full()
                .gap(crate::theme::spacing::INLINE)
                .items_center()
                .child(rune::icon(crate::icons::get(icon).unwrap(), theme::icon::SMALL).color(c(tone)))
                .child(text(name.clone()).font(screen::body(TEXT, 600.0)).color(c("text-bright")).truncate().min_w(0.0).grow(1.0))
                .child(small(j["headline"].as_str().unwrap_or(""), head_c)),
        );
    let steps = j["steps"].as_array().cloned().unwrap_or_default();
    if state == "running" && steps.is_empty() {
        col = col.child(small("Waiting for the daemon to say what it will do...", "text-muted").wrap());
    }
    for (i, s) in steps.iter().enumerate() {
        let st = s["state"].as_str().unwrap_or("waiting");
        let (mark, mc) = match st {
            "done" => ("check", "green"),
            "running" => ("reload", "blue"),
            "failed" => ("close", "red"),
            _ => ("chevron-right", "text-muted"),
        };
        col = col.child(
            row()
                .key(format!("job-{name}-step-{}", i + 1))
                .w_full()
                .gap(crate::theme::spacing::INLINE)
                .items_center()
                .child(rune::icon(crate::icons::get(mark).unwrap(), theme::icon::SMALL).color(c(mc)))
                .child(small(s["label"].as_str().unwrap_or(""), if st == "waiting" { "text-muted" } else { "text" }).truncate().min_w(0.0).grow(1.0)),
        );
        if let Some(d) = s["detail"].as_str() {
            // What sbx prints under the running step, its last line.
            col = col.child(text(d.to_string()).font(screen::body(crate::theme::font_size::CAPTION, 400.0).mono()).color(c("text-muted")).truncate().min_w(0.0).pl(crate::theme::icon::SMALL + crate::theme::spacing::INLINE)
.help(d.to_string()));
        }
    }
    if let Some(e) = j["error"].as_str() {
        col = col.child(small(e, if state == "asking" { "amber" } else { "red-text" }).wrap().key(format!("job-{name}-error")));
    }
    for w in j["warnings"].as_array().into_iter().flatten().filter_map(J::as_str) {
        col = col.child(small(w, "amber").wrap());
    }
    let mut buttons = row().w_full().gap(4.0).items_center().child(el().grow(1.0));
    let mut any = false;
    if j["can_retry"] == true {
        let n = name.clone();
        buttons = buttons.child(action(format!("job-retry-{name}"), "Retry", Some("reload"), "blue", move |s: &mut App| s.statusbar_job_intent("create_retry", &n)));
        any = true;
    }
    if state != "running" {
        let n = name.clone();
        buttons = buttons.child(action(format!("job-dismiss-{name}"), "Dismiss", None, "neutral", move |s: &mut App| s.statusbar_job_intent("create_dismiss", &n)));
        any = true;
    }
    if any {
        col = col.child(buttons);
    }
    col
}

/// The popover's surface, anchored above the bring-up entry, its list
/// bounded by the window's height and scrolled inside.
pub fn popover(s: &App) -> El {
    let jobs = listed();
    let max_h = (s.window.1 - 120.0).clamp(120.0, 520.0);
    let mut list = rune::col().w_full().gap(8.0);
    for j in &jobs {
        list = list.child(card(j));
    }
    let failed = jobs.iter().filter(|j| j["state"] == "failed").count();
    let title = match (jobs.len() - failed, failed) {
        (0, 1) => "1 sandbox not created".to_string(),
        (0, n) => format!("{n} sandboxes not created"),
        (1, 0) => "1 sandbox being created".to_string(),
        (n, 0) => format!("{n} sandboxes being created"),
        (n, f) => format!("{n} being created, {f} not created"),
    };
    let mut surface = rune::col()
        .key("status-jobs-pop")
        .role("dialog")
        .label("Sandboxes being created")
        .anchored("status-jobs".to_string(), rune::Side::Top, rune::Align::Start, 6.0)
        .w(POP_W.min(s.window.0 - 16.0))
        .padding(12.0)
        .gap(crate::theme::spacing::INLINE)
        .fill(c("surface"))
        .border(1.0, c("border"))
        .radius(theme::radius("md").unwrap())
        .lift(crate::theme::elevation::OVERLAY)
        .child(text(title).key("status-jobs-pop-title").font(screen::body(crate::theme::font_size::BODY, 600.0)).color(c("text-bright")))
        .child(rune::col().key("status-jobs-pop-list").w_full().max_h(max_h).scroll(rune::ScrollSpec { stick_bottom: false }).child(list));
    if !screen::reduced_motion() {
        surface = surface.transition(screen::quick_fade());
    }
    surface
}
