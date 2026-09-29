//! La finestra **"Cronaca del treno"** (tasto N) e l'avviso delle novità.
//!
//! In testa: il modello, acceso o spento, le chiamate rimaste nel budget
//! orario, la latenza media, "Chiedi ora" (aiuto per lo sviluppo, dentro il
//! budget) e "In pausa" (niente richieste finché è spuntato).
//!
//! Sotto, le voci dalla più recente: icona (quella procedurale per gli
//! oggetti), nome, tipo e giorno, descrizione, "Perché: …" e lo stato
//! ("proposta — in attesa del Custode" o "rifiutata: motivo"). Aprendo una
//! voce si vede il suo pannello (`ai_ui::render_panel`), dal vivo.
//!
//! Quando arriva una novità compare in alto "Novità sul treno: «Nome»";
//! un click apre la cronaca.

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, RichText};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use narrator::Proposal;

use crate::ai_ui::{PanelQueue, render_panel};
use crate::item_icons::{ItemIcons, show_icon};
use crate::narrator_bridge::{ChronicleEntry, EntryStatus, NarratorState, TOAST_SECS};
use crate::state::Sim;
use crate::ui::PointerCheck;

const WIDTH: f32 = 440.0;
const ICON_SIZE: f32 = 32.0;
const PROPOSED: Color32 = Color32::from_rgb(230, 200, 110);
const REJECTED: Color32 = Color32::from_rgb(230, 110, 100);
const ON: Color32 = Color32::from_rgb(120, 200, 120);
const OFF: Color32 = Color32::from_rgb(170, 170, 170);

pub struct ChronicleUiPlugin;

impl Plugin for ChronicleUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChronicleWindow>()
            .init_resource::<ItemIcons>()
            .add_systems(Update, toggle_chronicle)
            .add_systems(
                EguiPrimaryContextPass,
                (chronicle_window, novelty_toast).before(PointerCheck),
            );
    }
}

/// La finestra è aperta; quante voci recenti mostrare già aperte.
#[derive(Resource)]
pub struct ChronicleWindow {
    pub open: bool,
    pub expanded_newest: usize,
}

impl Default for ChronicleWindow {
    fn default() -> Self {
        Self {
            open: false,
            expanded_newest: 1,
        }
    }
}

fn toggle_chronicle(keys: Res<ButtonInput<KeyCode>>, mut window: ResMut<ChronicleWindow>) {
    if keys.just_pressed(KeyCode::KeyN) {
        window.open = !window.open;
    }
}

/// Il senso di "in attesa del Custode".
pub const CUSTODE_HINT: &str =
    "Il Custode, che farà entrare le novità nel mondo, non esiste ancora (passo A2)";

/// Lo stato di una voce e il suo colore.
pub fn status_line(entry: &ChronicleEntry) -> (String, Color32) {
    match entry.status {
        EntryStatus::Proposed => ("proposta — in attesa del Custode".to_string(), PROPOSED),
        EntryStatus::Rejected => (
            format!("rifiutata: {}", entry.reason.as_deref().unwrap_or("?")),
            REJECTED,
        ),
        EntryStatus::Failed => (
            format!("non riuscita: {}", entry.reason.as_deref().unwrap_or("?")),
            REJECTED,
        ),
    }
}

fn entry_icon(ui: &mut egui::Ui, icons: &mut ItemIcons, entry: &ChronicleEntry) {
    let ctx = ui.ctx().clone();
    let texture = match entry.draft.as_ref().map(|d| &d.proposal) {
        Some(Proposal::NewItem {
            appearance: Some(look),
            ..
        }) => icons.item(&ctx, look),
        Some(p) => icons.kind(&ctx, p.kind_name()),
        None => icons.kind(&ctx, "?"),
    };
    show_icon(ui, texture, ICON_SIZE);
}

/// Una voce: testa sempre visibile, dettagli e pannello a comparsa.
fn entry_ui(
    ui: &mut egui::Ui,
    entry: &ChronicleEntry,
    index: usize,
    open_by_default: bool,
    sim: &Sim,
    state: &NarratorState,
    icons: &mut ItemIcons,
) -> Option<narrator::Action> {
    let mut pressed = None;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(WIDTH - 36.0);
        ui.horizontal(|ui| {
            entry_icon(ui, icons, entry);
            ui.vertical(|ui| {
                ui.label(RichText::new(entry.title()).strong().size(15.0));
                let kind = entry
                    .draft
                    .as_ref()
                    .map_or("risposta", |d| d.proposal.kind_name());
                ui.weak(format!("{kind} · giorno {}", entry.day));
            });
        });
        if let Some(d) = &entry.draft {
            ui.add(egui::Label::new(d.proposal.description()).wrap());
            ui.add(
                egui::Label::new(RichText::new(format!("Perché: {}", d.rationale)).weak()).wrap(),
            );
        }
        let (status, colour) = status_line(entry);
        let label = ui.add(egui::Label::new(RichText::new(status).color(colour)).wrap());
        if entry.status == EntryStatus::Proposed {
            label.on_hover_text(CUSTODE_HINT);
        }
        let Some(d) = &entry.draft else {
            return;
        };
        let id = ui.make_persistent_id(("cronaca", index));
        egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            id,
            open_by_default,
        )
        .show_header(ui, |ui| {
            ui.label(if d.panel.is_some() {
                "Pannello e dettagli"
            } else {
                "Dettagli"
            });
        })
        .body(|ui| {
            if let Some(panel) = &d.panel {
                pressed = render_panel(ui, panel, &sim.world, &state.stats);
            }
            egui::CollapsingHeader::new("La bozza per il Custode")
                .id_salt(("bozza", index))
                .default_open(false)
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(d.proposal.to_string()).small().weak())
                            .wrap(),
                    );
                });
        });
    });
    pressed
}

fn chronicle_window(
    mut contexts: EguiContexts,
    sim: Res<Sim>,
    state: Option<ResMut<NarratorState>>,
    mut window: ResMut<ChronicleWindow>,
    mut icons: ResMut<ItemIcons>,
    mut queue: ResMut<PanelQueue>,
) {
    if !window.open {
        return;
    }
    let Some(mut state) = state else {
        return;
    };
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let mut open = true;
    let mut ask = false;
    let max_height = (ctx.content_rect().height() - 190.0).max(200.0);
    let budget = state.budget_left();
    let latency = state.mean_latency_ms();
    egui::Window::new("Cronaca del treno")
        .id(egui::Id::new("chronicle_window"))
        .default_pos([crate::ui::MARGIN, 48.0])
        .default_width(WIDTH)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.set_width(WIDTH);
            // Testa: il modello e i comandi.
            ui.horizontal_wrapped(|ui| {
                if state.is_on() {
                    ui.colored_label(ON, "acceso");
                    ui.label(RichText::new(&state.model).strong());
                } else {
                    ui.colored_label(OFF, format!("Narratore {}", state.model));
                }
                if let Some(b) = budget {
                    ui.weak(format!("· budget: {b} chiamate quest'ora"));
                }
                if let Some(ms) = latency {
                    ui.weak(format!("· latenza media {:.1} s", ms as f64 / 1000.0));
                }
            });
            if let Some(e) = &state.config_error {
                ui.colored_label(REJECTED, format!("Configurazione non valida: {e}"));
            }
            if state.is_on() {
                ui.horizontal(|ui| {
                    let can_ask = !state.busy() && budget.is_some_and(|b| b > 0);
                    if ui
                        .add_enabled(can_ask, egui::Button::new("Chiedi ora"))
                        .on_hover_text("Aiuto per lo sviluppo: una novità subito (usa il budget)")
                        .clicked()
                    {
                        ask = true;
                    }
                    ui.checkbox(&mut state.paused, "In pausa");
                    if state.busy() {
                        ui.spinner();
                        ui.weak("il Narratore ci pensa…");
                    }
                });
            } else {
                ui.weak("Imposta LLM_API_URL e LLM_MODEL in .env per accenderlo.");
            }
            ui.weak("Le proposte non cambiano ancora il mondo: aspettano il Custode.");
            ui.separator();
            if state.chronicle.is_empty() {
                ui.weak("Ancora niente: il Narratore propone una novità al giorno, dalle 6:00.");
                return;
            }
            let expanded = window.expanded_newest;
            let state_ref = &*state;
            egui::ScrollArea::vertical()
                .max_height(max_height)
                .min_scrolled_height(max_height)
                .show(ui, |ui| {
                    for (rank, (i, entry)) in
                        state_ref.chronicle.iter().enumerate().rev().enumerate()
                    {
                        if let Some(a) =
                            entry_ui(ui, entry, i, rank < expanded, &sim, state_ref, &mut icons)
                        {
                            queue.0.push(a);
                        }
                    }
                });
        });
    if ask {
        state.maybe_request(&sim.world, true);
    }
    if !open {
        window.open = false;
    }
}

fn novelty_toast(
    mut contexts: EguiContexts,
    time: Res<Time<Real>>,
    state: Option<ResMut<NarratorState>>,
    mut window: ResMut<ChronicleWindow>,
) {
    let Some(mut state) = state else {
        return;
    };
    let Some(toast) = &state.toast else {
        return;
    };
    let age = time.elapsed_secs_f64() - toast.shown_at;
    if age > TOAST_SECS {
        state.toast = None;
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let text = toast.text.clone();
    let mut clicked = false;
    egui::Area::new(egui::Id::new("novelty_toast"))
        .anchor(egui::Align2::CENTER_TOP, [0.0, 118.0])
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            // Svanisce nell'ultimo secondo.
            ui.multiply_opacity(((TOAST_SECS - age) as f32).clamp(0.0, 1.0));
            let frame = egui::Frame::popup(ui.style())
                .stroke(egui::Stroke::new(1.5, PROPOSED))
                .show(ui, |ui| {
                    ui.label(RichText::new(text).color(PROPOSED).strong().size(15.0));
                    ui.weak("Click o N: cronaca del treno");
                });
            let response = frame
                .response
                .interact(egui::Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            clicked = response.clicked();
        });
    if clicked {
        window.open = true;
        state.toast = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_lines() {
        let mut e = ChronicleEntry {
            day: 1,
            status: EntryStatus::Proposed,
            draft: None,
            reason: None,
            attempts: 1,
            latency_ms: 0,
        };
        assert_eq!(status_line(&e).0, "proposta — in attesa del Custode");
        e.status = EntryStatus::Rejected;
        e.reason = Some("il nome «Tè» esiste già".into());
        assert_eq!(status_line(&e).0, "rifiutata: il nome «Tè» esiste già");
        assert_eq!(e.title(), "Risposta non valida");
        e.status = EntryStatus::Failed;
        e.reason = Some("risposta troncata (il modello ha finito i token)".into());
        assert_eq!(
            status_line(&e).0,
            "non riuscita: risposta troncata (il modello ha finito i token)"
        );
        assert_eq!(e.title(), "Nessuna novità");
    }
}
