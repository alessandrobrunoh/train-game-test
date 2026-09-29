//! Salute: guarigione, fame che consuma, chi è grave resta a letto e viene
//! accudito (vedi [`crate::combat`]).
//!
//! Every minute ([`World::update_needs`]) health comes back by
//! [`crate::SimParams::heal_per_minute`] (faster asleep and well fed),
//! wounds close with it; starving past
//! [`crate::SimParams::starvation_grace_minutes`] drains it instead, and a
//! badly wounded NPC out of bed bleeds. Nothing here draws randomness, and
//! nothing changes for who is in full health.

use super::World;
use crate::action::{Action, ActionKind, ActionOption};
use crate::carriage::{CarriageKind, StationKind};
use crate::combat::MAX_HEALTH;
use crate::defs::ItemUse;
use crate::ids::CarriageId;
use crate::item::ItemKind;
use crate::npc::{LifeStage, Npc, RelationKind};
use crate::params::SimParams;

/// Bedridden NPCs hungrier than this get food from family and friends.
const CARE_HUNGER: f32 = 0.5;
/// Friends at least this close bring food too.
const CARE_AFFINITY: f32 = 0.3;
/// A bedridden NPC this hungry walks to a Mensa anyway.
const BEDRIDDEN_EATS_OUT: f32 = 0.25;
/// Idle filler of a bedridden NPC (no randomness).
const BEDRIDDEN_IDLE_MINUTES: u64 = 15;

/// What a minute did to an NPC's health.
pub(super) enum Health {
    Fine,
    /// It bled out (0 health from its wounds).
    BledOut,
}

/// One minute of healing (or bleeding) for `npc`, not starving. Only
/// called below full health.
pub(super) fn recover(p: &SimParams, npc: &mut Npc) -> Health {
    let asleep = matches!(npc.action, Action::Sleep(_));
    if npc.health < p.bleed_below && npc.injury > 0.0 && !asleep {
        npc.health = (npc.health - p.bleed_per_minute).max(0.0);
        return if npc.health <= 0.0 {
            Health::BledOut
        } else {
            Health::Fine
        };
    }
    let rate = heal_rate(p, asleep, npc.needs.hunger);
    npc.health = (npc.health + rate).min(MAX_HEALTH);
    npc.injury = (npc.injury - rate).clamp(0.0, MAX_HEALTH - npc.health);
    Health::Fine
}

/// Health regained in a minute: faster in bed and well fed.
pub(super) fn heal_rate(p: &SimParams, asleep: bool, hunger: f32) -> f32 {
    let mut rate = p.heal_per_minute;
    if asleep {
        rate *= p.bed_heal_factor;
    }
    if hunger >= 0.6 {
        rate *= p.fed_heal_factor;
    }
    rate
}

/// Health lost per minute of starvation past the grace period.
pub(super) fn starvation_drain(p: &SimParams) -> f32 {
    let span = p
        .starvation_minutes
        .saturating_sub(p.starvation_grace_minutes)
        .max(1);
    MAX_HEALTH / span as f32
}

impl World {
    /// Options of a bedridden NPC ([`crate::SimParams::bedridden_below`]):
    /// to bed at home (or the nearest free bed), eating where it is, and
    /// only when very hungry to a Mensa. No work, no errands.
    pub(super) fn bedridden_options(&self, i: usize) -> Vec<ActionOption> {
        let p = &self.params;
        let now = self.clock;
        let npc = &self.npcs[i];
        let here = npc.carriage;
        let carriage = &self.carriages[here.index()];
        let mut options = Vec::with_capacity(4);
        let mut push = |action, minutes: u64, goal| {
            options.push(ActionOption {
                action,
                minutes: minutes.max(1),
                goal,
                description: String::new(),
            });
        };
        push(Action::Idle, BEDRIDDEN_IDLE_MINUTES, None);
        if let Some(bed) = carriage.free_station_for(StationKind::Bed, npc.id) {
            let minutes = if p.is_long_sleep(now.hour()) {
                now.next_at(p.wake_hour, 0) - now
            } else {
                2 * p.nap_minutes
            };
            push(Action::Sleep(bed), minutes, None);
        }
        let can_eat_here = carriage.kind == CarriageKind::Mensa
            && carriage.stock.has(ItemKind::Razione, p.razioni_per_meal)
            && carriage.has_free(StationKind::Table);
        if can_eat_here && let Some(table) = carriage.roomiest_station(StationKind::Table) {
            push(Action::Eat(table), p.eat_minutes, None);
        }
        let has_bed = |c: CarriageId| self.carriages[c.index()].has_free(StationKind::Bed);
        if here != npc.home && has_bed(npc.home) {
            let to = npc.home;
            push(
                Action::Travel { to },
                self.walk_minutes(i, to),
                Some(ActionKind::Sleep),
            );
        } else if !has_bed(here)
            && let Some(to) = self
                .carriages
                .iter()
                .filter(|c| c.id != here && c.has_free(StationKind::Bed))
                .min_by_key(|c| (c.id.distance(here), c.id))
                .map(|c| c.id)
        {
            push(
                Action::Travel { to },
                self.walk_minutes(i, to),
                Some(ActionKind::Sleep),
            );
        }
        if !can_eat_here
            && npc.needs.hunger < BEDRIDDEN_EATS_OUT
            && let Some(to) = self.mensa_for_meal(here)
        {
            push(
                Action::Travel { to },
                self.walk_minutes(i, to),
                Some(ActionKind::Eat),
            );
        }
        options
    }

    /// Hourly: hungry bedridden NPCs get a meal from their partner, family
    /// or a close friend who is up and about: something the carer carries,
    /// else a Razione from the Mensa nearest the patient's home.
    pub(super) fn care_for_bedridden(&mut self) {
        let below = self.params.bedridden_below;
        let patients: Vec<usize> = (0..self.npcs.len())
            .filter(|&i| {
                let n = &self.npcs[i];
                n.health < below
                    && n.needs.hunger < CARE_HUNGER
                    && !matches!(n.action, Action::Eat(_))
            })
            .collect();
        for i in patients {
            let Some(carer) = self.carer_of(i) else {
                continue;
            };
            let food = self.npcs[carer]
                .inventory
                .items
                .items()
                .into_iter()
                .map(|(item, _)| item)
                .find(|&item| item.usage() == ItemUse::Food);
            let brought = match food {
                Some(item) => {
                    self.npcs[carer].inventory.items.remove(item, 1);
                    Some(item)
                }
                None => self.razione_for(self.npcs[i].home),
            };
            if let Some(item) = brought {
                self.gift_effect(i, item);
                self.combat.meals_brought += 1;
            }
        }
    }

    /// Who brings food to NPC `i`: the closest of partner, family and
    /// friends who is awake, not travelling, 14+ and not bedridden itself.
    fn carer_of(&self, i: usize) -> Option<usize> {
        let npc = &self.npcs[i];
        npc.relations
            .iter()
            .filter(|r| r.kind.is_family() || r.affinity >= CARE_AFFINITY)
            .filter_map(|r| {
                let k = self.npc_index(r.other)?;
                let c = &self.npcs[k];
                let able = c.is_awake()
                    && c.age >= LifeStage::GIOVANE_FROM
                    && c.health >= self.params.bedridden_below
                    && !matches!(c.action, Action::Travel { .. } | Action::Attack(_));
                let closeness = r.affinity
                    + match r.kind {
                        RelationKind::Partner => 1.0,
                        RelationKind::Friend => 0.0,
                        _ => 0.5,
                    };
                able.then_some((closeness, k))
            })
            .max_by(|a, b| a.0.total_cmp(&b.0).then(b.1.cmp(&a.1)))
            .map(|(_, k)| k)
    }

    /// Takes a meal's Razioni from the Mensa nearest `home` that has them.
    fn razione_for(&mut self, home: CarriageId) -> Option<ItemKind> {
        let per_meal = self.params.razioni_per_meal;
        let mensa = self
            .carriages
            .iter()
            .filter(|c| c.kind == CarriageKind::Mensa && c.stock.has(ItemKind::Razione, per_meal))
            .min_by_key(|c| (c.id.distance(home), c.id))
            .map(|c| c.id)?;
        self.carriages[mensa.index()]
            .stock
            .take(ItemKind::Razione, per_meal);
        Some(ItemKind::Razione)
    }

    /// End of a tick: the player's health comes back (faster asleep).
    pub(super) fn player_recover(&mut self) {
        let p = &self.params;
        let me = &self.player;
        if me.health >= MAX_HEALTH || me.dead.is_some() {
            return;
        }
        let hunger = me.needs.map_or(1.0, |n| n.hunger);
        let rate = heal_rate(p, me.is_asleep(), hunger);
        let me = &mut self.player;
        me.health = (me.health + rate).min(MAX_HEALTH);
        me.injury = (me.injury - rate).clamp(0.0, MAX_HEALTH - me.health);
    }
}
