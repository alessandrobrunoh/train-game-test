//! Conversazioni tra NPC: accoppiamento a due, fine simmetrica, morte,
//! battute, pettegolezzi, determinismo e salvataggio.

use sim::{
    Action, ActionKind, Brain, CarriageKind, Conversation, DecisionRequest, Event, EventKind,
    ItemKind, MINUTES_PER_DAY, News, NpcId, Personality, Relation, RelationKind, Sex, SimParams,
    Temper, Tone, Topic, UtilityBrain, World,
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

fn index(w: &World, id: NpcId) -> usize {
    w.npcs.iter().position(|n| n.id == id).unwrap()
}

/// Lives like [`UtilityBrain`], but `pick` may choose an option instead.
struct Scripted<F> {
    inner: UtilityBrain,
    pick: F,
}

impl<F: FnMut(&World, &DecisionRequest) -> Option<usize>> Brain for Scripted<F> {
    fn decide(&mut self, world: &World, requests: &[DecisionRequest]) -> Vec<usize> {
        let mut choices = self.inner.decide(world, requests);
        for (k, req) in requests.iter().enumerate() {
            if let Some(c) = (self.pick)(world, req) {
                choices[k] = c;
            }
        }
        choices
    }

    fn wants_descriptions(&self) -> bool {
        false
    }
}

fn scripted<F: FnMut(&World, &DecisionRequest) -> Option<usize>>(pick: F) -> Scripted<F> {
    Scripted {
        inner: UtilityBrain::new(1),
        pick,
    }
}

/// The option of `req` doing `action`, if offered.
fn option(req: &DecisionRequest, action: Action) -> Option<usize> {
    req.options.iter().position(|o| o.action == action)
}

/// Sets the tie `a` → `b` (friend) to `affinity`.
fn tie(w: &mut World, a: usize, b: usize, affinity: f32) {
    let other = w.npcs[b].id;
    let rels = &mut w.npcs[a].relations;
    match rels.iter_mut().find(|r| r.other == other) {
        Some(r) => r.affinity = affinity,
        None => rels.push(Relation {
            other,
            kind: RelationKind::Friend,
            affinity,
        }),
    }
}

/// Two adults `a` and `b` (index of `b` lower) without family ties, close
/// friends, in the same carriage. Everyone else keeps idling for a long
/// while; `a` decides at the next tick, `b` too if `b_decides`.
fn pair(w: &mut World, b_decides: bool) -> (usize, usize) {
    let adult_alone = |w: &World, i: usize| {
        let n = &w.npcs[i];
        n.age >= 18 && n.age < 60 && n.relations.iter().all(|r| !r.kind.is_family())
    };
    let a = (1..w.npcs.len()).find(|&i| adult_alone(w, i)).unwrap();
    let b = (0..a).find(|&j| w.npcs[j].age >= 18).unwrap();
    let now = w.clock;
    for n in &mut w.npcs {
        n.action = Action::Idle;
        n.action_until = now + 2000;
    }
    let here = w.npcs[a].carriage;
    w.npcs[b].carriage = here;
    w.npcs[a].action_until = now;
    if b_decides {
        w.npcs[b].action_until = now;
    }
    tie(w, a, b, 0.9);
    tie(w, b, a, 0.9);
    (a, b)
}

fn check_lines(c: &Conversation) {
    // Fewer than 3 only if cut short by a death.
    assert!(!c.lines.is_empty() && c.lines.len() <= 6, "{c:?}");
    for (k, l) in c.lines.iter().enumerate() {
        assert!(l.at >= c.since && l.at <= c.until, "{c:?}");
        assert!(
            l.text.chars().count() <= sim::dialogue::MAX_LINE_CHARS,
            "{}",
            l.text
        );
        assert!(!l.text.is_empty() && !l.text.contains('{'), "{}", l.text);
        let speaker = if k % 2 == 0 { c.a } else { c.b };
        assert_eq!(l.speaker, speaker);
    }
    assert!(c.lines.windows(2).all(|w| w[0].at <= w[1].at));
}

#[test]
fn a_chat_with_someone_free_is_two_sided_and_ends_for_both() {
    let mut w = World::generate_with_params(42, 10, 100, quiet());
    let (a, b) = pair(&mut w, false);
    let (ida, idb) = (w.npcs[a].id, w.npcs[b].id);
    let mut used = false;
    let mut brain = scripted(|_: &World, req: &DecisionRequest| {
        if req.npc != ida || used {
            return None;
        }
        used = true;
        Some(option(req, Action::Socialize(idb)).expect("the friend is offered"))
    });
    let start = w.clock;
    w.tick(&mut brain);
    let c = w.conversation_of(ida).expect("a conversation").clone();
    assert_eq!((c.a, c.b), (ida, idb));
    assert_eq!(w.conversation_of(idb), Some(&c));
    assert_eq!(c.since, start);
    assert_eq!(w.npcs[b].action, Action::Socialize(ida));
    assert_eq!(w.npcs[a].action, Action::Socialize(idb));
    assert_eq!(w.npcs[a].action_until, c.until);
    assert_eq!(w.npcs[b].action_until, c.until);
    assert_eq!(c.carriage, w.npcs[a].carriage);
    check_lines(&c);
    assert!(c.lines.len() >= 3);
    assert_eq!(w.conversation_counters.conversations, 1);

    let before = w.npcs[a].affinity(idb);
    let social = (w.npcs[a].needs.social, w.npcs[b].needs.social);
    while w.clock < c.until {
        w.tick(&mut brain);
        assert!(w.conversation_of(ida).is_some());
    }
    // Both gained while talking.
    assert!(w.npcs[a].needs.social >= social.0.min(0.99));
    assert!(w.npcs[b].needs.social >= social.1.min(0.99));
    w.tick(&mut brain);
    assert!(w.conversations().iter().all(|x| x.id != c.id));
    assert_eq!(w.recent_conversations().last().map(|x| x.id), Some(c.id));
    for i in [a, b] {
        assert_eq!(w.npcs[i].action_since, c.until, "both decided again");
    }
    let after = w.npcs[a].affinity(idb);
    match c.tone {
        Tone::Tense => assert!(after < before),
        _ => assert!(after > before),
    }
    assert_eq!(after, w.npcs[b].affinity(ida), "both sides change alike");
}

#[test]
fn a_busy_or_hostile_partner_gets_a_short_one_sided_chat() {
    // Hostile: `b` can't stand `a`.
    let mut w = World::generate_with_params(42, 10, 100, quiet());
    let (a, b) = pair(&mut w, false);
    let (ida, idb) = (w.npcs[a].id, w.npcs[b].id);
    tie(&mut w, b, a, -0.9);
    let mut brain = scripted(|_: &World, req: &DecisionRequest| {
        (req.npc == ida).then(|| option(req, Action::Socialize(idb)).unwrap())
    });
    let now = w.clock;
    w.tick(&mut brain);
    assert!(w.conversations().is_empty());
    assert_eq!(w.npcs[a].action, Action::Socialize(idb));
    assert!(w.npcs[a].action_until.since(now) <= w.params.one_sided_chat_minutes);
    assert_eq!(w.npcs[b].action, Action::Idle);
    assert_eq!(w.conversation_counters.one_sided, 1);

    // Busy: `b` decides first in the same tick and leaves.
    let mut w = World::generate_with_params(42, 10, 100, quiet());
    let (a, b) = pair(&mut w, true);
    let (ida, idb) = (w.npcs[a].id, w.npcs[b].id);
    let mut brain = scripted(|_: &World, req: &DecisionRequest| {
        if req.npc == idb {
            req.options
                .iter()
                .position(|o| matches!(o.action, Action::Travel { .. }))
        } else if req.npc == ida {
            option(req, Action::Socialize(idb))
        } else {
            None
        }
    });
    w.tick(&mut brain);
    assert!(matches!(w.npcs[b].action, Action::Travel { .. }));
    assert_eq!(w.npcs[a].action, Action::Socialize(idb));
    assert!(w.conversation_of(ida).is_none());
    assert_eq!(w.conversation_counters.one_sided, 1);
}

#[test]
fn an_eating_partner_talks_along_until_the_meal_ends() {
    let mut w = World::generate_with_params(42, 10, 100, quiet());
    let (a, b) = pair(&mut w, true);
    let mensa = w
        .carriages
        .iter()
        .find(|c| c.kind == CarriageKind::Mensa)
        .unwrap()
        .id;
    w.npcs[a].carriage = mensa;
    w.npcs[b].carriage = mensa;
    w.npcs[b].needs.hunger = 0.2;
    let (ida, idb) = (w.npcs[a].id, w.npcs[b].id);
    let mut brain = scripted(|_: &World, req: &DecisionRequest| {
        if req.npc == idb {
            req.options
                .iter()
                .position(|o| matches!(o.action, Action::Eat(_)))
        } else if req.npc == ida {
            option(req, Action::Socialize(idb))
        } else {
            None
        }
    });
    w.tick(&mut brain);
    assert!(matches!(w.npcs[b].action, Action::Eat(_)));
    let c = w.conversation_of(ida).expect("talks while eating").clone();
    assert_eq!(c.b, idb);
    assert!(c.until <= w.npcs[b].action_until);
    assert_eq!(w.npcs[a].action_until, c.until);
    assert_eq!(w.conversation_counters.while_eating, 1);
    check_lines(&c);
}

#[test]
fn a_death_ends_the_conversation_for_the_other() {
    let mut w = World::generate_with_params(42, 10, 100, quiet());
    let (a, b) = pair(&mut w, false);
    let (ida, idb) = (w.npcs[a].id, w.npcs[b].id);
    let mut used = false;
    let mut brain = scripted(|_: &World, req: &DecisionRequest| {
        if req.npc != ida || used {
            return None;
        }
        used = true;
        option(req, Action::Socialize(idb))
    });
    w.tick(&mut brain);
    let c = w.conversation_of(ida).unwrap().clone();
    w.run(&mut brain, 5);
    // `b` starves to death in the next minute.
    let starve = w.params.starvation_minutes;
    let b = index(&w, idb);
    w.npcs[b].needs.hunger = 0.0;
    w.npcs[b].starving_minutes = starve - 1;
    let death = w.clock;
    w.tick(&mut brain);
    assert!(w.npc(idb).is_none());
    assert!(w.conversation_of(ida).is_none());
    let ended = w.recent_conversations().last().unwrap();
    assert_eq!(ended.id, c.id);
    assert_eq!(ended.until, death);
    assert!(ended.lines.iter().all(|l| l.at <= death));
    assert_eq!(w.conversation_counters.cut_short, 1);
    let a = index(&w, ida);
    assert_eq!(w.npcs[a].action, Action::Idle);
    assert_eq!(w.npcs[a].action_until, death);
    w.tick(&mut brain);
    assert_eq!(w.npcs[a].action_since, death + 1, "decides again at once");
}

/// Invariants of every conversation over a few days of normal life.
#[test]
fn conversations_stay_consistent_over_days() {
    let mut w = World::generate(7, 10, 100);
    let mut brain = UtilityBrain::new(7);
    for _ in 0..3 * DAY {
        w.tick(&mut brain);
        for c in w.conversations() {
            let (a, b) = (w.npc(c.a).unwrap(), w.npc(c.b).unwrap());
            assert!(w.same_place(a, b), "{c:?}");
            assert_eq!(a.action, Action::Socialize(b.id));
            assert!(
                b.action == Action::Socialize(a.id) || matches!(b.action, Action::Eat(_)),
                "{:?}",
                b.action
            );
            assert!(c.until.0 + 1 > w.clock.0 && c.since < c.until);
            assert!(
                w.conversations()
                    .iter()
                    .filter(|x| x.involves(a.id) || x.involves(b.id))
                    .count()
                    == 1
            );
        }
    }
    let counters = &w.conversation_counters;
    assert!(counters.conversations > 300, "{counters:?}");
    assert!(counters.two_sided_share() > 0.7, "{counters:?}");
    assert!(
        counters.by_topic.iter().filter(|&&n| n > 0).count() >= 5,
        "{counters:?}"
    );
    assert!(counters.by_tone.iter().all(|&n| n > 0), "{counters:?}");
    assert!(w.recent_conversations().len() <= w.params.recent_conversations_kept);
    for c in w.recent_conversations().iter().chain(w.conversations()) {
        check_lines(c);
    }
    // Notable ones are logged, but rarely.
    let logged = w
        .events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::Chat { .. }))
        .count() as u64;
    assert_eq!(logged, counters.logged);
    assert!(logged >= 1 && logged <= 3 * 24 / w.params.conversation_log_hours * 2 + 2);
    for e in w
        .events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::Chat { .. }))
    {
        let text = e.to_string();
        assert!(text.contains('«') && text.contains('»'), "{text}");
    }
}

#[test]
fn open_conversations_survive_save_and_load() {
    let mut a = World::generate(42, 10, 100);
    let mut brain_a = UtilityBrain::new(42);
    // Evening: plenty of chats going on.
    a.run(&mut brain_a, DAY + 13 * 60);
    assert!(!a.conversations().is_empty());
    let json = serde_json::to_string(&a).unwrap();
    let mut b: World = serde_json::from_str(&json).unwrap();
    assert_eq!(b.conversations(), a.conversations());
    let mut brain_b = brain_a.clone();
    a.run(&mut brain_a, 6 * 60);
    b.run(&mut brain_b, 6 * 60);
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap()
    );
}

#[test]
fn conversations_are_deterministic() {
    let run = || {
        let mut w = World::generate(3, 10, 100);
        w.run(&mut UtilityBrain::new(3), 2 * DAY);
        w
    };
    let (a, b) = (run(), run());
    assert!(a.conversation_counters.conversations > 0);
    assert_eq!(a.recent_conversations(), b.recent_conversations());
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap()
    );
}

/// Gossip about a caught thief lowers the listener's opinion of them.
#[test]
fn gossip_about_a_thief_spreads_distrust() {
    let mut base = World::generate_with_params(42, 10, 100, quiet());
    let (a, b) = pair(&mut base, false);
    let (ida, idb) = (base.npcs[a].id, base.npcs[b].id);
    // A third adult both know: the thief.
    let x = (0..base.npcs.len())
        .find(|&k| k != a && k != b && base.npcs[k].age >= 18)
        .unwrap();
    let idx = base.npcs[x].id;
    tie(&mut base, a, x, 0.4);
    tie(&mut base, b, x, 0.5);
    // Two gossips.
    let gossip = Personality::default()
        .with(Temper::Pettegolo)
        .with(Temper::Curioso);
    base.npcs[a].personality = Some(gossip);
    base.npcs[b].personality = Some(gossip);
    let thief = &base.npcs[x];
    let theft = EventKind::Theft {
        npc: idx,
        name: thief.name.clone(),
        sex: thief.sex,
        item: ItemKind::Attrezzo,
        carriage: thief.carriage,
        caught: true,
        fine: 10,
    };
    base.events.push(Event {
        time: base.clock,
        kind: theft,
    });

    let mut found = 0;
    for trial in 0..60 {
        let mut w = base.clone();
        // A different world RNG state per trial.
        for _ in 0..trial {
            w.options(ida);
        }
        let mut used = false;
        let mut brain = scripted(|_: &World, req: &DecisionRequest| {
            if req.npc != ida || used {
                return None;
            }
            used = true;
            option(req, Action::Socialize(idb))
        });
        w.tick(&mut brain);
        let Some(c) = w.conversation_of(ida).cloned() else {
            continue;
        };
        if c.about != Some(idx) || c.news != Some(News::TheftCaught) || c.tone == Tone::Tense {
            continue;
        }
        assert!(matches!(c.topic, Topic::Gossip | Topic::News));
        let before = w.npcs[b].affinity(idx);
        let teller = w.npcs[a].affinity(idx);
        while w.conversations().iter().any(|x| x.id == c.id) {
            w.tick(&mut brain);
        }
        let after = w.npc(idb).unwrap().affinity(idx);
        assert!(
            (before - after - w.params.gossip_affinity).abs() < 1e-5,
            "{before} -> {after}"
        );
        assert_eq!(
            w.npc(ida).unwrap().affinity(idx),
            teller,
            "the teller's own opinion stays"
        );
        found += 1;
        let first = &c.lines[0].text;
        let name = w.npcs[x].first_name().to_string();
        assert!(first.contains(&name) || c.topic == Topic::News, "{first}");
    }
    assert!(found >= 3, "only {found} conversations about the thief");
}

#[test]
fn chat_options_say_with_whom_and_what_about() {
    let mut w = World::generate_with_params(42, 10, 100, quiet());
    let (a, b) = pair(&mut w, false);
    let idb = w.npcs[b].id;
    let name = w.npcs[b].name.clone();
    let friend = w.npcs[b].sex.pick("amica", "amico");
    let options = w.options(w.npcs[a].id);
    let chat = options
        .iter()
        .find(|o| o.action == Action::Socialize(idb))
        .unwrap();
    assert!(
        chat.description
            .starts_with(&format!("chiacchiera con {name} ({friend}) ")),
        "{}",
        chat.description
    );
    assert!(
        chat.description
            .ends_with(&format!("({} min)", chat.minutes))
    );
    let topic = w.likely_topic(w.npcs[a].id, idb).unwrap();
    assert!(chat.description.contains(topic.about_phrase()));
}

#[test]
fn everyone_has_a_personality_and_newborns_inherit_one() {
    let params = SimParams {
        birth_chance_per_year: 3.0,
        ..SimParams::default()
    };
    let mut w = World::generate_with_params(5, 10, 100, params);
    assert!(w.npcs.iter().all(|n| {
        n.personality
            .is_some_and(|p| (2..=Personality::MAX_TAGS).contains(&p.len()))
    }));
    let founders = w.npcs.len();
    w.run(&mut UtilityBrain::new(5), 12 * DAY);
    let born: Vec<_> = w.npcs.iter().filter(|n| !w.is_founder(n.id)).collect();
    assert!(!born.is_empty(), "no births among {founders}");
    for n in born {
        let p = n.personality.expect("drawn at birth");
        assert!((2..=3).contains(&p.len()));
    }
    // Saves from before personalities: derived from the id, stable.
    let mut old = w.npcs[0].clone();
    old.personality = None;
    assert_eq!(old.personality(), old.personality());
    assert!((2..=3).contains(&old.personality().len()));
    assert!(!old.personality().describe(Sex::Female).is_empty());
}

#[test]
fn stats_count_conversations() {
    let mut w = World::generate(42, 10, 100);
    w.run(&mut UtilityBrain::new(42), DAY);
    let s = sim::Stats::of(&w);
    assert_eq!(s.conversations_open, w.conversations().len());
    assert_eq!(s.conversations, w.conversation_counters);
    assert!(s.count(ActionKind::Socialize) >= s.conversations_open);
    assert!(s.to_string().contains("conversazioni"));
}
