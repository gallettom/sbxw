//! Who may call what.
//!
//! **The UI listener** serves the browser, the island and the extension, and
//! every decision moving information between sandboxes or starting work lives
//! here. Two checks, in order:
//!
//! 1. *Another web page* in the user's browser. Refuse a `Host` that is not
//!    the daemon's (DNS rebinding) and an `Origin` that is not the daemon's
//!    page or the extension (a cross-site WebSocket would open a sandbox shell
//!    or run `monitor_cmd`). Native clients send no `Origin`.
//! 2. *Anyone else who can reach the port*: another user of this machine,
//!    another process, a sandbox whose policy reaches it. Every request carries
//!    the UI token (see `token`), as a bearer header or the browser's cookie.
//!    Without it, only three answers: the sign-in page at `/` (and the
//!    `/?t=` sign-in itself), the side panel's "sign in first" page, and a
//!    health reply reduced to the proof a client checks before it trusts the
//!    port.
//!
//! **The agent listener** serves sandboxes only, and only the calls an agent
//! may make. Each carries the sandbox's token, which identifies the calling
//! sandbox, so none can speak for another.

use super::error::ApiError;
use crate::app::App;
use super::token;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::{header, request::Parts, HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sbxw_core::ports;
use sbxw_core::SandboxRef;
use std::sync::Arc;

/// The extension's fixed id (from the public key in its manifest).
pub const EXTENSION_ID: &str = "ikdindkpcjhibhhmndgimmhndjbglgpe";

/// The `Host` values the UI answers to.
pub fn allowed_hosts(web_addr: &str) -> Option<Vec<String>> {
    let ip = ports::ip_of(web_addr);
    let port = ports::port_of(web_addr);
    if ip == "0.0.0.0" || ip == "::" {
        // Bound on every interface on purpose: any name may reach it; the
        // Origin check alone stands between it and a page.
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
    let ours = |t: &str| sbxw_core::ids::same_secret(t, &app.ui_token);
    // On loopback the cookie lives at `sbxw.localhost:<port>` only: a cookie
    // is scoped to a host name, not a port, and sandbox apps are published
    // on `127.0.0.1` and `localhost`, where a dev server would read it.
    let host = host.unwrap_or("").to_ascii_lowercase();
    let cookie_host = cookie_host(&app.cfg.web_addr);
    let cookie_here = cookie_host.as_deref().is_none_or(|c| c == host);
    let signed_in = presented_token(h, cookie_here).is_some_and(|t| ours(&t));
    let get = req.method() == Method::GET;
    let path = req.uri().path();
    if get && path == "/" {
        if let Some(t) = query_value(req.uri().query(), "t") {
            let fresh = ours(t);
            if fresh && !cookie_here {
                // Same sign-in, at the one name the cookie may be set for.
                let to = format!("http://{}/?{}", cookie_host.unwrap_or_default(), req.uri().query().unwrap_or(""));
                return (StatusCode::SEE_OTHER, [(header::LOCATION, to), (header::CACHE_CONTROL, "no-store".into())]).into_response();
            }
            if fresh || signed_in {
                return sign_in(fresh.then_some(app.ui_token.as_str()), req.uri().query());
            }
            return sign_in_page();
        }
    }
    if signed_in {
        // `/api/shutdown` exits without running destructors: forget the
        // token on the way in.
        if req.method() == Method::POST && path == "/api/shutdown" {
            token::remove(&app.store.paths, &app.ui_token);
        }
        return next.run(req).await;
    }
    match (get, path) {
        (true, "/") => sign_in_page(),
        (true, "/sidepanel") => side_panel_sign_in(),
        (true, "/api/health") => {
            let proof = query_value(req.uri().query(), "nonce")
                .filter(|n| (16..=128).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_alphanumeric()))
                .map(|n| token::proof(&app.ui_token, n));
            axum::Json(serde_json::json!({ "ok": true, "proof": proof })).into_response()
        }
        _ => {
            tracing::debug!("refused {} {}: no UI token", req.method(), path);
            (
                StatusCode::UNAUTHORIZED,
                [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                "sbxw: not signed in. Run `sbxw web` to open sbxw.\n",
            )
                .into_response()
        }
    }
}

/// The only `Host` a browser signs in at, when the daemon is on loopback
/// (`127.*`): `sbxw.localhost:<port>`. `None` for any other bind (a LAN
/// address, every interface): the cookie is then set for whatever name the
/// browser used, as there is no name of sbxw's own to give it.
pub fn cookie_host(web_addr: &str) -> Option<String> {
    ports::ip_of(web_addr)
        .starts_with("127.")
        .then(|| format!("{}:{}", ports::WEB_HOSTNAME, ports::port_of(web_addr)))
}

/// The token a request presents: `Authorization: Bearer`, or the cookie when
/// `cookie` (it arrived at the host it may be set for).
fn presented_token(h: &HeaderMap, cookie: bool) -> Option<String> {
    let bearer = h
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim);
    let cookie = || {
        if !cookie {
            return None;
        }
        h.get_all(header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(';'))
            .find_map(|kv| kv.trim().strip_prefix(token::COOKIE)?.strip_prefix('='))
    };
    bearer.or_else(cookie).filter(|t| !t.is_empty()).map(str::to_string)
}

fn query_value<'a>(query: Option<&'a str>, key: &str) -> Option<&'a str> {
    query?.split('&').find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
}

/// `/?t=<token>`: keep it as a cookie and drop it from the address (and the
/// history). Other parameters stay; the browser keeps the `#sandbox=`
/// fragment across the redirect.
fn sign_in(fresh: Option<&str>, query: Option<&str>) -> Response {
    let rest: Vec<&str> = query.unwrap_or("").split('&').filter(|kv| !kv.is_empty() && *kv != "t" && !kv.starts_with("t=")).collect();
    let to = if rest.is_empty() { "/".to_string() } else { format!("/?{}", rest.join("&")) };
    let mut res = (StatusCode::SEE_OTHER, [(header::LOCATION, to), (header::CACHE_CONTROL, "no-store".into())]).into_response();
    if let Some(t) = fresh {
        let cookie = format!("{}={t}; HttpOnly; SameSite=Strict; Path=/", token::COOKIE);
        if let Ok(v) = header::HeaderValue::from_str(&cookie) {
            res.headers_mut().insert(header::SET_COOKIE, v);
        }
    }
    res
}

const SIGN_IN_PAGE: &str = r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>sbxw</title>
<style>body{font:15px/1.5 system-ui,sans-serif;max-width:34em;margin:4em auto;padding:0 1em}code{background:#8882;padding:.1em .3em;border-radius:3px}</style></head>
<body>
<h1>Open sbxw from a terminal</h1>
<p>Run <code>sbxw web</code> (or <code>sbxw web &lt;sandbox&gt;</code>). It prints a sign-in address: open it in this browser.</p>
<p>The address changes each time the sbxw daemon starts, so a tab left open from before needs the new one.</p>
</body>
</html>
"#;

fn sign_in_page() -> Response {
    (StatusCode::UNAUTHORIZED, [(header::CACHE_CONTROL, "no-store")], axum::response::Html(SIGN_IN_PAGE)).into_response()
}

const SIDE_PANEL_SIGN_IN: &str = r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>sbxw side panel</title>
<style>body{font:14px/1.5 system-ui,sans-serif;margin:1.5em 1em}code{background:#8882;padding:.1em .3em;border-radius:3px}</style></head>
<body>
<p><b>This browser is not signed in to sbxw.</b></p>
<p>In a terminal, run <code>sbxw web</code> (or <code>sbxw extension</code>) and open the sign-in address it prints, in this browser. Then <a href="/sidepanel">reload the panel</a>.</p>
<p>Needed again each time the sbxw daemon starts.</p>
</body>
</html>
"#;

/// `/sidepanel` without the cookie: framed by the extension, so it says what
/// to do instead of a blank frame.
fn side_panel_sign_in() -> Response {
    let csp = format!("frame-ancestors 'self' chrome-extension://{EXTENSION_ID}");
    (
        StatusCode::UNAUTHORIZED,
        [(header::CONTENT_SECURITY_POLICY, csp), (header::CACHE_CONTROL, "no-store".into())],
        axum::response::Html(SIDE_PANEL_SIGN_IN),
    )
        .into_response()
}

/// The sandbox an agent call comes from, proven by its token: the
/// `Authorization: Bearer` header, or `?token=` (the code map kit's bare
/// `curl "$SBXW_CODEMAP_DONE"`).
pub struct Agent(pub SandboxRef);

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
        let sb = app
            .store
            .sandbox_for_token(&token)
            .ok_or_else(|| ApiError::unauthorized("unknown agent token — run `sbxw up <name>` to reinstall the tooling"))?;
        Ok(Agent(sb))
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
