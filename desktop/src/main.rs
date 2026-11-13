//! sbxw desktop (spike): a window with a Luau screen on the left and a
//! terminal on the right, both fed by the daemon through `client::Daemon`.
//!
//! sbxw-desktop --daemon 127.0.0.1:7399 --token test-token [--sandbox web-app] [--screen screens/sandboxes.luau]
//! (--token-file <path> reads the daemon's `run/ui-token` instead.)

use gpui::*;
use sbxw_desktop::client::{fixture_sandboxes, Daemon, HttpDaemon, TerminalRequest};
use sbxw_desktop::screen::ScreenView;
use sbxw_desktop::terminal::TerminalView;
use std::path::PathBuf;
use std::sync::Arc;

struct Args {
    daemon: String,
    token: Option<String>,
    sandbox: String,
    screen: PathBuf,
}

fn args() -> Args {
    let mut a = Args {
        daemon: "127.0.0.1:7399".into(),
        token: None,
        sandbox: "web-app".into(),
        screen: PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/screens/sandboxes.luau")),
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let v = it.next().unwrap_or_default();
        match k.as_str() {
            "--daemon" => a.daemon = v,
            "--token" => a.token = Some(v),
            "--token-file" => a.token = std::fs::read_to_string(&v).ok().map(|s| s.trim().to_string()),
            "--sandbox" => a.sandbox = v,
            "--screen" => a.screen = v.into(),
            _ => eprintln!("unknown argument {k}"),
        }
    }
    a
}

struct Shell {
    screen: Entity<ScreenView>,
    terminal: Option<Entity<TerminalView>>,
    status: String,
}

impl Render for Shell {
    fn render(&mut self, _w: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let right: AnyElement = match &self.terminal {
            Some(t) => t.clone().into_any_element(),
            None => div().p_4().text_color(rgb(0xf38ba8)).child(self.status.clone()).into_any_element(),
        };
        div()
            .flex()
            .flex_row()
            .size_full()
            .bg(rgb(0x181825))
            .text_color(rgb(0xcdd6f4))
            .font_family(".SystemUIFont")
            .child(self.screen.clone())
            .child(div().flex().flex_col().flex_grow().h_full().child(right))
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,naga=warn,blade_graphics=warn,cosmic_text=warn")).init();
    let a = args();
    let token = a.token.clone().unwrap_or_default();
    let daemon: Result<Arc<dyn Daemon>, String> =
        HttpDaemon::connect(&a.daemon, &token).map(|d| Arc::new(d) as Arc<dyn Daemon>).map_err(|e| format!("{e:#}"));

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some("sbxw desktop (spike)".into()), ..Default::default() }),
            ..Default::default()
        };
        let window = cx
            .open_window(opts, |window, cx| {
                let (sandboxes, source, status) = match &daemon {
                    Ok(d) => match d.sandboxes() {
                        Ok(s) => (s, d.describe(), String::new()),
                        Err(e) => (fixture_sandboxes(), "fixture data".into(), format!("{e:#}")),
                    },
                    Err(e) => (fixture_sandboxes(), "fixture data".into(), e.clone()),
                };
                let data = serde_json::json!({ "sandboxes": sandboxes, "source": source, "terminal": a.sandbox });
                let screen = cx.new(|cx| ScreenView::new(a.screen.clone(), data, cx));
                let req = TerminalRequest { sandbox: a.sandbox.clone(), mode: Some("bash".into()), thread: None };
                let (terminal, status) = match daemon.as_ref().map_err(String::clone).and_then(|d| d.open_terminal(&req).map_err(|e| format!("{e:#}"))) {
                    Ok(conn) => (Some(cx.new(|cx| TerminalView::new(conn, cx))), status),
                    Err(e) => (None, format!("no terminal: {e}")),
                };
                if let Some(t) = &terminal {
                    t.read(cx).focus_handle().clone().focus(window);
                }
                cx.new(|_| Shell { screen, terminal, status })
            })
            .expect("open window");
        let _ = window;
        cx.activate(true);
    });
}
