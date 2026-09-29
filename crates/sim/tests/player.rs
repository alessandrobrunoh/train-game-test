//! Il giocatore come personaggio (Fase 4): cabina riservata, sonno, baule,
//! affinità degli NPC, saluti, salvataggi e determinismo.

use sim::{
    Action, CarriageKind, ChestError, GREET_MINUTES, GameTime, GiveError, ItemKind,
    MINUTES_PER_DAY, Place, PlayerTie, Regard, SleepError, StationKind, UtilityBrain, World,
};

const DAY: u64 = MINUTES_PER_DAY;

fn world() -> (World, UtilityBrain) {
    (World::generate(42, 10, 200), UtilityBrain::new(42))
}

fn snapshot(w: &World) -> String {
    serde_json::to_string(w).expect("world serializes")
}

fn home(w: &World) -> Place {
    w.player.home.expect("the player has a cabin").place()
}

#[test]
fn the_player_starts_in_a_private_bed_upstairs() {
    let (w, _) = world();
    let cabin = w.player.home.unwrap();
    let c = &w.carriages[cabin.carriage.index()];
    assert_eq!(c.kind, CarriageKind::Dormitorio);
    assert_eq!(cabin.floor, c.floors() - 1);
    let bed = c.station(cabin.bed).unwrap();
    assert_eq!(bed.kind, StationKind::Bed);
    assert_eq!(bed.floor, cabin.floor);
    assert!(!bed.is_shared() && !bed.has_room());
    // Not one of the NPCs' beds.
    let shared = c
        .stations
        .iter()
        .filter(|s| s.kind == StationKind::Bed && s.is_shared())
        .count();
    assert_eq!(w.beds(cabin.carriage), shared);
    assert_eq!(w.player.place, cabin.place());
    assert!(w.player.at_home());
    assert_eq!(w.player.name, sim::DEFAULT_PLAYER_NAME);
    assert_eq!(w.player.tokens, sim::PLAYER_START_TOKENS);
    assert!(w.player.inventory.is_empty() && w.player.chest.is_empty());
    assert!(w.player.needs.is_none() && w.player.job.is_none());
}

/// Over a month (crowded dormitories, births) no NPC ever takes the
/// player's bed.
#[test]
fn no_npc_ever_sleeps_in_the_players_bed() {
    let mut w = World::generate(5, 10, 260);
    let mut brain = UtilityBrain::new(5);
    let cabin = w.player.home.unwrap();
    let mut sleepers_there = 0;
    for _ in 0..30 * 24 {
        w.run(&mut brain, 60);
        let bed = w.carriages[cabin.carriage.index()]
            .station(cabin.bed)
            .unwrap();
        assert_eq!(bed.occupancy, 0, "at {}", w.clock);
        for n in &w.npcs {
            if n.carriage == cabin.carriage {
                assert_ne!(
                    n.action,
                    Action::Sleep(cabin.bed),
                    "{} at {}",
                    n.name,
                    w.clock
                );
                sleepers_there += usize::from(matches!(n.action, Action::Sleep(_)));
            }
        }
    }
    // The dormitory itself is used.
    assert!(sleepers_there > 0);
}

#[test]
fn sleeping_fast_forwards_to_the_morning() {
    let (mut w, mut brain) = world();
    // 14:00: too early.
    w.run(&mut brain, 8 * 60);
    assert_eq!(w.player_go_to_bed(), Err(SleepError::TooEarly));
    // 21:30, but away from home.
    w.run(&mut brain, 7 * 60 + 30);
    let away = Place {
        carriage: sim::CarriageId(3),
        floor: 0,
    };
    w.set_player_place(away);
    assert_eq!(w.player_go_to_bed(), Err(SleepError::NotHome));
    w.set_player_place(home(&w));
    let wake = w.player_go_to_bed().expect("goes to bed");
    assert_eq!(wake, GameTime::from_dhm(2, 6, 0));
    assert!(w.player.is_asleep());
    assert_eq!(w.player_go_to_bed(), Err(SleepError::AlreadyAsleep));
    // The game runs the world until the wake time: the player wakes up.
    w.run(&mut brain, wake - w.clock);
    assert_eq!(w.clock, wake);
    assert_eq!((w.clock.hour(), w.clock.minute()), (6, 0));
    assert!(!w.player.is_asleep());
    // Nobody greets a sleeper, and after waking a nap is not allowed.
    assert_eq!(w.player_go_to_bed(), Err(SleepError::TooEarly));
}

#[test]
fn the_chest_is_used_only_at_home() {
    let (mut w, _) = world();
    w.player.inventory.add(ItemKind::Rottame, 14);
    assert_eq!(w.player_store(0), Ok(10));
    assert_eq!(w.player.chest.count(ItemKind::Rottame), 10);
    assert_eq!(w.player.inventory.count(ItemKind::Rottame), 4);
    assert_eq!(w.player_store(0), Err(ChestError::EmptySlot));
    assert_eq!(w.player_store(1), Ok(4));
    assert_eq!(w.player.chest.count(ItemKind::Rottame), 14);
    assert_eq!(w.player_retrieve(1), Ok(4));
    w.set_player_place(Place {
        carriage: sim::CarriageId(2),
        floor: 0,
    });
    assert_eq!(w.player_retrieve(0), Err(ChestError::NotHome));
    assert_eq!(w.player.chest.count(ItemKind::Rottame), 10);
    // A full inventory takes out only what tops up its stacks.
    w.set_player_place(home(&w));
    w.player.inventory.add(ItemKind::Attrezzo, 99);
    assert_eq!(w.player.inventory.free_slots(), 0);
    assert_eq!(w.player_retrieve(0), Ok(6));
    assert_eq!(w.player_retrieve(0), Err(ChestError::NoRoom));
    assert_eq!(w.player.chest.count(ItemKind::Rottame), 4);
}

#[test]
fn gifts_raise_and_seen_thefts_lower_the_affinity() {
    let (mut w, _) = world();
    let i = 0;
    let id = w.npcs[i].id;
    assert_eq!(w.npcs[i].regard(), Regard::Stranger);
    w.npcs[i].needs.hunger = 0.1;
    w.player.inventory.add(ItemKind::Razione, 2);
    w.player_give(id, ItemKind::Razione).unwrap();
    let after_gift = w.npcs[i].player_affinity();
    assert!((after_gift - sim::GIFT_AFFINITY).abs() < 1e-6);
    assert_eq!(w.npcs[i].regard(), Regard::Acquaintance);

    // Taking from a storage in front of people: each witness likes the player less.
    let officina = w
        .carriages
        .iter()
        .find(|c| c.kind == CarriageKind::Officina)
        .unwrap()
        .id;
    let witnesses: Vec<usize> = (0..w.npcs.len()).take(3).collect();
    for &k in &witnesses {
        let n = &mut w.npcs[k];
        n.carriage = officina;
        n.floor = 0;
        n.action = Action::Idle;
    }
    let bystander = w.npcs.iter().position(|n| n.carriage != officina).unwrap();
    assert_eq!(w.player_take(officina, ItemKind::Rottame, 1), 1);
    for &k in &witnesses {
        assert!(
            w.npcs[k].player_affinity() < if k == i { after_gift } else { 0.0 },
            "{}",
            w.npcs[k].name
        );
    }
    assert!(w.npcs[bystander].player.is_none());

    // Who distrusts the player refuses even food.
    w.npcs[i].player = Some(PlayerTie {
        affinity: -0.5,
        ..PlayerTie::default()
    });
    assert_eq!(w.npcs[i].regard(), Regard::Wary);
    assert_eq!(
        w.player_give(id, ItemKind::Razione),
        Err(GiveError::Distrust)
    );
    assert_eq!(w.player.inventory.count(ItemKind::Razione), 1);
}

#[test]
fn friends_greet_the_player_where_it_is() {
    let (mut w, mut brain) = world();
    let here = home(&w);
    // A friend idling in the cabin's room, a stranger next to it.
    let (f, s) = (0, 1);
    for k in [f, s] {
        let n = &mut w.npcs[k];
        n.carriage = here.carriage;
        n.floor = here.floor;
        n.action = Action::Idle;
        n.action_until = w.clock + 30;
    }
    w.npcs[f].player = Some(PlayerTie {
        affinity: 0.8,
        ..PlayerTie::default()
    });
    let friend = w.npcs[f].id;
    // Within a greeting period the friend says hello (only once).
    let mut greeted_at = None;
    for _ in 0..10 {
        w.tick(&mut brain);
        if let Some(g) = w.greeting_of(friend) {
            greeted_at.get_or_insert(g.since);
            assert!(g.text.contains(w.player.first_name()), "{}", g.text);
        }
    }
    let since = greeted_at.expect("the friend greeted the player");
    assert!(w.greetings().iter().all(|g| g.npc == friend));
    assert!(w.wants_to_talk(friend));
    let npc = w.npc(friend).unwrap();
    assert!(npc.action_until >= since + GREET_MINUTES || npc.action != Action::Idle);
    // A gift answers the friend: nothing more to say.
    w.player.inventory.add(ItemKind::Te, 1);
    let _ = w.player_give(friend, ItemKind::Te);
    assert!(!w.wants_to_talk(friend));
    // Not again within the cooldown.
    w.run(&mut brain, 2 * 60);
    assert!(w.greeting_of(friend).is_none() || w.npc(friend).is_none());
}

#[test]
fn player_place_is_clamped_to_the_train() {
    let (mut w, _) = world();
    w.set_player_place(Place {
        carriage: sim::CarriageId(999),
        floor: 7,
    });
    let last = w.carriages.last().unwrap();
    assert_eq!(w.player.place.carriage, last.id);
    assert!(w.player.place.floor < last.floors());
    w.set_player_name("   Ada   Lovelace  ");
    assert_eq!(w.player.name, "Ada Lovelace");
    assert_eq!(w.player.first_name(), "Ada");
}

/// The same player actions on two copies of a world give the same world;
/// a saved world (with the player) continues identically.
#[test]
fn player_actions_are_deterministic_and_saved() {
    let (mut a, mut brain_a) = world();
    let (mut b, mut brain_b) = world();
    let serra = a
        .carriages
        .iter()
        .find(|c| c.kind == CarriageKind::Serra)
        .unwrap()
        .id;
    let script = |w: &mut World, brain: &mut UtilityBrain| {
        for day in 0..3u64 {
            w.run(brain, 9 * 60);
            w.set_player_place(Place {
                carriage: serra,
                floor: 0,
            });
            w.player_take(serra, ItemKind::Verdura, 3);
            let k = (day as usize * 7) % w.npcs.len();
            w.npcs[k].needs.hunger = 0.3;
            let id = w.npcs[k].id;
            let _ = w.player_give(id, ItemKind::Verdura);
            w.set_player_place(home(w));
            let _ = w.player_store(0);
            w.run(brain, 13 * 60);
            let _ = w.player_go_to_bed();
            w.run(brain, 2 * 60);
        }
    };
    script(&mut a, &mut brain_a);
    script(&mut b, &mut brain_b);
    assert_eq!(snapshot(&a), snapshot(&b));
    assert!(a.player.chest.count(ItemKind::Verdura) > 0);
    assert!(a.npcs.iter().any(|n| n.player.is_some()));

    let mut c: World = serde_json::from_str(&snapshot(&a)).expect("world deserializes");
    assert_eq!(c.player, a.player);
    let mut brain_c = brain_a.clone();
    a.run(&mut brain_a, DAY);
    c.run(&mut brain_c, DAY);
    assert_eq!(snapshot(&a), snapshot(&c));
}
