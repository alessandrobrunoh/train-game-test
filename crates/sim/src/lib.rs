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
//!   Mensa; eating takes one Razione. Meals are free for everyone.
//! - Goods: the train sheds Rottame into the Officine every hour; Operai turn
//!   it into Attrezzi or Vestiti (whichever is scarcer on the train); Mercanti
//!   bring them to their Mercato, where NPCs buy them with the tokens earned
//!   as wages. Tokens spent go back to the train administration (they leave
//!   circulation), wages mint new ones.
//! - Owned Attrezzi boost Contadini/Operai output and wear with work; owned
//!   Vestiti slow tiredness and wear daily. Broken ones are bought again.
//!
//! Every storage is capped per carriage and item; Verdura and Razioni spoil a
//! little every midnight.
//!
//! 1 tick = 1 game minute. All randomness comes from seeded ChaCha RNGs, so a
//! world is fully deterministic given its seed and the brain's seed.

pub mod action;
pub mod brain;
pub mod carriage;
pub mod event;
pub mod ids;
pub mod item;
mod names;
pub mod npc;
pub mod params;
pub mod stats;
pub mod time;
pub mod world;

pub use action::{Action, ActionKind, ActionOption, DecisionRequest};
pub use brain::{Brain, RandomBrain, UtilityBrain, UtilityWeights};
pub use carriage::{Carriage, CarriageKind, Station, StationKind};
pub use event::{DeathCause, Event, EventKind};
pub use ids::{CarriageId, NpcId, StationId};
pub use item::{ItemKind, Stock};
pub use npc::{Inventory, Job, Needs, Npc};
pub use params::SimParams;
pub use stats::Stats;
pub use time::{GameTime, MINUTES_PER_DAY, MINUTES_PER_HOUR};
pub use world::{BuyError, GiveError, World};
