//! The Narratore (step A3 of "Direzione nuova" in
//! `docs/piano-vita-ed-economia.md`): it watches the train and asks a big LLM
//! for one novelty per game day (an item, a recipe, a job or an event) that
//! answers a current need or tension of the train.
//!
//! **The AI proposes, the engine decides.** `sim` stays deterministic and
//! AI-free: this crate only reads it. Proposals are drafts: the Custode in
//! `sim` (step A2) will validate them against the live catalogs and apply
//! them to the world at a fixed game minute. Until then nothing is applied.
//!
//! - [`WorldSummary`]: a compact snapshot of the world, the prompt context.
//! - [`Draft`] / [`Proposal`] / [`Effect`]: what the model answers (draft
//!   types; the real ones come from the Custode). Everything refers to items,
//!   recipes and jobs **by name**: the Custode resolves names to stable keys.
//! - [`guard::precheck`]: a provisional local check, the Custode's stand-in,
//!   with an Italian reason the model gets back for its one retry.
//! - [`Narrator`]: builds the Italian prompts, calls the model through an
//!   [`llm::LlmQueue`] inside a [`llm::Budget`] without blocking, parses,
//!   prechecks and retries once.
//!
//! **Recording.** Every call goes through an [`llm::Recorder`]
//! ([`Narrator::take_recording`]) and every exchange is logged with its
//! verdict, latency and tokens ([`Narrator::exchanges`]). That log is for
//! measurement and for replaying LLM answers offline with [`llm::Replay`];
//! it is not the canonical game history: `World::apply` (A2) will record
//! each applied proposal inside the World, so a save alone replays the game.

pub mod guard;
mod narrator;
pub mod proposal;
pub mod summary;

pub use guard::{Known, Rejection, precheck};
pub use narrator::{
    Exchange, Narrator, NarratorConfig, NarratorOutcome, Novelty, Requested, Verdict, parse,
    retry_prompt, suggested_kind, system_prompt, user_prompt,
};
pub use proposal::{Category, Draft, Effect, Ingredient, Need, Proposal, Rationale};
pub use summary::{Catalog, WorldSummary};
