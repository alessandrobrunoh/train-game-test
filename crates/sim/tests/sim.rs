use std::collections::HashMap;

use sim::{
    Action, Brain, CarriageId, CarriageKind, DecisionRequest, EventKind, GameTime, MINUTES_PER_DAY,
    SimParams, StationId, StationKind, UtilityBrain, World,
};

const DAY: u64 = MINUTES_PER_DAY;

fn world() -> (World, UtilityBrain) {
    (World::generate(42, 10, 100), UtilityBrain::new(42))
}

fn snapshot(world: &World) -> String {
    serde_json::to_string(world).expect("world serializes")
}

/// Station occupancy must match the NPCs actually using each station.
fn assert_occupancy_consistent(world: &World) {
    let mut used: HashMap<(CarriageId, StationId), u16> = HashMap::new();
    for npc in &world.npcs {
        if let Some(s) = npc.action.station() {
            *used.entry((npc.carriage, s)).or_default() += 1;
        }
    }
    for c in &world.carriages {
        for s in &c.stations {
            let expected = used.get(&(c.id, s.id)).copied().unwrap_or(0);
            assert_eq!(
                s.occupancy, expected,
                "occupancy of {} station {:?}",
                c.name, s.id
            );
            assert!(s.occupancy <= s.capacity, "station over capacity");
        }
    }
}

#[test]
fn same_seed_same_state() {
    let (mut a, mut brain_a) = world();
    let (mut b, mut brain_b) = world();
    assert_eq!(snapshot(&a), snapshot(&b));
    a.run(&mut brain_a, 3 * DAY);
    b.run(&mut brain_b, 3 * DAY);
    assert_eq!(snapshot(&a), snapshot(&b));

    let mut c = World::generate(43, 10, 100);
    c.run(&mut UtilityBrain::new(43), 3 * DAY);
    assert_ne!(snapshot(&a), snapshot(&c));
}

#[test]
fn save_load_roundtrip_continues_identically() {
    let (mut a, mut brain_a) = world();
    a.run(&mut brain_a, DAY + 123);
    let mut b: World = serde_json::from_str(&snapshot(&a)).expect("world deserializes");
    let mut brain_b = brain_a.clone();
    a.run(&mut brain_a, DAY);
    b.run(&mut brain_b, DAY);
    assert_eq!(snapshot(&a), snapshot(&b));
}

#[test]
fn needs_stay_in_range_and_stations_consistent() {
    let (mut w, mut brain) = world();
    for t in 0..5 * DAY {
        w.tick(&mut brain);
        for npc in &w.npcs {
            let n = npc.needs;
            for v in [n.hunger, n.energy, n.social] {
                assert!((0.0..=1.0).contains(&v), "need out of range: {v}");
            }
        }
        if t % 97 == 0 {
            assert_occupancy_consistent(&w);
        }
    }
}

#[test]
fn economy_is_sustainable_for_30_days() {
    let (mut w, mut brain) = world();
    w.run(&mut brain, 30 * DAY);
    let deaths = w
        .events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::NpcDied { .. }))
        .count();
    assert_eq!(deaths, 0);
    assert_eq!(w.npcs.len(), 100);
    assert!(
        w.food_in_mense() > 50.0,
        "mense almost empty: {}",
        w.food_in_mense()
    );
    let cap = w.params.food_storage_cap * w.carriages.len() as f32;
    assert!(w.total_stock().food < cap);
    let stats = sim::Stats::of(&w);
    assert!(
        stats.avg_needs.hunger > 0.4,
        "population is hungry: {stats}"
    );
}

#[test]
fn daily_rhythm_sleep_at_night_work_by_day() {
    let (mut w, mut brain) = world();
    w.run(&mut brain, GameTime::from_dhm(3, 3, 0) - w.clock);
    let night = sim::Stats::of(&w);
    assert!(night.count(sim::ActionKind::Sleep) >= 90, "night: {night}");
    w.run(&mut brain, GameTime::from_dhm(3, 10, 30) - w.clock);
    let day = sim::Stats::of(&w);
    assert!(day.count(sim::ActionKind::Work) >= 50, "day: {day}");
    assert!(day.count(sim::ActionKind::Sleep) <= 5, "day: {day}");
}

#[test]
fn travel_moves_npc_to_destination() {
    let (mut w, mut brain) = world();
    let id = w.npcs[0].id;
    let from = w.npcs[0].carriage;
    let to = CarriageId(if from.0 < 5 { from.0 + 5 } else { from.0 - 5 });
    let minutes = w.travel_minutes(from, to);
    let now = w.clock;
    let npc = &mut w.npcs[0];
    npc.action = Action::Travel { to };
    npc.action_since = now;
    npc.action_until = now + minutes;

    w.run(&mut brain, minutes);
    assert_eq!(
        w.npc(id).unwrap().carriage,
        from,
        "arrives only when the trip ends"
    );
    w.tick(&mut brain);
    assert_eq!(w.npc(id).unwrap().carriage, to);

    // And during normal play people actually move around the train.
    let before: Vec<_> = w.npcs.iter().map(|n| n.carriage).collect();
    w.run(&mut brain, DAY / 2);
    let moved = w
        .npcs
        .iter()
        .zip(&before)
        .filter(|(n, c)| n.carriage != **c)
        .count();
    assert!(moved > 10, "only {moved} NPCs moved");
}

#[test]
fn generated_options_are_valid() {
    let (mut w, mut brain) = world();
    for _ in 0..12 {
        w.run(&mut brain, 173);
        let ids: Vec<_> = w.npcs.iter().map(|n| n.id).collect();
        for id in ids {
            let options = w.options(id);
            let npc = w.npc(id).unwrap();
            let here = &w.carriages[npc.carriage.index()];
            assert_eq!(options.first().map(|o| o.action), Some(Action::Idle));
            for o in &options {
                assert!(!o.description.is_empty());
                assert!(o.minutes > 0);
                let station = |s: StationId| here.station(s).expect("station exists");
                match o.action {
                    Action::Eat(s) => {
                        assert_eq!(here.kind, CarriageKind::Mensa);
                        assert_eq!(station(s).kind, StationKind::Table);
                        assert!(station(s).has_room());
                        assert!(here.stock.food >= w.params.food_per_meal);
                    }
                    Action::Sleep(s) => {
                        assert_eq!(station(s).kind, StationKind::Bed);
                        assert!(station(s).has_room());
                    }
                    Action::Work(s) => {
                        let job = npc.job.expect("only workers work");
                        assert!(job.in_shift(w.clock));
                        assert_eq!(npc.workplace, Some(here.id));
                        assert_eq!(station(s).kind, job.station_kind());
                        assert!(station(s).has_room());
                    }
                    Action::Travel { to } => {
                        assert_ne!(to, here.id);
                        assert!(w.carriage(to).is_some());
                        assert!(o.goal.is_some());
                        assert_eq!(o.minutes, w.travel_minutes(here.id, to).max(1));
                    }
                    Action::Socialize(other) => {
                        assert_ne!(other, id);
                        assert_eq!(w.npc(other).unwrap().carriage, here.id);
                    }
                    Action::Idle => {}
                }
            }
        }
    }
}

/// Picks nonsense indices: the sim must cope (NPCs idle briefly).
struct BrokenBrain;

impl Brain for BrokenBrain {
    fn decide(&mut self, _: &World, requests: &[DecisionRequest]) -> Vec<usize> {
        vec![usize::MAX; requests.len() / 2]
    }
}

#[test]
fn invalid_choices_fall_back_to_idle() {
    let (mut w, _) = world();
    w.run(&mut BrokenBrain, 60);
    assert!(w.npcs.iter().all(|n| n.action == Action::Idle));
}

/// Checks the batching contract: one call per tick, each NPC at most once.
struct BatchCheck {
    inner: UtilityBrain,
    calls: u64,
    last: Option<GameTime>,
}

impl Brain for BatchCheck {
    fn decide(&mut self, world: &World, requests: &[DecisionRequest]) -> Vec<usize> {
        assert_ne!(
            self.last,
            Some(world.clock),
            "decide called twice in one tick"
        );
        self.last = Some(world.clock);
        self.calls += 1;
        let mut seen = std::collections::HashSet::new();
        for r in requests {
            assert!(seen.insert(r.npc));
            assert!(!r.options.is_empty());
            assert!(r.options.iter().all(|o| !o.description.is_empty()));
        }
        self.inner.decide(world, requests)
    }
}

#[test]
fn decisions_are_batched_per_tick() {
    let (mut w, inner) = world();
    let mut brain = BatchCheck {
        inner,
        calls: 0,
        last: None,
    };
    w.run(&mut brain, DAY);
    assert!(brain.calls > 100);
}

#[test]
fn starvation_kills_and_is_logged() {
    let params = SimParams {
        food_per_farm_minute: 0.0,
        starvation_minutes: 12 * 60,
        ..SimParams::default()
    };
    let mut w = World::generate_with_params(1, 8, 30, params);
    for c in &mut w.carriages {
        c.stock.food = 0.0;
    }
    w.run(&mut UtilityBrain::new(1), 3 * DAY);
    assert!(
        w.npcs.is_empty(),
        "{} NPCs survived without food",
        w.npcs.len()
    );
    let kinds: Vec<_> = w.events.iter().map(|e| &e.kind).collect();
    assert!(kinds.iter().any(|k| matches!(k, EventKind::FoodShortage)));
    assert!(
        kinds
            .iter()
            .any(|k| matches!(k, EventKind::NpcStarving { .. }))
    );
    assert_eq!(
        kinds
            .iter()
            .filter(|k| matches!(k, EventKind::NpcDied { .. }))
            .count(),
        30
    );
    assert_occupancy_consistent(&w);
}

#[test]
fn npc_context_describes_state() {
    let (w, _) = world();
    let npc = &w.npcs[0];
    let text = w.npc_context(npc.id).unwrap();
    assert!(text.contains(&npc.name));
    assert!(text.contains("Giorno 1 06:00"));
    assert!(text.contains("carrozza"));
}
