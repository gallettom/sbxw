//! Remote access, level 2: approving from the phone with a passkey
//! (`.sbxw-artifacts/remote/API-level2.md`, `docs/security.md`).
//!
//! An authorisation in front of the core functions the desktop and the web
//! UI call, never a second way of doing the thing: `plan_approve` with the
//! plan's own threads, `plan_refuse`, `plan_merge`, [`App::approval_answer`]
//! (shared with the UI route) and `remove`. The rules are
//! `sbxw_core::remote_access::{level2, webauthn}`; the routes are
//! `http::remote_approvals` (the phone) and `http::remote_approvals_admin`
//! (the Mac).
//!
//! Never logged, never journaled: a goal, a summary, a note, a diff, a path,
//! an option label. Info lines say `device <id>: <kind>` and nothing else.

use crate::app::{now_ms, App};
use crate::git;
use crate::svc::remote_access::{write_private, DeviceCtx};
use sbxw_core::approvals::Approval;
use sbxw_core::plans::{MergeState, Plan, PlanState, Variant};
use sbxw_core::remote_access::journal::Kind;
use sbxw_core::remote_access::level2::{self as l2, Action, Challenges, Enrolment, PasskeyFile, Purpose};
use sbxw_core::remote_access::limits::Limiter;
use sbxw_core::remote_access::webauthn::{self as wa, b64url_decode, b64url_encode, Fail};
use sbxw_core::{SandboxName, SandboxRef};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Files listed per branch of a merge, at most.
pub const MERGE_FILES_MAX: usize = 200;

/// Level 2's state, held in the remote runtime (one lock).
pub struct Level2 {
    /// On: stored in `state.json` as `approvals`.
    pub enabled: bool,
    pub passkeys: PasskeyFile,
    pub enrol: Enrolment,
    pub challenges: Challenges,
    /// Failed assertions, per device id: 5 a minute, 20 an hour.
    fail_minute: Limiter,
    fail_hour: Limiter,
    /// `passkey/start`, per device id: 5 per 10 minutes.
    start_limit: Limiter,
}

fn passkeys_path(dir: &Path) -> PathBuf {
    dir.join("passkeys.json")
}

impl Level2 {
    /// `passkeys.json` beside `state.json`. Unreadable: no passkey (the
    /// person grants again), never a guess.
    pub fn load(dir: &Path, enabled: bool) -> Self {
        let passkeys = match std::fs::read(passkeys_path(dir)) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_else(|e| {
                tracing::warn!("remote access: passkeys.json unreadable ({e}); no passkey is kept");
                PasskeyFile::default()
            }),
            Err(_) => PasskeyFile::default(),
        };
        Level2 {
            enabled,
            passkeys,
            enrol: Enrolment::default(),
            challenges: Challenges::default(),
            fail_minute: Limiter::new(60_000, 5, u32::MAX),
            fail_hour: Limiter::new(3_600_000, 20, u32::MAX),
            start_limit: Limiter::new(600_000, 5, u32::MAX),
        }
    }

    pub fn save_passkeys(&self, dir: &Path) {
        let bytes = serde_json::to_vec_pretty(&self.passkeys).expect("serialisable");
        if let Err(e) = write_private(dir, &passkeys_path(dir), &bytes) {
            tracing::warn!("remote access: cannot write passkeys.json: {e}");
        }
    }

    /// A device went (revoked, logged out, idle): its passkey, request,
    /// grant and challenge go with it. True when a passkey went.
    pub fn forget_device(&mut self, id: &str) -> bool {
        self.enrol.drop_device(id);
        self.challenges.drop_device(id);
        self.passkeys.drop_device(id)
    }

    /// Level 2 off: every request, grant and challenge goes; passkeys stay.
    pub fn turn_off(&mut self) {
        self.enabled = false;
        self.enrol.clear();
        self.challenges.clear();
    }
}

/// A refusal of a level 2 route: a status, a fixed `reason` the page
/// switches on, and a plain sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L2Error {
    pub status: u16,
    pub reason: &'static str,
    pub error: String,
    pub retry_ms: Option<u64>,
}

impl L2Error {
    fn new(status: u16, reason: &'static str, error: impl Into<String>) -> Self {
        L2Error { status, reason, error: error.into(), retry_ms: None }
    }
    pub fn off() -> Self {
        Self::new(403, "level2_off", "Approving from the phone is off. Turn it on at the Mac: sbxw remote approvals on")
    }
    fn no_passkey(why: &str) -> Self {
        Self::new(403, "no_passkey", why)
    }
    fn passkey() -> Self {
        Self::new(403, "passkey", "The Face ID check failed. Nothing was done.")
    }
    fn changed() -> Self {
        Self::new(409, "changed", "This changed since it was shown. Nothing was done: look at it again.")
    }
    fn refused(why: String) -> Self {
        Self::new(409, "refused", why)
    }
    pub fn not_found() -> Self {
        Self::new(404, "not_found", "not found")
    }
    fn bad(why: impl Into<String>) -> Self {
        Self::new(400, "bad", why)
    }
    fn limited(ms: u64) -> Self {
        L2Error { retry_ms: Some(ms), ..Self::new(429, "limited", "Too many failed Face ID checks. Try again later.") }
    }
}

fn no_grant() -> L2Error {
    L2Error::new(403, "no_grant", "Allow this phone at the Mac first: sbxw remote approvals allow <device>")
}

const NEEDS_FACE_ID: &str = "Approving needs Face ID: enable it from your Mac.";
const NAME_CHANGED: &str = "The tailnet name changed: enable Face ID again from your Mac.";

/// A WebAuthn assertion as the page sends it (base64url fields).
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Assertion {
    pub id: String,
    pub client_data_json: String,
    pub authenticator_data: String,
    pub signature: String,
    /// Sent by some pages; never read.
    #[serde(default)]
    pub user_handle: Option<String>,
}

/// An item as the phone is shown it, and the digest of what an approval
/// of it covers.
pub struct Shown {
    pub view: Value,
    pub digest: String,
    /// Only for a target the phone may act on now (a merge whose tips are
    /// all read, a network request with a phone option...).
    pub actionable: bool,
    /// The network request's phone option ids.
    pub options: Vec<String>,
    /// What the journal names: a plan id, a sandbox name, a host.
    pub journal_target: String,
    /// A merge: the commit ids the digest covers, which the merge must take.
    pub pins: Option<crate::svc::plans::MergePins>,
}

fn journal_kind(a: Action) -> Kind {
    match a {
        Action::PlanApprove => Kind::PlanApprove,
        Action::PlanRefuse => Kind::PlanRefuse,
        Action::Merge => Kind::Merge,
        Action::NetworkRespond => Kind::NetworkRespond,
        Action::SandboxRemove => Kind::SandboxRemove,
    }
}

/// Why sbx's answer to a request was not passed on.
#[derive(Debug)]
pub enum AnswerError {
    /// No longer listed: answered elsewhere, or lapsed.
    Gone,
    /// Not one of the request's options.
    NotOffered(String),
    Sbx(anyhow::Error),
}

impl App {
    // ── The answer to an sbx request, shared with the UI route ───────────

    /// Answer a pending sbx request with one of ITS options, refusing a
    /// request sbx no longer lists (a port request lapses after 45 s). The
    /// one path the web UI's `/api/approvals/respond` and the phone take.
    pub async fn approval_answer(&self, id: &str, option: &str) -> Result<Approval, AnswerError> {
        let pending = self.sbx.approvals().await.map_err(AnswerError::Sbx)?;
        let a = pending.into_iter().find(|a| a.id == id).ok_or(AnswerError::Gone)?;
        if !a.offers(option) {
            return Err(AnswerError::NotOffered(option.to_string()));
        }
        self.sbx.approval_respond(&a.id, option).await.map_err(AnswerError::Sbx)?;
        Ok(a)
    }

    // ── The Mac's side ───────────────────────────────────────────────────

    pub fn remote_approvals_status(&self) -> Value {
        let mut rt = self.remote_access.lock().unwrap();
        let now = now_ms();
        let enabled = rt.level2.enabled;
        let devices: Vec<(String, String)> = rt.devices.devices().iter().map(|d| (d.id.clone(), d.name.clone())).collect();
        let list: Vec<Value> = devices
            .into_iter()
            .map(|(id, name)| {
                let pk = rt.level2.passkeys.get(&id).map(|p| json!({ "created_ms": p.created_ms, "rp_id": p.rp_id }));
                let request = rt.level2.enrol.pending(&id, now);
                let granted = rt.level2.enrol.granted_until(&id, now);
                json!({ "id": id, "name": name, "passkey": pk, "request": request, "granted_until_ms": granted })
            })
            .collect();
        json!({ "enabled": enabled, "devices": list })
    }

    /// Turn level 2 on: only with remote access on. Only the admin routes
    /// (UI token) reach this.
    pub fn remote_approvals_enable(&self) -> Result<(), String> {
        let mut rt = self.remote_access.lock().unwrap();
        if !rt.enabled {
            return Err("remote access is off: turn it on first (sbxw remote on)".into());
        }
        if !rt.level2.enabled {
            rt.level2.enabled = true;
            rt.save();
            tracing::info!("remote access: approving from the phone turned on");
        }
        Ok(())
    }

    pub fn remote_approvals_disable(&self) {
        let mut rt = self.remote_access.lock().unwrap();
        let was = rt.level2.enabled;
        rt.level2.turn_off();
        if was {
            rt.save();
            tracing::info!("remote access: approving from the phone turned off");
        }
    }

    /// The person types the digits the phone shows: a grant for that device.
    pub fn remote_passkey_allow(&self, device_id: &str, check: &str) -> Result<u64, l2::AllowError> {
        let mut rt = self.remote_access.lock().unwrap();
        if rt.devices.get(device_id).is_none() {
            return Err(l2::AllowError::Unknown);
        }
        let r = rt.level2.enrol.allow(device_id, check, now_ms());
        match &r {
            Ok(_) => tracing::info!("remote access: device {device_id}: passkey registration granted on the Mac"),
            Err(l2::AllowError::Refused) => {
                let name = rt.devices.get(device_id).map(|d| d.name.clone()).unwrap_or_default();
                rt.record_target(device_id, &name, Kind::Refused, None);
                tracing::info!("remote access: device {device_id}: passkey request refused after wrong digits");
            }
            Err(_) => {}
        }
        r
    }

    pub fn remote_passkey_refuse(&self, device_id: &str) -> bool {
        let mut rt = self.remote_access.lock().unwrap();
        let done = rt.level2.enrol.refuse(device_id, now_ms());
        if done {
            let name = rt.devices.get(device_id).map(|d| d.name.clone()).unwrap_or_default();
            rt.record_target(device_id, &name, Kind::Refused, None);
        }
        done
    }

    /// Drop a device's passkey (and its request, grant and challenge).
    pub fn remote_passkey_revoke(&self, device_id: &str) -> bool {
        let mut rt = self.remote_access.lock().unwrap();
        let gone = rt.level2.forget_device(device_id);
        if gone {
            let dir = rt.dir.clone();
            rt.level2.save_passkeys(&dir);
            tracing::info!("remote access: device {device_id}: passkey revoked on the Mac");
        }
        gone
    }

    // ── The phone's side: registration ───────────────────────────────────

    pub(crate) fn l2_on(&self) -> Result<(), L2Error> {
        if self.remote_access.lock().unwrap().level2.enabled {
            Ok(())
        } else {
            Err(L2Error::off())
        }
    }

    pub fn remote_passkey_start(&self, d: &DeviceCtx) -> Result<(String, String, u64), L2Error> {
        let mut rt = self.remote_access.lock().unwrap();
        if !rt.level2.enabled {
            return Err(L2Error::off());
        }
        let now = now_ms();
        rt.level2.start_limit.hit(&d.id, now).map_err(|ms| L2Error { retry_ms: Some(ms), ..L2Error::new(429, "limited", "Too many requests. Try again later.") })?;
        let r = rt.level2.enrol.start(&d.id, now);
        tracing::info!("remote access: device {}: passkey requested", d.id);
        Ok(r)
    }

    /// The state of the request the caller started (`request_id`, which
    /// only that client was given): "none" for any other id, so a second
    /// holder of the cookie learns nothing about the person's request.
    pub fn remote_passkey_status(&self, d: &DeviceCtx, request_id: &str) -> Result<Value, L2Error> {
        let mut rt = self.remote_access.lock().unwrap();
        if !rt.level2.enabled {
            return Err(L2Error::off());
        }
        let (st, exp) = rt.level2.enrol.status(&d.id, request_id, now_ms());
        let state = match st {
            l2::EnrolStatus::None => "none",
            l2::EnrolStatus::Pending => "pending",
            l2::EnrolStatus::Granted => "granted",
            l2::EnrolStatus::Refused => "refused",
        };
        Ok(json!({ "state": state, "expires_ms": exp }))
    }

    /// Creation options, with a fresh 2-minute challenge bound to the
    /// request: only with a live grant the Mac made for THAT request
    /// (review P1: a grant is not the device's, it is the request's).
    pub fn remote_passkey_options(&self, d: &DeviceCtx, request_id: &str) -> Result<Value, L2Error> {
        let mut rt = self.remote_access.lock().unwrap();
        if !rt.level2.enabled {
            return Err(L2Error::off());
        }
        let rp = rt.serving_name().ok_or_else(|| L2Error::no_passkey("Face ID needs the tailnet address of sbxw."))?;
        let now = now_ms();
        if rt.level2.enrol.granted(&d.id, request_id, now).is_none() {
            return Err(no_grant());
        }
        let c = rt.level2.challenges.issue(&d.id, Purpose::Register { request: request_id.to_string() }, now);
        Ok(json!({
            "public_key": {
                "rp": { "id": rp, "name": "sbxw" },
                "user": { "id": b64url_encode(d.id.as_bytes()), "name": d.name, "displayName": d.name },
                "challenge": b64url_encode(&c.bytes),
                "pubKeyCredParams": [{ "type": "public-key", "alg": wa::COSE_ALG_ES256 }],
                "authenticatorSelection": { "authenticatorAttachment": "platform", "residentKey": "preferred", "userVerification": "required" },
                "attestation": "none",
                "timeout": l2::CREATE_TTL_MS,
                "excludeCredentials": [],
            },
            "expires_ms": c.expires_ms,
        }))
    }

    /// Verify a registration for request `request_id`. A call that does not
    /// present the request's id is `no_grant` and spends nothing (it cannot
    /// burn the person's grant). With the id, the grant and the creation
    /// challenge are spent first, whatever the outcome.
    pub fn remote_passkey_register(&self, d: &DeviceCtx, request_id: &str, raw_id: &str, client_data: &str, attestation: &str) -> Result<(), L2Error> {
        let mut rt = self.remote_access.lock().unwrap();
        if !rt.level2.enabled {
            return Err(L2Error::off());
        }
        let now = now_ms();
        if rt.level2.enrol.granted(&d.id, request_id, now).is_none() {
            return Err(no_grant());
        }
        let granted = rt.level2.enrol.take_grant(&d.id, request_id, now);
        let challenge = rt.level2.challenges.take_register(&d.id, request_id, now);
        let rp = rt.serving_name();
        let verdict = (|| {
            if !granted {
                return Err(Fail::Challenge);
            }
            let c = challenge.ok_or(Fail::Challenge)?;
            let rp = rp.as_deref().ok_or(Fail::RpIdHash)?;
            let raw_id = b64url_decode(raw_id, wa::MAX_CREDENTIAL_ID)?;
            let cd = b64url_decode(client_data, wa::MAX_CLIENT_DATA)?;
            let att = b64url_decode(attestation, wa::MAX_ATTESTATION)?;
            wa::verify_registration(rp, &c.bytes, &raw_id, &cd, &att).map(|n| (n, rp.to_string()))
        })();
        match verdict {
            Ok((n, rp)) => {
                rt.level2.passkeys.put(l2::Passkey {
                    device_id: d.id.clone(),
                    credential_id: b64url_encode(&n.id),
                    public_key: b64url_encode(&n.public_key),
                    counter: n.counter,
                    rp_id: rp,
                    created_ms: now,
                });
                let dir = rt.dir.clone();
                rt.level2.save_passkeys(&dir);
                rt.record_target(&d.id, &d.name, Kind::PasskeyRegistered, None);
                tracing::info!("remote access: device {}: passkey_registered", d.id);
                Ok(())
            }
            Err(f) => {
                rt.record_target(&d.id, &d.name, Kind::PasskeyFailed, None);
                tracing::info!("remote access: device {}: passkey_failed", d.id);
                tracing::debug!("remote access: registration refused at {f:?}");
                Err(if granted { L2Error::passkey() } else { no_grant() })
            }
        }
    }

    // ── The phone's side: what waits ─────────────────────────────────────

    /// `GET /r/approvals`: whatever level 2's state.
    pub async fn remote_approvals_overview(self: &Arc<Self>, d: &DeviceCtx) -> Value {
        let (on, passkey, rp_ok) = {
            let mut rt = self.remote_access.lock().unwrap();
            let rp = rt.serving_name();
            let pk = rt.level2.passkeys.get(&d.id).cloned();
            (rt.level2.enabled, pk.is_some(), pk.is_some_and(|p| Some(p.rp_id) == rp))
        };
        let counts = if on { Some(self.remote_approval_counts().await) } else { None };
        json!({ "level2": on, "passkey": passkey, "rp_ok": rp_ok, "counts": counts })
    }

    /// How many items wait, per kind: numbers only (the events' badge).
    pub async fn remote_approval_counts(self: &Arc<Self>) -> Value {
        let plans = self.l2_pending_plans().len();
        let merges = self.l2_proposed_merges().len();
        let network = self.sbx.approvals().await.map(|l| l.iter().filter(|a| a.kind == "network").count()).unwrap_or(0);
        let stopped = self.l2_stopped().await.len();
        json!({ "plans": plans, "merges": merges, "network": network, "stopped": stopped })
    }

    fn l2_pending_plans(&self) -> Vec<Plan> {
        self.plans_list().into_iter().filter(|p| p.state == PlanState::Pending && !p.closed).collect()
    }

    fn l2_proposed_merges(&self) -> Vec<Plan> {
        self.plans_list().into_iter().filter(|p| p.merge.as_ref().is_some_and(|m| m.state == MergeState::Proposed)).collect()
    }

    fn mask(&self) -> impl Fn(&str) -> String {
        let rules = self.anonymise_rules().unwrap_or_default();
        move |t: &str| sbxw_core::anonymise::mask(t, &rules).text
    }

    pub fn remote_plans(&self) -> Result<Value, L2Error> {
        self.l2_on()?;
        let m = self.mask();
        let list: Vec<Value> = self.l2_pending_plans().iter().map(|p| json!({ "id": p.id, "sandbox": p.sandbox, "summary": m(&p.summary), "threads": p.threads.len(), "created_ms": p.created_ms })).collect();
        Ok(json!({ "plans": list }))
    }

    pub fn remote_merges(&self) -> Result<Value, L2Error> {
        self.l2_on()?;
        let list: Vec<Value> = self
            .l2_proposed_merges()
            .iter()
            .map(|p| {
                let n = if p.variant == Variant::Shared { 1 } else { p.merge.as_ref().map(|m| m.order.len()).unwrap_or(0) };
                json!({ "id": p.id, "sandbox": p.sandbox, "variant": p.variant, "branches": n })
            })
            .collect();
        Ok(json!({ "merges": list }))
    }

    pub async fn remote_network(&self) -> Result<Value, L2Error> {
        self.l2_on()?;
        let list = self.sbx.approvals().await.map_err(|e| L2Error::new(503, "unavailable", format!("sbx could not list its requests: {e:#}")))?;
        let m = self.mask();
        let out: Vec<Value> = list.iter().filter(|a| a.kind == "network").map(|a| network_view(a, &m, false)).collect();
        Ok(json!({ "requests": out }))
    }

    pub async fn remote_stopped(&self) -> Result<Value, L2Error> {
        self.l2_on()?;
        let out: Vec<Value> = self.l2_stopped().await.iter().map(|(s, ws, chat)| json!({ "name": s.name, "agent": s.agent, "chat": chat, "workspace": ws, "created_at": s.created_at })).collect();
        Ok(json!({ "sandboxes": out }))
    }

    /// This Mac's stopped sandboxes a phone may remove: stopped, with a
    /// creation time, a name sbxw takes, not the Jarvis sandbox.
    async fn l2_stopped(&self) -> Vec<(sbxw_sbx::SandboxInfo, Option<String>, bool)> {
        self.sbx
            .ls()
            .await
            .into_iter()
            .filter(|s| s.is_stopped() && s.created_at.is_some())
            .filter_map(|s| {
                let n = SandboxName::parse(&s.name).ok()?;
                let r = SandboxRef::from(n.clone());
                if crate::svc::jarvis_sandbox::is_jarvis_name(&r) {
                    return None;
                }
                let rec = self.store.get(&n);
                let chat = self.store.chat_workspace(&n).is_some();
                Some((s, rec.workspace.map(|w| w.to_string_lossy().into_owned()), chat))
            })
            .collect()
    }

    /// One item, shown, with its digest: `None` when there is no such item
    /// a phone may see (unknown, decided, answered, running...).
    pub async fn remote_item(self: &Arc<Self>, action: Action, target: &str) -> Result<Option<Shown>, L2Error> {
        Ok(match action {
            Action::PlanApprove | Action::PlanRefuse => {
                if !l2::valid_plan_id(target) {
                    return Ok(None);
                }
                self.l2_pending_plans().into_iter().find(|p| p.id == target).map(|p| self.plan_shown(&p))
            }
            Action::Merge => {
                if !l2::valid_plan_id(target) {
                    return Ok(None);
                }
                match self.l2_proposed_merges().into_iter().find(|p| p.id == target) {
                    Some(p) => Some(self.merge_shown(&p).await),
                    None => None,
                }
            }
            Action::NetworkRespond => {
                if !l2::valid_network_id(target) {
                    return Ok(None);
                }
                let list = self.sbx.approvals().await.map_err(|e| L2Error::new(503, "unavailable", format!("sbx could not list its requests: {e:#}")))?;
                let m = self.mask();
                list.iter().find(|a| a.id == target && a.kind == "network").map(|a| {
                    let options: Vec<String> = a.options.iter().filter(|o| l2::phone_option(&o.id, &o.label)).map(|o| o.id.clone()).collect();
                    let digest = l2::digest(&json!({ "id": a.id, "sandbox": a.sandbox, "host": a.title, "options": options }));
                    Shown { view: network_view(a, &m, true), digest, actionable: !options.is_empty(), options, journal_target: a.title.clone(), pins: None }
                })
            }
            Action::SandboxRemove => {
                let Ok(n) = SandboxName::parse(target) else { return Ok(None) };
                self.l2_stopped().await.into_iter().find(|(s, _, _)| s.name == n.as_str()).map(|(s, ws, chat)| {
                    let digest = l2::digest(&json!({ "name": s.name, "state": "stopped", "created_at": s.created_at }));
                    let mut confirm = vec![
                        format!("Remove {}?", s.name),
                        format!("Sandbox: {} ({}).", s.name, if chat { "a chat" } else if s.agent.is_empty() { "agent" } else { s.agent.as_str() }),
                        "This is irreversible: everything inside the sandbox is lost (installed packages, files outside its workspace, its thread records). Its terminals close.".to_string(),
                    ];
                    if let Some(w) = ws.as_deref().filter(|w| !w.is_empty()) {
                        confirm.push(format!("Its workspace folder stays on this machine: {w}"));
                    }
                    let view = json!({ "sandbox": { "name": s.name, "agent": s.agent, "chat": chat, "workspace": ws, "created_at": s.created_at }, "confirm": confirm });
                    Shown { view, digest, actionable: true, options: vec![], journal_target: s.name.clone(), pins: None }
                })
            }
        })
    }

    fn plan_shown(&self, p: &Plan) -> Shown {
        let m = self.mask();
        let threads: Vec<Value> = p.threads.iter().map(|t| json!({ "name": t.name, "goal": t.goal, "scope": t.scope, "repo": t.repo, "model": t.model, "ticket": t.ticket })).collect();
        let digest = l2::digest(&json!({ "id": p.id, "sandbox": p.sandbox, "summary": p.summary, "justification": p.justification, "variant": p.variant, "threads": threads }));
        let shown: Vec<Value> = p
            .threads
            .iter()
            .map(|t| json!({ "name": t.name, "model": t.model, "repo": t.repo, "scope": m(&t.scope), "goal": m(&t.goal), "ticket": t.ticket.map(|n| format!("T-{n}")) }))
            .collect();
        let view = json!({ "plan": { "id": p.id, "sandbox": p.sandbox, "summary": m(&p.summary), "justification": m(&p.justification), "variant": p.variant, "main_uncommitted": p.main_uncommitted, "threads": shown } });
        Shown { view, digest, actionable: true, options: vec![], journal_target: p.id.clone(), pins: None }
    }

    async fn merge_shown(&self, p: &Plan) -> Shown {
        let m = self.mask();
        let merge = p.merge.clone().expect("a proposed merge");
        let branches = self.merge_branches(p).await;
        let tips: Vec<Value> = branches.iter().map(|b| json!({ "thread": b.thread, "tip": b.tip, "base_tip": b.base_tip })).collect();
        let distillate: Vec<Value> = merge.distillate.iter().map(|i| json!({ "kind": i.kind, "text": i.text })).collect();
        let digest = l2::digest(&json!({ "id": p.id, "sandbox": p.sandbox, "order": merge.order, "note": merge.note, "distillate": distillate, "branches": tips }));
        let actionable = !branches.is_empty() && branches.iter().all(|b| b.tip.is_some() && b.base_tip.is_some());
        let short = |s: &Option<String>| s.as_deref().map(|s| s.chars().take(7).collect::<String>());
        let shown: Vec<Value> = branches
            .iter()
            .map(|b| {
                let known_conflict = merge.results.iter().any(|r| r.thread == b.thread && !r.ok);
                json!({
                    "thread": b.thread, "branch": format!("sbxw/{}", b.thread), "repo": b.repo,
                    "tip": short(&b.tip), "base": { "branch": b.base_branch, "tip": short(&b.base_tip) },
                    "ahead": b.ahead, "files_changed": b.files.len() + b.more_files,
                    "insertions": b.files.iter().filter_map(|f| f.added).sum::<u64>(),
                    "deletions": b.files.iter().filter_map(|f| f.removed).sum::<u64>(),
                    "files": b.files.iter().map(|f| json!({ "path": f.path, "insertions": f.added, "deletions": f.removed })).collect::<Vec<_>>(),
                    "files_truncated": b.more_files > 0,
                    "known_conflict": known_conflict,
                })
            })
            .collect();
        let dist_shown: Vec<Value> = merge.distillate.iter().map(|i| json!({ "kind": i.kind, "text": m(&i.text) })).collect();
        let view = json!({ "merge": { "id": p.id, "sandbox": p.sandbox, "variant": p.variant, "order": merge.order, "note": m(&merge.note), "distillate": dist_shown, "branches": shown } });
        let pins = actionable.then(|| branches.iter().filter_map(|b| Some((b.thread.clone(), (b.tip.clone()?, b.base_tip.clone()?)))).collect());
        Shown { view, digest, actionable, options: vec![], journal_target: p.id.clone(), pins }
    }

    /// Each branch a merge would take, read on the host (git, read only):
    /// its tip, its base's tip and branch, commits ahead, files changed.
    async fn merge_branches(&self, p: &Plan) -> Vec<BranchFacts> {
        let Ok(sb) = SandboxRef::parse(&p.sandbox) else { return vec![] };
        let Some(merge) = &p.merge else { return vec![] };
        let rec = self.store.get(&sb);
        let repo_of = |t: &str| {
            rec.threads
                .get(t)
                .map(|r| r.repo.clone())
                .or_else(|| p.threads.iter().find(|x| x.name == t).and_then(|x| x.repo.clone()))
                .unwrap_or_default()
        };
        let names: Vec<(String, String)> = if p.variant == Variant::Shared {
            vec![(p.team_name(), p.threads.first().map(|t| repo_of(&t.name)).unwrap_or_default())]
        } else {
            merge.order.iter().map(|t| (t.clone(), repo_of(t))).collect()
        };
        let mut out = Vec::new();
        let (Some(ws), Ok(host)) = (rec.workspace.clone(), self.host_for(&sb).await) else {
            return names.into_iter().map(|(thread, repo)| BranchFacts { thread, repo, ..Default::default() }).collect();
        };
        for (thread, repo) in names {
            let mut b = BranchFacts { thread: thread.clone(), repo: repo.clone(), ..Default::default() };
            if sbxw_core::ThreadName::parse(&thread).is_err() {
                out.push(b);
                continue;
            }
            let Some(root) = git::repo_root(&host, &ws, &repo).await else {
                out.push(b);
                continue;
            };
            let line = |o: Option<String>| o.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            b.base_tip = line(git::git(&host, &root, &["rev-parse", "--verify", "--quiet", "HEAD"]).await);
            b.base_branch = git::branch_of(&host, &root).await;
            let full = format!("refs/heads/sbxw/{thread}");
            b.tip = line(git::git(&host, &root, &["rev-parse", "--verify", "--quiet", &full]).await);
            if let (Some(tip), Some(base)) = (&b.tip, &b.base_tip) {
                let range = format!("{base}..{tip}");
                b.ahead = line(git::git(&host, &root, &["rev-list", "--count", &range]).await).and_then(|n| n.parse().ok());
                let three = format!("{base}...{tip}");
                let files = git::git(&host, &root, &["diff", "--numstat", "--no-renames", "--no-color", "--no-ext-diff", &three]).await.map(|r| sbxw_core::commits::parse_numstat(&r)).unwrap_or_default();
                b.more_files = files.len().saturating_sub(MERGE_FILES_MAX);
                b.files = files.into_iter().take(MERGE_FILES_MAX).collect();
            }
            out.push(b);
        }
        out
    }

    // ── The phone's side: the challenge and the action ───────────────────

    /// A challenge bound to {device, credential, action, target, digest,
    /// option}: only when the digest the phone was shown is still current.
    pub async fn remote_challenge(self: &Arc<Self>, d: &DeviceCtx, action: Action, target: &str, digest: &str, option: Option<&str>) -> Result<Value, L2Error> {
        if !l2::valid_digest(digest) {
            return Err(L2Error::bad("digest: 64 lower-case hex, as the detail gave it"));
        }
        if (action == Action::NetworkRespond) != option.is_some() {
            return Err(L2Error::bad("option: given for a network request, and only for one"));
        }
        let (rp, cred) = self.l2_credential(d)?;
        let shown = self.remote_item(action, target).await?;
        let Some(shown) = shown.filter(|s| s.digest == digest && s.actionable) else {
            return Err(L2Error::changed());
        };
        if let Some(o) = option {
            if !shown.options.iter().any(|x| x == o) {
                return Err(L2Error::bad("this option is not one the phone may answer"));
            }
        }
        let mut rt = self.remote_access.lock().unwrap();
        if !rt.level2.enabled {
            return Err(L2Error::off());
        }
        let c = rt.level2.challenges.issue(&d.id, Purpose::Act { action, target: target.into(), digest: digest.into(), option: option.map(str::to_string) }, now_ms());
        Ok(json!({
            "public_key": {
                "challenge": b64url_encode(&c.bytes),
                "rpId": rp,
                "allowCredentials": [{ "type": "public-key", "id": cred }],
                "userVerification": "required",
                "timeout": l2::ACT_TTL_MS,
            },
            "expires_ms": c.expires_ms,
        }))
    }

    /// Level 2 on, and this device's passkey made for the current RP ID:
    /// `(rp id, credential id)`.
    fn l2_credential(&self, d: &DeviceCtx) -> Result<(String, String), L2Error> {
        let mut rt = self.remote_access.lock().unwrap();
        if !rt.level2.enabled {
            return Err(L2Error::off());
        }
        let pk = rt.level2.passkeys.get(&d.id).cloned().ok_or_else(|| L2Error::no_passkey(NEEDS_FACE_ID))?;
        let rp = rt.serving_name().ok_or_else(|| L2Error::no_passkey(NAME_CHANGED))?;
        if pk.rp_id != rp {
            return Err(L2Error::no_passkey(NAME_CHANGED));
        }
        Ok((rp, pk.credential_id))
    }

    /// Verify `a` against the device's taken challenge: the digest it was
    /// bound to. Every failure counts against this device only; a counter
    /// regression drops its passkey.
    fn l2_verify(&self, d: &DeviceCtx, action: Action, target: &str, option: Option<&str>, a: &Assertion) -> Result<String, L2Error> {
        let mut rt = self.remote_access.lock().unwrap();
        if !rt.level2.enabled {
            return Err(L2Error::off());
        }
        let now = now_ms();
        let pk = rt.level2.passkeys.get(&d.id).cloned().ok_or_else(|| L2Error::no_passkey(NEEDS_FACE_ID))?;
        let rp = rt.serving_name().filter(|rp| *rp == pk.rp_id).ok_or_else(|| L2Error::no_passkey(NAME_CHANGED))?;
        rt.level2.fail_minute.check(&d.id, now).map_err(L2Error::limited)?;
        rt.level2.fail_hour.check(&d.id, now).map_err(L2Error::limited)?;
        let taken = rt.level2.challenges.take(&d.id, now);
        let verdict = (|| {
            let c = taken.ok_or(Fail::Challenge)?;
            if !l2::challenge_fits(&c, action, target, option) {
                return Err(Fail::Challenge);
            }
            let Purpose::Act { digest, .. } = &c.purpose else { return Err(Fail::Challenge) };
            if b64url_decode(&a.id, wa::MAX_CREDENTIAL_ID)? != b64url_decode(&pk.credential_id, wa::MAX_CREDENTIAL_ID)? {
                return Err(Fail::CredentialId);
            }
            let cd = b64url_decode(&a.client_data_json, wa::MAX_CLIENT_DATA)?;
            let ad = b64url_decode(&a.authenticator_data, wa::MAX_AUTH_DATA)?;
            let sig = b64url_decode(&a.signature, wa::MAX_SIGNATURE)?;
            let key = b64url_decode(&pk.public_key, 65)?;
            wa::verify_assertion(&rp, &c.bytes, &key, pk.counter, &cd, &ad, &sig).map(|n| (n, digest.clone()))
        })();
        match verdict {
            Ok((counter, digest)) => {
                if counter != pk.counter {
                    rt.level2.passkeys.set_counter(&d.id, counter);
                    let dir = rt.dir.clone();
                    rt.level2.save_passkeys(&dir);
                }
                Ok(digest)
            }
            Err(f) => {
                let _ = rt.level2.fail_minute.hit(&d.id, now);
                let _ = rt.level2.fail_hour.hit(&d.id, now);
                rt.record_target(&d.id, &d.name, Kind::PasskeyFailed, None);
                tracing::info!("remote access: device {}: passkey_failed", d.id);
                tracing::debug!("remote access: assertion refused at {f:?}");
                if f == Fail::CounterRegression {
                    rt.level2.forget_device(&d.id);
                    let dir = rt.dir.clone();
                    rt.level2.save_passkeys(&dir);
                    tracing::warn!("remote access: device {}: its passkey's counter went back, a sign it was copied; the passkey is dropped", d.id);
                }
                Err(L2Error::passkey())
            }
        }
    }

    /// One level 2 action: the passkey check, the digest check, then the
    /// SAME core function the desktop calls, with its refusals.
    pub async fn remote_act(self: &Arc<Self>, d: &DeviceCtx, action: Action, target: &str, option: Option<&str>, a: &Assertion) -> Result<Value, L2Error> {
        let digest = self.l2_verify(d, action, target, option, a)?;
        let shown = self.remote_item(action, target).await.ok().flatten();
        let Some(shown) = shown.filter(|s| s.digest == digest && s.actionable) else {
            self.l2_journal(d, Kind::Changed, target);
            return Err(L2Error::changed());
        };
        let jt = shown.journal_target.clone();
        let done: Result<Value, String> = match action {
            // The plan's own threads, read once more and held to the digest:
            // what starts is exactly what the phone showed.
            Action::PlanApprove => match self.plan(target).filter(|p| p.state == PlanState::Pending && self.plan_shown(p).digest == digest) {
                Some(p) => self.plan_approve(target, p.threads, None).await.map(|p| json!({ "plan": p })),
                None => {
                    self.l2_journal(d, Kind::Changed, &jt);
                    return Err(L2Error::changed());
                }
            },
            Action::PlanRefuse => self.plan_refuse(target, None).map(|p| json!({ "plan": p })),
            // Pinned to the commits the digest covers (review P2): a branch or
            // base that moves after this check stops the merge as "changed".
            Action::Merge => match self.plan_merge_pinned(target, false, None, shown.pins.as_ref()).await {
                Ok(m) => match m.results.iter().find(|r| r.error.as_deref().is_some_and(|e| e.starts_with("changed:"))) {
                    Some(r) => {
                        let merged: Vec<&str> = m.results.iter().filter(|x| x.ok).map(|x| x.thread.as_str()).collect();
                        self.l2_journal(d, Kind::Changed, &jt);
                        let already = if merged.is_empty() { "Nothing was merged.".to_string() } else { format!("Already merged: {}.", merged.join(", ")) };
                        return Err(L2Error::new(409, "changed", format!("sbxw/{} changed since it was approved: the merge stopped there. {already}", r.thread)));
                    }
                    None => Ok(json!({ "merge": m })),
                },
                Err(e) => Err(e),
            },
            Action::NetworkRespond => {
                let o = option.unwrap_or_default();
                match self.approval_answer(target, o).await {
                    Ok(_) => Ok(json!({})),
                    Err(AnswerError::Gone) => {
                        self.l2_journal(d, Kind::Changed, &jt);
                        return Err(L2Error::changed());
                    }
                    Err(AnswerError::NotOffered(o)) => Err(format!("\u{201c}{o}\u{201d} is not one of this request's options")),
                    Err(AnswerError::Sbx(e)) => Err(format!("{e:#}")),
                }
            }
            Action::SandboxRemove => match SandboxRef::parse(target) {
                Ok(sb) => self.remove(&[sb]).await.map(|_| json!({})).map_err(|e| format!("{e:#}")),
                Err(e) => Err(e.to_string()),
            },
        };
        match done {
            Ok(mut v) => {
                self.l2_journal(d, journal_kind(action), &jt);
                v["ok"] = json!(true);
                Ok(v)
            }
            Err(e) => {
                self.l2_journal(d, Kind::Refused, &jt);
                Err(L2Error::refused(e))
            }
        }
    }

    fn l2_journal(&self, d: &DeviceCtx, kind: Kind, target: &str) {
        let mut rt = self.remote_access.lock().unwrap();
        rt.record_target(&d.id, &d.name, kind, Some(target.to_string()));
        let code = serde_json::to_value(kind).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        tracing::info!("remote access: device {}: {code}", d.id);
    }
}

#[derive(Default, Debug, Clone)]
struct BranchFacts {
    thread: String,
    repo: String,
    tip: Option<String>,
    base_tip: Option<String>,
    base_branch: Option<String>,
    ahead: Option<u32>,
    files: Vec<sbxw_core::commits::FileChange>,
    more_files: usize,
}

fn network_view(a: &Approval, m: &impl Fn(&str) -> String, detail: bool) -> Value {
    let (phone, mac): (Vec<_>, Vec<_>) = a.options.iter().partition(|o| l2::phone_option(&o.id, &o.label));
    let mut v = json!({
        "id": a.id, "sandbox": a.sandbox, "host": a.title, "created_at": a.created_at,
        "options": phone.iter().map(|o| json!({ "id": o.id, "label": o.label })).collect::<Vec<_>>(),
        "mac_only": mac.iter().map(|o| if o.label.is_empty() { o.id.clone() } else { o.label.clone() }).collect::<Vec<_>>(),
    });
    if detail {
        v["detail"] = json!(m(&a.detail));
    }
    v
}
