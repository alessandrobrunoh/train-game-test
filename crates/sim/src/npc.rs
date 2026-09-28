//! NPC: bisogni, lavoro, inventario.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::action::Action;
use crate::carriage::{CarriageKind, StationKind};
use crate::ids::{CarriageId, NpcId};
use crate::item::ItemKind;
use crate::time::GameTime;

/// Needs in `0..=1`, where 1 means fully satisfied.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Needs {
    pub hunger: f32,
    pub energy: f32,
    pub social: f32,
}

impl Needs {
    pub fn clamp(&mut self) {
        self.hunger = self.hunger.clamp(0.0, 1.0);
        self.energy = self.energy.clamp(0.0, 1.0);
        self.social = self.social.clamp(0.0, 1.0);
    }
}

impl Default for Needs {
    fn default() -> Self {
        Self {
            hunger: 1.0,
            energy: 1.0,
            social: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Job {
    /// Works the grow beds of a Serra: grows Verdura.
    Contadino,
    /// Works the kitchen of a Mensa: cooks Verdura from the Serre into Razioni.
    Cuoco,
    /// Works a bench in an Officina: turns Rottame into Attrezzi and Vestiti.
    Operaio,
    /// Works the counter of a Mercato: brings Attrezzi and Vestiti from the
    /// Officine to the Mercato.
    Mercante,
}

impl Job {
    pub const ALL: [Job; 4] = [Job::Contadino, Job::Cuoco, Job::Operaio, Job::Mercante];

    pub fn name(self) -> &'static str {
        match self {
            Job::Contadino => "contadino",
            Job::Cuoco => "cuoco",
            Job::Operaio => "operaio",
            Job::Mercante => "mercante",
        }
    }

    pub fn workplace_kind(self) -> CarriageKind {
        match self {
            Job::Contadino => CarriageKind::Serra,
            Job::Cuoco => CarriageKind::Mensa,
            Job::Operaio => CarriageKind::Officina,
            Job::Mercante => CarriageKind::Mercato,
        }
    }

    pub fn station_kind(self) -> StationKind {
        match self {
            Job::Contadino => StationKind::GrowBed,
            Job::Cuoco => StationKind::Stove,
            Job::Operaio => StationKind::Workbench,
            Job::Mercante => StationKind::Counter,
        }
    }

    /// Whether an Attrezzo boosts (and wears with) this job's work.
    pub fn uses_tool(self) -> bool {
        matches!(self, Job::Contadino | Job::Operaio)
    }

    /// Work shift as `[start, end)` hours, interrupted by [`Job::LUNCH_BREAK`].
    pub fn shift(self) -> (u32, u32) {
        match self {
            Job::Contadino => (7, 16),
            Job::Cuoco => (6, 15),
            Job::Operaio => (8, 17),
            Job::Mercante => (9, 18),
        }
    }

    /// Lunch break `[start, end)` hours: no work, everyone gets a chance to eat.
    pub const LUNCH_BREAK: (u32, u32) = (12, 13);

    /// Whether `time` falls in working hours (shift minus lunch break).
    pub fn in_shift(self, time: GameTime) -> bool {
        let (start, end) = self.shift();
        let (break_start, break_end) = Self::LUNCH_BREAK;
        let hour = time.hour();
        (start..end).contains(&hour) && !(break_start..break_end).contains(&hour)
    }

    /// Minutes until the current stretch of work ends (lunch break or end of
    /// shift); 0 outside working hours.
    pub fn shift_minutes_left(self, time: GameTime) -> u64 {
        if !self.in_shift(time) {
            return 0;
        }
        let (break_start, _) = Self::LUNCH_BREAK;
        let end = if time.hour() < break_start {
            break_start
        } else {
            self.shift().1
        };
        u64::from(end * 60 - time.minute_of_day())
    }
}

impl fmt::Display for Job {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// What an NPC owns. Meals are free (eaten at a Mensa), so bulk items never
/// sit in personal inventories: only tokens and at most one Attrezzo and one
/// Vestito, each with a durability in `(0, 1]` (removed when it reaches 0).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Inventory {
    /// Wage tokens earned by working, spent at the Mercati.
    pub tokens: u32,
    /// Durability of the owned Attrezzo, if any.
    pub tool: Option<f32>,
    /// Durability of the owned Vestito, if any.
    pub clothes: Option<f32>,
}

impl Inventory {
    /// Durability of the owned unit of `item` (only Attrezzo and Vestito can be owned).
    pub fn durability(&self, item: ItemKind) -> Option<f32> {
        match item {
            ItemKind::Attrezzo => self.tool,
            ItemKind::Vestito => self.clothes,
            _ => None,
        }
    }

    pub fn has(&self, item: ItemKind) -> bool {
        self.durability(item).is_some()
    }

    pub(crate) fn slot_mut(&mut self, item: ItemKind) -> Option<&mut Option<f32>> {
        match item {
            ItemKind::Attrezzo => Some(&mut self.tool),
            ItemKind::Vestito => Some(&mut self.clothes),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Npc {
    pub id: NpcId,
    pub name: String,
    pub age: u32,
    /// Carriage the NPC is in. While travelling it stays the origin until arrival.
    pub carriage: CarriageId,
    /// Dormitorio the NPC lives in.
    pub home: CarriageId,
    pub job: Option<Job>,
    /// Carriage where the NPC works (matches `job.workplace_kind()`).
    pub workplace: Option<CarriageId>,
    pub needs: Needs,
    pub inventory: Inventory,
    pub action: Action,
    pub action_since: GameTime,
    pub action_until: GameTime,
    /// Consecutive minutes spent with hunger at 0.
    pub starving_minutes: u64,
}

impl Npc {
    /// Progress of the current action in `0..=1` (handy for animating travel).
    pub fn action_progress(&self, now: GameTime) -> f32 {
        let total = self.action_until.since(self.action_since);
        if total == 0 {
            1.0
        } else {
            (now.since(self.action_since) as f32 / total as f32).min(1.0)
        }
    }

    pub fn is_awake(&self) -> bool {
        !matches!(self.action, Action::Sleep(_))
    }

    /// Whether the NPC would buy `item` at a Mercato (tokens aside): a worker
    /// whose job uses tools without an Attrezzo, anyone without a Vestito.
    pub fn wants(&self, item: ItemKind) -> bool {
        match item {
            ItemKind::Attrezzo => {
                self.job.is_some_and(Job::uses_tool) && self.inventory.tool.is_none()
            }
            ItemKind::Vestito => self.inventory.clothes.is_none(),
            _ => false,
        }
    }

    /// Whether the NPC accepts `item` from the player ([`crate::World::player_give`]):
    /// food unless nearly full, an Attrezzo or Vestito only if it [`Npc::wants`] it.
    pub fn accepts_gift(&self, item: ItemKind) -> bool {
        match item {
            ItemKind::Razione | ItemKind::Verdura => self.needs.hunger < GIFT_FULL_HUNGER,
            ItemKind::Rottame => false,
            ItemKind::Attrezzo | ItemKind::Vestito => self.wants(item),
        }
    }
}

/// NPCs at least this full refuse food from the player.
pub const GIFT_FULL_HUNGER: f32 = 0.9;
