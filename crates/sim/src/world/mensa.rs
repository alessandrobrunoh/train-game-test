//! Mense: turni, code per un posto a tavola e affollamento.
//!
//! - Meal shifts ([`crate::Npc::meal_shift`]) spread breakfast, lunch and
//!   dinner (and the workers' lunch break) over `meal_shifts` windows.
//! - Who can't find a seat queues ([`Action::Wait`]) and is seated at the
//!   first free table, first come first served ([`World::serve_queues`]).
//! - Going to eat elsewhere, the NPC picks the Mensa with the shortest travel
//!   plus expected wait, counting who is already queuing or on the way.
//! - A Mensa with many people idling or chatting is crowded
//!   ([`World::is_crowded`]): brains avoid lingering there and nobody goes
//!   there just to chat.
//!
//! [`World::mensa_occupancy`] is a read-only snapshot for tests, the headless
//! example and the UI.

use crate::action::{Action, ActionOption};
use crate::carriage::{Carriage, CarriageKind, StationKind};
use crate::ids::CarriageId;
use crate::item::ItemKind;
use crate::npc::Npc;

use super::World;

/// Below this hunger an NPC idling in a full Mensa counts as waiting in
/// [`World::mensa_role`] (the default brain eats at meal times below 0.7).
pub const WANTS_TO_EAT_BELOW: f32 = 0.7;

/// What an NPC in a Mensa is doing there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MensaRole {
    /// Seated at a table.
    Eating,
    /// Queuing for a seat ([`Action::Wait`]), or hungry and idling/chatting
    /// while every seat is taken.
    Waiting,
    /// Idling or chatting, not waiting to eat.
    Loitering,
    /// A cook at a stove.
    Working,
    /// Anything else (leaving, buying).
    Other,
}

/// Who is in one Mensa right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MensaOccupancy {
    pub carriage: CarriageId,
    pub tables: usize,
    /// Seats at all tables (tables × capacity).
    pub seats: usize,
    /// NPCs in the carriage (travellers leaving it included).
    pub present: usize,
    /// Per [`MensaRole`].
    pub eating: usize,
    pub waiting: usize,
    pub loitering: usize,
    pub working: usize,
}

impl MensaOccupancy {
    pub fn free_seats(&self) -> usize {
        self.seats.saturating_sub(self.eating)
    }
}

/// Per carriage, as of the last tick with decisions: people queuing, idling
/// or chatting, and travelling there.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct CarriageLoad {
    queued: u16,
    lingering: u16,
    incoming: u16,
}

/// Seats at the tables of `c` and how many are taken.
fn seats(c: &Carriage) -> (usize, usize) {
    c.stations
        .iter()
        .filter(|s| s.kind == StationKind::Table)
        .fold((0, 0), |(seats, taken), s| {
            (
                seats + usize::from(s.capacity),
                taken + usize::from(s.occupancy),
            )
        })
}

impl World {
    /// Whether `npc` is in working hours now (its job's shift minus the lunch
    /// break of its meal shift).
    pub fn works_now(&self, npc: &Npc) -> bool {
        let lunch = self.params.lunch_break(self.params.meal_shift(npc));
        npc.job.is_some_and(|job| job.works_at(self.clock, lunch))
    }

    /// People queuing for a seat in `carriage` (as of the last tick with decisions).
    pub fn queue_len(&self, carriage: CarriageId) -> usize {
        self.load
            .get(carriage.index())
            .map_or(0, |l| usize::from(l.queued))
    }

    /// Whether `carriage` is a Mensa with at least `mensa_crowd` people
    /// idling or chatting in it (as of the last tick with decisions).
    pub fn is_crowded(&self, carriage: CarriageId) -> bool {
        self.carriage(carriage)
            .is_some_and(|c| c.kind == CarriageKind::Mensa)
            && self
                .load
                .get(carriage.index())
                .is_some_and(|l| l.lingering >= self.params.mensa_crowd)
    }

    /// Minutes someone arriving now at `carriage` would wait for a seat,
    /// counting who is queuing or already on the way there (0 with free
    /// seats for everyone; `None` if it is not a Mensa with tables).
    pub fn expected_wait_minutes(&self, carriage: CarriageId) -> Option<u64> {
        let c = self.carriage(carriage)?;
        if c.kind != CarriageKind::Mensa {
            return None;
        }
        let (seats, taken) = seats(c);
        if seats == 0 {
            return None;
        }
        let load = self.load.get(carriage.index()).copied().unwrap_or_default();
        let ahead = usize::from(load.queued) + usize::from(load.incoming);
        let free = seats - taken.min(seats);
        if ahead < free {
            return Some(0);
        }
        // A seat frees up every `eat_minutes / seats` minutes on average.
        let turns = (ahead - free + 1) as u64;
        Some((turns * self.params.eat_minutes).div_ceil(seats as u64))
    }

    /// The best other Mensa to go and eat from `here`: food in stock, and the
    /// shortest travel plus expected wait (nearest first on ties).
    pub(super) fn mensa_for_meal(&self, here: CarriageId) -> Option<CarriageId> {
        let per_meal = self.params.razioni_per_meal;
        self.carriages
            .iter()
            .filter(|c| {
                c.id != here
                    && c.kind == CarriageKind::Mensa
                    && c.stock.has(ItemKind::Razione, per_meal)
            })
            .filter_map(|c| {
                let wait = self.expected_wait_minutes(c.id)?;
                let cost = self.travel_minutes(here, c.id) + wait;
                Some(((cost, c.id.distance(here), c.id.0), c.id))
            })
            .min_by_key(|&(key, _)| key)
            .map(|(_, id)| id)
    }

    /// Someone started queuing in `carriage` (keeps the queue count current
    /// within a tick, for [`World::queue_len`]).
    pub(super) fn joined_queue(&mut self, carriage: CarriageId) {
        if let Some(l) = self.load.get_mut(carriage.index()) {
            l.queued += 1;
        }
    }

    /// Refreshes the per-carriage queue / lingering / incoming counts.
    pub(super) fn refresh_load(&mut self) {
        self.load.clear();
        self.load
            .resize(self.carriages.len(), CarriageLoad::default());
        for npc in &self.npcs {
            match npc.action {
                Action::Wait => self.load[npc.carriage.index()].queued += 1,
                Action::Idle | Action::Socialize(_) => {
                    self.load[npc.carriage.index()].lingering += 1;
                }
                Action::Travel { to } => {
                    if let Some(l) = self.load.get_mut(to.index()) {
                        l.incoming += 1;
                    }
                }
                _ => {}
            }
        }
    }

    /// Seats queuing NPCs at tables that freed up, in arrival order (then by
    /// id); a Mensa out of Razioni serves nobody.
    pub(super) fn serve_queues(&mut self) {
        let mut queued: Vec<usize> = (0..self.npcs.len())
            .filter(|&i| self.npcs[i].action == Action::Wait)
            .collect();
        if queued.is_empty() {
            return;
        }
        queued.sort_by_key(|&i| (self.npcs[i].action_since, self.npcs[i].id));
        for i in queued {
            let c = &self.carriages[self.npcs[i].carriage.index()];
            if !c.stock.has(ItemKind::Razione, self.params.razioni_per_meal) {
                continue;
            }
            let Some(table) = c.roomiest_station(StationKind::Table) else {
                continue;
            };
            let seat = ActionOption {
                action: Action::Eat(table),
                minutes: self.params.eat_minutes,
                goal: None,
                description: String::new(),
            };
            self.start_action(i, Some(&seat));
        }
    }

    /// What `npc` is doing in the Mensa it is in (None outside the Mense).
    pub fn mensa_role(&self, npc: &Npc) -> Option<MensaRole> {
        let c = self.carriage(npc.carriage)?;
        if c.kind != CarriageKind::Mensa {
            return None;
        }
        let full = !c.has_free(StationKind::Table);
        Some(match npc.action {
            Action::Eat(_) => MensaRole::Eating,
            Action::Work(_) => MensaRole::Working,
            Action::Wait => MensaRole::Waiting,
            Action::Idle | Action::Socialize(_)
                if full && npc.needs.hunger < WANTS_TO_EAT_BELOW =>
            {
                MensaRole::Waiting
            }
            Action::Idle | Action::Socialize(_) => MensaRole::Loitering,
            _ => MensaRole::Other,
        })
    }

    /// One entry per Mensa, head to tail.
    pub fn mensa_occupancy(&self) -> Vec<MensaOccupancy> {
        let mut out: Vec<MensaOccupancy> = self
            .carriages
            .iter()
            .filter(|c| c.kind == CarriageKind::Mensa)
            .map(|c| MensaOccupancy {
                carriage: c.id,
                tables: c
                    .stations
                    .iter()
                    .filter(|s| s.kind == StationKind::Table)
                    .count(),
                seats: seats(c).0,
                present: 0,
                eating: 0,
                waiting: 0,
                loitering: 0,
                working: 0,
            })
            .collect();
        for npc in &self.npcs {
            let Some(role) = self.mensa_role(npc) else {
                continue;
            };
            let Some(m) = out.iter_mut().find(|m| m.carriage == npc.carriage) else {
                continue;
            };
            m.present += 1;
            match role {
                MensaRole::Eating => m.eating += 1,
                MensaRole::Waiting => m.waiting += 1,
                MensaRole::Loitering => m.loitering += 1,
                MensaRole::Working => m.working += 1,
                MensaRole::Other => {}
            }
        }
        out
    }
}

impl World {
    /// e.g. "Turno mensa 2: colazione alle 06:40, pranzo alle 12:40, cena alle 19:40."
    pub(super) fn meal_context(&self, npc: &Npc) -> String {
        let p = &self.params;
        let shift = p.meal_shift(npc);
        let at = |meal: usize| {
            let (start, _) = p.meal_window(meal, shift);
            format!("{:02}:{:02}", start / 60, start % 60)
        };
        format!(
            "Turno mensa {}: colazione alle {}, pranzo alle {}, cena alle {}.",
            shift + 1,
            at(0),
            at(1),
            at(2)
        )
    }
}
