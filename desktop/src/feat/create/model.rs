//! The create dialogs' state and rules, with no daemon and no window: what
//! the person chose, what the core sends, what it asks before sending, and a
//! bring-up's progress from the bus. Ported from `assets/js/create.js` and
//! the bring-up part of `assets/js/sandboxes.js`; the comments there say why
//! each rule is as it is.
//!
//! The form lives here, in the core, not in the screen: the screen shows it
//! (`data.create`) and changes it one intent at a time. So what the core
//! sends, and what its confirmation lists, is what the core holds, never
//! words the screen made up.

use serde_json::{json, Value as J};
use std::time::{Duration, Instant};

/// Which dialog is open.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Dialog {
    /// "New sandbox": workspace or chat.
    Chooser,
    Workspace,
    Chat,
}

impl Dialog {
    pub fn name(self) -> &'static str {
        match self {
            Dialog::Chooser => "chooser",
            Dialog::Workspace => "workspace",
            Dialog::Chat => "chat",
        }
    }
    pub fn parse(s: &str) -> Option<Dialog> {
        Some(match s {
            "chooser" => Dialog::Chooser,
            "workspace" => Dialog::Workspace,
            "chat" => Dialog::Chat,
            _ => return None,
        })
    }
}

/// `GET /api/fs`: a folder and its sub-folders.
#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<(String, String)>,
}

impl Listing {
    pub fn from_json(v: &J) -> Option<Listing> {
        let path = v["path"].as_str()?.to_string();
        let parent = v["parent"].as_str().map(str::to_string);
        let entries = v["entries"]
            .as_array()
            .map(|a| a.iter().filter_map(|e| Some((e["name"].as_str()?.to_string(), e["path"].as_str()?.to_string()))).collect())
            .unwrap_or_default();
        Some(Listing { path, parent, entries })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Favourite {
    pub path: String,
    pub name: String,
    pub missing: bool,
}

pub fn favourites_from(v: &J) -> Vec<Favourite> {
    v["favourites"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|f| {
                    Some(Favourite { path: f["path"].as_str()?.to_string(), name: f["name"].as_str().unwrap_or("").to_string(), missing: f["missing"].as_bool().unwrap_or(false) })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One folder of the sandbox. The first is the workspace (always read-write).
#[derive(Debug, Clone, PartialEq)]
pub struct Repo {
    pub path: String,
    pub readonly: bool,
}

/// A restricted sub-folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Carve {
    pub path: String,
    /// `none` or `ro`.
    pub access: &'static str,
    /// `strict` or `soft`.
    pub enforce: &'static str,
}

/// Picking a restricted sub-folder: the browser stays inside `root`.
#[derive(Debug, Clone, PartialEq)]
pub struct CarvePick {
    pub root: String,
    pub cur: String,
    /// Where the browser was, to go back there.
    pub prev: String,
    pub typed: String,
    pub error: Option<String>,
}

/// The last `/api/carve/preview` answer for the current folders.
#[derive(Debug, Clone, PartialEq)]
pub enum Preview {
    /// Asked, no answer yet.
    Checking,
    /// The daemon has no such route (404) or did not answer.
    Unavailable,
    Answer(J),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PortRow {
    pub sandbox: String,
    pub host: String,
    pub alias: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KitMount {
    pub kit: String,
    pub target: String,
    pub host: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RemoteHost {
    pub name: String,
    pub address: String,
    /// `up`, `down`, `never`.
    pub link: String,
}

/// The "Rebuild the starting template" offer (`template = "auto"` only).
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateOffer {
    pub note: String,
    pub help: String,
}

/// `GET /api/template/status` → the offer, or none.
pub fn template_offer(v: &J) -> Option<TemplateOffer> {
    if v["auto"].as_bool() != Some(true) {
        return None;
    }
    let t = &v["template"];
    let state = t["state"].as_str()?;
    let tag = t["record"]["tag"].as_str();
    Some(if state == "fresh" {
        TemplateOffer {
            note: "Save this sandbox as the new starting point. Takes several extra minutes.".into(),
            help: format!("Replaces {}. The sandbox opens once its kits have installed and it is saved.", tag.unwrap_or("the template")),
        }
    } else {
        let changed: Vec<&str> = t["changed"].as_array().map(|a| a.iter().filter_map(J::as_str).collect()).unwrap_or_default();
        TemplateOffer {
            note: "Wait for this sandbox to be saved as the starting point. Left unticked, it opens now and the template is built in the background.".into(),
            help: match tag {
                Some(tag) => format!("{tag} is out of date ({}).", changed.join(", ")),
                None => "No template has been saved for these kits yet.".into(),
            },
        }
    })
}

pub const NAME_RULE: &str = "Lowercase letters, digits, and hyphens only; must start and end with a letter or digit";
pub const CARVE_STRICT: &str = "Strict: the folder never enters the sandbox. The parent is shared folder by folder, so files lying directly in it are not visible.";
pub const CARVE_SOFT: &str = "Soft: the parent is shared whole and the folder is covered. An agent using root can remove the cover, so this guards against mistakes only.";
/// How long after the last change the carve preview is asked for (create.js: 400 ms).
pub const PREVIEW_DELAY: Duration = Duration::from_millis(400);

/// What the name field keeps of what was typed: lowercase, `[a-z0-9-]`, no
/// leading hyphen; and whether anything had to go (the rule is then shown).
pub fn normalize_name(typed: &str) -> (String, bool) {
    let bad = typed.chars().any(|c| !(c.is_ascii_alphanumeric() || c == '-')) || typed.starts_with('-');
    let cleaned: String = typed.to_ascii_lowercase().chars().filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-').collect();
    (cleaned.trim_start_matches('-').to_string(), bad)
}

/// The daemon's sandbox name rule, `^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$`.
pub fn valid_name(n: &str) -> bool {
    let b = n.as_bytes();
    let ok = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit();
    !b.is_empty() && b.len() <= 63 && ok(b[0]) && ok(b[b.len() - 1]) && b.iter().all(|&c| ok(c) || c == b'-')
}

/// `a` is inside `b` (strictly).
pub fn inside(a: &str, b: &str) -> bool {
    let b = if b.ends_with('/') { b.to_string() } else { format!("{b}/") };
    a.starts_with(&b)
}

/// The form of the two create dialogs.
#[derive(Debug, Clone)]
pub struct Form {
    pub dialog: Option<Dialog>,
    /// `local`, or a remote host's name.
    pub host: String,
    pub hosts: Vec<RemoteHost>,
    pub name: String,
    pub name_note: Option<String>,
    pub chat_name: String,
    pub chat_note: Option<String>,
    pub chat_suggestion: String,
    /// The folder the browser shows; the workspace while no repo is added.
    pub selected: String,
    pub listing: Option<Listing>,
    pub listing_error: Option<String>,
    pub favourites: Vec<Favourite>,
    pub repos: Vec<Repo>,
    pub carves: Vec<Carve>,
    pub carve_pick: Option<CarvePick>,
    pub preview: Option<Preview>,
    /// What the shown preview was asked for.
    pub preview_sig: String,
    pub preview_due: Option<Instant>,
    /// Bumped by every new question; an answer for an older one is dropped.
    pub preview_seq: u64,
    pub ports: Vec<PortRow>,
    pub kit_mounts: Vec<KitMount>,
    pub template: Option<TemplateOffer>,
    pub rebuild: bool,
    pub remote_control: bool,
    pub chrome: bool,
    pub git_url: String,
    /// The last refusal of a call made from the dialog (picker, favourites...).
    pub error: Option<String>,
    /// Bumped by every browse; an older listing is dropped.
    pub browse_seq: u64,
}

impl Default for Form {
    fn default() -> Self {
        Form {
            dialog: None,
            host: "local".into(),
            hosts: vec![],
            name: String::new(),
            name_note: None,
            chat_name: String::new(),
            chat_note: None,
            chat_suggestion: String::new(),
            selected: String::new(),
            listing: None,
            listing_error: None,
            favourites: vec![],
            repos: vec![],
            carves: vec![],
            carve_pick: None,
            preview: None,
            preview_sig: String::new(),
            preview_due: None,
            preview_seq: 0,
            ports: vec![],
            kit_mounts: vec![],
            template: None,
            rebuild: false,
            remote_control: false,
            chrome: false,
            git_url: String::new(),
            error: None,
            browse_seq: 0,
        }
    }
}

impl Form {
    /// A fresh workspace form (the dialog opens empty each time, as the web UI's).
    pub fn reset_workspace(&mut self, last_host: &str) {
        let hosts = std::mem::take(&mut self.hosts);
        let favourites = std::mem::take(&mut self.favourites);
        *self = Form { hosts, favourites, ..Form::default() };
        self.host = if self.hosts.iter().any(|h| h.name == last_host) { last_host.to_string() } else { "local".into() };
        self.dialog = Some(Dialog::Workspace);
    }

    pub fn reset_chat(&mut self, suggestion: String) {
        self.chat_name.clear();
        self.chat_note = None;
        self.chat_suggestion = suggestion;
        self.error = None;
        self.dialog = Some(Dialog::Chat);
    }

    pub fn remote(&self) -> bool {
        self.host != "local"
    }

    /// The ref the sandbox will have: `host~name` off this machine.
    pub fn sandbox_ref(&self, short: &str) -> String {
        if self.remote() {
            format!("{}~{short}", self.host)
        } else {
            short.to_string()
        }
    }

    /// Changing host forgets what depended on the old one.
    pub fn set_host(&mut self, host: &str) {
        self.host = if host == "local" || self.hosts.iter().any(|h| h.name == host) { host.to_string() } else { "local".into() };
        self.git_url.clear();
        self.repos.clear();
        self.reset_carve();
        self.selected.clear();
        self.listing = None;
        if self.remote() {
            self.kit_mounts.clear();
        }
    }

    pub fn set_name(&mut self, typed: &str) {
        let (clean, bad) = normalize_name(typed);
        self.name = clean;
        self.name_note = bad.then(|| NAME_RULE.to_string());
    }

    pub fn set_chat_name(&mut self, typed: &str) {
        let (clean, bad) = normalize_name(typed);
        self.chat_name = clean;
        self.chat_note = bad.then(|| NAME_RULE.to_string());
    }

    /// Why the name cannot be used, or None. `taken` lists existing refs.
    pub fn name_problem(&self, name: &str, taken: &[String], empty_ok: bool) -> Option<String> {
        if name.is_empty() {
            return if empty_ok { None } else { Some("Give the sandbox a name.".into()) };
        }
        let r = self.sandbox_ref(name);
        if taken.iter().any(|t| *t == r) {
            return Some(format!("\"{r}\" already exists: pick another name"));
        }
        if !valid_name(name) {
            return Some(NAME_RULE.to_string());
        }
        None
    }

    // ---- folders ----

    /// A listing arrived for browse `seq`. Returns false when it is stale, or
    /// when a carve pick left its folder (the caller then browses to the root).
    pub fn take_listing(&mut self, seq: u64, l: Listing) -> Result<bool, String> {
        if seq != self.browse_seq {
            return Ok(false);
        }
        self.listing_error = None;
        if let Some(p) = &mut self.carve_pick {
            if l.path != p.root && !inside(&l.path, &p.root) {
                return Err(p.root.clone());
            }
            p.cur = l.path.clone();
        } else {
            self.selected = l.path.clone();
        }
        self.listing = Some(l);
        self.changed();
        Ok(true)
    }

    pub fn has_repo(&self, path: &str) -> bool {
        self.repos.iter().any(|r| r.path == path)
    }

    pub fn toggle_repo(&mut self, path: &str) {
        if self.has_repo(path) {
            self.repos.retain(|r| r.path != path);
        } else {
            self.repos.push(Repo { path: path.to_string(), readonly: true });
        }
        self.changed();
    }

    pub fn repo_main(&mut self, i: usize) {
        if i < self.repos.len() {
            let r = self.repos.remove(i);
            self.repos.insert(0, r);
            self.changed();
        }
    }

    pub fn repo_mode(&mut self, i: usize, readonly: bool) {
        if i > 0 {
            if let Some(r) = self.repos.get_mut(i) {
                r.readonly = readonly;
                self.changed();
            }
        }
    }

    pub fn repo_remove(&mut self, i: usize) {
        if i < self.repos.len() {
            self.repos.remove(i);
            self.changed();
        }
    }

    /// Two folders one inside the other: sbx shares a directory once.
    pub fn nested(&self) -> Option<(String, String)> {
        for a in &self.repos {
            for b in &self.repos {
                if a.path != b.path && inside(&a.path, &b.path) {
                    return Some((a.path.clone(), b.path.clone()));
                }
            }
        }
        None
    }

    pub fn main_path(&self) -> &str {
        self.repos.first().map(|r| r.path.as_str()).unwrap_or(&self.selected)
    }

    // ---- restricted sub-folders ----

    /// The folders a restriction can live in: the workspace and the RW extras.
    pub fn carve_parents(&self) -> Vec<String> {
        if !self.repos.is_empty() {
            return self.repos.iter().enumerate().filter(|(i, r)| *i == 0 || !r.readonly).map(|(_, r)| r.path.clone()).collect();
        }
        if self.selected.is_empty() {
            vec![]
        } else {
            vec![self.selected.clone()]
        }
    }

    pub fn carve_parent_of(&self, c: &str) -> Option<String> {
        self.carve_parents().into_iter().filter(|p| inside(c, p)).max_by_key(|p| p.len())
    }

    pub fn reset_carve(&mut self) {
        self.carves.clear();
        self.carve_pick = None;
        self.preview = None;
        self.preview_sig.clear();
        self.preview_due = None;
        self.preview_seq += 1;
    }

    /// Starts picking a sub-folder of `root`; the caller browses to it.
    pub fn carve_pick_start(&mut self, root: &str) -> bool {
        if !self.carve_parents().iter().any(|p| p == root) {
            return false;
        }
        self.carve_pick = Some(CarvePick { root: root.into(), cur: root.into(), prev: self.selected.clone(), typed: String::new(), error: None });
        true
    }

    /// Leaves picking; returns where to browse back to.
    pub fn carve_pick_end(&mut self) -> Option<String> {
        self.carve_pick.take().map(|p| p.prev)
    }

    /// Adds the folder being browsed, or the typed path. Returns where to
    /// browse back to when it was added.
    pub fn carve_add(&mut self, typed: bool) -> Option<String> {
        let p = self.carve_pick.as_mut()?;
        let path = if typed {
            let t = p.typed.trim().trim_end_matches('/').to_string();
            if !t.starts_with('/') || t.split('/').any(|s| s == "..") {
                p.error = Some("Type an absolute path, without \"..\".".into());
                return None;
            }
            if !inside(&t, &p.root) {
                p.error = Some(format!("The path must be inside {}.", p.root));
                return None;
            }
            t
        } else {
            if p.cur == p.root {
                return None;
            }
            p.cur.trim_end_matches('/').to_string()
        };
        if !self.carves.iter().any(|c| c.path == path) {
            self.carves.push(Carve { path, access: "none", enforce: "strict" });
        }
        let back = self.carve_pick_end();
        self.changed();
        back
    }

    pub fn carve_set(&mut self, i: usize, what: &str) {
        let Some(c) = self.carves.get_mut(i) else { return };
        match what {
            "none" | "ro" => c.access = if what == "none" { "none" } else { "ro" },
            "strict" | "soft" => c.enforce = if what == "strict" { "strict" } else { "soft" },
            "remove" => {
                self.carves.remove(i);
            }
            _ => return,
        }
        self.changed();
    }

    /// The preview refuses the restrictions: Create is held.
    pub fn carve_blocked(&self) -> bool {
        !self.carves.is_empty() && matches!(&self.preview, Some(Preview::Answer(v)) if v["ok"] == false)
    }

    /// What `/api/carve/preview` and the create body share.
    pub fn carve_body(&self) -> J {
        let mut b = json!({
            "path": self.main_path(),
            "mounts": self.repos.iter().skip(1).map(|r| json!({ "path": r.path, "readonly": r.readonly })).collect::<Vec<_>>(),
            "carve": self.carves.iter().map(|c| json!({ "path": c.path, "access": c.access, "enforce": c.enforce })).collect::<Vec<_>>(),
        });
        if self.remote() {
            b["host"] = self.host.clone().into();
        }
        b
    }

    /// After any change of folders or restrictions: drop restrictions whose
    /// folder left, and ask the preview again (debounced) if what it depends
    /// on moved.
    pub fn changed(&mut self) {
        let parents = self.carve_parents();
        self.carves.retain(|c| parents.iter().any(|p| inside(&c.path, p)));
        if self.carves.is_empty() {
            self.preview = None;
            self.preview_sig.clear();
            self.preview_due = None;
            return;
        }
        let sig = self.carve_body().to_string();
        if sig != self.preview_sig {
            self.preview_sig = sig;
            self.preview = Some(Preview::Checking);
            self.preview_due = Some(Instant::now() + PREVIEW_DELAY);
            self.preview_seq += 1;
        }
    }

    /// The preview to ask for now, if one is due: `(seq, body)`.
    pub fn preview_now(&mut self, now: Instant) -> Option<(u64, J)> {
        let due = self.preview_due?;
        if now < due {
            return None;
        }
        self.preview_due = None;
        Some((self.preview_seq, self.carve_body()))
    }

    pub fn take_preview(&mut self, seq: u64, p: Preview) {
        if seq == self.preview_seq && !self.carves.is_empty() {
            self.preview = Some(p);
        }
    }

    /// Files a strict restriction hides next to it (from the preview).
    pub fn carve_hidden_files(&self, c: &Carve) -> Vec<String> {
        let Some(Preview::Answer(v)) = &self.preview else { return vec![] };
        if c.enforce != "strict" {
            return vec![];
        }
        v["unshared"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(J::as_str)
                    .filter(|f| {
                        let dir = &f[..f.rfind('/').unwrap_or(0)];
                        c.path == dir || inside(&c.path, dir)
                    })
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The preview's one line, as create.js words it.
    pub fn preview_summary(&self) -> Option<String> {
        let p = self.preview.as_ref()?;
        Some(match p {
            Preview::Checking => "Checking...".into(),
            Preview::Unavailable => "Preview not available.".into(),
            Preview::Answer(v) if v["ok"] == false => format!("Preview: {}", v["error"].as_str().unwrap_or("error")),
            Preview::Answer(v) => {
                let n = |k: usize, one: &str, many: &str| format!("{k} {}", if k == 1 { one } else { many });
                let stub = v["stub"].as_bool().unwrap_or(false);
                let real = v["mounts"].as_array().map(|m| m.iter().enumerate().filter(|(k, m)| m["mask"] != true && !(stub && *k == 0)).count()).unwrap_or(0);
                let mut parts = vec![n(real, "folder mounted", "folders mounted")];
                let none = self.carves.iter().filter(|c| c.access == "none").count();
                let ro = self.carves.len() - none;
                if none > 0 {
                    parts.push(format!("{none} excluded"));
                }
                if ro > 0 {
                    parts.push(format!("{ro} read-only"));
                }
                let files = v["unshared"].as_array().map_or(0, |a| a.len());
                if files > 0 {
                    parts.push(n(files, "file not visible in the sandbox", "files not visible in the sandbox"));
                }
                parts.join(", ")
            }
        })
    }

    // ---- ports, kit folders ----

    pub fn set_port(&mut self, i: usize, field: &str, value: &str) {
        if let Some(p) = self.ports.get_mut(i) {
            let v = value.trim().to_string();
            match field {
                "sandbox" => p.sandbox = v,
                "host" => p.host = v,
                "alias" => p.alias = v,
                _ => {}
            }
        }
    }

    /// The ports to publish, as the create body takes them. A row whose
    /// sandbox port is not a number 1 to 65535 is left out, as the web UI
    /// does; one whose host port is not a number is refused.
    pub fn ports_body(&self) -> Result<Vec<J>, String> {
        let port = |s: &str| s.parse::<u32>().ok().filter(|p| (1..=65535).contains(p));
        let mut out = vec![];
        for p in &self.ports {
            let Some(sbx) = port(&p.sandbox) else {
                if p.sandbox.is_empty() {
                    continue;
                }
                return Err(format!("Port \"{}\" is not a number from 1 to 65535.", p.sandbox));
            };
            let mut e = json!({ "sandbox_port": sbx });
            if !p.host.is_empty() {
                e["host_port"] = port(&p.host).ok_or_else(|| format!("Port \"{}\" is not a number from 1 to 65535.", p.host))?.into();
            }
            if !p.alias.is_empty() {
                e["alias"] = p.alias.clone().into();
            }
            out.push(e);
        }
        Ok(out)
    }

    fn kit_mounts_body(&self) -> serde_json::Map<String, J> {
        if self.remote() {
            return Default::default();
        }
        self.kit_mounts.iter().filter(|k| !k.host.trim().is_empty()).map(|k| (k.target.clone(), J::from(k.host.trim()))).collect()
    }

    // ---- sending ----

    /// Create is pressable: a good free name, a folder (or a git URL on a
    /// remote host), no nested folders, no refused restriction, good ports.
    pub fn can_create(&self, taken: &[String]) -> bool {
        let path_ok = !self.repos.is_empty() || !self.selected.is_empty() || (self.remote() && !self.git_url.trim().is_empty());
        self.name_problem(&self.name, taken, false).is_none() && path_ok && self.nested().is_none() && !self.carve_blocked() && self.ports_body().is_ok()
    }

    /// `POST /api/sandboxes/create`'s body (`approved` added when sending).
    /// The path is empty when a git URL is to be cloned first.
    pub fn create_body(&self) -> Result<J, String> {
        let mut b = json!({
            "name": self.sandbox_ref(&self.name),
            "path": if self.remote() && !self.git_url.trim().is_empty() { "" } else { self.main_path() },
            "mounts": self.carve_body()["mounts"].clone(),
            "ports": self.ports_body()?,
            "template": if !self.remote() && self.template.is_some() && self.rebuild { "rebuild" } else { "default" },
            "remote_control": self.remote_control,
            "chrome": self.chrome,
            "kit_mounts": J::Object(self.kit_mounts_body()),
        });
        if !self.carves.is_empty() {
            b["carve"] = self.carve_body()["carve"].clone();
        }
        Ok(b)
    }

    /// What the sandbox will reach on the person's machine beyond its
    /// workspace, one plain line each. Empty: nothing to confirm.
    pub fn grants(&self) -> Vec<String> {
        let mut out = vec![];
        let on = if self.remote() { format!("on {}", self.host) } else { "on this machine".into() };
        for r in self.repos.iter().skip(1) {
            out.push(format!("Mount {} {on}, {}", r.path, if r.readonly { "read-only" } else { "read-write (the agent may change it)" }));
        }
        for k in &self.kit_mounts {
            if !self.remote() && !k.host.trim().is_empty() {
                out.push(format!("Share {} with the sandbox at {} (kit {})", k.host.trim(), k.target, k.kit));
            }
        }
        if let Ok(ports) = self.ports_body() {
            for p in ports {
                let host = p.get("host_port").unwrap_or(&p["sandbox_port"]);
                let alias = p["alias"].as_str().map(|a| format!(" as http://{a}")).unwrap_or_default();
                out.push(format!("Publish sandbox port {} on port {host} {on}{alias}", p["sandbox_port"]));
            }
        }
        if self.chrome {
            out.push("Let the agent open and read tabs in your browser (Claude in Chrome)".into());
        }
        out
    }

    /// The confirmation's first line: what the sandbox is.
    pub fn summary(&self) -> String {
        let what = if self.remote() && !self.git_url.trim().is_empty() {
            format!("a clone of {} on {}", self.git_url.trim(), self.host)
        } else {
            format!("{} read-write", self.main_path())
        };
        format!("\"{}\" works in {what}.", self.sandbox_ref(&self.name))
    }
}

/// A host-access refusal: `409 {consent: {config, grants}}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Consent {
    pub config: String,
    pub grants: Vec<String>,
}

pub fn consent_of(v: &J) -> Option<Consent> {
    let c = v.get("consent")?;
    let grants: Vec<String> = c["grants"].as_array()?.iter().map(|g| g.as_str().map(str::to_string).unwrap_or_else(|| g.to_string())).collect();
    Some(Consent { config: c["config"].as_str().unwrap_or("sbxw.toml").to_string(), grants })
}

/// What a bring-up's answer means for the next step.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    Ready { name: String, warnings: Vec<String> },
    /// Ask the person about these lines, then send again with them.
    AskConsent(Consent),
    Failed(String),
}

/// Reads `POST /api/sandboxes/create|chat`'s answer (`status`, body), given
/// the lines already approved: the same lines refused twice are an error.
pub fn read_answer(status: u16, v: &J, asked_name: &str, approved: &[String]) -> Answer {
    if (200..300).contains(&status) && v["ok"] != false {
        let warnings = v["warnings"].as_array().map(|a| a.iter().filter_map(J::as_str).map(str::to_string).collect()).unwrap_or_default();
        return Answer::Ready { name: v["name"].as_str().unwrap_or(asked_name).to_string(), warnings };
    }
    if let Some(c) = consent_of(v) {
        if c.grants != approved {
            return Answer::AskConsent(c);
        }
    }
    Answer::Failed(v["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {status}")))
}

// ---------------------------------------------------------------------------
// Bring-ups in progress

#[derive(Debug, Clone, PartialEq)]
pub enum JobState {
    /// Waiting for the daemon (or for the person's answer to a question).
    Running,
    /// Held for the person's answer about host access (the core's dialog).
    Asking(String),
    Ready,
    Failed(String),
}

/// One bring-up, from the click (or the first `provision` event of one
/// another client started) until the person dismisses it.
#[derive(Debug, Clone)]
pub struct Job {
    pub sandbox: String,
    /// `create` or `chat`; none for one this app did not start.
    pub route: Option<&'static str>,
    /// What was sent, to send again on Retry.
    pub body: Option<J>,
    /// A git URL to clone on `host` first, then send `body` with its path.
    pub clone: Option<(String, String)>,
    pub steps: Vec<(String, String)>,
    /// The running step's index; -1 before the first.
    pub at: i64,
    pub detail: String,
    pub warnings: Vec<String>,
    pub state: JobState,
    /// This app is waiting for its POST's answer.
    pub posting: bool,
    pub since: Instant,
    /// Where the bring-up's time went, as the daemon's `done` event says it
    /// (`ready in 41 s: create 28 s, boot 6 s, ...`); none from an older daemon.
    pub summary: Option<String>,
}

impl Job {
    pub fn new(sandbox: &str, route: Option<&'static str>, body: Option<J>) -> Job {
        Job {
            sandbox: sandbox.into(),
            route,
            body,
            clone: None,
            steps: vec![],
            at: -1,
            detail: String::new(),
            warnings: vec![],
            state: JobState::Running,
            posting: route.is_some(),
            since: Instant::now(),
            summary: None,
        }
    }

    /// Ready to try again with the same body.
    pub fn restart(&mut self) {
        self.steps.clear();
        self.at = -1;
        self.detail.clear();
        self.warnings.clear();
        self.summary = None;
        self.state = JobState::Running;
        self.posting = true;
        self.since = Instant::now();
    }
}

/// Applies one `provision` event (`{sandbox, kind, ...}`): creates the job
/// when this app did not start it. Returns the sandbox it was about.
pub fn apply_provision(jobs: &mut Vec<Job>, data: &str) -> Option<String> {
    let ev: J = serde_json::from_str(data).ok()?;
    let name = ev["sandbox"].as_str()?.to_string();
    let kind = ev["kind"].as_str()?;
    let idx = match jobs.iter().position(|j| j.sandbox == name) {
        Some(i) => i,
        None if kind == "done" => return Some(name),
        None => {
            jobs.push(Job::new(&name, None, None));
            jobs.len() - 1
        }
    };
    let j = &mut jobs[idx];
    match kind {
        "plan" => {
            j.steps = ev["steps"].as_array().map(|a| a.iter().filter_map(|s| Some((s["id"].as_str()?.to_string(), s["label"].as_str().unwrap_or("").to_string()))).collect()).unwrap_or_default();
            j.at = -1;
            j.detail.clear();
            if j.state != JobState::Running && !j.posting {
                j.state = JobState::Running;
            }
        }
        "started" => {
            if let Some(i) = j.steps.iter().position(|(id, _)| ev["id"] == id.as_str()) {
                j.at = i as i64;
            }
            j.detail.clear();
        }
        "detail" => j.detail = ev["text"].as_str().unwrap_or("").to_string(),
        "warning" => {
            if let Some(t) = ev["text"].as_str() {
                j.warnings.push(t.to_string());
            }
        }
        "done" => {
            // The daemon's timing summary, plain text, whoever posted it.
            j.summary = ev["summary"].as_str().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
            // A job this app posted ends with its POST's answer (which says
            // more); one it only watches ends here.
            if !j.posting {
                j.state = match ev["error"].as_str() {
                    Some(e) => JobState::Failed(e.to_string()),
                    None => JobState::Ready,
                };
                if j.route.is_none() && j.state == JobState::Ready {
                    jobs.remove(idx);
                }
            }
        }
        _ => {}
    }
    Some(name)
}

/// One job for the screen.
pub fn job_view(j: &Job) -> J {
    let steps: Vec<J> = j
        .steps
        .iter()
        .enumerate()
        .map(|(i, (_, label))| {
            let i = i as i64;
            let state = if j.state == JobState::Ready || i < j.at {
                "done"
            } else if i == j.at {
                if matches!(j.state, JobState::Failed(_)) {
                    "failed"
                } else {
                    "running"
                }
            } else {
                "waiting"
            };
            json!({ "label": label, "state": state, "detail": if state == "running" && !j.detail.is_empty() { J::from(j.detail.clone()) } else { J::Null } })
        })
        .collect();
    let (state, error) = match &j.state {
        JobState::Running => ("running", None),
        JobState::Asking(why) => ("asking", Some(why.clone())),
        JobState::Ready => ("ready", None),
        JobState::Failed(e) => ("failed", Some(e.clone())),
    };
    let headline = match &j.state {
        JobState::Running if j.at >= 0 && !j.steps.is_empty() => format!("Creating... {}/{}", j.at + 1, j.steps.len()),
        JobState::Running => "Creating...".into(),
        JobState::Asking(_) => "Waiting for you".into(),
        JobState::Ready => "Ready".into(),
        JobState::Failed(_) => "Not created".into(),
    };
    json!({
        "sandbox": j.sandbox,
        "state": state,
        "headline": headline,
        "steps": steps,
        "error": error,
        "warnings": j.warnings,
        "can_retry": matches!(j.state, JobState::Failed(_) | JobState::Asking(_)) && j.body.is_some(),
        "summary": if j.state == JobState::Ready { j.summary.clone().map(J::from).unwrap_or(J::Null) } else { J::Null },
    })
}

// ---------------------------------------------------------------------------
// Hosts sbx denied during a kit install (sbx 0.54; the `denied` progress
// event, `sbxw_core::progress::Progress::Denied`)

/// One failed kit install and the hosts sbx denied, kept after its bring-up
/// ended (a kit add that fails on an existing sandbox still ends "ready")
/// until it is retried or dismissed.
#[derive(Debug, Clone, PartialEq)]
pub struct Denied {
    pub sandbox: String,
    /// `create` (`sbx create` failed) or `kits` (`sbx kit add`).
    pub step: String,
    pub kit: Option<String>,
    pub hosts: Vec<String>,
    /// `sbx` (named in sbx's message) or `policy_log`.
    pub source: String,
    /// `sandbox`, or `global` when the sandbox does not exist (the rule is then every sandbox's).
    pub allow_scope: String,
    /// sbx's words, as the daemon clipped them.
    pub message: String,
    /// Hosts an allow rule was added for since (this app's own calls).
    pub allowed: Vec<String>,
    /// The bring-up to send again on Retry (create's route and body).
    pub route: Option<&'static str>,
    pub body: Option<J>,
    pub clone: Option<(String, String)>,
}

/// A host as an allow rule takes it: a name of letters, digits, `-` and `.`
/// (the daemon's are already so; anything else from an event is dropped).
fn host_ok(h: &str) -> bool {
    !h.is_empty() && h.len() <= 253 && !h.starts_with(['-', '.']) && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
}

impl Denied {
    /// From a `denied` event; None for another kind, or no usable host.
    pub fn from_event(ev: &J) -> Option<Denied> {
        if ev["kind"] != "denied" {
            return None;
        }
        let mut hosts: Vec<String> = Vec::new();
        for h in ev["hosts"].as_array()?.iter().filter_map(J::as_str).map(str::trim) {
            if host_ok(h) && !hosts.iter().any(|x| x == h) {
                hosts.push(h.to_string());
            }
        }
        if hosts.is_empty() {
            return None;
        }
        let exists = ev["exists"] != false;
        // The scope follows the daemon's word, and never says "this sandbox"
        // for one that does not exist.
        let allow_scope = if !exists || ev["allow_scope"] == "global" { "global" } else { "sandbox" };
        Some(Denied {
            sandbox: ev["sandbox"].as_str()?.to_string(),
            step: ev["step"].as_str().unwrap_or("kits").to_string(),
            kit: ev["kit"].as_str().map(str::to_string),
            hosts,
            source: ev["source"].as_str().unwrap_or("sbx").to_string(),
            allow_scope: allow_scope.to_string(),
            message: ev["message"].as_str().unwrap_or("").to_string(),
            allowed: vec![],
            route: None,
            body: None,
            clone: None,
        })
    }

    pub fn view(&self) -> J {
        let hosts: Vec<J> = self.hosts.iter().map(|h| json!({ "host": h, "allowed": self.allowed.contains(h) })).collect();
        json!({
            "sandbox": self.sandbox,
            "step": self.step,
            "kit": self.kit,
            "hosts": hosts,
            "left": self.hosts.iter().filter(|h| !self.allowed.contains(h)).count(),
            "source": self.source,
            "allow_scope": self.allow_scope,
            "message": self.message,
            "can_retry": self.body.is_some(),
        })
    }
}

/// How a bring-up stands in the status bar (shell/statusbar.rs).
#[derive(Debug, Clone, PartialEq)]
pub enum BarState {
    Running,
    /// Held for the person's answer about host access.
    Asking,
    Failed(String),
    /// Ready a moment ago (the bar says "created", then clears).
    Created,
}

/// One bring-up as the status bar shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct BarJob {
    pub sandbox: String,
    /// What runs now: the step's label, or the words of its detail line
    /// with their percentage ("Pulling image 63%"). None before the plan.
    pub step: Option<String>,
    /// Done out of the plan, 0 to 1; a running step's percentage counts in.
    pub frac: f32,
    pub state: BarState,
    /// When it started (the most recent one shows first).
    pub since: Instant,
}

/// The last percentage in a detail line ("... 63%", "63.5%"), 0 to 100.
pub fn percent(t: &str) -> Option<f32> {
    t.split_whitespace().rev().find_map(|w| {
        let n = w.trim_end_matches(|c: char| c == ')' || c == ',' || c == '.').strip_suffix('%')?;
        n.parse::<f32>().ok().filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 100.0))
    })
}

/// A step's words for the bar: with a percentage in its detail, the
/// detail's first plain words (an image reference or a path is left out)
/// and the percentage, else the step's label.
pub fn step_words(label: &str, detail: &str) -> String {
    let Some(p) = percent(detail) else { return label.to_string() };
    let words: Vec<&str> = detail.split_whitespace().take_while(|w| !w.contains(['/', ':', '%', '@']) && !w.chars().any(|c| c.is_ascii_digit())).take(3).collect();
    let head = if words.is_empty() { label.to_string() } else { words.join(" ") };
    format!("{head} {:.0}%", p.floor())
}

/// A job for the bar (none once ready: the bar says "created" from its own list).
pub fn bar_job(j: &Job) -> Option<BarJob> {
    let total = j.steps.len();
    let done = j.at.max(0) as usize;
    let running = j.steps.get(j.at.max(0) as usize).filter(|_| j.at >= 0);
    let within = if matches!(j.state, JobState::Running) { percent(&j.detail).map_or(0.0, |p| p / 100.0) } else { 0.0 };
    let frac = if total == 0 { 0.0 } else { ((done as f32 + within) / total as f32).clamp(0.0, 1.0) };
    let state = match &j.state {
        JobState::Running => BarState::Running,
        JobState::Asking(_) => BarState::Asking,
        JobState::Failed(e) => BarState::Failed(e.clone()),
        JobState::Ready => return None,
    };
    let step = match &state {
        BarState::Asking => Some("Waiting for you".to_string()),
        BarState::Failed(_) => Some(match running {
            Some((_, l)) => format!("{l} failed"),
            None => "Not created".to_string(),
        }),
        _ => running.map(|(_, l)| step_words(l, &j.detail)),
    };
    Some(BarJob { sandbox: j.sandbox.clone(), step, frac, state, since: j.since })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(path: &str, parent: Option<&str>, kids: &[&str]) -> Listing {
        Listing { path: path.into(), parent: parent.map(str::to_string), entries: kids.iter().map(|k| (k.to_string(), format!("{path}/{k}"))).collect() }
    }

    #[test]
    fn the_name_is_cleaned_as_typed_and_checked_like_the_daemon() {
        assert_eq!(normalize_name("My_App"), ("myapp".into(), true));
        assert_eq!(normalize_name("-web"), ("web".into(), true));
        assert_eq!(normalize_name("web-2"), ("web-2".into(), false));
        assert!(valid_name("a") && valid_name("web-2") && !valid_name("web-") && !valid_name("") && !valid_name(&"a".repeat(64)));
        let mut f = Form::default();
        let taken = vec!["docs".to_string(), "box~web".to_string()];
        assert_eq!(f.name_problem("", &taken, false).as_deref(), Some("Give the sandbox a name."));
        assert_eq!(f.name_problem("", &taken, true), None, "a chat's name may be left empty");
        assert!(f.name_problem("docs", &taken, false).unwrap().contains("already exists"));
        assert_eq!(f.name_problem("web", &taken, false), None);
        f.hosts = vec![RemoteHost { name: "box".into(), address: "box.lan".into(), link: "up".into() }];
        f.set_host("box");
        assert!(f.name_problem("web", &taken, false).unwrap().contains("\"box~web\" already exists"));
        assert_eq!(f.name_problem("web-", &taken, false).as_deref(), Some(NAME_RULE));
    }

    #[test]
    fn folders_repos_and_the_body() {
        let mut f = Form::default();
        f.reset_workspace("local");
        f.set_name("shop");
        assert!(!f.can_create(&[]), "no folder yet");
        f.browse_seq = 1;
        assert_eq!(f.take_listing(1, listing("/u/dev/shop", Some("/u/dev"), &["backend", "frontend"])), Ok(true));
        assert_eq!(f.take_listing(0, listing("/u", None, &[])), Ok(false), "a stale listing is dropped");
        assert!(f.can_create(&[]));
        assert!(f.grants().is_empty(), "the workspace alone asks nothing");
        f.toggle_repo("/u/dev/shop/backend");
        f.toggle_repo("/u/dev/shop/frontend");
        assert_eq!(f.main_path(), "/u/dev/shop/backend");
        f.repo_mode(1, false);
        let b = f.create_body().unwrap();
        assert_eq!(b["path"], "/u/dev/shop/backend");
        assert_eq!(b["mounts"], json!([{ "path": "/u/dev/shop/frontend", "readonly": false }]));
        assert!(b.get("carve").is_none(), "nothing is sent without restrictions");
        assert_eq!(b["template"], "default");
        assert_eq!(f.grants(), vec!["Mount /u/dev/shop/frontend on this machine, read-write (the agent may change it)".to_string()]);
        f.toggle_repo("/u/dev/shop");
        assert_eq!(f.nested(), Some(("/u/dev/shop/backend".into(), "/u/dev/shop".into())));
        assert!(!f.can_create(&[]), "nested folders hold Create");
        f.repo_remove(2);
        f.repo_main(1);
        assert_eq!(f.repos[0].path, "/u/dev/shop/frontend");
    }

    #[test]
    fn ports_kit_folders_and_chrome_are_listed_for_the_confirmation() {
        let mut f = Form::default();
        f.reset_workspace("local");
        f.set_name("web");
        f.selected = "/w".into();
        f.ports = vec![PortRow { sandbox: "3000".into(), host: "".into(), alias: "app.local".into() }, PortRow::default()];
        f.kit_mounts = vec![KitMount { kit: "sbxw:ssh".into(), target: "/home/agent/.ssh".into(), host: "/u/.ssh".into() }];
        f.chrome = true;
        let b = f.create_body().unwrap();
        assert_eq!(b["ports"], json!([{ "sandbox_port": 3000, "alias": "app.local" }]));
        assert_eq!(b["kit_mounts"], json!({ "/home/agent/.ssh": "/u/.ssh" }));
        let g = f.grants();
        assert_eq!(g.len(), 3, "{g:?}");
        assert!(g[0].contains("/u/.ssh") && g[1].contains("port 3000") && g[1].contains("http://app.local") && g[2].contains("Chrome"));
        f.ports[1].sandbox = "99999".into();
        assert!(f.create_body().unwrap_err().contains("99999"));
        assert!(!f.can_create(&[]));
    }

    #[test]
    fn restrictions_follow_their_folders_and_the_preview() {
        let mut f = Form::default();
        f.reset_workspace("local");
        f.selected = "/w".into();
        assert!(f.carve_pick_start("/w"));
        assert!(!f.carve_pick_start("/elsewhere"));
        f.browse_seq = 5;
        assert_eq!(f.take_listing(5, listing("/w/secret", Some("/w"), &[])), Ok(true));
        assert_eq!(f.selected, "/w", "picking does not move the workspace");
        assert_eq!(f.take_listing(5, listing("/", None, &[])), Err("/w".into()), "the pick cannot leave its folder");
        assert_eq!(f.carve_add(false), Some("/w".into()));
        assert_eq!(f.carves, vec![Carve { path: "/w/secret".into(), access: "none", enforce: "strict" }]);
        assert_eq!(f.preview, Some(Preview::Checking));
        let (seq, body) = f.preview_now(Instant::now() + PREVIEW_DELAY).unwrap();
        assert_eq!(body["carve"][0]["path"], "/w/secret");
        f.take_preview(seq, Preview::Answer(json!({ "ok": true, "mounts": [{ "mask": false }, { "mask": false }], "unshared": ["/w/notes.txt"], "stub": false })));
        assert_eq!(f.preview_summary().unwrap(), "2 folders mounted, 1 excluded, 1 file not visible in the sandbox");
        assert_eq!(f.carve_hidden_files(&f.carves[0]), vec!["/w/notes.txt".to_string()]);
        f.carve_set(0, "soft");
        assert!(f.carve_hidden_files(&f.carves[0]).is_empty(), "soft hides nothing next to it");
        assert_eq!(f.preview, Some(Preview::Checking), "a change asks again");
        let (seq2, _) = f.preview_now(Instant::now() + PREVIEW_DELAY).unwrap();
        f.take_preview(seq, Preview::Unavailable);
        assert_eq!(f.preview, Some(Preview::Checking), "an old answer is dropped");
        f.take_preview(seq2, Preview::Answer(json!({ "ok": false, "error": "cannot mount" })));
        assert!(f.carve_blocked());
        assert_eq!(f.create_body().unwrap()["carve"], json!([{ "path": "/w/secret", "access": "none", "enforce": "soft" }]));
        // Typed paths.
        f.carve_pick_start("/w");
        f.carve_pick.as_mut().unwrap().typed = "/w/../etc".into();
        assert_eq!(f.carve_add(true), None);
        assert!(f.carve_pick.as_ref().unwrap().error.as_deref().unwrap().contains("absolute"));
        f.carve_pick.as_mut().unwrap().typed = "/x".into();
        f.carve_add(true);
        assert!(f.carve_pick.as_ref().unwrap().error.as_deref().unwrap().contains("inside /w"));
        f.carve_pick.as_mut().unwrap().typed = "/w/keys/".into();
        f.carve_add(true);
        assert_eq!(f.carves[1].path, "/w/keys");
        // The workspace changes: its restrictions go.
        f.selected = "/v".into();
        f.changed();
        assert!(f.carves.is_empty() && f.preview.is_none());
    }

    #[test]
    fn answers_consent_and_progress() {
        let ok = read_answer(200, &json!({ "ok": true, "warnings": ["alias needs sudo"] }), "web", &[]);
        assert_eq!(ok, Answer::Ready { name: "web".into(), warnings: vec!["alias needs sudo".into()] });
        let c = json!({ "ok": false, "error": "needs approval", "consent": { "config": "/w/sbxw.toml", "grants": ["mount /etc"] } });
        assert_eq!(read_answer(409, &c, "web", &[]), Answer::AskConsent(Consent { config: "/w/sbxw.toml".into(), grants: vec!["mount /etc".into()] }));
        assert_eq!(read_answer(409, &c, "web", &["mount /etc".into()]), Answer::Failed("needs approval".into()), "the same lines twice: an error");
        assert_eq!(read_answer(502, &json!({ "ok": false, "error": "sbx: no space left" }), "web", &[]), Answer::Failed("sbx: no space left".into()));
        assert_eq!(read_answer(200, &json!({ "ok": true, "name": "chat-1a2b3c" }), "", &[]), Answer::Ready { name: "chat-1a2b3c".into(), warnings: vec![] });

        let mut jobs = vec![Job::new("web", Some("create"), Some(json!({})))];
        apply_provision(&mut jobs, r#"{"sandbox":"web","kind":"plan","steps":[{"id":"create","label":"Create"},{"id":"kits","label":"Install kits"}]}"#);
        apply_provision(&mut jobs, r#"{"sandbox":"web","kind":"started","id":"kits"}"#);
        apply_provision(&mut jobs, r#"{"sandbox":"web","kind":"detail","text":"pulling 40%"}"#);
        let v = job_view(&jobs[0]);
        assert_eq!(v["headline"], "Creating... 2/2");
        assert_eq!(v["steps"][0]["state"], "done");
        assert_eq!(v["steps"][1]["detail"], "pulling 40%");
        apply_provision(&mut jobs, r#"{"sandbox":"web","kind":"done","error":"boom"}"#);
        assert_eq!(job_view(&jobs[0])["summary"], J::Null, "no summary while not ready");
        assert_eq!(jobs[0].state, JobState::Running, "a posted job waits for its answer");
        jobs[0].posting = false;
        jobs[0].state = JobState::Failed("sbx: boom".into());
        let v = job_view(&jobs[0]);
        assert_eq!((v["state"].as_str(), v["can_retry"].as_bool(), v["steps"][1]["state"].as_str()), (Some("failed"), Some(true), Some("failed")));
        // Another client's bring-up is shown too, and goes when done.
        apply_provision(&mut jobs, r#"{"sandbox":"other","kind":"plan","steps":[]}"#);
        assert_eq!(jobs.len(), 2);
        apply_provision(&mut jobs, r#"{"sandbox":"other","kind":"done"}"#);
        assert_eq!(jobs.len(), 1);
    }

    /// The daemon's timing summary on `done` is kept as plain text and
    /// shown once ready; an older daemon's `done` has none, and none is shown.
    #[test]
    fn the_done_summary_is_shown_when_ready_and_only_when_sent() {
        let mut jobs = vec![Job::new("web", Some("create"), Some(json!({})))];
        apply_provision(&mut jobs, r#"{"sandbox":"web","kind":"done","error":null,"summary":"ready in 41 s: create 28 s, boot 6 s, tooling 3 s"}"#);
        jobs[0].posting = false;
        jobs[0].state = JobState::Ready;
        assert_eq!(job_view(&jobs[0])["summary"], "ready in 41 s: create 28 s, boot 6 s, tooling 3 s");
        // Retry forgets it until the next end.
        jobs[0].restart();
        assert_eq!(jobs[0].summary, None);
        // An older daemon: no field, nothing shown.
        let mut old = vec![Job::new("docs", Some("create"), Some(json!({})))];
        apply_provision(&mut old, r#"{"sandbox":"docs","kind":"done","error":null}"#);
        old[0].posting = false;
        old[0].state = JobState::Ready;
        assert_eq!(job_view(&old[0])["summary"], J::Null);
        // Markup stays text: the view carries the daemon's string as is.
        let mut m = vec![Job::new("m", Some("create"), Some(json!({})))];
        apply_provision(&mut m, r#"{"sandbox":"m","kind":"done","summary":"<b>ready</b> in 1 s"}"#);
        m[0].posting = false;
        m[0].state = JobState::Ready;
        assert_eq!(job_view(&m[0])["summary"], "<b>ready</b> in 1 s");
    }

    #[test]
    fn the_template_offer() {
        assert_eq!(template_offer(&json!({ "auto": false })), None);
        let o = template_offer(&json!({ "auto": true, "template": { "state": "stale", "record": { "tag": "sbxw-auto:1" }, "changed": ["sbx 0.52 → 0.53"] } })).unwrap();
        assert!(o.help.contains("sbxw-auto:1 is out of date (sbx 0.52 → 0.53)"));
        let o = template_offer(&json!({ "auto": true, "template": { "state": "missing" } })).unwrap();
        assert!(o.help.contains("No template"));
    }
}
