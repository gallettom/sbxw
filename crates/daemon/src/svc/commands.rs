//! The skills and slash commands a sandbox's Claude session knows, for the
//! desktop input's autocompletion (`GET /api/sandboxes/:name/commands`).
//!
//! It never starts a sandbox. `sbx exec` starts a stopped one before running
//! its command (`Sbx::start` is an `exec` of `true`), so a sandbox is read
//! from inside only when `sbx ls` says it is running; an `exec` into a
//! running sandbox runs one `node` process as the agent user and changes
//! nothing else. A stopped sandbox, or one whose read failed or timed out,
//! is answered from its workspace on the host (local disk or SSH, through
//! [`Host::fs`]), marked `complete: false` with the reason.
//!
//! Only names and one-line descriptions leave the daemon
//! ([`sbxw_core::commands`]); nothing but counts is logged.

use crate::app::App;
use crate::host::{Host, Kind as FsKind};
use anyhow::{bail, Context, Result};
use sbxw_core::commands::{self, Found, Kind, Origin, HEAD_BYTES, MAX_COMMANDS};
use sbxw_core::SandboxRef;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Longest a read (in the sandbox, or of the workspace on the host) may take.
const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Directory entries looked at per folder, whatever they turn out to be.
const MAX_ENTRIES: usize = 2000;
/// A file up to this size is read whole and cut; a larger one is read with
/// the host's `head`, so a huge file costs no more than a small one.
const READ_WHOLE: u64 = 32 * 1024;

/// Lists the project's and home's skills and commands from inside the
/// sandbox, printing `{items:[{name,kind,origin,head}]}` where `head` is the
/// file's frontmatter, from its first `HEAD` bytes. A symlink is followed
/// only when it resolves inside the folder being listed; the workspace's
/// folders must resolve inside the workspace.
const SANDBOX_SCRIPT: &str = r#"
const fs = require('fs'), path = require('path');
const ws = (process.argv[2] || '').replace(/^=/, '');
const MAX = Number((process.argv[3] || '').replace(/^=/, '')), HEAD = Number((process.argv[4] || '').replace(/^=/, ''));
const MAX_ENTRIES = Number((process.argv[5] || '').replace(/^=/, ''));
const NAME = /^[a-z0-9][a-z0-9:_-]{0,63}$/;
const items = [];
const real = p => { try { return fs.realpathSync(p); } catch { return null; } };
const under = (root, p) => { const r = real(p); return r && (r === root || r.startsWith(root + '/')) ? r : null; };
function head(p) {
  let fd;
  try {
    fd = fs.openSync(p, 'r');
    const b = Buffer.alloc(HEAD), n = fs.readSync(fd, b, 0, HEAD, 0);
    const m = b.subarray(0, n).toString('utf8').match(/^---\r?\n[\s\S]*?\n---/);
    return m ? m[0] : '';
  } catch { return ''; } finally { if (fd !== undefined) try { fs.closeSync(fd); } catch {} }
}
function isFile(p) { try { return fs.statSync(p).isFile(); } catch { return false; } }
function isDir(p) { try { return fs.statSync(p).isDirectory(); } catch { return false; } }
function scan(base, origin, within) {
  for (const [sub, kind] of [['skills', 'skill'], ['commands', 'command']]) {
    const root = real(path.join(base, sub));
    if (!root || !isDir(root) || (within && !(root === within || root.startsWith(within + '/')))) continue;
    let names = [];
    try { names = fs.readdirSync(root).sort().slice(0, MAX_ENTRIES); } catch { continue; }
    for (const e of names) {
      if (items.length >= MAX) return;
      if (kind === 'skill') {
        const dir = under(root, path.join(root, e));
        if (!NAME.test(e) || !dir || dir === root || !isDir(dir)) continue;
        const md = under(root, path.join(dir, 'SKILL.md'));
        if (!md || !isFile(md)) continue;
        items.push({ name: e, kind, origin, head: head(md) });
      } else {
        const name = e.endsWith('.md') ? e.slice(0, -3) : '';
        const f = under(root, path.join(root, e));
        if (!NAME.test(name) || !f || !isFile(f)) continue;
        items.push({ name, kind, origin, head: head(f) });
      }
    }
  }
}
const wsReal = ws ? real(ws) : null;
if (wsReal) scan(path.join(wsReal, '.claude'), 'workspace', wsReal);
scan('/home/agent/.claude', 'home', null);
process.stdout.write(JSON.stringify({ items }) + '\n');
"#;

#[derive(Deserialize)]
struct Listed {
    name: String,
    kind: Kind,
    origin: Origin,
    #[serde(default)]
    head: String,
}

/// The script's output, kept to entries that fit [`Listed`], whose origin
/// is one the script may claim, at most [`MAX_COMMANDS`] of them.
fn parse_listing(raw: &str) -> Result<Vec<Found>> {
    #[derive(Deserialize)]
    struct Listing {
        items: Vec<Value>,
    }
    let l: Listing = serde_json::from_str(raw.trim()).context("reading the sandbox's command list")?;
    Ok(l.items
        .into_iter()
        .filter_map(|v| serde_json::from_value::<Listed>(v).ok())
        .filter(|l| l.origin != Origin::BuiltIn)
        .take(MAX_COMMANDS)
        .map(|l| Found { name: l.name, kind: l.kind, origin: l.origin, head: cut(l.head.as_bytes()) })
        .collect())
}

/// The first [`HEAD_BYTES`] of a file, as text.
fn cut(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(HEAD_BYTES)]).into_owned()
}

/// `p` resolved, if it stays inside `root`.
async fn inside(host: &Host, root: &Path, p: &Path) -> Option<PathBuf> {
    let r = host.fs.canonicalize(p).await.ok()?;
    r.starts_with(root).then_some(r)
}

async fn kind_of(host: &Host, p: &Path) -> Option<FsKind> {
    host.fs.stat(p).await.ok().flatten().map(|m| m.kind)
}

/// The first [`HEAD_BYTES`] of a regular file on the host.
async fn host_head(host: &Host, p: &Path) -> String {
    let Ok(Some(meta)) = host.fs.stat(p).await else { return String::new() };
    if meta.kind != FsKind::File {
        return String::new();
    }
    if meta.len <= READ_WHOLE {
        return host.fs.read(p, READ_WHOLE).await.map(|b| cut(&b)).unwrap_or_default();
    }
    let cmd = sbxw_sbx::host::HostCmd::new("head", vec!["-c".into(), HEAD_BYTES.to_string(), "--".into(), p.to_string_lossy().into_owned()]);
    match host.exec.exec(cmd).await {
        Ok(out) if out.success() => cut(&out.stdout),
        _ => String::new(),
    }
}

/// The workspace's `.claude/skills/*/SKILL.md` and `.claude/commands/*.md`,
/// read on the host. Each folder must resolve inside the workspace and each
/// entry inside its folder: a symlink pointing anywhere else is skipped.
async fn workspace_part(host: &Host, ws: &Path) -> Result<Vec<Found>> {
    let ws = host.fs.canonicalize(ws).await.with_context(|| format!("{}: workspace not found on the host", ws.display()))?;
    let mut out = Vec::new();
    for (sub, kind) in [("skills", Kind::Skill), ("commands", Kind::Command)] {
        let Some(root) = inside(host, &ws, &ws.join(".claude").join(sub)).await else { continue };
        if kind_of(host, &root).await != Some(FsKind::Dir) {
            continue;
        }
        let Ok(entries) = host.fs.read_dir(&root).await else { continue };
        for e in entries.into_iter().take(MAX_ENTRIES) {
            if out.len() >= MAX_COMMANDS {
                return Ok(out);
            }
            let (name, file) = match kind {
                Kind::Skill => {
                    if !commands::valid_name(&e.name) {
                        continue;
                    }
                    let Some(dir) = inside(host, &root, &root.join(&e.name)).await else { continue };
                    if dir == root || kind_of(host, &dir).await != Some(FsKind::Dir) {
                        continue;
                    }
                    let Some(md) = inside(host, &root, &dir.join("SKILL.md")).await else { continue };
                    (e.name, md)
                }
                Kind::Command => {
                    let Some(name) = e.name.strip_suffix(".md").filter(|n| commands::valid_name(n)).map(str::to_string) else { continue };
                    let Some(f) = inside(host, &root, &root.join(&e.name)).await else { continue };
                    (name, f)
                }
            };
            if kind_of(host, &file).await != Some(FsKind::File) {
                continue;
            }
            out.push(Found { name, kind, origin: Origin::Workspace, head: host_head(host, &file).await });
        }
    }
    Ok(out)
}

impl App {
    /// Run [`SANDBOX_SCRIPT`] in a running `sb`, within [`READ_TIMEOUT`].
    async fn commands_in_sandbox(&self, sb: &SandboxRef, ws: Option<&Path>) -> Result<Vec<Found>> {
        let ws = ws.map(|w| w.to_string_lossy().into_owned()).unwrap_or_default();
        // `sbx exec` refuses an empty element: values go in behind `=`.
        let args = [ws, MAX_COMMANDS.to_string(), HEAD_BYTES.to_string(), MAX_ENTRIES.to_string()].map(|a| format!("={a}"));
        let mut argv = vec!["node", "-"];
        argv.extend(args.iter().map(String::as_str));
        let sbx = self.sbx_for(sb).await?;
        let run = sbx.exec(&sb.name, &argv, Some(SANDBOX_SCRIPT.as_bytes().to_vec()));
        let out = tokio::time::timeout(READ_TIMEOUT, run).await.map_err(|_| anyhow::anyhow!("timed out"))??;
        if !out.success() {
            bail!("the listing script failed");
        }
        parse_listing(&out.stdout_str())
    }

    /// The host-readable part, within [`READ_TIMEOUT`]; `Err` is the reason
    /// it is empty.
    async fn commands_on_host(&self, sb: &SandboxRef, ws: Option<&Path>) -> Result<Vec<Found>, &'static str> {
        let Some(ws) = ws else { return Err("it has no recorded workspace") };
        let host = self.host_for(sb).await.map_err(|_| "its host cannot be reached")?;
        match tokio::time::timeout(READ_TIMEOUT, workspace_part(&host, ws)).await {
            Ok(Ok(found)) => Ok(found),
            Ok(Err(_)) => Err("its workspace cannot be read on the host"),
            Err(_) => Err("reading its workspace on the host took too long"),
        }
    }

    /// What `/` can complete to in `sb`'s Claude session:
    /// `{commands:[{name,description,kind,origin}], complete, reason?}`.
    pub async fn sandbox_commands(&self, sb: &SandboxRef) -> Result<Value> {
        let ws = self.store.workspace(sb);
        let running = match self.sbx_for(sb).await {
            Ok(sbx) => sbx.lookup(&sb.name).await.is_some_and(|i| i.is_running()),
            Err(_) => false,
        };
        let (found, reason) = if running {
            match self.commands_in_sandbox(sb, ws.as_deref()).await {
                Ok(found) => (found, None),
                Err(_) => match self.commands_on_host(sb, ws.as_deref()).await {
                    Ok(found) => (found, Some("The sandbox did not answer: only the project's own skills and commands are listed, with Claude Code's built-in ones.".to_string())),
                    Err(why) => (vec![], Some(format!("The sandbox did not answer, and {why}: only Claude Code's built-in commands are listed."))),
                },
            }
        } else {
            match self.commands_on_host(sb, ws.as_deref()).await {
                Ok(found) => (found, Some("The sandbox is not running: only the project's own skills and commands are listed, with Claude Code's built-in ones.".to_string())),
                Err(why) => (vec![], Some(format!("The sandbox is not running, and {why}: only Claude Code's built-in commands are listed."))),
            }
        };
        let list = commands::merge(found, true);
        tracing::debug!(count = list.len(), complete = reason.is_none(), "commands listed");
        let mut v = json!({ "commands": list, "complete": reason.is_none() });
        if let Some(r) = reason {
            v["reason"] = Value::String(r);
        }
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::{app_with, hosts, ls, sbx_answer};
    use sbxw_core::SandboxRef;
    use sbxw_sbx::runner::Output;
    use std::path::Path;

    fn write(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    /// A workspace with one skill and one command, and the hostile cases.
    fn workspace(root: &Path) -> std::path::PathBuf {
        let ws = root.join("ws");
        write(&ws.join(".claude/skills/mine/SKILL.md"), "---\nname: mine\ndescription: My skill\n---\nbody");
        write(&ws.join(".claude/commands/ship.md"), "---\ndescription: Ship\u{202e}\u{7}it\tnow\n---\n");
        write(&ws.join(".claude/commands/Bad Name.md"), "---\ndescription: x\n---\n");
        write(&ws.join(".claude/commands/notes.txt"), "x");
        let long = format!("---\ndescription: {}\n---\n", "w".repeat(300));
        write(&ws.join(".claude/skills/wordy/SKILL.md"), &long);
        // A large file: its frontmatter is still read, from its head.
        write(&ws.join(".claude/commands/big.md"), &format!("---\ndescription: Big\n---\n{}", "x".repeat(100_000)));
        // Links out of the listed folders are not followed; a link inside is.
        write(&root.join("outside/SKILL.md"), "---\ndescription: SECRET\n---\n");
        write(&root.join("outside.md"), "---\ndescription: SECRET\n---\n");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("outside"), ws.join(".claude/skills/escape")).unwrap();
            std::os::unix::fs::symlink(root.join("outside.md"), ws.join(".claude/commands/escape2.md")).unwrap();
            std::os::unix::fs::symlink(root.join("outside/SKILL.md"), ws.join(".claude/skills/mine2")).unwrap();
            std::fs::create_dir_all(ws.join(".claude/skills/linked")).unwrap();
            std::os::unix::fs::symlink(root.join("outside.md"), ws.join(".claude/skills/linked/SKILL.md")).unwrap();
            std::os::unix::fs::symlink(ws.join(".claude/commands/ship.md"), ws.join(".claude/commands/alias.md")).unwrap();
        }
        ws
    }

    fn names(v: &serde_json::Value) -> Vec<String> {
        v["commands"].as_array().unwrap().iter().map(|c| format!("{}:{}:{}", c["name"].as_str().unwrap(), c["kind"].as_str().unwrap(), c["origin"].as_str().unwrap())).collect()
    }

    #[tokio::test]
    async fn a_stopped_sandbox_gives_its_workspace_and_is_never_started() {
        let (d, app, fake) = app_with(|a: &[String]| match a.first().map(String::as_str) {
            Some("ls") => ls(&[("myapp", "stopped")]),
            _ => sbx_answer(a),
        });
        let ws = workspace(d.path());
        let sb = SandboxRef::parse("myapp").unwrap();
        app.store.update(&sb, |r| r.workspace = Some(ws.clone())).unwrap();
        let v = app.sandbox_commands(&sb).await.unwrap();

        assert_eq!(v["complete"], false);
        assert!(v["reason"].as_str().unwrap().contains("not running"), "{v}");
        let got = names(&v);
        for want in ["mine:skill:workspace", "ship:command:workspace", "wordy:skill:workspace", "big:command:workspace", "alias:command:workspace", "clear:command:built-in"] {
            assert!(got.contains(&want.to_string()), "{want} in {got:?}");
        }
        let text = v.to_string();
        assert!(!text.contains("SECRET") && !text.contains("escape") && !text.contains("linked") && !text.contains("mine2"), "no link out: {text}");
        assert!(!text.contains("Bad") && !text.contains("notes"), "{text}");
        let by = |n: &str| v["commands"].as_array().unwrap().iter().find(|c| c["name"] == n).unwrap()["description"].as_str().unwrap().to_string();
        assert_eq!(by("ship"), "Shipit now", "one line, the bidi override gone");
        assert_eq!(by("big"), "Big");
        assert_eq!(by("wordy").chars().count(), sbxw_core::commands::MAX_DESCRIPTION);
        assert!(text.len() < 8000, "no file content beyond descriptions");

        let calls = fake.calls.lock().unwrap().clone();
        assert!(calls.iter().all(|c| c.first().map(String::as_str) != Some("exec")), "nothing ran in the sandbox, so nothing started it: {calls:?}");
    }

    #[tokio::test]
    async fn the_host_read_is_the_same_over_ssh() {
        for (label, host) in hosts() {
            let d = tempfile::tempdir().unwrap();
            let ws = workspace(d.path());
            let mut got: Vec<String> = super::workspace_part(&host, &ws).await.unwrap().into_iter().map(|f| f.name).collect();
            got.sort();
            assert_eq!(got, ["alias", "big", "mine", "ship", "wordy"], "{label}");
        }
    }

    #[tokio::test]
    async fn a_running_sandbox_gives_workspace_and_home() {
        let (d, app, fake) = app_with(|a: &[String]| match a.first().map(String::as_str) {
            Some("ls") => ls(&[("myapp", "running")]),
            Some("exec") if a.iter().any(|x| x == "node") => Output::ok(
                &serde_json::json!({ "items": [
                    { "name": "mine", "kind": "skill", "origin": "workspace", "head": "---\ndescription: Inside\n---" },
                    { "name": "handoff", "kind": "command", "origin": "home", "head": "---\ndescription: Hand\u{200b}off\n---" },
                    { "name": "mine", "kind": "command", "origin": "home", "head": "" },
                    { "name": "../evil", "kind": "skill", "origin": "home", "head": "" },
                    { "name": "fake", "kind": "command", "origin": "built-in", "head": "" },
                    { "name": "x", "kind": "<b>", "origin": "home" },
                    "not an object",
                ] })
                .to_string(),
            ),
            _ => sbx_answer(a),
        });
        let ws = workspace(d.path());
        let sb = SandboxRef::parse("myapp").unwrap();
        app.store.update(&sb, |r| r.workspace = Some(ws.clone())).unwrap();
        let v = app.sandbox_commands(&sb).await.unwrap();
        assert_eq!(v["complete"], true, "{v}");
        assert!(v.get("reason").is_none());
        assert_eq!(names(&v), ["clear:command:built-in", "compact:command:built-in", "context:command:built-in", "handoff:command:home", "mine:skill:workspace", "model:command:built-in"]);
        assert_eq!(v["commands"][3]["description"], "Handoff");
        let exec = fake.calls.lock().unwrap().iter().filter(|c| c.first().map(String::as_str) == Some("exec")).count();
        assert_eq!(exec, 1, "one read");
    }

    #[tokio::test]
    async fn a_failed_read_falls_back_to_the_workspace() {
        for answer in [Output::fail(1, "boom"), Output::ok("<html>")] {
            let (d, app, _) = app_with(move |a: &[String]| match a.first().map(String::as_str) {
                Some("ls") => ls(&[("myapp", "running")]),
                Some("exec") => answer.clone(),
                _ => sbx_answer(a),
            });
            let ws = workspace(d.path());
            let sb = SandboxRef::parse("myapp").unwrap();
            app.store.update(&sb, |r| r.workspace = Some(ws.clone())).unwrap();
            let v = app.sandbox_commands(&sb).await.unwrap();
            assert_eq!(v["complete"], false);
            assert!(v["reason"].as_str().unwrap().contains("did not answer"), "{v}");
            assert!(names(&v).contains(&"mine:skill:workspace".to_string()));
        }
    }

    #[tokio::test]
    async fn no_workspace_still_answers() {
        let (_d, app, _) = app_with(sbx_answer);
        let v = app.sandbox_commands(&SandboxRef::parse("ghost").unwrap()).await.unwrap();
        assert_eq!(v["complete"], false);
        assert!(v["reason"].as_str().unwrap().contains("no recorded workspace"), "{v}");
        assert_eq!(v["commands"].as_array().unwrap().len(), sbxw_core::commands::BUILT_IN.len());
    }

    #[tokio::test]
    async fn the_count_is_capped() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().join("ws");
        for i in 0..400 {
            write(&ws.join(format!(".claude/commands/c{i:03}.md")), "---\ndescription: d\n---\n");
        }
        let host = crate::host::Host::local();
        assert_eq!(super::workspace_part(&host, &ws).await.unwrap().len(), sbxw_core::commands::MAX_COMMANDS);
        let many: Vec<_> = (0..400).map(|i| serde_json::json!({ "name": format!("s{i}"), "kind": "skill", "origin": "home", "head": "" })).collect();
        let raw = serde_json::json!({ "items": many }).to_string();
        assert_eq!(super::parse_listing(&raw).unwrap().len(), sbxw_core::commands::MAX_COMMANDS);
    }

    /// On the UI listener only, behind its token; the agent listener has no such route.
    #[tokio::test]
    async fn the_route_needs_the_ui_token() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;
        let (_d, app, _) = app_with(sbx_answer);
        let req = |auth: bool| {
            let b = Request::builder().uri("/api/sandboxes/myapp/commands").header("host", "127.0.0.1:7681");
            let b = if auth { b } else { b.header("authorization", "Bearer sbxw_nope") };
            b.body(Body::empty()).unwrap()
        };
        let res = crate::testing::signed_in(app.clone()).oneshot(req(true)).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v: serde_json::Value = serde_json::from_slice(&axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap()).unwrap();
        assert_eq!((v["ok"].clone(), v["complete"].clone()), (serde_json::json!(true), serde_json::json!(false)));
        let res = crate::testing::signed_in(app.clone()).oneshot(req(false)).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        let res = crate::http::agent_router(app.clone()).oneshot(req(true)).await.unwrap();
        assert_ne!(res.status(), StatusCode::OK);
        let bad = Request::builder().uri("/api/sandboxes/..%2Fx/commands").header("host", "127.0.0.1:7681").body(Body::empty()).unwrap();
        assert_ne!(crate::testing::signed_in(app).oneshot(bad).await.unwrap().status(), StatusCode::OK);
    }

    /// The script itself, run by the host's node when there is one: links
    /// out are skipped, names checked, only frontmatter handed back.
    #[test]
    fn the_sandbox_script_lists_what_the_host_read_does() {
        let Ok(node) = which_node() else { return };
        let d = tempfile::tempdir().unwrap();
        let ws = workspace(d.path());
        use std::io::Write;
        let mut child = std::process::Command::new(node)
            .arg("-")
            .args([format!("={}", ws.display()), "=300".into(), "=4096".into(), "=2000".into()])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(super::SANDBOX_SCRIPT.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let got = super::parse_listing(&String::from_utf8_lossy(&out.stdout)).unwrap();
        let mut ws_names: Vec<&str> = got.iter().filter(|f| f.origin == sbxw_core::commands::Origin::Workspace).map(|f| f.name.as_str()).collect();
        ws_names.sort();
        assert_eq!(ws_names, ["alias", "big", "mine", "ship", "wordy"]);
        assert!(got.iter().all(|f| !f.head.contains("body") && !f.head.contains("xxxx")), "frontmatter only");
    }

    fn which_node() -> Result<std::path::PathBuf, ()> {
        let path = std::env::var_os("PATH").ok_or(())?;
        std::env::split_paths(&path).map(|p| p.join("node")).find(|p| p.is_file()).ok_or(())
    }
}
