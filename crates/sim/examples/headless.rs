//! Simulazione senza grafica con riepilogo giornaliero o annuale.
//!
//! `cargo run -p sim --release --example headless [seed] [carrozze] [npc] [giorni] [--years N]`
//! (default: 42 10 100 30). Con `--years N` (o `--anni N`) simula N anni di
//! vita e stampa una riga per anno invece che per giorno. Alla fine stampa
//! alcune conversazioni tra NPC con le loro battute.

use std::time::Instant;

use sim::{
    Action, ActionKind, Choice, ConversationCounters, DeathCause, DeliberationCounters,
    DeliberationKind, EventKind, ItemKind, LifeStage, MINUTES_PER_DAY, Needs, Stats, Tally, Tone,
    Topic, UtilityBrain, World,
};

fn main() {
    let mut args: Vec<u64> = Vec::new();
    let mut years: Option<u64> = None;
    let mut raw = std::env::args().skip(1);
    while let Some(a) = raw.next() {
        if a == "--years" || a == "--anni" {
            years = raw.next().and_then(|y| y.parse().ok());
        } else if let Ok(n) = a.parse() {
            args.push(n);
        }
    }
    let arg = |i: usize, default: u64| args.get(i).copied().unwrap_or(default);
    let (seed, carriages, npcs, days) = (arg(0, 42), arg(1, 10), arg(2, 100), arg(3, 30));

    let mut world = World::generate(seed, carriages as usize, npcs as usize);
    let mut brain = UtilityBrain::new(seed);
    if let Some(years) = years {
        run_years(&mut world, &mut brain, seed, years);
        return;
    }

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
        let talk_before = world.conversation_counters.clone();
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
            "G{day:2} | pop {:3} | saz {:.2} en {:.2} soc {:.2} | pasti {:.1} | verd {:4.0} raz {:4.0} rott {:3.0} | attr {:2}/{:3} vest {:2}/{:3} | comprati {:2}a {:2}v rotti {:2}a {:2}v | gettoni {:5} | {} | {}",
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
            talk_summary(&talk_before, &world.conversation_counters, 1),
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
    print_conversations(
        &ConversationCounters::default(),
        &world.conversation_counters,
        days,
    );
    print_samples(&world);
}

/// Compact daily line: conversations, share two-sided, share tense.
fn talk_summary(before: &ConversationCounters, after: &ConversationCounters, days: u64) -> String {
    let convs = after.conversations - before.conversations;
    let chats = after.chats - before.chats;
    let tense = after.by_tone[Tone::Tense.index()] - before.by_tone[Tone::Tense.index()];
    format!(
        "conv {:.0}/g ({:.0}% a due, {:.0}% tese)",
        convs as f64 / days.max(1) as f64,
        100.0 * convs as f64 / chats.max(1) as f64,
        100.0 * tense as f64 / convs.max(1) as f64,
    )
}

/// Conversations between two snapshots of the counters: rate, topics, tones.
fn print_conversations(before: &ConversationCounters, after: &ConversationCounters, days: u64) {
    let convs = (after.conversations - before.conversations).max(1) as f64;
    let share = |now: u64, then: u64| 100.0 * (now - then) as f64 / convs;
    let topics: Vec<String> = Topic::ALL
        .iter()
        .map(|t| {
            let k = t.index();
            format!(
                "{} {:.0}%",
                t.name(),
                share(after.by_topic[k], before.by_topic[k])
            )
        })
        .collect();
    let tones: Vec<String> = Tone::ALL
        .iter()
        .map(|t| {
            let k = t.index();
            format!(
                "{} {:.0}%",
                t.name(),
                share(after.by_tone[k], before.by_tone[k])
            )
        })
        .collect();
    println!(
        "      {} | a tavola {} a senso unico {} interrotte {} | argomenti: {} | toni: {} | battute {:.1} per conv. | pettegolezzi creduti {} | registrate {}",
        talk_summary(before, after, days),
        after.while_eating - before.while_eating,
        after.one_sided - before.one_sided,
        after.cut_short - before.cut_short,
        topics.join(" "),
        tones.join(" "),
        (after.lines - before.lines) as f64 / convs,
        after.gossip_spread - before.gossip_spread,
        after.logged - before.logged,
    );
}

/// Up to 5 recent conversations with different topics, with their lines.
fn print_samples(world: &World) {
    let name = |id: sim::NpcId| {
        world
            .npc(id)
            .map_or_else(|| format!("#{}", id.0), |n| n.name.clone())
    };
    println!("\nAlcune conversazioni recenti:");
    let mut shown: Vec<Topic> = Vec::new();
    for c in world.recent_conversations().iter().rev() {
        if shown.len() >= 5 || shown.contains(&c.topic) || c.lines.len() < 3 {
            continue;
        }
        shown.push(c.topic);
        let about = c
            .about
            .map(|id| format!(", su {}", name(id)))
            .unwrap_or_default();
        println!(
            "[{} → {:02}:{:02}] {} e {}: {} ({}{about})",
            c.since,
            c.until.hour(),
            c.until.minute(),
            name(c.a),
            name(c.b),
            c.topic.name(),
            c.tone.name(),
        );
        for l in &c.lines {
            let who = name(l.speaker);
            let first = who.split(' ').next().unwrap_or("?").to_string();
            println!(
                "   {:02}:{:02} {first}: {}",
                l.at.hour(),
                l.at.minute(),
                l.text
            );
        }
    }
}

/// Runs the world until the next midnight (the first day starts at 06:00).
fn run_day(world: &mut World, brain: &mut UtilityBrain) {
    let ticks = MINUTES_PER_DAY - u64::from(world.clock.minute_of_day());
    world.run(brain, ticks);
}

/// Simulates `years` years of life, one summary line per year.
fn run_years(world: &mut World, brain: &mut UtilityBrain, seed: u64, years: u64) {
    let s = Stats::of(world);
    println!(
        "Treno: {} carrozze, {} NPC, seed {seed}, {} giorni per anno, {} cuccette (nascite fino a {} abitanti)",
        world.carriages.len(),
        s.population,
        world.params.days_per_year,
        s.beds,
        s.max_population,
    );
    println!(
        "Per anno: popolazione (% delle cuccette), bambini/giovani/adulti/anziani, coppie, nati/morti (di fame)/nascite negate nell'anno, \
         età media, nati sul treno, scorte a fine anno (verdura, razioni), attrezzi e vestiti posseduti, gettoni degli NPC\n\
         Economia per anno: moneta totale e tesoreria, gettoni per adulto (media, mediana), indice di Gini, livello di paghe e prezzi, \
         spreco di verdura (marcita o persa a magazzino pieno) e giorni di austerità nell'anno"
    );
    println!(
        "Deliberazioni per anno: aperte per tipo (coppia/figlio/furto/protesta), proposte accettate/rifiutate/rinviate, \
         figli tentati/rimandati, furti (scoperti), aiuti dati/negati, proteste convocate/concessioni"
    );
    let start = Instant::now();
    let mut min_pop = s.population;
    let mut max_pop = s.population;
    for year in 1..=years {
        let before = world.life.clone();
        let econ_before = world.economy.counters.clone();
        let delib_before = world.deliberation_counters.clone();
        let talk_before = world.conversation_counters.clone();
        for _ in 0..world.params.days_per_year {
            run_day(world, brain);
            let pop = world.npcs.len();
            min_pop = min_pop.min(pop);
            max_pop = max_pop.max(pop);
        }
        let s = Stats::of(world);
        let life = &world.life;
        let starved = life.deaths_by_cause[DeathCause::Starvation.index()]
            - before.deaths_by_cause[DeathCause::Starvation.index()];
        println!(
            "A{year:3} | pop {:4} ({:3.0}%) | {:3} bamb {:3} giov {:3} adul {:3} anz | coppie {:3} | nati {:3} morti {:3} (fame {}) negate {:3} | età {:4.1} | nati sul treno {:3.0}% | verd {:4.0} raz {:4.0} | attr {:3} vest {:3} | gettoni {:6}",
            s.population,
            100.0 * s.population as f32 / s.beds.max(1) as f32,
            s.stage(LifeStage::Bambino),
            s.stage(LifeStage::Giovane),
            s.stage(LifeStage::Adulto),
            s.stage(LifeStage::Anziano),
            s.couples,
            life.births_total - before.births_total,
            life.deaths_total - before.deaths_total,
            starved,
            life.births_denied_total - before.births_denied_total,
            s.avg_age,
            100.0 * (s.population - s.founders) as f32 / s.population.max(1) as f32,
            s.stored.get(ItemKind::Verdura),
            s.stored.get(ItemKind::Razione),
            s.owned(ItemKind::Attrezzo),
            s.owned(ItemKind::Vestito),
            s.tokens,
        );
        let c = &world.economy.counters;
        let since = |now: Tally, then: Tally| now.get() - then.get();
        let capped = since(c.verdura_capped, econ_before.verdura_capped);
        let lost = capped + since(c.verdura_spoiled, econ_before.verdura_spoiled);
        let grown = capped + since(c.verdura_grown, econ_before.verdura_grown);
        println!(
            "      moneta {} tesoro {:6} | gettoni per adulto {:5.1} (mediana {:3}) gini {:.2} | paga {:3.0}% | verdura sprecata {:4.1}% | austerità {} giorni",
            s.money_supply,
            s.treasury,
            s.tokens_per_adult,
            s.median_tokens_per_adult,
            s.tokens_gini,
            s.pay_level * 100.0,
            100.0 * lost / grown.max(1.0),
            c.austerity_days - econ_before.austerity_days,
        );
        print_deliberations(&delib_before, &world.deliberation_counters);
        print_conversations(
            &talk_before,
            &world.conversation_counters,
            u64::from(world.params.days_per_year),
        );
        if s.population == 0 {
            break;
        }
    }
    let life = &world.life;
    println!(
        "\nSimulati {years} anni in {:.1?}. Nati {}, morti {} (vecchiaia {}, fame {}), nascite negate {}, coppie formate {}. Popolazione min {min_pop}, max {max_pop}, finale {}.",
        start.elapsed(),
        life.births_total,
        life.deaths_total,
        life.deaths_by_cause[DeathCause::OldAge.index()],
        life.deaths_by_cause[DeathCause::Starvation.index()],
        life.births_denied_total,
        life.couples_formed_total,
        world.npcs.len(),
    );
    let d = &world.deliberation_counters;
    println!(
        "Deliberazioni: {} aperte ({:.1} al giorno), {} decise dalle regole, {} dal cervello, {} annullate.",
        d.opened_total(),
        d.opened_total() as f64 / (years * u64::from(world.params.days_per_year)).max(1) as f64,
        d.by_rules.iter().sum::<u64>(),
        d.by_brain.iter().sum::<u64>(),
        d.cancelled,
    );
    print_deliberations(&DeliberationCounters::default(), d);
    println!(
        "Ultimi eventi della vita ({} eventi in totale):",
        world.events_total()
    );
    let life_events: Vec<_> = world
        .events
        .iter()
        .filter(|e| {
            !matches!(
                e.kind,
                EventKind::ItemBought { .. } | EventKind::ItemBroke { .. }
            )
        })
        .collect();
    for e in &life_events[life_events.len().saturating_sub(12)..] {
        println!("  {e}");
    }
    if let Some(npc) = world.npcs.iter().rev().find(|n| n.partner().is_some()) {
        println!("{}", world.npc_context(npc.id).unwrap_or_default());
    }
    println!("\nUltima deliberazione di ogni tipo:");
    for k in 0..DeliberationKind::COUNT {
        let Some(r) = world
            .recent_deliberations()
            .iter()
            .rev()
            .find(|r| r.deliberation.kind.index() == k)
        else {
            continue;
        };
        let d = &r.deliberation;
        println!(
            "[{}] {}\n  {}",
            d.asked,
            d.question,
            d.context.replace('\n', "\n  ")
        );
        for (i, o) in d.options.iter().enumerate() {
            let mark = if i == r.choice { "->" } else { "  " };
            println!("  {mark} {} ({})", o.description, o.choice.key());
        }
    }
    println!();
    print_conversations(
        &ConversationCounters::default(),
        &world.conversation_counters,
        years * u64::from(world.params.days_per_year),
    );
    print_samples(world);
}

/// One line of deliberation counts between two snapshots of the counters.
fn print_deliberations(before: &DeliberationCounters, after: &DeliberationCounters) {
    let opened = |k: usize| after.opened[k] - before.opened[k];
    let chosen = |c: Choice| after.chosen(c) - before.chosen(c);
    let kinds: Vec<String> = (0..DeliberationKind::COUNT)
        .map(|k| opened(k).to_string())
        .collect();
    println!(
        "      delib {:3} ({}) | coppia {}/{}/{} | figlio {}/{} | furti {} ({} scoperti) aiuti {}/{} | proteste {} concessioni {}",
        (0..DeliberationKind::COUNT).map(opened).sum::<u64>(),
        kinds.join("/"),
        chosen(Choice::Accept),
        chosen(Choice::Refuse),
        chosen(Choice::AskForTime),
        chosen(Choice::TryForChild),
        chosen(Choice::Wait),
        after.thefts - before.thefts,
        after.thefts_caught - before.thefts_caught,
        after.help_given - before.help_given,
        after.help_refused - before.help_refused,
        after.protests_called - before.protests_called,
        after.concessions - before.concessions,
    );
}
