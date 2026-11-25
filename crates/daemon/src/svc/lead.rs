//! Lead mode as a state the daemon can vouch for (reliability audit, D3 to
//! D5): a sandbox's lead is `waiting` (the person pressed Team, the agent is
//! not idle yet), `activating` (`/lead` was typed, the session has not
//! confirmed), or `leading` (the session called `lead_ack`). Off is absent.
//!
//! `leading` is what [`App::leading`] lists: a bound lead with no pending
//! step. Waiting and activating live in `App::lead_pending`, in memory only:
//! a lead goes to disk (`LeadRecord`) once it is confirmed, so a restarted
//! daemon never brings back a lead nobody confirmed.
//!
//! Every change of state sends `Event::Lead`, every fall back to off
//! `Event::LeadEnd`: a client reads `/api/lead-states` on either.

use crate::app::{now_ms, App};
use crate::bus::Event;
use crate::store::LeadRecord;
use sbxw_core::messages;
use sbxw_core::session::{SessionKey, SessionKind};
use sbxw_core::SandboxRef;
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

/// How long a Team click waits for an idle agent before it falls back to off.
pub const LEAD_WAIT_MAX_MS: u64 = 10 * 60 * 1000;
/// How long the session has, once `/lead` is typed (or a new conversation of
/// the lead started), to call `lead_ack`.
pub const LEAD_ACK_MAX_MS: u64 = 90 * 1000;
/// How long the reason of a fall back to off stays in `/api/lead-states`.
pub const LEAD_REASON_KEEP_MS: u64 = 2 * 60 * 1000;
/// How often waits and deadlines are looked at ([`App::lead_tick`]).
pub const TICK: Duration = Duration::from_secs(2);

/// What `lead_ack` answers a session sbxw did not make lead (a `/lead`
/// typed by hand, a cancelled or expired request). Fixed text.
pub const ACK_REFUSED: &str = "Refused: sbxw has no Team request for this sandbox, so lead mode \
     is off and this session is not the lead. Do not lead: propose no plan and follow no thread. \
     Tell the person that a /lead typed by hand does not turn lead mode on: to make this session \
     the lead, they press Team in sbxw.";

/// `lead_ack` while the Team click still waits for an idle agent.
pub const ACK_WAITING: &str = "Not yet: the person pressed Team, and sbxw types /lead itself as \
     soon as this session is idle. Do not lead yet: finish your turn.";

/// `lead_ack` from a thread's session.
pub const ACK_THREAD: &str = "Refused: a thread cannot be the lead. Only the sandbox's main \
     session leads; carry on with your thread's goal.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LeadPhase {
    Waiting,
    Activating,
    Leading,
    /// Only with a reason, a short while after a fall back to off.
    Off,
}

/// A lead asked for and not confirmed yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pending {
    /// [`LeadPhase::Waiting`] or [`LeadPhase::Activating`].
    pub phase: LeadPhase,
    pub since_ms: u64,
    /// Past it, the lead falls back to off.
    pub until_ms: u64,
}

/// One sandbox in `/api/lead-states`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LeadState {
    pub state: LeadPhase,
    pub since_ms: u64,
    /// The last failure message, "" when none.
    pub reason: String,
}

impl App {
    /// Every sandbox whose lead is not off, and those that fell back to off
    /// in the last [`LEAD_REASON_KEEP_MS`] (state `off`, with the reason).
    pub fn lead_states(&self) -> BTreeMap<String, LeadState> {
        let leading = self.leading();
        let now = now_ms();
        let mut out = BTreeMap::new();
        {
            let mut failed = self.lead_failed.lock().unwrap();
            failed.retain(|_, (_, at)| now.saturating_sub(*at) <= LEAD_REASON_KEEP_MS);
            for (sb, (reason, at)) in failed.iter() {
                out.insert(sb.to_string(), LeadState { state: LeadPhase::Off, since_ms: *at, reason: reason.clone() });
            }
        }
        let reason_of = |out: &BTreeMap<String, LeadState>, sb: &str| out.get(sb).map(|s| s.reason.clone()).unwrap_or_default();
        let recs = self.lead_recs.lock().unwrap().clone();
        for name in leading {
            let since_ms = SandboxRef::parse(&name).ok().and_then(|sb| recs.get(&sb).map(|r| r.since_ms)).unwrap_or(now);
            let reason = reason_of(&out, &name);
            out.insert(name, LeadState { state: LeadPhase::Leading, since_ms, reason });
        }
        for (sb, p) in self.lead_pending.lock().unwrap().iter() {
            let name = sb.to_string();
            let reason = reason_of(&out, &name);
            out.insert(name, LeadState { state: p.phase, since_ms: p.since_ms, reason });
        }
        out
    }

    /// The pending step of `sandbox`'s lead, if any.
    pub fn lead_pending_of(&self, sandbox: &SandboxRef) -> Option<Pending> {
        self.lead_pending.lock().unwrap().get(sandbox).copied()
    }

    /// The desktop's Team click (`POST /api/sandboxes/:name/lead`): `/lead`
    /// is typed only into an idle agent, by the phone's rule
    /// (`remote_can_type`); otherwise the lead waits, and [`App::lead_tick`]
    /// types it once the agent is idle, for [`LEAD_WAIT_MAX_MS`] at most.
    /// Idempotent: a lead already asked for, activating or leading is left.
    pub async fn lead_request(self: &Arc<Self>, sandbox: &SandboxRef) -> Result<(), String> {
        if self.lead_pending_of(sandbox).is_some() || self.leading().contains(&sandbox.to_string()) {
            return Ok(());
        }
        self.lead_install(sandbox).await?;
        // No terminal yet (the pane is still connecting, the sandbox was
        // stopped): opened here, and its agent is waited for like a busy one.
        if !self.sessions.is_live(&SessionKey::agent(sandbox)) {
            self.open_session(&SessionKey::agent(sandbox)).await.map_err(|e| format!("{e:#}"))?;
        }
        if self.remote_can_type(sandbox).is_ok() {
            return self.lead_type(sandbox, false).await;
        }
        let now = now_ms();
        {
            let mut pending = self.lead_pending.lock().unwrap();
            if pending.contains_key(sandbox) {
                return Ok(());
            }
            pending.insert(sandbox.clone(), Pending { phase: LeadPhase::Waiting, since_ms: now, until_ms: now + LEAD_WAIT_MAX_MS });
        }
        self.lead_failed.lock().unwrap().remove(sandbox);
        tracing::info!("plans: '{sandbox}' waits for an idle agent before /lead is typed");
        self.send(Event::Lead(sandbox.into()));
        Ok(())
    }

    /// `POST /api/sandboxes/:name/lead/cancel`: a lead `waiting` or
    /// `activating` goes back to off. Nothing is typed; a `lead_ack` that
    /// comes later is refused.
    pub fn lead_cancel(&self, sandbox: &SandboxRef) -> Result<(), String> {
        if self.lead_pending.lock().unwrap().remove(sandbox).is_none() {
            return Err(format!("'{sandbox}' has no lead waiting or activating"));
        }
        self.forget_lead(sandbox);
        self.lead_failed.lock().unwrap().remove(sandbox);
        tracing::info!("plans: the person cancelled the lead of '{sandbox}' before it was confirmed");
        self.send(Event::LeadEnd(sandbox.into()));
        Ok(())
    }

    /// The relay's `lead_ack`, from `sandbox`'s token: its main session says
    /// it took the lead. Turns `activating` into `leading`; anything else is
    /// refused with a fixed text (the agent's words are never read). The
    /// answer, on success, is what the session is told: the open plans it
    /// follows again, when Team came back on over plans its end had closed.
    pub fn lead_ack(self: &Arc<Self>, sandbox: &SandboxRef, thread: Option<&str>) -> Result<String, String> {
        if thread.is_some_and(|t| !t.trim().is_empty()) {
            return Err(ACK_THREAD.into());
        }
        let phase = self.lead_pending_of(sandbox).map(|p| p.phase);
        match phase {
            Some(LeadPhase::Activating) => {}
            Some(_) => return Err(ACK_WAITING.into()),
            None if self.leading().contains(&sandbox.to_string()) => {
                return Ok("Confirmed: you already lead this sandbox.".into());
            }
            None => {
                tracing::info!("plans: '{sandbox}' called lead_ack with no lead requested: refused");
                return Err(ACK_REFUSED.into());
            }
        }
        // Still the terminal `/lead` was typed into?
        if !self.leads.lock().unwrap().contains_key(sandbox) {
            self.lead_fail(sandbox, "The agent's terminal closed before the session confirmed the lead.");
            return Err(ACK_REFUSED.into());
        }
        let now = now_ms();
        let rec = {
            let mut recs = self.lead_recs.lock().unwrap();
            let Some(r) = recs.get_mut(sandbox) else {
                drop(recs);
                self.lead_fail(sandbox, "The lead's record was lost before it was confirmed.");
                return Err(ACK_REFUSED.into());
            };
            r.since_ms = now;
            r.bound_ms = now;
            r.clone()
        };
        self.lead_pending.lock().unwrap().remove(sandbox);
        self.lead_failed.lock().unwrap().remove(sandbox);
        self.save_lead(sandbox, Some(rec));
        tracing::info!("plans: '{sandbox}' confirmed it leads");
        self.send(Event::Lead(sandbox.into()));
        // Plans its last end closed are this sandbox's lead's again.
        let back = self.plans.lock().unwrap().resume_lead(&sandbox.to_string(), now);
        let text = messages::lead_resumed(&back);
        if !back.is_empty() {
            tracing::info!("plans: '{sandbox}' leads again, {} open plan(s) given back", back.len());
        }
        for p in back {
            self.plan_changed(p);
        }
        Ok(match text {
            Some(t) => format!("Confirmed: you lead this sandbox.\n\n{t}"),
            None => "Confirmed: you lead this sandbox.".into(),
        })
    }

    /// Install `/lead` and refresh the relay server (a sandbox from before
    /// plans lacks its tools).
    pub(crate) async fn lead_install(&self, sandbox: &SandboxRef) -> Result<(), String> {
        use sbxw_core::tooling::Tooling;
        let token = self.store.token(sandbox).map_err(|e| format!("{e:#}"))?;
        let batch = Tooling::new()
            .agent_token(&token)
            .relay(&self.agent_port_for(sandbox).await.map_err(|e| format!("{e:#}"))?)
            .lead_command();
        let outcome = self.sbx_of(sandbox).await?.tooling(&sandbox.name, &batch).await.map_err(|e| format!("{e:#}"))?;
        if let Some((piece, e)) = outcome.failed.first() {
            return Err(format!("{piece}: {e}"));
        }
        Ok(())
    }

    /// Type `/lead` into the main terminal now, and wait for `lead_ack`
    /// ([`LEAD_ACK_MAX_MS`]). `from_waiting`: only if the lead still waits
    /// (a cancel in between wins).
    pub(crate) async fn lead_type(self: &Arc<Self>, sandbox: &SandboxRef, from_waiting: bool) -> Result<(), String> {
        let Some(s) = self.sessions.live(&SessionKey::agent(sandbox)) else {
            return Err(format!("'{sandbox}' has no agent terminal open: open it, then press Team"));
        };
        let now = now_ms();
        {
            let mut leads = self.leads.lock().unwrap();
            let mut recs = self.lead_recs.lock().unwrap();
            let mut pending = self.lead_pending.lock().unwrap();
            match pending.get(sandbox).map(|p| p.phase) {
                Some(LeadPhase::Waiting) if from_waiting => {}
                None if !from_waiting && !leads.contains_key(sandbox) => {}
                // Cancelled, already typed, or already leading.
                _ => return Ok(()),
            }
            // The conversation the person turned the team on in. Unknown when
            // the agent has not reported yet: filled by the first `leading()`
            // while this very terminal is still the main one.
            let session_id = self.main_session_id(sandbox);
            leads.insert(sandbox.clone(), Arc::as_ptr(&s) as usize);
            recs.insert(sandbox.clone(), LeadRecord { since_ms: now, session_id, bound_ms: now });
            pending.insert(sandbox.clone(), Pending { phase: LeadPhase::Activating, since_ms: now, until_ms: now + LEAD_ACK_MAX_MS });
        }
        self.lead_failed.lock().unwrap().remove(sandbox);
        if let Err(e) = self.push_text(sandbox, SessionKind::Agent, messages::LEAD_COMMAND).await {
            self.lead_fail(sandbox, &format!("sbxw could not type /lead: {e}"));
            return Err(e);
        }
        tracing::info!("plans: /lead typed into '{sandbox}', waiting for its lead_ack");
        self.send(Event::Lead(sandbox.into()));
        Ok(())
    }

    /// A lead not confirmed, or lost, falls back to off with `reason`, shown
    /// in `/api/lead-states` for [`LEAD_REASON_KEEP_MS`].
    pub(crate) fn lead_fail(&self, sandbox: &SandboxRef, reason: &str) {
        self.lead_pending.lock().unwrap().remove(sandbox);
        self.forget_lead(sandbox);
        self.lead_dropped(sandbox, reason);
    }

    /// Bookkeeping of a fall back to off: the reason kept, `LeadEnd` sent.
    pub(crate) fn lead_dropped(&self, sandbox: &SandboxRef, reason: &str) {
        tracing::info!("plans: lead mode of '{sandbox}' is off: {reason}");
        self.lead_failed.lock().unwrap().insert(sandbox.clone(), (reason.to_string(), now_ms()));
        self.send(Event::LeadEnd(sandbox.into()));
    }

    /// Every [`TICK`]: a waiting lead is typed once its agent is idle, or
    /// falls back to off past [`LEAD_WAIT_MAX_MS`]; an activating one falls
    /// back past its deadline; and [`App::leading`] runs, which sends
    /// `LeadEnd` for every lead it drops.
    pub async fn lead_tick(self: &Arc<Self>) {
        let now = now_ms();
        let pending: Vec<(SandboxRef, Pending)> = self.lead_pending.lock().unwrap().iter().map(|(s, p)| (s.clone(), *p)).collect();
        for (sb, p) in pending {
            match p.phase {
                LeadPhase::Waiting if now >= p.until_ms => self.lead_fail(
                    &sb,
                    "The agent was not idle within 10 minutes, so /lead was not typed. Press Team again when it is idle.",
                ),
                LeadPhase::Waiting if !self.sessions.is_live(&SessionKey::agent(&sb)) => {
                    self.lead_fail(&sb, "The agent's terminal closed before it was idle.")
                }
                LeadPhase::Waiting => {
                    if self.remote_can_type(&sb).is_ok() {
                        // On a failure, `lead_type` has fallen back to off.
                        let _ = self.lead_type(&sb, true).await;
                    }
                }
                LeadPhase::Activating if now >= p.until_ms => self.lead_fail(
                    &sb,
                    "The session did not confirm it took the lead within 90 seconds. Press Team again.",
                ),
                _ => {}
            }
        }
        self.leading();
    }

    /// A new conversation in the lead's own terminal (`/clear`, a
    /// compaction, `/resume`): the record follows it, back to `activating`
    /// until it calls `lead_ack` again. Only ever keeps a lead that exists
    /// (`leading` or `activating`, bound to this very terminal); never
    /// creates one. `true` when the lead was moved: the session must be
    /// given its instructions again (see `svc::session_context`).
    pub(crate) fn lead_follow_conversation(&self, sandbox: &SandboxRef, session_id: &str, compacted: bool) -> bool {
        let Some(s) = self.sessions.live(&SessionKey::agent(sandbox)) else { return false };
        // The session the terminal shows must be this one: a session started
        // over SSH, or a thread's, is not the lead's.
        if self.main_session_id(sandbox).as_deref() != Some(session_id) {
            return false;
        }
        let now = now_ms();
        {
            let leads = self.leads.lock().unwrap();
            let mut recs = self.lead_recs.lock().unwrap();
            let mut pending = self.lead_pending.lock().unwrap();
            if leads.get(sandbox) != Some(&(Arc::as_ptr(&s) as usize)) {
                return false;
            }
            let Some(rec) = recs.get_mut(sandbox) else { return false };
            if matches!(pending.get(sandbox).map(|p| p.phase), Some(LeadPhase::Waiting)) {
                return false;
            }
            let moved = rec.session_id.as_deref() != Some(session_id);
            if !moved && !compacted {
                return false;
            }
            rec.session_id = Some(session_id.to_string());
            rec.bound_ms = now;
            pending.insert(sandbox.clone(), Pending { phase: LeadPhase::Activating, since_ms: now, until_ms: now + LEAD_ACK_MAX_MS });
        }
        tracing::info!("plans: the lead of '{sandbox}' started a new conversation: its instructions are given again, waiting for its lead_ack");
        self.send(Event::Lead(sandbox.into()));
        true
    }
}

#[cfg(test)]
#[path = "lead_tests.rs"]
mod tests;
