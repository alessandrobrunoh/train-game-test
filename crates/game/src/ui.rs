//! Interfaccia egui: controlli del tempo, ispettore NPC, registro eventi e carrozze.
//!
//! Tutte le finestre sono piccole e ancorate ai bordi dello schermo, così il
//! centro (dove la camera mostra il treno) resta libero.

use bevy::prelude::*;
use bevy_egui::egui::{self, Align2, Color32, RichText};
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use sim::{Action, ActionKind, CarriageKind, EventKind, Npc, NpcId, Stats, World};

use crate::state::{PointerOverUi, SelectedNpc, Sim, SimClock};

/// Velocità selezionabili, in minuti di gioco per secondo reale.
const SPEEDS: [f32; 4] = [1.0, 10.0, 60.0, 600.0];
/// Quanti eventi mostrare nel registro (i più recenti).
const EVENT_LOG_LEN: usize = 50;
/// Ogni quanti secondi reali ricalcolare statistiche e presenze.
const CACHE_REFRESH_SECS: f32 = 0.25;
/// Distanza delle finestre dai bordi dello schermo.
const MARGIN: f32 = 8.0;
/// Larghezza fissa dell'ispettore.
const INSPECTOR_WIDTH: f32 = 300.0;
/// Larghezza del registro eventi.
const EVENT_LOG_WIDTH: f32 = 380.0;
/// Messaggio mostrato finché la risorsa `Sim` non esiste.
const NOT_STARTED: &str = "simulazione non avviata";

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<EguiPlugin>() {
            app.add_plugins(EguiPlugin::default());
        }
        app.init_resource::<UiCache>()
            .add_systems(Update, refresh_cache)
            .add_systems(
                EguiPrimaryContextPass,
                (
                    time_panel,
                    inspector,
                    event_log,
                    carriage_overview,
                    // Per ultimo, dopo che tutte le finestre sono state disegnate.
                    update_pointer_over_ui,
                )
                    .chain(),
            );
    }
}

/// Dati aggregati ricalcolati poche volte al secondo invece che a ogni frame.
#[derive(Resource)]
struct UiCache {
    timer: Timer,
    stats: Option<Stats>,
    /// NPC presenti in ogni carrozza, indicizzato come `World::carriages`.
    population: Vec<usize>,
}

impl Default for UiCache {
    fn default() -> Self {
        Self {
            timer: Timer::from_seconds(CACHE_REFRESH_SECS, TimerMode::Repeating),
            stats: None,
            population: Vec::new(),
        }
    }
}

fn refresh_cache(time: Res<Time<Real>>, sim: Option<Res<Sim>>, mut cache: ResMut<UiCache>) {
    let Some(sim) = sim else {
        if cache.stats.is_some() {
            cache.stats = None;
            cache.population.clear();
        }
        return;
    };
    let due = cache.timer.tick(time.delta()).just_finished();
    if !due && cache.stats.is_some() {
        return;
    }
    let world = &sim.world;
    cache.stats = Some(Stats::of(world));
    let population = &mut cache.population;
    population.clear();
    population.resize(world.carriages.len(), 0);
    for npc in &world.npcs {
        if let Some(count) = population.get_mut(npc.carriage.index()) {
            *count += 1;
        }
    }
}

// ----------------------------------------------------------------------
// Pannello del tempo (in alto al centro)
// ----------------------------------------------------------------------

fn time_panel(
    mut contexts: EguiContexts,
    sim: Option<Res<Sim>>,
    mut clock: ResMut<SimClock>,
    cache: Res<UiCache>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    egui::Window::new("Tempo")
        .title_bar(false)
        .resizable(false)
        .anchor(Align2::CENTER_TOP, [0.0, MARGIN])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                match &sim {
                    Some(sim) => ui.strong(sim.world.clock.to_string()),
                    None => ui.weak(NOT_STARTED),
                };
                ui.separator();
                let label = if clock.paused { "▶ Riprendi" } else { "⏸ Pausa" };
                if ui.button(label).clicked() {
                    clock.paused = !clock.paused;
                }
                ui.separator();
                for speed in SPEEDS {
                    let active = clock.minutes_per_second == speed;
                    let response = ui
                        .selectable_label(active, format!("{speed}x"))
                        .on_hover_text(format!("{speed} minuti di gioco al secondo"));
                    if response.clicked() && !active {
                        clock.minutes_per_second = speed;
                    }
                }
            });
            if let Some(stats) = &cache.stats {
                ui.horizontal(|ui| stats_row(ui, stats));
            }
        });
}

/// Riga di statistiche globali sotto l'orologio.
fn stats_row(ui: &mut egui::Ui, stats: &Stats) {
    ui.label(format!("Popolazione {}", stats.population))
        .on_hover_ui(|ui| {
            for kind in ActionKind::ALL {
                ui.label(format!("{}: {}", kind.name(), stats.count(kind)));
            }
        });
    ui.separator();
    let needs = stats.avg_needs;
    for (name, value) in [
        ("Sazietà", needs.hunger),
        ("Energia", needs.energy),
        ("Socialità", needs.social),
    ] {
        ui.colored_label(need_color(value), format!("{name} {:.0}%", value * 100.0));
    }
    ui.separator();
    ui.label(format!(
        "Cibo {:.0} (mense {:.0})",
        stats.food_total, stats.food_in_mense
    ));
}

// ----------------------------------------------------------------------
// Ispettore dell'NPC selezionato (a destra)
// ----------------------------------------------------------------------

fn inspector(
    mut contexts: EguiContexts,
    sim: Option<Res<Sim>>,
    mut selected: ResMut<SelectedNpc>,
) {
    let Some(id) = selected.0 else {
        return;
    };
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let mut open = true;
    egui::Window::new("Ispettore")
        .anchor(Align2::RIGHT_TOP, [-MARGIN, MARGIN])
        .resizable(false)
        .min_width(INSPECTOR_WIDTH)
        .max_width(INSPECTOR_WIDTH)
        .open(&mut open)
        .show(ctx, |ui| match sim.as_deref() {
            None => {
                ui.weak(NOT_STARTED);
            }
            Some(sim) => match sim.world.npc(id) {
                Some(npc) => npc_details(ui, &sim.world, npc),
                None => missing_npc(ui, &sim.world, id),
            },
        });
    if !open {
        selected.0 = None;
    }
}

fn npc_details(ui: &mut egui::Ui, world: &World, npc: &Npc) {
    let now = world.clock;
    ui.heading(&npc.name);
    ui.label(format!("{} anni · {}", npc.age, npc.id));
    ui.separator();

    let job = match (npc.job, npc.workplace) {
        (Some(job), Some(place)) => format!("{job} in {}", world.carriage_label(place)),
        (Some(job), None) => job.to_string(),
        (None, _) => "nessuno".to_string(),
    };
    field(ui, "Lavoro", &job);
    field(ui, "Casa", &world.carriage_label(npc.home));
    field(ui, "Si trova in", &world.carriage_label(npc.carriage));

    ui.separator();
    field(ui, "Azione", &action_text(world, npc));
    let until = npc.action_until;
    let until_text = if until.day() == now.day() {
        format!("{:02}:{:02}", until.hour(), until.minute())
    } else {
        until.to_string()
    };
    ui.add(egui::ProgressBar::new(npc.action_progress(now)).text(format!(
        "{} rimasti, fino alle {until_text}",
        format_minutes(until.since(now))
    )));

    ui.separator();
    ui.strong("Bisogni");
    for (name, value) in [
        ("Sazietà", npc.needs.hunger),
        ("Energia", npc.needs.energy),
        ("Socialità", npc.needs.social),
    ] {
        ui.add(
            egui::ProgressBar::new(value)
                .fill(need_color(value))
                .text(format!("{name} {:.0}%", value * 100.0)),
        );
    }
    if npc.starving_minutes > 0 {
        ui.colored_label(
            DANGER,
            format!(
                "Sta morendo di fame da {}",
                format_minutes(npc.starving_minutes)
            ),
        );
    }

    ui.separator();
    field(ui, "Gettoni", &npc.inventory.tokens.to_string());

    if let Some(context) = world.npc_context(npc.id) {
        egui::CollapsingHeader::new("Contesto per il cervello")
            .default_open(false)
            .show(ui, |ui| ui.label(context));
    }
}

/// L'NPC selezionato non esiste più: cerca nel registro come è morto.
fn missing_npc(ui: &mut egui::Ui, world: &World, id: NpcId) {
    let death = world.events.iter().rev().find(|event| {
        matches!(&event.kind, EventKind::NpcDied { npc, .. } if *npc == id)
    });
    match death {
        Some(event) => {
            ui.colored_label(DANGER, event.to_string());
        }
        None => {
            ui.label(format!("L'NPC {id} non esiste più."));
        }
    }
    ui.weak("Chiudi l'ispettore per deselezionarlo.");
}

/// Etichetta + valore, che va a capo se il valore è lungo.
fn field(ui: &mut egui::Ui, name: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.weak(format!("{name}:"));
        ui.label(value);
    });
}

/// Descrizione dell'azione corrente, es. "lavora (aiuola 2)".
fn action_text(world: &World, npc: &Npc) -> String {
    let verb = npc.action.kind().name();
    match npc.action {
        Action::Eat(station) | Action::Sleep(station) | Action::Work(station) => {
            match world
                .carriage(npc.carriage)
                .and_then(|c| c.station(station))
            {
                Some(s) => format!("{verb} ({} {})", s.kind.name(), station.0 + 1),
                None => verb.to_string(),
            }
        }
        Action::Travel { to } => format!("{verb} verso {}", world.carriage_label(to)),
        Action::Socialize(other) => match world.npc(other) {
            Some(partner) => format!("{verb} con {}", partner.name),
            None => verb.to_string(),
        },
        Action::Idle => verb.to_string(),
    }
}

// ----------------------------------------------------------------------
// Registro eventi (in basso a sinistra)
// ----------------------------------------------------------------------

fn event_log(mut contexts: EguiContexts, sim: Option<Res<Sim>>) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let total = sim.as_ref().map_or(0, |sim| sim.world.events.len());
    // L'id resta fisso anche se il titolo cambia col numero di eventi.
    egui::Window::new(format!("Eventi ({total})"))
        .id(egui::Id::new("event_log"))
        // Lascia spazio al testo d'aiuto dei comandi in basso a sinistra.
        .anchor(Align2::LEFT_BOTTOM, [MARGIN, -40.0])
        .resizable(false)
        .min_width(EVENT_LOG_WIDTH)
        .max_width(EVENT_LOG_WIDTH)
        .show(ctx, |ui| {
            let Some(sim) = &sim else {
                ui.weak(NOT_STARTED);
                return;
            };
            let events = &sim.world.events;
            if events.is_empty() {
                ui.weak("Nessun evento.");
                return;
            }
            egui::ScrollArea::vertical()
                .max_height(160.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for event in events.iter().rev().take(EVENT_LOG_LEN) {
                        ui.colored_label(event_color(&event.kind), event.to_string());
                    }
                });
        });
}

fn event_color(kind: &EventKind) -> Color32 {
    match kind {
        EventKind::NpcDied { .. } => DANGER,
        EventKind::NpcStarving { .. } => WARNING,
        EventKind::FoodShortage => Color32::from_rgb(230, 200, 80),
        EventKind::FoodRestocked => GOOD,
    }
}

// ----------------------------------------------------------------------
// Elenco carrozze (in alto a sinistra, chiuso di default)
// ----------------------------------------------------------------------

fn carriage_overview(
    mut contexts: EguiContexts,
    sim: Option<Res<Sim>>,
    selected: Res<SelectedNpc>,
    cache: Res<UiCache>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    egui::Window::new("Carrozze")
        .anchor(Align2::LEFT_TOP, [MARGIN, MARGIN])
        .resizable(false)
        .default_open(false)
        .show(ctx, |ui| {
            let Some(sim) = &sim else {
                ui.weak(NOT_STARTED);
                return;
            };
            let world = &sim.world;
            // Evidenzia la carrozza dove si trova l'NPC selezionato.
            let highlighted = selected
                .0
                .and_then(|id| world.npc(id))
                .map(|npc| npc.carriage);
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .show(ui, |ui| {
                    egui::Grid::new("carriage_grid")
                        .num_columns(5)
                        .striped(true)
                        .spacing([12.0, 2.0])
                        .show(ui, |ui| {
                            for header in ["N.", "Nome", "Tipo", "Presenti", "Scorte"] {
                                ui.strong(header);
                            }
                            ui.end_row();
                            for carriage in &world.carriages {
                                let text = |s: String| {
                                    let text = RichText::new(s);
                                    if highlighted == Some(carriage.id) {
                                        text.strong().color(Color32::WHITE)
                                    } else {
                                        text
                                    }
                                };
                                let present = cache
                                    .population
                                    .get(carriage.id.index())
                                    .copied()
                                    .unwrap_or(0);
                                let stock = match carriage.kind {
                                    CarriageKind::Mensa | CarriageKind::Serra => {
                                        format!("{:.0} cibo", carriage.stock.food)
                                    }
                                    CarriageKind::Officina => {
                                        format!("{:.0} mat.", carriage.stock.materials)
                                    }
                                    CarriageKind::Dormitorio => "-".to_string(),
                                };
                                ui.label(text(carriage.id.to_string()));
                                ui.label(text(carriage.name.clone()));
                                ui.label(text(carriage.kind.to_string()));
                                ui.label(text(present.to_string()));
                                ui.label(text(stock));
                                ui.end_row();
                            }
                        });
                });
        });
}

// ----------------------------------------------------------------------
// Input
// ----------------------------------------------------------------------

/// Segnala agli altri sistemi se il puntatore è sopra un pannello egui,
/// così i click sulle finestre non selezionano NPC nel mondo.
fn update_pointer_over_ui(mut contexts: EguiContexts, mut over_ui: ResMut<PointerOverUi>) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let over = ctx.egui_wants_pointer_input() || ctx.is_pointer_over_egui();
    // Scrive solo se cambia, per non segnare la risorsa come modificata ogni frame.
    if over_ui.0 != over {
        over_ui.0 = over;
    }
}

// ----------------------------------------------------------------------
// Utilità
// ----------------------------------------------------------------------

const DANGER: Color32 = Color32::from_rgb(230, 80, 80);
const WARNING: Color32 = Color32::from_rgb(240, 160, 60);
const GOOD: Color32 = Color32::from_rgb(110, 190, 110);

/// Colore di un bisogno in `0..=1` (1 = soddisfatto), con le stesse soglie
/// di `World::npc_context` (critica / bassa / media / buona).
fn need_color(value: f32) -> Color32 {
    match value {
        v if v < 0.2 => DANGER,
        v if v < 0.45 => WARNING,
        v if v < 0.75 => Color32::from_rgb(220, 200, 90),
        _ => GOOD,
    }
}

/// Minuti di gioco in forma leggibile, es. "2h 05m" o "45m".
fn format_minutes(minutes: u64) -> String {
    let (hours, minutes) = (minutes / 60, minutes % 60);
    if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else {
        format!("{minutes}m")
    }
}
