//! Finestra "Inventario" del giocatore (tasto I): gettoni e oggetti a slot.
//!
//! L'inventario vive nella sim (`world.player.inventory`, uno
//! `sim::SlotInventory`): [`sim::INVENTORY_SLOTS`] scomparti, ciascuno con
//! una pila di un solo oggetto (materiali fino a 10, cibo fino a 5, oggetti
//! durevoli uno per scomparto). La griglia degli scomparti ([`slot_grid`]) è
//! la stessa del baule della cabina (vedi `cabin.rs`).

use bevy::prelude::*;
use bevy_egui::egui::{self, Align2, Color32, RichText};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use sim::{ItemKind, SlotInventory, World};

use crate::item_icons::ItemIcons;
use crate::state::Sim;
use crate::storage::{item_color, plural_title};
use crate::ui::{MARGIN, PointerCheck, color32};

/// Lato di uno scomparto (pixel logici).
const SLOT_SIZE: f32 = 44.0;
/// Scomparti per riga.
pub(crate) const SLOT_COLUMNS: usize = 4;

/// La finestra dell'inventario è aperta.
#[derive(Resource, Default)]
pub(crate) struct InventoryWindow {
    pub(crate) open: bool,
}

pub struct InventoryPlugin;

impl Plugin for InventoryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InventoryWindow>()
            .add_systems(Update, toggle_inventory)
            .add_systems(
                EguiPrimaryContextPass,
                inventory_window.before(PointerCheck),
            );
    }
}

fn toggle_inventory(keys: Res<ButtonInput<KeyCode>>, mut window: ResMut<InventoryWindow>) {
    if keys.just_pressed(KeyCode::KeyI) {
        window.open = !window.open;
    }
}

/// Sigla di un oggetto nello scomparto: le prime lettere del nome.
pub(crate) fn short_name(item: ItemKind) -> String {
    let name = item.name();
    let word = name
        .split_whitespace()
        .find(|w| w.chars().count() > 2)
        .unwrap_or(name);
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars.take(3)).collect(),
        None => String::new(),
    }
}

/// Testo scuro o chiaro, leggibile sul colore `bg`.
fn ink_on(bg: Color32) -> Color32 {
    let luma = 0.299 * f32::from(bg.r()) + 0.587 * f32::from(bg.g()) + 0.114 * f32::from(bg.b());
    if luma > 140.0 {
        Color32::from_rgb(30, 26, 36)
    } else {
        Color32::from_rgb(245, 240, 230)
    }
}

/// Griglia degli scomparti di `inventory`, [`SLOT_COLUMNS`] per riga: ogni
/// pila col colore dell'oggetto, la sigla e il numero (gli oggetti aggiunti
/// dal Custode con la loro icona). Se `hint` c'è, gli scomparti pieni sono
/// cliccabili (con quel suggerimento): restituisce quello cliccato.
pub(crate) fn slot_grid(
    ui: &mut egui::Ui,
    world: &World,
    icons: &mut ItemIcons,
    inventory: &SlotInventory,
    id: &str,
    hint: Option<&str>,
) -> Option<usize> {
    let mut clicked = None;
    egui::Grid::new(id)
        .spacing([4.0, 4.0])
        .min_col_width(SLOT_SIZE)
        .show(ui, |ui| {
            for (i, slot) in inventory.slots().iter().enumerate() {
                let sense = if slot.is_some() && hint.is_some() {
                    egui::Sense::click()
                } else {
                    egui::Sense::hover()
                };
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(SLOT_SIZE, SLOT_SIZE), sense);
                let painter = ui.painter();
                let frame = if response.hovered() && slot.is_some() {
                    Color32::from_rgb(240, 220, 140)
                } else {
                    Color32::from_gray(90)
                };
                painter.rect_filled(rect, 3.0, Color32::from_gray(28));
                painter.rect_stroke(
                    rect,
                    3.0,
                    egui::Stroke::new(1.0, frame),
                    egui::StrokeKind::Inside,
                );
                if let Some(stack) = slot {
                    let def = world.catalog().get_item(stack.item);
                    let look = def.and_then(|d| d.appearance.as_ref());
                    let inner = rect.shrink(6.0);
                    if let Some(look) = look {
                        // Icona procedurale 16×16, ingrandita due volte.
                        let texture = icons.item(ui.ctx(), look);
                        let icon = egui::Rect::from_center_size(
                            inner.center() - egui::vec2(0.0, 2.0),
                            egui::vec2(32.0, 32.0),
                        );
                        ui.painter().image(
                            texture,
                            icon,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    } else {
                        let bg = color32(item_color(stack.item));
                        painter.rect_filled(inner, 2.0, bg);
                        painter.text(
                            inner.center() - egui::vec2(0.0, 4.0),
                            egui::Align2::CENTER_CENTER,
                            short_name(stack.item),
                            egui::FontId::proportional(11.0),
                            ink_on(bg),
                        );
                    }
                    let painter = ui.painter();
                    if stack.count > 1 || stack.item.stack_size() > 1 {
                        painter.text(
                            rect.right_bottom() - egui::vec2(4.0, 2.0),
                            egui::Align2::RIGHT_BOTTOM,
                            stack.count.to_string(),
                            egui::FontId::proportional(13.0),
                            Color32::WHITE,
                        );
                    }
                    let mut tip = format!(
                        "{} × {} (pila da {})\n{}",
                        stack.count,
                        plural_title(stack.item),
                        stack.item.stack_size(),
                        def.map_or("", |d| &d.description)
                    );
                    if def.is_some_and(|d| d.added.is_some()) {
                        tip.push_str("\nInventato dal Narratore, approvato dal Custode.");
                    }
                    if let Some(hint) = hint {
                        tip.push_str(&format!("\n{hint}"));
                    }
                    let response = response.on_hover_text(tip);
                    if response.clicked() {
                        clicked = Some(i);
                    }
                }
                if (i + 1) % SLOT_COLUMNS == 0 {
                    ui.end_row();
                }
            }
        });
    clicked
}

fn inventory_window(
    mut contexts: EguiContexts,
    sim: Res<Sim>,
    mut window: ResMut<InventoryWindow>,
    mut icons: ResMut<ItemIcons>,
) {
    if !window.open {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let player = &sim.world.player;
    let mut open = true;
    egui::Window::new("Inventario")
        .anchor(Align2::RIGHT_BOTTOM, [-MARGIN, -40.0])
        .resizable(false)
        .collapsible(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.strong(&player.name);
                ui.weak("·");
                ui.label(RichText::new(format!("{} gettoni", player.tokens)).strong());
            });
            let inv = &player.inventory;
            ui.weak(format!(
                "{} scomparti liberi su {}",
                inv.free_slots(),
                inv.len()
            ));
            ui.separator();
            slot_grid(ui, &sim.world, &mut icons, inv, "inventory_slots", None);
            ui.separator();
            ui.weak(
                "E vicino alle scorte: prendi (Q: cambia)\nE al bancone del Mercato: compra (Q: cambia)\nM: mercato, per comprare e vendere\nC: crafting\nE vicino a un NPC: regala (se non accetta niente, parla)\nT vicino a un NPC: chat (saluta, chiedi, incarichi, scambia…)\nNella tua cabina: E sul letto (dormi), E sul baule",
            );
        });
    if !open {
        window.open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_names_are_readable() {
        assert_eq!(short_name(ItemKind::Verdura), "Verd");
        assert_eq!(short_name(ItemKind::Te), "Tè");
        for item in ItemKind::BUILTIN {
            let s = short_name(item);
            assert!((1..=4).contains(&s.chars().count()), "{item:?}: {s}");
        }
    }
}
