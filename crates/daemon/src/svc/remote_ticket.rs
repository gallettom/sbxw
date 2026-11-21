//! A paired phone writes a ticket on a leader's Kanban (`API-kanban.md`).
//! The ticket is made by the web UI's own writer ([`App::ticket_create`]),
//! so it lands in Planned like the Mac's. The text goes through the same
//! anonymise rules a phone message does before it is stored.
//!
//! Never logged, never journaled: a title or a description. The journal
//! entry names the device and the kind only.

use super::remote_access::{DeviceCtx, Refusal};
use crate::app::App;
use sbxw_core::remote_access::journal::Kind;
use sbxw_core::tickets::{Draft, MAX_DESCRIPTION, MAX_TITLE};
use sbxw_core::SandboxRef;
use serde_json::{json, Value};
use std::sync::Arc;

impl App {
    /// `{n, num, masked}` of the new ticket: `masked` counts the words the
    /// rules replaced in the title and the description together.
    pub fn remote_ticket_create(self: &Arc<Self>, d: &DeviceCtx, sb: &SandboxRef, title: &str, description: &str) -> Result<Value, Refusal> {
        let title = title.trim();
        let description = description.trim();
        if title.is_empty() {
            return Err(Refusal::Bad("a ticket needs a title".into()));
        }
        if title.chars().count() > MAX_TITLE {
            return Err(Refusal::Bad(format!("a title is at most {MAX_TITLE} characters")));
        }
        if description.chars().count() > MAX_DESCRIPTION {
            return Err(Refusal::Bad(format!("a description is at most {MAX_DESCRIPTION} characters")));
        }
        let t = self.anonymise(title).map_err(Refusal::Unavailable)?;
        let (desc, desc_count) = if description.is_empty() {
            (String::new(), 0)
        } else {
            let s = self.anonymise(description).map_err(Refusal::Unavailable)?;
            (s.text, s.count)
        };
        let v = self.ticket_create(sb, Draft { title: Some(t.text), description: Some(desc), priority: None }).map_err(Refusal::Bad)?;
        self.remote_record(d, Kind::TicketCreate);
        let n = v.get("n").and_then(Value::as_u64).unwrap_or(0);
        Ok(json!({ "ok": true, "n": n, "num": sbxw_core::tickets::num(n as u32), "masked": t.count + desc_count }))
    }
}
