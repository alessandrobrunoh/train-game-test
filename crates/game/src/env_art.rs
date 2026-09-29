//! Pixel art dell'ambiente: carrozze (interni ed esterno), soffietti e
//! carrelli, più gli strumenti comuni per disegnare arte procedurale.
//!
//! Le immagini nascono una volta sola (per tipo di carrozza, per larghezza di
//! postazione...) e restano in [`ArtCache`]: nessuna generazione per frame.
//! Tutto è disegnato a 1 pixel d'arte = 1 unità mondo (la camera ingrandisce
//! 3×). Le funzioni di disegno qui usano coordinate "dal basso" (y = 0 è la
//! riga in fondo all'immagine), come il mondo di gioco; i modelli a righe di
//! testo (`Canvas::from_rows`) restano scritti dall'alto in basso.

use std::collections::HashMap;
use std::f32::consts::PI;

use bevy::prelude::*;
use sim::{CarriageKind, ItemKind};

use crate::art::{CLEAR, Canvas, Rgba};
use crate::train::{
    CARRIAGE_LENGTH, DOOR_HEIGHT, GANGWAY, INTERIOR_HEIGHT, STAIRS_LEFT, STAIRS_WIDTH, STOREY, WALL,
};

// --- Colori e disegno ---------------------------------------------------------

/// Colore opaco.
pub const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
    [r, g, b, 255]
}

/// Colore con trasparenza.
pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Rgba {
    [r, g, b, a]
}

/// Schiarisce (`f > 1`) o scurisce (`f < 1`) un colore, alfa invariato.
pub fn shade(c: Rgba, f: f32) -> Rgba {
    let ch = |v: u8| (f32::from(v) * f).round().clamp(0.0, 255.0) as u8;
    [ch(c[0]), ch(c[1]), ch(c[2]), c[3]]
}

/// Interpolazione lineare tra due colori (anche l'alfa).
pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    let ch = |i: usize| (f32::from(a[i]) + (f32::from(b[i]) - f32::from(a[i])) * t).round() as u8;
    [ch(0), ch(1), ch(2), ch(3)]
}

/// Rumore deterministico: un intero pseudo-casuale per ogni (x, y, seme).
pub fn hash2(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x27d4_eb2d)
        ^ (y as u32).wrapping_mul(0x1656_67b1)
        ^ seed.wrapping_mul(0x9e37_79b9);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    h
}

/// Riga dell'immagine per la quota `y` contata dal basso.
fn row(c: &Canvas, y: i32) -> i32 {
    c.height as i32 - 1 - y
}

/// Un pixel in coordinate dal basso; fuori dai bordi non fa nulla.
pub fn px(c: &mut Canvas, x: i32, y: i32, color: Rgba) {
    let r = row(c, y);
    if x >= 0 && r >= 0 {
        c.set(x as u32, r as u32, color);
    }
}

/// Legge un pixel in coordinate dal basso (trasparente fuori dai bordi).
pub fn get(c: &Canvas, x: i32, y: i32) -> Rgba {
    let r = row(c, y);
    if x < 0 || r < 0 || x >= c.width as i32 || r >= c.height as i32 {
        CLEAR
    } else {
        c.get(x as u32, r as u32)
    }
}

/// Rettangolo pieno con angolo in basso a sinistra in (`x`, `y`).
pub fn rect(c: &mut Canvas, x: i32, y: i32, w: i32, h: i32, color: Rgba) {
    for yy in y..y + h {
        for xx in x..x + w {
            px(c, xx, yy, color);
        }
    }
}

/// Disegna `sprite` con l'angolo in basso a sinistra in (`x`, `y`).
pub fn stamp(c: &mut Canvas, sprite: &Canvas, x: i32, y: i32) {
    let top = row(c, y + sprite.height as i32 - 1);
    c.overlay(sprite, x, top);
}

/// Segmento (Bresenham) tra due punti, estremi inclusi.
pub fn line(c: &mut Canvas, (x0, y0): (i32, i32), (x1, y1): (i32, i32), color: Rgba) {
    let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
    let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
    let (mut x, mut y, mut err) = (x0, y0, dx + dy);
    loop {
        px(c, x, y, color);
        if x == x1 && y == y1 {
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
}

/// Rivetto: un punto chiaro con l'ombra sotto.
fn rivet(c: &mut Canvas, x: i32, y: i32, base: Rgba) {
    px(c, x, y, shade(base, 1.45));
    px(c, x, y - 1, shade(base, 0.65));
}

// --- Caratteri 3×5 ------------------------------------------------------------

/// Righe di un carattere 3×5 (cifre e poche lettere), `None` se manca.
fn glyph(ch: char) -> Option<[&'static str; 5]> {
    Some(match ch {
        '0' => ["###", "#.#", "#.#", "#.#", "###"],
        '1' => [".#.", "##.", ".#.", ".#.", "###"],
        '2' => ["###", "..#", "###", "#..", "###"],
        '3' => ["###", "..#", ".##", "..#", "###"],
        '4' => ["#.#", "#.#", "###", "..#", "..#"],
        '5' => ["###", "#..", "###", "..#", "###"],
        '6' => ["###", "#..", "###", "#.#", "###"],
        '7' => ["###", "..#", ".#.", ".#.", ".#."],
        '8' => ["###", "#.#", "###", "#.#", "###"],
        '9' => ["###", "#.#", "###", "..#", "###"],
        'M' => ["#.#", "###", "###", "#.#", "#.#"],
        'E' => ["###", "#..", "##.", "#..", "###"],
        'N' => ["##.", "#.#", "#.#", "#.#", "#.#"],
        'U' => ["#.#", "#.#", "#.#", "#.#", "###"],
        _ => return None,
    })
}

/// Testo 3×5 (spazio di 1 pixel tra i caratteri) in un solo colore.
pub fn text3x5(text: &str, color: Rgba) -> Canvas {
    let glyphs: Vec<_> = text.chars().filter_map(glyph).collect();
    let width = (glyphs.len() * 4).saturating_sub(1) as u32;
    let mut c = Canvas::new(width.max(1), 5);
    for (i, g) in glyphs.iter().enumerate() {
        let glyph = Canvas::from_rows(g, &[('#', color)]);
        c.overlay(&glyph, i as i32 * 4, 0);
    }
    c
}

// --- Cache delle immagini -----------------------------------------------------

/// Chiave di un'immagine generata: una per tipo, dimensione e variante.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ArtKey {
    Interior(CarriageKind),
    Body {
        kind: CarriageKind,
        floors: u8,
    },
    Ladder,
    EndWall {
        door: bool,
    },
    Gangway,
    BogieFrame,
    Wheel(u8),
    Glow(GlowKind),
    Bed {
        width: u16,
        upper: bool,
        blanket: u8,
    },
    BedPlate(u16),
    Table(u16),
    Bench(u16),
    Stove(u16),
    Steam(u8),
    GrowBed {
        width: u16,
        variant: u8,
    },
    Workbench(u16),
    Sparks(u8),
    Counter {
        width: u16,
        hue: u8,
    },
    Shelf,
    /// Cabina del giocatore: baule, paravento e tappeto.
    Chest(u16),
    Screen,
    Rug(u16),
    Crate(ItemKind),
    Good(ItemKind),
    /// Arte dell'esterno (vedi `background.rs`).
    Outside(u8),
}

/// Immagini generate, riusate da tutte le entità che le mostrano.
///
/// Senza `Assets<Image>` (i test senza rendering) restituisce l'handle di
/// default e non genera nulla.
#[derive(Resource, Default)]
pub struct ArtCache {
    handles: HashMap<ArtKey, Handle<Image>>,
    /// Quante immagini sono state generate finora.
    pub generated: usize,
}

impl ArtCache {
    /// L'immagine di `key`, generata con `make` la prima volta.
    pub fn get(
        &mut self,
        images: Option<&mut Assets<Image>>,
        key: ArtKey,
        make: impl FnOnce() -> Canvas,
    ) -> Handle<Image> {
        if let Some(handle) = self.handles.get(&key) {
            return handle.clone();
        }
        let Some(images) = images else {
            return Handle::default();
        };
        let handle = images.add(make().to_image());
        self.generated += 1;
        self.handles.insert(key, handle.clone());
        handle
    }
}

/// Sprite con un'immagine della cache, disegnato alla dimensione `size`.
pub fn art_sprite(image: Handle<Image>, size: Vec2) -> Sprite {
    Sprite {
        image,
        custom_size: Some(size),
        ..default()
    }
}

// --- Componenti per luce e animazioni ------------------------------------------

/// Parete di fondo di una carrozza: più scura di notte (vedi `background.rs`).
#[derive(Component, Debug)]
pub struct InteriorArt;

/// Esterno del treno (tetto, telaio, carrelli, soffietti): segue la luce del giorno.
#[derive(Component, Debug)]
pub struct ExteriorArt;

/// Alone di luce di una lampada: più intenso di notte.
#[derive(Component, Debug)]
pub struct LampGlow {
    /// Intensità massima (alfa) dell'alone.
    pub strength: f32,
}

// --- Geometria delle carrozze -------------------------------------------------

/// Finestrini sulla parete di fondo: centri (x locali), base, larghezza, altezza.
pub const WINDOW_CENTERS: [i32; 4] = [40, 120, 200, 280];
pub const WINDOW_Y: i32 = 52;
pub const WINDOW_W: i32 = 36;
pub const WINDOW_H: i32 = 28;
/// Lampade sul soffitto (x locali).
pub const LAMP_XS: [i32; 3] = [80, 160, 240];
/// Quota della lampadina, dove si centra l'alone.
pub const LAMP_Y: f32 = 88.0;
/// Altezza del rivestimento basso delle pareti.
const WAINSCOT: i32 = 32;

/// Base del telaio sotto il pavimento e cima del tetto (quote mondo).
pub const BODY_BOTTOM: f32 = -14.0;
pub const ROOF_TOP: f32 = 122.0;

/// Carrelli: larghezza, distanza del centro dalla testata, ruote.
pub const BOGIE_W: f32 = 48.0;
pub const BOGIE_H: f32 = 12.0;
pub const BOGIE_INSET: f32 = 40.0;
pub const WHEEL_SIZE: f32 = 10.0;
/// Distanza delle ruote dal centro del carrello.
pub const WHEEL_OFFSET: f32 = 14.0;
/// Fotogrammi della rotazione delle ruote (un quarto di giro, 4 raggi).
pub const WHEEL_FRAMES: u8 = 4;

/// Soffietto: dal fondo del pavimento alla cima dei mantici esterni.
pub const GANGWAY_BOTTOM: f32 = -WALL;
pub const GANGWAY_TOP: f32 = 110.0;

/// Colori di una carrozza.
struct WallStyle {
    wall: Rgba,
    wainscot: Rgba,
    trim: Rgba,
    frame: Rgba,
    floor: Rgba,
}

fn wall_style(kind: CarriageKind) -> WallStyle {
    match kind {
        CarriageKind::Dormitorio => WallStyle {
            wall: rgb(52, 58, 86),
            wainscot: rgb(40, 43, 64),
            trim: rgb(30, 32, 48),
            frame: rgb(72, 76, 96),
            floor: rgb(92, 58, 64),
        },
        CarriageKind::Mensa => WallStyle {
            wall: rgb(150, 102, 64),
            wainscot: rgb(210, 202, 182),
            trim: rgb(104, 66, 38),
            frame: rgb(112, 74, 44),
            floor: rgb(196, 186, 164),
        },
        CarriageKind::Serra => WallStyle {
            wall: rgb(62, 90, 68),
            wainscot: rgb(96, 72, 46),
            trim: rgb(50, 64, 50),
            frame: rgb(88, 110, 90),
            floor: rgb(112, 86, 54),
        },
        CarriageKind::Officina => WallStyle {
            wall: rgb(86, 88, 94),
            wainscot: rgb(58, 56, 56),
            trim: rgb(44, 44, 48),
            frame: rgb(110, 112, 118),
            floor: rgb(118, 120, 126),
        },
        CarriageKind::Mercato => WallStyle {
            wall: rgb(116, 46, 44),
            wainscot: rgb(106, 70, 42),
            trim: rgb(70, 40, 26),
            frame: rgb(150, 110, 60),
            floor: rgb(142, 96, 58),
        },
    }
}

// --- Parete di fondo -------------------------------------------------------------

/// Parete di fondo della carrozza (larga quanto la carrozza, alta quanto
/// l'interno) con i finestrini trasparenti: dietro si vede l'esterno.
pub fn interior(kind: CarriageKind) -> Canvas {
    let s = wall_style(kind);
    let (w, h) = (CARRIAGE_LENGTH as i32, INTERIOR_HEIGHT as i32);
    let mut c = Canvas::new(w as u32, h as u32);

    // Muro con una grana leggera.
    rect(&mut c, 0, 0, w, h, s.wall);
    for y in WAINSCOT..h {
        for x in 0..w {
            if hash2(x, y, 11).is_multiple_of(13) {
                px(&mut c, x, y, shade(s.wall, 0.92));
            }
        }
    }
    // Giunti verticali dei pannelli, con rivetti.
    for sx in (40..w).step_by(80) {
        rect(
            &mut c,
            sx,
            WAINSCOT + 2,
            1,
            h - WAINSCOT - 8,
            shade(s.wall, 0.78),
        );
        rect(
            &mut c,
            sx + 1,
            WAINSCOT + 2,
            1,
            h - WAINSCOT - 8,
            shade(s.wall, 1.12),
        );
    }
    // Rivestimento basso, battiscopa, cornice sotto il soffitto.
    rect(&mut c, 0, 0, w, WAINSCOT, s.wainscot);
    rect(&mut c, 0, 0, w, 2, s.trim);
    rect(&mut c, 0, WAINSCOT, w, 2, s.trim);
    rect(&mut c, 0, WAINSCOT + 1, w, 1, shade(s.trim, 1.35));
    rect(&mut c, 0, h - 6, w, 6, s.trim);
    rect(&mut c, 0, h - 6, w, 1, shade(s.trim, 0.7));
    rect(&mut c, 0, h - 2, w, 1, shade(s.trim, 1.3));

    match kind {
        CarriageKind::Dormitorio => dormitorio_decor(&mut c, &s),
        CarriageKind::Mensa => mensa_decor(&mut c, &s),
        CarriageKind::Serra => serra_decor(&mut c, &s),
        CarriageKind::Officina => officina_decor(&mut c, &s),
        CarriageKind::Mercato => mercato_decor(&mut c, &s),
    }

    for cx in WINDOW_CENTERS {
        window(&mut c, cx, &s);
    }
    for x in LAMP_XS {
        match kind {
            CarriageKind::Serra => grow_light(&mut c, x),
            CarriageKind::Mercato => lantern(&mut c, x, x / 80),
            CarriageKind::Officina => cage_lamp(&mut c, x),
            _ => ceiling_lamp(&mut c, x, kind == CarriageKind::Dormitorio),
        }
    }
    c
}

/// Finestrino: telaio, davanzale e vetro quasi trasparente con un riflesso.
fn window(c: &mut Canvas, cx: i32, s: &WallStyle) {
    let (x0, y0, w, h) = (cx - WINDOW_W / 2, WINDOW_Y, WINDOW_W, WINDOW_H);
    // Ombra attorno, telaio e davanzale.
    rect(c, x0 - 1, y0 - 1, w + 2, h + 2, shade(s.wall, 0.7));
    rect(c, x0, y0, w, h, s.frame);
    rect(c, x0, y0 + h - 1, w, 1, shade(s.frame, 1.3));
    rect(c, x0 - 3, y0 - 3, w + 6, 3, s.trim);
    rect(c, x0 - 3, y0 - 1, w + 6, 1, shade(s.trim, 1.4));
    // Vetro: foro con angoli arrotondati.
    let (gx, gy, gw, gh) = (x0 + 2, y0 + 2, w - 4, h - 4);
    let glass = rgba(200, 228, 255, 26);
    rect(c, gx, gy, gw, gh, glass);
    for (x, y) in [
        (gx, gy),
        (gx + gw - 1, gy),
        (gx, gy + gh - 1),
        (gx + gw - 1, gy + gh - 1),
    ] {
        px(c, x, y, s.frame);
    }
    // Traversa del vasistas in alto.
    rect(c, gx, gy + gh - 7, gw, 1, s.frame);
    // Riflessi diagonali.
    let glint = rgba(235, 245, 255, 70);
    for t in 0..8 {
        px(c, gx + 3 + t, gy + 3 + t, glint);
        px(c, gx + 6 + t, gy + 3 + t, glint);
    }
    // Guarnizione scura sotto il vetro.
    rect(c, gx, gy, gw, 1, rgba(20, 24, 32, 90));
}

/// Lampada a sospensione con paralume.
fn ceiling_lamp(c: &mut Canvas, x: i32, dim: bool) {
    let metal = rgb(52, 50, 54);
    rect(c, x, 94, 1, 4, rgb(24, 24, 28));
    rect(c, x - 1, 93, 3, 1, metal);
    rect(c, x - 2, 92, 5, 1, metal);
    rect(c, x - 3, 91, 7, 1, shade(metal, 1.3));
    let bulb = if dim {
        rgb(230, 190, 120)
    } else {
        rgb(255, 236, 170)
    };
    rect(c, x - 1, 90, 3, 1, bulb);
    px(c, x, 89, shade(bulb, 1.1));
}

/// Lampada industriale con gabbia (Officina).
fn cage_lamp(c: &mut Canvas, x: i32) {
    let metal = rgb(40, 40, 44);
    rect(c, x, 93, 1, 5, metal);
    rect(c, x - 3, 92, 7, 1, rgb(200, 160, 40));
    rect(c, x - 2, 88, 5, 4, rgb(255, 230, 160));
    for yy in [88, 90] {
        rect(c, x - 2, yy, 5, 1, metal);
    }
    rect(c, x, 88, 1, 4, metal);
}

/// Lampada per la crescita delle piante (Serra): un lungo tubo viola.
fn grow_light(c: &mut Canvas, x: i32) {
    let housing = rgb(46, 52, 48);
    for dx in [-22, 22] {
        rect(c, x + dx, 94, 1, 4, rgb(30, 34, 30));
    }
    rect(c, x - 26, 92, 53, 2, housing);
    rect(c, x - 26, 93, 53, 1, shade(housing, 1.3));
    rect(c, x - 25, 91, 51, 1, rgb(248, 150, 240));
    rect(c, x - 24, 90, 49, 1, rgb(214, 112, 220));
}

/// Lanterna di carta appesa (Mercato), colore a rotazione.
fn lantern(c: &mut Canvas, x: i32, variant: i32) {
    let paper = match variant % 3 {
        0 => rgb(220, 70, 50),
        1 => rgb(236, 160, 50),
        _ => rgb(200, 60, 110),
    };
    rect(c, x, 94, 1, 4, rgb(30, 22, 18));
    rect(c, x - 2, 93, 5, 1, rgb(60, 40, 20));
    rect(c, x - 3, 88, 7, 5, paper);
    rect(c, x - 4, 89, 9, 3, paper);
    for yy in [89, 91] {
        rect(c, x - 4, yy, 9, 1, shade(paper, 0.8));
    }
    rect(c, x - 1, 89, 3, 3, mix(paper, rgb(255, 240, 180), 0.6));
    rect(c, x - 2, 87, 5, 1, rgb(60, 40, 20));
    px(c, x, 86, rgb(236, 190, 60));
}

/// Dormitorio: tubi lungo il soffitto con staffe e valvole, poster sbiaditi.
fn dormitorio_decor(c: &mut Canvas, s: &WallStyle) {
    let w = c.width as i32;
    let copper = rgb(126, 96, 76);
    let steel = rgb(84, 94, 110);
    rect(c, 0, 94, w, 3, copper);
    rect(c, 0, 96, w, 1, shade(copper, 1.3));
    rect(c, 0, 94, w, 1, shade(copper, 0.6));
    rect(c, 0, 90, w, 2, steel);
    rect(c, 0, 91, w, 1, shade(steel, 1.3));
    for x in (12..w).step_by(40) {
        rect(c, x, 89, 1, 9, s.trim);
    }
    // Valvole a volantino.
    for x in [20, 300] {
        rect(c, x - 2, 85, 5, 5, rgb(160, 50, 40));
        rect(c, x - 1, 86, 3, 3, s.wall);
        px(c, x, 87, rgb(160, 50, 40));
        rect(c, x, 90, 1, 1, rgb(160, 50, 40));
    }
    // Poster ingialliti tra i finestrini.
    for (i, x) in [72, 152, 232].into_iter().enumerate() {
        let paper = [rgb(150, 140, 110), rgb(120, 130, 140), rgb(140, 110, 100)][i];
        rect(c, x, 58, 16, 20, paper);
        rect(c, x + 2, 70, 12, 5, shade(paper, 0.6));
        rect(c, x + 2, 62, 12, 1, shade(paper, 0.7));
        rect(c, x + 2, 64, 9, 1, shade(paper, 0.7));
        px(c, x, 77, shade(paper, 0.5));
        px(c, x + 15, 77, shade(paper, 0.5));
    }
    // Rivestimento basso a doghe.
    for x in (0..w).step_by(10) {
        rect(c, x, 2, 1, WAINSCOT - 2, shade(s.wainscot, 0.8));
    }
}

/// Mensa: piastrelle, lavagna del menù e rastrelliera dei piatti.
fn mensa_decor(c: &mut Canvas, s: &WallStyle) {
    let w = c.width as i32;
    let grout = shade(s.wainscot, 0.8);
    for y in 2..WAINSCOT {
        for x in 0..w {
            let (tx, ty) = (x / 6, (y - 2) / 6);
            let color = if x % 6 == 0 || (y - 2) % 6 == 0 {
                grout
            } else if (tx + ty) % 2 == 0 {
                s.wainscot
            } else {
                shade(s.wainscot, 0.9)
            };
            px(c, x, y, color);
        }
    }
    // Fascia decorativa sopra le piastrelle.
    for x in (0..w).step_by(4) {
        rect(c, x, WAINSCOT + 3, 2, 1, rgb(170, 60, 40));
    }
    // Lavagna del menù.
    let (bx, by, bw, bh) = (62, 53, 36, 26);
    rect(c, bx, by, bw, bh, s.trim);
    rect(c, bx + 2, by + 2, bw - 4, bh - 4, rgb(34, 48, 40));
    let chalk = rgb(222, 226, 214);
    stamp(c, &text3x5("MENU", chalk), bx + 11, by + bh - 9);
    for (i, len) in [18, 14, 20].into_iter().enumerate() {
        let y = by + 12 - i as i32 * 4;
        for x in 0..len {
            if !hash2(x, y, 5).is_multiple_of(5) {
                px(c, bx + 5 + x, y, rgba(210, 214, 200, 200));
            }
        }
        stamp(
            c,
            &text3x5(&format!("{}", i + 2), rgb(240, 210, 90)),
            bx + 27,
            y - 2,
        );
    }
    // Rastrelliera con i piatti.
    rect(c, 222, 58, 36, 2, s.trim);
    for i in 0..6 {
        let x = 224 + i * 6;
        rect(c, x, 60, 5, 5, rgb(226, 226, 232));
        rect(c, x + 1, 61, 3, 3, rgb(200, 204, 214));
        px(c, x + 2, 62, rgb(120, 150, 190));
    }
    rect(c, 222, 72, 36, 2, s.trim);
    for i in 0..5 {
        let x = 226 + i * 7;
        rect(c, x, 74, 4, 5, rgb(170, 110, 60));
        rect(c, x, 78, 4, 1, rgb(200, 200, 200));
    }
}

/// Serra: traliccio con rampicanti e doghe di legno.
fn serra_decor(c: &mut Canvas, s: &WallStyle) {
    let w = c.width as i32;
    let lattice = shade(s.wall, 1.18);
    for y in WAINSCOT + 2..INTERIOR_HEIGHT as i32 - 6 {
        for x in 0..w {
            if (x + y) % 12 == 0 || (x - y).rem_euclid(12) == 0 {
                px(c, x, y, lattice);
            }
        }
    }
    for x in (0..w).step_by(5) {
        rect(c, x, 2, 1, WAINSCOT - 2, shade(s.wainscot, 0.75));
    }
    // Rampicanti: salgono a zig-zag con foglie.
    let leaf = [rgb(70, 150, 64), rgb(104, 184, 80), rgb(46, 110, 52)];
    for (i, x0) in [8, 64, 100, 150, 176, 236, 262, 312]
        .into_iter()
        .enumerate()
    {
        let top = 60 + (hash2(i as i32, 0, 3) % 30) as i32;
        let mut x = x0;
        for y in WAINSCOT..top {
            if y % 5 == 0 {
                x += if (y / 5) % 2 == 0 { 1 } else { -1 };
            }
            px(c, x, y, rgb(56, 100, 50));
            if hash2(x, y, 9).is_multiple_of(3) {
                let l = leaf[(hash2(x, y, 4) % 3) as usize];
                let side = if y % 2 == 0 { 1 } else { -1 };
                px(c, x + side, y, l);
                px(c, x + 2 * side, y + 1, l);
                px(c, x + side, y + 1, l);
            }
        }
    }
}

/// Officina: lamiere rivettate, fascia di pericolo e pannelli porta-attrezzi.
fn officina_decor(c: &mut Canvas, s: &WallStyle) {
    let w = c.width as i32;
    let h = INTERIOR_HEIGHT as i32;
    for y in WAINSCOT + 2..h - 6 {
        for x in 0..w {
            let (lx, ly) = (x % 20, (y - WAINSCOT - 2) % 16);
            if lx == 0 || ly == 0 {
                px(c, x, y, shade(s.wall, 0.75));
            } else if lx == 1 || ly == 15 {
                px(c, x, y, shade(s.wall, 1.12));
            }
        }
    }
    for y in (WAINSCOT + 4..h - 6).step_by(16) {
        for x in (3..w).step_by(20) {
            rivet(c, x, y + 11, s.wall);
            rivet(c, x + 14, y + 11, s.wall);
        }
    }
    // Fascia gialla e nera.
    for x in 0..w {
        for y in WAINSCOT - 4..WAINSCOT {
            let yellow = (x + y).rem_euclid(8) < 4;
            px(
                c,
                x,
                y,
                if yellow {
                    rgb(214, 170, 40)
                } else {
                    rgb(30, 30, 30)
                },
            );
        }
    }
    // Macchie d'olio sul rivestimento basso.
    for i in 0..30 {
        let (x, y) = (
            (hash2(i, 1, 7) % w as u32) as i32,
            (hash2(i, 2, 7) % 20) as i32 + 3,
        );
        rect(c, x, y, 2, 1, shade(s.wainscot, 0.8));
    }
    // Pannelli forati con gli attrezzi.
    for bx in [62, 222] {
        tool_board(c, bx, 54);
    }
}

fn tool_board(c: &mut Canvas, x0: i32, y0: i32) {
    let (w, h) = (36, 24);
    let board = rgb(128, 96, 62);
    rect(c, x0 - 1, y0 - 1, w + 2, h + 2, rgb(60, 44, 30));
    rect(c, x0, y0, w, h, board);
    for y in (y0 + 1..y0 + h).step_by(3) {
        for x in (x0 + 1..x0 + w).step_by(3) {
            px(c, x, y, shade(board, 0.7));
        }
    }
    let steel = rgb(176, 182, 192);
    let grip = rgb(170, 50, 40);
    // Chiave inglese.
    rect(c, x0 + 4, y0 + 4, 2, 14, steel);
    rect(c, x0 + 3, y0 + 17, 4, 3, steel);
    px(c, x0 + 4, y0 + 19, board);
    // Martello.
    rect(c, x0 + 11, y0 + 3, 2, 13, rgb(120, 76, 40));
    rect(c, x0 + 9, y0 + 16, 6, 3, steel);
    // Seghetto.
    rect(c, x0 + 18, y0 + 12, 12, 2, steel);
    for x in (x0 + 18..x0 + 30).step_by(2) {
        px(c, x, y0 + 11, steel);
    }
    rect(c, x0 + 28, y0 + 12, 4, 6, grip);
    // Cacciaviti.
    for (i, col) in [grip, rgb(40, 110, 170)].into_iter().enumerate() {
        let x = x0 + 21 + i as i32 * 5;
        rect(c, x, y0 + 3, 1, 5, steel);
        rect(c, x - 1, y0 + 8, 3, 3, col);
    }
}

/// Mercato: carta da parati a rombi, festoni di bandierine e tappeti appesi.
fn mercato_decor(c: &mut Canvas, s: &WallStyle) {
    let w = c.width as i32;
    let h = INTERIOR_HEIGHT as i32;
    for y in WAINSCOT + 2..h - 6 {
        for x in 0..w {
            let (lx, ly) = (x % 10, (y - WAINSCOT) % 10);
            if (lx - 5).abs() + (ly - 5).abs() == 4 {
                px(c, x, y, shade(s.wall, 1.25));
            } else if lx == 5 && ly == 5 {
                px(c, x, y, rgb(200, 150, 70));
            }
        }
    }
    for y in (2..WAINSCOT).step_by(5) {
        rect(c, 0, y, w, 1, shade(s.wainscot, 0.75));
    }
    for i in 0..40 {
        let (x, y) = (
            (hash2(i, 3, 2) % w as u32) as i32,
            3 + (hash2(i, 4, 2) % 5) as i32 * 5,
        );
        px(c, x, y + 2, shade(s.wainscot, 0.6));
    }
    // Tappeti appesi tra i finestrini.
    let rugs = [
        (rgb(40, 90, 150), rgb(230, 190, 80)),
        (rgb(40, 120, 90), rgb(230, 120, 60)),
        (rgb(150, 50, 110), rgb(90, 190, 190)),
    ];
    for (i, x0) in [68, 148, 228].into_iter().enumerate() {
        let (base, accent) = rugs[i];
        rect(c, x0, 50, 24, 30, base);
        rect(c, x0 + 2, 52, 20, 26, shade(base, 0.8));
        for y in 52..78 {
            for x in x0 + 2..x0 + 22 {
                let (dx, dy) = ((x - x0 - 12).abs(), (y - 65_i32).abs());
                if (dx + dy) % 6 == 0 {
                    px(c, x, y, accent);
                }
            }
        }
        rect(c, x0 - 1, 80, 26, 2, rgb(80, 50, 30));
        for x in (x0..x0 + 24).step_by(2) {
            px(c, x, 49, accent);
        }
    }
    // Festoni di bandierine sotto il soffitto.
    let flags = [
        rgb(220, 60, 50),
        rgb(240, 200, 60),
        rgb(60, 160, 90),
        rgb(60, 110, 200),
        rgb(236, 236, 226),
    ];
    for x in 0..w {
        let t = (x % 80) as f32 / 80.0;
        let sag = (4.0 * t * (1.0 - t) * 6.0).round() as i32;
        let y = 96 - sag;
        px(c, x, y, rgb(40, 30, 24));
        if x % 8 == 2 {
            let color = flags[((x / 8) % 5) as usize];
            for d in 0..4 {
                rect(c, x + d / 2, y - 1 - d, 4 - d, 1, color);
            }
        }
    }
}

// --- Esterno: tetto, pavimento, telaio ----------------------------------------------

/// Tetto, pavimento e telaio di una carrozza (senza le pareti di testata):
/// un'immagine larga quanto la carrozza da [`BODY_BOTTOM`] a [`ROOF_TOP`],
/// trasparente dove c'è l'interno.
pub fn body(kind: CarriageKind, floors: usize) -> Canvas {
    let s = wall_style(kind);
    let w = CARRIAGE_LENGTH as i32;
    let oy = -BODY_BOTTOM as i32;
    let rise = (floors.max(1) - 1) as i32 * STOREY as i32;
    let mut c = Canvas::new(w as u32, (ROOF_TOP - BODY_BOTTOM) as u32 + rise as u32);
    let ceil = INTERIOR_HEIGHT as i32 + rise + oy;
    let steel = rgb(96, 100, 110);
    let steel_dark = rgb(58, 60, 68);

    // Solai tra i piani: soffitto del piano sotto, pavimento di quello sopra,
    // col foro della scala.
    for f in 1..floors.max(1) as i32 {
        let top = oy + f * STOREY as i32;
        let slab = top - WALL as i32;
        rect(&mut c, 0, slab, w, WALL as i32, steel);
        rect(&mut c, 0, slab, w, 2, steel_dark);
        rect(&mut c, 0, slab + 1, w, 1, shade(steel_dark, 1.2));
        for x in (4..w).step_by(8) {
            rivet(&mut c, x, slab + 4, steel);
        }
        floor_surface(&mut c, kind, &s, top);
        let (hole, hole_w) = (STAIRS_LEFT as i32, STAIRS_WIDTH as i32);
        rect(&mut c, hole, slab, hole_w, WALL as i32, CLEAR);
        // Bordo del foro a strisce di pericolo.
        for x in [hole - 1, hole + hole_w] {
            for y in slab..top {
                let yellow = (y / 2) % 2 == 0;
                px(
                    &mut c,
                    x,
                    y,
                    if yellow {
                        rgb(214, 170, 40)
                    } else {
                        rgb(30, 30, 30)
                    },
                );
            }
        }
    }

    // Soffitto (visto da dentro) e fiancata del tetto.
    rect(&mut c, 0, ceil, w, 8, steel);
    rect(&mut c, 0, ceil, w, 2, steel_dark);
    rect(&mut c, 0, ceil + 1, w, 1, shade(steel_dark, 1.2));
    rect(&mut c, 0, ceil + 7, w, 1, shade(steel, 1.3));
    for x in (4..w).step_by(8) {
        rivet(&mut c, x, ceil + 5, steel);
    }
    // Cupola del tetto con la neve sopra.
    let roof = rgb(124, 130, 142);
    for (r, inset) in [1, 2, 3, 5, 8].into_iter().enumerate() {
        let y = ceil + 8 + r as i32;
        let color = if r == 4 { shade(roof, 1.25) } else { roof };
        rect(&mut c, inset, y, w - 2 * inset, 1, color);
    }
    if kind == CarriageKind::Serra {
        serra_roof(&mut c, ceil);
    }
    let top = ceil + 13;
    roof_fixtures(&mut c, kind, top);
    for x in 9..w - 9 {
        let bump = (hash2(x / 3, 0, 21) % 3) as i32;
        let depth = 1 + i32::from(bump == 2) + i32::from((x / 23) % 3 == 0);
        for d in 0..depth {
            if get(&c, x, top + d).eq(&CLEAR) {
                px(&mut c, x, top + d, rgb(236, 242, 250));
            }
        }
        px(&mut c, x, top - 1, rgb(196, 210, 230));
    }

    // Pavimento: superficie calpestabile e trave laterale rivettata.
    let floor = oy - WALL as i32;
    rect(&mut c, 0, floor, w, WALL as i32, steel_dark);
    rect(&mut c, 0, floor + 1, w, 4, rgb(72, 74, 84));
    rect(&mut c, 0, floor + 4, w, 1, shade(rgb(72, 74, 84), 1.25));
    for x in (3..w).step_by(8) {
        rivet(&mut c, x, floor + 3, rgb(72, 74, 84));
    }
    floor_surface(&mut c, kind, &s, oy);

    // Telaio sotto il pavimento, tra i carrelli: casse, serbatoio, tubi.
    let (fx0, fx1) = (68, w - 68);
    rect(&mut c, fx0, floor - 2, fx1 - fx0, 2, rgb(40, 40, 46));
    rect(&mut c, fx0, floor - 5, fx1 - fx0, 1, rgb(70, 60, 50));
    for (x, bw) in [(78, 26), (152, 18), (214, 24)] {
        rect(&mut c, x, 0, bw, floor - 2, rgb(50, 52, 60));
        rect(&mut c, x, floor - 3, bw, 1, rgb(80, 84, 94));
        rect(&mut c, x + 1, 1, 1, floor - 4, rgb(70, 74, 84));
        rivet(&mut c, x + bw - 3, floor - 5, rgb(50, 52, 60));
    }
    // Serbatoio cilindrico.
    rect(&mut c, 112, 1, 32, floor - 3, rgb(76, 84, 90));
    rect(&mut c, 111, 2, 34, floor - 5, rgb(76, 84, 90));
    rect(&mut c, 112, floor - 4, 32, 1, rgb(120, 130, 136));
    rect(&mut c, 112, 1, 32, 1, rgb(44, 48, 54));
    for x in [118, 138] {
        rect(&mut c, x, 1, 1, floor - 3, rgb(44, 48, 54));
    }
    c
}

/// Scala a pioli alta un piano, larga [`STAIRS_WIDTH`]: due montanti e i pioli.
pub fn ladder() -> Canvas {
    let (w, h) = (STAIRS_WIDTH as i32, STOREY as i32);
    let mut c = Canvas::new(w as u32, h as u32);
    let rail = rgb(120, 96, 64);
    let rung = rgb(150, 122, 82);
    for x in [2, w - 4] {
        rect(&mut c, x, 0, 2, h, rail);
        rect(&mut c, x, 0, 1, h, shade(rail, 1.3));
    }
    for y in (4..h).step_by(8) {
        rect(&mut c, 4, y, w - 8, 2, rung);
        rect(&mut c, 4, y + 1, w - 8, 1, shade(rung, 1.25));
    }
    c
}

/// Superficie del pavimento (2 righe) secondo il tipo di carrozza.
fn floor_surface(c: &mut Canvas, kind: CarriageKind, s: &WallStyle, oy: i32) {
    let w = c.width as i32;
    for x in 0..w {
        for (i, y) in [oy - 2, oy - 1].into_iter().enumerate() {
            let top = i == 1;
            let color = match kind {
                CarriageKind::Dormitorio => {
                    if (x + i as i32) % 4 == 0 {
                        shade(s.floor, 0.8)
                    } else {
                        s.floor
                    }
                }
                CarriageKind::Mensa => {
                    if (x / 4) % 2 == 0 {
                        s.floor
                    } else {
                        rgb(150, 62, 52)
                    }
                }
                CarriageKind::Serra => {
                    if x % 6 == 0 {
                        shade(s.floor, 0.55)
                    } else {
                        s.floor
                    }
                }
                CarriageKind::Officina => {
                    if (x + 2 * i as i32) % 5 == 0 {
                        shade(s.floor, 1.35)
                    } else {
                        s.floor
                    }
                }
                CarriageKind::Mercato => {
                    if x % 24 == 0 {
                        shade(s.floor, 0.6)
                    } else {
                        s.floor
                    }
                }
            };
            px(c, x, y, if top { shade(color, 1.12) } else { color });
        }
    }
    rect(c, 0, oy - 3, w, 1, rgb(30, 30, 36));
}

/// Serra: tetto a vetri con montanti, un po' trasparente.
fn serra_roof(c: &mut Canvas, ceil: i32) {
    let w = c.width as i32;
    let glass = rgba(150, 226, 196, 170);
    let bar = rgb(70, 90, 80);
    for y in ceil + 2..ceil + 12 {
        let inset = match y - ceil {
            9 => 2,
            10 => 3,
            11 => 5,
            _ => 1,
        };
        for x in inset..w - inset {
            let color = if x % 20 == 0 || y == ceil + 2 || y == ceil + 7 {
                bar
            } else if (x - y).rem_euclid(20) < 2 {
                rgba(220, 255, 240, 200)
            } else {
                glass
            };
            px(c, x, y, color);
        }
    }
}

/// Prese d'aria e comignoli sul tetto.
fn roof_fixtures(c: &mut Canvas, kind: CarriageKind, top: i32) {
    let vent = rgb(84, 88, 98);
    let xs: &[i32] = match kind {
        CarriageKind::Serra => &[150],
        _ => &[46, 150, 258],
    };
    for &x in xs {
        rect(c, x, top, 16, 5, vent);
        rect(c, x, top + 4, 16, 1, shade(vent, 1.3));
        for yy in [top + 1, top + 3] {
            rect(c, x + 2, yy, 12, 1, shade(vent, 0.55));
        }
        rect(c, x - 1, top + 5, 18, 1, rgb(236, 242, 250));
    }
    match kind {
        CarriageKind::Officina => {
            // Comignolo annerito.
            let x = 96;
            rect(c, x, top, 6, 10, rgb(50, 46, 44));
            rect(c, x - 1, top + 10, 8, 2, rgb(34, 32, 30));
            rect(c, x + 1, top, 1, 10, rgb(80, 74, 70));
        }
        CarriageKind::Mercato => {
            // Pennone con bandierina.
            let x = 300;
            rect(c, x, top, 1, 12, rgb(60, 44, 30));
            for d in 0..4 {
                rect(c, x + 1, top + 11 - d, 7 - 2 * d, 1, rgb(220, 60, 50));
                rect(c, x + 1, top + 8 + d, 7 - 2 * d, 1, rgb(220, 60, 50));
            }
        }
        CarriageKind::Dormitorio => {
            // Tubo di sfiato.
            let x = 206;
            rect(c, x, top, 3, 7, rgb(90, 80, 70));
            rect(c, x - 1, top + 7, 5, 1, rgb(60, 54, 48));
        }
        _ => {}
    }
}

/// Parete di testata (larga [`WALL`]): intera alle estremità del treno,
/// altrimenti solo l'architrave sopra il vano porta. Disegnata per il lato
/// sinistro; quella destra è specchiata.
pub fn end_wall(door: bool) -> Canvas {
    let w = WALL as i32;
    let bottom = if door { DOOR_HEIGHT as i32 } else { 0 };
    let h = INTERIOR_HEIGHT as i32 - bottom;
    let mut c = Canvas::new(w as u32, h as u32);
    let plate = rgb(74, 78, 90);
    rect(&mut c, 0, 0, w, h, plate);
    rect(&mut c, 0, 0, 1, h, shade(plate, 1.4));
    rect(&mut c, w - 1, 0, 1, h, shade(plate, 0.55));
    for y in (3..h).step_by(6) {
        rivet(&mut c, 2, y, plate);
        rivet(&mut c, 5, y, plate);
    }
    if door {
        // Bordo del vano porta a strisce di pericolo.
        for x in 0..w {
            for y in 0..2 {
                let yellow = (x + y) % 4 < 2;
                px(
                    &mut c,
                    x,
                    y,
                    if yellow {
                        rgb(214, 170, 40)
                    } else {
                        rgb(30, 30, 30)
                    },
                );
            }
        }
    } else {
        // Portello di emergenza sulla testata cieca.
        rect(&mut c, 1, 2, w - 2, 44, shade(plate, 0.85));
        rect(&mut c, 2, 3, w - 4, 42, plate);
        rect(&mut c, w - 3, 22, 1, 4, rgb(200, 170, 60));
    }
    c
}

/// Soffietto tra due carrozze: pedana, passaggio interno e mantici esterni.
pub fn gangway() -> Canvas {
    let w = GANGWAY as i32;
    let oy = -GANGWAY_BOTTOM as i32;
    let mut c = Canvas::new(w as u32, (GANGWAY_TOP - GANGWAY_BOTTOM) as u32);
    let door = DOOR_HEIGHT as i32 + oy;
    // Pedana di lamiera striata.
    let plate = rgb(96, 98, 106);
    rect(&mut c, 0, 0, w, oy, rgb(50, 52, 58));
    rect(&mut c, 0, oy - 2, w, 2, plate);
    for x in (0..w).step_by(3) {
        px(&mut c, x, oy - 1, shade(plate, 1.35));
    }
    // Passaggio: mantice visto da dentro, scuro, con una lucina.
    let inner = rgb(28, 28, 34);
    rect(&mut c, 0, oy, w, door - oy, inner);
    for x in (1..w).step_by(3) {
        rect(&mut c, x, oy, 1, door - oy, rgb(44, 44, 52));
    }
    rect(&mut c, w / 2 - 1, door - 4, 3, 2, rgb(240, 200, 120));
    // Architrave.
    rect(&mut c, 0, door, w, WALL as i32, rgb(74, 78, 90));
    rect(&mut c, 0, door + WALL as i32 - 1, w, 1, rgb(110, 114, 126));
    // Mantici esterni fino al tetto.
    let top = c.height as i32;
    let bellows_top = top - 3;
    for y in door + WALL as i32..bellows_top {
        for x in 0..w {
            let fold = if x % 4 < 2 {
                rgb(62, 62, 70)
            } else {
                rgb(40, 40, 48)
            };
            px(&mut c, x, y, fold);
        }
    }
    rect(&mut c, 1, bellows_top, w - 2, 1, rgb(80, 80, 90));
    rect(&mut c, 2, bellows_top + 1, w - 4, 2, rgb(236, 242, 250));
    c
}

/// Telaio di un carrello, dietro alle ruote (sprite separati, animati):
/// molle sotto la cassa, longherone sopra gli assi, ceppi dei freni.
pub fn bogie_frame() -> Canvas {
    let (w, h) = (BOGIE_W as i32, BOGIE_H as i32);
    let mut c = Canvas::new(w as u32, h as u32);
    let frame = rgb(46, 46, 52);
    let hi = rgb(84, 86, 96);
    let wheel_xs = [
        (BOGIE_W / 2.0 - WHEEL_OFFSET) as i32,
        (BOGIE_W / 2.0 + WHEEL_OFFSET) as i32,
    ];
    // Longherone con la trave centrale ribassata.
    rect(&mut c, 1, 6, w - 2, 3, frame);
    rect(&mut c, 1, 8, w - 2, 1, hi);
    rect(&mut c, 17, 2, w - 34, 5, frame);
    rect(&mut c, 18, 5, w - 36, 1, hi);
    // Molle tra telaio e cassa.
    for &x in &wheel_xs {
        for y in 9..h {
            let dx = if y % 2 == 0 { 0 } else { 1 };
            rect(&mut c, x - 3 + dx, y, 5, 1, rgb(150, 150, 160));
        }
    }
    // Ceppi dei freni alle estremità.
    for x in [0, w - 3] {
        rect(&mut c, x, 1, 3, 6, rgb(34, 34, 38));
    }
    c
}

/// Fotogramma `frame` della ruota (4 raggi, ruotati di 22,5° per fotogramma).
pub fn wheel(frame: u8) -> Canvas {
    let size = WHEEL_SIZE as i32;
    let mut c = Canvas::new(size as u32, size as u32);
    let center = (size as f32 - 1.0) / 2.0;
    let radius = size as f32 / 2.0;
    for y in 0..size {
        for x in 0..size {
            let d = Vec2::new(x as f32 - center, y as f32 - center).length();
            if d <= radius - 0.2 {
                let color = if d > radius - 1.4 {
                    rgb(28, 28, 32)
                } else if d > radius - 2.2 {
                    rgb(70, 70, 78)
                } else {
                    rgb(40, 40, 46)
                };
                px(&mut c, x, y, color);
            }
        }
    }
    let base = f32::from(frame) * (PI / 2.0) / f32::from(WHEEL_FRAMES);
    for k in 0..4 {
        let a = base + k as f32 * PI / 2.0;
        for step in 1..4 {
            let r = step as f32 + 0.3;
            let (x, y) = (center + a.cos() * r, center + a.sin() * r);
            px(
                &mut c,
                x.round() as i32,
                y.round() as i32,
                rgb(110, 110, 122),
            );
        }
    }
    let lo = (center.floor()) as i32;
    rect(&mut c, lo, lo, 2, 2, rgb(170, 170, 180));
    // Contrappeso: una macchia chiara che gira con la ruota.
    let a = base + PI / 4.0;
    let (x, y) = (center + a.cos() * 3.2, center + a.sin() * 3.2);
    px(&mut c, x.round() as i32, y.round() as i32, rgb(150, 60, 50));
    c
}

/// Tipo di alone di luce.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GlowKind {
    Warm,
    Grow,
    Lantern,
}

pub const GLOW_W: f32 = 64.0;
pub const GLOW_H: f32 = 56.0;

/// Alone ellittico sfumato a gradini (con un po' di retinatura ai bordi).
pub fn glow(kind: GlowKind) -> Canvas {
    let color = match kind {
        GlowKind::Warm => rgb(255, 214, 150),
        GlowKind::Grow => rgb(170, 255, 170),
        GlowKind::Lantern => rgb(255, 160, 90),
    };
    let (w, h) = (GLOW_W as i32, GLOW_H as i32);
    let mut c = Canvas::new(w as u32, h as u32);
    let (cx, cy) = ((w as f32 - 1.0) / 2.0, (h as f32 - 1.0) / 2.0);
    for y in 0..h {
        for x in 0..w {
            let d = Vec2::new((x as f32 - cx) / cx, (y as f32 - cy) / cy).length();
            // Gradini di intensità; tra un gradino e l'altro, scacchiera.
            let level = (1.0 - d) * 4.0;
            let dither = if (x + y) % 2 == 0 { 0.25 } else { -0.25 };
            let step = (level + dither).floor().clamp(0.0, 4.0);
            if step > 0.0 {
                let alpha = (step * 26.0) as u8;
                px(&mut c, x, y, [color[0], color[1], color[2], alpha]);
            }
        }
    }
    c
}

/// Alone adatto al tipo di carrozza e intensità massima.
pub fn glow_for(kind: CarriageKind) -> (GlowKind, f32) {
    match kind {
        CarriageKind::Dormitorio => (GlowKind::Warm, 0.55),
        CarriageKind::Serra => (GlowKind::Grow, 0.55),
        CarriageKind::Mercato => (GlowKind::Lantern, 0.9),
        _ => (GlowKind::Warm, 0.8),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interiors_match_the_carriage_and_differ_by_kind() {
        let all: Vec<Canvas> = CarriageKind::ALL.into_iter().map(interior).collect();
        for c in &all {
            assert_eq!((c.width, c.height), (320, 104));
        }
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn windows_let_the_outside_show_through() {
        let c = interior(CarriageKind::Mensa);
        for cx in WINDOW_CENTERS {
            let glass = get(&c, cx, WINDOW_Y + 8);
            assert!(glass[3] < 128, "vetro opaco a x = {cx}: {glass:?}");
        }
        // Il muro tra un finestrino e l'altro è opaco.
        assert_eq!(get(&c, 4, 40)[3], 255);
    }

    #[test]
    fn body_is_hollow_where_the_interior_is() {
        for kind in CarriageKind::ALL {
            let c = body(kind, 1);
            assert_eq!(c.width, CARRIAGE_LENGTH as u32);
            assert_eq!(c.height, (ROOF_TOP - BODY_BOTTOM) as u32);
            let oy = -BODY_BOTTOM as i32;
            assert_eq!(get(&c, 160, oy + 50), CLEAR);
            // Pavimento e soffitto sono pieni.
            assert_eq!(get(&c, 160, oy - 1)[3], 255);
            assert_eq!(get(&c, 160, oy + INTERIOR_HEIGHT as i32 + 3)[3], 255);
        }
    }

    #[test]
    fn two_floor_body_has_a_slab_with_a_stairwell() {
        let c = body(CarriageKind::Dormitorio, 2);
        let storey = STOREY as i32;
        assert_eq!(c.height, (ROOF_TOP - BODY_BOTTOM) as u32 + storey as u32);
        let oy = -BODY_BOTTOM as i32;
        let slab = oy + storey - 4;
        // Both storeys are hollow, the slab between them is solid...
        assert_eq!(get(&c, 160, oy + 50), CLEAR);
        assert_eq!(get(&c, 160, oy + storey + 50), CLEAR);
        assert_eq!(get(&c, 160, slab)[3], 255);
        // ...except above the ladder.
        let hole = (STAIRS_LEFT + STAIRS_WIDTH / 2.0) as i32;
        assert_eq!(get(&c, hole, slab), CLEAR);
        // The roof sits one storey higher.
        assert_eq!(
            get(&c, 160, oy + storey + INTERIOR_HEIGHT as i32 + 3)[3],
            255
        );
    }

    #[test]
    fn end_walls_and_gangway_fit_the_layout() {
        assert_eq!(end_wall(false).height, INTERIOR_HEIGHT as u32);
        assert_eq!(
            end_wall(true).height,
            (INTERIOR_HEIGHT - DOOR_HEIGHT) as u32
        );
        assert_eq!(end_wall(true).width, WALL as u32);
        let g = gangway();
        assert_eq!(g.width, GANGWAY as u32);
        assert_eq!(g.height, (GANGWAY_TOP - GANGWAY_BOTTOM) as u32);
    }

    #[test]
    fn wheel_frames_have_the_same_size_and_turn() {
        let frames: Vec<Canvas> = (0..WHEEL_FRAMES).map(wheel).collect();
        for f in &frames {
            assert_eq!((f.width, f.height), (WHEEL_SIZE as u32, WHEEL_SIZE as u32));
        }
        assert_ne!(frames[0], frames[1]);
        assert_eq!(bogie_frame().width, BOGIE_W as u32);
    }

    #[test]
    fn text_uses_three_by_five_glyphs() {
        let t = text3x5("12", rgb(1, 2, 3));
        assert_eq!((t.width, t.height), (7, 5));
        assert_eq!(t.get(1, 0), rgb(1, 2, 3));
        assert_eq!(t.get(3, 0), CLEAR);
    }

    #[test]
    fn color_helpers() {
        assert_eq!(shade(rgb(100, 200, 50), 0.5), rgb(50, 100, 25));
        assert_eq!(shade(rgb(200, 200, 200), 2.0), rgb(255, 255, 255));
        assert_eq!(mix(rgb(0, 0, 0), rgb(200, 100, 50), 0.5), rgb(100, 50, 25));
        assert_eq!(hash2(3, 4, 5), hash2(3, 4, 5));
        assert_ne!(hash2(3, 4, 5), hash2(4, 3, 5));
    }

    #[test]
    fn drawing_is_bottom_up_and_clipped() {
        let mut c = Canvas::new(4, 3);
        px(&mut c, 0, 0, rgb(9, 9, 9));
        assert_eq!(c.get(0, 2), rgb(9, 9, 9));
        rect(&mut c, 3, 2, 5, 5, rgb(1, 1, 1));
        assert_eq!(c.get(3, 0), rgb(1, 1, 1));
        px(&mut c, -1, 10, rgb(1, 1, 1));
        let mut big = Canvas::new(5, 5);
        stamp(
            &mut big,
            &Canvas::from_rows(&["a"], &[('a', rgb(7, 7, 7))]),
            1,
            0,
        );
        assert_eq!(get(&big, 1, 0), rgb(7, 7, 7));
    }

    #[test]
    fn cache_generates_each_key_once() {
        let mut images = Assets::<Image>::default();
        let mut cache = ArtCache::default();
        let mut calls = 0;
        let mut make = || {
            calls += 1;
            interior(CarriageKind::Serra)
        };
        let a = cache.get(
            Some(&mut images),
            ArtKey::Interior(CarriageKind::Serra),
            &mut make,
        );
        let b = cache.get(
            Some(&mut images),
            ArtKey::Interior(CarriageKind::Serra),
            &mut make,
        );
        assert_eq!(a, b);
        let c = cache.get(
            Some(&mut images),
            ArtKey::Interior(CarriageKind::Mensa),
            || interior(CarriageKind::Mensa),
        );
        assert_ne!(a, c);
        assert_eq!(calls, 1);
        assert_eq!(cache.generated, 2);
        // Senza immagini (test senza rendering): handle di default, niente cache.
        let none = cache.get(None, ArtKey::Gangway, gangway);
        assert_eq!(none, Handle::default());
        assert_eq!(cache.generated, 2);
    }
}
