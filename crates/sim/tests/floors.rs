//! Carrozze a più piani: letti divisi fra i piani, NPC che salgono e
//! scendono, chiacchiere solo con chi è sullo stesso piano.

use sim::{Action, CarriageKind, MINUTES_PER_DAY, StationKind, UtilityBrain, World};

fn world() -> (World, UtilityBrain) {
    (World::generate(42, 20, 400), UtilityBrain::new(42))
}

#[test]
fn dormitory_beds_are_spread_over_both_floors() {
    let (w, _) = world();
    for c in w
        .carriages
        .iter()
        .filter(|c| c.kind == CarriageKind::Dormitorio)
    {
        assert_eq!(c.floors(), 2);
        let beds = |floor| {
            c.stations
                .iter()
                .filter(|s| s.kind == StationKind::Bed && s.floor == floor)
                .count()
        };
        let (ground, upper) = (beds(0), beds(1));
        assert!(upper > 0, "{}: no beds upstairs", c.label());
        assert!(
            ground == upper || ground == upper + 1,
            "{ground} vs {upper}"
        );
    }
    // Single-floor carriages keep everything on the ground floor.
    for c in w.carriages.iter().filter(|c| c.floors() == 1) {
        assert!(c.stations.iter().all(|s| s.floor == 0), "{}", c.label());
    }
}

#[test]
fn npcs_are_on_the_floor_of_what_they_do() {
    let (mut w, mut brain) = world();
    let mut upstairs_sleepers = 0;
    for _ in 0..2 * MINUTES_PER_DAY {
        w.tick(&mut brain);
        for npc in &w.npcs {
            let c = &w.carriages[npc.carriage.index()];
            assert!(
                npc.floor < c.floors(),
                "{} on floor {}",
                npc.name,
                npc.floor
            );
            match npc.action {
                Action::Wait => assert_eq!(npc.floor, 0, "{}", npc.name),
                action => {
                    if let Some(station) = action.station() {
                        assert_eq!(npc.floor, c.floor_of(station), "{} {action:?}", npc.name);
                    }
                }
            }
            if matches!(npc.action, Action::Sleep(_)) && npc.floor == 1 {
                upstairs_sleepers += 1;
            }
        }
    }
    assert!(upstairs_sleepers > 0, "nobody ever slept upstairs");
}

#[test]
fn chats_only_between_people_on_the_same_floor() {
    let (mut w, mut brain) = world();
    let mut checked = 0;
    for _ in 0..2 * MINUTES_PER_DAY {
        w.tick(&mut brain);
        for c in w.conversations() {
            let (Some(a), Some(b)) = (w.npc(c.a), w.npc(c.b)) else {
                continue;
            };
            if a.carriage == b.carriage {
                assert_eq!(
                    a.floor, b.floor,
                    "{} and {} chat across floors",
                    a.name, b.name
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 0);
}

#[test]
fn same_place_needs_the_same_floor() {
    let (mut w, _) = world();
    let dorm = w
        .carriages
        .iter()
        .find(|c| c.kind == CarriageKind::Dormitorio)
        .map(|c| c.id)
        .unwrap();
    let (a, b) = (0, 1);
    for i in [a, b] {
        w.npcs[i].carriage = dorm;
        w.npcs[i].action = Action::Idle;
        w.npcs[i].floor = 0;
    }
    assert!(w.same_place(&w.npcs[a], &w.npcs[b]));
    w.npcs[b].floor = 1;
    assert!(!w.same_place(&w.npcs[a], &w.npcs[b]));
}

#[test]
fn coming_down_the_stairs_takes_time() {
    let (mut w, _) = world();
    let far = sim::CarriageId(10);
    w.npcs[0].carriage = sim::CarriageId(0);
    w.npcs[0].floor = 0;
    let ground = w.trip_minutes(&w.npcs[0], far);
    assert_eq!(ground, w.travel_minutes(sim::CarriageId(0), far));
    w.npcs[0].floor = 1;
    assert_eq!(
        w.trip_minutes(&w.npcs[0], far),
        ground + w.params.stairs_minutes
    );
}
