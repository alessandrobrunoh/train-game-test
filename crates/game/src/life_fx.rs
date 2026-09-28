//! Momenti di vita: piccoli effetti nel mondo e notifiche a comparsa.
//!
//! - Nel mondo (solo per gli NPC disegnati, cioè nelle carrozze inquadrate):
//!   un "+" sopra chi è appena nato, un cuore sopra i due di una nuova coppia,
//!   ogni tanto un cuoricino sopra due partner che chiacchierano insieme; chi
//!   muore svanisce in un secondo invece di sparire di colpo.
//! - Sullo schermo (in basso a destra): una pila di notifiche con gli eventi
//!   di vita più recenti (nascite, morti, coppie, maggiore età, pensione,
//!   vedovanza), che scadono dopo qualche secondo. Un click seleziona l'NPC,
//!   se è ancora vivo. Seguono i filtri del registro eventi.

use std::collections::{HashMap, VecDeque};

use bevy::prelude::*;
use bevy_egui::egui::{self, Align2, RichText};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use sim::{Action, Event, EventKind, NpcId};

use crate::inventory::InventoryWindow;
use crate::npc_render::{NpcSpriteIndex, NpcVisual};
use crate::saves::WorldRebuildSet;
use crate::state::{NpcSprite, SelectedNpc, Sim, WorldReplaced};
use crate::ui::{EventFilter, MARGIN, PointerCheck, event_color, event_message, year_of};

/// Durata della dissolvenza di chi muore (secondi).
const FADE_SECS: f32 = 1.0;
/// Di quanto sale mentre svanisce (unità mondo).
const FADE_RISE: f32 = 4.0;
/// Durata e salita delle icone sopra la testa.
const ICON_SECS: f32 = 2.2;
const ICON_RISE: f32 = 8.0;
/// Distanza tra la testa e l'icona.
const ICON_GAP: f32 = 5.0;
/// Frazione finale della vita dell'icona in cui svanisce.
const ICON_FADE: f32 = 0.4;
const ICON_Z: f32 = 7.0;
/// Scala del cuoricino dei partner che chiacchierano.
const SMALL_HEART: f32 = 0.6;
/// Ogni quanto (secondi) si controllano i partner che chiacchierano, con che
/// probabilità spunta un cuoricino e dopo quanto può rispuntare per la coppia.
const CHAT_CHECK_SECS: f32 = 0.5;
const CHAT_HEART_CHANCE: f32 = 0.25;
const CHAT_HEART_COOLDOWN: f32 = 4.0;

const PLUS_COLOR: Color = Color::srgb(0.55, 1.0, 0.55);
const HEART_COLOR: Color = Color::srgb(1.0, 0.35, 0.50);

/// Notifiche visibili al massimo e per quanto restano (secondi reali).
const MAX_TOASTS: usize = 5;
const TOAST_SECS: f64 = 6.0;
/// Ultimo secondo: la notifica svanisce.
const TOAST_FADE_SECS: f64 = 1.0;
const TOAST_WIDTH: f32 = 290.0;
/// Distanza dal fondo dello schermo (sopra la riga d'aiuto dei comandi).
const TOAST_BOTTOM: f32 = 40.0;

pub struct LifeFxPlugin;

impl Plugin for LifeFxPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Toasts>()
            .add_systems(
                PreUpdate,
                reset_life_fx
                    .in_set(WorldRebuildSet)
                    .run_if(on_message::<WorldReplaced>),
            )
            .add_systems(
                Update,
                (read_life_events, chat_hearts, move_icons, fade_out_dead)
                    .chain()
                    .after(crate::npc_render::NpcRenderSet),
            )
            .add_systems(EguiPrimaryContextPass, toast_stack.before(PointerCheck));
    }
}

// --- Eventi nuovi (dati puri) -------------------------------------------------

/// Eventi arrivati dopo `cursor` (numero di eventi già visti, contando anche
/// quelli scartati dal registro); aggiorna il cursore. `total` è
/// `World::events_total()`.
fn new_events<'a>(events: &'a [Event], total: u64, cursor: &mut u64) -> &'a [Event] {
    let len = events.len() as u64;
    let dropped = total.saturating_sub(len);
    let start = cursor.saturating_sub(dropped).min(len) as usize;
    *cursor = total;
    &events[start..]
}

/// Evento da notificare con un toast, e l'NPC da selezionare cliccandolo.
fn toast_npc(kind: &EventKind) -> Option<Option<NpcId>> {
    match kind {
        EventKind::Born { npc, .. }
        | EventKind::Coupled { npc, .. }
        | EventKind::CameOfAge { npc, .. }
        | EventKind::Retired { npc, .. }
        | EventKind::Widowed { npc, .. } => Some(Some(*npc)),
        EventKind::NpcDied { .. } => Some(None),
        _ => None,
    }
}

// --- Notifiche ----------------------------------------------------------------

#[derive(Clone, Debug)]
struct Toast {
    event: Event,
    /// NPC selezionato cliccando la notifica (se è ancora vivo).
    npc: Option<NpcId>,
    /// Istante (secondi reali) in cui è comparsa.
    shown_at: f64,
}

/// Pila delle notifiche, dalla più vecchia alla più recente.
#[derive(Resource, Default)]
struct Toasts(VecDeque<Toast>);

impl Toasts {
    fn push(&mut self, toast: Toast) {
        self.0.push_back(toast);
        while self.0.len() > MAX_TOASTS {
            self.0.pop_front();
        }
    }

    /// Toglie le notifiche più vecchie di `TOAST_SECS` all'istante `now`.
    fn expire(&mut self, now: f64) {
        self.0.retain(|t| now - t.shown_at < TOAST_SECS);
    }
}

/// Opacità di una notifica mostrata da `age` secondi: svanisce nell'ultimo secondo.
fn toast_opacity(age: f64) -> f32 {
    ((TOAST_SECS - age) / TOAST_FADE_SECS).clamp(0.0, 1.0) as f32
}

// --- Effetti nel mondo --------------------------------------------------------

/// Sprite di un NPC morto che svanisce (vedi `npc_render.rs`).
#[derive(Component, Default)]
pub(crate) struct FadingOut {
    elapsed: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IconKind {
    /// "+" sopra un neonato.
    Plus,
    /// Cuore sopra una nuova coppia.
    Heart,
    /// Cuoricino sopra due partner che chiacchierano.
    SmallHeart,
}

/// Sprite degli NPC, disgiunti dalle icone.
type NpcNotIcon = (With<NpcSprite>, Without<FloatingIcon>);

/// Icona che sale sopra la testa di un NPC e poi svanisce.
#[derive(Component)]
struct FloatingIcon {
    npc: NpcId,
    /// Segue la testa dell'NPC (i cuoricini dei partner restano fermi a metà
    /// strada tra i due).
    follows: bool,
    elapsed: f32,
    /// Ultima posizione della testa (se lo sprite sparisce l'icona resta lì).
    head: Vec2,
}

/// Rettangoli (centro, dimensioni) che compongono un'icona, in "pixel" mondo.
fn icon_pixels(kind: IconKind) -> Vec<(Vec2, Vec2)> {
    let r = |x: f32, y: f32, w: f32, h: f32| (Vec2::new(x, y), Vec2::new(w, h));
    match kind {
        IconKind::Plus => vec![r(0.0, 0.0, 5.0, 1.6), r(0.0, 0.0, 1.6, 5.0)],
        // .XX.XX.
        // XXXXXXX
        // XXXXXXX
        // .XXXXX.
        // ..XXX..
        // ...X...
        IconKind::Heart | IconKind::SmallHeart => vec![
            r(-1.5, 2.0, 2.0, 1.0),
            r(1.5, 2.0, 2.0, 1.0),
            r(0.0, 0.5, 7.0, 2.0),
            r(0.0, -1.0, 5.0, 1.0),
            r(0.0, -2.0, 3.0, 1.0),
            r(0.0, -3.0, 1.0, 1.0),
        ],
    }
}

fn spawn_icon(commands: &mut Commands, kind: IconKind, npc: NpcId, head: Vec2) {
    let (color, scale) = match kind {
        IconKind::Plus => (PLUS_COLOR, 1.0),
        IconKind::Heart => (HEART_COLOR, 1.0),
        IconKind::SmallHeart => (HEART_COLOR, SMALL_HEART),
    };
    commands
        .spawn((
            Name::new("Icona"),
            FloatingIcon {
                npc,
                follows: kind != IconKind::SmallHeart,
                elapsed: 0.0,
                head,
            },
            Transform::from_translation((head + Vec2::Y * ICON_GAP).extend(ICON_Z))
                .with_scale(Vec3::splat(scale)),
            Visibility::Inherited,
        ))
        .with_children(|parent| {
            for (center, size) in icon_pixels(kind) {
                parent.spawn((
                    Sprite::from_color(color, size),
                    Transform::from_translation(center.extend(0.0)),
                ));
            }
        });
}

/// Testa dello sprite di un NPC disegnato.
fn head_of(
    index: &NpcSpriteIndex,
    sprites: &Query<(&Transform, &NpcVisual), With<NpcSprite>>,
    id: NpcId,
) -> Option<Vec2> {
    let (transform, visual) = sprites.get(index.entity(id)?).ok()?;
    Some(transform.translation.truncate() + Vec2::Y * visual.half_height())
}

// --- Sistemi ------------------------------------------------------------------

/// Mondo sostituito: via notifiche e icone del mondo precedente (chi svanisce
/// lo toglie `npc_render`).
fn reset_life_fx(
    mut commands: Commands,
    mut toasts: ResMut<Toasts>,
    icons: Query<Entity, With<FloatingIcon>>,
) {
    toasts.0.clear();
    for entity in &icons {
        commands.entity(entity).despawn();
    }
}

/// Legge gli eventi nuovi: notifiche e icone sopra chi nasce o si mette in coppia.
#[allow(clippy::too_many_arguments)]
fn read_life_events(
    mut commands: Commands,
    time: Res<Time<Real>>,
    sim: Res<Sim>,
    filter: Res<EventFilter>,
    index: Res<NpcSpriteIndex>,
    sprites: Query<(&Transform, &NpcVisual), With<NpcSprite>>,
    mut toasts: ResMut<Toasts>,
    mut replaced: MessageReader<WorldReplaced>,
    mut cursor: Local<Option<u64>>,
) {
    let world = &sim.world;
    let total = world.events_total();
    // Al primo frame, e quando il mondo viene sostituito, si parte dagli
    // eventi già presenti, senza notificarli.
    if replaced.read().count() > 0 {
        *cursor = None;
    }
    let cursor = cursor.get_or_insert(total);
    let now = time.elapsed_secs_f64();
    for event in new_events(&world.events, total, cursor) {
        if let Some(npc) = toast_npc(&event.kind)
            && filter.allows(&event.kind)
        {
            toasts.push(Toast {
                event: event.clone(),
                npc,
                shown_at: now,
            });
        }
        let icons: &[(IconKind, NpcId)] = match &event.kind {
            EventKind::Born { npc, .. } => &[(IconKind::Plus, *npc)],
            EventKind::Coupled { npc, partner, .. } => {
                &[(IconKind::Heart, *npc), (IconKind::Heart, *partner)]
            }
            _ => &[],
        };
        for &(kind, npc) in icons {
            if let Some(head) = head_of(&index, &sprites, npc) {
                spawn_icon(&mut commands, kind, npc, head);
            }
        }
    }
    if toasts
        .0
        .front()
        .is_some_and(|t| now - t.shown_at >= TOAST_SECS)
    {
        toasts.expire(now);
    }
}

/// Ogni tanto un cuoricino sopra due partner che chiacchierano tra loro.
#[allow(clippy::too_many_arguments)]
fn chat_hearts(
    mut commands: Commands,
    time: Res<Time>,
    sim: Res<Sim>,
    index: Res<NpcSpriteIndex>,
    sprites: Query<(&Transform, &NpcVisual), With<NpcSprite>>,
    ids: Query<&NpcSprite>,
    mut timer: Local<f32>,
    mut cooldown: Local<HashMap<NpcId, f32>>,
) {
    *timer += time.delta_secs();
    if *timer < CHAT_CHECK_SECS {
        return;
    }
    *timer = 0.0;
    let now = time.elapsed_secs();
    cooldown.retain(|_, &mut until| until > now);
    let world = &sim.world;
    for sprite in &ids {
        let Some(npc) = world.npc(sprite.0) else {
            continue;
        };
        let Action::Socialize(other) = npc.action else {
            continue;
        };
        // Una volta per coppia (dal lato dell'id minore), solo tra partner
        // che si parlano a vicenda.
        if npc.id > other || npc.partner() != Some(other) || cooldown.contains_key(&npc.id) {
            continue;
        }
        let talks_back = world
            .npc(other)
            .is_some_and(|p| p.action == Action::Socialize(npc.id));
        if !talks_back || rand_chance(npc.id, now) >= CHAT_HEART_CHANCE {
            continue;
        }
        let heads = (
            head_of(&index, &sprites, npc.id),
            head_of(&index, &sprites, other),
        );
        if let (Some(a), Some(b)) = heads {
            // A metà strada tra i due, sopra il più alto.
            let head = Vec2::new((a.x + b.x) / 2.0, a.y.max(b.y));
            spawn_icon(&mut commands, IconKind::SmallHeart, npc.id, head);
            cooldown.insert(npc.id, now + CHAT_HEART_COOLDOWN);
        }
    }
}

/// Numero pseudo-casuale in `0..1` per un NPC e un istante (niente rng globale).
fn rand_chance(id: NpcId, now: f32) -> f32 {
    let mut z = (u64::from(id.0) << 32) ^ u64::from(now.to_bits());
    z = (z ^ (z >> 33)).wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    z = (z ^ (z >> 33)).wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    z ^= z >> 33;
    (z >> 40) as f32 / (1u64 << 24) as f32
}

/// Le icone seguono la testa dell'NPC, salgono e svaniscono.
fn move_icons(
    mut commands: Commands,
    time: Res<Time>,
    index: Res<NpcSpriteIndex>,
    sprites: Query<(&Transform, &NpcVisual), NpcNotIcon>,
    mut icons: Query<(Entity, &mut FloatingIcon, &mut Transform, &Children)>,
    mut pixels: Query<&mut Sprite, Without<NpcSprite>>,
) {
    let dt = time.delta_secs();
    for (entity, mut icon, mut transform, children) in &mut icons {
        icon.elapsed += dt;
        let t = icon.elapsed / ICON_SECS;
        if t >= 1.0 {
            commands.entity(entity).despawn();
            continue;
        }
        let head = index
            .entity(icon.npc)
            .filter(|_| icon.follows)
            .and_then(|e| sprites.get(e).ok())
            .map(|(tr, visual)| tr.translation.truncate() + Vec2::Y * visual.half_height());
        if let Some(head) = head {
            icon.head = head;
        }
        let position = icon.head + Vec2::Y * (ICON_GAP + ICON_RISE * t);
        transform.translation.x = position.x;
        transform.translation.y = position.y;
        let alpha = ((1.0 - t) / ICON_FADE).min(1.0);
        for &child in children {
            if let Ok(mut sprite) = pixels.get_mut(child) {
                sprite.color.set_alpha(alpha);
            }
        }
    }
}

/// Chi è morto svanisce salendo un poco, poi viene rimosso.
fn fade_out_dead(
    mut commands: Commands,
    time: Res<Time>,
    mut fading: Query<(
        Entity,
        &mut FadingOut,
        &mut Transform,
        &mut Sprite,
        &Children,
    )>,
    mut parts: Query<&mut Sprite, Without<FadingOut>>,
) {
    let dt = time.delta_secs();
    for (entity, mut fade, mut transform, mut sprite, children) in &mut fading {
        fade.elapsed += dt;
        let t = fade.elapsed / FADE_SECS;
        if t >= 1.0 {
            commands.entity(entity).despawn();
            continue;
        }
        let alpha = 1.0 - t;
        transform.translation.y += FADE_RISE / FADE_SECS * dt;
        sprite.color.set_alpha(alpha);
        for &child in children {
            if let Ok(mut part) = parts.get_mut(child) {
                part.color.set_alpha(alpha);
            }
        }
    }
}

/// Pila di notifiche in basso a destra (sopra l'inventario, se è aperto).
fn toast_stack(
    mut contexts: EguiContexts,
    time: Res<Time<Real>>,
    sim: Option<Res<Sim>>,
    inventory: Res<InventoryWindow>,
    toasts: Res<Toasts>,
    mut selected: ResMut<SelectedNpc>,
) {
    if toasts.0.is_empty() {
        return;
    }
    let Some(sim) = sim else {
        return;
    };
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let above_inventory = if inventory.open {
        ctx.memory(|m| m.area_rect(egui::Id::new("Inventario")))
            .map_or(0.0, |r| r.height() + MARGIN)
    } else {
        0.0
    };
    let now = time.elapsed_secs_f64();
    let world = &sim.world;
    let mut clicked = None;
    egui::Area::new(egui::Id::new("life_toasts"))
        .anchor(
            Align2::RIGHT_BOTTOM,
            [-MARGIN, -TOAST_BOTTOM - above_inventory],
        )
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.set_max_width(TOAST_WIDTH);
            for toast in &toasts.0 {
                let alive = toast.npc.filter(|&id| world.npc(id).is_some());
                ui.scope(|ui| {
                    ui.multiply_opacity(toast_opacity(now - toast.shown_at));
                    let frame = egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_width(TOAST_WIDTH);
                        let time = toast.event.time;
                        ui.label(
                            RichText::new(format!(
                                "Anno {} · {time}",
                                year_of(time, world.params.days_per_year)
                            ))
                            .small()
                            .weak(),
                        );
                        ui.colored_label(
                            event_color(&toast.event.kind),
                            event_message(&toast.event),
                        );
                    });
                    if let Some(id) = alive {
                        let response = frame
                            .response
                            .interact(egui::Sense::click())
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text("Click: seleziona");
                        if response.clicked() {
                            clicked = Some(id);
                        }
                    }
                });
            }
        });
    if let Some(id) = clicked {
        selected.0 = Some(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shortage(minutes: u64) -> Event {
        Event {
            time: sim::GameTime(minutes),
            kind: EventKind::Shortage {
                item: sim::ItemKind::Razione,
            },
        }
    }

    #[test]
    fn new_events_are_read_once_even_when_the_log_drops_old_ones() {
        let log: Vec<Event> = (0..5).map(shortage).collect();
        let mut cursor = 0;
        assert_eq!(new_events(&log, 5, &mut cursor).len(), 5);
        assert_eq!(cursor, 5);
        assert!(new_events(&log, 5, &mut cursor).is_empty());
        // Due eventi nuovi, e il registro ha scartato i primi tre.
        let log: Vec<Event> = (3..7).map(shortage).collect();
        let fresh = new_events(&log, 7, &mut cursor);
        assert_eq!(fresh.iter().map(|e| e.time.0).collect::<Vec<_>>(), [5, 6]);
        // Rimasti indietro più di quanto il registro conservi: tutto il registro.
        let mut stale = 1;
        assert_eq!(new_events(&log, 7, &mut stale).len(), 4);
    }

    #[test]
    fn toasts_keep_the_latest_and_expire() {
        let mut toasts = Toasts::default();
        for i in 0..MAX_TOASTS + 3 {
            toasts.push(Toast {
                event: shortage(i as u64),
                npc: None,
                shown_at: i as f64,
            });
        }
        assert_eq!(toasts.0.len(), MAX_TOASTS);
        assert_eq!(toasts.0.front().unwrap().event.time.0, 3);
        // Scadono quelle mostrate da almeno TOAST_SECS.
        toasts.expire(3.0 + TOAST_SECS);
        assert_eq!(toasts.0.len(), MAX_TOASTS - 1);
        toasts.expire(100.0);
        assert!(toasts.0.is_empty());
        assert_eq!(toast_opacity(0.0), 1.0);
        assert_eq!(toast_opacity(TOAST_SECS), 0.0);
        assert!((toast_opacity(TOAST_SECS - TOAST_FADE_SECS / 2.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn only_life_events_become_toasts() {
        let npc = NpcId(3);
        let born = EventKind::Born {
            npc,
            name: "Ada Neri".into(),
            sex: sim::Sex::Female,
            mother: NpcId(1),
            father: NpcId(2),
            mother_name: "Eva Neri".into(),
            father_name: "Leo Neri".into(),
        };
        assert_eq!(toast_npc(&born), Some(Some(npc)));
        let died = EventKind::NpcDied {
            npc,
            name: "Ada Neri".into(),
            cause: sim::DeathCause::OldAge,
            age: 80,
            sex: sim::Sex::Female,
        };
        // I morti non si possono selezionare.
        assert_eq!(toast_npc(&died), Some(None));
        assert_eq!(toast_npc(&shortage(0).kind), None);
    }

    #[test]
    fn icons_are_small() {
        for kind in [IconKind::Plus, IconKind::Heart] {
            for (center, size) in icon_pixels(kind) {
                let far = center.abs() + size / 2.0;
                assert!(far.x <= 3.5 && far.y <= 3.5, "{kind:?}");
            }
        }
    }
}
