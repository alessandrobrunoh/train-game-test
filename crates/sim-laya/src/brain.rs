//! `LayaBrain`: cervello ibrido e asincrono sopra un [`ChoiceModel`].
//!
//! Il `sim` vuole una risposta sincrona per ogni `DecisionRequest`, mentre un
//! modello come Laya risponde in decine o centinaia di millisecondi. Quindi
//! (vedi `docs/laya-brain.md`, sezioni 5 e 8):
//!
//! - `decide` **non blocca mai**: risponde subito con la scelta del ripiego
//!   (`UtilityBrain`), e intanto accoda al modello un numero limitato di
//!   domande (`budget_per_call`, al massimo `max_in_flight` in volo);
//! - un thread dedicato raccoglie le domande in lotti (fino a `batch_rows`
//!   righe o `batch_wait`) e chiama [`ChoiceModel::predict_batch`];
//! - al modello arrivano solo le `top_k` opzioni migliori per utilità
//!   (pre-filtro), con le descrizioni in italiano; la risposta si rimappa
//!   sugli indici originali;
//! - si chiede prima per gli NPC nelle carrozze "a fuoco" (vicine al
//!   giocatore, vedi [`LayaBrain::set_focus`]) e per le decisioni in cui le
//!   prime due opzioni hanno utilità quasi pari (`margin`);
//! - una risposta arrivata si applica alla decisione successiva dello stesso
//!   NPC solo se la sua **impronta** (contesto discretizzato + opzioni in
//!   forma astratta) coincide ancora e se la confidenza è almeno
//!   `min_confidence`; la stessa impronta fa da chiave per una cache LRU
//!   condivisa tra NPC ("in questa situazione si sceglie X");
//! - facoltativo, "sta pensando": un NPC a fuoco con una domanda in volo
//!   aspetta ([`sim::THINK`], ozio breve) invece di agire sulla scelta del
//!   ripiego, per al massimo `max_think_minutes` minuti di gioco.
//!
//! **Determinismo.** Le risposte asincrone dipendono dai tempi del modello:
//! due partite con lo stesso seme divergono. Per riprodurle si registra il
//! registro delle decisioni (`record_log`, [`LogEntry`]) e lo si rigioca con
//! [`ReplayBrain`]; per test e valutazioni c'è la modalità sincrona
//! ([`LayaBrain::attach_sync`]), in cui il modello si chiama nel `decide`.
//!
//! **Salvataggi.** Lo stato persistente è solo il ripiego ([`LayaBrain::fallback`],
//! serializzabile) più la [`LayaConfig`]; cache, domande in volo e modello
//! sono stato di esecuzione e ripartono da zero.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use sim::{
    Action, ActionKind, ActionOption, Brain, CarriageId, DecisionRequest, GameTime, ItemKind, Npc,
    NpcId, THINK, UtilityBrain, World,
};

use crate::model::{ChoiceAnswer, ChoiceModel, ChoiceQuery};

/// Massimo numero di opzioni passate al modello (vedi `docs/laya-brain.md`, §5).
pub const MAX_TOP_K: usize = 5;

/// Un cervello di ripiego che sa anche dare un punteggio (senza rumore) a
/// ogni opzione: serve al pre-filtro top-k e al margine tra le prime due.
pub trait ScoredBrain: Brain {
    /// Un punteggio per opzione, nello stesso ordine (vuoto se l'NPC non c'è).
    fn option_scores(&self, world: &World, request: &DecisionRequest) -> Vec<f32>;
}

impl ScoredBrain for UtilityBrain {
    fn option_scores(&self, world: &World, request: &DecisionRequest) -> Vec<f32> {
        self.scores(world, request)
    }
}

/// Parametri di [`LayaBrain`], modificabili in corsa.
#[derive(Clone, Debug, PartialEq)]
pub struct LayaConfig {
    /// Falso: decide solo il ripiego (il modello resta caricato).
    pub enabled: bool,
    /// Opzioni passate al modello (2..=[`MAX_TOP_K`]), le migliori per utilità.
    pub top_k: usize,
    /// Domande accodate al massimo per ogni chiamata a `decide`.
    pub budget_per_call: usize,
    /// Domande in volo al massimo (la coda del worker).
    pub max_in_flight: usize,
    /// Sotto questa probabilità della risposta migliore si usa il ripiego.
    pub min_confidence: f32,
    /// Fuori dal fuoco si chiede solo se le prime due opzioni distano meno di così.
    pub margin: f32,
    /// Gli NPC a fuoco aspettano la risposta ("sta pensando") invece di agire.
    pub think: bool,
    /// Durata di un'attesa (minuti di gioco).
    pub think_minutes: u64,
    /// Oltre questa attesa totale (minuti di gioco) si agisce col ripiego.
    pub max_think_minutes: u64,
    /// Voci della cache LRU (0 = niente cache).
    pub cache_capacity: usize,
    /// Worker: righe massime per lotto (letto quando il worker parte).
    pub batch_rows: usize,
    /// Worker: attesa massima per riempire un lotto (letta quando il worker parte).
    pub batch_wait: Duration,
    /// Registra ogni decisione (per i replay, vedi [`ReplayBrain`]).
    pub record_log: bool,
}

impl Default for LayaConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            top_k: MAX_TOP_K,
            budget_per_call: 4,
            max_in_flight: 64,
            min_confidence: 0.5,
            margin: 0.15,
            think: true,
            think_minutes: 5,
            max_think_minutes: 30,
            cache_capacity: 4096,
            batch_rows: 32,
            batch_wait: Duration::from_millis(15),
            record_log: false,
        }
    }
}

/// Chi ha preso una decisione.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    /// Il cervello di ripiego.
    Utility,
    /// Una risposta del modello per questo NPC.
    Laya,
    /// Una risposta del modello per una situazione equivalente (cache).
    Cache,
    /// Nessuna scelta: l'NPC aspetta la risposta ([`sim::THINK`]).
    Think,
}

impl Source {
    pub const ALL: [Source; 4] = [Source::Utility, Source::Laya, Source::Cache, Source::Think];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        match self {
            Source::Utility => "utility",
            Source::Laya => "laya",
            Source::Cache => "cache",
            Source::Think => "pensa",
        }
    }
}

/// Una decisione registrata (vedi [`LayaConfig::record_log`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogEntry {
    pub time: GameTime,
    pub npc: NpcId,
    /// Indice scelto, o [`sim::THINK`].
    pub choice: usize,
    pub source: Source,
}

/// L'ultima decisione di un NPC, per l'ispettore.
#[derive(Clone, Debug, PartialEq)]
pub struct DecisionInfo {
    pub time: GameTime,
    pub source: Source,
    /// Indice scelto, o [`sim::THINK`].
    pub choice: usize,
    /// Cosa avrebbe scelto il ripiego.
    pub fallback: usize,
    /// Probabilità della risposta del modello (Laya e cache).
    pub confidence: Option<f32>,
    /// Solo per Laya: le opzioni viste dal modello con le probabilità, dalla
    /// più probabile.
    pub options: Vec<(String, f32)>,
}

/// Contatori di [`LayaBrain`] (dall'ultimo [`LayaBrain::reset_stats`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayaStats {
    /// Decisioni per [`Source`] (indice [`Source::index`]).
    pub by_source: [u64; 4],
    pub jobs_sent: u64,
    /// Risposte arrivate (anche quelle poi scartate).
    pub jobs_answered: u64,
    /// Domande fallite (errore del modello o risposta malformata).
    pub jobs_failed: u64,
    /// Risposte applicate alla decisione dell'NPC che le aveva chieste.
    pub applied: u64,
    pub rejected_low_confidence: u64,
    /// Risposte arrivate quando la situazione dell'NPC era cambiata.
    pub rejected_stale: u64,
    pub cache_hits: u64,
    /// Decisioni Laya o cache confrontate col ripiego, e quante coincidono.
    pub compared: u64,
    pub agreed: u64,
    pub batches: u64,
    pub latency_total: Duration,
    pub latency_max: Duration,
    /// Domande in volo adesso.
    pub queue: usize,
    /// Istogramma della probabilità della risposta migliore, 10 fasce da 0.1.
    pub confidence_hist: [u64; 10],
    pub last_error: Option<String>,
}

impl LayaStats {
    pub fn decisions(&self) -> u64 {
        self.by_source.iter().sum()
    }

    pub fn count(&self, source: Source) -> u64 {
        self.by_source[source.index()]
    }

    /// Frazione di decisioni Laya/cache uguali a quelle del ripiego.
    pub fn agreement(&self) -> Option<f32> {
        (self.compared > 0).then(|| self.agreed as f32 / self.compared as f32)
    }

    /// Latenza media dall'invio alla risposta.
    pub fn avg_latency(&self) -> Option<Duration> {
        let answered = self.jobs_answered + self.jobs_failed;
        (answered > 0).then(|| self.latency_total / answered as u32)
    }

    /// Righe medie per lotto.
    pub fn avg_batch(&self) -> Option<f32> {
        (self.batches > 0)
            .then(|| (self.jobs_answered + self.jobs_failed) as f32 / self.batches as f32)
    }
}

/// Stato del modello di [`LayaBrain`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelStatus {
    /// Nessun modello: decide solo il ripiego.
    Missing,
    Loading,
    /// Pronto, col nome del modello.
    Ready(String),
    Failed(String),
}

/// Forma astratta di un'opzione, indipendente dall'NPC: con il contesto
/// discretizzato forma l'impronta di una decisione.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OptionSig {
    Idle,
    Eat,
    Sleep,
    Work,
    Buy(ItemKind),
    Socialize(Tie),
    Travel {
        goal: Option<ActionKind>,
        home: bool,
        /// Fascia della durata del viaggio: 0 (≤ 10 min), 1 (≤ 30), 2 (oltre).
        far: u8,
    },
}

/// Legame con chi si chiacchiera, per [`OptionSig::Socialize`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tie {
    Partner,
    Family,
    Friend,
    /// Conoscente con affinità negativa.
    Cold,
    Stranger,
}

impl OptionSig {
    pub fn of(npc: &Npc, option: &ActionOption) -> OptionSig {
        match option.action {
            Action::Idle => OptionSig::Idle,
            Action::Eat(_) => OptionSig::Eat,
            Action::Sleep(_) => OptionSig::Sleep,
            Action::Work(_) => OptionSig::Work,
            Action::Buy(item) => OptionSig::Buy(item),
            Action::Socialize(other) => OptionSig::Socialize(match npc.relation(other) {
                None => Tie::Stranger,
                Some(r) if r.kind == sim::RelationKind::Partner => Tie::Partner,
                Some(r) if r.kind.is_family() => Tie::Family,
                Some(r) if r.affinity > 0.0 => Tie::Friend,
                Some(_) => Tie::Cold,
            }),
            Action::Travel { to } => OptionSig::Travel {
                goal: option.goal,
                home: to == npc.home,
                far: match option.minutes {
                    0..=10 => 0,
                    11..=30 => 1,
                    _ => 2,
                },
            },
        }
    }
}

/// Fascia di un bisogno in `0..=1` (5 fasce).
fn band(v: f32) -> u8 {
    (v.clamp(0.0, 0.999) * 5.0) as u8
}

/// Impronta di una decisione: contesto discretizzato (ora, fasce dei bisogni,
/// turno, dove si trova, cosa gli manca) più l'insieme delle opzioni in forma
/// astratta. Deterministica tra esecuzioni (hasher con chiavi fisse).
pub fn fingerprint(world: &World, npc: &Npc, sigs: &[OptionSig]) -> u64 {
    let mut h = DefaultHasher::new();
    world.clock.hour().hash(&mut h);
    world.params.is_night(world.clock.hour()).hash(&mut h);
    (
        band(npc.needs.hunger),
        band(npc.needs.energy),
        band(npc.needs.social),
    )
        .hash(&mut h);
    npc.job.map(|j| j.in_shift(world.clock)).hash(&mut h);
    (
        npc.carriage == npc.home,
        Some(npc.carriage) == npc.workplace,
    )
        .hash(&mut h);
    world.carriage(npc.carriage).map(|c| c.kind).hash(&mut h);
    (npc.wants(ItemKind::Attrezzo), npc.wants(ItemKind::Vestito)).hash(&mut h);
    // L'insieme delle opzioni, non il loro ordine.
    let mut sig_hashes: Vec<u64> = sigs
        .iter()
        .map(|s| {
            let mut h = DefaultHasher::new();
            s.hash(&mut h);
            h.finish()
        })
        .collect();
    sig_hashes.sort_unstable();
    sig_hashes.hash(&mut h);
    h.finish()
}

/// Indici delle `k` opzioni col punteggio più alto, dal migliore (a pari
/// punteggio vince l'indice più basso).
pub fn top_k(scores: &[f32], k: usize) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..scores.len()).collect();
    idx.sort_by(|&a, &b| scores[b].total_cmp(&scores[a]).then(a.cmp(&b)));
    idx.truncate(k);
    idx
}

/// La domanda per un NPC: contesto, istruzione e le descrizioni delle opzioni scelte.
pub fn build_query(world: &World, request: &DecisionRequest, options: &[usize]) -> ChoiceQuery {
    let npc = world.npc(request.npc);
    let name = npc.map_or("l'NPC", |n| n.first_name());
    ChoiceQuery {
        context: world.npc_context(request.npc).unwrap_or_default(),
        instructions: format!("Quale azione sceglie {name} adesso?"),
        options: options
            .iter()
            .map(|&j| {
                let o = &request.options[j];
                if o.description.is_empty() {
                    world.describe_option(request.npc, o)
                } else {
                    o.description.clone()
                }
            })
            .collect(),
    }
}

// --- Worker -----------------------------------------------------------------

/// Carica il modello (sul thread del worker).
pub type ModelLoader = Box<dyn FnOnce() -> Result<Box<dyn ChoiceModel>, String> + Send>;

/// Una domanda in viaggio verso il modello.
struct Job {
    meta: JobMeta,
    query: ChoiceQuery,
}

/// Quello che serve per interpretare la risposta.
struct JobMeta {
    generation: u64,
    npc: NpcId,
    key: u64,
    sigs: Vec<OptionSig>,
    sent: Instant,
}

struct Reply {
    meta: JobMeta,
    options: Vec<String>,
    result: Result<ChoiceAnswer, String>,
    done: Instant,
}

enum WorkerMsg {
    Loaded(String),
    LoadFailed(String),
    Replies(Vec<Reply>),
}

/// Canali verso e dal thread del modello. Il `Mutex` rende il cervello `Sync`
/// (serve per stare in una risorsa Bevy); si usa solo con `get_mut`, senza lock.
struct Worker {
    jobs: Sender<Job>,
    msgs: Mutex<Receiver<WorkerMsg>>,
}

fn spawn_worker(loader: ModelLoader, rows: usize, wait: Duration) -> Worker {
    let (job_tx, job_rx) = mpsc::channel::<Job>();
    let (msg_tx, msg_rx) = mpsc::channel::<WorkerMsg>();
    let spawned = thread::Builder::new()
        .name("laya-worker".into())
        .spawn(move || worker_loop(loader, job_rx, msg_tx, rows.max(1), wait));
    if let Err(e) = spawned {
        // Il canale dei messaggi è già chiuso: `poll` lo vedrà come un fallimento.
        eprintln!("laya: impossibile avviare il worker: {e}");
    }
    Worker {
        jobs: job_tx,
        msgs: Mutex::new(msg_rx),
    }
}

fn worker_loop(
    loader: ModelLoader,
    jobs: Receiver<Job>,
    out: Sender<WorkerMsg>,
    rows: usize,
    wait: Duration,
) {
    let mut model = match loader() {
        Ok(model) => {
            let _ = out.send(WorkerMsg::Loaded(model.name().to_string()));
            model
        }
        Err(e) => {
            let _ = out.send(WorkerMsg::LoadFailed(e));
            return;
        }
    };
    // Finisce quando il cervello (il mittente) viene distrutto.
    while let Ok(first) = jobs.recv() {
        let mut batch = vec![first];
        let deadline = Instant::now() + wait;
        while batch.len() < rows {
            let left = deadline.saturating_duration_since(Instant::now());
            match jobs.recv_timeout(left) {
                Ok(job) => batch.push(job),
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
            }
        }
        let replies = run_batch(model.as_mut(), batch);
        if out.send(WorkerMsg::Replies(replies)).is_err() {
            return;
        }
    }
}

/// Chiama il modello su un lotto e abbina le risposte alle domande.
fn run_batch(model: &mut dyn ChoiceModel, batch: Vec<Job>) -> Vec<Reply> {
    let (metas, queries): (Vec<JobMeta>, Vec<ChoiceQuery>) =
        batch.into_iter().map(|j| (j.meta, j.query)).unzip();
    let result = model.predict_batch(&queries);
    let done = Instant::now();
    let answers: Vec<Result<ChoiceAnswer, String>> = match result {
        Ok(answers) if answers.len() == queries.len() => answers
            .into_iter()
            .zip(&queries)
            .map(|(a, q)| {
                if a.probabilities.len() == q.options.len() {
                    Ok(a)
                } else {
                    Err(format!(
                        "{} probabilità per {} opzioni",
                        a.probabilities.len(),
                        q.options.len()
                    ))
                }
            })
            .collect(),
        Ok(answers) => {
            let e = format!("{} risposte per {} domande", answers.len(), queries.len());
            vec![Err(e); queries.len()]
        }
        Err(e) => vec![Err(e); queries.len()],
    };
    metas
        .into_iter()
        .zip(queries)
        .zip(answers)
        .map(|((meta, query), result)| Reply {
            meta,
            options: query.options,
            result,
            done,
        })
        .collect()
}

enum Backend {
    None,
    /// Il `Mutex` rende il cervello `Sync`; si usa solo con `get_mut`.
    Sync(Mutex<Box<dyn ChoiceModel>>),
    Async(Worker),
}

// --- Cache LRU ----------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct CacheEntry {
    sig: OptionSig,
    confidence: f32,
    stamp: u64,
}

/// Cache LRU deterministica: impronta → opzione scelta.
#[derive(Default)]
struct Lru {
    map: HashMap<u64, CacheEntry>,
    order: BTreeMap<u64, u64>,
    clock: u64,
}

impl Lru {
    fn get(&mut self, key: u64) -> Option<CacheEntry> {
        let entry = self.map.get_mut(&key)?;
        self.order.remove(&entry.stamp);
        self.clock += 1;
        entry.stamp = self.clock;
        self.order.insert(self.clock, key);
        Some(*entry)
    }

    fn put(&mut self, key: u64, sig: OptionSig, confidence: f32, capacity: usize) {
        if capacity == 0 {
            return;
        }
        self.clock += 1;
        let entry = CacheEntry {
            sig,
            confidence,
            stamp: self.clock,
        };
        if let Some(old) = self.map.insert(key, entry) {
            self.order.remove(&old.stamp);
        }
        self.order.insert(self.clock, key);
        while self.map.len() > capacity {
            let Some((_, oldest)) = self.order.pop_first() else {
                break;
            };
            self.map.remove(&oldest);
        }
    }

    fn len(&self) -> usize {
        self.map.len()
    }

    fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }
}

// --- LayaBrain ------------------------------------------------------------------

/// Risposta arrivata e non ancora applicata.
struct Ready {
    key: u64,
    sig: OptionSig,
    /// Posizione scelta nella top-k (due opzioni possono avere la stessa forma).
    pos: usize,
    confidence: f32,
    options: Vec<(String, f32)>,
    arrived: GameTime,
}

/// Una decisione analizzata: top-k, forme astratte e impronta.
struct Prepared {
    top: Vec<usize>,
    sigs: Vec<OptionSig>,
    key: u64,
}

/// Confidenza e opzioni con probabilità di una decisione (per [`DecisionInfo`]).
type Details = (Option<f32>, Vec<(String, f32)>);

struct Candidate {
    i: usize,
    focus: bool,
    margin: f32,
}

/// Cervello ibrido: il ripiego `F` risponde sempre, un [`ChoiceModel`] (su
/// un thread dedicato, o in linea in modalità sincrona) corregge un
/// sottoinsieme delle decisioni. Vedi la documentazione del modulo.
pub struct LayaBrain<F: ScoredBrain> {
    fallback: F,
    config: LayaConfig,
    backend: Backend,
    status: ModelStatus,
    /// Cambia a ogni reset: le risposte di generazioni precedenti si scartano.
    generation: u64,
    focus: HashSet<CarriageId>,
    in_flight: HashMap<NpcId, GameTime>,
    ready: HashMap<NpcId, Ready>,
    cache: Lru,
    decisions: HashMap<NpcId, DecisionInfo>,
    log: Vec<LogEntry>,
    stats: LayaStats,
    /// Ora del mondo all'ultimo `decide` (per datare le risposte arrivate).
    clock: GameTime,
}

impl<F: ScoredBrain> LayaBrain<F> {
    /// Senza modello: decide solo `fallback` finché non se ne collega uno.
    pub fn new(fallback: F, config: LayaConfig) -> Self {
        Self {
            fallback,
            config,
            backend: Backend::None,
            status: ModelStatus::Missing,
            generation: 0,
            focus: HashSet::new(),
            in_flight: HashMap::new(),
            ready: HashMap::new(),
            cache: Lru::default(),
            decisions: HashMap::new(),
            log: Vec::new(),
            stats: LayaStats::default(),
            clock: GameTime(0),
        }
    }

    /// Modello asincrono, su un thread dedicato.
    pub fn with_model(fallback: F, model: impl ChoiceModel, config: LayaConfig) -> Self {
        let mut brain = Self::new(fallback, config);
        brain.attach_async(Box::new(model));
        brain
    }

    /// Modello chiamato in linea dentro `decide` (deterministico se lo è il modello).
    pub fn with_sync_model(fallback: F, model: impl ChoiceModel, config: LayaConfig) -> Self {
        let mut brain = Self::new(fallback, config);
        brain.attach_sync(Box::new(model));
        brain
    }

    /// Usa `model` su un thread dedicato (sostituisce il modello precedente).
    pub fn attach_async(&mut self, model: Box<dyn ChoiceModel>) {
        self.attach_loader(Box::new(move || Ok(model)));
    }

    /// Carica il modello sul thread del worker (lo stato resta
    /// [`ModelStatus::Loading`] finché non è pronto o fallisce).
    pub fn attach_loader(&mut self, loader: ModelLoader) {
        self.detach();
        let worker = spawn_worker(loader, self.config.batch_rows, self.config.batch_wait);
        self.backend = Backend::Async(worker);
        self.status = ModelStatus::Loading;
    }

    /// Modalità sincrona: il modello si chiama dentro `decide`, niente thread.
    pub fn attach_sync(&mut self, model: Box<dyn ChoiceModel>) {
        self.detach();
        self.status = ModelStatus::Ready(model.name().to_string());
        self.backend = Backend::Sync(Mutex::new(model));
    }

    /// Scollega il modello (il worker finisce da solo dopo il lotto in corso).
    pub fn detach(&mut self) {
        self.backend = Backend::None;
        self.status = ModelStatus::Missing;
        self.reset();
    }

    /// Dimentica cache, risposte e domande in volo (le risposte ancora in
    /// arrivo verranno scartate). Da chiamare quando il mondo cambia.
    pub fn reset(&mut self) {
        self.generation += 1;
        self.in_flight.clear();
        self.ready.clear();
        self.cache.clear();
        self.decisions.clear();
        self.stats.queue = 0;
    }

    pub fn fallback(&self) -> &F {
        &self.fallback
    }

    pub fn fallback_mut(&mut self) -> &mut F {
        &mut self.fallback
    }

    pub fn into_fallback(self) -> F {
        self.fallback
    }

    /// Sostituisce il ripiego (ad esempio dopo un caricamento): tiene modello
    /// e configurazione, dimentica lo stato legato al mondo precedente.
    pub fn replace_fallback(&mut self, fallback: F) {
        self.fallback = fallback;
        self.reset();
    }

    pub fn config(&self) -> &LayaConfig {
        &self.config
    }

    pub fn config_mut(&mut self) -> &mut LayaConfig {
        &mut self.config
    }

    pub fn status(&self) -> &ModelStatus {
        &self.status
    }

    /// Vero se il modello è pronto e attivo: le decisioni passano da Laya.
    pub fn is_active(&self) -> bool {
        self.config.enabled && matches!(self.status, ModelStatus::Ready(_))
    }

    pub fn is_sync(&self) -> bool {
        matches!(self.backend, Backend::Sync(_))
    }

    /// Carrozze "a fuoco": i loro NPC hanno la precedenza e possono aspettare.
    pub fn set_focus(&mut self, carriages: impl IntoIterator<Item = CarriageId>) {
        self.focus.clear();
        self.focus.extend(carriages);
    }

    pub fn is_focus(&self, carriage: CarriageId) -> bool {
        self.focus.contains(&carriage)
    }

    pub fn stats(&self) -> &LayaStats {
        &self.stats
    }

    pub fn reset_stats(&mut self) {
        self.stats = LayaStats {
            queue: self.in_flight.len(),
            ..LayaStats::default()
        };
    }

    /// Voci nella cache.
    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }

    /// L'ultima decisione presa per `npc` mentre il modello era attivo.
    pub fn decision(&self, npc: NpcId) -> Option<&DecisionInfo> {
        self.decisions.get(&npc)
    }

    /// Vero se c'è una domanda in volo per `npc`.
    pub fn is_pending(&self, npc: NpcId) -> bool {
        self.in_flight.contains_key(&npc)
    }

    pub fn log(&self) -> &[LogEntry] {
        &self.log
    }

    pub fn take_log(&mut self) -> Vec<LogEntry> {
        std::mem::take(&mut self.log)
    }

    /// Raccoglie i messaggi del worker (stato del caricamento e risposte).
    /// Lo fa già `decide`; il gioco lo chiama anche in pausa.
    pub fn poll(&mut self) {
        let Backend::Async(worker) = &mut self.backend else {
            return;
        };
        let rx = worker.msgs.get_mut().unwrap_or_else(|e| e.into_inner());
        let mut msgs = Vec::new();
        let mut disconnected = false;
        loop {
            match rx.try_recv() {
                Ok(msg) => msgs.push(msg),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
        for msg in msgs {
            match msg {
                WorkerMsg::Loaded(name) => self.status = ModelStatus::Ready(name),
                WorkerMsg::LoadFailed(e) => self.status = ModelStatus::Failed(e),
                WorkerMsg::Replies(replies) => {
                    self.stats.batches += 1;
                    for reply in replies {
                        self.on_reply(reply, self.clock);
                    }
                }
            }
        }
        if disconnected && !matches!(self.status, ModelStatus::Failed(_)) {
            self.status = ModelStatus::Failed("il worker del modello si è fermato".into());
            self.in_flight.clear();
        }
        self.stats.queue = self.in_flight.len();
    }

    fn on_reply(&mut self, reply: Reply, now: GameTime) {
        if reply.meta.generation != self.generation {
            return;
        }
        let npc = reply.meta.npc;
        self.in_flight.remove(&npc);
        let latency = reply.done.saturating_duration_since(reply.meta.sent);
        self.stats.latency_total += latency;
        self.stats.latency_max = self.stats.latency_max.max(latency);
        let answer = match reply.result {
            Ok(answer) => answer,
            Err(e) => {
                self.stats.jobs_failed += 1;
                self.stats.last_error = Some(e);
                return;
            }
        };
        self.stats.jobs_answered += 1;
        let Some((best, p)) = answer.best() else {
            return;
        };
        let bin = ((p * 10.0) as usize).min(9);
        self.stats.confidence_hist[bin] += 1;
        if p < self.config.min_confidence || !p.is_finite() {
            self.stats.rejected_low_confidence += 1;
            return;
        }
        let sig = reply.meta.sigs[best];
        self.cache
            .put(reply.meta.key, sig, p, self.config.cache_capacity);
        let mut options: Vec<(String, f32)> = reply
            .options
            .into_iter()
            .zip(answer.probabilities)
            .collect();
        options.sort_by(|a, b| b.1.total_cmp(&a.1));
        self.ready.insert(
            npc,
            Ready {
                key: reply.meta.key,
                sig,
                pos: best,
                confidence: p,
                options,
                arrived: now,
            },
        );
    }

    /// Applica la risposta pronta per l'NPC, se è ancora valida.
    fn take_ready(&mut self, npc: NpcId, prep: &Prepared) -> Option<(usize, Ready)> {
        let ready = self.ready.remove(&npc)?;
        let pos = if prep.sigs.get(ready.pos) == Some(&ready.sig) {
            Some(ready.pos)
        } else {
            prep.sigs.iter().position(|s| *s == ready.sig)
        };
        match pos {
            Some(pos) if ready.key == prep.key => {
                self.stats.applied += 1;
                Some((prep.top[pos], ready))
            }
            _ => {
                self.stats.rejected_stale += 1;
                None
            }
        }
    }

    fn should_think(&self, focus: bool, since: GameTime, now: GameTime) -> bool {
        self.config.think
            && focus
            && matches!(self.backend, Backend::Async(_))
            && now.since(since) < self.config.max_think_minutes
    }

    fn prepare(&self, world: &World, request: &DecisionRequest) -> Option<(Prepared, f32)> {
        let npc = world.npc(request.npc)?;
        if request.options.len() < 2 {
            return None;
        }
        let scores = self.fallback.option_scores(world, request);
        if scores.len() != request.options.len() {
            return None;
        }
        let k = self.config.top_k.clamp(2, MAX_TOP_K);
        let top = top_k(&scores, k);
        let margin = scores[top[0]] - scores[top[1]];
        let sigs: Vec<OptionSig> = top
            .iter()
            .map(|&j| OptionSig::of(npc, &request.options[j]))
            .collect();
        let key = fingerprint(world, npc, &sigs);
        Some((Prepared { top, sigs, key }, margin))
    }

    fn record(&mut self, world: &World, request: &DecisionRequest, info: DecisionInfo) {
        self.stats.by_source[info.source.index()] += 1;
        if matches!(info.source, Source::Laya | Source::Cache) {
            self.stats.compared += 1;
            if info.choice == info.fallback {
                self.stats.agreed += 1;
            }
        }
        if self.config.record_log {
            self.log.push(LogEntry {
                time: world.clock,
                npc: request.npc,
                choice: info.choice,
                source: info.source,
            });
        }
        self.decisions.insert(request.npc, info);
    }

    /// Toglie le voci degli NPC morti e le risposte troppo vecchie.
    fn prune(&mut self, world: &World) {
        let limit = world.npcs.len() * 2 + 256;
        if self.decisions.len() > limit {
            self.decisions.retain(|id, _| world.npc(*id).is_some());
        }
        if self.ready.len() > limit {
            let now = world.clock;
            self.ready
                .retain(|id, r| world.npc(*id).is_some() && now.since(r.arrived) < 24 * 60);
        }
    }
}

impl<F: ScoredBrain> Brain for LayaBrain<F> {
    /// Le descrizioni servono solo per le poche domande al modello: le
    /// formatta `LayaBrain` stesso, non il `sim` per tutte le opzioni.
    fn wants_descriptions(&self) -> bool {
        self.fallback.wants_descriptions()
    }

    fn think_minutes(&self) -> u64 {
        self.config.think_minutes.max(1)
    }

    fn decide(&mut self, world: &World, requests: &[DecisionRequest]) -> Vec<usize> {
        self.clock = world.clock;
        self.poll();
        let fallback = self.fallback.decide(world, requests);
        let mut out = fallback.clone();
        let now = world.clock;
        if !self.is_active() {
            if self.config.record_log {
                self.log
                    .extend(requests.iter().zip(&fallback).map(|(r, &c)| LogEntry {
                        time: now,
                        npc: r.npc,
                        choice: c,
                        source: Source::Utility,
                    }));
            }
            self.stats.by_source[Source::Utility.index()] += requests.len() as u64;
            return out;
        }

        let mut sources = vec![Source::Utility; requests.len()];
        let mut details: Vec<Details> = vec![(None, Vec::new()); requests.len()];
        let mut prepared: Vec<Option<Prepared>> = Vec::with_capacity(requests.len());
        let mut candidates = Vec::new();

        for (i, request) in requests.iter().enumerate() {
            let Some((prep, margin)) = self.prepare(world, request) else {
                prepared.push(None);
                continue;
            };
            let focus = world
                .npc(request.npc)
                .is_some_and(|n| self.is_focus(n.carriage));
            if let Some((idx, ready)) = self.take_ready(request.npc, &prep) {
                out[i] = idx;
                sources[i] = Source::Laya;
                details[i] = (Some(ready.confidence), ready.options);
            } else if let Some(entry) = self.cache.get(prep.key)
                && let Some(pos) = prep.sigs.iter().position(|s| *s == entry.sig)
            {
                self.stats.cache_hits += 1;
                out[i] = prep.top[pos];
                sources[i] = Source::Cache;
                details[i].0 = Some(entry.confidence);
            } else if let Some(&since) = self.in_flight.get(&request.npc) {
                if self.should_think(focus, since, now) {
                    out[i] = THINK;
                    sources[i] = Source::Think;
                }
            } else if focus || margin < self.config.margin {
                candidates.push(Candidate { i, focus, margin });
            }
            prepared.push(Some(prep));
        }

        // Prima gli NPC a fuoco, poi le decisioni più incerte.
        candidates.sort_by(|a, b| {
            b.focus
                .cmp(&a.focus)
                .then(a.margin.total_cmp(&b.margin))
                .then(a.i.cmp(&b.i))
        });
        let room = self
            .config
            .max_in_flight
            .saturating_sub(self.in_flight.len());
        candidates.truncate(self.config.budget_per_call.min(room));

        if !candidates.is_empty() {
            let sent = Instant::now();
            let jobs: Vec<Job> = candidates
                .iter()
                .filter_map(|c| {
                    let prep = prepared[c.i].as_ref()?;
                    Some(Job {
                        meta: JobMeta {
                            generation: self.generation,
                            npc: requests[c.i].npc,
                            key: prep.key,
                            sigs: prep.sigs.clone(),
                            sent,
                        },
                        query: build_query(world, &requests[c.i], &prep.top),
                    })
                })
                .collect();
            self.stats.jobs_sent += jobs.len() as u64;
            match &mut self.backend {
                Backend::Sync(model) => {
                    let model = model.get_mut().unwrap_or_else(|e| e.into_inner());
                    let replies = run_batch(model.as_mut(), jobs);
                    self.stats.batches += 1;
                    for reply in replies {
                        self.on_reply(reply, now);
                    }
                    for c in &candidates {
                        let Some(prep) = prepared[c.i].as_ref() else {
                            continue;
                        };
                        if let Some((idx, ready)) = self.take_ready(requests[c.i].npc, prep) {
                            out[c.i] = idx;
                            sources[c.i] = Source::Laya;
                            details[c.i] = (Some(ready.confidence), ready.options);
                        }
                    }
                }
                Backend::Async(worker) => {
                    let mut failed = false;
                    for job in jobs {
                        let npc = job.meta.npc;
                        if worker.jobs.send(job).is_err() {
                            failed = true;
                            break;
                        }
                        self.in_flight.insert(npc, now);
                    }
                    if failed {
                        self.status =
                            ModelStatus::Failed("il worker del modello si è fermato".into());
                    }
                    for c in &candidates {
                        if self.in_flight.contains_key(&requests[c.i].npc)
                            && self.should_think(c.focus, now, now)
                        {
                            out[c.i] = THINK;
                            sources[c.i] = Source::Think;
                        }
                    }
                }
                Backend::None => {}
            }
        }

        for (i, request) in requests.iter().enumerate() {
            let (confidence, options) = std::mem::take(&mut details[i]);
            let info = DecisionInfo {
                time: now,
                source: sources[i],
                choice: out[i],
                fallback: fallback[i],
                confidence,
                options,
            };
            self.record(world, request, info);
        }
        self.stats.queue = self.in_flight.len();
        self.prune(world);
        out
    }
}

// --- Replay -----------------------------------------------------------------------

/// Rigioca un registro di decisioni ([`LayaBrain::take_log`]): con lo stesso
/// mondo di partenza riproduce la partita anche se le risposte di Laya erano
/// arrivate in tempi non deterministici.
pub struct ReplayBrain {
    choices: HashMap<(GameTime, NpcId), usize>,
    think_minutes: u64,
    /// Decisioni chieste ma assenti dal registro (l'NPC ozia).
    pub missing: u64,
}

impl ReplayBrain {
    /// `think_minutes` deve essere quello della partita registrata.
    pub fn new(log: &[LogEntry], think_minutes: u64) -> Self {
        Self {
            choices: log.iter().map(|e| ((e.time, e.npc), e.choice)).collect(),
            think_minutes,
            missing: 0,
        }
    }
}

impl Brain for ReplayBrain {
    fn wants_descriptions(&self) -> bool {
        false
    }

    fn think_minutes(&self) -> u64 {
        self.think_minutes.max(1)
    }

    fn decide(&mut self, world: &World, requests: &[DecisionRequest]) -> Vec<usize> {
        requests
            .iter()
            .map(|r| match self.choices.get(&(world.clock, r.npc)) {
                Some(&c) => c,
                None => {
                    self.missing += 1;
                    0
                }
            })
            .collect()
    }
}
