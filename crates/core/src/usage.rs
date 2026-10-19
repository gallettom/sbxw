//! Account-wide subscription usage: the 5-hour and weekly windows Claude
//! Code's `/usage` prints.
//!
//! Two writers, last one wins: each sandbox's `statusLine` forwards what Claude
//! Code was told (fresh while an agent works), and the daemon polls Anthropic
//! *through a sandbox* every five minutes (right before anyone has spoken). A
//! reading only overwrites a window it actually carries, so a session that has
//! not seen its first response does not wipe a good value.

use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Default, Debug, PartialEq)]
pub struct Usage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub five_hour_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seven_day_pct: Option<f64>,
    /// Unix seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub five_hour_resets_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seven_day_resets_at: Option<i64>,
    pub updated_ms: u64,
}

/// One reading, from either writer.
#[derive(Deserialize, Debug, Default, Clone, PartialEq)]
pub struct Reading {
    pub five_hour_pct: Option<f64>,
    pub seven_day_pct: Option<f64>,
    pub five_hour_resets_at: Option<i64>,
    pub seven_day_resets_at: Option<i64>,
}

impl Usage {
    pub fn apply(&mut self, r: Reading, now: u64) {
        if r.five_hour_pct.is_some() {
            self.five_hour_pct = r.five_hour_pct;
            self.five_hour_resets_at = r.five_hour_resets_at;
        }
        if r.seven_day_pct.is_some() {
            self.seven_day_pct = r.seven_day_pct;
            self.seven_day_resets_at = r.seven_day_resets_at;
        }
        self.updated_ms = now;
    }
}

/// The endpoint behind `/usage`: costs no tokens, touches no model.
pub const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
pub const POLL_SECS: u64 = 300;
/// Backoff ceiling: the endpoint rate-limits.
pub const POLL_MAX_SECS: u64 = 1800;

/// The endpoint's answer as a reading.
pub fn from_api(raw: &str) -> Result<Reading, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|e| format!("unreadable answer ({e})"))?;
    let window = |k: &str| match v.get(k) {
        None => (None, None),
        Some(w) => (
            w.get("utilization").and_then(serde_json::Value::as_f64),
            w.get("resets_at").and_then(|r| r.as_str()).and_then(rfc3339_to_epoch),
        ),
    };
    let (five_hour_pct, five_hour_resets_at) = window("five_hour");
    let (seven_day_pct, seven_day_resets_at) = window("seven_day");
    if five_hour_pct.is_none() && seven_day_pct.is_none() {
        let head: String = raw.trim().chars().take(200).collect();
        return Err(format!("no five_hour/seven_day windows in it: {head}"));
    }
    Ok(Reading {
        five_hour_pct,
        seven_day_pct,
        five_hour_resets_at,
        seven_day_resets_at,
    })
}

/// `2026-09-03T12:00:00.398343+00:00` → unix seconds. Hinnant's
/// `days_from_civil`; one machine-generated shape, no date crate.
pub fn rfc3339_to_epoch(s: &str) -> Option<i64> {
    let field = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (field(0..4)?, field(5..7)?, field(8..10)?);
    let (h, mi, sec) = (field(11..13)?, field(14..16)?, field(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    let y = y - i64::from(mo <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let mut epoch = days * 86_400 + h * 3600 + mi * 60 + sec;
    let tail = s.get(19..)?.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let sign = tail.chars().next();
    if matches!(sign, Some('+') | Some('-')) {
        let digits: String = tail[1..].chars().filter(char::is_ascii_digit).collect();
        let off = digits.get(0..2)?.parse::<i64>().ok()? * 3600
            + digits.get(2..4).and_then(|m| m.parse::<i64>().ok()).unwrap_or(0) * 60;
        epoch += if sign == Some('-') { off } else { -off };
    }
    Some(epoch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reset_timestamp_becomes_unix_seconds() {
        assert_eq!(rfc3339_to_epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(rfc3339_to_epoch("2026-09-03T12:00:00.398343+00:00"), Some(1_788_436_800));
        assert_eq!(rfc3339_to_epoch("2026-09-03T14:00:00+02:00"), Some(1_788_436_800));
        assert_eq!(rfc3339_to_epoch("garbage"), None);
    }

    #[test]
    fn a_partial_reading_keeps_what_it_does_not_carry() {
        let mut u = Usage::default();
        u.apply(Reading { five_hour_pct: Some(10.0), seven_day_pct: Some(20.0), ..Default::default() }, 1);
        u.apply(Reading { five_hour_pct: Some(30.0), ..Default::default() }, 2);
        assert_eq!(u.five_hour_pct, Some(30.0));
        assert_eq!(u.seven_day_pct, Some(20.0));
        assert_eq!(u.updated_ms, 2);
    }

    #[test]
    fn an_api_answer_is_read_or_explained() {
        let r = from_api(r#"{"five_hour":{"utilization":33.0,"resets_at":"1970-01-01T01:00:00Z"}}"#).unwrap();
        assert_eq!(r.five_hour_pct, Some(33.0));
        assert_eq!(r.five_hour_resets_at, Some(3600));
        assert!(from_api(r#"{"error":"rate limited"}"#).unwrap_err().contains("rate limited"));
        assert!(from_api("<html>").is_err());
    }
}
