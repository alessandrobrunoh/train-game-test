//! Ciclo di vita: età, coppie, nascite, morti, ricambio generazionale.

use sim::{
    Action, BirthDenial, CarriageKind, DeathCause, EventKind, GameTime, ItemKind, Job, LifeStage,
    MINUTES_PER_DAY, Npc, NpcId, RelationKind, Sex, SimParams, StationKind, Stats, UtilityBrain,
    World,
};

const DAY: u64 = MINUTES_PER_DAY;

/// No random deaths nor births unless a test asks for them.
fn quiet_params() -> SimParams {
    SimParams {
        mortality_base: 0.0,
        birth_chance_per_year: 0.0,
        ..SimParams::default()
    }
}

fn world_with(params: SimParams) -> (World, UtilityBrain) {
    (
        World::generate_with_params(42, 10, 100, params),
        UtilityBrain::new(42),
    )
}

/// Runs until just after the next midnight (the daily life pass).
fn past_midnight(w: &mut World, brain: &mut UtilityBrain) {
    let midnight = w.clock.next_at(0, 0);
    w.run(brain, midnight - w.clock + 1);
}

fn year(w: &World) -> i64 {
    w.params.minutes_per_year() as i64
}

fn index(w: &World, id: NpcId) -> usize {
    w.npcs.iter().position(|n| n.id == id).unwrap()
}

fn events<'a>(w: &'a World, pred: impl Fn(&EventKind) -> bool + 'a) -> Vec<&'a EventKind> {
    w.events
        .iter()
        .map(|e| &e.kind)
        .filter(|k| pred(k))
        .collect()
}

/// Makes NPC `i` turn `age` at the coming midnight.
fn turns_at_midnight(w: &mut World, i: usize, age: u32) {
    let midnight = w.clock.next_at(0, 0).0 as i64;
    let y = year(w);
    let npc = &mut w.npcs[i];
    npc.born = midnight - i64::from(age) * y - 1;
    npc.age = age - 1;
}

fn make_friends(w: &mut World, a: usize, b: usize, affinity: f32) {
    for (x, y) in [(a, b), (b, a)] {
        let other = w.npcs[y].id;
        let npc = &mut w.npcs[x];
        npc.relations.retain(|r| r.other != other);
        npc.relations.push(sim::Relation {
            other,
            kind: RelationKind::Friend,
            affinity,
        });
    }
}

#[test]
fn generation_has_an_age_pyramid_and_families() {
    let w = World::generate(42, 20, 400);
    let s = Stats::of(&w);
    for stage in LifeStage::ALL {
        assert!(s.stage(stage) >= 15, "{stage:?}: {}", s.stage(stage));
    }
    assert!(s.couples >= 60, "only {} couples", s.couples);
    assert!(
        s.beds > s.population + s.population / 10,
        "no spare beds: {s}"
    );
    assert_eq!(s.max_population, w.max_population());
    for npc in &w.npcs {
        assert_eq!(npc.age, npc.age_years(w.clock, w.params.days_per_year));
        assert_eq!(npc.job.is_some(), npc.stage().works(), "{}", npc.name);
        if let Some(p) = npc.partner() {
            let p = w.npc(p).unwrap();
            assert_eq!(p.partner(), Some(npc.id));
            assert_ne!(p.sex, npc.sex);
            assert_eq!(p.home, npc.home, "partners live apart");
        }
        let parents: Vec<&Npc> = npc.parents().map(|p| w.npc(p).unwrap()).collect();
        if !parents.is_empty() {
            assert_eq!(parents.len(), 2);
            let father = parents.iter().find(|p| p.sex == Sex::Male).unwrap();
            assert_eq!(npc.surname(), father.surname());
            if npc.age < LifeStage::ADULTO_FROM {
                assert_eq!(npc.home, parents[0].home, "minor lives away from parents");
            }
            for p in parents {
                assert!(p.age >= npc.age + 18);
                assert!(p.children().any(|c| c == npc.id));
            }
        }
    }
    // Every Dormitorio has spare beds.
    for c in w
        .carriages
        .iter()
        .filter(|c| c.kind == CarriageKind::Dormitorio)
    {
        assert!(w.beds(c.id) > w.residents(c.id));
    }
}

#[test]
fn coming_of_age_assigns_a_job() {
    let (mut w, mut brain) = world_with(quiet_params());
    let i = w
        .npcs
        .iter()
        .position(|n| n.stage() == LifeStage::Giovane)
        .unwrap();
    let id = w.npcs[i].id;
    turns_at_midnight(&mut w, i, 18);
    past_midnight(&mut w, &mut brain);

    let npc = w.npc(id).unwrap();
    assert_eq!(npc.age, 18);
    let job = npc.job.expect("an adult works");
    let place = npc.workplace.expect("and has a workplace");
    assert_eq!(w.carriages[place.index()].kind, job.workplace_kind());
    let came = events(
        &w,
        |k| matches!(k, EventKind::CameOfAge { npc, .. } if *npc == id),
    );
    assert_eq!(came.len(), 1);
    assert!(matches!(came[0], EventKind::CameOfAge { job: Some(j), .. } if *j == job));
}

#[test]
fn workforce_follows_the_food_chain_first() {
    let (mut w, mut brain) = world_with(quiet_params());
    // Every Contadino retires at once: Operai must take their place.
    let farmers: Vec<usize> = (0..w.npcs.len())
        .filter(|&i| w.npcs[i].job == Some(Job::Contadino))
        .collect();
    assert!(farmers.len() >= 5);
    for &i in &farmers {
        turns_at_midnight(&mut w, i, 65);
    }
    let operai = w
        .npcs
        .iter()
        .filter(|n| n.job == Some(Job::Operaio))
        .count();
    past_midnight(&mut w, &mut brain);
    let count = |job| w.npcs.iter().filter(|n| n.job == Some(job)).count();
    let moved = farmers.len().min(operai);
    assert_eq!(count(Job::Contadino), moved);
    assert_eq!(count(Job::Operaio), operai - moved);
    for &i in &farmers {
        let npc = &w.npcs[i];
        assert_eq!(
            (npc.job, npc.workplace),
            (None, None),
            "{} still works",
            npc.name
        );
    }
}

#[test]
fn retirement_clears_job() {
    let (mut w, mut brain) = world_with(quiet_params());
    let i = w.npcs.iter().position(|n| n.job.is_some()).unwrap();
    let id = w.npcs[i].id;
    let job = w.npcs[i].job;
    turns_at_midnight(&mut w, i, 65);
    past_midnight(&mut w, &mut brain);

    let npc = w.npc(id).unwrap();
    assert_eq!(npc.stage(), LifeStage::Anziano);
    assert_eq!((npc.job, npc.workplace), (None, None));
    let retired = events(
        &w,
        |k| matches!(k, EventKind::Retired { npc, .. } if *npc == id),
    );
    assert_eq!(retired.len(), 1);
    assert!(matches!(retired[0], EventKind::Retired { job: j, .. } if *j == job));
    // It stays retired.
    past_midnight(&mut w, &mut brain);
    assert_eq!(w.npc(id).unwrap().job, None);
}

#[test]
fn couples_form_only_between_eligible_adults() {
    let (mut w, mut brain) = world_with(quiet_params());
    w.params.max_events = usize::MAX;
    let single = |w: &World, sex: Sex, stage: LifeStage, skip: &[usize]| {
        (0..w.npcs.len())
            .find(|&i| {
                let n = &w.npcs[i];
                n.sex == sex && n.stage() == stage && n.partner().is_none() && !skip.contains(&i)
            })
            .unwrap()
    };
    // A single woman and a single man close in age, in different Dormitori.
    let eligible =
        |n: &Npc, sex| n.sex == sex && n.stage() == LifeStage::Adulto && n.partner().is_none();
    let (a, b) = (0..w.npcs.len())
        .filter(|&a| eligible(&w.npcs[a], Sex::Female))
        .find_map(|a| {
            let b = (0..w.npcs.len()).find(|&b| {
                let (x, y) = (&w.npcs[a], &w.npcs[b]);
                eligible(y, Sex::Male) && x.age.abs_diff(y.age) <= 5 && x.home != y.home
            })?;
            Some((a, b))
        })
        .unwrap();
    // A minor with an adult: never.
    let minor = single(&w, Sex::Female, LifeStage::Giovane, &[]);
    let c = single(&w, Sex::Male, LifeStage::Adulto, &[b]);
    // Same sex (not enabled by default): never.
    let d = single(&w, Sex::Female, LifeStage::Adulto, &[a]);
    let e = single(&w, Sex::Female, LifeStage::Adulto, &[a, d]);
    make_friends(&mut w, a, b, 0.95);
    make_friends(&mut w, minor, c, 0.9);
    make_friends(&mut w, d, e, 0.9);
    let (ids, homes): (Vec<NpcId>, Vec<_>) = [a, b, minor, c, d, e]
        .iter()
        .map(|&i| (w.npcs[i].id, w.npcs[i].home))
        .unzip();
    // Different homes: one of them moves in with the other.
    assert_ne!(homes[0], homes[1], "pick partners from different dorms");

    past_midnight(&mut w, &mut brain);
    let npc = |k: usize| w.npc(ids[k]).unwrap();
    assert_eq!(npc(0).partner(), Some(ids[1]));
    assert_eq!(npc(1).partner(), Some(ids[0]));
    assert_eq!(npc(0).relation(ids[1]).unwrap().kind, RelationKind::Partner);
    assert_eq!(npc(0).home, npc(1).home, "partners live together");
    assert_eq!(npc(0).home, homes[1], "the woman moves in");
    assert_ne!(npc(2).partner(), Some(ids[3]));
    assert_ne!(npc(4).partner(), Some(ids[5]));
    let coupled = events(&w, |k| {
        matches!(k, EventKind::Coupled { npc, partner, .. }
            if [*npc, *partner].contains(&ids[0]) && [*npc, *partner].contains(&ids[1]))
    });
    assert_eq!(coupled.len(), 1);
    assert!(matches!(coupled[0], EventKind::Coupled { moved_to: Some(c), .. } if *c == homes[1]));
    for k in [2, 3, 4, 5] {
        let id = ids[k];
        let pairs = events(
            &w,
            |ev| matches!(ev, EventKind::Coupled { npc, partner, .. } if *npc == id || *partner == id),
        );
        // Nobody of them coupled with the forbidden friend.
        for ev in pairs {
            let EventKind::Coupled { npc, partner, .. } = ev else {
                unreachable!()
            };
            let other = if *npc == id { *partner } else { *npc };
            let forbidden = ids[if k % 2 == 0 { k + 1 } else { k - 1 }];
            assert_ne!(other, forbidden);
        }
    }
}

#[test]
fn family_and_friends_are_offered_first_for_a_chat() {
    let (mut w, _) = world_with(quiet_params());
    let i = w.npcs.iter().position(|n| n.partner().is_some()).unwrap();
    let id = w.npcs[i].id;
    let partner = w.npcs[i].partner().unwrap();
    let j = index(&w, partner);
    let here = w.npcs[i].carriage;
    w.npcs[j].carriage = here;
    w.npcs[j].action = Action::Idle;
    for _ in 0..5 {
        let options = w.options(id);
        assert!(
            options
                .iter()
                .any(|o| o.action == Action::Socialize(partner))
        );
    }
    // The utility brain prefers the partner to a stranger.
    let brain = UtilityBrain::new(1);
    let stranger = w
        .npcs
        .iter()
        .find(|n| w.npcs[i].relation(n.id).is_none() && n.id != id)
        .unwrap()
        .id;
    w.npcs[i].needs.social = 0.3;
    let option = |action| sim::ActionOption {
        action,
        minutes: 30,
        goal: None,
        description: String::new(),
    };
    let npc = &w.npcs[i];
    let with_partner = brain.score(&w, npc, &option(Action::Socialize(partner)));
    let with_stranger = brain.score(&w, npc, &option(Action::Socialize(stranger)));
    assert!(with_partner > with_stranger);
}

#[test]
fn chatting_builds_affinity() {
    let params = SimParams {
        quarrel_chance: 0.0,
        ..quiet_params()
    };
    let (mut w, mut brain) = world_with(params);
    let before: f32 = w
        .npcs
        .iter()
        .flat_map(|n| &n.relations)
        .map(|r| r.affinity)
        .sum();
    w.run(&mut brain, 18 * 60);
    let after: f32 = w
        .npcs
        .iter()
        .flat_map(|n| &n.relations)
        .map(|r| r.affinity)
        .sum();
    assert!(after > before + 5.0, "{before} -> {after}");
    for n in &w.npcs {
        let friends = n.relations_of(RelationKind::Friend).count();
        assert!(friends <= sim::MAX_RELATIONS);
        for r in &n.relations {
            assert!((-1.0..=1.0).contains(&r.affinity));
            assert_ne!(r.other, n.id);
        }
    }
}

#[test]
fn full_train_denies_births() {
    let params = SimParams {
        birth_chance_per_year: 1000.0,
        birth_max_bed_occupancy: 0.5,
        ..quiet_params()
    };
    let (mut w, mut brain) = world_with(params);
    w.params.max_events = usize::MAX;
    let population = w.npcs.len();
    assert!(population >= w.max_population());
    assert_eq!(w.birth_permit(), Err(BirthDenial::Overcrowded));
    w.run(&mut brain, 6 * DAY);
    assert_eq!(w.life.births_total, 0);
    assert!(events(&w, |k| matches!(k, EventKind::Born { .. })).is_empty());
    assert_eq!(w.npcs.len(), population);
    assert!(w.life.births_denied_total > 10);
    let denied = events(&w, |k| {
        matches!(
            k,
            EventKind::BirthDenied {
                reason: BirthDenial::Overcrowded,
                ..
            }
        )
    });
    // Logged, but not every refusal.
    assert!((1..=3).contains(&denied.len()), "{} logged", denied.len());

    // Room but no food: denied too.
    w.params.birth_max_bed_occupancy = 1.0;
    assert!(w.birth_permit().is_ok());
    for c in &mut w.carriages {
        c.stock.set(ItemKind::Razione, 0.0);
    }
    assert_eq!(w.birth_permit(), Err(BirthDenial::NotEnoughFood));
}

#[test]
fn newborns_join_their_family() {
    let params = SimParams {
        birth_chance_per_year: 1000.0,
        ..quiet_params()
    };
    let (mut w, mut brain) = world_with(params);
    w.params.max_events = usize::MAX;
    let population = w.npcs.len();
    let first_id = w.next_npc_id();
    past_midnight(&mut w, &mut brain);
    assert!(w.life.births_total > 0, "no births");
    assert_eq!(w.npcs.len(), population + w.life.births_total as usize);

    let baby = w.npc(first_id).expect("newborn");
    let born = events(
        &w,
        |k| matches!(k, EventKind::Born { npc, .. } if *npc == first_id),
    );
    let EventKind::Born {
        mother,
        father,
        name,
        sex,
        ..
    } = born[0]
    else {
        unreachable!()
    };
    let (m, f) = (w.npc(*mother).unwrap(), w.npc(*father).unwrap());
    assert_eq!((m.sex, f.sex), (Sex::Female, Sex::Male));
    assert_eq!(m.partner(), Some(f.id));
    assert!((w.params.fertile_min_age..=w.params.fertile_max_age).contains(&m.age));
    assert_eq!(&baby.name, name);
    assert_eq!(baby.sex, *sex);
    assert_eq!(baby.age, 0);
    assert_eq!(baby.stage(), LifeStage::Bambino);
    assert_eq!(baby.born, (w.clock.0 - 1) as i64);
    assert_eq!(baby.surname(), f.surname());
    assert_eq!(baby.home, m.home);
    assert_eq!(baby.job, None);
    let mut parents: Vec<NpcId> = baby.parents().collect();
    parents.sort();
    let mut expected = vec![m.id, f.id];
    expected.sort();
    assert_eq!(parents, expected);
    for p in [m, f] {
        assert_eq!(p.relation(baby.id).unwrap().kind, RelationKind::Child);
    }
    // Siblings: the mother's other children, both ways.
    for sibling in m.children().filter(|&c| c != baby.id) {
        assert_eq!(baby.relation(sibling).unwrap().kind, RelationKind::Sibling);
        assert_eq!(
            w.npc(sibling).unwrap().relation(baby.id).unwrap().kind,
            RelationKind::Sibling
        );
    }
    // Spacing: no second child for the same mother right away.
    let children_before = m.children().count();
    let mother_id = m.id;
    past_midnight(&mut w, &mut brain);
    assert_eq!(
        w.npc(mother_id).unwrap().children().count(),
        children_before
    );
    // Beds are never overbooked by births.
    for c in w
        .carriages
        .iter()
        .filter(|c| c.kind == CarriageKind::Dormitorio)
    {
        assert!(w.residents(c.id) <= w.beds(c.id));
    }
}

#[test]
fn death_frees_bed_and_station_and_passes_tokens() {
    // Only absurd ages die: the NPCs we age to 200.
    let params = SimParams {
        mortality_base: 1e-12,
        mortality_growth: 0.2,
        ..quiet_params()
    };
    let (mut w, mut brain) = world_with(params);
    w.params.max_events = usize::MAX;
    w.run(&mut brain, GameTime::from_dhm(1, 23, 50) - w.clock);

    // A partnered NPC dies asleep in a bed.
    let i = w.npcs.iter().position(|n| n.partner().is_some()).unwrap();
    let (id, partner) = (w.npcs[i].id, w.npcs[i].partner().unwrap());
    let partner_tokens = w.npc(partner).unwrap().inventory.tokens;
    let home = w.npcs[i].home;
    if let Some(s) = w.npcs[i].action.station() {
        let c = w.npcs[i].carriage;
        w.carriages[c.index()].stations[s.index()].occupancy -= 1;
    }
    let bed = w.carriages[home.index()]
        .free_station(StationKind::Bed)
        .unwrap();
    w.carriages[home.index()].stations[bed.index()].occupancy += 1;
    let npc = &mut w.npcs[i];
    npc.carriage = home;
    npc.action = Action::Sleep(bed);
    npc.action_until = GameTime::from_dhm(2, 6, 0);
    npc.inventory.tokens = 50;
    turns_at_midnight(&mut w, i, 200);

    // A parent without a partner leaves its tokens to the children.
    let k = w
        .npcs
        .iter()
        .position(|n| {
            n.children().count() >= 2
                && ![id, partner].contains(&n.id)
                && n.partner() != Some(id)
                && !n.children().any(|c| [id, partner].contains(&c))
        })
        .unwrap();
    let (parent, ex) = (w.npcs[k].id, w.npcs[k].partner().unwrap());
    let e = index(&w, ex);
    w.npcs[k].relations.retain(|r| r.other != ex);
    w.npcs[e].relations.retain(|r| r.other != parent);
    w.npcs[k].inventory.tokens = 7;
    turns_at_midnight(&mut w, k, 200);
    let heirs: Vec<NpcId> = w.npcs[k].children().collect();
    let heirs_tokens: u32 = heirs
        .iter()
        .map(|&h| w.npc(h).unwrap().inventory.tokens)
        .sum();

    let beds_used = w.carriages[home.index()].stations[bed.index()].occupancy;
    past_midnight(&mut w, &mut brain);

    assert!(w.npc(id).is_none() && w.npc(parent).is_none());
    assert_eq!(
        w.carriages[home.index()].stations[bed.index()].occupancy,
        beds_used - 1
    );
    let widow = w.npc(partner).unwrap();
    assert_eq!(widow.partner(), None);
    assert!(widow.inventory.tokens >= partner_tokens + 50);
    let heirs_after: u32 = heirs
        .iter()
        .map(|&h| w.npc(h).unwrap().inventory.tokens)
        .sum();
    assert!(heirs_after >= heirs_tokens + 7);
    for n in &w.npcs {
        assert!(n.relation(id).is_none() && n.relation(parent).is_none());
    }
    let died = events(
        &w,
        |k| matches!(k, EventKind::NpcDied { npc, cause: DeathCause::OldAge, age: 200, .. } if *npc == id),
    );
    assert_eq!(died.len(), 1);
    let widowed = events(
        &w,
        |k| matches!(k, EventKind::Widowed { npc, partner: p, .. } if *npc == partner && *p == id),
    );
    assert_eq!(widowed.len(), 1);
    assert_eq!(w.life.deaths_by_cause[DeathCause::OldAge.index()], 2);
}

#[test]
fn life_cycle_is_deterministic() {
    let params = SimParams {
        birth_chance_per_year: 2.0,
        mortality_base: 1e-3,
        ..SimParams::default()
    };
    let run = || {
        let mut w = World::generate_with_params(9, 10, 100, params.clone());
        w.run(&mut UtilityBrain::new(9), 2 * 12 * DAY);
        w
    };
    let (a, b) = (run(), run());
    assert!(a.life.births_total > 0 && a.life.deaths_total > 0);
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap()
    );
}

/// 50 years (600 game days) of the default train: slow in debug, run with
/// `cargo test -p sim --release -- --ignored`.
#[test]
#[ignore]
fn fifty_years_stay_balanced_with_turnover() {
    let mut w = World::generate(42, 20, 400);
    let mut brain = UtilityBrain::new(42);
    let beds = w.total_beds();
    for _ in 0..50 {
        w.run(&mut brain, 12 * DAY);
        let pop = w.npcs.len();
        assert!(
            pop * 10 >= beds * 7 && pop <= beds,
            "population {pop} of {beds} beds at {}",
            w.clock
        );
    }
    let s = Stats::of(&w);
    assert_eq!(w.life.deaths_by_cause[DeathCause::Starvation.index()], 0);
    assert!(w.life.births_total > 200, "{:?}", w.life);
    assert!(w.life.births_denied_total > 0, "never throttled");
    assert!(
        s.founders * 5 < s.population * 2,
        "{} of {} are founders",
        s.founders,
        s.population
    );
    for stage in LifeStage::ALL {
        assert!(s.stage(stage) > 0, "{stage:?} missing: {s}");
    }
}
