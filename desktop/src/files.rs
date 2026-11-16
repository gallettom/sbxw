//! The core's own file operations for features: downloads written by the
//! core, opening and revealing what it wrote, uploads of what the person
//! handed in. A feature asks for them as calls of method `Core` (inside a
//! `Confirm` where the rule says so; see `shell/features.rs`,
//! `feature_call`); it never names a path the core did not hand out or write
//! itself this run, and never computes a name or a verdict the core trusts
//! (`safe.rs`).
//!
//! | Call path | Body | Answer (to `on_answer`) | Direct? |
//! |---|---|---|---|
//! | `core:download` | `{get, name, to: downloads\|ask, after: nothing\|reveal\|open, tag}` | `{saved, name, bytes, verdict, kind, reason}` | only `to: downloads` with `after` nothing or reveal |
//! | `core:inspect` | `{path}` | `{saved, name, bytes, verdict, kind, reason}` | yes (read only, a file the core wrote) |
//! | `core:open` | `{path}` | `{opened}` | never: a `Confirm`; a `warn` file needs its ack box ticked; `refuse` never opens |
//! | `core:reveal` | `{path}` | `{revealed}` | yes (a file the core wrote) |
//! | `core:upload` | `{route, file, content_type?, tag}` | the daemon's answer | never: a `Confirm`; `file` handed in by the person this run |

use crate::client::Daemon;
use crate::safe;
use serde_json::{json, Value as J};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// The largest download and upload (the daemon's `UPLOAD_MAX`).
pub const MAX_BYTES: u64 = 128 << 20;

/// Files the person handed in this run (dropped, picked): the only ones an upload may read.
static HANDED: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);
/// Files the core wrote this run: size and SHA-256 when written.
static WRITTEN: Mutex<Option<HashMap<PathBuf, (u64, String)>>> = Mutex::new(None);
/// Uploads in progress or done, by tag: `{sent, total, state}`.
static UPLOADS: Mutex<Option<serde_json::Map<String, J>>> = Mutex::new(None);

/// The person handed these paths in (a drop, the file dialog).
pub fn hand_in(paths: &[PathBuf]) {
    let mut g = HANDED.lock().unwrap();
    let s = g.get_or_insert_with(HashSet::new);
    for p in paths {
        s.insert(std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()));
    }
}

pub fn was_handed(p: &Path) -> bool {
    let c = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    HANDED.lock().unwrap().as_ref().is_some_and(|s| s.contains(&c))
}

fn written(p: &Path) -> Option<(u64, String)> {
    WRITTEN.lock().unwrap().as_ref().and_then(|m| m.get(p).cloned())
}

/// `data.app.uploads`.
pub fn uploads() -> J {
    J::Object(UPLOADS.lock().unwrap().clone().unwrap_or_default())
}

fn set_upload(tag: &str, v: J) {
    UPLOADS.lock().unwrap().get_or_insert_with(Default::default).insert(tag.to_string(), v);
}

/// Where downloads go: the person's Downloads folder (`RUNE_DIRS_ROOT` moves it, for tests).
pub fn downloads_dir() -> Result<PathBuf, String> {
    rune::dirs::AppDirs::new("sbxw").and_then(|d| d.downloads().map(Path::to_path_buf)).ok_or_else(|| "no Downloads folder found".to_string())
}

fn sha(bytes: &[u8]) -> String {
    let h = Sha256::digest(bytes);
    h.iter().map(|b| format!("{b:02x}")).collect()
}

fn file_sha(p: &Path) -> std::io::Result<(u64, String)> {
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut n = 0u64;
    loop {
        let k = f.read(&mut buf)?;
        if k == 0 {
            break;
        }
        n += k as u64;
        h.update(&buf[..k]);
    }
    Ok((n, h.finalize().iter().map(|b| format!("{b:02x}")).collect()))
}

/// What a file the core wrote is: `{saved, name, bytes, verdict, kind, reason}`.
/// Refused: a path the core did not write, or a file changed since.
pub fn inspect(path: &Path) -> Result<J, String> {
    let (size, hash) = written(path).ok_or_else(|| format!("{}: not a file the app wrote", path.display()))?;
    let (n, h) = file_sha(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if (n, &h) != (size, &hash) {
        return Err(format!("{} changed since the app wrote it: not opened", path.display()));
    }
    let mut head = vec![0u8; 512];
    let k = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
    head.truncate(k);
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
    let (v, kind, reason) = safe::verdict(&name, &head);
    Ok(json!({ "saved": path, "name": name, "bytes": n, "verdict": v.name(), "kind": kind, "reason": reason }))
}

/// Downloads `get` (a daemon GET route) into `dir` under the sanitised
/// `name`: never over a file, written to a temporary file then renamed.
pub fn download(d: &dyn Daemon, get: &str, dir: &Path, name: &str) -> Result<(u16, J), String> {
    if !get.starts_with("/api/") {
        return Err(format!("{get}: not a daemon route"));
    }
    let (code, bytes, _ct) = d.request_raw(get, MAX_BYTES, Duration::from_secs(600)).map_err(|e| format!("{e:#}"))?;
    if !(200..300).contains(&code) {
        let v: J = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({ "error": String::from_utf8_lossy(&bytes).trim().to_string() }));
        return Ok((code, v));
    }
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!("the file is more than {} MB: not downloaded", MAX_BYTES >> 20));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = write_new(dir, name, &bytes)?;
    log::info!("downloaded {get} to {} ({} bytes)", path.display(), bytes.len());
    Ok((200, inspect(&path)?))
}

/// Writes `bytes` as a new file named after `name` in `dir` (safe.rs rules), quarantined, recorded.
pub fn write_new(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let (path, reserved) = safe::create_in(dir, name).map_err(|e| format!("{}: {e}", dir.display()))?;
    drop(reserved);
    let tmp = dir.join(format!(".{}.sbxw-part", safe::file_name(name)));
    let write = || -> std::io::Result<()> {
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o644);
        }
        let mut f = o.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, &path)
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_file(&path);
        return Err(format!("{}: {e}", path.display()));
    }
    safe::quarantine(&path);
    WRITTEN.lock().unwrap().get_or_insert_with(HashMap::new).insert(path.clone(), (bytes.len() as u64, sha(bytes)));
    Ok(path)
}

/// Opens a file the core wrote with the system's default app, if its
/// verdict lets it (`acked`: the person ticked the dialog's box).
pub fn open(path: &Path, acked: bool) -> Result<(u16, J), String> {
    let v = inspect(path)?;
    match v["verdict"].as_str() {
        Some("plain") => {}
        Some("warn") if acked => {}
        Some("warn") => return Err(format!("{}: {} ({}); opening it needs the box ticked", path.display(), v["kind"].as_str().unwrap_or(""), v["reason"].as_str().unwrap_or(""))),
        _ => return Err(format!("{}: {}", path.display(), v["reason"].as_str().unwrap_or("not opened"))),
    }
    if crate::shell::headless() {
        log::info!("headless: would open {}", path.display());
        return Ok((200, json!({ "opened": path, "headless": true })));
    }
    spawn_open(path, false)?;
    Ok((200, json!({ "opened": path })))
}

/// Shows a file the core wrote in the file manager.
pub fn reveal(path: &Path) -> Result<(u16, J), String> {
    if written(path).is_none() {
        return Err(format!("{}: not a file the app wrote", path.display()));
    }
    if crate::shell::headless() {
        log::info!("headless: would reveal {}", path.display());
        return Ok((200, json!({ "revealed": path, "headless": true })));
    }
    spawn_open(path, true)?;
    Ok((200, json!({ "revealed": path })))
}

fn spawn_open(path: &Path, reveal: bool) -> Result<(), String> {
    use std::process::Command;
    let mut c = if cfg!(target_os = "macos") {
        let mut c = Command::new("/usr/bin/open");
        if reveal {
            c.arg("-R");
        }
        c.arg("--").arg(path);
        c
    } else if cfg!(windows) {
        let mut c = Command::new("explorer.exe");
        if reveal {
            c.arg(format!("/select,{}", path.display()));
        } else {
            c.arg(path);
        }
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(if reveal { path.parent().unwrap_or(path) } else { path });
        c
    };
    c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    c.spawn().map(|_| ()).map_err(|e| format!("could not open {}: {e}", path.display()))
}

/// Counts what an upload has read, for its progress.
struct Counting<R> {
    inner: R,
    sent: u64,
    total: u64,
    tag: String,
    last: std::time::Instant,
    wake: Option<crate::client::Wake>,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.sent += n as u64;
        if self.last.elapsed() >= Duration::from_millis(100) || n == 0 {
            self.last = std::time::Instant::now();
            set_upload(&self.tag, json!({ "sent": self.sent, "total": self.total, "state": "sending" }));
            if let Some(w) = &self.wake {
                w();
            }
        }
        Ok(n)
    }
}

/// Uploads a file the person handed in to `route` (a daemon POST), streamed
/// with its length; progress in `data.app.uploads[tag]`.
pub fn upload(d: &dyn Daemon, route: &str, file: &Path, content_type: &str, tag: &str, wake: Option<crate::client::Wake>) -> Result<(u16, J), String> {
    if !route.starts_with("/api/") {
        return Err(format!("{route}: not a daemon route"));
    }
    if !was_handed(file) {
        return Err(format!("{}: not a file you handed in: refused", file.display()));
    }
    let meta = std::fs::metadata(file).map_err(|e| format!("{}: {e}", file.display()))?;
    if !meta.is_file() {
        return Err(format!("{}: not a file", file.display()));
    }
    let total = meta.len();
    if total > MAX_BYTES {
        return Err(format!("{} is {} MB, more than {} MB", file.display(), total >> 20, MAX_BYTES >> 20));
    }
    let f = std::fs::File::open(file).map_err(|e| format!("{}: {e}", file.display()))?;
    set_upload(tag, json!({ "sent": 0, "total": total, "state": "sending" }));
    let body = Counting { inner: f.take(total), sent: 0, total, tag: tag.to_string(), last: std::time::Instant::now(), wake };
    let r = d.send_body(route, content_type, Box::new(body), total, Duration::from_secs(900)).map_err(|e| format!("{e:#}"));
    let state = match &r {
        Ok((c, _)) if (200..300).contains(c) => "done",
        _ => "failed",
    };
    set_upload(tag, json!({ "sent": if state == "done" { total } else { 0 }, "total": total, "state": state }));
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_written_files_open_and_only_handed_files_upload() {
        let dir = std::env::temp_dir().join(format!("sbxw-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Not written by the core: neither inspected, opened nor revealed.
        let other = dir.join("other.md");
        std::fs::write(&other, "x").unwrap();
        assert!(inspect(&other).is_err() && open(&other, true).is_err() && reveal(&other).is_err());
        // Written by the core: inspected; a script named as a document is refused.
        let p = write_new(&dir, "../notes.md", b"# hello").unwrap();
        assert_eq!(p, dir.join("notes.md"));
        let v = inspect(&p).unwrap();
        assert_eq!((v["verdict"].as_str(), v["bytes"].as_u64()), (Some("plain"), Some(7)));
        let s = write_new(&dir, "report.pdf", b"#!/bin/sh\nrm -rf ~").unwrap();
        assert!(open(&s, true).unwrap_err().contains("script"));
        let h = write_new(&dir, "page.html", b"<script>").unwrap();
        assert!(open(&h, false).is_err(), "a warn file needs the box");
        // Changed on disk since: refused.
        std::fs::write(&p, "# changed").unwrap();
        assert!(inspect(&p).unwrap_err().contains("changed"));
        // A second write never replaces: (1).
        assert_eq!(write_new(&dir, "notes.md", b"again").unwrap(), dir.join("notes (1).md"));
        // Uploads: only what was handed in.
        assert!(!was_handed(&other));
        hand_in(&[other.clone()]);
        assert!(was_handed(&other));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A daemon answering one raw route.
    struct One(u16, Vec<u8>);
    impl Daemon for One {
        fn describe(&self) -> String {
            "one".into()
        }
        fn sandboxes(&self) -> anyhow::Result<Vec<crate::client::Sandbox>> {
            Ok(vec![])
        }
        fn stop_sandbox(&self, _: &str) -> anyhow::Result<()> {
            Ok(())
        }
        fn open_terminal(&self, _: &crate::client::TerminalRequest, _: crate::client::Wake) -> anyhow::Result<crate::client::TerminalConn> {
            anyhow::bail!("no")
        }
        fn subscribe(&self, _: &mut dyn FnMut(crate::client::BusEvent) -> bool) -> anyhow::Result<()> {
            Ok(())
        }
        fn request_raw(&self, _: &str, max: u64, _: Duration) -> anyhow::Result<(u16, Vec<u8>, String)> {
            let mut b = self.1.clone();
            b.truncate(max as usize + 1);
            Ok((self.0, b, "application/octet-stream".into()))
        }
    }

    #[test]
    fn a_download_gets_a_safe_new_name_and_its_verdict() {
        let dir = std::env::temp_dir().join(format!("sbxw-dl-{}", std::process::id()));
        let (code, v) = download(&One(200, b"%PDF-1.7".to_vec()), "/api/sandboxes/a/fs/file?path=x", &dir, "../../evil\u{202E}fdp.exe").unwrap();
        assert_eq!(code, 200);
        assert_eq!(v["name"], "evilfdp.exe");
        assert_eq!(v["verdict"], "refuse");
        let (_, v2) = download(&One(200, b"%PDF-1.7".to_vec()), "/api/x", &dir, "CON.pdf").unwrap();
        assert_eq!((v2["name"].as_str(), v2["verdict"].as_str()), (Some("_CON.pdf"), Some("plain")));
        let (_, v3) = download(&One(200, b"%PDF-1.7".to_vec()), "/api/x", &dir, "_CON.pdf").unwrap();
        assert_eq!(v3["name"], "_CON (1).pdf", "never over the first");
        // The daemon's refusal is handed back, nothing written.
        let (code, e) = download(&One(404, br#"{"error":"no such file"}"#.to_vec()), "/api/x", &dir, "gone.md").unwrap();
        assert_eq!((code, e["error"].as_str()), (404, Some("no such file")));
        assert!(!dir.join("gone.md").exists());
        assert!(download(&One(200, vec![]), "http://elsewhere/x", &dir, "a").is_err(), "only daemon routes");
        std::fs::remove_dir_all(&dir).ok();
    }
}
