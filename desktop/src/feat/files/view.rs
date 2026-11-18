//! What the Project panel shows, worked out from the daemon's answers
//! (`assets/js/files.js`): rows with readable sizes and times, the places,
//! what may be deleted, what a file previews as, the upload limits.

use serde_json::{json, Value as J};

/// The daemon's ceiling for one upload (`UPLOAD_MAX`, `crates/daemon/src/http/ui.rs`).
pub const UPLOAD_MAX: u64 = 128 * 1024 * 1024;
/// Files handed in at once, waiting for a sandbox.
pub const STAGED_MAX: usize = 10;
/// Text read for a preview (`FX_TEXT_MAX`); past it, the head only.
pub const TEXT_MAX: u64 = 1024 * 1024;
/// Text drawn: the rest is said, not drawn (a long log costs every frame).
pub const TEXT_SHOWN: usize = 128 * 1024;
/// A picture previewed (`FX_IMAGE_MAX`), before the core's own limits.
pub const IMAGE_MAX: u64 = 24 * 1024 * 1024;
/// Recent: the badge counts the last hour's files.
pub const FRESH_SECS: u64 = 3600;

/// `1240` → `1,240`.
pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn size_label(n: u64) -> String {
    const K: f64 = 1024.0;
    let f = n as f64;
    if n < 1024 {
        format!("{n} B")
    } else if f < K * K {
        format!("{:.1} KB", f / K)
    } else if f < K * K * K {
        format!("{:.1} MB", f / K / K)
    } else {
        format!("{:.1} GB", f / K / K / K)
    }
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `just now`, `5 min ago`, `3 h ago`, `2 d ago`, else the date.
pub fn ago(t: u64, now: u64) -> String {
    if t == 0 {
        return String::new();
    }
    let d = now.saturating_sub(t);
    if d < 60 {
        "just now".into()
    } else if d < 3600 {
        format!("{} min ago", d / 60)
    } else if d < 86_400 {
        format!("{} h ago", d / 3600)
    } else if d < 30 * 86_400 {
        format!("{} d ago", d / 86_400)
    } else {
        date(t)
    }
}

/// `2026-11-16` (UTC).
pub fn date(t: u64) -> String {
    // Civil from days (Howard Hinnant's algorithm).
    let z = (t / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// What a file previews as, from its name (and its size, for the caps).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Markdown,
    Image,
    /// HTML: not rendered here (no web view); its source is shown.
    Html,
    Text,
    /// PDF, Office, video, archives: no preview, download or open.
    Other,
}

pub fn ext(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, e)) if !stem.is_empty() => e.to_ascii_lowercase(),
        _ => String::new(),
    }
}

pub fn kind(name: &str) -> Kind {
    match ext(name).as_str() {
        "md" | "markdown" => Kind::Markdown,
        "png" | "jpg" | "jpeg" => Kind::Image,
        "html" | "htm" => Kind::Html,
        "pdf" | "docx" | "pptx" | "xlsx" | "doc" | "xls" | "ppt" | "zip" | "gz" | "tgz" | "tar" | "7z" | "mp4" | "m4v" | "mov" | "webm" | "mp3" | "wav" | "gif" | "webp" | "svg" | "bmp" | "ico" | "heic" | "woff" | "woff2" | "ttf" | "otf" | "so" | "dylib" | "exe" | "bin" => Kind::Other,
        _ => Kind::Text,
    }
}

/// A readable type, for rows and dialogs.
pub fn kind_label(name: &str) -> &'static str {
    match ext(name).as_str() {
        "md" | "markdown" => "Markdown document",
        "png" => "PNG picture",
        "jpg" | "jpeg" => "JPEG picture",
        "gif" => "GIF picture",
        "webp" => "WebP picture",
        "svg" => "SVG drawing (it can carry scripts)",
        "html" | "htm" => "HTML page",
        "pdf" => "PDF document",
        "docx" => "Word document",
        "xlsx" => "Excel workbook",
        "pptx" => "PowerPoint presentation",
        "csv" => "CSV table",
        "txt" | "log" => "Text file",
        "json" => "JSON file",
        "sh" | "bash" | "zsh" | "command" | "py" | "rb" | "pl" | "ps1" | "bat" | "cmd" => "Script",
        "exe" | "app" | "dmg" | "pkg" | "msi" | "deb" | "rpm" | "appimage" => "Program or installer",
        "" => "File with no extension",
        _ => "File",
    }
}

/// The preview of a file that is not read (too big, no preview): `None` to read it.
pub fn preview_without_reading(name: &str, size: u64) -> Option<J> {
    match kind(name) {
        Kind::Image if size > IMAGE_MAX => Some(json!({ "state": "note", "text": format!("Too large to preview ({}): download it.", size_label(size)) })),
        Kind::Html if size > 8 * TEXT_MAX => Some(json!({ "state": "note", "text": "Too large to preview: download it." })),
        Kind::Other => Some(json!({ "state": "note", "text": format!("{}: no preview here. Download it, or open it with this computer's app.", kind_label(name)) })),
        _ => None,
    }
}

/// Bytes read for a text preview: what is drawn, and the line under it.
pub fn text_preview(name: &str, bytes: &[u8], size: u64) -> J {
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return json!({ "state": "note", "text": "Binary file: no preview." });
    }
    let text = String::from_utf8_lossy(bytes);
    let mut shown: String = text.chars().take(TEXT_SHOWN).collect();
    let cut_here = shown.len() < text.len();
    let mut note = String::new();
    let read = bytes.len() as u64;
    if size > read || cut_here {
        let n = if cut_here { shown.len() as u64 } else { read };
        note = format!("First {} of {}: download it for the rest.", size_label(n), size_label(size.max(read)));
    }
    if cut_here {
        // Not in the middle of a line.
        if let Some(i) = shown.rfind('\n') {
            shown.truncate(i);
        }
    }
    let state = match kind(name) {
        Kind::Markdown => "markdown",
        Kind::Html => "html",
        _ => "text",
    };
    json!({ "state": state, "text": shown, "note": note })
}

/// A place in the sidebar of the browser (`fxBuiltins`).
pub fn places(listing: &J, starred: &[String]) -> Vec<J> {
    let mut v = vec![json!({ "id": "recent", "label": "Recent", "icon": "search", "recent": true })];
    let s = |k: &str| listing[k].as_str().filter(|x| !x.is_empty()).map(str::to_string);
    if let Some(a) = s("artifacts") {
        v.push(json!({ "id": a, "path": a, "label": ".sbxw-artifacts", "icon": "sparkle" }));
    }
    if let Some(w) = s("workspace") {
        v.push(json!({ "id": w, "path": w, "label": "Workspace", "icon": "folder" }));
    }
    for m in listing["mounts"].as_array().into_iter().flatten() {
        let p = m["path"].as_str().unwrap_or("");
        if p.is_empty() {
            continue;
        }
        let ro = m["readonly"] == true;
        v.push(json!({ "id": p, "path": p, "label": format!("{} ({})", p.rsplit('/').next().unwrap_or(p), if ro { "read only" } else { "read-write" }), "icon": "folder" }));
    }
    for w in listing["worktrees"].as_array().into_iter().flatten() {
        let p = w["artifacts"].as_str().or(w["path"].as_str()).unwrap_or("");
        if !p.is_empty() {
            v.push(json!({ "id": p, "path": p, "label": format!("thread {}", w["thread"].as_str().unwrap_or("?")), "icon": "columns" }));
        }
    }
    let home = s("home").unwrap_or_else(|| "/home/agent".into());
    v.push(json!({ "id": home, "path": home, "label": "Home", "icon": "folder" }));
    v.push(json!({ "id": "/var/log", "path": "/var/log", "label": "Kit logs", "icon": "file", "select": "sbx-kit-startup.log" }));
    v.push(json!({ "id": "/", "path": "/", "label": "Root", "icon": "folder" }));
    for p in starred {
        if !v.iter().any(|x| x["path"] == p.as_str()) {
            v.push(json!({ "id": p, "path": p, "label": p.rsplit('/').find(|c| !c.is_empty()).unwrap_or(p), "icon": "flag", "starred": true }));
        }
    }
    v
}

/// Whether `path` may be deleted from here (`fxCanDelete`; the daemon checks again).
pub fn can_delete(path: &str, listing: &J) -> bool {
    let trim = |s: &str| s.trim_end_matches('/').to_string();
    let ws = listing["workspace"].as_str().map(trim);
    let mut roots: Vec<String> = ws.iter().cloned().collect();
    let mounts = listing["mounts"].as_array().cloned().unwrap_or_default();
    roots.extend(mounts.iter().filter(|m| m["readonly"] != true).filter_map(|m| m["path"].as_str().map(trim)));
    roots.push(trim(listing["home"].as_str().unwrap_or("/home/agent")));
    roots.push("/tmp".into());
    let mut kept: Vec<String> = ws.iter().flat_map(|w| [format!("{w}/.sbxw-artifacts"), format!("{w}/.git"), format!("{w}/.sbxw-worktrees")]).collect();
    kept.extend(listing["worktrees"].as_array().into_iter().flatten().filter_map(|w| w["path"].as_str().map(trim)));
    let path = trim(path);
    if path.split('/').any(|c| c == ".." || c == ".") || !path.starts_with('/') {
        return false;
    }
    if roots.contains(&path) || kept.contains(&path) {
        return false;
    }
    if mounts.iter().any(|m| m["readonly"] == true && m["path"].as_str().is_some_and(|p| path.starts_with(&format!("{}/", trim(p))))) {
        return false;
    }
    roots.iter().any(|r| path.starts_with(&format!("{r}/")))
}

/// A child of `dir`.
pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// The folder a path is in.
pub fn parent(path: &str) -> String {
    match path.trim_end_matches('/').rsplit_once('/') {
        Some(("", _)) | None => "/".into(),
        Some((p, _)) => p.into(),
    }
}

/// `~`-style short folder for Recent's rows.
pub fn short_dir(path: &str, workspace: Option<&str>, home: &str) -> String {
    let dir = parent(path);
    if let Some(w) = workspace.filter(|w| !w.is_empty()) {
        if dir == w {
            return "workspace".into();
        }
        if let Some(r) = dir.strip_prefix(&format!("{w}/")) {
            return format!("workspace/{r}");
        }
    }
    if dir == home {
        return "~".into();
    }
    if let Some(r) = dir.strip_prefix(&format!("{home}/")) {
        return format!("~/{r}");
    }
    dir
}

/// The explorer's Locations, grouped, as a visible list (views2): "This
/// sandbox" (Home, Workspace, Temporary files, File system root, the kit
/// logs, the sandbox's other mounts), "Shared with sbxw" (Artifacts, Recent),
/// "Worktrees" (one per thread worktree) and "Pinned" (the person's starred
/// folders). Each `{id, label, icon, path?, tab?, help, select?}`; the ids
/// of path locations are their paths, as `places` had them, so
/// `files_place` takes both.
pub fn locations(listing: &J, starred: &[String], sandbox: &str) -> Vec<J> {
    let s = |k: &str| listing[k].as_str().filter(|x| !x.is_empty()).map(str::to_string);
    let home = s("home").unwrap_or_else(|| "/home/agent".into());
    let mut this = vec![json!({ "id": home, "path": home, "label": "Home", "icon": "folder", "help": format!("{home}: the agent's home inside {sandbox}") })];
    if let Some(w) = s("workspace") {
        this.push(json!({ "id": w, "path": w, "label": "Workspace", "icon": "folder", "workspace": true,
            "help": format!("{w}: the project, a folder of this computer mounted at the same path in {sandbox}") }));
    }
    this.push(json!({ "id": "/tmp", "path": "/tmp", "label": "Temporary files", "icon": "folder", "help": format!("/tmp in {sandbox}: where pasted pictures and some uploads land") }));
    this.push(json!({ "id": "/", "path": "/", "label": "File system root", "icon": "folder", "help": format!("/ in {sandbox}: its whole file system, as the agent sees it") }));
    for m in listing["mounts"].as_array().into_iter().flatten() {
        let p = m["path"].as_str().unwrap_or("");
        if p.is_empty() {
            continue;
        }
        let ro = m["readonly"] == true;
        this.push(json!({ "id": p, "path": p, "label": p.rsplit('/').find(|c| !c.is_empty()).unwrap_or(p), "icon": "folder",
            "help": format!("{p}: a folder of this computer mounted in {sandbox} ({})", if ro { "read only" } else { "read-write" }) }));
    }
    this.push(json!({ "id": "/var/log", "path": "/var/log", "label": "Kit logs", "icon": "file", "select": "sbx-kit-startup.log", "help": format!("/var/log in {sandbox}: how its kits started") }));
    let shared = vec![
        json!({ "id": "artifacts", "tab": "artifacts", "label": "Artifacts", "icon": "sparkle", "help": "What agents deliver: documents, pictures, reports in .sbxw-artifacts" }),
        json!({ "id": "recent", "tab": "recent", "label": "Recent", "icon": "search", "help": "What was written lately, in the workspace, home and /tmp" }),
    ];
    let trees: Vec<J> = listing["worktrees"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| {
            let p = w["path"].as_str().filter(|p| !p.is_empty())?;
            let t = w["thread"].as_str().unwrap_or("?");
            Some(json!({ "id": p, "path": p, "label": t, "icon": "branch", "thread": t, "help": format!("{p}: thread {t}'s worktree") }))
        })
        .collect();
    let known: Vec<String> = this.iter().chain(&trees).filter_map(|x| x["path"].as_str().map(str::to_string)).collect();
    let pinned: Vec<J> = starred
        .iter()
        .filter(|p| !known.contains(p))
        .map(|p| json!({ "id": p, "path": p, "label": p.rsplit('/').find(|c| !c.is_empty()).unwrap_or(p), "icon": "flag", "starred": true, "help": format!("{p} (pinned)") }))
        .collect();
    let mut out = vec![json!({ "group": "This sandbox", "items": this }), json!({ "group": "Shared with sbxw", "items": shared })];
    if !trees.is_empty() {
        out.push(json!({ "group": "Worktrees", "items": trees }));
    }
    if !pinned.is_empty() {
        out.push(json!({ "group": "Pinned", "items": pinned }));
    }
    out
}

/// The location a folder is in: the deepest path location that holds it
/// (a worktree before the workspace, the workspace before the root).
pub fn location_of(locations: &[J], path: &str) -> Option<String> {
    let under = |root: &str| root == "/" || path == root || path.starts_with(&format!("{}/", root.trim_end_matches('/')));
    locations
        .iter()
        .flat_map(|g| g["items"].as_array().cloned().unwrap_or_default())
        .filter_map(|x| x["path"].as_str().filter(|p| under(p)).map(|p| (p.len(), x["id"].as_str().unwrap_or("").to_string())))
        .max_by_key(|(n, _)| *n)
        .map(|(_, id)| id)
}

/// A folder's path as pressable segments, the home folder as `~`: each
/// `{label, path}`, from the top.
pub fn crumbs(path: &str, home: &str) -> Vec<J> {
    let home = home.trim_end_matches('/');
    let (mut out, rest, mut at) = if !home.is_empty() && (path == home || path.starts_with(&format!("{home}/"))) {
        (vec![json!({ "label": "~", "path": home })], path[home.len()..].to_string(), home.to_string())
    } else {
        (vec![json!({ "label": "/", "path": "/" })], path.to_string(), String::new())
    };
    for seg in rest.split('/').filter(|s| !s.is_empty()) {
        at = format!("{at}/{seg}");
        out.push(json!({ "label": seg, "path": at }));
    }
    out
}
