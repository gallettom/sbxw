//! Files handed to a sandbox by the person: where they land, under which
//! name, and the block that tells an agent about them. Shared by the web
//! UI's upload box (`POST /api/sandboxes/:name/upload`), the web chat's
//! attachments and the phone's upload (`docs/security.md#remote-access`).

/// The folder every uploaded file lands under, in the sandbox.
pub const UPLOAD_ROOT: &str = "/tmp/sbxw-uploads";

/// The longest name kept.
pub const NAME_MAX: usize = 120;

/// A browser's (or a phone's) file name as one safe path component. The
/// path is typed into a prompt, where a space would split it in two, so
/// anything but `[A-Za-z0-9._-]` becomes `_` (no control, invisible or
/// look-alike character survives); leading dots go, so no `..` and no
/// hidden file; at most [`NAME_MAX`] characters; `upload` when nothing is
/// left. Only the last path component is kept.
pub fn upload_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let safe: String = base.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).collect();
    let safe: String = safe.trim_start_matches('.').chars().take(NAME_MAX).collect();
    if safe.is_empty() {
        "upload".into()
    } else {
        safe
    }
}

/// Where a file of `name` (already [`upload_name`]d) lands, for an upload
/// made at `at_ms`.
pub fn dest(at_ms: u64, name: &str) -> String {
    format!("{UPLOAD_ROOT}/{at_ms}/{name}")
}

/// The first line of the block (the web chat's `chatAttachBlock`).
pub const ATTACH_HEADER: &str = "Attached files (uploaded to this sandbox):";

/// The block that names files sent with a message: built from the daemon's
/// own paths, never from a page's text.
pub fn attach_block(paths: &[String]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    let mut s = ATTACH_HEADER.to_string();
    for p in paths {
        s.push_str("\n- ");
        s.push_str(p);
    }
    s
}

/// The person's text, then a blank line and the block (the web chat's
/// `chatWithAttachments`).
pub fn with_attachments(text: &str, paths: &[String]) -> String {
    let block = attach_block(paths);
    match (text.is_empty(), block.is_empty()) {
        (_, true) => text.to_string(),
        (true, false) => block,
        (false, false) => format!("{text}\n\n{block}"),
    }
}

/// Names for files copied together into one folder: a repeated name gets
/// `2-`, `3-`... in front, so no copy overwrites another.
pub fn distinct_names(names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(names.len());
    for n in names {
        let mut pick = n.clone();
        let mut k = 2;
        while out.contains(&pick) {
            pick = format!("{k}-{n}").chars().take(NAME_MAX).collect();
            k += 1;
        }
        out.push(pick);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_one_safe_component() {
        assert_eq!(upload_name("shot 1 (final).png"), "shot_1__final_.png");
        assert_eq!(upload_name("../../etc/passwd"), "passwd");
        assert_eq!(upload_name("C:\\Users\\me\\clip.mov"), "clip.mov");
        assert_eq!(upload_name(".env"), "env");
        assert_eq!(upload_name("..."), "upload");
        assert_eq!(upload_name(".."), "upload");
        assert_eq!(upload_name("a/.."), "upload");
        assert_eq!(upload_name(""), "upload");
        assert_eq!(upload_name("été.jpg"), "_t_.jpg");
        // Look-alikes, invisible and control characters, a fullwidth slash.
        assert_eq!(upload_name("\u{0430}pple.png"), "_pple.png", "Cyrillic a");
        assert_eq!(upload_name("a\u{200b}b\u{202e}c\u{0007}d\n.txt"), "a_b_c_d_.txt");
        assert_eq!(upload_name("x\u{ff0f}..\u{ff0f}y"), "x_.._y", "a fullwidth solidus is not a separator, and stays inside one name");
        assert_eq!(upload_name("!rm -rf"), "_rm_-rf");
        assert_eq!(upload_name("#notes"), "_notes");
        assert_eq!(upload_name(&"a".repeat(4000)).len(), NAME_MAX);
        for raw in ["../../x", "/abs", "\\\\srv\\share", "a\0b", "...hidden", "\u{2028}x"] {
            let n = upload_name(raw);
            assert!(!n.starts_with('.') && !n.contains('/') && !n.contains('\\') && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')), "{raw:?} -> {n:?}");
        }
    }

    #[test]
    fn the_block_is_the_web_chat_s() {
        let paths = vec!["/tmp/sbxw-uploads/1/a.png".to_string(), "/tmp/sbxw-uploads/1/b.pdf".to_string()];
        assert_eq!(attach_block(&paths), "Attached files (uploaded to this sandbox):\n- /tmp/sbxw-uploads/1/a.png\n- /tmp/sbxw-uploads/1/b.pdf");
        assert_eq!(with_attachments("look", &paths), format!("look\n\n{}", attach_block(&paths)));
        assert_eq!(with_attachments("", &paths), attach_block(&paths));
        assert_eq!(with_attachments("look", &[]), "look");
        assert_eq!(dest(7, "a.png"), "/tmp/sbxw-uploads/7/a.png");
    }

    #[test]
    fn repeated_names_do_not_collide() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(distinct_names(&v(&["a.png", "a.png", "b", "a.png"])), v(&["a.png", "2-a.png", "b", "3-a.png"]));
        assert_eq!(distinct_names(&v(&["2-a", "a", "a"])), v(&["2-a", "a", "3-a"]));
    }
}
