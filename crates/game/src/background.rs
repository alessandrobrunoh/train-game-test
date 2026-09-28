//! Il mondo fuori dal treno: cielo, montagne, rovine, pali, binari e neve.
//!
//! Il treno è sempre in corsa, quindi il paesaggio scorre di continuo verso
//! sinistra a velocità costante (non dipende dalla velocità della sim). Ogni
//! strato ha una profondità: 1 = sul piano del treno (binari), 0 = infinitamente
//! lontano (cielo). Più uno strato è lontano, più lentamente scorre e più segue
//! la camera (parallasse).
//!
//! L'ora dell'orologio della sim guida il ciclo giorno/notte: colori del cielo,
//! stelle, sole e luna, luce sull'esterno; dentro le carrozze le lampade
//! si fanno più calde e visibili di notte.
//!
//! Le immagini nascono una volta all'avvio; per frame si spostano solo pochi
//! sprite (uno per strato) e i fiocchi di neve (un pool fisso).

use std::f32::consts::{PI, TAU};

use bevy::prelude::*;
use bevy::sprite::SpriteImageMode;
use bevy::transform::TransformSystems;

use crate::art::{Canvas, Rgba};
use crate::env_art::{
    ArtCache, ArtKey, ExteriorArt, InteriorArt, LampGlow, hash2, line, mix, px, rect, rgb, rgba,
    shade,
};
use crate::state::Sim;
use crate::train::{FLOOR_Y, WALL};

/// Velocità apparente del treno sul paesaggio (unità mondo al secondo).
pub const TRAIN_SPEED: f32 = 140.0;

/// Quota della superficie del binario (dove poggiano le ruote).
const RAIL_TOP: f32 = FLOOR_Y - WALL - 12.0;

/// Colore della neve al suolo.
const SNOW_GROUND: Rgba = rgb(222, 230, 242);

/// Fiocchi di neve nel pool.
const SNOW_FLAKES: usize = 220;

// Profondità (z) degli strati, tutti dietro al treno.
const Z_SKY: f32 = -100.0;
const Z_STARS: f32 = -99.0;
const Z_SUN: f32 = -98.5;
const Z_SNOW: f32 = -25.0;
const Z_GROUND_FILL: f32 = -31.0;

// --- Ciclo giorno/notte -------------------------------------------------------

/// Colori chiave del cielo; tra due chiavi si passa con una dissolvenza.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkyKey {
    Night,
    Dawn,
    Day,
    Dusk,
}

impl SkyKey {
    const ALL: [SkyKey; 4] = [SkyKey::Night, SkyKey::Dawn, SkyKey::Day, SkyKey::Dusk];

    fn index(self) -> usize {
        self as usize
    }

    /// Colori dall'alto verso l'orizzonte.
    fn stops(self) -> [Rgba; 3] {
        match self {
            SkyKey::Night => [rgb(6, 8, 24), rgb(16, 22, 52), rgb(38, 50, 86)],
            SkyKey::Dawn => [rgb(40, 54, 112), rgb(150, 122, 162), rgb(250, 184, 140)],
            SkyKey::Day => [rgb(72, 128, 196), rgb(132, 178, 222), rgb(208, 226, 238)],
            SkyKey::Dusk => [rgb(34, 34, 86), rgb(142, 80, 122), rgb(244, 142, 92)],
        }
    }
}

/// Transizioni del cielo: (ora di inizio, ora di fine, da, a).
const SKY_TIMELINE: [(f32, f32, SkyKey, SkyKey); 4] = [
    (4.5, 6.0, SkyKey::Night, SkyKey::Dawn),
    (6.0, 7.5, SkyKey::Dawn, SkyKey::Day),
    (17.0, 18.5, SkyKey::Day, SkyKey::Dusk),
    (18.5, 20.0, SkyKey::Dusk, SkyKey::Night),
];

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Cielo all'ora `hour` (0..24): chiave di base, chiave sopra e quanto si vede
/// quella sopra (0..1).
pub fn sky_blend(hour: f32) -> (SkyKey, SkyKey, f32) {
    let h = hour.rem_euclid(24.0);
    for (start, end, from, to) in SKY_TIMELINE {
        if h >= start && h < end {
            return (from, to, smoothstep(start, end, h));
        }
    }
    let key = if (7.5..17.0).contains(&h) {
        SkyKey::Day
    } else {
        SkyKey::Night
    };
    (key, key, 0.0)
}

/// Quanta luce del giorno c'è (0 = notte piena, 1 = giorno pieno).
pub fn daylight(hour: f32) -> f32 {
    let h = hour.rem_euclid(24.0);
    smoothstep(5.0, 7.5, h) * (1.0 - smoothstep(17.5, 20.0, h))
}

/// Quanto è "ora dorata" (alba e tramonto), 0..1.
fn golden(hour: f32) -> f32 {
    let h = hour.rem_euclid(24.0);
    let bell = |center: f32| (1.0 - ((h - center) / 1.4).abs()).max(0.0);
    bell(6.4).max(bell(18.6))
}

/// Tinta (moltiplicativa, RGB lineari 0..1) del paesaggio all'ora `hour`.
pub fn exterior_tint(hour: f32) -> [f32; 3] {
    const NIGHT: [f32; 3] = [0.30, 0.36, 0.56];
    const WARM: [f32; 3] = [1.0, 0.80, 0.68];
    let d = daylight(hour);
    let g = golden(hour) * 0.85;
    std::array::from_fn(|i| {
        let base = NIGHT[i] + (1.0 - NIGHT[i]) * d;
        base * (1.0 + (WARM[i] - 1.0) * g)
    })
}

/// Tinta delle pareti interne: di notte le accendono solo le lampade.
pub fn interior_tint(hour: f32) -> [f32; 3] {
    const NIGHT: [f32; 3] = [0.74, 0.68, 0.66];
    let d = daylight(hour);
    std::array::from_fn(|i| NIGHT[i] + (1.0 - NIGHT[i]) * d)
}

/// Intensità degli aloni delle lampade (moltiplica la loro forza massima).
pub fn lamp_level(hour: f32) -> f32 {
    0.12 + 0.88 * (1.0 - daylight(hour))
}

fn tint_color(t: [f32; 3]) -> Color {
    Color::srgb(t[0], t[1], t[2])
}

/// Luce del momento, calcolata dall'orologio della sim.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct DayNight {
    pub hour: f32,
    pub daylight: f32,
}

impl Default for DayNight {
    fn default() -> Self {
        Self {
            hour: 12.0,
            daylight: 1.0,
        }
    }
}

// --- Parallasse ---------------------------------------------------------------

/// Origine del motivo di uno strato largo `width` e profondo `depth`, con la
/// camera in `camera_x` dopo che il treno ha percorso `travelled` unità.
pub fn pattern_origin(camera_x: f32, travelled: f64, depth: f32, width: f32) -> f32 {
    let drift = (travelled * f64::from(depth)).rem_euclid(f64::from(width)) as f32;
    camera_x * (1.0 - depth) - drift
}

/// Bordo sinistro della prima ripetizione del motivo che copre `view_left`.
pub fn first_tile_left(origin: f32, view_left: f32, width: f32) -> f32 {
    origin + ((view_left - origin) / width).floor() * width
}

/// Ripetizioni necessarie per coprire una vista larga `view_width`.
pub fn tiles_needed(view_width: f32, width: f32) -> u32 {
    (view_width / width).ceil() as u32 + 1
}

/// Uno strato del paesaggio.
struct LayerSpec {
    name: &'static str,
    depth: f32,
    /// Quota mondo del bordo inferiore dell'immagine.
    bottom: f32,
    width: i32,
    height: i32,
    z: f32,
    make: fn(i32, i32) -> Canvas,
}

const LAYERS: [LayerSpec; 5] = [
    LayerSpec {
        name: "Montagne lontane",
        depth: 0.03,
        bottom: -40.0,
        width: 512,
        height: 236,
        z: -90.0,
        make: far_mountains,
    },
    LayerSpec {
        name: "Montagne",
        depth: 0.08,
        bottom: -40.0,
        width: 512,
        height: 170,
        z: -85.0,
        make: near_mountains,
    },
    LayerSpec {
        name: "Rovine",
        depth: 0.2,
        bottom: -40.0,
        width: 384,
        height: 160,
        z: -80.0,
        make: ruins,
    },
    LayerSpec {
        name: "Pali e cumuli",
        depth: 0.45,
        bottom: -40.0,
        width: 256,
        height: 130,
        z: -70.0,
        make: poles,
    },
    LayerSpec {
        name: "Binario",
        depth: 1.0,
        bottom: RAIL_TOP - 200.0,
        width: 256,
        height: 200,
        z: -30.0,
        make: ground,
    },
];

// --- Arte del paesaggio ---------------------------------------------------------

/// Matrice di Bayer 4×4 normalizzata (soglie per la retinatura).
fn bayer(x: i32, y: i32) -> f32 {
    const M: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    (f32::from(M[y.rem_euclid(4) as usize][x.rem_euclid(4) as usize]) + 0.5) / 16.0
}

/// Colore di una sfumatura a tre tappe in `t` (0 = prima tappa).
fn gradient(stops: [Rgba; 3], t: f32) -> Rgba {
    if t < 0.5 {
        mix(stops[0], stops[1], t * 2.0)
    } else {
        mix(stops[1], stops[2], (t - 0.5) * 2.0)
    }
}

pub const SKY_W: i32 = 32;
pub const SKY_H: i32 = 256;
const SKY_BANDS: f32 = 18.0;

/// Cielo: sfumatura a bande con retinatura, dall'alto all'orizzonte.
pub fn sky(key: SkyKey) -> Canvas {
    let mut c = Canvas::new(SKY_W as u32, SKY_H as u32);
    let stops = key.stops();
    for y in 0..SKY_H {
        let t = 1.0 - y as f32 / (SKY_H - 1) as f32;
        for x in 0..SKY_W {
            let f = t * (SKY_BANDS - 1.0);
            let band = f.floor() + if f.fract() > bayer(x, y) { 1.0 } else { 0.0 };
            px(&mut c, x, y, gradient(stops, band / (SKY_BANDS - 1.0)));
        }
    }
    c
}

/// Stelle (da ripetere su tutto il cielo).
pub fn stars() -> Canvas {
    let (w, h) = (160, 120);
    let mut c = Canvas::new(w as u32, h as u32);
    for i in 0..46 {
        let x = (hash2(i, 0, 31) % w as u32) as i32;
        let y = (hash2(i, 1, 31) % h as u32) as i32;
        let bright = 120 + (hash2(i, 2, 31) % 136) as u8;
        let color = rgba(230, 236, 255, bright);
        px(&mut c, x, y, color);
        if i % 9 == 0 {
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                px(&mut c, x + dx, y + dy, rgba(200, 210, 255, bright / 2));
            }
        }
    }
    c
}

/// Disco del sole (o della luna) con un alone.
pub fn sun_disc(moon: bool) -> Canvas {
    let size = 21;
    let mut c = Canvas::new(size as u32, size as u32);
    let center = (size - 1) as f32 / 2.0;
    let (core, halo) = if moon {
        (rgb(226, 230, 222), rgba(180, 200, 255, 40))
    } else {
        (rgb(255, 246, 214), rgba(255, 236, 180, 60))
    };
    for y in 0..size {
        for x in 0..size {
            let d = Vec2::new(x as f32 - center, y as f32 - center).length();
            if d <= 5.5 {
                let crater = moon && matches!((x - 8, y - 9), (0, 0) | (3, 2) | (1, 3) | (4, -1));
                px(&mut c, x, y, if crater { shade(core, 0.86) } else { core });
            } else if d <= 9.5 && (x + y) % 2 == 0 {
                px(&mut c, x, y, halo);
            }
        }
    }
    c
}

/// Vetta a punta periodica: 1 in cima, 0 a valle; ripete `k` volte in `w`.
fn peak(x: f32, w: f32, k: f32, phase: f32) -> f32 {
    1.0 - (PI * k * x / w + phase).sin().abs()
}

/// Sinusoide periodica su `w` con `k` onde.
fn wave(x: f32, w: f32, k: f32, phase: f32) -> f32 {
    (TAU * k * x / w + phase).sin()
}

/// Rumore periodico (ripete ogni `w` colonne).
fn col_noise(x: i32, w: i32, seed: u32) -> f32 {
    (hash2(x.rem_euclid(w), 0, seed) % 1000) as f32 / 1000.0
}

/// Colori di una catena montuosa.
struct RangeStyle {
    rock_lit: Rgba,
    rock_dark: Rgba,
    snow_lit: Rgba,
    snow_dark: Rgba,
    haze: Rgba,
    /// Quanto la foschia copre la base (0..1).
    haze_amount: f32,
}

/// Disegna una catena montuosa con creste `ridge(x)` (quota dal basso) sopra
/// una pianura innevata alta `plain`.
fn mountain_range(w: i32, h: i32, plain: i32, s: &RangeStyle, ridge: impl Fn(i32) -> f32) -> Canvas {
    let mut c = Canvas::new(w as u32, h as u32);
    for x in 0..w {
        let top = ridge(x).round().min((h - 1) as f32) as i32;
        let slope = ridge(x + 1) - ridge(x - 1);
        let lit = slope > 0.0;
        let snow_depth = 7.0 + 0.22 * (top - plain) as f32 + col_noise(x, w, 5) * 6.0;
        for y in plain..=top {
            let from_top = (top - y) as f32;
            // Canaloni di neve che scendono lungo i fianchi.
            let gully = (x * 2 + y).rem_euclid(11) == 0 && from_top < snow_depth * 2.2;
            let snowy = from_top < snow_depth || gully;
            let mut color = match (snowy, lit) {
                (true, true) => s.snow_lit,
                (true, false) => s.snow_dark,
                (false, true) => s.rock_lit,
                (false, false) => s.rock_dark,
            };
            // Bordo di luce sulla cresta.
            if y == top && lit {
                color = shade(color, 1.08);
            }
            // Foschia verso la base, a fasce orizzontali.
            let haze = (1.0 - (y - plain) as f32 / (top - plain).max(1) as f32) * s.haze_amount;
            let band = (haze * 4.0).floor() / 4.0;
            if band > 0.0 {
                color = mix(color, s.haze, band * 0.8);
            }
            px(&mut c, x, y, color);
        }
        for y in 0..plain {
            let color = if (x + y * 3).rem_euclid(17) == 0 {
                shade(s.haze, 0.94)
            } else {
                s.haze
            };
            px(&mut c, x, y, color);
        }
    }
    c
}

fn far_mountains(w: i32, h: i32) -> Canvas {
    let wf = w as f32;
    let style = RangeStyle {
        rock_lit: rgb(128, 142, 170),
        rock_dark: rgb(100, 112, 144),
        snow_lit: rgb(230, 238, 248),
        snow_dark: rgb(178, 194, 222),
        haze: rgb(196, 210, 228),
        haze_amount: 0.7,
    };
    mountain_range(w, h, 40, &style, |x| {
        let x = x as f32;
        118.0
            + 52.0 * peak(x, wf, 2.0, 0.4)
            + 30.0 * peak(x, wf, 5.0, 1.3)
            + 9.0 * wave(x, wf, 13.0, 0.7)
            + 3.0 * wave(x, wf, 31.0, 2.0)
    })
}

fn near_mountains(w: i32, h: i32) -> Canvas {
    let wf = w as f32;
    let style = RangeStyle {
        rock_lit: rgb(96, 108, 132),
        rock_dark: rgb(66, 76, 102),
        snow_lit: rgb(236, 242, 250),
        snow_dark: rgb(160, 178, 210),
        haze: rgb(206, 218, 234),
        haze_amount: 0.45,
    };
    mountain_range(w, h, 40, &style, |x| {
        let x = x as f32;
        76.0 + 34.0 * peak(x, wf, 3.0, 0.9)
            + 22.0 * peak(x, wf, 7.0, 2.1)
            + 6.0 * wave(x, wf, 17.0, 0.3)
            + 2.5 * wave(x, wf, 41.0, 1.0)
    })
}

/// Rovine di una città ghiacciata con un traliccio dell'alta tensione.
fn ruins(w: i32, h: i32) -> Canvas {
    let mut c = Canvas::new(w as u32, h as u32);
    let base = 40;
    let wall = rgb(74, 84, 108);
    let wall_lit = rgb(92, 104, 128);
    let window = rgb(46, 52, 70);
    let snow = rgb(214, 224, 240);
    let haze = rgb(170, 186, 210);

    // Traliccio e cavi (dietro agli edifici).
    let steel = rgb(58, 64, 82);
    let px0 = 250;
    let tower = 112;
    for (a, b) in [(-9, -2), (9, 2)] {
        line(&mut c, (px0 + a, base), (px0 + b, base + tower), steel);
    }
    for i in 0..9 {
        let y0 = base + i * 12;
        let y1 = y0 + 12;
        let half = |y: i32| 9 - 7 * (y - base) / tower;
        line(&mut c, (px0 - half(y0), y0), (px0 + half(y1), y1), steel);
        line(&mut c, (px0 + half(y0), y0), (px0 - half(y1), y1), steel);
    }
    for (y, half) in [(base + 100, 14), (base + 88, 11)] {
        rect(&mut c, px0 - half, y, half * 2 + 1, 1, steel);
    }
    for (dy, dx) in [(100, -14), (100, 14), (88, -11), (88, 11)] {
        for x in 0..w {
            let t = (x - (px0 + dx)).rem_euclid(w) as f32 / w as f32;
            let sag = (4.0 * t * (1.0 - t) * 22.0).round() as i32;
            px(&mut c, x, base + dy - 1 - sag, rgb(52, 58, 74));
        }
    }

    // Edifici diroccati.
    let mut x = 4;
    let mut i = 0;
    while x < w - 20 {
        let bw = 14 + (hash2(i, 0, 41) % 26) as i32;
        let bw = bw.min(w - 6 - x);
        let bh = 24 + (hash2(i, 1, 41) % 72) as i32;
        let broken = !hash2(i, 2, 41).is_multiple_of(3);
        for dx in 0..bw {
            let mut top = bh;
            if broken {
                // Crollo a V o a gradino sul lato destro.
                let edge = bw - dx;
                if edge < bw / 2 {
                    top -= (bw / 2 - edge) * 2 + (hash2(i, dx, 43) % 3) as i32;
                }
            }
            let top = top.max(8);
            for y in 0..top {
                let wx = (dx - 2).rem_euclid(6);
                let wy = y % 9;
                // Finestre vuote a file, qualcuna murata.
                let is_window = (0..2).contains(&wx)
                    && (3..7).contains(&wy)
                    && dx > 1
                    && dx < bw - 2
                    && y < top - 3
                    && !hash2(i * 31 + dx / 6, y / 9, 45).is_multiple_of(5);
                let mut color = if is_window {
                    window
                } else if dx < 2 {
                    wall_lit
                } else {
                    wall
                };
                // Neve sui davanzali.
                if is_window && wy == 3 {
                    color = shade(snow, 0.9);
                }
                px(&mut c, x + dx, base + y, color);
            }
            px(&mut c, x + dx, base + top, snow);
            if hash2(i, dx, 49).is_multiple_of(2) {
                px(&mut c, x + dx, base + top + 1, shade(snow, 0.95));
            }
        }
        x += bw + 2 + (hash2(i, 3, 41) % 12) as i32;
        i += 1;
    }

    // Cumuli di neve alla base e pianura.
    for x in 0..w {
        let drift = 4.0 + 3.0 * wave(x as f32, w as f32, 3.0, 0.5) + 2.0 * wave(x as f32, w as f32, 11.0, 1.0);
        for y in 0..base + drift.round() as i32 {
            let color = if y >= base + drift as i32 - 1 {
                rgb(232, 238, 248)
            } else {
                rgb(210, 220, 236)
            };
            px(&mut c, x, y, color);
        }
    }
    // Foschia leggera su tutto (tinta piatta, senza retino).
    for y in 0..h {
        for x in 0..w {
            let p = crate::env_art::get(&c, x, y);
            if p[3] > 0 {
                px(&mut c, x, y, mix(p, haze, 0.22));
            }
        }
    }
    c
}

/// Pali del telegrafo con i fili, cumuli di neve e sterpi.
fn poles(w: i32, h: i32) -> Canvas {
    let mut c = Canvas::new(w as u32, h as u32);
    let base = 40;
    let wood = rgb(62, 50, 44);
    let wood_lit = rgb(92, 76, 64);
    let wire = rgb(40, 40, 50);
    let spacing = w / 2;
    let arm = base + 76;
    // Fili (periodici tra un palo e l'altro).
    for x in 0..w {
        let t = (x - 40).rem_euclid(spacing) as f32 / spacing as f32;
        let sag = (4.0 * t * (1.0 - t) * 9.0).round() as i32;
        px(&mut c, x, arm - sag, wire);
        px(&mut c, x, arm - 5 - sag, wire);
    }
    for pole_x in [40, 40 + spacing] {
        rect(&mut c, pole_x, base - 6, 2, arm - base + 10, wood);
        rect(&mut c, pole_x, base - 6, 1, arm - base + 10, wood_lit);
        for y in [arm, arm - 5] {
            rect(&mut c, pole_x - 6, y, 14, 1, wood);
            for dx in [-6, 7] {
                px(&mut c, pole_x + dx, y + 1, rgb(180, 200, 190));
            }
        }
        px(&mut c, pole_x, arm + 4, rgb(236, 242, 250));
        px(&mut c, pole_x + 1, arm + 4, rgb(236, 242, 250));
    }
    // Cumuli di neve.
    let wf = w as f32;
    let bank = |x: i32| {
        let x = x as f32;
        (base as f32 + 8.0 + 6.0 * wave(x, wf, 2.0, 0.0) + 4.0 * wave(x, wf, 5.0, 1.0)
            + 1.5 * wave(x, wf, 13.0, 2.0))
            .round() as i32
    };
    for x in 0..w {
        let top = bank(x);
        let lit = bank(x + 1) >= bank(x - 1);
        for y in 0..=top {
            let color = if y == top {
                rgb(246, 250, 255)
            } else if !lit && y > top - 5 {
                rgb(196, 210, 232)
            } else if (x + 2 * y).rem_euclid(19) == 0 {
                rgb(206, 218, 236)
            } else {
                rgb(226, 234, 246)
            };
            px(&mut c, x, y, color);
        }
        // Sterpi secchi che spuntano dalla neve.
        if hash2(x, 0, 61).is_multiple_of(23) {
            let hgt = 3 + (hash2(x, 1, 61) % 5) as i32;
            rect(&mut c, x, top, 1, hgt, rgb(70, 56, 46));
            px(&mut c, x - 1, top + hgt - 2, rgb(70, 56, 46));
            px(&mut c, x + 1, top + hgt - 1, rgb(70, 56, 46));
        }
    }
    let _ = h;
    c
}

/// Binario (rotaia, traversine, massicciata) e terreno innevato sotto.
fn ground(w: i32, h: i32) -> Canvas {
    let mut c = Canvas::new(w as u32, h as u32);
    let top = h - 1;
    let snow = SNOW_GROUND;
    // Terreno innevato con qualche avvallamento e sasso.
    for y in 0..top - 12 {
        for x in 0..w {
            let n = hash2(x, y, 71) % 331;
            let color = if n == 0 {
                rgb(150, 150, 156)
            } else if (x + y * 5).rem_euclid(29) < 2 {
                rgb(204, 214, 232)
            } else {
                snow
            };
            px(&mut c, x, y, color);
        }
    }
    // Massicciata di pietrisco con la neve sopra.
    let gravel = [rgb(118, 112, 106), rgb(132, 126, 118), rgb(104, 98, 94)];
    for y in top - 12..top - 3 {
        for x in 0..w {
            // Sassi da 2×1 pixel, pochi riflessi di neve.
            let n = hash2(x / 2, y, 73);
            let g = if n.is_multiple_of(23) {
                rgb(206, 214, 228)
            } else {
                gravel[(n % 3) as usize]
            };
            px(&mut c, x, y, g);
        }
    }
    rect(&mut c, 0, top - 13, w, 1, rgb(190, 198, 214));
    // Traversine viste di testa.
    for x in (0..w).step_by(16) {
        rect(&mut c, x, top - 8, 10, 5, rgb(70, 52, 40));
        rect(&mut c, x, top - 4, 10, 1, rgb(96, 74, 58));
        rect(&mut c, x + 1, top - 3, 8, 1, rgb(224, 232, 244));
    }
    // Rotaia: fungo, anima, suola.
    rect(&mut c, 0, top - 1, w, 2, rgb(150, 152, 162));
    rect(&mut c, 0, top, w, 1, rgb(206, 208, 216));
    rect(&mut c, 0, top - 2, w, 1, rgb(80, 80, 90));
    rect(&mut c, 0, top - 3, w, 1, rgb(66, 66, 74));
    for x in (8..w).step_by(16) {
        px(&mut c, x, top - 3, rgb(40, 40, 46));
    }
    c
}

// --- Entità -------------------------------------------------------------------

/// Strato del paesaggio con parallasse (indice in [`LAYERS`]).
#[derive(Component, Debug)]
struct Layer(usize);

/// Metà del cielo: `false` la chiave di base, `true` quella in dissolvenza.
#[derive(Component, Debug)]
struct SkyPart(bool);

#[derive(Component, Debug)]
struct Stars;

/// Sole (`false`) o luna (`true`).
#[derive(Component, Debug)]
struct Orb(bool);

/// Terreno pieno sotto il binario, per le viste molto allargate.
#[derive(Component, Debug)]
struct GroundFill;

/// Un fiocco di neve: posizione normalizzata nella vista (0..1) e velocità.
#[derive(Component, Debug)]
struct Flake {
    pos: Vec2,
    velocity: Vec2,
    phase: f32,
    alpha: f32,
}

/// Immagini del cielo, una per chiave.
#[derive(Resource, Default)]
struct SkyImages(Vec<Handle<Image>>);

pub struct BackgroundPlugin;

impl Plugin for BackgroundPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ArtCache>()
            .init_resource::<DayNight>()
            .init_resource::<SkyImages>()
            .add_systems(Startup, spawn_background)
            .add_systems(Update, (update_day_night, apply_lighting).chain())
            .add_systems(
                PostUpdate,
                (scroll_layers, move_snow).before(TransformSystems::Propagate),
            );
    }
}

fn spawn_background(
    mut commands: Commands,
    mut art: ResMut<ArtCache>,
    mut sky_images: ResMut<SkyImages>,
    mut images: Option<ResMut<Assets<Image>>>,
) {
    let mut get = |key: u8, make: &dyn Fn() -> Canvas| {
        art.get(images.as_deref_mut(), ArtKey::Outside(key), make)
    };
    sky_images.0 = SkyKey::ALL
        .into_iter()
        .map(|k| get(10 + k.index() as u8, &|| sky(k)))
        .collect();

    let tiled_x = SpriteImageMode::Tiled {
        tile_x: true,
        tile_y: false,
        stretch_value: 1.0,
    };
    for (i, over) in [false, true].into_iter().enumerate() {
        commands.spawn((
            Name::new("Cielo"),
            SkyPart(over),
            Sprite {
                image: sky_images.0[SkyKey::Day.index()].clone(),
                custom_size: Some(Vec2::new(SKY_W as f32, SKY_H as f32)),
                image_mode: tiled_x.clone(),
                ..default()
            },
            Transform::from_xyz(0.0, 0.0, Z_SKY + i as f32 * 0.5),
        ));
    }
    commands.spawn((
        Name::new("Stelle"),
        Stars,
        Sprite {
            image: get(20, &stars),
            custom_size: Some(Vec2::new(160.0, 120.0)),
            image_mode: SpriteImageMode::Tiled {
                tile_x: true,
                tile_y: true,
                stretch_value: 1.0,
            },
            color: Color::WHITE.with_alpha(0.0),
            ..default()
        },
        Transform::from_xyz(0.0, 0.0, Z_STARS),
    ));
    for moon in [false, true] {
        let image = get(21 + u8::from(moon), &move || sun_disc(moon));
        commands.spawn((
            Name::new(if moon { "Luna" } else { "Sole" }),
            Orb(moon),
            Sprite::from_image(image),
            Transform::from_xyz(0.0, 0.0, Z_SUN),
        ));
    }

    for (i, spec) in LAYERS.iter().enumerate() {
        let image = get(i as u8, &|| (spec.make)(spec.width, spec.height));
        commands.spawn((
            Name::new(spec.name),
            Layer(i),
            Sprite {
                image,
                custom_size: Some(Vec2::new(spec.width as f32, spec.height as f32)),
                image_mode: tiled_x.clone(),
                ..default()
            },
            Transform::from_xyz(0.0, spec.bottom + spec.height as f32 / 2.0, spec.z),
        ));
    }
    commands.spawn((
        Name::new("Terreno"),
        GroundFill,
        Sprite {
            image: get(23, &|| {
                let mut c = Canvas::new(1, 1);
                px(&mut c, 0, 0, SNOW_GROUND);
                c
            }),
            custom_size: Some(Vec2::ONE),
            ..default()
        },
        Transform::from_xyz(0.0, 0.0, Z_GROUND_FILL),
    ));

    // Pool dei fiocchi: posizione, velocità e dimensione pseudo-casuali.
    for i in 0..SNOW_FLAKES as i32 {
        let r = |k: i32| (hash2(i, k, 91) % 10_000) as f32 / 10_000.0;
        let near = r(0);
        let size = if near > 0.8 { 2.0 } else { 1.0 };
        commands.spawn((
            Flake {
                pos: Vec2::new(r(1), r(2)),
                velocity: Vec2::new(
                    -(20.0 + TRAIN_SPEED * (0.15 + 0.6 * near)),
                    -(12.0 + 18.0 * near + 8.0 * r(3)),
                ),
                phase: r(4) * TAU,
                alpha: 0.55 + 0.4 * near,
            },
            Sprite::from_color(Color::WHITE, Vec2::splat(size)),
            Transform::from_xyz(0.0, 0.0, Z_SNOW + near),
        ));
    }
}

/// Aggiorna la luce del momento dall'orologio della sim.
fn update_day_night(sim: Res<Sim>, mut day_night: ResMut<DayNight>) {
    let hour = sim.world.clock.hour_f();
    let next = DayNight {
        hour,
        daylight: daylight(hour),
    };
    if *day_night != next {
        *day_night = next;
    }
}

/// Applica la luce a cielo, paesaggio, esterno e interno del treno (solo
/// quando l'ora cambia, cioè al massimo una volta per minuto di gioco).
#[allow(clippy::type_complexity)]
fn apply_lighting(
    day_night: Res<DayNight>,
    sky_images: Res<SkyImages>,
    mut clear: ResMut<ClearColor>,
    mut sprites: ParamSet<(
        Query<(&SkyPart, &mut Sprite)>,
        Query<&mut Sprite, With<Stars>>,
        Query<&mut Sprite, Or<(With<Layer>, With<GroundFill>)>>,
        Query<&mut Sprite, With<ExteriorArt>>,
        Query<&mut Sprite, With<InteriorArt>>,
        Query<(&LampGlow, &mut Sprite)>,
        Query<&mut Sprite, With<Flake>>,
    )>,
    new_sprites: Query<(), Or<(Added<ExteriorArt>, Added<InteriorArt>, Added<LampGlow>)>>,
) {
    // Anche quando il treno viene ricostruito (nuovi sprite con il colore di default).
    if !day_night.is_changed() && new_sprites.is_empty() {
        return;
    }
    let hour = day_night.hour;
    let (base, over, t) = sky_blend(hour);
    for (part, mut sprite) in &mut sprites.p0() {
        let key = if part.0 { over } else { base };
        if let Some(image) = sky_images.0.get(key.index())
            && sprite.image != *image
        {
            sprite.image = image.clone();
        }
        sprite.color = Color::WHITE.with_alpha(if part.0 { t } else { 1.0 });
    }
    let horizon = mix(base.stops()[2], over.stops()[2], t);
    clear.0 = Color::srgb_u8(horizon[0], horizon[1], horizon[2]);
    let night = 1.0 - day_night.daylight;
    for mut sprite in &mut sprites.p1() {
        sprite.color = Color::WHITE.with_alpha(night * 0.9);
    }
    let ext = exterior_tint(hour);
    for mut sprite in &mut sprites.p2() {
        sprite.color = tint_color(ext);
    }
    let train_ext: [f32; 3] = std::array::from_fn(|i| ext[i] + (1.0 - ext[i]) * 0.3);
    for mut sprite in &mut sprites.p3() {
        sprite.color = tint_color(train_ext);
    }
    let inside = interior_tint(hour);
    for mut sprite in &mut sprites.p4() {
        sprite.color = tint_color(inside);
    }
    let lamp = lamp_level(hour);
    for (glow, mut sprite) in &mut sprites.p5() {
        sprite.color = Color::WHITE.with_alpha(glow.strength * lamp);
    }
    let flake = [ext[0].max(0.55), ext[1].max(0.6), ext[2].max(0.75)];
    for mut sprite in &mut sprites.p6() {
        let alpha = sprite.color.alpha();
        sprite.color = tint_color(flake).with_alpha(alpha);
    }
}

/// Vista della camera: centro e dimensioni (in unità mondo).
fn camera_view(camera: &Query<(&Transform, &Projection), With<Camera2d>>) -> Option<(Vec2, Vec2)> {
    let (transform, projection) = camera.iter().next()?;
    let Projection::Orthographic(ortho) = projection else {
        return None;
    };
    let size = ortho.area.size();
    if size.x <= 0.0 || size.y <= 0.0 {
        return None;
    }
    Some((transform.translation.truncate(), size))
}

/// Sposta gli strati (parallasse + scorrimento), il cielo, le stelle e gli astri.
#[allow(clippy::type_complexity)]
fn scroll_layers(
    time: Res<Time<Real>>,
    day_night: Res<DayNight>,
    camera: Query<(&Transform, &Projection), With<Camera2d>>,
    mut layers: Query<(&Layer, &mut Sprite, &mut Transform), Without<Camera2d>>,
    mut sky: Query<
        (&mut Sprite, &mut Transform),
        (
            Or<(With<SkyPart>, With<Stars>)>,
            Without<Layer>,
            Without<Camera2d>,
        ),
    >,
    mut orbs: Query<
        (&Orb, &mut Sprite, &mut Transform),
        (Without<Layer>, Without<SkyPart>, Without<Stars>, Without<Camera2d>),
    >,
    mut fill: Query<
        (&mut Sprite, &mut Transform),
        (
            With<GroundFill>,
            Without<Layer>,
            Without<SkyPart>,
            Without<Stars>,
            Without<Orb>,
            Without<Camera2d>,
        ),
    >,
) {
    let Some((center, view)) = camera_view(&camera) else {
        return;
    };
    let travelled = time.elapsed_secs_f64() * f64::from(TRAIN_SPEED);
    let margin = 8.0;
    let view_left = center.x - view.x / 2.0 - margin;
    for (layer, mut sprite, mut transform) in &mut layers {
        let spec = &LAYERS[layer.0];
        let width = spec.width as f32;
        let origin = pattern_origin(center.x, travelled, spec.depth, width);
        let left = first_tile_left(origin, view_left, width);
        let tiles = tiles_needed(view.x + 2.0 * margin, width);
        let size = Vec2::new(tiles as f32 * width, spec.height as f32);
        if sprite.custom_size != Some(size) {
            sprite.custom_size = Some(size);
        }
        transform.translation.x = left + size.x / 2.0;
    }
    let cover = view + Vec2::splat(2.0 * margin);
    for (mut sprite, mut transform) in &mut sky {
        if sprite.custom_size != Some(cover) {
            sprite.custom_size = Some(cover);
        }
        transform.translation.x = center.x;
        transform.translation.y = center.y;
    }
    // Sole e luna attraversano il cielo in un arco, relativo alla vista.
    let hour = day_night.hour;
    for (orb, mut sprite, mut transform) in &mut orbs {
        let rise = if orb.0 { 19.0 } else { 6.0 };
        let t = (hour - rise).rem_euclid(24.0) / 12.0;
        let visible = t < 1.0;
        let x = center.x + (t - 0.5) * view.x * 0.8;
        let y = center.y + view.y * (0.05 + 0.38 * (t * PI).sin());
        transform.translation.x = x;
        transform.translation.y = y;
        let alpha = if !visible {
            0.0
        } else if orb.0 {
            1.0 - day_night.daylight
        } else {
            day_night.daylight
        };
        if (sprite.color.alpha() - alpha).abs() > 1e-3 {
            sprite.color = Color::WHITE.with_alpha(alpha);
        }
    }
    let bottom = LAYERS[LAYERS.len() - 1].bottom;
    for (mut sprite, mut transform) in &mut fill {
        let depth = (bottom - (center.y - view.y / 2.0) + margin).max(1.0);
        let size = Vec2::new(cover.x, depth);
        if sprite.custom_size != Some(size) {
            sprite.custom_size = Some(size);
        }
        transform.translation.x = center.x;
        transform.translation.y = bottom - depth / 2.0;
    }
}

/// Neve: i fiocchi scorrono verso sinistra (il treno corre) e cadono,
/// ricomparendo dall'altro lato della vista.
fn move_snow(
    time: Res<Time<Real>>,
    camera: Query<(&Transform, &Projection), With<Camera2d>>,
    mut flakes: Query<(&mut Flake, &mut Transform, &mut Sprite), Without<Camera2d>>,
) {
    let Some((center, view)) = camera_view(&camera) else {
        return;
    };
    let dt = time.delta_secs().min(0.1);
    let t = time.elapsed_secs();
    let area = view + Vec2::splat(16.0);
    for (mut flake, mut transform, mut sprite) in &mut flakes {
        let step = flake.velocity * dt / area;
        flake.pos = (flake.pos + step).rem_euclid(Vec2::ONE);
        let sway = (t * 1.7 + flake.phase).sin() * 3.0;
        let offset = (flake.pos - Vec2::splat(0.5)) * area;
        transform.translation.x = center.x + offset.x + sway;
        transform.translation.y = center.y + offset.y;
        if sprite.color.alpha() != flake.alpha {
            sprite.color.set_alpha(flake.alpha);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_wrap_around_the_view() {
        let width = 256.0;
        for (camera, travelled, depth) in [
            (0.0, 0.0, 1.0),
            (1234.5, 98_765.4, 0.45),
            (-50.0, 1e9, 0.03),
            (6700.0, 3.3, 0.2),
        ] {
            let origin = pattern_origin(camera, travelled, depth, width);
            let view_left = camera - 220.0;
            let left = first_tile_left(origin, view_left, width);
            assert!(left <= view_left + 1e-3, "{left} > {view_left}");
            assert!(left > view_left - width - 1e-3);
            let k = (left - origin) / width;
            assert!((k - k.round()).abs() < 1e-3, "non allineato al motivo: {k}");
            // Le ripetizioni coprono tutta la vista.
            let tiles = tiles_needed(440.0, width);
            assert!(left + tiles as f32 * width >= camera + 220.0);
        }
    }

    #[test]
    fn far_layers_follow_the_camera_near_ones_the_world() {
        // Profondità 1 (binari): la camera non sposta il motivo.
        assert_eq!(
            pattern_origin(0.0, 10.0, 1.0, 256.0),
            pattern_origin(500.0, 10.0, 1.0, 256.0)
        );
        // Profondità 0 (cielo): il motivo resta attaccato alla camera.
        assert_eq!(pattern_origin(500.0, 10.0, 0.0, 256.0), 500.0);
        // Il paesaggio scorre verso sinistra col passare del tempo.
        let a = pattern_origin(0.0, 10.0, 0.5, 256.0);
        let b = pattern_origin(0.0, 20.0, 0.5, 256.0);
        assert!(b < a);
    }

    #[test]
    fn day_and_night() {
        assert_eq!(daylight(2.0), 0.0);
        assert_eq!(daylight(12.0), 1.0);
        assert_eq!(daylight(23.0), 0.0);
        assert!(daylight(6.5) > 0.0 && daylight(6.5) < 1.0);
        assert!(daylight(19.0) > 0.0 && daylight(19.0) < 1.0);
        assert_eq!(sky_blend(12.0), (SkyKey::Day, SkyKey::Day, 0.0));
        assert_eq!(sky_blend(1.0).0, SkyKey::Night);
        assert_eq!(sky_blend(22.0).0, SkyKey::Night);
        let (a, b, t) = sky_blend(18.0);
        assert_eq!((a, b), (SkyKey::Day, SkyKey::Dusk));
        assert!(t > 0.0 && t < 1.0);
        // L'esterno di notte è più scuro e più blu; di giorno è neutro.
        let night = exterior_tint(0.0);
        assert!(night[2] > night[0] && night[0] < 0.5);
        assert_eq!(exterior_tint(12.0), [1.0, 1.0, 1.0]);
        // Al tramonto è caldo.
        let dusk = exterior_tint(18.5);
        assert!(dusk[0] > dusk[2]);
        // Le lampade si vedono di più di notte.
        assert!(lamp_level(0.0) > lamp_level(12.0));
        assert!(interior_tint(0.0)[0] < interior_tint(12.0)[0]);
    }

    #[test]
    fn outside_art_tiles_seamlessly() {
        for spec in &LAYERS {
            let c = (spec.make)(spec.width, spec.height);
            assert_eq!((c.width as i32, c.height as i32), (spec.width, spec.height));
            // Prima e ultima colonna quasi uguali: la ripetizione non si vede.
            let differ = (0..c.height)
                .filter(|&y| {
                    let (a, b) = (c.get(0, y), c.get(c.width - 1, y));
                    a[3] != b[3]
                })
                .count();
            assert!(differ <= 12, "{}: {differ} righe diverse al bordo", spec.name);
            // Il fondo è pieno (nessun buco verso il cielo sotto l'orizzonte).
            assert_eq!(c.get(c.width / 2, c.height - 1)[3], 255, "{}", spec.name);
        }
        for key in SkyKey::ALL {
            let s = sky(key);
            assert_eq!((s.width as i32, s.height as i32), (SKY_W, SKY_H));
            assert!(s.pixels.iter().all(|p| p[3] == 255));
        }
        assert!(stars().pixels.iter().any(|p| p[3] > 0));
    }
}
