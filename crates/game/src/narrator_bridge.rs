//! Il Narratore nel gioco (A3 di "Direzione nuova" in
//! `docs/piano-vita-ed-economia.md`) e il suo passaggio al Custode (A2).
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
//! **Il Custode.** Una bozza che passa il controllo del Narratore si
//! consegna al mondo con `World::schedule` per la **prossima ora piena** di
//! gioco: in quel minuto la sim la esamina (`World::review`) e, se ha senso,
//! la fa entrare nel mondo (`World::apply`); l'esito resta nel mondo
//! (`World::decisions`), e con esso nel salvataggio. La cronaca segue
//! l'esito ([`NarratorState::sync`]): "entrata nel mondo (giorno X, ore Y)"
//! o "respinta dal Custode: motivo".
//!
//! Tiene:
//! - la **cronaca**: ogni esito con giorno, verdetto, bozza e motivo, e il
//!   numero con cui la bozza è stata consegnata al Custode;
//! - il `Known` aggiornato con le bozze accettate (le prossime proposte
//!   possono citarle anche prima che entrino nel mondo).
//!
//! Le statistiche derivate e il loro storico sono nel mondo
//! (`World::statistics`). Cronaca e pausa finiscono nella parte "gioco" del
//! salvataggio, come JSON ([`NarratorState::save_json`], vedi `save_file.rs`).
//!
//! **Voci "proposta" di una cronaca vecchia** (accettate prima che il
//! Custode esistesse, senza numero): al caricamento si consegnano al Custode
//! per la prossima ora piena, come una bozza appena accettata; lo storico
//! delle statistiche salvato con la cronaca passa al mondo quando la
//! statistica ci entra.

use std::collections::BTreeMap;
use std::sync::Mutex;

use bevy::prelude::*;
use llm::LlmConfig;
use narrator::{
    Catalog, Draft, Known, Narrator, NarratorConfig, NarratorOutcome, Requested, Verdict,
    WorldSummary,
};
use serde::{Deserialize, Serialize};
use sim::{GameTime, MINUTES_PER_HOUR, World};

use crate::sim_bridge::SimTickSet;
use crate::state::Sim;

/// Ora del giorno da cui si chiede la novità del giorno.
pub const REQUEST_HOUR: u32 = 6;
/// Secondi reali per cui resta visibile l'avviso di una novità.
pub const TOAST_SECS: f64 = 10.0;
/// Minuti di gioco dopo una richiesta fallita prima dell'unico nuovo
/// tentativo dello stesso giorno.
pub const RETRY_AFTER_MINUTES: u64 = 120;

/// La prossima ora piena dopo `t` (il minuto fissato in cui il Custode
/// esamina una bozza).
pub fn next_full_hour(t: GameTime) -> GameTime {
    GameTime((t.minutes() / MINUTES_PER_HOUR + 1) * MINUTES_PER_HOUR)
}

// --- Cronaca -----------------------------------------------------------------------

/// Come è finita la richiesta di un giorno.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryStatus {
    /// Passata il controllo del Narratore, non ancora consegnata al Custode
    /// (solo nelle cronache di prima del Custode: al caricamento si
    /// consegna).
    #[serde(rename = "proposta")]
    Proposed,
    /// Consegnata al Custode: aspetta il suo minuto (`at`).
    #[serde(rename = "in_arrivo")]
    Scheduled,
    /// Entrata nel mondo.
    #[serde(rename = "entrata")]
    Applied,
    /// Respinta dal Custode (il motivo in `reason`).
    #[serde(rename = "respinta")]
    Refused,
    /// Rifiutata dal controllo del Narratore anche al secondo tentativo.
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
    /// Perché è stata rifiutata, respinta o è fallita.
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub attempts: u8,
    #[serde(default)]
    pub latency_ms: u64,
    /// Il numero della consegna al Custode (`World::schedule`).
    #[serde(default)]
    pub seq: Option<u32>,
    /// Il minuto in cui il Custode la esamina (o l'ha esaminata).
    #[serde(default)]
    pub at: Option<GameTime>,
    /// Cosa è entrato nel mondo, detto dal Custode.
    #[serde(default)]
    pub summary: Option<String>,
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
            seq: None,
            at: None,
            summary: None,
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

    /// La bozza, se il Narratore l'ha accettata (entrata, in arrivo o
    /// respinta dal Custode dopo).
    pub fn accepted(&self) -> Option<&Draft> {
        matches!(
            self.status,
            EntryStatus::Proposed
                | EntryStatus::Scheduled
                | EntryStatus::Applied
                | EntryStatus::Refused
        )
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

// --- Salvataggio -------------------------------------------------------------------------

/// Cosa del Narratore va nel salvataggio (JSON dentro il corpo postcard: i
/// campi nuovi si aggiungono con `#[serde(default)]` senza cambiare formato).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NarratorSave {
    #[serde(default)]
    pub chronicle: Vec<ChronicleEntry>,
    /// Storico delle statistiche di una cronaca di prima del Custode
    /// (chiave → (ora di gioco, valore)); ora lo storico è nel mondo.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
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
    /// Storico di statistiche di una cronaca vecchia, in attesa che la
    /// statistica entri nel mondo.
    legacy_history: BTreeMap<String, Vec<(u64, f32)>>,
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
            legacy_history: BTreeMap::new(),
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

    /// Raccoglie un esito arrivato (senza aspettare) e lo mette in cronaca;
    /// una bozza accettata va al Custode. Una richiesta fallita di oggi si
    /// riprova una volta, più tardi.
    pub fn poll(&mut self, now: f64, world: &mut World) -> Option<&ChronicleEntry> {
        let outcome = self.narrator_mut()?.poll()?;
        let entry = ChronicleEntry::from_outcome(&outcome);
        let clock = world.clock;
        if entry.status == EntryStatus::Failed
            && entry.day == clock.day()
            && self.retried_day != Some(entry.day)
        {
            self.retry_after = Some(clock + RETRY_AFTER_MINUTES);
        }
        self.record(entry, now, Some(world));
        self.chronicle.last()
    }

    /// Aggiunge una voce: se accettata, aggiorna `known`, la consegna al
    /// Custode di `world` (per la prossima ora piena) e mostra l'avviso.
    pub fn record(&mut self, mut entry: ChronicleEntry, now: f64, world: Option<&mut World>) {
        if let Some(draft) = entry.accepted().cloned() {
            self.known.add(&draft.proposal);
            if let Some(world) = world {
                deliver(&mut entry, world);
            }
            let when = entry.at.map_or(String::new(), |t| {
                format!(" (entra alle {:02}:00)", t.hour())
            });
            self.toast = Some(NoveltyToast {
                text: format!("Novità sul treno: «{}»{when}", draft.proposal.name()),
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

    /// Aggiorna la cronaca con le decisioni del Custode (ogni frame, costa
    /// poco): le voci in arrivo diventano entrate o respinte, con l'avviso.
    pub fn sync(&mut self, world: &mut World, now: f64) {
        let mut toast = None;
        let mut refused: Vec<(String, String)> = Vec::new();
        for entry in self.chronicle.iter_mut() {
            if entry.status != EntryStatus::Scheduled {
                continue;
            }
            let Some(decision) = entry.seq.and_then(|seq| world.decision(seq)) else {
                continue;
            };
            entry.at = Some(decision.at);
            let name = entry.title();
            match &decision.result {
                Ok(applied) => {
                    entry.status = EntryStatus::Applied;
                    entry.summary = Some(applied.summary.clone());
                    toast = Some(format!("Entrata nel mondo: «{name}»"));
                }
                Err(why) => {
                    entry.status = EntryStatus::Refused;
                    entry.reason = Some(why.0.clone());
                    toast = Some(format!("Respinta dal Custode: «{name}»"));
                    refused.push((name, why.0.clone()));
                }
            }
        }
        // What the Custode refused doesn't exist: the Narratore must not
        // refer to it (and learns why).
        if !refused.is_empty() {
            self.rebuild_known(world);
            if let Some(n) = self.narrator_mut() {
                for (name, why) in &refused {
                    n.refused(name, why);
                }
            }
        }
        if let Some(text) = toast {
            self.toast = Some(NoveltyToast {
                text,
                shown_at: now,
            });
        }
        // Lo storico di una cronaca vecchia, quando la statistica entra.
        if !self.legacy_history.is_empty() {
            let names: Vec<String> = self
                .legacy_history
                .keys()
                .filter(|k| world.statistics().get(k).is_some())
                .cloned()
                .collect();
            for name in names {
                if let Some(h) = self.legacy_history.remove(&name) {
                    world.adopt_statistic_history(&name, h);
                }
            }
        }
    }

    /// `known` dal catalogo del mondo più le bozze accettate che il Custode
    /// non ha respinto.
    fn rebuild_known(&mut self, world: &World) {
        self.known = Known::from_catalog(&Catalog::of(world.catalog()));
        for entry in &self.chronicle {
            if entry.status != EntryStatus::Refused
                && let Some(d) = entry.accepted()
            {
                self.known.add(&d.proposal);
            }
        }
    }

    /// Cronaca e pausa in JSON, per il salvataggio.
    pub fn save_json(&self) -> String {
        let save = NarratorSave {
            chronicle: self.chronicle.clone(),
            history: self.legacy_history.clone(),
            paused: self.paused,
        };
        serde_json::to_string(&save).unwrap_or_default()
    }

    /// Sostituisce la cronaca con quella di un salvataggio (vuoto o
    /// illeggibile: una partita nuova) per il mondo `world` appena caricato.
    /// Il Narratore dimentica il resto e una richiesta in volo si scarta. Le
    /// voci "proposta" di una cronaca di prima del Custode si consegnano al
    /// Custode per la prossima ora piena.
    pub fn restore_json(&mut self, json: &str, world: &mut World) {
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
        self.legacy_history = save.history;
        let accepted: Vec<(u64, Draft)> = self
            .chronicle
            .iter()
            .filter(|e| e.status != EntryStatus::Refused)
            .filter_map(|e| Some((e.day, e.accepted()?.clone())))
            .collect();
        self.rebuild_known(world);
        for entry in self.chronicle.iter_mut() {
            if entry.status == EntryStatus::Proposed {
                deliver(entry, world);
            }
        }
        self.last_asked = self.chronicle.iter().map(|e| e.day).max();
        self.last_request = None;
        self.toast = None;
        self.retry_after = None;
        self.retried_day = None;
        let last = self.last_asked;
        if let Some(n) = self.narrator_mut() {
            n.restore(accepted, last);
        }
        self.sync(world, 0.0);
        self.toast = None;
    }
}

/// Consegna la bozza di `entry` al Custode di `world`, per la prossima ora
/// piena.
fn deliver(entry: &mut ChronicleEntry, world: &mut World) {
    let Some(draft) = entry.draft.clone() else {
        return;
    };
    let at = next_full_hour(world.clock);
    entry.seq = Some(world.schedule(draft, at));
    entry.at = Some(at);
    entry.status = EntryStatus::Scheduled;
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

/// Chiede, raccoglie e segue il Custode: ogni frame, senza mai aspettare.
fn narrate(time: Res<Time<Real>>, mut sim: ResMut<Sim>, mut state: ResMut<NarratorState>) {
    let now = time.elapsed_secs_f64();
    if state.is_on() {
        state.poll(now, &mut sim.world);
        // Chiede solo se è l'ora (o il momento di riprovare): altrimenti
        // non fa niente e non costruisce il riassunto.
        state.maybe_request(&sim.world, false);
    }
    // Solo se qualcosa aspetta il Custode (non tocca il mondo altrimenti).
    if state
        .chronicle
        .iter()
        .any(|e| e.status == EntryStatus::Scheduled)
        || !state.legacy_history.is_empty()
    {
        state.sync(&mut sim.world, now);
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

    /// Un oggetto, come lo scrive il modello.
    pub(crate) const BORRACCIA: &str = r#"{"motivo": "In coda si beve poco e male.", "novita": {"tipo": "oggetto", "nome": "Borraccia di latta", "descrizione": "Una borraccia battuta a mano: tiene l'acqua calda.", "categoria": "durevole", "valore": 14, "pila": 3, "ingredienti": [{"oggetto": "metallo", "qta": 1}], "lavoro": "operaio", "aspetto": {"forma": "bottiglia", "colore": "grigio", "dettaglio": "etichetta"}}}"#;

    fn wait_chronicle(app: &mut App, n: usize) {
        for _ in 0..200 {
            if app.world().resource::<NarratorState>().chronicle.len() >= n {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
            app.update();
        }
        panic!("nessun esito");
    }

    /// Fa girare il mondo di `minutes` minuti (come farebbe il gioco) e un frame.
    fn advance(app: &mut App, minutes: u64) {
        {
            let mut sim = app.world_mut().resource_mut::<Sim>();
            let Sim { world, brain } = &mut *sim;
            world.run(brain, minutes);
        }
        app.update();
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
        wait_chronicle(&mut app, 1);
        let state = app.world().resource::<NarratorState>();
        assert_eq!(state.chronicle.len(), 1);
        let entry = &state.chronicle[0];
        assert_eq!((entry.day, entry.status), (2, EntryStatus::Scheduled));
        assert_eq!(entry.at, Some(GameTime::from_dhm(2, 7, 0)));
        assert_eq!(
            state.toast.as_ref().unwrap().text,
            "Novità sul treno: «Morale» (entra alle 07:00)"
        );
        assert!(state.known.stat("morale").is_some());
        assert_eq!(app.world().resource::<Sim>().world.pending().len(), 1);
        // Una volta sola al giorno.
        app.update();
        assert_eq!(app.world().resource::<NarratorState>().chronicle.len(), 1);
        assert!(!app.world().resource::<NarratorState>().busy());
    }

    #[test]
    fn an_accepted_proposal_becomes_real_at_the_next_full_hour() {
        let mut app = app(mock_state(BORRACCIA));
        set_clock(&mut app, GameTime::from_dhm(1, 6, 20));
        app.update();
        wait_chronicle(&mut app, 1);
        {
            let state = app.world().resource::<NarratorState>();
            assert_eq!(state.chronicle[0].status, EntryStatus::Scheduled);
            let world = &app.world().resource::<Sim>().world;
            assert!(world.catalog().find_item("borraccia di latta").is_none());
        }
        // 06:59: not yet.
        advance(&mut app, 39);
        assert_eq!(
            app.world().resource::<NarratorState>().chronicle[0].status,
            EntryStatus::Scheduled
        );
        // 07:00 passes: the Custode applies it, the chronicle follows.
        advance(&mut app, 2);
        let state = app.world().resource::<NarratorState>();
        let entry = &state.chronicle[0];
        assert_eq!(entry.status, EntryStatus::Applied, "{:?}", entry.reason);
        assert_eq!(entry.at, Some(GameTime::from_dhm(1, 7, 0)));
        assert!(
            entry
                .summary
                .as_deref()
                .unwrap()
                .contains("borraccia di latta")
        );
        assert_eq!(
            state.toast.as_ref().unwrap().text,
            "Entrata nel mondo: «Borraccia di latta»"
        );
        let world = &app.world().resource::<Sim>().world;
        let item = world
            .catalog()
            .find_item("Borracce di latta")
            .expect("real");
        assert_eq!(
            world
                .catalog()
                .item(item)
                .appearance
                .as_ref()
                .unwrap()
                .shape,
            "bottiglia"
        );
        assert_eq!(world.applied().count(), 1);
        // The chronicle's status reads "entrata nel mondo (giorno 1, ore 07:00)".
        assert_eq!(
            crate::chronicle_ui::status_line(entry).0,
            "entrata nel mondo (giorno 1, ore 07:00)"
        );
    }

    #[test]
    fn a_refused_proposal_says_why() {
        let bad = r#"{"motivo": "Serve metallo.", "novita": {"tipo": "oggetto", "nome": "Pepita", "descrizione": "Metallo trovato nel nulla.", "categoria": "materia_prima", "valore": 3, "ingredienti": [], "lavoro": "operaio"}}"#;
        let mut app = app(mock_state(bad));
        set_clock(&mut app, GameTime::from_dhm(1, 6, 0));
        app.update();
        wait_chronicle(&mut app, 1);
        advance(&mut app, 61);
        let state = app.world().resource::<NarratorState>();
        let entry = &state.chronicle[0];
        assert_eq!(entry.status, EntryStatus::Refused);
        assert!(
            entry.reason.as_deref().unwrap().contains("Serra"),
            "{entry:?}"
        );
        assert!(
            crate::chronicle_ui::status_line(entry)
                .0
                .starts_with("respinta dal Custode: ")
        );
        // It doesn't exist: the next proposals can't refer to it.
        assert!(state.known.item("pepita").is_none());
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
        assert_eq!(state.chronicle[1].status, EntryStatus::Scheduled);
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
    fn statistics_live_in_the_world_and_are_sampled_hourly() {
        let mut state = mock_state(MORALE);
        let mut world = World::generate(5, 8, 60);
        let mut brain = UtilityBrain::new(5);
        state.record(
            ChronicleEntry {
                day: 1,
                status: EntryStatus::Proposed,
                draft: Some(narrator::parse(MORALE).unwrap()),
                reason: None,
                attempts: 1,
                latency_ms: 0,
                seq: None,
                at: None,
                summary: None,
            },
            0.0,
            Some(&mut world),
        );
        assert!(world.statistics().is_empty(), "not before the full hour");
        for _ in 0..4 {
            world.run(&mut brain, 60);
        }
        state.sync(&mut world, 0.0);
        assert_eq!(state.chronicle[0].status, EntryStatus::Applied);
        assert_eq!(world.statistics().len(), 1);
        let h: Vec<(u64, f32)> = world.statistics().history("MORALE").collect();
        assert!(h.len() >= 3, "{h:?}");
        assert!(h.windows(2).all(|w| w[1].0 == w[0].0 + 1));
        assert!(h.iter().all(|(_, v)| (0.0..=100.0).contains(v)));
    }

    #[test]
    fn chronicle_survives_json() {
        let mut state = mock_state(MORALE);
        let mut world = World::generate(5, 8, 60);
        state.record(
            ChronicleEntry {
                day: 3,
                status: EntryStatus::Proposed,
                draft: Some(narrator::parse(MORALE).unwrap()),
                reason: None,
                attempts: 2,
                latency_ms: 1300,
                seq: None,
                at: None,
                summary: None,
            },
            0.0,
            Some(&mut world),
        );
        state.record(
            ChronicleEntry {
                day: 4,
                status: EntryStatus::Rejected,
                draft: None,
                reason: Some("nella risposta non c'è un oggetto JSON".into()),
                attempts: 2,
                latency_ms: 900,
                seq: None,
                at: None,
                summary: None,
            },
            0.0,
            None,
        );
        state.paused = true;
        let json = state.save_json();
        let mut again = mock_state(MORALE);
        again.restore_json(&json, &mut world);
        assert_eq!(again.chronicle, state.chronicle);
        assert!(again.paused);
        assert_eq!(again.last_asked, Some(4));
        assert!(again.known.stat("Morale").is_some());
        // Still waiting for its hour: not delivered twice.
        assert_eq!(world.pending().len(), 1);
        // Vuoto o rotto: una partita nuova.
        again.restore_json("", &mut world);
        assert!(again.chronicle.is_empty());
        again.restore_json("{rotto", &mut world);
        assert!(again.chronicle.is_empty());
    }

    #[test]
    fn old_proposals_go_to_the_custode_on_load() {
        // A chronicle saved before the Custode: "proposta", no number, and
        // the statistics' history kept by the game.
        let old = format!(
            r#"{{"chronicle": [{{"day": 2, "status": "proposta", "draft": {MORALE}, "attempts": 1, "latency_ms": 900}}], "history": {{"morale": [[4, 50.0], [5, 51.0]]}}, "paused": false}}"#
        );
        let mut world = World::generate(5, 8, 60);
        let mut brain = UtilityBrain::new(5);
        let mut state = mock_state(MORALE);
        state.restore_json(&old, &mut world);
        let entry = &state.chronicle[0];
        assert_eq!(entry.status, EntryStatus::Scheduled);
        assert_eq!(entry.at, Some(GameTime::from_dhm(1, 7, 0)));
        world.run(&mut brain, 61);
        state.sync(&mut world, 0.0);
        assert_eq!(state.chronicle[0].status, EntryStatus::Applied);
        // The old history went to the world, before the new samples.
        let h: Vec<(u64, f32)> = world.statistics().history("Morale").collect();
        assert_eq!(&h[..2], &[(4, 50.0), (5, 51.0)]);
        assert!(h.len() >= 3);
    }
}
