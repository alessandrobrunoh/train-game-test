//! Structured answers: the model answers in JSON, checked with serde.

use serde::de::DeserializeOwned;

use crate::{Llm, LlmError, Message, Request};

/// The JSON value in a model's answer: the whole text, or the content of a
/// ```` ``` ```` fence, or the span from the first `{`/`[` to the last
/// `}`/`]` (models like to add a sentence around it).
pub fn extract_json(text: &str) -> Option<&str> {
    let text = text.trim();
    if let Some(start) = text.find("```") {
        let after = &text[start + 3..];
        // Skip the language tag (```json).
        let body = after.split_once('\n').map_or(after, |(_, b)| b);
        if let Some(end) = body.find("```") {
            return extract_json(&body[..end]);
        }
    }
    let open = text.find(['{', '['])?;
    let close = if text[open..].starts_with('{') {
        text.rfind('}')?
    } else {
        text.rfind(']')?
    };
    (close > open).then(|| &text[open..=close])
}

/// Completes `request` (marked as JSON) and parses the answer as `T`. If the
/// answer isn't valid, asks once more with the parse error; then gives up
/// with [`LlmError::BadResponse`].
pub fn complete_json<T: DeserializeOwned>(llm: &dyn Llm, request: &Request) -> Result<T, LlmError> {
    let mut request = request.clone().json();
    let mut error = String::new();
    for attempt in 0..2 {
        let answer = llm.complete(&request)?;
        let parsed = extract_json(&answer.text)
            .ok_or_else(|| "nessun JSON nella risposta".to_string())
            .and_then(|json| serde_json::from_str::<T>(json).map_err(|e| e.to_string()));
        match parsed {
            Ok(value) => return Ok(value),
            Err(e) => error = e,
        }
        if attempt == 0 {
            request.messages.push(Message::assistant(answer.text));
            request.messages.push(Message::user(format!(
                "La risposta non è JSON valido per il formato richiesto ({error}). \
                 Rispondi di nuovo con il solo JSON, senza altro testo."
            )));
        }
    }
    Err(LlmError::BadResponse(error))
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;
    use crate::MockLlm;

    #[test]
    fn json_is_found_in_chatty_answers() {
        assert_eq!(extract_json(r#"{"a":1}"#), Some(r#"{"a":1}"#));
        assert_eq!(
            extract_json("Ecco il lavoro:\n```json\n{\"a\": [1, 2]}\n```\nSpero vada bene."),
            Some(r#"{"a": [1, 2]}"#)
        );
        assert_eq!(
            extract_json("Certo! {\"nome\": \"Erborista\"} Fatto."),
            Some(r#"{"nome": "Erborista"}"#)
        );
        assert_eq!(extract_json("[1, 2]"), Some("[1, 2]"));
        assert_eq!(extract_json("niente qui"), None);
        assert_eq!(extract_json("} al contrario {"), None);
    }

    #[derive(Debug, PartialEq, Deserialize)]
    struct Lavoro {
        nome: String,
        carrozza: String,
    }

    #[test]
    fn a_bad_answer_is_retried_once_with_the_error() {
        let llm = MockLlm::new(|r| {
            // The second attempt carries the first answer and the error.
            Ok(if r.messages.len() == 2 {
                "Non so, forse un Erborista?".to_string()
            } else {
                assert!(r.messages[3].content.contains("JSON"));
                r#"{"nome": "Erborista", "carrozza": "Serra"}"#.to_string()
            })
        });
        let got: Lavoro = complete_json(&llm, &Request::new("s", "inventa un lavoro")).unwrap();
        assert_eq!(
            got,
            Lavoro {
                nome: "Erborista".into(),
                carrozza: "Serra".into()
            }
        );
        assert_eq!(llm.calls(), 2);
    }

    #[test]
    fn gives_up_after_the_retry() {
        let llm = MockLlm::fixed(r#"{"nome": "Erborista"}"#);
        let err = complete_json::<Lavoro>(&llm, &Request::new("s", "u")).unwrap_err();
        assert!(
            matches!(&err, LlmError::BadResponse(e) if e.contains("carrozza")),
            "{err}"
        );
        assert_eq!(llm.calls(), 2);
        // Network errors are not retried here (the client already did).
        let down = MockLlm::scripted([Err(LlmError::Timeout)]);
        assert_eq!(
            complete_json::<Lavoro>(&down, &Request::new("s", "u")),
            Err(LlmError::Timeout)
        );
        assert_eq!(down.calls(), 1);
    }
}
