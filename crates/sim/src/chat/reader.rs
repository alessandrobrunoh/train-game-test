//! Capire il testo libero della chat: da una frase in italiano a un [`Intent`].
//!
//! [`KeywordReader`] is deterministic and knows no grammar: it normalizes
//! the text (lowercase, no accents, no punctuation, no courtesy words like
//! "per favore", stretched vowels squeezed: "ciaooo" → "ciao") and scores
//! every intent by the keywords it contains. A keyword is a word ("quanto"),
//! a stem ("lavor*": lavoro, lavori, lavorare) or a phrase ("che si dice");
//! words and stems of five letters or more also match with a typo (one
//! edit, a swap of two letters counting as one; two edits from eight
//! letters). Each keyword counts once; the scores are weighted so that a
//! greeting in front of a question does not hide the question ("Ciao, quanto
//! costa?" asks for prices). Below [`MIN_SCORE`] the reader gives up and the
//! NPC says it did not understand.
//!
//! [`IntentReader`] is the seam for smarter readers (an LLM from the future
//! `llm` crate): anything that maps a text to an intent can drive the chat.

use super::Intent;

/// Anything that understands what the player typed.
pub trait IntentReader {
    /// The intent of `text`, or None if it can't tell.
    fn read(&self, text: &str) -> Option<Intent>;
}

/// Keyword and typo-tolerant matcher (see the module docs).
#[derive(Clone, Copy, Debug, Default)]
pub struct KeywordReader;

/// Least score for an answer.
const MIN_SCORE: f32 = 1.2;
const EXACT: f32 = 3.0;
const PHRASE: f32 = 4.0;
const ONE_TYPO: f32 = 2.0;
const TWO_TYPOS: f32 = 1.3;
/// Words and stems shorter than this must match exactly.
const FUZZY_FROM: usize = 5;
/// Two typos are tolerated from this length.
const TWO_TYPOS_FROM: usize = 8;

/// Courtesy phrases removed before matching ("per favore" is not a favour).
const COURTESY: &[&str] = &[
    "per favore",
    "per piacere",
    "per cortesia",
    "scusa",
    "scusi",
];

/// Keywords of each intent with its weight; on equal scores the earlier
/// rows win. `*` ends a stem, a space makes a phrase.
const TABLE: &[(Intent, f32, &[&str])] = &[
    (
        Intent::Insult,
        1.2,
        &[
            "idiota",
            "idioti",
            "stupid*",
            "cretin*",
            "scemo",
            "scema",
            "scemi",
            "imbecill*",
            "deficient*",
            "stronz*",
            "vaffanculo",
            "fanculo",
            "cogli*",
            "somaro",
            "somara",
            "asino",
            "babbeo",
            "babbea",
            "tonto",
            "tonta",
            "verme",
            "schifos*",
            "puzzi",
            "puzzolent*",
            "inutile",
            "sfigat*",
            "fallito",
            "fallita",
            "odio",
            "taci",
            "zitto",
            "zitta",
            "cafone",
            "cafona",
            "ignorante",
            "maledett*",
            "pezzente",
            "antipatic*",
            "insopportabil*",
            "fai schifo",
            "vai al diavolo",
            "sei brutto",
            "sei brutta",
            "ti odio",
            "lasciami in pace",
            "mi dai sui nervi",
        ],
    ),
    (
        Intent::AskFavour,
        1.0,
        &[
            "favor*",
            "aiut*",
            "incaric*",
            "compit*",
            "commission*",
            "mission*",
            "bisogno",
            "servirebbe",
            "occorre",
            "ti serve",
            "vi serve",
            "le serve",
            "una mano",
            "posso fare",
            "hai bisogno",
            "consegna",
            "ecco",
        ],
    ),
    (
        Intent::AskPrices,
        1.0,
        &[
            "prezz*",
            "costa",
            "costano",
            "costo",
            "costi",
            "convien*",
            "convenient*",
            "economic*",
            "listin*",
            "spend*",
            "quanto",
            "quanti",
            "caro",
            "cari",
            "cara",
            "care",
            "costa meno",
            "dove compro",
            "dove comprare",
            "quanto viene",
        ],
    ),
    (
        Intent::AskJob,
        1.0,
        &[
            "lavor*",
            "mestier*",
            "impieg*",
            "occupi",
            "occupazion*",
            "profession*",
            "turno",
            "turni",
            "mansion*",
            "cosa fai",
            "che fai",
            "di cosa ti occupi",
        ],
    ),
    (
        Intent::Trade,
        1.0,
        &[
            "scambi*",
            "compr*",
            "vend*",
            "barat*",
            "merce",
            "mercanzi*",
            "affar*",
            "negozi*",
            "commerci*",
            "bottega",
            "banco",
            "cos hai",
            "fare affari",
        ],
    ),
    (
        Intent::Gift,
        1.0,
        &[
            "regal*",
            "dono",
            "doni",
            "donare",
            "omaggio",
            "pensierino",
            "tieni",
            "ho qualcosa per te",
            "per te",
            "ti do",
            "ti ho portato",
        ],
    ),
    (
        Intent::AskNews,
        1.0,
        &[
            "notizi*",
            "novit*",
            "pettegol*",
            "gossip",
            "successo",
            "accad*",
            "raccont*",
            "news",
            "voci",
            "nuove",
            "che si dice",
            "cosa si dice",
            "si dice",
            "hai sentito",
            "le ultime",
            "di nuovo",
            "cosa dicono",
            "dicono di me",
        ],
    ),
    (
        Intent::Farewell,
        0.8,
        &[
            "arrivederci",
            "arrivederla",
            "addio",
            "buonanotte",
            "vado",
            "bye",
            "ciao ciao",
            "a presto",
            "alla prossima",
            "ci vediamo",
            "devo andare",
            "a dopo",
            "a domani",
            "me ne vado",
            "ti saluto",
            "buona giornata",
            "buona serata",
            "buona notte",
        ],
    ),
    (
        Intent::Greet,
        0.6,
        &[
            "ciao",
            "salve",
            "buongiorno",
            "buonasera",
            "buondi",
            "hey",
            "ehi",
            "ehila",
            "hola",
            "hello",
            "bentrovat*",
            "piacere",
            "come stai",
            "come va",
            "tutto bene",
            "buon pomeriggio",
            "buona sera",
            "buon giorno",
            "come te la passi",
        ],
    ),
];

impl IntentReader for KeywordReader {
    fn read(&self, text: &str) -> Option<Intent> {
        let text = normalize(text);
        if text.is_empty() {
            return None;
        }
        let padded = format!(" {text} ");
        let tokens: Vec<&str> = text.split(' ').collect();
        let mut best: Option<(Intent, f32)> = None;
        for &(intent, weight, keys) in TABLE {
            let score = weight
                * keys
                    .iter()
                    .map(|k| key_score(k, &padded, &tokens))
                    .sum::<f32>();
            if score >= MIN_SCORE && best.is_none_or(|(_, b)| score > b) {
                best = Some((intent, score));
            }
        }
        best.map(|(intent, _)| intent)
    }
}

/// Lowercase ASCII words separated by single spaces: accents removed,
/// punctuation and apostrophes turned into spaces, courtesy phrases dropped,
/// runs of three or more equal letters and stretched final vowels squeezed.
pub fn normalize(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    for c in text.chars().flat_map(char::to_lowercase) {
        let c = match c {
            'à' | 'á' | 'â' | 'ä' => 'a',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ò' | 'ó' | 'ô' | 'ö' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            c if c.is_ascii_alphanumeric() => c,
            _ => ' ',
        };
        plain.push(c);
    }
    let words: Vec<String> = plain.split_whitespace().map(squeeze).collect();
    let mut joined = format!(" {} ", words.join(" "));
    for courtesy in COURTESY {
        joined = joined.replace(&format!(" {courtesy} "), " ");
    }
    joined.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// "ciaooo" → "ciao", "grazieee" → "grazie", "nooo" → "no", "brrr" → "brr".
fn squeeze(word: &str) -> String {
    let chars: Vec<char> = word.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    for &c in &chars {
        let n = out.len();
        if n >= 2 && out[n - 1] == c && out[n - 2] == c {
            continue;
        }
        out.push(c);
    }
    // A doubled final vowel is stretched speech ("ciaoo"), not Italian.
    while out.len() >= 3 {
        let n = out.len();
        if out[n - 1] == out[n - 2] && matches!(out[n - 1], 'a' | 'e' | 'o' | 'u') {
            out.pop();
        } else {
            break;
        }
    }
    out.into_iter().collect()
}

/// How well `key` matches the text (0 if it doesn't).
fn key_score(key: &str, padded: &str, tokens: &[&str]) -> f32 {
    if key.contains(' ') {
        return if padded.contains(&format!(" {key} ")) {
            PHRASE
        } else {
            0.0
        };
    }
    let (stem, is_stem) = match key.strip_suffix('*') {
        Some(stem) => (stem, true),
        None => (key, false),
    };
    let stem_len = stem.chars().count();
    let mut best: f32 = 0.0;
    for token in tokens {
        let exact = if is_stem {
            token.starts_with(stem)
        } else {
            *token == stem
        };
        if exact {
            return EXACT;
        }
        if stem_len < FUZZY_FROM {
            continue;
        }
        // A stem is compared with the start of the word (±1 letter).
        let edits = if is_stem {
            let chars: Vec<char> = token.chars().collect();
            (stem_len.saturating_sub(1)..=stem_len + 1)
                .filter(|&n| n <= chars.len() && n > 0)
                .map(|n| {
                    let prefix: String = chars[..n].iter().collect();
                    distance(&prefix, stem)
                })
                .min()
                .unwrap_or(usize::MAX)
        } else {
            distance(token, stem)
        };
        let score = match edits {
            1 => ONE_TYPO,
            2 if stem_len >= TWO_TYPOS_FROM => TWO_TYPOS,
            _ => 0.0,
        };
        best = best.max(score);
    }
    best
}

/// Optimal string alignment distance: insertions, deletions, substitutions
/// and swaps of two adjacent letters.
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > 2 {
        return a.len().abs_diff(b.len());
    }
    let (n, m) = (a.len(), b.len());
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[n][m]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Option<Intent> {
        KeywordReader.read(text)
    }

    #[test]
    fn normalizes_accents_punctuation_and_stretched_words() {
        assert_eq!(normalize("  Novità?!  Ciaooo, "), "novita ciao");
        assert_eq!(normalize("C'è qualcosa di nuovo?"), "c e qualcosa di nuovo");
        assert_eq!(normalize("Per favore, dimmi i PREZZI"), "dimmi i prezzi");
        assert_eq!(normalize("Grazieee mille"), "grazie mille");
        assert_eq!(normalize("…"), "");
    }

    #[test]
    fn typo_distance() {
        assert_eq!(distance("lavoro", "lavoro"), 0);
        assert_eq!(distance("lavroo", "lavoro"), 1);
        assert_eq!(distance("prezo", "prezzo"), 1);
        assert_eq!(distance("arivederci", "arrivederci"), 1);
        assert_eq!(distance("notzie", "notizie"), 1);
        assert_eq!(distance("abc", "xyz"), 3);
    }

    #[test]
    fn maps_varied_phrasings_to_the_right_intent() {
        let cases: &[(&str, Intent)] = &[
            // Greetings.
            ("Ciao!", Intent::Greet),
            ("ciaooo", Intent::Greet),
            ("Buongiorno Marta", Intent::Greet),
            ("salve, come stai?", Intent::Greet),
            ("Ehi, tutto bene?", Intent::Greet),
            ("buon giorno", Intent::Greet),
            ("buongiorn", Intent::Greet),
            // Work.
            ("Che lavoro fai?", Intent::AskJob),
            ("di cosa ti occupi", Intent::AskJob),
            ("Come va il lavoro?", Intent::AskJob),
            ("dove lavori?", Intent::AskJob),
            ("che mestiere fai", Intent::AskJob),
            ("che lavroo fai", Intent::AskJob),
            ("a che ora inizia il tuo turno?", Intent::AskJob),
            // Prices.
            ("Quanto costa un attrezzo?", Intent::AskPrices),
            ("Ciao, quanto costano i vestiti?", Intent::AskPrices),
            ("dove costa meno la roba?", Intent::AskPrices),
            ("I prezzi sono alti?", Intent::AskPrices),
            ("sai i prezi del mercato?", Intent::AskPrices),
            ("dove conviene comprare?", Intent::AskPrices),
            ("Per favore, dimmi i prezzi", Intent::AskPrices),
            // Favours.
            ("Posso fare qualcosa per te?", Intent::AskFavour),
            ("Hai bisogno di aiuto?", Intent::AskFavour),
            ("ti serve una mano?", Intent::AskFavour),
            ("hai un incarico per me?", Intent::AskFavour),
            ("mi fai un favore?", Intent::AskFavour),
            ("posso aiutarti?", Intent::AskFavour),
            ("posso aiutrati", Intent::AskFavour),
            ("Ecco quello che mi avevi chiesto", Intent::AskFavour),
            // Gifts.
            ("Ho un regalo per te", Intent::Gift),
            ("tieni, è per te", Intent::Gift),
            ("vorrei regalarti una cosa", Intent::Gift),
            ("ti ho portato un pensierino", Intent::Gift),
            ("un regaloo per te", Intent::Gift),
            // Trade.
            ("Cosa vendi?", Intent::Trade),
            ("facciamo uno scambio?", Intent::Trade),
            ("vorrei comprare qualcosa", Intent::Trade),
            ("vendo rottami, ti interessa?", Intent::Trade),
            ("facciamo affari", Intent::Trade),
            ("scambiamo qualcosa?", Intent::Trade),
            // News.
            ("Novità?", Intent::AskNews),
            ("Che si dice in giro?", Intent::AskNews),
            ("hai sentito le ultime?", Intent::AskNews),
            ("raccontami qualche pettegolezzo", Intent::AskNews),
            ("cosa è successo?", Intent::AskNews),
            ("c'è qualcosa di nuovo?", Intent::AskNews),
            ("notzie?", Intent::AskNews),
            ("cosa dicono di me?", Intent::AskNews),
            // Insults.
            ("Sei un idiota", Intent::Insult),
            ("stupido!", Intent::Insult),
            ("sei proprio uno stupdo", Intent::Insult),
            ("fai schifo", Intent::Insult),
            ("vai al diavolo, cretino", Intent::Insult),
            ("Ciao, imbecille", Intent::Insult),
            ("sei insopportabile", Intent::Insult),
            // Goodbyes.
            ("Arrivederci", Intent::Farewell),
            ("arivederci!", Intent::Farewell),
            ("ciao ciao", Intent::Farewell),
            ("A presto!", Intent::Farewell),
            ("devo andare", Intent::Farewell),
            ("ci vediamo domani", Intent::Farewell),
            ("buonanotte", Intent::Farewell),
        ];
        let wrong: Vec<String> = cases
            .iter()
            .filter(|(text, intent)| read(text) != Some(*intent))
            .map(|(text, intent)| format!("«{text}»: {:?} instead of {intent:?}", read(text)))
            .collect();
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    #[test]
    fn falls_back_when_it_does_not_understand() {
        for text in [
            "",
            "   ",
            "?!",
            "banana",
            "il treno è lungo",
            "hai del cibo",
            "qwerty asdf",
            "grazie",
            "per favore",
        ] {
            assert_eq!(read(text), None, "«{text}»");
        }
    }
}
