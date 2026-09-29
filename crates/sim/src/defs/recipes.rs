//! Ricette: cosa si fabbrica, da cosa e dove.

use crate::carriage::{CarriageKind, StationKind};
use crate::item::ItemKind;
use crate::params::SimParams;

#[derive(Debug)]
pub struct RecipeDef {
    pub output: ItemKind,
    pub input: Option<Input>,
    /// Where the work is done.
    pub station: StationKind,
    /// Output per minute of work, before the tool bonus.
    pub rate: fn(&SimParams) -> f32,
}

#[derive(Debug)]
pub struct Input {
    pub item: ItemKind,
    /// Units used per unit of output.
    pub per_output: fn(&SimParams) -> f32,
    pub from: Source,
}

/// Where the worker takes an input from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The storage of the carriage where the work is done.
    Here,
    /// The carriages of this kind, nearest first.
    Nearest(CarriageKind),
}

pub static RECIPES: [RecipeDef; 4] = [VERDURA, RAZIONE, ATTREZZO, VESTITO];

/// Contadini grow it in the Serre.
pub(crate) const VERDURA: RecipeDef = RecipeDef {
    output: ItemKind::Verdura,
    input: None,
    station: StationKind::GrowBed,
    rate: |p| p.verdura_per_farm_minute,
};

/// Cuochi cook it in the Mense from Verdura of the Serre.
pub(crate) const RAZIONE: RecipeDef = RecipeDef {
    output: ItemKind::Razione,
    input: Some(Input {
        item: ItemKind::Verdura,
        per_output: |p| 1.0 / p.razioni_per_verdura,
        from: Source::Nearest(CarriageKind::Serra),
    }),
    station: StationKind::Stove,
    rate: |p| p.razioni_per_cook_minute,
};

/// Operai make it in the Officine from Rottame.
pub(crate) const ATTREZZO: RecipeDef = RecipeDef {
    output: ItemKind::Attrezzo,
    input: Some(Input {
        item: ItemKind::Rottame,
        per_output: |p| p.rottame_per_attrezzo,
        from: Source::Here,
    }),
    station: StationKind::Workbench,
    rate: |p| p.attrezzi_per_craft_minute,
};

/// Operai make it in the Officine from Rottame.
pub(crate) const VESTITO: RecipeDef = RecipeDef {
    output: ItemKind::Vestito,
    input: Some(Input {
        item: ItemKind::Rottame,
        per_output: |p| p.rottame_per_vestito,
        from: Source::Here,
    }),
    station: StationKind::Workbench,
    rate: |p| p.vestiti_per_craft_minute,
};
