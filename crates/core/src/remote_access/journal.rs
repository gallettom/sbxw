//! The remote journal (Q17): which device did what kind of thing, and when.
//! Never what it said: an entry has no field that could hold a message, an
//! answer or a transcription, by construction.

use serde::{Deserialize, Serialize};

/// Entries kept on disk.
pub const MAX_ENTRIES: usize = 1000;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Paired,
    Message,
    Answer,
    Interrupt,
    /// The model of a leader changed from a closed list (the id is never kept).
    Model,
    /// Lead mode turned on or off for a sandbox from the phone.
    LeadOn,
    LeadOff,
    Transcribe,
    /// A pairing the person refused on the Mac, or an action the daemon refused.
    Refused,
    Revoked,
    Logout,
    /// Dropped after 30 days idle.
    Expired,
    /// A file handed to a leader (its size class only, never its name).
    Upload,
    /// A ticket written on a leader's Kanban from the phone (never its text).
    TicketCreate,
    // Level 2 (API-level2.md section 8).
    PlanApprove,
    PlanRefuse,
    Merge,
    NetworkRespond,
    SandboxRemove,
    PasskeyRegistered,
    PasskeyFailed,
    /// The item changed between the challenge and the action: nothing done.
    Changed,
    /// A phone asked to set up Face ID (no digits, no request id): a flood
    /// of these means another client holds a copy of the device's cookie.
    PasskeyRequested,
    // Web Push (API.md section 2.8). Never the endpoint, a key or a payload:
    // `push_sent` carries the kind (`p` or `n`) in `target`, `push_failed`
    // its status class (`4xx`, `network`...).
    /// The phone turned notifications on.
    PushOn,
    /// Turned off: by the phone, or from the Mac.
    PushOff,
    PushSent,
    PushFailed,
    /// Apple said the subscription is gone (404, 410) or the sidecar refused
    /// it: deleted.
    PushGone,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub at_ms: u64,
    pub device_id: String,
    pub device_name: String,
    pub kind: Kind,
    /// `upload` only: how big, as a class.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<super::uploads::SizeClass>,
    /// Level 2: what was acted on, by NAME or id only (a plan id, a sandbox
    /// name, a host), never a goal, a note, a diff or a message. Absent from
    /// the line when there is none, so earlier lines read the same.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

/// Keep the newest [`MAX_ENTRIES`].
pub fn bound(entries: &mut Vec<Entry>) {
    if entries.len() > MAX_ENTRIES {
        let drop = entries.len() - MAX_ENTRIES;
        entries.drain(..drop);
    }
}

/// The journal file's lines, skipping any that do not parse.
pub fn parse_lines(text: &str) -> Vec<Entry> {
    let mut out: Vec<Entry> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    bound(&mut out);
    out
}

pub fn to_lines(entries: &[Entry]) -> String {
    entries.iter().filter_map(|e| serde_json::to_string(e).ok()).map(|l| l + "\n").collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_and_round_trips() {
        let mut v: Vec<Entry> = (0..MAX_ENTRIES as u64 + 5).map(|i| Entry { at_ms: i, device_id: "d".into(), device_name: "n".into(), kind: Kind::Message, size: None, target: None }).collect();
        bound(&mut v);
        assert_eq!(v.len(), MAX_ENTRIES);
        assert_eq!(v[0].at_ms, 5, "the oldest go");
        let text = to_lines(&v[..2]);
        assert_eq!(text, "{\"at_ms\":5,\"device_id\":\"d\",\"device_name\":\"n\",\"kind\":\"message\"}\n{\"at_ms\":6,\"device_id\":\"d\",\"device_name\":\"n\",\"kind\":\"message\"}\n");
        let up = Entry { at_ms: 9, device_id: "d".into(), device_name: "n".into(), kind: Kind::Upload, size: Some(super::super::uploads::SizeClass::Over10Mb), target: None };
        assert_eq!(to_lines(std::slice::from_ref(&up)), "{\"at_ms\":9,\"device_id\":\"d\",\"device_name\":\"n\",\"kind\":\"upload\",\"size\":\"over_10mb\"}\n");
        assert_eq!(parse_lines(&to_lines(std::slice::from_ref(&up))), [up]);
        assert_eq!(parse_lines(&format!("{text}garbage\n{{\"at_ms\":1,\"device_id\":\"d\",\"device_name\":\"n\",\"kind\":\"message\",\"text\":\"x\"}}\n")).len(), 2, "no extra field is read back");
    }
}
