//! Creating a sandbox: the chooser, the workspace and chat dialogs, and the
//! bring-ups in progress (`assets/js/create.js`, the bring-up part of
//! `assets/js/sandboxes.js`). The rules are in [`model`]; this file turns
//! intents and daemon answers into changes of the form and into [`Action`]s.
//!
//! The form and the jobs live in the core (one [`State`] per app), never in
//! the screen: what is sent, and what the confirmations list, is the core's
//! copy. Two confirmations, both drawn by the core:
//!
//! * before sending, when the sandbox would reach more of this machine than
//!   its workspace (extra folders, kit folders, published ports, Chrome):
//!   the lines come from the core's form ([`model::Form::grants`]);
//! * when the daemon refuses for want of host access its `sbxw.toml` asks
//!   for (`409 {consent: {config, grants}}`): the lines are the daemon's,
//!   sent back as `approved` on Allow, as `postBringUp` does.
//!
//! Routes (`crates/daemon/src/http/ui.rs`): `GET /api/fs?path=&host=`,
//! `POST /api/fs/pick`, `GET`/`POST /api/fs/favourites`, `GET /api/remote-hosts`,
//! `POST /api/remote-hosts/:host/clone`, `GET /api/kit-mounts`,
//! `GET /api/template/status`, `POST /api/carve/preview`,
//! `POST /api/sandboxes/create`, `POST /api/sandboxes/chat`; the
//! `provision` stream event.

pub mod model;
mod standin;

use super::{Action, Asked, Call, Confirm, Ctx, Feature, ScreenDecl, Slot};
use crate::client::enc;
use crate::intent::{Arg, Risk, Spec};
use model::{Answer, Denied, Dialog, Form, Job, JobState, Listing, Preview};
use serde_json::{json, Value as J};
#[cfg(not(test))]
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The dialog's screen. The bring-ups have no screen: the status bar shows
/// them, and its press lists them in a popover (shell/statusbar/jobs.rs).
pub const SCREEN: &str = "create";
/// `data.create`: the form (and the jobs, for the scripts) as the screens see them.
pub const KEY: &str = "create";
/// A bring-up can pull an image: give it half an hour.
const BRING_UP_SECS: u64 = 1800;
/// The native folder picker waits for the person.
const PICK_SECS: u64 = 600;

const DIALOGS: &[&str] = &["chooser", "workspace", "chat"];
const FIELDS: &[&str] = &["name", "chat_name", "git_url", "port_sandbox", "port_host", "port_alias", "kit_mount", "carve_typed"];

pub const FEATURE: Feature = Feature {
    name: "create",
    events: &["provision"],
    screens: &[
        ScreenDecl { name: SCREEN, src: include_str!("../../../screens/create.luau"), slot: Slot::Modal { width: 640.0, light_dismiss: false } },
    ],
    intents: &[
        Spec { name: "create_open", risk: Risk::Direct, args: &[("dialog", Arg::OneOf(DIALOGS), false)], doc: "Opens the New sandbox dialog: the chooser (default), the workspace form or the chat form." },
        Spec { name: "create_host", risk: Risk::Direct, args: &[("host", Arg::Str, true)], doc: "Where the sandbox runs: `local` or a remote host's name (the folder list follows)." },
        Spec { name: "create_browse", risk: Risk::Direct, args: &[("path", Arg::Str, false)], doc: "Shows that folder in the dialog's folder list (no `path`: the home folder)." },
        Spec { name: "create_pick", risk: Risk::Direct, args: &[("kit", Arg::Int(1, 64), false)], doc: "Opens the system's folder picker; the folder chosen is browsed to, or fills kit folder `kit`." },
        Spec { name: "create_star", risk: Risk::Direct, args: &[("path", Arg::Str, true)], doc: "Stars or unstars a folder (the shortcuts above the folder list)." },
        Spec {
            name: "create_repo",
            risk: Risk::Direct,
            args: &[("action", Arg::OneOf(&["toggle", "add", "main", "rw", "ro", "remove"]), true), ("path", Arg::Str, false), ("index", Arg::Int(1, 64), false)],
            doc: "The sandbox's folders: `toggle` a `path`, `add` the folder shown; on folder `index`: make it the workspace (`main`), read-write, read-only, `remove`.",
        },
        Spec {
            name: "create_carve",
            risk: Risk::Direct,
            args: &[("action", Arg::OneOf(&["pick", "use", "typed", "cancel", "none", "ro", "strict", "soft", "remove"]), true), ("path", Arg::Str, false), ("index", Arg::Int(1, 64), false)],
            doc: "Restricted sub-folders: `pick` one inside `path`, `use` the folder shown, add the `typed` one, `cancel`; on restriction `index`: no access, read-only, strict, soft, `remove`.",
        },
        Spec { name: "create_port", risk: Risk::Direct, args: &[("action", Arg::OneOf(&["add", "remove"]), true), ("index", Arg::Int(1, 64), false)], doc: "Adds a port row, or removes row `index`." },
        Spec {
            name: "create_set",
            risk: Risk::Direct,
            args: &[("field", Arg::OneOf(FIELDS), true), ("value", Arg::Str, false), ("index", Arg::Int(1, 64), false)],
            doc: "Sets a text field of the form (no `value`: empty); `index` for a port row or a kit folder. The name is cleaned as typed.",
        },
        Spec { name: "create_toggle", risk: Risk::Direct, args: &[("field", Arg::OneOf(&["remote_control", "chrome", "rebuild"]), true), ("on", Arg::Bool, true)], doc: "Ticks or unticks Remote control, Claude in Chrome, or Rebuild the starting template." },
        Spec {
            name: "create_submit",
            risk: Risk::Direct,
            args: &[],
            doc: "Creates the workspace sandbox from the core's form. When it would reach more of this machine than its workspace, the core first lists what in its own dialog.",
        },
        Spec { name: "create_chat", risk: Risk::Direct, args: &[], doc: "Creates the chat sandbox (an empty folder, no code of yours)." },
        Spec { name: "create_retry", risk: Risk::Direct, args: &[("sandbox", Arg::Str, true)], doc: "Sends a failed creation again, as it was." },
        Spec { name: "create_dismiss", risk: Risk::Direct, args: &[("sandbox", Arg::Str, true)], doc: "Forgets a finished or failed creation." },
    ],
    act,
    on_event: Some(on_event),
    on_answer: Some(on_answer),
    extras: Some(extras),
    standin: Some(standin::route),
    // The host-access lines go out only from the core's dialog, on Allow.
    core_only: &["approved"],
    // The web UI's ⌥N and ⌥M (Ctrl+Option on macOS).
    shortcuts: &[
        crate::feat::Shortcut { key: 'n', intent: "create_open", args: "{}", help: "New sandbox" },
        crate::feat::Shortcut { key: 'm', intent: "create_open", args: r#"{"dialog":"chat"}"#, help: "New chat" },
    ],
    ..Feature::EMPTY
};

/// The app's form and jobs.
#[derive(Debug, Default)]
pub struct State {
    pub form: Form,
    pub jobs: Vec<Job>,
    /// The host picked last time.
    pub last_host: String,
    /// Bring-ups that ended ready, and when (the status bar's "created").
    pub ready: Vec<(String, Instant)>,
    /// Kit installs that failed on hosts sbx denied, one per sandbox
    /// (`data.create.denied`; the Kits panel and the bring-ups' popover).
    pub denied: Vec<Denied>,
}

/// How long the status bar says "created" after a bring-up ends ready.
pub const CREATED_FOR: Duration = Duration::from_secs(4);

/// The bring-ups for the status bar (shell/statusbar.rs): the ones in the
/// list (running, asking, failed), then the ones ready in the last
/// [`CREATED_FOR`], most recent first.
pub fn bar_jobs() -> Vec<model::BarJob> {
    let mut st = state();
    st.bar_jobs(Instant::now())
}

impl State {
    pub fn bar_jobs(&mut self, now: Instant) -> Vec<model::BarJob> {
        self.ready.retain(|(_, t)| now.saturating_duration_since(*t) < CREATED_FOR);
        let mut out: Vec<model::BarJob> = self.jobs.iter().filter_map(model::bar_job).collect();
        for (name, t) in &self.ready {
            if !out.iter().any(|j| &j.sandbox == name) {
                out.push(model::BarJob { sandbox: name.clone(), step: None, frac: 1.0, state: model::BarState::Created, since: *t });
            }
        }
        out.sort_by(|a, b| b.since.cmp(&a.since));
        out
    }

    fn mark_ready(&mut self, name: &str) {
        self.ready.retain(|(n, _)| n != name);
        self.ready.push((name.to_string(), Instant::now()));
    }
}

#[cfg(not(test))]
static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));

/// The bring-ups and the form: one per process.
#[cfg(not(test))]
fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap()
}

// Under test, one per test thread: an App test's bring-ups (a fixture's
// replayed `provision`, a stand-in's) are drawn by the status bar and the
// corner cards of that test's window only, never over another test's panes.
#[cfg(test)]
thread_local! {
    static STATE: &'static Mutex<State> = Box::leak(Box::new(Mutex::new(State::default())));
}

#[cfg(test)]
fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.with(|s| *s).lock().unwrap()
}

fn act(ctx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
    state().act(ctx, a)
}
fn on_event(ctx: &Ctx, name: &str, data: &J) -> Vec<Action> {
    state().on_event(ctx, name, data)
}
fn on_answer(ctx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
    state().on_answer(ctx, call, r)
}

/// "New sandbox" (the sidebar's header, or the rail when it is hidden).
/// Bring-ups in progress show in the status bar (shell/statusbar.rs), whose
/// press lists them in a popover.
fn extras(_: &Ctx) -> Vec<super::Extra> {
    vec![super::Extra {
        place: super::Place::Topbar,
        id: "create".into(),
        label: "New".into(),
        icon: Some("plus".into()),
        tone: "muted".into(),
        help: "New sandbox".into(),
        count: None,
        intent: Some(("create_open".into(), json!({}))),
    }]
}

/// Every sandbox ref that exists (names are refused when taken).
fn taken(ctx: &Ctx) -> Vec<String> {
    ctx.get("sandboxes").as_array().map(|a| a.iter().filter_map(|s| s["name"].as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn idx(a: &Asked) -> usize {
    a.i("index").unwrap_or(1).max(1) as usize - 1
}

impl State {
    /// `data.create`.
    pub fn view(&self, taken: &[String]) -> J {
        let f = &self.form;
        let starred = |p: &str| f.favourites.iter().any(|x| x.path == p);
        let listing = f.listing.as_ref().map(|l| {
            let root = f.carve_pick.as_ref().map(|p| p.root.clone());
            let can_up = l.parent.as_deref().is_some_and(|p| root.as_deref().is_none_or(|r| p == r || model::inside(p, r)));
            json!({
                "path": l.path, "parent": l.parent, "can_up": can_up,
                "entries": l.entries.iter().map(|(n, p)| json!({ "name": n, "path": p, "starred": starred(p), "added": f.has_repo(p) })).collect::<Vec<_>>(),
            })
        });
        let nested = f.nested();
        let repo_hint = match &nested {
            Some((a, b)) => format!("{a} is inside {b}. Pick folders side by side, not one inside another."),
            None if !f.repos.is_empty() => "The agent works in the first one. The others sit beside it, at the same path.".into(),
            None => "Other repos the agent can read, or change, next to the project folder.".into(),
        };
        let parents = f.carve_parents();
        let base = |p: &str| p.trim_end_matches('/').rsplit('/').next().unwrap_or(p).to_string();
        let blocked = f.carve_blocked();
        let carves: Vec<J> = f
            .carves
            .iter()
            .map(|c| {
                let par = f.carve_parent_of(&c.path).unwrap_or_default();
                json!({
                    "path": c.path,
                    "rel": c.path[par.len().min(c.path.len())..].trim_start_matches('/'),
                    "parent_label": (parents.len() > 1).then(|| base(&par)),
                    "access": c.access, "enforce": c.enforce,
                    "note": if c.enforce == "strict" { model::CARVE_STRICT } else { model::CARVE_SOFT },
                    "hidden_count": f.carve_hidden_files(c).len(),
                    "error": blocked.then(|| match &f.preview { Some(Preview::Answer(v)) => v["error"].as_str().unwrap_or("This cannot be mounted.").to_string(), _ => String::new() }),
                })
            })
            .collect();
        let preview = f.preview_summary().map(|summary| {
            let (warnings, files, more, stub) = match &f.preview {
                Some(Preview::Answer(v)) if v["ok"] != false => {
                    let files: Vec<J> = v["unshared"].as_array().cloned().unwrap_or_default();
                    let shown = files.iter().take(20).cloned().collect::<Vec<_>>();
                    (v["warnings"].clone(), shown, files.len().saturating_sub(20), v["stub"].as_bool().unwrap_or(false))
                }
                _ => (json!([]), vec![], 0, false),
            };
            json!({ "summary": summary, "bad": blocked, "warnings": warnings, "files": files, "more": more, "stub": stub })
        });
        let name_problem = f.name_problem(&f.name, taken, false);
        let create_problem = if let Some(p) = &name_problem {
            Some(p.clone())
        } else if f.main_path().is_empty() && !(f.remote() && !f.git_url.trim().is_empty()) {
            Some("Pick a folder.".into())
        } else if nested.is_some() {
            Some("Folders are nested.".into())
        } else if blocked {
            Some("A restriction cannot be mounted.".into())
        } else {
            f.ports_body().err()
        };
        let chat_name = if f.chat_name.is_empty() { None } else { Some(f.chat_name.as_str()) };
        let mut hosts = vec![];
        if !f.hosts.is_empty() {
            hosts.push(json!({ "id": "local", "label": "local (this machine)" }));
            for h in &f.hosts {
                let state = match h.link.as_str() {
                    "up" => "",
                    "down" => ", down",
                    _ => ", not connected yet",
                };
                hosts.push(json!({ "id": h.name, "label": format!("{} ({}{state})", h.name, h.address) }));
            }
        }
        json!({
            "dialog": f.dialog.map(Dialog::name),
            "hosts": hosts,
            "host": f.host,
            "remote": f.remote(),
            "name": f.name,
            "name_note": f.name_note,
            "name_problem": name_problem,
            "chat_name": f.chat_name,
            "chat_suggestion": f.chat_suggestion,
            "chat_note": f.chat_note,
            "chat_problem": chat_name.and_then(|n| f.name_problem(n, taken, true)),
            "path": if f.carve_pick.is_some() { f.selected.clone() } else { f.listing.as_ref().map(|l| l.path.clone()).unwrap_or_else(|| f.selected.clone()) },
            "path_tag": if f.repos.is_empty() { "Selected" } else { "Browsing" },
            "listing": listing,
            "listing_error": f.listing_error,
            "favourites": f.favourites.iter().map(|x| json!({ "path": x.path, "name": x.name, "missing": x.missing })).collect::<Vec<_>>(),
            "repos": f.repos.iter().map(|r| json!({ "path": r.path, "readonly": r.readonly })).collect::<Vec<_>>(),
            "repo_hint": repo_hint,
            "repo_hint_error": nested.is_some(),
            "can_add_repo": !f.selected.is_empty() && !f.has_repo(&f.selected) && f.carve_pick.is_none(),
            "carve_parents": parents.iter().map(|p| json!({ "path": p, "label": base(p) })).collect::<Vec<_>>(),
            "carves": carves,
            "carve_pick": f.carve_pick.as_ref().map(|p| json!({ "root": p.root, "cur": p.cur, "can_use": p.cur != p.root, "typed": p.typed, "error": p.error })),
            "preview": preview,
            "ports": f.ports.iter().map(|p| json!({ "sandbox": p.sandbox, "host": p.host, "alias": p.alias })).collect::<Vec<_>>(),
            "kit_mounts": f.kit_mounts.iter().map(|k| json!({ "kit": k.kit, "target": k.target, "host": k.host })).collect::<Vec<_>>(),
            "template": f.template.as_ref().map(|t| json!({ "note": t.note, "help": t.help })),
            "rebuild": f.rebuild,
            "remote_control": f.remote_control,
            "chrome": f.chrome,
            "git_url": f.git_url,
            "can_create": f.can_create(taken),
            "create_problem": create_problem,
            "error": f.error,
            "jobs": self.jobs_view(),
            "denied": self.denied.iter().map(Denied::view).collect::<Vec<_>>(),
        })
    }

    fn set(&self, taken: &[String]) -> Action {
        Action::Set { key: KEY.into(), value: self.view(taken) }
    }

    /// Reads a folder into the list (no path: home).
    fn browse(&mut self, path: Option<&str>) -> Action {
        self.form.browse_seq += 1;
        let mut q = vec![];
        if let Some(p) = path {
            q.push(format!("path={}", enc(p)));
        }
        if self.form.remote() {
            q.push(format!("host={}", enc(&self.form.host)));
        }
        let path = if q.is_empty() { "/api/fs".to_string() } else { format!("/api/fs?{}", q.join("&")) };
        Action::Call(Call::get(path, "read a folder").tag(&format!("fs:{}", self.form.browse_seq)))
    }

    /// The carve preview, when the folders or restrictions moved.
    fn preview(&mut self) -> Option<Action> {
        let (seq, body) = self.form.preview_now(std::time::Instant::now() + model::PREVIEW_DELAY)?;
        Some(Action::Call(Call::post("/api/carve/preview", body, "preview the restrictions").quiet().tag(&format!("preview:{seq}"))))
    }

    pub fn act(&mut self, ctx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
        let taken = taken(ctx);
        let mut out = vec![];
        let f = &mut self.form;
        match a.name {
            "create_open" => {
                // The host last used, across runs too (the app's state file).
                if self.last_host.is_empty() {
                    if let Some(h) = ctx.get("app")["kept"]["create"]["host"].as_str() {
                        self.last_host = h.to_string();
                    }
                }
                let f = &mut self.form;
                let d = a.s("dialog").and_then(Dialog::parse).unwrap_or(Dialog::Chooser);
                match d {
                    Dialog::Chooser => f.dialog = Some(Dialog::Chooser),
                    Dialog::Chat => f.reset_chat(format!("chat-{}", crate::client::random_hex(3))),
                    Dialog::Workspace => {
                        f.reset_workspace(&self.last_host);
                        out.push(Action::Call(Call::get("/api/fs/favourites", "read the starred folders").tag("favourites")));
                        out.push(Action::Call(Call::get("/api/remote-hosts", "read the remote hosts").tag("hosts")));
                        out.push(Action::Call(Call::get("/api/kit-mounts", "read the kits' folders").tag("kit-mounts")));
                        out.push(Action::Call(Call::get("/api/template/status", "read the template's state").tag("template")));
                        out.push(self.browse(None));
                    }
                }
                out.push(Action::OpenModal { screen: SCREEN.into(), arg: d.name().into() });
            }
            "create_host" => {
                f.set_host(a.req("host"));
                self.last_host = f.host.clone();
                out.push(Action::Remember { key: "host".into(), value: J::String(f.host.clone()) });
                if !f.remote() {
                    out.push(Action::Call(Call::get("/api/kit-mounts", "read the kits' folders").tag("kit-mounts")));
                }
                out.push(self.browse(None));
            }
            "create_browse" => {
                let p = a.s("path").map(str::to_string);
                if let (Some(pk), Some(p)) = (&f.carve_pick, &p) {
                    if *p != pk.root && !model::inside(p, &pk.root) {
                        return Err(format!("Choose a folder inside {}.", pk.root));
                    }
                }
                out.push(self.browse(p.as_deref()));
            }
            "create_pick" => {
                f.error = None;
                let tag = match a.i("kit") {
                    Some(k) => format!("pick:{k}"),
                    None => "pick".into(),
                };
                out.push(Action::Call(Call::post("/api/fs/pick", json!({}), "open the folder picker").timeout(PICK_SECS).tag(&tag)));
            }
            "create_star" => {
                let path = a.req("path");
                let on = !f.favourites.iter().any(|x| x.path == path);
                out.push(Action::Call(Call::post("/api/fs/favourites", json!({ "path": path, "favourite": on }), "star a folder").tag("favourites")));
            }
            "create_repo" => match a.req("action") {
                "toggle" => f.toggle_repo(a.s("path").ok_or("create_repo toggle: 'path' is required")?),
                "add" => {
                    if !f.selected.is_empty() && !f.has_repo(&f.selected) {
                        let p = f.selected.clone();
                        f.toggle_repo(&p);
                    }
                }
                "main" => f.repo_main(idx(a)),
                "rw" => f.repo_mode(idx(a), false),
                "ro" => f.repo_mode(idx(a), true),
                _ => f.repo_remove(idx(a)),
            },
            "create_carve" => match a.req("action") {
                "pick" => {
                    let root = a.s("path").ok_or("create_carve pick: 'path' is required")?.to_string();
                    if !f.carve_pick_start(&root) {
                        return Err(format!("{root} is not one of this sandbox's read-write folders."));
                    }
                    out.push(self.browse(Some(&root)));
                }
                "use" | "typed" => {
                    if let Some(back) = f.carve_add(a.req("action") == "typed") {
                        out.push(self.browse(Some(&back)));
                    }
                }
                "cancel" => {
                    if let Some(back) = f.carve_pick_end() {
                        out.push(self.browse(if back.is_empty() { None } else { Some(&back) }));
                    }
                }
                what => f.carve_set(idx(a), what),
            },
            "create_port" => {
                if a.req("action") == "add" {
                    f.ports.push(model::PortRow::default());
                } else if idx(a) < f.ports.len() {
                    f.ports.remove(idx(a));
                }
            }
            "create_set" => {
                let v = a.s("value").unwrap_or("");
                let i = idx(a);
                match a.req("field") {
                    "name" => f.set_name(v),
                    "chat_name" => f.set_chat_name(v),
                    "git_url" => f.git_url = v.to_string(),
                    "port_sandbox" => f.set_port(i, "sandbox", v),
                    "port_host" => f.set_port(i, "host", v),
                    "port_alias" => f.set_port(i, "alias", v),
                    "kit_mount" => {
                        if let Some(k) = f.kit_mounts.get_mut(i) {
                            k.host = v.to_string();
                        }
                    }
                    _ => {
                        if let Some(p) = &mut f.carve_pick {
                            p.typed = v.to_string();
                            p.error = None;
                        }
                    }
                }
            }
            "create_toggle" => {
                let on = a.b("on").unwrap_or(false);
                match a.req("field") {
                    "remote_control" => f.remote_control = on,
                    "chrome" => f.chrome = on,
                    _ => f.rebuild = on,
                }
            }
            "create_submit" => out.extend(self.submit(&taken)?),
            "create_chat" => out.extend(self.chat(&taken)?),
            "create_retry" => {
                let name = a.req("sandbox");
                // A kit install that failed after its bring-up ended: the
                // same request again (the daemon adopts the sandbox and adds
                // the kits not applied yet).
                if !self.jobs.iter().any(|j| j.sandbox == name) {
                    let d = self.denied.iter().find(|d| d.sandbox == name).ok_or_else(|| format!("No creation of {name} to retry."))?;
                    let (Some(route), Some(body)) = (d.route, d.body.clone()) else { return Err(format!("{name} was started elsewhere: bring it up again from there.")) };
                    let mut job = Job::new(name, Some(route), Some(body));
                    job.clone = d.clone.clone();
                    self.jobs.push(job);
                }
                let j = self.jobs.iter_mut().find(|j| j.sandbox == name).expect("a job");
                let (Some(route), Some(body)) = (j.route, j.body.clone()) else { return Err(format!("{name} was started elsewhere: create it again from New sandbox.")) };
                j.restart();
                let clone = j.clone.clone();
                self.denied.retain(|d| d.sandbox != name);
                out.push(Action::Call(bring_up(route, name, body, vec![], clone)));
            }
            "create_dismiss" => {
                self.jobs.retain(|j| j.sandbox != a.req("sandbox"));
                self.denied.retain(|d| d.sandbox != a.req("sandbox"));
            }
            other => return Err(format!("{other}: not a create intent")),
        }
        out.extend(self.preview());
        // The view first: the calls above answer later and set it again.
        out.insert(0, self.set(&taken));
        Ok(out)
    }

    fn submit(&mut self, taken: &[String]) -> Result<Vec<Action>, String> {
        let f = &self.form;
        if let Some(p) = f.name_problem(&f.name, taken, false) {
            return Err(p);
        }
        if !f.can_create(taken) {
            return Err("The form is not complete: pick a folder, and check the folders and ports.".into());
        }
        let body = f.create_body()?;
        let name = f.sandbox_ref(&f.name);
        let clone = (f.remote() && !f.git_url.trim().is_empty()).then(|| (f.host.clone(), f.git_url.trim().to_string()));
        let grants = f.grants();
        let summary = f.summary();
        self.last_host = f.host.clone();
        self.form.dialog = None;
        let mut job = Job::new(&name, Some("create"), Some(body.clone()));
        job.clone = clone.clone();
        let call = bring_up("create", &name, body, vec![], clone);
        let mut out = vec![Action::CloseModal];
        if grants.is_empty() {
            self.add_job(job);
            out.push(Action::Call(call));
        } else {
            // Shown in the status bar as waiting for the answer: a Cancel
            // leaves it there, its popover has Retry (asks again) and Dismiss.
            job.state = JobState::Asking("Waiting for your answer: the sandbox reaches more of this machine than its workspace.".into());
            job.posting = false;
            self.add_job(job);
            let mut c = Confirm::new(format!("Create {name}?"), call).line(summary).line("It will also be able to:").labels("Create", "Cancel");
            for g in grants {
                c = c.line(format!("• {g}"));
            }
            out.push(c.into_action());
        }
        Ok(out)
    }

    fn chat(&mut self, taken: &[String]) -> Result<Vec<Action>, String> {
        let f = &self.form;
        let typed = f.chat_name.clone();
        if let Some(p) = f.name_problem(&typed, taken, true) {
            return Err(p);
        }
        let name = if typed.is_empty() { f.chat_suggestion.clone() } else { typed };
        let body = json!({ "name": name });
        self.form.dialog = None;
        self.add_job(Job::new(&name, Some("chat"), Some(body.clone())));
        Ok(vec![Action::CloseModal, Action::Call(bring_up("chat", &name, body, vec![], None))])
    }

    fn add_job(&mut self, job: Job) {
        self.jobs.retain(|j| j.sandbox != job.sandbox);
        self.jobs.push(job);
    }

    pub fn on_event(&mut self, ctx: &Ctx, name: &str, data: &J) -> Vec<Action> {
        if name != "provision" {
            return vec![];
        }
        let text = match data {
            J::String(s) => s.clone(),
            v => v.to_string(),
        };
        let ev: J = serde_json::from_str(&text).unwrap_or_default();
        if let Some(mut d) = Denied::from_event(&ev) {
            // Kept with the request that brought it up, for Retry.
            if let Some(j) = self.jobs.iter().find(|j| j.sandbox == d.sandbox) {
                (d.route, d.body, d.clone) = (j.route, j.body.clone(), j.clone.clone());
            }
            self.denied.retain(|x| x.sandbox != d.sandbox);
            let n = d.hosts.len();
            let what = d.kit.clone().map(|k| format!("kit {k}")).unwrap_or_else(|| "its kits".into());
            let name = d.sandbox.clone();
            self.denied.push(d);
            return vec![
                Action::Notice {
                    text: format!("{name}: sbx denied {n} host{} while installing {what}: allow them in Kits, then Retry", if n == 1 { "" } else { "s" }),
                    error: true,
                    then: Some(("kits_show".into(), json!({ "sandbox": name }))),
                },
                self.set(&taken(ctx)),
            ];
        }
        let had = self.jobs.iter().any(|j| ev["sandbox"] == j.sandbox.as_str());
        // A bring-up this app posted, whose answer came before its `done`:
        // the daemon's timing summary in a notice of its own (plain text).
        if let (false, "done", true, Some(t), Some(sb)) = (had, ev["kind"].as_str().unwrap_or(""), ev["error"].is_null(), ev["summary"].as_str().map(str::trim).filter(|t| !t.is_empty()), ev["sandbox"].as_str()) {
            if self.ready.iter().any(|(n, _)| n == sb) {
                return vec![Action::Notice { text: format!("Sandbox \"{sb}\": {t}"), error: false, then: None }];
            }
        }
        let failed_before = self.jobs.iter().any(|j| ev["sandbox"] == j.sandbox.as_str() && matches!(j.state, JobState::Failed(_)));
        let Some(sandbox) = model::apply_provision(&mut self.jobs, &text) else { return vec![] };
        // A watched bring-up that ended ready (a posted one ends with its answer).
        let done_ok = had && ev["kind"] == "done" && ev["error"].is_null();
        if done_ok && self.jobs.iter().find(|j| j.sandbox == sandbox).is_none_or(|j| j.state == JobState::Ready) {
            self.mark_ready(&sandbox);
        }
        let mut out = vec![];
        // A job held for the person's answer runs once the daemon starts it.
        if let Some(j) = self.jobs.iter_mut().find(|j| j.sandbox == sandbox) {
            if matches!(j.state, JobState::Asking(_)) && j.route.is_some() {
                j.state = JobState::Running;
                j.posting = true;
            }
        }
        // One this app only watches failed: said once (one it posted says so with its answer).
        if let Some(JobState::Failed(e)) = self.jobs.iter().find(|j| j.sandbox == sandbox).map(|j| &j.state) {
            if !failed_before {
                out.push(failed_notice(&sandbox, e));
            }
        }
        out.push(self.set(&taken(ctx)));
        out
    }

    pub fn on_answer(&mut self, ctx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
        let taken = taken(ctx);
        let (kind, rest) = call.tag.split_once(':').unwrap_or((call.tag.as_str(), ""));
        let mut out = vec![];
        let f = &mut self.form;
        let refusal = |r: &Result<(u16, J), String>| -> Option<String> {
            match r {
                Err(e) => Some(e.clone()),
                Ok((s, v)) if !(200..300).contains(s) || v["ok"] == false => Some(v["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {s}"))),
                _ => None,
            }
        };
        match kind {
            "fs" => {
                let seq: u64 = rest.parse().unwrap_or(0);
                if seq == f.browse_seq {
                    match (&r, refusal(&r)) {
                        (Ok((_, v)), None) => match Listing::from_json(v).map(|l| f.take_listing(seq, l)) {
                            Some(Err(root)) => out.push(self.browse(Some(&root))),
                            Some(Ok(_)) => {}
                            None => f.listing_error = Some("Error loading directory".into()),
                        },
                        (_, Some(e)) => f.listing_error = Some(format!("Error loading directory: {e}")),
                        _ => {}
                    }
                }
            }
            "favourites" => match refusal(&r) {
                None => f.favourites = model::favourites_from(&r.as_ref().unwrap().1),
                Some(e) => f.error = Some(format!("Could not update favourites: {e}")),
            },
            "hosts" => {
                // No remote hosts, or an older daemon: the field stays hidden.
                f.hosts = match &r {
                    Ok((200, v)) => v["hosts"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|h| {
                                    Some(model::RemoteHost { name: h["name"].as_str()?.into(), address: h["address"].as_str().unwrap_or("").into(), link: h["link"].as_str().unwrap_or("never").into() })
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    _ => vec![],
                };
                if !f.hosts.iter().any(|h| h.name == f.host) && f.host != "local" {
                    f.host = "local".into();
                }
            }
            "kit-mounts" => {
                if !f.remote() {
                    f.kit_mounts = match &r {
                        Ok((200, v)) => v["mounts"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|m| Some(model::KitMount { kit: m["kit"].as_str().unwrap_or("").into(), target: m["target"].as_str()?.into(), host: m["host"].as_str().unwrap_or("").into() }))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        _ => vec![],
                    };
                }
            }
            "template" => f.template = r.as_ref().ok().and_then(|(s, v)| (*s == 200).then(|| model::template_offer(v)).flatten()),
            "pick" => match &r {
                Ok((_, v)) if v["cancelled"] == true => {}
                Ok((s, v)) if (200..300).contains(s) && v["ok"] == true => {
                    let path = v["path"].as_str().unwrap_or("").to_string();
                    match rest.parse::<usize>() {
                        Ok(k) => {
                            if let Some(m) = f.kit_mounts.get_mut(k.saturating_sub(1)) {
                                m.host = path;
                            }
                        }
                        Err(_) => out.push(self.browse(Some(&path))),
                    }
                }
                _ => f.error = Some(refusal(&r).unwrap_or_else(|| "Failed to open the folder picker".into())),
            },
            "preview" => {
                let seq: u64 = rest.parse().unwrap_or(0);
                let p = match &r {
                    Ok((404, _)) | Err(_) => Preview::Unavailable,
                    Ok((_, v)) => Preview::Answer(v.clone()),
                };
                f.take_preview(seq, p);
            }
            "clone" => out.extend(self.cloned(rest, r)),
            "up" => out.extend(self.answered(rest, call, r)),
            _ => {}
        }
        out.extend(self.preview());
        out.insert(0, self.set(&taken));
        out
    }

    /// A git URL was cloned on the remote host: create on the clone.
    fn cloned(&mut self, name: &str, r: Result<(u16, J), String>) -> Vec<Action> {
        let Some(j) = self.jobs.iter_mut().find(|j| j.sandbox == name) else { return vec![] };
        match r {
            Ok((s, v)) if (200..300).contains(&s) && v["ok"] != false => {
                let mut body = j.body.clone().unwrap_or_default();
                body["path"] = v["path"].clone();
                vec![Action::Call(bring_up(j.route.unwrap_or("create"), name, body, vec![], None))]
            }
            r => {
                let (host, url) = j.clone.clone().unwrap_or_default();
                let why = match r {
                    Err(e) => e,
                    Ok((s, v)) => v["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {s}")),
                };
                j.posting = false;
                let e = format!("Could not clone {url} on {host}: {why}");
                let n = failed_notice(name, &e);
                j.state = JobState::Failed(e);
                vec![n]
            }
        }
    }

    /// A bring-up's answer: ready (open its terminal), a question about
    /// host access (the core's dialog, with the daemon's lines), or why not.
    fn answered(&mut self, name: &str, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
        let Some(j) = self.jobs.iter_mut().find(|j| j.sandbox == name) else { return vec![] };
        let approved: Vec<String> = call.body["approved"].as_array().map(|a| a.iter().filter_map(J::as_str).map(str::to_string).collect()).unwrap_or_default();
        let answer = match &r {
            Err(e) => Answer::Failed(e.clone()),
            Ok((s, v)) => model::read_answer(*s, v, name, &approved),
        };
        j.posting = false;
        match answer {
            Answer::Ready { name: real, warnings } => {
                j.state = JobState::Ready;
                let shown = j.sandbox.clone();
                // The daemon's timing summary, when its `done` came first
                // (plain text; a later one gets a notice of its own, `on_event`).
                let timing = j.summary.as_ref().map_or(String::new(), |t| format!(" ({t})"));
                let text = match warnings.last() {
                    Some(w) => format!("Sandbox \"{real}\" is ready{timing}; {w}"),
                    None if j.body.as_ref().is_some_and(|b| b["remote_control"] == true) => format!("Sandbox \"{real}\" is ready{timing}: press Remote in its pane (before sbx 0.53: /login there first)"),
                    None => format!("Sandbox \"{real}\" is ready{timing}"),
                };
                let error = !warnings.is_empty();
                self.mark_ready(&shown);
                // Done: the job goes; a warning beside it is the notice's (in red).
                self.jobs.retain(|j| j.sandbox != name);
                vec![
                    Action::Notice { text, error, then: None },
                    Action::Intent { name: "select".into(), args: json!({ "sandbox": real }) },
                    Action::Intent { name: "open_terminal".into(), args: json!({ "sandbox": real, "pane": "spare" }) },
                ]
            }
            Answer::AskConsent(c) => {
                j.state = JobState::Asking(format!("Waiting for your answer: {} asks for host access you have not approved yet.", c.config));
                // The lines are the daemon's, shown by the core's dialog under
                // "You grant:" and sent back as `approved` only on Allow.
                let again = bring_up(j.route.unwrap_or("create"), name, call.body.clone(), vec![], None);
                let d = Confirm::new("Allow host access?", again)
                    .line(format!("{} asks for host access you have not approved yet.", c.config))
                    .line("This file can be edited from inside a sandbox. Allow only what you expect. `sbxw config grants --revoke` forgets the approval.")
                    .grant_lines("approved", c.grants)
                    .labels("Allow", "Cancel")
                    .danger();
                vec![d.into_action()]
            }
            Answer::Failed(e) => {
                let n = failed_notice(name, &e);
                j.state = JobState::Failed(e);
                vec![n]
            }
        }
    }
}

/// A creation failed: an error notice; the status bar keeps it in red until
/// it is retried or dismissed (its popover has both).
fn failed_notice(name: &str, why: &str) -> Action {
    Action::Notice { text: format!("{name} was not created: {}. The status bar keeps it until you dismiss it.", why.trim_end_matches('.')), error: true, then: None }
}

/// The bring-ups as the status bar's popover lists them, oldest first.
pub fn jobs_view() -> Vec<J> {
    state().jobs_view()
}

impl State {
    /// Each job's view, with the hosts sbx denied it (`denied`) when any.
    pub fn jobs_view(&self) -> Vec<J> {
        self.jobs
            .iter()
            .map(|j| {
                let mut v = model::job_view(j);
                if let Some(d) = self.denied.iter().find(|d| d.sandbox == j.sandbox) {
                    v["denied"] = d.view();
                }
                v
            })
            .collect()
    }

    /// An allow rule was added for `host` (network's answer): marked on
    /// the kit failures that list it, for that sandbox or, global, all.
    pub fn mark_allowed(&mut self, sandbox: &str, host: &str, global: bool) -> bool {
        let mut changed = false;
        for d in self.denied.iter_mut().filter(|d| global || d.sandbox == sandbox) {
            if d.hosts.iter().any(|h| h == host) && !d.allowed.iter().any(|h| h == host) {
                d.allowed.push(host.to_string());
                changed = true;
            }
        }
        changed
    }
}

/// Network's answer to an allow rule it added: the kit failures listing
/// that host say so (`data.create`), or nothing when none does.
pub fn mark_allowed(ctx: &Ctx, sandbox: &str, host: &str, global: bool) -> Option<Action> {
    let mut st = state();
    st.mark_allowed(sandbox, host, global).then(|| st.set(&taken(ctx)))
}

/// `POST /api/sandboxes/<route>` with the approved lines, or the clone first.
fn bring_up(route: &str, name: &str, mut body: J, approved: Vec<String>, clone: Option<(String, String)>) -> Call {
    if let Some((host, url)) = clone {
        return Call::post(format!("/api/remote-hosts/{}/clone", enc(&host)), json!({ "url": url }), format!("clone {url} on {host}")).timeout(BRING_UP_SECS).tag(&format!("clone:{name}"));
    }
    // Never set here: only the core's dialog adds `approved` (core_only).
    let _ = approved;
    if let Some(o) = body.as_object_mut() {
        o.remove("approved");
    }
    Call::post(format!("/api/sandboxes/{route}"), body, format!("create {name}")).timeout(BRING_UP_SECS).tag(&format!("up:{name}"))
}

#[cfg(test)]
mod tests;
