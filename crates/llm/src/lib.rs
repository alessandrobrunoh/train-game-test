//! Client for the "big" language models that build the world (see "Direzione
//! nuova" in `docs/piano-vita-ed-economia.md`).
//!
//! - [`LlmConfig`]: where the model lives, read from `.env` / the environment
//!   (`LLM_API_URL`, `LLM_API_KEY`, `LLM_MODEL`, …). No configuration means no
//!   AI: the game keeps running on its fast brains.
//! - [`Llm`]: one blocking completion. [`OpenAiClient`] speaks the
//!   OpenAI-compatible chat API (OpenAI, OpenRouter, Ollama, LM Studio, vLLM…);
//!   [`MockLlm`] answers from a script, for tests.
//! - [`LlmQueue`]: requests run on background threads inside a [`Budget`] of
//!   calls per hour; the caller polls for results and never waits.
//! - [`Recorder`] / [`Replay`]: every answer can be logged and played back, so
//!   a game that used the AI can be reloaded and replayed identically.
//! - [`complete_json`]: asks for JSON, extracts it from the answer and retries
//!   once with the parse error.
//!
//! The simulation stays deterministic: it never calls this crate directly.
//! Whoever drives the AI submits a request, and applies the answer at a game
//! minute fixed in advance (or falls back to the fast brain if it's late).

mod budget;
mod client;
mod config;
mod error;
mod json;
mod message;
mod mock;
mod queue;
mod record;

pub use budget::Budget;
pub use client::{Llm, OpenAiClient};
pub use config::{ConfigError, LlmConfig, parse_dotenv};
pub use error::LlmError;
pub use json::{complete_json, extract_json};
pub use message::{Message, Request, Response, Role, Usage};
pub use mock::MockLlm;
pub use queue::{Done, LlmQueue, Ticket};
pub use record::{RecordedCall, Recorder, Replay, request_key};
