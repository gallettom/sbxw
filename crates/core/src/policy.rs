//! The network policy panel: three `sbx` listings turned into one honest view.
//!
//! No single sbx command answers "what can this sandbox reach?": `policy ls
//! <sb>` says which policies govern it, `--wide` names the resources, `policy
//! log` says what was actually allowed or blocked. Each is parsed
//! schema-less — sbx's columns are not pinned — and labelled by *role* so the
//! UI lays it out by meaning. A listing that is not a table is shown raw rather
//! than as "no rules", which would be the dangerous lie.

use crate::table;
use serde::Serialize;

/// What a column holds, for the ones the UI knows how to present; `""` for
/// anything else, so a column sbx adds shows up instead of vanishing.
pub fn column_roles(columns: &[String]) -> Vec<&'static str> {
    columns
        .iter()
        .map(|c| {
            let c = c.trim().to_ascii_uppercase();
            match () {
                // Before the generic id arm: `policy rm` takes a *rule* id, and a
                // policy id there would delete a whole policy.
                _ if c == "RULE ID" || c == "RULE_ID" || c == "RULEID" => "rule_id",
                _ if c == "POLICY" || c == "ID" || c.ends_with(" ID") => "id",
                _ if c == "SOURCE" || c == "ORIGIN" => "source",
                _ if c.starts_with("APPLIES") || c == "SCOPE" || c == "TARGET" => "applies",
                _ if c == "SUMMARY" || c == "RULES" => "summary",
                _ if c == "SANDBOX" => "sandbox",
                _ if c == "HOST" || c == "DOMAIN" || c == "RESOURCE" => "host",
                _ if c == "RULE" => "rule",
                _ if c == "REASON" || c == "DETAIL" => "reason",
                _ if c == "ACTION" || c == "DECISION" => "action",
                _ if c == "TYPE" => "type",
                _ if c == "STATUS" || c == "STATE" => "status",
                _ if c.contains("SEEN") || c == "TIME" || c == "WHEN" => "when",
                _ => "",
            }
        })
        .collect()
}

/// Does a policy scoped `applies` govern `sandbox`? `all` and anything
/// unrecognised do: one row too many is cosmetic, a hidden rule is a lie.
pub fn applies_to(applies: &str, sandbox: &str) -> bool {
    match applies.trim().split_once(':') {
        Some(("sandbox", target)) => target.trim() == sandbox,
        _ => true,
    }
}

/// Most rows a view renders; the UI says how many were dropped.
pub const VIEW_ROW_LIMIT: usize = 200;

#[derive(Serialize, Default, Debug, PartialEq)]
pub struct PolicyView {
    pub columns: Vec<String>,
    pub roles: Vec<&'static str>,
    pub rows: Vec<Vec<String>>,
    /// Rows dropped as other sandboxes'.
    pub other_sandboxes: usize,
    /// Rows beyond the display limit.
    pub truncated: usize,
    /// sbx accepted the sandbox argument; false means host-wide rules, which
    /// must not be presented as this sandbox's.
    pub sandbox_scoped: bool,
    pub raw: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl PolicyView {
    pub fn failed(error: String) -> Self {
        Self {
            error: Some(error),
            ..Default::default()
        }
    }

    /// One listing, scoped to `sandbox`.
    pub fn build(raw: &str, sandbox_scoped: bool, sandbox: &str) -> Self {
        let t = table::parse(raw);
        let roles = column_roles(&t.columns);
        let (mut rows, other_sandboxes) = scope_rows(t.rows, &roles, sandbox);
        let truncated = rows.len().saturating_sub(VIEW_ROW_LIMIT);
        rows.truncate(VIEW_ROW_LIMIT);
        Self {
            columns: t.columns,
            roles,
            rows,
            other_sandboxes,
            truncated,
            sandbox_scoped,
            raw: raw.trim_end().to_string(),
            error: None,
        }
    }
}

/// Drop the rows of other sandboxes, counting them. Handles `APPLIES TO`
/// (`sandbox:<name>` / `all`) and a bare `SANDBOX` column.
fn scope_rows(rows: Vec<Vec<String>>, roles: &[&str], sandbox: &str) -> (Vec<Vec<String>>, usize) {
    let col = roles
        .iter()
        .position(|r| *r == "applies")
        .map(|i| (i, false))
        .or_else(|| roles.iter().position(|r| *r == "sandbox").map(|i| (i, true)));
    let Some((i, bare)) = col else {
        return (rows, 0);
    };
    let total = rows.len();
    let kept: Vec<Vec<String>> = rows
        .into_iter()
        .filter(|r| match r.get(i) {
            None => true,
            Some(cell) if bare && !cell.trim().is_empty() => {
                applies_to(&format!("sandbox:{}", cell.trim()), sandbox)
            }
            Some(cell) => applies_to(cell, sandbox),
        })
        .collect();
    let dropped = total - kept.len();
    (kept, dropped)
}

/// Rule ids of `--wide` rows allowing exactly `resource`, split by what the
/// listing proves about their scope: `mine` are scoped to `sandbox` by name
/// (safe to remove), `unsure` carry no scope column (a host-wide rule would
/// look the same, so they are reported, never removed).
pub struct Found {
    pub mine: Vec<String>,
    pub unsure: Vec<String>,
}

pub fn rule_ids_for(view: &PolicyView, resource: &str, sandbox: &str) -> Found {
    let col = |role: &str| view.roles.iter().position(|r| *r == role);
    let mut found = Found { mine: vec![], unsure: vec![] };
    let (Some(id), Some(host)) = (col("rule_id"), col("host")) else {
        return found;
    };
    let action = col("action");
    let scope = col("applies").map(|i| (i, false)).or_else(|| col("sandbox").map(|i| (i, true)));
    for r in &view.rows {
        let hit = r.get(host).is_some_and(|h| h.split(',').any(|h| h.trim() == resource))
            && action.is_none_or(|a| r.get(a).is_some_and(|v| v.eq_ignore_ascii_case("allow")));
        let Some(rule) = r.get(id).filter(|v| !v.is_empty()).cloned() else { continue };
        if !hit {
            continue;
        }
        match scope {
            Some((i, bare)) => {
                let cell = r.get(i).map(|c| c.trim()).unwrap_or("");
                let named = if bare { cell == sandbox } else { cell == format!("sandbox:{sandbox}") };
                if named {
                    found.mine.push(rule);
                }
            }
            None => found.unsure.push(rule),
        }
    }
    found
}

pub const ARG_LIMIT: usize = 512;

/// One policy argument, or why it cannot be used. sbx gets argv, never a
/// shell, so the danger is a value read as a flag.
pub fn clean_arg(value: &str, what: &str) -> Result<String, String> {
    let v = value.trim();
    if v.is_empty() {
        return Err(format!("{what} is required"));
    }
    if v.len() > ARG_LIMIT {
        return Err(format!("{what} is too long (max {ARG_LIMIT} characters)"));
    }
    if v.starts_with('-') {
        return Err(format!("{what} must not start with '-' — sbx would read it as a flag"));
    }
    Ok(v.to_string())
}

/// What narrows a network rule to some HTTP requests: methods, a path glob,
/// and query parameters the request must carry (`--query name=value`, sbx
/// 0.50, a value pattern; every one is required). Empty is the whole host.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HttpMatch {
    pub methods: Vec<String>,
    pub path: Option<String>,
    pub query: Vec<String>,
}

impl HttpMatch {
    /// From what the panel sends: comma-separated methods and `name=value`
    /// pairs, a path glob. Each part checked the way a resource is.
    pub fn parse(methods: &str, path: &str, query: &str) -> Result<Self, String> {
        let list = |raw: &str| raw.split(',').map(str::trim).filter(|p| !p.is_empty()).map(String::from).collect::<Vec<_>>();
        let methods: Vec<String> = list(methods).into_iter().map(|m| m.to_ascii_uppercase()).collect();
        if let Some(m) = methods.iter().find(|m| !m.chars().all(|c| c.is_ascii_uppercase())) {
            return Err(format!("method {m:?} — expected names like GET, POST or ANY"));
        }
        let path = Some(path.trim()).filter(|p| !p.is_empty()).map(String::from);
        if let Some(p) = &path {
            if !p.starts_with('/') || p.len() > ARG_LIMIT {
                return Err(format!("path {p:?} — expected a glob starting with /, such as /v1/**"));
            }
        }
        let query = list(query);
        for q in &query {
            let ok = q.split_once('=').is_some_and(|(n, _)| !n.trim().is_empty()) && !q.starts_with('-') && q.len() <= ARG_LIMIT;
            if !ok {
                return Err(format!("query {q:?} — expected name=value (the value may be a pattern, release*)"));
            }
        }
        Ok(Self { methods, path, query })
    }

    pub fn is_empty(&self) -> bool {
        self.methods.is_empty() && self.path.is_none() && self.query.is_empty()
    }

    /// The flags, in sbx's spelling.
    pub fn args(&self) -> Vec<String> {
        let mut a = Vec::new();
        if !self.methods.is_empty() {
            a.extend(["--method".into(), self.methods.join(",")]);
        }
        if let Some(p) = &self.path {
            a.extend(["--path".into(), p.clone()]);
        }
        for q in &self.query {
            a.extend(["--query".into(), q.clone()]);
        }
        a
    }
}

/// "a.com, b.com" (what a person types) → "a.com,b.com" (what sbx wants).
pub fn clean_resources(raw: &str) -> Result<String, String> {
    let cleaned = clean_arg(raw, "resource")?;
    let parts: Vec<String> = cleaned
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| clean_arg(p, "resource"))
        .collect::<Result<_, _>>()?;
    if parts.is_empty() {
        return Err("resource is required".into());
    }
    Ok(parts.join(","))
}

/// One host in `sbx policy log --format json` (0.48+).
#[derive(Serialize, Debug, Clone, PartialEq, Default)]
pub struct EgressEntry {
    pub host: String,
    pub rule: String,
    pub reason: String,
    pub proxy_type: String,
    pub count: u64,
    pub last_seen: String,
}

/// `sbx policy log --format json`: `{updated_at, allowed: [...], blocked: [...]}`.
#[derive(Serialize, Debug, Default, PartialEq)]
pub struct EgressLog {
    pub allowed: Vec<EgressEntry>,
    pub blocked: Vec<EgressEntry>,
}

impl EgressLog {
    /// `None` when `raw` is not that document, so the caller can read a table.
    pub fn parse(raw: &str) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(raw.trim()).ok()?;
        let obj = v.as_object()?;
        if !obj.contains_key("allowed") && !obj.contains_key("blocked") {
            return None;
        }
        let list = |k: &str| -> Vec<EgressEntry> {
            let mut out: Vec<EgressEntry> = obj
                .get(k)
                .and_then(|x| x.as_array())
                .map(|rows| rows.iter().filter_map(entry).collect())
                .unwrap_or_default();
            // Most recent first: what the person just tried is what they look for.
            out.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
            out
        };
        Some(Self { allowed: list("allowed"), blocked: list("blocked") })
    }

    /// Blocked hosts grouped by host (one row per host, counts summed), the
    /// ones an organisation policy blocks marked so the UI offers no allow.
    pub fn blocked_hosts(&self) -> Vec<BlockedHost> {
        let mut out: Vec<BlockedHost> = Vec::new();
        for e in &self.blocked {
            match out.iter_mut().find(|b| b.host == e.host) {
                Some(b) => b.count += e.count,
                None => out.push(BlockedHost {
                    resource: allow_resource(&e.host),
                    host: e.host.clone(),
                    count: e.count,
                    last_seen: e.last_seen.clone(),
                    reason: e.reason.clone(),
                    governed: is_org_block(&e.reason) || is_org_block(&e.rule),
                }),
            }
        }
        out.truncate(VIEW_ROW_LIMIT);
        out
    }

    /// The log as a table view, so the panel's "Recent egress" renders it the
    /// way it renders a parsed table.
    pub fn view(&self) -> PolicyView {
        let columns: Vec<String> = ["HOST", "ACTION", "RULE", "REASON", "COUNT", "LAST SEEN"].map(String::from).to_vec();
        let mut rows: Vec<(String, Vec<String>)> = self
            .allowed
            .iter()
            .map(|e| ("allow", e))
            .chain(self.blocked.iter().map(|e| ("deny", e)))
            .map(|(action, e)| {
                (e.last_seen.clone(), vec![e.host.clone(), action.into(), e.rule.clone(), e.reason.clone(), e.count.to_string(), e.last_seen.clone()])
            })
            .collect();
        rows.sort_by(|a, b| b.0.cmp(&a.0));
        let mut rows: Vec<Vec<String>> = rows.into_iter().map(|(_, r)| r).collect();
        let truncated = rows.len().saturating_sub(VIEW_ROW_LIMIT);
        rows.truncate(VIEW_ROW_LIMIT);
        PolicyView {
            roles: column_roles(&columns),
            columns,
            rows,
            truncated,
            sandbox_scoped: true,
            ..Default::default()
        }
    }
}

fn entry(v: &serde_json::Value) -> Option<EgressEntry> {
    let text = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let host = text("host");
    if host.is_empty() {
        return None;
    }
    Some(EgressEntry {
        host,
        rule: text("rule"),
        reason: text("reason"),
        proxy_type: text("proxy_type"),
        count: v.get("count").and_then(|x| x.as_u64()).unwrap_or(1),
        last_seen: text("last_seen"),
    })
}

/// A host the sandbox tried to reach and was refused.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct BlockedHost {
    pub host: String,
    /// What an allow rule for it names: the host without its port.
    pub resource: String,
    pub count: u64,
    pub last_seen: String,
    pub reason: String,
    /// Blocked by organisation policy: sbx will refuse an allow, so none is offered.
    pub governed: bool,
}

fn is_org_block(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("org policy") || t.contains("organization") || t.contains("organisation") || t.contains("governance")
}

/// The host part of a logged destination, the form an allow rule takes:
/// `api.x.com:443` → `api.x.com`. A bracketed IPv6 literal is kept whole.
pub fn allow_resource(host: &str) -> String {
    let h = host.trim().trim_end_matches('.');
    if h.starts_with('[') {
        return h.split_once(']').map(|(a, _)| format!("{a}]")).unwrap_or_else(|| h.to_string());
    }
    match h.rsplit_once(':') {
        Some((name, port)) if !name.contains(':') && port.chars().all(|c| c.is_ascii_digit()) => name.to_string(),
        _ => h.to_string(),
    }
}

/// One server in `sbx mcp ls --format json` (0.48+: a bare array).
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct McpServer {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
}

pub fn parse_mcp_ls(raw: &str) -> Option<Vec<McpServer>> {
    let rows: Vec<serde_json::Value> = serde_json::from_str(raw.trim()).ok()?;
    Some(
        rows.iter()
            .filter_map(|v| {
                Some(McpServer {
                    name: v.get("name")?.as_str()?.to_string(),
                    kind: v.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string(),
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_http_match_is_checked_and_spelled_as_sbx_flags() {
        let m = HttpMatch::parse("get, head", "/search", "team=platform, label=release*").unwrap();
        assert_eq!(m.args().join(" "), "--method GET,HEAD --path /search --query team=platform --query label=release*");
        assert!(HttpMatch::parse("", "", "").unwrap().is_empty());
        assert!(HttpMatch::parse("GET;rm", "", "").is_err());
        assert!(HttpMatch::parse("", "search", "").is_err(), "a path is a glob from /");
        assert!(HttpMatch::parse("", "", "=x").is_err());
        assert!(HttpMatch::parse("", "", "--sandbox=x").is_err(), "never a flag");
    }

    #[test]
    fn the_egress_log_json_groups_blocked_hosts() {
        let raw = r#"{"updated_at":"2026-10-26T10:00:00Z",
          "allowed":[{"sandbox_id":"s1","host":"api.anthropic.com:443","proxy_type":"https","rule":"r1","reason":"allowed","count":9,"first_seen":"2026-10-26T09:00:00Z","last_seen":"2026-10-26T09:59:00Z"}],
          "blocked":[{"sandbox_id":"s1","host":"registry.npmjs.org:443","proxy_type":"https","rule":"default-deny-all","reason":"no matching allow rule","count":3,"first_seen":"a","last_seen":"2026-10-26T09:58:00Z"},
                     {"sandbox_id":"s1","host":"registry.npmjs.org:443","proxy_type":"https","rule":"default-deny-all","reason":"no matching allow rule","count":2,"first_seen":"a","last_seen":"2026-10-26T09:50:00Z"},
                     {"sandbox_id":"s1","host":"pastebin.com:443","proxy_type":"https","rule":"","reason":"Blocked by org policy","count":1,"first_seen":"a","last_seen":"2026-10-26T09:40:00Z"}]}"#;
        let log = EgressLog::parse(raw).unwrap();
        let b = log.blocked_hosts();
        assert_eq!(b.len(), 2);
        assert_eq!((b[0].host.as_str(), b[0].count), ("registry.npmjs.org:443", 5));
        assert!(!b[0].governed && b[1].governed, "an org block offers no allow");
        let v = log.view();
        assert_eq!(v.rows.len(), 4);
        assert_eq!(v.rows[0][1], "allow", "newest first");
        assert!(EgressLog::parse("HOST  RULE  REASON\n").is_none());
    }

    #[test]
    fn an_allow_rule_names_the_host_not_the_port() {
        assert_eq!(allow_resource("registry.npmjs.org:443"), "registry.npmjs.org");
        assert_eq!(allow_resource("example.com."), "example.com");
        assert_eq!(allow_resource("[::1]:8080"), "[::1]");
        assert_eq!(allow_resource("10.0.0.1"), "10.0.0.1");
    }

    #[test]
    fn mcp_servers_are_a_bare_array() {
        let s = parse_mcp_ls(r#"[{"name":"github","type":"remote","gateway":{"url":"x"}},{"type":"stdio"}]"#).unwrap();
        assert_eq!(s, vec![McpServer { name: "github".into(), kind: "remote".into() }]);
    }

    const LS: &str = "POLICY          SOURCE   APPLIES TO        SUMMARY\n\
                      local-default   local    all               12 rules\n\
                      p-neos          local    sandbox:neos      2 rules\n\
                      p-other         local    sandbox:other     1 rule\n";

    #[test]
    fn rows_of_other_sandboxes_are_dropped_and_counted() {
        let v = PolicyView::build(LS, true, "neos");
        assert_eq!(v.rows.len(), 2);
        assert_eq!(v.other_sandboxes, 1);
        assert_eq!(v.roles, ["id", "source", "applies", "summary"]);
    }

    #[test]
    fn rule_id_and_policy_id_are_distinct_roles() {
        let cols: Vec<String> = ["RULE ID", "POLICY", "NAME ID"].map(String::from).to_vec();
        assert_eq!(column_roles(&cols), ["rule_id", "id", "id"]);
    }

    #[test]
    fn stale_ui_port_rules_are_found_by_resource() {
        let wide = "RULE ID   SANDBOX   RESOURCE          ACTION\n\
                    r-1       neos      localhost:7681    allow\n\
                    r-2       neos      localhost:7682    allow\n\
                    r-3       other     localhost:7681    allow\n";
        let v = PolicyView::build(wide, true, "neos");
        let f = rule_ids_for(&v, "localhost:7681", "neos");
        assert_eq!(f.mine, ["r-1"]);
        assert!(f.unsure.is_empty());
        let global = "RULE ID   APPLIES TO     RESOURCE          ACTION\n\
                      r-9       all            localhost:7681    allow\n";
        let v = PolicyView::build(global, true, "neos");
        assert!(rule_ids_for(&v, "localhost:7681", "neos").mine.is_empty(), "a host-wide rule is never revoked");
        let bare = "RULE ID   RESOURCE          ACTION\n\
                    r-5       localhost:7681    allow\n";
        let v = PolicyView::build(bare, true, "neos");
        assert_eq!(rule_ids_for(&v, "localhost:7681", "neos").unsure, ["r-5"]);
    }

    #[test]
    fn args_reject_flag_lookalikes_and_empties() {
        assert!(clean_arg("-rf", "x").is_err());
        assert!(clean_arg("  ", "x").is_err());
        assert!(clean_arg(&"a".repeat(600), "x").is_err());
        assert_eq!(clean_resources(" a.com, ,b.com ").unwrap(), "a.com,b.com");
        assert!(clean_resources("a.com,-x").is_err());
    }

    #[test]
    fn a_non_table_keeps_its_raw_text() {
        let v = PolicyView::build("policy listing unavailable\n", false, "n");
        assert!(v.rows.is_empty() && v.raw == "policy listing unavailable" && !v.sandbox_scoped);
    }
}
