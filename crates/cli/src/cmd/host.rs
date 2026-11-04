//! `sbxw host`: the registry of remote hosts (`~/.sbxw/hosts.toml`).
//!
//! Not `sbxw hosts`, which is the `/etc/hosts` block. The rules and the ssh
//! argv live in `sbxw_core::remote_hosts`; this file runs ssh, asks the
//! question about the host key and touches the disk.

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use sbxw_core::remote_hosts::{RemoteHost, RemoteHosts, Target};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Subcommand)]
pub enum HostCmd {
    /// Register a remote host; shows its SSH host key to confirm first.
    #[command(after_help = "Examples:\n  sbxw host add buildbox --ssh buildbox\n  sbxw host add lab --host 10.0.0.5 --user tom --port 2222 --key ~/.ssh/id_ed25519")]
    Add {
        name: String,
        #[command(flatten)]
        conn: Conn,
        /// Allow secrets (tokens, API keys) to be sent to this host.
        #[arg(long)]
        trust_secrets: bool,
        /// Trust the host key without asking (for scripts).
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// List registered hosts.
    Ls,
    /// Check that a host answers over ssh and has `sbx`.
    Test { name: String },
    /// Change a host; only the flags given are changed.
    Edit {
        name: String,
        #[command(flatten)]
        conn: Conn,
        #[arg(long, conflicts_with = "no_trust_secrets")]
        trust_secrets: bool,
        #[arg(long)]
        no_trust_secrets: bool,
        /// Trust a changed host key without asking (for scripts).
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Unregister a host (its known_hosts line stays).
    Rm {
        name: String,
        /// Even if sbxw still holds state for it (the state is left on disk).
        #[arg(long)]
        force: bool,
    },
}

/// How to reach the host: an ssh alias, or host with optional user, port, key.
#[derive(Args, Default)]
pub struct Conn {
    /// An alias from ~/.ssh/config.
    #[arg(long)]
    ssh: Option<String>,
    /// Host name or address (instead of --ssh).
    #[arg(long)]
    host: Option<String>,
    #[arg(long)]
    user: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    /// Path to an identity file (a path only, never the key itself).
    #[arg(long)]
    key: Option<String>,
    /// Port of the agent on the host (default 7682).
    #[arg(long)]
    agent_port: Option<u16>,
}

pub fn run(cmd: HostCmd) -> Result<()> {
    match cmd {
        HostCmd::Add { name, conn, trust_secrets, yes } => add(&name, conn, trust_secrets, yes),
        HostCmd::Ls => ls(),
        HostCmd::Test { name } => test(&name),
        HostCmd::Edit { name, conn, trust_secrets, no_trust_secrets, yes } => {
            let trust = if trust_secrets {
                Some(true)
            } else if no_trust_secrets {
                Some(false)
            } else {
                None
            };
            edit(&name, conn, trust, yes)
        }
        HostCmd::Rm { name, force } => rm(&name, force),
    }
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).context("HOME is not set")
}

fn hosts_path() -> Result<PathBuf> {
    Ok(home()?.join(".sbxw").join("hosts.toml"))
}

fn load() -> Result<RemoteHosts> {
    let p = hosts_path()?;
    match std::fs::read_to_string(&p) {
        Ok(t) => RemoteHosts::parse(&t).with_context(|| p.display().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(RemoteHosts::default()),
        Err(e) => Err(e).with_context(|| p.display().to_string()),
    }
}

fn save(hosts: &RemoteHosts) -> Result<()> {
    let p = hosts_path()?;
    std::fs::create_dir_all(p.parent().expect("a parent"))?;
    let tmp = p.with_extension("toml.tmp");
    std::fs::write(&tmp, hosts.render())?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

fn add(name: &str, c: Conn, trust_secrets: bool, yes: bool) -> Result<()> {
    let mut hosts = load()?;
    if hosts.get(name).is_some() {
        bail!("host '{name}' already exists; use `sbxw host edit {name}`");
    }
    let host = RemoteHost::new(name, c.ssh, c.host, c.user, c.port, c.key, c.agent_port, trust_secrets)?;
    ensure_known(&host, yes)?;
    hosts.hosts.insert(name.to_string(), host);
    save(&hosts)?;
    println!("Added host '{name}'. Check it with: sbxw host test {name}");
    Ok(())
}

fn edit(name: &str, c: Conn, trust: Option<bool>, yes: bool) -> Result<()> {
    let mut hosts = load()?;
    let old = hosts.get(name).cloned().with_context(|| format!("no host '{name}'"))?;
    let (mut ssh, mut host, mut user, mut port, mut key) = match &old.target {
        Target::Alias(a) => (Some(a.clone()), None, None, None, None),
        Target::Direct { host, user, port, key } => (None, Some(host.clone()), user.clone(), *port, key.clone()),
    };
    // Switching between an alias and a spelled-out host drops the other form.
    if c.ssh.is_some() {
        ssh = c.ssh;
        (host, user, port, key) = (None, None, None, None);
    }
    if c.host.is_some() {
        host = c.host;
        ssh = None;
    }
    user = c.user.or(user);
    port = c.port.or(port);
    key = c.key.or(key);
    let new = RemoteHost::new(
        name,
        ssh,
        host,
        user,
        port,
        key,
        Some(c.agent_port.unwrap_or(old.agent_port)),
        trust.unwrap_or(old.trust_secrets),
    )?;
    if new.target != old.target {
        ensure_known(&new, yes)?;
    }
    hosts.hosts.insert(name.to_string(), new);
    save(&hosts)?;
    println!("Updated host '{name}'.");
    Ok(())
}

fn rm(name: &str, force: bool) -> Result<()> {
    let mut hosts = load()?;
    if hosts.get(name).is_none() {
        bail!("no host '{name}'");
    }
    let state = home()?.join(".sbxw").join("state").join("hosts").join(name);
    let busy = std::fs::read_dir(&state).map(|mut d| d.next().is_some()).unwrap_or(false);
    if busy && !force {
        bail!("{} still holds state for '{name}'; remove its sandboxes first, or pass --force", state.display());
    }
    hosts.hosts.remove(name);
    save(&hosts)?;
    println!("Removed host '{name}'.");
    Ok(())
}

fn ls() -> Result<()> {
    let hosts = load()?;
    if hosts.hosts.is_empty() {
        println!("No remote hosts. Add one with: sbxw host add <name> --ssh <alias>");
        return Ok(());
    }
    // sbxw_core::table parses sbx's output; it does not print, so pad by hand.
    let rows: Vec<[String; 4]> = hosts
        .hosts
        .values()
        .map(|h| {
            let target = match &h.target {
                Target::Alias(a) => format!("ssh {a}"),
                Target::Direct { host, user, port, .. } => {
                    let u = user.as_deref().map(|u| format!("{u}@")).unwrap_or_default();
                    let p = port.map(|p| format!(":{p}")).unwrap_or_default();
                    format!("{u}{host}{p}")
                }
            };
            [h.name.clone(), target, h.agent_port.to_string(), if h.trust_secrets { "yes" } else { "no" }.into()]
        })
        .collect();
    let head = ["NAME", "TARGET", "AGENT PORT", "TRUST SECRETS"];
    let w: Vec<usize> = (0..4)
        .map(|i| rows.iter().map(|r| r[i].chars().count()).chain([head[i].len()]).max().unwrap())
        .collect();
    let line = |cells: [&str; 4]| {
        let s: Vec<String> = cells.iter().zip(&w).map(|(c, w)| format!("{c:<w$}")).collect();
        println!("{}", s.join("  ").trim_end());
    };
    line(head);
    for r in &rows {
        line([&r[0], &r[1], &r[2], &r[3]]);
    }
    Ok(())
}

fn test(name: &str) -> Result<()> {
    let hosts = load()?;
    let h = hosts.get(name).with_context(|| format!("no host '{name}'"))?;
    let out = Command::new("ssh")
        .args(["-o", "ConnectTimeout=10"])
        .args(h.ssh_argv())
        .args(["sbx", "version"])
        .stdin(Stdio::null())
        .output()
        .context("could not run ssh")?;
    if out.status.success() {
        println!("OK {name}: {}", String::from_utf8_lossy(&out.stdout).trim());
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("{name}: {}", err.trim());
    }
}

/// What ssh would actually connect to: an alias may point somewhere else.
fn endpoint(h: &RemoteHost) -> Result<(String, u16)> {
    match &h.target {
        Target::Direct { host, port, .. } => Ok((host.clone(), port.unwrap_or(22))),
        Target::Alias(alias) => {
            let out = Command::new("ssh").args(["-G", alias]).output().context("could not run ssh")?;
            if !out.status.success() {
                bail!("ssh -G {alias}: {}", String::from_utf8_lossy(&out.stderr).trim());
            }
            let text = String::from_utf8_lossy(&out.stdout);
            let find = |k: &str| text.lines().find_map(|l| l.strip_prefix(k).map(|v| v.trim().to_string()));
            let host = find("hostname ").unwrap_or_else(|| alias.clone());
            let port = find("port ").and_then(|p| p.parse().ok()).unwrap_or(22);
            Ok((host, port))
        }
    }
}

/// How known_hosts spells a host: `host`, or `[host]:port` off port 22.
fn known_name(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    }
}

/// Make sure the host key is in known_hosts: if not, fetch it, show its
/// fingerprint, ask, and append it only on yes.
fn ensure_known(h: &RemoteHost, yes: bool) -> Result<()> {
    let (host, port) = endpoint(h)?;
    let known = home()?.join(".ssh").join("known_hosts");
    let kname = known_name(&host, port);
    if known.exists() {
        let found = Command::new("ssh-keygen")
            .arg("-F")
            .arg(&kname)
            .arg("-f")
            .arg(&known)
            .output()
            .context("could not run ssh-keygen")?;
        if found.status.success() && !found.stdout.is_empty() {
            println!("Host key for {kname} is already known.");
            return Ok(());
        }
    }
    let scan = Command::new("ssh-keyscan")
        .args(["-T", "10", "-p", &port.to_string(), &host])
        .stdin(Stdio::null())
        .output()
        .context("could not run ssh-keyscan")?;
    let keys: String = String::from_utf8_lossy(&scan.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| format!("{l}\n"))
        .collect();
    if keys.is_empty() {
        bail!("could not fetch a host key from {host}:{port}: {}", String::from_utf8_lossy(&scan.stderr).trim());
    }
    let mut fp = Command::new("ssh-keygen")
        .args(["-lf", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("could not run ssh-keygen")?;
    fp.stdin.take().expect("piped stdin").write_all(keys.as_bytes())?;
    let fp = fp.wait_with_output()?;
    if !fp.status.success() {
        bail!("ssh-keygen could not read the host key: {}", String::from_utf8_lossy(&fp.stderr).trim());
    }
    println!("Host key of {kname}:\n{}", String::from_utf8_lossy(&fp.stdout).trim_end());
    if !yes {
        if !std::io::stdin().is_terminal() {
            bail!("not a terminal: check the fingerprint above on the host, then pass --yes");
        }
        print!("Trust this key and add it to {}? [y/N] ", known.display());
        std::io::stdout().flush()?;
        let mut a = String::new();
        std::io::stdin().read_line(&mut a)?;
        if !matches!(a.trim().to_lowercase().as_str(), "y" | "yes") {
            bail!("host key not trusted; nothing changed");
        }
    }
    let dir = known.parent().expect("a parent");
    if !dir.exists() {
        std::fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&known)?;
    f.write_all(keys.as_bytes())?;
    println!("Added to {}.", known.display());
    Ok(())
}
