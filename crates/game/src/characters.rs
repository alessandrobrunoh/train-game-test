//! Personaggi in pixel art: generatore "paper doll" di NPC e giocatore.
//!
//! Ogni NPC riceve un aspetto deterministico ([`AppearanceKey`]) dal suo id,
//! dal sesso, dalla fascia d'età, dal lavoro e dal fatto di avere o no dei
//! vestiti. L'id sceglie uno tra [`LOOKS`] "look" per sesso (pelle, capelli,
//! colore della camicia, gonna, barba, bastone...), così centinaia di NPC
//! condividono un numero limitato di texture: ogni chiave diventa un foglio
//! di [`FRAMES`] fotogrammi generato una volta sola e tenuto in cache
//! ([`CharacterArt`]).
//!
//! Il disegno è a strati: testa e capelli, cappello da cuoco e fagotto dei
//! neonati sono modelli ASCII (vedi `art.rs`); busto, braccia, gambe, abiti e
//! attrezzi sono tracciati in base alla posa di ogni fotogramma. Alla fine un
//! bordo scuro di un pixel contorna la figura. Tutto è disegnato guardando a
//! destra: per guardare a sinistra lo sprite viene specchiato.
//!
//! Le unità del mondo sono i pixel dell'arte (la camera ingrandisce 3×). Il
//! fotogramma è più grande dell'ingombro del corpo ([`Stage::size`]) per far
//! spazio a braccia, attrezzi e cappello; l'ancora dello sprite
//! ([`Stage::anchor`]) mette il centro del corpo sul `Transform`.

use std::collections::HashMap;

use bevy::prelude::*;
use sim::{Action, Job, LifeStage, Npc, NpcId, Sex};

use crate::art::{Canvas, Rgba};

// --- Geometria del fotogramma ----------------------------------------------------

/// Dimensioni di un fotogramma degli NPC (pixel = unità mondo).
pub const CELL: UVec2 = UVec2::new(20, 24);
/// Riga dei piedi (l'ultima sotto resta per il bordo).
const FEET: i32 = 22;
/// Colonna del centro del corpo: il confine tra le colonne 9 e 10, così lo
/// sprite specchiato resta allineato.
const MID: i32 = 10;

/// Look diversi per sesso: pochi, così gli NPC condividono le texture.
pub const LOOKS: u8 = 12;

/// Colore del bordo attorno ai personaggi.
const OUTLINE: Rgba = rgb(0x1c, 0x16, 0x1e);

const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
    [r, g, b, 255]
}

// --- Fasce d'età ------------------------------------------------------------------

/// Fascia d'età disegnata: decide proporzioni e taglia.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Stage {
    /// Sotto i 3 anni: in fasce.
    Baby,
    Child,
    Youth,
    Adult,
    /// Un po' curvo, capelli grigi, a volte col bastone.
    Elder,
}

/// Sotto quest'età si è un neonato in fasce.
pub const BABY_YEARS: u32 = 3;

/// Proporzioni (righe) di una figura in piedi.
struct Dims {
    torso: i32,
    legs: i32,
}

/// Altezza della testa di tutte le figure (i bambini hanno la testa grande).
const HEAD: i32 = 6;

impl Stage {
    #[cfg(test)]
    pub const ALL: [Stage; 5] = [
        Stage::Baby,
        Stage::Child,
        Stage::Youth,
        Stage::Adult,
        Stage::Elder,
    ];

    pub fn of_age(age: u32) -> Self {
        if age < BABY_YEARS {
            return Stage::Baby;
        }
        match LifeStage::of_age(age) {
            LifeStage::Bambino => Stage::Child,
            LifeStage::Giovane => Stage::Youth,
            LifeStage::Adulto => Stage::Adult,
            LifeStage::Anziano => Stage::Elder,
        }
    }

    fn dims(self) -> Dims {
        match self {
            Stage::Baby => Dims { torso: 0, legs: 0 },
            Stage::Child => Dims { torso: 3, legs: 3 },
            Stage::Youth => Dims { torso: 4, legs: 4 },
            Stage::Adult => Dims { torso: 5, legs: 5 },
            Stage::Elder => Dims { torso: 5, legs: 4 },
        }
    }

    /// Ingombro del corpo in piedi (larghezza, altezza): collisioni e click.
    pub fn size(self) -> Vec2 {
        match self {
            Stage::Baby => BABY_SIZE,
            Stage::Child => Vec2::new(6.0, 12.0),
            Stage::Youth => Vec2::new(7.0, 14.0),
            Stage::Adult => Vec2::new(8.0, 16.0),
            Stage::Elder => Vec2::new(8.0, 15.0),
        }
    }

    /// Ingombro da sdraiato (lunghezza, spessore), con la testa a sinistra.
    pub fn lying_size(self) -> Vec2 {
        match self {
            Stage::Baby => BABY_SIZE,
            _ => Vec2::new(self.size().y, LYING_THICKNESS as f32),
        }
    }

    /// Ancora dello sprite: il centro del corpo in piedi (e da sdraiato)
    /// dentro il fotogramma, in coordinate normalizzate di Bevy.
    pub fn anchor(self) -> Vec2 {
        let center_y = (FEET + 1) as f32 - self.size().y / 2.0;
        Vec2::new(0.0, (CELL.y as f32 / 2.0 - center_y) / CELL.y as f32)
    }

    /// Passo della camminata: pixel percorsi per fotogramma.
    pub fn stride(self) -> f32 {
        match self {
            Stage::Baby => 1.5,
            Stage::Child => 2.0,
            Stage::Youth | Stage::Elder => 2.5,
            Stage::Adult => 3.0,
        }
    }
}

const BABY_SIZE: Vec2 = Vec2::new(6.0, 5.0);
/// Spessore di chi dorme (testa di lato + coperta).
const LYING_THICKNESS: i32 = 6;

// --- Aspetto ----------------------------------------------------------------------

/// Tutto ciò che decide l'aspetto di un NPC: NPC con la stessa chiave
/// condividono la stessa texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AppearanceKey {
    pub stage: Stage,
    pub sex: Sex,
    /// Uno tra [`LOOKS`], scelto dall'id.
    pub look: u8,
    /// Lavoro (divisa e gesti al lavoro); solo per adulti e anziani.
    pub job: Option<Job>,
    /// Ha un Vestito: senza, stracci grigi e piedi scalzi.
    pub dressed: bool,
}

/// Numero pseudo-casuale da una chiave (splitmix64).
fn mix(key: u64) -> u64 {
    let mut z = key.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Look di un NPC, fisso per tutta la vita.
pub fn look_of(id: NpcId) -> u8 {
    (mix(u64::from(id.0) ^ 0x4C4F_4F4B) % u64::from(LOOKS)) as u8
}

/// Aspetto di un NPC con età, lavoro e vestiti attuali.
pub fn appearance(npc: &Npc) -> AppearanceKey {
    appearance_of(
        npc.id,
        npc.sex,
        npc.age,
        npc.job,
        npc.inventory.clothes.is_some(),
    )
}

pub fn appearance_of(
    id: NpcId,
    sex: Sex,
    age: u32,
    job: Option<Job>,
    dressed: bool,
) -> AppearanceKey {
    let stage = Stage::of_age(age);
    AppearanceKey {
        stage,
        sex,
        look: look_of(id),
        job: job.filter(|_| matches!(stage, Stage::Adult | Stage::Elder)),
        dressed,
    }
}

/// Tratti fisici e gusti di un look.
#[derive(Clone, Copy, Debug)]
struct Traits {
    skin: usize,
    hair_style: usize,
    hair_color: usize,
    shirt: usize,
    skirt: bool,
    beard: bool,
    cane: bool,
    blanket: usize,
}

fn traits(sex: Sex, look: u8) -> Traits {
    let h = mix(u64::from(look) | (sex as u64) << 8 | 0x5EED << 16);
    let pick = |shift: u32, n: usize| ((h >> shift) % n as u64) as usize;
    Traits {
        // La pelle ruota con il look: tutte le tonalità sono presenti.
        skin: usize::from(look) % SKINS.len(),
        hair_style: pick(8, hair_styles(sex).len()),
        hair_color: pick(16, HAIR_COLORS.len()),
        shirt: pick(24, 6),
        skirt: sex == Sex::Female && pick(32, 3) != 0,
        beard: sex == Sex::Male && pick(40, 3) == 0,
        cane: pick(48, 2) == 0,
        blanket: pick(56, BLANKETS.len()),
    }
}

// --- Palette ----------------------------------------------------------------------

/// (colore, ombra)
type Pair = (Rgba, Rgba);

const SKINS: [Pair; 4] = [
    (rgb(0xf1, 0xc8, 0xa4), rgb(0xd2, 0x9c, 0x7e)),
    (rgb(0xdc, 0xa7, 0x7a), rgb(0xb8, 0x82, 0x5c)),
    (rgb(0xb0, 0x76, 0x4e), rgb(0x8c, 0x5a, 0x3a)),
    (rgb(0x7c, 0x4e, 0x34), rgb(0x5e, 0x3a, 0x28)),
];
const EYE: Rgba = rgb(0x2a, 0x1c, 0x26);
const MOUTH: Rgba = rgb(0x8a, 0x34, 0x34);
/// Niente nero puro: si confonderebbe con il bordo.
const HAIR_COLORS: [Pair; 5] = [
    (rgb(0x44, 0x36, 0x3a), rgb(0x30, 0x26, 0x2c)),
    (rgb(0x6e, 0x46, 0x2a), rgb(0x52, 0x32, 0x1e)),
    (rgb(0x9a, 0x62, 0x34), rgb(0x76, 0x48, 0x26)),
    (rgb(0xe0, 0xbc, 0x6a), rgb(0xb8, 0x90, 0x4a)),
    (rgb(0xb0, 0x4a, 0x26), rgb(0x86, 0x36, 0x1c)),
];
const GREY_HAIR: [Pair; 2] = [
    (rgb(0xc8, 0xc8, 0xc6), rgb(0x9a, 0x9a, 0x9e)),
    (rgb(0xee, 0xec, 0xe6), rgb(0xc0, 0xbe, 0xba)),
];
/// Camicie di chi non ha una divisa (adulti), dei bambini e degli anziani.
const ADULT_SHIRTS: [Pair; 6] = [
    (rgb(0x96, 0x68, 0xa8), rgb(0x74, 0x4e, 0x86)),
    (rgb(0x7c, 0x7c, 0x94), rgb(0x5e, 0x5e, 0x74)),
    (rgb(0xa8, 0x5e, 0x54), rgb(0x84, 0x46, 0x40)),
    (rgb(0x5e, 0x86, 0x7a), rgb(0x46, 0x68, 0x5e)),
    (rgb(0x8c, 0x74, 0x58), rgb(0x6c, 0x58, 0x42)),
    (rgb(0x62, 0x70, 0x9a), rgb(0x4a, 0x54, 0x78)),
];
const CHILD_SHIRTS: [Pair; 6] = [
    (rgb(0xec, 0x8c, 0xaa), rgb(0xc4, 0x6a, 0x88)),
    (rgb(0x78, 0xb4, 0xe6), rgb(0x58, 0x8e, 0xbe)),
    (rgb(0xec, 0xc8, 0x5a), rgb(0xc4, 0x9e, 0x40)),
    (rgb(0xe8, 0x8c, 0x46), rgb(0xc0, 0x6a, 0x30)),
    (rgb(0x78, 0xc8, 0x96), rgb(0x56, 0xa0, 0x74)),
    (rgb(0xd2, 0x50, 0x50), rgb(0xa8, 0x3a, 0x3a)),
];
const ELDER_SHIRTS: [Pair; 6] = [
    (rgb(0xaa, 0xa0, 0x8c), rgb(0x86, 0x7e, 0x6c)),
    (rgb(0x82, 0x6e, 0x64), rgb(0x64, 0x54, 0x4c)),
    (rgb(0x78, 0x82, 0x96), rgb(0x5c, 0x64, 0x76)),
    (rgb(0x96, 0x78, 0x8c), rgb(0x74, 0x5c, 0x6c)),
    (rgb(0x86, 0x8c, 0x6a), rgb(0x68, 0x6e, 0x50)),
    (rgb(0xa0, 0x84, 0x64), rgb(0x7e, 0x66, 0x4c)),
];
const PANTS: Pair = (rgb(0x4a, 0x44, 0x54), rgb(0x36, 0x32, 0x40));
const CHILD_PANTS: Pair = (rgb(0x5a, 0x5e, 0x86), rgb(0x44, 0x46, 0x68));
const SHOES: Rgba = rgb(0x3c, 0x2c, 0x24);
const BELT: Rgba = rgb(0x3a, 0x2a, 0x22);

const FARMER_SHIRT: Pair = (rgb(0xe2, 0xd0, 0xa0), rgb(0xbe, 0xaa, 0x80));
const FARMER_OVERALLS: Pair = (rgb(0x56, 0x96, 0x3e), rgb(0x3e, 0x72, 0x2e));
const COOK_SHIRT: Pair = (rgb(0xc4, 0xd2, 0xde), rgb(0x9a, 0xaa, 0xba));
const COOK_WHITE: Pair = (rgb(0xf8, 0xf8, 0xf4), rgb(0xcc, 0xcc, 0xd0));
const COOK_PANTS: Pair = (rgb(0x3e, 0x3e, 0x48), rgb(0x2e, 0x2e, 0x36));
const WORKER_SHIRT: Pair = (rgb(0xc8, 0xc8, 0xc4), rgb(0xa0, 0xa0, 0xa0));
const WORKER_OVERALLS: Pair = (rgb(0x46, 0x70, 0xc4), rgb(0x32, 0x52, 0x96));
const MERCHANT_SHIRT: Pair = (rgb(0xec, 0xe2, 0xc8), rgb(0xc4, 0xb8, 0x9e));
const MERCHANT_VEST: Pair = (rgb(0xd6, 0xa0, 0x34), rgb(0xa8, 0x78, 0x24));
const MERCHANT_PANTS: Pair = (rgb(0x6e, 0x4c, 0x34), rgb(0x54, 0x38, 0x26));
/// Stracci di chi non ha vestiti: grigi, appena tinti del colore del lavoro.
const RAGS: Pair = (rgb(0x9a, 0x94, 0x8a), rgb(0x76, 0x72, 0x6a));
const BLANKETS: [Pair; 4] = [
    (rgb(0x8a, 0x64, 0x5e), rgb(0x6a, 0x4a, 0x46)),
    (rgb(0x5e, 0x6e, 0x8c), rgb(0x46, 0x52, 0x6c)),
    (rgb(0x6e, 0x7a, 0x5a), rgb(0x52, 0x5c, 0x42)),
    (rgb(0x9a, 0x7a, 0x4e), rgb(0x78, 0x5c, 0x3a)),
];
const SHEET: Rgba = rgb(0xe6, 0xe2, 0xd6);
const WOOD: Pair = (rgb(0x96, 0x64, 0x38), rgb(0x6e, 0x46, 0x26));
const METAL: Pair = (rgb(0xb4, 0xba, 0xc4), rgb(0x78, 0x7e, 0x8a));
const COIN: Rgba = rgb(0xf4, 0xd2, 0x46);

/// Mescola due colori (`t` = quanto di `b`).
fn blend(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2]), 255]
}

fn job_color(job: Job) -> Rgba {
    match job {
        Job::Contadino => FARMER_OVERALLS.0,
        Job::Cuoco => COOK_WHITE.0,
        Job::Operaio => WORKER_OVERALLS.0,
        Job::Mercante => MERCHANT_VEST.0,
    }
}

/// Cosa si indossa sopra la camicia.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Garment {
    Plain,
    /// Salopette: pettorina e pantaloni del colore `accent`.
    Overalls,
    /// Grembiule bianco (cuochi).
    Apron,
    /// Gilet aperto (mercanti).
    Vest,
    /// Canottiera strappata, braccia nude, piedi scalzi.
    Rags,
}

/// Colori e capi di un aspetto.
#[derive(Clone, Copy, Debug)]
struct Paint {
    skin: Pair,
    hair: Pair,
    shirt: Pair,
    pants: Pair,
    shoes: Rgba,
    accent: Pair,
    blanket: Pair,
    garment: Garment,
    skirt: bool,
    toque: bool,
    beard: bool,
    cane: bool,
    belt: bool,
    hair_style: usize,
}

fn paint(key: &AppearanceKey) -> Paint {
    let t = traits(key.sex, key.look);
    let hair = if key.stage == Stage::Elder {
        GREY_HAIR[t.hair_color % GREY_HAIR.len()]
    } else {
        HAIR_COLORS[t.hair_color]
    };
    let young = matches!(key.stage, Stage::Baby | Stage::Child | Stage::Youth);
    let mut p = Paint {
        skin: SKINS[t.skin],
        hair,
        shirt: match key.stage {
            Stage::Elder => ELDER_SHIRTS[t.shirt],
            _ if young => CHILD_SHIRTS[t.shirt],
            _ => ADULT_SHIRTS[t.shirt],
        },
        pants: if young { CHILD_PANTS } else { PANTS },
        shoes: SHOES,
        accent: PANTS,
        blanket: BLANKETS[t.blanket],
        garment: Garment::Plain,
        skirt: t.skirt,
        toque: false,
        beard: t.beard && matches!(key.stage, Stage::Adult | Stage::Elder),
        cane: t.cane && key.stage == Stage::Elder,
        belt: !young,
        hair_style: t.hair_style,
    };
    // Vestito intero (gonna dello stesso colore della camicia) per le donne
    // senza divisa.
    if p.skirt {
        p.pants = p.shirt;
    }
    match key.job {
        Some(Job::Contadino) => {
            p.shirt = FARMER_SHIRT;
            p.pants = FARMER_OVERALLS;
            p.accent = FARMER_OVERALLS;
            p.garment = Garment::Overalls;
            p.skirt = false;
        }
        Some(Job::Operaio) => {
            p.shirt = WORKER_SHIRT;
            p.pants = WORKER_OVERALLS;
            p.accent = WORKER_OVERALLS;
            p.garment = Garment::Overalls;
            p.skirt = false;
        }
        Some(Job::Cuoco) => {
            p.shirt = COOK_SHIRT;
            p.pants = COOK_PANTS;
            p.accent = COOK_WHITE;
            p.garment = Garment::Apron;
            p.toque = true;
        }
        Some(Job::Mercante) => {
            p.shirt = MERCHANT_SHIRT;
            p.accent = MERCHANT_VEST;
            p.pants = MERCHANT_PANTS;
            p.garment = Garment::Vest;
        }
        None => {}
    }
    if p.garment != Garment::Plain {
        p.belt = false;
    }
    if !key.dressed {
        let tint = key.job.map_or(RAGS.0, job_color);
        let rags = (blend(RAGS.0, tint, 0.2), blend(RAGS.1, tint, 0.15));
        p.shirt = rags;
        p.pants = (rags.1, blend(rags.1, OUTLINE, 0.2));
        p.accent = rags;
        p.garment = Garment::Rags;
        p.skirt = false;
        p.toque = false;
        p.belt = false;
        p.shoes = p.skin.1;
        // Fasce grigie per i neonati.
        p.blanket = RAGS;
    }
    p
}

// --- Modelli ASCII -------------------------------------------------------------------

/// Testa (6×6) vista di tre quarti verso destra: `s` pelle, `S` ombra, `e` occhio.
const HEAD_ROWS: [&str; 6] = [
    ".ssss.", //
    "ssssss", //
    "ssssss", //
    "sSsses", //
    "ssssss", //
    ".sssS.", //
];

/// Capelli (8×10): la testa occupa colonne 1..=6 e righe 1..=6.
/// `h` capelli, `H` ombra.
const MALE_HAIR: [&[&str]; 5] = [
    // corti
    &[
        "........", //
        "..hhhh..", //
        ".hhhhhhh", //
        ".hHhhh..", //
        ".hH.....", //
        ".h......", //
    ],
    // rasati
    &[
        "........", //
        "..HhhH..", //
        ".Hhhhhh.", //
        ".hH.....", //
        ".H......", //
    ],
    // spettinati
    &[
        "..h.h...", //
        ".hhhhhh.", //
        ".hhhhhhh", //
        ".hHhhh.h", //
        ".hhH....", //
        ".hh.....", //
    ],
    // stempiato
    &[
        "........", //
        "........", //
        "..HH....", //
        ".hH.....", //
        ".hH.....", //
        ".h......", //
    ],
    // riga di lato
    &[
        "........", //
        "..hhhhh.", //
        ".hhhhhhh", //
        ".hHh...h", //
        ".hH.....", //
        ".H......", //
    ],
];

const FEMALE_HAIR: [&[&str]; 4] = [
    // lunghi sciolti
    &[
        "........", //
        "..hhhh..", //
        ".hhhhhhh", //
        "hhHhhh..", //
        "hhH.....", //
        "hhH.....", //
        "hhH.....", //
        "hH......", //
        "hH......", //
    ],
    // coda di cavallo
    &[
        "........", //
        "..hhhh..", //
        ".hhhhhhh", //
        "hhHhhh..", //
        "hhH.....", //
        "h.H.....", //
        "h.......", //
        "H.......", //
    ],
    // chignon
    &[
        "hH......", //
        "hhhhhh..", //
        ".hhhhhhh", //
        ".hHhhh..", //
        ".hH.....", //
        ".hH.....", //
    ],
    // caschetto
    &[
        "........", //
        "..hhhhh.", //
        ".hhhhhhh", //
        ".hHhhh.h", //
        ".hHh....", //
        ".hHh....", //
        ".hh.....", //
    ],
];

fn hair_styles(sex: Sex) -> &'static [&'static [&'static str]] {
    match sex {
        Sex::Male => &MALE_HAIR,
        Sex::Female => &FEMALE_HAIR,
    }
}

/// Barba (stessa griglia dei capelli).
const BEARD_ROWS: [&str; 8] = [
    "........", //
    "........", //
    "........", //
    "........", //
    "........", //
    "......H.", //
    "...hhhhh", //
    "....hhh.", //
];

/// Cappello da cuoco (8 di larghezza, poggia sulla testa): `w` bianco, `W` ombra.
const TOQUE_ROWS: [&str; 5] = [
    "..www...", //
    ".wwwww..", //
    ".wwwwwW.", //
    "..wwwW..", //
    ".WWWWWW.", //
];

/// Neonato in fasce (6×5): `w` fasce, `W` ombra, `h` ciuffo.
const BABY_ROWS: [&str; 5] = [
    "..wwh.", //
    ".wssss", //
    "wwsses", //
    "wwWsss", //
    ".wwwW.", //
];

/// Caratteri ammessi nei modelli ASCII (oltre a `.` e spazio).
#[cfg(test)]
const TEMPLATE_CHARS: &str = "sSehHwW";

// --- Pose -------------------------------------------------------------------------

/// Fotogrammi del foglio di un NPC, nell'ordine dell'atlante.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Frame {
    Idle0,
    Idle1,
    Walk0,
    Walk1,
    Walk2,
    Walk3,
    Eat0,
    Eat1,
    Work0,
    Work1,
    Tool0,
    Tool1,
    Talk0,
    Talk1,
    Buy0,
    Sleep,
}

pub const FRAMES: usize = 16;

impl Frame {
    pub const ALL: [Frame; FRAMES] = [
        Frame::Idle0,
        Frame::Idle1,
        Frame::Walk0,
        Frame::Walk1,
        Frame::Walk2,
        Frame::Walk3,
        Frame::Eat0,
        Frame::Eat1,
        Frame::Work0,
        Frame::Work1,
        Frame::Tool0,
        Frame::Tool1,
        Frame::Talk0,
        Frame::Talk1,
        Frame::Buy0,
        Frame::Sleep,
    ];

    pub fn index(self) -> usize {
        self as usize
    }
}

/// Dove sta la mano rispetto alla spalla (per un braccio lungo 4 pixel).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Arm {
    To(i32, i32),
    /// Alla bocca.
    Mouth,
}

const DOWN: Arm = Arm::To(0, 4);
const SWING_FWD: Arm = Arm::To(2, 3);
const SWING_BACK: Arm = Arm::To(-2, 3);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Legs {
    Stand,
    /// Scarto orizzontale del piede (vicino, lontano) e piede sollevato.
    Step {
        near: i32,
        far: i32,
        lift_near: bool,
        lift_far: bool,
    },
    Seated,
}

/// Oggetto in mano.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Held {
    None,
    HoeUp,
    HoeDown,
    HammerUp,
    HammerDown,
    Ladle,
    Coin,
    Cane,
}

#[derive(Clone, Copy, Debug)]
struct Pose {
    /// Scarto verticale di busto e testa (negativo = in su).
    bob: i32,
    /// Busto e testa sporti in avanti.
    lean: i32,
    near: Arm,
    far: Arm,
    legs: Legs,
    held: Held,
    mouth: bool,
}

impl Default for Pose {
    fn default() -> Self {
        Self {
            bob: 0,
            lean: 0,
            near: DOWN,
            far: DOWN,
            legs: Legs::Stand,
            held: Held::None,
            mouth: false,
        }
    }
}

fn step(near: i32, far: i32, lift_near: bool, lift_far: bool) -> Legs {
    Legs::Step {
        near,
        far,
        lift_near,
        lift_far,
    }
}

fn pose(key: &AppearanceKey, cane: bool, frame: Frame) -> Pose {
    let base = Pose::default();
    let stride = if key.stage == Stage::Child { 1 } else { 2 };
    let mut p = match frame {
        Frame::Idle0 | Frame::Sleep => base,
        // Respiro: spalle e testa scendono di un pixel.
        Frame::Idle1 => Pose { bob: 1, ..base },
        Frame::Walk0 => Pose {
            near: SWING_BACK,
            far: SWING_FWD,
            legs: step(stride, -stride, false, false),
            ..base
        },
        Frame::Walk1 => Pose {
            bob: -1,
            legs: step(0, 0, false, true),
            ..base
        },
        Frame::Walk2 => Pose {
            near: SWING_FWD,
            far: SWING_BACK,
            legs: step(-stride, stride, false, false),
            ..base
        },
        Frame::Walk3 => Pose {
            bob: -1,
            legs: step(0, 0, true, false),
            ..base
        },
        Frame::Eat0 => Pose {
            near: Arm::To(4, 1),
            legs: Legs::Seated,
            ..base
        },
        Frame::Eat1 => Pose {
            near: Arm::Mouth,
            legs: Legs::Seated,
            ..base
        },
        Frame::Work0 | Frame::Work1 | Frame::Tool0 | Frame::Tool1 => work_pose(key.job, frame),
        Frame::Talk0 => Pose {
            near: Arm::To(3, -1),
            mouth: true,
            ..base
        },
        Frame::Talk1 => Pose {
            near: Arm::To(3, 2),
            far: Arm::To(1, 3),
            ..base
        },
        Frame::Buy0 => Pose {
            near: Arm::To(4, 0),
            held: Held::Coin,
            ..base
        },
    };
    // Gli anziani col bastone lo tengono sempre (tranne quando la mano serve).
    let hand_free = matches!(
        frame,
        Frame::Idle0 | Frame::Idle1 | Frame::Walk0 | Frame::Walk1 | Frame::Walk2 | Frame::Walk3
    );
    if cane && hand_free {
        p.near = Arm::To(2, 3);
        p.held = Held::Cane;
    }
    p
}

/// Gesti al lavoro, per mestiere; `Tool*` sono con l'attrezzo in mano.
fn work_pose(job: Option<Job>, frame: Frame) -> Pose {
    let base = Pose::default();
    let second = matches!(frame, Frame::Work1 | Frame::Tool1);
    let tool = matches!(frame, Frame::Tool0 | Frame::Tool1);
    match job {
        Some(Job::Contadino) if tool => {
            if second {
                Pose {
                    lean: 1,
                    near: Arm::To(3, 2),
                    far: Arm::To(2, 2),
                    held: Held::HoeDown,
                    ..base
                }
            } else {
                Pose {
                    near: Arm::To(3, -1),
                    far: Arm::To(2, 0),
                    held: Held::HoeUp,
                    ..base
                }
            }
        }
        Some(Job::Contadino) => Pose {
            lean: 1,
            near: if second { Arm::To(2, 3) } else { Arm::To(3, 4) },
            far: if second { Arm::To(3, 4) } else { Arm::To(2, 3) },
            ..base
        },
        Some(Job::Operaio) if tool => {
            if second {
                Pose {
                    near: Arm::To(4, 1),
                    held: Held::HammerDown,
                    ..base
                }
            } else {
                Pose {
                    near: Arm::To(3, -3),
                    held: Held::HammerUp,
                    ..base
                }
            }
        }
        Some(Job::Operaio) => Pose {
            near: Arm::To(4, if second { 2 } else { 1 }),
            far: Arm::To(3, if second { 1 } else { 2 }),
            ..base
        },
        Some(Job::Cuoco) => Pose {
            near: if second { Arm::To(3, 2) } else { Arm::To(4, 1) },
            held: Held::Ladle,
            ..base
        },
        Some(Job::Mercante) => Pose {
            near: if second {
                Arm::To(4, 0)
            } else {
                Arm::To(3, -1)
            },
            held: if second { Held::Coin } else { Held::None },
            mouth: !second,
            ..base
        },
        None => base,
    }
}

// --- Disegno ----------------------------------------------------------------------

fn put(c: &mut Canvas, x: i32, y: i32, color: Rgba) {
    if x >= 0 && y >= 0 {
        c.set(x as u32, y as u32, color);
    }
}

/// Punti di un segmento (Bresenham), estremi inclusi.
fn line_points(a: IVec2, b: IVec2) -> Vec<IVec2> {
    let mut points = Vec::new();
    let (mut x, mut y) = (a.x, a.y);
    let dx = (b.x - a.x).abs();
    let dy = -(b.y - a.y).abs();
    let sx = if a.x < b.x { 1 } else { -1 };
    let sy = if a.y < b.y { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        points.push(IVec2::new(x, y));
        if x == b.x && y == b.y {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
    points
}

fn line(c: &mut Canvas, a: IVec2, b: IVec2, color: Rgba) {
    for p in line_points(a, b) {
        put(c, p.x, p.y, color);
    }
}

fn template(rows: &[&str], palette: &[(char, Rgba)]) -> Canvas {
    Canvas::from_rows(rows, palette)
}

/// Braccio dalla spalla alla mano: manica e poi la mano. Restituisce la mano.
fn draw_arm(c: &mut Canvas, shoulder: IVec2, hand: IVec2, sleeve: Rgba, skin: Rgba) -> IVec2 {
    let points = line_points(shoulder, hand);
    let last = points.len() - 1;
    for (i, p) in points.iter().enumerate() {
        put(c, p.x, p.y, if i == last { skin } else { sleeve });
    }
    hand
}

/// Gamba larga 2 pixel dall'anca al piede, con la scarpa che sporge in avanti.
/// Le prime `upper` righe sono del colore `cloth`, poi `bare` (gambe scoperte).
#[allow(clippy::too_many_arguments)]
fn draw_leg(
    c: &mut Canvas,
    hip: IVec2,
    foot_dx: i32,
    foot_y: i32,
    cloth: Rgba,
    bare: Rgba,
    upper: i32,
    shoe: Rgba,
) {
    let rows = foot_y - hip.y + 1;
    for r in 0..rows {
        let x = if rows > 1 {
            hip.x + ((foot_dx * r) as f32 / (rows - 1) as f32).round() as i32
        } else {
            hip.x + foot_dx
        };
        let y = hip.y + r;
        if r == rows - 1 {
            for dx in 0..3 {
                put(c, x + dx, y, shoe);
            }
        } else {
            let color = if r < upper { cloth } else { bare };
            put(c, x, y, color);
            put(c, x + 1, y, color);
        }
    }
}

fn head_palette(p: &Paint) -> [(char, Rgba); 3] {
    [('s', p.skin.0), ('S', p.skin.1), ('e', EYE)]
}

fn hair_palette(p: &Paint) -> [(char, Rgba); 2] {
    [('h', p.hair.0), ('H', p.hair.1)]
}

/// Testa, capelli, barba e cappello (8×10, la testa in 1..=6).
fn draw_head(p: &Paint, sex: Sex, eyes_closed: bool, hat: bool) -> Canvas {
    let mut head = Canvas::new(8, 10);
    let mut face = template(&HEAD_ROWS, &head_palette(p));
    if eyes_closed {
        face.recolor(EYE, p.skin.1);
    }
    head.overlay(&face, 1, 1);
    let styles = hair_styles(sex);
    let style = styles[p.hair_style % styles.len()];
    head.overlay(&template(style, &hair_palette(p)), 0, 0);
    if p.beard {
        head.overlay(&template(&BEARD_ROWS, &hair_palette(p)), 0, 0);
    }
    if hat && p.toque {
        // Il cappello copre la cima dei capelli.
        let toque = template(&TOQUE_ROWS, &[('w', COOK_WHITE.0), ('W', COOK_WHITE.1)]);
        let mut out = Canvas::new(8, 14);
        out.overlay(&head, 0, 4);
        out.overlay(&toque, 0, 0);
        return out;
    }
    head
}

/// Figura in piedi, seduta o al lavoro (tutte le fasce tranne i neonati).
fn draw_figure(c: &mut Canvas, key: &AppearanceKey, p: &Paint, pose: &Pose) {
    let d = key.stage.dims();
    let height = HEAD + d.torso + d.legs;
    let seated = pose.legs == Legs::Seated;
    // Seduti sulla panca dietro al tavolo (alto come un bancone): i fianchi
    // restano all'altezza della panca e i più piccoli si alzano, così la
    // testa di tutti spunta sopra il piano.
    let drop = if seated {
        height - Stage::Adult.size().y as i32
    } else {
        0
    };
    let top = FEET - height + 1 + pose.bob + drop;
    let torso_top = top + HEAD;
    let hip_y = torso_top + d.torso;
    let stoop = i32::from(key.stage == Stage::Elder);
    let lean = pose.lean;
    // Adulti e anziani hanno le spalle più larghe.
    let tw = if matches!(key.stage, Stage::Adult | Stage::Elder) {
        5
    } else {
        4
    };
    let tx0 = MID + 2 - tw + lean;
    let steps = d.torso - 1;
    let scale = |v: i32| (v as f32 * steps as f32 / 4.0).round() as i32;

    let rags = p.garment == Garment::Rags;
    let (sleeve, sleeve_far) = if rags {
        (p.skin.0, p.skin.1)
    } else {
        (p.shirt.1, blend(p.shirt.1, OUTLINE, 0.25))
    };
    let near_shoulder = IVec2::new(MID + lean, torso_top);
    let far_shoulder = IVec2::new(MID - 1 + lean, torso_top);
    let head_x = MID - 3 + lean + stoop;
    let head_y = top;
    let hand_at = |arm: Arm, shoulder: IVec2| match arm {
        Arm::To(x, y) => shoulder + IVec2::new(scale(x), scale(y)),
        Arm::Mouth => IVec2::new(head_x + 4, head_y + 4),
    };

    // Braccio lontano, dietro il busto.
    draw_arm(
        c,
        far_shoulder,
        hand_at(pose.far, far_shoulder),
        sleeve_far,
        p.skin.1,
    );

    // Gambe.
    let (cloth, cloth_far) = p.pants;
    let (bare, bare_far) = p.skin;
    let upper = if rags {
        d.legs / 2
    } else if p.skirt {
        0
    } else {
        d.legs
    };
    let near_hip = IVec2::new(MID, hip_y);
    let far_hip = IVec2::new(MID - 2, hip_y);
    match pose.legs {
        Legs::Seated => {
            // Coscia in avanti, stinco giù; la gamba lontana è nascosta.
            let thigh = if p.skirt { p.pants.0 } else { cloth };
            for x in MID - 2..=MID + 2 {
                put(c, x, hip_y, thigh);
            }
            let shin = if p.skirt || rags { bare } else { cloth };
            for y in hip_y + 1..FEET {
                put(c, MID + 1, y, shin);
                put(c, MID + 2, y, shin);
            }
            for x in MID + 1..=MID + 3 {
                put(c, x, FEET, p.shoes);
            }
        }
        Legs::Stand | Legs::Step { .. } => {
            let (near_dx, far_dx, lift_near, lift_far) = match pose.legs {
                Legs::Step {
                    near,
                    far,
                    lift_near,
                    lift_far,
                } => (near, far, lift_near, lift_far),
                _ => (0, 0, false, false),
            };
            let far_shoe = blend(p.shoes, OUTLINE, 0.3);
            draw_leg(
                c,
                far_hip,
                far_dx,
                FEET - i32::from(lift_far),
                cloth_far,
                bare_far,
                upper,
                far_shoe,
            );
            draw_leg(
                c,
                near_hip,
                near_dx,
                FEET - i32::from(lift_near),
                cloth,
                bare,
                upper,
                p.shoes,
            );
        }
    }

    // Busto: la colonna posteriore in ombra.
    for y in 0..d.torso {
        for x in 0..tw {
            let color = if x == 0 { p.shirt.1 } else { p.shirt.0 };
            put(c, tx0 + x, torso_top + y, color);
        }
    }
    // Schiena curva degli anziani.
    if stoop == 1 {
        put(c, tx0 - 1, torso_top + 1, p.shirt.1);
        put(c, tx0, torso_top - 1, p.shirt.1);
    }
    match p.garment {
        Garment::Overalls => {
            for y in 1..d.torso {
                for x in 1..tw {
                    put(c, tx0 + x, torso_top + y, p.accent.0);
                }
            }
            put(c, tx0, torso_top + d.torso - 1, p.accent.1);
            // Bretella.
            put(c, tx0 + 1, torso_top, p.accent.1);
        }
        Garment::Apron => {
            for y in 1..d.torso {
                for x in tw - 2..tw + 1 {
                    put(c, tx0 + x, torso_top + y, p.accent.0);
                }
            }
            // Laccio in vita.
            put(c, tx0 + tw - 3, torso_top + d.torso - 2, p.accent.1);
        }
        Garment::Vest => {
            for y in 0..d.torso {
                put(c, tx0, torso_top + y, p.accent.1);
                put(c, tx0 + 1, torso_top + y, p.accent.0);
                if y > 0 {
                    put(c, tx0 + tw - 1, torso_top + y, p.accent.0);
                }
            }
        }
        Garment::Rags => {
            // Strappi: un buco e l'orlo sfilacciato.
            put(c, tx0 + 2, torso_top + d.torso / 2, p.skin.1);
            put(c, tx0 + tw - 1, torso_top + d.torso - 1, p.skin.0);
            put(c, tx0 + 1, torso_top, p.skin.0);
        }
        Garment::Plain => {}
    }
    if p.belt && !p.skirt {
        for x in 0..tw {
            put(c, tx0 + x, hip_y - 1, BELT);
        }
    }
    // Gonna: si allarga verso l'orlo.
    if p.skirt && !seated {
        let len = (d.legs - 2).max(1);
        for r in 0..len {
            let (from, to) = if r == 0 { (-2, 1) } else { (-3, 2) };
            for x in from..=to {
                let color = if x == from { p.pants.1 } else { p.pants.0 };
                put(c, MID + x, hip_y + r, color);
            }
        }
    }
    // Il grembiule scende sulle cosce.
    if p.garment == Garment::Apron && !seated {
        for r in 0..(d.legs - 2).max(1) {
            for x in 0..3 {
                put(c, MID + x, hip_y + r, p.accent.0);
            }
            put(c, MID - 1, hip_y + r, p.accent.1);
        }
    }

    // Testa e capelli (con il cappello del cuoco).
    let head = draw_head(p, key.sex, false, true);
    let extra = head.height as i32 - 10;
    c.overlay(&head, head_x - 1, head_y - 1 - extra);
    if pose.mouth {
        put(c, head_x + 4, head_y + 5, MOUTH);
    }

    // Braccio vicino e oggetto in mano.
    let near_hand = hand_at(pose.near, near_shoulder);
    let hand = draw_arm(c, near_shoulder, near_hand, sleeve, p.skin.0);
    draw_held(c, pose.held, hand);
}

fn draw_held(c: &mut Canvas, held: Held, hand: IVec2) {
    let at = |dx: i32, dy: i32| hand + IVec2::new(dx, dy);
    match held {
        Held::None => {}
        Held::Coin => put(c, hand.x + 1, hand.y, COIN),
        Held::Cane => line(c, at(0, 1), IVec2::new(hand.x, FEET), WOOD.1),
        Held::Ladle => {
            line(c, at(0, 1), at(0, 3), METAL.1);
            put(c, hand.x + 1, hand.y + 3, METAL.0);
        }
        Held::HoeUp => {
            // Zappa alzata davanti al viso, lama in alto che punta avanti.
            line(c, at(-1, 2), at(1, -4), WOOD.0);
            put(c, hand.x + 2, hand.y - 4, METAL.0);
            put(c, hand.x + 3, hand.y - 4, METAL.0);
            put(c, hand.x + 3, hand.y - 3, METAL.1);
        }
        Held::HoeDown => {
            // Colpo: la lama affonda nella terra davanti ai piedi.
            line(c, at(-2, -2), at(2, 3), WOOD.0);
            put(c, hand.x + 2, hand.y + 4, METAL.0);
            put(c, hand.x + 1, hand.y + 4, METAL.1);
        }
        Held::HammerUp => {
            line(c, at(0, -1), at(0, -2), WOOD.0);
            for dx in -1..=1 {
                put(c, hand.x + dx, hand.y - 3, METAL.0);
            }
            put(c, hand.x - 1, hand.y - 4, METAL.1);
        }
        Held::HammerDown => {
            put(c, hand.x + 1, hand.y, WOOD.0);
            for dy in -1..=1 {
                put(c, hand.x + 2, hand.y + dy, METAL.0);
            }
            put(c, hand.x + 3, hand.y - 1, METAL.1);
        }
    }
}

/// Riga del centro del corpo nel fotogramma (per chi dorme).
fn center_row(stage: Stage) -> i32 {
    ((FEET + 1) as f32 - stage.size().y / 2.0).floor() as i32
}

/// Chi dorme: testa sul cuscino a sinistra (a pancia in su), coperta sul resto.
fn draw_lying(c: &mut Canvas, key: &AppearanceKey, p: &Paint) {
    let length = key.stage.size().y as i32;
    let x0 = MID - length / 2;
    let top = center_row(key.stage) - LYING_THICKNESS / 2;
    let bottom = top + LYING_THICKNESS - 1;
    // Testa ruotata: la cima a sinistra, il viso in su.
    let mut head = draw_head(p, key.sex, true, false).rotated_ccw();
    // Lo chignon oltre la cima della testa finirebbe sul bordo del fotogramma.
    for y in 0..head.height {
        head.set(0, y, crate::art::CLEAR);
    }
    // Dopo la rotazione la testa occupa colonne 1..=6 e righe 1..=6.
    c.overlay(&head, x0 - 1, bottom - 6);
    // Coperta, con il lenzuolo risvoltato sotto il mento.
    let (blanket, shade) = p.blanket;
    let from = x0 + 5;
    let to = x0 + length - 1;
    for x in from..=to {
        for y in top + 1..=bottom {
            let corner = (x == to && y == top + 1) || (x == from && y == top + 1);
            if corner {
                continue;
            }
            let color = if x <= from + 1 {
                SHEET
            } else if y == bottom {
                shade
            } else {
                blanket
            };
            put(c, x, y, color);
        }
    }
    // I piedi sollevano la coperta.
    put(c, to - 1, top, blanket);
    put(c, to - 2, top, blanket);
}

fn draw_baby(c: &mut Canvas, p: &Paint, frame: Frame) {
    let blink = matches!(frame, Frame::Idle1 | Frame::Sleep);
    let bob = match frame {
        Frame::Walk1 | Frame::Walk3 => -1,
        _ => 0,
    };
    let (w, ws) = p.blanket;
    let mut palette = vec![('w', w), ('W', ws), ('h', p.hair.0)];
    palette.extend(head_palette(p));
    let mut baby = template(&BABY_ROWS, &palette);
    if blink {
        baby.recolor(EYE, p.skin.1);
    }
    let x = MID - 3;
    let y = FEET - 4 + bob;
    c.overlay(&baby, x, y);
}

/// Un fotogramma di un NPC, con il bordo.
pub fn frame_canvas(key: &AppearanceKey, frame: Frame) -> Canvas {
    let mut c = Canvas::new(CELL.x, CELL.y);
    let p = paint(key);
    if key.stage == Stage::Baby {
        draw_baby(&mut c, &p, frame);
    } else if frame == Frame::Sleep {
        draw_lying(&mut c, key, &p);
    } else {
        let pose = pose(key, p.cane, frame);
        draw_figure(&mut c, key, &p, &pose);
    }
    c.outlined(OUTLINE)
}

/// Foglio di tutti i fotogrammi, in fila.
pub fn sheet(key: &AppearanceKey) -> Canvas {
    let mut out = Canvas::new(CELL.x * FRAMES as u32, CELL.y);
    for frame in Frame::ALL {
        let cell = frame_canvas(key, frame);
        out.overlay(&cell, (frame.index() as u32 * CELL.x) as i32, 0);
    }
    out
}

/// Sagoma bianca un pixel più larga di ogni fotogramma (evidenzia la selezione).
pub fn silhouette(sheet: &Canvas, cell: UVec2) -> Canvas {
    let mut out = Canvas::new(sheet.width, sheet.height);
    let cells = sheet.width / cell.x.max(1);
    for i in 0..cells {
        let x0 = (i * cell.x) as i32;
        let mut one = Canvas::new(cell.x, cell.y);
        for y in 0..cell.y as i32 {
            for x in 0..cell.x as i32 {
                if sheet.is_opaque(x0 + x, y) {
                    put(&mut one, x, y, [255; 4]);
                }
            }
        }
        out.overlay(&one.outlined([255; 4]), x0, 0);
    }
    out
}

// --- Animazioni -------------------------------------------------------------------

/// Animazione di un NPC, decisa dalla sua azione e da come si muove.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Anim {
    Idle,
    Walk,
    Eat,
    Work,
    /// Al lavoro con l'attrezzo in mano.
    WorkTool,
    Talk,
    Buy,
    Sleep,
}

/// Animazione per un'azione. `moving`: lo sprite sta camminando verso il suo
/// posto; `lying`: è già sdraiato a letto; `has_tool`: possiede un attrezzo.
pub fn anim_for(action: &Action, moving: bool, lying: bool, has_tool: bool) -> Anim {
    if lying {
        return Anim::Sleep;
    }
    if moving {
        return Anim::Walk;
    }
    match action {
        Action::Eat(_) => Anim::Eat,
        Action::Work(_) if has_tool => Anim::WorkTool,
        Action::Work(_) => Anim::Work,
        Action::Socialize(_) => Anim::Talk,
        Action::Buy(_) => Anim::Buy,
        // Chi viaggia resta a metà passo mentre aspetta di muoversi ancora.
        Action::Travel { .. } => Anim::Walk,
        Action::Sleep(_) | Action::Idle | Action::Wait => Anim::Idle,
    }
}

/// Sequenza di fotogrammi con la durata di ognuno (secondi reali).
fn timeline(anim: Anim) -> &'static [(Frame, f32)] {
    match anim {
        Anim::Idle => &[(Frame::Idle0, 0.9), (Frame::Idle1, 0.6)],
        Anim::Eat => &[(Frame::Eat0, 0.8), (Frame::Eat1, 0.5)],
        Anim::Work => &[(Frame::Work0, 0.4), (Frame::Work1, 0.4)],
        Anim::WorkTool => &[(Frame::Tool0, 0.35), (Frame::Tool1, 0.25)],
        Anim::Talk => &[
            (Frame::Talk0, 0.45),
            (Frame::Idle0, 0.35),
            (Frame::Talk1, 0.5),
            (Frame::Idle0, 0.6),
        ],
        Anim::Buy => &[(Frame::Buy0, 0.8), (Frame::Idle0, 0.7)],
        Anim::Sleep => &[(Frame::Sleep, 1.0)],
        Anim::Walk => &[
            (Frame::Walk0, 1.0),
            (Frame::Walk1, 1.0),
            (Frame::Walk2, 1.0),
            (Frame::Walk3, 1.0),
        ],
    }
}

/// Fotogramma di una sequenza al tempo `t` (ciclica).
fn cycle(frames: &[(Frame, f32)], t: f32) -> Frame {
    let total: f32 = frames.iter().map(|&(_, d)| d).sum();
    let mut t = t.rem_euclid(total);
    for &(frame, duration) in frames {
        if t < duration {
            return frame;
        }
        t -= duration;
    }
    frames[frames.len() - 1].0
}

/// Fotogramma da mostrare. `clock`: secondi reali dall'inizio dell'animazione
/// (con uno sfasamento per NPC); `walked`: pixel percorsi, così i passi
/// seguono lo spostamento vero dello sprite; `stride`: pixel per fotogramma.
pub fn anim_frame(anim: Anim, clock: f32, walked: f32, stride: f32) -> Frame {
    if anim == Anim::Walk {
        let steps = (walked / stride.max(0.1)).floor() as i64;
        return timeline(Anim::Walk)[steps.rem_euclid(4) as usize].0;
    }
    cycle(timeline(anim), clock)
}

// --- Giocatore --------------------------------------------------------------------

/// Fotogramma del giocatore (più grande degli NPC: ingombro 12×24).
pub const PLAYER_CELL: UVec2 = UVec2::new(28, 32);
const P_FEET: i32 = 30;
const P_MID: i32 = 14;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlayerFrame {
    Idle0,
    Idle1,
    Walk0,
    Walk1,
    Walk2,
    Walk3,
    Jump,
    Fall,
}

pub const PLAYER_FRAMES: usize = 8;

impl PlayerFrame {
    pub const ALL: [PlayerFrame; PLAYER_FRAMES] = [
        PlayerFrame::Idle0,
        PlayerFrame::Idle1,
        PlayerFrame::Walk0,
        PlayerFrame::Walk1,
        PlayerFrame::Walk2,
        PlayerFrame::Walk3,
        PlayerFrame::Jump,
        PlayerFrame::Fall,
    ];

    pub fn index(self) -> usize {
        self as usize
    }
}

/// Pixel percorsi per fotogramma della corsa del giocatore.
pub const PLAYER_STRIDE: f32 = 5.0;

/// Fotogramma del giocatore: in aria salta o cade, a terra corre (i passi
/// seguono la strada fatta) o respira.
pub fn player_frame(grounded: bool, velocity: Vec2, clock: f32, walked: f32) -> PlayerFrame {
    if !grounded {
        return if velocity.y > 0.0 {
            PlayerFrame::Jump
        } else {
            PlayerFrame::Fall
        };
    }
    if velocity.x.abs() > 5.0 {
        let steps = (walked / PLAYER_STRIDE).floor() as i64;
        return [
            PlayerFrame::Walk0,
            PlayerFrame::Walk1,
            PlayerFrame::Walk2,
            PlayerFrame::Walk3,
        ][steps.rem_euclid(4) as usize];
    }
    if clock.rem_euclid(1.6) < 1.0 {
        PlayerFrame::Idle0
    } else {
        PlayerFrame::Idle1
    }
}

/// Ancora dello sprite del giocatore: il centro dell'ingombro 12×24, piedi in basso.
pub fn player_anchor(size: Vec2) -> Vec2 {
    let center_y = (P_FEET + 1) as f32 - size.y / 2.0;
    Vec2::new(
        0.0,
        (PLAYER_CELL.y as f32 / 2.0 - center_y) / PLAYER_CELL.y as f32,
    )
}

/// Cappuccio con il viso e la sciarpa gialla (10×10).
/// `g` cappuccio, `G` ombra, `s`/`S` pelle, `e` occhio, `y`/`Y` sciarpa, `k` capelli.
const HOOD_ROWS: [&str; 10] = [
    "...gggg...", //
    "..gggggg..", //
    ".gggggggg.", //
    ".ggGkkkss.", //
    ".gGkssses.", //
    ".gGssssss.", //
    ".gGgsssS..", //
    ".yyyyyyyy.", //
    "yyYyyyyyy.", //
    "yY.YYYY...", //
];

/// Busto del giocatore (8×8): giaccone rattoppato con la cinghia della
/// borsa. `c`/`C` giaccone, `p` toppa, `b` cinghia.
const COAT_ROWS: [&str; 8] = [
    ".Cccccc.", //
    "CCcbcccc", //
    "CCccbccc", //
    "CCcpcbcc", //
    "CCcccccb", //
    "CCbbbbbb", //
    "CCcccccc", //
    ".Cccccc.", //
];

const HOOD: Pair = (rgb(0x6a, 0x5e, 0x52), rgb(0x4c, 0x42, 0x3a));
const COAT: Pair = (rgb(0x5a, 0x62, 0x58), rgb(0x42, 0x48, 0x40));
const PATCH: Rgba = rgb(0x8a, 0x6a, 0x4a);
const SCARF: Pair = (rgb(0xf2, 0xc4, 0x34), rgb(0xc8, 0x96, 0x1e));
const TROUSERS: Pair = (rgb(0x4a, 0x3e, 0x36), rgb(0x36, 0x2c, 0x26));
const BOOTS: Pair = (rgb(0x2e, 0x24, 0x20), rgb(0x22, 0x1a, 0x18));
const P_SKIN: Pair = (rgb(0xe0, 0xae, 0x86), rgb(0xb8, 0x86, 0x64));
const GLOVE: Rgba = rgb(0x6e, 0x4a, 0x30);

/// Posa del giocatore: busto su/giù, piedi (vicino, lontano), braccia
/// (scarto della mano), piedi sollevati.
struct PlayerPose {
    bob: i32,
    near_foot: IVec2,
    far_foot: IVec2,
    near_hand: IVec2,
    far_hand: IVec2,
    /// Lembo della sciarpa che sventola dietro.
    scarf: i32,
}

fn player_pose(frame: PlayerFrame) -> PlayerPose {
    let still = PlayerPose {
        bob: 0,
        near_foot: IVec2::new(0, 0),
        far_foot: IVec2::new(0, 0),
        near_hand: IVec2::new(1, 6),
        far_hand: IVec2::new(-1, 6),
        scarf: 0,
    };
    match frame {
        PlayerFrame::Idle0 => still,
        PlayerFrame::Idle1 => PlayerPose { bob: 1, ..still },
        PlayerFrame::Walk0 => PlayerPose {
            near_foot: IVec2::new(3, 0),
            far_foot: IVec2::new(-3, 0),
            near_hand: IVec2::new(-3, 5),
            far_hand: IVec2::new(3, 5),
            scarf: 1,
            ..still
        },
        PlayerFrame::Walk1 => PlayerPose {
            bob: -1,
            far_foot: IVec2::new(0, -2),
            scarf: 2,
            ..still
        },
        PlayerFrame::Walk2 => PlayerPose {
            near_foot: IVec2::new(-3, 0),
            far_foot: IVec2::new(3, 0),
            near_hand: IVec2::new(3, 5),
            far_hand: IVec2::new(-3, 5),
            scarf: 1,
            ..still
        },
        PlayerFrame::Walk3 => PlayerPose {
            bob: -1,
            near_foot: IVec2::new(0, -2),
            scarf: 2,
            ..still
        },
        PlayerFrame::Jump => PlayerPose {
            bob: -1,
            near_foot: IVec2::new(2, -3),
            far_foot: IVec2::new(-2, -1),
            near_hand: IVec2::new(3, -3),
            far_hand: IVec2::new(-3, 3),
            scarf: 0,
        },
        PlayerFrame::Fall => PlayerPose {
            bob: -1,
            near_foot: IVec2::new(1, -1),
            far_foot: IVec2::new(-2, -2),
            near_hand: IVec2::new(4, -1),
            far_hand: IVec2::new(-4, -1),
            scarf: 3,
        },
    }
}

/// Un fotogramma del giocatore, con il bordo.
pub fn player_frame_canvas(frame: PlayerFrame) -> Canvas {
    let mut c = Canvas::new(PLAYER_CELL.x, PLAYER_CELL.y);
    let pose = player_pose(frame);
    // Ingombro 12×24: testa 8, busto 8, gambe 8.
    let top = P_FEET - 24 + 1 + pose.bob;
    let torso_top = top + 8;
    let hip_y = torso_top + 8;

    // Braccio lontano.
    let far_shoulder = IVec2::new(P_MID - 2, torso_top + 1);
    thick_arm(
        &mut c,
        far_shoulder,
        far_shoulder + pose.far_hand,
        COAT.1,
        blend(GLOVE, OUTLINE, 0.3),
    );
    // Gambe: 3 pixel di larghezza, stivali alti due righe.
    for (hip_x, foot, cloth, boot) in [
        (P_MID - 3, pose.far_foot, TROUSERS.1, BOOTS.1),
        (P_MID, pose.near_foot, TROUSERS.0, BOOTS.0),
    ] {
        let foot_y = P_FEET + foot.y;
        let rows = foot_y - hip_y + 1;
        for r in 0..rows {
            let x = hip_x + ((foot.x * r) as f32 / (rows - 1).max(1) as f32).round() as i32;
            let y = hip_y + r;
            let color = if r >= rows - 2 { boot } else { cloth };
            for dx in 0..3 {
                put(&mut c, x + dx, y, color);
            }
            if r == rows - 1 {
                put(&mut c, x + 3, y, boot);
            }
        }
    }
    // Busto.
    let coat = template(
        &COAT_ROWS,
        &[
            ('c', COAT.0),
            ('C', COAT.1),
            ('p', PATCH),
            ('b', rgb(0x3a, 0x2c, 0x22)),
        ],
    );
    c.overlay(&coat, P_MID - 4, torso_top);
    // Lembo della sciarpa che sventola dietro il collo.
    let tail_y = torso_top;
    for i in 0..3 {
        let x = P_MID - 5 - i;
        let y = tail_y + i + 1 - pose.scarf.min(i + 1);
        put(&mut c, x, y, if i == 2 { SCARF.1 } else { SCARF.0 });
    }
    // Testa.
    let hood = template(
        &HOOD_ROWS,
        &[
            ('g', HOOD.0),
            ('G', HOOD.1),
            ('s', P_SKIN.0),
            ('S', P_SKIN.1),
            ('e', EYE),
            ('y', SCARF.0),
            ('Y', SCARF.1),
            ('k', rgb(0x4a, 0x30, 0x22)),
        ],
    );
    c.overlay(&hood, P_MID - 5, top - 1);
    // Braccio vicino.
    let near_shoulder = IVec2::new(P_MID + 1, torso_top + 1);
    thick_arm(
        &mut c,
        near_shoulder,
        near_shoulder + pose.near_hand,
        COAT.0,
        GLOVE,
    );
    c.outlined(OUTLINE)
}

/// Braccio largo 2 pixel (giocatore).
fn thick_arm(c: &mut Canvas, shoulder: IVec2, hand: IVec2, sleeve: Rgba, glove: Rgba) {
    let points = line_points(shoulder, hand);
    let last = points.len() - 1;
    for (i, p) in points.iter().enumerate() {
        let color = if i + 1 >= last { glove } else { sleeve };
        put(c, p.x, p.y, color);
        put(c, p.x + 1, p.y, color);
    }
}

/// Foglio del giocatore, in fila.
pub fn player_sheet() -> Canvas {
    let mut out = Canvas::new(PLAYER_CELL.x * PLAYER_FRAMES as u32, PLAYER_CELL.y);
    for frame in PlayerFrame::ALL {
        let cell = player_frame_canvas(frame);
        out.overlay(&cell, (frame.index() as u32 * PLAYER_CELL.x) as i32, 0);
    }
    out
}

// --- Cache delle texture -------------------------------------------------------------

/// Texture generate, condivise da tutti gli NPC con lo stesso aspetto.
#[derive(Resource)]
pub struct CharacterArt {
    layout: Handle<TextureAtlasLayout>,
    sheets: HashMap<AppearanceKey, Handle<Image>>,
    highlights: HashMap<AppearanceKey, Handle<Image>>,
    /// Fogli generati dall'avvio (per statistiche e test).
    pub generated: usize,
}

impl FromWorld for CharacterArt {
    fn from_world(world: &mut World) -> Self {
        let layout = TextureAtlasLayout::from_grid(CELL, FRAMES as u32, 1, None, None);
        let layout = world
            .resource_mut::<Assets<TextureAtlasLayout>>()
            .add(layout);
        Self {
            layout,
            sheets: HashMap::new(),
            highlights: HashMap::new(),
            generated: 0,
        }
    }
}

impl CharacterArt {
    /// Texture del foglio di un aspetto, generata al primo uso.
    pub fn sheet(&mut self, key: AppearanceKey, images: &mut Assets<Image>) -> Handle<Image> {
        if let Some(handle) = self.sheets.get(&key) {
            return handle.clone();
        }
        let handle = images.add(sheet(&key).to_image());
        self.generated += 1;
        self.sheets.insert(key, handle.clone());
        handle
    }

    /// Sagoma per evidenziare la selezione, generata solo quando serve.
    pub fn highlight(&mut self, key: AppearanceKey, images: &mut Assets<Image>) -> Handle<Image> {
        if let Some(handle) = self.highlights.get(&key) {
            return handle.clone();
        }
        let handle = images.add(silhouette(&sheet(&key), CELL).to_image());
        self.highlights.insert(key, handle.clone());
        handle
    }

    pub fn atlas(&self, frame: Frame) -> TextureAtlas {
        TextureAtlas {
            layout: self.layout.clone(),
            index: frame.index(),
        }
    }

    /// Fogli in cache.
    pub fn cached(&self) -> usize {
        self.sheets.len()
    }

    /// Libera i fogli (e le sagome) che nessuno usa più.
    pub fn retain_used(&mut self, used: &std::collections::HashSet<AppearanceKey>) {
        self.sheets.retain(|key, _| used.contains(key));
        self.highlights.retain(|key, _| used.contains(key));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim::World;

    fn key(stage: Stage, sex: Sex, look: u8, job: Option<Job>, dressed: bool) -> AppearanceKey {
        AppearanceKey {
            stage,
            sex,
            look,
            job,
            dressed,
        }
    }

    fn all_keys() -> Vec<AppearanceKey> {
        let mut keys = Vec::new();
        for stage in Stage::ALL {
            for sex in Sex::ALL {
                for look in 0..LOOKS {
                    for job in [
                        None,
                        Some(Job::Contadino),
                        Some(Job::Cuoco),
                        Some(Job::Operaio),
                        Some(Job::Mercante),
                    ] {
                        for dressed in [true, false] {
                            keys.push(key(stage, sex, look, job, dressed));
                        }
                    }
                }
            }
        }
        keys
    }

    #[test]
    fn templates_use_only_palette_characters_and_have_consistent_widths() {
        let mut sets: Vec<(&str, Vec<&str>, usize)> = vec![
            ("testa", HEAD_ROWS.to_vec(), 6),
            ("barba", BEARD_ROWS.to_vec(), 8),
            ("cappello", TOQUE_ROWS.to_vec(), 8),
            ("neonato", BABY_ROWS.to_vec(), 6),
        ];
        for (i, style) in MALE_HAIR.iter().chain(FEMALE_HAIR.iter()).enumerate() {
            assert!(style.len() <= 10, "capelli {i}: troppo lunghi");
            sets.push(("capelli", style.to_vec(), 8));
        }
        for (name, rows, width) in sets {
            for row in rows {
                assert_eq!(row.chars().count(), width, "{name}: {row:?}");
                for ch in row.chars() {
                    assert!(
                        ch == '.' || TEMPLATE_CHARS.contains(ch),
                        "{name}: carattere {ch:?}"
                    );
                }
            }
        }
        for row in HOOD_ROWS {
            assert_eq!(row.chars().count(), 10);
            assert!(row.chars().all(|c| c == '.' || "gGsSeyYk".contains(c)));
        }
        for row in COAT_ROWS {
            assert_eq!(row.chars().count(), 8);
            assert!(row.chars().all(|c| c == '.' || "cCpb".contains(c)));
        }
    }

    #[test]
    fn every_variant_draws_every_frame_inside_its_cell() {
        for key in all_keys() {
            let s = sheet(&key);
            assert_eq!((s.width, s.height), (CELL.x * FRAMES as u32, CELL.y));
            for frame in Frame::ALL {
                let c = frame_canvas(&key, frame);
                assert_eq!((c.width, c.height), (CELL.x, CELL.y));
                let opaque = c.pixels.iter().filter(|p| p[3] != 0).count();
                assert!(opaque > 20, "{key:?} {frame:?} è vuoto");
                // Niente tocca i lati del fotogramma: il contorno ci sta e i
                // fotogrammi vicini non si toccano.
                for y in 0..CELL.y {
                    for x in [0, CELL.x - 1] {
                        assert_eq!(c.get(x, y)[3], 0, "{key:?} {frame:?} ({x}, {y})");
                    }
                }
            }
        }
        for frame in PlayerFrame::ALL {
            let c = player_frame_canvas(frame);
            assert_eq!((c.width, c.height), (PLAYER_CELL.x, PLAYER_CELL.y));
            for y in 0..PLAYER_CELL.y {
                assert_eq!(c.get(0, y)[3], 0);
                assert_eq!(c.get(PLAYER_CELL.x - 1, y)[3], 0);
            }
        }
    }

    #[test]
    fn feet_stand_on_the_bottom_of_the_body() {
        // In piedi i piedi sono sull'ultima riga del corpo, così lo sprite
        // poggia sul pavimento con l'ancora di `Stage::anchor`.
        for stage in Stage::ALL {
            let k = key(stage, Sex::Male, 0, None, true);
            let c = frame_canvas(&k, Frame::Idle0);
            let lowest = (0..CELL.y)
                .rev()
                .find(|&y| (0..CELL.x).any(|x| c.get(x, y)[3] != 0 && c.get(x, y) != OUTLINE))
                .unwrap();
            assert_eq!(lowest as i32, FEET, "{stage:?}");
            let highest = (0..CELL.y)
                .find(|&y| (0..CELL.x).any(|x| c.get(x, y)[3] != 0 && c.get(x, y) != OUTLINE))
                .unwrap();
            let height = FEET - highest as i32 + 1;
            assert!(
                (height as f32 - stage.size().y).abs() <= 1.0,
                "{stage:?}: alto {height}"
            );
        }
    }

    #[test]
    fn generation_is_deterministic() {
        let world = World::generate(42, 20, 400);
        for npc in world.npcs.iter().take(50) {
            let a = appearance(npc);
            assert_eq!(a, appearance(&npc.clone()));
            assert_eq!(sheet(&a), sheet(&a));
        }
        // Stesso id, stesso look anche con età e lavoro diversi.
        let id = NpcId(77);
        let young = appearance_of(id, Sex::Female, 8, None, true);
        let old = appearance_of(id, Sex::Female, 70, Some(Job::Cuoco), false);
        assert_eq!(young.look, old.look);
        assert_eq!(young.stage, Stage::Child);
        assert_eq!(old.stage, Stage::Elder);
        // I bambini non hanno divisa.
        assert_eq!(
            appearance_of(id, Sex::Male, 10, Some(Job::Cuoco), true).job,
            None
        );
    }

    #[test]
    fn many_npcs_share_few_textures() {
        let world = World::generate(42, 20, 400);
        let keys: std::collections::HashSet<_> = world.npcs.iter().map(appearance).collect();
        let n = world.npcs.len();
        assert!(keys.len() * 2 <= n, "{} texture per {n} NPC", keys.len());
        // Tetto teorico: fasce × sessi × look × (lavori + nessuno) × vestiti.
        assert!(keys.len() <= Stage::ALL.len() * 2 * LOOKS as usize * 5 * 2);
        // La cache genera una volta per chiave.
        let mut ecs = bevy::ecs::world::World::new();
        ecs.insert_resource(Assets::<Image>::default());
        ecs.insert_resource(Assets::<TextureAtlasLayout>::default());
        let mut art = CharacterArt::from_world(&mut ecs);
        let mut images = ecs.resource_mut::<Assets<Image>>();
        for npc in &world.npcs {
            art.sheet(appearance(npc), &mut images);
        }
        assert_eq!(art.cached(), keys.len());
        assert_eq!(art.generated, keys.len());
        assert_eq!(images.len(), keys.len());
    }

    #[test]
    fn clothes_and_jobs_change_the_look() {
        let k = key(Stage::Adult, Sex::Male, 3, Some(Job::Contadino), true);
        let bare = AppearanceKey {
            dressed: false,
            ..k
        };
        assert_ne!(sheet(&k), sheet(&bare));
        // Senza vestiti: stracci meno saturi della divisa.
        let saturation = |c: Rgba| {
            let (hi, lo) = (c[..3].iter().max().unwrap(), c[..3].iter().min().unwrap());
            hi - lo
        };
        let dressed = paint(&k);
        let rags = paint(&bare);
        assert!(saturation(rags.shirt.0) < saturation(dressed.pants.0));
        assert_eq!(rags.garment, Garment::Rags);
        for job in Job::ALL {
            let a = key(Stage::Adult, Sex::Female, 1, Some(job), true);
            let b = key(Stage::Adult, Sex::Female, 1, None, true);
            assert_ne!(sheet(&a), sheet(&b), "{job:?}");
        }
    }

    #[test]
    fn animation_follows_action_and_movement() {
        use sim::{ItemKind, StationId};
        let s = StationId(0);
        assert_eq!(anim_for(&Action::Eat(s), false, false, false), Anim::Eat);
        assert_eq!(anim_for(&Action::Eat(s), true, false, false), Anim::Walk);
        assert_eq!(
            anim_for(&Action::Work(s), false, false, true),
            Anim::WorkTool
        );
        assert_eq!(anim_for(&Action::Work(s), false, false, false), Anim::Work);
        assert_eq!(anim_for(&Action::Sleep(s), false, true, false), Anim::Sleep);
        // Chi va a letto cammina, poi si sdraia.
        assert_eq!(anim_for(&Action::Sleep(s), true, false, false), Anim::Walk);
        assert_eq!(anim_for(&Action::Sleep(s), false, false, false), Anim::Idle);
        assert_eq!(
            anim_for(&Action::Socialize(NpcId(1)), false, false, false),
            Anim::Talk
        );
        assert_eq!(
            anim_for(&Action::Buy(ItemKind::Vestito), false, false, false),
            Anim::Buy
        );
        assert_eq!(anim_for(&Action::Idle, false, false, false), Anim::Idle);
        let travel = Action::Travel {
            to: sim::CarriageId(3),
        };
        assert_eq!(anim_for(&travel, false, false, false), Anim::Walk);
    }

    #[test]
    fn frames_are_a_pure_function_of_time_and_distance() {
        // Respiro: prima il fotogramma di base, poi quello del respiro, ciclico.
        assert_eq!(anim_frame(Anim::Idle, 0.0, 0.0, 3.0), Frame::Idle0);
        assert_eq!(anim_frame(Anim::Idle, 1.0, 0.0, 3.0), Frame::Idle1);
        assert_eq!(anim_frame(Anim::Idle, 1.5, 0.0, 3.0), Frame::Idle0);
        assert_eq!(anim_frame(Anim::Idle, -0.1, 0.0, 3.0), Frame::Idle1);
        // Camminata: i passi seguono la strada, non il tempo.
        let walk: Vec<_> = (0..8)
            .map(|i| anim_frame(Anim::Walk, 123.0, i as f32 * 3.0 + 0.5, 3.0))
            .collect();
        assert_eq!(
            walk,
            [
                Frame::Walk0,
                Frame::Walk1,
                Frame::Walk2,
                Frame::Walk3,
                Frame::Walk0,
                Frame::Walk1,
                Frame::Walk2,
                Frame::Walk3
            ]
        );
        assert_eq!(
            anim_frame(Anim::Walk, 0.0, 4.0, 3.0),
            anim_frame(Anim::Walk, 99.0, 4.0, 3.0)
        );
        // Ogni animazione usa solo i suoi fotogrammi.
        for t in 0..40 {
            let t = t as f32 * 0.1;
            assert!(matches!(
                anim_frame(Anim::WorkTool, t, 0.0, 3.0),
                Frame::Tool0 | Frame::Tool1
            ));
            assert!(matches!(
                anim_frame(Anim::Eat, t, 0.0, 3.0),
                Frame::Eat0 | Frame::Eat1
            ));
            assert_eq!(anim_frame(Anim::Sleep, t, 0.0, 3.0), Frame::Sleep);
            assert!(matches!(
                anim_frame(Anim::Talk, t, 0.0, 3.0),
                Frame::Talk0 | Frame::Talk1 | Frame::Idle0
            ));
        }
    }

    #[test]
    fn player_frames_follow_the_body() {
        assert_eq!(
            player_frame(false, Vec2::new(0.0, 100.0), 0.0, 0.0),
            PlayerFrame::Jump
        );
        assert_eq!(
            player_frame(false, Vec2::new(50.0, -10.0), 0.0, 0.0),
            PlayerFrame::Fall
        );
        assert_eq!(player_frame(true, Vec2::ZERO, 0.0, 0.0), PlayerFrame::Idle0);
        assert_eq!(player_frame(true, Vec2::ZERO, 1.2, 0.0), PlayerFrame::Idle1);
        assert_eq!(
            player_frame(true, Vec2::new(110.0, 0.0), 0.0, PLAYER_STRIDE * 2.5),
            PlayerFrame::Walk2
        );
    }

    #[test]
    fn anchors_put_the_body_center_on_the_transform() {
        for stage in Stage::ALL {
            let a = stage.anchor();
            // Dal centro del fotogramma all'ancora, in pixel (y in su).
            let center_from_bottom = (a.y + 0.5) * CELL.y as f32;
            let feet_from_bottom = (CELL.y as i32 - 1 - FEET) as f32;
            assert!(
                (center_from_bottom - feet_from_bottom - stage.size().y / 2.0).abs() < 1e-4,
                "{stage:?}"
            );
        }
    }
}
