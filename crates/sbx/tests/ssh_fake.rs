//! `SshExec` end to end against `scripts/fake-ssh`: the real argv, the real
//! quoting through a real `sh -c`, stdin, exit codes, and a host that is down.

use sbxw_core::remote_hosts::RemoteHost;
use sbxw_sbx::host::{is_unreachable, HostCmd, HostExec};
use sbxw_sbx::ssh::SshExec;
use sbxw_sbx::Sbx;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// One fake host: a home, a log and a down-switch of its own, behind a
/// wrapper `ssh` that sets them, so tests running in parallel never share
/// process environment.
struct World {
    dir: PathBuf,
    home: PathBuf,
    log: PathBuf,
    down: PathBuf,
    ssh: PathBuf,
}

impl World {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("sbxw-ssh-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&dir);
        let home = dir.join("home");
        std::fs::create_dir_all(home.join("bin")).unwrap();
        let fake = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/fake-ssh").canonicalize().unwrap();
        let (log, down, ssh) = (dir.join("ssh.log"), dir.join("down"), dir.join("ssh"));
        write_exe(
            &ssh,
            &format!(
                "#!/bin/sh\nPATH='{}/bin':\"$PATH\" FAKE_SSH_LOG='{}' FAKE_SSH_HOME='{}' FAKE_SSH_DOWN='{}' exec '{}' \"$@\"\n",
                home.display(),
                log.display(),
                home.display(),
                down.display(),
                fake.display()
            ),
        );
        Self { dir, home, log, down, ssh }
    }

    fn exec(&self, host: RemoteHost) -> Arc<SshExec> {
        Arc::new(SshExec::with_ssh(host, self.dir.join("ctl"), &self.ssh))
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn write_exe(p: &Path, body: &str) {
    std::fs::write(p, body).unwrap();
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn host() -> RemoteHost {
    RemoteHost::new("bb", Some("bb.example".into()), None, None, None, None, None, false).unwrap()
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

/// A remote `sbx`: `ls` prints an empty list, `secret set` keeps its stdin.
fn install_sbx(w: &World) -> String {
    let sbx = w.home.join("bin/sbx");
    write_exe(
        &sbx,
        "#!/bin/sh\ncase \"$1\" in\n  ls) echo '[]' ;;\n  secret) cat > \"$HOME/secret.in\"; echo \"$*\" > \"$HOME/secret.args\" ;;\n  *) echo \"unknown $1\" >&2; exit 2 ;;\nesac\n",
    );
    sbx.display().to_string()
}

#[tokio::test]
async fn sbx_client_runs_on_the_host_and_secrets_stay_on_stdin() {
    let w = World::new();
    let program = install_sbx(&w);
    let x = w.exec(host().with_sbx_path(Some(program.clone())).unwrap());
    let sbx = Sbx::new(Arc::new(x.on_host().await.unwrap()));

    assert!(sbx.ls().await.is_empty());
    let secret = "ghp_s3cr3t-'$(id)'";
    sbx.secret_set("github", secret, None).await.unwrap();

    assert_eq!(std::fs::read_to_string(w.home.join("secret.in")).unwrap(), secret);
    assert_eq!(std::fs::read_to_string(w.home.join("secret.args")).unwrap().trim(), "secret set github");
    let log = w.log();
    assert!(log.contains("-S"), "{log}");
    assert!(log.contains(&format!("'{program}' 'ls' '--format' 'json'")), "{log}");
    assert!(!log.contains("s3cr3t"), "the secret reached ssh's argv: {log}");
}

#[tokio::test]
async fn hostile_words_arrive_whole() {
    let w = World::new();
    let x = w.exec(host());
    let words = ["$(id)", "`id`", "it's", "\"q\"", "a  b", "x\ny", "", "-n", "$HOME", "*"];
    let mut args = s(&["%s\\0"]);
    args.extend(words.iter().map(|x| x.to_string()));
    let out = x.exec(HostCmd::new("printf", args)).await.unwrap();
    assert!(out.success());
    let got: Vec<String> = out.stdout_str().split_terminator('\0').map(String::from).collect();
    assert_eq!(got, words);
}

#[tokio::test]
async fn exit_codes_cwd_and_home() {
    let w = World::new();
    std::fs::create_dir_all(w.home.join("it's a dir")).unwrap();
    let x = w.exec(host());

    let out = x.exec(HostCmd::new("sh", s(&["-c", "echo no >&2; exit 3"]))).await.unwrap();
    assert_eq!(out.code, Some(3));
    assert_eq!(String::from_utf8_lossy(&out.stderr), "no\n");

    let cwd = w.home.join("it's a dir").display().to_string();
    let out = x.exec(HostCmd { cwd: Some(cwd.clone()), ..HostCmd::new("pwd", vec![]) }).await.unwrap();
    assert_eq!(out.stdout_str().trim(), cwd);

    assert_eq!(x.home().await.unwrap(), w.home);
    // Asked once, then cached.
    x.home().await.unwrap();
    assert_eq!(w.log().matches("HOME").count(), 1, "{}", w.log());
}

#[tokio::test]
async fn sbx_is_detected_with_a_login_shell_once() {
    let w = World::new();
    let program = install_sbx(&w);
    // The host's `sh`, first on its PATH: a real login shell would read
    // /etc/profile and ~/.profile, which belong to whatever machine runs the
    // tests. This one answers `-lc` like a login shell that found sbx.
    write_exe(&w.home.join("bin/sh"), "#!/bin/sh\n[ \"$1\" = -lc ] && shift && exec /bin/sh -c \"$@\"\nexec /bin/sh \"$@\"\n");
    let x = w.exec(host());
    assert_eq!(x.sbx_program().await.unwrap(), program);
    assert_eq!(x.sbx_program().await.unwrap(), program);
    let log = w.log();
    assert_eq!(log.matches("'sh' '-lc' 'command -v sbx'").count(), 1, "{log}");

    // A configured sbx_path is used as is: nothing is asked.
    let y = w.exec(host().with_sbx_path(Some("/opt/sbx".into())).unwrap());
    assert_eq!(y.sbx_program().await.unwrap(), "/opt/sbx");
    assert_eq!(w.log(), log);

    // Not found is an error that says what to do, and is not "unreachable".
    let w2 = World::new();
    write_exe(&w2.home.join("bin/sh"), "#!/bin/sh\n[ \"$1\" = -lc ] && exit 1\nexec /bin/sh \"$@\"\n");
    let e = w2.exec(host()).sbx_program().await.unwrap_err();
    assert!(!is_unreachable(&e));
    assert!(format!("{e:#}").contains("sbx_path"), "{e:#}");
}

#[tokio::test]
async fn a_down_host_is_unreachable_and_a_failed_command_is_not() {
    let w = World::new();
    let x = w.exec(host());
    std::fs::write(&w.down, "").unwrap();
    let e = x.exec(HostCmd::new("true", vec![])).await.unwrap_err();
    assert!(is_unreachable(&e), "{e:#}");
    assert!(format!("{e:#}").contains("Connection refused"), "{e:#}");
    assert!(is_unreachable(&x.home().await.unwrap_err()));

    std::fs::remove_file(&w.down).unwrap();
    let out = x.exec(HostCmd::new("false", vec![])).await.unwrap();
    assert_eq!(out.code, Some(1));
}

#[tokio::test]
async fn a_missing_ssh_is_unreachable() {
    let x = SshExec::with_ssh(host(), "/nonexistent/ctl".into(), "/no/such/ssh");
    let e = x.exec(HostCmd::new("true", vec![])).await.unwrap_err();
    assert!(is_unreachable(&e), "{e:#}");
}
