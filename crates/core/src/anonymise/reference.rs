//! The masking rules as they were before the Rust quality pass (finding 1,
//! `.sbxw-artifacts/reports/rust-quality-core.md`), kept verbatim as the
//! oracle the faster [`super::mask`] is compared with. Test-only: never
//! built into sbxw.

use super::{fold_text, is_mark, is_word, Masked, Rules};

/// `text` with the listed names and the known shapes masked.
pub fn mask(text: &str, rules: &Rules) -> Masked {
    let c: Vec<char> = text.chars().collect();
    let (folded, start) = fold_text(&c);
    let word = |i: usize| c.get(i).is_some_and(|&x| is_word(x));
    // A listed name at `i`: how many characters of `text` it covers. It ends
    // where a character of `text` ends (not inside the `ss` of a `ß`), takes
    // the marks on its last letter, and is a whole word.
    let name_at = |i: usize, said: &[char]| -> Option<usize> {
        let end = start[i] + said.len();
        if !folded[start[i]..].starts_with(said) {
            return None;
        }
        let mut j = i + 1;
        while start[j] < end {
            j += 1;
        }
        if start[j] != end {
            return None;
        }
        while c.get(j).is_some_and(|&x| is_mark(x)) {
            j += 1;
        }
        (!word(j)).then_some(j - i)
    };
    let mut out = String::with_capacity(text.len());
    let mut count = 0;
    let mut i = 0;
    while i < c.len() {
        let starts_word = i == 0 || !word(i - 1);
        if starts_word {
            // An address first: a listed name at its start would leave the
            // rest of it behind.
            let shape = rules
                .has("email")
                .then(|| email(&c, i))
                .flatten()
                .map(|n| (n, "[email]"))
                .or_else(|| rules.replace.iter().find_map(|(said, sent)| name_at(i, said).map(|n| (n, sent.as_str()))))
                .or_else(|| rules.has("iban").then(|| iban(&c, i)).flatten().map(|n| (n, "[iban]")))
                .or_else(|| rules.has("phone").then(|| phone(&c, i)).flatten().map(|n| (n, "[phone]")))
                .or_else(|| rules.has("secret").then(|| secret(&c, i)).flatten().map(|n| (n, "[secret]")));
            if let Some((len, sent)) = shape {
                out.push_str(sent);
                count += 1;
                i += len;
                continue;
            }
        }
        out.push(c[i]);
        i += 1;
    }
    Masked { text: out, count }
}

/// `name.surname@example.com` starting at `i`: its length.
fn email(c: &[char], i: usize) -> Option<usize> {
    let local = c[i..].iter().take_while(|&&x| is_word(x) || matches!(x, '.' | '_' | '%' | '+' | '-')).count();
    if local == 0 || c.get(i + local) != Some(&'@') {
        return None;
    }
    let start = i + local + 1;
    let mut domain = c[start..].iter().take_while(|&&x| is_word(x) || matches!(x, '.' | '-')).count();
    // A sentence's own full stop is not the domain's.
    while domain > 0 && !is_word(c[start + domain - 1]) {
        domain -= 1;
    }
    let host: String = c[start..start + domain].iter().collect();
    let tld = host.rsplit('.').next().unwrap_or("");
    let tld_ok = tld.chars().filter(|x| x.is_alphabetic()).count() >= 2 && tld.chars().all(|x| x.is_alphabetic() || is_mark(x));
    (host.contains('.') && tld_ok).then_some(local + 1 + domain)
}

/// A phone number starting at `i`: `+41 22 717 21 11`, `0041…`, `022 717 21 11`,
/// `06.12.34.56.78`. It starts with `+` or `0` and holds 9 to 15 digits, so
/// a count, a year or a version is left alone. Without that prefix, see
/// [`grouped_phone`].
fn phone(c: &[char], i: usize) -> Option<usize> {
    if continues_a_number(c, i) {
        return None;
    }
    if !matches!(c.get(i), Some('+' | '0')) {
        return grouped_phone(c, i);
    }
    let (mut n, mut digits, mut end) = (i, 0, i);
    if c[i] == '+' {
        n += 1;
    }
    while let Some(&x) = c.get(n) {
        if x.is_ascii_digit() {
            digits += 1;
            end = n + 1;
        } else if !(matches!(x, ' ' | '.' | '-' | '\u{a0}') && c.get(n + 1).is_some_and(char::is_ascii_digit)) {
            break;
        }
        n += 1;
    }
    let alone = !c.get(end).is_some_and(|x| x.is_alphanumeric());
    ((9..=15).contains(&digits) && alone).then_some(end - i)
}

/// The digit at `i` is the rest of a number already begun: the decimals of
/// `3.141 592…`, the end of a version, a time or a date (`10 11 2026 14 30`
/// read from `11`), the thousands of `1 000 000 000`.
fn continues_a_number(c: &[char], i: usize) -> bool {
    c.get(i).is_some_and(char::is_ascii_digit)
        && i >= 2
        && matches!(c[i - 1], '.' | ',' | ':' | '/' | '-' | '\'' | ' ' | '\u{a0}')
        && c[i - 2].is_ascii_digit()
}

/// Words next to a number that make it an amount.
const CURRENCIES: &[&str] = &[
    "chf", "fr", "frs", "franc", "francs", "eur", "euro", "euros", "€", "usd", "dollar", "dollars", "$", "gbp", "£", "livre",
    "livres", "pound", "pounds", "jpy", "¥", "sek", "nok", "dkk", "kr", "pln", "zł", "zl", "czk", "kč",
];

/// A phone number without `+` or `0` at `i`, as a recogniser writes one:
/// `41 22 717 21 11`, `41.22.717.21.11`, `33-1-45-67-89-01`. At least three
/// groups of digits, the first of 1 to 4 and the others of 2 to 4 (the second
/// may be of one, `33 1 45 67 89 01`, unless dots separate them), separated
/// by one space, dot or dash, 9 to 15 digits in all. Left alone: a date
/// (`10 11 2026 14 30`, `2026-11-10`), dots mixed with other separators
/// (`10.11.2026 14.30`), a version or an address (a group of one digit, an
/// IPv4 address), a decimal or what follows one, thousands (every group
/// after the first of three digits) and an amount (a currency next to it).
fn grouped_phone(c: &[char], i: usize) -> Option<usize> {
    if !c.get(i)?.is_ascii_digit() {
        return None;
    }
    let mut groups: Vec<&[char]> = Vec::new();
    let (mut seps, mut n) = (Vec::new(), i);
    loop {
        let g = c[n..].iter().take_while(|x| x.is_ascii_digit()).count();
        groups.push(&c[n..n + g]);
        n += g;
        match c.get(n) {
            Some(&s @ (' ' | '.' | '-' | '\u{a0}')) if c.get(n + 1).is_some_and(char::is_ascii_digit) => {
                seps.push(if s == '\u{a0}' { ' ' } else { s });
                n += 1;
            }
            _ => break,
        }
    }
    let end = n;
    let value = |g: &[char]| g.iter().collect::<String>().parse::<u32>().unwrap_or(u32::MAX);
    let digits: usize = groups.iter().map(|g| g.len()).sum();
    // The area code after a country code may be one digit (`33 1 45…`), but
    // not between dots, where it is a version's.
    let short_area = !seps.contains(&'.') && groups.get(1).is_some_and(|g| g.len() == 1);
    let shaped = groups.len() >= 3
        && (9..=15).contains(&digits)
        && (1..=4).contains(&groups[0].len())
        && groups[1..].iter().enumerate().all(|(k, g)| (2..=4).contains(&g.len()) || (k == 0 && short_area));
    if !shaped {
        return None;
    }
    if seps.contains(&'.') && seps.iter().any(|&s| s != '.') {
        return None;
    }
    if groups[1..].iter().all(|g| g.len() == 3) {
        return None;
    }
    let ipv4 = seps.iter().all(|&s| s == '.') && groups.len() == 4 && groups.iter().all(|g| g.len() <= 3 && value(g) <= 255);
    let (a, b, d) = (groups[0], groups[1], groups[2]);
    let year = |g: &[char]| g.len() == 4 && (1900..=2099).contains(&value(g));
    let day_month = |x: &[char], y: &[char]| x.len() <= 2 && y.len() <= 2 && (1..=31).contains(&value(x)) && (1..=31).contains(&value(y));
    let date = (day_month(a, b) && year(d)) || (year(a) && day_month(b, d));
    if ipv4 || date {
        return None;
    }
    // What follows: not a letter or digit, not a decimal or a time.
    if c.get(end).is_some_and(|&x| is_word(x)) {
        return None;
    }
    if matches!(c.get(end), Some('.' | ',' | ':' | '\'')) && c.get(end + 1).is_some_and(char::is_ascii_digit) {
        return None;
    }
    // A currency on either side.
    let mut before: Vec<char> = c[..i].iter().rev().skip_while(|x| x.is_whitespace()).take_while(|x| !x.is_whitespace()).copied().collect();
    before.reverse();
    let before: String = before.into_iter().collect();
    let after: String = c[end..].iter().skip_while(|x| x.is_whitespace()).take_while(|x| !x.is_whitespace()).collect();
    let money = |w: &str| {
        let w = w.trim_matches(|x: char| matches!(x, '.' | ',' | ';' | ':' | '(' | ')')).to_lowercase();
        CURRENCIES.contains(&w.as_str())
    };
    if money(&before) || money(&after) {
        return None;
    }
    Some(end - i)
}

/// An IBAN starting at `i`, in one piece or in groups: `CH93 0076 2011 6238 5295 7`.
fn iban(c: &[char], i: usize) -> Option<usize> {
    let head = c.get(i..i + 4)?;
    if !(head[..2].iter().all(char::is_ascii_uppercase) && head[2..].iter().all(char::is_ascii_digit)) {
        return None;
    }
    let (mut n, mut chars, mut end) = (i, 0, i);
    while let Some(&x) = c.get(n) {
        if x.is_ascii_digit() || x.is_ascii_uppercase() {
            chars += 1;
            end = n + 1;
        } else if !(x == ' ' && c.get(n + 1).is_some_and(|y| y.is_ascii_digit() || y.is_ascii_uppercase())) {
            break;
        }
        // A word after the number is not part of it: groups hold digits, or
        // at most four characters.
        if x == ' ' && c[n + 1..].iter().take_while(|y| y.is_ascii_alphanumeric()).count() > 4 && !c[n + 1].is_ascii_digit() {
            break;
        }
        n += 1;
    }
    let alone = !c.get(end).is_some_and(|x| x.is_alphanumeric());
    ((15..=34).contains(&chars) && alone).then_some(end - i)
}

/// What a key or a token starts with, whatever follows.
const SECRET_PREFIXES: &[&str] = &["sk-", "ghp_", "gho_", "ghs_", "github_pat_", "xoxb-", "xoxp-", "AKIA", "eyJ", "glpat-"];

/// A key or a token starting at `i`: one unbroken run of letters, digits,
/// `_` and `-`, either long and mixed, or with a known prefix. A commit hash
/// (hexadecimal only) is not a secret.
fn secret(c: &[char], i: usize) -> Option<usize> {
    let run: String = c[i..].iter().take_while(|x| x.is_ascii_alphanumeric() || matches!(x, '_' | '-')).collect();
    let (letters, digits) = (run.chars().any(|x| x.is_ascii_alphabetic()), run.chars().any(|x| x.is_ascii_digit()));
    let known = SECRET_PREFIXES.iter().any(|p| run.starts_with(p)) && run.len() >= 16;
    let hex = run.chars().all(|x| x.is_ascii_hexdigit());
    // Upper and lower case mixed with digits: random, not a `snake_case_name_2`.
    let random = run.len() >= 24 && letters && digits && !hex && run.chars().any(|x| x.is_ascii_uppercase()) && run.chars().any(|x| x.is_ascii_lowercase());
    (known || random).then_some(run.chars().count())
}


/// Random texts over an alphabet that hits every shape and its edges.
#[test]
fn the_faster_mask_masks_exactly_as_before() {
    let rules = Rules::parse(
        "[replace]\n\"Jeanne Martin\" = \"Personne A\"\n\"Martin\" = \"Personne B\"\n\"ab\" = \"Nom\"\n\"sk\" = \"Nom2\"\n",
    )
    .unwrap();
    let alphabet: Vec<char> = "aAbBsSkKxX019_-.@+ 0 1 2 . -€éCH9e\u{301}ß\nIJ".chars().collect();
    let pieces = [
        "sk-", "ghp_", "AKIA", "eyJ", "glpat-", "CH93 0076 2011 6238 5295 7", "+41 22 717 21 11", "41 22 717 21 11", "0041",
        "jeanne@example.com", "a.b@c.de", "Martin", "Jeanne-Martin", "AbCdEfGh12345678901234567890", "deadbeef", "10.11.2026",
        "CHF", " ",
    ];
    let mut seed: u64 = 0x5eed_1234_abcd_0001;
    let mut next = |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as usize
    };
    for _ in 0..20_000 {
        let mut s = String::new();
        let len = next(120);
        while s.chars().count() < len {
            if next(4) == 0 {
                s.push_str(pieces[next(pieces.len())]);
            } else {
                s.push(alphabet[next(alphabet.len())]);
            }
        }
        for r in [&rules, &Rules::default()] {
            assert_eq!(super::mask(&s, r), mask(&s, r), "on {s:?}");
        }
    }
}
