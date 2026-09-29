//! Icone procedurali 16×16 degli oggetti inventati dal Narratore, dal loro
//! `aspetto` (forma, colore, dettaglio: liste chiuse in
//! `narrator::appearance`). L'AI non disegna: sceglie da un menù.
//!
//! Ogni forma è un modello a righe di testo (come il resto della pixel art,
//! vedi `art.rs`) con quattro toni ricavati dal colore: contorno `k`, base
//! `b`, luce `h`, ombra `s`; alcuni pezzi hanno colori fissi (sughero, metallo,
//! carta, fiamma). Il dettaglio si disegna sopra, solo sui pixel della base.
//! Le icone diventano texture egui una volta sola ([`ItemIcons`]).
//!
//! Per le voci della cronaca che non sono oggetti ci sono icone per tipo
//! ([`kind_canvas`]): ricetta, lavoro, evento, statistica.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy_egui::egui;
use narrator::{Appearance, Colour, Detail, Shape};

use crate::art::{Canvas, Rgba};
use crate::env_art::{hash2, mix, rgb, rgba, shade};

const CORK: Rgba = rgb(128, 86, 50);
const CORK_DARK: Rgba = rgb(86, 56, 32);
const METAL: Rgba = rgb(176, 182, 192);
const METAL_DARK: Rgba = rgb(112, 118, 130);
const PAPER: Rgba = rgb(234, 222, 192);
const FLAME: Rgba = rgb(255, 216, 96);
const FLAME_HOT: Rgba = rgb(242, 138, 44);
const GLOW: Rgba = rgb(255, 238, 176);
const SHINE: Rgba = rgba(255, 255, 255, 230);

/// Il modello di una forma: 16 righe da 16 caratteri.
fn template(shape: Shape) -> [&'static str; 16] {
    match shape {
        Shape::Bar => [
            "................",
            "................",
            "................",
            "................",
            "...kkkkkkkkkk...",
            "..khhhhhhhhhhk..",
            ".khbbbbbbbbbbsk.",
            ".khbbbbbbbbbbsk.",
            ".kbbbbbbbbbbbsk.",
            ".kbbbbbbbbbbbsk.",
            ".kbbbbbbbbbbssk.",
            "..kssssssssssk..",
            "...kkkkkkkkkk...",
            "................",
            "................",
            "................",
        ],
        Shape::Ingot => [
            "................",
            "................",
            "................",
            "................",
            "....kkkkkkkk....",
            "...khhhhhhhhk...",
            "..khhhhhhhhhhk..",
            ".kkkkkkkkkkkkkk.",
            ".kbbbbbbbbbbbbk.",
            ".kbbbbbbbbbbbsk.",
            ".kbbbbbbbbbbssk.",
            ".kssssssssssssk.",
            ".kkkkkkkkkkkkkk.",
            "................",
            "................",
            "................",
        ],
        Shape::Sack => [
            "................",
            "......kkkk......",
            ".....kcCCck.....",
            "......kCCk......",
            ".....kbbbbk.....",
            "....kbhbbbbk....",
            "...kbhbbbbbbk...",
            "..kbhbbbbbbbsk..",
            "..kbhbbbbbbbsk..",
            ".kbhbbbbbbbbbsk.",
            ".kbbbbbbbbbbbsk.",
            ".kbbbbbbbbbbbsk.",
            ".kbbbbbbbbbbssk.",
            "..ksssssssssk...",
            "...kkkkkkkkkk...",
            "................",
        ],
        Shape::Bottle => [
            "......kkkk......",
            "......kcck......",
            "......kkkk......",
            "......kbhk......",
            "......kbhk......",
            ".....kbbhbk.....",
            "....kbbbbhbk....",
            "...kbbbbbbhbk...",
            "...kbbbbbbhbk...",
            "...kbbbbbbbbk...",
            "...kbwbbbbbbk...",
            "...kbwbbbbbsk...",
            "...kbbbbbbbsk...",
            "...kssssssssk...",
            "....kkkkkkkk....",
            "................",
        ],
        Shape::Jar => [
            "................",
            "................",
            "....kkkkkkkk....",
            "....kmmmmmmk....",
            "....kMMMMMMk....",
            "...kkkkkkkkkk...",
            "..kbhbbbbbbbbk..",
            "..kbhbbbbbbbsk..",
            "..kbhbbbbbbbsk..",
            "..kbbbbbbbbbsk..",
            "..kbbbbbbbbbsk..",
            "..kbbbbbbbbbsk..",
            "..kbbbbbbbbssk..",
            "..kssssssssssk..",
            "...kkkkkkkkkk...",
            "................",
        ],
        Shape::Tool => [
            "................",
            "..kkkkkkkkkkk...",
            ".kmmmmmmmmmmmk..",
            ".kmwmmmmmmmmMk..",
            ".kMMMMMMMMMMMk..",
            "..kkkkkhbkkkk...",
            "......khbk......",
            "......khbk......",
            "......khbk......",
            "......khbk......",
            "......khbk......",
            "......khsk......",
            "......khsk......",
            "......kbsk......",
            "......kssk......",
            "......kkkk......",
        ],
        Shape::Cloth => [
            "................",
            "................",
            "................",
            "..kkkkkkkkkkkk..",
            ".khhhhhhhhhhhhk.",
            ".kbbbbbbbbbbbbk.",
            ".kssssssssssssk.",
            ".kkkkkkkkkkkkkk.",
            ".khhhhhhhhhhhhk.",
            ".kbbbbbbbbbbbbk.",
            ".kssssssssssssk.",
            ".kkkkkkkkkkkkkk.",
            ".khhhhhhhhhhhhk.",
            ".kbbbbbbbbbbbbk.",
            "..kkkkkkkkkkkk..",
            "................",
        ],
        Shape::Roll => [
            "................",
            "................",
            "................",
            "................",
            "...kkkkkkkkkkk..",
            "..khhhhhhhhhkbk.",
            "..kbbbbbbbbkbsbk",
            "..kbbbbbbbbkbkbk",
            "..kbbbbbbbbkbsbk",
            "..kssssssssksbk.",
            "...kkkkkkkkkkk..",
            "................",
            "................",
            "................",
            "................",
            "................",
        ],
        Shape::Loaf => [
            "................",
            "................",
            "................",
            "................",
            "................",
            "....kkkkkkkk....",
            "...kkhhhhhhkk...",
            "..khhbbbbbbhhk..",
            ".khbbsbbsbbsbbk.",
            ".kbbbbbbbbbbbbk.",
            ".kbbbbbbbbbbbsk.",
            ".kssssssssssssk.",
            "..kkkkkkkkkkkk..",
            "................",
            "................",
            "................",
        ],
        Shape::Leaf => [
            "................",
            "...........kkk..",
            ".........kkbhhk.",
            ".......kkbbbhhk.",
            "......kbbbbbhbk.",
            ".....kbbbbbhbbk.",
            "....kbbbbbhbbbk.",
            "...kbbbbbhbbbk..",
            "...kbbbbhbbbbk..",
            "..kbbbbhbbbbk...",
            "..kbbbhbbbbsk...",
            "..kbbhbbbssk....",
            "..kbhsssskk.....",
            ".kCkkkkk........",
            "kC..............",
            "................",
        ],
        Shape::Lamp => [
            "......kkkk......",
            ".....k....k.....",
            "....kkkkkkkk....",
            "....kbbbbbbk....",
            "...kkkkkkkkkk...",
            "...kbggggggbk...",
            "...kbggfFggbk...",
            "...kbgfFFfgbk...",
            "...kbgfFFfgbk...",
            "...kbggffggbk...",
            "...kbggggggbk...",
            "...kkkkkkkkkk...",
            "....kbbbbbbk....",
            "....kssssssk....",
            "....kkkkkkkk....",
            "................",
        ],
        Shape::Toy => [
            "................",
            "...kkk....kkk...",
            "..kbbbk..kbbbk..",
            "..kbhbkkkkbhbk..",
            "...kbbbbbbbbk...",
            "...kbkbbbbkbk...",
            "...kbbbppbbbk...",
            "...kbbbkkbbbk...",
            "....kbbbbbbk....",
            "..kkkbbbbbbkkk..",
            ".kbbkbhhhhbkbbk.",
            ".kbbkbhhhhbkbbk.",
            "..kkkbbbbbbkkk..",
            "...kbbbkkbbbk...",
            "...kssk..kssk...",
            "....kk....kk....",
        ],
        Shape::Box => [
            "................",
            "................",
            "................",
            "..kkkkkkkkkkkk..",
            "..khhhhhhhhhhk..",
            ".kkkkkkkkkkkkkk.",
            ".kbbbbbbbbbbbbk.",
            ".kbsbbbbbbbbsbk.",
            ".kbbbbbbbbbbbbk.",
            ".kssssssssssssk.",
            ".kbbbbbbbbbbbbk.",
            ".kbsbbbbbbbbsbk.",
            ".kbbbbbbbbbbbbk.",
            ".kssssssssssssk.",
            ".kkkkkkkkkkkkkk.",
            "................",
        ],
        Shape::Coin => [
            "................",
            "................",
            ".....kkkkkk.....",
            "...kkhhhhhhkk...",
            "..khhbbbbbbbsk..",
            ".khbbbkkkkbbbsk.",
            ".khbbkbbbbkbbsk.",
            ".kbbbkbbbbkbbsk.",
            ".kbbbkbbbbkbbsk.",
            ".kbbbkbbbbkbbsk.",
            ".kbbbbkkkkbbbsk.",
            "..kbbbbbbbbbsk..",
            "...kkssssssskk..",
            ".....kkkkkk.....",
            "................",
            "................",
        ],
        Shape::Gem => [
            "................",
            "................",
            "................",
            "....kkkkkkkk....",
            "...khwhhhhbhk...",
            "..khhhbhhhbbbk..",
            ".kkkkkkkkkkkkkk.",
            "..kbhbbbbbbssk..",
            "...kbhbbbbssk...",
            "....kbhbbssk....",
            ".....kbhssk.....",
            "......kbsk......",
            ".......kk.......",
            "................",
            "................",
            "................",
        ],
        Shape::Book => [
            "................",
            "................",
            "..kkkkkkkkkkkk..",
            "..kChhhhhhhhhk..",
            "..kCbbbbbbbbbk..",
            "..kCbkkkkkkkbk..",
            "..kCbkpppppkbk..",
            "..kCbkkkkkkkbk..",
            "..kCbbbbbbbbbk..",
            "..kCbbbbbbbbsk..",
            "..kCbbbbbbbbsk..",
            "..kCbbbbbbbbsk..",
            "..kCpppppppppk..",
            "..kkkkkkkkkkkk..",
            "................",
            "................",
        ],
    }
}

/// I quattro toni di un colore della palette.
struct Tones {
    ink: Rgba,
    base: Rgba,
    light: Rgba,
    dark: Rgba,
}

fn tones(colour: Colour) -> Tones {
    let [r, g, b] = colour.rgb();
    let base = rgb(r, g, b);
    let ink = if colour == Colour::Black {
        rgb(18, 16, 22)
    } else {
        mix(shade(base, 0.35), rgb(24, 20, 28), 0.4)
    };
    Tones {
        ink,
        base,
        light: mix(base, rgb(255, 255, 255), 0.35),
        dark: shade(base, 0.72),
    }
}

fn palette(t: &Tones) -> [(char, Rgba); 16] {
    [
        ('k', t.ink),
        ('b', t.base),
        ('h', t.light),
        ('s', t.dark),
        ('c', CORK),
        ('C', CORK_DARK),
        ('m', METAL),
        ('M', METAL_DARK),
        ('p', PAPER),
        ('f', FLAME),
        ('F', FLAME_HOT),
        ('g', GLOW),
        ('w', SHINE),
        // Riservati.
        ('x', t.ink),
        ('y', t.base),
        ('z', t.light),
    ]
}

/// L'icona di una forma nel colore dato, senza dettaglio.
pub fn shape_canvas(shape: Shape, colour: Colour) -> Canvas {
    Canvas::from_rows(&template(shape), &palette(&tones(colour)))
}

/// Pixel della base (b, h, s) di una forma: dove va il dettaglio.
fn body_mask(shape: Shape) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    for (y, row) in template(shape).iter().enumerate() {
        for (x, ch) in row.chars().enumerate() {
            if matches!(ch, 'b' | 'h' | 's') {
                out.push((x as u32, y as u32));
            }
        }
    }
    out
}

/// Disegna `detail` sulla base della forma.
fn draw_detail(canvas: &mut Canvas, shape: Shape, colour: Colour, detail: Detail) {
    let body = body_mask(shape);
    if body.is_empty() {
        return;
    }
    let t = tones(colour);
    let (x0, x1) = (
        body.iter().map(|p| p.0).min().unwrap_or(0),
        body.iter().map(|p| p.0).max().unwrap_or(0),
    );
    let (y0, y1) = (
        body.iter().map(|p| p.1).min().unwrap_or(0),
        body.iter().map(|p| p.1).max().unwrap_or(0),
    );
    let inside = |x: u32, y: u32| body.contains(&(x, y));
    // Un colore che si stacca dalla base.
    let contrast = if matches!(colour, Colour::Yellow | Colour::Gold | Colour::White) {
        rgb(150, 60, 50)
    } else {
        rgb(232, 196, 92)
    };
    match detail {
        Detail::Stripes => {
            for &(x, y) in &body {
                if (y + x / 5) % 3 == 0 {
                    canvas.set(x, y, shade(t.base, 0.78));
                }
            }
        }
        Detail::Dots => {
            for &(x, y) in &body {
                if y % 3 == 1 && (x + y) % 3 == 0 {
                    canvas.set(x, y, mix(t.base, rgb(255, 255, 255), 0.6));
                }
            }
        }
        Detail::Label => {
            // Una fascia di carta nel mezzo, lontana dai bordi se c'è posto.
            let mid = (y0 + y1) / 2 + 1;
            let mut drawn = false;
            for y in mid.saturating_sub(1)..=mid + 1 {
                for x in x0 + 2..=x1.saturating_sub(2) {
                    if inside(x, y) && inside(x.saturating_sub(2), y) && inside(x + 2, y) {
                        let edge = y != mid;
                        canvas.set(x, y, if edge { shade(PAPER, 0.8) } else { PAPER });
                        drawn = true;
                    }
                }
            }
            if !drawn {
                for &(x, y) in body.iter().filter(|p| p.1 == mid) {
                    canvas.set(x, y, PAPER);
                }
            }
        }
        Detail::Shine => {
            // Una stellina in alto a destra e un riflesso.
            let (cx, cy) = (x1.saturating_sub(2), y0 + 2);
            for (dx, dy) in [(0i32, 0i32), (1, 0), (-1, 0), (0, 1), (0, -1)] {
                let (x, y) = (cx as i32 + dx, cy as i32 + dy);
                if x >= 0 && y >= 0 {
                    canvas.set(x as u32, y as u32, SHINE);
                }
            }
            for &(x, y) in &body {
                if x == x0 + 1 && y > y0 + 1 && y < y0 + 4 {
                    canvas.set(x, y, SHINE);
                }
            }
        }
        Detail::Crack => {
            // Una linea spezzata che scende lungo la base, riga per riga.
            let mut offset: i32 = 1;
            for y in y0..=y1 {
                let row: Vec<u32> = body.iter().filter(|p| p.1 == y).map(|p| p.0).collect();
                let (Some(&lo), Some(&hi)) = (row.iter().min(), row.iter().max()) else {
                    continue;
                };
                let width = (hi - lo) as i32;
                let x = lo as i32 + (width / 2 + offset).clamp(0, width);
                canvas.set(x as u32, y, t.ink);
                offset += if hash2(x, y as i32, 7).is_multiple_of(2) {
                    1
                } else {
                    -1
                };
                offset = offset.clamp(-3, 3);
            }
        }
        Detail::Patch => {
            // Sul pixel della base più vicino al centro (la base può essere
            // solo una cornice, come nella lampada).
            let (cx, cy) = ((x0 + x1) as f32 / 2.0, (y0 + y1) as f32 / 2.0);
            let &(bx, by) = body
                .iter()
                .min_by(|a, b| {
                    let d = |p: &&(u32, u32)| (p.0 as f32 - cx).powi(2) + (p.1 as f32 - cy).powi(2);
                    d(a).total_cmp(&d(b))
                })
                .unwrap_or(&(cx as u32, cy as u32));
            let (px, py) = (bx.saturating_sub(1), by.saturating_sub(1));
            for y in py..py + 4 {
                for x in px..px + 4 {
                    if inside(x, y) {
                        let stitch =
                            (x == px || x == px + 3 || y == py || y == py + 3) && (x + y) % 2 == 0;
                        canvas.set(x, y, if stitch { t.ink } else { contrast });
                    }
                }
            }
        }
    }
}

/// L'icona di un `aspetto`; valori sconosciuti diventano una scatola grigia.
pub fn icon_canvas(look: &Appearance) -> Canvas {
    let shape = look.shape().unwrap_or(Shape::Box);
    let colour = look.colour().unwrap_or(Colour::Grey);
    let mut canvas = shape_canvas(shape, colour);
    if let Ok(Some(detail)) = look.detail() {
        draw_detail(&mut canvas, shape, colour, detail);
    }
    canvas
}

/// Icone per le voci della cronaca che non sono oggetti: "ricetta",
/// "lavoro", "evento", "statistica" (altro: un punto di domanda grigio).
pub fn kind_canvas(kind: &str) -> Canvas {
    match kind {
        "ricetta" => {
            let mut c = shape_canvas(Shape::Book, Colour::Orange);
            draw_detail(&mut c, Shape::Book, Colour::Orange, Detail::Label);
            c
        }
        "lavoro" => shape_canvas(Shape::Tool, Colour::Brown),
        "evento" => Canvas::from_rows(
            &[
                "................",
                "....kkkkkkkk....",
                "...kbbbbbbbbk...",
                "..kbbbbwwbbbbk..",
                "..kbbbbwwbbbbk..",
                "..kbbbbwwbbbbk..",
                "..kbbbbwwbbbbk..",
                "..kbbbbbbbbbbk..",
                "..kbbbbwwbbbbk..",
                "...kbbbbbbbbk...",
                "....kkkbbkkk....",
                "......kbk.......",
                "......kk........",
                "................",
                "................",
                "................",
            ],
            &palette(&tones(Colour::Red)),
        ),
        "statistica" => Canvas::from_rows(
            &[
                "................",
                "................",
                "...........kkk..",
                "...........khk..",
                ".......kkk.kbk..",
                ".......khk.kbk..",
                ".......kbk.kbk..",
                "...kkk.kbk.kbk..",
                "...khk.kbk.kbk..",
                "...kbk.kbk.kbk..",
                "...kbk.kbk.kbk..",
                "...kbk.kbk.kbk..",
                "...ksk.ksk.ksk..",
                ".kkkkkkkkkkkkkk.",
                "................",
                "................",
            ],
            &palette(&tones(Colour::Sky)),
        ),
        _ => shape_canvas(Shape::Box, Colour::Grey),
    }
}

/// Texture egui delle icone, create una volta per chiave.
#[derive(Resource, Default)]
pub struct ItemIcons {
    textures: HashMap<String, egui::TextureHandle>,
}

impl ItemIcons {
    /// La texture per `key`, disegnata da `draw` la prima volta.
    pub fn get(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        draw: impl FnOnce() -> Canvas,
    ) -> egui::TextureId {
        self.textures
            .entry(key.to_string())
            .or_insert_with(|| {
                let canvas = draw();
                let bytes: Vec<u8> = canvas.pixels.iter().flatten().copied().collect();
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [canvas.width as usize, canvas.height as usize],
                    &bytes,
                );
                ctx.load_texture(format!("icona {key}"), image, egui::TextureOptions::NEAREST)
            })
            .id()
    }

    /// L'icona di un oggetto dal suo aspetto.
    pub fn item(&mut self, ctx: &egui::Context, look: &Appearance) -> egui::TextureId {
        self.get(ctx, &format!("oggetto/{}", look.key()), || {
            icon_canvas(look)
        })
    }

    /// L'icona di un tipo di novità.
    pub fn kind(&mut self, ctx: &egui::Context, kind: &str) -> egui::TextureId {
        self.get(ctx, &format!("tipo/{kind}"), || kind_canvas(kind))
    }
}

/// L'immagine di un oggetto in una lista: la sua icona 16×16 se ha un
/// aspetto (gli oggetti aggiunti dal Custode), altrimenti il quadratino del
/// suo colore (lo stesso delle casse nel mondo).
pub fn item_badge(
    ui: &mut egui::Ui,
    icons: &mut ItemIcons,
    world: &sim::World,
    item: sim::ItemKind,
) -> egui::Response {
    if let Some(look) = world
        .catalog()
        .get_item(item)
        .and_then(|d| d.appearance.as_ref())
    {
        let texture = icons.item(ui.ctx(), look);
        return show_icon(ui, texture, 16.0);
    }
    let (rect, response) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
    let [r, g, b, a] = crate::storage::item_color(item).to_srgba().to_u8_array();
    ui.painter()
        .rect_filled(rect, 1.0, egui::Color32::from_rgba_unmultiplied(r, g, b, a));
    response
}

/// Mostra un'icona `size` × `size` (multiplo di 16 per restare nitida).
pub fn show_icon(ui: &mut egui::Ui, texture: egui::TextureId, size: f32) -> egui::Response {
    ui.add(egui::Image::from_texture(egui::load::SizedTexture::new(
        texture,
        egui::vec2(size, size),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lato di un'icona in pixel.
    const ICON: u32 = 16;

    #[test]
    fn every_shape_is_16_by_16_and_has_a_body() {
        for shape in Shape::ALL {
            for (i, row) in template(shape).iter().enumerate() {
                assert_eq!(row.chars().count(), 16, "{shape:?} row {i}: {row:?}");
            }
            let c = shape_canvas(shape, Colour::Red);
            assert_eq!((c.width, c.height), (ICON, ICON), "{shape:?}");
            assert!(body_mask(shape).len() >= 12, "{shape:?}");
        }
        for kind in ["ricetta", "lavoro", "evento", "statistica", "altro"] {
            let c = kind_canvas(kind);
            assert_eq!((c.width, c.height), (ICON, ICON), "{kind}");
        }
    }

    #[test]
    fn colours_and_details_change_the_icon() {
        let plain = Appearance::new(Shape::Sack, Colour::Brown, None);
        let red = Appearance::new(Shape::Sack, Colour::Red, None);
        assert_ne!(icon_canvas(&plain), icon_canvas(&red));
        for detail in Detail::ALL {
            for shape in Shape::ALL {
                let with = Appearance::new(shape, Colour::Green, Some(detail));
                let without = Appearance::new(shape, Colour::Green, None);
                assert_ne!(
                    icon_canvas(&with),
                    icon_canvas(&without),
                    "{shape:?} + {detail:?} draws nothing"
                );
            }
        }
        // Valori sconosciuti: una scatola grigia, niente panico.
        let odd = Appearance {
            shape: "sfera".into(),
            colour: "fucsia".into(),
            detail: Some("fiamme".into()),
        };
        assert_eq!(icon_canvas(&odd), shape_canvas(Shape::Box, Colour::Grey));
    }
}
