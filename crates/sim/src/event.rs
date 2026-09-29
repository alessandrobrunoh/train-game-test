//! Registro eventi della simulazione.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::deliberation::{Choice, DeliberationId, DeliberationKind, Grievance, Resolver};
use crate::dialogue::{ConversationId, Tone, Topic};
use crate::ids::{CarriageId, NpcId};
use crate::item::ItemKind;
use crate::npc::{Job, Sex};
use crate::time::GameTime;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub time: GameTime,
    pub kind: EventKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EventKind {
    /// Hunger reached 0.
    NpcStarving { npc: NpcId, name: String },
    NpcDied {
        npc: NpcId,
        name: String,
        cause: DeathCause,
        /// Age in whole years at death.
        age: u32,
        sex: Sex,
    },
    /// A child was born to the couple `mother` + `father`.
    Born {
        npc: NpcId,
        name: String,
        sex: Sex,
        mother: NpcId,
        father: NpcId,
        mother_name: String,
        father_name: String,
    },
    /// Turned 18: got a job where the workforce was most needed (if any).
    CameOfAge {
        npc: NpcId,
        name: String,
        sex: Sex,
        job: Option<Job>,
    },
    /// Turned 65: left their job and lives on the stipend.
    Retired {
        npc: NpcId,
        name: String,
        sex: Sex,
        job: Option<Job>,
    },
    /// Two NPCs became partners; `moved_to` is the Dormitorio one of them
    /// moved into to live together (if they didn't already and there was room).
    Coupled {
        npc: NpcId,
        name: String,
        partner: NpcId,
        partner_name: String,
        moved_to: Option<CarriageId>,
    },
    /// `npc`'s partner died.
    Widowed {
        npc: NpcId,
        name: String,
        sex: Sex,
        partner: NpcId,
        partner_name: String,
    },
    /// The administration refused a couple a child. Logged at most once every
    /// `SimParams::birth_denied_log_days` (see `World::life` for the full count).
    BirthDenied {
        mother: NpcId,
        mother_name: String,
        father: NpcId,
        father_name: String,
        reason: BirthDenial,
    },
    /// No outlet ([`ItemKind::outlet`]) has a whole unit of `item` left.
    /// Tracked for Razione (Mense), Attrezzo and Vestito (Mercati); checked hourly.
    Shortage { item: ItemKind },
    /// `item` is available again in at least one outlet after a shortage.
    Restocked { item: ItemKind },
    /// An NPC bought one unit of `item` at the Mercato `carriage`.
    ItemBought {
        npc: NpcId,
        name: String,
        item: ItemKind,
        price: u32,
        carriage: CarriageId,
    },
    /// An owned Attrezzo broke or a Vestito wore out (durability reached 0).
    ItemBroke {
        npc: NpcId,
        name: String,
        item: ItemKind,
    },
    /// The player took `amount` units of `item` from the storage of `carriage`
    /// (see [`crate::World::player_take`]).
    PlayerTook {
        item: ItemKind,
        amount: u32,
        carriage: CarriageId,
    },
    /// The player bought one `item` at the Mercato `carriage`
    /// (see [`crate::World::player_buy`]).
    PlayerBought {
        item: ItemKind,
        price: u32,
        carriage: CarriageId,
    },
    /// The player gave one `item` to an NPC (see [`crate::World::player_give`]).
    PlayerGave {
        npc: NpcId,
        name: String,
        item: ItemKind,
    },
    /// A deliberation opened (see [`crate::Deliberation`]). Logged only for
    /// brains that answer deliberations: with the built-in rules alone it is
    /// resolved in the same minute and only `DeliberationResolved` is logged.
    DeliberationAsked {
        id: DeliberationId,
        npc: NpcId,
        name: String,
        kind: DeliberationKind,
        question: String,
    },
    /// `npc` decided: `description` is the chosen option's text.
    DeliberationResolved {
        id: DeliberationId,
        npc: NpcId,
        name: String,
        kind: DeliberationKind,
        choice: Choice,
        description: String,
        by: Resolver,
        /// The brain's confidence (None for the rules).
        confidence: Option<f32>,
    },
    /// `npc` stole `item` at the Mercato `carriage`; if `caught` the item
    /// stays there and `fine` tokens are paid.
    Theft {
        npc: NpcId,
        name: String,
        sex: Sex,
        item: ItemKind,
        carriage: CarriageId,
        caught: bool,
        fine: u32,
    },
    /// `npc` asked `helper` for tokens: `tokens` given (0: refused).
    HelpAsked {
        npc: NpcId,
        name: String,
        helper: NpcId,
        helper_name: String,
        tokens: u32,
    },
    /// A protest gathering was called in `place`, from `start` to `end`.
    ProtestCalled {
        grievance: Grievance,
        place: CarriageId,
        start: GameTime,
        end: GameTime,
    },
    /// The administration gave in to `protesters` recent protesters: more
    /// births allowed until `until` (BirthDenied), or emergency rations for
    /// the hungry (FoodShortage, `until` is None).
    AdminConceded {
        grievance: Grievance,
        protesters: u32,
        until: Option<GameTime>,
    },
    /// The treasury could not cover the day's wages and stipends: everyone
    /// got `paid_percent`% of what was due. Logged at most every
    /// `SimParams::economy_log_days`.
    Austerity { paid_percent: u32 },
    /// The administration changed the pay level (wages, stipends and Mercato
    /// prices), now `level_percent`% of the base, up if `raised`. Logged when it moved
    /// by 10 points or more, at most every `SimParams::economy_log_days`.
    PayChanged { level_percent: u32, raised: bool },
    /// A notable conversation started: a quarrel (`tone` Tense) or gossip
    /// about a theft. Logged at most every
    /// `SimParams::conversation_log_hours` per kind (see [`crate::Conversation`]).
    Chat {
        id: ConversationId,
        /// Who started it...
        npc: NpcId,
        name: String,
        /// ...with whom.
        other: NpcId,
        other_name: String,
        topic: Topic,
        tone: Tone,
        /// Whom they talk about, if anyone.
        about: Option<NpcId>,
        /// The line that sums it up.
        line: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeathCause {
    Starvation,
    OldAge,
}

impl DeathCause {
    pub const ALL: [DeathCause; 2] = [DeathCause::Starvation, DeathCause::OldAge];

    pub fn index(self) -> usize {
        self as usize
    }
}

/// Why the train administration refused a birth (checked in this order).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BirthDenial {
    /// The population reached `World::max_population`.
    Overcrowded,
    /// No Dormitorio has a free bed.
    NoBeds,
    /// Not enough Razioni in the Mense.
    NotEnoughFood,
}

impl fmt::Display for BirthDenial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BirthDenial::Overcrowded => "il treno è al completo",
            BirthDenial::NoBeds => "non ci sono cuccette libere",
            BirthDenial::NotEnoughFood => "le razioni non bastano",
        })
    }
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] ", self.time)?;
        match &self.kind {
            EventKind::NpcStarving { name, .. } => write!(f, "{name} sta morendo di fame"),
            EventKind::NpcDied {
                name,
                cause,
                age,
                sex,
                ..
            } => {
                let died = sex.pick("morta", "morto");
                match cause {
                    DeathCause::Starvation => write!(f, "{name} è {died} di fame a {age} anni"),
                    DeathCause::OldAge => write!(f, "{name} è {died} di vecchiaia a {age} anni"),
                }
            }
            EventKind::Born {
                name,
                sex,
                mother_name,
                father_name,
                ..
            } => write!(
                f,
                "È {} {name}, {} di {mother_name} e {father_name}",
                sex.pick("nata", "nato"),
                sex.pick("figlia", "figlio")
            ),
            EventKind::CameOfAge { name, sex, job, .. } => {
                write!(
                    f,
                    "{name} è {} maggiorenne",
                    sex.pick("diventata", "diventato")
                )?;
                match job {
                    Some(job) => write!(f, " e lavora come {job}"),
                    None => Ok(()),
                }
            }
            EventKind::Retired { name, sex, .. } => {
                write!(f, "{name} è {} in pensione", sex.pick("andata", "andato"))
            }
            EventKind::Coupled {
                name,
                partner_name,
                moved_to,
                ..
            } => {
                write!(f, "{name} e {partner_name} sono una coppia")?;
                match moved_to {
                    Some(c) => write!(f, " e vanno a vivere insieme (carrozza {c})"),
                    None => Ok(()),
                }
            }
            EventKind::Widowed {
                name,
                sex,
                partner_name,
                ..
            } => write!(
                f,
                "{name} è {} dopo la morte di {partner_name}",
                sex.pick("rimasta vedova", "rimasto vedovo")
            ),
            EventKind::BirthDenied {
                mother_name,
                father_name,
                reason,
                ..
            } => write!(
                f,
                "L'amministrazione nega un figlio a {mother_name} e {father_name}: {reason}"
            ),
            EventKind::Shortage { item } => match item {
                ItemKind::Razione => write!(f, "Carestia: nessuna Mensa ha più razioni"),
                item => write!(
                    f,
                    "Scarsità: nessun {} ha più {}",
                    item.outlet(),
                    item.plural()
                ),
            },
            EventKind::Restocked { item } => write!(
                f,
                "Di nuovo disponibili: {} ({})",
                item.plural(),
                item.outlet()
            ),
            EventKind::ItemBought {
                name, item, price, ..
            } => write!(
                f,
                "{name} ha comprato {} per {price} gettoni",
                item.with_article()
            ),
            EventKind::ItemBroke { name, item, .. } => match item {
                ItemKind::Vestito => write!(f, "Il vestito di {name} è ridotto a brandelli"),
                item => write!(f, "L'{} di {name} si è rotto", item.name()),
            },
            EventKind::PlayerTook {
                item,
                amount,
                carriage,
            } => match amount {
                1 => write!(
                    f,
                    "Hai preso {} dalle scorte della carrozza {carriage}",
                    item.with_article()
                ),
                n => write!(
                    f,
                    "Hai preso {n} {} dalle scorte della carrozza {carriage}",
                    item.plural()
                ),
            },
            EventKind::PlayerBought {
                item,
                price,
                carriage,
            } => write!(
                f,
                "Hai comprato {} per {price} gettoni (carrozza {carriage})",
                item.with_article()
            ),
            EventKind::PlayerGave { name, item, .. } => {
                write!(f, "Hai dato {} a {name}", item.with_article())
            }
            EventKind::DeliberationAsked { name, question, .. } => {
                write!(f, "{name} ci pensa: {question}")
            }
            EventKind::DeliberationResolved {
                name,
                kind,
                description,
                by,
                confidence,
                ..
            } => {
                write!(f, "{name} ha deciso ({}): {description}", kind.topic())?;
                match (by, confidence) {
                    (Resolver::Brain, Some(c)) => write!(f, " [cervello, {:.0}%]", c * 100.0),
                    (Resolver::Brain, None) => f.write_str(" [cervello]"),
                    (Resolver::Rules, _) => Ok(()),
                }
            }
            EventKind::Theft {
                name,
                sex,
                item,
                carriage,
                caught,
                fine,
                ..
            } => {
                if *caught {
                    write!(
                        f,
                        "{name} è {} a rubare {} al Mercato (carrozza {carriage}): multa di {fine} gettoni",
                        sex.pick("stata sorpresa", "stato sorpreso"),
                        item.with_article()
                    )
                } else {
                    write!(
                        f,
                        "{name} ha rubato {} al Mercato (carrozza {carriage}) senza farsi vedere",
                        item.with_article()
                    )
                }
            }
            EventKind::HelpAsked {
                name,
                helper_name,
                tokens,
                ..
            } => match tokens {
                0 => write!(
                    f,
                    "{helper_name} non aiuta {name}, che gli aveva chiesto qualche gettone"
                ),
                1 => write!(f, "{helper_name} aiuta {name} con 1 gettone"),
                n => write!(f, "{helper_name} aiuta {name} con {n} gettoni"),
            },
            EventKind::ProtestCalled {
                grievance,
                place,
                start,
                end,
            } => write!(
                f,
                "Protesta {} nella carrozza {place}, giorno {} dalle {:02}:{:02} alle {:02}:{:02}",
                grievance.against(),
                start.day(),
                start.hour(),
                start.minute(),
                end.hour(),
                end.minute()
            ),
            EventKind::AdminConceded {
                grievance,
                protesters,
                until,
            } => {
                write!(
                    f,
                    "L'amministrazione cede alle proteste ({protesters} persone): "
                )?;
                match (grievance, until) {
                    (Grievance::BirthDenied, Some(t)) => {
                        write!(f, "più nascite consentite fino al giorno {}", t.day())
                    }
                    (Grievance::BirthDenied, None) => f.write_str("più nascite consentite"),
                    (Grievance::FoodShortage, _) => {
                        f.write_str("razioni d'emergenza per chi ha fame")
                    }
                }
            }
            EventKind::Austerity { paid_percent } => write!(
                f,
                "Tesoreria a secco: l'amministrazione paga solo il {paid_percent}% di salari e sussidi"
            ),
            EventKind::PayChanged {
                level_percent,
                raised,
            } => write!(
                f,
                "L'amministrazione {} paghe e prezzi: ora al {level_percent}% dei valori base",
                if *raised { "alza" } else { "abbassa" }
            ),
            EventKind::Chat {
                name,
                other_name,
                topic,
                tone,
                line,
                ..
            } => match (tone, topic) {
                (Tone::Tense, _) => write!(f, "{name} e {other_name} litigano: «{line}»"),
                (_, Topic::Gossip) => write!(f, "{name} spettegola con {other_name}: «{line}»"),
                _ => write!(
                    f,
                    "{name} e {other_name} parlano di {}: «{line}»",
                    topic.name()
                ),
            },
        }
    }
}
