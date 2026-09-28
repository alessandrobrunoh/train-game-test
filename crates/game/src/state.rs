//! Stato condiviso tra i moduli del gioco: la simulazione e i controlli del tempo.
//!
//! `sim_bridge.rs` fa avanzare la simulazione e disegna gli NPC,
//! `ui.rs` mostra controlli del tempo, ispettore ed eventi.

use bevy::prelude::*;
use sim::{NpcId, UtilityBrain, World};

pub struct StatePlugin;

impl Plugin for StatePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SimClock>()
            .init_resource::<SelectedNpc>()
            .init_resource::<PointerOverUi>();
    }
}

/// Il mondo simulato e il cervello che decide per gli NPC.
#[derive(Resource)]
pub struct Sim {
    pub world: World,
    pub brain: UtilityBrain,
}

/// Velocità del tempo di gioco.
#[derive(Resource)]
pub struct SimClock {
    pub paused: bool,
    /// Minuti di gioco per secondo reale (1 tick della sim = 1 minuto).
    pub minutes_per_second: f32,
    /// Frazione di tick accumulata tra un frame e l'altro.
    pub accumulator: f32,
}

impl Default for SimClock {
    fn default() -> Self {
        Self {
            paused: false,
            minutes_per_second: 1.0,
            accumulator: 0.0,
        }
    }
}

/// NPC selezionato con un click, mostrato nell'ispettore.
#[derive(Resource, Default)]
pub struct SelectedNpc(pub Option<NpcId>);

/// Vero quando il puntatore è sopra un pannello egui: i click non vanno al mondo.
#[derive(Resource, Default)]
pub struct PointerOverUi(pub bool);

/// Sprite che rappresenta un NPC della simulazione.
#[derive(Component)]
pub struct NpcSprite(pub NpcId);
