//! Talking to the daemon — and starting it when nobody has.
//!
//! There is one daemon per machine (per `web_addr`). The CLI never becomes a
//! second one: it asks `/api/health`, starts `sbxw daemon` detached when
//! nothing answers, and waits for it to.

use anyhow::{anyhow, bail, Context, Result};
use sbxw_core::ports;
use sbxw_daemon::store::Paths;
use serde_json::Value;
use std::io::BufRead;
use std::path::Path;
use std::time::{Duration, Instant};

pub struct Daemon {
    pub base: String,
}

/// The URL a client reaches the daemon at: an address bound on every
/// interface is reached on the loopback.
pub fn base_url(web_addr: &str) -> String {
    let ip = ports::ip_of(web_addr);
    let host = match ip {
        "" | "0.0.0.0" | "::" => "127.0.0.1".to_string(),
        v6 if v6.contains(':') => format!("[{v6}]"),
        other => other.to_string(),
    };
    format!("http://{host}:{}", ports::port_of(web_addr))
}

fn error_of(e: ureq::Error) -> anyhow::Error {
    match e {
        ureq::Error::Status(code, resp) => {
            let body: Value = resp.into_json().unwrap_or(Value::Null);
            anyhow!("{}", body["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {code}")))
        }
        ureq::Error::Transport(t) => anyhow!("the sbxw daemon is not reachable: {t}"),
    }
}

impl Daemon {
    pub fn new(web_addr: &str) -> Self {
        Self { base: base_url(web_addr) }
    }

    pub fn health(&self) -> Option<Value> {
        ureq::get(&format!("{}/api/health", self.base))
            .timeout(Duration::from_millis(800))
            .call()
            .ok()?
            .into_json()
            .ok()
    }

    pub fn post(&self, path: &str, body: Value) -> Result<Value> {
        ureq::post(&format!("{}{path}", self.base))
            .send_json(body)
            .map_err(error_of)?
            .into_json()
            .context("the daemon's answer was not JSON")
    }

    /// POST and read an NDJSON answer line by line; returns the last line.
    pub fn stream(&self, path: &str, body: Value, mut each: impl FnMut(&Value)) -> Result<Value> {
        let resp = ureq::post(&format!("{}{path}", self.base)).send_json(body).map_err(error_of)?;
        let mut last = Value::Null;
        for line in std::io::BufReader::new(resp.into_reader()).lines() {
            let line = line.context("the daemon closed the stream")?;
            if line.trim().is_empty() {
                continue;
            }
            let v: Value = serde_json::from_str(&line).context("an unreadable progress line")?;
            each(&v);
            last = v;
        }
        Ok(last)
    }

    /// The daemon, running: started detached when nothing answers.
    pub fn ensure(&self, paths: &Paths, config: &Path, use_api_key: bool) -> Result<()> {
        if let Some(h) = self.health() {
            let theirs = h["version"].as_str().unwrap_or("?");
            if theirs != env!("CARGO_PKG_VERSION") {
                eprintln!(
                    "sbxw  note: the running daemon is v{theirs}, this is v{} — `sbxw down` then retry to switch",
                    env!("CARGO_PKG_VERSION")
                );
            }
            return Ok(());
        }
        std::fs::create_dir_all(paths.run())?;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(paths.daemon_log())
            .with_context(|| format!("opening {}", paths.daemon_log().display()))?;
        let mut cmd = std::process::Command::new(std::env::current_exe()?);
        cmd.arg("daemon").arg("--config").arg(config);
        if use_api_key {
            cmd.arg("--use-api-key");
        }
        cmd.stdout(log.try_clone()?).stderr(log).stdin(std::process::Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Out of this terminal's process group: Ctrl+C here must not reach it.
            cmd.process_group(0);
        }
        let mut child = cmd.spawn().context("could not start the sbxw daemon")?;
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if self.health().is_some() {
                return Ok(());
            }
            if let Ok(Some(status)) = child.try_wait() {
                let log = std::fs::read_to_string(paths.daemon_log()).unwrap_or_default();
                let tail: Vec<&str> = log.lines().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect();
                bail!("the sbxw daemon exited ({status}):\n{}", tail.join("\n"));
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        bail!("the sbxw daemon did not answer within 20 s — see {}", paths.daemon_log().display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_base_url_is_loopback_for_a_wildcard() {
        assert_eq!(base_url("127.0.0.1:7681"), "http://127.0.0.1:7681");
        assert_eq!(base_url("0.0.0.0:7681"), "http://127.0.0.1:7681");
        assert_eq!(base_url("[::1]:7681"), "http://[::1]:7681");
    }
}
