//! Pixel art di postazioni e magazzini: letti a castello, tavoli, cucine,
//! aiuole, banchi da lavoro, banconi del mercato, scaffali e casse.
//!
//! Le postazioni hanno larghezze diverse secondo quante ce ne stanno nella
//! carrozza (vedi `stations.rs`), quindi sono disegnate in modo procedurale
//! alla larghezza richiesta (arrotondata al pixel) e tenute in cache per
//! larghezza. Casse e merce hanno una misura fissa e sono modelli a righe di
//! testo.

use sim::ItemKind;

use crate::art::{Canvas, Rgba};
use crate::env_art::{hash2, mix, px, rect, rgb, rgba, shade, text3x5};
use crate::stations::{BED_TOP, LEVEL_HEIGHT};

/// Altezza dell'immagine di un letto (sopra la sua base): testiera compresa.
pub const BED_H: i32 = 10;
/// Altezza dell'immagine della cucina, pentola compresa.
pub const STOVE_H: i32 = 23;
pub const TABLE_H: i32 = 14;
pub const BENCH_H: i32 = 5;
pub const GROW_H: i32 = 20;
pub const WORKBENCH_H: i32 = 22;
pub const COUNTER_H: i32 = 40;
/// Nuvoletta di vapore sopra la pentola e scintille sulla morsa.
pub const STEAM_W: i32 = 9;
pub const STEAM_H: i32 = 12;
pub const SPARKS_W: i32 = 9;
pub const SPARKS_H: i32 = 7;
/// Fotogrammi delle animazioni.
pub const STEAM_FRAMES: u8 = 3;
pub const SPARK_FRAMES: u8 = 3;
/// Quota della pentola sulla cucina e della morsa sul banco.
pub const POT_TOP: f32 = 22.0;
pub const VISE_TOP: f32 = 18.0;

// --- Letti --------------------------------------------------------------------

/// Colori delle coperte (per varietà tra un letto e l'altro); l'ultima,
/// gialla come la sciarpa, è quella del letto del giocatore.
const BLANKETS: [Rgba; 6] = [
    rgb(92, 104, 64),
    rgb(140, 56, 52),
    rgb(64, 84, 132),
    rgb(120, 92, 60),
    rgb(98, 72, 118),
    rgb(214, 170, 48),
];
/// Coperte delle cuccette degli NPC.
pub const BLANKET_VARIANTS: u8 = 5;
/// Coperta del letto del giocatore.
pub const PLAYER_BLANKET: u8 = 5;

/// Baule della cabina e paravento che la separa dal dormitorio.
pub const CHEST_H: i32 = 11;
pub const SCREEN_W: i32 = 5;
pub const SCREEN_H: i32 = 46;
pub const RUG_H: i32 = 2;

/// Cuccetta larga `w`: telaio, materasso, cuscino, coperta e testiere. I
/// letti dei piani alti (`upper`) hanno i montanti e una scaletta che
/// scendono fino al letto sotto: l'immagine parte `LEVEL_HEIGHT` più in basso.
pub fn bed(w: i32, upper: bool, blanket: u8) -> Canvas {
    let below = if upper { LEVEL_HEIGHT as i32 } else { 0 };
    let mut c = Canvas::new(w as u32, (BED_H + below) as u32);
    let b = below;
    let post = rgb(78, 74, 72);
    let frame = rgb(96, 70, 48);
    let top = BED_TOP as i32;

    // Montanti e scaletta verso il piano di sotto.
    rect(&mut c, 0, 0, 1, b + BED_H - 1, post);
    rect(&mut c, w - 1, 0, 1, b + BED_H - 1, post);
    if upper && w >= 8 {
        let lx = w - 5;
        rect(&mut c, lx, 0, 1, b + 3, post);
        for y in (3..b).step_by(4) {
            rect(&mut c, lx, y, 4, 1, shade(post, 1.3));
        }
    }
    // Telaio.
    rect(&mut c, 0, b, w, 3, frame);
    rect(&mut c, 0, b + 2, w, 1, shade(frame, 1.3));
    rect(&mut c, 0, b, w, 1, shade(frame, 0.7));
    // Materasso a righe.
    for x in 1..w - 1 {
        for y in b + 3..b + top {
            let color = if x % 3 == 0 {
                rgb(176, 180, 190)
            } else {
                rgb(206, 202, 186)
            };
            px(&mut c, x, y, color);
        }
    }
    // Coperta ripiegata sui due terzi verso i piedi.
    let blanket_color = BLANKETS[blanket as usize % BLANKETS.len()];
    let bx = (w * 2 / 5).max(3);
    rect(&mut c, bx, b + 3, w - 1 - bx, 3, blanket_color);
    rect(&mut c, bx, b + 5, w - 1 - bx, 1, shade(blanket_color, 1.25));
    rect(&mut c, bx, b + 3, 1, 3, shade(blanket_color, 1.45));
    for x in (bx + 3..w - 1).step_by(4) {
        px(&mut c, x, b + 4, shade(blanket_color, 0.75));
    }
    // Cuscino alla testa (a sinistra).
    let pw = (w / 4).clamp(2, 5);
    rect(&mut c, 1, b + top, pw, 2, rgb(236, 232, 220));
    px(&mut c, 1, b + top + 1, rgb(200, 196, 186));
    rect(&mut c, 1, b + top - 1, pw, 1, rgb(214, 210, 198));
    // Testiere con il pomolo.
    for x in [0, w - 1] {
        px(&mut c, x, b + BED_H - 1, shade(post, 1.5));
    }
    c
}

/// Targhetta di ottone con il numero della cuccetta.
pub fn bed_plate(number: u16) -> Canvas {
    let digits = text3x5(&number.to_string(), rgb(52, 36, 18));
    let (w, h) = (digits.width as i32 + 2, 7);
    let mut c = Canvas::new(w as u32, h as u32);
    rect(&mut c, 0, 0, w, h, rgb(176, 140, 70));
    rect(&mut c, 0, 0, w, 1, rgb(120, 92, 44));
    rect(&mut c, 0, h - 1, w, 1, rgb(214, 184, 110));
    c.overlay(&digits, 1, 1);
    c
}

// --- Cabina del giocatore --------------------------------------------------------

/// Baule di legno con le cerniere e la serratura d'ottone, largo `w`.
pub fn chest(w: i32) -> Canvas {
    let mut c = Canvas::new(w as u32, CHEST_H as u32);
    let wood = rgb(128, 84, 46);
    let band = rgb(70, 52, 40);
    let brass = rgb(214, 176, 84);
    // Cassa e coperchio bombato.
    rect(&mut c, 0, 0, w, 7, wood);
    rect(&mut c, 0, 0, w, 1, shade(wood, 0.6));
    rect(&mut c, 1, 7, w - 2, 3, shade(wood, 1.15));
    rect(&mut c, 2, 10, w - 4, 1, shade(wood, 1.3));
    rect(&mut c, 0, 6, w, 1, shade(wood, 0.75));
    // Fasce di ferro.
    for x in [2, w - 3] {
        rect(&mut c, x, 0, 1, 10, band);
    }
    // Serratura.
    let cx = w / 2;
    rect(&mut c, cx - 1, 4, 3, 4, brass);
    px(&mut c, cx, 5, shade(brass, 0.5));
    c
}

/// Paravento di legno e tela che chiude la cabina verso il dormitorio.
pub fn screen() -> Canvas {
    let mut c = Canvas::new(SCREEN_W as u32, SCREEN_H as u32);
    let wood = rgb(110, 76, 48);
    let cloth = rgb(188, 170, 128);
    rect(&mut c, 0, 0, 1, SCREEN_H, wood);
    rect(&mut c, SCREEN_W - 1, 0, 1, SCREEN_H, wood);
    rect(&mut c, 1, 2, SCREEN_W - 2, SCREEN_H - 4, cloth);
    rect(&mut c, 0, SCREEN_H - 2, SCREEN_W, 2, shade(wood, 1.2));
    rect(&mut c, 0, 0, SCREEN_W, 2, shade(wood, 0.8));
    for y in (6..SCREEN_H - 4).step_by(8) {
        rect(&mut c, 1, y, SCREEN_W - 2, 1, shade(cloth, 0.85));
    }
    c
}

/// Tappeto a righe sul pavimento della cabina, largo `w`.
pub fn rug(w: i32) -> Canvas {
    let mut c = Canvas::new(w as u32, RUG_H as u32);
    let red = rgb(150, 58, 52);
    rect(&mut c, 0, 0, w, RUG_H, red);
    for x in (1..w - 1).step_by(3) {
        px(&mut c, x, 1, rgb(214, 170, 48));
    }
    px(&mut c, 0, 1, shade(red, 0.7));
    px(&mut c, w - 1, 1, shade(red, 0.7));
    c
}

// --- Mensa --------------------------------------------------------------------

/// Tavolo a piedistallo con scodelle e bicchieri.
pub fn table(w: i32) -> Canvas {
    let mut c = Canvas::new(w as u32, TABLE_H as u32);
    let wood = rgb(176, 128, 78);
    let cx = w / 2;
    rect(&mut c, cx - 4, 0, 9, 1, shade(wood, 0.6));
    rect(&mut c, cx - 1, 1, 3, 8, shade(wood, 0.7));
    px(&mut c, cx - 1, 4, shade(wood, 0.9));
    rect(&mut c, 0, 9, w, 3, wood);
    rect(&mut c, 0, 9, w, 1, shade(wood, 0.6));
    rect(&mut c, 0, 11, w, 1, rgb(226, 194, 136));
    // Coperti: scodelle e bicchieri alternati.
    let places = (w / 6).max(1);
    for i in 0..places {
        let x = (w * (2 * i + 1)) / (2 * places);
        if i % 2 == 0 {
            rect(&mut c, x - 1, 12, 3, 1, rgb(214, 216, 224));
            px(&mut c, x - 2, 13, rgb(214, 216, 224));
            px(&mut c, x + 2, 13, rgb(214, 216, 224));
            px(&mut c, x, 13, rgb(170, 110, 60));
        } else {
            rect(&mut c, x, 12, 1, 2, rgb(160, 200, 220));
        }
    }
    c
}

/// Panca bassa dietro al tavolo.
pub fn bench(w: i32) -> Canvas {
    let mut c = Canvas::new(w as u32, BENCH_H as u32);
    let wood = rgb(130, 90, 54);
    rect(&mut c, 0, 3, w, 2, wood);
    rect(&mut c, 0, 4, w, 1, shade(wood, 1.3));
    for x in [1, w - 3] {
        rect(&mut c, x, 0, 2, 3, shade(wood, 0.7));
    }
    c
}

/// Cucina: forno con lo sportello acceso, fornello e pentola.
pub fn stove(w: i32) -> Canvas {
    let mut c = Canvas::new(w as u32, STOVE_H as u32);
    let steel = rgb(74, 76, 84);
    rect(&mut c, 0, 0, w, 14, steel);
    rect(&mut c, 0, 0, 1, 14, shade(steel, 1.3));
    rect(&mut c, w - 1, 0, 1, 14, shade(steel, 0.7));
    // Sportello del forno con la finestrella.
    if w >= 6 {
        rect(&mut c, 2, 2, w - 4, 8, shade(steel, 0.7));
        rect(&mut c, 3, 4, w - 6, 4, rgb(226, 110, 40));
        rect(&mut c, 3, 6, w - 6, 1, rgb(255, 186, 90));
        rect(&mut c, 2, 10, w - 4, 1, rgb(170, 172, 180));
    }
    for x in (2..w - 1).step_by(3) {
        px(&mut c, x, 12, rgb(200, 196, 180));
    }
    // Piano con la fiamma.
    rect(&mut c, 0, 14, w, 2, rgb(40, 40, 44));
    rect(&mut c, w / 2 - 3, 15, 7, 1, rgb(255, 110, 30));
    px(&mut c, w / 2, 15, rgb(255, 210, 90));
    // Pentola.
    let pw = (w - 4).clamp(3, 9);
    let px0 = (w - pw) / 2;
    let pot = rgb(150, 154, 166);
    rect(&mut c, px0, 16, pw, 5, pot);
    rect(&mut c, px0, 16, 1, 5, shade(pot, 1.25));
    rect(&mut c, px0 + pw - 1, 16, 1, 5, shade(pot, 0.7));
    rect(&mut c, px0 - 1, 19, 1, 1, pot);
    rect(&mut c, px0 + pw, 19, 1, 1, pot);
    rect(&mut c, px0, 21, pw, 1, shade(pot, 0.75));
    px(&mut c, px0 + pw / 2, 22, shade(pot, 0.6));
    c
}

/// Fotogramma del vapore che sale dalla pentola.
pub fn steam(frame: u8) -> Canvas {
    let mut c = Canvas::new(STEAM_W as u32, STEAM_H as u32);
    let rise = i32::from(frame) * 4;
    for (i, (x, y, r)) in [(4, 1, 1), (3, 5, 2), (5, 9, 1)].into_iter().enumerate() {
        let y = (y + rise) % STEAM_H;
        let sway = if (i as u8 + frame).is_multiple_of(2) {
            0
        } else {
            1
        };
        let alpha = 170 - (y * 10) as u8;
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy <= r * r + 1 {
                    px(&mut c, x + dx + sway, y + dy, rgba(240, 244, 250, alpha));
                }
            }
        }
    }
    c
}

// --- Serra --------------------------------------------------------------------

/// Aiuola: cassone di legno con la terra e piante di tipo diverso secondo `variant`.
pub fn grow_bed(w: i32, variant: u8) -> Canvas {
    let mut c = Canvas::new(w as u32, GROW_H as u32);
    let wood = rgb(124, 86, 50);
    rect(&mut c, 0, 0, w, 6, wood);
    rect(&mut c, 0, 3, w, 1, shade(wood, 0.7));
    rect(&mut c, 0, 5, w, 1, shade(wood, 1.25));
    for x in [0, w - 1] {
        rect(&mut c, x, 0, 1, 6, shade(wood, 0.6));
    }
    rect(&mut c, 1, 6, w - 2, 1, rgb(62, 42, 28));
    let seed = u32::from(variant);
    let greens = [rgb(64, 150, 58), rgb(98, 186, 74), rgb(42, 112, 50)];
    let mut x = 2;
    let mut i = 0;
    while x < w - 2 {
        let kind = (hash2(i, 0, seed) + seed) % 3;
        match kind {
            0 => {
                // Cespo di lattuga.
                for (dx, dy) in [
                    (0, 0),
                    (1, 0),
                    (2, 0),
                    (-1, 0),
                    (0, 1),
                    (1, 1),
                    (-1, 1),
                    (0, 2),
                ] {
                    let g = greens[((dx + dy + 3) % 3) as usize];
                    px(&mut c, x + dx, 7 + dy, g);
                }
                px(&mut c, x + 1, 9, greens[1]);
                x += 5;
            }
            1 => {
                // Pomodoro: stelo alto con bacche rosse.
                let h = 8 + (hash2(i, 1, seed) % 4) as i32;
                rect(&mut c, x, 7, 1, h, rgb(60, 120, 52));
                for y in (9..7 + h).step_by(3) {
                    px(&mut c, x - 1, y, greens[1]);
                    px(&mut c, x + 1, y + 1, greens[0]);
                }
                px(&mut c, x + 1, 10, rgb(220, 60, 40));
                px(&mut c, x - 1, 12, rgb(230, 80, 40));
                x += 4;
            }
            _ => {
                // Carote: ciuffi sottili.
                for dx in 0..2 {
                    let h = 3 + (hash2(i, dx, seed) % 3) as i32;
                    rect(&mut c, x + dx * 2, 7, 1, h, greens[(dx % 3) as usize]);
                    px(&mut c, x + dx * 2 - 1, 7 + h - 1, greens[1]);
                    px(&mut c, x + dx * 2, 6, rgb(230, 130, 40));
                }
                x += 5;
            }
        }
        i += 1;
    }
    c
}

// --- Officina -----------------------------------------------------------------

/// Banco da lavoro con morsa, attrezzi e ripiano.
pub fn workbench(w: i32) -> Canvas {
    let mut c = Canvas::new(w as u32, WORKBENCH_H as u32);
    let metal = rgb(58, 60, 68);
    let wood = rgb(150, 100, 56);
    for x in [1, w - 3] {
        rect(&mut c, x, 0, 2, 11, metal);
        px(&mut c, x, 5, shade(metal, 1.4));
    }
    rect(&mut c, 1, 3, w - 2, 1, metal);
    // Cassetta sul ripiano.
    if w >= 14 {
        rect(&mut c, w / 2 - 3, 4, 7, 4, rgb(170, 50, 40));
        rect(&mut c, w / 2 - 3, 7, 7, 1, rgb(200, 80, 60));
        rect(&mut c, w / 2 - 1, 8, 3, 1, metal);
    }
    rect(&mut c, 0, 11, w, 3, wood);
    rect(&mut c, 0, 11, w, 1, rgb(96, 98, 108));
    rect(&mut c, 0, 13, w, 1, shade(wood, 1.25));
    // Morsa a sinistra.
    let vise = rgb(60, 100, 140);
    rect(&mut c, 2, 14, 6, 2, vise);
    rect(&mut c, 2, 16, 2, 2, vise);
    rect(&mut c, 6, 16, 2, 2, vise);
    rect(&mut c, 0, 15, 2, 1, rgb(170, 170, 180));
    px(&mut c, 4, 17, rgb(150, 150, 160));
    // Martello e bulloni sul piano.
    if w >= 16 {
        rect(&mut c, w - 11, 14, 6, 1, rgb(120, 76, 40));
        rect(&mut c, w - 5, 14, 2, 2, rgb(170, 172, 182));
        for x in [w - 14, w - 12] {
            px(&mut c, x, 14, rgb(190, 190, 200));
        }
    }
    c
}

/// Fotogramma delle scintille che schizzano dalla morsa.
pub fn sparks(frame: u8) -> Canvas {
    let mut c = Canvas::new(SPARKS_W as u32, SPARKS_H as u32);
    let hot = [rgb(255, 250, 200), rgb(255, 210, 90), rgb(255, 140, 40)];
    for i in 0..7 {
        let h = hash2(i, i32::from(frame), 77);
        let x = (h % SPARKS_W as u32) as i32;
        let y = ((h >> 8) % SPARKS_H as u32) as i32;
        px(&mut c, x, y, hot[(h >> 16) as usize % 3]);
    }
    px(&mut c, SPARKS_W / 2, 0, hot[0]);
    c
}

// --- Mercato ------------------------------------------------------------------

/// Colori dei banconi (tendina e fronte).
const STALL_HUES: [Rgba; 4] = [
    rgb(200, 60, 50),
    rgb(40, 130, 140),
    rgb(220, 150, 40),
    rgb(110, 70, 150),
];
pub const STALL_VARIANTS: u8 = STALL_HUES.len() as u8;

/// Bancone del mercato con il fronte colorato e la tendina a strisce.
pub fn counter(w: i32, hue: u8) -> Canvas {
    let mut c = Canvas::new(w as u32, COUNTER_H as u32);
    let color = STALL_HUES[hue as usize % STALL_HUES.len()];
    let wood = rgb(120, 80, 46);
    // Fronte a doghe colorate.
    rect(&mut c, 0, 0, w, 11, wood);
    for x in 1..w - 1 {
        let stripe = if (x / 3) % 2 == 0 {
            color
        } else {
            shade(color, 0.8)
        };
        rect(&mut c, x, 1, 1, 9, stripe);
    }
    rect(&mut c, 0, 0, w, 1, shade(wood, 0.6));
    // Piano.
    rect(&mut c, 0, 11, w, 3, rgb(206, 170, 110));
    rect(&mut c, 0, 11, w, 1, shade(wood, 0.8));
    rect(&mut c, 0, 13, w, 1, rgb(232, 204, 150));
    // Pali e tendina.
    for x in [1, w - 2] {
        rect(&mut c, x, 14, 1, 18, rgb(84, 60, 36));
    }
    for y in 31..38 {
        let inset = (y - 31) / 3;
        for x in inset..w - inset {
            let stripe = if (x / 3) % 2 == 0 {
                color
            } else {
                rgb(238, 232, 214)
            };
            px(
                &mut c,
                x,
                y,
                if y == 37 { shade(stripe, 1.1) } else { stripe },
            );
        }
    }
    // Bordo a festoni.
    for x in 0..w {
        if x % 3 != 2 {
            let stripe = if (x / 3) % 2 == 0 {
                color
            } else {
                rgb(238, 232, 214)
            };
            px(&mut c, x, 30, shade(stripe, 0.85));
        }
    }
    rect(&mut c, 0, 38, w, 1, shade(color, 0.6));
    // Cartellino appeso.
    if w >= 12 {
        let x = w / 2;
        px(&mut c, x, 29, rgb(60, 40, 20));
        rect(&mut c, x - 2, 25, 5, 4, rgb(236, 226, 190));
        rect(&mut c, x - 1, 26, 3, 1, mix(color, rgb(0, 0, 0), 0.3));
    }
    c
}

// --- Magazzino ------------------------------------------------------------------

/// Casse: misura, passo della griglia e bordo dello scaffale.
pub const CRATE_W: i32 = 7;
pub const CRATE_H: i32 = 7;
pub const CRATE_STEP_X: i32 = 9;
pub const CRATE_STEP_Y: i32 = 8;
pub const SHELF_PAD_X: i32 = 3;
pub const SHELF_PAD_Y: i32 = 3;

/// Scaffale largo `w` e alto `h` con `rows` ripiani (sotto ogni fila di casse).
pub fn shelf(w: i32, h: i32, rows: i32) -> Canvas {
    let mut c = Canvas::new(w as u32, h as u32);
    let back = rgb(52, 38, 28);
    let wood = rgb(96, 66, 42);
    rect(&mut c, 0, 0, w, h, back);
    for x in (4..w).step_by(6) {
        rect(&mut c, x, 0, 1, h, shade(back, 0.8));
    }
    for r in 0..=rows {
        let y = SHELF_PAD_Y - 1 + r * CRATE_STEP_Y;
        rect(&mut c, 0, y, w, 1, wood);
    }
    rect(&mut c, 0, h - 2, w, 2, wood);
    rect(&mut c, 0, h - 1, w, 1, shade(wood, 1.3));
    rect(&mut c, 0, 0, w, 2, shade(wood, 0.8));
    for x in [0, w - 2] {
        rect(&mut c, x, 0, 2, h, wood);
        rect(&mut c, x, 0, 1, h, shade(wood, 1.25));
    }
    c
}

/// Modello di una cassa (7×7) per tipo di oggetto.
pub fn crate_art(item: ItemKind) -> Canvas {
    match item {
        ItemKind::Verdura => Canvas::from_rows(
            &[
                ".gGlGg.", //
                "gGlgGlg", //
                "WWWWWWW", //
                "wdwdwdw", //
                "WWWWWWW", //
                "wdwdwdw", //
                "DDDDDDD", //
            ],
            &[
                ('g', rgb(70, 150, 60)),
                ('G', rgb(44, 110, 48)),
                ('l', rgb(120, 196, 80)),
                ('W', rgb(170, 124, 76)),
                ('w', rgb(146, 104, 62)),
                ('d', rgb(80, 56, 34)),
                ('D', rgb(106, 74, 44)),
            ],
        ),
        ItemKind::Razione => Canvas::from_rows(
            &[
                "yyyyyyy", //
                "YYYtYYY", //
                "YYYtYYY", //
                "rrrrrrr", //
                "rwwwwwr", //
                "YYYYYYY", //
                "ddddddd", //
            ],
            &[
                ('y', rgb(232, 190, 120)),
                ('Y', rgb(206, 160, 96)),
                ('t', rgb(150, 110, 60)),
                ('r', rgb(180, 50, 40)),
                ('w', rgb(240, 230, 210)),
                ('d', rgb(150, 110, 64)),
            ],
        ),
        ItemKind::Rottame => Canvas::from_rows(
            &[
                "...s...", //
                "..oOs..", //
                ".sOoRo.", //
                "oRsOoso", //
                "OoRosRo", //
                "RoOsoOR", //
                "ooRoRoo", //
            ],
            &[
                ('o', rgb(150, 90, 56)),
                ('O', rgb(104, 60, 40)),
                ('R', rgb(176, 110, 70)),
                ('s', rgb(150, 156, 166)),
            ],
        ),
        ItemKind::Attrezzo => Canvas::from_rows(
            &[
                "bbbbbbb", //
                "bmbbbmb", //
                "bmbmbhb", //
                "bhbmbhb", //
                "bhbhbhb", //
                "bhbhbbb", //
                "BBBBBBB", //
            ],
            &[
                ('b', rgb(120, 92, 60)),
                ('B', rgb(84, 62, 40)),
                ('m', rgb(186, 192, 204)),
                ('h', rgb(180, 60, 44)),
            ],
        ),
        ItemKind::Vestito => Canvas::from_rows(
            &[
                ".......", //
                ".ccccc.", //
                ".CCCCC.", //
                "eeeeeee", //
                "EEEEEEE", //
                ".ccccc.", //
                ".CCCCC.", //
            ],
            &[
                ('c', rgb(210, 90, 100)),
                ('C', rgb(160, 60, 72)),
                ('e', rgb(90, 120, 180)),
                ('E', rgb(64, 88, 140)),
            ],
        ),
        // Balla di cotone legata con lo spago.
        ItemKind::Cotone => Canvas::from_rows(
            &[
                ".wWwWw.", //
                "wWwwwWw", //
                "ttttttt", //
                "wWwWwWw", //
                "WwwWwwW", //
                "ttttttt", //
                ".wWwWw.", //
            ],
            &[
                ('w', rgb(238, 234, 222)),
                ('W', rgb(206, 200, 186)),
                ('t', rgb(150, 110, 64)),
            ],
        ),
        // Mazzi di erbe in un cesto.
        ItemKind::Erbe => Canvas::from_rows(
            &[
                "g.G.g.G", //
                "GgLgGlg", //
                ".gGlGg.", //
                "bbbbbbb", //
                "bBbBbBb", //
                "BbBbBbB", //
                "DDDDDDD", //
            ],
            &[
                ('g', rgb(64, 140, 110)),
                ('G', rgb(40, 100, 80)),
                ('L', rgb(150, 196, 120)),
                ('l', rgb(120, 170, 100)),
                ('b', rgb(186, 146, 84)),
                ('B', rgb(150, 112, 60)),
                ('D', rgb(104, 74, 42)),
            ],
        ),
        // Lingotti di metallo impilati.
        ItemKind::Metallo => Canvas::from_rows(
            &[
                "..mMm..", //
                "..sSs..", //
                ".mMmMm.", //
                ".sSsSs.", //
                "mMmMmMm", //
                "sSsSsSs", //
                "ddddddd", //
            ],
            &[
                ('m', rgb(196, 202, 212)),
                ('M', rgb(232, 236, 242)),
                ('s', rgb(132, 138, 150)),
                ('S', rgb(110, 116, 128)),
                ('d', rgb(70, 74, 82)),
            ],
        ),
        // Pezze di stoffa piegate, di tre colori.
        ItemKind::Tessuto => Canvas::from_rows(
            &[
                ".......", //
                "bbbbbbb", //
                "BBBBBBB", //
                "yyyyyyy", //
                "YYYYYYY", //
                "ppppppp", //
                "PPPPPPP", //
            ],
            &[
                ('b', rgb(96, 136, 196)),
                ('B', rgb(64, 98, 152)),
                ('y', rgb(224, 184, 92)),
                ('Y', rgb(182, 142, 62)),
                ('p', rgb(154, 104, 164)),
                ('P', rgb(112, 72, 122)),
            ],
        ),
        // Teiera di terracotta con la tazza.
        ItemKind::Te => Canvas::from_rows(
            &[
                "...K...", //
                "..LLL..", //
                "hTTTTT.", //
                "hTwTTTs", //
                "hTTTTs.", //
                ".TTTT..", //
                "..ccc..", //
            ],
            &[
                ('K', rgb(230, 200, 120)),
                ('L', rgb(168, 70, 50)),
                ('T', rgb(204, 94, 62)),
                ('w', rgb(242, 204, 184)),
                ('h', rgb(150, 60, 40)),
                ('s', rgb(190, 84, 56)),
                ('c', rgb(236, 232, 220)),
            ],
        ),
        // Due coperte piegate, a quadri.
        ItemKind::Coperta => Canvas::from_rows(
            &[
                ".......", //
                "ggggggg", //
                "gwgwgwg", //
                "GGGGGGG", //
                "rrrrrrr", //
                "rwrwrwr", //
                "RRRRRRR", //
            ],
            &[
                ('g', rgb(150, 76, 132)),
                ('G', rgb(110, 52, 96)),
                ('r', rgb(92, 112, 70)),
                ('R', rgb(64, 80, 48)),
                ('w', rgb(226, 214, 190)),
            ],
        ),
        // Lanterna con la fiamma accesa.
        ItemKind::Lampada => Canvas::from_rows(
            &[
                "...h...", //
                "..MMM..", //
                ".MyYyM.", //
                ".MYFYM.", //
                ".MyYyM.", //
                "..MMM..", //
                ".MMMMM.", //
            ],
            &[
                ('h', rgb(126, 126, 134)),
                ('M', rgb(86, 86, 94)),
                ('y', rgb(246, 214, 116)),
                ('Y', rgb(255, 238, 170)),
                ('F', rgb(255, 150, 56)),
            ],
        ),
        // Orsacchiotto di pezza.
        ItemKind::Giocattolo => Canvas::from_rows(
            &[
                ".b...b.", //
                ".bbbbb.", //
                "bbebebb", //
                ".bbnbb.", //
                "..bbb..", //
                ".bBbBb.", //
                ".b...b.", //
            ],
            &[
                ('b', rgb(176, 114, 62)),
                ('B', rgb(226, 150, 170)),
                ('e', rgb(40, 30, 30)),
                ('n', rgb(70, 44, 32)),
            ],
        ),
    }
}

/// Merce esposta sui banconi del Mercato (solo attrezzi e vestiti).
pub fn good_art(item: ItemKind) -> Canvas {
    match item {
        ItemKind::Attrezzo => Canvas::from_rows(
            &[
                "mm....", //
                "mhhhhh", //
                "mm....", //
            ],
            &[('m', rgb(186, 192, 204)), ('h', rgb(150, 90, 50))],
        ),
        _ => Canvas::from_rows(
            &[
                "cCcCc", //
                "ccccc", //
                "CCCCC", //
            ],
            &[('c', rgb(210, 90, 100)), ('C', rgb(160, 60, 72))],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env_art::get;

    #[test]
    fn crate_templates_share_one_size() {
        for item in ItemKind::ALL {
            let c = crate_art(item);
            assert_eq!(
                (c.width as i32, c.height as i32),
                (CRATE_W, CRATE_H),
                "{item:?}"
            );
        }
        assert_eq!(good_art(ItemKind::Attrezzo).height, 3);
        assert_eq!(good_art(ItemKind::Vestito).width, 5);
    }

    #[test]
    fn stations_are_drawn_at_the_requested_width() {
        for w in [4, 9, 14, 20, 28, 36] {
            assert_eq!(bed(w, false, 0).width as i32, w);
            assert_eq!(bed(w, true, 1).height as i32, BED_H + LEVEL_HEIGHT as i32);
            assert_eq!(table(w).height as i32, TABLE_H);
            assert_eq!(bench(w).width as i32, w);
            assert_eq!(stove(w).height as i32, STOVE_H);
            assert_eq!(grow_bed(w, 2).width as i32, w);
            assert_eq!(workbench(w).width as i32, w);
            assert_eq!(counter(w, 3).height as i32, COUNTER_H);
        }
    }

    #[test]
    fn beds_are_solid_up_to_the_mattress() {
        let c = bed(20, false, 0);
        // Il materasso arriva a BED_TOP, sopra c'è solo il cuscino a sinistra.
        assert_eq!(get(&c, 10, BED_TOP as i32 - 1)[3], 255);
        assert_eq!(get(&c, 10, BED_TOP as i32 + 2)[3], 0);
    }

    #[test]
    fn animations_have_distinct_frames() {
        assert_ne!(steam(0), steam(1));
        assert_ne!(sparks(0), sparks(1));
        assert_eq!(steam(2).width as i32, STEAM_W);
        assert_eq!(sparks(2).height as i32, SPARKS_H);
    }

    #[test]
    fn shelf_holds_the_crate_grid() {
        let (cols, rows) = (4, 5);
        let w = SHELF_PAD_X * 2 + cols * CRATE_STEP_X - (CRATE_STEP_X - CRATE_W);
        assert_eq!(w, 40);
        let h = SHELF_PAD_Y + rows * CRATE_STEP_Y + 1;
        assert!(h <= 44);
        assert_eq!(shelf(40, 44, rows).height, 44);
        assert!(bed_plate(123).width > bed_plate(7).width);
    }
}
