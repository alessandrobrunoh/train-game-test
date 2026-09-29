//! Testo della chat del giocatore ([`crate::chat`]): le frasi del giocatore
//! (risposte suggerite) e le risposte degli NPC.
//!
//! An answer depends on the [`Intent`], the [`Band`] of the NPC's affinity
//! with the player (every intent has templates for all three), the NPC's
//! personality ([`Temper`]: the grumpy cut short, the shy stammer, the
//! cheerful cheer, the kind soften an insult, complainers complain), its
//! state (hungry, tired, on shift: a short remark after the answer), its age
//! (babies babble, kids talk like kids) and a [`ChatFact`] gathered by the
//! world (its job and workplace, a price it knows, a favour, a piece of news).
//!
//! Templates use placeholders filled in by [`render`]:
//! - `{p}` the player's first name (skipped with a stranger, who doesn't
//!   know it yet), `{s}` the NPC's, `{S:x/y}` the NPC's female / male form;
//! - `{j}` / `{k}` the NPC's job, without / with the article ("la cuoca"),
//!   `{w}` where it works ("in serra"), `{turno}` its shift ("dalle 7 alle
//!   16"), `{fine}` its end ("alle 16");
//! - `{al}` / `{nel}` / `{del}` the fact's carriage with the preposition
//!   ("al Bazar", "nella Serra 3"), `{am}` the other Mercato ("all'Emporio");
//! - `{un}` the item with its article ("un attrezzo"), `{i}` / `{li}` its
//!   plural (with the article), `{I:x/y}` the form agreeing with it; `{c}` /
//!   `{c2}` prices, `{r}` a reward ("12 gettoni", "un gettone"), `{q}` how
//!   many ("due barre di metallo"), `{N:x/y}` the singular / plural form
//!   agreeing with it ("mi {N:serve/servono}");
//! - `{a}` / `{A:x/y}` whom a piece of gossip is about;
//! - `{g}` / `{bg}` "Buongiorno" / "Buona giornata"… by the hour.
//!
//! A template whose data is missing is skipped for another; the variants
//! are picked with a seed of their own (deterministic, the world's RNG is
//! never touched).

use super::grammar;
use super::text::{self, BABBLE, Mix};
use crate::chat::{Band, Favour, Intent, MAX_CHAT_LINE_CHARS};
use crate::item::ItemKind;
use crate::npc::{Job, LifeStage, Sex};
use crate::personality::{Personality, Temper};

type Pool = &'static [&'static str];
/// Pools by band: low, mid, high.
type Banded = [Pool; 3];

/// Under this age people only babble.
const BABY_AGE: u32 = 4;

/// The NPC the player talks to.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ChatNpc<'a> {
    pub first: &'a str,
    pub sex: Sex,
    pub age: u32,
    pub job: Option<Job>,
    pub personality: Personality,
    pub hungry: bool,
    pub tired: bool,
    /// On shift right now.
    pub working: bool,
}

/// Something the player did that people talk about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Deed {
    /// Took `item` from a storage (the fact's place).
    Took,
    /// Gave `item` to someone (`who`).
    Gave,
}

/// What the world knows that the answer needs.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ChatFact<'a> {
    Nothing,
    /// The NPC's job (`place`: its workplace, None if it has no job).
    Job {
        place: Option<&'a str>,
        shift: (u32, u32),
    },
    /// The cheapest Mercato the NPC knows for `item`, and the dearest if
    /// another; `own`: the NPC is the Mercante there.
    Price {
        item: ItemKind,
        market: &'a str,
        price: u32,
        other: Option<(&'a str, u32)>,
        own: bool,
    },
    /// The NPC knows no Mercato.
    NoPrices,
    /// A favour asked now (new, or not told yet).
    FavourOffer(Favour),
    /// The favour asked earlier, not done yet.
    FavourReminder(Favour),
    /// The favour done; the NPC paid `paid` tokens.
    FavourDone {
        favour: Favour,
        paid: u32,
    },
    NoFavour,
    /// Whether the NPC accepts something the player has.
    Gift {
        accepts: bool,
    },
    /// The player just gave `item`.
    Gave(ItemKind),
    /// A Mercante (at its counter or not) or someone who sends the player
    /// to a Mercato (`market`); `opens` is when the counter opens.
    Trade {
        merchant: bool,
        at_counter: bool,
        market: Option<&'a str>,
        opens: u32,
    },
    /// A piece of news, already told in words.
    News(String),
    /// Gossip about the player.
    Deed {
        deed: Deed,
        item: ItemKind,
        place: &'a str,
        who: Option<(&'a str, Sex)>,
    },
    NoNews,
}

/// Everything a chat line depends on.
#[derive(Clone, Debug)]
pub(crate) struct ChatScene<'a> {
    pub npc: ChatNpc<'a>,
    /// The player's first name.
    pub player: &'a str,
    pub band: Band,
    /// The NPC never had to do with the player.
    pub stranger: bool,
    /// The same thing again too soon (greeting, insult): no effect.
    pub repeat: bool,
    /// Hour of the day (0–23).
    pub hour: u32,
    pub fact: ChatFact<'a>,
}

impl ChatScene<'_> {
    fn band_index(&self) -> usize {
        match self.band {
            Band::Low => 0,
            Band::Mid => 1,
            Band::High => 2,
        }
    }

    fn pool(&self, pools: Banded) -> Pool {
        pools[self.band_index()]
    }

    fn kid(&self) -> bool {
        self.npc.age < LifeStage::GIOVANE_FROM
    }
}

// ----------------------------------------------------------------------
// Entry points
// ----------------------------------------------------------------------

/// What the player says with a suggested reply.
pub(crate) fn player_line(intent: Intent, scene: &ChatScene, seed: u64) -> String {
    let pool = match intent {
        Intent::Greet if scene.stranger => P_GREET_STRANGER,
        Intent::Greet => P_GREET,
        Intent::AskJob => P_JOB,
        Intent::AskPrices => P_PRICES,
        Intent::AskFavour => match scene.fact {
            ChatFact::FavourDone { .. } => P_FAVOUR_DONE,
            ChatFact::FavourReminder(_) => P_FAVOUR_REMIND,
            _ => P_FAVOUR,
        },
        Intent::Gift => P_GIFT,
        Intent::Trade => P_TRADE,
        Intent::AskNews => P_NEWS,
        Intent::Insult => P_INSULT,
        Intent::Farewell => P_BYE,
    };
    // The player's own line: its name is known, the NPC's too.
    let scene = ChatScene {
        stranger: false,
        ..scene.clone()
    };
    let mut rng = Mix(seed ^ 0x005E_ED0F_91A7);
    pick(pool, &scene, &mut rng).unwrap_or_else(|| intent.label().to_string())
}

/// What the player says handing over `item` (see [`ChatFact::Gave`]).
pub(crate) fn gift_line(scene: &ChatScene, seed: u64) -> String {
    let mut rng = Mix(seed ^ 0x61F7);
    pick(P_GIVE, scene, &mut rng).unwrap_or_else(|| "Tieni.".to_string())
}

/// The NPC's answer to `intent`.
pub(crate) fn answer(intent: Intent, scene: &ChatScene, seed: u64) -> String {
    let mut rng = Mix(seed);
    if scene.npc.age < BABY_AGE {
        return pick(BABBLE, scene, &mut rng).unwrap_or_else(|| "Gu-gu!".to_string());
    }
    let pools = answer_pools(intent, scene, &mut rng);
    let line =
        pick(scene.pool(pools), scene, &mut rng).unwrap_or_else(|| fallback(scene).to_string());
    with_tail(line, intent, scene, &mut rng)
}

/// The NPC did not understand what the player typed.
pub(crate) fn not_understood(scene: &ChatScene, seed: u64) -> String {
    let mut rng = Mix(seed ^ 0xD0_17);
    if scene.npc.age < BABY_AGE {
        return pick(BABBLE, scene, &mut rng).unwrap_or_else(|| "Gu-gu!".to_string());
    }
    let pools = if scene.npc.personality.has(Temper::Timido) && rng.chance(0.5) {
        [SHY_HUH, SHY_HUH, SHY_HUH]
    } else {
        HUH
    };
    pick(scene.pool(pools), scene, &mut rng).unwrap_or_else(|| "Eh?".to_string())
}

/// The NPC speaks first (it had "something to say", see
/// [`crate::PlayerTie::wants_to_talk`]): a favour to ask
/// ([`ChatFact::FavourOffer`]) or, from a friend, a warm opener.
pub(crate) fn opening(scene: &ChatScene, seed: u64) -> String {
    let mut rng = Mix(seed ^ 0x0FE7);
    if scene.npc.age < BABY_AGE {
        return pick(BABBLE, scene, &mut rng).unwrap_or_else(|| "Gu-gu!".to_string());
    }
    let pool = match &scene.fact {
        ChatFact::FavourOffer(f) if f.reward > 0 => OPEN_FAVOUR,
        ChatFact::FavourOffer(_) => OPEN_FAVOUR_FREE,
        _ => OPEN_FRIEND,
    };
    pick(pool, scene, &mut rng).unwrap_or_else(|| fallback(scene).to_string())
}

// ----------------------------------------------------------------------
// Choosing the pools
// ----------------------------------------------------------------------

fn answer_pools(intent: Intent, scene: &ChatScene, rng: &mut Mix) -> Banded {
    let p = scene.npc.personality;
    let tag = |t: Temper, rng: &mut Mix| p.has(t) && rng.chance(0.5);
    match intent {
        Intent::Greet => {
            if scene.repeat {
                GREET_AGAIN
            } else if scene.kid() {
                KID_GREET
            } else if scene.stranger {
                [GREET_STRANGER, GREET_STRANGER, GREET_STRANGER]
            } else if tag(Temper::Burbero, rng) {
                GRUMPY_GREET
            } else if tag(Temper::Timido, rng) {
                SHY_GREET
            } else if tag(Temper::Allegro, rng) {
                CHEERFUL_GREET
            } else if tag(Temper::Gentile, rng) {
                KIND_GREET
            } else {
                GREET
            }
        }
        Intent::AskJob => job_pools(scene, rng),
        Intent::AskPrices => match &scene.fact {
            ChatFact::Price { own: true, .. } => PRICE_OWN,
            ChatFact::Price { .. } if tag(Temper::Lamentoso, rng) => LAMENT_PRICE,
            ChatFact::Price { .. } => PRICE,
            _ => NO_PRICES,
        },
        Intent::AskFavour => match &scene.fact {
            ChatFact::FavourOffer(f) if f.reward > 0 => FAVOUR_OFFER,
            ChatFact::FavourOffer(_) => FAVOUR_OFFER_FREE,
            ChatFact::FavourReminder(_) => FAVOUR_REMIND,
            ChatFact::FavourDone { paid: 0, .. } => FAVOUR_DONE_FREE,
            ChatFact::FavourDone { .. } => FAVOUR_DONE,
            _ => NO_FAVOUR,
        },
        Intent::Gift => match &scene.fact {
            ChatFact::Gave(_) => GAVE,
            ChatFact::Gift { accepts: true } => GIFT_YES,
            _ => GIFT_NO,
        },
        Intent::Trade => match &scene.fact {
            ChatFact::Trade {
                merchant: true,
                at_counter: true,
                ..
            } => TRADE_COUNTER,
            ChatFact::Trade { merchant: true, .. } => TRADE_CLOSED,
            ChatFact::Trade {
                market: Some(_), ..
            } => TRADE_ELSEWHERE,
            _ => TRADE_NOTHING,
        },
        Intent::AskNews => {
            let gossip = p.has(Temper::Pettegolo);
            match &scene.fact {
                // Gossips tell even who they don't like.
                ChatFact::News(_) if gossip => [GOSSIP_NEWS, NEWS[1], GOSSIP_NEWS],
                ChatFact::News(_) => NEWS,
                ChatFact::Deed {
                    deed: Deed::Took, ..
                } => DEED_TOOK,
                ChatFact::Deed { .. } => DEED_GAVE,
                _ => NO_NEWS,
            }
        }
        Intent::Insult => {
            if scene.repeat {
                INSULT_AGAIN
            } else if scene.kid() {
                KID_INSULT
            } else if tag(Temper::Burbero, rng) {
                GRUMPY_INSULT
            } else if tag(Temper::Timido, rng) {
                SHY_INSULT
            } else if tag(Temper::Gentile, rng) {
                KIND_INSULT
            } else {
                INSULT
            }
        }
        Intent::Farewell => {
            if scene.kid() {
                KID_BYE
            } else if tag(Temper::Timido, rng) {
                SHY_BYE
            } else if tag(Temper::Allegro, rng) {
                CHEERFUL_BYE
            } else if tag(Temper::Burbero, rng) {
                GRUMPY_BYE
            } else {
                BYE
            }
        }
    }
}

fn job_pools(scene: &ChatScene, rng: &mut Mix) -> Banded {
    let p = scene.npc.personality;
    let has_place = matches!(scene.fact, ChatFact::Job { place: Some(_), .. });
    if scene.npc.job.is_none() || !has_place {
        return match LifeStage::of_age(scene.npc.age) {
            LifeStage::Bambino => KID_JOB,
            LifeStage::Giovane => YOUTH_JOB,
            LifeStage::Anziano => RETIRED_JOB,
            LifeStage::Adulto => JOBLESS,
        };
    }
    if scene.npc.working && scene.band != Band::Low && rng.chance(0.5) {
        return [JOB[0], WORKING_JOB, WORKING_JOB];
    }
    if p.has(Temper::Burbero) && rng.chance(0.5) {
        GRUMPY_JOB
    } else if p.has(Temper::Lamentoso) && rng.chance(0.5) {
        LAMENT_JOB
    } else if p.has(Temper::Allegro) && rng.chance(0.5) {
        CHEERFUL_JOB
    } else {
        JOB
    }
}

/// A short remark after the answer: hunger, tiredness, the shift, a
/// question back (curious), more talk (talkative).
fn with_tail(line: String, intent: Intent, scene: &ChatScene, rng: &mut Mix) -> String {
    if scene.band == Band::Low || scene.kid() || scene.repeat {
        return line;
    }
    let p = scene.npc.personality;
    let chatty = matches!(intent, Intent::Greet | Intent::AskJob | Intent::AskNews);
    let tail = if !chatty {
        None
    } else if scene.npc.hungry && rng.chance(0.6) {
        Some(HUNGRY_TAIL)
    } else if scene.npc.tired && rng.chance(0.6) {
        Some(TIRED_TAIL)
    } else if intent == Intent::Greet && scene.npc.working && rng.chance(0.5) {
        Some(WORKING_TAIL)
    } else if intent == Intent::Greet && p.has(Temper::Curioso) && rng.chance(0.4) {
        Some(CURIOUS_TAIL)
    } else if p.has(Temper::Chiacchierone) && rng.chance(0.4) {
        Some(CHATTY_TAIL)
    } else {
        None
    };
    let Some(pool) = tail else {
        return line;
    };
    match pick(pool, scene, rng) {
        Some(extra) if line.chars().count() + 1 + extra.chars().count() <= MAX_CHAT_LINE_CHARS => {
            format!("{line} {extra}")
        }
        _ => line,
    }
}

/// When no template fits (never with ordinary names, see the tests).
fn fallback(scene: &ChatScene) -> &'static str {
    match scene.band {
        Band::Low => "Mh-mh.",
        Band::Mid => "Già, già.",
        Band::High => "Eh, già!",
    }
}

// ----------------------------------------------------------------------
// Rendering
// ----------------------------------------------------------------------

/// Renders a random template of `pool` that fits [`MAX_CHAT_LINE_CHARS`].
fn pick(pool: Pool, scene: &ChatScene, rng: &mut Mix) -> Option<String> {
    if pool.is_empty() {
        return None;
    }
    let start = rng.below(pool.len());
    (0..pool.len()).find_map(|k| {
        let text = render(pool[(start + k) % pool.len()], scene)?;
        (text.chars().count() <= MAX_CHAT_LINE_CHARS).then_some(text)
    })
}

/// The item the fact is about, if any.
fn item_of(fact: &ChatFact) -> Option<ItemKind> {
    match fact {
        ChatFact::Price { item, .. } | ChatFact::Gave(item) | ChatFact::Deed { item, .. } => {
            Some(*item)
        }
        ChatFact::FavourOffer(f) | ChatFact::FavourReminder(f) => Some(f.item),
        ChatFact::FavourDone { favour, .. } => Some(favour.item),
        _ => None,
    }
}

/// The carriage the fact is about, if any.
fn place_of<'a>(fact: &ChatFact<'a>) -> Option<&'a str> {
    match fact {
        ChatFact::Job { place, .. } => *place,
        ChatFact::Price { market, .. } => Some(market),
        ChatFact::Trade { market, .. } => *market,
        ChatFact::Deed { place, .. } => Some(place),
        _ => None,
    }
}

fn favour_of(fact: &ChatFact) -> Option<(Favour, u32)> {
    match fact {
        ChatFact::FavourOffer(f) | ChatFact::FavourReminder(f) => Some((*f, f.reward)),
        ChatFact::FavourDone { favour, paid } => Some((*favour, *paid)),
        _ => None,
    }
}

/// Fills the placeholders of `template`; None if it needs data the scene
/// lacks.
fn render(template: &str, scene: &ChatScene) -> Option<String> {
    let npc = &scene.npc;
    let fact = &scene.fact;
    let item = item_of(fact);
    let place = place_of(fact);
    let item_fem = item.is_some_and(|i| grammar::item_noun(i.name(), i.plural()).0);
    let mut out = String::with_capacity(template.len() + 24);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let len = rest[open..].find('}')?;
        let token = &rest[open + 1..open + len];
        rest = &rest[open + len + 1..];
        if let Some((who, forms)) = token.split_once(':') {
            let (female, male) = forms.split_once('/').unwrap_or((forms, forms));
            let female_form = match who {
                "S" => npc.sex == Sex::Female,
                "I" => item_fem,
                // Number: the singular form for one unit of a favour.
                "N" => !favour_of(fact).is_some_and(|(f, _)| f.count > 1),
                "A" => match fact {
                    ChatFact::Deed {
                        who: Some((_, sex)),
                        ..
                    } => *sex == Sex::Female,
                    _ => return None,
                },
                _ => return None,
            };
            text::push_joined(&mut out, if female_form { female } else { male });
            continue;
        }
        let value: String = match token {
            "p" if scene.stranger => return None,
            "p" => scene.player.to_string(),
            "s" => npc.first.to_string(),
            "j" => text::job_word(npc.job, npc.sex).to_string(),
            "k" => {
                let word = text::job_word(npc.job, npc.sex);
                let fem = npc.sex == Sex::Female;
                format!("{}{word}", grammar::article(word, fem, false).with(None))
            }
            "w" => text::workplace(npc.job).to_string(),
            "turno" | "fine" => {
                let ChatFact::Job { shift, .. } = fact else {
                    return None;
                };
                if token == "fine" {
                    grammar::at_hour(false, shift.1)
                } else {
                    format!(
                        "{} {}",
                        grammar::at_hour(true, shift.0),
                        grammar::at_hour(false, shift.1)
                    )
                }
            }
            "apre" => match fact {
                ChatFact::Trade { opens, .. } => grammar::at_hour(false, *opens),
                _ => return None,
            },
            "al" => grammar::prep_a(place?),
            "nel" => grammar::prep_in(place?),
            "del" => grammar::prep_di(place?),
            "am" => match fact {
                ChatFact::Price {
                    other: Some((m, _)),
                    ..
                } => grammar::prep_a(m),
                _ => return None,
            },
            "c" => match fact {
                ChatFact::Price { price, .. } => grammar::tokens(*price),
                _ => return None,
            },
            "c2" => match fact {
                ChatFact::Price {
                    other: Some((_, p)),
                    ..
                } => grammar::tokens(*p),
                _ => return None,
            },
            "un" => item?.with_article().to_string(),
            "Un" => grammar::capitalize(item?.with_article()),
            "i" => item?.plural().to_string(),
            "li" => {
                let item = item?;
                let (fem, plural) = grammar::item_noun(item.name(), item.plural());
                let word = item.plural();
                format!("{}{word}", grammar::article(word, fem, plural).with(None))
            }
            "q" => {
                let (f, _) = favour_of(fact)?;
                grammar::counted(f.count, f.item.with_article(), f.item.plural())
            }
            "r" => grammar::tokens(favour_of(fact)?.1),
            "news" => match fact {
                ChatFact::News(text) => text.clone(),
                _ => return None,
            },
            "a" => match fact {
                ChatFact::Deed {
                    who: Some((name, _)),
                    ..
                } => (*name).to_string(),
                _ => return None,
            },
            "g" => text::greeting(scene.hour).to_string(),
            "bg" => text::goodbye(scene.hour).to_string(),
            _ => return None,
        };
        text::push_joined(&mut out, &value);
    }
    out.push_str(rest);
    Some(grammar::capitalize(&grammar::tidy(&out)))
}

// ----------------------------------------------------------------------
// The player's lines
// ----------------------------------------------------------------------

const P_GREET: Pool = &["Ciao, {s}!", "{g}, {s}.", "Ehi, {s}, come va?"];
const P_GREET_STRANGER: Pool = &[
    "Ciao, sono {p}.",
    "{g}! Mi chiamo {p}.",
    "Piacere, io sono {p}.",
];
const P_JOB: Pool = &[
    "Di cosa ti occupi?",
    "Che lavoro fai?",
    "Come va il lavoro?",
];
const P_PRICES: Pool = &[
    "Sai dove conviene comprare?",
    "Com'è il mercato, in questi giorni?",
    "Dove costa meno la roba?",
];
const P_FAVOUR: Pool = &["Posso fare qualcosa per te?", "Ti serve una mano?"];
const P_FAVOUR_REMIND: Pool = &["Cosa ti serviva?", "Ricordami: cosa ti serve?"];
const P_FAVOUR_DONE: Pool = &["Ecco {q}, come promesso.", "Ti ho portato {q}."];
const P_GIFT: Pool = &["Ho qualcosa per te.", "Ti ho portato un pensiero."];
const P_GIVE: Pool = &[
    "Tieni: {un}.",
    "Questo è per te: {un}.",
    "Ecco {un}, per te.",
];
const P_TRADE: Pool = &["Facciamo affari?", "Hai qualcosa da vendere?"];
const P_NEWS: Pool = &["Novità?", "Che si dice in giro?", "Hai sentito qualcosa?"];
const P_INSULT: Pool = &[
    "Sei insopportabile!",
    "Mi dai sui nervi.",
    "Che faccia antipatica.",
];
const P_BYE: Pool = &["Ci vediamo, {s}.", "A presto!", "Devo andare. Ciao!"];

// ----------------------------------------------------------------------
// The NPC's answers, by band: [low, mid, high]
// ----------------------------------------------------------------------

// Greetings.
const GREET: Banded = [
    &["Mh.", "Che vuoi?", "Ah. Sei tu.", "{g}. Se proprio devo."],
    &[
        "{g}, {p}.",
        "Ciao, {p}. Tutto bene?",
        "Salve, {p}.",
        "Oh, ciao {p}!",
        "{g}.",
    ],
    &[
        "{p}! Che bello vederti!",
        "Ciao, {p}! Come stai?",
        "Ehi, {p}! Tutto bene?",
        "{p}! Sempre un piacere.",
    ],
];
const GREET_STRANGER: Pool = &[
    "Piacere, io sono {s}.",
    "Ciao. Non ci conosciamo: sono {s}.",
    "{g}. Io sono {s}.",
];
const GREET_AGAIN: Banded = [
    &["Ancora tu?", "Basta, ho capito."],
    &[
        "Ci siamo già salutati, {p}.",
        "Sì, ciao di nuovo.",
        "Ancora ciao?",
    ],
    &["Ancora ciao, {p}!", "Ciao di nuovo, {p}!"],
];
const GRUMPY_GREET: Banded = [
    &["Sparisci.", "Non ho tempo per te."],
    &["Mh. Ciao.", "Sì, sì. Ciao."],
    &["Ah, {p}. Ciao, va'.", "Mh. Ciao, {p}."],
];
const SHY_GREET: Banded = [
    &["…", "Ah… ciao."],
    &["Oh… c-ciao, {p}.", "Ehm… {g}."],
    &["Ciao, {p}… mi fa piacere vederti.", "Oh! C-ciao, {p}."],
];
const CHEERFUL_GREET: Banded = [
    &["Ciao comunque!", "Oh, ciao!"],
    &["Ciao, {p}! Che bella giornata!", "{g}, {p}! Tutto a posto?"],
    &["{p}! Mi hai messo di buon umore!", "Evviva, {p}! Ciao!"],
];
const KIND_GREET: Banded = [
    &["{g}.", "Ciao. Posso aiutarti?"],
    &["{g}, {p}. Come stai?", "Ciao, {p}. Tutto bene a casa?"],
    &[
        "{p}, che piacere vederti. Stai bene?",
        "Ciao, {p}! Ti trovo bene.",
    ],
];
const KID_GREET: Banded = [
    &["Non parlo con te!", "Vai via!"],
    &["Ciao! Tu chi sei?", "Ciao, {p}!"],
    &["{p}! Giochi con me?", "Ciao, {p}! Guarda cosa so fare!"],
];

// Work.
const JOB: Banded = [
    &[
        "Non sono affari tuoi.",
        "Lavoro. Ti basta?",
        "Faccio {k}. Contento?",
    ],
    &[
        "Faccio {k}, {nel}.",
        "Sono {j}. Lavoro {nel}, {turno}.",
        "Lavoro {w}, {nel}.",
    ],
    &[
        "Faccio {k} {nel}! Passa a trovarmi.",
        "Sono {j} {nel}, {turno}. Mi piace, sai?",
        "Lavoro {w}, {nel}: vieni quando vuoi!",
    ],
];
const WORKING_JOB: Pool = &[
    "Sto lavorando proprio adesso, lo vedi.",
    "Sono di turno {nel} fino {fine}.",
    "Faccio {k}, e sono di turno: fai presto.",
];
const GRUMPY_JOB: Banded = [
    &["Fatti gli affari tuoi."],
    &["Faccio {k}. Punto.", "Lavoro {nel}. Altro?"],
    &["Faccio {k}, {p}. Niente di che."],
];
const LAMENT_JOB: Banded = [
    &["Un lavoro da cani, ecco."],
    &[
        "Faccio {k}. Una fatica, credimi.",
        "Lavoro {nel}, e mi pagano una miseria.",
    ],
    &["Faccio {k}… stanc{S:a/o} mort{S:a/o}. Ma tiro avanti."],
];
const CHEERFUL_JOB: Banded = [
    &["Lavoro, lavoro!"],
    &["Faccio {k} {nel}, e mi piace!"],
    &["Faccio {k}! Il lavoro più bello del treno!"],
];
const KID_JOB: Banded = [
    &["Non te lo dico!"],
    &[
        "Io non lavoro, sono piccol{S:a/o}!",
        "Io gioco tutto il giorno!",
    ],
    &["Da grande voglio fare il cuoco!", "Io gioco! Vuoi giocare?"],
];
const YOUTH_JOB: Banded = [
    &["Non sono affari tuoi."],
    &[
        "Non lavoro ancora: aspetto i diciott'anni.",
        "Aiuto a casa, per ora.",
    ],
    &[
        "Tra poco mi danno un lavoro, {p}!",
        "Non ancora, ma non vedo l'ora!",
    ],
];
const RETIRED_JOB: Banded = [
    &["Ho già lavorato abbastanza."],
    &[
        "Sono in pensione, ormai.",
        "Ho lavorato una vita: ora mi riposo.",
    ],
    &["In pensione, {p}! Finalmente mi godo il treno."],
];
const JOBLESS: Banded = [
    &["Non sono affari tuoi."],
    &[
        "Sono senza lavoro, per ora.",
        "Aspetto che mi diano un posto.",
    ],
    &["Niente, {p}: aspetto un posto. Se senti qualcosa, dimmelo!"],
];

// Prices.
const PRICE: Banded = [
    &[
        "Chiedi al mercante.",
        "Non ti faccio da listino.",
        "E io che ne so?",
    ],
    &[
        "{Un} costa {c} {al}.",
        "{al} {un} costa {c}, mi pare.",
        "Per {li} vai {al}: {c} {I:l'una/l'uno}.",
    ],
    &[
        "Te lo dico io: {un} costa {c} {al}.",
        "{al} {un} costa {c}; {am} {c2}.",
        "Per {li} vai {al}, {c}. Fidati di me.",
    ],
];
const PRICE_OWN: Banded = [
    &[
        "{c}, e non si tratta.",
        "Al mio banco {un} costa {c}. Prendere o lasciare.",
    ],
    &["Al mio banco {un} costa {c}.", "Da me {un} costa {c}."],
    &[
        "Da me {un} costa {c}, e a te faccio un buon prezzo!",
        "Per te, {p}: {un} a {c}, qui al banco.",
    ],
];
const LAMENT_PRICE: Banded = [
    &["Prezzi da ladri, ovunque."],
    &[
        "{Un} costa {c} {al}. Una rapina!",
        "{c} per {un}, {al}. Che prezzi!",
    ],
    &["Guarda, {p}: {un} costa {c} {al}. Una vergogna!"],
];
const NO_PRICES: Banded = [
    &["Chiedi al mercante.", "E io che ne so?"],
    &[
        "Non vado mai al Mercato, non saprei.",
        "Non ne ho idea, {p}.",
    ],
    &[
        "Non lo so, {p}. Chiedi a un mercante!",
        "Mi spiace, {p}: al Mercato non ci vado.",
    ],
];

// Favours.
const FAVOUR_OFFER: Banded = [
    &["Portami {q}. Ti do {r}, niente di più."],
    &[
        "Mi porteresti {q}? Ti do {r}.",
        "Mi {N:servirebbe/servirebbero} {q}: ti do {r}.",
    ],
    &[
        "Sì! Mi porteresti {q}? Ti do {r}.",
        "Mi {N:serve/servono} {q}, {p}. Ti do {r}.",
    ],
];
const FAVOUR_OFFER_FREE: Banded = [
    &["Portami {q}. Gettoni non ne ho."],
    &[
        "Mi porteresti {q}? Non ho gettoni, ma te ne sarei grat{S:a/o}.",
        "Mi {N:servirebbe/servirebbero} {q}… ma non ho un gettone.",
    ],
    &["Mi porteresti {q}, {p}? Non posso pagarti, ma te ne sarei grat{S:a/o}."],
];
const FAVOUR_REMIND: Banded = [
    &["E {q}? Aspetto."],
    &[
        "Aspetto ancora {q}.",
        "Ti ricordi? Mi {N:serve/servono} {q}.",
    ],
    &[
        "Mi {N:serve/servono} ancora {q}, {p}.",
        "Non ti scordare {q}, eh!",
    ],
];
const FAVOUR_DONE: Banded = [
    &["Mh. Grazie. Ecco {r}."],
    &[
        "Grazie! Ecco i tuoi {r}.",
        "Proprio quello che mi serviva. Tieni: {r}.",
    ],
    &["Grazie, {p}! Tieni, {r}.", "Sei d'oro, {p}! Ecco {r}."],
];
const FAVOUR_DONE_FREE: Banded = [
    &["Mh. Grazie."],
    &["Grazie! Non ho gettoni, ma ti devo un favore."],
    &[
        "Grazie di cuore, {p}!",
        "Sei d'oro, {p}! Ti devo un favore.",
    ],
];
const NO_FAVOUR: Banded = [
    &["No.", "Da te? Niente."],
    &[
        "Per ora non mi serve niente, grazie.",
        "Niente, grazie. Sei gentile.",
    ],
    &[
        "Niente, {p}, ma grazie del pensiero!",
        "Per ora niente, ma ti terrò presente!",
    ],
];

// Gifts.
const GIFT_YES: Banded = [
    &["Non voglio niente da te.", "Tieniti la tua roba."],
    &["Un regalo? Vediamo…", "Per me? Cos'è?"],
    &[
        "Per me? Che pensiero gentile!",
        "Un regalo? Sei un tesoro, {p}!",
    ],
];
const GIFT_NO: Banded = [
    &["Non voglio niente da te.", "Tieniti la tua roba."],
    &["Grazie, ma non mi serve niente di quello che hai."],
    &["Grazie, {p}, ma non mi serve niente di quello che hai."],
];
const GAVE: Banded = [
    &["Mh. Grazie."],
    &["Grazie per {un}!", "Oh, {un}! Grazie."],
    &[
        "Grazie, {p}! {Un} mi serviva proprio.",
        "Sei un tesoro! Grazie per {un}.",
    ],
];

// Trade.
const TRADE_COUNTER: Banded = [
    &["Paga e prendi. Niente sconti."],
    &["Vediamo cosa ti serve.", "Ecco la merce del banco."],
    &[
        "Per te, {p}, i prezzi migliori!",
        "Guarda pure, {p}: roba buona.",
    ],
];
const TRADE_CLOSED: Banded = [
    &["Il banco è chiuso."],
    &[
        "Ora non sono al banco. Passa {al} più tardi.",
        "Il banco apre {apre}: vieni {al}.",
    ],
    &["Passa {al}, {p}: il banco apre {apre}."],
];
const TRADE_ELSEWHERE: Banded = [
    &["Non ho niente da scambiare con te."],
    &["Io non vendo niente. Prova {al}."],
    &["Non vendo niente, {p}. Ma {al} trovi di tutto."],
];
const TRADE_NOTHING: Banded = [
    &["Non ho niente da scambiare con te."],
    &["Io non vendo niente.", "Non ho niente da vendere."],
    &["Non ho niente da vendere, {p}. Mi spiace!"],
];

// News.
const NEWS: Banded = [
    &["Non sono affari tuoi.", "Chiedi a qualcun altro."],
    &["{news}"],
    &["{news}", "Senti questa, {p}! {news}"],
];
const GOSSIP_NEWS: Pool = &["{news}", "Senti questa! {news}", "Resti tra noi: {news}"];
const NO_NEWS: Banded = [
    &["Niente che ti riguardi."],
    &[
        "Niente di nuovo, per fortuna.",
        "Tutto tranquillo, mi pare.",
    ],
    &[
        "Niente di nuovo, {p}. Una noia!",
        "Tutto tranquillo. Tu che mi racconti?",
    ],
];
const DEED_TOOK: Banded = [
    &[
        "So cos'hai preso {nel}. Vergogna.",
        "Tutti sanno cos'hai preso {nel}.",
    ],
    &[
        "Dicono che hai preso della roba {nel}…",
        "Si dice in giro che prendi roba {nel}.",
    ],
    &[
        "Occhio, {p}: dicono che hai preso roba {nel}.",
        "Ti hanno visto prendere {un} {nel}, sai?",
    ],
];
const DEED_GAVE: Banded = [
    &["Fai regali a tutti, eh? A me mai."],
    &[
        "Ho sentito che hai regalato {un} a {a}.",
        "Dicono che hai fatto un regalo a {a}.",
    ],
    &[
        "Ho saputo di {un} per {a}. Bel gesto, {p}!",
        "{a} parla bene di te, sai?",
    ],
];

// Insults.
const INSULT: Banded = [
    &["Come ti permetti?!", "Vattene, {p}.", "Sparisci!"],
    &[
        "Ma come ti permetti?",
        "Che maleducazione!",
        "Non ti ho fatto niente!",
    ],
    &[
        "{p}! Perché mi tratti così?",
        "Questa da te non me l'aspettavo.",
        "Mi hai ferit{S:a/o}, {p}.",
    ],
];
const INSULT_AGAIN: Banded = [
    &["Hai finito?", "Sì, sì. Ho capito."],
    &["Ancora? Che noia.", "L'ho sentito, grazie."],
    &["Basta, {p}. Ho capito.", "Ancora? Mi fai pena."],
];
const GRUMPY_INSULT: Banded = [
    &["Ripetilo, se hai coraggio."],
    &["Stai attento a come parli."],
    &["Ripetilo e non siamo più amici."],
];
const SHY_INSULT: Banded = [&["…"], &["P-perché dici così?"], &["…perché, {p}?"]];
const KIND_INSULT: Banded = [
    &["Mi dispiace che la pensi così."],
    &["Hai una brutta giornata, eh?"],
    &["Dai, {p}. Non lo pensi davvero."],
];
const KID_INSULT: Banded = [
    &["Lo dico alla mamma!"],
    &["Non si dicono le parolacce!", "Lo dico alla mamma!"],
    &["Perché sei cattiv{S:a/o} con me?"],
];

// Goodbyes.
const BYE: Banded = [
    &["Finalmente.", "Sì, vai pure.", "Era ora."],
    &["{bg}, {p}.", "Ciao, {p}.", "A presto."],
    &[
        "A presto, {p}!",
        "{bg}, {p}! Torna presto.",
        "Ciao, {p}! Passa quando vuoi.",
    ],
];
const SHY_BYE: Banded = [&["…"], &["C-ciao…"], &["Ciao, {p}… torna, eh."]];
const CHEERFUL_BYE: Banded = [
    &["Ciao!"],
    &["Ciao ciao, {p}!"],
    &["Ciao, {p}! Buona fortuna!"],
];
const GRUMPY_BYE: Banded = [&["Bene."], &["Mh. Ciao."], &["Ciao, va'."]];
const KID_BYE: Banded = [
    &["Bleah!"],
    &["Ciao ciao!"],
    &["Ciao, {p}! Torna a giocare!"],
];

// Not understood.
const HUH: Banded = [
    &["Eh? Parla chiaro.", "Non ti capisco. E non mi interessa."],
    &["Non ho capito, {p}.", "Come, scusa?", "Cosa intendi?"],
    &[
        "Scusa, {p}, non ho capito. Ripeti?",
        "Eh? Dimmi meglio, {p}.",
    ],
];
const SHY_HUH: Pool = &["Ehm… non ho capito.", "S-scusa?"];

// The NPC speaks first.
const OPEN_FAVOUR: Pool = &[
    "{p}! Mi faresti un favore? Mi {N:serve/servono} {q}.",
    "Ehi, {p}: mi porteresti {q}? Ti do {r}.",
    "{p}, ti cercavo! Mi {N:servirebbe/servirebbero} {q}, ti do {r}.",
];
const OPEN_FAVOUR_FREE: Pool = &[
    "{p}, mi faresti un favore? Mi {N:serve/servono} {q}.",
    "Ehi, {p}: mi porteresti {q}?",
];
const OPEN_FRIEND: Pool = &[
    "{p}! Ti cercavo, sai?",
    "Ehi, {p}! Volevo proprio salutarti.",
    "{p}! Che bello, passavi di qui?",
];

// Remarks after the answer.
const HUNGRY_TAIL: Pool = &["Ho una fame…", "Scusa, ho lo stomaco vuoto."];
const TIRED_TAIL: Pool = &["Sono stanc{S:a/o} mort{S:a/o}.", "Che sonno, oggi."];
const WORKING_TAIL: Pool = &["Ma sto lavorando, eh.", "Fai presto, sono di turno."];
const CURIOUS_TAIL: Pool = &["E tu, {p}?", "E tu come stai?"];
const CHATTY_TAIL: Pool = &["Sai, a proposito…", "Ma parliamo, parliamo!"];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::GameTime;

    const PLACES: [&str; 5] = [
        "Il Bazar",
        "Serra 3",
        "Alveare",
        "Officina Grande",
        "La Brace",
    ];

    fn npc(sex: Sex, age: u32, job: Option<Job>, personality: Personality) -> ChatNpc<'static> {
        ChatNpc {
            first: if sex == Sex::Female { "Elena" } else { "Aldo" },
            sex,
            age,
            job,
            personality,
            hungry: false,
            tired: false,
            working: false,
        }
    }

    fn favour(count: u32, reward: u32) -> Favour {
        Favour {
            item: ItemKind::Metallo,
            count,
            reward,
            since: GameTime(0),
            until: GameTime(100),
            told: true,
        }
    }

    /// The facts the world may pass with each intent.
    fn facts(intent: Intent) -> Vec<ChatFact<'static>> {
        match intent {
            Intent::AskJob => vec![
                ChatFact::Job {
                    place: Some("Serra 3"),
                    shift: (7, 16),
                },
                ChatFact::Job {
                    place: Some("Alveare"),
                    shift: (8, 1),
                },
                ChatFact::Job {
                    place: None,
                    shift: (0, 0),
                },
            ],
            Intent::AskPrices => vec![
                ChatFact::Price {
                    item: ItemKind::Attrezzo,
                    market: "Il Bazar",
                    price: 23,
                    other: Some(("Alveare", 31)),
                    own: false,
                },
                ChatFact::Price {
                    item: ItemKind::Vestito,
                    market: "Officina Grande",
                    price: 1,
                    other: None,
                    own: true,
                },
                ChatFact::Price {
                    item: ItemKind::Coperta,
                    market: "La Brace",
                    price: 9,
                    other: None,
                    own: false,
                },
                ChatFact::NoPrices,
            ],
            Intent::AskFavour => vec![
                ChatFact::FavourOffer(favour(2, 14)),
                ChatFact::FavourOffer(favour(1, 0)),
                ChatFact::FavourReminder(favour(3, 5)),
                ChatFact::FavourDone {
                    favour: favour(2, 14),
                    paid: 1,
                },
                ChatFact::FavourDone {
                    favour: favour(2, 14),
                    paid: 0,
                },
                ChatFact::NoFavour,
            ],
            Intent::Gift => vec![
                ChatFact::Gift { accepts: true },
                ChatFact::Gift { accepts: false },
                ChatFact::Gave(ItemKind::Razione),
                ChatFact::Gave(ItemKind::Attrezzo),
            ],
            Intent::Trade => vec![
                ChatFact::Trade {
                    merchant: true,
                    at_counter: true,
                    market: Some("Il Bazar"),
                    opens: 9,
                },
                ChatFact::Trade {
                    merchant: true,
                    at_counter: false,
                    market: Some("Il Bazar"),
                    opens: 9,
                },
                ChatFact::Trade {
                    merchant: false,
                    at_counter: false,
                    market: Some("Alveare"),
                    opens: 9,
                },
                ChatFact::Trade {
                    merchant: false,
                    at_counter: false,
                    market: None,
                    opens: 9,
                },
            ],
            Intent::AskNews => vec![
                ChatFact::News("È nata Anna, dei Rossi!".to_string()),
                ChatFact::NoNews,
                ChatFact::Deed {
                    deed: Deed::Took,
                    item: ItemKind::Rottame,
                    place: "Alveare",
                    who: None,
                },
                ChatFact::Deed {
                    deed: Deed::Gave,
                    item: ItemKind::Te,
                    place: "Serra 3",
                    who: Some(("Anna", Sex::Female)),
                },
            ],
            _ => vec![ChatFact::Nothing],
        }
    }

    fn voices() -> Vec<ChatNpc<'static>> {
        let mut out = vec![
            npc(
                Sex::Female,
                30,
                Some(Job::Contadino),
                Personality::default(),
            ),
            npc(Sex::Male, 45, Some(Job::Mercante), Personality::default()),
            npc(Sex::Female, 9, None, Personality::default()),
            npc(Sex::Male, 16, None, Personality::default()),
            npc(Sex::Female, 70, None, Personality::default()),
            npc(Sex::Male, 30, None, Personality::default()),
            npc(Sex::Male, 2, None, Personality::default()),
        ];
        for tag in Temper::ALL {
            for sex in Sex::ALL {
                let mut v = npc(
                    sex,
                    35,
                    Some(Job::Operaio),
                    Personality::default().with(tag),
                );
                v.hungry = sex == Sex::Female;
                v.tired = sex == Sex::Male;
                v.working = tag.index() % 2 == 0;
                out.push(v);
            }
        }
        out
    }

    fn check(line: &str, what: &str) {
        assert!(!line.is_empty(), "{what}: empty");
        assert!(!line.contains(['{', '}']), "{what}: «{line}»");
        assert!(
            line.chars().count() <= MAX_CHAT_LINE_CHARS,
            "{what}: too long «{line}»"
        );
        let slips = grammar::slips(line, &PLACES);
        assert!(slips.is_empty(), "{what}: «{line}» {slips:?}");
    }

    /// Every intent has a proper answer (not the generic fallback) for
    /// every band, with every personality, age and fact.
    #[test]
    fn every_intent_has_a_reply_for_every_band() {
        for intent in Intent::ALL {
            for band in Band::ALL {
                for fact in facts(intent) {
                    for voice in voices() {
                        for stranger in [false, true] {
                            for repeat in [false, true] {
                                for seed in 0..12u64 {
                                    let scene = ChatScene {
                                        npc: voice,
                                        player: "Ada",
                                        band,
                                        stranger,
                                        repeat,
                                        hour: (seed * 5 % 24) as u32,
                                        fact: fact.clone(),
                                    };
                                    let what = format!(
                                        "{intent:?} {band:?} {fact:?} {} {}y {:?} s{stranger} r{repeat}",
                                        voice.first, voice.age, voice.personality
                                    );
                                    let line = answer(intent, &scene, seed);
                                    check(&line, &what);
                                    if voice.age >= BABY_AGE && !stranger {
                                        assert_ne!(line, fallback(&scene), "{what}");
                                    }
                                    check(&player_line(intent, &scene, seed), &what);
                                    check(&not_understood(&scene, seed), &what);
                                    check(&opening(&scene, seed), &what);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn answers_use_the_facts_and_the_grammar() {
        let voice = npc(Sex::Female, 30, Some(Job::Cuoco), Personality::default());
        let scene = |band, fact| ChatScene {
            npc: voice,
            player: "Ada",
            band,
            stranger: false,
            repeat: false,
            hour: 9,
            fact,
        };
        let job = scene(
            Band::Mid,
            ChatFact::Job {
                place: Some("Alveare"),
                shift: (6, 15),
            },
        );
        let lines: Vec<String> = (0..30).map(|s| answer(Intent::AskJob, &job, s)).collect();
        assert!(
            lines.iter().any(|l| l.contains("nell'Alveare")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("dalle 6 alle 15")),
            "{lines:?}"
        );
        assert!(lines.iter().any(|l| l.contains("la cuoca")), "{lines:?}");
        let price = scene(
            Band::High,
            ChatFact::Price {
                item: ItemKind::Attrezzo,
                market: "Il Bazar",
                price: 23,
                other: Some(("Alveare", 31)),
                own: false,
            },
        );
        let lines: Vec<String> = (0..30)
            .map(|s| answer(Intent::AskPrices, &price, s))
            .collect();
        assert!(lines.iter().any(|l| l.contains("al Bazar")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("23 gettoni")), "{lines:?}");
        let offer = scene(Band::Mid, ChatFact::FavourOffer(favour(2, 14)));
        let lines: Vec<String> = (0..30)
            .map(|s| answer(Intent::AskFavour, &offer, s))
            .collect();
        assert!(
            lines.iter().all(|l| l.contains("due barre di metallo")),
            "{lines:?}"
        );
        assert!(!lines.iter().any(|l| l.contains("serve due")), "{lines:?}");
        assert!(lines.iter().all(|l| l.contains("14 gettoni")), "{lines:?}");
        // A stranger doesn't know the player's name.
        let stranger = ChatScene {
            stranger: true,
            ..scene(Band::Mid, ChatFact::Nothing)
        };
        for s in 0..20 {
            assert!(!answer(Intent::Greet, &stranger, s).contains("Ada"));
            assert!(player_line(Intent::Greet, &stranger, s).contains("Ada"));
        }
    }
}
