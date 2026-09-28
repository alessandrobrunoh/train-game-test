//! Simulazione senza grafica con riepilogo giornaliero.
//!
//! `cargo run -p sim --release --example headless [seed] [carrozze] [npc] [giorni]`
//! (default: 42 10 100 30)

use std::time::Instant;

use sim::{Action, ActionKind, EventKind, MINUTES_PER_DAY, Needs, Stats, UtilityBrain, World};

fn main() {
    let args: Vec<u64> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    let arg = |i: usize, default: u64| args.get(i).copied().unwrap_or(default);
    let (seed, carriages, npcs, days) = (arg(0, 42), arg(1, 10), arg(2, 100), arg(3, 30));

    let mut world = World::generate(seed, carriages as usize, npcs as usize);
    let mut brain = UtilityBrain::new(seed);

    println!("Treno: {carriages} carrozze, {npcs} NPC, seed {seed}");
    for c in &world.carriages {
        let residents = world.npcs.iter().filter(|n| n.home == c.id).count();
        let workers = world
            .npcs
            .iter()
            .filter(|n| n.workplace == Some(c.id))
            .count();
        println!(
            "  {:<42} {:3} postazioni, {:3} residenti, {:3} lavoratori",
            c.label().to_string(),
            c.stations.len(),
            residents,
            workers
        );
    }
    if let Some(first) = world.npcs.first() {
        println!("\n{}\n", world.npc_context(first.id).unwrap_or_default());
    }
    println!(
        "Per giorno: medie dei bisogni, scorte a mezzanotte, pasti per persona, % del tempo per azione"
    );

    let start = Instant::now();
    for _ in 0..days {
        let day = world.clock.day();
        let mut needs = Needs {
            hunger: 0.0,
            energy: 0.0,
            social: 0.0,
        };
        let mut minutes = [0usize; ActionKind::ALL.len()];
        let mut meals = 0usize;
        let mut samples = 0.0;
        // Run until the next midnight (the first day starts at 06:00).
        let ticks = MINUTES_PER_DAY - u64::from(world.clock.minute_of_day());
        for _ in 0..ticks {
            let now = world.clock;
            world.tick(&mut brain);
            let s = Stats::of(&world);
            needs.hunger += s.avg_needs.hunger;
            needs.energy += s.avg_needs.energy;
            needs.social += s.avg_needs.social;
            samples += 1.0;
            for (m, c) in minutes.iter_mut().zip(s.actions) {
                *m += c;
            }
            meals += world
                .npcs
                .iter()
                .filter(|n| matches!(n.action, Action::Eat(_)) && n.action_since == now)
                .count();
        }
        let s = Stats::of(&world);
        let total: usize = minutes.iter().sum::<usize>().max(1);
        let distribution: Vec<String> = ActionKind::ALL
            .iter()
            .map(|&k| {
                format!(
                    "{} {:2.0}%",
                    k.name(),
                    100.0 * minutes[k as usize] as f32 / total as f32
                )
            })
            .collect();
        println!(
            "Giorno {day:2} | pop {:4} | sazietà {:.2} energia {:.2} social {:.2} | cibo {:6.0} (mense {:5.0}) mat {:5.0} | pasti/pers {:.1} | {}",
            s.population,
            needs.hunger / samples,
            needs.energy / samples,
            needs.social / samples,
            s.food_total,
            s.food_in_mense,
            s.materials,
            meals as f32 / s.population.max(1) as f32,
            distribution.join(" "),
        );
    }
    let elapsed = start.elapsed();

    let deaths = world
        .events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::NpcDied { .. }))
        .count();
    println!(
        "\nSimulati {days} giorni in {elapsed:.1?}. Morti: {deaths}, popolazione finale: {}",
        world.npcs.len()
    );
    println!("Ultimi eventi ({} in totale):", world.events.len());
    for e in world.events.iter().rev().take(10).rev() {
        println!("  {e}");
    }
    if let Some(npc) = world.npcs.first() {
        println!("{}", world.npc_context(npc.id).unwrap_or_default());
    }
}
