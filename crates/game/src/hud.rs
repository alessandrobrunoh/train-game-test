//! HUD minimale: chi è il giocatore, quanti gettoni ha e in quale carrozza
//! si trova (e se è nella sua cabina).

use bevy::prelude::*;

use crate::player::Player;
use crate::state::Sim;
use crate::train::{TrainLayout, TrainLocation};

#[derive(Component)]
struct LocationText;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_hud)
            .add_systems(Update, update_location_text);
    }
}

fn spawn_hud(mut commands: Commands) {
    let font = TextFont {
        font_size: FontSize::Px(20.0),
        ..default()
    };
    // In basso al centro: in alto ci sono i pannelli egui (tempo, carrozze).
    commands
        .spawn(Node {
            position_type: PositionType::Absolute,
            left: px(0),
            right: px(0),
            bottom: px(40),
            justify_content: JustifyContent::Center,
            ..default()
        })
        .with_child((
            LocationText,
            Text::new(""),
            font.clone(),
            TextColor(Color::WHITE),
        ));
    commands.spawn((
        Text::new(
            "A/D: muovi   Spazio/W: salta   W/S: scale   Z: vista piani   E: interagisci (in cabina: letto, baule)   Q: cambia oggetto   I: inventario   C: crafting   M: mercato   P: pausa   1-5: velocità   Click: seleziona NPC   F: segui NPC   V: fumetti   G: popolazione   H: storia   B: cervello   F5: salva   F9: carica   Esc: partite",
        ),
        TextFont {
            font_size: FontSize::Px(14.0),
            ..font
        },
        TextColor(Color::srgba(1.0, 1.0, 1.0, 0.8)),
        Node {
            position_type: PositionType::Absolute,
            bottom: px(12),
            left: px(12),
            ..default()
        },
    ));
}

fn update_location_text(
    layout: Res<TrainLayout>,
    sim: Res<Sim>,
    player: Single<&Transform, With<Player>>,
    mut text: Single<&mut Text, With<LocationText>>,
) {
    let place = match layout.location_at(player.translation.x) {
        TrainLocation::Carriage(i) => match sim.world.carriages.get(i) {
            Some(c) => c.label().to_string(),
            None => format!("Carrozza {}", i + 1),
        },
        TrainLocation::Gangway(i) => format!("Passaggio {} -> {}", i + 1, i + 2),
    };
    let me = &sim.world.player;
    let home = if me.at_home() {
        " · nella tua cabina"
    } else {
        ""
    };
    let label = format!("{} · {} gettoni    {place}{home}", me.name, me.tokens);
    // Aggiorna solo se cambia, per non segnare il testo come modificato ogni frame.
    if text.0 != label {
        text.0 = label;
    }
}
