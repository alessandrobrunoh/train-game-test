//! Testo delle conversazioni: battute a modelli in italiano.
//!
//! Every conversation follows one *thread* (a subject with a mood) as a
//! small script of 3–6 lines alternating speakers, `a` first:
//!
//! 1. optional greeting and greeting back (by family tie, tone, time of
//!    day and personality) — not for news, gossip and small talk, which
//!    open straight away, nor for quarrels;
//! 2. the *statement* by `a`: the thread's opener, with the news, whom it
//!    is about, the job, the need, the family tie;
//! 3. the *reply* by `b`, consistent with the thread's [`Mood`]: pleased
//!    about good news, sympathetic about bad news, shocked or judging at a
//!    scandal, agreeing with a complaint (or pushing back if the tone is
//!    tense);
//! 4. optional *follow-up* by `a` on the same subject (it answers `b`'s
//!    question, insists in a quarrel, or elaborates), and `b`'s *reaction*;
//! 5. a closing line (and, in the longest ones, a goodbye back).
//!
//! Personality ([`Temper`]) changes the wording, never the subject: the
//! grumpy answer curtly, the shy stammer, the curious ask (and get an
//! answer), gossips ask to keep the secret, complainers see the dark side of
//! the same news, the kind comfort, the cheerful cheer. Small children
//! babble or talk like kids.
//!
//! Templates use placeholders, filled in by [`render`]:
//! - `{n}` the listener's first name, `{s}` the speaker's;
//! - `{a}` / `{f}` first name / surname of whom the talk is about, `{o}`
//!   their partner (couples); `{fam}` / `{dei}` / `{ai}` their family ("i
//!   Rossi", "degli Esposito", "ai Rossi");
//! - `{i}` the item of the news (plural), `{li}` with its article ("le
//!   razioni", "gli attrezzi"), `{I:x/y}` the form agreeing with it;
//! - `{al}` / `{nel}` / `{del}` the carriage they are in, with the
//!   preposition ("all'Alveare", "nella Brace", "del Refettorio");
//! - `{w}` / `{W}` where the speaker / listener works ("in serra"), `{j}`
//!   the speaker's job, `{k}` with its article ("l'operaia");
//! - `{g}` "Buongiorno"/"Buonasera", `{bg}` "Buona giornata"/"Buona
//!   serata"/"Buonanotte", by the hour;
//! - `{S:x/y}`, `{L:x/y}`, `{A:x/y}`: the female / male form for the
//!   speaker, the listener, the subject ("stanc{S:a/o}").
//!
//! Words joined to a placeholder follow [`grammar`]: "Bruno ed Elena", "ad
//! Anna"; the first letter is capitalized. A template whose data is missing
//! (no subject) or whose text would exceed [`MAX_LINE_CHARS`] (long names)
//! is skipped for the next. Variants are picked with a seeded RNG of its own
//! (deterministic, and it never touches the world's RNG).

use super::grammar::{self, Prep};
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
    /// Hour of the day (0–23) when they start talking.
    pub hour: u32,
}

type Pool = &'static [&'static str];

/// How the subject of a thread feels: it chooses the answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mood {
    /// Good news, praise, a job well done.
    Good,
    /// Somebody else's misfortune.
    Bad,
    /// Theft, stinginess, spite.
    Scandal,
    /// The speaker's own trouble: hunger, tiredness, a grievance, bad work.
    Gripe,
    /// Greetings, family, small talk.
    Neutral,
}

/// One subject for the whole conversation: the opener, the answers by
/// tone (friendly, neutral, tense), the follow-up on the same subject and,
/// if the generic ones by mood don't fit, the reactions to it.
#[derive(Clone, Copy, Debug)]
struct Thread {
    mood: Mood,
    open: Pool,
    reply: [Pool; 3],
    follow: Pool,
    react: Option<[Pool; 3]>,
    /// What `a` says when `b` disagrees, if the generic one by mood won't do.
    insist: Option<Pool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    A,
    B,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Beat {
    Greet,
    GreetBack,
    Statement,
    Reply,
    Follow,
    React,
    Close,
    CloseBack,
}

/// What kind of line the follow-up was: the reaction answers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FollowKind {
    Plain,
    /// Answers the listener's question.
    Answer,
    /// Keeps arguing (tense).
    Insist,
    /// A complainer's dark side of it.
    Lament,
    /// "Resti tra noi."
    Secret,
    Shy,
    Kid,
}

/// A tiny deterministic generator (SplitMix64) for picking variants: much
/// cheaper to seed than the world's ChaCha, and text needs no more.
pub(super) struct Mix(pub(super) u64);

impl Mix {
    pub(super) fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (n > 0).
    pub(super) fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    /// True with probability `p`.
    pub(super) fn chance(&mut self, p: f64) -> bool {
        ((self.next() >> 11) as f64 / (1u64 << 53) as f64) < p
    }

    pub(super) fn one_of<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len())]
    }
}

/// Writes the lines of a conversation from `since` to `until`.
pub(crate) fn write(s: &Script, seed: u64, since: GameTime, until: GameTime) -> Vec<Line> {
    compose(s, seed, since, until)
        .into_iter()
        .map(|(_, _, line)| line)
        .collect()
}

/// The script's beats for `n` lines.
fn layout(n: usize, greet: bool) -> &'static [Beat] {
    use Beat::*;
    match (n, greet) {
        (0..=3, _) => &[Statement, Reply, Close],
        (4, _) => &[Statement, Reply, Follow, Close],
        (5, true) => &[Greet, GreetBack, Statement, Reply, Close],
        (5, false) => &[Statement, Reply, Follow, React, Close],
        (_, true) => &[Greet, GreetBack, Statement, Reply, Follow, Close],
        (_, false) => &[Statement, Reply, Follow, React, Close, CloseBack],
    }
}

/// Whether the conversation may start with a greeting: not when the news
/// can't wait, nor in a quarrel, nor with a baby.
fn greetable(s: &Script) -> bool {
    !matches!(s.topic, Topic::SmallTalk | Topic::News | Topic::Gossip)
        && s.tone != Tone::Tense
        && s.a.age.min(s.b.age) >= BABY_AGE
}

/// The lines with their beat and template.
fn compose(
    s: &Script,
    seed: u64,
    since: GameTime,
    until: GameTime,
) -> Vec<(Beat, &'static str, Line)> {
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
    let thread = thread(s, &mut rng);
    let greet = n >= 5 && greetable(s) && rng.chance(0.5);
    let beats = layout(n, greet);

    let mut asked = false;
    let mut kind = FollowKind::Plain;
    let mut out: Vec<(Beat, &'static str, Line)> = Vec::with_capacity(n);
    for (k, &beat) in beats.iter().enumerate() {
        let side = if k % 2 == 0 { Side::A } else { Side::B };
        let can_ask = beats.get(k + 1) == Some(&Beat::Follow);
        let pool = match beat {
            Beat::Greet => greet_pool(s, side, &mut rng),
            Beat::GreetBack => greet_back_pool(s, side, &mut rng),
            Beat::Statement => statement_pool(s, side, &thread),
            Beat::Reply => {
                let (pool, q) = reply_pool(s, side, &thread, can_ask, &mut rng);
                asked = q;
                pool
            }
            Beat::Follow => {
                let (pool, f) = follow_pool(s, side, &thread, asked, &mut rng);
                kind = f;
                pool
            }
            Beat::React => react_pool(s, side, &thread, kind, &mut rng),
            Beat::Close => close_pool(s, side, &thread, &mut rng),
            Beat::CloseBack => close_back_pool(s, side, &mut rng),
        };
        // Avoid saying the same thing twice.
        let said: Vec<&str> = out.iter().map(|(_, _, l)| l.text.as_str()).collect();
        let (template, text) = pick(pool, s, side, &said, &mut rng)
            .unwrap_or_else(|| (fallback(beat), fallback(beat).to_string()));
        let speaker = match side {
            Side::A => s.a.id,
            Side::B => s.b.id,
        };
        let at = since + minutes * k as u64 / n as u64;
        out.push((beat, template, Line { speaker, at, text }));
    }
    out
}

fn fallback(beat: Beat) -> &'static str {
    match beat {
        Beat::Greet => "Ciao!",
        Beat::GreetBack => "Ciao.",
        Beat::Statement => "Ehi!",
        Beat::Reply => "Già.",
        Beat::Follow => "Eh…",
        Beat::React => "Mh.",
        Beat::Close | Beat::CloseBack => "Ciao.",
    }
}

fn speakers<'s, 'a>(s: &'s Script<'a>, side: Side) -> (&'s Voice<'a>, &'s Voice<'a>) {
    match side {
        Side::A => (&s.a, &s.b),
        Side::B => (&s.b, &s.a),
    }
}

/// What the listener is to the speaker, if family.
fn tie_of(s: &Script, side: Side) -> Option<RelationKind> {
    match side {
        Side::A => s.tie,
        Side::B => s.tie.map(RelationKind::inverse),
    }
}

fn by_tone(s: &Script, pools: [Pool; 3]) -> Pool {
    pools[match s.tone {
        Tone::Friendly => 0,
        Tone::Neutral => 1,
        Tone::Tense => 2,
    }]
}

/// Babies babble, and everybody talks to a baby like to a baby.
fn baby_pool(s: &Script, side: Side, goodbye: bool) -> Option<Pool> {
    let (me, you) = speakers(s, side);
    if me.age < BABY_AGE {
        Some(BABBLE)
    } else if you.age < BABY_AGE && !goodbye {
        Some(TO_BABY)
    } else {
        None
    }
}

// ----------------------------------------------------------------------
// Threads
// ----------------------------------------------------------------------

/// The subject of the conversation.
fn thread(s: &Script, rng: &mut Mix) -> Thread {
    match s.topic {
        Topic::SmallTalk => small_talk(s),
        Topic::Work => match s.a.job {
            None => WORK_ASK,
            Some(job) => {
                let p = s.a.personality;
                let good = if p.has(Temper::Lamentoso) || p.has(Temper::Burbero) {
                    0.25
                } else if p.has(Temper::Allegro) {
                    0.7
                } else {
                    0.5
                };
                work(job, rng.chance(good))
            }
        },
        Topic::Needs => match s.need {
            Need::Hunger => NEEDS_HUNGER,
            Need::Tiredness => NEEDS_TIRED,
            Need::Loneliness => NEEDS_LONELY,
        },
        Topic::Family => {
            let kid = s.a.age < KID_AGE;
            let moves: &[Thread] = match s.tie {
                Some(RelationKind::Partner) => &[PARTNER_INVITE, PARTNER_ASK, PARTNER_LOVE],
                Some(RelationKind::Child) => &[TO_CHILD_ASK, TO_CHILD_CARE],
                Some(RelationKind::Parent) if kid => &[KID_REQUEST, KID_TELL],
                Some(RelationKind::Parent) => &[TO_PARENT_ASK],
                Some(RelationKind::Sibling) => &[SIBLING_MEMORY, SIBLING_ASK, SIBLING_FOLKS],
                Some(RelationKind::Friend) | None => &[FRIEND_FOLKS],
            };
            rng.one_of(moves)
        }
        Topic::News => match s.news {
            Some(news) => news_thread(news, s.about.is_some(), false),
            None => small_talk(s),
        },
        Topic::Gossip => match (s.news, s.about) {
            (_, None) => small_talk(s),
            (Some(news), Some(_)) => news_thread(news, true, true),
            (None, Some(_)) if s.fond => GOSSIP_FOND,
            (None, Some(_)) => GOSSIP_SPITE,
        },
        Topic::Complaint => match s.news {
            Some(News::Shortage(ItemKind::Razione)) => COMPLAINT_FOOD,
            Some(News::BirthDenied) => COMPLAINT_BIRTH,
            Some(News::Austerity | News::PayCut) => COMPLAINT_MONEY,
            _ => rng.one_of(&[
                COMPLAINT_SLEEP,
                COMPLAINT_COLD,
                COMPLAINT_TEA,
                COMPLAINT_LIFE,
            ]),
        },
    }
}

fn small_talk(s: &Script) -> Thread {
    Thread {
        mood: Mood::Neutral,
        open: by_tone(
            s,
            [SMALL_OPEN_FRIENDLY, SMALL_OPEN_NEUTRAL, SMALL_OPEN_TENSE],
        ),
        reply: [SMALL_REPLY_FRIENDLY, SMALL_REPLY_NEUTRAL, SMALL_REPLY_TENSE],
        follow: SMALL_REMARK,
        react: Some([REMARK_REACT_FRIENDLY, REACT_NEUTRAL_NEUTRAL, REACT_TENSE]),
        insist: None,
    }
}

fn work(job: Job, good: bool) -> Thread {
    let open = match (job, good) {
        (Job::Contadino, true) => WORK_GOOD_CONTADINO,
        (Job::Contadino, false) => WORK_BAD_CONTADINO,
        (Job::Cuoco, true) => WORK_GOOD_CUOCO,
        (Job::Cuoco, false) => WORK_BAD_CUOCO,
        (Job::Operaio, true) => WORK_GOOD_OPERAIO,
        (Job::Operaio, false) => WORK_BAD_OPERAIO,
        (Job::Mercante, true) => WORK_GOOD_MERCANTE,
        (Job::Mercante, false) => WORK_BAD_MERCANTE,
    };
    if good {
        Thread {
            mood: Mood::Good,
            open,
            reply: [
                WORK_GOOD_REPLY_FRIENDLY,
                WORK_GOOD_REPLY_NEUTRAL,
                WORK_GOOD_REPLY_TENSE,
            ],
            follow: WORK_GOOD_FOLLOW,
            react: None,
            insist: None,
        }
    } else {
        Thread {
            mood: Mood::Gripe,
            open,
            reply: [
                WORK_BAD_REPLY_FRIENDLY,
                WORK_BAD_REPLY_NEUTRAL,
                WORK_BAD_REPLY_TENSE,
            ],
            follow: WORK_BAD_FOLLOW,
            react: None,
            insist: None,
        }
    }
}

fn mood_of(valence: Valence) -> Mood {
    match valence {
        Valence::Good => Mood::Good,
        Valence::Bad => Mood::Bad,
        Valence::Scandal => Mood::Scandal,
    }
}

/// A piece of news told (or gossiped about, `gossip`): `about` if we know
/// whom it is about.
fn news_thread(news: News, about: bool, gossip: bool) -> Thread {
    let mood = mood_of(news.valence());
    let open = if gossip {
        gossip_open(news)
    } else {
        news_open(news, about)
    };
    let vague = open == NEWS_VAGUE;
    let (friendly, follow) = news_specific(news);
    let (friendly, follow) = if vague {
        (None, generic_follow(mood))
    } else {
        (friendly, follow)
    };
    let reply = match mood {
        Mood::Good => [GOOD_REPLY_FRIENDLY, GOOD_REPLY_NEUTRAL, GOOD_REPLY_TENSE],
        Mood::Bad => [BAD_REPLY_FRIENDLY, BAD_REPLY_NEUTRAL, BAD_REPLY_TENSE],
        _ => [
            SCANDAL_REPLY_FRIENDLY,
            SCANDAL_REPLY_NEUTRAL,
            SCANDAL_REPLY_TENSE,
        ],
    };
    Thread {
        mood,
        open,
        reply: [friendly.unwrap_or(reply[0]), reply[1], reply[2]],
        follow,
        react: None,
        insist: None,
    }
}

fn generic_follow(mood: Mood) -> Pool {
    match mood {
        Mood::Good => GOOD_FOLLOW,
        Mood::Bad => BAD_FOLLOW,
        _ => SCANDAL_FOLLOW,
    }
}

fn news_open(news: News, about: bool) -> Pool {
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

fn gossip_open(news: News) -> Pool {
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

/// The friendly answer (if the generic one by valence won't do) and the
/// follow-up of a piece of news.
fn news_specific(news: News) -> (Option<Pool>, Pool) {
    match news {
        News::Birth => (Some(BIRTH_REPLY), BIRTH_FOLLOW),
        News::Death => (Some(DEATH_REPLY), DEATH_FOLLOW),
        News::Couple => (Some(COUPLE_REPLY), COUPLE_FOLLOW),
        News::Widowed => (Some(WIDOWED_REPLY), WIDOWED_FOLLOW),
        News::TheftCaught => (Some(THEFT_REPLY), THEFT_CAUGHT_FOLLOW),
        News::TheftUnseen => (None, THEFT_UNSEEN_FOLLOW),
        News::HelpGiven => (Some(GENEROUS_REPLY), HELP_GIVEN_FOLLOW),
        News::HelpRefused => (None, HELP_REFUSED_FOLLOW),
        News::Protest(Grievance::BirthDenied) => (None, PROTEST_BIRTHS_FOLLOW),
        News::Protest(Grievance::FoodShortage) => (None, PROTEST_FOOD_FOLLOW),
        News::Concession(Grievance::BirthDenied) => (None, CONCESSION_BIRTHS_FOLLOW),
        News::Concession(Grievance::FoodShortage) => (None, CONCESSION_FOOD_FOLLOW),
        News::Shortage(ItemKind::Razione) => (None, SHORTAGE_FOOD_FOLLOW),
        News::Shortage(_) => (None, SHORTAGE_FOLLOW),
        News::Restocked(_) => (None, RESTOCKED_FOLLOW),
        News::BirthDenied => (None, BIRTH_DENIED_FOLLOW),
        News::CameOfAge => (None, CAME_OF_AGE_FOLLOW),
        News::Retired => (Some(RETIRED_REPLY), RETIRED_FOLLOW),
        News::Austerity => (None, AUSTERITY_FOLLOW),
        News::PayRaised => (None, PAY_RAISED_FOLLOW),
        News::PayCut => (None, PAY_CUT_FOLLOW),
    }
}

// ----------------------------------------------------------------------
// Beats
// ----------------------------------------------------------------------

fn greet_pool(s: &Script, side: Side, rng: &mut Mix) -> Pool {
    if let Some(pool) = baby_pool(s, side, false) {
        return pool;
    }
    let (me, _) = speakers(s, side);
    let p = me.personality;
    if p.has(Temper::Timido) && rng.chance(0.4) {
        return SHY_GREET;
    }
    if p.has(Temper::Burbero) && rng.chance(0.4) {
        return GRUMPY_GREET;
    }
    if p.has(Temper::Allegro) && s.tone == Tone::Friendly && rng.chance(0.4) {
        return CHEERFUL_GREET;
    }
    if let Some(pool) = tie_greet(tie_of(s, side)) {
        return pool;
    }
    by_tone(s, [GREET_FRIENDLY, GREET_NEUTRAL, GREET_TENSE])
}

fn greet_back_pool(s: &Script, side: Side, rng: &mut Mix) -> Pool {
    if tie_of(s, side) == Some(RelationKind::Partner) && s.tone != Tone::Tense {
        return GREET_BACK_PARTNER;
    }
    if let Some(pool) = baby_pool(s, side, false) {
        return pool;
    }
    let (me, _) = speakers(s, side);
    let p = me.personality;
    if p.has(Temper::Timido) && rng.chance(0.4) {
        return SHY_GREET;
    }
    if p.has(Temper::Burbero) && rng.chance(0.4) {
        return GRUMPY_GREET_BACK;
    }
    if let Some(pool) = tie_greet(tie_of(s, side)) {
        return pool;
    }
    by_tone(s, [GREET_BACK_FRIENDLY, GREET_BACK_NEUTRAL, GREET_TENSE])
}

/// Greeting to one's family (`tie`: what the listener is to the speaker).
fn tie_greet(tie: Option<RelationKind>) -> Option<Pool> {
    match tie? {
        RelationKind::Partner => Some(GREET_PARTNER),
        RelationKind::Child => Some(GREET_CHILD),
        RelationKind::Parent => Some(GREET_PARENT),
        RelationKind::Sibling => Some(GREET_SIBLING),
        RelationKind::Friend => None,
    }
}

fn statement_pool(s: &Script, side: Side, thread: &Thread) -> Pool {
    match baby_pool(s, side, false) {
        Some(TO_BABY) => TO_BABY_OPEN,
        Some(pool) => pool,
        None => thread.open,
    }
}

/// The answer to the statement; true if it is a question (the follow-up
/// then answers it).
fn reply_pool(
    s: &Script,
    side: Side,
    thread: &Thread,
    can_ask: bool,
    rng: &mut Mix,
) -> (Pool, bool) {
    if let Some(pool) = baby_pool(s, side, false) {
        return (pool, false);
    }
    let (me, _) = speakers(s, side);
    let (p, mood) = (me.personality, thread.mood);
    if s.tone == Tone::Tense {
        let kid = me.age < KID_AGE && rng.chance(0.5);
        return (if kid { KID_TENSE } else { thread.reply[2] }, false);
    }
    if me.age < KID_AGE && rng.chance(0.5) {
        return (kid_reply(mood), false);
    }
    if p.has(Temper::Timido) && rng.chance(0.35) {
        return (shy_reply(mood), false);
    }
    if p.has(Temper::Burbero) && rng.chance(0.5) {
        return (curt(mood), false);
    }
    // The curious ask where the news comes from (not about an opinion).
    if p.has(Temper::Curioso)
        && can_ask
        && matches!(s.topic, Topic::News | Topic::Gossip)
        && s.news.is_some()
        && mood != Mood::Neutral
        && rng.chance(0.4)
    {
        return (CURIOUS, true);
    }
    if p.has(Temper::Gentile)
        && let Some(pool) = kind(mood)
        && rng.chance(0.4)
    {
        return (pool, false);
    }
    if p.has(Temper::Allegro)
        && let Some(pool) = cheerful(s, mood, false)
        && rng.chance(0.4)
    {
        return (pool, false);
    }
    (by_tone(s, thread.reply), false)
}

fn follow_pool(
    s: &Script,
    side: Side,
    thread: &Thread,
    asked: bool,
    rng: &mut Mix,
) -> (Pool, FollowKind) {
    if let Some(pool) = baby_pool(s, side, false) {
        return (pool, FollowKind::Kid);
    }
    let (me, _) = speakers(s, side);
    let (p, mood) = (me.personality, thread.mood);
    if asked {
        return (answer(mood), FollowKind::Answer);
    }
    if me.age < KID_AGE && rng.chance(0.5) {
        return (KID, FollowKind::Kid);
    }
    if s.tone == Tone::Tense {
        return (
            thread.insist.unwrap_or_else(|| insist(mood)),
            FollowKind::Insist,
        );
    }
    if p.has(Temper::Pettegolo) && mood == Mood::Scandal && rng.chance(0.5) {
        return (SECRET, FollowKind::Secret);
    }
    if p.has(Temper::Lamentoso) && s.topic != Topic::Family && rng.chance(0.4) {
        return (lament(mood), FollowKind::Lament);
    }
    if p.has(Temper::Timido) && rng.chance(0.3) {
        return (SHY_FOLLOW, FollowKind::Shy);
    }
    (thread.follow, FollowKind::Plain)
}

fn react_pool(s: &Script, side: Side, thread: &Thread, follow: FollowKind, rng: &mut Mix) -> Pool {
    if let Some(pool) = baby_pool(s, side, false) {
        return pool;
    }
    let (me, _) = speakers(s, side);
    let (p, mood) = (me.personality, thread.mood);
    let kid = me.age < KID_AGE;
    match follow {
        FollowKind::Kid if kid => return KID,
        FollowKind::Kid => return REACT_TO_KID,
        FollowKind::Secret => return by_tone(s, REACT_SECRET),
        FollowKind::Lament => return by_tone(s, REACT_LAMENT),
        FollowKind::Shy => return by_tone(s, REACT_SHY),
        FollowKind::Insist if p.has(Temper::Gentile) && rng.chance(0.4) => return PEACEMAKER,
        FollowKind::Insist => return REACT_TENSE,
        FollowKind::Answer if s.tone != Tone::Tense => return by_tone(s, REACT_ANSWER),
        FollowKind::Answer | FollowKind::Plain => {}
    }
    if kid && rng.chance(0.5) {
        return kid_reply(mood);
    }
    if s.tone != Tone::Tense {
        if p.has(Temper::Timido) && rng.chance(0.35) {
            return shy_reply(mood);
        }
        if p.has(Temper::Burbero) && rng.chance(0.5) {
            return curt(mood);
        }
        if p.has(Temper::Gentile)
            && let Some(pool) = kind(mood)
            && rng.chance(0.4)
        {
            return pool;
        }
        if p.has(Temper::Allegro)
            && let Some(pool) = cheerful(s, mood, true)
            && rng.chance(0.4)
        {
            return pool;
        }
    }
    by_tone(s, thread.react.unwrap_or_else(|| react(mood)))
}

fn close_pool(s: &Script, side: Side, thread: &Thread, rng: &mut Mix) -> Pool {
    if let Some(pool) = baby_pool(s, side, true) {
        return pool;
    }
    let (me, _) = speakers(s, side);
    let p = me.personality;
    if p.has(Temper::Burbero) && rng.chance(0.5) {
        return GRUMPY_CLOSE;
    }
    if p.has(Temper::Timido) && rng.chance(0.3) {
        return SHY_CLOSE;
    }
    if me.age < KID_AGE && rng.chance(0.5) {
        return KID_CLOSE;
    }
    if s.tone == Tone::Tense {
        return CLOSE_TENSE;
    }
    // Who did most of the talking apologizes for it.
    if side == Side::A && p.has(Temper::Chiacchierone) && rng.chance(0.3) {
        return CHATTY_CLOSE;
    }
    if let Some(pool) = tie_close(s, side)
        && rng.chance(0.5)
    {
        return pool;
    }
    // After a trouble, who told it thanks, who heard it wishes well.
    match (s.tone, thread.mood, side) {
        (Tone::Friendly, Mood::Gripe, Side::A) => CLOSE_THANKS,
        (Tone::Friendly, Mood::Bad | Mood::Gripe, _) => CLOSE_SOMBER,
        (Tone::Friendly, _, _) => CLOSE_FRIENDLY,
        _ => CLOSE_NEUTRAL,
    }
}

fn close_back_pool(s: &Script, side: Side, rng: &mut Mix) -> Pool {
    if let Some(pool) = baby_pool(s, side, true) {
        return pool;
    }
    let (me, _) = speakers(s, side);
    let p = me.personality;
    if p.has(Temper::Burbero) && rng.chance(0.5) {
        return GRUMPY_CLOSE;
    }
    if p.has(Temper::Timido) && rng.chance(0.3) {
        return SHY_CLOSE;
    }
    if me.age < KID_AGE && rng.chance(0.5) {
        return KID_CLOSE;
    }
    if s.tone != Tone::Tense
        && let Some(pool) = tie_close(s, side)
        && rng.chance(0.5)
    {
        return pool;
    }
    by_tone(
        s,
        [CLOSE_BACK_FRIENDLY, CLOSE_BACK_NEUTRAL, CLOSE_BACK_TENSE],
    )
}

/// Goodbye to one's family.
fn tie_close(s: &Script, side: Side) -> Option<Pool> {
    let (_, you) = speakers(s, side);
    match tie_of(s, side)? {
        RelationKind::Partner => Some(CLOSE_PARTNER),
        RelationKind::Child if you.age < KID_AGE => Some(CLOSE_TO_KID),
        RelationKind::Child => Some(CLOSE_TO_CHILD),
        RelationKind::Parent => Some(CLOSE_TO_PARENT),
        RelationKind::Sibling => Some(CLOSE_SIBLING),
        RelationKind::Friend => None,
    }
}

// Personality and mood.

fn shy_reply(mood: Mood) -> Pool {
    match mood {
        Mood::Good => SHY_GOOD,
        Mood::Bad => SHY_BAD,
        Mood::Gripe => SHY_GRIPE,
        Mood::Scandal => SHY_SCANDAL,
        Mood::Neutral => SHY,
    }
}

fn curt(mood: Mood) -> Pool {
    match mood {
        Mood::Good => CURT_GOOD,
        Mood::Bad => CURT_BAD,
        Mood::Scandal => CURT_SCANDAL,
        Mood::Gripe => CURT_GRIPE,
        Mood::Neutral => CURT,
    }
}

fn kid_reply(mood: Mood) -> Pool {
    match mood {
        Mood::Good => KID_GOOD,
        Mood::Bad => KID_BAD,
        Mood::Scandal => KID_SCANDAL,
        Mood::Gripe => KID_GRIPE,
        Mood::Neutral => KID_NEUTRAL,
    }
}

fn kind(mood: Mood) -> Option<Pool> {
    match mood {
        Mood::Bad => Some(KIND_BAD),
        Mood::Gripe => Some(KIND_GRIPE),
        _ => None,
    }
}

/// A cheerful answer (`react`: to the follow-up rather than the opener).
fn cheerful(s: &Script, mood: Mood, react: bool) -> Option<Pool> {
    match (mood, s.topic) {
        (Mood::Good, Topic::Work) => Some(CHEERFUL_WORK),
        (Mood::Good, Topic::Gossip) if s.news.is_none() => Some(CHEERFUL_FOND),
        (Mood::Good, _) => Some(CHEERFUL_GOOD),
        (Mood::Gripe, _) => Some(CHEERFUL_GRIPE),
        (Mood::Neutral, Topic::SmallTalk) if react => Some(CHEERFUL_SMALL),
        (Mood::Neutral, Topic::SmallTalk) => Some(CHEERFUL_HOW),
        _ => None,
    }
}

fn answer(mood: Mood) -> Pool {
    match mood {
        Mood::Scandal => ANSWER_SCANDAL,
        _ => ANSWER,
    }
}

fn insist(mood: Mood) -> Pool {
    match mood {
        Mood::Good => INSIST_GOOD,
        Mood::Bad => INSIST_BAD,
        Mood::Scandal => INSIST_SCANDAL,
        Mood::Gripe => INSIST_GRIPE,
        Mood::Neutral => INSIST_NEUTRAL,
    }
}

fn lament(mood: Mood) -> Pool {
    match mood {
        Mood::Good => LAMENT_GOOD,
        Mood::Bad => LAMENT_BAD,
        Mood::Gripe => LAMENT_GRIPE,
        Mood::Scandal => LAMENT_SCANDAL,
        Mood::Neutral => LAMENT_NEUTRAL,
    }
}

fn react(mood: Mood) -> [Pool; 3] {
    match mood {
        Mood::Good => [REACT_GOOD_FRIENDLY, REACT_GOOD_NEUTRAL, REACT_TENSE],
        Mood::Bad => [REACT_BAD_FRIENDLY, REACT_BAD_NEUTRAL, REACT_TENSE],
        Mood::Scandal => [REACT_SCANDAL_FRIENDLY, REACT_NEUTRAL_NEUTRAL, REACT_TENSE],
        Mood::Gripe => [REACT_GRIPE_FRIENDLY, REACT_GRIPE_NEUTRAL, REACT_GRIPE_TENSE],
        Mood::Neutral => [REACT_NEUTRAL_FRIENDLY, REACT_NEUTRAL_NEUTRAL, REACT_TENSE],
    }
}

// ----------------------------------------------------------------------
// Rendering
// ----------------------------------------------------------------------

/// Renders a random template of `pool` that fits [`MAX_LINE_CHARS`],
/// preferring one not `said` yet.
fn pick(
    pool: Pool,
    s: &Script,
    side: Side,
    said: &[&str],
    rng: &mut Mix,
) -> Option<(&'static str, String)> {
    if pool.is_empty() {
        return None;
    }
    let start = rng.below(pool.len());
    let fitting = (0..pool.len()).filter_map(|k| {
        let template = pool[(start + k) % pool.len()];
        let text = render(template, s, side)?;
        (text.chars().count() <= MAX_LINE_CHARS).then_some((template, text))
    });
    let mut first = None;
    for (template, text) in fitting {
        if !said.contains(&text.as_str()) {
            return Some((template, text));
        }
        first.get_or_insert((template, text));
    }
    first
}

/// "in serra", "in cucina"...: where someone with `job` works.
pub(super) fn workplace(job: Option<Job>) -> &'static str {
    match job {
        Some(Job::Contadino) => "in serra",
        Some(Job::Cuoco) => "in cucina",
        Some(Job::Operaio) => "in officina",
        Some(Job::Mercante) => "al banco",
        None => "in giro",
    }
}

pub(super) fn job_word(job: Option<Job>, sex: Sex) -> &'static str {
    match job {
        Some(Job::Contadino) => sex.pick("contadina", "contadino"),
        Some(Job::Cuoco) => sex.pick("cuoca", "cuoco"),
        Some(Job::Operaio) => sex.pick("operaia", "operaio"),
        Some(Job::Mercante) => "mercante",
        None => sex.pick("disoccupata", "disoccupato"),
    }
}

/// The item of the news: plural and whether it is feminine ("scorte" if
/// the news is about no item).
fn news_item(news: Option<News>) -> (&'static str, bool) {
    match news {
        Some(News::Shortage(item) | News::Restocked(item)) => (
            item.plural(),
            matches!(item, ItemKind::Verdura | ItemKind::Razione),
        ),
        _ => ("scorte", true),
    }
}

pub(super) fn greeting(hour: u32) -> &'static str {
    if (5..14).contains(&hour) {
        "Buongiorno"
    } else {
        "Buonasera"
    }
}

pub(super) fn goodbye(hour: u32) -> &'static str {
    match hour {
        5..=16 => "Buona giornata",
        17..=21 => "Buona serata",
        _ => "Buonanotte",
    }
}

/// Appends `value` after `out`, turning a joining "e"/"a" into "ed"/"ad"
/// before a vowel ("Bruno ed Elena", "ad Anna").
pub(super) fn push_joined(out: &mut String, value: &str) {
    for (word, euphonic) in [("e", grammar::e_ed(value)), ("a", grammar::a_ad(value))] {
        if euphonic.len() == word.len() {
            continue;
        }
        let joined = out.ends_with(&format!(" {word} "))
            || out == &format!("{word} ")
            || out == &format!("{} ", word.to_uppercase());
        if joined {
            out.pop();
            out.push('d');
            out.push(' ');
            break;
        }
    }
    out.push_str(value);
}

/// Fills the placeholders of `template` for `side` speaking; None if it
/// needs data the script lacks (whom the talk is about, their partner).
fn render(template: &str, s: &Script, side: Side) -> Option<String> {
    let (me, you) = speakers(s, side);
    let about = s.about;
    let (item, item_fem) = news_item(s.news);
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let len = rest[open..].find('}')?;
        let token = &rest[open + 1..open + len];
        rest = &rest[open + len + 1..];
        if let Some((who, forms)) = token.split_once(':') {
            let (female, male) = forms.split_once('/').unwrap_or((forms, forms));
            let female_form = match who {
                "S" => me.sex == Sex::Female,
                "L" => you.sex == Sex::Female,
                "I" => item_fem,
                _ => about.is_some_and(|a| a.sex == Sex::Female),
            };
            push_joined(&mut out, if female_form { female } else { male });
            continue;
        }
        let value: String = match token {
            "n" => you.first.to_string(),
            "s" => me.first.to_string(),
            "a" => about?.first.clone(),
            "f" => about?.surname.clone(),
            "o" => {
                let other = &about?.other;
                if other.is_empty() {
                    return None;
                }
                other.clone()
            }
            "fam" => grammar::family(None, &about?.surname),
            "dei" => grammar::family(Some(Prep::Di), &about?.surname),
            "ai" => grammar::family(Some(Prep::A), &about?.surname),
            "i" => item.to_string(),
            "li" => format!(
                "{}{item}",
                grammar::article(item, item_fem, true).with(None)
            ),
            "al" => grammar::prep_a(s.place),
            "nel" => grammar::prep_in(s.place),
            "del" => grammar::prep_di(s.place),
            "w" => workplace(me.job).to_string(),
            "W" => workplace(you.job).to_string(),
            "j" => job_word(me.job, me.sex).to_string(),
            "k" => {
                let word = job_word(me.job, me.sex);
                let fem = me.sex == Sex::Female;
                format!("{}{word}", grammar::article(word, fem, false).with(None))
            }
            "g" => greeting(s.hour).to_string(),
            "bg" => goodbye(s.hour).to_string(),
            _ => return None,
        };
        push_joined(&mut out, &value);
    }
    out.push_str(rest);
    Some(grammar::capitalize(&grammar::tidy(&out)))
}

// ----------------------------------------------------------------------
// Templates
// ----------------------------------------------------------------------

// Greetings.

const GREET_FRIENDLY: Pool = &[
    "Ciao {n}!",
    "{g}, {n}!",
    "Ehi {n}! Hai un minuto?",
    "Oh, {n}! Che piacere.",
    "Ciao {n}, ti stavo cercando!",
];
const GREET_NEUTRAL: Pool = &["{g}.", "{g}, {n}.", "Ciao, {n}.", "Salve, {n}."];
const GREET_TENSE: Pool = &["Oh. Ciao, {n}.", "Ah, sei tu.", "Sì?"];
const GREET_BACK_FRIENDLY: Pool = &[
    "Ciao {n}! Dimmi tutto.",
    "Oh, {n}! Che bello vederti.",
    "Eccomi! Dimmi pure.",
    "{g}, {n}!",
];
const GREET_BACK_NEUTRAL: Pool = &["{g}.", "Ciao, {n}. Dimmi.", "Sì, dimmi."];
const GREET_PARTNER: Pool = &["Ciao, amore!", "Eccoti, tesoro!", "Amore mio, ciao!"];
const GREET_BACK_PARTNER: Pool = &["Ciao, tesoro!", "Eccomi, amore.", "Ciao, amore mio."];
const GREET_CHILD: Pool = &["Ciao, tesoro!", "Eccoti, {n}!", "Vieni qui, {n}!"];
const GREET_PARENT: Pool = &["Ciao, {L:mamma/papà}!", "Eccomi, {L:mamma/papà}!"];
const GREET_SIBLING: Pool = &["Ehi, {L:sorella/fratello}!", "Ciao, {n}!"];
const SHY_GREET: Pool = &["Ehm… ciao, {n}.", "C-ciao…", "Oh… ciao."];
const GRUMPY_GREET: Pool = &["Ehi.", "{n}.", "Mh. Ciao."];
const GRUMPY_GREET_BACK: Pool = &["Mh. Che c'è?", "Sì?", "Dimmi, ho da fare."];
const CHEERFUL_GREET: Pool = &["{n}! Che bello vederti!", "Ciao {n}! Che bella giornata!"];

// Small talk: how are you, then a remark on life aboard.

const SMALL_OPEN_FRIENDLY: Pool = &[
    "Ciao {n}, tutto bene?",
    "Ehi {n}! Come va oggi?",
    "Ciao {n}! Come stai?",
    "Oh, {n}! Come te la passi?",
    "{g}, {n}! Tutto bene?",
    "Ben trovat{L:a/o}, {n}! Come va?",
];
const SMALL_OPEN_NEUTRAL: Pool = &[
    "{g}. Tutto a posto?",
    "Ciao {n}. Come va?",
    "Si va avanti, {n}?",
    "Che si dice in giro?",
];
const SMALL_OPEN_TENSE: Pool = &[
    "Ancora tu, {n}?",
    "Guarda chi si vede…",
    "Oh. Sei tu, {n}.",
    "Sempre tra i piedi, eh?",
];
const SMALL_REPLY_FRIENDLY: Pool = &[
    "Si tira avanti, dai.",
    "Benone, grazie!",
    "Tutto bene, grazie!",
    "Bene, adesso che ci sei tu.",
    "Non mi lamento, dai.",
    "Stanc{S:a/o}, ma content{S:a/o}.",
];
const SMALL_REPLY_NEUTRAL: Pool = &[
    "Mah, si va avanti.",
    "Il solito.",
    "Come sempre, {n}.",
    "Niente di nuovo.",
    "Tutto uguale a ieri.",
];
const SMALL_REPLY_TENSE: Pool = &[
    "Cosa vuoi, {n}?",
    "Fatti gli affari tuoi.",
    "Non ho voglia di parlare.",
    "Lasciami in pace, dai.",
];
const SMALL_REMARK: Pool = &[
    "Fuori si gela, dicono -80.",
    "Stanotte il treno ha sobbalzato.",
    "Hai sentito che vento stamattina?",
    "Oggi {al} c'è un bel via vai.",
    "Il tè della mensa è sempre peggio.",
    "Mi è sembrato di vedere il sole.",
    "Chissà dove siamo adesso.",
    "Le luci hanno tremato di nuovo.",
    "Quanta neve contro i finestrini!",
    "Abbiamo preso un ponte, l'hai sentito?",
];
const REMARK_REACT_FRIENDLY: Pool = &[
    "Eh già, pensa te!",
    "Me ne sono accort{S:a/o} anch'io!",
    "Davvero? Non ci avevo fatto caso.",
    "Hai ragione, strano davvero.",
];

// Work: a good day or a bad one; or asking about the listener's.

const WORK_GOOD_CONTADINO: Pool = &[
    "La verdura viene su bene, sai?",
    "Oggi ho raccolto un sacco di verdura.",
    "In serra è spuntato un pomodoro!",
];
const WORK_BAD_CONTADINO: Pool = &[
    "Oggi in serra non si respira.",
    "I pomodori non vogliono crescere.",
    "Le lampade della serra scaldano poco.",
    "Ho le mani piene di terra.",
];
const WORK_GOOD_CUOCO: Pool = &[
    "La zuppa di oggi l'ho fatta io!",
    "Oggi in cucina è filato tutto liscio.",
    "Ho inventato una zuppa nuova!",
];
const WORK_BAD_CUOCO: Pool = &[
    "In cucina oggi era un inferno.",
    "Ho pelato verdura tutta la mattina.",
    "Mancano sempre le pentole grandi.",
];
const WORK_GOOD_OPERAIO: Pool = &[
    "Ho saldato un pezzo perfetto!",
    "Oggi in officina si rideva.",
    "Ho aggiustato il riscaldamento!",
];
const WORK_BAD_OPERAIO: Pool = &[
    "In officina si suda, altro che freddo.",
    "Ho saldato lamiere tutto il giorno.",
    "Il rottame non finisce mai.",
    "Mi fischiano ancora le orecchie.",
];
const WORK_GOOD_MERCANTE: Pool = &[
    "Oggi ho venduto tre attrezzi!",
    "Al banco oggi c'era la fila.",
    "Tutti vogliono vestiti nuovi.",
];
const WORK_BAD_MERCANTE: Pool = &[
    "Oggi al banco non passa nessuno.",
    "I prezzi li decidono in testa.",
    "Nessuno ha più gettoni da spendere.",
];
const WORK_GOOD_REPLY_FRIENDLY: Pool = &[
    "Brav{L:a/o}, sei instancabile.",
    "Che bello, complimenti!",
    "Si vede che ti piace.",
];
const WORK_GOOD_REPLY_NEUTRAL: Pool = &["Ah, bene.", "Meglio così.", "Buon per te."];
const WORK_GOOD_REPLY_TENSE: Pool = &[
    "Non ti montare la testa.",
    "Content{L:a/o} tu…",
    "Sai che fatica, eh.",
];
const WORK_GOOD_FOLLOW: Pool = &[
    "Fare {k} mi piace, sai?",
    "Domani faccio ancora meglio.",
    "Almeno {w} si sta al caldo.",
    "Con un attrezzo nuovo farei il doppio.",
];
const WORK_BAD_REPLY_FRIENDLY: Pool = &[
    "Ti capisco, anche da me è dura.",
    "Dai, che il turno finisce presto.",
    "Almeno si lavora in compagnia!",
];
const WORK_BAD_REPLY_NEUTRAL: Pool = &[
    "Il lavoro è lavoro.",
    "Si fa quel che si può.",
    "Almeno ci pagano.",
];
const WORK_BAD_REPLY_TENSE: Pool = &[
    "Smettila di lamentarti.",
    "E io che dovrei dire?",
    "Lavora e non parlare.",
];
const WORK_BAD_FOLLOW: Pool = &[
    "Il capoturno non mi dà tregua.",
    "Mi fa male la schiena.",
    "Fare {k} non è uno scherzo.",
    "Domani attacco presto, di nuovo.",
];
const WORK_ASK_OPEN: Pool = &[
    "Com'è andata {W} oggi?",
    "Si lavora tanto {W}?",
    "Come va il lavoro, {n}?",
];
const WORK_ASKED_FRIENDLY: Pool = &[
    "Tanto, ma mi piace!",
    "Si fatica, ma si va avanti.",
    "Bene, grazie che me lo chiedi.",
];
const WORK_ASKED_NEUTRAL: Pool = &[
    "Il solito, niente di speciale.",
    "Come sempre.",
    "Si lavora, che vuoi.",
];
const WORK_ASKED_TENSE: Pool = &[
    "Perché, ti interessa?",
    "Meglio che non te lo dica.",
    "Prova tu a lavorare, poi vediamo.",
];
const WORK_ASK_FOLLOW: Pool = &[
    "Mi piacerebbe lavorare {W}.",
    "Beat{L:a/o} te che hai un posto.",
    "Chissà se cercano qualcuno.",
];

// Needs.

const NEEDS_OPEN_HUNGER: Pool = &[
    "Ho una fame che non ci vedo.",
    "Mi brontola lo stomaco…",
    "Darei un gettone per una razione.",
];
const NEEDS_HUNGER_FRIENDLY: Pool = &[
    "Vieni, andiamo in mensa insieme.",
    "Tieni duro, tra poco si mangia.",
    "Anch'io, sai? Ho un buco qui.",
];
const NEEDS_HUNGER_NEUTRAL: Pool = &["Eh, capita.", "Anche a me, sai.", "Manca poco alla mensa."];
const NEEDS_HUNGER_FOLLOW: Pool = &[
    "Speriamo che la cena sia buona.",
    "Mangerei anche il tè, pensa.",
    "Oggi la zuppa la voglio doppia.",
];
const NEEDS_OPEN_TIRED: Pool = &[
    "Sono stanc{S:a/o} mort{S:a/o}.",
    "Stanotte non ho chiuso occhio.",
    "Mi reggo in piedi a fatica.",
    "Avrei bisogno di un pisolino.",
];
const NEEDS_TIRED_FRIENDLY: Pool = &[
    "Riposati un po', dai.",
    "Ti capisco, anch'io.",
    "Tieni duro, {n}.",
];
const NEEDS_TIRED_NEUTRAL: Pool = &["Eh, succede.", "Siamo tutti così.", "Passerà."];
const NEEDS_TIRED_FOLLOW: Pool = &[
    "Stasera vado a letto presto.",
    "Questo treno ci sfinisce.",
    "Un po' di tè caldo mi salverebbe.",
];
const NEEDS_REPLY_TENSE: Pool = &[
    "E cosa vuoi che ci faccia?",
    "Non sei l'unic{L:a/o}.",
    "Lamentati con qualcun altro.",
    "Arrangiati.",
];
const NEEDS_OPEN_LONELY: Pool = &[
    "Mi mancava fare due chiacchiere.",
    "Non parlo con nessuno da ore.",
    "Che bello vedere una faccia amica.",
];
const NEEDS_LONELY_FRIENDLY: Pool = &[
    "Anche a me, {n}!",
    "Ci sono io, tranquill{L:a/o}.",
    "Quando vuoi, sai dove trovarmi.",
];
const NEEDS_LONELY_NEUTRAL: Pool = &["Eh, capita.", "Ci si sente soli, qui."];
const NEEDS_LONELY_TENSE: Pool = &["Non ho tempo, scusa.", "Cerca qualcun altro, dai."];
const NEEDS_LONELY_FOLLOW: Pool = &[
    "Grazie di fermarti, davvero.",
    "Dovremmo vederci più spesso.",
    "Mi fa bene parlare un po'.",
];
const NEEDS_LONELY_REACT: Pool = &["Figurati, {n}.", "Hai ragione, sai?", "Quando vuoi."];

// Family.

const PARTNER_INVITE_OPEN: Pool = &[
    "Stasera ceniamo insieme?",
    "Ti ho tenuto il posto in mensa.",
    "Domani mangiamo insieme?",
];
const PARTNER_INVITE_FRIENDLY: Pool = &[
    "Certo, con piacere!",
    "Volentieri, amore.",
    "Non vedo l'ora!",
];
const PARTNER_INVITE_NEUTRAL: Pool = &["Va bene.", "Se riesco, sì.", "Vediamo, dai."];
const INVITE_TENSE: Pool = &[
    "Non adesso, per favore.",
    "Vedremo.",
    "Sempre a organizzare, tu.",
];
const PARTNER_INVITE_FOLLOW: Pool = &[
    "Ti aspetto, allora.",
    "Porto io il tè.",
    "Così stiamo un po' insieme.",
];
const PARTNER_ASK_OPEN: Pool = &[
    "Hai dormito bene, amore?",
    "Come stai, tesoro?",
    "Com'è andata oggi, {n}?",
];
const PARTNER_ASK_FRIENDLY: Pool = &[
    "Bene, adesso che ci sei tu.",
    "Tutto bene, grazie amore.",
    "Benone, tesoro.",
];
const ASKED_NEUTRAL: Pool = &["Più o meno.", "Si tira avanti.", "Tutto normale."];
const ASKED_TENSE: Pool = &[
    "Lasciami stare, oggi.",
    "Sempre le stesse domande…",
    "Ne parliamo dopo.",
];
const TOGETHER_FOLLOW: Pool = &[
    "Dovremmo stare più insieme.",
    "Dovremmo vederci più spesso.",
    "Stasera cena tutti insieme, eh.",
    "La famiglia prima di tutto.",
];
const PARTNER_LOVE_OPEN: Pool = &[
    "Mi sei mancat{L:a/o} oggi.",
    "Ti ho pensat{L:a/o} tutto il giorno.",
];
const PARTNER_LOVE_FRIENDLY: Pool = &[
    "Anch'io, amore mio.",
    "Sei sempre così dolce.",
    "Anch'io ti ho pensat{L:a/o}.",
];
const PARTNER_LOVE_NEUTRAL: Pool = &["Ah, sì?", "Mh, anche tu."];
const PARTNER_LOVE_TENSE: Pool = &["Non adesso, per favore.", "Sì, sì…"];
const TO_CHILD_ASK_OPEN: Pool = &[
    "Hai mangiato, tesoro?",
    "Com'è andata oggi, {n}?",
    "Tutto bene, tesoro?",
];
const CHILD_ANSWER_FRIENDLY: Pool = &[
    "Sì, {L:mamma/papà}!",
    "Tutto bene, grazie!",
    "Benone, {L:mamma/papà}.",
];
const CHILD_ANSWER_NEUTRAL: Pool = &["Sì, sì.", "Più o meno.", "Il solito."];
const CHILD_ANSWER_TENSE: Pool = &[
    "Non sono più {S:una bambina/un bambino}!",
    "Uffa, sempre le stesse domande!",
    "Lasciami stare, oggi.",
];
const TO_CHILD_ASK_FOLLOW: Pool = &[
    "Sono fier{S:a/o} di te, {n}.",
    "Ti voglio bene, sai?",
    "Sei il mio orgoglio, {n}.",
];
const TO_CHILD_ASK_REACT: Pool = &["Grazie, {L:mamma/papà}!", "Oh, {L:mamma/papà}…"];
const TO_CHILD_CARE_OPEN: Pool = &[
    "Copriti bene, fa freddo.",
    "Non fare tardi stasera.",
    "Mangia qualcosa, eh.",
];
const TO_CHILD_CARE_FRIENDLY: Pool = &["Sì, {L:mamma/papà}!", "Va bene, promesso."];
const TO_CHILD_CARE_NEUTRAL: Pool = &["Sì, sì.", "Va bene."];
const TO_CHILD_CARE_TENSE: Pool = &[
    "Non sono più {S:una bambina/un bambino}!",
    "Lo so, lo so!",
    "Uffa!",
];
const TO_CHILD_CARE_FOLLOW: Pool = &["Te lo dico per il tuo bene.", "Mi preoccupo sempre, sai?"];
const TO_CHILD_CARE_REACT: Pool = &["Lo so, {L:mamma/papà}.", "Lo so, grazie."];
const KID_REQUEST_OPEN: Pool = &[
    "{L:Mamma/Papà}, hai un minuto?",
    "{L:Mamma/Papà}, mi racconti una storia?",
    "{L:Mamma/Papà}, ho fame!",
];
const KID_REQUEST_FRIENDLY: Pool = &["Certo, tesoro.", "Ma certo, {n}.", "Arrivo, tesoro."];
const KID_REQUEST_NEUTRAL: Pool = &["Sì, dopo.", "Un attimo, {n}."];
const KID_REQUEST_TENSE: Pool = &["Non adesso, {n}.", "Sono stanc{S:a/o}, dopo."];
const KID_REQUEST_FOLLOW: Pool = &[
    "Grazie, {L:mamma/papà}!",
    "Sei {L:la mamma/il papà} migliore!",
];
const KID_REQUEST_REACT: Pool = &["Ma figurati, tesoro.", "Su, vieni qui."];
const KID_TELL_OPEN: Pool = &[
    "Sai cosa ho visto oggi?",
    "{L:Mamma/Papà}, indovina cosa ho visto!",
];
const KID_TELL_FRIENDLY: Pool = &["Racconta, tesoro!", "Dimmi tutto!", "Cosa? Racconta!"];
const KID_TELL_NEUTRAL: Pool = &["Mh, dimmi.", "Dimmi, {n}."];
const KID_TELL_TENSE: Pool = &["Non adesso, {n}.", "Dopo, {n}, dopo."];
const KID_TELL_FOLLOW: Pool = &[
    "Un'ombra enorme, fuori!",
    "Un topo in cuccetta!",
    "Il sole dal finestrino!",
];
const KID_TELL_REACT: Pool = &[
    "Ma dai! Davvero?",
    "Pensa un po'!",
    "Che occhi che hai, {n}!",
];
const TO_PARENT_ASK_OPEN: Pool = &[
    "Tutto bene, {L:mamma/papà}?",
    "Come ti senti oggi, {L:mamma/papà}?",
    "Tutto a posto, {L:mamma/papà}?",
];
const PARENT_ANSWER_FRIENDLY: Pool = &["Bene, tesoro, grazie.", "Benone, grazie {n}."];
const PARENT_ANSWER_NEUTRAL: Pool = &["Si tira avanti.", "Gli acciacchi, sai."];
const PARENT_ANSWER_TENSE: Pool = &["Non ti preoccupare per me.", "Sto bene, smettila."];
const TO_PARENT_ASK_FOLLOW: Pool = &[
    "Se ti serve, ci sono.",
    "Passo a trovarti stasera.",
    "Riguardati, mi raccomando.",
];
const TO_PARENT_ASK_REACT: Pool = &["Grazie, tesoro.", "Sei un tesoro, {n}."];
const SIBLING_MEMORY_OPEN: Pool = &[
    "Ti ricordi quando eravamo piccoli?",
    "Ti ricordi la vecchia cuccetta?",
];
const SIBLING_MEMORY_FRIENDLY: Pool = &["Come dimenticarlo!", "Che tempi, eh!"];
const SIBLING_MEMORY_NEUTRAL: Pool = &["Vagamente.", "Mah, un po'."];
const SIBLING_MEMORY_TENSE: Pool = &["Preferisco non ricordare.", "Eri insopportabile, allora."];
const SIBLING_MEMORY_FOLLOW: Pool = &[
    "Litigavamo sempre, ahah.",
    "Eravamo inseparabili.",
    "Mi mancano quei tempi.",
];
const SIBLING_MEMORY_REACT: Pool = &["Anche a me, sai?", "Eh, altri tempi.", "Ahah, è vero!"];
const SIBLING_ASK_OPEN: Pool = &["{L:Sorella/Fratello}, come stai?", "Ehi, tutto bene?"];
const SIBLING_ASK_FRIENDLY: Pool = &["Tutto bene, grazie!", "Benone, grazie!"];
const FOLKS_OPEN: Pool = &[
    "Come sta la tua famiglia?",
    "Tutto bene a casa?",
    "I tuoi come stanno?",
];
const SIBLING_FOLKS_OPEN: Pool = &["Hai sentito i nostri?", "Come stanno i nostri?"];
const FOLKS_FRIENDLY: Pool = &[
    "Tutto bene, grazie di chiedere.",
    "Stanno tutti bene, grazie!",
];
const FOLKS_TENSE: Pool = &["Perché ti interessa?", "Fatti gli affari tuoi."];
const FOLKS_FOLLOW: Pool = &["Salutameli tanto.", "La famiglia prima di tutto."];
const PARTNER_INVITE_REACT: Pool = &["Perfetto, amore.", "Sì, dai!", "Va benissimo."];
const KID_TELL_REACT_NEUTRAL: Pool = &["Pensa un po'.", "Ah, sì?"];
const FOLKS_REACT: Pool = &["Certo, lo farò.", "Grazie, {n}."];
const FAMILY_REACT_FRIENDLY: Pool = &["Hai ragione, sai?", "Lo penso anch'io.", "Sì, dai."];
const FAMILY_REACT_NEUTRAL: Pool = &["Sì, sì.", "Va bene.", "Certo."];

// News.

const NEWS_BIRTH: Pool = &[
    "È nat{A:a/o} {a}, {dei}!",
    "{A:Una bimba/Un bimbo} in casa {f}!",
    "{fam} hanno avuto {A:una bimba/un bimbo}!",
];
const NEWS_DEATH: Pool = &[
    "Hai saputo? È mort{A:a/o} {a} {f}.",
    "{a} {f} non c'è più…",
    "Ci ha lasciati {a} {f}.",
];
const NEWS_COUPLE: Pool = &[
    "{a} e {o} stanno insieme!",
    "Hai visto {a} e {o}? Coppia!",
    "Finalmente {a} e {o}!",
];
const NEWS_WIDOWED: Pool = &[
    "Pover{A:a/o} {a}, è rimast{A:a/o} sol{A:a/o}.",
    "{a} è {A:vedova/vedovo}, ora…",
];
const NEWS_THEFT_CAUGHT: Pool = &[
    "Hanno beccato {a} a rubare!",
    "{a} {f}? Sorpres{A:a/o} a rubare!",
    "Hai sentito di {a}? {A:Ladra/Ladro}!",
];
const NEWS_THEFT_UNSEEN: Pool = &[
    "Qualcuno ha rubato al mercato.",
    "Al mercato è sparita della merce!",
    "C'è un ladro sul treno, sai?",
];
const NEWS_HELP_GIVEN: Pool = &[
    "{a} ha prestato gettoni a un amico.",
    "Che cuore d'oro, {a}.",
];
const NEWS_HELP_REFUSED: Pool = &[
    "{a} non ha aiutato nessuno, sai?",
    "{a} non dà un gettone a nessuno.",
];
const NEWS_PROTEST_BIRTHS: Pool = &[
    "C'è una protesta per le nascite!",
    "Protestano contro il divieto di figli.",
];
const NEWS_PROTEST_FOOD: Pool = &[
    "Si protesta per le razioni!",
    "Vogliono protestare per il cibo.",
];
const NEWS_CONCESSION_BIRTHS: Pool = &[
    "Hanno ceduto: più nascite!",
    "Le proteste hanno funzionato!",
];
const NEWS_CONCESSION_FOOD: Pool = &[
    "Razioni d'emergenza, finalmente!",
    "L'amministrazione ha ceduto!",
];
const NEWS_SHORTAGE_FOOD: Pool = &[
    "Le mense sono senza razioni!",
    "Non c'è più niente da mangiare!",
];
const NEWS_SHORTAGE: Pool = &[
    "Al mercato non ci sono più {i}!",
    "Finit{I:e/i} {li}, pensa te.",
];
const NEWS_RESTOCKED: Pool = &["Sono tornat{I:e/i} {li}!", "Di nuovo {i} al mercato!"];
const NEWS_BIRTH_DENIED: Pool = &["{ai} hanno negato un figlio.", "Niente bambino per {fam}…"];
const NEWS_CAME_OF_AGE: Pool = &[
    "{a} è {A:diventata/diventato} grande!",
    "{a} ha compiuto diciott'anni!",
];
const NEWS_RETIRED: Pool = &[
    "{a} è {A:andata/andato} in pensione.",
    "{a} ha smesso di lavorare, beat{A:a/o}!",
];
const NEWS_AUSTERITY: Pool = &["Paghe tagliate, hai visto?", "La tesoreria è a secco!"];
const NEWS_PAY_RAISED: Pool = &["Hanno alzato le paghe!", "Più gettoni per tutti, pare."];
const NEWS_PAY_CUT: Pool = &["Hanno abbassato le paghe…", "Paghe più basse, di nuovo."];
const NEWS_VAGUE: Pool = &[
    "Hai sentito le ultime?",
    "Ne succedono di cose, eh?",
    "Sai cos'è successo {al}?",
];

const BIRTH_REPLY: Pool = &[
    "Che bella notizia!",
    "Auguri {ai}!",
    "Un bimbo in più, che gioia!",
];
const BIRTH_FOLLOW: Pool = &[
    "Speriamo che cresca in salute.",
    "{fam} sono al settimo cielo.",
    "Chissà a chi somiglia.",
];
const DEATH_REPLY: Pool = &[
    "Che tristezza…",
    "Pover{A:a/o} {a}… mi dispiace.",
    "Riposi in pace.",
];
const DEATH_FOLLOW: Pool = &[
    "Era una brava persona.",
    "{A:La/Lo} ricorderemo tutti.",
    "Il treno si porta via i migliori.",
];
const COUPLE_REPLY: Pool = &[
    "Che bella coppia!",
    "Era ora, quei due!",
    "Che bella notizia!",
];
const COUPLE_FOLLOW: Pool = &[
    "Stanno proprio bene insieme.",
    "Si vedeva da mesi, dai!",
    "Speriamo che durino.",
];
const WIDOWED_REPLY: Pool = &[
    "Pover{A:a/o} {a}…",
    "Mi dispiace tanto.",
    "Che dolore, davvero.",
];
const WIDOWED_FOLLOW: Pool = &[
    "Bisognerà star{A:le/gli} vicino.",
    "Erano così felici insieme.",
    "Non è giusto, davvero.",
];
const THEFT_REPLY: Pool = &[
    "Non ci posso credere!",
    "{a}? Ma dai!",
    "Chi l'avrebbe mai detto!",
];
const THEFT_CAUGHT_FOLLOW: Pool = &[
    "{A:Le/Gli} hanno dato una bella multa.",
    "Sembrava tanto onest{A:a/o}…",
    "Io i miei gettoni li nascondo.",
];
const THEFT_UNSEEN_FOLLOW: Pool = &[
    "Io i miei gettoni li nascondo.",
    "Chissà chi è stato.",
    "Bisogna stare attenti, ora.",
];
const GENEROUS_REPLY: Pool = &[
    "Che brava persona!",
    "Di gente così ce n'è poca.",
    "Che bel gesto!",
];
const HELP_GIVEN_FOLLOW: Pool = &[
    "Ha un cuore d'oro, davvero.",
    "Se servisse, aiuterei anch'io.",
    "Così si fa, tra vicini.",
];
const HELP_REFUSED_FOLLOW: Pool = &[
    "Con tutti i gettoni che ha!",
    "Io non l'avrei mai fatto.",
    "Poi quando serve a {A:lei/lui}…",
];
const PROTEST_BIRTHS_FOLLOW: Pool = &[
    "Hanno ragione a protestare.",
    "Un figlio non è un lusso.",
    "Speriamo che li ascoltino.",
];
const PROTEST_FOOD_FOLLOW: Pool = &[
    "Con la fame non si scherza.",
    "Hanno ragione, si mangia poco.",
    "Speriamo che li ascoltino.",
];
const CONCESSION_BIRTHS_FOLLOW: Pool = &[
    "Finalmente ci hanno ascoltati!",
    "Più bambini sul treno, che bello.",
    "Protestare è servito.",
];
const CONCESSION_FOOD_FOLLOW: Pool = &[
    "Finalmente si mangia un po'.",
    "Protestare è servito.",
    "Almeno per un po' si mangia.",
];
const SHORTAGE_FOOD_FOLLOW: Pool = &[
    "Speriamo nei cuochi.",
    "Io ho già la pancia vuota.",
    "Bisogna stringere la cinghia.",
];
const SHORTAGE_FOLLOW: Pool = &[
    "E adesso come si fa?",
    "I prezzi saliranno, vedrai.",
    "Speriamo che arrivino presto.",
];
const RESTOCKED_FOLLOW: Pool = &[
    "Era ora!",
    "Adesso si ragiona.",
    "Speriamo che i prezzi scendano.",
];
const BIRTH_DENIED_FOLLOW: Pool = &[
    "Poveretti, ci speravano tanto.",
    "Un figlio non si nega a nessuno.",
    "Chissà come stanno adesso.",
];
const CAME_OF_AGE_FOLLOW: Pool = &[
    "Sembra ieri che era piccol{A:a/o}.",
    "Adesso dovrà trovarsi un lavoro.",
    "Come passa il tempo.",
];
const RETIRED_REPLY: Pool = &[
    "Se l'è proprio meritata!",
    "Che bella notizia!",
    "Beat{A:a/o} {A:lei/lui}!",
];
const RETIRED_FOLLOW: Pool = &[
    "Ha lavorato una vita intera.",
    "Adesso si riposa, finalmente.",
    "Toccherà anche a noi, un giorno.",
];
const AUSTERITY_FOLLOW: Pool = &[
    "Qualcuno ha speso troppo.",
    "Chi paga siamo sempre noi.",
    "Ce la faremo, come sempre.",
];
const PAY_RAISED_FOLLOW: Pool = &[
    "Finalmente qualche gettone in più.",
    "Stasera si festeggia!",
    "Era anche ora.",
];
const PAY_CUT_FOLLOW: Pool = &[
    "Chi paga siamo sempre noi.",
    "E i prezzi invece salgono.",
    "Come si arriva a fine mese?",
];

// Gossip.

const GOSSIP_THEFT: Pool = &[
    "Sai di {a}? L'hanno beccat{A:a/o}!",
    "{a} che ruba… chi l'avrebbe detto!",
    "Ma lo sai che {a} ruba?",
];
const GOSSIP_COUPLE: Pool = &[
    "Hai visto {a} con {o}? Eh eh!",
    "Tra {a} e {o} c'è del tenero!",
    "Pare che {a} si sia sistemat{A:a/o}.",
];
const GOSSIP_BIRTH: Pool = &[
    "{A:La piccola/Il piccolo} {dei} è bellissim{A:a/o}!",
    "Hai visto {a}, {A:la nuova/il nuovo} {dei}?",
];
const GOSSIP_WIDOWED: Pool = &[
    "{a} non esce più di casa…",
    "Hai visto {a}? È distrutt{A:a/o}.",
];
const GOSSIP_STINGY: Pool = &[
    "{a} è tirchi{A:a/o}, te lo dico io.",
    "{a} non aiuta mai nessuno.",
];
const GOSSIP_GENEROUS: Pool = &["{a} ha un cuore d'oro, sai?", "{a} aiuta sempre tutti."];
const GOSSIP_GROWN: Pool = &[
    "{a} è cresciut{A:a/o} tanto!",
    "Hai visto quant'è grande {a}?",
];
const GOSSIP_OPEN_FOND: Pool = &[
    "{a} è proprio una brava persona.",
    "Hai visto {a} ultimamente? In forma!",
    "{a} mi fa sempre ridere.",
    "Sai che {a} lavora benissimo?",
];
const GOSSIP_OPEN_SPITE: Pool = &[
    "{a}? Non mi fido per niente.",
    "{a} si dà un sacco di arie.",
    "Hai visto come ci guarda {a}?",
    "{a} non fa che lamentarsi.",
];
const FOND_REPLY_FRIENDLY: Pool = &[
    "Sì, è proprio in gamba.",
    "Hai ragione, {A:le/gli} voglio bene.",
    "Me l'hanno detto anche altri.",
    "Una persona d'oro, davvero.",
];
const FOND_REPLY_NEUTRAL: Pool = &["Può darsi.", "Non {A:la/lo} conosco bene.", "Sarà, sarà."];
const FOND_REPLY_TENSE: Pool = &[
    "Tu vedi del buono in tutti.",
    "Mah, a me non sembra.",
    "Parli sempre bene di tutti, tu.",
];
const FOND_FOLLOW: Pool = &[
    "Dovremmo invitar{A:la/lo} a cena.",
    "{A:La/Lo} stimo davvero tanto.",
    "Di gente così ce n'è poca.",
];
const FOND_REACT: Pool = &[
    "Hai proprio ragione.",
    "Sono d'accordo, sai?",
    "Eh sì, davvero.",
];
const FOND_INSIST: Pool = &[
    "Dico solo quel che penso.",
    "È la verità, e lo sai.",
    "Un po' di gentilezza, dai.",
];
const SPITE_REPLY_FRIENDLY: Pool = &[
    "Eh, l'ho notato anch'io!",
    "Non dirlo a me…",
    "Hai ragione, stiamone alla larga.",
];
const SPITE_REPLY_NEUTRAL: Pool = &[
    "Mah, non saprei.",
    "Ognuno è fatto a modo suo.",
    "Può essere.",
];
const SPITE_REPLY_TENSE: Pool = &[
    "Parli male di tutti, tu.",
    "Guarda che è mi{A:a/o} amic{A:a/o}.",
    "Pensa a te, piuttosto.",
];
const SPITE_FOLLOW: Pool = &[
    "Io da {A:lei/lui} sto alla larga.",
    "Ha qualcosa che non mi torna.",
    "Non {A:la/lo} sopporto proprio.",
];

// Answers and follow-ups by valence.

const GOOD_REPLY_FRIENDLY: Pool = &[
    "Che bella notizia!",
    "Meraviglioso, davvero!",
    "Finalmente una gioia!",
    "Sono proprio content{S:a/o}!",
];
const GOOD_REPLY_NEUTRAL: Pool = &[
    "Ah, bene.",
    "Buon per loro.",
    "Non lo sapevo.",
    "Meglio così.",
];
const GOOD_REPLY_TENSE: Pool = &[
    "E a me che importa?",
    "Contenti loro…",
    "Sai che novità.",
    "Buon per loro, io no.",
];
const BAD_REPLY_FRIENDLY: Pool = &[
    "Che tristezza…",
    "Mi dispiace tanto.",
    "Povera gente, davvero.",
    "Che brutta notizia.",
];
const BAD_REPLY_NEUTRAL: Pool = &[
    "Eh, capita.",
    "Brutta storia.",
    "Speriamo passi presto.",
    "Già, l'ho sentito.",
];
const BAD_REPLY_TENSE: Pool = &[
    "Cosa ci posso fare io?",
    "Sempre notizie brutte, tu.",
    "Non me ne parlare.",
];
const SCANDAL_REPLY_FRIENDLY: Pool = &[
    "Non ci posso credere!",
    "Ma dai! Davvero?",
    "Chi l'avrebbe mai detto!",
    "Che vergogna!",
];
const SCANDAL_REPLY_NEUTRAL: Pool = &[
    "Mah, succede.",
    "Non mi stupisce.",
    "Ognuno fa quel che può.",
    "Bah.",
];
const SCANDAL_REPLY_TENSE: Pool = &[
    "Sei sicur{L:a/o}? Non ti credo.",
    "Chiacchiere, solo chiacchiere.",
    "Pensa agli affari tuoi.",
    "E tu come lo sai?",
];
const GOOD_FOLLOW: Pool = &[
    "Ci voleva, dopo tanto grigio.",
    "Bisogna festeggiare!",
    "Almeno una cosa va bene.",
    "Lo dicono tutti {al}.",
];
const BAD_FOLLOW: Pool = &[
    "Non so dove andremo a finire.",
    "Speriamo non tocchi a noi.",
    "Ne parlano tutti {al}.",
    "Il treno non perdona.",
];
const SCANDAL_FOLLOW: Pool = &[
    "Me l'ha detto uno che c'era.",
    "Lo sanno già tutti {al}.",
    "Io l'avevo sempre detto.",
    "Che tempi, che tempi.",
];

// Complaints: agree, shrug or push back.

const COMPLAINT_FOOD_OPEN: Pool = &[
    "Razioni sempre più piccole…",
    "Mensa vuota, di nuovo!",
    "Con questa fame chi lavora?",
];
const COMPLAINT_FOOD_FRIENDLY: Pool = &[
    "Hai ragione, si fa la fame.",
    "Lo dico anch'io da giorni!",
    "Tieni duro, cambierà.",
];
const COMPLAINT_FOOD_TENSE: Pool = &[
    "Ringrazia che mangi.",
    "Smettila di lamentarti!",
    "Sempre a brontolare, tu.",
];
const COMPLAINT_FOOD_FOLLOW: Pool = &[
    "Quelli di testa mangiano bene.",
    "Prima o poi protesto, giuro.",
    "Stasera si salta la cena, pare.",
];
const COMPLAINT_BIRTH_OPEN: Pool = &[
    "Ci hanno negato un figlio…",
    "Chi sono loro per dirci di no?",
    "Vogliono decidere pure i figli!",
];
const COMPLAINT_BIRTH_FRIENDLY: Pool = &[
    "Non è giusto, hai ragione.",
    "Mi dispiace tanto, davvero.",
    "Dovete protestare!",
];
const COMPLAINT_BIRTH_NEUTRAL: Pool = &["Sono le regole, purtroppo.", "Che ci vuoi fare."];
const COMPLAINT_BIRTH_TENSE: Pool = &["C'è già poco da mangiare.", "Le regole valgono per tutti."];
const COMPLAINT_BIRTH_FOLLOW: Pool = &[
    "Un figlio non è un lusso.",
    "Prima o poi protesto, giuro.",
    "Non ci arrenderemo.",
];
const COMPLAINT_MONEY_OPEN: Pool = &[
    "Paghe a metà, e noi zitti.",
    "La tesoreria piange, dicono.",
    "Paghe giù e prezzi su, bello.",
    "Con questi gettoni non si vive.",
];
const COMPLAINT_MONEY_FRIENDLY: Pool = &[
    "Hai ragione, è una vergogna.",
    "Lo dico anch'io da giorni!",
    "Siamo in tanti a pensarla così.",
];
const COMPLAINT_MONEY_NEUTRAL: Pool = &["Che ci vuoi fare.", "Tocca stringere la cinghia."];
const COMPLAINT_MONEY_TENSE: Pool = &["Almeno un lavoro ce l'hai.", "Smettila di lamentarti!"];
const COMPLAINT_MONEY_FOLLOW: Pool = &[
    "E i prezzi invece salgono.",
    "Chi paga siamo sempre noi.",
    "Una volta non era così.",
];
const COMPLAINT_SLEEP_OPEN: Pool = &[
    "In questo treno non si dorme mai.",
    "Rumore tutta la notte, sempre.",
];
const COMPLAINT_SLEEP_FOLLOW: Pool = &[
    "Stanotte mi metto i tappi.",
    "Chiederò di cambiare cuccetta.",
];
const COMPLAINT_COLD_OPEN: Pool = &[
    "Fa un freddo cane in cuccetta.",
    "Qui dentro si gela, oggi.",
];
const COMPLAINT_COLD_FOLLOW: Pool = &[
    "Mi servirebbe un vestito nuovo.",
    "Dormo con tre coperte, pensa.",
];
const COMPLAINT_TEA_OPEN: Pool = &["Il tè sa di ferro, di nuovo.", "Questo tè è acqua sporca."];
const COMPLAINT_TEA_FOLLOW: Pool = &["Chissà che acqua usano.", "Una volta il tè era buono."];
const COMPLAINT_LIFE_OPEN: Pool = &[
    "Qui nessuno ascolta nessuno.",
    "Si lavora e basta, che vita.",
];
const COMPLAINT_LIFE_FOLLOW: Pool = &["Una volta non era così.", "Qualcuno dovrà pur dirlo."];
const COMPLAINT_REPLY_FRIENDLY: Pool = &[
    "Hai ragione, non è giusto.",
    "Lo dico anch'io da sempre!",
    "Siamo in tanti a pensarla così.",
];
const COMPLAINT_REPLY_NEUTRAL: Pool = &[
    "Eh, è così.",
    "Che ci vuoi fare.",
    "Sempre la stessa storia.",
    "Lamentarsi non serve.",
];
const COMPLAINT_REPLY_TENSE: Pool = &[
    "Smettila di lamentarti!",
    "Sempre a brontolare, tu.",
    "Lavora e stai zitt{L:a/o}.",
];

// Reactions to the follow-up, by mood.

const REACT_GOOD_FRIENDLY: Pool = &[
    "Eh già, che bello!",
    "Hai proprio ragione.",
    "Proprio così!",
];
const REACT_GOOD_NEUTRAL: Pool = &["Già.", "Meglio così.", "Vedremo."];
const REACT_BAD_FRIENDLY: Pool = &[
    "Hai ragione, purtroppo.",
    "Speriamo in bene, dai.",
    "Già, che tristezza.",
];
const REACT_BAD_NEUTRAL: Pool = &["Già.", "Eh, è così.", "Speriamo bene."];
const REACT_SCANDAL_FRIENDLY: Pool =
    &["Pensa te!", "Incredibile, davvero.", "Chi l'avrebbe detto!"];
const REACT_GRIPE_FRIENDLY: Pool = &[
    "Tieni duro, {n}.",
    "Ti capisco, davvero.",
    "Hai ragione, purtroppo.",
];
const REACT_GRIPE_NEUTRAL: Pool = &["Eh, è così.", "Già.", "Che ci vuoi fare."];
const REACT_GRIPE_TENSE: Pool = &[
    "Sempre a brontolare, tu.",
    "Dici sempre così.",
    "Sì, sì, certo.",
];
const REACT_NEUTRAL_FRIENDLY: Pool = &["Eh già!", "Hai proprio ragione.", "Ahah, vero!"];
const REACT_NEUTRAL_NEUTRAL: Pool = &["Già.", "Può darsi.", "Sarà.", "Mah, chissà."];
const REACT_TENSE: Pool = &[
    "E allora?",
    "Non mi interessa.",
    "Sì, sì, certo.",
    "Dici sempre così.",
    "Mah, figurati.",
];
const REACT_ANSWER: [Pool; 3] = [
    &[
        "Allora è proprio vero…",
        "Ah, ecco. Pensa…",
        "Se lo dicono, sarà vero.",
    ],
    &["Ah, ecco.", "Capito."],
    REACT_TENSE,
];
const REACT_SECRET: [Pool; 3] = [
    &["Muto come un pesce.", "Tranquill{L:a/o}, non dico niente."],
    &["Va bene, va bene.", "Sì, sì."],
    &["Lo sa già mezzo treno.", "E allora perché me lo dici?"],
];
const REACT_LAMENT: [Pool; 3] = [
    &["Dai, non essere pessimista!", "Su, non buttarti giù."],
    &["Mah, vedremo.", "Può darsi."],
    &["Sempre a brontolare, tu.", "Che lagna."],
];
const REACT_SHY: [Pool; 3] = [
    &["Tranquill{L:a/o}, {n}.", "Non ti preoccupare."],
    &["Mh.", "Va be'."],
    &["Parla, dai!", "Mah."],
];

// Personality.

const SHY: Pool = &["…", "Ehm… sì.", "Mh-mh.", "S-sì, certo.", "…già."];
const SHY_GOOD: Pool = &["Oh… che bello.", "S-sì… è vero.", "…davvero? Bene."];
const SHY_BAD: Pool = &["Oh… mi dispiace.", "…che tristezza.", "…"];
const SHY_GRIPE: Pool = &["Oh… mi dispiace.", "…ti capisco.", "…"];
const SHY_SCANDAL: Pool = &["Oh… davvero?", "…non ci credo.", "…"];
const SHY_FOLLOW: Pool = &["…", "Ehm… niente.", "Scusa, parlo poco."];
const SHY_CLOSE: Pool = &["C-ciao…", "…a dopo.", "Beh… ciao.", "Scusa, devo andare…"];
const CURT: Pool = &["Mh.", "Sì, sì…", "Bah.", "Se lo dici tu."];
const CURT_GOOD: Pool = &["Mh. Bene.", "Bah. Meglio così.", "Mh."];
const CURT_BAD: Pool = &["Capita.", "Mh.", "Brutta storia."];
const CURT_SCANDAL: Pool = &["Bah.", "Non mi stupisce.", "Tsk."];
const CURT_GRIPE: Pool = &["Capita a tutti.", "Mh.", "E allora?"];
const GRUMPY_CLOSE: Pool = &["Ciao.", "Vado.", "Basta così."];
const CURIOUS: Pool = &[
    "Davvero? Chi te l'ha detto?",
    "Ma dai! E tu come lo sai?",
    "Come l'hai saputo?",
    "Davvero? Da chi l'hai saputo?",
];
const ANSWER: Pool = &[
    "L'ho saputo stamattina {al}.",
    "Me l'ha detto una vicina.",
    "Lo dicono tutti {al}.",
    "L'ho sentito in mensa.",
    "Me l'hanno detto stamattina.",
];
const ANSWER_SCANDAL: Pool = &[
    "Me l'ha detto uno che c'era.",
    "Lo sanno già tutti {al}.",
    "L'ho sentito {al}, giuro.",
];
const KIND_BAD: Pool = &["Che dolore… mi dispiace.", "Se serve una mano, ci sono."];
const KIND_GRIPE: Pool = &[
    "Ti capisco, davvero.",
    "Se ti serve, ci sono.",
    "Non preoccuparti, {n}.",
];
const PEACEMAKER: Pool = &["Dai, non litighiamo.", "Scusa, non volevo."];
const CHEERFUL_GOOD: Pool = &["Ahah, che bello!", "Evviva!", "Questa sì che è bella!"];
const CHEERFUL_GRIPE: Pool = &["Dai, che ce la facciamo!", "Su con la vita, {n}!"];
const CHEERFUL_SMALL: Pool = &["Ahah, vero!", "Ahah, pensa te!", "Tu sì che mi fai ridere!"];
const CHEERFUL_HOW: Pool = &["Alla grande, grazie!", "Benissimo! Che bella giornata!"];
const CHEERFUL_WORK: Pool = &["Evviva! Brav{L:a/o}!", "Ahah, grande!"];
const CHEERFUL_FOND: Pool = &["Ahah, è vero!", "Sì, è una forza!"];
const LAMENT_GOOD: Pool = &[
    "Durerà poco, vedrai.",
    "A me mai niente, però.",
    "Speriamo che duri, almeno.",
];
const LAMENT_BAD: Pool = &[
    "Sempre peggio, sempre peggio…",
    "Tocca sempre ai migliori.",
    "Una disgrazia dopo l'altra.",
];
const LAMENT_GRIPE: Pool = &[
    "Sempre peggio, sempre peggio…",
    "A me va sempre tutto storto.",
    "Non se ne può più.",
    "E nessuno fa niente.",
];
const LAMENT_SCANDAL: Pool = &["Non ci si può fidare di nessuno.", "Che tempi, che tempi."];
const LAMENT_NEUTRAL: Pool = &["Tanto domani è peggio.", "Qui dentro è tutto grigio."];
const SECRET: Pool = &["Ma non dirlo in giro, eh.", "Resti tra noi, mi raccomando."];
const INSIST_GOOD: Pool = &[
    "Potresti anche esserne content{L:a/o}.",
    "Sei sempre così acid{L:a/o}.",
    "Una bella notizia, per una volta!",
];
const INSIST_BAD: Pool = &[
    "Un po' di cuore, almeno!",
    "Potrebbe capitare anche a te.",
    "Non c'è niente da ridere.",
];
const INSIST_SCANDAL: Pool = &[
    "Lo sanno tutti, informati.",
    "Guarda che è vero.",
    "Io dico solo quel che ho sentito.",
];
const INSIST_GRIPE: Pool = &[
    "Almeno ascoltami, no?",
    "Dico solo come stanno le cose.",
    "Tu non ti lamenti mai, eh?",
];
const INSIST_NEUTRAL: Pool = &[
    "Volevo solo fare due chiacchiere.",
    "Che caratterino, eh.",
    "Scusa tanto, eh.",
];
const CHATTY_CLOSE: Pool = &[
    "Parlo troppo, eh? Scusa!",
    "Ti ho tenut{L:a/o} fin troppo, ciao!",
];

// Goodbyes.

const CLOSE_FRIENDLY: Pool = &[
    "Ci vediamo dopo!",
    "A presto, {n}!",
    "Salutami tutti!",
    "{bg}, {n}!",
    "Alla prossima!",
    "Un giorno ti offro un tè.",
];
const CLOSE_SOMBER: Pool = &[
    "Stammi bene, {n}.",
    "Abbi cura di te, {n}.",
    "Ci vediamo, {n}.",
    "Forza, eh. A presto.",
];
const CLOSE_THANKS: Pool = &[
    "Grazie, {n}. A presto.",
    "Meno male che ci sei tu.",
    "Grazie, mi hai tirat{S:a/o} su.",
];
const CLOSE_NEUTRAL: Pool = &[
    "Va be', ci vediamo.",
    "Io vado. Ciao.",
    "Ciao, {n}.",
    "Alla prossima.",
    "Devo andare.",
];
const CLOSE_TENSE: Pool = &[
    "Lasciamo perdere.",
    "Non ne voglio parlare.",
    "Basta, me ne vado.",
    "Fai come ti pare.",
];
const CLOSE_BACK_FRIENDLY: Pool = &["Ciao, {n}! A presto.", "A dopo!", "Stammi bene!", "{bg}!"];
const CLOSE_BACK_NEUTRAL: Pool = &["Ciao.", "Sì, ciao.", "A dopo."];
const CLOSE_BACK_TENSE: Pool = &["Ecco, brav{L:a/o}.", "Finalmente.", "Sì, vai pure."];
const CLOSE_PARTNER: Pool = &[
    "A stasera, amore.",
    "Ti voglio bene. Ciao!",
    "Ciao, amore mio.",
];
const CLOSE_TO_KID: Pool = &["Fai {L:la brava/il bravo}, eh!", "Ciao, tesoro."];
const CLOSE_TO_CHILD: Pool = &["Ciao, tesoro.", "Riguardati, {n}."];
const CLOSE_TO_PARENT: Pool = &["Ciao, {L:mamma/papà}!", "A dopo, {L:mamma/papà}!"];
const CLOSE_SIBLING: Pool = &["Ciao, {L:sorella/fratello}!", "Ci vediamo, {n}."];
const KID_CLOSE: Pool = &["Ciao ciao!", "Vado a giocare!", "Ciao, {n}!"];

// Children.

pub(super) const BABBLE: Pool = &["Gu-gu!", "Ah-ah!", "Bla!", "Pappa!", "Mmmh!", "Eeeh!"];
const KID: Pool = &[
    "Posso andare a giocare?",
    "Uffa, che noia!",
    "Guarda cosa so fare!",
    "Ho fame!",
    "Mi racconti una storia?",
    "Perché il treno non si ferma?",
];
const REACT_TO_KID: Pool = &["Dopo, {n}, dopo.", "Sì, sì, tesoro.", "Eh, lo so, {n}."];
const KID_GOOD: Pool = &["Che bello!", "Evviva!"];
const KID_BAD: Pool = &["Oh no…", "Che brutto…"];
const KID_SCANDAL: Pool = &["Davvero?!", "Ooh!"];
const KID_GRIPE: Pool = &["Anch'io!", "Uffa, sì.", "Poverin{L:a/o}!"];
const KID_TENSE: Pool = &["Non mi interessa!", "Uffa, che noia!", "E allora?"];
const KID_NEUTRAL: Pool = &["Sì!", "Mh, sì.", "Boh!"];
const TO_BABY_OPEN: Pool = &[
    "Ma che bell{L:a/o} che sei!",
    "Chi è {L:la piccola/il piccolo}?",
    "Cucù, {n}!",
    "Ciao, {n}! Ciao!",
];
const TO_BABY: Pool = &[
    "Ma che bell{L:a/o} che sei!",
    "Chi è {L:la piccola/il piccolo}?",
    "Cucù, {n}!",
    "Fai un sorriso, {n}!",
    "Guarda che manine!",
    "Sì, sì, hai ragione tu!",
    "Ma quanto sei cresciut{L:a/o}!",
];

// ----------------------------------------------------------------------
// Threads
// ----------------------------------------------------------------------

const WORK_ASK: Thread = Thread {
    mood: Mood::Neutral,
    open: WORK_ASK_OPEN,
    reply: [WORK_ASKED_FRIENDLY, WORK_ASKED_NEUTRAL, WORK_ASKED_TENSE],
    follow: WORK_ASK_FOLLOW,
    react: Some([REACT_NEUTRAL_FRIENDLY, REACT_NEUTRAL_NEUTRAL, REACT_TENSE]),
    insist: None,
};
const NEEDS_HUNGER: Thread = Thread {
    mood: Mood::Gripe,
    open: NEEDS_OPEN_HUNGER,
    reply: [
        NEEDS_HUNGER_FRIENDLY,
        NEEDS_HUNGER_NEUTRAL,
        NEEDS_REPLY_TENSE,
    ],
    follow: NEEDS_HUNGER_FOLLOW,
    react: None,
    insist: None,
};
const NEEDS_TIRED: Thread = Thread {
    mood: Mood::Gripe,
    open: NEEDS_OPEN_TIRED,
    reply: [NEEDS_TIRED_FRIENDLY, NEEDS_TIRED_NEUTRAL, NEEDS_REPLY_TENSE],
    follow: NEEDS_TIRED_FOLLOW,
    react: None,
    insist: None,
};
const NEEDS_LONELY: Thread = Thread {
    mood: Mood::Neutral,
    open: NEEDS_OPEN_LONELY,
    reply: [
        NEEDS_LONELY_FRIENDLY,
        NEEDS_LONELY_NEUTRAL,
        NEEDS_LONELY_TENSE,
    ],
    follow: NEEDS_LONELY_FOLLOW,
    react: Some([NEEDS_LONELY_REACT, FAMILY_REACT_NEUTRAL, REACT_TENSE]),
    insist: None,
};
const FAMILY_REACT: Option<[Pool; 3]> =
    Some([FAMILY_REACT_FRIENDLY, FAMILY_REACT_NEUTRAL, REACT_TENSE]);
const PARTNER_INVITE: Thread = Thread {
    mood: Mood::Neutral,
    open: PARTNER_INVITE_OPEN,
    reply: [
        PARTNER_INVITE_FRIENDLY,
        PARTNER_INVITE_NEUTRAL,
        INVITE_TENSE,
    ],
    follow: PARTNER_INVITE_FOLLOW,
    react: Some([PARTNER_INVITE_REACT, FAMILY_REACT_NEUTRAL, REACT_TENSE]),
    insist: None,
};
const PARTNER_ASK: Thread = Thread {
    mood: Mood::Neutral,
    open: PARTNER_ASK_OPEN,
    reply: [PARTNER_ASK_FRIENDLY, ASKED_NEUTRAL, ASKED_TENSE],
    follow: TOGETHER_FOLLOW,
    react: FAMILY_REACT,
    insist: None,
};
const PARTNER_LOVE: Thread = Thread {
    mood: Mood::Neutral,
    open: PARTNER_LOVE_OPEN,
    reply: [
        PARTNER_LOVE_FRIENDLY,
        PARTNER_LOVE_NEUTRAL,
        PARTNER_LOVE_TENSE,
    ],
    follow: TOGETHER_FOLLOW,
    react: FAMILY_REACT,
    insist: None,
};
const TO_CHILD_ASK: Thread = Thread {
    mood: Mood::Neutral,
    open: TO_CHILD_ASK_OPEN,
    reply: [
        CHILD_ANSWER_FRIENDLY,
        CHILD_ANSWER_NEUTRAL,
        CHILD_ANSWER_TENSE,
    ],
    follow: TO_CHILD_ASK_FOLLOW,
    react: Some([TO_CHILD_ASK_REACT, FAMILY_REACT_NEUTRAL, REACT_TENSE]),
    insist: None,
};
const TO_CHILD_CARE: Thread = Thread {
    mood: Mood::Neutral,
    open: TO_CHILD_CARE_OPEN,
    reply: [
        TO_CHILD_CARE_FRIENDLY,
        TO_CHILD_CARE_NEUTRAL,
        TO_CHILD_CARE_TENSE,
    ],
    follow: TO_CHILD_CARE_FOLLOW,
    react: Some([TO_CHILD_CARE_REACT, FAMILY_REACT_NEUTRAL, REACT_TENSE]),
    insist: None,
};
const KID_REQUEST: Thread = Thread {
    mood: Mood::Neutral,
    open: KID_REQUEST_OPEN,
    reply: [KID_REQUEST_FRIENDLY, KID_REQUEST_NEUTRAL, KID_REQUEST_TENSE],
    follow: KID_REQUEST_FOLLOW,
    react: Some([KID_REQUEST_REACT, FAMILY_REACT_NEUTRAL, REACT_TENSE]),
    insist: None,
};
const KID_TELL: Thread = Thread {
    mood: Mood::Neutral,
    open: KID_TELL_OPEN,
    reply: [KID_TELL_FRIENDLY, KID_TELL_NEUTRAL, KID_TELL_TENSE],
    follow: KID_TELL_FOLLOW,
    react: Some([KID_TELL_REACT, KID_TELL_REACT_NEUTRAL, REACT_TENSE]),
    insist: None,
};
const TO_PARENT_ASK: Thread = Thread {
    mood: Mood::Neutral,
    open: TO_PARENT_ASK_OPEN,
    reply: [
        PARENT_ANSWER_FRIENDLY,
        PARENT_ANSWER_NEUTRAL,
        PARENT_ANSWER_TENSE,
    ],
    follow: TO_PARENT_ASK_FOLLOW,
    react: Some([TO_PARENT_ASK_REACT, FAMILY_REACT_NEUTRAL, REACT_TENSE]),
    insist: None,
};
const SIBLING_MEMORY: Thread = Thread {
    mood: Mood::Neutral,
    open: SIBLING_MEMORY_OPEN,
    reply: [
        SIBLING_MEMORY_FRIENDLY,
        SIBLING_MEMORY_NEUTRAL,
        SIBLING_MEMORY_TENSE,
    ],
    follow: SIBLING_MEMORY_FOLLOW,
    react: Some([SIBLING_MEMORY_REACT, FAMILY_REACT_NEUTRAL, REACT_TENSE]),
    insist: None,
};
const SIBLING_ASK: Thread = Thread {
    mood: Mood::Neutral,
    open: SIBLING_ASK_OPEN,
    reply: [SIBLING_ASK_FRIENDLY, ASKED_NEUTRAL, ASKED_TENSE],
    follow: TOGETHER_FOLLOW,
    react: FAMILY_REACT,
    insist: None,
};
const SIBLING_FOLKS: Thread = Thread {
    mood: Mood::Neutral,
    open: SIBLING_FOLKS_OPEN,
    reply: [FOLKS_FRIENDLY, ASKED_NEUTRAL, ASKED_TENSE],
    follow: FOLKS_FOLLOW,
    react: FAMILY_REACT,
    insist: None,
};
const FRIEND_FOLKS: Thread = Thread {
    mood: Mood::Neutral,
    open: FOLKS_OPEN,
    reply: [FOLKS_FRIENDLY, ASKED_NEUTRAL, FOLKS_TENSE],
    follow: FOLKS_FOLLOW,
    react: Some([FOLKS_REACT, FAMILY_REACT_NEUTRAL, REACT_TENSE]),
    insist: None,
};
const GOSSIP_FOND: Thread = Thread {
    mood: Mood::Good,
    open: GOSSIP_OPEN_FOND,
    reply: [FOND_REPLY_FRIENDLY, FOND_REPLY_NEUTRAL, FOND_REPLY_TENSE],
    follow: FOND_FOLLOW,
    react: Some([FOND_REACT, REACT_NEUTRAL_NEUTRAL, REACT_TENSE]),
    insist: Some(FOND_INSIST),
};
const GOSSIP_SPITE: Thread = Thread {
    mood: Mood::Scandal,
    open: GOSSIP_OPEN_SPITE,
    reply: [SPITE_REPLY_FRIENDLY, SPITE_REPLY_NEUTRAL, SPITE_REPLY_TENSE],
    follow: SPITE_FOLLOW,
    react: None,
    insist: None,
};
const COMPLAINT_FOOD: Thread = Thread {
    mood: Mood::Gripe,
    open: COMPLAINT_FOOD_OPEN,
    reply: [
        COMPLAINT_FOOD_FRIENDLY,
        COMPLAINT_REPLY_NEUTRAL,
        COMPLAINT_FOOD_TENSE,
    ],
    follow: COMPLAINT_FOOD_FOLLOW,
    react: None,
    insist: None,
};
const COMPLAINT_BIRTH: Thread = Thread {
    mood: Mood::Gripe,
    open: COMPLAINT_BIRTH_OPEN,
    reply: [
        COMPLAINT_BIRTH_FRIENDLY,
        COMPLAINT_BIRTH_NEUTRAL,
        COMPLAINT_BIRTH_TENSE,
    ],
    follow: COMPLAINT_BIRTH_FOLLOW,
    react: None,
    insist: None,
};
const COMPLAINT_MONEY: Thread = Thread {
    mood: Mood::Gripe,
    open: COMPLAINT_MONEY_OPEN,
    reply: [
        COMPLAINT_MONEY_FRIENDLY,
        COMPLAINT_MONEY_NEUTRAL,
        COMPLAINT_MONEY_TENSE,
    ],
    follow: COMPLAINT_MONEY_FOLLOW,
    react: None,
    insist: None,
};
const COMPLAINT_SLEEP: Thread = Thread {
    mood: Mood::Gripe,
    open: COMPLAINT_SLEEP_OPEN,
    reply: [
        COMPLAINT_REPLY_FRIENDLY,
        COMPLAINT_REPLY_NEUTRAL,
        COMPLAINT_REPLY_TENSE,
    ],
    follow: COMPLAINT_SLEEP_FOLLOW,
    react: None,
    insist: None,
};
const COMPLAINT_COLD: Thread = Thread {
    open: COMPLAINT_COLD_OPEN,
    follow: COMPLAINT_COLD_FOLLOW,
    ..COMPLAINT_SLEEP
};
const COMPLAINT_TEA: Thread = Thread {
    open: COMPLAINT_TEA_OPEN,
    follow: COMPLAINT_TEA_FOLLOW,
    ..COMPLAINT_SLEEP
};
const COMPLAINT_LIFE: Thread = Thread {
    open: COMPLAINT_LIFE_OPEN,
    follow: COMPLAINT_LIFE_FOLLOW,
    ..COMPLAINT_SLEEP
};

/// Every template pool, for [`template_count`] and the tests.
const ALL_POOLS: &[Pool] = &[
    GREET_FRIENDLY,
    GREET_NEUTRAL,
    GREET_TENSE,
    GREET_BACK_FRIENDLY,
    GREET_BACK_NEUTRAL,
    GREET_PARTNER,
    GREET_BACK_PARTNER,
    GREET_CHILD,
    GREET_PARENT,
    GREET_SIBLING,
    SHY_GREET,
    GRUMPY_GREET,
    GRUMPY_GREET_BACK,
    CHEERFUL_GREET,
    SMALL_OPEN_FRIENDLY,
    SMALL_OPEN_NEUTRAL,
    SMALL_OPEN_TENSE,
    SMALL_REPLY_FRIENDLY,
    SMALL_REPLY_NEUTRAL,
    SMALL_REPLY_TENSE,
    SMALL_REMARK,
    REMARK_REACT_FRIENDLY,
    WORK_GOOD_CONTADINO,
    WORK_BAD_CONTADINO,
    WORK_GOOD_CUOCO,
    WORK_BAD_CUOCO,
    WORK_GOOD_OPERAIO,
    WORK_BAD_OPERAIO,
    WORK_GOOD_MERCANTE,
    WORK_BAD_MERCANTE,
    WORK_GOOD_REPLY_FRIENDLY,
    WORK_GOOD_REPLY_NEUTRAL,
    WORK_GOOD_REPLY_TENSE,
    WORK_GOOD_FOLLOW,
    WORK_BAD_REPLY_FRIENDLY,
    WORK_BAD_REPLY_NEUTRAL,
    WORK_BAD_REPLY_TENSE,
    WORK_BAD_FOLLOW,
    WORK_ASK_OPEN,
    WORK_ASKED_FRIENDLY,
    WORK_ASKED_NEUTRAL,
    WORK_ASKED_TENSE,
    WORK_ASK_FOLLOW,
    NEEDS_OPEN_HUNGER,
    NEEDS_HUNGER_FRIENDLY,
    NEEDS_HUNGER_NEUTRAL,
    NEEDS_HUNGER_FOLLOW,
    NEEDS_OPEN_TIRED,
    NEEDS_TIRED_FRIENDLY,
    NEEDS_TIRED_NEUTRAL,
    NEEDS_TIRED_FOLLOW,
    NEEDS_REPLY_TENSE,
    NEEDS_OPEN_LONELY,
    NEEDS_LONELY_FRIENDLY,
    NEEDS_LONELY_NEUTRAL,
    NEEDS_LONELY_TENSE,
    NEEDS_LONELY_FOLLOW,
    NEEDS_LONELY_REACT,
    PARTNER_INVITE_OPEN,
    PARTNER_INVITE_FRIENDLY,
    PARTNER_INVITE_NEUTRAL,
    INVITE_TENSE,
    PARTNER_INVITE_FOLLOW,
    PARTNER_ASK_OPEN,
    PARTNER_ASK_FRIENDLY,
    ASKED_NEUTRAL,
    ASKED_TENSE,
    TOGETHER_FOLLOW,
    PARTNER_LOVE_OPEN,
    PARTNER_LOVE_FRIENDLY,
    PARTNER_LOVE_NEUTRAL,
    PARTNER_LOVE_TENSE,
    TO_CHILD_ASK_OPEN,
    CHILD_ANSWER_FRIENDLY,
    CHILD_ANSWER_NEUTRAL,
    CHILD_ANSWER_TENSE,
    TO_CHILD_ASK_FOLLOW,
    TO_CHILD_ASK_REACT,
    TO_CHILD_CARE_OPEN,
    TO_CHILD_CARE_FRIENDLY,
    TO_CHILD_CARE_NEUTRAL,
    TO_CHILD_CARE_TENSE,
    TO_CHILD_CARE_FOLLOW,
    TO_CHILD_CARE_REACT,
    KID_REQUEST_OPEN,
    KID_REQUEST_FRIENDLY,
    KID_REQUEST_NEUTRAL,
    KID_REQUEST_TENSE,
    KID_REQUEST_FOLLOW,
    KID_REQUEST_REACT,
    KID_TELL_OPEN,
    KID_TELL_FRIENDLY,
    KID_TELL_NEUTRAL,
    KID_TELL_TENSE,
    KID_TELL_FOLLOW,
    KID_TELL_REACT,
    TO_PARENT_ASK_OPEN,
    PARENT_ANSWER_FRIENDLY,
    PARENT_ANSWER_NEUTRAL,
    PARENT_ANSWER_TENSE,
    TO_PARENT_ASK_FOLLOW,
    TO_PARENT_ASK_REACT,
    SIBLING_MEMORY_OPEN,
    SIBLING_MEMORY_FRIENDLY,
    SIBLING_MEMORY_NEUTRAL,
    SIBLING_MEMORY_TENSE,
    SIBLING_MEMORY_FOLLOW,
    SIBLING_MEMORY_REACT,
    SIBLING_ASK_OPEN,
    SIBLING_ASK_FRIENDLY,
    FOLKS_OPEN,
    SIBLING_FOLKS_OPEN,
    FOLKS_FRIENDLY,
    FOLKS_TENSE,
    FOLKS_FOLLOW,
    PARTNER_INVITE_REACT,
    KID_TELL_REACT_NEUTRAL,
    FOLKS_REACT,
    FAMILY_REACT_FRIENDLY,
    FAMILY_REACT_NEUTRAL,
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
    BIRTH_REPLY,
    BIRTH_FOLLOW,
    DEATH_REPLY,
    DEATH_FOLLOW,
    COUPLE_REPLY,
    COUPLE_FOLLOW,
    WIDOWED_REPLY,
    WIDOWED_FOLLOW,
    THEFT_REPLY,
    THEFT_CAUGHT_FOLLOW,
    THEFT_UNSEEN_FOLLOW,
    GENEROUS_REPLY,
    HELP_GIVEN_FOLLOW,
    HELP_REFUSED_FOLLOW,
    PROTEST_BIRTHS_FOLLOW,
    PROTEST_FOOD_FOLLOW,
    CONCESSION_BIRTHS_FOLLOW,
    CONCESSION_FOOD_FOLLOW,
    SHORTAGE_FOOD_FOLLOW,
    SHORTAGE_FOLLOW,
    RESTOCKED_FOLLOW,
    BIRTH_DENIED_FOLLOW,
    CAME_OF_AGE_FOLLOW,
    RETIRED_REPLY,
    RETIRED_FOLLOW,
    AUSTERITY_FOLLOW,
    PAY_RAISED_FOLLOW,
    PAY_CUT_FOLLOW,
    GOSSIP_THEFT,
    GOSSIP_COUPLE,
    GOSSIP_BIRTH,
    GOSSIP_WIDOWED,
    GOSSIP_STINGY,
    GOSSIP_GENEROUS,
    GOSSIP_GROWN,
    GOSSIP_OPEN_FOND,
    GOSSIP_OPEN_SPITE,
    FOND_REPLY_FRIENDLY,
    FOND_REPLY_NEUTRAL,
    FOND_REPLY_TENSE,
    FOND_FOLLOW,
    FOND_REACT,
    FOND_INSIST,
    SPITE_REPLY_FRIENDLY,
    SPITE_REPLY_NEUTRAL,
    SPITE_REPLY_TENSE,
    SPITE_FOLLOW,
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
    COMPLAINT_FOOD_OPEN,
    COMPLAINT_FOOD_FRIENDLY,
    COMPLAINT_FOOD_TENSE,
    COMPLAINT_FOOD_FOLLOW,
    COMPLAINT_BIRTH_OPEN,
    COMPLAINT_BIRTH_FRIENDLY,
    COMPLAINT_BIRTH_NEUTRAL,
    COMPLAINT_BIRTH_TENSE,
    COMPLAINT_BIRTH_FOLLOW,
    COMPLAINT_MONEY_OPEN,
    COMPLAINT_MONEY_FRIENDLY,
    COMPLAINT_MONEY_NEUTRAL,
    COMPLAINT_MONEY_TENSE,
    COMPLAINT_MONEY_FOLLOW,
    COMPLAINT_SLEEP_OPEN,
    COMPLAINT_SLEEP_FOLLOW,
    COMPLAINT_COLD_OPEN,
    COMPLAINT_COLD_FOLLOW,
    COMPLAINT_TEA_OPEN,
    COMPLAINT_TEA_FOLLOW,
    COMPLAINT_LIFE_OPEN,
    COMPLAINT_LIFE_FOLLOW,
    COMPLAINT_REPLY_FRIENDLY,
    COMPLAINT_REPLY_NEUTRAL,
    COMPLAINT_REPLY_TENSE,
    REACT_GOOD_FRIENDLY,
    REACT_GOOD_NEUTRAL,
    REACT_BAD_FRIENDLY,
    REACT_BAD_NEUTRAL,
    REACT_SCANDAL_FRIENDLY,
    REACT_GRIPE_FRIENDLY,
    REACT_GRIPE_NEUTRAL,
    REACT_GRIPE_TENSE,
    REACT_NEUTRAL_FRIENDLY,
    REACT_NEUTRAL_NEUTRAL,
    REACT_TENSE,
    REACT_ANSWER[0],
    REACT_ANSWER[1],
    REACT_SECRET[0],
    REACT_SECRET[1],
    REACT_SECRET[2],
    REACT_LAMENT[0],
    REACT_LAMENT[1],
    REACT_LAMENT[2],
    REACT_SHY[0],
    REACT_SHY[1],
    REACT_SHY[2],
    SHY,
    SHY_GOOD,
    SHY_BAD,
    SHY_GRIPE,
    SHY_SCANDAL,
    SHY_FOLLOW,
    SHY_CLOSE,
    CURT,
    CURT_GOOD,
    CURT_BAD,
    CURT_SCANDAL,
    CURT_GRIPE,
    GRUMPY_CLOSE,
    CURIOUS,
    ANSWER,
    ANSWER_SCANDAL,
    KIND_BAD,
    KIND_GRIPE,
    PEACEMAKER,
    CHEERFUL_GOOD,
    CHEERFUL_GRIPE,
    CHEERFUL_SMALL,
    CHEERFUL_HOW,
    CHEERFUL_WORK,
    CHEERFUL_FOND,
    LAMENT_GOOD,
    LAMENT_BAD,
    LAMENT_GRIPE,
    LAMENT_SCANDAL,
    LAMENT_NEUTRAL,
    SECRET,
    INSIST_GOOD,
    INSIST_BAD,
    INSIST_SCANDAL,
    INSIST_GRIPE,
    INSIST_NEUTRAL,
    CHATTY_CLOSE,
    CLOSE_FRIENDLY,
    CLOSE_SOMBER,
    CLOSE_THANKS,
    CLOSE_NEUTRAL,
    CLOSE_TENSE,
    CLOSE_BACK_FRIENDLY,
    CLOSE_BACK_NEUTRAL,
    CLOSE_BACK_TENSE,
    CLOSE_PARTNER,
    CLOSE_TO_KID,
    CLOSE_TO_CHILD,
    CLOSE_TO_PARENT,
    CLOSE_SIBLING,
    KID_CLOSE,
    BABBLE,
    KID,
    REACT_TO_KID,
    KID_GOOD,
    KID_BAD,
    KID_SCANDAL,
    KID_GRIPE,
    KID_NEUTRAL,
    KID_TENSE,
    TO_BABY_OPEN,
    TO_BABY,
];

/// A piece of news told by `speaker` to the player (whose first name is
/// `listener`) in the chat ([`crate::chat`]): the same openers as the
/// conversations between NPCs, as news or, with `gossip`, as gossip. `place`
/// is where it happened (or where they are). None if no template fits.
#[allow(clippy::too_many_arguments)]
pub(crate) fn news_line(
    news: News,
    about: Option<&Subject>,
    gossip: bool,
    speaker: Voice,
    listener: &str,
    place: &str,
    hour: u32,
    seed: u64,
) -> Option<String> {
    let player = Voice {
        id: NpcId(u32::MAX),
        first: listener,
        sex: Sex::Male,
        age: 30,
        job: None,
        personality: Personality::default(),
    };
    let script = Script {
        topic: if gossip { Topic::Gossip } else { Topic::News },
        tone: Tone::Friendly,
        news: Some(news),
        a: speaker,
        b: player,
        tie: None,
        about,
        fond: true,
        need: Need::Loneliness,
        place,
        hour,
    };
    let pool = if gossip && about.is_some() {
        gossip_open(news)
    } else {
        news_open(news, about.is_some())
    };
    pick(pool, &script, Side::A, &[], &mut Mix(seed)).map(|(_, text)| text)
}

/// Number of distinct line templates.
pub fn template_count() -> usize {
    let mut all: Vec<&str> = ALL_POOLS.iter().flat_map(|p| p.iter().copied()).collect();
    all.sort_unstable();
    all.dedup();
    all.len()
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
