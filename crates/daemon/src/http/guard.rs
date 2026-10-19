//! Who may call what.
//!
//! **The UI listener** serves the browser, the island and the extension. A
//! sandbox cannot reach it — its network policy allows the agent port only —
//! so every decision that moves information between sandboxes or starts work
//! lives here. What remains to stop is *another web page* in the user's
//! browser: a request whose `Host` is not the daemon's (DNS rebinding) or
//! whose `Origin` is not the daemon's page or the extension (a cross-site
//! WebSocket would otherwise open a shell in a sandbox, or run `monitor_cmd`
//! on the host). Native clients send no `Origin` and pass.
//!
//! **The agent listener** serves sandboxes only, and only the calls an agent
//! may make. Each carries the sandbox's token; the daemon derives *which*
//! sandbox is calling from it, so none can speak for another.

use super::error::ApiError;
use crate::app::App;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::{header, request::Parts};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sbxw_core::ports;
use sbxw_core::SandboxName;
use std::sync::Arc;

/// The extension's fixed id (from the public key in its manifest).
pub const EXTENSION_ID: &str = "ikdindkpcjhibhhmndgimmhndjbglgpe";

/// The `Host` values the UI answers to.
pub fn allowed_hosts(web_addr: &str) -> Option<Vec<String>> {
    let ip = ports::ip_of(web_addr);
    let port = ports::port_of(web_addr);
    if ip == "0.0.0.0" || ip == "::" {
        // Bound on every interface on purpose: any name may reach it, and
        // only the Origin check stands between it and a page.
        return None;
    }
    let mut hosts = vec![
        format!("{}:{port}", ports::WEB_HOSTNAME),
        format!("localhost:{port}"),
        format!("127.0.0.1:{port}"),
        format!("[::1]:{port}"),
    ];
    let shown = if ip.contains(':') { format!("[{ip}]:{port}") } else { format!("{ip}:{port}") };
    if !hosts.contains(&shown) {
        hosts.push(shown);
    }
    Some(hosts)
}

/// May a request with these headers reach the UI?
pub fn ui_allows(allowed: Option<&[String]>, host: Option<&str>, origin: Option<&str>) -> Result<(), &'static str> {
    let host = host.unwrap_or("").to_ascii_lowercase();
    if let Some(list) = allowed {
        if !list.contains(&host) {
            return Err("unknown Host — reach sbxw at http://sbxw.localhost:<port>");
        }
    }
    match origin {
        None => Ok(()),
        Some(o) => {
            let o = o.trim_end_matches('/').to_ascii_lowercase();
            let same = o.strip_prefix("http://").or_else(|| o.strip_prefix("https://")) == Some(host.as_str());
            if same || o == format!("chrome-extension://{EXTENSION_ID}") {
                Ok(())
            } else {
                Err("cross-origin requests are refused")
            }
        }
    }
}

pub async fn ui_guard(State(app): State<Arc<App>>, req: Request, next: Next) -> Response {
    let h = req.headers();
    let host = h.get(header::HOST).and_then(|v| v.to_str().ok());
    let origin = h.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    let allowed = allowed_hosts(&app.cfg.web_addr);
    if let Err(why) = ui_allows(allowed.as_deref(), host, origin) {
        tracing::warn!("refused {} {} (host {host:?}, origin {origin:?}): {why}", req.method(), req.uri().path());
        return ApiError::forbidden(why).into_response();
    }
    next.run(req).await
}

/// The sandbox an agent call comes from, proven by its token: the
/// `Authorization: Bearer` header, or `?token=` (the code map kit's bare
/// `curl "$SBXW_CODEMAP_DONE"`).
pub struct Agent(pub SandboxName);

#[axum::async_trait]
impl FromRequestParts<Arc<App>> for Agent {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, app: &Arc<App>) -> Result<Self, ApiError> {
        let bearer = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string);
        let query = parts.uri.query().and_then(|q| {
            q.split('&')
                .find_map(|kv| kv.strip_prefix("token="))
                .map(|t| t.to_string())
                .filter(|t| !t.is_empty())
        });
        let Some(token) = bearer.or(query) else {
            return Err(ApiError::unauthorized(
                "no agent token — this sandbox's tooling predates it; run `sbxw up <name>` to reinstall it",
            ));
        };
        app.store
            .sandbox_for_token(&token)
            .map(Agent)
            .ok_or_else(|| ApiError::unauthorized("unknown agent token — run `sbxw up <name>` to reinstall the tooling"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ui_answers_its_own_names_and_origins_only() {
        let hosts = allowed_hosts("127.0.0.1:7681");
        let h = hosts.as_deref();
        assert!(ui_allows(h, Some("sbxw.localhost:7681"), None).is_ok());
        assert!(ui_allows(h, Some("127.0.0.1:7681"), Some("http://127.0.0.1:7681")).is_ok());
        assert!(ui_allows(h, Some("evil.example:7681"), None).is_err(), "DNS rebinding");
        assert!(ui_allows(h, Some("127.0.0.1:7681"), Some("http://evil.example")).is_err(), "CSWSH");
        assert!(ui_allows(h, Some("127.0.0.1:7681"), Some("null")).is_err());
        assert!(ui_allows(h, Some("127.0.0.1:7681"), Some(&format!("chrome-extension://{EXTENSION_ID}"))).is_ok());
        assert!(ui_allows(h, Some("127.0.0.1:7681"), Some("chrome-extension://other")).is_err());
        assert!(ui_allows(h, None, None).is_err());
    }

    #[test]
    fn bound_everywhere_any_host_but_still_same_origin() {
        let any = allowed_hosts("0.0.0.0:7681");
        assert!(any.is_none());
        assert!(ui_allows(None, Some("192.168.1.9:7681"), Some("http://192.168.1.9:7681")).is_ok());
        assert!(ui_allows(None, Some("192.168.1.9:7681"), Some("http://evil.example")).is_err());
    }
}
