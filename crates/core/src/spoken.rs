//! Text made fit to be read aloud: the symbols of technical writing become
//! words, or go. A voice model reads `>=`, `~/.sbxw` or `voice_warm` badly or
//! not at all; said in words, they are understood.
//!
//! [`DICT`] is the dictionary, one row per symbol, in French and English.
//! [`speakable`] applies it, with the few rules a table cannot hold (a dot
//! inside a name is said, the one ending a sentence is not; `12:30` stays a
//! time). The aim is to be understood by ear, not to spell: `_` is a pause
//! between two words, brackets and braces are dropped.
//!
//! Words are different: how a name is said is the person's to correct.
//! [`WORDS`] holds the built-in ones; `[voice.words]` in the daemon's
//! sbxw.toml adds or replaces them, and [`word_table`] merges the two into
//! what [`speakable`] reads.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The language the symbols are said in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Fr,
    En,
}

impl Lang {
    /// French for a French voice (`fr_male`), English for every other.
    pub fn of_voice(voice: &str) -> Lang {
        if voice.starts_with("fr_") {
            Lang::Fr
        } else {
            Lang::En
        }
    }
}

/// Symbol, French, English. Longer symbols first: the first match wins. An
/// empty word drops the symbol; a word starting with `,` is a pause.
pub const DICT: &[(&str, &str, &str)] = &[
    ("https://", "", ""),
    ("http://", "", ""),
    ("!==", "différent de", "not equal to"),
    ("===", "égal à", "equal to"),
    (">=", "supérieur ou égal à", "greater than or equal to"),
    ("<=", "inférieur ou égal à", "less than or equal to"),
    ("!=", "différent de", "not equal to"),
    ("==", "égal à", "equal to"),
    ("=>", "donne", "gives"),
    ("->", "vers", "to"),
    ("&&", "et", "and"),
    ("||", "ou", "or"),
    ("::", "", ""),
    ("()", "", ""),
    ("≥", "supérieur ou égal à", "greater than or equal to"),
    ("≤", "inférieur ou égal à", "less than or equal to"),
    ("≠", "différent de", "not equal to"),
    ("≈", "environ", "about"),
    ("→", "vers", "to"),
    ("←", "depuis", "from"),
    ("×", "fois", "times"),
    ("÷", "divisé par", "divided by"),
    ("§", "paragraphe", "section"),
    ("@", "arobase", "at"),
    ("&", "et", "and"),
    ("%", "pour cent", "percent"),
    ("=", "égal", "equals"),
    ("+", "plus", "plus"),
    ("<", "inférieur à", "less than"),
    (">", "supérieur à", "greater than"),
    ("~", "tilde", "tilde"),
    ("\\", "backslash", "backslash"),
    ("^", "puissance", "to the power of"),
    ("|", ",", ","),
    ("_", "", ""),
    ("`", "", ""),
    ("{", "", ""),
    ("}", "", ""),
    ("[", "", ""),
    ("]", "", ""),
];

/// Word, French, English: the built-in names a voice cannot read, spelt
/// letter by letter instead. Matched as a whole word, whatever its case. A
/// `[voice.words]` entry of the same word replaces its row.
pub const WORDS: &[(&str, &str, &str)] = &[("sbxw", "ece bé ixe double vé", "S B X W")];

/// At most this many entries in `[voice.words]`.
pub const MAX_WORDS: usize = 200;
/// A word is at most this many characters.
pub const MAX_WORD: usize = 40;
/// A spoken form is at most this many characters.
pub const MAX_SAID: usize = 120;

/// How one word is said, per language. A language left out: the word is
/// read as written in it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Said {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub en: Option<String>,
}

impl Said {
    pub fn get(&self, lang: Lang) -> Option<&str> {
        match lang {
            Lang::Fr => self.fr.as_deref(),
            Lang::En => self.en.as_deref(),
        }
    }
}

/// Where a word of the table comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// [`WORDS`], with no `[voice.words]` entry for it.
    BuiltIn,
    /// `[voice.words]` only.
    Configured,
    /// A `[voice.words]` entry for a word [`WORDS`] has too: it wins.
    ReplacesBuiltIn,
}

/// One word of the table [`speakable`] reads, in lower case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Word {
    pub word: String,
    pub said: Said,
    pub origin: Origin,
}

/// [`WORDS`] merged with `configured` (`[voice.words]`): a configured word
/// replaces the built-in one of the same word, whatever its case. Longer
/// words first, so `sbxw.toml` would win over `sbxw`.
pub fn word_table(configured: &BTreeMap<String, Said>) -> Vec<Word> {
    let mut out: Vec<Word> = WORDS
        .iter()
        .map(|(w, fr, en)| Word { word: w.to_lowercase(), said: Said { fr: Some(fr.to_string()), en: Some(en.to_string()) }, origin: Origin::BuiltIn })
        .collect();
    for (w, said) in configured {
        let w = w.to_lowercase();
        match out.iter_mut().find(|o| o.word == w) {
            Some(o) if o.origin == Origin::BuiltIn => *o = Word { word: w, said: said.clone(), origin: Origin::ReplacesBuiltIn },
            Some(o) => o.said = said.clone(),
            None => out.push(Word { word: w, said: said.clone(), origin: Origin::Configured }),
        }
    }
    out.sort_by(|a, b| b.word.chars().count().cmp(&a.word.chars().count()).then_with(|| a.word.cmp(&b.word)));
    out
}

/// A word `[voice.words]` may hold: 1 to [`MAX_WORD`] letters, digits, `-`,
/// `_` or `.`, starting and ending with a letter or digit (a word is matched
/// whole, between two non-letters).
pub fn check_word(word: &str) -> Result<(), String> {
    let n = word.chars().count();
    let ok_char = |c: char| c.is_alphanumeric() || matches!(c, '-' | '_' | '.');
    let ends = word.chars().next().is_some_and(char::is_alphanumeric) && word.chars().last().is_some_and(char::is_alphanumeric);
    if !(1..=MAX_WORD).contains(&n) || !word.chars().all(ok_char) || !ends {
        return Err(format!("'{word}' is not a word: 1 to {MAX_WORD} letters, digits, '-', '_' or '.', starting and ending with a letter or digit"));
    }
    Ok(())
}

/// A spoken form: one line of 1 to [`MAX_SAID`] characters, no control
/// characters.
pub fn check_said(word: &str, lang: &str, said: &str) -> Result<(), String> {
    if said.trim().is_empty() {
        return Err(format!("'{word}': the {lang} form is empty; leave {lang} out for the word to be read as written"));
    }
    if said.chars().count() > MAX_SAID || said.chars().any(char::is_control) {
        return Err(format!("'{word}': the {lang} form is one line of at most {MAX_SAID} characters, with no control characters"));
    }
    Ok(())
}

/// `[voice.words]` as [`crate::config::Config::validate`] checks it.
pub fn check_words(words: &BTreeMap<String, Said>) -> Result<(), String> {
    if words.len() > MAX_WORDS {
        return Err(format!("[voice.words] has {} entries: at most {MAX_WORDS}", words.len()));
    }
    let mut seen: Vec<String> = Vec::new();
    for (w, said) in words {
        check_word(w).map_err(|e| format!("[voice.words] {e}"))?;
        let lower = w.to_lowercase();
        if seen.contains(&lower) {
            return Err(format!("[voice.words] '{w}' is given twice (case does not count)"));
        }
        seen.push(lower);
        for (lang, s) in [("fr", &said.fr), ("en", &said.en)] {
            if let Some(s) = s {
                check_said(w, lang, s).map_err(|e| format!("[voice.words] {e}"))?;
            }
        }
    }
    Ok(())
}

/// Unit, French, English, in the plural. Said in full after a number only
/// (`0,43 s`, `8 Go`), longer units first.
pub const UNITS: &[(&str, &str, &str)] = &[
    ("kHz", "kilohertz", "kilohertz"),
    ("min", "minutes", "minutes"),
    ("ms", "millisecondes", "milliseconds"),
    ("Hz", "hertz", "hertz"),
    ("Go", "gigaoctets", "gigabytes"),
    ("Mo", "mégaoctets", "megabytes"),
    ("Ko", "kilooctets", "kilobytes"),
    ("ko", "kilooctets", "kilobytes"),
    ("GB", "gigaoctets", "gigabytes"),
    ("MB", "mégaoctets", "megabytes"),
    ("KB", "kilooctets", "kilobytes"),
    ("kB", "kilooctets", "kilobytes"),
    ("px", "pixels", "pixels"),
    ("km", "kilomètres", "kilometres"),
    ("s", "secondes", "seconds"),
    ("h", "heures", "hours"),
];

/// The words for the symbols whose reading depends on what surrounds them.
struct Words {
    slash: &'static str,
    divided: &'static str,
    dot: &'static str,
    colon: &'static str,
    minus: &'static str,
    dash: &'static str,
    times: &'static str,
    number: &'static str,
    hash: &'static str,
    dollar: &'static str,
    dollars: &'static str,
}

const FR: Words = Words {
    slash: "slash",
    divided: "divisé par",
    dot: "point",
    colon: "deux-points",
    minus: "moins",
    dash: "tiret",
    times: "fois",
    number: "numéro",
    hash: "dièse",
    dollar: "dollar",
    dollars: "dollars",
};

const EN: Words = Words {
    slash: "slash",
    divided: "divided by",
    dot: "dot",
    colon: "colon",
    minus: "minus",
    dash: "dash",
    times: "times",
    number: "number",
    hash: "hash",
    dollar: "dollar",
    dollars: "dollars",
};

/// `text` with its symbols said in words, in `lang`, and the names of
/// `words` (see [`word_table`]) said as the table says them.
pub fn speakable(text: &str, lang: Lang, words: &[Word]) -> String {
    let w = match lang {
        Lang::Fr => &FR,
        Lang::En => &EN,
    };
    let c: Vec<char> = text.chars().collect();
    let at = |i: usize| c.get(i).copied();
    let before = |i: usize| i.checked_sub(1).and_then(|j| c.get(j).copied());
    let word = |x: Option<char>| x.is_some_and(char::is_alphanumeric);
    let digit = |x: Option<char>| x.is_some_and(|x| x.is_ascii_digit());
    let gap = |x: Option<char>| x.is_none_or(char::is_whitespace);
    let starts = |i: usize, s: &str| s.chars().enumerate().all(|(k, x)| at(i + k) == Some(x));

    let mut out = String::with_capacity(text.len() + 32);
    let say = |out: &mut String, s: &str| {
        if s.starts_with(',') {
            let kept = out.trim_end().len();
            out.truncate(kept);
        } else {
            out.push(' ');
        }
        out.push_str(s);
        out.push(' ');
    };
    let mut i = 0;
    while i < c.len() {
        let (ch, prev, next) = (c[i], before(i), at(i + 1));
        // A unit right after a number, with or without a space: `0,43 s`, `120s`.
        let number_end = if digit(prev) { Some(i) } else if prev == Some(' ') && i >= 2 && digit(at(i - 2)) { Some(i - 1) } else { None };
        if let (true, Some(end)) = (ch.is_alphabetic(), number_end) {
            let unit = UNITS.iter().find(|(sym, ..)| starts(i, sym) && !word(at(i + sym.chars().count())) && !matches!(at(i + sym.chars().count()), Some('\'' | '’')));
            if let Some((sym, fr, en)) = unit {
                let said = if lang == Lang::Fr { fr } else { en };
                // `1 s` is one second; `11 s` and `0,1 s` are not.
                let one = c[end - 1] == '1' && !end.checked_sub(2).is_some_and(|k| c[k].is_ascii_digit() || matches!(c[k], '.' | ','));
                say(&mut out, if one { said.strip_suffix('s').unwrap_or(said) } else { said });
                i += sym.chars().count();
                continue;
            }
        }
        if !word(prev) {
            // A word with no form in this language is read as written.
            let spelt = words.iter().filter_map(|w| Some((w, w.said.get(lang)?))).find(|(w, _)| {
                let n = w.word.chars().count();
                !word(at(i + n)) && w.word.chars().enumerate().all(|(k, x)| at(i + k).is_some_and(|y| y.to_lowercase().eq(x.to_lowercase())))
            });
            if let Some((w, said)) = spelt {
                say(&mut out, said.trim());
                i += w.word.chars().count();
                continue;
            }
        }
        match ch {
            '/' if gap(prev) && gap(next) => say(&mut out, w.divided),
            '/' => say(&mut out, w.slash),
            // A dot inside or in front of a name: `sbxw.toml`, `3.2.1`, `~/.sbxw`.
            '.' if word(next) && (word(prev) || gap(prev) || matches!(prev, Some('/' | '~'))) => say(&mut out, w.dot),
            ':' if starts(i, "://") => {
                i += 3;
                continue;
            }
            // `host:7681`, `C:\`; `12:30` stays a time.
            ':' if word(prev) && (word(next) || next == Some('\\')) && !is_time(&c, i) => say(&mut out, w.colon),
            '#' if digit(next) => say(&mut out, w.number),
            '#' if word(next) => say(&mut out, w.hash),
            '#' => {}
            '$' if digit(next) => {
                let end = (i + 1..c.len()).find(|&k| !(c[k].is_ascii_digit() || (matches!(c[k], '.' | ',') && digit(at(k + 1))))).unwrap_or(c.len());
                out.push(' ');
                out.extend(&c[i + 1..end]);
                say(&mut out, w.dollars);
                i = end;
                continue;
            }
            '$' => say(&mut out, w.dollar),
            '*' if gap(prev) && gap(next) && i > 0 && i + 1 < c.len() => say(&mut out, w.times),
            '*' => {}
            // `--release`, `-m`: a flag. `-1`: a negative number. `x - y`: a
            // subtraction. A lone dash between words is a pause already.
            '-' if gap(prev) && next == Some('-') && word(at(i + 2)) => {
                say(&mut out, w.dash);
                say(&mut out, w.dash);
                i += 2;
                continue;
            }
            '-' if gap(prev) && digit(next) => say(&mut out, w.minus),
            '-' if gap(prev) && word(next) && !word(at(i + 2)) => say(&mut out, w.dash),
            '-' if gap(prev) && gap(next) && i > 0 && digit(c[..i].iter().rev().find(|x| !x.is_whitespace()).copied()) && digit(c[i + 1..].iter().find(|x| !x.is_whitespace()).copied()) => {
                say(&mut out, w.minus)
            }
            'n' if next == Some('°') && !word(prev) => {
                say(&mut out, w.number);
                i += 2;
                continue;
            }
            _ => match DICT.iter().find(|(sym, ..)| starts(i, sym)) {
                Some((sym, fr, en)) => {
                    let said = if lang == Lang::Fr { fr } else { en };
                    if said.is_empty() {
                        out.push(' ');
                    } else {
                        say(&mut out, said);
                    }
                    i += sym.chars().count();
                    continue;
                }
                None => out.push(ch),
            },
        }
        i += 1;
    }
    tidy(&out)
}

/// `12:30`: one or two digits, the colon at `i`, exactly two digits.
fn is_time(c: &[char], i: usize) -> bool {
    let d = |k: usize| c.get(k).is_some_and(char::is_ascii_digit);
    let lead = (0..i).rev().take_while(|&k| d(k)).count();
    let stands_alone = i.checked_sub(lead + 1).is_none_or(|k| !(c[k].is_alphanumeric() || c[k] == '.'));
    (1..=2).contains(&lead) && stands_alone && d(i + 1) && d(i + 2) && !d(i + 3)
}

/// One space between words, none before punctuation or after an opening
/// parenthesis.
fn tidy(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for part in s.split_whitespace() {
        let glued = part.starts_with([',', '.', ')']) && part.chars().nth(1).is_none_or(|x| !x.is_alphanumeric());
        if !out.is_empty() && !glued && !out.ends_with('(') {
            out.push(' ');
        }
        out.push_str(part);
    }
    // What a dropped symbol left empty: `Ok(())`.
    if out.contains("()") {
        return tidy(&out.replace("()", " "));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fr(s: &str) -> String {
        speakable(s, Lang::Fr, &word_table(&BTreeMap::new()))
    }

    fn en(s: &str) -> String {
        speakable(s, Lang::En, &word_table(&BTreeMap::new()))
    }

    #[test]
    fn plain_prose_is_left_alone() {
        let s = "Bonjour, ceci est un test : tout va bien (ou presque). Il est 12:30, et ça coûte 12 €.";
        assert_eq!(fr(s), s);
    }

    #[test]
    fn names_and_paths_are_said_in_words() {
        assert_eq!(fr("voice_warm"), "voice warm");
        assert_eq!(fr("dans sbxw.toml."), "dans ece bé ixe double vé point toml.");
        assert_eq!(fr("~/.sbxw/run/server.err.log"), "tilde slash point ece bé ixe double vé slash run slash server point err point log");
        assert_eq!(fr("C:\\Users\\test"), "C deux-points backslash Users backslash test");
        assert_eq!(fr("svc::voice"), "svc voice");
        assert_eq!(fr("ensure_speech() renvoie Ok(())"), "ensure speech renvoie Ok");
    }

    #[test]
    fn addresses_are_said_in_words() {
        assert_eq!(fr("http://127.0.0.1:7681/api/voice"), "127 point 0 point 0 point 1 deux-points 7681 slash api slash voice");
        assert_eq!(fr("prenom.nom@example.com"), "prenom point nom arobase example point com");
    }

    #[test]
    fn operators_are_said_in_words() {
        assert_eq!(fr("si x >= 10 && y != 0"), "si x supérieur ou égal à 10 et y différent de 0");
        assert_eq!(fr("z = x / y, sinon z = -1"), "z égal x divisé par y, sinon z égal moins 1");
        assert_eq!(fr("3 * 4 - 2"), "3 fois 4 moins 2");
        assert_eq!(fr("a -> b"), "a vers b");
        assert_eq!(fr("et/ou"), "et slash ou");
    }

    #[test]
    fn numbers_keep_their_meaning() {
        assert_eq!(fr("de 3.2.1 à 3.3.0, soit +1"), "de 3 point 2 point 1 à 3 point 3 point 0, soit plus 1");
        assert_eq!(fr("100 % en 0,43 s pour $15."), "100 pour cent en 0,43 secondes pour 15 dollars.");
        assert_eq!(fr("le ticket #42, la PR n° 7, le §3"), "le ticket numéro 42, la PR numéro 7, le paragraphe 3");
        assert_eq!(fr("à 9:05 sur le port 127.0.0.1:80"), "à 9:05 sur le port 127 point 0 point 0 point 1 deux-points 80");
    }

    #[test]
    fn a_name_no_voice_can_read_is_spelt() {
        assert_eq!(fr("Lance sbxw, puis SBXW down."), "Lance ece bé ixe double vé, puis ece bé ixe double vé down.");
        assert_eq!(fr("dans sbxw.toml"), "dans ece bé ixe double vé point toml");
        assert_eq!(fr("sbxwd et xsbxw"), "sbxwd et xsbxw");
        assert_eq!(en("run sbxw"), "run S B X W");
    }

    #[test]
    fn units_are_said_in_full_after_a_number() {
        assert_eq!(fr("en 0,43 s."), "en 0,43 secondes.");
        assert_eq!(fr("0.43s, puis 120 ms et 1 s"), "0 point 43 secondes, puis 120 millisecondes et 1 seconde");
        assert_eq!(fr("8 Go à 24 kHz pendant 5 min"), "8 gigaoctets à 24 kilohertz pendant 5 minutes");
        assert_eq!(fr("les 3 sandboxes, 2 hôtes, 4 s'il faut"), "les 3 sandboxes, 2 hôtes, 4 s'il faut");
        assert_eq!(en("11 s and 2 GB"), "11 seconds and 2 gigabytes");
    }

    #[test]
    fn commands_and_data_are_said_in_words() {
        assert_eq!(fr("cargo build --release -m \"fix\""), "cargo build tiret tiret release tiret m \"fix\"");
        assert_eq!(fr("$HOME ou ${PATH}"), "dollar HOME ou dollar PATH");
        assert_eq!(fr("nom | état | type"), "nom, état, type");
        assert_eq!(fr("{\"ok\": false} et la note [1]"), "\"ok\": false et la note 1");
        assert_eq!(fr("un mot-composé - une incise - et la suite"), "un mot-composé - une incise - et la suite");
    }

    #[test]
    fn markdown_marks_go() {
        assert_eq!(fr("## Titre"), "Titre");
        assert_eq!(fr("**gras** et `code`"), "gras et code");
    }

    #[test]
    fn another_voice_speaks_english() {
        assert_eq!(Lang::of_voice("fr_female"), Lang::Fr);
        assert_eq!(Lang::of_voice("casual_male"), Lang::En);
        assert_eq!(en("x >= 1 @ a.b"), "x greater than or equal to 1 at a dot b");
    }

    #[test]
    fn every_symbol_is_listed_once_and_longer_ones_first() {
        for (i, (sym, ..)) in DICT.iter().enumerate() {
            assert!(!DICT[..i].iter().any(|(earlier, ..)| sym.starts_with(earlier)), "'{sym}' is hidden by an earlier, shorter row");
        }
    }

    fn said(fr: Option<&str>, en: Option<&str>) -> Said {
        Said { fr: fr.map(String::from), en: en.map(String::from) }
    }

    #[test]
    fn configured_words_add_to_and_replace_the_built_in_ones() {
        let mut cfg = BTreeMap::new();
        cfg.insert("Nginx".to_string(), said(Some("engine ixe"), Some("engine X")));
        cfg.insert("sbxw".to_string(), said(Some("sandbox double vé"), None));
        let t = word_table(&cfg);
        assert_eq!(t.iter().map(|w| (w.word.as_str(), w.origin)).collect::<Vec<_>>(), [("nginx", Origin::Configured), ("sbxw", Origin::ReplacesBuiltIn)]);
        assert_eq!(speakable("NGINX devant sbxw, et nginx2", Lang::Fr, &t), "engine ixe devant sandbox double vé, et nginx2");
        // No English form: read as written, not the built-in spelling.
        assert_eq!(speakable("nginx and sbxw", Lang::En, &t), "engine X and sbxw");
        // The override gone, the built-in comes back.
        cfg.remove("sbxw");
        let t = word_table(&cfg);
        assert_eq!(speakable("sbxw", Lang::Fr, &t), "ece bé ixe double vé");
        assert_eq!(t.iter().find(|w| w.word == "sbxw").unwrap().origin, Origin::BuiltIn);
    }

    #[test]
    fn a_longer_word_wins_and_words_may_hold_dots() {
        let mut cfg = BTreeMap::new();
        cfg.insert("node.js".to_string(), said(Some("node jé esse"), None));
        cfg.insert("node".to_string(), said(Some("naude"), None));
        let t = word_table(&cfg);
        assert_eq!(speakable("node.js et node", Lang::Fr, &t), "node jé esse et naude");
    }

    #[test]
    fn configured_words_are_checked() {
        let one = |w: &str, s: Said| check_words(&BTreeMap::from([(w.to_string(), s)]));
        assert!(one("node.js", said(Some("node"), None)).is_ok());
        assert!(one("été-2", said(None, Some("summer two"))).is_ok());
        assert!(one("two words", said(Some("x"), None)).unwrap_err().contains("'two words' is not a word"));
        assert!(one("-x", said(Some("x"), None)).is_err());
        assert!(one("", said(Some("x"), None)).is_err());
        assert!(one(&"a".repeat(41), said(Some("x"), None)).is_err());
        assert!(one("x", said(Some("a\nb"), None)).unwrap_err().contains("'x': the fr form"));
        assert!(one("x", said(None, Some(&"a".repeat(121)))).unwrap_err().contains("at most 120"));
        assert!(one("x", said(Some(" "), None)).unwrap_err().contains("empty"));
        let twice = BTreeMap::from([("Sbx".to_string(), said(Some("a"), None)), ("sbx".to_string(), said(Some("b"), None))]);
        assert!(check_words(&twice).unwrap_err().contains("twice"));
        let many: BTreeMap<String, Said> = (0..=MAX_WORDS).map(|i| (format!("w{i}"), said(Some("x"), None))).collect();
        assert!(check_words(&many).unwrap_err().contains("at most 200"));
    }
}
