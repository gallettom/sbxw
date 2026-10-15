//! `sbxw extension` — put the sbxw browser extension where Chrome can load it.
//!
//! The extension is the side panel beside the app being tested (see
//! `assets/browser-extension/` and `web::sidepanel_handler`). It is a thin
//! shell around the daemon's `/sidepanel` page, so it rarely changes; what
//! does change ships inside sbxw and needs no reinstall.
//!
//! Chrome loads an unpacked extension from a folder, and keeps reading that
//! folder: it has to outlive the command, and stay put across sbxw updates.
//! Hence `~/.sbxw/browser-extension/`, rewritten whole on every run — by
//! install.sh and `sbxw update` too (`--refresh`), so it follows the binary.
//! Pointing Chrome at the folder is the one step only the user can take, once:
//! Chrome installs no unpacked extension on its own. After that, the panel
//! reloads the extension whenever the folder holds a newer one (panel.js).
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

/// The manifest's `version`. Chrome rereads the panel's files from the folder
/// every time it opens, but keeps the manifest and service worker it loaded
/// until the extension reloads; `defaults.js` repeats this version so the panel
/// can tell the two apart. Bump it whenever either of those files changes.
fn manifest_version() -> Result<String> {
    let manifest: serde_json::Value = serde_json::from_slice(FILES[0].1)?;
    Ok(manifest["version"].as_str().context("manifest.json has no version")?.to_string())
}

/// The daemon address an earlier install wrote into `dir`, if there is one.
fn installed_daemon(dir: &Path) -> Option<String> {
    let defaults = std::fs::read_to_string(dir.join("defaults.js")).ok()?;
    let value = defaults.split("const SBXW_DAEMON_DEFAULT = ").nth(1)?.split(";\n").next()?;
    serde_json::from_str(value).ok()
}

/// Write the extension into `dir`, replacing what an earlier run left there.
pub fn install(dir: &Path, daemon: &str) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let defaults = format!(
        "// Written by `sbxw extension`: the daemon address the panel starts from.\n\
         // The extension's options override it.\n\
         const SBXW_DAEMON_DEFAULT = {};\n\
         // The manifest version in this folder; the panel reloads the extension\n\
         // when Chrome still runs another one.\n\
         const SBXW_EXTENSION_VERSION = {};\n",
        serde_json::to_string(daemon)?,
        serde_json::to_string(&manifest_version()?)?
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

/// The command: install, then say how to load it. With `refresh` (install.sh,
/// `sbxw update`), keep the daemon address the folder was installed with, and
/// say how to load it only if there was nothing to refresh.
pub fn cmd(web_addr: &str, daemon: Option<String>, refresh: bool) -> Result<()> {
    let dir = default_dir();
    let first = !dir.join("manifest.json").is_file();
    let daemon = daemon
        .or_else(|| if refresh { installed_daemon(&dir) } else { None })
        .unwrap_or_else(|| daemon_url(web_addr));
    install(&dir, &daemon)?;
    if refresh && !first {
        println!("sbxw browser extension refreshed → {}", dir.display());
        return Ok(());
    }
    println!("sbxw browser extension → {}", dir.display());
    println!("  talks to the daemon at {daemon}\n");
    println!("First time, in Chrome or Edge:");
    println!("  1. open chrome://extensions (edge://extensions)");
    println!("  2. turn on Developer mode");
    println!("  3. Load unpacked → choose the folder above\n");
    println!("Then click the sbxw icon in the toolbar (Alt+Shift+S) on your app's tab.");
    println!("`sbxw update` keeps it current; the panel reloads the extension when it must.");
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
        let version = format!("const SBXW_EXTENSION_VERSION = \"{}\";", manifest_version().unwrap());
        assert!(defaults.contains(&version), "{defaults}");
        // What a refresh keeps.
        assert_eq!(installed_daemon(&dir).as_deref(), Some("http://127.0.0.1:7799"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Chrome keeps running the manifest and service worker it loaded until
    /// the extension reloads, and the panel reloads it only when the version
    /// changes: so either file changing without a new version would go unseen
    /// by every install that already has the extension.
    #[test]
    fn the_manifest_version_changes_with_the_manifest_or_the_service_worker() {
        // FNV-1a: stable across Rust releases, unlike `DefaultHasher`.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for (name, body) in FILES {
            if *name == "manifest.json" || *name == "service-worker.js" {
                for b in *body {
                    hash = (hash ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
                }
            }
        }
        let pinned = ("1.0.0", 0x9a1f_20b5_24ca_f737_u64);
        let version = manifest_version().unwrap();
        assert!(
            (version.as_str(), hash) == pinned,
            "manifest.json or service-worker.js changed: bump \"version\" in manifest.json \
             if it still says {}, then pin (\"{version}\", {hash:#018x}) here",
            pinned.0
        );
    }
}
