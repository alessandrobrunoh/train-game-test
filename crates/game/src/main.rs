//! TrainGame: vista laterale di un lungo treno pieno di NPC.
//! M0: grafica placeholder, giocatore controllabile, camera che lo segue.

mod camera;
mod fonts;
mod hud;
mod interaction;
mod inventory;
mod life_fx;
mod npc_render;
mod player;
mod population;
mod sim_bridge;
mod state;
mod stations;
mod storage;
mod train;
mod ui;

use bevy::prelude::*;

/// Colore del cielo dietro il treno.
const SKY_COLOR: Color = Color::srgb(0.55, 0.75, 0.92);

fn main() {
    App::new()
        .insert_resource(ClearColor(SKY_COLOR))
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Train".into(),
                        resolution: (1280, 720).into(),
                        ..default()
                    }),
                    ..default()
                })
                // Filtro "nearest" per le future texture in pixel art.
                .set(ImagePlugin::default_nearest()),
        )
        .add_plugins((
            fonts::FontsPlugin,
            state::StatePlugin,
            sim_bridge::SimBridgePlugin,
            train::TrainPlugin,
            stations::StationsPlugin,
            storage::StoragePlugin,
            npc_render::NpcRenderPlugin,
            player::PlayerPlugin,
            camera::CameraPlugin,
            hud::HudPlugin,
            ui::UiPlugin,
            interaction::InteractionPlugin,
            inventory::InventoryPlugin,
            life_fx::LifeFxPlugin,
            population::PopulationPlugin,
        ))
        .run();
}
