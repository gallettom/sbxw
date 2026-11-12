//! The network policy panel: three `sbx` listings turned into one view.
//!
//! No single sbx command answers "what can this sandbox reach?": `policy ls
//! <sb>` says which policies govern it, `--wide` names the resources, `policy
//! log` says what was allowed or blocked. Each is parsed schema-less (sbx's
//! columns are not pinned) and labelled by *role*. A listing that is not a
//! table is shown raw, never as "no rules": that would be the dangerous lie.

use crate::table;
use serde::Serialize;

/// What a column holds, for the ones the UI presents; `""` otherwise, so a
/// column sbx adds still shows.
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

pub const ARG_LIMIT: usize = 512;

/// One policy argument, or why it is unusable. sbx gets argv, not a shell, so
/// the danger is a value read as a flag.
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
                    // sbx 0.53 folds refused UDP destinations past the 20th
                    // into `<further blocked attempts>`: nothing to allow.
                    resource: if e.host.trim_start().starts_with('<') { String::new() } else { allow_resource(&e.host) },
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

    /// The log as a table view, so "Recent egress" renders it like any table.
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
    /// What an allow rule for it names: the host without its port. Empty for
    /// a row that is not one destination, which gets no Allow.
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

/// A bare host name, the only thing the Browser panel may allow: what a page
/// asked for is the sandbox's word, so no wildcard, port, list or flag.
pub fn clean_host(raw: &str) -> Result<String, String> {
    let h = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    let label = |l: &str| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if h.is_empty() || h.len() > 253 || !h.split('.').all(label) {
        return Err(format!("'{}' is not a host name", raw.trim().chars().take(80).collect::<String>()));
    }
    // A page can ask for this machine as easily as for a site, and a rule
    // without a port opens every service on it: that is a decision for the
    // network panel, where the port is named, not for one click here.
    let last = h.rsplit('.').next().unwrap_or_default();
    let this_machine = h == "localhost" || h.ends_with(".localhost") || h.ends_with(".internal") || h.ends_with(".local");
    if this_machine || last.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("'{h}' is this machine or an address on your network: allow it, with its port, in Ports & network"));
    }
    Ok(h)
}

/// The rule ids `after` has and `before` had not, among the rows naming
/// `host`: the rule an allow just made. Empty when the listing has no
/// rule-id column or no resource column: a rule that cannot be tied to the
/// host is not claimed, and then cannot be removed by id.
pub fn new_rule_ids(before: &PolicyView, after: &PolicyView, host: &str) -> Vec<String> {
    let ids = |v: &PolicyView| -> Vec<(String, String)> {
        let Some(i) = v.roles.iter().position(|r| *r == "rule_id") else { return vec![] };
        let h = v.roles.iter().position(|r| *r == "host");
        v.rows
            .iter()
            .filter_map(|row| {
                let id = row.get(i)?.trim();
                (!id.is_empty() && id != "-").then(|| (id.to_string(), h.and_then(|h| row.get(h)).cloned().unwrap_or_default()))
            })
            .collect()
    };
    let old: Vec<String> = ids(before).into_iter().map(|(id, _)| id).collect();
    ids(after)
        .into_iter()
        .filter(|(id, _)| !old.contains(id))
        .filter(|(_, res)| res.split([',', ' ']).any(|r| allow_resource(r).eq_ignore_ascii_case(host)))
        .map(|(id, _)| id)
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_browser_panel_allows_one_plain_host_only() {
        assert_eq!(clean_host(" Example.COM. ").unwrap(), "example.com");
        for bad in ["", "*.example.com", "a.com,b.com", "a.com:443", "-x", "a..b", "a b", "http://a.com", "**"] {
            assert!(clean_host(bad).is_err(), "{bad}");
        }
        for bad in ["localhost", "app.localhost", "host.docker.internal", "gateway.docker.internal", "127.0.0.1", "192.168.1.10", "printer.local"] {
            assert!(clean_host(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_rule_an_allow_made_is_the_new_row_naming_its_host() {
        const HEAD: &str = "RULE ID   SANDBOX   RESOURCE    ACTION\n";
        const R1: &str = "r-1       myapp     a.com       allow\n";
        let before = PolicyView::build(&format!("{HEAD}{R1}"), true, "myapp");
        let after = PolicyView::build(
            &format!("{HEAD}{R1}r-2       myapp     b.com:443   allow\nr-3       myapp     c.com       allow\n"),
            true,
            "myapp",
        );
        assert_eq!(new_rule_ids(&before, &after, "b.com"), ["r-2"]);
        assert!(new_rule_ids(&after, &after, "b.com").is_empty());
        let bare = PolicyView::build("RESOURCE   ACTION\nb.com      allow\n", true, "myapp");
        assert!(new_rule_ids(&before, &bare, "b.com").is_empty(), "no rule-id column, no id");
        let no_host = PolicyView::build("RULE ID   ACTION\nr-9       allow\n", true, "myapp");
        assert!(new_rule_ids(&before, &no_host, "b.com").is_empty(), "a rule that names no host is not claimed");
    }

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
    fn the_row_that_sums_up_further_attempts_offers_nothing_to_allow() {
        let raw = r#"{"allowed":[],"blocked":[{"host":"<further blocked attempts>","reason":"udp egress disabled","count":7,"last_seen":"2026-11-11T10:00:00Z"},{"host":"host.docker.internal","reason":"udp egress disabled","count":1,"last_seen":"2026-11-11T09:00:00Z"}]}"#;
        let b = EgressLog::parse(raw).unwrap().blocked_hosts();
        assert_eq!((b[0].host.as_str(), b[0].resource.as_str(), b[0].count), ("<further blocked attempts>", "", 7));
        assert_eq!(b[1].resource, "host.docker.internal");
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

    const LS: &str = "POLICY          SOURCE   APPLIES TO        SUMMARY\n\
                      local-default   local    all               12 rules\n\
                      p-myapp          local    sandbox:myapp      2 rules\n\
                      p-other         local    sandbox:other     1 rule\n";

    #[test]
    fn rows_of_other_sandboxes_are_dropped_and_counted() {
        let v = PolicyView::build(LS, true, "myapp");
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
