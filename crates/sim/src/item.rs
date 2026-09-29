//! Oggetti e magazzini.
//!
//! Items are stored in bulk as fractional amounts per [`ItemKind`] in a
//! [`Stock`] (carriage storage); only whole units can be eaten or sold, so a
//! renderer should show [`Stock::count`]. Durable items (Attrezzi, Vestiti)
//! owned by an NPC live in [`crate::Inventory`] with a durability instead.

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
    /// Clothes made by Operai from Rottame: slow down tiredness, wear daily.
    Vestito,
}

impl ItemKind {
    pub const COUNT: usize = 5;
    pub const ALL: [ItemKind; Self::COUNT] = [
        ItemKind::Verdura,
        ItemKind::Razione,
        ItemKind::Rottame,
        ItemKind::Attrezzo,
        ItemKind::Vestito,
    ];

    /// Position in [`ItemKind::ALL`] (and in per-item arrays).
    pub fn index(self) -> usize {
        self as usize
    }

    /// Singular, lowercase: "attrezzo".
    pub fn name(self) -> &'static str {
        match self {
            ItemKind::Verdura => "verdura",
            ItemKind::Razione => "razione",
            ItemKind::Rottame => "rottame",
            ItemKind::Attrezzo => "attrezzo",
            ItemKind::Vestito => "vestito",
        }
    }

    /// Plural, lowercase: "attrezzi".
    pub fn plural(self) -> &'static str {
        match self {
            ItemKind::Verdura => "verdure",
            ItemKind::Razione => "razioni",
            ItemKind::Rottame => "rottami",
            ItemKind::Attrezzo => "attrezzi",
            ItemKind::Vestito => "vestiti",
        }
    }

    /// With the indefinite article: "un attrezzo", "una razione".
    pub fn with_article(self) -> &'static str {
        match self {
            ItemKind::Verdura => "una cassetta di verdura",
            ItemKind::Razione => "una razione",
            ItemKind::Rottame => "un pezzo di rottame",
            ItemKind::Attrezzo => "un attrezzo",
            ItemKind::Vestito => "un vestito",
        }
    }

    /// Reference price in tokens (Mercato prices add a scarcity markup).
    pub fn base_value(self) -> u32 {
        match self {
            ItemKind::Verdura => 1,
            ItemKind::Razione => 2,
            ItemKind::Rottame => 1,
            ItemKind::Attrezzo => 40,
            ItemKind::Vestito => 12,
        }
    }

    /// Whether an owned unit wears out (tracked per NPC in [`crate::Inventory`]).
    pub fn has_durability(self) -> bool {
        matches!(self, ItemKind::Attrezzo | ItemKind::Vestito)
    }

    /// Whether NPCs can buy it at a Mercato.
    pub fn is_sold(self) -> bool {
        matches!(self, ItemKind::Attrezzo | ItemKind::Vestito)
    }

    /// Carriages where the item is made available to people: a shortage means
    /// none of them has a whole unit left.
    pub fn outlet(self) -> CarriageKind {
        match self {
            ItemKind::Verdura => CarriageKind::Serra,
            ItemKind::Razione => CarriageKind::Mensa,
            ItemKind::Rottame => CarriageKind::Officina,
            ItemKind::Attrezzo | ItemKind::Vestito => CarriageKind::Mercato,
        }
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
