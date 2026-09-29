//! Nomi: come il Custode confronta i nomi scritti dal modello con quelli
//! del catalogo.

/// The key of a name: lowercase, without accents and apostrophes, single
/// spaces. "Tè", "te" and " TE " are the same name.
pub fn normalize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.trim().chars().flat_map(char::to_lowercase) {
        let c = match c {
            'à' | 'á' | 'â' | 'ä' => 'a',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ò' | 'ó' | 'ô' | 'ö' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            '\'' | '’' | '-' | '_' => ' ',
            c => c,
        };
        if c == ' ' && (out.is_empty() || out.ends_with(' ')) {
            continue;
        }
        out.push(c);
    }
    out.trim_end().to_string()
}

/// Words after which a noun phrase goes on unchanged in the plural
/// ("borraccia *di* latta" → "borracce di latta").
const LINKS: [&str; 14] = [
    "di", "del", "della", "dei", "delle", "da", "dal", "a", "al", "per", "con", "in", "su", "e",
];

/// The Italian plural of one lowercase word (a guess): "borraccia" →
/// "borracce", "lume" → "lumi", "bacca" → "bacche", "tè" and "bar" stay.
fn plural_word(word: &str) -> String {
    let stem = |n: usize| &word[..word.len() - n];
    if word.ends_with("cia") || word.ends_with("gia") {
        // borraccia → borracce, but camicia → camicie: keep it simple.
        format!("{}e", stem(2))
    } else if word.ends_with("ca") {
        format!("{}che", stem(2))
    } else if word.ends_with("ga") {
        format!("{}ghe", stem(2))
    } else if word.ends_with("co") && word.chars().count() > 3 {
        format!("{}chi", stem(2))
    } else if word.ends_with("go") {
        format!("{}ghi", stem(2))
    } else if word.ends_with("io") {
        format!("{}i", stem(2))
    } else if word.ends_with('a') {
        format!("{}e", stem(1))
    } else if word.ends_with('o') || word.ends_with('e') {
        format!("{}i", stem(1))
    } else {
        word.to_string()
    }
}

/// A guess at the Italian plural of a lowercase noun phrase: the noun and
/// the adjectives after it change, up to a preposition ("sciarpa grezza" →
/// "sciarpe grezze", "filtro d'acqua" → "filtri d'acqua", "borraccia di
/// latta" → "borracce di latta"); words ending in a consonant or an
/// accented vowel don't change ("tè", "bar").
pub fn plural_of(name: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut linked = false;
    for word in name.split_whitespace() {
        let lower = word.to_lowercase();
        if !linked && (LINKS.contains(&lower.as_str()) || lower.contains('\'')) {
            linked = true;
        }
        if linked {
            out.push(word.to_string());
        } else {
            out.push(plural_word(word));
        }
    }
    out.join(" ")
}

/// The plural definite article of `plural` ("gli operai", "i contadini").
pub fn plural_article(plural: &str) -> &'static str {
    let first = plural.trim().to_lowercase();
    let s_impure =
        first.starts_with('s') && first.chars().nth(1).is_some_and(|c| !"aeiou".contains(c));
    let gli = first.starts_with(['a', 'e', 'i', 'o', 'u'])
        || s_impure
        || first.starts_with('z')
        || first.starts_with("gn")
        || first.starts_with("ps");
    if gli { "gli" } else { "i" }
}

/// "un"/"una"/"uno"/"l'" + name: the indefinite article, guessed from the
/// first word (feminine in -a, "uno" before s+consonant, z, gn, ps).
pub fn with_article(name: &str) -> String {
    let name = name.trim();
    let first = name.split(' ').next().unwrap_or_default().to_lowercase();
    let feminine = first.ends_with('a') || first.ends_with("ione") || first.ends_with("tà");
    let starts_vowel = first.starts_with(['a', 'e', 'i', 'o', 'u']);
    let s_impure =
        first.starts_with('s') && first.chars().nth(1).is_some_and(|c| !"aeiou".contains(c));
    let uno =
        s_impure || first.starts_with('z') || first.starts_with("gn") || first.starts_with("ps");
    let article = match (feminine, starts_vowel) {
        (true, true) => "un'",
        (true, false) => "una ",
        (false, _) if uno => "uno ",
        (false, _) => "un ",
    };
    format!("{article}{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_normalized() {
        assert_eq!(normalize("  Tè "), "te");
        assert_eq!(normalize("Filtro D'Acqua"), "filtro d acqua");
        assert_eq!(normalize("CAFFÈ  nero"), "caffe nero");
    }

    #[test]
    fn plurals_and_articles() {
        assert_eq!(plural_of("borraccia"), "borracce");
        assert_eq!(plural_of("filtro d'acqua"), "filtri d'acqua");
        assert_eq!(plural_of("sciarpa grezza"), "sciarpe grezze");
        assert_eq!(plural_of("borraccia di latta"), "borracce di latta");
        assert_eq!(plural_article("operai"), "gli");
        assert_eq!(plural_article("contadini"), "i");
        assert_eq!(plural_of("lume"), "lumi");
        assert_eq!(plural_of("sciarpa di lana"), "sciarpe di lana");
        assert_eq!(plural_of("bacca"), "bacche");
        assert_eq!(plural_of("tè"), "tè");
        assert_eq!(with_article("borraccia"), "una borraccia");
        assert_eq!(with_article("ampolla"), "un'ampolla");
        assert_eq!(with_article("scialle"), "uno scialle");
        assert_eq!(with_article("filtro"), "un filtro");
    }
}
