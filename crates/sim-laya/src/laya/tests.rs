//! Test del modello con i pesi.
//!
//! - Parità numerica con PyTorch sulla checkpoint sintetica `testdata/tiny`
//!   (golden generati da Laya upstream, vedi `testdata/NOTICE`): girano sempre
//!   con la feature `laya`, in pochi secondi.
//! - Test sulla checkpoint vera `laya-multilingual`: `#[ignore]`, scaricano
//!   ~680 MB la prima volta. Si lanciano con
//!   `cargo test -p sim-laya --release --features metal -- --ignored --nocapture`.

use std::path::PathBuf;

use serde_json::Value;

use super::sequence::{Budget, QuestionKind, Row, softmax_with_temperature};
use super::*;
use crate::model::{ChoiceModel, ChoiceQuery};

fn testdata() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/laya/testdata")
}

fn tiny(device: DevicePreference, precision: Precision) -> LayaModel {
    LayaModel::load(LayaOptions {
        source: ModelSource::Dir(testdata().join("tiny")),
        device,
        precision,
        // come la reference (`max_len` della config tiny)
        max_len: 768,
        head_max_len: None,
        max_batch_rows: 32,
        option_labels: OptionLabels::Plain,
        temperature: None,
        warm_up: false,
    })
    .expect("carica la checkpoint tiny")
}

fn reference() -> Value {
    let text = std::fs::read_to_string(testdata().join("tiny-reference.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn case<'a>(reference: &'a Value, name: &str) -> &'a Value {
    reference["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("caso {name}"))
}

fn kind(q: u64) -> QuestionKind {
    match q {
        0 => QuestionKind::Choice,
        1 => QuestionKind::Score,
        _ => QuestionKind::Noul,
    }
}

fn reference_rows(case: &Value) -> Vec<Row> {
    case["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|it| Row {
            ids: it["ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u32)
                .collect(),
            markers: it["markers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u32)
                .collect(),
            kind: kind(it["qtype"].as_u64().unwrap()),
        })
        .collect()
}

fn floats(v: &Value) -> Vec<f32> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap() as f32)
        .collect()
}

fn max_abs_diff(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

/// Le domande dei casi, nell'ordine di `tests/reference.py::cases()` di laya-candle,
/// con le opzioni rese come `render_options` di Python. (Il JSON perde l'ordine
/// delle chiavi, quindi le ricostruiamo qui.)
fn standard_questions() -> Vec<(QuestionKind, &'static str, Vec<String>)> {
    vec![
        (
            QuestionKind::Choice,
            "Which department?",
            vec![
                "billing: refunds".into(),
                "technical: bugs".into(),
                "sales: purchases".into(),
            ],
        ),
        (
            QuestionKind::Score,
            "How urgent?",
            vec![
                "level 0: low".into(),
                "level 1: medium".into(),
                "level 2: high".into(),
            ],
        ),
        (
            QuestionKind::Noul,
            "Is a refund requested?",
            vec![
                "false: no, the statement does not hold".into(),
                "true: yes, the statement holds".into(),
            ],
        ),
    ]
}

fn mixed_questions() -> Vec<(QuestionKind, &'static str, Vec<String>)> {
    let mut twenty = vec!["billing".to_string()];
    twenty.extend((0..19).map(|i| format!("department_{i}")));
    let mut qs = vec![
        (QuestionKind::Choice, "Choose", vec!["billing".to_string()]),
        (QuestionKind::Choice, "Which department?", twenty),
    ];
    qs.extend(standard_questions());
    qs
}

#[test]
fn tiny_tokenization_matches_python() {
    let model = tiny(DevicePreference::Cpu, Precision::F32);
    let reference = reference();
    let budget = Budget {
        max_len: 768,
        head_max_len: 128,
    };
    let cases = [
        ("basic", standard_questions()),
        ("mask_literals", standard_questions()),
        ("empty_state", standard_questions()),
        ("long", standard_questions()),
        ("mixed_options", mixed_questions()),
    ];
    for (name, questions) in cases {
        let case = case(&reference, name);
        let state = case["state"].as_str().unwrap();
        let expected = reference_rows(case);
        assert_eq!(expected.len(), questions.len(), "{name}");
        for ((k, ins, opts), want) in questions.iter().zip(&expected) {
            let got = model
                .row_builder()
                .build(*k, ins, opts, state, budget)
                .unwrap();
            assert_eq!(&got, want, "{name}: {ins}");
        }
    }
}

fn check_logits(model: &LayaModel, tol: f32) -> f32 {
    let reference = reference();
    let mut worst = 0.0f32;
    for case in reference["cases"].as_array().unwrap() {
        let rows = reference_rows(case);
        let refs: Vec<&Row> = rows.iter().collect();
        let got = model.raw_logits(&refs).unwrap();
        let want = case["raw"]["logits"].as_array().unwrap();
        for (g, w) in got.iter().zip(want) {
            let w = floats(w);
            let err = max_abs_diff(g, &w);
            // Le differenze tra opzioni sono piccole (~1e-3) con pesi casuali:
            // confrontiamo anche i logit centrati, che sono ciò che decide.
            let center = |v: &[f32]| {
                let m = v.iter().sum::<f32>() / v.len() as f32;
                v.iter().map(|x| x - m).collect::<Vec<_>>()
            };
            let err_c = max_abs_diff(&center(g), &center(&w));
            worst = worst.max(err).max(err_c);
            assert!(
                err < tol && err_c < tol,
                "{}: logit {g:?} vs {w:?} (err {err}, centrati {err_c})",
                case["name"]
            );
        }
    }
    worst
}

#[test]
fn tiny_logits_match_pytorch_on_cpu() {
    let model = tiny(DevicePreference::Cpu, Precision::F32);
    let worst = check_logits(&model, 1e-4);
    eprintln!("tiny cpu f32: errore massimo sui logit {worst:e}");
}

#[test]
fn tiny_padding_does_not_change_logits() {
    let model = tiny(DevicePreference::Cpu, Precision::F32);
    let reference = reference();
    let rows = reference_rows(case(&reference, "mixed_options"));
    let refs: Vec<&Row> = rows.iter().collect();
    let batched = model.raw_logits(&refs).unwrap();
    for (row, b) in rows.iter().zip(&batched) {
        let alone = model.raw_logits(&[row]).unwrap().remove(0);
        assert!(max_abs_diff(&alone, b) < 1e-5, "{alone:?} vs {b:?}");
    }
}

#[test]
fn tiny_probabilities_match_python_predictions() {
    let model = tiny(DevicePreference::Cpu, Precision::F32);
    let reference = reference();
    let case = case(&reference, "basic");
    let rows = reference_rows(case);
    let refs: Vec<&Row> = rows.iter().collect();
    let logits = model.raw_logits(&refs).unwrap();
    let answers = &case["prediction"]["answers"];
    // choice, 3 opzioni → bucket "choice:3-5" = 1.75
    let p = softmax_with_temperature(&logits[0], model.temperature_for(QuestionKind::Choice, 3));
    for (i, label) in ["billing", "technical", "sales"].iter().enumerate() {
        let want = answers["department"]["probabilities"][label]
            .as_f64()
            .unwrap() as f32;
        assert!((p[i] - want).abs() < 2e-4, "{label}: {} vs {want}", p[i]);
    }
    // score, 3 livelli → "score:3-5" = 1.25
    let p = softmax_with_temperature(&logits[1], model.agent.temperature(QuestionKind::Score, 3));
    for i in 0..3 {
        let want = answers["urgency"]["probabilities"][i.to_string()]
            .as_f64()
            .unwrap() as f32;
        assert!((p[i] - want).abs() < 2e-4);
    }
    // noul → temperature[2] = 1.9
    let p = softmax_with_temperature(&logits[2], model.agent.temperature(QuestionKind::Noul, 2));
    let want = answers["refund"]["noul"].as_f64().unwrap() as f32;
    assert!((p[1] - want).abs() < 2e-4);
}

#[test]
fn tiny_predict_batch_matches_python_end_to_end() {
    let mut model = tiny(DevicePreference::Cpu, Precision::F32);
    let reference = reference();
    let case = case(&reference, "mixed_options");
    let mut options = vec!["billing".to_string()];
    options.extend((0..19).map(|i| format!("department_{i}")));
    let query = ChoiceQuery {
        context: "Please refund".into(),
        instructions: "Which department?".into(),
        options: options.clone(),
    };
    let single = ChoiceQuery {
        context: "Please refund".into(),
        instructions: "Choose".into(),
        options: vec!["billing".into()],
    };
    let answers = model
        .predict_batch(&[query.clone(), single, query])
        .unwrap();
    assert_eq!(answers.len(), 3);
    assert_eq!(answers[1].probabilities, [1.0]);
    assert_eq!(answers[0], answers[2]);
    let want = &case["prediction"]["answers"]["twenty"]["probabilities"];
    for (label, p) in options.iter().zip(&answers[0].probabilities) {
        let w = want[label].as_f64().unwrap() as f32;
        assert!((p - w).abs() < 2e-4, "{label}: {p} vs {w}");
    }
    let sum: f32 = answers[0].probabilities.iter().sum();
    assert!((sum - 1.0).abs() < 1e-5);
}

#[test]
fn tiny_on_metal_matches_cpu() {
    let Ok(model) = LayaModel::load(LayaOptions {
        source: ModelSource::Dir(testdata().join("tiny")),
        device: DevicePreference::Metal,
        precision: Precision::F32,
        max_len: 768,
        warm_up: false,
        ..LayaOptions::default()
    }) else {
        eprintln!("Metal non disponibile: salto");
        return;
    };
    let worst = check_logits(&model, 1e-3);
    eprintln!("tiny metal f32: errore massimo {worst:e}");
    let model = LayaModel::load(LayaOptions {
        source: ModelSource::Dir(testdata().join("tiny")),
        device: DevicePreference::Metal,
        precision: Precision::F16,
        max_len: 768,
        warm_up: false,
        ..LayaOptions::default()
    })
    .unwrap();
    let worst = check_logits(&model, 2e-2);
    eprintln!("tiny metal f16: errore massimo {worst:e}");
}

// ---------------------------------------------------------------------------
// Checkpoint vera (`laya-multilingual`)
// ---------------------------------------------------------------------------

fn real(device: DevicePreference, precision: Precision) -> LayaModel {
    let model = LayaModel::load(LayaOptions {
        device,
        precision,
        ..LayaOptions::default()
    })
    .expect("carica laya-multilingual");
    eprintln!(
        "{} su {} {} in {:.2?}",
        model.name(),
        model.device_name(),
        model.precision_name(),
        model.load_time()
    );
    model
}

fn q(context: &str, instructions: &str, options: &[&str]) -> ChoiceQuery {
    ChoiceQuery {
        context: context.into(),
        instructions: instructions.into(),
        options: options.iter().map(|s| s.to_string()).collect(),
    }
}

/// Casi facili con risposta attesa: devono passare. I tre ticket vengono dal
/// README di Laya (esempi del Router, `laya-multilingual` per es/hi), con le
/// opzioni rese come i `criteria` a chiavi semantiche di Python.
fn sanity_queries() -> Vec<(ChoiceQuery, usize)> {
    let dept = [
        "billing: invoices, payments, refunds",
        "technical: bugs, outages, system errors",
        "other: everything else",
    ];
    vec![
        (
            q(
                "Marta ha moltissima fame, è in mensa alle 13:00.",
                "Cosa fa adesso Marta?",
                &["mangiare", "dormire", "lavorare"],
            ),
            0,
        ),
        (
            q(
                "Luca è stanchissimo: sono le 2 di notte e non dorme da due giorni. È accanto alla sua cuccetta.",
                "Cosa fa adesso Luca?",
                &["mangiare", "dormire", "lavorare"],
            ),
            1,
        ),
        (
            q(
                "Marta, 34 anni, cuoca. Ore 13:10, carrozza 3. Ha molta fame e non è stanca.",
                "Cosa fa adesso Marta?",
                &[
                    "mangia alla Mensa (carrozza 2)",
                    "dorme nella cuccetta (carrozza 5)",
                ],
            ),
            0,
        ),
        (
            q(
                "Marta is very hungry.",
                "What does Marta want to do?",
                &["eat", "sleep", "work"],
            ),
            0,
        ),
        (
            q(
                "Hi, we were billed twice for March. Please refund the duplicate today or we will cancel our plan.",
                "Which department should handle this?",
                &dept,
            ),
            0,
        ),
        (
            q(
                "La aplicación se cierra cada vez que abro la configuración.",
                "Which department should handle this?",
                &dept,
            ),
            1,
        ),
        (
            q(
                "मुझसे मार्च में दो बार शुल्क लिया गया, कृपया डुप्लिकेट राशि वापस करें।",
                "Which department should handle this?",
                &dept,
            ),
            0,
        ),
    ]
}

/// Casi in cui il modello base sbaglia o esita (negazioni implicite, bias di
/// posizione): solo stampati, per documentare i limiti zero-shot.
fn hard_queries() -> Vec<ChoiceQuery> {
    vec![
        q(
            "Marta, 34 anni, cuoca. Ore 13:10, carrozza 3. Fame alta, stanchezza bassa.",
            "Cosa fa adesso Marta?",
            &[
                "mangia alla Mensa (carrozza 2)",
                "dorme nella cuccetta (carrozza 5)",
            ],
        ),
        q(
            "Marta, 34 anni, cuoca. Ore 13:10, carrozza 3. Fame alta, stanchezza bassa.",
            "Cosa fa adesso Marta?",
            &[
                "dorme nella cuccetta (carrozza 5)",
                "mangia alla Mensa (carrozza 2)",
            ],
        ),
        q(
            "Marta is starving. It is 1 pm and she is standing in the dining car.",
            "What does Marta do now?",
            &["eat lunch", "go to sleep", "repair the engine"],
        ),
        q(
            "Marta is starving. It is 1 pm and she is standing in the dining car.",
            "What does Marta do now?",
            &["go to sleep", "repair the engine", "eat lunch"],
        ),
    ]
}

#[test]
#[ignore = "scarica laya-multilingual (~680 MB)"]
fn real_special_tokens_and_rows() {
    let model = real(DevicePreference::Cpu, Precision::F32);
    let sp = model.row_builder().special();
    // mmBERT: cls_token = <bos> (2), sep_token = <eos> (1), <mask> = 4, <pad> = 0
    assert_eq!((sp.cls, sp.sep, sp.mask, sp.pad), (2, 1, 4, 0));
    let row = model.build_row(&sanity_queries()[0].0).unwrap();
    assert_eq!(row.ids[0], 2);
    assert_eq!(*row.ids.last().unwrap(), 1);
    assert_eq!(row.markers.len(), 3);
    assert!(row.markers.iter().all(|&m| row.ids[m as usize] == 4));
    eprintln!(
        "riga Marta: {} token, marker {:?}",
        row.ids.len(),
        row.markers
    );
}

fn run_sanity(model: &mut LayaModel) -> Vec<Vec<f32>> {
    let cases = sanity_queries();
    let queries: Vec<ChoiceQuery> = cases.iter().map(|(q, _)| q.clone()).collect();
    let answers = model.predict_batch(&queries).unwrap();
    let mut wrong = Vec::new();
    for ((q, expected), a) in cases.iter().zip(&answers) {
        let (best, p) = a.best().unwrap();
        eprintln!(
            "{:?} → {:?} (p={p:.3}) {:?}",
            q.context.chars().take(50).collect::<String>(),
            q.options[best],
            a.probabilities
        );
        let sum: f32 = a.probabilities.iter().sum();
        assert!((sum - 1.0).abs() < 1e-4);
        assert!(a.probabilities.iter().all(|p| p.is_finite()));
        if best != *expected {
            wrong.push(q.context.clone());
        }
    }
    let hard = model.predict_batch(&hard_queries()).unwrap();
    for (q, a) in hard_queries().iter().zip(&hard) {
        eprintln!(
            "(difficile) {:?} {:?} → {:?}",
            q.context, q.options, a.probabilities
        );
    }
    assert!(wrong.is_empty(), "risposte sbagliate: {wrong:?}");
    answers.into_iter().map(|a| a.probabilities).collect()
}

#[test]
#[ignore = "scarica laya-multilingual (~680 MB)"]
fn real_behavioral_sanity_cpu_f32() {
    let mut model = real(DevicePreference::Cpu, Precision::F32);
    run_sanity(&mut model);
}

#[test]
#[ignore = "scarica laya-multilingual (~680 MB); richiede la feature metal"]
fn real_metal_f16_agrees_with_cpu_f32() {
    let mut gpu = real(DevicePreference::Metal, Precision::F16);
    let mut cpu = real(DevicePreference::Cpu, Precision::F32);
    let a = run_sanity(&mut gpu);
    let b = run_sanity(&mut cpu);
    let worst = a
        .iter()
        .zip(&b)
        .map(|(x, y)| max_abs_diff(x, y))
        .fold(0.0, f32::max);
    eprintln!("metal f16 vs cpu f32: differenza massima di probabilità {worst:e}");
    assert!(worst < 0.02, "{worst}");
    // Stessa domanda da sola o in un lotto con padding: stesse probabilità.
    let queries: Vec<ChoiceQuery> = sanity_queries().into_iter().map(|(q, _)| q).collect();
    let alone = gpu.predict_batch(&queries[..1]).unwrap();
    assert!(max_abs_diff(&alone[0].probabilities, &a[0]) < 5e-3);
    // Righe più lunghe della finestra locale (±64 token): SDPA con maschera a
    // finestra su Metal contro l'attenzione esplicita su CPU.
    let long_ctx = "Il treno attraversa le montagne mentre i passeggeri chiacchierano. ".repeat(20)
        + "Marta ha moltissima fame ed è davanti alla mensa aperta.";
    let long = [
        q(
            &long_ctx,
            "Cosa fa adesso Marta?",
            &["mangiare", "dormire", "lavorare"],
        ),
        q(
            "Marta dorme.",
            "Cosa fa adesso Marta?",
            &["mangiare", "dormire"],
        ),
    ];
    let row = gpu.build_row(&long[0]).unwrap();
    assert!(row.ids.len() > 200, "{}", row.ids.len());
    let g = gpu.predict_batch(&long).unwrap();
    let c = cpu.predict_batch(&long).unwrap();
    for (g, c) in g.iter().zip(&c) {
        let d = max_abs_diff(&g.probabilities, &c.probabilities);
        eprintln!(
            "riga lunga: metal {:?} cpu {:?}",
            g.probabilities, c.probabilities
        );
        assert!(d < 0.02, "{d}");
    }
}
