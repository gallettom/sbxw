//! MCP servers registered on the host with `sbx mcp`, as the web UI's MCP
//! dialog lists, creates and edits them.
//!
//! sbx has no `mcp edit`: an edit is `mcp rm` then `mcp add` under the same
//! name, so the spec is checked whole *before* anything is removed. Every
//! value goes to sbx in `--flag=value` form, so none can be read as a flag.
//! A remote server is always added with `--skip-auth`: an OAuth flow started
//! from the daemon would wait on a browser nobody watches, so authorizing stays
//! `sbx mcp auth <name>` in a terminal.
//!
//! A server that offers no dynamic client registration (GitHub's) needs a
//! pre-registered OAuth client: `client_id` goes to sbx as `--client-id`, and
//! a confidential client's secret is stored as `mcp:<name>:client_secret`
//! ([`CLIENT_SECRET`]) before the add, never in argv. That key is reserved: a
//! header placeholder cannot take it.
//!
//! **A credential header takes a placeholder.** A header whose name says it
//! carries a credential ([`credential_header`]) must hold a `${key}` in its
//! value ([`header_refusal`]): typed in clear, the secret would be in sbx's
//! argv, its stored definition and `sbx mcp ls`. [`McpSpec::add_args`] is the
//! one place this is enforced (the daemon's route, Jarvis and every
//! `Sbx::mcp_add` go through it); the web UI and the desktop mirror it, on the
//! table `tests/js/mcp-header-cases.json` all three are tested on.

use serde::{Deserialize, Serialize};

const ARG_LIMIT: usize = 2048;
const CLIENT_ID_LIMIT: usize = 256;

/// The secret key a pre-registered OAuth client's secret is stored under,
/// `mcp:<name>:client_secret`, as sbx reads it.
pub const CLIENT_SECRET: &str = "client_secret";

/// What sbx says when a server needs `--client-id` (sbx 0.5x: "... no dynamic
/// client registration); --client-id is required to register it").
const NEEDS_CLIENT_ID: &str = "--client-id is required";

/// One row of `sbx mcp ls --format json` (0.48+: a bare array). `detail` is
/// the whole row: fields beyond `name` and `type` are undocumented, so the UI
/// reads what it recognises (a URL, a command) to prefill an edit.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct McpServer {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub detail: serde_json::Value,
}

pub fn parse_mcp_ls(raw: &str) -> Option<Vec<McpServer>> {
    let rows: Vec<serde_json::Value> = serde_json::from_str(raw.trim()).ok()?;
    Some(
        rows.iter()
            .filter_map(|v| {
                Some(McpServer {
                    name: v.get("name")?.as_str()?.to_string(),
                    kind: v.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string(),
                    detail: v.clone(),
                })
            })
            .collect(),
    )
}

/// A server as the dialog's form describes it.
#[derive(Deserialize, Debug, Clone, Default)]
pub struct McpSpec {
    pub name: String,
    /// `remote` (an HTTP endpoint) or `stdio` (a command run on the host).
    pub kind: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub headers: Vec<String>,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub dir: String,
    /// A pre-registered OAuth client's id, remote only; empty is none.
    #[serde(default)]
    pub client_id: String,
}

/// Also names the server's secrets (`mcp:<name>:<key>`): only characters a
/// secret service name can carry.
pub fn check_name(name: &str) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() || n.len() > 64 {
        return Err("a server name is 1 to 64 characters".into());
    }
    if n.starts_with('-') || !n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) {
        return Err(format!("'{n}': a server name is letters, digits, '-', '_' and '.', not starting with '-'"));
    }
    Ok(n.to_string())
}

fn value(v: &str, what: &str) -> Result<String, String> {
    let v = v.trim();
    if v.is_empty() {
        return Err(format!("{what} is required"));
    }
    if v.len() > ARG_LIMIT || v.contains(['\n', '\r', '\0']) {
        return Err(format!("{what} must be one line of at most {ARG_LIMIT} characters"));
    }
    Ok(v.to_string())
}

/// An OAuth client id: one token of the characters client ids use (letters,
/// digits, `-._~:@/+=`), not starting with '-'.
pub fn check_client_id(id: &str) -> Result<String, String> {
    let id = id.trim();
    if id.is_empty() || id.len() > CLIENT_ID_LIMIT {
        return Err(format!("an OAuth client ID is 1 to {CLIENT_ID_LIMIT} characters"));
    }
    if id.starts_with('-') || !id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~' | ':' | '@' | '/' | '+' | '=')) {
        return Err("an OAuth client ID is letters, digits and - . _ ~ : @ / + =, not starting with '-'".into());
    }
    Ok(id.to_string())
}

/// sbx's refusal of an add, with what to do when the server wants a client
/// id and none was given. The refusal is sbx's words, kept whole.
pub fn add_error_hint(spec: &McpSpec, said: &str) -> Option<&'static str> {
    (said.contains(NEEDS_CLIENT_ID) && spec.client_id.trim().is_empty())
        .then_some("this server needs an OAuth client ID: fill the \"OAuth client ID\" field with the id of an OAuth app registered with the server's provider (GitHub: Settings, Developer settings, OAuth Apps)")
}

/// The words that make a header name a credential's, as the start or the end
/// of one of its word parts (`tests/js/mcp-header-cases.json`).
pub const CREDENTIAL_WORDS: &[&str] = &["auth", "token", "secret", "password", "passwd", "credential", "session", "cookie", "key"];

/// Does a header of this name carry a credential? The name, lowercased, is
/// split into word parts at every character that is not a letter or a digit;
/// it does when one part starts or ends with a word of [`CREDENTIAL_WORDS`]:
/// `Authorization`, `Proxy-Authorization`, `Cookie`, `X-API-Key`, `ApiKey`,
/// `X-Auth-Token`, `Mcp-Session-Id`; not `Accept`, `X-MCP-Toolsets`,
/// `Keep-Alive`. A harmless name caught by it (`Idempotency-Key`) still goes
/// with its value stored as a secret.
pub fn credential_header(name: &str) -> bool {
    name.to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|p| !p.is_empty())
        .any(|p| CREDENTIAL_WORDS.iter().any(|w| p.starts_with(w) || p.ends_with(w)))
}

/// The well-formed `${key}` placeholders in `text`, once each, in order: a
/// key is letters, digits, '-' and '_'.
pub fn placeholders_in(text: &str, out: &mut Vec<String>) {
    let mut rest = text;
    while let Some(i) = rest.find("${") {
        rest = &rest[i + 2..];
        let Some(j) = rest.find('}') else { break };
        let key = &rest[..j];
        if !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')) && !out.iter().any(|k| k == key) {
            out.push(key.to_string());
        }
        rest = &rest[j + 1..];
    }
}

/// Why a header (`Name: value`) cannot be sent as written, for a credential
/// header whose value holds no placeholder; `None` otherwise. The reason names
/// the header only, never a part of its value.
pub fn header_refusal(header: &str) -> Option<String> {
    let (name, value) = header.split_once(':')?;
    let name = name.trim();
    if !credential_header(name) {
        return None;
    }
    let mut keys = Vec::new();
    placeholders_in(value, &mut keys);
    if !keys.is_empty() {
        return None;
    }
    let example = if matches!(name.to_ascii_lowercase().as_str(), "authorization" | "proxy-authorization") { "Bearer ${token}" } else { "${token}" };
    Some(format!("{name} carries a credential: write its value with a placeholder, such as `{example}`, and sbxw asks for the secret separately"))
}

impl McpSpec {
    /// The argv of `sbx mcp add` for this spec, or why it cannot be added.
    pub fn add_args(&self) -> Result<Vec<String>, String> {
        let name = check_name(&self.name)?;
        let mut a = vec!["mcp".to_string(), "add".into(), name];
        match self.kind.as_str() {
            "remote" => {
                a.push(format!("--url={}", value(&self.url, "the URL")?));
                let mut seen = Vec::new();
                for h in self.headers.iter().filter(|h| !h.trim().is_empty()) {
                    let h = value(h, "a header")?;
                    let Some((key, _)) = h.split_once(':') else {
                        // Not repeated: a line without its name may be a pasted credential.
                        return Err("a header is 'Name: value', and one has no ':'".into());
                    };
                    let shown = key.trim();
                    let key = shown.to_ascii_lowercase();
                    if key.is_empty() || seen.contains(&key) {
                        // The name only: a value may be a credential.
                        return Err(format!("'{shown}': each header name once, and not empty"));
                    }
                    if let Some(why) = header_refusal(&h) {
                        return Err(why);
                    }
                    seen.push(key);
                    a.push(format!("--header={h}"));
                }
                if self.placeholders().iter().any(|k| k == CLIENT_SECRET) {
                    return Err(format!("${{{CLIENT_SECRET}}} is the OAuth client secret's own key: name the header's secret otherwise"));
                }
                if !self.client_id.trim().is_empty() {
                    a.push(format!("--client-id={}", check_client_id(&self.client_id)?));
                }
                a.push("--skip-auth".into());
            }
            "stdio" => {
                if !self.client_id.trim().is_empty() {
                    return Err("an OAuth client ID is for a remote server, not a local command".into());
                }
                a.push(format!("--command={}", value(&self.command, "the command")?));
                let args: Vec<&str> = self.args.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
                if args.iter().any(|s| s.contains(',')) {
                    return Err("sbx separates arguments with commas, so one cannot contain a comma".into());
                }
                if !args.is_empty() {
                    a.push(format!("--args={}", value(&args.join(","), "the arguments")?));
                }
                if !self.dir.trim().is_empty() {
                    a.push(format!("--dir={}", value(&self.dir, "the directory")?));
                }
            }
            k => return Err(format!("'{k}': a server is remote or stdio")),
        }
        Ok(a)
    }

    /// The `${key}` placeholders in the headers, each a secret sbx resolves
    /// from `mcp:<name>:<key>`.
    pub fn placeholders(&self) -> Vec<String> {
        let mut out = Vec::new();
        for h in &self.headers {
            placeholders_in(h, &mut out);
        }
        out
    }

    /// Whether the add stores an OAuth client secret: only with a client id.
    pub fn takes_client_secret(&self) -> bool {
        self.kind == "remote" && !self.client_id.trim().is_empty()
    }
}

/// The secret service a header placeholder is stored under.
pub fn secret_service(server: &str, key: &str) -> String {
    format!("mcp:{server}:{key}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_servers_are_a_bare_array() {
        let s = parse_mcp_ls(r#"[{"name":"github","type":"remote","gateway":{"url":"x"}},{"type":"stdio"}]"#).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].name.as_str(), s[0].kind.as_str()), ("github", "remote"));
        assert_eq!(s[0].detail["gateway"]["url"], "x", "the whole row is kept");
    }

    #[test]
    fn a_remote_server_is_added_with_flags_that_cannot_be_misread() {
        let spec = McpSpec {
            name: "acme".into(),
            kind: "remote".into(),
            url: " https://mcp.acme.com/mcp ".into(),
            headers: vec!["Authorization: Bearer ${api-key}".into(), "".into()],
            ..Default::default()
        };
        assert_eq!(
            spec.add_args().unwrap(),
            ["mcp", "add", "acme", "--url=https://mcp.acme.com/mcp", "--header=Authorization: Bearer ${api-key}", "--skip-auth"]
        );
        assert_eq!(spec.placeholders(), ["api-key"]);
        assert_eq!(secret_service("acme", "api-key"), "mcp:acme:api-key");
    }

    #[test]
    fn a_stdio_server_joins_its_arguments_with_commas() {
        let spec = McpSpec {
            name: "pw".into(),
            kind: "stdio".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "@playwright/mcp@latest".into()],
            dir: "/srv".into(),
            ..Default::default()
        };
        assert_eq!(spec.add_args().unwrap(), ["mcp", "add", "pw", "--command=npx", "--args=-y,@playwright/mcp@latest", "--dir=/srv"]);
    }

    #[test]
    fn a_bad_spec_is_refused_before_sbx_sees_it() {
        let ok = McpSpec { name: "a".into(), kind: "remote".into(), url: "https://x".into(), ..Default::default() };
        assert!(ok.add_args().is_ok());
        assert!(McpSpec { name: "--force".into(), ..ok.clone() }.add_args().is_err());
        assert!(McpSpec { name: "a b".into(), ..ok.clone() }.add_args().is_err());
        assert!(McpSpec { url: "".into(), ..ok.clone() }.add_args().is_err());
        assert!(McpSpec { headers: vec!["NoColon".into()], ..ok.clone() }.add_args().is_err());
        assert!(McpSpec { headers: vec!["A: 1".into(), "a: 2".into()], ..ok.clone() }.add_args().is_err());
        assert!(McpSpec { url: "https://x\n--force".into(), ..ok.clone() }.add_args().is_err());
        assert!(McpSpec { kind: "other".into(), ..ok }.add_args().is_err());
        let stdio = McpSpec { name: "s".into(), kind: "stdio".into(), command: "node".into(), args: vec!["a,b".into()], ..Default::default() };
        assert!(stdio.add_args().is_err());
    }

    #[test]
    fn a_client_id_goes_to_sbx_as_one_flag_before_skip_auth() {
        let spec = McpSpec {
            name: "github".into(),
            kind: "remote".into(),
            url: "https://api.githubcopilot.com/mcp".into(),
            headers: vec!["X-MCP-Toolsets: repos".into()],
            client_id: " Iv1.0123abcd ".into(),
            ..Default::default()
        };
        assert_eq!(
            spec.add_args().unwrap(),
            ["mcp", "add", "github", "--url=https://api.githubcopilot.com/mcp", "--header=X-MCP-Toolsets: repos", "--client-id=Iv1.0123abcd", "--skip-auth"]
        );
        assert!(spec.takes_client_secret());
        assert!(!McpSpec { client_id: "".into(), ..spec.clone() }.takes_client_secret());
        assert_eq!(secret_service("github", CLIENT_SECRET), "mcp:github:client_secret");
        for ok in ["abc", "my-preregistered-client", "a.b_c~d:e@f/g+h=", "x".repeat(256).as_str()] {
            assert!(check_client_id(ok).is_ok(), "{ok}");
        }
        for bad in ["-x", "--force", "a b", "a\nb", "a;b", "a,b", "é", "x".repeat(257).as_str()] {
            assert!(McpSpec { client_id: bad.into(), ..spec.clone() }.add_args().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_client_id_is_remote_only_and_its_secret_key_is_reserved() {
        let stdio = McpSpec { name: "s".into(), kind: "stdio".into(), command: "node".into(), client_id: "abc".into(), ..Default::default() };
        assert!(stdio.add_args().unwrap_err().contains("remote"));
        assert!(!stdio.takes_client_secret());
        let clash = McpSpec { name: "a".into(), kind: "remote".into(), url: "https://x".into(), headers: vec!["X-Key: ${client_secret}".into()], ..Default::default() };
        assert!(clash.add_args().unwrap_err().contains("client secret"));
    }

    #[test]
    fn sbx_asking_for_a_client_id_gets_a_hint_only_when_none_was_given() {
        let said = r#"error: server "github" advertises no registration endpoint (no dynamic client registration); --client-id is required to register it"#;
        let none = McpSpec { name: "github".into(), kind: "remote".into(), url: "https://x".into(), ..Default::default() };
        assert!(add_error_hint(&none, said).unwrap().contains("OAuth client ID"));
        assert!(add_error_hint(&McpSpec { client_id: "abc".into(), ..none.clone() }, said).is_none());
        assert!(add_error_hint(&none, "error: something else").is_none());
    }

    /// The table the web UI's and the desktop's copies of the rule are tested on.
    const CASES: &str = include_str!("../../../tests/js/mcp-header-cases.json");

    fn cases() -> serde_json::Value {
        serde_json::from_str(CASES).unwrap()
    }

    fn strs<'a>(v: &'a serde_json::Value) -> Vec<&'a str> {
        v.as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect()
    }

    fn remote(headers: &[&str]) -> McpSpec {
        McpSpec { name: "acme".into(), kind: "remote".into(), url: "https://x".into(), headers: headers.iter().map(|h| h.to_string()).collect(), ..Default::default() }
    }

    #[test]
    fn the_shared_table_is_the_core_rule() {
        let c = cases();
        assert_eq!(strs(&c["words"]), CREDENTIAL_WORDS);
        for name in strs(&c["credential"]) {
            assert!(credential_header(name), "{name} carries a credential");
            for v in strs(&c["values"]["ok"]) {
                assert!(remote(&[&format!("{name}: {v}")]).add_args().is_ok(), "{name}: {v}");
            }
            for v in strs(&c["values"]["refused"]) {
                let e = remote(&[&format!("{name}: {v}")]).add_args().unwrap_err();
                let key = name.to_ascii_lowercase();
                let example = c["example"].get(key.as_str()).unwrap_or(&c["example"]["other"]).as_str().unwrap();
                assert_eq!(e, c["message"].as_str().unwrap().replace("NAME", name).replace("EXAMPLE", example), "{name}: {v}");
            }
        }
        for name in strs(&c["free"]) {
            assert!(!credential_header(name), "{name} is free");
            for v in strs(&c["values"]["refused"]) {
                assert!(remote(&[&format!("{name}: {v}")]).add_args().is_ok(), "{name}: {v}");
            }
        }
    }

    #[test]
    fn a_literal_credential_is_refused_without_a_word_of_its_value() {
        let token = "ghp_Sup3rS3cretT0kenValue";
        for h in [format!("Authorization: Bearer {token}"), format!("x-api-key:{token}"), format!("COOKIE: sid={token}"), format!("X-Auth-Token: {token} ${{")] {
            let e = remote(&["Accept: application/json", &h]).add_args().unwrap_err();
            assert!(e.contains("carries a credential") && e.contains("placeholder"), "{e}");
            assert!(!e.contains(token) && !e.contains("Sup3r") && !e.contains("sid="), "{e}");
        }
        let e = remote(&["Authorization: Bearer abc"]).add_args().unwrap_err();
        assert_eq!(e, "Authorization carries a credential: write its value with a placeholder, such as `Bearer ${token}`, and sbxw asks for the secret separately");
        // Other refusals of a header name it, never repeat its value.
        let e = remote(&["Authorization: Bearer ${a}", "authorization: Bearer s3cr3t"]).add_args().unwrap_err();
        assert!(e.contains("authorization") && !e.contains("s3cr3t"), "{e}");
        let e = remote(&["Bearer s3cr3t"]).add_args().unwrap_err();
        assert!(!e.contains("s3cr3t"), "{e}");
    }

    #[test]
    fn a_credential_header_with_a_placeholder_goes_and_other_headers_stay_free() {
        let spec = remote(&["Authorization: Bearer ${token}", "X-MCP-Toolsets: repos", "Accept: application/json"]);
        assert_eq!(spec.add_args().unwrap()[4], "--header=Authorization: Bearer ${token}");
        assert_eq!(spec.placeholders(), ["token"]);
        assert!(header_refusal("Accept: application/json").is_none());
        assert!(header_refusal("Api-Key: ${k}").is_none());
        assert!(header_refusal("Api-Key: $k").is_some());
    }
}
