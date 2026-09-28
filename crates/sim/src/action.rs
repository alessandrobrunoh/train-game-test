//! Azioni degli NPC.

use serde::{Deserialize, Serialize};

use crate::ids::{CarriageId, NpcId, StationId};

/// What an NPC is doing. Station ids are local to the NPC's current carriage.
///
/// Effects:
/// - `Eat`: takes one portion from the Mensa stock at start, restores hunger while running.
/// - `Sleep`: restores energy while running (no energy decay).
/// - `Work`: produces resources and pays a wage when it completes.
/// - `Travel`: moves the NPC to `to` when it completes.
/// - `Socialize`: restores social while running; the partner gets a bonus at the end.
/// - `Idle`: nothing, short filler.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Action {
    Eat(StationId),
    Sleep(StationId),
    Work(StationId),
    Travel { to: CarriageId },
    Socialize(NpcId),
    Idle,
}

/// Coarse action category, for stats and rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ActionKind {
    Eat,
    Sleep,
    Work,
    Travel,
    Socialize,
    Idle,
}

impl ActionKind {
    pub const ALL: [ActionKind; 6] = [
        ActionKind::Eat,
        ActionKind::Sleep,
        ActionKind::Work,
        ActionKind::Travel,
        ActionKind::Socialize,
        ActionKind::Idle,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ActionKind::Eat => "mangia",
            ActionKind::Sleep => "dorme",
            ActionKind::Work => "lavora",
            ActionKind::Travel => "viaggia",
            ActionKind::Socialize => "socializza",
            ActionKind::Idle => "ozia",
        }
    }
}

impl Action {
    pub fn kind(&self) -> ActionKind {
        match self {
            Action::Eat(_) => ActionKind::Eat,
            Action::Sleep(_) => ActionKind::Sleep,
            Action::Work(_) => ActionKind::Work,
            Action::Travel { .. } => ActionKind::Travel,
            Action::Socialize(_) => ActionKind::Socialize,
            Action::Idle => ActionKind::Idle,
        }
    }

    /// Station occupied by this action, if any (in the NPC's current carriage).
    pub fn station(&self) -> Option<StationId> {
        match *self {
            Action::Eat(s) | Action::Sleep(s) | Action::Work(s) => Some(s),
            _ => None,
        }
    }
}

/// One candidate the sim offers to a [`crate::Brain`].
#[derive(Clone, Debug, PartialEq)]
pub struct ActionOption {
    pub action: Action,
    /// Planned duration in minutes (already decided by the sim).
    pub minutes: u64,
    /// For `Travel`: what the NPC means to do at the destination.
    pub goal: Option<ActionKind>,
    /// Short natural-language description, e.g. "mangia in Mensa «Il Refettorio»
    /// (carrozza 2)". Empty if the brain opted out via `Brain::wants_descriptions`.
    pub description: String,
}

/// All candidates for one NPC that needs a decision this tick.
#[derive(Clone, Debug, PartialEq)]
pub struct DecisionRequest {
    pub npc: NpcId,
    pub options: Vec<ActionOption>,
}
