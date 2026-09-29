//! Cervello degli NPC basato su Laya, un "System One model" che sceglie tra
//! opzioni date (vedi `docs/laya-brain.md`).
//!
//! Tiene il `sim` libero da dipendenze ML. Due metà indipendenti:
//! - `model`: il contratto [`ChoiceModel`] (contesto + opzioni → probabilità)
//!   e le sue implementazioni (il modello Laya vero, un mock per i test);
//! - `brain`: `LayaBrain`, che implementa `sim::Brain` sopra un `ChoiceModel`
//!   in modo asincrono, con `UtilityBrain` come ripiego.

pub mod brain;
pub mod eval;
pub mod laya;
pub mod loader;
pub mod mock;
pub mod model;

pub use brain::{
    DecisionInfo, LayaBrain, LayaConfig, LayaStats, LogEntry, ModelStatus, ReplayBrain,
    ScoredBrain, Source,
};
#[cfg(feature = "laya")]
pub use laya::{LayaModel, LayaOptions};
pub use mock::MockModel;
pub use model::{ChoiceAnswer, ChoiceModel, ChoiceQuery};
