//! Oggetti e magazzini.
//!
//! Items are stored in bulk as fractional amounts per [`ItemKind`] in a
//! [`Stock`] (carriage storage); only whole units can be eaten or sold, so a
//! renderer should show [`Stock::count`]. Durable items (Attrezzi, Vestiti)
//! owned by an NPC live in [`crate::Inventory`] with a durability instead.
//!
//! **Kinds are data.** An [`ItemKind`] is the item's stable id in the
//! world's [`crate::Catalog`] (the 13 builtin items first, then the ones the
//! Custode adds during a game, see [`crate::custode`]) plus a handle to what
//! never changes about it, its [`ItemInfo`]: names, what it is for
//! ([`ItemUse`]) and how many stack in a slot. So an item can be named and
//! stacked anywhere, without the world at hand; every other property (value,
//! storage, outlet, spoilage, recipes…) is read from the world's catalog
//! (`world.catalog().item(kind)`). Two kinds are equal when their ids are.
//!
//! The builtin items keep their old names as associated constants
//! (`ItemKind::Verdura`), and serialize as before in human-readable formats
//! (`"Verdura"`); a new item serializes with its id and its [`ItemInfo`], so
//! it can be read back on its own.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::carriage::CarriageKind;
use crate::defs::ItemUse;
use crate::defs::items::{BUILTIN_CODES, BUILTIN_INFOS, DEFAULT_STACK_LIMIT};

/// What never changes about a kind of item (see the module docs).
#[derive(Debug, PartialEq, Eq)]
pub struct ItemInfo {
    /// Stable key: "verdura" for a builtin item, the normalized name for a
    /// new one ("filtro d acqua").
    pub key: &'static str,
    /// Singular, lowercase: "attrezzo".
    pub name: &'static str,
    /// Plural, lowercase: "attrezzi".
    pub plural: &'static str,
    /// With the indefinite article: "un attrezzo".
    pub with_article: &'static str,
    pub usage: ItemUse,
    /// Carriages where people get it: a shortage means none of them has a
    /// whole unit left.
    pub outlet: CarriageKind,
    /// Most units in one inventory slot; None: [`DEFAULT_STACK_LIMIT`].
    pub stack_limit: Option<u32>,
}

/// An [`ItemInfo`] with owned strings: how a new item's info is built and
/// serialized.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemInfoData {
    pub key: String,
    pub name: String,
    pub plural: String,
    pub with_article: String,
    pub usage: ItemUse,
    pub outlet: CarriageKind,
    #[serde(default)]
    pub stack_limit: Option<u32>,
}

impl ItemInfoData {
    fn of(info: &ItemInfo) -> ItemInfoData {
        ItemInfoData {
            key: info.key.to_string(),
            name: info.name.to_string(),
            plural: info.plural.to_string(),
            with_article: info.with_article.to_string(),
            usage: info.usage,
            outlet: info.outlet,
            stack_limit: info.stack_limit,
        }
    }

    fn matches(&self, info: &ItemInfo) -> bool {
        self.key == info.key
            && self.name == info.name
            && self.plural == info.plural
            && self.with_article == info.with_article
            && self.usage == info.usage
            && self.outlet == info.outlet
            && self.stack_limit == info.stack_limit
    }
}

/// Infos of the items created during this process, shared by content: the
/// same info is leaked once, however many worlds or saves use it.
static INTERNED: Mutex<Vec<&'static ItemInfo>> = Mutex::new(Vec::new());

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

/// The shared `&'static` copy of `data`.
fn intern(data: &ItemInfoData) -> &'static ItemInfo {
    let mut interned = INTERNED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = interned.iter().find(|i| data.matches(i)) {
        return found;
    }
    let info: &'static ItemInfo = Box::leak(Box::new(ItemInfo {
        key: leak(&data.key),
        name: leak(&data.name),
        plural: leak(&data.plural),
        with_article: leak(&data.with_article),
        usage: data.usage,
        outlet: data.outlet,
        stack_limit: data.stack_limit,
    }));
    interned.push(info);
    info
}

/// A kind of item: its id in the catalog and its [`ItemInfo`].
#[derive(Clone, Copy)]
pub struct ItemKind {
    id: u16,
    info: &'static ItemInfo,
}

#[allow(non_upper_case_globals)]
impl ItemKind {
    /// Raw crop grown by Contadini in the Serre.
    pub const Verdura: ItemKind = ItemKind::builtin(0);
    /// Cooked food portion made by Cuochi in the Mense; one per meal, free.
    pub const Razione: ItemKind = ItemKind::builtin(1);
    /// Scrap: the train sheds it steadily into the Officine.
    pub const Rottame: ItemKind = ItemKind::builtin(2);
    /// Tool made by Operai from Rottame: boosts Contadini and Operai, wears with work.
    pub const Attrezzo: ItemKind = ItemKind::builtin(3);
    /// Clothes sewn by Operai from Tessuto: slow down tiredness, wear daily.
    pub const Vestito: ItemKind = ItemKind::builtin(4);
    /// Raw fibre grown by Contadini in the Serre: woven into Tessuto.
    pub const Cotone: ItemKind = ItemKind::builtin(5);
    /// Herbs grown by Contadini in the Serre: brewed into Tè.
    pub const Erbe: ItemKind = ItemKind::builtin(6);
    /// Worked metal, cast by Operai from Rottame: for Attrezzi, Lampade, Giocattoli.
    pub const Metallo: ItemKind = ItemKind::builtin(7);
    /// Cloth woven by Operai from Cotone: for Vestiti, Coperte, Giocattoli.
    pub const Tessuto: ItemKind = ItemKind::builtin(8);
    /// A pot of tea brewed by Cuochi from Erbe, served with the meals.
    pub const Te: ItemKind = ItemKind::builtin(9);
    /// Blanket: kept in the Dormitori, residents sleep better.
    pub const Coperta: ItemKind = ItemKind::builtin(10);
    /// Lamp: kept in the Dormitori, residents at home feel less lonely.
    pub const Lampada: ItemKind = ItemKind::builtin(11);
    /// Toy: kept in the Dormitori, children at home feel less lonely.
    pub const Giocattolo: ItemKind = ItemKind::builtin(12);

    /// How many builtin items there are (ids `0..BUILTIN_COUNT`).
    pub const BUILTIN_COUNT: usize = 13;
    /// The builtin items, in id order. A world may have more: iterate its
    /// catalog ([`crate::Catalog::kinds`]) to see them all.
    pub const BUILTIN: [ItemKind; Self::BUILTIN_COUNT] = [
        ItemKind::Verdura,
        ItemKind::Razione,
        ItemKind::Rottame,
        ItemKind::Attrezzo,
        ItemKind::Vestito,
        ItemKind::Cotone,
        ItemKind::Erbe,
        ItemKind::Metallo,
        ItemKind::Tessuto,
        ItemKind::Te,
        ItemKind::Coperta,
        ItemKind::Lampada,
        ItemKind::Giocattolo,
    ];

    const fn builtin(id: u16) -> ItemKind {
        ItemKind {
            id,
            info: &BUILTIN_INFOS[id as usize],
        }
    }

    /// A new kind with id `id` (at least [`ItemKind::BUILTIN_COUNT`]).
    pub(crate) fn new(id: u16, info: &ItemInfoData) -> ItemKind {
        debug_assert!(usize::from(id) >= Self::BUILTIN_COUNT);
        ItemKind {
            id,
            info: intern(info),
        }
    }

    /// Stable id: the position in the world's catalog.
    pub fn id(self) -> u16 {
        self.id
    }

    /// Position in the catalog (and in per-item vectors, like [`Stock`]).
    pub fn index(self) -> usize {
        usize::from(self.id)
    }

    /// Whether it is one of the 13 items every world starts with.
    pub fn is_builtin(self) -> bool {
        self.index() < Self::BUILTIN_COUNT
    }

    /// Names and nature (see [`ItemInfo`]).
    pub fn info(self) -> &'static ItemInfo {
        self.info
    }

    /// Stable key: "verdura", "filtro d acqua".
    pub fn key(self) -> &'static str {
        self.info.key
    }

    /// Code of a builtin item as in the saves and in `Debug`: "Verdura";
    /// the key for a new one.
    pub fn code(self) -> &'static str {
        BUILTIN_CODES
            .get(self.index())
            .copied()
            .unwrap_or(self.info.key)
    }

    /// Singular, lowercase: "attrezzo".
    pub fn name(self) -> &'static str {
        self.info.name
    }

    /// Plural, lowercase: "attrezzi".
    pub fn plural(self) -> &'static str {
        self.info.plural
    }

    /// With the indefinite article: "un attrezzo", "una razione".
    pub fn with_article(self) -> &'static str {
        self.info.with_article
    }

    /// What it is for: who wants it and where an owned unit is kept.
    pub fn usage(self) -> ItemUse {
        self.info.usage
    }

    /// Carriages where the item is made available to people: a shortage
    /// means none of them has a whole unit left.
    pub fn outlet(self) -> CarriageKind {
        self.info.outlet
    }

    /// Whether an owned unit wears out (tracked per NPC in [`crate::Inventory`]).
    pub fn has_durability(self) -> bool {
        self.info.usage.is_owned()
    }

    /// Most units one slot of a [`crate::SlotInventory`] holds
    /// ([`ItemInfo::stack_limit`], at least 1).
    pub fn stack_size(self) -> u32 {
        self.info.stack_limit.unwrap_or(DEFAULT_STACK_LIMIT).max(1)
    }

    /// The builtin item with this code ("Verdura"), if any.
    pub fn from_code(code: &str) -> Option<ItemKind> {
        BUILTIN_CODES
            .iter()
            .position(|&c| c == code)
            .map(|i| ItemKind::BUILTIN[i])
    }
}

impl PartialEq for ItemKind {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for ItemKind {}

impl Hash for ItemKind {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl PartialOrd for ItemKind {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ItemKind {
    fn cmp(&self, other: &Self) -> Ordering {
        self.id.cmp(&other.id)
    }
}

impl fmt::Debug for ItemKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_builtin() {
            f.write_str(self.code())
        } else {
            write!(f, "Nuovo({}#{})", self.info.key, self.id)
        }
    }
}

impl fmt::Display for ItemKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// How a kind travels: its id, and for a new item its info.
#[derive(Serialize, Deserialize)]
struct Wire {
    id: u16,
    #[serde(default)]
    info: Option<ItemInfoData>,
}

impl Wire {
    fn of(kind: ItemKind) -> Wire {
        Wire {
            id: kind.id,
            info: (!kind.is_builtin()).then(|| ItemInfoData::of(kind.info)),
        }
    }

    fn kind<E: serde::de::Error>(self) -> Result<ItemKind, E> {
        if usize::from(self.id) < ItemKind::BUILTIN_COUNT {
            return Ok(ItemKind::builtin(self.id));
        }
        match self.info {
            Some(info) => Ok(ItemKind::new(self.id, &info)),
            None => Err(E::custom(format!("oggetto {} senza descrizione", self.id))),
        }
    }
}

impl Serialize for ItemKind {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if s.is_human_readable() && self.is_builtin() {
            s.serialize_str(self.code())
        } else {
            Wire::of(*self).serialize(s)
        }
    }
}

impl<'de> Deserialize<'de> for ItemKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        if !d.is_human_readable() {
            return Wire::deserialize(d)?.kind();
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Human {
            Code(String),
            Wire(Wire),
        }
        match Human::deserialize(d)? {
            Human::Code(code) => ItemKind::from_code(&code)
                .ok_or_else(|| D::Error::custom(format!("oggetto sconosciuto «{code}»"))),
            Human::Wire(w) => w.kind(),
        }
    }
}

/// Bulk amounts per item kind (a carriage's storage, or a sum of them),
/// indexed by [`ItemKind::index`]. It grows with the catalog: an item past
/// its end has 0.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stock(Vec<f32>);

impl Default for Stock {
    fn default() -> Self {
        Stock(vec![0.0; ItemKind::BUILTIN_COUNT])
    }
}

impl PartialEq for Stock {
    /// Equal amounts for every item (missing ones count as 0).
    fn eq(&self, other: &Self) -> bool {
        let n = self.0.len().max(other.0.len());
        (0..n).all(|i| self.at(i) == other.at(i))
    }
}

impl Stock {
    fn at(&self, i: usize) -> f32 {
        self.0.get(i).copied().unwrap_or(0.0)
    }

    fn slot(&mut self, item: ItemKind) -> &mut f32 {
        let i = item.index();
        if i >= self.0.len() {
            self.0.resize(i + 1, 0.0);
        }
        &mut self.0[i]
    }

    /// Exact (possibly fractional) amount.
    pub fn get(&self, item: ItemKind) -> f32 {
        self.at(item.index())
    }

    /// Whole units available (what can be eaten, sold, drawn).
    pub fn count(&self, item: ItemKind) -> u32 {
        self.get(item).max(0.0).floor() as u32
    }

    pub fn has(&self, item: ItemKind, amount: f32) -> bool {
        self.get(item) >= amount
    }

    pub fn set(&mut self, item: ItemKind, amount: f32) {
        *self.slot(item) = amount.max(0.0);
    }

    /// Adds up to `amount` without exceeding `cap`; returns what was added.
    pub fn add(&mut self, item: ItemKind, amount: f32, cap: f32) -> f32 {
        let slot = self.slot(item);
        let added = amount.min(cap - *slot).max(0.0);
        *slot += added;
        added
    }

    /// Removes up to `amount`; returns what was taken.
    pub fn take(&mut self, item: ItemKind, amount: f32) -> f32 {
        if item.index() >= self.0.len() {
            return 0.0;
        }
        let slot = self.slot(item);
        let taken = amount.min(*slot).max(0.0);
        *slot -= taken;
        taken
    }

    /// `(item index, amount)` for every slot, in id order (see
    /// [`crate::Catalog::kinds`] for the kinds).
    pub fn amounts(&self) -> impl Iterator<Item = (usize, f32)> + '_ {
        self.0.iter().copied().enumerate()
    }

    /// `(item, amount)` for the builtin items, in id order.
    pub fn iter(&self) -> impl Iterator<Item = (ItemKind, f32)> + '_ {
        ItemKind::BUILTIN.into_iter().map(|k| (k, self.get(k)))
    }

    /// No whole unit of anything.
    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|&a| a.max(0.0).floor() == 0.0)
    }

    /// Adds `other` to `self` (no cap).
    pub fn merge(&mut self, other: &Stock) {
        if other.0.len() > self.0.len() {
            self.0.resize(other.0.len(), 0.0);
        }
        for (a, b) in self.0.iter_mut().zip(other.0.iter()) {
            *a += b;
        }
    }
}

impl fmt::Display for Stock {
    /// Whole units, e.g. "120 verdure, 1 attrezzo" (or "vuoto"). Items the
    /// Custode added are counted at the end ("3 altri oggetti"): their names
    /// are in the world's catalog.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for item in ItemKind::BUILTIN {
            let n = self.count(item);
            if n == 0 {
                continue;
            }
            if !first {
                f.write_str(", ")?;
            }
            first = false;
            let name = if n == 1 { item.name() } else { item.plural() };
            write!(f, "{n} {name}")?;
        }
        let others: u32 = self
            .0
            .iter()
            .skip(ItemKind::BUILTIN_COUNT)
            .map(|a| a.max(0.0).floor() as u32)
            .sum();
        if others > 0 {
            if !first {
                f.write_str(", ")?;
            }
            first = false;
            write!(f, "{others} altri oggetti")?;
        }
        if first {
            f.write_str("vuoto")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_add_take_respect_bounds() {
        let mut s = Stock::default();
        assert_eq!(s.add(ItemKind::Verdura, 5.5, 4.0), 4.0);
        assert_eq!(s.add(ItemKind::Verdura, 1.0, 4.0), 0.0);
        assert_eq!(s.take(ItemKind::Verdura, 10.0), 4.0);
        assert_eq!(s.get(ItemKind::Verdura), 0.0);
        s.add(ItemKind::Attrezzo, 2.7, 10.0);
        s.add(ItemKind::Razione, 1.0, 10.0);
        assert_eq!(s.count(ItemKind::Attrezzo), 2);
        assert_eq!(s.to_string(), "1 razione, 2 attrezzi");
        assert_eq!(Stock::default().to_string(), "vuoto");
    }

    fn sapone() -> ItemInfoData {
        ItemInfoData {
            key: "sapone".into(),
            name: "sapone".into(),
            plural: "saponi".into(),
            with_article: "un sapone".into(),
            usage: ItemUse::Material,
            outlet: CarriageKind::Mercato,
            stack_limit: Some(5),
        }
    }

    #[test]
    fn new_kinds_grow_the_stock_and_travel_alone() {
        let soap = ItemKind::new(13, &sapone());
        assert_eq!(soap.name(), "sapone");
        assert_eq!(soap.stack_size(), 5);
        assert_ne!(soap, ItemKind::Giocattolo);
        let mut s = Stock::default();
        assert_eq!(s.get(soap), 0.0);
        assert_eq!(s.take(soap, 1.0), 0.0);
        s.add(soap, 3.0, 10.0);
        assert_eq!(s.count(soap), 3);
        assert_eq!(s.to_string(), "3 altri oggetti");
        let mut empty = Stock::default();
        assert_ne!(s, empty);
        empty.add(soap, 3.0, 10.0);
        assert_eq!(s, empty);
        // The same content is shared, and a kind reads back on its own.
        assert!(std::ptr::eq(
            soap.info(),
            ItemKind::new(13, &sapone()).info()
        ));
        let json = serde_json::to_string(&[ItemKind::Te, soap]).unwrap();
        assert!(json.starts_with("[\"Te\",{\"id\":13"), "{json}");
        let back: Vec<ItemKind> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, [ItemKind::Te, soap]);
        assert_eq!(back[1].plural(), "saponi");
        assert_eq!(format!("{:?}", ItemKind::Te), "Te");
    }
}
