//! Il mondo: treno, NPC, orologio e ciclo di simulazione.

use rand::seq::{IndexedRandom, SliceRandom};
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::action::{Action, ActionKind, ActionOption, DecisionRequest};
use crate::brain::Brain;
use crate::carriage::{Carriage, CarriageKind, Resources, StationKind};
use crate::event::{DeathCause, Event, EventKind};
use crate::ids::{CarriageId, NpcId, StationId};
use crate::names;
use crate::npc::{Inventory, Job, Needs, Npc};
use crate::params::SimParams;
use crate::time::GameTime;

/// Expected minutes of actual work per worker per day, used to size the
/// workforce at generation time.
const EXPECTED_WORK_MINUTES_PER_DAY: f32 = 420.0;
/// Workforce is sized to produce this multiple of the daily food need.
const FOOD_SAFETY_MARGIN: f32 = 1.3;
const MEALS_PER_DAY: f32 = 3.0;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct World {
    pub clock: GameTime,
    pub params: SimParams,
    /// Ordered head to tail; `carriages[i].id == CarriageId(i)`.
    pub carriages: Vec<Carriage>,
    /// Living NPCs, sorted by id.
    pub npcs: Vec<Npc>,
    pub events: Vec<Event>,
    next_npc_id: u32,
    /// Edge-trigger for FoodShortage / FoodRestocked events.
    food_shortage: bool,
    rng: ChaCha8Rng,
    /// Scratch buffer reused every tick (see `presence_index`).
    #[serde(skip)]
    presence: Vec<Vec<usize>>,
}

impl World {
    /// Generates a train with `n_carriages` (≥ 1) carriages repeating
    /// Dormitorio / Mensa / Serra / Officina, and `n_npcs` random NPCs.
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
        let mut per_kind = [0usize; 4];
        let mut carriages: Vec<Carriage> = (0..n_carriages)
            .map(|i| {
                let k = i % CarriageKind::ALL.len();
                let kind = CarriageKind::ALL[k];
                let list = match kind {
                    CarriageKind::Dormitorio => names::DORM_NAMES,
                    CarriageKind::Mensa => names::MENSA_NAMES,
                    CarriageKind::Serra => names::SERRA_NAMES,
                    CarriageKind::Officina => names::OFFICINA_NAMES,
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
                    stock: Resources::default(),
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

        // --- People ---
        let ages: Vec<u32> = (0..n_npcs)
            .map(|_| match rng.random_range(0..100) {
                0..15 => rng.random_range(3..18),
                15..85 => rng.random_range(18..65),
                _ => rng.random_range(65..90),
            })
            .collect();

        // Size the workforce so food production covers everyone with a margin.
        let workers: Vec<usize> = (0..n_npcs)
            .filter(|&i| (18..65).contains(&ages[i]))
            .collect();
        let daily_food = n_npcs as f32 * MEALS_PER_DAY * params.food_per_meal * FOOD_SAFETY_MARGIN;
        let quota = |rate: f32| {
            let needed = (daily_food / (EXPECTED_WORK_MINUTES_PER_DAY * rate)).ceil();
            needed.min(workers.len() as f32) as usize
        };
        let mut jobs: Vec<Option<Job>> = Vec::with_capacity(workers.len());
        let available = |job: Job| carriages.iter().any(|c| c.kind == job.workplace_kind());
        if available(Job::Contadino) {
            jobs.extend(std::iter::repeat_n(
                Some(Job::Contadino),
                quota(params.food_per_farm_minute),
            ));
        }
        if available(Job::Cuoco) {
            jobs.extend(std::iter::repeat_n(
                Some(Job::Cuoco),
                quota(params.food_per_cook_minute),
            ));
        }
        jobs.truncate(workers.len());
        let operaio = available(Job::Operaio).then_some(Job::Operaio);
        jobs.resize(workers.len(), operaio);
        jobs.shuffle(&mut rng);

        let mut job_of = vec![None; n_npcs];
        for (&w, job) in workers.iter().zip(jobs) {
            job_of[w] = job;
        }

        let home_of: Vec<CarriageId> = (0..n_npcs)
            .map(|i| {
                dorms
                    .get(i % dorms.len().max(1))
                    .copied()
                    .unwrap_or(CarriageId(0))
            })
            .collect();

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
        for (i, &age) in ages.iter().enumerate() {
            let first = names::FIRST_NAMES
                .choose(&mut rng)
                .copied()
                .unwrap_or("Anna");
            let last = names::SURNAMES.choose(&mut rng).copied().unwrap_or("Rossi");
            let (home, job, workplace) = (home_of[i], job_of[i], workplace_of[i]);
            npcs.push(Npc {
                id: NpcId(i as u32),
                name: format!("{first} {last}"),
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
                inventory: Inventory::default(),
                action: Action::Idle,
                action_since: clock,
                action_until: clock + rng.random_range(0..30),
                starving_minutes: 0,
            });
        }

        // --- Stations and starting stock, sized on who lives/works where ---
        let mense_count = mense.len().max(1);
        for c in carriages.iter_mut() {
            let id = c.id;
            let residents = npcs.iter().filter(|n| n.home == id).count();
            let workers_here = |job: Job| {
                npcs.iter()
                    .filter(|n| n.job == Some(job) && n.workplace == Some(id))
                    .count()
            };
            match c.kind {
                CarriageKind::Dormitorio => c.push_stations(StationKind::Bed, residents + 2, 1),
                CarriageKind::Mensa => {
                    let seats = (n_npcs as f32 * 0.5 / mense_count as f32).ceil() as usize;
                    c.push_stations(StationKind::Table, seats.div_ceil(6).max(1), 6);
                    c.push_stations(StationKind::Stove, workers_here(Job::Cuoco) + 1, 1);
                    c.stock.food = 100.0;
                }
                CarriageKind::Serra => {
                    let n = workers_here(Job::Contadino).div_ceil(2) + 1;
                    c.push_stations(StationKind::GrowBed, n, 2);
                    c.stock.food = 100.0;
                }
                CarriageKind::Officina => {
                    let n = workers_here(Job::Operaio).div_ceil(2) + 1;
                    c.push_stations(StationKind::Workbench, n, 2);
                }
            }
        }

        World {
            clock,
            params,
            carriages,
            npcs,
            events: Vec::new(),
            next_npc_id: n_npcs as u32,
            food_shortage: false,
            rng,
            presence: Vec::new(),
        }
    }

    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    pub fn npc(&self, id: NpcId) -> Option<&Npc> {
        self.npc_index(id).map(|i| &self.npcs[i])
    }

    fn npc_index(&self, id: NpcId) -> Option<usize> {
        self.npcs.binary_search_by_key(&id, |n| n.id).ok()
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
        Some(format!(
            "{} ({moment}). {}, {} anni, si trova in {}. Casa: {}. {job} \
             Sazietà {} ({:.2}), energia {} ({:.2}), socialità {} ({:.2}).",
            self.clock,
            npc.name,
            npc.age,
            self.carriage_label(npc.carriage),
            self.carriage_label(npc.home),
            level(npc.needs.hunger),
            npc.needs.hunger,
            level(npc.needs.energy),
            npc.needs.energy,
            level(npc.needs.social),
            npc.needs.social,
        ))
    }

    /// Food stored in all Mense (what can actually be eaten).
    pub fn food_in_mense(&self) -> f32 {
        self.carriages
            .iter()
            .filter(|c| c.kind == CarriageKind::Mensa)
            .map(|c| c.stock.food)
            .sum()
    }

    pub fn total_stock(&self) -> Resources {
        self.carriages
            .iter()
            .fold(Resources::default(), |acc, c| Resources {
                food: acc.food + c.stock.food,
                materials: acc.materials + c.stock.materials,
            })
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
    /// 2. collects one [`DecisionRequest`] per idle NPC and asks the brain once;
    /// 3. starts the chosen actions (re-validated: if a station filled up in
    ///    the meantime the NPC idles briefly and decides again);
    /// 4. updates needs, starvation and deaths;
    /// 5. daily/hourly bookkeeping (food spoilage, shortage events).
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
            for (k, (&i, req)) in deciding.iter().zip(requests).enumerate() {
                let option = choices.get(k).and_then(|&c| req.options.get(c));
                self.start_action(i, option);
            }
            self.presence = presence;
        }

        self.update_needs();

        if now.minute() == 0 {
            if now.minute_of_day() == 0 {
                self.spoil_food();
            }
            self.check_food_shortage();
        }

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
    fn presence_index(&mut self) -> Vec<Vec<usize>> {
        let mut presence = std::mem::take(&mut self.presence);
        presence.resize_with(self.carriages.len(), Vec::new);
        presence.iter_mut().for_each(Vec::clear);
        for (i, npc) in self.npcs.iter().enumerate() {
            if matches!(
                npc.action,
                Action::Idle | Action::Eat(_) | Action::Socialize(_)
            ) {
                presence[npc.carriage.index()].push(i);
            }
        }
        presence
    }

    /// Candidate actions for NPC at index `i` (descriptions left empty).
    /// Only valid options are generated; `Idle` is always the first one.
    fn options_for(&mut self, i: usize, presence: &[Vec<usize>]) -> Vec<ActionOption> {
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
                && c.stock.food >= p.food_per_meal
                && c.has_free(StationKind::Table)
        };
        let can_sleep_at = |c: &Carriage| c.has_free(StationKind::Bed);

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

        // Chat with up to two people around.
        for &j in presence[here.index()]
            .sample(&mut self.rng, 3)
            .filter(|&&j| j != i)
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
        // Visit the liveliest other carriage.
        if let Some(to) = presence
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
            Action::Eat(_) => format!("mangia in {}", here.label()),
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
            Action::Socialize(other) => match self.npc(other) {
                Some(other) => format!("chiacchiera con {} ({minutes} min)", other.name),
                None => format!("chiacchiera ({minutes} min)"),
            },
            Action::Travel { to } => {
                let why = match option.goal {
                    Some(ActionKind::Eat) => " per mangiare",
                    Some(ActionKind::Sleep) if to == npc.home => " per tornare a casa a dormire",
                    Some(ActionKind::Sleep) => " per dormire",
                    Some(ActionKind::Work) => " per lavorare",
                    Some(ActionKind::Socialize) => " per fare due chiacchiere",
                    _ => "",
                };
                let dest = self
                    .carriage(to)
                    .map(|c| c.label().to_string())
                    .unwrap_or_default();
                format!("va in {dest} ({minutes} min){why}")
            }
        }
    }

    fn finish_action(&mut self, i: usize) {
        let npc = &self.npcs[i];
        let (action, here, job) = (npc.action, npc.carriage, npc.job);
        let minutes = npc.action_until.since(npc.action_since);
        if let Some(station) = action.station() {
            self.release(here, station);
        }
        match action {
            Action::Work(_) => {
                self.npcs[i].inventory.tokens += (minutes as u32 * self.params.wage_per_hour) / 60;
                if let Some(job) = job {
                    self.produce(job, here, minutes as f32);
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
                }
            }
            Action::Eat(_) | Action::Sleep(_) | Action::Idle => {}
        }
        self.npcs[i].action = Action::Idle;
    }

    fn produce(&mut self, job: Job, here: CarriageId, minutes: f32) {
        let p = &self.params;
        match job {
            Job::Contadino => {
                let stock = &mut self.carriages[here.index()].stock;
                stock.food =
                    (stock.food + minutes * p.food_per_farm_minute).min(p.food_storage_cap);
            }
            Job::Operaio => {
                let stock = &mut self.carriages[here.index()].stock;
                stock.materials = (stock.materials + minutes * p.materials_per_work_minute)
                    .min(p.materials_storage_cap);
            }
            Job::Cuoco => {
                // The cook fetches ingredients from the Serre (nearest first)
                // and turns them into portions in this Mensa.
                let space = p.food_storage_cap - self.carriages[here.index()].stock.food;
                let mut wanted = (minutes * p.food_per_cook_minute).min(space.max(0.0));
                let mut serre: Vec<usize> = self
                    .carriages
                    .iter()
                    .filter(|c| c.kind == CarriageKind::Serra)
                    .map(|c| c.id.index())
                    .collect();
                serre.sort_by_key(|&s| CarriageId(s as u16).distance(here));
                let mut moved = 0.0;
                for s in serre {
                    if wanted <= 0.0 {
                        break;
                    }
                    let take = self.carriages[s].stock.food.min(wanted);
                    self.carriages[s].stock.food -= take;
                    wanted -= take;
                    moved += take;
                }
                self.carriages[here.index()].stock.food += moved;
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
        let chosen = option.filter(|o| self.still_valid(here, &o.action));
        let (action, minutes) = match chosen {
            Some(o) => (o.action, o.minutes.max(1)),
            None => (Action::Idle, self.params.idle_min.max(1)),
        };
        if let Some(station) = action.station() {
            self.carriages[here.index()].stations[station.index()].occupancy += 1;
        }
        if let Action::Eat(_) = action {
            self.carriages[here.index()].stock.food -= self.params.food_per_meal;
        }
        let npc = &mut self.npcs[i];
        npc.action = action;
        npc.action_since = now;
        npc.action_until = now + minutes;
    }

    fn still_valid(&self, here: CarriageId, action: &Action) -> bool {
        let carriage = &self.carriages[here.index()];
        let station_free = |s: StationId| carriage.station(s).is_some_and(|s| s.has_room());
        match *action {
            Action::Eat(s) => station_free(s) && carriage.stock.food >= self.params.food_per_meal,
            Action::Sleep(s) | Action::Work(s) => station_free(s),
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
                    needs.hunger -= p.hunger_decay;
                    needs.energy -= p.energy_decay;
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
            let npc = self.npcs.remove(i);
            if let Some(station) = npc.action.station() {
                self.release(npc.carriage, station);
            }
            self.events.push(Event {
                time: now,
                kind: EventKind::NpcDied {
                    npc: npc.id,
                    name: npc.name,
                    cause: DeathCause::Starvation,
                },
            });
        }
    }

    fn spoil_food(&mut self) {
        let keep = 1.0 - self.params.food_spoilage_per_day;
        for c in &mut self.carriages {
            c.stock.food *= keep;
        }
    }

    fn check_food_shortage(&mut self) {
        let empty = self.food_in_mense() < self.params.food_per_meal;
        if empty != self.food_shortage {
            self.food_shortage = empty;
            let kind = if empty {
                EventKind::FoodShortage
            } else {
                EventKind::FoodRestocked
            };
            self.events.push(Event {
                time: self.clock,
                kind,
            });
        }
    }
}
