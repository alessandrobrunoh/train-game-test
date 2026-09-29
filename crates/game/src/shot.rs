//! Screenshot automatici, per controllare la grafica senza giocare:
//! `TRAINGAME_SHOTS=<cartella> cargo run -p game`.
//!
//! Porta il giocatore davanti alla prima carrozza a più piani, salva uno
//! screenshot per scena (piano terra, piano di sopra, sulla scala, vista
//! allargata) nella cartella e chiude il gioco. Senza la variabile non fa nulla.

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::window::PrimaryWindow;

use crate::camera::WideView;
use crate::player::{Body, Player, start_position};
use crate::train::{FLOOR_Y, STAIRS_WIDTH, TrainLayout, floor_y};

const SHOTS_ENV: &str = "TRAINGAME_SHOTS";
/// Attesa prima della prima scena (arte generata, camera ferma)...
const FIRST_WAIT: f32 = 4.0;
/// ...e tra una scena e l'altra (la camera raggiunge il giocatore).
const SCENE_WAIT: f32 = 2.5;

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
        })
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
    mut wide: ResMut<WideView>,
    mut body: Single<&mut Body, With<Player>>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
    mut exit: MessageWriter<AppExit>,
) {
    // Una finestra dietro alle altre non viene disegnata (screenshot neri).
    if script.scene == 0 && !window.focused {
        window.focused = true;
    }
    script.wait -= time.delta_secs();
    if script.wait > 0.0 {
        return;
    }
    // Scatta la scena preparata al giro precedente.
    if (1..=SCENES.len()).contains(&script.scene) {
        let (name, ..) = SCENES[script.scene - 1];
        let path = script.dir.join(format!("{name}.png"));
        commands
            .spawn(Screenshot::primary_window())
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
