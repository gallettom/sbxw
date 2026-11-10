//! Talking to the daemon — and starting it when nobody has.
//!
//! One daemon per machine (per `web_addr`). The CLI never starts a second one:
//! it asks `/api/health`, starts `sbxw daemon` detached when nothing answers,
//! and waits.
//!
//! Every request carries the UI token (`~/.sbxw/run/ui-token`), and none is
//! sent before whatever answers on the port has proved it knows that token
//! (`/api/health?nonce=`, see the daemon's `http::token`): a process squatting
//! the port gets neither the token nor an API key.

use anyhow::{anyhow, bail, Context, Result};
use sbxw_core::ports;
use sbxw_daemon::http::token;
use sbxw_daemon::store::Paths;
use serde_json::Value;
use std::io::BufRead;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct Daemon {
    pub base: String,
    paths: Paths,
    /// The token, once the daemon proved it knows it.
    verified: Mutex<Option<String>>,
}

/// What answers on the daemon's port.
enum Peer {
    Nobody,
    /// Our daemon, with the token it proved it knows.
    Ours(String),
    /// Something that could not prove it: another process, or an sbxw
    /// daemon from before the UI token.
    Stranger,
}

/// URL a client uses for the daemon; a wildcard bind is reached on loopback.
pub fn base_url(web_addr: &str) -> String {
    let ip = ports::ip_of(web_addr);
    let host = match ip {
        "" | "0.0.0.0" | "::" => "127.0.0.1".to_string(),
        v6 if v6.contains(':') => format!("[{v6}]"),
        other => other.to_string(),
    };
    format!("http://{host}:{}", ports::port_of(web_addr))
}

/// `page` with `?t=<token>` before its fragment.
fn with_token(page: &str, t: &str) -> String {
    let (before, fragment) = page.split_once('#').map_or((page, None), |(b, f)| (b, Some(f)));
    let before = before.trim_end_matches('/');
    match fragment {
        Some(f) => format!("{before}/?t={t}#{f}"),
        None => format!("{before}/?t={t}"),
    }
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
        Self { base: base_url(web_addr), paths: Paths::from_home(), verified: Mutex::new(None) }
    }

    /// Ask whoever answers to prove it knows the token in the file, without
    /// sending the token.
    fn probe(&self) -> Peer {
        let nonce = sbxw_core::ids::random_hex(16);
        let Ok(resp) = ureq::get(&format!("{}/api/health", self.base))
            .query("nonce", &nonce)
            .timeout(Duration::from_millis(800))
            .call()
        else {
            return Peer::Nobody;
        };
        let answer: Value = resp.into_json().unwrap_or(Value::Null);
        match (token::read(&self.paths), answer["proof"].as_str()) {
            (Some(t), Some(p)) if sbxw_core::ids::same_secret(p, &token::proof(&t, &nonce)) => Peer::Ours(t),
            _ => Peer::Stranger,
        }
    }

    fn stranger(&self) -> anyhow::Error {
        anyhow!(
            "something is listening on {} but could not prove it is your sbxw daemon, so sbxw sent it nothing. \
             Another process may be holding that port (an sbxw daemon older than v{} answers this way too: `sbxw down` stops it). \
             Stop it, or set another `web_addr`.",
            self.base.trim_start_matches("http://"),
            env!("CARGO_PKG_VERSION")
        )
    }

    /// The token to send, proved first; `fresh` forgets the one proved before
    /// (the daemon restarted and minted another).
    fn token(&self, fresh: bool) -> Result<String> {
        let mut verified = self.verified.lock().unwrap();
        if fresh {
            *verified = None;
        }
        if let Some(t) = verified.as_ref() {
            return Ok(t.clone());
        }
        match self.probe() {
            Peer::Ours(t) => Ok(verified.insert(t).clone()),
            Peer::Stranger => Err(self.stranger()),
            Peer::Nobody => bail!("the sbxw daemon is not reachable at {}", self.base),
        }
    }

    /// One request with the token, again with a fresh one after a 401.
    fn send(&self, req: impl Fn() -> ureq::Request, body: Option<&Value>) -> Result<ureq::Response> {
        let mut fresh = false;
        loop {
            let r = req().set("Authorization", &format!("Bearer {}", self.token(fresh)?));
            let answer = match body {
                Some(b) => r.send_json(b),
                None => r.call(),
            };
            match answer {
                Err(ureq::Error::Status(401, _)) if !fresh => fresh = true,
                other => return other.map_err(error_of),
            }
        }
    }

    /// The daemon's health, when *our* daemon answers.
    pub fn health(&self) -> Option<Value> {
        let Peer::Ours(t) = self.probe() else { return None };
        *self.verified.lock().unwrap() = Some(t.clone());
        ureq::get(&format!("{}/api/health", self.base))
            .set("Authorization", &format!("Bearer {t}"))
            .timeout(Duration::from_millis(800))
            .call()
            .ok()?
            .into_json()
            .ok()
    }

    pub fn get(&self, path: &str) -> Result<Value> {
        let url = format!("{}{path}", self.base);
        self.send(|| ureq::get(&url), None)?
            .into_json()
            .context("the daemon's answer was not JSON")
    }

    pub fn post(&self, path: &str, body: Value) -> Result<Value> {
        let url = format!("{}{path}", self.base);
        self.send(|| ureq::post(&url), Some(&body))?
            .into_json()
            .context("the daemon's answer was not JSON")
    }

    /// The address a browser signs in at: `base` (the page's address) with
    /// the token, once the daemon proved it knows it. `None` when no daemon
    /// of ours answers.
    pub fn sign_in_url(&self, page: &str) -> Option<String> {
        Some(with_token(page, &self.token(false).ok()?))
    }

    /// POST, read the NDJSON answer line by line, return the last line.
    pub fn stream(&self, path: &str, body: Value, mut each: impl FnMut(&Value)) -> Result<Value> {
        let url = format!("{}{path}", self.base);
        let resp = self.send(|| ureq::post(&url), Some(&body))?;
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

    /// Ensures the daemon runs, starting it detached if nothing answers.
    pub fn ensure(&self, paths: &Paths, config: &Path, use_api_key: bool) -> Result<()> {
        if let Peer::Stranger = self.probe() {
            return Err(self.stranger());
        }
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
            match self.probe() {
                Peer::Ours(t) => {
                    *self.verified.lock().unwrap() = Some(t);
                    return Ok(());
                }
                // Ours writes its token before it answers: anything else that
                // answers is not ours, and sbxw sends it nothing.
                Peer::Stranger => {
                    let _ = child.kill();
                    return Err(self.stranger());
                }
                Peer::Nobody => {}
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

    use std::io::{Read, Write};

    #[test]
    fn the_sign_in_address_keeps_the_fragment() {
        assert_eq!(with_token("http://sbxw.localhost:7681/#sandbox=a", "T"), "http://sbxw.localhost:7681/?t=T#sandbox=a");
        assert_eq!(with_token("http://127.0.0.1:7681", "T"), "http://127.0.0.1:7681/?t=T");
    }

    /// One HTTP server thread answering every request with `body`, recording
    /// what it was sent.
    fn squatter(body: &'static str) -> (String, std::sync::Arc<Mutex<Vec<String>>>) {
        serve(move |_| body.to_string())
    }

    fn serve(answer: impl Fn(&str) -> String + Send + 'static) -> (String, std::sync::Arc<Mutex<Vec<String>>>) {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for mut c in l.incoming().flatten() {
                let mut buf = [0u8; 4096];
                let n = c.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                let body = answer(&req);
                log.lock().unwrap().push(req);
                let _ = write!(c, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        (addr, seen)
    }

    #[test]
    fn a_process_squatting_the_port_gets_no_token() {
        let d = std::env::temp_dir().join(format!("sbxw-client-test-{}", std::process::id()));
        let paths = Paths::under(&d);
        token::write(&paths, "sbxw_ui_secret").unwrap();
        for answer in [r#"{"ok":true,"version":"3.3.0"}"#, r#"{"ok":true,"proof":"00"}"#] {
            let (addr, seen) = squatter(answer);
            let daemon = Daemon { base: base_url(&addr), paths: paths.clone(), verified: Mutex::new(None) };
            let err = daemon.post("/api/sandboxes/x/up", serde_json::json!({"api_key": "sk-ant-x"})).unwrap_err().to_string();
            assert!(err.contains(&addr) && err.contains("could not prove"), "{err}");
            assert!(daemon.health().is_none());
            assert!(daemon.sign_in_url("http://sbxw.localhost:1/").is_none());
            assert!(daemon.ensure(&paths, Path::new("sbxw.toml"), true).is_err(), "no second daemon on a taken port");
            let seen = seen.lock().unwrap();
            assert!(seen.iter().all(|r| r.starts_with("GET /api/health?nonce=")), "{seen:?}");
            assert!(!seen.iter().any(|r| r.contains("sbxw_ui_secret") || r.contains("sk-ant")), "{seen:?}");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_daemon_that_proves_the_token_gets_it() {
        let d = std::env::temp_dir().join(format!("sbxw-client-test-ok-{}", std::process::id()));
        let paths = Paths::under(&d);
        token::write(&paths, "sbxw_ui_secret").unwrap();
        let (addr, seen) = serve(|req| {
            let nonce = req.split("nonce=").nth(1).and_then(|r| r.split([' ', '&']).next());
            match nonce {
                Some(n) => format!(r#"{{"ok":true,"proof":"{}"}}"#, token::proof("sbxw_ui_secret", n)),
                None => r#"{"ok":true,"version":"x"}"#.to_string(),
            }
        });
        let daemon = Daemon { base: base_url(&addr), paths: paths.clone(), verified: Mutex::new(None) };
        assert_eq!(daemon.get("/api/state").unwrap()["version"], "x");
        assert_eq!(daemon.sign_in_url("http://h:1/#sandbox=a").unwrap(), "http://h:1/?t=sbxw_ui_secret#sandbox=a");
        let seen = seen.lock().unwrap();
        assert!(seen[0].starts_with("GET /api/health?nonce=") && !seen[0].contains("sbxw_ui_secret"), "{seen:?}");
        assert!(seen[1].starts_with("GET /api/state") && seen[1].to_ascii_lowercase().contains("authorization: bearer sbxw_ui_secret"), "{seen:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_base_url_is_loopback_for_a_wildcard() {
        assert_eq!(base_url("127.0.0.1:7681"), "http://127.0.0.1:7681");
        assert_eq!(base_url("0.0.0.0:7681"), "http://127.0.0.1:7681");
        assert_eq!(base_url("[::1]:7681"), "http://[::1]:7681");
    }
}
