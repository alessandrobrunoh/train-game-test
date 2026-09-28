//! Simulazione del treno: mondo, NPC, economia. Nessuna dipendenza grafica.
//!
//! The sim is location-abstract: an NPC only knows which carriage it is in and
//! which action it performs until when ("Anna è nella carrozza 7, lavora
//! all'aiuola 2 fino alle 14:00"). Decisions happen only when an action ends,
//! so thousands of NPCs are cheap and off-screen carriages never need
//! catching up. The renderer maps `(carriage, action, station)` to sprites.
//!
//! Food flow: Contadini produce food into their Serra's stock; Cuochi working
//! at a Mensa move food from the Serre (nearest first) into that Mensa; eating
//! takes one portion from the Mensa where the NPC eats. Stocks are capped per
//! carriage and a fraction spoils every midnight.
//!
//! 1 tick = 1 game minute. All randomness comes from seeded ChaCha RNGs, so a
//! world is fully deterministic given its seed and the brain's seed.

pub mod action;
pub mod brain;
pub mod carriage;
pub mod event;
pub mod ids;
mod names;
pub mod npc;
pub mod params;
pub mod stats;
pub mod time;
pub mod world;

pub use action::{Action, ActionKind, ActionOption, DecisionRequest};
pub use brain::{Brain, RandomBrain, UtilityBrain, UtilityWeights};
pub use carriage::{Carriage, CarriageKind, Resources, Station, StationKind};
pub use event::{DeathCause, Event, EventKind};
pub use ids::{CarriageId, NpcId, StationId};
pub use npc::{Inventory, Job, Needs, Npc};
pub use params::SimParams;
pub use stats::Stats;
pub use time::{GameTime, MINUTES_PER_DAY, MINUTES_PER_HOUR};
pub use world::World;
