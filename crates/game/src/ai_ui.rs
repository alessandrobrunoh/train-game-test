//! Il disegno generico delle interfacce inventate dal Narratore.
//!
//! - [`render_panel`] disegna un `narrator::Panel` (testi, valori, barre,
//!   liste, pulsanti) leggendo il mondo in sola lettura a ogni frame: i
//!   valori sono dal vivo, anche su ciò che il Custode ha aggiunto (i nomi si
//!   risolvono sul catalogo del mondo). Un nome che il mondo non ha (una
//!   proposta che entra all'ora fissata, o respinta) si mostra "—" con il
//!   motivo nel suggerimento, e il suo pulsante è spento.
//! - I pulsanti restituiscono un'[`PanelAction`]; il sistema
//!   [`run_panel_actions`] la traduce ([`plan`]) in un'azione del giocatore
//!   che esiste già: aprire crafting (C) sulla ricetta, Mercato (M)
//!   sull'oggetto, inventario (I), la chat con chi fa quel lavoro, la
//!   finestra delle statistiche, o dire dov'è una carrozza (senza
//!   teletrasporto).
//! - La finestra **"Statistiche del treno"** (tasto K): ogni statistica
//!   derivata del mondo (`World::statistics`) con valore, barra sulla sua
//!   scala, testo delle soglie e un piccolo grafico dello storico
//!   (campionato dalla sim ogni ora di gioco e salvato con il mondo).

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, RichText};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use narrator::sources::{self, Reading};
use narrator::{Action, Element, Format, Panel, Source, Statistic};
use sim::{NpcId, RecipeId, World};

use crate::chat::{ChatCommand, ChatQueue};
use crate::crafting::CraftingWindow;
use crate::inventory::InventoryWindow;
use crate::market_ui::MarketWindow;
use crate::ui::{MARGIN, PointerCheck};

/// Cosa chiede un pulsante di un pannello.
pub type PanelAction = Action;

/// Righe mostrate al più in una lista.
const LIST_ROWS: usize = narrator::panel::MAX_ROWS;
const STATS_WIDTH: f32 = 360.0;
const SPARK_SIZE: egui::Vec2 = egui::vec2(150.0, 30.0);
/// Secondi reali di un avviso di un pulsante.
const NOTICE_SECS: f64 = 5.0;
const PENDING_INK: Color32 = Color32::from_rgb(200, 170, 110);
const NOTE_INK: Color32 = Color32::from_rgb(240, 200, 120);
const BAR_FILL: Color32 = Color32::from_rgb(96, 150, 200);
const SPARK_INK: Color32 = Color32::from_rgb(140, 200, 240);

pub struct AiUiPlugin;

impl Plugin for AiUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<StatsWindow>()
            .init_resource::<PanelQueue>()
            .init_resource::<PanelNotice>()
            .init_resource::<CraftingWindow>()
            .init_resource::<MarketWindow>()
            .init_resource::<InventoryWindow>()
            .init_resource::<ChatQueue>()
            .add_systems(Update, (toggle_stats, run_panel_actions))
            .add_systems(
                EguiPrimaryContextPass,
                (stats_window, panel_notice).before(PointerCheck),
            );
    }
}

/// La finestra "Statistiche del treno" (tasto K), con quale statistica in
/// evidenza.
#[derive(Resource, Default)]
pub struct StatsWindow {
    pub open: bool,
    pub focus: Option<String>,
}

/// Azioni chieste dai pulsanti, eseguite nel prossimo `Update`.
#[derive(Resource, Default)]
pub struct PanelQueue(pub Vec<PanelAction>);

/// Avviso di un pulsante ("nessun cuoco qui vicino…").
#[derive(Resource, Default)]
pub struct PanelNotice(pub Option<(String, f64)>);

// --- Dati puri -------------------------------------------------------------------------

/// Cosa fare per un'azione, deciso sul mondo di adesso.
#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    Crafting(RecipeId),
    Market(sim::ItemKind),
    Inventory,
    Chat(NpcId),
    Stat(String),
    Notice(String),
}

/// Se il pulsante si può premere, o perché no (il suggerimento).
pub fn availability(action: &Action, world: &World) -> Result<(), String> {
    let pending = |what: String| Err(format!("{what}: {}", sources::PENDING));
    let cat = world.catalog();
    match action {
        Action::OpenCrafting(r) => match sources::recipe(cat, r) {
            Some(_) => Ok(()),
            None => pending(format!("la ricetta «{r}»")),
        },
        Action::OpenMarket(i) => match sources::item(cat, i) {
            Some(_) => Ok(()),
            None => pending(format!("l'oggetto «{i}»")),
        },
        Action::OpenInventory => Ok(()),
        Action::TalkToJob(j) => match sources::job(cat, j) {
            Some(_) => Ok(()),
            None => pending(format!("il lavoro «{j}»")),
        },
        Action::ShowStat(s) => match world.statistics().get(s) {
            Some(_) => Ok(()),
            None => pending(format!("la statistica «{s}»")),
        },
        Action::GoTo(k) => match sources::carriage_kind(k) {
            Some(_) => Ok(()),
            None => Err(format!("non c'è il tipo di carrozza «{k}»")),
        },
        Action::Invalid(why) => Err(why.clone()),
    }
}

/// L'azione del giocatore per `action`, sul mondo di adesso.
pub fn plan(world: &World, action: &Action) -> Plan {
    if let Err(why) = availability(action, world) {
        return Plan::Notice(why);
    }
    let here = world.player.place.carriage;
    let cat = world.catalog();
    match action {
        Action::OpenCrafting(r) => sources::recipe(cat, r).map_or_else(
            || Plan::Notice(format!("la ricetta «{r}» non c'è")),
            Plan::Crafting,
        ),
        Action::OpenMarket(i) => sources::item(cat, i).map_or_else(
            || Plan::Notice(format!("l'oggetto «{i}» non c'è")),
            Plan::Market,
        ),
        Action::OpenInventory => Plan::Inventory,
        Action::TalkToJob(name) => {
            let Some(job) = sources::job(cat, name) else {
                return Plan::Notice(format!("il lavoro «{name}» non c'è"));
            };
            let mut workers: Vec<&sim::Npc> =
                world.npcs.iter().filter(|n| n.job == Some(job)).collect();
            workers.sort_by_key(|n| (n.carriage.distance(here), n.id));
            if let Some(n) = workers.iter().find(|n| world.can_chat(n.id).is_ok()) {
                return Plan::Chat(n.id);
            }
            match workers.first() {
                Some(n) => Plan::Notice(format!(
                    "Nessun {} qui vicino: {} è in {}.",
                    job.name(),
                    n.name,
                    world.carriage_label(n.carriage)
                )),
                None => Plan::Notice(format!("Nessuno fa il {} sul treno.", job.name())),
            }
        }
        Action::ShowStat(name) => Plan::Stat(name.clone()),
        Action::GoTo(name) => {
            let Some(kind) = sources::carriage_kind(name) else {
                return Plan::Notice(format!("non c'è il tipo di carrozza «{name}»"));
            };
            if world.carriage(here).is_some_and(|c| c.kind == kind) {
                return Plan::Notice(format!("Sei già in {}.", world.carriage_label(here)));
            }
            let nearest = world
                .carriages
                .iter()
                .filter(|c| c.kind == kind)
                .min_by_key(|c| (c.id.distance(here), c.id.index()));
            match nearest {
                Some(c) => {
                    let n = c.id.distance(here);
                    let side = if c.id.index() < here.index() {
                        "a sinistra"
                    } else {
                        "a destra"
                    };
                    let steps = if n == 1 {
                        "1 carrozza".to_string()
                    } else {
                        format!("{n} carrozze")
                    };
                    Plan::Notice(format!(
                        "{} è a {steps} {side}.",
                        world.carriage_label(c.id)
                    ))
                }
                None => Plan::Notice(format!("Su questo treno non c'è una {}.", kind.name())),
            }
        }
        Action::Invalid(why) => Plan::Notice(why.clone()),
    }
}

fn plain(v: f32) -> String {
    if (v - v.round()).abs() < 0.005 {
        format!("{v:.0}")
    } else if v.abs() < 10.0 {
        format!("{v:.2}")
    } else {
        format!("{v:.1}")
    }
}

/// Un valore nel suo formato: le frazioni (bisogni, riempimento,
/// affinità) in percentuale si moltiplicano per 100.
pub fn format_value(v: f32, format: Format, source: &Source) -> String {
    match format {
        Format::Number => plain(v),
        Format::Percent if source.is_fraction() => format!("{:.0}%", v * 100.0),
        Format::Percent => format!("{:.0}%", v),
        Format::Tokens => format!("{:.0} gettoni", v),
    }
}

/// Il valore di una barra sulla sua scala: una frazione su una scala più
/// grande di 1 si legge in centesimi (il modello scrive spesso 0–100).
pub fn bar_value(v: f32, source: &Source, min: f32, max: f32) -> f32 {
    let v = if source.is_fraction() && max > 1.5 {
        v * 100.0
    } else {
        v
    };
    ((v - min) / (max - min)).clamp(0.0, 1.0)
}

// --- Disegno ------------------------------------------------------------------------------

fn dash(ui: &mut egui::Ui, reading: &Reading) {
    let r = ui.label(RichText::new("—").color(PENDING_INK).strong());
    if let Some(why) = reading.why() {
        r.on_hover_text(why);
    }
}

/// Disegna `panel` con i valori di adesso; restituisce il pulsante premuto.
pub fn render_panel(ui: &mut egui::Ui, panel: &Panel, world: &World) -> Option<PanelAction> {
    let mut pressed = None;
    let stats = world.statistics().book();
    let book = Some(&stats);
    egui::Frame::group(ui.style())
        .fill(ui.visuals().extreme_bg_color)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(RichText::new(&panel.title).strong());
            for (i, element) in panel.elements.iter().enumerate() {
                ui.push_id(i, |ui| match element {
                    Element::Text { text } => {
                        ui.add(egui::Label::new(RichText::new(text).italics()).wrap());
                    }
                    Element::Value {
                        label,
                        source,
                        format,
                    } => {
                        ui.horizontal(|ui| {
                            ui.label(label).on_hover_text(source.to_string());
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| match source.read(world, book) {
                                    Reading::Value(v) => {
                                        // Una statistica "numero" con la sua unità.
                                        let text = match (source, format) {
                                            (Source::Stat { name }, Format::Number) => stats
                                                .get(name)
                                                .map_or_else(|| plain(v), |s| s.format(v)),
                                            _ => format_value(v, *format, source),
                                        };
                                        ui.strong(text);
                                    }
                                    other => dash(ui, &other),
                                },
                            );
                        });
                    }
                    Element::Bar {
                        label,
                        source,
                        min,
                        max,
                    } => {
                        ui.label(label).on_hover_text(source.to_string());
                        match source.read(world, book) {
                            Reading::Value(v) => {
                                let f = bar_value(v, source, *min, *max);
                                let shown = min + f * (max - min);
                                ui.add(
                                    egui::ProgressBar::new(f)
                                        .fill(BAR_FILL)
                                        .desired_height(14.0)
                                        .text(format!(
                                            "{} ({}–{})",
                                            plain(shown),
                                            plain(*min),
                                            plain(*max)
                                        )),
                                );
                            }
                            other => dash(ui, &other),
                        }
                    }
                    Element::List { label, source } => {
                        ui.label(label).on_hover_text(source.to_string());
                        match source.rows(world, LIST_ROWS) {
                            Ok(rows) if rows.rows.is_empty() => {
                                ui.weak("  nessuno");
                            }
                            Ok(rows) => {
                                egui::Grid::new("rows")
                                    .num_columns(2)
                                    .spacing([12.0, 1.0])
                                    .show(ui, |ui| {
                                        for row in &rows.rows {
                                            ui.label(&row.label);
                                            ui.weak(&row.value);
                                            ui.end_row();
                                        }
                                    });
                                if rows.more > 0 {
                                    ui.weak(format!("  e altri {}", rows.more));
                                }
                            }
                            Err(reading) => dash(ui, &reading),
                        }
                    }
                    Element::Button { label, action } => {
                        let ok = availability(action, world);
                        let button = ui.add_enabled(ok.is_ok(), egui::Button::new(label));
                        match ok {
                            Ok(()) => {
                                if button.on_hover_text(action.to_string()).clicked() {
                                    pressed = Some(action.clone());
                                }
                            }
                            Err(why) => {
                                button.on_disabled_hover_text(why);
                            }
                        }
                    }
                    Element::Unknown => {
                        ui.weak("(elemento sconosciuto)");
                    }
                });
            }
        });
    pressed
}

/// Un piccolo grafico dello storico sulla scala `[lo, hi]`.
fn sparkline(ui: &mut egui::Ui, points: &[(u64, f32)], lo: f32, hi: f32) {
    let (rect, response) = ui.allocate_exact_size(SPARK_SIZE, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    if points.len() < 2 || hi <= lo {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "storico in arrivo",
            egui::FontId::proportional(10.0),
            Color32::GRAY,
        );
        return;
    }
    let (t0, t1) = (points[0].0, points[points.len() - 1].0.max(points[0].0 + 1));
    let line: Vec<egui::Pos2> = points
        .iter()
        .map(|&(t, v)| {
            let x = (t - t0) as f32 / (t1 - t0) as f32;
            let y = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
            egui::pos2(
                rect.left() + 2.0 + x * (rect.width() - 4.0),
                rect.bottom() - 2.0 - y * (rect.height() - 4.0),
            )
        })
        .collect();
    painter.add(egui::Shape::line(line, egui::Stroke::new(1.5, SPARK_INK)));
    let hours = t1 - t0;
    response.on_hover_text(format!(
        "Ultime {hours} ore di gioco ({} campioni)",
        points.len()
    ));
}

/// Una statistica: valore, barra, soglie, storico.
fn stat_row(ui: &mut egui::Ui, stat: &Statistic, world: &World, focus: bool) {
    let frame = egui::Frame::group(ui.style());
    let frame = if focus {
        frame.stroke(egui::Stroke::new(1.5, NOTE_INK))
    } else {
        frame
    };
    frame.show(ui, |ui| {
        ui.set_width(STATS_WIDTH - 24.0);
        let reading = world.statistics().book().read(&stat.name, world);
        ui.horizontal(|ui| {
            ui.label(RichText::new(&stat.name).strong().size(15.0))
                .on_hover_text(format!("Formula: {}", stat.formula));
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| match &reading {
                    Reading::Value(v) => {
                        ui.label(RichText::new(stat.format(*v)).strong().size(15.0));
                    }
                    other => dash(ui, other),
                },
            );
        });
        ui.add(egui::Label::new(RichText::new(&stat.description).weak().small()).wrap());
        let [lo, hi] = stat.scale;
        if let Reading::Value(v) = reading {
            ui.add(
                egui::ProgressBar::new(((v - lo) / (hi - lo)).clamp(0.0, 1.0))
                    .fill(BAR_FILL)
                    .desired_height(10.0),
            );
            for note in stat.notes(v) {
                ui.colored_label(NOTE_INK, note);
            }
        }
        ui.horizontal(|ui| {
            let history: Vec<(u64, f32)> = world.statistics().history(&stat.name).collect();
            sparkline(ui, &history, lo, hi);
            ui.weak(format!("scala {}–{}", plain(lo), plain(hi)));
        });
    });
}

// --- Sistemi ---------------------------------------------------------------------------------

fn toggle_stats(keys: Res<ButtonInput<KeyCode>>, mut window: ResMut<StatsWindow>) {
    if keys.just_pressed(KeyCode::KeyK) {
        window.open = !window.open;
        if !window.open {
            window.focus = None;
        }
    }
}

fn stats_window(
    mut contexts: EguiContexts,
    sim: Res<crate::state::Sim>,
    mut window: ResMut<StatsWindow>,
) {
    if !window.open {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let mut open = true;
    let center = ctx.content_rect().center();
    let max_height = (ctx.content_rect().height() - 220.0).max(200.0);
    egui::Window::new("Statistiche del treno")
        .id(egui::Id::new("stats_window"))
        .default_pos(center + egui::vec2(40.0, -300.0))
        .default_width(STATS_WIDTH)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.set_width(STATS_WIDTH);
            let stats = sim.world.statistics();
            if stats.is_empty() {
                ui.weak(
                    "Nessuna statistica ancora. Il Narratore ne inventa una quando il treno \
                     ha una tensione che nessun numero misura (N: cronaca).",
                );
                return;
            }
            ui.weak("Numeri inventati dal Narratore, calcolati dal treno ogni ora.");
            egui::ScrollArea::vertical()
                .max_height(max_height)
                .min_scrolled_height(max_height)
                .show(ui, |ui| {
                    for stat in stats.iter() {
                        let focus = window.focus.as_deref().is_some_and(|f| {
                            narrator::guard::normalize(f) == narrator::guard::normalize(&stat.name)
                        });
                        stat_row(ui, stat, &sim.world, focus);
                    }
                });
        });
    if !open {
        window.open = false;
        window.focus = None;
    }
}

#[allow(clippy::too_many_arguments)]
fn run_panel_actions(
    time: Res<Time<Real>>,
    sim: Res<crate::state::Sim>,
    mut queue: ResMut<PanelQueue>,
    mut notice: ResMut<PanelNotice>,
    mut crafting: ResMut<CraftingWindow>,
    mut market: ResMut<MarketWindow>,
    mut inventory: ResMut<InventoryWindow>,
    mut chat: ResMut<ChatQueue>,
    mut stats_window: ResMut<StatsWindow>,
) {
    if queue.0.is_empty() {
        return;
    }
    for action in std::mem::take(&mut queue.0) {
        match plan(&sim.world, &action) {
            Plan::Crafting(recipe) => {
                crafting.open = true;
                crafting.focus = Some(recipe);
            }
            Plan::Market(item) => market.show_item(item),
            Plan::Inventory => inventory.open = true,
            Plan::Chat(id) => chat.0.push(ChatCommand::Open(id)),
            Plan::Stat(name) => {
                stats_window.open = true;
                stats_window.focus = Some(name);
            }
            Plan::Notice(text) => notice.0 = Some((text, time.elapsed_secs_f64())),
        }
    }
}

fn panel_notice(
    mut contexts: EguiContexts,
    time: Res<Time<Real>>,
    mut notice: ResMut<PanelNotice>,
) {
    let Some((text, at)) = &notice.0 else {
        return;
    };
    if time.elapsed_secs_f64() - at > NOTICE_SECS {
        notice.0 = None;
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    egui::Area::new(egui::Id::new("panel_notice"))
        .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -80.0 - MARGIN])
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.colored_label(NOTE_INK, text.as_str());
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim::{CarriageKind, ItemKind, Job};

    fn world() -> World {
        World::generate(9, 10, 80)
    }

    fn action(json: &str) -> Action {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn buttons_map_to_player_actions() {
        let mut world = world();
        let morale: narrator::Draft =
            narrator::parse(crate::narrator_bridge::tests::MORALE).unwrap();
        let now = world.clock;
        world.apply(&morale, now).expect("statistic applied");
        let coperta = world.catalog().recipe_by_key("coperta").unwrap();
        assert_eq!(
            plan(&world, &action(r#"{"apri_crafting": "coperta"}"#)),
            Plan::Crafting(coperta)
        );
        assert_eq!(
            plan(&world, &action(r#"{"apri_mercato": "Tè"}"#)),
            Plan::Market(ItemKind::Te)
        );
        assert_eq!(
            plan(&world, &action(r#"{"apri_inventario": true}"#)),
            Plan::Inventory
        );
        assert_eq!(
            plan(&world, &action(r#"{"mostra_statistica": "morale"}"#)),
            Plan::Stat("morale".into())
        );
        // Dov'è la Serra più vicina: un avviso, niente teletrasporto.
        let Plan::Notice(text) = plan(&world, &action(r#"{"vai_a": "Serre"}"#)) else {
            panic!()
        };
        let here = world.player.place.carriage;
        let in_serra = world
            .carriage(here)
            .is_some_and(|c| c.kind == CarriageKind::Serra);
        assert!(
            if in_serra {
                text.contains("Sei già")
            } else {
                text.contains("carrozz")
            },
            "{text}"
        );
        // Parlare con un cuoco: la chat se ce n'è uno vicino, se no dove sta.
        match plan(&world, &action(r#"{"parla_con_lavoro": "cuoco"}"#)) {
            Plan::Chat(id) => {
                assert_eq!(world.npc(id).unwrap().job, Some(Job::Cuoco));
                assert!(world.can_chat(id).is_ok());
            }
            Plan::Notice(text) => assert!(text.contains("cuoco"), "{text}"),
            other => panic!("{other:?}"),
        }
        // Cose che il mondo non ha: spente, con il motivo.
        let pending = [
            r#"{"apri_crafting": "Scialle di stracci"}"#,
            r#"{"apri_mercato": "Borraccia"}"#,
            r#"{"parla_con_lavoro": "Erborista"}"#,
            r#"{"mostra_statistica": "Nebbia"}"#,
        ];
        for p in pending {
            let why = availability(&action(p), &world).unwrap_err();
            assert!(why.contains("non ancora nel mondo"), "{p}: {why}");
            assert!(matches!(plan(&world, &action(p)), Plan::Notice(_)));
        }
        assert!(availability(&action(r#"{"vai_a": "Stiva"}"#), &world).is_err());
        // Once the Custode applies them, the same buttons work.
        let borraccia: narrator::Draft = serde_json::from_str(
            r#"{"motivo": "Serve acqua in coda.", "novita": {"tipo": "oggetto",
            "nome": "Borraccia", "descrizione": "Una borraccia di latta.",
            "categoria": "durevole", "valore": 8, "ingredienti": [{"oggetto": "metallo",
            "qta": 1}], "lavoro": "operaio"}}"#,
        )
        .unwrap();
        world.apply(&borraccia, now).expect("item applied");
        assert!(availability(&action(pending[1]), &world).is_ok());
        let Plan::Market(item) = plan(&world, &action(pending[1])) else {
            panic!()
        };
        assert_eq!(item.name(), "borraccia");
        assert!(matches!(
            plan(&world, &action(r#"{"apri_crafting": "borraccia"}"#)),
            Plan::Crafting(_)
        ));
    }

    #[test]
    fn values_are_formatted() {
        let need: Source = serde_json::from_str(r#"{"bisogno": "energia"}"#).unwrap();
        let stock: Source = serde_json::from_str(r#"{"scorta": "verdura"}"#).unwrap();
        assert_eq!(format_value(0.634, Format::Percent, &need), "63%");
        assert_eq!(format_value(63.0, Format::Percent, &stock), "63%");
        assert_eq!(format_value(12.0, Format::Number, &stock), "12");
        assert_eq!(format_value(0.25, Format::Number, &need), "0.25");
        assert_eq!(format_value(7.0, Format::Tokens, &stock), "7 gettoni");
        assert!((bar_value(0.5, &need, 0.0, 1.0) - 0.5).abs() < 1e-6);
        assert!((bar_value(0.5, &need, 0.0, 100.0) - 0.5).abs() < 1e-6);
        assert_eq!(bar_value(500.0, &stock, 0.0, 100.0), 1.0);
    }
}
