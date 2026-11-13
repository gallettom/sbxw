//! sbxw desktop (spike): a window with a Luau screen on the left and a
//! terminal on the right, both fed by the daemon through `client::Daemon`.
//!
//! sbxw-desktop --daemon 127.0.0.1:7399 --token test-token [--sandbox web-app] [--screen screens/sandboxes.luau]
//! (--token-file <path> reads the daemon's `run/ui-token` instead.)
//!
//! Rune's own flags work too, in every build (the `tooling` feature):
//! `--snap out.png`, `--tree tree.json`, `--script s.json`, `--size WxH`, `--at T`.
//! In a headless run, a script mark named `screen:<path>` loads that file as
//! the screen at that time (the virtual clock cannot wait for a real edit).

use rune::{col, el, grid, row, text, AppSpec, Cx, El, KeyEvent, TextStyle, Tx, Waker};
use sbxw_desktop::client::{fixture_sandboxes, Daemon, HttpDaemon, TerminalRequest};
use sbxw_desktop::screen::{self, ScreenState};
use sbxw_desktop::terminal::{self, rgb, TerminalState, LINE_HEIGHT, PAD, STATUS_H};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

struct Args {
    daemon: String,
    token: Option<String>,
    sandbox: String,
    screen: PathBuf,
    dump_text: Option<PathBuf>,
}

fn args() -> Args {
    let mut a = Args {
        daemon: "127.0.0.1:7399".into(),
        token: None,
        sandbox: "web-app".into(),
        screen: PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/screens/sandboxes.luau")),
        dump_text: None,
    };
    // What Rune did not take as its own flags.
    let mut it = rune::extra_args().iter();
    while let Some(k) = it.next() {
        let v = it.next().cloned().unwrap_or_default();
        match k.as_str() {
            "--daemon" => a.daemon = v,
            "--token" => a.token = Some(v),
            "--token-file" => a.token = std::fs::read_to_string(&v).ok().map(|s| s.trim().to_string()),
            "--sandbox" => a.sandbox = v,
            "--screen" => a.screen = v.into(),
            "--dump-text" => a.dump_text = Some(v.into()),
            _ => eprintln!("unknown argument {k}"),
        }
    }
    a
}

struct App {
    daemon: Result<Arc<dyn Daemon>, String>,
    sandbox: String,
    screen: ScreenState,
    term: Option<TerminalState>,
    status: String,
    /// Script marks `screen:<path>` not yet applied (headless only).
    marks: Vec<(f64, PathBuf)>,
    /// Where to write the terminal's text when it changes (`--dump-text`):
    /// Rune's `--tree` names the terminal element but not its lines.
    dump_text: Option<PathBuf>,
}

fn init() -> App {
    let a = args();
    let token = a.token.clone().unwrap_or_default();
    let daemon: Result<Arc<dyn Daemon>, String> =
        HttpDaemon::connect(&a.daemon, &token).map(|d| Arc::new(d) as Arc<dyn Daemon>).map_err(|e| format!("{e:#}"));
    let (sandboxes, source, status) = match &daemon {
        Ok(d) => match d.sandboxes() {
            Ok(s) => (s, d.describe(), String::new()),
            Err(e) => (fixture_sandboxes(), "fixture data".into(), format!("{e:#}")),
        },
        Err(e) => (fixture_sandboxes(), "fixture data".into(), e.clone()),
    };
    let data = serde_json::json!({ "sandboxes": sandboxes, "source": source, "terminal": a.sandbox });
    let marks = rune::tooling::headless_run()
        .map(|r| r.marks().filter_map(|(n, t)| n.strip_prefix("screen:").map(|p| (t, PathBuf::from(p)))).collect())
        .unwrap_or_default();
    App { daemon, sandbox: a.sandbox, screen: ScreenState::new(a.screen, data), term: None, status, marks, dump_text: a.dump_text }
}

fn setup(s: &mut App, waker: Waker) {
    s.screen.watch(waker.clone());
    let req = TerminalRequest { sandbox: s.sandbox.clone(), mode: Some("bash".into()), thread: None };
    match s.daemon.as_ref().map_err(String::clone).and_then(|d| d.open_terminal(&req, waker).map_err(|e| format!("{e:#}"))) {
        Ok(conn) => s.term = Some(TerminalState::new(conn)),
        Err(e) => s.status = format!("no terminal: {e}"),
    }
}

fn headless() -> bool {
    rune::tooling::headless_run().is_some()
}

fn poll(s: &mut App, clock: f64, _tx: &mut Tx) -> bool {
    let mut changed = s.screen.poll();
    while let Some((t, _)) = s.marks.first() {
        if *t > clock {
            break;
        }
        let (_, path) = s.marks.remove(0);
        s.screen.load_file(path);
        changed = true;
    }
    if let Some(t) = &mut s.term {
        // Headless frames run on a virtual clock: wait for the shell instead.
        let term_changed = t.poll(headless().then_some(Duration::from_millis(1500)));
        if let (true, Some(p)) = (term_changed, &s.dump_text) {
            let _ = std::fs::write(p, format!("{}x{}\n{}\n", t.size.0, t.size.1, t.screen_text().trim_end()));
        }
        changed |= term_changed;
    }
    changed
}

fn on_key(s: &mut App, ev: &KeyEvent, _tx: &mut Tx) -> bool {
    s.term.as_mut().is_some_and(|t| t.on_key(ev))
}

fn terminal_view(s: &App) -> El {
    let Some(t) = &s.term else {
        return col().key("terminal").grow(1.0).h_full().padding(16.0).child(text(s.status.clone()).color(rgb(0xf38ba8)).wrap());
    };
    let (cols, rows) = t.size;
    let cw = terminal::cell_width();
    let mut g = grid(cols as u16, cw, LINE_HEIGHT, t.rows.clone()).key("grid").w(cols as f32 * cw).h(rows as f32 * LINE_HEIGHT);
    if let Some((c, r)) = t.cursor {
        g = g.grid_caret(c, r);
    }
    col()
        .key("terminal")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .padding(PAD)
        .fill(rgb(terminal::BG))
        .focusable(true)
        .on_key(|s: &mut App, ev: &KeyEvent, _tx: &mut Tx| on_key(s, ev, _tx))
        .on_layout(|s: &mut App, r: [f32; 4]| {
            if let Some(t) = &mut s.term {
                t.fit(r[2], r[3]);
            }
        })
        .child(
            el().key("term-status").h(STATUS_H).child(
                text(format!("terminal {cols}x{rows}  {}", t.status)).font(TextStyle::new(12.0, 400.0)).color(rgb(0x6c7086)),
            ),
        )
        .child(g)
}

fn view(s: &App, _cx: &mut Cx<App>) -> El {
    row()
        .w_full()
        .h_full()
        .fill(rgb(0x181825))
        .child(screen::view(&s.screen, |s: &mut App| &mut s.screen))
        .child(terminal_view(s))
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,wgpu=warn,naga=warn")).init();
    rune::run(AppSpec::<App> {
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        families: ("Inter", terminal::MONO_FAMILIES),
        ..AppSpec::new("sbxw desktop (spike)", (1280.0, 800.0), init, view)
    });
}
