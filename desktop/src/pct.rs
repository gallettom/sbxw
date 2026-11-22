//! Percent-decoding (`%xx`) of paths, query values and link addresses.
//!
//! One decoder for the whole app. It works on bytes and never panics:
//! a malformed escape (`%` at the end, `%2`, `%zz`, `%+1`, `%` before a
//! multi-byte character) is kept as literal text. A `+` turns into a space
//! only where the caller asks (query values).
//!
//! Invalid UTF-8 after decoding (`%FF`) is the caller's choice: [`decode`]
//! replaces it (U+FFFD, what most call sites always did) and [`decode_strict`]
//! refuses it with `None` (what the file reader's links always did).

/// The decoded bytes. `plus_is_space`: `+` becomes a space (form-encoded query
/// values); an escaped `%2B` stays a `+` either way.
pub fn decode_bytes(s: &str, plus_is_space: bool) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'%' {
            if let (Some(h), Some(l)) = (b.get(i + 1).and_then(|&x| hex(x)), b.get(i + 2).and_then(|&x| hex(x))) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(if plus_is_space && c == b'+' { b' ' } else { c });
        i += 1;
    }
    out
}

/// Decoded, invalid UTF-8 replaced with U+FFFD.
pub fn decode(s: &str, plus_is_space: bool) -> String {
    String::from_utf8_lossy(&decode_bytes(s, plus_is_space)).into_owned()
}

/// Decoded; `None` when the result is not valid UTF-8.
pub fn decode_strict(s: &str, plus_is_space: bool) -> Option<String> {
    String::from_utf8(decode_bytes(s, plus_is_space)).ok()
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    //! The eight copies this module replaced, kept verbatim (as `old_*`) to
    //! pin their behaviour: identical on valid input, and what changed on
    //! malformed input.
    use super::*;

    /// `feat/mod.rs` (query values): `+` is a space, lossy.
    fn old_feat_mod(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'%' if i + 2 < b.len() => match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                        continue;
                    }
                    Err(_) => out.push(b'%'),
                },
                b'+' => out.push(b' '),
                c => out.push(c),
            }
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// `feat/create/standin.rs`: `+` is a space, slices the `&str`.
    fn old_create_standin(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = vec![];
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' && i + 2 < b.len() {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
            }
            out.push(if b[i] == b'+' { b' ' } else { b[i] });
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// `feat/setup/standin.rs`, `feat/browser/mod.rs` (`decode_seg`) and
    /// `bin/standin.rs`: byte-identical bodies, slice the `&str`.
    fn old_str_slice(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' && i + 2 < b.len() {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
            }
            out.push(b[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// `feat/monitor/mod.rs` (`pct_decode`): off-by-one bound.
    fn old_monitor(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' && i + 2 < b.len() + 1 {
                if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    out.push(v);
                    i += 3;
                    continue;
                }
            }
            out.push(b[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// `feat/files/codemap.rs` (`dec`) and, before its strict UTF-8 step,
    /// `feat/files/explorer.rs`: byte slices, no panic.
    fn old_bytes(s: &str) -> Vec<u8> {
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' && i + 2 < b.len() {
                if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    out.push(v);
                    i += 3;
                    continue;
                }
            }
            out.push(b[i]);
            i += 1;
        }
        out
    }
    fn old_codemap(s: &str) -> String {
        String::from_utf8_lossy(&old_bytes(s)).into_owned()
    }
    fn old_explorer(s: &str) -> Option<String> {
        String::from_utf8(old_bytes(s)).ok()
    }

    /// Well-formed input: every escape is `%` and two hex digits.
    const VALID: &[&str] = &[
        "",
        "plain",
        "a b",
        "a+b",
        "%20",
        "a%20b",
        "%2B",
        "%2b",
        "%2F",
        "/x/My%20Doc.md",
        "%41%42%43",
        "trail%41",
        "%25",
        "%2520",
        "%C3%A9t%C3%A9",
        "%E2%82%AC",
        "%F0%9F%98%80",
        "été",
        "€ and 😀",
        "%00",
        "%7f",
        "%FF",
        "%C3",
        "%C3%28",
        "sandbox-name_1.2",
        "q=1&r=2",
    ];

    #[test]
    fn valid_input_is_unchanged_at_every_call_site() {
        for s in VALID {
            assert_eq!(decode(s, true), old_feat_mod(s), "feat/mod {s:?}");
            assert_eq!(decode(s, true), old_create_standin(s), "create/standin {s:?}");
            assert_eq!(decode(s, false), old_str_slice(s), "setup, browser, bin standin {s:?}");
            assert_eq!(decode(s, false), old_monitor(s), "monitor {s:?}");
            assert_eq!(decode(s, false), old_codemap(s), "codemap {s:?}");
            assert_eq!(decode_strict(s, false), old_explorer(s), "explorer {s:?}");
        }
    }

    #[test]
    fn some_valid_results() {
        assert_eq!(decode("a+b%2Bc", true), "a b+c");
        assert_eq!(decode("a+b%2Bc", false), "a+b+c");
        assert_eq!(decode("%C3%A9", false), "é");
        assert_eq!(decode("%FF", false), "\u{FFFD}");
        assert_eq!(decode_strict("%FF", false), None);
        assert_eq!(decode_strict("%2e%2E", false).as_deref(), Some(".."));
    }

    /// Malformed input whose result did not change anywhere (where the old
    /// copy did not panic).
    #[test]
    fn malformed_input_kept_as_literal() {
        let same = ["%", "a%", "%2", "a%4", "%zz", "%g1", "%1g", "%%41", "%é", "%éa", "100%", "%-1", "% 1"];
        for s in same {
            assert_eq!(decode(s, true), old_feat_mod(s), "feat/mod {s:?}");
            assert_eq!(decode(s, false), old_codemap(s), "codemap {s:?}");
            assert_eq!(decode_strict(s, false), old_explorer(s), "explorer {s:?}");
        }
        assert_eq!(decode("%%41", false), "%A");
        assert_eq!(decode("%2", false), "%2");
        assert_eq!(decode("%é", false), "%é");
        assert_eq!(decode("%-1", false), "%-1");
    }

    /// `u8::from_str_radix` takes a leading `+`, so the old copies decoded
    /// `%+1` to the byte 0x01. It is not an escape: now kept as text.
    #[test]
    fn plus_sign_escape_is_no_longer_decoded() {
        assert_eq!(old_feat_mod("%+1"), "\u{1}");
        assert_eq!(old_codemap("%+f"), "\u{f}");
        assert_eq!(old_str_slice("%+A"), "\u{a}");
        assert_eq!(old_monitor("%+1"), "\u{1}");
        assert_eq!(old_create_standin("%+1"), "\u{1}");
        assert_eq!(decode("%+1", false), "%+1");
        assert_eq!(decode("%+1", true), "% 1");
        assert_eq!(decode_strict("%+1", false).as_deref(), Some("%+1"));
    }

    #[test]
    #[should_panic]
    fn old_monitor_panicked_on_one_trailing_digit() {
        old_monitor("%4");
    }

    #[test]
    #[should_panic]
    fn old_str_slice_panicked_on_multibyte_after_percent_digit() {
        old_str_slice("%a\u{e9}");
    }

    #[test]
    #[should_panic]
    fn old_str_slice_panicked_on_three_byte_char_after_percent() {
        old_str_slice("%\u{20ac}");
    }

    #[test]
    #[should_panic]
    fn old_create_standin_panicked_on_multibyte_after_percent_digit() {
        old_create_standin("%a\u{e9}");
    }

    #[test]
    fn new_decoder_on_the_old_panics() {
        assert_eq!(decode("%4", false), "%4");
        assert_eq!(decode("x/%4", false), "x/%4");
        assert_eq!(decode("%a\u{e9}", false), "%a\u{e9}");
        assert_eq!(decode("%\u{20ac}", false), "%\u{20ac}");
        assert_eq!(decode("%\u{20ac}", true), "%\u{20ac}");
        assert_eq!(decode("%1\u{1f600}", true), "%1\u{1f600}");
        // The panic-free copies already gave these results.
        for s in ["%a\u{e9}", "%\u{20ac}", "%1\u{1f600}"] {
            assert_eq!(decode(s, true), old_feat_mod(s));
            assert_eq!(decode(s, false), old_codemap(s));
        }
    }

    /// Any string, any position: no panic, and an input without `%` (and
    /// without `+` when it is a space) comes back unchanged.
    #[test]
    fn never_panics() {
        let parts = ["%", "2", "z", "+", "\u{e9}", "\u{20ac}", "\u{1f600}", "a", "F", "%%"];
        let mut n = 0u32;
        for a in parts {
            for b in parts {
                for c in parts {
                    for d in parts {
                        let s = format!("{a}{b}{c}{d}");
                        let _ = decode(&s, true);
                        let _ = decode_strict(&s, false);
                        n += 1;
                    }
                }
            }
        }
        assert_eq!(n, 10_000);
        assert_eq!(decode("a\u{e9}\u{20ac}+", false), "a\u{e9}\u{20ac}+");
    }
}
