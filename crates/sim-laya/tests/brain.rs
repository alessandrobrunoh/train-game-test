use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use sim::{Brain, CarriageId, DecisionRequest, MINUTES_PER_DAY, NpcId, THINK, UtilityBrain, World};
use sim_laya::brain::top_k;
use sim_laya::{
    ChoiceAnswer, ChoiceModel, ChoiceQuery, LayaBrain, LayaConfig, MockModel, ModelStatus,
    ReplayBrain, Source,
};

fn world(seed: u64) -> World {
    World::generate(seed, 10, 120)
}

fn all_carriages(w: &World) -> Vec<CarriageId> {
    w.carriages.iter().map(|c| c.id).collect()
}

fn request(w: &mut World, id: NpcId) -> DecisionRequest {
    DecisionRequest {
        npc: id,
        options: w.options(id),
    }
}

/// An NPC with at least `n` options right now.
fn npc_with_options(w: &mut World, n: usize) -> NpcId {
    let ids: Vec<NpcId> = w.npcs.iter().map(|n| n.id).collect();
    ids.into_iter()
        .find(|&id| w.clone().options(id).len() >= n)
        .expect("an NPC with enough options")
}

/// Polls until `done` holds (the worker answers asynchronously).
fn wait_for(brain: &mut LayaBrain<UtilityBrain>, done: impl Fn(&LayaBrain<UtilityBrain>) -> bool) {
    let start = Instant::now();
    while !done(brain) {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "worker timed out"
        );
        brain.poll();
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Always picks the last option offered, with certainty.
struct PickLast;

impl ChoiceModel for PickLast {
    fn name(&self) -> &str {
        "last"
    }

    fn predict_batch(&mut self, queries: &[ChoiceQuery]) -> Result<Vec<ChoiceAnswer>, String> {
        Ok(queries
            .iter()
            .map(|q| {
                let n = q.options.len();
                ChoiceAnswer {
                    probabilities: (0..n).map(|i| if i + 1 == n { 1.0 } else { 0.0 }).collect(),
                }
            })
            .collect())
    }
}

/// Checks every choice of the wrapped brain.
struct Checked<'a>(&'a mut dyn Brain);

impl Brain for Checked<'_> {
    fn wants_descriptions(&self) -> bool {
        self.0.wants_descriptions()
    }

    fn think_minutes(&self) -> u64 {
        self.0.think_minutes()
    }

    fn decide(&mut self, world: &World, requests: &[DecisionRequest]) -> Vec<usize> {
        let out = self.0.decide(world, requests);
        assert_eq!(out.len(), requests.len());
        for (r, &c) in requests.iter().zip(&out) {
            assert!(c == THINK || c < r.options.len(), "invalid choice {c}");
        }
        out
    }
}

fn sync_brain(model: impl ChoiceModel, config: LayaConfig) -> LayaBrain<UtilityBrain> {
    LayaBrain::with_sync_model(UtilityBrain::new(1), model, config)
}

#[test]
fn decide_never_blocks_on_a_slow_model() {
    let mut w = world(1);
    let model = MockModel::new().with_latency(Duration::from_millis(300), Duration::ZERO);
    let mut brain = LayaBrain::with_model(UtilityBrain::new(1), model, LayaConfig::default());
    brain.set_focus(all_carriages(&w));
    wait_for(&mut brain, |b| b.is_active());
    for _ in 0..30 {
        let start = Instant::now();
        w.tick(&mut brain);
        assert!(start.elapsed() < Duration::from_millis(250), "tick blocked");
    }
    assert!(brain.stats().jobs_sent > 0);
}

#[test]
fn budget_and_queue_are_respected() {
    let mut w = world(2);
    let model = MockModel::new().with_latency(Duration::from_millis(200), Duration::ZERO);
    let rows = model.rows_counter();
    let config = LayaConfig {
        budget_per_call: 3,
        max_in_flight: 7,
        ..LayaConfig::default()
    };
    let mut brain = LayaBrain::with_model(UtilityBrain::new(2), model, config);
    brain.set_focus(all_carriages(&w));
    wait_for(&mut brain, |b| b.is_active());
    let mut sent_before = 0;
    for _ in 0..20 {
        w.tick(&mut brain);
        let sent = brain.stats().jobs_sent;
        assert!(sent - sent_before <= 3, "more than the budget in one call");
        assert!(brain.stats().queue <= 7, "queue over its cap");
        sent_before = sent;
    }
    assert!(sent_before >= 3);
    assert!(rows.load(Ordering::Relaxed) as u64 <= sent_before);
}

#[test]
fn fresh_answers_apply_and_stale_ones_are_rejected() {
    let mut w = world(3);
    let id = npc_with_options(&mut w, 3);
    let config = LayaConfig {
        think: false,
        ..LayaConfig::default()
    };
    let mut brain = LayaBrain::with_model(UtilityBrain::new(3), PickLast, config);
    let here = w.npc(id).unwrap().carriage;
    brain.set_focus([here]);
    wait_for(&mut brain, |b| b.is_active());

    // Same situation: the answer applies.
    let req = request(&mut w, id);
    brain.decide(&w, std::slice::from_ref(&req));
    assert_eq!(brain.stats().jobs_sent, 1);
    assert!(brain.is_pending(id));
    wait_for(&mut brain, |b| b.stats().jobs_answered == 1);
    brain.decide(&w, std::slice::from_ref(&req));
    assert_eq!(brain.stats().applied, 1);
    assert_eq!(brain.decision(id).unwrap().source, Source::Laya);
    assert!(brain.decision(id).unwrap().confidence.unwrap() >= 0.5);
    assert!(!brain.decision(id).unwrap().options.is_empty());

    // Ask again (cache emptied), then the situation changes before it's used.
    brain.reset();
    brain.decide(&w, std::slice::from_ref(&req));
    wait_for(&mut brain, |b| !b.is_pending(id));
    let npc = w.npcs.iter_mut().find(|n| n.id == id).unwrap();
    npc.needs.hunger = if npc.needs.hunger > 0.5 { 0.05 } else { 0.95 };
    let changed = request(&mut w, id);
    brain.decide(&w, &[changed]);
    assert_eq!(brain.stats().rejected_stale, 1);
    assert_ne!(brain.decision(id).unwrap().source, Source::Laya);
}

#[test]
fn low_confidence_answers_fall_back() {
    let mut w = world(4);
    let id = npc_with_options(&mut w, 3);
    let config = LayaConfig {
        min_confidence: 0.6,
        ..LayaConfig::default()
    };
    let mut brain = sync_brain(MockModel::new().with_temperature(1000.0), config);
    brain.set_focus(all_carriages(&w));
    let req = request(&mut w, id);
    let fallback = UtilityBrain::new(1).decide(&w, std::slice::from_ref(&req));
    let out = brain.decide(&w, std::slice::from_ref(&req));
    assert_eq!(out, fallback);
    let s = brain.stats();
    assert_eq!(
        (s.jobs_answered, s.rejected_low_confidence, s.applied),
        (1, 1, 0)
    );
    assert_eq!(brain.decision(id).unwrap().source, Source::Utility);
    assert_eq!(brain.cache_len(), 0);
}

#[test]
fn answers_map_back_through_the_top_k() {
    let mut w = world(5);
    let id = npc_with_options(&mut w, 4);
    for k in [2, 3, 5] {
        let config = LayaConfig {
            top_k: k,
            ..LayaConfig::default()
        };
        let mut brain = sync_brain(PickLast, config);
        brain.set_focus(all_carriages(&w));
        let req = request(&mut w, id);
        let scores = UtilityBrain::new(1).scores(&w, &req);
        let top = top_k(&scores, k);
        assert_eq!(top.len(), k.min(req.options.len()));
        let out = brain.decide(&w, std::slice::from_ref(&req));
        // The model picked the last (worst) of the top-k: the original index is top[k-1].
        assert_eq!(out[0], *top.last().unwrap());
        let info = brain.decision(id).unwrap();
        assert_eq!(info.source, Source::Laya);
        assert_eq!(info.options.len(), top.len());
    }
    // The prefilter never passes more than 5 options.
    let mut brain = sync_brain(
        PickLast,
        LayaConfig {
            top_k: 50,
            ..LayaConfig::default()
        },
    );
    brain.set_focus(all_carriages(&w));
    let req = request(&mut w, id);
    brain.decide(&w, std::slice::from_ref(&req));
    assert!(brain.decision(id).unwrap().options.len() <= 5);
}

#[test]
fn equivalent_situations_hit_the_cache() {
    let mut w = world(6);
    let id = npc_with_options(&mut w, 3);
    let model = MockModel::new().with_temperature(0.001);
    let rows = model.rows_counter();
    let mut brain = sync_brain(
        model,
        LayaConfig {
            min_confidence: 0.0,
            ..LayaConfig::default()
        },
    );
    brain.set_focus(all_carriages(&w));
    let req = request(&mut w, id);
    let first = brain.decide(&w, std::slice::from_ref(&req));
    assert_eq!(brain.decision(id).unwrap().source, Source::Laya);
    let second = brain.decide(&w, std::slice::from_ref(&req));
    // The same kind of option (two chats with strangers are equivalent).
    let kind = |i: usize| req.options[i].action.kind();
    assert_eq!(kind(first[0]), kind(second[0]));
    assert_eq!(brain.decision(id).unwrap().source, Source::Cache);
    assert_eq!(brain.stats().cache_hits, 1);
    assert_eq!(rows.load(Ordering::Relaxed), 1, "asked the model twice");
    // No cache: asked again.
    let mut brain = sync_brain(
        PickLast,
        LayaConfig {
            cache_capacity: 0,
            ..LayaConfig::default()
        },
    );
    brain.set_focus(all_carriages(&w));
    brain.decide(&w, std::slice::from_ref(&req));
    brain.decide(&w, std::slice::from_ref(&req));
    assert_eq!(brain.stats().cache_hits, 0);
    assert_eq!(brain.stats().jobs_sent, 2);
}

fn run_sync(seed: u64, days: u64) -> (String, Vec<sim_laya::LogEntry>, sim_laya::LayaStats) {
    let mut w = world(seed);
    let config = LayaConfig {
        record_log: true,
        ..LayaConfig::default()
    };
    let mut brain = LayaBrain::with_sync_model(UtilityBrain::new(seed), MockModel::new(), config);
    brain.set_focus([CarriageId(0), CarriageId(1), CarriageId(2)]);
    w.run(&mut Checked(&mut brain), days * MINUTES_PER_DAY);
    let stats = brain.stats().clone();
    (format!("{:?}", w.npcs), brain.take_log(), stats)
}

#[test]
fn sync_mode_is_deterministic() {
    let (a, log_a, stats) = run_sync(7, 1);
    let (b, log_b, _) = run_sync(7, 1);
    assert_eq!(a, b);
    assert_eq!(log_a, log_b);
    assert!(stats.count(Source::Laya) > 0);
    assert!(stats.count(Source::Cache) > 0);
    // Never "thinks" in sync mode: the answer is always there.
    assert_eq!(stats.count(Source::Think), 0);
}

#[test]
fn async_brain_runs_for_days_and_replays_exactly() {
    let seed = 8;
    let mut w = world(seed);
    let start = w.clone();
    let model = MockModel::new().with_latency(Duration::from_millis(2), Duration::ZERO);
    let config = LayaConfig {
        record_log: true,
        budget_per_call: 6,
        ..LayaConfig::default()
    };
    let mut brain = LayaBrain::with_model(UtilityBrain::new(seed), model, config);
    brain.set_focus([CarriageId(1), CarriageId(4)]);
    wait_for(&mut brain, |b| b.is_active());
    for _ in 0..3 * 24 {
        w.run(&mut Checked(&mut brain), 60);
        // Give the worker a moment, as the game's frames would.
        std::thread::sleep(Duration::from_millis(1));
    }
    let s = brain.stats().clone();
    assert!(s.jobs_sent > 0 && s.jobs_answered > 0, "{s:?}");
    assert!(s.count(Source::Laya) + s.count(Source::Cache) > 0, "{s:?}");
    assert_eq!(s.jobs_failed, 0);
    for npc in &w.npcs {
        for v in [npc.needs.hunger, npc.needs.energy, npc.needs.social] {
            assert!((0.0..=1.0).contains(&v));
        }
    }

    // Replaying the log reproduces the run exactly.
    let log = brain.take_log();
    let mut replay = ReplayBrain::new(&log, brain.config().think_minutes)
        .with_deliberations(brain.take_deliberation_log());
    let mut again = start;
    again.run(&mut replay, 3 * 24 * 60);
    assert_eq!(replay.missing, 0);
    assert_eq!(format!("{:?}", again.npcs), format!("{:?}", w.npcs));
}

#[test]
fn focus_npcs_think_while_waiting_then_give_up() {
    let mut w = world(9);
    let id = npc_with_options(&mut w, 3);
    let model = MockModel::new().with_latency(Duration::from_secs(3), Duration::ZERO);
    let config = LayaConfig {
        think_minutes: 5,
        max_think_minutes: 10,
        ..LayaConfig::default()
    };
    let mut brain = LayaBrain::with_model(UtilityBrain::new(9), model, config);
    brain.set_focus([w.npc(id).unwrap().carriage]);
    wait_for(&mut brain, |b| b.is_active());
    assert_eq!(brain.think_minutes(), 5);
    let req = request(&mut w, id);
    assert_eq!(brain.decide(&w, std::slice::from_ref(&req)), vec![THINK]);
    assert_eq!(brain.decision(id).unwrap().source, Source::Think);
    // Still waiting 5 minutes later...
    w.clock = w.clock + 5;
    let req = request(&mut w, id);
    assert_eq!(brain.decide(&w, std::slice::from_ref(&req)), vec![THINK]);
    // ...but not after 10: acts on the fallback.
    w.clock = w.clock + 5;
    let req = request(&mut w, id);
    let out = brain.decide(&w, std::slice::from_ref(&req));
    assert_ne!(out[0], THINK);
    assert_eq!(brain.decision(id).unwrap().source, Source::Utility);
    // Without focus nobody thinks.
    brain.set_focus([]);
    brain.reset();
    let out = brain.decide(&w, std::slice::from_ref(&req));
    assert_ne!(out[0], THINK);
}

#[test]
fn disabled_or_missing_model_is_exactly_the_fallback() {
    let reference = {
        let mut w = world(10);
        w.run(&mut UtilityBrain::new(10), MINUTES_PER_DAY);
        format!("{:?}", w.npcs)
    };
    let mut w = world(10);
    let mut brain = sync_brain(
        MockModel::new(),
        LayaConfig {
            enabled: false,
            ..LayaConfig::default()
        },
    );
    *brain.fallback_mut() = UtilityBrain::new(10);
    brain.set_focus(all_carriages(&w));
    w.run(&mut brain, MINUTES_PER_DAY);
    assert_eq!(format!("{:?}", w.npcs), reference);
    assert_eq!(brain.stats().jobs_sent, 0);

    let mut w = world(10);
    let mut brain = LayaBrain::new(UtilityBrain::new(10), LayaConfig::default());
    assert_eq!(brain.status(), &ModelStatus::Missing);
    w.run(&mut brain, MINUTES_PER_DAY);
    assert_eq!(format!("{:?}", w.npcs), reference);
}

#[test]
fn a_failing_loader_is_reported_and_harmless() {
    let mut w = world(11);
    let mut brain = LayaBrain::new(UtilityBrain::new(11), LayaConfig::default());
    brain.attach_loader(Box::new(|| Err("pesi non trovati".to_string())));
    assert_eq!(brain.status(), &ModelStatus::Loading);
    wait_for(&mut brain, |b| !matches!(b.status(), ModelStatus::Loading));
    assert_eq!(
        brain.status(),
        &ModelStatus::Failed("pesi non trovati".into())
    );
    w.run(&mut Checked(&mut brain), 120);
    assert_eq!(brain.stats().jobs_sent, 0);
}

#[test]
fn replacing_the_fallback_drops_old_answers() {
    let mut w = world(12);
    let id = npc_with_options(&mut w, 3);
    let mut brain = LayaBrain::with_model(
        UtilityBrain::new(12),
        MockModel::new().with_latency(Duration::from_millis(50), Duration::ZERO),
        LayaConfig::default(),
    );
    brain.set_focus(all_carriages(&w));
    wait_for(&mut brain, |b| b.is_active());
    let req = request(&mut w, id);
    brain.decide(&w, std::slice::from_ref(&req));
    assert!(brain.is_pending(id));
    brain.replace_fallback(UtilityBrain::new(99));
    assert!(!brain.is_pending(id));
    std::thread::sleep(Duration::from_millis(120));
    brain.poll();
    // The old answer arrived but belongs to the previous world.
    assert_eq!(brain.stats().jobs_answered, 0);
    assert_eq!(brain.cache_len(), 0);
    assert!(brain.is_active());
}

#[test]
fn the_brain_can_live_in_a_bevy_resource() {
    fn send_sync<T: Send + Sync + 'static>() {}
    send_sync::<LayaBrain<UtilityBrain>>();
}
