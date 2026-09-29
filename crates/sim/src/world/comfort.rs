//! Comodità: il tè ai pasti, coperte, lampade e giocattoli nei Dormitori.
//!
//! Shared goods benefit whoever is where they are kept ([`Amenity`]):
//! - Tè: each meal at a Mensa that has it uses
//!   [`SimParams::te_per_meal`] of a pot and gives a little energy and
//!   sociality ([`World::serve_tea`]).
//! - Coperte, Lampade, Giocattoli: every midnight a share of those in the
//!   Dormitori wears out, then the administration hands out new ones from
//!   the Officine (nearest first) up to a target per resident (per child
//!   for the Giocattoli) ([`World::furnish_dorms`]). Their coverage (stock
//!   over target, at most 1) speeds up sleep and slows the loss of
//!   sociality of who is at home ([`need_factors`]).
//!
//! No randomness: amounts are deterministic.

use super::World;
use crate::carriage::CarriageKind;
use crate::defs::Amenity;
use crate::ids::CarriageId;
use crate::item::ItemKind;
use crate::npc::{LifeStage, Npc};
use crate::params::SimParams;

/// Comfort goods kept in the Dormitori, in delivery order.
const DORM_GOODS: [ItemKind; 3] = [ItemKind::Coperta, ItemKind::Lampada, ItemKind::Giocattolo];

/// How well a carriage is provided with comfort goods: coverage in `0..=1`
/// of its target for each (0 outside the Dormitori).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Comfort {
    /// Coperte per resident, over [`SimParams::coperte_per_resident`].
    pub bedding: f32,
    /// Lampade per resident, over [`SimParams::lampade_per_resident`].
    pub light: f32,
    /// Giocattoli per child, over [`SimParams::giocattoli_per_child`].
    pub toys: f32,
}

/// Multipliers of an NPC's sleep energy gain and sociality loss this minute,
/// given the comfort of each carriage (`levels`, indexed like the carriages).
pub(super) fn need_factors(p: &SimParams, levels: &[Comfort], npc: &Npc) -> (f32, f32) {
    let sleep = match levels.get(npc.carriage.index()) {
        Some(c) if c.bedding > 0.0 => 1.0 + p.coperta_sleep_bonus * c.bedding,
        _ => 1.0,
    };
    let social = match levels.get(npc.home.index()) {
        Some(c) if npc.carriage == npc.home => {
            let mut relief = p.lampada_social_relief * c.light;
            if npc.stage() == LifeStage::Bambino {
                relief += p.giocattolo_social_relief * c.toys;
            }
            (1.0 - relief).clamp(0.0, 1.0)
        }
        _ => 1.0,
    };
    (sleep, social)
}

/// Units of a Dormitorio good wanted for `residents` people of whom
/// `children` are children.
fn target(p: &SimParams, amenity: Option<Amenity>, residents: usize, children: usize) -> f32 {
    match amenity {
        Some(Amenity::Bedding) => residents as f32 * p.coperte_per_resident,
        Some(Amenity::Light) => residents as f32 * p.lampade_per_resident,
        Some(Amenity::Toys) => children as f32 * p.giocattoli_per_child,
        Some(Amenity::MealDrink) | None => 0.0,
    }
}

/// Share of a Dormitorio good worn out every midnight.
fn wear(p: &SimParams, amenity: Option<Amenity>) -> f32 {
    match amenity {
        Some(Amenity::Bedding) => p.coperta_wear_per_day,
        Some(Amenity::Light) => p.lampada_wear_per_day,
        Some(Amenity::Toys) => p.giocattolo_wear_per_day,
        Some(Amenity::MealDrink) | None => 0.0,
    }
    .clamp(0.0, 1.0)
}

fn coverage(stock: f32, target: f32) -> f32 {
    if target > 0.0 {
        (stock / target).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

impl World {
    /// Residents and children per carriage (by home), indexed like `carriages`.
    pub(super) fn households(&self) -> (Vec<usize>, Vec<usize>) {
        let mut residents = vec![0; self.carriages.len()];
        let mut children = vec![0; self.carriages.len()];
        for npc in &self.npcs {
            let h = npc.home.index();
            residents[h] += 1;
            if npc.stage() == LifeStage::Bambino {
                children[h] += 1;
            }
        }
        (residents, children)
    }

    /// Comfort of every carriage, indexed like `carriages`.
    pub(super) fn comfort_levels(&self) -> Vec<Comfort> {
        let (residents, children) = self.households();
        let p = &self.params;
        self.carriages
            .iter()
            .map(|c| {
                if c.kind != CarriageKind::Dormitorio {
                    return Comfort::default();
                }
                let (r, k) = (residents[c.id.index()], children[c.id.index()]);
                let level = |item: ItemKind| {
                    let amenity = self.catalog.item(item).amenity;
                    coverage(c.stock.get(item), target(p, amenity, r, k))
                };
                Comfort {
                    bedding: level(ItemKind::Coperta),
                    light: level(ItemKind::Lampada),
                    toys: level(ItemKind::Giocattolo),
                }
            })
            .collect()
    }

    /// Comfort of one carriage (all 0 outside the Dormitori).
    pub fn comfort(&self, carriage: CarriageId) -> Comfort {
        self.comfort_levels()
            .get(carriage.index())
            .copied()
            .unwrap_or_default()
    }

    /// Units of `item` the administration wants in `carriage` (a
    /// Dormitorio, for its comfort goods; 0 otherwise), capped by its storage.
    pub fn furnishing_target(&self, carriage: CarriageId, item: ItemKind) -> f32 {
        let (residents, children) = self.households();
        self.furnishing_target_of(carriage, item, &residents, &children)
    }

    /// [`World::furnishing_target`] given [`World::households`].
    pub(super) fn furnishing_target_of(
        &self,
        carriage: CarriageId,
        item: ItemKind,
        residents: &[usize],
        children: &[usize],
    ) -> f32 {
        let Some(c) = self.carriage(carriage) else {
            return 0.0;
        };
        let amenity = self.catalog.get_item(item).and_then(|d| d.amenity);
        if amenity.is_none_or(|a| a.place() != c.kind) || c.kind != CarriageKind::Dormitorio {
            return 0.0;
        }
        let (r, k) = (residents[carriage.index()], children[carriage.index()]);
        let cap = self.catalog.storage_cap(&self.params, c.kind, item);
        target(&self.params, amenity, r, k).min(cap)
    }

    /// NPC `i` just sat down to eat: a cup of Tè if its Mensa has some.
    pub(super) fn serve_tea(&mut self, i: usize) {
        let p = &self.params;
        let per_meal = p.te_per_meal;
        let (energy, social) = (p.te_energy_boost, p.te_social_boost);
        if per_meal <= 0.0 {
            return;
        }
        let here = self.npcs[i].carriage.index();
        let stock = &mut self.carriages[here].stock;
        if !stock.has(ItemKind::Te, per_meal) {
            return;
        }
        stock.take(ItemKind::Te, per_meal);
        let heal = self.params.te_heal;
        let npc = &mut self.npcs[i];
        let needs = &mut npc.needs;
        needs.energy = (needs.energy + energy).min(1.0);
        needs.social = (needs.social + social).min(1.0);
        // Tè helps who is hurt get better.
        if npc.is_hurt() {
            npc.health = (npc.health + heal).min(crate::combat::MAX_HEALTH);
            npc.injury = npc.injury.min(crate::combat::MAX_HEALTH - npc.health);
        }
    }

    /// Midnight: comfort goods in the Dormitori wear out, then the Officine
    /// (nearest first) hand out whole new ones up to each Dormitorio's target.
    pub(super) fn furnish_dorms(&mut self) {
        let (residents, children) = self.households();
        for item in DORM_GOODS {
            let amenity = self.catalog.item(item).amenity;
            let keep = 1.0 - wear(&self.params, amenity);
            let dorms: Vec<usize> = self
                .carriages
                .iter()
                .filter(|c| c.kind == CarriageKind::Dormitorio)
                .map(|c| c.id.index())
                .collect();
            for d in dorms {
                let p = &self.params;
                let c = &mut self.carriages[d];
                let worn = c.stock.get(item) * keep;
                c.stock.set(item, worn);
                let cap = self.catalog.storage_cap(p, c.kind, item);
                let wanted = target(p, amenity, residents[d], children[d]).min(cap);
                let mut missing = (wanted - worn).floor();
                if missing < 1.0 {
                    continue;
                }
                for o in self.nearest_of_kind(CarriageKind::Officina, CarriageId(d as u16)) {
                    if missing < 1.0 {
                        break;
                    }
                    let whole = self.carriages[o].stock.count(item) as f32;
                    let taken = self.carriages[o].stock.take(item, missing.min(whole));
                    self.carriages[d].stock.add(item, taken, cap);
                    missing -= taken;
                }
            }
        }
    }
}
