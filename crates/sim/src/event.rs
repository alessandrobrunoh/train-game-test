//! Registro eventi della simulazione.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::NpcId;
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
    /// No Mensa has food left.
    FoodShortage,
    /// Food is available again in at least one Mensa after a shortage.
    FoodRestocked,
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
            EventKind::FoodShortage => write!(f, "Carestia: nessuna Mensa ha più cibo"),
            EventKind::FoodRestocked => write!(f, "Le Mense sono di nuovo rifornite"),
        }
    }
}
