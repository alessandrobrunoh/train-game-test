//! Fumetti dei dialoghi tra NPC (Fase 5.3 di `docs/piano-vita-ed-economia.md`).
//!
//! Per ogni conversazione della sim ([`World::conversations`]) tra NPC
//! disegnati, un fumetto sopra la testa di chi parla con la battuta corrente
//! ([`Conversation::line_at`]), a capo su al massimo due righe. Il fondo è
//! pixel art a nove fette come i fumetti di `bubbles.rs`, colorato col tono:
//! bianco caldo se amichevole, grigio se neutro, bordo rosso se teso. Il
//! fumetto si allarga dalla parte opposta all'interlocutore, così la sua testa
//! resta libera per una piccola reazione (cuoricino, nota, "!").
//!
//! **Ritmo.** Le battute seguono l'ora di gioco, ma ognuna resta almeno
//! [`MIN_LINE_SECS`] secondi reali; se nel frattempo ne sono state dette altre
//! si salta all'ultima (vedi [`pace`]). Oltre [`FAST_SPEED`] minuti di gioco
//! al secondo il testo lampeggerebbe: si vede solo "…" (o "!" se il tono è teso).
//!
//! **Quanti.** Al massimo [`MAX_TEXT`] fumetti con il testo: prima quello
//! dell'NPC selezionato, poi i più vicini al centro dell'inquadratura; gli
//! altri hanno solo "…" (vedi [`assign_slots`]). Chi ha un fumetto di
//! deliberazione (`bubbles.rs`) non ne ha uno di dialogo: mai due fumetti
//! sulla stessa testa. Fumetti vicini si impilano (vedi [`stack`]).
//!
//! **Il giocatore.** Gli NPC che lo conoscono e gli vogliono bene lo salutano
//! (`World::greetings`): un fumetto caldo sopra la loro testa per almeno
//! [`GREET_SECS`] secondi reali ([`GreetBubble`]). Chi ha qualcosa da dirgli
//! (`World::wants_to_talk`, per ora un amico che l'ha appena salutato; poi
//! la chat e gli incarichi della Fase 5.4) ha un "!" giallo sopra la testa.
//!
//! **La chat** (`chat.rs`): le battute tra il giocatore e l'NPC con cui parla
//! ([`ChatSays`]) compaiono come fumetti sopra le loro teste (grigio per il
//! giocatore), per qualche secondo reale; intanto saluto e "!" di quell'NPC
//! non si vedono.
//!
//! Il tasto **V** passa tra tutti i fumetti, solo quello del selezionato e
//! nessuno ([`SpeechMode`]). L'ispettore mostra la conversazione in corso e
//! le ultime concluse ([`conversation_section`]).

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use bevy::sprite::{Anchor, BorderRect, SliceScaleMode, SpriteImageMode, TextureSlicer};
use bevy::text::{Justify, LineBreak, TextLayoutInfo};
use bevy_egui::egui::{self, Color32, RichText};
use sim::{Conversation, ConversationId, GameTime, Line, Npc, NpcId, Tone, World};

use crate::art::{Canvas, Rgba};
use crate::bubbles::{BubblesSet, DeliberationHeads};
use crate::chat::ChatWindow;
use crate::env_art::{px, rect, rgb};
use crate::life_fx::head_of;
use crate::npc_render::{NpcRenderSet, NpcSpriteIndex, NpcVisual};
use crate::player::Player;
use crate::saves::WorldRebuildSet;
use crate::sim_bridge::DeliberationGrace;
use crate::state::{NpcSprite, SelectedNpc, Sim, SimClock, WorldReplaced};

/// Sotto i fumetti delle deliberazioni (8), sopra le icone di `life_fx` (7).
const CHAT_Z: f32 = 7.8;
const REACTION_Z: f32 = 7.2;
/// Distanza tra la testa e il fondo del fumetto.
const GAP: f32 = 3.0;
/// Oltre questa velocità (minuti di gioco al secondo) niente testo, solo "…".
pub(crate) const FAST_SPEED: f32 = 10.0;
/// Tempo reale minimo per cui resta una battuta (e un "…" dopo la fine).
pub(crate) const MIN_LINE_SECS: f64 = 2.5;
const MIN_MARK_SECS: f64 = 1.0;
/// Durata della dissolvenza in entrata e in uscita (secondi reali).
const FADE_SECS: f32 = 0.25;
/// Fumetti con il testo e con i soli puntini, al massimo.
pub(crate) const MAX_TEXT: usize = 5;
pub(crate) const MAX_MARKS: usize = 12;
/// Impaginazione: caratteri per riga e righe al massimo.
pub(crate) const LINE_CHARS: usize = 22;
pub(crate) const MAX_LINES: usize = 2;
/// Testo: font grande e scala ridotta (come `bubbles.rs`).
const TEXT_SIZE: f32 = 32.0;
const TEXT_SCALE: f32 = 0.16;
/// Margini del testo nel fumetto, e stime prima che il testo sia impaginato.
const PAD_X: f32 = 3.0;
const PAD_Y: f32 = 2.0;
const CHAR_WIDTH: f32 = 3.2;
const LINE_HEIGHT: f32 = 6.0;
/// Margine attorno all'inquadratura entro cui una testa conta come visibile.
const VIEW_MARGIN: f32 = 4.0;
/// Un fumetto che ha già il testo lo tiene finché un altro non è più vicino
/// al centro di così tanto (niente scambi continui mentre la camera si muove).
const KEEP_BONUS: f32 = 40.0;
/// Spazio verticale tra due fumetti impilati.
const STACK_GAP: f32 = 1.0;
/// Reazioni di chi ascolta: durata, salita e probabilità a ogni battuta.
const REACTION_SECS: f32 = 1.6;
const REACTION_RISE: f32 = 4.0;
const REACTION_CHANCE: f32 = 0.45;

const INK: Rgba = rgb(40, 36, 52);
const PAPER_WARM: Rgba = rgb(255, 246, 226);
const PAPER_GREY: Rgba = rgb(218, 218, 224);
const TENSE_BORDER: Rgba = rgb(176, 36, 44);
const TENSE_PAPER: Rgba = rgb(255, 236, 232);
const HEART: Rgba = rgb(230, 70, 100);
const NOTE: Rgba = rgb(70, 110, 210);
const TALK_BORDER: Rgba = rgb(150, 110, 20);
const TALK_PAPER: Rgba = rgb(255, 226, 110);
/// I saluti stanno davanti al giocatore (10), che di solito è lì accanto.
const GREET_Z: f32 = 10.5;
/// Un saluto al giocatore resta almeno tanti secondi reali.
pub(crate) const GREET_SECS: f64 = 3.0;
/// Il "!" di chi ha qualcosa da dire ondeggia di tanto (unità) e così veloce.
const MARK_BOB: f32 = 1.5;
const MARK_BOB_SPEED: f32 = 3.0;

const TEXT_COLOR: Color = Color::srgb(0.16, 0.14, 0.2);
const TENSE_TEXT: Color = Color::srgb(0.42, 0.08, 0.1);

pub struct SpeechPlugin;

impl Plugin for SpeechPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpeechMode>()
            .init_resource::<ChatSays>()
            .add_systems(Startup, make_art)
            .add_systems(
                PreUpdate,
                reset_speech
                    .in_set(WorldRebuildSet)
                    .run_if(on_message::<WorldReplaced>),
            )
            .add_systems(
                Update,
                (
                    speech_keys,
                    update_speech,
                    place_speech,
                    move_reactions,
                    update_greetings,
                    center_greet_texts,
                    update_talk_marks,
                    update_chat_says,
                    center_say_texts,
                )
                    .chain()
                    .after(BubblesSet)
                    .after(NpcRenderSet),
            );
    }
}

// --- Modalità (tasto V) --------------------------------------------------------------

/// Quali fumetti dei dialoghi mostrare.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SpeechMode {
    #[default]
    All,
    /// Solo quello dell'NPC selezionato.
    Selected,
    Off,
}

impl SpeechMode {
    pub(crate) fn next(self) -> Self {
        match self {
            SpeechMode::All => SpeechMode::Selected,
            SpeechMode::Selected => SpeechMode::Off,
            SpeechMode::Off => SpeechMode::All,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            SpeechMode::All => "tutti",
            SpeechMode::Selected => "solo il selezionato",
            SpeechMode::Off => "spenti",
        }
    }
}

// --- Testi ------------------------------------------------------------------------------

/// Bordo e carta del fumetto per ogni tono (indice di [`Tone::index`]).
const TONE_COLORS: [(Rgba, Rgba); Tone::COUNT] = [
    (INK, PAPER_WARM),
    (INK, PAPER_GREY),
    (TENSE_BORDER, TENSE_PAPER),
];

/// Tronca una parola troppo lunga a `width` caratteri, l'ultimo è "…".
fn cut_word(word: &str, width: usize) -> String {
    if word.chars().count() <= width {
        return word.to_string();
    }
    let mut cut: String = word.chars().take(width.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// Manda a capo `text` su al massimo `max_lines` righe di `width` caratteri,
/// senza spezzare le parole (quelle più lunghe di una riga si troncano con
/// "…"). Se non ci sta tutto, l'ultima riga finisce con "…".
pub(crate) fn wrap_text(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let width = width.max(2);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut truncated = false;
    for word in text.split_whitespace() {
        let word = cut_word(word, width);
        if current.is_empty() {
            current = word;
        } else if current.chars().count() + 1 + word.chars().count() <= width {
            current.push(' ');
            current.push_str(&word);
        } else {
            lines.push(std::mem::replace(&mut current, word));
            if lines.len() >= max_lines {
                truncated = true;
                break;
            }
        }
    }
    if !truncated && !current.is_empty() {
        if lines.len() < max_lines {
            lines.push(current);
        } else {
            truncated = true;
        }
    }
    if !truncated && lines.len() == 2 {
        return balance(&lines, width);
    }
    if truncated
        && let Some(last) = lines.last_mut()
        && !last.ends_with('…')
    {
        // Via le ultime parole finché ci sta anche "…".
        while last.chars().count() + 1 > width {
            match last.rsplit_once(' ') {
                Some((head, _)) => *last = head.to_string(),
                None => *last = last.chars().take(width - 1).collect(),
            }
        }
        last.push('…');
    }
    lines
}

/// Due righe di lunghezza simile ("Un giorno ti / offro un tè." invece di
/// "Un giorno ti offro un / tè."), ognuna entro `width` caratteri.
fn balance(lines: &[String], width: usize) -> Vec<String> {
    let words: Vec<&str> = lines.iter().flat_map(|l| l.split(' ')).collect();
    let len = |ws: &[&str]| ws.iter().map(|w| w.chars().count()).sum::<usize>() + ws.len() - 1;
    let best = (1..words.len())
        .map(|k| (len(&words[..k]).max(len(&words[k..])), k))
        .filter(|&(longest, _)| longest <= width)
        .min();
    match best {
        Some((_, k)) => vec![words[..k].join(" "), words[k..].join(" ")],
        None => lines.to_vec(),
    }
}

// --- Ritmo delle battute (dati puri) ---------------------------------------------------

/// Quale battuta di una conversazione è sullo schermo e da quando (secondi reali).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Pacer {
    line: Option<usize>,
    since: f64,
}

/// Cosa c'è nel fumetto.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Shown {
    /// La battuta, già a capo (righe separate da `\n`).
    Text(String),
    /// Solo "…": troppo veloce, o troppi fumetti.
    Dots,
    /// "!" al posto dei puntini, se il tono è teso.
    Bang,
}

/// La battuta da mostrare all'ora di gioco `now` e all'istante reale `real`:
/// quella detta per ultima, ma senza cambiare prima che la precedente sia
/// rimasta [`MIN_LINE_SECS`] secondi (le battute intermedie si saltano).
/// Restituisce chi parla e cosa mostrare: il testo se `text`, altrimenti
/// "…" o "!". `None` finché nessuno ha ancora parlato.
pub(crate) fn pace(
    conv: &Conversation,
    now: GameTime,
    real: f64,
    text: bool,
    pacer: &mut Pacer,
) -> Option<(NpcId, Shown)> {
    let spoken = conv.lines.iter().take_while(|l| l.at <= now).count();
    if let Some(target) = spoken.checked_sub(1) {
        let advance = match pacer.line {
            None => true,
            // Il tempo è tornato indietro (non dovrebbe): si segue la sim.
            Some(current) if target < current => true,
            Some(current) => target > current && real - pacer.since >= MIN_LINE_SECS,
        };
        if advance {
            pacer.line = Some(target);
            pacer.since = real;
        }
    }
    let line = conv.lines.get(pacer.line?)?;
    Some((line.speaker, shown_for(line, conv.tone, text)))
}

fn shown_for(line: &Line, tone: Tone, text: bool) -> Shown {
    if text {
        Shown::Text(wrap_text(&line.text, LINE_CHARS, MAX_LINES).join("\n"))
    } else if tone == Tone::Tense {
        Shown::Bang
    } else {
        Shown::Dots
    }
}

// --- Chi ha il fumetto (dati puri) -----------------------------------------------------

/// Una conversazione con chi parla disegnato.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Candidate {
    pub speaker: NpcId,
    /// Coinvolge l'NPC selezionato.
    pub selected: bool,
    /// La testa di chi parla è nell'inquadratura.
    pub on_screen: bool,
    /// Distanza della testa dal centro dell'inquadratura (meno un bonus per
    /// chi ha già il testo).
    pub distance: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    /// Fumetto con la battuta.
    Text,
    /// Fumetto piccolo con "…".
    Mark,
    Hidden,
}

/// Che fumetto dare a ogni candidato: niente se i fumetti sono spenti, se chi
/// parla è fuori dall'inquadratura o ha già un fumetto di deliberazione
/// (`busy`, che ha la precedenza); poi il testo ai primi [`MAX_TEXT`] (il
/// selezionato, poi i più vicini al centro), i puntini ai successivi
/// [`MAX_MARKS`]. Con [`SpeechMode::Selected`] solo il selezionato.
pub(crate) fn assign_slots(
    candidates: &[Candidate],
    mode: SpeechMode,
    busy: &HashSet<NpcId>,
) -> Vec<Slot> {
    let mut slots = vec![Slot::Hidden; candidates.len()];
    if mode == SpeechMode::Off {
        return slots;
    }
    let mut order: Vec<usize> = (0..candidates.len())
        .filter(|&i| {
            let c = &candidates[i];
            c.on_screen && !busy.contains(&c.speaker) && (mode == SpeechMode::All || c.selected)
        })
        .collect();
    order.sort_by(|&i, &j| {
        let (a, b) = (&candidates[i], &candidates[j]);
        b.selected
            .cmp(&a.selected)
            .then(a.distance.total_cmp(&b.distance))
            .then(a.speaker.cmp(&b.speaker))
    });
    for (rank, i) in order.into_iter().enumerate() {
        slots[i] = if rank < MAX_TEXT {
            Slot::Text
        } else if rank < MAX_TEXT + MAX_MARKS {
            Slot::Mark
        } else {
            Slot::Hidden
        };
    }
    slots
}

/// Impila i fumetti (in ordine di priorità): chi si sovrappone a uno già
/// messo sale sopra di lui. Restituisce di quanto sale ognuno.
pub(crate) fn stack(rects: &[Rect]) -> Vec<f32> {
    let overlaps = |a: &Rect, b: &Rect| {
        a.min.x < b.max.x
            && a.max.x > b.min.x
            && a.min.y < b.max.y + STACK_GAP
            && a.max.y + STACK_GAP > b.min.y
    };
    let mut placed: Vec<Rect> = Vec::with_capacity(rects.len());
    let mut offsets = Vec::with_capacity(rects.len());
    for rect in rects {
        let mut moved = *rect;
        // Ogni spostamento porta sopra un fumetto già messo: al più tanti giri.
        for _ in 0..=placed.len() {
            let Some(top) = placed
                .iter()
                .filter(|p| overlaps(&moved, p))
                .map(|p| p.max.y)
                .reduce(f32::max)
            else {
                break;
            };
            let rise = top + STACK_GAP - moved.min.y;
            moved.min.y += rise;
            moved.max.y += rise;
        }
        offsets.push(moved.min.y - rect.min.y);
        placed.push(moved);
    }
    offsets
}

// --- Arte -------------------------------------------------------------------------------

/// Immagini dei fumetti (generate all'avvio), per tono ([`Tone::index`]).
#[derive(Resource)]
struct SpeechArt {
    /// Fondo a nove fette e codina.
    body: [Handle<Image>; Tone::COUNT],
    tail: [Handle<Image>; Tone::COUNT],
    /// Fumetto piccolo con i puntini, e con "!" (teso).
    dots: [Handle<Image>; Tone::COUNT],
    bang: Handle<Image>,
    /// "!" giallo: ha qualcosa da dire al giocatore.
    talk: Handle<Image>,
    /// Reazioni di chi ascolta.
    heart: Handle<Image>,
    note: Handle<Image>,
    alarm: Handle<Image>,
}

/// Fondo 9×9 a nove fette (bordo 3), angoli arrotondati.
fn body_canvas((border, paper): (Rgba, Rgba)) -> Canvas {
    let mut c = Canvas::new(9, 9);
    rect(&mut c, 1, 0, 7, 9, border);
    rect(&mut c, 0, 1, 9, 7, border);
    rect(&mut c, 1, 1, 7, 7, paper);
    c
}

/// Codina 4×3, verso la testa in basso a sinistra (la prima riga apre il bordo).
fn tail_canvas((border, paper): (Rgba, Rgba)) -> Canvas {
    Canvas::from_rows(&["kppk", "kpk.", "kk.."], &[('k', border), ('p', paper)])
}

/// Fumetto piccolo 11×7 con tre puntini.
fn dots_canvas((border, paper): (Rgba, Rgba)) -> Canvas {
    let mut c = Canvas::new(11, 7);
    rect(&mut c, 1, 0, 9, 7, border);
    rect(&mut c, 0, 1, 11, 5, border);
    rect(&mut c, 1, 1, 9, 5, paper);
    for x in [3, 5, 7] {
        px(&mut c, x, 3, INK);
    }
    c
}

/// Fumetto piccolo 7×7 con "!" (conversazione tesa, ad alta velocità).
fn bang_canvas() -> Canvas {
    Canvas::from_rows(
        &[
            ".kkkkk.", "kppRppk", "kppRppk", "kppRppk", "kpppppk", "kppRppk", ".kkkkk.",
        ],
        &[('k', TENSE_BORDER), ('p', TENSE_PAPER), ('R', TENSE_BORDER)],
    )
}

/// Fumetto piccolo 7×9 con un "!" (ha qualcosa da dire al giocatore).
fn talk_canvas() -> Canvas {
    Canvas::from_rows(
        &[
            ".kkkkk.", "kppRppk", "kppRppk", "kppRppk", "kpppppk", "kppRppk", ".kkkkk.", "..kk...",
            "..k....",
        ],
        &[('k', TALK_BORDER), ('p', TALK_PAPER), ('R', TALK_BORDER)],
    )
}

fn heart_canvas() -> Canvas {
    Canvas::from_rows(
        &[".h.h.", "hhhhh", "hhhhh", ".hhh.", "..h.."],
        &[('h', HEART)],
    )
}

fn note_canvas() -> Canvas {
    Canvas::from_rows(
        &["..nn.", "..n.n", "..n..", "nnn..", "nnn.."],
        &[('n', NOTE)],
    )
}

fn alarm_canvas() -> Canvas {
    Canvas::from_rows(&["rr", "rr", "rr", "..", "rr"], &[('r', TENSE_BORDER)])
}

fn make_art(mut commands: Commands, images: Option<ResMut<Assets<Image>>>) {
    let Some(mut images) = images else {
        return;
    };
    let mut add = |c: Canvas| images.add(c.to_image());
    let art = SpeechArt {
        body: TONE_COLORS.map(|t| add(body_canvas(t))),
        tail: TONE_COLORS.map(|t| add(tail_canvas(t))),
        dots: TONE_COLORS.map(|t| add(dots_canvas(t))),
        bang: add(bang_canvas()),
        talk: add(talk_canvas()),
        heart: add(heart_canvas()),
        note: add(note_canvas()),
        alarm: add(alarm_canvas()),
    };
    commands.insert_resource(art);
}

fn slicer() -> SpriteImageMode {
    SpriteImageMode::Sliced(TextureSlicer {
        border: BorderRect::all(3.0),
        center_scale_mode: SliceScaleMode::Stretch,
        sides_scale_mode: SliceScaleMode::Stretch,
        max_corner_scale: 1.0,
    })
}

// --- Componenti -------------------------------------------------------------------------

/// A che punto è un fumetto.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Life {
    Active,
    /// Tolto (fumetti spenti, troppi, deliberazione): svanisce subito.
    Hidden,
    /// Conversazione finita: resta fino a `hold` (secondi reali), poi svanisce.
    Ended {
        hold: f64,
    },
}

/// Fumetto di una conversazione; il fondo è lo sprite dell'entità.
#[derive(Component)]
struct ChatBubble {
    conv: ConversationId,
    tone: Tone,
    pacer: Pacer,
    speaker: NpcId,
    listener: NpcId,
    shown: Option<Shown>,
    life: Life,
    alpha: f32,
    /// Ordine di priorità (0 = il più importante), per impilare.
    rank: usize,
    /// Ultima posizione della testa di chi parla (se sparisce resta lì) e x
    /// dell'interlocutore.
    head: Vec2,
    listener_x: Option<f32>,
    tail: Entity,
    stem: Entity,
    text: Entity,
}

/// Codina e gambo del fumetto (figli).
#[derive(Component)]
struct ChatPart;

/// Testo del fumetto (figlio).
#[derive(Component)]
struct ChatText;

/// Reazione sopra chi ascolta.
#[derive(Component)]
struct Reaction {
    npc: NpcId,
    elapsed: f32,
    head: Vec2,
}

/// Qualsiasi entità di questo modulo (per il reset).
#[derive(Component)]
struct SpeechEntity;

// --- Sistemi ----------------------------------------------------------------------------

fn reset_speech(
    mut commands: Commands,
    parts: Query<Entity, With<SpeechEntity>>,
    mut says: ResMut<ChatSays>,
) {
    for entity in &parts {
        commands.entity(entity).despawn();
    }
    *says = ChatSays::default();
}

fn speech_keys(keys: Res<ButtonInput<KeyCode>>, mut mode: ResMut<SpeechMode>) {
    if keys.just_pressed(KeyCode::KeyV) {
        *mode = mode.next();
    }
}

/// La camera del mondo (disgiunta dagli NPC).
type CameraOnly = (With<Camera2d>, Without<NpcSprite>);

/// Centro e rettangolo dell'inquadratura, in coordinate mondo.
fn view_of(camera: &Query<(&Transform, &Projection), CameraOnly>) -> Option<(Vec2, Rect)> {
    let (transform, projection) = camera.iter().next()?;
    let Projection::Orthographic(ortho) = projection else {
        return None;
    };
    let center = transform.translation.truncate();
    Some((
        center,
        Rect::from_corners(center + ortho.area.min, center + ortho.area.max),
    ))
}

/// Numero pseudo-casuale in `0..1` per una battuta (niente rng globale).
fn line_chance(conv: ConversationId, line: usize) -> f32 {
    let mut z = conv.0.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (line as u64);
    z = (z ^ (z >> 33)).wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    z = (z ^ (z >> 33)).wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    z ^= z >> 33;
    (z >> 40) as f32 / (1u64 << 24) as f32
}

/// Sceglie chi ha un fumetto, fa avanzare le battute, crea e toglie i fumetti.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_speech(
    mut commands: Commands,
    time: Res<Time<Real>>,
    sim: Res<Sim>,
    clock: Res<SimClock>,
    grace: Option<Res<DeliberationGrace>>,
    mode: Res<SpeechMode>,
    selected: Res<SelectedNpc>,
    busy: Res<DeliberationHeads>,
    art: Option<Res<SpeechArt>>,
    index: Res<NpcSpriteIndex>,
    sprites: Query<(&Transform, &NpcVisual), With<NpcSprite>>,
    camera: Query<(&Transform, &Projection), CameraOnly>,
    mut bubbles: Query<(Entity, &mut ChatBubble)>,
    mut texts: Query<&mut Text2d, With<ChatText>>,
    reactions: Query<&Reaction>,
) {
    let Some(art) = art else {
        return;
    };
    let real = time.elapsed_secs_f64();
    let world = &sim.world;
    let now = world.clock;
    let speed = grace.as_deref().map_or(clock.minutes_per_second, |g| {
        g.cap(clock.minutes_per_second)
    });
    let fast = !clock.paused && speed > FAST_SPEED;
    let view = view_of(&camera);

    // Fumetti ancora legati a una conversazione.
    let existing: HashMap<ConversationId, Entity> = bubbles
        .iter()
        .filter(|(_, b)| !matches!(b.life, Life::Ended { .. }))
        .map(|(e, b)| (b.conv, e))
        .collect();

    let mut convs: Vec<&Conversation> = Vec::new();
    let mut candidates: Vec<Candidate> = Vec::new();
    for conv in world.conversations() {
        let bubble = existing
            .get(&conv.id)
            .and_then(|&e| bubbles.get(e).ok())
            .map(|(_, b)| b);
        // Chi parla: quello già sullo schermo, o l'ultimo che ha parlato.
        let speaker = match bubble.filter(|b| b.shown.is_some()) {
            Some(b) => b.speaker,
            None => match conv.line_at(now) {
                Some(line) => line.speaker,
                None => continue,
            },
        };
        let Some(head) = head_of(&index, &sprites, speaker) else {
            continue;
        };
        let has_text = bubble.is_some_and(|b| matches!(b.shown, Some(Shown::Text(_))));
        let (on_screen, distance) = match view {
            Some((center, rect)) => (
                rect.inflate(VIEW_MARGIN).contains(head),
                head.distance(center) - if has_text { KEEP_BONUS } else { 0.0 },
            ),
            None => (true, 0.0),
        };
        candidates.push(Candidate {
            speaker,
            selected: selected.0.is_some_and(|s| conv.involves(s)),
            on_screen,
            distance,
        });
        convs.push(conv);
    }
    let slots = assign_slots(&candidates, *mode, &busy.0);
    // Priorità per impilare: testo prima dei puntini, poi come per i posti.
    let mut ranked: Vec<usize> = (0..convs.len())
        .filter(|&i| slots[i] != Slot::Hidden)
        .collect();
    ranked.sort_by(|&i, &j| {
        let (a, b) = (&candidates[i], &candidates[j]);
        (slots[i] == Slot::Mark)
            .cmp(&(slots[j] == Slot::Mark))
            .then(b.selected.cmp(&a.selected))
            .then(a.distance.total_cmp(&b.distance))
    });

    let reacting: HashSet<NpcId> = reactions.iter().map(|r| r.npc).collect();
    let mut shown_convs: HashSet<ConversationId> = HashSet::new();
    let mut talking: HashSet<NpcId> = HashSet::new();
    for (rank, &i) in ranked.iter().enumerate() {
        let conv = convs[i];
        let text_mode = slots[i] == Slot::Text && !fast;
        shown_convs.insert(conv.id);
        talking.extend([conv.a, conv.b]);
        let (react, speaker, listener) = match existing
            .get(&conv.id)
            .and_then(|&e| bubbles.get_mut(e).ok())
        {
            Some((_, mut bubble)) => {
                let update = refresh(&mut bubble, conv, now, real, text_mode, rank);
                if update.text_changed
                    && let (Some(Shown::Text(text)), Ok(mut t)) =
                        (&bubble.shown, texts.get_mut(bubble.text))
                {
                    t.0.clone_from(text);
                }
                (update.react, bubble.speaker, bubble.listener)
            }
            None => {
                let mut bubble = new_bubble(conv);
                let update = refresh(&mut bubble, conv, now, real, text_mode, rank);
                let who = (bubble.speaker, bubble.listener);
                spawn_bubble(&mut commands, &art, bubble);
                (update.react, who.0, who.1)
            }
        };
        if let Some(line) = react
            && line_chance(conv.id, line) < REACTION_CHANCE
            && !busy.0.contains(&listener)
            && !reacting.contains(&listener)
            && let Some(image) = reaction_image(&art, world, conv.tone, speaker, listener)
            && let Some(head) = head_of(&index, &sprites, listener)
        {
            commands.spawn((
                Name::new("Reazione"),
                SpeechEntity,
                Reaction {
                    npc: listener,
                    elapsed: 0.0,
                    head,
                },
                Sprite::from_image(image),
                Anchor::BOTTOM_CENTER,
                Transform::from_xyz(head.x, head.y + GAP, REACTION_Z),
            ));
        }
    }

    // Gli altri: nascosti, o con la conversazione finita.
    for (_, mut bubble) in &mut bubbles {
        if shown_convs.contains(&bubble.conv) {
            continue;
        }
        let hold = bubble.pacer.since
            + match bubble.shown {
                Some(Shown::Text(_)) => MIN_LINE_SECS,
                _ => MIN_MARK_SECS,
            };
        let alive = world.conversations().iter().any(|c| c.id == bubble.conv);
        let life = match bubble.life {
            Life::Active | Life::Hidden if alive => Life::Hidden,
            Life::Active | Life::Hidden => Life::Ended { hold },
            ended => ended,
        };
        // Chi ha cominciato un'altra conversazione non tiene il fumetto vecchio.
        let life = if talking.contains(&bubble.speaker) || talking.contains(&bubble.listener) {
            Life::Ended { hold: 0.0 }
        } else {
            life
        };
        if bubble.life != life {
            bubble.life = life;
        }
        if bubble.rank != usize::MAX {
            bubble.rank = usize::MAX;
        }
    }
}

/// Esito di [`refresh`].
struct Refresh {
    /// Il testo da mostrare è cambiato.
    text_changed: bool,
    /// Battuta nuova (in modalità testo): chi ascolta può reagire.
    react: Option<usize>,
}

/// Fa avanzare le battute di un fumetto mostrato.
fn refresh(
    bubble: &mut ChatBubble,
    conv: &Conversation,
    now: GameTime,
    real: f64,
    text_mode: bool,
    rank: usize,
) -> Refresh {
    bubble.life = Life::Active;
    bubble.rank = rank;
    let before = bubble.pacer.line;
    let mut update = Refresh {
        text_changed: false,
        react: None,
    };
    let Some((speaker, shown)) = pace(conv, now, real, text_mode, &mut bubble.pacer) else {
        return update;
    };
    let line_changed = bubble.pacer.line != before;
    bubble.speaker = speaker;
    bubble.listener = conv.other(speaker).unwrap_or(conv.b);
    if bubble.shown.as_ref() != Some(&shown) {
        if line_changed && bubble.shown.is_some() {
            // Battuta nuova: un piccolo "pop".
            bubble.alpha = bubble.alpha.min(0.6);
        }
        update.text_changed = matches!(shown, Shown::Text(_));
        bubble.shown = Some(shown);
    }
    if line_changed && text_mode {
        update.react = bubble.pacer.line;
    }
    update
}

/// Reazione di chi ascolta secondo il tono: cuoricino tra partner, nota se è
/// amichevole, "!" se è tesa, niente se è neutra.
fn reaction_image(
    art: &SpeechArt,
    world: &World,
    tone: Tone,
    speaker: NpcId,
    listener: NpcId,
) -> Option<Handle<Image>> {
    match tone {
        Tone::Friendly => {
            let partners = world
                .npc(speaker)
                .is_some_and(|n| n.partner() == Some(listener));
            Some(if partners {
                art.heart.clone()
            } else {
                art.note.clone()
            })
        }
        Tone::Neutral => None,
        Tone::Tense => Some(art.alarm.clone()),
    }
}

/// Stato iniziale del fumetto di `conv` (le entità dei figli le mette `spawn_bubble`).
fn new_bubble(conv: &Conversation) -> ChatBubble {
    ChatBubble {
        conv: conv.id,
        tone: conv.tone,
        pacer: Pacer::default(),
        speaker: conv.a,
        listener: conv.b,
        shown: None,
        life: Life::Active,
        alpha: 0.0,
        rank: 0,
        head: Vec2::ZERO,
        listener_x: None,
        tail: Entity::PLACEHOLDER,
        stem: Entity::PLACEHOLDER,
        text: Entity::PLACEHOLDER,
    }
}

/// Crea il fumetto: fondo (con `bubble`), codina, gambo e testo. La
/// posizione vera la mette `place_speech`.
fn spawn_bubble(commands: &mut Commands, art: &SpeechArt, mut bubble: ChatBubble) {
    let tone = bubble.tone.index();
    let (border, _) = TONE_COLORS[tone];
    let text_color = if bubble.tone == Tone::Tense {
        TENSE_TEXT
    } else {
        TEXT_COLOR
    };
    bubble.tail = commands
        .spawn((
            ChatPart,
            Sprite::from_image(art.tail[tone].clone()),
            Anchor::TOP_LEFT,
            Transform::from_xyz(1.0, 1.0, 0.01),
        ))
        .id();
    bubble.stem = commands
        .spawn((
            ChatPart,
            Sprite::from_color(Color::srgb_u8(border[0], border[1], border[2]), Vec2::ONE),
            Anchor::TOP_LEFT,
            Transform::from_xyz(1.0, -2.0, 0.01),
            Visibility::Hidden,
        ))
        .id();
    let text = match &bubble.shown {
        Some(Shown::Text(text)) => text.clone(),
        _ => String::new(),
    };
    bubble.text = commands
        .spawn((
            ChatText,
            Text2d::new(text),
            TextFont {
                font_size: FontSize::Px(TEXT_SIZE),
                ..default()
            },
            TextLayout::new(Justify::Center, LineBreak::NoWrap),
            TextColor(text_color),
            Anchor::CENTER,
            Transform::from_xyz(0.0, 0.0, 0.02).with_scale(Vec3::splat(TEXT_SCALE)),
        ))
        .id();
    let children = [bubble.tail, bubble.stem, bubble.text];
    commands
        .spawn((
            Name::new("Dialogo"),
            SpeechEntity,
            bubble,
            Sprite {
                image: art.body[tone].clone(),
                custom_size: Some(Vec2::splat(9.0)),
                image_mode: slicer(),
                color: Color::WHITE.with_alpha(0.0),
                ..default()
            },
            Anchor::BOTTOM_LEFT,
            Transform::from_xyz(0.0, -1000.0, CHAT_Z),
            Visibility::Hidden,
        ))
        .add_children(&children);
}

/// Il fondo di un fumetto (disgiunto da NPC, camera, codine e testi).
type BubbleOnly = (
    Without<NpcSprite>,
    Without<Camera2d>,
    Without<ChatPart>,
    Without<ChatText>,
);
type PartOnly = (
    With<ChatPart>,
    Without<ChatBubble>,
    Without<NpcSprite>,
    Without<Camera2d>,
);
type TextOnly = (
    With<ChatText>,
    Without<ChatPart>,
    Without<ChatBubble>,
    Without<NpcSprite>,
    Without<Camera2d>,
);

/// Dimensioni del fondo per ciò che mostra (misurate sul testo impaginato, se c'è).
fn bubble_size(shown: Option<&Shown>, layout: Option<&TextLayoutInfo>) -> Vec2 {
    match shown {
        Some(Shown::Text(text)) => {
            let measured = layout
                .filter(|info| info.size.x > 0.0)
                .map(|info| info.size / info.scale_factor.max(1e-3) * TEXT_SCALE);
            let content = measured.unwrap_or_else(|| {
                let lines = text.lines().count().max(1);
                let chars = text.lines().map(|l| l.chars().count()).max().unwrap_or(0);
                Vec2::new(chars as f32 * CHAR_WIDTH, lines as f32 * LINE_HEIGHT)
            });
            let size = content + 2.0 * Vec2::new(PAD_X, PAD_Y);
            Vec2::new(size.x.round().max(9.0), size.y.round().max(9.0))
        }
        Some(Shown::Dots) => Vec2::new(11.0, 7.0),
        Some(Shown::Bang) => Vec2::new(7.0, 7.0),
        None => Vec2::splat(9.0),
    }
}

/// Posizione, dimensioni, impilamento e dissolvenza dei fumetti.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn place_speech(
    mut commands: Commands,
    time: Res<Time<Real>>,
    art: Option<Res<SpeechArt>>,
    index: Res<NpcSpriteIndex>,
    sprites: Query<(&Transform, &NpcVisual), With<NpcSprite>>,
    camera: Query<(&Transform, &Projection), CameraOnly>,
    mut bubbles: Query<
        (
            Entity,
            &mut ChatBubble,
            &mut Transform,
            &mut Sprite,
            &mut Visibility,
        ),
        BubbleOnly,
    >,
    mut parts: Query<(&mut Transform, &mut Sprite, &mut Visibility, &mut Anchor), PartOnly>,
    mut texts: Query<
        (
            &TextLayoutInfo,
            &mut Transform,
            &mut TextColor,
            &mut Visibility,
        ),
        TextOnly,
    >,
) {
    let Some(art) = art else {
        return;
    };
    let dt = time.delta_secs();
    let real = time.elapsed_secs_f64();
    let view = view_of(&camera).map(|(_, rect)| rect);

    // Prima passata: testa, dissolvenza, dimensioni e lato.
    let mut placed: Vec<(usize, Entity, Rect, bool)> = Vec::new();
    for (entity, mut bubble, _, _, mut visibility) in &mut bubbles {
        let head = head_of(&index, &sprites, bubble.speaker);
        if let Some(head) = head {
            bubble.head = head;
        }
        bubble.listener_x = head_of(&index, &sprites, bubble.listener).map(|h| h.x);
        let target = match bubble.life {
            _ if bubble.head == Vec2::ZERO || bubble.shown.is_none() => 0.0,
            Life::Active => 1.0,
            Life::Hidden => 0.0,
            Life::Ended { hold } => {
                if real < hold {
                    1.0
                } else {
                    0.0
                }
            }
        };
        let step = dt / FADE_SECS;
        bubble.alpha = if target > bubble.alpha {
            (bubble.alpha + step).min(target)
        } else {
            (bubble.alpha - step).max(target)
        };
        if bubble.alpha <= 0.0 {
            if bubble.life != Life::Active {
                commands.entity(entity).despawn();
            } else {
                visibility.set_if_neq(Visibility::Hidden);
            }
            continue;
        }
        let layout = texts.get(bubble.text).ok().map(|(info, ..)| info);
        let size = bubble_size(bubble.shown.as_ref(), layout);
        let head = bubble.head;
        // Il fumetto si allarga dalla parte opposta all'interlocutore...
        let mut left = bubble.listener_x.is_some_and(|x| x > head.x + 0.5);
        let min_x = |left: bool| {
            if left {
                head.x + 2.0 - size.x
            } else {
                head.x - 2.0
            }
        };
        // ...ma resta nell'inquadratura se può.
        if let Some(view) = view {
            if !left && min_x(false) + size.x > view.max.x && min_x(true) >= view.min.x {
                left = true;
            } else if left && min_x(true) < view.min.x && min_x(false) + size.x <= view.max.x {
                left = false;
            }
        }
        let min = Vec2::new(min_x(left), head.y + GAP);
        placed.push((
            bubble.rank,
            entity,
            Rect::from_corners(min, min + size),
            left,
        ));
    }
    placed.sort_by_key(|&(rank, entity, ..)| (rank, entity));
    let rects: Vec<Rect> = placed.iter().map(|p| p.2).collect();
    let offsets = stack(&rects);

    // Seconda passata: applica.
    for (order, ((_, entity, rect, left), rise)) in placed.into_iter().zip(offsets).enumerate() {
        let Ok((_, bubble, mut transform, mut sprite, mut visibility)) = bubbles.get_mut(entity)
        else {
            continue;
        };
        let size = rect.size();
        let tone = bubble.tone.index();
        let alpha = bubble.alpha;
        transform.translation = Vec3::new(
            rect.min.x,
            rect.min.y + rise,
            CHAT_Z - order.min(30) as f32 * 0.03,
        );
        let (image, sliced) = match bubble.shown {
            Some(Shown::Text(_)) | None => (&art.body[tone], true),
            Some(Shown::Dots) => (&art.dots[tone], false),
            Some(Shown::Bang) => (&art.bang, false),
        };
        if sprite.image != *image {
            sprite.image = image.clone();
            sprite.image_mode = if sliced {
                slicer()
            } else {
                SpriteImageMode::Auto
            };
        }
        let custom = sliced.then_some(size);
        if sprite.custom_size != custom {
            sprite.custom_size = custom;
        }
        sprite.color.set_alpha(alpha);
        visibility.set_if_neq(Visibility::Inherited);

        if let Ok((mut transform, mut sprite, _, mut anchor)) = parts.get_mut(bubble.tail) {
            transform.translation.x = if left { size.x - 1.0 } else { 1.0 };
            anchor.set_if_neq(if left {
                Anchor::TOP_RIGHT
            } else {
                Anchor::TOP_LEFT
            });
            sprite.flip_x = left;
            sprite.color.set_alpha(alpha);
        }
        if let Ok((mut transform, mut sprite, mut visibility, _)) = parts.get_mut(bubble.stem) {
            if rise > 0.5 {
                transform.translation.x = if left { size.x - 2.0 } else { 1.0 };
                sprite.custom_size = Some(Vec2::new(1.0, rise));
                sprite.color.set_alpha(alpha);
                visibility.set_if_neq(Visibility::Inherited);
            } else {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
        if let Ok((_, mut transform, mut color, mut visibility)) = texts.get_mut(bubble.text) {
            if matches!(bubble.shown, Some(Shown::Text(_))) {
                transform.translation.x = size.x / 2.0;
                transform.translation.y = size.y / 2.0;
                color.0.set_alpha(alpha);
                visibility.set_if_neq(Visibility::Inherited);
            } else {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
}

/// Le reazioni seguono la testa di chi ascolta, salgono e svaniscono.
fn move_reactions(
    mut commands: Commands,
    time: Res<Time<Real>>,
    index: Res<NpcSpriteIndex>,
    sprites: Query<(&Transform, &NpcVisual), With<NpcSprite>>,
    mut reactions: Query<(Entity, &mut Reaction, &mut Transform, &mut Sprite), Without<NpcSprite>>,
) {
    let dt = time.delta_secs();
    for (entity, mut reaction, mut transform, mut sprite) in &mut reactions {
        reaction.elapsed += dt;
        let t = reaction.elapsed / REACTION_SECS;
        if t >= 1.0 {
            commands.entity(entity).despawn();
            continue;
        }
        if let Some(head) = head_of(&index, &sprites, reaction.npc) {
            reaction.head = head;
        }
        transform.translation.x = reaction.head.x;
        transform.translation.y = reaction.head.y + GAP + REACTION_RISE * t;
        sprite.color.set_alpha(((1.0 - t) / 0.4).min(1.0));
    }
}

// --- Ispettore ----------------------------------------------------------------------------

const TONE_FRIENDLY: Color32 = Color32::from_rgb(110, 190, 110);
const TONE_NEUTRAL: Color32 = Color32::from_gray(170);
const TONE_TENSE: Color32 = Color32::from_rgb(230, 80, 80);
/// La battuta che si sta dicendo adesso.
const CURRENT_LINE: Color32 = Color32::from_rgb(245, 220, 140);
/// Conversazioni recenti mostrate al massimo.
const RECENT_TALKS: usize = 5;

// --- Saluti al giocatore -------------------------------------------------------------

/// Fumetto di un saluto al giocatore (`World::greetings`).
#[derive(Component)]
struct GreetBubble {
    npc: NpcId,
    since: GameTime,
    /// Istante reale in cui è comparso.
    born: f64,
    /// Ultima posizione della testa (se lo sprite sparisce resta lì).
    head: Vec2,
    text: Entity,
}

/// Testo (figlio) di un saluto.
#[derive(Component)]
struct GreetText;

/// "!" sopra chi ha qualcosa da dire al giocatore.
#[derive(Component)]
struct TalkMark {
    npc: NpcId,
}

/// Il fondo di un saluto (disgiunto da NPC, testi e "!").
type GreetOnly = (
    Without<NpcSprite>,
    Without<GreetText>,
    Without<TalkMark>,
    Without<ChatBubble>,
    Without<ChatPart>,
    Without<ChatText>,
    Without<SayBubble>,
    Without<SayText>,
);

/// Crea, sposta e toglie i fumetti dei saluti al giocatore.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_greetings(
    mut commands: Commands,
    time: Res<Time<Real>>,
    sim: Res<Sim>,
    clock: Res<SimClock>,
    mode: Res<SpeechMode>,
    art: Option<Res<SpeechArt>>,
    index: Res<NpcSpriteIndex>,
    sprites: Query<(&Transform, &NpcVisual), With<NpcSprite>>,
    mut bubbles: Query<(Entity, &mut GreetBubble, &mut Transform, &mut Sprite), GreetOnly>,
    texts: Query<&TextLayoutInfo, With<GreetText>>,
    chat: Option<Res<ChatWindow>>,
) {
    let Some(art) = art else {
        return;
    };
    let real = time.elapsed_secs_f64();
    let world = &sim.world;
    let fast = !clock.paused && clock.minutes_per_second > FAST_SPEED;
    // Chi parla col giocatore ha i fumetti della chat.
    let chatting = chat.and_then(|c| c.npc);
    let mut shown: HashSet<(NpcId, GameTime)> = HashSet::new();
    for (entity, mut bubble, mut transform, mut sprite) in &mut bubbles {
        let alive = world
            .greeting_of(bubble.npc)
            .is_some_and(|g| g.since == bubble.since);
        let head = head_of(&index, &sprites, bubble.npc);
        if (!alive && real - bubble.born >= GREET_SECS)
            || *mode == SpeechMode::Off
            || chatting == Some(bubble.npc)
        {
            commands.entity(entity).despawn();
            continue;
        }
        shown.insert((bubble.npc, bubble.since));
        if let Some(head) = head {
            bubble.head = head;
        }
        let text = world
            .greeting_of(bubble.npc)
            .map(|g| wrap_text(&g.text, LINE_CHARS, MAX_LINES).join("\n"));
        let layout = texts.get(bubble.text).ok();
        let size = bubble_size(text.map(Shown::Text).as_ref(), layout);
        if sprite.custom_size != Some(size) {
            sprite.custom_size = Some(size);
        }
        let age = (real - bubble.born) as f32;
        let alpha = (age / FADE_SECS).min(1.0);
        sprite.color = Color::WHITE.with_alpha(alpha);
        transform.translation = Vec3::new(
            bubble.head.x.round(),
            (bubble.head.y + GAP + 2.0).round(),
            GREET_Z,
        );
    }
    if fast || *mode == SpeechMode::Off {
        return;
    }
    for greeting in world.greetings() {
        if shown.contains(&(greeting.npc, greeting.since)) || chatting == Some(greeting.npc) {
            continue;
        }
        let Some(head) = head_of(&index, &sprites, greeting.npc) else {
            continue;
        };
        let text = wrap_text(&greeting.text, LINE_CHARS, MAX_LINES).join("\n");
        let size = bubble_size(Some(&Shown::Text(text.clone())), None);
        let text_entity = commands
            .spawn((
                GreetText,
                Text2d::new(text),
                TextFont {
                    font_size: FontSize::Px(TEXT_SIZE),
                    ..default()
                },
                TextLayout::new(Justify::Center, LineBreak::NoWrap),
                TextColor(TEXT_COLOR),
                Anchor::CENTER,
                Transform::from_xyz(0.0, size.y / 2.0, 0.02).with_scale(Vec3::splat(TEXT_SCALE)),
            ))
            .id();
        let tail = commands
            .spawn((
                Sprite::from_image(art.tail[Tone::Friendly.index()].clone()),
                Anchor::TOP_LEFT,
                Transform::from_xyz(-2.0, 1.0, 0.01),
            ))
            .id();
        commands
            .spawn((
                Name::new("Saluto"),
                SpeechEntity,
                GreetBubble {
                    npc: greeting.npc,
                    since: greeting.since,
                    born: real,
                    head,
                    text: text_entity,
                },
                Sprite {
                    image: art.body[Tone::Friendly.index()].clone(),
                    custom_size: Some(size),
                    image_mode: slicer(),
                    color: Color::WHITE.with_alpha(0.0),
                    ..default()
                },
                Anchor::BOTTOM_CENTER,
                Transform::from_xyz(head.x, head.y + GAP + 2.0, GREET_Z),
            ))
            .add_children(&[tail, text_entity]);
    }
}

/// Tiene il testo al centro del suo fumetto (le dimensioni cambiano quando
/// il testo è impaginato).
fn center_greet_texts(
    bubbles: Query<(&GreetBubble, &Sprite)>,
    mut texts: Query<&mut Transform, With<GreetText>>,
) {
    for (bubble, sprite) in &bubbles {
        let Some(size) = sprite.custom_size else {
            continue;
        };
        if let Ok(mut t) = texts.get_mut(bubble.text)
            && (t.translation.y - size.y / 2.0).abs() > 0.01
        {
            t.translation.y = size.y / 2.0;
        }
    }
}

/// Un "!" giallo sopra chi ha qualcosa da dire al giocatore (e non lo sta
/// già salutando).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_talk_marks(
    mut commands: Commands,
    time: Res<Time<Real>>,
    sim: Res<Sim>,
    mode: Res<SpeechMode>,
    art: Option<Res<SpeechArt>>,
    index: Res<NpcSpriteIndex>,
    sprites: Query<(&Transform, &NpcVisual), With<NpcSprite>>,
    mut marks: Query<
        (Entity, &TalkMark, &mut Transform),
        (Without<NpcSprite>, Without<GreetBubble>, Without<SayBubble>),
    >,
    chat: Option<Res<ChatWindow>>,
) {
    let Some(art) = art else {
        return;
    };
    let world = &sim.world;
    let bob = (time.elapsed_secs() * MARK_BOB_SPEED).sin() * MARK_BOB;
    let chatting = chat.and_then(|c| c.npc);
    let wanted = |id: NpcId| {
        *mode != SpeechMode::Off
            && world.wants_to_talk(id)
            && world.greeting_of(id).is_none()
            && chatting != Some(id)
    };
    let mut has_mark: HashSet<NpcId> = HashSet::new();
    for (entity, mark, mut transform) in &mut marks {
        match head_of(&index, &sprites, mark.npc).filter(|_| wanted(mark.npc)) {
            Some(head) => {
                has_mark.insert(mark.npc);
                transform.translation =
                    Vec3::new(head.x.round(), (head.y + GAP + bob).round(), REACTION_Z);
            }
            None => commands.entity(entity).despawn(),
        }
    }
    for npc in &world.npcs {
        if has_mark.contains(&npc.id) || !wanted(npc.id) {
            continue;
        }
        if let Some(head) = head_of(&index, &sprites, npc.id) {
            commands.spawn((
                Name::new("Ha qualcosa da dirti"),
                SpeechEntity,
                TalkMark { npc: npc.id },
                Sprite::from_image(art.talk.clone()),
                Anchor::BOTTOM_CENTER,
                Transform::from_xyz(head.x, head.y + GAP, REACTION_Z),
            ));
        }
    }
}

// --- Battute della chat del giocatore ------------------------------------------------------

/// Chi dice una battuta della chat.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Sayer {
    Player,
    Npc(NpcId),
}

/// Una battuta della chat da mostrare in un fumetto.
#[derive(Clone, Debug)]
pub(crate) struct Say {
    pub(crate) who: Sayer,
    pub(crate) text: String,
    serial: u64,
}

/// Battute della chat (le aggiunge `chat.rs`): una per chi parla, l'ultima.
/// Restano finché il loro fumetto non svanisce.
#[derive(Resource, Default)]
pub(crate) struct ChatSays {
    says: Vec<Say>,
    next: u64,
}

impl ChatSays {
    /// `who` dice `text` (al posto della sua battuta precedente).
    pub(crate) fn say(&mut self, who: Sayer, text: String) {
        self.says.retain(|s| s.who != who);
        self.says.push(Say {
            who,
            text,
            serial: self.next,
        });
        self.next += 1;
    }

    /// Le battute ancora da mostrare o in mostra.
    pub(crate) fn pending(&self) -> impl Iterator<Item = &Say> {
        self.says.iter()
    }
}

/// Caratteri per riga e righe al massimo dei fumetti della chat (più lunghi
/// di quelli tra NPC: si legge con il tempo fermo).
const SAY_CHARS: usize = 26;
const SAY_LINES: usize = 4;
/// Il fumetto sopra il giocatore: quanto sopra il centro del suo corpo.
const PLAYER_HEAD: f32 = 13.0;

/// Secondi reali per cui resta una battuta della chat.
pub(crate) fn say_secs(text: &str) -> f64 {
    (2.5 + 0.06 * text.chars().count() as f64).clamp(GREET_SECS, 9.0)
}

/// Fumetto di una battuta della chat.
#[derive(Component)]
struct SayBubble {
    who: Sayer,
    serial: u64,
    born: f64,
    life: f64,
    head: Vec2,
    /// Da che parte si allarga: -1 a sinistra, 1 a destra (lontano
    /// dall'interlocutore, così i due fumetti non si coprono), 0 al centro.
    side: i8,
    text: Entity,
    tail: Entity,
}

/// Da che parte allargare il fumetto di chi sta in `head`, se l'altro è in `other`.
fn side_away(head: Vec2, other: Option<Vec2>) -> i8 {
    match other {
        Some(o) if o.x > head.x => -1,
        Some(_) => 1,
        None => 0,
    }
}

/// Ancora del fondo, x del punto di aggancio rispetto alla testa, x della
/// codina e del testo (rispetto al punto di aggancio) per un lato.
fn side_layout(side: i8, width: f32) -> (Anchor, f32, f32, f32) {
    match side {
        -1 => (Anchor::BOTTOM_RIGHT, 4.0, -6.0, -width / 2.0),
        1 => (Anchor::BOTTOM_LEFT, -4.0, 2.0, width / 2.0),
        _ => (Anchor::BOTTOM_CENTER, 0.0, -2.0, 0.0),
    }
}

/// Testo (figlio) di una battuta della chat.
#[derive(Component)]
struct SayText;

type SayOnly = (
    Without<NpcSprite>,
    Without<Player>,
    Without<GreetBubble>,
    Without<GreetText>,
    Without<TalkMark>,
    Without<ChatBubble>,
    Without<ChatPart>,
    Without<ChatText>,
    Without<SayText>,
    Without<SayTail>,
);

/// Crea, sposta e toglie i fumetti della chat del giocatore.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_chat_says(
    mut commands: Commands,
    time: Res<Time<Real>>,
    mut says: ResMut<ChatSays>,
    art: Option<Res<SpeechArt>>,
    index: Res<NpcSpriteIndex>,
    sprites: Query<(&Transform, &NpcVisual), With<NpcSprite>>,
    player: Query<&Transform, (With<Player>, Without<NpcSprite>, Without<SayBubble>)>,
    mut bubbles: Query<
        (
            Entity,
            &mut SayBubble,
            &mut Transform,
            &mut Sprite,
            &mut Anchor,
        ),
        SayOnly,
    >,
    texts: Query<&TextLayoutInfo, With<SayText>>,
    chat: Option<Res<ChatWindow>>,
) {
    let Some(art) = art else {
        return;
    };
    let real = time.elapsed_secs_f64();
    let head_of_sayer = |who: Sayer| match who {
        Sayer::Npc(id) => head_of(&index, &sprites, id),
        Sayer::Player => player
            .iter()
            .next()
            .map(|t| t.translation.truncate() + Vec2::Y * PLAYER_HEAD),
    };
    // L'interlocutore: il giocatore per l'NPC, l'NPC della chat per il giocatore.
    let chatting = chat.and_then(|c| c.npc);
    let other_of = |who: Sayer| match who {
        Sayer::Npc(_) => head_of_sayer(Sayer::Player),
        Sayer::Player => chatting.and_then(|id| head_of_sayer(Sayer::Npc(id))),
    };
    let mut shown: HashSet<u64> = HashSet::new();
    let mut expired: Vec<u64> = Vec::new();
    for (entity, mut bubble, mut transform, mut sprite, mut anchor) in &mut bubbles {
        let current = says.pending().any(|s| s.serial == bubble.serial);
        let age = real - bubble.born;
        if !current || age >= bubble.life {
            commands.entity(entity).despawn();
            if current {
                expired.push(bubble.serial);
            }
            continue;
        }
        shown.insert(bubble.serial);
        let head = head_of_sayer(bubble.who).unwrap_or(bubble.head);
        bubble.head = head;
        bubble.side = side_away(head, other_of(bubble.who));
        let layout = texts.get(bubble.text).ok();
        let text = says
            .pending()
            .find(|s| s.serial == bubble.serial)
            .map(|s| wrap_text(&s.text, SAY_CHARS, SAY_LINES).join("\n"));
        let size = bubble_size(text.map(Shown::Text).as_ref(), layout);
        if sprite.custom_size != Some(size) {
            sprite.custom_size = Some(size);
        }
        let fade_in = (age as f32 / FADE_SECS).min(1.0);
        let fade_out = ((bubble.life - age) as f32 / FADE_SECS).min(1.0);
        sprite.color = Color::WHITE.with_alpha(fade_in.min(fade_out));
        let (wanted, dx, _, _) = side_layout(bubble.side, size.x);
        if *anchor != wanted {
            *anchor = wanted;
        }
        transform.translation =
            Vec3::new((head.x + dx).round(), (head.y + GAP + 2.0).round(), GREET_Z);
    }
    if !expired.is_empty() {
        says.says.retain(|s| !expired.contains(&s.serial));
    }
    for say in says.pending() {
        if shown.contains(&say.serial) {
            continue;
        }
        let Some(head) = head_of_sayer(say.who) else {
            continue;
        };
        let tone = match say.who {
            Sayer::Player => Tone::Neutral,
            Sayer::Npc(_) => Tone::Friendly,
        };
        let text = wrap_text(&say.text, SAY_CHARS, SAY_LINES).join("\n");
        let size = bubble_size(Some(&Shown::Text(text.clone())), None);
        let side = side_away(head, other_of(say.who));
        let (bubble_anchor, dx, tail_x, text_x) = side_layout(side, size.x);
        let text_entity = commands
            .spawn((
                SayText,
                Text2d::new(text),
                TextFont {
                    font_size: FontSize::Px(TEXT_SIZE),
                    ..default()
                },
                TextLayout::new(Justify::Center, LineBreak::NoWrap),
                TextColor(TEXT_COLOR),
                Anchor::CENTER,
                Transform::from_xyz(text_x, size.y / 2.0, 0.02).with_scale(Vec3::splat(TEXT_SCALE)),
            ))
            .id();
        let tail = commands
            .spawn((
                SayTail,
                Sprite::from_image(art.tail[tone.index()].clone()),
                Anchor::TOP_LEFT,
                Transform::from_xyz(tail_x, 1.0, 0.01),
            ))
            .id();
        commands
            .spawn((
                Name::new("Battuta della chat"),
                SpeechEntity,
                SayBubble {
                    who: say.who,
                    serial: say.serial,
                    born: real,
                    life: say_secs(&say.text),
                    head,
                    side,
                    text: text_entity,
                    tail,
                },
                Sprite {
                    image: art.body[tone.index()].clone(),
                    custom_size: Some(size),
                    image_mode: slicer(),
                    color: Color::WHITE.with_alpha(0.0),
                    ..default()
                },
                bubble_anchor,
                Transform::from_xyz(head.x + dx, head.y + GAP + 2.0, GREET_Z),
            ))
            .add_children(&[tail, text_entity]);
    }
}

/// Codina (figlia) di una battuta della chat.
#[derive(Component)]
struct SayTail;

/// Tiene il testo al centro del suo fumetto e la codina sopra la testa
/// (dimensioni e lato cambiano quando il testo è impaginato).
#[allow(clippy::type_complexity)]
fn center_say_texts(
    bubbles: Query<(&SayBubble, &Sprite)>,
    mut parts: ParamSet<(
        Query<&mut Transform, With<SayText>>,
        Query<&mut Transform, With<SayTail>>,
    )>,
) {
    for (bubble, sprite) in &bubbles {
        let Some(size) = sprite.custom_size else {
            continue;
        };
        let (_, _, tail_x, text_x) = side_layout(bubble.side, size.x);
        let wanted = Vec2::new(text_x, size.y / 2.0);
        if let Ok(mut t) = parts.p0().get_mut(bubble.text)
            && t.translation.truncate().distance(wanted) > 0.01
        {
            t.translation.x = wanted.x;
            t.translation.y = wanted.y;
        }
        if let Ok(mut t) = parts.p1().get_mut(bubble.tail)
            && (t.translation.x - tail_x).abs() > 0.01
        {
            t.translation.x = tail_x;
        }
    }
}

fn tone_color(tone: Tone) -> Color32 {
    match tone {
        Tone::Friendly => TONE_FRIENDLY,
        Tone::Neutral => TONE_NEUTRAL,
        Tone::Tense => TONE_TENSE,
    }
}

/// Nome di un NPC come link; vero se è stato cliccato.
fn npc_link(ui: &mut egui::Ui, world: &World, id: NpcId) -> bool {
    match world.npc(id) {
        Some(npc) => ui
            .link(&npc.name)
            .on_hover_text("Click: seleziona")
            .clicked(),
        None => {
            ui.weak(id.to_string());
            false
        }
    }
}

/// Nome proprio (la prima parola del nome) di chi ha detto una battuta.
fn first_name(world: &World, id: NpcId) -> String {
    world
        .npc(id)
        .and_then(|n| n.name.split_whitespace().next().map(str::to_string))
        .unwrap_or_else(|| id.to_string())
}

/// "08:30" se è oggi, altrimenti "Giorno 3 08:30".
fn time_label(time: GameTime, now: GameTime) -> String {
    if time.day() == now.day() {
        format!("{:02}:{:02}", time.hour(), time.minute())
    } else {
        time.to_string()
    }
}

/// Battute dette fino a `now`, una per riga ("Marta: Ciao!").
fn transcript(world: &World, conv: &Conversation, now: GameTime) -> String {
    conv.lines
        .iter()
        .take_while(|l| l.at <= now)
        .map(|l| format!("{}: {}", first_name(world, l.speaker), l.text))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Nell'ispettore: la conversazione in corso (con chi, di cosa, le battute
/// dette finora) e le ultime concluse. Restituisce l'NPC cliccato.
pub(crate) fn conversation_section(ui: &mut egui::Ui, world: &World, npc: &Npc) -> Option<NpcId> {
    let now = world.clock;
    let mut clicked = None;
    if let Some(conv) = world.conversation_of(npc.id) {
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label("Sta parlando con");
            if let Some(other) = conv.other(npc.id)
                && npc_link(ui, world, other)
            {
                clicked = Some(other);
            }
            ui.label(conv.topic.about_phrase());
            if let Some(about) = conv.about {
                ui.label("su");
                if npc_link(ui, world, about) {
                    clicked = Some(about);
                }
            }
            ui.colored_label(
                tone_color(conv.tone),
                format!("(tono {})", conv.tone.name()),
            );
        });
        // Le battute dette finora; l'ultima è quella che si sta dicendo.
        let spoken = conv.lines.iter().take_while(|l| l.at <= now).count();
        for (i, line) in conv.lines[..spoken].iter().enumerate() {
            let text = format!("{}: {}", first_name(world, line.speaker), line.text);
            if i + 1 == spoken {
                ui.label(RichText::new(text).strong().color(CURRENT_LINE));
            } else {
                ui.label(RichText::new(text).weak());
            }
        }
        if spoken == 0 {
            ui.weak("…");
        }
    }
    let recent: Vec<&Conversation> = world
        .recent_conversations()
        .iter()
        .rev()
        .filter(|c| c.involves(npc.id))
        .take(RECENT_TALKS)
        .collect();
    if !recent.is_empty() {
        egui::CollapsingHeader::new(format!("Conversazioni recenti ({})", recent.len()))
            .id_salt("inspector_recent_talks")
            .default_open(false)
            .show(ui, |ui| {
                for conv in recent {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.weak(time_label(conv.since, now));
                        if let Some(other) = conv.other(npc.id)
                            && npc_link(ui, world, other)
                        {
                            clicked = Some(other);
                        }
                        ui.label(format!("· {}", conv.topic.name()))
                            .on_hover_text(transcript(world, conv, conv.until));
                        ui.colored_label(tone_color(conv.tone), conv.tone.name());
                    });
                }
            });
    }
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim::{CarriageId, Topic};

    fn conv(tone: Tone, times: &[u64]) -> Conversation {
        Conversation {
            id: ConversationId(7),
            a: NpcId(1),
            b: NpcId(2),
            carriage: CarriageId(0),
            since: GameTime(100),
            until: GameTime(times.last().copied().unwrap_or(100) + 5),
            topic: Topic::SmallTalk,
            about: None,
            tone,
            lines: times
                .iter()
                .enumerate()
                .map(|(i, &at)| Line {
                    speaker: NpcId(1 + (i as u32 % 2)),
                    at: GameTime(at),
                    text: format!("Battuta numero {i}"),
                })
                .collect(),
            news: None,
        }
    }

    #[test]
    fn wrapping_keeps_words_and_two_lines() {
        assert_eq!(wrap_text("Ciao!", 22, 2), ["Ciao!"]);
        assert!(wrap_text("   ", 22, 2).is_empty());
        let lines = wrap_text("Hai visto che bella giornata oggi?", 22, 2);
        assert_eq!(lines, ["Hai visto che bella", "giornata oggi?"]);
        // Righe bilanciate (non "Un giorno ti offro un / tè.").
        let lines = wrap_text("Un giorno ti offro un tè.", 22, 2);
        assert_eq!(lines, ["Un giorno ti", "offro un tè."]);
        // Troppo lungo: due righe, l'ultima finisce con "…", parole intere.
        let text = "Hai sentito della carenza di razioni in mensa stamattina?";
        let lines = wrap_text(text, 22, 2);
        assert_eq!(lines.len(), 2);
        assert!(lines.last().unwrap().ends_with('…'));
        let words: Vec<&str> = text.split_whitespace().collect();
        for line in &lines {
            assert!(line.chars().count() <= 22, "{line}");
            for word in line.trim_end_matches('…').split_whitespace() {
                assert!(words.contains(&word), "{word}");
            }
        }
        // Parola più lunga di una riga: troncata con "…".
        let lines = wrap_text("Precipitevolissimevolmente, davvero", 22, 2);
        assert_eq!(lines, ["Precipitevolissimevol…", "davvero"]);
        assert!(lines.iter().all(|l| l.chars().count() <= 22));
    }

    #[test]
    fn wrapping_never_exceeds_the_limits() {
        let texts = [
            "a b c d e f g h i j k l m n o p q r s t u v w x y z a b c d e f g h i j k l m n",
            "Una frase qualunque ma piuttosto lunga da mandare a capo più volte di fila",
            "Supercalifragilistichespiralidoso supercalifragilistichespiralidoso",
            "Sì.",
        ];
        for text in texts {
            for width in [8, 16, 22] {
                let lines = wrap_text(text, width, MAX_LINES);
                assert!(!lines.is_empty() && lines.len() <= MAX_LINES, "{text}");
                assert!(
                    lines.iter().all(|l| l.chars().count() <= width),
                    "{lines:?}"
                );
            }
        }
    }

    /// Simula `secs` secondi reali a 60 fps con il tempo di gioco a `speed`
    /// minuti al secondo; restituisce (istante, testo) a ogni cambio.
    fn run(conv: &Conversation, speed: f64, secs: f64, text: bool) -> Vec<(f64, Shown)> {
        let mut pacer = Pacer::default();
        let mut log: Vec<(f64, Shown)> = Vec::new();
        let frames = (secs * 60.0) as u64;
        for frame in 0..=frames {
            let real = frame as f64 / 60.0;
            let now = GameTime(conv.since.0 + (real * speed) as u64);
            if let Some((_, shown)) = pace(conv, now, real, text, &mut pacer)
                && log.last().map(|(_, s)| s) != Some(&shown)
            {
                log.push((real, shown));
            }
        }
        log
    }

    #[test]
    fn slow_conversations_show_every_line_in_order() {
        // Una battuta ogni 5 minuti a 1 minuto al secondo: tutte, in ordine.
        let c = conv(Tone::Friendly, &[100, 105, 110, 115]);
        let log = run(&c, 1.0, 30.0, true);
        let texts: Vec<_> = log.iter().map(|(_, s)| s.clone()).collect();
        let expected: Vec<Shown> = (0..4)
            .map(|i| Shown::Text(format!("Battuta numero {i}")))
            .collect();
        assert_eq!(texts, expected);
    }

    #[test]
    fn fast_lines_stay_on_screen_and_outdated_ones_are_skipped() {
        // Una battuta al minuto a 10 minuti al secondo: 0.1 s l'una.
        let times: Vec<u64> = (100..130).collect();
        let c = conv(Tone::Neutral, &times);
        let log = run(&c, 10.0, 4.0, true);
        assert!(log.len() >= 2);
        for pair in log.windows(2) {
            assert!(pair[1].0 - pair[0].0 >= MIN_LINE_SECS - 1e-9, "{log:?}");
        }
        // Dopo la prima, si passa all'ultima detta (non alla successiva).
        assert_eq!(log[0].1, Shown::Text("Battuta numero 0".into()));
        assert_eq!(log[1].1, Shown::Text("Battuta numero 25".into()));
        // Prima che qualcuno parli, niente.
        let mut pacer = Pacer::default();
        assert_eq!(pace(&c, GameTime(99), 0.0, true, &mut pacer), None);
    }

    #[test]
    fn at_high_speed_only_dots_or_bang() {
        let c = conv(Tone::Friendly, &[100, 101]);
        let mut pacer = Pacer::default();
        assert_eq!(
            pace(&c, GameTime(100), 0.0, false, &mut pacer),
            Some((NpcId(1), Shown::Dots))
        );
        let c = conv(Tone::Tense, &[100, 101]);
        let mut pacer = Pacer::default();
        assert_eq!(
            pace(&c, GameTime(100), 0.0, false, &mut pacer).map(|(_, s)| s),
            Some(Shown::Bang)
        );
        // Anche i puntini cambiano parlante al più ogni MIN_LINE_SECS.
        assert_eq!(
            pace(&c, GameTime(101), 1.0, false, &mut pacer).map(|(who, _)| who),
            Some(NpcId(1))
        );
        assert_eq!(
            pace(&c, GameTime(101), MIN_LINE_SECS, false, &mut pacer).map(|(who, _)| who),
            Some(NpcId(2))
        );
    }

    fn candidate(id: u32, distance: f32) -> Candidate {
        Candidate {
            speaker: NpcId(id),
            selected: false,
            on_screen: true,
            distance,
        }
    }

    #[test]
    fn text_goes_to_the_selected_then_the_closest_with_a_cap() {
        let mut cands: Vec<Candidate> = (0..20).map(|i| candidate(i, i as f32 * 10.0)).collect();
        // Il selezionato è lontano ma ha comunque il testo.
        cands[19].selected = true;
        // Fuori dall'inquadratura: niente, anche se vicino.
        cands[0].on_screen = false;
        let slots = assign_slots(&cands, SpeechMode::All, &HashSet::new());
        assert_eq!(slots.iter().filter(|&&s| s == Slot::Text).count(), MAX_TEXT);
        assert_eq!(slots[19], Slot::Text);
        assert_eq!(slots[0], Slot::Hidden);
        assert_eq!(&slots[1..5], &[Slot::Text; 4]);
        assert_eq!(slots[5], Slot::Mark);
        assert_eq!(
            slots.iter().filter(|&&s| s == Slot::Mark).count(),
            MAX_MARKS.min(14)
        );
        assert_eq!(slots[18], Slot::Hidden);

        let only = assign_slots(&cands, SpeechMode::Selected, &HashSet::new());
        assert_eq!(only.iter().filter(|&&s| s != Slot::Hidden).count(), 1);
        assert_eq!(only[19], Slot::Text);
        let off = assign_slots(&cands, SpeechMode::Off, &HashSet::new());
        assert!(off.iter().all(|&s| s == Slot::Hidden));
    }

    #[test]
    fn deliberation_bubbles_win_over_chat_bubbles() {
        let cands = [candidate(1, 0.0), candidate(2, 5.0)];
        let busy: HashSet<NpcId> = [NpcId(1)].into();
        let slots = assign_slots(&cands, SpeechMode::All, &busy);
        // Chi ha un fumetto di deliberazione non ha quello del dialogo, e il
        // posto passa al successivo.
        assert_eq!(slots, [Slot::Hidden, Slot::Text]);
        let mut selected = cands;
        selected[0].selected = true;
        assert_eq!(
            assign_slots(&selected, SpeechMode::Selected, &busy),
            [Slot::Hidden, Slot::Hidden]
        );
    }

    #[test]
    fn close_bubbles_are_stacked() {
        let r = |x: f32, y: f32, w: f32| Rect::new(x, y, x + w, y + 10.0);
        let rects = [
            r(0.0, 50.0, 60.0),
            r(20.0, 52.0, 60.0),
            r(200.0, 50.0, 60.0),
            r(10.0, 50.0, 20.0),
        ];
        let offsets = stack(&rects);
        assert_eq!(offsets[0], 0.0);
        // Il secondo sale sopra il primo.
        assert_eq!(offsets[1], 60.0 + STACK_GAP - 52.0);
        // Lontano: resta dov'è.
        assert_eq!(offsets[2], 0.0);
        // Il quarto sale sopra entrambi.
        assert_eq!(50.0 + offsets[3], 52.0 + offsets[1] + 10.0 + STACK_GAP);
    }

    #[test]
    fn modes_cycle() {
        let mode = SpeechMode::default();
        assert_eq!(mode.next().next().next(), mode);
        assert_ne!(mode.next().label(), mode.label());
    }

    #[test]
    fn bubble_art_sizes() {
        for colors in TONE_COLORS {
            let body = body_canvas(colors);
            assert_eq!((body.width, body.height), (9, 9));
            assert_eq!(body.get(0, 0)[3], 0);
            let dots = dots_canvas(colors);
            assert_eq!((dots.width, dots.height), (11, 7));
            assert_eq!(tail_canvas(colors).width, 4);
        }
        assert_eq!(bubble_size(Some(&Shown::Dots), None), Vec2::new(11.0, 7.0));
        // Stima prima dell'impaginazione: due righe più alte di una.
        let one = bubble_size(Some(&Shown::Text("Ciao".into())), None);
        let two = bubble_size(Some(&Shown::Text("Ciao\ncome va?".into())), None);
        assert!(two.y > one.y && two.x > one.x);
    }

    #[test]
    fn game_font_has_the_ellipsis() {
        let face = ttf_parser::Face::parse(include_bytes!("../assets/fonts/PixelifySans.ttf"), 0)
            .expect("font valido");
        assert!(face.glyph_index('…').is_some_and(|g| g.0 != 0));
    }
}
