//! Finestra "Crafting" (tasto C): le ricette che il giocatore può fare nella
//! carrozza dove si trova, con gli ingredienti che ha e quelli che servono.
//!
//! Le ricette sono le stesse degli NPC (`sim::RECIPES`) e si fanno solo dove
//! c'è la loro postazione (aiuole in Serra, cucina in Mensa, banchi da lavoro
//! in Officina). Quelle di base si conoscono dall'inizio; le altre si
//! imparano (più avanti, parlando con gli NPC) e intanto compaiono come "da
//! imparare".
//!
//! Con "Crea" il giocatore lavora per i minuti di gioco della ricetta: il
//! tempo scorre alla velocità scelta e si ferma in pausa. Alla fine la sim
//! controlla postazione e ingredienti (`World::player_craft`), li consuma e
//! mette il risultato nell'inventario. Uscire dalla carrozza o caricare
//! un'altra partita annulla il lavoro senza perdere niente: gli ingredienti
//! si consumano solo alla fine.

use bevy::prelude::*;
use bevy_egui::egui::{self, Align2, Color32, RichText};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use sim::{CarriageId, GameTime, ItemKind, RECIPES, RecipeDef, World};

use crate::player::Player;
use crate::state::{PlayerInventory, Sim, WorldReplaced};
use crate::train::{TrainLayout, TrainLocation};
use crate::ui::{MARGIN, PointerCheck, item_swatch};

const HAVE: Color32 = Color32::from_rgb(110, 190, 110);
const MISSING: Color32 = Color32::from_rgb(230, 80, 80);
/// Larghezza della finestra.
const WIDTH: f32 = 330.0;

/// Stato della finestra e del lavoro in corso.
#[derive(Resource, Default)]
pub(crate) struct CraftingWindow {
    pub(crate) open: bool,
    job: Option<CraftJob>,
    /// Esito dell'ultimo lavoro (o perché non è partito).
    message: Option<String>,
}

/// Un lavoro al banco: finisce a `until` (tempo di gioco).
#[derive(Clone, Copy, Debug)]
struct CraftJob {
    recipe: &'static RecipeDef,
    carriage: CarriageId,
    since: GameTime,
    until: GameTime,
}

pub struct CraftingPlugin;

impl Plugin for CraftingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CraftingWindow>()
            .add_systems(
                Update,
                (
                    toggle_crafting,
                    cancel_on_new_world.run_if(on_message::<WorldReplaced>),
                    advance_job,
                )
                    .chain(),
            )
            .add_systems(EguiPrimaryContextPass, crafting_window.before(PointerCheck));
    }
}

// --- Dati puri ------------------------------------------------------------------

/// Carrozza in cui si trova il giocatore (nei soffietti nessuna).
fn player_carriage(layout: &TrainLayout, world: &World, x: f32) -> Option<CarriageId> {
    match layout.location_at(x) {
        TrainLocation::Carriage(i) => world.carriages.get(i).map(|c| c.id),
        TrainLocation::Gangway(_) => None,
    }
}

/// Prima lettera maiuscola.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// "Coperta", "3 Tè": il nome giusto per `n` unità.
fn amount_label(item: ItemKind, n: u32) -> String {
    if n == 1 {
        capitalized(item.name())
    } else {
        format!("{n} {}", capitalized(item.plural()))
    }
}

/// Perché il giocatore non può iniziare `recipe` adesso (None: può).
fn blocker(
    world: &World,
    inventory: &PlayerInventory,
    recipe: &RecipeDef,
    here: Option<CarriageId>,
    busy: bool,
) -> Option<String> {
    if !inventory.knows(recipe) {
        return Some("Non conosci ancora questa ricetta".to_string());
    }
    let here_ok = here
        .and_then(|c| world.carriage(c))
        .is_some_and(|c| c.stations.iter().any(|s| s.kind == recipe.station));
    if !here_ok {
        return Some(format!("Serve {}", station_place(recipe)));
    }
    for (item, needed) in recipe.player_inputs(&world.params) {
        let have = inventory.count(item);
        if have < needed {
            let missing = needed - have;
            return Some(if missing == 1 {
                format!("Ti manca 1 {}", item.name())
            } else {
                format!("Ti mancano {missing} {}", item.plural())
            });
        }
    }
    busy.then(|| "Stai già lavorando".to_string())
}

/// "un banco da lavoro (Officina)".
fn station_place(recipe: &RecipeDef) -> String {
    let places: Vec<&str> = recipe.places().map(|k| k.name()).collect();
    format!("{} ({})", recipe.station.name(), places.join(", "))
}

/// Il lavoro è finito (o annullato): chiede alla sim di fare la ricetta.
fn finish(world: &mut World, inventory: &mut PlayerInventory, job: &CraftJob) -> String {
    match world.player_craft(job.recipe, &mut inventory.items, job.carriage) {
        Ok(n) => format!("Fatto: {}", amount_label(job.recipe.output, n)),
        Err(e) => format!("Non riuscito: {e}"),
    }
}

// --- Sistemi --------------------------------------------------------------------

fn toggle_crafting(keys: Res<ButtonInput<KeyCode>>, mut window: ResMut<CraftingWindow>) {
    if keys.just_pressed(KeyCode::KeyC) {
        window.open = !window.open;
    }
}

fn cancel_on_new_world(mut window: ResMut<CraftingWindow>) {
    if window.job.take().is_some() {
        window.message = Some("Lavoro annullato".to_string());
    }
}

/// Fa avanzare il lavoro: finisce all'ora giusta, si annulla se il
/// giocatore lascia la carrozza.
fn advance_job(
    mut sim: ResMut<Sim>,
    layout: Res<TrainLayout>,
    player: Single<&Transform, With<Player>>,
    mut inventory: ResMut<PlayerInventory>,
    mut window: ResMut<CraftingWindow>,
) {
    let Some(job) = window.job else {
        return;
    };
    let here = player_carriage(&layout, &sim.world, player.translation.x);
    if here != Some(job.carriage) {
        window.job = None;
        window.message = Some("Lavoro interrotto: hai lasciato la carrozza".to_string());
        return;
    }
    if sim.world.clock < job.until {
        return;
    }
    window.job = None;
    window.message = Some(finish(&mut sim.world, &mut inventory, &job));
}

#[allow(clippy::too_many_arguments)]
fn crafting_window(
    mut contexts: EguiContexts,
    sim: Option<Res<Sim>>,
    layout: Res<TrainLayout>,
    player: Single<&Transform, With<Player>>,
    inventory: Res<PlayerInventory>,
    mut window: ResMut<CraftingWindow>,
) {
    if !window.open {
        return;
    }
    let Some(sim) = sim else {
        return;
    };
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let world = &sim.world;
    let here = player_carriage(&layout, world, player.translation.x);
    let mut open = true;
    let mut start = None;
    let mut cancel = false;
    let max_height = (ctx.content_rect().height() - 4.0 * MARGIN - 80.0).max(160.0);
    egui::Window::new("Crafting")
        .anchor(Align2::LEFT_BOTTOM, [MARGIN, -40.0])
        .default_width(WIDTH)
        .resizable(false)
        .collapsible(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.set_width(WIDTH);
            match here.and_then(|c| world.carriage(c)) {
                Some(c) => ui.label(format!("Sei in {}", c.label())),
                None => ui.weak("Non sei in una carrozza"),
            };
            if let Some(job) = window.job {
                let total = job.until.since(job.since).max(1);
                let done = world.clock.since(job.since).min(total);
                ui.horizontal(|ui| {
                    ui.add(
                        egui::ProgressBar::new(done as f32 / total as f32)
                            .desired_width(WIDTH - 90.0)
                            .text(format!(
                                "{}: {} min",
                                capitalized(job.recipe.name),
                                total - done
                            )),
                    );
                    if ui.button("Annulla").clicked() {
                        cancel = true;
                    }
                });
            }
            if let Some(message) = &window.message {
                ui.colored_label(Color32::from_rgb(240, 220, 140), message);
            }
            ui.separator();

            let local: Vec<&'static RecipeDef> = here.map_or(Vec::new(), |c| world.recipes_at(c));
            egui::ScrollArea::vertical()
                .max_height(max_height)
                .show(ui, |ui| {
                    if local.is_empty() {
                        ui.weak(
                            "Qui non c'è una postazione per lavorare: \
                             cerca una Serra, una Mensa o un'Officina.",
                        );
                    }
                    for &recipe in &local {
                        if recipe_row(ui, world, &inventory, recipe, here, window.job.is_some()) {
                            start = Some(recipe);
                        }
                    }
                    let elsewhere: Vec<&RecipeDef> = RECIPES
                        .iter()
                        .filter(|r| !local.iter().any(|l| l.key == r.key))
                        .collect();
                    if !elsewhere.is_empty() {
                        egui::CollapsingHeader::new(format!("Altre ricette ({})", elsewhere.len()))
                            .default_open(local.is_empty())
                            .show(ui, |ui| {
                                for recipe in elsewhere {
                                    recipe_row(ui, world, &inventory, recipe, here, true);
                                }
                            });
                    }
                });
        });
    if !open {
        window.open = false;
    }
    if cancel {
        window.job = None;
        window.message = Some("Lavoro annullato".to_string());
    }
    if let (Some(recipe), Some(carriage)) = (start, here) {
        let minutes = recipe.player_minutes(&world.params);
        window.job = Some(CraftJob {
            recipe,
            carriage,
            since: world.clock,
            until: world.clock + minutes,
        });
        window.message = None;
    }
}

/// Una ricetta: risultato, ingredienti (posseduti / necessari), tempo e il
/// pulsante "Crea". Restituisce vero se è stato premuto.
fn recipe_row(
    ui: &mut egui::Ui,
    world: &World,
    inventory: &PlayerInventory,
    recipe: &'static RecipeDef,
    here: Option<CarriageId>,
    busy: bool,
) -> bool {
    let p = &world.params;
    let known = inventory.knows(recipe);
    let blocked = blocker(world, inventory, recipe, here, busy);
    let mut clicked = false;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(WIDTH - 24.0);
        ui.horizontal(|ui| {
            item_swatch(ui, recipe.output);
            let title = if known {
                format!(
                    "{} → {}",
                    capitalized(recipe.name),
                    amount_label(recipe.output, recipe.batch)
                )
            } else {
                format!("??? → {}", amount_label(recipe.output, recipe.batch))
            };
            ui.strong(title).on_hover_text(recipe.output.description());
        });
        ui.horizontal_wrapped(|ui| {
            let inputs = recipe.player_inputs(p);
            if inputs.is_empty() {
                ui.weak("Niente ingredienti");
            }
            for (item, needed) in inputs {
                let have = inventory.count(item);
                item_swatch(ui, item);
                ui.label(
                    RichText::new(format!("{} {have}/{needed}", capitalized(item.name())))
                        .color(if have >= needed { HAVE } else { MISSING }),
                )
                .on_hover_text(item.description());
            }
        });
        ui.horizontal(|ui| {
            ui.weak(format!(
                "{} min · {}",
                recipe.player_minutes(p),
                station_place(recipe)
            ));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let button = ui.add_enabled(blocked.is_none(), egui::Button::new("Crea"));
                clicked = button.clicked();
                if let Some(why) = &blocked {
                    button.on_disabled_hover_text(why);
                }
            });
        });
        if !known {
            ui.weak("Da imparare: qualcuno sul treno potrebbe insegnartela.");
        }
    });
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim::{CarriageKind, Stock};

    fn first(world: &World, kind: CarriageKind) -> CarriageId {
        world.carriages.iter().find(|c| c.kind == kind).unwrap().id
    }

    fn inventory(items: &[(ItemKind, f32)]) -> PlayerInventory {
        let mut stock = Stock::default();
        for &(item, n) in items {
            stock.set(item, n);
        }
        PlayerInventory {
            items: stock,
            ..PlayerInventory::default()
        }
    }

    #[test]
    fn blockers_explain_what_is_missing() {
        let world = World::generate(3, 10, 40);
        let officina = first(&world, CarriageKind::Officina);
        let mensa = first(&world, CarriageKind::Mensa);
        let coperta = RecipeDef::by_key("coperta").unwrap();
        let lampada = RecipeDef::by_key("lampada").unwrap();
        let mut inv = inventory(&[(ItemKind::Tessuto, 2.0)]);
        assert_eq!(blocker(&world, &inv, coperta, Some(officina), false), None);
        assert!(
            blocker(&world, &inv, coperta, Some(officina), true)
                .unwrap()
                .contains("lavorando")
        );
        let wrong = blocker(&world, &inv, coperta, Some(mensa), false).unwrap();
        assert!(wrong.contains("banco da lavoro"), "{wrong}");
        assert!(wrong.contains("Officina"), "{wrong}");
        inv.items.set(ItemKind::Tessuto, 1.0);
        let missing = blocker(&world, &inv, coperta, Some(officina), false).unwrap();
        assert_eq!(missing, "Ti manca 1 tessuto");
        // Not basic: must be learnt first.
        let unknown = blocker(&world, &inv, lampada, Some(officina), false).unwrap();
        assert!(unknown.contains("conosci"), "{unknown}");
        assert!(inv.learn(lampada));
        assert!(!inv.learn(lampada));
        assert!(inv.knows(lampada));
    }

    #[test]
    fn finishing_a_job_crafts_through_the_sim() {
        let mut world = World::generate(3, 10, 40);
        let officina = first(&world, CarriageKind::Officina);
        let mut inv = inventory(&[(ItemKind::Tessuto, 2.0)]);
        let job = CraftJob {
            recipe: RecipeDef::by_key("coperta").unwrap(),
            carriage: officina,
            since: world.clock,
            until: world.clock + 60,
        };
        assert_eq!(finish(&mut world, &mut inv, &job), "Fatto: Coperta");
        assert_eq!(inv.count(ItemKind::Coperta), 1);
        assert_eq!(inv.count(ItemKind::Tessuto), 0);
        let again = finish(&mut world, &mut inv, &job);
        assert!(again.starts_with("Non riuscito"), "{again}");
        assert_eq!(amount_label(ItemKind::Te, 3), "3 Tè");
    }
}
