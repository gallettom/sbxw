//! A screen described in Luau, drawn by the core with Rune elements,
//! reloaded when its file changes. A reload keeps the screen's state (it
//! lives in the VM); a file that fails to load or render keeps the last good
//! view and shows the error above it.
//!
//! The file is watched by a thread (no file I/O on the UI thread during
//! frames, as Rune asks): it stats the file every 150 ms and, on a change,
//! sends the new source and wakes the UI, whose `poll` loads it.

use crate::luau::{Limits, LuauHost, Node};
use crate::terminal::rgb;
use rune::{col, el, row, text, El, TextStyle};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

pub const PANEL_WIDTH: f32 = 320.0;

pub struct ScreenState {
    host: LuauHost,
    pub path: PathBuf,
    /// What the core hands the screen: the daemon's data, as JSON.
    data: serde_json::Value,
    pub last_good: Option<Node>,
    pub error: Option<String>,
    pub reloads: u32,
    sources: Option<mpsc::Receiver<Result<String, String>>>,
}

impl ScreenState {
    pub fn new(path: PathBuf, data: serde_json::Value) -> Self {
        let mut v = Self {
            host: LuauHost::new(Limits::default()).expect("Luau VM"),
            path,
            data,
            last_good: None,
            error: None,
            reloads: 0,
            sources: None,
        };
        let src = std::fs::read_to_string(&v.path).map_err(|e| e.to_string());
        v.load(src);
        v
    }

    /// Starts the watcher thread; `wake` asks the UI to call [`Self::poll`].
    pub fn watch(&mut self, wake: Arc<dyn Fn() + Send + Sync>) {
        let (tx, rx) = mpsc::channel();
        self.sources = Some(rx);
        let path = self.path.clone();
        std::thread::Builder::new()
            .name("screen-watch".into())
            .spawn(move || {
                let mtime = |p: &PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
                let mut seen: Option<SystemTime> = mtime(&path);
                loop {
                    std::thread::sleep(Duration::from_millis(150));
                    let m = mtime(&path);
                    if m != seen {
                        seen = m;
                        if tx.send(std::fs::read_to_string(&path).map_err(|e| e.to_string())).is_err() {
                            break;
                        }
                        wake();
                    }
                }
            })
            .expect("watcher thread");
    }

    /// Loads what the watcher sent. True when the view changed.
    pub fn poll(&mut self) -> bool {
        let mut latest = None;
        if let Some(rx) = &self.sources {
            while let Ok(src) = rx.try_recv() {
                latest = Some(src);
            }
        }
        match latest {
            Some(src) => {
                self.load(src);
                true
            }
            None => false,
        }
    }

    /// Switches to another file and loads it now (headless runs, which use a
    /// virtual clock, swap files at script marks instead of editing one).
    pub fn load_file(&mut self, path: PathBuf) {
        self.path = path;
        let src = std::fs::read_to_string(&self.path).map_err(|e| e.to_string());
        self.load(src);
    }

    fn load(&mut self, src: Result<String, String>) {
        let name = self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let loaded = src.map_err(|e| format!("{name}: {e}")).and_then(|src| self.host.load(&name, &src));
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

    pub fn set_data(&mut self, data: serde_json::Value) {
        self.data = data;
        self.redraw();
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

    pub fn act(&mut self, action: &str, arg: Option<&str>) {
        if let Err(e) = self.host.act(action, arg) {
            self.error = Some(e);
        }
        self.redraw();
    }
}

const BODY: TextStyle = TextStyle::new(14.0, 400.0);
const SMALL: TextStyle = TextStyle::new(11.0, 400.0);

/// The panel. `get` finds the screen in the app's state, for the buttons.
pub fn view<S: 'static>(s: &ScreenState, get: fn(&mut S) -> &mut ScreenState) -> El {
    let mut ids = 0usize;
    let mut panel = col().key("screen").w(PANEL_WIDTH).h_full().padding(12.0).gap(8.0).fill(rgb(0x11111b));
    if let Some(e) = &s.error {
        panel = panel.child(
            el().key("screen-error").w_full().padding(8.0).radius(6.0).fill(rgb(0x5c1f2b)).child(
                text(format!("screen error, showing the last good view:\n{e}")).font(TextStyle::new(12.0, 400.0)).color(rgb(0xffd7de)).wrap().w_full(),
            ),
        );
    }
    if let Some(n) = &s.last_good {
        panel = panel.child(node(n, &mut ids, get));
    }
    panel.child(text(format!("{} (loads: {})", s.path.display(), s.reloads)).font(SMALL).color(rgb(0x6c7086)).wrap())
}

fn node<S: 'static>(n: &Node, ids: &mut usize, get: fn(&mut S) -> &mut ScreenState) -> El {
    *ids += 1;
    match n {
        Node::Text { text: t, color, bold } => {
            text(t.clone()).font(if *bold { TextStyle::new(14.0, 700.0) } else { BODY }).color(rgb(color.unwrap_or(0xcdd6f4)))
        }
        Node::Row(k) => k.iter().fold(row().gap(8.0).items_center(), |r, c| r.child(node(c, ids, get))),
        Node::Column(k) => k.iter().fold(col().gap(4.0), |r, c| r.child(node(c, ids, get))),
        Node::List(k) => k.iter().enumerate().fold(col().w_full().border(1.0, rgb(0x313244)).radius(6.0), |l, (i, c)| {
            if i > 0 {
                // A 1 pt separator between items.
                l.child(el().w_full().h(1.0).fill(rgb(0x313244)))
            } else {
                l
            }
            .child(el().w_full().px(8.0).py(4.0).child(node(c, ids, get)))
        }),
        Node::Button { label, action, arg } => {
            let (action, arg) = (action.clone(), arg.clone());
            // Keys name the button by action and argument, so a script can
            // click "btn-select-docs".
            let key = format!("btn-{action}-{}", arg.as_deref().unwrap_or(""));
            el().key(key)
                .role("button")
                .label(label.clone())
                .px(8.0)
                .radius(4.0)
                .fill(rgb(0x45475a))
                .hover(rgb(0x585b70))
                .cursor(rune::Cursor::Pointer)
                .on_press(move |s: &mut S| get(s).act(&action, arg.as_deref()))
                .child(text(label.clone()).font(BODY).color(rgb(0xffffff)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const SCREEN: &str = r#"
return {
  init = function() return { n = 0 } end,
  render = function(state, data) return text{ "TITLE " .. state.n } end,
  actions = { bump = function(state) state.n = state.n + 1 end },
}"#;

    fn title(s: &ScreenState) -> String {
        match &s.last_good {
            Some(Node::Text { text, .. }) => text.clone(),
            other => format!("{other:?}"),
        }
    }

    /// The watcher thread sees a real edit, wakes the UI, and `poll` reloads
    /// it with the state kept; a broken edit keeps the last good view.
    #[test]
    fn the_watcher_reloads_a_real_edit_and_keeps_state() {
        let dir = std::env::temp_dir().join(format!("sbxw-screen-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.luau");
        std::fs::write(&path, SCREEN).unwrap();
        let mut s = ScreenState::new(path.clone(), serde_json::json!({}));
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = wakes.clone();
        s.watch(Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }));
        s.act("bump", None);
        assert_eq!(title(&s), "TITLE 1");

        let wait_wake = |n: usize| {
            for _ in 0..100 {
                if wakes.load(Ordering::SeqCst) >= n {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            panic!("no wake");
        };
        // Some file systems keep whole-second mtimes: wait past the second.
        std::thread::sleep(Duration::from_millis(1100));
        std::fs::write(&path, SCREEN.replace("TITLE", "EDITED")).unwrap();
        wait_wake(1);
        assert!(s.poll());
        assert_eq!(title(&s), "EDITED 1", "state kept across the reload");

        std::thread::sleep(Duration::from_millis(1100));
        std::fs::write(&path, "return {{{").unwrap();
        wait_wake(2);
        assert!(s.poll());
        assert_eq!(title(&s), "EDITED 1", "last good view kept");
        assert!(s.error.as_deref().unwrap_or("").contains("s.luau"), "error shown: {:?}", s.error);
        assert_eq!(s.reloads, 3);
        std::fs::remove_dir_all(&dir).ok();
    }
}
