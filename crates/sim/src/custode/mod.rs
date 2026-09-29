//! Il Custode: controlla le proposte del Narratore e le applica al mondo.
//!
//! **L'AI propone, il motore decide** (see "Direzione nuova" in
//! `docs/piano-vita-ed-economia.md`). The Narratore (crate `narrator`) writes
//! [`Draft`]s: a [`Proposal`] (a new item, recipe or job, an event, a
//! statistic) with its reason and optionally a [`Panel`] and an item's
//! [`Appearance`]. Everything refers to items, recipes, jobs and kinds of
//! carriage **by name**; the Custode resolves the names against the world's
//! live [`crate::Catalog`] (case, accents and plurals aside, see
//! [`normalize`]) and checks the meaning:
//!
//! - [`crate::World::review`] → a [`Plan`] (everything resolved, with the
//!   defaults for what the draft leaves out) or a [`Rejection`] with an
//!   Italian reason;
//! - [`crate::World::apply`] → reviews again and makes it real: new items,
//!   recipes and jobs join the catalog (ids after the existing ones), a new
//!   job gets stations in its carriages, an event changes stocks and needs
//!   once, a statistic joins [`crate::World::statistics`]. The outcome is
//!   recorded in the World ([`crate::World::decisions`],
//!   [`crate::World::applied`]), so a save alone replays the game;
//! - [`crate::World::schedule`] → applies a draft at a fixed game minute
//!   (the game uses the next full hour), inside `World::tick`: a replay
//!   applies it at the same minute.
//!
//! What becomes real, and how, is in `world/custode.rs`.

pub mod appearance;
pub mod names;
pub mod panel;
pub mod proposal;
pub mod sources;
pub mod statistic;

use std::collections::{BTreeMap, VecDeque};
use std::fmt;

use serde::{Deserialize, Serialize};

pub use appearance::Appearance;
pub use names::normalize;
pub use panel::Panel;
pub use proposal::{Category, Draft, Effect, Ingredient, Need, Proposal};
pub use statistic::{StatBook, Statistic};

use crate::carriage::{CarriageKind, StationKind};
use crate::defs::{ItemCategory, ItemUse, RecipeId, Source, Work};
use crate::item::{ItemInfoData, ItemKind};
use crate::job::JobInfoData;
use crate::npc::Job;
use crate::time::GameTime;

/// Why the Custode refused a proposal, in Italian.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejection(pub String);

impl Rejection {
    pub fn new(reason: impl Into<String>) -> Rejection {
        Rejection(reason.into())
    }
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Rejection {}

// --- Plan -----------------------------------------------------------------------------

/// What applying a proposal would do, everything resolved. Short-lived:
/// its size doesn't matter.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    /// A new item and the recipe that makes it.
    Item {
        info: ItemInfoData,
        item: ItemPlan,
        recipe: RecipePlan,
    },
    /// A new recipe (for an existing item).
    Recipe(RecipePlan),
    Job(JobPlan),
    Event(EventPlan),
    Statistic(Statistic),
}

/// The properties of a new item (see [`crate::ItemDef`]).
#[derive(Clone, Debug, PartialEq)]
pub struct ItemPlan {
    pub description: String,
    pub base_value: u32,
    pub category: ItemCategory,
    /// Storage per kind of carriage.
    pub stores: Vec<(CarriageKind, f32)>,
    pub spoilage: Option<f32>,
    /// Food: satiety, energy, sociality restored by one unit.
    pub consume: Option<(f32, f32, f32)>,
    /// Wanted by who can afford it (a durable good).
    pub desired: bool,
    /// Brought to the Mercati by the Mercanti and sold on their shelves.
    pub sold: bool,
    pub appearance: Option<Appearance>,
}

/// What a recipe makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Existing(ItemKind),
    /// The new item of the same plan.
    New,
}

/// A new recipe (see [`crate::RecipeDef`]).
#[derive(Clone, Debug, PartialEq)]
pub struct RecipePlan {
    pub key: String,
    pub name: String,
    pub output: Output,
    pub batch: u32,
    pub minutes: f32,
    /// Item, units per batch, where the workers take it.
    pub inputs: Vec<(ItemKind, u32, Source)>,
    pub station: StationKind,
    /// The job whose workers make it.
    pub maker: Job,
    /// The maker's workplace kind has to start storing an item it makes.
    pub store_output: Option<(CarriageKind, f32)>,
}

/// A new job (see [`crate::JobDef`]).
#[derive(Clone, Debug, PartialEq)]
pub struct JobPlan {
    pub info: JobInfoData,
    pub description: String,
    pub work: WorkPlan,
    /// Workers wanted per carriage of the workplace kind.
    pub staff: u16,
    /// NPCs one of its stations holds.
    pub station_capacity: u16,
}

#[derive(Clone, Debug, PartialEq)]
pub enum WorkPlan {
    /// These recipes (the scarcest output first when more than one).
    Make(Vec<RecipeId>),
    /// A service (see [`Work::Service`]).
    Service { need: Need, per_minute: f32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct EventPlan {
    pub title: String,
    pub description: String,
    pub effects: Vec<EventEffect>,
}

/// One effect of an event, resolved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EventEffect {
    /// Units added (or removed) over the carriages of a kind.
    Stock {
        kind: CarriageKind,
        item: ItemKind,
        delta: i32,
    },
    /// A need of the people in the carriages of a kind (everyone: None).
    Need {
        kind: Option<CarriageKind>,
        need: Need,
        delta: f32,
    },
}

// --- Records --------------------------------------------------------------------------

/// What a proposal became.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Applied {
    #[serde(default)]
    pub item: Option<ItemKind>,
    #[serde(default)]
    pub recipe: Option<RecipeId>,
    #[serde(default)]
    pub job: Option<Job>,
    /// Key of a new statistic.
    #[serde(default)]
    pub statistic: Option<String>,
    /// What changed, one Italian sentence ("Nuovo oggetto «borraccia», lo
    /// fanno gli operai").
    pub summary: String,
}

/// A proposal the Custode decided on, at a game minute.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    /// Order of arrival ([`crate::World::schedule`] returns it).
    pub seq: u32,
    /// When it was applied (or refused).
    pub at: GameTime,
    pub draft: Draft,
    pub result: Result<Applied, Rejection>,
}

impl Decision {
    pub fn applied(&self) -> Option<&Applied> {
        self.result.as_ref().ok()
    }
}

/// A draft waiting for its minute.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scheduled {
    pub seq: u32,
    pub at: GameTime,
    pub draft: Draft,
}

/// Samples kept per statistic (10 days of hours).
pub const STAT_HISTORY: usize = 240;

/// The statistics of the world and their history (see
/// [`crate::World::statistics`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Statistics {
    #[serde(default)]
    stats: Vec<Statistic>,
    /// Key → (game hour, value), oldest first.
    #[serde(default)]
    history: BTreeMap<String, VecDeque<(u64, f32)>>,
    #[serde(default)]
    last_hour: Option<u64>,
    /// Rebuilt from `stats`.
    #[serde(skip)]
    book: Option<StatBook>,
}

impl Statistics {
    /// The statistics, as a book that evaluates them (read-only).
    pub fn book(&self) -> StatBook {
        match &self.book {
            Some(b) => b.clone(),
            None => {
                let mut book = StatBook::new();
                for s in &self.stats {
                    book.add(s.clone());
                }
                book
            }
        }
    }

    /// In the order they were added.
    pub fn iter(&self) -> impl Iterator<Item = &Statistic> {
        self.stats.iter()
    }

    pub fn len(&self) -> usize {
        self.stats.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stats.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&Statistic> {
        let key = normalize(name);
        self.stats.iter().find(|s| normalize(&s.name) == key)
    }

    /// The history of the statistic called `name`: (game hour, value),
    /// oldest first.
    pub fn history(&self, name: &str) -> impl Iterator<Item = (u64, f32)> + '_ {
        self.history
            .get(&normalize(name))
            .into_iter()
            .flatten()
            .copied()
    }

    pub(crate) fn add(&mut self, stat: Statistic) {
        self.stats.push(stat);
        self.book = None;
    }

    /// Samples every statistic once per game hour; true if it did.
    pub(crate) fn sample(&mut self, world: &crate::World) -> bool {
        let hour = world.clock.minutes() / crate::MINUTES_PER_HOUR;
        if self.stats.is_empty() || self.last_hour == Some(hour) {
            return false;
        }
        self.last_hour = Some(hour);
        let book = self.book();
        for s in &self.stats {
            let Some(v) = book.read(&s.name, world).value() else {
                continue;
            };
            let h = self.history.entry(normalize(&s.name)).or_default();
            h.push_back((hour, v));
            while h.len() > STAT_HISTORY {
                h.pop_front();
            }
        }
        self.book = Some(book);
        true
    }

    /// Replaces the history of `name` (a save of the game from before the
    /// statistics lived in the World).
    pub fn set_history(&mut self, name: &str, samples: impl IntoIterator<Item = (u64, f32)>) {
        let h: VecDeque<(u64, f32)> = samples.into_iter().collect();
        self.history.insert(normalize(name), h);
    }
}

/// The Custode's part of the World: what waits, what was decided, the
/// statistics. Saved as JSON text (the proposals are JSON-shaped), so it
/// travels in any serde format.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct CustodeState {
    #[serde(default)]
    pub pending: Vec<Scheduled>,
    #[serde(default)]
    pub decisions: Vec<Decision>,
    #[serde(default)]
    pub statistics: Statistics,
    #[serde(default)]
    pub next_seq: u32,
}

/// Serializes a value as a JSON string (for JSON-shaped types inside
/// formats that don't describe themselves, like the saves' postcard).
pub(crate) mod json_text {
    use serde::de::{DeserializeOwned, Error as _};
    use serde::ser::Error as _;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<T: Serialize, S: Serializer>(value: &T, s: S) -> Result<S::Ok, S::Error> {
        let text = serde_json::to_string(value).map_err(S::Error::custom)?;
        text.serialize(s)
    }

    pub fn deserialize<'de, T: DeserializeOwned, D: Deserializer<'de>>(
        d: D,
    ) -> Result<T, D::Error> {
        let text = String::deserialize(d)?;
        serde_json::from_str(&text).map_err(D::Error::custom)
    }
}

// --- Helpers ------------------------------------------------------------------------

/// Replaces the recipes a job's work makes.
pub(crate) fn set_recipes(work: &mut Work, recipes: Vec<RecipeId>) {
    match work {
        Work::MakeScarcest(list) | Work::MakeStaple { recipes: list, .. } => {
            *list = recipes.into();
        }
        Work::Make(_) => {
            *work = match recipes.as_slice() {
                [one] => Work::Make(*one),
                _ => Work::MakeScarcest(recipes.into()),
            }
        }
        Work::Trade { .. } | Work::Service { .. } => {}
    }
}

/// The usage of a new item of `category`.
pub(crate) fn usage_of(category: ItemCategory) -> ItemUse {
    match category {
        ItemCategory::Consumable => ItemUse::Food,
        ItemCategory::Raw | ItemCategory::Intermediate | ItemCategory::Durable => ItemUse::Material,
    }
}

/// The sim's category of a draft's.
pub(crate) fn category_of(c: Category) -> ItemCategory {
    match c {
        Category::Raw => ItemCategory::Raw,
        Category::Intermediate => ItemCategory::Intermediate,
        Category::Consumable => ItemCategory::Consumable,
        Category::Durable => ItemCategory::Durable,
    }
}
