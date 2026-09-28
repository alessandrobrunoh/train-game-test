//! Salvataggi in gioco: tasti, finestra "Partite", autosalvataggi e
//! sostituzione del mondo (formato e cartelle in `save_file.rs`).
//!
//! - F5 salva nello slot "rapido", F9 carica l'ultimo salvataggio della
//!   partita in corso, Esc (o il bottone nel pannello del tempo) apre la
//!   finestra "Partite": elenco delle partite e dei loro salvataggi, Salva con
//!   nome, Carica, Elimina e "Nuova partita".
//! - Ogni `AUTOSAVE_EVERY_DAYS` giorni di gioco un salvataggio automatico
//!   negli slot a rotazione `auto-1..3`.
//! - Il mondo si serializza sul thread principale (qualche ms); compressione
//!   e scrittura su disco vanno in un thread a parte.
//!
//! Tasti, finestra e autosalvataggi accodano comandi in [`SaveQueue`], eseguiti
//! all'inizio del frame successivo (`PreUpdate`, [`WorldSwapSet`]). Caricare o
//! creare una partita sostituisce `Sim`, `PlayerInventory`, la posizione del
//! giocatore, `SimClock` (in pausa dopo un caricamento) e `RunInfo`, azzera
//! selezione e "Segui" e invia `WorldReplaced`: i moduli che tengono dati
//! derivati dal mondo li ricostruiscono in [`WorldRebuildSet`], subito dopo e
//! nello stesso frame (treno, postazioni, magazzini, sprite, grafici, notifiche).

use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::Instant;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy_egui::egui::{self, Align2, Color32, RichText};
use bevy_egui::input::EguiWantsInput;
use bevy_egui::{EguiContexts, EguiPreUpdateSet, EguiPrimaryContextPass};
use sim::{UtilityBrain, World};

use crate::player::{Body, Player, start_position};
use crate::save_file::{
    self, EncodedSave, RunEntry, SaveBodyRef, SaveFile, SaveHeader, create_run, format_age,
    format_utc, latest_save, list_runs, next_auto_slot, now_ms, run_of_save, sanitize_slot,
    slot_path,
};
use crate::sim_bridge::{SIM_CARRIAGES, SIM_NPCS, SIM_SEED, SimTickSet};
use crate::state::{
    FollowNpc, PlayerInventory, RunInfo, SelectedNpc, Sim, SimClock, WorldReplaced,
};
use crate::train::TrainLayout;
use crate::ui::PointerCheck;

/// Slot del salvataggio rapido (F5).
pub const QUICK_SLOT: &str = "rapido";
/// Giorni di gioco tra due salvataggi automatici (12 = un anno).
pub const AUTOSAVE_EVERY_DAYS: u64 = 12;
/// Limiti della nuova partita.
pub const MAX_CARRIAGES: usize = 60;
pub const MAX_NPCS: usize = 3000;
/// Per quanti secondi reali resta visibile l'avviso (errori più a lungo).
const NOTICE_SECS: f64 = 3.0;
const ERROR_NOTICE_SECS: f64 = 8.0;
const WINDOW_WIDTH: f32 = 560.0;
const ERROR_COLOR: Color32 = Color32::from_rgb(255, 120, 110);
const OK_COLOR: Color32 = Color32::from_rgb(170, 230, 150);

/// Sostituzione del mondo (caricamenti, nuova partita), in `PreUpdate`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorldSwapSet;

/// Ricostruzione dello stato derivato dal mondo dopo `WorldReplaced`, in
/// `PreUpdate` subito dopo [`WorldSwapSet`].
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorldRebuildSet;

#[derive(Default)]
pub struct SavesPlugin {
    /// Cartella dati; `None` = `$TRAINGAME_DATA_DIR` o quella della piattaforma.
    pub data_dir: Option<PathBuf>,
}

impl Plugin for SavesPlugin {
    fn build(&self, app: &mut App) {
        let data_dir = self
            .data_dir
            .clone()
            .unwrap_or_else(save_file::default_data_dir);
        // La run della partita creata da `SimBridgePlugin`, già qui (non in
        // Startup): altri plugin la leggono all'avvio.
        let run = new_run_info(&data_dir, SIM_SEED);
        info!("Partita {} in {}", run.run_id, run.dir.display());
        app.insert_resource(run)
            .insert_resource(SaveConfig { data_dir })
            .init_resource::<SaveQueue>()
            .init_resource::<SaveTasks>()
            .init_resource::<Autosave>()
            .init_resource::<SaveNotice>()
            .init_resource::<SavesWindow>()
            .configure_sets(PreUpdate, (WorldSwapSet, WorldRebuildSet).chain())
            .add_systems(
                PreUpdate,
                (
                    absorb_keys_while_typing.after(EguiPreUpdateSet::ProcessInput),
                    process_commands.in_set(WorldSwapSet),
                ),
            )
            .add_systems(
                Update,
                (save_shortcuts, autosave.after(SimTickSet), poll_save_tasks),
            )
            .add_systems(
                EguiPrimaryContextPass,
                (saves_window, save_notice).before(PointerCheck),
            );
    }
}

/// Crea la cartella di una nuova run; se non si può, la run resta senza
/// cartella (i salvataggi daranno errore, il gioco no).
fn new_run_info(data_dir: &Path, seed: u64) -> RunInfo {
    let secs = now_ms() / 1000;
    match create_run(data_dir, seed, secs) {
        Ok((run_id, dir)) => RunInfo { run_id, dir },
        Err(e) => {
            warn!("Impossibile creare la cartella della partita: {e}");
            let run_id = save_file::run_id(seed, secs);
            let dir = save_file::runs_dir(data_dir).join(&run_id);
            RunInfo { run_id, dir }
        }
    }
}

#[derive(Resource, Debug)]
pub struct SaveConfig {
    pub data_dir: PathBuf,
}

/// Cosa fare all'inizio del prossimo frame.
#[derive(Clone, Debug, PartialEq)]
pub enum SaveCommand {
    Save {
        slot: String,
    },
    Autosave,
    /// L'ultimo salvataggio della partita in corso.
    QuickLoad,
    Load(PathBuf),
    NewGame {
        seed: u64,
        carriages: usize,
        npcs: usize,
    },
    DeleteSave(PathBuf),
    DeleteRun(PathBuf),
}

#[derive(Resource, Default, Debug)]
pub struct SaveQueue(pub Vec<SaveCommand>);

/// Salvataggi in scrittura su un altro thread.
#[derive(Resource, Default)]
struct SaveTasks(Vec<PendingSave>);

struct PendingSave {
    slot: String,
    auto: bool,
    started: Instant,
    /// Byte scritti o errore.
    handle: JoinHandle<Result<usize, String>>,
}

/// Giorno di gioco del prossimo salvataggio automatico (`None`: da fissare).
#[derive(Resource, Default, Debug)]
struct Autosave {
    next_day: Option<u64>,
}

/// Avviso in basso ("Partita salvata" o un errore).
#[derive(Resource, Default, Debug)]
struct SaveNotice(Option<Notice>);

#[derive(Debug)]
struct Notice {
    text: String,
    error: bool,
    shown_at: f64,
}

impl SaveNotice {
    fn show(&mut self, now: f64, text: impl Into<String>, error: bool) {
        let text = text.into();
        if error {
            warn!("{text}");
        } else {
            info!("{text}");
        }
        self.0 = Some(Notice {
            text,
            error,
            shown_at: now,
        });
    }
}

/// Finestra "Partite" (Esc).
#[derive(Resource)]
pub(crate) struct SavesWindow {
    pub(crate) open: bool,
    slot: String,
    seed: u64,
    carriages: usize,
    npcs: usize,
    /// Eliminazione in attesa di conferma (file o cartella).
    confirm: Option<PathBuf>,
    listing: Vec<RunEntry>,
    /// L'elenco va riletto dal disco.
    dirty: bool,
}

impl SavesWindow {
    /// Rilegge l'elenco dal disco e annulla le conferme in sospeso.
    pub(crate) fn refresh(&mut self) {
        self.dirty = true;
        self.confirm = None;
    }
}

impl Default for SavesWindow {
    fn default() -> Self {
        Self {
            open: false,
            slot: "manuale".to_string(),
            seed: SIM_SEED,
            carriages: SIM_CARRIAGES,
            npcs: SIM_NPCS,
            confirm: None,
            listing: Vec::new(),
            dirty: true,
        }
    }
}

// --- Dati puri: dal file (o da un seme) alla partita ----------------------------

/// Tutto ciò che sostituisce la partita in corso.
pub struct LoadedGame {
    pub world: World,
    pub brain: UtilityBrain,
    pub player: Vec2,
    pub inventory: PlayerInventory,
    pub clock: SimClock,
    pub run: RunInfo,
}

impl LoadedGame {
    /// Partita da un salvataggio della run `run` (in pausa, alla velocità salvata).
    pub fn from_save(file: SaveFile, run: RunInfo) -> Self {
        let body = file.body;
        Self {
            world: body.world,
            brain: body.brain,
            player: Vec2::from_array(body.player),
            inventory: PlayerInventory {
                tokens: body.tokens,
                items: body.items,
            },
            clock: SimClock {
                paused: true,
                minutes_per_second: body.minutes_per_second,
                accumulator: 0.0,
            },
            run,
        }
    }

    /// Nuova partita (seme, carrozze e persone entro i limiti).
    pub fn new_game(seed: u64, carriages: usize, npcs: usize, run: RunInfo) -> Self {
        let carriages = carriages.clamp(1, MAX_CARRIAGES);
        let npcs = npcs.clamp(1, MAX_NPCS);
        Self {
            world: World::generate(seed, carriages, npcs),
            brain: UtilityBrain::new(seed),
            player: start_position(),
            inventory: PlayerInventory::default(),
            clock: SimClock::default(),
            run,
        }
    }
}

/// Serializza la partita (thread principale): il resto lo fa [`EncodedSave::finish`].
pub fn encode_game(
    run_id: &str,
    slot: &str,
    sim: &Sim,
    player: Vec2,
    inventory: &PlayerInventory,
    clock: &SimClock,
) -> Result<EncodedSave, save_file::SaveError> {
    let header = SaveHeader::of(run_id, slot, now_ms(), &sim.world);
    let body = SaveBodyRef {
        world: &sim.world,
        brain: &sim.brain,
        player: player.to_array(),
        tokens: inventory.tokens,
        items: inventory.items,
        minutes_per_second: clock.minutes_per_second,
        paused: clock.paused,
    };
    save_file::encode(&header, &body)
}

// --- Sistemi --------------------------------------------------------------------

/// Mentre si scrive in un campo di testo egui i tasti non vanno al gioco
/// (altrimenti "p" metterebbe in pausa, "1" cambierebbe velocità...).
fn absorb_keys_while_typing(
    wants: Option<Res<EguiWantsInput>>,
    keys: Option<ResMut<ButtonInput<KeyCode>>>,
) {
    if let (Some(wants), Some(mut keys)) = (wants, keys)
        && wants.wants_keyboard_input()
    {
        keys.reset_all();
    }
}

/// F5 salva, F9 carica l'ultimo salvataggio, Esc apre/chiude "Partite".
fn save_shortcuts(
    keys: Res<ButtonInput<KeyCode>>,
    mut queue: ResMut<SaveQueue>,
    mut window: ResMut<SavesWindow>,
) {
    if keys.just_pressed(KeyCode::F5) {
        queue.0.push(SaveCommand::Save {
            slot: QUICK_SLOT.to_string(),
        });
    }
    if keys.just_pressed(KeyCode::F9) {
        queue.0.push(SaveCommand::QuickLoad);
    }
    if keys.just_pressed(KeyCode::Escape) {
        window.open = !window.open;
        window.refresh();
    }
}

/// Un salvataggio automatico ogni `AUTOSAVE_EVERY_DAYS` giorni di gioco.
fn autosave(sim: Res<Sim>, mut auto: ResMut<Autosave>, mut queue: ResMut<SaveQueue>) {
    let day = sim.world.clock.day();
    let next = *auto.next_day.get_or_insert(day + AUTOSAVE_EVERY_DAYS);
    if day >= next {
        auto.next_day = Some(day + AUTOSAVE_EVERY_DAYS);
        queue.0.push(SaveCommand::Autosave);
    }
}

/// Le risorse della partita che un caricamento sostituisce.
#[derive(SystemParam)]
struct GameAccess<'w, 's> {
    sim: ResMut<'w, Sim>,
    inventory: ResMut<'w, PlayerInventory>,
    clock: ResMut<'w, SimClock>,
    run: ResMut<'w, RunInfo>,
    selected: ResMut<'w, SelectedNpc>,
    follow: ResMut<'w, FollowNpc>,
    autosave: ResMut<'w, Autosave>,
    replaced: MessageWriter<'w, WorldReplaced>,
    player: Query<'w, 's, (&'static mut Body, &'static mut Transform), With<Player>>,
    camera: Query<'w, 's, &'static mut Transform, (With<Camera2d>, Without<Player>)>,
}

impl GameAccess<'_, '_> {
    fn player_position(&self) -> Vec2 {
        self.player
            .single()
            .map_or_else(|_| start_position(), |(body, _)| body.position)
    }

    /// Sostituisce la partita in corso e avvisa gli altri moduli.
    fn replace(&mut self, game: LoadedGame) {
        // Dentro il treno nuovo, anche se ha meno carrozze.
        let (left, right) = TrainLayout::from_world(&game.world).inner_bounds();
        let player = Vec2::new(game.player.x.clamp(left + 8.0, right - 8.0), game.player.y);
        self.sim.world = game.world;
        self.sim.brain = game.brain;
        *self.inventory = game.inventory;
        *self.clock = game.clock;
        *self.run = game.run;
        self.selected.0 = None;
        self.follow.0 = false;
        self.autosave.next_day = None;
        if let Ok((mut body, mut transform)) = self.player.single_mut() {
            body.teleport(player);
            transform.translation.x = player.x;
            transform.translation.y = player.y;
        }
        // La camera salta subito sul giocatore invece di attraversare il treno.
        if let Ok(mut camera) = self.camera.single_mut() {
            camera.translation.x = player.x;
        }
        self.replaced.write(WorldReplaced);
    }
}

/// Esegue i comandi accodati: salvataggi (serializzati qui, scritti su un
/// altro thread), caricamenti, nuove partite ed eliminazioni.
fn process_commands(
    mut queue: ResMut<SaveQueue>,
    config: Res<SaveConfig>,
    time: Res<Time<Real>>,
    mut tasks: ResMut<SaveTasks>,
    mut notice: ResMut<SaveNotice>,
    mut window: ResMut<SavesWindow>,
    mut game: GameAccess,
) {
    if queue.0.is_empty() {
        return;
    }
    let now = time.elapsed_secs_f64();
    for command in std::mem::take(&mut queue.0) {
        match command {
            SaveCommand::Save { slot } => {
                start_save(&mut game, &mut tasks, &mut notice, now, slot, false)
            }
            SaveCommand::Autosave => {
                let slot = next_auto_slot(&game.run.dir);
                start_save(&mut game, &mut tasks, &mut notice, now, slot, true);
            }
            SaveCommand::QuickLoad => {
                finish_all(&mut tasks, &mut notice, now);
                match latest_save(&game.run.dir) {
                    Some(save) => load(&mut game, &mut notice, now, &save.path),
                    None => notice.show(
                        now,
                        "Nessun salvataggio per questa partita (F5 per salvare)",
                        true,
                    ),
                }
            }
            SaveCommand::Load(path) => {
                finish_all(&mut tasks, &mut notice, now);
                load(&mut game, &mut notice, now, &path);
            }
            SaveCommand::NewGame {
                seed,
                carriages,
                npcs,
            } => {
                let run = new_run_info(&config.data_dir, seed);
                let loaded = LoadedGame::new_game(seed, carriages, npcs, run);
                let (c, n) = (loaded.world.carriages.len(), loaded.world.npcs.len());
                game.replace(loaded);
                notice.show(
                    now,
                    format!("Nuova partita: seme {seed}, {c} carrozze, {n} persone"),
                    false,
                );
            }
            SaveCommand::DeleteSave(path) => match std::fs::remove_file(&path) {
                Ok(()) => notice.show(now, "Salvataggio eliminato", false),
                Err(e) => notice.show(now, format!("Eliminazione non riuscita: {e}"), true),
            },
            SaveCommand::DeleteRun(dir) => {
                if dir == game.run.dir {
                    notice.show(now, "Non si può eliminare la partita in corso", true);
                } else {
                    match save_file::delete_run(&config.data_dir, &dir) {
                        Ok(()) => notice.show(now, "Partita eliminata", false),
                        Err(e) => notice.show(now, format!("Eliminazione non riuscita: {e}"), true),
                    }
                }
            }
        }
    }
    window.dirty = true;
}

/// Serializza ora, comprime e scrive in un altro thread.
fn start_save(
    game: &mut GameAccess,
    tasks: &mut SaveTasks,
    notice: &mut SaveNotice,
    now: f64,
    slot: String,
    auto: bool,
) {
    let started = Instant::now();
    let player = game.player_position();
    let encoded = encode_game(
        &game.run.run_id,
        &slot,
        &game.sim,
        player,
        &game.inventory,
        &game.clock,
    );
    let encoded = match encoded {
        Ok(encoded) => encoded,
        Err(e) => {
            notice.show(now, format!("Salvataggio non riuscito: {e}"), true);
            return;
        }
    };
    debug!(
        "Salvataggio {slot}: {} byte serializzati in {:.1} ms",
        encoded.raw_len(),
        started.elapsed().as_secs_f64() * 1000.0
    );
    let path = slot_path(&game.run.dir, &slot);
    let handle = std::thread::spawn(move || {
        let bytes = encoded.finish();
        save_file::write_atomic(&path, &bytes)
            .map(|()| bytes.len())
            .map_err(|e| format!("{e} ({})", path.display()))
    });
    tasks.0.push(PendingSave {
        slot,
        auto,
        started,
        handle,
    });
}

fn load(game: &mut GameAccess, notice: &mut SaveNotice, now: f64, path: &Path) {
    let started = Instant::now();
    let file = match save_file::read_save(path) {
        Ok(file) => file,
        Err(e) => {
            notice.show(now, format!("Caricamento non riuscito: {e}"), true);
            return;
        }
    };
    let (run_id, dir) =
        run_of_save(path).unwrap_or_else(|| (file.header.run_id.clone(), game.run.dir.clone()));
    let label = format!("{} ({})", file.header.game_time_label(), file.header.slot);
    game.replace(LoadedGame::from_save(file, RunInfo { run_id, dir }));
    info!(
        "Caricato {} in {:.1} ms",
        path.display(),
        started.elapsed().as_secs_f64() * 1000.0
    );
    notice.show(
        now,
        format!("Partita caricata: {label} · in pausa, P per riprendere"),
        false,
    );
}

/// Aspetta i salvataggi in corso (prima di caricare: magari è proprio quello).
fn finish_all(tasks: &mut SaveTasks, notice: &mut SaveNotice, now: f64) {
    for save in tasks.0.drain(..) {
        report(save, notice, now);
    }
}

fn report(save: PendingSave, notice: &mut SaveNotice, now: f64) {
    let (slot, auto, started) = (save.slot, save.auto, save.started);
    match save.handle.join() {
        Ok(Ok(bytes)) => {
            debug!(
                "Salvataggio {slot}: {bytes} byte in {:.1} ms",
                started.elapsed().as_secs_f64() * 1000.0
            );
            let text = if auto {
                format!("Salvataggio automatico ({slot})")
            } else {
                format!("Partita salvata ({slot})")
            };
            notice.show(now, text, false);
        }
        Ok(Err(e)) => notice.show(now, format!("Salvataggio non riuscito: {e}"), true),
        Err(_) => notice.show(now, "Salvataggio non riuscito: errore interno", true),
    }
}

/// Raccoglie i salvataggi finiti.
fn poll_save_tasks(
    time: Res<Time<Real>>,
    mut tasks: ResMut<SaveTasks>,
    mut notice: ResMut<SaveNotice>,
    mut window: ResMut<SavesWindow>,
) {
    if !tasks.0.iter().any(|t| t.handle.is_finished()) {
        return;
    }
    let now = time.elapsed_secs_f64();
    let (done, pending): (Vec<_>, Vec<_>) = std::mem::take(&mut tasks.0)
        .into_iter()
        .partition(|t| t.handle.is_finished());
    tasks.0 = pending;
    for save in done {
        report(save, &mut notice, now);
    }
    window.dirty = true;
}

// --- Interfaccia ---------------------------------------------------------------

/// Avviso in basso al centro, sopra la riga della carrozza.
fn save_notice(mut contexts: EguiContexts, time: Res<Time<Real>>, mut notice: ResMut<SaveNotice>) {
    let Some(current) = &notice.0 else {
        return;
    };
    let age = time.elapsed_secs_f64() - current.shown_at;
    let duration = if current.error {
        ERROR_NOTICE_SECS
    } else {
        NOTICE_SECS
    };
    if age > duration {
        notice.0 = None;
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let color = if current.error { ERROR_COLOR } else { OK_COLOR };
    egui::Area::new(egui::Id::new("save_notice"))
        .anchor(Align2::CENTER_BOTTOM, [0.0, -72.0])
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(ctx, |ui| {
            ui.multiply_opacity(((duration - age) / 0.5).clamp(0.0, 1.0) as f32);
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.colored_label(color, &current.text);
            });
        });
}

/// Finestra "Partite": salva con nome, elenco di partite e salvataggi, nuova partita.
fn saves_window(
    mut contexts: EguiContexts,
    mut window: ResMut<SavesWindow>,
    config: Res<SaveConfig>,
    run: Res<RunInfo>,
    sim: Res<Sim>,
    tasks: Res<SaveTasks>,
    mut queue: ResMut<SaveQueue>,
) {
    if !window.open {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    if window.dirty {
        window.listing = list_runs(&config.data_dir);
        window.dirty = false;
    }
    let window = &mut *window;
    let mut open = true;
    let center = ctx.content_rect().center();
    let now = now_ms();
    egui::Window::new("Partite")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_pos(center - egui::vec2(WINDOW_WIDTH / 2.0, 240.0))
        .default_width(WINDOW_WIDTH)
        .show(ctx, |ui| {
            ui.set_width(WINDOW_WIDTH);
            current_game(
                ui,
                window,
                &run,
                &sim.world,
                !tasks.0.is_empty(),
                &mut queue,
            );
            ui.separator();
            ui.strong("Salvataggi");
            egui::ScrollArea::vertical()
                .max_height(300.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let runs = std::mem::take(&mut window.listing);
                    let mut shown = 0;
                    for entry in &runs {
                        let current = entry.dir == run.dir;
                        if entry.saves.is_empty() && !current {
                            continue;
                        }
                        shown += 1;
                        run_section(ui, window, entry, current, now, &mut queue);
                    }
                    if shown == 0 {
                        ui.weak("Nessun salvataggio.");
                    }
                    window.listing = runs;
                });
            ui.separator();
            new_game(ui, window, &mut queue);
            ui.add_space(4.0);
            ui.weak(format!("Cartella: {}", config.data_dir.display()));
        });
    if !open {
        window.open = false;
        window.confirm = None;
    }
}

/// Partita in corso e "Salva con nome".
fn current_game(
    ui: &mut egui::Ui,
    window: &mut SavesWindow,
    run: &RunInfo,
    world: &World,
    saving: bool,
    queue: &mut SaveQueue,
) {
    let summary = SaveHeader::of(&run.run_id, "", 0, world);
    ui.horizontal(|ui| {
        ui.strong("Partita in corso");
        ui.label(format!(
            "{} · popolazione {}",
            summary.game_time_label(),
            summary.population
        ));
    });
    ui.weak(run_title(&run.run_id));
    ui.horizontal(|ui| {
        ui.label("Nome:");
        ui.add(
            egui::TextEdit::singleline(&mut window.slot)
                .desired_width(160.0)
                .char_limit(save_file::MAX_SLOT_CHARS),
        );
        let slot = sanitize_slot(&window.slot);
        let response = ui.add_enabled(slot.is_some(), egui::Button::new("Salva"));
        if let Some(slot) = slot {
            let exists = slot_path(&run.dir, &slot).exists();
            let response = response.on_hover_text(if exists {
                format!("Sovrascrive «{slot}»")
            } else {
                format!("Salva come «{slot}»")
            });
            if response.clicked() {
                queue.0.push(SaveCommand::Save { slot });
            }
        }
        if saving {
            ui.spinner();
        }
    });
    ui.weak("F5: salvataggio rapido · F9: carica l'ultimo salvataggio · Esc: chiudi");
}

/// "Seme 42 · 28/09/2026 14:03 UTC" dall'id della run.
fn run_title(run_id: &str) -> String {
    match save_file::parse_run_id(run_id) {
        Some((seed, secs)) => format!("Seme {seed} · iniziata il {}", format_utc(secs * 1000)),
        None => run_id.to_string(),
    }
}

/// Una partita con i suoi salvataggi.
fn run_section(
    ui: &mut egui::Ui,
    window: &mut SavesWindow,
    entry: &RunEntry,
    current: bool,
    now: u64,
    queue: &mut SaveQueue,
) {
    let mut title = run_title(&entry.run_id);
    if current {
        title = format!("{title} (in corso)");
    }
    egui::CollapsingHeader::new(RichText::new(title).strong())
        .id_salt(&entry.run_id)
        .default_open(current)
        .show(ui, |ui| {
            if entry.saves.is_empty() {
                ui.weak("Nessun salvataggio.");
            }
            egui::Grid::new(("saves", &entry.run_id))
                .num_columns(5)
                .spacing([10.0, 4.0])
                .striped(true)
                .show(ui, |ui| {
                    for save in &entry.saves {
                        ui.label(&save.slot);
                        match &save.header {
                            Ok(header) => {
                                ui.label(header.game_time_label());
                                ui.label(format!("{} persone", header.population))
                                    .on_hover_text(format!("{} carrozze", header.carriages));
                                ui.weak(format_age(now, header.created_ms))
                                    .on_hover_text(format_utc(header.created_ms));
                            }
                            Err(e) => {
                                ui.colored_label(ERROR_COLOR, "illeggibile")
                                    .on_hover_text(e);
                                ui.label("");
                                ui.label("");
                            }
                        }
                        ui.horizontal(|ui| {
                            if save.header.is_ok() && ui.button("Carica").clicked() {
                                queue.0.push(SaveCommand::Load(save.path.clone()));
                            }
                            confirm_delete(
                                ui,
                                window,
                                &save.path,
                                "Elimina",
                                || SaveCommand::DeleteSave(save.path.clone()),
                                queue,
                            );
                        });
                        ui.end_row();
                    }
                });
            if !current {
                confirm_delete(
                    ui,
                    window,
                    &entry.dir,
                    "Elimina partita",
                    || SaveCommand::DeleteRun(entry.dir.clone()),
                    queue,
                );
            }
        });
}

/// Bottone di eliminazione con conferma ("Sicuro? Sì / No").
fn confirm_delete(
    ui: &mut egui::Ui,
    window: &mut SavesWindow,
    path: &Path,
    label: &str,
    command: impl FnOnce() -> SaveCommand,
    queue: &mut SaveQueue,
) {
    if window.confirm.as_deref() == Some(path) {
        ui.horizontal(|ui| {
            ui.colored_label(ERROR_COLOR, "Sicuro?");
            if ui.button("Sì").clicked() {
                queue.0.push(command());
                window.confirm = None;
            }
            if ui.button("No").clicked() {
                window.confirm = None;
            }
        });
    } else if ui.button(label).clicked() {
        window.confirm = Some(path.to_path_buf());
    }
}

/// Seme, carrozze e persone della nuova partita.
fn new_game(ui: &mut egui::Ui, window: &mut SavesWindow, queue: &mut SaveQueue) {
    ui.strong("Nuova partita");
    egui::Grid::new("new_game")
        .num_columns(2)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            ui.label("Seme");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut window.seed).speed(1.0));
                if ui.button("Casuale").clicked() {
                    window.seed = random_seed();
                }
            });
            ui.end_row();
            ui.label("Carrozze");
            ui.add(egui::DragValue::new(&mut window.carriages).range(1..=MAX_CARRIAGES));
            ui.end_row();
            ui.label("Persone");
            ui.add(
                egui::DragValue::new(&mut window.npcs)
                    .range(1..=MAX_NPCS)
                    .speed(5.0),
            );
            ui.end_row();
        });
    ui.horizontal(|ui| {
        if ui.button("Crea nuova partita").clicked() {
            queue.0.push(SaveCommand::NewGame {
                seed: window.seed,
                carriages: window.carriages,
                npcs: window.npcs,
            });
        }
        ui.weak("I progressi non salvati della partita in corso andranno persi.");
    });
}

/// Seme dall'orologio (splitmix64), solo per il bottone "Casuale".
fn random_seed() -> u64 {
    let mut z = now_ms().wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) % 1_000_000
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::save_file::tests::{TempDir, world_bytes};
    use crate::stations::StationLayout;
    use crate::train::Carriage;
    use sim::MINUTES_PER_DAY;

    fn brain_bytes(brain: &UtilityBrain) -> Vec<u8> {
        postcard::to_allocvec(brain).unwrap()
    }

    fn save_bytes(sim: &Sim, slot: &str) -> Vec<u8> {
        let clock = SimClock::default();
        encode_game(
            "seed3-1",
            slot,
            sim,
            Vec2::new(100.0, 12.0),
            &PlayerInventory::default(),
            &clock,
        )
        .unwrap()
        .finish()
    }

    #[test]
    fn save_load_continue_matches_continuing_without_saving() {
        let mut sim = Sim {
            world: World::generate(3, 12, 150),
            brain: UtilityBrain::new(3),
        };
        sim.world.run(&mut sim.brain, 3 * MINUTES_PER_DAY + 321);
        let bytes = save_bytes(&sim, "rapido");
        let run = RunInfo {
            run_id: "seed3-1".into(),
            dir: PathBuf::from("/tmp/x"),
        };
        let mut loaded = LoadedGame::from_save(save_file::decode(&bytes).unwrap(), run);
        assert!(loaded.clock.paused);
        assert_eq!(loaded.player, Vec2::new(100.0, 12.0));
        assert_eq!(loaded.inventory.tokens, PlayerInventory::default().tokens);

        // Si prosegue su entrambi (a cavallo di una mezzanotte, con nascite e morti).
        let ticks = 2 * MINUTES_PER_DAY + 17;
        sim.world.run(&mut sim.brain, ticks);
        loaded.world.run(&mut loaded.brain, ticks);
        assert_eq!(world_bytes(&loaded.world), world_bytes(&sim.world));
        assert_eq!(brain_bytes(&loaded.brain), brain_bytes(&sim.brain));
    }

    /// Dimensioni e tempi di un salvataggio del treno di default (400 persone)
    /// dopo qualche anno: `cargo test -p game save_size -- --nocapture`.
    #[test]
    fn save_size_and_timing_after_a_few_years() {
        let mut sim = Sim {
            world: World::generate(SIM_SEED, SIM_CARRIAGES, SIM_NPCS),
            brain: UtilityBrain::new(SIM_SEED),
        };
        let start = Instant::now();
        sim.world
            .run(&mut sim.brain, 3 * AUTOSAVE_EVERY_DAYS * MINUTES_PER_DAY);
        let sim_secs = start.elapsed().as_secs_f64();
        let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;

        let t = Instant::now();
        let encoded = encode_game(
            "x",
            "y",
            &sim,
            Vec2::ZERO,
            &PlayerInventory::default(),
            &SimClock::default(),
        )
        .unwrap();
        let encode_ms = ms(t);
        let raw = encoded.raw_len();
        let t = Instant::now();
        let bytes = encoded.finish();
        let compress_ms = ms(t);
        let t = Instant::now();
        let file = save_file::decode(&bytes).unwrap();
        let decode_ms = ms(t);
        assert_eq!(world_bytes(&file.body.world), world_bytes(&sim.world));
        println!(
            "3 anni simulati in {sim_secs:.1} s; {} persone, {} eventi in memoria\n\
             serializzazione {encode_ms:.2} ms ({raw} byte), compressione {compress_ms:.2} ms \
             ({} byte, {:.0}%), lettura {decode_ms:.2} ms",
            sim.world.npcs.len(),
            sim.world.events.len(),
            bytes.len(),
            bytes.len() as f64 / raw as f64 * 100.0,
        );
    }

    /// App senza finestra con i plugin che dipendono dalla forma del treno.
    fn test_app(data: &Path) -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::input::InputPlugin,
            crate::state::StatePlugin,
            crate::sim_bridge::SimBridgePlugin,
            SavesPlugin {
                data_dir: Some(data.to_path_buf()),
            },
            crate::train::TrainPlugin,
            crate::stations::StationsPlugin,
            crate::storage::StoragePlugin,
            crate::population::PopulationPlugin,
        ));
        app.finish();
        app.cleanup();
        app.update();
        app
    }

    fn carriage_entities(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<(), With<Carriage>>()
            .iter(app.world())
            .count()
    }

    fn run_command(app: &mut App, command: SaveCommand) {
        app.world_mut().resource_mut::<SaveQueue>().0.push(command);
        app.update();
    }

    /// Aspetta che i salvataggi in corso siano scritti.
    fn wait_saves(app: &mut App) {
        for _ in 0..500 {
            if app.world().resource::<SaveTasks>().0.is_empty() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
            app.update();
        }
        panic!("salvataggio mai finito");
    }

    #[test]
    fn loading_a_train_of_another_length_rebuilds_it() {
        let tmp = TempDir::new("rebuild");
        let mut app = test_app(&tmp.0);
        let first_run = app.world().resource::<RunInfo>().clone();
        assert!(first_run.dir.is_dir(), "la run nasce all'avvio");
        assert_eq!(carriage_entities(&mut app), SIM_CARRIAGES);

        // Nuova partita con 7 carrozze: tutto si ricostruisce nello stesso frame.
        run_command(
            &mut app,
            SaveCommand::NewGame {
                seed: 9,
                carriages: 7,
                npcs: 60,
            },
        );
        let small_run = app.world().resource::<RunInfo>().clone();
        assert_ne!(small_run.dir, first_run.dir);
        assert!(small_run.run_id.starts_with("seed9-"));
        assert_eq!(app.world().resource::<TrainLayout>().len(), 7);
        assert_eq!(app.world().resource::<StationLayout>().carriages.len(), 7);
        assert_eq!(carriage_entities(&mut app), 7);

        // Si va avanti un po' e si salva.
        {
            let mut sim = app.world_mut().resource_mut::<Sim>();
            let Sim { world, brain } = &mut *sim;
            world.run(brain, MINUTES_PER_DAY + 5);
        }
        run_command(
            &mut app,
            SaveCommand::Save {
                slot: "sette".into(),
            },
        );
        wait_saves(&mut app);
        let path = save_file::slot_path(&small_run.dir, "sette");
        assert!(path.is_file());
        let saved = world_bytes(&app.world().resource::<Sim>().world);

        // Un'altra partita più lunga, poi si ricarica quella da 7.
        run_command(
            &mut app,
            SaveCommand::NewGame {
                seed: 1,
                carriages: 25,
                npcs: 100,
            },
        );
        assert_eq!(carriage_entities(&mut app), 25);
        assert_eq!(app.world().resource::<TrainLayout>().len(), 25);
        run_command(&mut app, SaveCommand::Load(path));
        assert_eq!(carriage_entities(&mut app), 7);
        assert_eq!(app.world().resource::<TrainLayout>().len(), 7);
        assert_eq!(app.world().resource::<StationLayout>().carriages.len(), 7);
        assert_eq!(world_bytes(&app.world().resource::<Sim>().world), saved);
        assert_eq!(app.world().resource::<RunInfo>().dir, small_run.dir);
        assert!(app.world().resource::<SimClock>().paused);

        // F9: l'ultimo salvataggio della run in corso.
        run_command(
            &mut app,
            SaveCommand::NewGame {
                seed: 2,
                carriages: 3,
                npcs: 10,
            },
        );
        run_command(&mut app, SaveCommand::QuickLoad);
        assert!(
            app.world()
                .resource::<SaveNotice>()
                .0
                .as_ref()
                .unwrap()
                .error
        );
        assert_eq!(carriage_entities(&mut app), 3);
    }
}
