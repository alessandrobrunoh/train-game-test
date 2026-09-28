//! Simula senza grafica registrando lo storico in SQLite, poi stampa qualche query.
//!
//! `cargo run -p history --example record [--db <cartella>] [--years N] [seed] [carrozze] [npc]`
//! (default: cartella temporanea, 20 anni, 42 10 100). Il database è
//! `<cartella>/history.sqlite`; senza `--db` viene cancellato alla fine.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use history::History;
use sim::{MINUTES_PER_DAY, UtilityBrain, World};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut dir: Option<PathBuf> = None;
    let mut years = 20u64;
    let mut nums: Vec<u64> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--db" => dir = args.next().map(PathBuf::from),
            "--years" | "--anni" => {
                years = args.next().and_then(|y| y.parse().ok()).unwrap_or(years)
            }
            _ => nums.extend(a.parse::<u64>().ok()),
        }
    }
    let arg = |i: usize, default: u64| nums.get(i).copied().unwrap_or(default);
    let (seed, carriages, npcs) = (arg(0, 42), arg(1, 10), arg(2, 100));
    let temporary = dir.is_none();
    let dir = dir.unwrap_or_else(|| {
        std::env::temp_dir().join(format!("traingame-record-{}", std::process::id()))
    });

    let mut world = World::generate(seed, carriages as usize, npcs as usize);
    let mut brain = UtilityBrain::new(seed);
    let mut history = History::open(&dir)?;
    println!(
        "Treno: {carriages} carrozze, {npcs} NPC, seed {seed}, {years} anni -> {}",
        history
            .path()
            .map_or_else(|| "(memoria)".into(), |p| p.display().to_string())
    );

    let days_per_year = world.params.days_per_year;
    let start = Instant::now();
    let mut syncing = Duration::ZERO;
    let mut slowest = Duration::ZERO;
    let mut syncs = 0u32;
    for _ in 0..years * u64::from(days_per_year) {
        world.run(&mut brain, MINUTES_PER_DAY);
        let t = Instant::now();
        let report = history.sync(&world)?;
        let spent = t.elapsed();
        syncing += spent;
        slowest = slowest.max(spent);
        syncs += 1;
        if let Some((from, to)) = report.gap {
            eprintln!("attenzione: eventi {from}..{to} persi prima della sincronizzazione");
        }
    }
    let counts = history.counts()?;
    println!(
        "Simulati {years} anni in {:.1?} ({} sincronizzazioni, media {:.2?}, massimo {:.2?} per giorno di gioco)",
        start.elapsed(),
        syncs,
        syncing / syncs.max(1),
        slowest
    );
    println!(
        "Storico: {} eventi, {} persone ({} vive, {} morte), {} coppie, {} eventi persi",
        counts.events, counts.people, counts.alive, counts.dead, counts.couples, counts.lost_events
    );
    println!(
        "Mondo:   {} eventi, {} nati, {} morti, {} coppie formate, {} vivi",
        world.events_total(),
        world.life.births_total,
        world.life.deaths_total,
        world.life.couples_formed_total,
        world.npcs.len()
    );

    let now = world.clock;
    let describe = |p: &history::PersonRow| {
        let age = p
            .age_at(now, days_per_year)
            .map_or("?".into(), |a| a.to_string());
        let dagger = if p.is_alive() { "" } else { " †" };
        format!("{} ({age}{dagger})", p.name)
    };

    println!("\nPiù figli:");
    for (p, n) in history.most_children(5)? {
        println!("  {n} figli  {}", describe(&p));
    }
    println!("Più longevi:");
    for p in history.longest_lived(5)? {
        println!("  {}", describe(&p));
    }
    println!("Famiglie più numerose:");
    for (surname, total, alive) in history.largest_families(5)? {
        println!("  {surname}: {total} persone, {alive} in vita");
    }
    println!("Morti per causa:");
    for (cause, n) in history.deaths_by_cause()? {
        println!("  {cause:?}: {n}");
    }
    println!("Nascite e morti per anno:");
    for y in history.yearly_counts(days_per_year)? {
        println!(
            "  anno {:3}: {:3} nascite {:3} morti",
            y.year, y.births, y.deaths
        );
    }

    if let Some((patriarch, _)) = history.most_children(1)?.into_iter().next() {
        println!("\nBiografia di {}:", patriarch.name);
        let bio = history.biography(patriarch.id)?;
        for e in bio
            .iter()
            .filter(|e| !matches!(e.kind.as_str(), "ItemBought" | "ItemBroke"))
        {
            println!("  [{}] {}", e.time, e.text);
        }
        if let Some(tree) = history.family_tree(patriarch.id, 2, 3)? {
            println!("Discendenti ({} persone nell'albero):", tree.len());
            print_descendants(&tree, 1, &describe);
        }
    }

    let t = Instant::now();
    let found = history.search_people("a", 50)?;
    println!(
        "\nRicerca \"a\": {} risultati in {:.2?}",
        found.len(),
        t.elapsed()
    );

    drop(history);
    if temporary {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

fn print_descendants(
    node: &history::FamilyNode,
    depth: usize,
    describe: &dyn Fn(&history::PersonRow) -> String,
) {
    for child in &node.children {
        let partners: Vec<String> = child.partners.iter().map(|p| p.name.clone()).collect();
        let partners = if partners.is_empty() {
            String::new()
        } else {
            format!(" ⚭ {}", partners.join(", "))
        };
        println!(
            "{}{}{partners}",
            "  ".repeat(depth),
            describe(&child.person)
        );
        print_descendants(child, depth + 1, describe);
    }
}
