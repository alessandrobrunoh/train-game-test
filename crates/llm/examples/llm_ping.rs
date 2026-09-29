//! Checks the LLM configured in `.env`: one plain answer through the
//! background queue, one structured (JSON) answer.
//!
//! `cargo run -p llm --example llm_ping`

use std::sync::Arc;
use std::time::Duration;

use llm::{Budget, LlmConfig, LlmQueue, OpenAiClient, Request, complete_json};
use serde::Deserialize;

const SYSTEM: &str = "Sei il Narratore di un gioco ambientato su un treno che gira \
    senza sosta intorno a un mondo ghiacciato. Rispondi in italiano.";

#[derive(Debug, Deserialize)]
struct Mestiere {
    nome: String,
    carrozza: String,
    descrizione: String,
}

fn main() {
    let config = match LlmConfig::load() {
        Ok(Some(config)) => config,
        Ok(None) => {
            eprintln!(
                "Nessun modello configurato: copia .env.example in .env e imposta \
                 LLM_API_URL, LLM_API_KEY e LLM_MODEL."
            );
            std::process::exit(2);
        }
        Err(e) => {
            eprintln!("Configurazione non valida: {e}");
            std::process::exit(2);
        }
    };
    println!("Modello: {} su {}", config.model, config.endpoint);
    let budget = Budget::per_hour(config.max_calls_per_hour);
    let timeout = config.timeout;
    let client = Arc::new(OpenAiClient::new(config));

    // 1. Through the queue, as the game does: submit, then collect.
    let mut queue = LlmQueue::new(client.clone(), 1, budget);
    let request = Request::new(SYSTEM, "Presentati in una frase.").max_tokens(120);
    if let Err(e) = queue.submit(request) {
        eprintln!("Richiesta rifiutata: {e}");
        std::process::exit(1);
    }
    match queue.wait_next(timeout * 3 + Duration::from_secs(5)) {
        Some(done) => match done.result {
            Ok(r) => {
                println!(
                    "\nRisposta ({} ms, modello {}):\n{}",
                    r.latency.as_millis(),
                    r.model,
                    r.text
                );
                if let Some(u) = r.usage {
                    println!(
                        "Token: {} in entrata, {} in uscita",
                        u.prompt_tokens, u.completion_tokens
                    );
                }
            }
            Err(e) => {
                eprintln!("Errore: {e}");
                std::process::exit(1);
            }
        },
        None => {
            eprintln!("Nessuna risposta in tempo.");
            std::process::exit(1);
        }
    }

    // 2. A structured answer, checked with serde (one retry if it isn't valid).
    let request = Request::new(
        SYSTEM,
        "Sul treno molti passeggeri si ammalano e nessuno li cura. Inventa un mestiere \
         nuovo che risolva il problema. Rispondi solo con JSON: \
         {\"nome\": string, \"carrozza\": string, \"descrizione\": string}",
    )
    .max_tokens(300);
    match complete_json::<Mestiere>(client.as_ref(), &request) {
        Ok(m) => println!(
            "\nMestiere inventato: {} ({})\n{}",
            m.nome, m.carrozza, m.descrizione
        ),
        Err(e) => {
            eprintln!("Risposta strutturata non valida: {e}");
            std::process::exit(1);
        }
    }
    println!(
        "\nChiamate ancora disponibili quest'ora: {}",
        queue.budget_left()
    );
}
