//! The Browser panel's network grants.
//!
//! The sandbox's own Chromium goes through the sandbox proxy, so a page on a
//! host the policy does not allow is refused. The panel offers that host with
//! one click, and every allow it makes is written for **this sandbox only**
//! and remembered here until the browser is closed: the person then keeps
//! them or has them removed. Kept in memory: after a daemon restart the rules
//! stand as ordinary sandbox rules, removable from Ports & network.

use crate::app::App;
use anyhow::Result;
use sbxw_core::{policy, SandboxRef};
use serde::Serialize;

/// The script the panel runs in the sandbox, also shipped by the
/// agent-skills kit for the agent's own use: one file, one source.
pub const SCRIPT: &str = include_str!("../../../../assets/agent-skills/files/home/agent/.sbxw/browser/browser.mjs");
pub const SCRIPT_PATH: &str = "/home/agent/.sbxw/browser/browser.mjs";

/// One host allowed from the panel, and the rule ids sbx listed for it
/// (none when the rule could not be told apart with certainty).
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Grant {
    pub host: String,
    pub rules: Vec<String>,
}

/// A host whose rule could not be removed, and why.
#[derive(Serialize, Debug, PartialEq)]
pub struct Failed {
    pub host: String,
    pub why: String,
}

/// What closing the browser did with the grants.
#[derive(Serialize, Debug, Default, PartialEq)]
pub struct Settled {
    pub kept: Vec<String>,
    pub removed: Vec<String>,
    pub failed: Vec<Failed>,
}

/// The `browser` event: a sandbox's agent started or stopped browsing.
#[derive(Serialize, Clone, Debug)]
pub struct BrowserState {
    pub sandbox: SandboxRef,
    pub active: bool,
}

/// A listing whose rows can be compared row for row: sbx listed this
/// sandbox's rules, all of them. One that failed, fell back to the host-wide
/// rules or was cut short would make older rules look new.
fn whole(view: &policy::PolicyView) -> bool {
    view.error.is_none() && view.sandbox_scoped && view.truncated == 0
}

impl App {
    /// Allow `host` for `sandbox` alone and remember it as the panel's.
    pub async fn browser_allow(&self, sandbox: &SandboxRef, host: &str) -> Result<Grant> {
        let host = policy::clean_host(host).map_err(anyhow::Error::msg)?;
        let sbx = self.sbx_for(sandbox).await?;
        // One allow at a time: the rule it made is the one that appeared
        // between two listings, which another allow in between would blur.
        let _one = self.browser_allowing.lock().await;
        let before = sbx.policy_ls(&sandbox.name, true).await;
        tracing::info!("browser panel: allowing {host} for sandbox '{sandbox}' only");
        sbx.policy("allow", Some(&sandbox.name), &host).await?;
        let after = sbx.policy_ls(&sandbox.name, true).await;
        // Without two whole listings the rule is not identified, and is
        // then never removed by id: the person removes it by hand.
        let rules = match whole(&before) && whole(&after) {
            true => policy::new_rule_ids(&before, &after, &host),
            false => Vec::new(),
        };
        let mut book = self.browser_grants.lock().unwrap();
        let grants = book.entry(sandbox.clone()).or_default();
        if let Some(g) = grants.iter_mut().find(|g| g.host == host) {
            for r in rules {
                if !g.rules.contains(&r) {
                    g.rules.push(r);
                }
            }
            return Ok(g.clone());
        }
        let grant = Grant { host, rules };
        grants.push(grant.clone());
        Ok(grant)
    }

    /// The agent of `sandbox` started (or stopped) using its browser. Only
    /// a change is announced: every command of the agent's says so.
    pub fn browser_set_active(&self, sandbox: &SandboxRef, active: bool) {
        let changed = {
            let mut set = self.browsing.lock().unwrap();
            if active { set.insert(sandbox.clone()) } else { set.remove(sandbox) }
        };
        if changed {
            self.send(crate::bus::Event::Browser(BrowserState { sandbox: sandbox.clone(), active }));
        }
    }

    pub fn browsing(&self) -> Vec<SandboxRef> {
        let mut v: Vec<SandboxRef> = self.browsing.lock().unwrap().iter().cloned().collect();
        v.sort();
        v
    }

    pub fn browser_grants(&self, sandbox: &SandboxRef) -> Vec<Grant> {
        self.browser_grants.lock().unwrap().get(sandbox).cloned().unwrap_or_default()
    }

    /// The sandbox is gone: nobody browses in it, and its rules went with it.
    pub fn browser_forget(&self, sandbox: &SandboxRef) {
        self.browser_set_active(sandbox, false);
        self.browser_grants.lock().unwrap().remove(sandbox);
    }

    /// The browser closed: keep what the panel allowed, or remove it. A
    /// grant whose removal failed stays on the list, to be settled again.
    pub async fn browser_settle(&self, sandbox: &SandboxRef, keep: bool) -> Result<Settled> {
        let mut done = Settled::default();
        if keep {
            let grants = self.browser_grants.lock().unwrap().remove(sandbox).unwrap_or_default();
            done.kept = grants.into_iter().map(|g| g.host).collect();
            return Ok(done);
        }
        // Before the list is taken: a host that cannot be reached now leaves
        // it whole for the next try.
        let sbx = self.sbx_for(sandbox).await?;
        let grants = self.browser_grants.lock().unwrap().remove(sandbox).unwrap_or_default();
        let mut again = Vec::new();
        for g in grants {
            if g.rules.is_empty() {
                done.failed.push(Failed { host: g.host, why: "its rule could not be identified: remove it in Ports & network".into() });
                continue;
            }
            let mut left = Vec::new();
            let mut why = String::new();
            for rule in &g.rules {
                tracing::info!("browser panel: removing rule {rule} ({}) from sandbox '{sandbox}'", g.host);
                if let Err(e) = sbx.policy_rm(rule, Some(&sandbox.name)).await {
                    why = format!("{e:#}");
                    left.push(rule.clone());
                }
            }
            if left.is_empty() {
                done.removed.push(g.host);
            } else {
                done.failed.push(Failed { host: g.host.clone(), why });
                again.push(Grant { host: g.host, rules: left });
            }
        }
        if !again.is_empty() {
            self.browser_grants.lock().unwrap().entry(sandbox.clone()).or_default().extend(again);
        }
        Ok(done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_a_change_of_browsing_is_announced() {
        let (_d, app, _fake) = app_with(sbx_answer);
        let mut rx = app.bus.subscribe();
        app.browser_set_active(&sb(), true);
        app.browser_set_active(&sb(), true);
        assert_eq!(app.browsing(), [sb()]);
        app.browser_set_active(&sb(), false);
        let names: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).map(|e| e.data()).collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names[0].contains("\"active\":true") && names[1].contains("\"active\":false"));
        assert!(app.browsing().is_empty());
    }
    use crate::testing::{app_with, sbx_answer};
    use sbxw_sbx::runner::Output;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    fn sb() -> SandboxRef {
        SandboxRef::parse("myapp").unwrap()
    }

    #[tokio::test]
    async fn an_allow_is_for_this_sandbox_only_and_removed_by_its_rule_id() {
        let allowed = Arc::new(AtomicBool::new(false));
        let seen = allowed.clone();
        let (_d, app, fake) = app_with(move |a| {
            let line = a.join(" ");
            if line.starts_with("policy allow") {
                seen.store(true, Ordering::SeqCst);
                Output::ok("")
            } else if line.starts_with("policy ls") {
                let mut t = "RULE ID   SANDBOX   RESOURCE      ACTION\nr-1       myapp     a.com         allow\n".to_string();
                if seen.load(Ordering::SeqCst) {
                    t.push_str("r-2       myapp     example.com   allow\n");
                }
                Output::ok(&t)
            } else {
                sbx_answer(a)
            }
        });
        assert!(app.browser_allow(&sb(), "*.example.com").await.is_err(), "no wildcard from a page");
        let g = app.browser_allow(&sb(), "Example.com").await.unwrap();
        assert_eq!(g, Grant { host: "example.com".into(), rules: vec!["r-2".into()] });
        assert!(fake.log().contains(&"policy allow network --sandbox myapp example.com".to_string()), "{:?}", fake.log());
        assert_eq!(app.browser_grants(&sb()).len(), 1);

        let done = app.browser_settle(&sb(), false).await.unwrap();
        assert_eq!(done.removed, ["example.com"]);
        assert!(fake.called("policy rm network --id r-2 --sandbox myapp"), "{:?}", fake.log());
        assert!(!fake.called("policy rm network --id r-1"), "a rule the panel did not make stays");
        assert!(app.browser_grants(&sb()).is_empty());
    }

    #[tokio::test]
    async fn a_listing_that_is_not_this_sandboxes_whole_one_identifies_no_rule() {
        // sbx refuses the sandbox argument: the listing falls back to the
        // host-wide rules, where a rule someone else made would look new.
        let (_d, app, fake) = app_with(|a| {
            let line = a.join(" ");
            if line.starts_with("policy ls myapp") {
                Output::fail(1, "unknown sandbox")
            } else if line.starts_with("policy ls") {
                Output::ok("RULE ID   SANDBOX   RESOURCE      ACTION\ng-1       all       example.com   allow\n")
            } else {
                sbx_answer(a)
            }
        });
        let g = app.browser_allow(&sb(), "example.com").await.unwrap();
        assert!(g.rules.is_empty(), "{g:?}");
        let done = app.browser_settle(&sb(), false).await.unwrap();
        assert_eq!(done.failed[0].host, "example.com");
        assert!(!fake.called("policy rm"), "a host-wide rule is never removed from here");
    }

    #[tokio::test]
    async fn a_removed_sandbox_leaves_no_grant_and_nobody_browsing() {
        let (_d, app, _fake) = app_with(sbx_answer);
        app.browser_allow(&sb(), "example.com").await.unwrap();
        app.browser_set_active(&sb(), true);
        app.browser_forget(&sb());
        assert!(app.browser_grants(&sb()).is_empty() && app.browsing().is_empty());
        assert!(app.browser_allow(&sb(), "host.docker.internal").await.is_err(), "this machine is not one click away");
    }

    #[tokio::test]
    async fn keeping_removes_nothing_and_a_rule_without_an_id_is_reported() {
        let (_d, app, fake) = app_with(sbx_answer);
        app.browser_allow(&sb(), "example.com").await.unwrap();
        let kept = app.browser_settle(&sb(), true).await.unwrap();
        assert_eq!(kept.kept, ["example.com"]);
        assert!(!fake.called("policy rm"));

        app.browser_allow(&sb(), "example.org").await.unwrap();
        let done = app.browser_settle(&sb(), false).await.unwrap();
        assert!(done.removed.is_empty());
        assert_eq!(done.failed[0].host, "example.org");
        assert!(!fake.called("policy rm"), "nothing is removed by guesswork");
    }
}
