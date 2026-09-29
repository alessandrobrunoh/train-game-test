//! Il cervello degli NPC in gioco: modalità, stato del modello, fuoco e finestra "Cervello" (tasto B).
//!
//! `Sim::brain` è sempre un [`GameBrain`] (`LayaBrain` sopra `UtilityBrain`):
//! - **Utility**: Laya spento, decide solo `UtilityBrain`;
//! - **Laya (ibrido)**: il modello vero (feature `laya`) si carica su un thread
//!   a parte ("caricamento modello…"), poi corregge in modo asincrono una parte
//!   delle decisioni; se non è compilato o fallisce, l'opzione resta
//!   indisponibile col motivo e decide `UtilityBrain`;
//! - **Mock (prova)**: lo stesso percorso ibrido con `MockModel`, per provare
//!   interfaccia e statistiche senza pesi.
//!
//! Il fuoco di Laya sono le carrozze visibili dalla camera (più una per lato,
//! come per gli sprite): i loro NPC hanno la precedenza e, se l'opzione è
//! attiva, "stanno pensando" (ozio breve) mentre aspettano la risposta.
//! `TRAINGAME_BRAIN=laya|mock|utility` sceglie la modalità all'avvio.
//!
//! **Deliberazioni** (scelte di vita rare): con Laya (o il mock) attivo il
//! cervello risponde anche a quelle; la finestra ha una sezione
//! "Deliberazioni" (statistiche per tipo, accordo con le regole, soglia,
//! peso della regola, tregua) con le ultime decisioni cliccabili, e
//! l'ispettore una sezione "In mente". Qui si aggiorna anche chi, in vista,
//! aspetta la risposta di Laya ([`DeliberationGrace`], che rallenta il tempo).
//! I salvataggi contengono solo `UtilityBrain`: caricando una partita la
//! modalità scelta resta quella in corso.

use std::time::Duration;

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, RichText};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use sim::{Brain, CarriageId, DeliberationKind, Npc, NpcId, Resolver, THINK, World};
use sim_laya::loader::{real_model_loader, unavailable_reason};
use sim_laya::{DeliberationStatus, MockModel, ModelStatus, Source};

use crate::npc_render::visible_window;
use crate::sim_bridge::{DeliberationGrace, SimTickSet};
use crate::state::{GameBrain, SelectedNpc, Sim};
use crate::ui::{PointerCheck, format_minutes};

/// Latenza simulata del modello di prova.
const MOCK_LATENCY: Duration = Duration::from_millis(30);
const OK_COLOR: Color32 = Color32::from_rgb(120, 200, 120);
const WAIT_COLOR: Color32 = Color32::from_rgb(230, 200, 90);
const ERROR_COLOR: Color32 = Color32::from_rgb(235, 110, 90);

pub struct BrainUiPlugin;

impl Plugin for BrainUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BrainWindow>()
            .init_resource::<BrainChoice>()
            .add_systems(Startup, mode_from_env)
            .add_systems(
                Update,
                (toggle_window, update_focus_and_poll, update_grace)
                    .chain()
                    .before(SimTickSet),
            )
            .add_systems(EguiPrimaryContextPass, brain_window.before(PointerCheck));
    }
}

/// Finestra "Cervello" aperta o chiusa (tasto B).
#[derive(Resource, Default)]
pub struct BrainWindow {
    pub open: bool,
}

/// Chi decide per gli NPC.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BrainMode {
    #[default]
    Utility,
    Laya,
    Mock,
}

impl BrainMode {
    fn label(self) -> &'static str {
        match self {
            BrainMode::Utility => "Utility",
            BrainMode::Laya => "Laya (ibrido)",
            BrainMode::Mock => "Mock (prova)",
        }
    }
}

/// Modalità scelta e quale modello è collegato al cervello.
#[derive(Resource, Debug, Default)]
pub struct BrainChoice {
    pub mode: BrainMode,
    /// Il modello attualmente collegato (anche mentre si usa Utility).
    loaded: Option<BrainMode>,
}

impl BrainChoice {
    /// Passa a `mode`: carica il modello se serve (in background), poi
    /// accende o spegne Laya nel cervello.
    pub fn select(&mut self, brain: &mut GameBrain, mode: BrainMode) {
        self.mode = mode;
        match mode {
            BrainMode::Utility => {}
            BrainMode::Laya if self.loaded != Some(mode) => {
                if unavailable_reason().is_none() {
                    brain.attach_loader(real_model_loader());
                    self.loaded = Some(mode);
                }
            }
            BrainMode::Mock if self.loaded != Some(mode) => {
                brain.attach_async(Box::new(
                    MockModel::new().with_latency(MOCK_LATENCY, Duration::ZERO),
                ));
                self.loaded = Some(mode);
            }
            _ => {}
        }
        brain.config_mut().enabled = mode != BrainMode::Utility && self.loaded == Some(mode);
        brain.reset_stats();
    }

    /// Ricarica il modello della modalità corrente (dopo un errore).
    pub fn retry(&mut self, brain: &mut GameBrain) {
        self.loaded = None;
        brain.detach();
        let mode = self.mode;
        self.select(brain, mode);
    }
}

/// `TRAINGAME_BRAIN=laya` (o `mock`) sceglie la modalità all'avvio.
fn mode_from_env(sim: Option<ResMut<Sim>>, mut choice: ResMut<BrainChoice>) {
    let Some(mode) = std::env::var("TRAINGAME_BRAIN")
        .ok()
        .and_then(|v| parse_mode(&v))
    else {
        return;
    };
    if let Some(mut sim) = sim {
        choice.select(&mut sim.brain, mode);
        info!("Cervello: {}", mode.label());
    }
}

fn parse_mode(value: &str) -> Option<BrainMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "utility" => Some(BrainMode::Utility),
        "laya" => Some(BrainMode::Laya),
        "mock" => Some(BrainMode::Mock),
        _ => None,
    }
}

fn toggle_window(keys: Res<ButtonInput<KeyCode>>, mut window: ResMut<BrainWindow>) {
    if keys.just_pressed(KeyCode::KeyB) {
        window.open = !window.open;
    }
}

/// Fuoco = carrozze visibili; raccoglie anche i messaggi del worker (stato
/// del caricamento e risposte) quando la simulazione è in pausa.
fn update_focus_and_poll(
    sim: Option<ResMut<Sim>>,
    camera: Option<Single<(&Transform, &Projection), With<Camera2d>>>,
    mut last: Local<Option<(usize, usize, usize)>>,
    mut last_status: Local<Option<ModelStatus>>,
) {
    let Some(mut sim) = sim else {
        return;
    };
    // Non segna `Sim` come modificato: cambia solo lo stato interno del cervello.
    let sim = sim.bypass_change_detection();
    sim.brain.poll();
    let status = sim.brain.status();
    if last_status.as_ref() != Some(status) {
        match status {
            ModelStatus::Loading => info!("Cervello: caricamento modello…"),
            ModelStatus::Ready(name) => info!("Cervello: modello pronto ({name})"),
            ModelStatus::Failed(e) => warn!("Cervello: errore del modello: {e}"),
            ModelStatus::Missing => {}
        }
        *last_status = Some(status.clone());
    }
    let Some(camera) = camera else {
        return;
    };
    let (transform, projection) = *camera;
    let Projection::Orthographic(ortho) = projection else {
        return;
    };
    let len = sim.world.carriages.len();
    let (lo, hi) = visible_window(transform.translation.x, ortho.area, len);
    if *last != Some((lo, hi, len)) {
        *last = Some((lo, hi, len));
        sim.brain.set_focus((lo..=hi).map(|i| CarriageId(i as u16)));
    }
}

/// Chi, nelle carrozze a fuoco, aspetta Laya per una deliberazione (da non più
/// di `timeout` reali): finché c'è qualcuno, `advance_sim` rallenta il tempo.
fn update_grace(sim: Option<Res<Sim>>, mut grace: ResMut<DeliberationGrace>) {
    let waiting: Vec<String> = match &sim {
        Some(sim) if grace.enabled => waiting_for_laya(&sim.world, &sim.brain, grace.timeout),
        _ => Vec::new(),
    };
    if grace.waiting != waiting {
        grace.waiting = waiting;
    }
}

/// Nomi (di battesimo) degli NPC a fuoco con una deliberazione chiesta al
/// modello da meno di `timeout`.
fn waiting_for_laya(world: &World, brain: &GameBrain, timeout: Duration) -> Vec<String> {
    let mut names: Vec<(NpcId, String)> = brain
        .pending_deliberations()
        .filter(|info| info.sent.elapsed() < timeout)
        .filter_map(|info| world.npc(info.npc))
        .filter(|npc| brain.is_focus(npc.carriage))
        .map(|npc| (npc.id, npc.first_name().to_string()))
        .collect();
    names.sort();
    names.dedup();
    names.into_iter().map(|(_, name)| name).collect()
}

fn status_text(brain: &GameBrain, choice: &BrainChoice) -> (String, Color32) {
    if choice.mode == BrainMode::Laya
        && let Some(reason) = unavailable_reason()
    {
        return (format!("non disponibile: {reason}"), ERROR_COLOR);
    }
    match brain.status() {
        ModelStatus::Missing => ("nessun modello: decide UtilityBrain".into(), Color32::GRAY),
        ModelStatus::Loading => ("caricamento modello…".into(), WAIT_COLOR),
        ModelStatus::Ready(name) if brain.is_active() => (format!("pronto: {name}"), OK_COLOR),
        ModelStatus::Ready(name) => (format!("{name} caricato, spento"), Color32::GRAY),
        ModelStatus::Failed(e) => (format!("errore: {e}"), ERROR_COLOR),
    }
}

fn brain_window(
    mut contexts: EguiContexts,
    sim: Option<ResMut<Sim>>,
    mut window: ResMut<BrainWindow>,
    mut choice: ResMut<BrainChoice>,
    mut grace: ResMut<DeliberationGrace>,
    mut selected: ResMut<SelectedNpc>,
) {
    if !window.open {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let Some(mut sim) = sim else {
        return;
    };
    let mut open = true;
    let mut clicked = None;
    // Il cervello cambia solo se si tocca qualcosa: niente `Sim` modificato a ogni frame.
    let Sim { world, brain } = sim.bypass_change_detection();
    let max_height = (ctx.content_rect().height() - 160.0).max(200.0);
    egui::Window::new("Cervello")
        .open(&mut open)
        .default_pos([crate::ui::MARGIN, 120.0])
        .default_width(340.0)
        .resizable(false)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .max_height(max_height)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Modalità:");
                        for mode in [BrainMode::Utility, BrainMode::Laya, BrainMode::Mock] {
                            let unavailable =
                                (mode == BrainMode::Laya).then(unavailable_reason).flatten();
                            let response = ui.add_enabled(
                                unavailable.is_none(),
                                egui::RadioButton::new(choice.mode == mode, mode.label()),
                            );
                            let response = match unavailable {
                                Some(reason) => response.on_disabled_hover_text(reason),
                                None => response,
                            };
                            if response.clicked() && choice.mode != mode {
                                choice.select(brain, mode);
                            }
                        }
                    });
                    let (text, color) = status_text(brain, &choice);
                    ui.horizontal_wrapped(|ui| {
                        ui.weak("Stato:");
                        ui.colored_label(color, text);
                    });
                    if matches!(brain.status(), ModelStatus::Failed(_))
                        && choice.mode != BrainMode::Utility
                        && ui.button("Riprova").clicked()
                    {
                        choice.retry(brain);
                    }
                    ui.separator();
                    egui::CollapsingHeader::new("Azioni di tutti i giorni")
                        .default_open(false)
                        .show(ui, |ui| {
                            stats_section(ui, brain);
                            ui.separator();
                            settings_section(ui, brain);
                        });
                    egui::CollapsingHeader::new("Deliberazioni")
                        .default_open(true)
                        .show(ui, |ui| {
                            clicked = deliberations_section(ui, world, brain, &mut grace);
                        });
                });
        });
    if !open {
        window.open = false;
    }
    if let Some(id) = clicked {
        selected.0 = Some(id);
    }
}

/// Nomi brevi dei tipi di deliberazione, per le tabelle.
const KIND_SHORT: [&str; DeliberationKind::COUNT] = ["coppia", "figlio", "furto", "protesta"];

/// Scelta in parole brevi: "accetta", "chiede tempo".
fn choice_word(choice: sim::Choice) -> String {
    choice.key().replace('_', " ")
}

/// Sezione "Deliberazioni" della finestra: statistiche, impostazioni e ultime
/// decisioni; restituisce l'NPC cliccato.
fn deliberations_section(
    ui: &mut egui::Ui,
    world: &World,
    brain: &mut GameBrain,
    grace: &mut DeliberationGrace,
) -> Option<NpcId> {
    let mut clicked = None;
    let answering = brain.answers_deliberations();
    ui.horizontal_wrapped(|ui| {
        ui.weak("Chi decide:");
        if answering {
            ui.colored_label(
                OK_COLOR,
                "Laya, se è abbastanza sicuro; se no le regole alla scadenza",
            );
        } else {
            ui.label("le regole del sim, subito");
        }
    });
    let s = brain.stats().deliberations.clone();
    let counters = &world.deliberation_counters;
    egui::Grid::new("delib_stats")
        .num_columns(7)
        .striped(true)
        .show(ui, |ui| {
            for head in ["", "chieste", "risposte", "date", "incerte", "tardi", "accordo"] {
                ui.label(RichText::new(head).small().weak());
            }
            ui.end_row();
            for (k, name) in KIND_SHORT.iter().enumerate() {
                ui.label(RichText::new(*name).small());
                for n in [s.asked[k], s.answered[k], s.applied[k], s.low_confidence[k], s.late[k]] {
                    ui.label(RichText::new(n.to_string()).small());
                }
                ui.label(RichText::new(pct(s.agreement(Some(k)))).small());
                ui.end_row();
            }
        })
        .response
        .on_hover_text(
            "chieste a Laya · risposte arrivate · date al mondo · sotto soglia (decidono le regole) · \
             arrivate dopo la scadenza · stessa scelta più probabile della regola",
        );
    let latency = s.avg_latency().map_or("—".to_string(), |l| {
        format!(
            "{:.0} ms (max {:.0})",
            l.as_secs_f64() * 1000.0,
            s.latency_max.as_secs_f64() * 1000.0
        )
    });
    ui.horizontal_wrapped(|ui| {
        ui.weak("Accordo con le regole:");
        ui.label(pct(s.agreement(None)));
        ui.weak("· latenza:");
        ui.label(latency);
    });
    ui.horizontal_wrapped(|ui| {
        ui.weak("Nel mondo:");
        ui.label(format!(
            "{} aperte · {} decise da Laya · {} dalle regole · {} annullate",
            world.open_deliberations().len(),
            counters.by_brain.iter().sum::<u64>(),
            counters.by_rules.iter().sum::<u64>(),
            counters.cancelled
        ));
    });

    let mut config = brain.config().clone();
    ui.checkbox(
        &mut config.deliberations,
        "Laya risponde alle deliberazioni",
    );
    ui.add_enabled(
        config.deliberations,
        egui::Slider::new(&mut config.deliberation_min_confidence, 0.0..=1.0)
            .text("soglia deliberazioni"),
    )
    .on_hover_text("Sotto questa probabilità decidono le regole alla scadenza");
    ui.add_enabled(
        config.deliberations,
        egui::Slider::new(&mut config.prior_weight, 0.0..=1.0).text("peso delle regole"),
    )
    .on_hover_text(
        "Miscela le probabilità di Laya con quelle delle regole (0 = solo Laya). \
         Con 0.5 Laya decide soprattutto quando è d'accordo con le regole",
    );
    if &config != brain.config() {
        *brain.config_mut() = config;
    }
    ui.horizontal(|ui| {
        ui.checkbox(&mut grace.enabled, "Tregua").on_hover_text(
            "Mentre qualcuno in vista aspetta Laya per una deliberazione, il tempo rallenta \
                 (al più per qualche secondo reale)",
        );
        ui.add_enabled(
            grace.enabled,
            egui::DragValue::new(&mut grace.max_speed)
                .range(1.0..=600.0)
                .suffix(" min/s"),
        );
        let mut secs = grace.timeout.as_secs_f32();
        if ui
            .add_enabled(
                grace.enabled,
                egui::DragValue::new(&mut secs)
                    .range(0.5..=10.0)
                    .speed(0.1)
                    .suffix(" s"),
            )
            .changed()
        {
            grace.timeout = Duration::from_secs_f32(secs);
        }
    });

    ui.separator();
    ui.strong("Ultime decisioni");
    let recent: Vec<_> = world.recent_deliberations().iter().rev().take(20).collect();
    if recent.is_empty() {
        ui.weak("ancora nessuna");
    }
    egui::ScrollArea::vertical()
        .id_salt("delib_recent")
        .max_height(200.0)
        .show(ui, |ui| {
            for r in recent {
                let d = &r.deliberation;
                let name = world
                    .npc(d.npc)
                    .map_or_else(|| d.npc.to_string(), |n| n.name.clone());
                let chosen = r
                    .chosen()
                    .map_or_else(String::new, |o| choice_word(o.choice));
                let by = match (r.by, r.confidence) {
                    (Resolver::Brain, Some(c)) => format!("Laya {:.0}%", c * 100.0),
                    (Resolver::Brain, None) => "Laya".to_string(),
                    (Resolver::Rules, _) => match brain
                        .deliberation_info(d.id)
                        .and_then(|i| i.confidence.map(|c| (i.status, c)))
                    {
                        Some((DeliberationStatus::LowConfidence, c)) => {
                            format!("regole (Laya {:.0}%)", c * 100.0)
                        }
                        Some((DeliberationStatus::Late, _)) => "regole (Laya in ritardo)".into(),
                        _ => "regole".to_string(),
                    },
                };
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "{:02}:{:02}",
                            r.resolved.hour(),
                            r.resolved.minute()
                        ))
                        .small()
                        .weak(),
                    );
                    let alive = world.npc(d.npc).is_some();
                    let link = ui.add_enabled(alive, egui::Link::new(RichText::new(name).small()));
                    if link.on_hover_text(&d.question).clicked() {
                        clicked = Some(d.npc);
                    }
                    ui.label(
                        RichText::new(format!("{}: {chosen}", KIND_SHORT[d.kind.index()])).small(),
                    );
                    let color = if r.by == Resolver::Brain {
                        OK_COLOR
                    } else {
                        Color32::GRAY
                    };
                    ui.label(RichText::new(by).small().color(color));
                });
            }
        });
    clicked
}

fn pct(v: Option<f32>) -> String {
    v.map_or("—".to_string(), |v| format!("{:.0}%", v * 100.0))
}

/// Sezione dell'ispettore "In mente": la deliberazione aperta dell'NPC (con
/// le probabilità di Laya e delle regole, chi deciderà, quando) e le ultime chiuse.
pub(crate) fn mind_section(ui: &mut egui::Ui, world: &World, brain: &GameBrain, npc: &Npc) {
    let open = world.deliberation_of(npc.id);
    let recent: Vec<_> = world
        .recent_deliberations()
        .iter()
        .rev()
        .filter(|r| r.deliberation.npc == npc.id)
        .take(4)
        .collect();
    if open.is_none() && recent.is_empty() {
        return;
    }
    ui.separator();
    ui.strong("In mente");
    if let Some(d) = open {
        ui.label(RichText::new(&d.question).italics());
        let rule = world.deliberation_rule_weights(d.id).unwrap_or_default();
        let info = brain.deliberation_info(d.id);
        let laya = info.and_then(|i| i.model.clone());
        egui::Grid::new("mind_options")
            .num_columns(3)
            .spacing([6.0, 2.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Laya").small().weak());
                ui.label(RichText::new("regole").small().weak());
                ui.label(RichText::new("opzione").small().weak());
                ui.end_row();
                for (k, o) in d.options.iter().enumerate() {
                    match laya.as_ref().and_then(|p| p.get(k)) {
                        Some(&p) => bar(ui, p, OK_COLOR),
                        None => {
                            ui.label(RichText::new("—").small().weak());
                        }
                    }
                    match rule.get(k) {
                        Some(&p) => bar(ui, p, Color32::from_gray(140)),
                        None => {
                            ui.label("");
                        }
                    }
                    ui.label(RichText::new(&o.description).small());
                    ui.end_row();
                }
            });
        let who = if !brain.answers_deliberations() {
            ("decidono le regole".to_string(), Color32::GRAY)
        } else {
            match info.map(|i| (i.status, i.confidence)) {
                None | Some((DeliberationStatus::Pending, _)) => {
                    ("Laya ci pensa… (se no le regole)".to_string(), WAIT_COLOR)
                }
                Some((DeliberationStatus::LowConfidence, c)) => {
                    let mixed = if brain.config().prior_weight > 0.0 {
                        " con le regole"
                    } else {
                        ""
                    };
                    (
                        format!(
                            "Laya è incerto ({:.0}%{mixed}): decideranno le regole",
                            c.unwrap_or(0.0) * 100.0
                        ),
                        Color32::GRAY,
                    )
                }
                Some((DeliberationStatus::Failed, _)) => (
                    "errore di Laya: decideranno le regole".to_string(),
                    ERROR_COLOR,
                ),
                Some((status, _)) => (status.name().to_string(), Color32::GRAY),
            }
        };
        ui.colored_label(who.1, who.0);
        let left = d.deadline.since(world.clock);
        ui.weak(format!(
            "Scadenza tra {} (alle {:02}:{:02})",
            format_minutes(left),
            d.deadline.hour(),
            d.deadline.minute()
        ));
    }
    if !recent.is_empty() {
        ui.label(RichText::new("Decisioni recenti").small().weak());
        for r in recent {
            let d = &r.deliberation;
            let chosen = r.chosen().map_or("", |o| o.description.as_str());
            let by = match (r.by, r.confidence) {
                (Resolver::Brain, Some(c)) => format!(" · Laya {:.0}%", c * 100.0),
                (Resolver::Brain, None) => " · Laya".to_string(),
                (Resolver::Rules, _) => " · regole".to_string(),
            };
            ui.label(
                RichText::new(format!(
                    "g{} {:02}:{:02} · {}: {chosen}{by}",
                    r.resolved.day(),
                    r.resolved.hour(),
                    r.resolved.minute(),
                    d.kind.topic()
                ))
                .small(),
            )
            .on_hover_text(&d.question);
        }
    }
}

/// Barretta di probabilità con la percentuale accanto.
fn bar(ui: &mut egui::Ui, p: f32, color: Color32) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        ui.add(
            egui::ProgressBar::new(p)
                .desired_width(22.0)
                .desired_height(6.0)
                .fill(color),
        );
        ui.label(RichText::new(format!("{:.0}%", p * 100.0)).small());
    });
}

fn stats_section(ui: &mut egui::Ui, brain: &mut GameBrain) {
    let s = brain.stats().clone();
    let total = s.decisions().max(1) as f32;
    ui.strong("Decisioni");
    ui.horizontal_wrapped(|ui| {
        for source in Source::ALL {
            let n = s.count(source);
            ui.label(format!(
                "{} {} ({:.0}%)",
                source.name(),
                n,
                n as f32 / total * 100.0
            ));
        }
    });
    let agreement = s
        .agreement()
        .map_or("—".to_string(), |a| format!("{:.0}%", a * 100.0));
    let latency = s.avg_latency().map_or("—".to_string(), |l| {
        format!(
            "{:.0} ms (max {:.0})",
            l.as_secs_f64() * 1000.0,
            s.latency_max.as_secs_f64() * 1000.0
        )
    });
    egui::Grid::new("brain_stats")
        .num_columns(2)
        .show(ui, |ui| {
            let mut row = |name: &str, value: String| {
                ui.weak(name);
                ui.label(value);
                ui.end_row();
            };
            row("Accordo con Utility", agreement);
            row("Latenza media", latency);
            row("In coda", s.queue.to_string());
            row(
                "Domande",
                format!(
                    "{} inviate · {} risposte · {} fallite",
                    s.jobs_sent, s.jobs_answered, s.jobs_failed
                ),
            );
            row(
                "Risposte",
                format!(
                    "{} applicate · {} poco sicure · {} superate",
                    s.applied, s.rejected_low_confidence, s.rejected_stale
                ),
            );
            row(
                "Cache",
                format!("{} usi · {} voci", s.cache_hits, brain.cache_len()),
            );
            row(
                "Lotti",
                s.avg_batch().map_or("—".to_string(), |b| {
                    format!("{} (media {b:.1} righe)", s.batches)
                }),
            );
            row(
                "Confidenza",
                s.confidence_hist
                    .iter()
                    .map(|c| c.to_string())
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        });
    if let Some(e) = &s.last_error {
        ui.colored_label(ERROR_COLOR, format!("Ultimo errore: {e}"));
    }
    if ui.small_button("Azzera statistiche").clicked() {
        brain.reset_stats();
    }
}

fn settings_section(ui: &mut egui::Ui, brain: &mut GameBrain) {
    let mut config = brain.config().clone();
    ui.add(egui::Slider::new(&mut config.budget_per_call, 0..=32).text("domande per tick"))
        .on_hover_text("Quante decisioni al massimo si chiedono a Laya a ogni tick");
    ui.add(egui::Slider::new(&mut config.max_in_flight, 1..=256).text("coda massima"));
    ui.add(egui::Slider::new(&mut config.min_confidence, 0.0..=1.0).text("soglia di confidenza"))
        .on_hover_text("Sotto questa probabilità si usa UtilityBrain");
    ui.add(egui::Slider::new(&mut config.top_k, 2..=sim_laya::brain::MAX_TOP_K).text("top-k"))
        .on_hover_text("Opzioni passate a Laya: le migliori per UtilityBrain");
    ui.add(egui::Slider::new(&mut config.margin, 0.0..=1.0).text("margine"))
        .on_hover_text(
            "Fuori dalle carrozze visibili si chiede solo se le prime due opzioni distano meno di così",
        );
    ui.horizontal(|ui| {
        ui.checkbox(&mut config.think, "Chi è in vista aspetta Laya");
        ui.add_enabled(
            config.think,
            egui::DragValue::new(&mut config.think_minutes)
                .range(1..=30)
                .suffix(" min"),
        );
    });
    if &config != brain.config() {
        *brain.config_mut() = config;
    }
}

/// Sezione dell'ispettore: chi ha deciso l'azione corrente dell'NPC.
pub(crate) fn decision_section(ui: &mut egui::Ui, brain: &GameBrain, npc: &Npc) {
    ui.separator();
    ui.strong("Cervello");
    let Some(info) = brain.decision(npc.id) else {
        ui.weak(if brain.is_active() {
            "nessuna decisione da quando Laya è attivo"
        } else {
            "decide UtilityBrain"
        });
        return;
    };
    let current = npc.action_since == info.time;
    let who = match info.source {
        Source::Utility => "UtilityBrain",
        Source::Laya => "Laya",
        Source::Cache => "Laya (cache)",
        Source::Think => "in attesa di Laya",
    };
    let when = format!("{:02}:{:02}", info.time.hour(), info.time.minute());
    ui.horizontal_wrapped(|ui| {
        ui.weak(if current {
            "Azione corrente:"
        } else {
            "Ultima decisione:"
        });
        ui.label(format!("{who} (alle {when})"));
    });
    if info.source == Source::Think && current && info.choice == THINK {
        ui.colored_label(WAIT_COLOR, "sta pensando…");
    }
    if let Some(conf) = info.confidence {
        let differs = if info.choice != info.fallback {
            " · UtilityBrain avrebbe scelto altro"
        } else {
            " · come UtilityBrain"
        };
        ui.label(format!("Confidenza {:.0}%{differs}", conf * 100.0));
    }
    for (description, p) in info.options.iter().take(5) {
        ui.horizontal(|ui| {
            ui.add(
                egui::ProgressBar::new(*p)
                    .desired_width(60.0)
                    .text(RichText::new(format!("{:.0}%", p * 100.0)).small()),
            );
            ui.label(RichText::new(description).small());
        });
    }
    if brain.is_pending(npc.id) {
        ui.weak("domanda a Laya in corso…");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::new_brain;
    use sim::{Brain, MINUTES_PER_DAY, UtilityBrain, World};

    #[test]
    fn modes_switch_without_touching_the_fallback() {
        let mut brain = new_brain(UtilityBrain::new(1));
        let mut choice = BrainChoice::default();
        assert!(!brain.is_active());
        choice.select(&mut brain, BrainMode::Mock);
        assert_eq!(choice.mode, BrainMode::Mock);
        // Il mock si carica sul thread del worker.
        let start = std::time::Instant::now();
        while !brain.is_active() {
            assert!(start.elapsed() < Duration::from_secs(5));
            brain.poll();
            std::thread::sleep(Duration::from_millis(1));
        }
        let mut world = World::generate(1, 8, 80);
        brain.set_focus((0..8).map(CarriageId));
        world.run(&mut brain, MINUTES_PER_DAY / 2);
        assert!(brain.stats().jobs_sent > 0);

        // Si torna a Utility: il modello resta caricato ma spento.
        choice.select(&mut brain, BrainMode::Utility);
        assert!(!brain.is_active());
        assert!(matches!(brain.status(), ModelStatus::Ready(_)));
        // Un caricamento sostituisce il ripiego e tiene la modalità.
        choice.select(&mut brain, BrainMode::Mock);
        brain.replace_fallback(UtilityBrain::new(2));
        assert!(brain.is_active());
        assert!(brain.wants_descriptions() == UtilityBrain::new(0).wants_descriptions());
    }

    #[test]
    fn modes_parse_from_the_environment_value() {
        assert_eq!(parse_mode(" Laya "), Some(BrainMode::Laya));
        assert_eq!(parse_mode("mock"), Some(BrainMode::Mock));
        assert_eq!(parse_mode("utility"), Some(BrainMode::Utility));
        assert_eq!(parse_mode("gpt"), None);
    }

    #[cfg(not(feature = "laya"))]
    #[test]
    fn laya_is_unavailable_without_the_feature() {
        let mut brain = new_brain(UtilityBrain::new(1));
        let mut choice = BrainChoice::default();
        choice.select(&mut brain, BrainMode::Laya);
        assert!(!brain.is_active());
        let (text, _) = status_text(&brain, &choice);
        assert!(text.starts_with("non disponibile"), "{text}");
    }
}
