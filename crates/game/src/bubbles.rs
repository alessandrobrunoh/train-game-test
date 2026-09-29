//! Fumetti delle deliberazioni e cartelli delle proteste, nel mondo.
//!
//! Solo per gli NPC disegnati (le carrozze inquadrate), e poco costosi:
//! - **pensiero** ("…" con un'icona per tipo: cuore, bebè, moneta, pugno)
//!   sopra chi ha una deliberazione aperta, cioè mentre Laya ci pensa (con le
//!   sole regole le deliberazioni si chiudono nello stesso minuto e non si vede);
//! - **fumetto** con la risposta per ~3 s reali quando si chiude ("Sì!",
//!   "Non ancora...", "Ruba!", "Protesto!"), in blu se ha deciso Laya;
//! - **cartello** sopra la Mensa dove si protesta ("Protesta: nascite!").
//!
//! L'arte è pixel art generata con `Canvas` una volta sola (fumetti a nove
//! fette, così si allargano col testo); il testo usa il font del gioco.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy::sprite::{Anchor, BorderRect, SliceScaleMode, SpriteImageMode, TextureSlicer};
use bevy::text::TextLayoutInfo;
use sim::{Choice, DeliberationId, DeliberationKind, EventKind, Grievance, NpcId, Resolver};

use crate::art::{Canvas, Rgba};
use crate::env_art::{px, rect, rgb};
use crate::npc_render::{NpcRenderSet, NpcSpriteIndex, NpcVisual};
use crate::saves::WorldRebuildSet;
use crate::state::{NpcSprite, Sim, WorldReplaced};
use crate::train::{FLOOR_Y, TrainLayout};

/// Sopra le icone di `life_fx` (7) e il marcatore di selezione (6).
const BUBBLE_Z: f32 = 8.0;
/// Distanza tra la testa e il fondo del fumetto.
const BUBBLE_GAP: f32 = 3.0;
/// Quanto resta il fumetto con la risposta (secondi reali), e l'ultimo tratto in cui svanisce.
const SPEECH_SECS: f32 = 3.0;
const SPEECH_FADE: f32 = 0.6;
/// Fumetti con la risposta al massimo contemporaneamente.
const MAX_SPEECH: usize = 12;
/// Testo: font grande e scala ridotta (come le etichette delle carrozze).
const TEXT_SIZE: f32 = 32.0;
const TEXT_SCALE: f32 = 0.16;
/// Margine orizzontale e altezza del fumetto con la risposta.
const SPEECH_PAD: f32 = 3.0;
const SPEECH_HEIGHT: f32 = 9.0;
/// Larghezza stimata di un carattere (prima che il testo sia impaginato).
const CHAR_WIDTH: f32 = 3.2;
/// Cartello della protesta: altezza del fondo dal pavimento.
const SIGN_Y: f32 = 34.0;
const SIGN_HEIGHT: f32 = 10.0;

const INK: Rgba = rgb(40, 36, 52);
const PAPER: Rgba = rgb(250, 248, 240);
const HEART: Rgba = rgb(230, 70, 100);
const SKIN: Rgba = rgb(240, 196, 160);
const SKIN_DARK: Rgba = rgb(196, 140, 110);
const BONNET: Rgba = rgb(140, 190, 240);
const GOLD: Rgba = rgb(240, 200, 60);
const GOLD_DARK: Rgba = rgb(180, 130, 30);
const WOOD: Rgba = rgb(150, 105, 60);
const WOOD_DARK: Rgba = rgb(96, 64, 36);
const BOARD: Rgba = rgb(236, 226, 196);

const TEXT_COLOR: Color = Color::srgb(0.16, 0.14, 0.2);
/// Il testo delle risposte di Laya.
const BRAIN_COLOR: Color = Color::srgb(0.12, 0.3, 0.75);
const SIGN_TEXT: Color = Color::srgb(0.6, 0.1, 0.1);

pub struct BubblesPlugin;

impl Plugin for BubblesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, make_art)
            .add_systems(
                PreUpdate,
                reset_bubbles
                    .in_set(WorldRebuildSet)
                    .run_if(on_message::<WorldReplaced>),
            )
            .add_systems(
                Update,
                (
                    thought_bubbles,
                    speech_bubbles,
                    move_bubbles,
                    protest_signs,
                    fit_text,
                )
                    .chain()
                    .after(NpcRenderSet),
            );
    }
}

// --- Arte -------------------------------------------------------------------------

/// Immagini dei fumetti (generate all'avvio).
#[derive(Resource)]
struct BubbleArt {
    /// Pensiero per tipo di deliberazione ([`DeliberationKind::index`]).
    thought: [Handle<Image>; DeliberationKind::COUNT],
    /// Fumetto a nove fette e la sua codina.
    speech: Handle<Image>,
    tail: Handle<Image>,
    /// Tavola del cartello (nove fette) e il suo palo.
    board: Handle<Image>,
    pole: Handle<Image>,
}

/// Icona 5×5 di un tipo di deliberazione.
fn icon(kind: usize) -> Canvas {
    let rows: [&str; 5] = match kind {
        // Cuore (proposta di coppia).
        0 => [".h.h.", "hhhhh", "hhhhh", ".hhh.", "..h.."],
        // Bebè con la cuffietta (figlio).
        1 => [".bbb.", "bssbb", "sksks", "sssss", ".sss."],
        // Moneta (furto).
        2 => [".ggg.", "gGgGg", "ggGgg", "gGgGg", ".ggg."],
        // Pugno alzato (protesta).
        _ => [".s.s.", "sksks", "sssss", "ssss.", ".ss.."],
    };
    Canvas::from_rows(
        &rows,
        &[
            ('h', HEART),
            ('b', BONNET),
            ('s', SKIN),
            ('k', SKIN_DARK),
            ('g', GOLD),
            ('G', GOLD_DARK),
        ],
    )
}

/// Nuvoletta di pensiero 17×13: corpo arrotondato con l'icona e tre puntini,
/// e due bollicine verso la testa (in basso a sinistra).
fn thought_canvas(kind: usize) -> Canvas {
    let mut c = Canvas::new(17, 13);
    // Corpo (coordinate dal basso: il corpo va da y=4 a y=12).
    rect(&mut c, 2, 4, 13, 9, INK);
    rect(&mut c, 1, 5, 15, 7, INK);
    rect(&mut c, 3, 5, 11, 7, PAPER);
    rect(&mut c, 2, 6, 13, 5, PAPER);
    // Bollicine.
    rect(&mut c, 3, 1, 3, 3, INK);
    px(&mut c, 4, 2, PAPER);
    px(&mut c, 1, 0, INK);
    // Icona e puntini.
    let icon = icon(kind);
    crate::env_art::stamp(&mut c, &icon, 3, 6);
    for x in [9, 11, 13] {
        px(&mut c, x, 6, INK);
    }
    c
}

/// Fumetto 9×9 a nove fette (bordo 3): angoli arrotondati.
fn speech_canvas() -> Canvas {
    let mut c = Canvas::new(9, 9);
    rect(&mut c, 1, 0, 7, 9, INK);
    rect(&mut c, 0, 1, 9, 7, INK);
    rect(&mut c, 1, 1, 7, 7, PAPER);
    c
}

/// Codina del fumetto 4×3, verso la testa in basso a sinistra.
fn tail_canvas() -> Canvas {
    Canvas::from_rows(&["kppk", "kpk.", "kk.."], &[('k', INK), ('p', PAPER)])
}

/// Tavola del cartello 9×9 a nove fette.
fn board_canvas() -> Canvas {
    let mut c = Canvas::new(9, 9);
    rect(&mut c, 0, 0, 9, 9, WOOD_DARK);
    rect(&mut c, 1, 1, 7, 7, BOARD);
    c
}

fn pole_canvas() -> Canvas {
    let mut c = Canvas::new(2, 1);
    px(&mut c, 0, 0, WOOD);
    px(&mut c, 1, 0, WOOD_DARK);
    c
}

fn make_art(mut commands: Commands, images: Option<ResMut<Assets<Image>>>) {
    let Some(mut images) = images else {
        return;
    };
    let mut add = |c: Canvas| images.add(c.to_image());
    let art = BubbleArt {
        thought: std::array::from_fn(|k| add(thought_canvas(k))),
        speech: add(speech_canvas()),
        tail: add(tail_canvas()),
        board: add(board_canvas()),
        pole: add(pole_canvas()),
    };
    commands.insert_resource(art);
}

/// Sprite a nove fette (i bordi restano a 3 pixel), largo `size`.
fn sliced(image: Handle<Image>, size: Vec2) -> Sprite {
    Sprite {
        image,
        custom_size: Some(size),
        image_mode: SpriteImageMode::Sliced(TextureSlicer {
            border: BorderRect::all(3.0),
            center_scale_mode: SliceScaleMode::Stretch,
            sides_scale_mode: SliceScaleMode::Stretch,
            max_corner_scale: 1.0,
        }),
        ..default()
    }
}

// --- Testi --------------------------------------------------------------------------

/// Cosa dice un NPC quando ha deciso.
pub(crate) fn speech_text(choice: Choice) -> &'static str {
    match choice {
        Choice::Accept => "Sì!",
        Choice::Refuse => "No, grazie.",
        Choice::AskForTime => "Ci penso...",
        Choice::TryForChild => "Proviamoci!",
        Choice::Wait => "Non ancora...",
        Choice::Steal => "Ruba!",
        Choice::Save => "Risparmio.",
        Choice::AskForHelp => "Mi aiuti?",
        Choice::Protest => "Protesto!",
        Choice::Endure => "Pazienza...",
        Choice::WorkHarder => "Al lavoro...",
    }
}

/// Testo del cartello di una protesta.
pub(crate) fn sign_text(grievance: Grievance, members: usize) -> String {
    let what = match grievance {
        Grievance::BirthDenied => "nascite",
        Grievance::FoodShortage => "razioni",
    };
    format!("Protesta: {what}! ({members})")
}

// --- Componenti -----------------------------------------------------------------------

/// Nuvoletta sopra chi sta decidendo.
#[derive(Component)]
struct ThoughtBubble {
    npc: NpcId,
    id: DeliberationId,
}

/// Fumetto con la risposta.
#[derive(Component)]
struct SpeechBubble {
    npc: NpcId,
    elapsed: f32,
    /// Ultima posizione della testa (se lo sprite sparisce il fumetto resta lì).
    head: Vec2,
}

/// Cartello di una protesta in corso.
#[derive(Component)]
struct ProtestSign {
    place: sim::CarriageId,
    grievance: Grievance,
    members: usize,
}

/// Testo di un fumetto o cartello: allarga lo sfondo (il genitore) quando è impaginato.
#[derive(Component)]
struct FitText {
    /// Altezza dello sfondo e margine orizzontale.
    height: f32,
    pad: f32,
}

/// Qualsiasi entità di questo modulo (per il reset).
#[derive(Component)]
struct BubblePart;

/// Testa dello sprite di un NPC disegnato.
fn head_of(
    index: &NpcSpriteIndex,
    sprites: &Query<(&Transform, &NpcVisual), NpcOnly>,
    id: NpcId,
) -> Option<Vec2> {
    let (transform, visual) = sprites.get(index.entity(id)?).ok()?;
    Some(transform.translation.truncate() + Vec2::Y * visual.half_height())
}

type NpcOnly = (
    With<NpcSprite>,
    Without<ThoughtBubble>,
    Without<SpeechBubble>,
);

// --- Sistemi --------------------------------------------------------------------------

fn reset_bubbles(mut commands: Commands, parts: Query<Entity, With<BubblePart>>) {
    for entity in &parts {
        commands.entity(entity).despawn();
    }
}

/// Una nuvoletta per ogni deliberazione aperta di un NPC disegnato.
fn thought_bubbles(
    mut commands: Commands,
    sim: Res<Sim>,
    art: Option<Res<BubbleArt>>,
    index: Res<NpcSpriteIndex>,
    bubbles: Query<(Entity, &ThoughtBubble)>,
) {
    let Some(art) = art else {
        return;
    };
    let world = &sim.world;
    let wanted: HashMap<NpcId, (DeliberationId, usize)> = world
        .open_deliberations()
        .iter()
        .filter(|d| index.entity(d.npc).is_some())
        .map(|d| (d.npc, (d.id, d.kind.index())))
        .collect();
    let mut shown = HashMap::new();
    for (entity, bubble) in &bubbles {
        if wanted.get(&bubble.npc).map(|w| w.0) == Some(bubble.id) {
            shown.insert(bubble.npc, ());
        } else {
            commands.entity(entity).despawn();
        }
    }
    for (&npc, &(id, kind)) in &wanted {
        if shown.contains_key(&npc) {
            continue;
        }
        commands.spawn((
            Name::new("Pensiero"),
            BubblePart,
            ThoughtBubble { npc, id },
            Sprite::from_image(art.thought[kind].clone()),
            Anchor::BOTTOM_LEFT,
            // La posizione vera la mette `move_bubbles`.
            Transform::from_xyz(0.0, -1000.0, BUBBLE_Z),
            Visibility::Hidden,
        ));
    }
}

/// Un fumetto con la risposta per ogni deliberazione chiusa di un NPC disegnato.
#[allow(clippy::too_many_arguments)]
fn speech_bubbles(
    mut commands: Commands,
    sim: Res<Sim>,
    art: Option<Res<BubbleArt>>,
    index: Res<NpcSpriteIndex>,
    sprites: Query<(&Transform, &NpcVisual), NpcOnly>,
    existing: Query<(Entity, &SpeechBubble)>,
    mut replaced: MessageReader<WorldReplaced>,
    mut cursor: Local<Option<u64>>,
) {
    let world = &sim.world;
    let total = world.events_total();
    if replaced.read().count() > 0 {
        *cursor = None;
    }
    let seen = cursor.get_or_insert(total);
    let fresh = total.saturating_sub(*seen) as usize;
    *seen = total;
    let Some(art) = art else {
        return;
    };
    if fresh == 0 {
        return;
    }
    let start = world.events.len().saturating_sub(fresh);
    let mut count = existing.iter().count();
    let mut gone: Vec<Entity> = Vec::new();
    for event in &world.events[start..] {
        let EventKind::DeliberationResolved {
            npc, choice, by, ..
        } = event.kind
        else {
            continue;
        };
        let Some(head) = head_of(&index, &sprites, npc) else {
            continue;
        };
        // Un fumetto per NPC: il nuovo sostituisce il vecchio.
        for (entity, bubble) in &existing {
            if bubble.npc == npc && !gone.contains(&entity) {
                commands.entity(entity).despawn();
                gone.push(entity);
                count = count.saturating_sub(1);
            }
        }
        if count >= MAX_SPEECH {
            continue;
        }
        count += 1;
        let text = speech_text(choice);
        let width = text.chars().count() as f32 * CHAR_WIDTH + 2.0 * SPEECH_PAD;
        let color = if by == Resolver::Brain {
            BRAIN_COLOR
        } else {
            TEXT_COLOR
        };
        commands
            .spawn((
                Name::new("Fumetto"),
                BubblePart,
                SpeechBubble {
                    npc,
                    elapsed: 0.0,
                    head,
                },
                sliced(art.speech.clone(), Vec2::new(width, SPEECH_HEIGHT)),
                Anchor::BOTTOM_LEFT,
                Transform::from_xyz(head.x, head.y + BUBBLE_GAP, BUBBLE_Z),
            ))
            .with_children(|parent| {
                parent.spawn((
                    Sprite::from_image(art.tail.clone()),
                    Anchor::TOP_LEFT,
                    Transform::from_xyz(1.0, 1.0, 0.01),
                ));
                parent.spawn((
                    FitText {
                        height: SPEECH_HEIGHT,
                        pad: SPEECH_PAD,
                    },
                    Text2d::new(text),
                    TextFont {
                        font_size: FontSize::Px(TEXT_SIZE),
                        ..default()
                    },
                    TextColor(color),
                    Anchor::CENTER,
                    Transform::from_xyz(width / 2.0, SPEECH_HEIGHT / 2.0, 0.02)
                        .with_scale(Vec3::splat(TEXT_SCALE)),
                ));
            });
    }
}

/// Pensieri e fumetti seguono la testa; i fumetti svaniscono dopo `SPEECH_SECS`.
#[allow(clippy::type_complexity)]
fn move_bubbles(
    mut commands: Commands,
    time: Res<Time<Real>>,
    index: Res<NpcSpriteIndex>,
    sprites: Query<(&Transform, &NpcVisual), NpcOnly>,
    mut thoughts: Query<
        (&ThoughtBubble, &mut Transform, &mut Visibility),
        (Without<SpeechBubble>, Without<NpcSprite>),
    >,
    mut speeches: Query<
        (
            Entity,
            &mut SpeechBubble,
            &mut Transform,
            &mut Sprite,
            &Children,
        ),
        (Without<ThoughtBubble>, Without<NpcSprite>),
    >,
    mut parts: Query<(Option<&mut Sprite>, Option<&mut TextColor>), Without<SpeechBubble>>,
) {
    let dt = time.delta_secs();
    let bob = (time.elapsed_secs() * 2.5).sin().round();
    for (bubble, mut transform, mut visibility) in &mut thoughts {
        match head_of(&index, &sprites, bubble.npc) {
            Some(head) => {
                // Leggermente a destra della testa, con le bollicine verso di lei.
                transform.translation.x = head.x - 2.0;
                transform.translation.y = head.y + BUBBLE_GAP - 1.0 + bob;
                visibility.set_if_neq(Visibility::Inherited);
            }
            None => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
    for (entity, mut bubble, mut transform, mut sprite, children) in &mut speeches {
        bubble.elapsed += dt;
        if bubble.elapsed >= SPEECH_SECS {
            commands.entity(entity).despawn();
            continue;
        }
        if let Some(head) = head_of(&index, &sprites, bubble.npc) {
            bubble.head = head;
        }
        transform.translation.x = bubble.head.x - 1.0;
        transform.translation.y = bubble.head.y + BUBBLE_GAP + 2.0;
        let alpha = ((SPEECH_SECS - bubble.elapsed) / SPEECH_FADE).min(1.0);
        sprite.color.set_alpha(alpha);
        for &child in children {
            if let Ok((part, text)) = parts.get_mut(child) {
                if let Some(mut part) = part {
                    part.color.set_alpha(alpha);
                }
                if let Some(mut text) = text {
                    text.0.set_alpha(alpha);
                }
            }
        }
    }
}

/// Un cartello sopra ogni protesta in corso (nella Mensa dove si riuniscono).
fn protest_signs(
    mut commands: Commands,
    sim: Res<Sim>,
    art: Option<Res<BubbleArt>>,
    signs: Query<(Entity, &ProtestSign)>,
) {
    let Some(art) = art else {
        return;
    };
    let world = &sim.world;
    let now = world.clock;
    let running: Vec<(sim::CarriageId, Grievance, usize)> = world
        .gatherings()
        .iter()
        .filter(|g| g.is_running(now))
        .map(|g| (g.place, g.grievance, g.members.len()))
        .collect();
    let mut kept = Vec::new();
    for (entity, sign) in &signs {
        let key = (sign.place, sign.grievance, sign.members);
        if running.contains(&key) && !kept.contains(&key) {
            kept.push(key);
        } else {
            commands.entity(entity).despawn();
        }
    }
    for (slot, &(place, grievance, members)) in running.iter().enumerate() {
        if kept.contains(&(place, grievance, members)) {
            continue;
        }
        let text = sign_text(grievance, members);
        let width = text.chars().count() as f32 * CHAR_WIDTH + 2.0 * SPEECH_PAD;
        // Due proteste nella stessa carrozza: cartelli affiancati.
        let x = TrainLayout::carriage_center_x(place.index()) + slot as f32 * 70.0
            - 35.0 * (running.len() - 1) as f32;
        let bottom = FLOOR_Y + SIGN_Y;
        commands
            .spawn((
                Name::new("Cartello"),
                BubblePart,
                ProtestSign {
                    place,
                    grievance,
                    members,
                },
                sliced(art.board.clone(), Vec2::new(width, SIGN_HEIGHT)),
                Anchor::BOTTOM_CENTER,
                Transform::from_xyz(x, bottom, BUBBLE_Z - 0.5),
            ))
            .with_children(|parent| {
                // Due pali fino all'altezza delle mani.
                for side in [-1.0, 1.0] {
                    parent.spawn((
                        Sprite {
                            image: art.pole.clone(),
                            custom_size: Some(Vec2::new(2.0, 12.0)),
                            ..default()
                        },
                        Anchor::TOP_CENTER,
                        Transform::from_xyz(side * (width / 2.0 - 4.0), 0.0, -0.01),
                    ));
                }
                parent.spawn((
                    FitText {
                        height: SIGN_HEIGHT,
                        pad: SPEECH_PAD,
                    },
                    Text2d::new(text),
                    TextFont {
                        font_size: FontSize::Px(TEXT_SIZE),
                        ..default()
                    },
                    TextColor(SIGN_TEXT),
                    Anchor::CENTER,
                    Transform::from_xyz(0.0, SIGN_HEIGHT / 2.0, 0.02)
                        .with_scale(Vec3::splat(TEXT_SCALE)),
                ));
            });
    }
}

/// Quando il testo è impaginato, lo sfondo (il genitore) si adatta alla sua larghezza.
#[allow(clippy::type_complexity)]
fn fit_text(
    mut texts: Query<
        (&TextLayoutInfo, &FitText, &ChildOf, &mut Transform),
        Changed<TextLayoutInfo>,
    >,
    mut backs: Query<(&mut Sprite, &Anchor), Without<FitText>>,
) {
    for (info, fit, child_of, mut transform) in &mut texts {
        if info.size.x <= 0.0 {
            continue;
        }
        let logical = info.size.x / info.scale_factor.max(1e-3);
        let width = (logical * transform.scale.x + 2.0 * fit.pad).round();
        let Ok((mut sprite, anchor)) = backs.get_mut(child_of.parent()) else {
            continue;
        };
        sprite.custom_size = Some(Vec2::new(width, fit.height));
        // Con l'ancora in basso a sinistra il testo va al centro della nuova larghezza.
        if *anchor == Anchor::BOTTOM_LEFT {
            transform.translation.x = width / 2.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_choice_has_a_short_line() {
        for choice in Choice::ALL {
            let text = speech_text(choice);
            assert!(!text.is_empty() && text.chars().count() <= 16, "{text}");
        }
        assert_eq!(
            sign_text(Grievance::BirthDenied, 5),
            "Protesta: nascite! (5)"
        );
    }

    #[test]
    fn bubble_art_is_small_and_pixel_sized() {
        for kind in 0..DeliberationKind::COUNT {
            let c = thought_canvas(kind);
            assert_eq!((c.width, c.height), (17, 13));
            // L'icona è disegnata (ha pixel di un colore non d'inchiostro/carta).
            assert!(
                c.pixels
                    .iter()
                    .any(|p| p[3] != 0 && *p != INK && *p != PAPER)
            );
        }
        let s = speech_canvas();
        assert_eq!((s.width, s.height), (9, 9));
        // Angoli trasparenti: arrotondato.
        assert_eq!(s.get(0, 0)[3], 0);
        assert_eq!(s.get(4, 4), PAPER);
    }
}
