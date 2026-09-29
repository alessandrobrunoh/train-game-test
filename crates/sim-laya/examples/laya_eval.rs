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
//! Deliberazioni (scelte di vita rare, seconda parte): `--delib-per-kind N`
//! (casi ovvi per tipo, 6), `--delib-sampled N` (deliberazioni della
//! partita, 120), `--only-deliberations` / `--no-deliberations`. I modelli si
//! valutano senza e con la regola miscelata (`prior_weight` 0 e 0.5).
//!
//! Il contesto è quello italiano di `World::npc_context`: una variante inglese
//! non esiste nel `sim` (andrebbe scritta a parte), quindi non è valutata.

use std::time::Instant;

use sim_laya::eval::deliberations::{
    DelibRow, DelibScenario, OBVIOUS_DELIBERATIONS, delib_baseline_row, delib_model_row,
    format_delib_table, format_threshold_table, obvious_deliberations, predict_all,
    sampled_deliberations,
};
use sim_laya::eval::{
    EvalRow, OBVIOUS_KINDS, Scenario, baseline_row, format_table, model_row, obvious_scenarios,
    sampled_scenarios,
};
use sim_laya::{ChoiceModel, MockModel};

struct Args {
    per_kind: usize,
    sampled: usize,
    top_k: usize,
    batch: usize,
    seed: u64,
    laya: bool,
    actions: bool,
    deliberations: bool,
    delib_per_kind: usize,
    delib_sampled: usize,
}

fn parse_args() -> Args {
    let mut args = Args {
        per_kind: 8,
        sampled: 300,
        top_k: 5,
        batch: 16,
        seed: 1,
        laya: true,
        actions: true,
        deliberations: true,
        delib_per_kind: 6,
        delib_sampled: 120,
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
            "--delib-per-kind" => args.delib_per_kind = num("--delib-per-kind") as usize,
            "--delib-sampled" => args.delib_sampled = num("--delib-sampled") as usize,
            "--only-deliberations" => args.actions = false,
            "--no-deliberations" => args.deliberations = false,
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

/// Il modello Laya vero, se compilato e caricabile (una volta sola per le due parti).
fn load_laya(args: &Args) -> Option<Result<Box<dyn ChoiceModel>, String>> {
    if !args.laya {
        return None;
    }
    if let Some(reason) = sim_laya::loader::unavailable_reason() {
        println!("Laya vero non valutato: {reason}\n");
        return None;
    }
    let load = Instant::now();
    let model = sim_laya::loader::load_real_model();
    if let Ok(model) = &model {
        println!(
            "Caricato {} in {:.1} s\n",
            model.name(),
            load.elapsed().as_secs_f32()
        );
    }
    Some(model)
}

fn main() {
    let args = parse_args();
    let mut laya = load_laya(&args);
    if args.actions {
        actions(&args, laya.as_mut());
    }
    if args.deliberations {
        deliberations(&args, laya.as_mut());
    }
}

/// Prima parte: le azioni di tutti i giorni.
fn actions(args: &Args, laya: Option<&mut Result<Box<dyn ChoiceModel>, String>>) {
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

    match laya {
        Some(Ok(model)) => {
            let name = format!("Laya ({})", model.name());
            rows.push(model_row(
                &name,
                model.as_mut(),
                &obvious,
                &sampled,
                args.batch,
            ));
        }
        Some(Err(e)) => rows.push(EvalRow {
            name: "Laya".into(),
            error: Some(e.clone()),
            ..EvalRow::default()
        }),
        None => {}
    }

    println!("{}", format_table(&rows));
    println!(
        "(a) accuratezza sugli scenari ovvi · (b) accordo con UtilityBrain sulle decisioni della partita"
    );
    println!("conf. = probabilità media della risposta scelta; istogramma in 5 fasce da 0.2\n");

    // Dettaglio per tipo di scenario ovvio.
    print!("{:<36}", "accuratezza per tipo");
    for r in &rows {
        print!(" {:>12}", short(&r.name, 12));
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

fn short(name: &str, n: usize) -> String {
    name.chars().take(n).collect()
}

/// Seconda parte: le deliberazioni.
fn deliberations(args: &Args, laya: Option<&mut Result<Box<dyn ChoiceModel>, String>>) {
    let start = Instant::now();
    let obvious = obvious_deliberations(args.seed, args.delib_per_kind);
    let sampled = sampled_deliberations(args.seed + 1, args.delib_sampled);
    let mut per_kind = [0usize; sim::DeliberationKind::COUNT];
    for s in &sampled {
        per_kind[s.topic()] += 1;
    }
    println!(
        "\n=== Deliberazioni ===\nScenari: {} ovvi (a), {} dalla partita (b: {} coppia, {} figlio, {} furto, {} protesta) · costruiti in {:.1} s\n",
        obvious.len(),
        sampled.len(),
        per_kind[0],
        per_kind[1],
        per_kind[2],
        per_kind[3],
        start.elapsed().as_secs_f32()
    );
    let mut rows: Vec<DelibRow> = vec![
        delib_baseline_row(
            "Regola (argmax)",
            &obvious,
            &sampled,
            DelibScenario::rule_argmax,
        ),
        delib_baseline_row("Regola (estratta)", &obvious, &sampled, |s| s.rule_sampled),
        delib_baseline_row("Caso", &obvious, &sampled, |s| s.random),
    ];
    let model_rows = |name: &str, model: &mut dyn ChoiceModel, rows: &mut Vec<DelibRow>| {
        let result = predict_all(model, &obvious, args.batch).and_then(|(a, ra)| {
            predict_all(model, &sampled, args.batch).map(|(b, rb)| (a, b, ra, rb))
        });
        match result {
            Ok((a, b, ra, rb)) => {
                let n = (a.len() + b.len()) as f64;
                let rate = n / (a.len() as f64 / ra.max(1e-9) + b.len() as f64 / rb.max(1e-9));
                for w in [0.0, 0.5] {
                    let label = if w == 0.0 {
                        name.to_string()
                    } else {
                        format!("{name} + regola {w}")
                    };
                    rows.push(delib_model_row(
                        &label,
                        &obvious,
                        &sampled,
                        &a,
                        &b,
                        w,
                        Some(rate),
                    ));
                }
            }
            Err(e) => rows.push(DelibRow {
                name: name.to_string(),
                error: Some(e),
                ..DelibRow::default()
            }),
        }
    };
    model_rows("MockModel", &mut MockModel::new(), &mut rows);
    match laya {
        Some(Ok(model)) => model_rows("Laya", model.as_mut(), &mut rows),
        Some(Err(e)) => rows.push(DelibRow {
            name: "Laya".into(),
            error: Some(e.clone()),
            ..DelibRow::default()
        }),
        None => {}
    }

    println!("{}", format_delib_table(&rows));
    println!(
        "a = accuratezza sui casi ovvi · b = accordo con la scelta più probabile della regola, in totale e per tipo"
    );
    println!("conf. = probabilità media della scelta; istogramma su a+b in 5 fasce da 0.2\n");
    println!("{}", format_threshold_table(&rows));
    println!("coperte = risposte con confidenza sopra soglia (le altre restano alla regola)\n");

    print!("{:<44}", "accuratezza per caso ovvio");
    for r in &rows {
        print!(" {:>10}", short(&r.name, 10));
    }
    println!();
    for case in OBVIOUS_DELIBERATIONS {
        print!("{case:<44}");
        for r in &rows {
            let v = r
                .accuracy_by_case
                .iter()
                .find(|(k, _)| *k == case)
                .map_or("—".to_string(), |(_, v)| format!("{:.0}%", v * 100.0));
            print!(" {v:>10}");
        }
        println!();
    }
}
