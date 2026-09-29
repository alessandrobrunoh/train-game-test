//! Postazioni dentro le carrozze (solo grafica, arte in `prop_art.rs`).
//!
//! Nella sim una postazione non ha posizione: esiste solo come `(CarriageId,
//! StationId)`. Qui ogni postazione riceve in modo deterministico un posto sul
//! pavimento della sua carrozza. Le postazioni dello stesso tipo formano un
//! gruppo (nell'ordine della sim: es. in Mensa prima i tavoli, poi le cucine);
//! i gruppi si affiancano da sinistra a destra, lasciando libera la zona
//! magazzino vicino alla testata destra (vedi `storage.rs`) nelle carrozze
//! che hanno scorte. Se lo spazio non basta, le
//! cuccette si impilano a castello (finché c'è spazio sotto il soffitto) e poi
//! tutte le larghezze si riducono in proporzione, così niente si sovrappone
//! ed esce dalle pareti.
//!
//! In Mensa tra i tavoli e le cucine resta libera la zona della coda
//! ([`queue_zone`]): chi aspetta un posto (`Action::Wait`) si mette in fila lì,
//! rivolto alle cucine. A ogni tavolo si siede in due file, entrambe dietro
//! al tavolo (vedi [`seat_offset`]).

use bevy::prelude::*;
use sim::{CarriageId, Station, StationId, StationKind, World};

use crate::env_art::{ArtCache, ArtKey, art_sprite, hash2};
use crate::prop_art;
use crate::saves::WorldRebuildSet;
use crate::state::{Sim, WorldReplaced};
use crate::storage::{has_storage, storage_range};
use crate::train::{
    CARRIAGE_LENGTH, FLOOR_Y, INTERIOR_HEIGHT, STAIRS_LEFT, STAIRS_WIDTH, TrainLayout, WALL,
    floor_y,
};

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
/// Larghezza della zona della coda in Mensa, tra i tavoli e le cucine.
pub const QUEUE_ZONE: f32 = 34.0;
/// La fila lontana (dei commensali, della coda) sta più in alto di tanto.
pub const FAR_ROW_RISE: f32 = 3.0;

/// Altezza del materasso: chi dorme ci si sdraia sopra.
pub const BED_TOP: f32 = 5.0;

// Profondità: dietro agli NPC, tranne i tavoli che stanno davanti a chi mangia.
const Z_STATION: f32 = -3.0;
const Z_TABLE: f32 = 4.0;
/// Le scintille schizzano davanti a chi lavora.
const Z_SPARKS: f32 = 4.2;

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
    /// Piano della carrozza (0 = piano terra).
    pub floor: u8,
}

impl StationSpot {
    /// Quota della base della postazione.
    pub fn base_y(&self) -> f32 {
        floor_y(usize::from(self.floor)) + f32::from(self.level) * LEVEL_HEIGHT
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
        // Tre posti per fila (due file dietro al tavolo).
        StationKind::Table => Style {
            width: 22.0,
            height: 12.0,
            stackable: false,
        },
        // I cuochi lavorano spalla a spalla.
        StationKind::Stove => Style {
            width: 9.0,
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
        StationKind::Counter => Style {
            width: 30.0,
            height: 14.0,
            stackable: false,
        },
    }
}

/// Intervallo x (locale) percorribile e arredabile dentro una carrozza.
pub fn interior_range() -> (f32, f32) {
    (WALL + EDGE_MARGIN, CARRIAGE_LENGTH - WALL - EDGE_MARGIN)
}

/// Intervallo x (locale) dove stanno le postazioni: tutto l'interno, meno la
/// zona magazzino (e uno spazio) se la carrozza ha scorte.
pub fn station_range(with_storage: bool) -> (f32, f32) {
    let (left, right) = interior_range();
    if with_storage {
        (left, storage_range().0 - GROUP_GAP)
    } else {
        (left, right)
    }
}

/// Come [`layout_carriage`], ma ogni piano della carrozza è disposto per
/// conto suo con le postazioni che ci stanno.
pub fn layout_floors(stations: &[Station], range: (f32, f32)) -> Vec<StationSpot> {
    let top = stations.iter().map(|s| s.floor).max().unwrap_or(0);
    if top == 0 {
        return layout_carriage(stations, range);
    }
    let mut spots = layout_carriage(stations, range);
    for floor in 0..=top {
        let ids: Vec<usize> = (0..stations.len())
            .filter(|&i| stations[i].floor == floor)
            .collect();
        let here: Vec<Station> = ids.iter().map(|&i| stations[i].clone()).collect();
        for (spot, &i) in layout_carriage(&here, range).into_iter().zip(&ids) {
            spots[i] = StationSpot { floor, ..spot };
        }
    }
    spots
}

/// Posti delle postazioni di una carrozza dentro `range` (vedi
/// [`station_range`]); l'indice è lo `StationId`.
pub fn layout_carriage(stations: &[Station], range: (f32, f32)) -> Vec<StationSpot> {
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

    let (left, right) = range;
    let usable = right - left;
    // Tra i tavoli e le cucine (Mensa) resta la zona della coda.
    let gap_list: Vec<f32> = groups
        .windows(2)
        .map(|w| match (w[0].kind, w[1].kind) {
            (StationKind::Table, StationKind::Stove) => QUEUE_ZONE,
            _ => GROUP_GAP,
        })
        .collect();
    let gap_after = |k: usize| gap_list.get(k).copied().unwrap_or(0.0);
    let gaps: f32 = gap_list.iter().sum();
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
            floor: 0,
        };
        stations.len()
    ];
    for (k, g) in groups.iter().enumerate() {
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
                floor: 0,
            };
        }
        cursor += g.cols as f32 * slot + gap_after(k);
    }
    spots
}

/// Zona della coda di una Mensa (x locali `[sinistra, destra]`): tra
/// l'ultimo tavolo e la prima cucina. None se la carrozza non ha entrambi.
pub fn queue_zone(spots: &[StationSpot]) -> Option<(f32, f32)> {
    let edge = |kind: StationKind| spots.iter().filter(move |s| s.kind == kind);
    let tables = edge(StationKind::Table).map(|s| s.x + s.width / 2.0);
    let left = tables.fold(None, |m: Option<f32>, x| Some(m.map_or(x, |m| m.max(x))))?;
    let stoves = edge(StationKind::Stove).map(|s| s.x - s.width / 2.0);
    let right = stoves.fold(None, |m: Option<f32>, x| Some(m.map_or(x, |m| m.min(x))))?;
    (right > left).then_some((left, right))
}

/// Dove si siede il commensale `seat` (0..`capacity`) di un tavolo largo
/// `width`: scarto orizzontale dal centro, scarto verticale e se sta nella
/// fila lontana. Tutti siedono dietro al tavolo (che resta davanti a loro):
/// la prima metà dei posti nella fila vicina, la seconda in quella lontana,
/// più in alto di [`FAR_ROW_RISE`], disegnata dietro e sfalsata di mezzo
/// posto, così le teste non si coprono.
pub fn seat_offset(capacity: u16, seat: u16, width: f32) -> (f32, f32, bool) {
    let capacity = capacity.max(1);
    let seat = seat % capacity;
    let per_row = capacity.div_ceil(2);
    let (row, i) = (seat / per_row, seat % per_row);
    let step = width / f32::from(per_row);
    let x = -width / 2.0 + step * (f32::from(i) + 0.5);
    if row == 0 {
        (x, 0.0, false)
    } else {
        (x - step / 2.0, FAR_ROW_RISE, true)
    }
}

/// Posti di tutte le postazioni del treno, calcolati una volta all'avvio
/// (le postazioni della sim non cambiano durante la partita).
#[derive(Resource, Debug, Default)]
pub struct StationLayout {
    pub carriages: Vec<Vec<StationSpot>>,
    /// Per carrozza: zona della coda (Mensa, vedi [`queue_zone`]).
    queues: Vec<Option<(f32, f32)>>,
    /// Per carrozza con una coda: dove sta chi ozia o chiacchiera, lontano
    /// dai tavoli (davanti al magazzino, oltre le cucine).
    lounges: Vec<Option<(f32, f32)>>,
}

impl StationLayout {
    pub fn from_world(world: &World) -> Self {
        let carriages: Vec<Vec<StationSpot>> = world
            .carriages
            .iter()
            .map(|c| {
                let mut range = station_range(has_storage(&world.params, c.kind));
                if c.kind.def().floors > 1 {
                    // La scala per il piano di sopra occupa l'inizio della carrozza.
                    range.0 = range.0.max(STAIRS_LEFT + STAIRS_WIDTH + GROUP_GAP);
                }
                layout_floors(&c.stations, range)
            })
            .collect();
        let queues: Vec<_> = carriages.iter().map(|spots| queue_zone(spots)).collect();
        let lounges = world
            .carriages
            .iter()
            .zip(&queues)
            .map(|(c, queue)| {
                queue.map(|_| {
                    let (_, right) = interior_range();
                    (station_range(has_storage(&world.params, c.kind)).1, right)
                })
            })
            .collect();
        Self {
            carriages,
            queues,
            lounges,
        }
    }

    /// Zona della coda della carrozza (x locali), se è una Mensa.
    pub fn queue(&self, carriage: CarriageId) -> Option<(f32, f32)> {
        self.queues.get(carriage.index()).copied().flatten()
    }

    /// Dove oziare in una Mensa senza stare addosso ai tavoli (x locali).
    pub fn lounge(&self, carriage: CarriageId) -> Option<(f32, f32)> {
        self.lounges.get(carriage.index()).copied().flatten()
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
        app.init_resource::<ArtCache>()
            .init_resource::<StationAnim>()
            .add_systems(Startup, spawn_stations)
            .add_systems(
                PreUpdate,
                rebuild_stations
                    .in_set(WorldRebuildSet)
                    .run_if(on_message::<WorldReplaced>),
            )
            .add_systems(Update, animate_stations);
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

/// Radice delle postazioni di una carrozza (i figli sono gli sprite).
#[derive(Component, Debug)]
pub(crate) struct StationsRoot;

/// Vapore animato sopra una cucina.
#[derive(Component, Debug)]
struct Steam {
    phase: u8,
}

/// Scintille su un banco da lavoro: si vedono quando qualcuno ci lavora.
#[derive(Component, Debug)]
struct Sparks {
    carriage: usize,
    station: usize,
}

/// Fotogrammi delle animazioni delle postazioni.
#[derive(Resource, Default)]
pub(crate) struct StationAnim {
    steam: Vec<Handle<Image>>,
    sparks: Vec<Handle<Image>>,
}

/// Fotogrammi al secondo di vapore e scintille.
const STEAM_FPS: f32 = 3.0;
const SPARKS_FPS: f32 = 14.0;

/// Mondo sostituito: posti e grafica delle postazioni si rifanno da capo.
pub(crate) fn rebuild_stations(
    mut commands: Commands,
    sim: Res<Sim>,
    mut layout: ResMut<StationLayout>,
    old: Query<Entity, With<StationsRoot>>,
    mut art: ResMut<ArtCache>,
    mut anim: ResMut<StationAnim>,
    mut images: Option<ResMut<Assets<Image>>>,
) {
    for entity in &old {
        commands.entity(entity).despawn();
    }
    *layout = StationLayout::from_world(&sim.world);
    spawn_station_entities(
        &mut commands,
        &layout,
        &mut art,
        &mut anim,
        images.as_deref_mut(),
    );
}

/// Disegna le postazioni di ogni carrozza, raggruppate sotto una radice.
fn spawn_stations(
    mut commands: Commands,
    layout: Res<StationLayout>,
    mut art: ResMut<ArtCache>,
    mut anim: ResMut<StationAnim>,
    mut images: Option<ResMut<Assets<Image>>>,
) {
    spawn_station_entities(
        &mut commands,
        &layout,
        &mut art,
        &mut anim,
        images.as_deref_mut(),
    );
}

/// Larghezza in pixel d'arte di una postazione.
fn art_width(spot: &StationSpot) -> i32 {
    (spot.width.round() as i32).max(3)
}

fn spawn_station_entities(
    commands: &mut Commands,
    layout: &StationLayout,
    art: &mut ArtCache,
    anim: &mut StationAnim,
    mut images: Option<&mut Assets<Image>>,
) {
    anim.steam = (0..prop_art::STEAM_FRAMES)
        .map(|f| {
            art.get(images.as_deref_mut(), ArtKey::Steam(f), || {
                prop_art::steam(f)
            })
        })
        .collect();
    anim.sparks = (0..prop_art::SPARK_FRAMES)
        .map(|f| {
            art.get(images.as_deref_mut(), ArtKey::Sparks(f), || {
                prop_art::sparks(f)
            })
        })
        .collect();

    for (index, spots) in layout.carriages.iter().enumerate() {
        let origin = Vec2::new(TrainLayout::carriage_left(index), FLOOR_Y);
        let mut sprites: Vec<(Sprite, Transform, Option<StationFx>)> = Vec::new();
        let mut counters = 0u8;
        for (station, spot) in spots.iter().enumerate() {
            let w = art_width(spot);
            let wu = w as u16;
            let base = spot.base_y() - FLOOR_Y;
            // Sprite con il bordo inferiore alla quota `bottom` (locale).
            let mut place = |image: Handle<Image>, height: i32, bottom: f32, z: f32| {
                let size = Vec2::new(spot.width, height as f32);
                sprites.push((
                    art_sprite(image, size),
                    Transform::from_xyz(spot.x, bottom + size.y / 2.0, z),
                    None,
                ));
            };
            match spot.kind {
                StationKind::Bed => {
                    let upper = spot.level > 0;
                    let blanket = (hash2(index as i32, station as i32, 3)
                        % u32::from(prop_art::BLANKET_VARIANTS))
                        as u8;
                    let key = ArtKey::Bed {
                        width: wu,
                        upper,
                        blanket,
                    };
                    let image = art.get(images.as_deref_mut(), key, || {
                        prop_art::bed(w, upper, blanket)
                    });
                    let below = if upper { LEVEL_HEIGHT } else { 0.0 };
                    place(
                        image,
                        prop_art::BED_H + below as i32,
                        base - below,
                        Z_STATION,
                    );
                }
                StationKind::Table => {
                    let bench = art.get(images.as_deref_mut(), ArtKey::Bench(wu), || {
                        prop_art::bench(w)
                    });
                    place(bench, prop_art::BENCH_H, base, Z_STATION);
                    let image = art.get(images.as_deref_mut(), ArtKey::Table(wu), || {
                        prop_art::table(w)
                    });
                    place(image, prop_art::TABLE_H, base, Z_TABLE);
                }
                StationKind::Stove => {
                    let image = art.get(images.as_deref_mut(), ArtKey::Stove(wu), || {
                        prop_art::stove(w)
                    });
                    place(image, prop_art::STOVE_H, base, Z_STATION);
                    let size = Vec2::new(prop_art::STEAM_W as f32, prop_art::STEAM_H as f32);
                    let phase = (station % usize::from(prop_art::STEAM_FRAMES)) as u8;
                    sprites.push((
                        art_sprite(anim.steam.first().cloned().unwrap_or_default(), size),
                        Transform::from_xyz(
                            spot.x,
                            base + prop_art::POT_TOP + size.y / 2.0,
                            Z_STATION + 0.1,
                        ),
                        Some(StationFx::Steam(Steam { phase })),
                    ));
                }
                StationKind::GrowBed => {
                    let variant = (hash2(index as i32, station as i32, 8) % 7) as u8;
                    let key = ArtKey::GrowBed { width: wu, variant };
                    let image = art.get(images.as_deref_mut(), key, || {
                        prop_art::grow_bed(w, variant)
                    });
                    place(image, prop_art::GROW_H, base, Z_STATION);
                }
                StationKind::Workbench => {
                    let image = art.get(images.as_deref_mut(), ArtKey::Workbench(wu), || {
                        prop_art::workbench(w)
                    });
                    place(image, prop_art::WORKBENCH_H, base, Z_STATION);
                    let size = Vec2::new(prop_art::SPARKS_W as f32, prop_art::SPARKS_H as f32);
                    let x = spot.x - spot.width / 2.0 + 5.0;
                    sprites.push((
                        art_sprite(anim.sparks.first().cloned().unwrap_or_default(), size),
                        Transform::from_xyz(
                            x,
                            base + prop_art::VISE_TOP + size.y / 2.0 - 2.0,
                            Z_SPARKS,
                        ),
                        Some(StationFx::Sparks(Sparks {
                            carriage: index,
                            station,
                        })),
                    ));
                }
                StationKind::Counter => {
                    let hue = counters % prop_art::STALL_VARIANTS;
                    counters += 1;
                    let key = ArtKey::Counter { width: wu, hue };
                    let image = art.get(images.as_deref_mut(), key, || prop_art::counter(w, hue));
                    place(image, prop_art::COUNTER_H, base, Z_STATION);
                }
            }
        }
        // Numero di ogni colonna di cuccette, su una targhetta sopra la più alta.
        for (number, top) in bunk_columns(spots).into_iter().enumerate() {
            let number = number as u16 + 1;
            let plate = prop_art::bed_plate(number);
            let size = Vec2::new(plate.width as f32, plate.height as f32);
            if size.x + 2.0 > top.width {
                continue;
            }
            let image = art.get(images.as_deref_mut(), ArtKey::BedPlate(number), || plate);
            let y = top.base_y() - FLOOR_Y + prop_art::BED_H as f32 + 1.0 + size.y / 2.0;
            sprites.push((
                art_sprite(image, size),
                Transform::from_xyz(top.x, y, Z_STATION),
                None,
            ));
        }
        commands
            .spawn((
                Name::new(format!("Postazioni carrozza {}", index + 1)),
                StationsRoot,
                Transform::from_translation(origin.extend(0.0)),
                Visibility::default(),
            ))
            .with_children(|parent| {
                for (sprite, transform, fx) in sprites {
                    let mut entity = parent.spawn((sprite, transform));
                    match fx {
                        Some(StationFx::Steam(steam)) => {
                            entity.insert(steam);
                        }
                        Some(StationFx::Sparks(sparks)) => {
                            entity.insert((sparks, Visibility::Hidden));
                        }
                        None => {}
                    }
                }
            });
    }
}

/// Cuccetta più alta di ogni colonna di letti a castello, da sinistra a destra.
fn bunk_columns(spots: &[StationSpot]) -> Vec<StationSpot> {
    let mut tops: Vec<StationSpot> = Vec::new();
    for spot in spots.iter().filter(|s| s.kind == StationKind::Bed) {
        match tops
            .iter_mut()
            .find(|t| t.floor == spot.floor && (t.x - spot.x).abs() < 0.5)
        {
            Some(top) if spot.level > top.level => *top = *spot,
            Some(_) => {}
            None => tops.push(*spot),
        }
    }
    tops.sort_by(|a, b| a.floor.cmp(&b.floor).then(a.x.total_cmp(&b.x)));
    tops
}

/// Effetto animato attaccato a uno sprite di postazione.
enum StationFx {
    Steam(Steam),
    Sparks(Sparks),
}

/// Vapore sempre in movimento; scintille solo dove qualcuno lavora, a sprazzi.
fn animate_stations(
    time: Res<Time<Real>>,
    anim: Res<StationAnim>,
    sim: Res<Sim>,
    mut steam: Query<(&Steam, &mut Sprite), Without<Sparks>>,
    mut sparks: Query<(&Sparks, &mut Sprite, &mut Visibility)>,
    mut last: Local<(u64, u64)>,
) {
    let t = time.elapsed_secs_f64();
    let steam_tick = (t * f64::from(STEAM_FPS)) as u64;
    if steam_tick != last.0 && !anim.steam.is_empty() {
        last.0 = steam_tick;
        for (s, mut sprite) in &mut steam {
            let frame = (steam_tick + u64::from(s.phase)) as usize % anim.steam.len();
            sprite.image = anim.steam[frame].clone();
        }
    }
    let spark_tick = (t * f64::from(SPARKS_FPS)) as u64;
    if spark_tick != last.1 && !anim.sparks.is_empty() {
        last.1 = spark_tick;
        for (s, mut sprite, mut visibility) in &mut sparks {
            let busy = sim
                .world
                .carriages
                .get(s.carriage)
                .and_then(|c| c.stations.get(s.station))
                .is_some_and(|st| st.occupancy > 0);
            // A sprazzi: circa metà dei fotogrammi, diversi per ogni banco.
            let burst = hash2(s.carriage as i32, s.station as i32, (spark_tick / 3) as u32) % 5 < 3;
            let show = busy && burst;
            visibility.set_if_neq(if show {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            });
            if show {
                let frame = (spark_tick as usize + s.station) % anim.sparks.len();
                sprite.image = anim.sparks[frame].clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upper_floor_stations_are_laid_out_upstairs() {
        let world = World::generate(42, 20, 400);
        let layout = StationLayout::from_world(&world);
        let mut upstairs = 0;
        for (c, spots) in world.carriages.iter().zip(&layout.carriages) {
            for (station, spot) in c.stations.iter().zip(spots) {
                assert_eq!(spot.floor, station.floor, "{}", c.label());
                let base = floor_y(usize::from(station.floor));
                assert!(spot.base_y() >= base && spot.base_y() < base + INTERIOR_HEIGHT);
                upstairs += usize::from(station.floor > 0);
            }
        }
        assert!(upstairs > 0);
    }

    fn check(world: &World) {
        let layout = StationLayout::from_world(world);
        for (c, spots) in world.carriages.iter().zip(&layout.carriages) {
            let with_storage = has_storage(&world.params, c.kind);
            let (left, right) = station_range(with_storage);
            // Le postazioni non invadono il magazzino.
            if with_storage {
                assert!(right < storage_range().0);
            }
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
                let floor = floor_y(usize::from(spot.floor));
                assert!(top <= floor + INTERIOR_HEIGHT, "{spot:?}");
            }
            // Nessuna sovrapposizione tra postazioni dello stesso piano.
            for (i, a) in spots.iter().enumerate() {
                for b in &spots[i + 1..] {
                    if a.level == b.level && a.floor == b.floor {
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
    fn bunk_columns_are_numbered_left_to_right() {
        let world = World::generate(42, 20, 400);
        let layout = StationLayout::from_world(&world);
        let spots = &layout.carriages[0];
        let columns = bunk_columns(spots);
        // Piano per piano: colonne piene dal basso, numerate da sinistra.
        let mut expected = 0;
        for floor in 0..=spots.iter().map(|s| s.floor).max().unwrap() {
            let here: Vec<_> = spots.iter().filter(|s| s.floor == floor).collect();
            let levels = here.iter().map(|s| s.level).max().unwrap();
            expected += here.len().div_ceil(usize::from(levels) + 1);
            let xs: Vec<f32> = columns
                .iter()
                .filter(|c| c.floor == floor)
                .map(|c| c.x)
                .collect();
            assert!(xs.windows(2).all(|w| w[0] < w[1]));
        }
        assert_eq!(columns.len(), expected);
        assert!(columns.windows(2).all(|w| w[0].floor <= w[1].floor));
        assert!(columns.iter().all(|c| c.kind == StationKind::Bed));
    }

    #[test]
    fn mense_keep_a_queue_zone_between_tables_and_stoves() {
        let world = World::generate(42, 20, 400);
        let layout = StationLayout::from_world(&world);
        for c in &world.carriages {
            let queue = layout.queue(c.id);
            if c.kind != sim::CarriageKind::Mensa {
                assert_eq!(queue, None);
                assert_eq!(layout.lounge(c.id), None);
                continue;
            }
            let (left, right) = queue.unwrap();
            // La zona più i mezzi spazi tra le postazioni ai due lati.
            let width = right - left;
            assert!(
                (QUEUE_ZONE..=QUEUE_ZONE + SLOT_GAP).contains(&width),
                "{left}..{right}"
            );
            // Chi ozia sta oltre le cucine, lontano dai tavoli.
            let (lounge, _) = layout.lounge(c.id).unwrap();
            let spots = &layout.carriages[c.id.index()];
            assert!(spots.iter().all(|s| s.right() <= lounge + 1e-3));
            // Tavoli abbastanza larghi per tre commensali per fila.
            let tables = spots.iter().filter(|s| s.kind == StationKind::Table);
            assert!(tables.clone().all(|s| s.width >= 15.0), "{spots:?}");
        }
    }

    #[test]
    fn seats_fill_two_rows_behind_the_table() {
        let width = 18.0;
        let seats: Vec<_> = (0..6).map(|k| seat_offset(6, k, width)).collect();
        for (k, &(x, rise, far)) in seats.iter().enumerate() {
            assert_eq!(far, k >= 3);
            assert_eq!(rise, if far { FAR_ROW_RISE } else { 0.0 });
            assert!(x.abs() <= width / 2.0, "{k}: {x}");
        }
        // Le file sono sfalsate: nessuna testa esattamente sopra un'altra.
        for (k, a) in seats.iter().enumerate() {
            for b in &seats[..k] {
                assert!((a.0 - b.0).abs() > 1.0, "{a:?} {b:?}");
            }
        }
        assert_eq!(seat_offset(1, 0, width), (0.0, 0.0, false));
        assert_eq!(seat_offset(6, 6, width), seats[0]);
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
