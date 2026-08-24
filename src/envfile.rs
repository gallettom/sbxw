//! Reading and writing sbx's own `.sbxenv.yaml` **environment files** (0.39+).
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
    /// `tcp` (sbx's default) unless the file says otherwise. Carried so a
    /// rewrite doesn't turn someone's `udp` mapping into a TCP one.
    pub protocol: Option<String>,
}

impl From<&crate::sbx::PortMapping> for Port {
    /// A published mapping, as an environment file would spell it.
    ///
    /// The two defaults are dropped rather than written out: sbx's own default
    /// host interface is loopback and its default protocol is `tcp`, so naming
    /// either adds noise to a file a person reads. This conversion used to be
    /// inlined at two call sites that disagreed about exactly that — one kept
    /// `hostIP: 127.0.0.1`, the other dropped it — which meant `sbxw env run`
    /// and the Env panel described the same sandbox differently.
    fn from(m: &crate::sbx::PortMapping) -> Self {
        Port {
            sandbox: m.sandbox_port,
            host: Some(m.host_port),
            host_ip: (!m.host_ip.is_empty() && m.host_ip != "127.0.0.1").then(|| m.host_ip.clone()),
            protocol: (!m.proto.is_empty() && m.proto != "tcp").then(|| m.proto.clone()),
        }
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
/// Windows path or a `$VAR` cannot grow a meaning it didn't have. (Environment
/// files do interpolate `${VAR}` — but that is sbx's substitution pass over the
/// file, which single quotes don't stop; anything sbxw exports is already
/// resolved, so this is about YAML's own escapes, not sbx's.)
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

/// The two names sbx looks for when it is handed a directory, in its order.
const FILE_NAMES: [&str; 2] = [".sbxenv.yaml", ".sbxenv.yml"];

/// Turn the `PATH...` arguments into the files to read, exactly as sbx does:
/// a directory means `.sbxenv.yaml` (falling back to `.sbxenv.yml`) inside it,
/// a file means itself, and nothing at all means the working directory.
pub fn resolve_paths(inputs: &[PathBuf], cwd: &Path) -> Result<Vec<PathBuf>> {
    let inputs: Vec<PathBuf> = if inputs.is_empty() {
        vec![cwd.to_path_buf()]
    } else {
        inputs.to_vec()
    };
    let mut out = Vec::new();
    for raw in inputs {
        let p = if raw.is_absolute() {
            raw
        } else {
            cwd.join(raw)
        };
        if p.is_dir() {
            let found = FILE_NAMES
                .iter()
                .map(|n| p.join(n))
                .find(|c| c.is_file())
                .with_context(|| format!("no .sbxenv.yaml or .sbxenv.yml in {}", p.display()))?;
            out.push(found);
        } else if p.is_file() {
            out.push(p);
        } else {
            bail!("no such environment file or directory: {}", p.display());
        }
    }
    Ok(out)
}

/// A merged, interpolated environment file: the parsed document, plus where it
/// came from.
///
/// `doc` is kept as a generic `Value` and not as a struct, because sbxw has to
/// be able to hand a **modified** document back to sbx with only the ports
/// changed. Round-tripping through a typed struct would silently drop
/// `secrets`, `bindings`, `registries`, `mcp` and `sandboxOptions` — every one
/// of which matters to the sandbox and none of which sbxw models.
pub struct Loaded {
    pub doc: Value,
    /// Directory of the **first** file: what sbx resolves relative paths
    /// against, and what sbxw must therefore use too.
    pub base_dir: PathBuf,
    pub sources: Vec<PathBuf>,
    /// `${VAR}` references that had no value on this host, in order of first
    /// appearance. Left verbatim in the document — sbx gets its own chance to
    /// resolve them — but reported, since an unresolved path is the likeliest
    /// cause of a confusing failure later.
    pub unresolved: Vec<String>,
}

/// Read, interpolate and merge the files, in order.
///
/// The merge is sbx's, spelled out because getting it wrong is invisible until
/// it matters: **nested mappings merge by key, lists concatenate, and a later
/// scalar replaces an earlier one**. Lists concatenating is the surprising half
/// and the reason sbxw rewrites the document to change a port instead of
/// layering a second file over it.
pub fn load(paths: &[PathBuf], lookup: &dyn Fn(&str) -> Option<String>) -> Result<Loaded> {
    let first = paths.first().context("no environment file to read")?;
    let base_dir = first
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let mut doc = Value::Null;
    let mut unresolved: Vec<String> = Vec::new();
    for path in paths {
        let raw =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        // Interpolation runs over the *text*, before the YAML is parsed, which
        // is what lets `${PORT}` stand in a field typed as a number. Doing it
        // on parsed values instead would make `host: ${PORT}` a string and the
        // schema would reject it.
        let (text, missing) = interpolate(&raw, lookup);
        for m in missing {
            if !unresolved.contains(&m) {
                unresolved.push(m);
            }
        }
        let parsed: Value =
            serde_norway::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        doc = merge(doc, parsed);
    }

    Ok(Loaded {
        doc,
        base_dir,
        sources: paths.to_vec(),
        unresolved,
    })
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

/// Substitute `${VAR}`, `$VAR` and `${VAR:-default}` in `text`.
///
/// Only the three forms sbx documents. A reference with no value and no
/// default is left **verbatim** rather than blanked: sbxw is not the last
/// reader of this file, and turning `${HOME}/src` into `/src` would quietly
/// mount the wrong directory where leaving it alone produces an error that
/// names the variable.
fn interpolate(text: &str, lookup: &dyn Fn(&str) -> Option<String>) -> (String, Vec<String>) {
    let bytes: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut missing = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != '$' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        let braced = bytes.get(i + 1) == Some(&'{');
        let name_start = if braced { i + 2 } else { i + 1 };
        let mut j = name_start;
        while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == '_') {
            j += 1;
        }
        // `$` followed by nothing name-shaped is just a `$`.
        if j == name_start {
            out.push('$');
            i += 1;
            continue;
        }
        let name: String = bytes[name_start..j].iter().collect();

        let (default, end) = if braced {
            match bytes.get(j) {
                Some('}') => (None, j + 1),
                Some(':') if bytes.get(j + 1) == Some(&'-') => {
                    let d_start = j + 2;
                    let mut k = d_start;
                    while k < bytes.len() && bytes[k] != '}' {
                        k += 1;
                    }
                    if k >= bytes.len() {
                        // Unterminated — not a reference at all.
                        out.push('$');
                        i += 1;
                        continue;
                    }
                    (Some(bytes[d_start..k].iter().collect::<String>()), k + 1)
                }
                _ => {
                    out.push('$');
                    i += 1;
                    continue;
                }
            }
        } else {
            (None, j)
        };

        match lookup(&name).filter(|v| !v.is_empty()).or(default) {
            Some(value) => out.push_str(&value),
            None => {
                missing.push(name);
                out.extend(&bytes[i..end]);
            }
        }
        i = end;
    }
    (out, missing)
}

/// The parts of an environment file sbxw acts on, extracted from `doc`.
///
/// Deliberately a *view*: everything here is also still in `doc`, and `doc` is
/// what gets handed back to sbx. Fields sbxw cannot act on are reduced to a
/// `present` flag, because the only thing sbxw does with them is say they are
/// there — see `Spec::delegated`.
#[derive(Debug, Default, PartialEq)]
pub struct Spec {
    pub name: Option<String>,
    pub agent: Option<String>,
    /// As written in the file, before being resolved against `base_dir`.
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
const DELEGATED_KEYS: [&str; 5] = ["secrets", "bindings", "registries", "mcp", "sandboxOptions"];

impl Loaded {
    /// Read the document into the view sbxw acts on.
    ///
    /// Tolerant where sbx is strict: an unknown key is ignored rather than
    /// refused, because sbxw is not the authority on this format and a file
    /// using a field from a newer sbx must still be runnable. sbx itself does
    /// the rejecting, a moment later, with a better message than sbxw could.
    pub fn spec(&self) -> Result<Spec> {
        let map = match &self.doc {
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
            delegated,
        })
    }

    /// Replace the document's `ports` list, keeping every other key as it was.
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

    /// Rewrite a path-bearing key so the document no longer depends on sitting
    /// in `base_dir` — which is what lets sbxw hand sbx a derived file from a
    /// scratch directory instead of writing one into the user's repository.
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

    pub fn set_additional(&mut self, mounts: &[MountSpec]) {
        if mounts.is_empty() {
            return;
        }
        let seq = Value::Sequence(
            mounts
                .iter()
                .map(|m| {
                    let mut e = serde_norway::Mapping::new();
                    e.insert("path".into(), m.path.clone().into());
                    if m.read_only {
                        e.insert("readOnly".into(), true.into());
                    }
                    Value::Mapping(e)
                })
                .collect(),
        );
        self.set("additionalWorkspaces", seq);
    }

    /// Pin the sandbox name into the document.
    ///
    /// Called whenever the file leaves it out. sbx would default it to
    /// `<agent>-<workspace-basename>`, and sbxw would have to guess the same
    /// string to find the sandbox again afterwards — including whatever
    /// sanitising sbx applies. Writing the name down removes the guess.
    pub fn set_name(&mut self, name: &str) {
        self.set("name", name.into());
    }

    pub fn set_kits(&mut self, kits: &[String]) {
        if kits.is_empty() {
            return;
        }
        let seq = Value::Sequence(kits.iter().map(|k| Value::String(k.clone())).collect());
        self.set("kits", seq);
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

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |name: &str| {
            owned
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sbxw-envfile-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn load_str(body: &str, lookup: &dyn Fn(&str) -> Option<String>, tag: &str) -> Loaded {
        let dir = scratch(tag);
        let f = dir.join(".sbxenv.yaml");
        std::fs::write(&f, body).unwrap();
        load(&[f], lookup).expect("load")
    }

    /// Interpolation runs over the *text*, before the YAML is parsed. That is
    /// the only order in which `host: ${PORT}` can land in a field the schema
    /// types as a number — substituting into parsed values would make it the
    /// string "3000" and the schema would refuse it.
    #[test]
    fn a_variable_can_stand_where_the_schema_wants_a_number() {
        let loaded = load_str(
            "schemaVersion: \"1\"\nagent: claude\nports:\n  - sandbox: 3000\n    host: ${PORT}\n",
            &env_of(&[("PORT", "3999")]),
            "numeric",
        );
        let spec = loaded.spec().unwrap();
        assert_eq!(spec.ports[0].host, Some(3999));
    }

    #[test]
    fn the_three_documented_forms_all_resolve() {
        let lookup = env_of(&[("SET", "yes"), ("EMPTY", "")]);
        let (out, missing) = interpolate(
            "a=${SET} b=$SET c=${UNSET:-fallback} d=${EMPTY:-used} e=$UNSET f=${UNSET}",
            &lookup,
        );
        // An empty value counts as unset for `:-`, matching every shell.
        assert_eq!(out, "a=yes b=yes c=fallback d=used e=$UNSET f=${UNSET}");
        // Left verbatim rather than blanked, and reported once per name.
        assert_eq!(missing, vec!["UNSET", "UNSET"]);
    }

    /// `${HOME}/src` becoming `/src` would mount the wrong directory silently.
    /// Leaving it alone produces an error naming the variable instead.
    #[test]
    fn an_unresolved_reference_is_left_for_sbx_rather_than_blanked() {
        let (out, _) = interpolate("workspace: ${NOPE}/neos", &env_of(&[]));
        assert_eq!(out, "workspace: ${NOPE}/neos");
    }

    #[test]
    fn text_that_only_looks_like_a_reference_is_left_alone() {
        let lookup = env_of(&[("A", "x")]);
        for text in ["cost: $5", "raw $ sign", "${unterminated:-oops", "${}"] {
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

        let spec = load(&[a, b], &env_of(&[])).unwrap().spec().unwrap();
        assert_eq!(spec.name.as_deref(), Some("over"), "a later scalar wins");
        assert_eq!(spec.kits, vec!["one", "two"], "lists concatenate");
        assert_eq!(spec.env.get("A").map(String::as_str), Some("1"));
        assert_eq!(spec.env.get("B").map(String::as_str), Some("two"));
        assert_eq!(spec.env.get("C").map(String::as_str), Some("3"));
    }

    /// The object form's path is read; its `clone` flag is left in the document
    /// for sbx, and survives a rewrite because `set_workspace` keeps the mapping.
    #[test]
    fn the_workspace_object_form_yields_its_path_and_keeps_clone_for_sbx() {
        let mut loaded = load_str(
            "agent: claude\nworkspace:\n  path: ./neos\n  clone: true\n",
            &env_of(&[]),
            "clone",
        );
        assert_eq!(loaded.spec().unwrap().workspace.as_deref(), Some("./neos"));

        loaded.set_workspace("/abs/neos");
        let out = loaded.to_yaml().unwrap();
        assert!(
            out.contains("clone: true"),
            "clone survived rewrite:\n{out}"
        );
        assert!(out.contains("/abs/neos"), "{out}");
    }

    #[test]
    fn a_port_without_a_host_key_means_ephemeral_not_missing() {
        let spec = load_str(
            "agent: claude\nports:\n  - sandbox: 3000\n  - sandbox: 4000\n    host: 4000\n",
            &env_of(&[]),
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
             sandboxOptions:\n  memory: 8g\n\
             ports:\n  - sandbox: 4200\n    host: 4200\n",
            &env_of(&[]),
            "preserve",
        );
        assert_eq!(
            loaded.spec().unwrap().delegated,
            vec!["secrets", "mcp", "sandboxOptions"]
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
            workspace: "${SBXW_PROJECTS_ROOT:-/home/you/dev}/neos".into(),
            kits: vec![
                "docker.io/sbx/playwright-kit:latest".into(),
                "${SBXW_PROJECTS_ROOT:-/home/you/dev}/kits/headroom".into(),
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

        let dir = scratch("roundtrip");
        let f = dir.join(".sbxenv.yaml");
        std::fs::write(&f, &text).unwrap();
        // Read back on a machine where the variable is *not* set, which is the
        // case the fallback exists for.
        let spec = load(&[f], &|_| None)
            .expect("the export parses")
            .spec()
            .unwrap();

        assert_eq!(spec.name.as_deref(), Some("neos"));
        assert_eq!(spec.agent.as_deref(), Some("claude"));
        // The whole point of exporting `${VAR:-/abs/path}`: with no variable
        // set, the file still resolves to a real directory.
        assert_eq!(spec.workspace.as_deref(), Some("/home/you/dev/neos"));
        assert_eq!(spec.kits[1], "/home/you/dev/kits/headroom");
        // A registry reference has no variable in it and comes back verbatim.
        assert_eq!(spec.kits[0], "docker.io/sbx/playwright-kit:latest");
        assert_eq!(spec.env, written.env, "every value kept its type and text");
        assert_eq!(spec.ports, written.ports, "including the absent host port");
        // The header is comments, so it contributes no keys at all.
        assert!(spec.delegated.is_empty());

        // On a machine that *does* set the variable, it beats the fallback —
        // which is what makes one committed file work in two checkout layouts.
        let f2 = dir.join("second.sbxenv.yaml");
        std::fs::write(&f2, &text).unwrap();
        let elsewhere = load(&[f2], &|name| {
            (name == "SBXW_PROJECTS_ROOT").then(|| "/srv/repos".to_string())
        })
        .unwrap()
        .spec()
        .unwrap();
        assert_eq!(elsewhere.workspace.as_deref(), Some("/srv/repos/neos"));
        assert_eq!(elsewhere.kits[1], "/srv/repos/kits/headroom");
    }

    #[test]
    fn a_directory_resolves_to_the_file_inside_it_and_a_missing_one_errors() {
        let dir = scratch("resolve");
        std::fs::write(dir.join(".sbxenv.yml"), "agent: claude\n").unwrap();
        // .yaml is preferred, .yml is the fallback — here only .yml exists.
        assert_eq!(
            resolve_paths(std::slice::from_ref(&dir), &dir).unwrap(),
            vec![dir.join(".sbxenv.yml")]
        );
        std::fs::write(dir.join(".sbxenv.yaml"), "agent: claude\n").unwrap();
        assert_eq!(
            resolve_paths(&[], &dir).unwrap(),
            vec![dir.join(".sbxenv.yaml")],
            "no argument means the working directory"
        );
        assert!(resolve_paths(&[dir.join("nope")], &dir).is_err());
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
