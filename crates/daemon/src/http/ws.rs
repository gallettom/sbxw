//! The terminal WebSocket: a persistent PTY bridged to the browser.
//!
//! Reconnect replays the last 256 KB, repainted, with terminal queries stripped
//! (or a fresh xterm answers each one back into the shell).
//!
//! Messages:
//! * Binary, both ways: PTY bytes. Typed input (not the terminal's answers to
//!   the program, see [`crate::pty::is_typed_input`]) makes this client the
//!   size's owner.
//! * Text from the client: `{"type":"resize","cols","rows"}`, the size this
//!   client wants; it becomes the owner and the PTY takes that size (one size
//!   for all clients: see `pty.rs`, "One size for every client").
//! * Text from the daemon: an error line to show (not JSON);
//!   `{"type":"attention","sandbox"}` on a real bell; and, only to a client
//!   that opened with `size=1`, `{"type":"size","cols","rows","owner"}`: the
//!   PTY's size and whether this client sets it, sent before the replay and on
//!   every change. Clients that did not ask never get it (the web UI before
//!   this frame wrote unknown JSON into the terminal).

use super::ui::settle_ws_thread;
use crate::app::App;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use sbxw_core::session::{SessionKey, SessionKind};
use sbxw_core::terminal::strip_replay_side_effects;
use sbxw_core::SandboxRef;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct WsQuery {
    sandbox: Option<String>,
    mode: Option<String>,
    thread: Option<String>,
    repo: Option<String>,
    model: Option<String>,
    /// `1` on a session's first open: in the main checkout, not a worktree.
    checkout: Option<String>,
    /// `1`: send `{"type":"size"}` frames.
    size: Option<String>,
}

pub async fn handler(ws: WebSocketUpgrade, Query(q): Query<WsQuery>, State(app): State<Arc<App>>) -> Response {
    let kind = match SessionKind::from_query(q.mode.as_deref(), q.thread.as_deref()) {
        Ok(k) => k,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
    let key = if kind == SessionKind::Monitor {
        SessionKey::monitor()
    } else {
        let raw = q.sandbox.unwrap_or_else(|| app.initial_sandbox.lock().unwrap().clone());
        match SandboxRef::parse(&raw) {
            Ok(sb) => SessionKey::new(sb, kind),
            Err(e) => return (StatusCode::BAD_REQUEST, e.0).into_response(),
        }
    };
    // Only a thread session chooses its repo, model and whether it shares the
    // main checkout.
    let (repo, model, checkout) = match key.kind {
        SessionKind::Thread(_) => (q.repo, q.model.filter(|m| !m.is_empty()), q.checkout.as_deref() == Some("1")),
        _ => (None, None, false),
    };
    if model.as_deref().is_some_and(|m| !sbxw_core::names::is_valid_model_id(m)) {
        return (StatusCode::BAD_REQUEST, "invalid model id").into_response();
    }
    let sizes = q.size.as_deref() == Some("1");
    ws.on_upgrade(move |socket| async move {
        if let Err(e) = bridge(socket, app, key, repo, model, checkout, sizes).await {
            tracing::warn!("tty bridge ended: {e:#}");
        }
    })
}

async fn bridge(
    socket: WebSocket,
    app: Arc<App>,
    key: SessionKey,
    repo: Option<String>,
    model: Option<String>,
    checkout: bool,
    sizes: bool,
) -> anyhow::Result<()> {
    let (mut tx, mut rx) = socket.split();
    let opened = async {
        if let Some(sb) = &key.sandbox {
            settle_ws_thread(&app, sb, &key.kind, repo.as_deref(), model.as_deref(), checkout).await.map_err(anyhow::Error::msg)?;
        }
        app.open_session(&key).await
    }
    .await;
    let session = match opened {
        Ok((s, _)) => s,
        Err(e) => {
            // Shown in the pane, where the requester is looking.
            let _ = tx.send(Message::Text(format!("\r\n\x1b[31m[sbxw] {e:#}\x1b[0m\r\n"))).await;
            return Err(e);
        }
    };
    // Subscribe before reading the replay, so nothing falls in between.
    let mut out = session.subscribe();
    let mut bell = session.subscribe_bell();
    let me = session.attach();
    let mut size_rx = session.sizes();
    // The size first, so the replay is drawn in a grid of the PTY's size.
    if sizes {
        let now = *size_rx.borrow_and_update();
        tx.send(Message::Text(size_frame(now, me))).await.ok();
    }
    let replay = strip_replay_side_effects(&session.replay());
    if !replay.is_empty() {
        tx.send(Message::Binary(replay)).await.ok();
    }
    let sandbox = key.sandbox_str();
    let pump = tokio::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            tokio::select! {
                r = size_rx.changed(), if sizes => match r {
                    Ok(()) => {
                        let now = *size_rx.borrow_and_update();
                        if tx.send(Message::Text(size_frame(now, me))).await.is_err() { break }
                    }
                    Err(_) => break,
                },
                r = out.recv() => match r {
                    Ok(chunk) => if tx.send(Message::Binary(chunk)).await.is_err() { break },
                    Err(RecvError::Lagged(n)) => tracing::warn!("WebSocket lagged, dropped {n} PTY frames"),
                    Err(RecvError::Closed) => break,
                },
                r = bell.recv() => match r {
                    Ok(()) => {
                        let msg = serde_json::json!({ "type": "attention", "sandbox": sandbox }).to_string();
                        if tx.send(Message::Text(msg)).await.is_err() { break }
                    }
                    Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => break,
                },
            }
        }
    });
    while let Some(Ok(msg)) = rx.next().await {
        match msg {
            Message::Binary(data) => {
                if crate::pty::is_typed_input(&data) {
                    session.touch(me);
                }
                session.write(&[&data]).await
            }
            Message::Text(text) => {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
                if v.get("type").and_then(|t| t.as_str()) == Some("resize") {
                    let dim = |k: &str, d: u64| v.get(k).and_then(|c| c.as_u64()).unwrap_or(d).min(u16::MAX as u64) as u16;
                    let (cols, rows) = (dim("cols", 100), dim("rows", 30));
                    tracing::debug!("resize {key}: client {me} wants {cols}x{rows}");
                    session.want(me, cols, rows);
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    // The socket closed; the PTY stays, for the next reconnect. Its size goes
    // to another client after a grace period, unless one claims it first (a
    // reload does).
    pump.abort();
    if session.detach(me) {
        tokio::spawn(async move {
            tokio::time::sleep(crate::pty::HANDBACK_GRACE).await;
            session.hand_back();
        });
    }
    Ok(())
}

/// The `{"type":"size"}` frame for client `me`.
pub(crate) fn size_frame(s: crate::pty::EffectiveSize, me: u64) -> String {
    serde_json::json!({ "type": "size", "cols": s.cols, "rows": s.rows, "owner": s.owner == Some(me) }).to_string()
}
