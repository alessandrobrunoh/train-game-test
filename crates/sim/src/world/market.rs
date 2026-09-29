//! Mercato: specialità delle carrozze, prezzi per distanza dal produttore,
//! storico dei prezzi e vendite del giocatore.
//!
//! - **Specialties.** At generation every producing carriage specializes in
//!   1–2 of the recipes of its kind ([`CarriageKind::recipes`]): an Officina
//!   in Attrezzi, in Vestiti or in both
//!   ([`SimParams::second_specialty_chance`]). They are drawn from their own
//!   RNG stream, so the rest of the generation is the same as without them,
//!   and every recipe of a kind on the train is someone's specialty. A
//!   carriage with a choice of recipes makes its specialties first, at
//!   [`SimParams::specialty_output_bonus`], and the others only when none of
//!   its specialties can be made (storage full, inputs missing), at
//!   [`SimParams::off_specialty_output`] (see `World::produce`). A kind with
//!   a single recipe (Serra, Mensa) just makes it, as before.
//! - **Prices.** At a Mercato, `(base value + transport_per_carriage ×
//!   distance) × pay level × (1 + scarcity_markup × (1 − fill))`, where the
//!   distance counts the carriages to the nearest carriage specialized in the
//!   item (its *producer*) and `fill` is the share of the Mercato's shelf
//!   that is stocked. Carriages and specialties never change after
//!   generation, so the nearest producers are computed once, in [`Market`].
//! - **History.** At generation and every midnight the price of every item
//!   at every Mercato is sampled; the last [`SimParams::price_history_days`]
//!   samples are kept ([`World::price_history`]) for trends and charts. It
//!   lives in the sim rather than in the game: deterministic, saved with the
//!   world and available headless.
//! - **Player sales** ([`World::player_sell`]): the Mercato pays the player
//!   [`SimParams::player_sell_share`] of its price (a little more or less by
//!   the Mercante's affinity), from the treasury. The player's tokens are
//!   in the sim ([`crate::PlayerCharacter::tokens`]): the money supply does
//!   not change.

use std::collections::VecDeque;
use std::fmt;

use rand::seq::{IndexedRandom, SliceRandom};
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use super::{World, price_at};
use crate::carriage::{Carriage, CarriageKind};
use crate::event::EventKind;
use crate::ids::CarriageId;
use crate::item::ItemKind;
use crate::params::SimParams;

/// Seed of the specialties' own RNG stream (xored with the world seed).
const SPECIALTY_SEED: u64 = 0x5BEC_1A17_1E5B;
/// `Market::producers` entry for "nobody makes it".
const NO_PRODUCER: u16 = u16::MAX;
/// A trend compares the price with its mean over this many days before today.
pub const TREND_DAYS: u64 = 3;
/// Changes smaller than this share of the reference price (and than half a
/// token) are no trend.
const TREND_TOLERANCE: f32 = 0.05;

/// Specialties, nearest producers and price history of the train.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Market {
    /// Per carriage (indexed like `World::carriages`): the outputs of the
    /// recipes it specializes in, in [`ItemKind::ALL`] order. Empty for who
    /// makes nothing. A world without them (all empty) produces as before
    /// and counts every carriage of a producing kind as a producer.
    specialties: Vec<Vec<ItemKind>>,
    /// Per carriage and item (`carriage * ItemKind::COUNT + item`): the
    /// nearest producer of the item, or [`NO_PRODUCER`]. Empty in a world
    /// without them: no transport in the prices.
    producers: Vec<u16>,
    /// Daily price samples, oldest first.
    history: VecDeque<PriceSample>,
}

impl Market {
    /// Market of `carriages` with the given `specialties` (one list per carriage).
    pub(super) fn new(carriages: &[Carriage], specialties: Vec<Vec<ItemKind>>) -> Market {
        let mut market = Market {
            specialties,
            producers: Vec::new(),
            history: VecDeque::new(),
        };
        market.locate_producers(carriages);
        market
    }

    pub(super) fn specialties_of(&self, carriage: CarriageId) -> &[ItemKind] {
        self.specialties
            .get(carriage.index())
            .map_or(&[][..], Vec::as_slice)
    }

    /// Whether `c` produces `item`: one of its specialties or, in a world
    /// without specialties, a recipe of its kind.
    fn produces(&self, c: &Carriage, item: ItemKind) -> bool {
        match self.specialties_of(c.id) {
            [] if self.specialties.iter().all(Vec::is_empty) => c.kind.makes(item),
            specialties => specialties.contains(&item),
        }
    }

    /// Fills [`Market::producers`] (nearest first, towards the head on a tie).
    fn locate_producers(&mut self, carriages: &[Carriage]) {
        let n = carriages.len();
        self.producers = vec![NO_PRODUCER; n * ItemKind::COUNT];
        for item in ItemKind::ALL {
            let makers: Vec<usize> = carriages
                .iter()
                .filter(|c| self.produces(c, item))
                .map(|c| c.id.index())
                .collect();
            for c in 0..n {
                if let Some(&m) = makers.iter().min_by_key(|&&m| (m.abs_diff(c), m)) {
                    self.producers[c * ItemKind::COUNT + item.index()] = m as u16;
                }
            }
        }
    }

    /// Nearest producer of `item` to `carriage`.
    pub fn producer(&self, carriage: CarriageId, item: ItemKind) -> Option<CarriageId> {
        let k = carriage.index() * ItemKind::COUNT + item.index();
        match self.producers.get(k) {
            Some(&m) if m != NO_PRODUCER => Some(CarriageId(m)),
            _ => None,
        }
    }

    /// Carriages between `carriage` and the nearest producer of `item` (0
    /// if nobody makes it).
    pub fn distance(&self, carriage: CarriageId, item: ItemKind) -> u32 {
        self.producer(carriage, item)
            .map_or(0, |m| m.distance(carriage))
    }
}

/// Specialties of each of `carriages` for a world generated with `seed`.
pub(super) fn assign_specialties(
    seed: u64,
    carriages: &[Carriage],
    p: &SimParams,
) -> Vec<Vec<ItemKind>> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed ^ SPECIALTY_SEED);
    let second = f64::from(p.second_specialty_chance.clamp(0.0, 1.0));
    let mut out: Vec<Vec<ItemKind>> = carriages
        .iter()
        .map(|c| {
            let mut items: Vec<ItemKind> = c.kind.recipes().iter().map(|r| r.output).collect();
            if items.len() > 1 {
                let n = if rng.random_bool(second) { 2 } else { 1 };
                items.shuffle(&mut rng);
                items.truncate(n);
                items.sort();
            }
            items
        })
        .collect();
    // Every recipe of a kind on the train is someone's specialty: a missing
    // one goes to a carriage of that kind with the fewest specialties.
    for kind in CarriageKind::ALL {
        for recipe in kind.recipes() {
            let item = recipe.output;
            let of_kind: Vec<usize> = carriages
                .iter()
                .filter(|c| c.kind == kind)
                .map(|c| c.id.index())
                .collect();
            if of_kind.iter().any(|&c| out[c].contains(&item)) {
                continue;
            }
            let Some(fewest) = of_kind.iter().map(|&c| out[c].len()).min() else {
                continue;
            };
            let candidates: Vec<usize> = of_kind
                .into_iter()
                .filter(|&c| out[c].len() == fewest)
                .collect();
            if let Some(&c) = candidates.choose(&mut rng) {
                out[c].push(item);
                out[c].sort();
            }
        }
    }
    out
}

/// Mercato price of an item with base value `base`, `distance` carriages
/// from its producer, with its shelf `fill` (`0..=1`) stocked, at pay level
/// `level`. At least 1.
pub fn mercato_price(p: &SimParams, level: f32, base: u32, distance: u32, fill: f32) -> u32 {
    let transport = p.transport_per_carriage.max(0.0) * distance as f32;
    let value = (base as f32 + transport) * level.max(0.0);
    let price = value * (1.0 + p.scarcity_markup * (1.0 - fill.clamp(0.0, 1.0)));
    price.round().max(1.0) as u32
}

/// Prices of every Mercato on one day.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriceSample {
    /// Game day (sampled at its midnight, or at generation).
    pub day: u64,
    /// [`ItemKind::COUNT`] prices per Mercato, head to tail (see
    /// [`World::markets`]); 0 where the item is not sold.
    prices: Vec<u32>,
}

impl PriceSample {
    /// Price of `item` at the `rank`-th Mercato (head to tail), if sold there.
    pub fn price(&self, rank: usize, item: ItemKind) -> Option<u32> {
        self.prices
            .get(rank * ItemKind::COUNT + item.index())
            .copied()
            .filter(|&p| p > 0)
    }
}

/// Where a price is going, compared with its recent mean.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Trend {
    Up,
    Flat,
    Down,
}

impl Trend {
    /// Trend of a price now at `now` that was `reference` on average in the
    /// last days (`None`: no history yet, flat).
    pub fn of(reference: Option<f32>, now: u32) -> Trend {
        let Some(reference) = reference else {
            return Trend::Flat;
        };
        let diff = now as f32 - reference;
        let tolerance = (reference.abs() * TREND_TOLERANCE).max(0.5);
        if diff > tolerance {
            Trend::Up
        } else if diff < -tolerance {
            Trend::Down
        } else {
            Trend::Flat
        }
    }
}

/// An item at a Mercato, for the market window and the price list.
#[derive(Clone, Debug, PartialEq)]
pub struct MarketQuote {
    pub item: ItemKind,
    /// Selling price now.
    pub price: u32,
    /// What the Mercato pays the player for one ([`World::player_sell_price`]).
    pub buyback: u32,
    /// What the Mercato asks the player for one ([`World::player_price`]).
    pub player_price: u32,
    /// Whole units on the shelf, and how many fit.
    pub stock: u32,
    pub cap: u32,
    /// Nearest carriage specialized in the item, and how many carriages away.
    pub producer: Option<CarriageId>,
    pub distance: u32,
    /// Mean price over the last [`TREND_DAYS`] days (without today).
    pub reference: Option<f32>,
    pub trend: Trend,
}

/// Why [`World::player_sell`] failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SellError {
    /// Not a Mercato, or it does not deal in that item.
    NotForSale,
    /// No Mercante is working at a counter.
    NoMerchant,
    /// The shelf is full.
    NoRoom,
    /// The treasury cannot pay.
    NoMoney,
    /// The player has none.
    NotOwned,
}

impl fmt::Display for SellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SellError::NotForSale => f.write_str("qui non si compra"),
            SellError::NoMerchant => f.write_str("nessun mercante al bancone"),
            SellError::NoRoom => f.write_str("lo scaffale è pieno"),
            SellError::NoMoney => f.write_str("la cassa del treno è vuota"),
            SellError::NotOwned => f.write_str("non ce l'hai"),
        }
    }
}

impl World {
    /// What `carriage` specializes in (outputs of its recipes), in
    /// [`ItemKind::ALL`] order; empty for who makes nothing.
    pub fn specialties(&self, carriage: CarriageId) -> &[ItemKind] {
        self.market.specialties_of(carriage)
    }

    /// Replaces the specialties of `carriage` (keeping only what its kind
    /// makes) and recomputes the nearest producers. For tests and scenarios.
    pub fn set_specialties(&mut self, carriage: CarriageId, items: &[ItemKind]) {
        let Some(c) = self.carriages.get(carriage.index()) else {
            return;
        };
        let mut items: Vec<ItemKind> = items
            .iter()
            .copied()
            .filter(|&item| c.kind.makes(item))
            .collect();
        items.sort();
        items.dedup();
        let market = &mut self.market;
        if market.specialties.len() < self.carriages.len() {
            market.specialties.resize(self.carriages.len(), Vec::new());
        }
        market.specialties[carriage.index()] = items;
        market.locate_producers(&self.carriages);
    }

    /// Nearest carriage that produces `item` (see [`World::specialties`]) and
    /// how many carriages away from `from` it is.
    pub fn nearest_producer(&self, from: CarriageId, item: ItemKind) -> Option<(CarriageId, u32)> {
        self.market
            .producer(from, item)
            .map(|m| (m, m.distance(from)))
    }

    /// The Mercati, head to tail (the ranks of [`PriceSample::price`]).
    pub fn markets(&self) -> Vec<CarriageId> {
        self.carriages
            .iter()
            .filter(|c| c.kind == CarriageKind::Mercato)
            .map(|c| c.id)
            .collect()
    }

    /// The Mercato nearest to `from` (towards the head on a tie).
    pub fn nearest_market(&self, from: CarriageId) -> Option<CarriageId> {
        self.markets()
            .into_iter()
            .min_by_key(|m| (m.distance(from), m.0))
    }

    /// Daily price samples, oldest first (at most
    /// [`SimParams::price_history_days`]).
    pub fn price_history(&self) -> &VecDeque<PriceSample> {
        &self.market.history
    }

    /// `(day, price)` of `item` at the Mercato `market` in the history.
    pub fn price_series(&self, market: CarriageId, item: ItemKind) -> Vec<(u64, u32)> {
        let Some(rank) = self.markets().iter().position(|&m| m == market) else {
            return Vec::new();
        };
        self.market
            .history
            .iter()
            .filter_map(|s| Some((s.day, s.price(rank, item)?)))
            .collect()
    }

    /// Mean price of `item` at `market` over the last [`TREND_DAYS`] days
    /// before today, if sampled.
    fn reference_price(&self, rank: usize, item: ItemKind) -> Option<f32> {
        let today = self.clock.day();
        let recent: Vec<u32> = self
            .market
            .history
            .iter()
            .filter(|s| s.day < today && s.day + TREND_DAYS >= today)
            .filter_map(|s| s.price(rank, item))
            .collect();
        (!recent.is_empty()).then(|| recent.iter().sum::<u32>() as f32 / recent.len() as f32)
    }

    /// Everything the Mercato `carriage` deals in, in [`ItemKind::ALL`]
    /// order (empty if it is not a Mercato).
    pub fn market_quotes(&self, carriage: CarriageId) -> Vec<MarketQuote> {
        let Some(c) = self.carriage(carriage) else {
            return Vec::new();
        };
        let rank = self.markets().iter().position(|&m| m == carriage);
        ItemKind::ALL
            .into_iter()
            .filter_map(|item| {
                let price = self.price(carriage, item)?;
                let producer = self.market.producer(carriage, item);
                let reference = rank.and_then(|r| self.reference_price(r, item));
                Some(MarketQuote {
                    item,
                    price,
                    buyback: self.player_sell_price(carriage, item).unwrap_or(0),
                    player_price: self.player_price(carriage, item).unwrap_or(price),
                    stock: c.stock.count(item),
                    cap: self.params.storage_cap(c.kind, item).max(0.0).floor() as u32,
                    producer,
                    distance: producer.map_or(0, |m| m.distance(carriage)),
                    reference,
                    trend: Trend::of(reference, price),
                })
            })
            .collect()
    }

    /// What a Mercato selling at `price` pays the player for one unit.
    fn buyback(&self, price: u32) -> u32 {
        let share = self.params.player_sell_share.clamp(0.0, 1.0);
        (price as f32 * share).floor() as u32
    }

    /// What the Mercato `carriage` would pay the player now for one `item`.
    pub fn sell_price(&self, carriage: CarriageId, item: ItemKind) -> Option<u32> {
        self.price(carriage, item).map(|price| self.buyback(price))
    }

    /// What the Mercato `carriage` pays the player now for one `item`:
    /// [`World::sell_price`], higher or lower by the affinity of the
    /// Mercante on duty (like [`World::player_price`]), never above what it
    /// asks the player.
    pub fn player_sell_price(&self, carriage: CarriageId, item: ItemKind) -> Option<u32> {
        let pay = self.sell_price(carriage, item)?;
        let affinity = self
            .merchant_on_duty(carriage)
            .map_or(0.0, crate::Npc::player_affinity);
        let factor = 1.0 + super::player::PRICE_PER_AFFINITY * affinity.clamp(-1.0, 1.0);
        let pay = (pay as f32 * factor).floor() as u32;
        let ask = self.player_price(carriage, item).unwrap_or(0);
        Some(pay.min(ask.saturating_sub(1)))
    }

    /// The player sells one `item` from its inventory to the Mercato
    /// `carriage`, which pays [`World::player_sell_price`] from the treasury
    /// and puts the item on its shelf. A Mercante must be at the counter and
    /// the shelf must have room. Tokens only move between the treasury and
    /// the player: the money supply does not change. Returns the payment.
    pub fn player_sell(&mut self, carriage: CarriageId, item: ItemKind) -> Result<u32, SellError> {
        let pay = self
            .player_sell_price(carriage, item)
            .ok_or(SellError::NotForSale)?;
        if self.merchant_on_duty(carriage).is_none() {
            return Err(SellError::NoMerchant);
        }
        if !self.player.inventory.has(item, 1) {
            return Err(SellError::NotOwned);
        }
        let cap = self.params.storage_cap(CarriageKind::Mercato, item);
        let c = &mut self.carriages[carriage.index()];
        if c.stock.get(item) + 1.0 > cap {
            return Err(SellError::NoRoom);
        }
        if self.economy.treasury < u64::from(pay) {
            return Err(SellError::NoMoney);
        }
        c.stock.add(item, 1.0, cap);
        self.player.inventory.remove(item, 1);
        self.economy.treasury -= u64::from(pay);
        self.player.tokens = self.player.tokens.saturating_add(pay);
        self.economy.counters.player_sales += u64::from(pay);
        self.log(EventKind::PlayerSold {
            item,
            price: pay,
            carriage,
        });
        Ok(pay)
    }

    /// Samples today's prices (at generation and every midnight).
    pub(super) fn record_prices(&mut self) {
        let markets = self.markets();
        let mut prices = Vec::with_capacity(markets.len() * ItemKind::COUNT);
        for &m in &markets {
            let c = &self.carriages[m.index()];
            for item in ItemKind::ALL {
                let price = price_at(&self.params, self.economy.pay_level, &self.market, c, item);
                prices.push(price.unwrap_or(0));
            }
        }
        let day = self.clock.day();
        let keep = self.params.price_history_days;
        let history = &mut self.market.history;
        if history.back().is_some_and(|s| s.day == day) {
            history.pop_back();
        }
        history.push_back(PriceSample { day, prices });
        while history.len() > keep {
            history.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_grows_with_distance_and_scarcity() {
        let p = SimParams::default();
        let price = |distance, fill| mercato_price(&p, 1.0, 12, distance, fill);
        for fill in [0.0, 0.5, 1.0] {
            for d in 0..20 {
                assert!(price(d + 1, fill) >= price(d, fill), "d {d} fill {fill}");
            }
            assert!(price(10, fill) > price(0, fill));
        }
        assert_eq!(price(0, 1.0), 12);
        assert_eq!(price(3, 1.0), 15);
        assert_eq!(price(0, 0.0), 18);
        assert!(price(0, 0.0) > price(0, 1.0));
        // Indexed by the pay level, never below 1.
        assert_eq!(mercato_price(&p, 2.0, 12, 3, 1.0), 30);
        assert_eq!(mercato_price(&p, 0.0, 12, 3, 1.0), 1);
    }

    #[test]
    fn trend_needs_a_real_change() {
        assert_eq!(Trend::of(None, 10), Trend::Flat);
        assert_eq!(Trend::of(Some(10.0), 10), Trend::Flat);
        assert_eq!(Trend::of(Some(10.0), 11), Trend::Up);
        assert_eq!(Trend::of(Some(10.0), 9), Trend::Down);
        assert_eq!(Trend::of(Some(40.0), 41), Trend::Flat);
        assert_eq!(Trend::of(Some(40.0), 43), Trend::Up);
        assert_eq!(Trend::of(Some(10.4), 10), Trend::Flat);
    }
}
