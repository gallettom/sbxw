//! `sbxw env run` and `sbxw env export`.
//!
//! **run**: `sbx env create` does the creation — it is the only thing that
//! applies an environment file whole (secrets, bindings, registries, MCP,
//! skills, kits, mounts) — then the rest of sbxw's pipeline runs through the
//! daemon like any `sbxw up`. Host ports are settled *before* sbx is called:
//! a port sbx cannot publish costs the whole sandbox (and leaves its secrets
//! behind), and only rewriting the file can move one.
//!
//! **export**: `sbxw.toml` as the file sbx reads, written beside the
//! workspace, never inside it.

use super::{load_config, up, UpArgs};
use anyhow::{bail, Context, Result};
use sbxw_core::envexport;
use sbxw_core::envfile::{self, Port};
use sbxw_core::names::{self, SandboxName};
use sbxw_daemon::store::{write_atomic, Paths, Store};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

fn host_port_is_free(ip: &str, port: u16) -> bool {
    let addr = match ip.parse() {
        Ok(ip) => std::net::SocketAddr::new(ip, port),
        Err(_) => std::net::SocketAddr::from(([0, 0, 0, 0], port)),
    };
    std::net::TcpListener::bind(addr).is_ok()
}

/// Within a hundred: past that something is wrong and a person should look.
fn next_free(ip: &str, from: u16) -> Option<u16> {
    (from..from.saturating_add(100)).find(|p| host_port_is_free(ip, *p))
}

/// The ports to use and whether any moved. Ephemeral (no host) and UDP ones
/// are left alone — a TCP bind cannot test a UDP port, and an untested port is
/// not reported as tested. With a terminal each conflict is a question; without
/// one, the next free port is taken and said.
fn negotiate(ports: &[Port], interactive: bool) -> Result<(Vec<Port>, bool)> {
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
        let suggestion = next_free(ip, want.saturating_add(1));
        let chosen = if interactive && std::io::stdin().is_terminal() {
            eprint!(
                "  ! host port {want} ({ip}) is busy — port for sandbox:{} [{}] ",
                p.sandbox,
                suggestion.map(|s| s.to_string()).unwrap_or_default()
            );
            let _ = std::io::stderr().flush();
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer)?;
            match answer.trim() {
                "" => suggestion.context("no free host port near the one in the environment file")?,
                other => other.parse().with_context(|| format!("'{other}' is not a port number"))?,
            }
        } else {
            let s = suggestion.with_context(|| format!("host port {want} is busy and no free port was found near it"))?;
            eprintln!("  ! host port {want} ({ip}) is busy — using {s} for sandbox:{}", p.sandbox);
            s
        };
        moved |= chosen != want;
        out.push(Port { host: Some(chosen), ..p.clone() });
    }
    Ok((out, moved))
}

#[allow(clippy::too_many_arguments)]
pub fn run(paths: Vec<PathBuf>, config: PathBuf, no_web: bool, yes: bool, use_api_key: bool, tail: bool, env_args: Vec<String>) -> Result<()> {
    let rt = super::runtime();
    super::check_sbx(&rt)?;
    let cli_args: Vec<(String, String)> = env_args
        .iter()
        .map(|pair| {
            pair.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .with_context(|| format!("--env-arg {pair}: expected NAME=VALUE"))
        })
        .collect::<Result<_>>()?;
    let cwd = std::env::current_dir()?;
    let files = envfile::resolve_paths(&paths, &cwd)?;
    let base = envfile::user_base();
    let mut loaded = envfile::load(&files, base.as_deref(), &cli_args)?;
    let spec = loaded.spec()?;
    for name in &loaded.legacy_vars {
        eprintln!("sbxw  warning: ${{{name}}} is not expanded since sbx 0.42 — use `${{{{ env.args.{name} }}}}` with --env-arg, or a path relative to the file");
    }
    for name in &loaded.unresolved {
        eprintln!("sbxw  warning: ${{{{ {name} }}}} has no value — left as written, so sbx sees it unresolved too");
    }
    for (secret, command) in loaded.relative_secret_commands() {
        eprintln!("sbxw  warning: secrets.{secret}.command (`{command}`) uses a relative path; since sbx 0.46 it runs from a fresh temporary directory");
    }
    let shown = loaded.sources.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(" + ");
    let Some(workspace) = spec.workspace.as_deref().map(PathBuf::from) else {
        bail!("{shown} declares no `workspace:` — since sbx 0.42 that mounts nothing. Add `workspace: .`");
    };
    let workspace = std::fs::canonicalize(&workspace).with_context(|| format!("the environment file's workspace does not exist: {}", workspace.display()))?;
    let agent = spec.agent.clone().unwrap_or_else(|| "claude".into());
    let name = match spec.name.clone() {
        Some(n) => SandboxName::parse(&n)?,
        None => names::sanitize_sandbox_name(&format!(
            "{agent}-{}",
            workspace.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "sandbox".into())
        )),
    };
    println!("sbxw  {shown}{} → sandbox '{name}'", loaded.user_source.as_ref().map(|u| format!(" (over {})", u.display())).unwrap_or_default());

    let store = Store::open(Paths::from_home());
    let sbx = sbxw_sbx::Sbx::process();
    if rt.block_on(sbx.exists(&name)) {
        println!("sbxw  '{name}' already exists — not re-created (sbx applies a file's workspaces, kits, ports and secrets at creation only)");
    } else {
        let (ports, moved) = negotiate(&spec.ports, !yes)?;
        let targets: Vec<String> = if moved {
            let (from_base, own) = ports.split_at(spec.user_ports);
            if from_base != &spec.ports[..spec.user_ports] {
                bail!("a busy host port is declared in ~/.sbxenv.yaml, which sbx merges beneath every project — change it there");
            }
            loaded.set_ports(own);
            loaded.set_workspace(&workspace.to_string_lossy());
            let dir = store.paths.env_dir(&name);
            write_atomic(&dir.join(envfile::FILE_NAME), loaded.to_yaml()?.as_bytes())?;
            println!("sbxw  ports adjusted — running from a rewritten copy at {}", dir.join(envfile::FILE_NAME).display());
            vec![dir.to_string_lossy().into_owned()]
        } else {
            envfile::absolute_inputs(&paths, &cwd).iter().map(|p| p.to_string_lossy().into_owned()).collect()
        };
        if !spec.delegated.is_empty() {
            println!("sbxw  handing {} to sbx — sbxw does not read those", spec.delegated.join(", "));
        }
        // sbx shows a plan and asks: the terminal is the one place to answer.
        sbxw_sbx::interactive(&sbxw_sbx::env_create_args(&targets, &name, &env_args))?;
        if !rt.block_on(sbx.exists(&name)) {
            bail!("`sbx env create` reported success but no sandbox named '{name}' exists — check `sbx ls`");
        }
    }

    // The ports the sandbox *has*: an ephemeral or renegotiated one differs
    // from the file, and an alias pointed at the wrong port looks like a
    // broken dev server.
    let published: Vec<Port> = rt.block_on(sbx.ports(&name)).iter().map(Port::from).collect();
    let effective = envfile::Spec {
        ports: if published.is_empty() { spec.ports.clone() } else { published },
        ..spec
    };
    let (cfg, borrowed) = envexport::config_from_spec(&effective, load_config(&config)?);
    if !borrowed.is_empty() {
        println!("sbxw  taken from {}:", if config.exists() { config.display().to_string() } else { "sbxw's defaults".into() });
        for line in &borrowed {
            println!("        · {line}");
        }
    }
    // The merge, on disk: the daemon reads the sandbox's config from a file.
    let merged = store.paths.env_dir(&name).join("sbxw.toml");
    write_atomic(&merged, cfg.to_toml()?.as_bytes())?;
    up(UpArgs {
        name: Some(name.to_string()),
        path: Some(workspace),
        ro: vec![],
        add_sandbox: false,
        config: merged,
        no_web,
        use_api_key,
        tail,
        chat: false,
        rebuild_template: false,
    })
}

pub fn export(path: Option<PathBuf>, name: Option<String>, config: PathBuf, out: Option<PathBuf>, force: bool) -> Result<()> {
    let cfg = load_config(&config)?;
    let workspace = std::fs::canonicalize(path.unwrap_or(std::env::current_dir()?)).context("workspace path does not exist")?;
    let name = match name {
        Some(n) => SandboxName::parse(&n)?,
        None => names::sanitize_sandbox_name(&workspace.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()),
    };
    let dest = out.unwrap_or_else(|| envexport::export_dir_for(&workspace, None).join(envfile::FILE_NAME));
    let to_stdout = dest.as_os_str() == "-";
    let dir = if to_stdout {
        envexport::export_dir_for(&workspace, None)
    } else {
        let parent = match dest.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => std::env::current_dir()?,
        };
        std::fs::canonicalize(&parent).with_context(|| format!("no such directory: {}", parent.display()))?
    };
    let mut cfg = cfg;
    cfg.kits = sbxw_daemon::bundled::resolve(&sbxw_daemon::store::Paths::from_home(), &cfg.kits)?;
    let file = envexport::env_file_for(&name, &cfg, &workspace, envexport::ports_from_config(&cfg), &dir);
    let rendered = file.render(&format!("sbxw {}", env!("CARGO_PKG_VERSION")));
    if to_stdout {
        print!("{rendered}");
        return Ok(());
    }
    if dest.exists() && !force {
        bail!("{} already exists — pass --force to overwrite it", dest.display());
    }
    if dir.starts_with(&workspace) {
        eprintln!("sbxw  warning: {} is inside the workspace, so an agent can edit the file that configures its next sandbox", dest.display());
    }
    std::fs::write(&dest, &rendered).with_context(|| format!("writing {}", dest.display()))?;
    println!("wrote {}", dest.display());
    if dest.file_name() == Some(std::ffi::OsStr::new(envfile::FILE_NAME)) {
        println!("run it with:  cd {} && sbx env run", dir.display());
    } else {
        println!("run it with:  sbx env run {}", dest.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_free_port_is_kept_and_ephemeral_or_udp_left_alone() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let busy = l.local_addr().unwrap().port();
        let ports = vec![
            Port { sandbox: 1, host: None, host_ip: None, protocol: None },
            Port { sandbox: 2, host: Some(busy), host_ip: Some("127.0.0.1".into()), protocol: Some("udp".into()) },
            Port { sandbox: 3, host: Some(busy), host_ip: Some("127.0.0.1".into()), protocol: None },
        ];
        let (out, moved) = negotiate(&ports, false).unwrap();
        assert!(moved);
        assert_eq!(out[0].host, None);
        assert_eq!(out[1].host, Some(busy));
        assert_ne!(out[2].host, Some(busy));
    }
}
