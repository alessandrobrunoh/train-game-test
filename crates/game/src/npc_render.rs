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
//! Aspetto: ogni NPC è uno sprite in pixel art generato da `characters.rs`
//! (fascia d'età, sesso, look, divisa del lavoro, stracci se non ha vestiti)
//! e animato in base a cosa fa: respira da fermo, cammina con passi legati
//! allo spostamento vero, mangia seduto portando la mano alla bocca, lavora
//! (con l'attrezzo in mano se ne ha uno), gesticola chiacchierando, porge una
//! moneta al bancone e dorme sdraiato sotto la coperta. Guarda dove va, o
//! verso il compagno, la postazione o la mamma. Le animazioni seguono il
//! tempo reale, non la velocità della simulazione (e si fermano in pausa).
//!
//! I neonati (sotto i 3 anni) che oziano stanno accanto alla mamma, se è
//! nella stessa carrozza. Chi dorme è disegnato più scuro. Chi muore sotto
//! gli occhi del giocatore svanisce (vedi `life_fx.rs`). L'NPC selezionato ha
//! un contorno ciano e un marcatore sopra la testa.

use std::collections::{HashMap, HashSet};

use bevy::{prelude::*, sprite::Anchor, window::PrimaryWindow};
use sim::{Action, CarriageId, Npc, NpcId, Sex, StationKind, World};

use crate::camera::follow_target;
use crate::characters::{
    AppearanceKey, BABY_YEARS, CharacterArt, Frame, Stage, anim_for, anim_frame, appearance,
};
use crate::life_fx::FadingOut;
use crate::saves::WorldRebuildSet;
use crate::state::{NpcSprite, PointerOverUi, SelectedNpc, Sim, SimClock, WorldReplaced};
use crate::stations::{BED_TOP, StationLayout, interior_range};
use crate::train::{CARRIAGE_PITCH, FLOOR_Y, TrainLayout};

/// Larghezza di un NPC adulto: margine dei punti di pausa dalle pareti.
const ADULT_WIDTH: f32 = 8.0;
/// Distanza tra il neonato e la mamma.
const BABY_OFFSET: f32 = 6.0;
/// Entro questa distanza dal letto l'NPC è "arrivato" e si sdraia.
const ARRIVE_DISTANCE: f32 = 0.5;
/// Durata della rotazione quando si sdraia o si alza (secondi).
const LIE_DOWN_TIME: f32 = 0.15;

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
/// Sotto questo scarto orizzontale (unità) non ci si gira verso la postazione.
const FACE_DEADZONE: f32 = 1.0;

const SELECTED_COLOR: Color = Color::srgb(0.30, 1.00, 1.00);
/// Chi dorme è disegnato più scuro.
const SLEEP_TINT: Color = Color::srgb(0.6, 0.6, 0.66);

/// Distanza tra il bordo del bancone e il cliente.
const COUNTER_CLEARANCE: f32 = 1.0;
/// Scarto massimo tra clienti dello stesso bancone, per non sovrapporsi del tutto.
const CUSTOMER_SPREAD: f32 = 6.0;

/// Ogni quanto (secondi reali) si liberano i fogli che nessuno usa più, e
/// da quanti fogli in cache in su.
const EVICT_SECS: f32 = 10.0;
const EVICT_ABOVE: usize = 160;

pub struct NpcRenderPlugin;

impl Plugin for NpcRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NpcSpriteIndex>()
            .init_resource::<CharacterArt>()
            .add_systems(Startup, spawn_marker)
            .add_systems(
                PreUpdate,
                reset_npc_sprites
                    .in_set(WorldRebuildSet)
                    .run_if(on_message::<WorldReplaced>),
            )
            .add_systems(
                Update,
                (
                    pick_npc,
                    sync_npc_sprites,
                    move_npc_sprites,
                    update_selection,
                    evict_unused_sheets,
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

/// Contorno ciano dell'NPC selezionato (figlio dell'entità `NpcSprite`,
/// nascosto se non è selezionato).
#[derive(Component)]
struct NpcHighlight;

/// Sprite degli NPC, vivi o che stanno svanendo.
type SpriteOrFading = Or<(With<NpcSprite>, With<FadingOut>)>;

/// Filtro delle query sui contorni di selezione, disgiunto dagli NPC.
type HighlightOnly = (With<NpcHighlight>, Without<NpcSprite>);

/// Ciò che il contorno di selezione copia dallo sprite dell'NPC.
type SelectedParts = (
    &'static Transform,
    &'static Sprite,
    &'static Anchor,
    &'static NpcVisual,
);

/// Posizione e visibilità del marcatore di selezione.
type MarkerParts = (&'static mut Transform, &'static mut Visibility);

/// Dove guardare quando si è fermi.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Face {
    /// Come prima (o dove si stava andando).
    Keep,
    Left,
    Right,
    /// Verso questa x mondo (la postazione, la mamma).
    Toward(f32),
}

/// Stato grafico di uno sprite NPC.
#[derive(Component)]
pub(crate) struct NpcVisual {
    highlight: Entity,
    key: AppearanceKey,
    /// Dove lo sprite vuole andare (coordinate mondo).
    target: Vec2,
    /// La posa chiede di stare sdraiato (dorme in un letto).
    wants_lying: bool,
    /// Avanzamento della transizione: 0 = in piedi, 1 = sdraiato.
    lie: f32,
    /// Semiampiezze del corpo in coordinate mondo (per il click).
    half_extents: Vec2,
    /// Azione corrente e attrezzo posseduto: decidono l'animazione.
    action: Action,
    has_tool: bool,
    face: Face,
    facing_left: bool,
    /// Secondi reali dall'inizio dell'animazione corrente (con uno sfasamento
    /// per NPC, così non respirano tutti insieme).
    clock: f32,
    /// Pixel percorsi camminando (scandiscono i passi).
    walked: f32,
    phase: f32,
    frame: Frame,
    anim: crate::characters::Anim,
}

impl NpcVisual {
    /// Metà altezza dell'ingombro attuale (per mettere qualcosa sopra la testa).
    pub(crate) fn half_height(&self) -> f32 {
        self.half_extents.y
    }

    fn stage(&self) -> Stage {
        self.key.stage
    }

    fn rotation(&self) -> Quat {
        // Durante la transizione il corpo in piedi ruota verso il letto; una
        // volta sdraiato si passa al fotogramma disegnato apposta.
        if self.lie <= 0.0 || self.lie >= 1.0 || self.stage() == Stage::Baby {
            Quat::IDENTITY
        } else {
            Quat::from_rotation_z(std::f32::consts::FRAC_PI_2 * self.lie)
        }
    }

    /// Semiampiezze del corpo (bounding box allineata agli assi).
    fn current_half_extents(&self) -> Vec2 {
        if self.lie >= 1.0 {
            return self.stage().lying_size() / 2.0;
        }
        let half = self.stage().size() / 2.0;
        let angle = if self.stage() == Stage::Baby {
            0.0
        } else {
            std::f32::consts::FRAC_PI_2 * self.lie
        };
        let (sin, cos) = angle.sin_cos();
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
    /// Centro del corpo in coordinate mondo.
    position: Vec2,
    /// Sdraiato (a letto).
    lying: bool,
    /// Dove guarda da fermo.
    face: Face,
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
    let margin = ADULT_WIDTH;
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

/// Ingombro del corpo in piedi (larghezza, altezza), dalla fascia d'età.
fn body_size(npc: &Npc) -> Vec2 {
    Stage::of_age(npc.age).size()
}

/// La mamma dell'NPC, se è viva.
fn mother<'w>(world: &'w World, npc: &Npc) -> Option<&'w Npc> {
    npc.parents()
        .filter_map(|id| world.npc(id))
        .find(|p| p.sex == Sex::Female)
}

/// Dove sta un neonato che ozia, e dove sta la mamma: accanto a lei, se è
/// nella stessa carrozza e non sta viaggiando.
fn baby_x(world: &World, stations: &StationLayout, npc: &Npc) -> Option<(f32, f32)> {
    if npc.age >= BABY_YEARS {
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
    let mother_x = anchor_x(world, stations, mother);
    let x = mother_x + side * BABY_OFFSET;
    let (left, right) = interior_range();
    Some((
        x.clamp(
            carriage_x(npc.carriage, left),
            carriage_x(npc.carriage, right),
        ),
        mother_x,
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
    let standing = |x: f32, face: Face| Pose {
        position: Vec2::new(x, FLOOR_Y + size.y / 2.0),
        lying: false,
        face,
    };
    let c = npc.carriage;
    match npc.action {
        Action::Sleep(s) => match stations.spot(c, s) {
            Some(spot) => {
                let lying = Stage::of_age(npc.age).lying_size();
                // La testa sul cuscino (a sinistra): i piccoli non stanno al centro.
                let head_end = spot.x - spot.width / 2.0 + 1.0;
                Pose {
                    position: Vec2::new(
                        carriage_x(c, head_end + lying.x / 2.0),
                        spot.base_y() + BED_TOP + lying.y / 2.0,
                    ),
                    lying: true,
                    face: Face::Right,
                }
            }
            None => standing(idle_x(npc), Face::Keep),
        },
        Action::Eat(s) | Action::Work(s) => {
            let Some(spot) = stations.spot(c, s) else {
                return standing(idle_x(npc), Face::Keep);
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
            // Si guarda verso il centro della postazione (chi mangia guarda
            // i vicini di tavola).
            standing(
                carriage_x(c, spot.x) + offset,
                Face::Toward(carriage_x(c, spot.x)),
            )
        }
        Action::Socialize(other) => {
            let side = if npc.id < other { -1.0 } else { 1.0 };
            let x = match world.npc(other) {
                // Si parlano a vicenda: ai due lati del punto d'incontro.
                Some(p) if p.carriage == c && p.action == Action::Socialize(npc.id) => {
                    Some(chat_spot(c, npc.id, other) + side * CHAT_DISTANCE / 2.0)
                }
                // Il compagno fa altro (es. mangia): gli si mette accanto.
                Some(p) if p.carriage == c => {
                    Some(anchor_x(world, stations, p) + side * CHAT_DISTANCE)
                }
                _ => None,
            };
            let (left, right) = interior_range();
            match x {
                // Chi sta a sinistra guarda a destra, e viceversa.
                Some(x) => standing(
                    x.clamp(carriage_x(c, left), carriage_x(c, right)),
                    if side < 0.0 { Face::Right } else { Face::Left },
                ),
                None => standing(idle_x(npc), Face::Keep),
            }
        }
        Action::Travel { to } => standing(travel_x(world, npc, to), Face::Keep),
        // Il cliente sta a destra del bancone.
        Action::Buy(_) => standing(anchor_x(world, stations, npc), Face::Left),
        Action::Idle => match baby_x(world, stations, npc) {
            Some((x, mother_x)) => standing(x, Face::Toward(mother_x)),
            None => standing(idle_x(npc), Face::Keep),
        },
    }
}

/// Dove va disegnato l'NPC (centro del corpo), anche se non ha uno sprite.
pub(crate) fn npc_position(world: &World, stations: &StationLayout, npc: &Npc) -> Vec2 {
    npc_pose(world, stations, npc, 0).position
}

/// Chi dorme è disegnato più scuro.
fn npc_tint(npc: &Npc) -> Color {
    if npc.is_awake() {
        Color::WHITE
    } else {
        SLEEP_TINT
    }
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

/// Da che parte guardare: si guarda dove si va; da fermi, secondo `face`.
fn facing_left(was_left: bool, step_x: f32, position_x: f32, face: Face) -> bool {
    if step_x.abs() > 1e-3 {
        return step_x < 0.0;
    }
    match face {
        Face::Keep => was_left,
        Face::Left => true,
        Face::Right => false,
        Face::Toward(x) if (x - position_x).abs() > FACE_DEADZONE => x < position_x,
        Face::Toward(_) => was_left,
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

/// Mondo sostituito: gli id degli NPC ora indicano altre persone, quindi via
/// tutti gli sprite (senza dissolvenza); `sync_npc_sprites` li ricrea.
fn reset_npc_sprites(
    mut commands: Commands,
    mut index: ResMut<NpcSpriteIndex>,
    sprites: Query<Entity, SpriteOrFading>,
) {
    for entity in &sprites {
        commands.entity(entity).despawn();
    }
    index.entities.clear();
}

/// Crea, aggiorna e rimuove gli sprite degli NPC della finestra visibile.
#[allow(clippy::too_many_arguments)]
fn sync_npc_sprites(
    mut commands: Commands,
    sim: Res<Sim>,
    stations: Res<StationLayout>,
    camera: Single<(&Transform, &Projection), With<Camera2d>>,
    mut index: ResMut<NpcSpriteIndex>,
    mut art: ResMut<CharacterArt>,
    mut images: ResMut<Assets<Image>>,
    mut sprites: Query<(&mut Sprite, &mut Anchor, &mut NpcVisual), With<NpcSprite>>,
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
        let key = appearance(npc);
        let tint = npc_tint(npc);
        let has_tool = npc.inventory.tool.is_some();

        let existing = index.entities.get(&npc.id).map(|&(e, _)| e);
        if let Some(entity) = existing
            && let Ok((mut sprite, mut anchor, mut visual)) = sprites.get_mut(entity)
        {
            // Posizione, animazione e rotazione le aggiorna `move_npc_sprites`,
            // in base a dove si trova davvero lo sprite.
            visual.target = pose.position;
            visual.wants_lying = pose.lying;
            visual.action = npc.action;
            visual.has_tool = has_tool;
            visual.face = pose.face;
            if visual.key != key {
                // Cresciuto, cambiato lavoro, vestiti consumati o comprati.
                visual.key = key;
                sprite.image = art.sheet(key, &mut images);
                anchor.0 = key.stage.anchor();
            }
            if sprite.color != tint {
                sprite.color = tint;
            }
            index.entities.insert(npc.id, (entity, frame));
            continue;
        }

        // Nasce già nella posa finale (anche sdraiato, se è già a letto).
        let phase = hash01(u64::from(npc.id.0) ^ 0xA11A) * 4.0;
        let lie = if pose.lying { 1.0 } else { 0.0 };
        let moving = false;
        let anim = anim_for(&npc.action, moving, pose.lying, has_tool);
        let first = anim_frame(anim, phase, 0.0, key.stage.stride());
        let mut visual = NpcVisual {
            highlight: Entity::PLACEHOLDER,
            key,
            target: pose.position,
            wants_lying: pose.lying,
            lie,
            half_extents: Vec2::ZERO,
            action: npc.action,
            has_tool,
            face: pose.face,
            facing_left: facing_left(
                hash01(u64::from(npc.id.0) ^ 0xFACE) < 0.5,
                0.0,
                pose.position.x,
                pose.face,
            ),
            clock: phase,
            walked: 0.0,
            phase,
            frame: first,
            anim,
        };
        visual.half_extents = visual.current_half_extents();

        if let Some(stale) = existing {
            commands.entity(stale).despawn();
        }
        let image = art.sheet(key, &mut images);
        let anchor = Anchor(key.stage.anchor());
        // Contorno di selezione: sempre presente (nascosto), dietro lo sprite.
        let highlight = commands
            .spawn((
                NpcHighlight,
                Sprite {
                    color: SELECTED_COLOR,
                    ..default()
                },
                anchor,
                Transform::from_xyz(0.0, 0.0, -Z_STEP / 2.0),
                Visibility::Hidden,
            ))
            .id();
        visual.highlight = highlight;
        // Ogni NPC ha una profondità propria: niente sfarfallio tra sprite sovrapposti.
        let z = NPC_Z + (npc.id.0 % 1000) as f32 * Z_STEP;
        let sprite = Sprite {
            color: tint,
            flip_x: visual.facing_left && lie < 1.0,
            ..Sprite::from_atlas_image(image, art.atlas(first))
        };
        let entity = commands
            .spawn((
                Name::new(npc.name.clone()),
                NpcSprite(npc.id),
                visual,
                sprite,
                anchor,
                Transform::from_translation(pose.position.extend(z)),
            ))
            .add_child(highlight)
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

/// Muove gli sprite verso la loro posizione obiettivo, li fa sdraiare solo
/// una volta arrivati al letto (mentre ci vanno camminano in piedi) e sceglie
/// il fotogramma dell'animazione.
fn move_npc_sprites(
    time: Res<Time<Real>>,
    clock: Res<SimClock>,
    art: Res<CharacterArt>,
    mut sprites: Query<(&mut Transform, &mut Sprite, &mut NpcVisual), With<NpcSprite>>,
) {
    let dt = time.delta_secs();
    // In pausa i gesti si fermano (i passi seguono comunque lo spostamento).
    let anim_dt = if clock.paused { 0.0 } else { dt };
    for (mut transform, mut sprite, mut visual) in &mut sprites {
        let current = transform.translation.truncate();
        let delta = visual.target - current;
        let distance = delta.length();
        let teleport = distance > TELEPORT_DISTANCE;
        let next = if teleport {
            visual.target
        } else {
            let step = (WALK_SPEED + distance * CATCH_UP) * dt;
            current + delta.clamp_length_max(step)
        };
        if next != current {
            transform.translation.x = next.x;
            transform.translation.y = next.y;
        }
        let step = if teleport { Vec2::ZERO } else { next - current };

        let arrived = next.distance(visual.target) <= ARRIVE_DISTANCE;
        let goal = if visual.wants_lying && arrived {
            1.0
        } else {
            0.0
        };
        let lie = if teleport {
            // Teletrasportato: niente animazione.
            goal
        } else {
            visual.lie + (goal - visual.lie).clamp(-dt / LIE_DOWN_TIME, dt / LIE_DOWN_TIME)
        };
        if lie != visual.lie {
            visual.lie = lie;
        }
        let half_extents = visual.current_half_extents();
        if visual.half_extents != half_extents {
            visual.half_extents = half_extents;
        }
        let rotation = visual.rotation();
        if transform.rotation != rotation {
            transform.rotation = rotation;
        }

        // Animazione.
        let moving = !arrived && !teleport;
        let anim = anim_for(&visual.action, moving, lie >= 1.0, visual.has_tool);
        if anim != visual.anim {
            visual.anim = anim;
            visual.clock = visual.phase;
        } else {
            visual.clock += anim_dt;
        }
        visual.walked += step.length();
        let stage = visual.stage();
        let frame = if lie > 0.0 && lie < 1.0 {
            // Mentre si sdraia o si alza: il corpo in piedi che ruota.
            Frame::Idle0
        } else {
            anim_frame(anim, visual.clock, visual.walked, stage.stride())
        };
        let left = facing_left(visual.facing_left, step.x, next.x, visual.face);
        if left != visual.facing_left {
            visual.facing_left = left;
        }
        // Sdraiati (o mentre ci si sdraia) la testa va sempre a sinistra.
        let flip = visual.facing_left && lie <= 0.0;
        if sprite.flip_x != flip {
            sprite.flip_x = flip;
        }
        if visual.frame != frame || sprite.texture_atlas.is_none() {
            visual.frame = frame;
            sprite.texture_atlas = Some(art.atlas(frame));
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

/// Mostra il marcatore sopra l'NPC selezionato (se è disegnato) e il suo
/// contorno ciano, che segue fotogramma, verso e ancora dello sprite.
#[allow(clippy::too_many_arguments)]
fn update_selection(
    time: Res<Time>,
    selected: Res<SelectedNpc>,
    index: Res<NpcSpriteIndex>,
    mut art: ResMut<CharacterArt>,
    mut images: ResMut<Assets<Image>>,
    sprites: Query<SelectedParts, (Without<SelectionMarker>, Without<NpcHighlight>)>,
    mut highlights: Query<(&mut Sprite, &mut Anchor, &mut Visibility), HighlightOnly>,
    mut marker: Single<MarkerParts, (With<SelectionMarker>, Without<NpcHighlight>)>,
    mut shown: Local<Option<(Entity, AppearanceKey)>>,
) {
    let (marker_transform, visibility) = &mut *marker;
    let target = selected
        .0
        .and_then(|id| index.entities.get(&id))
        .and_then(|&(entity, _)| sprites.get(entity).ok());
    let highlight = target.map(|(_, _, _, visual)| visual.highlight);
    // Il contorno di prima si nasconde se la selezione è cambiata.
    if let Some((previous, _)) = *shown
        && Some(previous) != highlight
    {
        if let Ok((_, _, mut vis)) = highlights.get_mut(previous) {
            vis.set_if_neq(Visibility::Hidden);
        }
        *shown = None;
    }
    let Some((transform, sprite, anchor, visual)) = target else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    visibility.set_if_neq(Visibility::Visible);
    let bob = (time.elapsed_secs() * 4.0).sin() * 1.5;
    let top = transform.translation.y + visual.half_extents.y;
    marker_transform.translation.x = transform.translation.x;
    marker_transform.translation.y = top + 6.0 + bob;

    if let Ok((mut outline, mut outline_anchor, mut vis)) = highlights.get_mut(visual.highlight) {
        if *shown != Some((visual.highlight, visual.key)) {
            outline.image = art.highlight(visual.key, &mut images);
            *shown = Some((visual.highlight, visual.key));
        }
        if outline.texture_atlas.as_ref().map(|a| a.index)
            != sprite.texture_atlas.as_ref().map(|a| a.index)
        {
            outline.texture_atlas = sprite.texture_atlas.clone();
        }
        if outline.flip_x != sprite.flip_x {
            outline.flip_x = sprite.flip_x;
        }
        if *outline_anchor != *anchor {
            *outline_anchor = *anchor;
        }
        vis.set_if_neq(Visibility::Inherited);
    }
}

/// Ogni tanto libera i fogli degli aspetti che nessuno sprite usa più
/// (NPC usciti dalla vista, cresciuti, morti).
fn evict_unused_sheets(
    time: Res<Time<Real>>,
    mut art: ResMut<CharacterArt>,
    visuals: Query<&NpcVisual>,
    mut timer: Local<f32>,
) {
    *timer += time.delta_secs();
    if *timer < EVICT_SECS {
        return;
    }
    *timer = 0.0;
    if art.cached() <= EVICT_ABOVE {
        return;
    }
    let used: HashSet<AppearanceKey> = visuals.iter().map(|v| v.key).collect();
    art.retain_used(&used);
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
        let size = |age| Stage::of_age(age).size();
        let (baby, child, youth, adult, old) = (size(0), size(8), size(16), size(30), size(90));
        assert_eq!(adult, Vec2::new(8.0, 16.0));
        assert_eq!(size(18), adult);
        assert_eq!(size(64), adult);
        assert!(baby.y < child.y && child.y < youth.y && youth.y < adult.y);
        assert!(baby.x <= child.x && child.x < adult.x);
        assert!(baby.y <= adult.y / 3.0, "i neonati sono minuscoli: {baby}");
        assert!(
            youth.y >= adult.y * 0.85,
            "i giovani sono quasi adulti: {youth}"
        );
        assert!(old.y < adult.y && old.y > adult.y * 0.8, "{old}");
        assert_eq!(old.x, adult.x);
        // Non si rimpicciolisce mai crescendo (tranne da anziani).
        for age in 0..64 {
            assert!(size(age).y <= size(age + 1).y, "{age}");
        }
    }

    #[test]
    fn idle_babies_stay_next_to_their_mother_and_look_at_her() {
        let mut sim = crate::sim_bridge::new_sim();
        // Qualche anno, finché nasce qualcuno.
        let baby = loop {
            sim.world.tick(&mut sim.brain);
            if let Some(b) = sim
                .world
                .npcs
                .iter()
                .find(|n| n.age < BABY_YEARS && mother(&sim.world, n).is_some())
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
        let pose = npc_pose(world, &stations, b, 0);
        let bx = pose.position.x;
        let mx = npc_pose(world, &stations, m, 0).position.x;
        assert!((bx - mx).abs() <= BABY_OFFSET + 0.01, "{bx} vs {mx}");
        assert_eq!(pose.face, Face::Toward(mx));
        assert_eq!(facing_left(false, 0.0, bx, pose.face), mx < bx);
        // Se la mamma è altrove, il neonato va per conto suo.
        let mut other = b.clone();
        other.carriage = CarriageId((dorm.0 + 1) % world.carriages.len() as u16);
        assert_eq!(baby_x(world, &stations, &other), None);
    }

    #[test]
    fn facing_follows_movement_then_the_pose() {
        // Camminando si guarda dove si va.
        assert!(facing_left(false, -0.5, 0.0, Face::Right));
        assert!(!facing_left(true, 0.5, 0.0, Face::Left));
        // Da fermi: la posa decide, oppure si resta come prima.
        assert!(facing_left(false, 0.0, 0.0, Face::Left));
        assert!(!facing_left(true, 0.0, 0.0, Face::Right));
        assert!(facing_left(true, 0.0, 0.0, Face::Keep));
        assert!(facing_left(false, 0.0, 10.0, Face::Toward(0.0)));
        assert!(!facing_left(true, 0.0, 10.0, Face::Toward(20.0)));
        // Al centro della postazione non ci si gira.
        assert!(facing_left(true, 0.0, 10.0, Face::Toward(10.5)));
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
    fn buyers_stand_next_to_a_staffed_counter_facing_it() {
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
            assert_eq!(pose.face, Face::Left);
        }
    }

    #[test]
    fn sleepers_lie_on_the_mattress_with_the_head_on_the_pillow() {
        let mut world = World::generate(42, 20, 400);
        let stations = StationLayout::from_world(&world);
        let dorm = world.npcs[0].home;
        let (bed, spot) = stations.carriages[dorm.index()]
            .iter()
            .enumerate()
            .find(|(_, s)| s.kind == StationKind::Bed)
            .map(|(i, s)| (sim::StationId(i as u16), *s))
            .unwrap();
        for age in [1, 8, 16, 30, 80] {
            let npc = &mut world.npcs[0];
            npc.carriage = dorm;
            npc.age = age;
            npc.action = Action::Sleep(bed);
            let pose = npc_pose(&world, &stations, &world.npcs[0], 0);
            let lying = Stage::of_age(age).lying_size();
            assert!(pose.lying);
            let bottom = pose.position.y - lying.y / 2.0;
            assert!((bottom - (spot.base_y() + BED_TOP)).abs() < 1e-4, "{age}");
            let left = pose.position.x - lying.x / 2.0;
            let pillow = carriage_x(dorm, spot.x - spot.width / 2.0 + 1.0);
            assert!((left - pillow).abs() < 1e-4, "{age}");
        }
    }

    #[test]
    fn sleeping_npcs_are_darker() {
        let world = World::generate(42, 20, 400);
        let mut npc = world.npcs[0].clone();
        npc.action = Action::Idle;
        assert_eq!(npc_tint(&npc), Color::WHITE);
        npc.action = Action::Sleep(sim::StationId(0));
        let dark = npc_tint(&npc).to_srgba();
        assert!(dark.red < 1.0 && dark.green < 1.0);
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
