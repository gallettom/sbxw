//! The one list of invisible and bidirectional characters sbxw never lets
//! through: every place that strips or refuses them asks [`is_invisible`]
//! (`docs/security.md#invisible-characters`). Each place keeps its own action
//! (remove, refuse, replace) and its own rule for control characters
//! (`char::is_control`, with or without line feed and tab); this list is what
//! they all share on top of that.
//!
//! It is the union of what the places held before (2026-11): every format
//! character (Unicode category Cf, as of Unicode 17), the line and paragraph
//! separators, and the characters that draw nothing (combining grapheme
//! joiner, Hangul fillers, Khmer inherent vowels, Mongolian selectors,
//! variation selectors, the whole tag and variation-selector plane block).
//! The phone page holds the same list (`RC_INVISIBLE_RANGES` in
//! `assets/remote/remote-core.js`); a JS test compares the two.

/// Inclusive ranges, sorted, one per line (the JS test reads this table).
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
    ('\u{fe00}', '\u{fe0f}'),     // variation selectors
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

/// Every character of [`INVISIBLE`], for the tests of each place (the
/// daemon's included).
pub fn all() -> impl Iterator<Item = char> {
    INVISIBLE.iter().flat_map(|&(a, b)| a..=b)
}

/// The characters just outside each range, which no place may touch for
/// being invisible: the boundaries the tests check.
pub fn neighbours() -> impl Iterator<Item = char> {
    INVISIBLE
        .iter()
        .flat_map(|&(a, b)| [char::from_u32(a as u32 - 1), char::from_u32(b as u32 + 1)])
        .flatten()
        .filter(|c| !is_invisible(*c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_are_sorted_and_apart() {
        for w in INVISIBLE.windows(2) {
            assert!(w[0].0 <= w[0].1 && (w[0].1 as u32) + 1 < w[1].0 as u32, "{w:?}");
        }
        assert!(all().all(|c| !c.is_whitespace() || c == '\u{2028}' || c == '\u{2029}'));
        assert!(neighbours().count() >= 30);
    }

    /// Every place that strips or refuses invisible characters handles every
    /// character of the list with its own action, and leaves the characters
    /// around each range alone.
    #[test]
    fn every_place_handles_every_character() {
        use crate::{canvas, mac, remote_access, terminal, tickets, uploads};
        for c in all() {
            let s = format!("a{c}b");
            assert_eq!(terminal::sanitize_typed(&s), "ab", "sanitize_typed removes {c:?}");
            assert!(terminal::is_unsafe_typed_char(c), "{c:?}");
            assert_eq!(mac::clean_reason(&s), "a b", "clean_reason breaks the word at {c:?}");
            assert_eq!(canvas::clean(&s, 99, false), "ab", "canvas::clean removes {c:?}");
            assert_eq!(canvas::clean(&s, 99, true), "ab", "canvas::clean (lines) removes {c:?}");
            assert_eq!(tickets::clean(&s, 99, true), "ab", "tickets::clean removes {c:?}");
            assert_eq!(remote_access::check_message(&s), Err(remote_access::MessageRefusal::Control), "check_message refuses {c:?}");
            assert_eq!(remote_access::clean_device_name(&s), None, "clean_device_name refuses {c:?}");
            assert_eq!(uploads::upload_name(&s), "a_b", "upload_name replaces {c:?}");
        }
        for c in neighbours() {
            let s = format!("a{c}b");
            let control = c.is_control();
            assert_eq!(terminal::sanitize_typed(&s), s, "{c:?} kept");
            if c.is_whitespace() {
                assert_eq!(mac::clean_reason(&s), "a b", "{c:?} is white space");
            } else {
                assert_eq!(mac::clean_reason(&s), s, "{c:?} kept");
            }
            assert!(!control, "{c:?}");
            assert_eq!(canvas::clean(&s, 99, true), s, "{c:?} kept");
            assert_eq!(tickets::clean(&s, 99, true), s, "{c:?} kept");
            assert_eq!(remote_access::check_message(&s), Ok(()), "{c:?} accepted");
            assert_eq!(remote_access::clean_device_name(&s).as_deref(), Some(s.as_str()), "{c:?} accepted");
        }
    }
}
