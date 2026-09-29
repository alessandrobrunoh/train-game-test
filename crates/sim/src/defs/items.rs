//! Catalogo degli oggetti.

use crate::carriage::CarriageKind;
use crate::item::ItemKind;
use crate::params::SimParams;

/// What an item is for: who wants it and where an owned unit is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemUse {
    /// Eaten, drunk or cooked; accepted as a gift by who is hungry.
    Food,
    /// Raw material for crafters, or a shared good kept in a carriage
    /// ([`ItemDef::amenity`]): nobody owns it.
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

/// Broad kind of item, for grouping in lists and generic icons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemCategory {
    /// Grown, or shed by the train.
    Raw,
    /// Made from raw materials, used to make other things.
    Intermediate,
    /// Used up: eaten, drunk.
    Consumable,
    /// Lasts, wearing out with use.
    Durable,
}

impl ItemCategory {
    pub const ALL: [ItemCategory; 4] = [
        ItemCategory::Raw,
        ItemCategory::Intermediate,
        ItemCategory::Consumable,
        ItemCategory::Durable,
    ];

    /// Capitalized plural: "Materie prime".
    pub fn name(self) -> &'static str {
        match self {
            ItemCategory::Raw => "Materie prime",
            ItemCategory::Intermediate => "Semilavorati",
            ItemCategory::Consumable => "Consumabili",
            ItemCategory::Durable => "Oggetti durevoli",
        }
    }
}

/// A shared good kept where it is used, benefiting whoever is there (see
/// the `World` comfort rules and the `SimParams` comfort parameters).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Amenity {
    /// Served with the meals at the Mense: a little energy and company.
    MealDrink,
    /// Dormitorio: residents sleeping there regain energy faster.
    Bedding,
    /// Dormitorio: residents awake at home lose sociality more slowly.
    Light,
    /// Dormitorio: children awake at home lose sociality more slowly.
    Toys,
}

impl Amenity {
    /// Kind of carriage where the amenity is used.
    pub fn place(self) -> CarriageKind {
        match self {
            Amenity::MealDrink => CarriageKind::Mensa,
            Amenity::Bedding | Amenity::Light | Amenity::Toys => CarriageKind::Dormitorio,
        }
    }
}

/// Storage of an item in a kind of carriage, declared with the item. The
/// carriage table ([`crate::defs::CARRIAGES`]) lists the older ones; the two
/// together give [`SimParams::storage_cap`].
#[derive(Debug)]
pub struct Store {
    pub carriage: CarriageKind,
    pub cap: fn(&SimParams) -> f32,
    /// Stock of each such carriage at generation.
    pub start: f32,
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
    /// What it is for, one short Italian sentence (tooltips, crafting window).
    pub description: &'static str,
    /// Reference price in tokens (Mercato prices scale it, see `price_at`).
    pub base_value: u32,
    pub usage: ItemUse,
    pub category: ItemCategory,
    /// Whether NPCs buy it at the Mercati.
    pub sold: bool,
    /// Carriages where people get it: a shortage means none of them has a
    /// whole unit left.
    pub outlet: CarriageKind,
    /// Whether a shortage raises an event ([`crate::EventKind::Shortage`]).
    pub shortage_reported: bool,
    /// Fraction of the stored amount lost every midnight.
    pub spoilage: Option<fn(&SimParams) -> f32>,
    /// Shared use where it is kept (Tè at the Mense, comfort goods in the
    /// Dormitori), if any.
    pub amenity: Option<Amenity>,
    /// Storage declared here rather than in the carriage table.
    pub stores: &'static [Store],
}

pub static ITEMS: [ItemDef; ItemKind::COUNT] = ROWS;

// A `const` copy, readable at compile time (see `count`/`select`).
const ROWS: [ItemDef; ItemKind::COUNT] = [
    ItemDef {
        kind: ItemKind::Verdura,
        name: "verdura",
        plural: "verdure",
        with_article: "una cassetta di verdura",
        description: "Cresce nelle Serre; in Mensa si cucina in razioni.",
        base_value: 1,
        usage: ItemUse::Food,
        category: ItemCategory::Raw,
        sold: false,
        outlet: CarriageKind::Serra,
        shortage_reported: false,
        spoilage: Some(|p| p.verdura_spoilage_per_day),
        amenity: None,
        stores: &[],
    },
    ItemDef {
        kind: ItemKind::Razione,
        name: "razione",
        plural: "razioni",
        with_article: "una razione",
        description: "Un pasto: in Mensa se ne mangia una a testa.",
        base_value: 2,
        usage: ItemUse::Food,
        category: ItemCategory::Consumable,
        sold: false,
        outlet: CarriageKind::Mensa,
        shortage_reported: true,
        spoilage: Some(|p| p.razioni_spoilage_per_day),
        amenity: None,
        stores: &[],
    },
    ItemDef {
        kind: ItemKind::Rottame,
        name: "rottame",
        plural: "rottami",
        with_article: "un pezzo di rottame",
        description: "Il treno lo perde di continuo; in Officina si fonde in metallo.",
        base_value: 1,
        usage: ItemUse::Material,
        category: ItemCategory::Raw,
        sold: false,
        outlet: CarriageKind::Officina,
        shortage_reported: false,
        spoilage: None,
        amenity: None,
        stores: &[],
    },
    ItemDef {
        kind: ItemKind::Attrezzo,
        name: "attrezzo",
        plural: "attrezzi",
        with_article: "un attrezzo",
        description: "Contadini e operai lavorano più in fretta; si consuma lavorando.",
        base_value: 40,
        usage: ItemUse::Tool,
        category: ItemCategory::Durable,
        sold: true,
        outlet: CarriageKind::Mercato,
        shortage_reported: true,
        spoilage: None,
        amenity: None,
        stores: &[],
    },
    ItemDef {
        kind: ItemKind::Vestito,
        name: "vestito",
        plural: "vestiti",
        with_article: "un vestito",
        description: "Tiene caldo: ci si stanca meno. Si consuma ogni giorno.",
        base_value: 12,
        usage: ItemUse::Clothes,
        category: ItemCategory::Durable,
        sold: true,
        outlet: CarriageKind::Mercato,
        shortage_reported: true,
        spoilage: None,
        amenity: None,
        stores: &[],
    },
    ItemDef {
        kind: ItemKind::Cotone,
        name: "cotone",
        plural: "cotone",
        with_article: "una balla di cotone",
        description: "Cresce nelle Serre; in Officina si tesse in tessuto.",
        base_value: 1,
        usage: ItemUse::Material,
        category: ItemCategory::Raw,
        sold: false,
        outlet: CarriageKind::Serra,
        shortage_reported: false,
        spoilage: None,
        amenity: None,
        stores: &[Store {
            carriage: CarriageKind::Serra,
            cap: |p| p.crops_storage_cap,
            start: 20.0,
        }],
    },
    ItemDef {
        kind: ItemKind::Erbe,
        name: "erbe",
        plural: "erbe",
        with_article: "un mazzo di erbe",
        description: "Crescono nelle Serre; in Mensa ci si fa il tè. Appassiscono.",
        base_value: 1,
        usage: ItemUse::Material,
        category: ItemCategory::Raw,
        sold: false,
        outlet: CarriageKind::Serra,
        shortage_reported: false,
        spoilage: Some(|p| p.erbe_spoilage_per_day),
        amenity: None,
        stores: &[Store {
            carriage: CarriageKind::Serra,
            cap: |p| p.crops_storage_cap,
            start: 15.0,
        }],
    },
    ItemDef {
        kind: ItemKind::Metallo,
        name: "metallo",
        plural: "metallo",
        with_article: "una barra di metallo",
        description: "Rottame fuso: serve per attrezzi, lampade e giocattoli.",
        base_value: 3,
        usage: ItemUse::Material,
        category: ItemCategory::Intermediate,
        sold: false,
        outlet: CarriageKind::Officina,
        shortage_reported: false,
        spoilage: None,
        amenity: None,
        stores: &[Store {
            carriage: CarriageKind::Officina,
            cap: |p| p.workshop_parts_cap,
            start: 10.0,
        }],
    },
    ItemDef {
        kind: ItemKind::Tessuto,
        name: "tessuto",
        plural: "tessuto",
        with_article: "una pezza di tessuto",
        description: "Cotone tessuto: serve per vestiti, coperte e giocattoli.",
        base_value: 3,
        usage: ItemUse::Material,
        category: ItemCategory::Intermediate,
        sold: false,
        outlet: CarriageKind::Officina,
        shortage_reported: false,
        spoilage: None,
        amenity: None,
        stores: &[Store {
            carriage: CarriageKind::Officina,
            cap: |p| p.workshop_parts_cap,
            start: 10.0,
        }],
    },
    ItemDef {
        kind: ItemKind::Te,
        name: "tè",
        plural: "tè",
        with_article: "una teiera di tè",
        description: "Servito ai pasti in Mensa: un po' di energia e di compagnia.",
        base_value: 1,
        usage: ItemUse::Food,
        category: ItemCategory::Consumable,
        sold: false,
        outlet: CarriageKind::Mensa,
        shortage_reported: false,
        spoilage: None,
        amenity: Some(Amenity::MealDrink),
        stores: &[Store {
            carriage: CarriageKind::Mensa,
            cap: |p| p.te_storage_cap,
            start: 20.0,
        }],
    },
    ItemDef {
        kind: ItemKind::Coperta,
        name: "coperta",
        plural: "coperte",
        with_article: "una coperta",
        description: "Nei Dormitori: chi ci dorme recupera le forze più in fretta.",
        base_value: 10,
        usage: ItemUse::Material,
        category: ItemCategory::Durable,
        sold: false,
        outlet: CarriageKind::Dormitorio,
        shortage_reported: false,
        spoilage: None,
        amenity: Some(Amenity::Bedding),
        stores: &[
            Store {
                carriage: CarriageKind::Officina,
                cap: |p| p.workshop_goods_cap,
                start: 5.0,
            },
            Store {
                carriage: CarriageKind::Dormitorio,
                cap: |p| p.dorm_coperte_cap,
                start: 40.0,
            },
        ],
    },
    ItemDef {
        kind: ItemKind::Lampada,
        name: "lampada",
        plural: "lampade",
        with_article: "una lampada",
        description: "Nei Dormitori: le serate a casa sono meno solitarie.",
        base_value: 15,
        usage: ItemUse::Material,
        category: ItemCategory::Durable,
        sold: false,
        outlet: CarriageKind::Dormitorio,
        shortage_reported: false,
        spoilage: None,
        amenity: Some(Amenity::Light),
        stores: &[
            Store {
                carriage: CarriageKind::Officina,
                cap: |p| p.workshop_goods_cap,
                start: 2.0,
            },
            Store {
                carriage: CarriageKind::Dormitorio,
                cap: |p| p.dorm_lampade_cap,
                start: 8.0,
            },
        ],
    },
    ItemDef {
        kind: ItemKind::Giocattolo,
        name: "giocattolo",
        plural: "giocattoli",
        with_article: "un giocattolo",
        description: "Nei Dormitori: i bambini a casa si sentono meno soli.",
        base_value: 10,
        usage: ItemUse::Material,
        category: ItemCategory::Durable,
        sold: false,
        outlet: CarriageKind::Dormitorio,
        shortage_reported: false,
        spoilage: None,
        amenity: Some(Amenity::Toys),
        stores: &[
            Store {
                carriage: CarriageKind::Officina,
                cap: |p| p.workshop_goods_cap,
                start: 2.0,
            },
            Store {
                carriage: CarriageKind::Dormitorio,
                cap: |p| p.dorm_giocattoli_cap,
                start: 6.0,
            },
        ],
    },
];

impl ItemKind {
    pub fn def(self) -> &'static ItemDef {
        &ITEMS[self.index()]
    }

    /// What the item is for (one short Italian sentence).
    pub fn description(self) -> &'static str {
        self.def().description
    }

    pub fn category(self) -> ItemCategory {
        self.def().category
    }

    /// Shared use where it is kept, if any.
    pub fn amenity(self) -> Option<Amenity> {
        self.def().amenity
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
