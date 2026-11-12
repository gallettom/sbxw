//! `template = "auto"`: a template sbxw keeps itself, one per kit list, and
//! the facts that decide when it is stale.
//!
//! A template is a snapshot of a sandbox right after its bring-up: its kits
//! installed, nobody's work in it yet. It goes stale when what is frozen in it
//! changes: the sbx that made the image, or the *content* of its kits' specs
//! (what their startup installed). Not sbxw's version: the tooling sbxw puts in
//! a sandbox is rewritten on every bring-up anyway, so a UI-only release must
//! not cost a rebuild of several minutes. Which kits it holds is its identity,
//! not a staleness: projects with different kits each get their own template.
//!
//! A kit's startup commands run in the background once the container starts,
//! after `sbx create` returns, so "installed" means watching the guest's
//! processes until none of those commands still runs.

use serde::{Deserialize, Serialize};

/// Files a snapshot must not carry: the agent token and Claude OAuth
/// credentials. A sandbox made from the template gets its own at its first
/// tooling step.
pub const SECRETS: [&str; 2] = [crate::tooling::TOKEN_PATH, "/home/agent/.claude/.credentials.json"];

/// What `template` in sbxw.toml asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Off,
    /// A tag the person saved and named.
    Tag(String),
    /// sbxw keeps one per kit list, rebuilt when sbx or sbxw changes.
    Auto,
}

pub const AUTO: &str = "auto";

pub fn mode(template: &str) -> Mode {
    match template.trim() {
        "" => Mode::Off,
        AUTO => Mode::Auto,
        t => Mode::Tag(t.to_string()),
    }
}

/// What a bring-up does about an automatic template.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Choice {
    /// Use it when fresh; otherwise create from sbx's image and say so.
    #[default]
    Default,
    /// Create from sbx's image, then snapshot this sandbox as the new template
    /// and remove the old one.
    Rebuild,
    /// Use it even though it is stale.
    Keep,
    /// Create from sbx's image; leave the template alone.
    Skip,
}

/// One automatic template, as recorded when it was saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub tag: String,
    pub sbx: String,
    pub sbxw: String,
    /// The kit references it was created with, in order: its identity.
    pub kits: Vec<String>,
    /// Seconds since the Unix epoch.
    pub created_at: u64,
    /// [`fingerprint`] of its kits' specs; empty in a record saved before it
    /// was kept, which is then judged on sbxw's version instead.
    #[serde(default)]
    pub content: String,
    /// When a sandbox was last created from it, in seconds since the Unix
    /// epoch; 0 in a record from before this was kept.
    #[serde(default)]
    pub used_at: u64,
}

/// An automatic template no sandbox was created from for this long is
/// removed: each kit list a project ever had keeps one, and a snapshot of a
/// sandbox is large. It costs nothing but time to get back, since a missing
/// one is rebuilt in the background.
pub const UNUSED_AFTER_SECS: u64 = 30 * 86_400;

/// Take out the records nobody used for [`UNUSED_AFTER_SECS`], except `keep`
/// (the one just saved). A record from before `used_at` was kept starts its
/// clock now: when it was last used is not known, so it is not removed on
/// that account. Whether any record changed is the second value.
pub fn take_unused(records: &mut Vec<Record>, now: u64, keep: &str) -> (Vec<Record>, bool) {
    let mut changed = false;
    for r in records.iter_mut().filter(|r| r.used_at == 0) {
        r.used_at = now;
        changed = true;
    }
    let (gone, kept): (Vec<Record>, Vec<Record>) =
        std::mem::take(records).into_iter().partition(|r| r.tag != keep && now.saturating_sub(r.used_at) >= UNUSED_AFTER_SECS);
    *records = kept;
    changed |= !gone.is_empty();
    (gone, changed)
}

/// What a template is judged against: the running versions and the content
/// of the configured kits' specs now.
pub struct Current<'a> {
    pub sbx: &'a str,
    pub sbxw: &'a str,
    pub content: &'a str,
}

/// One hash over every kit spec's text (or the reference, for a kit with no
/// local spec), in order.
pub fn fingerprint(specs: &[String]) -> String {
    format!("{:016x}", fnv1a(&specs.join("\u{0}")))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Freshness {
    Fresh { record: Record },
    /// `changed` says what moved, e.g. `sbx 0.48.0 → 0.49.1`.
    Stale { record: Record, changed: Vec<String> },
    Missing,
}

/// The record for this kit list, judged against the running versions.
pub fn check(records: &[Record], kits: &[String], now: &Current) -> Freshness {
    let Some(r) = records.iter().find(|r| r.kits == kits) else {
        return Freshness::Missing;
    };
    let mut changed = Vec::new();
    if r.sbx != now.sbx {
        changed.push(format!("sbx {} → {}", r.sbx, now.sbx));
    }
    if r.content.is_empty() {
        if r.sbxw != now.sbxw {
            changed.push(format!("sbxw {} → {}", r.sbxw, now.sbxw));
        }
    } else if r.content != now.content {
        changed.push(format!("kit specs changed (sbxw {} → {})", r.sbxw, now.sbxw));
    }
    if changed.is_empty() {
        Freshness::Fresh { record: r.clone() }
    } else {
        Freshness::Stale { record: r.clone(), changed }
    }
}

/// Replace this kit list's record; the one it replaces is returned, so its
/// template can be removed.
pub fn replace(records: &mut Vec<Record>, new: Record) -> Option<Record> {
    let old = records.iter().position(|r| r.kits == new.kits).map(|i| records.remove(i));
    records.push(new);
    old
}

/// `sbxw-auto:<kits hash>-<sbxw>-<sbx>`: lowercase, within Docker's tag
/// alphabet, unique per kit list and version pair.
pub fn auto_tag(kits: &[String], sbxw: &str, sbx: &str) -> String {
    let clean = |v: &str| v.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' { c } else { '-' }).collect::<String>();
    format!("sbxw-auto:{:08x}-{}-{}", fnv1a(&kits.join("\n")) as u32, clean(sbxw), clean(sbx))
}

/// The sandbox the daemon builds this kit list's automatic template in:
/// `sbxw-tpl-<kits hash>`, one per kit list, nobody's to work in.
pub fn builder_name(kits: &[String]) -> String {
    format!("sbxw-tpl-{:08x}", fnv1a(&kits.join("\n")) as u32)
}

/// Stable across builds and platforms, unlike `std`'s hasher.
fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// Every startup command in a kit spec, each argv joined by spaces (as it shows
/// in the guest's process list). Read wherever a `startup:` list sits: v1
/// `commands.startup`, v2 `setup.startup`, v3 `lifecycle@1` config, also inside
/// capability groups.
pub fn startup_commands(spec: &str) -> Vec<String> {
    let Ok(doc) = serde_norway::from_str::<serde_norway::Value>(spec) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    collect_startup(&doc, &mut out);
    out
}

fn collect_startup(v: &serde_norway::Value, out: &mut Vec<String>) {
    use serde_norway::Value;
    match v {
        Value::Mapping(map) => {
            for (k, val) in map {
                if k.as_str() == Some("startup") {
                    if let Value::Sequence(entries) = val {
                        for e in entries {
                            let argv: Vec<&str> = match e.get("command") {
                                Some(Value::Sequence(a)) => a.iter().filter_map(Value::as_str).collect(),
                                Some(Value::String(s)) => vec![s.as_str()],
                                _ => continue,
                            };
                            if !argv.is_empty() {
                                out.push(argv.join(" "));
                            }
                        }
                    }
                } else {
                    collect_startup(val, out);
                }
            }
        }
        Value::Sequence(items) => items.iter().for_each(|i| collect_startup(i, out)),
        _ => {}
    }
}

/// The startup commands still running, given one process command line per
/// line (what [`PROCESS_LIST`] prints). A forked background job keeps its
/// parent's command line, so a script waiting on its downloads still counts.
pub fn still_running<'a>(processes: &str, commands: &'a [String]) -> Vec<&'a str> {
    commands
        .iter()
        .filter(|c| processes.lines().any(|l| l.trim() == c.as_str() || l.trim().starts_with(&format!("{c} "))))
        .map(String::as_str)
        .collect()
}

/// The guest's uptime in seconds on the first line, then one command line
/// per process, from `/proc` — no dependency on `ps`. After [`MARKERS`]: the
/// boot's id, then one `<marker file> <boot id it was written in>` per end
/// marker the kits' startup scripts left (see [`marker_of`]).
pub const PROCESS_LIST: &str = r#"cut -d' ' -f1 /proc/uptime; for p in /proc/[0-9]*; do tr '\0' ' ' < "$p/cmdline" 2>/dev/null; echo; done; echo '#sbxw-markers'; cat /proc/sys/kernel/random/boot_id 2>/dev/null; for m in /home/agent/.sbxw/startup/*.done; do [ -f "$m" ] && echo "$(basename "$m") $(cat "$m" 2>/dev/null)"; done; true"#;

/// The line of [`PROCESS_LIST`]'s output after which the end markers come.
pub const MARKERS: &str = "#sbxw-markers";

/// The end marker a startup command leaves in `~/.sbxw/startup/` when it is
/// over: its script's path (the first absolute argument) with `/` as `_`,
/// then `.done`. sbxw's bundled kits write one, holding the boot's id, from
/// an exit trap; a command with no script path has none.
pub fn marker_of(command: &str) -> Option<String> {
    let script = command.split(' ').skip(1).find(|a| a.starts_with('/'))?;
    Some(format!("{}.done", script.replace('/', "_")))
}

/// The commands whose end marker was written in this boot, read from
/// [`PROCESS_LIST`]'s output. A marker from another boot (one a template
/// carried, or an earlier start) is not this start's.
pub fn finished<'a>(listing: &str, commands: &'a [String]) -> Vec<&'a str> {
    let mut lines = listing.lines().skip_while(|l| l.trim() != MARKERS).skip(1);
    let Some(boot) = lines.next().map(str::trim).filter(|b| !b.is_empty()) else {
        return Vec::new();
    };
    let marks: Vec<&str> = lines.filter_map(|l| l.trim().split_once(' ')).filter(|(_, at)| at.trim() == boot).map(|(m, _)| m).collect();
    commands.iter().filter(|c| marker_of(c).is_some_and(|m| marks.contains(&m.as_str()))).map(String::as_str).collect()
}

/// The uptime [`PROCESS_LIST`] printed first, if it did.
pub fn uptime(processes: &str) -> Option<f64> {
    processes.lines().next()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(kits: &[&str], sbx: &str, sbxw: &str) -> Record {
        Record { tag: auto_tag(&kits.iter().map(|k| k.to_string()).collect::<Vec<_>>(), sbxw, sbx), sbx: sbx.into(), sbxw: sbxw.into(), kits: kits.iter().map(|k| k.to_string()).collect(), created_at: 1, content: "c1".into(), used_at: 0 }
    }

    fn now<'a>(sbx: &'a str, sbxw: &'a str, content: &'a str) -> Current<'a> {
        Current { sbx, sbxw, content }
    }

    #[test]
    fn the_mode_reads_auto_a_tag_or_nothing() {
        assert_eq!(mode(""), Mode::Off);
        assert_eq!(mode(" auto "), Mode::Auto);
        assert_eq!(mode("me/base:1"), Mode::Tag("me/base:1".into()));
    }

    #[test]
    fn a_version_change_makes_it_stale_and_says_which() {
        let kits = vec!["sbxw:codemap".to_string()];
        let records = vec![rec(&["sbxw:codemap"], "0.48.0", "2.1.3")];
        assert!(matches!(check(&records, &kits, &now("0.48.0", "2.1.3", "c1")), Freshness::Fresh { .. }));
        match check(&records, &kits, &now("0.49.1", "2.1.3", "c1")) {
            Freshness::Stale { changed, .. } => assert_eq!(changed, ["sbx 0.48.0 → 0.49.1"]),
            other => panic!("{other:?}"),
        }
        assert_eq!(check(&records, &["other".into()], &now("0.48.0", "2.1.3", "c1")), Freshness::Missing, "another kit list is another template");
        assert!(matches!(check(&records, &kits, &now("0.48.0", "2.1.4", "c1")), Freshness::Fresh { .. }), "a release that leaves the kits alone keeps it");
        match check(&records, &kits, &now("0.48.0", "2.1.4", "c2")) {
            Freshness::Stale { changed, .. } => assert_eq!(changed, ["kit specs changed (sbxw 2.1.3 → 2.1.4)"]),
            other => panic!("{other:?}"),
        }
        let old = vec![Record { content: String::new(), ..records[0].clone() }];
        assert!(matches!(check(&old, &kits, &now("0.48.0", "2.1.4", "c1")), Freshness::Stale { .. }), "a record without content falls back to the version");
    }

    #[test]
    fn replacing_returns_the_old_record_of_the_same_kits_only() {
        let mut records = vec![rec(&["a"], "0.48.0", "2.1.3"), rec(&["b"], "0.48.0", "2.1.3")];
        let old = replace(&mut records, rec(&["a"], "0.49.1", "2.1.3")).unwrap();
        assert_eq!(old.sbx, "0.48.0");
        assert_eq!(records.len(), 2);
        assert!(replace(&mut records, rec(&["c"], "0.49.1", "2.1.3")).is_none());
    }

    #[test]
    fn a_template_unused_for_a_month_is_taken_out_and_an_old_record_starts_its_clock() {
        let day = 86_400;
        let now = 1000 * day;
        let at = |tag: &str, used_at: u64| Record { tag: tag.into(), used_at, ..rec(&[tag], "0.48.0", "2.1.3") };
        let mut records = vec![at("recent", now - 29 * day), at("old", now - 30 * day), at("saved", now - 90 * day), at("unknown", 0)];
        let (gone, changed) = take_unused(&mut records, now, "saved");
        assert_eq!(gone.iter().map(|r| r.tag.as_str()).collect::<Vec<_>>(), ["old"]);
        assert!(changed);
        assert_eq!(records.iter().map(|r| r.tag.as_str()).collect::<Vec<_>>(), ["recent", "saved", "unknown"]);
        assert_eq!(records[2].used_at, now, "its last use is not known: counted from now");
        assert_eq!(take_unused(&mut records, now, "saved"), (vec![], false), "nothing to write the second time");
    }

    #[test]
    fn the_tag_is_a_valid_docker_tag_per_kits_and_versions() {
        let t = auto_tag(&["sbxw:codemap".into()], "2.1.3", "0.49.1");
        assert!(t.starts_with("sbxw-auto:") && t.ends_with("-2.1.3-0.49.1"), "{t}");
        assert!(t.len() <= 128 && t == t.to_lowercase());
        assert_ne!(t, auto_tag(&["sbxw:k8s-tools".into()], "2.1.3", "0.49.1"));
    }

    #[test]
    fn startup_commands_are_found_in_every_spec_version() {
        let v2 = "schemaVersion: \"2\"\nsetup:\n  startup:\n    - command: [\"bash\", \"/home/agent/.local/bin/x\"]\n";
        assert_eq!(startup_commands(v2), ["bash /home/agent/.local/bin/x"]);
        let v3 = "schemaVersion: \"3\"\ncapabilities:\n  - group:\n      capabilities:\n        - type: com.docker.sandbox/lifecycle@1\n          config:\n            startup:\n              - command: [sh, -c, \"echo ready\"]\n";
        assert_eq!(startup_commands(v3), ["sh -c echo ready"]);
        assert!(startup_commands("schemaVersion: \"3\"\ncapabilities: []\n").is_empty());
    }

    #[test]
    fn an_end_marker_of_this_boot_says_a_command_is_over() {
        let cmds = vec!["bash /home/agent/.sbxw/agent-skills/install.sh".to_string(), "node /k/hooks.mjs".to_string(), "sh -c true".to_string()];
        assert_eq!(marker_of(&cmds[0]).as_deref(), Some("_home_agent_.sbxw_agent-skills_install.sh.done"));
        assert_eq!(marker_of(&cmds[2]), None, "no script, no marker");
        let listing = "12.5\n/sbin/init \n#sbxw-markers\nboot-b\n_home_agent_.sbxw_agent-skills_install.sh.done boot-b\n_k_hooks.mjs.done boot-a\n";
        assert_eq!(finished(listing, &cmds), ["bash /home/agent/.sbxw/agent-skills/install.sh"], "another boot's marker does not count");
        assert!(finished("12.5\n/sbin/init \n", &cmds).is_empty(), "an sbx that printed no markers");
        assert_eq!(uptime(listing), Some(12.5));
        // The script that lists them shows in its own listing, on one line.
        assert!(!PROCESS_LIST.contains('\n'));
    }

    #[test]
    fn a_command_still_listed_is_still_running() {
        let cmds = vec!["bash /home/agent/.sbxw/agent-skills/install.sh".to_string(), "node /x.mjs".to_string()];
        let ps = "/sbin/init \nbash /home/agent/.sbxw/agent-skills/install.sh \nsleep 5 \n";
        assert_eq!(still_running(ps, &cmds), ["bash /home/agent/.sbxw/agent-skills/install.sh"]);
        assert!(still_running("node /x.mjs.bak \n", &cmds).is_empty(), "a prefix of another path is not it");
        assert_eq!(uptime("42.17\n/sbin/init \n"), Some(42.17));
        assert_eq!(uptime("/sbin/init \n"), None);
    }
}
