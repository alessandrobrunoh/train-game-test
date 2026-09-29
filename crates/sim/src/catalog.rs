//! Cataloghi del mondo: oggetti, ricette e lavori, ampliabili durante la
//! partita.
//!
//! Every [`crate::World`] owns a [`Catalog`] ([`crate::World::catalog`]). It
//! starts from the builtin rows of [`crate::defs`] ([`Catalog::builtin`]: 13
//! items, 12 recipes, 4 jobs) and only grows: the Custode
//! ([`crate::custode`]) appends what it accepts, so ids are stable
//! ([`ItemKind::index`], [`RecipeId`], [`Job::index`] are positions here).
//! Every per-kind property is read from here: storage caps per kind of
//! carriage ([`Catalog::storage_cap`]), spoilage, outlets, what is sold, who
//! makes what ([`Catalog::recipes_of_kind`], [`Catalog::maker`]).
//!
//! Cloning is cheap (the tables are shared, copied only when something is
//! added), so the sim takes a copy to read it while it changes the world.
//!
//! **Saves.** Only what was added is serialized (the builtin rows come from
//! the code, so tuning [`SimParams`] still tunes them); a world without
//! additions writes three empty lists.

use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::carriage::CarriageKind;
use crate::custode::names::normalize;
use crate::defs::{CARRIAGES, ITEMS, ItemDef, JOBS, JobDef, Num, RECIPES, RecipeDef, RecipeId};
use crate::item::ItemKind;
use crate::npc::Job;
use crate::params::SimParams;

#[derive(Clone, Debug)]
struct Tables {
    items: Vec<ItemDef>,
    recipes: Vec<RecipeDef>,
    jobs: Vec<JobDef>,
    /// Per item (by index), per kind of carriage: the storage cap.
    caps: Vec<[Option<Num>; CarriageKind::COUNT]>,
}

/// Items, recipes and jobs of a world (see the module docs).
#[derive(Clone, Debug)]
pub struct Catalog(Arc<Tables>);

impl Default for Catalog {
    fn default() -> Self {
        Catalog::builtin()
    }
}

/// Storage caps of `def` per kind of carriage: the carriage table first,
/// then what the item declares.
fn caps_of(def: &ItemDef) -> [Option<Num>; CarriageKind::COUNT] {
    CarriageKind::ALL.map(|kind| {
        CARRIAGES[kind.index()]
            .storage
            .iter()
            .find(|s| s.item == def.kind)
            .map(|s| s.cap)
            .or_else(|| {
                def.stores
                    .iter()
                    .find(|s| s.carriage == kind)
                    .map(|s| s.cap)
            })
    })
}

impl Catalog {
    /// The builtin rows only (what a new world starts with).
    pub fn builtin() -> Catalog {
        static BUILTIN: OnceLock<Catalog> = OnceLock::new();
        BUILTIN
            .get_or_init(|| {
                let items: Vec<ItemDef> = ITEMS.to_vec();
                let caps = items.iter().map(caps_of).collect();
                Catalog(Arc::new(Tables {
                    items,
                    recipes: RECIPES.to_vec(),
                    jobs: JOBS.to_vec(),
                    caps,
                }))
            })
            .clone()
    }

    fn tables_mut(&mut self) -> &mut Tables {
        Arc::make_mut(&mut self.0)
    }

    // ------------------------------------------------------------------
    // Items
    // ------------------------------------------------------------------

    /// Every item, in id order.
    pub fn items(&self) -> &[ItemDef] {
        &self.0.items
    }

    /// Every kind of item, in id order.
    pub fn kinds(&self) -> impl Iterator<Item = ItemKind> + '_ {
        self.0.items.iter().map(|d| d.kind)
    }

    pub fn item_count(&self) -> usize {
        self.0.items.len()
    }

    /// The definition of `item`. Panics for an item of another world.
    pub fn item(&self, item: ItemKind) -> &ItemDef {
        match self.get_item(item) {
            Some(def) => def,
            None => panic!("{item:?} is not in this catalog"),
        }
    }

    /// The definition of `item`, if this catalog has it.
    pub fn get_item(&self, item: ItemKind) -> Option<&ItemDef> {
        self.0.items.get(item.index()).filter(|d| d.kind == item)
    }

    /// The kind with index `index`, if any.
    pub fn kind_at(&self, index: usize) -> Option<ItemKind> {
        self.0.items.get(index).map(|d| d.kind)
    }

    /// Whether `item` was added by the Custode (not a builtin item).
    pub fn is_added(&self, item: ItemKind) -> bool {
        !item.is_builtin()
    }

    /// Most of `item` a carriage of `kind` stores (0: it never stores it).
    pub fn storage_cap(&self, p: &SimParams, kind: CarriageKind, item: ItemKind) -> f32 {
        match self.0.caps.get(item.index()) {
            Some(caps) => caps[kind.index()].map_or(0.0, |cap| cap.get(p)),
            None => 0.0,
        }
    }

    /// Fraction of `item` stock that spoils every midnight.
    pub fn spoilage_per_day(&self, p: &SimParams, item: ItemKind) -> f32 {
        self.get_item(item)
            .and_then(|d| d.spoilage)
            .map_or(0.0, |s| s.get(p))
    }

    /// Reference price of `item` in tokens.
    pub fn base_value(&self, item: ItemKind) -> u32 {
        self.get_item(item).map_or(1, |d| d.base_value)
    }

    /// Items NPCs buy at the Mercati's shelves, in id order.
    pub fn sold_items(&self) -> Vec<ItemKind> {
        self.0
            .items
            .iter()
            .filter(|d| d.sold)
            .map(|d| d.kind)
            .collect()
    }

    /// Items whose shortage is reported, in id order.
    pub fn shortage_reported(&self) -> Vec<ItemKind> {
        self.0
            .items
            .iter()
            .filter(|d| d.shortage_reported)
            .map(|d| d.kind)
            .collect()
    }

    /// The item called `name` (singular, plural or key; case, accents and
    /// apostrophes ignored).
    pub fn find_item(&self, name: &str) -> Option<ItemKind> {
        let key = normalize(name);
        if key.is_empty() {
            return None;
        }
        self.kinds()
            .find(|k| k.key() == key || normalize(k.name()) == key || normalize(k.plural()) == key)
    }

    // ------------------------------------------------------------------
    // Recipes
    // ------------------------------------------------------------------

    /// Every recipe, in id order.
    pub fn recipes(&self) -> &[RecipeDef] {
        &self.0.recipes
    }

    /// Every recipe id, in order.
    pub fn recipe_ids(&self) -> impl Iterator<Item = RecipeId> + use<> {
        (0..self.0.recipes.len() as u16).map(RecipeId)
    }

    pub fn recipe(&self, id: RecipeId) -> &RecipeDef {
        &self.0.recipes[id.index()]
    }

    pub fn get_recipe(&self, id: RecipeId) -> Option<&RecipeDef> {
        self.0.recipes.get(id.index())
    }

    /// The recipe with this key.
    pub fn recipe_by_key(&self, key: &str) -> Option<RecipeId> {
        self.0
            .recipes
            .iter()
            .position(|r| r.key == key)
            .map(|i| RecipeId(i as u16))
    }

    /// The recipe called `name` (or with that key), or failing that the
    /// first recipe that makes the item called `name`.
    pub fn find_recipe(&self, name: &str) -> Option<RecipeId> {
        let key = normalize(name);
        let found = self
            .0
            .recipes
            .iter()
            .position(|r| normalize(&r.name) == key || normalize(&r.key) == key)
            .or_else(|| {
                let item = self.find_item(name)?;
                self.0.recipes.iter().position(|r| r.output == item)
            });
        found.map(|i| RecipeId(i as u16))
    }

    /// The job whose workers make `recipe`, if any.
    pub fn maker(&self, recipe: RecipeId) -> Option<Job> {
        self.0
            .jobs
            .iter()
            .find(|j| j.work.recipes().contains(&recipe))
            .map(|j| j.job)
    }

    /// Kinds of carriage that have the station `recipe` needs.
    pub fn places(&self, recipe: RecipeId) -> impl Iterator<Item = CarriageKind> + use<> {
        let station = self.recipe(recipe).station;
        CarriageKind::ALL
            .into_iter()
            .filter(move |k| k.def().stations.iter().any(|s| s.kind == station))
    }

    /// Whether `recipe` can be done in a carriage of `kind`.
    pub fn can_be_made_in(&self, recipe: RecipeId, kind: CarriageKind) -> bool {
        self.places(recipe).any(|k| k == kind)
    }

    /// Recipes worked in carriages of `kind` (by the jobs whose workplace
    /// it is), in job then recipe order, each output once.
    pub fn recipes_of_kind(&self, kind: CarriageKind) -> Vec<RecipeId> {
        let mut out: Vec<RecipeId> = Vec::new();
        for job in self
            .0
            .jobs
            .iter()
            .filter(|j| j.job.workplace_kind() == kind)
        {
            for &recipe in job.work.recipes() {
                let output = self.recipe(recipe).output;
                if !out.iter().any(|&r| self.recipe(r).output == output) {
                    out.push(recipe);
                }
            }
        }
        out
    }

    /// Whether carriages of `kind` make `item`.
    pub fn makes(&self, kind: CarriageKind, item: ItemKind) -> bool {
        self.0
            .jobs
            .iter()
            .filter(|j| j.job.workplace_kind() == kind)
            .any(|j| {
                j.work
                    .recipes()
                    .iter()
                    .any(|&r| self.recipe(r).output == item)
            })
    }

    /// Whether some recipe makes `item`.
    pub fn is_made(&self, item: ItemKind) -> bool {
        self.0.recipes.iter().any(|r| r.output == item)
    }

    // ------------------------------------------------------------------
    // Jobs
    // ------------------------------------------------------------------

    /// Every job, in id order.
    pub fn jobs(&self) -> &[JobDef] {
        &self.0.jobs
    }

    /// Every job, in id order.
    pub fn job_kinds(&self) -> impl Iterator<Item = Job> + '_ {
        self.0.jobs.iter().map(|d| d.job)
    }

    pub fn job_count(&self) -> usize {
        self.0.jobs.len()
    }

    /// The definition of `job`. Panics for a job of another world.
    pub fn job(&self, job: Job) -> &JobDef {
        match self.get_job(job) {
            Some(def) => def,
            None => panic!("{job:?} is not in this catalog"),
        }
    }

    pub fn get_job(&self, job: Job) -> Option<&JobDef> {
        self.0.jobs.get(job.index()).filter(|d| d.job == job)
    }

    /// The job called `name` (singular, plural or key).
    pub fn find_job(&self, name: &str) -> Option<Job> {
        let key = normalize(name);
        if key.is_empty() {
            return None;
        }
        self.job_kinds()
            .find(|j| j.key() == key || normalize(j.name()) == key || normalize(j.plural()) == key)
    }

    // ------------------------------------------------------------------
    // Growth (the Custode)
    // ------------------------------------------------------------------

    /// Appends `def` (its kind must have the next id).
    pub(crate) fn push_item(&mut self, def: ItemDef) {
        let t = self.tables_mut();
        debug_assert_eq!(def.kind.index(), t.items.len());
        t.caps.push(caps_of(&def));
        t.items.push(def);
    }

    /// Appends `def` and returns its id.
    pub(crate) fn push_recipe(&mut self, def: RecipeDef) -> RecipeId {
        let t = self.tables_mut();
        t.recipes.push(def);
        RecipeId((t.recipes.len() - 1) as u16)
    }

    /// Appends `def` (its job must have the next id).
    pub(crate) fn push_job(&mut self, def: JobDef) {
        let t = self.tables_mut();
        debug_assert_eq!(def.job.index(), t.jobs.len());
        t.jobs.push(def);
    }

    /// Changes what `job` does (a new recipe for it).
    pub(crate) fn job_mut(&mut self, job: Job) -> Option<&mut JobDef> {
        self.tables_mut().jobs.get_mut(job.index())
    }

    /// Replaces the storage of `item` declared with it, and refreshes the caps.
    pub(crate) fn item_mut(&mut self, item: ItemKind) -> Option<&mut ItemDef> {
        self.tables_mut().items.get_mut(item.index())
    }

    /// Recomputes the storage caps (after an item changed).
    pub(crate) fn refresh_caps(&mut self) {
        let t = self.tables_mut();
        t.caps = t.items.iter().map(caps_of).collect();
    }
}

impl Catalog {
    /// What is wrong with this catalog, in Italian (empty: nothing). The
    /// rules every catalog keeps, the builtin one and every one the Custode
    /// grows: recipes that can be made somewhere by someone, from inputs that
    /// exist and are stored where they are fetched; items with a source, a
    /// use, a value and a place to be stored; no chain of recipes that makes
    /// something from nothing in a loop.
    pub fn problems(&self, p: &SimParams) -> Vec<String> {
        let mut out = Vec::new();
        for (i, recipe) in self.0.recipes.iter().enumerate() {
            let id = RecipeId(i as u16);
            let what = format!("la ricetta «{}»", recipe.name);
            if self.0.recipes[..i].iter().any(|r| r.key == recipe.key) {
                out.push(format!("{what}: chiave «{}» ripetuta", recipe.key));
            }
            if recipe.batch == 0 || recipe.rate(p) <= 0.0 {
                out.push(format!("{what} non produce niente"));
            }
            if recipe.name.trim().is_empty() {
                out.push(format!("una ricetta di «{}» non ha nome", recipe.output));
            }
            if self.places(id).next().is_none() {
                out.push(format!(
                    "{what}: nessuna carrozza ha la postazione «{}»",
                    recipe.station.name()
                ));
            }
            let Some(maker) = self.maker(id) else {
                out.push(format!("{what}: nessun lavoro la fa"));
                continue;
            };
            let workplace = maker.workplace_kind();
            if !self.can_be_made_in(id, workplace) {
                out.push(format!("{what} non si può fare in {workplace}"));
            }
            if self.storage_cap(p, workplace, recipe.output) <= 0.0 {
                out.push(format!(
                    "{what}: {workplace} non può tenere «{}»",
                    recipe.output
                ));
            }
            for (k, input) in recipe.inputs.iter().enumerate() {
                if input.item == recipe.output {
                    out.push(format!("{what} usa «{}» per fare sé stesso", input.item));
                }
                if recipe.inputs[..k].iter().any(|o| o.item == input.item) {
                    out.push(format!("{what}: «{}» ripetuto", input.item));
                }
                let amount = input.amount.get(p);
                if !(amount > 0.0 && amount.is_finite()) || recipe.per_output(input, p) <= 0.0 {
                    out.push(format!("{what}: quantità di «{}» non valida", input.item));
                }
                let from = match input.from {
                    crate::defs::Source::Here => workplace,
                    crate::defs::Source::Nearest(kind) => kind,
                };
                if self.storage_cap(p, from, input.item) <= 0.0 {
                    out.push(format!(
                        "{what} prende «{}» da {from}, che non lo tiene",
                        input.item
                    ));
                }
            }
            if recipe.player_inputs(p).iter().any(|&(_, n)| n < 1) || recipe.player_minutes(p) < 1 {
                out.push(format!("{what}: il giocatore non può farla"));
            }
        }
        for def in &self.0.items {
            let item = def.kind;
            let what = format!("l'oggetto «{item}»");
            let made = self.is_made(item);
            if !made && !crate::defs::EXTERNAL_ITEMS.contains(&item) {
                out.push(format!("niente fa {what}"));
            }
            let ingredient = self
                .0
                .recipes
                .iter()
                .any(|r| r.inputs.iter().any(|i| i.item == item));
            // Razioni are eaten at the Mense; Tè and comfort goods are
            // amenities; Attrezzi and Vestiti are owned; new durable goods
            // are wanted; food is eaten; what the Custode added is traded
            // at the stalls at least.
            let used = def.added.is_some()
                || ingredient
                || def.amenity.is_some()
                || def.usage().is_owned()
                || def.desired
                || def.consume.is_some()
                || item == ItemKind::Razione;
            if !used {
                out.push(format!("{what} non serve a niente"));
            }
            if def.base_value == 0 {
                out.push(format!("{what} non vale niente"));
            }
            if def.usage().is_owned() && item.stack_size() != 1 {
                out.push(format!("{what} si possiede uno per casella"));
            }
            if item.name().is_empty()
                || item.plural().is_empty()
                || item.with_article().is_empty()
                || def.description.is_empty()
            {
                out.push(format!("{what} senza nomi o descrizione"));
            }
            if !CarriageKind::ALL
                .iter()
                .any(|&k| self.storage_cap(p, k, item) > 0.0)
            {
                out.push(format!("{what} non si conserva da nessuna parte"));
            }
            for store in def.stores.iter() {
                if !(store.cap.get(p) >= store.start && store.start >= 0.0) {
                    out.push(format!("{what}: scorta iniziale oltre il massimo"));
                }
                if CARRIAGES[store.carriage.index()]
                    .storage
                    .iter()
                    .any(|s| s.item == item)
                {
                    out.push(format!(
                        "{what} in {} è dichiarato due volte",
                        store.carriage
                    ));
                }
            }
            if let Some(amenity) = def.amenity
                && self.storage_cap(p, amenity.place(), item) <= 0.0
            {
                out.push(format!(
                    "{what} si usa in {} ma non ci sta",
                    amenity.place()
                ));
            }
            if self.storage_cap(p, item.outlet(), item) <= 0.0 {
                out.push(format!(
                    "{what} si distribuisce in {}, che non lo tiene",
                    item.outlet()
                ));
            }
        }
        // Every output can be reached from what comes from outside (the
        // train's scrap, and what is grown from nothing).
        let mut reachable: Vec<ItemKind> = crate::defs::EXTERNAL_ITEMS.to_vec();
        loop {
            let before = reachable.len();
            for r in &self.0.recipes {
                if !reachable.contains(&r.output)
                    && r.inputs.iter().all(|i| reachable.contains(&i.item))
                {
                    reachable.push(r.output);
                }
            }
            if reachable.len() == before {
                break;
            }
        }
        for r in &self.0.recipes {
            if !reachable.contains(&r.output) {
                out.push(format!(
                    "«{}» nasce solo da un giro chiuso di ricette",
                    r.output
                ));
            }
        }
        out.sort();
        out.dedup();
        out
    }
}

/// What the Custode added, the serialized part of a [`Catalog`].
#[derive(Serialize, Deserialize)]
struct Added {
    items: Vec<ItemDef>,
    recipes: Vec<RecipeDef>,
    jobs: Vec<JobDef>,
    /// New recipes of builtin jobs: (job index, recipe id).
    #[serde(default)]
    builtin_work: Vec<(u16, Vec<RecipeId>)>,
}

impl Serialize for Catalog {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let t = &self.0;
        let builtin_work = JOBS
            .iter()
            .zip(&t.jobs)
            .filter(|(old, new)| old.work.recipes() != new.work.recipes())
            .map(|(_, new)| (new.job.id(), new.work.recipes().to_vec()))
            .collect();
        Added {
            items: t.items[ITEMS.len()..].to_vec(),
            recipes: t.recipes[RECIPES.len()..].to_vec(),
            jobs: t.jobs[JOBS.len()..].to_vec(),
            builtin_work,
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for Catalog {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let added = Added::deserialize(d)?;
        let mut catalog = Catalog::builtin();
        for def in added.items {
            if def.kind.index() != catalog.item_count() {
                return Err(D::Error::custom("catalogo: oggetti fuori ordine"));
            }
            catalog.push_item(def);
        }
        for def in added.recipes {
            catalog.push_recipe(def);
        }
        for def in added.jobs {
            if def.job.index() != catalog.job_count() {
                return Err(D::Error::custom("catalogo: lavori fuori ordine"));
            }
            catalog.push_job(def);
        }
        for (job, recipes) in added.builtin_work {
            let Some(job) = catalog.job_kinds().nth(usize::from(job)) else {
                continue;
            };
            if let Some(def) = catalog.job_mut(job) {
                crate::custode::set_recipes(&mut def.work, recipes);
            }
        }
        Ok(catalog)
    }
}
