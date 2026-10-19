//! Starred folders in the create dialog's picker.
//!
//! Kept host-side (they name directories on *this* machine), canonical (so
//! `~/dev`, `~/dev/` and a symlink are one star), at most twelve, and never
//! edited behind the user's back: a folder that is not there right now is
//! shown greyed, not dropped.

use serde::Serialize;
use std::path::Path;

pub const MAX: usize = 12;

#[derive(Serialize, PartialEq, Debug)]
pub struct Favourite {
    pub path: String,
    pub name: String,
    pub missing: bool,
}

/// Chip labels: the folder's own name, with its parent folded in only for
/// names that clash (`work/projects`, `perso/projects`).
pub fn labels(paths: &[String]) -> Vec<String> {
    let base = |p: &str| {
        Path::new(p)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.to_string())
    };
    let bases: Vec<String> = paths.iter().map(|p| base(p)).collect();
    paths
        .iter()
        .zip(&bases)
        .map(|(path, name)| {
            if bases.iter().filter(|b| *b == name).count() < 2 {
                return name.clone();
            }
            match Path::new(path).parent().map(|p| base(&p.to_string_lossy())) {
                Some(parent) if !parent.is_empty() => format!("{parent}/{name}"),
                _ => name.clone(),
            }
        })
        .collect()
}

pub fn payload(paths: Vec<String>, exists: impl Fn(&str) -> bool) -> Vec<Favourite> {
    let names = labels(&paths);
    paths
        .into_iter()
        .zip(names)
        .map(|(path, name)| Favourite {
            missing: !exists(&path),
            path,
            name,
        })
        .collect()
}

/// Star `canonical` (already resolved by the caller) in `list`.
pub fn star(list: &mut Vec<String>, canonical: String) -> Result<(), String> {
    if !list.contains(&canonical) {
        if list.len() >= MAX {
            return Err(format!(
                "you already have {MAX} favourite folders — unstar one to add another"
            ));
        }
        list.push(canonical);
    }
    Ok(())
}

/// Unstar by the path as given *or* its canonical form — a folder that is
/// gone cannot be canonicalised, and unstarring it is exactly the point.
pub fn unstar(list: &mut Vec<String>, raw: &str, canonical: Option<&str>) {
    list.retain(|p| p != raw && canonical.is_none_or(|c| p != c));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chips_only_grow_a_parent_when_names_collide() {
        let l = labels(&["/a/work/projects".into(), "/a/perso/projects".into(), "/a/dev".into()]);
        assert_eq!(l, ["work/projects", "perso/projects", "dev"]);
    }

    #[test]
    fn twelve_at_most_and_unstarring_works_on_missing_folders() {
        let mut list: Vec<String> = (0..MAX).map(|i| format!("/f{i}")).collect();
        assert!(star(&mut list, "/new".into()).is_err());
        star(&mut list, "/f0".into()).unwrap();
        unstar(&mut list, "/f0", None);
        assert_eq!(list.len(), MAX - 1);
        unstar(&mut list, "/link", Some("/f1"));
        assert!(!list.contains(&"/f1".to_string()));
    }

    #[test]
    fn missing_folders_are_flagged_not_dropped() {
        let p = payload(vec!["/here".into(), "/gone".into()], |p| p == "/here");
        assert!(!p[0].missing && p[1].missing);
    }
}
