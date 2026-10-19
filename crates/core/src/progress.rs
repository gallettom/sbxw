//! A bring-up's running commentary: the steps it will take, announced before
//! the first, then which one is running and what `sbx` printed under it.
//!
//! Plain data. Whoever runs a bring-up passes a sink; the daemon forwards it to
//! every browser tab and to the terminal that typed `sbxw up`.

use serde::Serialize;
use std::sync::Arc;

pub const WORKSPACE: &str = "workspace";
pub const CREATE: &str = "create";
pub const BOOT: &str = "boot";
pub const TOOLING: &str = "tooling";
pub const POLICY: &str = "policy";
pub const KITS: &str = "kits";
pub const PORTS: &str = "ports";

const MAX_DETAIL: usize = 160;

#[derive(Clone, Serialize, Debug, PartialEq, serde::Deserialize)]
pub struct Step {
    pub id: String,
    pub label: String,
}

impl Step {
    pub fn new(id: &str, label: impl Into<String>) -> Self {
        Step {
            id: id.to_string(),
            label: label.into(),
        }
    }
}

/// Flattened on the wire beside the sandbox name: a tab switches on `kind`.
#[derive(Clone, Serialize, Debug, PartialEq, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Progress {
    Plan { steps: Vec<Step> },
    Started { id: String },
    Detail { text: String },
    /// Also says what is left to do by hand (a `/etc/hosts` alias nobody could
    /// authorise), without failing the bring-up.
    Warning { text: String },
    Done { error: Option<String> },
}

pub type Sink = Arc<dyn Fn(Progress) + Send + Sync>;

/// A sink that drops everything.
pub fn silent() -> Sink {
    Arc::new(|_| {})
}

/// One line of child output as a detail: trimmed, blank dropped, clipped.
pub fn detail(text: &str) -> Option<Progress> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let text = match text.char_indices().nth(MAX_DETAIL) {
        Some((i, _)) => format!("{}…", &text[..i]),
        None => text.to_string(),
    };
    Some(Progress::Detail { text })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_are_trimmed_dropped_and_clipped() {
        assert_eq!(detail("   "), None);
        assert_eq!(detail(" x "), Some(Progress::Detail { text: "x".into() }));
        match detail(&"y".repeat(500)).unwrap() {
            Progress::Detail { text } => assert_eq!(text.chars().count(), MAX_DETAIL + 1),
            _ => unreachable!(),
        }
    }

    #[test]
    fn the_wire_shape_is_tagged_by_kind() {
        let v = serde_json::to_value(Progress::Started { id: CREATE.into() }).unwrap();
        assert_eq!(v, serde_json::json!({"kind": "started", "id": "create"}));
    }
}
