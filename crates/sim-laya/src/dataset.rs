//! Dataset per il fine-tuning di Laya, estratto dal `sim` (vedi
//! `examples/export_dataset.rs`, `tools/laya-finetune/` e il §10 di
//! `docs/laya-brain.md`).
//!
//! Una riga JSONL è un "caso" nel formato di `LocalLLaMA/typed-decisions`,
//! quello del notebook ufficiale di fine-tuning di Laya: uno `state`, un
//! dizionario `questions` (una sola domanda `choice`, id [`QUESTION_ID`]) e un
//! dizionario `gold` con la distribuzione dell'insegnante per opzione. Le
//! opzioni sono una **lista** di descrizioni (`criteria = [d0, d1, …]`): è la
//! forma che Laya rende senza lettere, come fa `LayaModel` nel gioco
//! (`OptionLabels::Plain`), quindi `gold.probabilities` e `gold.label` usano
//! le descrizioni come chiavi.
//!
//! Da dove vengono le righe, per ogni seme:
//! - **azioni della partita**: un mondo generato vive `days` giorni con
//!   `UtilityBrain`; un campione uniforme (reservoir) delle decisioni con
//!   almeno 2 opzioni diventa una domanda sulle top-k opzioni per utilità,
//!   come in `LayaBrain`. Insegnante: `softmax(utilità / T)`;
//! - **deliberazioni della partita**: tutte quelle che si aprono (fino alla
//!   quota), con la distribuzione della regola del `sim` come insegnante;
//! - **casi ovvi** di [`crate::eval`] (azioni e deliberazioni) con le risposte
//!   giuste: distribuzione uniforme sulle opzioni giuste, ripetuti
//!   `obvious_repeat` volte nel train (con un ordine delle opzioni diverso).
//!
//! Poi: duplicati tolti, split train/val/test **per seme** (nessun mondo è in
//! due split), ordine delle opzioni mescolato riga per riga con l'etichetta
//! rimappata (Laya ha un bias di posizione, e le top-k arrivano ordinate per
//! utilità: senza mescolarle la risposta sarebbe quasi sempre la prima).
//!
//! Tutto è deterministico dati i parametri, e senza dipendenze ML.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};

use sim::{Brain, DecisionRequest, Deliberation, MINUTES_PER_DAY, UtilityBrain, World};

use crate::brain::{build_query, top_k};
use crate::deliberation::{argmax, deliberation_query};
use crate::eval::deliberations::obvious_deliberations;
use crate::eval::obvious_scenarios;

/// Versione dello schema delle righe (campo `schema`).
pub const SCHEMA_VERSION: u32 = 1;

/// Id dell'unica domanda di ogni riga, in `questions` e `gold`.
pub const QUESTION_ID: &str = "decision";

/// Semi usati da `laya_eval` con le opzioni di default (`--seed 1`: scenari
/// ovvi dal mondo 1, partite dai mondi 2 e 3). Per non allenarsi sulla
/// valutazione l'esportatore li salta, se non glielo si chiede.
pub const EVAL_SEEDS: [u64; 3] = [1, 2, 3];

/// Tipo di decisione.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RowKind {
    /// Azione di tutti i giorni (`World::npc_context` + top-k opzioni).
    Action,
    /// Deliberazione (proposta di coppia, figlio, furto, protesta).
    Deliberation,
}

impl RowKind {
    pub const ALL: [RowKind; 2] = [RowKind::Action, RowKind::Deliberation];

    pub fn name(self) -> &'static str {
        match self {
            RowKind::Action => "action",
            RowKind::Deliberation => "deliberation",
        }
    }
}

/// Da dove viene una riga.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RowSource {
    /// Decisione vera di una partita, etichettata da `UtilityBrain` o dalla regola.
    Sim,
    /// Caso "ovvio" costruito a mano, con le risposte giuste.
    Obvious,
}

impl RowSource {
    pub fn name(self) -> &'static str {
        match self {
            RowSource::Sim => "sim",
            RowSource::Obvious => "obvious",
        }
    }
}

/// Parte del dataset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Split {
    Train,
    Val,
    Test,
}

impl Split {
    pub const ALL: [Split; 3] = [Split::Train, Split::Val, Split::Test];

    pub fn name(self) -> &'static str {
        match self {
            Split::Train => "train",
            Split::Val => "val",
            Split::Test => "test",
        }
    }
}

/// Come si sceglie `gold.label` dalla distribuzione dell'insegnante.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LabelMode {
    /// L'opzione più probabile (la migliore per utilità, la più probabile per la regola).
    #[default]
    Greedy,
    /// Un'estrazione dalla distribuzione, come fa il `sim` con la regola: le
    /// etichette non sono sempre la scelta avida.
    Sample,
}

impl LabelMode {
    pub fn name(self) -> &'static str {
        match self {
            LabelMode::Greedy => "greedy",
            LabelMode::Sample => "sample",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "greedy" => Some(LabelMode::Greedy),
            "sample" => Some(LabelMode::Sample),
            _ => None,
        }
    }
}

/// Parametri dell'esportazione.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportConfig {
    /// Semi dei mondi; ognuno finisce tutto in un solo split.
    pub seeds: Vec<u64>,
    /// Giorni di gioco registrati per seme (dopo un giorno di rodaggio).
    pub days: u64,
    /// Righe della partita al massimo **per tipo** (azioni, deliberazioni),
    /// divise in parti uguali tra i semi. I casi ovvi si aggiungono a parte.
    pub max_rows: usize,
    pub actions: bool,
    pub deliberations: bool,
    pub labels: LabelMode,
    /// Opzioni per domanda di azione (le migliori per utilità, come `LayaConfig::top_k`).
    pub top_k: usize,
    /// Temperatura della softmax sulle utilità (l'insegnante delle azioni).
    pub teacher_temperature: f32,
    /// Casi ovvi di azione per tipo e per seme (0 = nessuno).
    pub obvious_per_kind: usize,
    /// Casi ovvi di deliberazione per tipo e per seme (0 = nessuno).
    pub obvious_delib_per_kind: usize,
    /// Copie di ogni caso ovvio nel train (sovracampionamento, ≥ 1).
    pub obvious_repeat: usize,
    pub val_fraction: f32,
    pub test_fraction: f32,
    /// Thread di lavoro (un seme per volta ciascuno).
    pub threads: usize,
    pub carriages: usize,
    pub npcs: usize,
    /// Moltiplicatore di `SimParams::deliberation_rate` (più deliberazioni per giorno).
    pub deliberation_rate: f32,
    /// Quota massima di una categoria di etichetta ("ozia", "viaggia", …) tra
    /// le azioni della partita di un seme (1 = nessun limite). Senza, "ozia"
    /// è metà delle etichette di `UtilityBrain`.
    pub max_label_share: f32,
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            seeds: (10..=49).collect(),
            days: 3 * u64::from(sim::SimParams::default().days_per_year),
            max_rows: 20_000,
            actions: true,
            deliberations: true,
            labels: LabelMode::Greedy,
            top_k: 5,
            teacher_temperature: 0.05,
            obvious_per_kind: 4,
            obvious_delib_per_kind: 4,
            obvious_repeat: 2,
            val_fraction: 0.1,
            test_fraction: 0.1,
            threads: std::thread::available_parallelism().map_or(4, |n| n.get()),
            carriages: 20,
            npcs: 300,
            deliberation_rate: 3.0,
            max_label_share: 0.35,
        }
    }
}

/// Una riga del dataset (una domanda `choice`).
#[derive(Clone, Debug, PartialEq)]
pub struct DatasetRow {
    /// Unico nel dataset, es. `s12-act-00042` o `s12-obv-act-00003-r1`.
    pub id: String,
    pub kind: RowKind,
    pub source: RowSource,
    /// "partita", il tipo di deliberazione ("furto") o il caso ovvio.
    pub case: String,
    pub seed: u64,
    pub split: Split,
    pub state: String,
    pub instructions: String,
    /// Descrizioni delle opzioni, già mescolate.
    pub options: Vec<String>,
    /// Categoria di ogni opzione ("mangia", "Steal", …), per le statistiche.
    pub tags: Vec<String>,
    /// Distribuzione dell'insegnante, stesso ordine di `options`, somma 1.
    pub probabilities: Vec<f32>,
    /// Indice dell'etichetta in `options`.
    pub label: usize,
}

impl DatasetRow {
    /// Stima dei token della riga intera (≈ 3.5 caratteri per token con il
    /// tokenizer di mmBERT sull'italiano, più i token speciali e i `[MASK]`).
    /// `finetune.py` stampa quelli veri.
    pub fn estimated_tokens(&self) -> usize {
        let chars = self.state.chars().count()
            + "choice question: ".len()
            + self.instructions.chars().count()
            + self
                .options
                .iter()
                .map(|o| o.chars().count() + 1)
                .sum::<usize>();
        (chars as f32 / 3.5).ceil() as usize + self.options.len() + 4
    }

    /// La riga in una linea JSON (vedi il modulo per lo schema).
    pub fn to_json_line(&self) -> String {
        let mut o = String::with_capacity(self.state.len() + 512);
        let _ = write!(o, "{{\"schema\":{SCHEMA_VERSION},\"id\":");
        json_string(&mut o, &self.id);
        o.push_str(",\"kind\":");
        json_string(&mut o, self.kind.name());
        o.push_str(",\"source\":");
        json_string(&mut o, self.source.name());
        o.push_str(",\"case\":");
        json_string(&mut o, &self.case);
        let _ = write!(o, ",\"seed\":{},\"split\":", self.seed);
        json_string(&mut o, self.split.name());
        o.push_str(",\"state\":");
        json_string(&mut o, &self.state);
        o.push_str(",\"questions\":{");
        json_string(&mut o, QUESTION_ID);
        o.push_str(":{\"type\":\"choice\",\"instructions\":");
        json_string(&mut o, &self.instructions);
        o.push_str(",\"criteria\":");
        json_list(&mut o, &self.options);
        o.push_str("}},\"gold\":{");
        json_string(&mut o, QUESTION_ID);
        o.push_str(":{\"label\":");
        json_string(&mut o, &self.options[self.label]);
        let _ = write!(o, ",\"label_index\":{},\"probabilities\":{{", self.label);
        for (i, (opt, p)) in self.options.iter().zip(&self.probabilities).enumerate() {
            if i > 0 {
                o.push(',');
            }
            json_string(&mut o, opt);
            let p = if p.is_finite() {
                p.clamp(0.0, 1.0)
            } else {
                0.0
            };
            let _ = write!(o, ":{p:.5}");
        }
        o.push_str("}}},\"option_tags\":");
        json_list(&mut o, &self.tags);
        o.push_str(",\"label_tag\":");
        json_string(&mut o, &self.tags[self.label]);
        o.push('}');
        o
    }

    /// Chiave per i duplicati: tipo, stato, domanda e l'insieme delle opzioni.
    fn dedup_key(&self) -> u64 {
        let mut opts: Vec<&str> = self.options.iter().map(String::as_str).collect();
        opts.sort_unstable();
        let mut h = DefaultHasher::new();
        (self.kind, &self.state, &self.instructions, opts).hash(&mut h);
        h.finish()
    }

    /// Mescola le opzioni (con etichette e probabilità) in modo deterministico.
    fn shuffle_options(&mut self, salt: u64) {
        let mut h = DefaultHasher::new();
        (&self.id, salt).hash(&mut h);
        let mut rng = SplitMix(h.finish());
        let n = self.options.len();
        let mut perm: Vec<usize> = (0..n).collect();
        for i in (1..n).rev() {
            perm.swap(i, rng.below(i + 1));
        }
        let pick = |v: &[String]| perm.iter().map(|&j| v[j].clone()).collect::<Vec<_>>();
        self.options = pick(&self.options);
        self.tags = pick(&self.tags);
        self.probabilities = perm.iter().map(|&j| self.probabilities[j]).collect();
        self.label = perm.iter().position(|&j| j == self.label).unwrap_or(0);
    }
}

/// Aggiunge `s` come stringa JSON.
pub fn json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn json_list(out: &mut String, items: &[String]) {
    out.push('[');
    for (i, s) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        json_string(out, s);
    }
    out.push(']');
}

/// Generatore splitmix64: piccolo, deterministico, senza dipendenze.
#[derive(Clone, Debug)]
struct SplitMix(u64);

impl SplitMix {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Numero in `0..1`.
    fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Indice in `0..n` (`n` > 0).
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }
}

/// `softmax(scores / t)`; i punteggi non finiti contano come impossibili.
pub fn teacher_softmax(scores: &[f32], t: f32) -> Vec<f32> {
    let t = f64::from(if t.is_finite() && t > 0.0 { t } else { 1.0 });
    let z: Vec<f64> = scores
        .iter()
        .map(|&s| {
            if s.is_finite() {
                f64::from(s) / t
            } else {
                f64::NEG_INFINITY
            }
        })
        .collect();
    let max = z.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !max.is_finite() {
        return vec![1.0 / scores.len().max(1) as f32; scores.len()];
    }
    let e: Vec<f64> = z.iter().map(|v| (v - max).exp()).collect();
    let sum: f64 = e.iter().sum();
    e.iter().map(|v| (v / sum) as f32).collect()
}

/// Rinormalizza una distribuzione (uniforme se è tutta zero o non finita).
fn normalized(p: &[f32]) -> Vec<f32> {
    let clean: Vec<f32> = p
        .iter()
        .map(|&v| if v.is_finite() { v.max(0.0) } else { 0.0 })
        .collect();
    let sum: f32 = clean.iter().sum();
    if sum > 0.0 {
        clean.iter().map(|v| v / sum).collect()
    } else {
        vec![1.0 / p.len().max(1) as f32; p.len()]
    }
}

fn sample(p: &[f32], roll: f32) -> usize {
    let mut acc = 0.0;
    for (i, &w) in p.iter().enumerate() {
        acc += w;
        if roll < acc {
            return i;
        }
    }
    p.len().saturating_sub(1)
}

const SKIP_DUPLICATE_OPTIONS: &str = "righe con opzioni dalla stessa descrizione";
/// Non è una riga scartata: un'opzione uguale a una migliore, tolta dalla domanda.
const SKIP_SAME_OPTION: &str = "opzioni doppie tolte (la riga resta)";

/// Tiene al più `quota` righe, con al più `max_share · quota` righe per
/// categoria di etichetta, scelte a caso.
fn cap_label_share(
    mut rows: Vec<DatasetRow>,
    quota: usize,
    max_share: f32,
    rng: &mut SplitMix,
) -> Vec<DatasetRow> {
    for i in (1..rows.len()).rev() {
        rows.swap(i, rng.below(i + 1));
    }
    let cap = ((quota as f32 * max_share.max(0.0)).ceil() as usize).max(1);
    let mut per_tag: BTreeMap<String, usize> = BTreeMap::new();
    let mut out = Vec::with_capacity(quota.min(rows.len()));
    for row in rows {
        if out.len() >= quota {
            break;
        }
        let n = per_tag.entry(row.tags[row.label].clone()).or_default();
        if *n < cap {
            *n += 1;
            out.push(row);
        }
    }
    out
}

fn has_duplicates(options: &[String]) -> bool {
    let mut seen = HashSet::new();
    options.iter().any(|o| !seen.insert(o.as_str()))
}

/// Assegna i semi agli split: gli ultimi `test_fraction` al test, i
/// precedenti `val_fraction` alla validazione, il resto al train. Con 2 semi:
/// train e test; con 1: solo train. Semi ordinati e senza ripetizioni.
pub fn assign_splits(seeds: &[u64], val_fraction: f32, test_fraction: f32) -> Vec<(u64, Split)> {
    let mut seeds = seeds.to_vec();
    seeds.sort_unstable();
    seeds.dedup();
    let n = seeds.len();
    let count = |f: f32, min_n: usize| {
        if n >= min_n && f > 0.0 {
            ((n as f32 * f).round() as usize).max(1)
        } else {
            0
        }
    };
    let n_test = count(test_fraction, 2);
    let n_val = count(val_fraction, 3).min(n.saturating_sub(n_test + 1));
    let n_train = n - n_test - n_val;
    seeds
        .into_iter()
        .enumerate()
        .map(|(i, s)| {
            let split = if i < n_train {
                Split::Train
            } else if i < n_train + n_val {
                Split::Val
            } else {
                Split::Test
            };
            (s, split)
        })
        .collect()
}

// --- Partite -----------------------------------------------------------------------

/// Il cervello della partita: `UtilityBrain` decide, e intanto si tiene un
/// campione uniforme (reservoir) delle decisioni e delle deliberazioni.
struct Recorder<'a> {
    inner: UtilityBrain,
    cfg: &'a ExportConfig,
    seed: u64,
    rng: SplitMix,
    recording: bool,
    action_quota: usize,
    delib_quota: usize,
    actions_seen: u64,
    delibs_seen: u64,
    actions: Vec<DatasetRow>,
    delibs: Vec<DatasetRow>,
    skipped: BTreeMap<&'static str, usize>,
}

/// Algoritmo R: dove mettere l'`seen`-esimo elemento (contato da 1), se va tenuto.
fn reservoir_slot(rng: &mut SplitMix, seen: u64, len: usize, quota: usize) -> Option<usize> {
    if quota == 0 {
        None
    } else if len < quota {
        Some(len)
    } else {
        let j = (rng.next_u64() % seen) as usize;
        (j < quota).then_some(j)
    }
}

fn keep(rows: &mut Vec<DatasetRow>, slot: usize, row: DatasetRow) {
    if slot == rows.len() {
        rows.push(row);
    } else {
        rows[slot] = row;
    }
}

impl Recorder<'_> {
    fn label(&mut self, p: &[f32]) -> usize {
        match self.cfg.labels {
            LabelMode::Greedy => argmax(p).map_or(0, |(i, _)| i),
            LabelMode::Sample => sample(p, self.rng.unit()),
        }
    }

    fn action_row(&mut self, world: &World, r: &DecisionRequest) -> Option<DatasetRow> {
        let scores = self.inner.scores(world, r);
        // Le top-k per utilità, ma senza opzioni con la stessa descrizione: il
        // `sim` a volte offre due volte lo stesso spostamento (es. "va in
        // Dormitorio … per fare due chiacchiere", una per amico), e per il
        // modello sarebbero la stessa opzione. Si tiene la prima.
        let mut top = Vec::with_capacity(self.cfg.top_k);
        let mut seen = HashSet::new();
        for j in top_k(&scores, scores.len()) {
            if top.len() == self.cfg.top_k {
                break;
            }
            let o = &r.options[j];
            let text = if o.description.is_empty() {
                world.describe_option(r.npc, o)
            } else {
                o.description.clone()
            };
            if seen.insert(text) {
                top.push(j);
            } else {
                *self.skipped.entry(SKIP_SAME_OPTION).or_default() += 1;
            }
        }
        if top.len() < 2 {
            return None;
        }
        let query = build_query(world, r, &top);
        if query.context.is_empty() {
            *self.skipped.entry("contesto vuoto").or_default() += 1;
            return None;
        }
        if has_duplicates(&query.options) {
            *self.skipped.entry(SKIP_DUPLICATE_OPTIONS).or_default() += 1;
            return None;
        }
        let top_scores: Vec<f32> = top.iter().map(|&j| scores[j]).collect();
        let probabilities = teacher_softmax(&top_scores, self.cfg.teacher_temperature);
        let label = self.label(&probabilities);
        Some(DatasetRow {
            id: String::new(),
            kind: RowKind::Action,
            source: RowSource::Sim,
            case: "partita".into(),
            seed: self.seed,
            split: Split::Train,
            state: query.context,
            instructions: query.instructions,
            options: query.options,
            tags: top
                .iter()
                .map(|&j| r.options[j].action.kind().name().to_string())
                .collect(),
            probabilities,
            label,
        })
    }

    fn delib_row(&mut self, world: &World, d: &Deliberation) -> Option<DatasetRow> {
        let query = deliberation_query(d);
        let rule = world
            .deliberation_rule_weights(d.id)
            .filter(|r| r.len() == query.options.len());
        let Some(rule) = rule else {
            *self.skipped.entry("regola mancante").or_default() += 1;
            return None;
        };
        if has_duplicates(&query.options) {
            *self.skipped.entry(SKIP_DUPLICATE_OPTIONS).or_default() += 1;
            return None;
        }
        let probabilities = normalized(&rule);
        let label = self.label(&probabilities);
        Some(DatasetRow {
            id: String::new(),
            kind: RowKind::Deliberation,
            source: RowSource::Sim,
            case: d.kind.topic().into(),
            seed: self.seed,
            split: Split::Train,
            state: query.context,
            instructions: query.instructions,
            options: query.options,
            tags: d
                .options
                .iter()
                .map(|o| format!("{:?}", o.choice))
                .collect(),
            probabilities,
            label,
        })
    }
}

impl Brain for Recorder<'_> {
    fn decide(&mut self, world: &World, requests: &[DecisionRequest]) -> Vec<usize> {
        let choices = self.inner.decide(world, requests);
        if self.recording && self.cfg.actions {
            for r in requests.iter().filter(|r| r.options.len() >= 2) {
                self.actions_seen += 1;
                let slot = reservoir_slot(
                    &mut self.rng,
                    self.actions_seen,
                    self.actions.len(),
                    self.action_quota,
                );
                if let Some(slot) = slot
                    && let Some(row) = self.action_row(world, r)
                {
                    keep(&mut self.actions, slot, row);
                }
            }
        }
        choices
    }

    fn wants_descriptions(&self) -> bool {
        // `build_query` descrive solo le top-k opzioni, quando serve.
        false
    }

    fn deliberations_opened(&mut self, world: &World, new: &[Deliberation]) {
        if !(self.recording && self.cfg.deliberations) {
            return;
        }
        for d in new {
            self.delibs_seen += 1;
            let slot = reservoir_slot(
                &mut self.rng,
                self.delibs_seen,
                self.delibs.len(),
                self.delib_quota,
            );
            if let Some(slot) = slot
                && let Some(row) = self.delib_row(world, d)
            {
                keep(&mut self.delibs, slot, row);
            }
        }
    }
}

/// Le righe di un seme, prima di duplicati, id e mescolamento.
struct SeedRows {
    seed: u64,
    rows: Vec<DatasetRow>,
    skipped: BTreeMap<&'static str, usize>,
    actions_seen: u64,
    delibs_seen: u64,
}

fn run_seed(cfg: &ExportConfig, seed: u64) -> SeedRows {
    let n_seeds = cfg.seeds.len().max(1);
    let quota = cfg.max_rows.div_ceil(n_seeds);
    let balance = cfg.max_label_share < 1.0;
    let mut rec = Recorder {
        inner: UtilityBrain::new(seed),
        cfg,
        seed,
        rng: SplitMix(seed ^ 0x0da7_a5e7),
        recording: false,
        // Con il limite per categoria se ne tengono il doppio, poi si sfoltisce.
        action_quota: match (cfg.actions, balance) {
            (false, _) => 0,
            (true, false) => quota,
            (true, true) => quota * 2,
        },
        delib_quota: if cfg.deliberations { quota } else { 0 },
        actions_seen: 0,
        delibs_seen: 0,
        actions: Vec::new(),
        delibs: Vec::new(),
        skipped: BTreeMap::new(),
    };
    if cfg.days > 0 && quota > 0 {
        let mut world = World::generate(seed, cfg.carriages, cfg.npcs);
        world.params.deliberation_rate *= cfg.deliberation_rate;
        // Come `sampled_deliberations`: proteste più probabili e, nei semi
        // dispari, dormitori "pieni" prima (più nascite negate, più proteste).
        world.params.protest_chance_on_denial = 1.0;
        world.params.protest_chance_on_shortage = 0.5;
        if seed % 2 == 1 {
            world.params.birth_max_bed_occupancy = 0.6;
        }
        world.run(&mut rec, MINUTES_PER_DAY);
        rec.recording = true;
        for _ in 0..cfg.days {
            world.run(&mut rec, MINUTES_PER_DAY);
        }
    }
    let mut rng = SplitMix(seed ^ 0x0b71_0005);
    let mut rows = std::mem::take(&mut rec.actions);
    if balance {
        rows = cap_label_share(rows, quota, cfg.max_label_share, &mut rng);
    }
    rows.append(&mut rec.delibs);
    let mut skipped = std::mem::take(&mut rec.skipped);
    let mut pick = |good: &[usize], best: usize| match cfg.labels {
        LabelMode::Greedy => best,
        LabelMode::Sample => good[rng.below(good.len())],
    };

    if cfg.actions && cfg.obvious_per_kind > 0 {
        for s in obvious_scenarios(seed, cfg.obvious_per_kind, cfg.top_k) {
            // Posizioni (nelle top-k) delle risposte giuste, dalla migliore per utilità.
            let good: Vec<usize> = (0..s.top.len())
                .filter(|&i| s.acceptable.contains(&s.top[i]))
                .collect();
            if good.is_empty() {
                *skipped
                    .entry("risposta giusta fuori dalle top-k")
                    .or_default() += 1;
                continue;
            }
            if has_duplicates(&s.query.options) {
                *skipped.entry(SKIP_DUPLICATE_OPTIONS).or_default() += 1;
                continue;
            }
            let mut probabilities = vec![0.0; s.top.len()];
            for &i in &good {
                probabilities[i] = 1.0 / good.len() as f32;
            }
            let label = pick(&good, good[0]);
            rows.push(DatasetRow {
                id: String::new(),
                kind: RowKind::Action,
                source: RowSource::Obvious,
                case: s.kind.into(),
                seed,
                split: Split::Train,
                tags: s
                    .top
                    .iter()
                    .map(|&j| s.options[j].action.kind().name().to_string())
                    .collect(),
                state: s.query.context,
                instructions: s.query.instructions,
                options: s.query.options,
                probabilities,
                label,
            });
        }
    }
    if cfg.deliberations && cfg.obvious_delib_per_kind > 0 {
        for s in obvious_deliberations(seed, cfg.obvious_delib_per_kind) {
            if has_duplicates(&s.query.options) {
                *skipped.entry(SKIP_DUPLICATE_OPTIONS).or_default() += 1;
                continue;
            }
            let n = s.query.options.len();
            let mut probabilities = vec![0.0; n];
            for &i in &s.acceptable {
                probabilities[i] = 1.0 / s.acceptable.len() as f32;
            }
            // Tra le giuste, la più probabile per la regola.
            let best = *s
                .acceptable
                .iter()
                .max_by(|&&a, &&b| {
                    let (ra, rb) = (s.rule.get(a), s.rule.get(b));
                    ra.partial_cmp(&rb)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then(b.cmp(&a))
                })
                .expect("almeno una giusta");
            let label = pick(&s.acceptable, best);
            rows.push(DatasetRow {
                id: String::new(),
                kind: RowKind::Deliberation,
                source: RowSource::Obvious,
                case: s.case.into(),
                seed,
                split: Split::Train,
                tags: s
                    .deliberation
                    .options
                    .iter()
                    .map(|o| format!("{:?}", o.choice))
                    .collect(),
                state: s.query.context,
                instructions: s.query.instructions,
                options: s.query.options,
                probabilities,
                label,
            });
        }
    }
    SeedRows {
        seed,
        rows,
        skipped,
        actions_seen: rec.actions_seen,
        delibs_seen: rec.delibs_seen,
    }
}

// --- Esportazione ------------------------------------------------------------------

/// Il dataset esportato.
#[derive(Clone, Debug)]
pub struct Dataset {
    /// Righe in ordine di seme, già mescolate e con gli id.
    pub rows: Vec<DatasetRow>,
    pub splits: Vec<(u64, Split)>,
    pub stats: DatasetStats,
}

impl Dataset {
    pub fn rows_in(&self, split: Split) -> impl Iterator<Item = &DatasetRow> {
        self.rows.iter().filter(move |r| r.split == split)
    }

    pub fn seeds_in(&self, split: Split) -> Vec<u64> {
        self.splits
            .iter()
            .filter(|(_, s)| *s == split)
            .map(|(seed, _)| *seed)
            .collect()
    }
}

/// Esporta il dataset (bloccante: fa girare le partite, in parallelo per seme).
pub fn export(cfg: &ExportConfig) -> Dataset {
    let splits = assign_splits(&cfg.seeds, cfg.val_fraction, cfg.test_fraction);
    let seeds: Vec<u64> = splits.iter().map(|(s, _)| *s).collect();
    let threads = cfg.threads.clamp(1, seeds.len().max(1));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut per_seed: Vec<SeedRows> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(&seed) = seeds.get(i) else { break };
                        out.push(run_seed(cfg, seed));
                    }
                    out
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().expect("thread dell'esportazione"))
            .collect()
    });
    per_seed.sort_by_key(|s| s.seed);

    let mut stats = DatasetStats::default();
    let mut seen = HashSet::new();
    let mut rows = Vec::new();
    for SeedRows {
        seed,
        rows: seed_rows,
        skipped,
        actions_seen,
        delibs_seen,
    } in per_seed
    {
        let split = splits
            .iter()
            .find(|(s, _)| *s == seed)
            .map_or(Split::Train, |(_, s)| *s);
        for (reason, n) in skipped {
            *stats.skipped.entry(reason).or_default() += n;
        }
        stats.actions_seen += actions_seen;
        stats.deliberations_seen += delibs_seen;
        let mut counters: BTreeMap<(RowKind, RowSource), usize> = BTreeMap::new();
        for mut row in seed_rows {
            if !seen.insert(row.dedup_key()) {
                stats.duplicates += 1;
                continue;
            }
            let n = counters.entry((row.kind, row.source)).or_default();
            let kind = match row.kind {
                RowKind::Action => "act",
                RowKind::Deliberation => "del",
            };
            let src = match row.source {
                RowSource::Sim => "",
                RowSource::Obvious => "obv-",
            };
            row.id = format!("s{seed}-{src}{kind}-{n:05}");
            *n += 1;
            row.split = split;
            let copies = if row.source == RowSource::Obvious && split == Split::Train {
                cfg.obvious_repeat.max(1)
            } else {
                1
            };
            for copy in 0..copies {
                let mut r = row.clone();
                if copy > 0 {
                    r.id = format!("{}-r{copy}", row.id);
                }
                r.shuffle_options(copy as u64);
                rows.push(r);
            }
        }
    }
    for split in Split::ALL {
        let s = &mut stats.splits[split as usize];
        for r in rows.iter().filter(|r| r.split == split) {
            s.add(r);
        }
    }
    Dataset {
        rows,
        splits,
        stats,
    }
}

// --- Statistiche -------------------------------------------------------------------

/// Numeri di uno split.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SplitStats {
    pub rows: usize,
    pub by_origin: BTreeMap<(RowKind, RowSource), usize>,
    /// Etichette per tipo e categoria ("mangia", "Steal", …).
    pub label_tags: BTreeMap<(RowKind, String), usize>,
    /// Posizione dell'etichetta tra le opzioni (dopo il mescolamento).
    pub label_positions: BTreeMap<usize, usize>,
    /// Righe per numero di opzioni.
    pub option_counts: BTreeMap<usize, usize>,
    pub tokens_total: usize,
    pub tokens_max: usize,
    /// Somma della probabilità più alta dell'insegnante (confidenza media).
    pub teacher_confidence_total: f64,
}

impl SplitStats {
    fn add(&mut self, r: &DatasetRow) {
        self.rows += 1;
        *self.by_origin.entry((r.kind, r.source)).or_default() += 1;
        *self
            .label_tags
            .entry((r.kind, r.tags[r.label].clone()))
            .or_default() += 1;
        *self.label_positions.entry(r.label).or_default() += 1;
        *self.option_counts.entry(r.options.len()).or_default() += 1;
        let t = r.estimated_tokens();
        self.tokens_total += t;
        self.tokens_max = self.tokens_max.max(t);
        self.teacher_confidence_total +=
            f64::from(r.probabilities.iter().copied().fold(0.0, f32::max));
    }
}

/// Numeri dell'esportazione.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DatasetStats {
    /// Per split, indicizzati da `Split as usize`.
    pub splits: [SplitStats; 3],
    /// Righe tolte perché uguali a una precedente.
    pub duplicates: usize,
    /// Righe scartate, per motivo (opzioni duplicate, risposta giusta fuori dalle top-k, …).
    pub skipped: BTreeMap<&'static str, usize>,
    /// Decisioni (≥ 2 opzioni) e deliberazioni viste durante le partite.
    pub actions_seen: u64,
    pub deliberations_seen: u64,
}

impl DatasetStats {
    /// Tabella leggibile.
    pub fn format(&self, splits: &[(u64, Split)]) -> String {
        let mut o = String::new();
        let skipped: Vec<String> = self
            .skipped
            .iter()
            .map(|(reason, n)| format!("{n} {reason}"))
            .collect();
        let _ = writeln!(
            o,
            "Partite: {} decisioni viste, {} deliberazioni viste · {} duplicati tolti · scartate: {}",
            self.actions_seen,
            self.deliberations_seen,
            self.duplicates,
            if skipped.is_empty() {
                "nessuna".to_string()
            } else {
                skipped.join(", ")
            }
        );
        for split in Split::ALL {
            let s = &self.splits[split as usize];
            let seeds: Vec<String> = splits
                .iter()
                .filter(|(_, sp)| *sp == split)
                .map(|(seed, _)| seed.to_string())
                .collect();
            let _ = writeln!(
                o,
                "\n[{}] {} righe · semi: {}",
                split.name(),
                s.rows,
                if seeds.is_empty() {
                    "—".to_string()
                } else {
                    seeds.join(",")
                }
            );
            if s.rows == 0 {
                continue;
            }
            for ((kind, source), n) in &s.by_origin {
                let _ = writeln!(o, "  {:<13} {:<8} {n:>7}", kind.name(), source.name());
            }
            for kind in RowKind::ALL {
                let tags: Vec<String> = s
                    .label_tags
                    .iter()
                    .filter(|((k, _), _)| *k == kind)
                    .map(|((_, tag), n)| format!("{tag} {n}"))
                    .collect();
                if !tags.is_empty() {
                    let _ = writeln!(o, "  etichette {:<13} {}", kind.name(), tags.join(" · "));
                }
            }
            let pos: Vec<String> = s
                .label_positions
                .iter()
                .map(|(p, n)| format!("{p}: {:.0}%", 100.0 * *n as f32 / s.rows as f32))
                .collect();
            let _ = writeln!(o, "  posizione dell'etichetta   {}", pos.join(" · "));
            let k: Vec<String> = s
                .option_counts
                .iter()
                .map(|(k, n)| format!("{k}: {n}"))
                .collect();
            let _ = writeln!(o, "  opzioni per riga           {}", k.join(" · "));
            let _ = writeln!(
                o,
                "  token stimati              media {:.0}, max {}",
                s.tokens_total as f32 / s.rows as f32,
                s.tokens_max
            );
            let _ = writeln!(
                o,
                "  confidenza dell'insegnante media {:.2}",
                s.teacher_confidence_total / s.rows as f64
            );
        }
        o
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_are_by_seed_and_never_empty_train() {
        let s = assign_splits(&(10..30).collect::<Vec<_>>(), 0.1, 0.1);
        let count = |x| s.iter().filter(|(_, sp)| *sp == x).count();
        assert_eq!(
            (count(Split::Train), count(Split::Val), count(Split::Test)),
            (16, 2, 2)
        );
        assert_eq!(s.last(), Some(&(29, Split::Test)));
        let two = assign_splits(&[5, 4], 0.1, 0.1);
        assert_eq!(two, vec![(4, Split::Train), (5, Split::Test)]);
        let three = assign_splits(&[1, 2, 3], 0.1, 0.1);
        assert_eq!(
            three,
            vec![(1, Split::Train), (2, Split::Val), (3, Split::Test)]
        );
        assert_eq!(assign_splits(&[7], 0.5, 0.5), vec![(7, Split::Train)]);
    }

    #[test]
    fn shuffling_remaps_the_label_and_keeps_pairs() {
        let mut r = DatasetRow {
            id: "x".into(),
            kind: RowKind::Action,
            source: RowSource::Sim,
            case: "partita".into(),
            seed: 1,
            split: Split::Train,
            state: "s".into(),
            instructions: "q".into(),
            options: ["a", "b", "c", "d", "e"].map(String::from).to_vec(),
            tags: ["A", "B", "C", "D", "E"].map(String::from).to_vec(),
            probabilities: vec![0.6, 0.1, 0.1, 0.1, 0.1],
            label: 0,
        };
        let before = r.dedup_key();
        let positions: HashSet<usize> = (0..20)
            .map(|salt| {
                let mut c = r.clone();
                c.shuffle_options(salt);
                assert_eq!(c.options[c.label], "a");
                assert_eq!(c.tags[c.label], "A");
                assert_eq!(c.probabilities[c.label], 0.6);
                assert_eq!(c.dedup_key(), before);
                c.label
            })
            .collect();
        assert!(positions.len() >= 3, "{positions:?}");
        // Deterministico: stesso id e stesso sale, stesso ordine.
        let mut again = r.clone();
        r.shuffle_options(7);
        again.shuffle_options(7);
        assert_eq!(r, again);
    }

    #[test]
    fn teacher_softmax_is_a_distribution() {
        let p = teacher_softmax(&[1.0, 0.9, f32::NEG_INFINITY], 0.1);
        assert!((p.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert!(p[0] > p[1] && p[2] == 0.0);
        assert_eq!(teacher_softmax(&[f32::NAN, f32::NAN], 0.1), vec![0.5, 0.5]);
    }

    #[test]
    fn json_strings_are_escaped() {
        let mut s = String::new();
        json_string(&mut s, "a\"b\\c\nd\u{1}é");
        assert_eq!(s, "\"a\\\"b\\\\c\\nd\\u0001é\"");
    }
}
