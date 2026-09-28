//! Font del gioco: Pixelify Sans (SIL Open Font License 1.1, vedi
//! `assets/fonts/PixelifySans-OFL.txt`).
//!
//! Il font di default di Bevy (un sottoinsieme di Fira Mono) non ha « » né
//! tutte le lettere accentate. Qui lo si sostituisce: il file viene incluso
//! nell'eseguibile e inserito al posto del font di default, così ogni
//! `TextFont` senza font esplicito lo usa, e il gioco funziona da qualunque
//! cartella venga lanciato (niente dipendenza dalla cartella `assets/`).

use bevy::prelude::*;

/// Pixelify Sans (variabile, peso 400 di default).
const GAME_FONT: &[u8] = include_bytes!("../assets/fonts/PixelifySans.ttf");

/// Da aggiungere dopo `DefaultPlugins`: il `TextPlugin` di Bevy registra il suo
/// font di default nel proprio `build`, e questo lo sovrascrive.
pub struct FontsPlugin;

impl Plugin for FontsPlugin {
    fn build(&self, app: &mut App) {
        app.world_mut()
            .resource_mut::<Assets<Font>>()
            .insert(AssetId::default(), Font::from_bytes(GAME_FONT.to_vec()))
            .expect("l'handle del font di default è sempre valido");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_font_has_guillemets_and_italian_accents() {
        let face = ttf_parser::Face::parse(GAME_FONT, 0).expect("font valido");
        for c in "«»àèéìòùÀÈÉÌÒÙ'\"-".chars() {
            assert!(
                face.glyph_index(c).is_some_and(|g| g.0 != 0),
                "manca il glifo {c:?}"
            );
        }
    }

    #[test]
    fn replaces_bevy_default_font() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            bevy::text::TextPlugin,
            FontsPlugin,
        ));
        let fonts = app.world().resource::<Assets<Font>>();
        let font = fonts.get(AssetId::default()).expect("font di default");
        assert_eq!(font.data.as_ref(), GAME_FONT);
    }
}
