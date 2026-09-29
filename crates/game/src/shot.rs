//! Screenshot automatici, per controllare la grafica senza giocare:
//! `TRAINGAME_SHOTS=<cartella> cargo run -p game`.
//!
//! Porta la simulazione alla sera (chi dorme, dorme anche al piano di sopra),
//! porta il giocatore davanti alla prima carrozza a più piani, salva uno
//! screenshot per scena (piano terra, piano di sopra, sulla scala, vista
//! allargata, la cabina del giocatore con un amico che lo saluta, la chat
//! con l'amico) nella cartella e chiude il gioco. Senza la variabile non fa
//! nulla.
//!
//! La camera disegna su un'immagine fuori schermo, non sulla finestra: macOS
//! non disegna le finestre coperte o con lo schermo bloccato (screenshot
//! neri). Anche le finestre egui finiscono nell'immagine: l'ultima scena ha
//! inventario e baule aperti.

use std::path::PathBuf;

use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

use sim::GameTime;

use crate::cabin::ChestWindow;
use crate::camera::WideView;
use crate::chat::{ChatCommand, ChatQueue};
use crate::inventory::InventoryWindow;
use crate::player::{Body, Player, start_position};
use crate::state::Sim;
use crate::stations::StationLayout;
use crate::train::{FLOOR_Y, STAIRS_WIDTH, TrainLayout, floor_y};

const SHOTS_ENV: &str = "TRAINGAME_SHOTS";
/// Attesa prima della prima scena (arte generata, camera ferma)...
const FIRST_WAIT: f32 = 4.0;
/// ...e tra una scena e l'altra (la camera raggiunge il giocatore).
const SCENE_WAIT: f32 = 2.5;
/// Ora del giorno 1 a cui si porta la simulazione prima delle scene.
const EVENING: (u64, u64) = (22, 30);

pub struct ShotPlugin;

impl Plugin for ShotPlugin {
    fn build(&self, app: &mut App) {
        let Some(dir) = std::env::var_os(SHOTS_ENV).filter(|d| !d.is_empty()) else {
            return;
        };
        app.insert_resource(ShotScript {
            dir: PathBuf::from(dir),
            scene: 0,
            wait: FIRST_WAIT,
            target: Handle::default(),
        })
        .add_systems(PostStartup, render_offscreen)
        .add_systems(Update, run_script);
    }
}

#[derive(Resource)]
struct ShotScript {
    dir: PathBuf,
    /// Prossima scena da preparare (`SCENES.len()` = fine).
    scene: usize,
    /// Secondi reali prima di scattare la scena preparata.
    wait: f32,
    /// Immagine su cui disegna la camera.
    target: Handle<Image>,
}

/// Dimensioni dell'immagine fuori schermo (16:9, come la finestra).
const SHOT_SIZE: (u32, u32) = (1920, 1080);

/// Fa disegnare la camera su un'immagine invece che sulla finestra.
fn render_offscreen(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut script: ResMut<ShotScript>,
    camera: Single<Entity, With<Camera2d>>,
) {
    let image = Image::new_target_texture(
        SHOT_SIZE.0,
        SHOT_SIZE.1,
        TextureFormat::Rgba8UnormSrgb,
        None,
    );
    script.target = images.add(image);
    commands
        .entity(*camera)
        .insert(RenderTarget::Image(script.target.clone().into()));
}

/// Dove mettere il giocatore in una scena.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Spot {
    /// Nella prima carrozza a più piani: piano e x locale (`None` = sulla scala).
    Floor(usize, Option<f32>),
    /// Nella sua cabina, accanto al letto; con inventario e baule aperti.
    Cabin { windows: bool },
    /// Nella sua cabina, in chat con l'amico.
    Chat,
}

/// Nome del file, posto del giocatore, vista allargata.
const SCENES: [(&str, Spot, bool); 7] = [
    ("1-piano-terra", Spot::Floor(0, Some(160.0)), false),
    ("2-piano-sopra", Spot::Floor(1, Some(160.0)), false),
    ("3-sulla-scala", Spot::Floor(0, None), false),
    ("4-vista-allargata", Spot::Floor(1, Some(120.0)), true),
    ("5-cabina", Spot::Cabin { windows: false }, false),
    ("6-inventario-baule", Spot::Cabin { windows: true }, false),
    ("7-chat", Spot::Chat, false),
];

/// Un amico del giocatore sveglio nella cabina, e qualcosa nell'inventario
/// e nel baule: la scena mostra il saluto e le finestre piene. Restituisce l'amico.
fn stage_cabin(world: &mut sim::World) -> Option<sim::NpcId> {
    let home = world.player.home?;
    world.set_player_place(home.place());
    if world.player.inventory.is_empty() {
        for (item, n) in [
            (sim::ItemKind::Rottame, 14),
            (sim::ItemKind::Verdura, 3),
            (sim::ItemKind::Te, 2),
            (sim::ItemKind::Attrezzo, 1),
        ] {
            world.player.inventory.add(item, n);
        }
        world.player.chest.add(sim::ItemKind::Tessuto, 7);
        world.player.chest.add(sim::ItemKind::Coperta, 2);
    }
    let i = world
        .npcs
        .iter()
        .position(|n| n.carriage == home.carriage && n.age >= 18)?;
    let now = world.clock;
    let npc = &mut world.npcs[i];
    if let Some(s) = npc.action.station() {
        let station = &mut world.carriages[npc.carriage.index()].stations[s.index()];
        station.occupancy = station.occupancy.saturating_sub(1);
    }
    npc.floor = home.floor;
    npc.action = sim::Action::Idle;
    npc.action_since = now;
    npc.action_until = now + 60;
    if npc.player.is_none() {
        npc.player = Some(sim::PlayerTie {
            affinity: 0.8,
            ..sim::PlayerTie::default()
        });
    }
    Some(npc.id)
}

#[allow(clippy::too_many_arguments)]
fn run_script(
    mut commands: Commands,
    time: Res<Time<Real>>,
    layout: Res<TrainLayout>,
    stations: Res<StationLayout>,
    mut script: ResMut<ShotScript>,
    mut sim: ResMut<Sim>,
    mut wide: ResMut<WideView>,
    mut ui_windows: (
        ResMut<InventoryWindow>,
        ResMut<ChestWindow>,
        ResMut<ChatQueue>,
    ),
    mut body: Single<&mut Body, With<Player>>,
    mut exit: MessageWriter<AppExit>,
) {
    script.wait -= time.delta_secs();
    if script.wait > 0.0 {
        return;
    }
    if script.scene == 0 {
        let Sim { world, brain } = &mut *sim;
        let evening = GameTime::from_dhm(1, EVENING.0, EVENING.1);
        let minutes = evening.since(world.clock);
        world.run(brain, minutes);
    }
    // Scatta la scena preparata al giro precedente.
    if (1..=SCENES.len()).contains(&script.scene) {
        let (name, ..) = SCENES[script.scene - 1];
        let path = script.dir.join(format!("{name}.png"));
        commands
            .spawn(Screenshot::image(script.target.clone()))
            .observe(save_to_disk(path));
    }
    let Some(&(_, spot, wide_view)) = SCENES.get(script.scene) else {
        // Lascia un attimo al salvataggio dell'ultima immagine.
        if script.scene == SCENES.len() {
            script.scene += 1;
            script.wait = 1.0;
        } else {
            exit.write(AppExit::Success);
        }
        return;
    };
    let index = (0..layout.len())
        .find(|&i| layout.floors(i) > 1)
        .unwrap_or(0);
    let half_height = start_position().y - FLOOR_Y;
    let left = TrainLayout::carriage_left(index);
    let (stairs_x0, _) = TrainLayout::stairs_x(index);
    let position = match (spot, stations.cabin) {
        (Spot::Floor(floor, Some(x)), _) => Vec2::new(left + x, floor_y(floor) + half_height),
        (Spot::Floor(floor, None), _) => {
            Vec2::new(stairs_x0 + STAIRS_WIDTH / 2.0, floor_y(floor) + 50.0)
        }
        (Spot::Cabin { windows }, Some(cabin)) => {
            let Sim { world, brain } = &mut *sim;
            stage_cabin(world);
            // Qualche minuto: l'amico saluta (i saluti sono ogni 5 minuti).
            if !windows {
                world.run(brain, 5);
            }
            ui_windows.0.open = windows;
            ui_windows.1.open = windows;
            let x = TrainLayout::carriage_left(cabin.carriage.index()) + cabin.bed_x + 18.0;
            Vec2::new(x, cabin.base_y() + half_height)
        }
        (Spot::Chat, Some(cabin)) => {
            let friend = stage_cabin(&mut sim.world);
            ui_windows.0.open = false;
            ui_windows.1.open = false;
            if let Some(id) = friend {
                ui_windows.2.0.extend([
                    ChatCommand::Open(id),
                    ChatCommand::Say(sim::Intent::Greet),
                    ChatCommand::Say(sim::Intent::AskJob),
                    ChatCommand::Type("ciao, quanto costa un vestito?".to_string()),
                    ChatCommand::Say(sim::Intent::AskFavour),
                    ChatCommand::Say(sim::Intent::AskNews),
                ]);
            }
            let x = TrainLayout::carriage_left(cabin.carriage.index()) + cabin.bed_x + 18.0;
            Vec2::new(x, cabin.base_y() + half_height)
        }
        (Spot::Cabin { .. } | Spot::Chat, None) => start_position(),
    };
    body.teleport(position);
    body.climbing = spot == Spot::Floor(0, None);
    wide.0 = wide_view;
    script.scene += 1;
    script.wait = SCENE_WAIT;
}
