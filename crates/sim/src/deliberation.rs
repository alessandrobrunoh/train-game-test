//! Deliberazioni: decisioni di vita rare e importanti.
//!
//! Everyday actions are picked by a [`crate::Brain`] among short options every
//! few minutes. A few life choices are different: rare, with lasting effects
//! and best described in words ("Marta accetta la proposta di Luca?"). The
//! world opens a [`Deliberation`] for them, with an Italian question, a
//! situation text from the NPC's point of view and 2–4 natural-language
//! options. A brain may answer it ([`crate::Brain::deliberations_resolved`]);
//! otherwise the built-in rule (a scored, seeded random choice, see
//! [`crate::World::deliberation_rule_weights`]) decides at the deadline.
//!
//! Kinds (see [`DeliberationKind`]): a couple proposal, whether to have a
//! child, the temptation to steal, and whether to protest. Life goes on while
//! a deliberation is open: the NPC keeps acting normally.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::{CarriageId, NpcId};
use crate::item::ItemKind;
use crate::time::GameTime;

/// Stable deliberation identifier (never reused).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DeliberationId(pub u64);

impl fmt::Display for DeliberationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "D{}", self.0)
    }
}

/// What people protest against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Grievance {
    /// The administration refused the NPC's couple a child.
    BirthDenied,
    /// The Mense ran out of Razioni.
    FoodShortage,
}

impl Grievance {
    pub const ALL: [Grievance; 2] = [Grievance::BirthDenied, Grievance::FoodShortage];

    pub fn index(self) -> usize {
        self as usize
    }

    /// "contro il divieto di nascite".
    pub fn against(self) -> &'static str {
        match self {
            Grievance::BirthDenied => "contro il divieto di nascite",
            Grievance::FoodShortage => "contro la mancanza di razioni",
        }
    }
}

/// What is being decided, and by whom (the deliberating NPC is
/// [`Deliberation::npc`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeliberationKind {
    /// `from` asked the NPC to become a couple. Options: [`Choice::Accept`],
    /// [`Choice::Refuse`], [`Choice::AskForTime`].
    CoupleProposal { from: NpcId },
    /// The administration would allow the NPC (the woman of the couple) and
    /// `partner` a child. Options: [`Choice::TryForChild`], [`Choice::Wait`].
    HaveChild { partner: NpcId },
    /// The NPC needs `item` but cannot afford it at the Mercato `market`.
    /// Options: [`Choice::Steal`], [`Choice::Save`] and, if someone could
    /// help, [`Choice::AskForHelp`] (asking `helper`).
    Theft {
        item: ItemKind,
        market: CarriageId,
        helper: Option<NpcId>,
    },
    /// The NPC was hit by `grievance`; a protest gathers in `place`. Options:
    /// [`Choice::Protest`], [`Choice::Endure`] and, for workers,
    /// [`Choice::WorkHarder`].
    Protest {
        grievance: Grievance,
        place: CarriageId,
    },
}

impl DeliberationKind {
    pub const COUNT: usize = 4;
    /// Short names, indexed by [`DeliberationKind::index`].
    pub const TOPICS: [&'static str; Self::COUNT] =
        ["proposta di coppia", "avere un figlio", "furto", "protesta"];

    /// Position in per-kind arrays (see [`DeliberationCounters`]).
    pub fn index(self) -> usize {
        match self {
            DeliberationKind::CoupleProposal { .. } => 0,
            DeliberationKind::HaveChild { .. } => 1,
            DeliberationKind::Theft { .. } => 2,
            DeliberationKind::Protest { .. } => 3,
        }
    }

    /// Short Italian topic: "proposta di coppia", "avere un figlio", "furto", "protesta".
    pub fn topic(self) -> &'static str {
        Self::TOPICS[self.index()]
    }

    /// The other NPC involved, if any (proposer, partner, or who would be asked for help).
    pub fn other(self) -> Option<NpcId> {
        match self {
            DeliberationKind::CoupleProposal { from } => Some(from),
            DeliberationKind::HaveChild { partner } => Some(partner),
            DeliberationKind::Theft { helper, .. } => helper,
            DeliberationKind::Protest { .. } => None,
        }
    }
}

/// A possible answer. The same choice always means the same effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Choice {
    /// Couple proposal: become partners (they move in together).
    Accept,
    /// Couple proposal: say no (affinity drops, no new proposal for a while).
    Refuse,
    /// Couple proposal: ask for time (the proposal comes back in a few days).
    AskForTime,
    /// Child: try now (a child is born if the administration still allows it).
    TryForChild,
    /// Child: not now.
    Wait,
    /// Theft: take the item, risking to be caught (fine, lost affinity).
    Steal,
    /// Theft: give up and save tokens.
    Save,
    /// Theft: ask the helper for tokens.
    AskForHelp,
    /// Protest: join the gathering (leaves work for a few hours).
    Protest,
    /// Protest: accept the situation.
    Endure,
    /// Protest: resign oneself and work more.
    WorkHarder,
}

impl Choice {
    pub const COUNT: usize = 11;
    pub const ALL: [Choice; Self::COUNT] = [
        Choice::Accept,
        Choice::Refuse,
        Choice::AskForTime,
        Choice::TryForChild,
        Choice::Wait,
        Choice::Steal,
        Choice::Save,
        Choice::AskForHelp,
        Choice::Protest,
        Choice::Endure,
        Choice::WorkHarder,
    ];

    /// Position in [`Choice::ALL`] (and in per-choice arrays).
    pub fn index(self) -> usize {
        self as usize
    }

    /// Stable machine key in Italian, e.g. "accetta", "chiede_tempo".
    pub fn key(self) -> &'static str {
        match self {
            Choice::Accept => "accetta",
            Choice::Refuse => "rifiuta",
            Choice::AskForTime => "chiede_tempo",
            Choice::TryForChild => "provarci_ora",
            Choice::Wait => "aspettare",
            Choice::Steal => "rubare",
            Choice::Save => "risparmiare",
            Choice::AskForHelp => "chiedere_aiuto",
            Choice::Protest => "protestare",
            Choice::Endure => "accettare",
            Choice::WorkHarder => "lavorare_di_piu",
        }
    }
}

/// One option of a deliberation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeliberationOption {
    pub choice: Choice,
    /// Natural-language Italian, third person, e.g. "accetta e diventa la
    /// compagna di Luca Rossi".
    pub description: String,
}

/// An open question for one NPC. Texts are written when it opens (a snapshot
/// of the situation then).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Deliberation {
    pub id: DeliberationId,
    /// Who decides.
    pub npc: NpcId,
    pub kind: DeliberationKind,
    pub asked: GameTime,
    /// The built-in rule decides at this time if no brain answered before.
    pub deadline: GameTime,
    /// Italian, e.g. "Marta Rossi accetta la proposta di Luca Bianchi di diventare una coppia?".
    pub question: String,
    /// Italian situation text from the NPC's point of view: its state (like
    /// [`crate::World::npc_context`]), character, the people involved and what is at stake.
    pub context: String,
    /// 2–4 options; answers refer to them by index.
    pub options: Vec<DeliberationOption>,
}

impl Deliberation {
    /// Index of the option with `choice`, if offered.
    pub fn option_of(&self, choice: Choice) -> Option<usize> {
        self.options.iter().position(|o| o.choice == choice)
    }
}

/// A brain's answer to an open deliberation (see
/// [`crate::Brain::deliberations_resolved`]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeliberationAnswer {
    pub id: DeliberationId,
    /// Index into [`Deliberation::options`].
    pub choice: usize,
    /// The brain's confidence in `0..=1` (reported in events and history).
    pub confidence: f32,
}

/// Who resolved a deliberation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Resolver {
    /// The built-in rule (at the deadline, or at once for brains that do not
    /// answer deliberations).
    Rules,
    /// The brain's answer.
    Brain,
}

impl Resolver {
    pub fn name(self) -> &'static str {
        match self {
            Resolver::Rules => "regole",
            Resolver::Brain => "cervello",
        }
    }
}

/// A closed deliberation, kept for a while for the UI
/// ([`crate::World::recent_deliberations`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResolvedDeliberation {
    pub deliberation: Deliberation,
    /// Index into `deliberation.options`.
    pub choice: usize,
    pub by: Resolver,
    pub confidence: Option<f32>,
    pub resolved: GameTime,
}

impl ResolvedDeliberation {
    pub fn chosen(&self) -> Option<&DeliberationOption> {
        self.deliberation.options.get(self.choice)
    }
}

/// A protest: who joined it, where and when they gather. While it runs,
/// its members only travel to `place` and stand there.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gathering {
    pub grievance: Grievance,
    pub place: CarriageId,
    pub start: GameTime,
    pub end: GameTime,
    pub members: Vec<NpcId>,
}

impl Gathering {
    pub fn is_running(&self, now: GameTime) -> bool {
        self.start <= now && now < self.end
    }
}

/// Deliberation counters since the world was generated.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliberationCounters {
    /// Opened, per [`DeliberationKind::index`].
    pub opened: [u64; DeliberationKind::COUNT],
    /// Resolved by the rules / by the brain, per kind.
    pub by_rules: [u64; DeliberationKind::COUNT],
    pub by_brain: [u64; DeliberationKind::COUNT],
    /// Resolutions per [`Choice::index`].
    pub choices: [u64; Choice::COUNT],
    /// Closed without a decision (someone involved died, or the situation
    /// changed: e.g. the proposer found another partner).
    pub cancelled: u64,
    /// Brain answers ignored (unknown or closed id, option out of range).
    pub answers_ignored: u64,
    pub thefts: u64,
    pub thefts_caught: u64,
    pub help_given: u64,
    pub help_refused: u64,
    /// Protest gatherings called, and concessions of the administration.
    pub protests_called: u64,
    pub concessions: u64,
}

impl DeliberationCounters {
    pub fn opened_total(&self) -> u64 {
        self.opened.iter().sum()
    }

    pub fn resolved_total(&self) -> u64 {
        self.by_rules.iter().chain(&self.by_brain).sum()
    }

    pub fn chosen(&self, choice: Choice) -> u64 {
        self.choices[choice.index()]
    }
}
