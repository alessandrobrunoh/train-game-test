//! The Narratore (step A3 of "Direzione nuova" in
//! `docs/piano-vita-ed-economia.md`): it watches the train and asks a big LLM
//! for one novelty per game day (an item, a recipe, a job or an event) that
//! answers a current need or tension of the train.
//!
//! **The AI proposes, the engine decides.** `sim` stays deterministic and
//! AI-free: this crate only reads it. An accepted draft goes to the Custode
//! in `sim` (`World::schedule`, `World::review`, `World::apply`), which
//! checks it against the world's live catalogs and makes it real at a fixed
//! game minute, or refuses it with a reason.
//!
//! - [`WorldSummary`]: a compact snapshot of the world, the prompt context.
//! - [`Draft`] / [`Proposal`] / [`Effect`]: what the model answers, the
//!   Custode's own types (`sim::custode`, re-exported with [`panel`],
//!   [`sources`], [`statistic`] and [`appearance`]). Everything refers to
//!   items, recipes and jobs **by name**: the Custode resolves names to ids.
//! - [`guard::precheck`]: the shape of an answer (lengths, names, closed
//!   lists, references to what exists) checked before the Custode sees it,
//!   with an Italian reason the model gets back for its one retry.
//! - [`Panel`] (`interfaccia`): custom UI attached to a novelty, as
//!   declarative data the game renders; values and lists come from the
//!   closed [`Source`] / [`ListSource`] whitelists (evaluated read-only in
//!   [`sources`]), buttons from the closed [`Action`] whitelist.
//! - [`Statistic`]: a derived number (a tiny closed [`Formula`] over
//!   sources), evaluated read-only by a [`StatBook`].
//! - [`Appearance`] (`aspetto`): shape, colour and detail of a new item's
//!   procedural icon, from closed lists.
//! - [`Narrator`]: builds the Italian prompts, calls the model through an
//!   [`llm::LlmQueue`] inside a [`llm::Budget`] without blocking, parses,
//!   prechecks and retries once.
//!
//! **Recording.** Every call goes through an [`llm::Recorder`]
//! ([`Narrator::take_recording`]) and every exchange is logged with its
//! verdict, latency and tokens ([`Narrator::exchanges`]). That log is for
//! measurement and for replaying LLM answers offline with [`llm::Replay`];
//! it is not the canonical game history: the World records every decision of
//! the Custode (`World::decisions`), so a save alone replays the game.

pub mod appearance;
pub mod guard;
mod narrator;
pub mod panel;
pub mod proposal;
pub mod sources;
pub mod statistic;
pub mod summary;

pub use appearance::{Appearance, Colour, Detail, Shape};
pub use panel::{Action, Element, Format, Panel};
pub use sources::{ListSource, Reading, Source};
pub use statistic::{Formula, StatBook, Statistic, Threshold};

pub use guard::{Known, Rejection, precheck};
pub use narrator::{
    Exchange, MAX_SUGGESTED_STATS, Narrator, NarratorConfig, NarratorOutcome, Novelty, Requested,
    SHORTER_PROMPT, Verdict, describe, parse, retry_prompt, suggested_kind, system_prompt,
    user_prompt,
};
pub use proposal::{Category, Draft, Effect, Ingredient, Need, Proposal, Rationale};
pub use summary::{Catalog, WorldSummary};
