//! Registro eventi della simulazione.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::combat::{Fighter, Motive};
use crate::deliberation::{Choice, DeliberationId, DeliberationKind, Grievance, Resolver};
use crate::dialogue::{ConversationId, Tone, Topic};
use crate::gang::{DisbandReason, GangId, LeaveReason};
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
    /// The player sold one `item` for `price` tokens to the Mercato
    /// `carriage` (see [`crate::World::player_sell`]).
    PlayerSold {
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
    /// `attacker` hit `victim` in `place` for `damage` hit points (0: the
    /// blow missed). `first` marks the opening blow of a fight (logged even
    /// when it misses; later blows only when they land). See [`crate::combat`].
    Attacked {
        attacker: Fighter,
        attacker_name: String,
        victim: Fighter,
        victim_name: String,
        damage: u32,
        place: CarriageId,
        motive: Motive,
        first: bool,
    },
    /// `killer` killed `victim` in `place` (by a blow, or `victim` died of
    /// the wounds it left). Followed by the `NpcDied` event of the victim.
    Killed {
        killer: Fighter,
        killer_name: String,
        victim: Fighter,
        victim_name: String,
        place: CarriageId,
    },
    /// The player fainted in `place` (health at 0): it wakes up in its
    /// cabin, having lost `tokens` tokens and `item` to `by` (if an NPC
    /// knocked it down; else to the treasury).
    Fainted {
        by: Option<NpcId>,
        by_name: Option<String>,
        place: CarriageId,
        tokens: u32,
        item: Option<ItemKind>,
        /// With [`crate::SimParams::permadeath`]: the player died (game over).
        dead: bool,
    },
    /// A gang was founded in `place` (see [`crate::gang`]), or split from
    /// another (`split_from`) when its leader died.
    GangFounded {
        gang: GangId,
        gang_name: String,
        leader: NpcId,
        leader_name: String,
        members: u32,
        place: CarriageId,
        split_from: Option<String>,
    },
    /// `npc` joined a gang.
    GangJoined {
        gang: GangId,
        gang_name: String,
        npc: NpcId,
        name: String,
    },
    /// `npc` (None: the player) left a gang.
    GangLeft {
        gang: GangId,
        gang_name: String,
        npc: Option<NpcId>,
        name: String,
        reason: LeaveReason,
    },
    /// A gang member asked `victim` for the pizzo in `place`: `tokens` paid
    /// into the gang's treasury, or refused (`paid` false: a beating
    /// follows, see [`crate::Motive::Pizzo`]).
    GangExtortion {
        gang: GangId,
        gang_name: String,
        collector: Fighter,
        collector_name: String,
        victim: Fighter,
        victim_name: String,
        tokens: u32,
        paid: bool,
        place: CarriageId,
    },
    /// A killing ordered by a gang's leader was done: `killer` killed
    /// `victim` (after the `Killed` event).
    GangHit {
        gang: GangId,
        gang_name: String,
        killer: NpcId,
        killer_name: String,
        victim: Fighter,
        victim_name: String,
        place: CarriageId,
    },
    /// A gang has a new leader (its leader died); `contested`: a contender
    /// split away with its followers.
    GangLeader {
        gang: GangId,
        gang_name: String,
        leader: NpcId,
        leader_name: String,
        contested: bool,
    },
    /// A gang disbanded (too few members, or merged into `into_name`).
    GangDisbanded {
        gang: GangId,
        gang_name: String,
        reason: DisbandReason,
        into_name: Option<String>,
    },
    /// The player joined a gang, invited by `by`.
    PlayerJoinedGang {
        gang: GangId,
        gang_name: String,
        by: NpcId,
        by_name: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeathCause {
    /// Fame: health drained by long starvation.
    Starvation,
    OldAge,
    /// Violenza: killed by a blow.
    Violence,
    /// Ferite: died of the wounds left by a fight.
    Wounds,
}

impl DeathCause {
    pub const COUNT: usize = 4;
    pub const ALL: [DeathCause; Self::COUNT] = [
        DeathCause::Starvation,
        DeathCause::OldAge,
        DeathCause::Violence,
        DeathCause::Wounds,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    /// "di fame", "di vecchiaia", "uccisa"/"ucciso", "per le ferite"
    /// (after "morto").
    pub fn how(self, sex: Sex) -> &'static str {
        match self {
            DeathCause::Starvation => "di fame",
            DeathCause::OldAge => "di vecchiaia",
            DeathCause::Violence => sex.pick("uccisa", "ucciso"),
            DeathCause::Wounds => "per le ferite",
        }
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
                    DeathCause::Violence => write!(
                        f,
                        "{name} è {} a {age} anni",
                        sex.pick("stata uccisa", "stato ucciso")
                    ),
                    DeathCause::Wounds => write!(f, "{name} è {died} per le ferite a {age} anni"),
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
                item if *item == ItemKind::Razione => {
                    write!(f, "Carestia: nessuna Mensa ha più razioni")
                }
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
                item if *item == ItemKind::Vestito => {
                    write!(f, "Il vestito di {name} è ridotto a brandelli")
                }
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
            EventKind::PlayerSold {
                item,
                price,
                carriage,
            } => write!(
                f,
                "Hai venduto {} per {price} gettoni (carrozza {carriage})",
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
            EventKind::Attacked {
                attacker_name,
                victim_name,
                damage,
                place,
                motive,
                first,
                ..
            } => match (first, damage) {
                (true, 0) => write!(
                    f,
                    "{attacker_name} aggredisce {victim_name} nella carrozza {place} ({}), ma manca il colpo",
                    motive.name()
                ),
                (true, d) => write!(
                    f,
                    "{attacker_name} aggredisce {victim_name} nella carrozza {place} ({}): -{d} salute",
                    motive.name()
                ),
                (false, d) => write!(f, "{attacker_name} colpisce {victim_name}: -{d} salute"),
            },
            EventKind::Killed {
                killer_name,
                victim_name,
                place,
                ..
            } => write!(
                f,
                "{killer_name} ha ucciso {victim_name} nella carrozza {place}"
            ),
            EventKind::Fainted {
                by_name,
                tokens,
                item,
                dead,
                ..
            } => {
                if *dead {
                    return match by_name {
                        Some(by) => write!(f, "Sei morto: ti ha ucciso {by}"),
                        None => f.write_str("Sei morto"),
                    };
                }
                match by_name {
                    Some(by) => write!(f, "{by} ti ha messo al tappeto: sei svenuto")?,
                    None => f.write_str("Sei svenuto")?,
                }
                match (tokens, item) {
                    (0, None) => Ok(()),
                    (t, None) => write!(f, " e hai perso {t} gettoni"),
                    (0, Some(i)) => write!(f, " e hai perso {}", i.with_article()),
                    (t, Some(i)) => {
                        write!(f, " e hai perso {t} gettoni e {}", i.with_article())
                    }
                }
            }
            EventKind::GangFounded {
                gang_name,
                leader_name,
                members,
                place,
                split_from,
                ..
            } => match split_from {
                Some(from) => write!(
                    f,
                    "Scissione: {leader_name} lascia «{from}» e fonda «{gang_name}» ({members} membri, carrozza {place})"
                ),
                None => write!(
                    f,
                    "Nasce una banda: «{gang_name}», {members} membri con {leader_name} a capo (carrozza {place})"
                ),
            },
            EventKind::GangJoined {
                gang_name, name, ..
            } => write!(f, "{name} entra nella banda «{gang_name}»"),
            EventKind::GangLeft {
                gang_name,
                npc,
                name,
                reason,
                ..
            } => match npc {
                None => write!(f, "Hai lasciato la banda «{gang_name}»"),
                Some(_) => write!(f, "{name} lascia la banda «{gang_name}» {}", reason.label()),
            },
            EventKind::GangExtortion {
                gang_name,
                collector,
                collector_name,
                victim,
                victim_name,
                tokens,
                paid,
                ..
            } => match (collector, victim, paid) {
                (Fighter::Player, _, _) => write!(
                    f,
                    "Hai riscosso {tokens} gettoni di pizzo da {victim_name} per «{gang_name}»"
                ),
                (_, Fighter::Player, true) => write!(
                    f,
                    "Hai pagato {tokens} gettoni di pizzo a «{gang_name}» ({collector_name})"
                ),
                (_, Fighter::Player, false) => {
                    write!(f, "Ti rifiuti di pagare il pizzo a «{gang_name}»")
                }
                (_, _, true) => write!(
                    f,
                    "{collector_name} riscuote {tokens} gettoni di pizzo da {victim_name} per «{gang_name}»"
                ),
                (_, _, false) => write!(
                    f,
                    "{victim_name} si rifiuta di pagare il pizzo a {collector_name} («{gang_name}»)"
                ),
            },
            EventKind::GangHit {
                gang_name,
                killer_name,
                victim_name,
                place,
                ..
            } => write!(
                f,
                "Regolamento di conti: {killer_name} ha ucciso {victim_name} per ordine del capo di «{gang_name}» (carrozza {place})"
            ),
            EventKind::GangLeader {
                gang_name,
                leader_name,
                contested,
                ..
            } => {
                write!(f, "{leader_name} è il nuovo capo di «{gang_name}»")?;
                if *contested {
                    f.write_str(" dopo una lotta per il comando")?;
                }
                Ok(())
            }
            EventKind::GangDisbanded {
                gang_name,
                reason,
                into_name,
                ..
            } => match (reason, into_name) {
                (DisbandReason::Merged(_), Some(into)) => {
                    write!(f, "La banda «{gang_name}» si unisce a «{into}»")
                }
                _ => write!(f, "La banda «{gang_name}» si scioglie"),
            },
            EventKind::PlayerJoinedGang {
                gang_name, by_name, ..
            } => write!(
                f,
                "Sei entrato nella banda «{gang_name}» (ti ha invitato {by_name})"
            ),
        }
    }
}
