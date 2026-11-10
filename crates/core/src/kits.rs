//! Kits: sbx's declarative extension point, and the facts sbxw has learnt
//! about applying them.
//!
//! * **Kit spec v3 only (sbx 0.51).** `claude` resolves to the published v3
//!   workload `docker.io/sbx/claude:latest`, and a v1/v2 kit cannot be composed
//!   with a v3 one. sbxw ships and writes only v3 kits, and refuses a local
//!   v1/v2 kit in `kits` before creating anything.
//! * **Creation is the moment to apply a kit.** `sbx kit add` on an existing
//!   sandbox recreates its container, and refuses kits declaring credentials,
//!   ports, volumes, generated lifecycle files or host-level denies (image
//!   content, install/startup hooks and allow rules it takes). A refusal tells
//!   the person to recreate; sbxw never recreates on its own.
//! * **sbx keeps a kit's path, not a copy.** A container swap re-resolves each
//!   kit by its original path; a missing directory fails the swap and names the
//!   *wrong* kit. Hence the stable directories bundled and generated kits are
//!   written to.

use std::path::{Path, PathBuf};

/// The kits that ship inside sbxw, referred to as `sbxw:<name>` in `kits`.
pub const BUNDLED: [&str; 8] = ["codemap", "agent-skills", "md-to-pdf-tools", "retro", "k8s-tools", "git-identity", "ssh-agent", "terminal-mac"];

/// sbxw's own kit, applied to every sandbox before the project's: Node.js,
/// which sbx 0.51's Claude Code image lacks and every in-sandbox piece of sbxw
/// runs on. Not one of [`BUNDLED`]: it is not offered, and not listed in `kits`.
pub const BASE: &str = "base";

/// `kits` as applied: [`BASE`] first, then the project's, without repeating it.
pub fn with_base(kits: &[String]) -> Vec<String> {
    let base = format!("sbxw:{BASE}");
    std::iter::once(base.clone()).chain(kits.iter().filter(|k| **k != base).cloned()).collect()
}

/// What a project without a `kits` list gets. k8s-tools is opt-in: only
/// Kubernetes projects need kubectl, k3d and skaffold. So are git-identity and
/// ssh-agent (v3 capability kits): each hands the sandbox something of the
/// person's — their name and email, their SSH keys' signatures.
pub const DEFAULT_KITS: [&str; 4] = ["sbxw:codemap", "sbxw:agent-skills", "sbxw:md-to-pdf-tools", "sbxw:retro"];

/// One line per bundled kit, for the web UI's Kits dialog.
pub fn about(name: &str) -> &'static str {
    match name {
        "codemap" => "/codemap and /codemap-lens: agents read a repo's code map before its source",
        "agent-skills" => "/archify, /brag, /i-have-adhd, /handoff, /grilling, /frontend-design, with Chromium and FFmpeg",
        "md-to-pdf-tools" => "/md-to-pdf, with WeasyPrint, poppler-utils and Pillow",
        "retro" => "/retro: past sessions read back for what should become a script, hook, skill, rule or kit",
        "k8s-tools" => "kubectl, k3d and skaffold in ~/.local/bin",
        "git-identity" => "your host's Git user.name and user.email in the sandbox",
        "ssh-agent" => "your host's SSH agent, restricted to signing commits and the common Git hosts",
        "terminal-mac" => "a virtual Mac the agent drives (Safari, Mac apps) through the host's terminal-mac; Apple silicon hosts",
        _ => "",
    }
}

/// The root `kits` of `raw` (an sbxw.toml) as written; `None` when there is no
/// such line, and the defaults apply.
pub fn read_kits_line(raw: &str) -> anyhow::Result<Option<Vec<String>>> {
    let v: toml::Value = toml::from_str(raw)?;
    Ok(v.get("kits").and_then(|k| k.as_array()).map(|a| a.iter().filter_map(|k| k.as_str().map(String::from)).collect()))
}

/// `raw` (an sbxw.toml) with its root `kits` replaced by `kits`, one entry
/// per line, or removed when `None` (the defaults then apply). Everything
/// else — comments, order, the `[[ports]]` tables — is left as written: the
/// person keeps this file by hand. `toml_edit` puts a root key before the
/// first table, where TOML needs it.
pub fn set_kits_line(raw: &str, kits: Option<&[String]>) -> anyhow::Result<String> {
    let mut doc: toml_edit::DocumentMut = raw.parse()?;
    match kits {
        None => {
            doc.remove("kits");
        }
        Some(kits) => {
            // Comments inside the list (often kits commented out for later)
            // stay with their slot: a kept entry keeps its own; one replaced in
            // place (a path turned into its `sbxw:` name) hands them over; one
            // removed passes them to the next kept entry.
            let old = doc.get("kits").and_then(|k| k.as_array()).cloned().unwrap_or_default();
            let olds: Vec<Option<&str>> = old.iter().map(|v| v.as_str()).collect();
            let comment = |v: &toml_edit::Value| {
                let p = v.decor().prefix().and_then(|p| p.as_str()).unwrap_or("");
                p.contains('#').then(|| format!("\n{}\n", p.trim_matches(|c: char| c.is_whitespace())))
            };
            let mut above: std::collections::HashMap<&str, String> = Default::default();
            let mut carry = String::new();
            for (i, v) in old.iter().enumerate() {
                let own = comment(v).unwrap_or_default();
                match v.as_str() {
                    Some(k) if kits.iter().any(|n| n == k) => {
                        above.insert(k, std::mem::take(&mut carry) + &own);
                    }
                    _ => match kits.get(i).filter(|n| !olds.contains(&Some(n.as_str()))) {
                        Some(n) => {
                            above.insert(n.as_str(), std::mem::take(&mut carry) + &own);
                        }
                        None => carry += &own,
                    },
                }
            }
            let mut arr = toml_edit::Array::new();
            for k in kits {
                let lead = match above.get(k.as_str()).filter(|c| !c.is_empty()) {
                    Some(c) => format!("{}    ", c.replace("\n\n", "\n")),
                    None => "\n    ".into(),
                };
                arr.push_formatted(toml_edit::Value::from(k.as_str()).decorated(lead, ""));
            }
            // Comments after the last entry stay, on lines of their own, with
            // those of removed entries no later entry took.
            let tail = old.trailing().as_str().filter(|t| t.contains('#')).map(|t| format!("\n{}\n", t.trim_matches(|c: char| c.is_whitespace())));
            let trailing = (carry + &tail.unwrap_or_default()).replace("\n\n", "\n");
            arr.set_trailing(if trailing.is_empty() { "\n".to_string() } else { trailing });
            arr.set_trailing_comma(true);
            // Keep the comment block above an existing line.
            let prefix = doc.as_table().key("kits").and_then(|k| k.leaf_decor().prefix()).cloned();
            doc["kits"] = toml_edit::value(arr);
            if let (Some(prefix), Some(mut key)) = (prefix, doc.as_table_mut().key_mut("kits")) {
                key.leaf_decor_mut().set_prefix(prefix);
            }
        }
    }
    Ok(doc.to_string())
}

/// The bundled kit a reference names (`sbxw:codemap` → `codemap`).
pub fn bundled_name(kit: &str) -> Option<&str> {
    kit.strip_prefix("sbxw:")
}

/// One `kits = [...]` entry resolved against the config's directory, only when
/// it is a local path. Joining an OCI ref onto the project would turn
/// `docker.io/sbx/k:1` into a missing directory: absolute and URL-shaped
/// references pass through, `./x` and `../x` always resolve, a bare `x/y` only
/// if it exists on disk.
pub fn resolve_kit_ref(config_dir: &Path, kit: String) -> String {
    let p = Path::new(&kit);
    if p.is_absolute() || kit.contains("://") {
        return kit;
    }
    let joined = config_dir.join(p);
    if kit.starts_with("./") || kit.starts_with("../") || joined.exists() {
        return std::fs::canonicalize(&joined)
            .unwrap_or_else(|_| joined.components().collect::<PathBuf>())
            .to_string_lossy()
            .into_owned();
    }
    kit
}

/// The `schemaVersion` of a kit directory's spec, `None` when it has no
/// readable `spec.yaml` (a ZIP, a registry reference, a companion-pair kit
/// named after its stem).
pub fn spec_version(dir: &Path) -> Option<u32> {
    let raw = std::fs::read_to_string(dir.join("spec.yaml")).ok()?;
    raw.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim() == "schemaVersion").then(|| v.trim().trim_matches(|c| c == '"' || c == '\'').parse().ok())?
    })
}

/// Does the kit directory hold a v3 spec? sbx's `kit validate` reads only v1/v2
/// and refuses every v3 one (`field capabilities not found`), even the spec
/// project's own examples; a v3 kit is left to `sbx create`, which reads it.
pub fn is_v3_kit(dir: &Path) -> bool {
    spec_version(dir) == Some(3)
}

/// Why a local kit cannot be applied with sbx 0.51's `claude` workload, if it
/// cannot: a v1/v2 spec does not compose with a v3 one.
pub fn incompatible_spec(dir: &Path) -> Option<String> {
    match spec_version(dir)? {
        3 => None,
        v => Some(format!(
            "kit spec v{v} — sbx 0.51's Claude Code workload is a v3 kit, and v1/v2 kits do not compose with v3 ones. \
             Convert it (docs/kits-and-skills.md#writing-your-own-kit) or remove it from `kits`"
        )),
    }
}

/// What a local kit *is*, for a fingerprint: its `spec.yaml` and every file
/// under `files/` (the image content a v3 build copies), path then bytes, in
/// path order. A reference that is not a local directory is its own text.
pub fn content_text(kit: &str) -> String {
    let dir = Path::new(kit);
    let Ok(spec) = std::fs::read_to_string(dir.join("spec.yaml")) else {
        return kit.to_string();
    };
    let mut files = Vec::new();
    collect_files(&dir.join("files"), &mut files);
    files.sort();
    let mut out = spec;
    for f in files {
        let rel = f.strip_prefix(dir).unwrap_or(&f).to_string_lossy().replace('\\', "/");
        out.push('\u{0}');
        out.push_str(&rel);
        out.push('\u{0}');
        out.push_str(&String::from_utf8_lossy(&std::fs::read(&f).unwrap_or_default()));
    }
    out
}

fn collect_files(dir: &Path, acc: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        match e.file_type() {
            Ok(t) if t.is_dir() => collect_files(&e.path(), acc),
            Ok(t) if t.is_file() => acc.push(e.path()),
            _ => {}
        }
    }
}

/// The sandbox paths a spec declares `host-mount@1` at — what `--mount
/// HOST:TARGET` may fill (sbx 0.50). A path written as `${{ kit.args.X }}`
/// is the argument's default, the value sbx uses when none is passed.
pub fn host_mount_targets(spec: &str) -> Vec<String> {
    let Ok(doc) = serde_norway::from_str::<serde_norway::Value>(spec) else {
        return Vec::new();
    };
    let arg_default = |name: &str| doc.get("args")?.get(name)?.get("default")?.as_str().map(str::to_string);
    let Some(caps) = doc.get("capabilities").and_then(|c| c.as_sequence()) else {
        return Vec::new();
    };
    caps.iter()
        .filter(|c| c.get("type").and_then(|t| t.as_str()) == Some("com.docker.sandbox/host-mount@1"))
        .filter_map(|c| c.get("config")?.get("path")?.as_str())
        .filter_map(|p| match p.trim().strip_prefix("${{").and_then(|r| r.strip_suffix("}}")) {
            Some(expr) => arg_default(expr.trim().strip_prefix("kit.args.")?),
            None => Some(p.to_string()),
        })
        .collect()
}

/// Is a failed `sbx kit add` the "recreate the sandbox instead" refusal? Since
/// 0.51 that includes a sandbox created by an earlier sbx from its embedded
/// Claude spec, and a v3 kit refused for declaring what only creation applies.
pub fn add_needs_recreate(err: &str) -> bool {
    let lower = err.to_lowercase();
    ["does not yet apply", "recreate the sandbox", "must be recreated", "requires sandbox creation", "cannot be combined", "can't be combined"]
        .iter()
        .any(|s| lower.contains(s))
}

/// Did a `sbx exec … node` fail for want of `node` in the sandbox? The shell
/// says "not found" with status 127; sbx's own exec, "executable file not
/// found".
pub fn node_missing(err: &str) -> bool {
    let lower = err.to_lowercase();
    lower.contains("node") && (lower.contains("not found") || lower.contains("exit status 127") || lower.contains("no such file"))
}

/// Is a failed `sbx kit add` sbx asking a person to approve permissions the
/// kit adds (0.51: new network rules), which it cannot do without a terminal
/// and offers no flag for? Then the person runs the command themselves.
pub fn add_needs_approval(err: &str) -> bool {
    let lower = err.to_lowercase();
    lower.contains("stdin is not a terminal") || lower.contains("run this command in a terminal")
}

/// Does sbx need `kit.allowedSources` before it fetches this reference? Git
/// URLs and non-Docker-Hub registries are blocked by default.
pub fn needs_allowlist(kit: &str) -> bool {
    if ["http://", "https://", "git@", "git://", "ssh://"]
        .iter()
        .any(|p| kit.starts_with(p))
    {
        return true;
    }
    if !kit.starts_with('/') && !kit.starts_with('.') {
        if let Some(first) = kit.split('/').next() {
            if (first.contains('.') || first.contains(':')) && !first.contains("docker.io") {
                return true;
            }
        }
    }
    false
}

/// What sbxw reads of `sbx inspect --format json` (0.51).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inspected {
    /// The kits, by the path or reference each was applied from.
    pub kits: Vec<String>,
    /// The sbx that created it (`v0.51.0`); `None` for a sandbox an sbx
    /// older than 0.51 created, which did not record it.
    pub created_version: Option<String>,
}

/// `None` when `raw` is not that document.
pub fn parse_inspect(raw: &str) -> Option<Inspected> {
    let v: serde_json::Value = serde_json::from_str(raw.trim()).ok()?;
    v.as_object()?;
    let kits = v.get("kits").and_then(|k| k.as_array()).map(|a| a.iter().filter_map(|k| k.as_str().map(String::from)).collect()).unwrap_or_default();
    let created_version = v.get("created_version").and_then(|c| c.as_str()).map(str::trim).filter(|c| !c.is_empty()).map(String::from);
    Some(Inspected { kits, created_version })
}

/// The kits `sbx inspect --format json` lists; `None` when `raw` is not
/// that document.
pub fn applied_kits(raw: &str) -> Option<Vec<String>> {
    parse_inspect(raw).map(|i| i.kits)
}

/// Is `kit` already applied to a sandbox? Asked before `sbx kit add`, which
/// recreates the container, so a miss costs a recreate on every bring-up.
///
/// sbxw's own record of what it applied answers first, then what `sbx
/// inspect` lists: the same reference, or the same last path segment (a
/// bundled kit applied from another path is the same kit). A segment under
/// three characters is never matched on its own.
pub fn already_applied(kit: &str, applied: &[String], recorded: &[String]) -> bool {
    let segment = |k: &str| k.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
    let mine = segment(kit);
    recorded.iter().any(|r| r == kit) || applied.iter().any(|r| r == kit || (mine.len() >= 3 && segment(r) == mine))
}

/// Is a refused `policy allow|deny` the organisation owning the policy? That is
/// a governed host, not a broken one: a warning, never a failed bring-up.
pub fn policy_is_governed(err: &str) -> bool {
    let lower = err.to_lowercase();
    lower.contains("managed by your organization") || lower.contains("managed by your organisation")
}

/// The per-sandbox volumes kit (spec v3): one `volume@1` per path. sbx keeps
/// that storage when the sandbox is removed and reattaches it when the same
/// name is created again *with the same kit* — so the spec is rendered the
/// same way for the same list (sorted, deduplicated) and its directory
/// outlives the sandbox.
pub fn volumes_kit_spec(paths: &[String]) -> String {
    let mut paths: Vec<&str> = paths.iter().map(|p| p.trim_end_matches('/')).collect();
    paths.sort_unstable();
    paths.dedup();
    let mut out = String::from(
        "# syntax=docker/sandbox-kit:3\nschemaVersion: \"3\"\nkind: mixin\ndisplayName: sbxw-volumes\n\
         description: Persistent storage sbxw asks for (`volumes` in sbxw.toml)\ncapabilities:\n",
    );
    for p in paths {
        // YAML single-quoted scalar: a quote is doubled, nothing else escapes.
        out.push_str(&format!("  - type: com.docker.sandbox/volume@1\n    config:\n      path: '{}'\n", p.replace('\'', "''")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kits_line_is_rewritten_and_the_rest_of_the_file_kept() {
        let raw = "# top\ntemplate = \"auto\"\n\n# which kits\nkits = [\n  \"sbxw:codemap\", # inline\n]\n\n# Apps\n[[ports]]\nalias = \"myapp.local\"\nsandbox_port = 4200\n";
        let out = set_kits_line(raw, Some(&["sbxw:codemap".into(), "/a b/assets/retro".into()])).unwrap();
        assert!(out.starts_with("# top\ntemplate = \"auto\"\n\n# which kits\nkits = [\n    \"sbxw:codemap\",\n    \"/a b/assets/retro\",\n# inline\n]\n"), "{out}");
        assert!(out.ends_with("# Apps\n[[ports]]\nalias = \"myapp.local\"\nsandbox_port = 4200\n"), "{out}");
        let back: toml::Value = toml::from_str(&out).unwrap();
        assert_eq!(back["kits"].as_array().unwrap().len(), 2);
        assert!(back.get("ports").is_some());
    }

    #[test]
    fn kits_commented_out_inside_the_list_are_kept() {
        let raw = "kits = [\n#   \"/x/assets/headroom\",\n    \"/x/assets/codemap\",\n    \"/x/assets/retro\"\n]\n";
        let out = set_kits_line(raw, Some(&["/x/assets/codemap".into(), "sbxw:k8s-tools".into()])).unwrap();
        assert_eq!(out, "kits = [\n#   \"/x/assets/headroom\",\n    \"/x/assets/codemap\",\n    \"sbxw:k8s-tools\",\n]\n");
    }

    #[test]
    fn a_path_turned_into_its_name_keeps_the_comments_above_it() {
        let raw = "kits = [\n#   \"/x/assets/headroom\",\n    \"/x/assets/codemap\",\n    \"/x/my-kit\",\n    \"/x/assets/retro\"\n]\n";
        let out = set_kits_line(raw, Some(&["sbxw:codemap".into(), "/x/my-kit".into(), "sbxw:retro".into()])).unwrap();
        assert_eq!(out, "kits = [\n#   \"/x/assets/headroom\",\n    \"sbxw:codemap\",\n    \"/x/my-kit\",\n    \"sbxw:retro\",\n]\n");
        let out = set_kits_line(raw, Some(&["/x/my-kit".into()])).unwrap();
        assert_eq!(out, "kits = [\n#   \"/x/assets/headroom\",\n    \"/x/my-kit\",\n]\n", "a removed entry's comments go to the next one kept");
        let out = set_kits_line(raw, Some(&[])).unwrap();
        assert_eq!(out, "kits = [\n#   \"/x/assets/headroom\",\n]\n", "or to the end of the list");
    }

    #[test]
    fn a_kits_line_goes_at_the_root_never_under_ports() {
        let raw = "template = \"auto\"\n\n[[ports]]\nalias = \"a.local\"\nsandbox_port = 3000\n";
        let out = set_kits_line(raw, Some(&["sbxw:retro".into()])).unwrap();
        let back: toml::Value = toml::from_str(&out).unwrap();
        assert_eq!(back["kits"][0].as_str(), Some("sbxw:retro"), "{out}");
        assert!(back["ports"][0].get("kits").is_none(), "{out}");
        let gone = set_kits_line(&out, None).unwrap();
        assert!(!gone.contains("kits"), "no line: the defaults apply\n{gone}");
        assert_eq!(set_kits_line("", Some(&[])).unwrap().trim(), "kits = [\n]");
    }

    #[test]
    fn every_bundled_kit_is_described() {
        for b in BUNDLED {
            assert!(!about(b).is_empty(), "{b}");
        }
    }

    #[test]
    fn kit_refs_resolve_only_when_they_are_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("local/kit")).unwrap();
        let d = dir.path();
        assert_eq!(resolve_kit_ref(d, "/abs".into()), "/abs");
        assert_eq!(resolve_kit_ref(d, "git+https://x/y".into()), "git+https://x/y");
        assert_eq!(resolve_kit_ref(d, "docker.io/sbx/k:1".into()), "docker.io/sbx/k:1");
        let resolved = resolve_kit_ref(d, "local/kit".into());
        assert!(resolved.ends_with("local/kit") && Path::new(&resolved).is_absolute());
        let missing = resolve_kit_ref(d, "./not-yet".into());
        assert!(missing.ends_with("/not-yet") && !missing.contains("/./"));
    }

    #[test]
    fn host_mount_targets_are_read_with_their_argument_defaults() {
        // The spec project's own conformance fixture, trimmed.
        let spec = "schemaVersion: \"3\"\nkind: mixin\nargs:\n  mount_path:\n    default: /var/tmp/kit-tck-host\n\
                    capabilities:\n  - type: com.docker.sandbox/host-mount@1\n    config:\n      path: \"${{ kit.args.mount_path }}\"\n\
                    \x20 - type: com.docker.sandbox/host-mount@1\n    config: {path: /home/agent/history}\n\
                    \x20 - type: com.docker.sandbox/volume@1\n    config: {path: /data}\n";
        assert_eq!(host_mount_targets(spec), ["/var/tmp/kit-tck-host", "/home/agent/history"]);
        assert!(host_mount_targets("schemaVersion: \"2\"\nname: x\n").is_empty());
    }

    #[test]
    fn the_volumes_kit_is_the_same_kit_for_the_same_list() {
        let a = volumes_kit_spec(&["/home/agent/.npm".into(), "/home/agent/.cache/pip/".into()]);
        let b = volumes_kit_spec(&["/home/agent/.cache/pip".into(), "/home/agent/.npm".into(), "/home/agent/.npm".into()]);
        assert_eq!(a, b, "order, trailing slashes and repeats do not make another kit");
        let v: serde_norway::Value = serde_norway::from_str(&a).unwrap();
        assert_eq!(v["schemaVersion"], "3");
        assert_eq!(v["capabilities"][0]["type"], "com.docker.sandbox/volume@1");
        assert_eq!(v["capabilities"][0]["config"]["path"], "/home/agent/.cache/pip");
    }

    #[test]
    fn a_v3_kit_is_told_apart_by_its_spec() {
        let d = tempfile::tempdir().unwrap();
        assert!(!is_v3_kit(d.path()), "no spec, nothing to say");
        std::fs::write(d.path().join("spec.yaml"), "# syntax=docker/sandbox-kit:3\nschemaVersion: \"3\"\nkind: mixin\n").unwrap();
        assert!(is_v3_kit(d.path()));
        std::fs::write(d.path().join("spec.yaml"), "schemaVersion: '2'\nname: x\n").unwrap();
        assert!(!is_v3_kit(d.path()));
    }

    #[test]
    fn startup_and_refusals_are_recognised() {
        assert!(add_needs_recreate("kit declares commands.startup, which the kit-add recreate flow does not yet apply"));
        assert!(add_needs_recreate("sandbox was created from an embedded agent spec and must be recreated before kit add"));
        assert!(add_needs_recreate("v2 kits cannot be combined with v3 kits"));
        assert!(!add_needs_recreate("network unreachable"));
    }

    #[test]
    fn sources_needing_an_allowlist() {
        assert!(needs_allowlist("https://github.com/x/y"));
        assert!(needs_allowlist("ghcr.io/o/k:1"));
        assert!(!needs_allowlist("docker.io/sbx/k:1"));
        assert!(!needs_allowlist("sbx/k:1"));
        assert!(!needs_allowlist("./kit"));
        assert!(!needs_allowlist("/abs/kit"));
    }

    #[test]
    fn an_applied_kit_is_found_by_record_path_or_segment() {
        // What sbx 0.51 prints.
        let raw = r#"{"name":"smoke051","kits":["docker.io/sbx/claude@sha256:611f","/Users/t/sbxw/assets/base","/Users/t/sbxw/assets/codemap","/Users/t/.sbxw/kits/k8s-tools"],"state":"running"}"#;
        let applied = applied_kits(raw).unwrap();
        assert!(already_applied("/Users/t/sbxw/assets/codemap", &applied, &[]), "same path");
        assert!(already_applied("/Users/t/.sbxw/kits/codemap/", &applied, &[]), "a bundled kit from another path");
        assert!(already_applied("/x/k8s-tools", &applied, &[]));
        assert!(!already_applied("/Users/t/.sbxw/kits/agent-skills", &applied, &[]), "not listed");
        assert!(already_applied("/k/agent-skills", &[], &["/k/agent-skills".into()]), "sbxw's own record");
        assert!(!already_applied("/k/ba", &["/z/ba".into()], &[]), "a short name is not enough");
        assert_eq!(applied_kits("Name: x\nKits: a"), None, "only the JSON document");
        assert_eq!(parse_inspect(raw).unwrap().created_version, None, "an sbx before 0.51 recorded none");
        let new = parse_inspect(r#"{"name":"n","kits":[],"created_version":"v0.51.0"}"#).unwrap();
        assert_eq!(new.created_version.as_deref(), Some("v0.51.0"));
    }

    #[test]
    fn the_base_kit_goes_first_once() {
        assert_eq!(with_base(&[]), ["sbxw:base"]);
        assert_eq!(with_base(&["sbxw:codemap".into(), "sbxw:base".into()]), ["sbxw:base", "sbxw:codemap"]);
    }

    #[test]
    fn a_v1_or_v2_kit_is_refused_and_a_v3_one_is_not() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(incompatible_spec(d.path()), None, "no spec, nothing to say");
        std::fs::write(d.path().join("spec.yaml"), "schemaVersion: \"2\"\nkind: mixin\nname: x\n").unwrap();
        assert!(incompatible_spec(d.path()).unwrap().contains("kit spec v2"));
        std::fs::write(d.path().join("spec.yaml"), "# syntax=docker/sandbox-kit:3\nschemaVersion: '3'\nkind: mixin\n").unwrap();
        assert_eq!(incompatible_spec(d.path()), None);
        assert_eq!(spec_version(d.path()), Some(3));
    }

    #[test]
    fn a_kit_s_content_covers_its_files() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("spec.yaml"), "schemaVersion: \"3\"\n").unwrap();
        std::fs::create_dir_all(d.path().join("files/home/agent")).unwrap();
        std::fs::write(d.path().join("files/home/agent/a.md"), "one").unwrap();
        let k = d.path().to_string_lossy().into_owned();
        let before = content_text(&k);
        std::fs::write(d.path().join("files/home/agent/a.md"), "two").unwrap();
        assert_ne!(before, content_text(&k), "a changed file is a changed kit");
        assert_eq!(content_text("docker.io/sbx/k:1"), "docker.io/sbx/k:1");
    }

    #[test]
    fn governance_refusals_are_told_apart() {
        assert!(policy_is_governed("ERROR: rule is Managed by your organization"));
        assert!(!policy_is_governed("connection refused"));
    }
}
