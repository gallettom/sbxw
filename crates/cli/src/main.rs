//! sbxw — a thin wrapper around the standalone `sbx` (Docker Sandboxes) CLI for
//! local development with the Claude Code agent. Never `docker sandbox`, only `sbx`.
//!
//! A **client** of one long-lived daemon (`sbxw daemon`, started on demand).
//! `sbxw up` asks for the sudo password for `/etc/hosts` (only a terminal can),
//! then hands the bring-up to the daemon and renders its progress. Plain `sbx`
//! calls (`ls`, `stop`, `rm`, …) run directly.

mod client;
mod cmd;
mod extension;
mod render;
mod update;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use std::path::PathBuf;

fn banner() -> String {
    use std::io::IsTerminal;
    let colour = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    let (blue, green, reset) = if colour {
        ("\x1b[1;38;5;75m", "\x1b[1;38;5;77m", "\x1b[0m")
    } else {
        ("", "", "")
    };
    // The web UI header (`>_sbxw` in bold mono), drawn in quadrant blocks: 2×2
    // dots a cell, sampled twice as finely across as down (cells are 2:1).
    const ROWS: [(&str, &str, &str); 6] = [
        ("                   ", "        ▐█▌", ""),
        ("▗▄▖                ", " ▗▄▄▄▄  ▐█▙▄▄▄ ", " ▄▄  ▄▄ ▄▖    ▄▄"),
        ("▝▀▜██▄▄            ", "▗██▀▀▀  ▐█▛▀▜█▌", " ▝█▙▟█▘ ▜█ ▄▄ █▌"),
        ("  ▄▄▟██▘           ", " ▀▜██▙▖ ▐█▌  ██", "  ▗██▌  ▐█▄██▐█▘"),
        ("▐█▀▀▘   ▄▄▄▄▄▄▄▖   ", "▗▄▄▄▟█▌ ▐█▙▄▟█▌", " ▗█▛▜█▄  ██▌▜██"),
        ("        ▀▀▀▀▀▀▀▘   ", " ▀▀▀▀▘  ▝▀▘▀▀▘ ", " ▀▀  ▀▀▘ ▀▀ ▝▀▘"),
    ];
    let mut out = String::from("\n");
    for (p, sb, xw) in ROWS {
        out.push_str(&format!("  {green}{p}{blue}{sb}{green}{xw}{reset}\n"));
    }
    out.pop();
    out
}

#[derive(Parser)]
#[command(name = "sbxw", version, about = "Light wrapper around `sbx` for Claude Code dev sandboxes", before_help = banner())]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create (if needed) and provision a sandbox, served by the sbxw daemon.
    /// Omit the name to just start the daemon (browse/create from the UI).
    #[command(after_help = "Remote hosts (EXPERIMENTAL, not yet tried on a real KVM host):\n  \
        sbxw up buildbox~myapp --path /srv/myapp      a sandbox on host `buildbox` (see `sbxw host add`)\n  \
        sbxw up buildbox~myapp --path /srv/myapp --git https://example.org/myapp.git\n\n\
        The path is the host's; its sbxw.toml is read there. Secrets go only to a host added with --trust-secrets.")]
    Up {
        /// A sandbox name, or `<host>~<name>` for one on a remote host (experimental).
        name: Option<String>,
        /// Code path the agent edits in place. Defaults to the current directory.
        /// For a remote sandbox, an absolute path on that host.
        path: Option<PathBuf>,
        /// The same as the PATH argument, spelled as a flag.
        #[arg(long = "path", value_name = "DIR", conflicts_with = "path")]
        path_flag: Option<PathBuf>,
        /// Clone this Git URL into the path first, when the path does not exist yet.
        #[arg(long, value_name = "URL")]
        git: Option<String>,
        /// Extra directories to mount read-only (repeatable).
        #[arg(long = "ro", value_name = "DIR")]
        ro: Vec<PathBuf>,
        /// Without a name, derive one from the workspace directory.
        #[arg(long = "add-sandbox")]
        add_sandbox: bool,
        #[arg(long, default_value = "sbxw.toml")]
        config: PathBuf,
        /// Attach the agent in this terminal instead (no daemon).
        #[arg(long)]
        no_web: bool,
        /// Store ANTHROPIC_API_KEY as the global `anthropic` secret.
        #[arg(long)]
        use_api_key: bool,
        /// Follow the daemon log afterwards.
        #[arg(long)]
        tail: bool,
        /// With `template = "auto"`: once this new sandbox is up, save it as
        /// the template for its kits and remove the previous one.
        #[arg(long)]
        rebuild_template: bool,
        /// Create the sandbox for Remote Control: no OAuth token or API key
        /// from sbxw; run /login in its agent to sign in to claude.ai.
        #[arg(long)]
        remote_control: bool,
        /// Approve the host access the config asks for (mounts, host files,
        /// kits, network, host command, listeners) as shown, without asking.
        #[arg(long)]
        allow_host_access: bool,
    },
    /// A throwaway chat sandbox: an agent on an empty workspace.
    #[command(after_help = "Examples:\n  sbxw chat\n  sbxw chat brainstorm\n  sbxw rm brainstorm")]
    Chat {
        name: Option<String>,
        #[arg(long, default_value = "sbxw.toml")]
        config: PathBuf,
        #[arg(long)]
        no_web: bool,
        #[arg(long)]
        use_api_key: bool,
        #[arg(long)]
        tail: bool,
        /// Approve the host access the config asks for (mounts, host files,
        /// kits, network, host command, listeners) as shown, without asking.
        #[arg(long)]
        allow_host_access: bool,
    },
    /// What a person approved of an sbxw.toml's host access, and revoking it.
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
    /// Tail the daemon's log.
    Logs {
        /// Only lines mentioning this sandbox.
        name: Option<String>,
        #[arg(short = 'n', long, default_value = "40")]
        lines: u32,
    },
    /// (Re)publish the configured ports of a running sandbox.
    Ports {
        name: String,
        #[arg(long, default_value = "sbxw.toml")]
        config: PathBuf,
    },
    /// Open the web UI on a sandbox (starting the daemon if needed).
    Web {
        name: String,
        #[arg(long, default_value = "sbxw.toml")]
        config: PathBuf,
        /// Approve the host access the config asks for (mounts, host files,
        /// kits, network, host command, listeners) as shown, without asking.
        #[arg(long)]
        allow_host_access: bool,
    },
    /// An interactive bash shell in a running sandbox.
    Bash { name: String },
    /// SSH into `<name>.sbx`, or run one command there (experimental).
    Ssh {
        name: Option<String>,
        #[arg(long)]
        setup: bool,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Import your host agents' skills into the store sandboxes share.
    Skills {
        #[command(subcommand)]
        cmd: SkillsCmd,
    },
    /// List sandboxes.
    Ls,
    /// Published port mappings for one or all sandboxes.
    PortsLs {
        name: Option<String>,
        #[arg(long)]
        all: bool,
    },
    /// Stop sandboxes (state kept).
    Stop {
        names: Vec<String>,
        #[arg(long)]
        all: bool,
    },
    /// Remove sandboxes permanently, and everything sbxw keeps about them.
    Rm {
        names: Vec<String>,
        #[arg(long)]
        all: bool,
    },
    /// Remove every *stopped* sandbox, after showing them.
    Prune {
        #[arg(long, visible_alias = "until", value_name = "DURATION")]
        since: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Check sbxw (sbx version, config, daemon, local kits), then summarise
    /// `sbx diagnose`.
    Doctor {
        #[arg(long, default_value = "sbxw.toml")]
        config: PathBuf,
        /// Print sbx's full diagnose report as JSON instead.
        #[arg(long)]
        json: bool,
    },
    /// Snapshot a set-up sandbox, so new ones start from it (`template` in sbxw.toml).
    Template {
        #[command(subcommand)]
        cmd: TemplateCmd,
    },
    /// The virtual Mac (terminal-mac) the `sbxw:terminal-mac` kit lends to sandboxes.
    Mac {
        #[command(subcommand)]
        cmd: cmd::mac::MacCmd,
        #[arg(long, default_value = "sbxw.toml")]
        config: PathBuf,
    },
    /// The voice concierge's local models: status, install, cancel, rm.
    Voice {
        #[command(subcommand)]
        cmd: Option<cmd::voice::VoiceCmd>,
        #[arg(long, default_value = "sbxw.toml")]
        config: PathBuf,
    },
    /// The shared download cache of the bundled kits: show or clear it.
    Cache {
        #[command(subcommand)]
        action: Option<CacheAction>,
        #[arg(long, default_value = "sbxw.toml")]
        config: PathBuf,
    },
    /// Run or write an `sbxenv.yaml` environment file.
    Env {
        #[command(subcommand)]
        cmd: EnvCmd,
    },
    /// Show, re-apply (the one place sudo may ask) or remove the /etc/hosts block.
    Hosts {
        #[command(subcommand)]
        action: Option<HostsAction>,
        #[arg(long, default_value = "sbxw.toml")]
        config: PathBuf,
    },
    /// Remote hosts: the registry in ~/.sbxw/hosts.toml (not the /etc/hosts block).
    Host {
        #[command(subcommand)]
        cmd: cmd::host::HostCmd,
    },
    /// Stop the daemon and remove the /etc/hosts block; with a name, close
    /// that sandbox's terminals only.
    Down { name: Option<String> },
    /// Install the latest release in place of this binary.
    Update {
        #[arg(long)]
        check: bool,
        #[arg(long)]
        no_island: bool,
    },
    /// Install the browser extension (side panel beside the app under test).
    Extension {
        #[arg(long, default_value = "sbxw.toml")]
        config: PathBuf,
        #[arg(long)]
        daemon: Option<String>,
        #[arg(long, hide = true)]
        refresh: bool,
    },
    /// Print a shell completion script.
    Completion { shell: Option<clap_complete::Shell> },
    /// Run the daemon in the foreground (what `sbxw up` starts in the background).
    #[command(hide = true)]
    Daemon {
        #[arg(long, default_value = "sbxw.toml")]
        config: PathBuf,
        #[arg(long)]
        use_api_key: bool,
    },
}

#[derive(Subcommand)]
enum EnvCmd {
    /// Bring a sandbox up from an `sbxenv.yaml`, with sbxw's pipeline around it.
    Run {
        paths: Vec<PathBuf>,
        #[arg(long, default_value = "sbxw.toml")]
        config: PathBuf,
        #[arg(long)]
        no_web: bool,
        #[arg(long, short = 'y')]
        yes: bool,
        #[arg(long)]
        use_api_key: bool,
        #[arg(long)]
        tail: bool,
        #[arg(long = "env-arg", value_name = "NAME=VALUE")]
        env_args: Vec<String>,
    },
    /// Write an `sbxenv.yaml` beside the workspace, from sbxw.toml.
    Export {
        path: Option<PathBuf>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value = "sbxw.toml")]
        config: PathBuf,
        #[arg(long, short = 'o', value_name = "FILE")]
        out: Option<PathBuf>,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum SkillsCmd {
    Import {
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum TemplateCmd {
    /// Save a sandbox as a local template (its secrets are taken out first).
    Save {
        name: String,
        /// The template's tag, e.g. `myproject:base`.
        tag: String,
    },
    /// List saved templates.
    Ls,
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// The host-access grants approved for a config file, and those it asks
    /// for that are not approved yet.
    Grants {
        #[arg(long, default_value = "sbxw.toml")]
        config: PathBuf,
        /// Forget what was approved: the next bring-up asks again.
        #[arg(long)]
        revoke: bool,
    },
}

#[derive(Subcommand, Clone, Copy, Default)]
pub enum HostsAction {
    #[default]
    Show,
    Sync,
    Clear,
}

#[derive(Subcommand, Clone, Copy, Default)]
pub enum CacheAction {
    #[default]
    Show,
    /// Empty it; the next sandbox created refills it.
    Clear,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Up { name, path, path_flag, git, ro, add_sandbox, config, no_web, use_api_key, tail, rebuild_template, remote_control, allow_host_access } => cmd::up_with(
            cmd::UpArgs { name, path: path.or(path_flag), git, ro, add_sandbox, config, no_web, use_api_key, tail, chat: false, rebuild_template, remote_control },
            allow_host_access,
        ),
        Cmd::Chat { name, config, no_web, use_api_key, tail, allow_host_access } => cmd::chat(name, config, no_web, use_api_key, tail, allow_host_access),
        Cmd::Config { cmd: ConfigCmd::Grants { config, revoke } } => cmd::config_grants(&config, revoke),
        Cmd::Logs { name, lines } => cmd::logs(name, lines),
        Cmd::Ports { name, config } => cmd::ports(&name, &config),
        Cmd::Web { name, config, allow_host_access } => cmd::web(&name, &config, allow_host_access),
        Cmd::Bash { name } => cmd::bash(&name),
        Cmd::Ssh { name, setup, command } => cmd::ssh(name, setup, &command),
        Cmd::Skills { cmd: SkillsCmd::Import { dry_run, force } } => {
            let mut a = vec!["skills".to_string(), "import".into()];
            if dry_run {
                a.push("--dry-run".into());
            }
            if force {
                a.push("--force".into());
            }
            sbxw_sbx::interactive(&a)
        }
        Cmd::Ls => cmd::ls(),
        Cmd::PortsLs { name, all } => cmd::ports_ls(name, all),
        Cmd::Stop { names, all } => cmd::stop(names, all),
        Cmd::Rm { names, all } => cmd::rm(names, all),
        Cmd::Prune { since, dry_run, yes } => cmd::prune(since.as_deref(), dry_run, yes),
        Cmd::Env { cmd: EnvCmd::Run { paths, config, no_web, yes, use_api_key, tail, env_args } } => {
            cmd::env::run(paths, config, no_web, yes, use_api_key, tail, env_args)
        }
        Cmd::Env { cmd: EnvCmd::Export { path, name, config, out, force } } => cmd::env::export(path, name, config, out, force),
        Cmd::Hosts { action, config } => cmd::hosts(action.unwrap_or_default(), &config),
        Cmd::Host { cmd } => cmd::host::run(cmd),
        Cmd::Doctor { config, json } => cmd::doctor::run(&config, json),
        Cmd::Template { cmd: TemplateCmd::Save { name, tag } } => cmd::template::save(&name, &tag),
        Cmd::Template { cmd: TemplateCmd::Ls } => cmd::template::ls(),
        Cmd::Mac { cmd, config } => cmd::mac::run(cmd, &config),
        Cmd::Voice { cmd, config } => cmd::voice::run(cmd.unwrap_or_default(), &config),
        Cmd::Cache { action, config } => cmd::cache(action.unwrap_or_default(), &config),
        Cmd::Down { name } => cmd::down(name),
        Cmd::Update { check, no_island } => update::run(check, no_island),
        Cmd::Extension { config, daemon, refresh } => {
            let cfg = cmd::load_config(&config)?;
            extension::cmd(&cfg.web_addr, daemon, refresh)
        }
        Cmd::Completion { shell } => {
            let shell = shell
                .or_else(detect_shell)
                .ok_or_else(|| anyhow::anyhow!("could not detect your shell from $SHELL — pass one, e.g. `sbxw completion zsh`"))?;
            let mut c = Cli::command();
            let name = c.get_name().to_string();
            clap_complete::generate(shell, &mut c, name, &mut std::io::stdout());
            Ok(())
        }
        Cmd::Daemon { config, use_api_key } => cmd::daemon(&config, use_api_key),
    }
}

fn detect_shell() -> Option<clap_complete::Shell> {
    let shell = std::env::var("SHELL").ok()?;
    match std::path::Path::new(&shell).file_name()?.to_str()? {
        "bash" => Some(clap_complete::Shell::Bash),
        "zsh" => Some(clap_complete::Shell::Zsh),
        "fish" => Some(clap_complete::Shell::Fish),
        "elvish" => Some(clap_complete::Shell::Elvish),
        "pwsh" | "powershell" => Some(clap_complete::Shell::PowerShell),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cli_is_well_formed() {
        Cli::command().debug_assert();
    }
}
