//! Finestra "Mercato" (tasto M).
//!
//! - Scheda **Mercato**: il Mercato più vicino al giocatore (quello in cui si
//!   trova, se è in un Mercato). Per ogni oggetto in vendita: prezzo, tendenza
//!   rispetto agli ultimi giorni, scorte sullo scaffale, carrozza produttrice
//!   più vicina e distanza, quanti ne ha il giocatore, e i pulsanti Compra
//!   (`World::player_buy`) e Vendi (`World::player_sell`, pagato dalla cassa
//!   del treno a una parte del prezzo). Si compra e si vende solo stando nel
//!   Mercato, con un mercante al bancone.
//! - Scheda **Listino**: tabella oggetti × Mercati, ogni riga colorata dal
//!   Mercato più economico (verde) al più caro (rosso), e sotto il grafico
//!   dello storico dei prezzi di un oggetto nei vari Mercati.
//!
//! Lo storico è campionato una volta al giorno nella sim
//! (`World::price_history`, salvato con il mondo), non qui.

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, RichText};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use egui_plot::{Corner, Legend, Line, Plot, Points};
use sim::{CarriageId, ItemKind, MarketQuote, Trend, World};

use crate::player::Player;
use crate::state::{PlayerInventory, Sim};
use crate::storage::plural_title;
use crate::train::{TrainLayout, TrainLocation};
use crate::ui::{PointerCheck, item_swatch};

const WINDOW_WIDTH: f32 = 560.0;
const PLOT_HEIGHT: f32 = 180.0;

/// Scala dei prezzi del listino: economico, medio, caro.
const CHEAP: Color32 = Color32::from_rgb(110, 190, 110);
const MIDDLE: Color32 = Color32::from_rgb(230, 200, 90);
const EXPENSIVE: Color32 = Color32::from_rgb(230, 80, 80);
/// Tendenza: il prezzo sale (male per chi compra), scende, resta uguale.
const RISING: Color32 = EXPENSIVE;
const FALLING: Color32 = CHEAP;
const STEADY: Color32 = Color32::GRAY;
/// Colori delle linee dei Mercati nel grafico (poi si ricomincia).
const MARKET_COLORS: [Color32; 6] = [
    Color32::from_rgb(120, 200, 240),
    Color32::from_rgb(240, 160, 60),
    Color32::from_rgb(190, 120, 220),
    Color32::from_rgb(110, 190, 110),
    Color32::from_rgb(255, 110, 150),
    Color32::from_rgb(220, 200, 90),
];

pub struct MarketUiPlugin;

impl Plugin for MarketUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MarketWindow>()
            .add_systems(Update, toggle_window)
            .add_systems(EguiPrimaryContextPass, market_window.before(PointerCheck));
    }
}

/// La finestra è aperta (tasto M), con quale scheda e quale grafico.
#[derive(Resource, Default)]
pub(crate) struct MarketWindow {
    pub(crate) open: bool,
    tab: Tab,
    /// Oggetto del grafico dello storico (il primo in vendita se `None`).
    chart_item: Option<ItemKind>,
    /// Esito dell'ultimo acquisto o vendita.
    message: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Mercato,
    Listino,
}

/// Cosa il giocatore ha chiesto con un pulsante.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Trade {
    Buy(ItemKind),
    Sell(ItemKind),
}

// --- Dati puri ----------------------------------------------------------------

/// Colore di `price` nella scala da `min` (verde) a `max` (rosso), passando
/// per il giallo. Se sono tutti uguali, verde.
fn price_color(price: u32, min: u32, max: u32) -> Color32 {
    if max <= min {
        return CHEAP;
    }
    let t = (price.clamp(min, max) - min) as f32 / (max - min) as f32;
    if t <= 0.5 {
        lerp(CHEAP, MIDDLE, t * 2.0)
    } else {
        lerp(MIDDLE, EXPENSIVE, (t - 0.5) * 2.0)
    }
}

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// Freccia e colore di una tendenza.
fn trend_arrow(trend: Trend) -> (&'static str, Color32) {
    match trend {
        Trend::Up => ("⬆", RISING),
        Trend::Down => ("⬇", FALLING),
        Trend::Flat => ("➡", STEADY),
    }
}

/// "qui", "1 carrozza", "3 carrozze".
fn distance_text(distance: u32) -> String {
    match distance {
        0 => "qui".to_string(),
        1 => "1 carrozza".to_string(),
        n => format!("{n} carrozze"),
    }
}

/// Carrozza in cui si trova il giocatore (nel soffietto conta quella davanti).
fn carriage_of(location: TrainLocation) -> CarriageId {
    match location {
        TrainLocation::Carriage(i) | TrainLocation::Gangway(i) => CarriageId(i as u16),
    }
}

/// Perché il giocatore non può comprare `quote` (None: può).
fn buy_blocker(
    quote: &MarketQuote,
    ready: Result<(), &'static str>,
    tokens: u32,
) -> Option<String> {
    if let Err(why) = ready {
        return Some(why.to_string());
    }
    if quote.stock == 0 {
        return Some("Esaurito".to_string());
    }
    (tokens < quote.price).then(|| format!("Servono {} gettoni", quote.price))
}

/// Perché il giocatore non può vendere `quote` (None: può).
fn sell_blocker(
    quote: &MarketQuote,
    ready: Result<(), &'static str>,
    owned: u32,
) -> Option<String> {
    if let Err(why) = ready {
        return Some(why.to_string());
    }
    if owned == 0 {
        return Some(format!("Non hai {}", quote.item.plural()));
    }
    (quote.stock >= quote.cap).then(|| "Lo scaffale è pieno".to_string())
}

/// Se il giocatore, nella carrozza `here`, può trattare con il Mercato `market`.
fn trade_ready(world: &World, market: CarriageId, here: CarriageId) -> Result<(), &'static str> {
    if market != here {
        return Err("Vai al Mercato per comprare o vendere");
    }
    if world.merchant_on_duty(market).is_none() {
        return Err("Nessun mercante al bancone");
    }
    Ok(())
}

/// Esegue un acquisto o una vendita; restituisce l'esito da mostrare.
fn perform(
    world: &mut World,
    inventory: &mut PlayerInventory,
    market: CarriageId,
    trade: Trade,
) -> String {
    match trade {
        Trade::Buy(item) => match world.player_buy(market, item, &mut inventory.tokens) {
            Ok(price) => {
                inventory.add(item, 1);
                format!("Comprato {} per {price} gettoni", item.with_article())
            }
            Err(e) => capitalized(&e.to_string()),
        },
        Trade::Sell(item) => {
            if inventory.count(item) == 0 {
                return format!("Non hai {}", item.with_article());
            }
            match world.player_sell(market, item, &mut inventory.tokens) {
                Ok(pay) => {
                    inventory.remove_one(item);
                    format!("Venduto {} per {pay} gettoni", item.with_article())
                }
                Err(e) => capitalized(&e.to_string()),
            }
        }
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Nome breve di un Mercato: «Il Bazar» (5).
fn market_name(world: &World, market: CarriageId) -> String {
    match world.carriage(market) {
        Some(c) => format!("«{}» ({})", c.name, c.id),
        None => format!("carrozza {market}"),
    }
}

// --- Sistemi --------------------------------------------------------------------

fn toggle_window(keys: Res<ButtonInput<KeyCode>>, mut window: ResMut<MarketWindow>) {
    if keys.just_pressed(KeyCode::KeyM) {
        window.open = !window.open;
    }
}

fn market_window(
    mut contexts: EguiContexts,
    sim: Option<ResMut<Sim>>,
    layout: Option<Res<TrainLayout>>,
    player: Query<&Transform, With<Player>>,
    mut inventory: ResMut<PlayerInventory>,
    mut window: ResMut<MarketWindow>,
) {
    if !window.open {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let here = match (&layout, player.single()) {
        (Some(layout), Ok(transform)) => carriage_of(layout.location_at(transform.translation.x)),
        _ => CarriageId(0),
    };
    let mut sim = sim;
    let mut open = true;
    let mut trade = None;
    let mut market = None;
    let center = ctx.content_rect().center();
    egui::Window::new("Mercato")
        .open(&mut open)
        .default_pos(center - egui::vec2(WINDOW_WIDTH / 2.0, 220.0))
        .default_width(WINDOW_WIDTH)
        .resizable(true)
        .show(ctx, |ui| {
            let Some(sim) = &sim else {
                ui.weak("simulazione non avviata");
                return;
            };
            let world = &sim.world;
            ui.horizontal(|ui| {
                ui.selectable_value(&mut window.tab, Tab::Mercato, "Mercato");
                ui.selectable_value(&mut window.tab, Tab::Listino, "Listino");
            });
            ui.separator();
            let Some(nearest) = world.nearest_market(here) else {
                ui.weak("Su questo treno non ci sono Mercati.");
                return;
            };
            market = Some(nearest);
            match window.tab {
                Tab::Mercato => {
                    trade = market_tab(ui, world, nearest, here, &inventory, &window.message);
                }
                Tab::Listino => {
                    let chart_item = &mut window.chart_item;
                    price_list(ui, world, here, chart_item);
                }
            }
        });
    if let (Some(trade), Some(market), Some(sim)) = (trade, market, sim.as_mut()) {
        let message = perform(&mut sim.world, &mut inventory, market, trade);
        window.message = Some(message);
    }
    if !open {
        window.open = false;
    }
}

/// Scheda "Mercato": restituisce l'acquisto o la vendita chiesti.
fn market_tab(
    ui: &mut egui::Ui,
    world: &World,
    market: CarriageId,
    here: CarriageId,
    inventory: &PlayerInventory,
    message: &Option<String>,
) -> Option<Trade> {
    let ready = trade_ready(world, market, here);
    ui.horizontal(|ui| {
        ui.strong(world.carriage_label(market));
        if market == here {
            ui.label(RichText::new("sei qui").color(CHEAP));
        } else {
            ui.weak(format!("a {}", distance_text(market.distance(here))));
        }
    });
    ui.horizontal(|ui| {
        match world.merchant_on_duty(market) {
            Some(npc) => ui.label(format!("Al bancone: {}", npc.name)),
            None => ui.weak("Nessun mercante al bancone"),
        };
        ui.separator();
        ui.label(format!("Hai {} gettoni", inventory.tokens));
    });
    ui.separator();
    let quotes = world.market_quotes(market);
    let mut trade = None;
    egui::Grid::new("market_grid")
        .num_columns(8)
        .striped(true)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            for header in [
                "Oggetto",
                "Prezzo",
                "",
                "Scaffale",
                "Prodotto in",
                "Distanza",
                "Hai",
                "",
            ] {
                ui.strong(header);
            }
            ui.end_row();
            for q in &quotes {
                ui.horizontal(|ui| {
                    item_swatch(ui, q.item);
                    ui.label(plural_title(q.item));
                });
                ui.strong(format!("{} g", q.price));
                let (arrow, color) = trend_arrow(q.trend);
                let since = match q.reference {
                    Some(r) => format!(
                        "Media degli ultimi {} giorni: {r:.1} gettoni",
                        sim::TREND_DAYS
                    ),
                    None => "Nessuno storico ancora".to_string(),
                };
                ui.label(RichText::new(arrow).color(color))
                    .on_hover_text(since);
                let stock = RichText::new(format!("{}/{}", q.stock, q.cap));
                ui.label(if q.stock == 0 {
                    stock.color(EXPENSIVE)
                } else {
                    stock
                });
                match q.producer {
                    Some(p) => ui.label(world.carriage_label(p)),
                    None => ui.weak("nessuno"),
                };
                ui.label(distance_text(q.distance));
                let owned = inventory.count(q.item);
                ui.label(owned.to_string());
                ui.horizontal(|ui| {
                    let buy = buy_blocker(q, ready, inventory.tokens);
                    let response = ui.add_enabled(buy.is_none(), egui::Button::new("Compra"));
                    if response.clicked() {
                        trade = Some(Trade::Buy(q.item));
                    }
                    if let Some(why) = buy {
                        response.on_disabled_hover_text(why);
                    }
                    let sell = sell_blocker(q, ready, owned);
                    let label = format!("Vendi ({} g)", q.buyback);
                    let response = ui.add_enabled(sell.is_none(), egui::Button::new(label));
                    if response.clicked() {
                        trade = Some(Trade::Sell(q.item));
                    }
                    if let Some(why) = sell {
                        response.on_disabled_hover_text(why);
                    }
                });
                ui.end_row();
            }
        });
    if let Some(message) = message {
        ui.separator();
        ui.label(message);
    }
    ui.separator();
    let share = (world.params.player_sell_share * 100.0).round();
    ui.label(
        RichText::new(format!(
            "Il prezzo cresce con la distanza dal produttore e quando lo scaffale si svuota. \
             Il Mercato ricompra al {share:.0}% del prezzo, pagando dalla cassa del treno."
        ))
        .small()
        .weak(),
    );
    trade
}

/// Scheda "Listino": tabella oggetti × Mercati e storico dei prezzi.
fn price_list(
    ui: &mut egui::Ui,
    world: &World,
    here: CarriageId,
    chart_item: &mut Option<ItemKind>,
) {
    let markets = world.markets();
    let quotes: Vec<Vec<MarketQuote>> = markets.iter().map(|&m| world.market_quotes(m)).collect();
    let items: Vec<ItemKind> = ItemKind::ALL
        .into_iter()
        .filter(|&item| quotes.iter().flatten().any(|q| q.item == item))
        .collect();
    let quote = |k: usize, item: ItemKind| quotes[k].iter().find(|q| q.item == item);
    egui::ScrollArea::horizontal().show(ui, |ui| {
        egui::Grid::new("price_list_grid")
            .num_columns(markets.len() + 1)
            .striped(true)
            .spacing([14.0, 4.0])
            .show(ui, |ui| {
                ui.strong("Oggetto");
                for &m in &markets {
                    let name = RichText::new(market_name(world, m)).strong();
                    let name = if m == here {
                        name.color(Color32::WHITE)
                    } else {
                        name
                    };
                    ui.label(name).on_hover_text(world.carriage_label(m));
                }
                ui.end_row();
                for &item in &items {
                    ui.horizontal(|ui| {
                        item_swatch(ui, item);
                        ui.label(plural_title(item));
                    });
                    let prices: Vec<u32> = (0..markets.len())
                        .filter_map(|k| quote(k, item).map(|q| q.price))
                        .collect();
                    let (min, max) = (
                        prices.iter().copied().min().unwrap_or(0),
                        prices.iter().copied().max().unwrap_or(0),
                    );
                    for k in 0..markets.len() {
                        match quote(k, item) {
                            Some(q) => {
                                let (arrow, _) = trend_arrow(q.trend);
                                let color = price_color(q.price, min, max);
                                let producer = q
                                    .producer
                                    .map_or("nessuno".to_string(), |p| world.carriage_label(p));
                                ui.label(
                                    RichText::new(format!("{} g {arrow}", q.price)).color(color),
                                )
                                .on_hover_text(format!(
                                    "Scaffale {}/{} · prodotto in {producer}, {}",
                                    q.stock,
                                    q.cap,
                                    distance_text(q.distance)
                                ));
                            }
                            None => {
                                ui.weak("·");
                            }
                        }
                    }
                    ui.end_row();
                }
            });
    });
    ui.label(
        RichText::new("Verde: il Mercato più economico per quell'oggetto; rosso: il più caro.")
            .small()
            .weak(),
    );
    ui.separator();

    let Some(&first) = items.first() else {
        return;
    };
    let item = chart_item.filter(|i| items.contains(i)).unwrap_or(first);
    ui.horizontal(|ui| {
        ui.weak("Storico:");
        for &i in &items {
            if ui.selectable_label(i == item, plural_title(i)).clicked() {
                *chart_item = Some(i);
            }
        }
    });
    let series: Vec<(String, Vec<[f64; 2]>)> = markets
        .iter()
        .map(|&m| {
            let points = world
                .price_series(m, item)
                .into_iter()
                .map(|(day, price)| [day as f64, f64::from(price)])
                .collect();
            (market_name(world, m), points)
        })
        .collect();
    let all = || series.iter().flat_map(|(_, points)| points.iter());
    let top = all().map(|p| p[1]).fold(0.0, f64::max);
    let first_day = all().map(|p| p[0]).fold(f64::INFINITY, f64::min);
    let last_day = all().map(|p| p[0]).fold(0.0, f64::max);
    Plot::new(("price_history", item))
        .height(PLOT_HEIGHT)
        .legend(Legend::default().position(Corner::LeftTop))
        .x_axis_label("giorno")
        .y_axis_label("gettoni")
        .include_y(0.0)
        .include_y(top * 1.3)
        .include_x(first_day.min(last_day) - 1.0)
        .include_x(last_day + 1.0)
        .allow_scroll(false)
        .show(ui, |plot| {
            for (k, (name, points)) in series.into_iter().enumerate() {
                let color = MARKET_COLORS[k % MARKET_COLORS.len()];
                // Punti e linea con lo stesso nome: una sola voce in legenda.
                plot.points(
                    Points::new(name.clone(), points.clone())
                        .color(color)
                        .radius(2.5),
                );
                plot.line(Line::new(name, points).color(color).width(2.0));
            }
        });
    ui.label(
        RichText::new(format!(
            "Un prezzo al giorno, a mezzanotte (ultimi {} giorni).",
            world.params.price_history_days
        ))
        .small()
        .weak(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim::{Action, CarriageKind, Job, StationKind};

    #[test]
    fn price_colors_go_from_cheap_to_expensive() {
        assert_eq!(price_color(10, 10, 20), CHEAP);
        assert_eq!(price_color(15, 10, 20), MIDDLE);
        assert_eq!(price_color(20, 10, 20), EXPENSIVE);
        // Fuori scala: ai bordi. Tutti uguali: verde.
        assert_eq!(price_color(5, 10, 20), CHEAP);
        assert_eq!(price_color(99, 10, 20), EXPENSIVE);
        assert_eq!(price_color(12, 12, 12), CHEAP);
        // Più caro, più rosso e meno verde.
        let (a, b) = (price_color(12, 10, 20), price_color(18, 10, 20));
        assert!(b.r() >= a.r() && b.g() < a.g());
    }

    #[test]
    fn trends_and_distances_read_well() {
        assert_eq!(trend_arrow(Trend::Up), ("⬆", RISING));
        assert_eq!(trend_arrow(Trend::Down), ("⬇", FALLING));
        assert_eq!(trend_arrow(Trend::Flat).0, "➡");
        assert_eq!(distance_text(0), "qui");
        assert_eq!(distance_text(1), "1 carrozza");
        assert_eq!(distance_text(4), "4 carrozze");
        assert_eq!(carriage_of(TrainLocation::Gangway(3)), CarriageId(3));
        assert_eq!(carriage_of(TrainLocation::Carriage(7)), CarriageId(7));
    }

    #[test]
    fn buys_and_sells_only_at_a_staffed_market() {
        let mut world = World::generate(42, 20, 400);
        let market = world
            .carriages
            .iter()
            .find(|c| c.kind == CarriageKind::Mercato)
            .unwrap()
            .id;
        let far = CarriageId(0);
        assert!(trade_ready(&world, market, far).is_err());
        assert_eq!(
            trade_ready(&world, market, market),
            Err("Nessun mercante al bancone")
        );
        // Un mercante al primo bancone.
        let counter = world.carriages[market.index()]
            .free_station(StationKind::Counter)
            .unwrap();
        let m = world
            .npcs
            .iter()
            .position(|n| n.job == Some(Job::Mercante))
            .unwrap();
        world.npcs[m].carriage = market;
        world.npcs[m].action = Action::Work(counter);
        world.carriages[market.index()].stations[counter.index()].occupancy += 1;
        let ready = trade_ready(&world, market, market);
        assert_eq!(ready, Ok(()));

        let quote = world
            .market_quotes(market)
            .into_iter()
            .find(|q| q.item == ItemKind::Vestito)
            .unwrap();
        assert_eq!(
            buy_blocker(&quote, ready, 0),
            Some(format!("Servono {} gettoni", quote.price))
        );
        assert_eq!(buy_blocker(&quote, ready, 1000), None);
        assert_eq!(
            sell_blocker(&quote, ready, 0),
            Some("Non hai vestiti".to_string())
        );
        assert_eq!(sell_blocker(&quote, ready, 1), None);

        let mut inv = PlayerInventory {
            tokens: 100,
            ..PlayerInventory::default()
        };
        let supply = world.money_supply();
        let message = perform(&mut world, &mut inv, market, Trade::Buy(ItemKind::Vestito));
        assert!(message.starts_with("Comprato un vestito"), "{message}");
        assert_eq!(inv.count(ItemKind::Vestito), 1);
        let spent = 100 - inv.tokens;
        let message = perform(&mut world, &mut inv, market, Trade::Sell(ItemKind::Vestito));
        assert!(message.starts_with("Venduto un vestito"), "{message}");
        assert_eq!(inv.count(ItemKind::Vestito), 0);
        let earned = inv.tokens + spent - 100;
        assert!(earned > 0 && earned < spent);
        // La moneta della sim cambia solo per quello che il giocatore porta o prende.
        assert_eq!(
            world.money_supply(),
            supply + u64::from(spent) - u64::from(earned)
        );
        // Niente da vendere.
        let message = perform(&mut world, &mut inv, market, Trade::Sell(ItemKind::Vestito));
        assert_eq!(message, "Non hai un vestito");
        assert_eq!(
            market_name(&world, market),
            format!("«{}» ({})", world.carriages[market.index()].name, market)
        );
    }
}
