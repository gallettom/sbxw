//! Skills invoked from the phone: a closed list (`phone_skills`), the same
//! pattern as the model switch. The phone sends a name and optional plain
//! text; the daemon builds `/<name> <args>` itself, so the `/` rule of
//! messages stays whole. Only at an idle prompt.

use super::remote_access::{DeviceCtx, Reason, Refusal};
use crate::app::App;
use sbxw_core::remote_access::journal::Kind;
use sbxw_core::remote_access::{self as rules, MessageRefusal as M};
use sbxw_core::session::SessionKind;
use sbxw_core::SandboxRef;
use std::sync::Arc;

/// Longest args text, in bytes.
pub const ARGS_MAX: usize = 2000;

impl App {
    /// `GET /r/skills`: the names the phone may invoke.
    pub fn remote_skills(&self) -> serde_json::Value {
        serde_json::json!({ "skills": self.cfg.phone_skills })
    }

    /// Types `/<name> <args>` for a name of `phone_skills`. The args are
    /// held to a message's rules except the leading `/` (they never start
    /// the line), masked, one line, and checked again on the exact string.
    pub async fn remote_run_skill(self: &Arc<Self>, d: &DeviceCtx, sb: &SandboxRef, name: &str, args: &str) -> Result<(), Refusal> {
        if !self.cfg.phone_skills.iter().any(|s| s == name) {
            return Err(Refusal::Bad("skill: not in the list of skills".into()));
        }
        let args = args.trim();
        if args.len() > ARGS_MAX {
            return Err(Refusal::Bad("args: too long".into()));
        }
        if args.contains(['\n', '\r']) {
            return Err(Refusal::Reason(Reason::Control));
        }
        if !args.is_empty() {
            // A prefix takes the first-character rules out; control and
            // invisible characters are still refused.
            rules::check_message(&format!("a {args}")).map_err(|m| match m {
                M::Control => Refusal::Reason(Reason::Control),
                M::Command => Refusal::Reason(Reason::Command),
                M::Empty => Refusal::Reason(Reason::Empty),
            })?;
        }
        let masked = if args.is_empty() { String::new() } else { self.anonymise(args).map_err(Refusal::Unavailable)?.text };
        let line = if masked.is_empty() { format!("/{name}") } else { format!("/{name} {masked}") };
        if line.contains(['\n', '\r']) || sbxw_core::terminal::sanitize_typed(&line) != line {
            return Err(Refusal::Reason(Reason::Control));
        }
        if let Some(why) = self.remote_read_only(sb) {
            self.remote_record(d, Kind::Refused);
            return Err(Refusal::Conflict(format!("'{sb}' is read only from the phone ({why})")));
        }
        if let Err(r) = self.remote_can_type(sb) {
            self.remote_record(d, Kind::Refused);
            return Err(Refusal::Reason(r));
        }
        self.push_as_person(sb, SessionKind::Agent, &line).await.map_err(Refusal::Conflict)?;
        tracing::info!("remote access: device {}: skill", d.id);
        self.remote_record(d, Kind::Skill);
        Ok(())
    }
}
