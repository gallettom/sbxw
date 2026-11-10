//! `sbxw update`: the latest release in place of this binary — and, on macOS,
//! a newer build of an installed SbxwIsland.app. Mirrors install.sh.

use anyhow::{Context, Result};
use std::path::PathBuf;

// ── Self-update ──────────────────────────────────────────────────────────
// Mirrors install.sh's download, OS/arch and sudo-fallback logic, so
// `sbxw update` acts like re-running the installer on the latest release,
// without piping curl through a shell script.
const REPO: &str = "gallettom/sbxw";

/// The public key that signs each release's sha256sums.txt, as one line:
/// `ssh-ed25519 AAAA... sbxw-release`. Empty means release signatures are not
/// enabled in this build: only the sha256 checks run. Must equal
/// `RELEASE_PUBLIC_KEY` in install.sh (a test checks it). How to set it:
/// docs/cli.md#release-signatures.
const RELEASE_PUBLIC_KEY: &str = "";
/// The signer identity and the `ssh-keygen -Y` namespace of a release
/// signature, so a signature made with the same key for anything else does
/// not pass for one.
const SIG_NAMESPACE: &str = "sbxw-release";

/// Checks GitHub for a newer sbxw release and, unless `check_only`, downloads
/// and installs it in place of the currently running binary.
pub fn run(check_only: bool, no_island: bool) -> Result<()> {
    let ui = Ui::new();
    let current = env!("CARGO_PKG_VERSION");
    println!("{}\n", crate::banner());

    let latest_tag = ui.spin("checking for updates", latest_release_tag)?;
    let latest = latest_tag.trim_start_matches('v');
    let base = format!("https://github.com/{REPO}/releases/download/{latest_tag}");

    // The island is versioned apart from the CLI, so it can be stale when the
    // binary is not: check it on both paths.
    let island = |check: bool| {
        if no_island {
            return;
        }
        if let Err(e) = update_island(&ui, &base, check) {
            ui.warn(&format!(
                "could not refresh {ISLAND_APP}: {e:#}\ninstall it manually from https://github.com/{REPO}/releases"
            ));
        }
    };

    let exe = std::env::current_exe().context("could not resolve current executable path")?;

    // The island bullet only when there is one to refresh: updating never
    // installs what the user declined.
    let island_planned = !no_island && std::env::consts::OS == "macos" && installed_island_app().is_some();

    if parse_version(latest) <= parse_version(current) {
        ui.ok("sbxw", &format!("v{current} is the latest"));
        if !check_only {
            ui.plan("sbxw update will refresh", &[PlanItem::extension(), PlanItem::island(island_planned)]);
            refresh_extension(&ui, &exe);
        }
        island(check_only);
        println!();
        return Ok(());
    }

    ui.ok("new release", &format!("{latest_tag}  (you have v{current})"));
    ui.plan(
        if check_only { "sbxw update would install" } else { "sbxw update will install" },
        &[
            Some(PlanItem {
                colour: BLUE,
                title: format!("sbxw {latest_tag}"),
                place: exe.display().to_string(),
                why: "CLI and daemon, sha256-checked".into(),
            }),
            PlanItem::extension(),
            PlanItem::island(island_planned),
        ],
    );
    if check_only {
        island(true);
        println!("\n  run {} to install it\n", ui.cmd("sbxw update"));
        return Ok(());
    }

    let (os, arch) = target_os_arch()?;
    let artifact = format!("sbxw-{os}-{arch}");

    // Removed when it goes out of scope, on every exit path.
    let tmp = PrivateDir::new("sbxw-update")?;
    let tmp_dir = tmp.path();
    let tmp_bin = tmp_dir.join(&artifact);

    ui.spin(&format!("downloading {artifact}"), || {
        curl_download(&format!("{base}/{artifact}"), &tmp_bin)
            .with_context(|| format!("failed to download {artifact} for {latest_tag}"))
    })?;
    ui.ok("downloaded", &artifact);

    // Fail closed: no sums file, or no line for the artifact, aborts the
    // update (release.yml publishes one line per binary and the island zip).
    let signed = fetch_and_verify(&base, tmp_dir, &tmp_bin, &artifact)
        .context("verification failed — aborting update, nothing was installed")
        .inspect_err(|_| ui.fail("verify"))?;
    ui.ok("signature", signed.note());
    ui.ok("checksum", "sha256 verified");

    set_executable(&tmp_bin)?;

    install_binary(&ui, &tmp_bin, &exe)?;
    drop(tmp);
    ui.ok("installed", &exe.display().to_string());

    refresh_extension(&ui, &exe);
    island(false);

    println!(
        "\n  {}  sbxw {}v{current}{} → {}{latest_tag}{}\n",
        ui.paint(GREEN, "✔"),
        ui.code(DIM),
        ui.code(RESET),
        ui.code(GREEN),
        ui.code(RESET),
    );
    if daemon_running() {
        println!("  the daemon is still running the old build — restart it:");
        println!("    {}\n", ui.cmd("sbxw down && sbxw up"));
    }
    Ok(())
}

/// Whether a daemon answers on this directory's configured address — the
/// only case in which the old build is still running somewhere.
fn daemon_running() -> bool {
    let cfg = crate::cmd::load_config(std::path::Path::new("sbxw.toml")).unwrap_or_default();
    crate::client::Daemon::new(&cfg.web_addr).health().is_some()
}

/// Rewrites the browser extension folder from the binary at `exe`: after an
/// update, that is the new binary, which this process does not carry.
fn refresh_extension(ui: &Ui, exe: &std::path::Path) {
    let ok = std::process::Command::new(exe)
        .args(["extension", "--refresh"])
        .output()
        .is_ok_and(|o| o.status.success());
    if ok {
        ui.ok("browser extension", "refreshed");
    } else {
        ui.warn("could not refresh the browser extension — run `sbxw extension`");
    }
}

/// One row of the plan printed before anything is written: a coloured
/// chevron, what gets installed, where, and why.
struct PlanItem {
    colour: &'static str,
    title: String,
    place: String,
    why: String,
}

impl PlanItem {
    fn extension() -> Option<Self> {
        Some(Self {
            colour: ORANGE,
            title: "browser extension".into(),
            place: "~/.sbxw/browser-extension/".into(),
            why: "side panel".into(),
        })
    }

    fn island(installed: bool) -> Option<Self> {
        installed.then(|| Self {
            colour: PINK,
            title: "sbxw Island".into(),
            place: "~/Applications".into(),
            why: "menu-bar app, if a newer one ships".into(),
        })
    }
}

// ── Terminal output ──────────────────────────────────────────────────────
// Same look as a bring-up (render.rs): one ✓ line per step, a spinner on the
// running step, colours from the banner. Plain text when stdout is not a
// terminal or NO_COLOR is set.
const BLUE: &str = "\x1b[1;38;5;75m";
const GREEN: &str = "\x1b[1;38;5;77m";
const YELLOW: &str = "\x1b[38;5;221m";
const RED: &str = "\x1b[1;38;5;203m";
const ORANGE: &str = "\x1b[1;38;5;215m";
const PINK: &str = "\x1b[1;38;5;211m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";
/// Width of the label column, so the notes line up.
const LABEL: usize = 18;

struct Ui {
    tty: bool,
}

impl Ui {
    fn new() -> Self {
        use std::io::IsTerminal;
        Self { tty: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() }
    }

    fn code(&self, c: &'static str) -> &'static str {
        if self.tty {
            c
        } else {
            ""
        }
    }

    fn paint(&self, c: &'static str, s: &str) -> String {
        format!("{}{s}{}", self.code(c), self.code(RESET))
    }

    fn cmd(&self, s: &str) -> String {
        self.paint(BLUE, s)
    }

    fn ok(&self, label: &str, note: &str) {
        println!("  {} {label:<LABEL$} {}", self.paint(GREEN, "✓"), self.paint(DIM, note));
    }

    /// The plan: what is about to be installed, before any of it is.
    fn plan(&self, heading: &str, items: &[Option<PlanItem>]) {
        // Padded before painting, so the escape codes don't skew the columns.
        println!("\n  {}\n", self.paint(BOLD, heading));
        for item in items.iter().flatten() {
            println!(
                "  {} {} {:<28} {}",
                self.paint(item.colour, "›"),
                self.paint(BOLD, &format!("{:<19}", item.title)),
                item.place,
                self.paint(DIM, &item.why)
            );
        }
        println!();
    }

    fn fail(&self, label: &str) {
        println!("  {} {label}", self.paint(RED, "✕"));
    }

    fn warn(&self, text: &str) {
        println!("  {} {}", self.paint(YELLOW, "!"), self.paint(YELLOW, &text.replace('\n', "\n    ")));
    }

    /// Runs `work` with a spinner on `label…`, erased when it ends (the caller
    /// follows with its own ✓); a ✕ replaces it on failure.
    fn spin<T>(&self, label: &str, work: impl FnOnce() -> Result<T>) -> Result<T> {
        use std::io::Write;
        use std::sync::atomic::{AtomicBool, Ordering};
        if !self.tty {
            return work().inspect_err(|_| self.fail(label));
        }
        let done = AtomicBool::new(false);
        let result = std::thread::scope(|s| {
            s.spawn(|| {
                const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
                let mut out = std::io::stdout();
                for f in FRAMES.iter().cycle() {
                    if done.load(Ordering::Relaxed) {
                        break;
                    }
                    let _ = write!(out, "\r\x1b[2K  {BLUE}{f}{RESET} {label}…");
                    let _ = out.flush();
                    std::thread::sleep(std::time::Duration::from_millis(80));
                }
                let _ = write!(out, "\r\x1b[2K");
                let _ = out.flush();
            });
            let r = work();
            done.store(true, Ordering::Relaxed);
            r
        });
        result.inspect_err(|_| self.fail(label))
    }
}

// ── macOS companion app (sbxw Island) ────────────────────────────────────
// The install.sh bundle lives outside the binary: `sbxw update` refreshes it in
// place, only if already installed (it never installs what the user declined).
const ISLAND_APP: &str = "SbxwIsland.app";
/// Bundle executable name, i.e. what the running process is called.
const ISLAND_PROC: &str = "SbxwIsland";

/// Refreshes an installed `SbxwIsland.app` when the release ships a newer build.
/// macOS only; a no-op when the app is not installed. With `check_only` it only
/// reports.
///
/// The app's version is independent of the release tag, so the release
/// publishes `island-version.txt` beside the zip (see macos/build-app.sh): that
/// tells us the bundle is stale without downloading it.
fn update_island(ui: &Ui, base: &str, check_only: bool) -> Result<()> {
    if std::env::consts::OS != "macos" {
        return Ok(());
    }
    let Some(app) = installed_island_app() else {
        return Ok(()); // not installed — nothing to refresh
    };

    // Removed when it goes out of scope, on every exit path.
    let tmp = PrivateDir::new("sbxw-island")?;
    let tmp_dir = tmp.path();

    let tmp_ver = tmp_dir.join("island-version.txt");
    if curl_download(&format!("{base}/island-version.txt"), &tmp_ver).is_err() {
        // Releases before the island got its own version don't publish it.
        // Leaving the app alone beats bouncing it on every update.
        ui.ok("island", "no version marker in this release — left as is");
        return Ok(());
    }
    let latest = std::fs::read_to_string(&tmp_ver)?.trim().to_string();
    let installed = island_bundle_version(&app);

    if parse_version(&latest) <= parse_version(&installed) {
        ui.ok("island", &format!("v{installed} is up to date"));
        return Ok(());
    }
    if check_only {
        ui.ok(
            "island",
            &format!("v{latest} available  (you have v{installed})"),
        );
        return Ok(());
    }

    let artifact = "SbxwIsland-macos.zip";
    let tmp_zip = tmp_dir.join(artifact);
    ui.spin(&format!("downloading {artifact}"), || {
        curl_download(&format!("{base}/{artifact}"), &tmp_zip)
            .with_context(|| format!("failed to download {artifact}"))
    })?;

    fetch_and_verify(base, tmp_dir, &tmp_zip, artifact)
        .context("verification failed — leaving the installed app alone")?;

    // `ditto -x -k` is what install.sh uses; it restores the bundle layout.
    let status = std::process::Command::new(sbxw_core::sysbin::sys_bin("ditto"))
        .args(["-x", "-k"])
        .arg(&tmp_zip)
        .arg(tmp_dir)
        .status()
        .context("failed to run ditto")?;
    let staged = tmp_dir.join(ISLAND_APP);
    if !status.success() || !staged.is_dir() {
        anyhow::bail!("downloaded archive did not contain {ISLAND_APP}");
    }

    // Replacing the bundle under a running app leaves the old code running, so
    // quit it first — and put it back afterwards only if it *was* running.
    let was_running = island_running();
    if was_running {
        ui.spin("quitting SbxwIsland", || {
            quit_island();
            Ok(())
        })?;
    }

    // Past the quit, a failure must not leave the user without their island:
    // whatever bundle survives at `app` gets relaunched before we bail.
    if let Err(e) = swap_bundle(&staged, &app) {
        if was_running && app.is_dir() {
            let _ = open_island(&app);
        }
        return Err(e);
    }
    // Ad-hoc-signed, not notarised: without this Gatekeeper refuses to open it.
    let _ = std::process::Command::new(sbxw_core::sysbin::sys_bin("xattr"))
        .args(["-dr", "com.apple.quarantine"])
        .arg(&app)
        .status();

    let relaunched = was_running && open_island(&app).is_ok_and(|s| s.success());
    ui.ok(
        "island",
        &format!(
            "v{installed} → v{latest}{}",
            if relaunched { "  (relaunched)" } else { "" }
        ),
    );
    Ok(())
}

/// Replaces the bundle at `app` with the freshly extracted one. `mv` rather
/// than `std::fs::rename` for the cross-filesystem case (/tmp → ~/Applications).
fn swap_bundle(staged: &std::path::Path, app: &std::path::Path) -> Result<()> {
    std::fs::remove_dir_all(app)
        .with_context(|| format!("could not remove {} (permissions?)", app.display()))?;
    let status = std::process::Command::new(sbxw_core::sysbin::sys_bin("mv"))
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
    std::process::Command::new(sbxw_core::sysbin::sys_bin("open")).arg(app).status()
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
    let out = std::process::Command::new(sbxw_core::sysbin::sys_bin("defaults"))
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
    std::process::Command::new(sbxw_core::sysbin::sys_bin("pgrep"))
        .args(["-x", ISLAND_PROC])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Asks the app to quit (AppleScript, so it shuts down cleanly), then waits for
/// the process to go away, escalating to a signal if it doesn't.
fn quit_island() {
    let _ = std::process::Command::new(sbxw_core::sysbin::sys_bin("osascript"))
        .args(["-e", &format!("quit app \"{ISLAND_PROC}\"")])
        .status();
    if island_gone_within(30) {
        return;
    }
    let _ = std::process::Command::new(sbxw_core::sysbin::sys_bin("pkill"))
        .args(["-x", ISLAND_PROC])
        .status();
    island_gone_within(20);
}

/// Poll every 100 ms, up to `tries` times, for the island process to exit.
fn island_gone_within(tries: u32) -> bool {
    for _ in 0..tries {
        if !island_running() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    false
}

/// Fetches the latest release's tag name (e.g. "v1.0.8") from the GitHub API.
fn latest_release_tag() -> Result<String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let out = std::process::Command::new(sbxw_core::sysbin::sys_bin("curl"))
        .args(["--proto", "=https", "--proto-redir", "=https", "--tlsv1.2", "-fsSL", &url])
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
        .map(|p| {
            p.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .unwrap_or(0)
        })
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
    let status = std::process::Command::new(sbxw_core::sysbin::sys_bin("curl"))
        .args(["--proto", "=https", "--proto-redir", "=https", "--tlsv1.2", "-fsSL", url, "-o"])
        .arg(dest)
        .status()
        .context("failed to run curl — is it installed?")?;
    if !status.success() {
        anyhow::bail!("curl exited with {status}");
    }
    Ok(())
}

/// Whether the sums a download was checked against were signed.
#[derive(Debug, PartialEq)]
enum Signature {
    /// `RELEASE_PUBLIC_KEY` is empty: checksums only, as before signing.
    NotEnabled,
    Verified,
}

impl Signature {
    fn note(&self) -> &'static str {
        match self {
            Self::NotEnabled => "release signatures are not enabled in this build",
            Self::Verified => "sha256sums.txt signed by the release key",
        }
    }
}

/// Downloads the release's sha256sums.txt into `dir`, checks its signature
/// when this build carries a release key, then verifies `file` against its
/// line for `artifact_name`. Every failure is an error: a sums file or a
/// signature that cannot be fetched is not a reason to install unchecked.
fn fetch_and_verify(
    base: &str,
    dir: &std::path::Path,
    file: &std::path::Path,
    artifact_name: &str,
) -> Result<Signature> {
    let sums_path = dir.join("sha256sums.txt");
    curl_download(&format!("{base}/sha256sums.txt"), &sums_path)
        .context("could not download sha256sums.txt from the release")?;
    // The signature first: until it checks out, the sums prove nothing.
    let signed = if RELEASE_PUBLIC_KEY.is_empty() {
        Signature::NotEnabled
    } else {
        let sig_path = dir.join("sha256sums.txt.sig");
        curl_download(&format!("{base}/sha256sums.txt.sig"), &sig_path)
            .context("the release has no sha256sums.txt.sig: refusing an unsigned release")?;
        verify_sums_signature(RELEASE_PUBLIC_KEY, dir, &sums_path, &sig_path)?;
        Signature::Verified
    };
    let sums = std::fs::read_to_string(&sums_path).context("could not read sha256sums.txt")?;
    verify_checksum(file, artifact_name, &sums)?;
    Ok(signed)
}

/// The allowed-signers line `ssh-keygen -Y verify` reads for `public_key`
/// (`<type> <base64> [comment]`): `sbxw-release <type> <base64>`.
fn allowed_signers_line(public_key: &str) -> Result<String> {
    let mut parts = public_key.split_whitespace();
    match (parts.next(), parts.next()) {
        (Some(kind), Some(key)) if kind.starts_with("ssh-") || kind.starts_with("ecdsa-") || kind.starts_with("sk-") => {
            Ok(format!("{SIG_NAMESPACE} {kind} {key}\n"))
        }
        _ => anyhow::bail!("RELEASE_PUBLIC_KEY in this build is not a public key line"),
    }
}

/// Checks that `sig_path` is an OpenSSH signature of `sums_path` by
/// `public_key` in the `sbxw-release` namespace. The allowed-signers file is
/// written in `dir`, which must be private (a [`PrivateDir`]). `ssh-keygen`
/// comes from the system's own directories, never `PATH`.
fn verify_sums_signature(
    public_key: &str,
    dir: &std::path::Path,
    sums_path: &std::path::Path,
    sig_path: &std::path::Path,
) -> Result<()> {
    let allowed = dir.join("allowed_signers");
    std::fs::write(&allowed, allowed_signers_line(public_key)?).context("could not write allowed_signers")?;
    if !sig_path.is_file() {
        anyhow::bail!("sha256sums.txt.sig is missing: the release is not signed");
    }
    let keygen = sbxw_core::sysbin::sys_bin("ssh-keygen");
    if !keygen.is_file() {
        anyhow::bail!("ssh-keygen is not installed (looked for {}): cannot check the release signature", keygen.display());
    }
    let sums = std::fs::File::open(sums_path).context("could not read sha256sums.txt")?;
    let out = std::process::Command::new(&keygen)
        .args(["-Y", "verify", "-f"])
        .arg(&allowed)
        .args(["-I", SIG_NAMESPACE, "-n", SIG_NAMESPACE, "-s"])
        .arg(sig_path)
        .stdin(sums)
        .output()
        .context("failed to run ssh-keygen")?;
    if !out.status.success() {
        anyhow::bail!(
            "the signature on sha256sums.txt did not verify against this build's release key: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// The expected hash for `artifact_name` in a sha256sums.txt (lines look like
/// "<hash>  <filename>", or "<hash> *<filename>" in binary mode).
fn expected_checksum(sums: &str, artifact_name: &str) -> Result<String> {
    sums.lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            let hash = parts.next()?;
            let name = parts.next()?.trim_start_matches('*');
            (name == artifact_name).then(|| hash.to_string())
        })
        .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .with_context(|| format!("sha256sums.txt has no valid line for {artifact_name}"))
}

/// Verifies `bin_path`'s sha256 against the entry for `artifact_name` in the
/// text of a sha256sums.txt.
fn verify_checksum(bin_path: &std::path::Path, artifact_name: &str, sums: &str) -> Result<()> {
    let expected = expected_checksum(sums, artifact_name)?;
    let actual = sha256_hex(bin_path)?;
    if !actual.eq_ignore_ascii_case(&expected) {
        anyhow::bail!("checksum mismatch: expected {expected}, got {actual}");
    }
    Ok(())
}

/// Computes a file's sha256 by shelling out to `sha256sum` (Linux) or
/// `shasum -a 256` (macOS), avoiding a crypto crate dependency for one command.
fn sha256_hex(path: &std::path::Path) -> Result<String> {
    let out = if std::process::Command::new(sbxw_core::sysbin::sys_bin("sha256sum"))
        .arg("--version")
        .output()
        .is_ok()
    {
        std::process::Command::new(sbxw_core::sysbin::sys_bin("sha256sum")).arg(path).output()
    } else {
        std::process::Command::new(sbxw_core::sysbin::sys_bin("shasum"))
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

/// A temp directory only this user can enter: a random name created with
/// `create_dir` (which fails if something already sits there, so nobody can
/// pre-create it), mode 0700 on unix, removed with everything in it on drop.
/// A predictable `sbxw-update-<pid>` let another local user plant the
/// directory and swap the download between the checksum and the move.
struct PrivateDir(PathBuf);

impl PrivateDir {
    fn new(prefix: &str) -> Result<Self> {
        Self::new_in(&std::env::temp_dir(), prefix)
    }

    fn new_in(parent: &std::path::Path, prefix: &str) -> Result<Self> {
        let mut last_err = None;
        for _ in 0..16 {
            let dir = parent.join(format!("{prefix}-{:016x}", random_u64()));
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&dir) {
                Ok(()) => return Ok(Self(dir)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last_err = Some(e),
                Err(e) => {
                    return Err(e).with_context(|| format!("could not create {}", dir.display()))
                }
            }
        }
        Err(last_err.expect("loop ran"))
            .with_context(|| format!("could not create a private directory in {}", parent.display()))
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for PrivateDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 64 unpredictable bits from std alone: `RandomState` is seeded from the
/// OS's random source, mixed with the time and pid. Unpredictability is a
/// bonus here; `create_dir`'s exclusivity is what makes the directory safe.
fn random_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u32(std::process::id());
    if let Ok(t) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        h.write_u128(t.as_nanos());
    }
    h.finish()
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
fn install_binary(ui: &Ui, src: &std::path::Path, dest: &std::path::Path) -> Result<()> {
    // Output captured: its "Permission denied" is the expected case, said better below.
    let out = std::process::Command::new(sbxw_core::sysbin::sys_bin("mv"))
        .arg(src)
        .arg(dest)
        .output()
        .context("failed to run mv")?;
    if out.status.success() {
        return Ok(());
    }

    let dir = dest.parent().unwrap_or(dest);
    println!(
        "  {} {} is not writable — sudo needed",
        ui.paint(YELLOW, "⚿"),
        dir.display()
    );
    let status = std::process::Command::new(sbxw_core::sysbin::sys_bin("sudo"))
        .args(["-p", "    password for %u: ", "mv"])
        .arg(src)
        .arg(dest)
        .status()
        .context("failed to run sudo mv")?;
    if !status.success() {
        anyhow::bail!("`mv` failed both with and without sudo (exit {status})");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn expected_checksum_finds_the_artifact_line() {
        let sums = format!("{}  sbxw-linux-arm64\n{HASH}  sbxw-linux-x86_64\n", "0".repeat(64));
        assert_eq!(expected_checksum(&sums, "sbxw-linux-x86_64").unwrap(), HASH);
        // Binary-mode marker.
        let sums = format!("{HASH} *SbxwIsland-macos.zip\n");
        assert_eq!(expected_checksum(&sums, "SbxwIsland-macos.zip").unwrap(), HASH);
    }

    #[test]
    fn expected_checksum_fails_closed_without_a_line() {
        let sums = format!("{HASH}  sbxw-linux-x86_64\n");
        let err = expected_checksum(&sums, "sbxw-macos-arm64").unwrap_err();
        assert!(err.to_string().contains("sbxw-macos-arm64"), "{err}");
        assert!(expected_checksum("", "sbxw-macos-arm64").is_err());
        // A prefix of the name is not the name.
        assert!(expected_checksum(&format!("{HASH}  sbxw-macos-arm64.old\n"), "sbxw-macos-arm64").is_err());
    }

    #[test]
    fn expected_checksum_rejects_a_malformed_hash() {
        assert!(expected_checksum("deadbeef  sbxw-linux-x86_64\n", "sbxw-linux-x86_64").is_err());
    }

    #[test]
    fn verify_checksum_compares_the_file() {
        let tmp = PrivateDir::new("sbxw-update-test").unwrap();
        let file = tmp.path().join("empty");
        std::fs::write(&file, b"").unwrap();
        // HASH is the sha256 of the empty string.
        assert!(verify_checksum(&file, "a", &format!("{HASH}  a\n")).is_ok());
        let wrong = "1".repeat(64);
        assert!(verify_checksum(&file, "a", &format!("{wrong}  a\n")).is_err());
    }

    #[test]
    fn private_dir_is_fresh_private_and_removed_on_drop() {
        let parent = PrivateDir::new("sbxw-update-test").unwrap();
        let a = PrivateDir::new_in(parent.path(), "x").unwrap();
        let b = PrivateDir::new_in(parent.path(), "x").unwrap();
        assert_ne!(a.path(), b.path(), "names must not repeat");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(a.path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        std::fs::write(a.path().join("f"), b"x").unwrap();
        let path = a.path().to_path_buf();
        drop(a);
        assert!(!path.exists());
    }

    // ── Release signatures ───────────────────────────────────────────────

    /// The value assigned to `RELEASE_PUBLIC_KEY=` in install.sh.
    fn install_sh_public_key(script: &str) -> String {
        let line = script
            .lines()
            .find(|l| l.starts_with("RELEASE_PUBLIC_KEY="))
            .expect("install.sh assigns RELEASE_PUBLIC_KEY at the start of a line");
        let v = line["RELEASE_PUBLIC_KEY=".len()..].trim();
        v.trim_matches(|c| c == '"' || c == '\'').to_string()
    }

    #[test]
    fn the_installer_and_the_updater_carry_the_same_release_key() {
        let script = include_str!("../../../install.sh");
        assert_eq!(install_sh_public_key(script), RELEASE_PUBLIC_KEY, "set the same key line in both: docs/cli.md#release-signatures");
        assert_eq!(install_sh_public_key("RELEASE_PUBLIC_KEY=\"ssh-ed25519 AAAA c\"\n"), "ssh-ed25519 AAAA c");
    }

    #[test]
    fn the_allowed_signers_line_drops_the_comment() {
        assert_eq!(
            allowed_signers_line("ssh-ed25519 AAAAC3Nz sbxw-release").unwrap(),
            "sbxw-release ssh-ed25519 AAAAC3Nz\n"
        );
        assert!(allowed_signers_line("").is_err());
        assert!(allowed_signers_line("AAAAC3Nz").is_err());
        assert!(allowed_signers_line("garbage AAAAC3Nz").is_err());
    }

    /// A throwaway signer: two Ed25519 keys (`release`, `other`), a sums
    /// file, and its signature by `release`.
    struct Signed {
        dir: PrivateDir,
        key: String,
        sums: PathBuf,
        sig: PathBuf,
    }

    fn keygen(args: &[&str], dir: &std::path::Path) -> std::process::Output {
        std::process::Command::new(sbxw_core::sysbin::sys_bin("ssh-keygen"))
            .args(args)
            .current_dir(dir)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    }

    /// `None` (and a note on stderr) where ssh-keygen is missing.
    fn signed_fixture() -> Option<Signed> {
        if !sbxw_core::sysbin::sys_bin("ssh-keygen").is_file() {
            eprintln!("skipped: no ssh-keygen in the system directories");
            return None;
        }
        let dir = PrivateDir::new("sbxw-sig-test").unwrap();
        let d = dir.path();
        for name in ["release", "other"] {
            let o = keygen(&["-q", "-t", "ed25519", "-N", "", "-C", "sbxw-release", "-f", name], d);
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        }
        let sums = d.join("sha256sums.txt");
        std::fs::write(&sums, format!("{HASH}  sbxw-linux-x86_64\n")).unwrap();
        let o = keygen(&["-Y", "sign", "-f", "release", "-n", SIG_NAMESPACE, "sha256sums.txt"], d);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let key = std::fs::read_to_string(d.join("release.pub")).unwrap().trim().to_string();
        let sig = d.join("sha256sums.txt.sig");
        Some(Signed { dir, key, sums, sig })
    }

    /// The same four cases, through `check(key, dir, sums, sig) -> passed`.
    fn signature_cases(check: impl Fn(&str, &std::path::Path, &std::path::Path, &std::path::Path) -> bool) {
        let Some(f) = signed_fixture() else { return };
        let d = f.dir.path();
        let work = PrivateDir::new_in(d, "work").unwrap();
        assert!(check(&f.key, work.path(), &f.sums, &f.sig), "a good signature passes");

        // A sums file changed after signing.
        let tampered = d.join("tampered.txt");
        std::fs::write(&tampered, format!("{}  sbxw-linux-x86_64\n", "0".repeat(64))).unwrap();
        assert!(!check(&f.key, work.path(), &tampered, &f.sig), "a modified sums file fails");

        // Signed by a key other than the one the build carries.
        // Same bytes, so only the key differs.
        std::fs::copy(&f.sums, d.join("copy.txt")).unwrap();
        let o = keygen(&["-Y", "sign", "-f", "other", "-n", SIG_NAMESPACE, "copy.txt"], d);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        assert!(!check(&f.key, work.path(), &f.sums, &d.join("copy.txt.sig")), "another key's signature fails");

        // No signature at all.
        assert!(!check(&f.key, work.path(), &f.sums, &d.join("missing.sig")), "a missing signature fails");
    }

    #[test]
    fn the_updater_checks_the_release_signature() {
        signature_cases(|key, dir, sums, sig| verify_sums_signature(key, dir, sums, sig).is_ok());
    }

    #[test]
    fn the_installer_checks_the_release_signature() {
        // verify_sums_sig, cut out of install.sh and run on its own by sh.
        let script = include_str!("../../../install.sh");
        let start = script.find("verify_sums_sig() {").expect("install.sh defines verify_sums_sig");
        let end = script[start..].find("\n}\n").expect("verify_sums_sig ends") + start + 3;
        let func = &script[start..end];
        signature_cases(|key, dir, sums, sig| {
            let body = format!(
                "set -eu\nerr() {{ printf '%s\\n' \"$*\" >&2; }}\nRELEASE_PUBLIC_KEY='{key}'\n{func}\nverify_sums_sig \"$1\" \"$2\" \"$3\"\n"
            );
            std::process::Command::new(sbxw_core::sysbin::sys_bin("sh"))
                .args(["-c", &body, "sh"])
                .arg(sums)
                .arg(sig)
                .arg(dir)
                .output()
                .unwrap()
                .status
                .success()
        });
    }
}
