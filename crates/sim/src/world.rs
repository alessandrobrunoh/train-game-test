//! Il mondo: treno, NPC, orologio e ciclo di simulazione.

use std::fmt;

use rand::seq::{IndexedRandom, SliceRandom};
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::action::{Action, ActionKind, ActionOption, DecisionRequest};
use crate::brain::{Brain, THINK};
use crate::carriage::{Carriage, CarriageKind, StationKind};
use crate::event::{DeathCause, Event, EventKind};
use crate::ids::{CarriageId, NpcId, StationId};
use crate::item::{ItemKind, Stock};
use crate::names;
use crate::npc::{Inventory, Job, LifeStage, Needs, Npc, Relation, RelationKind, Sex};
use crate::params::SimParams;
use crate::time::GameTime;

mod life;

pub use life::LifeCounters;

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
/// Items whose shortage is reported (see [`EventKind::Shortage`]).
const TRACKED_SHORTAGES: [ItemKind; 3] = [ItemKind::Razione, ItemKind::Attrezzo, ItemKind::Vestito];
/// Items sold at the Mercati.
const SOLD: [ItemKind; 2] = [ItemKind::Attrezzo, ItemKind::Vestito];

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
    shortages: [bool; ItemKind::COUNT],
    rng: ChaCha8Rng,
    /// Scratch buffer reused every tick (see `presence_index`).
    #[serde(skip)]
    presence: Presence,
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

        // --- Carriages (stations are added once we know who uses them) ---
        let mut per_kind = [0usize; CarriageKind::ALL.len()];
        let mut carriages: Vec<Carriage> = (0..n_carriages)
            .map(|i| {
                let kind = LAYOUT[i % LAYOUT.len()];
                let k = kind as usize;
                let list = match kind {
                    CarriageKind::Dormitorio => names::DORM_NAMES,
                    CarriageKind::Mensa => names::MENSA_NAMES,
                    CarriageKind::Serra => names::SERRA_NAMES,
                    CarriageKind::Officina => names::OFFICINA_NAMES,
                    CarriageKind::Mercato => names::MERCATO_NAMES,
                };
                let n = per_kind[k];
                per_kind[k] += 1;
                let name = match list.get(n) {
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
        let mense = of_kind(CarriageKind::Mensa);

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

        // Size the workforce so food production covers everyone with a margin
        // (assuming nobody has a tool), then Mercanti; everyone else is an Operaio
        // (scrap-limited: idle hands keep the train in repair).
        let workers: Vec<usize> = (0..n_npcs)
            .filter(|&i| LifeStage::of_age(ages[i]).works())
            .collect();
        let quotas = job_quotas(&params, &carriages, n_npcs);
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
        for job in Job::ALL {
            let places = of_kind(job.workplace_kind());
            let mut staff: Vec<usize> = (0..n_npcs).filter(|&i| job_of[i] == Some(job)).collect();
            staff.sort_by_key(|&i| (home_of[i], i));
            for (k, &i) in staff.iter().enumerate() {
                workplace_of[i] = places.get(k * places.len() / staff.len()).copied();
            }
        }

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
            let tokens = if job.is_some() {
                rng.random_range(0..25)
            } else {
                rng.random_range(0..10)
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
                home,
                job,
                workplace,
                needs: Needs {
                    hunger: rng.random_range(0.4..0.8),
                    energy: rng.random_range(0.8..1.0),
                    social: rng.random_range(0.5..1.0),
                },
                inventory: Inventory {
                    tokens,
                    tool,
                    clothes,
                },
                action: Action::Idle,
                action_since: clock,
                action_until: clock + rng.random_range(0..30),
                starving_minutes: 0,
                relations: Vec::new(),
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
        let peak = job_quotas(&params, &carriages, peak_population);
        let per_place: [usize; 4] = Job::ALL.map(|job| {
            let places = carriages
                .iter()
                .filter(|c| c.kind == job.workplace_kind())
                .count()
                .max(1);
            peak[job.index()].div_ceil(places)
        });
        let mense_count = mense.len().max(1);
        for c in carriages.iter_mut() {
            let id = c.id;
            let workers_here = |job: Job| {
                npcs.iter()
                    .filter(|n| n.job == Some(job) && n.workplace == Some(id))
                    .count()
            };
            let staffed = |job: Job| workers_here(job).max(per_place[job.index()]);
            match c.kind {
                CarriageKind::Dormitorio => {
                    let d = dorms.iter().position(|&d| d == id).unwrap_or(0);
                    c.push_stations(StationKind::Bed, beds_of(dorm_residents[d]), 1);
                }
                CarriageKind::Mensa => {
                    let seats = (peak_population as f32 * 0.5 / mense_count as f32).ceil() as usize;
                    c.push_stations(StationKind::Table, seats.div_ceil(6).max(1), 6);
                    c.push_stations(StationKind::Stove, staffed(Job::Cuoco) + 1, 1);
                    c.stock.set(ItemKind::Razione, 100.0);
                }
                CarriageKind::Serra => {
                    let n = staffed(Job::Contadino).div_ceil(2) + 1;
                    c.push_stations(StationKind::GrowBed, n, 2);
                    c.stock.set(ItemKind::Verdura, 100.0);
                }
                CarriageKind::Officina => {
                    let n = workers_here(Job::Operaio).div_ceil(2) + 1;
                    c.push_stations(StationKind::Workbench, n, 2);
                    c.stock.set(ItemKind::Rottame, 20.0);
                    c.stock.set(ItemKind::Attrezzo, 5.0);
                    c.stock.set(ItemKind::Vestito, 5.0);
                }
                CarriageKind::Mercato => {
                    c.push_stations(StationKind::Counter, staffed(Job::Mercante) + 1, 1);
                    c.stock.set(ItemKind::Attrezzo, 10.0);
                    c.stock.set(ItemKind::Vestito, 10.0);
                }
            }
        }

        World {
            clock,
            params,
            carriages,
            npcs,
            events: Vec::new(),
            events_dropped: 0,
            next_npc_id: n_npcs as u32,
            founders: n_npcs as u32,
            life: LifeCounters::default(),
            last_birth_denied_log: None,
            shortages: [false; ItemKind::COUNT],
            rng,
            presence: Presence::default(),
        }
    }

    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    pub fn npc(&self, id: NpcId) -> Option<&Npc> {
        self.npc_index(id).map(|i| &self.npcs[i])
    }

    fn npc_index(&self, id: NpcId) -> Option<usize> {
        index_of(&self.npcs, id)
    }

    pub fn carriage(&self, id: CarriageId) -> Option<&Carriage> {
        self.carriages.get(id.index())
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
                let state = if job.in_shift(self.clock) {
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
        let family = self.family_context(npc);
        Some(format!(
            "{} ({moment}). {}, {} anni ({}), si trova in {}. Casa: {}. {job} {family}\
             Possiede {} gettoni, {tool} e {clothes}. \
             Sazietà {} ({:.2}), energia {} ({:.2}), socialità {} ({:.2}).",
            self.clock,
            npc.name,
            npc.age,
            npc.stage().name(npc.sex),
            self.carriage_label(npc.carriage),
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
    /// (whether or not it is in stock).
    pub fn price(&self, carriage: CarriageId, item: ItemKind) -> Option<u32> {
        price_at(&self.params, self.carriage(carriage)?, item)
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

    // ------------------------------------------------------------------
    // Player
    // ------------------------------------------------------------------
    //
    // The player is not an NPC: the game keeps its inventory and calls these
    // to act on the world. None of them uses randomness, so the world stays
    // deterministic given the same sequence of calls.

    /// The player takes up to `units` whole units of `item` from the storage
    /// of `carriage` (for free, but it is logged). Returns the units taken.
    pub fn player_take(&mut self, carriage: CarriageId, item: ItemKind, units: u32) -> u32 {
        let Some(c) = self.carriages.get_mut(carriage.index()) else {
            return 0;
        };
        let amount = units.min(c.stock.count(item));
        if amount == 0 {
            return 0;
        }
        c.stock.take(item, amount as f32);
        self.log(EventKind::PlayerTook {
            item,
            amount,
            carriage,
        });
        amount
    }

    /// The player buys one `item` at the Mercato `carriage`, paying the
    /// current price from `tokens`. A Mercante must be at the counter.
    /// Returns the price paid.
    pub fn player_buy(
        &mut self,
        carriage: CarriageId,
        item: ItemKind,
        tokens: &mut u32,
    ) -> Result<u32, BuyError> {
        let price = self.price(carriage, item).ok_or(BuyError::NotForSale)?;
        if self.merchant_on_duty(carriage).is_none() {
            return Err(BuyError::NoMerchant);
        }
        let c = &mut self.carriages[carriage.index()];
        if c.stock.count(item) < 1 {
            return Err(BuyError::OutOfStock);
        }
        if *tokens < price {
            return Err(BuyError::TooExpensive(price));
        }
        c.stock.take(item, 1.0);
        *tokens -= price;
        self.log(EventKind::PlayerBought {
            item,
            price,
            carriage,
        });
        Ok(price)
    }

    /// The player gives one `item` to `npc`, if it accepts it
    /// ([`Npc::accepts_gift`]). Food feeds (a Razione like a meal, raw Verdura
    /// half as much); an Attrezzo or Vestito arrives new. On error nothing
    /// changes and the player keeps the item.
    pub fn player_give(&mut self, npc: NpcId, item: ItemKind) -> Result<(), GiveError> {
        let i = self.npc_index(npc).ok_or(GiveError::NoSuchNpc)?;
        let restore = self.params.meal_restore;
        let target = &mut self.npcs[i];
        if !target.accepts_gift(item) {
            return Err(GiveError::NotWanted);
        }
        match item {
            ItemKind::Razione | ItemKind::Verdura => {
                let amount = if item == ItemKind::Razione {
                    restore
                } else {
                    restore / 2.0
                };
                target.needs.hunger = (target.needs.hunger + amount).min(1.0);
                target.starving_minutes = 0;
            }
            _ => {
                if let Some(slot) = target.inventory.slot_mut(item) {
                    *slot = Some(1.0);
                }
            }
        }
        let name = target.name.clone();
        self.log(EventKind::PlayerGave { npc, name, item });
        Ok(())
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
    ///    stipends, shortage events) and, at midnight, the life cycle (aging,
    ///    deaths, couples, births, workforce: see [`World::life`]).
    pub fn tick(&mut self, brain: &mut dyn Brain) {
        let now = self.clock;

        let mut deciding = Vec::new();
        for i in 0..self.npcs.len() {
            if self.npcs[i].action_until <= now {
                self.finish_action(i);
                deciding.push(i);
            }
        }

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
                let option = match choices.get(k) {
                    Some(&THINK) => Some(&think),
                    Some(&c) => req.options.get(c),
                    None => None,
                };
                self.start_action(i, option);
            }
            self.presence = presence;
        }

        self.update_needs();

        if now.minute() == 0 {
            self.shed_rottame();
            if now.minute_of_day() == 0 {
                self.spoil();
                self.wear_clothes();
                self.pay_stipends();
                self.daily_life();
            }
            self.check_shortages();
        }

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
        let mut presence = std::mem::take(&mut self.presence);
        let by_carriage = &mut presence.by_carriage;
        by_carriage.resize_with(self.carriages.len(), Vec::new);
        by_carriage.iter_mut().for_each(Vec::clear);
        presence.spot.clear();
        presence.spot.resize(self.next_npc_id as usize, 0);
        for (i, npc) in self.npcs.iter().enumerate() {
            if available_for_chat(npc) {
                by_carriage[npc.carriage.index()].push(i);
                presence.spot[npc.id.0 as usize] = npc.carriage.0 + 1;
            }
        }
        presence
    }

    /// Candidate actions for NPC at index `i` (descriptions left empty).
    /// Only valid options are generated; `Idle` is always the first one.
    fn options_for(&mut self, i: usize, presence: &Presence) -> Vec<ActionOption> {
        let p = &self.params;
        let now = self.clock;
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
        let wants_something = SOLD.iter().any(|&item| npc.wants(item));
        let can_shop_at = |c: &Carriage| {
            wants_something && SOLD.iter().any(|&item| can_buy_at(p, now, npc, c, item))
        };

        push(
            Action::Idle,
            self.rng.random_range(p.idle_min..=p.idle_max),
            None,
        );

        if can_eat_at(carriage)
            && let Some(table) = carriage.free_station(StationKind::Table)
        {
            push(Action::Eat(table), p.eat_minutes, None);
        }

        if let Some(bed) = carriage.free_station(StationKind::Bed) {
            let minutes = if p.is_long_sleep(now.hour()) {
                let wake =
                    now.next_at(p.wake_hour, 0) + self.rng.random_range(0..=p.wake_jitter_minutes);
                wake - now
            } else {
                p.nap_minutes
            };
            push(Action::Sleep(bed), minutes, None);
        }

        let working = match (npc.job, npc.workplace) {
            (Some(job), Some(place)) if job.in_shift(now) => Some((job, place)),
            _ => None,
        };
        if let Some((job, place)) = working
            && place == here
            && let Some(station) = carriage.free_station(job.station_kind())
        {
            let minutes = p.work_block_minutes.min(job.shift_minutes_left(now));
            push(Action::Work(station), minutes, None);
        }

        if wants_something {
            for item in SOLD {
                if can_buy_at(p, now, npc, carriage, item) {
                    push(Action::Buy(item), p.buy_minutes, None);
                }
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
            if closeness <= 0.0 || !presence.is_available_in(r.other, here) {
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
        for &j in presence.by_carriage[here.index()]
            .sample(&mut self.rng, 3)
            .filter(|&&j| j != i && !close.contains(&Some(self.npcs[j].id)))
            .take(2)
        {
            let minutes = self.rng.random_range(p.socialize_min..=p.socialize_max);
            push(Action::Socialize(self.npcs[j].id), minutes, None);
        }

        // --- Travel, each with the goal it serves ---
        let travel = |to: CarriageId| u64::from(to.distance(here)) * p.travel_minutes_per_carriage;
        let nearest = |pred: &dyn Fn(&Carriage) -> bool| {
            self.carriages
                .iter()
                .filter(|c| c.id != here && pred(c))
                .min_by_key(|c| c.id.distance(here))
                .map(|c| c.id)
        };
        if !can_eat_at(carriage)
            && let Some(to) = nearest(&can_eat_at)
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
        // Visit the liveliest other carriage.
        if let Some(to) = presence
            .by_carriage
            .iter()
            .enumerate()
            .filter(|&(c, people)| c != here.index() && !people.is_empty())
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
        match option.action {
            Action::Idle => format!("resta a oziare per {minutes} minuti"),
            Action::Eat(_) => format!("mangia una razione in {}", here.label()),
            Action::Sleep(_) if self.params.is_long_sleep(self.clock.hour()) => format!(
                "va a dormire in {} fino alle {:02}:{:02}",
                here.label(),
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
                let price = price_at(&self.params, here, item).unwrap_or(0);
                format!(
                    "compra {} in {} per {price} gettoni (ne ha {})",
                    item.with_article(),
                    here.label(),
                    npc.inventory.tokens
                )
            }
            Action::Socialize(other) => match self.npc(other) {
                Some(other) => format!("chiacchiera con {} ({minutes} min)", other.name),
                None => format!("chiacchiera ({minutes} min)"),
            },
            Action::Travel { to } => {
                let dest = self.carriage(to);
                let shopping = dest.and_then(|c| {
                    SOLD.into_iter()
                        .find(|&item| can_buy_at(&self.params, self.clock, npc, c, item))
                });
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
        let minutes = npc.action_until.since(npc.action_since);
        if let Some(station) = action.station() {
            self.release(here, station);
        }
        match action {
            Action::Work(_) => {
                self.npcs[i].inventory.tokens += (minutes as u32 * self.params.wage_per_hour) / 60;
                if let Some(job) = job {
                    let bonus = if job.uses_tool() && has_tool {
                        self.params.tool_output_bonus
                    } else {
                        1.0
                    };
                    self.produce(job, here, minutes as f32, bonus);
                    if job.uses_tool() {
                        let wear = minutes as f32 * self.params.tool_wear_per_work_minute;
                        self.wear_item(i, ItemKind::Attrezzo, wear);
                    }
                }
            }
            Action::Travel { to } => self.npcs[i].carriage = to,
            Action::Socialize(partner) => {
                let bonus = self.params.socialize_partner_bonus;
                if let Some(j) = self.npc_index(partner)
                    && self.npcs[j].carriage == here
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
            Action::Eat(_) | Action::Sleep(_) | Action::Buy(_) | Action::Idle => {}
        }
        self.npcs[i].action = Action::Idle;
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
        }
    }

    /// Indices of carriages of `kind`, nearest to `here` first.
    fn nearest_of_kind(&self, kind: CarriageKind, here: CarriageId) -> Vec<usize> {
        let mut found: Vec<usize> = self
            .carriages
            .iter()
            .filter(|c| c.kind == kind)
            .map(|c| c.id.index())
            .collect();
        found.sort_by_key(|&c| (CarriageId(c as u16).distance(here), c));
        found
    }

    /// Effect of `minutes` of work by a `job` in carriage `here`; `bonus`
    /// multiplies the output (tool).
    fn produce(&mut self, job: Job, here: CarriageId, minutes: f32, bonus: f32) {
        let p = &self.params;
        let kind = self.carriages[here.index()].kind;
        let cap = |item| p.storage_cap(kind, item);
        match job {
            Job::Contadino => {
                let amount = minutes * p.verdura_per_farm_minute * bonus;
                let cap = cap(ItemKind::Verdura);
                self.carriages[here.index()]
                    .stock
                    .add(ItemKind::Verdura, amount, cap);
            }
            Job::Cuoco => {
                // The cook fetches Verdura from the Serre (nearest first) and
                // cooks it into Razioni stored in this Mensa.
                let per_verdura = p.razioni_per_verdura;
                if per_verdura <= 0.0 {
                    return;
                }
                let cap = cap(ItemKind::Razione);
                let space = cap - self.carriages[here.index()].stock.get(ItemKind::Razione);
                let mut wanted =
                    (minutes * p.razioni_per_cook_minute).min(space.max(0.0)) / per_verdura;
                let mut used = 0.0;
                for s in self.nearest_of_kind(CarriageKind::Serra, here) {
                    if wanted <= 0.0 {
                        break;
                    }
                    let take = self.carriages[s].stock.take(ItemKind::Verdura, wanted);
                    wanted -= take;
                    used += take;
                }
                self.carriages[here.index()]
                    .stock
                    .add(ItemKind::Razione, used * per_verdura, cap);
            }
            Job::Operaio => {
                // Craft whatever is scarcer on the train (Officine + Mercati);
                // fall back to the other item if that one can't be made.
                let on_train =
                    |item| self.available(item) + self.stock_in(CarriageKind::Officina).get(item);
                let first = if on_train(ItemKind::Attrezzo) <= on_train(ItemKind::Vestito) {
                    [ItemKind::Attrezzo, ItemKind::Vestito]
                } else {
                    [ItemKind::Vestito, ItemKind::Attrezzo]
                };
                for item in first {
                    let (rate, cost) = match item {
                        ItemKind::Attrezzo => (p.attrezzi_per_craft_minute, p.rottame_per_attrezzo),
                        _ => (p.vestiti_per_craft_minute, p.rottame_per_vestito),
                    };
                    let stock = &self.carriages[here.index()].stock;
                    let space = (cap(item) - stock.get(item)).max(0.0);
                    let by_scrap = if cost > 0.0 {
                        stock.get(ItemKind::Rottame) / cost
                    } else {
                        f32::INFINITY
                    };
                    let made = (minutes * rate * bonus).min(space).min(by_scrap);
                    if made > 0.0 {
                        let stock = &mut self.carriages[here.index()].stock;
                        stock.take(ItemKind::Rottame, made * cost);
                        stock.add(item, made, f32::INFINITY);
                        break;
                    }
                }
            }
            Job::Mercante => {
                // Bring finished goods from the Officine (nearest first), the
                // item this Mercato has less of first.
                let mut budget = minutes * p.goods_per_trade_minute;
                let mut items = SOLD;
                items.sort_by(|a, b| {
                    let stock = &self.carriages[here.index()].stock;
                    stock.get(*a).total_cmp(&stock.get(*b))
                });
                let officine = self.nearest_of_kind(CarriageKind::Officina, here);
                for item in items {
                    let cap = cap(item);
                    for &o in &officine {
                        let space = cap - self.carriages[here.index()].stock.get(item);
                        let wanted = budget.min(space);
                        if wanted <= 0.0 {
                            break;
                        }
                        let taken = self.carriages[o].stock.take(item, wanted);
                        self.carriages[here.index()].stock.add(item, taken, cap);
                        budget -= taken;
                    }
                }
            }
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
        let (action, minutes) = match chosen {
            Some(o) => (o.action, o.minutes.max(1)),
            None => (Action::Idle, self.params.idle_min.max(1)),
        };
        if let Some(station) = action.station() {
            self.carriages[here.index()].stations[station.index()].occupancy += 1;
        }
        match action {
            Action::Eat(_) => {
                self.carriages[here.index()]
                    .stock
                    .take(ItemKind::Razione, self.params.razioni_per_meal);
            }
            Action::Buy(item) => self.buy(i, item),
            _ => {}
        }
        let npc = &mut self.npcs[i];
        npc.action = action;
        npc.action_since = now;
        npc.action_until = now + minutes;
    }

    /// NPC `i` pays for one `item` at its (Mercato) carriage and owns a new one.
    /// The tokens leave circulation (back to the train administration).
    fn buy(&mut self, i: usize, item: ItemKind) {
        let here = self.npcs[i].carriage;
        let carriage = &mut self.carriages[here.index()];
        let price = price_at(&self.params, carriage, item).unwrap_or(0);
        carriage.stock.take(item, 1.0);
        let npc = &mut self.npcs[i];
        npc.inventory.tokens = npc.inventory.tokens.saturating_sub(price);
        if let Some(slot) = npc.inventory.slot_mut(item) {
            *slot = Some(1.0);
        }
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
            Action::Buy(item) => can_buy_at(&self.params, self.clock, npc, carriage, item),
            Action::Travel { to } => to != here && to.index() < self.carriages.len(),
            Action::Socialize(partner) => self.npc_index(partner).is_some(),
            Action::Idle => true,
        }
    }

    fn update_needs(&mut self) {
        let p = &self.params;
        let now = self.clock;
        let mut dead = Vec::new();
        for (i, npc) in self.npcs.iter_mut().enumerate() {
            let needs = &mut npc.needs;
            match npc.action {
                Action::Sleep(_) => {
                    needs.hunger -= p.hunger_decay * p.sleep_hunger_factor;
                    needs.energy += p.sleep_energy_gain;
                }
                action => {
                    let warm = if npc.inventory.clothes.is_some() {
                        p.clothes_energy_factor
                    } else {
                        1.0
                    };
                    needs.hunger -= p.hunger_decay;
                    needs.energy -= p.energy_decay * warm;
                    needs.social -= p.social_decay;
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
                if npc.starving_minutes >= p.starvation_minutes {
                    dead.push(i);
                }
            } else {
                npc.starving_minutes = 0;
            }
        }
        for &i in dead.iter().rev() {
            self.kill(i, DeathCause::Starvation);
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
        let cap = p.storage_cap(CarriageKind::Officina, ItemKind::Rottame);
        for c in self
            .carriages
            .iter_mut()
            .filter(|c| c.kind == CarriageKind::Officina)
        {
            c.stock.add(ItemKind::Rottame, each, cap);
        }
    }

    /// Midnight: perishable items spoil.
    fn spoil(&mut self) {
        for item in ItemKind::ALL {
            let keep = 1.0 - self.params.spoilage_per_day(item).clamp(0.0, 1.0);
            if keep < 1.0 {
                for c in &mut self.carriages {
                    let amount = c.stock.get(item);
                    c.stock.set(item, amount * keep);
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

    /// Midnight: NPCs without a job get a small stipend.
    fn pay_stipends(&mut self) {
        let stipend = self.params.stipend_per_day;
        for npc in self.npcs.iter_mut().filter(|n| n.job.is_none()) {
            npc.inventory.tokens = npc.inventory.tokens.saturating_add(stipend);
        }
    }

    fn check_shortages(&mut self) {
        for item in TRACKED_SHORTAGES {
            let empty = self.available(item) < 1.0;
            let flag = &mut self.shortages[item.index()];
            if empty != *flag {
                *flag = empty;
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
}

impl fmt::Display for BuyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuyError::NotForSale => f.write_str("qui non si vende"),
            BuyError::NoMerchant => f.write_str("nessun mercante al bancone"),
            BuyError::OutOfStock => f.write_str("esaurito"),
            BuyError::TooExpensive(price) => write!(f, "servono {price} gettoni"),
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
}

impl fmt::Display for GiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GiveError::NoSuchNpc => f.write_str("non c'è più"),
            GiveError::NotWanted => f.write_str("non gli serve"),
        }
    }
}

/// Mercato price: base value, up to `1 + scarcity_markup` times it when the
/// shelf is empty.
fn price_at(p: &SimParams, c: &Carriage, item: ItemKind) -> Option<u32> {
    if c.kind != CarriageKind::Mercato || !item.is_sold() {
        return None;
    }
    let cap = p.storage_cap(c.kind, item);
    let fill = if cap > 0.0 {
        (c.stock.get(item) / cap).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let price = item.base_value() as f32 * (1.0 + p.scarcity_markup * (1.0 - fill));
    Some(price.round().max(0.0) as u32)
}

/// Whether `npc` can buy `item` at `c` at time `now`: a Mercato with a whole
/// unit in stock, the NPC wants it and can pay (the Mercati close at night).
fn can_buy_at(p: &SimParams, now: GameTime, npc: &Npc, c: &Carriage, item: ItemKind) -> bool {
    !p.is_night(now.hour())
        && npc.wants(item)
        && c.stock.count(item) >= 1
        && price_at(p, c, item).is_some_and(|price| price <= npc.inventory.tokens)
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
fn job_quotas(p: &SimParams, carriages: &[Carriage], population: usize) -> [usize; 4] {
    let mut quotas = [0; 4];
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
    if available(Job::Contadino) {
        let verdura = daily_razioni / p.razioni_per_verdura;
        quotas[Job::Contadino.index()] = quota(verdura, p.verdura_per_farm_minute);
    }
    if available(Job::Cuoco) {
        quotas[Job::Cuoco.index()] = quota(daily_razioni, p.razioni_per_cook_minute);
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
    /// NPC indices per carriage.
    by_carriage: Vec<Vec<usize>>,
    /// Per NPC id: carriage index + 1, or 0 if not available.
    spot: Vec<u16>,
}

impl Presence {
    fn is_available_in(&self, id: NpcId, carriage: CarriageId) -> bool {
        self.spot.get(id.0 as usize) == Some(&(carriage.0 + 1))
    }
}
