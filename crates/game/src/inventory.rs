//! Finestra "Inventario" del giocatore (tasto I): gettoni e oggetti posseduti.

use bevy::prelude::*;
use bevy_egui::egui::{self, Align2};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use sim::ItemKind;

use crate::state::PlayerInventory;
use crate::storage::plural_title;
use crate::ui::{MARGIN, PointerCheck, item_swatch};

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

fn inventory_window(
    mut contexts: EguiContexts,
    inventory: Res<PlayerInventory>,
    mut window: ResMut<InventoryWindow>,
) {
    if !window.open {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let mut open = true;
    egui::Window::new("Inventario")
        .anchor(Align2::RIGHT_BOTTOM, [-MARGIN, -40.0])
        .resizable(false)
        .collapsible(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.weak("Gettoni:");
                ui.strong(inventory.tokens.to_string());
            });
            ui.separator();
            let owned: Vec<(ItemKind, u32)> = ItemKind::ALL
                .into_iter()
                .map(|item| (item, inventory.count(item)))
                .filter(|&(_, n)| n > 0)
                .collect();
            if owned.is_empty() {
                ui.weak("Nessun oggetto.");
            } else {
                egui::Grid::new("inventory_grid")
                    .num_columns(2)
                    .spacing([16.0, 2.0])
                    .show(ui, |ui| {
                        for (item, n) in owned {
                            ui.horizontal(|ui| {
                                item_swatch(ui, item);
                                ui.label(plural_title(item));
                            });
                            ui.label(n.to_string());
                            ui.end_row();
                        }
                    });
            }
            ui.separator();
            ui.weak("E vicino alle scorte: prendi\nE al bancone del Mercato: compra (Q: cambia)\nE vicino a un NPC: regala");
        });
    if !open {
        window.open = false;
    }
}
