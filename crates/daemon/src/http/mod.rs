//! Two listeners, two routers.
//!
//! * **UI** (`web_addr`): the browser, the island, the extension. Guarded
//!   against other web pages (Host + Origin, see `guard`).
//! * **Agent** (`agent_addr`): the only port a sandbox's policy allows, the
//!   only calls an agent may make, each authenticated by its token.
//!
//! An agent call that lands on the UI port — a sandbox still running tooling
//! from before the split — gets a 410 that says how to fix it.

pub mod agent;
pub mod assets;
pub mod error;
pub mod guard;
pub mod stream;
pub mod ui;
pub mod ws;

#[cfg(test)]
mod tests;

use crate::app::App;
use axum::routing::{any, get};
use axum::Router;
use std::sync::Arc;

pub fn ui_router(app: Arc<App>) -> Router {
    Router::new()
        .merge(assets::routes())
        .merge(stream::routes())
        .merge(ui::routes())
        .route("/ws", get(ws::handler))
        .route("/api/hook", any(moved))
        .route("/api/relay/ask", any(moved))
        .route("/api/relay/shot", any(moved))
        .route("/api/relay/wait", any(moved))
        .route("/api/relay/reply", any(moved))
        .route("/api/plans/propose", any(moved))
        .route("/api/plans/report", any(moved))
        .layer(axum::middleware::from_fn_with_state(app.clone(), guard::ui_guard))
        .with_state(app)
}

pub fn agent_router(app: Arc<App>) -> Router {
    agent::routes().with_state(app)
}

async fn moved() -> error::ApiError {
    error::ApiError::new(
        axum::http::StatusCode::GONE,
        "sandboxes reach sbxw on its agent port now — run `sbxw up <name>` to reinstall this sandbox's tooling",
    )
}

/// Serve both until the process ends.
pub async fn serve(app: Arc<App>) -> anyhow::Result<()> {
    let ui = tokio::net::TcpListener::bind(&app.cfg.web_addr).await.map_err(|e| {
        anyhow::anyhow!("cannot listen on {} ({e}) — is another sbxw daemon running? (`sbxw down`)", app.cfg.web_addr)
    })?;
    let agent = tokio::net::TcpListener::bind(&app.cfg.agent_addr)
        .await
        .map_err(|e| anyhow::anyhow!("cannot listen on {} ({e})", app.cfg.agent_addr))?;
    tracing::info!("web UI on http://{} (http://sbxw.localhost:{})", app.cfg.web_addr, app.cfg.web_port());
    tracing::info!("agent API on {} (sandboxes reach it as host.docker.internal)", app.cfg.agent_addr);
    let (u, a) = tokio::join!(
        axum::serve(ui, ui_router(app.clone())),
        axum::serve(agent, agent_router(app.clone()))
    );
    u?;
    a?;
    Ok(())
}
