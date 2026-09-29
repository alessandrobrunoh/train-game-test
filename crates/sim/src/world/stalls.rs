//! Banchi del Mercato: chiunque mette in vendita qualsiasi oggetto.
//!
//! Every Mercato has, besides its own shelf, open **stalls** where anyone
//! sells anything from its inventory: food, materials, tools, clothes,
//! comfort goods (and, later, items the AI invents: nothing here depends on
//! a specific item, only on the catalog's `usage`, `base_value`,
//! `stack_limit` and `amenity`).
//!
//! - **Listings** ([`Listing`]): `qty` units of one item at `price_each`
//!   tokens, by a [`Seller`]: an NPC, the player or the Mercato itself.
//!   Listing ([`World::list_for_sale`]) moves the goods out of the seller's
//!   inventory into the listing; the seller must be at that Mercato (no
//!   Mercante needed: the stalls are self-service, and open at night too;
//!   NPCs just don't shop at night). Withdrawing ([`World::withdraw`])
//!   gives back what fits. After [`SimParams::listing_days`] unsold goods
//!   go back to the seller ([`StallEvent::ListingExpired`]); what does not
//!   fit, or has no seller any more, goes to the Mercato.
//! - **Buying** ([`World::buy_listing`]): the buyer pays the seller directly
//!   (tokens move from pocket to pocket; the Mercato's own sales go to the
//!   treasury, as on its shelf), and the goods go into the buyer's
//!   inventory: a purchase that does not fit is partial, or refused.
//! - **The Mercato's goods.** Its shelf (Attrezzi and Vestiti brought by
//!   the Mercanti, [`ItemKind::is_sold`]) is unchanged and still sold at
//!   [`World::price`] / [`World::player_price`]. Goods left to the Mercato
//!   that its shelf cannot hold (a dead seller's listing, an estate)
//!   become a [`Seller::Mercato`] listing at the current quote, which pays
//!   the treasury; unsold after [`SimParams::listing_days`], they go to the
//!   storage of the nearest carriages that keep them (the item's outlet
//!   first), so nothing piles up at the stalls.
//! - **Quotes** ([`World::quote`]): the shelf price for what the shelf deals
//!   in; for any other item the same formula ([`super::mercato_price`]):
//!   base value plus transport from the nearest producer, times the pay
//!   level, with the scarcity markup from the units on the stalls there
//!   (one full stack, [`ItemKind::stack_size`], counts as well stocked).
//!   [`World::market_offers`] lists every item with its quote and offers.
//!
//! NPCs ([`crate::Inventory::items`], [`crate::NPC_ITEM_SLOTS`] slots):
//! - they keep [`SimParams::own_share`] of their work's output (whole units,
//!   drawn from the `trade_rng` stream, at most
//!   [`SimParams::own_share_max_units`] of an item), keep the player's
//!   non-food gifts worth at least [`SimParams::gift_keep_min_value`], and
//!   buy at the stalls;
//! - a spare Attrezzo or Vestito is put on as soon as theirs is missing;
//!   own food is eaten below [`SimParams::eat_own_food_hunger`]; a comfort
//!   good their Dormitorio lacks is brought home;
//! - **buying**: who needs a tool, clothes or (adults with
//!   [`SimParams::comfort_buy_min_tokens`]) a comfort good for home, takes
//!   the cheapest offer at a Mercato, shelf or stall. Who is hungrier than
//!   [`SimParams::stall_food_hunger`] and is at a Mercato may buy food
//!   there; the Mense stay the way to eat (nobody walks to a stall for food);
//! - **selling**: whatever they don't use is listed as soon as they are at a
//!   Mercato (daytime), at the quote times their greed
//!   ([`SimParams::stall_greed`], [`SimParams::stall_kind_discount`]). Who
//!   is short of tokens ([`SimParams::sell_below_tokens`]) or holds goods
//!   worth [`SimParams::sell_surplus_value`] also walks to the nearest
//!   Mercato to sell (a low-priority [`crate::ActionKind::Buy`] trip). The
//!   last food is never sold by who is hungry;
//! - selling stops at a full stack of the item already on that Mercato's
//!   stalls, and a spare of a tool or clothes nearly worn out is kept;
//! - when they die, their items go to their partner or else children (what
//!   they can carry), then to their home's storage (what it can hold; the
//!   rest to the nearest Mercato), and their listings to the Mercato.
//!
//! Money is conserved ([`World::money_supply`]) and so are goods: the
//! *trade pool* (every NPC's items plus every listing,
//! [`World::trade_pool_units`]) only changes through the flows counted in
//! [`TradeCounters::into_pool`] and [`TradeCounters::out_of_pool`].
//!
//! The chronicle of the stalls is [`World::stall_log`] ([`StallRecord`],
//! with Italian texts): not [`crate::EventKind`] variants yet, so that the
//! crates matching on those keep compiling.

use std::collections::VecDeque;
use std::fmt;

use rand::RngExt;
use serde::{Deserialize, Serialize};

use super::market::{Market, mercato_price};
use super::{World, price_at};
use crate::action::Action;
use crate::carriage::{Carriage, CarriageKind};
use crate::defs::ItemUse;
use crate::event::EventKind;
use crate::ids::{CarriageId, NpcId};
use crate::item::ItemKind;
use crate::npc::{Job, LifeStage, Npc};
use crate::params::SimParams;
use crate::personality::Temper;
use crate::time::{GameTime, MINUTES_PER_DAY};

/// Entries kept in [`World::stall_log`].
pub const STALL_LOG_KEPT: usize = 300;
/// Hunger below which an NPC won't sell its last food.
const KEEP_LAST_FOOD_HUNGER: f32 = 0.5;
/// An NPC keeps a spare of a tool or clothes it uses below this durability.
const KEEP_SPARE_DURABILITY: f32 = 0.25;

/// Identifier of a [`Listing`], never reused.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ListingId(pub u32);

/// Who sells on a stall.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Seller {
    Npc(NpcId),
    Player,
    /// The Mercato itself: goods left to it; its sales pay the treasury.
    Mercato,
}

/// Who buys at a stall.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Buyer {
    Npc(NpcId),
    Player,
}

/// Goods on a stall of a Mercato.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    pub id: ListingId,
    /// The Mercato.
    pub market: CarriageId,
    pub seller: Seller,
    pub item: ItemKind,
    /// Units left (at least 1).
    pub qty: u32,
    /// Asking price of one unit (for [`Seller::Mercato`], refreshed hourly
    /// to the quote; buyers always pay the current quote).
    pub price_each: u32,
    /// When it was listed.
    pub since: GameTime,
}

impl Listing {
    /// When unsold goods go back to the seller (the Mercato's own go to the
    /// train's storage, see [`StallEvent::ListingExpired`]).
    pub fn expires(&self, p: &SimParams) -> GameTime {
        self.since + p.listing_days.max(1) * MINUTES_PER_DAY
    }
}

/// Where an offer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OfferSource {
    /// The Mercato's shelf: bought with [`World::player_buy`] (a Mercante
    /// must be at the counter).
    Shelf,
    /// A stall: bought with [`World::player_buy_listing`].
    Listing(ListingId),
}

/// One way to buy an item at a Mercato.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Offer {
    pub source: OfferSource,
    pub seller: Seller,
    /// Price of one unit for the player.
    pub price: u32,
    /// Whole units available.
    pub qty: u32,
}

/// An item at a Mercato, with every offer, for the market window.
#[derive(Clone, Debug, PartialEq)]
pub struct MarketOffer {
    pub item: ItemKind,
    /// What one unit is worth there now ([`World::quote`]).
    pub quote: u32,
    /// Shelf and stalls, cheapest first (the shelf first on a tie).
    pub offers: Vec<Offer>,
}

impl MarketOffer {
    /// The cheapest offer, if any.
    pub fn best(&self) -> Option<&Offer> {
        self.offers.first()
    }

    /// Units offered in all.
    pub fn units(&self) -> u32 {
        self.offers.iter().map(|o| o.qty).sum()
    }
}

/// A purchase at a stall.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StallSale {
    pub units: u32,
    /// Tokens paid in all.
    pub paid: u32,
}

/// Why a stall operation failed. Nothing changes on error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StallError {
    /// The carriage is not a Mercato.
    NotAMarket,
    /// The seller or buyer is not at that Mercato (or is asleep).
    NotThere,
    /// Not enough of the item to list.
    NotOwned,
    /// Zero units.
    BadQuantity,
    /// A price of 0.
    BadPrice,
    /// Already [`SimParams::max_listings_per_seller`] listings.
    TooManyListings,
    NoSuchListing,
    /// Only the seller can withdraw its goods (not the Mercato's).
    NotYours,
    /// Buying one's own goods.
    OwnListing,
    /// Not enough tokens; holds the price of the units asked.
    TooExpensive(u32),
    /// No room in the inventory.
    NoRoom,
    /// The NPC does not exist (any more).
    NoSuchNpc,
}

impl fmt::Display for StallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StallError::NotAMarket => f.write_str("qui non c'è un mercato"),
            StallError::NotThere => f.write_str("devi essere al mercato"),
            StallError::NotOwned => f.write_str("non ne hai abbastanza"),
            StallError::BadQuantity => f.write_str("quantità non valida"),
            StallError::BadPrice => f.write_str("il prezzo deve essere almeno 1 gettone"),
            StallError::TooManyListings => f.write_str("hai già troppi banchi aperti"),
            StallError::NoSuchListing => f.write_str("il banco non c'è più"),
            StallError::NotYours => f.write_str("non è roba tua"),
            StallError::OwnListing => f.write_str("è roba tua"),
            StallError::TooExpensive(price) => write!(f, "servono {price} gettoni"),
            StallError::NoRoom => f.write_str("l'inventario è pieno"),
            StallError::NoSuchNpc => f.write_str("non c'è più"),
        }
    }
}

/// Something that happened at the stalls ([`World::stall_log`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum StallEvent {
    /// New goods on a stall.
    Listed {
        listing: ListingId,
        market: CarriageId,
        seller: Seller,
        seller_name: String,
        item: ItemKind,
        qty: u32,
        price: u32,
    },
    /// `qty` units bought at `price` each.
    SoldAtStall {
        listing: ListingId,
        market: CarriageId,
        seller: Seller,
        seller_name: String,
        buyer: Buyer,
        buyer_name: String,
        item: ItemKind,
        qty: u32,
        price: u32,
    },
    /// Unsold after [`SimParams::listing_days`]: `qty` units back to the
    /// seller, of which `to_mercato` went to the Mercato instead. For the
    /// Mercato's own goods: `qty` units into the storage of the nearest
    /// carriages that keep them, `to_mercato` left on its stall (no room).
    ListingExpired {
        listing: ListingId,
        market: CarriageId,
        seller: Seller,
        seller_name: String,
        item: ItemKind,
        qty: u32,
        to_mercato: u32,
    },
    /// The seller took `qty` units back.
    Withdrawn {
        listing: ListingId,
        market: CarriageId,
        seller: Seller,
        seller_name: String,
        item: ItemKind,
        qty: u32,
    },
}

/// A [`StallEvent`] with its time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StallRecord {
    pub time: GameTime,
    pub event: StallEvent,
}

/// "una razione" / "3 razioni".
fn units_of(item: ItemKind, n: u32) -> String {
    if n == 1 {
        item.with_article().to_string()
    } else {
        format!("{n} {}", item.plural())
    }
}

impl fmt::Display for StallEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StallEvent::Listed {
                seller_name,
                item,
                qty,
                price,
                market,
                ..
            } => write!(
                f,
                "{seller_name} mette sul banco {} a {price} gettoni l'uno (carrozza {market})",
                units_of(*item, *qty)
            ),
            StallEvent::SoldAtStall {
                seller_name,
                buyer_name,
                item,
                qty,
                price,
                ..
            } => write!(
                f,
                "{buyer_name} compra {} al banco di {seller_name} per {} gettoni",
                units_of(*item, *qty),
                price * qty
            ),
            StallEvent::ListingExpired {
                seller: Seller::Mercato,
                item,
                qty,
                to_mercato,
                ..
            } => {
                let stored = qty - to_mercato;
                if stored == 0 {
                    write!(
                        f,
                        "Il Mercato tiene ancora sul banco {}",
                        units_of(*item, *qty)
                    )
                } else {
                    write!(
                        f,
                        "Il Mercato manda in magazzino {} rimaste invendute",
                        units_of(*item, stored)
                    )
                }
            }
            StallEvent::ListingExpired {
                seller_name,
                item,
                qty,
                to_mercato,
                ..
            } => {
                let back = units_of(*item, *qty);
                match (*to_mercato, qty - to_mercato) {
                    (0, _) => write!(
                        f,
                        "Il banco di {seller_name} chiude: {back} tornano indietro"
                    ),
                    (_, 0) => write!(
                        f,
                        "Il banco di {seller_name} chiude: {back} restano al Mercato"
                    ),
                    (m, _) => write!(
                        f,
                        "Il banco di {seller_name} chiude: {back} tornano indietro ({m} restano al Mercato)"
                    ),
                }
            }
            StallEvent::Withdrawn {
                seller_name,
                item,
                qty,
                ..
            } => write!(
                f,
                "{seller_name} ritira {} dal banco",
                units_of(*item, *qty)
            ),
        }
    }
}

impl fmt::Display for StallRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.time, self.event)
    }
}

/// Counters of the stalls and of the NPCs' belongings since generation.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TradeCounters {
    /// Listings opened, and units listed.
    pub listings: u64,
    pub listed_units: u64,
    /// Purchases at the stalls, units and tokens (to sellers or treasury).
    pub trades: u64,
    pub sold_units: u64,
    pub sold_tokens: u64,
    /// Of which between two NPCs.
    pub npc_trades: u64,
    /// Units back to the seller at expiry, and taken back by the seller.
    pub expired_units: u64,
    pub withdrawn_units: u64,
    /// Units left to the Mercato (shelf or [`Seller::Mercato`] listing).
    pub to_mercato_units: u64,
    /// Units kept by workers from their output ([`SimParams::own_share`]).
    pub own_share_units: u64,
    /// Player's gifts kept to sell.
    pub gifts_kept: u64,
    /// Own food eaten, spare goods put on, comfort goods brought home.
    pub eaten_units: u64,
    pub equipped_units: u64,
    pub furnished_units: u64,
    /// Units of dead NPCs' items put into their home's storage (what their
    /// heirs could not carry).
    pub estate_units: u64,
    /// Units into and out of the trade pool (every NPC's items and every
    /// listing, see [`World::trade_pool_units`]).
    pub into_pool: u64,
    pub out_of_pool: u64,
}

/// Units of `item` on the stalls of `market`.
fn stall_units(market: &Market, at: CarriageId, item: ItemKind) -> u32 {
    market.stall_units(at, item)
}

/// What one `item` is worth at `c` (a Mercato) now: the shelf price for
/// what its shelf deals in, else the same formula with the fill of its
/// stalls (a full stack counts as well stocked). None if not a Mercato.
pub(super) fn quote_at(
    p: &SimParams,
    level: f32,
    market: &Market,
    c: &Carriage,
    item: ItemKind,
) -> Option<u32> {
    if let Some(price) = price_at(p, level, market, c, item) {
        return Some(price);
    }
    if c.kind != CarriageKind::Mercato {
        return None;
    }
    let fill = stall_units(market, c.id, item) as f32 / item.stack_size().max(1) as f32;
    let distance = market.distance(c.id, item);
    Some(mercato_price(p, level, item.base_value(), distance, fill))
}

/// Price of one unit of listing `l` now (the Mercato's at the quote).
fn listing_price(p: &SimParams, level: f32, market: &Market, c: &Carriage, l: &Listing) -> u32 {
    match l.seller {
        Seller::Mercato => quote_at(p, level, market, c, l.item).unwrap_or(l.price_each),
        _ => l.price_each,
    }
}

/// Where an NPC buys: the Mercato's shelf, or a listing (by index).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Source {
    Shelf,
    Listing(usize),
}

/// The cheapest offer of `item` at `c` for NPC `buyer` (not its own
/// listings): `(price, source)`. The shelf wins ties, then older listings.
pub(super) fn best_offer(
    p: &SimParams,
    level: f32,
    market: &Market,
    c: &Carriage,
    item: ItemKind,
    buyer: NpcId,
) -> Option<(u32, Source)> {
    if c.kind != CarriageKind::Mercato {
        return None;
    }
    let shelf = (c.stock.count(item) >= 1)
        .then(|| price_at(p, level, market, c, item))
        .flatten()
        .map(|price| (price, Source::Shelf));
    let stall = market
        .listings
        .iter()
        .enumerate()
        .filter(|(_, l)| {
            l.market == c.id && l.item == item && l.qty > 0 && l.seller != Seller::Npc(buyer)
        })
        .map(|(k, l)| (listing_price(p, level, market, c, l), Source::Listing(k)))
        .min_by_key(|&(price, source)| match source {
            Source::Listing(k) => (price, k),
            Source::Shelf => (price, 0),
        });
    match (shelf, stall) {
        (Some(s), Some(l)) if l.0 < s.0 => Some(l),
        (Some(s), _) => Some(s),
        (None, l) => l,
    }
}

/// Whether `npc` would buy `item` now (money and place aside): a tool or
/// clothes it lacks ([`Npc::wants`]); food when hungry and it has none of
/// its own; a comfort good its Dormitorio lacks, for a well-off adult.
pub(super) fn wants_item(
    p: &SimParams,
    level: f32,
    market: &Market,
    npc: &Npc,
    item: ItemKind,
) -> bool {
    let items = &npc.inventory.items;
    match item.def().usage {
        // As [`Npc::wants`], with one catalog lookup.
        ItemUse::Tool => npc.job.is_some_and(Job::uses_tool) && npc.inventory.tool.is_none(),
        ItemUse::Clothes => npc.inventory.clothes.is_none(),
        ItemUse::Food => {
            npc.needs.hunger < p.stall_food_hunger
                && !items
                    .slots()
                    .iter()
                    .flatten()
                    .any(|s| s.item.def().usage == ItemUse::Food)
                && items.room_for(item) > 0
        }
        ItemUse::Material => {
            let rich = (p.comfort_buy_min_tokens as f32 * level).round() as u32;
            npc.age >= LifeStage::ADULTO_FROM
                && npc.inventory.tokens >= rich
                && items.count(item) == 0
                && items.room_for(item) > 0
                && market.short_goods.contains(&(npc.home, item))
        }
    }
}

/// Units of each item `npc` could sell: all it carries, except food it
/// needs (the last unit when hungry), a spare for a tool or clothes it uses
/// that are nearly worn out, and comfort goods its home lacks.
pub(super) fn sellable(market: &Market, npc: &Npc) -> Vec<(ItemKind, u32)> {
    let mut out = npc.inventory.items.items();
    for (item, n) in &mut out {
        let def = item.def();
        if def.usage == ItemUse::Food && npc.needs.hunger < KEEP_LAST_FOOD_HUNGER {
            *n = n.saturating_sub(1);
        }
        let uses = def.usage != ItemUse::Tool || npc.job.is_some_and(Job::uses_tool);
        let worn = npc
            .inventory
            .durability(*item)
            .is_some_and(|d| d < KEEP_SPARE_DURABILITY);
        if uses && worn {
            *n = n.saturating_sub(1);
        }
        if market.short_goods.contains(&(npc.home, *item)) {
            *n = 0;
        }
    }
    out.retain(|&(_, n)| n > 0);
    out
}

/// Whether NPC `id` would put `item` on a stall of the Mercato `at`: it has
/// no listing of it there yet, and the stalls there don't already offer a
/// full stack of it.
fn listable_at(market: &Market, id: NpcId, at: CarriageId, item: ItemKind) -> bool {
    let listed = market
        .listings
        .iter()
        .any(|l| l.seller == Seller::Npc(id) && l.market == at && l.item == item);
    !listed && stall_units(market, at, item) < item.stack_size()
}

/// Asking price of `npc` for an item quoted `quote`: greedier when less
/// honest, cheaper when kind. At least 1.
pub(super) fn ask_price(p: &SimParams, npc: &Npc, quote: u32) -> u32 {
    let greed = p.stall_greed * (1.0 - 2.0 * npc.traits.honesty.clamp(0.0, 1.0));
    let kind = if npc.personality().has(Temper::Gentile) {
        p.stall_kind_discount
    } else {
        0.0
    };
    (quote as f32 * (1.0 + greed - kind).max(0.1))
        .round()
        .max(1.0) as u32
}

impl World {
    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    /// Every open listing, oldest first.
    pub fn listings(&self) -> &[Listing] {
        &self.market.listings
    }

    /// Listing `id`, if still open.
    pub fn listing(&self, id: ListingId) -> Option<&Listing> {
        self.market.listings.iter().find(|l| l.id == id)
    }

    /// The stalls of the Mercato `market`, by item ([`ItemKind::ALL`]
    /// order), then cheapest first.
    pub fn listings_at(&self, market: CarriageId) -> Vec<&Listing> {
        let mut out: Vec<&Listing> = self
            .market
            .listings
            .iter()
            .filter(|l| l.market == market)
            .collect();
        out.sort_by_key(|l| (l.item, l.price_each, l.id));
        out
    }

    /// Open listings of `seller`, oldest first.
    pub fn listings_of(&self, seller: Seller) -> Vec<&Listing> {
        self.market
            .listings
            .iter()
            .filter(|l| l.seller == seller)
            .collect()
    }

    /// Name of a seller: the NPC's, the player's, or "il Mercato".
    pub fn seller_name(&self, seller: Seller) -> String {
        match seller {
            Seller::Npc(id) => self
                .npc(id)
                .map_or_else(|| "qualcuno".to_string(), |n| n.name.clone()),
            Seller::Player => self.player.name.clone(),
            Seller::Mercato => "il Mercato".to_string(),
        }
    }

    /// Name of a buyer.
    pub fn buyer_name(&self, buyer: Buyer) -> String {
        match buyer {
            Buyer::Npc(id) => self.seller_name(Seller::Npc(id)),
            Buyer::Player => self.player.name.clone(),
        }
    }

    /// What one `item` is worth at the Mercato `market` now (see the module
    /// docs); None if it is not a Mercato. The same as [`World::price`] for
    /// what its shelf deals in.
    pub fn quote(&self, market: CarriageId, item: ItemKind) -> Option<u32> {
        let c = self.carriage(market)?;
        quote_at(&self.params, self.economy.pay_level, &self.market, c, item)
    }

    /// Every item at the Mercato `market` ([`ItemKind::ALL`] order) with its
    /// quote and the offers for the player: the shelf at
    /// [`World::player_price`] (if it has a whole unit) and each stall.
    /// Empty if it is not a Mercato.
    pub fn market_offers(&self, market: CarriageId) -> Vec<MarketOffer> {
        let Some(c) = self.carriage(market) else {
            return Vec::new();
        };
        if c.kind != CarriageKind::Mercato {
            return Vec::new();
        }
        let (p, level) = (&self.params, self.economy.pay_level);
        ItemKind::ALL
            .into_iter()
            .filter_map(|item| {
                let quote = self.quote(market, item)?;
                let mut offers = Vec::new();
                let stock = c.stock.count(item);
                if stock >= 1
                    && let Some(price) = self.player_price(market, item)
                {
                    offers.push(Offer {
                        source: OfferSource::Shelf,
                        seller: Seller::Mercato,
                        price,
                        qty: stock,
                    });
                }
                for l in self.market.listings.iter() {
                    if l.market == market && l.item == item {
                        offers.push(Offer {
                            source: OfferSource::Listing(l.id),
                            seller: l.seller,
                            price: listing_price(p, level, &self.market, c, l),
                            qty: l.qty,
                        });
                    }
                }
                offers.sort_by_key(|o| {
                    let id = match o.source {
                        OfferSource::Shelf => None,
                        OfferSource::Listing(id) => Some(id),
                    };
                    (o.price, id)
                });
                Some(MarketOffer {
                    item,
                    quote,
                    offers,
                })
            })
            .collect()
    }

    /// Whether `npc` would buy `item` now (money and place aside): see the
    /// module docs.
    pub fn npc_wants(&self, npc: &Npc, item: ItemKind) -> bool {
        wants_item(
            &self.params,
            self.economy.pay_level,
            &self.market,
            npc,
            item,
        )
    }

    /// Whether `npc` would walk to a Mercato to sell what it doesn't need
    /// (daytime, short of tokens or holding goods worth
    /// [`SimParams::sell_surplus_value`], a listing to spare), counting only
    /// what it could actually list at the nearest Mercato.
    pub fn wants_to_sell(&self, npc: &Npc) -> bool {
        let p = &self.params;
        if npc.inventory.items.is_empty()
            || p.is_night(self.clock.hour())
            || npc.age < LifeStage::GIOVANE_FROM
        {
            return false;
        }
        if self.listing_count(Seller::Npc(npc.id)) >= p.max_listings_per_seller {
            return false;
        }
        // Only what it could actually list there (see `list_surplus`).
        let Some(at) = self.nearest_market(npc.carriage) else {
            return false;
        };
        let mut goods = sellable(&self.market, npc);
        goods.retain(|&(item, _)| listable_at(&self.market, npc.id, at, item));
        if goods.is_empty() {
            return false;
        }
        let level = self.economy.pay_level;
        let poor = (p.sell_below_tokens as f32 * level).round() as u32;
        if npc.inventory.tokens < poor {
            return true;
        }
        let value: u32 = goods
            .iter()
            .map(|&(item, n)| {
                let quote = self.quote(at, item).unwrap_or(item.base_value());
                quote.saturating_mul(n)
            })
            .sum();
        value as f32 >= p.sell_surplus_value as f32 * level
    }

    /// The chronicle of the stalls, oldest first (at most [`STALL_LOG_KEPT`]).
    pub fn stall_log(&self) -> &VecDeque<StallRecord> {
        &self.market.stall_log
    }

    /// Stall records logged since generation, including those dropped from
    /// [`World::stall_log`] (like [`World::events_total`]).
    pub fn stall_log_total(&self) -> u64 {
        self.market.stall_log_dropped + self.market.stall_log.len() as u64
    }

    /// Stall and belongings counters since generation.
    pub fn trade_counters(&self) -> &TradeCounters {
        &self.market.trade
    }

    /// Units in the trade pool: every NPC's items plus every listing.
    pub fn trade_pool_units(&self) -> u64 {
        let carried: u64 = self
            .npcs
            .iter()
            .flat_map(|n| n.inventory.items.items())
            .map(|(_, n)| u64::from(n))
            .sum();
        carried
            + self
                .market
                .listings
                .iter()
                .map(|l| u64::from(l.qty))
                .sum::<u64>()
    }

    // ------------------------------------------------------------------
    // Generic operations (NPCs and player)
    // ------------------------------------------------------------------

    /// `seller` puts `qty` units of `item` from its inventory on a stall of
    /// the Mercato `market` at `price` tokens each. The seller must be there
    /// (an NPC not walking away; the player awake). Returns the listing.
    pub fn list_for_sale(
        &mut self,
        seller: Seller,
        market: CarriageId,
        item: ItemKind,
        qty: u32,
        price: u32,
    ) -> Result<ListingId, StallError> {
        let c = self.carriage(market).ok_or(StallError::NotAMarket)?;
        if c.kind != CarriageKind::Mercato {
            return Err(StallError::NotAMarket);
        }
        if qty == 0 {
            return Err(StallError::BadQuantity);
        }
        if price == 0 {
            return Err(StallError::BadPrice);
        }
        if seller == Seller::Mercato {
            return Err(StallError::NotYours);
        }
        self.check_at_market(seller, market)?;
        let have = match seller {
            Seller::Npc(id) => self.npc(id).map_or(0, |n| n.inventory.items.count(item)),
            Seller::Player => self.player.inventory.count(item),
            Seller::Mercato => 0,
        };
        if have < qty {
            return Err(StallError::NotOwned);
        }
        if self.listing_count(seller) >= self.params.max_listings_per_seller {
            return Err(StallError::TooManyListings);
        }
        match seller {
            Seller::Npc(id) => {
                if let Some(i) = self.npc_index(id) {
                    self.npcs[i].inventory.items.remove(item, qty);
                }
            }
            Seller::Player => {
                self.player.inventory.remove(item, qty);
                self.market.trade.into_pool += u64::from(qty);
            }
            Seller::Mercato => {}
        }
        Ok(self.open_listing(seller, market, item, qty, price))
    }

    /// The seller takes back what fits of listing `id` into its inventory
    /// (it must be at that Mercato; the rest stays on the stall). Returns
    /// the units taken back.
    pub fn withdraw(&mut self, id: ListingId) -> Result<u32, StallError> {
        let k = self.listing_index(id)?;
        let l = self.market.listings[k].clone();
        if l.seller == Seller::Mercato {
            return Err(StallError::NotYours);
        }
        self.check_at_market(l.seller, l.market)?;
        let back = self.give_back(l.seller, l.item, l.qty, false);
        if back == 0 {
            return Err(StallError::NoRoom);
        }
        self.take_from_listing(k, back);
        self.market.trade.withdrawn_units += u64::from(back);
        let seller_name = self.seller_name(l.seller);
        self.stall_record(StallEvent::Withdrawn {
            listing: id,
            market: l.market,
            seller: l.seller,
            seller_name,
            item: l.item,
            qty: back,
        });
        Ok(back)
    }

    /// `buyer` buys up to `qty` units of listing `id` (what fits in its
    /// inventory) and pays the seller (the treasury for the Mercato's goods)
    /// directly. The buyer must be at that Mercato. An NPC lacking a tool or
    /// clothes puts on what it bought.
    pub fn buy_listing(
        &mut self,
        buyer: Buyer,
        id: ListingId,
        qty: u32,
    ) -> Result<StallSale, StallError> {
        let k = self.listing_index(id)?;
        let l = self.market.listings[k].clone();
        if qty == 0 {
            return Err(StallError::BadQuantity);
        }
        let own = matches!((buyer, l.seller), (Buyer::Player, Seller::Player))
            || matches!((buyer, l.seller), (Buyer::Npc(b), Seller::Npc(s)) if b == s);
        if own {
            return Err(StallError::OwnListing);
        }
        let as_seller = match buyer {
            Buyer::Npc(b) => Seller::Npc(b),
            Buyer::Player => Seller::Player,
        };
        self.check_at_market(as_seller, l.market)?;
        let c = &self.carriages[l.market.index()];
        let price = listing_price(&self.params, self.economy.pay_level, &self.market, c, &l);
        let (room, tokens) = match buyer {
            Buyer::Npc(b) => {
                let n = self.npc(b).ok_or(StallError::NoSuchNpc)?;
                (n.inventory.items.room_for(l.item), n.inventory.tokens)
            }
            Buyer::Player => (self.player.inventory.room_for(l.item), self.player.tokens),
        };
        let units = qty.min(l.qty).min(room);
        if units == 0 {
            return Err(StallError::NoRoom);
        }
        let cost = price.saturating_mul(units);
        if tokens < cost {
            return Err(StallError::TooExpensive(cost));
        }
        // Goods and money move.
        self.take_from_listing(k, units);
        match buyer {
            Buyer::Npc(b) => {
                let i = self.npc_index(b).ok_or(StallError::NoSuchNpc)?;
                let npc = &mut self.npcs[i];
                npc.inventory.tokens -= cost;
                npc.inventory.items.add(l.item, units);
                self.equip_spares(i);
            }
            Buyer::Player => {
                self.player.tokens -= cost;
                self.player.inventory.add(l.item, units);
                self.market.trade.out_of_pool += u64::from(units);
            }
        }
        self.pay_seller(l.seller, l.market, l.item, price, units, buyer);
        let t = &mut self.market.trade;
        t.trades += 1;
        t.sold_units += u64::from(units);
        t.sold_tokens += u64::from(cost);
        if matches!((buyer, l.seller), (Buyer::Npc(_), Seller::Npc(_))) {
            t.npc_trades += 1;
        }
        let (seller_name, buyer_name) = (self.seller_name(l.seller), self.buyer_name(buyer));
        self.stall_record(StallEvent::SoldAtStall {
            listing: id,
            market: l.market,
            seller: l.seller,
            seller_name,
            buyer,
            buyer_name,
            item: l.item,
            qty: units,
            price,
        });
        Ok(StallSale { units, paid: cost })
    }

    // ------------------------------------------------------------------
    // Player
    // ------------------------------------------------------------------

    /// The player puts `qty` units of `item` from its inventory on a stall
    /// of the Mercato `market` (it must be there) at `price` tokens each
    /// (see [`World::list_for_sale`]). Unsold goods come back after
    /// [`SimParams::listing_days`] (into the chest if the inventory is full).
    /// Selling to the Mercato at once is still [`World::player_sell`].
    pub fn player_list_for_sale(
        &mut self,
        market: CarriageId,
        item: ItemKind,
        qty: u32,
        price: u32,
    ) -> Result<ListingId, StallError> {
        self.list_for_sale(Seller::Player, market, item, qty, price)
    }

    /// The player takes back what fits of its listing `id` (see
    /// [`World::withdraw`]).
    pub fn player_withdraw(&mut self, id: ListingId) -> Result<u32, StallError> {
        if self.listing(id).is_some_and(|l| l.seller != Seller::Player) {
            return Err(StallError::NotYours);
        }
        self.withdraw(id)
    }

    /// The player buys up to `qty` units of listing `id` (see
    /// [`World::buy_listing`]). Buying from an NPC's stall pleases it like
    /// a purchase at its counter ([`super::PURCHASE_AFFINITY`]); buying the
    /// Mercato's goods pleases the Mercante on duty.
    pub fn player_buy_listing(&mut self, id: ListingId, qty: u32) -> Result<StallSale, StallError> {
        self.buy_listing(Buyer::Player, id, qty)
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    /// Open listings of `seller`.
    fn listing_count(&self, seller: Seller) -> usize {
        self.market
            .listings
            .iter()
            .filter(|l| l.seller == seller)
            .count()
    }

    /// Keeps [`Market::stall_count`] in step with the listings.
    fn listings_changed(&mut self) {
        self.market.recount_stalls(self.carriages.len());
    }

    fn listing_index(&self, id: ListingId) -> Result<usize, StallError> {
        self.market
            .listings
            .iter()
            .position(|l| l.id == id)
            .ok_or(StallError::NoSuchListing)
    }

    /// Whether `who` is at the Mercato `market` and can deal there.
    fn check_at_market(&self, who: Seller, market: CarriageId) -> Result<(), StallError> {
        let there = match who {
            Seller::Npc(id) => {
                let n = self.npc(id).ok_or(StallError::NoSuchNpc)?;
                n.carriage == market && !matches!(n.action, Action::Travel { .. }) && n.is_awake()
            }
            Seller::Player => self.player.place.carriage == market && !self.player.is_asleep(),
            Seller::Mercato => true,
        };
        if there {
            Ok(())
        } else {
            Err(StallError::NotThere)
        }
    }

    /// A new listing (the goods already taken from the seller).
    fn open_listing(
        &mut self,
        seller: Seller,
        market: CarriageId,
        item: ItemKind,
        qty: u32,
        price: u32,
    ) -> ListingId {
        let id = ListingId(self.market.next_listing);
        self.market.next_listing += 1;
        self.market.listings.push(Listing {
            id,
            market,
            seller,
            item,
            qty,
            price_each: price.max(1),
            since: self.clock,
        });
        self.listings_changed();
        let t = &mut self.market.trade;
        t.listings += 1;
        t.listed_units += u64::from(qty);
        let seller_name = self.seller_name(seller);
        self.stall_record(StallEvent::Listed {
            listing: id,
            market,
            seller,
            seller_name,
            item,
            qty,
            price,
        });
        id
    }

    /// Takes `units` from listing `k`, closing it when empty.
    fn take_from_listing(&mut self, k: usize, units: u32) {
        let l = &mut self.market.listings[k];
        l.qty -= units.min(l.qty);
        if l.qty == 0 {
            self.market.listings.remove(k);
        }
        self.listings_changed();
    }

    /// Gives up to `units` of `item` back to `seller`'s inventory; returns
    /// how many fit (with `chest`, the player's overflow goes to its chest).
    fn give_back(&mut self, seller: Seller, item: ItemKind, units: u32, chest: bool) -> u32 {
        match seller {
            Seller::Npc(id) => match self.npc_index(id) {
                Some(i) => {
                    let n = self.npcs[i].inventory.items.add(item, units);
                    self.equip_spares(i);
                    n
                }
                None => 0,
            },
            Seller::Player => {
                let p = &mut self.player;
                let mut n = p.inventory.add(item, units);
                if chest {
                    n += p.chest.add(item, units - n);
                }
                self.market.trade.out_of_pool += u64::from(n);
                n
            }
            Seller::Mercato => 0,
        }
    }

    /// Pays `seller` for `units` of `item` at `price` bought by `buyer`.
    fn pay_seller(
        &mut self,
        seller: Seller,
        market: CarriageId,
        item: ItemKind,
        price: u32,
        units: u32,
        buyer: Buyer,
    ) {
        let cost = price.saturating_mul(units);
        match seller {
            Seller::Npc(id) => {
                if let Some(i) = self.npc_index(id) {
                    let tokens = &mut self.npcs[i].inventory.tokens;
                    let kept = cost.min(u32::MAX - *tokens);
                    *tokens += kept;
                    self.deposit(cost - kept);
                    if buyer == Buyer::Player {
                        self.add_player_affinity(i, super::PURCHASE_AFFINITY);
                    }
                } else {
                    self.deposit(cost);
                }
            }
            Seller::Player => self.player.tokens = self.player.tokens.saturating_add(cost),
            Seller::Mercato => {
                // Like a sale from the shelf: into the treasury, logged.
                self.deposit(cost);
                for _ in 0..units {
                    match buyer {
                        Buyer::Npc(b) => {
                            self.economy.counters.purchases += u64::from(price);
                            let name = self.seller_name(Seller::Npc(b));
                            self.log(EventKind::ItemBought {
                                npc: b,
                                name,
                                item,
                                price,
                                carriage: market,
                            });
                        }
                        Buyer::Player => {
                            self.economy.counters.player_purchases += u64::from(price);
                            self.log(EventKind::PlayerBought {
                                item,
                                price,
                                carriage: market,
                            });
                        }
                    }
                }
                if buyer == Buyer::Player
                    && let Some(m) = self.merchant_on_duty(market).map(|m| m.id)
                    && let Some(i) = self.npc_index(m)
                {
                    self.add_player_affinity(i, super::PURCHASE_AFFINITY);
                }
            }
        }
    }

    /// Leaves `units` of `item` to the Mercato `market`: on its shelf what
    /// it deals in and has room for, the rest on its own stall.
    fn leave_to_mercato(&mut self, market: CarriageId, item: ItemKind, units: u32) {
        if units == 0 {
            return;
        }
        self.market.trade.to_mercato_units += u64::from(units);
        let cap = self.params.storage_cap(CarriageKind::Mercato, item);
        let c = &mut self.carriages[market.index()];
        let shelf = if cap > 0.0 {
            let room = (cap - c.stock.get(item)).max(0.0).floor() as u32;
            let n = units.min(room);
            c.stock.add(item, n as f32, cap);
            n
        } else {
            0
        };
        self.market.trade.out_of_pool += u64::from(shelf);
        let rest = units - shelf;
        if rest == 0 {
            return;
        }
        if let Some(l) = self
            .market
            .listings
            .iter_mut()
            .find(|l| l.market == market && l.item == item && l.seller == Seller::Mercato)
        {
            l.qty += rest;
            self.listings_changed();
        } else {
            let quote = self.quote(market, item).unwrap_or(item.base_value());
            self.open_listing(Seller::Mercato, market, item, rest, quote);
        }
    }

    /// Puts up to `units` of `item` into the storage of the carriages that
    /// keep it (its outlet kind first, nearest to `from` first). Returns the
    /// units stored.
    fn store_on_train(&mut self, from: CarriageId, item: ItemKind, units: u32) -> u32 {
        let outlet = item.outlet();
        let mut order: Vec<(bool, u32, usize)> = self
            .carriages
            .iter()
            .filter(|c| self.params.storage_cap(c.kind, item) > 0.0)
            .map(|c| (c.kind != outlet, c.id.distance(from), c.id.index()))
            .collect();
        order.sort_unstable();
        let mut left = units;
        for (_, _, k) in order {
            if left == 0 {
                break;
            }
            let c = &mut self.carriages[k];
            let cap = self.params.storage_cap(c.kind, item);
            let room = (cap - c.stock.get(item)).max(0.0).floor() as u32;
            let n = left.min(room);
            c.stock.add(item, n as f32, cap);
            left -= n;
        }
        let stored = units - left;
        self.market.trade.out_of_pool += u64::from(stored);
        stored
    }

    fn stall_record(&mut self, event: StallEvent) {
        let log = &mut self.market.stall_log;
        log.push_back(StallRecord {
            time: self.clock,
            event,
        });
        while log.len() > STALL_LOG_KEPT {
            log.pop_front();
            self.market.stall_log_dropped += 1;
        }
    }

    // ------------------------------------------------------------------
    // NPCs
    // ------------------------------------------------------------------

    /// NPC `i` puts on a spare Attrezzo or Vestito from its items if it
    /// has none on.
    pub(super) fn equip_spares(&mut self, i: usize) {
        let inv = &mut self.npcs[i].inventory;
        if inv.items.is_empty() {
            return;
        }
        for (item, _) in inv.items.items() {
            let Some(slot) = inv.slot_mut(item) else {
                continue;
            };
            if slot.is_none() {
                *slot = Some(1.0);
                inv.items.remove(item, 1);
                let t = &mut self.market.trade;
                t.equipped_units += 1;
                t.out_of_pool += 1;
            }
        }
    }

    /// NPC `i` eats one unit of its own food, if it has any. Returns whether.
    fn eat_own_food(&mut self, i: usize) -> bool {
        let food = self.npcs[i]
            .inventory
            .items
            .items()
            .into_iter()
            .map(|(item, _)| item)
            .find(|item| item.def().usage == ItemUse::Food);
        let Some(item) = food else {
            return false;
        };
        self.npcs[i].inventory.items.remove(item, 1);
        self.gift_effect(i, item);
        let t = &mut self.market.trade;
        t.eaten_units += 1;
        t.out_of_pool += 1;
        true
    }

    /// NPC `i`, at home, leaves there the comfort goods its Dormitorio lacks.
    fn furnish_home(&mut self, i: usize) {
        let npc = &self.npcs[i];
        let home = npc.home;
        if npc.carriage != home || npc.inventory.items.is_empty() {
            return;
        }
        let kind = self.carriages[home.index()].kind;
        for (item, n) in npc.inventory.items.items() {
            if !self.market.short_goods.contains(&(home, item)) {
                continue;
            }
            let cap = self.params.storage_cap(kind, item);
            let c = &mut self.carriages[home.index()];
            let room = (cap - c.stock.get(item)).max(0.0).floor() as u32;
            let put = n.min(room);
            if put == 0 {
                continue;
            }
            c.stock.add(item, put as f32, cap);
            self.npcs[i].inventory.items.remove(item, put);
            let t = &mut self.market.trade;
            t.furnished_units += u64::from(put);
            t.out_of_pool += u64::from(put);
        }
    }

    /// NPC `i` just finished an action: at a Mercato it lists what it
    /// doesn't need; at home it leaves the comfort goods its home lacks.
    pub(super) fn after_action(&mut self, i: usize) {
        if self.npcs[i].inventory.items.is_empty() {
            return;
        }
        let here = self.npcs[i].carriage;
        if here == self.npcs[i].home {
            self.furnish_home(i);
        }
        if self.carriages[here.index()].kind == CarriageKind::Mercato {
            self.list_surplus(i);
        }
    }

    /// NPC `i`, at a Mercato by day, lists everything it could sell (one
    /// listing per item, not already on a stall there, unless the stalls
    /// there already offer a full stack of it) at its asking price.
    fn list_surplus(&mut self, i: usize) {
        let p = &self.params;
        let npc = &self.npcs[i];
        if p.is_night(self.clock.hour()) || npc.age < LifeStage::GIOVANE_FROM {
            return;
        }
        let (id, market) = (npc.id, npc.carriage);
        for (item, n) in sellable(&self.market, npc) {
            if !listable_at(&self.market, id, market, item) {
                continue;
            }
            let Some(quote) = self.quote(market, item) else {
                return;
            };
            let price = ask_price(&self.params, &self.npcs[i], quote);
            if self
                .list_for_sale(Seller::Npc(id), market, item, n, price)
                .is_err()
            {
                return;
            }
        }
    }

    /// NPC `i` pays for one `item` at its (Mercato) carriage, at the
    /// cheapest offer (validated before: it can pay), and uses it: a tool or
    /// clothes are put on, food is eaten, anything else is kept.
    pub(super) fn npc_buy(&mut self, i: usize, item: ItemKind) {
        let here = self.npcs[i].carriage;
        let c = &self.carriages[here.index()];
        let id = self.npcs[i].id;
        let offer = best_offer(
            &self.params,
            self.economy.pay_level,
            &self.market,
            c,
            item,
            id,
        );
        match offer {
            Some((_, Source::Listing(k))) => {
                let listing = self.market.listings[k].id;
                if self.buy_listing(Buyer::Npc(id), listing, 1).is_ok()
                    && item.def().usage == ItemUse::Food
                {
                    self.eat_own_food(i);
                }
            }
            // The Mercato's shelf (as before stalls existed).
            _ => self.buy_from_shelf(i, item),
        }
    }

    /// Worker NPC `i` made `made` units of `item` in `here`: it keeps
    /// [`SimParams::own_share`] of them (whole units, the fraction drawn).
    pub(super) fn keep_own_share(&mut self, i: usize, here: CarriageId, item: ItemKind, made: f32) {
        let p = &self.params;
        let expected = (made * p.own_share).max(0.0);
        if expected <= 0.0 {
            return;
        }
        let mut units = expected.floor() as u32;
        if self.trade_rng.random::<f32>() < expected.fract() {
            units += 1;
        }
        if units == 0 {
            return;
        }
        let id = self.npcs[i].id;
        let items = &self.npcs[i].inventory.items;
        let listed: u32 = self
            .market
            .listings
            .iter()
            .filter(|l| l.seller == Seller::Npc(id) && l.item == item)
            .map(|l| l.qty)
            .sum();
        let held = items.count(item) + listed;
        let units = units
            .min(p.own_share_max_units.saturating_sub(held))
            .min(items.room_for(item))
            .min(self.carriages[here.index()].stock.count(item));
        if units == 0 {
            return;
        }
        self.carriages[here.index()].stock.take(item, units as f32);
        self.npcs[i].inventory.items.add(item, units);
        let t = &mut self.market.trade;
        t.own_share_units += u64::from(units);
        t.into_pool += u64::from(units);
        self.equip_spares(i);
    }

    /// Whether NPC `i` keeps `item`, a non-food gift it does not use.
    pub(super) fn keeps_gift(&self, i: usize, item: ItemKind) -> bool {
        let n = &self.npcs[i];
        item.def().usage == ItemUse::Material
            && item.base_value() >= self.params.gift_keep_min_value
            && n.inventory.items.room_for(item) > 0
    }

    /// NPC `i` keeps one `item` given by the player.
    pub(super) fn keep_gift(&mut self, i: usize, item: ItemKind) {
        if self.npcs[i].inventory.items.add(item, 1) == 1 {
            let t = &mut self.market.trade;
            t.gifts_kept += 1;
            t.into_pool += 1;
        }
    }

    /// A dead NPC's goods: its listings to the Mercato; its items to its
    /// partner or else its children (what they can carry), then into its
    /// home's storage (what it can hold), the rest to the nearest Mercato.
    pub(super) fn estate_goods(&mut self, npc: &Npc) {
        let seller = Seller::Npc(npc.id);
        let mut k = 0;
        while k < self.market.listings.len() {
            if self.market.listings[k].seller == seller {
                let l = self.market.listings.remove(k);
                self.listings_changed();
                self.leave_to_mercato(l.market, l.item, l.qty);
            } else {
                k += 1;
            }
        }
        let home = npc.home;
        let kind = self.carriages[home.index()].kind;
        let market = self.nearest_market(home);
        let heirs: Vec<usize> = match npc.partner().and_then(|p| self.npc_index(p)) {
            Some(p) => vec![p],
            None => npc.children().filter_map(|c| self.npc_index(c)).collect(),
        };
        for (item, mut n) in npc.inventory.items.items() {
            for &h in &heirs {
                n -= self.npcs[h].inventory.items.add(item, n);
                self.equip_spares(h);
            }
            let cap = self.params.storage_cap(kind, item);
            let c = &mut self.carriages[home.index()];
            let room = (cap - c.stock.get(item)).max(0.0).floor() as u32;
            let stored = n.min(room);
            c.stock.add(item, stored as f32, cap);
            let t = &mut self.market.trade;
            t.estate_units += u64::from(stored);
            t.out_of_pool += u64::from(stored);
            let rest = n - stored;
            match market {
                Some(m) => self.leave_to_mercato(m, item, rest),
                // No Mercato on the train: nowhere to keep it.
                None => self.market.trade.out_of_pool += u64::from(rest),
            }
        }
    }

    /// Hourly: which Dormitori lack which comfort goods; expired listings go
    /// back; the Mercato's listings follow the quote; NPCs eat their own
    /// food when hungry and bring home what their home lacks.
    pub(super) fn stalls_hour(&mut self) {
        self.refresh_short_goods();
        let now = self.clock;
        let mut k = 0;
        while k < self.market.listings.len() {
            let l = &self.market.listings[k];
            if l.expires(&self.params) <= now {
                let l = self.market.listings.remove(k);
                self.listings_changed();
                self.expire(l);
            } else {
                k += 1;
            }
        }
        let (p, level) = (&self.params, self.economy.pay_level);
        let quotes: Vec<(usize, u32)> = self
            .market
            .listings
            .iter()
            .enumerate()
            .filter(|(_, l)| l.seller == Seller::Mercato)
            .map(|(k, l)| {
                let c = &self.carriages[l.market.index()];
                (k, listing_price(p, level, &self.market, c, l))
            })
            .collect();
        for (k, price) in quotes {
            self.market.listings[k].price_each = price;
        }
        let hunger = self.params.eat_own_food_hunger;
        for i in 0..self.npcs.len() {
            let n = &self.npcs[i];
            if n.inventory.items.is_empty() || !n.is_awake() {
                continue;
            }
            if n.needs.hunger < hunger {
                self.eat_own_food(i);
            }
            self.furnish_home(i);
        }
    }

    /// An expired listing goes back to its seller (what does not fit, or
    /// has no seller, to the Mercato). The Mercato's own goods go to the
    /// storage of the nearest carriages that keep them; what finds no room
    /// stays on its stall a while longer.
    fn expire(&mut self, mut l: Listing) {
        if l.seller == Seller::Mercato {
            let stored = self.store_on_train(l.market, l.item, l.qty);
            let rest = l.qty - stored;
            self.stall_record(StallEvent::ListingExpired {
                listing: l.id,
                market: l.market,
                seller: l.seller,
                seller_name: self.seller_name(l.seller),
                item: l.item,
                qty: l.qty,
                to_mercato: rest,
            });
            if rest > 0 {
                l.qty = rest;
                l.since = self.clock;
                self.market.listings.push(l);
                self.listings_changed();
            }
            return;
        }
        let back = self.give_back(l.seller, l.item, l.qty, true);
        let rest = l.qty - back;
        self.market.trade.expired_units += u64::from(back);
        let seller_name = self.seller_name(l.seller);
        self.stall_record(StallEvent::ListingExpired {
            listing: l.id,
            market: l.market,
            seller: l.seller,
            seller_name,
            item: l.item,
            qty: l.qty,
            to_mercato: rest,
        });
        self.leave_to_mercato(l.market, l.item, rest);
    }

    /// (Dormitorio, item) pairs whose comfort good is a whole unit short of
    /// the administration's target.
    fn refresh_short_goods(&mut self) {
        let mut short = Vec::new();
        let dorms: Vec<CarriageId> = self
            .carriages
            .iter()
            .filter(|c| c.kind == CarriageKind::Dormitorio)
            .map(|c| c.id)
            .collect();
        let (residents, children) = self.households();
        for item in ItemKind::ALL {
            if item
                .amenity()
                .is_none_or(|a| a.place() != CarriageKind::Dormitorio)
            {
                continue;
            }
            for &d in &dorms {
                let stock = self.carriages[d.index()].stock.get(item);
                let target = self.furnishing_target_of(d, item, &residents, &children);
                if target - stock >= 1.0 {
                    short.push((d, item));
                }
            }
        }
        self.market.short_goods = short;
    }
}
