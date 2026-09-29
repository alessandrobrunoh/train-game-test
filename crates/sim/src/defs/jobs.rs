//! Lavori: le righe dei 4 lavori di partenza.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::Num;
use super::recipes::RecipeId;
use crate::carriage::{CarriageKind, StationKind};
use crate::custode::Need;
use crate::job::JobInfo;
use crate::npc::Job;
use crate::time::GameTime;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobDef {
    /// Id, names, workplace, station, shift, tool (see [`JobInfo`]).
    pub job: Job,
    /// What the job is, one short Italian sentence.
    pub description: Cow<'static, str>,
    pub work: Work,
    /// Workers wanted per carriage of the workplace kind, for a job the
    /// Custode added; None for the builtin jobs, staffed by the quotas of
    /// `World::staff_workforce`.
    pub staff: Option<u16>,
    /// When the Custode added it (None: a builtin job).
    pub added: Option<GameTime>,
}

impl JobDef {
    pub fn name(&self) -> &'static str {
        self.job.name()
    }
}

/// What a minute of work does (see `World::produce`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Work {
    /// Makes the recipe's output into the workplace storage, taking its
    /// input (if any) from the recipe's source.
    Make(RecipeId),
    /// Makes one of the recipes: the one whose output is scarcest on the
    /// train first (lowest share of its storage in the workplaces and
    /// outlets), falling back to the next if it can't be made.
    MakeScarcest(Cow<'static, [RecipeId]>),
    /// Makes the first recipe (the staple) while its output fills less than
    /// `keep` of the workplace storage; above that, like
    /// [`Work::MakeScarcest`] over all of them. Food first: the surplus
    /// labour makes the rest.
    MakeStaple {
        recipes: Cow<'static, [RecipeId]>,
        keep: Num,
    },
    /// Brings the items sold at the Mercati from the nearest carriages of
    /// `from` to the workplace, the item it has least of first.
    Trade {
        from: CarriageKind,
        /// Units moved per minute of work.
        rate: Num,
    },
    /// Makes nothing: a service to the people in the workplace (a guard, a
    /// doctor, a clerk). Every minute of work raises `need` of everyone
    /// else in the same carriage by `per_minute` (up to 1).
    Service { need: Need, per_minute: f32 },
}

impl Work {
    pub fn recipes(&self) -> &[RecipeId] {
        match self {
            Work::Make(recipe) => std::slice::from_ref(recipe),
            Work::MakeScarcest(recipes) | Work::MakeStaple { recipes, .. } => recipes,
            Work::Trade { .. } | Work::Service { .. } => &[],
        }
    }
}

/// Code of each builtin job in the saves and in `Debug`, by id.
pub(crate) const BUILTIN_JOB_CODES: [&str; Job::BUILTIN_COUNT] =
    ["Contadino", "Cuoco", "Operaio", "Mercante"];

/// Names, place and shift of each builtin job, by id.
pub(crate) static BUILTIN_JOB_INFOS: [JobInfo; Job::BUILTIN_COUNT] = [
    JobInfo {
        key: "contadino",
        name: "contadino",
        plural: "contadini",
        workplace: CarriageKind::Serra,
        station: StationKind::GrowBed,
        shift: (7, 16),
        uses_tool: true,
    },
    JobInfo {
        key: "cuoco",
        name: "cuoco",
        plural: "cuochi",
        workplace: CarriageKind::Mensa,
        station: StationKind::Stove,
        shift: (6, 15),
        uses_tool: false,
    },
    JobInfo {
        key: "operaio",
        name: "operaio",
        plural: "operai",
        workplace: CarriageKind::Officina,
        station: StationKind::Workbench,
        shift: (8, 17),
        uses_tool: true,
    },
    JobInfo {
        key: "mercante",
        name: "mercante",
        plural: "mercanti",
        workplace: CarriageKind::Mercato,
        station: StationKind::Counter,
        shift: (9, 18),
        uses_tool: false,
    },
];

pub static JOBS: [JobDef; Job::BUILTIN_COUNT] = [
    JobDef {
        job: Job::Contadino,
        description: Cow::Borrowed("Coltiva le aiuole di una Serra: verdura, poi cotone ed erbe."),
        work: Work::MakeStaple {
            recipes: Cow::Borrowed(&[RecipeId::VERDURA, RecipeId::COTONE, RecipeId::ERBE]),
            keep: Num::Param(|p| p.verdura_keep_fill),
        },
        staff: None,
        added: None,
    },
    JobDef {
        job: Job::Cuoco,
        description: Cow::Borrowed("Cucina in Mensa: razioni per tutti, poi il tè."),
        work: Work::MakeStaple {
            recipes: Cow::Borrowed(&[RecipeId::RAZIONE, RecipeId::TE]),
            keep: Num::Param(|p| p.razioni_keep_fill),
        },
        staff: None,
        added: None,
    },
    JobDef {
        job: Job::Operaio,
        description: Cow::Borrowed(
            "Lavora al banco di un'Officina: metallo, tessuto e quello che manca di più.",
        ),
        work: Work::MakeScarcest(Cow::Borrowed(&[
            RecipeId::ATTREZZO,
            RecipeId::VESTITO,
            RecipeId::COPERTA,
            RecipeId::LAMPADA,
            RecipeId::GIOCATTOLO,
            RecipeId::METALLO,
            RecipeId::TESSUTO,
        ])),
        staff: None,
        added: None,
    },
    JobDef {
        job: Job::Mercante,
        description: Cow::Borrowed("Porta attrezzi e vestiti dalle Officine al suo Mercato."),
        work: Work::Trade {
            from: CarriageKind::Officina,
            rate: Num::Param(|p| p.goods_per_trade_minute),
        },
        staff: None,
        added: None,
    },
];
