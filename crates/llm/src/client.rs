//! The provider: an OpenAI-compatible `chat/completions` endpoint.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::{LlmConfig, LlmError, Request, Response, Usage};

/// A language model that answers one request at a time, blocking.
/// Implementations must be usable from background threads.
pub trait Llm: Send + Sync {
    fn complete(&self, request: &Request) -> Result<Response, LlmError>;
}

/// Longest error body kept in [`LlmError::Http`].
const ERROR_BODY_CHARS: usize = 500;

/// Client for OpenAI-compatible APIs: OpenAI, OpenRouter, Ollama, LM Studio,
/// vLLM and any server speaking the same `chat/completions` protocol.
pub struct OpenAiClient {
    config: LlmConfig,
    agent: ureq::Agent,
}

impl OpenAiClient {
    pub fn new(config: LlmConfig) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(config.timeout))
            // Error statuses come back as responses, to read their body.
            .http_status_as_error(false)
            .build()
            .into();
        Self { config, agent }
    }

    pub fn config(&self) -> &LlmConfig {
        &self.config
    }

    /// The JSON body sent for `request`.
    fn body(&self, request: &Request) -> Value {
        let c = &self.config;
        let mut body = json!({
            "model": c.model,
            "messages": request.messages,
            "max_tokens": request.max_tokens.unwrap_or(c.max_tokens),
        });
        if let Some(t) = request.temperature.or(c.temperature) {
            body["temperature"] = json!(t);
        }
        if request.json && c.json_mode {
            body["response_format"] = json!({ "type": "json_object" });
        }
        body
    }

    /// One attempt, without retries.
    fn call(&self, request: &Request) -> Result<Response, LlmError> {
        let start = Instant::now();
        let mut http = self
            .agent
            .post(&self.config.endpoint)
            .header("Content-Type", "application/json");
        if let Some(key) = &self.config.api_key {
            http = http.header("Authorization", &format!("Bearer {key}"));
        }
        let mut answer = http
            .send(self.body(request).to_string())
            .map_err(|e| self.transport_error(e))?;
        let status = answer.status().as_u16();
        let text = answer
            .body_mut()
            .read_to_string()
            .map_err(|e| self.transport_error(e))?;
        if !(200..300).contains(&status) {
            let body: String = self.scrub(&text).chars().take(ERROR_BODY_CHARS).collect();
            return Err(LlmError::Http { status, body });
        }
        parse_answer(&text, start.elapsed())
    }

    fn transport_error(&self, e: ureq::Error) -> LlmError {
        match e {
            ureq::Error::Timeout(_) => LlmError::Timeout,
            ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut => LlmError::Timeout,
            e => LlmError::Transport(self.scrub(&e.to_string())),
        }
    }

    /// `text` without the API key, should a server echo it back.
    fn scrub(&self, text: &str) -> String {
        match &self.config.api_key {
            Some(key) if !key.is_empty() => text.replace(key.as_str(), "***"),
            _ => text.to_string(),
        }
    }
}

impl Llm for OpenAiClient {
    /// Retries rate limits, overloads and network errors up to
    /// `LLM_MAX_RETRIES` times, waiting a little longer each time.
    fn complete(&self, request: &Request) -> Result<Response, LlmError> {
        let mut attempt = 0;
        loop {
            match self.call(request) {
                Err(e) if e.is_retryable() && attempt < self.config.max_retries => {
                    attempt += 1;
                    std::thread::sleep(Duration::from_millis(500 * u64::from(attempt)));
                }
                result => return result,
            }
        }
    }
}

/// The text, model and usage of a `chat/completions` answer.
fn parse_answer(text: &str, latency: Duration) -> Result<Response, LlmError> {
    let bad = |why: &str| LlmError::BadResponse(why.to_string());
    let v: Value = serde_json::from_str(text).map_err(|e| bad(&format!("not JSON: {e}")))?;
    let message = &v["choices"][0]["message"];
    let content = message["content"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            let reason = v["choices"][0]["finish_reason"].as_str().unwrap_or("?");
            bad(&format!("no text in the answer (finish_reason: {reason})"))
        })?;
    let usage = v["usage"].as_object().map(|u| {
        let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0) as u32;
        Usage {
            prompt_tokens: n("prompt_tokens"),
            completion_tokens: n("completion_tokens"),
        }
    });
    Ok(Response {
        text: content.to_string(),
        model: v["model"].as_str().unwrap_or_default().to_string(),
        usage,
        latency,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answer_parsing() {
        let ok = r#"{"model":"m-1","choices":[{"message":{"role":"assistant","content":"Ciao!"},"finish_reason":"stop"}],"usage":{"prompt_tokens":12,"completion_tokens":3}}"#;
        let r = parse_answer(ok, Duration::from_millis(5)).unwrap();
        assert_eq!(r.text, "Ciao!");
        assert_eq!(r.model, "m-1");
        assert_eq!(
            r.usage,
            Some(Usage {
                prompt_tokens: 12,
                completion_tokens: 3
            })
        );
        let empty = r#"{"choices":[{"message":{"content":null},"finish_reason":"length"}]}"#;
        let e = parse_answer(empty, Duration::ZERO).unwrap_err();
        assert!(e.to_string().contains("length"), "{e}");
        assert!(matches!(
            parse_answer("<html>", Duration::ZERO),
            Err(LlmError::BadResponse(_))
        ));
    }
}
