pub mod doctor;
pub mod env;
pub mod host;
pub mod host_check;
pub mod host_provision;
pub mod mac;
pub mod template;
pub mod voice;

use crate::client::Daemon;
use crate::render::Renderer;
use crate::HostsAction;
use anyhow::{bail, Context, Result};
use sbxw_core::config::Config;
use sbxw_core::hosts;
use sbxw_core::names::{self, HostName, SandboxName, SandboxRef};
use sbxw_core::progress::{Progress, Sink};
use sbxw_daemon::hostsys::{Elevation, HostSys};
use sbxw_daemon::provision::BringUp;
use sbxw_daemon::store::{Paths, Store};
use sbxw_daemon::AppBuilder;
use sbxw_sbx::host::HostExec;
use sbxw_sbx::ssh::SshExec;
use sbxw_sbx::Sbx;
use serde_json::json;
use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// A sandbox named on the command line, as a ref: `myapp` or `local~myapp`
/// (this machine), `buildbox~myapp` (a host from `sbxw host add`; remote hosts
/// are experimental).
pub fn sandbox_arg(raw: &str) -> Result<SandboxRef> {
    Ok(SandboxRef::parse(raw)?)
}

/// A remote host from `~/.sbxw/hosts.toml`, over `ssh` on the daemon link's
/// ControlPath: its master if it holds one, else a connection of its own.
fn remote_exec(host: &HostName) -> Result<Arc<SshExec>> {
    let paths = Paths::from_home();
    let hosts = sbxw_daemon::remotes::load_hosts(&paths)?;
    let h = hosts.get(host.as_str()).with_context(|| format!("no remote host named '{host}' — add it with `sbxw host add {host}`"))?;
    let home = paths.root.parent().map(Path::to_path_buf).unwrap_or_default();
    Ok(Arc::new(SshExec::new(h.clone(), sbxw_sbx::ssh::control_path(&home, host))))
}

/// The `sbx` that manages sandboxes on `host`: this machine's, or the
/// host's over ssh.
fn sbx_on(rt: &tokio::runtime::Runtime, host: &HostName) -> Result<Sbx> {
    if host.is_local() {
        return Ok(Sbx::process());
    }
    let exec = remote_exec(host)?;
    Ok(Sbx::new(Arc::new(rt.block_on(exec.on_host())?)))
}

/// One `sbx` per host the refs name, made once each.
struct SbxPerHost<'a> {
    rt: &'a tokio::runtime::Runtime,
    made: HashMap<HostName, Sbx>,
}

impl<'a> SbxPerHost<'a> {
    fn new(rt: &'a tokio::runtime::Runtime) -> Self {
        Self { rt, made: HashMap::new() }
    }
    fn get(&mut self, host: &HostName) -> Result<Sbx> {
        if let Some(s) = self.made.get(host) {
            return Ok(s.clone());
        }
        let s = sbx_on(self.rt, host)?;
        self.made.insert(host.clone(), s.clone());
        Ok(s)
    }
}

/// `sbx …` interactively on a remote host: `ssh -tt` with the host's `sbx`.
fn interactive_on(rt: &tokio::runtime::Runtime, host: &HostName, args: &[String]) -> Result<()> {
    if host.is_local() {
        return sbxw_sbx::interactive(args);
    }
    let exec = remote_exec(host)?;
    let sbx = rt.block_on(exec.sbx_program())?;
    let (program, argv) = exec.tty_argv(&sbx, args);
    let status = std::process::Command::new(&program).args(&argv).status().with_context(|| format!("could not run {program}"))?;
    if !status.success() {
        bail!("`sbx {}` on host '{host}' exited with {status}", args.first().map(String::as_str).unwrap_or(""));
    }
    Ok(())
}

pub fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("a tokio runtime")
}

pub fn load_config(path: &Path) -> Result<Config> {
    Config::load(path, &std::env::current_dir()?)
}

/// The config the daemon starts with: the file `cfg` was loaded from, else
/// `given` made absolute.
pub(crate) fn config_path(cfg: &Config, given: &Path) -> Result<PathBuf> {
    cfg.source.clone().map_or_else(|| abs(given), Ok)
}

pub(crate) fn abs(p: &Path) -> Result<PathBuf> {
    Ok(if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir()?.join(p) })
}

/// Log lines held while the agent owns the terminal (`--no-web`): in raw mode
/// a bare `\n` staircases each line across a full-screen TUI. Printed once the
/// agent exits.
static PARKED: Mutex<Option<Vec<u8>>> = Mutex::new(None);

struct Terminal;

impl std::io::Write for Terminal {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if let Some(buf) = PARKED.lock().unwrap().as_mut() {
            buf.extend_from_slice(b);
            return Ok(b.len());
        }
        std::io::stderr().write(b)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stderr().flush()
    }
}

fn park_logs() {
    *PARKED.lock().unwrap() = Some(Vec::new());
}

fn flush_parked_logs() {
    let parked = PARKED.lock().unwrap().take().unwrap_or_default();
    if !parked.is_empty() {
        eprintln!("\n─ notes from the bring-up (while the agent held the terminal) ─");
        eprint!("{}", String::from_utf8_lossy(&parked));
    }
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("sbxw=info,sbxw_daemon=info,sbx=info")),
        )
        .with_target(false)
        .with_writer(|| Terminal)
        .try_init();
}

/// The sbx version, checked against the floor. Cached per sbx binary (path,
/// size, mtime): `sbx version` runs only after sbx is installed or upgraded.
/// Only a version that passed the floor is cached.
pub(crate) fn check_sbx(rt: &tokio::runtime::Runtime) -> Result<Option<String>> {
    let skip = std::env::var_os(sbxw_sbx::SKIP_VERSION_CHECK_ENV).is_some();
    let cache = Paths::from_home().run().join("sbx-version");
    let key = sbx_binary_key();
    if let (Some(key), Ok(saved)) = (&key, std::fs::read_to_string(&cache)) {
        if let Some((k, v)) = saved.split_once('\n') {
            if k == key && !v.trim().is_empty() {
                return Ok(Some(v.trim().to_string()));
            }
        }
    }
    let v = rt.block_on(Sbx::process().check_version(skip))?;
    // The cache holds the sbx it was written for: a different one is an upgrade.
    let before = std::fs::read_to_string(&cache).ok().and_then(|s| s.lines().nth(1).map(str::to_string)).unwrap_or_default();
    for note in sbxw_sbx::upgrade_notes(&before, v.as_deref().unwrap_or_default()) {
        eprintln!("sbxw  note: {note}");
    }
    if let (Some(key), Some(v)) = (key, &v) {
        let _ = sbxw_daemon::store::write_atomic(&cache, format!("{key}\n{v}\n").as_bytes());
    }
    Ok(v)
}

/// `path|size|mtime` of the `sbx` that PATH resolves to; `None` if not found.
fn sbx_binary_key() -> Option<String> {
    let path = std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join("sbx")).find(|p| p.is_file())?;
    let real = std::fs::canonicalize(&path).unwrap_or(path);
    let meta = std::fs::metadata(&real).ok()?;
    let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    Some(format!("{}|{}|{mtime}", real.display(), meta.len()))
}

/// A sandbox name for a workspace: its folder's name, sanitised, and
/// `-copy`, `-copy-1`, … when that name is recorded for a different path.
fn derive_name(store: &Store, workspace: &Path) -> SandboxName {
    let base = names::sanitize_sandbox_name(&workspace.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
    (0..)
        .map(|n| names::copy_candidate(&base, n))
        .find(|c| match store.workspace(c) {
            None => true,
            Some(existing) => existing == workspace,
        })
        .expect("an unbounded search finds a name")
}

fn web_url(cfg: &Config, sandbox: Option<&str>) -> String {
    let port = cfg.web_port();
    let host = if ports_is_loopback(&cfg.web_addr) {
        format!("http://sbxw.localhost:{port}")
    } else {
        crate::client::base_url(&cfg.web_addr)
    };
    match sandbox {
        Some(s) => format!("{host}/#sandbox={s}"),
        None => host,
    }
}

fn ports_is_loopback(addr: &str) -> bool {
    sbxw_core::ports::ip_of(addr).starts_with("127.")
}

/// The aliases a bring-up wants, written here — in the terminal, the one
/// place `sudo` can ask — so the daemon finds nothing to change.
fn apply_aliases_here(store: &Store, cfg: &Config) {
    let mut wanted = cfg.wanted_aliases(&[]);
    wanted.extend(store.pending_hosts());
    let sys = HostSys::system(if std::io::stdin().is_terminal() { Elevation::Interactive } else { Elevation::Background });
    for w in sys.apply(store, &wanted) {
        eprintln!("sbxw  warning: {w}");
    }
    if sys.missing(&wanted).is_empty() {
        store.clear_pending_hosts();
    }
}

pub struct UpArgs {
    pub name: Option<String>,
    pub path: Option<PathBuf>,
    /// Clone this into `path` first, when it does not exist yet.
    pub git: Option<String>,
    pub ro: Vec<PathBuf>,
    pub add_sandbox: bool,
    pub config: PathBuf,
    pub no_web: bool,
    pub use_api_key: bool,
    pub tail: bool,
    pub chat: bool,
    /// With `template = "auto"`: snapshot this new sandbox as the template.
    pub rebuild_template: bool,
    /// Create it for Remote Control: sbxw signs its agent in with nothing.
    pub remote_control: bool,
}

fn template_choice(rebuild: bool) -> sbxw_core::templates::Choice {
    if rebuild {
        sbxw_core::templates::Choice::Rebuild
    } else {
        sbxw_core::templates::Choice::Default
    }
}

pub fn up(a: UpArgs) -> Result<()> {
    let cfg = load_config(&a.config)?;
    let config_abs = config_path(&cfg, &a.config)?;
    let paths = Paths::from_home();
    let store = Store::open(paths.clone());
    let daemon = Daemon::new(&cfg.web_addr);
    let rt = runtime();
    let sbx_version = check_sbx(&rt)?;

    let workspace = match &a.path {
        Some(p) => p.clone(),
        None => std::env::current_dir()?,
    };
    let name = match (a.name, a.add_sandbox) {
        (Some(n), _) => Some(sandbox_arg(&n)?),
        (None, true) => {
            let ws = std::fs::canonicalize(&workspace).with_context(|| format!("workspace path does not exist: {}", workspace.display()))?;
            let n = derive_name(&store, &ws);
            eprintln!("sbxw  --add-sandbox → using sandbox name '{n}'");
            Some(SandboxRef::local(n))
        }
        (None, false) => None,
    };
    let Some(name) = name else {
        if a.no_web {
            bail!("--no-web needs a sandbox name to attach to");
        }
        apply_aliases_here(&store, &cfg);
        daemon.ensure(&paths, &config_abs, a.use_api_key)?;
        eprintln!("sbxw  daemon ready → {}", web_url(&cfg, None));
        return if a.tail { logs(None, 20) } else { Ok(()) };
    };
    let (workspace, ro) = if name.is_local() {
        // With --git, a path not there yet is cloned into by the daemon.
        let workspace = match (&a.git, std::fs::canonicalize(&workspace)) {
            (_, Ok(w)) => w,
            (Some(_), Err(_)) => abs(&workspace)?,
            (None, Err(_)) => bail!("workspace path does not exist: {}", workspace.display()),
        };
        let ro: Vec<String> = a
            .ro
            .iter()
            .map(|p| std::fs::canonicalize(p).map(|c| c.to_string_lossy().into_owned()))
            .collect::<std::io::Result<_>>()
            .context("a --ro path does not exist")?;
        (workspace, ro)
    } else {
        // The host's paths, checked on the host by the bring-up.
        eprintln!("sbxw  note: remote hosts are experimental — not yet tried on a real KVM host");
        if a.path.is_none() || !workspace.is_absolute() {
            bail!("a sandbox on host '{}' needs --path with an absolute path on that host", name.host);
        }
        if let Some(p) = a.ro.iter().find(|p| !p.is_absolute()) {
            bail!("--ro {}: give an absolute path on host '{}'", p.display(), name.host);
        }
        (workspace, a.ro.iter().map(|p| p.to_string_lossy().into_owned()).collect())
    };
    let api_key = a.use_api_key.then(|| std::env::var("ANTHROPIC_API_KEY").ok()).flatten();
    if a.use_api_key && api_key.is_none() {
        eprintln!("sbxw  warning: --use-api-key set but ANTHROPIC_API_KEY is empty or unset");
    }
    // sbx 0.51 signs Claude in on the host (`/login` once, token kept there),
    // so sbxw writes no token into the sandbox.
    if sbxw_daemon::svc::sandboxes::oauth_token_from_env().is_some() {
        eprintln!(
            "sbxw  note: CLAUDE_CODE_OAUTH_TOKEN is no longer injected — sbx keeps your claude.ai sign-in on the host. \
             Run /login once in a sandbox's agent pane, or store an API key with --use-api-key."
        );
    }

    apply_aliases_here(&store, &cfg);

    if a.no_web {
        return up_here(&rt, paths, cfg, sbx_version, BringUp {
            name,
            workspace,
            ro,
            rw: vec![],
            config: load_config(&a.config)?,
            extra_ports: vec![],
            chat: a.chat,
            api_key,
            template: template_choice(a.rebuild_template),
            remote_control: a.remote_control,
            git: a.git.clone(),
        });
    }

    daemon.ensure(&paths, &config_abs, a.use_api_key)?;
    let mut r = Renderer::new();
    let last = daemon.stream(
        &format!("/api/sandboxes/{name}/up"),
        json!({
            "path": workspace,
            "ro": ro,
            "config": cfg.source,
            "chat": a.chat,
            "api_key": api_key,
            "template": template_choice(a.rebuild_template),
            "remote_control": a.remote_control,
            "git": a.git,
        }),
        |v| {
            if let Some(p) = v.get("progress").and_then(|p| serde_json::from_value::<Progress>(p.clone()).ok()) {
                r.show(&p);
            }
        },
    )?;
    if last["ok"] != true {
        bail!("{}", last["error"].as_str().unwrap_or("the bring-up failed"));
    }
    eprintln!("sbxw  '{name}' is up → {}", web_url(&cfg, Some(&name.to_string())));
    if a.tail {
        logs(Some(name.to_string()), 20)?;
    }
    Ok(())
}

/// `--no-web`: the same pipeline in this process, then the agent on this
/// terminal. The daemon-owned port publisher runs as long as the agent does.
fn up_here(rt: &tokio::runtime::Runtime, paths: Paths, cfg: Config, sbx_version: Option<String>, req: BringUp) -> Result<()> {
    init_tracing();
    let app = AppBuilder {
        paths,
        cfg,
        sbx: Sbx::process(),
        hosts: HostSys::system(Elevation::Interactive),
        use_api_key: req.api_key.is_some(),
        sbx_version,
    }
    .build();
    let r = Mutex::new(Renderer::new());
    let sink: Sink = Arc::new(move |p| r.lock().unwrap().show(&p));
    let name = req.name.clone();
    rt.block_on(app.provision(req, Some(sink)))?;
    eprintln!("sbxw  attaching the agent in this terminal (Ctrl-C to detach)");
    let cfg = &app.cfg;
    let args = sbxw_sbx::attach_args(&name.name, &cfg.env_pairs(), &cfg.env_files, &cfg.workdir);
    // A remote sandbox's agent through this process's link to its host,
    // which stays up (and keeps the hooks' tunnel) while it runs.
    let (program, args) = if name.is_local() {
        ("sbx".to_string(), args)
    } else {
        rt.block_on(async {
            let r = app.ready_remote(&name).await?;
            let exec = r.link.exec();
            let sbx = exec.sbx_program().await?;
            anyhow::Ok(exec.tty_argv(&sbx, &args))
        })?
    };
    park_logs();
    let out = rt.block_on(async move {
        tokio::task::spawn_blocking(move || {
            if program == "sbx" {
                return sbxw_sbx::interactive(&args);
            }
            let status = std::process::Command::new(&program).args(&args).status()?;
            if !status.success() {
                bail!("the agent exited with {status}");
            }
            Ok(())
        })
        .await
    });
    flush_parked_logs();
    out?
}

pub fn chat(name: Option<String>, config: PathBuf, no_web: bool, use_api_key: bool, tail: bool) -> Result<()> {
    let name = match name {
        Some(n) => {
            let r = sandbox_arg(&n)?;
            if !r.is_local() {
                bail!("'{r}': chat sandboxes run on this machine only");
            }
            r.name
        }
        None => names::mint_chat_name(u32::from_str_radix(&sbxw_core::ids::random_hex(4), 16).unwrap_or(0)),
    };
    let paths = Paths::from_home();
    let ws = paths.chat_workspace(&name);
    std::fs::create_dir_all(&ws).with_context(|| format!("could not create {}", ws.display()))?;
    eprintln!("chat sandbox '{name}' → empty workspace {}", ws.display());
    up(UpArgs {
        name: Some(name.to_string()),
        path: Some(ws),
        git: None,
        ro: vec![],
        add_sandbox: false,
        config,
        no_web,
        use_api_key,
        tail,
        chat: true,
        rebuild_template: false,
        remote_control: false,
    })
}

pub fn web(name: &str, config: &Path) -> Result<()> {
    let name = sandbox_arg(name)?;
    let cfg = load_config(config)?;
    let daemon = Daemon::new(&cfg.web_addr);
    daemon.ensure(&Paths::from_home(), &config_path(&cfg, config)?, false)?;
    daemon.post("/api/initial", json!({ "sandbox": name }))?;
    eprintln!("sbxw  → {}", web_url(&cfg, Some(&name.to_string())));
    Ok(())
}

pub fn logs(name: Option<String>, lines: u32) -> Result<()> {
    let log = Paths::from_home().daemon_log();
    if !log.exists() {
        bail!("no daemon log yet ({}) — start it with `sbxw up`", log.display());
    }
    let mut tail = std::process::Command::new(sbxw_core::sysbin::sys_bin("tail"));
    tail.args(["-n", &lines.to_string(), "-f"]).arg(&log);
    match name {
        None => {
            tail.status()?;
        }
        Some(n) => {
            // The log names a sandbox by its ref's canonical form.
            let n = SandboxRef::parse(&n).map(|r| r.to_string()).unwrap_or(n);
            let child = tail.stdout(std::process::Stdio::piped()).spawn()?;
            let out = std::io::BufReader::new(child.stdout.context("no output")?);
            for line in std::io::BufRead::lines(out) {
                let line = line?;
                if line.contains(&n) {
                    println!("{line}");
                }
            }
        }
    }
    Ok(())
}

pub fn ports(name: &str, config: &Path) -> Result<()> {
    let name = sandbox_arg(name)?;
    let cfg = load_config(config)?;
    if !name.is_local() {
        // Published on the host and forwarded here by the daemon's link.
        let daemon = Daemon::new(&cfg.web_addr);
        if daemon.health().is_none() {
            bail!("'{name}' is on another host: its ports are forwarded by the daemon — start it with `sbxw up`");
        }
        for p in cfg.wanted_ports(&[]) {
            let body = json!({ "sandbox_port": p.sandbox_port, "host_port": p.host_port });
            match daemon.post(&format!("/api/sandboxes/{name}/ports/publish"), body) {
                Ok(_) => println!("published {}:{} on host '{}', forwarded here", p.host_port, p.sandbox_port, name.host),
                Err(e) => eprintln!("could not publish {}: {e:#}", p.sandbox_port),
            }
        }
        return Ok(());
    }
    let rt = runtime();
    let sbx = Sbx::process();
    for spec in sbxw_core::ports::publish_specs(&cfg.wanted_ports(&[]), cfg.ip_per_app) {
        match rt.block_on(sbx.publish(&name.name, &spec)) {
            Ok(()) => println!("published {spec}"),
            Err(e) => eprintln!("could not publish {spec}: {e:#}"),
        }
    }
    Ok(())
}

pub fn bash(name: &str) -> Result<()> {
    let name = sandbox_arg(name)?;
    interactive_on(&runtime(), &name.host, &sbxw_sbx::bash_args(&name.name))
}

pub fn ssh(name: Option<String>, setup: bool, command: &[String]) -> Result<()> {
    if setup {
        runtime().block_on(Sbx::process().setup_ssh()).context("`sbx setup ssh` failed")?;
        println!("SSH configured — reach any sandbox with `ssh <name>.sbx`.");
        if name.is_none() {
            return Ok(());
        }
    }
    let Some(name) = name else {
        bail!("specify a sandbox name, or pass --setup to register the `*.sbx` SSH host block");
    };
    let name = sandbox_arg(&name)?;
    if !name.is_local() {
        bail!("`sbxw ssh` reaches this machine's sandboxes; for '{name}' use `sbxw bash {name}`");
    }
    let host = format!("{}.sbx", name.name);
    let status = std::process::Command::new("ssh")
        .arg(&host)
        .args(command)
        .status()
        .context("failed to spawn `ssh` — is an OpenSSH client installed?")?;
    if status.success() {
        return Ok(());
    }
    let configured = std::env::var_os("HOME")
        .and_then(|h| std::fs::read_to_string(PathBuf::from(h).join(".ssh/config")).ok())
        .is_some_and(|s| s.contains(".sbx"));
    if status.code() == Some(255) && !configured {
        bail!("`ssh {host}` failed and no `*.sbx` entry was found in ~/.ssh/config.\nRun `sbxw ssh --setup` once to register it.");
    }
    bail!("`ssh {host}` exited with {status}")
}

/// One row of `sbxw ls`.
struct LsRow {
    name: String,
    agent: String,
    created: String,
    status: String,
}

/// Every sandbox: this machine's, then each remote host's by ref. A host
/// that cannot be reached is listed from sbxw's records, with the state last
/// seen, marked unreachable.
pub fn ls() -> Result<()> {
    let rt = runtime();
    let row = |name: String, s: &sbxw_sbx::SandboxInfo| {
        let dot = if s.is_running() { "●" } else if s.is_stopped() { "○" } else { "?" };
        // RFC 3339 cut to the minute: `2026-10-20T10:00` reads at a glance.
        let created: String = s.created_at.as_deref().unwrap_or("").replace('T', " ").chars().take(16).collect();
        let held = if s.detached { "  (kept up)" } else { "" };
        LsRow { name, agent: s.agent.clone(), created, status: format!("{dot} {}{held}", s.status) }
    };
    let mut rows: Vec<LsRow> = rt.block_on(Sbx::process().ls()).iter().map(|s| row(s.name.clone(), s)).collect();
    let paths = Paths::from_home();
    let store = Store::open(paths.clone());
    let hosts = sbxw_daemon::remotes::load_hosts(&paths).unwrap_or_else(|e| {
        eprintln!("sbxw  warning: {e:#}");
        Default::default()
    });
    for h in hosts.hosts.values() {
        let known: Vec<SandboxRef> = store.names().into_iter().filter(|n| n.host == h.name).collect();
        match rt.block_on(remote_ls(&h.name)) {
            Ok(list) => {
                for s in &list {
                    let Ok(n) = SandboxName::parse(&s.name) else { continue };
                    let r = SandboxRef::new(h.name.clone(), n);
                    if known.contains(&r) && store.get(&r).last_status.as_deref() != Some(s.status.as_str()) {
                        let st = s.status.clone();
                        let _ = store.update(&r, |rec| rec.last_status = Some(st));
                    }
                    rows.push(row(r.to_string(), s));
                }
            }
            Err(e) if sbxw_sbx::host::is_unreachable(&e) => {
                eprintln!("sbxw  {e:#}");
                for r in known {
                    let last = store.get(&r).last_status.unwrap_or_else(|| "unknown".into());
                    rows.push(LsRow { name: r.to_string(), agent: "claude".into(), created: String::new(), status: format!("✕ offline (last seen {last})") });
                }
            }
            Err(e) => eprintln!("sbxw  warning: host '{}': {e:#}", h.name),
        }
    }
    if rows.is_empty() {
        println!("No sandboxes.");
        return Ok(());
    }
    let wn = rows.iter().map(|r| r.name.len()).max().unwrap_or(7).max(7);
    let wa = rows.iter().map(|r| r.agent.len()).max().unwrap_or(5).max(5);
    println!("{:<wn$}  {:<wa$}  {:<20}  STATUS", "SANDBOX", "AGENT", "CREATED");
    println!("{:-<wn$}  {:-<wa$}  {:-<20}  ------", "", "", "");
    for r in &rows {
        println!("{:<wn$}  {:<wa$}  {:<20}  {}", r.name, r.agent, r.created, r.status);
    }
    Ok(())
}

/// `sbx ls` on a remote host. Unlike `Sbx::ls`, a host that cannot be
/// reached is an error (a `HostUnreachable`), not an empty list.
async fn remote_ls(host: &HostName) -> Result<Vec<sbxw_sbx::SandboxInfo>> {
    let exec = remote_exec(host)?;
    let sbx = Sbx::new(Arc::new(exec.on_host().await?));
    sbxw_daemon::svc::sandboxes::list_strict(&sbx).await
}

pub fn ports_ls(name: Option<String>, all: bool) -> Result<()> {
    let rt = runtime();
    let mut per_host = SbxPerHost::new(&rt);
    let names: Vec<String> = match (all, name) {
        // This machine's: a remote host's are listed by naming them.
        (true, _) => rt.block_on(Sbx::process().ls()).into_iter().map(|s| s.name).collect(),
        (false, Some(n)) => vec![n],
        (false, None) => bail!("specify a sandbox name, or pass --all"),
    };
    if names.is_empty() {
        println!("No sandboxes.");
    }
    let multi = names.len() > 1;
    for n in names {
        let sb = sandbox_arg(&n)?;
        if multi {
            println!("=== {n} ===");
        }
        match per_host.get(&sb.host).and_then(|sbx| rt.block_on(sbx.ports_raw(&sb.name))) {
            Ok(out) if out.trim().is_empty() => println!("  (no ports published)"),
            Ok(out) => println!("{}", out.trim_end()),
            Err(e) => eprintln!("  error: {e:#}"),
        }
        if multi {
            println!();
        }
    }
    Ok(())
}

pub fn stop(names: Vec<String>, all: bool) -> Result<()> {
    let rt = runtime();
    let mut per_host = SbxPerHost::new(&rt);
    let targets: Vec<String> = if all {
        // This machine's: a remote host's are stopped by naming them.
        rt.block_on(Sbx::process().ls()).into_iter().filter(|s| s.is_running()).map(|s| s.name).collect()
    } else if names.is_empty() {
        bail!("specify at least one sandbox name, or use --all");
    } else {
        names
    };
    if targets.is_empty() {
        println!("No running sandboxes to stop.");
    }
    for n in targets {
        let sb = sandbox_arg(&n)?;
        let sbx = per_host.get(&sb.host).with_context(|| format!("failed to stop '{n}'"))?;
        rt.block_on(sbx.stop(&sb.name)).with_context(|| format!("failed to stop '{n}'"))?;
        println!("stopped  {n}");
    }
    Ok(())
}

/// Remove, then forget everything kept about them; the daemon closes their
/// terminals if it runs. Each host's sandboxes are removed by its own sbx.
pub fn rm(names: Vec<String>, all: bool) -> Result<()> {
    let rt = runtime();
    let targets: Vec<SandboxRef> = if all {
        // This machine's: a remote host's are removed by naming them.
        rt.block_on(Sbx::process().ls()).into_iter().filter_map(|s| SandboxName::parse(&s.name).ok()).map(SandboxRef::local).collect()
    } else if names.is_empty() {
        bail!("specify at least one sandbox name, or use --all");
    } else {
        names.iter().map(|n| sandbox_arg(n)).collect::<Result<_, _>>()?
    };
    if targets.is_empty() {
        println!("No sandboxes.");
        return Ok(());
    }
    let mut by_host: std::collections::BTreeMap<HostName, Vec<SandboxName>> = Default::default();
    for t in &targets {
        by_host.entry(t.host.clone()).or_default().push(t.name.clone());
    }
    let mut per_host = SbxPerHost::new(&rt);
    let store = Store::open(Paths::from_home());
    let cfg = load_config(Path::new("sbxw.toml")).unwrap_or_default();
    let daemon = Daemon::new(&cfg.web_addr);
    let live = daemon.health().is_some();
    let (mut failed, mut total) = (0, 0);
    for (host, list) in by_host {
        let sbx = per_host.get(&host)?;
        // Named, even for --all, so sbx answers per name.
        let results = rt.block_on(sbx.rm(&list))?;
        total += results.len();
        let gone: Vec<SandboxRef> = list
            .iter()
            .filter(|n| results.iter().any(|r| r.removed && r.name == n.as_str()))
            .map(|n| SandboxRef::new(host.clone(), n.clone()))
            .collect();
        let chats: Vec<PathBuf> = gone.iter().filter_map(|n| store.chat_workspace(n)).collect();
        for n in &gone {
            store.forget(n);
            if live {
                let _ = daemon.post(&format!("/api/sandboxes/{n}/detach"), json!({}));
            }
            println!("removed  {n}");
        }
        for dir in chats {
            let _ = std::fs::remove_dir_all(dir);
        }
        for r in results.iter().filter(|r| !r.removed) {
            failed += 1;
            let shown = SandboxName::parse(&r.name).map(|n| SandboxRef::new(host.clone(), n).to_string()).unwrap_or_else(|_| r.name.clone());
            eprintln!("failed   {shown}: {}", r.error.as_deref().unwrap_or("not removed"));
        }
    }
    if failed > 0 {
        bail!("{failed} of {total} sandbox(es) not removed");
    }
    Ok(())
}

/// `sbxw cache`: the bundled kits' shared downloads (see `sbxw_daemon::cache`).
pub fn cache(action: crate::CacheAction, config: &Path) -> Result<()> {
    use sbxw_daemon::cache;
    let paths = Paths::from_home();
    match action {
        crate::CacheAction::Show => {
            let cfg = load_config(config)?;
            let ttl = cfg.cache_ttl();
            println!("{}{}", paths.cache().display(), if cfg.cache { "" } else { "  (off: cache = false)" });
            let flavours = cache::flavours(&paths);
            if flavours.is_empty() {
                println!("  empty — the next sandbox created with a bundled kit fills it");
            }
            for f in flavours {
                let state = match f.age {
                    Some(a) if a < ttl => format!("filled, expires in {}h", (ttl - a).as_secs() / 3600),
                    Some(_) => "expired — the next sandbox refills it".to_string(),
                    None if f.filled.is_empty() => "incomplete".to_string(),
                    None => format!("partial ({})", f.filled.join(", ")),
                };
                println!("  {:<20} {:>8.1} MB  {state}", f.name, f.bytes as f64 / 1e6);
            }
            if cache::is_filling(&paths) {
                println!("  a sandbox is filling it");
            }
            Ok(())
        }
        crate::CacheAction::Clear => {
            cache::clear(&paths)?;
            println!("removed {} — the next sandbox refills it", paths.cache().display());
            Ok(())
        }
    }
}

/// `sbx prune`, previewed and confirmed, then the cleanup `rm` does for
/// whatever disappeared: a name-set difference, robust to prune's output.
pub fn prune(since: Option<&str>, dry_run: bool, yes: bool) -> Result<()> {
    let rt = runtime();
    check_sbx(&rt)?;
    let sbx = Sbx::process();
    let preview = rt.block_on(sbx.prune(since, true))?;
    if !preview.trim().is_empty() {
        println!("{}", preview.trim_end());
    }
    if dry_run {
        return Ok(());
    }
    let before: Vec<String> = rt.block_on(sbx.ls()).into_iter().map(|s| s.name).collect();
    if !yes {
        if !std::io::stdin().is_terminal() {
            bail!("`sbxw prune` removes sandboxes permanently and there is no terminal to confirm on — re-run with --yes (or --dry-run)");
        }
        eprint!("remove the stopped sandboxes listed above? [y/N] ");
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim(), "y" | "Y" | "yes" | "Yes") {
            println!("nothing removed");
            return Ok(());
        }
    }
    let out = rt.block_on(sbx.prune(since, false))?;
    if !out.trim().is_empty() {
        println!("{}", out.trim_end());
    }
    let after: Vec<String> = rt.block_on(sbx.ls()).into_iter().map(|s| s.name).collect();
    let store = Store::open(Paths::from_home());
    for gone in before.iter().filter(|n| !after.contains(n)) {
        // sbx lists this machine's sandboxes only.
        let Ok(gone) = SandboxName::parse(gone) else { continue };
        if let Some(dir) = store.chat_workspace(&gone) {
            let _ = std::fs::remove_dir_all(dir);
        }
        store.forget(&gone);
    }
    Ok(())
}

pub fn hosts(action: HostsAction, config: &Path) -> Result<()> {
    let store = Store::open(Paths::from_home());
    let sys = HostSys::system(Elevation::Interactive);
    match action {
        HostsAction::Show => {
            let entries = sys.read_block();
            if entries.is_empty() {
                println!("no sbxw entries in /etc/hosts");
            }
            for a in &entries {
                println!("{}\t{}", a.ip, a.hostname);
            }
            let pending = store.pending_hosts();
            if !pending.is_empty() {
                println!("\npending (run `sbxw hosts sync` to apply):");
                for a in &pending {
                    println!("{}\t{}", a.ip, a.hostname);
                }
            }
        }
        HostsAction::Clear => {
            sys.clear()?;
            store.clear_pending_hosts();
            println!("removed the sbxw /etc/hosts block");
        }
        HostsAction::Sync => {
            let cfg = load_config(config)?;
            let mut wanted = cfg.wanted_aliases(&[]);
            wanted.extend(store.pending_hosts());
            sys.ensure_loopback(&wanted)?;
            sys.merge(&wanted)?;
            let missing = hosts::missing(&sys.read_block(), &wanted);
            if !missing.is_empty() {
                bail!("still missing from /etc/hosts: {}", missing.join(", "));
            }
            store.clear_pending_hosts();
            for a in &wanted {
                println!("{}\t{}", a.ip, a.hostname);
            }
            println!("/etc/hosts is up to date");
        }
    }
    Ok(())
}

/// No name: stop the daemon (it closes every terminal) and clear the hosts
/// block. A name: close that sandbox's terminals only.
pub fn down(name: Option<String>) -> Result<()> {
    let cfg = load_config(Path::new("sbxw.toml")).unwrap_or_default();
    let daemon = Daemon::new(&cfg.web_addr);
    let paths = Paths::from_home();
    match name {
        Some(n) => {
            let n = sandbox_arg(&n)?;
            if daemon.health().is_none() {
                println!("no sbxw daemon is running");
                return Ok(());
            }
            daemon.post(&format!("/api/sandboxes/{n}/detach"), json!({}))?;
            println!("closed the terminals of '{n}'");
        }
        None => {
            if daemon.health().is_some() {
                let _ = daemon.post("/api/shutdown", json!({}));
                println!("stopped the sbxw daemon");
            } else if let Ok(pid) = std::fs::read_to_string(paths.daemon_pid()) {
                let _ = std::process::Command::new(sbxw_core::sysbin::sys_bin("kill")).arg(pid.trim()).status();
                println!("stopped the sbxw daemon (pid {})", pid.trim());
            } else {
                println!("no sbxw daemon is running");
            }
            let _ = std::fs::remove_file(paths.daemon_pid());
            HostSys::system(Elevation::Interactive).clear()?;
            println!("removed the sbxw /etc/hosts block");
        }
    }
    Ok(())
}

/// The daemon itself, in the foreground (its own log is wherever stdout goes).
pub fn daemon(config: &Path, use_api_key: bool) -> Result<()> {
    init_tracing();
    let cfg = load_config(config)?;
    let rt = runtime();
    let sbx_version = check_sbx(&rt)?;
    let paths = Paths::from_home();
    std::fs::create_dir_all(paths.run())?;
    std::fs::write(paths.daemon_pid(), std::process::id().to_string())?;
    tracing::info!("sbxw daemon {} (sbx {}) — config {}", env!("CARGO_PKG_VERSION"), sbx_version.as_deref().unwrap_or("?"), cfg.source.as_ref().map_or("defaults".into(), |p| p.display().to_string()));
    let app = AppBuilder {
        paths: paths.clone(),
        cfg,
        sbx: Sbx::process(),
        hosts: HostSys::system(Elevation::Background),
        use_api_key,
        sbx_version,
    }
    .build();
    let result = rt.block_on(async move {
        app.start_background();
        sbxw_daemon::http::serve(app).await
    });
    let _ = std::fs::remove_file(paths.daemon_pid());
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sandbox_argument_is_a_ref() {
        assert_eq!(sandbox_arg("myapp").unwrap().to_string(), "myapp");
        assert_eq!(sandbox_arg("local~myapp").unwrap().to_string(), "myapp");
        let r = sandbox_arg("buildbox~myapp").unwrap();
        assert_eq!((r.host.as_str(), r.name.as_str()), ("buildbox", "myapp"));
        assert!(sandbox_arg("a~b~c").is_err());
        assert!(sandbox_arg("ne os").is_err());
    }

    #[test]
    fn a_derived_name_steps_aside_for_another_path() {
        let d = tempfile_dir();
        let store = Store::open(Paths::under(&d));
        let a = d.join("x/myapp");
        let b = d.join("y/myapp");
        assert_eq!(derive_name(&store, &a).as_str(), "myapp");
        store.update(SandboxName::parse("myapp").unwrap(), |r| r.workspace = Some(a.clone())).unwrap();
        assert_eq!(derive_name(&store, &a).as_str(), "myapp", "its own record is no clash");
        assert_eq!(derive_name(&store, &b).as_str(), "myapp-copy");
        let _ = std::fs::remove_dir_all(d);
    }

    fn tempfile_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("sbxw-cli-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_url_names_the_sandbox() {
        let cfg = Config::default();
        assert_eq!(web_url(&cfg, Some("myapp")), "http://sbxw.localhost:7681/#sandbox=myapp");
    }
}
