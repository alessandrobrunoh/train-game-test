//! Salute, ferite e combattimento.

use serde_json::Value;
use sim::{
    Action, AttackError, CarriageKind, DeathCause, EventKind, Fighter, Grudge, GrudgeReason,
    MAX_HEALTH, Motive, NpcId, Personality, Place, Reaction, SimParams, StationKind, Stats,
    UtilityBrain, World,
};

const DAY: u64 = 24 * 60;

/// Keys added with health and combat: stripped before comparing a world with
/// the fingerprint taken before they existed.
const NEW_KEYS: &[&str] = &[
    // Npc and PlayerCharacter
    "health",
    "injury",
    "grudges",
    "violence",
    "last_attacker",
    "fainted",
    "dead",
    // World
    "combat_rng",
    "fights",
    "combat",
    // SimParams
    "permadeath",
    "heal_per_minute",
    "bed_heal_factor",
    "fed_heal_factor",
    "te_heal",
    "starvation_grace_minutes",
    "hurt_below",
    "hurt_walk_factor",
    "hurt_work_factor",
    "bedridden_below",
    "bleed_below",
    "bleed_per_minute",
    "blow_damage",
    "weapon_bonus",
    "hit_chance",
    "fight_minutes",
    "quarrel_fight_chance",
    "grudge_attack_per_hour",
    "thief_attack_chance",
    "robbery_per_hour",
    "grudge_victim",
    "grudge_loved_ones",
    "grudge_decay_per_day",
    "attack_affinity",
    "witness_affinity",
    "violence_per_fight",
    "violence_per_kill",
    "violence_decay_per_day",
    "faint_health",
    "faint_token_share",
];

/// FNV-1a of a string.
fn fnv(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Removes the new keys everywhere in `v`.
fn strip(v: &mut Value) {
    match v {
        Value::Object(map) => {
            for key in NEW_KEYS {
                map.remove(*key);
            }
            // Deaths per cause: only the causes that existed before.
            if let Some(Value::Array(causes)) = map.get_mut("deaths_by_cause") {
                causes.truncate(2);
            }
            for value in map.values_mut() {
                strip(value);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(strip),
        _ => {}
    }
}

/// Fingerprint of a world without what health and combat added.
fn fingerprint(w: &World) -> u64 {
    let mut v = serde_json::to_value(w).expect("the world serializes");
    strip(&mut v);
    fnv(&v.to_string())
}

fn index(w: &World, id: NpcId) -> usize {
    w.npcs.iter().position(|n| n.id == id).expect("alive")
}

fn events<'a>(w: &'a World, pred: impl Fn(&EventKind) -> bool + 'a) -> Vec<&'a EventKind> {
    w.events
        .iter()
        .map(|e| &e.kind)
        .filter(|k| pred(k))
        .collect()
}

/// Adults of 20–50 (indices), awake.
fn adults(w: &World) -> Vec<usize> {
    (0..w.npcs.len())
        .filter(|&i| (20..=50).contains(&w.npcs[i].age) && w.npcs[i].is_awake())
        .collect()
}

/// Puts NPC `i` idle in `place` for `minutes` (it won't decide meanwhile).
fn park(w: &mut World, i: usize, place: Place, minutes: u64) {
    let now = w.clock;
    let npc = &mut w.npcs[i];
    npc.carriage = place.carriage;
    npc.floor = place.floor;
    npc.action = Action::Idle;
    npc.action_since = now;
    npc.action_until = now + minutes;
}

/// A calm world: no deliberations, no NPC violence.
fn calm() -> SimParams {
    SimParams {
        deliberation_rate: 0.0,
        violence: 0.0,
        ..SimParams::default()
    }
}

/// A place where people are: a Mercato's ground floor.
fn market(w: &World) -> Place {
    let carriage = w
        .carriages
        .iter()
        .find(|c| c.kind == CarriageKind::Mercato)
        .expect("a Mercato")
        .id;
    Place { carriage, floor: 0 }
}

/// With `violence` 0 nothing changes: world generation and 60 days of life
/// match the fingerprint taken before health and combat existed (the new
/// fields stripped), and no NPC ever starts a fight.
#[test]
fn without_violence_the_world_is_the_same_as_before() {
    let params = SimParams {
        violence: 0.0,
        ..SimParams::default()
    };
    let mut w = World::generate_with_params(42, 20, 400, params);
    assert_eq!(fingerprint(&w), 0x8c26_2c6a_5c22_b618, "generation changed");
    let mut brain = UtilityBrain::new(42);
    w.run(&mut brain, 60 * DAY);
    assert_eq!(fingerprint(&w), 0x3cd4_b887_2306_8cb3, "60 days changed");
    assert_eq!(w.combat.total_fights(), 0);
    assert!(w.fights().is_empty());
    assert!(w.npcs.iter().all(|n| n.grudges.is_empty()));
    assert_eq!(w.life.deaths_by_cause[DeathCause::Starvation.index()], 0);
}

/// Blows of the player take health away; at 0 the NPC dies of violence,
/// killed by the player; it becomes news and its loved ones want revenge.
#[test]
fn damage_kills_with_the_right_cause() {
    let mut w = World::generate_with_params(3, 10, 80, calm());
    let place = market(&w);
    let victim = *adults(&w)
        .iter()
        .find(|&&i| w.npcs[i].partner().is_some())
        .expect("an adult in a couple");
    let id = w.npcs[victim].id;
    let partner = w.npcs[victim].partner().unwrap();
    park(&mut w, victim, place, 600);
    w.npcs[victim].health = 20.0;
    w.set_player_place(place);
    let mut brain = UtilityBrain::new(3);
    let mut blows = 0;
    while w.npc(id).is_some() {
        match w.player_attack(id) {
            Ok(out) => {
                assert!(out.damage >= 0.0 && out.health <= MAX_HEALTH);
                if out.killed {
                    assert_eq!(out.health, 0.0);
                }
            }
            // It fled or walked away: bring it back.
            Err(AttackError::NotHere) => {
                let i = index(&w, id);
                park(&mut w, i, place, 600);
            }
            Err(e) => panic!("{e}"),
        }
        w.tick(&mut brain);
        blows += 1;
        assert!(blows < 100, "never died");
    }
    let died = events(
        &w,
        |k| matches!(k, EventKind::NpcDied { npc, .. } if *npc == id),
    );
    assert!(
        matches!(
            died[..],
            [EventKind::NpcDied {
                cause: DeathCause::Violence,
                ..
            }]
        ),
        "{died:?}"
    );
    let killed = events(&w, |k| matches!(k, EventKind::Killed { .. }));
    assert!(
        matches!(killed[..], [EventKind::Killed { killer: Fighter::Player, victim: Fighter::Npc(v), .. }] if *v == id),
        "{killed:?}"
    );
    assert_eq!(w.life.deaths_by_cause[DeathCause::Violence.index()], 1);
    assert_eq!(w.combat.killed, 1);
    // The partner wants revenge on the player, and the player is feared.
    let p = w.npc(partner).unwrap();
    let g = p.grudge_against(Fighter::Player).expect("a grudge");
    assert_eq!(g.reason, GrudgeReason::KilledLovedOne(id));
    assert!(w.player.violence > 0.5, "{}", w.player.violence);
    assert!(w.player.reputation() >= sim::Reputation::Violent);
}

/// An NPC with a grudge attacks its target when they meet; a target already
/// at death's door dies, killed by the NPC. Witnesses like the attacker less.
#[test]
fn a_grudge_can_kill() {
    let params = SimParams {
        grudge_attack_per_hour: 1.0,
        violence: 1.0,
        ..calm()
    };
    let mut w = World::generate_with_params(5, 10, 80, params);
    let place = market(&w);
    let people = adults(&w);
    let (a, v, witness) = (people[0], people[1], people[2]);
    let (ida, idv, idw) = (w.npcs[a].id, w.npcs[v].id, w.npcs[witness].id);
    for i in [a, v, witness] {
        park(&mut w, i, place, 600);
    }
    // A hot-head with a deep grudge.
    let now = w.clock;
    let attacker = &mut w.npcs[a];
    attacker.traits.boldness = 1.0;
    attacker.traits.honesty = 0.0;
    attacker.personality = Some(Personality::default());
    attacker.grudges.push(Grudge {
        against: Fighter::Npc(idv),
        reason: GrudgeReason::Attacked,
        strength: 1.0,
        since: now,
    });
    w.npcs[v].health = 3.0;
    let before = w.npcs[witness].affinity(ida);
    let mut brain = UtilityBrain::new(5);
    for _ in 0..3 * 60 {
        w.tick(&mut brain);
        if w.npc(idv).is_none() {
            break;
        }
    }
    assert!(w.npc(idv).is_none(), "the victim survived");
    let killed = events(&w, |k| matches!(k, EventKind::Killed { .. }));
    assert!(
        matches!(killed[..], [EventKind::Killed { killer: Fighter::Npc(k), .. }] if *k == ida),
        "{killed:?}"
    );
    assert!(matches!(
        events(&w, |k| matches!(k, EventKind::Attacked { first: true, .. }))[..],
        [EventKind::Attacked {
            motive: Motive::Grudge,
            ..
        }]
    ));
    assert_eq!(w.life.deaths_by_cause[DeathCause::Violence.index()], 1);
    let w_after = w.npc(idw).unwrap().affinity(ida);
    assert!(w_after < before, "witness {before} -> {w_after}");
    assert!(w.npc(ida).unwrap().violence > 0.5);
    assert!(
        w.npc(ida).unwrap().grudges.is_empty(),
        "grudge on the dead forgotten"
    );
}

/// Badly wounded out of bed, an NPC bleeds and can die of its wounds (the
/// last attacker counts as its killer).
#[test]
fn wounds_can_kill() {
    let params = SimParams {
        bleed_per_minute: 1.0,
        ..calm()
    };
    let mut w = World::generate_with_params(9, 10, 60, params);
    let people = adults(&w);
    let (i, other) = (people[0], w.npcs[people[1]].id);
    let id = w.npcs[i].id;
    let place = Place {
        carriage: w.npcs[i].carriage,
        floor: w.npcs[i].floor,
    };
    park(&mut w, i, place, 600);
    let npc = &mut w.npcs[i];
    npc.health = 3.0;
    npc.injury = 50.0;
    npc.last_attacker = Some(Fighter::Npc(other));
    let mut brain = UtilityBrain::new(9);
    w.run(&mut brain, 5);
    assert!(w.npc(id).is_none());
    assert_eq!(w.life.deaths_by_cause[DeathCause::Wounds.index()], 1);
    assert!(matches!(
        events(&w, |k| matches!(k, EventKind::Killed { .. }))[..],
        [EventKind::Killed { killer: Fighter::Npc(k), .. }] if *k == other
    ));
}

/// Health comes back over time, three times as fast asleep in a bed.
#[test]
fn health_heals_over_time_and_faster_in_bed() {
    let mut w = World::generate_with_params(11, 10, 60, calm());
    let dorm = w
        .carriages
        .iter()
        .find(|c| c.kind == CarriageKind::Dormitorio)
        .unwrap()
        .id;
    let people = adults(&w);
    let (awake, asleep) = (people[0], people[1]);
    let place = Place {
        carriage: dorm,
        floor: 0,
    };
    park(&mut w, awake, place, 600);
    park(&mut w, asleep, place, 600);
    let bed = w.carriages[dorm.index()]
        .free_station_for(StationKind::Bed, w.npcs[asleep].id)
        .unwrap();
    w.carriages[dorm.index()].stations[bed.index()].occupancy += 1;
    w.npcs[asleep].floor = w.carriages[dorm.index()].floor_of(bed);
    w.npcs[asleep].action = Action::Sleep(bed);
    for i in [awake, asleep] {
        w.npcs[i].health = 50.0;
        w.npcs[i].injury = 50.0;
        w.npcs[i].needs.hunger = 1.0;
    }
    let (ida, ids) = (w.npcs[awake].id, w.npcs[asleep].id);
    let mut brain = UtilityBrain::new(11);
    w.run(&mut brain, 100);
    let (a, s) = (w.npc(ida).unwrap(), w.npc(ids).unwrap());
    let gained = |n: &sim::Npc| n.health - 50.0;
    assert!(gained(a) > 0.5, "awake {}", a.health);
    assert!(
        gained(s) > 2.5 * gained(a),
        "asleep {} awake {}",
        s.health,
        a.health
    );
    assert!(s.injury < 50.0 && s.injury <= MAX_HEALTH - s.health);
}

/// Hunger at 0 drains health after a grace period and kills at
/// `starvation_minutes`, of hunger.
#[test]
fn starvation_drains_health_then_kills() {
    let mut w = World::generate_with_params(13, 10, 60, calm());
    let i = adults(&w)[0];
    let id = w.npcs[i].id;
    let place = Place {
        carriage: w.npcs[i].carriage,
        floor: w.npcs[i].floor,
    };
    let (grace, total) = (
        w.params.starvation_grace_minutes,
        w.params.starvation_minutes,
    );
    park(&mut w, i, place, total + 10);
    // A very long walk: nobody pulls it into a chat or a meal.
    let to = sim::CarriageId((place.carriage.0 + 1) % 10);
    w.npcs[i].action = Action::Travel { to };
    // Nobody to bring food, nothing to eat.
    w.npcs[i].relations.clear();
    w.npcs[i].inventory.items = sim::SlotInventory::new(sim::NPC_ITEM_SLOTS);
    w.npcs[i].needs.hunger = 0.0;
    let mut brain = UtilityBrain::new(13);
    w.run(&mut brain, grace);
    assert_eq!(
        w.npc(id).unwrap().health,
        MAX_HEALTH,
        "still in the grace period"
    );
    w.run(&mut brain, (total - grace) / 2);
    let h = w.npc(id).unwrap().health;
    assert!((40.0..60.0).contains(&h), "{h}");
    w.run(&mut brain, (total - grace) / 2 + 5);
    assert!(w.npc(id).is_none(), "still alive");
    assert_eq!(w.life.deaths_by_cause[DeathCause::Starvation.index()], 1);
}

/// A badly hurt NPC goes home to bed and doesn't work; hungry, its family
/// brings it food.
#[test]
fn the_bedridden_go_home_and_get_fed() {
    let mut w = World::generate_with_params(17, 10, 80, calm());
    let i = *adults(&w)
        .iter()
        .find(|&&i| w.npcs[i].job.is_some() && w.npcs[i].partner().is_some())
        .unwrap();
    let id = w.npcs[i].id;
    let away = w
        .carriages
        .iter()
        .find(|c| c.id != w.npcs[i].home && c.kind != CarriageKind::Dormitorio)
        .unwrap()
        .id;
    park(
        &mut w,
        i,
        Place {
            carriage: away,
            floor: 0,
        },
        1,
    );
    let npc = &mut w.npcs[i];
    npc.health = 15.0;
    npc.injury = 85.0;
    npc.needs.hunger = 0.3;
    let home = npc.home;
    let mut brain = UtilityBrain::new(17);
    let mut worked = false;
    let mut slept_home = false;
    for _ in 0..8 * 60 {
        w.tick(&mut brain);
        let n = w.npc(id).unwrap();
        worked |= matches!(n.action, Action::Work(_));
        slept_home |= n.carriage == home && matches!(n.action, Action::Sleep(_));
    }
    assert!(!worked);
    assert!(slept_home, "never in bed at home");
    assert!(w.combat.meals_brought > 0, "nobody brought food");
    assert!(w.npc(id).unwrap().health > 15.0);
}

/// The victim and its loved ones hold a grudge against the player,
/// witnesses and the victim like the player less.
#[test]
fn attacking_makes_enemies() {
    let mut w = World::generate_with_params(21, 10, 80, calm());
    let place = market(&w);
    let people = adults(&w);
    let victim = *people
        .iter()
        .find(|&&i| w.npcs[i].partner().is_some())
        .unwrap();
    let witness = *people
        .iter()
        .find(|&&i| i != victim && Some(w.npcs[i].id) != w.npcs[victim].partner())
        .unwrap();
    let (id, idw) = (w.npcs[victim].id, w.npcs[witness].id);
    let partner = w.npcs[victim].partner().unwrap();
    park(&mut w, victim, place, 600);
    park(&mut w, witness, place, 600);
    w.set_player_place(place);
    assert!(!w.is_hostile_to_player(id));
    let mut brain = UtilityBrain::new(21);
    let mut dealt = 0.0;
    let mut first = None;
    while dealt < 15.0 {
        let out = w.player_attack(id).expect("in reach");
        first = first.or(out.reaction);
        dealt += out.damage;
        // Keep it here whatever it decided.
        let i = index(&w, id);
        park(&mut w, i, place, 600);
    }
    assert!(first.is_some(), "no reaction to the first blow");
    // The fight ends once the player stops.
    w.run(&mut brain, w.params.fight_minutes + 2);
    assert!(w.fight_of(Fighter::Player).is_none());
    let v = w.npc(id).unwrap();
    assert!(v.grudge_against(Fighter::Player).is_some());
    assert!(v.player_affinity() < -0.3, "{}", v.player_affinity());
    assert!(v.health < MAX_HEALTH && v.last_attacker == Some(Fighter::Player));
    let p = w.npc(partner).unwrap();
    assert!(
        matches!(p.grudge_against(Fighter::Player), Some(g) if g.reason == GrudgeReason::HurtLovedOne(id))
    );
    assert!(p.player_affinity() < 0.0);
    assert!(w.npc(idw).unwrap().player_affinity() < 0.0, "the witness");
    assert!(w.is_hostile_to_player(id));
    assert_eq!(w.combat.fights[Motive::Player.index()], 1);
    // Hitting back at who is hostile is self-defense.
    let i = index(&w, id);
    park(&mut w, i, place, 600);
    w.player_attack(id).unwrap();
    assert_eq!(w.combat.fights[Motive::Defense.index()], 1);
    // Out of reach.
    let far = Place {
        carriage: sim::CarriageId((place.carriage.0 + 2) % 10),
        floor: 0,
    };
    w.set_player_place(far);
    assert_eq!(w.player_attack(id), Err(AttackError::NotHere));
}

/// Knocked down, the player faints: it loses a share of its tokens (and its
/// most valuable thing) to the attacker and wakes up in its cabin the next
/// morning with little health. Money is conserved.
#[test]
fn the_player_faints_and_wakes_up_in_the_cabin() {
    let params = SimParams {
        grudge_attack_per_hour: 1.0,
        violence: 1.0,
        ..calm()
    };
    let mut w = World::generate_with_params(23, 10, 80, params);
    let place = market(&w);
    let a = adults(&w)[0];
    let ida = w.npcs[a].id;
    park(&mut w, a, place, 600);
    let now = w.clock;
    let attacker = &mut w.npcs[a];
    attacker.traits.boldness = 1.0;
    attacker.traits.honesty = 0.0;
    attacker.personality = Some(Personality::default());
    attacker.grudges.push(Grudge {
        against: Fighter::Player,
        reason: GrudgeReason::Attacked,
        strength: 1.0,
        since: now,
    });
    let tokens = w.npcs[a].inventory.tokens;
    w.player.tokens = 100;
    w.player.inventory.add(sim::ItemKind::Attrezzo, 1);
    w.player.health = 4.0;
    w.set_player_place(place);
    let money = w.money_supply();
    let mut brain = UtilityBrain::new(23);
    for _ in 0..3 * 60 {
        w.tick(&mut brain);
        if w.player.fainted.is_some() {
            break;
        }
    }
    assert!(w.player.fainted.is_some(), "never fainted");
    let fainted = events(&w, |k| matches!(k, EventKind::Fainted { .. }));
    assert!(
        matches!(fainted[..], [EventKind::Fainted { by: Some(b), tokens: 25, item: Some(it), dead: false, .. }] if *b == ida && *it == sim::ItemKind::Attrezzo),
        "{fainted:?}"
    );
    assert_eq!(w.player.tokens, 75);
    assert_eq!(w.npc(ida).unwrap().inventory.tokens, tokens + 25);
    assert_eq!(w.money_supply(), money);
    let cabin = w.player.home.unwrap();
    assert_eq!(w.player.place, cabin.place());
    assert!(w.player.is_down());
    assert!((w.params.faint_health..w.params.faint_health + 1.0).contains(&w.player.health));
    let wake = w.player.asleep_until.unwrap();
    assert_eq!((wake.hour(), wake.minute()), (w.params.wake_hour, 0));
    assert!(wake > w.clock);
    assert_eq!(w.player_attack(ida), Err(AttackError::Down));
    // The world runs on; the player wakes up in the morning.
    let left = wake.since(w.clock);
    w.run(&mut brain, left + 1);
    assert!(!w.player.is_asleep() && !w.player.is_down());
    assert!(w.player.health > w.params.faint_health);
}

/// With "morte permanente" the player dies instead.
#[test]
fn permadeath_ends_the_game() {
    let params = SimParams {
        grudge_attack_per_hour: 1.0,
        violence: 1.0,
        permadeath: true,
        ..calm()
    };
    let mut w = World::generate_with_params(29, 10, 60, params);
    let place = market(&w);
    let a = adults(&w)[0];
    let id = w.npcs[a].id;
    park(&mut w, a, place, 600);
    let now = w.clock;
    let attacker = &mut w.npcs[a];
    attacker.traits.boldness = 1.0;
    attacker.traits.honesty = 0.0;
    attacker.personality = Some(Personality::default());
    attacker.grudges.push(Grudge {
        against: Fighter::Player,
        reason: GrudgeReason::Attacked,
        strength: 1.0,
        since: now,
    });
    w.set_player_place(place);
    w.player.health = 0.5;
    let mut brain = UtilityBrain::new(29);
    for _ in 0..3 * 60 {
        w.tick(&mut brain);
        if w.player.dead.is_some() {
            break;
        }
    }
    assert!(w.player.dead.is_some(), "the player never died");
    assert!(matches!(
        events(&w, |k| matches!(k, EventKind::Fainted { .. }))[..],
        [EventKind::Fainted { dead: true, .. }]
    ));
    assert!(w.player.is_down());
    assert_eq!(w.player_attack(id), Err(AttackError::Down));
}

/// With the default violence a train of 400 sees a handful of fights in 30
/// days, few deaths, and nobody starves.
#[test]
fn default_violence_is_rare() {
    let mut w = World::generate(42, 20, 400);
    let mut brain = UtilityBrain::new(42);
    w.run(&mut brain, 30 * DAY);
    let c = &w.combat;
    let fights = c.npc_fights();
    assert!((1..=30).contains(&fights), "{fights} fights: {c:?}");
    let violent = w.life.deaths_by_cause[DeathCause::Violence.index()]
        + w.life.deaths_by_cause[DeathCause::Wounds.index()];
    assert!(violent <= 2, "{violent} violent deaths");
    assert_eq!(w.life.deaths_by_cause[DeathCause::Starvation.index()], 0);
    let firsts = events(&w, |k| matches!(k, EventKind::Attacked { first: true, .. })).len();
    assert!(firsts > 0);
    assert!(c.fought_back + c.fled + c.gave_in > 0, "{c:?}");
    let s = Stats::of(&w);
    assert!(s.population > 380, "{s}");
}

/// Fights are deterministic, and a world saved in the middle of one
/// continues exactly as the original.
#[test]
fn fights_are_deterministic_and_survive_save_load() {
    let params = SimParams {
        violence: 5.0,
        ..SimParams::default()
    };
    let run = || {
        let mut w = World::generate_with_params(31, 10, 150, params.clone());
        let mut brain = UtilityBrain::new(31);
        w.run(&mut brain, 12 * DAY + 12 * 60);
        (w, brain)
    };
    let (mut a, mut brain_a) = run();
    let (b, _) = run();
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap()
    );
    assert!(a.combat.npc_fights() > 0, "{:?}", a.combat);
    // The player starts a fight, then the world is saved and loaded.
    let place = market(&a);
    let i = adults(&a)[0];
    let id = a.npcs[i].id;
    park(&mut a, i, place, 600);
    a.set_player_place(place);
    a.player_attack(id).unwrap();
    assert!(a.fights().iter().any(|f| f.is_active()));
    let saved = serde_json::to_string(&a).unwrap();
    let mut c: World = serde_json::from_str(&saved).unwrap();
    let mut brain_c = brain_a.clone();
    a.run(&mut brain_a, 2 * DAY);
    c.run(&mut brain_c, 2 * DAY);
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&c).unwrap()
    );
}

/// A reaction is one of the three, and fleeing leads away.
#[test]
fn victims_react() {
    let mut w = World::generate_with_params(37, 10, 120, calm());
    let place = market(&w);
    let mut seen = Vec::new();
    for i in adults(&w).into_iter().take(12) {
        let id = w.npcs[i].id;
        park(&mut w, i, place, 600);
        w.set_player_place(place);
        let out = w.player_attack(id).unwrap();
        let r = out.reaction.expect("a reaction");
        if let Reaction::Flee { to } = r {
            assert_ne!(to, place.carriage);
            let n = w.npc(id).unwrap();
            assert_eq!(n.action, Action::Travel { to });
        }
        seen.push(r);
    }
    assert!(
        seen.iter().any(|r| matches!(r, Reaction::Flee { .. })),
        "{seen:?}"
    );
}
