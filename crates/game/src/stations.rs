//! Postazioni dentro le carrozze (solo grafica).
//!
//! Nella sim una postazione non ha posizione: esiste solo come `(CarriageId,
//! StationId)`. Qui ogni postazione riceve in modo deterministico un posto sul
//! pavimento della sua carrozza. Le postazioni dello stesso tipo formano un
//! gruppo (nell'ordine della sim: es. in Mensa prima i tavoli, poi le cucine);
//! i gruppi si affiancano da sinistra a destra. Se lo spazio non basta, le
//! cuccette si impilano a castello (finché c'è spazio sotto il soffitto) e poi
//! tutte le larghezze si riducono in proporzione, così niente si sovrappone
//! ed esce dalle pareti.

use bevy::prelude::*;
use sim::{CarriageId, Station, StationId, StationKind, World};

use crate::state::Sim;
use crate::train::{CARRIAGE_LENGTH, FLOOR_Y, INTERIOR_HEIGHT, TrainLayout, WALL};

/// Distanza verticale tra i piani di un letto a castello.
pub const LEVEL_HEIGHT: f32 = 20.0;
/// Spazio libero sopra il materasso più alto, per chi ci dorme.
const BUNK_HEADROOM: f32 = 12.0;
/// Spazio libero tra le pareti di testata e la prima/ultima postazione.
const EDGE_MARGIN: f32 = 4.0;
/// Spazio tra due gruppi di postazioni diverse.
const GROUP_GAP: f32 = 10.0;
/// Spazio tra due postazioni vicine dello stesso gruppo.
const SLOT_GAP: f32 = 2.0;

/// Altezza del materasso: chi dorme ci si sdraia sopra.
pub const BED_TOP: f32 = 5.0;

// Profondità: dietro agli NPC, tranne i tavoli che stanno davanti a chi mangia.
const Z_STATION: f32 = -3.0;
const Z_TABLE: f32 = 4.0;

const WOOD: Color = Color::srgb(0.45, 0.30, 0.18);
const TABLE_TOP: Color = Color::srgb(0.88, 0.78, 0.58);
const MATTRESS: Color = Color::srgb(0.80, 0.78, 0.70);
const PILLOW: Color = Color::srgb(0.95, 0.95, 0.92);
const METAL: Color = Color::srgb(0.25, 0.26, 0.28);
const BURNER: Color = Color::srgb(0.95, 0.45, 0.15);
const SOIL: Color = Color::srgb(0.30, 0.20, 0.12);
const PLANTS: Color = Color::srgb(0.18, 0.62, 0.22);
const BENCH: Color = Color::srgb(0.78, 0.60, 0.30);

/// Posto di una postazione, in coordinate locali alla carrozza (x = 0 è il
/// bordo sinistro della carrozza).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StationSpot {
    pub kind: StationKind,
    /// Centro orizzontale.
    pub x: f32,
    pub width: f32,
    /// Piano del letto a castello (0 = a terra).
    pub level: u8,
}

impl StationSpot {
    /// Quota della base della postazione.
    pub fn base_y(&self) -> f32 {
        FLOOR_Y + f32::from(self.level) * LEVEL_HEIGHT
    }

    #[cfg(test)]
    pub fn left(&self) -> f32 {
        self.x - self.width / 2.0
    }

    #[cfg(test)]
    pub fn right(&self) -> f32 {
        self.x + self.width / 2.0
    }
}

/// Ingombro preferito di un tipo di postazione.
struct Style {
    width: f32,
    height: f32,
    /// Si può impilare (letti a castello).
    stackable: bool,
}

impl Style {
    /// Piani massimi che stanno sotto il soffitto.
    fn max_levels(&self) -> usize {
        if self.stackable {
            ((INTERIOR_HEIGHT - self.height - BUNK_HEADROOM) / LEVEL_HEIGHT) as usize + 1
        } else {
            1
        }
    }
}

fn style(kind: StationKind) -> Style {
    match kind {
        StationKind::Bed => Style {
            width: 20.0,
            height: BED_TOP,
            stackable: true,
        },
        StationKind::Table => Style {
            width: 36.0,
            height: 12.0,
            stackable: false,
        },
        StationKind::Stove => Style {
            width: 14.0,
            height: 16.0,
            stackable: false,
        },
        StationKind::GrowBed => Style {
            width: 28.0,
            height: 12.0,
            stackable: false,
        },
        StationKind::Workbench => Style {
            width: 26.0,
            height: 14.0,
            stackable: false,
        },
    }
}

/// Intervallo x (locale) percorribile e arredabile dentro una carrozza.
pub fn interior_range() -> (f32, f32) {
    (WALL + EDGE_MARGIN, CARRIAGE_LENGTH - WALL - EDGE_MARGIN)
}

/// Posti delle postazioni di una carrozza; l'indice è lo `StationId`.
pub fn layout_carriage(stations: &[Station]) -> Vec<StationSpot> {
    struct Group {
        kind: StationKind,
        members: Vec<usize>,
        levels: usize,
        cols: usize,
        pref: f32,
    }

    // Gruppi per tipo, nell'ordine di prima comparsa.
    let mut groups: Vec<Group> = Vec::new();
    for (i, s) in stations.iter().enumerate() {
        match groups.iter_mut().find(|g| g.kind == s.kind) {
            Some(g) => g.members.push(i),
            None => groups.push(Group {
                kind: s.kind,
                members: vec![i],
                levels: 1,
                cols: 1,
                pref: style(s.kind).width,
            }),
        }
    }
    for g in &mut groups {
        g.cols = g.members.len();
    }

    let (left, right) = interior_range();
    let usable = right - left;
    let gaps = GROUP_GAP * groups.len().saturating_sub(1) as f32;
    let wanted = |groups: &[Group]| groups.iter().map(|g| g.cols as f32 * g.pref).sum::<f32>();

    // Impila i letti finché non ci stanno (o si arriva al massimo dei piani),
    // allargando sempre il gruppo più ingombrante.
    while wanted(&groups) + gaps > usable {
        let Some(g) = groups
            .iter_mut()
            .filter(|g| g.levels < style(g.kind).max_levels() && g.cols > 1)
            .max_by(|a, b| (a.cols as f32 * a.pref).total_cmp(&(b.cols as f32 * b.pref)))
        else {
            break;
        };
        g.levels += 1;
        g.cols = g.members.len().div_ceil(g.levels);
    }

    // Se ancora non basta, si stringe tutto in proporzione.
    let total = wanted(&groups);
    let scale = if total > 0.0 {
        ((usable - gaps).max(1.0) / total).min(1.0)
    } else {
        1.0
    };
    let block = total * scale + gaps;
    let mut cursor = left + (usable - block).max(0.0) / 2.0;

    let mut spots = vec![
        StationSpot {
            kind: StationKind::Bed,
            x: 0.0,
            width: 0.0,
            level: 0,
        };
        stations.len()
    ];
    for g in &groups {
        let slot = g.pref * scale;
        // Con slot minuscoli (treni affollatissimi) anche lo spazio si riduce.
        let gap = SLOT_GAP.min(slot * 0.25);
        for (k, &i) in g.members.iter().enumerate() {
            // Si riempie una colonna dal basso verso l'alto, poi la successiva.
            let col = k / g.levels;
            let level = k % g.levels;
            spots[i] = StationSpot {
                kind: g.kind,
                x: cursor + (col as f32 + 0.5) * slot,
                width: slot - gap,
                level: level as u8,
            };
        }
        cursor += g.cols as f32 * slot + GROUP_GAP;
    }
    spots
}

/// Posti di tutte le postazioni del treno, calcolati una volta all'avvio
/// (le postazioni della sim non cambiano durante la partita).
#[derive(Resource, Debug, Default)]
pub struct StationLayout {
    pub carriages: Vec<Vec<StationSpot>>,
}

impl StationLayout {
    pub fn from_world(world: &World) -> Self {
        Self {
            carriages: world
                .carriages
                .iter()
                .map(|c| layout_carriage(&c.stations))
                .collect(),
        }
    }

    pub fn spot(&self, carriage: CarriageId, station: StationId) -> Option<&StationSpot> {
        self.carriages
            .get(carriage.index())
            .and_then(|spots| spots.get(station.index()))
    }
}

pub struct StationsPlugin;

impl Plugin for StationsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_stations);
    }

    /// Come il layout del treno, nasce dalla sim dopo il `build` di tutti i plugin.
    fn finish(&self, app: &mut App) {
        let sim = app
            .world()
            .get_resource::<Sim>()
            .expect("StationsPlugin richiede la risorsa Sim (SimBridgePlugin)");
        let layout = StationLayout::from_world(&sim.world);
        app.insert_resource(layout);
    }
}

/// Un rettangolo di una postazione: angolo in basso a sinistra relativo alla
/// base della postazione, dimensioni, colore.
fn station_rects(spot: &StationSpot) -> Vec<(Vec2, Vec2, Color)> {
    let w = spot.width;
    let rect = |x: f32, y: f32, width: f32, height: f32, color: Color| {
        (Vec2::new(x, y), Vec2::new(width, height), color)
    };
    match spot.kind {
        StationKind::Bed => vec![
            rect(-w / 2.0, 0.0, w, 3.0, WOOD),
            rect(-w / 2.0 + 1.0, 3.0, w - 2.0, BED_TOP - 3.0, MATTRESS),
            rect(-w / 2.0 + 1.0, BED_TOP, 3.0_f32.min(w / 4.0), 1.5, PILLOW),
        ],
        StationKind::Table => vec![
            rect(-1.5, 0.0, 3.0, 9.0, WOOD),
            rect(-w / 2.0, 9.0, w, 3.0, TABLE_TOP),
        ],
        StationKind::Stove => vec![
            rect(-w / 2.0, 0.0, w, 14.0, METAL),
            rect(-w / 2.0 + 2.0, 14.0, (w - 4.0).max(1.0), 2.0, BURNER),
        ],
        StationKind::GrowBed => vec![
            rect(-w / 2.0, 0.0, w, 6.0, SOIL),
            rect(-w / 2.0 + 2.0, 6.0, (w - 4.0).max(1.0), 6.0, PLANTS),
        ],
        StationKind::Workbench => vec![
            rect(-w / 2.0 + 1.0, 0.0, 2.0, 11.0, METAL),
            rect(w / 2.0 - 3.0, 0.0, 2.0, 11.0, METAL),
            rect(-w / 2.0, 11.0, w, 3.0, BENCH),
        ],
    }
}

/// Disegna le postazioni di ogni carrozza, raggruppate sotto una radice.
fn spawn_stations(mut commands: Commands, layout: Res<StationLayout>) {
    for (index, spots) in layout.carriages.iter().enumerate() {
        let origin = Vec2::new(TrainLayout::carriage_left(index), FLOOR_Y);
        commands
            .spawn((
                Name::new(format!("Postazioni carrozza {}", index + 1)),
                Transform::from_translation(origin.extend(0.0)),
                Visibility::default(),
            ))
            .with_children(|parent| {
                for spot in spots {
                    let z = if spot.kind == StationKind::Table {
                        Z_TABLE
                    } else {
                        Z_STATION
                    };
                    for (corner, size, color) in station_rects(spot) {
                        let center =
                            Vec2::new(spot.x, spot.base_y() - FLOOR_Y) + corner + size / 2.0;
                        parent.spawn((
                            Sprite::from_color(color, size),
                            Transform::from_translation(center.extend(z)),
                        ));
                    }
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(world: &World) {
        let layout = StationLayout::from_world(world);
        let (left, right) = interior_range();
        for (c, spots) in world.carriages.iter().zip(&layout.carriages) {
            assert_eq!(spots.len(), c.stations.len());
            for (spot, station) in spots.iter().zip(&c.stations) {
                assert_eq!(spot.kind, station.kind);
                assert!(spot.width > 0.0);
                assert!(spot.left() >= left - 1e-3, "{c:?} {spot:?}");
                assert!(spot.right() <= right + 1e-3, "{c:?} {spot:?}");
                let mut top = spot.base_y() + style(spot.kind).height;
                if spot.kind == StationKind::Bed {
                    top += BUNK_HEADROOM;
                }
                assert!(top <= FLOOR_Y + INTERIOR_HEIGHT, "{spot:?}");
            }
            // Nessuna sovrapposizione tra postazioni dello stesso piano.
            for (i, a) in spots.iter().enumerate() {
                for b in &spots[i + 1..] {
                    if a.level == b.level {
                        let apart = a.right() <= b.left() + 1e-3 || b.right() <= a.left() + 1e-3;
                        assert!(apart, "{a:?} si sovrappone a {b:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn stations_fit_the_default_train() {
        check(&World::generate(42, 20, 400));
    }

    #[test]
    fn stations_fit_small_and_crowded_trains() {
        check(&World::generate(1, 1, 5));
        check(&World::generate(7, 4, 30));
        check(&World::generate(3, 4, 2000));
    }

    #[test]
    fn crowded_dorms_use_bunk_beds() {
        let world = World::generate(42, 20, 400);
        let layout = StationLayout::from_world(&world);
        let beds = layout.carriages[0]
            .iter()
            .filter(|s| s.kind == StationKind::Bed);
        assert!(beds.clone().any(|s| s.level > 0));
        assert!(beds.clone().all(|s| s.width >= 12.0));
    }
}
