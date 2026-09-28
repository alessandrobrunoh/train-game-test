//! Disegno degli NPC della simulazione e selezione con il mouse.
//!
//! Si disegnano solo gli NPC della "finestra visibile": le carrozze inquadrate
//! dalla camera più una per lato, più chi sta viaggiando attraverso di esse.
//! Ogni NPC visibile ha un'entità `NpcSprite`; esce dalla finestra o muore e
//! l'entità viene rimossa. La posizione deriva dall'azione corrente:
//! - mangia/dorme/lavora: alla sua postazione (vedi `stations.rs`), con un
//!   posto diverso per ognuno di chi la condivide;
//! - socializza: accanto al compagno;
//! - compra: al bancone del Mercato, davanti a un mercante al lavoro;
//! - ozia: in un punto deterministico della carrozza, diverso a ogni pausa;
//! - viaggia: interpolato tra il centro della carrozza di partenza e quello
//!   di arrivo in base all'avanzamento dell'azione.
//!
//! Gli sprite camminano verso la posizione obiettivo e si teletrasportano se
//! è troppo lontana (es. a velocità di gioco alte).
//!
//! Indicatori economici: chi lavora con un attrezzo lo tiene in mano (un
//! rettangolino accanto al corpo); chi non ha vestiti è disegnato più pallido.
//!
//! Età e sesso: la taglia cresce con l'età (neonati minuscoli, a 18 anni si è
//! adulti, gli anziani si accorciano un po'); i capelli sono una calotta sulla
//! testa, corta per gli uomini e con due ciocche ai lati per le donne, grigia
//! per gli anziani. Il colore del corpo resta quello del lavoro. I neonati
//! (sotto i 3 anni) che oziano stanno accanto alla mamma, se è nella stessa
//! carrozza. Chi muore sotto gli occhi del giocatore svanisce (vedi `life_fx.rs`).

use std::collections::HashMap;

use bevy::{prelude::*, window::PrimaryWindow};
use sim::{Action, CarriageId, Job, LifeStage, Npc, NpcId, Sex, StationKind, World};

use crate::camera::follow_target;
use crate::life_fx::FadingOut;
use crate::state::{NpcSprite, PointerOverUi, SelectedNpc, Sim};
use crate::stations::{BED_TOP, StationLayout, interior_range};
use crate::train::{CARRIAGE_PITCH, FLOOR_Y, TrainLayout};

/// Dimensioni di un NPC adulto (larghezza, altezza).
const ADULT_SIZE: Vec2 = Vec2::new(8.0, 16.0);
/// Taglia per età (anni, larghezza, altezza), interpolata tra un punto e
/// l'altro: neonati minuscoli, bambini piccoli, giovani quasi adulti, anziani
/// un po' più bassi.
const SIZE_BY_AGE: [(f32, f32, f32); 7] = [
    (0.0, 4.0, 5.0),
    (3.0, 5.0, 8.0),
    (10.0, 6.0, 11.0),
    (14.0, 7.0, 13.5),
    (18.0, ADULT_SIZE.x, ADULT_SIZE.y),
    (65.0, ADULT_SIZE.x, ADULT_SIZE.y),
    (85.0, ADULT_SIZE.x, 14.5),
];
/// Sotto quest'età chi ozia sta accanto alla mamma.
const BABY_AGE: u32 = 3;
/// Distanza tra il neonato e la mamma.
const BABY_OFFSET: f32 = 6.0;

/// Capelli, davanti al corpo: calotta corta per gli uomini; per le donne un
/// blocco più largo del corpo e più lungo, che sporge sopra la testa e ai lati.
const HAIR_CAP: f32 = 2.0;
const LONG_HAIR: f32 = 6.0;
const LONG_HAIR_ABOVE: f32 = 1.0;
const LONG_HAIR_SIDE: f32 = 1.0;
/// Niente capelli neri: si confonderebbero con il bordo scuro.
const HAIR_COLORS: [Color; 5] = [
    Color::srgb(0.30, 0.19, 0.11),
    Color::srgb(0.42, 0.26, 0.13),
    Color::srgb(0.58, 0.36, 0.17),
    Color::srgb(0.88, 0.74, 0.40),
    Color::srgb(0.76, 0.34, 0.14),
];
const GREY_HAIR: Color = Color::srgb(0.86, 0.86, 0.84);
/// Chi è sdraiato è disegnato più sottile, per leggersi come "a letto".
const LYING_THICKNESS: f32 = 0.75;
/// Entro questa distanza dal letto l'NPC è "arrivato" e si sdraia.
const ARRIVE_DISTANCE: f32 = 0.5;
/// Durata della rotazione quando si sdraia o si alza (secondi).
const LIE_DOWN_TIME: f32 = 0.15;
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
const MERCANTE_COLOR: Color = Color::srgb(0.95, 0.75, 0.25);
const JOBLESS_COLOR: Color = Color::srgb(0.75, 0.45, 0.85);
const CHILD_COLOR: Color = Color::srgb(1.00, 0.60, 0.75);
const ELDER_COLOR: Color = Color::srgb(0.72, 0.72, 0.70);
/// Chi dorme è disegnato più scuro.
const SLEEP_DARKEN: f32 = 0.6;
/// Chi non ha vestiti è disegnato più pallido: quanto il colore va verso il
/// suo grigio e quanto si schiarisce.
const NO_CLOTHES_DESATURATE: f32 = 0.55;
const NO_CLOTHES_LIGHTEN: f32 = 0.12;

/// Attrezzo in mano: dimensioni, colore e inclinazione.
const TOOL_SIZE: Vec2 = Vec2::new(1.5, 7.0);
const TOOL_COLOR: Color = Color::srgb(0.80, 0.82, 0.86);
const TOOL_TILT: f32 = -0.5;
/// Distanza tra il bordo del bancone e il cliente.
const COUNTER_CLEARANCE: f32 = 1.0;
/// Scarto massimo tra clienti dello stesso bancone, per non sovrapporsi del tutto.
const CUSTOMER_SPREAD: f32 = 6.0;

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
                    .in_set(NpcRenderSet)
                    .after(follow_target),
            );
    }
}

/// I sistemi che creano e muovono gli sprite degli NPC (per ordinarsi dopo).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct NpcRenderSet;

/// Bordo scuro dietro lo sprite di un NPC (figlio dell'entità `NpcSprite`).
#[derive(Component)]
struct NpcOutline;

/// Attrezzo in mano (figlio dell'entità `NpcSprite`), visibile mentre lavora.
#[derive(Component)]
struct NpcTool;

/// Capelli (figlio dell'entità `NpcSprite`).
#[derive(Component)]
struct NpcHair;

/// Filtro delle query sugli attrezzi, disgiunto dalla camera.
type ToolOnly = (With<NpcTool>, Without<Camera2d>);

/// Filtro delle query sui capelli, disgiunto da corpi e bordi.
type HairOnly = (With<NpcHair>, Without<NpcSprite>, Without<NpcOutline>);

/// Stato grafico di uno sprite NPC.
#[derive(Component)]
pub(crate) struct NpcVisual {
    outline: Entity,
    tool: Entity,
    hair: Entity,
    /// Capelli lunghi (donne).
    long_hair: bool,
    /// L'attrezzo in mano è visibile.
    tool_shown: bool,
    /// Dove lo sprite vuole andare (coordinate mondo).
    target: Vec2,
    /// Corpo in piedi (larghezza, altezza).
    standing: Vec2,
    /// Corpo sdraiato, prima della rotazione di 90°: (spessore, lunghezza).
    /// Resta quello dell'ultimo letto anche dopo essersi alzato, per animare
    /// la transizione.
    lying: Vec2,
    /// La posa chiede di stare sdraiato (dorme in un letto).
    wants_lying: bool,
    /// Avanzamento della transizione: 0 = in piedi, 1 = sdraiato.
    lie: f32,
    /// Semiampiezze del rettangolo in coordinate mondo (per il click).
    half_extents: Vec2,
}

impl NpcVisual {
    /// Metà altezza dell'ingombro attuale (per mettere qualcosa sopra la testa).
    pub(crate) fn half_height(&self) -> f32 {
        self.half_extents.y
    }

    /// Dimensione dello sprite (prima della rotazione) con la transizione attuale.
    fn body(&self) -> Vec2 {
        self.standing.lerp(self.lying, self.lie)
    }

    fn rotation(&self) -> Quat {
        Quat::from_rotation_z(std::f32::consts::FRAC_PI_2 * self.lie)
    }

    /// Semiampiezze del rettangolo ruotato (bounding box allineata agli assi).
    fn rotated_half_extents(&self) -> Vec2 {
        let half = self.body() / 2.0;
        let (sin, cos) = (std::f32::consts::FRAC_PI_2 * self.lie).sin_cos();
        Vec2::new(
            cos.abs() * half.x + sin.abs() * half.y,
            sin.abs() * half.x + cos.abs() * half.y,
        )
    }
}

/// Marcatore sopra la testa dell'NPC selezionato.
#[derive(Component)]
struct SelectionMarker;

/// NPC attualmente disegnati: id -> (entità, ultimo frame in cui era visibile).
#[derive(Resource, Default)]
pub(crate) struct NpcSpriteIndex {
    entities: HashMap<NpcId, (Entity, u32)>,
    frame: u32,
}

impl NpcSpriteIndex {
    /// Entità dello sprite dell'NPC, se è disegnato.
    pub(crate) fn entity(&self, id: NpcId) -> Option<Entity> {
        self.entities.get(&id).map(|&(e, _)| e)
    }
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

/// Taglia del corpo (larghezza, altezza) a una certa età, vedi [`SIZE_BY_AGE`].
fn size_for_age(age: u32) -> Vec2 {
    let age = age as f32;
    let (first, last) = (SIZE_BY_AGE[0], SIZE_BY_AGE[SIZE_BY_AGE.len() - 1]);
    if age <= first.0 {
        return Vec2::new(first.1, first.2);
    }
    for pair in SIZE_BY_AGE.windows(2) {
        let ((a0, w0, h0), (a1, w1, h1)) = (pair[0], pair[1]);
        if age <= a1 {
            let t = (age - a0) / (a1 - a0);
            return Vec2::new(w0 + (w1 - w0) * t, h0 + (h1 - h0) * t);
        }
    }
    Vec2::new(last.1, last.2)
}

fn body_size(npc: &Npc) -> Vec2 {
    size_for_age(npc.age)
}

/// Capelli per un corpo di dimensioni `body` (prima della rotazione):
/// dimensioni e scarto verticale del centro rispetto al centro del corpo.
fn hair_rect(body: Vec2, long: bool) -> (Vec2, f32) {
    let top = body.y / 2.0;
    if long {
        let length = LONG_HAIR.min(body.y * 0.7);
        let size = Vec2::new(body.x + 2.0 * LONG_HAIR_SIDE, length);
        (size, top + LONG_HAIR_ABOVE - length / 2.0)
    } else {
        let cap = HAIR_CAP.min(body.y * 0.3);
        (Vec2::new(body.x, cap), top - cap / 2.0)
    }
}

/// Colore dei capelli: grigi da anziani, altrimenti uno a caso (fisso per NPC).
fn hair_color(npc: &Npc) -> Color {
    if npc.stage() == LifeStage::Anziano {
        return GREY_HAIR;
    }
    let pick = hash01(u64::from(npc.id.0) ^ 0x4841_4952);
    HAIR_COLORS[((pick * HAIR_COLORS.len() as f32) as usize).min(HAIR_COLORS.len() - 1)]
}

/// Transform dei capelli (figli dello sprite) per un corpo di dimensioni `body`.
fn hair_transform(body: Vec2, long: bool) -> Transform {
    let (_, offset) = hair_rect(body, long);
    Transform::from_xyz(0.0, offset, Z_STEP / 4.0)
}

/// La mamma dell'NPC, se è viva.
fn mother<'w>(world: &'w World, npc: &Npc) -> Option<&'w Npc> {
    npc.parents()
        .filter_map(|id| world.npc(id))
        .find(|p| p.sex == Sex::Female)
}

/// Dove sta un neonato che ozia: accanto alla mamma, se è nella stessa
/// carrozza e non sta viaggiando.
fn baby_x(world: &World, stations: &StationLayout, npc: &Npc) -> Option<f32> {
    if npc.age >= BABY_AGE {
        return None;
    }
    let mother = mother(world, npc)?;
    if mother.carriage != npc.carriage || matches!(mother.action, Action::Travel { .. }) {
        return None;
    }
    let side = if hash01(u64::from(npc.id.0)) < 0.5 {
        -1.0
    } else {
        1.0
    };
    let x = anchor_x(world, stations, mother) + side * BABY_OFFSET;
    let (left, right) = interior_range();
    Some(x.clamp(
        carriage_x(npc.carriage, left),
        carriage_x(npc.carriage, right),
    ))
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
        Action::Buy(_) => counter_x(world, stations, npc).unwrap_or_else(|| idle_x(npc)),
        Action::Idle => idle_x(npc),
    }
}

/// Dove sta chi compra: accanto a un bancone del Mercato, di preferenza uno
/// con un mercante al lavoro (scelto in modo deterministico per NPC).
fn counter_x(world: &World, stations: &StationLayout, npc: &Npc) -> Option<f32> {
    let c = npc.carriage;
    let carriage = world.carriage(c)?;
    let counters = || {
        carriage
            .stations
            .iter()
            .filter(|s| s.kind == StationKind::Counter)
    };
    // Solo i mercanti lavorano ai banconi: occupato = c'è un mercante.
    let staffed = counters().filter(|s| s.occupancy > 0).count();
    let pick = hash01(u64::from(npc.id.0) ^ 0xC0FFEE);
    let station = if staffed > 0 {
        let n = ((pick * staffed as f32) as usize).min(staffed - 1);
        counters().filter(|s| s.occupancy > 0).nth(n)?
    } else {
        let total = counters().count();
        let n = ((pick * total as f32) as usize).min(total.checked_sub(1)?);
        counters().nth(n)?
    };
    let spot = stations.spot(c, station.id)?;
    // Il cliente sta sul lato destro del bancone; il mercante al centro.
    let size = body_size(npc);
    let jitter = hash01(u64::from(npc.id.0) ^ npc.action_since.minutes()) * CUSTOMER_SPREAD;
    let x = spot.x + spot.width / 2.0 + COUNTER_CLEARANCE + size.x / 2.0 + jitter;
    let (_, right) = interior_range();
    Some(carriage_x(c, x.min(right - size.x / 2.0)))
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
            Some(spot) => {
                let length_scale = ((spot.width - 2.0) / size.y).clamp(0.3, 1.0);
                // La testa sul cuscino (a sinistra): i piccoli non stanno al centro.
                let head_end = spot.x - spot.width / 2.0 + 1.0;
                Pose {
                    position: Vec2::new(
                        carriage_x(c, head_end + size.y * length_scale / 2.0),
                        spot.base_y() + BED_TOP + size.x * LYING_THICKNESS / 2.0,
                    ),
                    lying: true,
                    length_scale,
                }
            }
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
        Action::Buy(_) => standing(anchor_x(world, stations, npc)),
        Action::Idle => standing(baby_x(world, stations, npc).unwrap_or_else(|| idle_x(npc))),
    }
}

/// Dove va disegnato l'NPC (centro del corpo), anche se non ha uno sprite.
pub(crate) fn npc_position(world: &World, stations: &StationLayout, npc: &Npc) -> Vec2 {
    npc_pose(world, stations, npc, 0).position
}

/// Vero se l'NPC va disegnato con l'attrezzo in mano.
fn holds_tool(npc: &Npc) -> bool {
    matches!(npc.action, Action::Work(_)) && npc.inventory.tool.is_some()
}

fn npc_color(npc: &Npc) -> Color {
    let base = match npc.job {
        Some(Job::Contadino) => CONTADINO_COLOR,
        Some(Job::Cuoco) => CUOCO_COLOR,
        Some(Job::Operaio) => OPERAIO_COLOR,
        Some(Job::Mercante) => MERCANTE_COLOR,
        None => match npc.stage() {
            LifeStage::Bambino | LifeStage::Giovane => CHILD_COLOR,
            LifeStage::Anziano => ELDER_COLOR,
            LifeStage::Adulto => JOBLESS_COLOR,
        },
    };
    let mut c = base.to_srgba();
    if npc.inventory.clothes.is_none() {
        // Senza vestiti: più pallido (desaturato e un po' più chiaro).
        let gray = 0.3 * c.red + 0.59 * c.green + 0.11 * c.blue;
        let pale = |v: f32| (v + (gray - v) * NO_CLOTHES_DESATURATE + NO_CLOTHES_LIGHTEN).min(1.0);
        c = Srgba::rgb(pale(c.red), pale(c.green), pale(c.blue));
    }
    if !npc.is_awake() {
        c = Srgba::rgb(
            c.red * SLEEP_DARKEN,
            c.green * SLEEP_DARKEN,
            c.blue * SLEEP_DARKEN,
        );
    }
    c.into()
}

/// Carrozze della finestra visibile: quelle inquadrate più una per lato.
pub(crate) fn visible_window(camera_x: f32, view: Rect, carriages: usize) -> (usize, usize) {
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
    mut tools: Query<(&mut Visibility, &mut Transform), ToolOnly>,
    mut hairs: Query<&mut Sprite, HairOnly>,
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
        let lying = Vec2::new(size.x * LYING_THICKNESS, size.y * pose.length_scale);
        let tool_shown = holds_tool(npc);
        let hair = hair_color(npc);
        let long_hair = npc.sex == Sex::Female;

        let existing = index.entities.get(&npc.id).map(|&(e, _)| e);
        if let Some(entity) = existing
            && let Ok((mut sprite, mut visual)) = sprites.get_mut(entity)
        {
            // Dimensioni e rotazione le aggiorna `move_npc_sprites`, in base
            // a dove si trova davvero lo sprite.
            visual.target = pose.position;
            if visual.standing != size {
                // È cresciuto: l'attrezzo resta accanto al fianco.
                visual.standing = size;
                if let Ok((_, mut transform)) = tools.get_mut(visual.tool) {
                    transform.translation.x = tool_x(size);
                }
            }
            visual.wants_lying = pose.lying;
            if pose.lying {
                visual.lying = lying;
            }
            if sprite.color != color {
                sprite.color = color;
            }
            if let Ok(mut outline) = outlines.get_mut(visual.outline)
                && outline.color != outline_color
            {
                outline.color = outline_color;
            }
            if visual.tool_shown != tool_shown {
                visual.tool_shown = tool_shown;
                if let Ok((mut visibility, _)) = tools.get_mut(visual.tool) {
                    *visibility = tool_visibility(tool_shown);
                }
            }
            if let Ok(mut sprite) = hairs.get_mut(visual.hair)
                && sprite.color != hair
            {
                sprite.color = hair;
            }
            index.entities.insert(npc.id, (entity, frame));
            continue;
        }

        // Nasce già nella posa finale (anche sdraiato, se è già a letto).
        let mut visual = NpcVisual {
            outline: Entity::PLACEHOLDER,
            tool: Entity::PLACEHOLDER,
            hair: Entity::PLACEHOLDER,
            long_hair,
            tool_shown,
            target: pose.position,
            standing: size,
            lying,
            wants_lying: pose.lying,
            lie: if pose.lying { 1.0 } else { 0.0 },
            half_extents: Vec2::ZERO,
        };
        visual.half_extents = visual.rotated_half_extents();
        let body = visual.body();
        let framed = body + Vec2::splat(2.0 * OUTLINE);

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
        visual.outline = outline;
        // Attrezzo tenuto accanto al fianco destro, inclinato.
        let tool = commands
            .spawn((
                NpcTool,
                Sprite::from_color(TOOL_COLOR, TOOL_SIZE),
                Transform::from_xyz(tool_x(size), -1.0, Z_STEP / 2.0)
                    .with_rotation(Quat::from_rotation_z(TOOL_TILT)),
                tool_visibility(tool_shown),
            ))
            .id();
        visual.tool = tool;
        let hair_entity = commands
            .spawn((
                NpcHair,
                Sprite::from_color(hair, hair_rect(body, long_hair).0),
                hair_transform(body, long_hair),
            ))
            .id();
        visual.hair = hair_entity;
        // Ogni NPC ha una profondità propria: niente sfarfallio tra sprite sovrapposti.
        let z = NPC_Z + (npc.id.0 % 1000) as f32 * Z_STEP;
        let rotation = visual.rotation();
        let entity = commands
            .spawn((
                Name::new(npc.name.clone()),
                NpcSprite(npc.id),
                visual,
                Sprite::from_color(color, body),
                Transform::from_translation(pose.position.extend(z)).with_rotation(rotation),
            ))
            .add_children(&[outline, tool, hair_entity])
            .id();
        index.entities.insert(npc.id, (entity, frame));
    }

    // Via chi è uscito dalla finestra; chi è morto svanisce piano.
    index.entities.retain(|&id, &mut (entity, seen)| {
        if seen != frame {
            if world.npc(id).is_none() {
                commands
                    .entity(entity)
                    .remove::<(NpcSprite, NpcVisual)>()
                    .insert(FadingOut::default());
            } else {
                commands.entity(entity).despawn();
            }
        }
        seen == frame
    });
}

/// Posizione x dell'attrezzo in mano, accanto al fianco destro.
fn tool_x(body: Vec2) -> f32 {
    body.x / 2.0 + TOOL_SIZE.x / 2.0
}

fn tool_visibility(shown: bool) -> Visibility {
    if shown {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    }
}

/// Muove gli sprite verso la loro posizione obiettivo e li fa sdraiare solo
/// una volta arrivati al letto (mentre ci vanno camminano in piedi).
fn move_npc_sprites(
    time: Res<Time>,
    mut sprites: Query<(&mut Transform, &mut Sprite, &mut NpcVisual), With<NpcSprite>>,
    mut outlines: Query<&mut Sprite, (With<NpcOutline>, Without<NpcSprite>)>,
    mut hairs: Query<(&mut Sprite, &mut Transform), HairOnly>,
) {
    let dt = time.delta_secs();
    for (mut transform, mut sprite, mut visual) in &mut sprites {
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

        let arrived = next.distance(visual.target) <= ARRIVE_DISTANCE;
        let goal = if visual.wants_lying && arrived {
            1.0
        } else {
            0.0
        };
        let lie = if distance > TELEPORT_DISTANCE {
            // Teletrasportato: niente animazione.
            goal
        } else {
            visual.lie + (goal - visual.lie).clamp(-dt / LIE_DOWN_TIME, dt / LIE_DOWN_TIME)
        };
        if lie != visual.lie {
            visual.lie = lie;
        }
        let half_extents = visual.rotated_half_extents();
        if visual.half_extents != half_extents {
            visual.half_extents = half_extents;
        }

        let rotation = visual.rotation();
        if transform.rotation != rotation {
            transform.rotation = rotation;
        }
        let body = visual.body();
        if sprite.custom_size != Some(body) {
            sprite.custom_size = Some(body);
            if let Ok((mut hair, mut hair_transform)) = hairs.get_mut(visual.hair) {
                let (size, offset) = hair_rect(body, visual.long_hair);
                hair.custom_size = Some(size);
                hair_transform.translation.y = offset;
            }
        }
        let framed = body + Vec2::splat(2.0 * OUTLINE);
        if let Ok(mut outline) = outlines.get_mut(visual.outline)
            && outline.custom_size != Some(framed)
        {
            outline.custom_size = Some(framed);
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
    fn size_grows_with_age_and_shrinks_a_bit_when_old() {
        let baby = size_for_age(0);
        let child = size_for_age(8);
        let youth = size_for_age(16);
        let adult = size_for_age(30);
        let old = size_for_age(90);
        assert_eq!(adult, ADULT_SIZE);
        assert_eq!(size_for_age(18), ADULT_SIZE);
        assert_eq!(size_for_age(65), ADULT_SIZE);
        assert!(baby.y < child.y && child.y < youth.y && youth.y < adult.y);
        assert!(baby.x < child.x && child.x < adult.x);
        assert!(baby.y <= adult.y / 3.0, "i neonati sono minuscoli: {baby}");
        assert!(
            youth.y >= adult.y * 0.85,
            "i giovani sono quasi adulti: {youth}"
        );
        assert!(old.y < adult.y && old.y > adult.y * 0.8, "{old}");
        assert_eq!(old.x, adult.x);
        // Continua anno per anno.
        for age in 0..100 {
            let (a, b) = (size_for_age(age), size_for_age(age + 1));
            assert!((a - b).abs().max_element() <= 1.5, "{age}: {a} -> {b}");
        }
    }

    #[test]
    fn long_hair_is_wider_and_frames_the_head() {
        for age in [0, 5, 16, 40, 80] {
            let body = size_for_age(age);
            let (short, short_y) = hair_rect(body, false);
            let (long, long_y) = hair_rect(body, true);
            // Corti: dentro la testa, in cima.
            assert_eq!(short.x, body.x);
            assert!((short_y + short.y / 2.0 - body.y / 2.0).abs() < 1e-5);
            // Lunghi: più larghi, sporgono sopra e scendono più in basso.
            assert!(long.x > body.x);
            assert!(long_y + long.y / 2.0 > body.y / 2.0);
            assert!(long_y - long.y / 2.0 < short_y - short.y / 2.0);
            assert!(long.y < body.y, "{age}: {long} vs {body}");
        }
    }

    #[test]
    fn idle_babies_stay_next_to_their_mother() {
        let mut sim = crate::sim_bridge::new_sim();
        // Qualche anno, finché nasce qualcuno.
        let baby = loop {
            sim.world.tick(&mut sim.brain);
            if let Some(b) = sim
                .world
                .npcs
                .iter()
                .find(|n| n.age < BABY_AGE && mother(&sim.world, n).is_some())
            {
                break b.id;
            }
            assert!(sim.world.clock.day() < 400, "nessuna nascita");
        };
        let stations = StationLayout::from_world(&sim.world);
        let world = &mut sim.world;
        let mother_id = mother(world, world.npc(baby).unwrap()).unwrap().id;
        let dorm = world.npc(mother_id).unwrap().home;
        for npc in world.npcs.iter_mut() {
            if npc.id == baby || npc.id == mother_id {
                npc.carriage = dorm;
                npc.action = Action::Idle;
            }
        }
        let world = &sim.world;
        let b = world.npc(baby).unwrap();
        let m = world.npc(mother_id).unwrap();
        let bx = npc_pose(world, &stations, b, 0).position.x;
        let mx = npc_pose(world, &stations, m, 0).position.x;
        assert!((bx - mx).abs() <= BABY_OFFSET + 0.01, "{bx} vs {mx}");
        // Se la mamma è altrove, il neonato va per conto suo.
        let mut other = b.clone();
        other.carriage = CarriageId((dorm.0 + 1) % world.carriages.len() as u16);
        assert_eq!(baby_x(world, &stations, &other), None);
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
    fn buyers_stand_next_to_a_staffed_counter() {
        let mut world = World::generate(42, 20, 400);
        let stations = StationLayout::from_world(&world);
        let market = world
            .carriages
            .iter()
            .find(|c| c.kind == sim::CarriageKind::Mercato)
            .unwrap()
            .id;
        let counters: Vec<_> = world.carriages[market.index()]
            .stations
            .iter()
            .filter(|s| s.kind == StationKind::Counter)
            .map(|s| s.id)
            .collect();
        assert!(counters.len() >= 2);
        // Solo il secondo bancone ha un mercante: tutti i clienti vanno lì.
        let staffed = counters[1];
        world.carriages[market.index()].stations[staffed.index()].occupancy = 1;
        let spot = *stations.spot(market, staffed).unwrap();
        let edge = carriage_x(market, spot.x + spot.width / 2.0);
        for i in 0..20 {
            world.npcs[i].carriage = market;
            world.npcs[i].action = Action::Buy(sim::ItemKind::Vestito);
            let npc = &world.npcs[i];
            let pose = npc_pose(&world, &stations, npc, 0);
            let x = pose.position.x;
            let reach = COUNTER_CLEARANCE + body_size(npc).x + CUSTOMER_SPREAD;
            assert!(x > edge && x <= edge + reach, "{x} vs bancone {edge}");
            assert!(!pose.lying);
        }
    }

    #[test]
    fn npcs_without_clothes_are_paler() {
        let world = World::generate(42, 20, 400);
        let mut npc = world.npcs[0].clone();
        npc.action = Action::Idle;
        npc.inventory.clothes = Some(1.0);
        let dressed = npc_color(&npc).to_srgba();
        npc.inventory.clothes = None;
        let bare = npc_color(&npc).to_srgba();
        assert_ne!(dressed, bare);
        let spread = |c: Srgba| {
            let v = [c.red, c.green, c.blue];
            v.iter().copied().fold(f32::MIN, f32::max) - v.iter().copied().fold(f32::MAX, f32::min)
        };
        assert!(spread(bare) <= spread(dressed) + 1e-6);
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
