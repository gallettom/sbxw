//! Files dropped on a terminal pane: the guardrails that make the upload
//! dialog ask even when the person chose "Do not ask again for files I
//! drop". The core runs [`guardrails`] again before it lets a drop upload
//! without its dialog (`shell/features.rs`), so both sides read one list.
//!
//! Fail closed: a file that cannot be checked (its folder unreadable, its
//! size unknown) is a reason to ask.

use std::path::{Path, PathBuf};

/// File names that look like a secret, matched case-insensitively: `x*` a
/// prefix, `*x` a suffix, else the whole name.
pub const SECRET_NAMES: &[&str] = &[".env*", "*.pem", "*.key", "id_rsa*", "id_ed25519*", "*.p12", "*.pfx", "credentials*", ".npmrc", ".netrc", "*.kdbx"];

/// Folders whose files are asked about wherever they sit (`~/.ssh`, and a
/// copy of one anywhere else).
pub const SECRET_DIRS: &[&str] = &[".ssh", ".aws", ".gnupg"];

/// More files than this in one drop: asked.
pub const ASK_ABOVE_FILES: usize = 10;

/// A drop larger than this in all: asked (the daemon's hard limit is 128 MB a file).
pub const ASK_ABOVE_BYTES: u64 = 25 * 1024 * 1024;

fn matches(pattern: &str, name: &str) -> bool {
    let (p, n) = (pattern.to_lowercase(), name.to_lowercase());
    match (p.strip_prefix('*'), p.strip_suffix('*')) {
        (Some(suffix), _) => n.ends_with(suffix),
        (_, Some(prefix)) => n.starts_with(prefix),
        _ => n == p,
    }
}

/// Whether `name` looks like a secret ([`SECRET_NAMES`]).
pub fn secret_name(name: &str) -> bool {
    SECRET_NAMES.iter().any(|p| matches(p, name))
}

/// Why this drop is asked about even when the person chose not to be:
/// empty for a plain drop. `dropped`: how many files the drop held, before
/// the ten-at-a-time cut.
pub fn guardrails(paths: &[PathBuf], dropped: usize) -> Vec<String> {
    let mut why = vec![];
    if dropped > ASK_ABOVE_FILES {
        why.push(format!("{dropped} files dropped at once (more than {ASK_ABOVE_FILES})"));
    }
    let mut total: u64 = 0;
    for p in paths {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if secret_name(&name) {
            why.push(format!("{name}: its name looks like a secret (keys, tokens, credentials)"));
        }
        match std::fs::canonicalize(p) {
            Ok(real) => {
                if let Some(d) = secret_dir(&real) {
                    why.push(format!("{name}: it is in a {d} folder"));
                }
            }
            Err(e) => why.push(format!("{name}: could not be checked ({e})")),
        }
        match std::fs::metadata(p) {
            Ok(m) => total = total.saturating_add(m.len()),
            Err(e) => why.push(format!("{name}: its size could not be read ({e})")),
        }
    }
    if total > ASK_ABOVE_BYTES {
        why.push(format!("{} in all (more than {})", super::view::size_label(total), super::view::size_label(ASK_ABOVE_BYTES)));
    }
    why
}

fn secret_dir(path: &Path) -> Option<&'static str> {
    path.parent()?.components().find_map(|c| SECRET_DIRS.iter().copied().find(|d| c.as_os_str() == *d))
}
