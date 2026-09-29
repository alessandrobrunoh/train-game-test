//! Carrozze e postazioni.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::{CarriageId, NpcId, StationId};
use crate::item::Stock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CarriageKind {
    Dormitorio,
    Mensa,
    Serra,
    Officina,
    /// Market: Mercanti bring Attrezzi and Vestiti from the Officine and
    /// people buy them with tokens.
    Mercato,
}

impl CarriageKind {
    pub const COUNT: usize = 5;
    pub const ALL: [CarriageKind; Self::COUNT] = [
        CarriageKind::Dormitorio,
        CarriageKind::Mensa,
        CarriageKind::Serra,
        CarriageKind::Officina,
        CarriageKind::Mercato,
    ];

    /// Position in [`CarriageKind::ALL`] (and in per-kind arrays).
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        self.def().name
    }
}

impl fmt::Display for CarriageKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// What a station is for. Determines which actions can use it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StationKind {
    /// Cuccetta (Dormitorio): Sleep.
    Bed,
    /// Tavolo (Mensa): Eat.
    Table,
    /// Cucina (Mensa): Cuoco works here.
    Stove,
    /// Aiuola (Serra): Contadino works here.
    GrowBed,
    /// Banco da lavoro (Officina): Operaio works here.
    Workbench,
    /// Bancone (Mercato): Mercante works here. Customers buy standing nearby.
    Counter,
}

impl StationKind {
    pub const COUNT: usize = 6;
    pub const ALL: [StationKind; Self::COUNT] = [
        StationKind::Bed,
        StationKind::Table,
        StationKind::Stove,
        StationKind::GrowBed,
        StationKind::Workbench,
        StationKind::Counter,
    ];

    /// Position in [`StationKind::ALL`].
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        self.def().name
    }
}

/// A spot inside a carriage where actions happen. The sim only tracks how
/// many NPCs use it; the renderer decides where it is drawn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Station {
    pub id: StationId,
    pub kind: StationKind,
    pub capacity: u16,
    pub occupancy: u16,
    /// Storey (0 = ground floor, where the gangways are).
    #[serde(default)]
    pub floor: u8,
}

impl Station {
    pub fn has_room(&self) -> bool {
        self.occupancy < self.capacity
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Carriage {
    pub id: CarriageId,
    pub name: String,
    pub kind: CarriageKind,
    pub stations: Vec<Station>,
    /// Carriage storage. Each kind only stores the items listed in its
    /// [`crate::defs::CarriageDef::storage`], bounded by
    /// [`crate::SimParams::storage_cap`].
    pub stock: Stock,
}

impl Carriage {
    /// Displays as e.g. `Mensa «Il Refettorio» (carrozza 2)`.
    pub fn label(&self) -> impl fmt::Display + '_ {
        CarriageLabel(self)
    }

    pub fn station(&self, id: StationId) -> Option<&Station> {
        self.stations.get(id.index())
    }

    /// First station of `kind` with free capacity.
    pub fn free_station(&self, kind: StationKind) -> Option<StationId> {
        self.stations
            .iter()
            .find(|s| s.kind == kind && s.has_room())
            .map(|s| s.id)
    }

    /// Station of `kind` with the most free places (the first one on ties):
    /// diners spread over the tables instead of crowding the first ones.
    pub fn roomiest_station(&self, kind: StationKind) -> Option<StationId> {
        self.stations
            .iter()
            .filter(|s| s.kind == kind && s.has_room())
            .max_by_key(|s| (s.capacity - s.occupancy, std::cmp::Reverse(s.id.0)))
            .map(|s| s.id)
    }

    /// Free station of `kind` for NPC `who`: everyone tends to use the same
    /// one (its own bed), or the next free one after it. Spreads people over
    /// all the stations (and floors) instead of filling the first ones.
    pub fn free_station_for(&self, kind: StationKind, who: NpcId) -> Option<StationId> {
        let of_kind = || self.stations.iter().filter(move |s| s.kind == kind);
        let n = of_kind().count();
        if n == 0 {
            return None;
        }
        of_kind()
            .cycle()
            .skip(who.0 as usize % n)
            .take(n)
            .find(|s| s.has_room())
            .map(|s| s.id)
    }

    pub fn has_free(&self, kind: StationKind) -> bool {
        self.free_station(kind).is_some()
    }

    pub(crate) fn push_stations(
        &mut self,
        kind: StationKind,
        count: usize,
        capacity: u16,
        floor: u8,
    ) {
        for _ in 0..count {
            let id = StationId(self.stations.len() as u16);
            self.stations.push(Station {
                id,
                kind,
                capacity,
                occupancy: 0,
                floor,
            });
        }
    }

    /// Storeys (1, or more with stairs; see [`crate::defs::CarriageDef::floors`]).
    pub fn floors(&self) -> u8 {
        self.kind.def().floors.max(1)
    }

    /// Storey of `station` (0 if unknown).
    pub fn floor_of(&self, station: StationId) -> u8 {
        self.station(station).map_or(0, |s| s.floor)
    }
}

struct CarriageLabel<'a>(&'a Carriage);

impl fmt::Display for CarriageLabel<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let c = self.0;
        write!(f, "{} «{}» (carrozza {})", c.kind, c.name, c.id)
    }
}
