//! `OpenAiClient` against a tiny local HTTP server: what goes on the wire and
//! how answers and failures come back.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use llm::{Llm, LlmConfig, LlmError, OpenAiClient, Request};

/// What the server received.
struct Seen {
    request_line: String,
    headers: Vec<(String, String)>,
    body: serde_json::Value,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Serves `replies` (status, body, delay) to successive connections; returns
/// the base URL and what each request looked like.
fn server(replies: Vec<(u16, String, Duration)>) -> (String, mpsc::Receiver<Seen>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for (status, body, delay) in replies {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut headers = Vec::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                let (k, v) = line.split_once(':').unwrap();
                headers.push((k.trim().to_string(), v.trim().to_string()));
            }
            let len: usize = headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                .map_or(0, |(_, v)| v.parse().unwrap());
            let mut raw = vec![0; len];
            reader.read_exact(&mut raw).unwrap();
            let seen = Seen {
                request_line: request_line.trim_end().to_string(),
                headers,
                body: serde_json::from_slice(&raw).unwrap_or_default(),
            };
            let _ = tx.send(seen);
            thread::sleep(delay);
            let mut stream = stream;
            let _ = write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (url, rx)
}

fn client(url: &str, key: Option<&str>, extra: &[(&str, &str)]) -> OpenAiClient {
    let mut vars = vec![
        ("LLM_API_URL", url.to_string()),
        ("LLM_MODEL", "modello-prova".to_string()),
        ("LLM_TIMEOUT_SECS", "0.5".to_string()),
        ("LLM_MAX_RETRIES", "0".to_string()),
    ];
    if let Some(key) = key {
        vars.push(("LLM_API_KEY", key.to_string()));
    }
    vars.extend(extra.iter().map(|(k, v)| (*k, v.to_string())));
    let config = LlmConfig::from_vars(|k| {
        vars.iter()
            .rev()
            .find(|(name, _)| *name == k)
            .map(|(_, v)| v.clone())
    })
    .unwrap()
    .unwrap();
    OpenAiClient::new(config)
}

fn ok_body(text: &str) -> String {
    serde_json::json!({
        "model": "modello-prova-2026",
        "choices": [{"message": {"role": "assistant", "content": text}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 20, "completion_tokens": 4},
    })
    .to_string()
}

#[test]
fn a_call_sends_the_openai_format_and_reads_the_answer() {
    let (url, seen) = server(vec![(200, ok_body("Buongiorno!"), Duration::ZERO)]);
    let c = client(&url, Some("sk-segreta"), &[("LLM_JSON_MODE", "true")]);
    let request = Request::new("Sei un NPC del treno.", "Saluta.")
        .temperature(0.3)
        .max_tokens(50)
        .json();
    let answer = c.complete(&request).unwrap();
    assert_eq!(answer.text, "Buongiorno!");
    assert_eq!(answer.model, "modello-prova-2026");
    assert_eq!(answer.usage.unwrap().completion_tokens, 4);

    let seen = seen.recv().unwrap();
    assert_eq!(seen.request_line, "POST /v1/chat/completions HTTP/1.1");
    assert_eq!(seen.header("authorization"), Some("Bearer sk-segreta"));
    assert_eq!(seen.header("content-type"), Some("application/json"));
    let b = &seen.body;
    assert_eq!(b["model"], "modello-prova");
    assert_eq!(b["messages"][0]["role"], "system");
    assert_eq!(b["messages"][1]["content"], "Saluta.");
    assert_eq!(b["max_tokens"], 50);
    assert!((b["temperature"].as_f64().unwrap() - 0.3).abs() < 1e-6);
    assert_eq!(b["response_format"]["type"], "json_object");
}

#[test]
fn local_servers_need_no_key() {
    let (url, seen) = server(vec![(200, ok_body("ok"), Duration::ZERO)]);
    let c = client(&url, None, &[]);
    c.complete(&Request::new("s", "u")).unwrap();
    let seen = seen.recv().unwrap();
    assert_eq!(seen.header("authorization"), None);
    // JSON mode off: no response_format even for JSON requests.
    assert_eq!(seen.body["max_tokens"], 800);
    assert!(seen.body.get("temperature").is_none());
}

#[test]
fn error_statuses_keep_the_body_but_never_the_key() {
    let body = r#"{"error": {"message": "chiave sk-segreta non valida"}}"#.to_string();
    let (url, _seen) = server(vec![(401, body, Duration::ZERO)]);
    let c = client(&url, Some("sk-segreta"), &[]);
    let err = c.complete(&Request::new("s", "u")).unwrap_err();
    match &err {
        LlmError::Http { status, body } => {
            assert_eq!(*status, 401);
            assert!(body.contains("non valida"), "{body}");
        }
        other => panic!("{other:?}"),
    }
    assert!(!err.is_retryable());
    assert!(!err.to_string().contains("sk-segreta"), "{err}");
}

#[test]
fn overloads_are_retried() {
    let (url, seen) = server(vec![
        (503, "{}".to_string(), Duration::ZERO),
        (200, ok_body("seconda volta"), Duration::ZERO),
    ]);
    let c = client(&url, None, &[("LLM_MAX_RETRIES", "1")]);
    assert_eq!(
        c.complete(&Request::new("s", "u")).unwrap().text,
        "seconda volta"
    );
    assert_eq!(seen.try_iter().count(), 2);
}

#[test]
fn slow_servers_time_out() {
    let (url, _seen) = server(vec![(200, ok_body("tardi"), Duration::from_secs(3))]);
    let c = client(&url, None, &[]);
    assert_eq!(c.complete(&Request::new("s", "u")), Err(LlmError::Timeout));
}

#[test]
fn nobody_listening_is_a_transport_error() {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let c = client(&format!("http://127.0.0.1:{port}/v1"), None, &[]);
    let err = c.complete(&Request::new("s", "u")).unwrap_err();
    assert!(matches!(err, LlmError::Transport(_)), "{err:?}");
    assert!(err.is_retryable());
}
