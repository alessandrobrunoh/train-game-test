//! How a new item looks: shape, colour and an optional detail, from closed
//! lists. The game draws a 16×16 pixel-art icon from them (`item_icons.rs`
//! in `game`); the model never draws.
//!
//! The values stay strings in the draft so that the Narratore's precheck
//! can reject an unknown one with an Italian reason that lists the allowed
//! values; [`Appearance::shape`] and friends parse them. The Custode keeps
//! the appearance in the item's definition ([`crate::ItemDef::appearance`]),
//! so the icon survives saves and reloads.
//!
//! ```json
//! "aspetto": {"forma": "stoffa", "colore": "marrone", "dettaglio": "toppa"}
//! ```

use serde::{Deserialize, Serialize};

use crate::custode::names::normalize;

/// Shapes of an item icon, in the prompt's order.
pub const SHAPES: [&str; 16] = [
    "barra",
    "lingotto",
    "sacco",
    "bottiglia",
    "vasetto",
    "attrezzo",
    "stoffa",
    "rotolo",
    "pagnotta",
    "foglia",
    "lampada",
    "giocattolo",
    "scatola",
    "moneta",
    "gemma",
    "libro",
];

/// Named colours of the icon palette, in the prompt's order.
pub const COLOURS: [&str; 13] = [
    "rosso", "arancio", "giallo", "oro", "verde", "azzurro", "blu", "viola", "rosa", "marrone",
    "grigio", "bianco", "nero",
];

/// Overlays drawn on top of the shape.
pub const DETAILS: [&str; 6] = [
    "strisce",
    "puntini",
    "etichetta",
    "brillio",
    "crepa",
    "toppa",
];

/// A shape of [`SHAPES`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shape {
    Bar,
    Ingot,
    Sack,
    Bottle,
    Jar,
    Tool,
    Cloth,
    Roll,
    Loaf,
    Leaf,
    Lamp,
    Toy,
    Box,
    Coin,
    Gem,
    Book,
}

impl Shape {
    pub const ALL: [Shape; 16] = [
        Shape::Bar,
        Shape::Ingot,
        Shape::Sack,
        Shape::Bottle,
        Shape::Jar,
        Shape::Tool,
        Shape::Cloth,
        Shape::Roll,
        Shape::Loaf,
        Shape::Leaf,
        Shape::Lamp,
        Shape::Toy,
        Shape::Box,
        Shape::Coin,
        Shape::Gem,
        Shape::Book,
    ];

    /// The Italian name in [`SHAPES`].
    pub fn name(self) -> &'static str {
        SHAPES[self as usize]
    }
}

/// A colour of [`COLOURS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Colour {
    Red,
    Orange,
    Yellow,
    Gold,
    Green,
    Sky,
    Blue,
    Violet,
    Pink,
    Brown,
    Grey,
    White,
    Black,
}

impl Colour {
    pub const ALL: [Colour; 13] = [
        Colour::Red,
        Colour::Orange,
        Colour::Yellow,
        Colour::Gold,
        Colour::Green,
        Colour::Sky,
        Colour::Blue,
        Colour::Violet,
        Colour::Pink,
        Colour::Brown,
        Colour::Grey,
        Colour::White,
        Colour::Black,
    ];

    pub fn name(self) -> &'static str {
        COLOURS[self as usize]
    }

    /// The base sRGB colour of the palette (muted, to sit with the train's
    /// pixel art).
    pub fn rgb(self) -> [u8; 3] {
        match self {
            Colour::Red => [178, 58, 52],
            Colour::Orange => [210, 118, 48],
            Colour::Yellow => [222, 190, 70],
            Colour::Gold => [196, 150, 40],
            Colour::Green => [88, 150, 70],
            Colour::Sky => [104, 170, 214],
            Colour::Blue => [58, 88, 168],
            Colour::Violet => [128, 82, 160],
            Colour::Pink => [214, 120, 150],
            Colour::Brown => [128, 86, 54],
            Colour::Grey => [134, 138, 146],
            Colour::White => [226, 224, 214],
            Colour::Black => [66, 64, 76],
        }
    }
}

/// An overlay of [`DETAILS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Detail {
    Stripes,
    Dots,
    Label,
    Shine,
    Crack,
    Patch,
}

impl Detail {
    pub const ALL: [Detail; 6] = [
        Detail::Stripes,
        Detail::Dots,
        Detail::Label,
        Detail::Shine,
        Detail::Crack,
        Detail::Patch,
    ];

    pub fn name(self) -> &'static str {
        DETAILS[self as usize]
    }
}

/// A detail that is not one of [`DETAILS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnknownDetail;

/// The look of a new item, as the model writes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Appearance {
    #[serde(rename = "forma")]
    pub shape: String,
    #[serde(rename = "colore")]
    pub colour: String,
    #[serde(rename = "dettaglio", default)]
    pub detail: Option<String>,
}

fn find<T: Copy>(all: &[T], names: &[&str], value: &str) -> Option<T> {
    let key = normalize(value);
    names.iter().position(|n| *n == key).map(|i| all[i])
}

impl Appearance {
    pub fn new(shape: Shape, colour: Colour, detail: Option<Detail>) -> Self {
        Self {
            shape: shape.name().to_string(),
            colour: colour.name().to_string(),
            detail: detail.map(|d| d.name().to_string()),
        }
    }

    /// The shape, if it is one of [`SHAPES`] (case and accents ignored).
    pub fn shape(&self) -> Option<Shape> {
        find(&Shape::ALL, &SHAPES, &self.shape)
    }

    pub fn colour(&self) -> Option<Colour> {
        find(&Colour::ALL, &COLOURS, &self.colour)
    }

    /// `Ok(None)` without a detail (or "nessuno"); `Err` for an unknown one.
    pub fn detail(&self) -> Result<Option<Detail>, UnknownDetail> {
        match self.detail.as_deref().map(normalize).as_deref() {
            None | Some("") | Some("nessuno") | Some("nessuna") => Ok(None),
            Some(d) => find(&Detail::ALL, &DETAILS, d)
                .map(Some)
                .ok_or(UnknownDetail),
        }
    }

    /// A stable key for icon caches: "stoffa/marrone/toppa".
    pub fn key(&self) -> String {
        format!(
            "{}/{}/{}",
            normalize(&self.shape),
            normalize(&self.colour),
            self.detail.as_deref().map(normalize).unwrap_or_default()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_lists_agree() {
        for (i, s) in Shape::ALL.iter().enumerate() {
            assert_eq!(s.name(), SHAPES[i]);
        }
        for (i, c) in Colour::ALL.iter().enumerate() {
            assert_eq!(c.name(), COLOURS[i]);
        }
        for (i, d) in Detail::ALL.iter().enumerate() {
            assert_eq!(d.name(), DETAILS[i]);
        }
    }

    #[test]
    fn parses_leniently() {
        let a: Appearance =
            serde_json::from_str(r#"{"forma":"Bottiglia","colore":"VERDE","dettaglio":"brillio"}"#)
                .unwrap();
        assert_eq!(a.shape(), Some(Shape::Bottle));
        assert_eq!(a.colour(), Some(Colour::Green));
        assert_eq!(a.detail(), Ok(Some(Detail::Shine)));
        let plain: Appearance =
            serde_json::from_str(r#"{"forma":"sacco","colore":"marrone"}"#).unwrap();
        assert_eq!(plain.detail(), Ok(None));
        let bad = Appearance {
            shape: "sfera".into(),
            colour: "fucsia".into(),
            detail: Some("fiamme".into()),
        };
        assert_eq!(bad.shape(), None);
        assert_eq!(bad.colour(), None);
        assert!(bad.detail().is_err());
        assert_eq!(
            Appearance::new(Shape::Cloth, Colour::Brown, Some(Detail::Patch)).key(),
            "stoffa/marrone/toppa"
        );
    }
}
