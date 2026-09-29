//! Mense: turni, code e affollamento.

mod common;

use std::collections::HashMap;

use sim::{
    Action, ActionKind, Brain, CarriageId, CarriageKind, DecisionRequest, GameTime, Job,
    MINUTES_PER_DAY, NpcId, StationKind, UtilityBrain, World,
};

fn mensa(w: &World) -> CarriageId {
    w.carriages
        .iter()
        .find(|c| c.kind == CarriageKind::Mensa)
        .unwrap()
        .id
}

/// Occupies every table of `c` (no NPC sits there: only the counters).
fn fill_tables(w: &mut World, c: CarriageId) {
    for s in &mut w.carriages[c.index()].stations {
        if s.kind == StationKind::Table {
            s.occupancy = s.capacity;
        }
    }
}

/// Picks `Wait` whenever it is offered, otherwise idles.
struct Queuer;

impl Brain for Queuer {
    fn wants_descriptions(&self) -> bool {
        false
    }

    fn decide(&mut self, _: &World, requests: &[DecisionRequest]) -> Vec<usize> {
        requests
            .iter()
            .map(|r| {
                r.options
                    .iter()
                    .position(|o| o.action == Action::Wait)
                    .unwrap_or(0)
            })
            .collect()
    }
}

#[test]
fn households_share_a_balanced_meal_shift() {
    let w = World::generate(42, 20, 400);
    let shifts = w.params.meal_shifts;
    let mut per_home: HashMap<(CarriageId, u8), usize> = HashMap::new();
    for npc in &w.npcs {
        let s = npc.meal_shift.expect("generated NPCs have a shift");
        assert!(s < shifts);
        *per_home.entry((npc.home, s)).or_default() += 1;
        // Partners and minor children eat with the family.
        if let Some(p) = npc.partner().and_then(|p| w.npc(p)) {
            assert_eq!(p.meal_shift, npc.meal_shift, "{} / {}", npc.name, p.name);
        }
    }
    for c in w
        .carriages
        .iter()
        .filter(|c| c.kind == CarriageKind::Dormitorio)
    {
        let counts: Vec<usize> = (0..shifts)
            .map(|s| per_home.get(&(c.id, s)).copied().unwrap_or(0))
            .collect();
        let (min, max) = (counts.iter().min().unwrap(), counts.iter().max().unwrap());
        // Households are placed largest first into the emptiest shift.
        assert!(max - min <= 6, "{}: {counts:?}", c.name);
    }
}

#[test]
fn meal_times_and_lunch_breaks_follow_the_shift() {
    let w = World::generate(42, 20, 400);
    let p = &w.params;
    let at = |h: u64, m: u64| GameTime::from_dhm(2, h, m);
    assert!(p.is_meal_time(0, at(12, 10)) && !p.is_meal_time(0, at(13, 10)));
    assert!(!p.is_meal_time(2, at(12, 10)) && p.is_meal_time(2, at(13, 30)));
    assert!(p.is_meal_time(1, at(19, 45)) && !p.is_meal_time(1, at(21, 0)));
    // Each shift's lunch is its workers' break.
    for shift in 0..p.meal_shifts {
        let (start, end) = p.lunch_break(shift);
        let job = Job::Operaio;
        let t = |m: u32| GameTime::from_dhm(2, u64::from(m / 60), u64::from(m % 60));
        assert!(job.works_at(t(start - 1), (start, end)));
        assert!(!job.works_at(t(start), (start, end)));
        assert!(!job.works_at(t(end - 1), (start, end)));
        assert!(job.works_at(t(end), (start, end)));
        assert_eq!(job.minutes_left_at(t(start - 30), (start, end)), 30);
    }
    // Default break (no shift) as before.
    assert!(!Job::Contadino.in_shift(at(12, 30)) && Job::Contadino.in_shift(at(13, 0)));
}

#[test]
fn full_mensa_offers_a_queue_served_first_come_first_served() {
    let mut w = World::generate(42, 20, 400);
    let m = mensa(&w);
    fill_tables(&mut w, m);
    let ids: Vec<NpcId> = w.npcs.iter().take(3).map(|n| n.id).collect();
    for (k, id) in ids.iter().enumerate() {
        let i = w.npcs.iter().position(|n| n.id == *id).unwrap();
        let npc = &mut w.npcs[i];
        npc.carriage = m;
        npc.needs.hunger = 0.2;
        npc.action = Action::Idle;
        // They decide one after the other: the first one queues first.
        npc.action_until = w.clock + k as u64;
    }
    let options = w.options(ids[0]);
    assert!(options.iter().all(|o| !matches!(o.action, Action::Eat(_))));
    let wait = options
        .iter()
        .find(|o| o.action == Action::Wait)
        .expect("queue");
    assert!(
        wait.description.starts_with("aspetta un posto in Mensa"),
        "{}",
        wait.description
    );
    // A hungry NPC prefers queuing here to walking to another Mensa.
    let brain = UtilityBrain::new(1);
    let req = DecisionRequest {
        npc: ids[0],
        options: options.clone(),
    };
    let scores = brain.scores(&w, &req);
    let best = (0..scores.len())
        .max_by(|&a, &b| scores[a].total_cmp(&scores[b]))
        .unwrap();
    assert_eq!(options[best].action, Action::Wait, "{options:?} {scores:?}");

    let mut queuer = Queuer;
    w.run(&mut queuer, 3);
    for id in &ids {
        assert_eq!(w.npc(*id).unwrap().action, Action::Wait);
    }
    assert_eq!(w.mensa_occupancy()[0].waiting, 3);
    // One seat frees up: the first in line sits down, the others keep waiting.
    let table = w.carriages[m.index()]
        .stations
        .iter()
        .position(|s| s.kind == StationKind::Table)
        .unwrap();
    w.carriages[m.index()].stations[table].occupancy -= 1;
    w.run(&mut queuer, 1);
    assert!(matches!(w.npc(ids[0]).unwrap().action, Action::Eat(_)));
    assert_eq!(w.npc(ids[1]).unwrap().action, Action::Wait);
    assert_eq!(w.npc(ids[2]).unwrap().action, Action::Wait);
    // Patience runs out: they decide again (and queue again, at the back).
    w.run(&mut queuer, w.params.queue_patience_minutes);
    let queued = w.npcs.iter().filter(|n| n.action == Action::Wait).count();
    assert_eq!(queued, 2);
}

#[test]
fn a_mensa_out_of_razioni_offers_no_queue() {
    let mut w = World::generate(42, 20, 400);
    let m = mensa(&w);
    fill_tables(&mut w, m);
    w.carriages[m.index()]
        .stock
        .set(sim::ItemKind::Razione, 0.0);
    let id = w.npcs[0].id;
    w.npcs[0].carriage = m;
    let options = w.options(id);
    assert!(options.iter().all(|o| o.action != Action::Wait));
}

#[test]
fn going_to_eat_avoids_a_full_mensa() {
    let mut w = World::generate(42, 20, 400);
    let mense: Vec<CarriageId> = w.mensa_occupancy().iter().map(|m| m.carriage).collect();
    // From next to the first Mensa, with its tables full, the second is better.
    let here = CarriageId(mense[0].0 + 1);
    fill_tables(&mut w, mense[0]);
    let id = w.npcs[0].id;
    w.npcs[0].carriage = here;
    w.npcs[0].needs.hunger = 0.2;
    // Many already queue in the first one.
    for n in w.npcs.iter_mut().skip(1).take(30) {
        n.carriage = mense[0];
        n.action = Action::Wait;
        n.action_until = w.clock + 30;
    }
    let options = w.options(id);
    let to_eat: Vec<CarriageId> = options
        .iter()
        .filter(|o| o.goal == Some(ActionKind::Eat))
        .filter_map(|o| match o.action {
            Action::Travel { to } => Some(to),
            _ => None,
        })
        .collect();
    assert_eq!(to_eat, vec![mense[1]], "{options:?}");
}

#[test]
fn mense_stay_uncrowded_over_a_few_days() {
    let mut w = World::generate(42, 20, 400);
    let mut brain = UtilityBrain::new(42);
    // Skip the first morning (everyone wakes up hungry at once).
    w.run(
        &mut brain,
        MINUTES_PER_DAY - u64::from(w.clock.minute_of_day()),
    );
    let peaks = common::mensa_peaks(&mut w, &mut brain, 2 * MINUTES_PER_DAY);
    let seats = w.mensa_occupancy()[0].seats;
    for p in &peaks {
        assert!(p.eating <= seats);
        assert!(p.loitering <= 40, "{p:?}");
        assert!(p.present <= seats + 40, "{p:?}");
    }
    // Lunch and dinner are spread over the shifts: no Mensa fills up.
    let lunch = common::mensa_peaks_between(&mut w, &mut brain, 12 * 60, 15 * 60);
    assert!(lunch.iter().all(|p| p.eating < seats), "{lunch:?}");
}
