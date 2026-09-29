//! Controller platform cinematico scritto a mano (niente crate di fisica).
//!
//! L'input viene raccolto ogni frame, la fisica gira in `FixedUpdate` e la
//! posizione disegnata viene interpolata tra gli ultimi due passi fissi, così il
//! movimento resta fluido a qualunque frame rate.
//!
//! Aspetto: un ribelle della coda del treno, col cappuccio e una sciarpa
//! gialla (vedi `characters.rs`), animato da fermo, di corsa, in salto e in
//! caduta; guarda nella direzione in cui si muove.

use bevy::{prelude::*, sprite::Anchor};

use crate::characters::{
    PLAYER_CELL, PLAYER_FRAMES, PlayerFrame, player_anchor, player_frame, player_sheet,
};
use crate::state::FollowNpc;
use crate::train::{FLOOR_Y, TrainLayout};

/// Ingombro del giocatore (collisioni).
const PLAYER_SIZE: Vec2 = Vec2::new(12.0, 24.0);
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
/// Velocità sulle scale a pioli (unità/s).
const CLIMB_SPEED: f32 = 64.0;

#[derive(Component, Debug)]
pub struct Player;

/// Input raccolto a frame rate variabile e consumato dal passo fisso.
#[derive(Component, Debug, Default)]
struct PlayerInput {
    /// Direzione orizzontale: -1, 0 o 1.
    axis: f32,
    /// Su (1) o giù (-1) sulle scale: W/↑ e S/↓.
    vertical: f32,
    /// Salto premuto dall'ultimo passo fisso (bufferizzato).
    jump_pressed: bool,
    /// Tasto di salto tenuto premuto.
    jump_held: bool,
}

/// Stato dell'animazione del giocatore.
#[derive(Component, Debug, Default)]
struct PlayerAnim {
    /// Secondi reali (per il respiro da fermo).
    clock: f32,
    /// Strada fatta a terra (scandisce i passi).
    walked: f32,
    last_x: f32,
    facing_left: bool,
}

/// Stato fisico del giocatore; `position` è il centro del rettangolo.
#[derive(Component, Debug, Default)]
pub struct Body {
    pub position: Vec2,
    previous: Vec2,
    pub velocity: Vec2,
    pub grounded: bool,
    /// Aggrappato a una scala a pioli: niente gravità né piattaforme.
    pub climbing: bool,
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
                    (interpolate_transform, animate_player)
                        .chain()
                        .in_set(RunFixedMainLoopSystems::AfterFixedMainLoop),
                ),
            );
    }
}

impl Body {
    /// Sposta il corpo di colpo (caricamento di una partita), da fermo.
    pub fn teleport(&mut self, position: Vec2) {
        self.position = position;
        self.previous = position;
        self.velocity = Vec2::ZERO;
        self.grounded = false;
        self.climbing = false;
    }
}

/// Dove comincia il giocatore: al centro della prima carrozza, appoggiato al pavimento.
pub fn start_position() -> Vec2 {
    Vec2::new(
        TrainLayout::carriage_center_x(0),
        FLOOR_Y + PLAYER_SIZE.y / 2.0,
    )
}

fn spawn_player(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut layouts: ResMut<Assets<TextureAtlasLayout>>,
) {
    let start = start_position();
    let image = images.add(player_sheet().to_image());
    let layout = layouts.add(TextureAtlasLayout::from_grid(
        PLAYER_CELL,
        PLAYER_FRAMES as u32,
        1,
        None,
        None,
    ));
    commands.spawn((
        Name::new("Giocatore"),
        Player,
        PlayerInput::default(),
        PlayerAnim {
            last_x: start.x,
            ..default()
        },
        Body {
            position: start,
            previous: start,
            ..default()
        },
        Sprite::from_atlas_image(
            image,
            TextureAtlas {
                layout,
                index: PlayerFrame::Idle0.index(),
            },
        ),
        Anchor(player_anchor(PLAYER_SIZE)),
        Transform::from_translation(start.extend(PLAYER_Z)),
    ));
}

fn gather_input(
    keys: Res<ButtonInput<KeyCode>>,
    follow: Res<FollowNpc>,
    mut input: Single<&mut PlayerInput>,
) {
    // Mentre la camera segue un NPC il giocatore resta fermo.
    if follow.0 {
        input.axis = 0.0;
        input.vertical = 0.0;
        input.jump_held = false;
        return;
    }
    let left = keys.any_pressed([KeyCode::KeyA, KeyCode::ArrowLeft]);
    let right = keys.any_pressed([KeyCode::KeyD, KeyCode::ArrowRight]);
    let jump_keys = [KeyCode::Space, KeyCode::KeyW, KeyCode::ArrowUp];

    input.axis = f32::from(right as u8) - f32::from(left as u8);
    let up = keys.any_pressed([KeyCode::KeyW, KeyCode::ArrowUp]);
    let down = keys.any_pressed([KeyCode::KeyS, KeyCode::ArrowDown]);
    input.vertical = f32::from(up as u8) - f32::from(down as u8);
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

    if climb(body, input, &layout) {
        move_and_collide(body, &layout, dt);
        finish_climb(body, &layout, input.vertical);
        return;
    }

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

/// Scale a pioli: W/↑ davanti a una scala (non in cima) o S/↓ in cima
/// (sopra il foro del solaio) fanno aggrappare il giocatore, che poi sale e
/// scende senza gravità; A/D lo fanno staccare. Restituisce se è aggrappato
/// (la velocità è già impostata).
fn climb(body: &mut Body, input: &mut PlayerInput, layout: &TrainLayout) -> bool {
    let half = PLAYER_SIZE / 2.0;
    let Some(ladder) = layout.ladder_at(body.position, half) else {
        body.climbing = false;
        return false;
    };
    if !body.climbing {
        let feet = body.position.y - half.y;
        let at_top = feet >= ladder.max.y - 0.5;
        let at_bottom = feet <= ladder.min.y + 0.5;
        let grab = (input.vertical > 0.0 && !at_top) || (input.vertical < 0.0 && !at_bottom);
        if !grab {
            return false;
        }
        body.climbing = true;
        body.position.x = ladder.center().x;
    }
    if input.axis != 0.0 {
        body.climbing = false;
        return false;
    }
    // Su una scala W non fa saltare.
    input.jump_pressed = false;
    body.velocity = Vec2::new(0.0, input.vertical * CLIMB_SPEED);
    true
}

/// Arrivato in cima (sul foro del solaio) o in fondo, il giocatore si stacca.
fn finish_climb(body: &mut Body, layout: &TrainLayout, vertical: f32) {
    let half = PLAYER_SIZE / 2.0;
    let feet = body.position.y - half.y;
    match layout.ladder_at(body.position, half) {
        Some(ladder) if vertical > 0.0 && feet >= ladder.max.y => {
            body.position.y = ladder.max.y + half.y;
            body.climbing = false;
            body.grounded = true;
            body.velocity.y = 0.0;
        }
        Some(_) if vertical < 0.0 && body.grounded => body.climbing = false,
        Some(_) => {}
        None => body.climbing = false,
    }
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
    let feet_before = body.position.y - half.y;
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
    // Piattaforme (fori dei solai): reggono solo chi ci arriva da sopra e
    // non sta usando la scala.
    if !body.climbing && body.velocity.y <= 0.0 {
        for platform in &layout.platforms {
            if overlaps(body.position, half, platform) && feet_before >= platform.max.y - 0.01 {
                body.position.y = platform.max.y + half.y;
                body.grounded = true;
                body.velocity.y = 0.0;
            }
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

/// Fotogramma e verso dello sprite dalla fisica del corpo.
fn animate_player(
    time: Res<Time<Real>>,
    mut player: Query<(&Body, &Transform, &mut PlayerAnim, &mut Sprite), With<Player>>,
) {
    for (body, transform, mut anim, mut sprite) in &mut player {
        anim.clock += time.delta_secs();
        let x = transform.translation.x;
        let dx = x - anim.last_x;
        anim.last_x = x;
        // Un salto lungo (caricamento di una partita) non conta come passi.
        if body.grounded && dx.abs() < PLAYER_SIZE.x {
            anim.walked += dx.abs();
        }
        if body.velocity.x.abs() > 1.0 {
            anim.facing_left = body.velocity.x < 0.0;
        }
        // Sulla scala resta nella posa da fermo (non ha animazioni proprie).
        let (grounded, velocity) = if body.climbing {
            (true, Vec2::ZERO)
        } else {
            (body.grounded, body.velocity)
        };
        let frame = player_frame(grounded, velocity, anim.clock, anim.walked);
        if let Some(atlas) = sprite.texture_atlas.as_mut()
            && atlas.index != frame.index()
        {
            atlas.index = frame.index();
        }
        if sprite.flip_x != anim.facing_left {
            sprite.flip_x = anim.facing_left;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::train::{TrainLocation, WALL};
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

    /// Un passo fisso completo con l'input dato (come `step_physics`).
    fn step(body: &mut Body, input: &mut PlayerInput, layout: &TrainLayout) {
        body.previous = body.position;
        if climb(body, input, layout) {
            move_and_collide(body, layout, DT);
            finish_climb(body, layout, input.vertical);
            return;
        }
        body.velocity.x = input.axis * RUN_SPEED;
        body.velocity.y = (body.velocity.y - GRAVITY * DT).max(-MAX_FALL_SPEED);
        move_and_collide(body, layout, DT);
    }

    fn two_floor_layout() -> (TrainLayout, usize) {
        let kinds = vec![
            CarriageKind::Mensa,
            CarriageKind::Dormitorio,
            CarriageKind::Serra,
        ];
        let layout = TrainLayout::new(kinds);
        let index = 1;
        assert_eq!(layout.floors(index), 2);
        (layout, index)
    }

    #[test]
    fn climbs_the_ladder_to_the_upper_floor_and_back() {
        let (layout, index) = two_floor_layout();
        let (x0, x1) = TrainLayout::stairs_x(index);
        let mut body = body_at((x0 + x1) / 2.0 + 2.0);
        let mut input = PlayerInput {
            vertical: 1.0,
            ..default()
        };
        for _ in 0..64 * 4 {
            step(&mut body, &mut input, &layout);
        }
        let upper = crate::train::floor_y(1) + PLAYER_SIZE.y / 2.0;
        assert_eq!(body.position.y, upper, "at the top of the ladder");
        assert!(!body.climbing && body.grounded);

        // Standing on the hole: nothing happens without input.
        input.vertical = 0.0;
        for _ in 0..64 {
            step(&mut body, &mut input, &layout);
        }
        assert_eq!(body.position.y, upper);

        // Walks off the hole onto the slab, then comes back down the ladder.
        input.axis = 1.0;
        for _ in 0..16 {
            step(&mut body, &mut input, &layout);
        }
        assert_eq!(body.position.y, upper);
        input.axis = -1.0;
        while body.position.x > (x0 + x1) / 2.0 {
            step(&mut body, &mut input, &layout);
        }
        input.axis = 0.0;
        input.vertical = -1.0;
        for _ in 0..64 * 4 {
            step(&mut body, &mut input, &layout);
        }
        assert_eq!(
            body.position.y,
            FLOOR_Y + PLAYER_SIZE.y / 2.0,
            "back on the ground floor"
        );
        assert!(!body.climbing && body.grounded);
    }

    #[test]
    fn upper_floor_ends_are_closed() {
        let (layout, index) = two_floor_layout();
        let (x0, x1) = TrainLayout::stairs_x(index);
        let mut body = body_at((x0 + x1) / 2.0);
        body.position.y += crate::train::STOREY;
        let mut input = PlayerInput {
            axis: -1.0,
            ..default()
        };
        for _ in 0..64 * 2 {
            step(&mut body, &mut input, &layout);
        }
        let wall = TrainLayout::carriage_left(index) + WALL;
        assert_eq!(body.position.x, wall + PLAYER_SIZE.x / 2.0);
        assert_eq!(
            body.position.y,
            crate::train::floor_y(1) + PLAYER_SIZE.y / 2.0
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
