//! Stato condiviso tra i moduli del gioco: la simulazione e i controlli del tempo.
//!
//! `sim_bridge.rs` fa avanzare la simulazione e disegna gli NPC,
//! `ui.rs` mostra controlli del tempo, ispettore ed eventi, `interaction.rs`
//! e `inventory.rs` gestiscono cosa il giocatore prende, compra e regala.

use bevy::prelude::*;
use sim::{ItemKind, NpcId, Stock, UtilityBrain, World};

/// Gettoni con cui il giocatore comincia la partita.
pub const PLAYER_START_TOKENS: u32 = 50;

pub struct StatePlugin;

impl Plugin for StatePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SimClock>()
            .init_resource::<SelectedNpc>()
            .init_resource::<FollowNpc>()
            .init_resource::<SimPerf>()
            .init_resource::<PlayerInventory>()
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

/// Vero quando la camera segue l'NPC selezionato invece del giocatore (tasto F).
/// Torna falso da solo quando non c'è più una selezione (vedi `camera.rs`).
/// Mentre è attivo il giocatore resta fermo e non interagisce.
#[derive(Resource, Default)]
pub struct FollowNpc(pub bool);

/// Quanto va veloce davvero la simulazione (vedi `sim_bridge.rs`).
#[derive(Resource, Debug, Default)]
pub struct SimPerf {
    /// Minuti di gioco simulati per secondo reale (media mobile).
    pub effective: f32,
    /// Vero se nell'ultimo periodo il tempo per frame non è bastato e parte
    /// dell'arretrato è stata scartata.
    pub behind: bool,
}

/// Vero quando il puntatore è sopra un pannello egui: i click non vanno al mondo.
#[derive(Resource, Default)]
pub struct PointerOverUi(pub bool);

/// Sprite che rappresenta un NPC della simulazione.
#[derive(Component)]
pub struct NpcSprite(pub NpcId);

/// Cosa possiede il giocatore: gettoni e oggetti (solo unità intere).
#[derive(Resource, Debug)]
pub struct PlayerInventory {
    pub tokens: u32,
    pub items: Stock,
}

impl Default for PlayerInventory {
    fn default() -> Self {
        Self {
            tokens: PLAYER_START_TOKENS,
            items: Stock::default(),
        }
    }
}

impl PlayerInventory {
    pub fn count(&self, item: ItemKind) -> u32 {
        self.items.count(item)
    }

    pub fn add(&mut self, item: ItemKind, units: u32) {
        self.items.add(item, units as f32, f32::INFINITY);
    }

    /// Toglie un'unità, se c'è.
    pub fn remove_one(&mut self, item: ItemKind) -> bool {
        if self.count(item) == 0 {
            return false;
        }
        self.items.take(item, 1.0);
        true
    }
}
