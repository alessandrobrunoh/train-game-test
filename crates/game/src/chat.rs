//! Chat con gli NPC (Fase 5.4 di `docs/piano-vita-ed-economia.md`).
//!
//! **T** vicino a un NPC sveglio (o **E**, se non c'è niente da regalargli;
//! vedi `interaction.rs`) apre la finestra di chat: ritratto, nome, lavoro,
//! "Ti considera: …" con l'affinità, l'incarico in corso, lo storico dei
//! messaggi (la memoria della sim, `World::chat_log`), le risposte suggerite
//! (`sim::Intent`) e un campo di testo libero, letto da
//! `sim::KeywordReader` (Invio invia, Esc chiude).
//!
//! **Il tempo si ferma** mentre si parla: aprendo la chat la sim va in pausa
//! (come con P) e alla chiusura riparte, se l'aveva fermata la chat. P e
//! 1-5 restano attivi, quindi chi vuole può far scorrere il tempo anche
//! chiacchierando. Il giocatore resta fermo finché la chat è aperta.
//!
//! Tutto passa dalla sim (`World::player_chat*`) al minuto corrente, come
//! gli altri `player_*`: la chat non cambia il determinismo. Le battute
//! compaiono anche come fumetti sopra l'NPC e sopra il giocatore
//! (`speech::ChatSays`). "Regala" apre la scelta del regalo nella finestra,
//! "Scambia" con un mercante al banco apre la finestra del Mercato.
//!
//! Le bande (`gang_ui.rs`): un amico che è in una banda può invitarti (il
//! "!"; "Entra nella banda" / "Rifiuta"), la vittima di un incarico della tua
//! banda ha "Riscuoti il pizzo", e un membro che ti chiede il pizzo sulle
//! tue vendite "Paga" / "Rifiuta".

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, RichText};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use sim::chat::MAX_CHAT_INPUT_CHARS;
use sim::{
    Band, ChatAction, ChatReply, Intent, ItemKind, KeywordReader, Npc, NpcId, Regard, Speaker,
    World,
};

use crate::art::Canvas;
use crate::characters::{Frame, appearance, frame_canvas};
use crate::gang_ui::{GangCommand, GangQueue, gang_color, swatch};
use crate::market_ui::MarketWindow;
use crate::saves::WorldRebuildSet;
use crate::sim_bridge::SimTickSet;
use crate::speech::{ChatSays, Sayer};
use crate::state::{Sim, SimClock, WorldReplaced};
use crate::ui::PointerCheck;

/// Lato di un pixel del ritratto (pixel logici).
const PORTRAIT_SCALE: f32 = 3.0;
const WINDOW_WIDTH: f32 = 400.0;
const HISTORY_HEIGHT: f32 = 230.0;
const NPC_INK: Color32 = Color32::from_rgb(235, 225, 200);
const PLAYER_INK: Color32 = Color32::from_rgb(140, 200, 245);
const NPC_PAPER: Color32 = Color32::from_rgb(52, 48, 44);
const PLAYER_PAPER: Color32 = Color32::from_rgb(34, 50, 66);
const FAVOUR_INK: Color32 = Color32::from_rgb(240, 210, 110);

/// La chat aperta (con chi) e il suo stato di interfaccia.
#[derive(Resource, Default)]
pub(crate) struct ChatWindow {
    /// Con chi si parla (None: chiusa).
    pub(crate) npc: Option<NpcId>,
    /// Testo che si sta scrivendo.
    input: String,
    /// Mostra la scelta del regalo.
    gift: bool,
    /// Ultimo avviso (regalo rifiutato, NPC andato via…).
    note: Option<String>,
    /// La pausa l'ha messa la chat: alla chiusura il tempo riparte.
    paused_by_chat: bool,
    /// Porta il cursore nel campo di testo al prossimo disegno.
    focus: bool,
    /// Ritratto dell'NPC (calcolato all'apertura).
    portrait: Option<Canvas>,
}

impl ChatWindow {
    pub(crate) fn is_open(&self) -> bool {
        self.npc.is_some()
    }
}

/// Cosa fare con la chat: la finestra (o `interaction.rs`) lo chiede, il
/// sistema [`apply_chat`] lo esegue sulla sim.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ChatCommand {
    Open(NpcId),
    Say(Intent),
    Type(String),
    Give(ItemKind),
    Close,
}

/// Comandi in attesa, eseguiti nel prossimo `Update`.
#[derive(Resource, Default)]
pub(crate) struct ChatQueue(pub(crate) Vec<ChatCommand>);

pub struct ChatPlugin;

impl Plugin for ChatPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChatWindow>()
            .init_resource::<ChatQueue>()
            .init_resource::<ChatSays>()
            .init_resource::<MarketWindow>()
            .init_resource::<GangQueue>()
            .add_systems(
                PreUpdate,
                reset_chat
                    .in_set(WorldRebuildSet)
                    .run_if(on_message::<WorldReplaced>),
            )
            // Esc chiude la chat (prima che apra la finestra "Partite").
            .add_systems(PreUpdate, chat_escape.after(bevy::input::InputSystems))
            .add_systems(
                Update,
                (apply_chat, close_when_gone).chain().before(SimTickSet),
            )
            .add_systems(EguiPrimaryContextPass, chat_window.before(PointerCheck));
    }
}

// --- Dati puri -------------------------------------------------------------------

/// "contadina · Serra 3", "pensionato", "bambina".
fn role_line(world: &World, npc: &Npc) -> String {
    match (npc.job, npc.workplace) {
        (Some(job), Some(place)) => {
            let place = world
                .carriage(place)
                .map_or(String::new(), |c| c.name.clone());
            format!("{} · {place}", job.name())
        }
        _ => npc.stage().name(npc.sex).to_string(),
    }
}

/// "Ti considera: amica" (con chi non ti conosce: "Non ti conosce").
fn regard_line(npc: &Npc) -> String {
    match npc.regard() {
        Regard::Stranger => "Non ti conosce".to_string(),
        r => format!("Ti considera: {}", r.label(npc.sex)),
    }
}

/// Testo del pulsante di una risposta suggerita: "Chiedi un favore"
/// diventa "Consegna" quando il giocatore ha ciò che l'incarico chiede.
fn intent_label(world: &World, npc: NpcId, intent: Intent) -> String {
    if intent == Intent::AskFavour
        && let Some(f) = world.player_favour(npc)
        && f.told
        && world.player.inventory.has(f.item, f.count)
    {
        return "Consegna l'incarico".to_string();
    }
    intent.label().to_string()
}

/// Esegue un comando della chat; restituisce la risposta della sim, se ce n'è una.
fn run_command(
    world: &mut World,
    clock: &mut SimClock,
    window: &mut ChatWindow,
    market: &mut MarketWindow,
    says: &mut ChatSays,
    command: ChatCommand,
) -> Option<ChatReply> {
    let open = |window: &mut ChatWindow, clock: &mut SimClock| {
        if !clock.paused {
            clock.paused = true;
            window.paused_by_chat = true;
        }
    };
    match command {
        ChatCommand::Open(id) => {
            if let Err(e) = world.can_chat(id) {
                window.note = world.npc(id).map(|n| format!("{} {e}", n.first_name()));
                return None;
            }
            let was_open = window.is_open();
            if window.npc != Some(id) {
                window.input.clear();
                window.gift = false;
            }
            window.npc = Some(id);
            window.note = None;
            window.focus = true;
            window.portrait = world
                .npc(id)
                .map(|n| frame_canvas(&appearance(n), Frame::Idle0));
            if !was_open {
                open(window, clock);
            }
            if let Ok(Some(line)) = world.player_chat_start(id) {
                says.say(Sayer::Npc(id), line);
            }
            None
        }
        ChatCommand::Close => {
            close(window, clock);
            None
        }
        ChatCommand::Say(intent) => {
            let id = window.npc?;
            let reply = world.player_chat(id, intent);
            after_reply(world, window, market, says, id, reply)
        }
        ChatCommand::Type(text) => {
            let id = window.npc?;
            if text.trim().is_empty() {
                return None;
            }
            let reply = world.player_chat_text(id, &text, &KeywordReader);
            after_reply(world, window, market, says, id, reply)
        }
        ChatCommand::Give(item) => {
            let id = window.npc?;
            window.gift = false;
            match world.player_chat_gift(id, item) {
                Ok(reply) => {
                    says.say(Sayer::Player, reply.said.clone());
                    says.say(Sayer::Npc(id), reply.answer.clone());
                    window.note = None;
                    Some(reply)
                }
                Err(e) => {
                    let name = world.npc(id).map_or("", |n| n.first_name());
                    window.note = Some(format!("{name}: {e}"));
                    None
                }
            }
        }
    }
}

fn after_reply(
    world: &World,
    window: &mut ChatWindow,
    market: &mut MarketWindow,
    says: &mut ChatSays,
    id: NpcId,
    reply: Result<ChatReply, sim::ChatError>,
) -> Option<ChatReply> {
    let reply = match reply {
        Ok(reply) => reply,
        Err(e) => {
            let name = world.npc(id).map_or("", |n| n.first_name());
            window.note = Some(format!("{name} {e}"));
            return None;
        }
    };
    if !reply.said.is_empty() {
        says.say(Sayer::Player, reply.said.clone());
    }
    says.say(Sayer::Npc(id), reply.answer.clone());
    window.note = (reply.tokens > 0).then(|| format!("Hai ricevuto {} gettoni.", reply.tokens));
    match reply.action {
        ChatAction::OpenGift => window.gift = true,
        ChatAction::OpenMarket(_) => market.open = true,
        ChatAction::End => window.npc = None,
        ChatAction::None => {}
    }
    Some(reply)
}

/// Chiude la chat; se la pausa l'aveva messa lei, il tempo riparte.
fn close(window: &mut ChatWindow, clock: &mut SimClock) {
    if window.paused_by_chat && clock.paused {
        clock.paused = false;
    }
    window.npc = None;
    window.gift = false;
    window.paused_by_chat = false;
    window.portrait = None;
}

// --- Sistemi --------------------------------------------------------------------------

fn reset_chat(mut window: ResMut<ChatWindow>, mut queue: ResMut<ChatQueue>) {
    *window = ChatWindow::default();
    queue.0.clear();
}

fn chat_escape(
    window: Res<ChatWindow>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut queue: ResMut<ChatQueue>,
) {
    if window.is_open() && keys.just_pressed(KeyCode::Escape) {
        keys.clear_just_pressed(KeyCode::Escape);
        queue.0.push(ChatCommand::Close);
    }
}

fn apply_chat(
    mut sim: ResMut<Sim>,
    mut clock: ResMut<SimClock>,
    mut window: ResMut<ChatWindow>,
    mut market: ResMut<MarketWindow>,
    mut says: ResMut<ChatSays>,
    mut queue: ResMut<ChatQueue>,
) {
    if queue.0.is_empty() {
        return;
    }
    let commands = std::mem::take(&mut queue.0);
    for command in commands {
        run_command(
            &mut sim.world,
            &mut clock,
            &mut window,
            &mut market,
            &mut says,
            command,
        );
    }
    // Chiusa da un saluto: il tempo riparte.
    if !window.is_open() && window.paused_by_chat {
        close(&mut window, &mut clock);
    }
}

/// La chat si chiude se l'NPC non c'è più, si addormenta o il giocatore
/// si allontana (es. un caricamento, il sonno).
fn close_when_gone(sim: Res<Sim>, mut clock: ResMut<SimClock>, mut window: ResMut<ChatWindow>) {
    let Some(id) = window.npc else {
        return;
    };
    if sim.world.can_chat(id).is_err() {
        close(&mut window, &mut clock);
    }
}

fn paint_portrait(ui: &mut egui::Ui, canvas: &Canvas) {
    let size = egui::vec2(
        canvas.width as f32 * PORTRAIT_SCALE,
        canvas.height as f32 * PORTRAIT_SCALE,
    );
    let (rect, _) = ui.allocate_exact_size(size + egui::vec2(8.0, 8.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 4.0, Color32::from_rgb(70, 78, 96));
    let origin = rect.min + egui::vec2(4.0, 4.0);
    for y in 0..canvas.height {
        for x in 0..canvas.width {
            let [r, g, b, a] = canvas.get(x, y);
            if a == 0 {
                continue;
            }
            let min = origin + egui::vec2(x as f32, y as f32) * PORTRAIT_SCALE;
            painter.rect_filled(
                egui::Rect::from_min_size(min, egui::vec2(PORTRAIT_SCALE, PORTRAIT_SCALE)),
                0.0,
                Color32::from_rgba_unmultiplied(r, g, b, a),
            );
        }
    }
}

/// Una riga dello storico: l'NPC a sinistra, il giocatore a destra.
fn history_line(ui: &mut egui::Ui, speaker: Speaker, name: &str, text: &str) {
    let (ink, paper, layout) = match speaker {
        Speaker::Npc => (
            NPC_INK,
            NPC_PAPER,
            egui::Layout::left_to_right(egui::Align::TOP),
        ),
        Speaker::Player => (
            PLAYER_INK,
            PLAYER_PAPER,
            egui::Layout::right_to_left(egui::Align::TOP),
        ),
    };
    ui.with_layout(layout, |ui| {
        egui::Frame::new()
            .fill(paper)
            .corner_radius(6.0)
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.set_max_width(WINDOW_WIDTH * 0.78);
                ui.label(RichText::new(name).small().color(ink.gamma_multiply(0.7)));
                ui.add(egui::Label::new(RichText::new(text).color(ink)).wrap());
            });
    });
}

fn chat_window(
    mut contexts: EguiContexts,
    sim: Res<Sim>,
    time: Res<Time<Real>>,
    mut window: ResMut<ChatWindow>,
    mut queue: ResMut<ChatQueue>,
    mut gangs: ResMut<GangQueue>,
) {
    let Some(id) = window.npc else {
        return;
    };
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let world = &sim.world;
    let Some(npc) = world.npc(id) else {
        return;
    };
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        queue.0.push(ChatCommand::Close);
        return;
    }
    let mut open = true;
    let center = ctx.content_rect().center();
    let mut commands: Vec<ChatCommand> = Vec::new();
    let mut gang_commands: Vec<GangCommand> = Vec::new();
    egui::Window::new(format!("Chat con {}", npc.first_name()))
        .id(egui::Id::new("chat_window"))
        .default_pos(center + egui::vec2(260.0, -260.0))
        .default_width(WINDOW_WIDTH)
        .resizable(false)
        .collapsible(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.set_width(WINDOW_WIDTH);
            // Ritratto, nome, lavoro, affinità.
            ui.horizontal(|ui| {
                if let Some(canvas) = &window.portrait {
                    paint_portrait(ui, canvas);
                }
                ui.vertical(|ui| {
                    ui.label(RichText::new(&npc.name).strong().size(17.0));
                    ui.weak(format!("{} · {} anni", role_line(world, npc), npc.age));
                    ui.weak(npc.personality().describe(npc.sex));
                    let affinity = npc.player_affinity();
                    let band_color = match Band::of(npc.player.as_ref()) {
                        Band::Low => Color32::from_rgb(230, 110, 100),
                        Band::Mid => Color32::from_rgb(220, 200, 120),
                        Band::High => Color32::from_rgb(130, 210, 130),
                    };
                    ui.label(RichText::new(regard_line(npc)).color(band_color));
                    ui.add(
                        egui::ProgressBar::new((affinity + 1.0) / 2.0)
                            .desired_width(180.0)
                            .desired_height(8.0)
                            .fill(band_color),
                    )
                    .on_hover_text(format!("Affinità {affinity:+.2} (da −1 a +1)"));
                });
            });
            if let Some(f) = world.player_favour(id).filter(|f| f.told) {
                let what = sim::dialogue::grammar::counted(
                    f.count,
                    f.item.with_article(),
                    f.item.plural(),
                );
                let have = world.player.inventory.count(f.item);
                ui.label(
                    RichText::new(format!(
                        "Incarico: portare {what} ({}), entro il giorno {}. Ne hai {have}.",
                        sim::dialogue::grammar::tokens(f.reward),
                        f.until.day()
                    ))
                    .color(FAVOUR_INK),
                );
            }
            gang_offers(ui, world, npc, &mut gang_commands);
            if let Some(note) = gangs.note(time.elapsed_secs_f64()) {
                ui.label(RichText::new(note).color(FAVOUR_INK));
            }
            ui.separator();
            // Storico.
            let log = world.chat_log(id);
            egui::ScrollArea::vertical()
                .max_height(HISTORY_HEIGHT)
                .min_scrolled_height(HISTORY_HEIGHT)
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if log.is_empty() {
                        ui.weak("Non vi siete ancora parlati.");
                    }
                    for line in log {
                        let name = match line.speaker {
                            Speaker::Npc => npc.first_name(),
                            Speaker::Player => world.player.first_name(),
                        };
                        history_line(ui, line.speaker, name, &line.text);
                        ui.add_space(2.0);
                    }
                });
            ui.separator();
            // Scelta del regalo.
            if window.gift {
                let options = world.chat_gift_options(id);
                ui.horizontal_wrapped(|ui| {
                    ui.label("Cosa regali?");
                    for item in &options {
                        if ui.button(format!("Dai {}", item.with_article())).clicked() {
                            commands.push(ChatCommand::Give(*item));
                        }
                    }
                    if options.is_empty() {
                        ui.weak("Non hai niente che accetti.");
                    }
                    if ui.small_button("Annulla").clicked() {
                        window.gift = false;
                    }
                });
                ui.separator();
            }
            // Risposte suggerite.
            egui::Grid::new("chat_intents")
                .num_columns(3)
                .spacing([6.0, 6.0])
                .show(ui, |ui| {
                    for (k, intent) in Intent::ALL.into_iter().enumerate() {
                        let label = intent_label(world, id, intent);
                        let button = egui::Button::new(label).min_size(egui::vec2(124.0, 24.0));
                        if ui.add(button).on_hover_text(intent.describe()).clicked() {
                            commands.push(ChatCommand::Say(intent));
                        }
                        if k % 3 == 2 {
                            ui.end_row();
                        }
                    }
                });
            ui.add_space(4.0);
            // Testo libero.
            ui.horizontal(|ui| {
                let edit = egui::TextEdit::singleline(&mut window.input)
                    .hint_text("Scrivi qualcosa…")
                    .char_limit(MAX_CHAT_INPUT_CHARS)
                    .desired_width(WINDOW_WIDTH - 70.0);
                let response = ui.add(edit);
                if window.focus {
                    response.request_focus();
                    window.focus = false;
                }
                let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let send = ui.button("Invia").clicked();
                if (enter || send) && !window.input.trim().is_empty() {
                    commands.push(ChatCommand::Type(std::mem::take(&mut window.input)));
                    window.focus = true;
                }
            });
            if let Some(note) = &window.note {
                ui.label(RichText::new(note).color(FAVOUR_INK));
            }
            ui.weak("Invio: invia · Esc: chiudi · il tempo è fermo mentre parli");
        });
    if !open {
        commands.push(ChatCommand::Close);
    }
    queue.0.extend(commands);
    gangs.commands.extend(gang_commands);
}

/// Le offerte delle bande in chat: l'invito di un membro amico, il pizzo
/// da riscuotere per la tua banda, quello che un membro ti chiede.
fn gang_offers(ui: &mut egui::Ui, world: &World, npc: &Npc, commands: &mut Vec<GangCommand>) {
    let id = npc.id;
    if let Some(g) = world.gang_invite_from(id) {
        ui.horizontal_wrapped(|ui| {
            swatch(ui, gang_color(g));
            ui.label(
                RichText::new(format!("Ti invita nella banda «{}».", g.name)).color(FAVOUR_INK),
            );
            if ui
                .button("Entra nella banda")
                .on_hover_text("Protezione, una parte del tesoro e qualche incarico.")
                .clicked()
            {
                commands.push(GangCommand::Join(id));
            }
            if ui.button("Rifiuta").clicked() {
                commands.push(GangCommand::Refuse(id));
            }
        });
    }
    if let Some(t) = world.gang_task().filter(|t| t.victim == id) {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!(
                    "Incarico della banda: {} gettoni di pizzo.",
                    t.tokens
                ))
                .color(FAVOUR_INK),
            );
            if ui.button("Riscuoti il pizzo").clicked() {
                commands.push(GangCommand::Collect(id));
            }
        });
    }
    if let Some(g) = world.gang_of(id)
        && g.player_due > 0
        && g.demanded.is_some()
    {
        ui.horizontal_wrapped(|ui| {
            swatch(ui, gang_color(g));
            ui.label(
                RichText::new(format!(
                    "Chiede {} gettoni di pizzo per «{}».",
                    g.player_due, g.name
                ))
                .color(FAVOUR_INK),
            );
            let can = world.player.tokens >= g.player_due;
            if ui.add_enabled(can, egui::Button::new("Paga")).clicked() {
                commands.push(GangCommand::Pay(g.id));
            }
            if ui.button("Rifiuta").clicked() {
                commands.push(GangCommand::RefusePizzo(g.id));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::time::TimeUpdateStrategy;
    use sim::{Action, Place, PlayerTie};

    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::input::InputPlugin,
            crate::state::StatePlugin,
            crate::sim_bridge::SimBridgePlugin,
            ChatPlugin,
        ))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            100,
        )));
        app.finish();
        app.cleanup();
        app.update();
        app
    }

    /// An awake adult idling where the player is.
    fn stage_npc(world: &mut World) -> NpcId {
        let i = world
            .npcs
            .iter()
            .position(|n| n.age >= 20 && n.age < 60 && n.job != Some(sim::Job::Mercante))
            .unwrap();
        let now = world.clock;
        let n = &mut world.npcs[i];
        if let Some(s) = n.action.station() {
            let st = &mut world.carriages[n.carriage.index()].stations[s.index()];
            st.occupancy = st.occupancy.saturating_sub(1);
        }
        n.floor = 0;
        n.action = Action::Idle;
        n.action_since = now;
        n.action_until = now + 600;
        let (id, carriage) = (n.id, n.carriage);
        world.set_player_place(Place { carriage, floor: 0 });
        id
    }

    /// The chat opens (and stops the time), a suggested reply applies its
    /// effect in the sim and shows as bubbles, closing restarts the time.
    #[test]
    fn the_chat_opens_and_a_suggested_reply_applies_its_effect() {
        let mut app = app();
        let id = stage_npc(&mut app.world_mut().resource_mut::<Sim>().world);
        let queue = |app: &mut App, c: ChatCommand| {
            app.world_mut().resource_mut::<ChatQueue>().0.push(c);
            app.update();
        };
        queue(&mut app, ChatCommand::Open(id));
        assert_eq!(app.world().resource::<ChatWindow>().npc, Some(id));
        assert!(app.world().resource::<SimClock>().paused);
        let clock = app.world().resource::<Sim>().world.clock;

        queue(&mut app, ChatCommand::Say(Intent::Greet));
        let world = &app.world().resource::<Sim>().world;
        let npc = world.npc(id).unwrap();
        assert!((npc.player_affinity() - sim::chat::CHAT_GREET_AFFINITY).abs() < 1e-6);
        assert_eq!(world.chat_log(id).len(), 2);
        // Time stood still while talking.
        assert_eq!(world.clock, clock);
        let says = app.world().resource::<ChatSays>();
        assert!(says.pending().any(|s| s.who == Sayer::Player));
        assert!(says.pending().any(|s| s.who == Sayer::Npc(id)));

        // Free text works too.
        queue(&mut app, ChatCommand::Type("che lavoro fai?".into()));
        assert_eq!(app.world().resource::<Sim>().world.chat_log(id).len(), 4);

        // "Regala" with something the NPC accepts opens the gift choice.
        {
            let mut sim = app.world_mut().resource_mut::<Sim>();
            let w = &mut sim.world;
            w.player.inventory.add(ItemKind::Razione, 1);
            let i = w.npcs.iter().position(|n| n.id == id).unwrap();
            w.npcs[i].needs.hunger = 0.2;
        }
        queue(&mut app, ChatCommand::Say(Intent::Gift));
        assert!(app.world().resource::<ChatWindow>().gift);
        queue(&mut app, ChatCommand::Give(ItemKind::Razione));
        assert_eq!(
            app.world()
                .resource::<Sim>()
                .world
                .player
                .inventory
                .count(ItemKind::Razione),
            0
        );

        // Goodbye closes the chat and the time runs again.
        queue(&mut app, ChatCommand::Say(Intent::Farewell));
        assert_eq!(app.world().resource::<ChatWindow>().npc, None);
        assert!(!app.world().resource::<SimClock>().paused);
    }

    #[test]
    fn a_merchant_at_the_counter_opens_the_market_window() {
        let mut app = app();
        let id = {
            let mut sim = app.world_mut().resource_mut::<Sim>();
            let w = &mut sim.world;
            let market = w.markets()[0];
            let counter = w.carriages[market.index()]
                .free_station(sim::StationKind::Counter)
                .unwrap();
            let i = w
                .npcs
                .iter()
                .position(|n| n.job == Some(sim::Job::Mercante))
                .unwrap();
            let now = w.clock;
            let n = &mut w.npcs[i];
            if let Some(s) = n.action.station() {
                let st = &mut w.carriages[n.carriage.index()].stations[s.index()];
                st.occupancy = st.occupancy.saturating_sub(1);
            }
            n.carriage = market;
            n.workplace = Some(market);
            n.floor = 0;
            n.action = Action::Work(counter);
            n.action_until = now + 600;
            n.player = Some(PlayerTie {
                affinity: 0.6,
                ..PlayerTie::default()
            });
            let id = n.id;
            w.carriages[market.index()].stations[counter.index()].occupancy += 1;
            w.set_player_place(Place {
                carriage: market,
                floor: 0,
            });
            id
        };
        app.world_mut()
            .resource_mut::<ChatQueue>()
            .0
            .extend([ChatCommand::Open(id), ChatCommand::Say(Intent::Trade)]);
        app.update();
        assert!(app.world().resource::<MarketWindow>().open);
        assert!(app.world().resource::<ChatWindow>().is_open());
        app.world_mut()
            .resource_mut::<ChatQueue>()
            .0
            .push(ChatCommand::Close);
        app.update();
        assert!(!app.world().resource::<ChatWindow>().is_open());
        assert!(!app.world().resource::<SimClock>().paused);
    }

    #[test]
    fn labels() {
        let mut world = World::generate(3, 10, 200);
        let id = stage_npc(&mut world);
        let npc = world.npc(id).unwrap().clone();
        assert_eq!(regard_line(&npc), "Non ti conosce");
        assert!(!role_line(&world, &npc).is_empty());
        assert_eq!(
            intent_label(&world, id, Intent::AskFavour),
            "Chiedi un favore"
        );
    }
}
