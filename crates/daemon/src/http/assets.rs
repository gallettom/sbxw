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

macro_rules! vendor {
    ($file:literal, $ctype:literal) => {
        (concat!("/vendor/", $file), $ctype, include_str!(concat!("../../../../assets/vendor/", $file)))
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
    js!("mcp.js"),
    js!("hosts.js"),
    js!("mac.js"),
    js!("github.js"),
    js!("approvals.js"),
    js!("setup.js"),
    js!("upload.js"),
    js!("lifecycle.js"),
    js!("main.js"),
    js!("relay.js"),
    js!("plans.js"),
    js!("chat-core.js"),
    js!("worktrees.js"),
    js!("board.js"),
    js!("chat-dock.js"),
    js!("concierge.js"),
    js!("sidepanel.js"),
    // xterm.js, vendored from the npm registry (see assets/vendor/README.md)
    // rather than fetched from a CDN into a page that can open a host terminal.
    vendor!("xterm.js", "application/javascript; charset=utf-8"),
    vendor!("addon-fit.js", "application/javascript; charset=utf-8"),
    vendor!("addon-canvas.js", "application/javascript; charset=utf-8"),
    vendor!("xterm.css", "text/css; charset=utf-8"),
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

/// The shell with its templated values baked in. Each placeholder in the
/// inline config script stands for a whole JavaScript value, so it gets a
/// complete string literal (see [`js_string`]): `monitor_cmd` and the colours
/// come from an `sbxw.toml` an agent can write, and this page can open a
/// terminal on the host.
async fn index(State(app): State<Arc<App>>) -> impl IntoResponse {
    let initial = app.initial_sandbox.lock().unwrap().clone();
    Html(render_index(&app.cfg, &initial, &app.versions.sbxw, app.versions.sbx.as_deref().unwrap_or("")))
}

fn render_index(cfg: &sbxw_core::config::Config, initial: &str, sbxw: &str, sbx: &str) -> String {
    let monitor = cfg.monitor_cmd.join(" ");
    fill(
        INDEX,
        &[
            ("__SANDBOX__", initial),
            ("__MONITOR__", &monitor),
            ("__TERM_COLOR__", &cfg.terminal_color),
            ("__TERM_BAR_COLOR__", &cfg.terminal_bar_color),
            ("__TERM_TEXT_COLOR__", &cfg.terminal_text_color),
            ("__SBXW_VERSION__", sbxw),
            ("__SBX_VERSION__", sbx),
        ],
    )
}

/// One pass over the template, so a value that happens to contain another
/// placeholder's name is never substituted a second time.
fn fill(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    'outer: while let Some(i) = rest.find("__") {
        for (name, value) in vars {
            if rest[i..].starts_with(name) {
                out.push_str(&rest[..i]);
                out.push_str(&js_string(value));
                rest = &rest[i + name.len()..];
                continue 'outer;
            }
        }
        out.push_str(&rest[..i + 2]);
        rest = &rest[i + 2..];
    }
    out.push_str(rest);
    out
}

/// A JavaScript string literal safe inside an HTML `<script>` element: JSON
/// escaping, plus `<`, `>` and `&` (no `</script>` or `<!--`) and U+2028/U+2029
/// (line terminators in older JS) as `\uXXXX`.
fn js_string(s: &str) -> String {
    let json = serde_json::to_string(s).expect("a string always serializes");
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        match c {
            '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
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

    fn config_block(page: &str) -> &str {
        let start = page.find("const INITIAL_SANDBOX").unwrap();
        let end = start + page[start..].find("</script>").unwrap();
        &page[start..end]
    }

    #[test]
    fn a_hostile_monitor_cmd_stays_inside_its_string() {
        let cfg = sbxw_core::config::Config {
            monitor_cmd: vec!["x</script><script>alert(1)".into(), "\"\n\u{2028}&".into()],
            terminal_color: "\";</script><script>alert(2)//".into(),
            ..Default::default()
        };
        let page = render_index(&cfg, "__MONITOR__", "1.0", "");
        let block = config_block(&page);
        assert!(!block.contains("<script"), "{block}");
        assert!(!block.contains('\u{2028}'));
        assert!(block.contains(r#"const MONITOR_CMD = "x\u003c/script\u003e\u003cscript\u003ealert(1) \"\n\u2028\u0026";"#), "{block}");
        // A value naming another placeholder is not substituted again.
        assert!(block.contains(r#"const INITIAL_SANDBOX = "__MONITOR__";"#), "{block}");
        assert!(block.contains(r#"const SBX_VERSION  = "";"#), "{block}");
        assert!(!page.contains("__SANDBOX__") && !page.contains("__TERM_COLOR__"));
        assert!(page.contains("const MONITOR_SANDBOX = '__host__';"));
    }

    #[test]
    fn the_shell_loads_every_script_it_names() {
        let singleton = STATIC.iter().find(|(p, _, _)| *p == "/js/singleton.js").unwrap().2;
        for (path, _, _) in STATIC {
            if path.starts_with("/js/") && *path != "/js/singleton.js" && *path != "/js/sidepanel.js" {
                let file = path.trim_start_matches("/js/");
                assert!(singleton.contains(file), "singleton.js does not load {file}");
            }
            if path.starts_with("/vendor/") && path.ends_with(".js") {
                assert!(singleton.contains(&format!("'{path}'")), "singleton.js does not load {path}");
            }
            if path.starts_with("/vendor/") {
                let body = STATIC.iter().find(|(p, _, _)| p == path).unwrap().2;
                assert!(!body.contains("sourceMappingURL"), "{path} still names a source map");
            }
        }
        assert!(INDEX.contains(r#"href="/vendor/xterm.css""#), "index.html does not link the vendored stylesheet");
    }

    /// Nothing the shell loads comes from another origin: a page that can open
    /// a terminal on the host runs only code compiled into the binary.
    #[test]
    fn the_shell_loads_nothing_from_a_cdn() {
        let singleton = STATIC.iter().find(|(p, _, _)| *p == "/js/singleton.js").unwrap().2;
        // Every `<script>` and `<link>` tag in the shell.
        for tag in ["<script", "<link"] {
            for (i, _) in INDEX.match_indices(tag) {
                let end = i + INDEX[i..].find('>').unwrap();
                let el = &INDEX[i..end];
                for attr in ["src=\"", "href=\""] {
                    if let Some(j) = el.find(attr) {
                        let url = &el[j + attr.len()..];
                        let url = &url[..url.find('"').unwrap_or(url.len())];
                        let local = url.starts_with("data:") || (url.starts_with('/') && !url.starts_with("//"));
                        assert!(local, "index.html loads a remote URL: {el}");
                    }
                }
            }
        }
        // Every string literal in the script list `singleton.js` loads.
        let start = singleton.find("const SBXW_APP_SCRIPTS").unwrap();
        let list = &singleton[start..start + singleton[start..].find("];").unwrap()];
        let code: String = list.lines().filter(|l| !l.trim_start().starts_with("//")).collect();
        for entry in code.split('\'').skip(1).step_by(2) {
            assert!(entry.starts_with('/') && !entry.starts_with("//"), "singleton.js loads {entry}");
        }
        assert!(!singleton.contains("https://") && !INDEX.contains("cdn.jsdelivr.net"));
    }
}
