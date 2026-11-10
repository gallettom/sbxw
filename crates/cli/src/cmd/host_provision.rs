//! `sbxw host provision <name>`: take a plain Ubuntu host to "sbx installed,
//! signed in, network policy set", interactively, in the user's own terminal.
//!
//! Why interactive: sudo on the host needs a password and sbxw never sees it,
//! so the two install commands run through `ssh -t` with the user's terminal
//! attached and the user types it there. The Docker PAT (Personal Access
//! Token) is typed once without echo and goes to the host on stdin of
//! `sbx login --password-stdin`: never argv, a file or a log, and it is not
//! kept after that call.
//!
//! Every step prints `✓`, `-` (skipped, with the reason) or `✗` and the run
//! stops at the first `✗` with what to do. Checks come first and run before
//! anything is installed. `--dry-run` still runs the read-only probes (it
//! needs them to know what a real run would do) and prints every command that
//! would change the host instead of running it.
//!
//! What was verified against Docker's published CLI reference (2026-11-04) and
//! what was not is written down in docs/remote-hosts.md, "Provisioning".

use super::host_check::{Outcome, Report, Step};
use anyhow::{bail, Context, Result};
use sbxw_core::remote_hosts::{RemoteHost, RemoteHosts};
use sbxw_sbx::host::{HostCmd, HostExec};
use sbxw_sbx::ssh::{control_path, SshExec};
use sbxw_sbx::{parse_version, MIN_VERSION};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Copy, Default)]
pub struct Opts {
    /// Accept the safe defaults without asking. Never supplies the PAT.
    pub yes: bool,
    pub skip_install: bool,
    pub dry_run: bool,
}

/// Docker's documented install, as two commands so each shows its own result.
/// `REPO_ONLY=1` adds the apt repository without installing Docker Engine.
const INSTALL_REPO: &str = "curl -fsSL https://get.docker.com | sudo REPO_ONLY=1 sh";
const INSTALL_SBX: &str = "sudo apt install -y docker-sbx";

/// One read-only look at the host, as `key=value` lines.
const FACTS_SH: &str = r#"if [ -r /etc/os-release ]; then . /etc/os-release; fi
echo "id=${ID:-unknown}"
echo "version=${VERSION_ID:-unknown}"
echo "arch=$(uname -m)"
echo "user=$(id -un)"
if [ -e /dev/kvm ]; then echo kvm=yes; else echo kvm=no; fi
if grep -qE '(vmx|svm)' /proc/cpuinfo 2>/dev/null; then echo cpuvirt=yes; else echo cpuvirt=no; fi
if id -nG | tr ' ' '\n' | grep -qx kvm; then echo kvmgroup=yes; else echo kvmgroup=no; fi
if getent group kvm | cut -d: -f4 | tr ',' '\n' | grep -qx "$(id -un)"; then echo kvmdb=yes; else echo kvmdb=no; fi"#;

/// The terminal, as far as provisioning needs it. A trait so tests can script
/// the user and record what ran on the host.
pub trait Term {
    fn say(&mut self, text: &str);
    fn confirm(&mut self, question: &str, default: bool) -> Result<bool>;
    fn line(&mut self, question: &str, default: &str) -> Result<String>;
    /// Typed without echo.
    fn secret(&mut self, question: &str) -> Result<String>;
    /// Run a local program with this terminal attached (the `ssh -t` calls);
    /// its exit code, `None` when a signal ended it.
    fn tty(&mut self, program: &str, args: &[String]) -> Result<Option<i32>>;
}

pub struct StdTerm;

fn read_line() -> Result<String> {
    let mut s = String::new();
    if std::io::stdin().read_line(&mut s)? == 0 {
        bail!("input ended (no terminal?)");
    }
    Ok(s.trim_end_matches(['\r', '\n']).to_string())
}

impl Term for StdTerm {
    fn say(&mut self, text: &str) {
        println!("{text}");
    }

    fn confirm(&mut self, question: &str, default: bool) -> Result<bool> {
        let hint = if default { "[Y/n]" } else { "[y/N]" };
        print!("{question} {hint} ");
        std::io::stdout().flush()?;
        Ok(match read_line()?.trim().to_lowercase().as_str() {
            "" => default,
            "y" | "yes" => true,
            _ => false,
        })
    }

    fn line(&mut self, question: &str, default: &str) -> Result<String> {
        if default.is_empty() {
            print!("{question} ");
        } else {
            print!("{question} [{default}] ");
        }
        std::io::stdout().flush()?;
        let a = read_line()?;
        Ok(if a.trim().is_empty() { default.to_string() } else { a.trim().to_string() })
    }

    fn secret(&mut self, question: &str) -> Result<String> {
        if !std::io::stdin().is_terminal() {
            bail!("the token must be typed in a terminal; it is never read from a pipe or an argument");
        }
        print!("{question} ");
        std::io::stdout().flush()?;
        // No new dependency: `stty` turns echo off, and back on whatever happens.
        let off = std::process::Command::new("stty").arg("-echo").status().context("could not run stty")?;
        if !off.success() {
            bail!("could not turn the terminal echo off; not asking for a token");
        }
        let typed = read_line();
        let _ = std::process::Command::new("stty").arg("echo").status();
        println!();
        typed
    }

    fn tty(&mut self, program: &str, args: &[String]) -> Result<Option<i32>> {
        let st = std::process::Command::new(program).args(args).status().with_context(|| format!("could not run {program}"))?;
        Ok(st.code())
    }
}

/// Steps as they are recorded, each printed at once.
struct Run<'a> {
    report: Report,
    t: &'a mut dyn Term,
}

impl Run<'_> {
    fn add(&mut self, title: &str, outcome: Outcome) {
        let step = Step { title: title.to_string(), outcome };
        self.t.say(&step.line());
        self.report.steps.push(step);
    }
    fn pass(&mut self, title: &str, detail: impl Into<String>) {
        self.add(title, Outcome::Pass(detail.into()));
    }
    fn skip(&mut self, title: &str, why: impl Into<String>) {
        self.add(title, Outcome::Skip(why.into()));
    }
    /// Records the failure and says what to do about it.
    fn fail(&mut self, title: &str, why: impl Into<String>, todo: &str) {
        self.add(title, Outcome::Fail(why.into()));
        if !todo.is_empty() {
            self.t.say(&format!("\n{todo}"));
        }
    }
    /// A command that would change the host, shown instead of run.
    fn would(&mut self, how: &str, line: &str) {
        self.t.say(&format!("    would run on the host ({how}): {line}"));
    }
}

fn line_of(program: &str, args: &[String]) -> String {
    HostCmd::new(program, args.to_vec()).argv_line()
}

fn last_line(out: &sbxw_sbx::runner::Output) -> String {
    let both = [String::from_utf8_lossy(&out.stderr).into_owned(), out.stdout_str()].join("\n");
    both.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("no reason given").to_string()
}

fn why(out: &sbxw_sbx::runner::Output) -> String {
    match out.code {
        None => "timed out".into(),
        Some(c) => format!("exit {c}: {}", last_line(out)),
    }
}

/// `24.04` -> (24, 4).
fn parse_ubuntu(v: &str) -> Option<(u32, u32)> {
    let (a, b) = v.trim().split_once('.')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

fn fact<'a>(facts: &'a [(String, String)], key: &str) -> &'a str {
    facts.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()).unwrap_or("")
}

const NO_KVM_HELP: &str = "sbx runs sandboxes in microVMs, which need hardware virtualization (/dev/kvm). \
On a virtual machine that means nested virtualization must be on.\n\
AWS: use an Intel instance type that supports the NestedVirtualization CPU option (C8i, M8i, R8i at least); \
t3a and other AMD or Graviton types cannot.\n\
Elsewhere: enable nested virtualization for the VM in its hypervisor, or use a bare-metal host.";

/// The sbx on the host: `sbx_path` from hosts.toml, else what a login shell finds.
async fn locate(exec: &dyn HostExec, host: &RemoteHost) -> Result<Option<String>> {
    if let Some(p) = &host.sbx_path {
        return Ok(Some(p.clone()));
    }
    let out = exec.exec(HostCmd::new("sh", vec!["-lc".into(), "command -v sbx".into()])).await?;
    Ok(out.stdout_str().lines().map(str::trim).find(|l| !l.is_empty() && out.success()).map(String::from))
}

async fn version_of(exec: &dyn HostExec, sbx: &str) -> Result<Result<(u32, u32, u32), String>> {
    let out = exec.exec(HostCmd { timeout: Some(Duration::from_secs(60)), ..HostCmd::new(sbx, vec!["version".into()]) }).await?;
    if !out.success() {
        return Ok(Err(why(&out)));
    }
    let text = if out.stdout.is_empty() { String::from_utf8_lossy(&out.stderr).into_owned() } else { out.stdout_str() };
    Ok(parse_version(&text).ok_or_else(|| format!("no version in {:?}", text.trim())))
}

fn fmt_v(v: (u32, u32, u32)) -> String {
    format!("{}.{}.{}", v.0, v.1, v.2)
}

/// Run the steps up to (not including) the final `sbxw host check`. Stops at
/// the first `✗`; the report says where.
pub async fn provision(exec: &Arc<dyn HostExec>, host: &RemoteHost, o: &Opts, t: &mut dyn Term) -> Result<Report> {
    let exec: &dyn HostExec = &**exec;
    let mut r = Run { report: Report::default(), t };

    // 1. Checks, before anything is installed.
    let out = exec.exec(HostCmd::new("sh", vec!["-c".into(), FACTS_SH.into()])).await?;
    if !out.success() {
        r.fail("host: facts", why(&out), "Could not read the host's OS and KVM state; fix the ssh connection (`sbxw host test`) and retry.");
        return Ok(r.report);
    }
    let facts: Vec<(String, String)> =
        out.stdout_str().lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
    let mut bad = 0;
    let mut group_only = false;

    let (id, ver) = (fact(&facts, "id"), fact(&facts, "version"));
    const T_OS: &str = "check: Ubuntu 24.04 or newer";
    match (id, parse_ubuntu(ver)) {
        ("ubuntu", Some(v)) if v >= (24, 4) => r.pass(T_OS, format!("Ubuntu {ver}")),
        ("ubuntu", _) => {
            bad += 1;
            r.add(T_OS, Outcome::Fail(format!("Ubuntu {ver} is older than 24.04")));
        }
        _ => {
            bad += 1;
            r.add(T_OS, Outcome::Fail(format!("this is {id} {ver}; `host provision` only knows Ubuntu (install sbx by hand, then run `sbxw host check`)")));
        }
    }
    const T_ARCH: &str = "check: x86-64 or arm64";
    match fact(&facts, "arch") {
        a @ ("x86_64" | "aarch64" | "arm64") => r.pass(T_ARCH, a),
        a => {
            bad += 1;
            r.add(T_ARCH, Outcome::Fail(format!("{a} is not supported by sbx")));
        }
    }
    const T_KVM: &str = "check: /dev/kvm";
    let kvm = fact(&facts, "kvm") == "yes";
    if kvm {
        r.pass(T_KVM, "present");
    } else {
        bad += 1;
        let why = if fact(&facts, "cpuvirt") == "yes" {
            "missing; the CPU has vmx/svm, so the kvm kernel module is probably not loaded"
        } else {
            "missing, and the CPU shows no vmx/svm: no hardware virtualization is exposed to this machine"
        };
        r.add(T_KVM, Outcome::Fail(why.into()));
    }
    const T_GRP: &str = "check: user in the kvm group";
    let user = fact(&facts, "user").to_string();
    if fact(&facts, "kvmgroup") == "yes" {
        r.pass(T_GRP, user.clone());
    } else if fact(&facts, "kvmdb") == "yes" {
        bad += 1;
        r.add(T_GRP, Outcome::Fail(format!("{user} is in the group, but this ssh session predates it")));
    } else {
        bad += 1;
        group_only = bad == 1 && kvm;
        r.add(T_GRP, Outcome::Fail(format!("{user} is not in the kvm group")));
    }

    if bad > 0 {
        if !kvm {
            r.t.say(&format!("\n{NO_KVM_HELP}"));
        }
        if group_only && !user.is_empty() {
            let args = vec!["usermod".to_string(), "-aG".into(), "kvm".into(), user.clone()];
            let (prog, argv) = exec.tty_argv("sudo", &args);
            let line = line_of("sudo", &args);
            let go = if o.dry_run { true } else { o.yes || r.t.confirm(&format!("\nAdd {user} to the kvm group (`{line}`, sudo asks for your password here)?"), true)? };
            if go {
                if o.dry_run {
                    r.would("ssh -t", &line);
                } else {
                    let code = r.t.tty(&prog, &argv)?;
                    if code != Some(0) {
                        r.fail("kvm group: added", format!("`{line}` exited with {code:?}"), "");
                        return Ok(r.report);
                    }
                    r.pass("kvm group: added", user.clone());
                    r.t.say("\nGroup changes only reach new logins. Close every ssh connection to the host (a shared one too: `ssh -O exit <host>`; the sbxw daemon's own link reconnects by itself), then run `sbxw host provision` again.");
                }
                // The checks are not done until the user reconnects.
                if !o.dry_run {
                    r.add("provision: stopped", Outcome::Fail("reconnect, then run it again".into()));
                    return Ok(r.report);
                }
            } else {
                r.t.say(&format!("\nTo do it yourself on the host: sudo usermod -aG kvm {user}, then log in again."));
                r.add("provision: stopped", Outcome::Fail("the user is not in the kvm group".into()));
                return Ok(r.report);
            }
        } else {
            let todo = if fact(&facts, "kvmdb") == "yes" {
                "Close every ssh connection to the host (`ssh -O exit <host>` for a shared one), then run this again.".to_string()
            } else {
                String::from("Nothing was installed. Fix the failed checks above, then run this again.")
            };
            r.add("provision: stopped", Outcome::Fail("a check failed".into()));
            r.t.say(&format!("\n{todo}"));
            return Ok(r.report);
        }
    }

    // 2. Install sbx.
    const T_IN: &str = "install: sbx";
    let mut sbx = locate(exec, host).await?;
    let mut installed_now = false;
    if let Some(p) = &sbx {
        match version_of(exec, p).await? {
            Ok(v) if v >= MIN_VERSION => {
                let (a, b, c) = MIN_VERSION;
                r.skip(T_IN, format!("sbx {} is already installed at {p} (needs {a}.{b}.{c} or newer)", fmt_v(v)));
            }
            Ok(v) => {
                let (a, b, c) = MIN_VERSION;
                r.fail(
                    T_IN,
                    format!("sbx {} at {p} is older than {a}.{b}.{c}", fmt_v(v)),
                    "Upgrade it on the host, for example: sudo apt update && sudo apt install --only-upgrade docker-sbx",
                );
                return Ok(r.report);
            }
            Err(e) => {
                r.fail(T_IN, format!("`{p} version` failed: {e}"), "sbx is there but does not answer; look at it on the host.");
                return Ok(r.report);
            }
        }
    } else if o.skip_install {
        r.fail(T_IN, "sbx is not installed and --skip-install was given", "Install it first, or run this without --skip-install.");
        return Ok(r.report);
    } else {
        r.t.say(&format!(
            "\nsbx is not installed. It will be installed from Docker's apt repository with:\n  {INSTALL_REPO}\n  {INSTALL_SBX}\nsudo asks for the host user's password in this terminal; sbxw never sees it."
        ));
        let go = o.dry_run || o.yes || r.t.confirm("Install sbx on the host now?", true)?;
        if !go {
            r.fail(T_IN, "declined", "Install sbx yourself, then run this again with --skip-install.");
            return Ok(r.report);
        }
        let steps: [(&str, Vec<String>); 2] =
            [("sh", vec!["-c".into(), INSTALL_REPO.into()]), ("sh", vec!["-c".into(), INSTALL_SBX.into()])];
        for (prog, args) in &steps {
            if o.dry_run {
                r.would("ssh -t", &line_of(prog, args));
                continue;
            }
            let (p, argv) = exec.tty_argv(prog, args);
            let code = r.t.tty(&p, &argv)?;
            if code != Some(0) {
                r.fail(T_IN, format!("`{}` exited with {code:?}", args[1]), "Nothing else was changed. Fix the error above and run this again.");
                return Ok(r.report);
            }
        }
        if o.dry_run {
            r.skip(T_IN, "dry run: not installed");
        } else {
            installed_now = true;
            sbx = locate(exec, host).await?.or_else(|| Some("/usr/bin/sbx".into()));
            match version_of(exec, sbx.as_deref().unwrap_or("sbx")).await? {
                Ok(v) if v >= MIN_VERSION => r.pass(T_IN, format!("sbx {}", fmt_v(v))),
                Ok(v) => {
                    r.fail(T_IN, format!("installed sbx {} is older than required", fmt_v(v)), "");
                    return Ok(r.report);
                }
                Err(e) => {
                    r.fail(T_IN, format!("installed, but `sbx version` failed: {e}"), "");
                    return Ok(r.report);
                }
            }
        }
    }
    let sbx = sbx.unwrap_or_else(|| "sbx".into());

    // 3. Sign in. No read-only command tells whether sbx is signed in (an
    // unsigned-in `sbx ls` just lists nothing), so the user is asked.
    const T_LOGIN: &str = "sign in: Docker";
    let login = |user: &str| vec!["login".to_string(), "--username".into(), user.to_string(), "--password-stdin".into()];
    if o.dry_run {
        let l = line_of(&sbx, &login("<docker-username>"));
        r.t.say("\nsbx cannot say whether it is signed in, so a real run asks. If you answer yes it asks for your Docker username and a token (not echoed).");
        r.would("stdin = the token", &l);
        r.skip(T_LOGIN, "dry run: not signed in");
    } else if o.yes {
        r.skip(T_LOGIN, "--yes cannot supply the token; run `sbxw host provision` again without --yes if sbx is not signed in");
    } else {
        let need = r.t.confirm(
            "\nIs sbx on the host NOT yet signed in to Docker, and should it be now?\n(sbx cannot say, so sbxw asks. Answer n if you signed in before.)",
            installed_now,
        )?;
        if !need {
            r.skip(T_LOGIN, "you said it is already signed in");
        } else {
            r.t.say("\nA Personal Access Token (PAT) is a password you create just for this: Docker Hub, Account settings, Personal access tokens, New access token.\nIt is sent to the host on stdin of `sbx login` and sbxw does not keep it.");
            let user = r.t.line("Docker username:", "")?;
            if user.is_empty() {
                r.fail(T_LOGIN, "no username given", "");
                return Ok(r.report);
            }
            let mut pat = r.t.secret("Docker PAT (typed without echo):")?;
            if pat.is_empty() {
                r.fail(T_LOGIN, "no token given", "");
                return Ok(r.report);
            }
            let mut stdin = std::mem::take(&mut pat).into_bytes();
            stdin.push(b'\n');
            let mut args = login(&user);
            args.push("--no-input".into());
            let out = exec.exec(HostCmd { stdin: Some(stdin), timeout: Some(Duration::from_secs(120)), ..HostCmd::new(&sbx, args) }).await?;
            if out.success() {
                r.pass(T_LOGIN, format!("signed in as {user}"));
            } else {
                // sbx's own message only: the token is not echoed by it, and was never in argv.
                r.fail(T_LOGIN, why(&out), "Check the username and that the token is valid and not expired, then run this again.");
                return Ok(r.report);
            }
        }
    }

    // 4. Answer sbx's first-run network policy question up front, so a later
    // `sbx run` over ssh never meets it. `sbx policy init` is the documented
    // non-interactive way; `--no-input` makes it fail rather than ask.
    const T_POL: &str = "network policy: global default";
    let choice = if o.dry_run || o.yes {
        "balanced".to_string()
    } else {
        r.t.say("\nsbx asks once which network policy sandboxes on this machine get:\n  1  Open        all outbound traffic allowed\n  2  Balanced    default deny, plus AI APIs, package managers, code hosts (recommended)\n  3  Locked Down nothing allowed until you add rules");
        match r.t.line("Policy (1, 2 or 3):", "2")?.as_str() {
            "1" => "allow-all".into(),
            "2" => "balanced".into(),
            "3" => "deny-all".into(),
            other => {
                r.fail(T_POL, format!("'{other}' is not 1, 2 or 3"), "");
                return Ok(r.report);
            }
        }
    };
    let pargs = vec!["policy".to_string(), "init".into(), choice.clone(), "--no-input".into()];
    if o.dry_run {
        r.would("ssh", &line_of(&sbx, &pargs));
        r.skip(T_POL, format!("dry run: would set {choice}"));
    } else {
        let out = exec.exec(HostCmd { timeout: Some(Duration::from_secs(120)), ..HostCmd::new(&sbx, pargs.clone()) }).await?;
        let msg = last_line(&out);
        if out.success() {
            r.pass(T_POL, choice);
        } else if msg.to_lowercase().contains("already") {
            r.skip(T_POL, format!("already set on the host ({msg})"));
        } else {
            r.fail(
                T_POL,
                why(&out),
                &format!("Set it by hand on the host: {}\n(`sbx policy reset` clears an earlier choice.)", line_of(&sbx, &pargs[..3])),
            );
            return Ok(r.report);
        }
    }
    Ok(r.report)
}

/// The command: provision, then `sbxw host check` for the verdict.
pub fn run(name: &str, hosts: &RemoteHosts, o: Opts) -> Result<()> {
    let host = hosts.get(name).with_context(|| format!("no host '{name}'"))?;
    let home = std::env::var_os("HOME").map(PathBuf::from).context("HOME is not set")?;
    if !o.dry_run && !std::io::stdin().is_terminal() {
        bail!("`host provision` is interactive (sudo and the token are typed here); run it in a terminal, or add --dry-run to see the plan");
    }
    // The daemon's ControlPath: rides its master when one is up.
    let exec: Arc<dyn HostExec> = Arc::new(SshExec::new(host.clone(), control_path(&home, &host.name)));
    println!("Provisioning host '{name}'{} (EXPERIMENTAL)\n", if o.dry_run { ", dry run: nothing on the host will change" } else { "" });
    let rt = super::runtime();
    let report = rt.block_on(provision(&exec, host, &o, &mut StdTerm))?;
    if !report.ok() {
        bail!("provisioning stopped at a failed step (see above)");
    }
    if o.dry_run {
        println!("\nDry run done. A real run ends with `sbxw host check {name}`.");
        return Ok(());
    }
    println!("\nProvisioned. Now `sbxw host check {name}`:\n");
    super::host_check::run(name, hosts, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sbxw_sbx::host::FakeExec;
    use sbxw_sbx::runner::Output;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const PAT: &str = "dckr_pat_SECRET-0123456789";

    #[derive(Default)]
    struct FakeTerm {
        said: Vec<String>,
        confirms: VecDeque<bool>,
        lines: VecDeque<String>,
        secrets: VecDeque<String>,
        ttys: Vec<String>,
        asked: Vec<String>,
    }

    impl Term for FakeTerm {
        fn say(&mut self, t: &str) {
            self.said.push(t.to_string());
        }
        fn confirm(&mut self, q: &str, d: bool) -> Result<bool> {
            self.asked.push(q.to_string());
            Ok(self.confirms.pop_front().unwrap_or(d))
        }
        fn line(&mut self, q: &str, d: &str) -> Result<String> {
            self.asked.push(q.to_string());
            Ok(self.lines.pop_front().unwrap_or_else(|| d.to_string()))
        }
        fn secret(&mut self, q: &str) -> Result<String> {
            self.asked.push(q.to_string());
            Ok(self.secrets.pop_front().expect("a scripted token"))
        }
        fn tty(&mut self, p: &str, a: &[String]) -> Result<Option<i32>> {
            self.ttys.push(line_of(p, a));
            Ok(Some(0))
        }
    }

    impl FakeTerm {
        fn text(&self) -> String {
            self.said.join("\n")
        }
    }

    const FACTS_OK: &str = "id=ubuntu\nversion=26.04\narch=x86_64\nuser=tom\nkvm=yes\ncpuvirt=yes\nkvmgroup=yes\nkvmdb=yes\n";

    /// A host: `sbx_after` = how many `command -v sbx` calls fail before sbx is "installed".
    fn host_exec(facts: &'static str, sbx_after: usize) -> Arc<FakeExec> {
        let seen = AtomicUsize::new(0);
        FakeExec::new(move |c| {
            let line = c.argv_line();
            Ok(if c.args.first().map(String::as_str) == Some("-c") && c.args[1].contains("/etc/os-release") {
                Output::ok(facts)
            } else if line.contains("command -v sbx") {
                if seen.fetch_add(1, Ordering::SeqCst) >= sbx_after {
                    Output::ok("/usr/bin/sbx\n")
                } else {
                    Output::fail(1, "")
                }
            } else if line.ends_with("version") {
                Output::ok("sbx version v0.51.0\n")
            } else {
                Output::ok("")
            })
        })
    }

    fn host() -> RemoteHost {
        RemoteHost::new("buildbox", Some("buildbox".into()), None, None, None, None, None, false).unwrap()
    }

    fn go(fx: &Arc<FakeExec>, o: Opts, t: &mut FakeTerm) -> Report {
        let exec: Arc<dyn HostExec> = fx.clone();
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(provision(&exec, &host(), &o, t)).unwrap()
    }

    #[test]
    fn dry_run_on_a_fresh_host_prints_every_changing_command_and_runs_none() {
        let fx = host_exec(FACTS_OK, usize::MAX);
        let mut t = FakeTerm::default();
        let rep = go(&fx, Opts { dry_run: true, ..Default::default() }, &mut t);
        assert!(rep.ok(), "{}", t.text());
        let text = t.text();
        assert!(text.contains(&format!("would run on the host (ssh -t): sh -c {INSTALL_REPO}")), "{text}");
        assert!(text.contains(&format!("sh -c {INSTALL_SBX}")), "{text}");
        assert!(text.contains("sbx login --username <docker-username> --password-stdin"), "{text}");
        assert!(text.contains("sbx policy init balanced --no-input"), "{text}");
        // Only read-only probes reached the host; nothing was typed or run in a terminal.
        let log = fx.log();
        assert!(log.iter().all(|l| !l.contains("apt") && !l.contains("login") && !l.contains("policy")), "{log:?}");
        assert!(t.ttys.is_empty() && t.asked.is_empty(), "{:?} {:?}", t.ttys, t.asked);
    }

    #[test]
    fn dry_run_on_a_provisioned_host_skips_the_install() {
        let fx = host_exec(FACTS_OK, 0);
        let mut t = FakeTerm::default();
        let rep = go(&fx, Opts { dry_run: true, ..Default::default() }, &mut t);
        assert!(rep.ok(), "{}", t.text());
        let text = t.text();
        assert!(text.contains("- install: sbx: skipped, sbx 0.51.0 is already installed at /usr/bin/sbx"), "{text}");
        assert!(!text.contains("get.docker.com") && !text.contains("apt install"), "{text}");
        assert!(text.contains("/usr/bin/sbx policy init balanced --no-input"), "{text}");
        assert!(t.ttys.is_empty());
    }

    #[test]
    fn a_failed_check_refuses_before_installing_anything() {
        let fx = host_exec("id=ubuntu\nversion=26.04\narch=x86_64\nuser=tom\nkvm=no\ncpuvirt=no\nkvmgroup=no\nkvmdb=no\n", usize::MAX);
        let mut t = FakeTerm::default();
        let rep = go(&fx, Opts { yes: true, ..Default::default() }, &mut t);
        assert!(!rep.ok());
        let text = t.text();
        assert!(text.contains("NestedVirtualization") && text.contains("t3a"), "{text}");
        assert!(t.ttys.is_empty(), "nothing may run on the host: {:?}", t.ttys);
        assert!(!fx.called("sh -lc"), "not even a look for sbx: {:?}", fx.log());
    }

    #[test]
    fn old_ubuntu_and_other_distros_fail_the_check() {
        for (id, v) in [("ubuntu", "22.04"), ("debian", "12")] {
            let facts: &'static str =
                Box::leak(format!("id={id}\nversion={v}\narch=x86_64\nuser=tom\nkvm=yes\ncpuvirt=yes\nkvmgroup=yes\nkvmdb=yes\n").into_boxed_str());
            let mut t = FakeTerm::default();
            assert!(!go(&host_exec(facts, 0), Opts::default(), &mut t).ok(), "{id} {v}");
        }
    }

    #[test]
    fn a_missing_kvm_group_offers_usermod_then_asks_to_reconnect() {
        let facts = "id=ubuntu\nversion=24.04\narch=aarch64\nuser=tom\nkvm=yes\ncpuvirt=yes\nkvmgroup=no\nkvmdb=no\n";
        let fx = host_exec(facts, 0);
        let mut t = FakeTerm::default();
        let rep = go(&fx, Opts::default(), &mut t);
        assert_eq!(t.ttys, ["sudo usermod -aG kvm tom"]);
        assert!(!rep.ok(), "must stop so the user reconnects");
        assert!(t.text().contains("run `sbxw host provision` again"), "{}", t.text());
    }

    #[test]
    fn a_real_run_installs_signs_in_and_sets_the_policy_without_leaking_the_token() {
        let fx = host_exec(FACTS_OK, 1);
        let mut t = FakeTerm::default();
        t.confirms.extend([true, true]); // install, sign in
        t.lines.extend(["tom-docker".to_string(), "2".to_string()]);
        t.secrets.push_back(PAT.to_string());
        let rep = go(&fx, Opts::default(), &mut t);
        assert!(rep.ok(), "{}", t.text());
        assert_eq!(t.ttys.len(), 2, "{:?}", t.ttys);
        assert!(t.ttys[0].ends_with(INSTALL_REPO) && t.ttys[1].ends_with(INSTALL_SBX), "{:?}", t.ttys);
        // The login: username in argv, the token on stdin and nowhere else.
        let login = fx.cmds.lock().unwrap().iter().find(|c| c.args.first().map(String::as_str) == Some("login")).cloned().expect("a login call");
        assert_eq!(login.args, ["login", "--username", "tom-docker", "--password-stdin", "--no-input"]);
        assert_eq!(login.stdin.as_deref(), Some(format!("{PAT}\n").as_bytes()));
        assert!(fx.called("/usr/bin/sbx policy init balanced"), "{:?}", fx.log());
        for place in fx.log().into_iter().chain(t.said.clone()).chain(t.ttys.clone()).chain(t.asked.clone()) {
            assert!(!place.contains(PAT), "the token leaked into {place:?}");
        }
        assert!(!format!("{:?}", fx.cmds.lock().unwrap()).contains(PAT), "Debug of a command shows the token");
    }

    #[test]
    fn yes_takes_the_defaults_but_never_signs_in() {
        let fx = host_exec(FACTS_OK, 0);
        let mut t = FakeTerm::default();
        let rep = go(&fx, Opts { yes: true, ..Default::default() }, &mut t);
        assert!(rep.ok(), "{}", t.text());
        assert!(t.asked.is_empty(), "--yes must not ask: {:?}", t.asked);
        assert!(!fx.log().iter().any(|l| l.contains("login")), "{:?}", fx.log());
        assert!(t.text().contains("--yes cannot supply the token"), "{}", t.text());
        assert!(fx.called("/usr/bin/sbx policy init balanced --no-input"));
    }

    #[test]
    fn skip_install_fails_when_sbx_is_missing() {
        let fx = host_exec(FACTS_OK, usize::MAX);
        let mut t = FakeTerm::default();
        let rep = go(&fx, Opts { skip_install: true, yes: true, ..Default::default() }, &mut t);
        assert!(!rep.ok());
        assert!(t.ttys.is_empty());
    }

    #[test]
    fn a_policy_that_is_already_set_is_skipped_not_failed() {
        let fx = FakeExec::new(|c| {
            let l = c.argv_line();
            Ok(if c.args.first().map(String::as_str) == Some("-c") {
                Output::ok(FACTS_OK)
            } else if l.contains("command -v sbx") {
                Output::ok("/usr/bin/sbx\n")
            } else if l.ends_with("version") {
                Output::ok("v0.51.0\n")
            } else if l.contains("policy init") {
                Output::fail(1, "the global policy is already initialized")
            } else {
                Output::ok("")
            })
        });
        let mut t = FakeTerm::default();
        t.confirms.push_back(false); // not signing in
        let rep = go(&fx, Opts::default(), &mut t);
        assert!(rep.ok(), "{}", t.text());
        assert!(t.text().contains("already set on the host"), "{}", t.text());
    }
}
