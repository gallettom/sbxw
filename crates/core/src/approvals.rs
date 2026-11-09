//! sbx's pending approvals (`sbx policy approval`, 0.51): what sbx waits for a
//! person to decide — a host the sandbox reached that no rule allows, a kit's
//! request for a host port (which lapses after 45 s). sbx keeps one list, each
//! entry offering its own options; sbxw shows it and passes the answer back,
//! never choosing one itself.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApprovalOption {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Approval {
    /// `<kind>:<opaque>` — `network:…` for a refused host.
    pub id: String,
    /// What the id's prefix says it is: `network`, `port`…
    pub kind: String,
    pub sandbox: String,
    /// The thing asked about: `api.github.com:443`.
    pub title: String,
    pub detail: String,
    /// RFC 3339.
    pub created_at: String,
    pub options: Vec<ApprovalOption>,
}

impl Approval {
    pub fn offers(&self, option: &str) -> bool {
        self.options.iter().any(|o| o.id == option)
    }
}

/// `sbx policy approval ls --format json`: a bare array. `None` when `raw` is
/// not that; an entry missing its id or options is skipped, since nothing
/// could be answered on it.
pub fn parse(raw: &str) -> Option<Vec<Approval>> {
    let rows: Vec<serde_json::Value> = serde_json::from_str(raw.trim()).ok()?;
    let text = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    Some(
        rows.iter()
            .filter_map(|v| {
                let id = text(v, "id");
                let options: Vec<ApprovalOption> = v
                    .get("options")?
                    .as_array()?
                    .iter()
                    .filter_map(|o| Some(ApprovalOption { id: o.get("id")?.as_str()?.to_string(), label: o.get("label").and_then(|l| l.as_str()).unwrap_or("").to_string() }))
                    .collect();
                if id.is_empty() || options.is_empty() {
                    return None;
                }
                Some(Approval {
                    kind: id.split_once(':').map(|(k, _)| k.to_string()).unwrap_or_default(),
                    sandbox: text(v, "sandbox_name"),
                    title: text(v, "title"),
                    detail: text(v, "detail"),
                    created_at: text(v, "created_at"),
                    options,
                    id,
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_sbx_0_51_lists_is_read() {
        // Trimmed from a real `sbx policy approval ls --format json`.
        let raw = r#"[{"created_at":"2026-11-06T15:08:54.850902+01:00","detail":"Protocol: TCP\nResource type: domain\nno matching allow rule (default deny)","id":"network:85TMuE6","options":[{"id":"allow","label":"Allow"},{"id":"dismiss","label":"Dismiss"}],"sandbox_name":"sbxw","title":"cli.github.com:443"},
                      {"id":"port:x","sandbox_name":"myapp","title":"5173","options":[]}]"#;
        let a = parse(raw).unwrap();
        assert_eq!(a.len(), 1, "nothing to answer on, nothing listed");
        assert_eq!((a[0].kind.as_str(), a[0].sandbox.as_str(), a[0].title.as_str()), ("network", "sbxw", "cli.github.com:443"));
        assert!(a[0].offers("allow") && !a[0].offers("deny"));
        assert_eq!(parse("ID TITLE"), None);
        assert_eq!(parse("[]"), Some(vec![]));
    }
}
