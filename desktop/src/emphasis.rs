//! Keyword pills: an agent writes a short label in double brackets beside
//! what it describes (`[[Done]]`, `[[Failed: 3 tests]]`, `[[Needs you]]`);
//! the app draws it as a rounded, coloured pill. The word decides the
//! colour; only the drawing changes (the text, copy, search and what goes to
//! the program are the characters as written).
//!
//! The idea comes from pi-emphasis (github.com/johnlindquist/pi-emphasis);
//! nothing of its code or word tables is used here.
//!
//! This module is the one place that knows the grammar and the words:
//!
//! * [`find`]: the markers in a run of characters (a terminal row, a
//!   string), as indexes. Allocation-free when there is no `[[`.
//! * [`Vocab`]: the words and their [`Tone`]s: a built-in default, or the
//!   daemon's `[emphasis]` configuration ([`Vocab::from_json`]).
//! * [`Pill`] and [`palette`]: a tone's colours on a given background, made
//!   to pass the contrast test there (the terminal's own background, or the
//!   app's surface).
//!
//! Users: `terminal/pills.rs` (the grid's decoration), `screen.rs`
//! (`pills(text)` for Luau screens).
//!
//! # Grammar (emphasis-core's rule, the same in the web UI)
//!
//! The JavaScript regex `/\[\[(\p{L}(?:[\p{L}\p{N} -]{0,30}[\p{L}\p{N}])?)(?::([^\[\]\p{Cc}]{1,48}))?\]\]/gu`,
//! written by hand ([`find`]; the app has no regex crate) and held to the
//! shared cases (`fixtures/emphasis/emphasis-cases.json`):
//!
//! * `[[` WORD [`:` DETAIL] `]]` on one logical line. WORD: 1 to
//!   [`MAX_WORD`] characters, a letter first, a letter or digit last,
//!   letters, digits, spaces and hyphens. DETAIL: 1 to [`MAX_DETAIL`]
//!   characters, no `[`, `]` or control character.
//! * Left to right, never overlapping; no nesting (`[[a [[Done]] b]]` holds
//!   one marker, `[[Done]]`); `[[[Done]]]` holds `[[Done]]` from offset 1.
//! * The word is looked up lower-cased with runs of spaces made one. A word
//!   the vocabulary does not know is a pill in the unknown tone only when its
//!   first letter is upper case: `[[summary]]` (a code map's wiki link)
//!   stays plain text.
//! * In a terminal, a soft-wrapped line is matched joined and a marker cut
//!   by the wrap is drawn as one piece per row (`terminal/pills.rs`).

use crate::theme::{contrast, over};
use serde_json::Value as J;

/// The longest word, and the longest detail, in characters.
pub const MAX_WORD: usize = 32;
pub const MAX_DETAIL: usize = 48;

/// What a pill's colour says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tone {
    /// Settled: done, passed, merged.
    Green,
    /// Something is wrong: failed, broken, blocked.
    Red,
    /// Needs the person, or look here.
    Amber,
    /// Context, and any word the vocabulary does not know.
    Blue,
    /// Jarvis, the lead, the team.
    Purple,
    /// Consequences that cannot be undone or that cost: a filled red pill
    /// (the configuration's `danger`).
    Danger,
    /// The kind of work (a fix, docs, a refactor).
    Cyan,
}

pub const TONES: [Tone; 7] = [Tone::Green, Tone::Red, Tone::Amber, Tone::Blue, Tone::Purple, Tone::Danger, Tone::Cyan];

impl Tone {
    /// The tone's name in the configuration (`tone = "green"`).
    pub fn name(self) -> &'static str {
        match self {
            Tone::Green => "green",
            Tone::Red => "red",
            Tone::Amber => "amber",
            Tone::Blue => "blue",
            Tone::Purple => "purple",
            Tone::Danger => "danger",
            Tone::Cyan => "cyan",
        }
    }

    pub fn parse(s: &str) -> Option<Tone> {
        let s = s.trim().to_ascii_lowercase().replace('_', "-");
        TONES.into_iter().find(|t| t.name() == s).or(match s.as_str() {
            // Names the configuration may use for a family's look.
            "settled" | "ok" => Some(Tone::Green),
            "wrong" | "error" => Some(Tone::Red),
            "needs-you" | "warn" | "attention" => Some(Tone::Amber),
            "context" | "neutral" | "info" => Some(Tone::Blue),
            "team" | "jarvis" => Some(Tone::Purple),
            "red-solid" | "solid" | "irreversible" => Some(Tone::Danger),
            "kind" => Some(Tone::Cyan),
            _ => None,
        })
    }

    /// The tone's colour as an app token (theme.rs).
    pub fn token(self) -> &'static str {
        match self {
            Tone::Green => "green",
            Tone::Red => "red",
            Tone::Amber => "amber",
            Tone::Blue => "blue",
            Tone::Purple => "purple",
            Tone::Danger => "pill-danger",
            Tone::Cyan => "cyan",
        }
    }
}

/// One marker found, as indexes into the characters given to [`find`]:
/// `start` is the first `[`, `end` one past the last `]`; the label is
/// `start + 2 .. label_end`, its word `start + 2 .. word_end` (the label's
/// end when there is no detail).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Mark {
    pub start: usize,
    pub word_end: usize,
    pub label_end: usize,
    pub end: usize,
    pub tone: Tone,
}

/// The words and their tones.
#[derive(Clone, Debug, PartialEq)]
pub struct Vocab {
    /// Lower-case words with single spaces (synonyms are words of the same tone).
    words: Vec<(String, Tone)>,
    /// The tone of an unknown word that starts with a capital.
    pub unknown: Tone,
}

/// The built-in vocabulary, used until the daemon gives one: the families
/// of emphasis-core's default `[emphasis]` (the word, then its synonyms).
const DEFAULT: &[(Tone, &[&str])] = &[
    (Tone::Green, &["done", "verified", "fixed", "passed"]),
    (Tone::Red, &["failed", "error", "broken"]),
    (Tone::Amber, &["needs you", "blocked", "decide", "question"]),
    (Tone::Danger, &["irreversible", "destructive", "costly"]),
    (Tone::Blue, &["note", "context", "fyi"]),
    (Tone::Cyan, &["working", "in progress", "next", "plan"]),
    (Tone::Purple, &["team", "lead", "thread", "jarvis"]),
];

impl Default for Vocab {
    fn default() -> Self {
        Vocab::new(DEFAULT.iter().flat_map(|(t, ws)| ws.iter().map(move |w| (w.to_string(), *t))))
    }
}

/// A word as the vocabulary keys it: lower case, runs of spaces made one.
fn key_chars<'a>(word: impl Iterator<Item = char> + 'a) -> impl Iterator<Item = char> + 'a {
    let mut space = false;
    word.flat_map(char::to_lowercase).filter(move |c| {
        let s = *c == ' ';
        let keep = !(s && space);
        space = s;
        keep
    })
}

impl Vocab {
    pub fn new(words: impl IntoIterator<Item = (String, Tone)>) -> Vocab {
        let words = words.into_iter().map(|(w, t)| (key_chars(w.trim().chars()).collect::<String>(), t)).filter(|(w, _)| !w.is_empty()).collect();
        Vocab { words, unknown: Tone::Blue }
    }

    /// The vocabulary from the daemon's `GET /api/emphasis` (or its
    /// `emphasis` event): `words` (every word and synonym, lower case, to its
    /// tone), else `families` (`{word, tone, also: [...]}`), and
    /// `unknown_tone`. `None` when it has no word that parses (the caller
    /// keeps the default).
    pub fn from_json(v: &J) -> Option<Vocab> {
        let mut words = Vec::new();
        if let Some(m) = v.get("words").and_then(J::as_object) {
            for (w, t) in m {
                if let Some(t) = t.as_str().and_then(Tone::parse) {
                    words.push((w.clone(), t));
                }
            }
        }
        if words.is_empty() {
            for f in v.get("families").and_then(J::as_array).into_iter().flatten() {
                let Some(tone) = f.get("tone").and_then(J::as_str).and_then(Tone::parse) else { continue };
                let main = f.get("word").and_then(J::as_str).into_iter();
                let also = f.get("also").and_then(J::as_array).into_iter().flatten().filter_map(J::as_str);
                words.extend(main.chain(also).map(|w| (w.to_string(), tone)));
            }
        }
        if words.is_empty() {
            return None;
        }
        let mut v2 = Vocab::new(words);
        if let Some(t) = v.get("unknown_tone").and_then(J::as_str).and_then(Tone::parse) {
            v2.unknown = t;
        }
        Some(v2)
    }

    /// The tone of a word, or `None` when the word is not a pill (unknown
    /// and not capitalised).
    pub fn tone_of(&self, word: &[char]) -> Option<Tone> {
        match self.words.iter().find(|(w, _)| w.chars().eq(key_chars(word.iter().copied()))) {
            Some((_, t)) => Some(*t),
            None => word.first().is_some_and(|c| c.is_uppercase()).then_some(self.unknown),
        }
    }

    pub fn tone_of_str(&self, word: &str) -> Option<Tone> {
        let chars: Vec<char> = word.chars().collect();
        self.tone_of(&chars)
    }
}

/// `\p{L}` and `\p{N}`.
fn letter(c: char) -> bool {
    c.is_alphabetic()
}
fn letter_or_digit(c: char) -> bool {
    c.is_alphabetic() || c.is_numeric()
}

/// The markers in `chars`, appended to `out` in order (indexes into
/// `chars`). Allocates nothing unless a marker is found (and then only
/// `out`'s growth).
pub fn find(chars: &[char], vocab: &Vocab, out: &mut Vec<Mark>) {
    let n = chars.len();
    let mut i = 0;
    while i + 4 < n {
        if chars[i] == '[' && chars[i + 1] == '[' {
            if let Some(m) = marker_at(chars, i, vocab) {
                out.push(m);
                i = m.end;
                continue;
            }
        }
        i += 1;
    }
}

/// The marker starting at `i` (the first `[`), if one does, as the regex
/// would match it there.
fn marker_at(chars: &[char], i: usize, vocab: &Vocab) -> Option<Mark> {
    let n = chars.len();
    let a = i + 2;
    if !chars.get(a).copied().is_some_and(letter) {
        return None;
    }
    // The word: the longest run of word characters (a shorter one would be
    // followed by a word character, neither `:` nor `]`).
    let mut q = a;
    while q < n && (letter_or_digit(chars[q]) || chars[q] == ' ' || chars[q] == '-') {
        q += 1;
    }
    if q - a > MAX_WORD || !letter_or_digit(chars[q - 1]) {
        return None;
    }
    let closes = |k: usize| k + 1 < n && chars[k] == ']' && chars[k + 1] == ']';
    let label_end = match chars.get(q) {
        Some(']') if closes(q) => q,
        Some(':') => {
            let mut k = q + 1;
            while k < n && !matches!(chars[k], '[' | ']') && !chars[k].is_control() {
                k += 1;
            }
            if !(1..=MAX_DETAIL).contains(&(k - q - 1)) || !closes(k) {
                return None;
            }
            k
        }
        _ => return None,
    };
    let tone = vocab.tone_of(&chars[a..q])?;
    Some(Mark { start: i, word_end: q, label_end, end: label_end + 2, tone })
}

/// A piece of a string with markers: plain text, or a pill.
#[derive(Clone, Debug, PartialEq)]
pub enum Piece<'a> {
    Text(&'a str),
    Pill { word: &'a str, detail: Option<&'a str>, tone: Tone },
}

/// `s` cut into text and pills (the brackets dropped), for the app's own
/// screens. Never anything but text: see `screen.rs`, `pills`.
pub fn pieces<'a>(s: &'a str, vocab: &Vocab) -> Vec<Piece<'a>> {
    if !s.contains("[[") {
        return vec![Piece::Text(s)];
    }
    let idx: Vec<(usize, char)> = s.char_indices().collect();
    let chars: Vec<char> = idx.iter().map(|(_, c)| *c).collect();
    let byte = |i: usize| idx.get(i).map_or(s.len(), |(b, _)| *b);
    let mut marks = Vec::new();
    find(&chars, vocab, &mut marks);
    let mut out = Vec::new();
    let mut at = 0;
    for m in marks {
        if byte(m.start) > at {
            out.push(Piece::Text(&s[at..byte(m.start)]));
        }
        let word = &s[byte(m.start + 2)..byte(m.word_end)];
        let detail = (m.word_end < m.label_end).then(|| s[byte(m.word_end + 1)..byte(m.label_end)].trim());
        out.push(Piece::Pill { word, detail, tone: m.tone });
        at = byte(m.end);
    }
    if at < s.len() {
        out.push(Piece::Text(&s[at..]));
    }
    out
}

/// A pill's colours on one background, as 0xRRGGBB.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Pill {
    /// The pill's fill.
    pub bg: u32,
    /// The word.
    pub strong: u32,
    /// The detail after `:`.
    pub detail: u32,
    /// A hairline round the pill, or none (0: none).
    pub line: u32,
}

/// The tone's base colour, for a dark or a light background: its token in
/// the dark palette, or in the light one (the darker members of each hue),
/// whichever theme is active: the ground decides, so a light terminal in
/// the dark app (a `terminal_color` setting) still gets the light ones.
fn base(tone: Tone, light: bool) -> u32 {
    use crate::theme::{color_in, Mode};
    let m = if light { Mode::Light } else { Mode::Dark };
    color_in(m, tone.token()).or_else(|| color_in(m, "blue")).unwrap_or_default()
}

/// How much of the tone a tinted pill takes over the background under it
/// (views2: half of the first 0.20, so a pill reads as a calm label).
pub const TINT: f32 = 0.10;
/// The word's colour: the body text moved this far toward the tone (not the
/// saturated tone; regular weight, as the detail).
pub const WORD_MIX: f32 = 0.45;

// The candidates shown to the user (views2/pills/candidates-*.png), fill and
// word mix: A 0.16 / 0.65, B 0.10 / 0.45 (shipped), C 0.06 / 0.35 with a
// hairline in the tone at 45% (`palette_with(.., line = true)`).

/// The pill's height within its cell: this much air above and below (points),
/// so pills on consecutive lines never touch; its ends are half-discs.
pub const AIR: f32 = 1.5;
/// What text on a pill must reach (text under 18 pt).
pub const NEED: f64 = 4.5;

/// `c` moved toward black or white (whichever is further from `bg`) until it
/// reaches `need` against `bg`.
fn readable(c: u32, bg: u32, need: f64) -> u32 {
    let to = if crate::theme::luminance(bg) > 0.18 { crate::theme::BLACK } else { crate::theme::WHITE };
    let mut t = 0.0;
    let mut v = c;
    while contrast(v, bg) < need && t < 1.0 {
        t += 0.05;
        v = crate::terminal::theme::mix(c, to, t);
    }
    v
}

/// The pill colours of `tone` on `ground` (the terminal's background, or the
/// app surface the text sits on), with `text` the ground's body text. Tinted
/// tones: the tone over the ground at [`TINT`], the word in the tone (moved
/// until it reads at [`NEED`]), the detail in the body text (likewise).
/// [`Tone::Danger`]: a filled red with white (or black) text.
pub fn palette(tone: Tone, ground: u32, text: u32) -> Pill {
    palette_with(tone, ground, text, TINT, WORD_MIX, false)
}

/// [`palette`] with another strength (the candidates sheet).
pub fn palette_with(tone: Tone, ground: u32, text: u32, tint: f32, word_mix: f32, line: bool) -> Pill {
    let light = crate::theme::luminance(ground) > 0.18;
    let b = base(tone, light);
    if tone == Tone::Danger {
        // The fill darkened until white reads on it.
        let mut bg = b;
        let mut t = 0.0;
        let white = crate::theme::WHITE;
        while contrast(white, bg) < NEED && t < 1.0 {
            t += 0.05;
            bg = crate::terminal::theme::mix(b, crate::theme::BLACK, t);
        }
        return Pill { bg, strong: white, detail: white, line: 0 };
    }
    let bg = over(b, tint, ground);
    let word = crate::terminal::theme::mix(text, b, word_mix);
    Pill { bg, strong: readable(word, bg, NEED), detail: readable(text, bg, NEED), line: if line { over(b, 0.45, ground) } else { 0 } }
}

/// The seven tones' colours on one ground, in [`TONES`] order.
pub fn palettes(ground: u32, text: u32) -> [Pill; 7] {
    TONES.map(|t| palette(t, ground, text))
}

pub fn index(t: Tone) -> usize {
    TONES.iter().position(|x| *x == t).unwrap_or(3)
}

// ---------------------------------------------------------------------------
// The app's own screens (`text{ ..., pills = true }`, `markdown{ ..., pills = ... }`)

thread_local! {
    /// The daemon's payload and the sandboxes the app's preference turns off,
    /// as the screens' data last had them (set by the shell with the data).
    static SCREENS: std::cell::RefCell<(J, Vec<String>)> = const { std::cell::RefCell::new((J::Null, Vec::new())) };
}

/// What screens' `pills` read: the daemon's payload (`data.emphasis`) and
/// the app's preference (`data.app.kept.emphasis.off`).
pub fn set_screen_context(cfg: &J, off: Vec<String>) {
    SCREENS.with(|s| *s.borrow_mut() = (cfg.clone(), off));
}

/// The vocabulary when pills are on for `sandbox` (`""`: the global switch),
/// else `None`.
pub fn for_screen(sandbox: &str) -> Option<std::rc::Rc<Vocab>> {
    SCREENS.with(|s| {
        let s = s.borrow();
        let cfg = (!s.0.is_null()).then_some(&s.0);
        let sb = (!sandbox.is_empty()).then_some(sandbox);
        let off = sb.is_some_and(|x| s.1.iter().any(|o| o == x));
        enabled(cfg, sb, off).then(|| crate::feat::emphasis::vocab(&s.0))
    })
}

/// The pill colours on the app's surfaces (panels and dialogs are `surface`;
/// a pill's text reads on its own fill whatever is under it).
pub fn screen_palette() -> [Pill; 7] {
    let v = |n: &str| crate::theme::color_value(n).unwrap();
    palettes(v("surface"), v("text"))
}

/// `s` as rich text: its plain parts as they are, each marker as a pill
/// (the brackets dropped; a space of padding inside each end, the word in the
/// body text tinted toward the tone, the detail in the body text, both
/// regular, on the tone's faint fill; the caller rounds it into a capsule,
/// [`span_radius`]).
/// Appends to `content` and `spans` (byte offsets into `content`). Text only:
/// the result is one text element's characters and styles, so nothing in `s`
/// can make a control, a link or an intent.
pub fn rich_into(s: &str, vocab: &Vocab, palette: &[Pill; 7], content: &mut String, spans: &mut Vec<rune::Span>) {
    for p in pieces(s, vocab) {
        match p {
            Piece::Text(t) => content.push_str(t),
            Piece::Pill { word, detail, tone } => {
                let c = palette[index(tone)];
                let bg = Some(crate::theme::hex(c.bg));
                let a = content.len();
                content.push('\u{2009}');
                content.push_str(word);
                let w = content.len();
                let mut sp = rune::Span::new(a, w).color(crate::theme::hex(c.strong));
                sp.bg = bg;
                spans.push(sp);
                let d0 = content.len();
                if let Some(d) = detail {
                    content.push_str(": ");
                    content.push_str(d);
                }
                content.push('\u{2009}');
                let mut sd = rune::Span::new(d0, content.len()).color(crate::theme::hex(c.detail));
                sd.bg = bg;
                spans.push(sd);
            }
        }
    }
}

/// The corner radius that makes a text's pills capsules at `size` points
/// (Rune's span background is 1.9 cap heights tall, a cap about 0.7 em).
pub fn span_radius(size: f32) -> f32 {
    (size * 0.7 * 1.9 / 2.0).round()
}

/// The effective switch: the daemon's (global, then this sandbox's) and the
/// app's own view preference; off wins.
pub fn enabled(cfg: Option<&J>, sandbox: Option<&str>, app_off: bool) -> bool {
    if app_off {
        return false;
    }
    let Some(cfg) = cfg else { return true };
    // `sandboxes` holds the daemon's effective switch per sandbox (global
    // and per sandbox already combined); a sandbox it does not list, and the
    // app's own screens, follow `enabled`.
    if let Some(on) = sandbox.and_then(|s| cfg.get("sandboxes")?.get(s)?.as_bool()) {
        return on;
    }
    cfg.get("enabled").and_then(J::as_bool) != Some(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(s: &str) -> Vec<(String, Tone)> {
        let chars: Vec<char> = s.chars().collect();
        let mut out = vec![];
        find(&chars, &Vocab::default(), &mut out);
        out.iter().map(|m| (chars[m.start..m.end].iter().collect(), m.tone)).collect()
    }

    #[test]
    fn known_word_synonym_detail_unknown() {
        assert_eq!(marks("Tests [[Done]] now"), vec![("[[Done]]".into(), Tone::Green)]);
        assert_eq!(marks("[[passed]] and [[VERIFIED]]"), vec![("[[passed]]".into(), Tone::Green), ("[[VERIFIED]]".into(), Tone::Green)]);
        assert_eq!(marks("x [[Failed: 3 tests]]"), vec![("[[Failed: 3 tests]]".into(), Tone::Red)]);
        assert_eq!(marks("[[Needs  you]]"), vec![("[[Needs  you]]".into(), Tone::Amber)], "spaces collapsed for the lookup");
        assert_eq!(marks("[[Wibble]]"), vec![("[[Wibble]]".into(), Tone::Blue)], "unknown, capitalised: the unknown tone");
        assert!(marks("[[summary]] [[base-kit]]").is_empty(), "unknown, lower case: a wiki link, plain");
        let chars: Vec<char> = "[[Failed: 3 tests]]".chars().collect();
        let mut m = vec![];
        find(&chars, &Vocab::default(), &mut m);
        assert_eq!(chars[m[0].start + 2..m[0].word_end].iter().collect::<String>(), "Failed");
        assert_eq!(chars[m[0].word_end..m[0].label_end].iter().collect::<String>(), ": 3 tests");
    }

    #[test]
    fn what_is_not_a_marker() {
        let w = |n: usize| format!("[[D{}]]", "a".repeat(n - 1));
        assert!(marks(&w(MAX_WORD + 1)).is_empty(), "word too long");
        assert_eq!(marks(&w(MAX_WORD)).len(), 1, "exactly the limit");
        assert!(marks(&format!("[[Done:{}]]", "x".repeat(MAX_DETAIL + 1))).is_empty(), "detail too long");
        assert_eq!(marks(&format!("[[Done:{}]]", "x".repeat(MAX_DETAIL))).len(), 1);
        assert_eq!(marks("[[a [[Done]] b]]"), vec![("[[Done]]".into(), Tone::Green)], "nested: only the inner one");
        assert_eq!(marks("[[[Done]]]"), vec![("[[Done]]".into(), Tone::Green)], "from offset 1");
        assert!(marks("[[ Done]]").is_empty() && marks("[[Done ]]").is_empty() && marks("[[Done-]]").is_empty(), "ends");
        assert!(marks("[[0]]").is_empty() && marks("[[]]").is_empty(), "no letter first");
        assert!(marks("[[Done]").is_empty() && marks("[[Done").is_empty(), "not closed");
        assert!(marks("[[Failed:]]").is_empty(), "empty detail");
        assert!(marks("[[Done: a\u{7}b]]").is_empty(), "control character in the detail");
        assert!(marks("[[a=b]]").is_empty(), "not a word");
    }

    #[test]
    fn two_on_a_line_and_wide_characters() {
        assert_eq!(marks("[[Done]] then [[Failed]]").len(), 2);
        assert_eq!(marks("日本 [[Done]]").len(), 1);
        assert_eq!(marks("[[Done: 完了]]").len(), 1);
    }

    #[test]
    fn pieces_of_a_string() {
        let p = pieces("ok [[Failed: 3 tests]] then [[done]].", &Vocab::default());
        assert_eq!(
            p,
            vec![
                Piece::Text("ok "),
                Piece::Pill { word: "Failed", detail: Some("3 tests"), tone: Tone::Red },
                Piece::Text(" then "),
                Piece::Pill { word: "done", detail: None, tone: Tone::Green },
                Piece::Text("."),
            ]
        );
        assert_eq!(pieces("plain", &Vocab::default()), vec![Piece::Text("plain")]);
    }

    #[test]
    fn vocabulary_from_the_daemon() {
        let words = Vocab::from_json(&serde_json::json!({"unknown_tone": "purple", "words": {"shipped": "green", "out the door": "green", "wibble": "danger"}})).unwrap();
        assert_eq!(words.tone_of_str("Wibble"), Some(Tone::Danger));
        assert_eq!(words.tone_of_str("OUT  THE DOOR"), Some(Tone::Green));
        assert_eq!(words.tone_of_str("Done"), Some(Tone::Purple), "the daemon's list replaces the default; unknown tone from it");
        assert_eq!(words.tone_of_str("done"), None);
        let fams = Vocab::from_json(&serde_json::json!({"families": [{"word": "done", "tone": "green", "also": ["Shipped"]}]})).unwrap();
        assert_eq!(fams.tone_of_str("shipped"), Some(Tone::Green));
        assert!(Vocab::from_json(&serde_json::json!({})).is_none());
    }

    #[test]
    fn the_switch_off_wins() {
        let cfg = serde_json::json!({"enabled": true, "sandboxes": {"quiet": false, "loud": true}});
        assert!(enabled(Some(&cfg), Some("loud"), false));
        assert!(enabled(Some(&cfg), Some("other"), false), "absent: the global switch");
        assert!(!enabled(Some(&cfg), Some("quiet"), false));
        assert!(!enabled(Some(&cfg), Some("loud"), true), "the app's own preference off");
        assert!(!enabled(Some(&serde_json::json!({"enabled": false})), Some("loud"), false));
        assert!(!enabled(Some(&serde_json::json!({"enabled": false})), None, false));
        assert!(enabled(None, None, false), "no config yet: on");
    }

    /// Every tone's word and detail on its pill, on dark and light grounds.
    #[test]
    fn contrast_of_every_tone() {
        // The terminal's grounds, light and dark, and coloured lines a pill
        // sits on (a diff's green and red bands): it is tinted over those.
        let grounds = [(0x123c22, 0xc9d1d9), (0x46141a, 0xc9d1d9), (0xe6ffec, 0x24292f), (0x0d1117, 0xc9d1d9), (0x161b22, 0xc9d1d9), (0x000000, 0xffffff), (0xffffff, 0x24292f), (0xf6f8fa, 0x24292f), (0x2b2b2b, 0xc9d1d9), (0xfdf6e3, 0x657b83), (0x808080, 0x000000)];
        for (g, text) in grounds {
            for t in TONES {
                let p = palette(t, g, text);
                for fg in [p.strong, p.detail] {
                    let r = contrast(fg, p.bg);
                    assert!(r >= NEED, "{t:?} on {g:06x}: {fg:06x} on {:06x} is {r:.2}", p.bg);
                }
                if t != Tone::Danger {
                    assert_ne!(p.bg, g, "a tinted pill shows on its ground");
                }
            }
        }
    }
}

/// emphasis-core's shared cases (`crates/core/testdata/emphasis-cases.json`,
/// copied under `fixtures/emphasis/`): the web UI and the app must find
/// exactly these markers.
#[cfg(test)]
mod shared_cases {
    use super::*;

    const COPY: &str = include_str!("../fixtures/emphasis/emphasis-cases.json");

    #[test]
    fn every_shared_case() {
        let v: J = serde_json::from_str(COPY).unwrap();
        let vocab = Vocab::default();
        let mut failed = vec![];
        for case in v["cases"].as_array().unwrap() {
            let text = case["text"].as_str().unwrap();
            let chars: Vec<char> = text.chars().collect();
            let mut marks = vec![];
            find(&chars, &vocab, &mut marks);
            let got: Vec<J> = marks
                .iter()
                .map(|m| {
                    let s = |a: usize, b: usize| chars[a..b].iter().collect::<String>();
                    serde_json::json!({
                        "at": m.start, "len": m.end - m.start, "word": s(m.start + 2, m.word_end),
                        "detail": (m.word_end < m.label_end).then(|| s(m.word_end + 1, m.label_end)),
                        "tone": m.tone.name(),
                    })
                })
                .collect();
            if got != *case["markers"].as_array().unwrap() {
                failed.push(format!("{text:?}: got {got:?}, want {}", case["markers"]));
            }
        }
        assert!(failed.is_empty(), "{}", failed.join("\n"));
    }

    /// Once both branches are merged the original sits in the same checkout:
    /// the copy must be the same file.
    #[test]
    fn the_copy_is_not_stale() {
        let original = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/core/testdata/emphasis-cases.json");
        if let Ok(o) = std::fs::read_to_string(&original) {
            assert_eq!(o, COPY, "fixtures/emphasis/emphasis-cases.json differs from {}: copy it again", original.display());
        }
    }
}

#[cfg(test)]
mod screen_tests {
    use super::*;

    /// Screens' pills are capsules: the radius half the span background's height.
    #[test]
    fn screen_pills_are_capsules() {
        for size in [12.0f32, 13.0, 14.0, 16.0] {
            let h = size * 0.7 * 1.9;
            assert!((span_radius(size) - h / 2.0).abs() <= 0.5, "{size}: {} for {h}", span_radius(size));
        }
    }

    #[test]
    fn rich_text_has_the_pills_and_nothing_else() {
        let (mut content, mut spans) = (String::new(), vec![]);
        rich_into("ok [[Failed: 3 tests]] and [[summary]].", &Vocab::default(), &screen_palette(), &mut content, &mut spans);
        assert_eq!(content, "ok \u{2009}Failed: 3 tests\u{2009} and [[summary]].");
        assert_eq!(spans.len(), 2, "the word and the detail");
        assert_eq!(&content[spans[0].start as usize..spans[0].end as usize], "\u{2009}Failed");
        assert_eq!(spans[0].weight, None, "regular weight");
        assert!(spans.iter().all(|s| s.bg.is_some() && s.underline.is_none()), "styles only");
        assert_eq!(spans[0].bg, spans[1].bg, "one pill");
    }

    #[test]
    fn screens_follow_the_switch() {
        set_screen_context(&serde_json::json!({"enabled": true, "words": {"done": "green"}, "sandboxes": {"quiet": false}}), vec!["mine-off".into()]);
        assert!(for_screen("").is_some());
        assert!(for_screen("quiet").is_none(), "the daemon's off for that sandbox");
        assert!(for_screen("mine-off").is_none(), "the app's off");
        set_screen_context(&serde_json::json!({"enabled": false, "words": {"done": "green"}}), vec![]);
        assert!(for_screen("").is_none());
        set_screen_context(&J::Null, vec![]);
        assert!(for_screen("").is_some(), "no answer yet: the built-in vocabulary");
    }

    /// Each tone's word and detail on its pill read at 4.5:1 (the pill is
    /// opaque, so whatever surface it sits on).
    #[test]
    fn screen_pills_read() {
        for p in screen_palette() {
            assert!(contrast(p.strong, p.bg) >= NEED && contrast(p.detail, p.bg) >= NEED);
        }
    }
}

#[cfg(test)]
mod palette_print {
    /// `cargo test palette_values -- --ignored --nocapture`: each tone's
    /// fill, word and detail on the web UI's dark and light grounds (what
    /// the web UI matches).
    #[test]
    #[ignore]
    fn palette_values() {
        for (g, t) in [(0x0d1117u32, 0xc9d1d9u32), (0xffffff, 0x24292f)] {
            for tone in super::TONES {
                let p = super::palette(tone, g, t);
                println!("{g:06x} {:?}: fill #{:06x} word #{:06x} detail #{:06x}", tone, p.bg, p.strong, p.detail);
            }
        }
    }
}
