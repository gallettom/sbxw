//! The core's rules for files that land on the person's machine: the name a
//! download is written under ([`file_name`], [`create_in`]) and whether the
//! app may open it with the system's default app ([`verdict`]). Security
//! rules: they live here, once, and the core applies them itself in its
//! downloads, opens and uploads (`src/files.rs`). A name or a verdict a
//! feature computed is never trusted.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

/// Longest name, UTF-8 bytes (the stem is cut, the extension kept).
pub const MAX_NAME: usize = 200;

/// The one list of invisible and bidirectional characters
/// (`docs/security.md#invisible-characters`): a copy of `INVISIBLE` in
/// `crates/core/src/invisible.rs`, which the desktop crate does not depend
/// on. A test compares the two tables line for line. U+FE0E and U+FE0F are
/// left out on purpose, so emoji like ❤️ pass. Every place of the desktop that
/// strips, refuses or shows invisible characters asks [`is_invisible`], each
/// with its own action and its own rule for control characters.
/// Inclusive ranges, sorted, one per line (the tests read this table).
pub const INVISIBLE: &[(char, char)] = &[
    ('\u{00ad}', '\u{00ad}'),     // soft hyphen
    ('\u{034f}', '\u{034f}'),     // combining grapheme joiner
    ('\u{0600}', '\u{0605}'),     // Arabic number signs
    ('\u{061c}', '\u{061c}'),     // Arabic letter mark
    ('\u{06dd}', '\u{06dd}'),     // Arabic end of ayah
    ('\u{070f}', '\u{070f}'),     // Syriac abbreviation mark
    ('\u{0890}', '\u{0891}'),     // Arabic pound and piastre marks above
    ('\u{08e2}', '\u{08e2}'),     // Arabic disputed end of ayah
    ('\u{115f}', '\u{1160}'),     // Hangul choseong and jungseong fillers
    ('\u{17b4}', '\u{17b5}'),     // Khmer inherent vowels
    ('\u{180b}', '\u{180f}'),     // Mongolian variation selectors and vowel separator
    ('\u{200b}', '\u{200f}'),     // zero-width space, ZWNJ, ZWJ, LRM, RLM
    ('\u{2028}', '\u{2029}'),     // line and paragraph separators
    ('\u{202a}', '\u{202e}'),     // bidi embeddings and overrides
    ('\u{2060}', '\u{206f}'),     // word joiner, invisible operators, bidi isolates, deprecated formats
    ('\u{3164}', '\u{3164}'),     // Hangul filler
    ('\u{fe00}', '\u{fe0d}'),     // variation selectors 1 to 14 (not 15 and 16, see below)
    ('\u{feff}', '\u{feff}'),     // byte order mark, zero-width no-break space
    ('\u{ffa0}', '\u{ffa0}'),     // half-width Hangul filler
    ('\u{fff9}', '\u{fffb}'),     // interlinear annotation
    ('\u{110bd}', '\u{110bd}'),   // Kaithi number sign
    ('\u{110cd}', '\u{110cd}'),   // Kaithi number sign above
    ('\u{13430}', '\u{1343f}'),   // Egyptian hieroglyph format controls
    ('\u{1bca0}', '\u{1bca3}'),   // shorthand format controls
    ('\u{1d173}', '\u{1d17a}'),   // musical symbol format controls
    ('\u{e0000}', '\u{e0fff}'),   // tags and variation selectors supplement
];

/// Is `c` one of [`INVISIBLE`]: a character that hides, reorders or changes
/// what text shows without showing itself?
pub fn is_invisible(c: char) -> bool {
    INVISIBLE.iter().any(|&(a, b)| (a..=b).contains(&c))
}

/// Every character of [`INVISIBLE`], for the tests of each place.
pub fn invisible_chars() -> impl Iterator<Item = char> {
    INVISIBLE.iter().flat_map(|&(a, b)| a..=b)
}

/// The characters just outside each range of [`INVISIBLE`], which no place
/// may touch for being invisible: the boundaries the tests check.
#[cfg(test)]
pub fn invisible_neighbours() -> Vec<char> {
    INVISIBLE
        .iter()
        .flat_map(|&(a, b)| [char::from_u32(a as u32 - 1), char::from_u32(b as u32 + 1)])
        .flatten()
        .filter(|c| !is_invisible(*c))
        .collect()
}

/// Replaced with `_`: control characters, what Windows refuses in a name,
/// and slash lookalikes.
fn replaced(c: char) -> bool {
    c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\u{2215}' | '\u{2044}' | '\u{FF0F}' | '\u{FF3C}' | '\u{29F8}' | '\u{29F9}')
}

/// Windows device names, with any extension (`con.txt`, `NUL.tar.gz`).
fn reserved_stem(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    let stem = stem.replace(['¹', '²', '³'], "1");
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") {
        return true;
    }
    let b = stem.as_bytes();
    b.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && b[3].is_ascii_digit()
}

/// The name a file from a sandbox is written under: no folder, no traversal,
/// no hidden file, nothing Windows refuses, no invisible reordering, at most
/// [`MAX_NAME`] bytes. Never empty (`download`).
/// Composed first (NFC), so a decomposed accent becomes one character and
/// every rule after sees the name as it will be shown.
pub fn file_name(raw: &str) -> String {
    let nfc = icu_normalizer::ComposingNormalizerBorrowed::new_nfc().normalize(raw);
    let last = nfc.rsplit(['/', '\\']).next().unwrap_or("");
    let mut s: String = last.chars().filter(|c| !is_invisible(*c)).map(|c| if replaced(c) { '_' } else { c }).collect();
    s = s.trim_start_matches(['.', ' ']).trim_end_matches(['.', ' ']).to_string();
    if s.is_empty() {
        return "download".into();
    }
    if reserved_stem(&s) {
        s.insert(0, '_');
    }
    cap(&s, MAX_NAME)
}

/// `name` cut to `max` bytes at a character boundary, the extension kept.
fn cap(name: &str, max: usize) -> String {
    if name.len() <= max {
        return name.to_string();
    }
    let (stem, ext) = split_ext(name);
    let ext = if ext.len() + 1 < max / 2 { ext } else { "" };
    let room = max - if ext.is_empty() { 0 } else { ext.len() + 1 };
    let mut cut = room.min(stem.len());
    while !stem.is_char_boundary(cut) {
        cut -= 1;
    }
    if ext.is_empty() {
        stem[..cut].to_string()
    } else {
        format!("{}.{ext}", &stem[..cut])
    }
}

/// `(stem, extension)`: the last dot, not a leading one.
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i + 1..]),
        _ => (name, ""),
    }
}

/// Creates a new file for `name` (already through [`file_name`]) in `dir`:
/// never over an existing file or through a symlink (`create_new`), `name
/// (1).ext` up to 99, then a timestamp. Mode 0644. The path is checked to
/// be directly in `dir`.
pub fn create_in(dir: &Path, name: &str) -> io::Result<(PathBuf, File)> {
    let name = file_name(name);
    let (stem, ext) = split_ext(&name);
    let dot = if ext.is_empty() { String::new() } else { format!(".{ext}") };
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let candidates = std::iter::once(name.clone()).chain((1..=99).map(|i| format!("{stem} ({i}){dot}"))).chain(std::iter::once(format!("{stem} {stamp}{dot}")));
    for n in candidates {
        let n = cap(&n, MAX_NAME + 12);
        let p = dir.join(&n);
        if p.parent() != Some(dir) || p.file_name().and_then(|f| f.to_str()) != Some(n.as_str()) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{n}: not a plain file name")));
        }
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o644);
        }
        match o.open(&p) {
            Ok(f) => return Ok((p, f)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{name}: every name is taken")))
}

/// Marks a file the app downloaded as coming from elsewhere (macOS
/// Gatekeeper's `com.apple.quarantine`, through `xattr`): opening it asks
/// first as a browser download would. Nothing elsewhere.
pub fn quarantine(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let v = format!("0081;{t:x};sbxw;");
        match std::process::Command::new("/usr/bin/xattr").arg("-w").arg("com.apple.quarantine").arg(v).arg(path).status() {
            Ok(s) if s.success() => {}
            other => log::warn!("quarantine {}: {other:?}", path.display()),
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = path;
}

// ---------------------------------------------------------------------------
// Opening by type

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Opens after the Confirm naming the file, its size and type.
    Plain,
    /// Opens only after the Confirm says why and the person ticks the box.
    Warn,
    /// Never opened by the app: download and show in its folder only.
    Refuse,
}

impl Verdict {
    pub fn name(self) -> &'static str {
        match self {
            Verdict::Plain => "plain",
            Verdict::Warn => "warn",
            Verdict::Refuse => "refuse",
        }
    }
}

const REFUSE: &[&str] = &[
    // macOS
    "app", "command", "tool", "terminal", "term", "pkg", "mpkg", "dmg", "workflow", "action", "scpt", "scptd", "applescript", "osax", "kext", "prefpane", "saver", "plugin", "bundle", "dylib", "webloc", "inetloc", "fileloc", "mobileconfig", "shortcut", "terminal", "qlgenerator", "mdimporter", "xpc", "systemextension",
    // Windows
    "exe", "com", "bat", "cmd", "ps1", "psm1", "psd1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "wsc", "hta", "msi", "msp", "mst", "msix", "msixbundle", "appx", "appxbundle", "scr", "pif", "cpl", "lnk", "url", "reg", "inf", "sys", "dll", "gadget", "application", "appref-ms", "chm", "scf", "library-ms", "search-ms", "settingcontent-ms", "xll", "iso", "img", "vhd", "vhdx", "mjs", "cjs",
    // Unix
    "sh", "bash", "zsh", "fish", "csh", "ksh", "run", "bin", "out", "elf", "appimage", "desktop", "deb", "rpm", "snap", "flatpak", "flatpakref", "py", "pyc", "pyw", "pl", "rb", "php", "lua", "tcl", "so", "ko",
    // Java
    "jar", "jnlp", "class",
];

const WARN: &[(&str, &str)] = &[
    ("html", "HTML page"), ("htm", "HTML page"), ("xhtml", "HTML page"), ("shtml", "HTML page"), ("mht", "web archive"), ("mhtml", "web archive"),
    ("svg", "SVG picture"), ("xml", "XML document"), ("xsl", "XSL stylesheet"),
    ("doc", "Word document (old format)"), ("xls", "Excel workbook (old format)"), ("ppt", "PowerPoint deck (old format)"),
    ("docm", "Word document with macros"), ("xlsm", "Excel workbook with macros"), ("pptm", "PowerPoint deck with macros"), ("dotm", "Word template with macros"), ("xltm", "Excel template with macros"), ("potm", "PowerPoint template with macros"), ("ppsm", "PowerPoint show with macros"),
    ("ps", "PostScript"), ("eps", "PostScript"),
    ("zip", "zip archive"), ("tar", "tar archive"), ("gz", "gzip archive"), ("tgz", "tar archive"), ("bz2", "bzip2 archive"), ("xz", "xz archive"), ("7z", "7-Zip archive"), ("rar", "RAR archive"),
    ("ics", "calendar file"), ("vcf", "contact card"),
];

const PLAIN: &[(&str, &str)] = &[
    ("md", "Markdown document"), ("markdown", "Markdown document"), ("txt", "text file"), ("csv", "CSV table"), ("tsv", "TSV table"), ("json", "JSON file"), ("log", "log file"),
    ("png", "PNG picture"), ("jpg", "JPEG picture"), ("jpeg", "JPEG picture"), ("gif", "GIF picture"), ("webp", "WebP picture"), ("bmp", "BMP picture"), ("tif", "TIFF picture"), ("tiff", "TIFF picture"), ("heic", "HEIC picture"),
    ("pdf", "PDF document"), ("docx", "Word document"), ("xlsx", "Excel workbook"), ("pptx", "PowerPoint deck"), ("odt", "OpenDocument text"), ("ods", "OpenDocument spreadsheet"), ("odp", "OpenDocument presentation"), ("rtf", "RTF document"),
    ("mp3", "MP3 audio"), ("wav", "WAV audio"), ("m4a", "M4A audio"), ("flac", "FLAC audio"), ("ogg", "Ogg audio"), ("mp4", "MP4 video"), ("m4v", "M4V video"), ("mov", "QuickTime video"), ("webm", "WebM video"),
];

/// What the first bytes say, when they are a program: its kind.
fn sniff_program(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(b"#!") {
        return Some("script (starts with #!)");
    }
    if head.starts_with(b"MZ") {
        return Some("Windows program");
    }
    if head.starts_with(b"\x7fELF") {
        return Some("Linux program");
    }
    const MACHO: &[[u8; 4]] = &[[0xFE, 0xED, 0xFA, 0xCE], [0xFE, 0xED, 0xFA, 0xCF], [0xCE, 0xFA, 0xED, 0xFE], [0xCF, 0xFA, 0xED, 0xFE], [0xCA, 0xFE, 0xBA, 0xBE]];
    if head.len() >= 4 && MACHO.iter().any(|m| head[..4] == *m) {
        return Some("macOS program");
    }
    None
}

/// For a plain type whose content is cheap to check: whether the first
/// bytes agree. `None`: nothing to check.
fn content_agrees(ext: &str, head: &[u8]) -> Option<bool> {
    let zip = head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06");
    Some(match ext {
        "pdf" => head.starts_with(b"%PDF"),
        "png" => head.starts_with(b"\x89PNG\r\n\x1a\n"),
        "jpg" | "jpeg" => head.starts_with(&[0xFF, 0xD8, 0xFF]),
        "gif" => head.starts_with(b"GIF8"),
        "webp" => head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP",
        "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp" => zip,
        _ => return None,
    })
}

/// Whether the app may open a file named `name` (any name: it goes through
/// [`file_name`] first) whose first bytes are `head` (at least 16 when the
/// file has them): the verdict, a readable type, and why.
pub fn verdict(name: &str, head: &[u8]) -> (Verdict, String, String) {
    let name = file_name(name);
    let (_, ext) = split_ext(&name);
    let ext = ext.to_ascii_lowercase();
    if let Some(kind) = sniff_program(head) {
        return (Verdict::Refuse, kind.to_string(), format!("its content is a {kind}, whatever its name says: the app does not open programs"));
    }
    if REFUSE.contains(&ext.as_str()) {
        return (Verdict::Refuse, format!(".{ext} file"), format!("a .{ext} file can run code on this machine: the app does not open it; it is in your folder"));
    }
    if let Some((_, kind)) = WARN.iter().find(|(e, _)| *e == ext) {
        let why = match ext.as_str() {
            "html" | "htm" | "xhtml" | "shtml" | "mht" | "mhtml" | "svg" | "xml" | "xsl" => "a browser runs the scripts in a local page",
            "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" => "unpacking it may produce programs",
            "ics" | "vcf" => "opening it imports it into your Calendar or Contacts",
            "ps" | "eps" => "PostScript is a programming language",
            _ => "it may contain macros",
        };
        return (Verdict::Warn, kind.to_string(), format!("{why}"));
    }
    if let Some((_, kind)) = PLAIN.iter().find(|(e, _)| *e == ext) {
        if content_agrees(&ext, head) == Some(false) {
            return (Verdict::Warn, kind.to_string(), format!("it is named .{ext} but its content is not a {kind}; the system decides what opens it"));
        }
        return (Verdict::Plain, kind.to_string(), String::new());
    }
    let kind = if ext.is_empty() { "file with no extension".to_string() } else { format!(".{ext} file") };
    (Verdict::Warn, kind, "the system decides what opens a file of this type".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table of `INVISIBLE` (from `pub const INVISIBLE` to its `];`).
    fn table(src: &str) -> &str {
        let start = src.find("pub const INVISIBLE").expect("INVISIBLE");
        let end = start + src[start..].find("];").expect("end of INVISIBLE");
        &src[start..end]
    }

    /// The desktop's copy is the core's list, line for line, comments too.
    #[test]
    fn invisible_is_the_core_list() {
        let core_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../crates/core/src/invisible.rs");
        let core = std::fs::read_to_string(core_path).expect("crates/core/src/invisible.rs");
        let ours = include_str!("safe.rs");
        assert_eq!(table(ours), table(&core), "copy INVISIBLE again from crates/core/src/invisible.rs");
        for w in INVISIBLE.windows(2) {
            assert!(w[0].0 <= w[0].1 && w[0].1 < w[1].0, "{w:?}");
        }
        assert!(!is_invisible('\u{fe0e}') && !is_invisible('\u{fe0f}'));
        assert!(invisible_chars().count() > 4000);
    }

    #[test]
    fn file_name_removes_every_invisible_character() {
        for c in invisible_chars() {
            assert_eq!(file_name(&format!("a{c}b.txt")), "ab.txt", "{c:?} removed");
        }
        for s in ["\u{2764}\u{fe0f} ok.txt", "\u{2764}\u{fe0e} ok.txt", "a\u{fe0f}b.txt", "a\u{fe0e}b.txt"] {
            assert_eq!(file_name(s), s, "{s:?} kept");
        }
        let nfc = icu_normalizer::ComposingNormalizerBorrowed::new_nfc();
        for c in invisible_neighbours() {
            let s = format!("a{c}b.txt");
            assert_eq!(file_name(&s), nfc.normalize(&s), "{c:?} kept");
        }
    }

    #[test]
    fn names_are_made_safe() {
        let a300 = format!("{}.md", "a".repeat(300));
        let a197 = format!("{}.md", "a".repeat(197));
        let cases: &[(&str, &str)] = &[
            ("../../etc/passwd", "passwd"),
            ("..\\..\\Windows\\win.ini", "win.ini"),
            ("/abs/report.md", "report.md"),
            (".bashrc", "bashrc"),
            ("...", "download"),
            ("", "download"),
            ("CON", "_CON"),
            ("con.txt", "_con.txt"),
            ("LPT1.log", "_LPT1.log"),
            ("NUL.tar.gz", "_NUL.tar.gz"),
            ("COM10.txt", "COM10.txt"),
            ("COM¹.txt", "_COM¹.txt"),
            ("report.pdf.", "report.pdf"),
            ("report.pdf   ", "report.pdf"),
            ("a<b>c:d\"e|f?g*h.txt", "a_b_c_d_e_f_g_h.txt"),
            ("evil\u{202E}txt.exe", "eviltxt.exe"),
            ("line\nbreak.md", "line_break.md"),
            ("file.txt:stream", "file.txt_stream"),
            ("a\u{2215}b.md", "a_b.md"),
            ("日本語.md", "日本語.md"),
            // NFC: a decomposed accent composes, then the other rules apply.
            ("Re\u{0301}sume\u{0301}.pdf", "Résumé.pdf"),
            (" .Cafe\u{0301} ", "Café"),
            ("CO\u{0301}N.txt", "CÓN.txt"),
            ("NU\u{0301}L.md", "NÚL.md"),
            ("con\u{200E}.txt", "_con.txt"),
            (&a300, &a197),
        ];
        for (i, o) in cases {
            assert_eq!(file_name(i), *o, "{i:?}");
        }
        // A long multi-byte name is cut at a character boundary.
        let long = format!("{}.md", "日".repeat(100));
        let n = file_name(&long);
        assert!(n.len() <= MAX_NAME && n.ends_with(".md"), "{n}");
    }

    #[test]
    fn a_taken_name_gets_a_number_and_nothing_is_overwritten() {
        let dir = std::env::temp_dir().join(format!("sbxw-safe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("report.md"), "mine").unwrap();
        let (p, _) = create_in(&dir, "report.md").unwrap();
        assert_eq!(p, dir.join("report (1).md"));
        let (p2, _) = create_in(&dir, "../report.md").unwrap();
        assert_eq!(p2, dir.join("report (2).md"));
        assert_eq!(std::fs::read_to_string(dir.join("report.md")).unwrap(), "mine");
        #[cfg(unix)]
        {
            // A dangling symlink under the name is never written through.
            std::os::unix::fs::symlink(dir.join("nowhere"), dir.join("link.txt")).unwrap();
            let (p3, _) = create_in(&dir, "link.txt").unwrap();
            assert_eq!(p3, dir.join("link (1).txt"));
            assert!(!dir.join("nowhere").exists());
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p3).unwrap().permissions().mode() & 0o111, 0, "never executable");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn what_opens_by_type_and_content() {
        let text = b"hello world, plain text here";
        let cases: &[(&str, &[u8], Verdict)] = &[
            ("report.md", text, Verdict::Plain),
            ("REPORT.PDF", b"%PDF-1.7 ...", Verdict::Plain),
            ("setup.exe", b"MZ..", Verdict::Refuse),
            ("photo.png.exe", text, Verdict::Refuse),
            ("invoice.pdf.lnk", text, Verdict::Refuse),
            ("Install.command", text, Verdict::Refuse),
            ("run.sh", text, Verdict::Refuse),
            ("tool.AppImage", text, Verdict::Refuse),
            ("x.desktop", text, Verdict::Refuse),
            ("a.webloc", text, Verdict::Refuse),
            ("a.url", text, Verdict::Refuse),
            ("evil\u{202E}gnp.exe", text, Verdict::Refuse),
            ("page.html", text, Verdict::Warn),
            ("diagram.svg", text, Verdict::Warn),
            ("budget.xlsm", text, Verdict::Warn),
            ("a.zip", b"PK\x03\x04", Verdict::Warn),
            ("notes", text, Verdict::Warn),
            ("notes", b"#!/bin/sh\necho", Verdict::Refuse),
            ("report.pdf", b"MZ\x90\x00", Verdict::Refuse),
            ("report.pdf", b"\x7fELF\x02\x01", Verdict::Refuse),
            ("report.pdf", &[0xCF, 0xFA, 0xED, 0xFE, 7], Verdict::Refuse),
            ("file.", text, Verdict::Warn),
            ("data.csv", b"a,b\n1,2", Verdict::Plain),
            // Named as a document, not one: downgraded with the reason.
            ("report.pdf", text, Verdict::Warn),
            ("shot.png", b"GIF89a", Verdict::Warn),
            ("deck.docx", text, Verdict::Warn),
            ("deck.docx", b"PK\x03\x04rest", Verdict::Plain),
        ];
        for (n, head, want) in cases {
            let (v, kind, why) = verdict(n, head);
            assert_eq!(v, *want, "{n:?}: {kind} / {why}");
            assert!(!kind.is_empty());
            if v != Verdict::Plain {
                assert!(!why.is_empty(), "{n}: a reason");
            }
        }
        assert_eq!(verdict("report.md", text).1, "Markdown document");
        assert_eq!(verdict("page.html", text).1, "HTML page");
    }
}
