//! The terminal WebSocket: a persistent PTY bridged to the browser.
//!
//! Reconnecting replays the last 256 KB — repainted, never re-asked: the
//! sequences that query the terminal are stripped from the replay, or a fresh
//! xterm answers each one back into the shell.

use super::ui::settle_ws_thread;
use crate::app::App;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use sbxw_core::session::{SessionKey, SessionKind};
use sbxw_core::terminal::strip_replay_side_effects;
use sbxw_core::SandboxName;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct WsQuery {
    sandbox: Option<String>,
    mode: Option<String>,
    thread: Option<String>,
    repo: Option<String>,
    model: Option<String>,
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
        match SandboxName::parse(&raw) {
            Ok(sb) => SessionKey::new(sb, kind),
            Err(e) => return (StatusCode::BAD_REQUEST, e.0).into_response(),
        }
    };
    // Only a thread's own session chooses its repo and model.
    let (repo, model) = match key.kind {
        SessionKind::Thread(_) => (q.repo, q.model.filter(|m| !m.is_empty())),
        _ => (None, None),
    };
    if model.as_deref().is_some_and(|m| !sbxw_core::names::is_valid_model_id(m)) {
        return (StatusCode::BAD_REQUEST, "invalid model id").into_response();
    }
    ws.on_upgrade(move |socket| async move {
        if let Err(e) = bridge(socket, app, key, repo, model).await {
            tracing::warn!("tty bridge ended: {e:#}");
        }
    })
}

async fn bridge(socket: WebSocket, app: Arc<App>, key: SessionKey, repo: Option<String>, model: Option<String>) -> anyhow::Result<()> {
    let (mut tx, mut rx) = socket.split();
    let opened = async {
        if let Some(sb) = &key.sandbox {
            settle_ws_thread(&app, sb, &key.kind, repo.as_deref(), model.as_deref()).map_err(anyhow::Error::msg)?;
        }
        app.open_session(&key).await
    }
    .await;
    let session = match opened {
        Ok((s, _)) => s,
        Err(e) => {
            // Said in the pane, where the person who asked is looking.
            let _ = tx.send(Message::Text(format!("\r\n\x1b[31m[sbxw] {e:#}\x1b[0m\r\n"))).await;
            return Err(e);
        }
    };
    // Subscribe before reading the replay, so nothing falls in between.
    let mut out = session.subscribe();
    let mut bell = session.subscribe_bell();
    let replay = strip_replay_side_effects(&session.replay());
    if !replay.is_empty() {
        tx.send(Message::Binary(replay)).await.ok();
    }
    let sandbox = key.sandbox_str().to_string();
    let pump = tokio::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            tokio::select! {
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
            Message::Binary(data) => session.write(&[&data]).await,
            Message::Text(text) => {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
                if v.get("type").and_then(|t| t.as_str()) == Some("resize") {
                    let cols = v.get("cols").and_then(|c| c.as_u64()).unwrap_or(100) as u16;
                    let rows = v.get("rows").and_then(|r| r.as_u64()).unwrap_or(30) as u16;
                    match session.resize(cols, rows) {
                        Ok(before) if before != Some((cols, rows)) => tracing::info!(
                            "resize {key}: {} → {cols}x{rows}",
                            before.map_or("?".into(), |(c, r)| format!("{c}x{r}"))
                        ),
                        Ok(_) => {}
                        Err(e) => tracing::warn!("resize {key} to {cols}x{rows} failed: {e}"),
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    // The socket closed — the PTY stays, for the next reconnect.
    pump.abort();
    Ok(())
}
