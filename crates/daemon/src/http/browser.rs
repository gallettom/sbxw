//! The Browser panel: the sandbox's own Chromium, watched from the UI.
//!
//! `GET /ws/browser?sandbox=` runs `browser.mjs serve` in the sandbox through
//! `sbx exec -i` and carries its line protocol both ways: pictures, the
//! address and refused hosts out; clicks, keys and navigation in. No port is
//! published: Chromium's command port stays on the sandbox's loopback, and
//! what reaches the host is this one pipe, behind the UI token like the rest.
//!
//! What comes out of the pipe is the sandbox's word. It is passed on as text
//! for the page to show, never acted on here; the one thing a person does
//! with it, allowing a host, goes through `svc::browser`, which checks it.

use super::error::{ok_with, ApiError, ApiResult};
use crate::app::App;
use crate::svc::browser::{SCRIPT, SCRIPT_PATH};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use sbxw_core::SandboxRef;
use serde::Deserialize;
use serde_json::json;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

type S = State<Arc<App>>;

/// A picture is some tens of kilobytes; nothing honest comes near this.
const LINE_LIMIT: u64 = 8 << 20;
/// A click or a key is a few hundred bytes; a paste is capped by the page.
const INPUT_LIMIT: usize = 256 << 10;
/// How much of the child's stderr is kept, for the reason it stopped.
const STDERR_TAIL: usize = 4096;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/ws/browser", get(socket))
        .route("/api/sandboxes/:name/browser/allow", post(allow))
        .route("/api/sandboxes/:name/browser/grants", get(grants))
        .route("/api/sandboxes/:name/browser/settle", post(settle))
        .route("/api/browser/active", get(active))
}

/// What a sandbox may call: its agent says it is using its browser, or has
/// stopped. Identity is the token's, never the body's.
pub fn agent_routes() -> Router<Arc<App>> {
    Router::new().route("/api/browser/state", post(state))
}

#[derive(Deserialize)]
struct StateBody {
    active: bool,
}

async fn state(State(app): S, super::guard::Agent(me): super::guard::Agent, Json(b): Json<StateBody>) -> ApiResult {
    app.browser_set_active(&me, b.active);
    Ok(super::error::ok())
}

async fn active(State(app): S) -> ApiResult {
    Ok(ok_with(json!({ "sandboxes": app.browsing() })))
}

fn sandbox(raw: &str) -> Result<SandboxRef, ApiError> {
    Ok(SandboxRef::parse(raw)?)
}

#[derive(Deserialize)]
struct HostBody {
    host: String,
}

async fn allow(State(app): S, Path(name): Path<String>, Json(b): Json<HostBody>) -> ApiResult {
    let grant = app.browser_allow(&sandbox(&name)?, &b.host).await?;
    Ok(ok_with(json!({ "grant": grant })))
}

async fn grants(State(app): S, Path(name): Path<String>) -> ApiResult {
    Ok(ok_with(json!({ "grants": app.browser_grants(&sandbox(&name)?) })))
}

#[derive(Deserialize)]
struct SettleBody {
    keep: bool,
}

async fn settle(State(app): S, Path(name): Path<String>, Json(b): Json<SettleBody>) -> ApiResult {
    let done = app.browser_settle(&sandbox(&name)?, b.keep).await?;
    Ok(ok_with(json!({ "settled": done })))
}

#[derive(Deserialize)]
struct SocketQuery {
    sandbox: String,
}

async fn socket(ws: WebSocketUpgrade, Query(q): Query<SocketQuery>, State(app): S) -> Response {
    let sb = match SandboxRef::parse(&q.sandbox) {
        Ok(sb) => sb,
        Err(e) => return (StatusCode::BAD_REQUEST, e.0).into_response(),
    };
    ws.on_upgrade(move |socket| async move {
        if let Err(e) = bridge(socket, app, sb).await {
            tracing::warn!("browser bridge ended: {e:#}");
        }
    })
}

/// The one field of a bridge message read here.
#[derive(Deserialize)]
struct Head<'a> {
    t: &'a str,
}

/// The last `keep` bytes of what `from` writes, read to its end.
async fn tail(mut from: impl tokio::io::AsyncRead + Unpin, keep: usize) -> String {
    let mut end: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match from.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                end.extend_from_slice(&buf[..n]);
                if end.len() > keep {
                    end.drain(..end.len() - keep);
                }
            }
        }
    }
    String::from_utf8_lossy(&end).into_owned()
}

fn note(kind: &str, msg: &str) -> Message {
    Message::Text(json!({ "t": kind, "msg": msg }).to_string())
}

async fn bridge(socket: WebSocket, app: Arc<App>, sb: SandboxRef) -> anyhow::Result<()> {
    let (mut tx, mut rx) = socket.split();
    if !sb.is_local() {
        let _ = tx.send(note("error", "The Browser panel works for sandboxes on this machine only, for now.")).await;
        return Ok(());
    }
    // Written at every open: a sandbox made before this sbxw has no copy, and
    // one made by an older sbxw has an older one.
    if let Err(e) = app.sbx.write_file(&sb.name, SCRIPT_PATH, SCRIPT.as_bytes(), false).await {
        let _ = tx.send(note("error", &format!("{e:#}"))).await;
        return Err(e);
    }
    let args = sbxw_sbx::exec_stream_args(&sb.name, &["node", SCRIPT_PATH, "serve"]);
    let mut child = match tokio::process::Command::new("sbx")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(note("error", &format!("cannot run sbx: {e}"))).await;
            return Err(e.into());
        }
    };
    let (Some(mut stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
        anyhow::bail!("sbx exec gave no pipes");
    };
    tracing::info!("browser panel: opened for sandbox '{sb}'");

    // Read as it comes, or a child that writes much to stderr fills the pipe
    // and stops. Only the end is kept: that is where the reason is.
    let said = tokio::spawn(tail(stderr, STDERR_TAIL));

    let gone = app.clone();
    let who = sb.clone();
    let mut out = tokio::spawn(async move {
        let mut reader = BufReader::with_capacity(64 << 10, stdout);
        let mut line = Vec::new();
        loop {
            line.clear();
            match (&mut reader).take(LINE_LIMIT).read_until(b'\n', &mut line).await {
                Ok(0) => break,
                Ok(_) if line.last() != Some(&b'\n') => {
                    tracing::warn!("browser bridge of '{who}': a line over {LINE_LIMIT} bytes, or cut short");
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::debug!("browser bridge of '{who}': read failed: {e}");
                    break;
                }
            }
            // Only the bridge's own messages go on: sbx may print lines of
            // its own on the way, and what a sandbox prints is its word.
            let Ok(head) = serde_json::from_slice::<Head>(&line) else { continue };
            // Chromium itself went away: nobody is browsing any more.
            if head.t == "closed" {
                gone.browser_set_active(&who, false);
            }
            let Ok(text) = std::str::from_utf8(&line) else { continue };
            if tx.send(Message::Text(text.trim_end().to_string())).await.is_err() {
                return;
            }
        }
        // The pipe ended without the bridge saying why: say what sbx said.
        if let Ok(said) = said.await {
            if let Some(last) = said.lines().rev().find(|l| !l.trim().is_empty()) {
                let _ = tx.send(note("error", last.trim())).await;
            }
        }
        let _ = tx.send(Message::Text(json!({ "t": "gone" }).to_string())).await;
    });

    // Until the page leaves or the bridge does: a bridge that stopped
    // reading must not keep its child alive behind an open socket.
    let mut bridge_ended = false;
    loop {
        tokio::select! {
            msg = rx.next() => match msg {
                Some(Ok(Message::Text(text))) => {
                    if text.len() > INPUT_LIMIT || text.contains('\n') {
                        continue;
                    }
                    if stdin.write_all(text.as_bytes()).await.is_err() || stdin.write_all(b"\n").await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            _ = &mut out => {
                bridge_ended = true;
                break;
            }
        }
    }
    // Closing stdin tells the bridge to leave; Chromium itself stays unless
    // the panel asked it to quit. A bridge that does not leave is killed.
    drop(stdin);
    if tokio::time::timeout(std::time::Duration::from_secs(3), child.wait()).await.is_err() {
        if let Err(e) = child.kill().await {
            tracing::warn!("browser bridge of '{sb}' did not stop: {e}");
        }
    }
    if !bridge_ended {
        out.abort();
    }
    tracing::info!("browser panel: closed for sandbox '{sb}'");
    Ok(())
}
