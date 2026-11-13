//! A screen described in Luau, drawn by the core, reloaded when its file
//! changes. A reload keeps the screen's state (it lives in the VM); a file
//! that fails to load or render keeps the last good view and shows the error
//! above it.

use crate::luau::{Limits, LuauHost, Node};
use gpui::*;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

pub const PANEL_WIDTH: f32 = 320.0;

pub struct ScreenView {
    host: LuauHost,
    path: PathBuf,
    mtime: Option<SystemTime>,
    /// What the core hands the screen: the daemon's data, as JSON.
    data: serde_json::Value,
    last_good: Option<Node>,
    error: Option<String>,
    pub reloads: u32,
    _watch: Task<()>,
}

impl ScreenView {
    pub fn new(path: PathBuf, data: serde_json::Value, cx: &mut Context<Self>) -> Self {
        let watch = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| loop {
            cx.background_executor().timer(Duration::from_millis(150)).await;
            if this.update(cx, |v, cx| v.poll(cx)).is_err() {
                break;
            }
        });
        let mut v = Self {
            host: LuauHost::new(Limits::default()).expect("Luau VM"),
            path,
            mtime: None,
            data,
            last_good: None,
            error: None,
            reloads: 0,
            _watch: watch,
        };
        v.reload();
        v
    }

    pub fn set_data(&mut self, data: serde_json::Value, cx: &mut Context<Self>) {
        self.data = data;
        self.redraw();
        cx.notify();
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        let m = std::fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        if m != self.mtime {
            self.reload();
            cx.notify();
        }
    }

    fn reload(&mut self) {
        self.mtime = std::fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        let name = self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let loaded = std::fs::read_to_string(&self.path).map_err(|e| format!("{name}: {e}")).and_then(|src| self.host.load(&name, &src));
        self.reloads += 1;
        match loaded {
            Ok(()) => {
                log::info!("screen {name} loaded (reload #{})", self.reloads);
                self.error = None;
                self.redraw();
            }
            Err(e) => {
                log::warn!("screen {name}: {e}");
                self.error = Some(e);
            }
        }
    }

    fn redraw(&mut self) {
        match self.host.render(&self.data) {
            Ok(n) => {
                self.last_good = Some(n);
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn element(&self, n: &Node, id: &mut usize, cx: &mut Context<Self>) -> AnyElement {
        *id += 1;
        match n {
            Node::Text { text, color, bold } => {
                let mut d = div().text_color(rgb(color.unwrap_or(0xcdd6f4))).child(text.clone());
                if *bold {
                    d = d.font_weight(FontWeight::BOLD);
                }
                d.into_any_element()
            }
            Node::Row(k) => div().flex().flex_row().gap_2().items_center().children(k.iter().map(|c| self.element(c, id, cx))).into_any_element(),
            Node::Column(k) => div().flex().flex_col().gap_1().children(k.iter().map(|c| self.element(c, id, cx))).into_any_element(),
            Node::List(k) => div()
                .flex()
                .flex_col()
                .border_1()
                .border_color(rgb(0x313244))
                .rounded_md()
                .children(k.iter().map(|c| div().px_2().py_1().border_b_1().border_color(rgb(0x313244)).child(self.element(c, id, cx))))
                .into_any_element(),
            Node::Button { label, action, arg } => {
                let (action, arg) = (action.clone(), arg.clone());
                div()
                    .id(("btn", *id))
                    .px_2()
                    .rounded_sm()
                    .bg(rgb(0x45475a))
                    .hover(|s| s.bg(rgb(0x585b70)))
                    .text_color(rgb(0xffffff))
                    .child(label.clone())
                    .on_click(cx.listener(move |v, _, _, cx| {
                        if let Err(e) = v.host.act(&action, arg.as_deref()) {
                            v.error = Some(e);
                        }
                        v.redraw();
                        cx.notify();
                    }))
                    .into_any_element()
            }
        }
    }
}

impl Render for ScreenView {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut id = 0;
        let body = self.last_good.as_ref().map(|n| self.element(n, &mut id, cx));
        div()
            .flex()
            .flex_col()
            .gap_2()
            .w(px(PANEL_WIDTH))
            .h_full()
            .p_3()
            .bg(rgb(0x11111b))
            .text_size(px(14.))
            .children(self.error.as_ref().map(|e| {
                div().p_2().rounded_md().bg(rgb(0x5c1f2b)).text_color(rgb(0xffd7de)).text_size(px(12.)).child(format!("screen error, showing the last good view:\n{e}"))
            }))
            .children(body)
            .child(div().text_color(rgb(0x6c7086)).text_size(px(11.)).child(format!("{} (loads: {})", self.path.display(), self.reloads)))
    }
}
