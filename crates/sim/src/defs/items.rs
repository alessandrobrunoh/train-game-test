//! Catalogo degli oggetti: le righe dei 13 oggetti di partenza.
//!
//! A world's catalog ([`crate::Catalog`]) starts from these rows and grows
//! with the items the Custode adds. Names, usage and stack limit are the
//! item's [`crate::ItemInfo`] (carried by every [`ItemKind`]); the rest is
//! the [`ItemDef`].

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::Num;
use crate::carriage::CarriageKind;
use crate::custode::Appearance;
use crate::item::{ItemInfo, ItemKind};
use crate::time::GameTime;

/// What an item is for: who wants it and where an owned unit is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
/// together give [`crate::Catalog::storage_cap`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Store {
    pub carriage: CarriageKind,
    pub cap: Num,
    /// Stock of each such carriage at generation.
    pub start: f32,
}

/// What eating or drinking one unit gives (a gift, or an NPC's own food).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Consume {
    /// Satiety (it also ends starvation).
    pub hunger: Num,
    pub energy: Num,
    pub social: Num,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ItemDef {
    /// Id, names, usage and stack limit.
    pub kind: ItemKind,
    /// What it is for, one short Italian sentence (tooltips, crafting window).
    pub description: Cow<'static, str>,
    /// Reference price in tokens (Mercato prices scale it, see `price_at`).
    pub base_value: u32,
    pub category: ItemCategory,
    /// Whether NPCs buy it at the Mercati.
    pub sold: bool,
    /// Whether a shortage raises an event ([`crate::EventKind::Shortage`]).
    pub shortage_reported: bool,
    /// Fraction of the stored amount lost every midnight.
    pub spoilage: Option<Num>,
    /// Shared use where it is kept (Tè at the Mense, comfort goods in the
    /// Dormitori), if any.
    pub amenity: Option<Amenity>,
    /// Storage declared here rather than in the carriage table.
    pub stores: Cow<'static, [Store]>,
    /// What eating or drinking one gives (food only).
    pub consume: Option<Consume>,
    /// Whether an adult who can well afford one wants to own one (a new
    /// durable item: it is bought at the stalls and kept).
    pub desired: bool,
    /// How its icon looks (items the Custode added; the builtin ones have
    /// hand-drawn icons).
    pub appearance: Option<Appearance>,
    /// When the Custode added it (None: a builtin item).
    pub added: Option<GameTime>,
}

impl ItemDef {
    /// Singular, lowercase: "attrezzo".
    pub fn name(&self) -> &'static str {
        self.kind.name()
    }

    pub fn plural(&self) -> &'static str {
        self.kind.plural()
    }

    pub fn with_article(&self) -> &'static str {
        self.kind.with_article()
    }

    pub fn usage(&self) -> ItemUse {
        self.kind.usage()
    }

    pub fn stack_size(&self) -> u32 {
        self.kind.stack_size()
    }

    /// Carriages where people get it (see [`ItemKind::outlet`]).
    pub fn outlet(&self) -> CarriageKind {
        self.kind.outlet()
    }
}

/// Units per inventory slot of an item without [`ItemDef::stack_limit`]
/// (The Escapists style: materials pile up to 10; the catalog sets 5 for
/// food and drinks and 1 for durable goods).
pub const DEFAULT_STACK_LIMIT: u32 = 10;

/// Code of each builtin item in the saves and in `Debug`, by id.
pub(crate) const BUILTIN_CODES: [&str; ItemKind::BUILTIN_COUNT] = [
    "Verdura",
    "Razione",
    "Rottame",
    "Attrezzo",
    "Vestito",
    "Cotone",
    "Erbe",
    "Metallo",
    "Tessuto",
    "Te",
    "Coperta",
    "Lampada",
    "Giocattolo",
];

/// Names and nature of each builtin item, by id.
pub(crate) static BUILTIN_INFOS: [ItemInfo; ItemKind::BUILTIN_COUNT] = [
    ItemInfo {
        key: "verdura",
        name: "verdura",
        plural: "verdure",
        with_article: "una cassetta di verdura",
        usage: ItemUse::Food,
        outlet: CarriageKind::Serra,
        stack_limit: None,
    },
    ItemInfo {
        key: "razione",
        name: "razione",
        plural: "razioni",
        with_article: "una razione",
        usage: ItemUse::Food,
        outlet: CarriageKind::Mensa,
        stack_limit: Some(5),
    },
    ItemInfo {
        key: "rottame",
        name: "rottame",
        plural: "rottami",
        with_article: "un pezzo di rottame",
        usage: ItemUse::Material,
        outlet: CarriageKind::Officina,
        stack_limit: None,
    },
    ItemInfo {
        key: "attrezzo",
        name: "attrezzo",
        plural: "attrezzi",
        with_article: "un attrezzo",
        usage: ItemUse::Tool,
        outlet: CarriageKind::Mercato,
        stack_limit: Some(1),
    },
    ItemInfo {
        key: "vestito",
        name: "vestito",
        plural: "vestiti",
        with_article: "un vestito",
        usage: ItemUse::Clothes,
        outlet: CarriageKind::Mercato,
        stack_limit: Some(1),
    },
    ItemInfo {
        key: "cotone",
        name: "cotone",
        plural: "cotone",
        with_article: "una balla di cotone",
        usage: ItemUse::Material,
        outlet: CarriageKind::Serra,
        stack_limit: None,
    },
    ItemInfo {
        key: "erbe",
        name: "erbe",
        plural: "erbe",
        with_article: "un mazzo di erbe",
        usage: ItemUse::Material,
        outlet: CarriageKind::Serra,
        stack_limit: None,
    },
    ItemInfo {
        key: "metallo",
        name: "metallo",
        plural: "metallo",
        with_article: "una barra di metallo",
        usage: ItemUse::Material,
        outlet: CarriageKind::Officina,
        stack_limit: None,
    },
    ItemInfo {
        key: "tessuto",
        name: "tessuto",
        plural: "tessuto",
        with_article: "una pezza di tessuto",
        usage: ItemUse::Material,
        outlet: CarriageKind::Officina,
        stack_limit: None,
    },
    ItemInfo {
        key: "te",
        name: "tè",
        plural: "tè",
        with_article: "una teiera di tè",
        usage: ItemUse::Food,
        outlet: CarriageKind::Mensa,
        stack_limit: Some(5),
    },
    ItemInfo {
        key: "coperta",
        name: "coperta",
        plural: "coperte",
        with_article: "una coperta",
        usage: ItemUse::Material,
        outlet: CarriageKind::Dormitorio,
        stack_limit: Some(1),
    },
    ItemInfo {
        key: "lampada",
        name: "lampada",
        plural: "lampade",
        with_article: "una lampada",
        usage: ItemUse::Material,
        outlet: CarriageKind::Dormitorio,
        stack_limit: Some(1),
    },
    ItemInfo {
        key: "giocattolo",
        name: "giocattolo",
        plural: "giocattoli",
        with_article: "un giocattolo",
        usage: ItemUse::Material,
        outlet: CarriageKind::Dormitorio,
        stack_limit: Some(1),
    },
];

pub static ITEMS: [ItemDef; ItemKind::BUILTIN_COUNT] = [
    ItemDef {
        kind: ItemKind::Verdura,
        description: Cow::Borrowed("Cresce nelle Serre; in Mensa si cucina in razioni."),
        base_value: 1,
        category: ItemCategory::Raw,
        sold: false,
        shortage_reported: false,
        spoilage: Some(Num::Param(|p| p.verdura_spoilage_per_day)),
        amenity: None,
        stores: Cow::Borrowed(&[]),
        consume: Some(Consume {
            hunger: Num::Param(|p| p.meal_restore / 2.0),
            energy: Num::Fixed(0.0),
            social: Num::Fixed(0.0),
        }),
        desired: false,
        appearance: None,
        added: None,
    },
    ItemDef {
        kind: ItemKind::Razione,
        description: Cow::Borrowed("Un pasto: in Mensa se ne mangia una a testa."),
        base_value: 2,
        category: ItemCategory::Consumable,
        sold: false,
        shortage_reported: true,
        spoilage: Some(Num::Param(|p| p.razioni_spoilage_per_day)),
        amenity: None,
        stores: Cow::Borrowed(&[]),
        consume: Some(Consume {
            hunger: Num::Param(|p| p.meal_restore),
            energy: Num::Fixed(0.0),
            social: Num::Fixed(0.0),
        }),
        desired: false,
        appearance: None,
        added: None,
    },
    ItemDef {
        kind: ItemKind::Rottame,
        description: Cow::Borrowed(
            "Il treno lo perde di continuo; in Officina si fonde in metallo.",
        ),
        base_value: 1,
        category: ItemCategory::Raw,
        sold: false,
        shortage_reported: false,
        spoilage: None,
        amenity: None,
        stores: Cow::Borrowed(&[]),
        consume: None,
        desired: false,
        appearance: None,
        added: None,
    },
    ItemDef {
        kind: ItemKind::Attrezzo,
        description: Cow::Borrowed(
            "Contadini e operai lavorano più in fretta; si consuma lavorando.",
        ),
        base_value: 40,
        category: ItemCategory::Durable,
        sold: true,
        shortage_reported: true,
        spoilage: None,
        amenity: None,
        stores: Cow::Borrowed(&[]),
        consume: None,
        desired: false,
        appearance: None,
        added: None,
    },
    ItemDef {
        kind: ItemKind::Vestito,
        description: Cow::Borrowed("Tiene caldo: ci si stanca meno. Si consuma ogni giorno."),
        base_value: 12,
        category: ItemCategory::Durable,
        sold: true,
        shortage_reported: true,
        spoilage: None,
        amenity: None,
        stores: Cow::Borrowed(&[]),
        consume: None,
        desired: false,
        appearance: None,
        added: None,
    },
    ItemDef {
        kind: ItemKind::Cotone,
        description: Cow::Borrowed("Cresce nelle Serre; in Officina si tesse in tessuto."),
        base_value: 1,
        category: ItemCategory::Raw,
        sold: false,
        shortage_reported: false,
        spoilage: None,
        amenity: None,
        stores: Cow::Borrowed(&[Store {
            carriage: CarriageKind::Serra,
            cap: Num::Param(|p| p.crops_storage_cap),
            start: 20.0,
        }]),
        consume: None,
        desired: false,
        appearance: None,
        added: None,
    },
    ItemDef {
        kind: ItemKind::Erbe,
        description: Cow::Borrowed("Crescono nelle Serre; in Mensa ci si fa il tè. Appassiscono."),
        base_value: 1,
        category: ItemCategory::Raw,
        sold: false,
        shortage_reported: false,
        spoilage: Some(Num::Param(|p| p.erbe_spoilage_per_day)),
        amenity: None,
        stores: Cow::Borrowed(&[Store {
            carriage: CarriageKind::Serra,
            cap: Num::Param(|p| p.crops_storage_cap),
            start: 15.0,
        }]),
        consume: None,
        desired: false,
        appearance: None,
        added: None,
    },
    ItemDef {
        kind: ItemKind::Metallo,
        description: Cow::Borrowed("Rottame fuso: serve per attrezzi, lampade e giocattoli."),
        base_value: 3,
        category: ItemCategory::Intermediate,
        sold: false,
        shortage_reported: false,
        spoilage: None,
        amenity: None,
        stores: Cow::Borrowed(&[Store {
            carriage: CarriageKind::Officina,
            cap: Num::Param(|p| p.workshop_parts_cap),
            start: 10.0,
        }]),
        consume: None,
        desired: false,
        appearance: None,
        added: None,
    },
    ItemDef {
        kind: ItemKind::Tessuto,
        description: Cow::Borrowed("Cotone tessuto: serve per vestiti, coperte e giocattoli."),
        base_value: 3,
        category: ItemCategory::Intermediate,
        sold: false,
        shortage_reported: false,
        spoilage: None,
        amenity: None,
        stores: Cow::Borrowed(&[Store {
            carriage: CarriageKind::Officina,
            cap: Num::Param(|p| p.workshop_parts_cap),
            start: 10.0,
        }]),
        consume: None,
        desired: false,
        appearance: None,
        added: None,
    },
    ItemDef {
        kind: ItemKind::Te,
        description: Cow::Borrowed("Servito ai pasti in Mensa: un po' di energia e di compagnia."),
        base_value: 1,
        category: ItemCategory::Consumable,
        sold: false,
        shortage_reported: false,
        spoilage: None,
        amenity: Some(Amenity::MealDrink),
        stores: Cow::Borrowed(&[Store {
            carriage: CarriageKind::Mensa,
            cap: Num::Param(|p| p.te_storage_cap),
            start: 20.0,
        }]),
        consume: Some(Consume {
            hunger: Num::Fixed(0.0),
            energy: Num::Param(|p| 2.0 * p.te_energy_boost),
            social: Num::Param(|p| 2.0 * p.te_social_boost),
        }),
        desired: false,
        appearance: None,
        added: None,
    },
    ItemDef {
        kind: ItemKind::Coperta,
        description: Cow::Borrowed("Nei Dormitori: chi ci dorme recupera le forze più in fretta."),
        base_value: 10,
        category: ItemCategory::Durable,
        sold: false,
        shortage_reported: false,
        spoilage: None,
        amenity: Some(Amenity::Bedding),
        stores: Cow::Borrowed(&[
            Store {
                carriage: CarriageKind::Officina,
                cap: Num::Param(|p| p.workshop_goods_cap),
                start: 5.0,
            },
            Store {
                carriage: CarriageKind::Dormitorio,
                cap: Num::Param(|p| p.dorm_coperte_cap),
                start: 40.0,
            },
        ]),
        consume: None,
        desired: false,
        appearance: None,
        added: None,
    },
    ItemDef {
        kind: ItemKind::Lampada,
        description: Cow::Borrowed("Nei Dormitori: le serate a casa sono meno solitarie."),
        base_value: 15,
        category: ItemCategory::Durable,
        sold: false,
        shortage_reported: false,
        spoilage: None,
        amenity: Some(Amenity::Light),
        stores: Cow::Borrowed(&[
            Store {
                carriage: CarriageKind::Officina,
                cap: Num::Param(|p| p.workshop_goods_cap),
                start: 2.0,
            },
            Store {
                carriage: CarriageKind::Dormitorio,
                cap: Num::Param(|p| p.dorm_lampade_cap),
                start: 8.0,
            },
        ]),
        consume: None,
        desired: false,
        appearance: None,
        added: None,
    },
    ItemDef {
        kind: ItemKind::Giocattolo,
        description: Cow::Borrowed("Nei Dormitori: i bambini a casa si sentono meno soli."),
        base_value: 10,
        category: ItemCategory::Durable,
        sold: false,
        shortage_reported: false,
        spoilage: None,
        amenity: Some(Amenity::Toys),
        stores: Cow::Borrowed(&[
            Store {
                carriage: CarriageKind::Officina,
                cap: Num::Param(|p| p.workshop_goods_cap),
                start: 2.0,
            },
            Store {
                carriage: CarriageKind::Dormitorio,
                cap: Num::Param(|p| p.dorm_giocattoli_cap),
                start: 6.0,
            },
        ]),
        consume: None,
        desired: false,
        appearance: None,
        added: None,
    },
];
