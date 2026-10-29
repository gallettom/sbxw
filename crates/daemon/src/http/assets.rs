//! The web UI, compiled into the binary: the HTML shell (the one templated
//! file), its stylesheet and scripts, and the extension's side panel page.

use super::guard::EXTENSION_ID;
use crate::app::App;
use axum::extract::State;
use axum::http::header;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::Router;
use std::sync::Arc;

const INDEX: &str = include_str!("../../../../assets/index.html");

macro_rules! js {
    ($file:literal) => {
        (
            concat!("/js/", $file),
            "application/javascript; charset=utf-8",
            include_str!(concat!("../../../../assets/js/", $file)),
        )
    };
}

/// A closed set known at compile time: nothing resolved at runtime, no path to
/// traverse.
const STATIC: &[(&str, &str, &str)] = &[
    ("/app.css", "text/css; charset=utf-8", include_str!("../../../../assets/app.css")),
    js!("singleton.js"),
    js!("util.js"),
    js!("term-color.js"),
    js!("panes.js"),
    js!("pane-controls.js"),
    js!("sandboxes.js"),
    js!("create.js"),
    js!("ports.js"),
    js!("files.js"),
    js!("codemap.js"),
    js!("ssh.js"),
    js!("envfile.js"),
    js!("help.js"),
    js!("skills.js"),
    js!("templates.js"),
    js!("kits.js"),
    js!("upload.js"),
    js!("lifecycle.js"),
    js!("main.js"),
    js!("relay.js"),
    js!("plans.js"),
    js!("chat-core.js"),
    js!("worktrees.js"),
    js!("board.js"),
    js!("chat-dock.js"),
    js!("sidepanel.js"),
];

const SIDEPANEL: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>sbxw side panel</title>
<link rel="stylesheet" href="/app.css">
</head>
<body class="sp-body">
<script>const SP_EXTENSION_ORIGIN = '__EXTENSION_ORIGIN__';</script>
<script src="/js/chat-core.js"></script>
<script src="/js/sidepanel.js"></script>
</body>
</html>
"#;

pub fn routes() -> Router<Arc<App>> {
    let mut r = Router::new().route("/", get(index)).route("/sidepanel", get(sidepanel));
    for (path, ctype, body) in STATIC {
        r = r.route(path, get(move || async move { ([(header::CONTENT_TYPE, *ctype)], *body) }));
    }
    r
}

/// The shell, with the six values it carries baked in. Quotes are stripped
/// from the monitor command rather than escaped: it lands in a JS string.
async fn index(State(app): State<Arc<App>>) -> impl IntoResponse {
    let monitor = app.cfg.monitor_cmd.join(" ").replace(['"', '\\'], "");
    let initial = app.initial_sandbox.lock().unwrap().clone();
    Html(
        INDEX
            .replace("__SANDBOX__", &initial)
            .replace("__MONITOR__", &monitor)
            .replace("__TERM_COLOR__", &app.cfg.terminal_color)
            .replace("__TERM_BAR_COLOR__", &app.cfg.terminal_bar_color)
            .replace("__SBXW_VERSION__", &app.versions.sbxw)
            .replace("__SBX_VERSION__", app.versions.sbx.as_deref().unwrap_or("")),
    )
}

/// Framed by the extension beside the app being tested; only it (or sbxw)
/// may frame it — what a panel types lands in an agent's prompt.
async fn sidepanel() -> impl IntoResponse {
    let ext = format!("chrome-extension://{EXTENSION_ID}");
    (
        [(header::CONTENT_SECURITY_POLICY, format!("frame-ancestors 'self' {ext}"))],
        Html(SIDEPANEL.replace("__EXTENSION_ORIGIN__", &ext)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shell_loads_every_script_it_names() {
        let singleton = STATIC.iter().find(|(p, _, _)| *p == "/js/singleton.js").unwrap().2;
        for (path, _, _) in STATIC {
            if path.starts_with("/js/") && *path != "/js/singleton.js" && *path != "/js/sidepanel.js" {
                let file = path.trim_start_matches("/js/");
                assert!(singleton.contains(file), "singleton.js does not load {file}");
            }
        }
    }
}
