//! `sbxw voice`: the voice concierge's models, from a terminal. The daemon
//! downloads (see its `svc::voice`); this starts the job and follows
//! `GET /api/voice`, so the terminal shows what the web UI's panel shows —
//! and Ctrl-C here leaves the download to the daemon, where `sbxw voice
//! cancel` or the UI stops it.

use super::{abs, load_config};
use crate::client::Daemon;
use anyhow::{bail, Result};
use clap::Subcommand;
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;
use std::time::Duration;

#[derive(Subcommand, Clone, Copy, Default)]
pub enum VoiceCmd {
    /// Which models the concierge uses and whether they are downloaded.
    #[default]
    Status,
    /// Download the models `[voice]` names (~6 GB, resumable, checksums from Hugging Face) into ~/.sbxw/models.
    Install {
        /// Do not ask before downloading.
        #[arg(long, short)]
        yes: bool,
    },
    /// Stop a running download; what is already there is kept to resume.
    Cancel,
    /// Delete every downloaded voice model.
    Rm,
}

pub fn run(cmd: VoiceCmd, config: &Path) -> Result<()> {
    let cfg = load_config(config)?;
    let daemon = Daemon::new(&cfg.web_addr);
    daemon.ensure(&sbxw_daemon::store::Paths::from_home(), &cfg.source.clone().map_or_else(|| abs(config), Ok)?, false)?;
    match cmd {
        VoiceCmd::Status => print_status(&daemon.get("/api/voice")?),
        VoiceCmd::Install { yes } => {
            let s = daemon.get("/api/voice")?;
            if s["ready"].as_bool().unwrap_or(false) {
                println!("the voice models are already installed");
                return Ok(());
            }
            print_status(&s);
            if !yes && !confirm(&format!("\ndownload about {}?", gb(s["approx_mb"].as_u64().unwrap_or(0))))? {
                bail!("nothing downloaded");
            }
            daemon.post("/api/voice/install", json!({}))?;
            let s = follow(&daemon)?;
            match s["job"]["state"].as_str() {
                Some("done") => println!("\ninstalled. Open the web UI's Concierge and hold its talk button (or {}) to talk.", s["push_to_talk_key"].as_str().unwrap_or("F8")),
                Some("cancelled") => bail!("cancelled — `sbxw voice install` resumes where it stopped"),
                _ => bail!("{}", s["job"]["error"].as_str().unwrap_or("the download did not finish")),
            }
        }
        VoiceCmd::Cancel => {
            daemon.post("/api/voice/cancel", json!({}))?;
            println!("download stopped; `sbxw voice install` resumes it");
        }
        VoiceCmd::Rm => {
            daemon.post("/api/voice/rm", json!({}))?;
            println!("voice models deleted");
        }
    }
    Ok(())
}

fn confirm(q: &str) -> Result<bool> {
    print!("{q} [y/N] ");
    std::io::stdout().flush()?;
    let mut a = String::new();
    std::io::stdin().read_line(&mut a)?;
    Ok(matches!(a.trim(), "y" | "Y" | "yes" | "o" | "oui"))
}

fn gb(mb: u64) -> String {
    format!("{:.1} GB", mb as f64 / 1000.0)
}

/// Poll until the job is no longer running, drawing one progress line.
fn follow(daemon: &Daemon) -> Result<Value> {
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let s = daemon.get("/api/voice")?;
        let j = &s["job"];
        match j["state"].as_str() {
            Some("running") | Some("verifying") => {
                let (done, total) = (j["done"].as_u64().unwrap_or(0), j["total"].as_u64().unwrap_or(0).max(1));
                let verb = if j["state"] == "verifying" { "checking" } else { "downloading" };
                print!(
                    "\r\x1b[2K  {verb} {} · {} — {:>3}% ({} / {} MB)",
                    j["model"].as_str().unwrap_or(""),
                    j["file"].as_str().unwrap_or(""),
                    done * 100 / total,
                    done / 1_000_000,
                    total / 1_000_000
                );
                std::io::stdout().flush()?;
            }
            _ => {
                println!();
                return Ok(s);
            }
        }
    }
}

fn print_status(s: &Value) {
    if !s["compiled"].as_bool().unwrap_or(false) {
        println!("note: this sbxw was built without voice; the models can be downloaded, but not used");
    }
    for m in s["models"].as_array().into_iter().flatten() {
        let state = match m["installed"]["state"].as_str() {
            Some("ready") => "installed".to_string(),
            Some("partial") => format!("partial ({} MB)", m["installed"]["have"].as_u64().unwrap_or(0) / 1_000_000),
            _ => "missing".to_string(),
        };
        println!(
            "{:<4} {:<44} ~{:>5} MB  {:<10} {}",
            m["role"].as_str().unwrap_or(""),
            m["label"].as_str().unwrap_or(""),
            m["approx_mb"].as_u64().unwrap_or(0),
            m["license"].as_str().unwrap_or(""),
            state
        );
    }
    if !s["ready"].as_bool().unwrap_or(false) {
        println!("\nnot installed — run `{}`", s["hint"].as_str().unwrap_or("sbxw voice install"));
    }
}
