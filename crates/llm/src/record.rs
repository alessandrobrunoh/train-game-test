//! Logging answers and playing them back: a game that used the AI can be
//! reloaded and replayed identically, and tests run on real answers offline.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::{Llm, LlmError, Request, Response};

/// One call as it happened. Failures are kept too, so that a replay takes
/// the same fallbacks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordedCall {
    /// [`request_key`] of `request`.
    pub key: String,
    pub request: Request,
    /// The answer, or the error's message.
    pub outcome: Result<Response, String>,
}

/// A stable key of a request (FNV-1a of its JSON): the same request gives the
/// same key on every run and machine.
pub fn request_key(request: &Request) -> String {
    let json = serde_json::to_string(request).expect("a request serializes");
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in json.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Wraps a model and logs every call.
pub struct Recorder<L> {
    inner: L,
    log: Arc<Mutex<Vec<RecordedCall>>>,
}

impl<L: Llm> Recorder<L> {
    pub fn new(inner: L) -> Self {
        Self {
            inner,
            log: Arc::default(),
        }
    }

    /// The calls logged so far, emptying the log (e.g. when saving).
    pub fn take_log(&self) -> Vec<RecordedCall> {
        std::mem::take(&mut *self.log.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl<L: Llm> Llm for Recorder<L> {
    fn complete(&self, request: &Request) -> Result<Response, LlmError> {
        let result = self.inner.complete(request);
        let call = RecordedCall {
            key: request_key(request),
            request: request.clone(),
            outcome: result.clone().map_err(|e| e.to_string()),
        };
        self.log
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(call);
        result
    }
}

/// Answers from a log: each request gets the recorded answers for the same
/// request, in order. Recorded failures come back as
/// [`LlmError::BadResponse`] (a truncated answer as [`LlmError::Truncated`]);
/// unknown requests as [`LlmError::NotRecorded`].
pub struct Replay {
    answers: Mutex<HashMap<String, VecDeque<Result<Response, String>>>>,
}

impl Replay {
    pub fn new(log: impl IntoIterator<Item = RecordedCall>) -> Self {
        let mut answers: HashMap<String, VecDeque<_>> = HashMap::new();
        for call in log {
            answers.entry(call.key).or_default().push_back(call.outcome);
        }
        Self {
            answers: Mutex::new(answers),
        }
    }
}

impl Llm for Replay {
    fn complete(&self, request: &Request) -> Result<Response, LlmError> {
        let mut answers = self.answers.lock().unwrap_or_else(|e| e.into_inner());
        match answers
            .get_mut(&request_key(request))
            .and_then(VecDeque::pop_front)
        {
            Some(Ok(response)) => Ok(response),
            Some(Err(error)) if error == crate::error::TRUNCATED => Err(LlmError::Truncated),
            Some(Err(error)) => Err(LlmError::BadResponse(error)),
            None => Err(LlmError::NotRecorded),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MockLlm;

    #[test]
    fn keys_are_stable_and_distinct() {
        let a = Request::new("sistema", "ciao");
        assert_eq!(request_key(&a), request_key(&a.clone()));
        assert_ne!(
            request_key(&a),
            request_key(&Request::new("sistema", "ciao!"))
        );
        assert_ne!(request_key(&a), request_key(&a.clone().json()));
        assert_eq!(request_key(&a).len(), 16);
    }

    #[test]
    fn record_then_replay() {
        let rec = Recorder::new(MockLlm::scripted([
            Ok("primo".to_string()),
            Err(LlmError::Timeout),
            Ok("terzo".to_string()),
            Err(LlmError::Truncated),
        ]));
        let (a, b) = (Request::new("s", "a"), Request::new("s", "b"));
        let live = [rec.complete(&a), rec.complete(&b), rec.complete(&a)];
        let c = Request::new("s", "c");
        assert_eq!(rec.complete(&c), Err(LlmError::Truncated));
        let log = rec.take_log();
        assert_eq!(log.len(), 4);
        assert!(rec.take_log().is_empty());
        // Through JSON, as in a save file.
        let json = serde_json::to_string(&log).unwrap();
        let replay = Replay::new(serde_json::from_str::<Vec<RecordedCall>>(&json).unwrap());
        assert_eq!(replay.complete(&a).unwrap().text, "primo");
        assert_eq!(replay.complete(&a).unwrap().text, "terzo");
        assert_eq!(
            replay.complete(&b),
            Err(LlmError::BadResponse("timed out".to_string()))
        );
        assert_eq!(replay.complete(&a), Err(LlmError::NotRecorded));
        // A truncation stays a truncation, so the replay retries the same way.
        assert_eq!(replay.complete(&c), Err(LlmError::Truncated));
        assert_eq!(live[0].as_ref().unwrap().text, "primo");
    }
}
