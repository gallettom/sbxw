//! sbxw — ultra-light wrapper around the standalone `sbx` (Docker Sandboxes) CLI
//! for local development with the Claude Code agent.
//!
//! It NEVER calls `docker sandbox`; only `sbx`.
//!
//! What `sbxw up <name> [path]` does, in order:
//!   1. apply a restrictive local-dev network policy (`sbx policy allow network`);
//!   2. create the sandbox if missing, mounting <path> (default: cwd) as the
//!      agent's working tree — edits flow both ways instantly (Git working-tree
//!      model). Only that directory is shared; the microVM keeps its own FS;
//!   3. set up host aliases (/etc/hosts + macOS lo0 aliases) for your apps;
//!   4. start a provisioning thread that, once the sandbox is `running`,
//!      (re)publishes ports (they are NOT persistent across restarts) and injects
//!      the Claude OAuth token;
//!   5. serve a browser terminal attached to the agent (`sbx run <name>`).
//!
//! Authentication:
//!   * API key  — pass `--use-api-key`; requires ANTHROPIC_API_KEY on the host.
//!                Stored via `sbx secret set -g anthropic`.
//!   * OAuth    — set CLAUDE_CODE_OAUTH_TOKEN on the host; sbxw generates an
//!                ephemeral mixin kit whose spec.yaml routes the token through the
//!                sbx request proxy. The real token never enters the container;
//!                the VM receives the sentinel "proxy-managed" and the proxy swaps
//!                the Authorization header on every outbound request.

mod config;
mod hosts;
mod sbx;
mod web;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use config::{Config, PortMap};
use hosts::HostAlias;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "sbxw", version, about = "Light wrapper around `sbx` for Claude Code dev sandboxes")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create (if needed), provision, and start the web terminal in the background.
    /// Omit the name to just start the web daemon (browse/create sandboxes from the UI).
    Up {
        /// Sandbox name. Omit to start only the web daemon.
        name: Option<String>,
        /// Code path the agent edits in place. Defaults to the current directory.
        path: Option<PathBuf>,
        /// Extra directories to mount read-only (repeatable).
        #[arg(long = "ro", value_name = "DIR")]
        ro: Vec<PathBuf>,
        /// Path to the project config. Defaults to ./sbxw.toml.
        #[arg(long, default_value = "sbxw.toml")]
        config: PathBuf,
        /// Don't start the web terminal; attach the agent in this terminal instead
        /// (runs in the foreground, no daemon).
        #[arg(long)]
        no_web: bool,
        /// If ANTHROPIC_API_KEY is set, store it as the global `anthropic` secret.
        #[arg(long)]
        use_api_key: bool,
        /// Follow the daemon log in this terminal after starting (like `sbxw logs`).
        #[arg(long)]
        tail: bool,
        /// Internal: already running as the daemon process. Do not pass manually.
        #[arg(long, hide = true)]
        daemon: bool,
    },
    /// Tail the log of a running sbxw daemon.
    Logs {
        /// Sandbox name.
        name: String,
        /// Lines of history to show before following.
        #[arg(short = 'n', long, default_value = "40")]
        lines: u32,
    },
    /// (Re)publish the configured ports for a running sandbox.
    Ports {
        name: String,
        #[arg(long, default_value = "sbxw.toml")]
        config: PathBuf,
    },
    /// Serve only the web terminal for an existing sandbox.
    Web {
        name: String,
        #[arg(long, default_value = "sbxw.toml")]
        config: PathBuf,
    },
    /// Open an interactive bash shell inside a running sandbox (foreground).
    Bash {
        /// Sandbox name.
        name: String,
    },
    /// List all sandboxes.
    Ls,
    /// Show published port mappings for one or all sandboxes.
    PortsLs {
        /// Sandbox name. Omit when using --all.
        name: Option<String>,
        /// Show ports for every sandbox.
        #[arg(long)]
        all: bool,
    },
    /// Stop one or more sandboxes (keeps state, can be restarted).
    Stop {
        /// Sandbox names to stop. Omit when using --all.
        names: Vec<String>,
        /// Stop every running sandbox.
        #[arg(long)]
        all: bool,
    },
    /// Remove one or more sandboxes permanently (irreversible).
    Rm {
        /// Sandbox names to remove. Omit when using --all.
        names: Vec<String>,
        /// Remove every sandbox.
        #[arg(long)]
        all: bool,
    },
    /// Kill the sbxw web daemon and clean up /etc/hosts aliases.
    Down {
        /// Sandbox whose daemon to stop. Omit to stop all daemons and clean /etc/hosts.
        name: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Up { name, path, ro, config, no_web, use_api_key, tail, daemon } => {
            if daemon || no_web {
                // Running as the daemon process itself, or in foreground-only mode:
                // init logging (goes to the redirected log file or this terminal).
                init_tracing();
                cmd_up(name, path, ro, config, no_web, use_api_key)
            } else {
                // Default: launch the web terminal as a background daemon.
                cmd_up_background(name, path, ro, config, use_api_key, tail)
            }
        }
        Cmd::Logs { name, lines } => {
            let log = daemon_log_path(&name);
            if !log.exists() {
                anyhow::bail!("no log file for '{name}' — start it with `sbxw up {name}` first");
            }
            let status = std::process::Command::new("tail")
                .args(["-n", &lines.to_string(), "-f", &log.to_string_lossy()])
                .status()?;
            if !status.success() {
                anyhow::bail!("`tail` exited with {status}");
            }
            Ok(())
        }
        Cmd::Ports { name, config } => {
            init_tracing();
            let cfg = Config::load_or_default(&config)?;
            publish_all_ports(&name, &cfg)
        }
        Cmd::Web { name, config } => {
            init_tracing();
            let cfg = Config::load_or_default(&config)?;
            let addr = cfg.web_addr.clone();
            let shell = cfg.web_shell.clone();
            run_web(&addr, name, shell, Arc::new(cfg), false)
        }
        Cmd::Bash { name } => {
            // Foreground bash shell: `sbx exec -it <name> -- bash`, inheriting this terminal.
            let status = std::process::Command::new("sbx")
                .args(["exec", "-it", &name, "--", "bash"])
                .status()?;
            if !status.success() {
                anyhow::bail!("`sbx exec -it {name} -- bash` exited with {status}");
            }
            Ok(())
        }
        Cmd::Ls => {
            let sandboxes = sbx::list_sandboxes();
            if sandboxes.is_empty() {
                println!("No sandboxes.");
                return Ok(());
            }
            // Dynamic column widths.
            let w_name = sandboxes.iter().map(|s| s.name.len()).max().unwrap_or(7).max(7);
            let w_agent = sandboxes.iter().map(|s| s.agent.len()).max().unwrap_or(5).max(5);
            println!("{:<w_name$}  {:<w_agent$}  STATUS",  "SANDBOX", "AGENT");
            println!("{:-<w_name$}  {:-<w_agent$}  ------", "", "");
            for s in &sandboxes {
                let dot = match s.status.as_str() {
                    "running" => "●",
                    "stopped" => "○",
                    _         => "?",
                };
                println!("{:<w_name$}  {:<w_agent$}  {dot} {}", s.name, s.agent, s.status);
            }
            Ok(())
        }
        Cmd::PortsLs { name, all } => {
            if !all && name.is_none() {
                anyhow::bail!("specify a sandbox name, or pass --all");
            }
            let names: Vec<String> = if all {
                sbx::list_sandboxes().into_iter().map(|s| s.name).collect()
            } else {
                vec![name.unwrap()]
            };
            if names.is_empty() {
                println!("No sandboxes.");
                return Ok(());
            }
            let multi = names.len() > 1;
            for n in &names {
                if multi {
                    println!("=== {n} ===");
                }
                match sbx::list_ports(n) {
                    Ok(out) => {
                        let trimmed = out.trim_end();
                        if trimmed.is_empty() {
                            println!("  (no ports published)");
                        } else {
                            println!("{trimmed}");
                        }
                    }
                    Err(e) => eprintln!("  error: {e:#}"),
                }
                if multi {
                    println!();
                }
            }
            Ok(())
        }
        Cmd::Stop { names, all } => {
            if !all && names.is_empty() {
                anyhow::bail!("specify at least one sandbox name, or use --all");
            }
            let targets: Vec<String> = if all {
                sbx::list_sandboxes()
                    .into_iter()
                    .filter(|s| s.status == "running")
                    .map(|s| s.name)
                    .collect()
            } else {
                names
            };
            if targets.is_empty() {
                println!("No running sandboxes to stop.");
                return Ok(());
            }
            for name in &targets {
                sbx::stop_sandbox(name)
                    .with_context(|| format!("failed to stop '{name}'"))?;
                println!("stopped  {name}");
            }
            Ok(())
        }
        Cmd::Rm { names, all } => {
            if !all && names.is_empty() {
                anyhow::bail!("specify at least one sandbox name, or use --all");
            }
            let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
            sbx::rm_sandboxes(&name_refs, all)?;
            if all {
                println!("all sandboxes removed");
            } else {
                for n in &names { println!("removed  {n}"); }
            }
            Ok(())
        }
        Cmd::Down { name } => {
            match name {
                Some(n) => kill_daemon(&n)?,
                None => {
                    // Kill every daemon tracked by a PID file…
                    let tmp = std::env::temp_dir();
                    if let Ok(entries) = std::fs::read_dir(&tmp) {
                        for entry in entries.flatten() {
                            let fname = entry.file_name().to_string_lossy().into_owned();
                            if let Some(n) = fname
                                .strip_prefix("sbxw-")
                                .and_then(|s| s.strip_suffix(".pid"))
                            {
                                let _ = kill_daemon(n);
                            }
                        }
                    }
                    // …plus any daemon started before PID files existed.
                    kill_untracked_daemons();
                    init_tracing();
                    hosts::clear_hosts_block()?;
                    tracing::info!("removed sbxw /etc/hosts block");
                }
            }
            Ok(())
        }
    }
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("sbxw=info,sbx=info")),
        )
        .with_target(false)
        .init();
}

/// Re-exec sbxw as a detached daemon, redirecting its output to a log file.
/// Prints a brief status line to the terminal, then either exits or tails the log.
fn cmd_up_background(
    name: Option<String>,
    path: Option<PathBuf>,
    ro: Vec<PathBuf>,
    config: PathBuf,
    use_api_key: bool,
    tail: bool,
) -> Result<()> {
    // Daemon log/pid files are keyed by sandbox name; fall back to "web" for
    // the name-less web-only daemon.
    let key = name.as_deref().unwrap_or("web");
    let log = daemon_log_path(key);

    // Load config just to show the web address in the status line.
    let web_addr = Config::load_or_default(&config)
        .ok()
        .map(|c| c.web_addr)
        .unwrap_or_else(|| "127.0.0.1:7681".into());

    // Create / truncate the log file before spawning so it exists for `tail -f`.
    let log_file = std::fs::OpenOptions::new()
        .create(true).write(true).truncate(true)
        .open(&log)?;

    // Reconstruct the Up args for the daemon re-exec.
    let exe = std::env::current_exe()?;
    let config_abs = if config.is_absolute() {
        config.clone()
    } else {
        std::env::current_dir()?.join(&config)
    };
    let mut args: Vec<std::ffi::OsString> = vec!["up".into()];
    if let Some(ref n) = name { args.push(n.into()); }
    if let Some(ref p) = path { args.push(p.into()); }
    for r in &ro { args.push("--ro".into()); args.push(r.into()); }
    args.push("--config".into()); args.push((&config_abs).into());
    if use_api_key { args.push("--use-api-key".into()); }
    args.push("--daemon".into());

    let mut cmd = std::process::Command::new(&exe);
    cmd.args(&args)
       .stdout(log_file.try_clone()?)
       .stderr(log_file)
       .stdin(std::process::Stdio::null());

    // Detach from our process group so Ctrl+C in the launching terminal
    // doesn't propagate to the daemon.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let child = cmd.spawn()?;
    let pid = child.id();

    // Write PID file so `sbxw down [name]` can kill this daemon later.
    let _ = std::fs::write(daemon_pid_path(key), pid.to_string());

    let web_port = web_addr.split(':').last().unwrap_or("7681");
    eprintln!("sbxw  pid {pid}  →  http://sbxw.localhost:{web_port}");
    eprintln!("logs  {}  (sbxw logs {key})", log.display());
    eprintln!("stop  sbxw down {key}");

    if tail {
        std::process::Command::new("tail")
            .args(["-n", "20", "-f", &log.to_string_lossy()])
            .status()?;
    }

    Ok(())
}

/// Path to the log file for a named sandbox daemon.
fn daemon_log_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("sbxw-{name}.log"))
}

/// Path to the PID file for a named sandbox daemon.
fn daemon_pid_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("sbxw-{name}.pid"))
}

/// Kill the sbxw daemon for `name`.
///
/// Two strategies, both attempted:
///   1. PID file (`/tmp/sbxw-<name>.pid`) written at daemon startup.
///   2. `pgrep` fallback for daemons started before PID files existed.
///
/// Uses SIGKILL (not SIGTERM): Tokio's runtime can delay or absorb SIGTERM
/// since it manages its own signal infrastructure.
fn kill_daemon(name: &str) -> Result<()> {
    let pid_file = daemon_pid_path(name);
    let mut pids: Vec<u32> = Vec::new();

    // Strategy 1: PID file.
    if pid_file.exists() {
        if let Ok(s) = std::fs::read_to_string(&pid_file) {
            if let Ok(pid) = s.trim().parse::<u32>() {
                pids.push(pid);
            }
        }
        let _ = std::fs::remove_file(&pid_file);
    }

    // Strategy 2: pgrep fallback (catches daemons without PID files).
    if let Ok(out) = std::process::Command::new("pgrep")
        .args(["-f", &format!("sbxw up {name}")])
        .output()
    {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if let Ok(pid) = line.trim().parse::<u32>() {
                if !pids.contains(&pid) {
                    pids.push(pid);
                }
            }
        }
    }

    if pids.is_empty() {
        println!("no sbxw daemon found for '{name}'");
        return Ok(());
    }

    for pid in pids {
        // SIGKILL — cannot be caught or ignored, guaranteed to terminate.
        let gone = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if gone {
            println!("stopped  sbxw [{name}]  pid {pid}");
        } else {
            println!("sbxw [{name}] pid {pid} already gone");
        }
    }
    Ok(())
}

/// Kill any sbxw `--daemon` processes not tracked by a PID file.
/// Used by `sbxw down` (no-name variant) as a catch-all cleanup.
fn kill_untracked_daemons() {
    let Ok(out) = std::process::Command::new("pgrep")
        .args(["-f", "sbxw.*--daemon"])
        .output()
    else { return };

    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Ok(pid) = line.trim().parse::<u32>() {
            let gone = std::process::Command::new("kill")
                .args(["-9", &pid.to_string()])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if gone {
                println!("stopped  sbxw daemon  pid {pid}  (untracked)");
            }
        }
    }
}

/// Map each PortMap to the (alias, ip) used on the host.
fn aliases_for(cfg: &Config) -> Vec<HostAlias> {
    cfg.ports
        .iter()
        .enumerate()
        .map(|(i, p)| HostAlias {
            hostname: p.alias.clone(),
            ip: if cfg.ip_per_app {
                format!("127.0.0.{}", 2 + i) // distinct loopback IP per app
            } else {
                "127.0.0.1".into()
            },
        })
        .collect()
}

/// Build the `sbx ports --publish` spec for one mapping.
fn port_spec(cfg: &Config, p: &PortMap, ip: &str) -> String {
    if cfg.ip_per_app {
        format!("{ip}:{}:{}", p.host_port, p.sandbox_port)
    } else {
        // Shared loopback: HOST_IP defaults to 127.0.0.1, so omit it.
        format!("{}:{}", p.host_port, p.sandbox_port)
    }
}

fn publish_all_ports(name: &str, cfg: &Config) -> Result<()> {
    let aliases = aliases_for(cfg);
    for (p, a) in cfg.ports.iter().zip(aliases.iter()) {
        let spec = port_spec(cfg, p, &a.ip);
        tracing::info!("publishing {} -> http://{}:{}", spec, a.hostname, p.host_port);
        if let Err(e) = sbx::publish_port(name, &spec) {
            tracing::warn!("could not publish {spec}: {e:#}");
        }
    }
    Ok(())
}

/// Full bring-up pipeline for a sandbox: OAuth kit, create-or-reuse, network
/// policy, API key, host aliases, and a port-publishing provisioning thread.
/// Does NOT start the web terminal or attach to this terminal — callers do that.
/// Called both by `cmd_up` (CLI) and by `api_create` (web UI) so they share
/// exactly the same provisioning path.
/// Extra ports added from the web UI at create time, merged with cfg.ports.
/// sandbox_port is mandatory; host_port defaults to sandbox_port; alias may be empty.
pub(crate) struct ExtraPort {
    pub sandbox_port: u16,
    pub host_port: u16,
    pub alias: String,
}

pub(crate) fn provision_sandbox(
    name: &str,
    workspace: &str,
    ro_strs: &[String],
    cfg: &Config,
    extra_ports: &[ExtraPort],
    use_api_key: bool,
) -> Result<()> {
    // 1. Prepare the OAuth kit if a token is available.
    let oauth_token = resolve_oauth_token();
    let kit_dir = if let Some(ref token) = oauth_token {
        match write_oauth_kit(token, &cfg.claude_subscription) {
            Ok(d) => {
                tracing::info!("OAuth kit prepared at {}", d.display());
                Some(d)
            }
            Err(e) => {
                tracing::warn!("could not prepare OAuth kit (will fall back to /login): {e:#}");
                None
            }
        }
    } else {
        None
    };
    let kit_path = kit_dir.as_deref().and_then(|p| p.to_str());

    // 2. Create the sandbox if it doesn't exist yet.
    if sbx::exists(name)? {
        tracing::info!("sandbox '{name}' already exists — reusing it");
        if let Some(kit) = kit_path {
            tracing::info!("applying OAuth kit to existing sandbox via kit add");
            if let Err(e) = sbx::kit_add(name, kit) {
                tracing::warn!("OAuth kit add failed (use /login in-session instead): {e:#}");
            }
        }
    } else {
        tracing::info!("creating sandbox '{name}' on workspace {workspace}");
        sbx::create_claude(name, workspace, ro_strs, kit_path)?;
    }

    // Clean up the ephemeral kit directory now that sbx has consumed it.
    if let Some(dir) = kit_dir {
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 3. Network policy (sandbox-scoped; requires the sandbox to exist).
    //    MUST run before kits: a kit's `startup` commands often download tools
    //    and need the egress allowlist already in place, or they 403.
    if !cfg.network_allow.is_empty() {
        let resources = cfg.network_allow.join(",");
        tracing::info!("network allowlist: {resources}");
        sbx::policy_allow_network(name, &resources)
            .context("failed to apply network allowlist")?;
    }
    if !cfg.network_deny.is_empty() {
        let resources = cfg.network_deny.join(",");
        tracing::info!("network denylist: {resources}");
        sbx::policy_deny_network(name, &resources)
            .context("failed to apply network denylist")?;
    }

    // 3b. User-defined kits from sbxw.toml (applied in order via sbx kit add).
    //     sbx kit add is idempotent — safe to run on every `sbxw up`.
    //     Runs AFTER network policy so kit startup commands have egress access.
    //     A kit reference is a directory (with spec.yaml), ZIP, OCI ref, or git URL.
    for kit in &cfg.kits {
        tracing::info!("applying kit: {kit}");
        if let Err(e) = sbx::kit_add(name, kit) {
            tracing::warn!("kit '{kit}' failed to apply: {e:#}");
        }
    }

    // 4. API-key auth (confirmed path) — optional.
    if use_api_key {
        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            if !key.trim().is_empty() {
                tracing::info!("storing ANTHROPIC_API_KEY as global `anthropic` secret");
                sbx::secret_set_stdin("anthropic", key.trim(), true, None)
                    .context("failed to store anthropic secret")?;
            }
        } else {
            tracing::warn!("--use-api-key set but ANTHROPIC_API_KEY is empty/unset");
        }
    }

    // Effective port list = config defaults + ports added from the UI.
    // Each entry carries (host_port, sandbox_port, alias) — alias may be empty.
    let all_ports: Vec<(u16, u16, String)> = cfg.ports.iter()
        .map(|p| (p.host_port, p.sandbox_port, p.alias.clone()))
        .chain(extra_ports.iter()
            .map(|p| (p.host_port, p.sandbox_port, p.alias.clone())))
        .collect();

    // 5. Host aliases for ports that have a named alias.
    //    IP assignment: when ip_per_app, each port gets 127.0.0.{2+i}.
    //    This is computed from the index in all_ports so the provisioning
    //    thread can reproduce the same IP without relying on the aliases list.
    let mut aliases: Vec<HostAlias> = all_ports.iter().enumerate()
        .filter(|(_, (_, _, alias))| !alias.is_empty())
        .map(|(i, (_, _, alias))| HostAlias {
            hostname: alias.clone(),
            ip: if cfg.ip_per_app {
                format!("127.0.0.{}", 2 + i)
            } else {
                "127.0.0.1".into()
            },
        })
        .collect();
    let web_ip = cfg.web_addr.split(':').next().unwrap_or("127.0.0.1").to_string();
    if web_ip.starts_with("127.") {
        aliases.push(HostAlias { hostname: "sbxw.localhost".into(), ip: web_ip });
    }
    let web_port = cfg.web_addr.split(':').last().unwrap_or("7681");
    hosts::ensure_loopback_aliases(&aliases)?;
    hosts::sync_hosts_block(&aliases)?;
    for (_, (host_port, sandbox_port, alias)) in all_ports.iter().enumerate().filter(|(_, (_, _, a))| !a.is_empty()) {
        tracing::info!("alias ready: http://{}:{} (sandbox :{})", alias, host_port, sandbox_port);
    }
    tracing::info!("web interface → http://sbxw.localhost:{web_port}");

    // 6. Provisioning thread: wait for `running`, then (re)publish ALL ports.
    //    IP is derived from the port's index in all_ports — no alias required.
    let prov_name = name.to_string();
    let prov_ip_per_app = cfg.ip_per_app;
    let prov_specs: Vec<String> = all_ports.iter().enumerate()
        .map(|(i, (host_port, sandbox_port, _))| {
            if prov_ip_per_app {
                let ip = format!("127.0.0.{}", 2 + i);
                format!("{ip}:{host_port}:{sandbox_port}")
            } else {
                format!("{host_port}:{sandbox_port}")
            }
        })
        .collect();
    std::thread::spawn(move || {
        // Wait up to ~60s for the sandbox to come up (started by `sbx run`).
        for _ in 0..120 {
            if sbx::is_running(&prov_name).unwrap_or(false) {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        // Re-publish ports (not persistent across restarts).
        for spec in &prov_specs {
            if let Err(e) = sbx::publish_port(&prov_name, spec) {
                tracing::warn!("publish {spec} failed: {e:#}");
            } else {
                tracing::info!("published {spec}");
            }
        }
        // Show what the daemon actually has published, for confirmation.
        if let Ok(table) = sbx::list_ports(&prov_name) {
            for line in table.lines() {
                tracing::info!(target: "sbx", "ports | {line}");
            }
        }
    });

    Ok(())
}

fn cmd_up(
    name: Option<String>,
    path: Option<PathBuf>,
    ro: Vec<PathBuf>,
    config: PathBuf,
    no_web: bool,
    use_api_key: bool,
) -> Result<()> {
    sbx::assert_available()?;
    let cfg = Config::load_or_default(&config)?;

    // Web-only mode: no sandbox name given. Just start the web daemon so the
    // user can browse / create / attach sandboxes from the UI. Nothing is
    // provisioned here — api_create handles provisioning per-sandbox.
    let Some(name) = name else {
        if no_web {
            anyhow::bail!("--no-web requires a sandbox name to attach to");
        }
        tracing::info!("starting web daemon only (no sandbox provisioned)");
        return run_web(&cfg.web_addr.clone(), String::new(), cfg.web_shell.clone(), Arc::new(cfg), use_api_key);
    };

    // Resolve workspace path (default: cwd), and make it absolute.
    let workspace = match path {
        Some(p) => p,
        None => std::env::current_dir()?,
    };
    let workspace = std::fs::canonicalize(&workspace)
        .with_context(|| format!("workspace path does not exist: {}", workspace.display()))?;
    let ws_str = workspace.to_string_lossy().to_string();
    let ro_strs: Vec<String> = ro
        .iter()
        .map(|p| std::fs::canonicalize(p).map(|c| c.to_string_lossy().to_string()))
        .collect::<std::io::Result<_>>()
        .context("a --ro path does not exist")?;

    // Resolve kit paths relative to the config file's directory so that
    // relative paths in sbxw.toml work regardless of where sbxw was invoked.
    let config_abs = if config.is_absolute() { config.clone() } else { std::env::current_dir()?.join(&config) };
    let config_dir = config_abs.parent().unwrap_or(config_abs.as_path());
    let mut cfg = cfg;
    cfg.kits = cfg.kits.into_iter().map(|k| {
        let p = std::path::Path::new(&k);
        if p.is_absolute() { k } else { config_dir.join(p).to_string_lossy().into_owned() }
    }).collect();

    provision_sandbox(&name, &ws_str, &ro_strs, &cfg, &[], use_api_key)?;

    // Start the agent: either via the web terminal or in this terminal.
    if no_web {
        tracing::info!("attaching agent in this terminal (no web). Ctrl-C to detach.");
        run_agent_foreground(&name)
    } else {
        run_web(&cfg.web_addr.clone(), name, cfg.web_shell.clone(), Arc::new(cfg), use_api_key)
    }
}

/// Foreground attach: `sbx run <name>` inheriting this terminal.
fn run_agent_foreground(name: &str) -> Result<()> {
    use std::process::Command;
    let status = Command::new("sbx").args(["run", name]).status()?;
    if !status.success() {
        anyhow::bail!("`sbx run {name}` exited with {status}");
    }
    Ok(())
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn run_web(addr: &str, name: String, shell: String, cfg: Arc<Config>, use_api_key: bool) -> Result<()> {
    web::serve(addr, name, shell, cfg, use_api_key).await
}

/// Returns the OAuth token from the host environment, if set and non-empty.
/// Checks CLAUDE_CODE_OAUTH_TOKEN first, then the legacy CLAUDE_OAUTH_TOKEN name.
fn resolve_oauth_token() -> Option<String> {
    for var in ["CLAUDE_CODE_OAUTH_TOKEN", "CLAUDE_OAUTH_TOKEN"] {
        if let Ok(v) = std::env::var(var) {
            if !v.trim().is_empty() {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

/// Write an ephemeral mixin kit directory whose spec.yaml injects the OAuth
/// token into the sandbox via `initFiles`.
///
/// Claude Code sandboxes expose `CLAUDE_ENV_FILE=/etc/sandbox-persistent.sh`:
/// a shell file sourced at agent startup. Writing the token there is the
/// idiomatic path — it works for new sandboxes (`--kit` at create time) and
/// for existing/stopped ones (`sbx kit add`).
///
/// The token is written into the spec.yaml on disk; the temp directory is
/// deleted by the caller immediately after sbx consumes it.
fn write_oauth_kit(token: &str, subscription: &str) -> Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!("sbxw-oauth-kit-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;

    // JSON for Claude Code's credentials file.
    // expiresAt: 2100-01-01T00:00:00Z in milliseconds.
    // refreshToken is set to the access token as a best-effort fallback;
    // the token is valid as-is so no refresh should be triggered.
    // subscriptionType comes from sbxw.toml (`claude_subscription`); it labels
    // the plan in-session, so it must match your actual tier.
    let credentials_json = format!(
        r#"{{"claudeAiOauth":{{"accessToken":"{token}","refreshToken":"{token}","expiresAt":4102444800000,"scopes":["user:inference"],"subscriptionType":"{subscription}"}}}}"#
    );
    std::fs::write(
        dir.join("spec.yaml"),
        format!(
            "schemaVersion: \"1\"\n\
             kind: mixin\n\
             name: claude-oauth\n\
             description: Injects OAuth credentials for Claude Code\n\
             \n\
             network:\n\
             \x20 allowedDomains:\n\
             \x20   - claude.ai\n\
             \n\
             commands:\n\
             \x20 initFiles:\n\
             \x20   - path: /home/agent/.claude/.credentials.json\n\
             \x20     content: '{credentials_json}'\n\
             \x20     mode: \"0600\"\n"
        ),
    )?;

    Ok(dir)
}
