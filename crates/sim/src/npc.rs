//! NPC: bisogni, lavoro, inventario.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::action::Action;
use crate::carriage::{CarriageKind, StationKind};
use crate::ids::{CarriageId, NpcId};
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
    /// Works the grow beds of a Serra: produces food.
    Contadino,
    /// Works the kitchen of a Mensa: brings food from the Serre into the Mensa.
    Cuoco,
    /// Works a bench in an Officina: produces materials.
    Operaio,
}

impl Job {
    pub const ALL: [Job; 3] = [Job::Contadino, Job::Cuoco, Job::Operaio];

    pub fn name(self) -> &'static str {
        match self {
            Job::Contadino => "contadino",
            Job::Cuoco => "cuoco",
            Job::Operaio => "operaio",
        }
    }

    pub fn workplace_kind(self) -> CarriageKind {
        match self {
            Job::Contadino => CarriageKind::Serra,
            Job::Cuoco => CarriageKind::Mensa,
            Job::Operaio => CarriageKind::Officina,
        }
    }

    pub fn station_kind(self) -> StationKind {
        match self {
            Job::Contadino => StationKind::GrowBed,
            Job::Cuoco => StationKind::Stove,
            Job::Operaio => StationKind::Workbench,
        }
    }

    /// Work shift as `[start, end)` hours, interrupted by [`Job::LUNCH_BREAK`].
    pub fn shift(self) -> (u32, u32) {
        match self {
            Job::Contadino => (7, 16),
            Job::Cuoco => (6, 15),
            Job::Operaio => (8, 17),
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    /// Wage tokens earned by working (no use yet).
    pub tokens: u32,
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
}
