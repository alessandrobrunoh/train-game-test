//! Il mondo: treno, NPC, orologio e ciclo di simulazione.

use std::fmt;

use rand::seq::{IndexedRandom, SliceRandom};
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::action::{Action, ActionKind, ActionOption, DecisionRequest};
use crate::brain::{Brain, THINK};
use crate::carriage::{Carriage, CarriageKind, Owner, StationKind};
use crate::catalog::Catalog;
use crate::combat::{CombatCounters, Fight, MAX_HEALTH};
use crate::defs::{ItemUse, StationCount};
use crate::deliberation::{
    Deliberation, DeliberationCounters, Gathering, Grievance, ResolvedDeliberation,
};
use crate::dialogue::{Conversation, ConversationCounters};
use crate::event::{DeathCause, Event, EventKind};
use crate::ids::{CarriageId, NpcId, StationId};
use crate::item::{ItemKind, Stock};
use crate::names;
use crate::npc::{Inventory, Job, LifeStage, Needs, Npc, Relation, RelationKind, Sex, Traits};
use crate::params::SimParams;
use crate::personality::Personality;
use crate::player::{Cabin, PlayerCharacter};
use crate::time::GameTime;

mod chat;
mod combat;
mod comfort;
mod conversation;
mod craft;
mod custode;
mod deliberate;
mod economy;
mod health;
mod life;
mod market;
mod mensa;
mod player;
mod stalls;

pub use chat::KNOWN_MARKET_REACH;
pub use comfort::Comfort;
pub use craft::CraftError;
pub use economy::{Economy, EconomyCounters, PerKind, Tally};
pub use life::LifeCounters;
pub use market::{Market, MarketQuote, PriceSample, SellError, TREND_DAYS, Trend, mercato_price};
pub use mensa::{MensaOccupancy, MensaRole};
pub use player::{
    GIFT_AFFINITY, GREET_AFFINITY, GREET_COOLDOWN_MINUTES, GREET_MINUTES, PRICE_PER_AFFINITY,
    PURCHASE_AFFINITY, THEFT_SEEN_AFFINITY,
};
pub use stalls::{
    Buyer, Listing, ListingId, MarketOffer, Offer, OfferSource, STALL_LOG_KEPT, Seller, StallError,
    StallEvent, StallRecord, StallSale, TradeCounters,
};

/// Expected minutes of actual work per worker per day, used to size the
/// workforce at generation time.
const EXPECTED_WORK_MINUTES_PER_DAY: f32 = 420.0;
/// Workforce is sized to produce this multiple of the daily food need.
const FOOD_SAFETY_MARGIN: f32 = 1.3;
const MEALS_PER_DAY: f32 = 3.0;
/// One Mercante per this many inhabitants (at least one per Mercato).
const NPCS_PER_MERCANTE: usize = 60;
/// Jobs with a staffing quota, by priority (food first). Everyone else is an Operaio.
const NEEDED_JOBS: [Job; 3] = [Job::Contadino, Job::Cuoco, Job::Mercante];
/// Generation: chance that a woman of 20+ is in a couple (if a man fits)...
const GENERATED_COUPLE_CHANCE: f64 = 0.7;
/// ...with at most this age difference.
const GENERATED_COUPLE_AGE_GAP: u32 = 6;
/// Generation: acquaintances drawn among housemates per NPC.
const GENERATED_FRIENDS: usize = 2;
/// Generation: starting tokens of workers and of everyone else.
const STARTING_TOKENS_WORKER: std::ops::Range<u32> = 30..120;
const STARTING_TOKENS_JOBLESS: std::ops::Range<u32> = 5..45;

/// Seed of the stalls' own RNG stream (xored with the world seed).
const TRADE_SEED: u64 = 0x57A1_1B0B_7E11;

/// The stalls' RNG stream of a world saved before it existed.
fn default_trade_rng() -> ChaCha8Rng {
    ChaCha8Rng::seed_from_u64(TRADE_SEED)
}

/// Seed of the fights' own RNG stream (xored with the world seed).
const COMBAT_SEED: u64 = 0xF157_C0FF_B10D;

/// The fights' RNG stream of a world saved before it existed.
fn default_combat_rng() -> ChaCha8Rng {
    ChaCha8Rng::seed_from_u64(COMBAT_SEED)
}

/// Repeating carriage pattern, head to tail. 20 carriages give 6 Dormitori,
/// 4 Mense, 4 Serre, 4 Officine and 2 Mercati.
const LAYOUT: [CarriageKind; 10] = [
    CarriageKind::Dormitorio,
    CarriageKind::Mensa,
    CarriageKind::Serra,
    CarriageKind::Officina,
    CarriageKind::Mercato,
    CarriageKind::Dormitorio,
    CarriageKind::Mensa,
    CarriageKind::Serra,
    CarriageKind::Dormitorio,
    CarriageKind::Officina,
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct World {
    pub clock: GameTime,
    /// Items, recipes and jobs of this world (see [`World::catalog`]).
    #[serde(default)]
    catalog: Catalog,
    /// Proposals waiting for their minute, the decisions taken, the derived
    /// statistics (see [`crate::custode`]).
    #[serde(default, with = "crate::custode::json_text")]
    custode: crate::custode::CustodeState,
    pub params: SimParams,
    /// Ordered head to tail; `carriages[i].id == CarriageId(i)`.
    pub carriages: Vec<Carriage>,
    /// Living NPCs, sorted by id.
    pub npcs: Vec<Npc>,
    /// Most recent events, oldest first; at most `params.max_events` (older
    /// ones are dropped, see [`World::events_total`]).
    pub events: Vec<Event>,
    /// Events dropped from the front of `events` so far.
    #[serde(default)]
    events_dropped: u64,
    next_npc_id: u32,
    /// NPCs generated with the world: ids below this (see [`World::is_founder`]).
    #[serde(default)]
    founders: u32,
    /// Births, deaths and other life-cycle counters.
    #[serde(default)]
    pub life: LifeCounters,
    /// When the last `BirthDenied` event was logged (they are rate-limited).
    #[serde(default)]
    last_birth_denied_log: Option<GameTime>,
    /// Edge-trigger for Shortage / Restocked events, indexed by item.
    shortages: Vec<bool>,
    rng: ChaCha8Rng,
    /// Randomness of the stalls and of the NPCs' belongings (see
    /// `stalls.rs`), a stream of its own so that it leaves the rest alone.
    #[serde(default = "default_trade_rng")]
    trade_rng: ChaCha8Rng,
    /// Open deliberations, sorted by id (see [`World::open_deliberations`]).
    #[serde(default)]
    deliberations: Vec<Deliberation>,
    #[serde(default)]
    next_deliberation_id: u64,
    /// Deliberations with a lower id were already passed to the brain.
    #[serde(default)]
    notified_until: u64,
    /// Last resolved deliberations, oldest first.
    #[serde(default)]
    recent_deliberations: Vec<ResolvedDeliberation>,
    /// Deliberation counters since the world was generated.
    #[serde(default)]
    pub deliberation_counters: DeliberationCounters,
    /// Pending proposal / temptation / protest cooldowns.
    #[serde(default)]
    cooldowns: Vec<deliberate::CooldownUntil>,
    /// Protest gatherings not over yet.
    #[serde(default)]
    gatherings: Vec<Gathering>,
    /// Recent protesters (when, about what), for the administration's response.
    #[serde(default)]
    protest_tally: Vec<(GameTime, Grievance)>,
    /// After protests the administration allows more births until then.
    #[serde(default)]
    birth_bonus_until: Option<GameTime>,
    /// Treasury, pay policy and production/money counters.
    #[serde(default)]
    pub economy: Economy,
    /// Specialties of the carriages, their nearest producers and the price
    /// history of the Mercati (see `market.rs`).
    #[serde(default)]
    market: Market,
    /// Conversations in progress, sorted by id (see [`World::conversations`]).
    #[serde(default)]
    conversations: Vec<Conversation>,
    /// Last finished conversations, oldest first.
    #[serde(default)]
    recent_conversations: Vec<Conversation>,
    #[serde(default)]
    next_conversation_id: u64,
    /// Conversation counters since the world was generated.
    #[serde(default)]
    pub conversation_counters: ConversationCounters,
    /// When a quarrel / gossip about a theft was last logged (rate limit).
    #[serde(default)]
    last_chat_log: [Option<GameTime>; 2],
    /// Notable recent facts people talk about (taken from the events as
    /// they are logged, oldest first).
    #[serde(default)]
    news: Vec<conversation::NewsItem>,
    /// Events already looked at for `news` (see `events_total`).
    #[serde(default)]
    news_seen: u64,
    /// The player, a character of the train (see [`crate::player`]).
    #[serde(default)]
    pub player: PlayerCharacter,
    /// Randomness of the fights (see `combat.rs`), a stream of its own:
    /// never drawn with `violence` 0 and no fights.
    #[serde(default = "default_combat_rng")]
    combat_rng: ChaCha8Rng,
    /// Fights going on, and those ended a few minutes ago (see [`World::fights`]).
    #[serde(default)]
    fights: Vec<Fight>,
    /// Fight counters since the world was generated.
    #[serde(default)]
    pub combat: CombatCounters,
    /// Scratch buffer reused every tick (see `presence_index`).
    #[serde(skip)]
    presence: Presence,
    /// Per-carriage queue / lingering / incoming counts, refreshed with
    /// `presence` (see `mensa.rs`).
    #[serde(skip)]
    load: Vec<mensa::CarriageLoad>,
}

impl World {
    /// Generates a train with `n_carriages` (≥ 1) carriages repeating
    /// Dormitorio / Mensa / Serra / Officina / Mercato / Dormitorio / Mensa /
    /// Serra / Dormitorio / Officina, and `n_npcs` random NPCs.
    /// The simulation starts on day 1 at 06:00.
    pub fn generate(seed: u64, n_carriages: usize, n_npcs: usize) -> World {
        Self::generate_with_params(seed, n_carriages, n_npcs, SimParams::default())
    }

    pub fn generate_with_params(
        seed: u64,
        n_carriages: usize,
        n_npcs: usize,
        params: SimParams,
    ) -> World {
        assert!(n_carriages >= 1, "a train needs at least one carriage");
        assert!(n_carriages <= u16::MAX as usize);
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let clock = GameTime::from_dhm(1, 6, 0);
        let catalog = Catalog::builtin();

        // --- Carriages (stations are added once we know who uses them) ---
        let mut per_kind = [0usize; CarriageKind::COUNT];
        let mut carriages: Vec<Carriage> = (0..n_carriages)
            .map(|i| {
                let kind = LAYOUT[i % LAYOUT.len()];
                let n = per_kind[kind.index()];
                per_kind[kind.index()] += 1;
                let name = match kind.def().names.get(n) {
                    Some(name) => (*name).to_string(),
                    None => format!("{} {}", kind.name(), n + 1),
                };
                Carriage {
                    id: CarriageId(i as u16),
                    name,
                    kind,
                    stations: Vec::new(),
                    stock: Stock::default(),
                }
            })
            .collect();
        let of_kind = |kind: CarriageKind| -> Vec<CarriageId> {
            carriages
                .iter()
                .filter(|c| c.kind == kind)
                .map(|c| c.id)
                .collect()
        };
        let dorms = of_kind(CarriageKind::Dormitorio);

        // --- People: a stationary age pyramid, couples, families, homes ---
        let year = params.minutes_per_year() as i64;
        let ages: Vec<u32> = (0..n_npcs).map(|_| sample_age(&params, &mut rng)).collect();
        let sexes: Vec<Sex> = (0..n_npcs)
            .map(|_| {
                if rng.random_bool(0.5) {
                    Sex::Female
                } else {
                    Sex::Male
                }
            })
            .collect();
        let mut surnames: Vec<&str> = (0..n_npcs)
            .map(|_| names::SURNAMES.choose(&mut rng).copied().unwrap_or("Rossi"))
            .collect();
        let adult = |i: usize| ages[i] >= LifeStage::ADULTO_FROM;

        // Couples: single women (in random order) pair with the single man
        // closest in age, within a few years.
        let mut partner_of: Vec<Option<usize>> = vec![None; n_npcs];
        let mut women: Vec<usize> = (0..n_npcs)
            .filter(|&i| sexes[i] == Sex::Female && ages[i] >= 20)
            .collect();
        women.shuffle(&mut rng);
        for &w in &women {
            if !rng.random_bool(GENERATED_COUPLE_CHANCE) {
                continue;
            }
            let man = (0..n_npcs)
                .filter(|&m| {
                    sexes[m] == Sex::Male
                        && ages[m] >= 20
                        && partner_of[m].is_none()
                        && ages[m].abs_diff(ages[w]) <= GENERATED_COUPLE_AGE_GAP
                })
                .min_by_key(|&m| (ages[m].abs_diff(ages[w]), m));
            if let Some(m) = man {
                partner_of[w] = Some(m);
                partner_of[m] = Some(w);
            }
        }
        let couples: Vec<(usize, usize)> = (0..n_npcs)
            .filter_map(|w| match (sexes[w], partner_of[w]) {
                (Sex::Female, Some(m)) => Some((w, m)),
                _ => None,
            })
            .collect();

        // Children: most minors and some younger adults belong to a couple
        // old enough to be their parents; they take the father's surname.
        let mut parents_of: Vec<Option<usize>> = vec![None; n_npcs]; // index into `couples`
        let mut kids = vec![0u32; couples.len()];
        let mut order: Vec<usize> = (0..n_npcs).collect();
        order.shuffle(&mut rng);
        for &k in &order {
            let chance = if adult(k) { 0.35 } else { 0.9 };
            if ages[k] >= 45 || !rng.random_bool(chance) {
                continue;
            }
            let candidates: Vec<usize> = couples
                .iter()
                .enumerate()
                .filter(|&(c, &(w, m))| {
                    let (mother_age, father_age) =
                        (ages[w].checked_sub(ages[k]), ages[m].checked_sub(ages[k]));
                    kids[c] < 4
                        && w != k
                        && m != k
                        && partner_of[k] != Some(w)
                        && partner_of[k] != Some(m)
                        && mother_age.is_some_and(|a| {
                            (params.fertile_min_age..=params.fertile_max_age).contains(&a)
                        })
                        && father_age.is_some_and(|a| a >= LifeStage::ADULTO_FROM)
                })
                .map(|(c, _)| c)
                .collect();
            if let Some(&c) = candidates.choose(&mut rng) {
                parents_of[k] = Some(c);
                kids[c] += 1;
            }
        }
        // Fathers first (oldest first), so surnames pass down generations.
        order.sort_by_key(|&k| (std::cmp::Reverse(ages[k]), k));
        for &k in &order {
            if let Some(c) = parents_of[k] {
                surnames[k] = surnames[couples[c].1];
            }
        }

        // Households: a couple with its minor children, everyone else alone.
        let mut households: Vec<Vec<usize>> = couples
            .iter()
            .enumerate()
            .map(|(c, &(w, m))| {
                let mut h = vec![w, m];
                h.extend((0..n_npcs).filter(|&k| parents_of[k] == Some(c) && !adult(k)));
                h
            })
            .collect();
        for i in 0..n_npcs {
            let with_parents = parents_of[i].is_some() && !adult(i);
            if partner_of[i].is_none() && !with_parents {
                households.push(vec![i]);
            }
        }
        households.shuffle(&mut rng);
        // Largest first, each into the emptiest Dormitorio: balanced dorms.
        households.sort_by_key(|h| std::cmp::Reverse(h.len()));
        let mut home_of = vec![CarriageId(0); n_npcs];
        let mut dorm_residents = vec![0usize; dorms.len()];
        for h in &households {
            let Some(d) = (0..dorms.len()).min_by_key(|&d| (dorm_residents[d], d)) else {
                break;
            };
            dorm_residents[d] += h.len();
            for &i in h {
                home_of[i] = dorms[d];
            }
        }
        // Meal shifts: a household eats together, in the shift of its
        // Dormitorio with the fewest people so far (no randomness used).
        let shifts = usize::from(params.meal_shifts.max(1));
        let mut shift_of = vec![0u8; n_npcs];
        let mut shift_load = vec![vec![0usize; shifts]; n_carriages];
        for h in &households {
            let Some(&first) = h.first() else {
                continue;
            };
            let load = &mut shift_load[home_of[first].index()];
            let s = (0..shifts).min_by_key(|&s| (load[s], s)).unwrap_or(0);
            load[s] += h.len();
            for &i in h {
                shift_of[i] = s as u8;
            }
        }

        // Size the workforce so food production covers everyone with a margin
        // (assuming nobody has a tool), then Mercanti; everyone else is an Operaio
        // (scrap-limited: idle hands keep the train in repair).
        let workers: Vec<usize> = (0..n_npcs)
            .filter(|&i| LifeStage::of_age(ages[i]).works())
            .collect();
        let quotas = job_quotas(&params, &catalog, &carriages, n_npcs);
        let mut jobs: Vec<Option<Job>> = Vec::with_capacity(workers.len());
        for job in NEEDED_JOBS {
            jobs.extend(std::iter::repeat_n(Some(job), quotas[job.index()]));
        }
        jobs.truncate(workers.len());
        let operaio = has_kind(&carriages, Job::Operaio.workplace_kind()).then_some(Job::Operaio);
        jobs.resize(workers.len(), operaio);
        jobs.shuffle(&mut rng);

        let mut job_of = vec![None; n_npcs];
        for (&w, job) in workers.iter().zip(jobs) {
            job_of[w] = job;
        }

        // Workplaces: split each job's workers, ordered by home position, into
        // equal contiguous groups over that job's carriages (head to tail).
        // Loads stay balanced and people tend to work near where they live.
        let mut workplace_of = vec![None; n_npcs];
        for job in catalog.job_kinds() {
            let places = of_kind(job.workplace_kind());
            let mut staff: Vec<usize> = (0..n_npcs).filter(|&i| job_of[i] == Some(job)).collect();
            staff.sort_by_key(|&i| (home_of[i], i));
            for (k, &i) in staff.iter().enumerate() {
                workplace_of[i] = places.get(k * places.len() / staff.len()).copied();
            }
        }

        // Characters from their own streams, so they don't disturb the rest.
        let mut trait_rng = ChaCha8Rng::seed_from_u64(seed ^ 0x7EA1_75C0_FFEE);
        let mut personality_rng = ChaCha8Rng::seed_from_u64(seed ^ 0x9E50_4A11_7A1C);
        let mut npcs = Vec::with_capacity(n_npcs);
        for i in 0..n_npcs {
            let (age, sex) = (ages[i], sexes[i]);
            let first = names::first_names(sex)
                .choose(&mut rng)
                .copied()
                .unwrap_or("Anna");
            let (home, job, workplace) = (home_of[i], job_of[i], workplace_of[i]);
            // Staggered durabilities so things don't all break on the same day.
            let tool = job
                .is_some_and(Job::uses_tool)
                .then(|| rng.random_range(0.1..1.0));
            let clothes = Some(rng.random_range(0.1..1.0));
            let traits = Traits::random(&mut trait_rng);
            let personality = Personality::random(traits, &mut personality_rng);
            let tokens = if job.is_some() {
                rng.random_range(STARTING_TOKENS_WORKER)
            } else {
                rng.random_range(STARTING_TOKENS_JOBLESS)
            };
            // Born before day 1: some time into the year of their current age.
            let born = clock.0 as i64 - i64::from(age) * year - rng.random_range(1..year);
            npcs.push(Npc {
                id: NpcId(i as u32),
                name: format!("{first} {}", surnames[i]),
                sex,
                born,
                age,
                carriage: home,
                floor: 0,
                home,
                job,
                workplace,
                needs: Needs {
                    hunger: rng.random_range(0.4..0.8),
                    energy: rng.random_range(0.8..1.0),
                    social: rng.random_range(0.5..1.0),
                },
                inventory: Inventory::new(tokens, tool, clothes),
                action: Action::Idle,
                action_since: clock,
                action_until: clock + rng.random_range(0..30),
                starving_minutes: 0,
                relations: Vec::new(),
                traits,
                meal_shift: Some(shift_of[i]),
                personality: Some(personality),
                player: None,
                health: MAX_HEALTH,
                injury: 0.0,
                grudges: Vec::new(),
                violence: 0.0,
                last_attacker: None,
            });
        }

        // --- Relations: partners, parents and children, siblings, and a
        // couple of acquaintances among housemates ---
        for &(w, m) in &couples {
            let affinity = rng.random_range(0.6..0.95);
            link(&mut npcs, w, m, RelationKind::Partner, affinity);
        }
        let mut children_of: Vec<Vec<usize>> = vec![Vec::new(); couples.len()];
        for (k, parents) in parents_of.iter().enumerate() {
            if let &Some(c) = parents {
                let (w, m) = couples[c];
                link(
                    &mut npcs,
                    w,
                    k,
                    RelationKind::Child,
                    rng.random_range(0.5..0.9),
                );
                link(
                    &mut npcs,
                    m,
                    k,
                    RelationKind::Child,
                    rng.random_range(0.5..0.9),
                );
                for &j in &children_of[c] {
                    link(
                        &mut npcs,
                        j,
                        k,
                        RelationKind::Sibling,
                        rng.random_range(0.3..0.8),
                    );
                }
                children_of[c].push(k);
            }
        }
        for (d, &dorm) in dorms.iter().enumerate() {
            let housemates: Vec<usize> = (0..n_npcs).filter(|&i| home_of[i] == dorm).collect();
            if housemates.len() < 2 || dorm_residents[d] < 2 {
                continue;
            }
            for &i in &housemates {
                for _ in 0..GENERATED_FRIENDS {
                    let Some(&j) = housemates.choose(&mut rng) else {
                        break;
                    };
                    let id = npcs[j].id;
                    if j != i && npcs[i].relation(id).is_none() {
                        link(
                            &mut npcs,
                            i,
                            j,
                            RelationKind::Friend,
                            rng.random_range(0.05..0.35),
                        );
                    }
                }
            }
        }

        // --- Stations and starting stock, sized on who lives/works where.
        // Beds leave some room for newborns; kitchens and grow beds are sized
        // for a train full to its birth limit ---
        let beds_of = |residents: usize| {
            (residents as f32 * (1.0 + params.spare_beds.max(0.0))).ceil() as usize + 1
        };
        let total_beds: usize = dorm_residents.iter().map(|&r| beds_of(r)).sum();
        let peak_population = max_population_for(&params, total_beds).max(n_npcs);
        let peak = job_quotas(&params, &catalog, &carriages, peak_population);
        let per_place: Vec<usize> = catalog
            .job_kinds()
            .map(|job| {
                let places = carriages
                    .iter()
                    .filter(|c| c.kind == job.workplace_kind())
                    .count()
                    .max(1);
                peak[job.index()].div_ceil(places)
            })
            .collect();
        let mut kind_count = [0usize; CarriageKind::COUNT];
        for c in &carriages {
            kind_count[c.kind.index()] += 1;
        }
        for c in carriages.iter_mut() {
            let id = c.id;
            let def = c.kind.def();
            for rule in def.stations {
                let n = match rule.count {
                    StationCount::Beds => {
                        let d = dorms.iter().position(|&d| d == id).unwrap_or(0);
                        beds_of(dorm_residents.get(d).copied().unwrap_or(0))
                    }
                    StationCount::Seats => {
                        let seats = (peak_population as f32 * params.mensa_seats_per_person
                            / kind_count[c.kind.index()] as f32)
                            .ceil() as usize;
                        seats.div_ceil(usize::from(rule.capacity)).max(1)
                    }
                    StationCount::Workers { job, peak } => {
                        let here = npcs
                            .iter()
                            .filter(|n| n.job == Some(job) && n.workplace == Some(id))
                            .count();
                        let workers = if peak {
                            here.max(per_place[job.index()])
                        } else {
                            here
                        };
                        workers.div_ceil(usize::from(rule.capacity)) + 1
                    }
                };
                let floors = if rule.spread { c.floors() } else { 1 };
                for f in 0..floors {
                    // Ground floor first; it takes the remainder.
                    let share = n / usize::from(floors)
                        + usize::from(usize::from(f) < n % usize::from(floors));
                    c.push_stations(rule.kind, share, rule.capacity, f);
                }
            }
            for &(item, amount) in def.start_stock {
                c.stock.set(item, amount);
            }
            // Storage declared with the items (see `defs::Store`).
            for def in catalog.items() {
                for store in def.stores.iter().filter(|s| s.carriage == c.kind) {
                    c.stock.set(def.kind, store.start);
                }
            }
        }

        // The player's cabin: a private bed upstairs in the first Dormitorio.
        let home = dorms.first().map(|&d| {
            let c = &mut carriages[d.index()];
            let floor = c.floors() - 1;
            let bed = c.push_private_station(StationKind::Bed, floor, Owner::Player);
            Cabin {
                carriage: d,
                bed,
                floor,
            }
        });
        let player = PlayerCharacter {
            place: home.map(|h| h.place()).unwrap_or_default(),
            home,
            ..PlayerCharacter::default()
        };

        let economy = Economy::start(&params, &npcs);
        let specialties = market::assign_specialties(seed, &catalog, &carriages, &params);
        let market = Market::new(&carriages, specialties, &catalog);
        let mut world = World {
            clock,
            catalog,
            custode: Default::default(),
            params,
            carriages,
            npcs,
            events: Vec::new(),
            events_dropped: 0,
            next_npc_id: n_npcs as u32,
            founders: n_npcs as u32,
            life: LifeCounters::default(),
            last_birth_denied_log: None,
            shortages: vec![false; ItemKind::BUILTIN_COUNT],
            rng,
            trade_rng: ChaCha8Rng::seed_from_u64(seed ^ TRADE_SEED),
            deliberations: Vec::new(),
            next_deliberation_id: 0,
            notified_until: 0,
            recent_deliberations: Vec::new(),
            deliberation_counters: DeliberationCounters::default(),
            cooldowns: Vec::new(),
            gatherings: Vec::new(),
            protest_tally: Vec::new(),
            birth_bonus_until: None,
            economy,
            market,
            conversations: Vec::new(),
            recent_conversations: Vec::new(),
            next_conversation_id: 0,
            conversation_counters: ConversationCounters::default(),
            last_chat_log: [None; 2],
            news: Vec::new(),
            news_seen: 0,
            player,
            combat_rng: ChaCha8Rng::seed_from_u64(seed ^ COMBAT_SEED),
            fights: Vec::new(),
            combat: CombatCounters::default(),
            presence: Presence::default(),
            load: Vec::new(),
        };
        world.record_prices();
        world
    }

    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    /// Items, recipes and jobs of this world: the builtin ones and what the
    /// Custode added (see [`crate::custode`]).
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Most of `item` a carriage of `kind` stores (see [`Catalog::storage_cap`]).
    pub fn storage_cap(&self, kind: CarriageKind, item: ItemKind) -> f32 {
        self.catalog.storage_cap(&self.params, kind, item)
    }

    pub fn npc(&self, id: NpcId) -> Option<&Npc> {
        self.npc_index(id).map(|i| &self.npcs[i])
    }

    fn npc_index(&self, id: NpcId) -> Option<usize> {
        index_of(&self.npcs, id)
    }

    pub fn carriage(&self, id: CarriageId) -> Option<&Carriage> {
        self.carriages.get(id.index())
    }

    /// Whether NPCs `a` and `b` are in the same place, close enough to talk:
    /// today the same carriage, neither of them travelling. Every "can
    /// these two talk?" check of the conversations goes through here.
    pub fn same_place(&self, a: &Npc, b: &Npc) -> bool {
        let travelling = |n: &Npc| matches!(n.action, Action::Travel { .. });
        a.carriage == b.carriage && a.floor == b.floor && !travelling(a) && !travelling(b)
    }

    /// NPCs currently in a carriage (travellers count as being in their origin).
    pub fn npcs_in(&self, carriage: CarriageId) -> impl Iterator<Item = &Npc> {
        self.npcs.iter().filter(move |n| n.carriage == carriage)
    }

    /// Id that the next spawned NPC would get.
    pub fn next_npc_id(&self) -> NpcId {
        NpcId(self.next_npc_id)
    }

    pub fn travel_minutes(&self, from: CarriageId, to: CarriageId) -> u64 {
        u64::from(from.distance(to)) * self.params.travel_minutes_per_carriage
    }

    /// Minutes `npc` takes to walk to `to`: the carriages to cross, plus the
    /// stairs down if it is on an upper floor; slower when it is hurt
    /// ([`SimParams::hurt_walk_factor`]).
    pub fn trip_minutes(&self, npc: &Npc, to: CarriageId) -> u64 {
        let stairs = if npc.floor > 0 {
            self.params.stairs_minutes
        } else {
            0
        };
        let minutes = self.travel_minutes(npc.carriage, to) + stairs;
        if npc.health < self.params.hurt_below {
            (minutes as f32 * self.params.hurt_walk_factor).round() as u64
        } else {
            minutes
        }
    }

    /// e.g. `Mensa «Il Refettorio» (carrozza 2)`.
    pub fn carriage_label(&self, id: CarriageId) -> String {
        match self.carriage(id) {
            Some(c) => c.label().to_string(),
            None => format!("carrozza {id}"),
        }
    }

    /// Short natural-language description of an NPC's situation, meant as
    /// context for text-based brains.
    pub fn npc_context(&self, id: NpcId) -> Option<String> {
        let npc = self.npc(id)?;
        let level = |v: f32| match v {
            v if v < 0.2 => "critica",
            v if v < 0.45 => "bassa",
            v if v < 0.75 => "media",
            _ => "buona",
        };
        let job = match (npc.job, npc.workplace) {
            (Some(job), Some(place)) => {
                let (start, end) = job.shift();
                let state = if self.works_now(npc) {
                    "in corso"
                } else {
                    "finito o non iniziato"
                };
                format!(
                    "Lavora come {job} in {}, turno {start:02}:00-{end:02}:00 ({state}).",
                    self.carriage_label(place)
                )
            }
            _ => "Non ha un lavoro.".to_string(),
        };
        let job = format!("{job} {}", self.meal_context(npc));
        let moment = if self.params.is_night(self.clock.hour()) {
            "notte"
        } else {
            "giorno"
        };
        let wear = |d: f32| match d {
            d if d < 0.2 => "quasi da buttare",
            d if d < 0.5 => "consumato",
            _ => "in buono stato",
        };
        let inv = &npc.inventory;
        let tool = match inv.tool {
            Some(d) => format!("un attrezzo {} ({:.0}%)", wear(d), d * 100.0),
            None if npc.wants(ItemKind::Attrezzo) => "nessun attrezzo (gli servirebbe)".to_string(),
            None => "nessun attrezzo".to_string(),
        };
        let clothes = match inv.clothes {
            Some(d) => format!("un vestito {} ({:.0}%)", wear(d), d * 100.0),
            None => "nessun vestito caldo".to_string(),
        };
        let clothes = match inv.items.items().as_slice() {
            [] => clothes,
            carried => {
                let list: Vec<String> = carried
                    .iter()
                    .map(|&(item, n)| match n {
                        1 => item.with_article().to_string(),
                        n => format!("{n} {}", item.plural()),
                    })
                    .collect();
                format!("{clothes}; porta con sé {}", list.join(", "))
            }
        };
        let mut family = self.family_context(npc);
        family.push_str(&self.health_context(npc));
        Some(format!(
            "{} ({moment}). {}, {} anni ({}), si trova in {}{}. Casa: {}. {job} {family}\
             Possiede {} gettoni, {tool} e {clothes}. \
             Sazietà {} ({:.2}), energia {} ({:.2}), socialità {} ({:.2}).",
            self.clock,
            npc.name,
            npc.age,
            npc.stage().name(npc.sex),
            self.carriage_label(npc.carriage),
            upstairs(npc.floor),
            self.carriage_label(npc.home),
            inv.tokens,
            level(npc.needs.hunger),
            npc.needs.hunger,
            level(npc.needs.energy),
            npc.needs.energy,
            level(npc.needs.social),
            npc.needs.social,
        ))
    }

    /// Health, wounds, grudges and reputation, for [`World::npc_context`]
    /// (empty for a healthy, peaceful NPC without grudges).
    fn health_context(&self, npc: &Npc) -> String {
        let mut text = String::new();
        if npc.is_hurt() {
            text.push_str(&format!(
                "Salute {:.0}/100 ({}{}). ",
                npc.health,
                npc.condition(&self.params).label(npc.sex),
                if npc.injury >= 1.0 {
                    format!(", ferite {:.0}", npc.injury)
                } else {
                    String::new()
                }
            ));
        }
        if let Some(g) = npc
            .grudges
            .iter()
            .max_by(|a, b| a.strength.total_cmp(&b.strength))
        {
            text.push_str(&format!("Ce l'ha con {}. ", self.fighter_name(g.against)));
        }
        let reputation = npc.reputation();
        if reputation != crate::combat::Reputation::Peaceful {
            text.push_str(&format!("Ha fama di {}. ", reputation.label(npc.sex)));
        }
        text
    }

    /// Amount of `item` where people can get it ([`ItemKind::outlet`]):
    /// Razioni in the Mense, Attrezzi/Vestiti in the Mercati, ...
    pub fn available(&self, item: ItemKind) -> f32 {
        self.stock_in(item.outlet()).get(item)
    }

    /// Sum of the storage of all carriages of `kind`.
    pub fn stock_in(&self, kind: CarriageKind) -> Stock {
        let mut total = Stock::default();
        for c in self.carriages.iter().filter(|c| c.kind == kind) {
            total.merge(&c.stock);
        }
        total
    }

    /// Sum of all carriage storage (personal inventories excluded).
    pub fn total_stock(&self) -> Stock {
        let mut total = Stock::default();
        for c in &self.carriages {
            total.merge(&c.stock);
        }
        total
    }

    /// Current price of one `item` at `carriage`, if it is a Mercato selling it
    /// (whether or not it is in stock): base value plus transport from the
    /// nearest producer, times the pay level, plus the scarcity markup (see
    /// `market.rs`).
    pub fn price(&self, carriage: CarriageId, item: ItemKind) -> Option<u32> {
        price_at(
            &self.params,
            &self.catalog,
            self.economy.pay_level,
            &self.market,
            self.carriage(carriage)?,
            item,
        )
    }

    /// Events logged since the world was generated, including those already
    /// dropped from [`World::events`]. The last `n` new events since a
    /// previous total `t` are `events[events.len() - (total - t)..]`, as long
    /// as fewer than `max_events` were logged in between.
    pub fn events_total(&self) -> u64 {
        self.events_dropped + self.events.len() as u64
    }

    /// A Mercante working at a counter of `carriage` right now, if any.
    pub fn merchant_on_duty(&self, carriage: CarriageId) -> Option<&Npc> {
        let c = self.carriage(carriage)?;
        let is_counter =
            |s: StationId| c.station(s).is_some_and(|s| s.kind == StationKind::Counter);
        // Cheap early exit: nobody at any counter.
        if !c
            .stations
            .iter()
            .any(|s| s.kind == StationKind::Counter && s.occupancy > 0)
        {
            return None;
        }
        self.npcs_in(carriage).find(|n| {
            n.job == Some(Job::Mercante) && matches!(n.action, Action::Work(s) if is_counter(s))
        })
    }

    fn log(&mut self, kind: EventKind) {
        self.events.push(Event {
            time: self.clock,
            kind,
        });
        self.trim_events();
    }

    /// Keeps at most `max_events` events. Drops a quarter more than needed so
    /// the (memmove) drain runs rarely.
    fn trim_events(&mut self) {
        let max = self.params.max_events;
        if self.events.len() > max {
            // Notable facts are kept for conversations before events go.
            self.collect_news();
            let keep = max - max / 4;
            let dropped = self.events.len() - keep;
            self.events.drain(..dropped);
            self.events_dropped += dropped as u64;
        }
    }

    // ------------------------------------------------------------------
    // Simulation
    // ------------------------------------------------------------------

    /// Advances the simulation by `ticks` minutes.
    pub fn run(&mut self, brain: &mut dyn Brain, ticks: u64) {
        for _ in 0..ticks {
            self.tick(brain);
        }
    }

    /// Advances the simulation by one minute:
    /// 1. finishes actions that end now (effects on completion);
    /// 2. collects one [`DecisionRequest`] per idle NPC and asks the brain once
    ///    (a [`THINK`] answer makes the NPC idle for [`Brain::think_minutes`]);
    /// 3. starts the chosen actions (re-validated: if a station filled up or an
    ///    item sold out in the meantime the NPC idles briefly and decides again);
    /// 4. updates needs, starvation and deaths;
    /// 5. hourly/daily bookkeeping (Rottame income, spoilage, clothes wear,
    ///    payday: wages, stipends and taxes, see [`Economy`]; shortage events) and, at midnight, the life cycle (aging,
    ///    deaths, couples, births, workforce: see [`World::life`]); hourly,
    ///    temptations to steal;
    /// 6. deliberations: new ones go to the brain, its answers are applied,
    ///    the others are resolved by the built-in rule at their deadline (at
    ///    once if [`Brain::answers_deliberations`] is false).
    pub fn tick(&mut self, brain: &mut dyn Brain) {
        let now = self.clock;
        // Proposals whose minute came, statistics on the hour (see `custode.rs`).
        self.custode_tick();

        // Conversations that are over end for both (before their actions do).
        self.end_conversations();
        let mut deciding = Vec::new();
        for i in 0..self.npcs.len() {
            if self.npcs[i].action_until <= now {
                self.finish_action(i);
                deciding.push(i);
            }
        }

        // Tables freed by who finished eating go to who is queuing first.
        self.serve_queues();

        if !deciding.is_empty() {
            let presence = self.presence_index();
            let mut requests: Vec<DecisionRequest> = deciding
                .iter()
                .map(|&i| DecisionRequest {
                    npc: self.npcs[i].id,
                    options: self.options_for(i, &presence),
                })
                .collect();
            if brain.wants_descriptions() {
                for req in &mut requests {
                    for o in &mut req.options {
                        o.description = self.describe_option(req.npc, o);
                    }
                }
            }
            let choices = brain.decide(self, &requests);
            let think = ActionOption {
                action: Action::Idle,
                minutes: brain.think_minutes().max(1),
                goal: None,
                description: String::new(),
            };
            for (k, (&i, req)) in deciding.iter().zip(requests).enumerate() {
                // Pulled into someone's conversation earlier in this loop.
                if self.npcs[i].action_until > now {
                    continue;
                }
                let option = match choices.get(k) {
                    Some(&THINK) => Some(&think),
                    Some(&c) => req.options.get(c),
                    None => None,
                };
                self.start_action(i, option);
            }
            self.presence = presence;
        }

        // Fights: one exchange of blows a minute.
        if !self.fights.is_empty() {
            self.combat_tick();
        }

        self.update_needs();

        if now.minute() == 0 {
            self.shed_rottame();
            if now.minute_of_day() == 0 {
                self.spoil();
                self.wear_clothes();
                self.payday();
                self.daily_life();
                self.record_prices();
                self.furnish_dorms();
                self.combat_midnight();
            }
            self.check_shortages();
            self.end_gatherings();
            self.temptations();
            self.stalls_hour();
            self.violence_hour();
            self.care_for_bedridden();
        }

        self.run_deliberations(brain);
        self.player_tick();
        self.player_recover();
        self.trim_events();
        self.clock = now + 1;
    }

    /// Generates the valid candidate actions for an NPC, with descriptions.
    /// Consumes world randomness (durations, chat partners), like a real tick would.
    pub fn options(&mut self, id: NpcId) -> Vec<ActionOption> {
        let Some(i) = self.npc_index(id) else {
            return Vec::new();
        };
        let presence = self.presence_index();
        let mut options = self.options_for(i, &presence);
        self.presence = presence;
        for o in &mut options {
            o.description = self.describe_option(id, o);
        }
        options
    }

    /// Indices of NPCs available for a chat, per carriage. Reuses the scratch
    /// buffer's allocations (hand it back via `self.presence` when done).
    fn presence_index(&mut self) -> Presence {
        self.refresh_load();
        let mut presence = std::mem::take(&mut self.presence);
        let by_carriage = &mut presence.by_carriage;
        by_carriage.resize_with(self.carriages.len(), Vec::new);
        by_carriage.iter_mut().for_each(Vec::clear);
        presence.spot.clear();
        presence.spot.resize(self.next_npc_id as usize, 0);
        // Who is already in a conversation is not available (marked first).
        const TALKING: u32 = u32::MAX;
        for c in &self.conversations {
            for id in [c.a, c.b] {
                if let Some(spot) = presence.spot.get_mut(id.0 as usize) {
                    *spot = TALKING;
                }
            }
        }
        for (i, npc) in self.npcs.iter().enumerate() {
            let spot = &mut presence.spot[npc.id.0 as usize];
            if *spot == TALKING {
                *spot = 0;
            } else if available_for_chat(npc) {
                by_carriage[npc.carriage.index()].push(i);
                *spot = Presence::spot_of(npc.carriage, npc.floor);
            }
        }
        presence
    }

    /// Candidate actions for NPC at index `i` (descriptions left empty).
    /// Only valid options are generated; `Idle` is always the first one.
    fn options_for(&mut self, i: usize, presence: &Presence) -> Vec<ActionOption> {
        let now = self.clock;
        // Protesters only go to the protest and stand there until it ends.
        if let Some((place, end)) = self.protest_duty(self.npcs[i].id) {
            let here = self.npcs[i].carriage;
            let (action, minutes, goal) = if here != place {
                (
                    Action::Travel { to: place },
                    self.trip_minutes(&self.npcs[i], place).max(1),
                    Some(ActionKind::Idle),
                )
            } else {
                (Action::Idle, end.since(now).max(1), None)
            };
            return vec![ActionOption {
                action,
                minutes,
                goal,
                description: String::new(),
            }];
        }
        // Badly hurt: home to bed (see `health.rs`).
        if self.npcs[i].health < self.params.bedridden_below {
            return self.bedridden_options(i);
        }
        let p = &self.params;
        let npc = &self.npcs[i];
        let here = npc.carriage;
        let carriage = &self.carriages[here.index()];
        let mut options = Vec::with_capacity(8);
        let mut push = |action, minutes, goal| {
            options.push(ActionOption {
                action,
                minutes,
                goal,
                description: String::new(),
            });
        };

        let can_eat_at = |c: &Carriage| {
            c.kind == CarriageKind::Mensa
                && c.stock.has(ItemKind::Razione, p.razioni_per_meal)
                && c.has_free(StationKind::Table)
        };
        let can_sleep_at = |c: &Carriage| c.has_free(StationKind::Bed);
        let (level, market, cat) = (self.economy.pay_level, &self.market, &self.catalog);
        // What the NPC would buy (catalog-driven, see `stalls.rs`); food is
        // only bought where it already is, nobody walks to a stall to eat.
        let wanted: Vec<ItemKind> = cat
            .kinds()
            .filter(|&item| stalls::wants_item(p, cat, level, market, npc, item))
            .collect();
        let wants_something = !wanted.is_empty();
        let can_shop_at = |c: &Carriage| {
            wanted.iter().any(|&item| {
                item.usage() != ItemUse::Food
                    && can_buy_at(p, cat, level, market, now, npc, c, item)
            })
        };

        push(
            Action::Idle,
            self.rng.random_range(p.idle_min..=p.idle_max),
            None,
        );

        if can_eat_at(carriage)
            && let Some(table) = carriage.roomiest_station(StationKind::Table)
        {
            push(Action::Eat(table), p.eat_minutes, None);
        }
        // Every table taken: queue for a seat, if the line isn't too long.
        if carriage.kind == CarriageKind::Mensa
            && carriage.stock.has(ItemKind::Razione, p.razioni_per_meal)
            && !carriage.has_free(StationKind::Table)
            && self.queue_len(here) < usize::from(p.mensa_queue_max)
        {
            push(Action::Wait, p.queue_patience_minutes.max(1), None);
        }

        if let Some(bed) = carriage.free_station_for(StationKind::Bed, npc.id) {
            let minutes = if p.is_long_sleep(now.hour()) {
                // Who has no job to go to wakes up for its breakfast shift.
                let late = match npc.job {
                    None => u64::from(p.meal_shift(npc)) * u64::from(p.meal_shift_minutes),
                    Some(_) => 0,
                };
                let wake = now.next_at(p.wake_hour, 0)
                    + late
                    + self.rng.random_range(0..=p.wake_jitter_minutes);
                wake - now
            } else {
                p.nap_minutes
            };
            push(Action::Sleep(bed), minutes, None);
        }

        let lunch = p.lunch_break(p.meal_shift(npc));
        let working = match (npc.job, npc.workplace) {
            (Some(job), Some(place)) if job.works_at(now, lunch) => Some((job, place)),
            _ => None,
        };
        if let Some((job, place)) = working
            && place == here
            && let Some(station) = carriage.free_station(job.station_kind())
        {
            let minutes = p.work_block_minutes.min(job.minutes_left_at(now, lunch));
            push(Action::Work(station), minutes, None);
        }

        for &item in &wanted {
            if can_buy_at(p, cat, level, market, now, npc, carriage, item) {
                push(Action::Buy(item), p.buy_minutes, None);
            }
        }

        // Chat with up to two family members or friends around (closest ties
        // first), then with up to two other people.
        let mut close: [Option<(f32, NpcId)>; 2] = [None; 2];
        for r in &npc.relations {
            let closeness = r.affinity
                + match r.kind {
                    RelationKind::Partner => 1.0,
                    RelationKind::Friend => 0.0,
                    _ => 0.5,
                };
            if closeness <= 0.0 || !presence.is_available_in(r.other, here, npc.floor) {
                continue;
            }
            // Keep the two closest, closest first.
            let better = |slot: Option<(f32, NpcId)>| slot.is_none_or(|(c, _)| closeness > c);
            if better(close[0]) {
                close[1] = close[0];
                close[0] = Some((closeness, r.other));
            } else if better(close[1]) {
                close[1] = Some((closeness, r.other));
            }
        }
        let close = close.map(|c| c.map(|(_, id)| id));
        for id in close.into_iter().flatten() {
            let minutes = self.rng.random_range(p.socialize_min..=p.socialize_max);
            push(Action::Socialize(id), minutes, None);
        }
        let same_floor: Vec<usize> = presence.by_carriage[here.index()]
            .iter()
            .copied()
            .filter(|&j| self.npcs[j].floor == npc.floor)
            .collect();
        for &j in same_floor
            .sample(&mut self.rng, 3)
            .filter(|&&j| j != i && !close.contains(&Some(self.npcs[j].id)))
            .take(2)
        {
            let minutes = self.rng.random_range(p.socialize_min..=p.socialize_max);
            push(Action::Socialize(self.npcs[j].id), minutes, None);
        }

        // --- Travel, each with the goal it serves ---
        let stairs = if npc.floor > 0 { p.stairs_minutes } else { 0 };
        // Hurt: slower on its feet.
        let slow = (npc.health < p.hurt_below).then_some(p.hurt_walk_factor);
        let travel = |to: CarriageId| {
            let minutes = u64::from(to.distance(here)) * p.travel_minutes_per_carriage + stairs;
            match slow {
                Some(factor) => (minutes as f32 * factor).round() as u64,
                None => minutes,
            }
        };
        let nearest = |pred: &dyn Fn(&Carriage) -> bool| {
            self.carriages
                .iter()
                .filter(|c| c.id != here && pred(c))
                .min_by_key(|c| c.id.distance(here))
                .map(|c| c.id)
        };
        if !can_eat_at(carriage)
            && let Some(to) = self.mensa_for_meal(here)
        {
            push(Action::Travel { to }, travel(to), Some(ActionKind::Eat));
        }
        if here != npc.home && can_sleep_at(&self.carriages[npc.home.index()]) {
            let to = npc.home;
            push(Action::Travel { to }, travel(to), Some(ActionKind::Sleep));
        } else if !can_sleep_at(carriage)
            && let Some(to) = nearest(&can_sleep_at)
        {
            push(Action::Travel { to }, travel(to), Some(ActionKind::Sleep));
        }
        if let Some((job, to)) = working
            && to != here
            && self.carriages[to.index()].has_free(job.station_kind())
        {
            push(Action::Travel { to }, travel(to), Some(ActionKind::Work));
        }
        if wants_something
            && !can_shop_at(carriage)
            && let Some(to) = nearest(&can_shop_at)
        {
            push(Action::Travel { to }, travel(to), Some(ActionKind::Buy));
        }
        // Goods to sell and a reason to (see `World::wants_to_sell`): to the
        // nearest Mercato's stalls, a low-priority errand.
        if carriage.kind != CarriageKind::Mercato
            && self.wants_to_sell(npc)
            && let Some(to) = self.nearest_market(here)
        {
            push(Action::Travel { to }, travel(to), Some(ActionKind::Buy));
        }
        // A crowded Mensa: go home and chat there instead.
        if here != npc.home && self.is_crowded(here) {
            let to = npc.home;
            push(
                Action::Travel { to },
                travel(to),
                Some(ActionKind::Socialize),
            );
        }
        // Visit the liveliest other carriage.
        if let Some(to) = presence
            .by_carriage
            .iter()
            .enumerate()
            .filter(|&(c, people)| {
                c != here.index() && !people.is_empty() && !self.is_crowded(CarriageId(c as u16))
            })
            .max_by_key(|&(c, people)| (people.len(), std::cmp::Reverse(c.abs_diff(here.index()))))
            .map(|(c, _)| CarriageId(c as u16))
        {
            push(
                Action::Travel { to },
                travel(to),
                Some(ActionKind::Socialize),
            );
        }

        options
    }

    /// Short natural-language description of an option for NPC `npc`, e.g.
    /// "mangia in Mensa «Il Refettorio» (carrozza 2)".
    pub fn describe_option(&self, npc: NpcId, option: &ActionOption) -> String {
        let Some(npc) = self.npc(npc) else {
            return String::new();
        };
        let here = &self.carriages[npc.carriage.index()];
        let end = self.clock + option.minutes;
        let minutes = option.minutes;
        if let Some(g) = self.protest_of(npc.id) {
            match option.action {
                Action::Travel { to } => {
                    return format!(
                        "va a protestare in {} ({minutes} min)",
                        self.carriage_label(to)
                    );
                }
                Action::Idle => {
                    return format!(
                        "protesta {} in {} fino alle {:02}:{:02}",
                        g.grievance.against(),
                        here.label(),
                        end.hour(),
                        end.minute()
                    );
                }
                _ => {}
            }
        }
        match option.action {
            Action::Idle if self.is_crowded(npc.carriage) => {
                format!("resta a oziare per {minutes} minuti nella mensa affollata")
            }
            Action::Idle => format!("resta a oziare per {minutes} minuti"),
            Action::Wait => format!(
                "aspetta un posto in {} per mangiare ({} in coda)",
                here.label(),
                self.queue_len(npc.carriage)
            ),
            Action::Eat(_) => format!("mangia una razione in {}", here.label()),
            Action::Sleep(bed) if self.params.is_long_sleep(self.clock.hour()) => format!(
                "va a dormire in {}{} fino alle {:02}:{:02}",
                here.label(),
                upstairs(here.floor_of(bed)),
                end.hour(),
                end.minute()
            ),
            Action::Sleep(_) => format!("fa un pisolino di {minutes} minuti"),
            Action::Work(station) => {
                let job = npc.job.map_or("lavoratore", |j| j.name());
                let spot = here
                    .station(station)
                    .map_or("postazione", |s| s.kind.name());
                format!(
                    "lavora come {job} ({spot} {}) fino alle {:02}:{:02}",
                    station.0 + 1,
                    end.hour(),
                    end.minute()
                )
            }
            Action::Buy(item) => {
                let level = self.economy.pay_level;
                let offer = stalls::best_offer(
                    &self.params,
                    &self.catalog,
                    level,
                    &self.market,
                    here,
                    item,
                    npc.id,
                );
                let price = offer.map_or(0, |(price, _)| price);
                let from = match offer {
                    Some((_, stalls::Source::Listing(k))) => {
                        let seller = self.market.listings[k].seller;
                        format!(" al banco di {}", self.seller_name(seller))
                    }
                    _ => String::new(),
                };
                format!(
                    "compra {}{from} in {} per {price} gettoni (ne ha {})",
                    item.with_article(),
                    here.label(),
                    npc.inventory.tokens
                )
            }
            Action::Socialize(other) => self.describe_chat(npc, other, minutes),
            Action::Attack(target) => {
                format!("aggredisce {} ({minutes} min)", self.fighter_name(target))
            }
            Action::Travel { to } => {
                let dest = self.carriage(to);
                let shopping = dest.and_then(|c| {
                    self.catalog.kinds().find(|&item| {
                        let level = self.economy.pay_level;
                        let (p, cat) = (&self.params, &self.catalog);
                        item.usage() != ItemUse::Food
                            && stalls::wants_item(p, cat, level, &self.market, npc, item)
                            && can_buy_at(p, cat, level, &self.market, self.clock, npc, c, item)
                    })
                });
                let selling = dest.is_some_and(|c| c.kind == CarriageKind::Mercato)
                    && self.wants_to_sell(npc);
                let why = match option.goal {
                    Some(ActionKind::Eat) => " per mangiare".to_string(),
                    Some(ActionKind::Sleep) if to == npc.home => {
                        " per tornare a casa a dormire".to_string()
                    }
                    Some(ActionKind::Sleep) => " per dormire".to_string(),
                    Some(ActionKind::Work) => " per lavorare".to_string(),
                    Some(ActionKind::Socialize) => " per fare due chiacchiere".to_string(),
                    Some(ActionKind::Buy) => match shopping {
                        Some(item) => format!(" per comprare {}", item.with_article()),
                        None if selling => " per vendere qualcosa ai banchi".to_string(),
                        None => " per fare acquisti".to_string(),
                    },
                    _ => String::new(),
                };
                let dest = dest.map(|c| c.label().to_string()).unwrap_or_default();
                format!("va in {dest} ({minutes} min){why}")
            }
        }
    }

    fn finish_action(&mut self, i: usize) {
        let npc = &self.npcs[i];
        let (action, here, job) = (npc.action, npc.carriage, npc.job);
        let has_tool = npc.inventory.tool.is_some();
        let hurt = npc.health < self.params.hurt_below;
        let minutes = npc.action_until.since(npc.action_since);
        if let Some(station) = action.station() {
            self.release(here, station);
        }
        match action {
            Action::Work(_) => {
                self.earn_wage(i, minutes);
                if let Some(job) = job {
                    let bonus = if job.uses_tool() && has_tool {
                        self.params.tool_output_bonus
                    } else {
                        1.0
                    };
                    // Hurt: less work gets done.
                    let effort = if hurt {
                        minutes as f32 * self.params.hurt_work_factor
                    } else {
                        minutes as f32
                    };
                    if let Some((item, made)) = self.produce(job, here, effort, bonus) {
                        self.keep_own_share(i, here, item, made);
                    }
                    if job.uses_tool() {
                        let wear = minutes as f32 * self.params.tool_wear_per_work_minute;
                        self.wear_item(i, ItemKind::Attrezzo, wear);
                    }
                }
            }
            Action::Travel { to } => {
                // Through the gangways, which are on the ground floor.
                self.npcs[i].carriage = to;
                self.npcs[i].floor = 0;
            }
            // A one-sided chat (conversations end in `end_conversations`):
            // the busy partner still gets a bonus and the tie changes.
            Action::Socialize(partner) => {
                let bonus = self.params.socialize_partner_bonus;
                if let Some(j) = self.npc_index(partner)
                    && self.same_place(&self.npcs[i], &self.npcs[j])
                {
                    let needs = &mut self.npcs[j].needs;
                    needs.social = (needs.social + bonus).min(1.0);
                    let gain = self.params.affinity_per_chat;
                    let delta = if self.rng.random::<f32>() < self.params.quarrel_chance {
                        -gain
                    } else {
                        gain
                    };
                    self.add_affinity(i, j, delta);
                }
            }
            Action::Eat(_)
            | Action::Sleep(_)
            | Action::Buy(_)
            | Action::Idle
            | Action::Wait
            | Action::Attack(_) => {}
        }
        self.npcs[i].action = Action::Idle;
        // Stalls and home (see `stalls.rs`).
        self.after_action(i);
    }

    /// Lowers the durability of NPC `i`'s `item`; removes it (with an event)
    /// when it reaches 0.
    fn wear_item(&mut self, i: usize, item: ItemKind, amount: f32) {
        let npc = &mut self.npcs[i];
        let Some(slot) = npc.inventory.slot_mut(item) else {
            return;
        };
        let Some(durability) = slot else {
            return;
        };
        *durability -= amount;
        if *durability <= 0.0 {
            *slot = None;
            self.events.push(Event {
                time: self.clock,
                kind: EventKind::ItemBroke {
                    npc: npc.id,
                    name: npc.name.clone(),
                    item,
                },
            });
            // A spare one carried along goes on at once.
            self.equip_spares(i);
        }
    }

    fn release(&mut self, carriage: CarriageId, station: StationId) {
        if let Some(s) = self.carriages[carriage.index()]
            .stations
            .get_mut(station.index())
        {
            s.occupancy = s.occupancy.saturating_sub(1);
        }
    }

    /// Starts `option` for NPC `i`, re-validating it against the current state.
    fn start_action(&mut self, i: usize, option: Option<&ActionOption>) {
        let now = self.clock;
        let here = self.npcs[i].carriage;
        let chosen = option.filter(|o| self.still_valid(i, &o.action));
        let (action, mut minutes) = match chosen {
            Some(o) => (o.action, o.minutes.max(1)),
            None => (Action::Idle, self.params.idle_min.max(1)),
        };
        if let Action::Socialize(partner) = action {
            minutes = self.start_chat(i, partner, minutes).max(1);
        }
        if let Some(station) = action.station() {
            let station = &mut self.carriages[here.index()].stations[station.index()];
            station.occupancy += 1;
            self.npcs[i].floor = station.floor;
        }
        if action == Action::Wait {
            // The Mensa queues are on the ground floor.
            self.npcs[i].floor = 0;
        }
        match action {
            Action::Eat(_) => {
                self.carriages[here.index()]
                    .stock
                    .take(ItemKind::Razione, self.params.razioni_per_meal);
                self.serve_tea(i);
            }
            Action::Wait => self.joined_queue(here),
            Action::Buy(item) => self.npc_buy(i, item),
            _ => {}
        }
        let npc = &mut self.npcs[i];
        npc.action = action;
        npc.action_since = now;
        npc.action_until = now + minutes;
    }

    /// NPC `i` pays for one `item` from the shelf of its (Mercato) carriage
    /// and owns a new one. The tokens go to the treasury.
    fn buy_from_shelf(&mut self, i: usize, item: ItemKind) {
        let here = self.npcs[i].carriage;
        let price = self.price(here, item).unwrap_or(0);
        self.carriages[here.index()].stock.take(item, 1.0);
        let npc = &mut self.npcs[i];
        // Validated before: the NPC can pay.
        let price = price.min(npc.inventory.tokens);
        npc.inventory.tokens -= price;
        match npc.inventory.slot_mut(item) {
            Some(slot) if slot.is_none() => *slot = Some(1.0),
            // Something else the shelf may deal in one day: carried along.
            _ => {
                if npc.inventory.items.add(item, 1) == 1 {
                    self.market.trade.into_pool += 1;
                }
            }
        }
        self.economy.treasury += u64::from(price);
        self.economy.counters.purchases += u64::from(price);
        let npc = &self.npcs[i];
        self.events.push(Event {
            time: self.clock,
            kind: EventKind::ItemBought {
                npc: npc.id,
                name: npc.name.clone(),
                item,
                price,
                carriage: here,
            },
        });
    }

    fn still_valid(&self, i: usize, action: &Action) -> bool {
        let npc = &self.npcs[i];
        let here = npc.carriage;
        let carriage = &self.carriages[here.index()];
        let station_free = |s: StationId| carriage.station(s).is_some_and(|s| s.has_room());
        match *action {
            Action::Eat(s) => {
                station_free(s)
                    && carriage
                        .stock
                        .has(ItemKind::Razione, self.params.razioni_per_meal)
            }
            Action::Sleep(s) | Action::Work(s) => station_free(s),
            Action::Buy(item) => {
                let level = self.economy.pay_level;
                can_buy_at(
                    &self.params,
                    &self.catalog,
                    level,
                    &self.market,
                    self.clock,
                    npc,
                    carriage,
                    item,
                )
            }
            Action::Travel { to } => to != here && to.index() < self.carriages.len(),
            Action::Socialize(partner) => self.npc_index(partner).is_some(),
            Action::Attack(target) => self.fighter_health(target).is_some(),
            Action::Idle => true,
            Action::Wait => {
                carriage.kind == CarriageKind::Mensa
                    && carriage
                        .stock
                        .has(ItemKind::Razione, self.params.razioni_per_meal)
                    && self.queue_len(here) < usize::from(self.params.mensa_queue_max)
            }
        }
    }

    fn update_needs(&mut self) {
        // Coperte, Lampade and Giocattoli of the Dormitori (see `comfort.rs`).
        let comfort = self.comfort_levels();
        let p = &self.params;
        let now = self.clock;
        let drain = health::starvation_drain(p);
        let mut dead = Vec::new();
        for (i, npc) in self.npcs.iter_mut().enumerate() {
            let (sleep_gain, social_decay) = comfort::need_factors(p, &comfort, npc);
            let needs = &mut npc.needs;
            match npc.action {
                Action::Sleep(_) => {
                    needs.hunger -= p.hunger_decay * p.sleep_hunger_factor;
                    needs.energy += p.sleep_energy_gain * sleep_gain;
                }
                action => {
                    let warm = if npc.inventory.clothes.is_some() {
                        p.clothes_energy_factor
                    } else {
                        1.0
                    };
                    needs.hunger -= p.hunger_decay;
                    needs.energy -= p.energy_decay * warm;
                    needs.social -= p.social_decay * social_decay;
                    match action {
                        Action::Eat(_) => {
                            needs.hunger += p.meal_restore / p.eat_minutes as f32;
                            needs.social += p.eat_social_gain;
                        }
                        Action::Socialize(_) => needs.social += p.socialize_gain,
                        _ => {}
                    }
                }
            }
            needs.clamp();

            if needs.hunger <= 0.0 {
                npc.starving_minutes += 1;
                if npc.starving_minutes == 1 {
                    self.events.push(Event {
                        time: now,
                        kind: EventKind::NpcStarving {
                            npc: npc.id,
                            name: npc.name.clone(),
                        },
                    });
                }
                // Past the grace period hunger eats away at health.
                if npc.starving_minutes > p.starvation_grace_minutes {
                    npc.health = (npc.health - drain).max(0.0);
                    if npc.health <= 0.0 {
                        dead.push((i, DeathCause::Starvation));
                    }
                }
            } else {
                npc.starving_minutes = 0;
                if npc.health < MAX_HEALTH
                    && let health::Health::BledOut = health::recover(p, npc)
                {
                    dead.push((i, DeathCause::Wounds));
                }
            }
        }
        for &(i, cause) in dead.iter().rev() {
            match (cause, self.npcs[i].last_attacker) {
                (DeathCause::Wounds, Some(killer)) => {
                    let place = self.npcs[i].carriage;
                    self.combat.died_of_wounds += 1;
                    self.slain(i, killer, place, cause);
                }
                _ => self.kill(i, cause),
            }
        }
    }

    /// Hourly: the train sheds Rottame, collected evenly into the Officine.
    fn shed_rottame(&mut self) {
        let p = &self.params;
        let officine = self
            .carriages
            .iter()
            .filter(|c| c.kind == CarriageKind::Officina)
            .count();
        if officine == 0 {
            return;
        }
        let each = p.rottame_per_carriage_hour * self.carriages.len() as f32 / officine as f32;
        let cap = self
            .catalog
            .storage_cap(p, CarriageKind::Officina, ItemKind::Rottame);
        let mut lost = 0.0;
        for c in self
            .carriages
            .iter_mut()
            .filter(|c| c.kind == CarriageKind::Officina)
        {
            lost += each - c.stock.add(ItemKind::Rottame, each, cap);
        }
        self.economy.counters.rottame_capped.add(lost);
    }

    /// Midnight: perishable items spoil.
    fn spoil(&mut self) {
        for item in self.catalog.clone().kinds() {
            let keep = 1.0
                - self
                    .catalog
                    .spoilage_per_day(&self.params, item)
                    .clamp(0.0, 1.0);
            if keep < 1.0 {
                let mut spoiled = 0.0;
                for c in &mut self.carriages {
                    let amount = c.stock.get(item);
                    c.stock.set(item, amount * keep);
                    spoiled += amount - c.stock.get(item);
                }
                let c = &mut self.economy.counters;
                if item == ItemKind::Verdura {
                    c.verdura_spoiled.add(spoiled);
                } else if item == ItemKind::Razione {
                    c.razioni_spoiled.add(spoiled);
                }
            }
        }
    }

    /// Midnight: owned Vestiti wear out a little.
    fn wear_clothes(&mut self) {
        let wear = self.params.clothes_wear_per_day;
        for i in 0..self.npcs.len() {
            self.wear_item(i, ItemKind::Vestito, wear);
        }
    }

    fn check_shortages(&mut self) {
        let mut famine = false;
        for item in self.catalog.shortage_reported() {
            let empty = self.available(item) < 1.0;
            if self.shortages.len() <= item.index() {
                self.shortages.resize(item.index() + 1, false);
            }
            let flag = &mut self.shortages[item.index()];
            if empty != *flag {
                *flag = empty;
                famine |= empty && item == ItemKind::Razione;
                let kind = if empty {
                    EventKind::Shortage { item }
                } else {
                    EventKind::Restocked { item }
                };
                self.events.push(Event {
                    time: self.clock,
                    kind,
                });
            }
        }
        if famine {
            self.food_protests();
        }
    }
}

/// Why [`World::player_buy`] failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuyError {
    /// Not a Mercato, or it does not sell that item.
    NotForSale,
    /// No Mercante is working at a counter.
    NoMerchant,
    /// No whole unit left.
    OutOfStock,
    /// Not enough tokens; holds the price.
    TooExpensive(u32),
    /// The player's inventory is full.
    NoRoom,
}

impl fmt::Display for BuyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuyError::NotForSale => f.write_str("qui non si vende"),
            BuyError::NoMerchant => f.write_str("nessun mercante al bancone"),
            BuyError::OutOfStock => f.write_str("esaurito"),
            BuyError::TooExpensive(price) => write!(f, "servono {price} gettoni"),
            BuyError::NoRoom => f.write_str("l'inventario è pieno"),
        }
    }
}

/// Why [`World::player_give`] failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GiveError {
    /// The NPC does not exist (any more).
    NoSuchNpc,
    /// Not hungry, already owns one, or has no use for it.
    NotWanted,
    /// The player has none.
    NotOwned,
    /// The NPC distrusts the player ([`crate::Regard::Wary`]).
    Distrust,
}

impl fmt::Display for GiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GiveError::NoSuchNpc => f.write_str("non c'è più"),
            GiveError::NotWanted => f.write_str("non gli serve"),
            GiveError::NotOwned => f.write_str("non ce l'hai"),
            GiveError::Distrust => f.write_str("non accetta niente da te"),
        }
    }
}

/// Mercato price: base value plus transport from the nearest producer, times
/// the pay level (the administration moves wages and prices together, see
/// [`Economy`]), up to `1 + scarcity_markup` times it when the shelf is
/// empty (see [`mercato_price`]).
fn price_at(
    p: &SimParams,
    cat: &Catalog,
    level: f32,
    market: &Market,
    c: &Carriage,
    item: ItemKind,
) -> Option<u32> {
    let def = cat.get_item(item)?;
    if c.kind != CarriageKind::Mercato || !def.sold {
        return None;
    }
    let cap = cat.storage_cap(p, c.kind, item);
    let fill = if cap > 0.0 {
        (c.stock.get(item) / cap).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let distance = market.distance(c.id, item);
    Some(mercato_price(p, level, def.base_value, distance, fill))
}

/// Whether `npc` can buy `item` at `c` at time `now` (pay level `level`): a
/// Mercato with a whole unit on its shelf or stalls, the NPC wants it and can
/// pay the cheapest offer (the Mercati close at night).
#[allow(clippy::too_many_arguments)]
fn can_buy_at(
    p: &SimParams,
    cat: &Catalog,
    level: f32,
    market: &Market,
    now: GameTime,
    npc: &Npc,
    c: &Carriage,
    item: ItemKind,
) -> bool {
    !p.is_night(now.hour())
        && stalls::wants_item(p, cat, level, market, npc, item)
        && stalls::best_offer(p, cat, level, market, c, item, npc.id)
            .is_some_and(|(price, _)| price <= npc.inventory.tokens)
}

/// " (al piano di sopra)" for an upper floor, nothing for the ground floor.
fn upstairs(floor: u8) -> &'static str {
    if floor > 0 {
        " (al piano di sopra)"
    } else {
        ""
    }
}

/// Whether the train has a carriage of `kind`.
fn has_kind(carriages: &[Carriage], kind: CarriageKind) -> bool {
    carriages.iter().any(|c| c.kind == kind)
}

/// Workers wanted per job (indexed by [`Job::index`]) for `population`
/// people: food production covering everyone with a margin (assuming nobody
/// has a tool), one Mercante per [`NPCS_PER_MERCANTE`] (at least one per
/// Mercato). 0 for jobs whose workplace the train lacks; Operaio is not
/// sized (it takes everyone else).
fn job_quotas(
    p: &SimParams,
    cat: &Catalog,
    carriages: &[Carriage],
    population: usize,
) -> Vec<usize> {
    let mut quotas = vec![0; cat.job_count()];
    let daily_razioni = population as f32 * MEALS_PER_DAY * p.razioni_per_meal * FOOD_SAFETY_MARGIN;
    let quota = |amount: f32, rate: f32| {
        let needed = (amount / (EXPECTED_WORK_MINUTES_PER_DAY * rate)).ceil();
        if needed.is_finite() {
            needed.max(0.0) as usize
        } else {
            0
        }
    };
    let available = |job: Job| has_kind(carriages, job.workplace_kind());
    // Side products: Tè for the meals (from Erbe), Cotone for the Vestiti.
    let recipe = |key: &str| cat.recipe_by_key(key).map(|r| cat.recipe(r));
    let rate = |key: &str| recipe(key).map_or(0.0, |r| r.rate(p));
    let daily_te = population as f32 * MEALS_PER_DAY * p.te_per_meal;
    let erbe_per_te = recipe("te")
        .and_then(|r| r.inputs.first().map(|i| r.per_output(i, p)))
        .unwrap_or(0.0);
    let daily_cotone = population as f32 * p.clothes_wear_per_day * p.tessuto_per_vestito;
    if available(Job::Contadino) {
        let verdura = daily_razioni / p.razioni_per_verdura;
        quotas[Job::Contadino.index()] = quota(verdura, p.verdura_per_farm_minute)
            + quota(daily_te * erbe_per_te, rate("erbe"))
            + quota(daily_cotone, rate("cotone"));
    }
    if available(Job::Cuoco) {
        quotas[Job::Cuoco.index()] =
            quota(daily_razioni, p.razioni_per_cook_minute) + quota(daily_te, rate("te"));
    }
    if available(Job::Mercante) {
        let mercati = carriages
            .iter()
            .filter(|c| c.kind == CarriageKind::Mercato)
            .count();
        quotas[Job::Mercante.index()] = population.div_ceil(NPCS_PER_MERCANTE).max(mercati);
    }
    quotas
}

/// Most people the administration lets live on a train with `beds` beds.
fn max_population_for(p: &SimParams, beds: usize) -> usize {
    (beds as f32 * p.birth_max_bed_occupancy.clamp(0.0, 1.0)).floor() as usize
}

/// An age drawn from the stationary pyramid of the mortality curve (steady
/// births): the density at age `x` is proportional to the survival to `x`.
fn sample_age(p: &SimParams, rng: &mut ChaCha8Rng) -> u32 {
    for _ in 0..1000 {
        let age = rng.random_range(0..100u32);
        if rng.random::<f32>() < p.survival(age as f32 + 0.5) {
            return age;
        }
    }
    30
}

/// Ties NPCs `a` and `b` (indices): `b` is `kind` to `a`, and the inverse to `b`.
fn link(npcs: &mut [Npc], a: usize, b: usize, kind: RelationKind, affinity: f32) {
    let (ida, idb) = (npcs[a].id, npcs[b].id);
    for (holder, other, kind) in [(a, idb, kind), (b, ida, kind.inverse())] {
        let relations = &mut npcs[holder].relations;
        match relations.iter_mut().find(|r| r.other == other) {
            Some(r) => {
                r.kind = kind;
                r.affinity = affinity;
            }
            None => relations.push(Relation {
                other,
                kind,
                affinity,
            }),
        }
    }
}

/// Index of NPC `id` in `npcs` (sorted by id).
fn index_of(npcs: &[Npc], id: NpcId) -> Option<usize> {
    npcs.binary_search_by_key(&id, |n| n.id).ok()
}

/// Whether someone can be approached for a chat (idle, eating or chatting).
fn available_for_chat(npc: &Npc) -> bool {
    matches!(
        npc.action,
        Action::Idle | Action::Eat(_) | Action::Socialize(_)
    )
}

/// Who can be approached for a chat, rebuilt every tick that has decisions
/// (a scratch buffer: its allocations are reused).
#[derive(Clone, Debug, Default)]
struct Presence {
    /// NPC indices per carriage (any floor).
    by_carriage: Vec<Vec<usize>>,
    /// Per NPC id: [`Presence::spot_of`] its carriage and floor, or 0 if not available.
    spot: Vec<u32>,
}

impl Presence {
    /// Floors a carriage can have, for [`Presence::spot_of`].
    const FLOOR_SLOTS: u32 = 4;

    fn spot_of(carriage: CarriageId, floor: u8) -> u32 {
        u32::from(carriage.0) * Self::FLOOR_SLOTS + u32::from(floor).min(Self::FLOOR_SLOTS - 1) + 1
    }

    fn is_available_in(&self, id: NpcId, carriage: CarriageId, floor: u8) -> bool {
        self.spot.get(id.0 as usize) == Some(&Self::spot_of(carriage, floor))
    }
}
