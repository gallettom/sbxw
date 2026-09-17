//! Reading and writing sbx's own `sbxenv.yaml` **environment files**.
//!
//! An environment file is the setup for a project in sbx's format: agent,
//! workspace, extra mounts, kits, env vars, secrets, MCP servers, ports.
//! `sbx env run` provisions a sandbox from one.
//!
//! sbxw both **exports** one (`sbxw env export`) and **runs** on one
//! (`sbxw env run`). Running does not mean delegating wholesale: sbx creates
//! the sandbox, sbxw does everything around it that the format cannot express.
//! Three documented behaviours shape that split, and they are the reason
//! `sbxw env run` is a pipeline rather than a passthrough:
//!
//! * **Re-running doesn't re-provision.** `sbx env run` applies only `env` and
//!   MCP changes to a sandbox that already exists; workspaces, kits, ports,
//!   secrets and `sandboxOptions` need a remove and recreate. So sbxw calls
//!   `sbx env create` **once**, for the creation, and keeps its own idempotent
//!   pipeline (policy, hooks, credentials, ports, `/etc/hosts`) for every run.
//! * **Ports are all-or-nothing.** If an environment file's port can't be
//!   published, creation fails *and removes the new sandbox* — and there is no
//!   CLI flag to override a port, nor can a second file replace one, because
//!   the merge **concatenates** lists. The only lever is the file itself. That
//!   is why sbxw checks the host ports *before* calling sbx and rewrites the
//!   document (see `Loaded::set_ports`) rather than retrying after a failure:
//!   a failed create also leaves provisioned secrets behind.
//! * **Half of sbxw isn't in the format.** The egress allowlist is `sbx policy`,
//!   not a file field; the browser terminal, the `/etc/hosts` aliases and the
//!   OAuth injection are sbxw's own. On export those omissions go into the
//!   file's header (`notes`); on run they come from `sbxw.toml`, which acts as
//!   the fallback layer under the environment file.
//!
//! And three from sbx 0.42/0.43 that decide how a file is *read*:
//!
//! * **The project file is `sbxenv.yaml`**, not hidden, and a directory means
//!   that name alone. `~/.sbxenv.yaml` is merged beneath it as shared defaults.
//! * **`${VAR}` is no longer expanded.** The file's own expressions are
//!   `${{ env.projectDir }}`, `${{ env.fileDir }}` and `${{ env.args.NAME }}`,
//!   the last supplied with `--env-arg` or defaulted in an `args:` block.
//! * **A relative path resolves against the file that declares it**, and a
//!   file with no `workspace:` mounts nothing at all.
//!
//! The format is `schemaVersion: "1"` and **sbx's loader rejects unknown
//! fields**, so `render` emits the documented set and nothing else. Reading is
//! deliberately more tolerant, and rewriting goes through the parsed document
//! rather than through `EnvFile`, so a field sbxw doesn't model — `secrets`,
//! `bindings`, `registries`, `mcp`, `sandboxOptions` — survives the round trip
//! untouched.

use anyhow::{bail, Context, Result};
use serde_norway::Value;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// One published port, in the environment file's spelling.
#[derive(Debug, Clone, PartialEq)]
pub struct Port {
    pub sandbox: u16,
    /// `None` means the key is absent, which sbx reads as "give me any free
    /// host port". That is a deliberate choice in this format, not a missing
    /// value — it is the one way to publish a port that cannot lose a race
    /// with something already on the host.
    pub host: Option<u16>,
    /// Host interface. `None` uses sbx's default (available IPv4/IPv6
    /// loopback); sbxw sets it only in `ip_per_app` mode, where the whole point
    /// is that each app gets a loopback address of its own.
    pub host_ip: Option<String>,
    /// sbx's default (`tcp4` since 0.42) unless the file says otherwise.
    /// Carried so a rewrite doesn't turn someone's `udp` mapping into a TCP one.
    pub protocol: Option<String>,
}

impl From<&crate::sbx::PortMapping> for Port {
    /// A published mapping, as an environment file would spell it.
    ///
    /// The two defaults are dropped rather than written out: sbx's own default
    /// host interface is loopback and its default protocol is `tcp4` (see
    /// `protocol_is_default`), so naming either adds noise to a file a person
    /// reads. This conversion used to be
    /// inlined at two call sites that disagreed about exactly that — one kept
    /// `hostIP: 127.0.0.1`, the other dropped it — which meant `sbxw env run`
    /// and the Env panel described the same sandbox differently.
    fn from(m: &crate::sbx::PortMapping) -> Self {
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
/// sbx 0.42 made `tcp4` the default, where it used to be dual-stack `tcp`. So
/// a bare `tcp` is no longer the default in general and has to be written out
/// to keep meaning "IPv6 too" — except on an explicit IPv4 address, which can
/// only be bound over IPv4 whatever the protocol says. That is every port sbxw
/// publishes itself, and dropping the word there keeps its exports as quiet as
/// they were.
pub fn protocol_is_default(proto: &str, host_ip: &str) -> bool {
    match proto {
        "" | "tcp4" => true,
        "tcp" => host_ip.parse::<std::net::Ipv4Addr>().is_ok(),
        _ => false,
    }
}

/// Everything `render` needs — already resolved, so this struct has no opinion
/// about where paths came from.
///
/// No `additionalWorkspaces`: sbxw's extra mounts come from `sbxw up --ro`,
/// which is argv rather than config, so neither export path has any to write.
/// The field existed, hard-coded empty at both call sites, until it was
/// removed — read support for the key lives on in `Spec::additional`, which
/// `sbxw env run` genuinely uses.
#[derive(Default)]
pub struct EnvFile {
    pub name: String,
    pub agent: String,
    pub workspace: String,
    /// `off`, `readonly` or `readwrite`; `None` leaves sbx's default
    /// (read-only sharing since 0.43).
    pub skills: Option<String>,
    pub kits: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub ports: Vec<Port>,
    /// Header lines recording what sbxw does that this file cannot express.
    /// Written as comments, so they travel with the file to whoever runs it.
    pub notes: Vec<String>,
}

impl EnvFile {
    /// The file, as text. Deterministic: the same config renders byte for byte
    /// the same file, so a regenerated export diffs to nothing when nothing
    /// changed.
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

        // schemaVersion is quoted in every example sbx publishes, and has to be:
        // bare 1 is an integer and the loader wants the string "1".
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
                // An absent `host` is sbx's "any free port", so omitting the
                // key is meaningful output rather than a gap to fill in.
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

/// A YAML scalar that reads back as the string it was given.
///
/// Quoting is the default, not the exception. A value taken from a config file
/// can be `yes`, `off`, `null`, `8.0` or `~`, every one of which a YAML reader
/// would hand back as something other than a string — and the fields here are
/// all typed as strings, so a plain `NODE_ENV: no` would be a type error at
/// best and the wrong value at worst. Only an unambiguous identifier-ish token
/// is left bare, which covers the names and paths that make up most of a file.
///
/// Single quotes rather than double: inside them the only escape is `''`, so a
/// Windows path cannot grow a meaning it didn't have. (Environment files do
/// substitute `${{ … }}` — but that is sbx's pass over the text, which single
/// quotes don't stop; sbxw exports no expressions, so this is about YAML's own
/// escapes, not sbx's.)
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

/// The one name sbx reads from a project directory (0.42+).
pub const FILE_NAME: &str = "sbxenv.yaml";

/// The user-level base, in the home directory, merged *beneath* every project
/// file as shared defaults. Hidden, unlike the project file — and the only
/// place the hidden name is still read.
pub const USER_FILE_NAME: &str = ".sbxenv.yaml";

/// Names sbx read from a project directory before 0.42 and no longer does.
/// Only used to explain a miss: a directory holding one of these and no
/// `sbxenv.yaml` is a project that has not been renamed yet, and "no
/// environment file here" would be the wrong thing to tell its owner.
const RETIRED_NAMES: [&str; 3] = [".sbxenv.yaml", ".sbxenv.yml", "sbxenv.yml"];

/// Turn the `PATH...` arguments into the files to read, exactly as sbx does:
/// a directory means `sbxenv.yaml` inside it, a file means itself, and nothing
/// at all means the working directory.
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
                    "{} is no longer read: since sbx 0.42 a project's environment file is \
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

/// The `PATH...` arguments made absolute, with the working directory standing
/// in for none. What `sbx env create` is handed when sbxw has nothing to
/// rewrite: a directory stays a directory, so sbx applies its own rules to it
/// — the user-level base included.
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
/// The documents are kept as generic `Value`s and not as a struct, because sbxw
/// has to be able to hand a **modified** document back to sbx with only the
/// ports changed. Round-tripping through a typed struct would silently drop
/// `secrets`, `bindings`, `registries`, `mcp`, `skills` and `sandboxOptions` —
/// every one of which matters to the sandbox and none of which sbxw models.
///
/// The user-level base is kept apart from the project files for the same
/// reason. sbx merges it beneath whatever directory it is given, so a rewritten
/// copy that already contained it would get it twice — and since lists
/// concatenate, every kit and port it declares would be applied twice.
pub struct Loaded {
    /// The project files, merged, with their relative paths made absolute.
    pub doc: Value,
    /// `~/.sbxenv.yaml`, likewise anchored; `Null` when there is none.
    pub user_doc: Value,
    /// The project files, in order. The user-level base is not among them.
    pub sources: Vec<PathBuf>,
    pub user_source: Option<PathBuf>,
    /// `${{ … }}` references that had no value, in order of first appearance.
    /// Left verbatim in the document — sbx gets its own chance to resolve
    /// them — but reported, since an unresolved path is the likeliest cause of
    /// a confusing failure later.
    pub unresolved: Vec<String>,
    /// `${VAR}` references, which sbx stopped expanding in 0.42. Reported so a
    /// file written for an older sbx says why its paths no longer resolve.
    pub legacy_vars: Vec<String>,
}

/// Read, interpolate, anchor and merge the files, in order, over the
/// user-level base.
///
/// The merge is sbx's, spelled out because getting it wrong is invisible until
/// it matters: **nested mappings merge by key, lists concatenate, and a later
/// scalar replaces an earlier one**. Lists concatenating is the surprising half
/// and the reason sbxw rewrites the document to change a port instead of
/// layering a second file over it.
///
/// Relative paths are resolved against **the file that declares them** (sbx
/// 0.43), so they are made absolute per file, before the merge — after it,
/// nothing says which file a `./kits/x` came from.
///
/// `cli_args` are the `--env-arg NAME=VALUE` pairs; they beat every `args:`
/// default.
pub fn load(
    paths: &[PathBuf],
    user_base: Option<&Path>,
    cli_args: &[(String, String)],
) -> Result<Loaded> {
    let first = paths.first().context("no environment file to read")?;
    // The project directory — what `${{ env.projectDir }}` names — is the
    // first project file's, even inside the user-level base.
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

    // `args:` defaults have to be known before any file is interpolated, and a
    // later file's default beats an earlier one's, like any other scalar.
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
        // Interpolation runs over the *text*, before the YAML is parsed, which
        // is what lets `${{ env.args.port }}` stand in a field typed as a
        // number. Doing it on parsed values instead would make
        // `host: ${{ env.args.port }}` a string and the schema would reject it.
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

/// The defaults a file's `args:` block declares.
///
/// sbx documents the block and the `${{ env.args.NAME }}` reference but not a
/// grammar sbxw could check against, so this reads the shapes a person would
/// write — `NAME: value`, `NAME: {default: value}`, or a list of
/// `{name, default}` — and ignores the rest. An argument this misses is not
/// lost: its reference stays in the text, and sbx resolves it.
fn arg_defaults(raw: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    // A file that only parses once interpolated has no block worth reading
    // here; sbx will say what is wrong with it.
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

/// Make one file's relative paths absolute, against that file's directory.
///
/// The workspace (either form), every additional workspace, and every kit
/// entry that names a local path. A kit given as a mapping is left alone: sbx
/// documents per-kit `args` there but not the name of the reference key, and
/// guessing it would be worse than leaving the entry to sbx — which resolves it
/// against the same directory anyway, as long as the file stays where it is.
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
                *s = crate::resolve_kit_ref(dir, std::mem::take(s));
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
        // A later scalar replaces an earlier one — and so does a value of a
        // different shape, which is the only sane reading of "replace".
        (_, over) => over,
    }
}

/// Substitute `${{ NAME }}` expressions in `text` (sbx 0.42+).
///
/// `NAME` is a dotted path — `env.projectDir`, `env.fileDir`,
/// `env.args.port` — and `lookup` decides what each one means. A reference
/// with no value is left **verbatim** rather than blanked: sbxw is not the last
/// reader of this file, and turning `${{ env.args.root }}/src` into `/src`
/// would quietly mount the wrong directory where leaving it alone produces an
/// error that names the argument.
///
/// Returns the text, the names with no value, and the names of any old-style
/// `${VAR}` references — which 0.42 stopped expanding, and which are left
/// exactly as they are, since that is what sbx does with them now.
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
            // `${VAR}` — no longer sbx's syntax. Noted, and copied through.
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

        // `${{`, optional blanks, a dotted name, optional blanks, `}}`.
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

/// The parts of an environment file sbxw acts on, extracted from the project
/// files merged over the user-level base.
///
/// Deliberately a *view*: everything here is also still in the documents, and
/// the documents are what gets handed back to sbx. Fields sbxw cannot act on are reduced to a
/// `present` flag, because the only thing sbxw does with them is say they are
/// there — see `Spec::delegated`.
#[derive(Debug, Default, PartialEq)]
pub struct Spec {
    pub name: Option<String>,
    pub agent: Option<String>,
    /// Absolute: resolved against the file that declared it. `None` means no
    /// file declares one, which since sbx 0.42 means *no* workspace mount — not
    /// the directory holding the file.
    ///
    /// The object form's `clone` flag is deliberately *not* extracted: sbxw
    /// never acts on it, and `set_workspace` preserves the mapping when it
    /// rewrites the path, so sbx reads the flag itself. A field here would only
    /// suggest sbxw handles clone mode.
    pub workspace: Option<String>,
    pub additional: Vec<MountSpec>,
    pub kits: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub ports: Vec<Port>,
    /// How many of `ports` come from the user-level base. They are first,
    /// because the base is merged beneath, and lists concatenate.
    pub user_ports: usize,
    /// Top-level keys sbx handles and sbxw only passes through.
    pub delegated: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub struct MountSpec {
    pub path: String,
    pub read_only: bool,
}

/// Keys sbx acts on that sbxw never touches — reported so a run says what it
/// handed over rather than leaving you to wonder whether it was read at all.
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
    /// Tolerant where sbx is strict: an unknown key is ignored rather than
    /// refused, because sbxw is not the authority on this format and a file
    /// using a field from a newer sbx must still be runnable. sbx itself does
    /// the rejecting, a moment later, with a better message than sbxw could.
    pub fn spec(&self) -> Result<Spec> {
        let merged = merge(self.user_doc.clone(), self.doc.clone());
        let user_ports = match self.user_doc.get("ports") {
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
        let get = |k: &str| map.get(Value::String(k.into()));
        let as_str = |v: Option<&Value>| match v {
            Some(Value::String(s)) => Some(s.clone()),
            // A name or version written unquoted comes back as a number.
            Some(Value::Number(n)) => Some(n.to_string()),
            _ => None,
        };

        let workspace = match get("workspace") {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Mapping(w)) => as_str(w.get(Value::String("path".into()))),
            _ => None,
        };

        let additional = match get("additionalWorkspaces") {
            Some(Value::Sequence(items)) => items
                .iter()
                .filter_map(|it| match it {
                    Value::Mapping(m) => {
                        as_str(m.get(Value::String("path".into()))).map(|path| MountSpec {
                            path,
                            read_only: matches!(
                                m.get(Value::String("readOnly".into())),
                                Some(Value::Bool(true))
                            ),
                        })
                    }
                    // A bare string is not the documented shape, but it is the
                    // obvious thing to write and costs nothing to accept.
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
                let num = |k: &str| match m.get(Value::String(k.into())) {
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
                    // Absent `host` means "give me an ephemeral one", which is
                    // a real choice in this format and not a missing value.
                    host: num("host").and_then(|h| u16::try_from(h).ok()),
                    host_ip: as_str(m.get(Value::String("hostIP".into()))),
                    protocol: as_str(m.get(Value::String("protocol".into()))),
                });
            }
        }

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
            env,
            ports,
            user_ports,
            delegated,
        })
    }

    /// Replace the project document's `ports` list, keeping every other key as
    /// it was.
    ///
    /// `ports` are the project's own — without the user-level base's, which sbx
    /// adds again when it reads the rewritten copy.
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
    /// A rewritten copy lives in sbxw's state directory, so a workspace it
    /// inherited from the user-level base as `${{ env.projectDir }}` would name
    /// *that* directory when sbx re-reads the base. Writing the resolved path
    /// into the copy wins over the base, because a later scalar replaces an
    /// earlier one.
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

    /// Interpolation runs over the *text*, before the YAML is parsed. That is
    /// the only order in which `host: ${{ env.args.port }}` can land in a field
    /// the schema types as a number — substituting into parsed values would
    /// make it the string "3000" and the schema would refuse it.
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

    /// sbx 0.42 stopped expanding `${VAR}`. sbxw leaves it exactly as sbx now
    /// does, and says so — a file written for 0.39 otherwise fails on a path
    /// with a literal `${HOME}` in it and nothing explains why.
    #[test]
    fn an_old_style_variable_is_left_alone_and_reported() {
        let (out, missing, legacy) =
            interpolate("workspace: ${HOME}/neos\nx: ${ROOT:-/p}", &lookup_of(&[]));
        assert_eq!(out, "workspace: ${HOME}/neos\nx: ${ROOT:-/p}");
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

    /// sbx's merge, and the half that surprises: **lists concatenate**. It is
    /// why sbxw rewrites the document to change a port instead of layering a
    /// second file over it — an override file could only ever add a third port.
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

    /// `~/.sbxenv.yaml` is merged *beneath* the project, and resolved against
    /// its own directory — but kept out of the document sbxw hands back, since
    /// sbx merges it again and its lists would be applied twice.
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

    /// The object form's path is read and anchored; its `clone` flag is left in
    /// the document for sbx, and survives a rewrite because `set_workspace`
    /// keeps the mapping.
    #[test]
    fn the_workspace_object_form_yields_its_path_and_keeps_clone_for_sbx() {
        let mut loaded = load_str(
            "agent: claude\nworkspace:\n  path: ./neos\n  clone: true\n",
            "clone",
        );
        let expected = parent_of(&loaded.sources[0]).join("neos");
        assert_eq!(
            loaded.spec().unwrap().workspace.as_deref(),
            Some(&*expected.to_string_lossy())
        );

        loaded.set_workspace("/abs/neos");
        let out = loaded.to_yaml().unwrap();
        assert!(
            out.contains("clone: true"),
            "clone survived rewrite:\n{out}"
        );
        assert!(out.contains("/abs/neos"), "{out}");
    }

    /// No `workspace:` is no mount (sbx 0.42), not "the file's directory".
    #[test]
    fn a_file_without_a_workspace_declares_none() {
        let spec = load_str("agent: claude\n", "no-workspace").spec().unwrap();
        assert_eq!(spec.workspace, None);
    }

    #[test]
    fn a_port_without_a_host_key_means_ephemeral_not_missing() {
        let spec = load_str(
            "agent: claude\nports:\n  - sandbox: 3000\n  - sandbox: 4000\n    host: 4000\n",
            "ephemeral",
        )
        .spec()
        .unwrap();
        assert_eq!(spec.ports[0].host, None);
        assert_eq!(spec.ports[1].host, Some(4000));
    }

    /// The whole reason rewriting goes through the parsed document: sbxw models
    /// none of these keys, and dropping one would silently unprovision a
    /// sandbox's credentials.
    #[test]
    fn rewriting_a_port_preserves_the_keys_sbxw_does_not_understand() {
        let mut loaded = load_str(
            "schemaVersion: \"1\"\nagent: claude\nworkspace: ./neos\n\
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
        // The *sandbox* port is not the one being negotiated and must not move.
        assert!(
            out.contains("sandbox: 4200"),
            "the sandbox port is untouched:\n{out}"
        );
    }

    /// `sbxw env export` → `sbxw env run` is a real round trip across the two
    /// YAML mechanisms in this module — `render` hand-writes it, `load` parses
    /// it back — and nothing crossed that seam. A quoting rule that `scalar`
    /// got wrong would surface here and nowhere else.
    #[test]
    fn a_rendered_file_parses_back_into_the_same_spec() {
        let written = EnvFile {
            name: "neos".into(),
            agent: "claude".into(),
            workspace: "./neos".into(),
            skills: Some("off".into()),
            kits: vec![
                "docker.io/sbx/playwright-kit:latest".into(),
                "./kits/headroom".into(),
            ],
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

        assert_eq!(spec.name.as_deref(), Some("neos"));
        assert_eq!(spec.agent.as_deref(), Some("claude"));
        assert_eq!(
            spec.workspace.as_deref(),
            Some(&*dir.join("neos").to_string_lossy())
        );
        assert_eq!(spec.kits[1], dir.join("kits/headroom").to_string_lossy());
        // A registry reference is not a path and comes back verbatim.
        assert_eq!(spec.kits[0], "docker.io/sbx/playwright-kit:latest");
        assert_eq!(spec.env, written.env, "every value kept its type and text");
        assert_eq!(spec.ports, written.ports, "including the absent host port");
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
        assert!(err.contains("no longer read"), "{err}");
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
        // A dual-stack publish has to say so since 0.42.
        assert!(!protocol_is_default("tcp", ""));
        assert!(!protocol_is_default("tcp", "::1"));
        assert!(!protocol_is_default("udp", "127.0.0.1"));
    }

    fn minimal() -> EnvFile {
        EnvFile {
            name: "neos".into(),
            agent: "claude".into(),
            workspace: "./neos".into(),
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
        assert_eq!(scalar("./neos"), "./neos");
        assert_eq!(scalar("NODE_ENV"), "NODE_ENV");
        assert_eq!(scalar("api.neos.local"), "api.neos.local");
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
    fn an_oci_kit_reference_keeps_its_colon() {
        // `docker.io/sbx/playwright-kit:latest` bare would be read as a mapping.
        let out = EnvFile {
            kits: vec!["docker.io/sbx/playwright-kit:latest".into()],
            ..minimal()
        }
        .render("sbxw 1.0");
        assert!(
            out.contains("  - 'docker.io/sbx/playwright-kit:latest'\n"),
            "{out}"
        );
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
        // The point of the export is that it is honest about being partial.
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

    #[test]
    fn the_same_config_renders_the_same_bytes() {
        let build = || {
            EnvFile {
                env: BTreeMap::from([
                    ("B".into(), "2".into()),
                    ("A".into(), "1".into()),
                    ("C".into(), "3".into()),
                ]),
                ..minimal()
            }
            .render("sbxw 1.0")
        };
        assert_eq!(build(), build());
        let out = build();
        let a = out.find("A:").unwrap();
        let b = out.find("B:").unwrap();
        assert!(
            a < b,
            "env keys are sorted, so a re-export diffs to nothing"
        );
    }
}
