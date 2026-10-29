//! Kits: sbx's declarative extension point, and the facts sbxw has learnt
//! about applying them.
//!
//! * **Creation is still the cheapest moment to apply a kit.** `sbx kit add` on
//!   an existing sandbox recreates its container, so every configured kit goes
//!   to `sbx create --kit`. `kit add` also takes a kit that declares
//!   startup commands (they run after the add and on every start), so an
//!   existing sandbox gets every kit; a refusal sbx still gives is a message
//!   telling the person to recreate, never a recreate sbxw decides on its own.
//! * **sbx keeps a kit's path, not a copy.** A container swap re-resolves every
//!   kit applied before it by its original path; a kit directory that is gone
//!   fails the swap and names the *wrong* kit. Hence the durable OAuth kit
//!   directory, and the narrow repair of an old temp-dir reference.

use std::path::{Path, PathBuf};

/// The kits that ship inside sbxw, referred to as `sbxw:<name>` in `kits`.
pub const BUNDLED: [&str; 7] = ["codemap", "agent-skills", "md-to-pdf-tools", "retro", "k8s-tools", "git-identity", "ssh-agent"];

/// What a project without a `kits` list gets. k8s-tools is opt-in: only
/// Kubernetes projects need kubectl, k3d and skaffold. So are git-identity and
/// ssh-agent (v3 capability kits): each hands the sandbox something
/// of the person's — their name and email, their SSH keys' signatures.
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
        _ => "",
    }
}

/// The bundled kit a local path is a copy of: `…/assets/<name>` in an sbxw
/// checkout, or `~/.sbxw/kits/<name>` where sbxw writes them out. Configs
/// written before `sbxw:` names list kits that way; the Kits dialog says so.
pub fn bundled_by_path(kit: &str) -> Option<&'static str> {
    let p = Path::new(kit.trim_end_matches('/'));
    let name = p.file_name()?.to_str()?;
    let parent = p.parent()?.file_name()?.to_str()?;
    let found = BUNDLED.iter().find(|b| **b == name)?;
    (parent == "assets" || parent == "kits").then_some(*found)
}

/// The root `kits` of `raw` (an sbxw.toml) as written; `None` when there is no
/// such line, and the defaults apply.
pub fn read_kits_line(raw: &str) -> anyhow::Result<Option<Vec<String>>> {
    let v: toml::Value = toml::from_str(raw)?;
    Ok(v.get("kits").and_then(|k| k.as_array()).map(|a| a.iter().filter_map(|k| k.as_str().map(String::from)).collect()))
}

/// `raw` (an sbxw.toml) with its root `kits` replaced by `kits`, one entry
/// per line, or removed when `None` (the defaults then apply). Everything
/// else — comments, order, the `[[ports]]` tables — is left as written: this
/// is a file the person keeps by hand. A root key goes before the first
/// table, where TOML needs it; `toml_edit` places it there.
pub fn set_kits_line(raw: &str, kits: Option<&[String]>) -> anyhow::Result<String> {
    let mut doc: toml_edit::DocumentMut = raw.parse()?;
    match kits {
        None => {
            doc.remove("kits");
        }
        Some(kits) => {
            // Comments inside the list — often kits commented out for later —
            // belong to the slot they sit in: an entry kept keeps its own, one
            // replaced in place (a path turned into its `sbxw:` name) hands
            // them to its replacement, and one removed to the next entry kept.
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

/// One `kits = [...]` entry resolved against the config's directory — when, and
/// only when, it is a local path.
///
/// A reference is a directory, a ZIP, an OCI ref or a git URL. Joining an OCI
/// ref onto the project turns `docker.io/sbx/k:1` into a missing directory.
/// So: absolute and URL-shaped references pass through; `./x`, `../x` always
/// resolve; a bare `x/y` resolves only if it exists on disk.
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

/// The name a kit reference shows up as in `sbx inspect`: the spec's `name:`,
/// else the reference's last segment without tag or `.zip`. Only used to
/// *skip* a re-apply, so a miss costs a redundant re-apply, nothing more.
pub fn display_name(kit: &str, spec: Option<&str>) -> String {
    if let Some(spec) = spec {
        for line in spec.lines() {
            // `name:` (v2) or `displayName:` (v3, which has no identity name).
            if let Some(rest) = line.strip_prefix("name:").or_else(|| line.strip_prefix("displayName:")) {
                let n = rest.trim().trim_matches(|c| c == '"' || c == '\'');
                if !n.is_empty() {
                    return n.to_string();
                }
            }
        }
    }
    let last = kit.rsplit('/').next().unwrap_or(kit);
    last.split(':')
        .next()
        .unwrap_or(last)
        .trim_end_matches(".zip")
        .to_string()
}

/// Does the kit directory hold a v3 spec? sbx 0.50's `kit validate` reads
/// only v1/v2 specs and refuses every v3 one (`field capabilities not found`),
/// even the spec project's own examples — so a v3 kit is left to `sbx create`,
/// which does read it.
pub fn is_v3_kit(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join("spec.yaml")).is_ok_and(|raw| {
        raw.lines().any(|l| {
            l.split_once(':').is_some_and(|(k, v)| k.trim() == "schemaVersion" && v.trim().trim_matches(|c| c == '"' || c == '\'') == "3")
        })
    })
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

/// Is a failed `sbx kit add` the "recreate the sandbox instead" refusal?
pub fn add_needs_recreate(err: &str) -> bool {
    let lower = err.to_lowercase();
    lower.contains("does not yet apply") || lower.contains("recreate the sandbox from scratch")
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

/// The part of `sbx inspect` that lists kits, or the whole text when none can
/// be found. Narrowed because inspect also lists custom secrets, and a secret
/// containing a kit's name would skip a kit never applied.
pub fn inspect_kits_section(raw: &str) -> String {
    json_kits_section(raw)
        .or_else(|| text_kits_section(raw))
        .unwrap_or_else(|| raw.to_string())
}

/// Is `kit` already applied to a sandbox? Asked before `sbx kit add`, which
/// recreates the container, so a miss costs a recreate on every bring-up.
///
/// sbxw's own record of what it applied answers first. Then `sbx inspect`,
/// which lists kits by the *path or reference* they were applied from (0.49:
/// `Kits: /a/codemap, /b/agent-skills`): the same reference, or the same last
/// segment — a bundled kit applied from an older path is the same kit — is a
/// match. The spec's `name:` is only a last resort, matched against those
/// segments: it need not be the folder's name (`k8s-tools` is `k8s-dev-tools`).
pub fn already_applied(kit: &str, spec: Option<&str>, inspect_section: &str, recorded: &[String]) -> bool {
    if recorded.iter().any(|r| r == kit) {
        return true;
    }
    let applied: Vec<&str> = inspect_section
        .split([',', '\n'])
        .map(|r| r.trim().trim_start_matches("- ").trim_matches(|c| matches!(c, '"' | '[' | ']' | '\'')).trim())
        .filter(|r| !r.is_empty())
        .collect();
    let segment = display_name(kit, None);
    let named = display_name(kit, spec);
    applied.iter().any(|r| {
        let theirs = display_name(r.trim_end_matches('/'), None);
        *r == kit || (theirs.len() >= 3 && (theirs == segment || theirs == named))
    })
}

fn json_kits_section(raw: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let (_, kits) = value
        .as_object()?
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("kits") || k.eq_ignore_ascii_case("kit"))?;
    Some(kits.to_string())
}

fn text_kits_section(raw: &str) -> Option<String> {
    let indent = |l: &str| l.len() - l.trim_start().len();
    let mut lines = raw.lines();
    let (label_indent, first) = lines.find_map(|line| {
        let (label, rest) = line.trim_start().split_once(':')?;
        let label = label.trim_end();
        (label.eq_ignore_ascii_case("kits") || label.eq_ignore_ascii_case("kit"))
            .then(|| (indent(line), rest.trim().to_string()))
    })?;
    let mut section = vec![first];
    for line in lines {
        if !line.trim().is_empty() && indent(line) <= label_indent {
            break;
        }
        section.push(line.trim().to_string());
    }
    Some(section.join("\n"))
}

/// The path of a stale sbxw OAuth kit named in a failed `sbx kit add`, if that
/// is the failure. Deliberately narrow — the basename must be sbxw's own, the
/// path absent, its parent present — because the caller writes to a path
/// parsed out of a subprocess's stderr.
pub fn stale_oauth_kit_path(err: &str) -> Option<PathBuf> {
    if !err.contains("does not exist") {
        return None;
    }
    err.split('"').map(PathBuf::from).find(|p| {
        p.is_absolute()
            && p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("sbxw-oauth-kit-"))
            && !p.exists()
            && p.parent().is_some_and(|parent| parent.is_dir())
    })
}

/// Is a refused `policy allow|deny` the organisation owning the policy? That is
/// a governed host, not a broken one: a warning, never a failed bring-up.
pub fn policy_is_governed(err: &str) -> bool {
    let lower = err.to_lowercase();
    lower.contains("managed by your organization") || lower.contains("managed by your organisation")
}

/// `~/.claude/.credentials.json` for an OAuth token. The expiry is 2100; the
/// refresh token is the access token, so no refresh is attempted.
pub fn oauth_credentials_json(token: &str, subscription: &str) -> String {
    serde_json::json!({
        "claudeAiOauth": {
            "accessToken": token,
            "refreshToken": token,
            "expiresAt": 4_102_444_800_000u64,
            "scopes": ["user:inference"],
            "subscriptionType": subscription,
        }
    })
    .to_string()
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

/// The OAuth mixin kit, spec v2. Without credentials it is the same kit minus
/// the file — what restores a dangling reference when no token is at hand.
pub fn oauth_kit_spec(credentials_json: Option<&str>) -> String {
    let files = credentials_json.map_or_else(String::new, |creds| {
        // YAML single-quoted scalar: a quote is doubled, nothing else escapes.
        let quoted = creds.replace('\'', "''");
        format!(
            "\nsetup:\n  files:\n    - path: /home/agent/.claude/.credentials.json\n      content: '{quoted}'\n      mode: \"0600\"\n"
        )
    });
    format!(
        "schemaVersion: \"2\"\nkind: mixin\nname: claude-oauth\n\
         description: Injects OAuth credentials for Claude Code\n\n\
         permissions:\n  network:\n    allow:\n      - claude.ai\n{files}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kits_line_is_rewritten_and_the_rest_of_the_file_kept() {
        let raw = "# top\ntemplate = \"auto\"\n\n# which kits\nkits = [\n  \"sbxw:codemap\", # inline\n]\n\n# Apps\n[[ports]]\nalias = \"neos.local\"\nsandbox_port = 4200\n";
        let out = set_kits_line(raw, Some(&["sbxw:codemap".into(), "/a b/assets/retro".into()])).unwrap();
        assert!(out.starts_with("# top\ntemplate = \"auto\"\n\n# which kits\nkits = [\n    \"sbxw:codemap\",\n    \"/a b/assets/retro\",\n# inline\n]\n"), "{out}");
        assert!(out.ends_with("# Apps\n[[ports]]\nalias = \"neos.local\"\nsandbox_port = 4200\n"), "{out}");
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
    fn a_checkout_or_written_out_path_is_known_as_its_bundled_kit() {
        assert_eq!(bundled_by_path("/Users/t/sbxw 2/assets/codemap"), Some("codemap"));
        assert_eq!(bundled_by_path("/Users/t/.sbxw/kits/retro/"), Some("retro"));
        assert_eq!(bundled_by_path("/Users/t/my-kits/codemap"), None);
        assert_eq!(bundled_by_path("/Users/t/sbxw/assets/headroom"), None);
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
    fn display_names_come_from_the_spec_or_the_reference() {
        assert_eq!(display_name("/x/codemap", Some("schemaVersion: \"2\"\nname: codemap\n")), "codemap");
        assert_eq!(display_name("/x/git-identity", Some("schemaVersion: \"3\"\ndisplayName: git-identity\n")), "git-identity");
        assert_eq!(display_name("ghcr.io/o/kit:1.0", None), "kit");
        assert_eq!(display_name("/x/kit.zip", None), "kit");
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
    fn the_kits_section_is_found_in_text_and_json() {
        let text = "Name: neos\nKits:\n  - codemap\n  - k8s\nSecrets:\n  - codemap-token\n";
        let s = inspect_kits_section(text);
        assert!(s.contains("k8s") && !s.contains("token"));
        let json = r#"{"name":"neos","kits":["a"],"secrets":["b"]}"#;
        assert_eq!(inspect_kits_section(json), r#"["a"]"#);
        assert_eq!(inspect_kits_section("NAME KITS\nneos a"), "NAME KITS\nneos a");
    }

    #[test]
    fn an_applied_kit_is_found_by_record_path_or_segment() {
        // What sbx 0.49.2 prints.
        let raw = "    Name:            sbxw-2\n    Agent:           claude\n    Kits:            /Users/t/.sbxw/state/kits/sbxw-2-oauth, /Users/t/sbxw 2/assets/md-to-pdf-tools, /Users/t/sbxw 2/assets/codemap, /Users/t/.sbxw/kits/k8s-tools\n";
        let section = inspect_kits_section(raw);
        assert!(already_applied("/Users/t/sbxw 2/assets/codemap", None, &section, &[]), "same path");
        assert!(already_applied("/Users/t/.sbxw/kits/md-to-pdf-tools", None, &section, &[]), "a bundled kit from an older path");
        assert!(already_applied("/Users/t/.sbxw/kits/k8s-tools", Some("name: k8s-dev-tools\n"), &section, &[]), "the folder, not the spec name");
        assert!(!already_applied("/Users/t/.sbxw/kits/agent-skills", None, &section, &[]), "not listed");
        assert!(already_applied("/k/agent-skills", None, "", &["/k/agent-skills".into()]), "sbxw's own record");
        assert!(!already_applied("/k/map", None, &section, &[]), "a short name is not a substring match");
    }

    #[test]
    fn a_stale_oauth_path_is_recognised_narrowly() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sbxw-oauth-kit-1234");
        let err = format!("ERROR: re-resolve original kit 0 (\"{}\"): path does not exist", p.display());
        assert_eq!(stale_oauth_kit_path(&err), Some(p.clone()));
        let other = dir.path().join("someone-elses");
        let err = format!("(\"{}\"): path does not exist", other.display());
        assert_eq!(stale_oauth_kit_path(&err), None);
    }

    #[test]
    fn the_oauth_kit_is_v2_and_its_content_survives_quotes() {
        let creds = oauth_credentials_json("tok'en", "max");
        let spec = oauth_kit_spec(Some(&creds));
        assert!(spec.starts_with("schemaVersion: \"2\""));
        let doc: serde_norway::Value = serde_norway::from_str(&spec).unwrap();
        let content = doc["setup"]["files"][0]["content"].as_str().unwrap();
        let v: serde_json::Value = serde_json::from_str(content).unwrap();
        assert_eq!(v["claudeAiOauth"]["accessToken"], "tok'en");
        assert_eq!(v["claudeAiOauth"]["subscriptionType"], "max");
        assert!(!oauth_kit_spec(None).contains("setup:"));
    }

    #[test]
    fn governance_refusals_are_told_apart() {
        assert!(policy_is_governed("ERROR: rule is Managed by your organization"));
        assert!(!policy_is_governed("connection refused"));
    }
}
