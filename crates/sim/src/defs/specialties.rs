//! Chi produce cosa: le ricette di ogni tipo di carrozza, tra cui si
//! scelgono le specialità (vedi `World::specialties`) e da cui dipende la
//! distanza dal produttore nei prezzi dei Mercati.

use super::jobs::JOBS;
use super::recipes::RecipeDef;
use crate::carriage::CarriageKind;
use crate::item::ItemKind;

impl CarriageKind {
    /// Recipes worked in carriages of this kind (by the jobs whose workplace
    /// it is), in table order, each output once.
    pub fn recipes(self) -> Vec<&'static RecipeDef> {
        let mut out: Vec<&'static RecipeDef> = Vec::new();
        for job in JOBS.iter().filter(|j| j.workplace == self) {
            for recipe in job.work.recipes() {
                if !out.iter().any(|r| r.output == recipe.output) {
                    out.push(recipe);
                }
            }
        }
        out
    }

    /// Whether carriages of this kind make `item`.
    pub fn makes(self, item: ItemKind) -> bool {
        JOBS.iter()
            .filter(|j| j.workplace == self)
            .any(|j| j.work.recipes().iter().any(|r| r.output == item))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn producing_kinds_and_their_recipes() {
        let outputs = |kind: CarriageKind| -> Vec<ItemKind> {
            kind.recipes().iter().map(|r| r.output).collect()
        };
        // The staple (food) comes first for Serre and Mense.
        assert_eq!(
            outputs(CarriageKind::Serra),
            [ItemKind::Verdura, ItemKind::Cotone, ItemKind::Erbe]
        );
        assert_eq!(
            outputs(CarriageKind::Mensa),
            [ItemKind::Razione, ItemKind::Te]
        );
        assert!(outputs(CarriageKind::Officina).len() >= 2);
        assert!(outputs(CarriageKind::Dormitorio).is_empty());
        // The Mercati trade, they make nothing.
        assert!(outputs(CarriageKind::Mercato).is_empty());
        for kind in CarriageKind::ALL {
            for item in ItemKind::ALL {
                assert_eq!(kind.makes(item), outputs(kind).contains(&item));
            }
        }
        // Everything sold at the Mercati is made somewhere.
        for item in ItemKind::SOLD {
            assert!(CarriageKind::ALL.iter().any(|k| k.makes(item)), "{item:?}");
        }
    }
}
