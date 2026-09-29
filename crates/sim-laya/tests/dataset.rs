//! Il dataset per il fine-tuning (`sim_laya::dataset`, `examples/export_dataset.rs`):
//! ogni riga JSONL si rilegge e rispetta lo schema che `tools/laya-finetune/finetune.py`
//! si aspetta (vedi anche `tools/laya-finetune/test_finetune_data.py`).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde_json::Value;
use sim_laya::dataset::{
    DatasetRow, ExportConfig, LabelMode, QUESTION_ID, RowKind, RowSource, SCHEMA_VERSION, Split,
    export,
};

fn small_config(labels: LabelMode) -> ExportConfig {
    ExportConfig {
        seeds: vec![10, 11, 12],
        days: 1,
        max_rows: 90,
        labels,
        obvious_per_kind: 1,
        obvious_delib_per_kind: 1,
        obvious_repeat: 2,
        threads: 3,
        ..ExportConfig::default()
    }
}

fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key]
        .as_str()
        .unwrap_or_else(|| panic!("{key} non è una stringa: {v}"))
}

/// Controlla una riga JSON contro lo schema e la confronta con la riga in memoria.
fn check_line(line: &str, row: &DatasetRow) {
    let v: Value = serde_json::from_str(line).expect("JSON valido");
    let obj = v.as_object().expect("oggetto");
    let keys: BTreeSet<&str> = obj.keys().map(String::as_str).collect();
    let expected: BTreeSet<&str> = [
        "schema",
        "id",
        "kind",
        "source",
        "case",
        "seed",
        "split",
        "state",
        "questions",
        "gold",
        "option_tags",
        "label_tag",
    ]
    .into_iter()
    .collect();
    assert_eq!(keys, expected);
    assert_eq!(v["schema"], SCHEMA_VERSION);
    assert_eq!(str_of(&v, "id"), row.id);
    assert!(["action", "deliberation"].contains(&str_of(&v, "kind")));
    assert!(["sim", "obvious"].contains(&str_of(&v, "source")));
    assert!(["train", "val", "test"].contains(&str_of(&v, "split")));
    assert_eq!(v["seed"].as_u64(), Some(row.seed));
    assert_eq!(str_of(&v, "state"), row.state);
    assert!(!row.state.is_empty());

    // questions: una sola domanda `choice` con i criteri come lista.
    let questions = v["questions"].as_object().expect("questions");
    assert_eq!(questions.len(), 1);
    let q = &questions[QUESTION_ID];
    assert_eq!(str_of(q, "type"), "choice");
    assert_eq!(str_of(q, "instructions"), row.instructions);
    let criteria: Vec<&str> = q["criteria"]
        .as_array()
        .expect("criteria è una lista")
        .iter()
        .map(|c| c.as_str().expect("criterio"))
        .collect();
    assert!((2..=5).contains(&criteria.len()), "{criteria:?}");
    assert_eq!(criteria, row.options);
    let unique: HashSet<&&str> = criteria.iter().collect();
    assert_eq!(unique.len(), criteria.len(), "criteri ripetuti");

    // gold: etichetta tra i criteri, distribuzione su tutti i criteri.
    let g = &v["gold"][QUESTION_ID];
    let label = str_of(g, "label");
    let index = g["label_index"].as_u64().expect("label_index") as usize;
    assert_eq!(criteria[index], label);
    assert_eq!(index, row.label);
    let probs = g["probabilities"].as_object().expect("probabilities");
    let prob_keys: BTreeSet<&str> = probs.keys().map(String::as_str).collect();
    assert_eq!(prob_keys, criteria.iter().copied().collect());
    let sum: f64 = probs.values().map(|p| p.as_f64().expect("numero")).sum();
    assert!((sum - 1.0).abs() < 1e-3, "somma {sum}");
    assert!(
        probs
            .values()
            .all(|p| (0.0..=1.0).contains(&p.as_f64().unwrap()))
    );
    // L'etichetta ha probabilità > 0 per l'insegnante.
    assert!(probs[label].as_f64().unwrap() > 0.0, "{line}");

    let tags = v["option_tags"].as_array().expect("option_tags");
    assert_eq!(tags.len(), criteria.len());
    assert_eq!(v["label_tag"], tags[index]);
}

#[test]
fn exported_rows_parse_and_match_the_schema() {
    let cfg = small_config(LabelMode::Greedy);
    let data = export(&cfg);
    assert!(!data.rows.is_empty());

    let mut ids = HashSet::new();
    let mut split_of_seed: BTreeMap<u64, Split> = BTreeMap::new();
    let mut origins = BTreeSet::new();
    let mut positions = BTreeSet::new();
    for row in &data.rows {
        check_line(&row.to_json_line(), row);
        assert!(ids.insert(row.id.clone()), "id ripetuto {}", row.id);
        // Ogni seme sta in un solo split.
        let split = *split_of_seed.entry(row.seed).or_insert(row.split);
        assert_eq!(split, row.split, "seme {} in due split", row.seed);
        origins.insert((row.kind, row.source));
        positions.insert(row.label);
    }
    assert_eq!(
        data.splits,
        vec![(10, Split::Train), (11, Split::Val), (12, Split::Test)]
    );
    for kind in RowKind::ALL {
        for source in [RowSource::Sim, RowSource::Obvious] {
            if kind == RowKind::Deliberation && source == RowSource::Sim {
                // In un giorno di gioco le deliberazioni possono mancare.
                continue;
            }
            assert!(
                origins.contains(&(kind, source)),
                "manca {kind:?}/{source:?}"
            );
        }
    }
    // Opzioni mescolate: l'etichetta non sta sempre al primo posto.
    assert!(positions.len() >= 3, "{positions:?}");

    // I casi ovvi sono ripetuti solo nel train, con un ordine diverso.
    let copies = data
        .rows
        .iter()
        .filter(|r| r.source == RowSource::Obvious && r.id.ends_with("-r1"))
        .count();
    assert!(copies > 0);
    assert!(
        data.rows
            .iter()
            .filter(|r| r.id.ends_with("-r1"))
            .all(|r| r.split == Split::Train)
    );

    // Le statistiche contano le stesse righe.
    let counted: usize = data.stats.splits.iter().map(|s| s.rows).sum();
    assert_eq!(counted, data.rows.len());
    let text = data.stats.format(&data.splits);
    assert!(text.contains("[train]") && text.contains("posizione dell'etichetta"));

    // Deterministico: la stessa configurazione dà le stesse righe.
    let again = export(&cfg);
    assert_eq!(again.rows, data.rows);
}

#[test]
fn sampled_labels_follow_the_teacher_distribution() {
    let data = export(&ExportConfig {
        deliberations: false,
        obvious_per_kind: 0,
        ..small_config(LabelMode::Sample)
    });
    assert!(data.rows.iter().all(|r| r.kind == RowKind::Action));
    let mut not_greedy = 0;
    for row in &data.rows {
        check_line(&row.to_json_line(), row);
        let best = row
            .probabilities
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        if row.probabilities[row.label] < best {
            not_greedy += 1;
        }
    }
    // Con 90 righe e un insegnante non sempre sicuro, qualche estrazione
    // non è la scelta migliore.
    assert!(not_greedy > 0, "tutte le estrazioni sono avide");
}
