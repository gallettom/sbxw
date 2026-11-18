//! The icons a screen may name: a closed set, drawn by Rune from these SVG
//! bodies (16-unit view box, stroked in the element's colour). Glyphs the web
//! UI uses as text (■ ▶ ↺ ✕ ＋ ⇌) are drawn here as icons so every platform
//! font shows them the same.

use rune::Icon;

macro_rules! icons {
    ($($id:ident $name:literal $body:expr;)*) => {
        $(static $id: Icon = Icon { name: $name, view: 16.0, body: $body };)*
        pub static ALL: &[&Icon] = &[$(&$id),*];
    };
}

// Bodies are concatenated at compile time so the stroke attributes stay in one place.
macro_rules! st {
    ($tag:literal, $attrs:literal) => {
        concat!("<", $tag, " ", $attrs, " fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.7\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>")
    };
}

icons! {
    PLAY "play" r#"<path d="M5 3.5v9l7-4.5z" fill="currentColor"/>"#;
    STOP "stop" r#"<rect x="4" y="4" width="8" height="8" rx="1" fill="currentColor"/>"#;
    RELOAD "reload" concat!(st!("path", r#"d="M13 8a5 5 0 1 1-1.5-3.6""#), st!("path", r#"d="M13 2.5v3h-3""#));
    PLUS "plus" concat!(st!("line", r#"x1="8" y1="3" x2="8" y2="13""#), st!("line", r#"x1="3" y1="8" x2="13" y2="8""#));
    CLOSE "close" concat!(st!("line", r#"x1="4" y1="4" x2="12" y2="12""#), st!("line", r#"x1="12" y1="4" x2="4" y2="12""#));
    TERMINAL "terminal" concat!(st!("rect", r#"x="1.5" y="2.5" width="13" height="11" rx="2""#), st!("polyline", r#"points="4.5,6 6.5,8 4.5,10""#), st!("line", r#"x1="8" y1="10.5" x2="11" y2="10.5""#));
    CHEVRON_DOWN "chevron-down" st!("polyline", r#"points="4,6 8,10 12,6""#);
    CHEVRON_RIGHT "chevron-right" st!("polyline", r#"points="6,4 10,8 6,12""#);
    CHECK "check" st!("polyline", r#"points="3,8.5 6.5,12 13,4.5""#);
    WARNING "warning" concat!(st!("path", r#"d="M8 2 14.5 13.5h-13z""#), st!("line", r#"x1="8" y1="6.5" x2="8" y2="9.5""#), r#"<circle cx="8" cy="11.6" r="0.9" fill="currentColor"/>"#);
    INFO "info" concat!(st!("circle", r#"cx="8" cy="8" r="6.25""#), st!("line", r#"x1="8" y1="7.5" x2="8" y2="11""#), r#"<circle cx="8" cy="5.2" r="0.9" fill="currentColor"/>"#);
    // A cog: eight square teeth round a ring and a hole (phase 3's spokes read as a sun).
    GEAR "gear" concat!(st!("path", r#"d="M12.69 6.57 L14.50 6.85 L14.50 9.15 L12.69 9.43 L12.33 10.30 L13.41 11.79 L11.79 13.41 L10.30 12.33 L9.43 12.69 L9.15 14.50 L6.85 14.50 L6.57 12.69 L5.70 12.33 L4.21 13.41 L2.59 11.79 L3.67 10.30 L3.31 9.43 L1.50 9.15 L1.50 6.85 L3.31 6.57 L3.67 5.70 L2.59 4.21 L4.21 2.59 L5.70 3.67 L6.57 3.31 L6.85 1.50 L9.15 1.50 L9.43 3.31 L10.30 3.67 L11.79 2.59 L13.41 4.21 L12.33 5.70z""#), st!("circle", r#"cx="8" cy="8" r="2""#));
    SEARCH "search" concat!(st!("circle", r#"cx="7" cy="7" r="4.5""#), st!("line", r#"x1="10.5" y1="10.5" x2="14" y2="14""#));
    FOLDER "folder" st!("path", r#"d="M1.5 4.5a1 1 0 0 1 1-1h3.5l1.5 1.5h6a1 1 0 0 1 1 1v6.5a1 1 0 0 1-1 1h-11a1 1 0 0 1-1-1z""#);
    FILE "file" concat!(st!("path", r#"d="M4 1.5h5l3.5 3.5v9.5h-8.5z""#), st!("polyline", r#"points="9,1.5 9,5 12.5,5""#));
    FLAG "flag" concat!(st!("line", r#"x1="3.5" y1="14.5" x2="3.5" y2="2""#), st!("path", r#"d="M3.5 2.5h8.5l-2 3 2 3h-8.5""#));
    PORTS "ports" concat!(st!("polyline", r#"points="10,2.5 13,5 10,7.5""#), st!("line", r#"x1="3" y1="5" x2="13" y2="5""#), st!("polyline", r#"points="6,8.5 3,11 6,13.5""#), st!("line", r#"x1="13" y1="11" x2="3" y2="11""#));
    COPY "copy" concat!(st!("rect", r#"x="5" y="5" width="9" height="9" rx="1.5""#), st!("path", r#"d="M11 5V3a1 1 0 0 0-1-1H3a1 1 0 0 0-1 1v7a1 1 0 0 0 1 1h2""#));
    TRASH "trash" concat!(st!("path", r#"d="M2.5 4h11M6 4V2.5h4V4M4 4l.7 9.5h6.6L12 4""#));
    MORE "more" r#"<circle cx="3.5" cy="8" r="1.3" fill="currentColor"/><circle cx="8" cy="8" r="1.3" fill="currentColor"/><circle cx="12.5" cy="8" r="1.3" fill="currentColor"/>"#;
    COLUMNS "columns" concat!(st!("rect", r#"x="1.5" y="2.5" width="13" height="11" rx="1.5""#), st!("line", r#"x1="8" y1="2.5" x2="8" y2="13.5""#));
    ROWS "rows" concat!(st!("rect", r#"x="1.5" y="2.5" width="13" height="11" rx="1.5""#), st!("line", r#"x1="1.5" y1="8" x2="14.5" y2="8""#));
    GRID "grid" concat!(st!("rect", r#"x="1.5" y="2.5" width="13" height="11" rx="1.5""#), st!("line", r#"x1="8" y1="2.5" x2="8" y2="13.5""#), st!("line", r#"x1="1.5" y1="8" x2="14.5" y2="8""#));
    SINGLE "single" st!("rect", r#"x="1.5" y="2.5" width="13" height="11" rx="1.5""#);
    CHAT "chat" st!("path", r#"d="M2 3.5a1 1 0 0 1 1-1h10a1 1 0 0 1 1 1v6.5a1 1 0 0 1-1 1H7l-3 2.5v-2.5H3a1 1 0 0 1-1-1z""#);
    MIC "mic" concat!(st!("rect", r#"x="5.5" y="1.5" width="5" height="8" rx="2.5""#), st!("path", r#"d="M3 7.5a5 5 0 0 0 10 0M8 12.5v2""#));
    SPEAKER "speaker" concat!(st!("path", r#"d="M2.5 6h2.5l3.5-3v10l-3.5-3h-2.5z""#), st!("path", r#"d="M11 5.5a3.5 3.5 0 0 1 0 5""#));
    SEND "send" concat!(st!("path", r#"d="M2 8l12-5.5-4.5 12-2.5-5z""#), st!("line", r#"x1="7" y1="9.5" x2="14" y2="2.5""#));
    SPARKLE "sparkle" st!("path", r#"d="M8 1.5c.6 3.3 1.9 5.2 5.5 6.5-3.6 1.3-4.9 3.2-5.5 6.5-.6-3.3-1.9-5.2-5.5-6.5 3.6-1.3 4.9-3.2 5.5-6.5z""#);
    // The status bar's: a chip with its pins, a coin, a question in a ring.
    CPU "cpu" concat!(st!("rect", r#"x="4" y="4" width="8" height="8" rx="1.5""#), st!("path", r#"d="M6.5 1.5v2.5M9.5 1.5v2.5M6.5 12v2.5M9.5 12v2.5M1.5 6.5h2.5M1.5 9.5h2.5M12 6.5h2.5M12 9.5h2.5""#));
    COIN "coin" concat!(st!("circle", r#"cx="8" cy="8" r="6.25""#), st!("path", r#"d="M10 5.8c-.4-.6-1.1-.9-2-.9-1.2 0-2 .6-2 1.5 0 2 4 1.1 4 3.2 0 .9-.9 1.5-2 1.5-.9 0-1.7-.4-2.1-1M8 3.8v1.1M8 11.1v1.1""#));
    HELP "help" concat!(st!("circle", r#"cx="8" cy="8" r="6.25""#), st!("path", r#"d="M6.2 6.2a1.9 1.9 0 0 1 3.7.6c0 1.3-1.9 1.6-1.9 2.8""#), r#"<circle cx="8" cy="11.5" r="0.9" fill="currentColor"/>"#);
    // The theme control's: a sun (light), a crescent (dark), a disc half
    // filled (match the system).
    SUN "sun" concat!(st!("circle", r#"cx="8" cy="8" r="2.75""#), st!("path", r#"d="M8 1.5v1.5M8 13v1.5M1.5 8h1.5M13 8h1.5M3.4 3.4l1.06 1.06M11.54 11.54l1.06 1.06M3.4 12.6l1.06-1.06M11.54 4.46l1.06-1.06""#));
    MOON "moon" st!("path", r#"d="M13.5 9.6A5.75 5.75 0 1 1 6.4 2.5a4.6 4.6 0 0 0 7.1 7.1z""#);
    APPEARANCE "appearance" concat!(st!("circle", r#"cx="8" cy="8" r="6.25""#), r#"<path d="M8 1.75a6.25 6.25 0 0 1 0 12.5z" fill="currentColor"/>"#);
    // A worktree: a branch off the main line.
    BRANCH "branch" concat!(st!("circle", r#"cx="4.5" cy="3.5" r="1.5""#), st!("circle", r#"cx="4.5" cy="12.5" r="1.5""#), st!("circle", r#"cx="11.5" cy="5" r="1.5""#), st!("path", r#"d="M4.5 5v6M11.5 6.5c0 3-7 2.5-7 4.5""#));
    LINK "link" concat!(st!("path", r#"d="M6.5 9.5l3-3""#), st!("path", r#"d="M7.5 4.5l1-1a2.5 2.5 0 0 1 3.5 3.5l-1 1M8.5 11.5l-1 1a2.5 2.5 0 0 1-3.5-3.5l1-1""#));
    // One mark per meaning, the same in the rail, the pane bars, the menus,
    // the panels and the palette: the browser a globe; remote control a
    // screen casting; the environment file a key; the team people; a
    // sandbox's threads one session forking into several (the worktree's
    // `branch` is the same family); network requests a shield; a shell a
    // prompt; the agents' questions a bubble with a question mark; Jarvis a
    // voice in a ring (the agent keeps the sparkle).
    // Commit history: a clock face with the arrow going back round it.
    HISTORY "history" concat!(st!("path", r#"d="M2.5 8a5.5 5.5 0 1 0 1.6-3.9""#), st!("polyline", r#"points="2.5,2.5 2.5,5.5 5.5,5.5""#), st!("polyline", r#"points="8,5 8,8 10.25,9.5""#));
    // The virtual Mac: a desktop screen on its stand (not Remote's casting screen).
    MONITOR "monitor" concat!(st!("rect", r#"x="1.5" y="2" width="13" height="9" rx="1.5""#), st!("path", r#"d="M8 11v3M5 14.25h6""#));
    GLOBE "globe" concat!(st!("circle", r#"cx="8" cy="8" r="6.25""#), st!("path", r#"d="M8 1.75c-2.6 2.6-2.6 9.9 0 12.5M8 1.75c2.6 2.6 2.6 9.9 0 12.5M1.75 8h12.5""#));
    REMOTE "remote" concat!(st!("path", r#"d="M1.75 5.5V3.75a1 1 0 0 1 1-1h10.5a1 1 0 0 1 1 1v8.5a1 1 0 0 1-1 1H10""#), st!("path", r#"d="M1.75 8.5a5 5 0 0 1 5 5M1.75 11a2.5 2.5 0 0 1 2.5 2.5""#), r#"<circle cx="2" cy="13.4" r="0.9" fill="currentColor"/>"#);
    KEY "key" concat!(st!("circle", r#"cx="4.75" cy="8" r="2.75""#), st!("path", r#"d="M7.5 8h7M12 8v2.5M14.25 8v2""#));
    PEOPLE "people" concat!(st!("circle", r#"cx="6" cy="5.25" r="2.25""#), st!("path", r#"d="M1.75 13.75c0-2.6 1.9-4.25 4.25-4.25s4.25 1.65 4.25 4.25""#), st!("path", r#"d="M10.25 3.25a2 2 0 0 1 0 4M11.75 9.6c1.5.5 2.5 1.9 2.5 4.15""#));
    THREADS "threads" concat!(st!("circle", r#"cx="8" cy="3" r="1.5""#), st!("circle", r#"cx="3.5" cy="13" r="1.5""#), st!("circle", r#"cx="8" cy="13" r="1.5""#), st!("circle", r#"cx="12.5" cy="13" r="1.5""#), st!("path", r#"d="M8 4.5v7M8 6.5c0 2.5-4.5 2.2-4.5 5M8 6.5c0 2.5 4.5 2.2 4.5 5""#));
    SHIELD "shield" st!("path", r#"d="M8 1.75l5.25 2v4c0 3.2-2.2 5.4-5.25 6.5-3.05-1.1-5.25-3.3-5.25-6.5v-4z""#);
    PROMPT "prompt" concat!(st!("polyline", r#"points="2.5,4 6.5,8 2.5,12""#), st!("line", r#"x1="8.5" y1="12.25" x2="13.5" y2="12.25""#));
    ASK "ask" concat!(st!("path", r#"d="M2 3.5a1 1 0 0 1 1-1h10a1 1 0 0 1 1 1v6.5a1 1 0 0 1-1 1H7l-3 2.5v-2.5H3a1 1 0 0 1-1-1z""#), st!("path", r#"d="M6.6 5.5a1.5 1.5 0 0 1 2.9.5c0 1-1.5 1.1-1.5 2""#), r#"<circle cx="8" cy="9.4" r="0.85" fill="currentColor"/>"#);
    JARVIS "jarvis" concat!(st!("circle", r#"cx="8" cy="8" r="6.25""#), st!("path", r#"d="M5.25 7v2M8 5v6M10.75 6.25v3.5""#));
    // The sbxw mark's prompt (the app icon's green ">", packaging/make-icon.py),
    // heavier than the set so it holds at 14 pt; the top bar draws it on the
    // icon's dark tile with the blue cursor beside it (topbar.luau).
    LOGO_PROMPT "logo-prompt" r#"<polyline points="4.5,3.5 9,8 4.5,12.5" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"/>"#;
    // The top bar's mark (topbar.luau): the same prompt flush left in its
    // box and heavier, drawn at 28 pt with the blue cursor beside it.
    LOGO_MARK "logo-mark" r#"<polyline points="1.6,3 7.4,8 1.6,13" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/>"#;
    // Team leads (the rail, the pager's mark, the palette): a crown over one
    // person, so it says "the one who leads", not the team's `people` that
    // the pane bar's Team uses. Filled crown, so it holds at 14 pt.
    LEADER "leader" concat!(r#"<path d="M4.25 5 3.75 1.25 6.25 3 8 0.75 9.75 3 12.25 1.25 11.75 5z" fill="currentColor"/>"#, st!("circle", r#"cx="8" cy="8" r="1.9""#), st!("path", r#"d="M3.25 15.25c0-2.2 2.1-3.5 4.75-3.5s4.75 1.3 4.75 3.5""#));
    // A keyword pill: a rounded label with a dot (keyword pills, emphasis.rs).
    TAG "tag" concat!(st!("rect", r#"x="1.75" y="4.75" width="12.5" height="6.5" rx="3.25""#), st!("line", r#"x1="5.5" y1="8" x2="10.5" y2="8""#));
    // The configure zone's views out of Setup, one mark each wherever it
    // shows: MCP servers a plug (a connector, not the network's shield);
    // Skills an open book (not the agent's sparkle, the team's people or
    // Jarvis's ring); Kits a packed box.
    PLUG "plug" concat!(st!("path", r#"d="M5.5 1.75v3M10.5 1.75v3""#), st!("path", r#"d="M3.5 4.75h9v2.5a4.5 4.5 0 0 1-9 0z""#), st!("path", r#"d="M8 11.75v2.5""#));
    BOOK "book" concat!(st!("path", r#"d="M8 4.25C6.5 3 4.5 2.75 1.75 2.75v9.5c2.75 0 4.75.25 6.25 1.5 1.5-1.25 3.5-1.5 6.25-1.5v-9.5C11.5 2.75 9.5 3 8 4.25z""#), st!("line", r#"x1="8" y1="4.25" x2="8" y2="13.75""#));
    BOX "box" concat!(st!("path", r#"d="M8 1.75l6 3v6.5l-6 3-6-3v-6.5z""#), st!("path", r#"d="M2 4.75l6 3 6-3M8 7.75v6.5""#));
}

pub fn get(name: &str) -> Option<&'static Icon> {
    ALL.iter().copied().find(|i| i.name == name)
}

pub fn exists(name: &str) -> bool {
    get(name).is_some()
}

pub fn names() -> Vec<&'static str> {
    ALL.iter().map(|i| i.name).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_icon_is_documented() {
        let doc = include_str!("../docs/screens.md");
        for n in super::names() {
            assert!(doc.contains(&format!("`{n}`")), "docs/screens.md does not list the icon {n}");
        }
    }
}
