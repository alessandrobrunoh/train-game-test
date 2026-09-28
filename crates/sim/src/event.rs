//! Registro eventi della simulazione.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::{CarriageId, NpcId};
use crate::item::ItemKind;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeathCause {
    Starvation,
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] ", self.time)?;
        match &self.kind {
            EventKind::NpcStarving { name, .. } => write!(f, "{name} sta morendo di fame"),
            EventKind::NpcDied { name, cause, .. } => match cause {
                DeathCause::Starvation => write!(f, "{name} è morto di fame"),
            },
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
