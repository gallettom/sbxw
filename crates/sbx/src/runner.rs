//! How an `sbx` command is run — the one seam tests replace.

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Where streamed output lines go.
pub type LineSink = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone, Default)]
pub struct Invocation {
    pub args: Vec<String>,
    /// Piped to stdin, then closed. Secrets travel here, never in argv.
    pub stdin: Option<Vec<u8>>,
    /// Called with each line of output as it arrives (stdout and stderr, split
    /// on `\r` too — a progress bar without a TTY still returns to column 0).
    pub on_line: Option<LineSink>,
}

impl Invocation {
    pub fn new(args: Vec<String>) -> Self {
        Self {
            args,
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Output {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Output {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
    pub fn ok(stdout: &str) -> Self {
        Self {
            code: Some(0),
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }
    pub fn fail(code: i32, stderr: &str) -> Self {
        Self {
            code: Some(code),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }
    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

/// `async_trait`, not native `async fn`: the runner is a trait object
/// (`Arc<dyn Runner>`), which native async traits cannot be.
#[async_trait]
#[allow(clippy::double_must_use)]
pub trait Runner: Send + Sync {
    async fn run(&self, inv: Invocation) -> Result<Output>;
}

/// The real thing: `tokio::process`, stdin closed unless given, both pipes
/// drained concurrently (a pipe nobody reads fills up and blocks the child).
pub struct Process {
    pub bin: String,
}

impl Default for Process {
    fn default() -> Self {
        Self { bin: "sbx".into() }
    }
}

#[async_trait]
impl Runner for Process {
    async fn run(&self, inv: Invocation) -> Result<Output> {
        tracing::debug!(target: "sbx", "sbx {}", inv.args.join(" "));
        let mut child = tokio::process::Command::new(&self.bin)
            .args(&inv.args)
            .stdin(if inv.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to spawn `{}` — is it on your PATH?", self.bin))?;
        // Stdin is fed while both pipes drain: sbx refusing at once (no such
        // sandbox) closes stdin unread, and its own message must win over the
        // broken pipe; a child writing a full pipe before it reads would
        // otherwise deadlock against this write.
        let stdin = child.stdin.take();
        let data = inv.stdin;
        let feed = async move {
            match (data, stdin) {
                (Some(data), Some(mut pipe)) => pipe.write_all(&data).await,
                _ => Ok(()),
            }
            // The pipe is dropped here, closing the child's stdin.
        };
        let out = child.stdout.take();
        let err = child.stderr.take();
        let sink = inv.on_line.clone();
        let (fed, stdout, stderr) = tokio::join!(feed, drain(out, sink.clone()), drain(err, sink));
        let status = child.wait().await?;
        if let Err(e) = fed {
            // A failure speaks for itself, in sbx's words. A success that
            // never took its input is not one: that input may be a secret.
            if status.success() {
                return Err(anyhow::Error::new(e).context(format!("`{}` exited before reading its input", self.bin)));
            }
        }
        Ok(Output {
            code: status.code(),
            stdout,
            stderr,
        })
    }
}

pub(crate) async fn drain<R: tokio::io::AsyncRead + Unpin>(
    pipe: Option<R>,
    sink: Option<LineSink>,
) -> Vec<u8> {
    let Some(mut pipe) = pipe else {
        return Vec::new();
    };
    let mut all = Vec::new();
    let mut line = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = match pipe.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        all.extend_from_slice(&buf[..n]);
        if let Some(sink) = &sink {
            for &b in &buf[..n] {
                if b == b'\n' || b == b'\r' {
                    emit(sink, &line);
                    line.clear();
                } else {
                    line.push(b);
                }
            }
        }
    }
    if let Some(sink) = &sink {
        emit(sink, &line);
    }
    all
}

fn emit(sink: &LineSink, line: &[u8]) {
    let text = strip_ansi(&String::from_utf8_lossy(line));
    if !text.trim().is_empty() {
        sink(text.trim());
    }
}

/// Drop ANSI escapes: colour survives a terminal, not a browser text node.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        if let Some('[') = chars.next() {
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        }
    }
    out
}

type Script = Box<dyn Fn(&[String]) -> Output + Send + Sync>;

/// A scripted `sbx` for tests: records every argv, answers with a closure.
pub struct Fake {
    pub calls: Mutex<Vec<Vec<String>>>,
    pub stdins: Mutex<Vec<Vec<u8>>>,
    answer: Script,
}

impl Fake {
    pub fn new(answer: impl Fn(&[String]) -> Output + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            stdins: Mutex::new(Vec::new()),
            answer: Box::new(answer),
        })
    }

    /// Every call so far, as space-joined strings.
    pub fn log(&self) -> Vec<String> {
        self.calls.lock().unwrap().iter().map(|c| c.join(" ")).collect()
    }

    pub fn called(&self, prefix: &str) -> bool {
        self.log().iter().any(|c| c.starts_with(prefix))
    }
}

#[async_trait]
impl Runner for Fake {
    async fn run(&self, inv: Invocation) -> Result<Output> {
        self.calls.lock().unwrap().push(inv.args.clone());
        if let Some(data) = &inv.stdin {
            self.stdins.lock().unwrap().push(data.clone());
        }
        let out = (self.answer)(&inv.args);
        if let Some(sink) = &inv.on_line {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                if !line.trim().is_empty() {
                    sink(line.trim());
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_escapes_are_stripped() {
        assert_eq!(strip_ansi("\x1b[32mpulling\x1b[0m image"), "pulling image");
        assert_eq!(strip_ansi("layer 1/4\x1b[2K"), "layer 1/4");
    }

    #[tokio::test]
    async fn a_real_process_streams_lines_split_on_carriage_returns() {
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let s = seen.clone();
        let runner = Process { bin: "sh".into() };
        let out = runner
            .run(Invocation {
                args: vec!["-c".into(), "printf 'a\\rb\\n'; printf 'err\\n' >&2; cat".into()],
                stdin: Some(b"piped".to_vec()),
                on_line: Some(Arc::new(move |l: &str| s.lock().unwrap().push(l.to_string()))),
            })
            .await
            .unwrap();
        assert!(out.success());
        assert!(out.stdout_str().ends_with("piped"));
        let mut lines = seen.lock().unwrap().clone();
        lines.sort();
        assert_eq!(lines, ["a", "b", "err", "piped"]);
    }

    fn big(n: usize) -> Vec<u8> {
        vec![b'x'; n]
    }

    /// sbx refusing at once (no such sandbox) closes its stdin unread: its
    /// own message and exit status are what the caller gets, not a broken pipe.
    #[tokio::test]
    async fn a_child_that_exits_without_reading_keeps_its_own_error() {
        let runner = Process { bin: "sh".into() };
        let out = runner
            .run(Invocation {
                args: vec!["-c".into(), "echo 'sandbox not found' >&2; exit 125".into()],
                stdin: Some(big(1 << 20)),
                on_line: None,
            })
            .await
            .expect("sbx's own answer, not a write error");
        assert_eq!(out.code, Some(125));
        assert_eq!(String::from_utf8_lossy(&out.stderr).trim(), "sandbox not found");
    }

    /// The same, but sbx says it succeeded without taking its input: that
    /// is not a success, the input never arrived.
    #[tokio::test]
    async fn a_child_that_succeeds_without_reading_its_input_is_an_error() {
        let runner = Process { bin: "sh".into() };
        let e = runner
            .run(Invocation { args: vec!["-c".into(), "exit 0".into()], stdin: Some(big(1 << 20)), on_line: None })
            .await
            .unwrap_err();
        assert!(format!("{e:#}").contains("before reading its input"), "{e:#}");
    }

    /// A child that writes a full pipe before it reads does not deadlock
    /// against a parent still writing its stdin.
    #[tokio::test]
    async fn output_is_drained_while_stdin_is_written() {
        let runner = Process { bin: "sh".into() };
        let run = runner.run(Invocation {
            args: vec!["-c".into(), "head -c 300000 /dev/zero; cat > /dev/null; echo done".into()],
            stdin: Some(big(300_000)),
            on_line: None,
        });
        let out = tokio::time::timeout(std::time::Duration::from_secs(10), run).await.expect("no deadlock").unwrap();
        assert!(out.success());
        assert_eq!(out.stdout.len(), 300_000 + "done\n".len());
    }
}
