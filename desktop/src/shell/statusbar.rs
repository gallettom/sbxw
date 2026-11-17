//! The status bar: one thin line across the bottom of the window, drawn by
//! the core in every layout, under the panes (their area shrinks by its
//! height, it never covers a terminal).
//!
//! Left: the bring-ups in progress (create's jobs, `feat::create::bar_jobs`);
//! a press opens the "New sandboxes" panel, as the top bar's "Creating"
//! does. A failure stays in red until that press (or until it is retried or
//! dismissed); a success says "created" for four seconds. Right, in this order: Help. Each entry runs an intent that exists
//! already, or opens a read-only popover. Its code stays here so the
//! window's structure can be rearranged around it.
//!
//! What fits ([`fit`]) follows the window's width: the bar never wraps and
//! never cuts a number. Under a width it drops, in this order, the
//! bring-up's step label, the word "CPU", the word "Help"; past that the
//! sandbox's name truncates (its tooltip has it whole).

use super::{bar, App};
use crate::feat::create::{self, model::BarJob, model::BarState};
use crate::intent::Intent;
use crate::screen;
use crate::theme::{self, c, layout};
use rune::{el, row, text, Cursor, El};
use std::time::{Duration, Instant};

/// The bar's height (12 pt text, 24 pt targets, a hairline above).
pub const H: f32 = layout::STATUS_BAR_H;
/// The bar's side padding, the gap between its right entries, and inside an entry.
pub const PAD: f32 = 8.0;
pub const GAP: f32 = 4.0;
pub const INNER: f32 = 8.0;
/// An entry: padding each side, its 12 pt icon, the gap to its text.
pub const ITEM_PX: f32 = 8.0;
pub const ICON: f32 = 12.0;
pub const ICON_GAP: f32 = 6.0;
/// The type: 12 pt; numbers in the monospace face are tabular already.
pub const TEXT: f32 = 12.0;
/// The bring-up's progress line: 56 by 3 pt, rounded.
pub const TRACK_W: f32 = 56.0;
pub const TRACK_H: f32 = 3.0;
/// The progress line moves to a new width over this long (never under
/// reduced motion, nor headless, where shots must not depend on timing).
pub const GLIDE: Duration = Duration::from_millis(180);

/// What a text of the bar measures (12 pt, `weight`).
pub fn measure(t: &str, weight: f32) -> f32 {
    bar::measure(t, TEXT, weight)
}

/// An entry's width: icon, then its text when it has one.
pub fn item_w(t: &str) -> f32 {
    ITEM_PX * 2.0 + ICON + if t.is_empty() { 0.0 } else { ICON_GAP + measure(t, 400.0) }
}

/// The bar's state between frames.
#[derive(Default)]
pub struct Status {
    /// The bring-ups shown, most recent first (failures already looked at left out).
    pub jobs: Vec<BarJob>,
    /// Failures looked at in the "New sandboxes" panel: sandbox, job start.
    seen: Vec<(String, Instant)>,
    /// The progress line's width now, and its glide: sandbox, from, to, start.
    pub frac: f32,
    glide: Option<(String, f32, f32, Instant)>,
}

/// The bring-ups the bar shows from create's list: failures looked at go.
pub fn shown_jobs(all: Vec<BarJob>, seen: &[(String, Instant)]) -> Vec<BarJob> {
    all.into_iter().filter(|j| !(matches!(j.state, BarState::Failed(_)) && seen.iter().any(|(n, t)| *n == j.sandbox && *t == j.since))).collect()
}

/// The "+N" after the shown bring-up, and its colour: red when one of the
/// others failed, amber when one waits for the person, else muted.
pub fn more(jobs: &[BarJob]) -> Option<(String, &'static str)> {
    let rest = jobs.get(1..).filter(|r| !r.is_empty())?;
    let tone = if rest.iter().any(|j| matches!(j.state, BarState::Failed(_))) {
        "red-text"
    } else if rest.iter().any(|j| j.state == BarState::Asking) {
        "amber"
    } else {
        "text-muted"
    };
    Some((format!("+{}", rest.len()), tone))
}

/// The words after the name: the step, or "created".
pub fn job_words(j: &BarJob) -> String {
    match j.state {
        BarState::Created => "created".into(),
        _ => j.step.clone().unwrap_or_else(|| "Starting".into()),
    }
}

/// The left entry's width: icon, name, the words when shown, the line, "+N".
pub fn jobs_w(jobs: &[BarJob], words: bool) -> f32 {
    let Some(j) = jobs.first() else { return 0.0 };
    let mut w = ITEM_PX * 2.0 + ICON + ICON_GAP + measure(&j.sandbox, 500.0);
    if words {
        w += INNER + measure(&job_words(j), 400.0);
    }
    if j.state != BarState::Created {
        w += INNER + TRACK_W;
    }
    if let Some((m, _)) = more(jobs) {
        w += INNER + measure(&m, 500.0);
    }
    w
}

/// What shows at a width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fit {
    pub step: bool,
    pub cpu_word: bool,
    pub help_word: bool,
}

/// What the bar shows `width` points wide: everything when it fits, else
/// the parts dropped in their order (the step, the word "CPU", the word
/// "Help") until it does.
pub fn fit(width: f32, jobs: &[BarJob]) -> Fit {
    let need = |f: Fit| PAD * 2.0 + jobs_w(jobs, f.step) + GAP + item_w(if f.help_word { "Help" } else { "" });
    let steps = [
        Fit { step: true, cpu_word: true, help_word: true },
        Fit { step: false, cpu_word: true, help_word: true },
        Fit { step: false, cpu_word: false, help_word: true },
        Fit { step: false, cpu_word: false, help_word: false },
    ];
    steps.into_iter().find(|f| need(*f) <= width + 0.5).unwrap_or(steps[3])
}

impl App {
    /// The person pressed the bring-ups entry: the failures it showed are
    /// looked at (the panel it opens has them, with Retry and Dismiss).
    fn statusbar_seen(&mut self) {
        for j in self.status.jobs.iter().filter(|j| matches!(j.state, BarState::Failed(_))) {
            self.status.seen.push((j.sandbox.clone(), j.since));
        }
    }

    /// Each poll: the bring-ups from create's list, failures looked at
    /// marked, the progress line's glide. Whether the bar changed, and when
    /// to look again (a "created" that expires, a glide's next frame).
    pub(super) fn statusbar_poll(&mut self, now: Instant) -> (bool, Option<Instant>) {
        let all = create::bar_jobs();
        // A failure dismissed or retried is forgotten.
        self.status.seen.retain(|(n, t)| all.iter().any(|j| j.sandbox == *n && j.since == *t));
        let jobs = shown_jobs(all, &self.status.seen);
        let mut wake = jobs.iter().filter(|j| j.state == BarState::Created).map(|j| j.since + create::CREATED_FOR).min();
        let st = &mut self.status;
        let mut changed = jobs != st.jobs;
        let target = jobs.first().map_or(0.0, |j| j.frac);
        let name = jobs.first().map(|j| j.sandbox.clone()).unwrap_or_default();
        let same = st.jobs.first().is_some_and(|j| j.sandbox == name);
        st.jobs = jobs;
        let still = screen::reduced_motion() || super::headless();
        match &st.glide {
            _ if !same || still => {
                st.glide = None;
                changed |= st.frac != target;
                st.frac = target;
            }
            Some((n, _, to, _)) if *n == name && *to == target => {}
            _ if (st.frac - target).abs() > 0.001 => st.glide = Some((name, st.frac, target, now)),
            _ => {}
        }
        if let Some((_, from, to, t0)) = st.glide.clone() {
            let k = (now.saturating_duration_since(t0).as_secs_f32() / GLIDE.as_secs_f32()).min(1.0);
            // Ease out: fast first, settling.
            let e = 1.0 - (1.0 - k) * (1.0 - k);
            st.frac = from + (to - from) * e;
            changed = true;
            if k >= 1.0 {
                st.glide = None;
            } else {
                wake = Some(wake.map_or(now + Duration::from_millis(16), |w| w.min(now + Duration::from_millis(16))));
            }
        }
        (changed, wake)
    }
}

/// The bar (key `status-bar`).
pub fn view(s: &App) -> El {
    let f = fit(s.window.0, &s.status.jobs);
    let right = row().key("status-right").gap(GAP).items_center().child(help_view(f.help_word));
    let mut left = row().key("status-left").grow(1.0).min_w(0.0).h_full().items_center();
    if let Some(j) = jobs_view(s, f) {
        left = left.child(j);
    }
    row()
        .key("status-bar")
        .role("status")
        .w_full()
        .h(H)
        .px(PAD)
        .gap(GAP)
        .items_center()
        .fill(c("status-bar"))
        .child(el().pin(Some(0.0), Some(0.0), Some(0.0), None).h(1.0).fill(c("status-line")))
        .child(left)
        .child(right)
}

/// The shown bring-up (key `status-jobs`): its mark, name, step, progress
/// line and "+N"; a press opens the "New sandboxes" panel.
fn jobs_view(s: &App, f: Fit) -> Option<El> {
    let jobs = &s.status.jobs;
    let j = jobs.first()?;
    let (icon, tone) = match &j.state {
        BarState::Running => ("reload", "blue"),
        BarState::Asking => ("warning", "amber"),
        BarState::Failed(_) => ("close", "red"),
        BarState::Created => ("check", "green"),
    };
    let words = job_words(j);
    let words_c = match j.state {
        BarState::Failed(_) => "red-text",
        BarState::Created => "green",
        BarState::Asking => "amber",
        BarState::Running => "text-muted",
    };
    let pct = (j.frac * 100.0).floor() as u32;
    let mut help = match &j.state {
        BarState::Failed(e) => format!("{}: not created. {e}", j.sandbox),
        BarState::Created => format!("{}: created", j.sandbox),
        BarState::Asking => format!("{}: waiting for your answer about host access", j.sandbox),
        BarState::Running => format!("{}: {words} ({pct}% of the plan)", j.sandbox),
    };
    for o in &jobs[1..] {
        help.push_str(&format!("\n{}: {}", o.sandbox, job_words(o)));
    }
    help.push_str("\nClick to see the new sandboxes");
    let mut e = row()
        .key("status-jobs")
        .role("button")
        .label(format!("{} {words}", j.sandbox))
        .help(help)
        .min_w(0.0)
        .h(layout::DENSE_H)
        .px(ITEM_PX)
        .gap(INNER)
        .items_center()
        .radius(theme::radius("xs").unwrap())
        .hover(c("hover-subtle"))
        .focus_ring(2.0, -2.0, c("focus"))
        .cursor(Cursor::Pointer)
        .on_press(|s: &mut App| {
            s.statusbar_seen();
            s.run_intent("statusbar", Intent::OpenPanel { screen: create::JOBS.into() });
            s.push_data();
        })
        .child(
            row()
                .gap(ICON_GAP)
                .items_center()
                .min_w(0.0)
                .child(rune::icon(crate::icons::get(icon).unwrap(), ICON).color(c(tone)))
                .child(text(j.sandbox.clone()).font(screen::body(TEXT, 500.0)).color(c("text-bright")).truncate().min_w(0.0)),
        );
    if f.step {
        e = e.child(text(words).key("status-jobs-step").font(screen::body(TEXT, 400.0)).color(c(words_c)));
    }
    if j.state != BarState::Created {
        let fill = match j.state {
            BarState::Failed(_) => "red",
            BarState::Asking => "amber",
            _ => "blue",
        };
        e = e.child(
            el().key("status-jobs-track")
                .role("progressbar")
                .label(format!("{pct}%"))
                .w(TRACK_W)
                .h(TRACK_H)
                .radius(TRACK_H / 2.0)
                .fill(c("border"))
                .child(el().key("status-jobs-fill").w(TRACK_W * s.status.frac.clamp(0.0, 1.0)).h_full().radius(TRACK_H / 2.0).fill(c(fill))),
        );
    }
    if let Some((m, tone)) = more(jobs) {
        e = e.child(text(m).key("status-jobs-more").font(screen::body(TEXT, 500.0)).color(c(tone)));
    }
    Some(e)
}

/// One pressable entry: its icon (muted for neutral text), then its text,
/// hovered on the quiet tint.
fn entry(key: &str, icon: &str, label: &str, color: &str, help: &str) -> El {
    let mut e = row()
        .key(key.to_string())
        .role("button")
        .label(if label.is_empty() { help.to_string() } else { label.to_string() })
        .help(help.to_string())
        .h(layout::DENSE_H)
        .px(ITEM_PX)
        .gap(ICON_GAP)
        .items_center()
        .radius(theme::radius("xs").unwrap())
        .hover(c("hover-subtle"))
        .focus_ring(2.0, -2.0, c("focus"))
        .cursor(Cursor::Pointer)
        .child(rune::icon(crate::icons::get(icon).unwrap(), ICON).color(c(if color == "text" { "text-muted" } else { color })));
    if !label.is_empty() {
        e = e.child(text(label.to_string()).font(screen::body(TEXT, 400.0)).color(c(color)));
    }
    e
}

/// Help at the far right: the existing `show_help` (the macOS Help menu
/// and Ctrl+Alt+H open the same dialog).
fn help_view(word: bool) -> El {
    let help = if cfg!(target_os = "macos") { "Help: shortcuts and how sbxw works (⌃⌥H)" } else { "Help: shortcuts and how sbxw works (Ctrl+Alt+H)" };
    entry("status-help", "help", if word { "Help" } else { "" }, "text", help).on_press(|s: &mut App| s.show_help())
}

#[cfg(test)]
mod tests;
