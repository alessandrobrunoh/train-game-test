//! Stato condiviso tra i moduli del gioco: la simulazione e i controlli del tempo.
//!
//! `sim_bridge.rs` fa avanzare la simulazione e disegna gli NPC,
//! `ui.rs` mostra controlli del tempo, ispettore ed eventi, `interaction.rs`
//! e `inventory.rs` gestiscono cosa il giocatore prende, compra e regala.
//! Il giocatore (nome, gettoni, inventario, cabina) vive nella sim:
//! `sim.world.player` (vedi `cabin.rs`).

use std::path::PathBuf;

use bevy::prelude::*;
use sim::{NpcId, UtilityBrain, World};
use sim_laya::{LayaBrain, LayaConfig};

pub struct StatePlugin;

impl Plugin for StatePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SimClock>()
            .init_resource::<SelectedNpc>()
            .init_resource::<FollowNpc>()
            .init_resource::<SimPerf>()
            .init_resource::<PointerOverUi>()
            .add_message::<WorldReplaced>()
            .add_message::<CatalogChanged>()
            .add_systems(
                PreUpdate,
                watch_catalog.before(crate::saves::WorldRebuildSet),
            );
    }
}

/// Manda [`CatalogChanged`] quando il Custode ha aggiunto oggetti, lavori o
/// postazioni al mondo.
fn watch_catalog(
    sim: Option<Res<Sim>>,
    mut last: Local<Option<(usize, usize, usize)>>,
    mut changed: MessageWriter<CatalogChanged>,
) {
    let Some(sim) = sim else {
        return;
    };
    let world = &sim.world;
    let now = (
        world.catalog().item_count(),
        world.catalog().job_count(),
        world.carriages.iter().map(|c| c.stations.len()).sum(),
    );
    if last.is_some_and(|l| l != now) {
        changed.write(CatalogChanged);
    }
    *last = Some(now);
}

/// Il cervello degli NPC: `UtilityBrain`, più Laya quando il giocatore lo
/// attiva (vedi `brain_ui.rs`). Nei salvataggi va solo il `UtilityBrain`
/// ([`LayaBrain::fallback`]); il resto è stato di esecuzione.
pub type GameBrain = LayaBrain<UtilityBrain>;

/// Peso della regola del `sim` nelle deliberazioni, nel gioco: Laya zero-shot
/// da solo sceglie quasi a caso (vedi `docs/laya-brain.md`, §9), miscelato a
/// metà con la regola decide soprattutto quando è d'accordo con lei.
pub const GAME_PRIOR_WEIGHT: f32 = 0.5;

/// Cervello del gioco sopra `utility`, con Laya spento finché non lo si sceglie.
/// Chi è in vista non aspetta Laya (`think`): con risposte lente gli NPC
/// resterebbero fermi; si riattiva dal pannello del cervello (B).
pub fn new_brain(utility: UtilityBrain) -> GameBrain {
    LayaBrain::new(
        utility,
        LayaConfig {
            enabled: false,
            think: false,
            prior_weight: GAME_PRIOR_WEIGHT,
            ..LayaConfig::default()
        },
    )
}

/// Il mondo simulato e il cervello che decide per gli NPC.
#[derive(Resource)]
pub struct Sim {
    pub world: World,
    pub brain: GameBrain,
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

/// Partita in corso: una "run" nasce con una nuova partita e continua nei
/// salvataggi. Lo storico SQLite (`history`) e i salvataggi (`saves`) vivono
/// nella sua cartella. La crea e la aggiorna `saves.rs`.
#[derive(Resource, Debug, Clone)]
pub struct RunInfo {
    /// Identificativo stabile della run, usato anche come nome della cartella.
    pub run_id: String,
    /// Cartella della run, ad esempio `.../TrainGame/runs/<run_id>/`.
    pub dir: PathBuf,
}

/// Inviato quando `Sim` viene sostituito da un caricamento o da una nuova
/// partita: chi tiene uno stato derivato dal mondo (storico, grafici,
/// sprite, selezione) deve ricostruirlo. `RunInfo` è già aggiornato.
#[derive(Message, Debug, Clone, Copy)]
pub struct WorldReplaced;

/// Inviato quando il Custode ha fatto crescere il mondo (oggetti nuovi nei
/// magazzini, postazioni per un lavoro nuovo): postazioni e magazzini si
/// ridisegnano, il resto no.
#[derive(Message, Debug, Clone, Copy)]
pub struct CatalogChanged;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_brain_does_not_make_npcs_wait_for_laya() {
        let brain = new_brain(UtilityBrain::new(1));
        assert!(!brain.config().enabled);
        assert!(!brain.config().think);
    }
}
