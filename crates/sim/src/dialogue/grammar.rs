//! Piccola grammatica italiana per le battute: articoli determinativi,
//! preposizioni articolate con i nomi delle carrozze («Il Refettorio» → "al
//! Refettorio", «Alveare» → "all'Alveare"), cognomi di famiglia ("i Rossi",
//! "gli Esposito"), d eufonica ("Bruno ed Elena", "ad Anna"), elisione di
//! "di" e maiuscola a inizio battuta.
//!
//! Carriage names are treated as common nouns that take the definite
//! article, like the places they are ("al Nido", "nella Serra 3"): a name
//! that starts with an article («La Brace», «Le Brande») keeps it, otherwise
//! gender and number are guessed from its first word («Cuccette Nord» is
//! feminine plural, «Officina Grande» feminine, «Dormitorio 6» masculine).

/// A definite article.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Article {
    Il,
    Lo,
    /// "l'" (elided, singular before a vowel).
    L,
    La,
    I,
    Gli,
    Le,
}

/// Simple prepositions that merge with the article.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prep {
    A,
    In,
    Di,
    Da,
    Su,
}

impl Article {
    /// The article alone, or merged with `prep`, followed by a space unless
    /// elided: "il ", "l'", "al ", "nell'", "degli "...
    pub fn with(self, prep: Option<Prep>) -> &'static str {
        use Article::*;
        let row: [&str; 6] = match self {
            Il => ["il ", "al ", "nel ", "del ", "dal ", "sul "],
            Lo => ["lo ", "allo ", "nello ", "dello ", "dallo ", "sullo "],
            L => ["l'", "all'", "nell'", "dell'", "dall'", "sull'"],
            La => ["la ", "alla ", "nella ", "della ", "dalla ", "sulla "],
            I => ["i ", "ai ", "nei ", "dei ", "dai ", "sui "],
            Gli => ["gli ", "agli ", "negli ", "degli ", "dagli ", "sugli "],
            Le => ["le ", "alle ", "nelle ", "delle ", "dalle ", "sulle "],
        };
        row[match prep {
            None => 0,
            Some(Prep::A) => 1,
            Some(Prep::In) => 2,
            Some(Prep::Di) => 3,
            Some(Prep::Da) => 4,
            Some(Prep::Su) => 5,
        }]
    }
}

fn first_char(word: &str) -> Option<char> {
    word.chars()
        .next()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
}

/// Starts with a vowel sound (h is silent: "l'hotel").
pub fn starts_with_vowel(word: &str) -> bool {
    let w = word.to_lowercase();
    let w = w.strip_prefix('h').unwrap_or(&w);
    matches!(
        w.chars().next(),
        Some('a' | 'e' | 'i' | 'o' | 'u' | 'à' | 'è' | 'é' | 'ì' | 'ò' | 'ù')
    )
}

/// Takes "lo" / "gli": s + consonant, z, x, y, gn, ps, pn.
fn takes_lo(word: &str) -> bool {
    let w = word.to_lowercase();
    let mut c = w.chars();
    match (c.next(), c.next()) {
        (Some('z' | 'x' | 'y'), _) => true,
        (Some('s'), Some(n)) => !matches!(
            n,
            'a' | 'e' | 'i' | 'o' | 'u' | 'à' | 'è' | 'é' | 'ì' | 'ò' | 'ù'
        ),
        (Some('g'), Some('n')) | (Some('p'), Some('s' | 'n')) => true,
        _ => false,
    }
}

/// The definite article for `word` (feminine? plural?).
pub fn article(word: &str, feminine: bool, plural: bool) -> Article {
    let vowel = starts_with_vowel(word);
    match (feminine, plural) {
        (true, true) => Article::Le,
        (true, false) if vowel => Article::L,
        (true, false) => Article::La,
        (false, true) if vowel || takes_lo(word) => Article::Gli,
        (false, true) => Article::I,
        (false, false) if vowel => Article::L,
        (false, false) if takes_lo(word) => Article::Lo,
        (false, false) => Article::Il,
    }
}

/// Head words of place names that are feminine plural although they end in -e.
const FEMININE_PLURALS: &[&str] = &[
    "cuccette", "brande", "carrozze", "cucine", "serre", "officine", "mense", "botteghe", "stanze",
    "erbe",
];

/// Gender and number guessed from a noun: (feminine, plural).
pub fn guess_noun(word: &str) -> (bool, bool) {
    let w = word.to_lowercase();
    if FEMININE_PLURALS.contains(&w.as_str()) {
        (true, true)
    } else if w.ends_with("ione") || w.ends_with('à') || w.ends_with('a') {
        (true, false)
    } else if w.ends_with('i') {
        (false, true)
    } else {
        (false, false)
    }
}

/// Splits a carriage name into its article and the rest: «Il Refettorio» →
/// (Il, "Refettorio"), «Alveare» → (L, "Alveare"). None if the name starts
/// with something that takes no article (a number).
pub fn place_article(name: &str) -> (Option<Article>, &str) {
    for (prefix, art) in [
        ("Il ", Article::Il),
        ("Lo ", Article::Lo),
        ("La ", Article::La),
        ("I ", Article::I),
        ("Gli ", Article::Gli),
        ("Le ", Article::Le),
        ("L'", Article::L),
        ("L’", Article::L),
    ] {
        if let Some(rest) = name.strip_prefix(prefix)
            && !rest.is_empty()
        {
            return (Some(art), rest);
        }
    }
    let head = name.split(' ').next().unwrap_or(name);
    if !first_char(head).is_some_and(char::is_alphabetic) {
        return (None, name);
    }
    let (feminine, plural) = guess_noun(head);
    (Some(article(head, feminine, plural)), name)
}

/// `prep` + the carriage `name`: "al Refettorio", "nell'Alveare".
pub fn place_with(prep: Option<Prep>, name: &str) -> String {
    match place_article(name) {
        (Some(art), rest) => format!("{}{rest}", art.with(prep)),
        (None, rest) => {
            let p = match prep {
                None => "",
                Some(Prep::A) => a_ad(rest),
                Some(Prep::In) => "in",
                Some(Prep::Di) => return format!("{}{rest}", di(rest)),
                Some(Prep::Da) => "da",
                Some(Prep::Su) => "su",
            };
            if p.is_empty() {
                rest.to_string()
            } else {
                format!("{p} {rest}")
            }
        }
    }
}

/// "al Refettorio", "alla Brace", "all'Alveare", "alle Cuccette Nord".
pub fn prep_a(name: &str) -> String {
    place_with(Some(Prep::A), name)
}

/// "nel Refettorio", "nella Brace", "nell'Alveare", "nelle Brande".
pub fn prep_in(name: &str) -> String {
    place_with(Some(Prep::In), name)
}

/// "del Refettorio", "della Brace", "dell'Alveare", "delle Brande".
pub fn prep_di(name: &str) -> String {
    place_with(Some(Prep::Di), name)
}

/// Gender and number of an item from its catalog names (`name` singular,
/// `plural`): (feminine, plural). Mass nouns ("cotone", "tè") have the same
/// name for both and stay singular.
pub fn item_noun(name: &str, plural: &str) -> (bool, bool) {
    let (feminine, many) = guess_noun(name);
    (feminine, many || plural != name)
}

/// Regular Italian plural of a noun: "barra" → "barre", "pezzo" → "pezzi",
/// "pacco" → "pacchi", "mensola" → "mensole", "razione" → "razioni".
pub fn pluralize(word: &str) -> String {
    let stem = |n: usize| &word[..word.len() - n];
    for (end, plural) in [
        ("ca", "che"),
        ("ga", "ghe"),
        ("co", "chi"),
        ("go", "ghi"),
        ("a", "e"),
        ("o", "i"),
        ("e", "i"),
    ] {
        if word.ends_with(end) {
            return format!("{}{plural}", stem(end.len()));
        }
    }
    word.to_string()
}

/// A small number in words ("due", "tre"…), digits from 11 on.
pub fn number_word(n: u32) -> String {
    const WORDS: [&str; 11] = [
        "zero", "uno", "due", "tre", "quattro", "cinque", "sei", "sette", "otto", "nove", "dieci",
    ];
    WORDS
        .get(n as usize)
        .map_or_else(|| n.to_string(), |w| w.to_string())
}

/// `n` units of an item whose single unit reads `one` ("una barra di
/// metallo", "un attrezzo") and whose plural name is `plural`: "una barra di
/// metallo", "due barre di metallo", "tre attrezzi".
pub fn counted(n: u32, one: &str, plural: &str) -> String {
    if n == 1 {
        return one.to_string();
    }
    let number = number_word(n);
    let mut words = one.splitn(3, ' ');
    match (words.next(), words.next(), words.next()) {
        (Some(_article), Some(unit), Some(rest)) if rest.starts_with("di ") => {
            format!("{number} {} {rest}", pluralize(unit))
        }
        _ => format!("{number} {plural}"),
    }
}

/// "N gettoni", "un gettone".
pub fn tokens(n: u32) -> String {
    if n == 1 {
        "un gettone".to_string()
    } else {
        format!("{n} gettoni")
    }
}

/// An hour with "da" (`prep_da`) or "a": "dalle 7", "alle 16"; written with
/// digits only 1 elides ("dall'1", "all'1").
pub fn at_hour(prep_da: bool, hour: u32) -> String {
    match (prep_da, hour) {
        (true, 1) => "dall'1".to_string(),
        (false, 1) => "all'1".to_string(),
        (true, h) => format!("dalle {h}"),
        (false, h) => format!("alle {h}"),
    }
}

/// A family by its surname: "Rossi" → (I, "Rossi"), "Esposito" → (Gli, ..).
pub fn family_article(surname: &str) -> Article {
    article(surname, false, true)
}

/// "i Rossi", "gli Esposito"; with `prep`: "dei Rossi", "agli Esposito".
pub fn family(prep: Option<Prep>, surname: &str) -> String {
    format!("{}{surname}", family_article(surname).with(prep))
}

/// "e" or "ed" before `next` (d eufonica before e-: "Bruno ed Elena").
pub fn e_ed(next: &str) -> &'static str {
    if first_char(next).is_some_and(|c| matches!(c, 'e' | 'è' | 'é')) {
        "ed"
    } else {
        "e"
    }
}

/// "a" or "ad" before `next` ("ad Anna").
pub fn a_ad(next: &str) -> &'static str {
    if first_char(next).is_some_and(|c| matches!(c, 'a' | 'à')) {
        "ad"
    } else {
        "a"
    }
}

/// "di " or "d'" before `next` (elided before a vowel: "d'inverno").
pub fn di(next: &str) -> &'static str {
    if starts_with_vowel(next) { "d'" } else { "di " }
}

/// First letter uppercase ("al Nido c'è…" → "Al Nido c'è…"); leading
/// punctuation ("…già") is left alone.
pub fn capitalize(line: &str) -> String {
    let mut chars = line.chars();
    match chars.next() {
        Some(c) if c.is_lowercase() => c.to_uppercase().chain(chars).collect(),
        _ => line.to_string(),
    }
}

/// Collapses runs of spaces and trims the line.
pub fn tidy(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for word in line.split(' ').filter(|w| !w.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// Grammar slips in a finished line, for the tests: "a A…" (needs "ad"),
/// "e E…" (needs "ed"), a bare preposition before a carriage name that
/// takes an article, double spaces, a lowercase first letter.
pub fn slips(line: &str, places: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    if line.contains("  ") {
        found.push("double space".to_string());
    }
    if line != line.trim() {
        found.push("untrimmed".to_string());
    }
    if line.chars().next().is_some_and(char::is_lowercase) {
        found.push("lowercase start".to_string());
    }
    let words: Vec<&str> = line
        .split(|c: char| {
            c.is_whitespace() || matches!(c, ',' | '!' | '?' | '.' | '…' | ':' | '«' | '»')
        })
        .filter(|w| !w.is_empty())
        .collect();
    for pair in words.windows(2) {
        let (w, next) = (pair[0], pair[1]);
        let lw = w.to_lowercase();
        if lw == "a" && a_ad(next) == "ad" {
            found.push(format!("«{w} {next}»"));
        }
        if lw == "e" && e_ed(next) == "ed" {
            found.push(format!("«{w} {next}»"));
        }
    }
    for place in places {
        let (art, rest) = place_article(place);
        if art.is_none() {
            continue;
        }
        for bare in ["a", "in", "di", "da", "su", "A", "In", "Di", "Da", "Su"] {
            if line.contains(&format!("{bare} {place}")) || line.contains(&format!("{bare} {rest}"))
            {
                // "a Refettorio" but not "al Refettorio".
                let at = line.find(&format!("{bare} {rest}")).unwrap_or(0);
                let before = &line[..at];
                if at == 0 || before.ends_with(' ') {
                    found.push(format!("«{bare} {rest}»"));
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names;

    #[test]
    fn carriage_names_get_the_right_preposition() {
        let cases = [
            ("Alveare", "all'Alveare", "nell'Alveare", "dell'Alveare"),
            (
                "Cuccette Nord",
                "alle Cuccette Nord",
                "nelle Cuccette Nord",
                "delle Cuccette Nord",
            ),
            ("Le Brande", "alle Brande", "nelle Brande", "delle Brande"),
            ("Nido", "al Nido", "nel Nido", "del Nido"),
            (
                "Il Refettorio",
                "al Refettorio",
                "nel Refettorio",
                "del Refettorio",
            ),
            (
                "Pentola Comune",
                "alla Pentola Comune",
                "nella Pentola Comune",
                "della Pentola Comune",
            ),
            ("La Brace", "alla Brace", "nella Brace", "della Brace"),
            (
                "Giardino d'Inverno",
                "al Giardino d'Inverno",
                "nel Giardino d'Inverno",
                "del Giardino d'Inverno",
            ),
            (
                "Orto Pensile",
                "all'Orto Pensile",
                "nell'Orto Pensile",
                "dell'Orto Pensile",
            ),
            ("La Vigna", "alla Vigna", "nella Vigna", "della Vigna"),
            ("La Fucina", "alla Fucina", "nella Fucina", "della Fucina"),
            ("Bullone", "al Bullone", "nel Bullone", "del Bullone"),
            (
                "Officina Grande",
                "all'Officina Grande",
                "nell'Officina Grande",
                "dell'Officina Grande",
            ),
            ("Il Bazar", "al Bazar", "nel Bazar", "del Bazar"),
            (
                "La Bottega",
                "alla Bottega",
                "nella Bottega",
                "della Bottega",
            ),
            ("Il Baratto", "al Baratto", "nel Baratto", "del Baratto"),
            (
                "Dormitorio 6",
                "al Dormitorio 6",
                "nel Dormitorio 6",
                "del Dormitorio 6",
            ),
            ("Mensa 4", "alla Mensa 4", "nella Mensa 4", "della Mensa 4"),
            ("Serra 3", "alla Serra 3", "nella Serra 3", "della Serra 3"),
            (
                "Officina 4",
                "all'Officina 4",
                "nell'Officina 4",
                "dell'Officina 4",
            ),
            (
                "Mercato 5",
                "al Mercato 5",
                "nel Mercato 5",
                "del Mercato 5",
            ),
            (
                "Lo Sgabuzzino",
                "allo Sgabuzzino",
                "nello Sgabuzzino",
                "dello Sgabuzzino",
            ),
            (
                "Stazione",
                "alla Stazione",
                "nella Stazione",
                "della Stazione",
            ),
            ("Bagni", "ai Bagni", "nei Bagni", "dei Bagni"),
            ("Specchi", "agli Specchi", "negli Specchi", "degli Specchi"),
            ("7", "a 7", "in 7", "di 7"),
        ];
        for (name, a, inn, di) in cases {
            assert_eq!(prep_a(name), a, "{name}");
            assert_eq!(prep_in(name), inn, "{name}");
            assert_eq!(prep_di(name), di, "{name}");
        }
        // Every name the generator uses is covered above.
        for list in [
            names::DORM_NAMES,
            names::MENSA_NAMES,
            names::SERRA_NAMES,
            names::OFFICINA_NAMES,
            names::MERCATO_NAMES,
        ] {
            for name in list {
                assert!(cases.iter().any(|c| c.0 == *name), "{name} not tested");
            }
        }
    }

    #[test]
    fn families_euphony_and_elision() {
        assert_eq!(family(None, "Rossi"), "i Rossi");
        assert_eq!(family(None, "Esposito"), "gli Esposito");
        assert_eq!(family(Some(Prep::Di), "Esposito"), "degli Esposito");
        assert_eq!(family(Some(Prep::A), "De Luca"), "ai De Luca");
        assert_eq!(family(Some(Prep::Di), "Zanetti"), "degli Zanetti");
        assert_eq!(e_ed("Elena"), "ed");
        assert_eq!(e_ed("è"), "ed");
        assert_eq!(e_ed("Anna"), "e");
        assert_eq!(a_ad("Anna"), "ad");
        assert_eq!(a_ad("Elena"), "a");
        assert_eq!(di("inverno"), "d'");
        assert_eq!(di("Refettorio"), "di ");
        assert_eq!(article("attrezzi", false, true), Article::Gli);
        assert_eq!(article("vestiti", false, true), Article::I);
        assert_eq!(article("operaia", true, false), Article::L);
        assert_eq!(article("zio", false, false), Article::Lo);
        assert_eq!(capitalize("al Nido c'è gente."), "Al Nido c'è gente.");
        assert_eq!(capitalize("…già."), "…già.");
        assert_eq!(tidy(" Ciao  Anna "), "Ciao Anna");
    }

    #[test]
    fn counts_and_plurals() {
        assert_eq!(pluralize("barra"), "barre");
        assert_eq!(pluralize("pezzo"), "pezzi");
        assert_eq!(pluralize("pacco"), "pacchi");
        assert_eq!(pluralize("razione"), "razioni");
        assert_eq!(
            counted(1, "una barra di metallo", "metallo"),
            "una barra di metallo"
        );
        assert_eq!(
            counted(2, "una barra di metallo", "metallo"),
            "due barre di metallo"
        );
        assert_eq!(
            counted(3, "un pezzo di rottame", "rottami"),
            "tre pezzi di rottame"
        );
        assert_eq!(
            counted(2, "una cassetta di verdura", "verdure"),
            "due cassette di verdura"
        );
        assert_eq!(counted(2, "un attrezzo", "attrezzi"), "due attrezzi");
        assert_eq!(counted(12, "una razione", "razioni"), "12 razioni");
        assert_eq!(item_noun("verdura", "verdure"), (true, true));
        assert_eq!(item_noun("cotone", "cotone"), (false, false));
        assert_eq!(item_noun("erbe", "erbe"), (true, true));
        assert_eq!(item_noun("tè", "tè"), (false, false));
        assert_eq!(tokens(1), "un gettone");
        assert_eq!(tokens(12), "12 gettoni");
        assert_eq!(at_hour(true, 7), "dalle 7");
        assert_eq!(at_hour(false, 1), "all'1");
    }

    #[test]
    fn slips_are_found() {
        let places = ["Alveare", "Il Refettorio"];
        assert!(!slips("Lo dicono tutti a Alveare.", &places).is_empty());
        assert!(!slips("Bruno e Elena stanno insieme!", &places).is_empty());
        assert!(!slips("Ne parlano al  Nido.", &places).is_empty());
        assert!(!slips("al Nido c'è gente.", &places).is_empty());
        assert!(!slips("Oggi a Refettorio si mangia.", &places).is_empty());
        assert!(slips("Lo dicono tutti all'Alveare.", &places).is_empty());
        assert!(slips("Bruno ed Elena stanno insieme!", &places).is_empty());
        assert!(slips("Oggi al Refettorio si mangia.", &places).is_empty());
        assert!(slips("…già.", &places).is_empty());
    }
}
