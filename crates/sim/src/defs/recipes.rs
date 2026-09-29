//! Ricette: cosa si fabbrica, da cosa, dove e in quanto tempo.
//!
//! The same rows drive the NPC workers (`World::produce`: a minute of work
//! makes `batch / minutes` of the output, times the tool bonus, taking the
//! inputs from their [`Source`]) and the player (`World::player_craft`: a
//! whole batch from the player's own items, at a carriage with the station).

use crate::carriage::{CarriageKind, StationKind};
use crate::item::ItemKind;
use crate::npc::Job;
use crate::params::SimParams;

#[derive(Debug)]
pub struct RecipeDef {
    /// Stable identifier (saves, the player's known recipes): "tessuto".
    pub key: &'static str,
    /// What doing it is called, lowercase: "tessere il cotone".
    pub name: &'static str,
    pub output: ItemKind,
    /// Units of `output` made by one batch.
    pub batch: u32,
    /// Minutes of work for one batch (NPCs: before the tool bonus).
    pub minutes: fn(&SimParams) -> f32,
    /// Used up by one batch.
    pub inputs: &'static [Input],
    /// Where the work is done.
    pub station: StationKind,
    /// Known from the start by everyone (the player too); the others have to
    /// be learnt (NPC workers know the recipes of their job).
    pub basic: bool,
}

#[derive(Debug)]
pub struct Input {
    pub item: ItemKind,
    /// Units used by one batch.
    pub amount: fn(&SimParams) -> f32,
    /// Where an NPC worker takes it from (the player uses its own items).
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

impl RecipeDef {
    /// Output per minute of work (before the tool bonus); 0 if the recipe
    /// takes no time or forever.
    pub fn rate(&self, p: &SimParams) -> f32 {
        let rate = self.batch as f32 / (self.minutes)(p);
        if rate.is_finite() && rate > 0.0 {
            rate
        } else {
            0.0
        }
    }

    /// Units of `input` used per unit of output.
    pub fn per_output(&self, input: &Input, p: &SimParams) -> f32 {
        (input.amount)(p) / self.batch.max(1) as f32
    }

    /// Whole units of each input one batch needs from the player.
    pub fn player_inputs(&self, p: &SimParams) -> Vec<(ItemKind, u32)> {
        self.inputs
            .iter()
            .map(|i| (i.item, (i.amount)(p).max(0.0).ceil() as u32))
            .collect()
    }

    /// Game minutes one batch takes the player (at least 1).
    pub fn player_minutes(&self, p: &SimParams) -> u64 {
        let minutes = (self.minutes)(p);
        if minutes.is_finite() {
            minutes.ceil().max(1.0) as u64
        } else {
            u64::MAX
        }
    }

    /// The job whose workers make it, if any.
    pub fn maker(&self) -> Option<Job> {
        Job::ALL
            .into_iter()
            .find(|j| j.def().work.recipes().iter().any(|r| r.key == self.key))
    }

    /// Kinds of carriage that have the station it needs.
    pub fn places(&self) -> impl Iterator<Item = CarriageKind> + '_ {
        CarriageKind::ALL
            .into_iter()
            .filter(|k| k.def().stations.iter().any(|s| s.kind == self.station))
    }

    /// Whether it can be done in a carriage of `kind`.
    pub fn can_be_made_in(&self, kind: CarriageKind) -> bool {
        self.places().any(|k| k == kind)
    }

    /// Position in [`RECIPES`].
    pub fn index(&self) -> usize {
        RECIPES
            .iter()
            .position(|r| r.key == self.key)
            .unwrap_or(usize::MAX)
    }

    /// The row with this key.
    pub fn by_key(key: &str) -> Option<&'static RecipeDef> {
        RECIPES.iter().find(|r| r.key == key)
    }
}

pub static RECIPES: [RecipeDef; 12] = [
    VERDURA, COTONE, ERBE, RAZIONE, TE, METALLO, TESSUTO, ATTREZZO, VESTITO, COPERTA, LAMPADA,
    GIOCATTOLO,
];

// --- Serra: aiuole -----------------------------------------------------------

/// Contadini grow it in the Serre.
pub(crate) const VERDURA: RecipeDef = RecipeDef {
    key: "verdura",
    name: "coltivare verdura",
    output: ItemKind::Verdura,
    batch: 1,
    minutes: |p| 1.0 / p.verdura_per_farm_minute,
    inputs: &[],
    station: StationKind::GrowBed,
    basic: true,
};

/// Contadini grow it in the Serre, once there is enough Verdura.
pub(crate) const COTONE: RecipeDef = RecipeDef {
    key: "cotone",
    name: "coltivare cotone",
    output: ItemKind::Cotone,
    batch: 1,
    minutes: |_| 20.0,
    inputs: &[],
    station: StationKind::GrowBed,
    basic: true,
};

/// Contadini grow them in the Serre, once there is enough Verdura.
pub(crate) const ERBE: RecipeDef = RecipeDef {
    key: "erbe",
    name: "coltivare erbe",
    output: ItemKind::Erbe,
    batch: 1,
    minutes: |_| 20.0,
    inputs: &[],
    station: StationKind::GrowBed,
    basic: true,
};

// --- Mensa: cucina -------------------------------------------------------------

/// Cuochi cook it in the Mense from Verdura of the Serre.
pub(crate) const RAZIONE: RecipeDef = RecipeDef {
    key: "razione",
    name: "cucinare una razione",
    output: ItemKind::Razione,
    batch: 1,
    minutes: |p| 1.0 / p.razioni_per_cook_minute,
    inputs: &[Input {
        item: ItemKind::Verdura,
        amount: |p| 1.0 / p.razioni_per_verdura,
        from: Source::Nearest(CarriageKind::Serra),
    }],
    station: StationKind::Stove,
    basic: true,
};

/// Cuochi brew it in the Mense from Erbe of the Serre, once there are
/// enough Razioni.
pub(crate) const TE: RecipeDef = RecipeDef {
    key: "te",
    name: "preparare il tè",
    output: ItemKind::Te,
    batch: 3,
    minutes: |_| 20.0,
    inputs: &[Input {
        item: ItemKind::Erbe,
        amount: |_| 1.0,
        from: Source::Nearest(CarriageKind::Serra),
    }],
    station: StationKind::Stove,
    basic: true,
};

// --- Officina: banchi da lavoro -----------------------------------------------

/// Operai cast Rottame into Metallo.
pub(crate) const METALLO: RecipeDef = RecipeDef {
    key: "metallo",
    name: "fondere il rottame",
    output: ItemKind::Metallo,
    batch: 1,
    minutes: |_| 20.0,
    inputs: &[Input {
        item: ItemKind::Rottame,
        amount: |_| 1.0,
        from: Source::Here,
    }],
    station: StationKind::Workbench,
    basic: true,
};

/// Operai weave Cotone from the Serre into Tessuto.
pub(crate) const TESSUTO: RecipeDef = RecipeDef {
    key: "tessuto",
    name: "tessere il cotone",
    output: ItemKind::Tessuto,
    batch: 1,
    minutes: |_| 20.0,
    inputs: &[Input {
        item: ItemKind::Cotone,
        amount: |_| 1.0,
        from: Source::Nearest(CarriageKind::Serra),
    }],
    station: StationKind::Workbench,
    basic: true,
};

/// Operai make it from Metallo.
pub(crate) const ATTREZZO: RecipeDef = RecipeDef {
    key: "attrezzo",
    name: "forgiare un attrezzo",
    output: ItemKind::Attrezzo,
    batch: 1,
    minutes: |p| 1.0 / p.attrezzi_per_craft_minute,
    inputs: &[Input {
        item: ItemKind::Metallo,
        amount: |p| p.metallo_per_attrezzo,
        from: Source::Here,
    }],
    station: StationKind::Workbench,
    basic: true,
};

/// Operai sew it from Tessuto.
pub(crate) const VESTITO: RecipeDef = RecipeDef {
    key: "vestito",
    name: "cucire un vestito",
    output: ItemKind::Vestito,
    batch: 1,
    minutes: |p| 1.0 / p.vestiti_per_craft_minute,
    inputs: &[Input {
        item: ItemKind::Tessuto,
        amount: |p| p.tessuto_per_vestito,
        from: Source::Here,
    }],
    station: StationKind::Workbench,
    basic: true,
};

/// Operai sew it from Tessuto, for the Dormitori.
pub(crate) const COPERTA: RecipeDef = RecipeDef {
    key: "coperta",
    name: "cucire una coperta",
    output: ItemKind::Coperta,
    batch: 1,
    minutes: |_| 60.0,
    inputs: &[Input {
        item: ItemKind::Tessuto,
        amount: |_| 2.0,
        from: Source::Here,
    }],
    station: StationKind::Workbench,
    basic: true,
};

/// Operai build it from Metallo and Rottame, for the Dormitori.
pub(crate) const LAMPADA: RecipeDef = RecipeDef {
    key: "lampada",
    name: "montare una lampada",
    output: ItemKind::Lampada,
    batch: 1,
    minutes: |_| 60.0,
    inputs: &[
        Input {
            item: ItemKind::Metallo,
            amount: |_| 1.0,
            from: Source::Here,
        },
        Input {
            item: ItemKind::Rottame,
            amount: |_| 1.0,
            from: Source::Here,
        },
    ],
    station: StationKind::Workbench,
    basic: false,
};

/// Operai make it from Tessuto and Metallo, for the children of the Dormitori.
pub(crate) const GIOCATTOLO: RecipeDef = RecipeDef {
    key: "giocattolo",
    name: "costruire un giocattolo",
    output: ItemKind::Giocattolo,
    batch: 1,
    minutes: |_| 45.0,
    inputs: &[
        Input {
            item: ItemKind::Tessuto,
            amount: |_| 1.0,
            from: Source::Here,
        },
        Input {
            item: ItemKind::Metallo,
            amount: |_| 1.0,
            from: Source::Here,
        },
    ],
    station: StationKind::Workbench,
    basic: false,
};
