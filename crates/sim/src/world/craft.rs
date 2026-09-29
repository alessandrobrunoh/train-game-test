//! Ricette: la produzione degli NPC e il crafting del giocatore.
//!
//! NPC workers and the player use the same rows of [`RECIPES`]:
//! - `World::produce`: `minutes` of work make `rate × minutes × tool bonus`
//!   of the output of one of the job's recipes (see [`Work`] for which one),
//!   bounded by the storage of the workplace and by the inputs, taken from
//!   their [`Source`] (nearest carriages first).
//! - [`World::player_craft`]: one whole batch from the player's inventory,
//!   in a carriage that has the recipe's station. It is instant in the sim:
//!   the game makes the player wait the recipe's minutes first.
//!
//! [`RECIPES`]: crate::defs::RECIPES

use std::fmt;

use super::World;
use crate::carriage::{CarriageKind, StationKind};
use crate::defs::{RECIPES, RecipeDef, Source, Work};
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
        recipe: &RecipeDef,
        carriage: CarriageId,
    ) -> Result<u32, CraftError> {
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
    /// [`RECIPES`] order.
    pub fn recipes_at(&self, carriage: CarriageId) -> Vec<&'static RecipeDef> {
        let Some(c) = self.carriage(carriage) else {
            return Vec::new();
        };
        RECIPES
            .iter()
            .filter(|r| c.stations.iter().any(|s| s.kind == r.station))
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
                cap += self.params.storage_cap(c.kind, item);
            }
        }
        if cap > 0.0 { stock / cap } else { 1.0 }
    }

    /// Effect of `minutes` of work by a `job` in carriage `here`; `bonus`
    /// multiplies the output (tool). Booked in [`super::EconomyCounters`],
    /// with the share of the work that produced nothing. Returns what was
    /// made and how much, if anything (moving goods makes nothing).
    pub(super) fn produce(
        &mut self,
        job: Job,
        here: CarriageId,
        minutes: f32,
        bonus: f32,
    ) -> Option<(ItemKind, f32)> {
        let mut output = None;
        let wasted = match &job.def().work {
            Work::Trade { from, rate } => {
                let potential = minutes * rate(&self.params);
                let moved = self.trade(*from, potential, here);
                1.0 - share(moved, potential)
            }
            work => {
                let order = self.recipe_order(work, job, here);
                // Specialties (see `market.rs`) only shape the "make the
                // scarcest" work; staples (food) are never slowed down.
                let specialized = matches!(work, Work::MakeScarcest(_));
                let work = minutes * bonus;
                let mut booked = None;
                for recipe in &order {
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
                    let first = order.first()?;
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

    /// The recipes of `work` in the order a worker in `here` tries them.
    fn recipe_order(&self, work: &Work, job: Job, here: CarriageId) -> Vec<&'static RecipeDef> {
        match work {
            Work::Make(recipe) => vec![*recipe],
            Work::MakeScarcest(recipes) => {
                // The carriage's specialties first, each group scarcest first.
                let mut order = self.by_scarcity(recipes, job);
                let specialties = self.market.specialties_of(here);
                if !specialties.is_empty() {
                    order.sort_by_key(|r| !specialties.contains(&r.output));
                }
                order
            }
            Work::MakeStaple { recipes, keep } => {
                let Some(staple) = recipes.first() else {
                    return Vec::new();
                };
                let c = &self.carriages[here.index()];
                let cap = self.params.storage_cap(c.kind, staple.output);
                let fill = if cap > 0.0 {
                    c.stock.get(staple.output) / cap
                } else {
                    1.0
                };
                if fill < keep(&self.params) {
                    // The staple first; the rest only if it can't be made.
                    recipes.iter().collect()
                } else {
                    self.by_scarcity(recipes, job)
                }
            }
            Work::Trade { .. } => Vec::new(),
        }
    }

    /// Output multiplier of `item` made in carriage `here`: a bonus for its
    /// specialties, a penalty for the rest (1 if it has none).
    fn specialty_factor(&self, here: CarriageId, item: ItemKind) -> f32 {
        let p = &self.params;
        match self.market.specialties_of(here) {
            [] => 1.0,
            s if s.contains(&item) => p.specialty_output_bonus.max(0.0),
            _ => p.off_specialty_output.max(0.0),
        }
    }

    /// `recipes` with the scarcest output on the train first (stable on ties).
    fn by_scarcity(&self, recipes: &'static [RecipeDef], job: Job) -> Vec<&'static RecipeDef> {
        let workplace = job.def().workplace;
        let mut order: Vec<(f32, &'static RecipeDef)> = recipes
            .iter()
            .map(|r| (self.fill_on_train(r.output, workplace), r))
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
        let space = (p.storage_cap(self.carriages[h].kind, item) - stock.get(item)).max(0.0);
        let mut made = potential.min(space);
        let mut needs: Vec<(ItemKind, f32, Vec<usize>)> = Vec::with_capacity(recipe.inputs.len());
        for input in recipe.inputs {
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
        let mut items = ItemKind::SOLD;
        items.sort_by(|a, b| {
            let stock = &self.carriages[here.index()].stock;
            stock.get(*a).total_cmp(&stock.get(*b))
        });
        let sources = self.nearest_of_kind(from, here);
        for item in items {
            let cap = self.params.storage_cap(kind, item);
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
