//! Simulazione del treno: mondo, NPC, economia. Nessuna dipendenza grafica.
//!
//! The sim is location-abstract: an NPC only knows which carriage it is in and
//! which action it performs until when ("Anna è nella carrozza 7, lavora
//! all'aiuola 2 fino alle 14:00"). Decisions happen only when an action ends,
//! so thousands of NPCs are cheap and off-screen carriages never need
//! catching up. The renderer maps `(carriage, action, station)` to sprites.
//!
//! Item flows (see [`item`]):
//! - Food: Contadini grow Verdura into their Serra; Cuochi at a Mensa cook
//!   Verdura taken from the Serre (nearest first) into Razioni stored in that
//!   Mensa; eating takes one Razione. Meals are free for everyone. People
//!   eat in meal shifts ([`Npc::meal_shift`]); in a full Mensa they queue for
//!   a seat ([`Action::Wait`]); see [`World::mensa_occupancy`].
//! - Goods: the train sheds Rottame into the Officine every hour; Operai turn
//!   it into Attrezzi or Vestiti (whichever is scarcer on the train); Mercanti
//!   bring them to their Mercato, where NPCs buy them with the tokens earned
//!   as wages.
//! - Money is a closed loop ([`Economy`]): wages and stipends are paid every
//!   midnight from the administration's treasury; purchases, fines, a tax on
//!   large savings and estates without heirs go back to it. Pay and prices
//!   follow a slow feedback on the treasury.
//! - Owned Attrezzi boost Contadini/Operai output and wear with work; owned
//!   Vestiti slow tiredness and wear daily. Broken ones are bought again.
//!
//! Per-kind properties (items, recipes, carriages, stations, jobs) are data
//! tables in [`defs`].
//!
//! Every storage is capped per carriage and item; Verdura and Razioni spoil a
//! little every midnight.
//!
//! Life cycle (once per game day, at midnight; a year lasts
//! `SimParams::days_per_year` days): people age ([`LifeStage`]: children and
//! youths don't work, adults do, the elderly retire), make friends by chatting
//! ([`Relation`]), form couples, have children when the train administration
//! allows it (free beds, enough food, below [`World::max_population`]), and die
//! of old age (or hunger). Jobs follow the population: new adults take the job
//! most needed, food first. See [`World::life`] and [`Stats`].
//!
//! Deliberations ([`deliberation`]): rare, meaningful life choices (accepting
//! a couple proposal, having a child, stealing, protesting) are asked as an
//! Italian question with 2–4 options; a brain may answer them, otherwise a
//! built-in rule decides. See [`World::open_deliberations`].
//!
//! Conversations ([`dialogue`]): a chat (`Action::Socialize`) with someone
//! free nearby becomes a two-sided [`Conversation`] with a topic, a tone set
//! by the pair's tie and [`Personality`], and short Italian lines for speech
//! bubbles; at the end it changes their affinity and spreads gossip. See
//! [`World::conversations`].
//!
//! 1 tick = 1 game minute. All randomness comes from seeded ChaCha RNGs, so a
//! world is fully deterministic given its seed and the brain's seed.

pub mod action;
pub mod brain;
pub mod carriage;
pub mod defs;
pub mod deliberation;
pub mod dialogue;
pub mod event;
pub mod ids;
pub mod item;
mod names;
pub mod npc;
pub mod params;
pub mod personality;
pub mod stats;
pub mod time;
pub mod world;

pub use action::{Action, ActionKind, ActionOption, DecisionRequest};
pub use brain::{Brain, RandomBrain, THINK, UtilityBrain, UtilityWeights};
pub use carriage::{Carriage, CarriageKind, Station, StationKind};
pub use defs::{CarriageDef, ItemDef, ItemUse, JobDef, RecipeDef, Work};
pub use deliberation::{
    Choice, Deliberation, DeliberationAnswer, DeliberationCounters, DeliberationId,
    DeliberationKind, DeliberationOption, Gathering, Grievance, ResolvedDeliberation, Resolver,
};
pub use dialogue::{
    Conversation, ConversationCounters, ConversationId, Line, News, Tone, Topic, Valence,
};
pub use event::{BirthDenial, DeathCause, Event, EventKind};
pub use ids::{CarriageId, NpcId, StationId};
pub use item::{ItemKind, Stock};
pub use npc::{
    Inventory, Job, LifeStage, MAX_RELATIONS, Needs, Npc, Relation, RelationKind, Sex, Traits,
};
pub use params::SimParams;
pub use personality::{Personality, Temper};
pub use stats::Stats;
pub use time::{GameTime, MINUTES_PER_DAY, MINUTES_PER_HOUR};
pub use world::{BuyError, Economy, EconomyCounters, GiveError, LifeCounters, Tally, World};
pub use world::{MensaOccupancy, MensaRole};
