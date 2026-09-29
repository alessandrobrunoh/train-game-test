//! Azioni degli NPC.

use serde::{Deserialize, Serialize};

use crate::combat::Fighter;
use crate::ids::{CarriageId, NpcId, StationId};
use crate::item::ItemKind;

/// What an NPC is doing. Station ids are local to the NPC's current carriage.
///
/// Effects:
/// - `Eat`: takes one Razione from the Mensa stock at start (free), restores hunger while running.
/// - `Sleep`: restores energy while running (no energy decay).
/// - `Work`: produces/moves items and pays a wage when it completes.
/// - `Buy`: at a Mercato, pays the price and takes one unit at start (no station).
/// - `Travel`: moves the NPC to `to` when it completes.
/// - `Socialize`: restores social while running. A free partner switches to
///   `Socialize` back with the same end and a [`crate::Conversation`] opens
///   (a partner who is eating talks along and gets a bonus at the end); a
///   busy one gets a short one-sided chat and the bonus.
/// - `Idle`: nothing, short filler.
/// - `Wait`: queues for a seat in a Mensa whose tables are full ("aspetta un
///   posto in mensa"): seated (`Eat`) as soon as one frees up, first come
///   first served, or decides again after `queue_patience_minutes`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Action {
    Eat(StationId),
    Sleep(StationId),
    Work(StationId),
    Travel { to: CarriageId },
    Socialize(NpcId),
    Buy(ItemKind),
    Idle,
    Wait,
    Attack(Fighter),
}

/// Coarse action category, for stats and rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ActionKind {
    Eat,
    Sleep,
    Work,
    Travel,
    Socialize,
    Buy,
    Idle,
    Wait,
    Attack,
}

impl ActionKind {
    pub const ALL: [ActionKind; 9] = [
        ActionKind::Eat,
        ActionKind::Sleep,
        ActionKind::Work,
        ActionKind::Travel,
        ActionKind::Socialize,
        ActionKind::Buy,
        ActionKind::Idle,
        ActionKind::Wait,
        ActionKind::Attack,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ActionKind::Eat => "mangia",
            ActionKind::Sleep => "dorme",
            ActionKind::Work => "lavora",
            ActionKind::Travel => "viaggia",
            ActionKind::Socialize => "socializza",
            ActionKind::Buy => "compra",
            ActionKind::Idle => "ozia",
            ActionKind::Wait => "aspetta",
            ActionKind::Attack => "combatte",
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
            Action::Buy(_) => ActionKind::Buy,
            Action::Idle => ActionKind::Idle,
            Action::Wait => ActionKind::Wait,
            Action::Attack(_) => ActionKind::Attack,
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
