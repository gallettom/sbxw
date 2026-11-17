//! The pager: a slim strip (28 pt) where the pane area begins, above the
//! panes. One tab per page, naming what is on it, the current one marked;
//! "+" for a new empty page; at the right the view switch (Tabs: one
//! terminal per page; Grid: the last count above one) and the "terminals per
//! page" menu (1, 2, 3, 4, 6, and Gather). Tabs that do not fit go into a
//! menu (`pager-more`), the current tab always shown.
//!
//! It sits on the pane area rather than in the top bar: it follows the
//! area's width (a docked panel narrows both), keeps the top bar's fit as it
//! was, and puts the page tabs where a dragged pane is dropped onto them.
//!
//! Keys: `pager`, `page-<k>` (from 1), `page-<k>-close`, `page-new`,
//! `pager-view-tabs`, `pager-view-grid`, `pager-per` (its list
//! `pager-per-list`, items `pager-per-<n>` and `pager-gather`), `pager-gather-now` (the Gather button beside it, while gathering would move something), `pager-more`
//! (`pager-more-list`, items `page-<k>`).

use super::pages::PER_CHOICES;
use super::*;

/// The strip's height.
pub const H: f32 = layout::PAGER_H;
const TAB_PX: f32 = 8.0;
const TAB_GAP: f32 = 6.0;
const TAB_TEXT: f32 = 12.0;
const TAB_DOT: f32 = 6.0;
const TAB_CLOSE: f32 = 16.0;
/// A tab's label is cut beyond this (its tooltip has it whole).
const TAB_LABEL_MAX: f32 = 160.0;
const GAP: f32 = 4.0;
const PLUS_W: f32 = 24.0;
const MORE_W: f32 = 44.0;

/// What a page's tab shows: its label, the tooltip, and a dot's tone when a
/// terminal on it wants the person (amber) or works (blue).
pub struct TabInfo {
    pub label: String,
    pub help: String,
    pub tone: Option<&'static str>,
    /// The group colour of the page's first terminal (the sidebar's and the
    /// pane bar's, `App::group_colors`), and those of the page's other groups.
    pub group: Option<u32>,
    pub others: Vec<u32>,
    /// Its one terminal shows a shell, not the agent (a prompt glyph).
    pub bash: bool,
}

/// A name cut to about `n` characters.
fn short(name: &str, n: usize) -> String {
    if name.chars().count() <= n {
        name.to_string()
    } else {
        format!("{}…", name.chars().take(n.saturating_sub(1)).collect::<String>())
    }
}

/// A tab's width (points) for its label.
pub fn tab_w(label: &str, dot: bool, bash: bool) -> f32 {
    TAB_PX * 2.0 + if dot { TAB_DOT + TAB_GAP } else { 0.0 } + if bash { TAB_PROMPT + TAB_GAP } else { 0.0 } + bar::measure(label, TAB_TEXT, 500.0).min(TAB_LABEL_MAX) + TAB_GAP + TAB_CLOSE
}

/// The prompt glyph of a tab whose terminal is a shell.
const TAB_PROMPT: f32 = 12.0;

/// Which tabs show inline in `avail` points: a run around `current`,
/// growing to the right, then to the left. `(first, end)`.
pub fn fit_tabs(widths: &[f32], current: usize, avail: f32) -> (usize, usize) {
    if widths.is_empty() {
        return (0, 0);
    }
    let cur = current.min(widths.len() - 1);
    let (mut a, mut b) = (cur, cur + 1);
    let mut used = widths[cur];
    loop {
        let mut grew = false;
        if b < widths.len() && used + GAP + widths[b] <= avail {
            used += GAP + widths[b];
            b += 1;
            grew = true;
        } else if a > 0 && used + GAP + widths[a - 1] <= avail {
            used += GAP + widths[a - 1];
            a -= 1;
            grew = true;
        }
        if !grew {
            return (a, b);
        }
    }
}

/// The right group's width: Tabs, Grid, the per-page menu.
fn controls_w(per: usize) -> f32 {
    let seg = |l: &str| bar::measure(l, TAB_TEXT, 500.0) + 16.0 + 14.0 + 4.0;
    seg("Tabs") + seg("Grid") + GAP + bar::measure(&per_label(per), TAB_TEXT, 500.0) + 16.0 + 12.0 + 4.0 + GAP
}

/// The leads view's mark at the pager's left.
const LEADS_LABEL: &str = "Team leads";

fn leads_w() -> f32 {
    16.0 + 14.0 + 6.0 + bar::measure(LEADS_LABEL, TAB_TEXT, 600.0) + GAP
}

/// The "Gather" button beside the per-page menu.
fn gather_w() -> f32 {
    bar::measure("Gather", TAB_TEXT, 500.0) + 16.0
}

fn per_label(per: usize) -> String {
    if per == 1 { "1 per page".into() } else { format!("{per} per page") }
}

impl App {
    /// Page `k`'s tab: the names of what is open on it, whole when they fit
    /// in a tab; else (or `compact`, when the pager is short of room) a
    /// count, "Page 2 · 3", the names in its tooltip. One name alone is
    /// shortened at most once.
    pub fn tab_info(&self, k: usize) -> TabInfo {
        self.tab_info_as(k, false)
    }

    pub fn tab_info_as(&self, k: usize, compact: bool) -> TabInfo {
        let Some(page) = self.pages.list.get(k) else { return TabInfo { label: String::new(), help: String::new(), tone: None, group: None, others: vec![], bash: false } };
        let panes: Vec<&Pane> = page.tile.order().into_iter().filter_map(|u| self.pane_by_uid(u)).collect();
        let open: Vec<&&Pane> = panes.iter().filter(|p| p.sandbox.is_some()).collect();
        let names: Vec<String> = open.iter().map(|p| pane_name(p)).collect();
        let joined = names.join(", ");
        let label = match names.len() {
            0 => "Empty".to_string(),
            1 => short(&names[0], if compact { 16 } else { 28 }),
            _ if !compact && bar::measure(&joined, TAB_TEXT, 500.0) <= TAB_LABEL_MAX => joined,
            n => format!("Page {} · {n}", k + 1),
        };
        let wants = open.iter().any(|p| p.attention);
        let working = open.iter().any(|p| terminal::session_for(self.feats.data.get("sessions"), p.sandbox.as_deref(), p.thread.as_deref(), p.mode == Mode::Agent).is_some_and(|s| s.state == "working"));
        let tone = if wants { Some("amber") } else if working { Some("blue") } else { None };
        let what = if names.is_empty() { "nothing open".to_string() } else { names.join(", ") };
        let state = if wants { " (a terminal wants you)" } else if working { " (an agent is working)" } else { "" };
        let colors = self.group_colors();
        let groups: Vec<Option<u32>> = open.iter().map(|p| p.sandbox.as_deref().and_then(|n| colors.get(n).copied())).collect();
        let group = groups.first().copied().flatten();
        let mut others: Vec<u32> = groups.iter().flatten().copied().filter(|g| Some(*g) != group).collect();
        others.dedup();
        let bash = open.len() == 1 && open[0].mode == Mode::Bash;
        let help = format!("Page {}: {what}{}{state}", k + 1, if bash { " (a shell)" } else { "" });
        TabInfo { label, help, tone, group, others, bash }
    }

    /// Whether "Gather" would move something: terminals on more pages than
    /// N per page needs, or empty panes among them (raising N by hand pulls
    /// nothing back by itself; the button beside the number says it can).
    pub fn gather_would_move(&self) -> bool {
        if self.pages.tabs() {
            return false;
        }
        let e = self.empty_uids();
        let mut p = self.pages.clone();
        p.gather(&|x| e.contains(&x));
        p.list.iter().map(|x| &x.tile).ne(self.pages.list.iter().map(|x| &x.tile))
    }

    /// Which tabs are inline: `(first, end)`; the others are in `pager-more`.
    pub fn pager_fit(&self) -> (usize, usize) {
        let (a, b, _) = self.pager_fit_as();
        (a, b)
    }

    /// [`App::pager_fit`], and whether the tabs show their compact labels
    /// (when the whole names do not all fit).
    pub fn pager_fit_as(&self) -> (usize, usize, bool) {
        let n = self.pages.list.len();
        let w = if self.pager_w > 0.0 { self.pager_w } else { self.window.0 - self.left_w() - layout::GUTTER * 2.0 };
        let fixed = PLUS_W + GAP + controls_w(self.pages.per) + GAP + if self.gather_would_move() { gather_w() + GAP } else { 0.0 } + if self.leads_on() { leads_w() } else { 0.0 };
        let widths = |compact: bool| -> Vec<f32> {
            (0..n).map(|k| {
                let t = self.tab_info_as(k, compact);
                tab_w(&t.label, t.tone.is_some(), t.bash)
            }).collect()
        };
        let total = |v: &[f32]| v.iter().sum::<f32>() + GAP * n.saturating_sub(1) as f32;
        let full = widths(false);
        if total(&full) + fixed <= w {
            return (0, n, false);
        }
        let short = widths(true);
        if total(&short) + fixed <= w {
            return (0, n, true);
        }
        let (a, b) = fit_tabs(&short, self.pages.current, (w - fixed - MORE_W - GAP).max(0.0));
        (a, b, true)
    }

    /// Whether the pager draws an element of that key (`wait:` marks).
    pub(super) fn pager_drawn(&self, key: &str) -> bool {
        let n = self.pages.list.len();
        let (a, b) = self.pager_fit();
        let fixed = matches!(key, "pager" | "page-new" | "pager-view-tabs" | "pager-view-grid" | "pager-per") || (key == "pager-gather-now" && self.gather_would_move()) || (key == "pager-leads" && self.leads_on());
        let more = (a > 0 || b < n) && key == "pager-more";
        let tab = key.strip_prefix("page-").and_then(|r| r.strip_suffix("-close").unwrap_or(r).parse::<usize>().ok()).is_some_and(|k| k >= 1 && k <= n && ((a..b).contains(&(k - 1)) || (!key.ends_with("-close") && self.pager_menu.as_deref() == Some("more"))));
        let per = self.pager_menu.as_deref() == Some("per") && (key == "pager-per-list" || key == "pager-gather" || PER_CHOICES.iter().any(|c| key == format!("pager-per-{c}")));
        let more_list = self.pager_menu.as_deref() == Some("more") && key == "pager-more-list";
        fixed || more || tab || per || more_list
    }

    /// The pager's open menu, if any (an overlay anchored to its button).
    pub(super) fn pager_menu_view(&self) -> Option<El> {
        match self.pager_menu.as_deref()? {
            "per" => {
                let labels: Vec<String> = PER_CHOICES.iter().map(|&n| if n == 1 { "1 per page (tabs)".to_string() } else { format!("{n} per page") }).collect();
                let gather = "Gather onto fewer pages".to_string();
                let mut rows: Vec<screen::MenuRow> = PER_CHOICES
                    .iter()
                    .zip(&labels)
                    .map(|(&n, l)| screen::MenuRow { key: format!("pager-per-{n}"), label: l, icon: Some(if n == 1 { "single" } else { "grid" }), tone: None, help: None, keys: &[], count: None, chosen: self.pages.per == n, disabled: false })
                    .collect();
                let gather_help = "Fill pages up to the count per page, in order: raising the count moves nothing by itself";
                rows.push(screen::MenuRow { key: "pager-gather".into(), label: &gather, icon: Some("columns"), tone: None, help: Some(gather_help), keys: &[], count: None, chosen: false, disabled: false });
                let mut list = screen::menu_surface("pager-per-list".into(), "pager-per".into(), rune::Align::End, screen::menu_width(&rows));
                for (i, r) in rows.iter().enumerate() {
                    let e = screen::menu_row(r);
                    let e = if i < PER_CHOICES.len() {
                        let n = PER_CHOICES[i];
                        e.on_press(move |s: &mut App| {
                            s.pager_menu = None;
                            s.run_intent("pager", Intent::Pages(PageOp::Per(n)));
                            s.push_data();
                        })
                    } else {
                        list = list.child(screen::menu_divider());
                        e.on_press(|s: &mut App| {
                            s.pager_menu = None;
                            s.run_intent("pager", Intent::Pages(PageOp::Gather));
                            s.push_data();
                        })
                    };
                    list = list.child(e);
                }
                Some(list)
            }
            "more" => {
                let (a, b) = self.pager_fit();
                let hidden: Vec<usize> = (0..self.pages.list.len()).filter(|k| !(a..b).contains(k)).collect();
                if hidden.is_empty() {
                    return None;
                }
                let infos: Vec<(usize, TabInfo)> = hidden.iter().map(|&k| (k, self.tab_info(k))).collect();
                let labels: Vec<String> = infos.iter().map(|(k, t)| format!("{} · {}", k + 1, t.label)).collect();
                let rows: Vec<screen::MenuRow> = infos
                    .iter()
                    .zip(&labels)
                    .map(|((k, t), l)| screen::MenuRow { key: format!("page-{}", k + 1), label: l, icon: Some("single"), tone: t.tone, help: Some(t.help.as_str()), keys: &[], count: None, chosen: false, disabled: false })
                    .collect();
                let mut list = screen::menu_surface("pager-more-list".into(), "pager-more".into(), rune::Align::Start, screen::menu_width(&rows));
                for ((k, _), r) in infos.iter().zip(&rows) {
                    let k = *k;
                    list = list.child(screen::menu_row(r).on_press(move |s: &mut App| {
                        s.pager_menu = None;
                        s.run_intent("pager", Intent::Pages(PageOp::Show(k)));
                        s.push_data();
                    }));
                }
                Some(list)
            }
            _ => None,
        }
    }
}

fn small_icon(name: &str, size: f32, color: rune::Color) -> El {
    rune::icon(crate::icons::get(name).unwrap(), size).color(color)
}

/// One page's tab.
fn tab(s: &App, k: usize, compact: bool) -> El {
    let t = s.tab_info_as(k, compact);
    let on = k == s.pages.current;
    // While a pane is dragged over it: the drop target.
    let target = s.drag.as_ref().and_then(|d| d.zone).is_some_and(|z| z == drag::Zone::Page { page: k, full: false } || z == drag::Zone::Page { page: k, full: true });
    let mut e = row()
        .key(format!("page-{}", k + 1))
        .role("tab")
        .label(t.help.clone())
        .help(t.help.clone())
        .checked(on)
        .h(H - 4.0)
        .pl(TAB_PX)
        .pr(4.0)
        .gap(TAB_GAP)
        .items_center()
        .radius(theme::radius("xs").unwrap())
        .cursor(Cursor::Pointer)
        .hover(c("elevated"))
        .on_layout(move |s: &mut App, r: [f32; 4]| {
            s.rects.tabs.insert(k, r);
        })
        .on_press(move |s: &mut App| {
            s.run_intent("pager", Intent::Pages(PageOp::Show(k)));
            s.push_data();
        });
    // Its group's colour as a tint (stronger for the current page); the
    // current one also has a 1 pt border and a 2 pt line at its foot in that
    // colour (the focus blue when it has none) and a heavier label, so it
    // reads without the colour too.
    let ground = theme::tab_ground(t.group, on);
    let line = theme::hex(t.group.unwrap_or_else(|| theme::color_value("focus").unwrap()));
    e = e.fill(theme::hex(ground)).hover(theme::hex(theme::over(theme::color_value("text").unwrap(), 0.08, ground)));
    if on {
        e = e.border(1.0, line).child(el().key(format!("page-{}-line", k + 1)).pin(Some(0.0), None, Some(0.0), Some(0.0)).h(2.0).fill(line));
    }
    if target {
        e = e.border(1.5, c("blue"));
    }
    if let Some(tone) = t.tone {
        // A dark ring keeps the dot clear on any tint.
        e = e.child(el().w(TAB_DOT).h(TAB_DOT).radius(TAB_DOT / 2.0).fill(c(tone)).border(1.0, c("floor")));
    }
    if t.bash {
        e = e.child(small_icon("prompt", TAB_PROMPT, theme::hex(theme::muted_on(ground))));
    }
    // A session's tab: the sandbox muted, what tells it apart (the thread)
    // bright; the cut falls on the second part.
    let (prefix, rest) = match t.label.split_once(" · ") {
        Some((a, b)) if !t.label.starts_with("Page ") => (Some(format!("{a} · ")), b.to_string()),
        _ => (None, t.label.clone()),
    };
    let weight = if on { 600.0 } else { 500.0 };
    if let Some(p) = prefix {
        e = e.child(text(p).font(small(TAB_TEXT, 500.0)).color(theme::hex(theme::muted_on(ground))));
    }
    e = e.child(text(rest).font(small(TAB_TEXT, weight)).color(c("text-bright")).truncate().min_w(0.0).max_w(TAB_LABEL_MAX));
    // Groups on the page besides the first one's: a small dot each.
    for g in t.others.iter().take(3) {
        e = e.child(el().w(5.0).h(5.0).radius(2.5).fill(theme::hex(*g)));
    }
    let help = if s.pages.list.len() > 1 { format!("Close page {} (its terminals' views close; the sessions go on)", k + 1) } else { "Close this page's terminals (the sessions go on)".to_string() };
    e.child(
        el().key(format!("page-{}-close", k + 1))
            .role("button")
            .label(format!("Close page {}", k + 1))
            .help(help)
            .w(TAB_CLOSE)
            .h(TAB_CLOSE)
            .items_center()
            .justify_center()
            .radius(theme::radius("xs").unwrap())
            .hover(c("border"))
            .cursor(Cursor::Pointer)
            .on_press(move |s: &mut App| {
                s.run_intent("pager", Intent::Pages(PageOp::Close(Some(k))));
                s.push_data();
            })
            .child(small_icon("close", 10.0, c("text-muted"))),
    )
}

/// A segment of the view switch.
fn segment(key: &str, label: &str, icon: &str, on: bool, help: &str, keys: Vec<String>, op: PageOp) -> El {
    row()
        .key(key.to_string())
        .role("radio")
        .label(label.to_string())
        .checked(on)
        .help(help.to_string())
        .help_keys(keys)
        .h(H - 6.0)
        .px(8.0)
        .gap(4.0)
        .items_center()
        .radius(theme::radius("xs").unwrap())
        .fill(c(if on { "elevated" } else { "surface-alt" }))
        .hover(c("elevated"))
        .cursor(Cursor::Pointer)
        .on_press(move |s: &mut App| {
            s.run_intent("pager", Intent::Pages(op.clone()));
            s.push_data();
        })
        .child(small_icon(icon, 14.0, c(if on { "blue" } else { "text-muted" })))
        .child(text(label.to_string()).font(small(TAB_TEXT, 500.0)).color(c(if on { "text-bright" } else { "text-muted" })))
}

/// The pager strip.
pub fn view(s: &App) -> El {
    let n = s.pages.list.len();
    let (a, b, compact) = s.pager_fit_as();
    let mut tabs = row().gap(GAP).items_center().min_w(0.0).clip_rounded(0.0);
    for k in a..b {
        tabs = tabs.child(tab(s, k, compact));
    }
    let mut left = row().gap(GAP).items_center().grow(1.0).min_w(0.0);
    // The team leads view says so before its tabs (leads.rs).
    if s.leads_on() {
        let (fg, tint) = crate::screen::action_tone("purple");
        left = left.child(
            row()
                .key("pager-leads")
                .role("button")
                .label("Team leads view: press for the layout before")
                .help("Only the team leaders' terminals. Press (or the rail's Team leads) for the layout before")
                .h(H - 6.0)
                .px(8.0)
                .gap(6.0)
                .items_center()
                .radius(theme::radius("xs").unwrap())
                .fill(theme::hex(tint))
                .hover(theme::hex(theme::over(fg, 0.12, tint)))
                .cursor(Cursor::Pointer)
                .on_press(|s: &mut App| {
                    s.toggle_leads();
                    s.push_data();
                })
                .child(small_icon("people", 14.0, theme::hex(fg)))
                .child(text(LEADS_LABEL).font(small(TAB_TEXT, 600.0)).color(theme::hex(fg))),
        );
    }
    if a > 0 || b < n {
        let hidden = n - (b - a);
        let open = s.pager_menu.as_deref() == Some("more");
        let hidden_tone = (0..n).filter(|k| !(a..b).contains(k)).filter_map(|k| s.tab_info(k).tone).min_by_key(|t| if *t == "amber" { 0 } else { 1 });
        let mut m = row()
            .key("pager-more")
            .role("button")
            .label(format!("{hidden} more pages"))
            .expanded(open)
            .help(format!("{hidden} more pages"))
            .h(H - 6.0)
            .px(6.0)
            .gap(4.0)
            .items_center()
            .radius(theme::radius("xs").unwrap())
            .fill(c(if open { "elevated" } else { "surface-alt" }))
            .hover(c("elevated"))
            .cursor(Cursor::Pointer)
            .on_press(|s: &mut App| s.pager_menu = if s.pager_menu.as_deref() == Some("more") { None } else { Some("more".into()) })
            .child(small_icon("more", 14.0, c("text")))
            .child(text(hidden.to_string()).font(small(TAB_TEXT, 500.0)).color(c("text")));
        if let Some(t) = hidden_tone {
            m = m.child(el().w(TAB_DOT).h(TAB_DOT).radius(TAB_DOT / 2.0).fill(c(t)));
        }
        if a > 0 {
            left = left.child(m).child(tabs);
        } else {
            left = left.child(tabs).child(m);
        }
    } else {
        left = left.child(tabs);
    }
    let plus_target = s.drag.as_ref().and_then(|d| d.zone) == Some(drag::Zone::NewPage);
    let mut plus = el()
        .key("page-new")
        .role("button")
        .label("New page")
        .help("A new empty page")
        .w(PLUS_W)
        .h(PLUS_W)
        .items_center()
        .justify_center()
        .radius(theme::radius("xs").unwrap())
        .hover(c("elevated"))
        .cursor(Cursor::Pointer)
        .on_layout(|s: &mut App, r: [f32; 4]| s.rects.plus = Some(r))
        .on_press(|s: &mut App| {
            s.run_intent("pager", Intent::Pages(PageOp::New));
            s.push_data();
        })
        .child(small_icon("plus", 14.0, c("text-muted")));
    if plus_target {
        plus = plus.border(1.5, c("blue"));
    }
    left = left.child(plus);
    let tabs_on = s.pages.tabs();
    let grid_n = s.pages.grid_per;
    let view_switch = row()
        .key("pager-view")
        .role("radiogroup")
        .label("View")
        .gap(2.0)
        .padding(1.0)
        .radius(theme::radius("xs").unwrap())
        .fill(c("surface-alt"))
        .child(segment("pager-view-tabs", "Tabs", "single", tabs_on, "Tabs: one terminal per page, the pager as its tab strip", palette::page_keys(true), PageOp::View(true)))
        .child(segment("pager-view-grid", "Grid", "grid", !tabs_on, &format!("Grid: up to {grid_n} terminals per page"), vec![], PageOp::View(false)));
    let per_open = s.pager_menu.as_deref() == Some("per");
    let per = row()
        .key("pager-per")
        .role("button")
        .label(format!("Terminals per page: {}", s.pages.per))
        .expanded(per_open)
        .help("How many terminals a page holds; lowering it moves the others onto the following pages")
        .h(H - 6.0)
        .px(8.0)
        .gap(4.0)
        .items_center()
        .radius(theme::radius("xs").unwrap())
        .fill(c(if per_open { "elevated" } else { "surface-alt" }))
        .hover(c("elevated"))
        .cursor(Cursor::Pointer)
        .on_press(|s: &mut App| s.pager_menu = if s.pager_menu.as_deref() == Some("per") { None } else { Some("per".into()) })
        .child(text(per_label(s.pages.per)).font(small(TAB_TEXT, 500.0)).color(c("text")))
        .child(small_icon("chevron-down", 12.0, c("text-muted")));
    let gather = s.gather_would_move().then(|| {
        row()
            .key("pager-gather-now")
            .role("button")
            .label("Gather")
            .help(format!("Gather the terminals onto pages of {}, in page order", s.pages.per))
            .h(H - 6.0)
            .px(8.0)
            .items_center()
            .radius(theme::radius("xs").unwrap())
            .fill(c("surface-alt"))
            .hover(c("elevated"))
            .cursor(Cursor::Pointer)
            .on_press(|s: &mut App| {
                s.run_intent("pager", Intent::Pages(PageOp::Gather));
                s.push_data();
            })
            .child(text("Gather").font(small(TAB_TEXT, 500.0)).color(c("blue")))
    });
    row()
        .key("pager")
        .role("tablist")
        .label("Pages of terminals")
        .h(H)
        .w_full()
        .gap(GAP)
        .items_center()
        .on_layout(|s: &mut App, r: [f32; 4]| {
            if (s.pager_w - r[2]).abs() > 0.5 {
                s.pager_w = r[2];
            }
        })
        .child(left)
        .child(view_switch)
        .children(gather)
        .child(per)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tabs_shown_always_hold_the_current_one() {
        let w = [100.0; 8];
        assert_eq!(fit_tabs(&w, 0, 1000.0), (0, 8));
        assert_eq!(fit_tabs(&w, 0, 310.0), (0, 3));
        assert_eq!(fit_tabs(&w, 7, 310.0), (5, 8));
        assert_eq!(fit_tabs(&w, 4, 310.0), (4, 7));
        assert_eq!(fit_tabs(&w, 3, 50.0), (3, 4), "the current tab even when nothing fits");
        assert_eq!(short("better-checkout", 12), "better-chec…");
    }
}
