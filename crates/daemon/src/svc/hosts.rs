//! The web UI's remote hosts (EXPERIMENTAL): `~/.sbxw/hosts.toml` listed with
//! each host's link, and edited — add, edit, remove, test — the way
//! `sbxw host` does it (crates/cli/src/cmd/host.rs), reimplemented here
//! because the CLI asks its questions on a terminal and this asks them in a
//! dialog.
//!
//! Host keys are checked strictly and trusted once, on a fingerprint the
//! person has seen: adding a host is two calls. [`App::host_key`] fetches the
//! key (`ssh-keyscan`) and shows its fingerprint (`ssh-keygen -lf`); the add
//! (or an edit that changes where the host is) then passes that fingerprint
//! back, the key is fetched again, and it is written to known_hosts only if
//! it is still the one shown. Nothing is kept between the two calls: a key
//! that changed in between is refused, not trusted.
//!
//! Every write goes through `sbxw_core::remote_hosts` (rules and rendering)
//! and is followed by [`App::reload_remotes`], so a changed host's link is
//! restarted from what the file says now.

use crate::app::App;
use crate::host::link::LinkState;
use anyhow::{anyhow, bail, Context, Result};
use sbxw_core::names::HostName;
use sbxw_core::remote_hosts::{RemoteHost, RemoteHosts, Target};
use sbxw_sbx::host::HostCmd;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

/// A latency older than this is timed again, in the background, on the next look.
const LATENCY_TTL: Duration = Duration::from_secs(30);
/// How long a timing round trip may take before it counts as none.
const LATENCY_TIMEOUT: Duration = Duration::from_secs(5);
/// `ssh-keyscan -T`, seconds.
const KEYSCAN_TIMEOUT: &str = "10";
/// A clone of a big repo over a slow link takes a while.
const CLONE_TIMEOUT: Duration = Duration::from_secs(600);
/// Where a git URL given in the create dialog is cloned on the host.
const CLONE_DIR: &str = "sbxw-repos";

/// One read-modify-write of hosts.toml at a time: two dialogs saving at once
/// must not drop each other's host.
static WRITE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// One host as the sidebar and the Hosts tab show it.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct HostView {
    pub name: String,
    #[serde(flatten)]
    pub form: HostForm,
    /// What ssh connects to (alias or address), for display.
    pub address: String,
    /// `up`, `down` (it was up, see `down_since`) or `never` (not connected yet).
    pub link: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub down_since: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    /// Sandboxes sbxw holds a record of on this host.
    pub sandboxes: usize,
}

/// A host's settings as the dialog sends them: all of them, every time (an
/// edit is the whole form, not the CLI's "only the flags given").
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct HostForm {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_port: Option<u16>,
    #[serde(default)]
    pub trust_secrets: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sbx_path: Option<String>,
}

fn blank_to_none(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

impl HostForm {
    /// The host this form describes, checked by the same rules as hosts.toml.
    pub fn to_host(&self, name: &str) -> Result<RemoteHost> {
        RemoteHost::new(
            name,
            blank_to_none(self.ssh.clone()),
            blank_to_none(self.host.clone()),
            blank_to_none(self.user.clone()),
            self.port,
            blank_to_none(self.key.clone()),
            self.agent_port,
            self.trust_secrets,
        )?
        .with_sbx_path(blank_to_none(self.sbx_path.clone()))
    }

    fn of(h: &RemoteHost) -> Self {
        let (ssh, host, user, port, key) = match &h.target {
            Target::Alias(a) => (Some(a.clone()), None, None, None, None),
            Target::Direct { host, user, port, key } => (None, Some(host.clone()), user.clone(), *port, key.clone()),
        };
        Self { ssh, host, user, port, key, agent_port: Some(h.agent_port), trust_secrets: h.trust_secrets, sbx_path: h.sbx_path.clone() }
    }
}

/// A host's SSH key, as the add dialog asks about it.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct HostKey {
    /// How known_hosts spells the host: `host`, or `[host]:port`.
    pub known_as: String,
    /// Already in known_hosts: nothing to confirm.
    pub known: bool,
    /// `ssh-keygen -lf` lines, sorted (keyscan's order varies); empty when known.
    pub fingerprints: Vec<String>,
}

/// What `test` found.
#[derive(Serialize, Debug, Clone)]
pub struct HostTest {
    pub version: String,
    pub latency_ms: u64,
}

/// How known_hosts spells a host: `host`, or `[host]:port` off port 22.
fn known_name(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    }
}

/// The text of a command's stderr, for an error.
fn err_text(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).trim().to_string()
}

/// Scanned keys: the known_hosts lines and their fingerprints, both sorted.
struct Scanned {
    lines: Vec<String>,
    fingerprints: Vec<String>,
}

fn duration_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

impl App {
    fn home_dir(&self) -> PathBuf {
        self.store.paths.root.parent().unwrap_or(Path::new("/")).to_path_buf()
    }

    fn known_hosts(&self) -> PathBuf {
        self.home_dir().join(".ssh").join("known_hosts")
    }

    /// Every registered host with its link. Hosts not started yet are not
    /// started here (the sandbox list does that); a stale latency is timed
    /// again in the background, so the answer never waits on ssh.
    pub fn remote_hosts_view(&self) -> Result<Vec<HostView>> {
        let hosts = crate::remotes::load_hosts(&self.store.paths)?;
        let started = self.started_remotes();
        let names = self.store.names();
        let mut out = Vec::new();
        for h in hosts.hosts.values() {
            let r = started.iter().find(|r| r.cfg.name == h.name && r.cfg == *h);
            let (link, down_since) = match r.map(|r| (*r.link.state().borrow(), r.ever_up())) {
                Some((LinkState::Up, _)) => ("up", None),
                Some((LinkState::Down { since }, true)) => ("down", Some(since)),
                _ => ("never", None),
            };
            let mut latency_ms = None;
            if let Some(r) = r {
                let last = *r.latency.lock().unwrap();
                latency_ms = last.map(|(_, d)| duration_ms(d));
                if link == "up" && last.is_none_or(|(at, _)| at.elapsed() > LATENCY_TTL) {
                    // Marked now so the next looks do not start more of these.
                    *r.latency.lock().unwrap() = Some((Instant::now(), last.map_or(Duration::ZERO, |(_, d)| d)));
                    let r = r.clone();
                    tokio::spawn(async move {
                        let _ = time_round_trip(&r).await;
                    });
                }
            }
            out.push(HostView {
                name: h.name.to_string(),
                form: HostForm::of(h),
                address: h.address().to_string(),
                link,
                down_since,
                latency_ms,
                sandboxes: names.iter().filter(|n| n.host == h.name).count(),
            });
        }
        Ok(out)
    }

    /// What ssh would actually connect to: an alias may point somewhere else.
    async fn endpoint(&self, h: &RemoteHost) -> Result<(String, u16)> {
        match &h.target {
            Target::Direct { host, port, .. } => Ok((host.clone(), port.unwrap_or(22))),
            Target::Alias(alias) => {
                let ssh = self.ssh_program.lock().unwrap().clone();
                let out = tokio::process::Command::new(&ssh)
                    .args(["-G", alias])
                    .stdin(Stdio::null())
                    .output()
                    .await
                    .with_context(|| format!("could not run {}", ssh.display()))?;
                if !out.status.success() {
                    bail!("ssh -G {alias}: {}", err_text(&out));
                }
                let text = String::from_utf8_lossy(&out.stdout);
                let find = |k: &str| text.lines().find_map(|l| l.strip_prefix(k).map(|v| v.trim().to_string()));
                let host = find("hostname ").unwrap_or_else(|| alias.clone());
                let port = find("port ").and_then(|p| p.parse().ok()).unwrap_or(22);
                Ok((host, port))
            }
        }
    }

    async fn is_known(&self, kname: &str) -> Result<bool> {
        let known = self.known_hosts();
        if !known.exists() {
            return Ok(false);
        }
        let found = tokio::process::Command::new("ssh-keygen")
            .arg("-F")
            .arg(kname)
            .arg("-f")
            .arg(&known)
            .stdin(Stdio::null())
            .output()
            .await
            .context("could not run ssh-keygen")?;
        Ok(found.status.success() && !found.stdout.is_empty())
    }

    /// Fetch the host's keys and fingerprint them. Nothing is written.
    async fn scan(&self, host: &str, port: u16) -> Result<Scanned> {
        let scan = tokio::process::Command::new("ssh-keyscan")
            .args(["-T", KEYSCAN_TIMEOUT, "-p", &port.to_string(), "--", host])
            .stdin(Stdio::null())
            .output()
            .await
            .context("could not run ssh-keyscan")?;
        let mut lines: Vec<String> = String::from_utf8_lossy(&scan.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(String::from)
            .collect();
        lines.sort();
        lines.dedup();
        if lines.is_empty() {
            bail!("could not fetch a host key from {host}:{port}: {}", err_text(&scan));
        }
        let mut fp = tokio::process::Command::new("ssh-keygen")
            .args(["-lf", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("could not run ssh-keygen")?;
        let mut stdin = fp.stdin.take().expect("piped stdin");
        stdin.write_all(lines.iter().map(|l| format!("{l}\n")).collect::<String>().as_bytes()).await?;
        drop(stdin);
        let fp = fp.wait_with_output().await?;
        if !fp.status.success() {
            bail!("ssh-keygen could not read the host key: {}", err_text(&fp));
        }
        let mut fingerprints: Vec<String> =
            String::from_utf8_lossy(&fp.stdout).lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();
        fingerprints.sort();
        Ok(Scanned { lines, fingerprints })
    }

    /// The first of the two calls of an add: the host's key, to be confirmed
    /// unless it is already known.
    pub async fn host_key(&self, h: &RemoteHost) -> Result<HostKey> {
        let (host, port) = self.endpoint(h).await?;
        let known_as = known_name(&host, port);
        if self.is_known(&known_as).await? {
            return Ok(HostKey { known_as, known: true, fingerprints: vec![] });
        }
        let s = self.scan(&host, port).await?;
        Ok(HostKey { known_as, known: false, fingerprints: s.fingerprints })
    }

    /// Make sure the host key is in known_hosts. When it is not, it is
    /// fetched again and appended only if its fingerprints are `confirmed`,
    /// the ones [`host_key`](Self::host_key) showed.
    async fn ensure_known(&self, h: &RemoteHost, confirmed: Option<&[String]>) -> Result<()> {
        let (host, port) = self.endpoint(h).await?;
        let kname = known_name(&host, port);
        if self.is_known(&kname).await? {
            return Ok(());
        }
        let Some(confirmed) = confirmed else {
            bail!("the host key of {kname} is not trusted yet: confirm its fingerprint first");
        };
        let s = self.scan(&host, port).await?;
        let mut want: Vec<String> = confirmed.iter().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
        want.sort();
        if want != s.fingerprints {
            bail!("the host key of {kname} is not the one you confirmed (it changed since it was shown): nothing changed");
        }
        let known = self.known_hosts();
        let dir = known.parent().expect("a parent");
        if !dir.exists() {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        }
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&known)?;
        f.write_all(s.lines.iter().map(|l| format!("{l}\n")).collect::<String>().as_bytes())?;
        tracing::info!(target: "host", "web UI: trusted the host key of {kname} (added to {})", known.display());
        Ok(())
    }

    fn save_hosts(&self, hosts: &RemoteHosts) -> Result<()> {
        let p = crate::remotes::hosts_file(&self.store.paths);
        std::fs::create_dir_all(p.parent().expect("a parent"))?;
        let tmp = p.with_extension("toml.tmp");
        std::fs::write(&tmp, hosts.render())?;
        std::fs::rename(&tmp, &p)?;
        self.reload_remotes()
    }

    /// Register a host. `fingerprints` are the ones the person confirmed,
    /// needed only when its key is not in known_hosts yet.
    pub async fn host_add(&self, name: &str, form: &HostForm, fingerprints: Option<&[String]>) -> Result<()> {
        let _w = WRITE.lock().await;
        let mut hosts = crate::remotes::load_hosts(&self.store.paths)?;
        if hosts.get(name).is_some() {
            bail!("host '{name}' already exists: edit it instead");
        }
        let host = form.to_host(name)?;
        self.ensure_known(&host, fingerprints).await?;
        hosts.hosts.insert(name.to_string(), host);
        self.save_hosts(&hosts)?;
        tracing::info!(target: "host", "web UI: added host '{name}' (remote hosts are experimental)");
        Ok(())
    }

    /// Replace a host's settings with `form`. A new address needs its key
    /// trusted, as an add does.
    pub async fn host_edit(&self, name: &str, form: &HostForm, fingerprints: Option<&[String]>) -> Result<()> {
        let _w = WRITE.lock().await;
        let mut hosts = crate::remotes::load_hosts(&self.store.paths)?;
        let old = hosts.get(name).cloned().ok_or_else(|| anyhow!("no host '{name}'"))?;
        let new = form.to_host(name)?;
        if new.target != old.target {
            self.ensure_known(&new, fingerprints).await?;
        }
        hosts.hosts.insert(name.to_string(), new);
        self.save_hosts(&hosts)?;
        tracing::info!(target: "host", "web UI: updated host '{name}'");
        Ok(())
    }

    /// Unregister a host. Refused while sbxw holds sandboxes there, unless
    /// `force` (their records stay on disk, as with `sbxw host rm --force`).
    pub async fn host_rm(&self, name: &str, force: bool) -> Result<()> {
        let _w = WRITE.lock().await;
        let mut hosts = crate::remotes::load_hosts(&self.store.paths)?;
        let Some(h) = hosts.get(name).cloned() else { bail!("no host '{name}'") };
        let held: Vec<String> = self.store.names().into_iter().filter(|n| n.host == h.name).map(|n| n.to_string()).collect();
        if !held.is_empty() && !force {
            bail!("sbxw still holds {} on '{name}' ({}): remove them first, or remove the host anyway", plural(held.len()), held.join(", "));
        }
        hosts.hosts.remove(name);
        self.save_hosts(&hosts)?;
        tracing::info!(target: "host", "web UI: removed host '{name}'");
        Ok(())
    }

    /// `sbx version` over the host's link, and a round trip timed.
    pub async fn host_test(&self, name: &HostName) -> Result<HostTest> {
        let r = self.remote(name).await?;
        r.ready().await?;
        let latency = time_round_trip(&r).await?;
        let prog = r.link.exec().sbx_program().await?;
        let out = r.host.exec.exec(HostCmd { timeout: Some(Duration::from_secs(30)), ..HostCmd::new(prog, vec!["version".into()]) }).await?;
        if !out.success() {
            bail!("`sbx version` failed on '{name}': {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(HostTest { version: out.stdout_str().trim().to_string(), latency_ms: duration_ms(latency) })
    }

    /// Clone `url` on the host, under `~/sbxw-repos/<repo>`, for a sandbox to
    /// be created there: the create dialog's "git URL". A folder already
    /// there is used as it is (cloned before, most likely), not cloned over.
    /// Returns the path on the host.
    pub async fn host_clone(&self, name: &HostName, url: &str) -> Result<String> {
        let url = url.trim();
        let repo = repo_folder(url)?;
        let r = self.remote(name).await?;
        r.ready().await?;
        let dir = r.host.exec.home().await?.join(CLONE_DIR).join(&repo);
        let dir_s = dir.to_string_lossy().into_owned();
        if let Some(m) = r.host.fs.stat(&dir).await? {
            if m.kind == crate::host::Kind::Dir {
                tracing::info!(target: "host", "web UI: {dir_s} already exists on '{name}'; using it as it is");
                return Ok(dir_s);
            }
            bail!("{dir_s} exists on '{name}' and is not a folder");
        }
        r.host.fs.create_dir_all(dir.parent().expect("a parent")).await?;
        tracing::info!(target: "host", "web UI: cloning {url} into {dir_s} on '{name}'");
        let cmd = HostCmd {
            timeout: Some(CLONE_TIMEOUT),
            ..HostCmd::new("git", vec!["clone".into(), "--".into(), url.into(), dir_s.clone()])
        };
        let out = r.host.exec.exec(cmd).await?;
        if !out.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            bail!("git clone on '{name}' failed: {}", err.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("no reason given"));
        }
        Ok(dir_s)
    }
}

/// Time `true` over the link, and keep the answer for the hosts list.
async fn time_round_trip(r: &Arc<crate::remotes::Remote>) -> Result<Duration> {
    let t = Instant::now();
    let out = r.host.exec.exec(HostCmd { timeout: Some(LATENCY_TIMEOUT), ..HostCmd::new("true", vec![]) }).await?;
    if !out.success() {
        bail!("the round trip to '{}' did not answer", r.cfg.name);
    }
    let d = t.elapsed();
    *r.latency.lock().unwrap() = Some((Instant::now(), d));
    Ok(d)
}

fn plural(n: usize) -> String {
    if n == 1 {
        "1 sandbox".into()
    } else {
        format!("{n} sandboxes")
    }
}

/// The folder a clone of `url` goes in: its last path part without `.git`.
/// The URL itself reaches git as one argument after `--`; it still may not
/// look like an option or hold spaces, as nothing a person types should.
fn repo_folder(url: &str) -> Result<String> {
    if url.is_empty() || url.starts_with('-') || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        bail!("'{url}' is not a git URL");
    }
    let last = url.trim_end_matches('/').rsplit(['/', ':']).next().unwrap_or("");
    let name = last.strip_suffix(".git").unwrap_or(last);
    let ok = !name.is_empty() && !name.starts_with('.') && name.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    if !ok {
        bail!("cannot tell a folder name from '{url}'");
    }
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::app;

    fn direct(host: &str) -> HostForm {
        HostForm { host: Some(host.into()), ..Default::default() }
    }

    /// A known_hosts line for `name`, so no ssh-keyscan is needed.
    fn trust(app: &App, name: &str) {
        let k = app.known_hosts();
        std::fs::create_dir_all(k.parent().unwrap()).unwrap();
        let mut t = std::fs::read_to_string(&k).unwrap_or_default();
        t.push_str(&format!("{name} ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl\n"));
        std::fs::write(&k, t).unwrap();
    }

    #[test]
    fn known_hosts_spelling() {
        assert_eq!(known_name("h", 22), "h");
        assert_eq!(known_name("h", 2222), "[h]:2222");
    }

    #[test]
    fn a_folder_from_a_git_url() {
        assert_eq!(repo_folder("https://github.com/acme/shop.git").unwrap(), "shop");
        assert_eq!(repo_folder("git@github.com:acme/shop").unwrap(), "shop");
        assert_eq!(repo_folder("https://example.org/x/my.repo/").unwrap(), "my.repo");
        for bad in ["", "-uhttps://x/y", "https://x/a b", "https://x/..", "https://x/.git"] {
            assert!(repo_folder(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_form_is_checked_like_the_file() {
        assert!(direct("10.0.0.5").to_host("lab").is_ok());
        assert!(direct("10.0.0.5").to_host("local").is_err());
        assert!(HostForm { ssh: Some("bb".into()), host: Some("h".into()), ..Default::default() }.to_host("x").is_err());
        assert!(HostForm { ssh: Some("bb".into()), user: Some("tom".into()), ..Default::default() }.to_host("x").is_err());
        // Blank fields are absent fields, as an empty input sends them.
        let h = HostForm { ssh: Some("bb".into()), host: Some(" ".into()), sbx_path: Some("".into()), ..Default::default() }.to_host("x").unwrap();
        assert_eq!(h.target, Target::Alias("bb".into()));
        assert_eq!(h.sbx_path, None);
        let round = HostForm::of(&h).to_host("x").unwrap();
        assert_eq!(round, h);
    }

    #[tokio::test]
    async fn add_edit_rm_write_hosts_toml() {
        let (_d, app) = app();
        trust(&app, "10.0.0.5");
        app.host_add("lab", &direct("10.0.0.5"), None).await.unwrap();
        assert!(app.host_add("lab", &direct("10.0.0.5"), None).await.unwrap_err().to_string().contains("already exists"));
        let on_disk = crate::remotes::load_hosts(&app.store.paths).unwrap();
        assert_eq!(on_disk.get("lab").unwrap().address(), "10.0.0.5");

        // Same address: no key question, only the settings change.
        let form = HostForm { trust_secrets: true, agent_port: Some(9000), ..direct("10.0.0.5") };
        app.host_edit("lab", &form, None).await.unwrap();
        let h = crate::remotes::load_hosts(&app.store.paths).unwrap().get("lab").cloned().unwrap();
        assert!(h.trust_secrets);
        assert_eq!(h.agent_port, 9000);

        let view = app.remote_hosts_view().unwrap();
        assert_eq!(view.len(), 1);
        assert_eq!((view[0].name.as_str(), view[0].link, view[0].sandboxes), ("lab", "never", 0));
        assert!(view[0].form.trust_secrets);

        app.host_rm("lab", false).await.unwrap();
        assert!(crate::remotes::load_hosts(&app.store.paths).unwrap().hosts.is_empty());
        assert!(app.host_rm("lab", false).await.is_err());
    }

    #[tokio::test]
    async fn an_unknown_key_needs_its_fingerprint() {
        let (_d, app) = app();
        // Not in known_hosts and no fingerprint confirmed: refused before any
        // scan, and nothing is written.
        let e = app.host_add("lab", &direct("10.0.0.5"), None).await.unwrap_err();
        assert!(e.to_string().contains("confirm its fingerprint"), "{e:#}");
        assert!(!crate::remotes::hosts_file(&app.store.paths).exists());
        assert!(!app.known_hosts().exists());
    }

    #[tokio::test]
    async fn a_host_with_sandboxes_is_kept_unless_forced() {
        let (_d, app) = app();
        trust(&app, "10.0.0.5");
        app.host_add("lab", &direct("10.0.0.5"), None).await.unwrap();
        let sb = sbxw_core::SandboxRef::parse("lab~neos").unwrap();
        app.store.update(&sb, |r| r.workspace = Some("/srv/neos".into())).unwrap();
        assert_eq!(app.remote_hosts_view().unwrap()[0].sandboxes, 1);
        let e = app.host_rm("lab", false).await.unwrap_err();
        assert!(e.to_string().contains("lab~neos"), "{e:#}");
        app.host_rm("lab", true).await.unwrap();
        assert!(crate::remotes::load_hosts(&app.store.paths).unwrap().hosts.is_empty());
    }

    #[tokio::test]
    async fn a_changed_host_loses_its_link_and_others_keep_theirs() {
        let (_d, app) = app();
        trust(&app, "10.0.0.5");
        trust(&app, "10.0.0.6");
        app.host_add("lab", &direct("10.0.0.5"), None).await.unwrap();
        app.host_add("lab2", &direct("10.0.0.6"), None).await.unwrap();
        // A program that is not there: the links start, and stay down.
        *app.ssh_program.lock().unwrap() = "/nonexistent/ssh".into();
        let lab = app.remote(&HostName::parse("lab").unwrap()).await.unwrap();
        let lab2 = app.remote(&HostName::parse("lab2").unwrap()).await.unwrap();
        app.host_edit("lab", &HostForm { trust_secrets: true, ..direct("10.0.0.5") }, None).await.unwrap();
        let started: Vec<String> = app.started_remotes().iter().map(|r| r.cfg.name.to_string()).collect();
        assert_eq!(started, ["lab2"]);
        assert!(Arc::ptr_eq(&lab2, &app.remote(&HostName::parse("lab2").unwrap()).await.unwrap()));
        let again = app.remote(&HostName::parse("lab").unwrap()).await.unwrap();
        assert!(!Arc::ptr_eq(&lab, &again));
        assert!(again.cfg.trust_secrets);
        app.host_rm("lab2", false).await.unwrap();
        assert!(app.started_remotes().iter().all(|r| r.cfg.name.as_str() != "lab2"));
    }
}
