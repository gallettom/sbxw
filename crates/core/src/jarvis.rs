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
use crate::kits;
use crate::mcp::McpSpec;
use crate::my_skills::{self, KitFile};
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
    Ok(())
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
    /// The daemon config's `network_allow`.
    NetworkAllow {
        #[serde(default)]
        add: Vec<String>,
        #[serde(default)]
        remove: Vec<String>,
    },
    McpServer { spec: McpServerSpec },
    McpRemove { name: String },
}

impl Change {
    pub fn kind(&self) -> &'static str {
        match self {
            Change::KitWrite { .. } => "kit_write",
            Change::KitRemove { .. } => "kit_remove",
            Change::KitsLine { .. } => "kits_line",
            Change::Setting { .. } => "setting",
            Change::NetworkAllow { .. } => "network_allow",
            Change::McpServer { .. } => "mcp_server",
            Change::McpRemove { .. } => "mcp_remove",
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
    McpAdd { replace: bool, spec: McpSpec },
    McpRm { name: String },
}

/// One change, planned.
#[derive(Debug, Clone)]
pub struct Planned {
    pub view: ChangeView,
    pub op: Op,
}

/// What the person reads about one change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeView {
    pub kind: String,
    pub title: String,
    pub diff: String,
    pub risk: Risk,
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
    let view = |title: String, diff: String, risk: Risk| ChangeView { kind: c.kind().into(), title, diff, risk };
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
                    Some(b) if kits::BUNDLED.contains(&b) => k.clone(),
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
            let diff = unified(&f.raw, &text, &f.path, &f.path);
            let title = format!("Kits of {} ({}): {}", file, f.path, change_words(&add, &remove));
            w.files.insert(file.clone(), TomlFile { path: f.path.clone(), raw: text });
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
            let diff = unified(&f.raw, &text, &f.path, &f.path);
            let shown = if value.is_null() { "the default".to_string() } else { value.to_string() };
            w.files.insert(DAEMON_FILE.into(), TomlFile { path: f.path.clone(), raw: text.clone() });
            Ok(Planned {
                view: view(format!("Set {key} to {shown} (takes effect when the daemon restarts)"), diff, Risk::Low),
                op: Op::WriteToml { path: f.path, text, network_added: vec![] },
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
            let diff = unified(&f.raw, &text, &f.path, &f.path);
            w.files.insert(DAEMON_FILE.into(), TomlFile { path: f.path.clone(), raw: text.clone() });
            Ok(Planned {
                view: view(format!("network_allow: {} (for sandboxes created after the daemon restarts)", change_words(&add, &remove)), diff, Risk::High),
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
            Ok(Planned { view: view(title, diff, Risk::High), op: Op::McpAdd { replace, spec: s } })
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
    }
}

fn daemon_file(w: &World) -> Result<TomlFile, String> {
    w.files.get(DAEMON_FILE).cloned().ok_or_else(|| "the daemon runs without an sbxw.toml, so there is no file to change".into())
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
    pub fn begin(&mut self, id: &str, via: Via) -> Result<Proposal, ApproveError> {
        let p = self.get(id).ok_or_else(|| ApproveError::NotFound(format!("no proposal '{id}'")))?.clone();
        if p.state != State::Pending || self.applying.as_deref() == Some(id) {
            return Err(ApproveError::Conflict(format!("proposal {id} is not waiting for approval")));
        }
        if via == Via::Voice && p.risk == Risk::High {
            return Err(ApproveError::Conflict("a high-risk proposal is approved with a click in the dialog, not by voice".into()));
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
        w.files.insert(DAEMON_FILE.into(), TomlFile { path: "/h/sbxw.toml".into(), raw: "# mine\nkits = [\"sbxw:codemap\"]\n\n[voice]\nstt = \"parakeet-tdt-0.6b-v3-int8\"\n".into() });
        w.files.insert("myapp".into(), TomlFile { path: "/p/sbxw.toml".into(), raw: String::new() });
        w
    }

    fn change(v: Value) -> Change {
        serde_json::from_value(v).unwrap()
    }

    fn kit_write(kit: &str, files: Value) -> Change {
        change(json!({"kind": "kit_write", "kit": kit, "files": files}))
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
        let high = vec![ChangeView { kind: "kit_remove".into(), title: "t".into(), diff: "".into(), risk: Risk::High }];
        let p = b.propose("j1".into(), "remove it", vec![], high, 1).unwrap();
        assert_eq!((p.risk, p.state), (Risk::High, State::Pending));
        assert!(b.propose("j2".into(), "again", vec![], vec![], 2).unwrap_err().contains("still waiting"));
        assert!(matches!(b.begin("j1", Via::Voice), Err(ApproveError::Conflict(_))));
        b.begin("j1", Via::Click).unwrap();
        assert!(b.begin("j1", Via::Click).is_err(), "once");
        assert!(b.refuse("j1", None, 3).is_err(), "not while applying");
        let done = b.finish("j1", Via::Click, vec![ChangeResult { ok: false, message: "boom".into(), restart: false }], None, 4).unwrap();
        assert_eq!((done.state, done.error.as_deref()), (State::Failed, Some("boom")));
        assert!(b.propose("j2".into(), " ", vec![], vec![], 5).unwrap_err().contains("summary"));
        b.propose("j2".into(), "s", vec![], vec![], 5).unwrap();
        assert_eq!(b.refuse("j2", Some(" no ".into()), 6).unwrap().reason.as_deref(), Some("no"));
        assert!(matches!(b.begin("nope", Via::Click), Err(ApproveError::NotFound(_))));
    }

    #[test]
    fn too_many_changes_are_refused() {
        let many: Vec<Change> = (0..=MAX_CHANGES).map(|_| change(json!({"kind": "setting", "key": "keep_awake", "value": "off"}))).collect();
        assert!(plan(&world(), &many).unwrap_err().contains("at most 20"));
        assert!(plan(&world(), &[]).is_err());
    }
}
