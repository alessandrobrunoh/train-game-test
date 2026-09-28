//! Ponte con la simulazione: crea il mondo simulato e lo fa avanzare nel tempo.
//!
//! La simulazione gira a tick interi (1 tick = 1 minuto di gioco). `SimClock`
//! dice quanti minuti scorrono per secondo reale; qui si accumula il tempo e si
//! eseguono i tick interi, con un tetto per frame così le velocità alte non
//! bloccano il gioco.

use bevy::prelude::*;
use sim::{UtilityBrain, World};

use crate::state::{Sim, SimClock};

// --- Parametri della partita -----------------------------------------------

/// Seme del mondo e del cervello degli NPC.
pub const SIM_SEED: u64 = 42;
/// Numero di carrozze del treno.
pub const SIM_CARRIAGES: usize = 20;
/// Popolazione iniziale.
pub const SIM_NPCS: usize = 400;

/// Massimo numero di tick eseguiti in un singolo frame.
const MAX_TICKS_PER_FRAME: u32 = 600;

/// Velocità selezionabili con i tasti 1-4 (minuti di gioco per secondo reale).
const SPEED_KEYS: [(KeyCode, f32); 4] = [
    (KeyCode::Digit1, 1.0),
    (KeyCode::Digit2, 10.0),
    (KeyCode::Digit3, 60.0),
    (KeyCode::Digit4, 600.0),
];

/// Crea la simulazione con i parametri della partita.
pub fn new_sim() -> Sim {
    Sim {
        world: World::generate(SIM_SEED, SIM_CARRIAGES, SIM_NPCS),
        brain: UtilityBrain::new(SIM_SEED),
    }
}

/// Sistema che fa avanzare la simulazione (per ordinare altri sistemi dopo di lui).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct SimTickSet;

pub struct SimBridgePlugin;

impl Plugin for SimBridgePlugin {
    fn build(&self, app: &mut App) {
        // Inserito subito (non in Startup): `TrainPlugin` ne legge le carrozze.
        app.insert_resource(new_sim()).add_systems(
            Update,
            (time_shortcuts, advance_sim).chain().in_set(SimTickSet),
        );
    }
}

/// P = pausa/riprendi, 1/2/3/4 = 1, 10, 60, 600 minuti al secondo.
fn time_shortcuts(keys: Res<ButtonInput<KeyCode>>, mut clock: ResMut<SimClock>) {
    if keys.just_pressed(KeyCode::KeyP) {
        clock.paused = !clock.paused;
    }
    for (key, speed) in SPEED_KEYS {
        if keys.just_pressed(key) {
            clock.minutes_per_second = speed;
        }
    }
}

fn advance_sim(time: Res<Time>, mut clock: ResMut<SimClock>, mut sim: ResMut<Sim>) {
    if clock.paused {
        return;
    }
    let ticks = accumulate(&mut clock, time.delta_secs());
    if ticks == 0 {
        return;
    }
    let Sim { world, brain } = &mut *sim;
    for _ in 0..ticks {
        world.tick(brain);
    }
}

/// Aggiunge il tempo del frame all'accumulatore e restituisce i tick interi da
/// eseguire (al massimo `MAX_TICKS_PER_FRAME`; l'arretrato oltre il tetto si perde).
fn accumulate(clock: &mut SimClock, dt: f32) -> u32 {
    clock.accumulator += dt * clock.minutes_per_second.max(0.0);
    let whole = clock.accumulator.floor();
    if whole >= MAX_TICKS_PER_FRAME as f32 {
        clock.accumulator = clock.accumulator.fract();
        return MAX_TICKS_PER_FRAME;
    }
    clock.accumulator -= whole;
    whole as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_fractions_of_a_tick() {
        let mut clock = SimClock {
            minutes_per_second: 1.0,
            ..default()
        };
        assert_eq!(accumulate(&mut clock, 0.4), 0);
        assert_eq!(accumulate(&mut clock, 0.4), 0);
        assert_eq!(accumulate(&mut clock, 0.4), 1);
        assert!((clock.accumulator - 0.2).abs() < 1e-5);
    }

    #[test]
    fn caps_ticks_per_frame_and_drops_the_backlog() {
        let mut clock = SimClock {
            minutes_per_second: 600.0,
            ..default()
        };
        assert_eq!(accumulate(&mut clock, 5.0), MAX_TICKS_PER_FRAME);
        assert!(clock.accumulator < 1.0);
    }
}
