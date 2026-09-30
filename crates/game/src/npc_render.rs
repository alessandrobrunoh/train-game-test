//! Disegno degli NPC della simulazione e selezione con il mouse.
//!
//! Si disegnano solo gli NPC della "finestra visibile": le carrozze inquadrate
//! dalla camera più una per lato, più chi sta viaggiando attraverso di esse.
//! Ogni NPC visibile ha un'entità `NpcSprite`; esce dalla finestra o muore e
//! l'entità viene rimossa. La posizione deriva dall'azione corrente:
//! - mangia/dorme/lavora: alla sua postazione (vedi `stations.rs`), con un
//!   posto diverso per ognuno di chi la condivide, che tiene finché resta lì
//!   (a tavola: tre dietro e tre davanti al tavolo, vedi `seat_offset`);
//! - aspetta un posto in Mensa: in fila nella zona della coda, tra i tavoli e
//!   le cucine, nell'ordine d'arrivo e rivolto alle cucine;
//! - socializza: accanto al compagno;
//! - compra: al bancone del Mercato, davanti a un mercante al lavoro;
//! - ozia: in un punto deterministico della carrozza, diverso a ogni pausa
//!   (in Mensa lontano dai tavoli, davanti al magazzino; lì si chiacchiera);
//! - viaggia: da dove si trovava lo sprite quando è partito al centro della
//!   carrozza di arrivo, attraversando porte e soffietti, in base
//!   all'avanzamento dell'azione misurato anche tra un tick e l'altro.
//!
//! Chi viaggia segue esattamente il suo tragitto (a velocità di gioco alte
//! corre, ma non salta mai carrozze). Gli altri sprite camminano verso la
//! posizione obiettivo e si teletrasportano solo per salti molto lunghi
//! (es. dopo un caricamento).
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
//! gli occhi del giocatore svanisce (vedi `life_fx.rs`); chi è stato ucciso
//! prima cade a terra. L'NPC selezionato ha un contorno ciano e un
//! marcatore sopra la testa.
//!
//! Risse (vedi `combat.rs`): chi aggredisce va addosso alla vittima (o al
//! giocatore) e mena le braccia; chi è ferito cammina più piano,
//! zoppicando; chi è grave sta a letto (dorme, come vuole la sim).

use std::collections::{HashMap, HashSet};

use bevy::{prelude::*, sprite::Anchor, window::PrimaryWindow};
use sim::{Action, CarriageId, Fighter, Npc, NpcId, Sex, StationKind, World};

use crate::camera::follow_target;
use crate::characters::{
    AppearanceKey, BABY_YEARS, CharacterArt, Frame, Stage, anim_for, anim_frame, appearance,
};
use crate::life_fx::FadingOut;
use crate::player::Player;
use crate::saves::WorldRebuildSet;
use crate::state::{NpcSprite, PointerOverUi, SelectedNpc, Sim, SimClock, WorldReplaced};
use crate::stations::{BED_TOP, FAR_ROW_RISE, StationLayout, interior_range, seat_offset};
use crate::train::{CARRIAGE_PITCH, FLOOR_Y, STOREY, TrainLayout, floor_y};

/// Larghezza di un NPC adulto: margine dei punti di pausa dalle pareti.
const ADULT_WIDTH: f32 = 8.0;
/// Distanza tra il neonato e la mamma.
const BABY_OFFSET: f32 = 6.0;
/// Entro questa distanza dal letto l'NPC è "arrivato" e si sdraia.
const ARRIVE_DISTANCE: f32 = 0.5;
/// Durata della rotazione quando si sdraia o si alza (secondi).
const LIE_DOWN_TIME: f32 = 0.15;

const NPC_Z: f32 = 3.0;
/// La fila lontana (a tavola, in coda) è disegnata dietro a quella vicina,
/// ma sempre davanti alle postazioni (`Z_STATION` = -3).
const FAR_Z: f32 = -0.6;
/// Scarto di profondità tra un NPC e l'altro (1000 NPC stanno in 0.5).
const Z_STEP: f32 = 0.0005;
const MARKER_Z: f32 = 6.0;

/// Velocità di camminata verso l'obiettivo (unità/s)...
const WALK_SPEED: f32 = 40.0;
/// ...che aumenta con la distanza, per non restare indietro a lungo (1/s).
const CATCH_UP: f32 = 3.0;
/// Oltre questa distanza lo sprite si teletrasporta invece di camminare.
/// Dentro una carrozza non succede mai; chi viaggia non si teletrasporta.
const TELEPORT_DISTANCE: f32 = CARRIAGE_PITCH * 1.5;
/// Distanza tra due NPC che chiacchierano.
const CHAT_DISTANCE: f32 = 10.0;
/// Distanza dal giocatore di chi lo saluta.
const GREET_DISTANCE: f32 = 14.0;
/// Distanza tra due che si picchiano (o dal giocatore).
const FIGHT_DISTANCE: f32 = 9.0;
/// Chi è ferito cammina a questa frazione della velocità, e zoppica
/// inclinandosi di al più questo angolo (radianti) a ogni passo.
const HURT_WALK: f32 = 0.6;
const LIMP_TILT: f32 = 0.1;
/// Distanza tra due persone in coda, e tra la prima e le cucine.
const QUEUE_SPACING: f32 = 6.0;
const QUEUE_HEAD_GAP: f32 = 2.0;

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
    /// Ferito (sotto la soglia della sim): cammina piano e zoppica.
    hurt: bool,
    face: Face,
    facing_left: bool,
    /// Profondità dello sprite (vedi [`sprite_z`]).
    z: f32,
    /// Secondi reali dall'inizio dell'animazione corrente (con uno sfasamento
    /// per NPC, così non respirano tutti insieme).
    clock: f32,
    /// Pixel percorsi camminando (scandiscono i passi).
    walked: f32,
    phase: f32,
    frame: Frame,
    anim: crate::characters::Anim,
    /// Viaggio in corso: da dove è partito lo sprite.
    travel: Option<TravelPath>,
}

/// Tragitto di un NPC che viaggia tra carrozze.
#[derive(Clone, Copy, Debug, PartialEq)]
struct TravelPath {
    /// Inizio dell'azione di viaggio (minuti di gioco): identifica il viaggio.
    since: u64,
    /// Dove si trovava lo sprite alla partenza...
    from_x: f32,
    /// ...e a che piano: da un piano alto prima si scende la scala.
    from_floor: u8,
}

impl NpcVisual {
    /// Metà altezza del corpo in piedi (la quota a cui cammina).
    fn standing_half_height(&self) -> f32 {
        self.stage().size().y / 2.0
    }

    /// Metà altezza dell'ingombro attuale (per mettere qualcosa sopra la testa).
    pub(crate) fn half_height(&self) -> f32 {
        self.half_extents.y
    }

    /// In piedi (né sdraiato né mentre si sdraia): per la fascia della banda.
    pub(crate) fn upright(&self) -> bool {
        self.lie <= 0.0
    }

    /// Altezza del corpo in piedi.
    pub(crate) fn body_height(&self) -> f32 {
        2.0 * self.standing_half_height()
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
    /// Nella fila lontana (a tavola, in coda): disegnato dietro agli altri.
    far: bool,
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

/// Punto casuale ma deterministico del pavimento di una carrozza (x mondo):
/// in Mensa nella zona per oziare, lontano dai tavoli.
fn floor_spot(stations: &StationLayout, carriage: CarriageId, key: u64) -> f32 {
    let (left, right) = stations.lounge(carriage).unwrap_or_else(interior_range);
    let margin = ADULT_WIDTH.min((right - left) / 2.0);
    carriage_x(
        carriage,
        left + margin + hash01(key) * (right - left - 2.0 * margin).max(0.0),
    )
}

/// Dove due NPC si incontrano per chiacchierare (uguale per entrambi).
fn chat_spot(stations: &StationLayout, carriage: CarriageId, a: NpcId, b: NpcId) -> f32 {
    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
    floor_spot(
        stations,
        carriage,
        (u64::from(lo.0) << 32) | u64::from(hi.0) | 1 << 63,
    )
}

/// Posto numero `slot` della coda di una Mensa (0 = il primo, accanto alle
/// cucine): x mondo, scarto verticale e se sta nella fila lontana. Una fila
/// riempie la zona della coda, poi si comincia la seconda (più in alto e
/// dietro, sfalsata di mezzo posto).
fn queue_spot(stations: &StationLayout, npc: &Npc, slot: u16) -> Option<(f32, f32, bool)> {
    let (left, right) = stations.queue(npc.carriage)?;
    let body = body_size(npc).x;
    let usable = (right - left - QUEUE_HEAD_GAP - body).max(0.0);
    let per_row = (usable / QUEUE_SPACING) as u16 + 1;
    // Oltre la seconda fila si torna alla prima (una coda più lunga del previsto).
    let (row, i) = ((slot / per_row) % 2, slot % per_row);
    let x = right
        - QUEUE_HEAD_GAP
        - body / 2.0
        - f32::from(i) * QUEUE_SPACING
        - f32::from(row) * QUEUE_SPACING / 2.0;
    let x = x.max(left + body / 2.0);
    Some((
        carriage_x(npc.carriage, x),
        f32::from(row) * FAR_ROW_RISE,
        row == 1,
    ))
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
                None => idle_x(stations, npc),
            }
        }
        Action::Socialize(other) => chat_spot(stations, npc.carriage, npc.id, other),
        Action::Travel { to } => travel_x(world, npc, to),
        Action::Buy(_) => counter_x(world, stations, npc).unwrap_or_else(|| idle_x(stations, npc)),
        Action::Idle | Action::Attack(_) => idle_x(stations, npc),
        Action::Wait => queue_spot(stations, npc, 0).map_or_else(|| idle_x(stations, npc), |q| q.0),
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

fn idle_x(stations: &StationLayout, npc: &Npc) -> f32 {
    floor_spot(
        stations,
        npc.carriage,
        (u64::from(npc.id.0) << 32) ^ npc.action_since.minutes(),
    )
}

fn travel_x(world: &World, npc: &Npc, to: CarriageId) -> f32 {
    let from = TrainLayout::carriage_center_x(npc.carriage.index());
    let dest = TrainLayout::carriage_center_x(to.index());
    from + (dest - from) * npc.action_progress(world.clock)
}

/// Avanzamento (0..=1) dell'azione corrente contando anche la frazione di
/// minuto già trascorsa verso il prossimo tick (`fraction`, 0..1): tra un tick
/// e l'altro chi viaggia continua a muoversi invece di avanzare a scatti.
///
/// Il tick del minuto `t` fa partire le azioni con `action_since = t` e poi
/// porta l'orologio a `t + 1`; l'azione finisce nel tick del minuto
/// `action_until`. Tra quei due tick l'avanzamento va quindi da 0 a 1 contando
/// un minuto in meno rispetto a `Npc::action_progress`.
fn smooth_progress(world: &World, npc: &Npc, fraction: f32) -> f32 {
    let total = npc.action_until.since(npc.action_since);
    if total == 0 {
        return 1.0;
    }
    let elapsed = world.clock.since(npc.action_since) as f32 - 1.0 + fraction.clamp(0.0, 1.0);
    (elapsed / total as f32).clamp(0.0, 1.0)
}

/// Posizione lungo il tragitto da `from_x` al centro della carrozza `to`.
fn travel_path_x(from_x: f32, to: CarriageId, progress: f32) -> f32 {
    let dest = TrainLayout::carriage_center_x(to.index());
    from_x + (dest - from_x) * progress
}

/// Centro (x) della scala della carrozza `index`.
fn stairs_center_x(index: usize) -> f32 {
    let (x0, x1) = TrainLayout::stairs_x(index);
    (x0 + x1) / 2.0
}

/// Dove sta (centro del corpo) chi viaggia verso `to` con avanzamento
/// `progress`. Chi parte da un piano alto usa i primi
/// `SimParams::stairs_minutes` del viaggio per andare alla scala (metà) e
/// scenderla (l'altra metà); poi cammina al piano terra, come chi parte da lì.
fn travel_target(
    world: &World,
    npc: &Npc,
    path: TravelPath,
    to: CarriageId,
    progress: f32,
) -> Vec2 {
    let half = body_size(npc).y / 2.0;
    let ground = FLOOR_Y + half;
    let total = npc.action_until.since(npc.action_since).max(1) as f32;
    let stairs = (world.params.stairs_minutes as f32 / total).clamp(0.0, 0.5);
    if path.from_floor == 0 || stairs <= 0.0 {
        return Vec2::new(travel_path_x(path.from_x, to, progress), ground);
    }
    let ladder = stairs_center_x(npc.carriage.index());
    let upper = floor_y(usize::from(path.from_floor)) + half;
    let half_stairs = stairs / 2.0;
    if progress < half_stairs {
        let t = progress / half_stairs;
        Vec2::new(path.from_x + (ladder - path.from_x) * t, upper)
    } else if progress < stairs {
        let t = (progress - half_stairs) / half_stairs;
        Vec2::new(ladder, upper + (ground - upper) * t)
    } else {
        let t = (progress - stairs) / (1.0 - stairs);
        Vec2::new(travel_path_x(ladder, to, t), ground)
    }
}

/// Piano (0 = terra) a cui sta un corpo con il centro a quota `y`.
fn floor_of(y: f32) -> usize {
    ((y - FLOOR_Y) / STOREY).floor().max(0.0) as usize
}

/// Prossima tappa di uno sprite in `current` diretto a `target` dentro una
/// carrozza con `floors` piani: se il bersaglio è a un altro piano si va alla
/// scala, la si sale o scende, e solo dopo si raggiunge il posto. `half` è la
/// metà altezza del corpo in piedi.
fn waypoint(current: Vec2, target: Vec2, half: f32, floors: usize) -> Vec2 {
    if floors <= 1 {
        return target;
    }
    let (from, to) = (floor_of(current.y), floor_of(target.y));
    let ladder = stairs_center_x((current.x / CARRIAGE_PITCH).floor().max(0.0) as usize);
    let level = |f: usize| floor_y(f) + half;
    let on_ladder = (current.x - ladder).abs() <= 0.5;
    let between_floors = (0..floors).all(|f| (current.y - level(f)).abs() > 1.0);
    if on_ladder && between_floors {
        // A metà scala: si finisce di salire o scendere.
        return Vec2::new(ladder, level(to));
    }
    if from == to {
        target
    } else if on_ladder {
        Vec2::new(ladder, level(to))
    } else {
        Vec2::new(ladder, level(from))
    }
}

/// Posa dell'NPC. `seat` è il suo posto tra chi usa la stessa postazione,
/// o in coda (0 = il primo).
fn npc_pose(world: &World, stations: &StationLayout, npc: &Npc, seat: u16) -> Pose {
    let size = body_size(npc);
    let floor = if matches!(npc.action, Action::Travel { .. }) {
        // I passaggi tra carrozze sono al piano terra (vedi `travel_target`).
        FLOOR_Y
    } else {
        floor_y(usize::from(npc.floor))
    };
    let standing = |x: f32, face: Face| Pose {
        position: Vec2::new(x, floor + size.y / 2.0),
        lying: false,
        face,
        far: false,
    };
    // Più in alto di `rise` e dietro agli altri se sta nella fila lontana.
    let in_row = |mut pose: Pose, rise: f32, far: bool| {
        pose.position.y += rise;
        pose.far = far;
        pose
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
                    far: false,
                }
            }
            None => standing(idle_x(stations, npc), Face::Keep),
        },
        Action::Eat(s) | Action::Work(s) => {
            let Some(spot) = stations.spot(c, s) else {
                return standing(idle_x(stations, npc), Face::Keep);
            };
            let capacity = world
                .carriage(c)
                .and_then(|carriage| carriage.station(s))
                .map_or(1, |st| st.capacity.max(1));
            let center = carriage_x(c, spot.x);
            if spot.kind == StationKind::Table {
                // Ognuno al suo posto, dietro o davanti al tavolo.
                let (dx, rise, far) = seat_offset(capacity, seat, spot.width);
                let pose = standing(center + dx, Face::Toward(center));
                return in_row(pose, rise, far);
            }
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
                    Some(chat_spot(stations, c, npc.id, other) + side * CHAT_DISTANCE / 2.0)
                }
                // Il compagno fa altro (es. mangia): gli si mette accanto,
                // tranne in Mensa (non si sta addosso ai tavoli).
                Some(p) if p.carriage == c && stations.lounge(c).is_none() => {
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
                None => standing(idle_x(stations, npc), Face::Keep),
            }
        }
        // In fila, rivolto alle cucine.
        Action::Wait => match queue_spot(stations, npc, seat) {
            Some((x, rise, far)) => in_row(standing(x, Face::Right), rise, far),
            None => standing(idle_x(stations, npc), Face::Keep),
        },
        Action::Travel { to } => standing(travel_x(world, npc, to), Face::Keep),
        // Il cliente sta a destra del bancone.
        Action::Buy(_) => standing(anchor_x(world, stations, npc), Face::Left),
        Action::Idle => match baby_x(world, stations, npc) {
            Some((x, mother_x)) => standing(x, Face::Toward(mother_x)),
            None => standing(idle_x(stations, npc), Face::Keep),
        },
        // Addosso alla vittima (al giocatore ci pensa `approach_player`).
        Action::Attack(Fighter::Npc(other)) => {
            let side = if npc.id < other { -1.0 } else { 1.0 };
            let x = match world.npc(other) {
                // Si picchiano a vicenda: ai due lati di un punto d'incontro.
                Some(p) if p.carriage == c && p.action == Action::Attack(Fighter::Npc(npc.id)) => {
                    Some(chat_spot(stations, c, npc.id, other) + side * FIGHT_DISTANCE / 2.0)
                }
                Some(p) if p.carriage == c && p.floor == npc.floor => {
                    Some(anchor_x(world, stations, p) + side * FIGHT_DISTANCE)
                }
                _ => None,
            };
            let (left, right) = interior_range();
            match x {
                Some(x) => standing(
                    x.clamp(carriage_x(c, left), carriage_x(c, right)),
                    if side < 0.0 { Face::Right } else { Face::Left },
                ),
                None => standing(idle_x(stations, npc), Face::Keep),
            }
        }
        Action::Attack(Fighter::Player) => standing(idle_x(stations, npc), Face::Keep),
    }
}

/// Chi sta salutando il giocatore (`World::greeting_of`) e ozia nel suo
/// stesso posto gli si avvicina, dalla parte da cui arriva, e lo guarda.
fn approach_player(world: &World, npc: &Npc, player: Vec2, pose: &mut Pose) {
    let greets = npc.action == Action::Idle && world.greeting_of(npc.id).is_some();
    // Chi picchia il giocatore gli va addosso.
    let fights = npc.action == Action::Attack(Fighter::Player);
    if !(greets || fights) || !world.with_player(npc) {
        return;
    }
    let distance = if fights {
        FIGHT_DISTANCE
    } else {
        GREET_DISTANCE
    };
    let side = if pose.position.x < player.x {
        -1.0
    } else {
        1.0
    };
    let (left, right) = interior_range();
    let c = npc.carriage;
    pose.position.x = (player.x + side * distance).clamp(carriage_x(c, left), carriage_x(c, right));
    pose.face = Face::Toward(player.x);
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

/// Componenti di uno sprite NPC aggiornati da `sync_npc_sprites`.
type SyncedSprite = (
    &'static mut Sprite,
    &'static mut Anchor,
    &'static mut NpcVisual,
    &'static Transform,
);

/// Postazione (carrozza, id) e posto che un NPC occupa, tra un fotogramma e l'altro.
type SeatKey = (CarriageId, u16, u16);

/// Profondità dello sprite: ogni NPC ha la sua (niente sfarfallio tra sprite
/// sovrapposti); la fila lontana sta dietro a quella vicina.
fn sprite_z(id: NpcId, far: bool) -> f32 {
    let base = if far { NPC_Z + FAR_Z } else { NPC_Z };
    base + (id.0 % 1000) as f32 * Z_STEP
}

/// Posti degli NPC della finestra visibile. A una postazione chi c'era già
/// tiene il suo posto (ricordato in `seats`) e chi arriva prende il primo
/// libero, così nessuno scivola quando un vicino se ne va; in coda conta
/// l'ordine d'arrivo nella propria Mensa (come la sim serve la coda).
fn assign_slots(
    world: &World,
    lo: usize,
    hi: usize,
    seats: &mut HashMap<NpcId, SeatKey>,
) -> HashMap<NpcId, u16> {
    let mut used: HashMap<(CarriageId, u16), u64> = HashMap::new();
    let mut kept: Vec<(NpcId, SeatKey)> = Vec::new();
    let mut newcomers: Vec<(NpcId, (CarriageId, u16))> = Vec::new();
    let mut queue: Vec<&Npc> = Vec::new();
    for npc in world.npcs.iter().filter(|n| in_window(n, lo, hi)) {
        if npc.action == Action::Wait {
            queue.push(npc);
            continue;
        }
        let Some(station) = npc.action.station() else {
            continue;
        };
        let key = (npc.carriage, station.0);
        match seats.get(&npc.id) {
            Some(&(c, s, seat))
                if (c, s) == key
                    && seat < 64
                    && used.get(&key).is_none_or(|m| m & (1u64 << seat) == 0) =>
            {
                *used.entry(key).or_default() |= 1u64 << seat;
                kept.push((npc.id, (c, s, seat)));
            }
            _ => newcomers.push((npc.id, key)),
        }
    }
    for (id, key) in newcomers {
        let mask = used.entry(key).or_default();
        let seat = (!*mask).trailing_zeros().min(63) as u16;
        *mask |= 1u64 << seat;
        kept.push((id, (key.0, key.1, seat)));
    }
    seats.clear();
    let mut slots = HashMap::with_capacity(kept.len() + queue.len());
    for (id, key) in kept {
        seats.insert(id, key);
        slots.insert(id, key.2);
    }
    queue.sort_by_key(|n| (n.carriage, n.action_since, n.id));
    let mut place = (None, 0u16);
    for n in queue {
        if place.0 != Some(n.carriage) {
            place = (Some(n.carriage), 0);
        }
        slots.insert(n.id, place.1);
        place.1 += 1;
    }
    slots
}

/// Crea, aggiorna e rimuove gli sprite degli NPC della finestra visibile.
#[allow(clippy::too_many_arguments)]
fn sync_npc_sprites(
    mut commands: Commands,
    sim: Res<Sim>,
    clock: Res<SimClock>,
    stations: Res<StationLayout>,
    camera: Single<(&Transform, &Projection), With<Camera2d>>,
    mut index: ResMut<NpcSpriteIndex>,
    mut art: ResMut<CharacterArt>,
    mut images: ResMut<Assets<Image>>,
    mut sprites: Query<SyncedSprite, With<NpcSprite>>,
    player: Query<&Transform, (With<Player>, Without<NpcSprite>)>,
    mut seats: Local<HashMap<NpcId, SeatKey>>,
) {
    let world = &sim.world;
    let player_pos = player.single().ok().map(|t| t.translation.truncate());
    // Frazione di minuto già trascorsa verso il prossimo tick.
    let fraction = clock.accumulator;
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
    let slots = assign_slots(world, lo, hi, &mut seats);

    for npc in &world.npcs {
        if !in_window(npc, lo, hi) {
            continue;
        }
        let seat = slots.get(&npc.id).copied().unwrap_or(0);
        let mut pose = npc_pose(world, &stations, npc, seat);
        if let Some(player) = player_pos {
            approach_player(world, npc, player, &mut pose);
        }
        let z = sprite_z(npc.id, pose.far);
        if let Action::Travel { to } = npc.action {
            // Posizione continua tra un tick e l'altro (usata se nasce ora).
            let path = TravelPath {
                since: npc.action_since.minutes(),
                from_x: TrainLayout::carriage_center_x(npc.carriage.index()),
                from_floor: npc.floor,
            };
            let progress = smooth_progress(world, npc, fraction);
            pose.position = travel_target(world, npc, path, to, progress);
        }
        let key = appearance(npc);
        let tint = npc_tint(npc);
        let has_tool = npc.inventory.tool.is_some();
        let hurt = npc.health < world.params.hurt_below;

        let existing = index.entities.get(&npc.id).map(|&(e, _)| e);
        if let Some(entity) = existing
            && let Ok((mut sprite, mut anchor, mut visual, transform)) = sprites.get_mut(entity)
        {
            // Posizione, animazione e rotazione le aggiorna `move_npc_sprites`,
            // in base a dove si trova davvero lo sprite.
            let mut target = pose.position;
            if let Action::Travel { to } = npc.action {
                // Il viaggio parte da dove era lo sprite, non dal centro della
                // carrozza, e avanza in modo continuo tra un tick e l'altro.
                let since = npc.action_since.minutes();
                let path = match visual.travel {
                    Some(path) if path.since == since => path,
                    _ => TravelPath {
                        since,
                        from_x: transform.translation.x,
                        from_floor: npc.floor,
                    },
                };
                visual.travel = Some(path);
                target = travel_target(world, npc, path, to, smooth_progress(world, npc, fraction));
            } else if visual.travel.is_some() {
                visual.travel = None;
            }
            visual.target = target;
            visual.wants_lying = pose.lying;
            visual.action = npc.action;
            visual.has_tool = has_tool;
            visual.hurt = hurt;
            visual.face = pose.face;
            visual.z = z;
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
            hurt,
            face: pose.face,
            facing_left: facing_left(
                hash01(u64::from(npc.id.0) ^ 0xFACE) < 0.5,
                0.0,
                pose.position.x,
                pose.face,
            ),
            z,
            clock: phase,
            walked: 0.0,
            phase,
            frame: first,
            anim,
            // Entra in vista già in viaggio: il tragitto è quello di `travel_x`,
            // dal centro della carrozza di partenza.
            travel: matches!(npc.action, Action::Travel { .. }).then(|| TravelPath {
                since: npc.action_since.minutes(),
                from_x: TrainLayout::carriage_center_x(npc.carriage.index()),
                from_floor: npc.floor,
            }),
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

    // Via chi è uscito dalla finestra; chi è morto svanisce piano (chi è
    // stato ucciso prima cade a terra).
    index.entities.retain(|&id, &mut (entity, seen)| {
        if seen != frame {
            if world.npc(id).is_none() {
                let fade = if killed_recently(world, id) {
                    FadingOut::fallen()
                } else {
                    FadingOut::default()
                };
                commands
                    .entity(entity)
                    .remove::<(NpcSprite, NpcVisual)>()
                    .insert(fade);
            } else {
                commands.entity(entity).despawn();
            }
        }
        seen == frame
    });
}

/// Se `id` è stato appena ucciso (un `Killed` tra gli ultimi eventi).
fn killed_recently(world: &World, id: NpcId) -> bool {
    world.events.iter().rev().take(64).any(
        |e| matches!(e.kind, sim::EventKind::Killed { victim: Fighter::Npc(v), .. } if v == id),
    )
}

/// Muove gli sprite verso la loro posizione obiettivo, li fa sdraiare solo
/// una volta arrivati al letto (mentre ci vanno camminano in piedi) e sceglie
/// il fotogramma dell'animazione.
fn move_npc_sprites(
    time: Res<Time<Real>>,
    clock: Res<SimClock>,
    art: Res<CharacterArt>,
    layout: Res<TrainLayout>,
    mut sprites: Query<(&mut Transform, &mut Sprite, &mut NpcVisual), With<NpcSprite>>,
) {
    let dt = time.delta_secs();
    // In pausa i gesti si fermano (i passi seguono comunque lo spostamento).
    let anim_dt = if clock.paused { 0.0 } else { dt };
    for (mut transform, mut sprite, mut visual) in &mut sprites {
        let current = transform.translation.truncate();
        let delta = visual.target - current;
        let distance = delta.length();
        let traveling = visual.travel.is_some();
        let teleport = !traveling && distance > TELEPORT_DISTANCE;
        let next = if teleport {
            visual.target
        } else if traveling {
            // Il tragitto è già continuo: lo si segue esattamente in x
            // (a velocità alte è una corsa); in y si scende dal letto
            // camminando, o si segue la scala se si parte da un piano alto.
            if visual.travel.is_some_and(|path| path.from_floor > 0) {
                visual.target
            } else {
                let dy = delta.y.clamp(-WALK_SPEED * dt, WALK_SPEED * dt);
                Vec2::new(visual.target.x, current.y + dy)
            }
        } else {
            // Per cambiare piano si passa dalla scala.
            let floors = layout.floors((current.x / CARRIAGE_PITCH).floor().max(0.0) as usize);
            let stop = waypoint(
                current,
                visual.target,
                visual.standing_half_height(),
                floors,
            );
            let speed = if visual.hurt {
                WALK_SPEED * HURT_WALK
            } else {
                WALK_SPEED
            };
            let step = (speed + distance * CATCH_UP) * dt;
            current + (stop - current).clamp_length_max(step)
        };
        if next != current {
            transform.translation.x = next.x;
            transform.translation.y = next.y;
        }
        if transform.translation.z != visual.z {
            transform.translation.z = visual.z;
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
        let mut rotation = visual.rotation();
        // Ferito: zoppica, inclinandosi a ogni passo.
        if visual.hurt && lie <= 0.0 && next.distance(visual.target) > ARRIVE_DISTANCE {
            let limp = (visual.walked * 0.35).sin() * LIMP_TILT;
            rotation *= Quat::from_rotation_z(limp);
        }
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
    fn changing_floor_goes_through_the_stairs() {
        let half = 12.0;
        let ladder = stairs_center_x(1);
        let left = TrainLayout::carriage_left(1);
        let ground = FLOOR_Y + half;
        let upper = floor_y(1) + half;
        let bed = Vec2::new(left + 200.0, upper + 8.0);
        // Al piano terra, lontano dalla scala: prima la scala, allo stesso piano.
        let start = Vec2::new(left + 150.0, ground);
        assert_eq!(waypoint(start, bed, half, 2), Vec2::new(ladder, ground));
        // Ai piedi della scala: su fino al piano del letto.
        let foot = Vec2::new(ladder, ground);
        assert_eq!(waypoint(foot, bed, half, 2), Vec2::new(ladder, upper));
        // A metà scala si continua a salire anche se "sembra" già sopra.
        let mid = Vec2::new(ladder, floor_y(1) + 2.0);
        assert_eq!(waypoint(mid, bed, half, 2), Vec2::new(ladder, upper));
        // In cima: dritti al letto.
        assert_eq!(waypoint(Vec2::new(ladder, upper), bed, half, 2), bed);
        // Stesso piano, o carrozza a un piano solo: nessuna deviazione.
        assert_eq!(
            waypoint(start, Vec2::new(left + 20.0, ground), half, 2).x,
            left + 20.0
        );
        assert_eq!(waypoint(start, bed, half, 1), bed);
    }

    #[test]
    fn travel_from_upstairs_comes_down_the_ladder_first() {
        let mut world = World::generate(3, 8, 40);
        let dorm = world
            .carriages
            .iter()
            .find(|c| c.floors() > 1)
            .map(|c| c.id)
            .unwrap();
        let to = CarriageId(dorm.0 + 3);
        let npc = &mut world.npcs[0];
        npc.carriage = dorm;
        npc.floor = 1;
        npc.action = Action::Travel { to };
        npc.action_since = world.clock;
        npc.action_until = world.clock
            + 3 * world.params.travel_minutes_per_carriage
            + world.params.stairs_minutes;
        let npc = world.npcs[0].clone();
        let from_x = TrainLayout::carriage_left(dorm.index()) + 200.0;
        let path = TravelPath {
            since: npc.action_since.minutes(),
            from_x,
            from_floor: 1,
        };
        let at = |p: f32| travel_target(&world, &npc, path, to, p);
        let half = body_size(&npc).y / 2.0;
        let ladder = stairs_center_x(dorm.index());
        assert_eq!(at(0.0), Vec2::new(from_x, floor_y(1) + half));
        let total = npc.action_until.since(npc.action_since) as f32;
        let stairs = world.params.stairs_minutes as f32 / total;
        let top = at(stairs / 2.0);
        assert!((top.x - ladder).abs() < 1e-3 && (top.y - (floor_y(1) + half)).abs() < 1e-3);
        let bottom = at(stairs);
        assert!((bottom.x - ladder).abs() < 1e-3 && (bottom.y - (FLOOR_Y + half)).abs() < 1e-3);
        let end = at(1.0);
        assert_eq!(
            end,
            Vec2::new(TrainLayout::carriage_center_x(to.index()), FLOOR_Y + half)
        );
        // Mai sotto il piano terra né oltre il piano di partenza.
        for k in 0..=100 {
            let p = at(k as f32 / 100.0);
            assert!(p.y >= FLOOR_Y + half - 1e-3 && p.y <= floor_y(1) + half + 1e-3);
        }
    }

    #[test]
    fn travel_path_is_continuous_and_crosses_every_carriage() {
        // Un viaggio lungo tre carrozze, seguito fotogramma per fotogramma
        // a 60 fps e velocità ×1 (1 minuto di gioco al secondo).
        let mut world = World::generate(3, 8, 40);
        let mut brain = sim::UtilityBrain::new(3);
        let npc = loop {
            world.tick(&mut brain);
            if let Some(n) = world.npcs.iter().find(|n| match n.action {
                Action::Travel { to } => to.distance(n.carriage) >= 3,
                _ => false,
            }) {
                break n.id;
            }
            assert!(world.clock.minutes() < 5 * 1440, "nessun viaggio lungo");
        };
        let Action::Travel { to } = world.npc(npc).unwrap().action else {
            unreachable!()
        };
        let from_x = TrainLayout::carriage_center_x(world.npc(npc).unwrap().carriage.index());
        let dest_x = TrainLayout::carriage_center_x(to.index());
        let mut last = from_x;
        let mut fraction = 0.0_f32;
        let mut largest_step = 0.0_f32;
        while let Some(n) = world.npc(npc)
            && n.action == (Action::Travel { to })
        {
            let x = travel_path_x(from_x, to, smooth_progress(&world, n, fraction));
            largest_step = largest_step.max((x - last).abs());
            last = x;
            fraction += 1.0 / 60.0;
            if fraction >= 1.0 {
                fraction -= 1.0;
                world.tick(&mut brain);
            }
        }
        // Arriva al centro della carrozza di destinazione...
        assert!(
            (last - dest_x).abs() < 1.0,
            "fermo a {last}, atteso {dest_x}"
        );
        // ...senza mai saltare più di pochi pixel per fotogramma.
        let per_frame = CARRIAGE_PITCH / world.params.travel_minutes_per_carriage as f32 / 60.0;
        assert!(
            largest_step <= per_frame * 1.5,
            "passo massimo {largest_step}, atteso ≈ {per_frame}"
        );
    }

    #[test]
    fn travel_starts_where_the_sprite_was() {
        let to = CarriageId(4);
        assert_eq!(travel_path_x(123.0, to, 0.0), 123.0);
        assert_eq!(
            travel_path_x(123.0, to, 1.0),
            TrainLayout::carriage_center_x(4)
        );
    }

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

    /// Prima Mensa del treno di default, con i posti delle sue postazioni.
    fn mensa(world: &World) -> CarriageId {
        world
            .carriages
            .iter()
            .find(|c| c.kind == sim::CarriageKind::Mensa)
            .unwrap()
            .id
    }

    #[test]
    fn diners_sit_in_distinct_seats_behind_their_table() {
        let mut world = World::generate(42, 20, 400);
        let stations = StationLayout::from_world(&world);
        let m = mensa(&world);
        let (table, capacity) = world.carriages[m.index()]
            .stations
            .iter()
            .find(|s| s.kind == StationKind::Table)
            .map(|s| (s.id, s.capacity))
            .unwrap();
        let spot = *stations.spot(m, table).unwrap();
        let center = carriage_x(m, spot.x);
        world.npcs[0].carriage = m;
        world.npcs[0].age = 30;
        world.npcs[0].action = Action::Eat(table);
        let npc = &world.npcs[0];
        let poses: Vec<Pose> = (0..capacity)
            .map(|seat| npc_pose(&world, &stations, npc, seat))
            .collect();
        let floor = FLOOR_Y + body_size(npc).y / 2.0;
        for (seat, pose) in poses.iter().enumerate() {
            let x = pose.position.x;
            // Al tavolo (al più mezzo corpo oltre il bordo), rivolto al centro.
            assert!((x - center).abs() <= spot.width / 2.0 + body_size(npc).x / 2.0);
            assert_eq!(pose.face, Face::Toward(center));
            // Metà nella fila vicina, metà in quella lontana (più in alto, dietro).
            let far = seat >= usize::from(capacity.div_ceil(2));
            assert_eq!(pose.far, far, "{seat}");
            let rise = if far { FAR_ROW_RISE } else { 0.0 };
            assert!((pose.position.y - floor - rise).abs() < 1e-4);
            // Nessuno si siede esattamente dove sta un altro.
            for other in &poses[..seat] {
                assert!(other.position.distance(pose.position) > 1.0, "{seat}");
            }
        }
        // I posti oltre la capienza non esistono: si ricomincia.
        assert_eq!(npc_pose(&world, &stations, npc, capacity), poses[0]);
    }

    #[test]
    fn the_queue_lines_up_between_tables_and_stoves_facing_the_kitchen() {
        let mut world = World::generate(42, 20, 400);
        let stations = StationLayout::from_world(&world);
        let m = mensa(&world);
        let (left, right) = stations.queue(m).expect("a Mensa has a queue zone");
        let tables = stations.carriages[m.index()]
            .iter()
            .filter(|s| s.kind == StationKind::Table);
        assert!(tables.clone().all(|s| s.x + s.width / 2.0 <= left + 1e-3));
        world.npcs[0].carriage = m;
        world.npcs[0].age = 30;
        world.npcs[0].action = Action::Wait;
        let npc = &world.npcs[0];
        let half = body_size(npc).x / 2.0;
        let max = world.params.mensa_queue_max;
        let poses: Vec<Pose> = (0..max)
            .map(|k| npc_pose(&world, &stations, npc, k))
            .collect();
        for (k, pose) in poses.iter().enumerate() {
            let x = pose.position.x;
            assert!(x - half >= carriage_x(m, left) - 1e-3, "{k}: {x}");
            assert!(x + half <= carriage_x(m, right) + 1e-3, "{k}: {x}");
            assert_eq!(pose.face, Face::Right);
            for other in &poses[..k] {
                assert!(other.position.distance(pose.position) > 1.0, "{k}");
            }
        }
        // Il primo è il più vicino alle cucine, il secondo subito dietro.
        assert!(poses[0].position.x > poses[1].position.x);
        assert!(!poses[0].far);
    }

    #[test]
    fn idlers_in_a_mensa_stay_off_the_tables() {
        let mut world = World::generate(42, 20, 400);
        let stations = StationLayout::from_world(&world);
        let m = mensa(&world);
        let (left, right) = stations.lounge(m).expect("a Mensa has a lounge");
        let partner = world.npcs[1].id;
        for i in 0..40 {
            let npc = &mut world.npcs[i];
            npc.carriage = m;
            npc.age = 30;
            npc.action_since = sim::GameTime(i as u64 * 17);
            npc.action = if i % 2 == 0 {
                Action::Idle
            } else {
                Action::Socialize(partner)
            };
        }
        // Chi chiacchiera con chi mangia non gli sta addosso.
        world.npcs[1].action = Action::Eat(sim::StationId(0));
        for npc in world.npcs.iter().take(40).filter(|n| n.id != partner) {
            let x = npc_pose(&world, &stations, npc, 0).position.x;
            assert!(
                (carriage_x(m, left)..=carriage_x(m, right)).contains(&x),
                "{} fa {:?} a {x}",
                npc.name,
                npc.action
            );
        }
    }

    #[test]
    fn diners_keep_their_seat_when_a_neighbour_leaves() {
        let mut world = World::generate(42, 20, 400);
        let m = mensa(&world);
        let table = sim::StationId(0);
        for i in 0..3 {
            world.npcs[i].carriage = m;
            world.npcs[i].action = Action::Eat(table);
        }
        let (lo, hi) = (0, world.carriages.len() - 1);
        let mut seats = HashMap::new();
        let first = assign_slots(&world, lo, hi, &mut seats);
        let ids: Vec<NpcId> = world.npcs[..3].iter().map(|n| n.id).collect();
        let taken: HashSet<u16> = ids.iter().map(|id| first[id]).collect();
        assert_eq!(taken.len(), 3);
        // Il primo se ne va, poi arriva un altro: prende il posto libero.
        world.npcs[0].action = Action::Idle;
        let second = assign_slots(&world, lo, hi, &mut seats);
        assert_eq!(second[&ids[1]], first[&ids[1]]);
        assert_eq!(second[&ids[2]], first[&ids[2]]);
        world.npcs[3].carriage = m;
        world.npcs[3].action = Action::Eat(table);
        let third = assign_slots(&world, lo, hi, &mut seats);
        assert_eq!(third[&world.npcs[3].id], first[&ids[0]]);
    }

    #[test]
    fn poses_stay_inside_the_train_over_a_day() {
        let mut sim = crate::sim_bridge::new_sim();
        let stations = StationLayout::from_world(&sim.world);
        let layout = TrainLayout::from_world(&sim.world);
        let (min_x, max_x) = layout.inner_bounds();
        let mut seats = HashMap::new();
        for _ in 0..24 * 60 / 5 {
            for _ in 0..5 {
                sim.world.tick(&mut sim.brain);
            }
            let last = sim.world.carriages.len() - 1;
            let slots = assign_slots(&sim.world, 0, last, &mut seats);
            // Mai due persone sullo stesso posto di una postazione.
            let mut taken = HashSet::new();
            for npc in &sim.world.npcs {
                if let Some(s) = npc.action.station() {
                    assert!(taken.insert((npc.carriage, s, slots[&npc.id])), "{npc:?}");
                }
            }
            for npc in &sim.world.npcs {
                let seat = slots.get(&npc.id).copied().unwrap_or(0);
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
