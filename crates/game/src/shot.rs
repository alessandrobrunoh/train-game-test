//! Screenshot automatici, per controllare la grafica senza giocare:
//! `TRAINGAME_SHOTS=<cartella> cargo run -p game`.
//!
//! Porta la simulazione alla sera (chi dorme, dorme anche al piano di sopra),
//! porta il giocatore davanti alla prima carrozza a più piani, salva uno
//! screenshot per scena (piano terra, piano di sopra, sulla scala, vista
//! allargata) nella cartella e chiude il gioco. Senza la variabile non fa nulla.
//!
//! La camera disegna su un'immagine fuori schermo, non sulla finestra: macOS
//! non disegna le finestre coperte o con lo schermo bloccato (screenshot
//! neri). Le finestre egui restano fuori dall'immagine.

use std::path::PathBuf;

use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

use sim::GameTime;

use crate::camera::WideView;
use crate::player::{Body, Player, start_position};
use crate::state::Sim;
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

/// Nome del file e posizione del giocatore: piano, x locale (`None` = sulla scala), vista allargata.
const SCENES: [(&str, usize, Option<f32>, bool); 4] = [
    ("1-piano-terra", 0, Some(160.0), false),
    ("2-piano-sopra", 1, Some(160.0), false),
    ("3-sulla-scala", 0, None, false),
    ("4-vista-allargata", 1, Some(120.0), true),
];

#[allow(clippy::too_many_arguments)]
fn run_script(
    mut commands: Commands,
    time: Res<Time<Real>>,
    layout: Res<TrainLayout>,
    mut script: ResMut<ShotScript>,
    mut sim: ResMut<Sim>,
    mut wide: ResMut<WideView>,
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
    let Some(&(_, floor, x, wide_view)) = SCENES.get(script.scene) else {
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
    let position = match x {
        Some(x) => Vec2::new(left + x, floor_y(floor) + half_height),
        None => Vec2::new(stairs_x0 + STAIRS_WIDTH / 2.0, floor_y(floor) + 50.0),
    };
    body.teleport(position);
    body.climbing = x.is_none();
    wide.0 = wide_view;
    script.scene += 1;
    script.wait = SCENE_WAIT;
}
