//! Ricette: cosa si fabbrica, da cosa, dove e in quanto tempo.
//!
//! The same rows drive the NPC workers (`World::produce`: a minute of work
//! makes `batch / minutes` of the output, times the tool bonus, taking the
//! inputs from their [`Source`]) and the player (`World::player_craft`: a
//! whole batch from the player's own items, at a carriage with the station).
//! These are the builtin rows; a world's catalog ([`crate::Catalog`]) adds
//! the recipes the Custode accepts, with ids after them ([`RecipeId`]).

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::Num;
use crate::carriage::{CarriageKind, StationKind};
use crate::item::ItemKind;
use crate::params::SimParams;
use crate::time::GameTime;

/// A recipe's position in the world's catalog: the builtin ones first, in
/// [`RECIPES`] order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RecipeId(pub u16);

impl RecipeId {
    pub const VERDURA: RecipeId = RecipeId(0);
    pub const COTONE: RecipeId = RecipeId(1);
    pub const ERBE: RecipeId = RecipeId(2);
    pub const RAZIONE: RecipeId = RecipeId(3);
    pub const TE: RecipeId = RecipeId(4);
    pub const METALLO: RecipeId = RecipeId(5);
    pub const TESSUTO: RecipeId = RecipeId(6);
    pub const ATTREZZO: RecipeId = RecipeId(7);
    pub const VESTITO: RecipeId = RecipeId(8);
    pub const COPERTA: RecipeId = RecipeId(9);
    pub const LAMPADA: RecipeId = RecipeId(10);
    pub const GIOCATTOLO: RecipeId = RecipeId(11);

    pub fn index(self) -> usize {
        usize::from(self.0)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecipeDef {
    /// Stable identifier (saves, the player's known recipes): "tessuto".
    pub key: Cow<'static, str>,
    /// What doing it is called, lowercase: "tessere il cotone".
    pub name: Cow<'static, str>,
    pub output: ItemKind,
    /// Units of `output` made by one batch.
    pub batch: u32,
    /// Minutes of work for one batch (NPCs: before the tool bonus).
    pub minutes: Num,
    /// Used up by one batch.
    pub inputs: Cow<'static, [Input]>,
    /// Where the work is done.
    pub station: StationKind,
    /// Known from the start by everyone (the player too); the others have to
    /// be learnt (NPC workers know the recipes of their job).
    pub basic: bool,
    /// When the Custode added it (None: a builtin recipe).
    pub added: Option<GameTime>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Input {
    pub item: ItemKind,
    /// Units used by one batch.
    pub amount: Num,
    /// Where an NPC worker takes it from (the player uses its own items).
    pub from: Source,
}

/// Where the worker takes an input from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    /// The storage of the carriage where the work is done.
    Here,
    /// The carriages of this kind, nearest first.
    Nearest(CarriageKind),
}

impl RecipeDef {
    /// Output per minute of work (before the tool bonus); 0 if the recipe
    /// takes no time or forever.
    pub fn rate(&self, p: &SimParams) -> f32 {
        let rate = self.batch as f32 / self.minutes.get(p);
        if rate.is_finite() && rate > 0.0 {
            rate
        } else {
            0.0
        }
    }

    /// Units of `input` used per unit of output.
    pub fn per_output(&self, input: &Input, p: &SimParams) -> f32 {
        input.amount.get(p) / self.batch.max(1) as f32
    }

    /// Whole units of each input one batch needs from the player.
    pub fn player_inputs(&self, p: &SimParams) -> Vec<(ItemKind, u32)> {
        self.inputs
            .iter()
            .map(|i| (i.item, i.amount.get(p).max(0.0).ceil() as u32))
            .collect()
    }

    /// Game minutes one batch takes the player (at least 1).
    pub fn player_minutes(&self, p: &SimParams) -> u64 {
        let minutes = self.minutes.get(p);
        if minutes.is_finite() {
            minutes.ceil().max(1.0) as u64
        } else {
            u64::MAX
        }
    }
}

pub static RECIPES: [RecipeDef; 12] = [
    VERDURA, COTONE, ERBE, RAZIONE, TE, METALLO, TESSUTO, ATTREZZO, VESTITO, COPERTA, LAMPADA,
    GIOCATTOLO,
];

// --- Serra: aiuole -----------------------------------------------------------

/// Contadini grow it in the Serre.
const VERDURA: RecipeDef = RecipeDef {
    key: Cow::Borrowed("verdura"),
    name: Cow::Borrowed("coltivare verdura"),
    output: ItemKind::Verdura,
    batch: 1,
    minutes: Num::Param(|p| 1.0 / p.verdura_per_farm_minute),
    inputs: Cow::Borrowed(&[]),
    station: StationKind::GrowBed,
    basic: true,
    added: None,
};

/// Contadini grow it in the Serre, once there is enough Verdura.
const COTONE: RecipeDef = RecipeDef {
    key: Cow::Borrowed("cotone"),
    name: Cow::Borrowed("coltivare cotone"),
    output: ItemKind::Cotone,
    batch: 1,
    minutes: Num::Param(|_| 20.0),
    inputs: Cow::Borrowed(&[]),
    station: StationKind::GrowBed,
    basic: true,
    added: None,
};

/// Contadini grow them in the Serre, once there is enough Verdura.
const ERBE: RecipeDef = RecipeDef {
    key: Cow::Borrowed("erbe"),
    name: Cow::Borrowed("coltivare erbe"),
    output: ItemKind::Erbe,
    batch: 1,
    minutes: Num::Param(|_| 20.0),
    inputs: Cow::Borrowed(&[]),
    station: StationKind::GrowBed,
    basic: true,
    added: None,
};

// --- Mensa: cucina -------------------------------------------------------------

/// Cuochi cook it in the Mense from Verdura of the Serre.
const RAZIONE: RecipeDef = RecipeDef {
    key: Cow::Borrowed("razione"),
    name: Cow::Borrowed("cucinare una razione"),
    output: ItemKind::Razione,
    batch: 1,
    minutes: Num::Param(|p| 1.0 / p.razioni_per_cook_minute),
    inputs: Cow::Borrowed(&[Input {
        item: ItemKind::Verdura,
        amount: Num::Param(|p| 1.0 / p.razioni_per_verdura),
        from: Source::Nearest(CarriageKind::Serra),
    }]),
    station: StationKind::Stove,
    basic: true,
    added: None,
};

/// Cuochi brew it in the Mense from Erbe of the Serre, once there are
/// enough Razioni.
const TE: RecipeDef = RecipeDef {
    key: Cow::Borrowed("te"),
    name: Cow::Borrowed("preparare il tè"),
    output: ItemKind::Te,
    batch: 3,
    minutes: Num::Param(|_| 20.0),
    inputs: Cow::Borrowed(&[Input {
        item: ItemKind::Erbe,
        amount: Num::Param(|_| 1.0),
        from: Source::Nearest(CarriageKind::Serra),
    }]),
    station: StationKind::Stove,
    basic: true,
    added: None,
};

// --- Officina: banchi da lavoro -----------------------------------------------

/// Operai cast Rottame into Metallo.
const METALLO: RecipeDef = RecipeDef {
    key: Cow::Borrowed("metallo"),
    name: Cow::Borrowed("fondere il rottame"),
    output: ItemKind::Metallo,
    batch: 1,
    minutes: Num::Param(|_| 20.0),
    inputs: Cow::Borrowed(&[Input {
        item: ItemKind::Rottame,
        amount: Num::Param(|_| 1.0),
        from: Source::Here,
    }]),
    station: StationKind::Workbench,
    basic: true,
    added: None,
};

/// Operai weave Cotone from the Serre into Tessuto.
const TESSUTO: RecipeDef = RecipeDef {
    key: Cow::Borrowed("tessuto"),
    name: Cow::Borrowed("tessere il cotone"),
    output: ItemKind::Tessuto,
    batch: 1,
    minutes: Num::Param(|_| 20.0),
    inputs: Cow::Borrowed(&[Input {
        item: ItemKind::Cotone,
        amount: Num::Param(|_| 1.0),
        from: Source::Nearest(CarriageKind::Serra),
    }]),
    station: StationKind::Workbench,
    basic: true,
    added: None,
};

/// Operai make it from Metallo.
const ATTREZZO: RecipeDef = RecipeDef {
    key: Cow::Borrowed("attrezzo"),
    name: Cow::Borrowed("forgiare un attrezzo"),
    output: ItemKind::Attrezzo,
    batch: 1,
    minutes: Num::Param(|p| 1.0 / p.attrezzi_per_craft_minute),
    inputs: Cow::Borrowed(&[Input {
        item: ItemKind::Metallo,
        amount: Num::Param(|p| p.metallo_per_attrezzo),
        from: Source::Here,
    }]),
    station: StationKind::Workbench,
    basic: true,
    added: None,
};

/// Operai sew it from Tessuto.
const VESTITO: RecipeDef = RecipeDef {
    key: Cow::Borrowed("vestito"),
    name: Cow::Borrowed("cucire un vestito"),
    output: ItemKind::Vestito,
    batch: 1,
    minutes: Num::Param(|p| 1.0 / p.vestiti_per_craft_minute),
    inputs: Cow::Borrowed(&[Input {
        item: ItemKind::Tessuto,
        amount: Num::Param(|p| p.tessuto_per_vestito),
        from: Source::Here,
    }]),
    station: StationKind::Workbench,
    basic: true,
    added: None,
};

/// Operai sew it from Tessuto, for the Dormitori.
const COPERTA: RecipeDef = RecipeDef {
    key: Cow::Borrowed("coperta"),
    name: Cow::Borrowed("cucire una coperta"),
    output: ItemKind::Coperta,
    batch: 1,
    minutes: Num::Param(|_| 60.0),
    inputs: Cow::Borrowed(&[Input {
        item: ItemKind::Tessuto,
        amount: Num::Param(|_| 2.0),
        from: Source::Here,
    }]),
    station: StationKind::Workbench,
    basic: true,
    added: None,
};

/// Operai build it from Metallo and Rottame, for the Dormitori.
const LAMPADA: RecipeDef = RecipeDef {
    key: Cow::Borrowed("lampada"),
    name: Cow::Borrowed("montare una lampada"),
    output: ItemKind::Lampada,
    batch: 1,
    minutes: Num::Param(|_| 60.0),
    inputs: Cow::Borrowed(&[
        Input {
            item: ItemKind::Metallo,
            amount: Num::Param(|_| 1.0),
            from: Source::Here,
        },
        Input {
            item: ItemKind::Rottame,
            amount: Num::Param(|_| 1.0),
            from: Source::Here,
        },
    ]),
    station: StationKind::Workbench,
    basic: false,
    added: None,
};

/// Operai make it from Tessuto and Metallo, for the children of the Dormitori.
const GIOCATTOLO: RecipeDef = RecipeDef {
    key: Cow::Borrowed("giocattolo"),
    name: Cow::Borrowed("costruire un giocattolo"),
    output: ItemKind::Giocattolo,
    batch: 1,
    minutes: Num::Param(|_| 45.0),
    inputs: Cow::Borrowed(&[
        Input {
            item: ItemKind::Tessuto,
            amount: Num::Param(|_| 1.0),
            from: Source::Here,
        },
        Input {
            item: ItemKind::Metallo,
            amount: Num::Param(|_| 1.0),
            from: Source::Here,
        },
    ]),
    station: StationKind::Workbench,
    basic: false,
    added: None,
};
