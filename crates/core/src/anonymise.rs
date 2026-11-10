//! What is dictated is masked before it leaves the machine: the names the
//! person listed become the aliases they chose, and what looks like an
//! e-mail address, a phone number, an IBAN or a key becomes a placeholder.
//!
//! No model: a list and a few shapes, so what is replaced is predictable and
//! the person can read it before the yes. The price is that a name nobody
//! listed goes through; the list is theirs to keep (`anonymise.toml` in
//! sbxw's home, never in a project: it holds the very names to hide).
//!
//! ```toml
//! patterns = ["email", "phone", "iban", "secret"]   # the default: all four
//!
//! [replace]
//! "Jeanne Martin" = "Personne A"
//! "Projet Atlas" = "Projet X"
//! ```

use serde::Deserialize;
use std::collections::BTreeMap;

/// The shapes masked without being listed.
pub const PATTERNS: &[&str] = &["email", "phone", "iban", "secret"];

/// `anonymise.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rules {
    /// What is said, folded (see [`fold`]), and what is sent instead; the
    /// longest first, so "Jeanne Martin" wins over "Martin".
    replace: Vec<(Vec<char>, String)>,
    patterns: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    replace: BTreeMap<String, String>,
    patterns: Option<Vec<String>>,
}

impl Default for Rules {
    /// No list: the shapes alone.
    fn default() -> Self {
        Rules { replace: Vec::new(), patterns: PATTERNS.iter().map(|p| p.to_string()).collect() }
    }
}

impl Rules {
    pub fn parse(toml_text: &str) -> Result<Rules, String> {
        let file: File = toml::from_str(toml_text).map_err(|e| e.message().to_string())?;
        let mut replace = Vec::new();
        for (said, sent) in file.replace {
            let chars: Vec<char> = said.chars().collect();
            let folded: Vec<char> = fold_text(&chars).0.iter().collect::<String>().trim().chars().collect();
            if !folded.iter().any(|c| c.is_alphanumeric()) {
                return Err(format!("[replace] '{said}' names nothing"));
            }
            if sent.trim().is_empty() {
                return Err(format!("[replace] '{said}' has no alias"));
            }
            replace.push((folded, sent.trim().to_string()));
        }
        replace.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
        let patterns = file.patterns.unwrap_or_else(|| PATTERNS.iter().map(|p| p.to_string()).collect());
        if let Some(unknown) = patterns.iter().find(|p| !PATTERNS.contains(&p.as_str())) {
            return Err(format!("unknown pattern '{unknown}' (known: {})", PATTERNS.join(", ")));
        }
        Ok(Rules { replace, patterns })
    }

    pub fn terms(&self) -> usize {
        self.replace.len()
    }

    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    fn has(&self, pattern: &str) -> bool {
        self.patterns.iter().any(|p| p == pattern)
    }
}

/// A text fit to send, and how many things were masked in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Masked {
    pub text: String,
    pub count: usize,
}

/// One character as it is compared, when that is one character: lower case,
/// without its diacritics, a hyphen or an apostrophe as a space. "Élodie",
/// "elodie" and "ELODIE" are one name; so are "Saint-Denis" and "saint denis"
/// (a recogniser writes either). Letters that fold to two (`ß`, `æ`, `œ`, `þ`)
/// and combining marks are folded in [`mask`] itself; here they come back
/// as their first letter, and a mark as itself.
pub fn fold(c: char) -> char {
    let mut out = Vec::new();
    fold_into(c, &mut out);
    out.first().copied().unwrap_or(c)
}

/// A combining diacritical mark (U+0300 to U+036F): what a decomposed `é`
/// carries after its `e`. Part of the word it sits in, dropped when folding.
fn is_mark(c: char) -> bool {
    ('\u{300}'..='\u{36f}').contains(&c)
}

/// Part of a word: a letter or a digit in any script, or a mark on one.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || is_mark(c)
}

/// `c` folded, appended to `out`: possibly nothing (a mark), possibly two
/// letters (`ß` is `ss`).
fn fold_into(c: char, out: &mut Vec<char>) {
    for l in c.to_lowercase() {
        if is_mark(l) {
            continue;
        }
        let two = match l {
            'ß' => "ss",
            'æ' | 'ǽ' => "ae",
            'œ' => "oe",
            'þ' => "th",
            'ĳ' => "ij",
            _ => "",
        };
        if !two.is_empty() {
            out.extend(two.chars());
            continue;
        }
        out.push(match l {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' | 'ǎ' | 'ȧ' | 'ǻ' => 'a',
            'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => 'c',
            'ď' | 'đ' | 'ð' => 'd',
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
            'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
            'ĥ' | 'ħ' => 'h',
            'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' | 'ǐ' => 'i',
            'ĵ' => 'j',
            'ķ' => 'k',
            'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => 'l',
            'ñ' | 'ń' | 'ņ' | 'ň' | 'ŉ' => 'n',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' | 'ǒ' | 'ǿ' => 'o',
            'ŕ' | 'ŗ' | 'ř' => 'r',
            'ś' | 'ŝ' | 'ş' | 'š' | 'ș' => 's',
            'ţ' | 'ť' | 'ŧ' | 'ț' => 't',
            'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' | 'ǔ' => 'u',
            'ŵ' => 'w',
            'ý' | 'ÿ' | 'ŷ' => 'y',
            'ź' | 'ż' | 'ž' => 'z',
            '-' | '‐' | '‑' | '–' | '\'' | '’' | 'ʼ' => ' ',
            l if l.is_whitespace() => ' ',
            l => l,
        });
    }
}

/// `chars` folded, with each run of spaces as one, and where each character
/// starts in the result (one more entry at the end: its length).
fn fold_text(chars: &[char]) -> (Vec<char>, Vec<usize>) {
    let mut out = Vec::with_capacity(chars.len());
    let mut start = Vec::with_capacity(chars.len() + 1);
    let mut one = Vec::with_capacity(2);
    for &c in chars {
        start.push(out.len());
        one.clear();
        fold_into(c, &mut one);
        for &x in &one {
            if !(x == ' ' && out.last() == Some(&' ')) {
                out.push(x);
            }
        }
    }
    start.push(out.len());
    (out, start)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Rules {
        Rules::parse(
            r#"
            [replace]
            "Jeanne Martin" = "Personne A"
            "Martin" = "Personne B"
            "Projet Atlas" = "Projet X"
            "Saint-Élie" = "Ville 1"
            "#,
        )
        .unwrap()
    }

    fn masked(s: &str) -> String {
        mask(s, &rules()).text
    }

    #[test]
    fn listed_names_become_their_aliases() {
        let m = mask("J'ai vu Jeanne Martin pour le projet Atlas, puis Martin seul.", &rules());
        assert_eq!(m.text, "J'ai vu Personne A pour le Projet X, puis Personne B seul.");
        assert_eq!(m.count, 3);
    }

    #[test]
    fn a_name_is_found_whatever_its_case_accents_or_hyphens() {
        assert_eq!(masked("JEANNE MARTIN habite saint elie."), "Personne A habite Ville 1.");
        assert_eq!(masked("à Saint-Élie, l'atlas"), "à Ville 1, l'atlas");
    }

    #[test]
    fn a_name_inside_another_word_is_left_alone() {
        assert_eq!(masked("la Martinique et les martinets"), "la Martinique et les martinets");
    }

    #[test]
    fn addresses_and_numbers_are_masked_by_their_shape() {
        assert_eq!(masked("Écris à prenom.nom@example.com."), "Écris à [email].");
        assert_eq!(masked("Appelle le +41 22 717 21 11 ou le 06.12.34.56.78, merci"), "Appelle le [phone] ou le [phone], merci");
        assert_eq!(masked("IBAN CH93 0076 2011 6238 5295 7 pour le virement"), "IBAN [iban] pour le virement");
        assert_eq!(masked("la clé sk-ant-api03-AbCdEf123456 et ghp_abcdefghijklmnop1234"), "la clé [secret] et [secret]");
        assert_eq!(masked("token aB3dE5gH7jK9mN1pQ3sT5vW7"), "token [secret]");
    }

    #[test]
    fn ordinary_numbers_names_and_hashes_are_left_alone() {
        let s = "En 2026, 1 000 000 de vues, version 3.2.1, 0,43 s, le commit 6c19f50a1b2c3d4e5f60718293a4b5c6d7e8f901, \
                 la fonction the_speech_server_is_started_2 et le port 127.0.0.1:7681.";
        assert_eq!(masked(s), s);
        assert_eq!(mask(s, &rules()).count, 0);
    }

    #[test]
    fn an_address_with_accents_is_masked_whole() {
        assert_eq!(masked("Écris à élodie@example.org."), "Écris à [email].");
        assert_eq!(masked("Émile.Zola@x.fr répond"), "[email] répond");
        assert_eq!(masked("contact: jürgen.müller@bücher.de, merci"), "contact: [email], merci");
        assert_eq!(masked("Écris à e\u{301}lodie@example.org"), "Écris à [email]");
        assert_eq!(masked("à zoë@exemple.fr"), "à [email]");
        // A listed name at the start of an address does not leave the rest behind.
        assert_eq!(masked("martin@example.org"), "[email]");
    }

    #[test]
    fn a_number_grouped_without_a_prefix_is_a_phone() {
        assert_eq!(masked("Appelle le 41 22 717 21 11 demain"), "Appelle le [phone] demain");
        assert_eq!(masked("au 41.22.717.21.11."), "au [phone].");
        assert_eq!(masked("ou le 33-1-45-67-89-01"), "ou le [phone]");
        assert_eq!(masked("numéro 79 123 45 67 89"), "numéro [phone]");
        assert_eq!(masked("tel 41\u{a0}22\u{a0}717\u{a0}21\u{a0}11"), "tel [phone]");
    }

    #[test]
    fn dates_times_versions_amounts_and_counts_are_not_phones() {
        for s in [
            // Dates and times, French and Swiss.
            "le 10 11 2026 14 30 en salle",
            "le 10.11.2026 14.30 en salle",
            "le 2026-11-10 14 30",
            "du 10.11.2026 au 12.11.2026",
            "à 14:30:00 puis 18 h 45",
            // Versions and addresses.
            "version 10.15.7, build 2024.10.15.1, IP 192.168.10.12",
            "macOS 14.6.1 et Chrome 130.0.6723.92",
            // Amounts with a currency, and plain counts.
            "CHF 1 250 000 000 et 1 250 000 000 EUR",
            "1'250'000 fr., 12 345 678,50 € et € 99 99 99 99",
            "1 000 000 000 de vues, 123 456 789 personnes",
            "CHF 12 34 56 78 90, 12 34 56 78 90 euros, 12-34-56-78-90 zł",
            "le code 123456789012 et 3.141 592 653 589",
        ] {
            assert_eq!(mask(s, &Rules::default()).text, s, "{s}");
        }
    }

    #[test]
    fn listed_names_fold_european_letters() {
        let r = Rules::parse(
            r#"
            [replace]
            "Łukasz Wałęsa" = "Personne C"
            "Søren Kierkegård" = "Personne D"
            "Strauß" = "Personne E"
            "Œdipe Ærø" = "Personne F"
            "Žofie Dvořáková" = "Personne G"
            "Élodie" = "Personne H"
            "Jeanne Martin" = "Personne A"
            "Martin" = "Personne B"
            "#,
        )
        .unwrap();
        let m = |s: &str| mask(s, &r).text;
        assert_eq!(m("j'ai vu lukasz walesa et ŁUKASZ WAŁĘSA"), "j'ai vu Personne C et Personne C");
        assert_eq!(m("Soren Kierkegard, søren kierkegård"), "Personne D, Personne D");
        assert_eq!(m("STRAUSS et strauss et Strauß."), "Personne E et Personne E et Personne E.");
        assert_eq!(m("oedipe aero"), "Personne F");
        assert_eq!(m("zofie dvorakova"), "Personne G");
        // Decomposed accents: e + U+0301.
        assert_eq!(m("E\u{301}lodie et e\u{301}lodie"), "Personne H et Personne H");
        assert_eq!(m("Martine\u{301} reste"), "Martine\u{301} reste");
        // Runs of whitespace between the words of a name.
        assert_eq!(m("Jeanne   Martin et Jeanne\n Martin et Jeanne - Martin"), "Personne A et Personne A et Personne A");
        // Still a whole word: a longer name is someone else.
        assert_eq!(m("Martins, Martinez, Strausse"), "Martins, Martinez, Strausse");
    }

    #[test]
    fn without_a_list_the_shapes_are_still_masked() {
        assert_eq!(mask("Jeanne Martin, a@b.ch", &Rules::default()).text, "Jeanne Martin, [email]");
    }

    #[test]
    fn patterns_can_be_turned_off() {
        let r = Rules::parse("patterns = [\"email\"]").unwrap();
        assert_eq!(mask("a@b.ch, +41 22 717 21 11", &r).text, "[email], +41 22 717 21 11");
        assert_eq!(r.patterns(), ["email"]);
    }

    #[test]
    fn a_wrong_file_is_refused_not_ignored() {
        assert!(Rules::parse("[replace]\n\"  \" = \"x\"").unwrap_err().contains("names nothing"));
        assert!(Rules::parse("[replace]\n\"Jeanne\" = \"\"").unwrap_err().contains("no alias"));
        assert!(Rules::parse("patterns = [\"faces\"]").unwrap_err().contains("unknown pattern"));
        assert!(Rules::parse("[remplace]\n\"a\" = \"b\"").is_err(), "a misspelt table hides nothing: say so");
        assert_eq!(Rules::parse("").unwrap(), Rules::default());
    }
}
