//! Lavori.

use super::recipes::{self, RecipeDef};
use crate::carriage::{CarriageKind, StationKind};
use crate::npc::Job;
use crate::params::SimParams;

#[derive(Debug)]
pub struct JobDef {
    pub job: Job,
    /// Lowercase: "contadino".
    pub name: &'static str,
    /// Kind of carriage where the job is done.
    pub workplace: CarriageKind,
    /// Station the worker occupies there.
    pub station: StationKind,
    /// Work shift as `[start, end)` hours, interrupted by the lunch break.
    pub shift: (u32, u32),
    /// Whether an owned Attrezzo boosts the output (and wears with work).
    pub uses_tool: bool,
    pub work: Work,
}

/// What a minute of work does (see `World::produce`).
#[derive(Debug)]
pub enum Work {
    /// Makes the recipe's output into the workplace storage, taking its
    /// input (if any) from the recipe's source.
    Make(&'static RecipeDef),
    /// Makes one of the recipes: the one whose output is scarcest on the
    /// train first (lowest share of its storage in the workplaces and
    /// outlets), falling back to the next if it can't be made.
    MakeScarcest(&'static [RecipeDef]),
    /// Makes the first recipe (the staple) while its output fills less than
    /// `keep` of the workplace storage; above that, like
    /// [`Work::MakeScarcest`] over all of them. Food first: the surplus
    /// labour makes the rest.
    MakeStaple {
        recipes: &'static [RecipeDef],
        keep: fn(&SimParams) -> f32,
    },
    /// Brings the items sold at the Mercati from the nearest carriages of
    /// `from` to the workplace, the item it has least of first.
    Trade {
        from: CarriageKind,
        /// Units moved per minute of work.
        rate: fn(&SimParams) -> f32,
    },
}

impl Work {
    pub fn recipes(&self) -> &'static [RecipeDef] {
        match self {
            Work::Make(recipe) => std::slice::from_ref(*recipe),
            Work::MakeScarcest(recipes) | Work::MakeStaple { recipes, .. } => recipes,
            Work::Trade { .. } => &[],
        }
    }
}

pub static JOBS: [JobDef; Job::COUNT] = [
    JobDef {
        job: Job::Contadino,
        name: "contadino",
        workplace: CarriageKind::Serra,
        station: StationKind::GrowBed,
        shift: (7, 16),
        uses_tool: true,
        work: Work::MakeStaple {
            recipes: &[recipes::VERDURA, recipes::COTONE, recipes::ERBE],
            keep: |p| p.verdura_keep_fill,
        },
    },
    JobDef {
        job: Job::Cuoco,
        name: "cuoco",
        workplace: CarriageKind::Mensa,
        station: StationKind::Stove,
        shift: (6, 15),
        uses_tool: false,
        work: Work::MakeStaple {
            recipes: &[recipes::RAZIONE, recipes::TE],
            keep: |p| p.razioni_keep_fill,
        },
    },
    JobDef {
        job: Job::Operaio,
        name: "operaio",
        workplace: CarriageKind::Officina,
        station: StationKind::Workbench,
        shift: (8, 17),
        uses_tool: true,
        work: Work::MakeScarcest(&[
            recipes::ATTREZZO,
            recipes::VESTITO,
            recipes::COPERTA,
            recipes::LAMPADA,
            recipes::GIOCATTOLO,
            recipes::METALLO,
            recipes::TESSUTO,
        ]),
    },
    JobDef {
        job: Job::Mercante,
        name: "mercante",
        workplace: CarriageKind::Mercato,
        station: StationKind::Counter,
        shift: (9, 18),
        uses_tool: false,
        work: Work::Trade {
            from: CarriageKind::Officina,
            rate: |p| p.goods_per_trade_minute,
        },
    },
];

impl Job {
    pub fn def(self) -> &'static JobDef {
        &JOBS[self.index()]
    }
}
