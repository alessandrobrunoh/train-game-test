//! Pixel art definita nel codice: griglie di caratteri + palette → `Image`.
//!
//! Ogni sprite è scritto come righe di testo, un carattere per pixel:
//! `.` (o spazio) è trasparente, ogni altro carattere viene cercato nella
//! palette. Così l'arte vive nel repository come testo, si modifica senza
//! editor grafici e più strati (corpo, vestiti, capelli) si compongono al volo.
//! In futuro gli stessi `Handle<Image>` potranno arrivare da PNG disegnati a mano.

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

/// Un colore RGBA a 8 bit per canale (sRGB).
pub type Rgba = [u8; 4];

/// Pixel trasparente.
pub const CLEAR: Rgba = [0, 0, 0, 0];

/// Bitmap RGBA in memoria, da comporre a strati prima di diventare un'`Image`.
#[derive(Clone, Debug, PartialEq)]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    /// Pixel riga per riga, dall'alto in basso.
    pub pixels: Vec<Rgba>,
}

impl Canvas {
    /// Tela trasparente.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![CLEAR; (width * height) as usize],
        }
    }

    /// Crea una tela da righe di caratteri. Le righe più corte sono completate
    /// con pixel trasparenti. Un carattere assente dalla palette fa panic:
    /// è un errore nella definizione dell'arte, meglio accorgersene subito.
    pub fn from_rows(rows: &[&str], palette: &[(char, Rgba)]) -> Self {
        let height = rows.len() as u32;
        let width = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0) as u32;
        let mut canvas = Self::new(width, height);
        for (y, row) in rows.iter().enumerate() {
            for (x, ch) in row.chars().enumerate() {
                if ch == '.' || ch == ' ' {
                    continue;
                }
                let color = palette
                    .iter()
                    .find(|(c, _)| *c == ch)
                    .map(|(_, rgba)| *rgba)
                    .unwrap_or_else(|| panic!("carattere '{ch}' non presente nella palette"));
                canvas.set(x as u32, y as u32, color);
            }
        }
        canvas
    }

    pub fn get(&self, x: u32, y: u32) -> Rgba {
        self.pixels[(y * self.width + x) as usize]
    }

    /// Scrive un pixel; fuori dai bordi non fa nulla.
    pub fn set(&mut self, x: u32, y: u32, color: Rgba) {
        if x < self.width && y < self.height {
            self.pixels[(y * self.width + x) as usize] = color;
        }
    }

    /// Disegna `other` sopra questa tela con l'angolo in alto a sinistra in
    /// (`x`, `y`). I pixel trasparenti di `other` lasciano vedere sotto.
    pub fn overlay(&mut self, other: &Canvas, x: i32, y: i32) {
        for oy in 0..other.height {
            for ox in 0..other.width {
                let color = other.get(ox, oy);
                if color[3] == 0 {
                    continue;
                }
                let (tx, ty) = (x + ox as i32, y + oy as i32);
                if tx >= 0 && ty >= 0 {
                    self.set(tx as u32, ty as u32, color);
                }
            }
        }
    }

    /// Sostituisce ogni pixel di colore `from` con `to` (palette swap).
    pub fn recolor(&mut self, from: Rgba, to: Rgba) {
        for pixel in &mut self.pixels {
            if *pixel == from {
                *pixel = to;
            }
        }
    }


    /// Copia ruotata di 90° in senso antiorario (la cima finisce a sinistra).
    pub fn rotated_ccw(&self) -> Self {
        let mut out = Self::new(self.height, self.width);
        for y in 0..self.height {
            for x in 0..self.width {
                out.set(y, self.width - 1 - x, self.get(x, y));
            }
        }
        out
    }

    /// Vero se il pixel (`x`, `y`) esiste e non è trasparente.
    pub fn is_opaque(&self, x: i32, y: i32) -> bool {
        x >= 0
            && y >= 0
            && (x as u32) < self.width
            && (y as u32) < self.height
            && self.get(x as u32, y as u32)[3] != 0
    }

    /// Bordo di un pixel attorno alle forme: ogni pixel trasparente che tocca
    /// (in orizzontale o in verticale) un pixel pieno diventa `color`.
    pub fn outlined(&self, color: Rgba) -> Self {
        let mut out = self.clone();
        for y in 0..self.height as i32 {
            for x in 0..self.width as i32 {
                if self.is_opaque(x, y) {
                    continue;
                }
                let touches = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                    .iter()
                    .any(|&(dx, dy)| self.is_opaque(x + dx, y + dy));
                if touches {
                    out.set(x as u32, y as u32, color);
                }
            }
        }
        out
    }

    /// Converte la tela in un'`Image` Bevy (sRGB, filtro "nearest" dal default
    /// del gioco). L'immagine resta anche in memoria CPU per poterla rileggere.
    pub fn to_image(&self) -> Image {
        let data = self.pixels.iter().flatten().copied().collect();
        Image::new(
            Extent3d {
                width: self.width.max(1),
                height: self.height.max(1),
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            data,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: Rgba = [255, 0, 0, 255];
    const G: Rgba = [0, 255, 0, 255];

    #[test]
    fn rows_become_pixels() {
        let c = Canvas::from_rows(&["r.", ".g", "r"], &[('r', R), ('g', G)]);
        assert_eq!((c.width, c.height), (2, 3));
        assert_eq!(c.get(0, 0), R);
        assert_eq!(c.get(1, 0), CLEAR);
        assert_eq!(c.get(1, 1), G);
        assert_eq!(c.get(1, 2), CLEAR);
    }

    #[test]
    fn overlay_keeps_transparent_holes_and_clips() {
        let mut base = Canvas::from_rows(&["rr", "rr"], &[('r', R)]);
        let top = Canvas::from_rows(&["g.", ".g"], &[('g', G)]);
        base.overlay(&top, 1, 1);
        assert_eq!(base.get(0, 0), R);
        assert_eq!(base.get(1, 1), G);
        assert_eq!(base.get(0, 1), R);
    }

    #[test]
    fn recolor_swaps_only_matching_pixels() {
        let mut c = Canvas::from_rows(&["rg"], &[('r', R), ('g', G)]);
        c.recolor(R, G);
        assert_eq!((c.get(0, 0), c.get(1, 0)), (G, G));
    }

    #[test]
    fn rotation_puts_the_top_on_the_left() {
        // r g
        // . .
        // . .
        let c = Canvas::from_rows(&["rg", "..", ".."], &[('r', R), ('g', G)]);
        let r = c.rotated_ccw();
        assert_eq!((r.width, r.height), (3, 2));
        assert_eq!(r.get(0, 1), R);
        assert_eq!(r.get(0, 0), G);
    }

    #[test]
    fn outline_surrounds_shapes() {
        let c = Canvas::from_rows(&["...", ".r.", "..."], &[('r', R)]).outlined(G);
        assert_eq!(c.get(1, 1), R);
        assert_eq!((c.get(0, 1), c.get(2, 1), c.get(1, 0), c.get(1, 2)), (G, G, G, G));
        // Gli angoli restano vuoti.
        assert_eq!(c.get(0, 0), CLEAR);
    }

    #[test]
    fn image_has_matching_size() {
        let image = Canvas::from_rows(&["rgr"], &[('r', R), ('g', G)]).to_image();
        assert_eq!(image.width(), 3);
        assert_eq!(image.height(), 1);
        assert_eq!(image.data.as_ref().map(Vec::len), Some(12));
    }

    #[test]
    #[should_panic(expected = "non presente nella palette")]
    fn unknown_character_panics() {
        Canvas::from_rows(&["x"], &[('r', R)]);
    }
}
