//! HUD minimale: chi è il giocatore, quanti gettoni ha e in quale carrozza
//! si trova (e se è nella sua cabina), con la barra della salute.

use bevy::prelude::*;

use crate::player::Player;
use crate::state::Sim;
use crate::train::{TrainLayout, TrainLocation};

#[derive(Component)]
struct LocationText;

/// Barra della salute del giocatore (la parte piena) e il suo numero.
#[derive(Component)]
struct HealthFill;

#[derive(Component)]
struct HealthText;

/// Larghezza della barra della salute (pixel logici).
const HEALTH_BAR_WIDTH: f32 = 120.0;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_hud)
            .add_systems(Update, (update_location_text, update_health_bar));
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
        .with_children(|row| {
            row.spawn((
                LocationText,
                Text::new(""),
                font.clone(),
                TextColor(Color::WHITE),
            ));
            // Salute: barra e numero, a destra del posto.
            row.spawn((
                Node {
                    width: px(HEALTH_BAR_WIDTH),
                    height: px(10),
                    margin: UiRect::left(px(16)),
                    align_self: AlignSelf::Center,
                    border: UiRect::all(px(1)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.1, 0.05, 0.05, 0.8)),
                BorderColor::all(Color::srgba(1.0, 1.0, 1.0, 0.5)),
            ))
            .with_child((
                HealthFill,
                Node {
                    width: percent(100),
                    height: percent(100),
                    ..default()
                },
                BackgroundColor(Color::srgb(0.35, 0.85, 0.35)),
            ));
            row.spawn((
                HealthText,
                Text::new(""),
                TextFont {
                    font_size: FontSize::Px(16.0),
                    ..font.clone()
                },
                TextColor(Color::WHITE),
                Node {
                    margin: UiRect::left(px(6)),
                    ..default()
                },
            ));
        });
    commands.spawn((
        Text::new(
            "A/D: muovi   Spazio/W: salta   W/S: scale   Z: vista piani   E: interagisci (in cabina: letto, baule)   T: parla con un NPC   X: colpisci (2 volte chi non è ostile)   Q: cambia oggetto   I: inventario   C: crafting   M: mercato   P: pausa   1-5: velocità   Click: seleziona NPC   F: segui NPC   V: fumetti   G: popolazione   H: storia   B: cervello   N: cronaca   K: statistiche   J: bande   F5: salva   F9: carica   Esc: partite",
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

/// La barra della salute del giocatore: lunga quanto la salute, verde,
/// gialla da ferito, rossa da grave.
fn update_health_bar(
    sim: Res<Sim>,
    mut fill: Single<(&mut Node, &mut BackgroundColor), With<HealthFill>>,
    mut text: Single<&mut Text, (With<HealthText>, Without<LocationText>)>,
) {
    let world = &sim.world;
    let health = world.player.health.clamp(0.0, sim::MAX_HEALTH);
    let width = percent(100.0 * health / sim::MAX_HEALTH);
    let color = match sim::Condition::of(health, &world.params) {
        sim::Condition::Healthy => Color::srgb(0.35, 0.85, 0.35),
        sim::Condition::Hurt => Color::srgb(0.95, 0.8, 0.2),
        sim::Condition::Bedridden => Color::srgb(0.9, 0.2, 0.15),
    };
    let (node, background) = &mut *fill;
    if node.width != width {
        node.width = width;
    }
    if background.0 != color {
        background.0 = color;
    }
    let label = format!("Salute {:.0}", health.ceil());
    if text.0 != label {
        text.0 = label;
    }
}
