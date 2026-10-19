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

/// The rule ids of `--wide` rows that allow exactly `resource` for `sandbox` —
/// how a stale rule from an older sbxw is found to be revoked.
pub fn rule_ids_for(view: &PolicyView, resource: &str) -> Vec<String> {
    let col = |role: &str| view.roles.iter().position(|r| *r == role);
    let (Some(id), Some(host)) = (col("rule_id"), col("host")) else {
        return Vec::new();
    };
    let action = col("action");
    view.rows
        .iter()
        .filter(|r| r.get(host).is_some_and(|h| h.split(',').any(|h| h.trim() == resource)))
        .filter(|r| action.is_none_or(|a| r.get(a).is_some_and(|v| v.eq_ignore_ascii_case("allow"))))
        .filter_map(|r| r.get(id).cloned().filter(|v| !v.is_empty()))
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(rule_ids_for(&v, "localhost:7681"), ["r-1"]);
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
