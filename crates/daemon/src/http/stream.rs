//! Server-sent events: one bus, filtered per endpoint.
//!
//! `/api/stream` is the browser's single connection, every event named (plain
//! HTTP has no multiplexing and browsers cap connections per origin at six).
//! The island keeps its older single-kind feeds, each a filter of the same bus.

use crate::app::App;
use crate::bus::Event;
use axum::extract::State;
use axum::response::sse::{Event as Sse, KeepAlive};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::json;
use std::convert::Infallible;
use std::sync::Arc;
use tokio_stream::wrappers::{BroadcastStream, ReceiverStream};

type SseResponse = axum::response::Sse<std::pin::Pin<Box<dyn Stream<Item = Result<Sse, Infallible>> + Send>>>;

fn sse(stream: impl Stream<Item = Sse> + Send + 'static) -> SseResponse {
    axum::response::Sse::new(Box::pin(stream.map(Ok)) as _).keep_alive(KeepAlive::default())
}

/// Every event of the bus that `pick` keeps, as an SSE frame. A lagged
/// receiver skips the gap rather than closing.
fn feed(app: &App, pick: fn(Event) -> Option<Sse>) -> SseResponse {
    sse(BroadcastStream::new(app.bus.subscribe()).filter_map(move |r| async move { r.ok().and_then(pick) }))
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/stream", get(stream))
        .route("/api/events", get(|State(app): State<Arc<App>>| async move {
            feed(&app, |e| match e {
                Event::Session(_) => Some(Sse::default().data(e.data())),
                _ => None,
            })
        }))
        .route("/api/relay/events", get(|State(app): State<Arc<App>>| async move {
            feed(&app, |e| match e {
                Event::Relay(_) => Some(Sse::default().data(e.data())),
                _ => None,
            })
        }))
        .route("/api/watch-events", get(|State(app): State<Arc<App>>| async move {
            feed(&app, |e| match e {
                Event::Watching(_) => Some(Sse::default().data(e.data())),
                _ => None,
            })
        }))
        .route("/api/focus", post(focus))
        .route("/api/watching", post(watching))
}

/// The browser's one connection: every event by name, plus `clients`, the
/// number of tabs open (the UI warns past one).
async fn stream(State(app): State<Arc<App>>) -> SseResponse {
    let (tx, rx) = tokio::sync::mpsc::channel::<Sse>(64);
    app.bus.clients.send_modify(|n| *n += 1);
    let _ = tx.try_send(Sse::default().event("clients").data(app.bus.clients.borrow().to_string()));
    let mut events = app.bus.subscribe();
    let mut clients = app.bus.clients.subscribe();
    let counter = app.bus.clients.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = tx.closed() => break,
                ev = events.recv() => match ev {
                    Ok(Event::Watching(_)) => {}
                    Ok(e) => {
                        if tx.send(Sse::default().event(e.name()).data(e.data())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => break,
                },
                changed = clients.changed() => {
                    if changed.is_err() { break; }
                    let n = *clients.borrow();
                    if tx.send(Sse::default().event("clients").data(n.to_string())).await.is_err() { break; }
                }
            }
        }
        counter.send_modify(|n| *n = n.saturating_sub(1));
    });
    sse(ReceiverStream::new(rx))
}

#[derive(Deserialize)]
struct SandboxBody {
    sandbox: String,
}

/// The island asks open tabs to show a sandbox; `clients` tells it whether a
/// tab is there to reuse or one must be opened.
async fn focus(State(app): State<Arc<App>>, Json(b): Json<SandboxBody>) -> Json<serde_json::Value> {
    let clients = *app.bus.clients.borrow();
    app.send(Event::Focus(b.sandbox));
    Json(json!({ "clients": clients }))
}

/// A tab reports the user is reading a sandbox's terminal: an event, not a
/// flag, so no heartbeat and nothing stale to silence a sandbox later.
async fn watching(State(app): State<Arc<App>>, Json(b): Json<SandboxBody>) -> Json<serde_json::Value> {
    let clients = *app.bus.clients.borrow();
    app.send(Event::Watching(b.sandbox));
    Json(json!({ "clients": clients }))
}
