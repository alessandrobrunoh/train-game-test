//! Valutazione dei cervelli degli NPC su scenari etichettati dal `sim`.
//!
//! ```text
//! cargo run -p sim-laya --example laya_eval                       # utility, random, mock
//! cargo run -p sim-laya --release --features laya --example laya_eval   # + Laya vero (CPU)
//! cargo run -p sim-laya --release --features metal --example laya_eval  # + Laya su Metal
//! ```
//!
//! Opzioni: `--per-kind N` (scenari ovvi per tipo, 8), `--sampled N` (decisioni
//! della partita, 300), `--top-k K` (5), `--batch B` (righe per lotto, 16),
//! `--seed S` (1), `--no-laya` (salta il modello vero anche se compilato).
//!
//! Il contesto è quello italiano di `World::npc_context`: una variante inglese
//! non esiste nel `sim` (andrebbe scritta a parte), quindi non è valutata.

use std::time::Instant;

use sim_laya::MockModel;
use sim_laya::eval::{
    EvalRow, OBVIOUS_KINDS, Scenario, baseline_row, format_table, model_row, obvious_scenarios,
    sampled_scenarios,
};

struct Args {
    per_kind: usize,
    sampled: usize,
    top_k: usize,
    batch: usize,
    seed: u64,
    laya: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        per_kind: 8,
        sampled: 300,
        top_k: 5,
        batch: 16,
        seed: 1,
        laya: true,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut num = |name: &str| -> u64 {
            it.next()
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| panic!("{name} vuole un numero"))
        };
        match flag.as_str() {
            "--per-kind" => args.per_kind = num("--per-kind") as usize,
            "--sampled" => args.sampled = num("--sampled") as usize,
            "--top-k" => args.top_k = (num("--top-k") as usize).clamp(2, 5),
            "--batch" => args.batch = (num("--batch") as usize).max(1),
            "--seed" => args.seed = num("--seed"),
            "--no-laya" => args.laya = false,
            "--english" => {
                eprintln!(
                    "--english: il sim produce il contesto solo in italiano, variante non disponibile"
                )
            }
            other => panic!("opzione sconosciuta: {other}"),
        }
    }
    args
}

fn in_top_rate(scenarios: &[Scenario]) -> f32 {
    if scenarios.is_empty() {
        return 0.0;
    }
    scenarios.iter().filter(|s| s.label_in_top()).count() as f32 / scenarios.len() as f32
}

fn main() {
    let args = parse_args();
    let start = Instant::now();
    let obvious = obvious_scenarios(args.seed, args.per_kind, args.top_k);
    let (sampled, utility_rate) = sampled_scenarios(args.seed + 1, args.sampled, 7, args.top_k);
    println!(
        "Scenari: {} ovvi (a), {} dalla partita (b), top-k {} · costruiti in {:.1} s",
        obvious.len(),
        sampled.len(),
        args.top_k,
        start.elapsed().as_secs_f32()
    );
    println!(
        "Risposta giusta tra le top-{}: (a) {:.0}%, (b) {:.0}% (tetto per i modelli)\n",
        args.top_k,
        in_top_rate(&obvious) * 100.0,
        in_top_rate(&sampled) * 100.0
    );

    let mut rows: Vec<EvalRow> = vec![
        baseline_row(
            "UtilityBrain",
            &obvious,
            &sampled,
            |s| s.utility,
            Some(utility_rate),
        ),
        baseline_row(
            "UtilityBrain (altro seme)",
            &obvious,
            &sampled,
            |s| s.utility_alt,
            None,
        ),
        baseline_row("RandomBrain", &obvious, &sampled, |s| s.random, None),
        model_row(
            "MockModel",
            &mut MockModel::new(),
            &obvious,
            &sampled,
            args.batch,
        ),
    ];

    if args.laya {
        match sim_laya::loader::unavailable_reason() {
            Some(reason) => println!("Laya vero non valutato: {reason}\n"),
            None => {
                let load = Instant::now();
                match sim_laya::loader::load_real_model() {
                    Ok(mut model) => {
                        println!(
                            "Caricato {} in {:.1} s",
                            model.name(),
                            load.elapsed().as_secs_f32()
                        );
                        let name = format!("Laya ({})", model.name());
                        rows.push(model_row(
                            &name,
                            model.as_mut(),
                            &obvious,
                            &sampled,
                            args.batch,
                        ));
                    }
                    Err(e) => rows.push(EvalRow {
                        name: "Laya".into(),
                        error: Some(e),
                        ..EvalRow::default()
                    }),
                }
            }
        }
    }

    println!("{}", format_table(&rows));
    println!(
        "(a) accuratezza sugli scenari ovvi · (b) accordo con UtilityBrain sulle decisioni della partita"
    );
    println!("conf. = probabilità media della risposta scelta; istogramma in 5 fasce da 0.2\n");

    // Dettaglio per tipo di scenario ovvio.
    print!("{:<36}", "accuratezza per tipo");
    for r in &rows {
        print!(" {:>12}", short(&r.name));
    }
    println!();
    for kind in OBVIOUS_KINDS {
        print!("{kind:<36}");
        for r in &rows {
            let v = r
                .accuracy_by_kind
                .iter()
                .find(|(k, _)| *k == kind)
                .map_or("—".to_string(), |(_, v)| format!("{:.0}%", v * 100.0));
            print!(" {v:>12}");
        }
        println!();
    }
}

fn short(name: &str) -> String {
    name.chars().take(12).collect()
}
