//! Oggetti e magazzini.
//!
//! Items are stored in bulk as fractional amounts per [`ItemKind`] in a
//! [`Stock`] (carriage storage); only whole units can be eaten or sold, so a
//! renderer should show [`Stock::count`]. Durable items (Attrezzi, Vestiti)
//! owned by an NPC live in [`crate::Inventory`] with a durability instead.
//! Per-kind properties are in the catalog [`crate::defs::ITEMS`], and how
//! each item is made in [`crate::defs::RECIPES`].

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::carriage::CarriageKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ItemKind {
    /// Raw crop grown by Contadini in the Serre.
    Verdura,
    /// Cooked food portion made by Cuochi in the Mense; one per meal, free.
    Razione,
    /// Scrap: the train sheds it steadily into the Officine.
    Rottame,
    /// Tool made by Operai from Rottame: boosts Contadini and Operai, wears with work.
    Attrezzo,
    /// Clothes sewn by Operai from Tessuto: slow down tiredness, wear daily.
    Vestito,
    /// Raw fibre grown by Contadini in the Serre: woven into Tessuto.
    Cotone,
    /// Herbs grown by Contadini in the Serre: brewed into Tè.
    Erbe,
    /// Worked metal, cast by Operai from Rottame: for Attrezzi, Lampade, Giocattoli.
    Metallo,
    /// Cloth woven by Operai from Cotone: for Vestiti, Coperte, Giocattoli.
    Tessuto,
    /// A pot of tea brewed by Cuochi from Erbe, served with the meals.
    Te,
    /// Blanket: kept in the Dormitori, residents sleep better.
    Coperta,
    /// Lamp: kept in the Dormitori, residents at home feel less lonely.
    Lampada,
    /// Toy: kept in the Dormitori, children at home feel less lonely.
    Giocattolo,
}

impl ItemKind {
    pub const COUNT: usize = 13;
    pub const ALL: [ItemKind; Self::COUNT] = [
        ItemKind::Verdura,
        ItemKind::Razione,
        ItemKind::Rottame,
        ItemKind::Attrezzo,
        ItemKind::Vestito,
        ItemKind::Cotone,
        ItemKind::Erbe,
        ItemKind::Metallo,
        ItemKind::Tessuto,
        ItemKind::Te,
        ItemKind::Coperta,
        ItemKind::Lampada,
        ItemKind::Giocattolo,
    ];

    /// Position in [`ItemKind::ALL`] (and in per-item arrays).
    pub fn index(self) -> usize {
        self as usize
    }

    /// Singular, lowercase: "attrezzo".
    pub fn name(self) -> &'static str {
        self.def().name
    }

    /// Plural, lowercase: "attrezzi".
    pub fn plural(self) -> &'static str {
        self.def().plural
    }

    /// With the indefinite article: "un attrezzo", "una razione".
    pub fn with_article(self) -> &'static str {
        self.def().with_article
    }

    /// Reference price in tokens (Mercato prices add a scarcity markup).
    pub fn base_value(self) -> u32 {
        self.def().base_value
    }

    /// Whether an owned unit wears out (tracked per NPC in [`crate::Inventory`]).
    pub fn has_durability(self) -> bool {
        self.def().usage.is_owned()
    }

    /// Whether NPCs can buy it at a Mercato.
    pub fn is_sold(self) -> bool {
        self.def().sold
    }

    /// Carriages where the item is made available to people: a shortage means
    /// none of them has a whole unit left.
    pub fn outlet(self) -> CarriageKind {
        self.def().outlet
    }
}

impl fmt::Display for ItemKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Bulk amounts per item kind (a carriage's storage, or a sum of them).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Stock([f32; ItemKind::COUNT]);

impl Stock {
    /// Exact (possibly fractional) amount.
    pub fn get(&self, item: ItemKind) -> f32 {
        self.0[item.index()]
    }

    /// Whole units available (what can be eaten, sold, drawn).
    pub fn count(&self, item: ItemKind) -> u32 {
        self.get(item).max(0.0).floor() as u32
    }

    pub fn has(&self, item: ItemKind, amount: f32) -> bool {
        self.get(item) >= amount
    }

    pub fn set(&mut self, item: ItemKind, amount: f32) {
        self.0[item.index()] = amount.max(0.0);
    }

    /// Adds up to `amount` without exceeding `cap`; returns what was added.
    pub fn add(&mut self, item: ItemKind, amount: f32, cap: f32) -> f32 {
        let slot = &mut self.0[item.index()];
        let added = amount.min(cap - *slot).max(0.0);
        *slot += added;
        added
    }

    /// Removes up to `amount`; returns what was taken.
    pub fn take(&mut self, item: ItemKind, amount: f32) -> f32 {
        let slot = &mut self.0[item.index()];
        let taken = amount.min(*slot).max(0.0);
        *slot -= taken;
        taken
    }

    /// `(item, amount)` for every kind, in [`ItemKind::ALL`] order.
    pub fn iter(&self) -> impl Iterator<Item = (ItemKind, f32)> + '_ {
        ItemKind::ALL.into_iter().map(|k| (k, self.get(k)))
    }

    /// No whole unit of anything.
    pub fn is_empty(&self) -> bool {
        ItemKind::ALL.iter().all(|&k| self.count(k) == 0)
    }

    /// Adds `other` to `self` (no cap).
    pub fn merge(&mut self, other: &Stock) {
        for (a, b) in self.0.iter_mut().zip(other.0) {
            *a += b;
        }
    }
}

impl fmt::Display for Stock {
    /// Whole units, e.g. "120 verdure, 1 attrezzo" (or "vuoto").
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for item in ItemKind::ALL {
            let n = self.count(item);
            if n == 0 {
                continue;
            }
            if !first {
                f.write_str(", ")?;
            }
            first = false;
            let name = if n == 1 { item.name() } else { item.plural() };
            write!(f, "{n} {name}")?;
        }
        if first {
            f.write_str("vuoto")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_add_take_respect_bounds() {
        let mut s = Stock::default();
        assert_eq!(s.add(ItemKind::Verdura, 5.5, 4.0), 4.0);
        assert_eq!(s.add(ItemKind::Verdura, 1.0, 4.0), 0.0);
        assert_eq!(s.take(ItemKind::Verdura, 10.0), 4.0);
        assert_eq!(s.get(ItemKind::Verdura), 0.0);
        s.add(ItemKind::Attrezzo, 2.7, 10.0);
        s.add(ItemKind::Razione, 1.0, 10.0);
        assert_eq!(s.count(ItemKind::Attrezzo), 2);
        assert_eq!(s.to_string(), "1 razione, 2 attrezzi");
        assert_eq!(Stock::default().to_string(), "vuoto");
    }
}
