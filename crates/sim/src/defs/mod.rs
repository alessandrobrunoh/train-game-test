//! Definizioni come dati: oggetti, ricette, carrozze e lavori.
//!
//! Every per-kind property lives in one static table here instead of `match`
//! arms scattered through the sim: adding a kind means adding a row. The enums
//! ([`ItemKind`], [`CarriageKind`], [`StationKind`], [`Job`]) stay the
//! identifiers (cheap, serialized); `kind.def()` gives the row. Tunable
//! numbers stay in [`crate::SimParams`] (saved with the world) and the rows
//! point at them through `fn(&SimParams) -> f32` accessors.
//!
//! Row `i` of each table describes the kind with index `i`; the tests below
//! check it. Recipes ([`RECIPES`]) are shared by the NPC workers and the
//! player.
//!
//! [`ItemKind`]: crate::ItemKind
//! [`CarriageKind`]: crate::CarriageKind
//! [`StationKind`]: crate::StationKind
//! [`Job`]: crate::Job

mod carriages;
mod items;
mod jobs;
mod recipes;
mod specialties;

pub use carriages::{
    CARRIAGES, CarriageDef, STATIONS, StationCount, StationDef, StationRule, Storage,
};
pub use items::{Amenity, ITEMS, ItemCategory, ItemDef, ItemUse, Store};
pub use jobs::{JOBS, JobDef, Work};
pub use recipes::{Input, RECIPES, RecipeDef, Source};

/// Items that no recipe makes: the train sheds them (see `World::shed_rottame`).
pub const EXTERNAL_ITEMS: [crate::ItemKind; 1] = [crate::ItemKind::Rottame];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::carriage::{CarriageKind, StationKind};
    use crate::item::ItemKind;
    use crate::npc::Job;
    use crate::params::SimParams;

    #[test]
    fn rows_follow_enum_order() {
        for (i, item) in ItemKind::ALL.into_iter().enumerate() {
            assert_eq!(ITEMS[i].kind, item);
            assert_eq!(item.index(), i);
        }
        for (i, kind) in CarriageKind::ALL.into_iter().enumerate() {
            assert_eq!(CARRIAGES[i].kind, kind);
            assert_eq!(kind.index(), i);
        }
        for (i, kind) in StationKind::ALL.into_iter().enumerate() {
            assert_eq!(STATIONS[i].kind, kind);
            assert_eq!(kind.index(), i);
        }
        for (i, job) in Job::ALL.into_iter().enumerate() {
            assert_eq!(JOBS[i].job, job);
            assert_eq!(job.index(), i);
        }
        for (i, recipe) in RECIPES.iter().enumerate() {
            assert_eq!(recipe.index(), i);
            assert_eq!(RecipeDef::by_key(recipe.key).map(|r| r.index()), Some(i));
        }
    }

    #[test]
    fn every_job_has_a_place_to_work() {
        for job in Job::ALL {
            let def = job.def();
            let rules = def.workplace.def().stations;
            assert!(
                rules.iter().any(|r| r.kind == def.station
                    && matches!(r.count, StationCount::Workers { job: j, .. } if j == job)),
                "{job:?}: no {:?} for its workers in a {:?}",
                def.station,
                def.workplace
            );
            for recipe in def.work.recipes() {
                assert_eq!(
                    recipe.station, def.station,
                    "{job:?} makes {:?}",
                    recipe.output
                );
                assert!(
                    RecipeDef::by_key(recipe.key).is_some(),
                    "{job:?} makes {:?} with a recipe missing from RECIPES",
                    recipe.output
                );
            }
        }
    }

    #[test]
    fn recipes_are_consistent() {
        let p = SimParams::default();
        for (i, recipe) in RECIPES.iter().enumerate() {
            let what = recipe.key;
            assert!(
                RECIPES[..i].iter().all(|r| r.key != recipe.key),
                "{what}: duplicate key"
            );
            assert!(
                recipe.batch > 0 && recipe.rate(&p) > 0.0,
                "{what} is never made"
            );
            assert!(!recipe.name.is_empty());
            assert!(
                recipe.places().next().is_some(),
                "{what}: no carriage has a {:?}",
                recipe.station
            );
            let maker = recipe
                .maker()
                .unwrap_or_else(|| panic!("nobody makes {what}"));
            let workplace = maker.def().workplace;
            assert!(recipe.can_be_made_in(workplace));
            assert!(
                p.storage_cap(workplace, recipe.output) > 0.0,
                "a {workplace:?} can't store the {:?} made there",
                recipe.output
            );
            for (k, input) in recipe.inputs.iter().enumerate() {
                assert_ne!(input.item, recipe.output, "{what} is made from itself");
                assert!(
                    recipe.inputs[..k].iter().all(|o| o.item != input.item),
                    "{what}: {:?} listed twice",
                    input.item
                );
                let amount = (input.amount)(&p);
                assert!(amount > 0.0 && amount.is_finite(), "{what}: bad amount");
                assert!(recipe.per_output(input, &p) > 0.0);
                let from = match input.from {
                    Source::Here => workplace,
                    Source::Nearest(kind) => kind,
                };
                assert!(
                    p.storage_cap(from, input.item) > 0.0,
                    "{:?} for {what} is fetched from a {from:?}, which never stores it",
                    input.item
                );
            }
            let player: Vec<ItemKind> = recipe.player_inputs(&p).iter().map(|i| i.0).collect();
            assert_eq!(player.len(), recipe.inputs.len());
            assert!(recipe.player_inputs(&p).iter().all(|&(_, n)| n >= 1));
            assert!(recipe.player_minutes(&p) >= 1);
        }
    }

    #[test]
    fn every_item_has_a_source_and_a_use() {
        for item in ItemKind::ALL {
            let def = item.def();
            let made = RECIPES.iter().any(|r| r.output == item);
            assert!(
                made || EXTERNAL_ITEMS.contains(&item),
                "nothing makes {item:?}"
            );
            let ingredient = RECIPES
                .iter()
                .any(|r| r.inputs.iter().any(|i| i.item == item));
            // Razioni are eaten at the Mense; Tè and comfort goods are
            // amenities; Attrezzi and Vestiti are owned.
            let used = ingredient
                || def.amenity.is_some()
                || def.usage.is_owned()
                || item == ItemKind::Razione;
            assert!(used, "{item:?} has no use");
            assert!(def.base_value > 0, "{item:?} is worth nothing");
            assert!(!def.name.is_empty() && !def.plural.is_empty());
            assert!(!def.with_article.is_empty() && !def.description.is_empty());
        }
    }

    #[test]
    fn storage_is_declared_once() {
        let p = SimParams::default();
        for item in ItemKind::ALL {
            assert!(
                CarriageKind::ALL
                    .iter()
                    .any(|&k| p.storage_cap(k, item) > 0.0),
                "{item:?} can't be stored anywhere"
            );
            for store in item.def().stores {
                assert!((store.cap)(&p) >= store.start && store.start >= 0.0);
                assert!(
                    store.carriage.def().storage.iter().all(|s| s.item != item),
                    "{item:?} in a {:?} is declared twice",
                    store.carriage
                );
            }
            if let Some(amenity) = item.amenity() {
                assert!(p.storage_cap(amenity.place(), item) > 0.0);
            }
        }
    }

    #[test]
    fn no_circular_chains_without_an_outside_source() {
        // Every recipe output can be reached from the external items.
        let mut reachable: Vec<ItemKind> = EXTERNAL_ITEMS.to_vec();
        loop {
            let before = reachable.len();
            for r in &RECIPES {
                if !reachable.contains(&r.output)
                    && r.inputs.iter().all(|i| reachable.contains(&i.item))
                {
                    reachable.push(r.output);
                }
            }
            if reachable.len() == before {
                break;
            }
        }
        for r in &RECIPES {
            assert!(reachable.contains(&r.output), "{} is unreachable", r.key);
        }
    }

    #[test]
    fn traded_and_reported_items() {
        assert_eq!(ItemKind::SOLD, [ItemKind::Attrezzo, ItemKind::Vestito]);
        assert_eq!(
            ItemKind::SHORTAGE_REPORTED,
            [ItemKind::Razione, ItemKind::Attrezzo, ItemKind::Vestito]
        );
        let p = SimParams::default();
        for item in ItemKind::SOLD {
            assert!(item.has_durability(), "{item:?} is sold but can't be owned");
            assert!(p.storage_cap(CarriageKind::Mercato, item) > 0.0);
        }
        for item in ItemKind::ALL {
            assert!(
                p.storage_cap(item.outlet(), item) > 0.0,
                "{item:?} is handed out in a {:?}, which never stores it",
                item.outlet()
            );
        }
    }
}
