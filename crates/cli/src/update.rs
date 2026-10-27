//! `sbxw update`: the latest release in place of this binary — and, on macOS,
//! a newer build of an installed SbxwIsland.app. Mirrors install.sh.

use anyhow::{Context, Result};
use std::path::PathBuf;

// ── Self-update ──────────────────────────────────────────────────────────
// Mirrors install.sh's own download/OS-arch/sudo-fallback logic so
// `sbxw update` behaves exactly like re-running the installer against the
// latest release, without requiring curl to be piped through a shell script.
const REPO: &str = "gallettom/sbxw";

/// Checks GitHub for a newer sbxw release and, unless `check_only`, downloads
/// and installs it in place of the currently running binary.
pub fn run(check_only: bool, no_island: bool) -> Result<()> {
    let ui = Ui::new();
    let current = env!("CARGO_PKG_VERSION");
    println!("{}\n", crate::banner());

    let latest_tag = ui.spin("checking for updates", latest_release_tag)?;
    let latest = latest_tag.trim_start_matches('v');
    let base = format!("https://github.com/{REPO}/releases/download/{latest_tag}");

    // The island versions apart from the CLI, so it can be stale even when the
    // binary isn't — check it on both paths.
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

    let tmp_dir = std::env::temp_dir().join(format!("sbxw-update-{}", std::process::id()));
    std::fs::create_dir_all(&tmp_dir)?;
    let tmp_bin = tmp_dir.join(&artifact);

    ui.spin(&format!("downloading {artifact}"), || {
        curl_download(&format!("{base}/{artifact}"), &tmp_bin)
            .with_context(|| format!("failed to download {artifact} for {latest_tag}"))
    })?;
    ui.ok("downloaded", &artifact);

    // Best-effort checksum verification against the release's sha256sums.txt
    // (published alongside every release by .github/workflows/release.yml).
    let tmp_sums = tmp_dir.join("sha256sums.txt");
    if curl_download(&format!("{base}/sha256sums.txt"), &tmp_sums).is_ok() {
        verify_checksum(&tmp_bin, &artifact, &tmp_sums)
            .context("checksum verification failed — aborting update")
            .inspect_err(|_| ui.fail("checksum"))?;
        ui.ok("checksum", "sha256 verified");
    } else {
        ui.warn("could not fetch sha256sums.txt — skipping checksum verification");
    }

    set_executable(&tmp_bin)?;

    install_binary(&ui, &tmp_bin, &exe)?;
    let _ = std::fs::remove_dir_all(&tmp_dir);
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

/// Rewrites the browser extension's folder from the binary at `exe` — after an
/// update that is the new one, whose extension this process does not carry.
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
// The same look as a bring-up (render.rs): one ✓ line per step, a spinner on
// the step that is running, colours from the banner. Plain text when stdout
// isn't a terminal or NO_COLOR is set.
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

    /// Runs `work` with a spinner on `label…` — erased when it ends, so the
    /// caller follows with its own ✓ line; a ✕ replaces it on failure.
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
// The bundle installed by install.sh lives outside the binary, so `sbxw update`
// refreshes it in place — but only if the user already has it: updating must
// never install something they declined.
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
fn update_island(ui: &Ui, base: &str, check_only: bool) -> Result<()> {
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
        ui.ok("island", "no version marker in this release — left as is");
        return Ok(());
    }
    let latest = std::fs::read_to_string(&tmp_ver)?.trim().to_string();
    let installed = island_bundle_version(&app);

    if parse_version(&latest) <= parse_version(&installed) {
        cleanup(&tmp_dir);
        ui.ok("island", &format!("v{installed} is up to date"));
        return Ok(());
    }
    if check_only {
        ui.ok(
            "island",
            &format!("v{latest} available  (you have v{installed})"),
        );
        cleanup(&tmp_dir);
        return Ok(());
    }

    let artifact = "SbxwIsland-macos.zip";
    let tmp_zip = tmp_dir.join(artifact);
    ui.spin(&format!("downloading {artifact}"), || {
        curl_download(&format!("{base}/{artifact}"), &tmp_zip)
            .with_context(|| format!("failed to download {artifact}"))
    })?;

    let tmp_sums = tmp_dir.join("sha256sums.txt");
    if curl_download(&format!("{base}/sha256sums.txt"), &tmp_sums).is_ok() {
        verify_checksum(&tmp_zip, artifact, &tmp_sums)
            .context("checksum verification failed — leaving the installed app alone")?;
    } else {
        ui.warn("could not fetch sha256sums.txt — skipping checksum verification");
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
        ui.spin("quitting SbxwIsland", || {
            quit_island();
            Ok(())
        })?;
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
    if island_gone_within(30) {
        return;
    }
    let _ = std::process::Command::new("pkill")
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
fn install_binary(ui: &Ui, src: &std::path::Path, dest: &std::path::Path) -> Result<()> {
    // Output captured: its "Permission denied" is the expected case, said better below.
    let out = std::process::Command::new("mv")
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
    let status = std::process::Command::new("sudo")
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
