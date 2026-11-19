//! The kits embedded by `build.rs`: what `sbxw:<name>` in `kits` refers to.
//! Each is a kit spec v3 directory (`spec.yaml` plus the `files/` tree its
//! `build:` copies). sbx keeps a kit's *path*, so each is written to a stable
//! `~/.sbxw/kits/<name>/`, and a file is rewritten only when this build's copy
//! differs: the path never goes missing and an unchanged tree keeps its cache.

use crate::store::{write_atomic, Paths};
use anyhow::{anyhow, Context, Result};
use sbxw_core::kits::bundled_name;
use std::path::Path;

include!(concat!(env!("OUT_DIR"), "/bundled_kits.rs"));

/// The kits that fill the shared cache: each drops `.filled-<name>` there
/// once its startup has downloaded into it.
pub const CACHING: [&str; 4] = ["agent-skills", "md-to-pdf-tools", "k8s-tools", "firecrawl"];

/// `kits` with every `sbxw:<name>` replaced by its written-out directory.
pub fn resolve(paths: &Paths, kits: &[String]) -> Result<Vec<String>> {
    kits.iter()
        .map(|k| match bundled_name(k) {
            None => Ok(k.clone()),
            Some(name) => write_out(paths, name),
        })
        .collect()
}

/// [`sbxw_core::templates::fingerprint`] of the kits `kits` resolves to: each
/// local kit's content, else the reference itself.
pub fn fingerprint(paths: &Paths, kits: &[String]) -> Result<String> {
    let texts: Vec<String> = resolve(paths, kits)?.iter().map(|k| sbxw_core::kits::content_text(k)).collect();
    Ok(sbxw_core::templates::fingerprint(&texts))
}

/// The kit directories this process has already written out and checked.
/// Every bring-up resolves its kits several times (the create, the template
/// fingerprint, a mirror), and comparing each embedded file with the disk
/// each time is file I/O on the async threads for nothing: the bytes are this
/// build's and do not change while it runs. A directory someone removed is
/// written again.
static WRITTEN: std::sync::Mutex<Vec<std::path::PathBuf>> = std::sync::Mutex::new(Vec::new());

fn write_out(paths: &Paths, name: &str) -> Result<String> {
    let (_, files) = KITS.iter().find(|(n, _)| *n == name).ok_or_else(|| anyhow!("no bundled kit named '{name}'"))?;
    let dir = paths.kits().join(name);
    if WRITTEN.lock().unwrap().contains(&dir) && dir.join("spec.yaml").is_file() {
        return Ok(dir.to_string_lossy().into_owned());
    }
    for (rel, mode, bytes) in *files {
        let file = dir.join(rel);
        if std::fs::read(&file).ok().as_deref() != Some(*bytes) {
            write_atomic(&file, bytes).with_context(|| format!("writing the bundled kit '{name}'"))?;
        }
        set_mode(&file, *mode);
    }
    prune(&dir.join("files"), &dir, files);
    // Readable by sbx and its builder; nothing here is a secret.
    set_tree_mode(&dir);
    WRITTEN.lock().unwrap().push(dir.clone());
    Ok(dir.to_string_lossy().into_owned())
}

/// Remove files under `files/` that this build no longer ships: the kit build
/// copies the whole tree, so a stale file would land in the sandbox.
fn prune(at: &Path, root: &Path, keep: &[(&str, u32, &[u8])]) {
    let Ok(entries) = std::fs::read_dir(at) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            prune(&p, root, keep);
            let _ = std::fs::remove_dir(&p); // only when empty
        } else {
            let rel = p.strip_prefix(root).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default();
            if !keep.iter().any(|(k, _, _)| *k == rel) {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}

#[cfg(unix)]
fn set_mode(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) {}

fn set_tree_mode(dir: &Path) {
    set_mode(dir, 0o755);
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.path().is_dir() {
                set_tree_mode(&e.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_references_are_written_once_at_a_stable_path() {
        let d = tempfile::tempdir().unwrap();
        let p = Paths::under(d.path());
        let kits = vec!["sbxw:codemap".to_string(), "/abs/other".to_string()];
        let r = resolve(&p, &kits).unwrap();
        assert_eq!(r[1], "/abs/other");
        let spec = p.kits().join("codemap/spec.yaml");
        assert!(std::fs::read_to_string(&spec).unwrap().contains("displayName: codemap"));
        let script = p.kits().join("codemap/files/home/agent/.sbxw/codemap/install-hooks.mjs");
        assert!(script.is_file());
        // A time of its own, far in the past: a rewrite would change it, at
        // any timestamp resolution (no sleep to wait one out).
        let before = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        std::fs::File::options().write(true).open(&spec).unwrap().set_modified(before).unwrap();
        resolve(&p, &kits).unwrap();
        assert_eq!(std::fs::metadata(&spec).unwrap().modified().unwrap(), before, "unchanged, not rewritten");
        assert!(resolve(&p, &["sbxw:nope".into()]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn scripts_keep_their_mode_and_stale_files_go() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = Paths::under(d.path());
        let stale = p.kits().join("retro/files/home/agent/.claude/commands/gone.md");
        std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
        std::fs::write(&stale, "old").unwrap();
        resolve(&p, &["sbxw:retro".into()]).unwrap();
        assert!(!stale.exists(), "a file this build does not ship is removed");
        let digest = p.kits().join("retro/files/home/agent/.sbxw/retro/digest.mjs");
        assert_eq!(std::fs::metadata(&digest).unwrap().permissions().mode() & 0o777, 0o755);
        let cmd = p.kits().join("retro/files/home/agent/.claude/commands/retro.md");
        assert_eq!(std::fs::metadata(&cmd).unwrap().permissions().mode() & 0o777, 0o644);
    }

    #[test]
    fn the_base_kit_carries_the_relay_guidance_the_tooling_writes() {
        let (_, files) = KITS.iter().find(|(n, _)| *n == sbxw_core::kits::BASE).unwrap();
        let (_, _, spec) = files.iter().find(|(r, _, _)| *r == "spec.yaml").unwrap();
        let doc: serde_norway::Value = serde_norway::from_str(std::str::from_utf8(spec).unwrap()).unwrap();
        let ctx = doc["capabilities"].as_sequence().unwrap().iter().find(|c| c["type"] == "com.docker.sandbox/agent-context@2").expect("agent-context@2");
        assert_eq!(ctx["config"]["content"].as_str(), Some(sbxw_core::tooling::relay_guidance().as_str()), "edit both, or neither");
    }

    #[test]
    fn the_jarvis_kit_carries_the_role_the_tooling_writes() {
        let (_, files) = KITS.iter().find(|(n, _)| *n == sbxw_core::kits::JARVIS).unwrap();
        let (_, _, spec) = files.iter().find(|(r, _, _)| *r == "spec.yaml").unwrap();
        let doc: serde_norway::Value = serde_norway::from_str(std::str::from_utf8(spec).unwrap()).unwrap();
        let ctx = doc["capabilities"].as_sequence().unwrap().iter().find(|c| c["type"] == "com.docker.sandbox/agent-context@2").expect("agent-context@2");
        assert_eq!(ctx["config"]["content"].as_str(), Some(sbxw_core::tooling::JARVIS_ROLE), "edit both, or neither");
    }

    #[test]
    fn every_bundled_kit_is_embedded_and_v3() {
        for name in sbxw_core::kits::BUNDLED.iter().chain([&sbxw_core::kits::BASE, &sbxw_core::kits::JARVIS]) {
            let (_, files) = KITS.iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("{name}"));
            let (_, _, spec) = files.iter().find(|(r, _, _)| *r == "spec.yaml").unwrap();
            let spec = std::str::from_utf8(spec).unwrap();
            assert!(spec.starts_with("# syntax=docker/sandbox-kit:3\nschemaVersion: \"3\"\n"), "{name}: v3 only since sbx 0.51");
            let doc: serde_norway::Value = serde_norway::from_str(spec).unwrap_or_else(|e| panic!("{name}: {e}"));
            // What the build copies is what is embedded.
            if let Some(build) = doc.get("build").and_then(|b| b.as_str()) {
                assert!(build.contains("COPY --chown=1000:1000 files/home/agent/ /home/agent/"), "{name}");
                assert!(files.iter().any(|(r, _, _)| r.starts_with("files/home/agent/")), "{name}: a build with nothing to copy");
            }
        }
    }

    #[test]
    fn the_jarvis_kit_installs_nothing_and_ships_its_server() {
        let (_, files) = KITS.iter().find(|(n, _)| *n == sbxw_core::kits::JARVIS).expect("embedded");
        let (_, _, spec) = files.iter().find(|(r, _, _)| *r == "spec.yaml").unwrap();
        let doc: serde_norway::Value = serde_norway::from_str(std::str::from_utf8(spec).unwrap()).unwrap();
        let kinds: Vec<&str> = doc["capabilities"].as_sequence().unwrap().iter().filter_map(|c| c["type"].as_str()).collect();
        // Light on purpose: no lifecycle hook, no network rule, no credential.
        assert_eq!(kinds, ["com.docker.sandbox/agent-context@2"], "{kinds:?}");
        let server = format!("files{}", sbxw_core::tooling::CONFIG_MCP_PATH);
        let (_, mode, bytes) = files.iter().find(|(r, _, _)| *r == server).unwrap_or_else(|| panic!("{server} is what the tooling registers"));
        // What a creation copies is what the batch refreshes an existing Jarvis with.
        assert_eq!(*bytes, sbxw_core::tooling::CONFIG_MCP.as_bytes(), "one source for the kit and the refresh");
        assert_eq!(*mode, 0o644, "the refresh writes it 0644, as the kit ships it");
        let rels: Vec<&str> = files.iter().map(|(r, _, _)| *r).collect();
        assert_eq!(rels.len(), 2, "the spec and the server, nothing else: {rels:?}");
        assert!(!sbxw_core::kits::BUNDLED.contains(&sbxw_core::kits::JARVIS), "never offered to a project");
    }

    fn firecrawl_file(rel: &str) -> &'static [u8] {
        let (_, files) = KITS.iter().find(|(n, _)| *n == "firecrawl").expect("embedded");
        files.iter().find(|(r, _, _)| *r == rel).unwrap_or_else(|| panic!("{rel}")).2
    }

    #[test]
    fn the_firecrawl_kit_installs_from_the_cache_and_widens_nothing() {
        let spec: serde_norway::Value = serde_norway::from_slice(firecrawl_file("spec.yaml")).unwrap();
        let caps = spec["capabilities"].as_sequence().unwrap();
        let kinds: Vec<&str> = caps.iter().filter_map(|c| c["type"].as_str()).collect();
        // No network rule, port, credential or grant: what it fetches goes
        // through the sandbox's own policy.
        assert_eq!(kinds, ["com.docker.sandbox/lifecycle@1", "com.docker.sandbox/agent-context@2"], "{kinds:?}");
        let ctx = caps.iter().find(|c| c["type"] == "com.docker.sandbox/agent-context@2").unwrap();
        assert_eq!(ctx["config"]["content"].as_str(), Some(sbxw_core::tooling::firecrawl_guidance().as_str()), "edit both, or neither");
        let startup = caps[0]["config"]["startup"][0]["command"].as_sequence().unwrap();
        assert_eq!(startup[1].as_str(), Some("/home/agent/.sbxw/firecrawl/install.sh"));
        assert!(CACHING.contains(&"firecrawl") && sbxw_core::kits::BUNDLED.contains(&"firecrawl"));
        assert!(sbxw_core::kits::DEFAULT_KITS.contains(&"sbxw:firecrawl"), "the default tool for the web");
        // The front script is the MCP server's command, and executable.
        let (_, files) = KITS.iter().find(|(n, _)| *n == "firecrawl").unwrap();
        for exe in ["files/home/agent/.sbxw/firecrawl/install.sh", "files/home/agent/.sbxw/firecrawl/shim/npx", &format!("files{}", sbxw_core::tooling::FIRECRAWL_PATH)] {
            let (_, mode, _) = files.iter().find(|(r, _, _)| *r == exe).unwrap_or_else(|| panic!("{exe}"));
            assert_eq!(*mode, 0o755, "{exe}");
        }
        // Pinned: the version and the crate's sha256, rustup-init by hash for
        // each architecture, the MCP package by its lock.
        let script = std::str::from_utf8(firecrawl_file("files/home/agent/.sbxw/firecrawl/install.sh")).unwrap();
        let pin = |k: &str| script.lines().find_map(|l| l.trim().strip_prefix(&format!("{k}="))).map(|v| v.split_whitespace().next().unwrap().to_string());
        assert!(pin("FIRECRAWL_VERSION").is_some_and(|v| v.split('.').count() == 3));
        assert!(pin("FIRECRAWL_CRATE_SHA256").is_some_and(|v| v.len() == 64 && v.chars().all(|c| c.is_ascii_hexdigit())));
        assert_eq!(script.matches("RUSTUP_SHA256=").count(), 3, "x86_64, aarch64, and none");
        assert!(script.contains("--locked") && script.contains("npm ci"), "the locks are what is installed");
        let lock: serde_json::Value = serde_json::from_slice(firecrawl_file("files/home/agent/.sbxw/firecrawl/mcp/package-lock.json")).unwrap();
        let mcp = &lock["packages"]["node_modules/firecrawl-mcp"];
        assert_eq!(mcp["version"].as_str(), pin("MCP_VERSION").as_deref());
        for (path, p) in lock["packages"].as_object().unwrap().iter().filter(|(k, _)| !k.is_empty()) {
            assert!(p["integrity"].as_str().is_some_and(|i| i.starts_with("sha512-")), "{path}");
        }
        // Nothing in the kit starts a listener.
        for (rel, _, bytes) in files.iter() {
            assert!(!String::from_utf8_lossy(bytes).contains("firecrawl-local serve"), "{rel}");
        }
    }

    /// The kit's startup, run for real (bash) against a fake home and cache.
    #[cfg(unix)]
    #[test]
    fn the_firecrawl_startup_copies_from_the_cache_and_never_fails() {
        use std::process::Command;
        if Command::new("bash").arg("--version").output().is_err() || Command::new("sha256sum").arg("--version").output().is_err() {
            eprintln!("bash or sha256sum not found — skipping");
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let p = Paths::under(&d.path().join("sbxw"));
        let kit = std::path::PathBuf::from(resolve(&p, &["sbxw:firecrawl".into()]).unwrap().remove(0));
        let script_text = std::fs::read_to_string(kit.join("files/home/agent/.sbxw/firecrawl/install.sh")).unwrap();
        let pin = |k: &str| script_text.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).unwrap().split_whitespace().next().unwrap().to_string();
        let (version, mcp) = (pin("FIRECRAWL_VERSION"), format!("mcp-{}", pin("MCP_VERSION")));
        let flavour = {
            let arch = Command::new("dpkg").arg("--print-architecture").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
            let codename = std::fs::read_to_string("/etc/os-release").ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("VERSION_CODENAME=").map(|v| v.trim_matches('"').to_string()))).unwrap_or_default();
            format!("{arch}-{codename}")
        };
        let cache = d.path().join("cache");
        let store = cache.join(&flavour).join("firecrawl");
        let fake_bin = |dir: &std::path::Path, sum: Option<&str>| {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("firecrawl-local"), "#!/bin/sh\necho fake\n").unwrap();
            let real = String::from_utf8(Command::new("sha256sum").arg(dir.join("firecrawl-local")).output().unwrap().stdout).unwrap();
            std::fs::write(dir.join("firecrawl-local.sha256"), sum.unwrap_or(real.split(' ').next().unwrap())).unwrap();
            std::fs::write(dir.join(".complete"), "").unwrap();
        };
        let fake_mcp = |dir: &std::path::Path| {
            std::fs::create_dir_all(dir.join("node_modules/firecrawl-mcp/dist")).unwrap();
            std::fs::write(dir.join("node_modules/firecrawl-mcp/dist/index.js"), "").unwrap();
            std::fs::write(dir.join(".complete"), "").unwrap();
        };
        // A home holding the kit's image content, as sbx's build leaves it.
        let home_with_kit = |name: &str| {
            let h = d.path().join(name);
            let out = Command::new("cp").arg("-a").arg(kit.join("files/home/agent")).arg(&h).output().unwrap();
            assert!(out.status.success());
            std::fs::create_dir_all(h.join(".claude/kits-agent-context")).unwrap();
            std::fs::write(h.join(".claude/kits-agent-context/firecrawl.md"), "# firecrawl\n\nwhat sbx wrote\n").unwrap();
            h
        };
        let run = |h: &std::path::Path, with_cache: bool| {
            let mut c = Command::new("bash");
            c.arg(h.join(".sbxw/firecrawl/install.sh")).env("HOME", h).env_remove("SBXW_CACHE_FILL").env_remove("SBXW_CACHE");
            if with_cache {
                c.env("SBXW_CACHE", &cache);
            }
            let out = c.output().unwrap();
            assert!(out.status.success(), "never fails the start: {out:?}");
            let marker = h.join(".sbxw/startup").join(format!("{}.done", h.join(".sbxw/firecrawl/install.sh").display().to_string().replace('/', "_")));
            assert!(marker.is_file(), "the end marker");
            let context = std::fs::read_to_string(h.join(".claude/kits-agent-context/firecrawl.md")).unwrap();
            (String::from_utf8_lossy(&out.stdout).into_owned(), context)
        };
        let installed = |h: &std::path::Path| std::fs::read_to_string(h.join(".sbxw/firecrawl/lib/binary.version")).ok().map(|v| v.trim().to_string());

        // No cache at all, and no network used: a note, and nothing else.
        let h = home_with_kit("a");
        let (_, ctx) = run(&h, false);
        assert!(ctx.starts_with("# firecrawl\n\nwhat sbx wrote\n"), "{ctx}");
        assert_eq!(ctx.matches("Firecrawl is not installed").count(), 1, "{ctx}");
        // A cache not filled yet: the note says sbxw is preparing it, once.
        std::fs::create_dir_all(cache.join(&flavour)).unwrap();
        run(&h, true);
        let (_, ctx) = run(&h, true);
        assert_eq!(ctx.matches("Firecrawl is not installed yet: sbxw is preparing it; use WebFetch meanwhile.").count(), 1, "{ctx}");
        assert_eq!(installed(&h), None);

        // Filled with an older version only: that one is used meanwhile.
        fake_bin(&store.join("0.0.1"), None);
        fake_mcp(&store.join("mcp-0.0.1"));
        let (_, ctx) = run(&h, true);
        assert_eq!(installed(&h).as_deref(), Some("0.0.1"));
        assert!(!ctx.contains("not installed"), "the note goes: {ctx}");

        // The pinned version arrives: it replaces the older one.
        fake_bin(&store.join(&version), None);
        fake_mcp(&store.join(&mcp));
        run(&h, true);
        assert_eq!(installed(&h).as_deref(), Some(version.as_str()));
        assert!(!h.join(".sbxw/firecrawl/lib/0.0.1").exists() && !h.join(".sbxw/firecrawl/lib/mcp-0.0.1").exists());
        let (out, ctx) = run(&h, true);
        assert!(out.contains("already installed"), "idempotent: {out}");
        assert_eq!(ctx, "# firecrawl\n\nwhat sbx wrote\n");
        // The front script finds it and keeps the cloud off.
        let shown = Command::new("bash").arg(h.join(".local/bin/firecrawl-local")).arg("--version").env("HOME", &h).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&shown.stdout), "fake\n");
        let refused = Command::new("bash").arg(h.join(".local/bin/firecrawl-local")).arg("login").env("HOME", &h).output().unwrap();
        assert_eq!(refused.status.code(), Some(64), "no login under sbxw");

        // A copy that does not match the hash the filler recorded is refused.
        std::fs::remove_dir_all(store.join(&version)).unwrap();
        fake_bin(&store.join(&version), Some(&"0".repeat(64)));
        let h2 = home_with_kit("b");
        let (_, ctx) = run(&h2, true);
        assert_eq!(installed(&h2), None);
        assert!(ctx.contains("Firecrawl is not installed yet"), "{ctx}");
    }
}
