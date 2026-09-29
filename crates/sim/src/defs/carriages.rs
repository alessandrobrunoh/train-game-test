//! Tipi di carrozza e di postazione.

use crate::carriage::{CarriageKind, StationKind};
use crate::item::ItemKind;
use crate::names;
use crate::npc::Job;
use crate::params::SimParams;

#[derive(Debug)]
pub struct CarriageDef {
    pub kind: CarriageKind,
    /// Display name of the kind: "Dormitorio".
    pub name: &'static str,
    /// Storeys: 1, or 2 with stairs (the gangways to the next carriages are
    /// on the ground floor).
    pub floors: u8,
    /// Proper names given to the carriages of this kind, in order; later
    /// ones are called "<kind> <n>".
    pub names: &'static [&'static str],
    /// Stations added at generation, in this order (it fixes the station ids).
    pub stations: &'static [StationRule],
    /// What the carriage stores, and how much of it at most. Items not listed
    /// are never stored there.
    pub storage: &'static [Storage],
    /// Stock at generation.
    pub start_stock: &'static [(ItemKind, f32)],
}

#[derive(Debug)]
pub struct Storage {
    pub item: ItemKind,
    pub cap: fn(&SimParams) -> f32,
}

/// A group of identical stations added to each carriage of a kind.
#[derive(Debug)]
pub struct StationRule {
    pub kind: StationKind,
    /// NPCs one station holds at once.
    pub capacity: u16,
    pub count: StationCount,
    /// Spread the stations evenly over the carriage's floors (ground floor
    /// first); otherwise they are all on the ground floor.
    pub spread: bool,
}

/// How many stations a [`StationRule`] adds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StationCount {
    /// Beds for the carriage's residents plus [`SimParams::spare_beds`] of
    /// them for newborns, plus one.
    Beds,
    /// Seats for [`SimParams::mensa_seats_per_person`] of the peak
    /// population, split among the carriages of this kind (at least one station).
    Seats,
    /// One station per `capacity` workers of `job` who work here, plus a
    /// spare. With `peak`, sized for a train at its birth limit (and never
    /// below who works here now).
    Workers { job: Job, peak: bool },
}

pub static CARRIAGES: [CarriageDef; CarriageKind::COUNT] = [
    CarriageDef {
        kind: CarriageKind::Dormitorio,
        name: "Dormitorio",
        floors: 2,
        names: names::DORM_NAMES,
        stations: &[StationRule {
            kind: StationKind::Bed,
            capacity: 1,
            count: StationCount::Beds,
            spread: true,
        }],
        storage: &[],
        start_stock: &[],
    },
    CarriageDef {
        kind: CarriageKind::Mensa,
        name: "Mensa",
        floors: 1,
        names: names::MENSA_NAMES,
        stations: &[
            StationRule {
                kind: StationKind::Table,
                capacity: 6,
                count: StationCount::Seats,
                spread: false,
            },
            StationRule {
                kind: StationKind::Stove,
                capacity: 1,
                count: StationCount::Workers {
                    job: Job::Cuoco,
                    peak: true,
                },
                spread: false,
            },
        ],
        storage: &[Storage {
            item: ItemKind::Razione,
            cap: |p| p.razioni_storage_cap,
        }],
        start_stock: &[(ItemKind::Razione, 100.0)],
    },
    CarriageDef {
        kind: CarriageKind::Serra,
        name: "Serra",
        floors: 1,
        names: names::SERRA_NAMES,
        stations: &[StationRule {
            kind: StationKind::GrowBed,
            capacity: 2,
            count: StationCount::Workers {
                job: Job::Contadino,
                peak: true,
            },
            spread: false,
        }],
        storage: &[Storage {
            item: ItemKind::Verdura,
            cap: |p| p.verdura_storage_cap,
        }],
        start_stock: &[(ItemKind::Verdura, 100.0)],
    },
    CarriageDef {
        kind: CarriageKind::Officina,
        name: "Officina",
        floors: 1,
        names: names::OFFICINA_NAMES,
        stations: &[StationRule {
            kind: StationKind::Workbench,
            capacity: 2,
            count: StationCount::Workers {
                job: Job::Operaio,
                peak: false,
            },
            spread: false,
        }],
        storage: &[
            Storage {
                item: ItemKind::Rottame,
                cap: |p| p.rottame_storage_cap,
            },
            Storage {
                item: ItemKind::Attrezzo,
                cap: |p| p.workshop_goods_cap,
            },
            Storage {
                item: ItemKind::Vestito,
                cap: |p| p.workshop_goods_cap,
            },
        ],
        start_stock: &[
            (ItemKind::Rottame, 20.0),
            (ItemKind::Attrezzo, 5.0),
            (ItemKind::Vestito, 5.0),
        ],
    },
    CarriageDef {
        kind: CarriageKind::Mercato,
        name: "Mercato",
        floors: 1,
        names: names::MERCATO_NAMES,
        stations: &[StationRule {
            kind: StationKind::Counter,
            capacity: 1,
            count: StationCount::Workers {
                job: Job::Mercante,
                peak: true,
            },
            spread: false,
        }],
        storage: &[
            Storage {
                item: ItemKind::Attrezzo,
                cap: |p| p.market_goods_cap,
            },
            Storage {
                item: ItemKind::Vestito,
                cap: |p| p.market_goods_cap,
            },
        ],
        start_stock: &[(ItemKind::Attrezzo, 10.0), (ItemKind::Vestito, 10.0)],
    },
];

impl CarriageKind {
    pub fn def(self) -> &'static CarriageDef {
        &CARRIAGES[self.index()]
    }
}

#[derive(Debug)]
pub struct StationDef {
    pub kind: StationKind,
    /// Lowercase: "banco da lavoro".
    pub name: &'static str,
}

pub static STATIONS: [StationDef; StationKind::COUNT] = [
    StationDef {
        kind: StationKind::Bed,
        name: "cuccetta",
    },
    StationDef {
        kind: StationKind::Table,
        name: "tavolo",
    },
    StationDef {
        kind: StationKind::Stove,
        name: "cucina",
    },
    StationDef {
        kind: StationKind::GrowBed,
        name: "aiuola",
    },
    StationDef {
        kind: StationKind::Workbench,
        name: "banco da lavoro",
    },
    StationDef {
        kind: StationKind::Counter,
        name: "bancone",
    },
];

impl StationKind {
    pub fn def(self) -> &'static StationDef {
        &STATIONS[self.index()]
    }
}
