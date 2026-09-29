//! NPC: bisogni, lavoro, inventario, età e relazioni.

use rand::RngExt;
use serde::{Deserialize, Serialize};

use crate::action::Action;
use crate::defs::ItemUse;
use crate::ids::{CarriageId, NpcId};
use crate::item::ItemKind;
use crate::personality::Personality;
use crate::player::{PlayerTie, Regard, SlotInventory};
use crate::time::{GameTime, MINUTES_PER_DAY};

/// Needs in `0..=1`, where 1 means fully satisfied.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Needs {
    pub hunger: f32,
    pub energy: f32,
    pub social: f32,
}

impl Needs {
    pub fn clamp(&mut self) {
        self.hunger = self.hunger.clamp(0.0, 1.0);
        self.energy = self.energy.clamp(0.0, 1.0);
        self.social = self.social.clamp(0.0, 1.0);
    }
}

impl Default for Needs {
    fn default() -> Self {
        Self {
            hunger: 1.0,
            energy: 1.0,
            social: 1.0,
        }
    }
}

pub use crate::job::Job;

/// What an NPC owns: tokens, the Attrezzo and Vestito it uses (each with a
/// durability in `(0, 1]`, removed when it reaches 0) and a few slots of
/// things it carries ([`Inventory::items`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Inventory {
    /// Wage tokens earned by working, spent at the Mercati.
    pub tokens: u32,
    /// Durability of the owned Attrezzo, if any.
    pub tool: Option<f32>,
    /// Durability of the owned Vestito, if any.
    pub clothes: Option<f32>,
    /// What the NPC carries, [`NPC_ITEM_SLOTS`] slots of any item from the
    /// catalog (new, whole units): a share of its own work, gifts from the
    /// player, purchases at the stalls. It eats its own food when hungry,
    /// brings comfort goods home and sells the rest at the Mercati's stalls
    /// (see `world/stalls.rs`). The equipped [`Inventory::tool`] and
    /// [`Inventory::clothes`] are not in here.
    #[serde(default = "npc_items")]
    pub items: SlotInventory,
}

/// Slots of an NPC's [`Inventory::items`].
pub const NPC_ITEM_SLOTS: usize = 6;

fn npc_items() -> SlotInventory {
    SlotInventory::new(NPC_ITEM_SLOTS)
}

impl Default for Inventory {
    fn default() -> Self {
        Self {
            tokens: 0,
            tool: None,
            clothes: None,
            items: npc_items(),
        }
    }
}

impl Inventory {
    /// Tokens, tool and clothes with empty [`Inventory::items`].
    pub fn new(tokens: u32, tool: Option<f32>, clothes: Option<f32>) -> Inventory {
        Inventory {
            tokens,
            tool,
            clothes,
            items: npc_items(),
        }
    }

    /// Durability of the owned unit of `item` (only Attrezzo and Vestito can be owned).
    pub fn durability(&self, item: ItemKind) -> Option<f32> {
        match item.usage() {
            ItemUse::Tool => self.tool,
            ItemUse::Clothes => self.clothes,
            ItemUse::Food | ItemUse::Material => None,
        }
    }

    pub fn has(&self, item: ItemKind) -> bool {
        self.durability(item).is_some()
    }

    pub(crate) fn slot_mut(&mut self, item: ItemKind) -> Option<&mut Option<f32>> {
        match item.usage() {
            ItemUse::Tool => Some(&mut self.tool),
            ItemUse::Clothes => Some(&mut self.clothes),
            ItemUse::Food | ItemUse::Material => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Npc {
    pub id: NpcId,
    /// Full name, "Nome Cognome" (first names never contain spaces, see [`Npc::surname`]).
    pub name: String,
    pub sex: Sex,
    /// Birth time in game minutes since day 1 00:00; negative for people
    /// born before the simulation started. The source of truth for the age.
    pub born: i64,
    /// Whole years of age, as of the last midnight (refreshed daily by the
    /// sim, exact at birth and generation). Cheap to read for renderers and
    /// stats; [`Npc::age_years`] gives the exact value at any time.
    pub age: u32,
    /// Carriage the NPC is in. While travelling it stays the origin until arrival.
    pub carriage: CarriageId,
    /// Storey of the carriage the NPC is on (0 = ground floor). Stations
    /// set it; travellers keep the floor they left from (they come down the
    /// stairs first, see [`crate::SimParams::stairs_minutes`]) and arrive on
    /// the ground floor.
    #[serde(default)]
    pub floor: u8,
    /// Dormitorio the NPC lives in.
    pub home: CarriageId,
    pub job: Option<Job>,
    /// Carriage where the NPC works (matches `job.workplace_kind()`).
    pub workplace: Option<CarriageId>,
    pub needs: Needs,
    pub inventory: Inventory,
    pub action: Action,
    pub action_since: GameTime,
    pub action_until: GameTime,
    /// Consecutive minutes spent with hunger at 0.
    pub starving_minutes: u64,
    /// Sparse social ties, at most [`MAX_RELATIONS`] entries of kind
    /// [`RelationKind::Friend`] plus family (partner, parents, children,
    /// siblings: never dropped while alive). Kept in insertion order; ties to
    /// dead NPCs are removed.
    #[serde(default)]
    pub relations: Vec<Relation>,
    /// Persistent character, drawn at birth (partly inherited).
    #[serde(default)]
    pub traits: Traits,
    /// Meal shift (turno mensa): when this NPC has breakfast, lunch (and its
    /// lunch break, if it works) and dinner. Households share one, balanced
    /// in each Dormitorio; newborns take their mother's. None: derived from
    /// the id (see [`crate::SimParams::meal_shift`]).
    #[serde(default)]
    pub meal_shift: Option<u8>,
    /// Personality tags (2–3, drawn at birth, see [`Personality`]): they
    /// give the tone of conversations. None (saves from before
    /// personalities): derived from the id, see [`Npc::personality`].
    #[serde(default)]
    pub personality: Option<Personality>,
    /// What the NPC thinks of the player (None: they never had to do with
    /// each other). Kept apart from [`Npc::relations`], which are between
    /// NPCs: see [`crate::player`].
    #[serde(default)]
    pub player: Option<PlayerTie>,
}

/// Character traits in `0..=1`, drawn at birth and never changed. They weigh
/// the built-in rules of the deliberations (see [`crate::Deliberation`]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Traits {
    /// High: rarely steals.
    pub honesty: f32,
    /// High: dares (protests, proposes, steals if dishonest); low: cautious.
    pub boldness: f32,
}

impl Default for Traits {
    fn default() -> Self {
        Self {
            honesty: 0.5,
            boldness: 0.5,
        }
    }
}

impl Traits {
    /// Random traits, bunched around 0.5 (mean of two uniform draws).
    pub fn random(rng: &mut impl rand::Rng) -> Traits {
        let mut draw = || (rng.random::<f32>() + rng.random::<f32>()) / 2.0;
        Traits {
            honesty: draw(),
            boldness: draw(),
        }
    }

    /// A child's traits: 60% the parents' average, 40% random.
    pub fn inherited(a: Traits, b: Traits, rng: &mut impl rand::Rng) -> Traits {
        let own = Traits::random(rng);
        let mix = |x: f32, y: f32, r: f32| (0.3 * (x + y) + 0.4 * r).clamp(0.0, 1.0);
        Traits {
            honesty: mix(a.honesty, b.honesty, own.honesty),
            boldness: mix(a.boldness, b.boldness, own.boldness),
        }
    }

    /// Short Italian description, e.g. "onesta e prudente".
    pub fn describe(&self, sex: Sex) -> String {
        let honesty = match self.honesty {
            h if h < 0.35 => sex.pick("poco scrupolosa", "poco scrupoloso"),
            h if h > 0.65 => sex.pick("molto onesta", "molto onesto"),
            _ => sex.pick("onesta", "onesto"),
        };
        let boldness = match self.boldness {
            b if b < 0.35 => "prudente",
            b if b > 0.65 => "audace",
            _ => sex.pick("riflessiva", "riflessivo"),
        };
        format!("{honesty} e {boldness}")
    }
}

impl Npc {
    /// Exact age in whole years at `now`, with years of `days_per_year` days.
    pub fn age_years(&self, now: GameTime, days_per_year: u32) -> u32 {
        let year = i64::from(days_per_year.max(1)) * MINUTES_PER_DAY as i64;
        ((now.0 as i64 - self.born).max(0) / year) as u32
    }

    /// Life stage from the cached [`Npc::age`].
    pub fn stage(&self) -> LifeStage {
        LifeStage::of_age(self.age)
    }

    /// Personality tags (derived from the id for NPCs saved without them).
    pub fn personality(&self) -> Personality {
        self.personality
            .unwrap_or_else(|| Personality::from_id(self.id, self.traits))
    }

    /// The surname: everything after the first space of `name`.
    pub fn surname(&self) -> &str {
        self.name
            .split_once(' ')
            .map_or(self.name.as_str(), |(_, last)| last)
    }

    /// The first name: `name` up to the first space.
    pub fn first_name(&self) -> &str {
        self.name
            .split_once(' ')
            .map_or(self.name.as_str(), |(first, _)| first)
    }

    /// The tie with `other`, if any.
    pub fn relation(&self, other: NpcId) -> Option<&Relation> {
        self.relations.iter().find(|r| r.other == other)
    }

    pub(crate) fn relation_mut(&mut self, other: NpcId) -> Option<&mut Relation> {
        self.relations.iter_mut().find(|r| r.other == other)
    }

    /// Affinity with `other` (0 without a tie).
    pub fn affinity(&self, other: NpcId) -> f32 {
        self.relation(other).map_or(0.0, |r| r.affinity)
    }

    /// Affinity with the player (0 if they never met).
    pub fn player_affinity(&self) -> f32 {
        self.player.map_or(0.0, |t| t.affinity)
    }

    /// How the NPC regards the player.
    pub fn regard(&self) -> Regard {
        Regard::of(self.player.as_ref())
    }

    /// Ties of a given kind.
    pub fn relations_of(&self, kind: RelationKind) -> impl Iterator<Item = &Relation> {
        self.relations.iter().filter(move |r| r.kind == kind)
    }

    pub fn partner(&self) -> Option<NpcId> {
        self.relations_of(RelationKind::Partner)
            .next()
            .map(|r| r.other)
    }

    /// Living children.
    pub fn children(&self) -> impl Iterator<Item = NpcId> + '_ {
        self.relations_of(RelationKind::Child).map(|r| r.other)
    }

    /// Living parents.
    pub fn parents(&self) -> impl Iterator<Item = NpcId> + '_ {
        self.relations_of(RelationKind::Parent).map(|r| r.other)
    }

    /// The non-family tie with the highest positive affinity.
    pub fn closest_friend(&self) -> Option<&Relation> {
        self.relations_of(RelationKind::Friend)
            .filter(|r| r.affinity > 0.0)
            .max_by(|a, b| a.affinity.total_cmp(&b.affinity))
    }

    /// Progress of the current action in `0..=1` (handy for animating travel).
    pub fn action_progress(&self, now: GameTime) -> f32 {
        let total = self.action_until.since(self.action_since);
        if total == 0 {
            1.0
        } else {
            (now.since(self.action_since) as f32 / total as f32).min(1.0)
        }
    }

    pub fn is_awake(&self) -> bool {
        !matches!(self.action, Action::Sleep(_))
    }

    /// Whether the NPC would buy `item` at a Mercato (tokens aside): a worker
    /// whose job uses tools without an Attrezzo, anyone without a Vestito.
    pub fn wants(&self, item: ItemKind) -> bool {
        match item.usage() {
            ItemUse::Tool => self.job.is_some_and(Job::uses_tool) && self.inventory.tool.is_none(),
            ItemUse::Clothes => self.inventory.clothes.is_none(),
            ItemUse::Food | ItemUse::Material => false,
        }
    }

    /// Whether the NPC accepts `item` from the player ([`crate::World::player_give`]):
    /// food unless nearly full, an Attrezzo or Vestito only if it [`Npc::wants`] it.
    pub fn accepts_gift(&self, item: ItemKind) -> bool {
        match item.usage() {
            ItemUse::Food => self.needs.hunger < GIFT_FULL_HUNGER,
            ItemUse::Material => false,
            ItemUse::Tool | ItemUse::Clothes => self.wants(item),
        }
    }
}

/// NPCs at least this full refuse food from the player.
pub const GIFT_FULL_HUNGER: f32 = 0.9;

/// Most [`RelationKind::Friend`] ties an NPC keeps (family ties are extra).
pub const MAX_RELATIONS: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Sex {
    Female,
    Male,
}

impl Sex {
    pub const ALL: [Sex; 2] = [Sex::Female, Sex::Male];

    /// Picks the Italian word form for this sex: `sex.pick("nata", "nato")`.
    pub fn pick<'a>(self, female: &'a str, male: &'a str) -> &'a str {
        match self {
            Sex::Female => female,
            Sex::Male => male,
        }
    }

    pub fn opposite(self) -> Sex {
        match self {
            Sex::Female => Sex::Male,
            Sex::Male => Sex::Female,
        }
    }

    /// "donna" / "uomo".
    pub fn name(self) -> &'static str {
        self.pick("donna", "uomo")
    }
}

/// Stage of life, from the age in years (see the `*_FROM` constants).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LifeStage {
    /// Under 14: no job.
    Bambino,
    /// 14-17: no job yet.
    Giovane,
    /// 18-64: works, can form a couple.
    Adulto,
    /// 65+: retired, gets the stipend.
    Anziano,
}

impl LifeStage {
    pub const ALL: [LifeStage; 4] = [
        LifeStage::Bambino,
        LifeStage::Giovane,
        LifeStage::Adulto,
        LifeStage::Anziano,
    ];
    pub const GIOVANE_FROM: u32 = 14;
    /// Coming of age: a job is assigned, couples can form.
    pub const ADULTO_FROM: u32 = 18;
    /// Retirement.
    pub const ANZIANO_FROM: u32 = 65;

    pub fn of_age(years: u32) -> LifeStage {
        match years {
            y if y < Self::GIOVANE_FROM => LifeStage::Bambino,
            y if y < Self::ADULTO_FROM => LifeStage::Giovane,
            y if y < Self::ANZIANO_FROM => LifeStage::Adulto,
            _ => LifeStage::Anziano,
        }
    }

    /// Position in [`LifeStage::ALL`].
    pub fn index(self) -> usize {
        self as usize
    }

    /// Lowercase Italian name, gendered: "bambina", "giovane", "adulto", "anziana".
    pub fn name(self, sex: Sex) -> &'static str {
        match self {
            LifeStage::Bambino => sex.pick("bambina", "bambino"),
            LifeStage::Giovane => "giovane",
            LifeStage::Adulto => sex.pick("adulta", "adulto"),
            LifeStage::Anziano => sex.pick("anziana", "anziano"),
        }
    }

    /// Whether NPCs at this stage hold a job.
    pub fn works(self) -> bool {
        self == LifeStage::Adulto
    }
}

/// What `other` is to the NPC holding the [`Relation`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RelationKind {
    /// Not family: an acquaintance or friend, depending on the affinity.
    Friend,
    Partner,
    /// `other` is a parent of the holder.
    Parent,
    /// `other` is a child of the holder.
    Child,
    Sibling,
}

impl RelationKind {
    pub fn is_family(self) -> bool {
        self != RelationKind::Friend
    }

    /// The kind seen from `other`'s side.
    pub fn inverse(self) -> RelationKind {
        match self {
            RelationKind::Parent => RelationKind::Child,
            RelationKind::Child => RelationKind::Parent,
            k => k,
        }
    }
}

/// A tie from one NPC to `other`. Ties are kept on both sides (friend ties
/// may be dropped on one side only when its list is full).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Relation {
    pub other: NpcId,
    pub kind: RelationKind,
    /// `-1..=1`: raised by chatting together, lowered by quarrels; friend ties
    /// fade slowly towards 0 and are forgotten near it.
    pub affinity: f32,
}
