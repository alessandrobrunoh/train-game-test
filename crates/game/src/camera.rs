//! Camera 2D che segue il giocatore con uno smorzamento morbido.
//!
//! In orizzontale l'inquadratura resta dentro il treno: non mostra mai oltre le
//! pareti di testata estreme (più un piccolo margine). Quando la camera è ferma
//! la sua posizione viene allineata alla griglia dei pixel dello schermo, così
//! testo e sprite restano nitidi senza introdurre scatti durante il movimento.

use bevy::{
    camera::{CameraProjection, ScalingMode},
    prelude::*,
    window::PrimaryWindow,
};

use crate::player::Player;
use crate::train::{TrainLayout, WALL};

/// Altezza visibile in unità mondo: con una finestra alta 720 px ogni unità
/// diventa esattamente 3 pixel, adatto alla pixel art.
const VIEWPORT_HEIGHT: f32 = 240.0;
/// Quanto sopra il giocatore guarda la camera, per inquadrare la carrozza.
const LOOK_UP: f32 = 30.0;
/// Velocità di inseguimento (più alto = più reattivo). In verticale è più lenta
/// così i salti non fanno sobbalzare l'inquadratura.
const DECAY_X: f32 = 6.0;
const DECAY_Y: f32 = 2.5;
/// Quanto cielo si vede oltre le pareti di testata alle estremità del treno.
const EDGE_MARGIN: f32 = 16.0;

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

/// Area visibile (in unità mondo, relativa alla camera) e dimensione di un
/// pixel fisico dello schermo in unità mondo.
///
/// La proiezione viene ricalcolata sulla finestra attuale invece di leggere
/// `area`, che Bevy aggiorna solo in `PostUpdate`: così è corretta anche al
/// primo frame e subito dopo un ridimensionamento.
fn view_metrics(ortho: &OrthographicProjection, window: &Window) -> Option<(Rect, f32)> {
    let size = window.physical_size().as_vec2();
    if size.x <= 0.0 || size.y <= 0.0 {
        // Finestra minimizzata.
        return None;
    }
    let mut ortho = ortho.clone();
    ortho.update(size.x, size.y);
    Some((ortho.area, ortho.area.height() / size.y))
}

/// Limita la x della camera perché la vista (larga `2 * half_width`) non esca
/// da `[left - EDGE_MARGIN, right + EDGE_MARGIN]`. Se il treno è più stretto
/// della vista, lo centra.
fn clamp_to_train(x: f32, half_width: f32, left: f32, right: f32) -> f32 {
    let lo = left - EDGE_MARGIN + half_width;
    let hi = right + EDGE_MARGIN - half_width;
    if lo > hi {
        (left + right) / 2.0
    } else {
        x.clamp(lo, hi)
    }
}

pub(crate) fn follow_player(
    time: Res<Time>,
    layout: Res<TrainLayout>,
    window: Single<&Window, With<PrimaryWindow>>,
    player: Single<&Transform, (With<Player>, Without<Camera2d>)>,
    camera: Single<(&mut Transform, &Projection), With<Camera2d>>,
    mut snapped: Local<bool>,
) {
    let (mut camera, projection) = camera.into_inner();
    let Projection::Orthographic(ortho) = projection else {
        return;
    };
    let Some((view, pixel)) = view_metrics(ortho, &window) else {
        return;
    };

    // Estremi esterni del treno (facce esterne delle testate).
    let (inner_left, inner_right) = layout.inner_bounds();
    let target = Vec2::new(
        clamp_to_train(
            player.translation.x,
            view.half_size().x,
            inner_left - WALL,
            inner_right + WALL,
        ),
        player.translation.y + LOOK_UP,
    );
    let current = camera.translation.truncate();

    let next = if !*snapped {
        // Al primo frame la camera salta direttamente sul giocatore.
        *snapped = true;
        target
    } else if (target - current).abs().max_element() < pixel {
        // Ferma (a meno di un pixel dall'obiettivo): allineata alla griglia
        // dei pixel. Il valore è stabile finché l'obiettivo non si sposta.
        (target / pixel).round() * pixel
    } else {
        let dt = time.delta_secs();
        let mut next = current;
        next.x.smooth_nudge(&target.x, DECAY_X, dt);
        next.y.smooth_nudge(&target.y, DECAY_Y, dt);
        next
    };

    if next != current {
        camera.translation.x = next.x;
        camera.translation.y = next.y;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_the_view_inside_the_train() {
        let half = 200.0;
        // Lontano dalle estremità: segue liberamente.
        assert_eq!(clamp_to_train(1000.0, half, 0.0, 5000.0), 1000.0);
        // All'estremità sinistra il bordo della vista si ferma al margine.
        let x = clamp_to_train(0.0, half, 0.0, 5000.0);
        assert_eq!(x - half, -EDGE_MARGIN);
        // All'estremità destra idem.
        let x = clamp_to_train(9000.0, half, 0.0, 5000.0);
        assert_eq!(x + half, 5000.0 + EDGE_MARGIN);
    }

    #[test]
    fn centers_a_train_narrower_than_the_view() {
        assert_eq!(clamp_to_train(-50.0, 400.0, 0.0, 320.0), 160.0);
        assert_eq!(clamp_to_train(900.0, 400.0, 0.0, 320.0), 160.0);
    }

    #[test]
    fn view_metrics_follow_the_window_aspect() {
        let ortho = OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical {
                viewport_height: VIEWPORT_HEIGHT,
            },
            ..OrthographicProjection::default_2d()
        };
        let window = Window {
            resolution: (1280, 720).into(),
            ..default()
        };
        let (view, pixel) = view_metrics(&ortho, &window).unwrap();
        assert!((view.height() - VIEWPORT_HEIGHT).abs() < 1e-3);
        assert!((view.width() - VIEWPORT_HEIGHT * 1280.0 / 720.0).abs() < 1e-3);
        assert!((pixel - VIEWPORT_HEIGHT / 720.0).abs() < 1e-6);
    }
}
