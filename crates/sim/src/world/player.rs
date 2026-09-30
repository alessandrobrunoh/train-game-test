//! Il giocatore nella sim: posto, inventario a slot, acquisti, regali,
//! prelievi, baule, sonno, affinità degli NPC e saluti.
//!
//! The game calls these to act on the world (see [`crate::player`] for the
//! model). None of them uses randomness, so the world stays deterministic
//! given the same sequence of calls; tokens only move between the player
//! and the treasury, so [`World::money_supply`] is constant.
//!
//! Affinity with the player ([`crate::Npc::player`]):
//! - a gift accepted: [`GIFT_AFFINITY`] (twice for an Attrezzo or Vestito);
//! - a purchase at the NPC's counter: [`PURCHASE_AFFINITY`] to the Mercante;
//! - a theft seen (taking from a carriage's storage in front of the people
//!   on its ground floor): [`THEFT_SEEN_AFFINITY`] to each of them.
//!
//! Effects: the Mercante on duty's affinity moves the player's prices by up
//! to [`PRICE_PER_AFFINITY`] (discount for friends, markup for who distrusts);
//! a [`Regard::Wary`] NPC refuses gifts; NPCs who like the player greet it
//! when they are idle in the same place ([`Greeting`], at most every
//! [`GREET_COOLDOWN_MINUTES`]) and step close for [`GREET_MINUTES`]; a
//! friend who greets has "something to say" ([`crate::PlayerTie::wants_to_talk`]),
//! and so has who thinks of a favour to ask (see `chat.rs`).

use super::{BuyError, GiveError, World};
use crate::action::Action;
use crate::event::EventKind;
use crate::ids::{CarriageId, NpcId};
use crate::item::ItemKind;
use crate::npc::Npc;
use crate::player::{
    ChestError, Greeting, Place, PlayerTie, Regard, SleepError, clean_player_name,
};
use crate::time::GameTime;

/// Affinity gained by an NPC who accepts a gift (twice for an Attrezzo or Vestito).
pub const GIFT_AFFINITY: f32 = 0.12;
/// Affinity gained by a Mercante when the player buys at its counter.
pub const PURCHASE_AFFINITY: f32 = 0.03;
/// Affinity lost by each NPC who sees the player take from a storage.
pub const THEFT_SEEN_AFFINITY: f32 = -0.1;
/// At affinity ±1 the Mercante's price for the player is this share lower / higher.
pub const PRICE_PER_AFFINITY: f32 = 0.1;
/// NPCs greet the player from this affinity on.
pub const GREET_AFFINITY: f32 = 0.25;
/// An NPC greets the player at most once in this many minutes.
pub const GREET_COOLDOWN_MINUTES: u64 = 8 * 60;
/// How long a greeting lasts (the NPC stays idle near the player meanwhile).
pub const GREET_MINUTES: u64 = 6;
/// Greetings are considered every this many minutes.
const GREET_EVERY: u64 = 5;

impl World {
    // ------------------------------------------------------------------
    // Place and identity
    // ------------------------------------------------------------------

    /// The game tells where the player is (every frame is fine). The
    /// carriage and floor are clamped to the train.
    pub fn set_player_place(&mut self, place: Place) {
        let last = self.carriages.len().saturating_sub(1);
        let carriage = CarriageId(place.carriage.index().min(last) as u16);
        let floors = self.carriage(carriage).map_or(1, |c| c.floors());
        let floor = place.floor.min(floors - 1);
        self.player.place = Place { carriage, floor };
    }

    /// Renames the player (trimmed, see [`clean_player_name`]).
    pub fn set_player_name(&mut self, name: &str) {
        self.player.name = clean_player_name(name);
    }

    /// Whether `npc` is where the player is, close enough to talk (like
    /// [`World::same_place`] between NPCs).
    pub fn with_player(&self, npc: &Npc) -> bool {
        let p = self.player.place;
        npc.carriage == p.carriage
            && npc.floor == p.floor
            && !matches!(npc.action, Action::Travel { .. })
            && !self.player.is_asleep()
    }

    // ------------------------------------------------------------------
    // Affinity
    // ------------------------------------------------------------------

    /// Changes NPC `i`'s affinity with the player by `delta` (a first
    /// meeting creates the tie); any interaction clears "wants to talk".
    pub(super) fn add_player_affinity(&mut self, i: usize, delta: f32) {
        let tie = self.npcs[i].player.get_or_insert_with(PlayerTie::default);
        tie.affinity = (tie.affinity + delta).clamp(-1.0, 1.0);
        tie.wants_to_talk = false;
    }

    /// What the Mercato `carriage` asks the player for one `item` now: its
    /// price, lower or higher by the affinity of the Mercante on duty.
    pub fn player_price(&self, carriage: CarriageId, item: ItemKind) -> Option<u32> {
        let price = self.price(carriage, item)?;
        let affinity = self
            .merchant_on_duty(carriage)
            .map_or(0.0, Npc::player_affinity);
        let factor = 1.0 - PRICE_PER_AFFINITY * affinity.clamp(-1.0, 1.0);
        Some((price as f32 * factor).round().max(1.0) as u32)
    }

    /// Whether NPC `id` has something to tell the player (the game shows a "!").
    pub fn wants_to_talk(&self, id: NpcId) -> bool {
        self.npc(id)
            .and_then(|n| n.player)
            .is_some_and(|t| t.wants_to_talk)
    }

    // ------------------------------------------------------------------
    // Items and tokens
    // ------------------------------------------------------------------

    /// The player takes up to `units` whole units of `item` from the storage
    /// of `carriage` into its inventory (for free, but it is logged and who
    /// is on the ground floor there sees it). Returns the units taken (0 if
    /// the storage has none or the inventory is full).
    pub fn player_take(&mut self, carriage: CarriageId, item: ItemKind, units: u32) -> u32 {
        let Some(c) = self.carriages.get_mut(carriage.index()) else {
            return 0;
        };
        let amount = units
            .min(c.stock.count(item))
            .min(self.player.inventory.room_for(item));
        if amount == 0 {
            return 0;
        }
        c.stock.take(item, amount as f32);
        self.player.inventory.add(item, amount);
        let witnesses: Vec<usize> = (0..self.npcs.len())
            .filter(|&i| {
                let n = &self.npcs[i];
                n.carriage == carriage
                    && n.floor == 0
                    && n.is_awake()
                    && !matches!(n.action, Action::Travel { .. })
            })
            .collect();
        for i in witnesses {
            self.add_player_affinity(i, THEFT_SEEN_AFFINITY);
        }
        self.log(EventKind::PlayerTook {
            item,
            amount,
            carriage,
        });
        amount
    }

    /// The player buys one `item` at the Mercato `carriage`, paying
    /// [`World::player_price`] into the treasury. A Mercante must be at the
    /// counter, and there must be room in the inventory. Returns the price paid.
    pub fn player_buy(&mut self, carriage: CarriageId, item: ItemKind) -> Result<u32, BuyError> {
        let price = self
            .player_price(carriage, item)
            .ok_or(BuyError::NotForSale)?;
        let merchant = self
            .merchant_on_duty(carriage)
            .map(|m| m.id)
            .ok_or(BuyError::NoMerchant)?;
        if self.carriages[carriage.index()].stock.count(item) < 1 {
            return Err(BuyError::OutOfStock);
        }
        if self.player.tokens < price {
            return Err(BuyError::TooExpensive(price));
        }
        if self.player.inventory.room_for(item) < 1 {
            return Err(BuyError::NoRoom);
        }
        self.carriages[carriage.index()].stock.take(item, 1.0);
        self.player.inventory.add(item, 1);
        self.player.tokens -= price;
        self.deposit(price);
        self.economy.counters.player_purchases += u64::from(price);
        if let Some(i) = self.npc_index(merchant) {
            self.add_player_affinity(i, PURCHASE_AFFINITY);
        }
        self.log(EventKind::PlayerBought {
            item,
            price,
            carriage,
        });
        Ok(price)
    }

    /// The player gives one `item` from its inventory to `npc`, if it
    /// accepts it ([`Npc::accepts_gift`], and it does not distrust the
    /// player). Food feeds (a Razione like a meal, raw Verdura half as
    /// much); a pot of Tè gives twice what a cup at a meal does; an Attrezzo
    /// or Vestito arrives new. Other goods worth at least
    /// [`crate::SimParams::gift_keep_min_value`] (materials, comfort goods)
    /// are kept in its [`crate::Inventory::items`], if there is room, to sell
    /// or bring home. The NPC likes the player more. On error nothing
    /// changes and the player keeps the item.
    pub fn player_give(&mut self, npc: NpcId, item: ItemKind) -> Result<(), GiveError> {
        let i = self.npc_index(npc).ok_or(GiveError::NoSuchNpc)?;
        if !self.player.inventory.has(item, 1) {
            return Err(GiveError::NotOwned);
        }
        if self.npcs[i].regard() == Regard::Wary {
            return Err(GiveError::Distrust);
        }
        let keep = !self.npcs[i].accepts_gift(item) && self.keeps_gift(i, item);
        if !self.npcs[i].accepts_gift(item) && !keep {
            return Err(GiveError::NotWanted);
        }
        if keep {
            // Something it does not use: kept, to sell (see `stalls.rs`).
            self.keep_gift(i, item);
        } else {
            self.gift_effect(i, item);
        }
        let name = self.npcs[i].name.clone();
        self.player.inventory.remove(item, 1);
        let gain = if item.usage().is_owned() {
            2.0 * GIFT_AFFINITY
        } else {
            GIFT_AFFINITY
        };
        self.add_player_affinity(i, gain);
        self.log(EventKind::PlayerGave { npc, name, item });
        Ok(())
    }

    /// What NPC `i` gets from one `item` the player hands over: food feeds,
    /// Tè refreshes, an owned good arrives new (see [`World::player_give`]).
    pub(super) fn gift_effect(&mut self, i: usize, item: ItemKind) {
        let consume = self.catalog.get_item(item).and_then(|d| d.consume);
        let p = &self.params;
        let target = &mut self.npcs[i];
        match consume {
            // Food and drinks, from the catalog ([`crate::Consume`]).
            Some(c) => {
                let (hunger, energy, social) = (c.hunger.get(p), c.energy.get(p), c.social.get(p));
                let needs = &mut target.needs;
                if hunger > 0.0 {
                    needs.hunger = (needs.hunger + hunger).min(1.0);
                    target.starving_minutes = 0;
                }
                if energy > 0.0 {
                    needs.energy = (needs.energy + energy).min(1.0);
                }
                if social > 0.0 {
                    needs.social = (needs.social + social).min(1.0);
                }
            }
            None => {
                if let Some(slot) = target.inventory.slot_mut(item) {
                    *slot = Some(1.0);
                }
            }
        }
    }

    /// Moves the stack in the inventory's `slot` into the chest (what fits).
    /// The player must be in the cabin. Returns the units moved.
    pub fn player_store(&mut self, slot: usize) -> Result<u32, ChestError> {
        self.chest_transfer(slot, true)
    }

    /// Moves the stack in the chest's `slot` into the inventory (what fits).
    pub fn player_retrieve(&mut self, slot: usize) -> Result<u32, ChestError> {
        self.chest_transfer(slot, false)
    }

    fn chest_transfer(&mut self, slot: usize, store: bool) -> Result<u32, ChestError> {
        if !self.player.at_home() {
            return Err(ChestError::NotHome);
        }
        let p = &mut self.player;
        let (from, to) = if store {
            (&mut p.inventory, &mut p.chest)
        } else {
            (&mut p.chest, &mut p.inventory)
        };
        if from.slots().get(slot).is_none_or(Option::is_none) {
            return Err(ChestError::EmptySlot);
        }
        match from.move_slot(slot, to) {
            0 => Err(ChestError::NoRoom),
            n => Ok(n),
        }
    }

    // ------------------------------------------------------------------
    // Sleep
    // ------------------------------------------------------------------

    /// The player goes to bed in its cabin (it must be there, from
    /// [`crate::SimParams::long_sleep_from_hour`] to the wake hour) and
    /// sleeps until the next [`crate::SimParams::wake_hour`]:00, which is
    /// returned. The world keeps running normally: the game fast-forwards it
    /// to that time, then the player wakes up by itself.
    pub fn player_go_to_bed(&mut self) -> Result<GameTime, SleepError> {
        let home = self.player.home.ok_or(SleepError::NoCabin)?;
        if self.player.is_asleep() {
            return Err(SleepError::AlreadyAsleep);
        }
        if self.player.place != home.place() {
            return Err(SleepError::NotHome);
        }
        if !self.params.is_long_sleep(self.clock.hour()) {
            return Err(SleepError::TooEarly);
        }
        let wake = self.clock.next_at(self.params.wake_hour, 0);
        self.player.asleep_until = Some(wake);
        self.player.greetings.clear();
        Ok(wake)
    }

    /// Wakes the player up now (e.g. the game interrupts the sleep).
    pub fn player_wake_up(&mut self) {
        self.player.asleep_until = None;
    }

    // ------------------------------------------------------------------
    // Every tick
    // ------------------------------------------------------------------

    /// Lines NPCs are saying to the player right now.
    pub fn greetings(&self) -> &[Greeting] {
        &self.player.greetings
    }

    /// The greeting NPC `id` is saying to the player, if any.
    pub fn greeting_of(&self, id: NpcId) -> Option<&Greeting> {
        self.player.greetings.iter().find(|g| g.npc == id)
    }

    /// End of a tick: the player wakes up; every few minutes an NPC who
    /// likes the player and is idle where it is greets it (and may think of
    /// a favour to ask); at midnight the "wants to talk" flags are cleared
    /// (except for favours still to tell) and expired favours dropped.
    pub(super) fn player_tick(&mut self) {
        let now = self.clock;
        if self.player.asleep_until.is_some_and(|t| t <= now + 1) {
            self.player.asleep_until = None;
        }
        self.player.greetings.retain(|g| g.until > now);
        if now.minute_of_day() == 0 {
            self.chat_midnight();
        }
        if now.minutes().is_multiple_of(GREET_EVERY) && !self.player.is_asleep() {
            self.greet_player();
        }
    }

    /// The idle NPC here who likes the player most (and has not greeted it
    /// lately) greets it and stays idle a little longer, near it.
    fn greet_player(&mut self) {
        let now = self.clock;
        if self.player.greetings.len() >= 2 {
            return;
        }
        let greeter = (0..self.npcs.len())
            .filter(|&i| {
                let n = &self.npcs[i];
                n.action == Action::Idle
                    && n.player.is_some_and(|t| {
                        t.affinity >= GREET_AFFINITY
                            && t.greeted
                                .is_none_or(|g| now.since(g) >= GREET_COOLDOWN_MINUTES)
                    })
                    && self.with_player(n)
                    && self.greeting_of(n.id).is_none()
            })
            .max_by(|&a, &b| {
                let (x, y) = (&self.npcs[a], &self.npcs[b]);
                x.player_affinity()
                    .total_cmp(&y.player_affinity())
                    .then(y.id.cmp(&x.id))
            });
        let Some(i) = greeter else {
            return;
        };
        let text = greeting_text(&self.npcs[i], self.player.first_name(), now);
        let npc = &mut self.npcs[i];
        npc.action_until = npc.action_until.max(now + GREET_MINUTES);
        let friend = npc.regard() == Regard::Friend;
        if let Some(t) = &mut npc.player {
            t.greeted = Some(now);
            t.wants_to_talk = friend;
        }
        self.player.greetings.push(Greeting {
            npc: npc.id,
            text,
            since: now,
            until: now + GREET_MINUTES,
        });
        self.maybe_offer_favour(i);
        // A friend in a gang may invite the player (see `gang.rs`).
        if !self.gangs.list.is_empty() {
            self.maybe_invite_player(i);
        }
    }
}

/// What an NPC says to greet the player: warmer from a friend, by the hour,
/// varied per NPC and day (no randomness).
fn greeting_text(npc: &Npc, player: &str, now: GameTime) -> String {
    let hello = match now.hour() {
        5..=11 => "Buongiorno",
        12..=17 => "Buon pomeriggio",
        _ => "Buonasera",
    };
    let pick = (u64::from(npc.id.0).wrapping_mul(31) + now.day()) as usize;
    match npc.regard() {
        Regard::Friend => {
            let lines = [
                format!("Ciao, {player}!"),
                format!("Ehi, {player}! Che bello vederti."),
                format!("{player}! Tutto bene?"),
                format!("{hello}, {player}! Come va?"),
            ];
            lines[pick % lines.len()].clone()
        }
        _ => {
            let lines = [
                format!("{hello}, {player}."),
                format!("Salve, {player}."),
                format!("Oh, {player}. {hello}."),
            ];
            lines[pick % lines.len()].clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defs::ItemUse;

    #[test]
    fn greetings_depend_on_the_regard() {
        let mut w = World::generate(1, 5, 20);
        let npc = &mut w.npcs[0];
        npc.player = Some(PlayerTie {
            affinity: 0.9,
            ..PlayerTie::default()
        });
        let friend = greeting_text(&w.npcs[0], "Ada", GameTime::from_dhm(1, 9, 0));
        assert!(friend.contains("Ada"), "{friend}");
        w.npcs[0].player = Some(PlayerTie {
            affinity: 0.3,
            ..PlayerTie::default()
        });
        let polite = greeting_text(&w.npcs[0], "Ada", GameTime::from_dhm(1, 20, 0));
        assert!(polite.contains("Ada"), "{polite}");
        assert!(!polite.contains('!'), "{polite}");
    }

    #[test]
    fn items_owned_by_the_player_are_used_for_gifts() {
        let mut w = World::generate(1, 5, 20);
        let id = w.npcs[0].id;
        w.npcs[0].needs.hunger = 0.1;
        assert_eq!(
            w.player_give(id, ItemKind::Razione),
            Err(GiveError::NotOwned)
        );
        assert!(ItemUse::Food == ItemKind::Razione.usage());
    }
}
