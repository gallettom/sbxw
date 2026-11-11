//! Carves on a real host: resolve the paths, list the folders a strict carve
//! splits, and hand all of it to the pure planner (`sbxw_core::carve`).
//!
//! Everything goes through the host's filesystem, so it works for a remote
//! host as it does here.

use crate::host::{Host, Kind as HostKind};
use anyhow::{bail, Context, Result};
use sbxw_core::carve::{self, Entry, Kind, Plan};
use sbxw_core::config::{Carve, Workspace};
use std::collections::HashMap;
use std::path::Path;

fn canon_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// The plan for `main` (the workspace, always read-write), the `extras` and
/// the `carves` (absolute host paths), on `host`. Paths are resolved there
/// first: a carve through a symlink means the folder it points at. Creates
/// nothing; `stub` and `mask` are only named.
pub async fn plan_on(host: &Host, main: &Path, extras: &[Workspace], carves: &[Carve], stub: &Path, mask: &Path, workdir: &str) -> Result<Plan> {
    let fs = &host.fs;
    let resolve = async |p: &str, what: &str| -> Result<String> {
        match fs.canonicalize(Path::new(p)).await {
            Ok(c) => Ok(canon_str(&c)),
            Err(_) => bail!("{what} {p}: no such folder"),
        }
    };
    let main = Workspace { path: resolve(&main.to_string_lossy(), "workspace").await?, target: None, readonly: false };
    let mut ex = vec![];
    for w in extras {
        ex.push(Workspace { path: resolve(&w.path, "folder").await?, ..w.clone() });
    }
    let mut cs = vec![];
    for c in carves {
        cs.push(Carve { path: resolve(&c.path, "carve").await?, ..c.clone() });
    }
    let mut listed: HashMap<String, Vec<Entry>> = HashMap::new();
    for dir in carve::listing_needs(&main, &ex, &cs) {
        let entries = fs.read_dir(Path::new(&dir)).await.with_context(|| format!("listing {dir}"))?;
        let entries = entries
            .into_iter()
            .map(|e| Entry {
                name: e.name,
                kind: match e.meta.kind {
                    HostKind::Dir => Kind::Dir,
                    HostKind::File => Kind::File,
                    HostKind::Symlink => Kind::Symlink,
                    _ => Kind::Other,
                },
            })
            .collect();
        listed.insert(dir, entries);
    }
    let list = |p: &str| listed.get(p).cloned().ok_or_else(|| "not listed".to_string());
    carve::plan(&main, &ex, &cs, &list, &canon_str(stub), &canon_str(mask), workdir).map_err(anyhow::Error::msg)
}

/// Make the two helper folders the plan names.
pub async fn make_helpers(host: &Host, stub: &Path, mask: &Path) -> Result<()> {
    for d in [stub, mask] {
        host.fs.create_dir_all(d).await.with_context(|| format!("creating {}", d.display()))?;
    }
    Ok(())
}
