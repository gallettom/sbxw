//! `.sbxw-artifacts/`: where non-code deliverables live, by convention.
//!
//! sbxw lists and serves what it finds there (a hook in the sandbox nudges the
//! agent to put new deliverables there). A code map and its lenses are
//! *directories* of linked markdown, so they are folded out of the file list
//! into summaries the map viewer reads — listing a map file by file buried the
//! deliverables beside it.

use crate::codemap::{CODEMAP_DIR, LENS_DIR};
use serde::Serialize;

pub const ARTIFACTS_DIR: &str = ".sbxw-artifacts";

pub const EXTENSIONS: &[&str] = &[
    "md", "markdown", "pdf", "png", "jpg", "jpeg", "gif", "svg", "webp", "docx", "pptx", "xlsx",
    "csv", "html", "txt",
];

pub const MAX_DEPTH: u32 = 6;

/// Seeded into a fresh workspace's artifacts folder.
pub const README: &str = "# .sbxw-artifacts\n\n\
Drop non-code deliverables here (wireframes, docs, diagrams, exports —\n\
.md .pdf .png .jpg .svg .webp .docx .pptx .xlsx .csv .html .txt) and they show up\n\
in the sbxw web UI's \"Files\" panel with a one-click download, instead of\n\
being buried in the repo.\n";

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Entry {
    /// Relative to `.sbxw-artifacts`, forward slashes.
    pub path: String,
    pub name: String,
    pub size: u64,
    /// Unix seconds.
    pub modified: u64,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct MapSummary {
    pub dir: String,
    pub slug: String,
    pub files: usize,
    pub modified: u64,
    pub entries: Vec<Entry>,
}

pub fn has_allowed_extension(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// Take everything under `dir/` out of `entries`, as one summary.
pub fn take_map(entries: &mut Vec<Entry>, dir: &str) -> Option<MapSummary> {
    let prefix = format!("{dir}/");
    let (map, rest): (Vec<_>, Vec<_>) = std::mem::take(entries)
        .into_iter()
        .partition(|e| e.path.starts_with(&prefix));
    *entries = rest;
    (!map.is_empty()).then(|| MapSummary {
        dir: dir.to_string(),
        slug: dir.rsplit('/').next().unwrap_or(dir).to_string(),
        files: map.len(),
        modified: map.iter().map(|e| e.modified).max().unwrap_or(0),
        entries: map,
    })
}

/// Take every lens (`codemap-lenses/<slug>/…`) out, newest first. A file loose
/// in `codemap-lenses/` belongs to no lens and stays in the list.
pub fn take_lenses(entries: &mut Vec<Entry>) -> Vec<MapSummary> {
    let prefix = format!("{LENS_DIR}/");
    let mut slugs: Vec<String> = Vec::new();
    for e in entries.iter() {
        if let Some((slug, _)) = e.path.strip_prefix(&prefix).and_then(|r| r.split_once('/')) {
            if !slug.is_empty() && !slugs.iter().any(|s| s == slug) {
                slugs.push(slug.to_string());
            }
        }
    }
    let mut lenses: Vec<MapSummary> = slugs
        .iter()
        .filter_map(|slug| take_map(entries, &format!("{prefix}{slug}")))
        .collect();
    lenses.sort_by_key(|l| std::cmp::Reverse(l.modified));
    lenses
}

/// The listing as the panel reads it: files, the map, the lenses.
pub fn fold(mut entries: Vec<Entry>) -> (Vec<Entry>, Option<MapSummary>, Vec<MapSummary>) {
    entries.sort_by_key(|e| std::cmp::Reverse(e.modified));
    let map = take_map(&mut entries, CODEMAP_DIR);
    let lenses = take_lenses(&mut entries);
    (entries, map, lenses)
}

pub fn guess_mime(filename: &str) -> &'static str {
    let ext = filename.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "md" | "markdown" => "text/markdown",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "csv" => "text/csv",
        "html" => "text/html",
        "txt" => "text/plain",
        "json" => "application/json",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(path: &str, modified: u64) -> Entry {
        Entry {
            path: path.into(),
            name: path.rsplit('/').next().unwrap().into(),
            size: 1,
            modified,
        }
    }

    #[test]
    fn the_map_and_lenses_are_folded_out() {
        let (files, map, lenses) = fold(vec![
            e("wire.png", 5),
            e("codemap/index.md", 3),
            e("codemap/a.md", 4),
            e("codemap-lenses/po/index.md", 2),
            e("codemap-lenses/dev/index.md", 6),
            e("codemap-lenses/loose.md", 1),
        ]);
        assert_eq!(files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["wire.png", "codemap-lenses/loose.md"]);
        let map = map.unwrap();
        assert_eq!((map.files, map.modified, map.slug.as_str()), (2, 4, "codemap"));
        assert_eq!(lenses.iter().map(|l| l.slug.as_str()).collect::<Vec<_>>(), ["dev", "po"]);
    }

    #[test]
    fn a_project_without_a_map_keeps_its_files() {
        let (files, map, lenses) = fold(vec![e("a.md", 1)]);
        assert_eq!(files.len(), 1);
        assert!(map.is_none() && lenses.is_empty());
    }

    #[test]
    fn extensions_and_mimes() {
        assert!(has_allowed_extension("A.PNG"));
        assert!(!has_allowed_extension("main.rs"));
        assert!(!has_allowed_extension("noext"));
        assert_eq!(guess_mime("x.md"), "text/markdown");
        assert_eq!(guess_mime("x.bin"), "application/octet-stream");
    }
}
