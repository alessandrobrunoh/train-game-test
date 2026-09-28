//! Controller platform cinematico scritto a mano (niente crate di fisica).
//!
//! L'input viene raccolto ogni frame, la fisica gira in `FixedUpdate` e la
//! posizione disegnata viene interpolata tra gli ultimi due passi fissi, così il
//! movimento resta fluido a qualunque frame rate.

use bevy::prelude::*;

use crate::train::{FLOOR_Y, TrainLayout};

/// Dimensioni del rettangolo del giocatore.
const PLAYER_SIZE: Vec2 = Vec2::new(12.0, 24.0);
const PLAYER_COLOR: Color = Color::srgb(0.95, 0.85, 0.30);
const PLAYER_Z: f32 = 10.0;

/// Velocità orizzontale massima (unità/s).
const RUN_SPEED: f32 = 110.0;
/// Accelerazione orizzontale a terra e in aria (unità/s²).
const GROUND_ACCEL: f32 = 1200.0;
const AIR_ACCEL: f32 = 700.0;
const GRAVITY: f32 = 900.0;
/// Velocità iniziale del salto: altezza massima ≈ v² / 2g ≈ 50 unità.
const JUMP_SPEED: f32 = 300.0;
/// Rilasciando il tasto durante la salita la velocità viene ridotta (salto variabile).
const JUMP_CUT: f32 = 0.5;
const MAX_FALL_SPEED: f32 = 400.0;

#[derive(Component, Debug)]
pub struct Player;

/// Input raccolto a frame rate variabile e consumato dal passo fisso.
#[derive(Component, Debug, Default)]
struct PlayerInput {
    /// Direzione orizzontale: -1, 0 o 1.
    axis: f32,
    /// Salto premuto dall'ultimo passo fisso (bufferizzato).
    jump_pressed: bool,
    /// Tasto di salto tenuto premuto.
    jump_held: bool,
}

/// Stato fisico del giocatore; `position` è il centro del rettangolo.
#[derive(Component, Debug, Default)]
pub struct Body {
    pub position: Vec2,
    previous: Vec2,
    pub velocity: Vec2,
    pub grounded: bool,
}

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_player)
            .add_systems(FixedUpdate, step_physics)
            .add_systems(
                RunFixedMainLoop,
                (
                    gather_input.in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop),
                    interpolate_transform.in_set(RunFixedMainLoopSystems::AfterFixedMainLoop),
                ),
            );
    }
}

fn spawn_player(mut commands: Commands) {
    // Parte al centro della prima carrozza, appoggiato al pavimento.
    let start = Vec2::new(
        TrainLayout::carriage_center_x(0),
        FLOOR_Y + PLAYER_SIZE.y / 2.0,
    );
    commands.spawn((
        Name::new("Giocatore"),
        Player,
        PlayerInput::default(),
        Body {
            position: start,
            previous: start,
            ..default()
        },
        Sprite::from_color(PLAYER_COLOR, PLAYER_SIZE),
        Transform::from_translation(start.extend(PLAYER_Z)),
    ));
}

fn gather_input(keys: Res<ButtonInput<KeyCode>>, mut input: Single<&mut PlayerInput>) {
    let left = keys.any_pressed([KeyCode::KeyA, KeyCode::ArrowLeft]);
    let right = keys.any_pressed([KeyCode::KeyD, KeyCode::ArrowRight]);
    let jump_keys = [KeyCode::Space, KeyCode::KeyW, KeyCode::ArrowUp];

    input.axis = f32::from(right as u8) - f32::from(left as u8);
    input.jump_held = keys.any_pressed(jump_keys);
    // Resta vero finché un passo fisso non lo consuma.
    input.jump_pressed |= keys.any_just_pressed(jump_keys);
}

fn step_physics(
    time: Res<Time>,
    layout: Res<TrainLayout>,
    mut player: Single<(&mut Body, &mut PlayerInput)>,
) {
    let dt = time.delta_secs();
    let (body, input) = &mut *player;
    body.previous = body.position;

    // Orizzontale: accelera verso la velocità desiderata.
    let accel = if body.grounded {
        GROUND_ACCEL
    } else {
        AIR_ACCEL
    };
    let target = input.axis * RUN_SPEED;
    body.velocity.x = move_towards(body.velocity.x, target, accel * dt);

    // Salto
    if std::mem::take(&mut input.jump_pressed) && body.grounded {
        body.velocity.y = JUMP_SPEED;
        body.grounded = false;
    }
    if !input.jump_held && body.velocity.y > 0.0 {
        body.velocity.y *= JUMP_CUT;
    }

    // Gravità
    body.velocity.y = (body.velocity.y - GRAVITY * dt).max(-MAX_FALL_SPEED);

    move_and_collide(body, &layout, dt);
}

/// Sposta il corpo un asse alla volta e lo spinge fuori dai solidi.
fn move_and_collide(body: &mut Body, layout: &TrainLayout, dt: f32) {
    let half = PLAYER_SIZE / 2.0;

    // Asse X
    body.position.x += body.velocity.x * dt;
    for solid in &layout.solids {
        if overlaps(body.position, half, solid) {
            if body.velocity.x > 0.0 {
                body.position.x = solid.min.x - half.x;
            } else if body.velocity.x < 0.0 {
                body.position.x = solid.max.x + half.x;
            }
            body.velocity.x = 0.0;
        }
    }

    // Asse Y: il contatto dall'alto rende il corpo "a terra".
    body.position.y += body.velocity.y * dt;
    body.grounded = false;
    for solid in &layout.solids {
        if overlaps(body.position, half, solid) {
            if body.velocity.y <= 0.0 {
                body.position.y = solid.max.y + half.y;
                body.grounded = true;
            } else {
                body.position.y = solid.min.y - half.y;
            }
            body.velocity.y = 0.0;
        }
    }

    // Rete di sicurezza: mai oltre le testate del treno.
    let (min_x, max_x) = layout.inner_bounds();
    body.position.x = body.position.x.clamp(min_x + half.x, max_x - half.x);
}

/// Sovrapposizione stretta: due rettangoli che si toccano soltanto non collidono.
fn overlaps(center: Vec2, half: Vec2, rect: &Rect) -> bool {
    center.x - half.x < rect.max.x
        && center.x + half.x > rect.min.x
        && center.y - half.y < rect.max.y
        && center.y + half.y > rect.min.y
}

fn move_towards(current: f32, target: f32, max_delta: f32) -> f32 {
    current + (target - current).clamp(-max_delta, max_delta)
}

/// Posizione disegnata = interpolazione tra gli ultimi due passi fissi.
fn interpolate_transform(fixed_time: Res<Time<Fixed>>, mut query: Query<(&mut Transform, &Body)>) {
    let alpha = fixed_time.overstep_fraction();
    for (mut transform, body) in &mut query {
        let pos = body.previous.lerp(body.position, alpha);
        transform.translation.x = pos.x;
        transform.translation.y = pos.y;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::train::TrainLocation;
    use sim::CarriageKind;

    const DT: f32 = 1.0 / 64.0;

    fn test_layout() -> TrainLayout {
        TrainLayout::new(CarriageKind::ALL.to_vec())
    }

    fn body_at(x: f32) -> Body {
        let position = Vec2::new(x, FLOOR_Y + PLAYER_SIZE.y / 2.0);
        Body {
            position,
            previous: position,
            ..default()
        }
    }

    #[test]
    fn walks_through_every_door_to_the_end_of_the_train() {
        let layout = test_layout();
        let mut body = body_at(TrainLayout::carriage_center_x(0));
        for _ in 0..64 * 30 {
            body.velocity.x = RUN_SPEED;
            body.velocity.y -= GRAVITY * DT;
            move_and_collide(&mut body, &layout, DT);
        }
        let (_, max_x) = layout.inner_bounds();
        assert_eq!(body.position.x, max_x - PLAYER_SIZE.x / 2.0);
        assert!(body.grounded);
        assert_eq!(
            layout.location_at(body.position.x),
            TrainLocation::Carriage(layout.len() - 1)
        );
    }

    #[test]
    fn jump_lands_back_on_the_floor() {
        let layout = test_layout();
        let mut body = body_at(TrainLayout::carriage_center_x(0));
        body.velocity.y = JUMP_SPEED;
        let mut peak = body.position.y;
        for _ in 0..64 * 2 {
            body.velocity.y = (body.velocity.y - GRAVITY * DT).max(-MAX_FALL_SPEED);
            move_and_collide(&mut body, &layout, DT);
            peak = peak.max(body.position.y);
        }
        assert!(peak > FLOOR_Y + PLAYER_SIZE.y / 2.0 + 30.0);
        assert!(body.grounded);
        assert_eq!(body.position.y, FLOOR_Y + PLAYER_SIZE.y / 2.0);
    }
}
