//! Catalogo degli oggetti.

use crate::carriage::CarriageKind;
use crate::item::ItemKind;
use crate::params::SimParams;

/// What an item is for: who wants it and where an owned unit is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemUse {
    /// Eaten or cooked; accepted as a gift by who is hungry.
    Food,
    /// Raw material for crafters; nobody else wants it.
    Material,
    /// Owned in [`crate::Inventory::tool`]: boosts the jobs that use tools.
    Tool,
    /// Owned in [`crate::Inventory::clothes`]: slows tiredness.
    Clothes,
}

impl ItemUse {
    /// Whether an owned unit wears out (and is tracked per NPC).
    pub fn is_owned(self) -> bool {
        matches!(self, ItemUse::Tool | ItemUse::Clothes)
    }
}

#[derive(Debug)]
pub struct ItemDef {
    pub kind: ItemKind,
    /// Singular, lowercase: "attrezzo".
    pub name: &'static str,
    /// Plural, lowercase: "attrezzi".
    pub plural: &'static str,
    /// With the indefinite article: "un attrezzo".
    pub with_article: &'static str,
    /// Reference price in tokens (Mercato prices scale it, see `price_at`).
    pub base_value: u32,
    pub usage: ItemUse,
    /// Whether NPCs buy it at the Mercati.
    pub sold: bool,
    /// Carriages where people get it: a shortage means none of them has a
    /// whole unit left.
    pub outlet: CarriageKind,
    /// Whether a shortage raises an event ([`crate::EventKind::Shortage`]).
    pub shortage_reported: bool,
    /// Fraction of the stored amount lost every midnight.
    pub spoilage: Option<fn(&SimParams) -> f32>,
}

pub static ITEMS: [ItemDef; ItemKind::COUNT] = ROWS;

// A `const` copy, readable at compile time (see `count`/`select`).
const ROWS: [ItemDef; ItemKind::COUNT] = [
    ItemDef {
        kind: ItemKind::Verdura,
        name: "verdura",
        plural: "verdure",
        with_article: "una cassetta di verdura",
        base_value: 1,
        usage: ItemUse::Food,
        sold: false,
        outlet: CarriageKind::Serra,
        shortage_reported: false,
        spoilage: Some(|p| p.verdura_spoilage_per_day),
    },
    ItemDef {
        kind: ItemKind::Razione,
        name: "razione",
        plural: "razioni",
        with_article: "una razione",
        base_value: 2,
        usage: ItemUse::Food,
        sold: false,
        outlet: CarriageKind::Mensa,
        shortage_reported: true,
        spoilage: Some(|p| p.razioni_spoilage_per_day),
    },
    ItemDef {
        kind: ItemKind::Rottame,
        name: "rottame",
        plural: "rottami",
        with_article: "un pezzo di rottame",
        base_value: 1,
        usage: ItemUse::Material,
        sold: false,
        outlet: CarriageKind::Officina,
        shortage_reported: false,
        spoilage: None,
    },
    ItemDef {
        kind: ItemKind::Attrezzo,
        name: "attrezzo",
        plural: "attrezzi",
        with_article: "un attrezzo",
        base_value: 40,
        usage: ItemUse::Tool,
        sold: true,
        outlet: CarriageKind::Mercato,
        shortage_reported: true,
        spoilage: None,
    },
    ItemDef {
        kind: ItemKind::Vestito,
        name: "vestito",
        plural: "vestiti",
        with_article: "un vestito",
        base_value: 12,
        usage: ItemUse::Clothes,
        sold: true,
        outlet: CarriageKind::Mercato,
        shortage_reported: true,
        spoilage: None,
    },
];

impl ItemKind {
    pub fn def(self) -> &'static ItemDef {
        &ITEMS[self.index()]
    }

    /// Items sold at the Mercati, in [`ItemKind::ALL`] order.
    pub const SOLD: [ItemKind; count(Flag::Sold)] = select(Flag::Sold);

    /// Items whose shortage is reported, in [`ItemKind::ALL`] order.
    pub const SHORTAGE_REPORTED: [ItemKind; count(Flag::ShortageReported)] =
        select(Flag::ShortageReported);
}

/// Boolean columns usable at compile time.
#[derive(Clone, Copy)]
enum Flag {
    Sold,
    ShortageReported,
}

const fn has(row: &ItemDef, flag: Flag) -> bool {
    match flag {
        Flag::Sold => row.sold,
        Flag::ShortageReported => row.shortage_reported,
    }
}

/// How many rows have `flag`.
const fn count(flag: Flag) -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < ROWS.len() {
        if has(&ROWS[i], flag) {
            n += 1;
        }
        i += 1;
    }
    n
}

/// The kinds whose rows have `flag`, in table order.
const fn select<const N: usize>(flag: Flag) -> [ItemKind; N] {
    let mut out = [ItemKind::Verdura; N];
    let (mut i, mut n) = (0, 0);
    while i < ROWS.len() {
        if has(&ROWS[i], flag) {
            out[n] = ROWS[i].kind;
            n += 1;
        }
        i += 1;
    }
    out
}
