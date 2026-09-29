//! Il giocatore come personaggio: posto nella sim, cabina, baule e sonno
//! (Fase 4 di `docs/piano-vita-ed-economia.md`).
//!
//! - **Posto.** Ogni frame la carrozza e il piano del giocatore passano alla
//!   sim (`World::set_player_place`): gli NPC lo vedono, lo salutano e
//!   notano i suoi furti. La fisica resta in `player.rs`.
//! - **Cabina.** Un letto privato al piano di sopra del primo Dormitorio,
//!   separato da un paravento, con il baule contro la parete (disegnati in
//!   `stations.rs`). Nessun NPC ci dorme.
//! - **Baule.** E sul baule apre la finestra "Baule": un clic su uno
//!   scomparto sposta la pila dall'inventario al baule o viceversa
//!   (`World::player_store` / `player_retrieve`). Si chiude allontanandosi.
//! - **Sonno.** E sul letto dalle 20:00 (`World::player_go_to_bed`): il tempo
//!   corre a [`SLEEP_SPEED`] minuti di gioco al secondo, anche in pausa, fino
//!   alle 06:00 (vedi `sim_bridge.rs`), con lo schermo scuro e l'ora; al
//!   risveglio la partita si salva da sola. "Svegliati" interrompe.

use bevy::prelude::*;
use bevy_egui::egui::{self, Align2, Color32, RichText};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use sim::GameTime;

use crate::inventory::slot_grid;
use crate::player::Player;
use crate::saves::{SaveCommand, SaveQueue, WorldRebuildSet};
use crate::sim_bridge::SimTickSet;
use crate::state::{FollowNpc, Sim, WorldReplaced};
use crate::train::{TrainLayout, place_at};
use crate::ui::PointerCheck;

/// Minuti di gioco al secondo reale mentre il giocatore dorme: una notte
/// di otto ore passa in poco più di un secondo e mezzo.
pub const SLEEP_SPEED: f32 = 300.0;

/// Finestra del baule (aperta con E sul baule della cabina).
#[derive(Resource, Default)]
pub(crate) struct ChestWindow {
    pub(crate) open: bool,
    message: Option<String>,
}

/// Il sonno in corso: da quando (per la barra di avanzamento).
#[derive(Resource, Default)]
struct SleepWatch {
    since: Option<GameTime>,
}

pub struct CabinPlugin;

impl Plugin for CabinPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChestWindow>()
            .init_resource::<SleepWatch>()
            .add_systems(
                PreUpdate,
                reset_cabin
                    .in_set(WorldRebuildSet)
                    .run_if(on_message::<WorldReplaced>),
            )
            .add_systems(Update, sync_player_place.before(SimTickSet))
            .add_systems(Update, (watch_sleep, close_chest_away).after(SimTickSet))
            .add_systems(
                EguiPrimaryContextPass,
                (chest_window, sleep_overlay).before(PointerCheck),
            );
    }
}

fn reset_cabin(mut chest: ResMut<ChestWindow>, mut watch: ResMut<SleepWatch>) {
    *chest = ChestWindow::default();
    watch.since = None;
}

/// La sim sa in che carrozza e a che piano è il giocatore (mentre la camera
/// segue un NPC il giocatore resta dov'è, e la sim lo sa lo stesso).
fn sync_player_place(
    mut sim: ResMut<Sim>,
    layout: Res<TrainLayout>,
    player: Single<&Transform, With<Player>>,
) {
    let place = place_at(&layout, player.translation.truncate());
    if sim.world.player.place != place {
        sim.world.set_player_place(place);
    }
}

/// Segue il sonno: all'addormentarsi ricorda l'ora (per la barra), al
/// risveglio salva la partita.
fn watch_sleep(
    sim: Res<Sim>,
    mut watch: ResMut<SleepWatch>,
    mut queue: ResMut<SaveQueue>,
    mut follow: ResMut<FollowNpc>,
) {
    let asleep = sim.world.player.is_asleep();
    match (watch.since, asleep) {
        (None, true) => {
            watch.since = Some(sim.world.clock);
            follow.0 = false;
        }
        (Some(_), false) => {
            watch.since = None;
            // Svegliato all'ora giusta (non interrotto): si salva.
            if sim.world.clock.hour() == sim.world.params.wake_hour && sim.world.clock.minute() == 0
            {
                queue.0.push(SaveCommand::Autosave);
            }
        }
        _ => {}
    }
}

/// Lontano dalla cabina il baule si chiude.
fn close_chest_away(sim: Res<Sim>, mut chest: ResMut<ChestWindow>) {
    if chest.open && !sim.world.player.at_home() {
        chest.open = false;
        chest.message = None;
    }
}

fn chest_window(
    mut contexts: EguiContexts,
    mut sim: ResMut<Sim>,
    mut chest: ResMut<ChestWindow>,
    mut icons: ResMut<crate::item_icons::ItemIcons>,
) {
    if !chest.open {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let mut open = true;
    let (mut store, mut retrieve) = (None, None);
    let center = ctx.content_rect().center();
    egui::Window::new("Baule")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_pos(center - egui::vec2(220.0, 200.0))
        .show(ctx, |ui| {
            let player = &sim.world.player;
            ui.label(format!("La cabina di {}", player.name));
            ui.weak("Clic su uno scomparto per spostare la pila.");
            ui.separator();
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.strong(format!(
                        "Inventario ({} liberi)",
                        player.inventory.free_slots()
                    ));
                    store = slot_grid(
                        ui,
                        &sim.world,
                        &mut icons,
                        &player.inventory,
                        "chest_inventory",
                        Some("Clic: metti nel baule"),
                    );
                });
                ui.separator();
                ui.vertical(|ui| {
                    ui.strong(format!("Baule ({} liberi)", player.chest.free_slots()));
                    retrieve = slot_grid(
                        ui,
                        &sim.world,
                        &mut icons,
                        &player.chest,
                        "chest_slots",
                        Some("Clic: prendi dal baule"),
                    );
                });
            });
            if let Some(message) = &chest.message {
                ui.separator();
                ui.colored_label(Color32::from_rgb(240, 220, 140), message);
            }
        });
    let world = &mut sim.world;
    let result = match (store, retrieve) {
        (Some(slot), _) => Some(world.player_store(slot).map(|n| ("Messi nel baule", n))),
        (_, Some(slot)) => Some(world.player_retrieve(slot).map(|n| ("Presi dal baule", n))),
        _ => None,
    };
    match result {
        Some(Ok((what, n))) => chest.message = Some(format!("{what}: {n}")),
        Some(Err(e)) => chest.message = Some(capitalized(&e.to_string())),
        None => {}
    }
    if !open {
        chest.open = false;
        chest.message = None;
    }
}

/// Mentre il giocatore dorme: schermo scuro, l'ora e quanto manca al
/// mattino, e "Svegliati".
fn sleep_overlay(mut contexts: EguiContexts, mut sim: ResMut<Sim>, watch: Res<SleepWatch>) {
    let Some(until) = sim.world.player.asleep_until else {
        return;
    };
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let now = sim.world.clock;
    let fainted = sim.world.player.is_down();
    let since = watch.since.unwrap_or(now);
    let total = until.since(since).max(1);
    let done = now.since(since).min(total);
    let mut wake = false;
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new("sleep_shade"))
        .fixed_pos(screen.min)
        .order(egui::Order::Background)
        .interactable(false)
        .show(ctx, |ui| {
            ui.painter()
                .rect_filled(screen, 0.0, Color32::from_black_alpha(190));
        });
    egui::Area::new(egui::Id::new("sleep_panel"))
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(260.0);
                ui.vertical_centered(|ui| {
                    // Svenuto dopo una rissa (vedi `combat.rs`): non ci si
                    // sveglia prima del mattino.
                    if fainted {
                        ui.label(RichText::new("Svenuto…").size(22.0).strong());
                        ui.label(format!(
                            "{} è stato riportato nella sua cabina · {:02}:{:02}",
                            sim.world.player.name,
                            now.hour(),
                            now.minute()
                        ));
                    } else {
                        ui.label(RichText::new("Zzz…").size(22.0).strong());
                        ui.label(format!(
                            "{} dorme nella sua cabina · {:02}:{:02}",
                            sim.world.player.name,
                            now.hour(),
                            now.minute()
                        ));
                    }
                    ui.add(
                        egui::ProgressBar::new(done as f32 / total as f32)
                            .text(format!("sveglia alle {:02}:00", until.hour())),
                    );
                    if !fainted && ui.button("Svegliati").clicked() {
                        wake = true;
                    }
                });
            });
        });
    if wake {
        sim.world.player_wake_up();
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::time::TimeUpdateStrategy;
    use sim::GameTime;

    use super::*;
    use crate::save_file::tests::TempDir;
    use crate::saves::SavesPlugin;

    /// Nel letto alle 21: il tempo corre (anche in pausa) fino alle 06:00, il
    /// giocatore si sveglia e la partita si salva da sola.
    #[test]
    fn sleeping_runs_to_the_morning_and_autosaves() {
        let tmp = TempDir::new("sleep");
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::input::InputPlugin,
            crate::state::StatePlugin,
            crate::sim_bridge::SimBridgePlugin,
            SavesPlugin {
                data_dir: Some(tmp.0.clone()),
            },
            crate::train::TrainPlugin,
            CabinPlugin,
        ))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            100,
        )));
        app.finish();
        app.cleanup();
        app.update();
        {
            let mut sim = app.world_mut().resource_mut::<Sim>();
            let Sim { world, brain } = &mut *sim;
            let evening = GameTime::from_dhm(1, 21, 0);
            world.run(brain, evening.since(world.clock));
            world.player_go_to_bed().expect("in cabina, di sera");
        }
        app.world_mut()
            .resource_mut::<crate::state::SimClock>()
            .paused = true;
        let mut woke = false;
        for _ in 0..400 {
            app.update();
            let world = &app.world().resource::<Sim>().world;
            if !world.player.is_asleep() {
                assert_eq!(world.clock, GameTime::from_dhm(2, 6, 0));
                woke = true;
                break;
            }
        }
        assert!(woke, "il giocatore non si è svegliato");
        // Il salvataggio automatico parte nel frame successivo.
        app.update();
        let run = app.world().resource::<crate::state::RunInfo>().dir.clone();
        for _ in 0..500 {
            if crate::save_file::latest_save(&run).is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
            app.update();
        }
        panic!("nessun salvataggio automatico al risveglio");
    }
}
