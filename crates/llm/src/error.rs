use std::fmt;

/// Why a call produced no usable answer. Messages never contain the API key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LlmError {
    /// The provider answered with an error status (`body` is truncated).
    Http { status: u16, body: String },
    /// Network problem: DNS, connection refused, TLS…
    Transport(String),
    /// No answer within `LLM_TIMEOUT_SECS`.
    Timeout,
    /// An answer that isn't what was asked (not the expected JSON, no text…).
    BadResponse(String),
    /// The answer hit `max_tokens` (`finish_reason: length`): its text is
    /// missing or cut. Asking again with more tokens, or for a shorter
    /// answer, may succeed.
    Truncated,
    /// The hourly call budget is spent: the caller uses its fallback.
    BudgetExceeded,
    /// [`crate::Replay`] has no recorded answer for this request.
    NotRecorded,
}

impl LlmError {
    /// Whether trying again later may succeed (rate limit, overload, network).
    pub fn is_retryable(&self) -> bool {
        match self {
            LlmError::Http { status, .. } => *status == 429 || *status >= 500,
            LlmError::Transport(_) | LlmError::Timeout => true,
            LlmError::BadResponse(_)
            | LlmError::Truncated
            | LlmError::BudgetExceeded
            | LlmError::NotRecorded => false,
        }
    }
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LlmError::Http { status, body } => write!(f, "HTTP {status}: {body}"),
            LlmError::Transport(e) => write!(f, "network error: {e}"),
            LlmError::Timeout => write!(f, "timed out"),
            LlmError::BadResponse(e) => write!(f, "bad response: {e}"),
            LlmError::Truncated => write!(f, "{TRUNCATED}"),
            LlmError::BudgetExceeded => write!(f, "hourly call budget exceeded"),
            LlmError::NotRecorded => write!(f, "no recorded answer for this request"),
        }
    }
}

impl std::error::Error for LlmError {}

/// The text of [`LlmError::Truncated`] (also how a log records it).
pub(crate) const TRUNCATED: &str = "truncated answer (finish_reason: length)";
