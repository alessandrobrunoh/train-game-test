use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::{Llm, LlmError, Request, Response};

type Answer = Box<dyn Fn(&Request) -> Result<String, LlmError> + Send + Sync>;

/// A fake model for tests: answers with a function of the request, or with a
/// script of answers in order. Optionally slow, to test waiting.
pub struct MockLlm {
    answer: Answer,
    delay: Duration,
    calls: AtomicUsize,
}

impl MockLlm {
    /// Answers every request with `answer(request)`.
    pub fn new(
        answer: impl Fn(&Request) -> Result<String, LlmError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            answer: Box::new(answer),
            delay: Duration::ZERO,
            calls: AtomicUsize::new(0),
        }
    }

    /// Answers with `script` in order; once it's over, with
    /// [`LlmError::NotRecorded`].
    pub fn scripted(script: impl IntoIterator<Item = Result<String, LlmError>>) -> Self {
        let script = Mutex::new(script.into_iter().collect::<VecDeque<_>>());
        Self::new(move |_| {
            script
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pop_front()
                .unwrap_or(Err(LlmError::NotRecorded))
        })
    }

    /// Always the same text.
    pub fn fixed(text: impl Into<String>) -> Self {
        let text = text.into();
        Self::new(move |_| Ok(text.clone()))
    }

    /// Waits `delay` before every answer.
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// Requests answered so far.
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Llm for MockLlm {
    fn complete(&self, request: &Request) -> Result<Response, LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        (self.answer)(request).map(|text| Response {
            text,
            model: "mock".to_string(),
            usage: None,
            latency: self.delay,
        })
    }
}
