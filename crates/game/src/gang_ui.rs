//! Le bande nel gioco (vedi `sim::gang`).
//!
//! - **Finestra "Bande del treno" (tasto J):** in testa la tua posizione
//!   (la banda in cui sei, l'incarico, l'invito, il pizzo che devi, con
//!   "Paga", "Rifiuta" e "Lascia la banda"), poi ogni banda con il suo
//!   colore: capo (un link che lo seleziona), membri con ruolo e lealtà,
//!   territorio (le carrozze contese sono segnate), tesoro, fama, rivali e
//!   alleati, cosa pensa di te (una barra da ostile ad amica) e gli ultimi
//!   fatti.
//! - **Ispettore:** la banda dell'NPC con il colore, il ruolo (capo,
//!   membro) e la lealtà ([`inspector_section`]).
//! - **Fasce:** una fascia di 2 pixel del colore della banda sul braccio di
//!   ogni membro in piedi ([`armbands`]).
//! - **Chat:** l'invito di un amico ("!"), il pizzo da riscuotere per la
//!   tua banda e quello che ti chiedono (`chat.rs` usa [`GangCommand`]).
//!
//! Tutto passa dalla sim (`World::player_*_gang`, `player_collect_pizzo`,
//! `player_pay_pizzo`…) al minuto corrente, come gli altri `player_*`.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, RichText};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use sim::{Gang, GangId, Npc, NpcId, World};

use crate::npc_render::{NpcRenderSet, NpcVisual};
use crate::saves::WorldRebuildSet;
use crate::sim_bridge::SimTickSet;
use crate::state::{NpcSprite, SelectedNpc, Sim, WorldReplaced};
use crate::ui::PointerCheck;

const WINDOW_WIDTH: f32 = 430.0;
/// Fatti recenti mostrati per banda.
const ACTS_SHOWN: usize = 5;
/// Fascia sul braccio: larghezza (frazione del corpo), altezza, quota sopra
/// il centro del corpo (frazione dell'altezza), profondità sopra lo sprite.
const BAND_WIDTH: f32 = 0.4;
const BAND_HEIGHT: f32 = 2.0;
const BAND_RISE: f32 = 0.1;
const BAND_Z: f32 = 0.0002;
/// Bordo scuro della fascia, sopra e sotto.
const BAND_EDGE: Color = Color::srgba(0.08, 0.06, 0.06, 0.9);
/// Quanto resta l'esito di un comando (secondi reali).
const NOTE_SECS: f64 = 5.0;
const HOSTILE: Color32 = Color32::from_rgb(230, 110, 100);
const FRIENDLY: Color32 = Color32::from_rgb(130, 210, 130);
const NEUTRAL: Color32 = Color32::from_rgb(220, 200, 120);
const DUE: Color32 = Color32::from_rgb(240, 210, 110);

pub struct GangUiPlugin;

impl Plugin for GangUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GangWindow>()
            .init_resource::<GangQueue>()
            .init_resource::<Armbands>()
            .add_systems(
                PreUpdate,
                reset_gangs
                    .in_set(WorldRebuildSet)
                    .run_if(on_message::<WorldReplaced>),
            )
            .add_systems(
                Update,
                (toggle_window, apply_gang_commands.before(SimTickSet)),
            )
            .add_systems(Update, armbands.after(NpcRenderSet).after(SimTickSet))
            .add_systems(EguiPrimaryContextPass, gang_window.before(PointerCheck));
    }
}

/// La finestra delle bande è aperta (tasto J).
#[derive(Resource, Default)]
pub(crate) struct GangWindow {
    pub(crate) open: bool,
}

/// Cosa fare con le bande: le finestre lo chiedono, [`apply_gang_commands`]
/// lo esegue sulla sim.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum GangCommand {
    /// Accetta l'invito del membro.
    Join(NpcId),
    /// Rifiuta l'invito del membro.
    Refuse(NpcId),
    Leave,
    /// Riscuote il pizzo dell'incarico da questo NPC.
    Collect(NpcId),
    /// Paga il pizzo dovuto alla banda.
    Pay(GangId),
    /// Rifiuta di pagarlo.
    RefusePizzo(GangId),
}

/// Comandi in attesa e l'esito dell'ultimo (testo, quando).
#[derive(Resource, Default)]
pub(crate) struct GangQueue {
    pub(crate) commands: Vec<GangCommand>,
    pub(crate) note: Option<(String, f64)>,
}

impl GangQueue {
    /// L'esito dell'ultimo comando, se è recente.
    pub(crate) fn note(&self, now: f64) -> Option<&str> {
        self.note
            .as_ref()
            .filter(|(_, at)| now - at < NOTE_SECS)
            .map(|(t, _)| t.as_str())
    }
}

/// Fasce sul braccio: NPC -> entità.
#[derive(Resource, Default)]
struct Armbands(HashMap<NpcId, Entity>);

#[derive(Component)]
struct Armband;

/// Il colore della fascia (figlio di [`Armband`], che fa da bordo).
#[derive(Component)]
struct ArmbandFill;

// --- Dati puri ------------------------------------------------------------------

/// Il colore di una banda.
pub(crate) fn gang_color(g: &Gang) -> Color32 {
    let [r, gr, b] = g.rgb();
    Color32::from_rgb(r, gr, b)
}

/// "ostile", "diffidente", "indifferente", "ben disposta", "amica", e il colore.
pub(crate) fn attitude_label(world: &World, gang: GangId) -> (&'static str, Color32) {
    let a = world.gang_attitude(gang);
    if world.is_gang_hostile(gang) {
        ("ostile", HOSTILE)
    } else if a < -0.1 {
        ("diffidente", HOSTILE)
    } else if a < 0.2 {
        ("indifferente", NEUTRAL)
    } else if a < 0.5 {
        ("ben disposta", FRIENDLY)
    } else {
        ("amica", FRIENDLY)
    }
}

/// Esegue un comando; restituisce il messaggio da mostrare.
pub(crate) fn run_gang_command(world: &mut World, command: &GangCommand) -> String {
    let gang_name = |w: &World, g: GangId| w.gang(g).map_or(String::new(), |g| g.name.clone());
    match *command {
        GangCommand::Join(npc) => match world.player_join_gang(npc) {
            Ok(g) => format!("Sei entrato nella banda «{}».", gang_name(world, g)),
            Err(e) => format!("Non puoi entrare: {e}."),
        },
        GangCommand::Refuse(npc) => match world.player_refuse_gang(npc) {
            Ok(()) => "Hai rifiutato l'invito.".to_string(),
            Err(e) => format!("{e}."),
        },
        GangCommand::Leave => {
            let name = world.player_gang().map(|g| g.name.clone());
            match world.player_leave_gang() {
                Ok(()) => format!(
                    "Hai lasciato «{}»: non l'hanno presa bene.",
                    name.unwrap_or_default()
                ),
                Err(e) => format!("{e}."),
            }
        }
        GangCommand::Collect(npc) => {
            let who = world
                .npc(npc)
                .map_or(String::new(), |n| n.first_name().to_string());
            match world.player_collect_pizzo(npc) {
                Ok(out) => format!(
                    "{who} ha pagato {} gettoni: {} sono tuoi, il resto va alla banda.",
                    out.paid, out.share
                ),
                Err(e) => format!("{who}: {e}."),
            }
        }
        GangCommand::Pay(g) => match world.player_pay_pizzo(g) {
            Ok(tokens) => format!("Hai pagato {tokens} gettoni a «{}».", gang_name(world, g)),
            Err(e) => format!("{e}."),
        },
        GangCommand::RefusePizzo(g) => match world.player_refuse_pizzo(g) {
            Ok(()) => format!(
                "Hai rifiutato il pizzo a «{}»: stai attento.",
                gang_name(world, g)
            ),
            Err(e) => format!("{e}."),
        },
    }
}

// --- Sistemi --------------------------------------------------------------------

fn reset_gangs(mut commands: Commands, mut queue: ResMut<GangQueue>, mut bands: ResMut<Armbands>) {
    *queue = GangQueue::default();
    for (_, entity) in bands.0.drain() {
        commands.entity(entity).despawn();
    }
}

fn toggle_window(keys: Res<ButtonInput<KeyCode>>, mut window: ResMut<GangWindow>) {
    if keys.just_pressed(KeyCode::KeyJ) {
        window.open = !window.open;
    }
}

fn apply_gang_commands(time: Res<Time<Real>>, mut sim: ResMut<Sim>, mut queue: ResMut<GangQueue>) {
    if queue.commands.is_empty() {
        return;
    }
    let now = time.elapsed_secs_f64();
    for command in std::mem::take(&mut queue.commands) {
        let note = run_gang_command(&mut sim.world, &command);
        queue.note = Some((note, now));
    }
}

/// Una fascia del colore della banda sul braccio dei membri disegnati e in
/// piedi, bordata di scuro sopra e sotto (si vede anche su vestiti dello
/// stesso colore).
#[allow(clippy::type_complexity)]
fn armbands(
    mut commands: Commands,
    sim: Res<Sim>,
    mut bands: ResMut<Armbands>,
    sprites: Query<(&NpcSprite, &Transform, &NpcVisual)>,
    mut placed: Query<
        (&mut Transform, &mut Sprite, &Children),
        (With<Armband>, Without<NpcSprite>),
    >,
    mut fills: Query<&mut Sprite, (With<ArmbandFill>, Without<Armband>)>,
) {
    let world = &sim.world;
    let mut wanted: HashMap<NpcId, (Vec3, Vec2, Color)> = HashMap::new();
    if !world.gangs().is_empty() {
        for (npc, transform, visual) in &sprites {
            let Some(g) = world.gang_of(npc.0) else {
                continue;
            };
            if !visual.upright() {
                continue;
            }
            let height = visual.body_height();
            let width = (height / 2.0 * BAND_WIDTH).round().max(2.0);
            let at = transform.translation + Vec3::new(0.0, (height * BAND_RISE).round(), BAND_Z);
            let [r, gr, b] = g.rgb();
            wanted.insert(
                npc.0,
                (at, Vec2::new(width, BAND_HEIGHT), Color::srgb_u8(r, gr, b)),
            );
        }
    }
    bands.0.retain(|id, entity| {
        let keep = wanted.contains_key(id);
        if !keep {
            commands.entity(*entity).despawn();
        }
        keep
    });
    let edge = |size: Vec2| size + Vec2::new(0.0, 2.0);
    for (id, (at, size, color)) in wanted {
        match bands.0.get(&id) {
            Some(&entity) => {
                if let Ok((mut transform, mut outline, children)) = placed.get_mut(entity) {
                    transform.translation = at;
                    outline.custom_size = Some(edge(size));
                    for &child in children {
                        if let Ok(mut fill) = fills.get_mut(child) {
                            fill.color = color;
                            fill.custom_size = Some(size);
                        }
                    }
                }
            }
            None => {
                let entity = commands
                    .spawn((
                        Name::new("Fascia della banda"),
                        Armband,
                        Sprite {
                            color: BAND_EDGE,
                            custom_size: Some(edge(size)),
                            ..default()
                        },
                        Transform::from_translation(at),
                    ))
                    .with_child((
                        ArmbandFill,
                        Sprite {
                            color,
                            custom_size: Some(size),
                            ..default()
                        },
                        Transform::from_xyz(0.0, 0.0, BAND_Z / 4.0),
                    ))
                    .id();
                bands.0.insert(id, entity);
            }
        }
    }
}

// --- Finestra -------------------------------------------------------------------

/// Un quadratino del colore della banda.
pub(crate) fn swatch(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 2.0, color);
}

/// Un link a un NPC (lo seleziona); restituisce l'NPC cliccato.
fn npc_link(ui: &mut egui::Ui, world: &World, id: NpcId) -> Option<NpcId> {
    match world.npc(id) {
        Some(n) => ui.link(&n.name).clicked().then_some(id),
        None => {
            ui.weak("qualcuno che non c'è più");
            None
        }
    }
}

fn gang_window(
    mut contexts: EguiContexts,
    time: Res<Time<Real>>,
    sim: Option<Res<Sim>>,
    mut window: ResMut<GangWindow>,
    mut queue: ResMut<GangQueue>,
    mut selected: ResMut<SelectedNpc>,
) {
    if !window.open {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let mut open = true;
    let mut clicked = None;
    let mut commands = Vec::new();
    let center = ctx.content_rect().center();
    let max_height = (ctx.content_rect().height() - 120.0).max(200.0);
    egui::Window::new("Bande del treno")
        .open(&mut open)
        .default_pos(center - egui::vec2(WINDOW_WIDTH / 2.0, 260.0))
        .default_width(WINDOW_WIDTH)
        .resizable(false)
        .show(ctx, |ui| {
            ui.set_width(WINDOW_WIDTH);
            let Some(sim) = &sim else {
                ui.weak("simulazione non avviata");
                return;
            };
            let world = &sim.world;
            summary(ui, world);
            ui.separator();
            clicked = player_section(ui, world, &mut commands).or(clicked);
            if let Some(note) = queue.note(time.elapsed_secs_f64()) {
                ui.label(RichText::new(note).color(DUE));
            }
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(max_height)
                .show(ui, |ui| {
                    if world.gangs().is_empty() {
                        ui.weak(
                            "Nessuna banda sul treno, per ora. Nascono da sole tra amici \
                             poveri, arrabbiati e violenti che vivono o lavorano vicini.",
                        );
                    }
                    for (k, g) in world.gangs().iter().enumerate() {
                        if let Some(id) = gang_section(ui, world, g, k < 2) {
                            clicked = Some(id);
                        }
                    }
                });
        });
    if !open {
        window.open = false;
    }
    if let Some(id) = clicked {
        selected.0 = Some(id);
    }
    queue.commands.extend(commands);
}

/// I numeri di tutte le bande.
fn summary(ui: &mut egui::Ui, world: &World) {
    let c = &world.gang_state().counters;
    let members: usize = world.gangs().iter().map(Gang::size).sum();
    ui.label(format!(
        "{} bande, {members} membri · nate {} · sciolte {}",
        world.gangs().len(),
        c.founded,
        c.disbanded + c.merges
    ));
    ui.weak(format!(
        "Pizzo riscosso {} volte ({} gettoni), rifiutato {} · risse tra bande {} · \
         regolamenti di conti {} (ordinati {}) · uccisi {}",
        c.pizzo_paid,
        c.pizzo_tokens,
        c.pizzo_refused,
        c.rival_fights,
        c.hits_done,
        c.hits_ordered,
        c.kills
    ));
}

/// La tua posizione: banda, incarico, invito, pizzo dovuto. Restituisce
/// l'NPC cliccato.
fn player_section(
    ui: &mut egui::Ui,
    world: &World,
    commands: &mut Vec<GangCommand>,
) -> Option<NpcId> {
    let mut clicked = None;
    let p = &world.gang_state().player;
    match world.player_gang() {
        Some(g) => {
            ui.horizontal(|ui| {
                swatch(ui, gang_color(g));
                ui.label(RichText::new(format!("Sei nella banda «{}»", g.name)).strong());
                if let Some(since) = p.since {
                    ui.weak(format!("dal giorno {}", since.day()));
                }
            });
            ui.weak(format!(
                "Ti proteggono, ti danno una parte del tesoro ogni notte. Incarichi fatti: {} ({} gettoni riscossi).",
                p.tasks_done, p.collected
            ));
            if let Some(t) = world.gang_task() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Incarico: riscuotere").color(DUE));
                    ui.label(RichText::new(format!("{} gettoni da", t.tokens)).color(DUE));
                    if let Some(id) = npc_link(ui, world, t.victim) {
                        clicked = Some(id);
                    }
                    ui.label(
                        RichText::new(format!("entro il giorno {} (parlagli: T)", t.until.day()))
                            .color(DUE),
                    );
                });
            }
            if ui
                .button("Lascia la banda")
                .on_hover_text("I membri se la legheranno al dito.")
                .clicked()
            {
                commands.push(GangCommand::Leave);
            }
        }
        None => {
            ui.weak("Non sei in nessuna banda.");
        }
    }
    if let Some(inv) = world.gang_invite()
        && let Some(g) = world.gang(inv.gang)
    {
        ui.horizontal_wrapped(|ui| {
            swatch(ui, gang_color(g));
            if let Some(id) = npc_link(ui, world, inv.by) {
                clicked = Some(id);
            }
            ui.label(format!(
                "ti invita in «{}» (fino al giorno {}): parlagli.",
                g.name,
                inv.until.day()
            ));
        });
    }
    for (gang, due, asked) in world.gang_pizzo_due() {
        let Some(g) = world.gang(gang) else {
            continue;
        };
        ui.horizontal_wrapped(|ui| {
            swatch(ui, gang_color(g));
            let text = if asked {
                format!("Devi {due} gettoni di pizzo a «{}»", g.name)
            } else {
                format!("«{}» vorrà {due} gettoni sulle tue vendite", g.name)
            };
            ui.label(RichText::new(text).color(DUE));
            let pay = ui.add_enabled(world.player.tokens >= due, egui::Button::new("Paga"));
            if pay.clicked() {
                commands.push(GangCommand::Pay(gang));
            }
            if ui
                .button("Rifiuta")
                .on_hover_text("La banda non la prenderà bene.")
                .clicked()
            {
                commands.push(GangCommand::RefusePizzo(gang));
            }
        });
    }
    clicked
}

/// Una banda; restituisce l'NPC cliccato.
fn gang_section(ui: &mut egui::Ui, world: &World, g: &Gang, open: bool) -> Option<NpcId> {
    let mut clicked = None;
    let color = gang_color(g);
    let title = RichText::new(format!("■ {}", g.name)).color(color).strong();
    egui::CollapsingHeader::new(title)
        .id_salt(("gang", g.id.0))
        .default_open(open)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.weak("Capo:");
                if let Some(id) = npc_link(ui, world, g.leader) {
                    clicked = Some(id);
                }
                if g.leaderless {
                    ui.colored_label(HOSTILE, "(senza capo)");
                }
                ui.weak(format!("· dal giorno {}", g.founded.day()));
            });
            ui.horizontal(|ui| {
                ui.weak("Tesoro:");
                ui.label(format!("{} gettoni", g.treasury));
                ui.weak("· Fama:");
                ui.label(g.reputation().label())
                    .on_hover_text(format!("Paura {:.0}%", g.fear * 100.0));
            });
            // Territorio.
            let turf: Vec<String> = g
                .territory
                .iter()
                .map(|&c| {
                    let others: Vec<String> = world
                        .gangs_holding(c)
                        .into_iter()
                        .filter(|o| o.id != g.id)
                        .map(|o| format!("«{}»", o.name))
                        .collect();
                    let name = world.carriage(c).map_or(c.to_string(), |c| c.name.clone());
                    if others.is_empty() {
                        format!("{name} ({c})")
                    } else {
                        format!("{name} ({c}, contesa con {})", others.join(", "))
                    }
                })
                .collect();
            ui.horizontal_wrapped(|ui| {
                ui.weak("Territorio:");
                ui.label(if turf.is_empty() {
                    "nessuno".to_string()
                } else {
                    turf.join("; ")
                });
            });
            // Rivali e alleati.
            let named = |ties: Vec<GangId>| -> String {
                ties.iter()
                    .filter_map(|&t| world.gang(t))
                    .map(|o| format!("«{}»", o.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let rivals = named(
                g.ties
                    .iter()
                    .filter(|t| t.stance < sim::RIVAL_BELOW)
                    .map(|t| t.other)
                    .collect(),
            );
            let allies = named(
                g.ties
                    .iter()
                    .filter(|t| t.stance >= sim::ALLY_FROM)
                    .map(|t| t.other)
                    .collect(),
            );
            if !rivals.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.weak("Rivali:");
                    ui.colored_label(HOSTILE, rivals);
                });
            }
            if !allies.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.weak("Alleati:");
                    ui.colored_label(FRIENDLY, allies);
                });
            }
            // Cosa pensa di te.
            let attitude = world.gang_attitude(g.id);
            let (label, tint) = attitude_label(world, g.id);
            ui.horizontal(|ui| {
                ui.weak("Verso di te:");
                ui.colored_label(tint, label);
                ui.add(
                    egui::ProgressBar::new((attitude + 1.0) / 2.0)
                        .desired_width(120.0)
                        .desired_height(8.0)
                        .fill(tint),
                )
                .on_hover_text(format!("{attitude:+.2} (da −1 a +1)"));
            });
            ui.weak(format!(
                "Pizzo {} ({} gettoni) · risse {} · uccisi {} · colpi ordinati {}/{}",
                g.tally.pizzo_paid,
                g.tally.pizzo_tokens,
                g.tally.fights,
                g.tally.kills,
                g.tally.hits_done,
                g.tally.hits_ordered
            ));
            // Membri.
            egui::CollapsingHeader::new(format!("Membri ({})", g.size()))
                .id_salt(("gang_members", g.id.0))
                .default_open(false)
                .show(ui, |ui| {
                    for m in &g.members {
                        ui.horizontal(|ui| {
                            if let Some(id) = npc_link(ui, world, m.id) {
                                clicked = Some(id);
                            }
                            let role = g.role(m.id).map_or("membro", |r| r.label());
                            ui.weak(format!("{role} · lealtà {:.0}%", m.loyalty * 100.0));
                        });
                    }
                    if world.gang_state().player.gang == Some(g.id) {
                        ui.label(RichText::new("e tu").color(FRIENDLY));
                    }
                });
            // Ultimi fatti.
            for act in g.acts.iter().rev().take(ACTS_SHOWN) {
                ui.label(
                    RichText::new(format!(
                        "g{} {:02}:{:02} · {}",
                        act.time.day(),
                        act.time.hour(),
                        act.time.minute(),
                        act.text
                    ))
                    .small()
                    .weak(),
                );
            }
        });
    clicked
}

/// Sezione "Banda" dell'ispettore: colore, nome, ruolo, lealtà.
pub(crate) fn inspector_section(ui: &mut egui::Ui, world: &World, npc: &Npc) {
    let Some(g) = world.gang_of(npc.id) else {
        return;
    };
    ui.separator();
    ui.horizontal(|ui| {
        swatch(ui, gang_color(g));
        ui.label(RichText::new(format!("Banda «{}»", g.name)).strong());
    });
    let role = g.role(npc.id).map_or("membro", |r| r.label());
    let loyalty = g.member(npc.id).map_or(0.0, |m| m.loyalty);
    ui.horizontal(|ui| {
        ui.weak("Ruolo:");
        ui.label(role);
        ui.weak("· tesoro della banda");
        ui.label(format!("{} gettoni", g.treasury));
    });
    ui.add(
        egui::ProgressBar::new(loyalty)
            .fill(gang_color(g))
            .text(format!("Lealtà {:.0}%", loyalty * 100.0)),
    );
    let (label, tint) = attitude_label(world, g.id);
    ui.horizontal(|ui| {
        ui.weak("La banda verso di te:");
        ui.colored_label(tint, label);
    });
}

#[cfg(test)]
mod tests {
    use sim::{PlayerTie, SimParams};

    use super::*;

    /// A world with a gang of three adults (the second one a friend of the
    /// player), and the gang's id.
    fn world_with_gang() -> (World, GangId, Vec<NpcId>) {
        let params = SimParams {
            deliberation_rate: 0.0,
            ..SimParams::default()
        };
        let mut world = World::generate_with_params(9, 10, 80, params);
        let ids: Vec<NpcId> = world
            .npcs
            .iter()
            .filter(|n| (20..=50).contains(&n.age))
            .take(3)
            .map(|n| n.id)
            .collect();
        let gang = world.found_gang(&ids).expect("a gang");
        let k = world.npcs.iter().position(|n| n.id == ids[1]).unwrap();
        world.npcs[k].player = Some(PlayerTie {
            affinity: 0.9,
            ..PlayerTie::default()
        });
        (world, gang, ids)
    }

    #[test]
    fn colours_and_attitudes() {
        let (world, gang, _) = world_with_gang();
        let g = world.gang(gang).unwrap();
        let [r, gr, b] = g.rgb();
        assert_eq!(gang_color(g), Color32::from_rgb(r, gr, b));
        let (label, _) = attitude_label(&world, gang);
        assert!(["ben disposta", "amica"].contains(&label), "{label}");
    }

    #[test]
    fn commands_report_what_happened() {
        let (mut world, gang, ids) = world_with_gang();
        let note = run_gang_command(&mut world, &GangCommand::Join(ids[1]));
        assert!(note.contains("Non puoi entrare"), "{note}");
        let note = run_gang_command(&mut world, &GangCommand::Pay(gang));
        assert!(note.contains("non devi niente"), "{note}");
        let note = run_gang_command(&mut world, &GangCommand::Leave);
        assert!(note.contains("non sei in una banda"), "{note}");
    }
}
