//! Il modello Laya vero, in-process con [candle](https://github.com/huggingface/candle).
//!
//! Porta in Rust dell'inferenza di Laya (<https://github.com/NandhaKishorM/laya>,
//! Apache-2.0, riferimento: commit `9d955671415fc19f069b9cc998928075c1f255ec`,
//! `laya/common.py` e `laya/agent.py`) per la checkpoint
//! [`download::LAYA_MULTILINGUAL_REPO`] (mmBERT-base + decision head).
//!
//! - [`sequence`]: righe `[CLS] "choice question: …" [SEP] [MASK] opt … [SEP] stato [SEP]`,
//!   padding dei lotti, temperature. Senza dipendenze ML, sempre compilato.
//! - Con la feature `laya`: [`LayaModel`] (tokenizer + encoder + head, CPU),
//!   con `metal` anche sulla GPU Apple.
//!
//! Solo domande `choice`: è l'unico tipo che serve al cervello degli NPC.

pub mod sequence;

#[cfg(feature = "laya")]
mod config;
#[cfg(feature = "laya")]
pub mod download;
#[cfg(feature = "laya")]
mod encoder;
#[cfg(feature = "laya")]
mod head;
#[cfg(feature = "laya")]
mod runtime;
#[cfg(feature = "laya")]
mod tokenize;

#[cfg(all(test, feature = "laya"))]
mod tests;

pub use sequence::OptionLabels;

#[cfg(feature = "laya")]
pub use download::{LAYA_MULTILINGUAL_REPO, LAYA_MULTILINGUAL_REVISION, ModelSource};
#[cfg(feature = "laya")]
pub use runtime::{DevicePreference, LayaModel, LayaOptions, Precision};
