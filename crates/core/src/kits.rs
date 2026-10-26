//! Kits: sbx's declarative extension point, and the facts sbxw has learnt
//! about applying them.
//!
//! * **Creation is still the cheapest moment to apply a kit.** `sbx kit add` on
//!   an existing sandbox recreates its container, so every configured kit goes
//!   to `sbx create --kit`. Since 0.48 `kit add` also takes a kit that declares
//!   startup commands (they run after the add and on every start), so an
//!   existing sandbox gets every kit; a refusal sbx still gives is a message
//!   telling the person to recreate, never a recreate sbxw decides on its own.
//! * **sbx keeps a kit's path, not a copy.** A container swap re-resolves every
//!   kit applied before it by its original path; a kit directory that is gone
//!   fails the swap and names the *wrong* kit. Hence the durable OAuth kit
//!   directory, and the narrow repair of an old temp-dir reference.

use std::path::{Path, PathBuf};

/// The kits that ship inside sbxw, referred to as `sbxw:<name>` in `kits`.
pub const BUNDLED: [&str; 6] = ["codemap", "agent-skills", "md-to-pdf-tools", "k8s-tools", "git-identity", "ssh-agent"];

/// What a project without a `kits` list gets. k8s-tools is opt-in: only
/// Kubernetes projects need kubectl, k3d and skaffold. So are git-identity and
/// ssh-agent (v3 capability kits, sbx 0.48+): each hands the sandbox something
/// of the person's — their name and email, their SSH keys' signatures.
pub const DEFAULT_KITS: [&str; 3] = ["sbxw:codemap", "sbxw:agent-skills", "sbxw:md-to-pdf-tools"];

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

/// Is a failed `sbx kit validate --offline` an sbx too old for the flag (0.48)?
pub fn validate_unsupported(err: &str) -> bool {
    let e = err.to_lowercase();
    e.contains("unknown flag") || e.contains("unknown command")
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
    fn startup_and_refusals_are_recognised() {
        assert!(validate_unsupported("Error: unknown flag: --offline"));
        assert!(!validate_unsupported("spec.yaml: unknown field `setpu`"));
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
