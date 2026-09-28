//! Livello decisionale, sostituibile.
//!
//! The sim generates only *valid* candidate options for each NPC that needs a
//! decision; a [`Brain`] just picks one per request. All requests of a tick are
//! batched into a single [`Brain::decide`] call, so an expensive brain (e.g. a
//! text encoder scoring `World::npc_context` against each option description)
//! can run one batched inference per tick.

use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;

use crate::action::{Action, ActionKind, ActionOption, DecisionRequest};
use crate::item::ItemKind;
use crate::npc::Npc;
use crate::world::World;

pub trait Brain {
    /// Returns the chosen option index for each request, in the same order.
    /// Out-of-range indices (or a short vector) make the NPC idle briefly.
    fn decide(&mut self, world: &World, requests: &[DecisionRequest]) -> Vec<usize>;

    /// Whether [`ActionOption::description`] must be filled in. Formatting the
    /// descriptions is the most expensive part of a tick, so brains that only
    /// look at the structured fields can opt out (they then get empty strings).
    fn wants_descriptions(&self) -> bool {
        true
    }
}

/// Hand-tuned weights for [`UtilityBrain`].
#[derive(Clone, Debug, PartialEq)]
pub struct UtilityWeights {
    pub eat: f32,
    /// Below this hunger urgency (`1 - hunger`) eating is not considered at meal times...
    pub min_hunger_to_eat: f32,
    /// ...and below this one outside meal times.
    pub min_hunger_to_snack: f32,
    pub meal_time_bonus: f32,
    pub night_sleep_base: f32,
    pub night_sleep: f32,
    pub day_nap: f32,
    /// Daytime naps only above this tiredness (`1 - energy`).
    pub min_tiredness_to_nap: f32,
    pub work: f32,
    pub socialize: f32,
    /// Extra desire to chat with someone, times the affinity with them
    /// (negative affinity: avoided)...
    pub friend_bonus: f32,
    /// ...plus this for family (partner, parents, children, siblings). Both
    /// scale with the social need, like `socialize`.
    pub family_bonus: f32,
    pub evening_social_bonus: f32,
    pub home_bonus: f32,
    /// Desire to buy a missing Attrezzo (for jobs that use one)...
    pub buy_tool: f32,
    /// ...and a missing Vestito. Both are scaled by wealth, from half (just
    /// affordable) to full (`comfortable_savings` times the base price or more).
    pub buy_clothes: f32,
    pub comfortable_savings: f32,
    pub idle: f32,
    /// Score lost per minute of travel.
    pub travel_cost_per_minute: f32,
    /// Uniform noise added to each score (`0..noise`).
    pub noise: f32,
}

impl Default for UtilityWeights {
    fn default() -> Self {
        Self {
            eat: 1.4,
            min_hunger_to_eat: 0.3,
            min_hunger_to_snack: 0.5,
            meal_time_bonus: 0.4,
            night_sleep_base: 0.6,
            night_sleep: 0.8,
            day_nap: 1.2,
            min_tiredness_to_nap: 0.75,
            work: 0.75,
            socialize: 0.9,
            friend_bonus: 0.4,
            family_bonus: 0.2,
            evening_social_bonus: 0.1,
            home_bonus: 0.1,
            buy_tool: 0.8,
            buy_clothes: 0.5,
            comfortable_savings: 3.0,
            idle: 0.15,
            travel_cost_per_minute: 0.01,
            noise: 0.08,
        }
    }
}

/// Fast rule-based brain: scores every option from needs, time of day and job
/// schedule, then picks the best (plus a little noise).
#[derive(Clone, Debug)]
pub struct UtilityBrain {
    pub weights: UtilityWeights,
    rng: ChaCha8Rng,
}

impl UtilityBrain {
    pub fn new(seed: u64) -> Self {
        Self {
            weights: UtilityWeights::default(),
            rng: ChaCha8Rng::seed_from_u64(seed),
        }
    }

    /// Deterministic score of an option (without noise).
    pub fn score(&self, world: &World, npc: &Npc, option: &ActionOption) -> f32 {
        match option.action {
            Action::Travel { to } => {
                let minutes = option.minutes as f32;
                let goal = match option.goal {
                    Some(goal) => self.goal_score(world, npc, goal),
                    None => self.weights.idle,
                };
                let home = if to == npc.home && option.goal == Some(ActionKind::Sleep) {
                    self.weights.home_bonus
                } else {
                    0.0
                };
                goal + home - minutes * self.weights.travel_cost_per_minute
            }
            Action::Buy(item) => self.buy_score(npc, item),
            Action::Socialize(other) => {
                let w = &self.weights;
                let tie = npc.relation(other).map_or(0.0, |r| {
                    let family = if r.kind.is_family() {
                        w.family_bonus
                    } else {
                        0.0
                    };
                    w.friend_bonus * r.affinity + family
                });
                self.goal_score(world, npc, ActionKind::Socialize) + (1.0 - npc.needs.social) * tie
            }
            action => self.goal_score(world, npc, action.kind()),
        }
    }

    /// Need-driven desire to buy `item`, weighted by how many tokens the NPC has.
    fn buy_score(&self, npc: &Npc, item: ItemKind) -> f32 {
        let w = &self.weights;
        if !npc.wants(item) {
            return -1.0;
        }
        let base = match item {
            ItemKind::Attrezzo => w.buy_tool,
            ItemKind::Vestito => w.buy_clothes,
            _ => return -1.0,
        };
        let comfortable = w.comfortable_savings * item.base_value() as f32;
        let wealth = if comfortable > 0.0 {
            (npc.inventory.tokens as f32 / comfortable).min(1.0)
        } else {
            1.0
        };
        base * (0.5 + 0.5 * wealth)
    }

    fn goal_score(&self, world: &World, npc: &Npc, kind: ActionKind) -> f32 {
        let w = &self.weights;
        let now = world.clock;
        let hour = now.hour();
        match kind {
            ActionKind::Eat => {
                let u = 1.0 - npc.needs.hunger;
                let meal_time = matches!(hour, 6 | 7 | 12 | 13 | 19 | 20);
                // Proper meals at meal times; outside them only when really hungry.
                let threshold = if meal_time {
                    w.min_hunger_to_eat
                } else {
                    w.min_hunger_to_snack
                };
                if u < threshold {
                    return -1.0;
                }
                let bonus = if meal_time { w.meal_time_bonus } else { 0.0 };
                w.eat * u * u + bonus
            }
            ActionKind::Sleep => {
                let u = 1.0 - npc.needs.energy;
                if world.params.is_night(hour) {
                    if u < 0.05 {
                        -1.0
                    } else {
                        w.night_sleep_base + w.night_sleep * u
                    }
                } else if u > w.min_tiredness_to_nap {
                    w.day_nap * u.powi(3)
                } else {
                    -1.0
                }
            }
            ActionKind::Work => match npc.job {
                Some(job) if job.in_shift(now) => w.work,
                _ => -1.0,
            },
            ActionKind::Socialize => {
                let u = 1.0 - npc.needs.social;
                let evening = (17..22).contains(&hour);
                w.socialize * u + if evening { w.evening_social_bonus } else { 0.0 }
            }
            // Travelling to shop: the best thing the NPC could buy there.
            ActionKind::Buy => [ItemKind::Attrezzo, ItemKind::Vestito]
                .into_iter()
                .map(|item| self.buy_score(npc, item))
                .fold(-1.0, f32::max),
            ActionKind::Idle | ActionKind::Travel => w.idle,
        }
    }
}

impl Brain for UtilityBrain {
    fn wants_descriptions(&self) -> bool {
        false
    }

    fn decide(&mut self, world: &World, requests: &[DecisionRequest]) -> Vec<usize> {
        requests
            .iter()
            .map(|req| {
                let Some(npc) = world.npc(req.npc) else {
                    return 0;
                };
                let mut best = (0, f32::NEG_INFINITY);
                for (i, option) in req.options.iter().enumerate() {
                    let noise = self.rng.random_range(0.0..self.weights.noise);
                    let score = self.score(world, npc, option) + noise;
                    if score > best.1 {
                        best = (i, score);
                    }
                }
                best.0
            })
            .collect()
    }
}

/// Picks a uniformly random option. Useful as a baseline and for tests.
#[derive(Clone, Debug)]
pub struct RandomBrain {
    rng: ChaCha8Rng,
}

impl RandomBrain {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: ChaCha8Rng::seed_from_u64(seed),
        }
    }
}

impl Brain for RandomBrain {
    fn wants_descriptions(&self) -> bool {
        false
    }

    fn decide(&mut self, _world: &World, requests: &[DecisionRequest]) -> Vec<usize> {
        requests
            .iter()
            .map(|r| self.rng.random_range(0..r.options.len().max(1)))
            .collect()
    }
}
