//! Benchmark di `LayaModel` su righe NPC realistiche (~80 token di contesto,
//! 5 opzioni da ~10 token).
//!
//! ```sh
//! cargo run --release -p sim-laya --features metal --example laya_bench            # Metal f16 + CPU f32
//! cargo run --release -p sim-laya --features metal --example laya_bench -- metal   # solo Metal
//! cargo run --release -p sim-laya --features laya  --example laya_bench -- cpu     # solo CPU
//! ```
//!
//! La prima volta scarica `laya-multilingual` (~680 MB) nella cache di Hugging Face.

use std::time::{Duration, Instant};

use sim_laya::laya::{DevicePreference, LayaModel, LayaOptions, Precision};
use sim_laya::{ChoiceModel, ChoiceQuery};

const NAMES: [&str; 8] = [
    "Marta", "Luca", "Giulia", "Paolo", "Sara", "Enzo", "Irene", "Bruno",
];
const JOBS: [&str; 8] = [
    "cuoca",
    "macchinista",
    "controllora",
    "meccanico",
    "medica",
    "facchino",
    "barista",
    "guardia",
];
const LEVELS: [&str; 4] = ["bassa", "media", "alta", "altissima"];

/// Una domanda NPC con contesto e opzioni variati da `i`.
fn npc_query(i: usize) -> ChoiceQuery {
    let name = NAMES[i % NAMES.len()];
    let job = JOBS[(i / 3) % JOBS.len()];
    let hour = 6 + (i * 7) % 17;
    let minute = (i * 13) % 60;
    let lvl = |k: usize| LEVELS[(i / (k + 1) + k) % LEVELS.len()];
    let context = format!(
        "{name}, {age} anni, {job}. Sono le {hour:02}:{minute:02} del giorno {day}, il treno viaggia \
         verso nord. {name} si trova nella carrozza {car}. Fame {f}, stanchezza {s}, voglia di \
         compagnia {c}, igiene {h}. Ha appena finito il turno e ha {coins} monete. \
         Vicino a lei ci sono {others} passeggeri e la mensa è {mensa}.",
        age = 20 + (i * 11) % 45,
        day = 1 + i % 30,
        car = 1 + i % 9,
        f = lvl(0),
        s = lvl(1),
        c = lvl(2),
        h = lvl(3),
        coins = (i * 37) % 200,
        others = i % 12,
        mensa = if i.is_multiple_of(2) {
            "aperta"
        } else {
            "chiusa"
        },
    );
    ChoiceQuery {
        context,
        instructions: format!("Quale azione sceglie {name} adesso?"),
        options: vec![
            format!("mangia qualcosa alla mensa della carrozza {}", 2 + i % 3),
            "dorme nella propria cuccetta fino al prossimo turno".into(),
            "chiacchiera con gli altri passeggeri nel vagone bar".into(),
            "si lava nel bagno in fondo alla carrozza".into(),
            format!("torna a lavorare come {job} nella carrozza di servizio"),
        ],
    }
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn bench(device: DevicePreference, precision: Precision) {
    let mut model = match LayaModel::load(LayaOptions {
        device,
        precision,
        ..LayaOptions::default()
    }) {
        Ok(m) => m,
        Err(e) => {
            println!("{device:?}/{precision:?}: non disponibile ({e})");
            return;
        }
    };
    let label = format!("{} {}", model.device_name(), model.precision_name());
    println!("\n== {label}");
    println!(
        "caricamento (pesi + tokenizer + warm-up): {:.2?}",
        model.load_time()
    );

    let sample: Vec<ChoiceQuery> = (0..64).map(npc_query).collect();
    let lens: Vec<usize> = sample
        .iter()
        .map(|q| model.build_row(q).unwrap().ids.len())
        .collect();
    let ctx_tokens: Vec<usize> = sample
        .iter()
        .map(|q| {
            let mut q = q.clone();
            let full = model.build_row(&q).unwrap().ids.len();
            q.context.clear();
            full - model.build_row(&q).unwrap().ids.len()
        })
        .collect();
    println!(
        "riga media {:.0} token (min {}, max {}), contesto medio {:.0} token, 5 opzioni",
        lens.iter().sum::<usize>() as f64 / lens.len() as f64,
        lens.iter().min().unwrap(),
        lens.iter().max().unwrap(),
        ctx_tokens.iter().sum::<usize>() as f64 / ctx_tokens.len() as f64,
    );

    // Latenza di una domanda singola.
    let runs = 20;
    let times: Vec<Duration> = (0..runs)
        .map(|i| {
            let q = [sample[i % sample.len()].clone()];
            let t = Instant::now();
            model.predict_batch(&q).unwrap();
            t.elapsed()
        })
        .collect();
    println!("domanda singola: mediana {:.1?} (su {runs})", median(times));

    // Throughput a lotti.
    for batch in [1usize, 8, 16, 32] {
        let iters = (64 / batch).clamp(3, 16);
        // un giro a vuoto per ogni forma nuova
        model.predict_batch(&sample[..batch]).unwrap();
        let mut times = Vec::with_capacity(iters);
        for it in 0..iters {
            let start = (it * batch) % (sample.len() - batch + 1);
            let qs = &sample[start..start + batch];
            let t = Instant::now();
            let answers = model.predict_batch(qs).unwrap();
            times.push(t.elapsed());
            assert_eq!(answers.len(), batch);
        }
        let med = median(times);
        let tokens: usize = lens[..batch].iter().sum();
        println!(
            "lotto {batch:>2}: {:>7.1?} per lotto, {:>6.1?} per domanda, {:>5.1} domande/s, {:>5.0} token/s",
            med,
            med / batch as u32,
            batch as f64 / med.as_secs_f64(),
            tokens as f64 / med.as_secs_f64()
        );
    }

    let a = model.predict_batch(&sample[..1]).unwrap();
    println!(
        "esempio: {:?}\n  → {:?}",
        sample[0].options, a[0].probabilities
    );
}

fn main() {
    let which = std::env::args().nth(1).unwrap_or_else(|| "all".into());
    if which == "all" || which == "metal" {
        bench(DevicePreference::Metal, Precision::F16);
    }
    if which == "metal-f32" {
        bench(DevicePreference::Metal, Precision::F32);
    }
    if which == "all" || which == "cpu" {
        bench(DevicePreference::Cpu, Precision::F32);
    }
}
