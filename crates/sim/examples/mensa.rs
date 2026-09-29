//! Affollamento delle Mense per ora del giorno.
//!
//! `cargo run -p sim --release --example mensa [seed] [carrozze] [npc] [giorni]`
//! (default: 42 20 400 6). Il primo giorno (che parte alle 06:00) non conta.
//! Stampa, per la Mensa più affollata, il massimo per ora di presenti, seduti,
//! in attesa e sfaccendati; poi posti, attesa media, pasti al giorno e perché
//! la gente si trova in Mensa all'ora di punta.

use std::collections::HashMap;

use sim::{
    Action, ActionKind, Brain, CarriageId, CarriageKind, DeathCause, DecisionRequest,
    MINUTES_PER_DAY, MensaRole, NpcId, UtilityBrain, World,
};

/// Ricorda l'obiettivo dell'ultimo viaggio scelto da ogni NPC.
struct Recorder {
    inner: UtilityBrain,
    goal: HashMap<NpcId, Option<ActionKind>>,
}

impl Brain for Recorder {
    fn wants_descriptions(&self) -> bool {
        false
    }

    fn decide(&mut self, world: &World, requests: &[DecisionRequest]) -> Vec<usize> {
        let choices = self.inner.decide(world, requests);
        for (req, &c) in requests.iter().zip(&choices) {
            if let Some(o) = req.options.get(c)
                && let Action::Travel { .. } = o.action
            {
                self.goal.insert(req.npc, o.goal);
            }
        }
        choices
    }
}

#[derive(Clone, Copy, Default)]
struct Peak {
    present: usize,
    eating: usize,
    waiting: usize,
    loitering: usize,
}

fn main() {
    let args: Vec<u64> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    let arg = |i: usize, default: u64| args.get(i).copied().unwrap_or(default);
    let (seed, carriages, npcs, days) = (arg(0, 42), arg(1, 20), arg(2, 400), arg(3, 6));

    let mut world = World::generate(seed, carriages as usize, npcs as usize);
    let mut brain = Recorder {
        inner: UtilityBrain::new(seed),
        goal: HashMap::new(),
    };
    let mense: Vec<CarriageId> = world.mensa_occupancy().iter().map(|m| m.carriage).collect();
    println!("Treno: {carriages} carrozze, {npcs} NPC, seed {seed}, {days} giorni");
    for m in world.mensa_occupancy() {
        let near_home = world
            .npcs
            .iter()
            .filter(|n| nearest(&mense, n.home) == m.carriage)
            .count();
        let near_work = world
            .npcs
            .iter()
            .filter(|n| {
                n.workplace
                    .is_some_and(|w| nearest(&mense, w) == m.carriage)
            })
            .count();
        println!(
            "  {:<40} {:2} tavoli, {:3} posti | più vicina a casa di {near_home:3}, al lavoro di {near_work:3}",
            world.carriage_label(m.carriage).to_string(),
            m.tables,
            m.seats,
        );
    }

    // Salta il primo giorno (parte alle 06:00, tutti affamati insieme).
    world.run(
        &mut brain,
        MINUTES_PER_DAY - u64::from(world.clock.minute_of_day()),
    );
    let pop_start = world.npcs.len();

    let mut peaks = vec![[Peak::default(); 24]; mense.len()];
    let mut waiting_since: HashMap<NpcId, u64> = HashMap::new();
    let (mut served, mut served_minutes, mut abandoned, mut longest) = (0u64, 0u64, 0u64, 0u64);
    let mut meals = 0u64;
    let mut meals_by_hour = [0u64; 24];
    let mut person_days = 0u64;
    // Code vere (Action::Wait): ingresso in coda, esito.
    let mut queue_since: HashMap<NpcId, u64> = HashMap::new();
    let (mut q_served, mut q_minutes, mut q_left, mut q_longest, mut q_max) =
        (0u64, 0u64, 0u64, 0u64, 0usize);
    // Perché si è in Mensa alle 12:30 e alle 19:30 (somma sui giorni).
    let mut why: HashMap<(usize, &'static str), usize> = HashMap::new();
    for _ in 0..days * MINUTES_PER_DAY {
        let now = world.clock;
        world.tick(&mut brain);
        if now.minute_of_day() == 0 {
            person_days += world.npcs.len() as u64;
        }
        for (k, m) in world.mensa_occupancy().iter().enumerate() {
            let p = &mut peaks[k][now.hour() as usize];
            p.present = p.present.max(m.present);
            p.eating = p.eating.max(m.eating);
            p.waiting = p.waiting.max(m.waiting);
            p.loitering = p.loitering.max(m.loitering);
        }
        let snapshot = matches!(now.minute_of_day(), 750 | 1170);
        q_max = q_max.max(
            world
                .npcs
                .iter()
                .filter(|n| n.action == Action::Wait)
                .count(),
        );
        for npc in &world.npcs {
            if matches!(npc.action, Action::Eat(_)) && npc.action_since == now {
                meals += 1;
                meals_by_hour[now.hour() as usize] += 1;
            }
            match (npc.action, queue_since.get(&npc.id).copied()) {
                (Action::Wait, None) => {
                    queue_since.insert(npc.id, now.minutes());
                }
                (Action::Wait, Some(_)) => {}
                (Action::Eat(_), Some(since)) => {
                    let w = now.minutes() - since;
                    q_served += 1;
                    q_minutes += w;
                    q_longest = q_longest.max(w);
                    queue_since.remove(&npc.id);
                }
                (_, Some(_)) => {
                    q_left += 1;
                    queue_since.remove(&npc.id);
                }
                (_, None) => {}
            }
            let role = world.mensa_role(npc);
            match (role, waiting_since.get(&npc.id).copied()) {
                (Some(MensaRole::Waiting), None) => {
                    waiting_since.insert(npc.id, now.minutes());
                }
                (Some(MensaRole::Waiting), Some(_)) => {}
                (Some(MensaRole::Eating), Some(since)) => {
                    let w = now.minutes() - since;
                    served += 1;
                    served_minutes += w;
                    longest = longest.max(w);
                    waiting_since.remove(&npc.id);
                }
                (_, Some(_)) => {
                    abandoned += 1;
                    waiting_since.remove(&npc.id);
                }
                (_, None) => {}
            }
            if snapshot && let Some(role) = role {
                let k = mense.iter().position(|&c| c == npc.carriage).unwrap_or(0);
                let reason = match (role, brain.goal.get(&npc.id).copied().flatten()) {
                    (MensaRole::Working, _) => "cuoco al lavoro",
                    (MensaRole::Eating, _) => "seduto a mangiare",
                    (MensaRole::Waiting, _) => "affamato senza posto",
                    (_, Some(ActionKind::Eat)) => "venuto a mangiare (sazio/altro)",
                    (_, Some(ActionKind::Socialize)) => "venuto a chiacchierare",
                    (_, Some(ActionKind::Work)) => "venuto a lavorare",
                    (_, Some(ActionKind::Sleep)) => "passava per dormire",
                    (_, _) => "altro",
                };
                *why.entry((k, reason)).or_default() += 1;
            }
        }
    }

    let busiest = (0..mense.len())
        .max_by_key(|&k| peaks[k].iter().map(|p| p.present).max().unwrap_or(0))
        .unwrap_or(0);
    let info = world.mensa_occupancy()[busiest];
    println!(
        "\nMensa più affollata: {} ({} tavoli, {} posti)",
        world.carriage_label(info.carriage),
        info.tables,
        info.seats
    );
    println!("ora | presenti seduti attesa sfaccendati | pasti iniziati (tutto il treno)");
    for h in 5..23 {
        let p = peaks[busiest][h];
        println!(
            "{h:02}  | {:8} {:6} {:6} {:11} | {:5}",
            p.present, p.eating, p.waiting, p.loitering, meals_by_hour[h]
        );
    }
    println!("\nPicchi di tutte le Mense (presenti/seduti/attesa):");
    for (k, &c) in mense.iter().enumerate() {
        let max = |f: fn(&Peak) -> usize| peaks[k].iter().map(f).max().unwrap_or(0);
        println!(
            "  {:<40} {:3}/{:3}/{:3} su {} posti",
            world.carriage_label(c).to_string(),
            max(|p| p.present),
            max(|p| p.eating),
            max(|p| p.waiting),
            world.mensa_occupancy()[k].seats
        );
    }
    println!(
        "\nAttese servite: {served}, media {:.1} min, massima {longest} min; abbandonate {abandoned}",
        served_minutes as f64 / served.max(1) as f64
    );
    println!(
        "In coda (Action::Wait): massimo {q_max} insieme sul treno, serviti {q_served} (attesa media {:.1} min, massima {q_longest} min), usciti dalla coda senza mangiare {q_left}",
        q_minutes as f64 / q_served.max(1) as f64
    );
    println!(
        "Pasti per persona al giorno: {:.2} (popolazione {pop_start} -> {}), morti di fame: {}",
        meals as f64 / person_days.max(1) as f64,
        world.npcs.len(),
        world.life.deaths_by_cause[DeathCause::Starvation.index()]
    );
    println!("\nPerché si è in Mensa alle 12:30 e 19:30 (media per giorno):");
    for (k, &c) in mense.iter().enumerate() {
        let mut rows: Vec<_> = why.iter().filter(|((m, _), _)| *m == k).collect();
        rows.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
        let parts: Vec<String> = rows
            .iter()
            .map(|((_, r), n)| format!("{r} {:.1}", **n as f64 / (2 * days) as f64))
            .collect();
        println!("  {}: {}", world.carriage_label(c), parts.join(", "));
    }
    let _ = CarriageKind::Mensa;
}

fn nearest(mense: &[CarriageId], from: CarriageId) -> CarriageId {
    mense
        .iter()
        .copied()
        .min_by_key(|m| (m.distance(from), m.0))
        .unwrap_or(from)
}
