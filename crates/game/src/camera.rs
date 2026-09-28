//! Camera 2D che segue il giocatore con uno smorzamento morbido.

use bevy::{camera::ScalingMode, prelude::*};

use crate::player::Player;

/// Altezza visibile in unità mondo: con una finestra alta 720 px ogni unità
/// diventa esattamente 3 pixel, adatto alla pixel art.
const VIEWPORT_HEIGHT: f32 = 240.0;
/// Quanto sopra il giocatore guarda la camera, per inquadrare la carrozza.
const LOOK_UP: f32 = 30.0;
/// Velocità di inseguimento (più alto = più reattivo). In verticale è più lenta
/// così i salti non fanno sobbalzare l'inquadratura.
const DECAY_X: f32 = 6.0;
const DECAY_Y: f32 = 2.5;

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        // `Update` gira dopo `RunFixedMainLoop`, quindi il Transform del
        // giocatore è già interpolato per questo frame.
        app.add_systems(Startup, spawn_camera)
            .add_systems(Update, follow_player);
    }
}

fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        Camera2d,
        Projection::from(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical {
                viewport_height: VIEWPORT_HEIGHT,
            },
            ..OrthographicProjection::default_2d()
        }),
    ));
}

fn follow_player(
    time: Res<Time>,
    player: Single<&Transform, (With<Player>, Without<Camera2d>)>,
    mut camera: Single<&mut Transform, With<Camera2d>>,
    mut snapped: Local<bool>,
) {
    let target = Vec2::new(player.translation.x, player.translation.y + LOOK_UP);

    // Al primo frame la camera salta direttamente sul giocatore.
    if !*snapped {
        camera.translation.x = target.x;
        camera.translation.y = target.y;
        *snapped = true;
        return;
    }

    let dt = time.delta_secs();
    camera.translation.x.smooth_nudge(&target.x, DECAY_X, dt);
    camera.translation.y.smooth_nudge(&target.y, DECAY_Y, dt);
}
