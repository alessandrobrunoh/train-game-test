//! Bande: nascita, reclutamento, pizzo, protezione, regolamenti di conti,
//! successione, scioglimento, il giocatore, salvataggi e bilanciamento.

use sim::{
    Action, CarriageId, CarriageKind, DeathCause, EventKind, Fighter, GangError, GangId, Grudge,
    GrudgeReason, LeaveReason, Motive, NpcId, Place, PlayerTie, Relation, RelationKind, SimParams,
    Stats, UtilityBrain, World,
};

const DAY: u64 = 24 * 60;

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

/// Adults of 20–50 (indices).
fn adults(w: &World) -> Vec<usize> {
    (0..w.npcs.len())
        .filter(|&i| (20..=50).contains(&w.npcs[i].age))
        .collect()
}

/// Puts NPC `i` idle in `place` for `minutes` (it won't decide meanwhile).
fn park(w: &mut World, i: usize, place: Place, minutes: u64) {
    let now = w.clock;
    let npc = &mut w.npcs[i];
    if let Some(s) = npc.action.station() {
        let station = &mut w.carriages[npc.carriage.index()].stations[s.index()];
        station.occupancy = station.occupancy.saturating_sub(1);
    }
    let npc = &mut w.npcs[i];
    npc.carriage = place.carriage;
    npc.floor = place.floor;
    npc.action = Action::Idle;
    npc.action_since = now;
    npc.action_until = now + minutes;
}

/// Ties NPCs `a` and `b` as friends with `affinity`, both ways.
fn befriend(w: &mut World, a: usize, b: usize, affinity: f32) {
    let (ida, idb) = (w.npcs[a].id, w.npcs[b].id);
    for (x, other) in [(a, idb), (b, ida)] {
        let rel = &mut w.npcs[x].relations;
        match rel.iter_mut().find(|r| r.other == other) {
            Some(r) => r.affinity = affinity,
            None => rel.push(Relation {
                other,
                kind: RelationKind::Friend,
                affinity,
            }),
        }
    }
}

/// A hot-headed, bold, not very honest character (aggression 1).
fn hothead(w: &mut World, i: usize) {
    let n = &mut w.npcs[i];
    n.traits.boldness = 1.0;
    n.traits.honesty = 0.0;
    n.personality = Some(sim::Personality::default());
}

/// A meek character (aggression 0): pays the pizzo.
fn meek(w: &mut World, i: usize) {
    let n = &mut w.npcs[i];
    n.traits.boldness = 0.0;
    n.traits.honesty = 1.0;
    n.personality = Some(sim::Personality::default());
}

/// Gangs don't form by themselves (only by the hook), nothing else moves.
fn quiet() -> SimParams {
    let mut p = SimParams {
        deliberation_rate: 0.0,
        quarrel_fight_chance: 0.0,
        grudge_attack_per_hour: 0.0,
        robbery_per_hour: 0.0,
        ..SimParams::default()
    };
    p.gang.found_per_day = 0.0;
    p.gang.recruit_per_day = 0.0;
    p.gang.leave_per_day = 0.0;
    p.gang.pizzo_per_hour = 0.0;
    p.gang.rival_fight_per_hour = 0.0;
    p.gang.hit_per_day = 0.0;
    p
}

fn first_of(w: &World, kind: CarriageKind) -> CarriageId {
    w.carriages
        .iter()
        .find(|c| c.kind == kind)
        .expect("a carriage")
        .id
}

/// A gang of `n` adults working at the first Mercato (its territory).
fn market_gang(w: &mut World, n: usize) -> (GangId, Vec<usize>, CarriageId) {
    let market = first_of(w, CarriageKind::Mercato);
    let people: Vec<usize> = adults(w).into_iter().take(n).collect();
    for &i in &people {
        hothead(w, i);
        w.npcs[i].workplace = Some(market);
    }
    for &a in &people {
        for &b in &people {
            if a < b {
                befriend(w, a, b, 0.7);
            }
        }
    }
    let ids: Vec<NpcId> = people.iter().map(|&i| w.npcs[i].id).collect();
    let gang = w.found_gang(&ids).expect("a gang");
    assert!(w.gang(gang).unwrap().holds(market));
    (gang, people, market)
}

/// With `violence` 0 (or `gangs` 0) no gang ever forms, even among the
/// sourest people.
#[test]
fn without_violence_no_gang_forms() {
    for (violence, gangs) in [(0.0, 1.0), (1.0, 0.0)] {
        let mut params = SimParams {
            violence,
            gangs,
            ..SimParams::default()
        };
        params.gang.found_per_day = 1.0;
        let mut w = World::generate_with_params(42, 20, 400, params);
        let mut brain = UtilityBrain::new(42);
        w.run(&mut brain, 60 * DAY);
        assert!(w.gangs().is_empty());
        assert_eq!(w.gang_state().counters, sim::GangCounters::default());
        assert!(events(&w, |k| matches!(k, EventKind::GangFounded { .. })).is_empty());
    }
}

/// Friends living together, poor, aggressive and holding a grudge against
/// the same person found a gang, led by one of them.
#[test]
fn a_sour_cluster_of_friends_founds_a_gang() {
    let mut params = quiet();
    params.gang.found_per_day = 1.0;
    params.gang.max_gangs = 1;
    let mut w = World::generate_with_params(11, 10, 120, params);
    let people = adults(&w);
    let (cluster, enemy) = (&people[..4], people[6]);
    let home = w.npcs[cluster[0]].home;
    let enemy_id = w.npcs[enemy].id;
    let now = w.clock;
    for &i in cluster {
        hothead(&mut w, i);
        let n = &mut w.npcs[i];
        n.home = home;
        n.inventory.tokens = 0;
        n.grudges.push(Grudge {
            against: Fighter::Npc(enemy_id),
            reason: GrudgeReason::Attacked,
            strength: 0.9,
            since: now,
        });
    }
    for &a in cluster {
        for &b in cluster {
            if a < b {
                befriend(&mut w, a, b, 0.9);
            }
        }
    }
    let ids: Vec<NpcId> = cluster.iter().map(|&i| w.npcs[i].id).collect();
    let mut brain = UtilityBrain::new(11);
    for _ in 0..20 {
        w.run(&mut brain, DAY);
        if !w.gangs().is_empty() {
            break;
        }
    }
    let g = w.gangs().first().expect("a gang was founded");
    assert!(ids.iter().all(|&id| g.is_member(id)), "{:?}", g.members);
    assert!(ids.contains(&g.leader));
    assert!(g.name.split(' ').count() >= 3, "{}", g.name);
    assert!(!g.territory.is_empty());
    let founded = events(&w, |k| matches!(k, EventKind::GangFounded { .. }));
    assert!(
        matches!(founded[..], [EventKind::GangFounded { leader, .. }] if *leader == g.leader),
        "{founded:?}"
    );
    assert_eq!(w.gang_of(ids[0]).map(|g| g.id), Some(g.id));
    assert_eq!(w.gang_state().counters.founded, 1);
}

/// A friend of the members joins; a hurt member may leave for fear.
#[test]
fn members_are_recruited_and_leave() {
    let mut params = quiet();
    params.gang.recruit_per_day = 1.0;
    let mut w = World::generate_with_params(12, 10, 120, params);
    let (gang, people, market) = market_gang(&mut w, 3);
    let recruit = adults(&w)[5];
    hothead(&mut w, recruit);
    w.npcs[recruit].workplace = Some(market);
    w.npcs[recruit].inventory.tokens = 0;
    befriend(&mut w, recruit, people[0], 0.9);
    let rid = w.npcs[recruit].id;
    let mut brain = UtilityBrain::new(12);
    for _ in 0..20 {
        w.run(&mut brain, DAY);
        if w.gang(gang).is_some_and(|g| g.size() > 3) {
            break;
        }
    }
    let g = w.gang(gang).expect("still there");
    assert!(g.size() > 3, "nobody joined");
    assert!(!events(&w, |k| matches!(k, EventKind::GangJoined { .. })).is_empty());
    assert!(g.is_member(rid) || g.size() > 3);
    // Leaving: a hurt member, with every reason to.
    w.params.gang.leave_per_day = 1.0;
    w.params.gang.recruit_per_day = 0.0;
    let leaver = w
        .gang(gang)
        .unwrap()
        .members
        .iter()
        .map(|m| m.id)
        .find(|&id| id != w.gang(gang).unwrap().leader)
        .unwrap();
    for _ in 0..10 {
        // Hurt just before midnight, when members decide.
        let wait = w.clock.next_at(0, 0).since(w.clock) - 1;
        w.run(&mut brain, wait);
        let i = index(&w, leaver);
        w.npcs[i].health = 40.0;
        w.run(&mut brain, 2);
        if w.gang_of(leaver).is_none() {
            break;
        }
    }
    assert!(w.gang_of(leaver).is_none(), "never left");
    let left = events(
        &w,
        |k| matches!(k, EventKind::GangLeft { npc: Some(id), .. } if *id == leaver),
    );
    assert!(
        matches!(
            left[..],
            [EventKind::GangLeft {
                reason: LeaveReason::Fear,
                ..
            }]
        ),
        "{left:?}"
    );
}

/// Members ask the pizzo of who works in their territory: the tokens go to
/// the gang's treasury, money is conserved, the victim holds a grudge.
#[test]
fn extortion_fills_the_treasury_and_money_is_conserved() {
    let mut params = quiet();
    params.gang.pizzo_per_hour = 1.0;
    let mut w = World::generate_with_params(13, 10, 120, params);
    let (gang, people, market) = market_gang(&mut w, 3);
    let victim = adults(&w)[6];
    meek(&mut w, victim);
    w.npcs[victim].workplace = Some(market);
    w.npcs[victim].inventory.tokens = 100;
    let vid = w.npcs[victim].id;
    let place = Place {
        carriage: market,
        floor: 0,
    };
    let money = w.money_supply();
    let mut brain = UtilityBrain::new(13);
    for _ in 0..(3 * 24) {
        for &i in &[people[0], victim] {
            let id = w.npcs[i].id;
            if w.fight_of(Fighter::Npc(id)).is_none() {
                let i = index(&w, id);
                park(&mut w, i, place, 120);
            }
        }
        w.run(&mut brain, 60);
        if w.gang(gang).unwrap().treasury > 0 {
            break;
        }
        assert_eq!(w.money_supply(), money);
    }
    let g = w.gang(gang).unwrap();
    assert!(g.treasury > 0, "no pizzo");
    assert_eq!(w.money_supply(), money);
    let paid = events(
        &w,
        |k| matches!(k, EventKind::GangExtortion { paid: true, victim: Fighter::Npc(v), .. } if *v == vid),
    );
    assert!(!paid.is_empty());
    let v = w.npc(vid).unwrap();
    assert!(v.inventory.tokens < 100);
    assert!(v.grudges.iter().any(|g| g.reason == GrudgeReason::Extorted));
    assert!(w.gang_state().counters.pizzo_tokens > 0);
}

/// A harm to a member is a grudge of the whole gang against the attacker.
#[test]
fn harming_a_member_makes_the_whole_gang_hold_a_grudge() {
    let mut w = World::generate_with_params(14, 10, 120, quiet());
    let (gang, people, market) = market_gang(&mut w, 3);
    let outsider = adults(&w)[7];
    let place = Place {
        carriage: market,
        floor: 0,
    };
    park(&mut w, outsider, place, 60);
    park(&mut w, people[1], place, 60);
    let (oid, vid) = (w.npcs[outsider].id, w.npcs[people[1]].id);
    assert!(w.npc_attack(oid, Fighter::Npc(vid), Motive::Grudge));
    let mut brain = UtilityBrain::new(14);
    w.run(&mut brain, 3);
    for &i in &[people[0], people[2]] {
        let id = w.npcs[i].id;
        let n = w.npc(id).unwrap();
        let g = n.grudge_against(Fighter::Npc(oid)).expect("a gang grudge");
        assert_eq!(g.reason, GrudgeReason::GangMate(Fighter::Npc(vid)));
    }
    assert!(w.gang(gang).is_some());
}

/// The leader orders a hit: the member sent kills the target, with a
/// `GangHit` event and the killer recorded.
#[test]
fn a_leaders_hit_can_kill() {
    let mut w = World::generate_with_params(15, 10, 120, quiet());
    let (gang, people, market) = market_gang(&mut w, 3);
    let target = adults(&w)[8];
    let tid = w.npcs[target].id;
    w.npcs[target].health = 6.0;
    let place = Place {
        carriage: market,
        floor: 0,
    };
    assert!(w.order_hit(gang, Fighter::Npc(tid)));
    let hit = w.gang(gang).unwrap().hit.expect("a hit");
    assert_ne!(hit.by, w.gang(gang).unwrap().leader);
    let by = hit.by;
    let mut brain = UtilityBrain::new(15);
    for _ in 0..48 {
        if w.npc(tid).is_none() {
            break;
        }
        for id in [tid, by] {
            if w.fight_of(Fighter::Npc(id)).is_none() {
                let i = index(&w, id);
                park(&mut w, i, place, 120);
            }
        }
        // Keep the target weak, it would heal.
        let i = index(&w, tid);
        w.npcs[i].health = w.npcs[i].health.min(6.0);
        w.run(&mut brain, 60);
    }
    assert!(w.npc(tid).is_none(), "the target survived");
    let hits = events(&w, |k| matches!(k, EventKind::GangHit { .. }));
    assert!(
        matches!(hits[..], [EventKind::GangHit { killer, victim: Fighter::Npc(v), gang: g, .. }]
            if *killer == by && *v == tid && *g == gang),
        "{hits:?}"
    );
    assert!(
        !events(
            &w,
            |k| matches!(k, EventKind::Killed { killer: Fighter::Npc(k), .. } if *k == by)
        )
        .is_empty()
    );
    assert_eq!(w.life.deaths_by_cause[DeathCause::Violence.index()], 1);
    assert_eq!(w.gang_state().counters.hits_done, 1);
    assert!(w.gang(gang).unwrap().hit.is_none());
    assert_eq!(w.gang(gang).unwrap().size(), people.len());
}

/// Bleeds NPC `id` to death (no killer).
fn bleed_out(w: &mut World, brain: &mut UtilityBrain, id: NpcId) {
    w.params.bleed_per_minute = 5.0;
    let i = index(w, id);
    let place = Place {
        carriage: w.npcs[i].carriage,
        floor: 0,
    };
    park(w, i, place, 60);
    w.npcs[i].health = 5.0;
    w.npcs[i].injury = 60.0;
    for _ in 0..30 {
        w.run(brain, 1);
        if w.npc(id).is_none() {
            return;
        }
    }
    panic!("it did not die");
}

/// When the leader dies the fittest member takes over.
#[test]
fn a_new_leader_follows_the_dead_one() {
    let mut w = World::generate_with_params(16, 10, 120, quiet());
    let (gang, _, _) = market_gang(&mut w, 3);
    let leader = w.gang(gang).unwrap().leader;
    let mut brain = UtilityBrain::new(16);
    // Past the hour: the successor is chosen at the next one.
    w.run(&mut brain, 1);
    bleed_out(&mut w, &mut brain, leader);
    assert!(w.gang(gang).unwrap().leaderless);
    w.run(&mut brain, 61);
    let g = w.gang(gang).expect("still a gang");
    assert!(!g.leaderless);
    assert_ne!(g.leader, leader);
    assert!(g.is_member(g.leader));
    let new = events(&w, |k| matches!(k, EventKind::GangLeader { .. }));
    assert!(
        matches!(new[..], [EventKind::GangLeader { leader, .. }] if *leader == g.leader),
        "{new:?}"
    );
}

/// A gang left with one member disbands; its treasury goes to the last one.
#[test]
fn a_gang_of_one_disbands() {
    let mut params = quiet();
    params.gang.pizzo_per_hour = 1.0;
    let mut w = World::generate_with_params(17, 10, 120, params);
    let (gang, people, _) = market_gang(&mut w, 2);
    let money = w.money_supply();
    let mut brain = UtilityBrain::new(17);
    let (a, b) = (w.npcs[people[0]].id, w.npcs[people[1]].id);
    bleed_out(&mut w, &mut brain, a);
    assert!(w.gang(gang).is_none());
    assert!(w.gangs().is_empty());
    assert!(w.gang_of(b).is_none());
    let gone = events(&w, |k| matches!(k, EventKind::GangDisbanded { .. }));
    assert_eq!(gone.len(), 1, "{gone:?}");
    assert_eq!(w.money_supply(), money);
}

/// A friend in a gang invites the player ("!"), who joins; collecting the
/// pizzo on a task pays the player its share, and so does the treasury.
#[test]
fn the_player_is_invited_joins_and_gets_a_share() {
    let mut w = World::generate_with_params(18, 10, 120, quiet());
    let (gang, people, market) = market_gang(&mut w, 3);
    let place = Place {
        carriage: market,
        floor: 0,
    };
    let friend = people[1];
    let fid = w.npcs[friend].id;
    w.npcs[friend].player = Some(PlayerTie {
        affinity: 0.9,
        ..PlayerTie::default()
    });
    park(&mut w, friend, place, 120);
    w.set_player_place(place);
    let mut brain = UtilityBrain::new(18);
    for _ in 0..30 {
        w.run(&mut brain, 1);
        if w.gang_invite().is_some() {
            break;
        }
    }
    let invite = w.gang_invite().expect("an invitation");
    assert_eq!((invite.gang, invite.by), (gang, fid));
    assert!(w.wants_to_talk(fid));
    let line = w.player_chat_start(fid).unwrap().expect("it speaks first");
    assert!(line.contains(&w.gang(gang).unwrap().name), "{line}");
    let not_inviter = w.npcs[people[0]].id;
    assert_eq!(w.player_join_gang(not_inviter), Err(GangError::NoInvite));
    assert_eq!(w.player_join_gang(fid), Ok(gang));
    assert_eq!(w.player_gang().map(|g| g.id), Some(gang));
    assert!(!events(&w, |k| matches!(k, EventKind::PlayerJoinedGang { .. })).is_empty());
    assert!(w.gang_attitude(gang) > 0.5);
    // A rich shopkeeper in the territory: the next task.
    let victim = adults(&w)[9];
    meek(&mut w, victim);
    w.npcs[victim].workplace = Some(market);
    w.npcs[victim].inventory.tokens = 400;
    let vid = w.npcs[victim].id;
    let money = w.money_supply();
    let midnight = w.clock.next_at(0, 0);
    let wait = midnight.since(w.clock) + 1;
    w.run(&mut brain, wait);
    let task = w.gang_task().expect("a task");
    assert_eq!(task.victim, vid);
    let i = index(&w, vid);
    park(&mut w, i, place, 60);
    w.set_player_place(place);
    let before = w.player.tokens;
    let out = w.player_collect_pizzo(vid).expect("it pays");
    assert!(out.paid > 0 && out.share > 0 && out.share < out.paid);
    assert_eq!(w.player.tokens, before + out.share);
    assert!(w.gang(gang).unwrap().treasury >= out.paid - out.share);
    assert!(w.gang_task().is_none());
    assert_eq!(w.money_supply(), money);
    // The treasury pays its members, the player too.
    let before = w.player.tokens;
    let midnight = w.clock.next_at(0, 0);
    let wait = midnight.since(w.clock) + 1;
    w.run(&mut brain, wait);
    assert!(w.player.tokens > before, "no share at midnight");
    assert_eq!(w.money_supply(), money);
    // Leaving costs grudges.
    assert_eq!(w.player_leave_gang(), Ok(()));
    let member = w.npc(w.npcs[people[0]].id).unwrap();
    assert!(
        member
            .grudge_against(Fighter::Player)
            .is_some_and(|g| g.reason == GrudgeReason::Defied)
    );
    assert!(w.player_gang().is_none());
    assert_eq!(w.player_leave_gang(), Err(GangError::NotMember));
}

/// Hitting a member makes its gang hostile to the player.
#[test]
fn attacking_a_member_makes_the_gang_hostile() {
    let mut w = World::generate_with_params(19, 10, 120, quiet());
    let (gang, people, market) = market_gang(&mut w, 3);
    let place = Place {
        carriage: market,
        floor: 0,
    };
    let (target, other) = (w.npcs[people[1]].id, w.npcs[people[2]].id);
    park(&mut w, people[1], place, 60);
    w.set_player_place(place);
    assert!(!w.is_hostile_to_player(other));
    w.player_attack(target).expect("in reach");
    assert!(w.is_gang_hostile(gang));
    assert!(w.is_hostile_to_player(other), "the whole gang");
}

/// The player sells at the stalls of a gang's territory: a member asks the
/// pizzo ("!"); paying moves the tokens to the treasury, refusing makes the
/// gang like the player less.
#[test]
fn the_gang_asks_the_player_the_pizzo_on_its_sales() {
    let mut w = World::generate_with_params(22, 10, 120, quiet());
    let (gang, people, market) = market_gang(&mut w, 3);
    let place = Place {
        carriage: market,
        floor: 0,
    };
    w.set_player_place(place);
    w.player.inventory.add(sim::ItemKind::Coperta, 2);
    let listing = w
        .player_list_for_sale(market, sim::ItemKind::Coperta, 2, 30)
        .expect("listed");
    let buyer = adults(&w)[9];
    park(&mut w, buyer, place, 120);
    w.npcs[buyer].inventory.tokens = 200;
    let bid = w.npcs[buyer].id;
    w.buy_listing(sim::Buyer::Npc(bid), listing, 2)
        .expect("sold");
    let due = w.gang(gang).unwrap().player_due;
    assert_eq!(due, 6, "10% of 60");
    let money = w.money_supply();
    // A member comes by (by day) and asks.
    let collector = w.npcs[people[0]].id;
    park(&mut w, people[0], place, 600);
    let mut brain = UtilityBrain::new(22);
    w.run(&mut brain, 3 * 60);
    assert!(w.gang(gang).unwrap().demanded.is_some(), "never asked");
    assert!(w.wants_to_talk(collector));
    let line = w.player_chat_start(collector).unwrap().expect("it asks");
    assert!(line.contains("gettoni"), "{line}");
    let before = (w.player.tokens, w.gang(gang).unwrap().treasury);
    assert_eq!(w.player_pay_pizzo(gang), Ok(due));
    assert_eq!(w.player.tokens, before.0 - due);
    assert_eq!(w.gang(gang).unwrap().treasury, before.1 + due);
    assert_eq!(w.money_supply(), money);
    assert_eq!(w.player_pay_pizzo(gang), Err(GangError::NothingDue));
    // Next time the player refuses.
    let attitude = w.gang_attitude(gang);
    w.player.inventory.add(sim::ItemKind::Coperta, 1);
    let listing = w
        .player_list_for_sale(market, sim::ItemKind::Coperta, 1, 30)
        .expect("listed");
    let i = index(&w, bid);
    park(&mut w, i, place, 60);
    w.buy_listing(sim::Buyer::Npc(bid), listing, 1)
        .expect("sold");
    assert_eq!(w.player_refuse_pizzo(gang), Ok(()));
    assert!(w.gang_attitude(gang) < attitude);
    assert!(
        events(&w, |k| matches!(
            k,
            EventKind::GangExtortion {
                victim: Fighter::Player,
                paid: false,
                ..
            }
        ))
        .len()
            == 1
    );
}

/// A world with gangs (a hit ordered, a treasury) saves and loads: both
/// continue identically; and the same seed gives the same gangs.
#[test]
fn gangs_are_deterministic_and_survive_save_load() {
    let mut params = SimParams {
        violence: 3.0,
        ..SimParams::default()
    };
    params.gang.found_per_day = 1.0;
    let run = || {
        let mut w = World::generate_with_params(20, 10, 150, params.clone());
        let mut brain = UtilityBrain::new(20);
        w.run(&mut brain, 40 * DAY);
        (w, brain)
    };
    let (mut a, mut brain_a) = run();
    let (b, _) = run();
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap()
    );
    assert!(a.gang_state().counters.founded > 0, "no gang in 40 days");
    if let Some(g) = a.gangs().first().map(|g| g.id) {
        let target = a
            .npcs
            .iter()
            .find(|n| a.gang_of(n.id).is_none() && n.age >= 20)
            .map(|n| n.id)
            .unwrap();
        a.order_hit(g, Fighter::Npc(target));
    }
    let saved = serde_json::to_string(&a).unwrap();
    let mut c: World = serde_json::from_str(&saved).unwrap();
    let mut brain_c = brain_a.clone();
    a.run(&mut brain_a, 3 * DAY);
    c.run(&mut brain_c, 3 * DAY);
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&c).unwrap()
    );
}

/// 20 years on a train of 400 with the default params: a few gangs, a few
/// killings, nobody starves, the population holds, money is conserved.
#[test]
fn twenty_years_of_gangs_stay_balanced() {
    let mut w = World::generate(42, 20, 400);
    let mut brain = UtilityBrain::new(42);
    let money = w.money_supply();
    let beds = w.total_beds();
    let mut most = 0;
    for _ in 0..20 {
        w.run(&mut brain, u64::from(w.params.days_per_year) * DAY);
        most = most.max(w.gangs().len());
        let pop = w.npcs.len();
        assert!(pop >= 380 && pop <= beds, "population {pop}");
        assert!(w.gangs().len() <= w.params.gang.max_gangs);
    }
    let c = &w.gang_state().counters;
    assert!(
        (2..=5).contains(&w.gangs().len()),
        "{} gangs: {c:?}",
        w.gangs().len()
    );
    assert!(most >= 2);
    assert!((1..=10).contains(&c.kills), "{} killings: {c:?}", c.kills);
    assert!(c.pizzo_paid > 20, "{c:?}");
    assert_eq!(w.life.deaths_by_cause[DeathCause::Starvation.index()], 0);
    assert_eq!(w.money_supply(), money);
    let members: usize = w.gangs().iter().map(|g| g.size()).sum();
    let s = Stats::of(&w);
    assert!(
        members * 5 < s.population,
        "{members} members of {}",
        s.population
    );
}
