//! TrainGame: vista laterale di un lungo treno pieno di NPC.
//! M0: grafica placeholder, giocatore controllabile, camera che lo segue.

mod ai_ui;
mod art;
mod background;
mod brain_ui;
mod bubbles;
mod cabin;
mod camera;
mod characters;
mod chat;
mod chronicle_ui;
mod combat;
mod crafting;
mod env_art;
mod fonts;
mod history_sync;
mod history_ui;
mod hud;
mod interaction;
mod inventory;
mod item_icons;
mod life_fx;
mod market_ui;
mod narrator_bridge;
mod npc_render;
mod player;
mod population;
mod prop_art;
mod save_file;
mod saves;
mod shot;
mod sim_bridge;
mod speech;
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
        // Al massimo 15 plugin per tupla: la prima parte crea stato, mondo e run.
        .add_plugins((
            fonts::FontsPlugin,
            state::StatePlugin,
            sim_bridge::SimBridgePlugin,
            saves::SavesPlugin::default(),
        ))
        .add_plugins((
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
            history_sync::HistorySyncPlugin,
            history_ui::HistoryUiPlugin,
            brain_ui::BrainUiPlugin,
        ))
        // Il mondo fuori dal treno (cielo, paesaggio, neve, giorno/notte), i
        // fumetti delle deliberazioni e dei dialoghi e il crafting (tasto C).
        .add_plugins((
            background::BackgroundPlugin,
            bubbles::BubblesPlugin,
            speech::SpeechPlugin,
            // Finestra "Mercato" e listino dei prezzi (tasto M).
            market_ui::MarketUiPlugin,
            crafting::CraftingPlugin,
            // Il giocatore nella sim: posto, cabina, baule e sonno.
            cabin::CabinPlugin,
            // Chat con gli NPC (tasto T).
            chat::ChatPlugin,
            // Salute e risse (tasto X), barre, danni, svenimento.
            combat::CombatPlugin,
        ))
        // Il Narratore (da `.env`, spento senza): una novità al giorno, la
        // cronaca (tasto N), i pannelli e le statistiche inventati (tasto K).
        .add_plugins((
            narrator_bridge::NarratorBridgePlugin,
            ai_ui::AiUiPlugin,
            chronicle_ui::ChronicleUiPlugin,
        ))
        // Screenshot automatici (solo con `TRAINGAME_SHOTS`).
        .add_plugins(shot::ShotPlugin)
        .run();
}
