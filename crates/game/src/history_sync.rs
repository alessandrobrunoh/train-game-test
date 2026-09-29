//! Storico SQLite della partita (`<cartella della run>/history.sqlite`, crate
//! `history`): ogni evento della simulazione finisce nel database, anche
//! quelli che il registro in memoria (2000 eventi) ha già dimenticato.
//!
//! - Sul thread principale si cattura solo il lotto di eventi nuovi
//!   ([`history::SyncBatch::capture`]: una copia di qualche centinaio di
//!   eventi, decine di µs); la scrittura (una transazione per lotto, più i
//!   checkpoint del WAL) la fa un thread dedicato, così il frame non aspetta
//!   mai il disco.
//! - Si cattura ogni [`SYNC_EVERY_SECS`] secondi reali, o prima se si sono
//!   accumulati [`SYNC_BACKLOG`] eventi: il registro in memoria ne tiene
//!   sempre almeno 1500, quindi anche a 3000x (~2 giorni di gioco e ~300
//!   eventi al secondo) non se ne perde nessuno. Se succede lo stesso (un
//!   blocco lungo), il buco è registrato nella tabella `gaps` e segnalato.
//! - Le query dell'interfaccia (`history_ui.rs`) usano una seconda
//!   connessione in sola lettura: col WAL leggono senza aspettare lo scrittore.
//! - `WorldReplaced` (caricamento o nuova partita): si svuota lo scrittore,
//!   si apre il database della run indicata da `RunInfo` e lo si riallinea al
//!   mondo caricato ([`history::History::reconcile`]: si cancella il "futuro"
//!   oltre il salvataggio). Prima della sostituzione si salvano su disco gli
//!   eventi non ancora scritti del mondo vecchio.
//! - All'uscita si scrive l'ultimo lotto e si aspetta lo scrittore.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use history::{History, SyncBatch, SyncReport};
use sim::World;

use crate::saves::{SaveQueue, WorldRebuildSet, WorldSwapSet};
use crate::sim_bridge::SimTickSet;
use crate::state::{RunInfo, Sim, WorldReplaced};

/// Secondi reali tra una sincronizzazione e l'altra.
pub const SYNC_EVERY_SECS: f32 = 0.5;
/// Eventi in attesa oltre i quali si sincronizza subito (il registro in
/// memoria ne tiene almeno `max_events * 3 / 4`).
pub const SYNC_BACKLOG: u64 = 400;

pub struct HistorySyncPlugin;

impl Plugin for HistorySyncPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HistoryDb>()
            .add_systems(
                PreUpdate,
                (
                    flush_before_swap.before(WorldSwapSet),
                    reopen_after_swap
                        .in_set(WorldRebuildSet)
                        .run_if(on_message::<WorldReplaced>),
                ),
            )
            .add_systems(Update, sync_history.after(SimTickSet))
            .add_systems(Last, flush_on_exit);
    }
}

/// Costi e contatori della sincronizzazione, mostrati nella finestra "Storia".
#[derive(Debug, Default, Clone)]
pub struct SyncStats {
    /// Lotti scritti ed eventi inseriti da quando il database è aperto.
    pub batches: u64,
    pub events: u64,
    /// Cattura sul thread principale (µs): ultima e massima.
    pub last_capture_us: u64,
    pub max_capture_us: u64,
    /// Scrittura sul thread dedicato (µs): ultima e massima.
    pub last_write_us: u64,
    pub max_write_us: u64,
    /// Eventi persi (buchi registrati) da quando il database è aperto.
    pub lost: u64,
}

/// Il database dello storico della partita in corso.
#[derive(Resource, Default)]
pub struct HistoryDb {
    /// Cartella della run aperta (anche se l'apertura è fallita).
    dir: Option<PathBuf>,
    writer: Option<Writer>,
    /// Connessione in sola lettura per l'interfaccia (`Mutex`: le risorse
    /// devono essere `Sync`, la connessione no).
    reader: Option<Mutex<History>>,
    /// Prossimo evento da catturare (tutti quelli prima sono stati inviati).
    next: u64,
    /// Le persone sono già state registrate (la prima cattura le include).
    seeded: bool,
    /// Secondi reali dall'ultima cattura.
    since_sync: f32,
    /// Errore di apertura o scrittura, se c'è: lo storico è sospeso.
    pub error: Option<String>,
    pub stats: SyncStats,
    /// Cambia a ogni riapertura e a ogni lotto scritto: le viste la usano
    /// per sapere quando rileggere.
    pub generation: u64,
}

impl HistoryDb {
    /// Esegue una query sulla connessione in sola lettura (se lo storico è aperto).
    pub fn read<T>(
        &self,
        query: impl FnOnce(&History) -> history::Result<T>,
    ) -> Option<Result<T, String>> {
        let reader = self.reader.as_ref()?;
        let history = reader.lock().ok()?;
        Some(query(&history).map_err(|e| e.to_string()))
    }

    pub fn is_open(&self) -> bool {
        self.writer.is_some()
    }

    /// Percorso del file del database aperto.
    pub fn path(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(history::DB_FILE))
    }

    /// Apre (o riapre) il database di `dir` e lo riallinea a `world`.
    fn open(&mut self, dir: &Path, world: &World) {
        self.close();
        self.dir = Some(dir.to_path_buf());
        self.stats = SyncStats::default();
        self.generation += 1;
        match open_writer(dir, world) {
            Ok((writer, reader, next, seeded)) => {
                self.writer = Some(writer);
                self.reader = Some(Mutex::new(reader));
                self.next = next;
                self.seeded = seeded;
                self.error = None;
                self.since_sync = 0.0;
            }
            Err(e) => {
                error!("Storico non disponibile ({}): {e}", dir.display());
                self.error = Some(e);
            }
        }
    }

    /// Scrive i lotti in coda e chiude le connessioni.
    fn close(&mut self) {
        if let Some(writer) = self.writer.take() {
            let done = writer.finish();
            self.absorb(done);
        }
        self.reader = None;
    }

    /// Cattura gli eventi nuovi di `world` e li manda allo scrittore.
    fn sync(&mut self, world: &World) {
        if self.writer.is_none() {
            return;
        }
        let total = world.events_total();
        if total == self.next && self.seeded {
            return;
        }
        let start = Instant::now();
        let batch = SyncBatch::capture(world, self.next, !self.seeded);
        let capture_us = start.elapsed().as_micros() as u64;
        self.stats.last_capture_us = capture_us;
        self.stats.max_capture_us = self.stats.max_capture_us.max(capture_us);
        if let Some((from, to)) = batch.gap {
            warn!(
                "Storico: {} eventi ({from}..{to}) usciti dal registro prima di essere salvati",
                to - from
            );
        }
        self.next = total;
        self.seeded = true;
        self.since_sync = 0.0;
        let sent = self.writer.as_ref().is_some_and(|w| w.send(batch).is_ok());
        if !sent {
            // Lo scrittore si è fermato: i suoi ultimi messaggi dicono perché.
            if let Some(writer) = self.writer.take() {
                let done = writer.finish();
                self.absorb(done);
            }
            self.error
                .get_or_insert_with(|| "lo scrittore dello storico si è fermato".to_string());
        }
    }

    /// Legge i resoconti dello scrittore.
    fn poll(&mut self) {
        let done: Vec<Done> = match &self.writer {
            Some(writer) => writer
                .done
                .lock()
                .map_or_else(|_| Vec::new(), |rx| rx.try_iter().collect()),
            None => return,
        };
        self.absorb(done);
    }

    fn absorb(&mut self, done: Vec<Done>) {
        for d in done {
            match d {
                Done::Synced { report, spent } => {
                    let us = spent.as_micros() as u64;
                    self.stats.batches += 1;
                    self.stats.events += report.inserted;
                    self.stats.last_write_us = us;
                    self.stats.max_write_us = self.stats.max_write_us.max(us);
                    if let Some((from, to)) = report.gap {
                        self.stats.lost += to - from;
                    }
                    if let Some(seq) = report.rolled_back_to {
                        warn!("Storico riportato all'evento {seq}");
                    }
                    self.generation += 1;
                }
                Done::Failed(e) => {
                    error!("Errore dello storico: {e}");
                    self.error = Some(e);
                }
            }
        }
    }
}

/// Apre scrittore e lettore; riallinea lo storico al mondo.
fn open_writer(dir: &Path, world: &World) -> Result<(Writer, History, u64, bool), String> {
    let start = Instant::now();
    let mut history = History::open(dir).map_err(|e| e.to_string())?;
    let report = history.reconcile(world).map_err(|e| e.to_string())?;
    if let Some(seq) = report.rolled_back_to {
        info!(
            "Storico riallineato al mondo caricato: eventi da {seq} in poi cancellati{}",
            if report.unverifiable {
                " (lo storico precedente viene da un'altra linea temporale)"
            } else {
                ""
            }
        );
    }
    let next = history.synced();
    let seeded = history.is_seeded();
    let reader = History::open_read_only(dir).map_err(|e| e.to_string())?;
    info!(
        "Storico aperto: {} ({} eventi già registrati, {:.1?})",
        dir.join(history::DB_FILE).display(),
        next,
        start.elapsed()
    );
    Ok((Writer::spawn(history), reader, next, seeded))
}

// --- Thread di scrittura ----------------------------------------------------------

enum Job {
    Batch(Box<SyncBatch>),
    Shutdown,
}

enum Done {
    Synced { report: SyncReport, spent: Duration },
    Failed(String),
}

struct Writer {
    jobs: Sender<Job>,
    done: Mutex<Receiver<Done>>,
    handle: Option<JoinHandle<()>>,
}

impl Writer {
    fn spawn(mut history: History) -> Writer {
        let (jobs, job_rx) = channel::<Job>();
        let (done_tx, done) = channel::<Done>();
        let handle = std::thread::Builder::new()
            .name("history-writer".into())
            .spawn(move || {
                for job in job_rx {
                    let Job::Batch(batch) = job else {
                        break;
                    };
                    let start = Instant::now();
                    let msg = match history.apply(&batch) {
                        Ok(report) => Done::Synced {
                            report,
                            spent: start.elapsed(),
                        },
                        Err(e) => Done::Failed(e.to_string()),
                    };
                    let failed = matches!(msg, Done::Failed(_));
                    if done_tx.send(msg).is_err() || failed {
                        break;
                    }
                }
            })
            .ok();
        Writer {
            jobs,
            done: Mutex::new(done),
            handle,
        }
    }

    fn send(&self, batch: SyncBatch) -> Result<(), ()> {
        self.jobs.send(Job::Batch(Box::new(batch))).map_err(|_| ())
    }

    /// Scrive i lotti in coda, ferma il thread e restituisce gli ultimi resoconti.
    fn finish(mut self) -> Vec<Done> {
        self.stop();
        self.done
            .lock()
            .map_or_else(|_| Vec::new(), |rx| rx.try_iter().collect())
    }

    fn stop(&mut self) {
        let _ = self.jobs.send(Job::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.stop();
    }
}

// --- Sistemi ------------------------------------------------------------------------

/// Cattura periodica (e apertura all'avvio, appena c'è `RunInfo`).
fn sync_history(
    time: Res<Time<Real>>,
    sim: Res<Sim>,
    run: Option<Res<RunInfo>>,
    mut db: ResMut<HistoryDb>,
) {
    let world = &sim.world;
    if db.dir.is_none() {
        let Some(run) = run else {
            return;
        };
        db.open(&run.dir, world);
    }
    db.poll();
    if !db.is_open() {
        return;
    }
    if world.events_total() < db.next {
        // Il mondo è tornato indietro senza `WorldReplaced`: si riallinea.
        if let Some(dir) = db.dir.clone() {
            db.open(&dir, world);
        }
    }
    db.since_sync += time.delta_secs();
    let backlog = world.events_total().saturating_sub(db.next);
    if db.since_sync >= SYNC_EVERY_SECS || backlog >= SYNC_BACKLOG || !db.seeded {
        db.sync(world);
    }
}

/// Prima che un comando di `saves.rs` sostituisca il mondo (o lo salvi), si
/// cattura quello che manca: la run vecchia resta completa.
fn flush_before_swap(queue: Option<Res<SaveQueue>>, sim: Res<Sim>, mut db: ResMut<HistoryDb>) {
    if queue.is_some_and(|q| !q.0.is_empty()) {
        db.sync(&sim.world);
    }
}

/// Mondo sostituito: si apre lo storico della run di `RunInfo`, riallineato.
fn reopen_after_swap(sim: Res<Sim>, run: Option<Res<RunInfo>>, mut db: ResMut<HistoryDb>) {
    let Some(run) = run else {
        return;
    };
    db.open(&run.dir, &sim.world);
}

/// All'uscita: ultimo lotto e attesa dello scrittore.
fn flush_on_exit(exit: MessageReader<AppExit>, sim: Res<Sim>, mut db: ResMut<HistoryDb>) {
    if exit.is_empty() {
        return;
    }
    db.sync(&sim.world);
    db.close();
    info!(
        "Storico chiuso ({} eventi scritti in questa sessione)",
        db.stats.events
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim::{MINUTES_PER_DAY, UtilityBrain};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "traingame-history-sync-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// L'app con i sistemi veri: a velocità altissima (qui ~900 eventi per
    /// frame) nessun evento va perso, un caricamento riallinea lo storico e
    /// l'uscita scrive l'ultimo lotto.
    #[test]
    fn the_plugin_keeps_up_reloads_and_flushes_on_exit() {
        let dir = temp_dir("app");
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<WorldReplaced>()
            .add_plugins(HistorySyncPlugin)
            .insert_resource(RunInfo {
                run_id: "test".into(),
                dir: dir.clone(),
            })
            .insert_resource(Sim {
                world: World::generate(42, 20, 400),
                brain: crate::state::new_brain(UtilityBrain::new(42)),
            });
        let advance = |app: &mut App, days: u64| {
            let mut sim = app.world_mut().resource_mut::<Sim>();
            let Sim { world, brain } = &mut *sim;
            world.run(brain, days * MINUTES_PER_DAY);
            app.update();
        };
        app.update();
        let mut saved = None;
        for frame in 0..30 {
            advance(&mut app, 5);
            if frame == 10 {
                saved = Some(app.world().resource::<Sim>().world.clone());
            }
        }
        let total = app.world().resource::<Sim>().world.events_total();
        assert!(total > 20_000, "{total}");

        // Caricamento di un salvataggio più vecchio.
        let saved = saved.unwrap();
        let saved_total = saved.events_total();
        app.world_mut().resource_mut::<Sim>().world = saved;
        app.world_mut().write_message(WorldReplaced);
        app.update();
        let db = app.world().resource::<HistoryDb>();
        assert!(db.error.is_none(), "{:?}", db.error);
        assert_eq!(db.next, saved_total);
        let counts = db.read(|h| h.counts()).unwrap().unwrap();
        assert_eq!((counts.events, counts.lost_events), (saved_total, 0));

        // Si gioca ancora un po' e si esce.
        for _ in 0..5 {
            advance(&mut app, 5);
        }
        app.world_mut().write_message(AppExit::Success);
        app.update();
        let world = &app.world().resource::<Sim>().world;
        let history = History::open(&dir).unwrap();
        let counts = history.counts().unwrap();
        assert_eq!(history.synced(), world.events_total());
        assert_eq!(
            (counts.events, counts.lost_events),
            (world.events_total(), 0)
        );
        assert_eq!(history.count_kind("Born").unwrap(), world.life.births_total);
        assert_eq!(counts.alive, world.npcs.len() as u64);
        let stats = &app.world().resource::<HistoryDb>().stats;
        eprintln!("{stats:?}");
        drop(history);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn writes_in_the_background_and_reopens_after_a_load() {
        let dir = temp_dir("reopen");
        let mut world = World::generate(8, 6, 60);
        let mut brain = UtilityBrain::new(8);
        let mut db = HistoryDb::default();
        db.open(&dir, &world);
        assert!(db.is_open(), "{:?}", db.error);
        world.run(&mut brain, 3 * MINUTES_PER_DAY);
        db.sync(&world);
        let saved = world.clone();
        world.run(&mut brain, 3 * MINUTES_PER_DAY);
        db.sync(&world);
        db.close();
        assert_eq!(db.stats.batches, 2);
        assert_eq!(db.stats.events, world.events_total());

        // "Caricamento" del salvataggio: il futuro sparisce dallo storico.
        db.open(&dir, &saved);
        assert_eq!(db.next, saved.events_total());
        let events = db.read(|h| h.counts()).unwrap().unwrap().events;
        assert_eq!(events, saved.events_total());
        db.close();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_broken_directory_disables_the_history_without_panicking() {
        let file = temp_dir("broken");
        std::fs::write(&file, b"not a directory").unwrap();
        let world = World::generate(1, 2, 10);
        let mut db = HistoryDb::default();
        db.open(&file, &world);
        assert!(!db.is_open());
        assert!(db.error.is_some());
        db.sync(&world);
        assert!(db.read(|h| h.counts()).is_none());
        std::fs::remove_file(&file).unwrap();
    }
}
