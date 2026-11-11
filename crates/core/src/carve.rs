//! Carves: a folder inside a mounted folder that the sandbox cannot read, or
//! can only read.
//!
//! sbx mounts whole folders, so a carve is made out of mounts. Two ways:
//!
//! * **soft**: the parent stays mounted as it is, and a second mount goes on
//!   top at the carve's place (an empty read-only folder for `none`, the
//!   folder itself read-only for `ro`). An agent with root can unmount it, so
//!   it guards against mistakes, not against a hostile agent.
//! * **strict**: the parent is not mounted as a whole. Every child folder on
//!   the way to the carve is mounted on its own, so the carved folder never
//!   enters the VM. The cost: files and other non-folder entries beside the
//!   mounted folders cannot be shared, and nothing can be created at those
//!   levels.
//!
//! [`plan`] decides which mounts that makes. It does no I/O: directory
//! listings come in through a callback, and the two helper folders (the stub
//! and the mask) are paths the caller has made.

use crate::config::{Carve, CarveAccess, CarveEnforce, Workspace};
use std::collections::BTreeMap;

/// What sbx will take at most (the planner refuses more).
pub const MAX_MOUNTS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Dir,
    File,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
}

impl Entry {
    pub fn new(name: &str, kind: Kind) -> Self {
        Entry { name: name.into(), kind }
    }
}

/// One mount sbx is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub host: String,
    /// Where it appears in the sandbox (equal to `host` unless an extra
    /// folder was given a `TARGET`).
    pub target: String,
    pub readonly: bool,
    /// An empty folder laid over something to hide it.
    pub mask: bool,
}

impl Mount {
    /// `PATH[:TARGET][:ro]`, the argument `sbx create` takes.
    pub fn spec(&self) -> String {
        let mut s = self.host.clone();
        if self.target != self.host {
            s.push(':');
            s.push_str(&self.target);
        }
        if self.readonly {
            s.push_str(":ro");
        }
        s
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The first mount: the main workspace as it is, or the stub when the
    /// main workspace is split.
    pub primary: Mount,
    /// The other mounts, in the order sbx must get them (a mount laid over
    /// another comes after it).
    pub mounts: Vec<Mount>,
    /// The absolute folder the agent should start in, when the main
    /// workspace is split (the primary is then the stub).
    pub workdir: Option<String>,
    /// Host entries that will not be visible in the sandbox: the files,
    /// symlinks and other non-folders at every split level.
    pub unshared: Vec<String>,
    pub warnings: Vec<String>,
    /// The primary is the stub, not the real workspace.
    pub stub: bool,
}

impl Plan {
    /// `sbx create`'s workspace argument.
    pub fn primary_spec(&self) -> String {
        self.primary.spec()
    }
    /// The extra specs, in order.
    pub fn specs(&self) -> Vec<String> {
        self.mounts.iter().map(Mount::spec).collect()
    }
    /// Every mount, the primary first.
    pub fn all_mounts(&self) -> impl Iterator<Item = &Mount> {
        std::iter::once(&self.primary).chain(self.mounts.iter())
    }
}

/// A host directory's entries, or why it cannot be listed.
pub type Lister<'a> = &'a dyn Fn(&str) -> Result<Vec<Entry>, String>;

fn parts(p: &str) -> Vec<&str> {
    p.split('/').filter(|c| !c.is_empty()).collect()
}

fn join(base: &str, rel: &[&str]) -> String {
    let mut s = base.trim_end_matches('/').to_string();
    for r in rel {
        s.push('/');
        s.push_str(r);
    }
    if s.is_empty() {
        "/".into()
    } else {
        s
    }
}

fn is_under(path: &[&str], root: &[&str]) -> bool {
    path.len() > root.len() && path[..root.len()] == *root
}

#[derive(Default)]
struct Node {
    access: Option<CarveAccess>,
    kids: BTreeMap<String, Node>,
}

struct Root<'a> {
    ws: &'a Workspace,
    host: Vec<&'a str>,
    /// Indices into the merged carve list, soft and strict.
    soft: Vec<usize>,
    strict: Vec<usize>,
}

impl Root<'_> {
    fn target_base(&self) -> &str {
        self.ws.target.as_deref().unwrap_or(&self.ws.path)
    }
}

/// Decide the mounts for `main`, the `extras` and the `carves`.
///
/// * Carve paths are absolute host paths, **resolved on the host by the
///   caller** (symlinks followed, `..` gone): the planner compares names and
///   never follows a symlink entry, which it reports as unshared.
/// * `list` gives a host folder's entries; it is called only for the folders
///   a strict carve splits.
/// * `stub` is an empty host folder for the primary when the main workspace
///   is split (sbx needs one). `mask` is an empty folder to lay read-only
///   over a soft `none` carve.
/// * `workdir` is the config's `workdir` (relative to the main workspace, or
///   absolute); it is used only when the main workspace is split.
pub fn plan(
    main: &Workspace,
    extras: &[Workspace],
    carves: &[Carve],
    list: Lister,
    stub: &str,
    mask: &str,
    workdir: &str,
) -> Result<Plan, String> {
    let carves = merge(carves)?;
    let mut roots: Vec<Root> = std::iter::once(main)
        .chain(extras.iter())
        .map(|ws| Root { ws, host: parts(&ws.path), soft: vec![], strict: vec![] })
        .collect();

    for (i, c) in carves.iter().enumerate() {
        let cp = parts(&c.path);
        if cp.is_empty() || cp.contains(&"..") || cp.contains(&".") {
            return Err(format!("carve {}: expected an absolute path with no `.` or `..`", c.path));
        }
        let inside: Vec<usize> = (0..roots.len()).filter(|&r| is_under(&cp, &roots[r].host)).collect();
        match inside.as_slice() {
            [r] => match c.enforce {
                CarveEnforce::Soft => roots[*r].soft.push(i),
                CarveEnforce::Strict => roots[*r].strict.push(i),
            },
            [] => {
                let equal = roots.iter().any(|r| r.host == cp);
                return Err(if equal {
                    format!("carve {}: that is a mounted folder itself; take it out of the workspaces instead", c.path)
                } else {
                    format!("carve {}: not inside the main workspace or a workspaces folder", c.path)
                });
            }
            many => {
                let names: Vec<&str> = many.iter().map(|&r| roots[r].ws.path.as_str()).collect();
                return Err(format!("carve {}: inside more than one mounted folder ({}); which one it belongs to is unclear", c.path, names.join(", ")));
            }
        }
    }

    let mut warnings = vec![];
    let mut unshared = vec![];
    let mut main_mounts: Vec<Mount> = vec![];
    let mut extra_mounts: Vec<Mount> = vec![];
    let mut split_main = false;
    let mut per_root: Vec<usize> = vec![];

    for (ri, root) in roots.iter().enumerate() {
        let out = if ri == 0 { &mut main_mounts } else { &mut extra_mounts };
        let before = out.len();
        let tb = root.target_base();
        let rel_of = |c: &Carve| -> Vec<String> {
            parts(&c.path)[root.host.len()..].iter().map(|s| s.to_string()).collect()
        };
        let split = !root.strict.is_empty();
        let mut children: Vec<Mount> = vec![];
        if split {
            if ri == 0 {
                split_main = true;
            }
            let mut trie = Node::default();
            for &i in &root.strict {
                let mut n = &mut trie;
                for comp in rel_of(&carves[i]) {
                    n = n.kids.entry(comp).or_default();
                }
                n.access = Some(carves[i].access);
            }
            for &s in &root.soft {
                let sp = parts(&carves[s].path);
                if root.strict.iter().any(|&t| is_under(&parts(&carves[t].path), &sp)) {
                    return Err(format!(
                        "carve {} (soft) holds a strict carve; make it strict, or move the strict one out",
                        carves[s].path
                    ));
                }
            }
            let before_unshared = unshared.len();
            walk(&root.ws.path, tb, &[], &trie, root.ws.readonly, list, &mut children, &mut unshared, &mut warnings)?;
            let n = unshared.len() - before_unshared;
            warnings.push(format!(
                "{} is split so a strict carve can keep a folder out of the sandbox: {} file{} and other entr{} beside the mounted folders will not be visible, and nothing can be created at those levels",
                root.ws.path,
                n,
                if n == 1 { "" } else { "s" },
                if n == 1 { "y" } else { "ies" },
            ));
        } else {
            children.push(Mount { host: root.ws.path.clone(), target: tb.to_string(), readonly: root.ws.readonly, mask: false });
        }
        // Soft carves go over whichever mount holds them.
        let mut soft: Vec<&usize> = root.soft.iter().collect();
        soft.sort_by(|&&a, &&b| carves[a].path.cmp(&carves[b].path));
        let mut soft_mounts = vec![];
        for &i in soft {
            let c = &carves[i];
            let rel = rel_of(c);
            let rel_refs: Vec<&str> = rel.iter().map(String::as_str).collect();
            let target = join(tb, &rel_refs);
            let held = children.iter().any(|m| target == m.target || target.starts_with(&format!("{}/", m.target.trim_end_matches('/'))));
            if !held {
                warnings.push(format!("carve {} is soft and lies where nothing is shared (inside a strictly carved folder), so it does nothing", c.path));
                continue;
            }
            soft_mounts.push(match c.access {
                CarveAccess::Hidden => Mount { host: mask.to_string(), target, readonly: true, mask: true },
                CarveAccess::ReadOnly => Mount { host: c.path.clone(), target, readonly: true, mask: false },
            });
        }
        out.extend(children);
        out.extend(soft_mounts);
        per_root.push(out.len() - before);
    }

    let soft_n = carves.iter().filter(|c| c.enforce == CarveEnforce::Soft).count();
    if soft_n > 0 {
        warnings.push(format!(
            "{soft_n} soft carve{}: held by a mount that an agent with root in the sandbox can remove. It guards against mistakes, not against a hostile agent; use strict for that",
            if soft_n == 1 { "" } else { "s" }
        ));
    }

    let (primary, workdir, stub_used) = if split_main {
        let wd = if workdir.trim().is_empty() {
            main.path.clone()
        } else if workdir.starts_with('/') {
            workdir.to_string()
        } else {
            join(&main.path, &parts(workdir))
        };
        (Mount { host: stub.to_string(), target: stub.to_string(), readonly: false, mask: false }, Some(wd), true)
    } else {
        let first = main_mounts.remove(0);
        (first, None, false)
    };
    let mut mounts = main_mounts;
    mounts.extend(extra_mounts);

    let total = 1 + mounts.len();
    if total > MAX_MOUNTS {
        // Name the carve whose folder gave the most mounts.
        let (ri, _) = per_root.iter().enumerate().max_by_key(|(_, n)| **n).unwrap_or((0, &0));
        let culprit = roots[ri].strict.first().map(|&i| carves[i].path.clone()).unwrap_or_else(|| roots[ri].ws.path.clone());
        return Err(format!(
            "the carves need {total} mounts and sbx takes at most {MAX_MOUNTS}: the strict carve at {culprit} splits {} into too many folders. Make it soft, or carve a folder nearer the top",
            roots[ri].ws.path
        ));
    }
    Ok(Plan { primary, mounts, workdir, unshared, warnings, stub: stub_used })
}

/// Same path twice: the stronger of each setting.
fn merge(carves: &[Carve]) -> Result<Vec<Carve>, String> {
    let mut out: Vec<Carve> = vec![];
    for c in carves {
        let path = if c.path.len() > 1 { c.path.trim_end_matches('/').to_string() } else { c.path.clone() };
        if !path.starts_with('/') {
            return Err(format!("carve {path}: expected an absolute host path"));
        }
        match out.iter_mut().find(|o| o.path == path) {
            Some(o) => {
                if c.access == CarveAccess::Hidden {
                    o.access = CarveAccess::Hidden;
                }
                if c.enforce == CarveEnforce::Strict {
                    o.enforce = CarveEnforce::Strict;
                }
            }
            None => out.push(Carve { path, ..c.clone() }),
        }
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn walk(
    host_base: &str,
    target_base: &str,
    rel: &[&str],
    node: &Node,
    readonly: bool,
    list: Lister,
    mounts: &mut Vec<Mount>,
    unshared: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    let dir = join(host_base, rel);
    let mut entries = list(&dir).map_err(|e| format!("listing {dir}: {e}"))?;
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    for name in node.kids.keys() {
        if !entries.iter().any(|e| &e.name == name) {
            return Err(format!("carve {}: no such folder", join(&dir, &[name])));
        }
    }
    for e in entries {
        let mut next: Vec<&str> = rel.to_vec();
        next.push(&e.name);
        let child_host = join(host_base, &next);
        let child_target = join(target_base, &next);
        let carved = node.kids.get(&e.name);
        if carved.is_some() && e.kind != Kind::Dir {
            return Err(format!("carve {child_host}: not a folder (sbx shares folders only)"));
        }
        match carved {
            None if e.kind == Kind::Dir => {
                mounts.push(Mount { host: child_host, target: child_target, readonly, mask: false });
            }
            None => unshared.push(child_host),
            Some(cn) => match (cn.access, cn.kids.is_empty()) {
                (Some(CarveAccess::Hidden), empty) => {
                    if !empty {
                        warnings.push(format!("carves inside {child_host} do nothing: the folder is not shared at all"));
                    }
                }
                (Some(CarveAccess::ReadOnly), true) => {
                    mounts.push(Mount { host: child_host, target: child_target, readonly: true, mask: false });
                }
                (Some(CarveAccess::ReadOnly), false) => {
                    walk(host_base, target_base, &next, cn, true, list, mounts, unshared, warnings)?;
                }
                (None, _) => {
                    walk(host_base, target_base, &next, cn, readonly, list, mounts, unshared, warnings)?;
                }
            },
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    type Fs = HashMap<&'static str, Vec<(&'static str, Kind)>>;

    fn fs(entries: &[(&'static str, &[(&'static str, Kind)])]) -> Fs {
        entries.iter().map(|(k, v)| (*k, v.to_vec())).collect()
    }

    fn lister(fs: &Fs) -> impl Fn(&str) -> Result<Vec<Entry>, String> + '_ {
        move |p| match fs.get(p) {
            Some(v) => Ok(v.iter().map(|(n, k)| Entry::new(n, *k)).collect()),
            None => Err("no such directory".into()),
        }
    }

    fn ws(path: &str) -> Workspace {
        Workspace { path: path.into(), target: None, readonly: false }
    }

    fn carve(path: &str, access: CarveAccess, enforce: CarveEnforce) -> Carve {
        Carve { path: path.into(), access, enforce }
    }

    const NONE: CarveAccess = CarveAccess::Hidden;
    const RO: CarveAccess = CarveAccess::ReadOnly;
    const STRICT: CarveEnforce = CarveEnforce::Strict;
    const SOFT: CarveEnforce = CarveEnforce::Soft;
    use Kind::{Dir, File, Symlink};

    fn run(main: Workspace, extras: &[Workspace], carves: &[Carve], fs: &Fs, workdir: &str) -> Result<Plan, String> {
        plan(&main, extras, carves, &lister(fs), "/stub", "/mask", workdir)
    }

    fn specs(p: &Plan) -> Vec<String> {
        std::iter::once(p.primary_spec()).chain(p.specs()).collect()
    }

    fn projects() -> Fs {
        fs(&[
            ("/p", &[("a", Dir), ("secret", Dir), ("b", Dir), ("README", File), ("link", Symlink), (".sbxw-artifacts", Dir)]),
        ])
    }

    #[test]
    fn no_carve_is_the_workspace_as_it_is() {
        let p = run(ws("/p"), &[ws("/x")], &[], &Fs::new(), "").unwrap();
        assert_eq!(specs(&p), ["/p", "/x"]);
        assert!(!p.stub && p.workdir.is_none() && p.unshared.is_empty() && p.warnings.is_empty());
    }

    #[test]
    fn strict_none_mounts_the_other_folders_and_reports_the_files() {
        let p = run(ws("/p"), &[], &[carve("/p/secret", NONE, STRICT)], &projects(), "").unwrap();
        assert_eq!(specs(&p), ["/stub", "/p/.sbxw-artifacts", "/p/a", "/p/b"]);
        assert!(p.stub);
        assert_eq!(p.workdir.as_deref(), Some("/p"));
        assert_eq!(p.unshared, ["/p/README", "/p/link"]);
        assert!(p.warnings[0].contains("2 files"), "{:?}", p.warnings);
        assert!(!p.all_mounts().any(|m| m.host.contains("secret")));
    }

    #[test]
    fn strict_ro_mounts_the_carved_folder_read_only() {
        let p = run(ws("/p"), &[], &[carve("/p/secret", RO, STRICT)], &projects(), "web").unwrap();
        assert_eq!(specs(&p), ["/stub", "/p/.sbxw-artifacts", "/p/a", "/p/b", "/p/secret:ro"]);
        assert_eq!(p.workdir.as_deref(), Some("/p/web"));
    }

    #[test]
    fn an_absolute_workdir_is_kept() {
        let p = run(ws("/p"), &[], &[carve("/p/secret", NONE, STRICT)], &projects(), "/p/a").unwrap();
        assert_eq!(p.workdir.as_deref(), Some("/p/a"));
    }

    #[test]
    fn soft_none_masks_and_keeps_the_parent() {
        let p = run(ws("/p"), &[], &[carve("/p/secret", NONE, SOFT)], &Fs::new(), "web").unwrap();
        assert_eq!(specs(&p), ["/p", "/mask:/p/secret:ro"]);
        assert!(p.mounts[0].mask && !p.stub && p.workdir.is_none() && p.unshared.is_empty());
        assert!(p.warnings.iter().any(|w| w.contains("root")), "{:?}", p.warnings);
    }

    #[test]
    fn soft_ro_mounts_the_folder_itself_read_only() {
        let p = run(ws("/p"), &[], &[carve("/p/secret", RO, SOFT)], &Fs::new(), "").unwrap();
        assert_eq!(specs(&p), ["/p", "/p/secret:ro"]);
    }

    #[test]
    fn a_carve_two_levels_down_walks_both_levels() {
        let f = fs(&[
            ("/p", &[("clients", Dir), ("tools", Dir), ("notes.txt", File)]),
            ("/p/clients", &[("acme", Dir), ("beta", Dir), ("list.csv", File)]),
        ]);
        let p = run(ws("/p"), &[], &[carve("/p/clients/acme", NONE, STRICT)], &f, "").unwrap();
        assert_eq!(specs(&p), ["/stub", "/p/clients/beta", "/p/tools"]);
        assert_eq!(p.unshared, ["/p/clients/list.csv", "/p/notes.txt"]);
    }

    #[test]
    fn several_strict_carves_share_the_walk_and_nested_ones_cooperate() {
        let f = fs(&[
            ("/p", &[("clients", Dir), ("tools", Dir)]),
            ("/p/clients", &[("acme", Dir), ("beta", Dir)]),
            ("/p/clients/beta", &[("keys", Dir), ("src", Dir)]),
        ]);
        let carves = [
            carve("/p/clients/acme", NONE, STRICT),
            carve("/p/clients/beta", RO, STRICT),
            carve("/p/clients/beta/keys", NONE, STRICT),
        ];
        let p = run(ws("/p"), &[], &carves, &f, "").unwrap();
        // beta is read-only, so what is mounted inside it is read-only too.
        assert_eq!(specs(&p), ["/stub", "/p/clients/beta/src:ro", "/p/tools"]);
        // A carve inside a folder that is hidden does nothing, and says so.
        let carves = [carve("/p/clients", NONE, STRICT), carve("/p/clients/acme", NONE, STRICT)];
        let p = run(ws("/p"), &[], &carves, &f, "").unwrap();
        assert_eq!(specs(&p), ["/stub", "/p/tools"]);
        assert!(p.warnings.iter().any(|w| w.contains("do nothing")));
    }

    #[test]
    fn a_soft_carve_inside_a_split_folder_goes_over_the_child_mount() {
        let carves = [carve("/p/secret", NONE, STRICT), carve("/p/a/private", NONE, SOFT), carve("/p/b/docs", RO, SOFT)];
        let p = run(ws("/p"), &[], &carves, &projects(), "").unwrap();
        assert_eq!(specs(&p), ["/stub", "/p/.sbxw-artifacts", "/p/a", "/p/b", "/mask:/p/a/private:ro", "/p/b/docs:ro"]);
        // Inside a strict ro folder it attaches to that mount; inside a hidden one it does nothing.
        let f = fs(&[("/p", &[("r", Dir), ("h", Dir)])]);
        let carves = [carve("/p/r", RO, STRICT), carve("/p/h", NONE, STRICT), carve("/p/r/x", NONE, SOFT), carve("/p/h/y", NONE, SOFT)];
        let p = run(ws("/p"), &[], &carves, &f, "").unwrap();
        assert_eq!(specs(&p), ["/stub", "/p/r:ro", "/mask:/p/r/x:ro"]);
        assert!(p.warnings.iter().any(|w| w.contains("/p/h/y")));
        // A soft carve holding a strict one is refused.
        let e = run(ws("/p"), &[], &[carve("/p/a", NONE, SOFT), carve("/p/a/b", NONE, STRICT)], &projects(), "").unwrap_err();
        assert!(e.contains("/p/a (soft) holds a strict carve"), "{e}");
    }

    #[test]
    fn an_extra_folder_with_a_target_lands_at_its_target() {
        let extra = Workspace { path: "/data".into(), target: Some("/w/data".into()), readonly: false };
        let f = fs(&[("/data", &[("raw", Dir), ("pii", Dir), ("x.csv", File)])]);
        let p = run(ws("/p"), std::slice::from_ref(&extra), &[carve("/data/pii", NONE, STRICT)], &f, "").unwrap();
        assert_eq!(specs(&p), ["/p", "/data/raw:/w/data/raw"]);
        assert!(!p.stub && p.workdir.is_none(), "the main workspace is not split");
        assert_eq!(p.unshared, ["/data/x.csv"]);
        // Soft, the mask goes at the target's place.
        let p = run(ws("/p"), &[extra], &[carve("/data/pii", NONE, SOFT)], &f, "").unwrap();
        assert_eq!(specs(&p), ["/p", "/data:/w/data", "/mask:/w/data/pii:ro"]);
    }

    #[test]
    fn a_read_only_extra_folder_keeps_its_children_read_only() {
        let extra = Workspace { path: "/data".into(), target: None, readonly: true };
        let f = fs(&[("/data", &[("raw", Dir), ("pii", Dir)])]);
        let p = run(ws("/p"), std::slice::from_ref(&extra), &[carve("/data/pii", NONE, STRICT)], &f, "").unwrap();
        assert_eq!(specs(&p), ["/p", "/data/raw:ro"]);
        let p = run(ws("/p"), &[extra], &[carve("/data/pii", NONE, SOFT)], &f, "").unwrap();
        assert_eq!(specs(&p), ["/p", "/data:ro", "/mask:/data/pii:ro"]);
    }

    #[test]
    fn a_read_only_main_workspace_splits_read_only() {
        let main = Workspace { path: "/p".into(), target: None, readonly: true };
        let p = run(main, &[], &[carve("/p/secret", NONE, STRICT)], &projects(), "").unwrap();
        assert_eq!(specs(&p), ["/stub", "/p/.sbxw-artifacts:ro", "/p/a:ro", "/p/b:ro"]);
    }

    #[test]
    fn symlinks_and_files_are_reported_never_followed() {
        let p = run(ws("/p"), &[], &[carve("/p/secret", NONE, STRICT)], &projects(), "").unwrap();
        assert!(p.unshared.contains(&"/p/link".to_string()));
        assert!(!p.all_mounts().any(|m| m.host == "/p/link"));
    }

    #[test]
    fn carves_in_the_wrong_place_are_refused() {
        let f = projects();
        let e = run(ws("/p"), &[], &[carve("/elsewhere/x", NONE, STRICT)], &f, "").unwrap_err();
        assert!(e.contains("/elsewhere/x") && e.contains("not inside"), "{e}");
        let e = run(ws("/p"), &[ws("/p/sub")], &[carve("/p", NONE, SOFT)], &f, "").unwrap_err();
        assert!(e.contains("mounted folder itself"), "{e}");
        let e = run(ws("/p"), &[ws("/x")], &[carve("/x", NONE, STRICT)], &f, "").unwrap_err();
        assert!(e.contains("mounted folder itself"), "{e}");
        let e = run(ws("/p"), &[ws("/p/sub")], &[carve("/p/sub/z", NONE, SOFT)], &f, "").unwrap_err();
        assert!(e.contains("more than one"), "{e}");
        let e = run(ws("/p"), &[], &[carve("/p/nope", NONE, STRICT)], &f, "").unwrap_err();
        assert!(e.contains("/p/nope") && e.contains("no such folder"), "{e}");
        let e = run(ws("/p"), &[], &[carve("/p/README", NONE, STRICT)], &f, "").unwrap_err();
        assert!(e.contains("not a folder"), "{e}");
        let e = run(ws("/p"), &[], &[carve("/p/a/../b", NONE, SOFT)], &f, "").unwrap_err();
        assert!(e.contains("`..`"), "{e}");
        let e = run(ws("/p"), &[], &[carve("rel", NONE, SOFT)], &f, "").unwrap_err();
        assert!(e.contains("absolute"), "{e}");
        let e = run(ws("/p"), &[], &[carve("/p/gone", NONE, STRICT)], &Fs::new(), "").unwrap_err();
        assert!(e.contains("listing /p"), "{e}");
    }

    #[test]
    fn the_same_path_twice_takes_the_stronger() {
        let carves = [carve("/p/secret", RO, SOFT), carve("/p/secret/", NONE, STRICT)];
        let p = run(ws("/p"), &[], &carves, &projects(), "").unwrap();
        assert!(!p.all_mounts().any(|m| m.host.contains("secret")));
    }

    #[test]
    fn too_many_mounts_are_refused_naming_the_carve() {
        let names: Vec<String> = (0..250).map(|i| format!("d{i}")).collect();
        let kids: Vec<(&'static str, Kind)> = names.iter().map(|n| (&*Box::leak(n.clone().into_boxed_str()), Dir)).collect();
        let mut f = fs(&[]);
        f.insert("/p", [kids, vec![("secret", Dir)]].concat());
        let e = run(ws("/p"), &[], &[carve("/p/secret", NONE, STRICT)], &f, "").unwrap_err();
        assert!(e.contains("251 mounts") && e.contains("200") && e.contains("/p/secret"), "{e}");
        // Soft needs no split.
        assert!(run(ws("/p"), &[], &[carve("/p/secret", NONE, SOFT)], &f, "").is_ok());
    }
}
