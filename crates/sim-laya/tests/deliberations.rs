//! `LayaBrain` e le deliberazioni: precedenza, soglia, ritardi, replay.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sim::{
    Brain, CarriageId, DecisionRequest, Deliberation, DeliberationAnswer, DeliberationKind,
    Grievance, MINUTES_PER_DAY, Resolver, UtilityBrain, World,
};
use sim_laya::{
    ChoiceAnswer, ChoiceModel, ChoiceQuery, DeliberationStatus, LayaBrain, LayaConfig, MockModel,
    ReplayBrain,
};

/// Un mondo piccolo dove le deliberazioni sono frequenti.
fn busy_world(seed: u64) -> World {
    let mut w = World::generate(seed, 10, 160);
    w.params.deliberation_rate = 6.0;
    w
}

fn all_carriages(w: &World) -> Vec<CarriageId> {
    w.carriages.iter().map(|c| c.id).collect()
}

fn wait_active(brain: &mut LayaBrain<UtilityBrain>) {
    let start = Instant::now();
    while !brain.is_active() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "model never loaded"
        );
        brain.poll();
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Stato confrontabile di un mondo (senza dipendere da serde qui).
fn snapshot(w: &World) -> String {
    format!(
        "{:?}\n{:?}\n{:?}\n{:?}\n{:?}",
        w.npcs,
        w.deliberation_counters,
        w.recent_deliberations(),
        w.open_deliberations(),
        w.events
    )
}

#[test]
fn sync_mode_answers_deliberations_in_the_same_tick() {
    let mut w = busy_world(1);
    let config = LayaConfig {
        deliberation_min_confidence: 0.0,
        ..LayaConfig::default()
    };
    let mut brain = LayaBrain::with_sync_model(UtilityBrain::new(1), MockModel::new(), config);
    assert!(brain.answers_deliberations());
    w.run(&mut brain, 3 * MINUTES_PER_DAY);
    let c = &w.deliberation_counters;
    let by_brain: u64 = c.by_brain.iter().sum();
    assert!(by_brain > 3, "{c:?}");
    assert_eq!(c.by_rules.iter().sum::<u64>(), 0, "{c:?}");
    assert_eq!(c.answers_ignored, 0);
    for r in w.recent_deliberations() {
        assert_eq!(r.by, Resolver::Brain);
        assert_eq!(
            r.resolved, r.deliberation.asked,
            "answered in the same tick"
        );
        assert!(r.confidence.is_some());
    }
    let s = &brain.stats().deliberations;
    assert_eq!(s.applied_total(), by_brain);
    assert_eq!(s.asked_total(), c.opened_total());
    assert_eq!(s.late_total(), 0);
    assert!(s.agreement(None).is_some());
    // Il mock segue a grandi linee la regola.
    assert!(s.agreement(None).unwrap() > 0.5, "{s:?}");
    let info = brain
        .deliberation_info(w.recent_deliberations()[0].deliberation.id)
        .expect("info");
    assert_eq!(info.status, DeliberationStatus::Applied);
    assert!(info.model.is_some() && info.confidence.is_some());
    // Le azioni di tutti i giorni restano contate a parte.
    assert!(brain.stats().jobs_sent > 0);
}

#[test]
fn unsure_answers_leave_deliberations_to_the_rules_at_the_deadline() {
    let mut w = busy_world(2);
    let config = LayaConfig {
        deliberation_min_confidence: 0.9,
        ..LayaConfig::default()
    };
    // Temperatura altissima: probabilità quasi uniformi.
    let model = MockModel::new().with_temperature(100.0);
    let mut brain = LayaBrain::with_sync_model(UtilityBrain::new(2), model, config);
    w.run(&mut brain, 3 * MINUTES_PER_DAY);
    let c = &w.deliberation_counters;
    assert_eq!(c.by_brain.iter().sum::<u64>(), 0, "{c:?}");
    assert!(c.by_rules.iter().sum::<u64>() > 3, "{c:?}");
    for r in w.recent_deliberations() {
        assert_eq!(r.by, Resolver::Rules);
        assert_eq!(r.resolved, r.deliberation.deadline, "the rule waited");
    }
    let s = &brain.stats().deliberations;
    assert!(s.low_confidence_total() > 3, "{s:?}");
    assert_eq!(s.applied_total(), 0);
}

#[test]
fn late_answers_are_ignored() {
    let mut w = busy_world(3);
    let model = MockModel::new().with_latency(Duration::from_millis(1500), Duration::ZERO);
    let config = LayaConfig {
        deliberation_min_confidence: 0.0,
        // Solo deliberazioni: le azioni non intasano il modello lento.
        budget_per_call: 0,
        ..LayaConfig::default()
    };
    let mut brain = LayaBrain::with_model(UtilityBrain::new(3), model, config);
    wait_active(&mut brain);
    // Due giorni di gioco passano in meno di 1.5 s: tutte le scadenze arrivano prima.
    w.run(&mut brain, 2 * MINUTES_PER_DAY);
    assert!(w.deliberation_counters.opened_total() > 0);
    std::thread::sleep(Duration::from_millis(1700));
    w.run(&mut brain, 10);
    let c = &w.deliberation_counters;
    assert_eq!(c.by_brain.iter().sum::<u64>(), 0, "{c:?}");
    assert_eq!(c.answers_ignored, 0, "late answers never reach the world");
    let s = &brain.stats().deliberations;
    assert!(s.asked_total() > 0);
    assert!(s.late_total() > 0, "{s:?}");
    assert_eq!(s.applied_total(), 0);
    assert!(
        brain
            .deliberation_infos()
            .any(|i| i.status == DeliberationStatus::Late)
    );
}

/// Registra l'ordine delle domande e le fa aspettare un po'.
struct Recorder {
    order: Arc<Mutex<Vec<String>>>,
    latency: Duration,
}

impl ChoiceModel for Recorder {
    fn name(&self) -> &str {
        "recorder"
    }

    fn predict_batch(&mut self, queries: &[ChoiceQuery]) -> Result<Vec<ChoiceAnswer>, String> {
        std::thread::sleep(self.latency);
        let mut order = self.order.lock().unwrap();
        for q in queries {
            order.push(q.instructions.clone());
        }
        Ok(queries
            .iter()
            .map(|q| ChoiceAnswer {
                probabilities: vec![1.0 / q.options.len() as f32; q.options.len()],
            })
            .collect())
    }
}

#[test]
fn deliberations_jump_the_queue_of_everyday_actions() {
    let mut w = World::generate(4, 10, 160);
    let order = Arc::new(Mutex::new(Vec::new()));
    let model = Recorder {
        order: order.clone(),
        latency: Duration::from_millis(20),
    };
    let config = LayaConfig {
        budget_per_call: 64,
        max_in_flight: 256,
        batch_rows: 4,
        deliberation_min_confidence: 0.0,
        cache_capacity: 0,
        ..LayaConfig::default()
    };
    let mut brain = LayaBrain::with_model(UtilityBrain::new(4), model, config);
    brain.set_focus(all_carriages(&w));
    wait_active(&mut brain);
    // Molte domande di azioni in coda...
    let ids: Vec<_> = w.npcs.iter().map(|n| n.id).collect();
    let requests: Vec<DecisionRequest> = ids
        .iter()
        .map(|&id| DecisionRequest {
            npc: id,
            options: w.options(id),
        })
        .filter(|r| r.options.len() >= 2)
        .take(40)
        .collect();
    brain.decide(&w, &requests);
    let actions = brain.stats().jobs_sent as usize;
    assert!(actions >= 30, "{actions}");
    // ...poi una deliberazione.
    let npc = w
        .npcs
        .iter()
        .find(|n| n.stage().works())
        .map(|n| n.id)
        .unwrap();
    let place = w.carriages[0].id;
    let id = w
        .open_deliberation(
            npc,
            DeliberationKind::Protest {
                grievance: Grievance::FoodShortage,
                place,
            },
        )
        .expect("opened");
    let d: Deliberation = w.deliberation(id).cloned().unwrap();
    brain.deliberations_opened(&w, std::slice::from_ref(&d));
    assert_eq!(brain.pending_deliberations().count(), 1);
    let start = Instant::now();
    let mut answers: Vec<DeliberationAnswer> = Vec::new();
    while answers.is_empty() {
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(2));
        answers = brain.deliberations_resolved(&w);
    }
    assert_eq!(answers[0].id, id);
    let order = order.lock().unwrap().clone();
    let at = order.iter().position(|q| *q == d.question).unwrap();
    // Al più un lotto di azioni (4 righe) prima della deliberazione.
    assert!(
        at <= 4,
        "deliberation served at position {at} of {}",
        order.len()
    );
    assert!(
        order.len() < actions + 1,
        "the deliberation did not wait for all actions"
    );
}

#[test]
fn async_runs_with_deliberations_replay_exactly() {
    let seed = 5;
    let mut w = busy_world(seed);
    let start = w.clone();
    let model = MockModel::new().with_latency(Duration::from_millis(1), Duration::ZERO);
    let config = LayaConfig {
        record_log: true,
        budget_per_call: 4,
        deliberation_min_confidence: 0.55,
        ..LayaConfig::default()
    };
    let mut brain = LayaBrain::with_model(UtilityBrain::new(seed), model, config);
    brain.set_focus([CarriageId(0), CarriageId(3)]);
    wait_active(&mut brain);
    for _ in 0..3 * 24 {
        w.run(&mut brain, 60);
        std::thread::sleep(Duration::from_millis(1));
    }
    let c = w.deliberation_counters.clone();
    assert!(c.by_brain.iter().sum::<u64>() > 0, "{c:?}");
    let s = brain.stats().deliberations.clone();
    assert!(s.applied_total() > 0, "{s:?}");

    let log = brain.take_log();
    let delib = brain.take_deliberation_log();
    assert_eq!(delib.answers.len() as u64, s.applied_total());
    assert_eq!(delib.modes.first().map(|m| m.1), Some(true));
    let mut replay = ReplayBrain::new(&log, brain.config().think_minutes).with_deliberations(delib);
    let mut again = start.clone();
    again.run(&mut replay, 3 * 24 * 60);
    assert_eq!(replay.missing, 0);
    assert_eq!(snapshot(&again), snapshot(&w));

    // Senza il registro delle deliberazioni il replay diverge (la regola decide subito).
    let mut plain = ReplayBrain::new(&log, brain.config().think_minutes);
    let mut other = start;
    other.run(&mut plain, 3 * 24 * 60);
    assert_ne!(snapshot(&other), snapshot(&w));
}

#[test]
fn without_an_active_model_deliberations_stay_with_the_rules() {
    let reference = {
        let mut w = busy_world(6);
        w.run(&mut UtilityBrain::new(6), 2 * MINUTES_PER_DAY);
        snapshot(&w)
    };
    assert!(reference.contains("DeliberationResolved"));
    let brains = [
        // Nessun modello.
        LayaBrain::new(UtilityBrain::new(6), LayaConfig::default()),
        // Laya spento.
        LayaBrain::with_sync_model(
            UtilityBrain::new(6),
            MockModel::new(),
            LayaConfig {
                enabled: false,
                ..LayaConfig::default()
            },
        ),
    ];
    for mut brain in brains {
        assert!(!brain.answers_deliberations());
        let mut w = busy_world(6);
        w.run(&mut brain, 2 * MINUTES_PER_DAY);
        assert_eq!(snapshot(&w), reference);
        assert_eq!(brain.stats().deliberations.asked_total(), 0);
    }
    // Deliberazioni spente: il modello decide solo le azioni.
    let mut brain = LayaBrain::with_sync_model(
        UtilityBrain::new(6),
        MockModel::new(),
        LayaConfig {
            deliberations: false,
            ..LayaConfig::default()
        },
    );
    assert!(!brain.answers_deliberations());
    let mut w = busy_world(6);
    w.run(&mut brain, 2 * MINUTES_PER_DAY);
    assert_eq!(w.deliberation_counters.by_brain.iter().sum::<u64>(), 0);
    assert!(w.deliberation_counters.by_rules.iter().sum::<u64>() > 0);
}

#[test]
fn open_deliberations_are_asked_again_after_a_reset() {
    let mut w = busy_world(7);
    // Soglia irraggiungibile: le deliberazioni restano aperte fino alla scadenza.
    let mut brain = LayaBrain::with_sync_model(
        UtilityBrain::new(7),
        MockModel::new(),
        LayaConfig {
            deliberation_min_confidence: 1.1,
            ..LayaConfig::default()
        },
    );
    let start = w.clock;
    while w.open_deliberations().is_empty() {
        assert!(
            w.clock.since(start) < 3 * MINUTES_PER_DAY,
            "no deliberation"
        );
        w.tick(&mut brain);
    }
    let open: Vec<_> = w.open_deliberations().iter().map(|d| d.id).collect();
    for &id in &open {
        assert_eq!(
            brain.deliberation_info(id).map(|i| i.status),
            Some(DeliberationStatus::LowConfidence)
        );
    }
    // Dopo un reset (es. partita caricata) il cervello le richiede al modello.
    brain.reset();
    brain.config_mut().deliberation_min_confidence = 0.0;
    w.tick(&mut brain);
    for id in open {
        assert!(w.deliberation(id).is_none());
        let r = w
            .recent_deliberations()
            .iter()
            .find(|r| r.deliberation.id == id)
            .expect("resolved");
        assert_eq!(r.by, Resolver::Brain);
    }
}
