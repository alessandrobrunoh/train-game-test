//! Pannello "Popolazione" (tasto G o bottone nel pannello del tempo): come
//! cambia la popolazione del treno nel corso delle generazioni.
//!
//! Una volta per giorno di gioco si salva un campione (popolazione, tetto
//! delle nascite, cuccette, stadi di vita, coppie, età media, razioni per
//! persona) in `PopulationHistory`. Quando la storia arriva a `MAX_SAMPLES`
//! campioni (200 anni di 12 giorni) se ne tiene uno ogni due e si campiona
//! con passo doppio: la memoria resta limitata e la storia copre sempre tutta
//! la partita. Nascite e morti si contano per anno, esatte.
//!
//! I grafici usano `egui_plot`, uno alla volta (schede), sotto i totali.

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, RichText};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use egui_plot::{Bar, BarChart, Corner, FilledArea, HLine, Legend, Line, LineStyle, Plot};
use sim::{DeathCause, ItemKind, LifeStage, Stats, World};

use crate::saves::WorldRebuildSet;
use crate::sim_bridge::SimTickSet;
use crate::state::{Sim, WorldReplaced};
use crate::ui::{PointerCheck, year_of};

/// Campioni tenuti al massimo prima di dimezzarli (200 anni da 12 giorni).
const MAX_SAMPLES: usize = 2400;
const PLOT_HEIGHT: f32 = 220.0;
const WINDOW_WIDTH: f32 = 560.0;
/// L'asse y arriva fino a questo multiplo del massimo, per far posto alla legenda.
const LEGEND_HEADROOM: f64 = 1.45;

const POPULATION_COLOR: Color32 = Color32::from_rgb(120, 200, 240);
const LIMIT_COLOR: Color32 = Color32::from_rgb(240, 160, 60);
const BEDS_COLOR: Color32 = Color32::from_rgb(170, 170, 170);
const BIRTHS_COLOR: Color32 = Color32::from_rgb(110, 190, 110);
const DEATHS_COLOR: Color32 = Color32::from_rgb(230, 80, 80);
const COUPLES_COLOR: Color32 = Color32::from_rgb(255, 110, 150);
const AGE_COLOR: Color32 = Color32::from_rgb(220, 200, 90);
const FOOD_COLOR: Color32 = Color32::from_rgb(230, 170, 90);

/// Colori degli stadi di vita (come gli NPC senza lavoro nel mondo).
fn stage_color(stage: LifeStage) -> Color32 {
    match stage {
        LifeStage::Bambino => Color32::from_rgb(255, 153, 191),
        LifeStage::Giovane => Color32::from_rgb(190, 120, 220),
        LifeStage::Adulto => Color32::from_rgb(90, 150, 255),
        LifeStage::Anziano => Color32::from_rgb(184, 184, 178),
    }
}

fn stage_title(stage: LifeStage) -> &'static str {
    match stage {
        LifeStage::Bambino => "Bambini",
        LifeStage::Giovane => "Giovani",
        LifeStage::Adulto => "Adulti",
        LifeStage::Anziano => "Anziani",
    }
}

pub struct PopulationPlugin;

impl Plugin for PopulationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PopulationWindow>()
            .init_resource::<PopulationHistory>()
            .add_systems(Update, (toggle_window, sample_population.after(SimTickSet)))
            .add_systems(
                PreUpdate,
                reset_history
                    .in_set(WorldRebuildSet)
                    .run_if(on_message::<WorldReplaced>),
            )
            .add_systems(
                EguiPrimaryContextPass,
                population_window.before(PointerCheck),
            );
    }
}

/// Il pannello è aperto (tasto G) e quale grafico mostra.
#[derive(Resource, Default)]
pub(crate) struct PopulationWindow {
    pub(crate) open: bool,
    chart: Chart,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum Chart {
    #[default]
    Popolazione,
    Stadi,
    NasciteMorti,
    Coppie,
    Razioni,
}

impl Chart {
    const ALL: [Chart; 5] = [
        Chart::Popolazione,
        Chart::Stadi,
        Chart::NasciteMorti,
        Chart::Coppie,
        Chart::Razioni,
    ];

    fn title(self) -> &'static str {
        match self {
            Chart::Popolazione => "Popolazione",
            Chart::Stadi => "Età",
            Chart::NasciteMorti => "Nascite e morti",
            Chart::Coppie => "Coppie ed età media",
            Chart::Razioni => "Razioni",
        }
    }
}

// --- Storia (dati puri) --------------------------------------------------------

/// Stato della popolazione all'inizio di un giorno di gioco.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Sample {
    /// Giorno di gioco (da 1).
    day: u64,
    population: u32,
    max_population: u32,
    beds: u32,
    /// Popolazione per stadio di vita, indicizzata da [`LifeStage::index`].
    stages: [u32; LifeStage::ALL.len()],
    couples: u32,
    founders: u32,
    avg_age: f32,
    /// Razioni nelle Mense per persona.
    razioni_per_person: f32,
}

impl Sample {
    fn of(world: &World) -> Sample {
        let stats = Stats::of(world);
        let population = stats.population;
        Sample {
            day: world.clock.day(),
            population: population as u32,
            max_population: stats.max_population as u32,
            beds: stats.beds as u32,
            stages: stats.stages.map(|n| n as u32),
            couples: stats.couples as u32,
            founders: stats.founders as u32,
            avg_age: stats.avg_age,
            razioni_per_person: world.available(ItemKind::Razione) / population.max(1) as f32,
        }
    }
}

/// Nascite e morti in un anno di gioco.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct YearTotals {
    births: u64,
    deaths: u64,
}

#[derive(Resource, Debug)]
struct PopulationHistory {
    /// Campioni in ordine di giorno.
    samples: Vec<Sample>,
    /// Giorni tra un campione e il successivo (raddoppia a ogni dimezzamento).
    stride: u64,
    /// Ultimo giorno visto (anche se non campionato).
    last_day: Option<u64>,
    /// Nascite e morti per anno (indice = anno - 1).
    yearly: Vec<YearTotals>,
    /// Totali di nascite e morti all'ultimo giorno visto.
    births_seen: u64,
    deaths_seen: u64,
}

impl Default for PopulationHistory {
    fn default() -> Self {
        Self {
            samples: Vec::new(),
            stride: 1,
            last_day: None,
            yearly: Vec::new(),
            births_seen: 0,
            deaths_seen: 0,
        }
    }
}

impl PopulationHistory {
    /// Vero se `day` è un giorno nuovo (la storia va aggiornata).
    fn is_new_day(&self, day: u64) -> bool {
        self.last_day != Some(day)
    }

    /// Registra il giorno `day` (dell'anno `year`) con i totali di nascite e
    /// morti fino ad allora; `sample` dà il campione, calcolato solo se serve.
    fn record(
        &mut self,
        day: u64,
        year: u64,
        births_total: u64,
        deaths_total: u64,
        sample: impl FnOnce() -> Sample,
    ) {
        if self.last_day.is_some_and(|last| day < last) {
            // L'orologio è tornato indietro (nuova partita): si ricomincia.
            *self = Self::default();
        }
        if self.last_day.is_none() {
            // Primo giorno: i totali precedenti non si attribuiscono a nessun anno.
            self.births_seen = births_total;
            self.deaths_seen = deaths_total;
        }
        self.last_day = Some(day);

        let index = year.saturating_sub(1) as usize;
        if self.yearly.len() <= index {
            self.yearly.resize(index + 1, YearTotals::default());
        }
        let totals = &mut self.yearly[index];
        totals.births += births_total.saturating_sub(self.births_seen);
        totals.deaths += deaths_total.saturating_sub(self.deaths_seen);
        self.births_seen = births_total;
        self.deaths_seen = deaths_total;

        let due = self
            .samples
            .last()
            .is_none_or(|last| day >= last.day + self.stride);
        if due {
            if self.samples.len() >= MAX_SAMPLES {
                self.halve();
            }
            self.samples.push(sample());
        }
    }

    /// Tiene un campione ogni due e raddoppia il passo.
    fn halve(&mut self) {
        let mut i = 0;
        self.samples.retain(|_| {
            i += 1;
            i % 2 == 1
        });
        self.stride *= 2;
    }
}

// --- Sistemi --------------------------------------------------------------------

fn toggle_window(keys: Res<ButtonInput<KeyCode>>, mut window: ResMut<PopulationWindow>) {
    if keys.just_pressed(KeyCode::KeyG) {
        window.open = !window.open;
    }
}

/// Mondo sostituito: la serie storica riparte dal mondo caricato.
fn reset_history(mut history: ResMut<PopulationHistory>) {
    *history = PopulationHistory::default();
}

fn sample_population(sim: Res<Sim>, mut history: ResMut<PopulationHistory>) {
    let world = &sim.world;
    let day = world.clock.day();
    if !history.is_new_day(day) {
        return;
    }
    history.record(
        day,
        year_of(world.clock, world.params.days_per_year),
        world.life.births_total,
        world.life.deaths_total,
        || Sample::of(world),
    );
}

fn population_window(
    mut contexts: EguiContexts,
    sim: Option<Res<Sim>>,
    history: Res<PopulationHistory>,
    mut window: ResMut<PopulationWindow>,
) {
    if !window.open {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let mut open = true;
    let center = ctx.content_rect().center();
    egui::Window::new("Popolazione")
        .open(&mut open)
        .default_pos(center - egui::vec2(WINDOW_WIDTH / 2.0, 200.0))
        .default_width(WINDOW_WIDTH)
        .resizable(true)
        .show(ctx, |ui| {
            let Some(sim) = &sim else {
                ui.weak("simulazione non avviata");
                return;
            };
            totals(ui, &sim.world, &history);
            ui.separator();
            ui.horizontal(|ui| {
                for chart in Chart::ALL {
                    ui.selectable_value(&mut window.chart, chart, chart.title());
                }
            });
            chart(ui, &sim.world, &history, window.chart);
        });
    if !open {
        window.open = false;
    }
}

/// Totali della partita e situazione attuale.
fn totals(ui: &mut egui::Ui, world: &World, history: &PopulationHistory) {
    let life = &world.life;
    let year = year_of(world.clock, world.params.days_per_year);
    let now = history.samples.last();
    egui::Grid::new("population_totals")
        .num_columns(2)
        .spacing([12.0, 2.0])
        .show(ui, |ui| {
            if let Some(s) = now {
                ui.weak("Oggi");
                ui.label(format!(
                    "Anno {year}: {} persone su {} cuccette (nascite fino a {})",
                    s.population, s.beds, s.max_population
                ));
                ui.end_row();
                ui.weak("");
                ui.label(format!(
                    "{} fondatori in vita, {} nati sul treno · {} coppie · età media {:.1}",
                    s.founders,
                    s.population.saturating_sub(s.founders),
                    s.couples,
                    s.avg_age
                ));
                ui.end_row();
            }
            ui.weak("In totale");
            ui.label(format!(
                "{} nati · {} morti ({} di vecchiaia, {} di fame)",
                life.births_total,
                life.deaths_total,
                life.deaths_by_cause[DeathCause::OldAge.index()],
                life.deaths_by_cause[DeathCause::Starvation.index()],
            ));
            ui.end_row();
            ui.weak("");
            ui.label(format!(
                "{} nascite negate · {} coppie formate",
                life.births_denied_total, life.couples_formed_total
            ));
            ui.end_row();
        });
}

/// Anno (frazionario, da 1) di un campione: l'anno N va da x = N a x = N + 1.
fn sample_x(sample: &Sample, days_per_year: u32) -> f64 {
    (sample.day - 1) as f64 / f64::from(days_per_year.max(1)) + 1.0
}

fn chart(ui: &mut egui::Ui, world: &World, history: &PopulationHistory, chart: Chart) {
    let dpy = world.params.days_per_year;
    let samples = &history.samples;
    let series = |f: &dyn Fn(&Sample) -> f64| -> Vec<[f64; 2]> {
        samples.iter().map(|s| [sample_x(s, dpy), f(s)]).collect()
    };
    let max_of = |f: &dyn Fn(&Sample) -> f64| samples.iter().map(f).fold(0.0, f64::max);
    // Spazio libero in alto per la legenda, sopra i dati.
    let plot = |top: f64| {
        Plot::new(("population_plot", chart))
            .height(PLOT_HEIGHT)
            .legend(
                Legend::default()
                    .position(Corner::LeftTop)
                    .follow_insertion_order(true),
            )
            .x_axis_label("anno")
            .include_y(0.0)
            .include_y(top * LEGEND_HEADROOM)
            .allow_scroll(false)
    };
    match chart {
        Chart::Popolazione => {
            plot(max_of(&|s| f64::from(s.beds))).show(ui, |plot| {
                plot.line(
                    Line::new("Cuccette", series(&|s| f64::from(s.beds)))
                        .color(BEDS_COLOR)
                        .style(LineStyle::dotted_dense()),
                );
                plot.line(
                    Line::new(
                        "Tetto delle nascite",
                        series(&|s| f64::from(s.max_population)),
                    )
                    .color(LIMIT_COLOR)
                    .style(LineStyle::dashed_loose()),
                );
                plot.line(
                    Line::new("Popolazione", series(&|s| f64::from(s.population)))
                        .color(POPULATION_COLOR)
                        .width(2.0),
                );
            });
        }
        Chart::Stadi => {
            // Aree impilate: bambini in basso, anziani in cima.
            let xs: Vec<f64> = samples.iter().map(|s| sample_x(s, dpy)).collect();
            let mut lower = vec![0.0; samples.len()];
            plot(max_of(&|s| f64::from(s.population))).show(ui, |plot| {
                for stage in LifeStage::ALL {
                    let upper: Vec<f64> = samples
                        .iter()
                        .zip(&lower)
                        .map(|(s, lo)| lo + f64::from(s.stages[stage.index()]))
                        .collect();
                    let color = stage_color(stage);
                    plot.add(
                        FilledArea::new(stage_title(stage), &xs, &lower, &upper)
                            .fill_color(color.gamma_multiply(0.6))
                            .stroke(egui::Stroke::new(1.0, color)),
                    );
                    lower = upper;
                }
            });
        }
        Chart::NasciteMorti => {
            let bars = |offset: f64, f: &dyn Fn(&YearTotals) -> u64| -> Vec<Bar> {
                history
                    .yearly
                    .iter()
                    .enumerate()
                    .map(|(i, y)| Bar::new(i as f64 + 1.5 + offset, f(y) as f64).width(0.4))
                    .collect()
            };
            let top = history
                .yearly
                .iter()
                .map(|y| y.births.max(y.deaths))
                .max()
                .unwrap_or(0);
            plot(top as f64).show(ui, |plot| {
                plot.bar_chart(
                    BarChart::new("Nascite", bars(-0.2, &|y| y.births)).color(BIRTHS_COLOR),
                );
                plot.bar_chart(
                    BarChart::new("Morti", bars(0.2, &|y| y.deaths)).color(DEATHS_COLOR),
                );
            });
        }
        Chart::Coppie => {
            let top = max_of(&|s| f64::from(s.couples).max(f64::from(s.avg_age)));
            plot(top).show(ui, |plot| {
                plot.line(
                    Line::new("Coppie", series(&|s| f64::from(s.couples)))
                        .color(COUPLES_COLOR)
                        .width(2.0),
                );
                plot.line(
                    Line::new("Età media", series(&|s| f64::from(s.avg_age)))
                        .color(AGE_COLOR)
                        .width(2.0),
                );
            });
        }
        Chart::Razioni => {
            let threshold = world.params.birth_min_razioni_per_person;
            let top = max_of(&|s| f64::from(s.razioni_per_person)).max(f64::from(threshold));
            plot(top).show(ui, |plot| {
                plot.hline(
                    HLine::new("Minimo per le nascite", threshold)
                        .color(DEATHS_COLOR)
                        .style(LineStyle::dashed_loose()),
                );
                plot.line(
                    Line::new(
                        "Razioni per persona",
                        series(&|s| f64::from(s.razioni_per_person)),
                    )
                    .color(FOOD_COLOR)
                    .width(2.0),
                );
            });
        }
    }
    if history.stride > 1 {
        ui.label(
            RichText::new(format!("Un campione ogni {} giorni", history.stride))
                .small()
                .weak(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(day: u64) -> Sample {
        Sample {
            day,
            population: 100,
            max_population: 120,
            beds: 124,
            stages: [10, 10, 60, 20],
            couples: 30,
            founders: 50,
            avg_age: 35.0,
            razioni_per_person: 2.0,
        }
    }

    #[test]
    fn samples_once_per_day() {
        let mut h = PopulationHistory::default();
        assert!(h.is_new_day(1));
        h.record(1, 1, 0, 0, || sample(1));
        assert!(!h.is_new_day(1));
        assert!(h.is_new_day(2));
        h.record(2, 1, 0, 0, || sample(2));
        assert_eq!(h.samples.len(), 2);
    }

    #[test]
    fn halves_the_history_when_full_and_keeps_its_span() {
        let mut h = PopulationHistory::default();
        let days = MAX_SAMPLES as u64 * 3;
        for day in 1..=days {
            h.record(day, 1, 0, 0, || sample(day));
        }
        assert!(h.samples.len() <= MAX_SAMPLES);
        assert!(h.samples.len() >= MAX_SAMPLES / 2);
        assert_eq!(h.stride, 4);
        // Sempre dal primo giorno, in ordine, fino a (quasi) oggi.
        assert_eq!(h.samples[0].day, 1);
        assert!(h.samples.windows(2).all(|w| w[0].day < w[1].day));
        assert!(h.samples.last().unwrap().day > days - h.stride);
    }

    #[test]
    fn counts_births_and_deaths_per_year() {
        let mut h = PopulationHistory::default();
        // Si parte con totali già diversi da zero: non contano.
        h.record(1, 1, 5, 2, || sample(1));
        h.record(2, 1, 7, 2, || sample(2));
        h.record(13, 2, 8, 5, || sample(13));
        h.record(14, 2, 10, 5, || sample(14));
        assert_eq!(
            h.yearly,
            [
                YearTotals {
                    births: 2,
                    deaths: 0
                },
                YearTotals {
                    births: 3,
                    deaths: 3
                }
            ]
        );
        // Orologio all'indietro: si ricomincia.
        h.record(1, 1, 0, 0, || sample(1));
        assert_eq!(h.samples.len(), 1);
        assert_eq!(h.yearly, [YearTotals::default()]);
    }

    #[test]
    fn samples_the_real_world() {
        let sim = crate::sim_bridge::new_sim();
        let s = Sample::of(&sim.world);
        assert_eq!(s.day, 1);
        assert_eq!(s.population as usize, sim.world.npcs.len());
        assert_eq!(s.stages.iter().sum::<u32>(), s.population);
        assert!(s.max_population <= s.beds && s.population <= s.beds);
        assert!(s.razioni_per_person > 0.0);
        assert_eq!(sample_x(&s, 12), 1.0);
    }
}
