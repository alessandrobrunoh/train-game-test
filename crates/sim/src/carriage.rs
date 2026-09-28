//! Carrozze e postazioni.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::{CarriageId, StationId};
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
    pub const ALL: [CarriageKind; 5] = [
        CarriageKind::Dormitorio,
        CarriageKind::Mensa,
        CarriageKind::Serra,
        CarriageKind::Officina,
        CarriageKind::Mercato,
    ];

    pub fn name(self) -> &'static str {
        match self {
            CarriageKind::Dormitorio => "Dormitorio",
            CarriageKind::Mensa => "Mensa",
            CarriageKind::Serra => "Serra",
            CarriageKind::Officina => "Officina",
            CarriageKind::Mercato => "Mercato",
        }
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
    pub fn name(self) -> &'static str {
        match self {
            StationKind::Bed => "cuccetta",
            StationKind::Table => "tavolo",
            StationKind::Stove => "cucina",
            StationKind::GrowBed => "aiuola",
            StationKind::Workbench => "banco da lavoro",
            StationKind::Counter => "bancone",
        }
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
    /// Carriage storage. Each kind only stores some items, bounded by
    /// [`crate::SimParams::storage_cap`]: Serra verdura; Mensa razioni;
    /// Officina rottame, attrezzi, vestiti; Mercato attrezzi, vestiti.
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

    pub fn has_free(&self, kind: StationKind) -> bool {
        self.free_station(kind).is_some()
    }

    pub(crate) fn push_stations(&mut self, kind: StationKind, count: usize, capacity: u16) {
        for _ in 0..count {
            let id = StationId(self.stations.len() as u16);
            self.stations.push(Station {
                id,
                kind,
                capacity,
                occupancy: 0,
            });
        }
    }
}

struct CarriageLabel<'a>(&'a Carriage);

impl fmt::Display for CarriageLabel<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let c = self.0;
        write!(f, "{} «{}» (carrozza {})", c.kind, c.name, c.id)
    }
}
