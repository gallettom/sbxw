//! `sbxw doctor`: sbxw's own checks, then what `sbx diagnose` reports.
//!
//! sbxw's checks are the ones only it can make (its config, its daemon, the
//! kits it will hand to `sbx create`); everything about sbx itself — running
//! CLI and shim processes, the global network policy, the ZFS setting that
//! slows image builds — is `sbx diagnose`'s, read as JSON and
//! summarised without pinning its shape.

use super::{check_sbx, load_config, runtime};
use crate::client::Daemon;
use anyhow::{bail, Result};
use sbxw_core::kits;
use sbxw_core::templates;
use sbxw_daemon::store::{Paths, Store};
use sbxw_sbx::Sbx;
use serde_json::Value;
use std::path::Path;

#[derive(Debug, PartialEq)]
enum Level {
    Ok,
    Warn,
    Fail,
}

struct Report {
    fails: usize,
}

impl Report {
    fn line(&mut self, level: Level, what: &str, detail: &str) {
        let mark = match level {
            Level::Ok => "✓",
            Level::Warn => "!",
            Level::Fail => {
                self.fails += 1;
                "✗"
            }
        };
        if detail.is_empty() {
            println!("  {mark} {what}");
        } else {
            println!("  {mark} {what} — {detail}");
        }
    }
}

pub fn run(config: &Path, json: bool) -> Result<()> {
    let rt = runtime();
    let sbx = Sbx::process();
    if json {
        // The raw document, for a bug report: sbx's own words, unedited.
        println!("{}", rt.block_on(sbx.diagnose())?.trim_end());
        return Ok(());
    }
    let mut r = Report { fails: 0 };

    println!("sbxw");
    let sbx_version = match check_sbx(&rt) {
        Ok(Some(v)) => {
            r.line(Level::Ok, "sbx", &format!("version {v}"));
            v
        }
        Ok(None) => {
            r.line(Level::Warn, "sbx", "version unreadable");
            "unknown".into()
        }
        Err(e) => {
            r.line(Level::Fail, "sbx", &format!("{e:#}"));
            "unknown".into()
        }
    };
    let cfg = match load_config(config) {
        Ok(c) => {
            let shown = if config.exists() { config.display().to_string() } else { "defaults (no sbxw.toml here)".into() };
            r.line(Level::Ok, "config", &shown);
            c
        }
        Err(e) => {
            r.line(Level::Fail, "config", &format!("{e:#}"));
            Default::default()
        }
    };
    // sbx 0.50 warns about these on macOS and Windows, where sbx otherwise
    // uses the system trust store: a replaced store breaks HTTPS verification
    // (the proxy, kit pulls, sign-in) in ways that look like network faults.
    if cfg!(any(target_os = "macos", target_os = "windows")) {
        for var in ["SSL_CERT_FILE", "SSL_CERT_DIR"] {
            if let Some(v) = std::env::var_os(var).filter(|v| !v.is_empty()) {
                r.line(
                    Level::Warn,
                    var,
                    &format!("set to {} — it replaces the system trust store and can make sbx's HTTPS checks fail", v.to_string_lossy()),
                );
            }
        }
    }
    match Daemon::new(&cfg.web_addr).health() {
        Some(_) => r.line(Level::Ok, "daemon", &format!("answering on {}", cfg.web_addr)),
        None => r.line(Level::Warn, "daemon", &format!("not running on {} — `sbxw up` starts it", cfg.web_addr)),
    }
    match cfg.template_mode() {
        templates::Mode::Off => {}
        templates::Mode::Tag(t) => r.line(Level::Ok, "template", &format!("new sandboxes start from {t}")),
        templates::Mode::Auto => {
            let store = Store::open(Paths::from_home());
            let content = sbxw_daemon::bundled::fingerprint(&store.paths, &cfg.kits).unwrap_or_default();
            let now = templates::Current { sbx: &sbx_version, sbxw: env!("CARGO_PKG_VERSION"), content: &content };
            match templates::check(&store.templates(), &cfg.kits, &now) {
                templates::Freshness::Fresh { record } => r.line(Level::Ok, "template (auto)", &record.tag),
                templates::Freshness::Stale { record, changed } => r.line(
                    Level::Warn,
                    "template (auto)",
                    &format!("{} is out of date ({}) — `sbxw up <name> --rebuild-template`", record.tag, changed.join(", ")),
                ),
                templates::Freshness::Missing => {
                    r.line(Level::Warn, "template (auto)", "none yet for these kits — `sbxw up <name> --rebuild-template`")
                }
            }
        }
    }
    for kit in &cfg.kits {
        if kits::bundled_name(kit).is_some() {
            continue;
        }
        if !Path::new(kit).exists() {
            let remote = kit.contains("://") || kit.contains(':') || kit.starts_with("git@");
            if remote {
                r.line(Level::Ok, &format!("kit {kit}"), "remote — checked by sbx at creation");
            } else {
                r.line(Level::Fail, &format!("kit {kit}"), "no such file or directory");
            }
            continue;
        }
        if kits::is_v3_kit(Path::new(kit)) {
            r.line(Level::Ok, &format!("kit {kit}"), "v3 — `sbx kit validate` cannot read v3 specs; checked by sbx at creation");
            continue;
        }
        match rt.block_on(sbx.kit_validate_offline(kit)) {
            Ok(()) => r.line(Level::Ok, &format!("kit {kit}"), "valid"),
            Err(e) => r.line(Level::Fail, &format!("kit {kit}"), &format!("{e:#}")),
        }
    }

    println!("\nsbx diagnose");
    match rt.block_on(sbx.diagnose()) {
        Ok(raw) => match serde_json::from_str::<Value>(raw.trim()) {
            Ok(v) => {
                let checks = summarise(&v);
                if checks.is_empty() {
                    println!("{}", serde_json::to_string_pretty(&v).unwrap_or(raw));
                }
                for (level, what, detail) in checks {
                    r.line(level, &what, &detail);
                }
            }
            Err(_) => println!("{}", raw.trim_end()),
        },
        Err(e) => r.line(Level::Fail, "sbx diagnose", &format!("{e:#}")),
    }
    println!("\n`sbxw doctor --json` prints sbx's full report.");
    if r.fails > 0 {
        bail!("{} check(s) failed", r.fails);
    }
    Ok(())
}

/// Every object in the report that names a check and gives it a status, as
/// lines. Schema-less on purpose: a field sbx renames costs a line, not the
/// command.
fn summarise(v: &Value) -> Vec<(Level, String, String)> {
    let mut out = Vec::new();
    walk(v, &mut out);
    out
}

fn walk(v: &Value, out: &mut Vec<(Level, String, String)>) {
    match v {
        Value::Array(items) => items.iter().for_each(|i| walk(i, out)),
        Value::Object(map) => {
            let text = |keys: &[&str]| keys.iter().find_map(|k| map.get(*k).and_then(Value::as_str)).map(str::to_string);
            let name = text(&["name", "check", "title", "id"]);
            let status = text(&["status", "result", "level", "severity", "state"]);
            match (name, status) {
                (Some(name), Some(status)) => {
                    let detail = text(&["message", "detail", "details", "hint", "error"]).unwrap_or_default();
                    out.push((level_of(&status), name, detail));
                }
                _ => map.values().for_each(|i| walk(i, out)),
            }
        }
        _ => {}
    }
}

fn level_of(status: &str) -> Level {
    match status.to_ascii_lowercase().as_str() {
        "ok" | "pass" | "passed" | "success" | "healthy" | "info" | "found" | "running" => Level::Ok,
        "fail" | "failed" | "error" | "critical" | "missing" => Level::Fail,
        _ => Level::Warn,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_are_found_wherever_the_report_nests_them() {
        let v: Value = serde_json::from_str(
            r#"{"version":"0.49.1","sections":[{"title":"Host","checks":[
                {"name":"mkfs.ext4","status":"ok"},
                {"name":"zfs sync","status":"warning","message":"sync=always slows image builds"}]},
               {"processes":[{"check":"sbx shim","result":"error","error":"2 shims running"}]}]}"#,
        )
        .unwrap();
        let c = summarise(&v);
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].0, Level::Ok);
        assert_eq!((c[1].0 == Level::Warn, c[1].2.as_str()), (true, "sync=always slows image builds"));
        assert_eq!(c[2].0, Level::Fail);
    }
}
