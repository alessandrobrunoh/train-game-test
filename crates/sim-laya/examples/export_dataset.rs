//! Esporta un dataset JSONL per il fine-tuning di Laya (vedi `sim_laya::dataset`,
//! `tools/laya-finetune/README.md` e il §10 di `docs/laya-brain.md`).
//!
//! ```text
//! cargo run -p sim-laya --release --example export_dataset -- --out laya-dataset
//! cargo run -p sim-laya --release --example export_dataset -- \
//!     --out laya-dataset --seeds 10..49 --years 3 --max-rows 20000 \
//!     --kinds actions,deliberations --labels greedy
//! ```
//!
//! Opzioni:
//! - `--out DIR` (`laya-dataset`): scrive `train.jsonl`, `val.jsonl`, `test.jsonl`, `manifest.json`;
//! - `--seeds A..B` (estremi inclusi, anche `A..=B`) o `A,B,C` (`10..49`);
//! - `--years N` (3; 1 anno = 12 giorni di gioco) o `--days N`: durata registrata per seme;
//! - `--max-rows N` (20000): righe della partita al massimo per tipo, divise tra i semi
//!   (i casi ovvi si aggiungono a parte);
//! - `--kinds actions,deliberations`;
//! - `--labels greedy|sample`: etichetta = scelta più probabile dell'insegnante,
//!   o un'estrazione dalla sua distribuzione (`gold.probabilities` è sempre la distribuzione);
//! - `--top-k K` (5), `--teacher-temp T` (0.05, softmax sulle utilità);
//! - `--obvious-per-kind N` (4), `--obvious-delib-per-kind N` (4), `--obvious-repeat R` (2):
//!   casi ovvi per tipo e per seme, e copie di ciascuno nel train;
//! - `--val F` (0.1), `--test F` (0.1): frazioni di semi per validazione e test;
//! - `--delib-rate X` (3): moltiplica la frequenza delle deliberazioni;
//! - `--max-label-share F` (0.35): quota massima di una categoria di etichetta
//!   ("ozia", "viaggia", …) tra le azioni della partita di un seme; 1 = nessun limite;
//! - `--threads N`, `--allow-eval-seeds` (non saltare i semi 1–3 di `laya_eval`).

use std::io::Write as _;
use std::path::PathBuf;
use std::time::Instant;

use sim_laya::dataset::{
    Dataset, EVAL_SEEDS, ExportConfig, LabelMode, SCHEMA_VERSION, Split, json_string,
};

struct Args {
    out: PathBuf,
    cfg: ExportConfig,
    allow_eval_seeds: bool,
}

fn parse_seeds(spec: &str) -> Result<Vec<u64>, String> {
    let bad = || format!("--seeds: non capisco {spec:?} (es. 10..29 oppure 3,5,8)");
    if let Some((a, b)) = spec.split_once("..") {
        let b = b.strip_prefix('=').unwrap_or(b);
        let (a, b): (u64, u64) = (
            a.trim().parse().map_err(|_| bad())?,
            b.trim().parse().map_err(|_| bad())?,
        );
        if a > b {
            return Err(bad());
        }
        return Ok((a..=b).collect());
    }
    spec.split(',')
        .map(|s| s.trim().parse().map_err(|_| bad()))
        .collect()
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        out: PathBuf::from("laya-dataset"),
        cfg: ExportConfig::default(),
        allow_eval_seeds: false,
    };
    let days_per_year = u64::from(sim::SimParams::default().days_per_year);
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{flag} vuole un valore"));
        let num = |v: String| -> Result<f64, String> {
            v.parse()
                .map_err(|_| format!("{flag}: {v:?} non è un numero"))
        };
        let cfg = &mut args.cfg;
        match flag.as_str() {
            "--out" => args.out = PathBuf::from(value()?),
            "--seeds" => cfg.seeds = parse_seeds(&value()?)?,
            "--years" => cfg.days = (num(value()?)? * days_per_year as f64).round() as u64,
            "--days" => cfg.days = num(value()?)? as u64,
            "--max-rows" => cfg.max_rows = num(value()?)? as usize,
            "--kinds" => {
                let v = value()?;
                cfg.actions = false;
                cfg.deliberations = false;
                for k in v.split(',').map(str::trim) {
                    match k {
                        "actions" | "action" => cfg.actions = true,
                        "deliberations" | "deliberation" => cfg.deliberations = true,
                        other => return Err(format!("--kinds: tipo sconosciuto {other:?}")),
                    }
                }
            }
            "--labels" => {
                let v = value()?;
                cfg.labels = LabelMode::parse(&v)
                    .ok_or_else(|| format!("--labels: {v:?} (greedy o sample)"))?;
            }
            "--top-k" => cfg.top_k = (num(value()?)? as usize).clamp(2, 5),
            "--teacher-temp" => cfg.teacher_temperature = num(value()?)? as f32,
            "--obvious-per-kind" => cfg.obvious_per_kind = num(value()?)? as usize,
            "--obvious-delib-per-kind" => cfg.obvious_delib_per_kind = num(value()?)? as usize,
            "--obvious-repeat" => cfg.obvious_repeat = (num(value()?)? as usize).max(1),
            "--val" => cfg.val_fraction = num(value()?)? as f32,
            "--test" => cfg.test_fraction = num(value()?)? as f32,
            "--threads" => cfg.threads = (num(value()?)? as usize).max(1),
            "--delib-rate" => cfg.deliberation_rate = num(value()?)? as f32,
            "--max-label-share" => cfg.max_label_share = num(value()?)? as f32,
            "--allow-eval-seeds" => args.allow_eval_seeds = true,
            "-h" | "--help" => {
                return Err(
                    "vedi l'intestazione di crates/sim-laya/examples/export_dataset.rs".to_string(),
                );
            }
            other => return Err(format!("opzione sconosciuta: {other}")),
        }
    }
    if !(args.cfg.actions || args.cfg.deliberations) {
        return Err("--kinds: serve almeno un tipo".into());
    }
    if !args.allow_eval_seeds {
        let before = args.cfg.seeds.len();
        args.cfg.seeds.retain(|s| !EVAL_SEEDS.contains(s));
        if args.cfg.seeds.len() < before {
            eprintln!(
                "Semi {EVAL_SEEDS:?} saltati: li usa `laya_eval` (--allow-eval-seeds per tenerli)"
            );
        }
    }
    if args.cfg.seeds.is_empty() {
        return Err("nessun seme".into());
    }
    Ok(args)
}

fn manifest(args: &Args, data: &Dataset) -> String {
    let cfg = &args.cfg;
    let mut kinds = Vec::new();
    if cfg.actions {
        kinds.push("\"action\"");
    }
    if cfg.deliberations {
        kinds.push("\"deliberation\"");
    }
    let mut o = format!(
        "{{\n  \"schema\": {SCHEMA_VERSION},\n  \"generator\": \"crates/sim-laya/examples/export_dataset.rs\",\n  \"question_id\": \"{}\",\n  \"question_type\": \"choice\",\n  \"kinds\": [{}],\n  \"labels\": \"{}\",\n  \"teacher_temperature\": {},\n  \"top_k\": {},\n  \"days_per_seed\": {},\n  \"max_rows_per_kind\": {},\n  \"obvious_per_kind\": {},\n  \"obvious_delib_per_kind\": {},\n  \"obvious_repeat_train\": {},\n  \"deliberation_rate\": {},\n  \"world\": {{\"carriages\": {}, \"npcs\": {}}},\n  \"eval_seeds_excluded\": {},\n  \"duplicates_removed\": {},\n  \"skipped\": {},\n  \"max_label_share\": {},\n  \"splits\": {{",
        sim_laya::dataset::QUESTION_ID,
        kinds.join(", "),
        cfg.labels.name(),
        cfg.teacher_temperature,
        cfg.top_k,
        cfg.days,
        cfg.max_rows,
        cfg.obvious_per_kind,
        cfg.obvious_delib_per_kind,
        cfg.obvious_repeat,
        cfg.deliberation_rate,
        cfg.carriages,
        cfg.npcs,
        !args.allow_eval_seeds,
        data.stats.duplicates,
        data.stats.skipped.values().sum::<usize>(),
        cfg.max_label_share,
    );
    for (i, split) in Split::ALL.into_iter().enumerate() {
        let seeds: Vec<String> = data.seeds_in(split).iter().map(u64::to_string).collect();
        let mut file = String::new();
        json_string(&mut file, &format!("{}.jsonl", split.name()));
        o.push_str(&format!(
            "{}\n    \"{}\": {{\"file\": {file}, \"rows\": {}, \"seeds\": [{}]}}",
            if i > 0 { "," } else { "" },
            split.name(),
            data.stats.splits[split as usize].rows,
            seeds.join(", ")
        ));
    }
    o.push_str("\n  }\n}\n");
    o
}

fn write_all(args: &Args, data: &Dataset) -> std::io::Result<()> {
    std::fs::create_dir_all(&args.out)?;
    for split in Split::ALL {
        let path = args.out.join(format!("{}.jsonl", split.name()));
        let mut f = std::io::BufWriter::new(std::fs::File::create(&path)?);
        for row in data.rows_in(split) {
            f.write_all(row.to_json_line().as_bytes())?;
            f.write_all(b"\n")?;
        }
        f.flush()?;
    }
    std::fs::write(args.out.join("manifest.json"), manifest(args, data))
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("export_dataset: {e}");
            std::process::exit(2);
        }
    };
    let cfg = &args.cfg;
    println!(
        "Esporto {} semi × {} giorni ({} NPC), tipi: {}{}, etichette {}, top-{} …",
        cfg.seeds.len(),
        cfg.days,
        cfg.npcs,
        if cfg.actions { "azioni " } else { "" },
        if cfg.deliberations {
            "deliberazioni"
        } else {
            ""
        },
        cfg.labels.name(),
        cfg.top_k
    );
    let start = Instant::now();
    let data = sim_laya::dataset::export(cfg);
    let elapsed = start.elapsed().as_secs_f32();
    if let Err(e) = write_all(&args, &data) {
        eprintln!("export_dataset: scrittura in {}: {e}", args.out.display());
        std::process::exit(1);
    }
    print!("{}", data.stats.format(&data.splits));
    println!(
        "\n{} righe in {} ({:.1} s)",
        data.rows.len(),
        args.out.display(),
        elapsed
    );
}
