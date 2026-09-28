//! Parametri di bilanciamento della simulazione.
//!
//! All rates are per game minute unless stated otherwise. Needs are in `0..=1`
//! (1 = fully satisfied). Stored inside [`crate::World`] so they are saved with
//! the world and can be tweaked at runtime.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SimParams {
    // --- Needs decay (always applied) ---
    /// Hunger decay while awake (asleep it is multiplied by `sleep_hunger_factor`).
    pub hunger_decay: f32,
    pub sleep_hunger_factor: f32,
    /// Energy decay while awake.
    pub energy_decay: f32,
    /// Social decay while awake.
    pub social_decay: f32,

    // --- Needs recovery while an action runs ---
    /// Total hunger restored by one meal (spread over `eat_minutes`).
    pub meal_restore: f32,
    pub sleep_energy_gain: f32,
    pub socialize_gain: f32,
    /// Social boost given to the partner when a conversation ends.
    pub socialize_partner_bonus: f32,
    /// Eating together at a Mensa is a bit social too.
    pub eat_social_gain: f32,

    // --- Durations (minutes) ---
    pub eat_minutes: u64,
    pub work_block_minutes: u64,
    pub nap_minutes: u64,
    pub socialize_min: u64,
    pub socialize_max: u64,
    pub idle_min: u64,
    pub idle_max: u64,
    /// Travel time per carriage crossed.
    pub travel_minutes_per_carriage: u64,
    /// Night sleep ends at this hour plus up to `wake_jitter_minutes`.
    pub wake_hour: u32,
    pub wake_jitter_minutes: u64,
    /// Hours in `[night_start_hour, 24) ∪ [0, wake_hour)` count as night.
    pub night_start_hour: u32,
    /// Sleep started from this hour on (or at night) lasts until morning;
    /// earlier it is a nap.
    pub long_sleep_from_hour: u32,

    // --- Economy ---
    /// Food produced by a Contadino per minute of work.
    pub food_per_farm_minute: f32,
    /// Food a Cuoco can move from the Serre into its Mensa per minute of work.
    pub food_per_cook_minute: f32,
    /// Materials produced by an Operaio per minute of work.
    pub materials_per_work_minute: f32,
    /// Food consumed by one meal.
    pub food_per_meal: f32,
    /// Max food a single carriage can store.
    pub food_storage_cap: f32,
    pub materials_storage_cap: f32,
    /// Fraction of stored food that spoils every midnight.
    pub food_spoilage_per_day: f32,
    /// Tokens paid per full hour of work.
    pub wage_per_hour: u32,

    // --- Death ---
    /// An NPC with hunger at 0 for this long dies.
    pub starvation_minutes: u64,
}

impl Default for SimParams {
    fn default() -> Self {
        Self {
            hunger_decay: 0.0014,
            sleep_hunger_factor: 0.5,
            energy_decay: 0.0008,
            social_decay: 0.001,

            meal_restore: 0.6,
            sleep_energy_gain: 0.0022,
            socialize_gain: 0.006,
            socialize_partner_bonus: 0.15,
            eat_social_gain: 0.002,

            eat_minutes: 30,
            work_block_minutes: 120,
            nap_minutes: 90,
            socialize_min: 20,
            socialize_max: 50,
            idle_min: 10,
            idle_max: 30,
            travel_minutes_per_carriage: 2,
            wake_hour: 6,
            wake_jitter_minutes: 60,
            night_start_hour: 22,
            long_sleep_from_hour: 20,

            food_per_farm_minute: 0.030,
            food_per_cook_minute: 0.10,
            materials_per_work_minute: 0.02,
            food_per_meal: 1.0,
            food_storage_cap: 400.0,
            materials_storage_cap: 1000.0,
            food_spoilage_per_day: 0.05,
            wage_per_hour: 1,

            starvation_minutes: 3 * 24 * 60,
        }
    }
}

impl SimParams {
    pub fn is_night(&self, hour: u32) -> bool {
        hour >= self.night_start_hour || hour < self.wake_hour
    }

    /// Whether a sleep started at `hour` lasts until morning.
    pub fn is_long_sleep(&self, hour: u32) -> bool {
        hour >= self.long_sleep_from_hour || hour < self.wake_hour
    }
}
