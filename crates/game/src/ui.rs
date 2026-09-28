//! Interfaccia egui: controlli del tempo, ispettore NPC, registro eventi e carrozze.
//!
//! Tutte le finestre sono piccole e ancorate ai bordi dello schermo, così il
//! centro (dove la camera mostra il treno) resta libero.

use bevy::prelude::*;
use bevy_egui::egui::{self, Align2, Color32, RichText};
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use sim::{Action, ActionKind, Carriage, EventKind, ItemKind, Npc, NpcId, Stats, World};

use crate::state::{PointerOverUi, SelectedNpc, Sim, SimClock};
use crate::storage::{item_color, plural_title, storable_items};

/// Velocità selezionabili, in minuti di gioco per secondo reale.
const SPEEDS: [f32; 4] = [1.0, 10.0, 60.0, 600.0];
/// Quanti eventi mostrare nel registro (i più recenti).
const EVENT_LOG_LEN: usize = 50;
/// Ogni quanti secondi reali ricalcolare statistiche e presenze.
const CACHE_REFRESH_SECS: f32 = 0.25;
/// Distanza delle finestre dai bordi dello schermo.
pub(crate) const MARGIN: f32 = 8.0;
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
                    update_pointer_over_ui.in_set(PointerCheck),
                )
                    .chain(),
            );
    }
}

/// Il controllo "puntatore sopra egui": le finestre di altri moduli vanno
/// disegnate prima (`.before(PointerCheck)`).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct PointerCheck;

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
                let label = if clock.paused {
                    "▶ Riprendi"
                } else {
                    "⏸ Pausa"
                };
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
        "Razioni {:.0} · Attrezzi in vendita {} · Vestiti in vendita {} · Gettoni {}",
        stats.stored.get(ItemKind::Razione),
        stats.on_sale.count(ItemKind::Attrezzo),
        stats.on_sale.count(ItemKind::Vestito),
        stats.tokens
    ));
}

// ----------------------------------------------------------------------
// Ispettore dell'NPC selezionato (a destra)
// ----------------------------------------------------------------------

fn inspector(mut contexts: EguiContexts, sim: Option<Res<Sim>>, mut selected: ResMut<SelectedNpc>) {
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
    ui.add(
        egui::ProgressBar::new(npc.action_progress(now)).text(format!(
            "{} rimasti, fino alle {until_text}",
            format_minutes(until.since(now))
        )),
    );

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
    ui.strong("Inventario");
    field(ui, "Gettoni", &npc.inventory.tokens.to_string());
    for (name, item) in [
        ("Attrezzo", ItemKind::Attrezzo),
        ("Vestito", ItemKind::Vestito),
    ] {
        ui.horizontal(|ui| {
            item_swatch(ui, item);
            ui.weak(format!("{name}:"));
            match npc.inventory.durability(item) {
                Some(d) => {
                    ui.add(
                        egui::ProgressBar::new(d)
                            .fill(need_color(d))
                            .text(format!("integrità {:.0}%", d * 100.0)),
                    );
                }
                None => {
                    ui.label("nessuno");
                }
            }
        });
    }
    let wanted: Vec<&str> = [ItemKind::Attrezzo, ItemKind::Vestito]
        .into_iter()
        .filter(|&item| npc.wants(item))
        .map(ItemKind::name)
        .collect();
    let wants = if wanted.is_empty() {
        "niente".to_string()
    } else {
        wanted.join(", ")
    };
    field(ui, "Vuole comprare", &wants);

    if let Some(context) = world.npc_context(npc.id) {
        egui::CollapsingHeader::new("Contesto per il cervello")
            .default_open(false)
            .show(ui, |ui| ui.label(context));
    }
}

/// L'NPC selezionato non esiste più: cerca nel registro come è morto.
fn missing_npc(ui: &mut egui::Ui, world: &World, id: NpcId) {
    let death = world
        .events
        .iter()
        .rev()
        .find(|event| matches!(&event.kind, EventKind::NpcDied { npc, .. } if *npc == id));
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
        Action::Buy(item) => format!("{verb} {}", item.with_article()),
        Action::Idle => verb.to_string(),
    }
}

// ----------------------------------------------------------------------
// Registro eventi (in basso a sinistra)
// ----------------------------------------------------------------------

fn event_log(mut contexts: EguiContexts, sim: Option<Res<Sim>>, mut hide_purchases: Local<bool>) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let total = sim.as_ref().map_or(0, |sim| sim.world.events_total());
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
            ui.checkbox(&mut hide_purchases, "Nascondi gli acquisti degli NPC");
            let hide = *hide_purchases;
            let mut shown = sim
                .world
                .events
                .iter()
                .rev()
                .filter(|e| !(hide && matches!(e.kind, EventKind::ItemBought { .. })))
                .take(EVENT_LOG_LEN)
                .peekable();
            if shown.peek().is_none() {
                ui.weak("Nessun evento.");
                return;
            }
            egui::ScrollArea::vertical()
                .max_height(160.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for event in shown {
                        ui.colored_label(event_color(&event.kind), event.to_string());
                    }
                });
        });
}

fn event_color(kind: &EventKind) -> Color32 {
    match kind {
        EventKind::NpcDied { .. } => DANGER,
        EventKind::NpcStarving { .. } => WARNING,
        EventKind::Shortage { .. } => Color32::from_rgb(230, 200, 80),
        EventKind::Restocked { .. } => GOOD,
        EventKind::ItemBroke { .. } => WARNING,
        EventKind::ItemBought { .. } => Color32::GRAY,
        EventKind::PlayerTook { .. }
        | EventKind::PlayerBought { .. }
        | EventKind::PlayerGave { .. } => PLAYER,
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
                        .num_columns(5 + ItemKind::COUNT)
                        .striped(true)
                        .spacing([10.0, 2.0])
                        .show(ui, |ui| {
                            for header in ["N.", "Nome", "Tipo", "Presenti"] {
                                ui.strong(header);
                            }
                            for item in ItemKind::ALL {
                                ui.horizontal(|ui| {
                                    item_swatch(ui, item);
                                    ui.strong(plural_title(item));
                                });
                            }
                            ui.strong("Prezzi");
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
                                ui.label(text(carriage.id.to_string()));
                                ui.label(text(carriage.name.clone()));
                                ui.label(text(carriage.kind.to_string()));
                                ui.label(text(present.to_string()));
                                stock_cells(ui, world, carriage);
                                ui.end_row();
                            }
                        });
                });
        });
}

/// Una colonna per oggetto (vuota se la carrozza non lo tiene) e i prezzi
/// dei Mercati.
fn stock_cells(ui: &mut egui::Ui, world: &World, carriage: &Carriage) {
    let storable = storable_items(&world.params, carriage.kind);
    for item in ItemKind::ALL {
        if storable.contains(&item) {
            let n = carriage.stock.count(item);
            let text = RichText::new(n.to_string());
            ui.label(if n == 0 { text.color(DANGER) } else { text });
        } else {
            ui.weak("·");
        }
    }
    let prices: Vec<String> = ItemKind::ALL
        .into_iter()
        .filter_map(|item| {
            let price = world.price(carriage.id, item)?;
            Some(format!("{} {price}", item.name()))
        })
        .collect();
    if prices.is_empty() {
        ui.weak("·");
    } else {
        ui.label(prices.join(" · "))
            .on_hover_text("Prezzo in gettoni: sale quando lo scaffale si svuota");
    }
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
/// Eventi causati dal giocatore.
const PLAYER: Color32 = Color32::from_rgb(120, 200, 240);

/// Colore Bevy -> egui.
pub(crate) fn color32(color: Color) -> Color32 {
    let [r, g, b, a] = color.to_srgba().to_u8_array();
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// Quadratino del colore di un oggetto (lo stesso delle casse nel mondo).
pub(crate) fn item_swatch(ui: &mut egui::Ui, item: ItemKind) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, 1.0, color32(item_color(item)));
}

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
