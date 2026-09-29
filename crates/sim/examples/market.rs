//! Mercati senza grafica: specialità delle carrozze e listino dei prezzi.
//!
//! `cargo run -p sim --release --example market [seed] [carrozze] [npc] [giorni]`
//! (default: 42 20 400 30). Stampa le specialità delle carrozze produttrici,
//! simula i giorni richiesti e poi il listino (oggetti × Mercati: prezzo,
//! scorte, produttore più vicino, distanza, tendenza), lo storico degli
//! ultimi giorni e quanto spesso gli scaffali sono rimasti vuoti.

use sim::{CarriageKind, MINUTES_PER_DAY, Trend, UtilityBrain, World};

fn main() {
    let args: Vec<u64> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    let arg = |i: usize, default: u64| args.get(i).copied().unwrap_or(default);
    let (seed, carriages, npcs, days) = (arg(0, 42), arg(1, 20), arg(2, 400), arg(3, 30));

    let mut world = World::generate(seed, carriages as usize, npcs as usize);
    let mut brain = UtilityBrain::new(seed);
    println!("Treno: {carriages} carrozze, {npcs} NPC, seed {seed}\nSpecialità:");
    for c in &world.carriages {
        let specialties = world.specialties(c.id);
        if specialties.is_empty() {
            continue;
        }
        let names: Vec<&str> = specialties.iter().map(|i| i.plural()).collect();
        println!("  {:<40} {}", c.label().to_string(), names.join(", "));
    }

    // Scaffali vuoti: ogni ora, per Mercato e oggetto in vendita.
    let markets = world.markets();
    let (mut samples, mut empty) = (0u64, 0u64);
    let start = std::time::Instant::now();
    for _ in 0..days * MINUTES_PER_DAY {
        world.tick(&mut brain);
        if world.clock.minute() == 0 {
            for &m in &markets {
                for item in world.catalog().sold_items() {
                    samples += 1;
                    empty += u64::from(world.carriages[m.index()].stock.count(item) == 0);
                }
            }
        }
    }
    println!(
        "\n{days} giorni simulati in {:.1?}. {}, paga {:.0}%.",
        start.elapsed(),
        world.clock,
        world.economy.pay_level * 100.0
    );

    println!("\nListino (prezzo in gettoni, scorte/capienza, produttore più vicino, tendenza):");
    let header: Vec<String> = markets
        .iter()
        .map(|&m| format!("{:<34}", world.carriage_label(m)))
        .collect();
    println!("  {:<10} {}", "", header.join(" "));
    for item in world.catalog().kinds() {
        let cells: Vec<String> = markets
            .iter()
            .map(
                |&m| match world.market_quotes(m).into_iter().find(|q| q.item == item) {
                    Some(q) => {
                        let arrow = match q.trend {
                            Trend::Up => "su",
                            Trend::Down => "giù",
                            Trend::Flat => "=",
                        };
                        let producer = q
                            .producer
                            .map_or("nessuno".to_string(), |p| format!("carr. {p}"));
                        format!(
                            "{:>3} g  {:>2}/{:<2} da {producer} a {} ({arrow})",
                            q.price, q.stock, q.cap, q.distance
                        )
                    }
                    None => "·".to_string(),
                },
            )
            .map(|cell| format!("{cell:<34}"))
            .collect();
        println!("  {:<10} {}", item.plural(), cells.join(" "));
    }

    println!("\nStorico (ultimi 10 giorni, prezzo per Mercato):");
    let history = world.price_history();
    for item in world.catalog().sold_items() {
        for (rank, &m) in markets.iter().enumerate() {
            let prices: Vec<String> = history
                .iter()
                .rev()
                .take(10)
                .rev()
                .filter_map(|s| s.price(rank, item))
                .map(|p| p.to_string())
                .collect();
            println!(
                "  {:<9} {:<34} {}",
                item.plural(),
                world.carriage_label(m),
                prices.join(" ")
            );
        }
    }
    let c = &world.economy.counters;
    println!(
        "\nScaffali vuoti: {:.1}% delle ore. Acquisti degli NPC: {} gettoni. Officine: {:.0} attrezzi, {:.0} vestiti fatti.",
        100.0 * empty as f64 / samples.max(1) as f64,
        c.purchases,
        c.attrezzi_crafted.get(),
        c.vestiti_crafted.get(),
    );
    for o in world
        .carriages
        .iter()
        .filter(|c| c.kind == CarriageKind::Officina)
    {
        println!("  {:<40} {}", o.label().to_string(), o.stock);
    }
}
