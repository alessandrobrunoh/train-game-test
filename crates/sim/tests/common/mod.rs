//! Shared test helpers.

#![allow(dead_code)]

use sim::{Brain, Choice, DecisionRequest, Deliberation, DeliberationAnswer, UtilityBrain, World};

/// A brain that lives like [`UtilityBrain`] and answers deliberations in the
/// same tick they open, with `policy` (None: no answer, the rule decides at
/// the deadline). It remembers every deliberation it was shown.
pub struct Scripted<F> {
    pub inner: UtilityBrain,
    pub policy: F,
    pub seen: Vec<Deliberation>,
    pending: Vec<DeliberationAnswer>,
}

impl<F: FnMut(&World, &Deliberation) -> Option<Choice>> Scripted<F> {
    pub fn new(seed: u64, policy: F) -> Self {
        Self {
            inner: UtilityBrain::new(seed),
            policy,
            seen: Vec::new(),
            pending: Vec::new(),
        }
    }
}

/// Answers `choice` to every deliberation that offers it.
pub fn always(
    seed: u64,
    choice: Choice,
) -> Scripted<impl FnMut(&World, &Deliberation) -> Option<Choice>> {
    Scripted::new(seed, move |_: &World, _: &Deliberation| Some(choice))
}

/// Never answers: the rule decides at the deadline.
pub fn silent(seed: u64) -> Scripted<impl FnMut(&World, &Deliberation) -> Option<Choice>> {
    Scripted::new(seed, |_: &World, _: &Deliberation| None)
}

impl<F: FnMut(&World, &Deliberation) -> Option<Choice>> Brain for Scripted<F> {
    fn decide(&mut self, world: &World, requests: &[DecisionRequest]) -> Vec<usize> {
        self.inner.decide(world, requests)
    }

    fn wants_descriptions(&self) -> bool {
        false
    }

    fn answers_deliberations(&self) -> bool {
        true
    }

    fn deliberations_opened(&mut self, world: &World, new: &[Deliberation]) {
        for d in new {
            self.seen.push(d.clone());
            if let Some(choice) = (self.policy)(world, d)
                && let Some(k) = d.option_of(choice)
            {
                self.pending.push(DeliberationAnswer {
                    id: d.id,
                    choice: k,
                    confidence: 0.75,
                });
            }
        }
    }

    fn deliberations_resolved(&mut self, _world: &World) -> Vec<DeliberationAnswer> {
        std::mem::take(&mut self.pending)
    }
}

/// Most people seen at once in one Mensa (see [`World::mensa_occupancy`]).
#[derive(Clone, Copy, Debug, Default)]
pub struct MensaPeak {
    pub carriage: Option<sim::CarriageId>,
    pub present: usize,
    pub eating: usize,
    pub waiting: usize,
    pub loitering: usize,
}

/// Runs `world` for `minutes` and returns each Mensa's peaks (head to tail)
/// over the minutes whose time of day is in `window` (minutes of the day).
pub fn mensa_peaks_in(
    world: &mut World,
    brain: &mut dyn Brain,
    minutes: u64,
    window: std::ops::Range<u32>,
) -> Vec<MensaPeak> {
    let mut peaks: Vec<MensaPeak> = Vec::new();
    for _ in 0..minutes {
        world.tick(brain);
        if !window.contains(&world.clock.minute_of_day()) {
            continue;
        }
        let now = world.mensa_occupancy();
        peaks.resize(now.len(), MensaPeak::default());
        for (p, m) in peaks.iter_mut().zip(now) {
            p.carriage = Some(m.carriage);
            p.present = p.present.max(m.present);
            p.eating = p.eating.max(m.eating);
            p.waiting = p.waiting.max(m.waiting);
            p.loitering = p.loitering.max(m.loitering);
        }
    }
    peaks
}

/// [`mensa_peaks_in`] over the whole day.
pub fn mensa_peaks(world: &mut World, brain: &mut dyn Brain, minutes: u64) -> Vec<MensaPeak> {
    mensa_peaks_in(world, brain, minutes, 0..sim::MINUTES_PER_DAY as u32)
}

/// Runs `world` until the next midnight and returns the Mensa peaks between
/// `from` and `to` (minutes of the day).
pub fn mensa_peaks_between(
    world: &mut World,
    brain: &mut dyn Brain,
    from: u32,
    to: u32,
) -> Vec<MensaPeak> {
    let left = sim::MINUTES_PER_DAY - u64::from(world.clock.minute_of_day());
    mensa_peaks_in(world, brain, left, from..to)
}
