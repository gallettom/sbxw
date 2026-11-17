//! Status pills: an agent writes `[[Done]]` or `[[Failed: 3 tests]]` beside
//! what it describes, and sbxw's terminal views draw the label as a coloured
//! pill. Only the drawing changes; the text keeps its brackets.
//!
//! * The vocabulary is configuration, `[emphasis.words]` in the daemon's
//!   `sbxw.toml`: one entry per family (its main word), with a tone, its other
//!   words (`also`) and one line on when to use it (`use`). The table, when
//!   present, *replaces* the built-in one whole: what the file shows is what
//!   is drawn and what the agent is told.
//! * A tone is a token name (`green`, `danger`, …), never a colour: each UI
//!   maps it to its own palette.
//! * The guidance written into a sandbox ([`guidance`]) is generated from the
//!   same table, so the two never disagree.
//! * The UIs read [`Contract`] (`GET /api/emphasis`) and implement the
//!   matching rule documented in `docs/configuration.md`; the shared cases
//!   are `testdata/emphasis-cases.json`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Longest word or synonym, in characters.
pub const MAX_WORD: usize = 32;
/// Longest `: detail` the UIs draw, in characters.
pub const MAX_DETAIL: usize = 48;
/// Most words (main words and synonyms together) in a vocabulary: each one
/// costs guidance tokens in every session.
pub const MAX_WORDS: usize = 48;
/// Most families.
pub const MAX_FAMILIES: usize = 12;
/// Longest `use` line.
pub const MAX_USE: usize = 80;

/// The tones a family may take, as token names. `danger` is a filled red, for
/// what cannot be undone or costs money; `cyan` is the kind of work.
pub const TONES: &[&str] = &["green", "red", "amber", "blue", "purple", "cyan", "danger"];
/// How a well-formed label that is not in the vocabulary is drawn (when its
/// first letter is upper case; see the matching rule).
pub const UNKNOWN_TONE: &str = "blue";

/// One family of `[emphasis.words]`, keyed by its main word.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Family {
    pub tone: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub also: Vec<String>,
    /// When to use it, one line, for the agent's guidance.
    #[serde(default, rename = "use", skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
}

/// `[emphasis]`. Every field is optional so that a project's `sbxw.toml` can
/// override the switches alone; the vocabulary is read from the daemon's file
/// only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EmphasisSettings {
    /// Draw pills. Default on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Write the guidance into sandboxes. Default on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guidance: Option<bool>,
    /// The vocabulary; `None` is the built-in one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub words: Option<BTreeMap<String, Family>>,
}

const BUILT_IN: &[(&str, &str, &[&str], &str)] = &[
    ("done", "green", &["verified", "fixed", "passed"], "finished and checked"),
    ("failed", "red", &["error", "broken"], "something did not work; say what"),
    ("needs you", "amber", &["blocked", "decide", "question"], "the person must act, answer or choose"),
    ("irreversible", "danger", &["destructive", "costly"], "cannot be undone, or costs money: before doing it"),
    ("note", "blue", &["context", "fyi"], "worth knowing, nothing to do"),
    ("working", "cyan", &["in progress", "next", "plan"], "the kind of work under way or coming next"),
    ("team", "purple", &["lead", "thread", "jarvis"], "from or for the lead, another thread or Jarvis"),
];

/// The built-in vocabulary.
pub fn built_in() -> BTreeMap<String, Family> {
    BUILT_IN
        .iter()
        .map(|(w, tone, also, when)| {
            (
                w.to_string(),
                Family { tone: tone.to_string(), also: also.iter().map(|s| s.to_string()).collect(), when: Some(when.to_string()) },
            )
        })
        .collect()
}

/// The lookup key of a word: lower case, runs of spaces made one.
pub fn key(word: &str) -> String {
    word.split(' ').filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ").to_lowercase()
}

/// A word or synonym: 1 to [`MAX_WORD`] characters, letters, digits, single
/// spaces and `-`, starting with a letter and ending with a letter or digit.
/// The same shape the UIs match, so every configured word can be drawn.
pub fn check_word(word: &str) -> Result<(), String> {
    let n = word.chars().count();
    let ok = (1..=MAX_WORD).contains(&n)
        && word.chars().next().is_some_and(char::is_alphabetic)
        && word.chars().last().is_some_and(char::is_alphanumeric)
        && word.chars().all(|c| c.is_alphanumeric() || c == ' ' || c == '-')
        && !word.contains("  ");
    if !ok {
        return Err(format!(
            "[emphasis.words] '{word}' is not a word: 1 to {MAX_WORD} letters, digits, single spaces or '-', starting with a letter and ending with a letter or digit"
        ));
    }
    Ok(())
}

/// `[emphasis.words]` as [`crate::config::Config::validate`] checks it: a bad
/// entry is refused with its reason, never dropped.
pub fn check_words(words: &BTreeMap<String, Family>) -> Result<(), String> {
    if words.len() > MAX_FAMILIES {
        return Err(format!("[emphasis.words] has {} families: at most {MAX_FAMILIES}", words.len()));
    }
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for (main, f) in words {
        if !TONES.contains(&f.tone.as_str()) {
            return Err(format!("[emphasis.words] '{main}': tone \"{}\" is not one of {}", f.tone, TONES.join(", ")));
        }
        if let Some(u) = &f.when {
            if u.trim().is_empty() || u.chars().count() > MAX_USE || u.chars().any(char::is_control) {
                return Err(format!("[emphasis.words] '{main}': use is one line of 1 to {MAX_USE} characters"));
            }
        }
        for w in std::iter::once(main).chain(f.also.iter()) {
            check_word(w)?;
            if let Some(other) = seen.insert(key(w), main.clone()) {
                return Err(format!("[emphasis.words] '{w}' is listed twice (in '{other}' and '{main}'): a word has one tone"));
            }
        }
    }
    if seen.len() > MAX_WORDS {
        return Err(format!("[emphasis.words] has {} words with their synonyms: at most {MAX_WORDS}", seen.len()));
    }
    Ok(())
}

impl EmphasisSettings {
    pub fn validate(&self) -> Result<(), String> {
        match &self.words {
            Some(w) => check_words(w),
            None => Ok(()),
        }
    }
    pub fn enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }
    pub fn guidance(&self) -> bool {
        self.guidance.unwrap_or(true)
    }
    /// The vocabulary in use: the configured table, else the built-in one.
    pub fn vocabulary(&self) -> BTreeMap<String, Family> {
        self.words.clone().unwrap_or_else(built_in)
    }
}

/// One family as the UIs and the guidance see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FamilyView {
    pub word: String,
    pub tone: String,
    pub also: Vec<String>,
    #[serde(rename = "use")]
    pub when: Option<String>,
}

/// `GET /api/emphasis`, and the `emphasis` event: what a UI needs to draw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Contract {
    /// The daemon-wide switch.
    pub enabled: bool,
    pub guidance: bool,
    pub unknown_tone: &'static str,
    pub tones: &'static [&'static str],
    pub max_word: usize,
    pub max_detail: usize,
    /// In the order of [`BUILT_IN`] for the built-in table, else by main word.
    pub families: Vec<FamilyView>,
    /// Every main word and synonym, by [`key`], to its tone.
    pub words: BTreeMap<String, String>,
    /// The effective switch of every sandbox sbxw knows, by its text form.
    pub sandboxes: BTreeMap<String, bool>,
}

/// The families of `vocab`, built-in order first (so the guidance reads
/// done, failed, needs you, …), then the others by name.
pub fn families(vocab: &BTreeMap<String, Family>) -> Vec<FamilyView> {
    let rank = |w: &str| BUILT_IN.iter().position(|(b, ..)| *b == w).unwrap_or(usize::MAX);
    let mut out: Vec<FamilyView> = vocab
        .iter()
        .map(|(w, f)| FamilyView { word: w.clone(), tone: f.tone.clone(), also: f.also.clone(), when: f.when.clone() })
        .collect();
    out.sort_by(|a, b| rank(&a.word).cmp(&rank(&b.word)).then_with(|| a.word.cmp(&b.word)));
    out
}

/// The contract from the daemon's settings and each sandbox's own override
/// (`None`: it follows the daemon's switch).
pub fn contract(daemon: &EmphasisSettings, sandboxes: &[(String, Option<bool>)]) -> Contract {
    let vocab = daemon.vocabulary();
    let mut words = BTreeMap::new();
    for (main, f) in &vocab {
        for w in std::iter::once(main).chain(f.also.iter()) {
            words.insert(key(w), f.tone.clone());
        }
    }
    Contract {
        enabled: daemon.enabled(),
        guidance: daemon.guidance(),
        unknown_tone: UNKNOWN_TONE,
        tones: TONES,
        max_word: MAX_WORD,
        max_detail: MAX_DETAIL,
        families: families(&vocab),
        words,
        sandboxes: sandboxes.iter().map(|(n, o)| (n.clone(), o.unwrap_or(daemon.enabled()))).collect(),
    }
}

/// Whether a sandbox draws pills and gets the guidance: its own file's
/// switches when set, else the daemon's.
pub fn effective(daemon: &EmphasisSettings, own: Option<&EmphasisSettings>) -> (bool, bool) {
    let enabled = own.and_then(|o| o.enabled).unwrap_or(daemon.enabled());
    let guidance = own.and_then(|o| o.guidance).unwrap_or(daemon.guidance());
    (enabled, guidance)
}

/// The heading the guidance opens with.
pub const GUIDANCE_HEAD: &str = "## Status labels";

fn label(word: &str) -> String {
    let mut c = word.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// What the agent reads in `CLAUDE.md`: what a label is, the words by family
/// and the limits. Generated from `vocab`, the same table the UIs draw from.
pub fn guidance(vocab: &BTreeMap<String, Family>) -> String {
    let fams = families(vocab);
    let first = fams.first().map(|f| label(&f.word)).unwrap_or_else(|| "Word".into());
    let mut s = format!(
        "{GUIDANCE_HEAD}\n\n\
         The person's terminal draws `[[Word]]` or `[[Word: detail]]` as a coloured pill. \
         Put one right beside the thing it describes, e.g. `Build [[{first}]]` or `[[{first}: 12 of 12]]`. \
         The word sets the colour:\n\n"
    );
    for f in fams {
        s.push_str(&format!("- [[{}]]", label(&f.word)));
        if !f.also.is_empty() {
            s.push_str(&format!(" (or {})", f.also.iter().map(|w| label(w)).collect::<Vec<_>>().join(", ")));
        }
        if let Some(u) = &f.when {
            s.push_str(&format!(": {u}"));
        }
        s.push_str(".\n");
    }
    s.push_str(
        "\nAt most three labels per reply, only where a status helps the person scan. \
         Never inside code, commands, file contents, commit messages, or anything written to disk or sent to a tool. \
         Plain words everywhere else.\n",
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_vocabulary_is_valid_and_short() {
        let v = built_in();
        check_words(&v).unwrap();
        let n: usize = v.values().map(|f| 1 + f.also.len()).sum();
        assert!((20..=25).contains(&n), "{n} words");
        assert!((6..=7).contains(&v.len()));
    }

    #[test]
    fn a_bad_entry_is_refused_with_its_reason() {
        let fam = |tone: &str, also: &[&str]| Family { tone: tone.into(), also: also.iter().map(|s| s.to_string()).collect(), when: None };
        let one = |w: &str, f: Family| BTreeMap::from([(w.to_string(), f)]);
        assert!(check_words(&one("done", fam("pink", &[]))).unwrap_err().contains("tone \"pink\""));
        assert!(check_words(&one("done", fam("green", &["a]]b"]))).unwrap_err().contains("is not a word"));
        assert!(check_words(&one(&"x".repeat(33), fam("green", &[]))).unwrap_err().contains("is not a word"));
        assert!(check_words(&one("1st", fam("green", &[]))).is_err(), "starts with a letter");
        assert!(check_words(&one("a  b", fam("green", &[]))).is_err(), "single spaces");
        assert!(check_words(&one("done", fam("green", &["Done"]))).unwrap_err().contains("listed twice"));
        let many: BTreeMap<String, Family> = (0..13).map(|i| (format!("w{i}"), fam("blue", &[]))).collect();
        assert!(check_words(&many).unwrap_err().contains("at most 12"));
        let wide: BTreeMap<String, Family> =
            (0..5).map(|i| (format!("w{i}"), fam("blue", &(0..10).map(|j| format!("s{i}x{j}")).collect::<Vec<_>>().iter().map(String::as_str).collect::<Vec<_>>()))).collect();
        assert!(check_words(&wide).unwrap_err().contains("at most 48"));
        let bad_use = BTreeMap::from([("done".to_string(), Family { tone: "green".into(), also: vec![], when: Some("a\nb".into()) })]);
        assert!(check_words(&bad_use).unwrap_err().contains("one line"));
        check_words(&one("Ça va", fam("cyan", &["prêt-à-porter2"]))).unwrap();
    }

    #[test]
    fn the_contract_resolves_synonyms_and_each_sandbox_s_switch() {
        let off = EmphasisSettings { enabled: Some(false), ..Default::default() };
        let c = contract(&EmphasisSettings::default(), &[("a".into(), None), ("b".into(), Some(false))]);
        assert!(c.enabled && c.guidance);
        assert_eq!(c.words["verified"], "green");
        assert_eq!(c.words["in progress"], "cyan");
        assert_eq!(c.words["irreversible"], "danger");
        assert_eq!(c.families[0].word, "done");
        assert_eq!(c.sandboxes, BTreeMap::from([("a".into(), true), ("b".into(), false)]));
        let c = contract(&off, &[("a".into(), None), ("b".into(), Some(true))]);
        assert_eq!(c.sandboxes, BTreeMap::from([("a".into(), false), ("b".into(), true)]));
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["families"][0]["use"], "finished and checked");
        assert_eq!(v["unknown_tone"], "blue");
        assert_eq!(effective(&off, Some(&EmphasisSettings { guidance: Some(false), ..Default::default() })), (false, false));
        assert_eq!(effective(&EmphasisSettings::default(), None), (true, true));
    }

    /// The web matcher (assets/js/emphasis.js) against the shared cases,
    /// with this crate's contract for the built-in vocabulary. Skipped
    /// without node, as the tooling tests are.
    #[test]
    fn the_web_matcher_passes_the_shared_cases() {
        if std::process::Command::new("node").arg("--version").output().is_err() {
            return;
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("contract.json");
        std::fs::write(&file, serde_json::to_vec(&contract(&EmphasisSettings::default(), &[])).unwrap()).unwrap();
        let out = std::process::Command::new("node").arg(root.join("scripts/test-emphasis.mjs")).arg(&file).output().unwrap();
        assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    }

    #[test]
    fn the_guidance_is_generated_from_the_vocabulary_and_stays_small() {
        let g = guidance(&built_in());
        assert!(g.starts_with(GUIDANCE_HEAD));
        assert!(g.contains("- [[Done]] (or Verified, Fixed, Passed): finished and checked.\n"), "{g}");
        assert!(g.contains("[[Needs you]]"));
        assert!(g.find("[[Done]]").unwrap() < g.find("[[Team]]").unwrap());
        assert!(g.contains("Never inside code"));
        // About four characters a token: well under 400 tokens.
        assert!(g.len() < 1400, "{} bytes:\n{g}", g.len());
        let custom = BTreeMap::from([("shipped".to_string(), Family { tone: "green".into(), also: vec!["live".into()], when: None })]);
        let g = guidance(&custom);
        assert!(g.contains("- [[Shipped]] (or Live).\n"), "{g}");
        assert!(!g.contains("[[Done]]"));
    }
}
