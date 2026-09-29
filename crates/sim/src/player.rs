//! Il giocatore come personaggio del treno (Fase 4 di
//! `docs/piano-vita-ed-economia.md`).
//!
//! The player lives in the world ([`crate::World::player`]) like the NPCs'
//! state does, and is saved with it:
//! - **Place.** The game owns the physics (the platformer body) and tells the
//!   sim, every frame, only the carriage and floor the player is in
//!   ([`crate::World::set_player_place`]).
//! - **Belongings.** Tokens (part of the money supply: the sim's money is
//!   fully conserved, see [`crate::World::money_supply`]), a
//!   [`SlotInventory`] of [`INVENTORY_SLOTS`] slots, a chest in the cabin
//!   ([`CHEST_SLOTS`] slots) and the recipes learnt beyond the basic ones.
//! - **Cabin.** A private bed ([`Cabin`], a station owned by the player that
//!   NPCs never use) upstairs in a Dormitorio, with the chest next to it.
//!   Sleeping there fast-forwards to the morning
//!   ([`crate::World::player_go_to_bed`]).
//! - **Ties.** NPCs keep what they think of the player in
//!   [`crate::Npc::player`] ([`PlayerTie`]), separate from their ties with
//!   each other: gifts and purchases raise it, thefts they witness lower it.
//!   It changes the price at their counter, whether they accept gifts, and
//!   whether they greet the player ([`Greeting`]), how they answer in the
//!   chat ([`crate::chat`]) and the favours they ask.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::chat::{ChatLog, Favour};
use crate::defs::RecipeDef;
use crate::ids::{CarriageId, NpcId, StationId};
use crate::item::ItemKind;
use crate::npc::{Job, Needs, Sex};
use crate::time::GameTime;

/// Slots of the player's inventory.
pub const INVENTORY_SLOTS: usize = 12;
/// Slots of the chest in the player's cabin.
pub const CHEST_SLOTS: usize = 24;
/// Tokens the player starts with.
pub const PLAYER_START_TOKENS: u32 = 100;
/// Name of a new player.
pub const DEFAULT_PLAYER_NAME: &str = "Viaggiatore";
/// Longest player name (characters).
pub const MAX_PLAYER_NAME_CHARS: usize = 24;

/// Where someone is: a carriage and one of its floors (0 = ground floor).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Place {
    pub carriage: CarriageId,
    pub floor: u8,
}

/// The player's cabin: a private bed (a [`crate::Station`] owned by the
/// player) in a Dormitorio; the chest stands next to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Cabin {
    pub carriage: CarriageId,
    pub bed: StationId,
    pub floor: u8,
}

impl Cabin {
    pub fn place(&self) -> Place {
        Place {
            carriage: self.carriage,
            floor: self.floor,
        }
    }
}

/// Some units of one item in a slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ItemStack {
    pub item: ItemKind,
    /// `1..=item.stack_size()`.
    pub count: u32,
}

/// A fixed number of slots, each empty or holding a stack of one item (at
/// most [`ItemKind::stack_size`] units, read from the item catalog): the
/// player's inventory and chest. It knows nothing about specific items: a
/// slot is an item id and a count, so a catalog that grows during the game
/// needs no change here.
/// Adding fills the existing stacks of the item first, then the first empty
/// slots; removing takes from the last stacks first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotInventory {
    slots: Vec<Option<ItemStack>>,
}

impl Default for SlotInventory {
    fn default() -> Self {
        Self::new(INVENTORY_SLOTS)
    }
}

impl SlotInventory {
    /// `n` empty slots.
    pub fn new(n: usize) -> Self {
        Self {
            slots: vec![None; n],
        }
    }

    pub fn slots(&self) -> &[Option<ItemStack>] {
        &self.slots
    }

    /// Number of slots.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Nothing in any slot.
    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }

    /// Empty slots.
    pub fn free_slots(&self) -> usize {
        self.slots.iter().filter(|s| s.is_none()).count()
    }

    /// Units of `item` over all slots.
    pub fn count(&self, item: ItemKind) -> u32 {
        self.slots
            .iter()
            .flatten()
            .filter(|s| s.item == item)
            .map(|s| s.count)
            .sum()
    }

    pub fn has(&self, item: ItemKind, units: u32) -> bool {
        self.count(item) >= units
    }

    /// Units of `item` that still fit: room left in its stacks plus the
    /// empty slots.
    pub fn room_for(&self, item: ItemKind) -> u32 {
        let stack = item.stack_size().max(1);
        self.slots
            .iter()
            .map(|s| match s {
                None => stack,
                Some(s) if s.item == item => stack.saturating_sub(s.count),
                Some(_) => 0,
            })
            .sum()
    }

    /// Adds up to `units` of `item`; returns how many fit.
    pub fn add(&mut self, item: ItemKind, units: u32) -> u32 {
        let stack = item.stack_size().max(1);
        let mut left = units;
        for s in self.slots.iter_mut().flatten() {
            if left == 0 {
                break;
            }
            if s.item == item && s.count < stack {
                let n = left.min(stack - s.count);
                s.count += n;
                left -= n;
            }
        }
        for slot in &mut self.slots {
            if left == 0 {
                break;
            }
            if slot.is_none() {
                let n = left.min(stack);
                *slot = Some(ItemStack { item, count: n });
                left -= n;
            }
        }
        units - left
    }

    /// Removes up to `units` of `item` (last stacks first); returns how many.
    pub fn remove(&mut self, item: ItemKind, units: u32) -> u32 {
        let mut left = units;
        for slot in self.slots.iter_mut().rev() {
            if left == 0 {
                break;
            }
            if let Some(s) = slot
                && s.item == item
            {
                let n = left.min(s.count);
                s.count -= n;
                left -= n;
                if s.count == 0 {
                    *slot = None;
                }
            }
        }
        units - left
    }

    /// Moves as much as fits of the stack in `slot` into `to`; returns the
    /// units moved (0 for an empty or missing slot, or no room).
    pub fn move_slot(&mut self, slot: usize, to: &mut SlotInventory) -> u32 {
        let Some(Some(stack)) = self.slots.get(slot).copied() else {
            return 0;
        };
        let moved = to.add(stack.item, stack.count);
        let s = &mut self.slots[slot];
        if moved >= stack.count {
            *s = None;
        } else if let Some(s) = s {
            s.count -= moved;
        }
        moved
    }

    /// The distinct items held, in slot order, with their total units.
    pub fn items(&self) -> Vec<(ItemKind, u32)> {
        let mut out: Vec<(ItemKind, u32)> = Vec::new();
        for s in self.slots.iter().flatten() {
            match out.iter_mut().find(|(item, _)| *item == s.item) {
                Some((_, n)) => *n += s.count,
                None => out.push((s.item, s.count)),
            }
        }
        out
    }
}

/// A line an NPC says to the player (shown as a speech bubble by the game).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Greeting {
    pub npc: NpcId,
    pub text: String,
    pub since: GameTime,
    pub until: GameTime,
}

/// What an NPC thinks of the player ([`crate::Npc::player`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PlayerTie {
    /// `-1..=1`: raised by gifts and purchases at the NPC's counter,
    /// lowered by thefts the NPC saw.
    pub affinity: f32,
    /// Last time the NPC greeted the player.
    pub greeted: Option<GameTime>,
    /// The NPC has something to tell the player: the game draws a "!" over
    /// its head. A friend who just greeted the player, or someone with a
    /// favour to ask ([`PlayerTie::favour`] not told yet); it speaks first
    /// when the player opens the chat. Cleared by any interaction with the
    /// player and at midnight (kept for a favour still to tell).
    pub wants_to_talk: bool,
    /// Last greeting in the chat that raised the affinity (see
    /// [`crate::chat::CHAT_BONUS_COOLDOWN_MINUTES`]).
    #[serde(default)]
    pub chat_bonus_at: Option<GameTime>,
    /// Last insult that lowered the affinity (see
    /// [`crate::chat::INSULT_COOLDOWN_MINUTES`]).
    #[serde(default)]
    pub insulted_at: Option<GameTime>,
    /// The errand the NPC asked the player, if any (see [`Favour`]).
    #[serde(default)]
    pub favour: Option<Favour>,
    /// When the NPC last offered a favour on its own.
    #[serde(default)]
    pub favour_offered_at: Option<GameTime>,
}

/// How an NPC regards the player, from its [`PlayerTie`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Regard {
    /// Has never had to do with the player.
    Stranger,
    /// Affinity below [`Regard::WARY_BELOW`]: refuses gifts, charges more.
    Wary,
    Acquaintance,
    /// Affinity from [`Regard::FRIEND_FROM`]: greets warmly, gives discounts.
    Friend,
}

impl Regard {
    pub const WARY_BELOW: f32 = -0.2;
    pub const FRIEND_FROM: f32 = 0.5;

    pub fn of(tie: Option<&PlayerTie>) -> Regard {
        match tie {
            None => Regard::Stranger,
            Some(t) if t.affinity < Self::WARY_BELOW => Regard::Wary,
            Some(t) if t.affinity >= Self::FRIEND_FROM => Regard::Friend,
            Some(_) => Regard::Acquaintance,
        }
    }

    /// "amica", "conoscente", "diffidente", "non ti conosce" (for "Ti considera: …").
    pub fn label(self, sex: Sex) -> &'static str {
        match self {
            Regard::Stranger => "non ti conosce",
            Regard::Wary => "diffidente",
            Regard::Acquaintance => "conoscente",
            Regard::Friend => sex.pick("amica", "amico"),
        }
    }
}

/// The player, a character of the train ([`crate::World::player`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerCharacter {
    pub name: String,
    /// Where the player is (synced by the game, see
    /// [`crate::World::set_player_place`]).
    pub place: Place,
    /// The player's cabin (None on a train without a Dormitorio).
    pub home: Option<Cabin>,
    pub inventory: SlotInventory,
    /// Personal storage in the cabin.
    pub chest: SlotInventory,
    /// Tokens: part of [`crate::World::money_supply`].
    pub tokens: u32,
    /// Keys ([`RecipeDef::key`]) of the recipes learnt beyond the basic ones.
    pub known_recipes: Vec<String>,
    /// Reserved: the job the player took (Fase 4.3).
    pub job: Option<Job>,
    /// Reserved: needs in "survival" mode (off: None, the default).
    pub needs: Option<Needs>,
    /// Sleeping in the cabin until then (see [`crate::World::player_go_to_bed`]).
    pub asleep_until: Option<GameTime>,
    /// Lines NPCs are saying to the player right now, oldest first.
    pub greetings: Vec<Greeting>,
    /// What the player and the NPCs said to each other in the chat, one log
    /// per NPC (at most [`crate::chat::CHAT_LOGS_KEPT`], the most recent last).
    pub chats: Vec<ChatLog>,
}

impl Default for PlayerCharacter {
    fn default() -> Self {
        Self {
            name: DEFAULT_PLAYER_NAME.to_string(),
            place: Place::default(),
            home: None,
            inventory: SlotInventory::new(INVENTORY_SLOTS),
            chest: SlotInventory::new(CHEST_SLOTS),
            tokens: PLAYER_START_TOKENS,
            known_recipes: Vec::new(),
            job: None,
            needs: None,
            asleep_until: None,
            greetings: Vec::new(),
            chats: Vec::new(),
        }
    }
}

impl PlayerCharacter {
    /// Whether the player can make `recipe`: the basic ones, and those learnt.
    pub fn knows(&self, recipe: &RecipeDef) -> bool {
        recipe.basic || self.known_recipes.iter().any(|k| k == recipe.key)
    }

    /// Learns a recipe; false if it was already known.
    pub fn learn(&mut self, recipe: &RecipeDef) -> bool {
        if self.knows(recipe) {
            return false;
        }
        self.known_recipes.push(recipe.key.to_string());
        true
    }

    pub fn is_asleep(&self) -> bool {
        self.asleep_until.is_some()
    }

    /// Whether the player is in the cabin (its carriage and floor).
    pub fn at_home(&self) -> bool {
        self.home.is_some_and(|h| h.place() == self.place)
    }

    /// The first name (up to the first space), for greetings.
    pub fn first_name(&self) -> &str {
        self.name.split_whitespace().next().unwrap_or(&self.name)
    }
}

/// A player name as typed: trimmed, at most [`MAX_PLAYER_NAME_CHARS`]
/// characters, [`DEFAULT_PLAYER_NAME`] if empty.
pub fn clean_player_name(name: &str) -> String {
    let name: String = name
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_PLAYER_NAME_CHARS)
        .collect();
    let name = name.trim().to_string();
    if name.is_empty() {
        DEFAULT_PLAYER_NAME.to_string()
    } else {
        name
    }
}

/// Why [`crate::World::player_go_to_bed`] failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SleepError {
    /// The train has no cabin for the player.
    NoCabin,
    /// Not in the cabin.
    NotHome,
    /// Before [`crate::SimParams::long_sleep_from_hour`] (and after waking).
    TooEarly,
    AlreadyAsleep,
}

impl fmt::Display for SleepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SleepError::NoCabin => f.write_str("non hai una cabina"),
            SleepError::NotHome => f.write_str("puoi dormire solo nel tuo letto"),
            SleepError::TooEarly => f.write_str("è troppo presto per dormire"),
            SleepError::AlreadyAsleep => f.write_str("stai già dormendo"),
        }
    }
}

/// Why a transfer between the inventory and the chest failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChestError {
    /// Not in the cabin.
    NotHome,
    /// Nothing in that slot.
    EmptySlot,
    /// The other side is full.
    NoRoom,
}

impl fmt::Display for ChestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChestError::NotHome => f.write_str("il baule è nella tua cabina"),
            ChestError::EmptySlot => f.write_str("lo scomparto è vuoto"),
            ChestError::NoRoom => f.write_str("non c'è posto"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stacks_fill_up_then_overflow_into_free_slots() {
        let mut inv = SlotInventory::new(3);
        assert_eq!(
            ItemKind::Rottame.stack_size(),
            crate::defs::DEFAULT_STACK_LIMIT
        );
        assert_eq!(ItemKind::Attrezzo.stack_size(), 1);
        assert_eq!(inv.room_for(ItemKind::Rottame), 30);
        assert_eq!(inv.add(ItemKind::Rottame, 12), 12);
        assert_eq!(
            inv.slots()[..2],
            [
                Some(ItemStack {
                    item: ItemKind::Rottame,
                    count: 10
                }),
                Some(ItemStack {
                    item: ItemKind::Rottame,
                    count: 2
                })
            ]
        );
        // Durable goods: one per slot.
        assert_eq!(inv.add(ItemKind::Attrezzo, 3), 1);
        assert_eq!(inv.free_slots(), 0);
        assert_eq!(inv.room_for(ItemKind::Attrezzo), 0);
        assert_eq!(inv.room_for(ItemKind::Rottame), 8);
        assert_eq!(inv.add(ItemKind::Rottame, 20), 8);
        assert_eq!(inv.count(ItemKind::Rottame), 20);
        assert_eq!(inv.add(ItemKind::Verdura, 1), 0);
        // Removing empties the last stacks first.
        assert_eq!(inv.remove(ItemKind::Rottame, 11), 11);
        assert_eq!(inv.slots()[1], None);
        assert_eq!(inv.count(ItemKind::Rottame), 9);
        assert_eq!(inv.remove(ItemKind::Te, 1), 0);
        assert_eq!(
            inv.items(),
            [(ItemKind::Rottame, 9), (ItemKind::Attrezzo, 1)]
        );
        assert!(!inv.is_empty());
    }

    #[test]
    fn move_slot_moves_what_fits() {
        let mut inv = SlotInventory::new(2);
        let mut chest = SlotInventory::new(1);
        inv.add(ItemKind::Verdura, 10);
        inv.add(ItemKind::Te, 4);
        chest.add(ItemKind::Verdura, 7);
        assert_eq!(inv.move_slot(0, &mut chest), 3);
        assert_eq!(inv.count(ItemKind::Verdura), 7);
        assert_eq!(inv.move_slot(1, &mut chest), 0);
        assert_eq!(chest.move_slot(0, &mut inv), 3);
        assert_eq!(inv.count(ItemKind::Verdura), 10);
        assert_eq!(chest.count(ItemKind::Verdura), 7);
        assert_eq!(inv.move_slot(5, &mut chest), 0);
    }

    #[test]
    fn regard_and_names() {
        let tie = |affinity| PlayerTie {
            affinity,
            ..PlayerTie::default()
        };
        assert_eq!(Regard::of(None), Regard::Stranger);
        assert_eq!(Regard::of(Some(&tie(-0.5))), Regard::Wary);
        assert_eq!(Regard::of(Some(&tie(0.0))), Regard::Acquaintance);
        assert_eq!(Regard::of(Some(&tie(0.7))), Regard::Friend);
        assert_eq!(Regard::Friend.label(Sex::Female), "amica");
        assert_eq!(clean_player_name("  Anna   Neve "), "Anna Neve");
        assert_eq!(clean_player_name("   "), DEFAULT_PLAYER_NAME);
        assert_eq!(
            clean_player_name(&"x".repeat(40)).chars().count(),
            MAX_PLAYER_NAME_CHARS
        );
    }
}
