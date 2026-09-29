//! Chi produce cosa: vedi [`crate::Catalog::recipes_of_kind`] e
//! [`crate::Catalog::makes`], tra cui si scelgono le specialità (vedi
//! `World::specialties`) e da cui dipende la distanza dal produttore nei
//! prezzi dei Mercati.

#[cfg(test)]
mod tests {
    use crate::carriage::CarriageKind;
    use crate::catalog::Catalog;
    use crate::item::ItemKind;

    #[test]
    fn producing_kinds_and_their_recipes() {
        let cat = Catalog::builtin();
        let outputs = |kind: CarriageKind| -> Vec<ItemKind> {
            cat.recipes_of_kind(kind)
                .iter()
                .map(|&r| cat.recipe(r).output)
                .collect()
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
            for item in cat.kinds() {
                assert_eq!(cat.makes(kind, item), outputs(kind).contains(&item));
            }
        }
        // Everything sold at the Mercati is made somewhere.
        for item in cat.sold_items() {
            assert!(
                CarriageKind::ALL.iter().any(|&k| cat.makes(k, item)),
                "{item:?}"
            );
        }
    }
}
