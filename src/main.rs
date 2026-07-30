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
//!   4. publish ports — a new sandbox gets them at creation (`sbx create -p`),
//!      so they're live from first boot; a provisioning thread then re-publishes
//!      once the sandbox reports `running`, covering the reused/restarted case
//!      (mappings don't survive a stop) and conflict recovery picking a
//!      different host port. It also injects the Claude OAuth token;
//!   5. serve a browser terminal attached to the agent (`sbx run <name>`).
//!
//! Authentication:
//!   * API key — pass `--use-api-key`; requires ANTHROPIC_API_KEY on the host,
//!     stored via `sbx secret set -g anthropic`.
//!   * OAuth — set CLAUDE_CODE_OAUTH_TOKEN on the host; sbxw generates an
//!     ephemeral mixin kit whose `initFiles` writes `~/.claude/.credentials.json`
//!     in the sandbox, so the agent is authenticated from first launch. On an
//!     already-*running* sandbox the file is refreshed via `sbx exec` instead,
//!     since `sbx kit add` (0.35+) recreates the container and would kill any
//!     attached session.

mod config;
mod hosts;
mod sbx;
mod web;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use config::Config;
use hosts::HostAlias;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Provisioning messages parked until the terminal is ours again, or `None`
/// when they should be logged as they happen.
///
/// The port-publishing thread outlives the moment the agent starts, by design:
/// it waits for the sandbox to report `running`, and on a fresh or stopped one
/// that is `sbx run` itself booting it. In daemon mode its output goes to the
/// log file and nobody minds. With `--no-web` it lands on a terminal the agent
/// has already switched to raw mode — where a bare `\n` moves down without
/// returning to column 0, so each line starts one step further right (the
/// "staircase"), on top of a full-screen TUI that is now corrupted.
///
/// Fixing the newlines alone would only make the interruption tidier. The
/// terminal belongs to the agent, so in foreground mode these lines wait here
/// and are printed once it exits.
static DEFERRED_PROVISIONING: Mutex<Option<Vec<String>>> = Mutex::new(None);

/// Park provisioning output instead of logging it (foreground/`--no-web`).
fn defer_provisioning_output() {
    *DEFERRED_PROVISIONING.lock().unwrap() = Some(Vec::new());
}

/// Report a provisioning message — live, or parked (see `DEFERRED_PROVISIONING`).
fn provisioning_report(warn: bool, msg: String) {
    if let Some(parked) = DEFERRED_PROVISIONING.lock().unwrap().as_mut() {
        parked.push(if warn {
            format!("WARN  {msg}")
        } else {
            format!("      {msg}")
        });
        return;
    }
    if warn {
        tracing::warn!("{msg}");
    } else {
        tracing::info!("{msg}");
    }
}

/// Print whatever the provisioning thread parked, now that the terminal is
/// line-disciplined again, and go back to logging live. A thread still running
/// past this point finds an empty sink and logs normally — which is correct,
/// since by then nothing is holding the terminal.
fn flush_provisioning_output() {
    let parked = DEFERRED_PROVISIONING.lock().unwrap().take();
    let Some(lines) = parked.filter(|l| !l.is_empty()) else {
        return;
    };
    eprintln!("\n─ provisioning notes (while the agent held the terminal) ─");
    for line in lines {
        eprintln!("{line}");
    }
}

#[derive(Parser)]
#[command(
    name = "sbxw",
    version,
    about = "Light wrapper around `sbx` for Claude Code dev sandboxes"
)]
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
    /// Start a throwaway chat sandbox: an agent with an empty workspace.
    ///
    /// Same as `sbxw up`, except the agent gets a fresh empty directory instead
    /// of one of your projects — so it has none of your code to read or edit.
    /// The workspace is deleted when the sandbox is removed.
    #[command(after_help = "\
Examples:
  sbxw chat                 # throwaway sandbox with a generated chat-xxxxxx name
  sbxw chat brainstorm      # ...or name it yourself
  sbxw rm brainstorm        # removes the sandbox and its empty workspace")]
    Chat {
        /// Sandbox name. Omit to generate a unique `chat-xxxxxx` one.
        name: Option<String>,
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
    },
    /// Tail the log of a running sbxw daemon.
    Logs {
        /// Sandbox name. Omit to tail the web-only daemon log.
        name: Option<String>,
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
    /// Open an SSH session into a sandbox — or run one command in it (experimental).
    ///
    /// Sandboxes are reachable as `<name>.sbx` once `sbx setup ssh` has added its
    /// managed block to your SSH config; `sbxw ssh --setup` runs that for you.
    /// The connection starts the sbx daemon and the sandbox on demand, so unlike
    /// `sbxw bash` this also works on a stopped sandbox.
    #[command(after_help = "\
Examples:
  sbxw ssh --setup              # one-time: add the managed *.sbx block to ~/.ssh/config
  sbxw ssh neos                 # interactive shell
  sbxw ssh neos -- git status   # one-shot command
  code --remote ssh-remote+neos.sbx /workspace   # VS Code / Cursor remote dev")]
    Ssh {
        /// Sandbox name. Omit when using --setup.
        name: Option<String>,
        /// Run `sbx setup ssh` (registers `<name>.sbx` in your SSH config) and exit.
        #[arg(long)]
        setup: bool,
        /// Command to run in the sandbox instead of an interactive shell.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Import skills from your host agents into the store shared by all sandboxes.
    ///
    /// Thin passthrough to `sbx skills import`. Imported skills persist after a
    /// sandbox is deleted and are mounted into new ones; set `share_skills = false`
    /// in sbxw.toml to keep the store out of the sandboxes sbxw creates.
    Skills {
        #[command(subcommand)]
        cmd: SkillsCmd,
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
    /// Check for a new sbxw release and install it in place of this binary.
    ///
    /// On macOS this also refreshes an already-installed SbxwIsland.app when the
    /// release carries a newer build of it (see --no-island).
    Update {
        /// Only check whether a newer version is available; don't install it.
        #[arg(long)]
        check: bool,
        /// Leave SbxwIsland.app alone (don't quit/replace/relaunch it).
        #[arg(long)]
        no_island: bool,
    },
    /// Print a shell completion script, so TAB completes subcommand and flag
    /// names instead of guessing (or running `sbxw help`).
    #[command(after_help = "\
Add one line to your shell rc file (regenerated fresh on every new shell, so
it never goes stale after `sbxw update`):
  zsh    ~/.zshrc         source <(sbxw completion zsh)
  bash   ~/.bashrc        source <(sbxw completion bash)
  fish   ~/.config/fish/config.fish   sbxw completion fish | source

Then open a new shell (or re-source the rc file).")]
    Completion {
        /// Target shell. Defaults to detecting the current shell from $SHELL.
        shell: Option<clap_complete::Shell>,
    },
}

#[derive(Subcommand)]
enum SkillsCmd {
    /// Discover skills from supported host agents and copy them into the store.
    Import {
        /// Preview what would be imported without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Replace skills that already exist in the store.
        #[arg(long)]
        force: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Up {
            name,
            path,
            ro,
            config,
            no_web,
            use_api_key,
            tail,
            daemon,
        } => {
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
        Cmd::Chat {
            name,
            config,
            no_web,
            use_api_key,
            tail,
        } => {
            // A chat sandbox is just `up` pointed at a fresh empty directory, so
            // hand off to the very same code path once the workspace exists. The
            // daemon re-exec that `cmd_up_background` performs is a plain `up` on
            // that path — by then there is nothing chat-specific left to do.
            let name = name.unwrap_or_else(mint_chat_name);
            if !is_valid_sandbox_name(&name) {
                anyhow::bail!(INVALID_NAME_MSG);
            }
            let workspace = PathBuf::from(prepare_chat_workspace(&name)?);
            eprintln!(
                "chat sandbox '{name}' → empty workspace {}",
                workspace.display()
            );
            if no_web {
                init_tracing();
                cmd_up(
                    Some(name),
                    Some(workspace),
                    vec![],
                    config,
                    true,
                    use_api_key,
                )
            } else {
                cmd_up_background(
                    Some(name),
                    Some(workspace),
                    vec![],
                    config,
                    use_api_key,
                    tail,
                )
            }
        }
        Cmd::Logs { name, lines } => {
            let key = name.as_deref().unwrap_or("web");
            let log = daemon_log_path(key);
            if !log.exists() {
                anyhow::bail!("no log file for '{key}' — start it with `sbxw up {key}` first");
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
            run_web(&addr, name, Arc::new(cfg), false)
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
        Cmd::Ssh {
            name,
            setup,
            command,
        } => cmd_ssh(name, setup, &command),
        Cmd::Skills { cmd } => match cmd {
            SkillsCmd::Import { dry_run, force } => sbx::skills_import(dry_run, force),
        },
        Cmd::Ls => {
            let sandboxes = sbx::list_sandboxes();
            if sandboxes.is_empty() {
                println!("No sandboxes.");
                return Ok(());
            }
            // Dynamic column widths.
            let w_name = sandboxes
                .iter()
                .map(|s| s.name.len())
                .max()
                .unwrap_or(7)
                .max(7);
            let w_agent = sandboxes
                .iter()
                .map(|s| s.agent.len())
                .max()
                .unwrap_or(5)
                .max(5);
            println!("{:<w_name$}  {:<w_agent$}  STATUS", "SANDBOX", "AGENT");
            println!("{:-<w_name$}  {:-<w_agent$}  ------", "", "");
            for s in &sandboxes {
                let dot = match s.status.as_str() {
                    "running" => "●",
                    "stopped" => "○",
                    _ => "?",
                };
                println!(
                    "{:<w_name$}  {:<w_agent$}  {dot} {}",
                    s.name, s.agent, s.status
                );
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
                sbx::stop_sandbox(name).with_context(|| format!("failed to stop '{name}'"))?;
                println!("stopped  {name}");
            }
            Ok(())
        }
        Cmd::Rm { names, all } => {
            if !all && names.is_empty() {
                anyhow::bail!("specify at least one sandbox name, or use --all");
            }
            let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
            // Resolve throwaway chat workspaces before the sandboxes go away, so
            // they can be deleted alongside — same cleanup the web UI's delete
            // does (see `api_rm`). `--all` empties the whole chat root, since by
            // then no sandbox is left to own any of it.
            let chat_dirs: Vec<PathBuf> = if all {
                vec![chat_workspace_root()]
            } else {
                names.iter().filter_map(|n| chat_workspace_of(n)).collect()
            };
            sbx::rm_sandboxes(&name_refs, all)?;
            for dir in &chat_dirs {
                if let Err(e) = std::fs::remove_dir_all(dir) {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        tracing::warn!("could not remove chat workspace {}: {e:#}", dir.display());
                    }
                }
            }
            if all {
                println!("all sandboxes removed");
            } else {
                for n in &names {
                    println!("removed  {n}");
                }
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
        Cmd::Update { check, no_island } => cmd_update(check, no_island),
        Cmd::Completion { shell } => cmd_completion(shell),
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

// ── Shell completions ────────────────────────────────────────────────────
/// Writes a completion script for `shell` (or the $SHELL-detected one) to
/// stdout. Output must stay pure — nothing but the script — since callers
/// pipe it straight into `source` or redirect it into a completions file.
fn cmd_completion(shell: Option<clap_complete::Shell>) -> Result<()> {
    let shell = shell.or_else(detect_shell).context(
        "could not detect your shell from $SHELL — pass one explicitly, e.g. `sbxw completion zsh`",
    )?;
    let mut cmd = Cli::command();
    let name = cmd.get_name().to_string();
    clap_complete::generate(shell, &mut cmd, name, &mut std::io::stdout());
    Ok(())
}

/// Best-effort shell detection from the $SHELL environment variable.
fn detect_shell() -> Option<clap_complete::Shell> {
    let shell_path = std::env::var("SHELL").ok()?;
    match std::path::Path::new(&shell_path).file_name()?.to_str()? {
        "bash" => Some(clap_complete::Shell::Bash),
        "zsh" => Some(clap_complete::Shell::Zsh),
        "fish" => Some(clap_complete::Shell::Fish),
        "elvish" => Some(clap_complete::Shell::Elvish),
        "pwsh" | "powershell" => Some(clap_complete::Shell::PowerShell),
        _ => None,
    }
}

// ── Self-update ──────────────────────────────────────────────────────────
// Mirrors install.sh's own download/OS-arch/sudo-fallback logic so
// `sbxw update` behaves exactly like re-running the installer against the
// latest release, without requiring curl to be piped through a shell script.
const REPO: &str = "gallettom/sbxw";

/// Checks GitHub for a newer sbxw release and, unless `check_only`, downloads
/// and installs it in place of the currently running binary.
fn cmd_update(check_only: bool, no_island: bool) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    println!("current version: v{current}");

    println!("checking for updates…");
    let latest_tag = latest_release_tag()?;
    let latest = latest_tag.trim_start_matches('v');
    let base = format!("https://github.com/{REPO}/releases/download/{latest_tag}");

    // The island versions apart from the CLI, so it can be stale even when the
    // binary isn't — check it on both paths.
    let island = |check: bool| {
        if no_island {
            return;
        }
        if let Err(e) = update_island(&base, check) {
            eprintln!("warning: could not refresh SbxwIsland.app: {e:#}");
            eprintln!("  install it manually from https://github.com/{REPO}/releases");
        }
    };

    if parse_version(latest) <= parse_version(current) {
        println!("sbxw is already up to date.");
        island(check_only);
        return Ok(());
    }

    println!("new version available: {latest_tag} (current: v{current})");
    if check_only {
        println!("run `sbxw update` to install it.");
        island(true);
        return Ok(());
    }

    let (os, arch) = target_os_arch()?;
    let artifact = format!("sbxw-{os}-{arch}");

    let tmp_dir = std::env::temp_dir().join(format!("sbxw-update-{}", std::process::id()));
    std::fs::create_dir_all(&tmp_dir)?;
    let tmp_bin = tmp_dir.join(&artifact);

    println!("downloading {artifact} ({latest_tag})…");
    curl_download(&format!("{base}/{artifact}"), &tmp_bin)
        .with_context(|| format!("failed to download {artifact} for {latest_tag}"))?;

    // Best-effort checksum verification against the release's sha256sums.txt
    // (published alongside every release by .github/workflows/release.yml).
    let tmp_sums = tmp_dir.join("sha256sums.txt");
    if curl_download(&format!("{base}/sha256sums.txt"), &tmp_sums).is_ok() {
        verify_checksum(&tmp_bin, &artifact, &tmp_sums)
            .context("checksum verification failed — aborting update")?;
        println!("checksum verified.");
    } else {
        eprintln!("warning: could not fetch sha256sums.txt — skipping checksum verification");
    }

    set_executable(&tmp_bin)?;

    let exe = std::env::current_exe().context("could not resolve current executable path")?;
    install_binary(&tmp_bin, &exe)?;
    let _ = std::fs::remove_dir_all(&tmp_dir);

    println!("sbxw updated: v{current} → {latest_tag}");
    island(false);
    println!("note: restart any running daemons to pick up the new build (`sbxw down` then `sbxw up …`).");
    Ok(())
}

// ── macOS companion app (sbxw Island) ────────────────────────────────────
// The bundle installed by install.sh lives outside the binary's reach, so it
// used to stay on whatever build shipped the day it was installed. `sbxw
// update` now refreshes it in place — but only if the user already has it:
// updating must never install something they declined.
const ISLAND_APP: &str = "SbxwIsland.app";
/// Bundle executable name, i.e. what the running process is called.
const ISLAND_PROC: &str = "SbxwIsland";

/// Refreshes an installed `SbxwIsland.app` when the release ships a newer build
/// of it. macOS-only, and a no-op when the app isn't installed. With
/// `check_only` it reports what it would do without touching anything.
///
/// The app's version is independent of the release tag, so the release
/// publishes `island-version.txt` next to the zip (see macos/build-app.sh) —
/// that's what tells us the bundle is stale without downloading it first.
fn update_island(base: &str, check_only: bool) -> Result<()> {
    if std::env::consts::OS != "macos" {
        return Ok(());
    }
    let Some(app) = installed_island_app() else {
        return Ok(()); // not installed — nothing to refresh
    };

    let tmp_dir = std::env::temp_dir().join(format!("sbxw-island-{}", std::process::id()));
    std::fs::create_dir_all(&tmp_dir)?;
    let cleanup = |dir: &std::path::Path| {
        let _ = std::fs::remove_dir_all(dir);
    };

    let tmp_ver = tmp_dir.join("island-version.txt");
    if curl_download(&format!("{base}/island-version.txt"), &tmp_ver).is_err() {
        // Releases before the island got its own version don't publish it.
        // Leaving the app alone beats bouncing it on every update.
        cleanup(&tmp_dir);
        println!("island: release has no version marker — leaving {ISLAND_APP} as is.");
        return Ok(());
    }
    let latest = std::fs::read_to_string(&tmp_ver)?.trim().to_string();
    let installed = island_bundle_version(&app);

    if parse_version(&latest) <= parse_version(&installed) {
        cleanup(&tmp_dir);
        println!("island: {ISLAND_APP} is up to date (v{installed}).");
        return Ok(());
    }
    println!("island: new build available: v{installed} → v{latest}");
    if check_only {
        cleanup(&tmp_dir);
        return Ok(());
    }

    let artifact = "SbxwIsland-macos.zip";
    let tmp_zip = tmp_dir.join(artifact);
    println!("island: downloading {artifact}…");
    curl_download(&format!("{base}/{artifact}"), &tmp_zip)
        .with_context(|| format!("failed to download {artifact}"))?;

    let tmp_sums = tmp_dir.join("sha256sums.txt");
    if curl_download(&format!("{base}/sha256sums.txt"), &tmp_sums).is_ok() {
        verify_checksum(&tmp_zip, artifact, &tmp_sums)
            .context("checksum verification failed — leaving the installed app alone")?;
    } else {
        eprintln!("warning: could not fetch sha256sums.txt — skipping checksum verification");
    }

    // `ditto -x -k` is what install.sh uses; it restores the bundle layout.
    let status = std::process::Command::new("ditto")
        .args(["-x", "-k"])
        .arg(&tmp_zip)
        .arg(&tmp_dir)
        .status()
        .context("failed to run ditto")?;
    let staged = tmp_dir.join(ISLAND_APP);
    if !status.success() || !staged.is_dir() {
        cleanup(&tmp_dir);
        anyhow::bail!("downloaded archive did not contain {ISLAND_APP}");
    }

    // Replacing the bundle under a running app leaves the old code running, so
    // quit it first — and put it back afterwards only if it *was* running.
    let was_running = island_running();
    if was_running {
        println!("island: quitting the running app…");
        quit_island();
    }

    // Past the quit, a failure must not leave the user without their island:
    // whatever bundle survives at `app` gets relaunched before we bail.
    if let Err(e) = swap_bundle(&staged, &app) {
        cleanup(&tmp_dir);
        if was_running && app.is_dir() {
            let _ = open_island(&app);
        }
        return Err(e);
    }
    // Ad-hoc-signed, not notarised: without this Gatekeeper refuses to open it.
    let _ = std::process::Command::new("xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(&app)
        .status();
    cleanup(&tmp_dir);

    println!("island: {ISLAND_APP} updated: v{installed} → v{latest}");
    if was_running {
        let _ = open_island(&app);
        println!("island: relaunched.");
    }
    Ok(())
}

/// Replaces the bundle at `app` with the freshly extracted one. `mv` rather
/// than `std::fs::rename` for the cross-filesystem case (/tmp → ~/Applications).
fn swap_bundle(staged: &std::path::Path, app: &std::path::Path) -> Result<()> {
    std::fs::remove_dir_all(app)
        .with_context(|| format!("could not remove {} (permissions?)", app.display()))?;
    let status = std::process::Command::new("mv")
        .arg(staged)
        .arg(app)
        .status()
        .context("failed to run mv")?;
    if !status.success() {
        anyhow::bail!("could not install the new bundle at {}", app.display());
    }
    Ok(())
}

fn open_island(app: &std::path::Path) -> std::io::Result<std::process::ExitStatus> {
    std::process::Command::new("open").arg(app).status()
}

/// Where install.sh puts the app, most-specific first. `None` when the user
/// never installed it.
fn installed_island_app() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(PathBuf::from(home).join("Applications").join(ISLAND_APP));
    }
    candidates.push(PathBuf::from("/Applications").join(ISLAND_APP));
    candidates.into_iter().find(|p| p.is_dir())
}

/// The installed bundle's CFBundleShortVersionString, or "0" when it can't be
/// read (an old or hand-built bundle) — which makes it compare as stale.
fn island_bundle_version(app: &std::path::Path) -> String {
    let plist = app.join("Contents").join("Info");
    let out = std::process::Command::new("defaults")
        .arg("read")
        .arg(&plist)
        .arg("CFBundleShortVersionString")
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let v = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if v.is_empty() {
                "0".into()
            } else {
                v
            }
        }
        _ => "0".into(),
    }
}

fn island_running() -> bool {
    std::process::Command::new("pgrep")
        .args(["-x", ISLAND_PROC])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Asks the app to quit (AppleScript, so it shuts down cleanly), then waits for
/// the process to go away, escalating to a signal if it doesn't.
fn quit_island() {
    let _ = std::process::Command::new("osascript")
        .args(["-e", &format!("quit app \"{ISLAND_PROC}\"")])
        .status();
    for _ in 0..30 {
        if !island_running() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = std::process::Command::new("pkill")
        .args(["-x", ISLAND_PROC])
        .status();
    for _ in 0..20 {
        if !island_running() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Fetches the latest release's tag name (e.g. "v1.0.8") from the GitHub API.
fn latest_release_tag() -> Result<String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let out = std::process::Command::new("curl")
        .args(["-fsSL", &url])
        .output()
        .context("failed to run curl — is it installed?")?;
    if !out.status.success() {
        anyhow::bail!(
            "GitHub API request failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).context("could not parse GitHub API response")?;
    json.get("tag_name")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .context("GitHub API response had no tag_name")
}

/// Parses a dotted version string (leading 'v' optional) into numeric
/// components so "1.10.0" correctly compares greater than "1.9.0".
fn parse_version(s: &str) -> Vec<u64> {
    s.trim_start_matches('v')
        .split('.')
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

/// Maps this build's OS/arch to the tokens used in release artifact names
/// (see .github/workflows/release.yml — e.g. "sbxw-macos-arm64").
fn target_os_arch() -> Result<(&'static str, &'static str)> {
    let os = match std::env::consts::OS {
        "macos" => "macos",
        "linux" => "linux",
        other => anyhow::bail!("unsupported OS: {other}"),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "arm64",
        other => anyhow::bail!("unsupported architecture: {other}"),
    };
    Ok((os, arch))
}

fn curl_download(url: &str, dest: &std::path::Path) -> Result<()> {
    let status = std::process::Command::new("curl")
        .args(["-fsSL", url, "-o"])
        .arg(dest)
        .status()
        .context("failed to run curl — is it installed?")?;
    if !status.success() {
        anyhow::bail!("curl exited with {status}");
    }
    Ok(())
}

/// Verifies `bin_path`'s sha256 against the entry for `artifact_name` in a
/// downloaded sha256sums.txt (lines look like "<hash>  <filename>").
fn verify_checksum(
    bin_path: &std::path::Path,
    artifact_name: &str,
    sums_path: &std::path::Path,
) -> Result<()> {
    let sums = std::fs::read_to_string(sums_path)?;
    let expected = sums
        .lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            let hash = parts.next()?;
            let name = parts.next()?.trim_start_matches('*');
            (name == artifact_name).then(|| hash.to_string())
        })
        .with_context(|| format!("no checksum entry for {artifact_name}"))?;

    let actual = sha256_hex(bin_path)?;
    if !actual.eq_ignore_ascii_case(&expected) {
        anyhow::bail!("checksum mismatch: expected {expected}, got {actual}");
    }
    Ok(())
}

/// Computes a file's sha256 by shelling out to `sha256sum` (Linux) or
/// `shasum -a 256` (macOS), avoiding a crypto crate dependency for one command.
fn sha256_hex(path: &std::path::Path) -> Result<String> {
    let out = if std::process::Command::new("sha256sum")
        .arg("--version")
        .output()
        .is_ok()
    {
        std::process::Command::new("sha256sum").arg(path).output()
    } else {
        std::process::Command::new("shasum")
            .args(["-a", "256"])
            .arg(path)
            .output()
    }
    .context("failed to run a sha256 checksum tool")?;
    if !out.status.success() {
        anyhow::bail!("could not compute checksum for {}", path.display());
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .map(str::to_string)
        .context("unexpected checksum tool output")
}

fn set_executable(path: &std::path::Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}

/// Installs `src` as `dest`. Shells out to `mv` (handles the cross-filesystem
/// case, unlike `std::fs::rename`) and, if that fails for lack of permission,
/// retries with `sudo mv` — the same fallback install.sh uses for
/// `/usr/local/bin`.
fn install_binary(src: &std::path::Path, dest: &std::path::Path) -> Result<()> {
    let status = std::process::Command::new("mv")
        .arg(src)
        .arg(dest)
        .status()
        .context("failed to run mv")?;
    if status.success() {
        return Ok(());
    }

    let dir = dest.parent().unwrap_or(dest);
    println!("sudo required to write to {}", dir.display());
    let status = std::process::Command::new("sudo")
        .arg("mv")
        .arg(src)
        .arg(dest)
        .status()
        .context("failed to run sudo mv")?;
    if !status.success() {
        anyhow::bail!("`mv` failed both with and without sudo (exit {status})");
    }
    Ok(())
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
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log)?;

    // Reconstruct the Up args for the daemon re-exec.
    let exe = std::env::current_exe()?;
    let config_abs = if config.is_absolute() {
        config.clone()
    } else {
        std::env::current_dir()?.join(&config)
    };
    let mut args: Vec<std::ffi::OsString> = vec!["up".into()];
    if let Some(ref n) = name {
        args.push(n.into());
    }
    if let Some(ref p) = path {
        args.push(p.into());
    }
    for r in &ro {
        args.push("--ro".into());
        args.push(r.into());
    }
    args.push("--config".into());
    args.push((&config_abs).into());
    if use_api_key {
        args.push("--use-api-key".into());
    }
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

    let web_port = web_port_of(&web_addr);
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

/// Durable, host-side state directory for sbxw — unlike the OS temp dir (used
/// for daemon logs/PIDs, which are fine to lose), this needs to survive
/// reboots and daemon restarts: it's currently the only copy of the
/// name→workspace mapping the web UI's artifacts panel depends on. Losing it
/// doesn't affect the sandbox itself (still runs fine), only the panel, which
/// silently goes blank until `sbxw up <name>` is run again. Falls back to the
/// OS temp dir if `$HOME` can't be resolved.
fn state_dir() -> PathBuf {
    let base = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(".sbxw").join("state");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Path to the file recording the host workspace directory for a named sandbox.
fn workspace_record_path(name: &str) -> PathBuf {
    state_dir().join(format!("{name}.workspace"))
}

/// Look up the host workspace directory `provision_sandbox` recorded for `name`.
/// Used by the web UI's artifacts panel, which reads files straight off the
/// host side of the bind mount instead of round-tripping through `sbx exec`.
pub(crate) fn workspace_for(name: &str) -> Option<PathBuf> {
    let raw = std::fs::read_to_string(workspace_record_path(name)).ok()?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}

/// Subdirectory (relative to the workspace root) where non-code deliverables
/// (wireframes, docs, exports...) are expected to live. Purely a convention:
/// sbxw doesn't enforce it, it just lists+serves whatever it finds there.
pub(crate) const ARTIFACTS_DIR: &str = ".sbxw-artifacts";

// ── Chat sandboxes ───────────────────────────────────────────────────────────
//
// A "chat" sandbox is a throwaway agent with no code to work on: its workspace
// is a fresh empty directory instead of one of your projects, so the only files
// the agent can see are the ones sbxw itself puts there (the `.sbxw-artifacts`
// folder `provision_sandbox` seeds). Everything downstream — provisioning,
// ports, kits, the web terminal — is the ordinary path; only the workspace
// differs. Both entry points (`sbxw chat` and the web UI's 💬 button) go
// through the helpers below so the two can't drift apart.
//
// Note that a chat sandbox still inherits the project's sbxw.toml, so it
// publishes the same `[[ports]]`. Two sandboxes wanting the same host port will
// contend for it; sbx's conflict recovery picks another one. Deliberate — a
// chat sandbox is otherwise indistinguishable from a normal one.

/// Root under which chat sandboxes get their empty, throwaway workspace.
///
/// Deliberately `/tmp` (not `std::env::temp_dir()`): on macOS the latter is
/// `/var/folders/…`, which Docker Desktop doesn't share by default and so
/// can't bind-mount. `/tmp` is shared (and already used for `sbxw-pastes`).
pub(crate) fn chat_workspace_root() -> PathBuf {
    PathBuf::from("/tmp/sbxw-chat")
}

/// Mint a unique-enough `chat-xxxxxx` name from the current time.
pub(crate) fn mint_chat_name() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("chat-{:06x}", (nanos as u64) & 0xff_ffff)
}

/// Sandbox names go into shell commands, file names and hostnames, so keep them
/// to an unambiguous alphabet.
pub(crate) fn is_valid_sandbox_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// What to tell the user when `is_valid_sandbox_name` says no. Shared with the
/// web UI so the CLI and the browser explain the same rule the same way.
pub(crate) const INVALID_NAME_MSG: &str =
    "name must be non-empty and contain only letters, digits, and hyphens";

/// Create the empty workspace for chat sandbox `name`, returning its path.
pub(crate) fn prepare_chat_workspace(name: &str) -> Result<String> {
    let dir = chat_workspace_root().join(name);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("could not create chat workspace {}", dir.display()))?;
    Ok(dir.to_string_lossy().into_owned())
}

/// Whether `name`'s recorded workspace is one of our throwaway chat directories
/// — i.e. safe to delete outright when the sandbox is removed.
pub(crate) fn chat_workspace_of(name: &str) -> Option<PathBuf> {
    workspace_for(name).filter(|p| p.starts_with(chat_workspace_root()))
}

/// Re-create a chat sandbox's workspace directory if it has vanished.
///
/// Chat workspaces live under `/tmp/sbxw-chat` (see `chat_workspace_root`), and
/// `/tmp` is periodically swept by the OS (tmpreaper, a reboot). Once the
/// directory is gone, `sbx run` refuses to start the sandbox — its bind-mount
/// source no longer exists — with a 422. But a chat workspace is empty and
/// disposable by definition, so re-creating the bare directory restores exactly
/// what the sandbox expects; the session simply starts on a clean slate.
///
/// Scoped strictly to chat workspaces: a *normal* sandbox whose workspace
/// disappeared is a real problem (the user's project moved or was deleted), and
/// silently re-creating an empty directory there would hide it. No-op for such
/// sandboxes, and for a chat workspace that is still present.
pub(crate) fn ensure_chat_workspace(name: &str) {
    if let Some(dir) = chat_workspace_of(name) {
        if !dir.exists() {
            match std::fs::create_dir_all(&dir) {
                Ok(()) => tracing::info!(
                    "re-created vanished chat workspace {} for '{name}'",
                    dir.display()
                ),
                Err(e) => tracing::warn!(
                    "could not re-create chat workspace {}: {e:#}",
                    dir.display()
                ),
            }
        }
    }
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
    else {
        return;
    };

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

/// A resolved port mapping: (host_port, sandbox_port, alias). Alias may be empty.
type PortTriple = (u16, u16, String);

/// Merge config ports with UI-added extra ports, preserving order. The index in
/// the result drives the per-app loopback IP, so callers must keep this ordering.
fn merged_ports(cfg: &Config, extra: &[ExtraPort]) -> Vec<PortTriple> {
    cfg.ports
        .iter()
        .map(|p| (p.host_port, p.sandbox_port, p.alias.clone()))
        .chain(
            extra
                .iter()
                .map(|p| (p.host_port, p.sandbox_port, p.alias.clone())),
        )
        .collect()
}

/// Port half of a `web_addr` ("127.0.0.1:7681" → "7681"). `rsplit` rather than
/// `split` so an IPv6 literal yields the port, not the first hextet.
fn web_port_of(addr: &str) -> &str {
    addr.rsplit(':').next().unwrap_or("7681")
}

/// Host IP a port binds to: a distinct loopback per app (`ip_per_app`), else 127.0.0.1.
fn host_ip_for(ip_per_app: bool, index: usize) -> String {
    if ip_per_app {
        format!("127.0.0.{}", 2 + index) // distinct loopback IP per app
    } else {
        "127.0.0.1".into()
    }
}

/// `sbx ports --publish` spec for each mapping. With `ip_per_app` the host IP is
/// explicit; otherwise it defaults to 127.0.0.1 and is omitted.
fn publish_specs(ports: &[PortTriple], ip_per_app: bool) -> Vec<String> {
    ports
        .iter()
        .enumerate()
        .map(|(i, (host, sbox, _))| {
            if ip_per_app {
                format!("{}:{host}:{sbox}", host_ip_for(true, i))
            } else {
                format!("{host}:{sbox}")
            }
        })
        .collect()
}

/// /etc/hosts aliases for the ports that declare a hostname.
fn host_aliases(ports: &[PortTriple], ip_per_app: bool) -> Vec<HostAlias> {
    ports
        .iter()
        .enumerate()
        .filter(|(_, (_, _, alias))| !alias.is_empty())
        .map(|(i, (_, _, alias))| HostAlias {
            hostname: alias.clone(),
            ip: host_ip_for(ip_per_app, i),
        })
        .collect()
}

fn publish_all_ports(name: &str, cfg: &Config) -> Result<()> {
    let ports = merged_ports(cfg, &[]);
    for spec in publish_specs(&ports, cfg.ip_per_app) {
        tracing::info!("publishing {spec}");
        if let Err(e) = sbx::publish_port(name, &spec) {
            tracing::warn!("could not publish {spec}: {e:#}");
        }
    }
    Ok(())
}

/// Best-effort guess at the name a kit reference shows up as in `sbx inspect`:
/// the `name:` field of a directory kit's spec.yaml, else the reference's last
/// path segment stripped of any tag / `.zip` extension. Used only to *skip*
/// re-applying kits (a false negative just re-applies, like pre-0.35 sbxw).
fn kit_display_name(kit: &str) -> String {
    let spec = std::path::Path::new(kit).join("spec.yaml");
    if let Ok(s) = std::fs::read_to_string(spec) {
        for line in s.lines() {
            if let Some(rest) = line.strip_prefix("name:") {
                let n = rest.trim().trim_matches(|c| c == '"' || c == '\'');
                if !n.is_empty() {
                    return n.to_string();
                }
            }
        }
    }
    let last = kit.rsplit('/').next().unwrap_or(kit);
    last.split(':')
        .next()
        .unwrap_or(last)
        .trim_end_matches(".zip")
        .to_string()
}

/// Returns true if a kit reference requires an explicit allowlist entry in sbx.
/// Git URLs (http/https/git@/git://) and non-Docker Hub OCI registries (any
/// hostname prefix other than docker.io) are blocked by default since sbx
/// restricts kit sources to Docker Hub only.
fn kit_needs_allowlist(kit: &str) -> bool {
    if kit.starts_with("http://")
        || kit.starts_with("https://")
        || kit.starts_with("git@")
        || kit.starts_with("git://")
        || kit.starts_with("ssh://")
    {
        return true;
    }
    // OCI ref with an explicit registry hostname (e.g. ghcr.io/owner/kit:tag).
    // Docker Hub refs have no hostname prefix ("owner/kit") or use "docker.io/".
    // Local paths start with '/' or '.'.
    if !kit.starts_with('/') && !kit.starts_with('.') {
        if let Some(first) = kit.split('/').next() {
            if (first.contains('.') || first.contains(':')) && !first.contains("docker.io") {
                return true;
            }
        }
    }
    false
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

/// `sbx create` with the configured port mappings, falling back to a create
/// without them.
///
/// Publishing at creation is all-or-nothing: sbx rejects the whole request with
/// a 409 if a single host port is already bound — something as ordinary as a
/// dev server still running outside the sandbox on 4200. Losing the sandbox
/// over that is the wrong trade, and it's a regression against how sbxw behaved
/// when ports were only ever published afterwards. So on failure we retry
/// bare, and leave the ports to the provisioning thread, which publishes them
/// one at a time and downgrades a conflict to a per-port warning.
fn create_with_port_fallback(name: &str, opts: &sbx::CreateOpts<'_>) -> Result<()> {
    let first = match sbx::create_claude(name, opts) {
        Ok(()) => return Ok(()),
        Err(e) if opts.publish.is_empty() => return Err(e),
        Err(e) => e,
    };

    // sbx may have kept the sandbox and failed only on the port mappings, or
    // rolled the whole thing back. Retrying on top of a live sandbox would
    // just collide with itself, so only retry when nothing is there.
    if sbx::exists(name).unwrap_or(false) {
        tracing::warn!(
            "'{name}' was created but its ports could not be published at creation \
             ({first:#}); they'll be retried individually once it's running"
        );
        return Ok(());
    }

    tracing::warn!(
        "creating '{name}' with its port mappings failed ({first:#}) — \
         retrying without them; each port is then published on its own, so a \
         busy host port costs you that port instead of the whole sandbox"
    );
    let bare = sbx::CreateOpts {
        publish: &[],
        ..*opts
    };
    sbx::create_claude(name, &bare)
        .with_context(|| format!("create with port mappings had failed with: {first:#}"))
}

pub(crate) fn provision_sandbox(
    name: &str,
    workspace: &str,
    ro_strs: &[String],
    cfg: &Config,
    extra_ports: &[ExtraPort],
    use_api_key: bool,
) -> Result<()> {
    // 0. Record the workspace path for this sandbox name (best-effort — used
    // by the web UI's artifacts panel), and make sure the conventional
    // deliverables folder exists so it's discoverable from the first session.
    let _ = std::fs::write(workspace_record_path(name), workspace);
    let artifacts_dir = std::path::Path::new(workspace).join(ARTIFACTS_DIR);
    if !artifacts_dir.exists() {
        if let Err(e) = std::fs::create_dir_all(&artifacts_dir) {
            tracing::warn!("could not create {}: {e:#}", artifacts_dir.display());
        } else {
            let _ = std::fs::write(
                artifacts_dir.join("README.md"),
                "# .sbxw-artifacts\n\n\
                 Drop non-code deliverables here (wireframes, docs, diagrams, exports —\n\
                 .md .pdf .png .jpg .svg .webp .docx .pptx .xlsx .csv .html .txt) and \
                 they show up\nin the sbxw web UI's \"Files\" panel with a one-click \
                 download, instead of\nbeing buried in the repo.\n",
            );
        }
    }

    // 1. Build the OAuth credentials payload if a token is available.
    let credentials_json =
        resolve_oauth_token().map(|t| oauth_credentials_json(&t, &cfg.claude_subscription));

    // Effective port list = config defaults + ports added from the UI. Resolved
    // up here (rather than just before publishing) so a fresh sandbox can be
    // created with the mappings already in place — see `sbx::create_claude`.
    let all_ports = merged_ports(cfg, extra_ports);
    let port_specs = publish_specs(&all_ports, cfg.ip_per_app);

    // 2. Create the sandbox if it doesn't exist yet.
    let existed = sbx::exists(name)?;
    if existed {
        tracing::info!("sandbox '{name}' already exists — reusing it");
        if let Some(ref creds) = credentials_json {
            if sbx::is_running(name).unwrap_or(false) {
                // Running sandbox: refresh the credentials file in place over
                // `sbx exec`. Since sbx 0.35, `kit add` recreates the sandbox
                // container, which would kill any live agent/bash session
                // attached through the web terminal — so no kit here.
                tracing::info!("refreshing OAuth credentials in running sandbox via sbx exec");
                if let Err(e) = sbx::write_oauth_credentials(name, creds) {
                    tracing::warn!(
                        "OAuth credential refresh failed (use /login in-session instead): {e:#}"
                    );
                }
                // The OAuth kit also allowlists claude.ai egress; mirror that.
                if let Err(e) = sbx::policy_allow_network(Some(name), "claude.ai") {
                    tracing::warn!("could not allow claude.ai egress: {e:#}");
                }
            } else {
                // Stopped sandbox: `sbx exec` can't reach it, so go through
                // `kit add`. The container re-creation this triggers (sbx
                // 0.35+) preserves state, and nothing is attached to a
                // stopped sandbox anyway.
                tracing::info!("applying OAuth kit to existing (stopped) sandbox via kit add");
                match write_oauth_kit(creds) {
                    Ok(dir) => {
                        if let Err(e) = sbx::kit_add(name, &dir.to_string_lossy()) {
                            tracing::warn!(
                                "OAuth kit add failed (use /login in-session instead): {e:#}"
                            );
                        }
                        let _ = std::fs::remove_dir_all(&dir);
                    }
                    Err(e) => {
                        tracing::warn!(
                            "could not prepare OAuth kit (will fall back to /login): {e:#}"
                        );
                    }
                }
            }
        }
    } else {
        tracing::info!("creating sandbox '{name}' on workspace {workspace}");
        let kit_dir = match credentials_json.as_deref().map(write_oauth_kit) {
            Some(Ok(d)) => {
                tracing::info!("OAuth kit prepared at {}", d.display());
                Some(d)
            }
            Some(Err(e)) => {
                tracing::warn!("could not prepare OAuth kit (will fall back to /login): {e:#}");
                None
            }
            None => None,
        };
        create_with_port_fallback(
            name,
            &sbx::CreateOpts {
                workspace,
                ro_mounts: ro_strs,
                kit_path: kit_dir.as_deref().and_then(|p| p.to_str()),
                publish: &port_specs,
                share_skills: cfg.share_skills,
            },
        )?;
        // Clean up the ephemeral kit directory now that sbx has consumed it.
        if let Some(dir) = kit_dir {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    // The daemon's port: both the in-sandbox hooks (which POST to it) and the
    // egress rule that lets them through have to name the same one.
    let web_port = web_port_of(&cfg.web_addr);

    // 2b. Pre-trust the workspace so Claude Code doesn't show the "workspace
    // has not been trusted" banner and ignore .claude/settings.local.json's
    // permissions.allow entries on first launch. Requires the container to
    // actually be running for `sbx exec` to reach it.
    if sbx::wait_until_running(name, Duration::from_secs(30)) {
        if let Err(e) = sbx::trust_workspace(name, workspace) {
            tracing::warn!(
                "could not pre-trust workspace (accept the trust dialog manually instead): {e:#}"
            );
        }
        // 2c. Enforce the .sbxw-artifacts convention: block Claude from
        // creating new non-code deliverables anywhere else (see
        // assets/enforce-artifacts.js). Best-effort — the panel still works
        // for anything the agent does place there even if this fails.
        if let Err(e) = sbx::install_artifact_hook(name) {
            tracing::warn!("could not install artifacts-enforcement hook: {e:#}");
        }
        // 2c-bis. Trusted session state: install hooks that POST Claude Code
        // lifecycle events to the daemon (the island derives session state from
        // these), and allow the sandbox to reach the host daemon. Best-effort —
        // the island simply won't track a session whose events can't be delivered.
        if let Err(e) = sbx::install_status_hooks(name, web_port) {
            tracing::warn!("could not install status hooks: {e:#}");
        }
        // Subscription usage (5h / weekly %) via Claude Code's statusLine — it
        // fetches the numbers itself and hands us structured JSON on stdin, so no
        // OAuth token is reused out-of-band.
        if let Err(e) = sbx::install_usage_statusline(name, web_port) {
            tracing::warn!("could not install usage statusLine: {e:#}");
        }
        // The hook reaches the host daemon via host.docker.internal, but the
        // proxy classifies that destination as `localhost:<port>` — so the
        // allow rule must name the loopback host and port, not the DNS alias.
        let hook_dest = format!("localhost:{web_port}");
        if let Err(e) = sbx::policy_allow_network(Some(name), &hook_dest) {
            tracing::warn!("could not allow {hook_dest} egress for hooks: {e:#}");
        }
        // 2d. Default model for the in-sandbox Claude Code (sbxw.toml's
        // `claude_model`, "claude-sonnet-5" by default). Best-effort — the
        // agent falls back to its own default if this fails.
        if !cfg.claude_model.is_empty() {
            if let Err(e) = sbx::set_default_model(name, &cfg.claude_model) {
                tracing::warn!("could not set default model '{}': {e:#}", cfg.claude_model);
            }
        }
    } else {
        tracing::warn!(
            "sandbox '{name}' did not come up in time; skipping workspace trust pre-seed"
        );
    }

    // 3. Network policy (sandbox-scoped; requires the sandbox to exist).
    //    MUST run before kits: a kit's `startup` commands often download tools
    //    and need the egress allowlist already in place, or they 403.
    if !cfg.network_allow.is_empty() {
        let resources = cfg.network_allow.join(",");
        tracing::info!("network allowlist: {resources}");
        sbx::policy_allow_network(Some(name), &resources)
            .context("failed to apply network allowlist")?;
    }
    if !cfg.network_deny.is_empty() {
        let resources = cfg.network_deny.join(",");
        tracing::info!("network denylist: {resources}");
        sbx::policy_deny_network(Some(name), &resources)
            .context("failed to apply network denylist")?;
    }

    // 3b. User-defined kits from sbxw.toml (applied in order via sbx kit add).
    //     Since sbx 0.35 `kit add` RECREATES the sandbox container (state is
    //     preserved and the kit's own network rules are composed in), so
    //     re-applying on every `sbxw up` is no longer free. `sbx inspect`
    //     (0.35+) lists the sandbox's kits: kits it already lists are skipped.
    //     On older sbx — or whenever inspect yields nothing usable — every kit
    //     is applied, matching the previous behaviour.
    //     Runs AFTER network policy so kit startup commands have egress access.
    //     A kit reference is a directory (with spec.yaml), ZIP, or OCI ref (docker.io by default).
    //     Git URLs and non-Docker Hub OCI refs require: sbx settings set kit.allowedSources <prefix>
    let inspect_out = if existed && !cfg.kits.is_empty() {
        sbx::inspect_raw(name).unwrap_or_default()
    } else {
        String::new()
    };
    for kit in &cfg.kits {
        if kit_needs_allowlist(kit) {
            tracing::warn!(
                "kit '{kit}' is a Git URL or non-Docker Hub registry — sbx now restricts kit \
                 sources to Docker Hub by default. Run `sbx settings set kit.allowedSources <prefix>` to allow it."
            );
        }
        let kit_name = kit_display_name(kit);
        if kit_name.len() >= 3 && inspect_out.contains(&kit_name) {
            tracing::info!(
                "kit '{kit}' already applied (listed by `sbx inspect`) — skipping; \
                 run `sbx kit add {name} {kit}` to force a re-apply"
            );
            continue;
        }
        tracing::info!("applying kit: {kit} (sbx 0.35+ recreates the container; state is kept)");
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

    // 5. Host aliases for ports that declare a hostname, plus the web interface.
    let mut aliases = host_aliases(&all_ports, cfg.ip_per_app);
    let web_ip = cfg
        .web_addr
        .split(':')
        .next()
        .unwrap_or("127.0.0.1")
        .to_string();
    if web_ip.starts_with("127.") {
        aliases.push(HostAlias {
            hostname: "sbxw.localhost".into(),
            ip: web_ip,
        });
    }
    hosts::ensure_loopback_aliases(&aliases)?;
    hosts::sync_hosts_block(&aliases)?;
    for (host_port, sandbox_port, alias) in all_ports.iter().filter(|(_, _, a)| !a.is_empty()) {
        tracing::info!("alias ready: http://{alias}:{host_port} (sandbox :{sandbox_port})");
    }
    tracing::info!("web interface → http://sbxw.localhost:{web_port}");

    // 6. Provisioning thread: wait for `running`, then (re)publish ALL ports.
    //    A *fresh* sandbox already got them via `sbx create -p`; this covers the
    //    reused/restarted one, where mappings don't survive a stop.
    let prov_name = name.to_string();
    let prov_specs = port_specs;
    std::thread::spawn(move || {
        // Wait up to ~60s for the sandbox to come up (started by `sbx run`).
        for _ in 0..120 {
            if sbx::is_running(&prov_name).unwrap_or(false) {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        // Re-publish ports. sbx restores them on restart, but we republish anyway
        // in case conflict recovery chose a different host port than we expect.
        for spec in &prov_specs {
            if let Err(e) = sbx::publish_port(&prov_name, spec) {
                // Per-port and non-fatal on purpose: a host port that something
                // else already holds should cost you that one alias, not the
                // sandbox and not the other ports.
                provisioning_report(
                    true,
                    format!(
                        "could not publish {spec}: {e:#}\n\
                         if that host port is taken, free it (or change host_port in \
                         sbxw.toml) and run `sbxw ports {prov_name}`"
                    ),
                );
            } else {
                provisioning_report(false, format!("published {spec}"));
            }
        }
        // Show what the daemon actually has published, for confirmation.
        if let Ok(table) = sbx::list_ports(&prov_name) {
            for line in table.lines() {
                provisioning_report(false, format!("ports | {line}"));
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
        return run_web(
            &cfg.web_addr.clone(),
            String::new(),
            Arc::new(cfg),
            use_api_key,
        );
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
    let config_abs = if config.is_absolute() {
        config.clone()
    } else {
        std::env::current_dir()?.join(&config)
    };
    let config_dir = config_abs.parent().unwrap_or(config_abs.as_path());
    let mut cfg = cfg;
    cfg.kits = cfg
        .kits
        .into_iter()
        .map(|k| {
            let p = std::path::Path::new(&k);
            if p.is_absolute() {
                k
            } else {
                config_dir.join(p).to_string_lossy().into_owned()
            }
        })
        .collect();

    // The port-publishing thread `provision_sandbox` leaves behind reports long
    // after the agent has claimed the terminal, so in foreground mode it has to
    // be muzzled *before* provisioning starts, not after.
    if no_web {
        defer_provisioning_output();
    }

    provision_sandbox(&name, &ws_str, &ro_strs, &cfg, &[], use_api_key)?;

    // Start the agent: either via the web terminal or in this terminal.
    if no_web {
        tracing::info!(
            "attaching agent in this terminal (no web). Ctrl-C to detach.\n\
             port publishing continues in the background — anything it reports \
             is shown when the agent exits."
        );
        let attached = run_agent_foreground(&name);
        flush_provisioning_output();
        attached
    } else {
        run_web(&cfg.web_addr.clone(), name, Arc::new(cfg), use_api_key)
    }
}

/// Foreground attach: `sbx run --name <name>` inheriting this terminal.
///
/// We re-attach to the existing sandbox by name. The positional-name form
/// (`sbx run <name>`) is deprecated as of the latest sbx release, so we use
/// the `--name` flag, which re-attaches independent of the working directory.
/// Since sbx 0.35 this also works for sandboxes created with a custom --kit
/// (like sbxw's OAuth kit) without re-passing the kit reference.
/// Does the user's SSH config appear to carry sbx's managed `*.sbx` block?
///
/// A heuristic used only to decide whether to *hint* at `sbx setup ssh` after a
/// failed connection — an `Include`d fragment would make this a false negative,
/// which is why it never blocks the attempt.
fn ssh_config_mentions_sbx() -> bool {
    let Some(home) = std::env::var_os("HOME") else {
        return false;
    };
    std::fs::read_to_string(PathBuf::from(home).join(".ssh").join("config"))
        .map(|s| s.contains(".sbx"))
        .unwrap_or(false)
}

/// `sbxw ssh` — connect to `<name>.sbx`, the host alias `sbx setup ssh` installs.
///
/// Deliberately does *not* second-guess the transport: sbx owns the SSH config
/// block, the port, and the user, and the connection brings the daemon and the
/// sandbox up on demand. sbxw only picks the hostname and reports a usable error.
fn cmd_ssh(name: Option<String>, setup: bool, command: &[String]) -> Result<()> {
    if setup {
        sbx::setup_ssh().context("`sbx setup ssh` failed")?;
        println!("SSH configured — reach any sandbox with `ssh <name>.sbx`.");
        if name.is_none() {
            return Ok(());
        }
    }
    let Some(name) = name else {
        anyhow::bail!(
            "specify a sandbox name, or pass --setup to register the `*.sbx` SSH host block"
        );
    };

    let host = format!("{name}.sbx");
    let mut args: Vec<&str> = vec![&host];
    args.extend(command.iter().map(String::as_str));
    let status = std::process::Command::new("ssh")
        .args(&args)
        .status()
        .context("failed to spawn `ssh` — is an OpenSSH client installed?")?;
    if status.success() {
        return Ok(());
    }
    // 255 is ssh's own failure code (config/connection/auth), as opposed to the
    // exit status of a command that ran fine on the other side and failed there.
    if status.code() == Some(255) && !ssh_config_mentions_sbx() {
        anyhow::bail!(
            "`ssh {host}` failed and no `*.sbx` entry was found in ~/.ssh/config.\n\
             Run `sbxw ssh --setup` once to register it (SSH access is experimental \
             and may need enabling in your sbx installation first)."
        );
    }
    anyhow::bail!("`ssh {host}` exited with {status}");
}

fn run_agent_foreground(name: &str) -> Result<()> {
    use std::process::Command;
    let status = Command::new("sbx").args(["run", "--name", name]).status()?;
    if !status.success() {
        anyhow::bail!("`sbx run --name {name}` exited with {status}");
    }
    Ok(())
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn run_web(addr: &str, name: String, cfg: Arc<Config>, use_api_key: bool) -> Result<()> {
    // `web_shell` reaches the daemon through `cfg` alone: passing it separately
    // as well left two copies of one setting to keep in step.
    web::serve(addr, name, cfg, use_api_key).await
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

/// JSON payload for Claude Code's `~/.claude/.credentials.json`.
/// expiresAt: 2100-01-01T00:00:00Z in milliseconds.
/// refreshToken is set to the access token as a best-effort fallback;
/// the token is valid as-is so no refresh should be triggered.
/// subscriptionType comes from sbxw.toml (`claude_subscription`); it labels
/// the plan in-session, so it must match your actual tier.
fn oauth_credentials_json(token: &str, subscription: &str) -> String {
    format!(
        r#"{{"claudeAiOauth":{{"accessToken":"{token}","refreshToken":"{token}","expiresAt":4102444800000,"scopes":["user:inference"],"subscriptionType":"{subscription}"}}}}"#
    )
}

/// Write an ephemeral mixin kit directory whose spec.yaml injects the OAuth
/// credentials into the sandbox via `initFiles`.
///
/// Used for new sandboxes (`--kit` at create time) and for existing *stopped*
/// ones (`sbx kit add`). Running sandboxes get the credentials file written
/// directly over `sbx exec` instead (see `sbx::write_oauth_credentials`),
/// because `sbx kit add` recreates the container since sbx 0.35.
///
/// The credentials are written into the spec.yaml on disk; the temp directory
/// is deleted by the caller immediately after sbx consumes it.
fn write_oauth_kit(credentials_json: &str) -> Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!("sbxw-oauth-kit-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;

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

#[cfg(test)]
mod tests {
    use super::*;

    /// The one test touching `DEFERRED_PROVISIONING`; it is process-global, so
    /// nothing else may park output concurrently.
    #[test]
    fn provisioning_output_parks_while_the_agent_holds_the_terminal() {
        defer_provisioning_output();
        provisioning_report(true, "could not publish 4200:4200".into());
        provisioning_report(false, "published 8000:8000".into());

        // Parked, not printed: the terminal is the agent's until it exits.
        {
            let sink = DEFERRED_PROVISIONING.lock().unwrap();
            let lines = sink.as_ref().expect("still deferring");
            assert_eq!(lines.len(), 2);
            assert!(lines[0].starts_with("WARN  "), "{:?}", lines[0]);
            assert!(lines[1].contains("published 8000:8000"));
        }

        flush_provisioning_output();

        // Draining also ends the deferral: a thread still publishing after the
        // agent exits should log live rather than pile up unseen.
        assert!(DEFERRED_PROVISIONING.lock().unwrap().is_none());
        provisioning_report(false, "late arrival".into());
        assert!(DEFERRED_PROVISIONING.lock().unwrap().is_none());
    }

    #[test]
    fn kit_display_name_reads_spec_yaml_name() {
        let dir = std::env::temp_dir().join(format!("sbxw-test-kit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("spec.yaml"),
            "schemaVersion: \"1\"\nkind: mixin\nname: \"my-kit\"\n",
        )
        .unwrap();
        assert_eq!(kit_display_name(&dir.to_string_lossy()), "my-kit");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn kit_display_name_falls_back_to_reference_segment() {
        assert_eq!(kit_display_name("owner/kit:1.2"), "kit");
        assert_eq!(kit_display_name("ghcr.io/owner/toolkit:latest"), "toolkit");
        assert_eq!(kit_display_name("/some/path/bundle.zip"), "bundle");
        assert_eq!(kit_display_name("plain-kit"), "plain-kit");
    }

    /// The island staleness check compares the release's island-version.txt to
    /// the installed bundle's CFBundleShortVersionString via `parse_version`.
    #[test]
    fn island_version_comparison_detects_stale_bundles() {
        let stale = |installed: &str, latest: &str| {
            parse_version(latest) > parse_version(installed) // triggers a refresh
        };
        assert!(stale("1.0.0", "1.1.0"));
        assert!(stale("1.9.0", "1.10.0")); // numeric, not lexicographic
        assert!(!stale("1.0.0", "1.0.0")); // same build: leave the app alone
        assert!(!stale("1.1.0", "1.0.0")); // never downgrade
                                           // Unreadable Info.plist ("0") and locally built bundles count as stale.
        assert!(stale("0", "1.0.0"));
        assert!(stale("0.0.0-dev", "1.0.0"));
    }

    #[test]
    fn web_port_of_extracts_the_port() {
        assert_eq!(web_port_of("127.0.0.1:7681"), "7681");
        assert_eq!(web_port_of("0.0.0.0:9000"), "9000");
        // IPv6 literals are colon-heavy — the *last* colon is the one that counts.
        assert_eq!(web_port_of("[::1]:7681"), "7681");
        // No port at all: fall back to the default rather than the whole host.
        assert_eq!(web_port_of("7681"), "7681");
    }

    #[test]
    fn island_bundle_version_is_zero_when_unreadable() {
        let missing = std::env::temp_dir().join("sbxw-test-no-such.app");
        assert_eq!(island_bundle_version(&missing), "0");
    }
}
