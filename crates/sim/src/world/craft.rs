//! Ricette: la produzione degli NPC e il crafting del giocatore.
//!
//! NPC workers and the player use the same recipes of the world's
//! catalog ([`crate::Catalog::recipes`]):
//! - `World::produce`: `minutes` of work make `rate × minutes × tool bonus`
//!   of the output of one of the job's recipes (see [`Work`] for which one),
//!   bounded by the storage of the workplace and by the inputs, taken from
//!   their [`Source`] (nearest carriages first).
//! - [`World::player_craft`]: one whole batch from the player's inventory,
//!   in a carriage that has the recipe's station. It is instant in the sim:
//!   the game makes the player wait the recipe's minutes first.

use std::fmt;

use super::World;
use crate::carriage::{CarriageKind, StationKind};
use crate::catalog::Catalog;
use crate::custode::Need;
use crate::defs::{RecipeDef, RecipeId, Source, Work};
use crate::ids::CarriageId;
use crate::item::ItemKind;
use crate::npc::Job;
use crate::params::SimParams;

/// Why [`World::player_craft`] failed. Nothing changes on error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CraftError {
    NoSuchCarriage,
    /// The carriage has no station of this kind, which the recipe needs.
    WrongPlace(StationKind),
    /// Not enough of an input.
    Missing {
        item: ItemKind,
        needed: u32,
        have: u32,
    },
    /// The player has not learnt it yet.
    Unknown,
    /// No room in the inventory for what it makes.
    NoRoom,
}

impl fmt::Display for CraftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CraftError::NoSuchCarriage => f.write_str("carrozza inesistente"),
            CraftError::WrongPlace(station) => {
                write!(f, "qui non c'è la postazione giusta ({})", station.name())
            }
            CraftError::Missing { item, needed, have } => {
                let name = if *needed == 1 {
                    item.name()
                } else {
                    item.plural()
                };
                write!(f, "servono {needed} {name}, ne hai {have}")
            }
            CraftError::Unknown => f.write_str("non conosci questa ricetta"),
            CraftError::NoRoom => f.write_str("l'inventario è pieno"),
        }
    }
}

/// Share of the potential output actually made.
fn share(made: f32, potential: f32) -> f32 {
    if potential > 0.0 {
        made / potential
    } else {
        0.0
    }
}

impl World {
    // ------------------------------------------------------------------
    // Player
    // ------------------------------------------------------------------

    /// The player makes one batch of `recipe` in `carriage` from its own
    /// inventory: it must know the recipe ([`crate::PlayerCharacter::knows`]),
    /// the carriage must have the recipe's station and the inventory the
    /// whole inputs ([`RecipeDef::player_inputs`]), which are used up, and
    /// room for the batch once they are gone. Returns the units made (added
    /// to the inventory). The time it takes ([`RecipeDef::player_minutes`])
    /// is up to the game. No randomness: the world stays deterministic.
    pub fn player_craft(
        &mut self,
        recipe: RecipeId,
        carriage: CarriageId,
    ) -> Result<u32, CraftError> {
        let cat = self.catalog.clone();
        let recipe = cat.get_recipe(recipe).ok_or(CraftError::Unknown)?;
        if !self.player.knows(recipe) {
            return Err(CraftError::Unknown);
        }
        let c = self.carriage(carriage).ok_or(CraftError::NoSuchCarriage)?;
        if !c.stations.iter().any(|s| s.kind == recipe.station) {
            return Err(CraftError::WrongPlace(recipe.station));
        }
        let inputs = recipe.player_inputs(&self.params);
        let items = &self.player.inventory;
        for &(item, needed) in &inputs {
            let have = items.count(item);
            if have < needed {
                return Err(CraftError::Missing { item, needed, have });
            }
        }
        // Tried on a copy: a full inventory keeps its ingredients.
        let mut after = items.clone();
        for &(item, needed) in &inputs {
            after.remove(item, needed);
        }
        if after.add(recipe.output, recipe.batch) < recipe.batch {
            return Err(CraftError::NoRoom);
        }
        self.player.inventory = after;
        Ok(recipe.batch)
    }

    /// Recipes that can be made in `carriage` (it has their station), in
    /// catalog order.
    pub fn recipes_at(&self, carriage: CarriageId) -> Vec<RecipeId> {
        let Some(c) = self.carriage(carriage) else {
            return Vec::new();
        };
        let cat = &self.catalog;
        cat.recipe_ids()
            .filter(|&r| c.stations.iter().any(|s| s.kind == cat.recipe(r).station))
            .collect()
    }

    // ------------------------------------------------------------------
    // NPC workers
    // ------------------------------------------------------------------

    /// Share of the storage for `item` that is filled, over the carriages
    /// of its outlet kind and of `workplace` kind (1 if none can hold it):
    /// how "scarce on the train" it is for [`Work::MakeScarcest`].
    pub fn fill_on_train(&self, item: ItemKind, workplace: CarriageKind) -> f32 {
        let (mut stock, mut cap) = (0.0, 0.0);
        for c in &self.carriages {
            if c.kind == item.outlet() || c.kind == workplace {
                stock += c.stock.get(item);
                cap += self.catalog.storage_cap(&self.params, c.kind, item);
            }
        }
        if cap > 0.0 { stock / cap } else { 1.0 }
    }

    /// Effect of `minutes` of work by a `job` in carriage `here`; `bonus`
    /// multiplies the output (tool). Booked in [`super::EconomyCounters`],
    /// with the share of the work that produced nothing. Returns what was
    /// made and how much, if anything (moving goods and services make
    /// nothing).
    pub(super) fn produce(
        &mut self,
        job: Job,
        here: CarriageId,
        minutes: f32,
        bonus: f32,
    ) -> Option<(ItemKind, f32)> {
        let mut output = None;
        let cat = self.catalog.clone();
        let wasted = match &cat.job(job).work {
            Work::Trade { from, rate } => {
                let potential = minutes * rate.get(&self.params);
                let moved = self.trade(*from, potential, here);
                1.0 - share(moved, potential)
            }
            Work::Service { need, per_minute } => {
                let served = self.serve(here, *need, per_minute * minutes * bonus);
                if served { 0.0 } else { 1.0 }
            }
            work => {
                let order = self.recipe_order(&cat, work, job, here);
                // Specialties (see `market.rs`) only shape the "make the
                // scarcest" work; staples (food) are never slowed down.
                let specialized = matches!(work, Work::MakeScarcest(_));
                let work = minutes * bonus;
                let mut booked = None;
                for &id in &order {
                    let recipe = cat.recipe(id);
                    let factor = if specialized {
                        self.specialty_factor(here, recipe.output)
                    } else {
                        1.0
                    };
                    let (made, potential) = self.run_recipe(recipe, here, work * factor);
                    if made > 0.0 {
                        booked = Some((recipe.output, made, potential));
                        break;
                    }
                }
                // Nothing made: the work is booked on the first choice.
                let booked = booked.or_else(|| {
                    let first = cat.recipe(*order.first()?);
                    Some((first.output, 0.0, work * first.rate(&self.params)))
                });
                match booked {
                    Some((item, made, potential)) => {
                        self.economy.counters.book_made(item, made, potential);
                        if made > 0.0 {
                            output = Some((item, made));
                        }
                        1.0 - share(made, potential)
                    }
                    None => 1.0,
                }
            }
        };
        self.book_work(job, minutes, wasted);
        output
    }

    /// A service ([`Work::Service`]): `amount` of `need` for everyone in
    /// carriage `here` who is not walking through it. Returns whether
    /// anyone was there.
    fn serve(&mut self, here: CarriageId, need: Need, amount: f32) -> bool {
        let amount = amount.max(0.0);
        let mut served = false;
        for npc in self.npcs.iter_mut().filter(|n| n.carriage == here) {
            if matches!(npc.action, crate::Action::Travel { .. }) {
                continue;
            }
            let value = match need {
                Need::Satiety => &mut npc.needs.hunger,
                Need::Energy => &mut npc.needs.energy,
                Need::Social => &mut npc.needs.social,
            };
            *value = (*value + amount).min(1.0);
            served = true;
        }
        served
    }

    /// The recipes of `work` in the order a worker in `here` tries them.
    fn recipe_order(
        &self,
        cat: &Catalog,
        work: &Work,
        job: Job,
        here: CarriageId,
    ) -> Vec<RecipeId> {
        match work {
            Work::Make(recipe) => vec![*recipe],
            Work::MakeScarcest(recipes) => {
                // The carriage's specialties first, each group scarcest first.
                let mut order = self.by_scarcity(cat, recipes, job);
                let specialties = self.market.specialties_of(here);
                if !specialties.is_empty() {
                    order.sort_by_key(|&r| !specialties.contains(&cat.recipe(r).output));
                }
                order
            }
            Work::MakeStaple { recipes, keep } => {
                let Some(&staple) = recipes.first() else {
                    return Vec::new();
                };
                let staple = cat.recipe(staple);
                let c = &self.carriages[here.index()];
                let cap = cat.storage_cap(&self.params, c.kind, staple.output);
                let fill = if cap > 0.0 {
                    c.stock.get(staple.output) / cap
                } else {
                    1.0
                };
                if fill < keep.get(&self.params) {
                    // The staple first; the rest only if it can't be made.
                    recipes.to_vec()
                } else {
                    self.by_scarcity(cat, recipes, job)
                }
            }
            Work::Trade { .. } | Work::Service { .. } => Vec::new(),
        }
    }

    /// Output multiplier of `item` made in carriage `here`: a bonus for its
    /// specialties, a penalty for the rest (1 if it has none). Items the
    /// Custode added are nobody's specialty: always 1.
    fn specialty_factor(&self, here: CarriageId, item: ItemKind) -> f32 {
        let p = &self.params;
        if !item.is_builtin() {
            return 1.0;
        }
        match self.market.specialties_of(here) {
            [] => 1.0,
            s if s.contains(&item) => p.specialty_output_bonus.max(0.0),
            _ => p.off_specialty_output.max(0.0),
        }
    }

    /// `recipes` with the scarcest output on the train first (stable on ties).
    fn by_scarcity(&self, cat: &Catalog, recipes: &[RecipeId], job: Job) -> Vec<RecipeId> {
        let workplace = job.workplace_kind();
        let mut order: Vec<(f32, RecipeId)> = recipes
            .iter()
            .map(|&r| (self.fill_on_train(cat.recipe(r).output, workplace), r))
            .collect();
        order.sort_by(|a, b| a.0.total_cmp(&b.0));
        order.into_iter().map(|(_, r)| r).collect()
    }

    /// Makes up to `work × rate` of `recipe`'s output into carriage `here`,
    /// bounded by its storage and by the inputs, taken from their sources
    /// (nearest first). Returns `(made, potential)`.
    fn run_recipe(&mut self, recipe: &RecipeDef, here: CarriageId, work: f32) -> (f32, f32) {
        let p: &SimParams = &self.params;
        let h = here.index();
        let item = recipe.output;
        let potential = work * recipe.rate(p);
        let stock = &self.carriages[h].stock;
        let cap = self.catalog.storage_cap(p, self.carriages[h].kind, item);
        let space = (cap - stock.get(item)).max(0.0);
        let mut made = potential.min(space);
        let mut needs: Vec<(ItemKind, f32, Vec<usize>)> = Vec::with_capacity(recipe.inputs.len());
        for input in recipe.inputs.iter() {
            if made <= 0.0 {
                break;
            }
            let per = recipe.per_output(input, p);
            if !per.is_finite() {
                made = 0.0;
                break;
            }
            if per <= 0.0 {
                continue;
            }
            let sources = match input.from {
                Source::Here => vec![h],
                Source::Nearest(kind) => self.nearest_of_kind(kind, here),
            };
            let available: f32 = sources
                .iter()
                .map(|&s| self.carriages[s].stock.get(input.item))
                .sum();
            made = made.min(available / per);
            needs.push((input.item, per, sources));
        }
        if made <= 0.0 {
            return (0.0, potential);
        }
        for (input, per, sources) in needs {
            let mut wanted = made * per;
            for s in sources {
                if wanted <= 0.0 {
                    break;
                }
                wanted -= self.carriages[s].stock.take(input, wanted);
            }
        }
        self.carriages[h].stock.add(item, made, f32::INFINITY);
        (made, potential)
    }

    /// Brings the goods sold at the Mercati from the nearest carriages of
    /// `from` to the Mercato `here`, the item it has less of first, up to
    /// `budget` units. Returns the units moved.
    fn trade(&mut self, from: CarriageKind, budget: f32, here: CarriageId) -> f32 {
        let kind = self.carriages[here.index()].kind;
        let mut left = budget;
        let mut items = self.catalog.sold_items();
        items.sort_by(|a, b| {
            let stock = &self.carriages[here.index()].stock;
            stock.get(*a).total_cmp(&stock.get(*b))
        });
        let sources = self.nearest_of_kind(from, here);
        for item in items {
            let cap = self.catalog.storage_cap(&self.params, kind, item);
            for &o in &sources {
                let space = cap - self.carriages[here.index()].stock.get(item);
                let wanted = left.min(space);
                if wanted <= 0.0 {
                    break;
                }
                let taken = self.carriages[o].stock.take(item, wanted);
                self.carriages[here.index()].stock.add(item, taken, cap);
                left -= taken;
            }
        }
        budget - left
    }

    /// Indices of carriages of `kind`, nearest to `here` first.
    pub(super) fn nearest_of_kind(&self, kind: CarriageKind, here: CarriageId) -> Vec<usize> {
        let mut found: Vec<usize> = self
            .carriages
            .iter()
            .filter(|c| c.kind == kind)
            .map(|c| c.id.index())
            .collect();
        found.sort_by_key(|&c| (CarriageId(c as u16).distance(here), c));
        found
    }
}
