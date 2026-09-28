//! TrainGame: vista laterale di un lungo treno pieno di NPC.
//! M0: grafica placeholder, giocatore controllabile, camera che lo segue.

mod camera;
mod hud;
mod npc_render;
mod player;
mod sim_bridge;
mod state;
mod stations;
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
            state::StatePlugin,
            sim_bridge::SimBridgePlugin,
            train::TrainPlugin,
            stations::StationsPlugin,
            npc_render::NpcRenderPlugin,
            player::PlayerPlugin,
            camera::CameraPlugin,
            hud::HudPlugin,
            ui::UiPlugin,
        ))
        .run();
}
