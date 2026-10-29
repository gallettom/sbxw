//! MCP servers registered on the host with `sbx mcp`, as the web UI's MCP
//! dialog lists, creates and edits them.
//!
//! sbx has no `mcp edit`: an edit is `mcp rm` then `mcp add` under the same
//! name, so the spec is checked whole *before* anything is removed. Every
//! value goes to sbx in `--flag=value` form, so none can be read as a flag of
//! its own. A remote server is always added with `--skip-auth`: an OAuth
//! dance started from the daemon would wait on a browser nobody is watching,
//! so authorizing stays `sbx mcp auth <name>` in a terminal.

use serde::{Deserialize, Serialize};

const ARG_LIMIT: usize = 2048;

/// One server in `sbx mcp ls --format json` (0.48+: a bare array). `detail`
/// is the whole row: its fields beyond `name` and `type` are not documented,
/// so the UI reads what it recognises (a URL, a command) to prefill an edit.
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
}

/// A server name: it also names the server's secrets (`mcp:<name>:<key>`),
/// so it keeps to the characters a secret service name can carry.
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
                        return Err(format!("'{h}': a header is 'Name: value'"));
                    };
                    let key = key.trim().to_ascii_lowercase();
                    if key.is_empty() || seen.contains(&key) {
                        return Err(format!("'{h}': each header name once, and not empty"));
                    }
                    seen.push(key);
                    a.push(format!("--header={h}"));
                }
                a.push("--skip-auth".into());
            }
            "stdio" => {
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
            let mut rest = h.as_str();
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
        out
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
        assert!(McpSpec { kind: "other".into(), ..ok.clone() }.add_args().is_err());
        let stdio = McpSpec { name: "s".into(), kind: "stdio".into(), command: "node".into(), args: vec!["a,b".into()], ..Default::default() };
        assert!(stdio.add_args().is_err());
    }
}
