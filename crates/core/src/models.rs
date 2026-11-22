//! The models a thread may be started on: Anthropic's live list, read by the
//! daemon through a sandbox once an hour, merged with `models` from
//! `sbxw.toml`, which is the fallback and the place to pin a model.
//!
//! Pure: parsing a page of `GET /v1/models`, keeping what looks like a Claude
//! chat model, and the merge. The daemon owns the schedule (`svc/models.rs`).

use crate::config::ModelChoice;
use crate::names::ModelId;

/// The first page; `page_url` adds the cursor for the next ones.
pub const MODELS_URL: &str = "https://api.anthropic.com/v1/models?limit=1000";
/// Headers the endpoint needs. The proxy of the sandbox adds the credential;
/// the beta header is what an OAuth bearer needs, and is ignored otherwise.
pub const MODELS_HEADERS: [(&str, &str); 2] = [("anthropic-version", "2023-06-01"), ("anthropic-beta", "oauth-2025-04-20")];
/// Read once at start, then once an hour.
pub const REFRESH_SECS: u64 = 3600;
/// A list longer than this many pages is not a model list.
pub const MAX_PAGES: usize = 10;
/// Labels come from outside: at most this many characters are shown.
const LABEL_MAX: usize = 64;

/// One model of the live list, already filtered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveModel {
    pub id: String,
    pub label: String,
    /// Unix seconds; 0 when the answer had none or an unreadable one.
    pub created_at: i64,
}

/// One page: the models it carried that we keep, and the cursor of the next
/// page when there is one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Page {
    pub models: Vec<LiveModel>,
    /// How many entries the page had before filtering (for the log).
    pub seen: usize,
    pub next: Option<String>,
}

pub fn page_url(after: Option<&str>) -> String {
    match after {
        Some(id) => format!("{MODELS_URL}&after_id={id}"),
        None => MODELS_URL.to_string(),
    }
}

/// A page of `GET /v1/models` (`data[].{id, display_name, created_at}`,
/// `has_more`, `last_id`), or why it is not one.
pub fn from_api(raw: &str) -> Result<Page, String> {
    let v: serde_json::Value = serde_json::from_str(raw.trim()).map_err(|e| format!("unreadable answer ({e})"))?;
    let Some(data) = v.get("data").and_then(serde_json::Value::as_array) else {
        let kind = v.pointer("/error/type").and_then(|t| t.as_str()).unwrap_or("no data array");
        return Err(format!("not a model list: {}", clean(kind, 64)));
    };
    let models = data.iter().filter_map(live_model).collect();
    // The cursor goes back into a URL: only a well-formed id is followed.
    let next = (v.get("has_more").and_then(serde_json::Value::as_bool) == Some(true))
        .then(|| v.get("last_id").and_then(|l| l.as_str()).and_then(|l| ModelId::parse(l).ok()))
        .flatten()
        .map(|id| id.as_str().to_string());
    Ok(Page { models, seen: data.len(), next })
}

fn live_model(m: &serde_json::Value) -> Option<LiveModel> {
    let id = m.get("id")?.as_str()?;
    if !is_claude_chat_model(id) {
        return None;
    }
    let label = m.get("display_name").and_then(|d| d.as_str()).map(|d| clean(d, LABEL_MAX)).filter(|d| !d.is_empty());
    let created_at = m.get("created_at").and_then(|c| c.as_str()).and_then(crate::usage::rfc3339_to_epoch).unwrap_or(0);
    Some(LiveModel { id: id.to_string(), label: label.unwrap_or_else(|| id.to_string()), created_at })
}

/// What `claude --model` takes and names a Claude model.
pub fn is_claude_chat_model(id: &str) -> bool {
    id.starts_with("claude-") && ModelId::parse(id).is_ok_and(|m| m.as_str() == id)
}

/// Control characters out, whitespace folded, cut to `max` characters.
fn clean(s: &str, max: usize) -> String {
    s.split(|c: char| c.is_whitespace() || c.is_control()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ").chars().take(max).collect()
}

/// The offered list. Never read: the configured list alone. Read: the live
/// models newest first, each under its configured label when `sbxw.toml`
/// names it, then the configured entries the live list does not have, in
/// their order (a pin outlives the model leaving the list).
pub fn offered(configured: &[ModelChoice], live: Option<&[LiveModel]>) -> Vec<ModelChoice> {
    let Some(live) = live else { return configured.to_vec() };
    let mut sorted: Vec<&LiveModel> = live.iter().collect();
    sorted.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| a.id.cmp(&b.id)));
    let mut out: Vec<ModelChoice> = Vec::with_capacity(live.len() + configured.len());
    for m in sorted {
        if out.iter().any(|o| o.id == m.id) {
            continue;
        }
        let label = configured.iter().find(|c| c.id == m.id).map_or_else(|| m.label.clone(), |c| c.label.clone());
        out.push(ModelChoice { id: m.id.clone(), label });
    }
    for c in configured {
        if !out.iter().any(|o| o.id == c.id) {
            out.push(c.clone());
        }
    }
    out
}

pub fn is_offered(list: &[ModelChoice], id: &str) -> bool {
    list.iter().any(|m| m.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::default_models;

    fn live(id: &str, label: &str, at: i64) -> LiveModel {
        LiveModel { id: id.into(), label: label.into(), created_at: at }
    }

    #[test]
    fn a_page_is_read_and_filtered() {
        let raw = r#"{"data":[
            {"type":"model","id":"claude-fable-5-2","display_name":"Claude Fable 5.2","created_at":"2026-10-01T00:00:00Z"},
            {"type":"model","id":"claude-opus-5-5","display_name":"Claude Opus 5.5","created_at":"2026-08-01T00:00:00Z"},
            {"type":"model","id":"text-embedder-1","display_name":"Embedder","created_at":"2026-01-01T00:00:00Z"},
            {"type":"model","id":"claude-Bad id","display_name":"x"},
            {"type":"model","id":"claude-quiet","display_name":"  two\nlines\u0007 "},
            {"type":"model","display_name":"no id"}
        ],"has_more":true,"first_id":"claude-fable-5-2","last_id":"claude-quiet"}"#;
        let p = from_api(raw).unwrap();
        assert_eq!(p.seen, 6);
        let ids: Vec<&str> = p.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["claude-fable-5-2", "claude-opus-5-5", "claude-quiet"]);
        assert_eq!(p.models[0].label, "Claude Fable 5.2");
        assert_eq!(p.models[0].created_at, 1_790_812_800);
        assert_eq!(p.models[2].label, "two lines");
        assert_eq!(p.models[2].created_at, 0);
        assert_eq!(p.next.as_deref(), Some("claude-quiet"));
        assert_eq!(page_url(p.next.as_deref()), "https://api.anthropic.com/v1/models?limit=1000&after_id=claude-quiet");
    }

    #[test]
    fn the_last_page_and_a_bad_cursor_end_the_paging() {
        assert_eq!(from_api(r#"{"data":[],"has_more":false,"last_id":"claude-x"}"#).unwrap().next, None);
        assert_eq!(from_api(r#"{"data":[],"has_more":true,"last_id":"x&evil=1"}"#).unwrap().next, None);
    }

    #[test]
    fn an_error_answer_is_explained_not_taken_for_an_empty_list() {
        let e = from_api(r#"{"type":"error","error":{"type":"authentication_error","message":"secret detail"}}"#).unwrap_err();
        assert!(e.contains("authentication_error"), "{e}");
        assert!(!e.contains("secret detail"));
        assert!(from_api("<html>").is_err());
    }

    #[test]
    fn only_claude_ids_claude_can_take_are_kept() {
        assert!(is_claude_chat_model("claude-fable-5-2"));
        for bad in ["fable", "claude-A", "claude-a b", " claude-x", "claude--flag;x", "-claude-x"] {
            assert!(!is_claude_chat_model(bad), "{bad}");
        }
    }

    #[test]
    fn never_read_offers_the_configured_list_alone() {
        assert_eq!(offered(&default_models(), None), default_models());
    }

    #[test]
    fn live_models_come_newest_first_and_configured_ones_keep_their_label_and_place() {
        let configured = vec![
            ModelChoice { id: "claude-opus-5-5".into(), label: "Opus".into() },
            ModelChoice { id: "claude-pinned-1".into(), label: "Pinned".into() },
        ];
        let l = [live("claude-opus-5-5", "Claude Opus 5.5", 2), live("claude-fable-5-2", "Claude Fable 5.2", 3), live("claude-old", "Old", 1), live("claude-old", "Old again", 1)];
        let got = offered(&configured, Some(&l));
        let pairs: Vec<(&str, &str)> = got.iter().map(|m| (m.id.as_str(), m.label.as_str())).collect();
        assert_eq!(pairs, [("claude-fable-5-2", "Claude Fable 5.2"), ("claude-opus-5-5", "Opus"), ("claude-old", "Old"), ("claude-pinned-1", "Pinned")]);
        assert!(is_offered(&got, "claude-pinned-1"));
        assert!(!is_offered(&got, "claude-unknown"));
    }

    #[test]
    fn an_empty_live_list_still_offers_the_configured_one() {
        assert_eq!(offered(&default_models(), Some(&[])), default_models());
    }
}
