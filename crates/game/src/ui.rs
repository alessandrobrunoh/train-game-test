//! Interfaccia egui: controlli del tempo, ispettore NPC, registro eventi e carrozze.
//!
//! L'ispettore mostra anche età, sesso, origine e legami dell'NPC (ogni
//! parente o amico è un link che lo seleziona) e il comando "Segui" (tasto F)
//! per far seguire l'NPC alla camera. Il registro eventi si filtra per
//! categoria; gli stessi filtri valgono per le notifiche di `life_fx.rs`.
//!
//! Tutte le finestre sono piccole e ancorate ai bordi dello schermo, così il
//! centro (dove la camera mostra il treno) resta libero.

use bevy::prelude::*;
use bevy_egui::egui::{self, Align2, Color32, RichText};
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use sim::{
    Action, ActionKind, Carriage, Event, EventKind, GameTime, ItemKind, Npc, NpcId, RelationKind,
    Stats, World,
};

use crate::brain_ui::BrainWindow;
use crate::history_ui::HistoryWindow;
use crate::population::PopulationWindow;
use crate::saves::SavesWindow;
use crate::sim_bridge::{DeliberationGrace, SPEEDS, seconds_per_year};
use crate::state::{FollowNpc, PointerOverUi, SelectedNpc, Sim, SimClock, SimPerf};
use crate::storage::{item_color, plural_title, storable_items};
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
            .init_resource::<EventFilter>()
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

#[allow(clippy::too_many_arguments)] // sistema Bevy: un parametro per finestra
fn time_panel(
    mut contexts: EguiContexts,
    sim: Option<Res<Sim>>,
    mut clock: ResMut<SimClock>,
    perf: Res<SimPerf>,
    mut population: ResMut<PopulationWindow>,
    mut saves: ResMut<SavesWindow>,
    mut history: ResMut<HistoryWindow>,
    mut brain: ResMut<BrainWindow>,
    cache: Res<UiCache>,
    grace: Res<DeliberationGrace>,
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
                let days_per_year = sim.as_ref().map_or(12, |s| s.world.params.days_per_year);
                match &sim {
                    Some(sim) => {
                        let now = sim.world.clock;
                        ui.strong(format!("Anno {} · {now}", year_of(now, days_per_year)))
                    }
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
                for (key, speed) in SPEEDS.into_iter().enumerate() {
                    let active = clock.minutes_per_second == speed;
                    let response = ui
                        .selectable_label(active, format!("{speed}x"))
                        .on_hover_text(format!(
                            "Tasto {}: {speed} minuti di gioco al secondo\nUn anno ({days_per_year} giorni) ≈ {}",
                            key + 1,
                            format_seconds(seconds_per_year(speed, days_per_year))
                        ));
                    if response.clicked() && !active {
                        clock.minutes_per_second = speed;
                    }
                }
                ui.separator();
                ui.toggle_value(&mut population.open, "Popolazione (G)");
                ui.toggle_value(&mut history.open, "Storia (H)");
                ui.toggle_value(&mut brain.open, "Cervello (B)");
                if ui.toggle_value(&mut saves.open, "Partite (Esc)").changed() {
                    saves.refresh();
                }
            });
            if grace.slows(clock.minutes_per_second) && !clock.paused {
                ui.colored_label(WAIT, grace_text(&grace))
                    .on_hover_text(
                        "Tregua: qualcuno in vista aspetta Laya per una scelta importante. \
                         Si disattiva nella finestra Cervello (B)",
                    );
            }
            if perf.behind && !clock.paused {
                let days_per_year = sim.as_ref().map_or(12, |s| s.world.params.days_per_year);
                ui.colored_label(
                    WARNING,
                    format!(
                        "⚠ La simulazione non tiene il passo: {:.0} minuti al secondo (un anno ≈ {})",
                        perf.effective,
                        format_seconds(seconds_per_year(perf.effective, days_per_year))
                    ),
                );
            }
            if let Some(stats) = &cache.stats {
                ui.horizontal(|ui| stats_row(ui, stats));
            }
        });
}

/// "Marta ci pensa… · rallentato: 2 decisioni in corso (60 min/s)".
fn grace_text(grace: &DeliberationGrace) -> String {
    let who = match grace.waiting.as_slice() {
        [one] => format!("{one} ci pensa…"),
        [a, b] => format!("{a} e {b} ci pensano…"),
        [a, ..] => format!("{a} e altri ci pensano…"),
        [] => String::new(),
    };
    let n = grace.waiting.len();
    let what = if n == 1 {
        "1 decisione in corso".to_string()
    } else {
        format!("{n} decisioni in corso")
    };
    format!("{who} · rallentato: {what} ({:.0} min/s)", grace.max_speed)
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

fn inspector(
    mut contexts: EguiContexts,
    sim: Option<Res<Sim>>,
    mut selected: ResMut<SelectedNpc>,
    mut follow: ResMut<FollowNpc>,
    mut history: ResMut<HistoryWindow>,
) {
    let Some(id) = selected.0 else {
        return;
    };
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let mut open = true;
    let mut clicked = None;
    let mut following = follow.0;
    let mut show_history = false;
    // Non più alto dello schermo: il resto scorre.
    let max_height = (ctx.content_rect().height() - 4.0 * MARGIN - 40.0).max(120.0);
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
                Some(npc) => {
                    egui::ScrollArea::vertical()
                        .max_height(max_height)
                        .show(ui, |ui| {
                            clicked = npc_details(ui, sim, npc, &mut following, &mut show_history);
                            crate::brain_ui::decision_section(ui, &sim.brain, npc);
                        });
                }
                None => missing_npc(ui, &sim.world, id),
            },
        });
    if follow.0 != following {
        follow.0 = following;
    }
    if show_history {
        history.show(id);
    }
    if !open {
        selected.0 = None;
    } else if let Some(other) = clicked {
        selected.0 = Some(other);
    }
}

/// Dettagli dell'NPC; restituisce il parente o amico cliccato, se c'è.
fn npc_details(
    ui: &mut egui::Ui,
    sim: &Sim,
    npc: &Npc,
    following: &mut bool,
    show_history: &mut bool,
) -> Option<NpcId> {
    let world = &sim.world;
    let now = world.clock;
    let mut clicked = None;
    ui.heading(&npc.name);
    let sex = npc.sex;
    ui.label(format!(
        "{} anni · {} · {}",
        npc.age,
        npc.stage().name(sex),
        sex.name()
    ));
    ui.horizontal(|ui| {
        let origin = if world.is_founder(npc.id) {
            sex.pick("fondatrice", "fondatore").to_string()
        } else {
            format!("{} sul treno", sex.pick("nata", "nato"))
        };
        ui.weak(format!("{origin} · {}", npc.id));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.toggle_value(following, "Segui (F)")
                .on_hover_text("La camera segue questo NPC invece del giocatore");
            if ui
                .button("Storia (H)")
                .on_hover_text("Biografia e albero genealogico")
                .clicked()
            {
                *show_history = true;
            }
        });
    });
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
    // Le scelte importanti in corso, subito sotto l'azione.
    crate::brain_ui::mind_section(ui, world, &sim.brain, npc);

    ui.separator();
    if let Some(id) = relations(ui, world, npc) {
        clicked = Some(id);
    }

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
    clicked
}

/// Famiglia e amici, ognuno cliccabile; restituisce chi è stato cliccato.
fn relations(ui: &mut egui::Ui, world: &World, npc: &Npc) -> Option<NpcId> {
    let mut clicked = None;
    ui.strong("Famiglia");
    let family = [
        ("Partner", RelationKind::Partner),
        ("Genitori", RelationKind::Parent),
        ("Figli", RelationKind::Child),
        ("Fratelli", RelationKind::Sibling),
    ];
    let mut any = false;
    for (label, kind) in family {
        let ids: Vec<NpcId> = npc.relations_of(kind).map(|r| r.other).collect();
        if ids.is_empty() {
            continue;
        }
        any = true;
        ui.horizontal_wrapped(|ui| {
            // Il partner ha l'etichetta in base al suo sesso, non a quello dell'NPC.
            let label = match (kind, ids.first().and_then(|&id| world.npc(id))) {
                (RelationKind::Partner, Some(p)) => p.sex.pick("Moglie", "Marito"),
                _ => label,
            };
            ui.weak(format!("{label}:"));
            for id in ids {
                if relative_link(ui, world, id) {
                    clicked = Some(id);
                }
            }
        });
    }
    if !any {
        ui.weak("Nessun parente in vita.");
    }

    let mut friends: Vec<(NpcId, f32)> = npc
        .relations_of(RelationKind::Friend)
        .map(|r| (r.other, r.affinity))
        .collect();
    friends.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    egui::CollapsingHeader::new(format!("Amici e conoscenti ({})", friends.len()))
        .id_salt("inspector_friends")
        .default_open(true)
        .show(ui, |ui| {
            if friends.is_empty() {
                ui.weak("Nessuno.");
                return;
            }
            egui::Grid::new("friends_grid")
                .num_columns(2)
                .spacing([8.0, 2.0])
                .show(ui, |ui| {
                    for (id, affinity) in friends {
                        if relative_link(ui, world, id) {
                            clicked = Some(id);
                        }
                        affinity_bar(ui, affinity);
                        ui.end_row();
                    }
                });
        });
    clicked
}

/// Nome (ed età) di un altro NPC come link; vero se è stato cliccato.
fn relative_link(ui: &mut egui::Ui, world: &World, id: NpcId) -> bool {
    match world.npc(id) {
        Some(other) => ui
            .link(format!("{} ({})", other.name, other.age))
            .on_hover_text(format!(
                "{} · {}\nClick: seleziona",
                other.stage().name(other.sex),
                world.carriage_label(other.carriage)
            ))
            .clicked(),
        None => {
            ui.weak(id.to_string());
            false
        }
    }
}

/// Barretta dell'affinità in `-1..=1`: verde se positiva, rossa se negativa.
fn affinity_bar(ui: &mut egui::Ui, affinity: f32) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(90.0, 10.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, Color32::from_gray(50));
    let fraction = affinity.abs().clamp(0.0, 1.0);
    let fill =
        egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * fraction, rect.height()));
    painter.rect_filled(fill, 2.0, if affinity >= 0.0 { GOOD } else { DANGER });
    response.on_hover_text(format!("Affinità {affinity:+.2}"));
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
        Action::Wait => format!("{verb} un posto a tavola"),
    }
}

// ----------------------------------------------------------------------
// Registro eventi (in basso a sinistra)
// ----------------------------------------------------------------------

/// Categoria di un evento, per i filtri del registro e delle notifiche.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EventCategory {
    Nascite,
    Morti,
    Coppie,
    /// Maggiore età e pensione.
    Eta,
    /// Acquisti degli NPC e oggetti consumati.
    Acquisti,
    Scarsita,
    Giocatore,
}

impl EventCategory {
    const ALL: [EventCategory; 7] = [
        EventCategory::Nascite,
        EventCategory::Morti,
        EventCategory::Coppie,
        EventCategory::Eta,
        EventCategory::Acquisti,
        EventCategory::Scarsita,
        EventCategory::Giocatore,
    ];

    pub(crate) fn of(kind: &EventKind) -> EventCategory {
        match kind {
            EventKind::Born { .. } | EventKind::BirthDenied { .. } => EventCategory::Nascite,
            EventKind::NpcDied { .. } | EventKind::NpcStarving { .. } => EventCategory::Morti,
            EventKind::Coupled { .. } | EventKind::Widowed { .. } => EventCategory::Coppie,
            EventKind::CameOfAge { .. } | EventKind::Retired { .. } => EventCategory::Eta,
            EventKind::ItemBought { .. } | EventKind::ItemBroke { .. } => EventCategory::Acquisti,
            EventKind::Shortage { .. } | EventKind::Restocked { .. } => EventCategory::Scarsita,
            EventKind::PlayerTook { .. }
            | EventKind::PlayerBought { .. }
            | EventKind::PlayerGave { .. } => EventCategory::Giocatore,
            EventKind::DeliberationAsked { kind, .. }
            | EventKind::DeliberationResolved { kind, .. } => match kind {
                sim::DeliberationKind::CoupleProposal { .. } => EventCategory::Coppie,
                sim::DeliberationKind::HaveChild { .. } => EventCategory::Nascite,
                sim::DeliberationKind::Theft { .. } => EventCategory::Acquisti,
                sim::DeliberationKind::Protest { .. } => EventCategory::Scarsita,
            },
            EventKind::Theft { .. } | EventKind::HelpAsked { .. } => EventCategory::Acquisti,
            EventKind::ProtestCalled { .. } | EventKind::AdminConceded { .. } => {
                EventCategory::Scarsita
            }
            EventKind::Austerity { .. } => EventCategory::Scarsita,
            EventKind::PayChanged { .. } => EventCategory::Acquisti,
        }
    }

    fn label(self) -> &'static str {
        match self {
            EventCategory::Nascite => "nascite",
            EventCategory::Morti => "morti",
            EventCategory::Coppie => "coppie",
            EventCategory::Eta => "età",
            EventCategory::Acquisti => "acquisti",
            EventCategory::Scarsita => "scarsità",
            EventCategory::Giocatore => "giocatore",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            EventCategory::Nascite => "Nascite e nascite negate dall'amministrazione",
            EventCategory::Morti => "Morti e NPC che muoiono di fame",
            EventCategory::Coppie => "Nuove coppie e vedovanze",
            EventCategory::Eta => "Maggiore età e pensione",
            EventCategory::Acquisti => "Acquisti degli NPC e oggetti consumati",
            EventCategory::Scarsita => "Scarsità e nuove scorte",
            EventCategory::Giocatore => "Quello che fai tu",
        }
    }
}

/// Categorie di eventi mostrate nel registro e nelle notifiche.
#[derive(Resource, Debug)]
pub(crate) struct EventFilter {
    /// Indicizzato come `EventCategory::ALL`.
    shown: [bool; EventCategory::ALL.len()],
}

impl Default for EventFilter {
    fn default() -> Self {
        Self {
            shown: [true; EventCategory::ALL.len()],
        }
    }
}

impl EventFilter {
    pub(crate) fn allows(&self, kind: &EventKind) -> bool {
        self.shown[EventCategory::of(kind) as usize]
    }

    fn shown_mut(&mut self, category: EventCategory) -> &mut bool {
        &mut self.shown[category as usize]
    }
}

fn event_log(mut contexts: EguiContexts, sim: Option<Res<Sim>>, mut filter: ResMut<EventFilter>) {
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
            let mut checkbox = |ui: &mut egui::Ui, category: EventCategory| {
                ui.checkbox(filter.shown_mut(category), category.label())
                    .on_hover_text(category.hint());
            };
            ui.horizontal(|ui| {
                ui.weak("Vita:");
                for category in &EventCategory::ALL[..4] {
                    checkbox(ui, *category);
                }
            });
            ui.horizontal(|ui| {
                ui.weak("Economia:");
                for category in &EventCategory::ALL[4..6] {
                    checkbox(ui, *category);
                }
                ui.separator();
                checkbox(ui, EventCategory::Giocatore);
            });
            let filter = &*filter;
            let mut shown = sim
                .world
                .events
                .iter()
                .rev()
                .filter(|e| filter.allows(&e.kind))
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

pub(crate) fn event_color(kind: &EventKind) -> Color32 {
    match kind {
        EventKind::NpcDied { .. } => DANGER,
        EventKind::NpcStarving { .. } => WARNING,
        EventKind::Shortage { .. } => Color32::from_rgb(230, 200, 80),
        EventKind::Restocked { .. } => GOOD,
        EventKind::ItemBroke { .. } => WARNING,
        EventKind::ItemBought { .. } => Color32::GRAY,
        EventKind::Born { .. } | EventKind::Coupled { .. } | EventKind::CameOfAge { .. } => GOOD,
        EventKind::Widowed { .. } | EventKind::BirthDenied { .. } => WARNING,
        EventKind::Retired { .. } => Color32::GRAY,
        EventKind::PlayerTook { .. }
        | EventKind::PlayerBought { .. }
        | EventKind::PlayerGave { .. } => PLAYER,
        EventKind::DeliberationAsked { .. } | EventKind::DeliberationResolved { .. } => {
            Color32::GRAY
        }
        EventKind::Theft { .. } | EventKind::ProtestCalled { .. } => WARNING,
        EventKind::HelpAsked { .. } | EventKind::AdminConceded { .. } => GOOD,
        EventKind::Austerity { .. } => WARNING,
        EventKind::PayChanged { .. } => Color32::GRAY,
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
/// Tempo rallentato per la tregua delle deliberazioni.
const WAIT: Color32 = Color32::from_rgb(230, 200, 90);
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

/// Testo di un evento senza l'orario tra parentesi quadre.
pub(crate) fn event_message(event: &Event) -> String {
    let text = event.to_string();
    match text.split_once("] ") {
        Some((_, message)) => message.to_string(),
        None => text,
    }
}

/// Anno di gioco (da 1) in cui cade `time`, con anni di `days_per_year` giorni.
pub(crate) fn year_of(time: GameTime, days_per_year: u32) -> u64 {
    (time.day() - 1) / u64::from(days_per_year.max(1)) + 1
}

/// Secondi reali in forma leggibile, es. "5.8 s", "4 min 48 s", "4 h 48 min".
fn format_seconds(seconds: f32) -> String {
    let s = seconds.max(0.0).round() as u64;
    if seconds < 60.0 {
        format!("{seconds:.1} s")
    } else if s < 3600 {
        format!("{} min {} s", s / 60, s % 60)
    } else {
        format!("{} h {} min", s / 3600, s % 3600 / 60)
    }
}

/// Minuti di gioco in forma leggibile, es. "2h 05m" o "45m".
pub(crate) fn format_minutes(minutes: u64) -> String {
    let (hours, minutes) = (minutes / 60, minutes % 60);
    if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else {
        format!("{minutes}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_events_by_category() {
        let mut filter = EventFilter::default();
        let bought = EventKind::ItemBought {
            npc: NpcId(1),
            name: "Ada".into(),
            item: ItemKind::Vestito,
            price: 3,
            carriage: sim::CarriageId(0),
        };
        let shortage = EventKind::Shortage {
            item: ItemKind::Razione,
        };
        assert_eq!(EventCategory::of(&bought), EventCategory::Acquisti);
        assert_eq!(EventCategory::of(&shortage), EventCategory::Scarsita);
        assert!(filter.allows(&bought) && filter.allows(&shortage));
        *filter.shown_mut(EventCategory::Acquisti) = false;
        assert!(!filter.allows(&bought));
        assert!(filter.allows(&shortage));
    }

    #[test]
    fn years_and_messages() {
        assert_eq!(year_of(GameTime::from_dhm(1, 6, 0), 12), 1);
        assert_eq!(year_of(GameTime::from_dhm(12, 23, 59), 12), 1);
        assert_eq!(year_of(GameTime::from_dhm(13, 0, 0), 12), 2);
        let event = Event {
            time: GameTime::from_dhm(3, 8, 0),
            kind: EventKind::Shortage {
                item: ItemKind::Razione,
            },
        };
        assert_eq!(
            event_message(&event),
            "Carestia: nessuna Mensa ha più razioni"
        );
        assert_eq!(format_seconds(5.76), "5.8 s");
        assert_eq!(format_seconds(288.0), "4 min 48 s");
        assert_eq!(format_seconds(17280.0), "4 h 48 min");
    }
}
