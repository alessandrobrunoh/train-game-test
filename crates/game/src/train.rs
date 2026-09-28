//! Layout del treno (dati puri) e spawn della grafica delle carrozze
//! (arte in `env_art.rs`; esterno, binari e cielo in `background.rs`).
//!
//! Numero, tipo e nome delle carrozze arrivano dalla simulazione (`Sim`):
//! la carrozza di indice `i` qui è la `CarriageId(i)` della sim. Il resto della
//! geometria deriva dalle costanti qui sotto. Le unità del mondo sono "pixel"
//! di pixel art (la camera le ingrandisce, vedi `camera.rs`).

use bevy::prelude::*;
use bevy::sprite::Text2dShadow;
use sim::{CarriageKind, World};

use crate::env_art::{self, ArtCache, ArtKey, ExteriorArt, InteriorArt, LampGlow, art_sprite};
use crate::saves::WorldRebuildSet;
use crate::state::{Sim, WorldReplaced};

// --- Costanti di layout -----------------------------------------------------

/// Lunghezza esterna di una carrozza (pareti di testata incluse).
pub const CARRIAGE_LENGTH: f32 = 320.0;
/// Altezza interna calpestabile (dal pavimento al soffitto).
pub const INTERIOR_HEIGHT: f32 = 104.0;
/// Spessore di pavimento, soffitto e pareti.
pub const WALL: f32 = 8.0;
/// Spazio tra due carrozze, coperto dal soffietto di passaggio.
pub const GANGWAY: f32 = 16.0;
/// Altezza del vano porta tra carrozze adiacenti.
pub const DOOR_HEIGHT: f32 = 48.0;
/// Passo tra l'inizio di una carrozza e l'inizio della successiva.
pub const CARRIAGE_PITCH: f32 = CARRIAGE_LENGTH + GANGWAY;
/// Quota della superficie del pavimento.
pub const FLOOR_Y: f32 = 0.0;

// Profondità (z) dei vari strati.
const Z_BACKGROUND: f32 = -10.0;
/// Aloni delle lampade, subito davanti alla parete di fondo.
const Z_GLOW: f32 = -9.0;
const Z_GANGWAY: f32 = -9.5;
const Z_DECOR: f32 = -5.0;
const Z_STRUCTURE: f32 = 0.0;
const Z_LABEL: f32 = 5.0;

/// Fotogrammi al secondo della rotazione delle ruote (indipendenti dalla sim:
/// il treno è sempre in corsa).
const WHEEL_FPS: f32 = 16.0;

const LABEL_COLOR: Color = Color::srgb(0.95, 0.93, 0.86);
const LABEL_SHADOW: Color = Color::srgba(0.02, 0.02, 0.06, 0.9);

// --- Layout -----------------------------------------------------------------

/// Posizione lungo il treno, usata ad esempio dall'HUD.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrainLocation {
    /// Dentro la carrozza con questo indice.
    Carriage(usize),
    /// Nel soffietto tra la carrozza `i` e la `i + 1`.
    Gangway(usize),
}

/// Descrizione statica del treno: tipi di carrozza e solidi per le collisioni.
#[derive(Resource, Debug)]
pub struct TrainLayout {
    /// Tipo di ogni carrozza, nell'ordine della sim (indice = `CarriageId`).
    pub carriages: Vec<CarriageKind>,
    /// Rettangoli solidi (pavimenti, soffitti, pareti) in coordinate mondo.
    pub solids: Vec<Rect>,
}

impl TrainLayout {
    pub fn new(carriages: Vec<CarriageKind>) -> Self {
        let count = carriages.len();
        let solids = (0..count)
            .flat_map(|i| carriage_solids(i, count))
            .collect();
        Self { carriages, solids }
    }

    /// Layout delle carrozze del mondo simulato.
    pub fn from_world(world: &World) -> Self {
        Self::new(world.carriages.iter().map(|c| c.kind).collect())
    }

    pub fn len(&self) -> usize {
        self.carriages.len()
    }

    /// Coordinata x del bordo sinistro della carrozza `index`.
    pub fn carriage_left(index: usize) -> f32 {
        index as f32 * CARRIAGE_PITCH
    }

    /// Centro (x) della carrozza `index`.
    pub fn carriage_center_x(index: usize) -> f32 {
        Self::carriage_left(index) + CARRIAGE_LENGTH / 2.0
    }

    /// Limiti interni percorribili del treno (facce interne delle testate estreme).
    pub fn inner_bounds(&self) -> (f32, f32) {
        let last = self.len().saturating_sub(1);
        (WALL, Self::carriage_left(last) + CARRIAGE_LENGTH - WALL)
    }

    /// In quale carrozza (o soffietto) si trova la coordinata x.
    pub fn location_at(&self, x: f32) -> TrainLocation {
        let last = self.len().saturating_sub(1);
        let index = ((x / CARRIAGE_PITCH).floor().max(0.0) as usize).min(last);
        let local = x - Self::carriage_left(index);
        if local > CARRIAGE_LENGTH && index < last {
            TrainLocation::Gangway(index)
        } else {
            TrainLocation::Carriage(index)
        }
    }
}

/// Rettangoli solidi (pavimento, soffitto, pareti di testata e soffietto alla
/// sua destra) della carrozza `index`, in coordinate mondo. La grafica è in
/// `env_art.rs` e ricalca questi rettangoli.
fn carriage_solids(index: usize, count: usize) -> Vec<Rect> {
    let x0 = TrainLayout::carriage_left(index);
    let x1 = x0 + CARRIAGE_LENGTH;
    let floor = FLOOR_Y;
    let ceil = FLOOR_Y + INTERIOR_HEIGHT;
    let is_first = index == 0;
    let is_last = index + 1 == count;

    let mut solids = vec![
        // Pavimento e soffitto
        Rect::new(x0, floor - WALL, x1, floor),
        Rect::new(x0, ceil, x1, ceil + WALL),
    ];
    // Pareti di testata: piene alle estremità del treno, altrimenti solo
    // l'architrave sopra il vano porta.
    let (left_bottom, right_bottom) = end_wall_bottoms(is_first, is_last);
    solids.push(Rect::new(x0, left_bottom, x0 + WALL, ceil));
    solids.push(Rect::new(x1 - WALL, right_bottom, x1, ceil));
    // Pedana e architrave del soffietto verso la carrozza successiva.
    if !is_last {
        let g1 = x1 + GANGWAY;
        solids.push(Rect::new(x1, floor - WALL, g1, floor));
        solids.push(Rect::new(
            x1,
            floor + DOOR_HEIGHT,
            g1,
            floor + DOOR_HEIGHT + WALL,
        ));
    }
    solids
}

/// Quota del bordo inferiore delle pareti di testata (sinistra, destra).
fn end_wall_bottoms(is_first: bool, is_last: bool) -> (f32, f32) {
    let door = FLOOR_Y + DOOR_HEIGHT;
    (
        if is_first { FLOOR_Y } else { door },
        if is_last { FLOOR_Y } else { door },
    )
}

// --- Plugin e spawn ---------------------------------------------------------

/// Marca l'entità radice di una carrozza (i figli sono i suoi sprite).
#[derive(Component, Debug)]
pub struct Carriage;

/// Ruota di un carrello: il suo fotogramma cambia per farla girare.
#[derive(Component, Debug)]
struct Wheel;

/// Fotogrammi della rotazione delle ruote.
#[derive(Resource, Default)]
struct WheelFrames(Vec<Handle<Image>>);

pub struct TrainPlugin;

impl Plugin for TrainPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ArtCache>()
            .init_resource::<WheelFrames>()
            .add_systems(Startup, spawn_carriages)
            .add_systems(
                PreUpdate,
                rebuild_train
                    .in_set(WorldRebuildSet)
                    .run_if(on_message::<WorldReplaced>),
            )
            .add_systems(Update, spin_wheels);
    }

    /// Il layout nasce dalla sim, inserita da `SimBridgePlugin`: `finish` gira
    /// dopo il `build` di tutti i plugin, quindi l'ordine in `main.rs` non conta.
    fn finish(&self, app: &mut App) {
        let sim = app
            .world()
            .get_resource::<Sim>()
            .expect("TrainPlugin richiede la risorsa Sim (SimBridgePlugin)");
        let layout = TrainLayout::from_world(&sim.world);
        app.insert_resource(layout);
    }
}

/// Mondo sostituito (caricamento o nuova partita): il numero e il tipo delle
/// carrozze possono cambiare, quindi layout e carrozze si rifanno da capo.
fn rebuild_train(
    mut commands: Commands,
    sim: Res<Sim>,
    mut layout: ResMut<TrainLayout>,
    old: Query<Entity, With<Carriage>>,
    mut art: ResMut<ArtCache>,
    mut wheels: ResMut<WheelFrames>,
    mut images: Option<ResMut<Assets<Image>>>,
) {
    for entity in &old {
        commands.entity(entity).despawn();
    }
    *layout = TrainLayout::from_world(&sim.world);
    spawn_carriage_entities(
        &mut commands,
        &layout,
        &sim.world,
        &mut art,
        &mut wheels,
        images.as_deref_mut(),
    );
}

/// Crea le entità grafiche di ogni carrozza, raggruppate sotto una radice.
fn spawn_carriages(
    mut commands: Commands,
    layout: Res<TrainLayout>,
    sim: Res<Sim>,
    mut art: ResMut<ArtCache>,
    mut wheels: ResMut<WheelFrames>,
    mut images: Option<ResMut<Assets<Image>>>,
) {
    spawn_carriage_entities(
        &mut commands,
        &layout,
        &sim.world,
        &mut art,
        &mut wheels,
        images.as_deref_mut(),
    );
}

fn spawn_carriage_entities(
    commands: &mut Commands,
    layout: &TrainLayout,
    world: &World,
    art: &mut ArtCache,
    wheels: &mut WheelFrames,
    mut images: Option<&mut Assets<Image>>,
) {
    wheels.0 = (0..env_art::WHEEL_FRAMES)
        .map(|f| art.get(images.as_deref_mut(), ArtKey::Wheel(f), || env_art::wheel(f)))
        .collect();
    let bogie = art.get(images.as_deref_mut(), ArtKey::BogieFrame, env_art::bogie_frame);
    let gangway = art.get(images.as_deref_mut(), ArtKey::Gangway, env_art::gangway);

    let count = layout.len();
    for (index, &kind) in layout.carriages.iter().enumerate() {
        let origin = Vec2::new(TrainLayout::carriage_left(index), FLOOR_Y);
        let title = match world.carriages.get(index) {
            Some(c) => format!("{} «{}»", c.kind, c.name),
            None => kind.name().to_string(),
        };
        let interior = art.get(images.as_deref_mut(), ArtKey::Interior(kind), || {
            env_art::interior(kind)
        });
        let body = art.get(images.as_deref_mut(), ArtKey::Body(kind), || {
            env_art::body(kind)
        });
        let (left_bottom, right_bottom) = end_wall_bottoms(index == 0, index + 1 == count);
        let mut ends = Vec::new();
        for (bottom, right) in [(left_bottom, false), (right_bottom, true)] {
            let door = bottom > FLOOR_Y;
            let image = art.get(images.as_deref_mut(), ArtKey::EndWall { door }, || {
                env_art::end_wall(door)
            });
            ends.push((image, bottom - FLOOR_Y, right));
        }
        let (glow_kind, strength) = env_art::glow_for(kind);
        let glow = art.get(images.as_deref_mut(), ArtKey::Glow(glow_kind), || {
            env_art::glow(glow_kind)
        });

        commands
            .spawn((
                Name::new(format!("Carrozza {} ({})", index + 1, title)),
                Carriage,
                Transform::from_translation(origin.extend(0.0)),
                Visibility::default(),
            ))
            .with_children(|parent| {
                let mid = CARRIAGE_LENGTH / 2.0;
                // Parete di fondo con i finestrini e aloni delle lampade.
                parent.spawn((
                    InteriorArt,
                    art_sprite(interior, Vec2::new(CARRIAGE_LENGTH, INTERIOR_HEIGHT)),
                    Transform::from_xyz(mid, INTERIOR_HEIGHT / 2.0, Z_BACKGROUND),
                ));
                for x in env_art::LAMP_XS {
                    let mut sprite = art_sprite(
                        glow.clone(),
                        Vec2::new(env_art::GLOW_W, env_art::GLOW_H),
                    );
                    sprite.color = Color::WHITE.with_alpha(strength * 0.4);
                    parent.spawn((
                        LampGlow { strength },
                        sprite,
                        Transform::from_xyz(x as f32, env_art::LAMP_Y - 12.0, Z_GLOW),
                    ));
                }
                // Tetto, pavimento e telaio.
                let body_h = env_art::ROOF_TOP - env_art::BODY_BOTTOM;
                parent.spawn((
                    ExteriorArt,
                    art_sprite(body, Vec2::new(CARRIAGE_LENGTH, body_h)),
                    Transform::from_xyz(mid, env_art::BODY_BOTTOM + body_h / 2.0, Z_STRUCTURE),
                ));
                // Pareti di testata.
                for (image, bottom, right) in ends {
                    let h = INTERIOR_HEIGHT - bottom;
                    let mut sprite = art_sprite(image, Vec2::new(WALL, h));
                    sprite.flip_x = right;
                    let x = if right {
                        CARRIAGE_LENGTH - WALL / 2.0
                    } else {
                        WALL / 2.0
                    };
                    parent.spawn((
                        ExteriorArt,
                        sprite,
                        Transform::from_xyz(x, bottom + h / 2.0, Z_STRUCTURE),
                    ));
                }
                // Carrelli con le ruote.
                let rail_top = -WALL - env_art::BOGIE_H;
                for bx in [env_art::BOGIE_INSET, CARRIAGE_LENGTH - env_art::BOGIE_INSET] {
                    parent.spawn((
                        ExteriorArt,
                        art_sprite(
                            bogie.clone(),
                            Vec2::new(env_art::BOGIE_W, env_art::BOGIE_H),
                        ),
                        Transform::from_xyz(bx, rail_top + env_art::BOGIE_H / 2.0, Z_DECOR),
                    ));
                    for dx in [-env_art::WHEEL_OFFSET, env_art::WHEEL_OFFSET] {
                        parent.spawn((
                            Wheel,
                            ExteriorArt,
                            art_sprite(
                                wheels.0.first().cloned().unwrap_or_default(),
                                Vec2::splat(env_art::WHEEL_SIZE),
                            ),
                            Transform::from_xyz(
                                bx + dx,
                                rail_top + env_art::WHEEL_SIZE / 2.0,
                                Z_DECOR + 0.1,
                            ),
                        ));
                    }
                }
                // Soffietto verso la carrozza successiva.
                if index + 1 < count {
                    let h = env_art::GANGWAY_TOP - env_art::GANGWAY_BOTTOM;
                    parent.spawn((
                        ExteriorArt,
                        art_sprite(gangway.clone(), Vec2::new(GANGWAY, h)),
                        Transform::from_xyz(
                            CARRIAGE_LENGTH + GANGWAY / 2.0,
                            env_art::GANGWAY_BOTTOM + h / 2.0,
                            Z_GANGWAY,
                        ),
                    ));
                }

                // Etichetta sopra il tetto. Font grande e scala ridotta: il testo
                // resta nitido nonostante lo zoom della camera.
                parent.spawn((
                    Text2d::new(title),
                    TextFont {
                        font_size: FontSize::Px(48.0),
                        ..default()
                    },
                    TextColor(LABEL_COLOR),
                    Text2dShadow {
                        offset: Vec2::new(4.0, -4.0),
                        color: LABEL_SHADOW,
                    },
                    Transform::from_xyz(mid, env_art::ROOF_TOP + 9.0, Z_LABEL)
                        .with_scale(Vec3::splat(0.25)),
                ));
            });
    }
}

/// Fa girare le ruote a velocità costante (il treno non si ferma mai).
fn spin_wheels(
    time: Res<Time<Real>>,
    frames: Res<WheelFrames>,
    mut wheels: Query<&mut Sprite, With<Wheel>>,
    mut last: Local<Option<usize>>,
) {
    if frames.0.is_empty() {
        return;
    }
    let frame = (time.elapsed_secs_f64() * f64::from(WHEEL_FPS)) as usize % frames.0.len();
    if *last == Some(frame) {
        return;
    }
    *last = Some(frame);
    for mut sprite in &mut wheels {
        sprite.image = frames.0[frame].clone();
    }
}
