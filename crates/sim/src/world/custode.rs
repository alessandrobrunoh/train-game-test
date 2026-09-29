//! Il Custode nel mondo: esame, applicazione e vita delle novità.
//!
//! See [`crate::custode`] for the flow. What each kind of novelty becomes:
//!
//! - **New item** (made from existing items by an existing job): a row in
//!   the catalog with its names (plural and article guessed if the draft has
//!   none), usage by category (a consumable is food and feeds, a durable
//!   good is wanted by who can afford it, raw materials and intermediates
//!   are materials), storage in its maker's workplace (the outlet), a
//!   spoilage for food and crops, the stack limit and the icon's look; and a
//!   recipe for it, added to the maker's work. Workers make it when it is
//!   the scarcest of their outputs and keep a share (so it reaches the
//!   stalls); the player crafts it at the same station.
//! - **New recipe**: a row in the catalog, added to its job's work.
//! - **New job**: a row in the catalog, stations for its workers in every
//!   carriage of its kind, and every midnight a few Operai (the train's
//!   spare hands) move to it until it has its staff, without leaving the
//!   Mense, Serre or Mercati short (see [`World::staff_added_jobs`]). It
//!   makes the items it names with the recipes that exist for them, or (a
//!   guard, a doctor) serves the people around it ([`Work::Service`]).
//! - **Event**: its stock and need changes, once, at the minute it is
//!   applied.
//! - **Statistic**: joins [`World::statistics`], sampled every game hour.
//!
//! The checks ([`World::review`]): names resolve (and new ones are free),
//! values in range, a recipe's job works where its inputs can be fetched and
//! its output stored, nothing is made from nothing outside a Serra, an
//! output is not worth much more than its inputs, no loop of recipes gives
//! back more than it takes, a new job has stations it can use in a kind of
//! carriage the train has, and the grown catalog still passes
//! [`crate::Catalog::problems`].

use std::borrow::Cow;

use super::{NEEDED_JOBS, World, job_quotas};
use crate::carriage::{CarriageKind, StationKind};
use crate::custode::sources::{Source as StatSource, carriage_kind};
use crate::custode::statistic::{Formula, MAX_TERMS, MAX_WEIGHT, cycle};
use crate::custode::{
    Applied, Category, Decision, Draft, Effect, EventEffect, EventPlan, Ingredient, ItemPlan,
    JobPlan, Need, Output, Plan, Proposal, RecipePlan, Rejection, Scheduled, Statistic, Statistics,
    WorkPlan, category_of, names, normalize, set_recipes, usage_of,
};
use crate::defs::{
    Consume, ItemCategory, ItemDef, ItemUse, JobDef, Num, RecipeDef, RecipeId, Source,
    StationCount, Store, Work,
};
use crate::item::{ItemInfoData, ItemKind};
use crate::job::JobInfoData;
use crate::npc::{Job, Npc};
use crate::time::GameTime;

/// Most ingredients of a recipe.
pub const MAX_INPUTS: usize = 4;
/// Most effects of an event.
pub const MAX_EFFECTS: usize = 5;
/// Shelf of a Mercato for a new durable good.
const MARKET_CAP: f32 = 10.0;
/// Workers a new job wants in every carriage of its kind.
pub const NEW_JOB_STAFF: u16 = 2;
/// Most people who move to one new job in one night.
const MOVES_PER_NIGHT: usize = 3;
/// Operai kept per Officina when people move to new jobs.
const MIN_OPERAI_PER_OFFICINA: usize = 2;
/// Need a service raises, per minute of work, for everyone around.
pub const SERVICE_PER_MINUTE: f32 = 0.0004;
/// Longest chain of recipes followed looking for a loop.
const LOOP_DEPTH: usize = 4;

fn reject<T>(reason: impl Into<String>) -> Result<T, Rejection> {
    Err(Rejection::new(reason))
}

fn in_range(what: &str, value: u32, min: u32, max: u32) -> Result<(), Rejection> {
    if (min..=max).contains(&value) {
        Ok(())
    } else {
        reject(format!("{what} deve essere tra {min} e {max} (è {value})"))
    }
}

/// Storage of a new item in its maker's workplace, by category.
fn default_cap(category: ItemCategory) -> f32 {
    match category {
        ItemCategory::Raw => 60.0,
        ItemCategory::Intermediate | ItemCategory::Consumable => 40.0,
        ItemCategory::Durable => 20.0,
    }
}

/// Minutes of work for one of a new item, by category.
fn default_minutes(category: ItemCategory) -> f32 {
    match category {
        ItemCategory::Raw | ItemCategory::Consumable => 20.0,
        ItemCategory::Intermediate => 30.0,
        ItemCategory::Durable => 60.0,
    }
}

impl World {
    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    /// Every proposal the Custode decided on, in order: applied or refused,
    /// at which minute.
    pub fn decisions(&self) -> &[Decision] {
        &self.custode.decisions
    }

    /// The proposals applied to this world, in order.
    pub fn applied(&self) -> impl Iterator<Item = &Decision> {
        self.custode.decisions.iter().filter(|d| d.result.is_ok())
    }

    /// The decision on the draft scheduled as `seq`, once taken.
    pub fn decision(&self, seq: u32) -> Option<&Decision> {
        self.custode.decisions.iter().find(|d| d.seq == seq)
    }

    /// Drafts waiting for their minute, soonest first.
    pub fn pending(&self) -> &[Scheduled] {
        &self.custode.pending
    }

    /// The derived statistics and their history.
    pub fn statistics(&self) -> &Statistics {
        &self.custode.statistics
    }

    /// Adds `samples` (game hour, value; an older record, e.g. a game saved
    /// before the statistics lived in the World) in front of the history of
    /// the statistic `name`, keeping the newest [`crate::custode::STAT_HISTORY`].
    pub fn adopt_statistic_history(&mut self, name: &str, samples: Vec<(u64, f32)>) {
        let stats = &mut self.custode.statistics;
        if stats.get(name).is_none() {
            return;
        }
        let newer: Vec<(u64, f32)> = stats.history(name).collect();
        let first = newer.first().map_or(u64::MAX, |s| s.0);
        let mut all: Vec<(u64, f32)> = samples.into_iter().filter(|s| s.0 < first).collect();
        all.extend(newer);
        let keep = all.len().saturating_sub(crate::custode::STAT_HISTORY);
        stats.set_history(name, all.into_iter().skip(keep));
    }

    /// What to schedule in a new world with the same seed (and brain) to
    /// replay this one: every decision's draft at its minute.
    pub fn replay_list(&self) -> Vec<(GameTime, Draft)> {
        let mut list: Vec<(GameTime, Draft)> = self
            .custode
            .decisions
            .iter()
            .map(|d| (d.at, d.draft.clone()))
            .collect();
        list.extend(self.custode.pending.iter().map(|p| (p.at, p.draft.clone())));
        list
    }

    // ------------------------------------------------------------------
    // Scheduling and applying
    // ------------------------------------------------------------------

    /// Applies `draft` at the start of the tick of minute `at` (now if `at`
    /// has passed), inside the simulation: a world replayed with the same
    /// schedule decides the same. Returns its sequence number, to find the
    /// decision later ([`World::decision`]).
    pub fn schedule(&mut self, draft: Draft, at: GameTime) -> u32 {
        let seq = self.custode.next_seq;
        self.custode.next_seq += 1;
        let at = at.max(self.clock);
        let k = self
            .custode
            .pending
            .iter()
            .position(|p| (p.at, p.seq) > (at, seq))
            .unwrap_or(self.custode.pending.len());
        self.custode.pending.insert(k, Scheduled { seq, at, draft });
        seq
    }

    /// Reviews `draft` and, if it passes, makes it real now (see the module
    /// docs); the outcome is recorded either way ([`World::decisions`]) at
    /// minute `at` (the current one, normally).
    pub fn apply(&mut self, draft: &Draft, at: GameTime) -> Result<Applied, Rejection> {
        let seq = self.custode.next_seq;
        self.custode.next_seq += 1;
        self.decide(seq, draft.clone(), at)
    }

    fn decide(&mut self, seq: u32, draft: Draft, at: GameTime) -> Result<Applied, Rejection> {
        let result = self
            .review(&draft.proposal)
            .and_then(|plan| self.commit(plan, at));
        self.custode.decisions.push(Decision {
            seq,
            at,
            draft,
            result: result.clone(),
        });
        result
    }

    /// Every tick: applies the drafts whose minute came; on the hour,
    /// samples the statistics.
    pub(super) fn custode_tick(&mut self) {
        let now = self.clock;
        while self.custode.pending.first().is_some_and(|p| p.at <= now) {
            let s = self.custode.pending.remove(0);
            let _ = self.decide(s.seq, s.draft, now);
        }
        if now.minute() == 0 && !self.custode.statistics.is_empty() {
            let mut stats = std::mem::take(&mut self.custode.statistics);
            stats.sample(self);
            self.custode.statistics = stats;
        }
    }

    // ------------------------------------------------------------------
    // Review
    // ------------------------------------------------------------------

    /// Checks `proposal` against this world and resolves it into a [`Plan`]
    /// (see the module docs), or says why not, in Italian.
    pub fn review(&self, proposal: &Proposal) -> Result<Plan, Rejection> {
        match proposal {
            Proposal::NewItem {
                name,
                description,
                category,
                base_value,
                stack_limit,
                made_from,
                made_by_job,
                appearance,
            } => self.review_item(
                name,
                description,
                *category,
                *base_value,
                *stack_limit,
                made_from,
                made_by_job,
                appearance.as_ref(),
            ),
            Proposal::NewRecipe {
                name,
                output,
                output_qty,
                inputs,
                job,
                minutes,
            } => self
                .review_recipe(name, output, *output_qty, inputs, job, *minutes)
                .map(Plan::Recipe),
            Proposal::NewJob {
                name,
                description,
                workplace_kind,
                makes,
                service,
            } => self.review_job(name, description, workplace_kind, makes, *service),
            Proposal::Event {
                title,
                description,
                effects,
            } => self.review_event(title, description, effects),
            Proposal::Statistic(stat) => self.review_statistic(stat),
        }
    }

    /// What already uses the name `key`, if anything.
    fn name_taken(&self, key: &str) -> Option<String> {
        let cat = &self.catalog;
        if let Some(item) = cat.find_item(key) {
            return Some(format!("l'oggetto «{item}»"));
        }
        if let Some(job) = cat.find_job(key) {
            return Some(format!("il lavoro «{job}»"));
        }
        if let Some(r) = cat
            .recipes()
            .iter()
            .find(|r| normalize(&r.name) == key || normalize(&r.key) == key)
        {
            return Some(format!("la ricetta «{}»", r.name));
        }
        if let Some(s) = self.custode.statistics.get(key) {
            return Some(format!("la statistica «{}»", s.name));
        }
        if carriage_kind(key).is_some() {
            return Some("un tipo di carrozza".to_string());
        }
        None
    }

    /// A new name: not empty, not too long, not taken. Returns its key.
    fn fresh_name(&self, name: &str) -> Result<String, Rejection> {
        let key = normalize(name);
        let n = name.trim().chars().count();
        if key.is_empty() || n > 40 {
            return reject(format!("il nome «{name}» deve avere da 1 a 40 caratteri"));
        }
        if let Some(what) = self.name_taken(&key) {
            return reject(format!("il nome «{name}» è già usato da {what}"));
        }
        Ok(key)
    }

    fn item_named(&self, name: &str, what: &str) -> Result<ItemKind, Rejection> {
        self.catalog.find_item(name).ok_or_else(|| {
            let known: Vec<&str> = self.catalog.kinds().map(ItemKind::name).collect();
            Rejection::new(format!(
                "{what} «{name}» non esiste; oggetti esistenti: {}",
                known.join(", ")
            ))
        })
    }

    /// A job that makes things (not a Mercante, not a service).
    fn making_job(&self, name: &str) -> Result<Job, Rejection> {
        let Some(job) = self.catalog.find_job(name) else {
            let known: Vec<&str> = self.catalog.job_kinds().map(Job::name).collect();
            return reject(format!(
                "il lavoro «{name}» non esiste; lavori esistenti: {}",
                known.join(", ")
            ));
        };
        match self.catalog.job(job).work {
            Work::Trade { .. } | Work::Service { .. } => reject(format!(
                "il {job} non fabbrica oggetti: scegli un lavoro che produce"
            )),
            _ => Ok(job),
        }
    }

    /// Where a worker in `workplace` takes `item` from: there if it is
    /// stored there, else the nearest carriages of a kind that stores it
    /// (its makers' first, then its outlet).
    fn source_for(&self, item: ItemKind, workplace: CarriageKind) -> Option<Source> {
        let cap = |k| self.catalog.storage_cap(&self.params, k, item);
        if cap(workplace) > 0.0 {
            return Some(Source::Here);
        }
        let makers = CarriageKind::ALL
            .into_iter()
            .filter(|&k| self.catalog.makes(k, item));
        makers
            .chain([item.outlet()])
            .chain(CarriageKind::ALL)
            .find(|&k| cap(k) > 0.0 && self.carriages.iter().any(|c| c.kind == k))
            .map(Source::Nearest)
    }

    /// The inputs of a recipe made in `workplace`, resolved.
    fn inputs(
        &self,
        list: &[Ingredient],
        workplace: CarriageKind,
        job: Job,
    ) -> Result<Vec<(ItemKind, u32, Source)>, Rejection> {
        if list.len() > MAX_INPUTS {
            return reject(format!("al massimo {MAX_INPUTS} ingredienti"));
        }
        let mut out: Vec<(ItemKind, u32, Source)> = Vec::new();
        for i in list {
            let item = self.item_named(&i.item, "l'ingrediente")?;
            if out.iter().any(|&(o, _, _)| o == item) {
                return reject(format!("l'ingrediente «{item}» è ripetuto"));
            }
            in_range(&format!("la quantità di «{item}»"), i.qty, 1, 10)?;
            let Some(from) = self.source_for(item, workplace) else {
                return reject(format!(
                    "«{item}» non si conserva in nessuna carrozza del treno: il {job} \
                     non saprebbe dove prenderlo"
                ));
            };
            out.push((item, i.qty, from));
        }
        Ok(out)
    }

    /// Value of `inputs` in tokens (base values).
    fn inputs_value(&self, inputs: &[(ItemKind, u32, Source)]) -> u32 {
        inputs
            .iter()
            .map(|&(item, qty, _)| self.catalog.base_value(item) * qty)
            .sum()
    }

    /// A recipe key not used yet, from `name`.
    fn fresh_recipe_key(&self, name: &str) -> String {
        let base = normalize(name);
        let mut key = base.clone();
        let mut n = 2;
        while self.catalog.recipe_by_key(&key).is_some() {
            key = format!("{base} {n}");
            n += 1;
        }
        key
    }

    #[allow(clippy::too_many_arguments)]
    fn review_item(
        &self,
        name: &str,
        description: &str,
        category: Category,
        base_value: u32,
        stack_limit: u32,
        made_from: &[Ingredient],
        made_by_job: &str,
        appearance: Option<&crate::custode::Appearance>,
    ) -> Result<Plan, Rejection> {
        let key = self.fresh_name(name)?;
        let job = self.making_job(made_by_job)?;
        in_range("il valore", base_value, 1, 200)?;
        in_range("la pila", stack_limit, 1, 50)?;
        if description.trim().is_empty() {
            return reject("serve una descrizione");
        }
        let category = category_of(category);
        let (workplace, station) = (job.workplace_kind(), job.station_kind());
        let inputs = self.inputs(made_from, workplace, job)?;
        if inputs.iter().any(|&(i, _, _)| normalize(i.name()) == key) {
            return reject(format!("«{name}» non può essere ingrediente di sé stesso"));
        }
        let grown = inputs.is_empty();
        if grown && (category != ItemCategory::Raw || station != StationKind::GrowBed) {
            return reject(format!(
                "«{name}» senza ingredienti si potrebbe solo coltivare in una Serra (una \
                 materia prima dei contadini): dagli degli ingredienti"
            ));
        }
        let inputs_value = self.inputs_value(&inputs);
        let cap = if grown { 5 } else { 3 * inputs_value + 15 };
        if base_value > cap {
            return reject(format!(
                "«{name}» varrebbe {base_value} gettoni, ma si fa con {inputs_value} gettoni di \
                 ingredienti: al massimo {cap}"
            ));
        }
        let lower = name.trim().to_lowercase();
        let usage = usage_of(category);
        let info = ItemInfoData {
            key: key.clone(),
            plural: names::plural_of(&lower),
            with_article: names::with_article(&lower),
            name: lower.clone(),
            usage,
            outlet: workplace,
            stack_limit: Some(if category == ItemCategory::Durable {
                stack_limit.min(5)
            } else {
                stack_limit
            }),
        };
        let consume = (usage == ItemUse::Food)
            .then(|| ((0.08 * base_value as f32).clamp(0.1, 0.5), 0.0, 0.0));
        let spoilage = match category {
            ItemCategory::Consumable => Some(0.05),
            ItemCategory::Raw if grown => Some(0.02),
            _ => None,
        };
        let appearance = appearance
            .filter(|a| a.shape().is_some() && a.colour().is_some())
            .cloned();
        // A durable good made in an Officina reaches the Mercati like the
        // Attrezzi: the Mercanti bring it, who can afford it buys it.
        let durable = category == ItemCategory::Durable;
        let sold = durable && workplace == CarriageKind::Officina;
        let mut stores = vec![(workplace, default_cap(category))];
        if sold {
            stores.push((CarriageKind::Mercato, MARKET_CAP));
        }
        let item = ItemPlan {
            description: description.trim().to_string(),
            base_value,
            category,
            stores,
            spoilage,
            consume,
            desired: durable,
            sold,
            appearance,
        };
        let recipe = RecipePlan {
            key: self.fresh_recipe_key(&key),
            name: format!("fare {}", info.with_article),
            output: Output::New,
            batch: 1,
            minutes: default_minutes(category),
            inputs,
            station,
            maker: job,
            store_output: None,
        };
        Ok(Plan::Item { info, item, recipe })
    }

    fn review_recipe(
        &self,
        name: &str,
        output: &str,
        output_qty: u32,
        inputs: &[Ingredient],
        job: &str,
        minutes: u32,
    ) -> Result<RecipePlan, Rejection> {
        self.fresh_name(name)?;
        let out = self.item_named(output, "il prodotto")?;
        let job = self.making_job(job)?;
        in_range("la quantità prodotta", output_qty, 1, 10)?;
        in_range("i minuti", minutes, 10, 480)?;
        let (workplace, station) = (job.workplace_kind(), job.station_kind());
        let inputs = self.inputs(inputs, workplace, job)?;
        if inputs.iter().any(|&(i, _, _)| i == out) {
            return reject(format!("«{out}» non può essere ingrediente di sé stesso"));
        }
        let def = self.catalog.item(out);
        if inputs.is_empty()
            && (def.category != ItemCategory::Raw || station != StationKind::GrowBed)
        {
            return reject(
                "una ricetta ha bisogno di ingredienti (senza, si può solo coltivare in una Serra)",
            );
        }
        // The same output from the same inputs exists already.
        let mut shape: Vec<ItemKind> = inputs.iter().map(|&(i, _, _)| i).collect();
        shape.sort();
        if let Some(same) = self.catalog.recipes().iter().find(|r| {
            let mut other: Vec<ItemKind> = r.inputs.iter().map(|i| i.item).collect();
            other.sort();
            r.output == out && other == shape
        }) {
            return reject(format!(
                "la ricetta «{}» fa già «{out}» con gli stessi ingredienti",
                same.name
            ));
        }
        // The output has to be kept where it is made.
        let mut store_output = None;
        if self.catalog.storage_cap(&self.params, workplace, out) <= 0.0 {
            if out.is_builtin() {
                return reject(format!(
                    "il {job} lavora in {workplace}, dove «{out}» non si conserva"
                ));
            }
            store_output = Some((workplace, default_cap(def.category)));
        }
        let value = self.catalog.base_value(out) * output_qty;
        let inputs_value = self.inputs_value(&inputs);
        let cap = 3 * inputs_value + 15;
        if value > cap {
            return reject(format!(
                "troppo facile: {output_qty} «{out}» valgono {value} gettoni e gli ingredienti \
                 {inputs_value}"
            ));
        }
        // No loop of recipes that gives back more than it takes.
        for &(input, qty, _) in &inputs {
            let there = output_qty as f32 / qty as f32;
            let back = self.best_yield(out, input, LOOP_DEPTH);
            if back > 0.0 && there * back >= 1.0 {
                return reject(format!(
                    "giro senza costo: da «{input}» si fa «{out}» e da «{out}» si torna a \
                     «{input}» guadagnandoci"
                ));
            }
        }
        Ok(RecipePlan {
            key: self.fresh_recipe_key(name),
            name: name.trim().to_lowercase(),
            output: Output::Existing(out),
            batch: output_qty,
            minutes: minutes as f32,
            inputs,
            station,
            maker: job,
            store_output,
        })
    }

    /// The most units of `to` one unit of `from` becomes through a chain of
    /// at most `depth` existing recipes (0: none).
    fn best_yield(&self, from: ItemKind, to: ItemKind, depth: usize) -> f32 {
        if depth == 0 {
            return 0.0;
        }
        let mut best: f32 = 0.0;
        for r in self.catalog.recipes() {
            let Some(input) = r.inputs.iter().find(|i| i.item == from) else {
                continue;
            };
            let per = r.per_output(input, &self.params);
            if per <= 0.0 || !per.is_finite() {
                continue;
            }
            let step = 1.0 / per;
            let y = if r.output == to {
                step
            } else {
                step * self.best_yield(r.output, to, depth - 1)
            };
            best = best.max(y);
        }
        best
    }

    fn review_job(
        &self,
        name: &str,
        description: &str,
        workplace: &str,
        makes: &[String],
        service: Option<Need>,
    ) -> Result<Plan, Rejection> {
        let key = self.fresh_name(name)?;
        if description.trim().is_empty() {
            return reject("serve una descrizione");
        }
        let Some(kind) = carriage_kind(workplace) else {
            let kinds: Vec<&str> = CarriageKind::ALL.iter().map(|k| k.name()).collect();
            return reject(format!(
                "la carrozza «{workplace}» non esiste; tipi di carrozza: {}",
                kinds.join(", ")
            ));
        };
        if !self.carriages.iter().any(|c| c.kind == kind) {
            return reject(format!("sul treno non c'è nessun {kind}"));
        }
        let Some(rule) = kind
            .def()
            .stations
            .iter()
            .find(|r| matches!(r.count, StationCount::Workers { .. }))
        else {
            let places: Vec<&str> = CarriageKind::ALL
                .iter()
                .filter(|k| {
                    k.def()
                        .stations
                        .iter()
                        .any(|r| matches!(r.count, StationCount::Workers { .. }))
                })
                .map(|k| k.name())
                .collect();
            return reject(format!(
                "in un {kind} non c'è una postazione di lavoro: scegli tra {}",
                places.join(", ")
            ));
        };
        let station = rule.kind;
        let work = if makes.is_empty() {
            let Some(need) = service else {
                return reject(
                    "un lavoro deve produrre oggetti esistenti o dare un «servizio» \
                     (sazieta, energia o socialita) a chi gli sta intorno",
                );
            };
            WorkPlan::Service {
                need,
                per_minute: SERVICE_PER_MINUTE,
            }
        } else {
            if makes.len() > 4 {
                return reject("un lavoro produce al massimo 4 cose");
            }
            let mut recipes: Vec<RecipeId> = Vec::new();
            for m in makes {
                let found: Vec<RecipeId> = match self.catalog.recipe_ids().find(|&r| {
                    let def = self.catalog.recipe(r);
                    normalize(&def.name) == normalize(m) || def.key == normalize(m)
                }) {
                    Some(r) => vec![r],
                    None => {
                        let item = self.item_named(m, "l'oggetto")?;
                        self.catalog
                            .recipe_ids()
                            .filter(|&r| self.catalog.recipe(r).output == item)
                            .collect()
                    }
                };
                let here: Vec<RecipeId> = found
                    .iter()
                    .copied()
                    .filter(|&r| self.catalog.recipe(r).station == station)
                    .collect();
                if here.is_empty() {
                    let can: Vec<&str> = self
                        .catalog
                        .recipes()
                        .iter()
                        .filter(|r| r.station == station)
                        .map(|r| r.output.name())
                        .collect();
                    return reject(format!(
                        "«{m}» non si fa a una {} ({kind}); lì si fa: {}",
                        station.name(),
                        can.join(", ")
                    ));
                }
                for r in here {
                    if !recipes.contains(&r) {
                        recipes.push(r);
                    }
                }
            }
            WorkPlan::Make(recipes)
        };
        let like = Job::BUILTIN
            .into_iter()
            .find(|j| j.workplace_kind() == kind);
        let lower = name.trim().to_lowercase();
        let info = JobInfoData {
            key,
            plural: names::plural_of(&lower),
            name: lower,
            workplace: kind,
            station,
            shift: like.map_or((8, 17), Job::shift),
            uses_tool: like.is_some_and(Job::uses_tool) && matches!(work, WorkPlan::Make(_)),
        };
        Ok(Plan::Job(JobPlan {
            info,
            description: description.trim().to_string(),
            work,
            staff: NEW_JOB_STAFF,
            station_capacity: rule.capacity.max(1),
        }))
    }

    fn review_event(
        &self,
        title: &str,
        description: &str,
        effects: &[Effect],
    ) -> Result<Plan, Rejection> {
        if title.trim().is_empty() || description.trim().is_empty() {
            return reject("un evento ha bisogno di un titolo e di una descrizione");
        }
        if effects.is_empty() || effects.len() > MAX_EFFECTS {
            return reject(format!("un evento deve avere da 1 a {MAX_EFFECTS} effetti"));
        }
        let kind_of = |name: &str| -> Result<CarriageKind, Rejection> {
            let kind = carriage_kind(name)
                .ok_or_else(|| Rejection::new(format!("la carrozza «{name}» non esiste")))?;
            if !self.carriages.iter().any(|c| c.kind == kind) {
                return reject(format!("sul treno non c'è nessun {kind}"));
            }
            Ok(kind)
        };
        let mut out = Vec::new();
        for e in effects {
            out.push(match e {
                Effect::Stock {
                    carriage_kind,
                    item,
                    delta,
                } => {
                    let kind = kind_of(carriage_kind)?;
                    let item = self.item_named(item, "l'oggetto")?;
                    if *delta == 0 || delta.abs() > 50 {
                        return reject(format!(
                            "la variazione di scorta deve essere tra -50 e 50 e non zero (è {delta})"
                        ));
                    }
                    if self.catalog.storage_cap(&self.params, kind, item) <= 0.0 {
                        return reject(format!("in un {kind} non si conserva «{item}»"));
                    }
                    EventEffect::Stock {
                        kind,
                        item,
                        delta: *delta,
                    }
                }
                Effect::Need {
                    carriage_kind,
                    need,
                    delta,
                } => {
                    let kind = carriage_kind.as_deref().map(kind_of).transpose()?;
                    if !delta.is_finite() || *delta == 0.0 || delta.abs() > 0.3 {
                        return reject(format!(
                            "la variazione di un bisogno deve essere tra -0.3 e 0.3 e non zero (è {delta})"
                        ));
                    }
                    EventEffect::Need {
                        kind,
                        need: *need,
                        delta: *delta,
                    }
                }
            });
        }
        Ok(Plan::Event(EventPlan {
            title: title.trim().to_string(),
            description: description.trim().to_string(),
            effects: out,
        }))
    }

    fn review_statistic(&self, stat: &Statistic) -> Result<Plan, Rejection> {
        self.fresh_name(&stat.name)?;
        let [lo, hi] = stat.scale;
        if !(lo.is_finite() && hi.is_finite() && lo < hi) {
            return reject(format!("la scala [{lo}, {hi}] non è valida"));
        }
        if let Formula::Invalid(why) = &stat.formula {
            return reject(why.clone());
        }
        let terms = stat.formula.terms();
        if terms.is_empty() || terms.len() > MAX_TERMS {
            return reject(format!("una formula deve avere da 1 a {MAX_TERMS} termini"));
        }
        for t in terms {
            if !t.weight.is_finite() || t.weight == 0.0 || t.weight.abs() > MAX_WEIGHT {
                return reject(format!("il peso {} non è valido", t.weight));
            }
            if let StatSource::Invalid(why) = &t.source {
                return reject(why.clone());
            }
            for (kind, name) in t.source.references() {
                let found = match kind {
                    "oggetto" => self.catalog.find_item(name).is_some(),
                    "lavoro" => self.catalog.find_job(name).is_some(),
                    "carrozza" => carriage_kind(name).is_some(),
                    "statistica" => self.custode.statistics.get(name).is_some(),
                    _ => false,
                };
                if !found {
                    return reject(format!("{kind} «{name}» non esiste nel mondo"));
                }
            }
        }
        let deps = self
            .custode
            .statistics
            .iter()
            .map(|s| {
                let refs = s.formula.stat_refs().into_iter().map(normalize).collect();
                (normalize(&s.name), refs)
            })
            .collect();
        if let Some(path) = cycle(&stat.name, &stat.formula.stat_refs(), &deps) {
            return reject(format!("ciclo tra statistiche: {}", path.join(" → ")));
        }
        Ok(Plan::Statistic(stat.clone()))
    }

    // ------------------------------------------------------------------
    // Commit
    // ------------------------------------------------------------------

    /// Makes `plan` real (it passed the review).
    fn commit(&mut self, plan: Plan, at: GameTime) -> Result<Applied, Rejection> {
        match plan {
            Plan::Item { info, item, recipe } => {
                let mut cat = self.catalog.clone();
                let kind = ItemKind::new(cat.item_count() as u16, &info);
                let stores: Vec<Store> = item
                    .stores
                    .iter()
                    .map(|&(carriage, cap)| Store {
                        carriage,
                        cap: Num::Fixed(cap),
                        start: 0.0,
                    })
                    .collect();
                cat.push_item(ItemDef {
                    kind,
                    description: Cow::Owned(item.description),
                    base_value: item.base_value,
                    category: item.category,
                    sold: item.sold,
                    shortage_reported: false,
                    spoilage: item.spoilage.map(Num::Fixed),
                    amenity: None,
                    stores: Cow::Owned(stores),
                    consume: item.consume.map(|(h, e, s)| Consume {
                        hunger: Num::Fixed(h),
                        energy: Num::Fixed(e),
                        social: Num::Fixed(s),
                    }),
                    desired: item.desired,
                    appearance: item.appearance,
                    added: Some(at),
                });
                let maker = recipe.maker;
                let id = self.push_recipe(&mut cat, recipe, kind, at);
                self.adopt(cat)?;
                Ok(Applied {
                    item: Some(kind),
                    recipe: Some(id),
                    job: None,
                    statistic: None,
                    summary: format!(
                        "Nuovo oggetto «{}»: lo fanno {} {} ({})",
                        kind.name(),
                        names::plural_article(maker.plural()),
                        maker.plural(),
                        maker.workplace_kind()
                    ),
                })
            }
            Plan::Recipe(recipe) => {
                let mut cat = self.catalog.clone();
                let Output::Existing(out) = recipe.output else {
                    return reject("la ricetta non ha un prodotto");
                };
                let maker = recipe.maker;
                let name = recipe.name.clone();
                let id = self.push_recipe(&mut cat, recipe, out, at);
                self.adopt(cat)?;
                Ok(Applied {
                    item: None,
                    recipe: Some(id),
                    job: None,
                    statistic: None,
                    summary: format!(
                        "Nuova ricetta «{name}»: {} {} fanno {}",
                        names::plural_article(maker.plural()),
                        maker.plural(),
                        out.plural()
                    ),
                })
            }
            Plan::Job(plan) => {
                let mut cat = self.catalog.clone();
                let job = Job::new(cat.job_count() as u16, &plan.info);
                let work = match &plan.work {
                    WorkPlan::Make(list) => match list.as_slice() {
                        [one] => Work::Make(*one),
                        list => Work::MakeScarcest(Cow::Owned(list.to_vec())),
                    },
                    WorkPlan::Service { need, per_minute } => Work::Service {
                        need: *need,
                        per_minute: *per_minute,
                    },
                };
                cat.push_job(JobDef {
                    job,
                    description: Cow::Owned(plan.description.clone()),
                    work,
                    staff: Some(plan.staff),
                    added: Some(at),
                });
                self.adopt(cat)?;
                // Stations for its workers, in every carriage of its kind.
                let per = usize::from(plan.staff).div_ceil(usize::from(plan.station_capacity)) + 1;
                for c in self
                    .carriages
                    .iter_mut()
                    .filter(|c| c.kind == plan.info.workplace)
                {
                    c.push_stations(plan.info.station, per, plan.station_capacity, 0);
                }
                let what = match &plan.work {
                    WorkPlan::Make(list) => {
                        let outs: Vec<&str> = list
                            .iter()
                            .map(|&r| self.catalog.recipe(r).output.plural())
                            .collect();
                        format!("fa {}", outs.join(", "))
                    }
                    WorkPlan::Service { need, .. } => {
                        format!("dà {} a chi gli sta intorno", need.name())
                    }
                };
                Ok(Applied {
                    item: None,
                    recipe: None,
                    job: Some(job),
                    statistic: None,
                    summary: format!(
                        "Nuovo lavoro «{}» in {}: {what}",
                        job.name(),
                        plan.info.workplace
                    ),
                })
            }
            Plan::Event(event) => {
                let mut changes = Vec::new();
                for e in &event.effects {
                    changes.push(self.event_effect(*e));
                }
                Ok(Applied {
                    item: None,
                    recipe: None,
                    job: None,
                    statistic: None,
                    summary: format!("Evento «{}»: {}", event.title, changes.join("; ")),
                })
            }
            Plan::Statistic(stat) => {
                let key = normalize(&stat.name);
                let name = stat.name.clone();
                // Sampled from the next full hour (see `custode_tick`).
                self.custode.statistics.add(stat);
                Ok(Applied {
                    item: None,
                    recipe: None,
                    job: None,
                    statistic: Some(key),
                    summary: format!("Nuova statistica «{name}»"),
                })
            }
        }
    }

    /// Adds `plan`, making `output`, to `cat` and to its maker's work.
    fn push_recipe(
        &self,
        cat: &mut crate::Catalog,
        plan: RecipePlan,
        output: ItemKind,
        at: GameTime,
    ) -> RecipeId {
        if let Some((kind, cap)) = plan.store_output
            && let Some(def) = cat.item_mut(output)
        {
            let mut stores = def.stores.to_vec();
            stores.push(Store {
                carriage: kind,
                cap: Num::Fixed(cap),
                start: 0.0,
            });
            def.stores = Cow::Owned(stores);
            cat.refresh_caps();
        }
        let inputs: Vec<crate::defs::Input> = plan
            .inputs
            .iter()
            .map(|&(item, qty, from)| crate::defs::Input {
                item,
                amount: Num::Fixed(qty as f32),
                from,
            })
            .collect();
        let id = cat.push_recipe(RecipeDef {
            key: Cow::Owned(plan.key),
            name: Cow::Owned(plan.name),
            output,
            batch: plan.batch,
            minutes: Num::Fixed(plan.minutes),
            inputs: Cow::Owned(inputs),
            station: plan.station,
            basic: true,
            added: Some(at),
        });
        if let Some(def) = cat.job_mut(plan.maker) {
            let mut list = def.work.recipes().to_vec();
            list.push(id);
            set_recipes(&mut def.work, list);
        }
        id
    }

    /// Takes `cat` as the world's catalog if it still passes every rule
    /// ([`crate::Catalog::problems`]); the market follows it.
    fn adopt(&mut self, cat: crate::Catalog) -> Result<(), Rejection> {
        let before = self.catalog.problems(&self.params);
        if let Some(problem) = cat
            .problems(&self.params)
            .into_iter()
            .find(|p| !before.contains(p))
        {
            return reject(problem);
        }
        self.catalog = cat;
        self.market.locate_producers(&self.carriages, &self.catalog);
        if !self.market.stall_count.is_empty() {
            self.market
                .recount_stalls(self.carriages.len(), self.catalog.item_count());
        }
        Ok(())
    }

    /// Applies one effect of an event now; returns what happened, in Italian.
    fn event_effect(&mut self, effect: EventEffect) -> String {
        match effect {
            EventEffect::Stock { kind, item, delta } => {
                let places: Vec<usize> = self
                    .carriages
                    .iter()
                    .filter(|c| c.kind == kind)
                    .map(|c| c.id.index())
                    .collect();
                if places.is_empty() {
                    return format!("nessun {kind}");
                }
                let cap = self.catalog.storage_cap(&self.params, kind, item);
                let each = delta.unsigned_abs() as f32 / places.len() as f32;
                let mut moved = 0.0;
                for c in places {
                    let stock = &mut self.carriages[c].stock;
                    moved += if delta > 0 {
                        stock.add(item, each, cap)
                    } else {
                        stock.take(item, each)
                    };
                }
                let sign = if delta > 0 { "+" } else { "-" };
                format!("{sign}{moved:.0} {} in {kind}", item.plural())
            }
            EventEffect::Need { kind, need, delta } => {
                let mut n = 0;
                for npc in self.npcs.iter_mut() {
                    let here = self.carriages.get(npc.carriage.index()).map(|c| c.kind);
                    if kind.is_some() && here != kind {
                        continue;
                    }
                    let value = need_of(npc, need);
                    *value = (*value + delta).clamp(0.0, 1.0);
                    n += 1;
                }
                let place = kind.map_or("tutti".to_string(), |k| format!("chi è in {k}"));
                format!("{} {delta:+.2} per {place} ({n} persone)", need.name())
            }
        }
    }

    // ------------------------------------------------------------------
    // Staffing
    // ------------------------------------------------------------------

    /// Midnight, after the builtin staffing: the jobs the Custode added
    /// fill up to their staff, a few people a night, from who has no job
    /// and then from the Operai (the most recent first), keeping
    /// [`MIN_OPERAI_PER_OFFICINA`] per Officina. If a Mensa, Serra or
    /// Mercato is short of workers and no Operaio is left, people from the
    /// new jobs go back to it first.
    pub(super) fn staff_added_jobs(&mut self, counts: &mut [usize], staff: &mut [usize]) {
        let added: Vec<(Job, usize)> = self
            .catalog
            .jobs()
            .iter()
            .filter_map(|d| Some((d.job, usize::from(d.staff?))))
            .collect();
        if added.is_empty() {
            return;
        }
        let is_added = |job: Option<Job>| job.is_some_and(|j| !j.is_builtin());
        // Vital jobs first.
        let mut quotas = job_quotas(
            &self.params,
            &self.catalog,
            &self.carriages,
            self.npcs.len(),
        );
        let farm = Job::Contadino.index();
        quotas[farm] = self.farm_quota(quotas[farm]);
        for job in NEEDED_JOBS {
            while counts[job.index()] < quotas[job.index()] {
                let Some(i) = self.npcs.iter().rposition(|n| is_added(n.job)) else {
                    break;
                };
                self.assign_job(i, Some(job), counts, staff);
            }
        }
        let officine = self
            .carriages
            .iter()
            .filter(|c| c.kind == CarriageKind::Officina)
            .count();
        let min_operai = officine * MIN_OPERAI_PER_OFFICINA;
        for (job, per_carriage) in added {
            let places = self
                .carriages
                .iter()
                .filter(|c| c.kind == job.workplace_kind())
                .count();
            let wanted = per_carriage * places;
            let mut moves = 0;
            while counts.get(job.index()).copied().unwrap_or(0) < wanted && moves < MOVES_PER_NIGHT
            {
                let jobless = self
                    .npcs
                    .iter()
                    .position(|n| n.stage().works() && n.job.is_none());
                let spare = || {
                    (counts[Job::Operaio.index()] > min_operai)
                        .then(|| self.npcs.iter().rposition(|n| n.job == Some(Job::Operaio)))
                        .flatten()
                };
                let Some(i) = jobless.or_else(spare) else {
                    break;
                };
                self.assign_job(i, Some(job), counts, staff);
                moves += 1;
            }
        }
    }
}

/// The need `need` of `npc`.
fn need_of(npc: &mut Npc, need: Need) -> &mut f32 {
    match need {
        Need::Satiety => &mut npc.needs.hunger,
        Need::Energy => &mut npc.needs.energy,
        Need::Social => &mut npc.needs.social,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::Action;
    use crate::custode::Draft;
    use crate::ids::CarriageId;

    #[test]
    fn a_service_raises_the_need_of_who_is_around() {
        let mut w = World::generate(3, 10, 80);
        let now = w.clock;
        let guard = w
            .apply(
                &Draft::new(
                    "Nessuno si sente al sicuro.",
                    Proposal::NewJob {
                        name: "Guardia".into(),
                        description: "Sorveglia il Mercato.".into(),
                        workplace_kind: "Mercato".into(),
                        makes: Vec::new(),
                        service: Some(Need::Social),
                    },
                ),
                now,
            )
            .unwrap()
            .job
            .unwrap();
        let market = w
            .carriages
            .iter()
            .find(|c| c.kind == CarriageKind::Mercato)
            .unwrap()
            .id;
        for n in w.npcs.iter_mut().take(5) {
            n.carriage = market;
            n.action = Action::Idle;
            n.needs.social = 0.2;
        }
        let far = w.npcs[5].id;
        w.npcs[5].needs.social = 0.2;
        w.npcs[5].carriage = CarriageId(0);
        assert_eq!(w.produce(guard, market, 100.0, 1.0), None);
        for n in w.npcs.iter().take(5) {
            assert!((n.needs.social - (0.2 + 100.0 * SERVICE_PER_MINUTE)).abs() < 1e-5);
        }
        assert_eq!(w.npc(far).unwrap().needs.social, 0.2, "only who is there");
        assert_eq!(w.economy.counters.wasted_share(guard), 0.0);
    }
}
