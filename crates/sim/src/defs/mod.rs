//! Definizioni come dati: oggetti, ricette, carrozze e lavori.
//!
//! Every per-kind property lives in a table row instead of `match` arms
//! scattered through the sim. Carriage and station kinds are fixed enums
//! with static rows ([`CARRIAGES`], [`STATIONS`], `kind.def()`). Items,
//! recipes and jobs are **data that grows during a game**: the static rows
//! here ([`ITEMS`], [`RECIPES`], [`JOBS`]) are only the seed of every world's
//! [`crate::Catalog`], which the Custode extends ([`crate::custode`]); the
//! sim reads them through `world.catalog()`. Their ids ([`crate::ItemKind`],
//! [`RecipeId`], [`crate::Job`]) are positions in the catalog, so row `i` of
//! each table is the builtin kind with id `i` (the tests below check it).
//!
//! Tunable numbers stay in [`crate::SimParams`] (saved with the world) and
//! the builtin rows point at them ([`Num::Param`]); what the Custode adds
//! has fixed numbers ([`Num::Fixed`]). Recipes are shared by the NPC workers
//! and the player.

pub(crate) mod carriages;
pub(crate) mod items;
pub(crate) mod jobs;
pub(crate) mod recipes;
mod specialties;

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::params::SimParams;

pub use carriages::{
    CARRIAGES, CarriageDef, STATIONS, StationCount, StationDef, StationRule, Storage,
};
pub use items::{
    Amenity, Consume, DEFAULT_STACK_LIMIT, ITEMS, ItemCategory, ItemDef, ItemUse, Store,
};
pub use jobs::{JOBS, JobDef, Work};
pub use recipes::{Input, RECIPES, RecipeDef, RecipeId, Source};

/// Items that no recipe makes: the train sheds them (see `World::shed_rottame`).
pub const EXTERNAL_ITEMS: [crate::ItemKind; 1] = [crate::ItemKind::Rottame];

/// A number of a definition: a tunable of [`SimParams`] (the builtin rows,
/// so that tuning the params tunes them) or a fixed value (what the Custode
/// adds). Serializes as its value.
#[derive(Clone, Copy)]
pub enum Num {
    Param(fn(&SimParams) -> f32),
    Fixed(f32),
}

impl Num {
    pub fn get(&self, p: &SimParams) -> f32 {
        match self {
            Num::Param(f) => f(p),
            Num::Fixed(v) => *v,
        }
    }
}

impl fmt::Debug for Num {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Num::Param(_) => f.write_str("Param"),
            Num::Fixed(v) => write!(f, "{v}"),
        }
    }
}

impl Serialize for Num {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.get(&SimParams::default()).serialize(s)
    }
}

impl<'de> Deserialize<'de> for Num {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Num::Fixed(f32::deserialize(d)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::carriage::{CarriageKind, StationKind};
    use crate::catalog::Catalog;
    use crate::item::ItemKind;
    use crate::npc::Job;
    use crate::params::SimParams;

    #[test]
    fn rows_follow_id_order() {
        let cat = Catalog::builtin();
        for (i, item) in ItemKind::BUILTIN.into_iter().enumerate() {
            assert_eq!(ITEMS[i].kind, item);
            assert_eq!(item.index(), i);
            assert_eq!(cat.kind_at(i), Some(item));
            assert_eq!(ItemKind::from_code(item.code()), Some(item));
        }
        for (i, kind) in CarriageKind::ALL.into_iter().enumerate() {
            assert_eq!(CARRIAGES[i].kind, kind);
            assert_eq!(kind.index(), i);
        }
        for (i, kind) in StationKind::ALL.into_iter().enumerate() {
            assert_eq!(STATIONS[i].kind, kind);
            assert_eq!(kind.index(), i);
        }
        for (i, job) in Job::BUILTIN.into_iter().enumerate() {
            assert_eq!(JOBS[i].job, job);
            assert_eq!(job.index(), i);
            assert_eq!(Job::from_code(job.code()), Some(job));
        }
        for (i, recipe) in RECIPES.iter().enumerate() {
            assert_eq!(cat.recipe_by_key(&recipe.key), Some(RecipeId(i as u16)));
        }
        let keys = [
            (RecipeId::VERDURA, "verdura"),
            (RecipeId::COTONE, "cotone"),
            (RecipeId::ERBE, "erbe"),
            (RecipeId::RAZIONE, "razione"),
            (RecipeId::TE, "te"),
            (RecipeId::METALLO, "metallo"),
            (RecipeId::TESSUTO, "tessuto"),
            (RecipeId::ATTREZZO, "attrezzo"),
            (RecipeId::VESTITO, "vestito"),
            (RecipeId::COPERTA, "coperta"),
            (RecipeId::LAMPADA, "lampada"),
            (RecipeId::GIOCATTOLO, "giocattolo"),
        ];
        for (id, key) in keys {
            assert_eq!(cat.recipe(id).key, key);
        }
    }

    #[test]
    fn every_job_has_a_place_to_work() {
        let cat = Catalog::builtin();
        for job in Job::BUILTIN {
            let def = cat.job(job);
            let rules = job.workplace_kind().def().stations;
            assert!(
                rules.iter().any(|r| r.kind == job.station_kind()
                    && matches!(r.count, StationCount::Workers { job: j, .. } if j == job)),
                "{job:?}: no {:?} for its workers in a {:?}",
                job.station_kind(),
                job.workplace_kind()
            );
            for &recipe in def.work.recipes() {
                let recipe = cat.recipe(recipe);
                assert_eq!(
                    recipe.station,
                    job.station_kind(),
                    "{job:?} makes {:?}",
                    recipe.output
                );
            }
        }
    }

    #[test]
    fn builtin_catalog_is_consistent() {
        let problems = Catalog::builtin().problems(&SimParams::default());
        assert!(problems.is_empty(), "{problems:#?}");
    }

    #[test]
    fn traded_and_reported_items() {
        let cat = Catalog::builtin();
        assert_eq!(cat.sold_items(), [ItemKind::Attrezzo, ItemKind::Vestito]);
        assert_eq!(
            cat.shortage_reported(),
            [ItemKind::Razione, ItemKind::Attrezzo, ItemKind::Vestito]
        );
        let p = SimParams::default();
        for item in cat.sold_items() {
            assert!(item.has_durability(), "{item:?} is sold but can't be owned");
            assert!(cat.storage_cap(&p, CarriageKind::Mercato, item) > 0.0);
        }
    }
}
