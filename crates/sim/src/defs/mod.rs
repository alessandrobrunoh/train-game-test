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
//! check it.
//!
//! [`ItemKind`]: crate::ItemKind
//! [`CarriageKind`]: crate::CarriageKind
//! [`StationKind`]: crate::StationKind
//! [`Job`]: crate::Job

mod carriages;
mod items;
mod jobs;
mod recipes;

pub use carriages::{
    CARRIAGES, CarriageDef, STATIONS, StationCount, StationDef, StationRule, Storage,
};
pub use items::{ITEMS, ItemDef, ItemUse};
pub use jobs::{JOBS, JobDef, Work};
pub use recipes::{Input, RECIPES, RecipeDef, Source};

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
            }
        }
    }

    #[test]
    fn recipes_use_known_items_and_can_be_stored() {
        let p = SimParams::default();
        for recipe in &RECIPES {
            assert!((recipe.rate)(&p) > 0.0, "{:?} is never made", recipe.output);
            let maker = Job::ALL
                .into_iter()
                .find(|j| {
                    j.def()
                        .work
                        .recipes()
                        .iter()
                        .any(|r| r.output == recipe.output)
                })
                .unwrap_or_else(|| panic!("nobody makes {:?}", recipe.output));
            let workplace = maker.def().workplace;
            assert!(
                p.storage_cap(workplace, recipe.output) > 0.0,
                "a {workplace:?} can't store the {:?} made there",
                recipe.output
            );
            if let Some(input) = &recipe.input {
                assert_ne!(
                    input.item, recipe.output,
                    "{:?} is made from itself",
                    recipe.output
                );
                assert!((input.per_output)(&p) > 0.0);
                let from = match input.from {
                    Source::Here => workplace,
                    Source::Nearest(kind) => kind,
                };
                assert!(
                    p.storage_cap(from, input.item) > 0.0,
                    "{:?} for {:?} is fetched from a {from:?}, which never stores it",
                    input.item,
                    recipe.output
                );
            }
        }
    }

    #[test]
    fn every_input_has_a_source() {
        // No circular chains without an outside source: every input is either
        // made by a recipe or brought in by the train (Rottame).
        for recipe in &RECIPES {
            if let Some(input) = &recipe.input {
                let made = RECIPES.iter().any(|r| r.output == input.item);
                assert!(
                    made || input.item == ItemKind::Rottame,
                    "nothing makes {:?}",
                    input.item
                );
            }
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
