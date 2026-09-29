//! Measures the Narratore: runs the sim headless and asks for one novelty per
//! game day, with the model in `.env` (or a fake one, or a recorded run).
//!
//! `cargo run -p narrator --example narrate -- [--days N] [--mock]
//!  [--replay FILE] [--record FILE] [--seed S] [--carriages C] [--npcs P]`
//!
//! Prints every proposal in Italian with its verdict, then latency (mean and
//! p95), tokens, acceptance and repeated names. The proposals are NOT applied
//! to the world: that needs the Custode (A2).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use llm::{Budget, LlmConfig, MockLlm, RecordedCall, Replay};
use narrator::guard::normalize;
use narrator::{
    Exchange, Narrator, NarratorConfig, NarratorOutcome, Requested, Verdict, WorldSummary,
};
use serde::{Deserialize, Serialize};
use sim::{MINUTES_PER_DAY, UtilityBrain, World};

/// Longest wait for one day's outcome (two calls and the client's retries).
const WAIT: Duration = Duration::from_secs(300);

/// A recorded run: enough to replay it without the network.
#[derive(Serialize, Deserialize)]
struct RunLog {
    seed: u64,
    carriages: usize,
    npcs: usize,
    days: u64,
    calls: Vec<RecordedCall>,
    exchanges: Vec<Exchange>,
}

struct Args {
    days: u64,
    mock: bool,
    replay: Option<String>,
    record: Option<String>,
    seed: u64,
    carriages: usize,
    npcs: usize,
}

fn args() -> Args {
    let mut a = Args {
        days: 5,
        mock: false,
        replay: None,
        record: None,
        seed: 42,
        carriages: 10,
        npcs: 100,
    };
    let mut raw = std::env::args().skip(1);
    while let Some(flag) = raw.next() {
        let mut value = || raw.next().unwrap_or_else(|| usage(&flag));
        match flag.as_str() {
            "--days" => a.days = value().parse().unwrap_or_else(|_| usage("--days")),
            "--seed" => a.seed = value().parse().unwrap_or_else(|_| usage("--seed")),
            "--carriages" => a.carriages = value().parse().unwrap_or_else(|_| usage("--carriages")),
            "--npcs" => a.npcs = value().parse().unwrap_or_else(|_| usage("--npcs")),
            "--replay" => a.replay = Some(value()),
            "--record" => a.record = Some(value()),
            "--mock" => a.mock = true,
            _ => usage(&flag),
        }
    }
    a
}

fn usage(flag: &str) -> ! {
    eprintln!(
        "Argomento non valido: {flag}\nUso: narrate [--days N] [--mock] [--replay FILE] [--record FILE] [--seed S] [--carriages C] [--npcs P]"
    );
    std::process::exit(2);
}

/// A fake model: valid proposals in turn, the first one a duplicate (to show
/// the retry).
fn mock() -> MockLlm {
    let calls = AtomicUsize::new(0);
    MockLlm::new(move |_| {
        let n = calls.fetch_add(1, Ordering::SeqCst);
        let novita = match n {
            0 => r#"{"tipo": "oggetto", "nome": "Coperta", "descrizione": "Una coperta.", "categoria": "durevole", "valore": 10, "pila": 5, "ingredienti": [{"oggetto": "tessuto", "qta": 2}], "lavoro": "operaio"}"#.to_string(),
            n => match n % 4 {
                1 => format!(r#"{{"tipo": "oggetto", "nome": "Sciarpa grezza {}", "descrizione": "Una sciarpa di tessuto grezzo.", "categoria": "durevole", "valore": 8, "pila": 5, "ingredienti": [{{"oggetto": "tessuto", "qta": 1}}], "lavoro": "operaio"}}"#, roman(n)),
                2 => format!(r#"{{"tipo": "evento", "titolo": "Gelata numero {}", "descrizione": "Il gelo entra dalle giunture e rovina le serre.", "effetti": [{{"effetto": "scorta", "carrozza": "Serra", "oggetto": "verdura", "delta": -10}}, {{"effetto": "bisogno", "carrozza": null, "bisogno": "energia", "delta": -0.1}}]}}"#, roman(n)),
                3 => format!(r#"{{"tipo": "lavoro", "nome": "Rammendatore {}", "descrizione": "Ripara vestiti e coperte logore.", "carrozza": "Officina", "produce": ["vestito", "coperta"]}}"#, roman(n)),
                _ => format!(r#"{{"tipo": "ricetta", "nome": "bollire le erbe {}", "prodotto": "tè", "qta": 2, "ingredienti": [{{"oggetto": "erbe", "qta": 1}}], "lavoro": "cuoco", "minuti": 30}}"#, roman(n)),
            },
        };
        Ok(format!(
            r#"{{"motivo": "Il treno ne ha bisogno oggi, lo dice lo stato delle scorte.", "novita": {novita}}}"#
        ))
    })
}

/// Distinct name suffixes for the fake model, in letters only.
fn roman(n: usize) -> &'static str {
    [
        "primo", "secondo", "terzo", "quarto", "quinto", "sesto", "settimo", "ottavo",
    ][n / 4 % 8]
}

fn run_day(world: &mut World, brain: &mut UtilityBrain) {
    let ticks = MINUTES_PER_DAY - u64::from(world.clock.minute_of_day());
    world.run(brain, ticks);
}

fn main() {
    let mut a = args();
    let replay_log: Option<RunLog> = a.replay.as_ref().map(|path| {
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
            eprintln!("Non riesco a leggere {path}: {e}");
            std::process::exit(2);
        });
        serde_json::from_str(&text).unwrap_or_else(|e| {
            eprintln!("{path} non è un registro valido: {e}");
            std::process::exit(2);
        })
    });
    let config = if let Some(log) = &replay_log {
        (a.seed, a.carriages, a.npcs, a.days) = (log.seed, log.carriages, log.npcs, log.days);
        println!(
            "Modello: registro {} ({} chiamate)",
            a.replay.as_deref().unwrap_or(""),
            log.calls.len()
        );
        NarratorConfig::new(
            Arc::new(Replay::new(log.calls.clone())),
            Budget::unlimited(),
        )
    } else if a.mock {
        println!("Modello: finto (--mock)");
        NarratorConfig::new(Arc::new(mock()), Budget::unlimited())
    } else {
        match (LlmConfig::load(), NarratorConfig::from_env()) {
            (Ok(Some(c)), Ok(Some(config))) => {
                println!(
                    "Modello: {} su {} (reasoning_effort: {})",
                    c.model,
                    c.endpoint,
                    c.reasoning_effort.as_deref().unwrap_or("non mandato")
                );
                config
            }
            (Err(e), _) | (_, Err(e)) => {
                eprintln!("Configurazione non valida: {e}");
                std::process::exit(2);
            }
            _ => {
                eprintln!("Nessun modello configurato (.env): usa --mock o --replay FILE.");
                std::process::exit(2);
            }
        }
    };
    println!(
        "Treno: {} carrozze, {} NPC, seed {}, {} giorni. Le proposte NON vengono applicate al mondo: serve il Custode (A2).\n",
        a.carriages, a.npcs, a.seed, a.days
    );

    let mut world = World::generate(a.seed, a.carriages, a.npcs);
    let mut brain = UtilityBrain::new(a.seed);
    let mut narrator = Narrator::new(config);
    let mut outcomes: Vec<NarratorOutcome> = Vec::new();
    run_day(&mut world, &mut brain);
    for i in 0..a.days {
        let day = world.clock.day();
        let summary = WorldSummary::from_world(&world);
        if i == 0 {
            println!(
                "Riassunto del mondo: {} caratteri JSON (≈{} token)",
                summary.to_json().chars().count(),
                summary.approx_tokens()
            );
        }
        let sent = narrator.request(&summary, day);
        // The world doesn't wait: the next day runs while the model thinks.
        run_day(&mut world, &mut brain);
        if sent != Requested::Sent {
            println!("── Giorno {day}: nessuna richiesta ({sent:?})\n");
            continue;
        }
        let Some(o) = narrator.wait(WAIT) else {
            println!("── Giorno {day}: nessuna risposta in {WAIT:?}\n");
            continue;
        };
        print_outcome(&o);
        outcomes.push(o);
    }

    print_stats(&narrator, &outcomes);
    if let Some(path) = &a.record {
        let log = RunLog {
            seed: a.seed,
            carriages: a.carriages,
            npcs: a.npcs,
            days: a.days,
            calls: narrator.take_recording(),
            exchanges: narrator.exchanges().to_vec(),
        };
        let json = serde_json::to_string_pretty(&log).expect("a log serializes");
        match std::fs::write(path, json) {
            Ok(()) => println!("Registro scritto in {path} ({} chiamate)", log.calls.len()),
            Err(e) => eprintln!("Non riesco a scrivere {path}: {e}"),
        }
    }
}

fn print_outcome(o: &NarratorOutcome) {
    let tries = if o.attempts > 1 {
        " al secondo tentativo"
    } else {
        ""
    };
    println!(
        "── Giorno {} ({} ms, {} token in entrata, {} in uscita)",
        o.day,
        o.latency.as_millis(),
        o.usage.prompt_tokens,
        o.usage.completion_tokens
    );
    match &o.verdict {
        Verdict::Accepted(d) => println!("ACCETTATA{tries}\n{d}\n"),
        Verdict::Rejected {
            reason,
            draft,
            answer,
        } => {
            println!("RIFIUTATA: {reason}");
            match draft {
                Some(d) => println!("{d}\n"),
                None => println!(
                    "  Risposta: {}\n",
                    answer.chars().take(400).collect::<String>()
                ),
            }
        }
        Verdict::Failed(e) => println!("ERRORE: {e}\n"),
    }
}

fn print_stats(narrator: &Narrator, outcomes: &[NarratorOutcome]) {
    let ex = narrator.exchanges();
    let mut ms: Vec<u64> = ex
        .iter()
        .filter(|e| e.answer.is_some())
        .map(|e| e.latency_ms)
        .collect();
    ms.sort_unstable();
    let mean = ms.iter().sum::<u64>() as f64 / ms.len().max(1) as f64;
    // Nearest rank.
    let p95 = ms
        .get(((ms.len() as f64 * 0.95).ceil() as usize).saturating_sub(1))
        .copied()
        .unwrap_or(0);
    let (pin, pout): (u64, u64) = ex.iter().fold((0, 0), |(i, o), e| {
        (
            i + u64::from(e.prompt_tokens),
            o + u64::from(e.completion_tokens),
        )
    });
    let n = outcomes.len().max(1) as f64;
    let accepted = outcomes
        .iter()
        .filter(|o| matches!(o.verdict, Verdict::Accepted(_)))
        .count();
    let first_try = outcomes
        .iter()
        .filter(|o| matches!(o.verdict, Verdict::Accepted(_)) && o.attempts == 1)
        .count();
    let rejected = outcomes
        .iter()
        .filter(|o| matches!(o.verdict, Verdict::Rejected { .. }))
        .count();
    let failed = outcomes.len() - accepted - rejected;
    let answered = ex.iter().filter(|e| e.answer.is_some()).count();
    let shape_errors = ex.iter().filter(|e| e.verdict.contains("JSON")).count();
    println!("══ Misure");
    println!(
        "Chiamate: {} ({} con risposta), latenza media {:.0} ms, p95 {} ms",
        ex.len(),
        answered,
        mean,
        p95
    );
    println!(
        "Token: {pin} in entrata ({:.0} per chiamata), {pout} in uscita ({:.0} per chiamata)",
        pin as f64 / answered.max(1) as f64,
        pout as f64 / answered.max(1) as f64
    );
    println!(
        "Esiti: {accepted}/{} accettate ({:.0}%; {first_try} al primo tentativo), {rejected} rifiutate, {failed} errori; risposte con JSON non valido: {shape_errors}/{answered}",
        outcomes.len(),
        100.0 * accepted as f64 / n
    );
    let mut names: BTreeMap<String, u32> = BTreeMap::new();
    for o in outcomes {
        let draft = match &o.verdict {
            Verdict::Accepted(d) => Some(d),
            Verdict::Rejected { draft, .. } => draft.as_ref(),
            Verdict::Failed(_) => None,
        };
        if let Some(d) = draft {
            *names.entry(normalize(d.proposal.name())).or_default() += 1;
        }
    }
    let repeated: Vec<String> = names
        .iter()
        .filter(|&(_, &c)| c > 1)
        .map(|(n, c)| format!("{n} ×{c}"))
        .collect();
    println!(
        "Nomi ripetuti: {}",
        if repeated.is_empty() {
            "nessuno".to_string()
        } else {
            repeated.join(", ")
        }
    );
    let kinds: BTreeMap<&str, u32> =
        narrator
            .novelties()
            .iter()
            .fold(BTreeMap::new(), |mut m, n| {
                *m.entry(n.kind.as_str()).or_default() += 1;
                m
            });
    println!("Novità accettate per tipo: {kinds:?}");
    let accepted: Vec<&narrator::Draft> = outcomes
        .iter()
        .filter_map(|o| match &o.verdict {
            Verdict::Accepted(d) => Some(d),
            _ => None,
        })
        .collect();
    let panels = accepted.iter().filter(|d| d.panel.is_some()).count();
    let elements: usize = accepted
        .iter()
        .filter_map(|d| d.panel.as_ref())
        .map(|p| p.elements.len())
        .sum();
    let items = accepted
        .iter()
        .filter(|d| matches!(d.proposal, narrator::Proposal::NewItem { .. }))
        .count();
    let looks = accepted
        .iter()
        .filter(|d| {
            matches!(
                d.proposal,
                narrator::Proposal::NewItem {
                    appearance: Some(_),
                    ..
                }
            )
        })
        .count();
    println!(
        "Pannelli: {panels}/{} novità accettate ({elements} elementi); aspetto: {looks}/{items} oggetti",
        accepted.len()
    );
    let rejected_first: Vec<&str> = ex
        .iter()
        .filter(|e| e.attempt == 1 && e.verdict.starts_with("rifiutata"))
        .map(|e| e.verdict.as_str())
        .collect();
    let truncated = ex
        .iter()
        .filter(|e| e.verdict.contains("truncated"))
        .count();
    let reasoning: Vec<u32> = ex
        .iter()
        .filter(|e| e.answer.is_some())
        .map(|e| e.reasoning_chars)
        .collect();
    println!(
        "Risposte troncate: {truncated}/{} chiamate; ragionamento medio {:.0} caratteri",
        ex.len(),
        reasoning.iter().sum::<u32>() as f64 / reasoning.len().max(1) as f64
    );
    println!("Rifiuti al primo tentativo: {}", rejected_first.len());
    for r in rejected_first {
        println!("  - {}", r.chars().take(200).collect::<String>());
    }
    println!("Nessuna proposta è stata applicata al mondo (serve il Custode, A2).");
}
