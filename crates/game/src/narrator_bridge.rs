//! Il Narratore nel gioco (A3 di "Direzione nuova" in
//! `docs/piano-vita-ed-economia.md`).
//!
//! [`NarratorState`] avvolge un `narrator::Narrator` costruito da `.env`
//! (spento se `LLM_API_URL` manca: il gioco va avanti come prima). Ogni
//! giorno di gioco, alla prima ora dopo le [`REQUEST_HOUR`], chiede una
//! novità con il riassunto del mondo; la risposta arriva in background e si
//! raccoglie ogni frame con `poll`, senza mai bloccare. Anche a velocità 5
//! (due giorni al secondo) si chiede al più una volta per giorno di gioco; un
//! giorno in cui la richiesta precedente è ancora in volo si salta
//! (`Requested::Busy`), e il budget orario di `.env` resta il tetto.
//!
//! Tiene:
//! - la **cronaca**: ogni esito con giorno, verdetto, bozza e motivo;
//! - il `Known` aggiornato con le bozze accettate (come se il Custode le
//!   avesse applicate: le prossime proposte possono citarle);
//! - il registro delle **statistiche derivate** ([`StatsRegistry`]), con lo
//!   storico campionato una volta per ora di gioco (al massimo
//!   [`HISTORY_CAP`] campioni ciascuna).
//!
//! **Niente viene applicato al mondo**: tutto qui legge `Sim` e non lo
//! modifica. Le proposte aspettano il Custode (A2).
//!
//! Cronaca e storico finiscono nella parte "gioco" del salvataggio, come JSON
//! ([`NarratorState::save_json`], vedi `save_file.rs`).

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

use bevy::prelude::*;
use llm::LlmConfig;
use narrator::{
    Catalog, Draft, Known, Narrator, NarratorConfig, NarratorOutcome, Proposal, Requested,
    StatBook, Verdict, WorldSummary,
};
use serde::{Deserialize, Serialize};
use sim::{GameTime, MINUTES_PER_HOUR, World};

use crate::sim_bridge::SimTickSet;
use crate::state::Sim;

/// Ora del giorno da cui si chiede la novità del giorno.
pub const REQUEST_HOUR: u32 = 6;
/// Campioni tenuti per statistica (10 giorni di ore).
pub const HISTORY_CAP: usize = 240;
/// Secondi reali per cui resta visibile l'avviso di una novità.
pub const TOAST_SECS: f64 = 10.0;
/// Minuti di gioco dopo una richiesta fallita prima dell'unico nuovo
/// tentativo dello stesso giorno.
pub const RETRY_AFTER_MINUTES: u64 = 120;

// --- Cronaca -----------------------------------------------------------------------

/// Come è finita la richiesta di un giorno.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryStatus {
    /// Passata il controllo: aspetta il Custode.
    #[serde(rename = "proposta")]
    Proposed,
    /// Rifiutata dal controllo anche al secondo tentativo.
    #[serde(rename = "rifiutata")]
    Rejected,
    /// Nessuna risposta usabile (rete, budget…).
    #[serde(rename = "errore")]
    Failed,
}

/// Una voce della cronaca del treno.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChronicleEntry {
    pub day: u64,
    pub status: EntryStatus,
    /// La bozza (anche rifiutata, se il JSON era valido).
    #[serde(default)]
    pub draft: Option<Draft>,
    /// Perché è stata rifiutata o è fallita.
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub attempts: u8,
    #[serde(default)]
    pub latency_ms: u64,
}

impl ChronicleEntry {
    pub fn from_outcome(o: &NarratorOutcome) -> Self {
        let (status, draft, reason) = match &o.verdict {
            Verdict::Accepted(d) => (EntryStatus::Proposed, Some(d.clone()), None),
            Verdict::Rejected { reason, draft, .. } => {
                (EntryStatus::Rejected, draft.clone(), Some(reason.clone()))
            }
            Verdict::Failed(e) => (EntryStatus::Failed, None, Some(e.clone())),
        };
        Self {
            day: o.day,
            status,
            draft,
            reason,
            attempts: o.attempts,
            latency_ms: o.latency.as_millis() as u64,
        }
    }

    /// Il nome della novità, o cosa è andato storto.
    pub fn title(&self) -> String {
        match (&self.draft, self.status) {
            (Some(d), _) => d.proposal.name().to_string(),
            (None, EntryStatus::Failed) => "Nessuna novità".to_string(),
            (None, _) => "Risposta non valida".to_string(),
        }
    }

    pub fn accepted(&self) -> Option<&Draft> {
        (self.status == EntryStatus::Proposed)
            .then_some(self.draft.as_ref())
            .flatten()
    }
}

/// Avviso "Novità sul treno: «Nome»".
#[derive(Clone, Debug, PartialEq)]
pub struct NoveltyToast {
    pub text: String,
    /// `Time<Real>` in secondi.
    pub shown_at: f64,
}

// --- Statistiche derivate ------------------------------------------------------------

/// Le statistiche accettate e il loro storico (ora di gioco, valore).
#[derive(Clone, Debug, Default)]
pub struct StatsRegistry {
    pub book: StatBook,
    history: BTreeMap<String, VecDeque<(u64, f32)>>,
    /// Ultima ora di gioco campionata (minuti / 60).
    last_hour: Option<u64>,
}

impl StatsRegistry {
    pub fn add(&mut self, stat: narrator::Statistic) {
        self.book.add(stat);
    }

    /// Campiona tutte le statistiche se è cominciata un'altra ora di gioco;
    /// restituisce vero se ha campionato. Chi non si può leggere (in attesa
    /// del Custode) non ha il campione.
    pub fn sample(&mut self, world: &World) -> bool {
        let hour = world.clock.minutes() / MINUTES_PER_HOUR;
        if self.last_hour == Some(hour) || self.book.is_empty() {
            return false;
        }
        self.last_hour = Some(hour);
        for stat in self.book.iter() {
            let Some(v) = self.book.read(&stat.name, world).value() else {
                continue;
            };
            let h = self.history.entry(key(&stat.name)).or_default();
            h.push_back((hour, v));
            while h.len() > HISTORY_CAP {
                h.pop_front();
            }
        }
        true
    }

    /// Lo storico di una statistica, dal più vecchio.
    pub fn history(&self, name: &str) -> impl Iterator<Item = (u64, f32)> + '_ {
        self.history.get(&key(name)).into_iter().flatten().copied()
    }
}

fn key(name: &str) -> String {
    narrator::guard::normalize(name)
}

// --- Salvataggio -------------------------------------------------------------------------

/// Cosa del Narratore va nel salvataggio (JSON dentro il corpo postcard: i
/// campi nuovi si aggiungono con `#[serde(default)]` senza cambiare formato).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NarratorSave {
    #[serde(default)]
    pub chronicle: Vec<ChronicleEntry>,
    /// Chiave della statistica → (ora di gioco, valore).
    #[serde(default)]
    pub history: BTreeMap<String, Vec<(u64, f32)>>,
    #[serde(default)]
    pub paused: bool,
}

// --- Stato --------------------------------------------------------------------------------

/// Il Narratore del gioco e ciò che ha raccontato finora.
#[derive(Resource)]
pub struct NarratorState {
    /// `None`: spento (niente `.env` o configurazione non valida). Nel
    /// `Mutex` perché la coda del client non è `Sync` (una risorsa Bevy deve
    /// esserlo); con `&mut self` si usa senza bloccare (`get_mut`).
    narrator: Option<Mutex<Narrator>>,
    /// Il nome del modello di `.env`, o perché è spento.
    pub model: String,
    /// La configurazione c'è ma non è valida.
    pub config_error: Option<String>,
    /// Il giocatore ha messo in pausa il Narratore: niente richieste.
    pub paused: bool,
    /// Dalla più vecchia.
    pub chronicle: Vec<ChronicleEntry>,
    /// Il catalogo più le novità accettate.
    pub known: Known,
    pub stats: StatsRegistry,
    /// Ultimo giorno di gioco per cui si è chiesto (o saltato).
    pub last_asked: Option<u64>,
    /// Com'è andata l'ultima richiesta.
    pub last_request: Option<Requested>,
    pub toast: Option<NoveltyToast>,
    /// Una richiesta fallita (rete, risposta troncata…) si riprova una volta
    /// da quest'ora, se è ancora lo stesso giorno.
    pub retry_after: Option<GameTime>,
    /// Il giorno che ha già avuto il suo nuovo tentativo.
    pub retried_day: Option<u64>,
}

impl NarratorState {
    /// Spento, con il motivo mostrato nella cronaca.
    pub fn off(why: impl Into<String>) -> Self {
        Self {
            narrator: None,
            model: why.into(),
            config_error: None,
            paused: false,
            chronicle: Vec::new(),
            known: Known::from_catalog(&Catalog::of_sim()),
            stats: StatsRegistry::default(),
            last_asked: None,
            last_request: None,
            toast: None,
            retry_after: None,
            retried_day: None,
        }
    }

    /// Acceso con `config` (un modello vero, un `MockLlm` o un `Replay`).
    pub fn new(config: NarratorConfig, model: impl Into<String>) -> Self {
        Self {
            narrator: Some(Mutex::new(Narrator::new(config))),
            ..Self::off(model)
        }
    }

    /// Dal `.env` e dall'ambiente (vedi `llm::LlmConfig`).
    pub fn from_env() -> Self {
        Self::from_config(LlmConfig::load())
    }

    /// Da una fonte di variabili qualsiasi (i test usano una mappa).
    #[cfg(test)]
    pub fn from_vars(var: impl Fn(&str) -> Option<String>) -> Self {
        Self::from_config(LlmConfig::from_vars(var))
    }

    fn from_config(config: Result<Option<LlmConfig>, llm::ConfigError>) -> Self {
        match config {
            Ok(Some(c)) => {
                let model = c.model.clone();
                info!("Narratore acceso: {model} su {}", c.endpoint);
                Self::new(NarratorConfig::from_llm_config(c), model)
            }
            Ok(None) => Self::off("spento (nessun LLM_API_URL in .env)"),
            Err(e) => {
                warn!("Narratore spento, configurazione non valida: {e}");
                let mut this = Self::off("spento (configurazione non valida)");
                this.config_error = Some(e.to_string());
                this
            }
        }
    }

    pub fn is_on(&self) -> bool {
        self.narrator.is_some()
    }

    fn narrator_mut(&mut self) -> Option<&mut Narrator> {
        self.narrator
            .as_mut()
            .map(|m| m.get_mut().unwrap_or_else(|e| e.into_inner()))
    }

    fn with_narrator<T>(&self, f: impl FnOnce(&Narrator) -> T) -> Option<T> {
        let m = self.narrator.as_ref()?;
        Some(f(&m.lock().unwrap_or_else(|e| e.into_inner())))
    }

    /// Una richiesta è in volo.
    pub fn busy(&self) -> bool {
        self.with_narrator(Narrator::busy).unwrap_or(false)
    }

    /// Chiamate ancora possibili in quest'ora (None se spento).
    pub fn budget_left(&mut self) -> Option<u32> {
        self.narrator_mut().map(Narrator::budget_left)
    }

    pub fn mean_latency_ms(&self) -> Option<u64> {
        self.with_narrator(Narrator::mean_latency)
            .flatten()
            .map(|d| d.as_millis() as u64)
    }

    /// Chiede la novità di oggi se è l'ora e non l'ha già fatto; `force`
    /// ("Chiedi ora") ignora ora, giorno e pausa, non il budget. Restituisce
    /// cosa è successo, se ha provato.
    pub fn maybe_request(&mut self, world: &World, force: bool) -> Option<Requested> {
        let day = world.clock.day();
        if self.retry_after.is_some_and(|t| t.day() < day) {
            self.retry_after = None;
        }
        let due = world.clock.hour() >= REQUEST_HOUR && self.last_asked.is_none_or(|d| d < day);
        let retry = !due && self.retry_after.is_some_and(|t| world.clock >= t);
        if !force && (self.paused || !(due || retry)) {
            return None;
        }
        let narrator = self.narrator_mut()?;
        let result = if narrator.busy() {
            if retry && !force {
                // Il nuovo tentativo aspetta che finisca quella in volo.
                return None;
            }
            // In volo: il giorno si salta (senza costruire il riassunto).
            Requested::Busy
        } else {
            let summary = WorldSummary::from_world(world);
            if force || retry {
                narrator.request_now(&summary, day)
            } else {
                narrator.request(&summary, day)
            }
        };
        if retry {
            self.retry_after = None;
            self.retried_day = Some(day);
        }
        // Il giorno è passato anche se era occupato: non si recupera.
        self.last_asked = Some(self.last_asked.map_or(day, |d| d.max(day)));
        if result != Requested::Sent {
            debug!("Narratore, giorno {day}: {result:?}");
        }
        self.last_request = Some(result.clone());
        Some(result)
    }

    /// Raccoglie un esito arrivato (senza aspettare) e lo mette in cronaca.
    /// Una richiesta fallita di oggi si riprova una volta, più tardi.
    pub fn poll(&mut self, now: f64, clock: GameTime) -> Option<&ChronicleEntry> {
        let outcome = self.narrator_mut()?.poll()?;
        let entry = ChronicleEntry::from_outcome(&outcome);
        if entry.status == EntryStatus::Failed
            && entry.day == clock.day()
            && self.retried_day != Some(entry.day)
        {
            self.retry_after = Some(clock + RETRY_AFTER_MINUTES);
        }
        self.record(entry, now);
        self.chronicle.last()
    }

    /// Aggiunge una voce: se accettata, aggiorna `known` e le statistiche e
    /// mostra l'avviso.
    pub fn record(&mut self, entry: ChronicleEntry, now: f64) {
        if let Some(draft) = entry.accepted() {
            self.accept(draft);
            self.toast = Some(NoveltyToast {
                text: format!("Novità sul treno: «{}»", draft.proposal.name()),
                shown_at: now,
            });
            info!("Narratore, giorno {}: {}", entry.day, draft.proposal.name());
        } else {
            info!(
                "Narratore, giorno {}: {:?} ({})",
                entry.day,
                entry.status,
                entry.reason.as_deref().unwrap_or("")
            );
        }
        self.chronicle.push(entry);
    }

    fn accept(&mut self, draft: &Draft) {
        self.known.add(&draft.proposal);
        if let Proposal::Statistic(stat) = &draft.proposal {
            self.stats.add(stat.clone());
        }
    }

    /// Cronaca, storico e pausa in JSON, per il salvataggio.
    pub fn save_json(&self) -> String {
        let save = NarratorSave {
            chronicle: self.chronicle.clone(),
            history: self
                .stats
                .history
                .iter()
                .map(|(k, v)| (k.clone(), v.iter().copied().collect()))
                .collect(),
            paused: self.paused,
        };
        serde_json::to_string(&save).unwrap_or_default()
    }

    /// Sostituisce cronaca e statistiche con quelle di un salvataggio (vuoto
    /// o illeggibile: una partita nuova). Il Narratore dimentica il resto e
    /// una richiesta in volo si scarta.
    pub fn restore_json(&mut self, json: &str) {
        let save: NarratorSave = if json.trim().is_empty() {
            NarratorSave::default()
        } else {
            serde_json::from_str(json).unwrap_or_else(|e| {
                warn!("Cronaca del Narratore illeggibile, si riparte da zero: {e}");
                NarratorSave::default()
            })
        };
        self.chronicle = save.chronicle;
        self.paused = save.paused;
        self.known = Known::from_catalog(&Catalog::of_sim());
        self.stats = StatsRegistry::default();
        let accepted: Vec<(u64, Draft)> = self
            .chronicle
            .iter()
            .filter_map(|e| Some((e.day, e.accepted()?.clone())))
            .collect();
        for (_, draft) in &accepted {
            self.accept(draft);
        }
        self.stats.history = save
            .history
            .into_iter()
            .map(|(k, v)| (k, v.into_iter().collect()))
            .collect();
        self.last_asked = self.chronicle.iter().map(|e| e.day).max();
        self.last_request = None;
        self.toast = None;
        self.retry_after = None;
        self.retried_day = None;
        let last = self.last_asked;
        if let Some(n) = self.narrator_mut() {
            n.restore(accepted, last);
        }
    }
}

// --- Plugin ---------------------------------------------------------------------------------

/// Il Narratore e il suo orario. Se `NarratorState` esiste già (test,
/// screenshot) lo tiene, altrimenti lo crea da `.env`.
pub struct NarratorBridgePlugin;

impl Plugin for NarratorBridgePlugin {
    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<NarratorState>() {
            app.insert_resource(NarratorState::from_env());
        }
        app.add_systems(Update, narrate.after(SimTickSet));
    }
}

/// Chiede, raccoglie e campiona: ogni frame, senza mai aspettare.
fn narrate(time: Res<Time<Real>>, sim: Res<Sim>, mut state: ResMut<NarratorState>) {
    let world = &sim.world;
    if state.is_on() {
        state.poll(time.elapsed_secs_f64(), world.clock);
        // Chiede solo se è l'ora (o il momento di riprovare): altrimenti
        // non fa niente e non costruisce il riassunto.
        state.maybe_request(world, false);
    }
    let hour = world.clock.minutes() / MINUTES_PER_HOUR;
    if !state.stats.book.is_empty() && state.stats.last_hour != Some(hour) {
        state.stats.sample(world);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use llm::{Budget, MockLlm};
    use sim::{GameTime, UtilityBrain};

    use super::*;
    use crate::state::new_brain;

    /// Un'interfaccia e una statistica valide, come le scrive il modello.
    pub(crate) const MORALE: &str = r#"{"motivo": "Nessuno misura quanto regge il treno.", "novita": {"tipo": "statistica", "nome": "Morale", "descrizione": "Quanto regge l'animo del treno.", "unita": "%", "scala": [0, 100], "formula": {"media": [{"peso": 100, "sorgente": {"bisogno": "sazieta"}}, {"peso": 100, "sorgente": {"bisogno": "socialita"}}]}, "soglie": [{"sotto": 30, "testo": "Il treno è allo stremo"}]}, "interfaccia": {"titolo": "Morale", "elementi": [{"tipo": "valore", "etichetta": "Morale", "sorgente": {"statistica": "Morale"}}, {"tipo": "pulsante", "etichetta": "Storico", "azione": {"mostra_statistica": "Morale"}}]}}"#;

    fn mock_state(answer: &str) -> NarratorState {
        let llm = Arc::new(MockLlm::fixed(answer));
        NarratorState::new(NarratorConfig::new(llm, Budget::unlimited()), "finto")
    }

    fn app(state: NarratorState) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(Sim {
                world: World::generate(5, 8, 60),
                brain: new_brain(UtilityBrain::new(5)),
            })
            .insert_resource(state)
            .add_plugins(NarratorBridgePlugin);
        app
    }

    fn set_clock(app: &mut App, t: GameTime) {
        app.world_mut().resource_mut::<Sim>().world.clock = t;
    }

    #[test]
    fn stays_off_without_config() {
        let state = NarratorState::from_vars(|_| None);
        assert!(!state.is_on());
        assert!(state.model.contains("spento"));
        let bad = NarratorState::from_vars(|k| (k == "LLM_API_URL").then(|| "http://x".into()));
        assert!(!bad.is_on());
        assert!(bad.config_error.unwrap().contains("LLM_MODEL"));
        // Il gioco gira senza chiedere niente.
        let mut app = app(NarratorState::from_vars(|_| None));
        for _ in 0..3 {
            app.update();
        }
        let state = app.world().resource::<NarratorState>();
        assert!(state.chronicle.is_empty());
        assert_eq!(state.last_request, None);
    }

    #[test]
    fn asks_at_six_and_shows_the_toast() {
        let mut app = app(mock_state(MORALE));
        set_clock(&mut app, GameTime::from_dhm(2, 5, 30));
        app.update();
        assert!(
            !app.world().resource::<NarratorState>().busy(),
            "not before 06:00"
        );
        assert_eq!(app.world().resource::<NarratorState>().last_request, None);
        set_clock(&mut app, GameTime::from_dhm(2, REQUEST_HOUR as u64, 0));
        app.update();
        assert_eq!(
            app.world().resource::<NarratorState>().last_request,
            Some(Requested::Sent)
        );
        for _ in 0..200 {
            if !app.world().resource::<NarratorState>().chronicle.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
            app.update();
        }
        let state = app.world().resource::<NarratorState>();
        assert_eq!(state.chronicle.len(), 1);
        let entry = &state.chronicle[0];
        assert_eq!((entry.day, entry.status), (2, EntryStatus::Proposed));
        assert_eq!(
            state.toast.as_ref().unwrap().text,
            "Novità sul treno: «Morale»"
        );
        assert!(state.known.stat("morale").is_some());
        assert_eq!(state.stats.book.len(), 1);
        // Una volta sola al giorno.
        app.update();
        assert_eq!(app.world().resource::<NarratorState>().chronicle.len(), 1);
        assert!(!app.world().resource::<NarratorState>().busy());
    }

    #[test]
    fn a_failed_day_is_retried_once_later() {
        let llm = Arc::new(MockLlm::scripted([
            Err(llm::LlmError::Truncated),
            Err(llm::LlmError::Truncated),
            Ok(MORALE.to_string()),
        ]));
        let mut app = app(NarratorState::new(
            NarratorConfig::new(llm, Budget::unlimited()),
            "finto",
        ));
        let wait = |app: &mut App, n: usize| {
            for _ in 0..200 {
                if app.world().resource::<NarratorState>().chronicle.len() >= n {
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
                app.update();
            }
            panic!("nessun esito");
        };
        set_clock(&mut app, GameTime::from_dhm(2, 6, 0));
        app.update();
        wait(&mut app, 1);
        {
            let state = app.world().resource::<NarratorState>();
            assert_eq!(state.chronicle[0].status, EntryStatus::Failed);
            assert!(state.toast.is_none(), "a failure is not a novelty");
            assert_eq!(state.retry_after, Some(GameTime::from_dhm(2, 8, 0)));
        }
        set_clock(&mut app, GameTime::from_dhm(2, 7, 0));
        app.update();
        assert!(!app.world().resource::<NarratorState>().busy(), "not yet");
        set_clock(&mut app, GameTime::from_dhm(2, 8, 0));
        app.update();
        wait(&mut app, 2);
        let state = app.world().resource::<NarratorState>();
        assert_eq!(state.chronicle[1].status, EntryStatus::Proposed);
        assert_eq!(state.retried_day, Some(2));
        // Una volta sola.
        set_clock(&mut app, GameTime::from_dhm(2, 12, 0));
        app.update();
        assert!(!app.world().resource::<NarratorState>().busy());
    }

    #[test]
    fn busy_days_are_skipped_and_pause_stops_requests() {
        let llm = Arc::new(MockLlm::fixed(MORALE).with_delay(Duration::from_millis(300)));
        let mut state = NarratorState::new(NarratorConfig::new(llm, Budget::unlimited()), "finto");
        let mut world = World::generate(5, 8, 60);
        world.clock = GameTime::from_dhm(1, 7, 0);
        assert_eq!(state.maybe_request(&world, false), Some(Requested::Sent));
        assert_eq!(
            state.maybe_request(&world, false),
            None,
            "already asked today"
        );
        world.clock = GameTime::from_dhm(2, 7, 0);
        assert_eq!(state.maybe_request(&world, false), Some(Requested::Busy));
        assert_eq!(state.last_asked, Some(2));
        state.paused = true;
        world.clock = GameTime::from_dhm(3, 7, 0);
        assert_eq!(state.maybe_request(&world, false), None);
    }

    #[test]
    fn statistics_history_is_sampled_hourly() {
        let mut state = mock_state(MORALE);
        state.record(
            ChronicleEntry {
                day: 1,
                status: EntryStatus::Proposed,
                draft: Some(narrator::parse(MORALE).unwrap()),
                reason: None,
                attempts: 1,
                latency_ms: 0,
            },
            0.0,
        );
        let mut world = World::generate(5, 8, 60);
        let mut brain = UtilityBrain::new(5);
        assert!(state.stats.sample(&world));
        assert!(!state.stats.sample(&world), "same hour");
        for _ in 0..3 {
            world.run(&mut brain, 60);
            assert!(state.stats.sample(&world));
        }
        let h: Vec<(u64, f32)> = state.stats.history("MORALE").collect();
        assert_eq!(h.len(), 4);
        assert!(h.windows(2).all(|w| w[1].0 == w[0].0 + 1));
        assert!(h.iter().all(|(_, v)| (0.0..=100.0).contains(v)));
        // Tetto allo storico.
        for _ in 0..HISTORY_CAP {
            world.run(&mut brain, 60);
            state.stats.sample(&world);
        }
        assert_eq!(state.stats.history("morale").count(), HISTORY_CAP);
    }

    #[test]
    fn chronicle_survives_json() {
        let mut state = mock_state(MORALE);
        state.record(
            ChronicleEntry {
                day: 3,
                status: EntryStatus::Proposed,
                draft: Some(narrator::parse(MORALE).unwrap()),
                reason: None,
                attempts: 2,
                latency_ms: 1300,
            },
            0.0,
        );
        state.record(
            ChronicleEntry {
                day: 4,
                status: EntryStatus::Rejected,
                draft: None,
                reason: Some("nella risposta non c'è un oggetto JSON".into()),
                attempts: 2,
                latency_ms: 900,
            },
            0.0,
        );
        state.stats.sample(&World::generate(5, 8, 60));
        state.paused = true;
        let json = state.save_json();
        let mut again = mock_state(MORALE);
        again.restore_json(&json);
        assert_eq!(again.chronicle, state.chronicle);
        assert!(again.paused);
        assert_eq!(again.last_asked, Some(4));
        assert_eq!(again.stats.book.len(), 1);
        assert_eq!(again.stats.history("morale").count(), 1);
        assert!(again.known.stat("Morale").is_some());
        // Vuoto o rotto: una partita nuova.
        again.restore_json("");
        assert!(again.chronicle.is_empty() && again.stats.book.is_empty());
        again.restore_json("{rotto");
        assert!(again.chronicle.is_empty());
    }
}
