//! Testo delle conversazioni: battute a modelli in italiano.
//!
//! Every conversation is a short script of 3–6 lines alternating speakers:
//! an opener by `a` (from the topic, the news, the family tie, the job), a
//! reply by `b` (from the tone and, for news, whether it is good, bad or a
//! scandal), then follow-ups and reactions, and a closing line. The
//! speakers' personality ([`Temper`]) may replace a line: the grumpy answer
//! curtly, the shy say "…", the gossips add "resti tra noi", the curious
//! ask, the complainers complain. Small children babble or talk like kids.
//!
//! Templates use placeholders, filled in by [`render`]:
//! - `{n}` the listener's first name, `{s}` the speaker's;
//! - `{a}` / `{f}` first name / surname of whom the talk is about, `{o}`
//!   their partner (couples);
//! - `{i}` the item of the news (plural), `{p}` at the carriage they are
//!   in ("al Refettorio");
//! - `{w}` / `{W}` where the speaker / listener works ("in serra"), `{j}`
//!   the speaker's job, `{k}` with its article ("l'operaia");
//! - `{S:x/y}`, `{L:x/y}`, `{A:x/y}`: the female / male form for the
//!   speaker, the listener, the subject ("stanc{S:a/o}").
//!
//! Variants are picked with a seeded RNG (deterministic); a template whose
//! text would exceed [`MAX_LINE_CHARS`] (long names) is skipped for the next.

use super::{Line, News, Tone, Topic, Valence};
use crate::deliberation::Grievance;
use crate::ids::NpcId;
use crate::item::ItemKind;
use crate::npc::{Job, RelationKind, Sex};
use crate::personality::{Personality, Temper};
use crate::time::GameTime;

/// Longest line, in characters (it must fit a speech bubble).
pub const MAX_LINE_CHARS: usize = 40;

/// Under this age people only babble; under [`KID_AGE`] they talk like kids.
const BABY_AGE: u32 = 4;
const KID_AGE: u32 = 14;

/// Someone taking part in a conversation.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Voice<'a> {
    pub id: NpcId,
    pub first: &'a str,
    pub sex: Sex,
    pub age: u32,
    pub job: Option<Job>,
    pub personality: Personality,
}

/// Whom the talk is about (maybe dead: the names come from the event).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Subject {
    pub first: String,
    pub surname: String,
    pub sex: Sex,
    /// Their partner's first name (couples), else empty.
    pub other: String,
}

/// The most pressing need, for [`Topic::Needs`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Need {
    Hunger,
    Tiredness,
    Loneliness,
}

/// Everything the text depends on.
#[derive(Clone, Debug)]
pub(crate) struct Script<'a> {
    pub topic: Topic,
    pub tone: Tone,
    pub news: Option<News>,
    /// Who starts.
    pub a: Voice<'a>,
    pub b: Voice<'a>,
    /// What `b` is to `a`, if family.
    pub tie: Option<RelationKind>,
    pub about: Option<&'a Subject>,
    /// Generic gossip: whether `a` likes whom it talks about.
    pub fond: bool,
    pub need: Need,
    /// Name of the carriage they are in.
    pub place: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    A,
    B,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Beat {
    Open,
    Reply,
    Follow,
    React,
    Close,
}

/// A tiny deterministic generator (SplitMix64) for picking variants: much
/// cheaper to seed than the world's ChaCha, and text needs no more.
struct Mix(u64);

impl Mix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (n > 0).
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    /// True with probability `p`.
    fn chance(&mut self, p: f64) -> bool {
        ((self.next() >> 11) as f64 / (1u64 << 53) as f64) < p
    }
}

/// Writes the lines of a conversation from `since` to `until`.
pub(crate) fn write(s: &Script, seed: u64, since: GameTime, until: GameTime) -> Vec<Line> {
    let mut rng = Mix(seed);
    let minutes = until.since(since).max(1);
    let mut n = (minutes / 8).clamp(3, 6) as i64;
    if s.a.personality.has(Temper::Chiacchierone) || s.b.personality.has(Temper::Chiacchierone) {
        n += 1;
    }
    if s.a.personality.has(Temper::Timido) && s.b.personality.has(Temper::Timido) {
        n -= 1;
    }
    let n = n.clamp(3, 6) as usize;
    let beats: &[Beat] = match n {
        3 => &[Beat::Open, Beat::Reply, Beat::Close],
        4 => &[Beat::Open, Beat::Reply, Beat::Follow, Beat::Close],
        5 => &[
            Beat::Open,
            Beat::Reply,
            Beat::Follow,
            Beat::React,
            Beat::Close,
        ],
        _ => &[
            Beat::Open,
            Beat::Reply,
            Beat::Follow,
            Beat::React,
            Beat::Follow,
            Beat::Close,
        ],
    };
    let mut lines: Vec<Line> = Vec::with_capacity(n);
    for (k, &beat) in beats.iter().enumerate() {
        let side = if k % 2 == 0 { Side::A } else { Side::B };
        let pool = pool_for(s, beat, side, k, &mut rng);
        let mut text = pick(pool, s, side, &mut rng).unwrap_or_else(|| fallback(beat).to_string());
        // Avoid saying the same thing twice in a row.
        if lines.iter().any(|l| l.text == text) {
            text = pick(pool, s, side, &mut rng).unwrap_or(text);
        }
        let speaker = match side {
            Side::A => s.a.id,
            Side::B => s.b.id,
        };
        let at = since + minutes * k as u64 / n as u64;
        lines.push(Line { speaker, at, text });
    }
    lines
}

fn fallback(beat: Beat) -> &'static str {
    match beat {
        Beat::Open => "Ciao!",
        Beat::Reply => "Già.",
        Beat::Follow => "Eh…",
        Beat::React => "Mh.",
        Beat::Close => "Ciao.",
    }
}

/// The pool of templates for one line.
fn pool_for(
    s: &Script,
    beat: Beat,
    side: Side,
    k: usize,
    rng: &mut Mix,
) -> &'static [&'static str] {
    let (me, you) = match side {
        Side::A => (&s.a, &s.b),
        Side::B => (&s.b, &s.a),
    };
    if me.age < BABY_AGE {
        return BABBLE;
    }
    if you.age < BABY_AGE && beat != Beat::Close {
        return TO_BABY;
    }
    let p = me.personality;
    let mut chance = |x: f64| rng.chance(x);
    if me.age < KID_AGE && matches!(beat, Beat::Reply | Beat::Follow | Beat::React) && chance(0.5) {
        return KID;
    }
    // Personality first (not on the opener, which carries the topic).
    match beat {
        Beat::Open => {}
        Beat::Reply if s.tone == Tone::Tense => {}
        Beat::Reply | Beat::React => {
            if p.has(Temper::Timido) && chance(0.35) {
                return SHY;
            }
            if p.has(Temper::Burbero) && chance(0.5) {
                return GRUMPY;
            }
            // The curious ask about what they just heard (not about a greeting).
            if p.has(Temper::Curioso)
                && s.tone != Tone::Tense
                && (beat == Beat::React || matches!(s.topic, Topic::News | Topic::Gossip))
                && chance(0.35)
            {
                return CURIOUS;
            }
            if p.has(Temper::Gentile)
                && (s.tone == Tone::Tense || matches!(s.topic, Topic::Needs | Topic::Complaint))
                && chance(0.4)
            {
                return KIND;
            }
            if beat == Beat::React && p.has(Temper::Allegro) && s.tone != Tone::Tense && chance(0.4)
            {
                return CHEERFUL;
            }
        }
        Beat::Follow => {
            if p.has(Temper::Pettegolo)
                && matches!(s.topic, Topic::Gossip | Topic::News)
                && chance(0.5)
            {
                return GOSSIPY;
            }
            if p.has(Temper::Lamentoso) && chance(0.4) {
                return LAMENT;
            }
            if p.has(Temper::Chiacchierone) && k > 2 && chance(0.4) {
                return CHATTY;
            }
            if p.has(Temper::Timido) && chance(0.3) {
                return SHY_FOLLOW;
            }
        }
        Beat::Close => {
            if p.has(Temper::Burbero) && chance(0.5) {
                return GRUMPY_CLOSE;
            }
            if p.has(Temper::Timido) && chance(0.3) {
                return SHY_CLOSE;
            }
        }
    }
    match beat {
        Beat::Open => open_pool(s),
        Beat::Reply => reply_pool(s),
        Beat::Follow => follow_pool(s),
        Beat::React => match s.tone {
            Tone::Friendly => REACT_FRIENDLY,
            Tone::Neutral => REACT_NEUTRAL,
            Tone::Tense => REACT_TENSE,
        },
        Beat::Close => match s.tone {
            Tone::Friendly => CLOSE_FRIENDLY,
            Tone::Neutral => CLOSE_NEUTRAL,
            Tone::Tense => CLOSE_TENSE,
        },
    }
}

/// Valence of what is talked about (News, Gossip).
fn valence(s: &Script) -> Valence {
    match s.news {
        Some(news) => news.valence(),
        None if s.fond => Valence::Good,
        None => Valence::Scandal,
    }
}

fn open_pool(s: &Script) -> &'static [&'static str] {
    match s.topic {
        Topic::SmallTalk => match s.tone {
            Tone::Friendly => SMALL_OPEN_FRIENDLY,
            Tone::Neutral => SMALL_OPEN_NEUTRAL,
            Tone::Tense => SMALL_OPEN_TENSE,
        },
        Topic::Work => match s.a.job {
            Some(Job::Contadino) => WORK_OPEN_CONTADINO,
            Some(Job::Cuoco) => WORK_OPEN_CUOCO,
            Some(Job::Operaio) => WORK_OPEN_OPERAIO,
            Some(Job::Mercante) => WORK_OPEN_MERCANTE,
            None => WORK_OPEN_ASK,
        },
        Topic::Needs => match s.need {
            Need::Hunger => NEEDS_OPEN_HUNGER,
            Need::Tiredness => NEEDS_OPEN_TIRED,
            Need::Loneliness => NEEDS_OPEN_LONELY,
        },
        Topic::Family => match s.tie {
            Some(RelationKind::Partner) => FAMILY_OPEN_PARTNER,
            Some(RelationKind::Child) => FAMILY_OPEN_TO_CHILD,
            Some(RelationKind::Parent) => FAMILY_OPEN_TO_PARENT,
            Some(RelationKind::Sibling) => FAMILY_OPEN_SIBLING,
            Some(RelationKind::Friend) | None => FAMILY_OPEN_FRIEND,
        },
        Topic::News => match (s.news, s.about) {
            (Some(news), _) => news_open(news, s.about.is_some()),
            (None, _) => SMALL_OPEN_NEUTRAL,
        },
        Topic::Gossip => match (s.news, s.about) {
            (_, None) => SMALL_OPEN_NEUTRAL,
            (Some(news), Some(_)) => gossip_open(news),
            (None, Some(_)) if s.fond => GOSSIP_OPEN_FOND,
            (None, Some(_)) => GOSSIP_OPEN_SPITE,
        },
        Topic::Complaint => match s.news {
            Some(News::Shortage(ItemKind::Razione)) => COMPLAINT_OPEN_FOOD,
            Some(News::BirthDenied) => COMPLAINT_OPEN_BIRTH,
            Some(News::Austerity) => COMPLAINT_OPEN_AUSTERITY,
            Some(News::PayCut) => COMPLAINT_OPEN_PAY,
            _ => COMPLAINT_OPEN,
        },
    }
}

fn news_open(news: News, about: bool) -> &'static [&'static str] {
    match news {
        News::Birth if about => NEWS_BIRTH,
        News::Death if about => NEWS_DEATH,
        News::Couple if about => NEWS_COUPLE,
        News::Widowed if about => NEWS_WIDOWED,
        News::TheftCaught if about => NEWS_THEFT_CAUGHT,
        News::HelpGiven if about => NEWS_HELP_GIVEN,
        News::HelpRefused if about => NEWS_HELP_REFUSED,
        News::BirthDenied if about => NEWS_BIRTH_DENIED,
        News::CameOfAge if about => NEWS_CAME_OF_AGE,
        News::Retired if about => NEWS_RETIRED,
        News::TheftUnseen => NEWS_THEFT_UNSEEN,
        News::Protest(Grievance::BirthDenied) => NEWS_PROTEST_BIRTHS,
        News::Protest(Grievance::FoodShortage) => NEWS_PROTEST_FOOD,
        News::Concession(Grievance::BirthDenied) => NEWS_CONCESSION_BIRTHS,
        News::Concession(Grievance::FoodShortage) => NEWS_CONCESSION_FOOD,
        News::Shortage(ItemKind::Razione) => NEWS_SHORTAGE_FOOD,
        News::Shortage(_) => NEWS_SHORTAGE,
        News::Restocked(_) => NEWS_RESTOCKED,
        News::Austerity => NEWS_AUSTERITY,
        News::PayRaised => NEWS_PAY_RAISED,
        News::PayCut => NEWS_PAY_CUT,
        // News about someone without their name: say it generically.
        _ => NEWS_VAGUE,
    }
}

fn gossip_open(news: News) -> &'static [&'static str] {
    match news {
        News::TheftCaught => GOSSIP_THEFT,
        News::Couple => GOSSIP_COUPLE,
        News::Birth => GOSSIP_BIRTH,
        News::Widowed => GOSSIP_WIDOWED,
        News::HelpRefused => GOSSIP_STINGY,
        News::HelpGiven => GOSSIP_GENEROUS,
        News::CameOfAge => GOSSIP_GROWN,
        _ => news_open(news, true),
    }
}

fn reply_pool(s: &Script) -> &'static [&'static str] {
    let by_tone = |f, n, t| match s.tone {
        Tone::Friendly => f,
        Tone::Neutral => n,
        Tone::Tense => t,
    };
    match s.topic {
        Topic::SmallTalk => by_tone(SMALL_REPLY_FRIENDLY, SMALL_REPLY_NEUTRAL, SMALL_REPLY_TENSE),
        // Asked about one's own work...
        Topic::Work if s.a.job.is_none() => {
            by_tone(WORK_ASKED_FRIENDLY, WORK_ASKED_NEUTRAL, WORK_ASKED_TENSE)
        }
        // ...or hearing about the other's.
        Topic::Work => by_tone(WORK_REPLY_FRIENDLY, WORK_REPLY_NEUTRAL, WORK_REPLY_TENSE),
        Topic::Needs => by_tone(NEEDS_REPLY_FRIENDLY, NEEDS_REPLY_NEUTRAL, NEEDS_REPLY_TENSE),
        Topic::Family => by_tone(
            FAMILY_REPLY_FRIENDLY,
            FAMILY_REPLY_NEUTRAL,
            FAMILY_REPLY_TENSE,
        ),
        Topic::Complaint => by_tone(
            COMPLAINT_REPLY_FRIENDLY,
            COMPLAINT_REPLY_NEUTRAL,
            COMPLAINT_REPLY_TENSE,
        ),
        Topic::Gossip if s.news.is_none() && s.fond => {
            by_tone(FOND_REPLY_FRIENDLY, FOND_REPLY_NEUTRAL, FOND_REPLY_TENSE)
        }
        Topic::Gossip if s.news.is_none() => {
            by_tone(SPITE_REPLY_FRIENDLY, SPITE_REPLY_NEUTRAL, SPITE_REPLY_TENSE)
        }
        Topic::News | Topic::Gossip => match valence(s) {
            Valence::Good => by_tone(GOOD_REPLY_FRIENDLY, GOOD_REPLY_NEUTRAL, GOOD_REPLY_TENSE),
            Valence::Bad => by_tone(BAD_REPLY_FRIENDLY, BAD_REPLY_NEUTRAL, BAD_REPLY_TENSE),
            Valence::Scandal => by_tone(
                SCANDAL_REPLY_FRIENDLY,
                SCANDAL_REPLY_NEUTRAL,
                SCANDAL_REPLY_TENSE,
            ),
        },
    }
}

fn follow_pool(s: &Script) -> &'static [&'static str] {
    match s.topic {
        Topic::SmallTalk => SMALL_FOLLOW,
        Topic::Work if s.a.job.is_some() => WORK_FOLLOW,
        Topic::Work => SMALL_FOLLOW,
        Topic::Needs => NEEDS_FOLLOW,
        Topic::Family => FAMILY_FOLLOW,
        Topic::Complaint => COMPLAINT_FOLLOW,
        Topic::Gossip => GOSSIP_FOLLOW,
        Topic::News => match valence(s) {
            Valence::Good => GOOD_FOLLOW,
            Valence::Bad => BAD_FOLLOW,
            Valence::Scandal => SCANDAL_FOLLOW,
        },
    }
}

/// Renders a random template of `pool` that fits [`MAX_LINE_CHARS`].
fn pick(pool: &[&str], s: &Script, side: Side, rng: &mut Mix) -> Option<String> {
    if pool.is_empty() {
        return None;
    }
    let start = rng.below(pool.len());
    (0..pool.len()).find_map(|k| {
        let text = render(pool[(start + k) % pool.len()], s, side);
        (text.chars().count() <= MAX_LINE_CHARS).then_some(text)
    })
}

/// "in serra", "in cucina"...: where someone with `job` works.
fn workplace(job: Option<Job>) -> &'static str {
    match job {
        Some(Job::Contadino) => "in serra",
        Some(Job::Cuoco) => "in cucina",
        Some(Job::Operaio) => "in officina",
        Some(Job::Mercante) => "al banco",
        None => "in giro",
    }
}

fn job_word(job: Option<Job>, sex: Sex) -> &'static str {
    match job {
        Some(Job::Contadino) => sex.pick("contadina", "contadino"),
        Some(Job::Cuoco) => sex.pick("cuoca", "cuoco"),
        Some(Job::Operaio) => sex.pick("operaia", "operaio"),
        Some(Job::Mercante) => "mercante",
        None => sex.pick("disoccupata", "disoccupato"),
    }
}

/// "al Refettorio", "alla Brace", "a Nido": at the carriage named `place`.
fn push_at(out: &mut String, place: &str) {
    for (article, prep) in [
        ("Il ", "al "),
        ("La ", "alla "),
        ("Lo ", "allo "),
        ("I ", "ai "),
        ("Le ", "alle "),
        ("Gli ", "agli "),
    ] {
        if let Some(rest) = place.strip_prefix(article) {
            out.push_str(prep);
            out.push_str(rest);
            return;
        }
    }
    if let Some(rest) = place.strip_prefix("L'") {
        out.push_str("all'");
        out.push_str(rest);
        return;
    }
    out.push_str("a ");
    out.push_str(place);
}

fn news_item(news: Option<News>) -> Option<ItemKind> {
    match news {
        Some(News::Shortage(item) | News::Restocked(item)) => Some(item),
        _ => None,
    }
}

/// Fills the placeholders of `template` for `side` speaking.
fn render(template: &str, s: &Script, side: Side) -> String {
    let (me, you) = match side {
        Side::A => (&s.a, &s.b),
        Side::B => (&s.b, &s.a),
    };
    let about = s.about;
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let Some(len) = rest[open..].find('}') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let token = &rest[open + 1..open + len];
        rest = &rest[open + len + 1..];
        if let Some((who, forms)) = token.split_once(':') {
            let (female, male) = forms.split_once('/').unwrap_or((forms, forms));
            let sex = match who {
                "S" => me.sex,
                "L" => you.sex,
                _ => about.map_or(Sex::Male, |a| a.sex),
            };
            out.push_str(sex.pick(female, male));
            continue;
        }
        match token {
            "n" => out.push_str(you.first),
            "s" => out.push_str(me.first),
            "a" => out.push_str(about.map_or("quello", |a| a.first.as_str())),
            "f" => out.push_str(about.map_or("vicini", |a| a.surname.as_str())),
            "o" => out.push_str(about.map_or("l'altra", |a| a.other.as_str())),
            "i" => out.push_str(news_item(s.news).map_or("scorte", ItemKind::plural)),
            "p" => push_at(&mut out, s.place),
            "w" => out.push_str(workplace(me.job)),
            "W" => out.push_str(workplace(you.job)),
            "j" => out.push_str(job_word(me.job, me.sex)),
            "k" => {
                let word = job_word(me.job, me.sex);
                let article = match (word.starts_with(['a', 'e', 'i', 'o', 'u']), me.sex) {
                    (true, _) => "l'",
                    (false, Sex::Female) => "la ",
                    (false, Sex::Male) => "il ",
                };
                out.push_str(article);
                out.push_str(word);
            }
            other => {
                out.push('{');
                out.push_str(other);
                out.push('}');
            }
        }
    }
    out.push_str(rest);
    out
}

// ----------------------------------------------------------------------
// Templates
// ----------------------------------------------------------------------

const SMALL_OPEN_FRIENDLY: &[&str] = &[
    "Ciao {n}, tutto bene?",
    "Ehi {n}! Come va oggi?",
    "{n}! Che piacere vederti.",
    "Ciao {n}! Che si dice?",
    "Oh, {n}! Ti trovo bene.",
    "Ehi {n}, vieni a sederti qui!",
    "{n}, sempre in giro tu, eh?",
    "Ben trovat{L:a/o}, {n}!",
];
const SMALL_OPEN_NEUTRAL: &[&str] = &[
    "Salve.",
    "Ciao {n}.",
    "Tutto a posto?",
    "Anche tu qui, {n}?",
    "Che si dice in giro?",
    "Si va avanti, {n}?",
];
const SMALL_OPEN_TENSE: &[&str] = &[
    "Ancora tu, {n}?",
    "Guarda chi si vede…",
    "Oh. Ciao, {n}.",
    "Che vuoi, {n}?",
    "Sempre tra i piedi, eh?",
];
const SMALL_REPLY_FRIENDLY: &[&str] = &[
    "Si tira avanti, e tu?",
    "Benone! E tu, {n}?",
    "Tutto bene, grazie!",
    "Bene, adesso che ci sei tu.",
    "Non mi lamento, dai.",
    "Stanc{S:a/o}, ma content{S:a/o}.",
];
const SMALL_REPLY_NEUTRAL: &[&str] = &[
    "Mah, si va avanti.",
    "Il solito.",
    "Come sempre, {n}.",
    "Niente di nuovo.",
    "Tutto uguale a ieri.",
];
const SMALL_REPLY_TENSE: &[&str] = &[
    "Cosa vuoi, {n}?",
    "Fatti gli affari tuoi.",
    "Non ho voglia di parlare.",
    "Ti serve qualcosa?",
    "Lasciami in pace, dai.",
];
const SMALL_FOLLOW: &[&str] = &[
    "Fuori si gela, dicono -80.",
    "Stanotte il treno ha sobbalzato.",
    "Hai sentito che vento stamattina?",
    "Oggi {p} c'è un bel via vai.",
    "Il tè della mensa è sempre peggio.",
    "Mi è sembrato di vedere il sole.",
    "Chissà dove siamo adesso.",
    "Le luci hanno tremato di nuovo.",
    "Quanta neve contro i finestrini!",
    "Abbiamo preso un ponte, l'hai sentito?",
];
const REACT_FRIENDLY: &[&str] = &[
    "Eh già!",
    "Hai proprio ragione.",
    "Pensa te!",
    "Me l'ero chiesto anch'io.",
    "Ahah, vero!",
    "Speriamo bene, dai.",
];
const REACT_NEUTRAL: &[&str] = &[
    "Già.",
    "Può darsi.",
    "Sarà.",
    "Mah, chissà.",
    "Se lo dici tu.",
    "Vedremo.",
];
const REACT_TENSE: &[&str] = &[
    "E allora?",
    "Non mi interessa.",
    "Che c'entra?",
    "Sì, sì, certo.",
    "Dici sempre così.",
    "Mah, figurati.",
];
const CLOSE_FRIENDLY: &[&str] = &[
    "Ci vediamo dopo!",
    "A presto, {n}!",
    "Salutami tutti!",
    "Buona giornata, {n}!",
    "Alla prossima!",
    "Un giorno ti offro un tè.",
];
const CLOSE_NEUTRAL: &[&str] = &[
    "Va be', ci vediamo.",
    "Io vado. Ciao.",
    "Ciao, {n}.",
    "Alla prossima.",
    "Devo andare.",
];
const CLOSE_TENSE: &[&str] = &[
    "Lasciamo perdere.",
    "Addio, {n}.",
    "Non ne voglio parlare.",
    "Basta, me ne vado.",
    "Fai come ti pare.",
];

const WORK_OPEN_CONTADINO: &[&str] = &[
    "Oggi in serra non si respira.",
    "I pomodori non vogliono crescere.",
    "Ho le mani piene di terra.",
    "Le lampade della serra scaldano poco.",
    "La verdura viene su bene, sai?",
];
const WORK_OPEN_CUOCO: &[&str] = &[
    "In cucina oggi era un inferno.",
    "Ho pelato verdura tutta la mattina.",
    "La zuppa di oggi l'ho fatta io!",
    "Mancano sempre le pentole grandi.",
];
const WORK_OPEN_OPERAIO: &[&str] = &[
    "In officina si suda, altro che freddo.",
    "Ho saldato lamiere tutto il giorno.",
    "Il rottame non finisce mai.",
    "Mi fischiano ancora le orecchie.",
];
const WORK_OPEN_MERCANTE: &[&str] = &[
    "Al banco oggi c'era la fila.",
    "Tutti vogliono vestiti nuovi.",
    "I prezzi li decidono in testa.",
    "Oggi ho venduto tre attrezzi!",
];
const WORK_OPEN_ASK: &[&str] = &[
    "Com'è andata {W} oggi?",
    "Si lavora tanto {W}?",
    "Come va il lavoro, {n}?",
];
const WORK_REPLY_FRIENDLY: &[&str] = &[
    "Ti capisco, anche da me è dura.",
    "Almeno si lavora in compagnia!",
    "Brav{L:a/o}, sei instancabile.",
    "Dai, che il turno finisce presto.",
];
const WORK_REPLY_NEUTRAL: &[&str] = &[
    "Il lavoro è lavoro.",
    "Da me niente di nuovo.",
    "Si fa quel che si può.",
    "Almeno ci pagano.",
];
const WORK_REPLY_TENSE: &[&str] = &[
    "E io che dovrei dire?",
    "Smettila di lamentarti.",
    "Lavora e non parlare.",
    "Tanto tu non fai mai niente.",
];
const WORK_ASKED_FRIENDLY: &[&str] = &[
    "Tanto, ma mi piace!",
    "Si fatica, ma si va avanti.",
    "Bene, grazie che me lo chiedi.",
];
const WORK_ASKED_NEUTRAL: &[&str] = &[
    "Il solito, niente di speciale.",
    "Come sempre.",
    "Si lavora, che vuoi.",
];
const WORK_ASKED_TENSE: &[&str] = &[
    "Perché, ti interessa?",
    "Meglio che non te lo dica.",
    "Prova tu a lavorare, poi vediamo.",
];
const WORK_FOLLOW: &[&str] = &[
    "Il capoturno non mi dà tregua.",
    "Domani attacco presto, di nuovo.",
    "Con un attrezzo nuovo farei il doppio.",
    "Almeno {w} si sta al caldo.",
    "Mi fa male la schiena.",
    "Aspetto la paga di mezzanotte.",
    "Fare {k} non è uno scherzo.",
];

const NEEDS_OPEN_HUNGER: &[&str] = &[
    "Ho una fame che non ci vedo.",
    "Quando apre la mensa?",
    "Mi brontola lo stomaco…",
    "Darei un gettone per una razione.",
];
const NEEDS_OPEN_TIRED: &[&str] = &[
    "Sono stanc{S:a/o} mort{S:a/o}.",
    "Stanotte non ho chiuso occhio.",
    "Mi reggo in piedi a fatica.",
    "Avrei bisogno di un pisolino.",
];
const NEEDS_OPEN_LONELY: &[&str] = &[
    "Mi mancava fare due chiacchiere.",
    "Non parlo con nessuno da ore.",
    "Che bello vedere una faccia amica.",
];
const NEEDS_REPLY_FRIENDLY: &[&str] = &[
    "Ti capisco, anch'io.",
    "Riposati un po', dai.",
    "Tieni duro, {n}.",
    "Ci sono io, tranquill{L:a/o}.",
];
const NEEDS_REPLY_NEUTRAL: &[&str] = &[
    "Eh, succede.",
    "Siamo tutti così.",
    "Passerà.",
    "Anche a me, sai.",
];
const NEEDS_REPLY_TENSE: &[&str] = &[
    "E cosa vuoi che ci faccia?",
    "Non sei l'unic{L:a/o}.",
    "Lamentati con qualcun altro.",
    "Arrangiati.",
];
const NEEDS_FOLLOW: &[&str] = &[
    "Stasera vado a letto presto.",
    "Speriamo che la cena sia buona.",
    "Questo treno ci sfinisce.",
    "Un po' di tè caldo mi salverebbe.",
];

const FAMILY_OPEN_PARTNER: &[&str] = &[
    "Stasera ceniamo insieme?",
    "Ti ho tenuto il posto in mensa.",
    "Mi sei mancat{L:a/o} oggi.",
    "Hai dormito bene, amore?",
    "Ti ho pensat{L:a/o} tutto il giorno.",
    "Come stai, tesoro?",
];
const FAMILY_OPEN_TO_CHILD: &[&str] = &[
    "Hai mangiato, tesoro?",
    "Non fare tardi stasera.",
    "Com'è andata oggi, {n}?",
    "Copriti bene, fa freddo.",
    "Sono fier{S:a/o} di te, {n}.",
];
const FAMILY_OPEN_TO_PARENT: &[&str] = &[
    "{L:Mamma/Papà}, hai un minuto?",
    "{L:Mamma/Papà}, ho fame!",
    "Sai cosa ho visto oggi?",
    "{L:Mamma/Papà}, mi racconti una storia?",
    "Tutto bene, {L:mamma/papà}?",
];
const FAMILY_OPEN_SIBLING: &[&str] = &[
    "Ti ricordi quando eravamo piccoli?",
    "Hai sentito i nostri?",
    "Ehi, {L:sorellina/fratellino}!",
    "{L:Sorella/Fratello}, come stai?",
];
const FAMILY_OPEN_FRIEND: &[&str] = &[
    "Come sta la tua famiglia?",
    "Salutami i tuoi, {n}.",
    "Tutto bene a casa?",
    "I tuoi come stanno?",
];
const FAMILY_REPLY_FRIENDLY: &[&str] = &[
    "Certo, con piacere!",
    "Tutto bene, grazie di chiedere.",
    "Sei sempre così dolce.",
    "Sì, e tu come stai?",
];
const FAMILY_REPLY_NEUTRAL: &[&str] = &[
    "Sì, sì.",
    "Tutto normale.",
    "Più o meno.",
    "Si tira avanti.",
];
const FAMILY_REPLY_TENSE: &[&str] = &[
    "Non adesso, per favore.",
    "Sempre le stesse domande…",
    "Lasciami stare, oggi.",
    "Ne parliamo dopo.",
];
const FAMILY_FOLLOW: &[&str] = &[
    "Dovremmo stare più insieme.",
    "La famiglia prima di tutto.",
    "Mi fai sempre stare meglio.",
    "Domani mangiamo insieme?",
];

const NEWS_BIRTH: &[&str] = &[
    "È nat{A:a/o} {a}, dei {f}!",
    "{A:Una bimba/Un bimbo} in casa {f}!",
    "I {f} hanno avuto {A:una bimba/un bimbo}!",
];
const NEWS_DEATH: &[&str] = &[
    "Hai saputo? È mort{A:a/o} {a} {f}.",
    "{a} {f} non c'è più…",
    "Ci ha lasciati {a} {f}.",
];
const NEWS_COUPLE: &[&str] = &[
    "{a} e {o} stanno insieme!",
    "Hai visto {a} e {o}? Coppia!",
    "Finalmente {a} e {o}!",
];
const NEWS_WIDOWED: &[&str] = &[
    "Pover{A:a/o} {a}, è rimast{A:a/o} sol{A:a/o}.",
    "{a} è {A:vedova/vedovo}, ora…",
];
const NEWS_THEFT_CAUGHT: &[&str] = &[
    "Hanno beccato {a} a rubare!",
    "{a} {f}? Sorpres{A:a/o} a rubare!",
    "Hai sentito di {a}? {A:Ladra/Ladro}!",
];
const NEWS_THEFT_UNSEEN: &[&str] = &[
    "Qualcuno ha rubato al mercato.",
    "Al mercato è sparita della merce!",
    "C'è un ladro sul treno, sai?",
];
const NEWS_HELP_GIVEN: &[&str] = &[
    "{a} ha prestato gettoni a un amico.",
    "Che cuore d'oro, {a}.",
];
const NEWS_HELP_REFUSED: &[&str] = &[
    "{a} non ha aiutato nessuno, sai?",
    "{a} non dà un gettone a nessuno.",
];
const NEWS_PROTEST_BIRTHS: &[&str] = &[
    "C'è una protesta per le nascite!",
    "Protestano contro il divieto di figli.",
];
const NEWS_PROTEST_FOOD: &[&str] = &[
    "Si protesta per le razioni!",
    "Vogliono protestare per il cibo.",
];
const NEWS_CONCESSION_BIRTHS: &[&str] = &[
    "Hanno ceduto: più nascite!",
    "Le proteste hanno funzionato!",
];
const NEWS_CONCESSION_FOOD: &[&str] = &[
    "Razioni d'emergenza, finalmente!",
    "L'amministrazione ha ceduto!",
];
const NEWS_SHORTAGE_FOOD: &[&str] = &[
    "Le mense sono senza razioni!",
    "Non c'è più niente da mangiare!",
];
const NEWS_SHORTAGE: &[&str] = &["Al mercato non ci sono più {i}!", "Finiti i {i}, pensa te."];
const NEWS_RESTOCKED: &[&str] = &["Sono tornati i {i}!", "Di nuovo {i}, finalmente!"];
const NEWS_BIRTH_DENIED: &[&str] = &[
    "Ai {f} hanno negato un figlio.",
    "Niente bambino per i {f}…",
];
const NEWS_CAME_OF_AGE: &[&str] = &[
    "{a} è {A:diventata/diventato} grande!",
    "{a} ha compiuto diciott'anni!",
];
const NEWS_RETIRED: &[&str] = &[
    "{a} è {A:andata/andato} in pensione.",
    "{a} ha smesso di lavorare, beat{A:a/o}!",
];
const NEWS_AUSTERITY: &[&str] = &["Paghe tagliate, hai visto?", "La tesoreria è a secco!"];
const NEWS_PAY_RAISED: &[&str] = &["Hanno alzato le paghe!", "Più gettoni per tutti, pare."];
const NEWS_PAY_CUT: &[&str] = &["Hanno abbassato le paghe…", "Paghe più basse, di nuovo."];
const NEWS_VAGUE: &[&str] = &[
    "Hai sentito le ultime?",
    "Ne succedono di cose, eh?",
    "Sai cos'è successo {p}?",
];

const GOSSIP_THEFT: &[&str] = &[
    "Sai di {a}? L'hanno beccat{A:a/o}!",
    "{a} che ruba… chi l'avrebbe detto!",
    "Ma lo sai che {a} ruba?",
];
const GOSSIP_COUPLE: &[&str] = &[
    "Hai visto {a} con {o}? Eh eh!",
    "Tra {a} e {o} c'è del tenero!",
    "Pare che {a} si sia sistemat{A:a/o}.",
];
const GOSSIP_BIRTH: &[&str] = &[
    "{A:La piccola/Il piccolo} dei {f} è bellissim{A:a/o}!",
    "Hai visto {a}, la nuova dei {f}?",
];
const GOSSIP_WIDOWED: &[&str] = &[
    "{a} non esce più di casa…",
    "Hai visto {a}? È distrutt{A:a/o}.",
];
const GOSSIP_STINGY: &[&str] = &[
    "{a} è tirchi{A:a/o}, te lo dico io.",
    "{a} non aiuta mai nessuno.",
];
const GOSSIP_GENEROUS: &[&str] = &["{a} ha un cuore d'oro, sai?", "{a} aiuta sempre tutti."];
const GOSSIP_GROWN: &[&str] = &[
    "{a} è cresciut{A:a/o} tanto!",
    "Hai visto quant'è grande {a}?",
];
const GOSSIP_OPEN_FOND: &[&str] = &[
    "{a} è proprio una brava persona.",
    "Hai visto {a} ultimamente?",
    "{a} mi fa sempre ridere.",
    "Sai che {a} lavora benissimo?",
];
const GOSSIP_OPEN_SPITE: &[&str] = &[
    "{a}? Non mi fido per niente.",
    "{a} si dà un sacco di arie.",
    "Hai visto come ci guarda {a}?",
    "{a} non fa che lamentarsi.",
];
const FOND_REPLY_FRIENDLY: &[&str] = &[
    "Sì, è proprio in gamba.",
    "Hai ragione, {A:le/gli} voglio bene.",
    "Me l'hanno detto anche altri.",
    "Una persona d'oro, davvero.",
];
const FOND_REPLY_NEUTRAL: &[&str] = &["Può darsi.", "Non {A:la/lo} conosco bene.", "Sarà, sarà."];
const FOND_REPLY_TENSE: &[&str] = &[
    "Tu vedi del buono in tutti.",
    "Mah, a me non sembra.",
    "Parli sempre bene di tutti, tu.",
];
const SPITE_REPLY_FRIENDLY: &[&str] = &[
    "Eh, l'ho notato anch'io!",
    "Non dirlo a me…",
    "Hai ragione, stiamone alla larga.",
];
const SPITE_REPLY_NEUTRAL: &[&str] = &[
    "Mah, non saprei.",
    "Ognuno è fatto a modo suo.",
    "Può essere.",
];
const SPITE_REPLY_TENSE: &[&str] = &[
    "Parli male di tutti, tu.",
    "Guarda che è mi{A:a/o} amic{A:a/o}.",
    "Pensa a te, piuttosto.",
];
const GOSSIP_FOLLOW: &[&str] = &[
    "Ma non dirlo in giro, eh.",
    "E non è tutto…",
    "Me l'ha detto una vicina di cuccetta.",
    "Resti tra noi, mi raccomando.",
    "Io l'ho sempre detto!",
];

const GOOD_REPLY_FRIENDLY: &[&str] = &[
    "Che bella notizia!",
    "Meraviglioso, davvero!",
    "Finalmente una gioia!",
    "Sono proprio content{S:a/o}!",
];
const GOOD_REPLY_NEUTRAL: &[&str] = &[
    "Ah, bene.",
    "Buon per loro.",
    "Non lo sapevo.",
    "Meglio così.",
];
const GOOD_REPLY_TENSE: &[&str] = &[
    "E a me che importa?",
    "Contenti loro…",
    "Sai che novità.",
    "Buon per loro, io no.",
];
const BAD_REPLY_FRIENDLY: &[&str] = &[
    "Che tristezza…",
    "Mi dispiace tanto.",
    "Povera gente, davvero.",
    "Che brutta notizia.",
];
const BAD_REPLY_NEUTRAL: &[&str] = &[
    "Eh, capita.",
    "Brutta storia.",
    "Speriamo passi presto.",
    "Già, l'ho sentito.",
];
const BAD_REPLY_TENSE: &[&str] = &[
    "Colpa loro, dico io.",
    "Cosa ci posso fare io?",
    "Sempre notizie brutte, tu.",
    "Non me ne parlare.",
];
const SCANDAL_REPLY_FRIENDLY: &[&str] = &[
    "Non ci posso credere!",
    "Ma dai! Davvero?",
    "Chi l'avrebbe mai detto!",
    "Che vergogna, poveretti.",
];
const SCANDAL_REPLY_NEUTRAL: &[&str] = &[
    "Mah, succede.",
    "Non mi stupisce.",
    "Ognuno fa quel che può.",
    "Bah.",
];
const SCANDAL_REPLY_TENSE: &[&str] = &[
    "Sei sicur{L:a/o}? Non ti credo.",
    "Chiacchiere, solo chiacchiere.",
    "Pensa agli affari tuoi.",
    "E tu come lo sai?",
];
const GOOD_FOLLOW: &[&str] = &[
    "Ci voleva, dopo tanto grigio.",
    "Bisogna festeggiare!",
    "Almeno una cosa va bene.",
    "Lo dicono tutti {p}.",
];
const BAD_FOLLOW: &[&str] = &[
    "Non so dove andremo a finire.",
    "Speriamo non tocchi a noi.",
    "Ne parlano tutti {p}.",
    "Il treno non perdona.",
];
const SCANDAL_FOLLOW: &[&str] = &[
    "Me l'ha detto uno che c'era.",
    "Lo sanno già tutti {p}.",
    "Io l'avevo sempre detto.",
    "Che tempi, che tempi.",
];

const COMPLAINT_OPEN_FOOD: &[&str] = &[
    "Razioni sempre più piccole…",
    "Mensa vuota, di nuovo!",
    "Con questa fame chi lavora?",
];
const COMPLAINT_OPEN_BIRTH: &[&str] = &[
    "Ci hanno negato un figlio…",
    "Chi sono loro per dirci di no?",
    "Vogliono decidere pure i figli!",
];
const COMPLAINT_OPEN_AUSTERITY: &[&str] =
    &["Paghe a metà, e noi zitti.", "La tesoreria piange, dicono."];
const COMPLAINT_OPEN_PAY: &[&str] = &[
    "Paghe giù e prezzi su, bello.",
    "Con questi gettoni non si vive.",
];
const COMPLAINT_OPEN: &[&str] = &[
    "In questo treno non si dorme mai.",
    "Fa un freddo cane in cuccetta.",
    "Qui nessuno ascolta nessuno.",
    "Il tè sa di ferro, di nuovo.",
    "Si lavora e basta, che vita.",
    "Rumore tutta la notte, sempre.",
];
const COMPLAINT_REPLY_FRIENDLY: &[&str] = &[
    "Hai ragione, non è giusto.",
    "Lo dico anch'io da sempre!",
    "Tieni duro, cambierà.",
    "Siamo in tanti a pensarla così.",
];
const COMPLAINT_REPLY_NEUTRAL: &[&str] = &[
    "Eh, è così.",
    "Che ci vuoi fare.",
    "Sempre la stessa storia.",
    "Lamentarsi non serve.",
];
const COMPLAINT_REPLY_TENSE: &[&str] = &[
    "Smettila di lamentarti!",
    "Sempre a brontolare, tu.",
    "Lavora e stai zitt{L:a/o}.",
    "Ringrazia che mangi.",
];
const COMPLAINT_FOLLOW: &[&str] = &[
    "Qualcuno dovrà pur dirlo.",
    "Prima o poi protesto, giuro.",
    "Quelli di testa mangiano bene.",
    "Una volta non era così.",
];

const GRUMPY: &[&str] = &[
    "Mh.",
    "E allora?",
    "Se lo dici tu.",
    "Non ho tempo.",
    "Bah.",
    "Tsk.",
    "Sì, sì…",
];
const GRUMPY_CLOSE: &[&str] = &["Ciao.", "Vado.", "Basta così."];
const SHY: &[&str] = &[
    "…",
    "Ehm… sì.",
    "Mh-mh.",
    "Oh… davvero?",
    "S-sì, certo.",
    "…già.",
];
const SHY_FOLLOW: &[&str] = &["…", "Ehm… niente.", "Scusa, parlo poco."];
const SHY_CLOSE: &[&str] = &["C-ciao…", "…a dopo.", "Beh… ciao.", "Scusa, devo andare…"];
const CHEERFUL: &[&str] = &[
    "Ahah, che bello!",
    "Dai, che ce la facciamo!",
    "Tu sì che mi fai ridere!",
    "Evviva!",
    "Mi hai messo di buonumore, {n}!",
];
const LAMENT: &[&str] = &[
    "Sempre peggio, sempre peggio…",
    "A me va sempre tutto storto.",
    "Mi fa male tutto, oggi.",
    "Non se ne può più.",
    "E nessuno fa niente.",
];
const CURIOUS: &[&str] = &[
    "E poi? Racconta!",
    "Davvero? Chi te l'ha detto?",
    "E tu cosa ne pensi?",
    "Come mai, secondo te?",
    "Aspetta, spiegami meglio.",
];
const KIND: &[&str] = &[
    "Ti capisco, davvero.",
    "Se ti serve, ci sono.",
    "Non preoccuparti, {n}.",
    "Sei un tesoro, {n}.",
];
const GOSSIPY: &[&str] = &[
    "Ma non dirlo in giro, eh.",
    "E non è tutto!",
    "Resti tra noi, mi raccomando.",
    "Ne ho sentite di belle…",
];
const CHATTY: &[&str] = &[
    "Ieri ho sognato il mare, pensa.",
    "Ho trovato un bottone d'oro, sai?",
    "Parlo troppo, eh? Scusa.",
    "Mia nonna diceva sempre così.",
    "E il tè? Ne parliamo del tè?",
];
const BABBLE: &[&str] = &["Gu-gu!", "Ah-ah!", "Bla!", "Pappa!", "Mmmh!", "Eeeh!"];
const KID: &[&str] = &[
    "Posso andare a giocare?",
    "Uffa, che noia!",
    "Guarda cosa so fare!",
    "Ho fame!",
    "Mi racconti una storia?",
    "Perché il treno non si ferma?",
];
const TO_BABY: &[&str] = &[
    "Ma che bell{L:a/o} che sei!",
    "Chi è {L:la piccola/il piccolo}?",
    "Cucù, {n}!",
    "Fai un sorriso, {n}!",
    "Guarda che manine!",
    "Sì, sì, hai ragione tu!",
    "Ma quanto sei cresciut{L:a/o}!",
];

/// Every template pool, for [`template_count`] and the tests.
const ALL_POOLS: &[&[&str]] = &[
    SMALL_OPEN_FRIENDLY,
    SMALL_OPEN_NEUTRAL,
    SMALL_OPEN_TENSE,
    SMALL_REPLY_FRIENDLY,
    SMALL_REPLY_NEUTRAL,
    SMALL_REPLY_TENSE,
    SMALL_FOLLOW,
    REACT_FRIENDLY,
    REACT_NEUTRAL,
    REACT_TENSE,
    CLOSE_FRIENDLY,
    CLOSE_NEUTRAL,
    CLOSE_TENSE,
    WORK_OPEN_CONTADINO,
    WORK_OPEN_CUOCO,
    WORK_OPEN_OPERAIO,
    WORK_OPEN_MERCANTE,
    WORK_OPEN_ASK,
    WORK_REPLY_FRIENDLY,
    WORK_REPLY_NEUTRAL,
    WORK_REPLY_TENSE,
    WORK_ASKED_FRIENDLY,
    WORK_ASKED_NEUTRAL,
    WORK_ASKED_TENSE,
    WORK_FOLLOW,
    NEEDS_OPEN_HUNGER,
    NEEDS_OPEN_TIRED,
    NEEDS_OPEN_LONELY,
    NEEDS_REPLY_FRIENDLY,
    NEEDS_REPLY_NEUTRAL,
    NEEDS_REPLY_TENSE,
    NEEDS_FOLLOW,
    FAMILY_OPEN_PARTNER,
    FAMILY_OPEN_TO_CHILD,
    FAMILY_OPEN_TO_PARENT,
    FAMILY_OPEN_SIBLING,
    FAMILY_OPEN_FRIEND,
    FAMILY_REPLY_FRIENDLY,
    FAMILY_REPLY_NEUTRAL,
    FAMILY_REPLY_TENSE,
    FAMILY_FOLLOW,
    NEWS_BIRTH,
    NEWS_DEATH,
    NEWS_COUPLE,
    NEWS_WIDOWED,
    NEWS_THEFT_CAUGHT,
    NEWS_THEFT_UNSEEN,
    NEWS_HELP_GIVEN,
    NEWS_HELP_REFUSED,
    NEWS_PROTEST_BIRTHS,
    NEWS_PROTEST_FOOD,
    NEWS_CONCESSION_BIRTHS,
    NEWS_CONCESSION_FOOD,
    NEWS_SHORTAGE_FOOD,
    NEWS_SHORTAGE,
    NEWS_RESTOCKED,
    NEWS_BIRTH_DENIED,
    NEWS_CAME_OF_AGE,
    NEWS_RETIRED,
    NEWS_AUSTERITY,
    NEWS_PAY_RAISED,
    NEWS_PAY_CUT,
    NEWS_VAGUE,
    GOSSIP_THEFT,
    GOSSIP_COUPLE,
    GOSSIP_BIRTH,
    GOSSIP_WIDOWED,
    GOSSIP_STINGY,
    GOSSIP_GENEROUS,
    GOSSIP_GROWN,
    GOSSIP_OPEN_FOND,
    GOSSIP_OPEN_SPITE,
    GOSSIP_FOLLOW,
    FOND_REPLY_FRIENDLY,
    FOND_REPLY_NEUTRAL,
    FOND_REPLY_TENSE,
    SPITE_REPLY_FRIENDLY,
    SPITE_REPLY_NEUTRAL,
    SPITE_REPLY_TENSE,
    GOOD_REPLY_FRIENDLY,
    GOOD_REPLY_NEUTRAL,
    GOOD_REPLY_TENSE,
    BAD_REPLY_FRIENDLY,
    BAD_REPLY_NEUTRAL,
    BAD_REPLY_TENSE,
    SCANDAL_REPLY_FRIENDLY,
    SCANDAL_REPLY_NEUTRAL,
    SCANDAL_REPLY_TENSE,
    GOOD_FOLLOW,
    BAD_FOLLOW,
    SCANDAL_FOLLOW,
    COMPLAINT_OPEN_FOOD,
    COMPLAINT_OPEN_BIRTH,
    COMPLAINT_OPEN_AUSTERITY,
    COMPLAINT_OPEN_PAY,
    COMPLAINT_OPEN,
    COMPLAINT_REPLY_FRIENDLY,
    COMPLAINT_REPLY_NEUTRAL,
    COMPLAINT_REPLY_TENSE,
    COMPLAINT_FOLLOW,
    GRUMPY,
    GRUMPY_CLOSE,
    SHY,
    SHY_FOLLOW,
    SHY_CLOSE,
    CHEERFUL,
    LAMENT,
    CURIOUS,
    KIND,
    GOSSIPY,
    CHATTY,
    BABBLE,
    KID,
    TO_BABY,
];

/// Number of distinct line templates.
pub fn template_count() -> usize {
    let mut all: Vec<&str> = ALL_POOLS.iter().flat_map(|p| p.iter().copied()).collect();
    all.sort_unstable();
    all.dedup();
    all.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voice(id: u32, first: &'static str, sex: Sex, job: Option<Job>) -> Voice<'static> {
        Voice {
            id: NpcId(id),
            first,
            sex,
            age: 35,
            job,
            personality: Personality::default(),
        }
    }

    fn subject(sex: Sex) -> Subject {
        Subject {
            first: sex.pick("Marta", "Luca").to_string(),
            surname: "Rossi".to_string(),
            sex,
            other: sex.pick("Gino", "Anna").to_string(),
        }
    }

    const NEWS: [News; 22] = [
        News::Birth,
        News::Death,
        News::Couple,
        News::Widowed,
        News::TheftCaught,
        News::TheftUnseen,
        News::HelpGiven,
        News::HelpRefused,
        News::Protest(Grievance::BirthDenied),
        News::Protest(Grievance::FoodShortage),
        News::Concession(Grievance::BirthDenied),
        News::Concession(Grievance::FoodShortage),
        News::Shortage(ItemKind::Razione),
        News::Shortage(ItemKind::Vestito),
        News::Restocked(ItemKind::Attrezzo),
        News::BirthDenied,
        News::CameOfAge,
        News::Retired,
        News::Austerity,
        News::PayRaised,
        News::PayCut,
        News::Restocked(ItemKind::Razione),
    ];

    #[test]
    fn every_template_fits_a_bubble_with_ordinary_names() {
        assert!(template_count() >= 150, "{} templates", template_count());
        for sex in Sex::ALL {
            let about = subject(sex.opposite());
            for pool in ALL_POOLS {
                for template in pool.iter() {
                    for job in [None, Some(Job::Operaio)] {
                        let s = Script {
                            topic: Topic::News,
                            tone: Tone::Neutral,
                            news: Some(News::Shortage(ItemKind::Attrezzo)),
                            a: voice(1, "Marta", sex, job),
                            b: voice(2, "Luca", sex.opposite(), job),
                            tie: None,
                            about: Some(&about),
                            fond: true,
                            need: Need::Hunger,
                            place: "Il Refettorio",
                        };
                        for side in [Side::A, Side::B] {
                            let text = render(template, &s, side);
                            assert!(
                                text.chars().count() <= MAX_LINE_CHARS,
                                "{text:?} ({} chars)",
                                text.chars().count()
                            );
                            assert!(!text.contains('{') && !text.contains('}'), "{text}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn every_topic_and_tone_writes_lines_for_both_sexes() {
        let long = Subject {
            first: "Margherita".to_string(),
            surname: "De Santis".to_string(),
            sex: Sex::Female,
            other: "Massimiliano".to_string(),
        };
        let mut seed = 0;
        for topic in Topic::ALL {
            for tone in Tone::ALL {
                for sex in Sex::ALL {
                    for news in NEWS.iter().map(|&n| Some(n)).chain([None]) {
                        for tie in [
                            None,
                            Some(RelationKind::Partner),
                            Some(RelationKind::Parent),
                        ] {
                            for about in [None, Some(&long)] {
                                seed += 1;
                                let s = Script {
                                    topic,
                                    tone,
                                    news,
                                    a: voice(1, "Alessandro", sex, Some(Job::Mercante)),
                                    b: voice(2, "Margherita", sex.opposite(), None),
                                    tie,
                                    about,
                                    fond: seed % 2 == 0,
                                    need: Need::Tiredness,
                                    place: "Giardino d'Inverno",
                                };
                                let (since, until) = (GameTime(1000), GameTime(1000 + seed % 50));
                                let lines = write(&s, seed, since, until);
                                assert!((3..=6).contains(&lines.len()));
                                for (k, l) in lines.iter().enumerate() {
                                    assert!(!l.text.is_empty());
                                    assert!(l.text.chars().count() <= MAX_LINE_CHARS, "{}", l.text);
                                    assert!(!l.text.contains('{'), "{}", l.text);
                                    assert!(l.at >= since && l.at <= until.max(since + 1));
                                    let speaker = if k % 2 == 0 { NpcId(1) } else { NpcId(2) };
                                    assert_eq!(l.speaker, speaker);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn gender_and_personality_shape_the_lines() {
        let s = |sex| Script {
            topic: Topic::Needs,
            tone: Tone::Friendly,
            news: None,
            a: voice(1, "Marta", sex, None),
            b: voice(2, "Luca", Sex::Male, None),
            tie: None,
            about: None,
            fond: true,
            need: Need::Tiredness,
            place: "Nido",
        };
        let t = "Sono stanc{S:a/o} mort{S:a/o}, {n}.";
        assert_eq!(
            render(t, &s(Sex::Female), Side::A),
            "Sono stanca morta, Luca."
        );
        assert_eq!(
            render(t, &s(Sex::Male), Side::A),
            "Sono stanco morto, Luca."
        );
        let mut worker = s(Sex::Female);
        worker.a.job = Some(Job::Operaio);
        assert_eq!(render("Fare {k}.", &worker, Side::A), "Fare l'operaia.");
        worker.a.job = Some(Job::Cuoco);
        assert_eq!(render("Fare {k}.", &worker, Side::A), "Fare la cuoca.");
        // Two shy people: some lines are just "…".
        let mut shy = s(Sex::Female);
        shy.a.personality = Personality::default().with(Temper::Timido);
        shy.b.personality = shy.a.personality;
        let silent = (0..200)
            .flat_map(|seed| write(&shy, seed, GameTime(0), GameTime(48)))
            .filter(|l| SHY.contains(&l.text.as_str()))
            .count();
        assert!(silent > 20, "{silent}");
        // Babies babble.
        let mut baby = s(Sex::Male);
        baby.b.age = 1;
        let lines = write(&baby, 3, GameTime(0), GameTime(30));
        assert!(BABBLE.contains(&lines[1].text.as_str()));
    }
}
