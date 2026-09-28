//! Registro eventi della simulazione.

use std::fmt;

use serde::{Deserialize, Serialize};

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
        }
    }
}
