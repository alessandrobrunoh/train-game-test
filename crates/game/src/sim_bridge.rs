//! Ponte con la simulazione: crea il mondo simulato e lo fa avanzare nel tempo.
//!
//! La simulazione gira a tick interi (1 tick = 1 minuto di gioco). `SimClock`
//! dice quanti minuti scorrono per secondo reale; qui si accumula il tempo e si
//! eseguono i tick interi finché non si esaurisce il tempo reale concesso al
//! frame (`TICK_BUDGET`): l'arretrato oltre il budget si scarta, così anche le
//! velocità più alte non bloccano il gioco. `SimPerf` riporta la velocità
//! effettiva, mostrata nel pannello del tempo quando la sim non tiene il passo.

use std::time::{Duration, Instant};

use bevy::prelude::*;
use sim::{UtilityBrain, World};

use crate::state::{Sim, SimClock, SimPerf};

// --- Parametri della partita -----------------------------------------------

/// Seme del mondo e del cervello degli NPC.
pub const SIM_SEED: u64 = 42;
/// Numero di carrozze del treno.
pub const SIM_CARRIAGES: usize = 20;
/// Popolazione iniziale.
pub const SIM_NPCS: usize = 400;

/// Tempo reale massimo speso a simulare in un frame: oltre, l'arretrato si
/// scarta (a 60 fps un frame dura ~16.7 ms, il resto serve a disegnare).
const TICK_BUDGET: Duration = Duration::from_millis(9);
/// Costante di tempo (secondi) della media mobile della velocità effettiva.
const PERF_SMOOTHING_SECS: f32 = 0.5;
/// Sotto questa frazione di tick eseguiti rispetto a quelli richiesti la sim
/// "non tiene il passo".
const BEHIND_THRESHOLD: f32 = 0.95;

/// Velocità selezionabili (minuti di gioco per secondo reale), tasti 1-5.
pub const SPEEDS: [f32; 5] = [1.0, 10.0, 60.0, 600.0, 3000.0];
const SPEED_KEYS: [KeyCode; 5] = [
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
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

/// P = pausa/riprendi, 1-5 = 1, 10, 60, 600, 3000 minuti al secondo.
fn time_shortcuts(keys: Res<ButtonInput<KeyCode>>, mut clock: ResMut<SimClock>) {
    if keys.just_pressed(KeyCode::KeyP) {
        clock.paused = !clock.paused;
    }
    for (key, speed) in SPEED_KEYS.into_iter().zip(SPEEDS) {
        if keys.just_pressed(key) {
            clock.minutes_per_second = speed;
        }
    }
}

fn advance_sim(
    time: Res<Time>,
    mut clock: ResMut<SimClock>,
    mut sim: ResMut<Sim>,
    mut perf: ResMut<SimPerf>,
    mut dropped: Local<f32>,
) {
    let dt = time.delta_secs();
    if clock.paused {
        if perf.effective != 0.0 || perf.behind {
            *perf = SimPerf::default();
        }
        return;
    }
    let wanted = accumulate(&mut clock, dt);
    let done = if wanted == 0 {
        0
    } else {
        let Sim { world, brain } = &mut *sim;
        let start = Instant::now();
        run_with_budget(
            wanted,
            || start.elapsed() >= TICK_BUDGET,
            || world.tick(brain),
        )
    };
    // Medie mobili: velocità effettiva e frazione di arretrato scartata.
    let k = if dt > 0.0 {
        1.0 - (-dt / PERF_SMOOTHING_SECS).exp()
    } else {
        0.0
    };
    let rate = if dt > 0.0 { done as f32 / dt } else { 0.0 };
    let lost = if wanted > 0 {
        1.0 - done as f32 / wanted as f32
    } else {
        0.0
    };
    *dropped += (lost - *dropped) * k;
    perf.effective += (rate - perf.effective) * k;
    let behind = *dropped > 1.0 - BEHIND_THRESHOLD;
    if perf.behind != behind {
        perf.behind = behind;
    }
}

/// Aggiunge il tempo del frame all'accumulatore e restituisce i tick interi da
/// eseguire; nell'accumulatore resta solo la frazione di tick.
fn accumulate(clock: &mut SimClock, dt: f32) -> u32 {
    clock.accumulator += dt * clock.minutes_per_second.max(0.0);
    let whole = clock.accumulator.floor();
    clock.accumulator -= whole;
    whole.min(u32::MAX as f32) as u32
}

/// Esegue fino a `ticks` tick, fermandosi appena `over_budget` diventa vero
/// (controllato dopo ogni tick, quindi almeno un tick viene sempre eseguito).
/// Restituisce i tick eseguiti; gli altri vanno scartati.
fn run_with_budget(
    ticks: u32,
    mut over_budget: impl FnMut() -> bool,
    mut tick: impl FnMut(),
) -> u32 {
    let mut done = 0;
    while done < ticks {
        tick();
        done += 1;
        if over_budget() {
            break;
        }
    }
    done
}

/// Secondi reali necessari per un anno di gioco alla velocità data.
pub fn seconds_per_year(minutes_per_second: f32, days_per_year: u32) -> f32 {
    let minutes = f64::from(days_per_year.max(1)) * sim::MINUTES_PER_DAY as f64;
    (minutes / f64::from(minutes_per_second.max(f32::MIN_POSITIVE))) as f32
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
    fn returns_the_whole_backlog_and_keeps_the_fraction() {
        let mut clock = SimClock {
            minutes_per_second: 3000.0,
            ..default()
        };
        assert_eq!(accumulate(&mut clock, 5.0), 15_000);
        assert!(clock.accumulator < 1.0);
    }

    #[test]
    fn stops_ticking_when_the_budget_is_spent() {
        let mut ticks = 0;
        let mut checks = 0;
        // Il budget finisce dopo il terzo tick: il resto si scarta.
        let done = run_with_budget(
            100,
            || {
                checks += 1;
                checks >= 3
            },
            || ticks += 1,
        );
        assert_eq!((done, ticks), (3, 3));
        // Con budget abbondante si eseguono tutti.
        let mut ticks = 0;
        assert_eq!(run_with_budget(50, || false, || ticks += 1), 50);
        assert_eq!(ticks, 50);
        assert_eq!(run_with_budget(0, || true, || panic!()), 0);
    }

    #[test]
    fn a_year_lasts_a_few_seconds_at_top_speed() {
        // 12 giorni da 1440 minuti a 3000 minuti al secondo.
        assert!((seconds_per_year(3000.0, 12) - 5.76).abs() < 1e-3);
        assert!((seconds_per_year(600.0, 12) - 28.8).abs() < 1e-3);
    }
}
