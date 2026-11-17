//! sbxw desktop: the native client of the sbxw daemon.
//!
//! `sbxw` (this desktop app; `cargo run --release` in desktop/) with no arguments talks to the daemon at 127.0.0.1:7681
//! with the token in `~/.sbxw/run/ui-token`, and retries in the background
//! while there is none. Options (after Rune's own):
//!
//! * `--daemon ADDR` (or `$SBXW_DAEMON`), `--token T`, `--token-file P`
//! * `--fixture FILE` or `--fixture <screen>/<name>`: no daemon, the
//!   fixture's data (`desktop/fixtures/`)
//! * `--sandbox NAME [--mode agent|bash]`: open a terminal at start (bash by default)
//! * `--solo SCREEN`: draw only that screen, filling the window
//! * `--screens-dir DIR` (or `$SBXW_DESKTOP_SCREENS`): the override folder
//! * `--dump-text FILE`: the focused terminal's text, written when it changes
//!
//! Rune's own flags work in every build (the `tooling` feature): `--snap
//! out.png`, `--tree tree.json`, `--script s.json`, `--size WxH`, `--at T`,
//! `--chrome`. `.sbxw-artifacts/desktop/testing.md` (repository root, outside git) has the whole harness.

use rune::{AppSpec, Cx, El, KeyEvent, Tx, Waker};
use sbxw_desktop::shell::{self, App, Args};
use sbxw_desktop::terminal;

fn init() -> App {
    App::new(Args::parse(rune::extra_args()))
}

fn setup(s: &mut App, waker: Waker) {
    s.setup(waker);
}

fn poll(s: &mut App, clock: f64, tx: &mut Tx) -> bool {
    s.poll(clock, tx)
}

fn on_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    s.on_key(ev, tx)
}

fn on_ime(s: &mut App, ime: &rune::Ime, tx: &mut Tx) -> bool {
    s.on_ime(ime, tx.focused())
}

// The status bar asks for stats only while the window shows.
fn on_visibility(s: &mut App, shown: bool, _tx: &mut Tx) {
    s.statusbar_visible(shown);
}

fn view(s: &App, cx: &mut Cx<App>) -> El {
    shell::view(s, cx)
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,wgpu=warn,naga=warn")).init();
    let spec = AppSpec::<App> {
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        min_size: Some((720.0, 480.0)),
        // The web UI's look: monospace everywhere (`--font-mono` on `body`).
        families: (terminal::MONO_FAMILIES, terminal::MONO_FAMILIES),
        ..AppSpec::new("sbxw", (1280.0, 800.0), init, view)
    };
    // `on_ime`: dead keys, CJK and emoji composed by the input method reach the focused terminal.
    // The window buttons on the top bar's line (shell::window_chrome).
    // Help in the menu bar's own Help menu (macOS; Rune reports native
    // menus unsupported elsewhere, where the shortcut and the palette have it).
    let help = rune::MenuItem::new("Help and shortcuts", |s: &mut App, _tx: &mut Tx| s.show_help()).id("sbxw-help").menu("Help");
    // The app's icon while it runs (packaging/make-icon.py): the Dock's on
    // macOS, the title bar's and taskbar's on Windows; Rune sets nothing on
    // Linux, where a desktop entry names the icon, nor headless.
    if let Err(e) = rune::set_app_icon(include_bytes!("../packaging/icon-1024.png").to_vec()) {
        log::warn!("app icon: {e}");
    }
    rune::App::new(spec).chrome(shell::window_chrome()).menu_item(help).on_ime(on_ime).on_visibility(on_visibility).run();
}
