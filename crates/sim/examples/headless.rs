//! Simulazione senza grafica con riepilogo giornaliero.
//!
//! `cargo run -p sim --release --example headless [seed] [carrozze] [npc] [giorni]`
//! (default: 42 10 100 30)

use std::time::Instant;

use sim::{
    Action, ActionKind, EventKind, ItemKind, MINUTES_PER_DAY, Needs, Stats, UtilityBrain, World,
};

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
            "  {:<42} {:3} postazioni, {:3} residenti, {:3} lavoratori | {}",
            c.label().to_string(),
            c.stations.len(),
            residents,
            workers,
            c.stock
        );
    }
    if let Some(first) = world.npcs.first() {
        println!("\n{}\n", world.npc_context(first.id).unwrap_or_default());
    }
    println!(
        "Per giorno: medie dei bisogni, pasti per persona, scorte a mezzanotte (verdura, razioni, rottame), \
         attrezzi e vestiti (in vendita/posseduti), acquisti e rotture del giorno, gettoni totali, % del tempo per azione"
    );

    let start = Instant::now();
    let mut deaths = 0usize;
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
        let events_before = world.events_total();
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
        let mut bought = [0usize; ItemKind::COUNT];
        let mut broke = [0usize; ItemKind::COUNT];
        // Il registro tiene solo gli ultimi `max_events`: si leggono i nuovi dalla coda.
        let new_events = (world.events_total() - events_before) as usize;
        for e in &world.events[world.events.len().saturating_sub(new_events)..] {
            match e.kind {
                EventKind::NpcDied { .. } => deaths += 1,
                EventKind::ItemBought { item, .. } => bought[item.index()] += 1,
                EventKind::ItemBroke { item, .. } => broke[item.index()] += 1,
                _ => {}
            }
        }
        let (a, v) = (ItemKind::Attrezzo, ItemKind::Vestito);
        let total: usize = minutes.iter().sum::<usize>().max(1);
        let distribution: Vec<String> = ActionKind::ALL
            .iter()
            .map(|&k| {
                format!(
                    "{} {:.0}%",
                    k.name(),
                    100.0 * minutes[k as usize] as f32 / total as f32
                )
            })
            .collect();
        println!(
            "G{day:2} | pop {:3} | saz {:.2} en {:.2} soc {:.2} | pasti {:.1} | verd {:4.0} raz {:4.0} rott {:3.0} | attr {:2}/{:3} vest {:2}/{:3} | comprati {:2}a {:2}v rotti {:2}a {:2}v | gettoni {:5} | {}",
            s.population,
            needs.hunger / samples,
            needs.energy / samples,
            needs.social / samples,
            meals as f32 / s.population.max(1) as f32,
            s.stored.get(ItemKind::Verdura),
            s.stored.get(ItemKind::Razione),
            s.stored.get(ItemKind::Rottame),
            s.on_sale.count(a),
            s.owned(a),
            s.on_sale.count(v),
            s.owned(v),
            bought[a.index()],
            bought[v.index()],
            broke[a.index()],
            broke[v.index()],
            s.tokens,
            distribution.join(" "),
        );
    }
    let elapsed = start.elapsed();

    println!(
        "\nSimulati {days} giorni in {elapsed:.1?}. Morti: {deaths}, popolazione finale: {}",
        world.npcs.len()
    );
    println!("Ultimi eventi ({} in totale):", world.events_total());
    for e in world.events.iter().rev().take(10).rev() {
        println!("  {e}");
    }
    if let Some(npc) = world.npcs.first() {
        println!("{}", world.npc_context(npc.id).unwrap_or_default());
    }
}
