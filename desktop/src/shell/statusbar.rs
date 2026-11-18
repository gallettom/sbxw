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
use serde_json::Value as J;
use std::time::{Duration, Instant};

pub mod numbers;
use numbers::{Cost, Cpu};

/// The key the stats read answers under (`Link::fetch`), taken by the bar
/// before the features see it.
pub const STATS_KEY: &str = "statusbar-stats";
/// The daemon's stats, read while the window shows: on the `stats` stream
/// event (every 5 s), else asked this often, and after a refusal this often.
pub const STATS_EVERY: Duration = Duration::from_secs(5);
pub const STATS_RETRY: Duration = Duration::from_secs(30);
/// The numbers on the bar change at most this often (no flicker).
pub const SHOWN_EVERY: Duration = Duration::from_secs(2);

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
    /// The window is off screen (minimised, hidden, covered): no stats asked.
    pub hidden: bool,
    /// The last stats, when they came, why they could not be read.
    stats: Option<J>,
    stats_at: Option<Instant>,
    stats_error: Option<String>,
    /// When stats were last asked for.
    asked: Option<Instant>,
    app_cpu: numbers::AppCpu,
    /// The numbers drawn, and when they were last changed.
    pub cpu: Option<Cpu>,
    pub cost: Option<Cost>,
    shown_at: Option<Instant>,
    /// Since when the CPU has been over [`numbers::HOT_SHARE`] of every core.
    hot_since: Option<Instant>,
    pub hot: bool,
    /// The read-only popover open over the bar.
    pub pop: Option<Pop>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pop {
    Cpu,
    Cost,
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

/// The right entries' texts at a fit: CPU with or without its word, cost.
pub fn right_w(cpu: &Cpu, cost: &Cost, f: Fit) -> f32 {
    item_w(&numbers::cpu_text(cpu, f.cpu_word)) + GAP + item_w(&numbers::cost_text(cost)) + GAP + item_w(if f.help_word { "Help" } else { "" })
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
pub fn fit(width: f32, jobs: &[BarJob], cpu: &Cpu, cost: &Cost) -> Fit {
    let need = |f: Fit| PAD * 2.0 + jobs_w(jobs, f.step) + GAP + right_w(cpu, cost, f);
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

    /// Whether the bar draws `key` now (the scripts' `wait:` marks), from
    /// create's list as it stands (a wait does not poll the bar).
    pub(super) fn statusbar_drawn(&self, key: &str) -> bool {
        let jobs = shown_jobs(create::bar_jobs(), &self.status.seen);
        let first = jobs.first();
        match key {
            "status-bar" | "status-left" | "status-right" | "status-cpu" | "status-cost" | "status-help" => true,
            "status-jobs" => first.is_some(),
            "status-jobs-track" | "status-jobs-fill" => first.is_some_and(|j| j.state != BarState::Created),
            "status-jobs-more" => more(&jobs).is_some(),
            "status-jobs-step" => first.is_some() && {
                let (cpu, cost) = self.statusbar_numbers();
                fit(self.window.0, &jobs, &cpu, &cost).step
            },
            "status-pop-scrim" => self.status.pop.is_some(),
            "status-cpu-pop" => self.status.pop == Some(Pop::Cpu),
            "status-cost-pop" => self.status.pop == Some(Pop::Cost),
            _ => false,
        }
    }

    /// The window went off screen or came back (Rune's `on_visibility`):
    /// stats are asked only while it shows, and at once when it shows again.
    pub fn statusbar_visible(&mut self, shown: bool) {
        self.status.hidden = !shown;
        if shown {
            self.status.asked = None;
        }
    }

    /// Stats arrived (the stream's `stats` event or a read), or a read failed.
    pub(super) fn statusbar_stats(&mut self, r: Result<J, String>, now: Instant) {
        match r {
            Ok(v) if v.is_object() => {
                self.status.stats = Some(v);
                self.status.stats_at = Some(now);
                self.status.stats_error = None;
            }
            Ok(_) => self.status.stats_error = Some("an answer that is not stats".into()),
            Err(e) => {
                // A daemon that never had stats, or lost them.
                self.status.stats_error = Some(e);
                if self.status.stats_at.is_none_or(|t| now.saturating_duration_since(t) > STATS_RETRY) {
                    self.status.stats = None;
                }
            }
        }
        self.status.app_cpu.sample(now);
    }

    /// The numbers as they stand now: CPU from the stats and this app,
    /// cost from the sandbox list and the sessions.
    fn statusbar_numbers(&self) -> (Cpu, Cost) {
        let st = &self.status;
        // A fixture can fix this app's share (shots that do not depend on the machine).
        let app = st.stats.as_ref().and_then(|v| v["app_cpu_pct"].as_f64()).or(st.app_cpu.pct);
        let cpu = numbers::cpu(st.stats.as_ref(), st.stats_error.as_deref(), app);
        let cost = numbers::cost(&self.sandboxes, self.feats.data.get("sessions"));
        (cpu, cost)
    }

    /// Asks for stats when due; the numbers drawn follow at most every
    /// [`SHOWN_EVERY`]. Whether they changed, and when to look again.
    fn statusbar_poll_numbers(&mut self, now: Instant) -> (bool, Option<Instant>) {
        let mut wake = None;
        let fixture = matches!(self.target, crate::client::Target::Fixture(_));
        if fixture && self.status.stats.is_none() && self.status.asked.is_none() {
            self.status.asked = Some(now);
            let v = self.raw.get("stats").cloned();
            match v {
                Some(v) => self.statusbar_stats(Ok(v), now),
                None => self.statusbar_stats(Err("the fixture has no stats".into()), now),
            }
        } else if !fixture && !self.status.hidden {
            if let (Some(d), Some(l)) = (self.daemon.clone(), &self.link) {
                let every = if self.status.stats_error.is_some() { STATS_RETRY } else { STATS_EVERY };
                let fresh = self.status.stats_at.is_some_and(|t| now.saturating_duration_since(t) < every + Duration::from_secs(2));
                let due = self.status.asked.map_or(now, |t| t + every);
                if !fresh && due <= now {
                    l.fetch(d, STATS_KEY.into(), vec![(None, "/api/stats".into())]);
                    self.status.asked = Some(now);
                    wake = Some(now + every);
                } else {
                    wake = Some(if fresh { self.status.stats_at.unwrap() + every + Duration::from_secs(2) } else { due });
                }
            }
        }
        let (cpu, cost) = self.statusbar_numbers();
        let st = &mut self.status;
        // Over 90% of every core for 30 s: amber.
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        if numbers::hot(cpu.total, cores) {
            let t = *st.hot_since.get_or_insert(now);
            if !st.hot && now.saturating_duration_since(t) < numbers::HOT_FOR {
                wake = Some(wake.map_or(t + numbers::HOT_FOR, |w: Instant| w.min(t + numbers::HOT_FOR)));
            }
        } else {
            st.hot_since = None;
        }
        let hot = st.hot_since.is_some_and(|t| now.saturating_duration_since(t) >= numbers::HOT_FOR);
        if st.cpu.as_ref() == Some(&cpu) && st.cost.as_ref() == Some(&cost) && st.hot == hot {
            return (false, wake);
        }
        // Headless shots and scripts see the numbers as they arrive.
        let every = if super::headless() { Duration::ZERO } else { SHOWN_EVERY };
        let due = st.shown_at.map_or(now, |t| t + every);
        if due > now {
            return (false, Some(wake.map_or(due, |w: Instant| w.min(due))));
        }
        st.cpu = Some(cpu);
        st.cost = Some(cost);
        st.hot = hot;
        st.shown_at = Some(now);
        (true, wake)
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
        let (more, w2) = self.statusbar_poll_numbers(now);
        let wake = match (wake, w2) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        (changed || more, wake)
    }
}

/// The bar (key `status-bar`).
pub fn view(s: &App) -> El {
    let (cpu, cost) = match (&s.status.cpu, &s.status.cost) {
        (Some(a), Some(b)) => (a.clone(), b.clone()),
        _ => s.statusbar_numbers(),
    };
    let f = fit(s.window.0, &s.status.jobs, &cpu, &cost);
    let right = row()
        .key("status-right")
        .gap(GAP)
        .items_center()
        .child(cpu_view(&cpu, f.cpu_word, s.status.hot, s.status.pop == Some(Pop::Cpu)))
        .child(cost_view(&cost, s.status.pop == Some(Pop::Cost)))
        .child(help_view(f.help_word));
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
        .shrink()
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
                .shrink()
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

/// The CPU entry (key `status-cpu`): neutral, amber when hot; "–" when unknown.
fn cpu_view(cpu: &Cpu, word: bool, hot: bool, open: bool) -> El {
    let color = if hot { "amber" } else { "text" };
    let mut help = numbers::cpu_help(cpu);
    if hot {
        help.push_str(&format!("\nOver {:.0}% of every core for {} s.", numbers::HOT_SHARE * 100.0, numbers::HOT_FOR.as_secs()));
    }
    let e = entry("status-cpu", "cpu", &numbers::cpu_text(cpu, word), color, &help).on_press(|s: &mut App| s.statusbar_toggle(Pop::Cpu));
    if open { e.fill(c("hover-subtle")) } else { e }
}

/// The cost entry (key `status-cost`).
fn cost_view(cost: &Cost, open: bool) -> El {
    let e = entry("status-cost", "coin", &numbers::cost_text(cost), "text", &numbers::cost_help(cost)).on_press(|s: &mut App| s.statusbar_toggle(Pop::Cost));
    if open { e.fill(c("hover-subtle")) } else { e }
}

impl App {
    fn statusbar_toggle(&mut self, p: Pop) {
        self.status.pop = if self.status.pop == Some(p) { None } else { Some(p) };
        if self.status.pop.is_some() {
            self.bar_menu = None;
            for s in &mut self.screens {
                s.close_menus();
            }
        }
    }

    /// Escape: the popover closes first.
    pub(super) fn statusbar_escape(&mut self) -> bool {
        self.status.pop.take().is_some()
    }
}

pub const POP_W: f32 = 340.0;

/// The open popover and the scrim under it (light dismiss: a press outside
/// closes it and does nothing else). Read-only: lines of text.
pub fn popover(s: &App) -> Vec<El> {
    let Some(p) = s.status.pop else { return vec![] };
    let (cpu, cost) = match (&s.status.cpu, &s.status.cost) {
        (Some(a), Some(b)) => (a.clone(), b.clone()),
        _ => s.statusbar_numbers(),
    };
    let (key, anchor, body) = match p {
        Pop::Cpu => ("status-cpu-pop", "status-cpu", cpu_pop(&cpu, max_lines(s))),
        Pop::Cost => ("status-cost-pop", "status-cost", cost_pop(&cost, max_lines(s))),
    };
    let mut surface = rune::col()
        .key(key)
        .role("dialog")
        .label(if p == Pop::Cpu { "CPU" } else { "Cost" })
        .anchored(anchor.to_string(), rune::Side::Top, rune::Align::End, 6.0)
        .w(POP_W.min(s.window.0 - 16.0))
        .padding(12.0)
        .gap(2.0)
        .fill(c("surface"))
        .border(1.0, c("border"))
        .radius(theme::radius("md").unwrap())
        .shadow(8.0, 24.0, crate::theme::shadow());
    if !screen::reduced_motion() {
        surface = surface.transition(screen::quick_fade());
    }
    for b in body {
        surface = surface.child(b);
    }
    vec![screen::menu_scrim("status-pop-scrim".into(), |s: &mut App| s.status.pop = None), surface]
}

/// The popover's lines that fit above the bar (20 pt each, with room for
/// its heading and notes); what does not fit is counted in one line.
fn max_lines(s: &App) -> usize {
    (((s.window.1 - 300.0) / 20.0).max(4.0)) as usize
}

fn more_line(key: &str, n: usize, what: &str) -> El {
    line(key.into(), format!("{n} more {what}"), "text-muted", String::new(), String::new(), "text-muted", 0.0)
}

fn line(key: String, left: String, left_c: &str, mid: String, right: String, right_c: &str, indent: f32) -> El {
    row()
        .key(key)
        .w_full()
        .h(20.0)
        .gap(8.0)
        .items_center()
        .pl(indent)
        .child(text(left).font(screen::body(TEXT, if indent > 0.0 { 400.0 } else { 500.0 })).color(c(left_c)).truncate().min_w(0.0).grow(1.0))
        .child(text(mid).font(screen::body(TEXT, 400.0)).color(c("text-muted")))
        .child(text(right).font(screen::body(TEXT, 500.0)).color(c(right_c)))
}

fn heading(t: &str) -> El {
    text(t.to_string()).font(screen::body(13.0, 600.0)).color(c("text-bright")).pb(6.0)
}

fn note(t: String) -> El {
    text(t).font(screen::body(TEXT, 400.0)).color(c("text-muted")).wrap().pt(8.0)
}

/// One line per sandbox, the daemon and this app, sorted by CPU; unknown
/// reads "no recent report", never 0%.
fn cpu_pop(cpu: &Cpu, max: usize) -> Vec<El> {
    let mut out = vec![heading("CPU, percent of one core")];
    for (i, r) in cpu.rows.iter().enumerate() {
        if i + 1 >= max && i + 1 < cpu.rows.len() {
            out.push(more_line("status-cpu-more", cpu.rows.len() - i, "lines, lower"));
            break;
        }
        let (v, vc) = match r.cpu {
            Some(x) => (numbers::pct(x), "text-bright"),
            None if r.kind == "app" => ("not measured".to_string(), "text-muted"),
            None => ("no recent report".to_string(), "text-muted"),
        };
        out.push(line(format!("status-cpu-row-{i}"), r.name.clone(), if r.kind == "sandbox" { "text" } else { "text-muted" }, numbers::mem_text(r), v, vc, 0.0));
    }
    let total = match cpu.total {
        Some(t) => format!("Total {}{}", numbers::pct(t), if cpu.partial { ", at least (partial)" } else { "" }),
        None => "Total unknown".into(),
    };
    out.push(el().w_full().h(1.0).fill(c("elevated")).mt(6.0));
    out.push(note(total));
    if let Some(w) = &cpu.why {
        out.push(note(format!("{w}.")));
    } else if cpu.counted < cpu.sandboxes {
        out.push(note(format!("{} of {} sandboxes report; the others have no recent report (a sandbox reports while a Claude session draws its status line).", cpu.counted, cpu.sandboxes)));
    }
    out
}

/// Per sandbox, then its sessions with their model; what ended sessions
/// cost as one line; one plain sentence on what the numbers are.
fn cost_pop(cost: &Cost, max: usize) -> Vec<El> {
    let mut out = vec![heading("Cost of the sessions")];
    let mut used = 0;
    for (i, sb) in cost.sandboxes.iter().enumerate() {
        let lines = 1 + sb.sessions.len() + sb.ended.is_some() as usize;
        if used + lines > max && i > 0 {
            out.push(more_line("status-cost-more", cost.sandboxes.len() - i, "sandboxes, cheaper"));
            break;
        }
        used += lines;
        out.push(line(format!("status-cost-sb-{i}"), sb.name.clone(), "text-bright", String::new(), sb.cost.map_or_else(|| "–".into(), numbers::usd), "text-bright", 0.0));
        for (k, se) in sb.sessions.iter().enumerate() {
            out.push(line(
                format!("status-cost-sb-{i}-{k}"),
                se.name.clone(),
                "text",
                se.model.clone().unwrap_or_default(),
                se.cost.map_or_else(|| "no cost yet".into(), numbers::usd),
                if se.cost.is_some() { "text" } else { "text-muted" },
                14.0,
            ));
        }
        if let Some(e) = sb.ended {
            out.push(line(format!("status-cost-sb-{i}-ended"), "ended sessions".into(), "text-muted", String::new(), numbers::usd(e), "text-muted", 14.0));
        }
    }
    if cost.sandboxes.is_empty() {
        out.push(text("No session has reported a cost yet.").font(screen::body(TEXT, 400.0)).color(c("text-muted")));
    }
    out.push(el().w_full().h(1.0).fill(c("elevated")).mt(6.0));
    if let Some(t) = cost.total {
        out.push(note(format!("Total {}", numbers::usd(t))));
    }
    out.push(note("Claude Code's estimate for these sessions, not an invoice.".into()));
    if cost.unknown > 0 {
        out.push(note(format!("{} session{} no cost reported yet, left out of the total.", cost.unknown, if cost.unknown == 1 { " has" } else { "s have" })));
    }
    out
}

/// One pressable entry: its icon (muted for neutral text), then its text,
/// hovered on the quiet tint.
fn entry(key: &str, icon: &str, label: &str, color: &str, help: &str) -> El {
    let mut e = row()
        .key(key.to_string())
        .role("button")
        .label(if label.is_empty() { help.to_string() } else { label.to_string() })
        .help(help.to_string())
        // Its measured width, exactly: the fit is computed from it.
        .w(item_w(label))
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
    let help = if cfg!(target_os = "macos") { "Help and shortcuts (⌃⌥H)" } else { "Help and shortcuts (Ctrl+Alt+H)" };
    entry("status-help", "help", if word { "Help" } else { "" }, "text", help).on_press(|s: &mut App| s.show_help())
}

#[cfg(test)]
mod tests;
