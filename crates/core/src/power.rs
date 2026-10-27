//! Keeping the host awake for Remote Control, and telling the agent when its
//! battery runs low.
//!
//! A Mac that idles to sleep takes Docker's VM, and with it every sandbox,
//! offline: a session driven from claude.ai stops answering mid-task. The
//! daemon holds `caffeinate` while it should stay up and lets go below a
//! battery floor, so a forgotten session never drains the machine flat.
//!
//! The person driving the session is away from the Mac and cannot see its
//! battery, so the agent is told instead: a note is added to its context on
//! the next prompt, once per few percent lost.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// `keep_awake` in `sbxw.toml`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum KeepAwake {
    /// Never hold the Mac awake.
    Off,
    /// While Remote Control is on in at least one session.
    #[default]
    Remote,
    /// For as long as the daemon runs.
    Always,
}

/// The `caffeinate` flags: no idle sleep (`-i`), no system sleep on AC
/// (`-s`), and gone with the daemon (`-w <pid>`) so a crash never leaves the
/// Mac held. The display may still sleep.
pub fn caffeinate_args(daemon_pid: u32) -> Vec<String> {
    vec!["-i".into(), "-s".into(), "-w".into(), daemon_pid.to_string()]
}

/// The host's battery, from `pmset -g batt`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Battery {
    pub percent: u8,
    /// Drawing from the battery rather than a charger.
    pub on_battery: bool,
}

/// Read `pmset -g batt`:
///
/// ```text
/// Now drawing from 'Battery Power'
///  -InternalBattery-0 (id=4653155)    82%; discharging; 5:12 remaining present: true
/// ```
///
/// `None` on a Mac without a battery (only the first line) or anything else.
pub fn parse_pmset(out: &str) -> Option<Battery> {
    let on_battery = out.lines().next()?.contains("'Battery Power'");
    let line = out.lines().find(|l| l.contains("InternalBattery"))?;
    let pct = line.split_whitespace().find_map(|w| w.trim_end_matches(';').strip_suffix('%'))?;
    Some(Battery { percent: pct.parse().ok()?, on_battery })
}

/// Whether `caffeinate` should be held now. Never on battery at or below
/// `min_battery`: the Mac is then let go to sleep rather than run flat.
pub fn hold(mode: KeepAwake, remote_on: bool, battery: Option<Battery>, min_battery: u8) -> bool {
    let wanted = match mode {
        KeepAwake::Off => false,
        KeepAwake::Remote => remote_on,
        KeepAwake::Always => true,
    };
    wanted && !battery.is_some_and(|b| b.on_battery && b.percent <= min_battery)
}

/// A fresh note is due only after this many more percent are lost.
pub const NOTICE_STEP: u8 = 5;

/// When each sandbox's agent was last told about the battery, as the
/// percentage it was told. Forgotten once the Mac is plugged in or charged
/// above the warning level, so the next drop is told again.
#[derive(Default)]
pub struct NoticeBook {
    told: HashMap<String, u8>,
}

impl NoticeBook {
    /// The note to add to `sandbox`'s next prompt, if one is due.
    pub fn due(&mut self, sandbox: &str, battery: Option<Battery>, warn_below: u8, min_battery: u8, mode: KeepAwake) -> Option<String> {
        let b = match battery {
            Some(b) if b.on_battery && b.percent <= warn_below => b,
            _ => {
                self.told.remove(sandbox);
                return None;
            }
        };
        if self.told.get(sandbox).is_some_and(|&last| b.percent + NOTICE_STEP > last) {
            return None;
        }
        self.told.insert(sandbox.to_string(), b.percent);
        Some(notice(b, min_battery, mode))
    }
}

/// What the agent is told. It says what will happen and asks the agent to
/// pass it on: the person is reading claude.ai, not the Mac's menu bar.
pub fn notice(b: Battery, min_battery: u8, mode: KeepAwake) -> String {
    let fate = if mode == KeepAwake::Off {
        "sbxw is not keeping it awake, so it may sleep when idle".to_string()
    } else if b.percent <= min_battery {
        format!("that is at or below {min_battery}%, so sbxw has stopped keeping it awake and it may sleep at any moment")
    } else {
        format!("sbxw keeps it awake until {min_battery}%, then lets it sleep")
    };
    format!(
        "sbxw host notice: the Mac running this sandbox is on battery at {}% — {fate}. \
         When it sleeps, this session goes offline until the Mac wakes. Tell the user \
         this in one line at the start of your reply, then carry on with their request.",
        b.percent
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ON_BATTERY: &str = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=4653155)\t82%; discharging; 5:12 remaining present: true\n";
    const CHARGED: &str = "Now drawing from 'AC Power'\n -InternalBattery-0 (id=4653155)\t100%; charged; 0:00 remaining present: true\n";
    const DESKTOP: &str = "Now drawing from 'AC Power'\n";

    fn bat(percent: u8) -> Option<Battery> {
        Some(Battery { percent, on_battery: true })
    }

    #[test]
    fn pmset_is_read_on_a_laptop_and_absent_on_a_desktop() {
        assert_eq!(parse_pmset(ON_BATTERY), bat(82));
        assert_eq!(parse_pmset(CHARGED), Some(Battery { percent: 100, on_battery: false }));
        assert_eq!(parse_pmset(DESKTOP), None);
        assert_eq!(parse_pmset(""), None);
    }

    #[test]
    fn the_mac_is_let_go_on_battery_at_the_floor_and_never_on_ac() {
        assert!(hold(KeepAwake::Remote, true, bat(6), 5));
        assert!(!hold(KeepAwake::Remote, true, bat(5), 5));
        assert!(hold(KeepAwake::Remote, true, Some(Battery { percent: 2, on_battery: false }), 5));
        assert!(hold(KeepAwake::Remote, true, None, 5));
        assert!(!hold(KeepAwake::Remote, false, None, 5));
        assert!(hold(KeepAwake::Always, false, None, 5));
        assert!(!hold(KeepAwake::Off, true, None, 5));
    }

    #[test]
    fn the_agent_is_told_once_per_step_and_again_after_a_charge() {
        let mut book = NoticeBook::default();
        let due = |book: &mut NoticeBook, b| book.due("neos", b, 20, 5, KeepAwake::Remote);
        assert!(due(&mut book, bat(30)).is_none());
        assert!(due(&mut book, bat(20)).unwrap().contains("20%"));
        assert!(due(&mut book, bat(17)).is_none());
        assert!(due(&mut book, bat(15)).unwrap().contains("keeps it awake until 5%"));
        assert!(due(&mut book, bat(5)).unwrap().contains("stopped keeping it awake"));
        assert!(due(&mut book, Some(Battery { percent: 5, on_battery: false })).is_none());
        assert!(due(&mut book, bat(5)).is_some());
        // Per sandbox: another agent has not been told.
        assert!(book.due("other", bat(5), 20, 5, KeepAwake::Remote).is_some());
    }

    #[test]
    fn caffeinate_dies_with_the_daemon() {
        assert_eq!(caffeinate_args(42), ["-i", "-s", "-w", "42"]);
    }
}
