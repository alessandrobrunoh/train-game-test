//! Scrive lo stato del mondo in JSON dopo 30 e 120 giorni, per confrontare
//! due versioni della sim (prova che un refactoring non cambia nulla).
//!
//! `cargo run -p sim --release --example world_dump -- <cartella> [seed...]`
//! (default: seed 1 7 42 99, 20 carrozze, 250 NPC).

use sim::{UtilityBrain, World};

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(args.next().expect("cartella di uscita"));
    let mut seeds: Vec<u64> = args.filter_map(|a| a.parse().ok()).collect();
    if seeds.is_empty() {
        seeds = vec![1, 7, 42, 99];
    }
    std::fs::create_dir_all(&dir).expect("cartella");
    for seed in seeds {
        let mut world = World::generate(seed, 20, 250);
        let mut brain = UtilityBrain::new(seed);
        let mut done = 0;
        for days in [30u64, 120] {
            world.run(&mut brain, (days - done) * sim::MINUTES_PER_DAY);
            done = days;
            let json = serde_json::to_string_pretty(&world).expect("json");
            let path = dir.join(format!("seed{seed}-day{days}.json"));
            std::fs::write(&path, json).expect("scrittura");
            println!("{} ({} NPC)", path.display(), world.npcs.len());
        }
    }
}
