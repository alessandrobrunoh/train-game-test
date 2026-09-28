//! Identificatori tipizzati.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Stable NPC identifier. Never reused, also after the NPC dies.
///
/// It is *not* an index into `World::npcs` (dead NPCs are removed); use
/// [`crate::World::npc`] to look one up (binary search, `npcs` is kept sorted by id).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NpcId(pub u32);

/// Carriage identifier. It is also the carriage's position along the train
/// (0 = head) and its index into `World::carriages`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CarriageId(pub u16);

/// Station identifier, **local to its carriage**: it is the index into
/// `Carriage::stations`. A station is fully identified by `(CarriageId, StationId)`;
/// actions that use a station always happen in the NPC's current carriage, so
/// `Npc::carriage` + `StationId` is unambiguous.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StationId(pub u16);

impl CarriageId {
    pub fn index(self) -> usize {
        self.0 as usize
    }

    /// Number of carriages between `self` and `other`.
    pub fn distance(self, other: CarriageId) -> u32 {
        self.0.abs_diff(other.0) as u32
    }
}

impl StationId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl fmt::Display for NpcId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

impl fmt::Display for CarriageId {
    /// Human-facing carriage number (1-based).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0 + 1)
    }
}
