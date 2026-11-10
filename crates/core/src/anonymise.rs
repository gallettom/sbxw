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
            let folded: Vec<char> = said.trim().chars().map(fold).collect();
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

/// One character as it is compared: lower case, without its accent, a hyphen
/// or an apostrophe as a space. "Élodie", "elodie" and "ELODIE" are one name;
/// so are "Saint-Denis" and "saint denis" (a recogniser writes either).
pub fn fold(c: char) -> char {
    match c.to_lowercase().next().unwrap_or(c) {
        'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' => 'a',
        'ç' => 'c',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'î' | 'ï' | 'í' | 'ì' => 'i',
        'ñ' => 'n',
        'ô' | 'ö' | 'ó' | 'ò' | 'õ' => 'o',
        'ù' | 'û' | 'ü' | 'ú' => 'u',
        'ÿ' | 'ý' => 'y',
        '-' | '‑' | '\'' | '’' => ' ',
        c => c,
    }
}

/// `text` with the listed names and the known shapes masked.
pub fn mask(text: &str, rules: &Rules) -> Masked {
    let c: Vec<char> = text.chars().collect();
    let folded: Vec<char> = c.iter().copied().map(fold).collect();
    let word = |i: usize| c.get(i).is_some_and(|x| x.is_alphanumeric());
    let mut out = String::with_capacity(text.len());
    let mut count = 0;
    let mut i = 0;
    while i < c.len() {
        let starts_word = i == 0 || !word(i - 1);
        if starts_word {
            let name = rules.replace.iter().find(|(said, _)| folded[i..].starts_with(said) && !word(i + said.len()));
            let shape = name.map(|(said, sent)| (said.len(), sent.as_str())).or_else(|| {
                None.or_else(|| rules.has("email").then(|| email(&c, i)).flatten().map(|n| (n, "[email]")))
                    .or_else(|| rules.has("iban").then(|| iban(&c, i)).flatten().map(|n| (n, "[iban]")))
                    .or_else(|| rules.has("phone").then(|| phone(&c, i)).flatten().map(|n| (n, "[phone]")))
                    .or_else(|| rules.has("secret").then(|| secret(&c, i)).flatten().map(|n| (n, "[secret]")))
            });
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
    let local = c[i..].iter().take_while(|x| x.is_ascii_alphanumeric() || matches!(x, '.' | '_' | '%' | '+' | '-')).count();
    if local == 0 || c.get(i + local) != Some(&'@') {
        return None;
    }
    let start = i + local + 1;
    let mut domain = c[start..].iter().take_while(|x| x.is_ascii_alphanumeric() || matches!(x, '.' | '-')).count();
    // A sentence's own full stop is not the domain's.
    while domain > 0 && !c[start + domain - 1].is_ascii_alphanumeric() {
        domain -= 1;
    }
    let host: String = c[start..start + domain].iter().collect();
    let tld = host.rsplit('.').next().unwrap_or("");
    (host.contains('.') && tld.len() >= 2 && tld.chars().all(|x| x.is_ascii_alphabetic())).then_some(local + 1 + domain)
}

/// A phone number starting at `i`: `+41 22 717 21 11`, `0041…`, `022 717 21 11`,
/// `06.12.34.56.78`. It starts with `+` or `0` and holds 9 to 15 digits, so
/// a count, a year or a version is left alone.
fn phone(c: &[char], i: usize) -> Option<usize> {
    if !matches!(c.get(i), Some('+' | '0')) {
        return None;
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
