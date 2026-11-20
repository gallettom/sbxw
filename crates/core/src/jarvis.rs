//! Jarvis: a Claude session in the daemon's own sandbox ([`SANDBOX`]) that
//! *proposes* changes to sbxw's configuration. The daemon checks each
//! proposal, the person approves it in the web UI, and only then the daemon
//! writes it.
//!
//! Everything here is decided from data: [`World`] is what the daemon read
//! (the sbxw.toml files it uses, the Jarvis kits, the MCP servers), and
//! [`plan`] turns a list of [`Change`]s into what would be written, with a
//! diff the person reads, or a precise refusal the agent can act on. The
//! daemon plans again at approval against the files as they are then: when
//! a diff differs from the one the person read, nothing is applied.
//!
//! What Jarvis can never do, by construction: choose a path on the host
//! (files are named by key, kits live under `state/kits/jarvis-*`), write a
//! kit's `spec.yaml` (sbxw renders it: image content and a `chmod` hook, no
//! install command, no network rule), set a setting outside [`SETTINGS`], or
//! hand sbx a secret (refused; the person adds them in the MCP dialog).

use crate::anonymise;
use crate::config::Config;
use crate::emphasis::{self, EmphasisSettings, Family};
use crate::kits;
use crate::mcp::McpSpec;
use crate::my_skills::{self, KitFile};
use crate::spoken::{self, Said};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// The daemon-owned sandbox Jarvis runs in, on the local host only: the
/// one caller the `/api/jarvis/*` agent routes answer.
pub const SANDBOX: &str = "sbxw-jarvis";

/// Every Jarvis kit's name starts with this; its directory is
/// `state/kits/<name>/`.
pub const KIT_PREFIX: &str = "jarvis-";

/// The `kits_line` file key of the daemon's own sbxw.toml; any other key is
/// a sandbox name.
pub const DAEMON_FILE: &str = "daemon";

pub const MAX_CHANGES: usize = 20;
pub const MAX_SUMMARY: usize = 2000;
pub const MAX_CONTEXT: usize = 16 * 1024;
pub const MAX_DESCRIPTION: usize = 200;
/// Settled proposals kept for the dialog's history.
pub const KEEP_SETTLED: usize = 50;

pub const DEFAULT_WAIT_SECS: u64 = 90;
pub const MAX_WAIT_SECS: u64 = 600;

/// How long `propose` and `wait` park.
pub fn wait_bound(requested: Option<u64>) -> std::time::Duration {
    std::time::Duration::from_secs(requested.unwrap_or(DEFAULT_WAIT_SECS).clamp(1, MAX_WAIT_SECS))
}

/// `jarvis-` and 1 to 40 of lowercase letters, digits and `-`.
pub fn valid_kit(name: &str) -> bool {
    name.strip_prefix(KIT_PREFIX).is_some_and(|slug| {
        (1..=40).contains(&slug.len()) && slug.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    })
}

/// Where a kit file may land under `/home/agent/`: relative, no `..`, the
/// character set of kept skills, and never sbxw's own files or Claude
/// Code's settings (hooks and permissions are not a kit's to change).
pub fn check_path(path: &str) -> Result<(), String> {
    if !my_skills::valid_path(path) {
        return Err(format!(
            "'{}': a kit file is a path relative to /home/agent/ (letters, digits, . _ - + @ and spaces, no '..', no leading '/')",
            path.chars().take(120).collect::<String>()
        ));
    }
    let forbidden = path == ".sbxw"
        || path.starts_with(".sbxw/")
        || path == ".claude.json"
        || path == ".claude/settings.json"
        || path == ".claude/settings.local.json";
    if forbidden {
        return Err(format!("'{path}': nothing under .sbxw/, nor .claude/settings.json, .claude/settings.local.json or .claude.json, is a kit's to write"));
    }
    // Code that runs without anyone invoking it, or that stands in for the
    // agent: a kit carries no startup command chosen by Jarvis, so none
    // reaches the sandbox by another door. Compared on the validated path,
    // so a look-alike deeper down (`.claude/skills/x/.bashrc`) is data.
    if SHELL_START_FILES.contains(&path) {
        return Err(format!("'{path}': a shell start file runs at every login, which a kit's file may not do; put a script under bin/ and a slash command or the kit's context that says when to run it"));
    }
    if path.strip_prefix(".local/bin/").is_some_and(|rest| rest.rsplit('/').next() == Some("claude")) {
        return Err(format!("'{path}': a kit may not replace the agent itself"));
    }
    if path == ".ssh" || path.starts_with(".ssh/") {
        return Err(format!("'{path}': nothing under .ssh/ is a kit's to write; SSH keys reach a sandbox through the sbxw:ssh-agent kit, added by the person"));
    }
    if path == ".claude/CLAUDE.md" {
        return Err(format!("'{path}': sbx writes the agent's CLAUDE.md; use the kit's `context` field for text the agent should read"));
    }
    if path == ".claude/hooks" || path.starts_with(".claude/hooks/") {
        return Err(format!("'{path}': hooks run without anyone invoking them, which a kit Jarvis writes may not carry"));
    }
    Ok(())
}

/// Shell start files at the home root: run at every login or shell.
/// `.inputrc` is not one: readline reads it as data.
const SHELL_START_FILES: &[&str] = &[".bashrc", ".bash_profile", ".bash_login", ".bash_logout", ".profile", ".zshrc", ".zshenv", ".zprofile"];

/// Bundled kits that hand the sandbox something of the person's (their SSH
/// agent, their Git identity), a grant (a port published without a
/// prompt), or a machine (the virtual Mac): adding one is high risk, a
/// click in the dialog. Every other bundled kit is in [`TOOL_KITS`]; a test
/// checks each one of [`kits::BUNDLED`] is in exactly one list, so a new
/// kit is never low risk by default.
pub const PERSONAL_KITS: &[&str] = &["ssh-agent", "git-identity", "dev-ports", "terminal-mac"];

/// Bundled kits that add tools and skills only: adding one is low risk.
pub const TOOL_KITS: &[&str] = &["codemap", "agent-skills", "md-to-pdf-tools", "retro", "k8s-tools", "firecrawl"];

/// The risk of adding bundled kit `name`: low only when it is known to add
/// tools alone.
pub fn bundled_risk(name: &str) -> Risk {
    if TOOL_KITS.contains(&name) && !PERSONAL_KITS.contains(&name) {
        Risk::Low
    } else {
        Risk::High
    }
}

/// One file of a kit as Jarvis writes it: relative to `/home/agent/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KitFileIn {
    pub path: String,
    pub content: String,
}

/// An MCP server as a proposal carries it: the MCP dialog's form, stored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpServerSpec {
    pub name: String,
    /// `remote` or `stdio`.
    #[serde(alias = "type")]
    pub kind: String,
    pub url: String,
    pub headers: Vec<String>,
    pub command: String,
    pub args: Vec<String>,
    pub dir: String,
}

impl McpServerSpec {
    pub fn to_spec(&self) -> McpSpec {
        McpSpec {
            name: self.name.clone(),
            kind: self.kind.clone(),
            url: self.url.clone(),
            headers: self.headers.clone(),
            command: self.command.clone(),
            args: self.args.clone(),
            dir: self.dir.clone(),
        }
    }
}

/// One change a proposal asks for. On the wire: `{"kind": "kit_write", …}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Change {
    /// Create or update a Jarvis kit: `files` are written (replacing a file
    /// of the same path), `remove` deleted, every other file kept.
    KitWrite {
        kit: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        files: Vec<KitFileIn>,
        #[serde(default)]
        remove: Vec<String>,
        /// Text for the agent's CLAUDE.md (`agent-context@2`); `""` removes it.
        #[serde(default)]
        context: Option<String>,
    },
    KitRemove { kit: String },
    /// The `kits` line of an sbxw.toml the daemon uses: [`DAEMON_FILE`] or a sandbox name.
    KitsLine {
        file: String,
        #[serde(default)]
        add: Vec<String>,
        #[serde(default)]
        remove: Vec<String>,
    },
    /// One of [`SETTINGS`] in the daemon's sbxw.toml; `null` removes the key.
    Setting { key: String, value: Value },
    /// One word of `[voice.words]` in the daemon's sbxw.toml: how it is said
    /// aloud. `fr` and `en` both absent removes the configured entry (a
    /// built-in word then comes back).
    VoiceWord {
        word: String,
        #[serde(default)]
        fr: Option<String>,
        #[serde(default)]
        en: Option<String>,
    },
    /// One family of `[emphasis.words]` (status pills) in the daemon's
    /// sbxw.toml, whole: its main word, tone, other words and `use` line.
    /// No `tone` removes the family. The first change copies the built-in
    /// vocabulary into the file, since a configured table replaces it.
    EmphasisWord {
        word: String,
        #[serde(default)]
        tone: Option<String>,
        #[serde(default)]
        also: Vec<String>,
        #[serde(default, rename = "use")]
        when: Option<String>,
    },
    /// `[emphasis]`'s switches in an sbxw.toml the daemon uses:
    /// [`DAEMON_FILE`] (the default) or a sandbox name (its own file).
    EmphasisSwitch {
        #[serde(default)]
        file: Option<String>,
        #[serde(default)]
        enabled: Option<bool>,
        #[serde(default)]
        guidance: Option<bool>,
    },
    /// The daemon config's `network_allow`.
    NetworkAllow {
        #[serde(default)]
        add: Vec<String>,
        #[serde(default)]
        remove: Vec<String>,
    },
    McpServer { spec: McpServerSpec },
    McpRemove { name: String },
    /// One screen of the desktop app, whole: written as `<name>.luau` in the
    /// app's override folder, which the app uses instead of its built-in.
    /// `name` must be a built-in screen's.
    DesktopScreen { name: String, content: String },
    /// Delete the override of screen `name`: the app's built-in comes back.
    DesktopScreenRemove { name: String },
}

impl Change {
    pub fn kind(&self) -> &'static str {
        match self {
            Change::KitWrite { .. } => "kit_write",
            Change::KitRemove { .. } => "kit_remove",
            Change::KitsLine { .. } => "kits_line",
            Change::Setting { .. } => "setting",
            Change::VoiceWord { .. } => "voice_word",
            Change::EmphasisWord { .. } => "emphasis_word",
            Change::EmphasisSwitch { .. } => "emphasis_switch",
            Change::NetworkAllow { .. } => "network_allow",
            Change::McpServer { .. } => "mcp_server",
            Change::McpRemove { .. } => "mcp_remove",
            Change::DesktopScreen { .. } => "desktop_screen",
            Change::DesktopScreenRemove { .. } => "desktop_screen_remove",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    #[default]
    Low,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Pending,
    Applied,
    Refused,
    Failed,
}

// ---------------------------------------------------------------------------
// Settings

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingType {
    Text,
    Number,
}

/// The settings Jarvis may change, and their type. Every one is read by the
/// daemon at start: a change takes effect at its next restart.
pub const SETTINGS: &[(&str, SettingType)] = &[
    ("voice.stt", SettingType::Text),
    ("voice.tts", SettingType::Text),
    ("voice.tts_voice", SettingType::Text),
    ("voice.push_to_talk_key", SettingType::Text),
    ("voice.dictate_key", SettingType::Text),
    ("claude_model", SettingType::Text),
    ("terminal_color", SettingType::Text),
    ("terminal_bar_color", SettingType::Text),
    ("terminal_text_color", SettingType::Text),
    ("keep_awake", SettingType::Text),
    ("keep_awake_min_battery", SettingType::Number),
    ("battery_warn_below", SettingType::Number),
    ("cache_ttl_days", SettingType::Number),
];

fn setting_type(key: &str) -> Option<SettingType> {
    SETTINGS.iter().find(|(k, _)| *k == key).map(|(_, t)| *t)
}

/// What a setting accepts: a closed list, or a hint when the list is open.
pub fn allowed_values(key: &str, models: &[String]) -> Value {
    use crate::voice::{Role, CATALOG, VOICES};
    let role = |r: Role| CATALOG.iter().filter(|m| m.role == r).map(|m| m.id).collect::<Vec<&'static str>>();
    match key {
        "voice.stt" => json!(role(Role::Stt)),
        "voice.tts" => json!(role(Role::Tts)),
        "voice.tts_voice" => json!(VOICES),
        "keep_awake" => json!(["off", "remote", "always"]),
        "claude_model" => json!(models),
        "voice.push_to_talk_key" | "voice.dictate_key" => json!("a KeyboardEvent.code such as F8; the two keys differ"),
        "terminal_color" | "terminal_bar_color" => json!("#rrggbb, or null for the default"),
        "terminal_text_color" => json!("#rrggbb, auto, or null for the default"),
        "keep_awake_min_battery" | "battery_warn_below" => json!("a percentage, 0 to 100"),
        "cache_ttl_days" => json!("days, 1 to 365"),
        _ => Value::Null,
    }
}

/// The value `cfg` has for `key` (one of [`SETTINGS`]).
pub fn current_value(cfg: &Config, key: &str) -> Value {
    let v = serde_json::to_value(cfg).unwrap_or_default();
    v.pointer(&format!("/{}", key.replace('.', "/"))).cloned().unwrap_or(Value::Null)
}

/// `raw` (an sbxw.toml) with `key` set to `value` (`null`: removed), checked
/// as Config parsing checks it. Comments and order are kept.
pub fn set_setting(raw: &str, key: &str, value: &Value, models: &[String]) -> Result<String, String> {
    let Some(ty) = setting_type(key) else {
        return Err(format!("'{key}' is not a setting Jarvis may change; the ones it may: {}", SETTINGS.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", ")));
    };
    let item = match (ty, value) {
        (_, Value::Null) => None,
        (SettingType::Text, Value::String(s)) => {
            let s = s.trim();
            if s.len() > 200 || s.contains(['\n', '\r', '\0']) {
                return Err(format!("{key}: one line of at most 200 characters"));
            }
            Some(toml_edit::value(s))
        }
        (SettingType::Number, Value::Number(n)) => match n.as_i64() {
            Some(i) if i >= 0 => Some(toml_edit::value(i)),
            _ => return Err(format!("{key}: a whole number, 0 or more")),
        },
        (SettingType::Text, _) => return Err(format!("{key}: expected a string")),
        (SettingType::Number, _) => return Err(format!("{key}: expected a number")),
    };
    if let (Some(v), "voice.push_to_talk_key" | "voice.dictate_key") = (value.as_str(), key) {
        if v.trim().is_empty() || v.trim().len() > 24 || !v.trim().chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(format!("{key}: a KeyboardEvent.code such as F8 (letters and digits)"));
        }
    }
    if let (Some(v), "claude_model") = (value.as_str(), key) {
        if !models.iter().any(|m| m == v.trim()) {
            return Err(format!("claude_model: '{}' is not one of the models sbxw offers ({})", v.trim(), models.join(", ")));
        }
    }
    if let (Some(n), "cache_ttl_days") = (value.as_u64(), key) {
        if !(1..=365).contains(&n) {
            return Err("cache_ttl_days: 1 to 365".into());
        }
    }
    let mut doc: toml_edit::DocumentMut = raw.parse().map_err(|e| format!("the daemon's sbxw.toml is not valid TOML: {e}"))?;
    match key.split_once('.') {
        Some((table, leaf)) => match item {
            Some(item) => {
                if !doc.contains_key(table) {
                    doc[table] = toml_edit::table();
                }
                let Some(t) = doc[table].as_table_like_mut() else {
                    return Err(format!("[{table}] in the daemon's sbxw.toml is not a table"));
                };
                t.insert(leaf, item);
            }
            None => {
                if let Some(t) = doc.get_mut(table).and_then(|t| t.as_table_like_mut()) {
                    t.remove(leaf);
                }
            }
        },
        None => match item {
            Some(item) => doc[key] = item,
            None => {
                doc.remove(key);
            }
        },
    }
    let next = doc.to_string();
    Config::parse(&next).map_err(|e| format!("{key}: {e:#}"))?;
    Ok(next)
}

// ---------------------------------------------------------------------------
// Voice words

/// `raw` (an sbxw.toml) with `word` in `[voice.words]` said as `said`
/// (`None`: its entry removed), and the table after it. The word is written
/// in lower case and replaces an entry of the same word whatever its case.
/// Checked as Config parsing checks it; comments and order are kept.
pub fn set_voice_word(raw: &str, word: &str, said: Option<&Said>) -> Result<(String, BTreeMap<String, Said>), String> {
    let word = word.trim();
    spoken::check_word(word)?;
    let lower = word.to_lowercase();
    if let Some(said) = said {
        for (lang, s) in [("fr", &said.fr), ("en", &said.en)] {
            if let Some(s) = s {
                spoken::check_said(word, lang, s)?;
            }
        }
    }
    let mut doc: toml_edit::DocumentMut = raw.parse().map_err(|e| format!("the daemon's sbxw.toml is not valid TOML: {e}"))?;
    if !doc.contains_key("voice") {
        let mut t = toml_edit::Table::new();
        t.set_implicit(true);
        doc["voice"] = toml_edit::Item::Table(t);
    }
    let Some(voice) = doc["voice"].as_table_like_mut() else {
        return Err("[voice] in the daemon's sbxw.toml is not a table".into());
    };
    if voice.get("words").is_none() {
        if said.is_none() {
            return Err(format!("'{lower}' has no entry in [voice.words], so there is none to remove"));
        }
        voice.insert("words", toml_edit::table());
    }
    let Some(words) = voice.get_mut("words").and_then(|w| w.as_table_like_mut()) else {
        return Err("[voice.words] in the daemon's sbxw.toml is not a table".into());
    };
    let existing: Vec<String> = words.iter().map(|(k, _)| k.to_string()).filter(|k| k.to_lowercase() == lower).collect();
    match said {
        Some(said) => {
            let mut entry = toml_edit::InlineTable::new();
            for (lang, s) in [("fr", &said.fr), ("en", &said.en)] {
                if let Some(s) = s {
                    entry.insert(lang, s.trim().into());
                }
            }
            // An entry already in lower case is replaced in place.
            for k in existing.iter().filter(|k| **k != lower) {
                words.remove(k);
            }
            words.insert(&lower, toml_edit::value(entry));
        }
        None => {
            if existing.is_empty() {
                return Err(format!("'{lower}' has no entry in [voice.words], so there is none to remove"));
            }
            for k in &existing {
                words.remove(k);
            }
        }
    }
    let empty = words.is_empty();
    if empty {
        voice.remove("words");
    }
    let next = doc.to_string();
    let cfg = Config::parse(&next).map_err(|e| format!("{e:#}"))?;
    Ok((next, cfg.voice.words))
}

// ---------------------------------------------------------------------------
// Status pills

/// Low when the change only removes words, recolours or rewords nothing the
/// agents read anew; high when it puts new text into every sandbox's
/// `CLAUDE.md` (a new word or synonym, a new or changed `use` line): that
/// text is an instruction every agent reads, as a kit's context is.
pub fn emphasis_risk(before: &BTreeMap<String, Family>, after: &BTreeMap<String, Family>) -> Risk {
    let words = |v: &BTreeMap<String, Family>| -> std::collections::BTreeSet<String> {
        v.iter().flat_map(|(m, f)| std::iter::once(m.clone()).chain(f.also.iter().cloned())).map(|w| emphasis::key(&w)).collect()
    };
    let old = words(before);
    let new_word = words(after).iter().any(|w| !old.contains(w));
    let new_use = after.iter().any(|(m, f)| f.when.is_some() && before.get(m).and_then(|b| b.when.as_ref()) != f.when.as_ref());
    if new_word || new_use {
        Risk::High
    } else {
        Risk::Low
    }
}

fn family_item(f: &Family) -> toml_edit::Item {
    let mut t = toml_edit::InlineTable::new();
    t.insert("tone", f.tone.as_str().into());
    if !f.also.is_empty() {
        t.insert("also", toml_edit::Value::Array(f.also.iter().map(String::as_str).collect()));
    }
    if let Some(u) = &f.when {
        t.insert("use", u.as_str().into());
    }
    toml_edit::value(t)
}

fn emphasis_table(doc: &mut toml_edit::DocumentMut) -> Result<&mut dyn toml_edit::TableLike, String> {
    if !doc.contains_key("emphasis") {
        let mut t = toml_edit::Table::new();
        t.set_implicit(true);
        doc["emphasis"] = toml_edit::Item::Table(t);
    }
    doc["emphasis"].as_table_like_mut().ok_or_else(|| "[emphasis] in the sbxw.toml is not a table".to_string())
}

/// `raw` (the daemon's sbxw.toml) with the family `word` of
/// `[emphasis.words]` set to `family` (`None`: removed), and the settings
/// after it. When the file has no table yet, the built-in vocabulary is
/// written first, since a configured table replaces it whole. Checked as
/// Config parsing checks it; comments and order are kept.
pub fn set_emphasis_word(raw: &str, word: &str, family: Option<&Family>) -> Result<(String, EmphasisSettings), String> {
    let word = word.trim();
    emphasis::check_word(word)?;
    let main = emphasis::key(word);
    let mut doc: toml_edit::DocumentMut = raw.parse().map_err(|e| format!("the daemon's sbxw.toml is not valid TOML: {e}"))?;
    let em = emphasis_table(&mut doc)?;
    if em.get("words").is_none() {
        let mut t = toml_edit::Table::new();
        for f in emphasis::families(&emphasis::built_in()) {
            t.insert(&f.word, family_item(&Family { tone: f.tone, also: f.also, when: f.when }));
        }
        em.insert("words", toml_edit::Item::Table(t));
    }
    let Some(words) = em.get_mut("words").and_then(|w| w.as_table_like_mut()) else {
        return Err("[emphasis.words] in the daemon's sbxw.toml is not a table".into());
    };
    let existing: Vec<String> = words.iter().map(|(k, _)| k.to_string()).filter(|k| emphasis::key(k) == main).collect();
    match family {
        Some(f) => {
            for k in existing.iter().filter(|k| **k != main) {
                words.remove(k);
            }
            words.insert(&main, family_item(f));
        }
        None => {
            if existing.is_empty() {
                return Err(format!("'{main}' is not a family of the status labels, so there is none to remove"));
            }
            for k in &existing {
                words.remove(k);
            }
        }
    }
    let next = doc.to_string();
    let cfg = Config::parse(&next).map_err(|e| format!("{e:#}"))?;
    Ok((next, cfg.emphasis))
}

/// `raw` with `[emphasis]`'s `enabled` and `guidance` set where given.
pub fn set_emphasis_switch(raw: &str, enabled: Option<bool>, guidance: Option<bool>) -> Result<(String, EmphasisSettings), String> {
    let mut doc: toml_edit::DocumentMut = raw.parse().map_err(|e| format!("the sbxw.toml is not valid TOML: {e}"))?;
    let em = emphasis_table(&mut doc)?;
    for (k, v) in [("enabled", enabled), ("guidance", guidance)] {
        if let Some(v) = v {
            em.insert(k, toml_edit::value(v));
        }
    }
    let next = doc.to_string();
    let cfg = Config::parse(&next).map_err(|e| format!("{e:#}"))?;
    Ok((next, cfg.emphasis))
}

// ---------------------------------------------------------------------------
// Kits

/// A Jarvis kit as sbxw keeps it: what it renders from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct JarvisKit {
    pub description: String,
    pub context: Option<String>,
    /// Path order.
    pub files: Vec<KitFileIn>,
}

pub const DEFAULT_DESCRIPTION: &str = "A kit Jarvis proposed and you approved in sbxw";

/// A rendered file is made executable in the sandbox when it starts with `#!`.
fn is_script(content: &str) -> bool {
    content.starts_with("#!")
}

/// The kit directory's files: a v3 `spec.yaml` (image content from
/// `files/home/agent/`, a `chmod` startup hook for scripts, an optional
/// `agent-context@2`), and that tree. Every scalar the agent supplied is
/// JSON-quoted, which YAML reads as written.
pub fn render_kit(name: &str, kit: &JarvisKit) -> Vec<KitFile> {
    let q = |s: &str| serde_json::to_string(s).unwrap_or_default();
    let mut files: Vec<&KitFileIn> = kit.files.iter().collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let mut spec = format!(
        "# syntax=docker/sandbox-kit:3\n# Written by sbxw from a proposal you approved; rewritten at the next one.\nschemaVersion: \"3\"\nkind: mixin\ndisplayName: {name}\ndescription: {}\n",
        q(&kit.description)
    );
    if !files.is_empty() {
        spec.push_str("build: |\n  FROM scratch\n  COPY --chown=1000:1000 files/home/agent/ /home/agent/\n");
    }
    let scripts: Vec<String> = files.iter().filter(|f| is_script(&f.content)).map(|f| format!("/home/agent/{}", f.path)).collect();
    let context = kit.context.as_deref().filter(|c| !c.trim().is_empty());
    if !scripts.is_empty() || context.is_some() {
        spec.push_str("capabilities:\n");
    }
    if !scripts.is_empty() {
        let argv: Vec<String> = ["chmod", "0755"].iter().map(|s| s.to_string()).chain(scripts).collect();
        spec.push_str(&format!(
            "  - type: com.docker.sandbox/lifecycle@1\n    config:\n      startup:\n        - command: {}\n          description: Make the kit's scripts executable\n",
            serde_json::to_string(&argv).unwrap_or_default()
        ));
    }
    if let Some(c) = context {
        spec.push_str(&format!("  - type: com.docker.sandbox/agent-context@2\n    optional: true\n    config:\n      content: {}\n", q(c)));
    }
    let mut out = vec![KitFile { rel: "spec.yaml".into(), executable: false, content: spec }];
    for f in files {
        out.push(KitFile { rel: format!("files/home/agent/{}", f.path), executable: false, content: f.content.clone() });
    }
    out
}

/// The description and context back from a `spec.yaml` [`render_kit`] wrote.
pub fn parse_spec(spec: &str) -> (String, Option<String>) {
    let doc: serde_norway::Value = serde_norway::from_str(spec).unwrap_or(serde_norway::Value::Null);
    let description = doc["description"].as_str().unwrap_or(DEFAULT_DESCRIPTION).to_string();
    let context = doc["capabilities"]
        .as_sequence()
        .into_iter()
        .flatten()
        .find(|c| c["type"].as_str() == Some("com.docker.sandbox/agent-context@2"))
        .and_then(|c| c["config"]["content"].as_str().map(String::from));
    (description, context)
}

fn check_text(what: &str, text: &str, max: usize) -> Result<(), String> {
    if text.len() > max {
        return Err(format!("{what}: at most {max} bytes"));
    }
    if text.contains("${{") {
        return Err(format!("{what}: '${{{{' is expanded by sbx; write it another way"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Secrets in MCP specs

/// Header names whose value is a credential whatever it looks like.
fn credential_header(name: &str) -> bool {
    let n = name.trim().to_ascii_lowercase();
    ["authorization", "cookie"].contains(&n.as_str()) || ["token", "secret", "key", "password", "auth"].iter().any(|w| n.contains(w))
}

/// Does `text` hold something shaped like a key or a token? The same shapes
/// the concierge masks before sending text out ([`anonymise`]'s `secret`).
fn looks_secret(text: &str) -> bool {
    let rules = anonymise::Rules::parse("patterns = [\"secret\"]").unwrap_or_default();
    anonymise::mask(text, &rules).count > 0
}

/// `text` without its `${key}` placeholders.
fn without_placeholders(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        match rest[i..].find('}') {
            Some(j) => rest = &rest[i + j + 1..],
            None => {
                rest = &rest[i..];
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

const SECRET_REFUSAL: &str = "Jarvis never handles secrets: put a ${key} placeholder where the secret goes, and the person adds its value in sbxw's MCP dialog";

fn check_mcp_secrets(spec: &McpServerSpec) -> Result<(), String> {
    for h in &spec.headers {
        let (name, value) = h.split_once(':').unwrap_or((h.as_str(), ""));
        let literal = without_placeholders(value);
        let scheme_only = literal.split_whitespace().all(|w| ["bearer", "basic", "token"].contains(&w.to_ascii_lowercase().as_str()));
        if (credential_header(name) && !scheme_only) || looks_secret(&literal) {
            return Err(format!("header '{}' carries a literal credential. {SECRET_REFUSAL}", name.trim()));
        }
    }
    for (what, text) in std::iter::once(("the URL", &spec.url)).chain(spec.args.iter().map(|a| ("an argument", a))) {
        if looks_secret(&without_placeholders(text)) {
            return Err(format!("{what} looks like it carries a key or a token. {SECRET_REFUSAL}"));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The world and the plan

/// One sbxw.toml the daemon uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TomlFile {
    pub path: String,
    /// Empty when the file does not exist yet.
    pub raw: String,
    /// The file does not exist yet: a change to it creates it.
    pub missing: bool,
}

impl TomlFile {
    /// The diff from this file to `text`: a new file when it is missing.
    fn diff_to(&self, text: &str) -> String {
        unified(&self.raw, text, if self.missing { "/dev/null" } else { &self.path }, &self.path)
    }

    /// What a title adds when the write creates the file.
    fn creates(&self) -> String {
        if self.missing {
            format!(", creating {} (it does not exist yet)", self.path)
        } else {
            String::new()
        }
    }

    /// The same file once `text` is written.
    fn written(&self, text: String) -> TomlFile {
        TomlFile { path: self.path.clone(), raw: text, missing: false }
    }
}

/// What the daemon read, for [`plan`].
#[derive(Debug, Clone, Default)]
pub struct World {
    /// By key: [`DAEMON_FILE`], or a sandbox name.
    pub files: BTreeMap<String, TomlFile>,
    pub kits: BTreeMap<String, JarvisKit>,
    /// `state/kits/`, absolute: a Jarvis kit's reference in a `kits` line is
    /// `<kit_root>/<name>`.
    pub kit_root: String,
    /// MCP server names; `None` when not read (no MCP change asked).
    pub mcp: Option<Vec<String>>,
    /// The models `claude_model` may name.
    pub models: Vec<String>,
    /// The desktop app's screens, as the daemon read them.
    pub desktop: Desktop,
}

impl World {
    pub fn kit_ref(&self, name: &str) -> String {
        format!("{}/{name}", self.kit_root.trim_end_matches('/'))
    }
}

/// What applying one change does, decided at planning.
#[derive(Debug, Clone)]
pub enum Op {
    /// Write the kit's whole directory (files under `files/` it does not name are pruned).
    WriteKit { kit: String, tree: Vec<KitFile> },
    RemoveKit { kit: String },
    /// Through the Kits dialog's own write, which also updates the daemon's kits.
    KitsLine { file: String, path: String, list: Vec<String> },
    /// The daemon's sbxw.toml rewritten in place; `network_added`: the
    /// hosts this write adds, recorded as approved.
    WriteToml { path: String, text: String, network_added: Vec<String> },
    /// The daemon's sbxw.toml rewritten in place, with `[voice.words]` as
    /// `words` after it: the voice takes the table at once.
    VoiceWords { path: String, text: String, words: BTreeMap<String, Said> },
    /// An sbxw.toml rewritten in place for `[emphasis]`; `live`: the daemon's
    /// settings after it, when the file is the daemon's (the UIs redraw at
    /// once). A sandbox's own file takes effect at its next bring-up.
    Emphasis { path: String, text: String, live: Option<EmphasisSettings> },
    McpAdd { replace: bool, spec: McpSpec },
    McpRm { name: String },
    /// The override of screen `name` written with `content`. The daemon
    /// builds the path from the name alone.
    WriteScreen { name: String, content: String },
    /// The override of screen `name` deleted.
    RemoveScreen { name: String },
}

/// One change, planned.
#[derive(Debug, Clone)]
pub struct Planned {
    pub view: ChangeView,
    pub op: Op,
}

/// What the person reads about one change.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeView {
    pub kind: String,
    pub title: String,
    pub diff: String,
    pub risk: Risk,
    /// The command line a `stdio` MCP server runs on this machine, outside
    /// any sandbox: what the person must acknowledge on screen to approve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_command: Option<String>,
    /// A desktop screen change: the intents the new screen can emit, as a
    /// best-effort scan of its text finds them (a hint, not a proof).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intents: Option<Vec<IntentHint>>,
    /// What the change was planned against, when the diff alone does not
    /// say it (a desktop screen: the override and the built-in). Approval
    /// compares it like the rest of the view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
}

/// Plan every change in order against `world` (each sees the ones before
/// it). The first invalid change refuses them all, named by its index.
pub fn plan(world: &World, changes: &[Change]) -> Result<Vec<Planned>, String> {
    if changes.is_empty() {
        return Err("a proposal needs at least one change".into());
    }
    if changes.len() > MAX_CHANGES {
        return Err(format!("{} changes: a proposal has at most {MAX_CHANGES}", changes.len()));
    }
    let mut w = world.clone();
    changes
        .iter()
        .enumerate()
        .map(|(i, c)| plan_one(&mut w, c).map_err(|e| format!("change {} ({}): {e}", i + 1, c.kind())))
        .collect()
}

/// The highest risk of the planned changes.
pub fn risk_of(planned: &[ChangeView]) -> Risk {
    planned.iter().map(|p| p.risk).max().unwrap_or_default()
}

fn clean_list(list: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for k in list.iter().map(|k| k.trim()).filter(|k| !k.is_empty()) {
        if !out.iter().any(|o| o == k) {
            out.push(k.to_string());
        }
    }
    out
}

fn plan_one(w: &mut World, c: &Change) -> Result<Planned, String> {
    let view = |title: String, diff: String, risk: Risk| ChangeView { kind: c.kind().into(), title, diff, risk, ..Default::default() };
    match c {
        Change::KitWrite { kit, description, files, remove, context } => {
            if !valid_kit(kit) {
                return Err(format!("'{kit}' is not a Jarvis kit name: 'jarvis-' then 1 to 40 of a-z, 0-9 and '-'"));
            }
            let old = w.kits.get(kit).cloned();
            let mut next = old.clone().unwrap_or_else(|| JarvisKit { description: DEFAULT_DESCRIPTION.into(), ..Default::default() });
            if let Some(d) = description {
                let d = d.trim();
                if d.contains(['\n', '\r']) || d.len() > MAX_DESCRIPTION {
                    return Err(format!("the description is one line of at most {MAX_DESCRIPTION} bytes"));
                }
                check_text("the description", d, MAX_DESCRIPTION)?;
                next.description = if d.is_empty() { DEFAULT_DESCRIPTION.into() } else { d.to_string() };
            }
            if let Some(ctx) = context {
                check_text("the context", ctx, MAX_CONTEXT)?;
                next.context = (!ctx.trim().is_empty()).then(|| ctx.clone());
            }
            for p in remove {
                check_path(p)?;
                if !next.files.iter().any(|f| &f.path == p) {
                    return Err(format!("'{p}' is not a file of {kit}, so it cannot be removed"));
                }
                next.files.retain(|f| &f.path != p);
            }
            let mut seen: Vec<&str> = Vec::new();
            for f in files {
                check_path(&f.path)?;
                if seen.contains(&f.path.as_str()) {
                    return Err(format!("'{}' is given twice", f.path));
                }
                seen.push(&f.path);
                if f.content.contains('\0') {
                    return Err(format!("'{}': text files only", f.path));
                }
                // A file and a folder of the same name cannot both exist.
                let clash = next.files.iter().chain(files).find(|o| o.path.starts_with(&format!("{}/", f.path)) || f.path.starts_with(&format!("{}/", o.path)));
                if let Some(o) = clash {
                    return Err(format!("'{}' and '{}': one is a folder of the other", f.path, o.path));
                }
                match next.files.iter_mut().find(|o| o.path == f.path) {
                    Some(o) => o.content = f.content.clone(),
                    None => next.files.push(f.clone()),
                }
            }
            next.files.sort_by(|a, b| a.path.cmp(&b.path));
            if next.files.len() > my_skills::MAX_FILES {
                return Err(format!("{} files: a kit Jarvis writes has at most {}", next.files.len(), my_skills::MAX_FILES));
            }
            let total: usize = next.files.iter().map(|f| f.content.len()).sum::<usize>() + next.context.as_ref().map_or(0, |c| c.len());
            if total > my_skills::MAX_BYTES {
                return Err(format!("{} KB: a kit Jarvis writes is at most {} KB", total / 1024, my_skills::MAX_BYTES / 1024));
            }
            if next.files.is_empty() && next.context.is_none() {
                return Err("a kit needs at least one file or a context".into());
            }
            if old.as_ref() == Some(&next) {
                return Err(format!("{kit} already is exactly this: nothing to change"));
            }
            let before = old.as_ref().map(|o| render_kit(kit, o)).unwrap_or_default();
            let tree = render_kit(kit, &next);
            let diff = tree_diff(kit, &before, &tree);
            let title = match &old {
                None => format!("Create kit {kit} ({} file{})", next.files.len(), plural(next.files.len())),
                Some(_) => format!("Update kit {kit}"),
            };
            w.kits.insert(kit.clone(), next);
            Ok(Planned { view: view(title, diff, Risk::High), op: Op::WriteKit { kit: kit.clone(), tree } })
        }
        Change::KitRemove { kit } => {
            if !valid_kit(kit) {
                return Err(format!("'{kit}': only a Jarvis kit (jarvis-*) can be removed"));
            }
            let Some(old) = w.kits.get(kit) else { return Err(format!("there is no kit {kit}")) };
            let r = w.kit_ref(kit);
            for (key, f) in &w.files {
                if kits::read_kits_line(&f.raw).ok().flatten().unwrap_or_default().contains(&r) {
                    return Err(format!("the kits line of '{key}' ({}) uses {kit}: remove it there first, with a kits_line change before this one", f.path));
                }
            }
            let diff = tree_diff(kit, &render_kit(kit, old), &[]);
            w.kits.remove(kit);
            Ok(Planned { view: view(format!("Remove kit {kit}"), diff, Risk::High), op: Op::RemoveKit { kit: kit.clone() } })
        }
        Change::KitsLine { file, add, remove } => {
            let keys = || w.files.keys().cloned().collect::<Vec<_>>().join(", ");
            let Some(f) = w.files.get(file).cloned() else {
                return Err(format!("'{file}' is not an sbxw.toml sbxw uses; the ones it uses: {}", keys()));
            };
            let as_ref = |k: &str| if valid_kit(k) { w.kit_ref(k) } else { k.to_string() };
            let current = kits::read_kits_line(&f.raw).map_err(|e| format!("{} is not valid TOML: {e}", f.path))?;
            let mut list: Vec<String> = current.unwrap_or_else(|| kits::DEFAULT_KITS.iter().map(|s| s.to_string()).collect());
            let (add, remove) = (clean_list(add), clean_list(remove));
            if add.is_empty() && remove.is_empty() {
                return Err("nothing to add or remove".into());
            }
            for k in &remove {
                let r = as_ref(k);
                if !list.contains(&r) {
                    return Err(format!("'{k}' is not in the kits line of '{file}' ({})", list.join(", ")));
                }
                list.retain(|x| *x != r);
            }
            let mut risk = Risk::Low;
            for k in &add {
                let r = match kits::bundled_name(k) {
                    Some(b) if kits::BUNDLED.contains(&b) => {
                        risk = risk.max(bundled_risk(b));
                        k.clone()
                    }
                    Some(_) => return Err(format!("'{k}' is not a bundled kit; the bundled ones: {}", kits::BUNDLED.iter().map(|b| format!("sbxw:{b}")).collect::<Vec<_>>().join(", "))),
                    None if valid_kit(k) && w.kits.contains_key(k) => {
                        risk = Risk::High;
                        w.kit_ref(k)
                    }
                    None if valid_kit(k) => return Err(format!("there is no kit {k}: write it first, with a kit_write change before this one")),
                    None => return Err(format!("'{k}': a kits line change adds sbxw:<bundled> or a jarvis-* kit, nothing else")),
                };
                if list.contains(&r) {
                    return Err(format!("'{k}' is already in the kits line of '{file}'"));
                }
                list.push(r);
            }
            let text = kits::set_kits_line(&f.raw, Some(list.as_slice())).map_err(|e| format!("{} is not valid TOML: {e}", f.path))?;
            Config::parse(&text).map_err(|e| format!("the new kits line: {e:#}"))?;
            let diff = f.diff_to(&text);
            let title = format!("Kits of {} ({}): {}{}", file, f.path, change_words(&add, &remove), f.creates());
            w.files.insert(file.clone(), f.written(text));
            Ok(Planned { view: view(title, diff, risk), op: Op::KitsLine { file: file.clone(), path: f.path, list } })
        }
        Change::Setting { key, value } => {
            if setting_type(key).is_none() {
                // The key first: the error the agent acts on, file or no file.
                return Err(set_setting("", key, value, &w.models).unwrap_err());
            }
            let f = daemon_file(w)?;
            let text = set_setting(&f.raw, key, value, &w.models)?;
            if text == f.raw {
                return Err(format!("{key} already is {value}"));
            }
            let diff = f.diff_to(&text);
            let shown = if value.is_null() { "the default".to_string() } else { value.to_string() };
            w.files.insert(DAEMON_FILE.into(), f.written(text.clone()));
            Ok(Planned {
                view: view(format!("Set {key} to {shown}{} (takes effect when the daemon restarts)", f.creates()), diff, Risk::Low),
                op: Op::WriteToml { path: f.path, text, network_added: vec![] },
            })
        }
        Change::VoiceWord { word, fr, en } => {
            let said = (fr.is_some() || en.is_some()).then(|| Said { fr: fr.clone(), en: en.clone() });
            spoken::check_word(word.trim())?;
            let f = daemon_file(w)?;
            let (text, words) = set_voice_word(&f.raw, word, said.as_ref())?;
            let lower = word.trim().to_lowercase();
            if text == f.raw {
                return Err(format!("'{lower}' is already said this way"));
            }
            let built_in = spoken::WORDS.iter().any(|(b, ..)| b.to_lowercase() == lower);
            let shown = |s: &Said| {
                let mut parts = Vec::new();
                if let Some(x) = &s.fr {
                    parts.push(format!("\"{}\" in French", x.trim()));
                }
                if let Some(x) = &s.en {
                    parts.push(format!("\"{}\" in English", x.trim()));
                }
                parts.join(", ")
            };
            let what = match &said {
                Some(s) => format!("Say '{lower}' as {}", shown(s)),
                None if built_in => format!("Remove the configured pronunciation of '{lower}' (the built-in one comes back)"),
                None => format!("Remove the pronunciation of '{lower}' (read as written)"),
            };
            let diff = f.diff_to(&text);
            w.files.insert(DAEMON_FILE.into(), f.written(text.clone()));
            Ok(Planned {
                view: view(format!("{what}{} (the voice uses it from the next sentence it reads)", f.creates()), diff, Risk::Low),
                op: Op::VoiceWords { path: f.path, text, words },
            })
        }
        Change::EmphasisWord { word, tone, also, when } => {
            let f = daemon_file(w)?;
            let before = Config::parse(&f.raw).map_err(|e| format!("the daemon's sbxw.toml: {e:#}"))?.emphasis.vocabulary();
            let family = tone.as_ref().map(|t| Family { tone: t.trim().to_string(), also: also.iter().map(|a| a.trim().to_string()).collect(), when: when.as_ref().map(|u| u.trim().to_string()) });
            if family.is_none() && (!also.is_empty() || when.is_some()) {
                return Err("no tone removes the family; give its tone to change it".into());
            }
            let (text, settings) = set_emphasis_word(&f.raw, word, family.as_ref())?;
            let main = emphasis::key(word.trim());
            if text == f.raw {
                return Err(format!("'{main}' already is exactly this"));
            }
            let after = settings.vocabulary();
            let risk = emphasis_risk(&before, &after);
            let what = match (&family, before.contains_key(&main)) {
                (None, _) => format!("Remove the status label '{main}' and its other words"),
                (Some(fam), false) => format!("Add the status label '{main}' ({})", fam.tone),
                (Some(fam), true) => format!("Change the status label '{main}' ({})", fam.tone),
            };
            let diff = f.diff_to(&text);
            w.files.insert(DAEMON_FILE.into(), f.written(text.clone()));
            Ok(Planned {
                view: view(format!("{what}{} (the terminals redraw at once; agents read the new guidance from their next bring-up and new session)", f.creates()), diff, risk),
                op: Op::Emphasis { path: f.path, text, live: Some(settings) },
            })
        }
        Change::EmphasisSwitch { file, enabled, guidance } => {
            let file = file.clone().unwrap_or_else(|| DAEMON_FILE.to_string());
            let keys = || w.files.keys().cloned().collect::<Vec<_>>().join(", ");
            let Some(f) = w.files.get(&file).cloned() else {
                return Err(format!("'{file}' is not an sbxw.toml sbxw uses; the ones it uses: {}", keys()));
            };
            if enabled.is_none() && guidance.is_none() {
                return Err("nothing to switch: give enabled, guidance or both".into());
            }
            let (text, settings) = set_emphasis_switch(&f.raw, *enabled, *guidance)?;
            if text == f.raw {
                return Err(format!("[emphasis] of '{file}' already is this"));
            }
            let live = w.files.get(DAEMON_FILE).is_some_and(|d| d.path == f.path).then_some(settings);
            let onoff = |b: bool| if b { "on" } else { "off" };
            let mut parts = Vec::new();
            if let Some(e) = enabled {
                parts.push(format!("status pills {}", onoff(*e)));
            }
            if let Some(g) = guidance {
                parts.push(format!("their guidance {}", onoff(*g)));
            }
            let when = match (&live, file == DAEMON_FILE) {
                (Some(_), true) => "the terminals follow at once; the guidance changes at each sandbox's next bring-up".to_string(),
                _ => format!("takes effect at '{file}''s next bring-up"),
            };
            let diff = f.diff_to(&text);
            w.files.insert(file.clone(), f.written(text.clone()));
            Ok(Planned {
                view: view(format!("Turn {} for {} ({}){} ({when})", parts.join(" and "), if file == DAEMON_FILE { "every sandbox".to_string() } else { format!("'{file}'") }, f.path, f.creates()), diff, Risk::Low),
                op: Op::Emphasis { path: f.path, text, live },
            })
        }
        Change::NetworkAllow { add, remove } => {
            let f = daemon_file(w)?;
            let cfg = Config::parse(&f.raw).map_err(|e| format!("the daemon's sbxw.toml: {e:#}"))?;
            let mut list = cfg.network_allow.clone();
            let (add, remove) = (clean_list(add), clean_list(remove));
            if add.is_empty() && remove.is_empty() {
                return Err("nothing to add or remove".into());
            }
            for h in &remove {
                if !list.contains(h) {
                    return Err(format!("'{h}' is not in network_allow ({})", list.join(", ")));
                }
                list.retain(|x| x != h);
            }
            let mut added = Vec::new();
            for h in &add {
                let h = crate::policy::clean_host(h)?;
                if list.contains(&h) {
                    return Err(format!("'{h}' is already in network_allow"));
                }
                list.push(h.clone());
                added.push(h);
            }
            let mut doc: toml_edit::DocumentMut = f.raw.parse().map_err(|e| format!("the daemon's sbxw.toml is not valid TOML: {e}"))?;
            let mut arr = toml_edit::Array::new();
            for h in &list {
                arr.push_formatted(toml_edit::Value::from(h.as_str()).decorated("\n    ", ""));
            }
            arr.set_trailing("\n");
            arr.set_trailing_comma(true);
            doc["network_allow"] = toml_edit::value(arr);
            let text = doc.to_string();
            Config::parse(&text).map_err(|e| format!("the new network_allow: {e:#}"))?;
            let diff = f.diff_to(&text);
            w.files.insert(DAEMON_FILE.into(), f.written(text.clone()));
            Ok(Planned {
                view: view(format!("network_allow: {}{} (for sandboxes created after the daemon restarts)", change_words(&add, &remove), f.creates()), diff, Risk::High),
                op: Op::WriteToml { path: f.path, text, network_added: added },
            })
        }
        Change::McpServer { spec } => {
            check_mcp_secrets(spec)?;
            let s = spec.to_spec();
            let args = s.add_args()?;
            let name = crate::mcp::check_name(&s.name)?;
            let servers = w.mcp.as_mut().ok_or("the MCP servers could not be listed")?;
            let replace = servers.contains(&name);
            let mut diff = String::new();
            if replace {
                diff.push_str(&format!("- sbx mcp rm {name}\n"));
            } else {
                servers.push(name.clone());
            }
            diff.push_str(&format!("+ sbx {}\n", args.join(" ")));
            let keys = s.placeholders();
            if !keys.is_empty() {
                diff.push_str(&format!("# secrets to add in the MCP dialog: {}\n", keys.iter().map(|k| format!("${{{k}}}")).collect::<Vec<_>>().join(", ")));
            }
            let title = format!("{} MCP server {name} ({})", if replace { "Replace" } else { "Add" }, s.kind);
            let mut v = view(title, diff, Risk::High);
            v.host_command = host_command(&s);
            Ok(Planned { view: v, op: Op::McpAdd { replace, spec: s } })
        }
        Change::McpRemove { name } => {
            let name = crate::mcp::check_name(name)?;
            let servers = w.mcp.as_mut().ok_or("the MCP servers could not be listed")?;
            if !servers.contains(&name) {
                return Err(format!("there is no MCP server '{name}' ({})", servers.join(", ")));
            }
            servers.retain(|s| *s != name);
            Ok(Planned { view: view(format!("Remove MCP server {name}"), format!("- sbx mcp rm {name}\n"), Risk::High), op: Op::McpRm { name } })
        }
        Change::DesktopScreen { name, content } => {
            let d = w.desktop.screen(name)?.clone();
            if content.contains('\0') {
                return Err(format!("the {name} screen: text only, no NUL character"));
            }
            if content.len() > MAX_SCREEN {
                return Err(format!("the {name} screen: {} KB, a screen is at most {} KB", content.len().div_ceil(1024), MAX_SCREEN / 1024));
            }
            if content.trim().is_empty() {
                return Err(format!("an empty {name} screen draws nothing; to give the app its built-in screen back, remove your override (desktop_screen_remove)"));
            }
            let (now, is_override) = d.effective(name).unwrap_or_default();
            if now == content {
                return Err(format!("the {name} screen already is exactly this: nothing to change"));
            }
            let (target, from) = (d.override_path(name), if is_override { d.override_path(name) } else { d.builtin_path(name) });
            let title = if is_override {
                format!("Desktop app: changes your {name} screen ({target}); the app reloads it at once")
            } else {
                format!("Desktop app: replaces the built-in {name} screen with yours ({target}); the app reloads it at once")
            };
            let hints = d.hints(content, now);
            let diff = format!("{}{}", hint_lines(&d, &hints, content), unified(now, content, &from, &target));
            let mut v = view(title, diff, Risk::High);
            v.intents = Some(hints);
            v.base = Some(d.fingerprint(name));
            w.desktop.overrides.insert(name.clone(), content.clone());
            Ok(Planned { view: v, op: Op::WriteScreen { name: name.clone(), content: content.clone() } })
        }
        Change::DesktopScreenRemove { name } => {
            let d = w.desktop.screen(name)?.clone();
            let Some(current) = d.overrides.get(name) else {
                return Err(format!("the {name} screen has no override: the app already uses its built-in screen"));
            };
            let builtin = d.builtin.get(name).map(String::as_str).unwrap_or_default();
            let hints = d.hints(builtin, current);
            let mut diff = hint_lines(&d, &hints, builtin);
            if current == builtin {
                diff.push_str("# Your override is the same as the built-in: removing it changes nothing on screen.\n");
            }
            diff.push_str(&unified(current, builtin, &d.override_path(name), &d.builtin_path(name)));
            let mut v = view(format!("Desktop app: removes your {name} screen ({}), which restores the built-in", d.override_path(name)), diff, Risk::High);
            v.intents = Some(hints);
            v.base = Some(d.fingerprint(name));
            w.desktop.overrides.remove(name);
            Ok(Planned { view: v, op: Op::RemoveScreen { name: name.clone() } })
        }
    }
}

/// What a `stdio` server runs on this machine, as one shell line: the
/// command and its arguments as sbx gets them (trimmed, blanks dropped),
/// after a `cd` into its directory when one is set. `None` for a remote one.
pub fn host_command(s: &McpSpec) -> Option<String> {
    if s.kind != "stdio" {
        return None;
    }
    let quote = |w: &str| {
        if !w.is_empty() && w.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,".contains(c)) {
            w.to_string()
        } else {
            format!("'{}'", w.replace('\'', "'\\''"))
        }
    };
    let mut words = vec![quote(s.command.trim())];
    words.extend(s.args.iter().map(|a| a.trim()).filter(|a| !a.is_empty()).map(quote));
    let line = words.join(" ");
    Some(match s.dir.trim() {
        "" => line,
        d => format!("cd {} && {line}", quote(d)),
    })
}

// ---------------------------------------------------------------------------
// Desktop screens

/// A screen's size limit, in bytes.
pub const MAX_SCREEN: usize = 64 * 1024;

/// A screen name: 1 to 40 of lowercase letters, digits and `-`.
pub fn valid_screen_name(name: &str) -> bool {
    (1..=40).contains(&name.len()) && name.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

/// One intent the desktop app's core knows, from its `intents.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentInfo {
    pub name: String,
    /// The risk the app declares for it, as it writes it (`direct`,
    /// `approval`, ...); `unknown` when it gives none.
    pub risk: String,
}

/// An intent a proposed screen can emit, from a scan of its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentHint {
    pub name: String,
    pub risk: String,
    /// The screen the app uses now does not emit it.
    #[serde(default)]
    pub new: bool,
}

/// The intents of the app's `intents.json`: an array of objects with a
/// `name` and a `risk` (other fields, such as `args`, ignored), the same
/// under an `intents` key, or an object keyed by name. A `risk` that is an
/// object is named by its `kind`, else its first key.
pub fn parse_intents(raw: &str) -> Result<Vec<IntentInfo>, String> {
    let v: Value = serde_json::from_str(raw).map_err(|e| format!("intents.json is not valid JSON: {e}"))?;
    let risk = |r: Option<&Value>| match r {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(o)) => o.get("kind").and_then(Value::as_str).map(String::from).or_else(|| o.keys().next().cloned()).unwrap_or_else(|| "unknown".into()),
        _ => "unknown".into(),
    };
    let rows: Vec<(String, Option<&Value>)> = match v.get("intents").unwrap_or(&v) {
        Value::Array(list) => list.iter().filter_map(|i| Some((i.get("name")?.as_str()?.to_string(), i.get("risk")))).collect(),
        Value::Object(map) => map.iter().map(|(k, i)| (k.clone(), i.get("risk"))).collect(),
        _ => return Err("intents.json is neither a list of intents nor an object of them".into()),
    };
    let mut out: Vec<IntentInfo> = Vec::new();
    for (name, r) in rows {
        let ok = !name.is_empty() && name.len() <= 64 && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-' || c == b'.');
        if ok && !out.iter().any(|i| i.name == name) {
            out.push(IntentInfo { name, risk: risk(r) });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// The names of `known` intents written as a string literal in `text`, read
/// as Luau ([`scan_screen`]): a best-effort reading of what a screen can
/// emit. A literal that is never emitted is counted; a name built at run
/// time is not, and [`hidden_intents`] says where the text does that in a
/// way the scan recognises.
pub fn scan_intents<'a>(text: &str, known: &'a [IntentInfo]) -> Vec<&'a IntentInfo> {
    let scan = scan_screen(text);
    known.iter().filter(|i| scan.literals.contains(&i.name)).collect()
}

/// Where `text` builds an intent in a way that hides its name, one line
/// each, `line N: what`. Empty does not prove there is none: a name or the
/// word `intent` assembled from pieces (`"int" .. "ent"`) and put in a table
/// by a computed key is invisible to a text scan.
pub fn hidden_intents(text: &str) -> Vec<String> {
    scan_screen(text).hidden
}

/// What [`scan_screen`] reads in a screen.
#[derive(Debug, Default)]
pub struct ScreenScan {
    /// Every string literal's value (escapes decoded).
    pub literals: std::collections::BTreeSet<String>,
    /// `line N: what`, where an intent's name is hidden from the scan.
    pub hidden: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Name(String),
    /// A string literal and its value.
    Str(String),
    /// An interpolated string (`` `a{b}` ``): not a literal.
    Interp,
    Sym(char),
    /// `..`, `==`, `+=` and the other operators of more than one character.
    Op(String),
    Num,
}

/// Luau tokens of `text` with their line, comments dropped. `Err` with the
/// line where a string or a comment does not end.
fn luau_tokens(text: &str) -> Result<Vec<(Tok, usize)>, usize> {
    let b: Vec<char> = text.chars().collect();
    let (mut i, mut line, mut out) = (0usize, 1usize, Vec::new());
    // `[`, `=`*n, `[` at `at`: the level n, if it opens a long bracket.
    let long_open = |at: usize| -> Option<usize> {
        let mut j = at + 1;
        while b.get(j) == Some(&'=') {
            j += 1;
        }
        (b.get(at) == Some(&'[') && b.get(j) == Some(&'[')).then_some(j - at - 1)
    };
    // From `at` (just after the opening bracket), the text up to `]`, `=`*n, `]`.
    let long_body = |at: usize, n: usize, line: &mut usize| -> Option<(String, usize)> {
        let mut j = at;
        while j < b.len() {
            if b[j] == ']' && (1..=n).all(|k| b.get(j + k) == Some(&'=')) && b.get(j + n + 1) == Some(&']') {
                return Some((b[at..j].iter().collect(), j + n + 2));
            }
            if b[j] == '\n' {
                *line += 1;
            }
            j += 1;
        }
        None
    };
    while i < b.len() {
        let c = b[i];
        let at_line = line;
        if c == '\n' {
            line += 1;
            i += 1;
        } else if c.is_whitespace() {
            i += 1;
        } else if c == '-' && b.get(i + 1) == Some(&'-') {
            if let Some(n) = long_open(i + 2) {
                let (_, end) = long_body(i + 2 + n + 2, n, &mut line).ok_or(at_line)?;
                i = end;
            } else {
                while i < b.len() && b[i] != '\n' {
                    i += 1;
                }
            }
        } else if let Some(n) = (c == '[').then(|| long_open(i)).flatten() {
            let (body, end) = long_body(i + n + 2, n, &mut line).ok_or(at_line)?;
            // A long string skips one newline right after its opening.
            let body = body.strip_prefix("\r\n").or_else(|| body.strip_prefix('\n')).unwrap_or(&body).to_string();
            out.push((Tok::Str(body), at_line));
            i = end;
        } else if c == '"' || c == '\'' {
            let mut v = String::new();
            i += 1;
            loop {
                let Some(&x) = b.get(i) else { return Err(at_line) };
                i += 1;
                if x == c {
                    break;
                }
                if x == '\n' {
                    return Err(at_line);
                }
                if x != '\\' {
                    v.push(x);
                    continue;
                }
                let Some(&e) = b.get(i) else { return Err(at_line) };
                i += 1;
                match e {
                    'n' => v.push('\n'),
                    't' => v.push('\t'),
                    'r' => v.push('\r'),
                    'a' => v.push('\u{7}'),
                    'b' => v.push('\u{8}'),
                    'f' => v.push('\u{c}'),
                    'v' => v.push('\u{b}'),
                    '\n' => {
                        line += 1;
                        v.push('\n');
                    }
                    'z' => {
                        while b.get(i).is_some_and(|x| x.is_whitespace()) {
                            if b[i] == '\n' {
                                line += 1;
                            }
                            i += 1;
                        }
                    }
                    'x' => {
                        let h: String = b.iter().skip(i).take(2).collect();
                        i += 2;
                        v.extend(u32::from_str_radix(&h, 16).ok().and_then(char::from_u32));
                    }
                    'u' if b.get(i) == Some(&'{') => {
                        let h: String = b.iter().skip(i + 1).take_while(|x| **x != '}').collect();
                        i += h.chars().count() + 2;
                        v.extend(u32::from_str_radix(&h, 16).ok().and_then(char::from_u32));
                    }
                    d if d.is_ascii_digit() => {
                        let mut n = d.to_digit(10).unwrap_or(0);
                        for _ in 0..2 {
                            match b.get(i).and_then(|x| x.to_digit(10)) {
                                Some(k) => {
                                    n = n * 10 + k;
                                    i += 1;
                                }
                                None => break,
                            }
                        }
                        v.extend(char::from_u32(n));
                    }
                    other => v.push(other),
                }
            }
            out.push((Tok::Str(v), at_line));
        } else if c == '`' {
            let (mut v, mut plain) = (String::new(), true);
            i += 1;
            loop {
                let Some(&x) = b.get(i) else { return Err(at_line) };
                i += 1;
                match x {
                    '`' => break,
                    '\\' => {
                        v.extend(b.get(i));
                        i += 1;
                    }
                    '{' => plain = false,
                    '\n' => {
                        line += 1;
                        v.push(x);
                    }
                    _ => v.push(x),
                }
            }
            out.push((if plain { Tok::Str(v) } else { Tok::Interp }, at_line));
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while b.get(i).is_some_and(|x| x.is_alphanumeric() || *x == '_') {
                i += 1;
            }
            out.push((Tok::Name(b[start..i].iter().collect()), at_line));
        } else if c.is_ascii_digit() {
            while b.get(i).is_some_and(|x| x.is_alphanumeric() || *x == '_' || *x == '.') {
                i += 1;
            }
            out.push((Tok::Num, at_line));
        } else {
            let two: String = b.iter().skip(i).take(3).collect();
            let op = ["...", "..=", "//="].into_iter().find(|o| two.starts_with(o)).or_else(|| {
                ["..", "==", "~=", "<=", ">=", "+=", "-=", "*=", "/=", "%=", "^=", "//", "::", "->"].into_iter().find(|o| two.starts_with(o))
            });
            match op {
                Some(o) => {
                    out.push((Tok::Op(o.into()), at_line));
                    i += o.len();
                }
                None => {
                    out.push((Tok::Sym(c), at_line));
                    i += 1;
                }
            }
        }
    }
    Ok(out)
}

/// Read a screen's Luau for what the intent hint needs: its string literals,
/// and the places where an intent is built in a way that hides its name.
/// The core recognises an intent as `intent("name", args)` or as any table
/// `{ kind = "intent", name = ..., args = ... }`, so the scan looks at:
///
/// - `intent(` whose name is not one plain string literal, and `intent`
///   passed around as a value;
/// - a table constructor with `kind = "intent"` (or `["kind"] = "intent"`)
///   whose `name` is not one plain string literal; one whose `kind` is
///   computed; one with a computed key (`[k] = v`);
/// - `kind` or `name` set field by field (`t.kind = ...`, `t["name"] = ...`),
///   and `rawset`, `setmetatable`, which set fields out of sight.
///
/// What it cannot see: the word `intent` or a name assembled from pieces and
/// stored through a computed key outside a constructor (`t[k] = v`), which
/// ordinary screens do all the time. Hence the hint is never a proof.
pub fn scan_screen(text: &str) -> ScreenScan {
    let mut scan = ScreenScan::default();
    let toks = match luau_tokens(text) {
        Ok(t) => t,
        Err(line) => {
            scan.hidden.push(format!("line {line}: a string or a comment does not end, so the rest of the file cannot be read"));
            return scan;
        }
    };
    let mut flag = |line: usize, what: &str| {
        let s = format!("line {line}: {what}");
        if !scan.hidden.contains(&s) {
            scan.hidden.push(s);
        }
    };
    let sep = |t: Option<&(Tok, usize)>| matches!(t, Some((Tok::Sym(',' | ';' | '}' | ')'), _)) | None);
    let assign = |t: Option<&(Tok, usize)>| matches!(t, Some((Tok::Sym('='), _)));
    for (k, (t, line)) in toks.iter().enumerate() {
        let (prev, next) = (k.checked_sub(1).and_then(|p| toks.get(p)).map(|x| &x.0), toks.get(k + 1).map(|x| &x.0));
        match t {
            Tok::Str(v) => {
                scan.literals.insert(v.clone());
            }
            Tok::Name(n) if n == "intent" && !matches!(prev, Some(Tok::Sym('.' | ':'))) => match next {
                Some(Tok::Sym('(')) => {
                    let lone = matches!(toks.get(k + 2), Some((Tok::Str(_), _))) && sep(toks.get(k + 3));
                    if !lone {
                        flag(*line, "intent(...) is called with a name that is not written as plain text");
                    }
                }
                Some(Tok::Str(_)) => {}
                Some(Tok::Sym('=')) if matches!(prev, Some(Tok::Sym('{' | ',' | ';'))) => {}
                _ => flag(*line, "intent is used as a value, so where its calls go cannot be followed"),
            },
            Tok::Name(n) if matches!(n.as_str(), "rawset" | "setmetatable") && next == Some(&Tok::Sym('(')) => {
                flag(*line, &format!("{n}(...) sets fields this scan does not see"));
            }
            // `t.kind = ...`, `t.name = ...`
            Tok::Name(n) if matches!(n.as_str(), "kind" | "name") && prev == Some(&Tok::Sym('.')) && assign(toks.get(k + 1)) => {
                flag(*line, &format!("a table's {n} is set field by field"));
            }
            // `t["kind"] = ...` outside a constructor (inside one, `[` follows `{`, `,` or `;`).
            Tok::Sym('[') if !matches!(prev, Some(Tok::Sym('{' | ',' | ';'))) => {
                if let (Some((Tok::Str(key), _)), Some((Tok::Sym(']'), _))) = (toks.get(k + 1), toks.get(k + 2)) {
                    if matches!(key.as_str(), "kind" | "name") && assign(toks.get(k + 3)) {
                        flag(*line, &format!("a table's {key} is set field by field"));
                    }
                }
            }
            _ => {}
        }
    }
    // Table constructors: each `{` with its matching `}`, fields at its own depth.
    let mut open = Vec::new();
    let mut pairs = Vec::new();
    for (k, (t, _)) in toks.iter().enumerate() {
        match t {
            Tok::Sym('{') => open.push(k),
            Tok::Sym('}') => {
                if let Some(o) = open.pop() {
                    pairs.push((o, k));
                }
            }
            _ => {}
        }
    }
    for (o, c) in pairs {
        let line = toks[o].1;
        let mut fields: Vec<&[(Tok, usize)]> = Vec::new();
        let (mut depth, mut from) = (0i32, o + 1);
        for k in o + 1..c {
            match &toks[k].0 {
                Tok::Sym('(' | '[' | '{') => depth += 1,
                Tok::Sym(')' | ']' | '}') => depth -= 1,
                Tok::Sym(',' | ';') if depth == 0 => {
                    fields.push(&toks[from..k]);
                    from = k + 1;
                }
                _ => {}
            }
        }
        fields.push(&toks[from..c]);
        let mut keyed: Vec<(String, &[(Tok, usize)])> = Vec::new();
        for f in fields {
            match f {
                [(Tok::Name(key), _), (Tok::Sym('='), _), value @ ..] => keyed.push((key.clone(), value)),
                [(Tok::Sym('['), _), (Tok::Str(key), _), (Tok::Sym(']'), _), (Tok::Sym('='), _), value @ ..] => keyed.push((key.clone(), value)),
                [(Tok::Sym('['), l), ..] => flag(*l, "a table key is computed ([...] = ...), so a kind or a name can hide in it"),
                _ => {}
            }
        }
        let literal = |v: &[(Tok, usize)]| match v {
            [(Tok::Str(s), _)] => Some(s.clone()),
            _ => None,
        };
        for (key, value) in &keyed {
            if key == "kind" && literal(value).is_none() {
                flag(line, "a table's kind is computed, not written as plain text");
            }
        }
        let is_intent = keyed.iter().any(|(k, v)| k == "kind" && literal(v).as_deref() == Some("intent"));
        if is_intent && !keyed.iter().any(|(k, v)| k == "name" && literal(v).is_some()) {
            flag(line, "a table with kind = \"intent\" has a name that is not written as plain text");
        }
    }
    scan.hidden.sort_by_key(|h| h.split(':').next().and_then(|l| l.trim_start_matches("line ").parse::<usize>().ok()).unwrap_or(0));
    scan
}

/// The desktop app's screens, as the daemon read them from sbxw's home.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Desktop {
    /// `builtin/` exists: the app has run on this machine.
    pub present: bool,
    /// `<sbxw home>/desktop/builtin`, absolute.
    pub builtin_dir: String,
    /// `<sbxw home>/desktop/screens`, absolute: the override folder.
    pub screens_dir: String,
    /// The built-in screens by name.
    pub builtin: BTreeMap<String, String>,
    /// The overrides by name.
    pub overrides: BTreeMap<String, String>,
    pub intents: Vec<IntentInfo>,
    /// Why `intents.json` could not be read, if it could not.
    pub intents_error: Option<String>,
}

impl Desktop {
    pub fn override_path(&self, name: &str) -> String {
        format!("{}/{name}.luau", self.screens_dir.trim_end_matches('/'))
    }

    pub fn builtin_path(&self, name: &str) -> String {
        format!("{}/screens/{name}.luau", self.builtin_dir.trim_end_matches('/'))
    }

    /// What the app uses now for `name`, and whether it is an override.
    pub fn effective(&self, name: &str) -> Option<(&str, bool)> {
        match self.overrides.get(name) {
            Some(o) => Some((o.as_str(), true)),
            None => self.builtin.get(name).map(|b| (b.as_str(), false)),
        }
    }

    /// Screen `name`, checked: the app has run, and it is a built-in's name.
    pub fn screen(&self, name: &str) -> Result<&Self, String> {
        if !self.present {
            return Err(format!(
                "sbxw's desktop app has not run on this machine yet ({} does not exist), so there is no screen to change: the person starts the desktop app once, then asks again",
                self.builtin_dir
            ));
        }
        if !valid_screen_name(name) {
            return Err(format!("'{}' is not a screen name: 1 to 40 of a-z, 0-9 and '-'", name.chars().take(60).collect::<String>()));
        }
        if !self.builtin.contains_key(name) {
            let names = self.builtin.keys().cloned().collect::<Vec<_>>().join(", ");
            return Err(format!(
                "the desktop app has no '{name}' screen (its screens: {names}). A change can only replace one of these: the app has no place to show a new screen in this version"
            ));
        }
        Ok(self)
    }

    /// The intents `next` can emit, each marked new when `now` does not.
    pub fn hints(&self, next: &str, now: &str) -> Vec<IntentHint> {
        let before = scan_intents(now, &self.intents);
        scan_intents(next, &self.intents).into_iter().map(|i| IntentHint { name: i.name.clone(), risk: i.risk.clone(), new: !before.contains(&i) }).collect()
    }

    /// What a change to `name` was planned against: the override (or its
    /// absence) and the built-in, so approval refuses when either moved.
    pub fn fingerprint(&self, name: &str) -> String {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |b: &[u8]| {
            for x in b {
                h ^= u64::from(*x);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        };
        match self.overrides.get(name) {
            Some(o) => {
                eat(b"override\0");
                eat(o.as_bytes());
            }
            None => eat(b"none\0"),
        }
        eat(b"\0builtin\0");
        eat(self.builtin.get(name).map(String::as_bytes).unwrap_or_default());
        format!("fnv1a64:{h:016x}")
    }
}

/// The lines above a screen's diff that list what it can emit.
fn hint_lines(d: &Desktop, hints: &[IntentHint], next: &str) -> String {
    let mut out = String::new();
    if let Some(e) = &d.intents_error {
        out.push_str(&format!("# The app's intents.json could not be read ({e}): no list of what this screen emits.\n"));
    } else if hints.is_empty() {
        out.push_str("# No intent of the app is written by name in this screen.\n");
    } else {
        out.push_str("# Intents written by name in this screen:\n");
        for h in hints {
            out.push_str(&format!("#   {} (risk: {}){}\n", h.name, h.risk, if h.new { ", NEW: the screen used now does not emit it" } else { "" }));
        }
    }
    let hidden = hidden_intents(next);
    if !hidden.is_empty() {
        out.push_str("# This screen builds an intent in a way that hides its name:\n");
        for h in hidden {
            out.push_str(&format!("#   {h}\n"));
        }
    }
    out.push_str("# This list is a hint: a screen can hide an intent name; read the diff.\n");
    out
}

fn daemon_file(w: &World) -> Result<TomlFile, String> {
    w.files.get(DAEMON_FILE).cloned().ok_or_else(|| "the daemon does not know where its sbxw.toml would be, so there is no file to change".into())
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn change_words(add: &[String], remove: &[String]) -> String {
    let mut parts = Vec::new();
    if !add.is_empty() {
        parts.push(format!("add {}", add.join(", ")));
    }
    if !remove.is_empty() {
        parts.push(format!("remove {}", remove.join(", ")));
    }
    parts.join("; ")
}

/// The diff of a kit directory, file by file, paths as `<kit>/<rel>`.
fn tree_diff(kit: &str, before: &[KitFile], after: &[KitFile]) -> String {
    let mut rels: Vec<&str> = before.iter().chain(after).map(|f| f.rel.as_str()).collect();
    rels.sort();
    rels.dedup();
    let mut out = String::new();
    for rel in rels {
        let old = before.iter().find(|f| f.rel == rel).map(|f| f.content.as_str());
        let new = after.iter().find(|f| f.rel == rel).map(|f| f.content.as_str());
        if old == new {
            continue;
        }
        let name = format!("{kit}/{rel}");
        let (a, b) = (if old.is_some() { name.clone() } else { "/dev/null".into() }, if new.is_some() { name } else { "/dev/null".into() });
        out.push_str(&unified(old.unwrap_or(""), new.unwrap_or(""), &a, &b));
    }
    out
}

// ---------------------------------------------------------------------------
// Unified diff

/// A unified diff of two texts, three lines of context; empty when equal.
pub fn unified(old: &str, new: &str, old_name: &str, new_name: &str) -> String {
    if old == new {
        return String::new();
    }
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    #[derive(Clone, Copy, PartialEq)]
    enum E {
        Eq,
        Del,
        Add,
    }
    // (op, index in a, index in b)
    let mut ops: Vec<(E, usize, usize)> = Vec::new();
    let (n, m) = (a.len(), b.len());
    if n.saturating_mul(m) <= 4_000_000 {
        let mut lcs = vec![0u32; (n + 1) * (m + 1)];
        let at = |i: usize, j: usize| i * (m + 1) + j;
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[at(i, j)] = if a[i] == b[j] { lcs[at(i + 1, j + 1)] + 1 } else { lcs[at(i + 1, j)].max(lcs[at(i, j + 1)]) };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            if i < n && j < m && a[i] == b[j] {
                ops.push((E::Eq, i, j));
                i += 1;
                j += 1;
            } else if i < n && (j == m || lcs[at(i + 1, j)] >= lcs[at(i, j + 1)]) {
                ops.push((E::Del, i, j));
                i += 1;
            } else {
                ops.push((E::Add, i, j));
                j += 1;
            }
        }
    } else {
        ops.extend((0..n).map(|i| (E::Del, i, 0)));
        ops.extend((0..m).map(|j| (E::Add, n, j)));
    }
    const CTX: usize = 3;
    let mut out = format!("--- {old_name}\n+++ {new_name}\n");
    let changed: Vec<usize> = ops.iter().enumerate().filter(|(_, o)| o.0 != E::Eq).map(|(k, _)| k).collect();
    let mut k = 0;
    while k < changed.len() {
        let start = changed[k].saturating_sub(CTX);
        let mut end = changed[k];
        while k + 1 < changed.len() && changed[k + 1] <= end + 2 * CTX + 1 {
            k += 1;
            end = changed[k];
        }
        let end = (end + CTX + 1).min(ops.len());
        let hunk = &ops[start..end];
        let (a0, b0) = (hunk[0].1, hunk[0].2);
        let al = hunk.iter().filter(|o| o.0 != E::Add).count();
        let bl = hunk.iter().filter(|o| o.0 != E::Del).count();
        let range = |s: usize, l: usize| if l == 0 { format!("{s},0") } else { format!("{},{l}", s + 1) };
        out.push_str(&format!("@@ -{} +{} @@\n", range(a0, al), range(b0, bl)));
        for o in hunk {
            match o.0 {
                E::Eq => out.push_str(&format!(" {}\n", a[o.1])),
                E::Del => out.push_str(&format!("-{}\n", a[o.1])),
                E::Add => out.push_str(&format!("+{}\n", b[o.2])),
            }
        }
        k += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// Proposals

/// What applying one change did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeResult {
    pub ok: bool,
    pub message: String,
    /// The daemon must restart for this change to take effect.
    #[serde(default)]
    pub restart: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    pub id: String,
    pub summary: String,
    pub risk: Risk,
    pub state: State,
    /// Milliseconds since the epoch.
    pub created: u64,
    #[serde(default)]
    pub settled: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
    pub changes: Vec<Change>,
    /// What the person read, one per change: what approval re-checks.
    pub views: Vec<ChangeView>,
    #[serde(default)]
    pub results: Vec<ChangeResult>,
    /// `click` or `voice`, once approved.
    #[serde(default)]
    pub via: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

impl Proposal {
    /// Some change runs a command on this machine: approving takes an
    /// explicit acknowledgement on screen.
    pub fn needs_ack(&self) -> bool {
        self.views.iter().any(|v| v.host_command.is_some())
    }

    /// What the agent reads: no diffs, which it wrote itself.
    pub fn agent_view(&self) -> Value {
        let mut v = json!({ "id": self.id, "state": self.state, "risk": self.risk });
        if let Some(e) = &self.error {
            v["error"] = e.clone().into();
        }
        if !self.results.is_empty() {
            v["results"] = serde_json::to_value(&self.results).unwrap_or_default();
        }
        if let Some(r) = &self.reason {
            v["reason"] = r.clone().into();
        }
        v
    }

    /// What the dialog reads.
    pub fn ui_view(&self) -> Value {
        let mut v = json!({
            "id": self.id,
            "summary": self.summary,
            "risk": self.risk,
            "state": self.state,
            "created": self.created,
            "changes": self.views,
        });
        for (k, val) in [("error", &self.error), ("via", &self.via), ("reason", &self.reason)] {
            if let Some(x) = val {
                v[k] = x.clone().into();
            }
        }
        if !self.results.is_empty() {
            v["results"] = serde_json::to_value(&self.results).unwrap_or_default();
        }
        if let Some(s) = self.settled {
            v["settled"] = s.into();
        }
        if self.needs_ack() {
            v["needs_ack"] = true.into();
        }
        v
    }
}

/// How a person approved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Via {
    Click,
    Voice,
}

/// The proposals, newest last. One pending at a time.
#[derive(Debug, Default)]
pub struct Book {
    list: Vec<Proposal>,
    /// The proposal being applied right now: a second approve is refused.
    applying: Option<String>,
}

impl Book {
    pub fn load(list: Vec<Proposal>) -> Self {
        Self { list, applying: None }
    }

    pub fn list(&self) -> Vec<Proposal> {
        self.list.clone()
    }

    pub fn get(&self, id: &str) -> Option<&Proposal> {
        self.list.iter().find(|p| p.id == id)
    }

    pub fn pending(&self) -> Option<&Proposal> {
        self.list.iter().find(|p| p.state == State::Pending)
    }

    /// Record a proposal already planned. Refused while another is pending.
    pub fn propose(&mut self, id: String, summary: &str, changes: Vec<Change>, views: Vec<ChangeView>, now: u64) -> Result<Proposal, String> {
        if let Some(p) = self.pending() {
            return Err(format!("proposal {} is still waiting for the person: wait on it, or ask them to settle it, before proposing again", p.id));
        }
        let summary = summary.trim();
        if summary.is_empty() {
            return Err("a proposal needs a summary: one or two sentences the person reads first".into());
        }
        let summary: String = summary.chars().take(MAX_SUMMARY).collect();
        let p = Proposal {
            id,
            summary,
            risk: risk_of(&views),
            state: State::Pending,
            created: now,
            settled: None,
            error: None,
            changes,
            views,
            results: vec![],
            via: None,
            reason: None,
        };
        self.list.push(p.clone());
        self.prune();
        Ok(p)
    }

    /// Start applying: only a pending proposal, once, and `voice` only for a
    /// low-risk one.
    pub fn begin(&mut self, id: &str, via: Via, ack_host_command: bool) -> Result<Proposal, ApproveError> {
        let p = self.get(id).ok_or_else(|| ApproveError::NotFound(format!("no proposal '{id}'")))?.clone();
        if p.state != State::Pending || self.applying.as_deref() == Some(id) {
            return Err(ApproveError::Conflict(format!("proposal {id} is not waiting for approval")));
        }
        if via == Via::Voice && p.risk == Risk::High {
            return Err(ApproveError::Conflict("a high-risk proposal is approved with a click in the dialog, not by voice".into()));
        }
        if p.needs_ack() && !ack_host_command {
            return Err(ApproveError::Conflict(
                "this proposal runs a command on this machine, outside any sandbox: it is approved only on screen, with that command acknowledged".into(),
            ));
        }
        self.applying = Some(id.to_string());
        Ok(p)
    }

    /// Settle an applied (or failed) proposal.
    pub fn finish(&mut self, id: &str, via: Via, results: Vec<ChangeResult>, error: Option<String>, now: u64) -> Option<Proposal> {
        if self.applying.as_deref() == Some(id) {
            self.applying = None;
        }
        let p = self.list.iter_mut().find(|p| p.id == id && p.state == State::Pending)?;
        p.state = if error.is_none() && results.iter().all(|r| r.ok) { State::Applied } else { State::Failed };
        p.error = error.or_else(|| results.iter().find(|r| !r.ok).map(|r| r.message.clone()));
        p.results = results;
        p.via = Some(match via {
            Via::Click => "click".into(),
            Via::Voice => "voice".into(),
        });
        p.settled = Some(now);
        let out = p.clone();
        self.prune();
        Some(out)
    }

    pub fn refuse(&mut self, id: &str, reason: Option<String>, now: u64) -> Result<Proposal, ApproveError> {
        if self.applying.as_deref() == Some(id) {
            return Err(ApproveError::Conflict(format!("proposal {id} is being applied")));
        }
        let p = self.list.iter_mut().find(|p| p.id == id).ok_or_else(|| ApproveError::NotFound(format!("no proposal '{id}'")))?;
        if p.state != State::Pending {
            return Err(ApproveError::Conflict(format!("proposal {id} is already settled")));
        }
        p.state = State::Refused;
        p.reason = reason.map(|r| r.trim().chars().take(MAX_SUMMARY).collect::<String>()).filter(|r| !r.is_empty());
        p.settled = Some(now);
        Ok(p.clone())
    }

    /// Keep every pending proposal and the last [`KEEP_SETTLED`] settled.
    fn prune(&mut self) {
        let settled = self.list.iter().filter(|p| p.state != State::Pending).count();
        let mut drop = settled.saturating_sub(KEEP_SETTLED);
        self.list.retain(|p| {
            if drop > 0 && p.state != State::Pending {
                drop -= 1;
                false
            } else {
                true
            }
        });
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApproveError {
    NotFound(String),
    Conflict(String),
}

impl std::fmt::Display for ApproveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApproveError::NotFound(m) | ApproveError::Conflict(m) => f.write_str(m),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> World {
        let mut w = World { kit_root: "/h/.sbxw/state/kits".into(), mcp: Some(vec!["github".into()]), models: vec!["claude-sonnet-5-5".into(), "claude-opus-5-5".into()], ..Default::default() };
        w.files.insert(DAEMON_FILE.into(), TomlFile { path: "/h/sbxw.toml".into(), raw: "# mine\nkits = [\"sbxw:codemap\"]\n\n[voice]\nstt = \"parakeet-tdt-0.6b-v3-int8\"\n".into(), missing: false });
        w.files.insert("myapp".into(), TomlFile { path: "/p/sbxw.toml".into(), raw: String::new(), missing: false });
        w
    }

    fn change(v: Value) -> Change {
        serde_json::from_value(v).unwrap()
    }

    fn kit_write(kit: &str, files: Value) -> Change {
        change(json!({"kind": "kit_write", "kit": kit, "files": files}))
    }

    #[test]
    fn an_emphasis_word_copies_the_built_ins_then_changes_one_family() {
        let w = world();
        // A recolour of a built-in word: low, the table is written whole.
        let p = plan(&w, &[change(json!({"kind": "emphasis_word", "word": "Done", "tone": "cyan", "also": ["verified", "fixed", "passed"], "use": "finished and checked"}))]).unwrap();
        assert_eq!((p[0].view.kind.as_str(), p[0].view.risk), ("emphasis_word", Risk::Low));
        assert!(p[0].view.title.contains("redraw at once") && p[0].view.title.contains("next bring-up"), "{}", p[0].view.title);
        let Op::Emphasis { text, live: Some(live), .. } = &p[0].op else { panic!("{:?}", p[0].op) };
        assert!(text.starts_with("# mine\n") && text.contains("[emphasis.words]") && text.contains("\"needs you\" = {"), "{text}");
        assert_eq!(live.vocabulary()["done"].tone, "cyan");
        assert_eq!(live.vocabulary().len(), emphasis::built_in().len());
        // A new word, or a new `use` line, is text every agent reads: high.
        let p = plan(&w, &[change(json!({"kind": "emphasis_word", "word": "Shipped", "tone": "green"}))]).unwrap();
        assert_eq!(p[0].view.risk, Risk::High);
        assert!(p[0].view.title.starts_with("Add the status label 'shipped'"));
        let p = plan(&w, &[change(json!({"kind": "emphasis_word", "word": "done", "tone": "green", "also": ["verified"], "use": "do whatever"}))]).unwrap();
        assert_eq!(p[0].view.risk, Risk::High);
        // Removing a family, or a synonym, is low.
        let p = plan(&w, &[change(json!({"kind": "emphasis_word", "word": "team"}))]).unwrap();
        assert_eq!(p[0].view.risk, Risk::Low);
        let Op::Emphasis { live: Some(live), .. } = &p[0].op else { panic!() };
        assert!(!live.vocabulary().contains_key("team"));
        let p = plan(&w, &[change(json!({"kind": "emphasis_word", "word": "failed", "tone": "red", "also": ["error"], "use": "something did not work; say what"}))]).unwrap();
        assert_eq!(p[0].view.risk, Risk::Low, "dropping 'broken' adds nothing");
        // Refused with the reason, never dropped.
        let e = |v: Value| plan(&w, &[change(v)]).unwrap_err();
        assert!(e(json!({"kind": "emphasis_word", "word": "done", "tone": "pink"})).contains("tone \"pink\""));
        assert!(e(json!({"kind": "emphasis_word", "word": "a]]b", "tone": "green"})).contains("is not a word"));
        assert!(e(json!({"kind": "emphasis_word", "word": "done", "tone": "green", "also": ["failed"]})).contains("listed twice"));
        assert!(e(json!({"kind": "emphasis_word", "word": "nope"})).contains("none to remove"));
        assert!(e(json!({"kind": "emphasis_word", "word": "done", "also": ["x"]})).contains("no tone removes"));
        assert!(e(json!({"kind": "emphasis_word", "word": "done", "tone": "green", "use": "a\nb"})).contains("one line"));
        assert!(serde_json::from_value::<Change>(json!({"kind": "emphasis_word", "word": "x", "colour": "y"})).is_err());
        // Two in one proposal: the second sees the first.
        let both = plan(&w, &[change(json!({"kind": "emphasis_word", "word": "shipped", "tone": "green"})), change(json!({"kind": "emphasis_word", "word": "Shipped"}))]).unwrap();
        let Op::Emphasis { live: Some(live), .. } = &both[1].op else { panic!() };
        assert!(!live.vocabulary().contains_key("shipped"));
    }

    #[test]
    fn an_emphasis_switch_is_low_and_says_when_it_takes_effect() {
        let w = world();
        let p = plan(&w, &[change(json!({"kind": "emphasis_switch", "enabled": false}))]).unwrap();
        assert_eq!((p[0].view.kind.as_str(), p[0].view.risk), ("emphasis_switch", Risk::Low));
        assert!(p[0].view.title.contains("at once"), "{}", p[0].view.title);
        let Op::Emphasis { text, live: Some(live), .. } = &p[0].op else { panic!() };
        assert!(text.contains("[emphasis]\nenabled = false\n"), "{text}");
        assert!(!live.enabled() && live.guidance());
        // A sandbox's own file: at its next bring-up, nothing live.
        let p = plan(&w, &[change(json!({"kind": "emphasis_switch", "file": "myapp", "guidance": false}))]).unwrap();
        assert!(p[0].view.title.contains("'myapp''s next bring-up"), "{}", p[0].view.title);
        let Op::Emphasis { text, live: None, path } = &p[0].op else { panic!() };
        assert_eq!((path.as_str(), text.as_str()), ("/p/sbxw.toml", "[emphasis]\nguidance = false\n"));
        let e = |v: Value| plan(&w, &[change(v)]).unwrap_err();
        assert!(e(json!({"kind": "emphasis_switch"})).contains("nothing to switch"));
        assert!(e(json!({"kind": "emphasis_switch", "file": "other", "enabled": true})).contains("not an sbxw.toml"));
        let again = { let mut w = w.clone(); w.files.get_mut("myapp").unwrap().raw = "[emphasis]\nenabled = true\n".into(); w };
        assert!(plan(&again, &[change(json!({"kind": "emphasis_switch", "file": "myapp", "enabled": true}))]).unwrap_err().contains("already"));
    }

    #[test]
    fn kit_names_are_jarvis_and_narrow() {
        assert!(valid_kit("jarvis-node-tools") && valid_kit("jarvis-x1"));
        for bad in ["jarvis-", "node", "jarvis-Up", "jarvis-a/b", "jarvis-..", &format!("jarvis-{}", "a".repeat(41))] {
            assert!(!valid_kit(bad), "{bad}");
        }
    }

    #[test]
    fn a_kit_path_cannot_leave_home_or_touch_sbxw_or_claude_settings() {
        for bad in ["../etc/passwd", "a/../../x", "/etc/x", "./x", "a//b", ".sbxw/relay.js", ".sbxw", ".claude/settings.json", ".claude.json", ".claude/settings.local.json", "a\nb", "${{x}}"] {
            assert!(check_path(bad).is_err(), "{bad:?}");
            let e = plan(&world(), &[kit_write("jarvis-x", json!([{"path": bad, "content": "x"}]))]).unwrap_err();
            assert!(e.starts_with("change 1 (kit_write): "), "{e}");
        }
        assert!(check_path(".claude/commands/hello.md").is_ok() && check_path("bin/tool.sh").is_ok());
    }

    #[test]
    fn code_that_runs_by_itself_or_replaces_the_agent_is_refused() {
        for bad in [".bashrc", ".bash_profile", ".bash_login", ".bash_logout", ".profile", ".zshrc", ".zshenv", ".zprofile",
            ".local/bin/claude", ".local/bin/x/claude", ".ssh/authorized_keys", ".ssh", ".claude/CLAUDE.md", ".claude/hooks/pre.sh", ".claude/hooks"] {
            assert!(check_path(bad).is_err(), "{bad}");
        }
        assert!(check_path(".bashrc").unwrap_err().contains("bin/"), "says what to use instead");
        assert!(check_path(".claude/CLAUDE.md").unwrap_err().contains("context"));
        for ok in [".inputrc", ".claude/skills/x/.bashrc", "notes/.profile", ".local/bin/claude-helper", ".local/bin/tool", "docs/.ssh/x", "x/.claude/CLAUDE.md", ".claude/commands/hooks.md"] {
            assert!(check_path(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn every_bundled_kit_has_a_risk_and_personal_ones_are_high() {
        for b in kits::BUNDLED {
            assert!(PERSONAL_KITS.contains(&b) != TOOL_KITS.contains(&b), "{b} is classified exactly once");
        }
        assert!(PERSONAL_KITS.iter().chain(TOOL_KITS).all(|k| kits::BUNDLED.contains(k)), "no stale name");
        for b in PERSONAL_KITS {
            let p = plan(&world(), &[change(json!({"kind": "kits_line", "file": "daemon", "add": [format!("sbxw:{b}")]}))]).unwrap();
            assert_eq!(p[0].view.risk, Risk::High, "{b}");
        }
        let mixed = plan(&world(), &[change(json!({"kind": "kits_line", "file": "daemon", "add": ["sbxw:retro", "sbxw:ssh-agent"]}))]).unwrap();
        assert_eq!(mixed[0].view.risk, Risk::High);
        assert_eq!(bundled_risk("some-new-kit"), Risk::High, "unknown is high");
    }

    #[test]
    fn an_unknown_field_or_kind_is_refused_at_parse() {
        assert!(serde_json::from_value::<Change>(json!({"kind": "kit_write", "kit": "jarvis-x", "spec": "x"})).is_err());
        assert!(serde_json::from_value::<Change>(json!({"kind": "shell", "cmd": "rm"})).is_err());
    }

    #[test]
    fn a_kit_write_renders_a_v3_kit_and_reads_back() {
        let c = change(json!({"kind": "kit_write", "kit": "jarvis-hello", "description": "Says hello", "context": "Use /hello.\n",
            "files": [{"path": ".claude/commands/hello.md", "content": "Say hello: 'quoted' \"x\"\n"}, {"path": "bin/hello.sh", "content": "#!/bin/sh\necho hi\n"}]}));
        let p = plan(&world(), &[c]).unwrap();
        assert_eq!(p[0].view.risk, Risk::High);
        assert!(p[0].view.title.starts_with("Create kit jarvis-hello"));
        let Op::WriteKit { tree, .. } = &p[0].op else { panic!() };
        let spec = &tree[0];
        assert_eq!(spec.rel, "spec.yaml");
        let doc: serde_norway::Value = serde_norway::from_str(&spec.content).unwrap();
        assert_eq!(doc["schemaVersion"].as_str(), Some("3"));
        assert!(doc["build"].as_str().unwrap().contains("COPY --chown=1000:1000 files/home/agent/ /home/agent/"));
        let caps = doc["capabilities"].as_sequence().unwrap();
        assert_eq!(caps[0]["config"]["startup"][0]["command"][2].as_str(), Some("/home/agent/bin/hello.sh"));
        assert_eq!(caps.len(), 2, "chmod and context, nothing else: {}", spec.content);
        assert!(!spec.content.contains("install") && !spec.content.contains("network"));
        assert!(tree.iter().all(|f| !f.executable));
        assert_eq!(tree.iter().find(|f| f.rel == "files/home/agent/bin/hello.sh").unwrap().content, "#!/bin/sh\necho hi\n");
        assert_eq!(parse_spec(&spec.content), ("Says hello".to_string(), Some("Use /hello.\n".to_string())));
        assert!(p[0].view.diff.contains("+++ jarvis-hello/files/home/agent/bin/hello.sh\n"), "{}", p[0].view.diff);
    }

    #[test]
    fn a_kit_update_keeps_other_files_and_shows_only_the_change() {
        let mut w = world();
        w.kits.insert("jarvis-x".into(), JarvisKit { description: "d".into(), context: None, files: vec![KitFileIn { path: "a.md".into(), content: "1\n2\n3\n".into() }, KitFileIn { path: "b.md".into(), content: "b\n".into() }] });
        let c = change(json!({"kind": "kit_write", "kit": "jarvis-x", "files": [{"path": "a.md", "content": "1\nTWO\n3\n"}], "remove": ["b.md"]}));
        let p = plan(&w, &[c]).unwrap();
        let d = &p[0].view.diff;
        assert!(d.contains("-2\n+TWO\n") && d.contains("+++ /dev/null"), "{d}");
        assert!(!d.contains("spec.yaml"), "the spec did not change: {d}");
        assert!(plan(&w, &[change(json!({"kind": "kit_write", "kit": "jarvis-x", "remove": ["nope.md"]}))]).is_err());
    }

    #[test]
    fn kits_line_takes_bundled_or_existing_jarvis_kits_and_rates_them() {
        let w = world();
        let low = plan(&w, &[change(json!({"kind": "kits_line", "file": "daemon", "add": ["sbxw:retro"]}))]).unwrap();
        assert_eq!(low[0].view.risk, Risk::Low);
        assert!(low[0].view.diff.contains("+    \"sbxw:retro\","), "{}", low[0].view.diff);
        let Op::KitsLine { list, .. } = &low[0].op else { panic!() };
        assert_eq!(list, &["sbxw:codemap", "sbxw:retro"]);
        assert!(plan(&w, &[change(json!({"kind": "kits_line", "file": "daemon", "add": ["./planted"]}))]).is_err());
        assert!(plan(&w, &[change(json!({"kind": "kits_line", "file": "/etc/sbxw.toml", "add": ["sbxw:retro"]}))]).unwrap_err().contains("not an sbxw.toml sbxw uses"));
        assert!(plan(&w, &[change(json!({"kind": "kits_line", "file": "daemon", "add": ["jarvis-x"]}))]).unwrap_err().contains("write it first"));
        let both = plan(&w, &[kit_write("jarvis-x", json!([{"path": "a.md", "content": "a"}])), change(json!({"kind": "kits_line", "file": "myapp", "add": ["jarvis-x"]}))]).unwrap();
        assert_eq!(both[1].view.risk, Risk::High);
        let Op::KitsLine { list, .. } = &both[1].op else { panic!() };
        assert!(list.contains(&"/h/.sbxw/state/kits/jarvis-x".to_string()), "defaults then the kit: {list:?}");
        assert_eq!(list.len(), kits::DEFAULT_KITS.len() + 1);
    }

    #[test]
    fn a_used_kit_cannot_be_removed_until_its_line_is() {
        let mut w = world();
        w.kits.insert("jarvis-x".into(), JarvisKit { description: "d".into(), context: Some("c".into()), files: vec![] });
        w.files.get_mut("myapp").unwrap().raw = "kits = [\"/h/.sbxw/state/kits/jarvis-x\"]\n".into();
        assert!(plan(&w, &[change(json!({"kind": "kit_remove", "kit": "jarvis-x"}))]).unwrap_err().contains("remove it there first"));
        assert!(plan(&w, &[change(json!({"kind": "kits_line", "file": "myapp", "remove": ["jarvis-x"]})), change(json!({"kind": "kit_remove", "kit": "jarvis-x"}))]).is_ok());
        assert!(plan(&w, &[change(json!({"kind": "kit_remove", "kit": "sbxw-my-skills"}))]).is_err());
    }

    #[test]
    fn only_allowlisted_settings_with_valid_values() {
        let w = world();
        let e = plan(&w, &[change(json!({"kind": "setting", "key": "web_addr", "value": "0.0.0.0:80"}))]).unwrap_err();
        assert!(e.contains("not a setting Jarvis may change"), "{e}");
        // Remote access is the person's own switch: Jarvis can neither move
        // its listener nor turn it on (it is state, not a setting).
        for key in ["remote_addr", "remote_access", "remote.enabled"] {
            let e = plan(&w, &[change(json!({"kind": "setting", "key": key, "value": "0.0.0.0:7683"}))]).unwrap_err();
            assert!(e.contains("not a setting Jarvis may change"), "{key}: {e}");
        }
        assert!(plan(&w, &[change(json!({"kind": "setting", "key": "voice.tts_voice", "value": "pierre"}))]).unwrap_err().contains("unknown tts_voice"));
        assert!(plan(&w, &[change(json!({"kind": "setting", "key": "battery_warn_below", "value": 300}))]).is_err());
        assert!(plan(&w, &[change(json!({"kind": "setting", "key": "claude_model", "value": "gpt"}))]).is_err());
        let ok = plan(&w, &[change(json!({"kind": "setting", "key": "voice.tts_voice", "value": "de_female"})), change(json!({"kind": "setting", "key": "battery_warn_below", "value": 30}))]).unwrap();
        assert_eq!(risk_of(&ok.iter().map(|p| p.view.clone()).collect::<Vec<_>>()), Risk::Low);
        let Op::WriteToml { text, .. } = &ok[1].op else { panic!() };
        assert!(text.starts_with("# mine\n") && text.contains("tts_voice = \"de_female\"") && text.contains("battery_warn_below = 30"), "{text}");
        let cfg = Config::parse(text).unwrap();
        assert_eq!(current_value(&cfg, "voice.tts_voice"), json!("de_female"));
    }

    /// The daemon started where there is no sbxw.toml: a setting creates it,
    /// says so, and stays low risk; the next change in the same proposal
    /// edits the file the first one creates.
    #[test]
    fn a_setting_on_a_missing_file_creates_it() {
        let mut w = world();
        w.files.insert(DAEMON_FILE.into(), TomlFile { path: "/h/sbxw.toml".into(), raw: String::new(), missing: true });
        let p = plan(&w, &[change(json!({"kind": "setting", "key": "voice.tts_voice", "value": "de_female"})), change(json!({"kind": "setting", "key": "battery_warn_below", "value": 30}))]).unwrap();
        assert!(p[0].view.diff.starts_with("--- /dev/null\n+++ /h/sbxw.toml\n"), "{}", p[0].view.diff);
        assert!(p[0].view.title.contains("creating /h/sbxw.toml"), "{}", p[0].view.title);
        assert_eq!(p[0].view.risk, Risk::Low);
        assert!(p[1].view.diff.starts_with("--- /h/sbxw.toml\n"), "{}", p[1].view.diff);
        assert!(!p[1].view.title.contains("creating"), "{}", p[1].view.title);
        let Op::WriteToml { path, text, .. } = &p[1].op else { panic!() };
        assert_eq!(path, "/h/sbxw.toml");
        let cfg = Config::parse(text).unwrap();
        assert_eq!((current_value(&cfg, "voice.tts_voice"), cfg.battery_warn_below), (json!("de_female"), 30));

        // A kits line on the missing file creates it too, from the defaults.
        let k = plan(&w, &[change(json!({"kind": "kits_line", "file": "daemon", "remove": ["sbxw:retro"]}))]).unwrap();
        assert!(k[0].view.diff.starts_with("--- /dev/null\n") && k[0].view.title.contains("creating /h/sbxw.toml"), "{:?}", k[0].view);
        assert_eq!(k[0].view.risk, Risk::Low);
    }

    #[test]
    fn a_voice_word_is_low_risk_and_writes_only_voice_words() {
        let w = world();
        let p = plan(&w, &[change(json!({"kind": "voice_word", "word": "Nginx", "fr": "engine ixe"}))]).unwrap();
        assert_eq!((p[0].view.kind.as_str(), p[0].view.risk), ("voice_word", Risk::Low));
        assert!(p[0].view.title.contains("Say 'nginx' as \"engine ixe\" in French"), "{}", p[0].view.title);
        assert!(p[0].view.diff.contains("+[voice.words]\n+nginx = { fr = \"engine ixe\" }\n"), "{}", p[0].view.diff);
        let Op::VoiceWords { text, words, .. } = &p[0].op else { panic!("{:?}", p[0].op) };
        assert!(text.starts_with("# mine\n"), "the rest of the file is kept: {text}");
        assert_eq!(words["nginx"].fr.as_deref(), Some("engine ixe"));
        assert_eq!(Config::parse(text).unwrap().voice.stt, "parakeet-tdt-0.6b-v3-int8");

        // Replacing the built-in, then removing that override, in one proposal.
        let both = plan(&w, &[change(json!({"kind": "voice_word", "word": "SBXW", "en": "sandbox W"})), change(json!({"kind": "voice_word", "word": "sbxw"}))]).unwrap();
        assert!(both[0].view.diff.contains("+sbxw = { en = \"sandbox W\" }"), "{}", both[0].view.diff);
        assert!(both[1].view.title.contains("the built-in one comes back"), "{}", both[1].view.title);
        let Op::VoiceWords { text, words, .. } = &both[1].op else { panic!() };
        assert!(words.is_empty() && !text.contains("words"), "{text}");

        // Refused: a bad word, a bad form, nothing to remove, an unknown field.
        assert!(plan(&w, &[change(json!({"kind": "voice_word", "word": "two words", "fr": "x"}))]).unwrap_err().contains("is not a word"));
        assert!(plan(&w, &[change(json!({"kind": "voice_word", "word": "x", "fr": "a\nb"}))]).unwrap_err().contains("one line"));
        assert!(plan(&w, &[change(json!({"kind": "voice_word", "word": "nginx"}))]).unwrap_err().contains("none to remove"));
        assert!(serde_json::from_value::<Change>(json!({"kind": "voice_word", "word": "x", "de": "y"})).is_err());
        // The same again: nothing to change.
        let mut again = world();
        again.files.get_mut(DAEMON_FILE).unwrap().raw.push_str("\n[voice.words]\nnginx = { fr = \"engine ixe\" }\n");
        assert!(plan(&again, &[change(json!({"kind": "voice_word", "word": "nginx", "fr": "engine ixe"}))]).unwrap_err().contains("already"));
        // An entry of another case is replaced, not doubled.
        let p = plan(&again, &[change(json!({"kind": "voice_word", "word": "NGINX", "fr": "n jinx"}))]).unwrap();
        assert!(p[0].view.diff.contains("-nginx = { fr = \"engine ixe\" }\n+nginx = { fr = \"n jinx\" }"), "{}", p[0].view.diff);

        // A missing daemon file is created.
        let mut gone = world();
        gone.files.insert(DAEMON_FILE.into(), TomlFile { path: "/h/sbxw.toml".into(), raw: String::new(), missing: true });
        let p = plan(&gone, &[change(json!({"kind": "voice_word", "word": "nginx", "en": "engine X"}))]).unwrap();
        assert!(p[0].view.diff.starts_with("--- /dev/null\n"), "{}", p[0].view.diff);
        assert!(p[0].view.title.contains("creating /h/sbxw.toml"), "{}", p[0].view.title);
        let Op::VoiceWords { text, .. } = &p[0].op else { panic!() };
        assert_eq!(text, "[voice.words]\nnginx = { en = \"engine X\" }\n");
    }

    #[test]
    fn network_hosts_are_bare_names() {
        let w = world();
        for bad in ["localhost", "10.0.0.1", "*.evil.com", "a.com:22", "box.internal"] {
            assert!(plan(&w, &[change(json!({"kind": "network_allow", "add": [bad]}))]).is_err(), "{bad}");
        }
        let p = plan(&w, &[change(json!({"kind": "network_allow", "add": ["Mirror.Example.com"]}))]).unwrap();
        let Op::WriteToml { network_added, text, .. } = &p[0].op else { panic!() };
        assert_eq!(network_added, &["mirror.example.com"]);
        assert!(Config::parse(text).unwrap().network_allow.contains(&"mirror.example.com".to_string()));
    }

    #[test]
    fn an_mcp_server_never_carries_a_secret() {
        let w = world();
        let spec = |headers: Value| change(json!({"kind": "mcp_server", "spec": {"name": "acme", "kind": "remote", "url": "https://mcp.acme.com/mcp", "headers": headers}}));
        for bad in [json!(["Authorization: Bearer abc123"]), json!(["X-Api-Key: whatever"]), json!(["X-Thing: sk-ant-api03-AbCdEf1234567890"])] {
            let e = plan(&w, &[spec(bad.clone())]).unwrap_err();
            assert!(e.contains("MCP dialog"), "{bad}: {e}");
        }
        let ok = plan(&w, &[spec(json!(["Authorization: Bearer ${api-key}", "Accept: application/json"]))]).unwrap();
        assert!(ok[0].view.diff.contains("+ sbx mcp add acme --url=https://mcp.acme.com/mcp") && ok[0].view.diff.contains("${api-key}"), "{}", ok[0].view.diff);
        let replace = plan(&w, &[change(json!({"kind": "mcp_server", "spec": {"name": "github", "kind": "remote", "url": "https://x"}}))]).unwrap();
        assert!(replace[0].view.diff.starts_with("- sbx mcp rm github\n"));
        assert!(plan(&w, &[change(json!({"kind": "mcp_remove", "name": "nope"}))]).is_err());
    }

    #[test]
    fn a_stdio_server_shows_its_host_command_and_needs_an_ack() {
        let w = world();
        let p = plan(&w, &[change(json!({"kind": "mcp_server", "spec": {"name": "pw", "kind": "stdio", "command": " npx ", "args": ["-y", "", "@playwright/mcp@latest", "a b"], "dir": "/srv/my dir"}}))]).unwrap();
        assert_eq!(p[0].view.host_command.as_deref(), Some("cd '/srv/my dir' && npx -y @playwright/mcp@latest 'a b'"));
        let remote = plan(&w, &[change(json!({"kind": "mcp_server", "spec": {"name": "r", "kind": "remote", "url": "https://x"}}))]).unwrap();
        assert_eq!(remote[0].view.host_command, None);

        let mut b = Book::default();
        let views: Vec<ChangeView> = p.into_iter().map(|x| x.view).collect();
        let prop = b.propose("j1".into(), "pw", vec![], views, 1).unwrap();
        assert!(prop.needs_ack() && prop.ui_view()["needs_ack"] == true);
        assert_eq!(prop.ui_view()["changes"][0]["host_command"], "cd '/srv/my dir' && npx -y @playwright/mcp@latest 'a b'");
        assert!(b.begin("j1", Via::Click, false).unwrap_err().to_string().contains("acknowledged"));
        assert!(matches!(b.begin("j1", Via::Voice, true), Err(ApproveError::Conflict(_))), "never by voice");
        b.begin("j1", Via::Click, true).unwrap();
        let rv: Vec<ChangeView> = remote.into_iter().map(|x| x.view).collect();
        let mut b2 = Book::default();
        let rp = b2.propose("j2".into(), "r", vec![], rv, 1).unwrap();
        assert!(!rp.needs_ack() && rp.ui_view().get("needs_ack").is_none());
        assert!(rp.ui_view()["changes"][0].get("host_command").is_none());
        b2.begin("j2", Via::Click, false).unwrap();
    }

    #[test]
    fn the_unified_diff_has_hunks_with_context() {
        let old: String = (1..=20).flat_map(|i| [format!("l{i}"), "\n".into()]).collect();
        let new = old.replace("l5\n", "five\n").replace("l18\n", "");
        let d = unified(&old, &new, "a", "b");
        assert!(d.starts_with("--- a\n+++ b\n@@ -2,7 +2,7 @@\n l2\n"), "{d}");
        assert!(d.contains("-l5\n+five\n") && d.contains("-l18\n"), "{d}");
        assert_eq!(d.matches("@@").count(), 4, "two hunks: {d}");
        assert_eq!(unified("x", "x", "a", "b"), "");
        assert_eq!(unified("", "new\n", "/dev/null", "b"), "--- /dev/null\n+++ b\n@@ -0,0 +1,1 @@\n+new\n");
    }

    #[test]
    fn one_pending_voice_only_for_low_risk_and_settled_once() {
        let mut b = Book::default();
        let high = vec![ChangeView { kind: "kit_remove".into(), title: "t".into(), diff: "".into(), risk: Risk::High, ..Default::default() }];
        let p = b.propose("j1".into(), "remove it", vec![], high, 1).unwrap();
        assert_eq!((p.risk, p.state), (Risk::High, State::Pending));
        assert!(b.propose("j2".into(), "again", vec![], vec![], 2).unwrap_err().contains("still waiting"));
        assert!(matches!(b.begin("j1", Via::Voice, false), Err(ApproveError::Conflict(_))));
        b.begin("j1", Via::Click, false).unwrap();
        assert!(b.begin("j1", Via::Click, false).is_err(), "once");
        assert!(b.refuse("j1", None, 3).is_err(), "not while applying");
        let done = b.finish("j1", Via::Click, vec![ChangeResult { ok: false, message: "boom".into(), restart: false }], None, 4).unwrap();
        assert_eq!((done.state, done.error.as_deref()), (State::Failed, Some("boom")));
        assert!(b.propose("j2".into(), " ", vec![], vec![], 5).unwrap_err().contains("summary"));
        b.propose("j2".into(), "s", vec![], vec![], 5).unwrap();
        assert_eq!(b.refuse("j2", Some(" no ".into()), 6).unwrap().reason.as_deref(), Some("no"));
        assert!(matches!(b.begin("nope", Via::Click, false), Err(ApproveError::NotFound(_))));
    }

    // What the desktop app writes in builtin/ (copies, see testdata/desktop-builtin/README.md).
    const TOPBAR: &str = include_str!("../testdata/desktop-builtin/screens/topbar.luau");
    const SANDBOXES: &str = include_str!("../testdata/desktop-builtin/screens/sandboxes.luau");
    const JARVIS: &str = include_str!("../testdata/desktop-builtin/screens/jarvis.luau");
    const INTENTS: &str = include_str!("../testdata/desktop-builtin/intents.json");

    fn desktop_world() -> World {
        let mut w = world();
        w.desktop = Desktop {
            present: true,
            builtin_dir: "/h/.sbxw/desktop/builtin".into(),
            screens_dir: "/h/.sbxw/desktop/screens".into(),
            builtin: [("topbar", TOPBAR), ("sandboxes", SANDBOXES), ("jarvis", JARVIS)].into_iter().map(|(n, t)| (n.to_string(), t.to_string())).collect(),
            overrides: BTreeMap::new(),
            intents: parse_intents(INTENTS).unwrap(),
            intents_error: None,
        };
        w
    }

    fn names(text: &str) -> Vec<String> {
        scan_intents(text, &desktop_world().desktop.intents).into_iter().map(|i| i.name.clone()).collect()
    }

    fn screen(name: &str, content: &str) -> Change {
        change(json!({"kind": "desktop_screen", "name": name, "content": content}))
    }

    #[test]
    fn a_desktop_screen_replaces_the_built_in_then_changes_the_override() {
        let w = desktop_world();
        let next = TOPBAR.replace("text{ \"sbxw\",", "text{ \"Open\",").replace("intent(\"toggle_jarvis_panel\")", "intent(\"stop_sandbox\", { sandbox = \"x\" })");
        let p = plan(&w, &[screen("topbar", &next)]).unwrap();
        let v = &p[0].view;
        assert_eq!((v.kind.as_str(), v.risk), ("desktop_screen", Risk::High));
        assert!(v.title.contains("replaces the built-in topbar screen") && v.title.contains("/h/.sbxw/desktop/screens/topbar.luau"), "{}", v.title);
        assert!(v.diff.contains("--- /h/.sbxw/desktop/builtin/screens/topbar.luau\n+++ /h/.sbxw/desktop/screens/topbar.luau\n"), "{}", v.diff);
        assert!(v.diff.contains("-      text{ \"sbxw\", weight") && v.diff.contains("+      text{ \"Open\", weight"), "{}", v.diff);
        assert!(v.diff.contains("#   stop_sandbox (risk: direct), NEW") && v.diff.contains("#   set_layout (risk: direct)\n"), "{}", v.diff);
        assert!(v.diff.contains("# This list is a hint: a screen can hide an intent name; read the diff.\n") && !v.diff.contains("hides its name"), "{}", v.diff);
        assert_eq!(
            v.intents.as_ref().unwrap(),
            &vec![IntentHint { name: "set_layout".into(), risk: "direct".into(), new: false }, IntentHint { name: "stop_sandbox".into(), risk: "direct".into(), new: true }]
        );
        let Op::WriteScreen { name, content } = &p[0].op else { panic!("{:?}", p[0].op) };
        assert_eq!((name.as_str(), content.as_str()), ("topbar", next.as_str()));

        // With an override in place: the diff is against it, the title says "your".
        let mut o = desktop_world();
        o.desktop.overrides.insert("topbar".into(), next.clone());
        let again = format!("{next}-- tidy\n");
        let p = plan(&o, &[screen("topbar", &again)]).unwrap();
        assert!(p[0].view.title.contains("changes your topbar screen"), "{}", p[0].view.title);
        assert!(p[0].view.diff.contains("--- /h/.sbxw/desktop/screens/topbar.luau\n+++ /h/.sbxw/desktop/screens/topbar.luau\n") && p[0].view.diff.contains("+-- tidy\n"), "{}", p[0].view.diff);
        assert!(p[0].view.diff.contains("#   stop_sandbox (risk: direct)\n"), "not new against the override: {}", p[0].view.diff);
        assert!(plan(&o, &[screen("topbar", &next)]).unwrap_err().contains("already is exactly this"));
        assert!(plan(&w, &[screen("topbar", TOPBAR)]).unwrap_err().contains("already is exactly this"));

        // Two in a row in one proposal: the second sees the first.
        let p = plan(&w, &[screen("topbar", &next), screen("topbar", &again)]).unwrap();
        assert!(p[1].view.title.contains("changes your topbar screen"), "{}", p[1].view.title);
    }

    #[test]
    fn removing_a_screen_override_restores_the_built_in() {
        let mut w = desktop_world();
        assert!(plan(&w, &[change(json!({"kind": "desktop_screen_remove", "name": "topbar"}))]).unwrap_err().contains("already uses its built-in"));
        w.desktop.overrides.insert("topbar".into(), "return { render = function() return text{ \"mine\" } end }\n".into());
        let p = plan(&w, &[change(json!({"kind": "desktop_screen_remove", "name": "topbar"}))]).unwrap();
        let v = &p[0].view;
        assert_eq!((v.kind.as_str(), v.risk), ("desktop_screen_remove", Risk::High));
        assert!(v.title.contains("restores the built-in"), "{}", v.title);
        assert!(v.diff.contains("--- /h/.sbxw/desktop/screens/topbar.luau\n+++ /h/.sbxw/desktop/builtin/screens/topbar.luau\n"), "{}", v.diff);
        assert!(v.diff.contains("#   set_layout (risk: direct), NEW") && v.diff.contains("#   toggle_jarvis_panel (risk: direct), NEW"), "the built-in emits what the override did not: {}", v.diff);
        assert!(matches!(&p[0].op, Op::RemoveScreen { name } if name == "topbar"));
    }

    #[test]
    fn a_desktop_screen_is_checked_before_anything_is_planned() {
        let w = desktop_world();
        let e = plan(&w, &[screen("settings", "return {}\n")]).unwrap_err();
        assert!(e.contains("no 'settings' screen") && e.contains("jarvis, sandboxes, topbar") && e.contains("no place to show a new screen"), "{e}");
        for bad in ["", "Topbar", "../topbar", "top bar", "a/b", &"a".repeat(41)] {
            assert!(plan(&w, &[screen(bad, "return {}\n")]).unwrap_err().contains("is not a screen name"), "{bad:?}");
        }
        assert!(plan(&w, &[screen("topbar", "a\0b")]).unwrap_err().contains("NUL"));
        assert!(plan(&w, &[screen("topbar", &"x".repeat(MAX_SCREEN + 1))]).unwrap_err().contains("at most 64 KB"));
        assert!(plan(&w, &[screen("topbar", &"x".repeat(MAX_SCREEN))]).is_ok());
        assert!(plan(&w, &[screen("topbar", " \n")]).unwrap_err().contains("remove your override"));
        assert!(serde_json::from_value::<Change>(json!({"kind": "desktop_screen", "name": "topbar"})).is_err(), "content is required");
        assert!(serde_json::from_value::<Change>(json!({"kind": "desktop_screen", "name": "topbar", "content": "x", "path": "/etc/x"})).is_err(), "no path from the agent");

        // The app never ran: said in plain words, nothing proposed.
        let mut none = world();
        none.desktop.builtin_dir = "/h/.sbxw/desktop/builtin".into();
        let e = plan(&none, &[screen("topbar", "return {}\n")]).unwrap_err();
        assert!(e.contains("desktop app has not run on this machine yet") && e.contains("/h/.sbxw/desktop/builtin"), "{e}");
        assert!(plan(&none, &[change(json!({"kind": "desktop_screen_remove", "name": "topbar"}))]).unwrap_err().contains("has not run"));
    }

    #[test]
    fn a_desktop_screen_is_never_approved_by_voice() {
        let p = plan(&desktop_world(), &[screen("topbar", "return { render = function() return text{ \"hi\" } end }\n")]).unwrap();
        let mut b = Book::default();
        b.propose("j1".into(), "a calmer top bar", vec![], p.into_iter().map(|x| x.view).collect(), 1).unwrap();
        assert!(matches!(b.begin("j1", Via::Voice, false), Err(ApproveError::Conflict(_))));
        b.begin("j1", Via::Click, false).unwrap();
    }

    /// Approval compares the views: a moved override or built-in changes the
    /// base, even where the diff would not show it.
    #[test]
    fn a_changed_base_changes_the_view() {
        let w = desktop_world();
        let next = "return { render = function() return text{ \"hi\" } end }\n";
        let then = plan(&w, &[screen("topbar", next)]).unwrap();
        let mut o = w.clone();
        o.desktop.overrides.insert("topbar".into(), TOPBAR.into());
        let now = plan(&o, &[screen("topbar", next)]).unwrap();
        assert_eq!(then[0].view.diff.lines().filter(|l| !l.starts_with("---") && !l.starts_with("+++")).collect::<Vec<_>>(), now[0].view.diff.lines().filter(|l| !l.starts_with("---") && !l.starts_with("+++")).collect::<Vec<_>>(), "same lines changed");
        assert_ne!(then[0].view, now[0].view);
        assert_ne!(then[0].view.base, now[0].view.base);

        // An override in place, and the built-in moves under it.
        let mut b = o.clone();
        b.desktop.builtin.insert("topbar".into(), format!("{TOPBAR}-- 2\n"));
        let (x, y) = (plan(&o, &[screen("topbar", next)]).unwrap(), plan(&b, &[screen("topbar", next)]).unwrap());
        assert_eq!(x[0].view.diff, y[0].view.diff);
        assert_ne!(x[0].view, y[0].view, "the base differs");
    }

    #[test]
    fn intents_json_is_read_in_its_shapes() {
        // The app's own export: {"intents": [{name, args, risk, asks?, doc}]}.
        let real = parse_intents(INTENTS).unwrap();
        assert_eq!(real.len(), 15);
        assert_eq!(real.iter().find(|i| i.name == "approve_proposal").unwrap().risk, "approval");
        assert_eq!(real.iter().find(|i| i.name == "stop_sandbox").unwrap().risk, "direct");
        let list = parse_intents(r#"{"intents": [{"name": "select", "risk": {"kind": "approval", "asks": "x"}}, {"name": "stop_sandbox"}]}"#).unwrap();
        assert_eq!(list, vec![IntentInfo { name: "select".into(), risk: "approval".into() }, IntentInfo { name: "stop_sandbox".into(), risk: "unknown".into() }]);
        assert_eq!(parse_intents(r#"{"open_terminal": {"risk": "direct"}}"#).unwrap()[0].name, "open_terminal");
        assert_eq!(parse_intents(r#"[{"name": "select", "risk": "direct"}]"#).unwrap()[0].name, "select");
        assert!(parse_intents("[").is_err() && parse_intents("3").is_err());
    }

    /// The built-in screens: every intent they emit is found by name, and
    /// nothing in them is flagged as hiding one.
    #[test]
    fn the_built_in_screens_read_clean() {
        assert_eq!(names(TOPBAR), ["set_layout", "toggle_jarvis_panel"]);
        assert_eq!(names(SANDBOXES), ["open_terminal", "select", "start_sandbox", "stop_sandbox"]);
        assert_eq!(names(JARVIS), ["approve_proposal", "open_link", "read_aloud", "send_to_jarvis", "start_jarvis", "talk_start", "talk_stop", "toggle_jarvis_panel"]);
        for (n, t) in [("topbar", TOPBAR), ("sandboxes", SANDBOXES), ("jarvis", JARVIS)] {
            assert_eq!(hidden_intents(t), Vec::<String>::new(), "{n}");
        }
    }

    #[test]
    fn literals_count_wherever_they_are_written_and_comments_do_not() {
        assert_eq!(names("intent('select', {}) -- stop_sandbox, open_link\n--[[ start_jarvis ]] x = 1"), ["select"]);
        assert_eq!(names("return { kind = \"intent\", name = 'stop_sandbox', args = {} }"), ["stop_sandbox"]);
        assert_eq!(names("local n = [[stop_sandbox]] local m = [==[\nselect]==] local o = \"\\115tart_jarvis\" local p = `open_link`"), ["open_link", "select", "start_jarvis", "stop_sandbox"]);
        assert_eq!(names("local x = \"preselect stop_sandbox_all\""), Vec::<String>::new(), "whole literals only");
    }

    #[test]
    fn an_intent_built_out_of_sight_is_flagged() {
        let flagged = |t: &str| hidden_intents(t);
        // intent( with a computed name, or intent passed around.
        assert_eq!(flagged("return intent(n, {})"), ["line 1: intent(...) is called with a name that is not written as plain text"]);
        assert_eq!(flagged("return intent(\"stop_\" .. \"sandbox\", {})").len(), 1);
        assert_eq!(flagged("local f = intent\nreturn f('select')"), ["line 1: intent is used as a value, so where its calls go cannot be followed"]);
        // A table the core takes for an intent, its name computed.
        for t in ["return { kind = \"intent\", name = n }", "return {kind='intent',name=n}", "return { [\"kind\"] = \"intent\", name = \"stop_\" .. x }", "return {\n  kind   =   'intent' ;\n  args = { name = 'select' } }"] {
            assert_eq!(flagged(t), ["line 1: a table with kind = \"intent\" has a name that is not written as plain text"], "{t}");
        }
        // ... unless its name is a literal: then it is counted, not flagged.
        assert!(flagged("return { kind = \"intent\", name = \"stop_sandbox\", args = { sandbox = \"x\" } }").is_empty());
        assert!(flagged("return { [\"kind\"] = \"intent\", [\"name\"] = \"select\" }").is_empty());
        // A kind that is computed, a computed key, fields set one by one.
        assert_eq!(flagged("return { kind = \"int\" .. \"ent\", name = \"select\" }"), ["line 1: a table's kind is computed, not written as plain text"]);
        assert_eq!(flagged("return { [k] = v }"), ["line 1: a table key is computed ([...] = ...), so a kind or a name can hide in it"]);
        assert_eq!(flagged("local t = {}\nt.kind = k\nt[\"name\"] = n"), ["line 2: a table's kind is set field by field", "line 3: a table's name is set field by field"]);
        assert_eq!(flagged("rawset(t, k, v)"), ["line 1: rawset(...) sets fields this scan does not see"]);
        assert_eq!(flagged("local s = \"never ends"), ["line 1: a string or a comment does not end, so the rest of the file cannot be read"]);
        // Not flagged: ordinary screen code.
        assert!(flagged("by_ws[w] = {}\nlog[#log + 1] = text{ \"x\" }\nlocal a = t.name\nif x.kind == \"y\" then end\nreturn { intent = 1, row{ key = \"k\" } }").is_empty());
        // Named honestly in the diff, with the line, and the hint line always.
        let p = plan(&desktop_world(), &[screen("topbar", "local n = \"stop_\" .. \"sandbox\"\nreturn { actions = { a = function() return { kind = \"intent\", name = n } end } }\n")]).unwrap();
        let d = &p[0].view.diff;
        assert!(d.contains("# No intent of the app is written by name in this screen.\n# This screen builds an intent in a way that hides its name:\n#   line 2: a table with kind = \"intent\" has a name"), "{d}");
        assert!(d.contains("# This list is a hint: a screen can hide an intent name; read the diff.\n"), "{d}");
    }

    #[test]
    fn too_many_changes_are_refused() {
        let many: Vec<Change> = (0..=MAX_CHANGES).map(|_| change(json!({"kind": "setting", "key": "keep_awake", "value": "off"}))).collect();
        assert!(plan(&world(), &many).unwrap_err().contains("at most 20"));
        assert!(plan(&world(), &[]).is_err());
    }
}
