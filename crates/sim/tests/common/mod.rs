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
