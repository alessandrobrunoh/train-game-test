//! Salute e combattimento nel gioco (vedi `sim::combat`).
//!
//! - **Tasto X:** colpisce l'NPC più vicino a portata (stesso piano, come
//!   per parlare). Il primo colpo a chi non è ostile va confermato:
//!   compare "Sei sicuro?" e bisogna premere di nuovo X entro un secondo.
//!   Chi è ostile (ti sta picchiando, ce l'ha con te) si colpisce subito.
//! - **Colpi:** chi viene colpito lampeggia di rosso ed è spinto indietro di
//!   qualche pixel; sopra la testa sale il danno ("-7", "mancato").
//! - **Barre della salute:** sopra gli NPC feriti o in una rissa (verde,
//!   gialla, rossa); quella del giocatore è nell'HUD (`hud.rs`).
//! - **Svenimento:** lo schermo si scurisce, il giocatore si risveglia nel
//!   letto della cabina il mattino dopo (il sonno lo gestisce `cabin.rs`),
//!   con un messaggio su cosa ha perso. Con la "morte permanente" (opzione
//!   della nuova partita) invece compare la schermata "Sei morto".
//! - **Ispettore:** salute, ferite, rissa in corso, rancori ("ce l'ha con")
//!   e fama ([`health_section`]).
//! - **Fumetti:** chi aggredisce e chi reagisce gridano una battuta
//!   ([`fight_shouts`], disegnate da `bubbles.rs`).
//!
//! Chi è ferito cammina zoppicando e più piano, chi è grave sta a letto, chi
//! viene ucciso cade e resta a terra un attimo (vedi `npc_render.rs` e
//! `life_fx.rs`).

use std::collections::HashMap;

use bevy::prelude::*;
use bevy::sprite::{Anchor, Text2dShadow};
use bevy_egui::egui::{self, Align2, Color32, RichText};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use sim::{
    Condition, Event, EventKind, Fighter, GrudgeReason, MAX_HEALTH, Motive, Npc, NpcId, Reaction,
    Reputation, World,
};

use crate::chat::ChatWindow;
use crate::npc_render::{NpcRenderSet, NpcSpriteIndex, NpcVisual};
use crate::player::{Body, Player, start_position};
use crate::saves::{SaveCommand, SaveQueue, SavesWindow, WorldRebuildSet};
use crate::sim_bridge::SimTickSet;
use crate::state::{FollowNpc, NpcSprite, Sim, WorldReplaced};
use crate::stations::StationLayout;
use crate::train::{FLOOR_Y, TrainLayout};
use crate::ui::PointerCheck;

/// Portata di un pugno (x, y), dal centro del giocatore al centro dell'NPC.
const REACH: Vec2 = Vec2::new(16.0, 20.0);
/// Entro quanto (secondi reali) la seconda X conferma il primo colpo.
const CONFIRM_SECS: f64 = 1.0;
/// Quanto resta il messaggio dopo un colpo (secondi reali).
const MESSAGE_SECS: f64 = 1.6;
/// Lampo rosso di chi viene colpito (secondi) e spinta (unità).
const FLASH_SECS: f32 = 0.25;
const KNOCK_BACK: f32 = 4.0;
const FLASH_COLOR: Color = Color::srgb(1.0, 0.35, 0.3);
/// Numeri del danno: durata, salita, testo.
const NUMBER_SECS: f32 = 1.0;
const NUMBER_RISE: f32 = 10.0;
const NUMBER_Z: f32 = 9.0;
const TEXT_SIZE: f32 = 32.0;
const NUMBER_SCALE: f32 = 0.24;
const DAMAGE_COLOR: Color = Color::srgb(1.0, 0.25, 0.2);
const PLAYER_DAMAGE_COLOR: Color = Color::srgb(1.0, 0.55, 0.1);
const MISS_COLOR: Color = Color::srgb(0.85, 0.85, 0.85);
/// Barra della salute sopra la testa: dimensioni, distanza, profondità.
const BAR_SIZE: Vec2 = Vec2::new(14.0, 2.0);
const BAR_GAP: f32 = 3.0;
const BAR_Z: f32 = 7.5;
const BAR_BACK: Color = Color::srgba(0.1, 0.05, 0.05, 0.85);
/// Metà altezza del corpo del giocatore (la testa è sopra il centro).
fn player_half_height() -> f32 {
    start_position().y - FLOOR_Y
}
/// Svenimento: quanto dura il buio prima del risveglio, e il ritorno della luce.
const FADE_OUT_SECS: f32 = 0.8;
const FADE_IN_SECS: f32 = 1.2;
/// Quanto resta il messaggio al risveglio (secondi reali).
const WAKE_MESSAGE_SECS: f64 = 6.0;

pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AttackState>()
            .init_resource::<HealthBars>()
            .init_resource::<FaintFx>()
            .add_systems(
                PreUpdate,
                reset_combat
                    .in_set(WorldRebuildSet)
                    .run_if(on_message::<WorldReplaced>),
            )
            .add_systems(Update, attack_key.before(SimTickSet))
            .add_systems(
                Update,
                (
                    fight_effects,
                    flash_hits,
                    move_numbers,
                    health_bars,
                    faint_watch,
                )
                    .chain()
                    .after(NpcRenderSet)
                    .after(SimTickSet),
            )
            .add_systems(
                EguiPrimaryContextPass,
                (attack_message, faint_overlay).before(PointerCheck),
            );
    }
}

// --- Stato ----------------------------------------------------------------------

/// Il tasto X: la conferma in attesa e l'esito dell'ultimo colpo.
#[derive(Resource, Default)]
struct AttackState {
    /// NPC da confermare (seconda X) e quando è stata premuta la prima.
    armed: Option<(NpcId, f64)>,
    /// Messaggio da mostrare e da quando.
    message: Option<(String, f64, Color32)>,
}

/// Barre della salute sopra gli NPC: NPC -> entità della barra.
#[derive(Resource, Default)]
struct HealthBars(HashMap<NpcId, Entity>);

/// Svenimento in corso (buio, risveglio) e il messaggio al risveglio.
#[derive(Resource, Default)]
struct FaintFx {
    /// Secondi reali dall'inizio del buio (None: niente svenimento in corso).
    dark: Option<f32>,
    /// Il corpo è già nel letto della cabina.
    moved: bool,
    /// Secondi reali dal risveglio (per la luce che torna).
    waking: Option<f32>,
    /// Cosa si è perso (dall'evento).
    lost: String,
    /// Messaggio al risveglio e quando è comparso.
    message: Option<(String, f64)>,
}

/// Numero del danno che sale sopra la testa e svanisce.
#[derive(Component)]
struct DamageNumber {
    elapsed: f32,
}

/// Lampo rosso di chi è stato colpito.
#[derive(Component)]
struct HitFlash {
    elapsed: f32,
}

/// Barra della salute di un NPC (lo sfondo; la parte piena è un figlio).
#[derive(Component)]
struct HealthBar;

#[derive(Component)]
struct HealthFill;

fn reset_combat(
    mut commands: Commands,
    mut state: ResMut<AttackState>,
    mut bars: ResMut<HealthBars>,
    mut faint: ResMut<FaintFx>,
) {
    *state = AttackState::default();
    for (_, entity) in bars.0.drain() {
        commands.entity(entity).despawn();
    }
    *faint = FaintFx::default();
}

// --- Dati puri ------------------------------------------------------------------

/// L'NPC più vicino a portata del giocatore in `player` (centro del corpo),
/// tra gli NPC disegnati (`npcs`: id e centro dello sprite).
fn nearest_npc(player: Vec2, npcs: impl Iterator<Item = (NpcId, Vec2)>) -> Option<NpcId> {
    npcs.filter(|&(_, pos)| {
        let d = (pos - player).abs();
        d.x <= REACH.x && d.y <= REACH.y
    })
    .min_by(|a, b| {
        let (da, db) = ((a.1.x - player.x).abs(), (b.1.x - player.x).abs());
        da.total_cmp(&db).then(a.0.cmp(&b.0))
    })
    .map(|(id, _)| id)
}

/// Se il colpo a `id` va confermato: il primo colpo a chi non è ostile
/// (non ti sta picchiando e non ce l'ha con te).
fn needs_confirm(world: &World, id: NpcId) -> bool {
    !world.is_hostile_to_player(id)
}

/// Colore della salute: verde, giallo sotto la soglia del ferito, rosso
/// sotto quella del grave.
fn health_color(world: &World, health: f32) -> Color {
    match Condition::of(health, &world.params) {
        Condition::Healthy => Color::srgb(0.35, 0.85, 0.35),
        Condition::Hurt => Color::srgb(0.95, 0.8, 0.2),
        Condition::Bedridden => Color::srgb(0.9, 0.2, 0.15),
    }
}

/// Se sopra l'NPC va disegnata la barra: è ferito o in una rissa.
fn shows_bar(world: &World, npc: &Npc) -> bool {
    npc.is_hurt() || world.fight_of(Fighter::Npc(npc.id)).is_some()
}

/// Cosa gridano i due di una rissa al primo colpo (`event` è un
/// `Attacked` con `first`): chi aggredisce, per il motivo, e la vittima,
/// per come ha reagito. Solo gli NPC (il giocatore non ha fumetti).
pub(crate) fn fight_shouts(world: &World, event: &Event) -> Vec<(NpcId, &'static str)> {
    let EventKind::Attacked {
        attacker,
        victim,
        motive,
        first: true,
        ..
    } = event.kind
    else {
        return Vec::new();
    };
    let mut shouts = Vec::new();
    if let Fighter::Npc(a) = attacker {
        let victim_sex = victim.npc().and_then(|v| world.npc(v)).map(|n| n.sex);
        let line = match motive {
            Motive::Quarrel => "Ora basta!",
            Motive::Grudge => "Te la faccio pagare!",
            Motive::Revenge => "Questa è per i miei!",
            Motive::Thief => match victim_sex {
                Some(sim::Sex::Female) => "Ladra!",
                _ => "Ladro!",
            },
            Motive::Robbery => "Dammi da mangiare!",
            Motive::Defense | Motive::Player => "Stai indietro!",
            Motive::Pizzo => "Paga, se sai cosa è bene!",
            Motive::Gang => "Questa è zona nostra!",
            Motive::Hit => "Ti manda i saluti il capo.",
        };
        shouts.push((a, line));
    }
    if let Fighter::Npc(v) = victim {
        let reaction = world
            .fights()
            .iter()
            .rev()
            .find(|f| f.attacker == attacker && f.victim == victim && f.since == event.time)
            .and_then(|f| f.reaction);
        let line = match reaction {
            Some(Reaction::FightBack) => "Vieni avanti!",
            Some(Reaction::Flee { .. }) => "Aiuto!",
            Some(Reaction::GiveIn) => "Lasciami stare!",
            None => "Ahi!",
        };
        shouts.push((v, line));
    }
    shouts
}

/// Perché ce l'ha con qualcuno, in breve.
fn grudge_reason(world: &World, reason: GrudgeReason) -> String {
    let name = |id: NpcId| {
        world
            .npc(id)
            .map_or_else(|| "qualcuno".to_string(), |n| n.first_name().to_string())
    };
    match reason {
        GrudgeReason::Attacked => "l'ha aggredito".to_string(),
        GrudgeReason::HurtLovedOne(who) => format!("ha picchiato {}", name(who)),
        GrudgeReason::KilledLovedOne(who) => format!("ha ucciso {}", name(who)),
        GrudgeReason::Theft => "l'ha sorpreso a rubare".to_string(),
        GrudgeReason::Extorted => "gli ha preso il pizzo".to_string(),
        GrudgeReason::GangMate(Fighter::Npc(who)) => {
            format!("ha colpito {}, della sua banda", name(who))
        }
        GrudgeReason::GangMate(Fighter::Player) => "ha colpito te, della sua banda".to_string(),
        GrudgeReason::Defied => "ha sfidato la sua banda".to_string(),
    }
}

/// Sezione "Salute" dell'ispettore: salute e ferite, la rissa in corso, i
/// rancori ("Ce l'ha con: …", ognuno un link) e la fama. Restituisce l'NPC
/// cliccato.
pub(crate) fn health_section(ui: &mut egui::Ui, world: &World, npc: &Npc) -> Option<NpcId> {
    let mut clicked = None;
    ui.strong("Salute");
    let condition = npc.condition(&world.params);
    let color = crate::ui::color32(health_color(world, npc.health));
    ui.add(
        egui::ProgressBar::new(npc.health / MAX_HEALTH)
            .fill(color)
            .text(format!(
                "{:.0}/{:.0} · {}",
                npc.health,
                MAX_HEALTH,
                condition.label(npc.sex)
            )),
    );
    if npc.injury >= 1.0 {
        ui.weak(format!("Ferite da guarire: {:.0}", npc.injury));
    }
    let mut link = |ui: &mut egui::Ui, who: Fighter| match who {
        Fighter::Npc(id) => match world.npc(id) {
            Some(n) => {
                if ui.link(&n.name).clicked() {
                    clicked = Some(id);
                }
            }
            None => {
                ui.label("qualcuno che non c'è più");
            }
        },
        Fighter::Player => {
            ui.colored_label(Color32::from_rgb(120, 200, 255), "te");
        }
    };
    if let Some(f) = world.fight_of(Fighter::Npc(npc.id)) {
        let other = f.opponent(Fighter::Npc(npc.id)).unwrap_or(f.attacker);
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(Color32::from_rgb(235, 120, 90), "In rissa con");
            link(ui, other);
            ui.weak(format!("({})", f.motive.name()));
        });
    }
    if let Some(by) = npc.last_attacker
        && npc.is_hurt()
    {
        ui.horizontal_wrapped(|ui| {
            ui.weak("Ferito da:");
            link(ui, by);
        });
    }
    if !npc.grudges.is_empty() {
        let mut grudges: Vec<_> = npc.grudges.iter().collect();
        grudges.sort_by(|a, b| b.strength.total_cmp(&a.strength));
        ui.weak("Ce l'ha con:");
        for g in grudges {
            ui.horizontal_wrapped(|ui| {
                link(ui, g.against);
                ui.weak(format!(
                    "— {} ({:.0}%)",
                    grudge_reason(world, g.reason),
                    g.strength * 100.0
                ));
            });
        }
    }
    let reputation = npc.reputation();
    let fama = ui.horizontal_wrapped(|ui| {
        ui.weak("Fama:");
        let color = match reputation {
            Reputation::Peaceful => Color32::GRAY,
            Reputation::Brawler => Color32::from_rgb(240, 160, 60),
            Reputation::Violent | Reputation::Feared => Color32::from_rgb(230, 80, 80),
        };
        ui.colored_label(color, reputation.label(npc.sex));
    });
    fama.response.on_hover_text(format!(
        "Violenza {:.0}% (sale con ogni rissa cominciata, di più per chi uccide; scende piano)",
        npc.violence * 100.0
    ));
    clicked
}

// --- Sistemi ----------------------------------------------------------------------

/// X: colpisce l'NPC più vicino (il primo colpo a chi non è ostile va
/// confermato con una seconda X entro un secondo).
#[allow(clippy::too_many_arguments)]
fn attack_key(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time<Real>>,
    mut sim: ResMut<Sim>,
    follow: Res<FollowNpc>,
    chat: Res<ChatWindow>,
    mut state: ResMut<AttackState>,
    player: Single<&Transform, With<Player>>,
    npcs: Query<(&NpcSprite, &Transform), Without<Player>>,
) {
    if !keys.just_pressed(KeyCode::KeyX) || follow.0 || chat.is_open() {
        return;
    }
    let now = time.elapsed_secs_f64();
    let world = &mut sim.world;
    if world.player.is_down() || world.player.is_asleep() {
        return;
    }
    let here = player.translation.truncate();
    let target = nearest_npc(
        here,
        npcs.iter()
            .map(|(npc, t)| (npc.0, t.translation.truncate())),
    );
    let Some(id) = target else {
        state.message = Some(("Nessuno a portata".to_string(), now, Color32::GRAY));
        return;
    };
    let name = world
        .npc(id)
        .map_or_else(String::new, |n| n.first_name().to_string());
    let confirmed = state
        .armed
        .is_some_and(|(armed, at)| armed == id && now - at <= CONFIRM_SECS);
    if needs_confirm(world, id) && !confirmed {
        state.armed = Some((id, now));
        state.message = Some((
            format!("Sei sicuro? Premi di nuovo X per colpire {name}"),
            now,
            Color32::from_rgb(255, 210, 90),
        ));
        return;
    }
    state.armed = None;
    let (text, color) = match world.player_attack(id) {
        Ok(out) if out.killed => (format!("Hai ucciso {name}"), Color32::from_rgb(230, 70, 70)),
        Ok(out) if out.damage <= 0.0 => ("Mancato!".to_string(), Color32::GRAY),
        Ok(out) => {
            let reaction = match out.reaction {
                Some(Reaction::FightBack) => " · reagisce!",
                Some(Reaction::Flee { .. }) => " · scappa",
                Some(Reaction::GiveIn) => " · si arrende",
                None => "",
            };
            (
                format!("Colpisci {name}: -{:.0}{reaction}", out.damage),
                Color32::from_rgb(255, 150, 120),
            )
        }
        Err(e) => (format!("{name}: {e}"), Color32::GRAY),
    };
    state.message = Some((text, now, color));
}

/// Nuovi colpi, uccisioni e svenimenti: numeri del danno, lampi, spinte.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn fight_effects(
    mut commands: Commands,
    sim: Res<Sim>,
    index: Res<NpcSpriteIndex>,
    mut sprites: Query<(&mut Transform, &NpcVisual), (With<NpcSprite>, Without<Player>)>,
    mut player: Query<(Entity, &Transform, &mut Body), (With<Player>, Without<NpcSprite>)>,
    mut faint: ResMut<FaintFx>,
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
    if fresh == 0 {
        return;
    }
    let start = world.events.len().saturating_sub(fresh);
    for event in &world.events[start..] {
        match &event.kind {
            EventKind::Attacked {
                attacker,
                victim,
                damage,
                first,
                ..
            } => {
                if *damage == 0 && !*first {
                    continue;
                }
                // Dove sta chi colpisce (per la spinta) e chi è colpito.
                let spot = |who: Fighter| -> Option<(Vec2, f32)> {
                    match who {
                        Fighter::Npc(id) => {
                            let e = index.entity(id)?;
                            let (t, v) = sprites.get(e).ok()?;
                            Some((t.translation.truncate(), v.half_height()))
                        }
                        Fighter::Player => player
                            .single()
                            .ok()
                            .map(|(_, t, _)| (t.translation.truncate(), player_half_height())),
                    }
                };
                let Some((at, half)) = spot(*victim) else {
                    continue;
                };
                let from = spot(*attacker).map(|(p, _)| p);
                let (text, color) = match (*damage, *victim) {
                    (0, _) => ("mancato".to_string(), MISS_COLOR),
                    (d, Fighter::Player) => (format!("-{d}"), PLAYER_DAMAGE_COLOR),
                    (d, _) => (format!("-{d}"), DAMAGE_COLOR),
                };
                commands.spawn((
                    Name::new("Danno"),
                    DamageNumber { elapsed: 0.0 },
                    Text2d::new(text),
                    TextFont {
                        font_size: FontSize::Px(TEXT_SIZE),
                        ..default()
                    },
                    TextColor(color),
                    Text2dShadow::default(),
                    Anchor::BOTTOM_CENTER,
                    Transform::from_xyz(at.x, at.y + half + 2.0, NUMBER_Z)
                        .with_scale(Vec3::splat(NUMBER_SCALE)),
                ));
                if *damage == 0 {
                    continue;
                }
                // Spinta lontano da chi colpisce, e il lampo rosso.
                let push = match from {
                    Some(p) if p.x > at.x => -KNOCK_BACK,
                    Some(_) => KNOCK_BACK,
                    None => KNOCK_BACK,
                };
                match *victim {
                    Fighter::Npc(id) => {
                        if let Some(e) = index.entity(id) {
                            if let Ok((mut t, _)) = sprites.get_mut(e) {
                                t.translation.x += push;
                            }
                            commands.entity(e).insert(HitFlash { elapsed: 0.0 });
                        }
                    }
                    Fighter::Player => {
                        if let Ok((e, _, mut body)) = player.single_mut() {
                            body.velocity.x += push * 20.0;
                            commands.entity(e).insert(HitFlash { elapsed: 0.0 });
                        }
                    }
                }
            }
            EventKind::Fainted {
                tokens,
                item,
                dead: false,
                ..
            } => {
                let mut lost = Vec::new();
                if *tokens > 0 {
                    lost.push(format!("{tokens} gettoni"));
                }
                if let Some(item) = item {
                    lost.push(item.with_article().to_string());
                }
                *faint = FaintFx {
                    dark: Some(0.0),
                    lost: lost.join(" e "),
                    ..FaintFx::default()
                };
            }
            _ => {}
        }
    }
}

/// Il lampo rosso: dopo `npc_render` (che rimette il colore normale).
fn flash_hits(
    mut commands: Commands,
    time: Res<Time<Real>>,
    mut flashed: Query<(Entity, &mut HitFlash, &mut Sprite, Has<Player>)>,
) {
    let dt = time.delta_secs();
    for (entity, mut flash, mut sprite, is_player) in &mut flashed {
        flash.elapsed += dt;
        if flash.elapsed >= FLASH_SECS {
            commands.entity(entity).remove::<HitFlash>();
            if is_player {
                sprite.color = Color::WHITE;
            }
            continue;
        }
        // Rosso pieno all'inizio, poi torna normale.
        let t = flash.elapsed / FLASH_SECS;
        let base = sprite.color.to_srgba();
        let red = FLASH_COLOR.to_srgba();
        let mix = |a: f32, b: f32| b + (a - b) * t;
        sprite.color = Color::srgba(
            mix(base.red, red.red),
            mix(base.green, red.green),
            mix(base.blue, red.blue),
            base.alpha,
        );
    }
}

/// I numeri del danno salgono e svaniscono.
fn move_numbers(
    mut commands: Commands,
    time: Res<Time<Real>>,
    mut numbers: Query<(Entity, &mut DamageNumber, &mut Transform, &mut TextColor)>,
) {
    let dt = time.delta_secs();
    for (entity, mut number, mut transform, mut color) in &mut numbers {
        number.elapsed += dt;
        if number.elapsed >= NUMBER_SECS {
            commands.entity(entity).despawn();
            continue;
        }
        transform.translation.y += NUMBER_RISE / NUMBER_SECS * dt;
        let t = number.elapsed / NUMBER_SECS;
        color.0.set_alpha((1.5 * (1.0 - t)).min(1.0));
    }
}

/// Barre della salute sopra gli NPC disegnati che sono feriti o in rissa.
#[allow(clippy::type_complexity)]
fn health_bars(
    mut commands: Commands,
    sim: Res<Sim>,
    mut bars: ResMut<HealthBars>,
    sprites: Query<(&NpcSprite, &Transform, &NpcVisual)>,
    mut backs: Query<(&mut Transform, &Children), (With<HealthBar>, Without<NpcSprite>)>,
    mut fills: Query<&mut Sprite, (With<HealthFill>, Without<HealthBar>)>,
) {
    let world = &sim.world;
    let mut wanted: HashMap<NpcId, (Vec2, f32)> = HashMap::new();
    for (npc, transform, visual) in &sprites {
        let Some(n) = world.npc(npc.0) else {
            continue;
        };
        if shows_bar(world, n) {
            let head =
                transform.translation.truncate() + Vec2::Y * (visual.half_height() + BAR_GAP);
            wanted.insert(n.id, (head, n.health));
        }
    }
    bars.0.retain(|id, entity| {
        let keep = wanted.contains_key(id);
        if !keep {
            commands.entity(*entity).despawn();
        }
        keep
    });
    for (id, (head, health)) in wanted {
        let share = (health / MAX_HEALTH).clamp(0.0, 1.0);
        let color = health_color(world, health);
        match bars.0.get(&id) {
            Some(&entity) => {
                if let Ok((mut transform, children)) = backs.get_mut(entity) {
                    transform.translation.x = head.x;
                    transform.translation.y = head.y;
                    for &child in children {
                        if let Ok(mut fill) = fills.get_mut(child) {
                            fill.custom_size = Some(Vec2::new(BAR_SIZE.x * share, BAR_SIZE.y));
                            fill.color = color;
                        }
                    }
                }
            }
            None => {
                let entity = commands
                    .spawn((
                        Name::new("Barra salute"),
                        HealthBar,
                        Sprite {
                            color: BAR_BACK,
                            custom_size: Some(BAR_SIZE + Vec2::splat(1.0)),
                            ..default()
                        },
                        Transform::from_xyz(head.x, head.y, BAR_Z),
                    ))
                    .with_child((
                        HealthFill,
                        Sprite {
                            color,
                            custom_size: Some(Vec2::new(BAR_SIZE.x * share, BAR_SIZE.y)),
                            ..default()
                        },
                        Anchor::CENTER_LEFT,
                        Transform::from_xyz(-BAR_SIZE.x / 2.0, 0.0, 0.01),
                    ))
                    .id();
                bars.0.insert(id, entity);
            }
        }
    }
}

/// Svenimento: dopo il buio il corpo va nel letto della cabina (la sim ha
/// già messo lì il giocatore); al risveglio torna la luce, con un messaggio.
fn faint_watch(
    time: Res<Time<Real>>,
    sim: Res<Sim>,
    stations: Res<StationLayout>,
    mut faint: ResMut<FaintFx>,
    mut player: Query<(&mut Body, &mut Transform), With<Player>>,
) {
    let dt = time.delta_secs();
    let me = &sim.world.player;
    if let Some(dark) = faint.dark.as_mut() {
        *dark += dt;
        if !faint.moved
            && let Some(cabin) = stations.cabin
            && let Ok((mut body, mut transform)) = player.single_mut()
        {
            let x = TrainLayout::carriage_left(cabin.carriage.index()) + cabin.bed_x + 12.0;
            let at = Vec2::new(x, cabin.base_y() + player_half_height());
            body.teleport(at);
            transform.translation.x = at.x;
            transform.translation.y = at.y;
            faint.moved = true;
        }
        if faint.dark.is_some_and(|d| d >= FADE_OUT_SECS) && !me.is_asleep() {
            faint.dark = None;
            faint.waking = Some(0.0);
            let lost = if faint.lost.is_empty() {
                String::new()
            } else {
                format!(" Hai perso {}.", faint.lost)
            };
            faint.message = Some((
                format!(
                    "Ti risvegli nella tua cabina, dolorante (salute {:.0}).{lost}",
                    me.health
                ),
                time.elapsed_secs_f64(),
            ));
        }
    } else if let Some(waking) = faint.waking.as_mut() {
        *waking += dt;
        if *waking >= FADE_IN_SECS {
            faint.waking = None;
        }
    }
    if faint
        .message
        .as_ref()
        .is_some_and(|(_, at)| time.elapsed_secs_f64() - at > WAKE_MESSAGE_SECS)
    {
        faint.message = None;
    }
}

/// Il messaggio del tasto X (conferma o esito), in basso al centro.
fn attack_message(
    mut contexts: EguiContexts,
    time: Res<Time<Real>>,
    mut state: ResMut<AttackState>,
) {
    let now = time.elapsed_secs_f64();
    if state
        .message
        .as_ref()
        .is_some_and(|(_, at, _)| now - at > MESSAGE_SECS)
    {
        state.message = None;
    }
    let Some((text, _, color)) = state.message.clone() else {
        return;
    };
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    egui::Area::new(egui::Id::new("attack_message"))
        .anchor(Align2::CENTER_BOTTOM, [0.0, -110.0])
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.label(RichText::new(text).color(color).strong());
            });
        });
}

/// Buio dello svenimento, luce al risveglio, messaggio; "Sei morto" con la
/// morte permanente.
fn faint_overlay(
    mut contexts: EguiContexts,
    sim: Res<Sim>,
    faint: Res<FaintFx>,
    mut queue: ResMut<SaveQueue>,
    mut saves: ResMut<SavesWindow>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let screen = ctx.content_rect();
    let shade = |ctx: &egui::Context, alpha: f32| {
        egui::Area::new(egui::Id::new("faint_shade"))
            .fixed_pos(screen.min)
            .order(egui::Order::Background)
            .interactable(false)
            .show(ctx, |ui| {
                ui.painter().rect_filled(
                    screen,
                    0.0,
                    Color32::from_black_alpha((alpha.clamp(0.0, 1.0) * 255.0) as u8),
                );
            });
    };
    if sim.world.player.dead.is_some() {
        shade(ctx, 0.85);
        egui::Area::new(egui::Id::new("game_over"))
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(280.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new("Sei morto")
                                .size(26.0)
                                .strong()
                                .color(Color32::from_rgb(230, 70, 70)),
                        );
                        ui.label("Morte permanente: la partita è finita.");
                        ui.horizontal(|ui| {
                            if ui.button("Carica l'ultimo salvataggio").clicked() {
                                queue.0.push(SaveCommand::QuickLoad);
                            }
                            if ui.button("Nuova partita…").clicked() {
                                saves.open = true;
                            }
                        });
                    });
                });
            });
        return;
    }
    if let Some(dark) = faint.dark {
        // Il buio arriva piano; poi resta, sotto il pannello del sonno.
        shade(ctx, dark / FADE_OUT_SECS);
    } else if let Some(waking) = faint.waking {
        shade(ctx, 1.0 - waking / FADE_IN_SECS);
    }
    if let Some((text, _)) = &faint.message {
        egui::Area::new(egui::Id::new("faint_message"))
            .anchor(Align2::CENTER_TOP, [0.0, 80.0])
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(RichText::new(text).color(Color32::from_rgb(255, 200, 120)));
                });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_nearest_npc_in_reach_is_hit() {
        let npcs = [
            (NpcId(1), Vec2::new(10.0, 0.0)),
            (NpcId(2), Vec2::new(-4.0, 2.0)),
            (NpcId(3), Vec2::new(3.0, 40.0)),
            (NpcId(4), Vec2::new(30.0, 0.0)),
        ];
        assert_eq!(nearest_npc(Vec2::ZERO, npcs.into_iter()), Some(NpcId(2)));
        assert_eq!(nearest_npc(Vec2::new(100.0, 0.0), npcs.into_iter()), None);
    }

    #[test]
    fn only_the_first_blow_at_a_peaceful_npc_needs_confirming() {
        let mut world = World::generate(3, 10, 60);
        let i = world
            .npcs
            .iter()
            .position(|n| n.age >= 20 && n.is_awake())
            .unwrap();
        let id = world.npcs[i].id;
        assert!(needs_confirm(&world, id));
        let place = sim::Place {
            carriage: world.npcs[i].carriage,
            floor: world.npcs[i].floor,
        };
        world.set_player_place(place);
        world.player_attack(id).unwrap();
        // Ora ce l'ha con il giocatore o ci sta combattendo.
        assert!(!needs_confirm(&world, id));
    }

    #[test]
    fn a_fight_shouts_by_motive_and_reaction() {
        let mut world = World::generate(3, 10, 60);
        let i = world
            .npcs
            .iter()
            .position(|n| n.age >= 20 && n.is_awake())
            .unwrap();
        let id = world.npcs[i].id;
        let place = sim::Place {
            carriage: world.npcs[i].carriage,
            floor: world.npcs[i].floor,
        };
        world.set_player_place(place);
        world.player_attack(id).unwrap();
        let first = world
            .events
            .iter()
            .rev()
            .find(|e| matches!(e.kind, EventKind::Attacked { first: true, .. }))
            .unwrap();
        let shouts = fight_shouts(&world, first);
        // Il giocatore non grida; la vittima sì, per come ha reagito.
        assert_eq!(shouts.len(), 1);
        assert_eq!(shouts[0].0, id);
        assert!(["Vieni avanti!", "Aiuto!", "Lasciami stare!"].contains(&shouts[0].1));
    }
}
