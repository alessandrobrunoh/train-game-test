//! Parametri di bilanciamento della simulazione.
//!
//! All rates are per game minute unless stated otherwise. Needs are in `0..=1`
//! (1 = fully satisfied). Stored inside [`crate::World`] so they are saved with
//! the world and can be tweaked at runtime.

use serde::{Deserialize, Serialize};

use crate::carriage::CarriageKind;
use crate::item::ItemKind;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
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
    /// Travel time per carriage crossed (5 min: at 1x speed an NPC walks a
    /// carriage in 5 real seconds instead of sprinting through it).
    pub travel_minutes_per_carriage: u64,
    /// Night sleep ends at this hour plus up to `wake_jitter_minutes`.
    pub wake_hour: u32,
    pub wake_jitter_minutes: u64,
    /// Hours in `[night_start_hour, 24) ∪ [0, wake_hour)` count as night.
    pub night_start_hour: u32,
    /// Sleep started from this hour on (or at night) lasts until morning;
    /// earlier it is a nap.
    pub long_sleep_from_hour: u32,

    // --- Production chains ---
    /// Verdura grown by a Contadino per minute of work (×`tool_output_bonus` with a tool).
    pub verdura_per_farm_minute: f32,
    /// Razioni a Cuoco can cook per minute of work, using Verdura from the Serre.
    pub razioni_per_cook_minute: f32,
    /// Razioni cooked from one Verdura.
    pub razioni_per_verdura: f32,
    /// Razioni consumed by one meal (meals are free).
    pub razioni_per_meal: f32,
    /// Rottame the train sheds per carriage per hour, split evenly among the Officine.
    pub rottame_per_carriage_hour: f32,
    /// Attrezzi an Operaio can make per minute of work (×`tool_output_bonus`).
    pub attrezzi_per_craft_minute: f32,
    /// Vestiti an Operaio can make per minute of work (×`tool_output_bonus`).
    pub vestiti_per_craft_minute: f32,
    pub rottame_per_attrezzo: f32,
    pub rottame_per_vestito: f32,
    /// Finished goods a Mercante can bring from the Officine per minute of work.
    pub goods_per_trade_minute: f32,

    // --- Durable goods ---
    /// Output multiplier for Contadini and Operai who own an Attrezzo.
    pub tool_output_bonus: f32,
    /// Attrezzo durability lost per minute of work.
    pub tool_wear_per_work_minute: f32,
    /// Vestito durability lost every midnight.
    pub clothes_wear_per_day: f32,
    /// Energy decay multiplier while owning a Vestito (warmer, less tired).
    pub clothes_energy_factor: f32,

    // --- Storage (per carriage and per item) ---
    pub verdura_storage_cap: f32,
    pub razioni_storage_cap: f32,
    pub rottame_storage_cap: f32,
    /// Attrezzi / Vestiti an Officina can hold (each).
    pub workshop_goods_cap: f32,
    /// Attrezzi / Vestiti a Mercato can hold (each).
    pub market_goods_cap: f32,
    /// Fraction of stored Verdura that spoils every midnight.
    pub verdura_spoilage_per_day: f32,
    /// Fraction of stored Razioni that spoils every midnight.
    pub razioni_spoilage_per_day: f32,

    // --- Money ---
    /// Tokens paid per full hour of work.
    pub wage_per_hour: u32,
    /// Tokens given every midnight to NPCs without a job (children, elderly,
    /// unemployed), so they can afford clothes too.
    pub stipend_per_day: u32,
    /// Mercato price = base value × (1 + markup × scarcity), where scarcity
    /// is `1 - stock / market_goods_cap` in that Mercato.
    pub scarcity_markup: f32,
    /// Duration of a purchase.
    pub buy_minutes: u64,

    // --- Death ---
    /// An NPC with hunger at 0 for this long dies.
    pub starvation_minutes: u64,

    // --- Life cycle (checked once per game day, at midnight) ---
    /// Game days in a year of life. Small so generations are watchable: with
    /// 12, a year lasts ~29 real seconds at 600 game minutes per second.
    pub days_per_year: u32,
    /// Gompertz mortality: yearly death hazard `base * e^(growth * age)`.
    /// The defaults put the median age at death around 78.
    pub mortality_base: f32,
    pub mortality_growth: f32,
    /// Affinity gained by both NPCs when a chat ends...
    pub affinity_per_chat: f32,
    /// ...unless they quarrel (this chance), losing as much instead.
    pub quarrel_chance: f32,
    /// Friend ties fade towards 0 by this much every day (family ties don't).
    pub affinity_decay_per_day: f32,
    /// Two single adults of opposite sex become a couple once their affinity
    /// reaches this...
    pub couple_affinity: f32,
    /// ...if their ages differ by at most this many years.
    pub couple_max_age_gap: u32,
    /// Whether couples may also form between two NPCs of the same sex (they
    /// don't have children).
    pub same_sex_couples: bool,
    /// Yearly chance that a couple with a fertile woman considers a child. If
    /// the administration allows it, she deliberates ("provarci ora" /
    /// "aspettare", see [`crate::DeliberationKind::HaveChild`]); otherwise the
    /// birth is denied.
    pub birth_chance_per_year: f32,
    /// Fertile ages of the mother, inclusive.
    pub fertile_min_age: u32,
    pub fertile_max_age: u32,
    /// Minimum years between two children of the same mother.
    pub birth_spacing_years: u32,
    /// The administration allows births only while the population is below
    /// this fraction of all beds ([`crate::World::max_population`])...
    pub birth_max_bed_occupancy: f32,
    /// ...and the Mense hold at least this many Razioni per person.
    pub birth_min_razioni_per_person: f32,
    /// At most one `BirthDenied` event every this many days.
    pub birth_denied_log_days: u64,
    /// Generation: spare beds in each Dormitorio, as a fraction of its residents.
    pub spare_beds: f32,

    // --- Deliberations (see [`crate::deliberation`]) ---
    /// Scales how often temptations (theft) and protests are considered;
    /// 0 disables them. Couple proposals and children follow the life cycle.
    pub deliberation_rate: f32,
    /// Game hours a brain has to answer, per kind; then the built-in rule decides.
    pub couple_deliberation_hours: u64,
    pub child_deliberation_hours: u64,
    pub theft_deliberation_hours: u64,
    pub protest_deliberation_hours: u64,
    /// Couple: after "chiede tempo" the proposal comes back after this many days...
    pub proposal_retry_days: u64,
    /// ...after a refusal not before this many days, and the affinity drops by this.
    pub proposal_refused_days: u64,
    pub proposal_refused_affinity: f32,
    /// Theft: hourly chance, while the Mercati are open, that an NPC (14+)
    /// who needs an Attrezzo or Vestito it cannot afford, at most
    /// `theft_max_distance` carriages from a Mercato that has one, is tempted.
    pub theft_temptation_per_hour: f32,
    pub theft_max_distance: u32,
    /// No new temptation for the same NPC for this many days.
    pub theft_cooldown_days: u64,
    /// Chance of being caught: this, plus `theft_caught_merchant` if a
    /// Mercante is at the counter, plus `theft_caught_per_witness` per other
    /// person in the Mercato (capped at 0.9 overall).
    pub theft_caught_base: f32,
    pub theft_caught_merchant: f32,
    pub theft_caught_per_witness: f32,
    /// Affinity a caught thief loses with everyone who knows them.
    pub theft_caught_affinity: f32,
    /// Protest: chance that each partner of a couple refused a child
    /// deliberates whether to protest...
    pub protest_chance_on_denial: f32,
    /// ...and that a hungry adult does when the Mense run out of Razioni (at
    /// most `protest_max_on_shortage` people each time).
    pub protest_chance_on_shortage: f32,
    pub protest_max_on_shortage: u32,
    /// No new protest deliberation for the same NPC for this many days.
    pub protest_cooldown_days: u64,
    /// How long a protest gathering lasts.
    pub protest_hours: u64,
    /// The administration gives in once `protest_threshold` people protested
    /// the same grievance within `protest_window_days`: against denied births
    /// it allows `protest_birth_bonus` more of the beds to be filled
    /// (on top of `birth_max_bed_occupancy`) for `protest_concession_days`;
    /// against hunger it hands out emergency rations.
    pub protest_threshold: u32,
    pub protest_window_days: u64,
    pub protest_birth_bonus: f32,
    pub protest_concession_days: u64,
    /// Resolved deliberations kept in [`crate::World::recent_deliberations`].
    pub recent_deliberations_kept: usize,

    // --- Event log ---
    /// Most events kept in [`crate::World::events`]: beyond it the oldest are
    /// dropped, a chunk at a time (see [`crate::World::events_total`]).
    #[serde(default = "default_max_events")]
    pub max_events: usize,
}

fn default_max_events() -> usize {
    2000
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
            travel_minutes_per_carriage: 5,
            wake_hour: 6,
            wake_jitter_minutes: 60,
            night_start_hour: 22,
            long_sleep_from_hour: 20,

            verdura_per_farm_minute: 0.030,
            razioni_per_cook_minute: 0.10,
            razioni_per_verdura: 1.0,
            razioni_per_meal: 1.0,
            rottame_per_carriage_hour: 0.3,
            attrezzi_per_craft_minute: 1.0 / 90.0,
            vestiti_per_craft_minute: 1.0 / 60.0,
            rottame_per_attrezzo: 2.0,
            rottame_per_vestito: 1.0,
            goods_per_trade_minute: 0.05,

            tool_output_bonus: 1.5,
            tool_wear_per_work_minute: 1.0 / (5.0 * 420.0),
            clothes_wear_per_day: 0.125,
            clothes_energy_factor: 0.85,

            verdura_storage_cap: 400.0,
            razioni_storage_cap: 400.0,
            rottame_storage_cap: 150.0,
            workshop_goods_cap: 40.0,
            market_goods_cap: 30.0,
            verdura_spoilage_per_day: 0.08,
            razioni_spoilage_per_day: 0.05,

            wage_per_hour: 1,
            stipend_per_day: 1,
            scarcity_markup: 0.5,
            buy_minutes: 15,

            starvation_minutes: 3 * 24 * 60,

            days_per_year: 12,
            mortality_base: 3.0e-5,
            mortality_growth: 0.1,
            affinity_per_chat: 0.05,
            quarrel_chance: 0.1,
            affinity_decay_per_day: 0.01,
            couple_affinity: 0.5,
            couple_max_age_gap: 12,
            same_sex_couples: false,
            birth_chance_per_year: 0.45,
            fertile_min_age: 18,
            fertile_max_age: 42,
            birth_spacing_years: 2,
            birth_max_bed_occupancy: 0.97,
            birth_min_razioni_per_person: 1.0,
            birth_denied_log_days: 3,
            spare_beds: 0.15,

            deliberation_rate: 1.0,
            couple_deliberation_hours: 6,
            child_deliberation_hours: 6,
            theft_deliberation_hours: 2,
            protest_deliberation_hours: 3,
            proposal_retry_days: 3,
            proposal_refused_days: 12,
            proposal_refused_affinity: 0.3,
            theft_temptation_per_hour: 0.1,
            theft_max_distance: 1,
            theft_cooldown_days: 6,
            theft_caught_base: 0.1,
            theft_caught_merchant: 0.25,
            theft_caught_per_witness: 0.03,
            theft_caught_affinity: 0.15,
            protest_chance_on_denial: 0.5,
            protest_chance_on_shortage: 0.3,
            protest_max_on_shortage: 8,
            protest_cooldown_days: 12,
            protest_hours: 3,
            protest_threshold: 3,
            protest_window_days: 12,
            protest_birth_bonus: 0.02,
            protest_concession_days: 24,
            recent_deliberations_kept: 64,

            max_events: default_max_events(),
        }
    }
}

impl SimParams {
    /// Game minutes in a year of life.
    pub fn minutes_per_year(&self) -> u64 {
        u64::from(self.days_per_year.max(1)) * crate::time::MINUTES_PER_DAY
    }

    /// Yearly death hazard (old age) at `age` years (Gompertz curve).
    pub fn mortality_per_year(&self, age: f32) -> f32 {
        self.mortality_base * (self.mortality_growth * age).exp()
    }

    /// Probability of surviving from birth to `age` years under
    /// [`SimParams::mortality_per_year`].
    pub fn survival(&self, age: f32) -> f32 {
        let b = self.mortality_growth;
        if b <= 0.0 {
            return (-self.mortality_base * age).exp();
        }
        (-(self.mortality_base / b) * ((b * age).exp() - 1.0)).exp()
    }

    /// Game minutes a brain has to answer a deliberation of `kind`.
    pub fn deliberation_minutes(&self, kind: &crate::DeliberationKind) -> u64 {
        use crate::DeliberationKind as K;
        let hours = match kind {
            K::CoupleProposal { .. } => self.couple_deliberation_hours,
            K::HaveChild { .. } => self.child_deliberation_hours,
            K::Theft { .. } => self.theft_deliberation_hours,
            K::Protest { .. } => self.protest_deliberation_hours,
        };
        hours * crate::time::MINUTES_PER_HOUR
    }

    pub fn is_night(&self, hour: u32) -> bool {
        hour >= self.night_start_hour || hour < self.wake_hour
    }

    /// Whether a sleep started at `hour` lasts until morning.
    pub fn is_long_sleep(&self, hour: u32) -> bool {
        hour >= self.long_sleep_from_hour || hour < self.wake_hour
    }

    /// How much of `item` a carriage of `kind` can store (0 = not stored there).
    pub fn storage_cap(&self, kind: CarriageKind, item: ItemKind) -> f32 {
        use CarriageKind as C;
        use ItemKind as I;
        match (kind, item) {
            (C::Serra, I::Verdura) => self.verdura_storage_cap,
            (C::Mensa, I::Razione) => self.razioni_storage_cap,
            (C::Officina, I::Rottame) => self.rottame_storage_cap,
            (C::Officina, I::Attrezzo | I::Vestito) => self.workshop_goods_cap,
            (C::Mercato, I::Attrezzo | I::Vestito) => self.market_goods_cap,
            _ => 0.0,
        }
    }

    /// Fraction of `item` stock that spoils every midnight.
    pub fn spoilage_per_day(&self, item: ItemKind) -> f32 {
        match item {
            ItemKind::Verdura => self.verdura_spoilage_per_day,
            ItemKind::Razione => self.razioni_spoilage_per_day,
            _ => 0.0,
        }
    }
}
