//! Layout del treno (dati puri) e spawn della grafica placeholder.
//!
//! Numero, tipo e nome delle carrozze arrivano dalla simulazione (`Sim`):
//! la carrozza di indice `i` qui è la `CarriageId(i)` della sim. Il resto della
//! geometria deriva dalle costanti qui sotto. Le unità del mondo sono "pixel"
//! di pixel art (la camera le ingrandisce, vedi `camera.rs`).

use bevy::prelude::*;
use sim::{CarriageKind, World};

use crate::state::Sim;

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
const Z_DECOR: f32 = -5.0;
const Z_STRUCTURE: f32 = 0.0;
const Z_LABEL: f32 = 5.0;

const STRUCTURE_COLOR: Color = Color::srgb(0.20, 0.20, 0.24);
const GANGWAY_COLOR: Color = Color::srgb(0.12, 0.12, 0.14);
const WINDOW_COLOR: Color = Color::srgb(0.62, 0.80, 0.92);
const BOGIE_COLOR: Color = Color::srgb(0.08, 0.08, 0.09);
const RAIL_COLOR: Color = Color::srgb(0.35, 0.33, 0.30);
const GROUND_COLOR: Color = Color::srgb(0.42, 0.36, 0.28);

// --- Tipi di carrozza -------------------------------------------------------

/// Colore dell'interno di una carrozza, in base alla sua funzione.
pub fn carriage_tint(kind: CarriageKind) -> Color {
    match kind {
        CarriageKind::Dormitorio => Color::srgb(0.36, 0.40, 0.58),
        CarriageKind::Mensa => Color::srgb(0.62, 0.44, 0.30),
        CarriageKind::Serra => Color::srgb(0.32, 0.54, 0.34),
        CarriageKind::Officina => Color::srgb(0.50, 0.47, 0.44),
    }
}

/// Etichetta della carrozza come `Carriage::label` della sim, ma con virgolette
/// ASCII: il font di default di Bevy non ha i caratteri « ».
pub fn carriage_display_label(carriage: &sim::Carriage) -> String {
    carriage.label().to_string().replace(['«', '»'], "\"")
}

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
        let solids = carriages
            .iter()
            .enumerate()
            .flat_map(|(i, &kind)| carriage_pieces(i, kind, count))
            .filter(|p| p.solid)
            .map(|p| p.rect)
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

/// Un rettangolo della carrozza: serve sia per disegnare sia per le collisioni.
struct Piece {
    rect: Rect,
    color: Color,
    z: f32,
    solid: bool,
}

impl Piece {
    fn new(x0: f32, y0: f32, x1: f32, y1: f32, color: Color, z: f32, solid: bool) -> Self {
        Self {
            rect: Rect::new(x0, y0, x1, y1),
            color,
            z,
            solid,
        }
    }
}

/// Tutti i pezzi della carrozza `index` (più il soffietto alla sua destra), in coordinate mondo.
fn carriage_pieces(index: usize, kind: CarriageKind, count: usize) -> Vec<Piece> {
    let x0 = TrainLayout::carriage_left(index);
    let x1 = x0 + CARRIAGE_LENGTH;
    let floor = FLOOR_Y;
    let ceil = FLOOR_Y + INTERIOR_HEIGHT;
    let is_first = index == 0;
    let is_last = index + 1 == count;

    let mut pieces = vec![
        // Sfondo interno
        Piece::new(
            x0,
            floor,
            x1,
            ceil,
            carriage_tint(kind),
            Z_BACKGROUND,
            false,
        ),
        // Pavimento e soffitto
        Piece::new(
            x0,
            floor - WALL,
            x1,
            floor,
            STRUCTURE_COLOR,
            Z_STRUCTURE,
            true,
        ),
        Piece::new(
            x0,
            ceil,
            x1,
            ceil + WALL,
            STRUCTURE_COLOR,
            Z_STRUCTURE,
            true,
        ),
    ];

    // Pareti di testata: piene alle estremità del treno, altrimenti solo
    // l'architrave sopra il vano porta.
    let left_bottom = if is_first { floor } else { floor + DOOR_HEIGHT };
    let right_bottom = if is_last { floor } else { floor + DOOR_HEIGHT };
    pieces.push(Piece::new(
        x0,
        left_bottom,
        x0 + WALL,
        ceil,
        STRUCTURE_COLOR,
        Z_STRUCTURE,
        true,
    ));
    pieces.push(Piece::new(
        x1 - WALL,
        right_bottom,
        x1,
        ceil,
        STRUCTURE_COLOR,
        Z_STRUCTURE,
        true,
    ));

    // Finestrini decorativi sulla parete di fondo
    const WINDOWS: usize = 4;
    const WINDOW_W: f32 = 36.0;
    let spacing = CARRIAGE_LENGTH / WINDOWS as f32;
    for w in 0..WINDOWS {
        let cx = x0 + spacing * (w as f32 + 0.5);
        pieces.push(Piece::new(
            cx - WINDOW_W / 2.0,
            floor + 52.0,
            cx + WINDOW_W / 2.0,
            floor + 80.0,
            WINDOW_COLOR,
            Z_DECOR,
            false,
        ));
    }

    // Carrelli sotto il pavimento
    for bx in [x0 + 40.0, x1 - 40.0] {
        pieces.push(Piece::new(
            bx - 24.0,
            floor - WALL - 12.0,
            bx + 24.0,
            floor - WALL,
            BOGIE_COLOR,
            Z_DECOR,
            false,
        ));
    }

    // Soffietto verso la carrozza successiva
    if !is_last {
        let g1 = x1 + GANGWAY;
        pieces.push(Piece::new(
            x1,
            floor,
            g1,
            floor + DOOR_HEIGHT,
            GANGWAY_COLOR,
            Z_BACKGROUND,
            false,
        ));
        pieces.push(Piece::new(
            x1,
            floor - WALL,
            g1,
            floor,
            STRUCTURE_COLOR,
            Z_STRUCTURE,
            true,
        ));
        pieces.push(Piece::new(
            x1,
            floor + DOOR_HEIGHT,
            g1,
            floor + DOOR_HEIGHT + WALL,
            STRUCTURE_COLOR,
            Z_STRUCTURE,
            true,
        ));
    }

    pieces
}

// --- Plugin e spawn ---------------------------------------------------------

/// Marca l'entità radice di una carrozza (i figli sono i suoi rettangoli).
#[derive(Component, Debug)]
pub struct Carriage;

pub struct TrainPlugin;

impl Plugin for TrainPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, (spawn_track, spawn_carriages));
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

/// Crea le entità grafiche di ogni carrozza, raggruppate sotto una radice.
fn spawn_carriages(mut commands: Commands, layout: Res<TrainLayout>, sim: Res<Sim>) {
    let count = layout.len();
    for (index, &kind) in layout.carriages.iter().enumerate() {
        let origin = Vec2::new(TrainLayout::carriage_left(index), FLOOR_Y);
        let title = match sim.world.carriages.get(index) {
            Some(c) => format!("{} \"{}\"", c.kind, c.name),
            None => kind.name().to_string(),
        };
        commands
            .spawn((
                Name::new(format!("Carrozza {} ({})", index + 1, title)),
                Carriage,
                Transform::from_translation(origin.extend(0.0)),
                Visibility::default(),
            ))
            .with_children(|parent| {
                for piece in carriage_pieces(index, kind, count) {
                    // I pezzi sono in coordinate mondo: li riporto locali alla radice.
                    let center = piece.rect.center() - origin;
                    parent.spawn((
                        Sprite::from_color(piece.color, piece.rect.size()),
                        Transform::from_translation(center.extend(piece.z)),
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
                    TextColor(Color::srgb(0.10, 0.10, 0.14)),
                    Transform::from_xyz(
                        CARRIAGE_LENGTH / 2.0,
                        INTERIOR_HEIGHT + WALL + 12.0,
                        Z_LABEL,
                    )
                    .with_scale(Vec3::splat(0.25)),
                ));
            });
    }
}

/// Binari e terreno sotto tutto il treno.
fn spawn_track(mut commands: Commands, layout: Res<TrainLayout>) {
    const MARGIN: f32 = 2000.0;
    let (left, right) = layout.inner_bounds();
    let width = right - left + 2.0 * MARGIN;
    let center_x = (left + right) / 2.0;
    let rail_top = FLOOR_Y - WALL - 12.0;

    commands.spawn((
        Name::new("Binario"),
        Sprite::from_color(RAIL_COLOR, Vec2::new(width, 4.0)),
        Transform::from_xyz(center_x, rail_top - 2.0, Z_DECOR),
    ));
    commands.spawn((
        Name::new("Terreno"),
        Sprite::from_color(GROUND_COLOR, Vec2::new(width, 200.0)),
        Transform::from_xyz(center_x, rail_top - 4.0 - 100.0, Z_BACKGROUND),
    ));
}
