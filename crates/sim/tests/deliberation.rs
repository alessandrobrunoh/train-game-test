//! Deliberazioni: apertura, scadenza, risposte del cervello, effetti.

mod common;

use common::{Scripted, always, silent};
use sim::{
    Action, Brain, CarriageId, CarriageKind, Choice, DecisionRequest, Deliberation,
    DeliberationAnswer, DeliberationId, DeliberationKind, EventKind, GameTime, Grievance, ItemKind,
    LifeStage, MINUTES_PER_DAY, NpcId, RelationKind, Resolver, Sex, SimParams, UtilityBrain, World,
};

const DAY: u64 = MINUTES_PER_DAY;

/// Nothing happens unless a test asks for it: no deaths, births, temptations
/// or protests.
fn quiet() -> SimParams {
    SimParams {
        mortality_base: 0.0,
        birth_chance_per_year: 0.0,
        theft_temptation_per_hour: 0.0,
        protest_chance_on_denial: 0.0,
        protest_chance_on_shortage: 0.0,
        max_events: usize::MAX,
        ..SimParams::default()
    }
}

fn world(params: SimParams) -> World {
    World::generate_with_params(42, 10, 100, params)
}

fn index(w: &World, id: NpcId) -> usize {
    w.npcs.iter().position(|n| n.id == id).unwrap()
}

fn first_of(w: &World, kind: CarriageKind) -> CarriageId {
    w.carriages.iter().find(|c| c.kind == kind).unwrap().id
}

/// Runs until just after the next midnight.
fn past_midnight(w: &mut World, brain: &mut dyn Brain) {
    let midnight = w.clock.next_at(0, 0);
    w.run(brain, midnight - w.clock + 1);
}

/// Moves NPC `i` to `carriage`, idle (and awake) for a long while.
fn park(w: &mut World, i: usize, carriage: CarriageId) {
    if let Some(s) = w.npcs[i].action.station() {
        let c = w.npcs[i].carriage;
        let st = &mut w.carriages[c.index()].stations[s.index()];
        st.occupancy -= 1;
    }
    let now = w.clock;
    let npc = &mut w.npcs[i];
    npc.carriage = carriage;
    npc.action = Action::Idle;
    npc.action_since = now;
    npc.action_until = now + 600;
}

/// A youth (no job, wants only clothes) with a living parent, made poor and
/// clothless in the first Mercato; every NPC in that situation is tempted at
/// the next full hour. Returns the youth and the parent.
fn tempt(w: &mut World) -> (NpcId, NpcId) {
    w.params.theft_temptation_per_hour = 1.0;
    let i = w
        .npcs
        .iter()
        .position(|n| n.stage() == LifeStage::Giovane && n.parents().any(|p| w.npc(p).is_some()))
        .expect("a youth with parents");
    let market = first_of(w, CarriageKind::Mercato);
    park(w, i, market);
    let npc = &mut w.npcs[i];
    npc.inventory.tokens = 5;
    npc.inventory.clothes = None;
    let parent = npc.parents().next().unwrap();
    (npc.id, parent)
}

fn resolved_events(w: &World) -> Vec<(DeliberationId, Choice, Resolver, Option<f32>)> {
    w.events
        .iter()
        .filter_map(|e| match e.kind {
            EventKind::DeliberationResolved {
                id,
                choice,
                by,
                confidence,
                ..
            } => Some((id, choice, by, confidence)),
            _ => None,
        })
        .collect()
}

#[test]
fn plain_brains_leave_deliberations_to_the_rules_at_once() {
    let mut w = World::generate(42, 20, 400);
    w.params.max_events = usize::MAX;
    let mut brain = UtilityBrain::new(42);
    for _ in 0..3 * DAY {
        w.tick(&mut brain);
        assert!(w.open_deliberations().is_empty());
    }
    let c = &w.deliberation_counters;
    assert!(c.opened_total() > 3, "{c:?}");
    assert_eq!(c.resolved_total() + c.cancelled, c.opened_total());
    assert_eq!(c.by_brain.iter().sum::<u64>(), 0);
    let resolved = resolved_events(&w);
    assert_eq!(resolved.len() as u64, c.resolved_total());
    assert!(
        resolved
            .iter()
            .all(|r| r.2 == Resolver::Rules && r.3.is_none())
    );
    assert!(
        !w.events
            .iter()
            .any(|e| matches!(e.kind, EventKind::DeliberationAsked { .. }))
    );
    for r in w.recent_deliberations() {
        assert_eq!(r.by, Resolver::Rules);
        let chosen = r.chosen().expect("valid choice");
        assert!(!chosen.description.is_empty());
        assert!((2..=4).contains(&r.deliberation.options.len()));
    }
    // Characters are drawn for everyone.
    assert!(w.npcs.iter().all(|n| {
        (0.0..=1.0).contains(&n.traits.honesty) && (0.0..=1.0).contains(&n.traits.boldness)
    }));
    assert!(w.npcs.windows(2).any(|p| p[0].traits != p[1].traits));
}

#[test]
fn unanswered_deliberations_close_at_the_deadline_by_rules() {
    let mut w = world(quiet());
    let (id, _) = tempt(&mut w);
    let mut brain = silent(1);
    w.tick(&mut brain); // 06:00: temptations are checked on the hour.
    w.params.theft_temptation_per_hour = 0.0;

    let d = w.deliberation_of(id).cloned().expect("tempted");
    let DeliberationKind::Theft {
        item,
        market,
        helper,
    } = d.kind
    else {
        panic!("{:?}", d.kind)
    };
    assert_eq!(item, ItemKind::Vestito);
    assert_eq!(market, first_of(&w, CarriageKind::Mercato));
    assert!(helper.is_some(), "the parent could help");
    assert_eq!(d.deadline, d.asked + w.params.theft_deliberation_hours * 60);
    let name = &w.npc(id).unwrap().name;
    assert!(d.question.contains(name.as_str()), "{}", d.question);
    assert!(d.context.contains("gettoni"), "{}", d.context);
    let choices: Vec<Choice> = d.options.iter().map(|o| o.choice).collect();
    assert_eq!(choices, [Choice::Steal, Choice::Save, Choice::AskForHelp]);
    assert!(
        brain.seen.iter().any(|s| s.id == d.id),
        "the brain was told"
    );
    assert!(w.events.iter().any(
        |e| matches!(&e.kind, EventKind::DeliberationAsked { id: i, question, .. } if *i == d.id && *question == d.question)
    ));
    let weights = w.deliberation_rule_weights(d.id).unwrap();
    assert_eq!(weights.len(), 3);
    assert!((weights.iter().sum::<f32>() - 1.0).abs() < 1e-4);

    // Still open until the deadline...
    w.run(&mut brain, d.deadline - w.clock);
    assert!(w.deliberation(d.id).is_some());
    // ...then the rule decides.
    w.tick(&mut brain);
    assert!(w.deliberation(d.id).is_none());
    let r = w
        .recent_deliberations()
        .iter()
        .find(|r| r.deliberation.id == d.id)
        .unwrap();
    assert_eq!(
        (r.by, r.confidence, r.resolved),
        (Resolver::Rules, None, d.deadline)
    );
    assert!(
        resolved_events(&w)
            .iter()
            .any(|e| e.0 == d.id && e.2 == Resolver::Rules)
    );
}

#[test]
fn an_early_brain_answer_wins() {
    let mut w = world(quiet());
    let (id, _) = tempt(&mut w);
    let mut brain = always(1, Choice::Save);
    w.tick(&mut brain);
    assert!(w.deliberation_of(id).is_none(), "answered in the same tick");
    let r = w
        .recent_deliberations()
        .iter()
        .find(|r| r.deliberation.npc == id)
        .expect("resolved");
    assert_eq!(r.by, Resolver::Brain);
    assert_eq!(r.confidence, Some(0.75));
    assert_eq!(r.chosen().unwrap().choice, Choice::Save);
    assert!(w.deliberation_counters.by_brain[2] >= 1);
    assert_eq!(w.deliberation_counters.by_rules[2], 0);
    let text = w
        .events
        .iter()
        .find(|e| matches!(e.kind, EventKind::DeliberationResolved { npc, .. } if npc == id))
        .unwrap()
        .to_string();
    assert!(
        text.contains("ha deciso (furto)") && text.contains("75%"),
        "{text}"
    );
}

/// Answers with an unknown id and an out-of-range option while a
/// deliberation is open, and with a valid option once it is closed.
struct Bogus {
    inner: UtilityBrain,
    ids: Vec<DeliberationId>,
}

impl Brain for Bogus {
    fn decide(&mut self, world: &World, requests: &[DecisionRequest]) -> Vec<usize> {
        self.inner.decide(world, requests)
    }
    fn answers_deliberations(&self) -> bool {
        true
    }
    fn deliberations_opened(&mut self, _: &World, new: &[Deliberation]) {
        self.ids.extend(new.iter().map(|d| d.id));
    }
    fn deliberations_resolved(&mut self, world: &World) -> Vec<DeliberationAnswer> {
        let mut out = vec![DeliberationAnswer {
            id: DeliberationId(u64::MAX),
            choice: 0,
            confidence: 1.0,
        }];
        for &id in &self.ids {
            let choice = if world.deliberation(id).is_some() {
                99
            } else {
                0
            };
            out.push(DeliberationAnswer {
                id,
                choice,
                confidence: 1.0,
            });
        }
        out
    }
}

#[test]
fn invalid_and_late_answers_are_ignored() {
    let mut w = world(quiet());
    let (id, _) = tempt(&mut w);
    let mut brain = Bogus {
        inner: UtilityBrain::new(1),
        ids: Vec::new(),
    };
    w.tick(&mut brain);
    w.params.theft_temptation_per_hour = 0.0;
    let d = w.deliberation_of(id).cloned().expect("still open");
    assert!(w.deliberation_counters.answers_ignored >= 2);
    w.run(&mut brain, d.deadline - w.clock + 1);
    assert!(w.deliberation(d.id).is_none());
    let before = w.deliberation_counters.answers_ignored;
    w.run(&mut brain, 10);
    // Late valid answers change nothing.
    assert!(w.deliberation_counters.answers_ignored >= before + 10);
    assert_eq!(w.deliberation_counters.by_brain.iter().sum::<u64>(), 0);
    let times = w
        .recent_deliberations()
        .iter()
        .filter(|r| r.deliberation.id == d.id)
        .count();
    assert_eq!(times, 1);
}

/// A single woman and a single man, close in age, in different Dormitori,
/// made close friends.
fn sweethearts(w: &mut World, affinity: f32) -> (NpcId, NpcId) {
    let eligible =
        |n: &sim::Npc, sex| n.sex == sex && n.stage() == LifeStage::Adulto && n.partner().is_none();
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
    (w.npcs[a].id, w.npcs[b].id)
}

fn proposals_between<F>(brain: &Scripted<F>, a: NpcId, b: NpcId) -> usize {
    brain
        .seen
        .iter()
        .filter(|d| {
            matches!(d.kind, DeliberationKind::CoupleProposal { from }
                if (d.npc, from) == (a, b) || (d.npc, from) == (b, a))
        })
        .count()
}

#[test]
fn accepted_proposal_forms_a_couple() {
    let mut w = world(quiet());
    let (a, b) = sweethearts(&mut w, 0.9);
    let mut brain = always(1, Choice::Accept);
    past_midnight(&mut w, &mut brain);
    assert_eq!(proposals_between(&brain, a, b), 1);
    let d = brain
        .seen
        .iter()
        .find(|d| d.npc == a || d.npc == b)
        .unwrap();
    assert!(
        d.question.contains("diventare una coppia"),
        "{}",
        d.question
    );
    assert_eq!(d.options.len(), 3);
    let (x, y) = (w.npc(a).unwrap(), w.npc(b).unwrap());
    assert_eq!(x.partner(), Some(b));
    assert_eq!(y.partner(), Some(a));
    assert_eq!(x.home, y.home);
    assert_eq!(w.life.couples_formed_total, 1);
    assert!(w.events.iter().any(|e| matches!(e.kind,
        EventKind::Coupled { npc, partner, .. } if [npc, partner].contains(&a) && [npc, partner].contains(&b))));
}

#[test]
fn refused_proposal_hurts_and_is_not_repeated_soon() {
    let mut w = world(quiet());
    let (a, b) = sweethearts(&mut w, 0.9);
    let mut brain = always(1, Choice::Refuse);
    past_midnight(&mut w, &mut brain);
    assert_eq!(w.npc(a).unwrap().partner(), None);
    let affinity = w.npc(a).unwrap().affinity(b);
    assert!((0.55..0.65).contains(&affinity), "{affinity}");
    assert!(w.npc(b).unwrap().affinity(a) < 0.65);
    // Still above the couple threshold, but no new proposal for a while.
    for _ in 0..3 {
        past_midnight(&mut w, &mut brain);
    }
    assert_eq!(proposals_between(&brain, a, b), 1);
    assert_eq!(w.life.couples_formed_total, 0);
}

#[test]
fn asking_for_time_brings_the_proposal_back() {
    let mut w = world(quiet());
    let (a, b) = sweethearts(&mut w, 0.9);
    let mut brain = always(1, Choice::AskForTime);
    past_midnight(&mut w, &mut brain);
    assert_eq!(proposals_between(&brain, a, b), 1);
    past_midnight(&mut w, &mut brain);
    assert_eq!(proposals_between(&brain, a, b), 1, "not the next day");
    for _ in 0..w.params.proposal_retry_days {
        past_midnight(&mut w, &mut brain);
    }
    assert_eq!(proposals_between(&brain, a, b), 2);
    assert!((w.npc(a).unwrap().affinity(b) - 0.9).abs() < 0.2);
}

#[test]
fn trying_for_a_child_gives_births_waiting_does_not() {
    let params = SimParams {
        birth_chance_per_year: 1000.0,
        ..quiet()
    };
    let mut w = world(params.clone());
    let mut brain = always(1, Choice::TryForChild);
    past_midnight(&mut w, &mut brain);
    let asked: Vec<&Deliberation> = brain
        .seen
        .iter()
        .filter(|d| matches!(d.kind, DeliberationKind::HaveChild { .. }))
        .collect();
    assert!(!asked.is_empty());
    for d in &asked {
        let DeliberationKind::HaveChild { partner } = d.kind else {
            unreachable!()
        };
        let mother = w.npc(d.npc).unwrap();
        assert_eq!(mother.sex, Sex::Female);
        assert_eq!(mother.partner(), Some(partner));
        assert_eq!(d.options.len(), 2);
        assert!(d.question.contains("figlio"));
    }
    assert!(w.life.births_total > 0);
    assert_eq!(
        w.life.births_total + w.life.births_denied_total,
        asked.len() as u64
    );

    let mut w = world(params);
    let mut brain = always(1, Choice::Wait);
    past_midnight(&mut w, &mut brain);
    assert!(w.deliberation_counters.opened[1] > 0);
    assert_eq!(w.life.births_total, 0);
    assert_eq!(
        w.deliberation_counters.chosen(Choice::Wait),
        w.deliberation_counters.opened[1]
    );
}

#[test]
fn caught_thieves_pay_and_lose_trust() {
    let params = SimParams {
        theft_caught_base: 1.0,
        ..quiet()
    };
    let mut w = world(params);
    let (id, parent) = tempt(&mut w);
    let market = first_of(&w, CarriageKind::Mercato);
    let stock = w.carriages[market.index()].stock.get(ItemKind::Vestito);
    let affinity = w.npc(parent).unwrap().affinity(id);
    let mut brain = always(1, Choice::Steal);
    w.tick(&mut brain);
    let thief = w.npc(id).unwrap();
    assert_eq!(thief.inventory.tokens, 0, "fined");
    assert_eq!(thief.inventory.clothes, None);
    let others_stole = w.deliberation_counters.thefts - w.deliberation_counters.thefts_caught;
    assert_eq!(others_stole, 0, "everyone is caught");
    assert_eq!(
        w.carriages[market.index()].stock.get(ItemKind::Vestito),
        stock
    );
    let after = w.npc(parent).unwrap().affinity(id);
    assert!((affinity - after - w.params.theft_caught_affinity).abs() < 1e-4);
    let event = w
        .events
        .iter()
        .find(|e| matches!(e.kind, EventKind::Theft { npc, .. } if npc == id))
        .unwrap();
    assert!(matches!(
        event.kind,
        EventKind::Theft {
            caught: true,
            fine: 5,
            ..
        }
    ));
    assert!(event.to_string().contains("multa di 5 gettoni"), "{event}");
}

#[test]
fn unseen_thieves_keep_the_loot() {
    let params = SimParams {
        theft_caught_base: -10.0,
        ..quiet()
    };
    let mut w = world(params);
    let (id, _) = tempt(&mut w);
    let market = first_of(&w, CarriageKind::Mercato);
    let stock = w.carriages[market.index()].stock.get(ItemKind::Vestito);
    let mut brain = always(1, Choice::Steal);
    w.tick(&mut brain);
    let thief = w.npc(id).unwrap();
    assert_eq!(thief.inventory.tokens, 5);
    assert_eq!(thief.inventory.clothes, Some(1.0));
    let stolen = w.deliberation_counters.thefts;
    assert!(stolen >= 1 && w.deliberation_counters.thefts_caught == 0);
    let left = w.carriages[market.index()].stock.get(ItemKind::Vestito);
    assert!(left < stock, "{stock} -> {left}");
    assert!(w.events.iter().any(
        |e| matches!(e.kind, EventKind::Theft { npc, caught: false, fine: 0, .. } if npc == id)
    ));
}

#[test]
fn family_helps_those_who_ask() {
    let mut w = world(quiet());
    let (id, parent) = tempt(&mut w);
    let (i, p) = (index(&w, id), index(&w, parent));
    w.npcs[p].inventory.tokens = 100;
    for (x, other) in [(i, parent), (p, id)] {
        w.npcs[x]
            .relations
            .iter_mut()
            .find(|r| r.other == other)
            .unwrap()
            .affinity = 1.0;
    }
    let mut brain = always(1, Choice::AskForHelp);
    w.tick(&mut brain);
    let d = brain.seen.iter().find(|d| d.npc == id).unwrap();
    assert!(matches!(d.kind, DeliberationKind::Theft { helper: Some(h), .. } if h == parent));
    let market = first_of(&w, CarriageKind::Mercato);
    let price = w.price(market, ItemKind::Vestito).unwrap();
    let gift = price - 5;
    assert_eq!(w.npc(id).unwrap().inventory.tokens, 5 + gift);
    assert_eq!(w.npc(parent).unwrap().inventory.tokens, 100 - gift);
    assert!(w.events.iter().any(|e| matches!(e.kind,
        EventKind::HelpAsked { npc, helper, tokens, .. } if npc == id && helper == parent && tokens == gift)));
    assert!(w.deliberation_counters.help_given >= 1);
}

#[test]
fn denied_births_lead_to_protests_and_the_administration_gives_in() {
    let params = SimParams {
        birth_chance_per_year: 1000.0,
        birth_max_bed_occupancy: 0.5,
        protest_chance_on_denial: 1.0,
        protest_threshold: 3,
        ..quiet()
    };
    let mut w = world(params);
    let limit = w.max_population();
    let mut brain = always(1, Choice::Protest);
    past_midnight(&mut w, &mut brain);
    assert!(w.life.births_denied_total >= 3);
    let protests: Vec<&Deliberation> = brain
        .seen
        .iter()
        .filter(|d| {
            matches!(
                d.kind,
                DeliberationKind::Protest {
                    grievance: Grievance::BirthDenied,
                    ..
                }
            )
        })
        .collect();
    assert!(protests.len() >= 3, "{}", protests.len());
    assert!(protests.iter().all(|d| d.options.len() >= 2));
    // One gathering, tomorrow morning, in the front-most Mensa.
    let place = first_of(&w, CarriageKind::Mensa);
    let [g] = w.gatherings() else {
        panic!("{:?}", w.gatherings())
    };
    assert_eq!(g.place, place);
    assert_eq!(g.start, GameTime::from_dhm(2, 8, 0));
    assert_eq!(g.end, g.start + w.params.protest_hours * 60);
    assert_eq!(g.members.len(), protests.len());
    let members = g.members.clone();
    let called = w
        .events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::ProtestCalled { .. }))
        .count();
    assert_eq!(called, 1);
    // The administration gives in: more births allowed for a while.
    assert!(w.birth_bonus_until().is_some());
    assert!(w.max_population() > limit);
    assert!(w.events.iter().any(|e| matches!(
        e.kind,
        EventKind::AdminConceded {
            grievance: Grievance::BirthDenied,
            until: Some(_),
            ..
        }
    )));

    // During the protest everyone goes there and stands.
    w.run(&mut brain, GameTime::from_dhm(2, 10, 30) - w.clock);
    for &m in &members {
        let n = w.npc(m).unwrap();
        assert!(w.protest_of(m).is_some());
        assert!(
            n.carriage == place && n.action == Action::Idle
                || n.action == Action::Travel { to: place },
            "{} {:?} in {}",
            n.name,
            n.action,
            n.carriage
        );
    }
    let first = index(&w, members[0]);
    let text = w.describe_option(
        members[0],
        &sim::ActionOption {
            action: Action::Idle,
            minutes: 30,
            goal: None,
            description: String::new(),
        },
    );
    assert!(
        text.starts_with("protesta"),
        "{text} ({})",
        w.npcs[first].name
    );
    // Then it is over.
    w.run(&mut brain, GameTime::from_dhm(2, 12, 0) - w.clock + 1);
    assert!(w.gatherings().is_empty());
    assert!(members.iter().all(|&m| w.protest_of(m).is_none()));
}

#[test]
fn hunger_protests_bring_emergency_rations() {
    let params = SimParams {
        protest_chance_on_shortage: 1.0,
        protest_threshold: 3,
        ..quiet()
    };
    let mut w = world(params);
    for c in &mut w.carriages {
        c.stock.set(ItemKind::Razione, 0.0);
    }
    let hungry: Vec<NpcId> = w
        .npcs
        .iter_mut()
        .filter(|n| n.age >= LifeStage::ADULTO_FROM)
        .map(|n| {
            n.needs.hunger = 0.3;
            n.id
        })
        .collect();
    let mut brain = always(1, Choice::Protest);
    w.tick(&mut brain); // 06:00: the shortage is noticed on the hour.
    let asked = brain
        .seen
        .iter()
        .filter(|d| {
            matches!(
                d.kind,
                DeliberationKind::Protest {
                    grievance: Grievance::FoodShortage,
                    ..
                }
            )
        })
        .count();
    assert_eq!(asked, w.params.protest_max_on_shortage as usize);
    assert!(w.events.iter().any(|e| matches!(
        e.kind,
        EventKind::AdminConceded {
            grievance: Grievance::FoodShortage,
            ..
        }
    )));
    // Everyone hungry got a meal's worth.
    for id in hungry {
        assert!(w.npc(id).unwrap().needs.hunger > 0.8);
    }
}

#[test]
fn open_deliberations_survive_save_and_load() {
    let mut a = world(quiet());
    tempt(&mut a);
    a.tick(&mut silent(7));
    a.params.theft_temptation_per_hour = 0.0;
    assert!(!a.open_deliberations().is_empty());
    let json = serde_json::to_string(&a).unwrap();
    let mut b: World = serde_json::from_str(&json).unwrap();
    assert_eq!(b.open_deliberations(), a.open_deliberations());
    let (mut ba, mut bb) = (silent(9), silent(9));
    a.run(&mut ba, 4 * 60);
    b.run(&mut bb, 4 * 60);
    assert!(a.open_deliberations().is_empty());
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap()
    );
    // Nothing was shown twice to the brain after loading.
    assert!(ba.seen.is_empty() && bb.seen.is_empty());
}

#[test]
fn deliberations_are_deterministic() {
    let run = || {
        let mut w = World::generate(5, 20, 400);
        w.run(&mut UtilityBrain::new(5), 4 * DAY);
        w
    };
    let (a, b) = (run(), run());
    assert!(a.deliberation_counters.opened_total() > 0);
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap()
    );
}

#[test]
fn deliberations_stay_rare_and_do_not_flood_the_log() {
    let mut w = World::generate(42, 20, 400);
    w.params.max_events = usize::MAX;
    let days = 12;
    w.run(&mut UtilityBrain::new(42), days * DAY);
    let opened = w.deliberation_counters.opened_total();
    assert!(
        (days..=40 * days).contains(&opened),
        "{opened} deliberations in {days} days"
    );
    let logged = w
        .events
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                EventKind::DeliberationAsked { .. }
                    | EventKind::DeliberationResolved { .. }
                    | EventKind::Theft { .. }
                    | EventKind::HelpAsked { .. }
                    | EventKind::ProtestCalled { .. }
                    | EventKind::AdminConceded { .. }
            )
        })
        .count() as u64;
    assert!(logged <= 3 * opened, "{logged} events for {opened}");
    assert!(
        logged * 5 < w.events.len() as u64,
        "{logged} of {} events",
        w.events.len()
    );
}

#[test]
fn deliberations_whose_premise_is_gone_are_cancelled() {
    let mut w = world(quiet());
    let (id, _) = tempt(&mut w);
    let mut brain = silent(1);
    w.tick(&mut brain);
    w.params.theft_temptation_per_hour = 0.0;
    let d = w.deliberation_of(id).cloned().expect("tempted");
    // Someone gives the youth clothes: no reason to steal any more.
    w.player_give(id, ItemKind::Vestito).unwrap();
    let cancelled = w.deliberation_counters.cancelled;
    w.run(&mut brain, d.deadline - w.clock + 1);
    assert!(w.deliberation(d.id).is_none());
    assert_eq!(w.deliberation_counters.cancelled, cancelled + 1);
    assert!(!resolved_events(&w).iter().any(|e| e.0 == d.id));
}
