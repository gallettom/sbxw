//! `sbxw extension` — put the sbxw browser extension where Chrome can load it.
//!
//! The extension is the side panel beside the app being tested (see
//! `assets/browser-extension/` and `web::sidepanel_handler`). It is a thin
//! shell around the daemon's `/sidepanel` page, so it rarely changes; what
//! does change ships inside sbxw and needs no reinstall.
//!
//! Chrome loads an unpacked extension from a folder, and keeps reading that
//! folder: it has to outlive the command, and stay put across sbxw updates.
//! Hence `~/.sbxw/browser-extension/`, rewritten whole on every run.
//!
//! Its scripts and manifest are files under `assets/browser-extension/`. Its
//! two pages are written from here, like the daemon's own `/sidepanel` shell,
//! and so is `defaults.js`: the daemon address the panel starts from, taken
//! from `web_addr` so a project that moved the daemon needs no options page.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Every file the extension consists of, as (path inside it, contents).
///
/// The icons are the chat dock's `>_` in its dot-matrix: solid pixels at the
/// toolbar's 16/32/48, the lit-and-unlit grid on the 128 of the extensions page.
const FILES: &[(&str, &[u8])] = &[
    ("manifest.json", include_bytes!("../assets/browser-extension/manifest.json")),
    ("service-worker.js", include_bytes!("../assets/browser-extension/service-worker.js")),
    ("panel.js", include_bytes!("../assets/browser-extension/panel.js")),
    ("options.js", include_bytes!("../assets/browser-extension/options.js")),
    ("panel.css", include_bytes!("../assets/browser-extension/panel.css")),
    ("panel.html", PANEL_HTML.as_bytes()),
    ("options.html", OPTIONS_HTML.as_bytes()),
    ("icons/icon16.png", include_bytes!("../assets/browser-extension/icons/icon16.png")),
    ("icons/icon32.png", include_bytes!("../assets/browser-extension/icons/icon32.png")),
    ("icons/icon48.png", include_bytes!("../assets/browser-extension/icons/icon48.png")),
    ("icons/icon128.png", include_bytes!("../assets/browser-extension/icons/icon128.png")),
];

/// The side panel: an iframe on the daemon's `/sidepanel`, a banner above it
/// for the tab permissions, and what to say when the daemon is not running.
const PANEL_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>sbxw</title>
<link rel="stylesheet" href="panel.css">
</head>
<body class="panel">
<div class="banner" id="banner" hidden></div>
<div class="down" id="down" hidden>
  <p>sbxw isn’t answering at <code id="down-addr"></code>.</p>
  <p>Start it with <code>sbxw up</code>, then retry.</p>
  <div class="row">
    <button type="button" class="primary" id="retry">Retry</button>
    <button type="button" class="quiet" id="options">Change the address</button>
  </div>
</div>
<iframe id="frame" title="sbxw chat" hidden></iframe>
<script src="defaults.js"></script>
<script src="panel.js"></script>
</body>
</html>
"#;

const OPTIONS_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>sbxw options</title>
<link rel="stylesheet" href="panel.css">
</head>
<body class="options">
<form id="form">
  <label for="daemon">sbxw daemon address</label>
  <div class="row">
    <input id="daemon" type="url" spellcheck="false" autocomplete="off">
    <button type="submit" class="primary">Save</button>
  </div>
  <p class="note" id="note"></p>
</form>
<script src="defaults.js"></script>
<script src="options.js"></script>
</body>
</html>
"#;

/// Where the extension is installed.
pub fn default_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".sbxw")
        .join("browser-extension")
}

/// The URL a browser reaches the daemon at, from `web_addr`. An address bound
/// on every interface is reached on the loopback.
pub fn daemon_url(web_addr: &str) -> String {
    let ip = crate::web_ip_of(web_addr);
    let port = crate::web_port_of(web_addr);
    let host = match ip {
        "" | "0.0.0.0" | "::" => "127.0.0.1".to_string(),
        v6 if v6.contains(':') => format!("[{v6}]"),
        other => other.to_string(),
    };
    format!("http://{host}:{port}")
}

/// Write the extension into `dir`, replacing what an earlier run left there.
pub fn install(dir: &Path, daemon: &str) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let defaults = format!(
        "// Written by `sbxw extension`: the daemon address the panel starts from.\n\
         // The extension's options override it.\n\
         const SBXW_DAEMON_DEFAULT = {};\n",
        serde_json::to_string(daemon)?
    );
    let all = FILES.iter().copied().chain(std::iter::once(("defaults.js", defaults.as_bytes())));
    for (name, body) in all {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        std::fs::write(&path, body).with_context(|| format!("could not write {}", path.display()))?;
    }
    Ok(())
}

/// The command: install, then say how to load it.
pub fn cmd(web_addr: &str, daemon: Option<String>) -> Result<()> {
    let dir = default_dir();
    let daemon = daemon.unwrap_or_else(|| daemon_url(web_addr));
    install(&dir, &daemon)?;
    println!("sbxw browser extension → {}", dir.display());
    println!("  talks to the daemon at {daemon}\n");
    println!("First time, in Chrome or Edge:");
    println!("  1. open chrome://extensions (edge://extensions)");
    println!("  2. turn on Developer mode");
    println!("  3. Load unpacked → choose the folder above\n");
    println!("Then click the sbxw icon in the toolbar (Alt+Shift+S) on your app's tab.");
    println!("After an sbxw update, run this again and click ↻ on the extension's card.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_daemon_is_reached_where_web_addr_says_and_on_loopback_when_bound_everywhere() {
        assert_eq!(daemon_url("127.0.0.1:7681"), "http://127.0.0.1:7681");
        assert_eq!(daemon_url("0.0.0.0:7700"), "http://127.0.0.1:7700");
        assert_eq!(daemon_url("[::1]:7681"), "http://[::1]:7681");
    }

    #[test]
    fn the_manifest_names_every_file_the_extension_is_installed_with() {
        let manifest: serde_json::Value =
            serde_json::from_slice(FILES[0].1).expect("manifest.json is JSON");
        let mut named = vec![
            &manifest["background"]["service_worker"],
            &manifest["side_panel"]["default_path"],
            &manifest["options_ui"]["page"],
        ];
        named.extend(manifest["icons"].as_object().expect("icons").values());
        named.extend(manifest["action"]["default_icon"].as_object().expect("action icons").values());
        for file in named {
            let file = file.as_str().unwrap();
            assert!(FILES.iter().any(|(n, _)| *n == file), "{file} is not installed");
        }
        // The key is what fixes the ID the daemon's `/sidepanel` trusts.
        assert!(manifest["key"].as_str().is_some_and(|k| k.len() > 100));
    }

    #[test]
    fn install_writes_every_file_and_the_daemon_address() {
        let dir = std::env::temp_dir().join(format!("sbxw-ext-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        install(&dir, "http://127.0.0.1:7799").unwrap();
        for (name, _) in FILES {
            assert!(dir.join(name).is_file(), "{name}");
        }
        let defaults = std::fs::read_to_string(dir.join("defaults.js")).unwrap();
        assert!(defaults.contains(r#"const SBXW_DAEMON_DEFAULT = "http://127.0.0.1:7799";"#));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
