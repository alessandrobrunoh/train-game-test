//! Disegno degli NPC della simulazione e selezione con il mouse.
//!
//! Si disegnano solo gli NPC della "finestra visibile": le carrozze inquadrate
//! dalla camera più una per lato, più chi sta viaggiando attraverso di esse.
//! Ogni NPC visibile ha un'entità `NpcSprite`; esce dalla finestra o muore e
//! l'entità viene rimossa. La posizione deriva dall'azione corrente:
//! - mangia/dorme/lavora: alla sua postazione (vedi `stations.rs`), con un
//!   posto diverso per ognuno di chi la condivide;
//! - socializza: accanto al compagno;
//! - ozia: in un punto deterministico della carrozza, diverso a ogni pausa;
//! - viaggia: interpolato tra il centro della carrozza di partenza e quello
//!   di arrivo in base all'avanzamento dell'azione.
//!
//! Gli sprite camminano verso la posizione obiettivo e si teletrasportano se
//! è troppo lontana (es. a velocità di gioco alte).

use std::collections::HashMap;

use bevy::{prelude::*, window::PrimaryWindow};
use sim::{Action, CarriageId, Job, Npc, NpcId, World};

use crate::sim_bridge::SimTickSet;
use crate::state::{NpcSprite, PointerOverUi, SelectedNpc, Sim};
use crate::stations::{BED_TOP, StationLayout, interior_range};
use crate::train::{CARRIAGE_PITCH, FLOOR_Y, TrainLayout};

/// Dimensioni di un NPC adulto e di un bambino (larghezza, altezza).
const ADULT_SIZE: Vec2 = Vec2::new(8.0, 16.0);
const CHILD_SIZE: Vec2 = Vec2::new(6.0, 11.0);
/// Età sotto la quale un NPC è disegnato come bambino.
const ADULT_AGE: u32 = 18;
const ELDER_AGE: u32 = 65;
/// Chi è sdraiato è disegnato più sottile, per leggersi come "a letto".
const LYING_THICKNESS: f32 = 0.75;
/// Spessore del bordo scuro attorno a ogni NPC.
const OUTLINE: f32 = 1.0;

const NPC_Z: f32 = 3.0;
/// Scarto di profondità tra un NPC e l'altro (1000 NPC stanno in 0.5).
const Z_STEP: f32 = 0.0005;
const MARKER_Z: f32 = 6.0;

/// Velocità di camminata verso l'obiettivo (unità/s)...
const WALK_SPEED: f32 = 40.0;
/// ...che aumenta con la distanza, per non restare indietro a lungo (1/s).
const CATCH_UP: f32 = 3.0;
/// Oltre questa distanza lo sprite si teletrasporta.
const TELEPORT_DISTANCE: f32 = 120.0;
/// Distanza tra due NPC che chiacchierano.
const CHAT_DISTANCE: f32 = 10.0;
/// Tolleranza del click attorno al rettangolo di un NPC.
const PICK_MARGIN: f32 = 3.0;

const OUTLINE_COLOR: Color = Color::srgb(0.08, 0.08, 0.10);
const SELECTED_COLOR: Color = Color::srgb(0.30, 1.00, 1.00);
const CONTADINO_COLOR: Color = Color::srgb(0.55, 0.90, 0.35);
const CUOCO_COLOR: Color = Color::srgb(0.96, 0.96, 0.96);
const OPERAIO_COLOR: Color = Color::srgb(0.30, 0.55, 1.00);
const JOBLESS_COLOR: Color = Color::srgb(0.75, 0.45, 0.85);
const CHILD_COLOR: Color = Color::srgb(1.00, 0.60, 0.75);
const ELDER_COLOR: Color = Color::srgb(0.72, 0.72, 0.70);
/// Chi dorme è disegnato più scuro.
const SLEEP_DARKEN: f32 = 0.6;

pub struct NpcRenderPlugin;

impl Plugin for NpcRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NpcSpriteIndex>()
            .add_systems(Startup, spawn_marker)
            .add_systems(
                Update,
                (
                    pick_npc,
                    sync_npc_sprites,
                    move_npc_sprites,
                    update_selection,
                )
                    .chain()
                    .after(SimTickSet),
            );
    }
}

/// Bordo scuro dietro lo sprite di un NPC (figlio dell'entità `NpcSprite`).
#[derive(Component)]
struct NpcOutline;

/// Stato grafico di uno sprite NPC.
#[derive(Component)]
struct NpcVisual {
    outline: Entity,
    /// Dove lo sprite vuole andare (coordinate mondo).
    target: Vec2,
    /// Semiampiezze del rettangolo in coordinate mondo (per il click).
    half_extents: Vec2,
}

/// Marcatore sopra la testa dell'NPC selezionato.
#[derive(Component)]
struct SelectionMarker;

/// NPC attualmente disegnati: id -> (entità, ultimo frame in cui era visibile).
#[derive(Resource, Default)]
struct NpcSpriteIndex {
    entities: HashMap<NpcId, (Entity, u32)>,
    frame: u32,
}

/// Posa di un NPC calcolata dalla sua azione.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pose {
    /// Centro dello sprite in coordinate mondo.
    position: Vec2,
    /// Sdraiato (a letto).
    lying: bool,
    /// Scala della lunghezza del corpo, per stare nei letti stretti.
    length_scale: f32,
}

// --- Calcolo della posa (dati puri) ----------------------------------------

/// Numero pseudo-casuale in `0..1` da una chiave (splitmix64).
fn hash01(key: u64) -> f32 {
    let mut z = key.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 40) as f32 / (1u64 << 24) as f32
}

fn carriage_x(carriage: CarriageId, local_x: f32) -> f32 {
    TrainLayout::carriage_left(carriage.index()) + local_x
}

/// Punto casuale ma deterministico del pavimento di una carrozza (x mondo).
fn floor_spot(carriage: CarriageId, key: u64) -> f32 {
    let (left, right) = interior_range();
    let margin = ADULT_SIZE.x;
    carriage_x(
        carriage,
        left + margin + hash01(key) * (right - left - 2.0 * margin),
    )
}

/// Dove due NPC si incontrano per chiacchierare (uguale per entrambi).
fn chat_spot(carriage: CarriageId, a: NpcId, b: NpcId) -> f32 {
    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
    floor_spot(
        carriage,
        (u64::from(lo.0) << 32) | u64::from(hi.0) | 1 << 63,
    )
}

fn body_size(npc: &Npc) -> Vec2 {
    if npc.age < ADULT_AGE {
        CHILD_SIZE
    } else {
        ADULT_SIZE
    }
}

/// Punto di riferimento dell'NPC, senza seguire chi chiacchiera con chi
/// (evita ricorsioni): postazione, punto d'incontro o punto di pausa.
fn anchor_x(world: &World, stations: &StationLayout, npc: &Npc) -> f32 {
    match npc.action {
        Action::Eat(s) | Action::Sleep(s) | Action::Work(s) => {
            match stations.spot(npc.carriage, s) {
                Some(spot) => carriage_x(npc.carriage, spot.x),
                None => idle_x(npc),
            }
        }
        Action::Socialize(other) => chat_spot(npc.carriage, npc.id, other),
        Action::Travel { to } => travel_x(world, npc, to),
        Action::Idle => idle_x(npc),
    }
}

fn idle_x(npc: &Npc) -> f32 {
    floor_spot(
        npc.carriage,
        (u64::from(npc.id.0) << 32) ^ npc.action_since.minutes(),
    )
}

fn travel_x(world: &World, npc: &Npc, to: CarriageId) -> f32 {
    let from = TrainLayout::carriage_center_x(npc.carriage.index());
    let dest = TrainLayout::carriage_center_x(to.index());
    from + (dest - from) * npc.action_progress(world.clock)
}

/// Posa dell'NPC. `seat` è il suo indice tra chi usa la stessa postazione.
fn npc_pose(world: &World, stations: &StationLayout, npc: &Npc, seat: u16) -> Pose {
    let size = body_size(npc);
    let standing = |x: f32| Pose {
        position: Vec2::new(x, FLOOR_Y + size.y / 2.0),
        lying: false,
        length_scale: 1.0,
    };
    let c = npc.carriage;
    match npc.action {
        Action::Sleep(s) => match stations.spot(c, s) {
            Some(spot) => Pose {
                position: Vec2::new(
                    carriage_x(c, spot.x),
                    spot.base_y() + BED_TOP + size.x * LYING_THICKNESS / 2.0,
                ),
                lying: true,
                length_scale: ((spot.width - 2.0) / size.y).clamp(0.3, 1.0),
            },
            None => standing(idle_x(npc)),
        },
        Action::Eat(s) | Action::Work(s) => {
            let Some(spot) = stations.spot(c, s) else {
                return standing(idle_x(npc));
            };
            let capacity = world
                .carriage(c)
                .and_then(|carriage| carriage.station(s))
                .map_or(1, |st| st.capacity.max(1));
            // Chi condivide la postazione si distribuisce sulla sua larghezza.
            let offset = if capacity > 1 {
                let t = f32::from(seat % capacity) / f32::from(capacity - 1);
                (t - 0.5) * (spot.width - size.x).max(0.0)
            } else {
                0.0
            };
            standing(carriage_x(c, spot.x) + offset)
        }
        Action::Socialize(other) => {
            let side = if npc.id < other { -1.0 } else { 1.0 };
            let x = match world.npc(other) {
                // Si parlano a vicenda: ai due lati del punto d'incontro.
                Some(p) if p.carriage == c && p.action == Action::Socialize(npc.id) => {
                    chat_spot(c, npc.id, other) + side * CHAT_DISTANCE / 2.0
                }
                // Il compagno fa altro (es. mangia): gli si mette accanto.
                Some(p) if p.carriage == c => anchor_x(world, stations, p) + side * CHAT_DISTANCE,
                _ => idle_x(npc),
            };
            let (left, right) = interior_range();
            standing(x.clamp(carriage_x(c, left), carriage_x(c, right)))
        }
        Action::Travel { to } => standing(travel_x(world, npc, to)),
        Action::Idle => standing(idle_x(npc)),
    }
}

fn npc_color(npc: &Npc) -> Color {
    let base = match npc.job {
        Some(Job::Contadino) => CONTADINO_COLOR,
        Some(Job::Cuoco) => CUOCO_COLOR,
        Some(Job::Operaio) => OPERAIO_COLOR,
        None if npc.age < ADULT_AGE => CHILD_COLOR,
        None if npc.age >= ELDER_AGE => ELDER_COLOR,
        None => JOBLESS_COLOR,
    };
    if npc.is_awake() {
        base
    } else {
        let c = base.to_srgba();
        Color::srgb(
            c.red * SLEEP_DARKEN,
            c.green * SLEEP_DARKEN,
            c.blue * SLEEP_DARKEN,
        )
    }
}

/// Carrozze della finestra visibile: quelle inquadrate più una per lato.
fn visible_window(camera_x: f32, view: Rect, carriages: usize) -> (usize, usize) {
    let last = carriages.saturating_sub(1) as i64;
    let index = |x: f32| (x / CARRIAGE_PITCH).floor() as i64;
    let lo = (index(camera_x + view.min.x) - 1).clamp(0, last);
    let hi = (index(camera_x + view.max.x) + 1).clamp(0, last);
    (lo as usize, hi as usize)
}

/// Vero se l'NPC va disegnato con questa finestra visibile.
fn in_window(npc: &Npc, lo: usize, hi: usize) -> bool {
    let here = npc.carriage.index();
    match npc.action {
        // Chi viaggia è visibile se il suo tragitto attraversa la finestra.
        Action::Travel { to } => {
            let (a, b) = (here.min(to.index()), here.max(to.index()));
            a <= hi && b >= lo
        }
        _ => (lo..=hi).contains(&here),
    }
}

// --- Sistemi ----------------------------------------------------------------

fn spawn_marker(mut commands: Commands) {
    commands.spawn((
        Name::new("Marcatore selezione"),
        SelectionMarker,
        Sprite::from_color(SELECTED_COLOR, Vec2::splat(4.0)),
        Transform::from_xyz(0.0, 0.0, MARKER_Z)
            .with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_4)),
        Visibility::Hidden,
    ));
}

/// Crea, aggiorna e rimuove gli sprite degli NPC della finestra visibile.
#[allow(clippy::too_many_arguments)]
fn sync_npc_sprites(
    mut commands: Commands,
    sim: Res<Sim>,
    stations: Res<StationLayout>,
    selected: Res<SelectedNpc>,
    camera: Single<(&Transform, &Projection), With<Camera2d>>,
    mut index: ResMut<NpcSpriteIndex>,
    mut sprites: Query<(&mut Sprite, &mut NpcVisual), With<NpcSprite>>,
    mut outlines: Query<&mut Sprite, (With<NpcOutline>, Without<NpcSprite>)>,
    mut seats: Local<HashMap<(CarriageId, u16), u16>>,
) {
    let world = &sim.world;
    let (camera_transform, projection) = *camera;
    let Projection::Orthographic(ortho) = projection else {
        return;
    };
    let (lo, hi) = visible_window(
        camera_transform.translation.x,
        ortho.area,
        world.carriages.len(),
    );

    index.frame = index.frame.wrapping_add(1);
    let frame = index.frame;
    seats.clear();

    for npc in &world.npcs {
        if !in_window(npc, lo, hi) {
            continue;
        }
        let seat = match npc.action.station() {
            Some(s) => {
                let next = seats.entry((npc.carriage, s.0)).or_insert(0);
                *next += 1;
                *next - 1
            }
            None => 0,
        };
        let pose = npc_pose(world, &stations, npc, seat);
        let size = body_size(npc);
        let color = npc_color(npc);
        let outline_color = if selected.0 == Some(npc.id) {
            SELECTED_COLOR
        } else {
            OUTLINE_COLOR
        };
        let length = size.y * pose.length_scale;
        let width = if pose.lying {
            size.x * LYING_THICKNESS
        } else {
            size.x
        };
        let half_extents = if pose.lying {
            Vec2::new(length, width) / 2.0
        } else {
            size / 2.0
        };

        let body = Vec2::new(width, length);
        let framed = body + Vec2::splat(2.0 * OUTLINE);
        let existing = index.entities.get(&npc.id).map(|&(e, _)| e);
        if let Some(entity) = existing
            && let Ok((mut sprite, mut visual)) = sprites.get_mut(entity)
        {
            visual.target = pose.position;
            visual.half_extents = half_extents;
            if sprite.color != color {
                sprite.color = color;
            }
            if sprite.custom_size != Some(body) {
                sprite.custom_size = Some(body);
            }
            if let Ok(mut outline) = outlines.get_mut(visual.outline) {
                if outline.color != outline_color {
                    outline.color = outline_color;
                }
                if outline.custom_size != Some(framed) {
                    outline.custom_size = Some(framed);
                }
            }
            index.entities.insert(npc.id, (entity, frame));
            continue;
        }

        if let Some(stale) = existing {
            commands.entity(stale).despawn();
        }
        let outline = commands
            .spawn((
                NpcOutline,
                Sprite::from_color(outline_color, framed),
                Transform::from_xyz(0.0, 0.0, -Z_STEP / 2.0),
            ))
            .id();
        // Ogni NPC ha una profondità propria: niente sfarfallio tra sprite sovrapposti.
        let z = NPC_Z + (npc.id.0 % 1000) as f32 * Z_STEP;
        let entity = commands
            .spawn((
                Name::new(npc.name.clone()),
                NpcSprite(npc.id),
                NpcVisual {
                    outline,
                    target: pose.position,
                    half_extents,
                },
                Sprite::from_color(color, body),
                Transform::from_translation(pose.position.extend(z))
                    .with_rotation(lying_rotation(pose.lying)),
            ))
            .add_child(outline)
            .id();
        index.entities.insert(npc.id, (entity, frame));
    }

    // Via chi è uscito dalla finestra o è morto.
    index.entities.retain(|_, &mut (entity, seen)| {
        if seen != frame {
            commands.entity(entity).despawn();
        }
        seen == frame
    });
}

fn lying_rotation(lying: bool) -> Quat {
    if lying {
        Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)
    } else {
        Quat::IDENTITY
    }
}

/// Muove gli sprite verso la loro posizione obiettivo.
fn move_npc_sprites(
    time: Res<Time>,
    mut sprites: Query<(&mut Transform, &NpcVisual), With<NpcSprite>>,
) {
    let dt = time.delta_secs();
    for (mut transform, visual) in &mut sprites {
        let current = transform.translation.truncate();
        let delta = visual.target - current;
        let distance = delta.length();
        let next = if distance > TELEPORT_DISTANCE {
            visual.target
        } else {
            let step = (WALK_SPEED + distance * CATCH_UP) * dt;
            current + delta.clamp_length_max(step)
        };
        if next != current {
            transform.translation.x = next.x;
            transform.translation.y = next.y;
        }
        // Sdraiato solo quando è arrivato al letto.
        let lying = visual.half_extents.x > visual.half_extents.y;
        let rotation = lying_rotation(lying && next == visual.target);
        if transform.rotation != rotation {
            transform.rotation = rotation;
        }
    }
}

/// Click sinistro nel mondo: seleziona l'NPC sotto il cursore o deseleziona.
fn pick_npc(
    buttons: Res<ButtonInput<MouseButton>>,
    over_ui: Res<PointerOverUi>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera2d>>,
    sprites: Query<(&NpcSprite, &Transform, &NpcVisual)>,
    mut selected: ResMut<SelectedNpc>,
) {
    if !buttons.just_pressed(MouseButton::Left) || over_ui.0 {
        return;
    }
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let (camera, camera_transform) = *camera;
    let Ok(point) = camera.viewport_to_world_2d(camera_transform, cursor) else {
        return;
    };
    let hit = sprites
        .iter()
        .filter_map(|(npc, transform, visual)| {
            let center = transform.translation.truncate();
            let d = (point - center).abs();
            let reach = visual.half_extents + Vec2::splat(PICK_MARGIN);
            (d.x <= reach.x && d.y <= reach.y).then(|| (npc.0, point.distance_squared(center)))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id);
    selected.0 = hit;
}

/// Mostra il marcatore sopra l'NPC selezionato (se è disegnato).
fn update_selection(
    time: Res<Time>,
    selected: Res<SelectedNpc>,
    index: Res<NpcSpriteIndex>,
    sprites: Query<(&Transform, &NpcVisual), Without<SelectionMarker>>,
    mut marker: Single<(&mut Transform, &mut Visibility), With<SelectionMarker>>,
) {
    let (marker_transform, visibility) = &mut *marker;
    let target = selected
        .0
        .and_then(|id| index.entities.get(&id))
        .and_then(|&(entity, _)| sprites.get(entity).ok());
    let Some((transform, visual)) = target else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    visibility.set_if_neq(Visibility::Visible);
    let bob = (time.elapsed_secs() * 4.0).sin() * 1.5;
    let top = transform.translation.y + visual.half_extents.y;
    marker_transform.translation.x = transform.translation.x;
    marker_transform.translation.y = top + 6.0 + bob;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_in_unit_range_and_deterministic() {
        for k in 0..1000 {
            let h = hash01(k);
            assert!((0.0..1.0).contains(&h));
            assert_eq!(h, hash01(k));
        }
    }

    #[test]
    fn window_covers_view_plus_one_carriage_per_side() {
        let view = Rect::new(-213.0, -120.0, 213.0, 120.0);
        // Camera al centro della carrozza 5.
        let x = TrainLayout::carriage_center_x(5);
        assert_eq!(visible_window(x, view, 20), (3, 7));
        assert_eq!(visible_window(0.0, view, 20), (0, 1));
        assert_eq!(visible_window(1e6, view, 20), (19, 19));
    }

    #[test]
    fn poses_stay_inside_the_train_over_a_day() {
        let mut sim = crate::sim_bridge::new_sim();
        let stations = StationLayout::from_world(&sim.world);
        let layout = TrainLayout::from_world(&sim.world);
        let (min_x, max_x) = layout.inner_bounds();
        for _ in 0..24 * 60 / 5 {
            for _ in 0..5 {
                sim.world.tick(&mut sim.brain);
            }
            let mut seats: HashMap<(CarriageId, u16), u16> = HashMap::new();
            for npc in &sim.world.npcs {
                let seat = npc.action.station().map_or(0, |s| {
                    let n = seats.entry((npc.carriage, s.0)).or_insert(0);
                    *n += 1;
                    *n - 1
                });
                let pose = npc_pose(&sim.world, &stations, npc, seat);
                assert!(
                    (min_x..=max_x).contains(&pose.position.x),
                    "{npc:?} {pose:?}"
                );
                assert!(pose.position.y > FLOOR_Y, "{npc:?} {pose:?}");
                if !matches!(npc.action, Action::Travel { .. }) {
                    // Chi non viaggia resta nella sua carrozza.
                    let left = TrainLayout::carriage_left(npc.carriage.index());
                    let (a, b) = interior_range();
                    assert!(
                        (left + a..=left + b).contains(&pose.position.x),
                        "{npc:?} {pose:?}"
                    );
                }
            }
        }
    }
}
