//! sbxw — ultra-light wrapper around the standalone `sbx` (Docker Sandboxes) CLI
//! for local development with the Claude Code agent.
//!
//! It NEVER calls `docker sandbox`; only `sbx`.
//!
//! What `sbxw up <name> [path]` does, in order:
//!   1. apply a restrictive local-dev network policy (`sbx policy allow network`);
//!   2. create the sandbox if missing — with every configured kit passed to
//!      `sbx create --kit`, creation being the only moment sbx applies a kit
//!      whole — mounting <path> (default: cwd) as the
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
//!     stored as a global `anthropic` secret (`sbx secret set` — see
//!     `sbx::secret_scope_args` for how the scope is spelled).
//!   * OAuth — set CLAUDE_CODE_OAUTH_TOKEN on the host; sbxw generates a mixin
//!     kit whose setup files write `~/.claude/.credentials.json` in the
//!     sandbox, so the agent is authenticated from first launch. On an
//!     already-*running* sandbox the file is refreshed via `sbx exec` instead,
//!     because `sbx kit add` recreates the container and would kill any
//!     attached session.
//!
//! Requires **sbx 0.39 or newer** and assumes it throughout — there are no
//! runtime fallbacks for older releases. See `sbx::MIN_SBX_VERSION`.

mod config;
mod envfile;
mod hosts;
mod relay;
mod sbx;
mod web;

use anyhow::{bail, Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use config::Config;
use hosts::HostAlias;
use std::path::{Path, PathBuf};
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

/// Banner for `--help`: the site's logo at terminal scale — a shell prompt and
/// the wordmark, in the colours the favicon already uses (`>_` and `xw` green
/// `#3fb950`, `sb` blue `#58a6ff`, as 256-colour approximations).
///
/// No card around it: boxing the wordmark capped how wide the letters could be,
/// and `x`/`w` are the two that need the room — squeezed into four columns their
/// diagonals collapse into a solid block and stop reading as letters. Off the
/// leash they get five and six columns and are legible again.
///
/// Colour is dropped when stdout isn't a terminal (a pipe, a file, a CI log) or
/// when `NO_COLOR` is set, so `sbxw --help > file` stays plain text.
///
/// `install.sh` prints the same badge in `sh`; the two are separate by
/// necessity, so keep them in step by eye.
fn banner() -> String {
    use std::io::IsTerminal;
    let colour = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    let (blue, green, reset) = if colour {
        ("\x1b[1;38;5;75m", "\x1b[1;38;5;77m", "\x1b[0m")
    } else {
        ("", "", "")
    };
    // Half blocks (▀▄█) put two pixel rows in one text row, which is what lets
    // the wordmark be twice the height of a line of text without taking over the
    // screen: five pixel rows land in three.
    // Columns are (prompt, sb, xw) so each row is coloured in three pieces.
    const ROWS: [(&str, &str, &str); 3] = [
        ("▀█▄   ", "█▀▀▀ █▀▀▄", "▀▄ ▄▀ █    █"),
        (" ▄█▀  ", "▀▀▀█ █▀▀▄", " ▄▀▄  █ ▄▄ █"),
        ("▀▀ ▀▀▀", "▀▀▀▀ ▀▀▀ ", "▀   ▀  ▀  ▀ "),
    ];

    let mut out = String::from("\n");
    for (prompt, sb, xw) in ROWS {
        out.push_str(&format!(
            "  {green}{prompt}{reset}  {blue}{sb}{green} {xw}{reset}\n"
        ));
    }
    // No trailing newline: clap puts its own blank line after `before_help`,
    // and two of them leaves the badge floating.
    out.pop();
    out
}

#[derive(Parser)]
#[command(
    name = "sbxw",
    version,
    about = "Light wrapper around `sbx` for Claude Code dev sandboxes",
    before_help = banner()
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
        /// Sandbox name. Omit to start only the web daemon (or, with
        /// `--add-sandbox`, to derive one from the workspace directory name).
        name: Option<String>,
        /// Code path the agent edits in place. Defaults to the current directory.
        path: Option<PathBuf>,
        /// Extra directories to mount read-only (repeatable).
        #[arg(long = "ro", value_name = "DIR")]
        ro: Vec<PathBuf>,
        /// When no name is given, derive one from the workspace directory name
        /// instead of starting the web-only daemon. On a clash with a
        /// different path already using that name, appends `-copy`,
        /// `-copy-1`, `-copy-2`, ... Ignored if a name is given explicitly.
        #[arg(long = "add-sandbox")]
        add_sandbox: bool,
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
    /// Remove every *stopped* sandbox (requires sbx 0.39+).
    ///
    /// The safe counterpart to `sbxw rm --all`: a running sandbox is never a
    /// candidate, so this can't take out the one you are working in. It shows
    /// what it would remove and asks before doing it.
    #[command(after_help = "\
Examples:
  sbxw prune                     # list the stopped sandboxes, then ask
  sbxw prune --since 168h        # ...only those stopped for over a week
  sbxw prune --dry-run           # list them and stop there
  sbxw prune --yes               # no prompt (scripts, cron)")]
    Prune {
        /// Only sandboxes stopped for longer than this (sbx duration, e.g. 168h).
        #[arg(long, value_name = "DURATION")]
        since: Option<String>,
        /// List what would be removed and exit without removing anything.
        #[arg(long)]
        dry_run: bool,
        /// Skip the confirmation prompt.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Run or write a `.sbxenv.yaml` environment file (requires sbx 0.39+).
    ///
    /// An environment file is sbx's own committable project setup: agent,
    /// workspace, mounts, kits, env, secrets, MCP servers, ports. `run` brings
    /// a sandbox up from one — sbx creates it, sbxw adds the policy,
    /// credentials, hooks, aliases and browser terminal the format cannot
    /// express. `export` writes one out from this project's sbxw.toml.
    Env {
        #[command(subcommand)]
        cmd: EnvCmd,
    },
    /// Inspect or re-apply the sbxw /etc/hosts aliases.
    ///
    /// Editing /etc/hosts needs root. The web daemon has no terminal to answer
    /// a `sudo` password prompt on, so anything it could not write is parked
    /// and applied here, where sudo can ask.
    Hosts {
        #[command(subcommand)]
        action: Option<HostsAction>,
        /// Path to sbxw.toml (its aliases are applied alongside the parked ones).
        #[arg(long, default_value = "sbxw.toml")]
        config: PathBuf,
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
enum EnvCmd {
    /// Bring a sandbox up from a `.sbxenv.yaml`, with sbxw's pipeline around it.
    ///
    /// `sbx env create` creates the sandbox from the file — workspace, kits,
    /// env, secrets, MCP servers, ports — and sbxw does the rest it always
    /// does: egress policy, OAuth credentials, hooks, `/etc/hosts` aliases and
    /// the browser terminal. Host ports are checked *before* anything is
    /// created, because a port sbx cannot publish costs you the whole sandbox.
    ///
    /// The environment file wins wherever it says something; `sbxw.toml` fills
    /// in what the format cannot express, and the run says which of its values
    /// it used.
    #[command(after_help = "\
Examples:
  sbxw env run                      # .sbxenv.yaml in the current directory
  sbxw env run ../neos-env          # ...or in that directory
  sbxw env run base.yaml local.yaml # merged in order, later files win
  sbxw env run --yes                # take free ports without asking
  sbxw env run --no-web             # attach here instead of the browser")]
    Run {
        /// Directories or environment files, merged in order. Defaults to the
        /// current directory.
        paths: Vec<PathBuf>,
        /// Path to the fallback config. Defaults to ./sbxw.toml.
        #[arg(long, default_value = "sbxw.toml")]
        config: PathBuf,
        /// Don't start the web terminal; attach the agent in this terminal.
        #[arg(long)]
        no_web: bool,
        /// Accept the suggested replacement for a busy host port without asking.
        #[arg(long, short = 'y')]
        yes: bool,
        /// If ANTHROPIC_API_KEY is set, store it as the global `anthropic` secret.
        #[arg(long)]
        use_api_key: bool,
        /// Follow the daemon log in this terminal after starting.
        #[arg(long)]
        tail: bool,
    },
    /// Generate a `.sbxenv.yaml` next to the workspace from sbxw.toml.
    #[command(after_help = "\
Examples:
  sbxw env export                       # write ../.sbxenv.yaml for ./ as the workspace
  sbxw env export --name neos ~/dev/neos
  sbxw env export -o -                  # print it instead of writing a file

Then, on any machine with sbx 0.39+ and no sbxw:
  sbx env run                           # from the directory holding the file")]
    Export {
        /// Workspace the agent edits. Defaults to the current directory.
        path: Option<PathBuf>,
        /// Sandbox name to write into the file. Defaults to the workspace's
        /// directory name, which is what `sbxw up --add-sandbox` would derive.
        #[arg(long)]
        name: Option<String>,
        /// Path to the project config. Defaults to ./sbxw.toml.
        #[arg(long, default_value = "sbxw.toml")]
        config: PathBuf,
        /// Where to write it. Defaults to `.sbxenv.yaml` in the workspace's
        /// *parent*, which is where sbx wants it — see the command's notes.
        /// `-` writes to stdout.
        #[arg(long, short = 'o', value_name = "FILE")]
        out: Option<PathBuf>,
        /// Overwrite an existing file.
        #[arg(long)]
        force: bool,
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

#[derive(clap::Subcommand, Clone, Copy, Default)]
enum HostsAction {
    /// Print the sbxw block, plus anything still waiting to be applied.
    #[default]
    Show,
    /// Apply the config's aliases and everything a daemon could not write.
    Sync,
    /// Remove the whole sbxw block from /etc/hosts.
    Clear,
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
            add_sandbox,
        } => {
            // Resolved once, up front: `cmd_up_background` keys the daemon's
            // log/pid files off `name`, so a derived name has to be settled
            // before it (and its own `--daemon` re-exec) ever see it — not
            // recomputed independently on each side, which could disagree.
            let name = resolve_up_name(name, &path, add_sandbox)?;
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
            // The daemon drives sbx as hard as the pipeline does, and it is a
            // separate entry point — `up`'s check never runs for it.
            sbx::assert_available()?;
            let cfg = load_config(&config)?;
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
            // The OAuth kit outlives every other command precisely so sbx can
            // re-resolve it; removal is the one point where it should go.
            let oauth_owners: Vec<String> = if all {
                sbx::list_sandboxes().into_iter().map(|s| s.name).collect()
            } else {
                names.clone()
            };
            sbx::rm_sandboxes(&name_refs, all)?;
            for n in &oauth_owners {
                forget_oauth_kit(n);
            }
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
        Cmd::Prune {
            since,
            dry_run,
            yes,
        } => {
            sbx::assert_available()?;
            cmd_prune(since.as_deref(), dry_run, yes)
        }
        Cmd::Env { cmd } => match cmd {
            EnvCmd::Run {
                paths,
                config,
                no_web,
                yes,
                use_api_key,
                tail,
            } => cmd_env_run(paths, config, no_web, yes, use_api_key, tail),
            EnvCmd::Export {
                path,
                name,
                config,
                out,
                force,
            } => cmd_env_export(path, name, config, out, force),
        },
        Cmd::Hosts { action, config } => cmd_hosts(action.unwrap_or_default(), config),
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

    // Load config for the status line's web address — and for the aliases we
    // settle below, before detaching.
    let cfg = Config::load_or_default(&config).ok();
    let web_addr = cfg
        .as_ref()
        .map(|c| c.web_addr.clone())
        .unwrap_or_else(|| "127.0.0.1:7681".into());

    // Write the /etc/hosts aliases *here*, in the terminal the user just typed
    // in, rather than leaving them to the daemon: this is the only moment a
    // `sudo` password prompt has somebody in front of it. The daemon inherits a
    // block that already has the config's aliases in it, so provisioning finds
    // nothing to change and never needs root at all.
    if let Some(ref cfg) = cfg {
        let aliases = wanted_aliases(cfg, &[]);
        for w in apply_host_aliases(&aliases) {
            eprintln!("warning: {w}");
        }
        if hosts::missing_aliases(&aliases).is_empty() {
            let _ = std::fs::remove_file(pending_hosts_path());
        }
    }

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

// ── Favourite workspace folders ──────────────────────────────────────────────
//
// Where the create-sandbox picker starts from. A developer keeps their projects
// under one or two roots (`~/dev`, `~/work/clients`), and the picker opening at
// `$HOME` every time makes them walk the same three clicks to reach a folder
// they will pick again tomorrow. Starring those roots turns that walk into one
// click, and the *subfolder* choice — the part that actually differs each time —
// is what the browser is then left doing.
//
// Kept host-side rather than in the browser's storage because these name
// directories on *this machine*: they must survive a different browser, a
// cleared profile, or the tab being opened from another device entirely, which
// is exactly when re-deriving them by hand is most annoying.

/// Path to the file holding the favourite folder list — a plain JSON array of
/// absolute paths, editable by hand if it ever comes to that.
fn favourites_path() -> PathBuf {
    state_dir().join("favourites.json")
}

/// Upper bound on how many folders can be starred. The chips share one row
/// under the picker; past a dozen the row is a wall of near-identical names and
/// picking from it is slower than browsing was.
pub(crate) const MAX_FAVOURITES: usize = 12;

/// The starred folders, in the order they were added. Unreadable or corrupt
/// state reads as "none": the picker still works, it just starts at `$HOME`.
pub(crate) fn favourite_folders() -> Vec<String> {
    favourites_in(&favourites_path())
}

/// Star or unstar `path`, returning the list as it now stands.
pub(crate) fn set_favourite_folder(path: &str, favourite: bool) -> Result<Vec<String>> {
    set_favourite_in(&favourites_path(), path, favourite)
}

// The two above are the whole API; the two below take the state file as an
// argument so they can be tested without a process-global `$HOME`.

fn favourites_in(file: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .unwrap_or_default()
}

/// Star or unstar `path` in `file`, returning the list as it now stands.
///
/// Adding canonicalizes first — `~/dev`, `~/dev/`, and a symlink to it are one
/// favourite, not three, and the stored path is the one `/api/fs` will report
/// when browsing there, so the star lights up on arrival. Removing deliberately
/// does *not*: a folder that has been deleted or lives on an unplugged drive
/// can't be canonicalized, and un-starring it is precisely what you want to do.
fn set_favourite_in(file: &std::path::Path, path: &str, favourite: bool) -> Result<Vec<String>> {
    let raw = path.trim();
    if raw.is_empty() {
        bail!("no folder given");
    }
    let mut list = favourites_in(file);

    if !favourite {
        let canonical = std::fs::canonicalize(raw)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        list.retain(|p| p != raw && (canonical.is_empty() || p != &canonical));
    } else {
        let resolved =
            std::fs::canonicalize(raw).with_context(|| format!("cannot star '{raw}'"))?;
        if !resolved.is_dir() {
            bail!("'{raw}' is not a folder");
        }
        let resolved = resolved.to_string_lossy().into_owned();
        if !list.contains(&resolved) {
            if list.len() >= MAX_FAVOURITES {
                bail!(
                    "you already have {MAX_FAVOURITES} favourite folders — unstar one to add another"
                );
            }
            list.push(resolved);
        }
    }

    let body = serde_json::to_string_pretty(&list)?;
    std::fs::write(file, body).with_context(|| format!("could not write {}", file.display()))?;
    Ok(list)
}

/// Resolves `--add-sandbox`: if a name was already given, or the flag wasn't
/// passed, returns `name` untouched (so the caller's existing "no name →
/// web-only daemon" behaviour is unaffected). Otherwise derives one from the
/// workspace directory so a developer never has to type or remember a name.
fn resolve_up_name(
    name: Option<String>,
    path: &Option<PathBuf>,
    add_sandbox: bool,
) -> Result<Option<String>> {
    if name.is_some() || !add_sandbox {
        return Ok(name);
    }
    let workspace = match path {
        Some(p) => p.clone(),
        None => std::env::current_dir()?,
    };
    let workspace = std::fs::canonicalize(&workspace)
        .with_context(|| format!("workspace path does not exist: {}", workspace.display()))?;
    let derived = derive_sandbox_name(&workspace);
    eprintln!("sbxw  --add-sandbox → using sandbox name '{derived}'");
    Ok(Some(derived))
}

/// Turns a workspace path into a sandbox name: the sanitized directory
/// basename, deduplicated against `workspace_for`'s records so two different
/// paths never silently share (and fight over) the same sandbox.
///
/// A name already recorded for *this* path is a cache hit, not a clash — that
/// is `sbxw up`'s normal reuse-if-exists case, so the plain base name comes
/// back unchanged. A name recorded for a *different* path is a clash, walked
/// through `<base>-copy`, `<base>-copy-1`, `<base>-copy-2`, ... until a free
/// or matching one turns up. `workspace_for` can lag a removed sandbox (`sbxw
/// rm` doesn't clear the record), so this occasionally skips a name that is
/// actually free again — harmless, since `sbx create --name` on a free name
/// just creates fresh under whatever we picked.
fn derive_sandbox_name(workspace: &Path) -> String {
    let base = workspace
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "sandbox".to_string());
    let base = sanitize_sandbox_name_component(&base);

    let mut candidate = base.clone();
    let mut clashes = 0u32;
    loop {
        match workspace_for(&candidate) {
            None => return candidate,
            Some(existing) if existing == workspace => return candidate,
            Some(_) => {
                clashes += 1;
                candidate = if clashes == 1 {
                    format!("{base}-copy")
                } else {
                    format!("{base}-copy-{}", clashes - 1)
                };
            }
        }
    }
}

/// Replaces anything outside `is_valid_sandbox_name`'s alphabet with `-`, so
/// a derived name is always accepted without the caller re-checking it.
fn sanitize_sandbox_name_component(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "sandbox".to_string()
    } else {
        cleaned
    }
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

/// Host half of a `web_addr` ("127.0.0.1:7681" → "127.0.0.1").
///
/// The IPv6 lesson `web_port_of` learned, applied to the other half: this used
/// to be `split(':').next()` written out at three call sites, which on
/// `[::1]:7681` returns `"["`. Nothing crashed — `"[".starts_with("127.")` is
/// simply false, so the `sbxw.localhost` alias was never added and the web UI
/// quietly had no hostname.
///
/// Splitting at the *last* colon leaves the brackets on, so they come off here;
/// what the callers want is something they can compare against `127.` and put
/// in `/etc/hosts`.
fn web_ip_of(addr: &str) -> &str {
    match addr.rsplit_once(':') {
        Some((host, _)) => host.trim_start_matches('[').trim_end_matches(']'),
        None => addr,
    }
}

/// Every hostname sbxw wants in `/etc/hosts`: the configured port aliases, the
/// web UI's own name when it is on loopback, and anything a previous run parked
/// because it could not get root.
///
/// One assembly, because there were three — `cmd_up_background`, `hosts sync`
/// and `provision_sandbox` each built the list inline, and the rule about which
/// web address earns an alias had to be right in all three.
fn wanted_aliases(cfg: &Config, extra_ports: &[ExtraPort]) -> Vec<HostAlias> {
    let mut aliases = host_aliases(&merged_ports(cfg, extra_ports), cfg.ip_per_app);
    let web_ip = web_ip_of(&cfg.web_addr);
    // Only a loopback address gets the name: `sbxw.localhost` pointing at a LAN
    // IP would be a claim about somebody else's machine.
    if web_ip.starts_with("127.") {
        aliases.push(HostAlias {
            hostname: "sbxw.localhost".into(),
            ip: web_ip.to_string(),
        });
    }
    aliases.extend(pending_aliases());
    aliases
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

/// Where aliases that could not be written to /etc/hosts are parked until
/// somebody runs `sbxw hosts sync` from a terminal. One `<ip>\t<hostname>` per
/// line, same shape as the /etc/hosts block itself.
fn pending_hosts_path() -> PathBuf {
    state_dir().join("pending-hosts")
}

/// Remember aliases the daemon wanted but could not write, so the recovery
/// command can apply them without having to guess: ports added from the web UI
/// exist in no config file, and would otherwise be lost with the error message.
pub(crate) fn remember_pending_aliases(aliases: &[HostAlias]) {
    let mut lines: Vec<String> = std::fs::read_to_string(pending_hosts_path())
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .filter(|l| !l.trim().is_empty())
        .collect();
    for a in aliases {
        if a.hostname.is_empty() {
            continue;
        }
        let line = format!("{}\t{}", a.ip, a.hostname);
        if !lines.contains(&line) {
            lines.push(line);
        }
    }
    let _ = std::fs::create_dir_all(state_dir());
    let _ = std::fs::write(pending_hosts_path(), lines.join("\n") + "\n");
}

/// Read back what `remember_pending_aliases` parked.
fn pending_aliases() -> Vec<HostAlias> {
    std::fs::read_to_string(pending_hosts_path())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            match (parts.next(), parts.next()) {
                (Some(ip), Some(hostname)) => Some(HostAlias {
                    ip: ip.to_string(),
                    hostname: hostname.to_string(),
                }),
                _ => None,
            }
        })
        .collect()
}

/// Put `aliases` into /etc/hosts (and on lo0, in ip-per-app mode), returning a
/// warning per step that did not work instead of an error.
///
/// Both steps need root, and the web daemon has no terminal to answer a
/// password prompt on: once sudo's cached password expires — minutes after the
/// install that asked for it — every privileged step it tries fails. So a
/// failure here is reported and parked for `sbxw hosts sync`, never fatal.
/// `hosts::merge_hosts_block` also keeps entries other sandboxes put there, so
/// in the steady state there is nothing to write and no sudo to need.
pub(crate) fn apply_host_aliases(aliases: &[HostAlias]) -> Vec<String> {
    let mut warnings = Vec::new();
    if let Err(e) = hosts::ensure_loopback_aliases(aliases) {
        warnings.push(format!("{e:#}"));
    }
    if let Err(e) = hosts::merge_hosts_block(aliases) {
        warnings.push(format!("/etc/hosts not updated: {e:#}"));
    }
    let missing = hosts::missing_aliases(aliases);
    if !missing.is_empty() {
        remember_pending_aliases(aliases);
        if warnings.is_empty() {
            // Write reported success yet the entries aren't there — worth its
            // own line, since the cause isn't the one everything else is.
            warnings.push(format!(
                "/etc/hosts write reported success but {} is still missing — \
                 run `sbxw hosts sync` in a terminal",
                missing.join(", ")
            ));
        }
        warnings.push(format!(
            "these hostnames won't resolve yet: {} (run `sbxw hosts sync` in a terminal)",
            missing.join(", ")
        ));
    }
    for w in &warnings {
        tracing::warn!("{w}");
    }
    warnings
}

/// `sbxw hosts [sync|show|clear]` — the terminal-side half of alias handling.
///
/// The point of `sync` is that it runs where a human is: `sudo` may prompt,
/// which the daemon can never let it do. It applies the config's aliases plus
/// anything a daemon parked in `pending-hosts`, then forgets the parked list.
fn cmd_hosts(action: HostsAction, config: PathBuf) -> Result<()> {
    match action {
        HostsAction::Show => {
            let entries = hosts::read_hosts_block();
            if entries.is_empty() {
                println!("no sbxw entries in /etc/hosts");
            }
            for a in &entries {
                println!("{}\t{}", a.ip, a.hostname);
            }
            let pending = pending_aliases();
            if !pending.is_empty() {
                println!("\npending (run `sbxw hosts sync` to apply):");
                for a in &pending {
                    println!("{}\t{}", a.ip, a.hostname);
                }
            }
            Ok(())
        }
        HostsAction::Clear => {
            hosts::clear_hosts_block()?;
            let _ = std::fs::remove_file(pending_hosts_path());
            println!("removed the sbxw /etc/hosts block");
            Ok(())
        }
        HostsAction::Sync => {
            let cfg = Config::load_or_default(&config)?;
            let aliases = wanted_aliases(&cfg, &[]);
            hosts::ensure_loopback_aliases(&aliases)?;
            hosts::merge_hosts_block(&aliases)?;
            let missing = hosts::missing_aliases(&aliases);
            if !missing.is_empty() {
                anyhow::bail!("still missing from /etc/hosts: {}", missing.join(", "));
            }
            // Only now: the parked list is the only record of UI-added aliases.
            let _ = std::fs::remove_file(pending_hosts_path());
            for a in &aliases {
                println!("{}\t{}", a.ip, a.hostname);
            }
            println!("/etc/hosts is up to date");
            Ok(())
        }
    }
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
/// re-applying kits, so a false negative costs a redundant re-apply, nothing more.
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

/// The part of `sbx inspect` output that lists the sandbox's kits, or the whole
/// blob when no such part can be identified.
///
/// The kit-skip test is a substring search, and `inspect` also reports the sandbox's
/// **custom secrets** to what `inspect` reports. Searching the whole blob for a
/// kit name therefore now collides with a secret (or any other field) that
/// merely contains it, and the failure is the silent kind: sbxw concludes the
/// kit is already applied and never applies it. Narrowing the haystack to the
/// kits section fixes that.
///
/// It degrades rather than guessing: `inspect`'s exact layout is not pinned by
/// the CLI reference and may well be a table with a KITS *column*, where no
/// section exists to find. In that case the caller gets the full text and the
/// old behaviour — never worse than today, better wherever the section is
/// recognisable.
fn inspect_kits_section(raw: &str) -> String {
    if let Some(section) = json_kits_section(raw).or_else(|| text_kits_section(raw)) {
        return section;
    }
    tracing::debug!("`sbx inspect` has no recognisable kits section; matching the whole output");
    raw.to_string()
}

/// `kits` out of a JSON `sbx inspect`, serialized back to text to be searched.
fn json_kits_section(raw: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let obj = value.as_object()?;
    let kits = obj
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("kits") || k.eq_ignore_ascii_case("kit"))?
        .1;
    Some(kits.to_string())
}

/// The `Kits:` block of a plain-text `sbx inspect`: the remainder of the label
/// line plus every following line indented deeper than it, which is how nested
/// values are set off from the next top-level field.
fn text_kits_section(raw: &str) -> Option<String> {
    let indent = |l: &str| l.len() - l.trim_start().len();

    let mut lines = raw.lines();
    let (label_indent, first) = lines.find_map(|line| {
        let trimmed = line.trim_start();
        let (label, rest) = trimmed.split_once(':')?;
        let label = label.trim_end();
        // "Kits", "kits", "Kit" — but not "Kit sources" or a sentence that
        // happens to open with those letters.
        (label.eq_ignore_ascii_case("kits") || label.eq_ignore_ascii_case("kit"))
            .then(|| (indent(line), rest.trim().to_string()))
    })?;

    let mut section = vec![first];
    for line in lines {
        if !line.trim().is_empty() && indent(line) <= label_indent {
            break;
        }
        section.push(line.trim().to_string());
    }
    Some(section.join("\n"))
}

/// Apply a bring-up network rule, surviving a host whose organisation owns it.
///
/// `policy allow|deny network` refuses with "managed by your
/// organization" when org governance overrides the local policy. That is a
/// correctly configured host, not a broken one: the org's rules are already in
/// force and are the ones that count. Failing `sbxw up` over it would leave
/// governed users unable to start a sandbox sbx itself considers fine, so the
/// refusal is a warning — and every *other* failure still aborts, because a
/// sandbox whose egress silently didn't apply is the surprise this check exists
/// to prevent.
fn apply_bringup_policy(kind: &str, result: Result<()>) -> Result<()> {
    let Err(e) = result else { return Ok(()) };

    let msg = format!("{e:#}");
    let lower = msg.to_lowercase();
    if lower.contains("managed by your organization")
        || lower.contains("managed by your organisation")
    {
        tracing::warn!(
            "network {kind} not applied — your organization manages this policy; \
             the sandbox runs under the org's rules instead:\n{msg}"
        );
        return Ok(());
    }
    Err(e).with_context(|| format!("failed to apply network {kind}"))
}

/// The path of a *stale sbxw OAuth kit* named in a failed `sbx kit add`, if the
/// failure is that one and nothing else.
///
/// Sandboxes created before the kit directory became durable recorded a path in
/// the OS temp dir that sbxw then deleted. Every container swap
/// re-resolves it, so those sandboxes reject **all** later `kit add` calls,
/// permanently, with the unrelated kit named as the thing that failed.
///
/// The path is recoverable from sbx's own message, so the repair is to put the
/// spec back where the sandbox is looking for it. Deliberately narrow — the
/// path must carry sbxw's own `sbxw-oauth-kit-` basename, must be absent, and
/// its parent must already exist — because this writes to a path parsed out of
/// a subprocess's stderr. Anything else is reported, not repaired.
fn stale_oauth_kit_path(err: &str) -> Option<PathBuf> {
    if !err.contains("does not exist") {
        return None;
    }
    err.split('"')
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("sbxw-oauth-kit-"))
                && !p.exists()
                && p.parent().is_some_and(|parent| parent.is_dir())
        })
}

/// `sbx kit add`, repairing a stale sbxw OAuth kit reference once and retrying.
///
/// Without the retry the user sees a kit they just configured fail to apply and
/// an error naming a temp directory they have never heard of, with no way back
/// short of recreating the sandbox.
fn kit_add_repairing_oauth(name: &str, kit: &str, credentials_json: Option<&str>) -> Result<()> {
    let Err(first) = sbx::kit_add(name, kit) else {
        return Ok(());
    };
    let Some(stale) = stale_oauth_kit_path(&format!("{first:#}")) else {
        return Err(first);
    };

    tracing::warn!(
        "sandbox '{name}' still refers to an OAuth kit sbxw used to delete after \
         creation ({}); restoring it so the sandbox can be recomposed",
        stale.display()
    );

    // No token this run: restore a kit that merely *resolves*. It keeps the
    // same name and its claude.ai rule, and leaves the credentials already in
    // the container alone — which a fresh spec with no token would too, but
    // this way nothing here depends on having one.
    let spec = match credentials_json {
        Some(creds) => oauth_kit_spec(creds),
        None => oauth_kit_spec_without_credentials(),
    };
    std::fs::create_dir_all(&stale)
        .with_context(|| format!("could not restore OAuth kit at {}", stale.display()))?;
    restrict_permissions(&stale, 0o700);
    let spec_path = stale.join("spec.yaml");
    std::fs::write(&spec_path, spec)
        .with_context(|| format!("could not write {}", spec_path.display()))?;
    restrict_permissions(&spec_path, 0o600);

    sbx::kit_add(name, kit).with_context(|| {
        format!(
            "retried after restoring the stale OAuth kit at {}; the first attempt failed with: {first:#}",
            stale.display()
        )
    })
}

/// Does this kit declare startup commands — `setup.startup` in spec v2, or
/// `commands.startup` in the v1 a third-party kit may still be written in?
///
/// Both spellings are accepted *on read* even though sbxw only ever writes v2:
/// this reads a file somebody else may own, and the question is what the kit
/// does, not which grammar its author chose.
///
/// It matters because `kit add` **refuses** such a kit: the recreate
/// flow behind it does not run startup commands, so rather than apply the kit
/// half-way it tells you to recreate the sandbox with `sbx create --kit`. All
/// three kits sbxw ships declare startup commands — that is how they install
/// anything — so this is the common case, not a corner one.
///
/// Only directory kits can be read; a ZIP or an OCI reference answers `false`
/// and the refusal is discovered from sbx's own error instead.
fn kit_declares_startup(kit: &str) -> bool {
    let Ok(spec) = std::fs::read_to_string(std::path::Path::new(kit).join("spec.yaml")) else {
        return false;
    };
    spec.lines().any(|l| l.trim() == "startup:")
}

/// Is this failed `sbx kit add` the "recreate the sandbox instead" refusal?
fn kit_add_needs_recreate(err: &str) -> bool {
    let lower = err.to_lowercase();
    lower.contains("does not yet apply") || lower.contains("recreate the sandbox from scratch")
}

/// Explain the one thing the user has to do, since sbxw will not do it for
/// them: recreating destroys a sandbox, which is not a call a provisioning step
/// gets to make silently.
fn warn_kit_needs_recreate(name: &str, kit: &str, sbx_said: Option<&str>) {
    let detail = sbx_said.map(|s| format!("\n{s}")).unwrap_or_default();
    tracing::warn!(
        "kit '{kit}' declares startup commands, which `sbx kit add` cannot apply to an \
         existing sandbox — '{name}' is unchanged. Recreate it to pick the kit \
         up: `sbxw rm {name}` then `sbxw up {name}`, which passes every configured kit to \
         `sbx create --kit`. Anything outside the workspace mount is lost in the process.{detail}"
    );
}

/// Warn when a kit reference is one sbx will not fetch without configuration.
fn warn_if_kit_source_needs_allowlist(kit: &str) {
    if kit_needs_allowlist(kit) {
        tracing::warn!(
            "kit '{kit}' is a Git URL or non-Docker Hub registry — sbx now restricts kit \
             sources to Docker Hub by default. Run `sbx settings set kit.allowedSources \
             <prefix>` to allow it."
        );
    }
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
) -> Result<Vec<String>> {
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

    let env_pairs = cfg.env_pairs();

    // 2. Create the sandbox if it doesn't exist yet.
    let existed = sbx::exists(name)?;
    // Set when creation carried the configured kits, so the `kit add` loop
    // below has nothing left to do.
    let mut created_with_kits = false;
    if existed {
        tracing::info!("sandbox '{name}' already exists — reusing it");
        // An existing sandbox keeps what was *baked in* at creation, but the
        // same flags go onto every `sbx run` attach (see `sbx::run_attach_args`)
        // and apply to the agent session — so an edited `env` reaches the agent
        // without recreating anything. Only a non-agent process (the Bash pane,
        // `web_shell`) is still on the creation-time set.
        if !env_pairs.is_empty() || !cfg.env_files.is_empty() {
            tracing::info!(
                "'{name}' already exists: sbxw.toml's env reaches the agent on attach, but \
                 the sandbox itself keeps what it was created with — a Bash pane won't see \
                 a variable added since. `sbxw rm {name}` and bring it up again to bake in \
                 the current set."
            );
        }
        if let Some(ref creds) = credentials_json {
            if sbx::is_running(name).unwrap_or(false) {
                // Running sandbox: refresh the credentials file in place over
                // `sbx exec`: `kit add` recreates the sandbox
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
                // `kit add`. The container re-creation this triggers preserves
                // state, and nothing is attached to a stopped sandbox anyway.
                tracing::info!("applying OAuth kit to existing (stopped) sandbox via kit add");
                match write_oauth_kit(name, creds) {
                    Ok(dir) => {
                        // Kept on disk: sbx re-resolves this path on every
                        // later container swap (see `oauth_kit_dir`).
                        if let Err(e) =
                            kit_add_repairing_oauth(name, &dir.to_string_lossy(), Some(creds))
                        {
                            tracing::warn!(
                                "OAuth kit add failed (use /login in-session instead): {e:#}"
                            );
                        }
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
        let kit_dir = match credentials_json
            .as_deref()
            .map(|creds| write_oauth_kit(name, creds))
        {
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
        // Every kit at once, OAuth first: creation is the only moment sbx
        // applies a kit whole (see `sbx::CreateOpts::kits`). Doing it here also
        // means a fresh sandbox is never recreated by a follow-up `kit add`.
        let mut kits: Vec<String> = kit_dir
            .as_deref()
            .map(|d| d.to_string_lossy().into_owned())
            .into_iter()
            .collect();
        kits.extend(cfg.kits.iter().cloned());
        for kit in &cfg.kits {
            warn_if_kit_source_needs_allowlist(kit);
        }

        // Every kit goes in here or the create fails outright. `--kit` is
        // repeatable and documented as such, so there is nothing left to hedge
        // against — sbxw used to retry with only the credentials kit because it
        // could not be sure older releases composed several.
        create_with_port_fallback(
            name,
            &sbx::CreateOpts {
                workspace,
                ro_mounts: ro_strs,
                kits: &kits,
                publish: &port_specs,
                share_skills: cfg.share_skills,
                env: &env_pairs,
                env_files: &cfg.env_files,
            },
        )?;
        created_with_kits = true;
        // The kit directory deliberately stays: sbx recorded its *path*, and
        // re-resolves it on every later container swap (see `oauth_kit_dir`).
        // `sbxw rm` is what cleans it up.
        drop(kit_dir);
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
        // 2c-ter. The cross-sandbox relay: a CLI this agent can run to ask
        // *another* sandbox's agent something, with a human routing the question
        // and releasing the answer (see `assets/relay-tool.js` and `/api/relay/*`).
        // Best-effort — without it the agent simply has no one to ask.
        if let Err(e) = sbx::install_relay_tool(name, web_port) {
            tracing::warn!("could not install the cross-sandbox relay: {e:#}");
        }
        // The hook reaches the host daemon via host.docker.internal, but the
        // proxy classifies that destination as `localhost:<port>` — so the
        // allow rule must name the loopback host and port, not the DNS alias.
        let hook_dest = format!("localhost:{web_port}");
        if let Err(e) = sbx::policy_allow_network(Some(name), &hook_dest) {
            tracing::warn!("could not allow {hook_dest} egress for hooks: {e:#}");
        }
        // 2d. The default model rides in as `ANTHROPIC_DEFAULT_MODEL` with the
        // rest of the environment (see `Config::effective_env`), so there is
        // nothing to install here. What is left is undoing the *old* mechanism:
        // sbxw used to write `model` into settings.json, and that key outranks
        // the variable — a sandbox carrying one would ignore `claude_model`
        // forever. Only sbxw's own leftover is removed; a `/model` choice that
        // says something else stays.
        let marker = model_migrated_marker(name);
        if !cfg.claude_model.is_empty() && !marker.exists() {
            match sbx::drop_stale_settings_model(name, &cfg.claude_model) {
                // Recorded so the next bring-up spends nothing on it. A missing
                // marker only ever costs a repeat of an idempotent no-op.
                Ok(()) => {
                    let _ = std::fs::write(&marker, "");
                }
                Err(e) => tracing::warn!(
                    "could not clear the old settings.json model key: {e:#} — if this \
                     sandbox predates ANTHROPIC_DEFAULT_MODEL it may stay on its old model"
                ),
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
        apply_bringup_policy(
            "allowlist",
            sbx::policy_allow_network(Some(name), &resources),
        )?;
    }
    if !cfg.network_deny.is_empty() {
        let resources = cfg.network_deny.join(",");
        tracing::info!("network denylist: {resources}");
        apply_bringup_policy("denylist", sbx::policy_deny_network(Some(name), &resources))?;
    }

    // 3b. User-defined kits from sbxw.toml, for a sandbox that already existed.
    //     A sandbox sbxw just created already has them — they went in as
    //     `sbx create --kit` (see `created_with_kits`), which is the only path
    //     that applies a kit whole.
    //
    //     Adding one to an existing sandbox goes through `sbx kit add`, which
    //     RECREATES the container (state preserved, the kit's own network rules
    //     composed in), so re-applying on every `sbxw up` is not free: `sbx
    //     inspect` lists the sandbox's kits and the ones it already names are
    //     skipped. Whenever inspect yields nothing usable, every kit is applied
    //     — the safe direction. The match is scoped to inspect's kits section,
    //     because inspect also reports the sandbox's custom secrets and a kit
    //     name appearing in one of those would skip a kit never applied.
    //
    //     Runs AFTER network policy so kit startup commands have egress access.
    //     A kit reference is a directory (with spec.yaml), ZIP, or OCI ref (docker.io by default).
    //     Git URLs and non-Docker Hub OCI refs require: sbx settings set kit.allowedSources <prefix>
    let pending_kits: &[String] = if created_with_kits { &[] } else { &cfg.kits };
    let inspect_out = if !pending_kits.is_empty() {
        inspect_kits_section(&sbx::inspect_raw(name).unwrap_or_default())
    } else {
        String::new()
    };
    for kit in pending_kits {
        warn_if_kit_source_needs_allowlist(kit);
        let kit_name = kit_display_name(kit);
        if kit_name.len() >= 3 && inspect_out.contains(&kit_name) {
            tracing::info!(
                "kit '{kit}' already applied (listed by `sbx inspect`) — skipping; \
                 run `sbx kit add {name} {kit}` to force a re-apply"
            );
            continue;
        }
        // `kit add` refuses a kit declaring startup commands outright,
        // and the refusal comes *after* it has swapped the container. When the
        // spec is readable we know that in advance, so don't put the sandbox
        // through a recreate that cannot succeed.
        if kit_declares_startup(kit) {
            warn_kit_needs_recreate(name, kit, None);
            continue;
        }
        tracing::info!("applying kit: {kit} (this recreates the container; state is kept)");
        match kit_add_repairing_oauth(name, kit, credentials_json.as_deref()) {
            Ok(()) => {}
            // Same refusal, for a kit whose spec sbxw could not read (a ZIP or
            // an OCI reference) — sbx is the one that knows, so it says so.
            Err(e) if kit_add_needs_recreate(&format!("{e:#}")) => {
                warn_kit_needs_recreate(name, kit, Some(&format!("{e:#}")));
            }
            Err(e) => tracing::warn!("kit '{kit}' failed to apply: {e:#}"),
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
    let aliases = wanted_aliases(cfg, extra_ports);
    // Non-fatal on purpose: the sandbox exists and works by now, and /etc/hosts
    // is a convenience on top of it. Failing here used to abort provisioning
    // *after* the sandbox was created — the web UI then saw an error, never
    // attached the new sandbox to a pane, and left it sitting in the sidebar.
    let warnings = apply_host_aliases(&aliases);
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

    Ok(warnings)
}

/// Where an exported `.sbxenv.yaml` goes when `-o` doesn't say: the workspace's
/// **parent**, not the workspace.
///
/// sbx is explicit about this and the reason is not tidiness. The agent can
/// write anywhere in a direct-mounted workspace, so an environment file inside
/// one is a file the agent can rewrite — and it is the file that decides what
/// the *next* sandbox gets: which kits, which mounts, which ports. Outside, it
/// is host-side configuration the sandbox cannot reach.
///
/// (sbxw.toml has the same shape of exposure and sbxw does not move it, because
/// it is a project's own file and moving it would break every existing setup.
/// It is worth knowing about: an agent that edits `network_allow` widens its own
/// egress on your next `sbxw up`. The README says so too.)
fn default_env_export_path(workspace: &Path) -> PathBuf {
    workspace.parent().unwrap_or(workspace).join(".sbxenv.yaml")
}

/// `sbxw env run` — create from a `.sbxenv.yaml`, then run sbxw's own pipeline
/// over the result.
///
/// The division of labour is the point. `sbx env create` is the only thing that
/// can apply an environment file whole — secrets, bindings, registries, MCP
/// servers, kits, mounts — so it does the creation. Everything sbxw adds and
/// the format cannot express (egress policy, OAuth credentials, hooks, the
/// `/etc/hosts` aliases, the browser terminal) comes after, through the same
/// `provision_sandbox` every other entry point uses, on a sandbox that by then
/// already exists.
///
/// Ports are settled **before** sbx is called, never after. A port sbx cannot
/// publish doesn't cost you the port, it costs you the sandbox — creation fails
/// and the new sandbox is removed — while the secrets it provisioned first stay
/// behind. And there is no way to fix it from the outside: `env create` has no
/// port flag, and a second file can only *add* ports because the merge
/// concatenates lists. So the file itself is rewritten, into a scratch copy,
/// with every path made absolute so it can live outside the original directory.
fn cmd_env_run(
    paths: Vec<PathBuf>,
    config: PathBuf,
    no_web: bool,
    yes: bool,
    use_api_key: bool,
    tail: bool,
) -> Result<()> {
    // `assert_available` already refuses anything below the floor, and `sbx env`
    // arrived in that same release — so there is nothing extra to check here.
    sbx::assert_available()?;

    let cwd = std::env::current_dir()?;
    let files = envfile::resolve_paths(&paths, &cwd)?;
    let mut loaded = envfile::load(&files, &|name| std::env::var(name).ok())?;
    let spec = loaded.spec()?;

    for name in &loaded.unresolved {
        eprintln!(
            "sbxw  warning: ${{{name}}} has no value on this host — left as written, so sbx \
             will see it unresolved too"
        );
    }

    // Paths in the file are relative to the *first* file's directory, which is
    // sbx's rule and therefore sbxw's. Owned, because the document it came from
    // is rewritten further down.
    let base = loaded.base_dir.clone();
    let workspace = match spec.workspace.as_deref() {
        Some(w) => {
            let p = Path::new(w);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                base.join(p)
            }
        }
        None => base.clone(),
    };
    let workspace = std::fs::canonicalize(&workspace).with_context(|| {
        format!(
            "the environment file's workspace does not exist: {}",
            workspace.display()
        )
    })?;
    let agent = spec.agent.clone().unwrap_or_else(|| "claude".into());

    // sbx would default the name to `<agent>-<basename>`; sbxw pins it instead,
    // so finding the sandbox afterwards is a lookup and not a re-derivation of
    // whatever sanitising sbx applies.
    let name = match spec.name.clone() {
        Some(n) => n,
        None => {
            let base_name = workspace
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "sandbox".into());
            sanitize_sandbox_name_component(&format!("{agent}-{base_name}"))
        }
    };
    if !is_valid_sandbox_name(&name) {
        bail!(INVALID_NAME_MSG);
    }

    println!(
        "sbxw  {} → sandbox '{name}'",
        display_sources(&loaded.sources)
    );

    // ── Creation, once, and only if there is nothing there yet ───────────────
    if sbx::exists(&name)? {
        println!(
            "sbxw  '{name}' already exists — not re-created. sbx applies an environment \
             file's workspaces, kits, ports and secrets at creation only, so `sbxw rm {name}` \
             first if the file has changed in those."
        );
    } else {
        let (ports, moved) = negotiate_ports(&spec.ports, !yes)?;
        let pinned_name = spec.name.is_none();
        let rewrite = moved || pinned_name;

        let create_args: Vec<String> = if rewrite {
            loaded.set_ports(&ports);
            if pinned_name {
                loaded.set_name(&name);
            }
            // The scratch copy lives outside `base`, so every relative path in
            // it has to be absolute or it would resolve against the wrong
            // directory. The kit resolver is the same one `sbxw.toml` uses, so
            // an OCI reference stays a reference.
            loaded.set_workspace(&workspace.to_string_lossy());
            let mounts: Vec<envfile::MountSpec> = spec
                .additional
                .iter()
                .map(|m| envfile::MountSpec {
                    path: base.join(&m.path).to_string_lossy().into_owned(),
                    read_only: m.read_only,
                })
                .collect();
            loaded.set_additional(&mounts);
            let kits: Vec<String> = spec
                .kits
                .iter()
                .map(|k| resolve_kit_ref(&base, k.clone()))
                .collect();
            loaded.set_kits(&kits);

            let dir = env_state_dir(&name);
            std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
            let derived = dir.join(".sbxenv.yaml");
            std::fs::write(&derived, loaded.to_yaml()?)
                .with_context(|| format!("writing {}", derived.display()))?;
            if moved {
                println!(
                    "sbxw  ports adjusted — running from a rewritten copy at {}",
                    derived.display()
                );
            }
            vec![derived.to_string_lossy().into_owned()]
        } else {
            files
                .iter()
                .map(|f| f.to_string_lossy().into_owned())
                .collect()
        };

        if !spec.delegated.is_empty() {
            println!(
                "sbxw  handing {} to sbx — sbxw does not read those",
                spec.delegated.join(", ")
            );
        }
        sbx::env_create(&create_args)?;

        // The name is pinned into the document precisely so this holds. If it
        // doesn't, stopping here is the only safe move: the provisioning pass
        // below creates a sandbox when it can't find one, and it would use
        // `sbx create` — a *second* sandbox, without the secrets, bindings and
        // MCP servers the environment file just provisioned into the first.
        if !sbx::exists(&name)? {
            bail!(
                "`sbx env create` reported success but no sandbox named '{name}' exists. \
                 Check `sbx ls`: if it created one under another name, remove it and give \
                 the environment file an explicit `name:` matching it."
            );
        }
    }

    // ── Everything the format cannot carry ───────────────────────────────────
    // The ports the *sandbox* has, not the ones the file asked for. They differ
    // whenever a busy host port was renegotiated above, and whenever the file
    // asked for an ephemeral one — whose value only exists once sbx has
    // published it. Getting this wrong points an `/etc/hosts` alias at a port
    // nothing is listening on, which looks exactly like a broken dev server.
    let published: Vec<envfile::Port> = sbx::list_ports_parsed(&name)
        .into_iter()
        .map(|m| envfile::Port {
            sandbox: m.sandbox_port,
            host: Some(m.host_port),
            host_ip: (!m.host_ip.is_empty()).then_some(m.host_ip),
            protocol: (!m.proto.is_empty() && m.proto != "tcp").then_some(m.proto),
        })
        .collect();
    let effective = envfile::Spec {
        ports: if published.is_empty() {
            // Nothing to read back — an sbx that reports ports differently, or
            // a sandbox with none. The file's own list is the best guess left.
            spec.ports.clone()
        } else {
            published
        },
        ..spec
    };

    let (cfg, borrowed) = config_from_spec(&effective, &base, load_config(&config)?);
    if !borrowed.is_empty() {
        let from = if config.exists() {
            config.display().to_string()
        } else {
            "sbxw's defaults".into()
        };
        println!("sbxw  taken from {from}:");
        for line in &borrowed {
            println!("        · {line}");
        }
    }

    // A config of its own on disk, because the web daemon is a separate process
    // that re-reads one: it has to see the same merge this run computed, not
    // the sbxw.toml the merge was only half of.
    let merged_path = env_state_dir(&name).join("sbxw.toml");
    if let Some(parent) = merged_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&merged_path, cfg.to_toml()?)
        .with_context(|| format!("writing {}", merged_path.display()))?;

    if no_web {
        init_tracing();
        cmd_up(
            Some(name),
            Some(workspace),
            vec![],
            merged_path,
            true,
            use_api_key,
        )
    } else {
        cmd_up_background(
            Some(name),
            Some(workspace),
            vec![],
            merged_path,
            use_api_key,
            tail,
        )
    }
}

/// The environment files a run was built from, as one short line.
fn display_sources(sources: &[PathBuf]) -> String {
    sources
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(" + ")
}

/// Fold an environment file's spec onto `sbxw.toml`, and say what came from
/// where.
///
/// The rule you chose: **the environment file wins, `sbxw.toml` is the
/// fallback.** It applies field by field, because the two files describe
/// overlapping but unequal things — the format has ports and env, it has no
/// egress allowlist, no `/etc/hosts` alias and no model. So a key the
/// environment file doesn't mention isn't "unset", it is delegated downwards.
///
/// The returned report is not decoration. Two files describing one sandbox is
/// exactly the setup where somebody edits the wrong one and watches nothing
/// happen; printing the borrowed values at creation time is what makes that
/// visible on the run where it matters.
fn config_from_spec(spec: &envfile::Spec, base: &Path, mut cfg: Config) -> (Config, Vec<String>) {
    let mut report = Vec::new();

    if spec.ports.is_empty() {
        if !cfg.ports.is_empty() {
            report.push(format!(
                "ports ({}) — the environment file declares none",
                cfg.ports
                    .iter()
                    .map(|p| format!("{}:{}", p.host_port, p.sandbox_port))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    } else {
        // An alias is the one thing the format cannot carry, so it is looked up
        // by sandbox port rather than lost: a file that publishes 4200 gets
        // `neos.local` back if sbxw.toml called 4200 that.
        let mut borrowed = Vec::new();
        cfg.ports = spec
            .ports
            .iter()
            .map(|p| {
                let alias = cfg
                    .ports
                    .iter()
                    .find(|c| c.sandbox_port == p.sandbox)
                    .map(|c| c.alias.clone())
                    .unwrap_or_default();
                if !alias.is_empty() {
                    borrowed.push(format!("{alias} → :{}", p.host.unwrap_or(p.sandbox)));
                }
                config::PortMap {
                    alias,
                    sandbox_port: p.sandbox,
                    // An ephemeral port isn't known until sbx has published it;
                    // the provisioning pass reads the real mapping back.
                    host_port: p.host.unwrap_or(p.sandbox),
                }
            })
            .collect();
        if !borrowed.is_empty() {
            report.push(format!("/etc/hosts aliases ({})", borrowed.join(", ")));
        }
    }

    if !spec.env.is_empty() {
        let overridden: Vec<&String> = spec
            .env
            .keys()
            .filter(|k| cfg.env.contains_key(*k))
            .collect();
        if !overridden.is_empty() {
            report.push(format!(
                "env — the environment file overrides {}",
                overridden
                    .iter()
                    .map(|k| k.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for (k, v) in &spec.env {
            cfg.env.insert(k.clone(), v.clone());
        }
    }

    // Kits are already on the sandbox by the time this config is used — sbx
    // applied them at creation. Handing them to the provisioning pass anyway is
    // deliberate: it skips the ones `sbx inspect` already lists, so this is a
    // no-op that also repairs a sandbox whose kit went missing.
    if !spec.kits.is_empty() {
        // Resolved against the environment file's own directory *before* they
        // are written into the merged config. That config lands in
        // `state/env/<name>/`, and `load_config` re-anchors relative entries to
        // whatever directory it read them from — so a raw `./kits/headroom`
        // would come back as `state/env/<name>/kits/headroom`, a path that has
        // never existed.
        cfg.kits = spec
            .kits
            .iter()
            .map(|k| resolve_kit_ref(base, k.clone()))
            .collect();
    } else if !cfg.kits.is_empty() {
        report.push(format!("kits ({})", cfg.kits.join(", ")));
    }

    if !cfg.network_allow.is_empty() {
        report.push(format!(
            "the egress allowlist ({} rules) — an environment file has no field for it",
            cfg.network_allow.len()
        ));
    }
    if !cfg.network_deny.is_empty() {
        report.push(format!(
            "the egress denylist ({} rules)",
            cfg.network_deny.len()
        ));
    }
    if !cfg.claude_model.is_empty() && !spec.env.contains_key("ANTHROPIC_DEFAULT_MODEL") {
        report.push(format!(
            "claude_model ({}) — passed as ANTHROPIC_DEFAULT_MODEL",
            cfg.claude_model
        ));
    }
    if cfg.ip_per_app {
        report.push("ip_per_app (one loopback IP per app)".into());
    }

    (cfg, report)
}

/// Can this host port still be bound?
///
/// Binding and immediately dropping is racy by nature — something can take the
/// port in the gap before sbx gets there. It is still worth doing, because the
/// race is rare and the thing it prevents is not: an environment file whose
/// port is already taken doesn't cost you the port, it costs you the sandbox
/// (sbx removes it) *and* leaves the secrets it provisioned first behind.
/// Checking first turns the common case from a failed create into a question.
fn host_port_is_free(ip: &str, port: u16) -> bool {
    use std::net::{Ipv4Addr, SocketAddr, TcpListener};
    let addr: SocketAddr = match ip.parse() {
        Ok(ip) => SocketAddr::new(ip, port),
        // An interface sbxw can't parse is one it can't test; assume free and
        // let sbx be the judge.
        Err(_) => SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)),
    };
    TcpListener::bind(addr).is_ok()
}

/// The first free host port at or after `from`, if there is one nearby.
///
/// Bounded rather than open-ended: if a hundred consecutive ports are busy the
/// answer is not the hundred-and-first, it is that something is wrong and a
/// person should look.
fn next_free_host_port(ip: &str, from: u16) -> Option<u16> {
    (from..from.saturating_add(100)).find(|p| host_port_is_free(ip, *p))
}

/// Settle the host ports an environment file asks for, before sbx is called.
///
/// Returns the ports to use and whether anything moved. A port with no `host`
/// is left alone — that is the file asking for an ephemeral port, which cannot
/// collide. A UDP mapping is left alone too: sbxw tests with a TCP bind, and a
/// test that cannot be performed must not be reported as a result.
///
/// With a terminal, each conflict is a question with the next free port as the
/// default. Without one — a daemon, CI, a hook — it takes that port and says
/// so, because the alternative is a run that stops for an answer nobody is
/// there to give.
fn negotiate_ports(
    ports: &[envfile::Port],
    interactive: bool,
) -> Result<(Vec<envfile::Port>, bool)> {
    use std::io::{IsTerminal, Write as _};

    let mut out = Vec::with_capacity(ports.len());
    let mut moved = false;
    for p in ports {
        let Some(want) = p.host else {
            out.push(p.clone());
            continue;
        };
        let ip = p.host_ip.as_deref().unwrap_or("127.0.0.1");
        let udp = p.protocol.as_deref().is_some_and(|s| s.starts_with("udp"));
        if udp || host_port_is_free(ip, want) {
            out.push(p.clone());
            continue;
        }

        let suggestion = next_free_host_port(ip, want.saturating_add(1));
        let chosen = if interactive && std::io::stdin().is_terminal() {
            let hint = suggestion.map(|s| s.to_string()).unwrap_or_default();
            eprint!(
                "  ! host port {want} ({ip}) is busy — port for sandbox:{} [{hint}] ",
                p.sandbox
            );
            let _ = std::io::stderr().flush();
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer)?;
            match answer.trim() {
                "" => suggestion.context(
                    "no free host port near the one in the environment file, and no answer given",
                )?,
                other => other
                    .parse::<u16>()
                    .with_context(|| format!("'{other}' is not a port number"))?,
            }
        } else {
            let s = suggestion.with_context(|| {
                format!("host port {want} is busy and no free port was found near it")
            })?;
            eprintln!(
                "  ! host port {want} ({ip}) is busy — using {s} for sandbox:{}",
                p.sandbox
            );
            s
        };

        moved |= chosen != want;
        out.push(envfile::Port {
            host: Some(chosen),
            ..p.clone()
        });
    }
    Ok((out, moved))
}

/// The variable an exported environment file routes its workspace through, so
/// the same committed file works on machines that keep their repositories in
/// different places.
///
/// `install.sh` offers to set it; nothing breaks if nobody does, because every
/// reference is written with a fallback (see `portable_workspace`).
pub(crate) const PROJECTS_ROOT_ENV: &str = "SBXW_PROJECTS_ROOT";

/// The developer's repository root, if they have declared one and this
/// workspace is actually under it.
///
/// The second half matters: a stale or unrelated `SBXW_PROJECTS_ROOT` must not
/// produce `${SBXW_PROJECTS_ROOT}/../../elsewhere/neos`. If the variable
/// doesn't contain this workspace it is simply not the right root for it, and
/// the workspace's own parent is used instead.
fn projects_root_for(workspace: &Path) -> Option<PathBuf> {
    let raw = std::env::var(PROJECTS_ROOT_ENV).ok()?;
    let root = PathBuf::from(raw.trim());
    if root.as_os_str().is_empty() {
        return None;
    }
    let root = std::fs::canonicalize(&root).unwrap_or(root);
    workspace.starts_with(&root).then_some(root)
}

/// The root a workspace's path is expressed against, and whether it came from
/// the environment rather than being guessed.
///
/// One chain, because two copies of it disagreed: the panel showed a
/// `projectsRoot` derived here while the `workspace:` line under it came from
/// `portable_workspace_with`'s own copy, so any edit to one made the UI report
/// a root the file wasn't using.
///
/// An explicit root that does not contain the workspace is refused — it cannot
/// be *this* workspace's root, and honouring it would emit
/// `${ROOT}/../../elsewhere`.
fn resolve_projects_root(workspace: &Path, explicit: Option<&Path>) -> (PathBuf, bool) {
    if let Some(r) = explicit.filter(|r| workspace.starts_with(r)) {
        return (r.to_path_buf(), false);
    }
    if let Some(r) = projects_root_for(workspace) {
        return (r, true);
    }
    let fallback = workspace
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| workspace.to_path_buf());
    (fallback, false)
}

/// The `workspace:` value for an exported path: routed through
/// `SBXW_PROJECTS_ROOT`, with the exporting machine's own root as the fallback.
///
/// ```text
/// ${SBXW_PROJECTS_ROOT:-/Users/thomas/Downloads}/neos
/// ```
///
/// Both halves earn their place. The **variable** is what makes the file
/// portable: a colleague who keeps repositories somewhere else exports it once
/// and every project's file resolves. The **fallback** is what stops the file
/// from being a trap: it works on the exporting machine, and on a colleague's
/// too if they happen to use the same layout, so nothing has to be configured
/// before the file will run at all. The price is one absolute path from the
/// exporter's machine sitting in a committed file — pass a root that contains
/// no personal detail, or accept it as the cost of a file that works.
///
/// The suffix is everything below the root, so a repository nested two levels
/// down keeps its shape.
///
/// The root is supplied rather than discovered, because the web UI lets you
/// edit it and an edit that changed only the *fallback* while the suffix stayed
/// wrong would be a confusing half-measure. See `resolve_projects_root`.
fn portable_workspace_with(workspace: &Path, root: Option<&Path>) -> String {
    let (root, _) = resolve_projects_root(workspace, root);
    let suffix = workspace
        .strip_prefix(&root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let root = root.to_string_lossy();
    if suffix.is_empty() {
        format!("${{{PROJECTS_ROOT_ENV}:-{root}}}")
    } else {
        format!("${{{PROJECTS_ROOT_ENV}:-{root}}}/{suffix}")
    }
}

/// `sbxw env export` — sbxw.toml, rendered as the `.sbxenv.yaml` sbx 0.39 reads.
///
/// One-way and non-authoritative: sbxw keeps provisioning from sbxw.toml, and
/// this exists so a contributor with plain `sbx` (or a CI runner) can bring up
/// the same sandbox. `crate::envfile` has the reasons sbxw doesn't simply run on
/// environment files instead.
fn cmd_env_export(
    path: Option<PathBuf>,
    name: Option<String>,
    config: PathBuf,
    out: Option<PathBuf>,
    force: bool,
) -> Result<()> {
    // Deliberately no `assert_available` here: exporting needs no sbx at all.
    // The file is read on whatever machine runs it, and writing one on a host
    // without sbx — or with an older one — is a perfectly good reason to run
    // this command.
    let cfg = load_config(&config)?;

    let workspace = match path {
        Some(p) => p,
        None => std::env::current_dir()?,
    };
    let workspace = std::fs::canonicalize(&workspace)
        .with_context(|| format!("workspace path does not exist: {}", workspace.display()))?;

    let name = name.unwrap_or_else(|| derive_sandbox_name(&workspace));
    if !is_valid_sandbox_name(&name) {
        bail!(INVALID_NAME_MSG);
    }

    let dest = out.unwrap_or_else(|| default_env_export_path(&workspace));
    let to_stdout = dest.as_os_str() == "-";
    // One root for the whole file, taken from the workspace — not one per path.
    // With `None`, every path resolved its own root (a kit beside the project
    // got `${ROOT:-/p/kits}/headroom` while the workspace got `${ROOT:-/p}/neos`),
    // so setting the variable moved them inconsistently. The web export always
    // passed a single root; this is the CLI catching up.
    let (root, _) = resolve_projects_root(&workspace, None);
    let file = env_file_for(
        &name,
        &cfg,
        &workspace,
        ports_from_config(&cfg),
        Some(&root),
    );
    let rendered = file.render(&env_file_generator());

    if to_stdout {
        print!("{rendered}");
        return Ok(());
    }
    if dest.exists() && !force {
        bail!(
            "{} already exists — pass --force to overwrite it",
            dest.display()
        );
    }
    if dest.starts_with(&workspace) {
        eprintln!(
            "sbxw  warning: {} is inside the workspace, so the agent can edit the file that \
             configures its own next sandbox. sbx recommends keeping it outside.",
            dest.display()
        );
    }
    std::fs::write(&dest, &rendered).with_context(|| format!("writing {}", dest.display()))?;
    println!("wrote {}", dest.display());
    println!(
        "run it with:  cd {} && sbx env run",
        dest.parent().unwrap_or(Path::new(".")).display()
    );
    Ok(())
}

/// Ports as an environment file spells them, from `sbxw.toml`'s own list.
fn ports_from_config(cfg: &Config) -> Vec<envfile::Port> {
    merged_ports(cfg, &[])
        .iter()
        .enumerate()
        .map(|(i, (host, sbox, _))| envfile::Port {
            sandbox: *sbox,
            host: Some(*host),
            host_ip: cfg.ip_per_app.then(|| host_ip_for(true, i)),
            protocol: None,
        })
        .collect()
}

/// The `/etc/hosts` alias `sbxw.toml` gives a sandbox port, if any.
///
/// An environment file has no field for it, so this is the only way the name
/// survives an export — matched on the sandbox port, which is the half that
/// does not move when a host port is renegotiated.
fn alias_for(cfg: &Config, sandbox_port: u16) -> String {
    cfg.ports
        .iter()
        .find(|c| c.sandbox_port == sandbox_port)
        .map(|c| c.alias.clone())
        .unwrap_or_default()
}

/// Build the environment file for `name`, from a config and a set of ports.
///
/// The one place an `EnvFile` is assembled. It used to be two — `cmd_env_export`
/// and `render_env_file_for` each built the struct field by field — and they had
/// already drifted: the same project exported `kits: ./local-kit` from the CLI
/// and `kits: ${SBXW_PROJECTS_ROOT:-…}/local-kit` from the browser. What
/// genuinely differs between the callers is only where the ports come from, so
/// that is the parameter; everything else is shared by construction.
///
/// Paths go through `${SBXW_PROJECTS_ROOT}` for kits as well as the workspace:
/// a kit checked out beside the project is exactly as machine-specific as the
/// project, and the variable is what the panel's whole UX is built around.
fn env_file_for(
    name: &str,
    cfg: &Config,
    workspace: &Path,
    ports: Vec<envfile::Port>,
    root: Option<&Path>,
) -> envfile::EnvFile {
    let triples: Vec<PortTriple> = ports
        .iter()
        .map(|p| {
            (
                p.host.unwrap_or(p.sandbox),
                p.sandbox,
                alias_for(cfg, p.sandbox),
            )
        })
        .collect();
    envfile::EnvFile {
        name: name.to_string(),
        agent: "claude".into(),
        workspace: portable_workspace_with(workspace, root),
        kits: cfg
            .kits
            .iter()
            .map(|k| {
                let p = Path::new(k);
                if p.is_absolute() {
                    portable_workspace_with(p, root)
                } else {
                    // A registry or git reference — `load_config` left it alone
                    // precisely because it is not a path, so neither do we.
                    k.clone()
                }
            })
            .collect(),
        env: cfg.effective_env(),
        ports,
        notes: env_export_notes(cfg, &triples),
    }
}

/// The `# Generated by …` line every exported file carries.
fn env_file_generator() -> String {
    format!("sbxw {}", env!("CARGO_PKG_VERSION"))
}

/// One rendered environment file, plus what the UI needs to explain it.
pub(crate) struct RenderedEnvFile {
    pub yaml: String,
    /// Where `save` would write it: beside the workspace, never inside it.
    pub dest: String,
    /// The root the `workspace:` expression is expressed against.
    pub projects_root: String,
    pub root_var: &'static str,
    /// True when that root came from the environment rather than being guessed
    /// from the workspace's parent.
    pub root_from_env: bool,
}

/// Render the environment file for an **existing sandbox**.
///
/// The CLI export reads `sbxw.toml` and reports the ports the config asks for.
/// This reads the ports the sandbox actually has, from `sbx ports`, which is
/// the better answer whenever the two differ — a port added from the web UI, a
/// host port that moved because the one in the config was busy. The aliases are
/// still `sbxw.toml`'s, because sbx has no field for them and nothing else
/// knows that 4200 is called `neos.local`.
pub(crate) fn render_env_file_for(
    name: &str,
    cfg: &Config,
    root_override: Option<&Path>,
) -> Result<RenderedEnvFile> {
    let workspace = workspace_for(name).with_context(|| {
        format!(
            "sbxw doesn't know which workspace '{name}' was created on — bring it up once \
             with sbxw and the record is written"
        )
    })?;

    // Live mappings first; the config's list is the fallback for a sandbox that
    // is stopped (nothing published) or an sbx whose `ports` output we can't parse.
    let live = sbx::list_ports_parsed(name);
    let ports: Vec<envfile::Port> = if live.is_empty() {
        ports_from_config(cfg)
    } else {
        live.iter().map(envfile::Port::from).collect()
    };

    let (effective_root, root_from_env) = resolve_projects_root(&workspace, root_override);
    let file = env_file_for(name, cfg, &workspace, ports, Some(&effective_root));

    Ok(RenderedEnvFile {
        yaml: file.render(&env_file_generator()),
        dest: default_env_export_path(&workspace)
            .to_string_lossy()
            .into_owned(),
        projects_root: effective_root.to_string_lossy().into_owned(),
        root_var: PROJECTS_ROOT_ENV,
        root_from_env,
    })
}

/// The header comments an exported file carries: every part of sbxw's pipeline
/// that has no field in the format, with the command that reproduces it.
///
/// This is the honest half of the export. An environment file that silently
/// dropped the egress allowlist would describe a sandbox with `**` egress —
/// a *more* permissive one than sbxw builds, which is the worst direction for
/// an omission to go in.
fn env_export_notes(cfg: &Config, ports: &[PortTriple]) -> Vec<String> {
    let mut notes = Vec::new();

    if !cfg.network_allow.is_empty() || !cfg.network_deny.is_empty() {
        let mut note = String::from(
            "the egress allowlist — policy is `sbx policy`, not a file field.\n\
             Run once per sandbox, on the host:",
        );
        if !cfg.network_allow.is_empty() {
            note.push_str(&format!(
                "\n  sbx policy allow network \"{}\"",
                cfg.network_allow.join(",")
            ));
        }
        if !cfg.network_deny.is_empty() {
            note.push_str(&format!(
                "\n  sbx policy deny network \"{}\"",
                cfg.network_deny.join(",")
            ));
        }
        note.push_str(
            "\nWithout them the sandbox falls back to the host's default policy,\n\
             which may be broader than what sbxw would have given it.",
        );
        notes.push(note);
    }

    notes.push(
        "OAuth credentials. sbxw injects CLAUDE_CODE_OAUTH_TOKEN as a kit at creation;\n\
         `sbx env run` does not. Run /login inside the sandbox, or add a secrets:\n\
         entry whose `command:` resolves the token on the host."
            .into(),
    );

    let aliases: Vec<String> = ports
        .iter()
        .filter(|(_, _, a)| !a.is_empty())
        .map(|(host, _, a)| format!("{a}:{host}"))
        .collect();
    if !aliases.is_empty() {
        notes.push(format!(
            "the /etc/hosts aliases ({}) — sbxw writes those on the host.\n\
             The ports below are published either way; only the names are missing.",
            aliases.join(", ")
        ));
    }

    if !cfg.share_skills {
        notes.push(
            "share_skills = false (`sbx create --no-share-skills`). An environment file\n\
             has no field for it, so `sbx env run` mounts the shared skill store."
                .into(),
        );
    }

    notes.push(
        "the browser terminal, the artifacts panel and the network-policy panel.\n\
         Those are sbxw itself — `sbx env run` attaches the agent to your terminal."
            .into(),
    );

    notes
}

/// `sbxw prune` — `sbx prune`, plus the two things on disk that sbx doesn't
/// know a sandbox owned.
///
/// A sandbox sbxw created leaves a credentials kit behind (deliberately: sbx
/// stores its *path* and re-resolves it on every container swap — see
/// `oauth_kit_dir`), and a `sbxw chat` sandbox also owns a throwaway workspace
/// directory. `sbxw rm` cleans up both because it knows the names it removed.
/// `sbx prune` chooses its own, so this reads the sandbox list either side of
/// the call and cleans up whatever disappeared — a name-set difference rather
/// than parsing prune's listing, so it survives any change to that output.
///
/// The preview is a real `--dry-run` call rather than sbxw's own guess at which
/// sandboxes qualify: `since` is sbx's duration parser and sbx's stop-time
/// bookkeeping, and a second opinion here could only ever disagree with what
/// the next call actually removes.
fn cmd_prune(since: Option<&str>, dry_run: bool, yes: bool) -> Result<()> {
    use std::io::IsTerminal;

    let preview = sbx::prune(since, true)?;
    let preview = preview.trim_end();
    if !preview.is_empty() {
        println!("{preview}");
    }
    if dry_run {
        return Ok(());
    }
    // sbx tells us nothing machine-readable about "how many", so the emptiness
    // check is on the names we can see for ourselves.
    let before: Vec<String> = sbx::list_sandboxes().into_iter().map(|s| s.name).collect();

    if !yes {
        if !std::io::stdin().is_terminal() {
            bail!(
                "`sbxw prune` removes sandboxes permanently and there is no terminal to \
                 confirm on — re-run with --yes (or --dry-run to only look)"
            );
        }
        eprint!("remove the stopped sandboxes listed above? [y/N] ");
        use std::io::Write as _;
        let _ = std::io::stderr().flush();
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim(), "y" | "Y" | "yes" | "Yes") {
            println!("nothing removed");
            return Ok(());
        }
    }

    let out = sbx::prune(since, false)?;
    let out = out.trim_end();
    if !out.is_empty() {
        println!("{out}");
    }

    let after: std::collections::HashSet<String> =
        sbx::list_sandboxes().into_iter().map(|s| s.name).collect();
    for gone in before.iter().filter(|n| !after.contains(*n)) {
        forget_oauth_kit(gone);
        if let Some(dir) = chat_workspace_of(gone) {
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    eprintln!(
                        "sbxw  warning: could not remove chat workspace {}: {e:#}",
                        dir.display()
                    );
                }
            }
        }
    }
    Ok(())
}

/// The directory a relative path in `sbxw.toml` is relative *to* — the config
/// file's own, not the current one, so `sbxw up -c ../other/sbxw.toml` resolves
/// that project's kits rather than this shell's.
fn config_dir_of(config: &Path) -> Result<PathBuf> {
    let abs = if config.is_absolute() {
        config.to_path_buf()
    } else {
        std::env::current_dir()?.join(config)
    };
    Ok(abs.parent().unwrap_or(&abs).to_path_buf())
}

/// Resolve one `kits = [...]` entry against the config's directory — when, and
/// only when, it is a local path.
///
/// A kit reference is four things at once (sbx's own list: a local directory, a
/// ZIP, an OCI registry reference, a `git+https://` / `git+ssh://` URL), and
/// only the first two are paths. Joining the others onto the project directory
/// turns `docker.io/sbx/playwright-kit:latest` into
/// `/home/you/project/docker.io/sbx/playwright-kit:latest`, which sbx then
/// reports as a missing directory — the reference was fine, sbxw broke it.
///
/// So: absolute and URL-shaped references pass through; `./x` and `../x` are
/// unambiguously paths and always resolve; a bare `x/y` resolves *only if the
/// result exists on disk*, and is otherwise left for sbx to read as a registry
/// reference. That last rule is the one that matters, because a bare name is
/// exactly where a directory and an OCI ref look alike.
fn resolve_kit_ref(config_dir: &Path, kit: String) -> String {
    let p = Path::new(&kit);
    if p.is_absolute() || kit.contains("://") {
        return kit;
    }
    let joined = config_dir.join(p);
    if kit.starts_with("./") || kit.starts_with("../") || joined.exists() {
        // Tidied, because these paths are also what `sbxw env export` writes
        // into a file a person reads and commits: `./neos/./local-kit` or
        // `/p/neos/../kits/headroom` both read like a bug.
        //
        // `canonicalize` when the path is real — it is the only thing that can
        // resolve `..` correctly through a symlink. When it isn't (a kit not
        // checked out yet), fall back to folding away the `.` components, which
        // is all that can be done without guessing.
        return std::fs::canonicalize(&joined)
            .unwrap_or_else(|_| joined.components().collect::<PathBuf>())
            .to_string_lossy()
            .into_owned();
    }
    kit
}

/// `Config::load_or_default` plus the path resolution that every entry point
/// which provisions a sandbox needs.
///
/// It lives here rather than in `Config::load_or_default` because resolution
/// needs the config file's directory, and a `Config` doesn't remember where it
/// came from. It used to be inline in `cmd_up`, which meant `sbxw web` and the
/// web-only daemon — both of which create sandboxes — read the *unresolved*
/// list, so a relative kit path worked or failed depending on which command you
/// happened to type.
fn load_config(config: &Path) -> Result<Config> {
    let mut cfg = Config::load_or_default(config)?;
    let dir = config_dir_of(config)?;
    cfg.kits = cfg
        .kits
        .into_iter()
        .map(|k| resolve_kit_ref(&dir, k))
        .collect();
    // `--env-file` is only ever a file, so it has none of the ambiguity above.
    cfg.env_files = cfg
        .env_files
        .into_iter()
        .map(|f| {
            let p = Path::new(&f);
            if p.is_absolute() {
                f
            } else {
                dir.join(p).to_string_lossy().into_owned()
            }
        })
        .collect();
    Ok(cfg)
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
    let cfg = load_config(&config)?;

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
        let attached = run_agent_foreground(&name, &cfg);
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
/// This also works for sandboxes created with a custom --kit
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

fn run_agent_foreground(name: &str, cfg: &Config) -> Result<()> {
    use std::process::Command;
    // Same argv as the web terminal's agent pane — including sbxw.toml's env,
    // which `sbx run` applies to the agent session. See `sbx::run_attach_args`.
    let args = sbx::run_attach_args(name, &cfg.env_pairs(), &cfg.env_files);
    let status = Command::new("sbx").args(&args).status()?;
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

/// Where the OAuth mixin kit for `name` lives on the host.
///
/// Stable per sandbox, and under `state_dir()` rather than the OS temp dir,
/// because **sbx keeps the reference, not a copy**. A container swap
/// (`sbx kit add`) recomposes the sandbox from its template, which re-resolves
/// every kit applied before it *by its original path*. A kit directory that has
/// been deleted — or sat in a temp dir a reboot cleared — fails that resolution
/// and takes the unrelated kit being added down with it:
///
/// ```text
/// ERROR: re-resolve original kit 0 ("/var/folders/…/sbxw-oauth-kit-36226"):
///        kit reference "…": path does not exist
/// ```
///
/// So this directory has to outlive the command that created it and stay put
/// for as long as the sandbox does. `sbxw rm` deletes it (see
/// `forget_oauth_kit`); nothing else should.
fn oauth_kit_dir(name: &str) -> PathBuf {
    state_dir().join("kits").join(format!("{name}-oauth"))
}

/// Write the mixin kit whose spec.yaml injects `name`'s OAuth credentials, and
/// return its directory.
///
/// Used for new sandboxes (`--kit` at create time) and for existing *stopped*
/// ones (`sbx kit add`). Running sandboxes get the credentials file written
/// directly over `sbx exec` instead (see `sbx::write_oauth_credentials`),
/// because `sbx kit add` recreates the container.
///
/// Rewriting in place is what keeps a re-resolve honest: the path sbx recorded
/// stays valid, and picks up the current token rather than the one the sandbox
/// was born with.
///
/// The file holds a live OAuth token, so it is written `0600` inside a `0700`
/// directory — this is a durable copy now, not one deleted seconds later.
fn write_oauth_kit(name: &str, credentials_json: &str) -> Result<PathBuf> {
    let dir = oauth_kit_dir(name);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("could not create OAuth kit dir {}", dir.display()))?;
    restrict_permissions(&dir, 0o700);

    let spec = dir.join("spec.yaml");
    std::fs::write(&spec, oauth_kit_spec(credentials_json))
        .with_context(|| format!("could not write {}", spec.display()))?;
    restrict_permissions(&spec, 0o600);
    Ok(dir)
}

/// Drop the OAuth kit directory for `name`. Called when the sandbox is removed:
/// until then the reference has to keep resolving (see `oauth_kit_dir`).
pub(crate) fn forget_oauth_kit(name: &str) {
    forget_sandbox_state(name)
}

/// Everything sbxw keeps on the host *about* one sandbox, removed together.
///
/// There were two teardown paths (`sbxw rm`, `sbxw prune`) each enumerating the
/// artefacts by hand, so a new kind of state cost an edit in both and forgetting
/// one was silent — which is what happened when `sbxw env run` started writing
/// `state/env/<name>/`: it survived `rm`, and a later sandbox reusing the name
/// inherited a stale merged config.
///
/// Best-effort throughout: this runs after the sandbox is already gone, and a
/// leftover file is worth a warning, not a failed command.
pub(crate) fn forget_sandbox_state(name: &str) {
    for dir in [oauth_kit_dir(name), env_state_dir(name)] {
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("could not remove {}: {e:#}", dir.display());
            }
        }
    }
    for file in [workspace_record_path(name), model_migrated_marker(name)] {
        let _ = std::fs::remove_file(file);
    }
}

/// Where `sbxw env run` keeps the documents it derives for one sandbox.
fn env_state_dir(name: &str) -> PathBuf {
    state_dir().join("env").join(name)
}

/// Marks that the pre-`ANTHROPIC_DEFAULT_MODEL` `model` key has been dealt with
/// for this sandbox.
///
/// The migration is one-shot by nature — its own doc says every later call is a
/// no-op — but "no-op" still cost three `sbx exec` spawns (write a script, run
/// it, delete it) on *every* bring-up, forever, on every sandbox. A marker
/// turns that into one local `exists()`.
fn model_migrated_marker(name: &str) -> PathBuf {
    state_dir().join(format!("{name}.model-migrated"))
}

/// Best-effort `chmod`. A no-op off Unix, where the token's protection is the
/// containing user profile rather than a mode bit.
fn restrict_permissions(path: &std::path::Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
}

/// The OAuth mixin's spec.yaml, in kit spec **v2**.
///
/// v2 is the only grammar sbxw emits. It restructures exactly the two sections
/// this kit uses — `network.allowedDomains` became `permissions.network.allow`,
/// `commands.initFiles` became `setup.files` — and its loader *rejects* v1
/// field names rather than tolerating them, so a spec has to commit to one.
/// sbxw used to pick per host; with the floor at 0.39 there is one answer.
fn oauth_kit_spec(credentials_json: &str) -> String {
    oauth_kit_spec_inner(Some(credentials_json))
}

/// The same kit minus the credentials file: same name, same claude.ai rule.
/// Used only to make a dangling reference resolve again when no token is
/// available this run (see `kit_add_repairing_oauth`).
fn oauth_kit_spec_without_credentials() -> String {
    oauth_kit_spec_inner(None)
}

fn oauth_kit_spec_inner(credentials_json: Option<&str>) -> String {
    let files = credentials_json.map_or_else(String::new, |creds| {
        format!(
            "\nsetup:\n\
             \x20 files:\n\
             \x20   - path: /home/agent/.claude/.credentials.json\n\
             \x20     content: '{creds}'\n\
             \x20     mode: \"0600\"\n"
        )
    });
    format!(
        "schemaVersion: \"2\"\n\
         kind: mixin\n\
         name: claude-oauth\n\
         description: Injects OAuth credentials for Claude Code\n\n\
         permissions:\n\
         \x20 network:\n\
         \x20   allow:\n\
         \x20     - claude.ai\n\
         {files}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory of its own for one test, with the favourites file
    /// inside it — so nothing here touches the real `$HOME`.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sbxw-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// The picker stores what `/api/fs` reports, which is the canonical path.
    /// Anything else and the star silently fails to light up on arrival — the
    /// folder *is* starred, it just never looks it, and the same root gets
    /// starred again under a second spelling.
    #[test]
    fn starring_a_folder_stores_it_canonically_and_only_once() {
        let dir = scratch("fav-canonical");
        let file = dir.join("favourites.json");
        let projects = dir.join("projects");
        std::fs::create_dir_all(&projects).unwrap();
        let canonical = projects
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();

        let once = set_favourite_in(&file, projects.to_str().unwrap(), true).unwrap();
        assert_eq!(once, vec![canonical.clone()]);

        // The same folder by a trailing slash and by a detour through `..`:
        // one favourite, not three.
        let again = set_favourite_in(&file, &format!("{}/", projects.display()), true).unwrap();
        assert_eq!(again, vec![canonical.clone()]);
        let detour = format!("{}/projects/../projects", dir.display());
        assert_eq!(
            set_favourite_in(&file, &detour, true).unwrap(),
            vec![canonical]
        );

        // And it survives the round trip through the file.
        assert_eq!(favourites_in(&file).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The case that would strand someone: a starred project folder that was
    /// moved or lives on a drive that isn't plugged in. It can't be
    /// canonicalized, so un-starring must work on the stored string alone —
    /// otherwise the one entry you want gone is the one that can't be removed.
    #[test]
    fn a_folder_that_no_longer_exists_can_still_be_unstarred() {
        let dir = scratch("fav-missing");
        let file = dir.join("favourites.json");
        let gone = dir.join("external-drive");
        std::fs::create_dir_all(&gone).unwrap();
        let stored = set_favourite_in(&file, gone.to_str().unwrap(), true).unwrap();
        assert_eq!(stored.len(), 1);

        std::fs::remove_dir_all(&gone).unwrap();
        // Still listed — the list is the user's, and an unplugged drive is not
        // a reason to edit it behind their back.
        assert_eq!(favourites_in(&file).len(), 1);

        let after = set_favourite_in(&file, &stored[0], false).unwrap();
        assert!(after.is_empty(), "{after:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two things that are not a folder to star: a file, and nothing at all.
    #[test]
    fn only_an_actual_folder_can_be_starred() {
        let dir = scratch("fav-notdir");
        let file = dir.join("favourites.json");
        let readme = dir.join("README.md");
        std::fs::write(&readme, "x").unwrap();

        assert!(set_favourite_in(&file, readme.to_str().unwrap(), true).is_err());
        assert!(set_favourite_in(&file, "  ", true).is_err());
        assert!(set_favourite_in(&file, &format!("{}/nope", dir.display()), true).is_err());
        assert!(favourites_in(&file).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The row of chips is one line under the picker; past a dozen it stops
    /// being a shortcut. The refusal has to say what to do about it.
    #[test]
    fn the_favourites_list_is_capped_with_an_actionable_message() {
        let dir = scratch("fav-cap");
        let file = dir.join("favourites.json");
        for i in 0..MAX_FAVOURITES {
            let d = dir.join(format!("p{i}"));
            std::fs::create_dir_all(&d).unwrap();
            set_favourite_in(&file, d.to_str().unwrap(), true).unwrap();
        }
        assert_eq!(favourites_in(&file).len(), MAX_FAVOURITES);

        let overflow = dir.join("one-too-many");
        std::fs::create_dir_all(&overflow).unwrap();
        let err = set_favourite_in(&file, overflow.to_str().unwrap(), true)
            .expect_err("the cap should refuse")
            .to_string();
        assert!(err.contains("unstar one"), "{err}");

        // Re-starring one already in the list is not a new entry, so the cap
        // must not refuse it.
        let existing = favourites_in(&file)[0].clone();
        assert!(set_favourite_in(&file, &existing, true).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

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

    /// The trap `inspect` sets: it also reports custom secrets, so
    /// a secret named after a kit made sbxw skip a kit it had never applied.
    #[test]
    fn a_secret_named_like_a_kit_no_longer_passes_for_the_kit() {
        let inspect = "Name: neos\n\
                       Status: running\n\
                       Kits:\n\
                       \x20 - claude-oauth\n\
                       Secrets:\n\
                       \x20 - headroom\n\
                       \x20 - anthropic\n";

        let section = inspect_kits_section(inspect);
        assert!(section.contains("claude-oauth"), "{section}");
        assert!(!section.contains("headroom"), "{section}");
        assert!(!section.contains("anthropic"), "{section}");
    }

    #[test]
    fn inspect_kits_section_reads_the_json_form_too() {
        let section = inspect_kits_section(
            r#"{"name":"neos","kits":["md-to-pdf-tools"],"secrets":["headroom"]}"#,
        );
        assert!(section.contains("md-to-pdf-tools"), "{section}");
        assert!(!section.contains("headroom"), "{section}");
    }

    /// Unknown layouts (a KITS *column*, say) must keep today's behaviour
    /// rather than reporting "no kits" and recreating the container every `up`.
    #[test]
    fn an_unrecognised_inspect_layout_falls_back_to_the_whole_output() {
        let table = "NAME  AGENT   KITS\nneos  claude  md-to-pdf-tools\n";
        assert_eq!(inspect_kits_section(table), table);
    }

    /// A governed host is a working host: `sbxw up` must not die because the
    /// organisation, not this machine, owns the network policy.
    #[test]
    fn org_governed_policy_warns_while_other_failures_still_abort() {
        let governed = Err(anyhow::anyhow!(
            "`sbx policy allow network` exited with exit status: 1:\n\
             this rule is managed by your organization — contact platform@acme.example"
        ));
        assert!(apply_bringup_policy("allowlist", governed).is_ok());

        let broken = Err(anyhow::anyhow!("no such sandbox: neos"));
        let err = apply_bringup_policy("allowlist", broken).unwrap_err();
        assert!(format!("{err:#}").contains("failed to apply network allowlist"));
    }

    /// The workspace goes through a variable *with* a fallback: the variable
    /// is what makes a committed file portable, the fallback is what stops it
    /// from being a trap on a machine where nobody set one.
    #[test]
    fn a_portable_workspace_keeps_the_suffix_below_the_root() {
        let ws = Path::new("/home/you/dev/acme/neos");
        assert_eq!(
            portable_workspace_with(ws, Some(Path::new("/home/you/dev"))),
            "${SBXW_PROJECTS_ROOT:-/home/you/dev}/acme/neos"
        );
        // No root given: the workspace's own parent, which always resolves.
        assert_eq!(
            portable_workspace_with(ws, None),
            "${SBXW_PROJECTS_ROOT:-/home/you/dev/acme}/neos"
        );
        // A root that doesn't contain the workspace cannot be its root, and
        // honouring it would emit `${ROOT}/../../elsewhere`.
        assert_eq!(
            portable_workspace_with(ws, Some(Path::new("/opt/other"))),
            "${SBXW_PROJECTS_ROOT:-/home/you/dev/acme}/neos"
        );
    }

    /// A busy host port is renegotiated *before* sbx is called. Getting this
    /// wrong doesn't cost a port, it costs the sandbox: sbx removes the one it
    /// was creating and leaves the secrets it provisioned first behind.
    #[test]
    fn a_busy_host_port_is_moved_and_a_free_one_is_left_alone() {
        use std::net::TcpListener;
        let held = TcpListener::bind("127.0.0.1:0").expect("bind");
        let busy = held.local_addr().unwrap().port();

        let ports = vec![
            envfile::Port {
                sandbox: 4200,
                host: Some(busy),
                host_ip: None,
                protocol: None,
            },
            // No host port: the file asked for an ephemeral one, which cannot
            // collide and must not be touched.
            envfile::Port {
                sandbox: 8000,
                host: None,
                host_ip: None,
                protocol: None,
            },
        ];
        // `false` = non-interactive, which is also what a daemon or CI gets.
        let (out, moved) = negotiate_ports(&ports, false).expect("negotiate");

        assert!(moved, "the busy port was reported as moved");
        assert_ne!(out[0].host, Some(busy), "it actually moved");
        assert!(
            out[0].host.unwrap() > busy,
            "to a port after the one asked for"
        );
        assert_eq!(out[0].sandbox, 4200, "the sandbox port is not negotiable");
        assert_eq!(out[1].host, None, "an ephemeral port stays ephemeral");

        drop(held);
        let (again, moved) = negotiate_ports(&ports, false).expect("negotiate");
        assert!(!moved, "nothing moves once the port is free");
        assert_eq!(again[0].host, Some(busy));
    }

    /// A UDP mapping can't be tested with a TCP bind, and a test that cannot be
    /// performed must not be reported as a result.
    #[test]
    fn a_udp_mapping_is_left_for_sbx_to_judge() {
        use std::net::TcpListener;
        let held = TcpListener::bind("127.0.0.1:0").expect("bind");
        let busy = held.local_addr().unwrap().port();
        let ports = vec![envfile::Port {
            sandbox: 51820,
            host: Some(busy),
            host_ip: None,
            protocol: Some("udp".into()),
        }];
        let (out, moved) = negotiate_ports(&ports, false).expect("negotiate");
        assert!(!moved);
        assert_eq!(out[0].host, Some(busy));
    }

    /// Two files describing one sandbox is exactly where somebody edits the
    /// wrong one and watches nothing happen. What was borrowed gets printed.
    #[test]
    fn the_environment_file_wins_and_sbxw_toml_fills_the_gaps() {
        let base = Config {
            network_allow: vec!["github.com".into()],
            claude_model: "claude-sonnet-5".into(),
            ports: vec![config::PortMap {
                alias: "neos.local".into(),
                sandbox_port: 4200,
                host_port: 4200,
            }],
            env: std::collections::BTreeMap::from([
                ("NODE_ENV".into(), "development".into()),
                ("ONLY_TOML".into(), "kept".into()),
            ]),
            ..Config::default()
        };
        let spec = envfile::Spec {
            ports: vec![envfile::Port {
                sandbox: 4200,
                host: Some(4201),
                host_ip: None,
                protocol: None,
            }],
            env: std::collections::BTreeMap::from([("NODE_ENV".into(), "test".into())]),
            ..Default::default()
        };

        let (cfg, report) = config_from_spec(&spec, Path::new("/env"), base);

        // The file's port wins…
        assert_eq!(cfg.ports.len(), 1);
        assert_eq!(cfg.ports[0].host_port, 4201);
        // …but keeps the alias, which the format has no field for. Pointing it
        // at 4200 would name a port nothing is listening on.
        assert_eq!(cfg.ports[0].alias, "neos.local");
        assert_eq!(cfg.env.get("NODE_ENV").map(String::as_str), Some("test"));
        assert_eq!(cfg.env.get("ONLY_TOML").map(String::as_str), Some("kept"));

        let report = report.join("\n");
        assert!(report.contains("neos.local → :4201"), "{report}");
        assert!(report.contains("egress allowlist"), "{report}");
        assert!(report.contains("claude-sonnet-5"), "{report}");
        assert!(
            report.contains("NODE_ENV"),
            "the override is named: {report}"
        );
    }

    /// A kit reference is four things at once, and only two of them are paths.
    /// Joining an OCI reference onto the project directory is how
    /// `docker.io/sbx/playwright-kit:latest` became a missing directory.
    #[test]
    fn only_kit_references_that_are_paths_are_resolved_against_the_config() {
        let dir = scratch("kit-refs");
        std::fs::create_dir_all(dir.join("assets/headroom")).unwrap();
        let at = |s: &str| dir.join(s).to_string_lossy().into_owned();

        // Registry and git references are not paths and must survive intact.
        for r#ref in [
            "docker.io/sbx/playwright-kit:latest",
            "ghcr.io/owner/kit:1.0",
            "git+https://github.com/docker/sbx-kits-contrib",
            "git+ssh://git@github.com/owner/kit",
        ] {
            assert_eq!(
                resolve_kit_ref(&dir, r#ref.to_string()),
                r#ref,
                "{ref} is not a path"
            );
        }

        // An explicit relative path always resolves, existing or not — the `./`
        // is the author saying which of the four kinds they meant. The marker
        // has done its job by then and is folded out of the result.
        assert_eq!(
            resolve_kit_ref(&dir, "./assets/headroom".into()),
            at("assets/headroom")
        );
        assert_eq!(
            resolve_kit_ref(&dir, "../side/kit".into()),
            at("../side/kit")
        );

        // A bare name resolves only when it turns out to be a real directory.
        assert_eq!(
            resolve_kit_ref(&dir, "assets/headroom".into()),
            at("assets/headroom")
        );
        assert_eq!(
            resolve_kit_ref(&dir, "assets/nothing-here".into()),
            "assets/nothing-here"
        );

        // Absolute stays absolute, wherever the config happens to live.
        let abs = at("assets/headroom");
        assert_eq!(resolve_kit_ref(Path::new("/elsewhere"), abs.clone()), abs);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The environment file goes *beside* the workspace, never inside it: the
    /// agent can rewrite anything it mounts, and this file decides what the next
    /// sandbox gets.
    #[test]
    fn an_exported_environment_file_lands_outside_the_workspace() {
        let dest = default_env_export_path(Path::new("/home/you/dev/neos"));
        assert_eq!(dest, Path::new("/home/you/dev/.sbxenv.yaml"));
        assert!(!dest.starts_with("/home/you/dev/neos"));
    }

    /// `web_port_of` learned the IPv6 lesson and wrote it in a comment; the
    /// other half of the same string was then split by hand at three sites,
    /// where `[::1]:7681` yields `"["` — so `starts_with("127.")` is false and
    /// the `sbxw.localhost` alias silently never gets added.
    #[test]
    fn both_halves_of_a_web_addr_survive_an_ipv6_literal() {
        assert_eq!(web_ip_of("127.0.0.1:7681"), "127.0.0.1");
        assert_eq!(web_port_of("127.0.0.1:7681"), "7681");

        assert_eq!(web_ip_of("[::1]:7681"), "::1");
        assert_eq!(web_port_of("[::1]:7681"), "7681");

        assert_eq!(web_ip_of("0.0.0.0:7681"), "0.0.0.0");
        // Not loopback, so it earns no alias — the check the three call sites
        // make, and the one that was broken.
        assert!(!web_ip_of("[::1]:7681").starts_with("127."));
        assert!(web_ip_of("127.0.0.2:7681").starts_with("127."));
    }

    /// One spelling for every path in an exported file. The CLI export used to
    /// write kits relative to the file's directory while the web panel wrote
    /// them through the variable, so the same project produced two different
    /// `kits:` lines depending on which one you used.
    #[test]
    fn every_exported_path_goes_through_the_root_variable() {
        let cfg = Config {
            kits: vec![
                "/home/you/dev/kits/headroom".into(),
                "docker.io/sbx/playwright-kit:latest".into(),
            ],
            ports: vec![],
            ..Config::default()
        };
        let ws = Path::new("/home/you/dev/neos");
        let root = Path::new("/home/you/dev");

        let file = env_file_for("neos", &cfg, ws, ports_from_config(&cfg), Some(root));
        assert_eq!(file.workspace, "${SBXW_PROJECTS_ROOT:-/home/you/dev}/neos");
        assert_eq!(
            file.kits[0],
            "${SBXW_PROJECTS_ROOT:-/home/you/dev}/kits/headroom"
        );
        // A registry reference is not a path and must survive untouched.
        assert_eq!(file.kits[1], "docker.io/sbx/playwright-kit:latest");
    }

    /// One root for the whole file, not one per path — otherwise setting
    /// `SBXW_PROJECTS_ROOT` moves the workspace and its kits to different
    /// places, which is worse than not using the variable at all.
    #[test]
    fn every_path_in_one_file_shares_one_root() {
        let cfg = Config {
            kits: vec!["/p/kits/headroom".into()],
            ports: vec![],
            ..Config::default()
        };
        let ws = Path::new("/p/neos");
        let (root, _) = resolve_projects_root(ws, None);

        let file = env_file_for("neos", &cfg, ws, ports_from_config(&cfg), Some(&root));
        assert_eq!(file.workspace, "${SBXW_PROJECTS_ROOT:-/p}/neos");
        assert_eq!(file.kits[0], "${SBXW_PROJECTS_ROOT:-/p}/kits/headroom");

        // Both suffixes hang off the same root, so one variable relocates the
        // whole file coherently.
        for path in [&file.workspace, &file.kits[0]] {
            assert!(
                path.starts_with("${SBXW_PROJECTS_ROOT:-/p}/"),
                "{path} does not share the file's root"
            );
        }
    }

    /// The root the panel *reports* and the root the file *uses* came from two
    /// copies of the same chain; they must not be able to disagree.
    #[test]
    fn the_reported_root_is_the_one_the_workspace_line_uses() {
        let ws = Path::new("/home/you/dev/acme/neos");

        let (root, from_env) = resolve_projects_root(ws, Some(Path::new("/home/you/dev")));
        assert_eq!(root, Path::new("/home/you/dev"));
        assert!(
            !from_env,
            "an explicit root did not come from the environment"
        );
        assert_eq!(
            portable_workspace_with(ws, Some(&root)),
            "${SBXW_PROJECTS_ROOT:-/home/you/dev}/acme/neos"
        );

        // A root that doesn't contain the workspace cannot be its root; both
        // the report and the line fall back to the parent.
        let (root, _) = resolve_projects_root(ws, Some(Path::new("/opt/other")));
        assert_eq!(root, Path::new("/home/you/dev/acme"));
        assert_eq!(
            portable_workspace_with(ws, Some(&root)),
            "${SBXW_PROJECTS_ROOT:-/home/you/dev/acme}/neos"
        );
    }

    /// An export that dropped the allowlist would describe a *more* permissive
    /// sandbox than sbxw builds, so every omission is named in the file.
    #[test]
    fn the_export_notes_name_what_the_format_cannot_carry() {
        let cfg = Config {
            network_allow: vec!["github.com".into(), "pypi.org".into()],
            network_deny: vec!["telemetry.example".into()],
            claude_model: "claude-sonnet-5".into(),
            share_skills: false,
            ..Config::default()
        };
        let ports = vec![(4200u16, 4200u16, "neos.local".to_string())];
        let notes = env_export_notes(&cfg, &ports).join("\n");

        assert!(notes.contains("sbx policy allow network \"github.com,pypi.org\""));
        assert!(notes.contains("sbx policy deny network \"telemetry.example\""));
        // The model is no longer a *note*: it rides in the file's own `env:`
        // block as ANTHROPIC_DEFAULT_MODEL, so an export carries it for real
        // rather than describing what the reader would have to do by hand.
        assert!(!notes.contains("claude-sonnet-5"), "{notes}");
        assert!(notes.contains("neos.local:4200"));
        assert!(notes.contains("--no-share-skills"));
        assert!(notes.contains("CLAUDE_CODE_OAUTH_TOKEN"));

        // Nothing to say about a setting left at its default.
        let quiet = env_export_notes(
            &Config {
                network_allow: vec![],
                network_deny: vec![],
                claude_model: String::new(),
                ports: vec![],
                ..Config::default()
            },
            &[],
        )
        .join("\n");
        assert!(!quiet.contains("sbx policy"));
        assert!(!quiet.contains("--no-share-skills"));
    }

    /// Every kit sbxw ships declares startup commands — that is how they
    /// install anything — so `kit add` refusing them is the common case.
    #[test]
    fn the_bundled_kits_are_all_startup_kits() {
        for kit in ["k8s-tools", "headroom", "md-to-pdf-tools"] {
            let dir = format!("assets/{kit}");
            assert!(
                kit_declares_startup(&dir),
                "{dir} was expected to declare startup commands"
            );
        }
        // Nothing to read is not a claim that there is nothing to run.
        assert!(!kit_declares_startup("assets/does-not-exist"));
        assert!(!kit_declares_startup("ghcr.io/owner/kit:1.0"));
    }

    /// A `startup:` key must be recognised in either grammar — sbxw writes v2,
    /// but a third-party kit it reads may still be v1 — and a kit that merely
    /// mentions the word must not be.
    #[test]
    fn startup_is_detected_in_both_kit_grammars() {
        let dir = std::env::temp_dir().join(format!("sbxw-startup-{}", std::process::id()));
        let write = |body: &str| {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("spec.yaml"), body).unwrap();
            kit_declares_startup(&dir.to_string_lossy())
        };

        assert!(write("commands:\n  startup:\n    - command: [\"true\"]\n"));
        assert!(write("setup:\n  startup:\n    - command: [\"true\"]\n"));
        assert!(!write("description: runs a startup script for you\n"));
        assert!(!write("setup:\n  files:\n    - path: /tmp/startup\n"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_recreate_refusal_is_told_apart_from_other_failures() {
        assert!(kit_add_needs_recreate(
            "ERROR: kit \"md-to-pdf-tools\" declares commands.startup, which the kit-add \
             recreate flow does not yet apply; recreate the sandbox from scratch via \
             `sbx rm` + `sbx create --kit` to use this kit"
        ));
        assert!(!kit_add_needs_recreate("path does not exist"));
        assert!(!kit_add_needs_recreate("no such sandbox: neos"));
    }

    /// The real failure this guards: a container swap re-resolves the OAuth kit sbxw
    /// used to delete, and the *unrelated* kit being added is what reports it.
    #[test]
    fn a_stale_oauth_kit_is_recognised_in_sbxs_own_message() {
        let dir = std::env::temp_dir().join("sbxw-oauth-kit-36226");
        let _ = std::fs::remove_dir_all(&dir);
        let err = format!(
            "`sbx kit add test /kits/md-to-pdf-tools` exited with exit status: 1:\n\
             ERROR: re-resolve original kit 0 (\"{}\"): kit reference \"{}\": path does not exist",
            dir.display(),
            dir.display()
        );
        assert_eq!(stale_oauth_kit_path(&err), Some(dir));
    }

    /// The repair writes to a path parsed out of a subprocess's stderr, so
    /// every guard on it earns its keep.
    #[test]
    fn the_repair_declines_anything_but_a_missing_sbxw_oauth_kit() {
        let missing = std::env::temp_dir().join("sbxw-oauth-kit-1");
        let _ = std::fs::remove_dir_all(&missing);
        let quoted = |p: &std::path::Path| {
            format!(
                "re-resolve original kit 0 (\"{}\"): path does not exist",
                p.display()
            )
        };

        // Someone else's kit, merely missing.
        let theirs = std::env::temp_dir().join("some-other-kit");
        assert_eq!(stale_oauth_kit_path(&quoted(&theirs)), None);

        // Ours, but the failure is not a missing path.
        assert_eq!(
            stale_oauth_kit_path(&format!(
                "re-resolve original kit 0 (\"{}\"): permission denied",
                missing.display()
            )),
            None
        );

        // Ours and missing, but its parent is gone too — we create the kit
        // directory, never the tree above it.
        let orphan = std::env::temp_dir()
            .join("sbxw-no-such-parent-xyz")
            .join("sbxw-oauth-kit-2");
        assert_eq!(stale_oauth_kit_path(&quoted(&orphan)), None);

        // A relative path is never a kit reference sbx recorded.
        assert_eq!(
            stale_oauth_kit_path(
                "re-resolve original kit 0 (\"sbxw-oauth-kit-3\"): does not exist"
            ),
            None
        );

        // And one that still exists needs no repair.
        std::fs::create_dir_all(&missing).unwrap();
        assert_eq!(stale_oauth_kit_path(&quoted(&missing)), None);
        let _ = std::fs::remove_dir_all(&missing);
    }

    /// Restoring a dangling reference must not depend on having a token: the
    /// point is only that the path resolves again.
    #[test]
    fn the_credential_free_kit_still_names_itself_and_keeps_its_rule() {
        let spec = oauth_kit_spec_without_credentials();
        assert!(spec.contains("name: claude-oauth"), "{spec}");
        assert!(spec.contains("claude.ai"), "{spec}");
        assert!(!spec.contains("credentials.json"), "{spec}");

        // Same grammar as the credential-carrying one — they are one document
        // with an optional section, and a mismatch would load nowhere.
        let with = oauth_kit_spec(r#"{"claudeAiOauth":{"accessToken":"t"}}"#);
        let version_line = |s: &str| s.lines().next().unwrap_or_default().to_string();
        assert_eq!(version_line(&spec), version_line(&with));
        assert_eq!(version_line(&spec), "schemaVersion: \"2\"");
    }

    /// The OAuth kit's whole purpose is to still be there later.
    #[test]
    fn the_oauth_kit_is_written_where_it_can_be_re_resolved() {
        let name = format!("sbxw-test-{}", std::process::id());
        let dir = write_oauth_kit(&name, r#"{"claudeAiOauth":{"accessToken":"t"}}"#).unwrap();

        assert_eq!(dir, oauth_kit_dir(&name));
        assert!(dir.starts_with(state_dir()), "{}", dir.display());
        assert!(dir.join("spec.yaml").exists());

        // Rewriting in place keeps the path sbx recorded valid.
        let again = write_oauth_kit(&name, r#"{"claudeAiOauth":{"accessToken":"fresh"}}"#).unwrap();
        assert_eq!(again, dir);
        let spec = std::fs::read_to_string(dir.join("spec.yaml")).unwrap();
        assert!(spec.contains("fresh"), "{spec}");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("spec.yaml"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "the spec holds a live OAuth token");
        }

        forget_oauth_kit(&name);
        assert!(!dir.exists());
        // Removing what is already gone is not an error.
        forget_oauth_kit(&name);
    }

    /// The v2 loader *rejects* v1 field names, so a spec that mixed the two
    /// would load nowhere. sbxw emits v2 and only v2.
    #[test]
    fn the_oauth_kit_is_written_in_spec_v2() {
        let spec = oauth_kit_spec(r#"{"claudeAiOauth":{"accessToken":"t"}}"#);
        assert!(spec.contains("accessToken"), "{spec}");
        assert!(spec.contains("schemaVersion: \"2\""), "{spec}");
        assert!(spec.contains("permissions:"), "{spec}");
        assert!(spec.contains("setup:"), "{spec}");
        // The v1 spellings must not survive anywhere in the document.
        assert!(!spec.contains("allowedDomains"), "{spec}");
        assert!(!spec.contains("initFiles"), "{spec}");

        // And `kit_display_name` must still find the name in it.
        let dir = std::env::temp_dir().join(format!("sbxw-oauth-spec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("spec.yaml"), &spec).unwrap();
        assert_eq!(kit_display_name(&dir.to_string_lossy()), "claude-oauth");
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

    #[test]
    fn sandbox_name_sanitization_keeps_only_the_valid_alphabet() {
        assert_eq!(sanitize_sandbox_name_component("my-project"), "my-project");
        assert_eq!(
            sanitize_sandbox_name_component("my project!"),
            "my-project-"
        );
        assert_eq!(sanitize_sandbox_name_component(""), "sandbox");
        assert!(is_valid_sandbox_name(&sanitize_sandbox_name_component(
            "a/b_c.d é"
        )));
    }

    /// Two different paths whose basenames collide must not end up sharing a
    /// derived name — that would mean two unrelated projects fighting over one
    /// sandbox. Same path, called twice, must be stable (the normal
    /// reuse-if-exists case), not pile up `-copy` suffixes on itself.
    #[test]
    fn derived_sandbox_names_dedupe_on_a_clash_and_are_stable_on_reuse() {
        let tag = std::process::id();
        let base = std::env::temp_dir().join(format!("sbxw-test-derive-{tag}"));
        let project_a = base.join("widgets");
        let project_b_root = base.join("other");
        let project_b = project_b_root.join("widgets"); // same basename, different path
        std::fs::create_dir_all(&project_a).unwrap();
        std::fs::create_dir_all(&project_b).unwrap();

        // Clean slate: neither name is recorded yet.
        let _ = std::fs::remove_file(workspace_record_path("widgets"));
        let _ = std::fs::remove_file(workspace_record_path("widgets-copy"));

        let name_a = derive_sandbox_name(&project_a);
        assert_eq!(name_a, "widgets");

        // `provision_sandbox` is what normally writes this; simulate it so the
        // next call sees project_a's name as taken.
        std::fs::write(
            workspace_record_path(&name_a),
            project_a.to_string_lossy().as_ref(),
        )
        .unwrap();

        // Same path again: reuse, not a new suffix.
        assert_eq!(derive_sandbox_name(&project_a), "widgets");

        // Different path, same basename: clashes, falls through to -copy.
        let name_b = derive_sandbox_name(&project_b);
        assert_eq!(name_b, "widgets-copy");

        let _ = std::fs::remove_file(workspace_record_path("widgets"));
        let _ = std::fs::remove_file(workspace_record_path("widgets-copy"));
        let _ = std::fs::remove_dir_all(&base);
    }
}
