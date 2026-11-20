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
    Transcribe,
    /// A pairing the person refused on the Mac, or an action the daemon refused.
    Refused,
    Revoked,
    Logout,
    /// Dropped after 30 days idle.
    Expired,
    /// A file handed to a leader (its size class only, never its name).
    Upload,
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
        let mut v: Vec<Entry> = (0..MAX_ENTRIES as u64 + 5).map(|i| Entry { at_ms: i, device_id: "d".into(), device_name: "n".into(), kind: Kind::Message, size: None }).collect();
        bound(&mut v);
        assert_eq!(v.len(), MAX_ENTRIES);
        assert_eq!(v[0].at_ms, 5, "the oldest go");
        let text = to_lines(&v[..2]);
        assert_eq!(text, "{\"at_ms\":5,\"device_id\":\"d\",\"device_name\":\"n\",\"kind\":\"message\"}\n{\"at_ms\":6,\"device_id\":\"d\",\"device_name\":\"n\",\"kind\":\"message\"}\n");
        let up = Entry { at_ms: 9, device_id: "d".into(), device_name: "n".into(), kind: Kind::Upload, size: Some(super::super::uploads::SizeClass::Over10Mb) };
        assert_eq!(to_lines(std::slice::from_ref(&up)), "{\"at_ms\":9,\"device_id\":\"d\",\"device_name\":\"n\",\"kind\":\"upload\",\"size\":\"over_10mb\"}\n");
        assert_eq!(parse_lines(&to_lines(std::slice::from_ref(&up))), [up]);
        assert_eq!(parse_lines(&format!("{text}garbage\n{{\"at_ms\":1,\"device_id\":\"d\",\"device_name\":\"n\",\"kind\":\"message\",\"text\":\"x\"}}\n")).len(), 2, "no extra field is read back");
    }
}
