//! Reading and writing sbx's own `sbxenv.yaml` **environment files**.
//!
//! An environment file is a project's setup in sbx's format: agent, workspace,
//! extra mounts, kits, env vars, secrets, MCP servers, ports. `sbx env run`
//! provisions a sandbox from one.
//!
//! sbxw **exports** one (`sbxw env export`) and **runs** on one (`sbxw env
//! run`): sbx creates the sandbox, sbxw does what the format cannot express.
//! Three sbx behaviours make `sbxw env run` a pipeline, not a passthrough:
//!
//! * **Re-running doesn't re-provision.** On an existing sandbox `sbx env run`
//!   applies only `env` and MCP changes; workspaces, kits, ports, secrets and
//!   `sandboxOptions` need a remove and recreate. So sbxw calls
//!   `sbx env create` **once** and keeps its own idempotent pipeline (policy,
//!   hooks, credentials, ports, `/etc/hosts`) for every run.
//! * **Ports are all-or-nothing.** If a port can't be published, creation fails
//!   *and removes the new sandbox*, leaving provisioned secrets behind. No CLI
//!   flag overrides a port. Since sbx 0.53 a later entry for the same sandbox
//!   port replaces an earlier one whole, across merged files too (other lists
//!   still concatenate), but a busy host port still fails the create. So sbxw
//!   checks host ports *before* calling sbx and rewrites the document (see
//!   `Loaded::set_ports`).
//! * **Half of sbxw isn't in the format.** The egress allowlist is `sbx policy`,
//!   not a file field; the browser terminal, `/etc/hosts` aliases and OAuth
//!   injection are sbxw's own. Export writes those omissions into the header
//!   (`notes`); run reads them from `sbxw.toml`, the fallback layer under the file.
//!
//! And three that decide how sbx *reads* a file:
//!
//! * **The project file is `sbxenv.yaml`**, not hidden; a directory means that
//!   name alone. `~/.sbxenv.yaml` is merged beneath it as shared defaults.
//! * **`${VAR}` is not expanded.** The file's expressions are
//!   `${{ env.projectDir }}`, `${{ env.fileDir }}` and `${{ env.args.NAME }}`
//!   (supplied by `--env-arg` or an `args:` default).
//! * **A relative path resolves against the file that declares it**, and a
//!   file with no `workspace:` mounts nothing.
//!
//! The format is `schemaVersion: "1"` and **sbx's loader rejects unknown
//! fields**, so `render` emits only the documented set. Reading is more
//! tolerant, and rewriting goes through the parsed document, not `EnvFile`, so
//! unmodelled fields (`secrets`, `bindings`, `registries`, `mcp`,
//! `sandboxOptions`) survive the round trip.

use anyhow::{bail, Context, Result};
use serde_norway::Value;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// One published port, as the environment file spells it.
#[derive(Debug, Clone, PartialEq)]
pub struct Port {
    pub sandbox: u16,
    /// `None`: key absent, which sbx reads as "any free host port", the one
    /// way that cannot lose a race with a host process.
    pub host: Option<u16>,
    /// Host interface. `None` is sbx's default (IPv4/IPv6 loopback); sbxw sets
    /// it only in `ip_per_app` mode (one loopback address per app).
    pub host_ip: Option<String>,
    /// `None` is sbx's default (`tcp4`). Kept so a rewrite can't turn `udp`
    /// into TCP.
    pub protocol: Option<String>,
}

impl Port {
    /// The same published port as `other`, the way sbx 0.53 matches entries:
    /// sandbox port and transport (`tcp4`, `tcp6` and no protocol are all TCP).
    pub fn same_port(&self, other: &Port) -> bool {
        let udp = |p: &Port| p.protocol.as_deref().is_some_and(|x| x.to_ascii_lowercase().starts_with("udp"));
        self.sandbox == other.sandbox && udp(self) == udp(other)
    }
}

impl From<&crate::ports::PortMapping> for Port {
    /// A published mapping as the file spells it. sbx's defaults (loopback,
    /// `tcp4`; see `protocol_is_default`) are dropped. `sbxw env run` and the
    /// Env panel both go through here, so they agree.
    fn from(m: &crate::ports::PortMapping) -> Self {
        Port {
            sandbox: m.sandbox_port,
            host: Some(m.host_port),
            host_ip: (!m.host_ip.is_empty() && m.host_ip != "127.0.0.1").then(|| m.host_ip.clone()),
            protocol: (!protocol_is_default(&m.proto, &m.host_ip)).then(|| m.proto.clone()),
        }
    }
}

/// Would sbx pick `proto` anyway, for a port bound on `host_ip`?
///
/// sbx defaults to `tcp4`, so a dual-stack `tcp` must be written out. An
/// explicit IPv4 address binds over IPv4 whatever the protocol says, which
/// covers every port sbxw publishes itself.
pub fn protocol_is_default(proto: &str, host_ip: &str) -> bool {
    match proto {
        "" | "tcp4" => true,
        "tcp" => host_ip.parse::<std::net::Ipv4Addr>().is_ok(),
        _ => false,
    }
}

/// Everything `render` needs, resolved.
///
/// `additionalWorkspaces` come from the config's `workspaces` (with 0.51's
/// `target`); `sbxw up --ro` mounts are argv and stay out. Read back via
/// `Spec::additional`.
#[derive(Default)]
pub struct EnvFile {
    pub name: String,
    pub agent: String,
    pub workspace: String,
    /// `off`, `readonly` or `readwrite`; `None` leaves sbx's default
    /// (read-only sharing).
    pub skills: Option<String>,
    pub kits: Vec<String>,
    /// Kit-declared host mounts: sandbox target → host directory, relative
    /// to the file (sbx 0.50 `mounts`).
    pub mounts: BTreeMap<String, String>,
    /// `additionalWorkspaces`: path relative to the file, target, read-only.
    pub additional: Vec<(String, Option<String>, bool)>,
    /// `sandboxOptions`, each value already a YAML scalar (`4`, `8g`, `false`).
    pub sandbox_options: Vec<(String, String)>,
    pub env: BTreeMap<String, String>,
    pub ports: Vec<Port>,
    /// What sbxw does that the file cannot express; written as header comments.
    pub notes: Vec<String>,
}

impl EnvFile {
    /// The file as text. Deterministic, so a re-export diffs to nothing when
    /// nothing changed.
    pub fn render(&self, generator: &str) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "# Generated by {generator} — `sbxw env export`.");
        out.push_str(
            "#\n\
             # sbxw provisions from sbxw.toml; this is the same project in sbx's own\n\
             # format, so `sbx env run` reproduces it on a machine without sbxw.\n\
             # Nothing keeps the two in step — re-export after editing sbxw.toml.\n\
             #\n\
             # Paths are relative to this file (sbx 0.43+), so it keeps working on\n\
             # any machine with the same layout below this directory.\n\
             #\n\
             # Keep this file OUTSIDE the workspace. The agent can write anywhere in a\n\
             # direct-mounted directory, and a file it can edit is a file it can use to\n\
             # widen its own next sandbox.\n",
        );
        if !self.notes.is_empty() {
            out.push_str("#\n# What sbxw does that an environment file cannot carry:\n");
            for note in &self.notes {
                for (i, line) in note.lines().enumerate() {
                    let bullet = if i == 0 { "#   * " } else { "#     " };
                    let _ = writeln!(out, "{bullet}{line}");
                }
            }
        }

        // Quoted: bare 1 is an integer; sbx's loader wants the string.
        out.push_str("\nschemaVersion: \"1\"\n");
        let _ = writeln!(out, "name: {}", scalar(&self.name));
        let _ = writeln!(out, "agent: {}", scalar(&self.agent));
        let _ = writeln!(out, "workspace: {}", scalar(&self.workspace));
        if let Some(mode) = &self.skills {
            let _ = writeln!(out, "skills: {}", scalar(mode));
        }

        if !self.kits.is_empty() {
            out.push_str("\nkits:\n");
            for k in &self.kits {
                let _ = writeln!(out, "  - {}", scalar(k));
            }
        }

        if !self.mounts.is_empty() {
            out.push_str("\nmounts:\n");
            for (target, host) in &self.mounts {
                let _ = writeln!(out, "  {}: {}", scalar(target), scalar(host));
            }
        }

        if !self.additional.is_empty() {
            out.push_str("\nadditionalWorkspaces:\n");
            for (path, target, ro) in &self.additional {
                let _ = writeln!(out, "  - path: {}", scalar(path));
                if let Some(t) = target {
                    let _ = writeln!(out, "    target: {}", scalar(t));
                }
                if *ro {
                    out.push_str("    readOnly: true\n");
                }
            }
        }

        if !self.sandbox_options.is_empty() {
            out.push_str("\nsandboxOptions:\n");
            for (k, v) in &self.sandbox_options {
                let _ = writeln!(out, "  {k}: {v}");
            }
        }

        if !self.env.is_empty() {
            out.push_str("\nenv:\n");
            for (k, v) in &self.env {
                let _ = writeln!(out, "  {}: {}", scalar(k), scalar(v));
            }
        }

        if !self.ports.is_empty() {
            out.push_str("\nports:\n");
            for p in &self.ports {
                let _ = writeln!(out, "  - sandbox: {}", p.sandbox);
                if let Some(h) = p.host {
                    let _ = writeln!(out, "    host: {h}");
                }
                if let Some(ip) = &p.host_ip {
                    let _ = writeln!(out, "    hostIP: {}", scalar(ip));
                }
                if let Some(proto) = &p.protocol {
                    let _ = writeln!(out, "    protocol: {}", scalar(proto));
                }
            }
        }

        out
    }
}

/// A YAML scalar that reads back as the string given.
///
/// Quoted by default: `yes`, `off`, `null`, `8.0` or `~` would read as
/// non-strings. Only unambiguous identifier-like tokens are left bare.
///
/// Single quotes: their only escape is `''`, so backslashes in a Windows path
/// survive. (sbx's `${{ … }}` pass ignores quotes; sbxw exports no expressions.)
fn scalar(s: &str) -> String {
    let plain = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'))
        && s.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_' || c == '.' || c == '/')
        && !matches!(
            s.to_ascii_lowercase().as_str(),
            "y" | "n" | "yes" | "no" | "true" | "false" | "on" | "off" | "null" | "none"
        );
    if plain {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "''"))
}

// ── Reading ──────────────────────────────────────────────────────────────────

/// The one name sbx reads from a project directory.
pub const FILE_NAME: &str = "sbxenv.yaml";

/// User-level base in the home directory, merged *beneath* every project file.
/// The only place the hidden name is read.
pub const USER_FILE_NAME: &str = ".sbxenv.yaml";

/// Names sbx does not read from a project directory. Only used to explain a
/// miss: a directory holding one and no `sbxenv.yaml` gets a "rename it" error,
/// not "no environment file here".
const RETIRED_NAMES: [&str; 3] = [".sbxenv.yaml", ".sbxenv.yml", "sbxenv.yml"];

/// The `PATH...` arguments as files to read, as sbx resolves them: a directory
/// means `sbxenv.yaml` inside it; no argument means the working directory.
pub fn resolve_paths(inputs: &[PathBuf], cwd: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for p in absolute_inputs(inputs, cwd) {
        if p.is_dir() {
            let file = p.join(FILE_NAME);
            if file.is_file() {
                out.push(file);
                continue;
            }
            if let Some(old) = RETIRED_NAMES
                .iter()
                .map(|n| p.join(n))
                .find(|c| c.is_file())
            {
                bail!(
                    "{} is not read: a project's environment file is \
                     `{FILE_NAME}`, not hidden. Rename it (`mv {} {}`).",
                    old.display(),
                    old.display(),
                    file.display()
                );
            }
            bail!("no {FILE_NAME} in {}", p.display());
        } else if p.is_file() {
            out.push(p);
        } else {
            bail!("no such environment file or directory: {}", p.display());
        }
    }
    Ok(out)
}

/// The `PATH...` arguments made absolute (cwd standing in for none). What
/// `sbx env create` gets when nothing needs rewriting: a directory stays a
/// directory, so sbx applies its own rules, user-level base included.
pub fn absolute_inputs(inputs: &[PathBuf], cwd: &Path) -> Vec<PathBuf> {
    if inputs.is_empty() {
        return vec![cwd.to_path_buf()];
    }
    inputs
        .iter()
        .map(|p| {
            if p.is_absolute() {
                p.clone()
            } else {
                cwd.join(p)
            }
        })
        .collect()
}

/// `~/.sbxenv.yaml`, when there is one.
pub fn user_base() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let f = PathBuf::from(home).join(USER_FILE_NAME);
    f.is_file().then_some(f)
}

/// A merged, interpolated environment file: the parsed documents, plus where
/// they came from.
///
/// Documents stay generic `Value`s: sbxw hands a **modified** document back
/// with only the ports changed, and a typed struct would drop the keys sbxw
/// does not model (`secrets`, `bindings`, `registries`, `mcp`, `skills`,
/// `sandboxOptions`).
///
/// The user-level base is kept apart: sbx merges it beneath whatever directory
/// it is given, so a rewritten copy containing it would apply its kits and
/// ports twice (lists concatenate).
pub struct Loaded {
    /// The project files, merged, with their relative paths made absolute.
    pub doc: Value,
    /// `~/.sbxenv.yaml`, likewise anchored; `Null` when there is none.
    pub user_doc: Value,
    /// The project files, in order. The user-level base is not among them.
    pub sources: Vec<PathBuf>,
    pub user_source: Option<PathBuf>,
    /// `${{ … }}` references with no value, in first-appearance order. Left
    /// verbatim for sbx to resolve, but reported: an unresolved path is the
    /// likeliest cause of a later confusing failure.
    pub unresolved: Vec<String>,
    /// `${VAR}` references, which sbx does not expand; reported so a file's
    /// unresolved paths are explained.
    pub legacy_vars: Vec<String>,
}

/// Read, interpolate, anchor and merge the files, in order, over the
/// user-level base.
///
/// The merge is sbx's: **nested mappings merge by key, lists concatenate, and
/// a later scalar replaces an earlier one**. Lists concatenating is why sbxw
/// rewrites the document to change a port rather than layering a file over it.
///
/// Relative paths resolve against **the file that declares them**, so they are
/// made absolute per file before merging; after, nothing says which file a
/// `./kits/x` came from.
///
/// `cli_args` are the `--env-arg NAME=VALUE` pairs; they beat every `args:`
/// default.
pub fn load(
    paths: &[PathBuf],
    user_base: Option<&Path>,
    cli_args: &[(String, String)],
) -> Result<Loaded> {
    let first = paths.first().context("no environment file to read")?;
    // `${{ env.projectDir }}` is the first project file's directory, even
    // inside the user-level base.
    let base_dir = parent_of(first);

    // Every layer in merge order, the user base first.
    let layers: Vec<&Path> = user_base
        .into_iter()
        .chain(paths.iter().map(PathBuf::as_path))
        .collect();
    let raws: Vec<String> = layers
        .iter()
        .map(|p| std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display())))
        .collect::<Result<_>>()?;

    // `args:` defaults must be known before interpolation; a later file's
    // default beats an earlier one's.
    let mut args: BTreeMap<String, String> = BTreeMap::new();
    for raw in &raws {
        args.extend(arg_defaults(raw));
    }
    args.extend(cli_args.iter().cloned());

    let mut project = Value::Null;
    let mut user = Value::Null;
    let mut unresolved: Vec<String> = Vec::new();
    let mut legacy_vars: Vec<String> = Vec::new();
    for (i, (path, raw)) in layers.iter().zip(&raws).enumerate() {
        let file_dir = parent_of(path);
        let lookup = |name: &str| -> Option<String> {
            match name {
                "env.projectDir" => Some(base_dir.to_string_lossy().into_owned()),
                "env.fileDir" => Some(file_dir.to_string_lossy().into_owned()),
                _ => name
                    .strip_prefix("env.args.")
                    .and_then(|a| args.get(a).cloned()),
            }
        };
        // Interpolate the *text*, before parsing, so `${{ env.args.port }}` can
        // stand where a number is typed; a parsed value would be a string.
        let (text, missing, legacy) = interpolate(raw, &lookup);
        for m in missing {
            if !unresolved.contains(&m) {
                unresolved.push(m);
            }
        }
        for v in legacy {
            if !legacy_vars.contains(&v) {
                legacy_vars.push(v);
            }
        }
        let mut parsed: Value =
            serde_norway::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        anchor(&mut parsed, &file_dir);
        if user_base.is_some() && i == 0 {
            user = parsed;
        } else {
            project = merge(project, parsed);
        }
    }

    Ok(Loaded {
        doc: project,
        user_doc: user,
        sources: paths.to_vec(),
        user_source: user_base.map(Path::to_path_buf),
        unresolved,
        legacy_vars,
    })
}

fn parent_of(path: &Path) -> PathBuf {
    path.parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Defaults from a file's `args:` block. sbx documents no grammar, so this
/// reads the shapes people write (`NAME: value`, `NAME: {default: value}`, a
/// list of `{name, default}`) and ignores the rest. A missed argument keeps its
/// reference, which sbx resolves.
fn arg_defaults(raw: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    // A file that parses only once interpolated yields no block here; sbx
    // reports the error.
    let Ok(Value::Mapping(doc)) = serde_norway::from_str::<Value>(raw) else {
        return out;
    };
    let scalar_of = |v: &Value| match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    };
    let default_of = |v: &Value| match v {
        Value::Mapping(m) => m.get("default").and_then(scalar_of),
        other => scalar_of(other),
    };
    match doc.get("args") {
        Some(Value::Mapping(m)) => {
            for (k, v) in m {
                if let (Some(k), Some(d)) = (scalar_of(k), default_of(v)) {
                    out.insert(k, d);
                }
            }
        }
        Some(Value::Sequence(items)) => {
            for it in items {
                let Value::Mapping(m) = it else { continue };
                if let (Some(k), Some(d)) = (
                    m.get("name").and_then(scalar_of),
                    m.get("default").and_then(scalar_of),
                ) {
                    out.insert(k, d);
                }
            }
        }
        _ => {}
    }
    out
}

/// Make one file's relative paths absolute against its directory: the
/// workspace (either form), each additional workspace, each local kit entry,
/// and the host side of each `mounts` entry. A mapping-form kit is left alone:
/// sbx documents its per-kit `args` but not the reference key's name, and
/// resolves it against the same directory as long as the file stays put.
fn anchor(doc: &mut Value, dir: &Path) {
    let Value::Mapping(map) = doc else { return };
    let abs = |s: &str| -> String {
        let p = Path::new(s);
        if p.is_absolute() {
            return s.to_string();
        }
        dir.join(p)
            .components()
            .collect::<PathBuf>()
            .to_string_lossy()
            .into_owned()
    };

    match map.get_mut("workspace") {
        Some(Value::String(s)) => *s = abs(s),
        Some(Value::Mapping(w)) => {
            if let Some(Value::String(s)) = w.get_mut("path") {
                *s = abs(s);
            }
        }
        _ => {}
    }
    if let Some(Value::Sequence(items)) = map.get_mut("additionalWorkspaces") {
        for it in items {
            match it {
                Value::String(s) => *s = abs(s),
                Value::Mapping(m) => {
                    if let Some(Value::String(s)) = m.get_mut("path") {
                        *s = abs(s);
                    }
                }
                _ => {}
            }
        }
    }
    if let Some(Value::Sequence(items)) = map.get_mut("kits") {
        for it in items {
            if let Value::String(s) = it {
                *s = crate::kits::resolve_kit_ref(dir, std::mem::take(s));
            }
        }
    }
    // Host sides only: keys are sandbox paths. `~/` is sbx's to expand.
    if let Some(Value::Mapping(m)) = map.get_mut("mounts") {
        for (_, v) in m.iter_mut() {
            if let Value::String(s) = v {
                if !s.starts_with('~') {
                    *s = abs(s);
                }
            }
        }
    }
}

/// sbx's merge rules, applied to two parsed documents.
fn merge(base: Value, over: Value) -> Value {
    match (base, over) {
        (Value::Mapping(mut a), Value::Mapping(b)) => {
            for (k, v) in b {
                let merged = match a.remove(&k) {
                    Some(existing) => merge(existing, v),
                    None => v,
                };
                a.insert(k, merged);
            }
            Value::Mapping(a)
        }
        (Value::Sequence(mut a), Value::Sequence(b)) => {
            a.extend(b);
            Value::Sequence(a)
        }
        // A later scalar replaces an earlier one, and so does a value of a
        // different shape.
        (_, over) => over,
    }
}

/// Substitute `${{ NAME }}` expressions in `text`; `lookup` gives each dotted
/// name (`env.projectDir`, `env.args.port`) its value. A reference with no
/// value is left **verbatim**, not blanked: `${{ env.args.root }}/src` becoming
/// `/src` would silently mount the wrong directory, while sbx's own error names
/// the argument.
///
/// Returns the text, names with no value, and `${VAR}` names (left as is; sbx
/// does not expand them).
fn interpolate(
    text: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> (String, Vec<String>, Vec<String>) {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut missing = Vec::new();
    let mut legacy = Vec::new();
    let mut i = 0;

    let is_name = |c: char| c.is_ascii_alphanumeric() || c == '_';
    while i < chars.len() {
        if chars[i] != '$' || chars.get(i + 1) != Some(&'{') {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        if chars.get(i + 2) != Some(&'{') {
            // `${VAR}` is not sbx's syntax: noted and copied through.
            let mut j = i + 2;
            while j < chars.len() && is_name(chars[j]) {
                j += 1;
            }
            if j > i + 2 && matches!(chars.get(j), Some('}') | Some(':')) {
                legacy.push(chars[i + 2..j].iter().collect());
            }
            out.push('$');
            i += 1;
            continue;
        }

        // `${{`, blanks, dotted name, blanks, `}}`.
        let mut j = i + 3;
        while chars.get(j) == Some(&' ') {
            j += 1;
        }
        let name_start = j;
        while j < chars.len() && (is_name(chars[j]) || chars[j] == '.') {
            j += 1;
        }
        let name: String = chars[name_start..j].iter().collect();
        while chars.get(j) == Some(&' ') {
            j += 1;
        }
        if name.is_empty() || chars.get(j) != Some(&'}') || chars.get(j + 1) != Some(&'}') {
            // Not an expression at all.
            out.push('$');
            i += 1;
            continue;
        }
        let end = j + 2;
        match lookup(&name) {
            Some(value) => out.push_str(&value),
            None => {
                missing.push(name);
                out.extend(&chars[i..end]);
            }
        }
        i = end;
    }
    (out, missing, legacy)
}

/// The parts of an environment file sbxw acts on, from the project files merged
/// over the user-level base.
///
/// A *view*: the data stays in the documents, which sbx gets back. Keys sbxw
/// cannot act on are only named, in `Spec::delegated`.
#[derive(Debug, Default, PartialEq)]
pub struct Spec {
    pub name: Option<String>,
    pub agent: Option<String>,
    /// Absolute, resolved against the declaring file. `None` means *no*
    /// workspace mount, not the file's directory.
    ///
    /// The object form's `clone` flag is not extracted: sbxw never acts on it,
    /// and `set_workspace` keeps the mapping, so sbx reads it.
    pub workspace: Option<String>,
    pub additional: Vec<MountSpec>,
    pub kits: Vec<String>,
    /// `mounts`: sandbox target → absolute host directory, resolved against the
    /// declaring file.
    pub mounts: BTreeMap<String, String>,
    pub env: BTreeMap<String, String>,
    pub ports: Vec<Port>,
    /// How many of `ports` come from the user-level base (first, since it
    /// merges beneath); one the project replaces is not counted, nor kept.
    pub user_ports: usize,
    /// Top-level keys sbx handles and sbxw only passes through.
    pub delegated: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub struct MountSpec {
    pub path: String,
    pub read_only: bool,
}

/// Keys sbx acts on that sbxw never touches; reported so a run says what it
/// handed over.
const DELEGATED_KEYS: [&str; 6] = [
    "secrets",
    "bindings",
    "registries",
    "mcp",
    "skills",
    "sandboxOptions",
];

impl Loaded {
    /// Read the document into the view sbxw acts on.
    ///
    /// Tolerant where sbx is strict: unknown keys are ignored, so a file using
    /// a newer sbx field still runs; sbx rejects it later with a better message.
    pub fn spec(&self) -> Result<Spec> {
        let merged = merge(self.user_doc.clone(), self.doc.clone());
        let base_ports = match self.user_doc.get("ports") {
            Some(Value::Sequence(items)) => items.len(),
            _ => 0,
        };
        let map = match &merged {
            Value::Mapping(m) => m,
            _ => bail!(
                "{} is not a YAML mapping — an environment file is a set of top-level keys",
                self.sources[0].display()
            ),
        };
        let get = |k: &str| map.get(k);
        let as_str = |v: Option<&Value>| match v {
            Some(Value::String(s)) => Some(s.clone()),
            // A name or version written unquoted comes back as a number.
            Some(Value::Number(n)) => Some(n.to_string()),
            _ => None,
        };

        let workspace = match get("workspace") {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Mapping(w)) => as_str(w.get("path")),
            _ => None,
        };

        let additional = match get("additionalWorkspaces") {
            Some(Value::Sequence(items)) => items
                .iter()
                .filter_map(|it| match it {
                    Value::Mapping(m) => {
                        as_str(m.get("path")).map(|path| MountSpec {
                            path,
                            read_only: matches!(
                                m.get("readOnly"),
                                Some(Value::Bool(true))
                            ),
                        })
                    }
                    // Not the documented shape, but the obvious thing to write.
                    Value::String(s) => Some(MountSpec {
                        path: s.clone(),
                        read_only: false,
                    }),
                    _ => None,
                })
                .collect(),
            _ => vec![],
        };

        let kits = match get("kits") {
            Some(Value::Sequence(items)) => items.iter().filter_map(|v| as_str(Some(v))).collect(),
            _ => vec![],
        };

        let mounts = match get("mounts") {
            Some(Value::Mapping(m)) => m.iter().filter_map(|(k, v)| Some((as_str(Some(k))?, as_str(Some(v))?))).collect(),
            _ => BTreeMap::new(),
        };

        let env = match get("env") {
            Some(Value::Mapping(m)) => m
                .iter()
                .filter_map(|(k, v)| {
                    let key = as_str(Some(k))?;
                    let val = match v {
                        Value::String(s) => s.clone(),
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => b.to_string(),
                        _ => return None,
                    };
                    Some((key, val))
                })
                .collect(),
            _ => BTreeMap::new(),
        };

        let mut ports = Vec::new();
        if let Some(Value::Sequence(items)) = get("ports") {
            for (i, it) in items.iter().enumerate() {
                let Value::Mapping(m) = it else { continue };
                let num = |k: &str| match m.get(k) {
                    Some(Value::Number(n)) => n.as_u64(),
                    // `host: "3000"` is wrong per the schema but unambiguous.
                    Some(Value::String(s)) => s.trim().parse::<u64>().ok(),
                    _ => None,
                };
                let sandbox = num("sandbox").with_context(|| {
                    format!(
                        "ports[{i}] has no numeric `sandbox` port in {}",
                        self.sources[0].display()
                    )
                })?;
                let sandbox = u16::try_from(sandbox)
                    .with_context(|| format!("ports[{i}]: {sandbox} is not a port number"))?;
                ports.push(Port {
                    sandbox,
                    host: num("host").and_then(|h| u16::try_from(h).ok()),
                    host_ip: as_str(m.get("hostIP")),
                    protocol: as_str(m.get("protocol")),
                });
            }
        }
        // sbx 0.53: the last entry for a port wins, whole, across merged files.
        // The base's entries come first, so the ones a project replaces go.
        let replaced: Vec<bool> = (0..ports.len()).map(|i| ports[i + 1..].iter().any(|later| later.same_port(&ports[i]))).collect();
        let user_ports = replaced.iter().take(base_ports).filter(|gone| !**gone).count();
        let mut gone = replaced.into_iter();
        ports.retain(|_| !gone.next().unwrap_or(false));

        let delegated = DELEGATED_KEYS
            .iter()
            .filter(|k| get(k).is_some_and(|v| !matches!(v, Value::Null)))
            .map(|k| (*k).to_string())
            .collect();

        Ok(Spec {
            name: as_str(get("name")),
            agent: as_str(get("agent")),
            workspace,
            additional,
            kits,
            mounts,
            env,
            ports,
            user_ports,
            delegated,
        })
    }

    /// `secrets.<name>.command` entries that lean on a **relative path**, as
    /// `(secret, command)`.
    ///
    /// sbx runs a secret command from a fresh temp directory on the host, so
    /// `./credential-helper` or `cat ../token` finds nothing and the secret fails
    /// far from its cause. That is deliberate (a helper in a writable workspace
    /// is one the agent could rewrite), so this is reported, not repaired:
    /// anchoring the path would undo the protection.
    ///
    /// "Relative" as a shell sees it: a program given by a non-absolute path, or
    /// any word starting `./` or `../`. A bare program name is a `PATH` lookup;
    /// a bare file name (`cat token`) can't be told from an argument, so it is
    /// not guessed at.
    pub fn relative_secret_commands(&self) -> Vec<(String, String)> {
        let merged = merge(self.user_doc.clone(), self.doc.clone());
        let Some(Value::Mapping(secrets)) = merged.get("secrets") else {
            return Vec::new();
        };
        let relative = |word: &str| word.starts_with("./") || word.starts_with("../");
        secrets
            .iter()
            .filter_map(|(name, entry)| {
                let Value::String(command) = entry.get("command")? else {
                    return None;
                };
                let mut words = command.split_whitespace();
                let program = words.next()?;
                let program_is_relative = program.contains('/')
                    && !program.starts_with('/')
                    && !program.starts_with('~')
                    && !program.starts_with('$');
                (program_is_relative || words.any(relative)).then(|| {
                    let name = match name {
                        Value::String(s) => s.clone(),
                        other => serde_norway::to_string(other)
                            .unwrap_or_default()
                            .trim()
                            .to_string(),
                    };
                    (name, command.clone())
                })
            })
            .collect()
    }

    /// Replace the project document's `ports`, keeping other keys. These are the
    /// project's own: sbx re-adds the base's when it reads the copy.
    pub fn set_ports(&mut self, ports: &[Port]) {
        let seq = Value::Sequence(
            ports
                .iter()
                .map(|p| {
                    let mut m = serde_norway::Mapping::new();
                    m.insert("sandbox".into(), (p.sandbox as u64).into());
                    if let Some(h) = p.host {
                        m.insert("host".into(), (h as u64).into());
                    }
                    if let Some(ip) = &p.host_ip {
                        m.insert("hostIP".into(), ip.clone().into());
                    }
                    if let Some(proto) = &p.protocol {
                        m.insert("protocol".into(), proto.clone().into());
                    }
                    Value::Mapping(m)
                })
                .collect(),
        );
        self.set("ports", seq);
    }

    /// Pin the workspace into the project document.
    ///
    /// The copy lives in sbxw's state directory, so a workspace inherited from
    /// the base as `${{ env.projectDir }}` would resolve to that directory on
    /// re-read. The resolved path wins: a later scalar replaces an earlier one.
    pub fn set_workspace(&mut self, path: &str) {
        match self.doc.get("workspace") {
            // Preserve the object form, and with it `clone`.
            Some(Value::Mapping(w)) => {
                let mut w = w.clone();
                w.insert("path".into(), path.into());
                self.set("workspace", Value::Mapping(w));
            }
            _ => self.set("workspace", path.into()),
        }
    }

    fn set(&mut self, key: &str, value: Value) {
        if let Value::Mapping(m) = &mut self.doc {
            m.insert(Value::String(key.into()), value);
        }
    }

    /// The document as YAML, ready to hand back to `sbx env create`.
    pub fn to_yaml(&self) -> Result<String> {
        serde_norway::to_string(&self.doc).context("re-serialising the environment file")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sbxw-envfile-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn load_str(body: &str, tag: &str) -> Loaded {
        let dir = scratch(tag);
        let f = dir.join(FILE_NAME);
        std::fs::write(&f, body).unwrap();
        load(&[f], None, &[]).expect("load")
    }

    fn lookup_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_string())
        }
    }

    /// Interpolate the text before parsing: a parsed `host: ${{ env.args.port }}`
    /// would be the string "3000", which the schema refuses.
    #[test]
    fn an_argument_can_stand_where_the_schema_wants_a_number() {
        let body = "schemaVersion: \"1\"\nagent: claude\nargs:\n  port:\n    default: 3999\n\
                    ports:\n  - sandbox: 3000\n    host: ${{ env.args.port }}\n";
        let loaded = load_str(body, "numeric");
        assert_eq!(loaded.spec().unwrap().ports[0].host, Some(3999));

        // `--env-arg` beats the file's default.
        let f = scratch("numeric-cli").join(FILE_NAME);
        std::fs::write(&f, body).unwrap();
        let cli = [("port".to_string(), "4100".to_string())];
        let spec = load(&[f], None, &cli).unwrap().spec().unwrap();
        assert_eq!(spec.ports[0].host, Some(4100));
    }

    #[test]
    fn expressions_resolve_with_or_without_inner_blanks() {
        let lookup = lookup_of(&[("env.fileDir", "/p"), ("env.args.x", "1")]);
        let (out, missing, legacy) = interpolate(
            "a=${{ env.fileDir }} b=${{env.args.x}} c=${{ env.args.nope }}",
            &lookup,
        );
        assert_eq!(out, "a=/p b=1 c=${{ env.args.nope }}");
        assert_eq!(missing, vec!["env.args.nope"]);
        assert!(legacy.is_empty());
    }

    /// sbx does not expand `${VAR}`: sbxw leaves and reports it, or a literal
    /// `${HOME}` path fails unexplained.
    #[test]
    fn an_old_style_variable_is_left_alone_and_reported() {
        let (out, missing, legacy) =
            interpolate("workspace: ${HOME}/myapp\nx: ${ROOT:-/p}", &lookup_of(&[]));
        assert_eq!(out, "workspace: ${HOME}/myapp\nx: ${ROOT:-/p}");
        assert!(missing.is_empty());
        assert_eq!(legacy, vec!["HOME", "ROOT"]);
    }

    #[test]
    fn text_that_only_looks_like_an_expression_is_left_alone() {
        let lookup = lookup_of(&[("env.fileDir", "/p")]);
        for text in [
            "cost: $5",
            "raw $ sign",
            "${{ env.fileDir",
            "${{ }}",
            "${{a b}}",
        ] {
            assert_eq!(interpolate(text, &lookup).0, text, "{text}");
        }
    }

    /// sbx 0.53: a later entry for the same sandbox port replaces the earlier
    /// one whole, the user base's included; another transport is another port.
    #[test]
    fn the_last_entry_for_a_port_wins_across_merged_files() {
        let home = scratch("ports-home");
        let project = scratch("ports-project");
        let user = home.join(USER_FILE_NAME);
        std::fs::write(&user, "ports:\n  - sandbox: 9000\n    host: 9000\n  - sandbox: 3000\n    host: 3000\n    hostIP: 127.0.0.2\n").unwrap();
        let file = project.join(FILE_NAME);
        std::fs::write(
            &file,
            "agent: claude\nworkspace: .\nports:\n  - sandbox: 3000\n    host: 3100\n  - sandbox: 3000\n    protocol: udp\n  - sandbox: 8080\n  - sandbox: 8080\n    host: 8081\n",
        )
        .unwrap();
        let spec = load(&[file], Some(&user), &[]).unwrap().spec().unwrap();
        let seen: Vec<(u16, Option<u16>, Option<&str>)> = spec.ports.iter().map(|p| (p.sandbox, p.host, p.protocol.as_deref())).collect();
        assert_eq!(seen, [(9000, Some(9000), None), (3000, Some(3100), None), (3000, None, Some("udp")), (8080, Some(8081), None)]);
        assert_eq!(spec.ports[1].host_ip, None, "a replacement replaces the whole entry");
        assert_eq!(spec.user_ports, 1, "the base's 3000 was replaced by the project's");
    }

    /// **Lists concatenate** (ports aside): sbxw rewrites the document to
    /// change a port.
    #[test]
    fn merging_follows_sbx_rules_including_lists_that_concatenate() {
        let dir = scratch("merge");
        let a = dir.join("base.yaml");
        let b = dir.join("local.yaml");
        std::fs::write(
            &a,
            "schemaVersion: \"1\"\nagent: claude\nname: base\nkits:\n  - one\nenv:\n  A: '1'\n  B: '2'\n",
        )
        .unwrap();
        std::fs::write(
            &b,
            "name: over\nkits:\n  - two\nenv:\n  B: 'two'\n  C: '3'\n",
        )
        .unwrap();

        let spec = load(&[a, b], None, &[]).unwrap().spec().unwrap();
        assert_eq!(spec.name.as_deref(), Some("over"), "a later scalar wins");
        assert_eq!(spec.kits, vec!["one", "two"], "lists concatenate");
        assert_eq!(spec.env.get("A").map(String::as_str), Some("1"));
        assert_eq!(spec.env.get("B").map(String::as_str), Some("two"));
        assert_eq!(spec.env.get("C").map(String::as_str), Some("3"));
    }

    /// `~/.sbxenv.yaml` merges *beneath* the project, resolved against its own
    /// directory, but stays out of the returned document: sbx re-merges it, and
    /// its lists would apply twice.
    #[test]
    fn the_user_base_sits_beneath_the_project_and_stays_out_of_the_rewrite() {
        let home = scratch("user-home");
        let project = scratch("user-project");
        let user = home.join(USER_FILE_NAME);
        std::fs::write(
            &user,
            "agent: claude\nworkspace: ${{ env.projectDir }}\nkits:\n  - ./kits/shared\n\
             ports:\n  - sandbox: 9000\n    host: 9000\nenv:\n  A: user\n  B: user\n",
        )
        .unwrap();
        let file = project.join(FILE_NAME);
        std::fs::write(
            &file,
            "kits:\n  - ./kits/own\nports:\n  - sandbox: 4200\n    host: 4200\nenv:\n  B: project\n",
        )
        .unwrap();

        let loaded = load(std::slice::from_ref(&file), Some(&user), &[]).unwrap();
        let spec = loaded.spec().unwrap();
        // `env.projectDir` is the project's directory even inside the base.
        assert_eq!(spec.workspace.as_deref(), Some(&*project.to_string_lossy()));
        // Each kit resolved against the file that named it.
        assert_eq!(
            spec.kits,
            vec![
                home.join("kits/shared").to_string_lossy().into_owned(),
                project.join("kits/own").to_string_lossy().into_owned(),
            ]
        );
        assert_eq!(spec.env.get("A").map(String::as_str), Some("user"));
        assert_eq!(spec.env.get("B").map(String::as_str), Some("project"));
        assert_eq!(spec.ports.len(), 2);
        assert_eq!(spec.user_ports, 1, "the base's ports come first");
        assert_eq!(loaded.user_source.as_deref(), Some(&*user));

        let out = loaded.to_yaml().unwrap();
        assert!(!out.contains("9000"), "the base is not copied:\n{out}");
        assert!(
            !out.contains("kits/shared"),
            "the base is not copied:\n{out}"
        );
        assert!(out.contains("kits/own"), "{out}");
    }

    /// The object form's path is read and anchored; `clone` stays in the
    /// document for sbx and survives a rewrite.
    #[test]
    fn the_workspace_object_form_yields_its_path_and_keeps_clone_for_sbx() {
        let mut loaded = load_str(
            "agent: claude\nworkspace:\n  path: ./myapp\n  clone: true\n",
            "clone",
        );
        let expected = parent_of(&loaded.sources[0]).join("myapp");
        assert_eq!(
            loaded.spec().unwrap().workspace.as_deref(),
            Some(&*expected.to_string_lossy())
        );

        loaded.set_workspace("/abs/myapp");
        let out = loaded.to_yaml().unwrap();
        assert!(
            out.contains("clone: true"),
            "clone survived rewrite:\n{out}"
        );
        assert!(out.contains("/abs/myapp"), "{out}");
    }

    /// No `workspace:` is no mount, not "the file's directory".
    #[test]
    fn a_file_without_a_workspace_declares_none() {
        let spec = load_str("agent: claude\n", "no-workspace").spec().unwrap();
        assert_eq!(spec.workspace, None);
    }

    /// sbx runs these from a temporary directory: a relative helper is
    /// reported, a program found on PATH or by absolute path is not.
    #[test]
    fn a_secret_command_on_a_relative_path_is_reported() {
        let loaded = load_str(
            "schemaVersion: \"1\"\nworkspace: .\n\
             secrets:\n\
             \x20 a:\n    command: ./credential-helper --json\n\
             \x20 b:\n    command: cat ../token\n\
             \x20 c:\n    command: gh auth token\n\
             \x20 d:\n    command: /opt/bin/helper\n\
             \x20 e:\n    command: bin/helper\n\
             \x20 f:\n    ref: op://vault/item\n",
            "relative-secrets",
        );
        let names: Vec<String> = loaded
            .relative_secret_commands()
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(names, ["a", "b", "e"]);
    }

    /// sbxw models none of these keys, and dropping one would silently
    /// unprovision a sandbox's credentials.
    #[test]
    fn rewriting_a_port_preserves_the_keys_sbxw_does_not_understand() {
        let mut loaded = load_str(
            "schemaVersion: \"1\"\nagent: claude\nworkspace: ./myapp\n\
             secrets:\n  anthropic:\n    command: 'op read x'\n\
             mcp:\n  servers:\n    - name: docs\n      url: 'https://example'\n\
             skills: 'off'\n\
             sandboxOptions:\n  memory: 8g\n\
             kits:\n  - name: ./kit\n    args:\n      level: 2\n\
             ports:\n  - sandbox: 4200\n    host: 4200\n",
            "preserve",
        );
        assert_eq!(
            loaded.spec().unwrap().delegated,
            vec!["secrets", "mcp", "skills", "sandboxOptions"]
        );

        loaded.set_ports(&[Port {
            sandbox: 4200,
            host: Some(4201),
            host_ip: None,
            protocol: None,
        }]);
        let out = loaded.to_yaml().unwrap();

        assert!(out.contains("op read x"), "secrets survived:\n{out}");
        assert!(out.contains("sandboxOptions"), "options survived:\n{out}");
        assert!(out.contains("servers"), "mcp survived:\n{out}");
        assert!(
            out.contains("skills: 'off'") || out.contains("skills: off"),
            "{out}"
        );
        assert!(out.contains("level: 2"), "kit args survived:\n{out}");
        assert!(out.contains("host: 4201"), "the host port moved:\n{out}");
        assert!(
            !out.contains("host: 4200"),
            "the old host port is gone:\n{out}"
        );
        // The sandbox port is not negotiated and must not move.
        assert!(
            out.contains("sandbox: 4200"),
            "the sandbox port is untouched:\n{out}"
        );
    }

    /// `export` → `run` crosses both YAML mechanisms: `render` hand-writes,
    /// `load` parses back. A wrong quoting rule in `scalar` only surfaces here.
    #[test]
    fn a_rendered_file_parses_back_into_the_same_spec() {
        let written = EnvFile {
            name: "myapp".into(),
            agent: "claude".into(),
            workspace: "./myapp".into(),
            skills: Some("off".into()),
            kits: vec![
                "docker.io/sbx/playwright-kit:latest".into(),
                "./kits/headroom".into(),
            ],
            mounts: BTreeMap::from([("/home/agent/history".into(), "./history".into())]),
            additional: vec![],
            sandbox_options: vec![],
            env: BTreeMap::from([
                ("NODE_ENV".into(), "test".into()),
                // The values that make `scalar`'s quoting load-bearing.
                ("FEATURE".into(), "no".into()),
                ("PORT".into(), "8".into()),
                ("QUOTED".into(), "it's".into()),
            ]),
            ports: vec![
                Port {
                    sandbox: 4200,
                    host: Some(4201),
                    host_ip: None,
                    protocol: None,
                },
                Port {
                    sandbox: 8000,
                    host: None,
                    host_ip: None,
                    protocol: None,
                },
            ],
            notes: vec!["a note that must not become a field".into()],
        };
        let text = written.render("sbxw test");
        assert!(text.contains("skills: 'off'\n"), "{text}");

        // Read back from wherever the file happens to sit: the paths move with it.
        let dir = scratch("roundtrip");
        let f = dir.join(FILE_NAME);
        std::fs::write(&f, &text).unwrap();
        let spec = load(&[f], None, &[])
            .expect("the export parses")
            .spec()
            .unwrap();

        assert_eq!(spec.name.as_deref(), Some("myapp"));
        assert_eq!(spec.agent.as_deref(), Some("claude"));
        assert_eq!(
            spec.workspace.as_deref(),
            Some(&*dir.join("myapp").to_string_lossy())
        );
        assert_eq!(spec.kits[1], dir.join("kits/headroom").to_string_lossy());
        // A registry reference is not a path and comes back verbatim.
        assert_eq!(spec.kits[0], "docker.io/sbx/playwright-kit:latest");
        assert_eq!(spec.env, written.env, "every value kept its type and text");
        assert_eq!(spec.ports, written.ports, "including the absent host port");
        assert_eq!(spec.mounts["/home/agent/history"], dir.join("history").to_string_lossy(), "the host side moves with the file");
        // The header is comments, so it contributes nothing but `skills`.
        assert_eq!(spec.delegated, vec!["skills"]);
    }

    #[test]
    fn a_directory_resolves_to_sbxenv_yaml_and_an_old_name_says_so() {
        let dir = scratch("resolve");
        std::fs::write(dir.join(".sbxenv.yaml"), "agent: claude\n").unwrap();
        let err = resolve_paths(std::slice::from_ref(&dir), &dir)
            .unwrap_err()
            .to_string();
        assert!(err.contains("is not read"), "{err}");
        assert!(err.contains(FILE_NAME), "{err}");

        std::fs::write(dir.join(FILE_NAME), "agent: claude\n").unwrap();
        assert_eq!(
            resolve_paths(&[], &dir).unwrap(),
            vec![dir.join(FILE_NAME)],
            "no argument means the working directory"
        );
        assert!(resolve_paths(&[dir.join("nope")], &dir).is_err());
        // An explicit file is read whatever it is called.
        assert_eq!(
            resolve_paths(&[dir.join(".sbxenv.yaml")], &dir).unwrap(),
            vec![dir.join(".sbxenv.yaml")]
        );
    }

    #[test]
    fn tcp_is_only_the_default_where_it_cannot_mean_ipv6() {
        assert!(protocol_is_default("tcp4", ""));
        assert!(protocol_is_default("", "::1"));
        assert!(protocol_is_default("tcp", "127.0.0.2"));
        // A dual-stack publish has to say so.
        assert!(!protocol_is_default("tcp", ""));
        assert!(!protocol_is_default("tcp", "::1"));
        assert!(!protocol_is_default("udp", "127.0.0.1"));
    }

    fn minimal() -> EnvFile {
        EnvFile {
            name: "myapp".into(),
            agent: "claude".into(),
            workspace: "./myapp".into(),
            ..Default::default()
        }
    }

    #[test]
    fn the_schema_version_is_a_string_not_a_number() {
        // sbx's loader takes "1" and rejects 1; the quotes are the field.
        assert!(minimal()
            .render("sbxw 1.0")
            .contains("schemaVersion: \"1\"\n"));
    }

    #[test]
    fn a_value_yaml_would_read_as_a_bool_is_quoted() {
        // `FEATURE_X: no` parses as false, and the field is a map of strings.
        for word in ["no", "yes", "true", "off", "null", "N"] {
            assert_eq!(
                scalar(word),
                format!("'{word}'"),
                "{word} must not be left bare"
            );
        }
    }

    #[test]
    fn a_value_yaml_would_read_as_a_number_is_quoted() {
        for num in ["8", "8.0", "0x1f", "1_000", "1e3"] {
            assert!(scalar(num).starts_with('\''), "{num} must not be left bare");
        }
    }

    #[test]
    fn plain_scalars_survive_for_the_tokens_that_make_up_most_of_a_file() {
        assert_eq!(scalar("claude"), "claude");
        assert_eq!(scalar("./myapp"), "./myapp");
        assert_eq!(scalar("NODE_ENV"), "NODE_ENV");
        assert_eq!(scalar("api.myapp.local"), "api.myapp.local");
        assert_eq!(scalar("/home/you/kits/headroom"), "/home/you/kits/headroom");
    }

    #[test]
    fn a_quote_in_a_value_is_doubled_not_escaped() {
        // Single-quoted YAML has exactly one escape, and a backslash isn't it.
        assert_eq!(scalar("it's"), "'it''s'");
        assert_eq!(scalar(r"C:\src"), r"'C:\src'");
        assert_eq!(scalar("a b"), "'a b'");
        assert_eq!(scalar(""), "''");
    }

    #[test]
    fn ports_carry_their_host_ip_only_when_there_is_one_to_carry() {
        let out = EnvFile {
            ports: vec![
                Port {
                    sandbox: 4200,
                    host: Some(4200),
                    host_ip: Some("127.0.0.2".into()),
                    protocol: None,
                },
                Port {
                    sandbox: 8000,
                    host: Some(8000),
                    host_ip: None,
                    protocol: None,
                },
            ],
            ..minimal()
        }
        .render("sbxw 1.0");
        assert!(out.contains("  - sandbox: 4200\n    host: 4200\n    hostIP: '127.0.0.2'\n"));
        assert!(out.ends_with("  - sandbox: 8000\n    host: 8000\n"));
    }

    #[test]
    fn omissions_are_recorded_in_the_file_itself() {
        let out = EnvFile {
            notes: vec!["the egress allowlist:\n  sbx policy allow network \"a,b\"".into()],
            ..minimal()
        }
        .render("sbxw 1.0");
        assert!(out.contains("#   * the egress allowlist:\n"));
        assert!(out.contains("#       sbx policy allow network \"a,b\"\n"));
    }

    #[test]
    fn an_empty_section_is_absent_rather_than_empty() {
        // `kits:` with nothing under it is null, not [], and the loader is strict.
        let out = minimal().render("sbxw 1.0");
        for field in ["kits:", "env:", "ports:"] {
            assert!(!out.contains(field), "{field} should not appear at all");
        }
    }
}
