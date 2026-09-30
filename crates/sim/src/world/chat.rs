//! La chat del giocatore nella sim (vedi [`crate::chat`]).
//!
//! Every exchange is applied at once, at the current minute, and uses no
//! randomness (the wording is picked with a seed made of the NPC's id, the
//! clock and how much they have talked): the same calls at the same minutes
//! give the same world.
//!
//! What the NPC knows:
//! - **Work:** its job, workplace and shift.
//! - **Prices:** the Mercati within [`KNOWN_MARKET_REACH`] carriages of its
//!   home, workplace or where it is (a Mercante knows them all), for an item
//!   sold there (one it wants first): the cheapest and the dearest.
//! - **News:** the recent facts of the train (like the conversations between
//!   NPCs), and what the player did: a theft from the storage of a carriage
//!   it lives, works or is in, a gift to someone it knows. Gossips tell
//!   everything, to anyone.
//! - **Favours:** something it needs, from the catalog: an owned good it
//!   lacks, food if hungry, an input of its work that its workplace is short
//!   of. The reward is what the item costs (the cheapest Mercato, else its
//!   base value at the pay level), up to [`FAVOUR_MAX_TOKEN_SHARE`] of the
//!   NPC's tokens; on delivery the NPC pays what it still has, up to that.

use super::{GiveError, World};
use crate::action::Action;
use crate::catalog::Catalog;
use crate::chat::{
    Band, CHAT_BONUS_COOLDOWN_MINUTES, CHAT_GREET_AFFINITY, CHAT_LOGS_KEPT, ChatAction, ChatError,
    ChatLine, ChatLog, ChatReply, FAVOUR_AFFINITY, FAVOUR_DAYS, FAVOUR_MAX_TOKEN_SHARE,
    FAVOUR_OFFER_COOLDOWN_MINUTES, Favour, INSULT_AFFINITY, INSULT_COOLDOWN_MINUTES, Intent,
    IntentReader, MAX_CHAT_INPUT_CHARS, MAX_OPEN_FAVOURS, Speaker, normalize,
};
use crate::defs::ItemUse;
use crate::dialogue::chat::{self as lines, ChatFact, ChatNpc, ChatScene, Deed};
use crate::dialogue::text;
use crate::event::EventKind;
use crate::ids::{CarriageId, NpcId};
use crate::item::ItemKind;
use crate::npc::{Job, LifeStage, Sex};
use crate::personality::Temper;
use crate::time::{GameTime, MINUTES_PER_DAY};

/// Below this hunger (energy) an NPC says it is hungry (tired).
const HUNGRY_BELOW: f32 = 0.3;
const TIRED_BELOW: f32 = 0.25;
/// Mercati an NPC knows: within this many carriages of its home, workplace
/// or where it is (Mercanti know them all).
pub const KNOWN_MARKET_REACH: u32 = 6;
/// NPCs ask food as a favour below this hunger.
const FAVOUR_HUNGER: f32 = 0.5;
/// NPCs ask for an input of their work when their workplace holds less than
/// this share of what it can store.
const FAVOUR_LOW_STOCK: f32 = 0.3;
/// Most units asked for a material.
const FAVOUR_MAX_UNITS: u32 = 3;

/// What the answer is about, owned (the text borrows names from the world
/// only after the effects are applied).
enum Known {
    Nothing,
    Job {
        place: Option<CarriageId>,
        shift: (u32, u32),
    },
    Price {
        item: ItemKind,
        market: CarriageId,
        price: u32,
        other: Option<(CarriageId, u32)>,
        own: bool,
    },
    NoPrices,
    FavourOffer(Favour),
    FavourReminder(Favour),
    FavourDone {
        favour: Favour,
        paid: u32,
    },
    NoFavour,
    Gift {
        accepts: bool,
    },
    Gave(ItemKind),
    Trade {
        merchant: bool,
        at_counter: bool,
        market: Option<CarriageId>,
        opens: u32,
    },
    News(String),
    Deed {
        deed: Deed,
        item: ItemKind,
        place: CarriageId,
        who: Option<(String, Sex)>,
    },
    NoNews,
}

/// Something the player did, as an NPC heard it: what, which item, where,
/// to whom (a gift).
type HeardDeed = (Deed, ItemKind, CarriageId, Option<(String, Sex)>);

/// The effects of an exchange, before the text.
struct Effects {
    known: Known,
    affinity: f32,
    tokens: u32,
    favour: Option<Favour>,
    action: ChatAction,
    repeat: bool,
}

impl Effects {
    fn of(known: Known) -> Effects {
        Effects {
            known,
            affinity: 0.0,
            tokens: 0,
            favour: None,
            action: ChatAction::None,
            repeat: false,
        }
    }
}

impl World {
    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    /// What the player and NPC `npc` said to each other (the last
    /// [`crate::chat::CHAT_MEMORY_LINES`] lines, oldest first).
    pub fn chat_log(&self, npc: NpcId) -> &[ChatLine] {
        self.player
            .chats
            .iter()
            .find(|l| l.npc == npc)
            .map_or(&[], |l| l.lines.as_slice())
    }

    /// The favour NPC `npc` asked the player (or is waiting to ask), if any.
    pub fn player_favour(&self, npc: NpcId) -> Option<Favour> {
        self.npc(npc).and_then(|n| n.player).and_then(|t| t.favour)
    }

    /// Favours open now, by NPC id.
    pub fn open_favours(&self) -> Vec<(NpcId, Favour)> {
        self.npcs
            .iter()
            .filter_map(|n| Some((n.id, n.player?.favour?)))
            .collect()
    }

    /// Whether the player can talk to NPC `npc` now.
    pub fn can_chat(&self, npc: NpcId) -> Result<(), ChatError> {
        self.chat_index(npc).map(|_| ())
    }

    /// What in the player's inventory NPC `npc` would accept as a gift, in
    /// slot order (empty if it distrusts the player).
    pub fn chat_gift_options(&self, npc: NpcId) -> Vec<ItemKind> {
        let Some(i) = self.npc_index(npc) else {
            return Vec::new();
        };
        let n = &self.npcs[i];
        if Band::of(n.player.as_ref()) == Band::Low {
            return Vec::new();
        }
        self.player
            .inventory
            .items()
            .into_iter()
            .map(|(item, _)| item)
            .filter(|&item| n.accepts_gift(item) || self.keeps_gift(i, item))
            .collect()
    }

    fn chat_index(&self, npc: NpcId) -> Result<usize, ChatError> {
        let i = self.npc_index(npc).ok_or(ChatError::NoSuchNpc)?;
        if self.player.is_asleep() {
            return Err(ChatError::PlayerAsleep);
        }
        let n = &self.npcs[i];
        if !n.is_awake() {
            return Err(ChatError::Asleep);
        }
        if !self.with_player(n) {
            return Err(ChatError::Away);
        }
        Ok(i)
    }

    // ------------------------------------------------------------------
    // Talking
    // ------------------------------------------------------------------

    /// The player opens the chat with NPC `npc`. If the NPC has something to
    /// say (a favour to ask, or a friend's "!") it speaks first: the line is
    /// returned and remembered, and the "!" goes away.
    pub fn player_chat_start(&mut self, npc: NpcId) -> Result<Option<String>, ChatError> {
        let i = self.chat_index(npc)?;
        // A gang's invitation or its pizzo come first (see `gang.rs`).
        if !self.gangs.list.is_empty()
            && let Some(text) = self.gang_opening(i)
        {
            if let Some(t) = &mut self.npcs[i].player {
                t.wants_to_talk = false;
            }
            self.remember(npc, Speaker::Npc, text.clone());
            return Ok(Some(text));
        }
        let Some(tie) = self.npcs[i].player else {
            return Ok(None);
        };
        let untold = tie.favour.filter(|f| !f.told);
        if untold.is_none() && !tie.wants_to_talk {
            return Ok(None);
        }
        let known = match untold {
            Some(f) => {
                let told = Favour { told: true, ..f };
                if let Some(t) = &mut self.npcs[i].player {
                    t.favour = Some(told);
                }
                Known::FavourOffer(told)
            }
            None => Known::Nothing,
        };
        if let Some(t) = &mut self.npcs[i].player {
            t.wants_to_talk = false;
        }
        let seed = self.chat_seed(npc, 0xA11);
        let text = {
            let scene = self.chat_scene(i, &known, false);
            lines::opening(&scene, seed)
        };
        self.remember(npc, Speaker::Npc, text.clone());
        Ok(Some(text))
    }

    /// The player says `intent` to NPC `npc` (a suggested reply): the NPC
    /// answers and the effects are applied (see [`crate::chat`]).
    pub fn player_chat(&mut self, npc: NpcId, intent: Intent) -> Result<ChatReply, ChatError> {
        let i = self.chat_index(npc)?;
        Ok(self.exchange(i, Some(intent), None))
    }

    /// The player types `text` to NPC `npc`: `reader` tells the intent (if
    /// it can't, the NPC says it did not understand, and nothing else
    /// happens). The typed line is remembered as written (trimmed, at most
    /// [`MAX_CHAT_INPUT_CHARS`] characters).
    pub fn player_chat_text(
        &mut self,
        npc: NpcId,
        text: &str,
        reader: &dyn IntentReader,
    ) -> Result<ChatReply, ChatError> {
        let i = self.chat_index(npc)?;
        let typed: String = text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(MAX_CHAT_INPUT_CHARS)
            .collect();
        let intent = reader.read(&typed);
        Ok(self.exchange(i, intent, Some(typed)))
    }

    /// The player gives `item` to NPC `npc` from the chat (see
    /// [`World::player_give`]): both lines are remembered.
    pub fn player_chat_gift(&mut self, npc: NpcId, item: ItemKind) -> Result<ChatReply, GiveError> {
        let i = self.chat_index(npc).map_err(|_| GiveError::NoSuchNpc)?;
        let before = self.npcs[i].player_affinity();
        let band = Band::of(self.npcs[i].player.as_ref());
        let stranger = self.npcs[i].player.is_none();
        self.player_give(npc, item)?;
        let known = Known::Gave(item);
        let seed = self.chat_seed(npc, 0x61F7);
        let (said, answer) = {
            let mut scene = self.chat_scene(i, &known, false);
            scene.band = band;
            scene.stranger = stranger;
            (
                lines::gift_line(&scene, seed),
                lines::answer(Intent::Gift, &scene, seed),
            )
        };
        self.remember(npc, Speaker::Player, said.clone());
        self.remember(npc, Speaker::Npc, answer.clone());
        Ok(ChatReply {
            intent: Some(Intent::Gift),
            said,
            answer,
            affinity: self.npcs[i].player_affinity() - before,
            tokens: 0,
            favour: None,
            action: ChatAction::None,
        })
    }

    /// One exchange with NPC `i`: `intent` (None: not understood), `typed`
    /// if the player wrote the line.
    fn exchange(&mut self, i: usize, intent: Option<Intent>, typed: Option<String>) -> ChatReply {
        let id = self.npcs[i].id;
        let tie = self.npcs[i].player;
        let band = Band::of(tie.as_ref());
        let stranger = tie.is_none();
        let seed = self.chat_seed(id, intent.map_or(99, |k| k as u64));
        // "Quanto costa un vestito?": the item named, if any.
        let named = typed
            .as_deref()
            .and_then(|t| mentioned_item(&self.catalog, t));
        let effects = match intent {
            Some(intent) => self.chat_effects(i, intent, band, seed, named),
            None => Effects::of(Known::Nothing),
        };
        // Talking creates the tie and clears the "!".
        self.add_player_affinity(i, effects.affinity);
        let (said, answer) = {
            let mut scene = self.chat_scene(i, &effects.known, effects.repeat);
            scene.band = band;
            scene.stranger = stranger;
            let said = typed.unwrap_or_else(|| match intent {
                Some(intent) => lines::player_line(intent, &scene, seed),
                None => String::new(),
            });
            let answer = match intent {
                Some(intent) => lines::answer(intent, &scene, seed),
                None => lines::not_understood(&scene, seed),
            };
            (said, answer)
        };
        if !said.is_empty() {
            self.remember(id, Speaker::Player, said.clone());
        }
        self.remember(id, Speaker::Npc, answer.clone());
        ChatReply {
            intent,
            said,
            answer,
            affinity: effects.affinity,
            tokens: effects.tokens,
            favour: effects.favour,
            action: effects.action,
        }
    }

    /// Applies what `intent` does (favours, cooldowns) and gathers what the
    /// answer needs. The affinity change is returned, not applied.
    fn chat_effects(
        &mut self,
        i: usize,
        intent: Intent,
        band: Band,
        seed: u64,
        named: Option<ItemKind>,
    ) -> Effects {
        let now = self.clock;
        let tie = self.npcs[i].player.unwrap_or_default();
        match intent {
            Intent::Greet => {
                let mut e = Effects::of(Known::Nothing);
                e.repeat = tie
                    .chat_bonus_at
                    .is_some_and(|t| now.since(t) < CHAT_BONUS_COOLDOWN_MINUTES);
                if !e.repeat {
                    e.affinity = CHAT_GREET_AFFINITY;
                    self.npcs[i]
                        .player
                        .get_or_insert_with(Default::default)
                        .chat_bonus_at = Some(now);
                }
                e
            }
            Intent::AskJob => {
                let n = &self.npcs[i];
                Effects::of(Known::Job {
                    place: n.job.and(n.workplace),
                    shift: n.job.map_or((0, 0), Job::shift),
                })
            }
            Intent::AskPrices => Effects::of(self.known_price(i, seed, named)),
            Intent::AskFavour => self.favour_talk(i, band),
            Intent::Gift => {
                let accepts = !self.chat_gift_options(self.npcs[i].id).is_empty();
                let mut e = Effects::of(Known::Gift { accepts });
                if accepts {
                    e.action = ChatAction::OpenGift;
                }
                e
            }
            Intent::Trade => {
                let n = &self.npcs[i];
                let merchant = n.job == Some(Job::Mercante) && n.workplace.is_some();
                let at_counter = merchant
                    && n.workplace
                        .and_then(|w| self.merchant_on_duty(w))
                        .is_some_and(|m| m.id == n.id);
                let market = if merchant {
                    n.workplace
                } else {
                    self.nearest_market(n.carriage)
                };
                let mut e = Effects::of(Known::Trade {
                    merchant,
                    at_counter,
                    market,
                    opens: Job::Mercante.shift().0,
                });
                if at_counter && let Some(m) = market {
                    e.action = ChatAction::OpenMarket(m);
                }
                e
            }
            Intent::AskNews => Effects::of(self.known_news(i, seed)),
            Intent::Insult => {
                let mut e = Effects::of(Known::Nothing);
                e.repeat = tie
                    .insulted_at
                    .is_some_and(|t| now.since(t) < INSULT_COOLDOWN_MINUTES);
                if !e.repeat {
                    e.affinity = INSULT_AFFINITY;
                    self.npcs[i]
                        .player
                        .get_or_insert_with(Default::default)
                        .insulted_at = Some(now);
                }
                e
            }
            Intent::Farewell => {
                let mut e = Effects::of(Known::Nothing);
                e.action = ChatAction::End;
                e
            }
        }
    }

    // ------------------------------------------------------------------
    // Favours
    // ------------------------------------------------------------------

    /// "Can I do something for you?": the favour asked (told now), done (the
    /// player has what it takes), reminded, or a new one if the NPC needs
    /// something and doesn't distrust the player.
    fn favour_talk(&mut self, i: usize, band: Band) -> Effects {
        let tie = self.npcs[i].player.unwrap_or_default();
        match tie.favour {
            Some(f) if !f.told => {
                let told = Favour { told: true, ..f };
                self.npcs[i]
                    .player
                    .get_or_insert_with(Default::default)
                    .favour = Some(told);
                let mut e = Effects::of(Known::FavourOffer(told));
                e.favour = Some(told);
                e
            }
            Some(f) if self.player.inventory.has(f.item, f.count) => {
                let paid = self.complete_favour(i, f);
                let mut e = Effects::of(Known::FavourDone { favour: f, paid });
                e.affinity = FAVOUR_AFFINITY;
                e.tokens = paid;
                e.favour = Some(f);
                e
            }
            Some(f) => {
                let mut e = Effects::of(Known::FavourReminder(f));
                e.favour = Some(f);
                e
            }
            None => match self.new_favour(i).filter(|_| band != Band::Low) {
                Some(f) => {
                    let told = Favour { told: true, ..f };
                    self.npcs[i]
                        .player
                        .get_or_insert_with(Default::default)
                        .favour = Some(told);
                    let mut e = Effects::of(Known::FavourOffer(told));
                    e.favour = Some(told);
                    e
                }
                None => Effects::of(Known::NoFavour),
            },
        }
    }

    /// A favour NPC `i` could ask now (see the module docs), not stored.
    pub(super) fn new_favour(&self, i: usize) -> Option<Favour> {
        let n = &self.npcs[i];
        if n.age < LifeStage::GIOVANE_FROM {
            return None;
        }
        let owned: Vec<ItemKind> = self.catalog.kinds().filter(|&item| n.wants(item)).collect();
        let food: Vec<ItemKind> = if n.needs.hunger < FAVOUR_HUNGER {
            self.catalog
                .kinds()
                .filter(|&item| item.usage() == ItemUse::Food && n.accepts_gift(item))
                .collect()
        } else {
            Vec::new()
        };
        let mut inputs: Vec<ItemKind> = Vec::new();
        if let (Some(job), Some(place)) = (n.job, n.workplace)
            && let Some(c) = self.carriage(place)
        {
            for &recipe in self.catalog.job(job).work.recipes() {
                for input in self.catalog.recipe(recipe).inputs.iter() {
                    let cap = self.catalog.storage_cap(&self.params, c.kind, input.item);
                    if cap > 0.0
                        && c.stock.get(input.item) < FAVOUR_LOW_STOCK * cap
                        && !inputs.contains(&input.item)
                    {
                        inputs.push(input.item);
                    }
                }
            }
        }
        let pick = (u64::from(n.id.0) + self.clock.day()) as usize;
        let (item, count) = if !owned.is_empty() {
            (owned[pick % owned.len()], 1)
        } else if !food.is_empty() {
            (food[pick % food.len()], 1)
        } else if !inputs.is_empty() {
            let item = inputs[pick % inputs.len()];
            (item, item.stack_size().clamp(1, FAVOUR_MAX_UNITS))
        } else {
            return None;
        };
        let value = self.item_value(item).saturating_mul(count);
        let budget = (n.inventory.tokens as f32 * FAVOUR_MAX_TOKEN_SHARE).floor() as u32;
        let now = self.clock;
        Some(Favour {
            item,
            count,
            reward: value.min(budget),
            since: now,
            until: now + FAVOUR_DAYS * MINUTES_PER_DAY,
            told: true,
        })
    }

    /// What one `item` is worth now: the cheapest Mercato price if it is
    /// sold, else its base value at the pay level.
    pub(super) fn item_value(&self, item: ItemKind) -> u32 {
        self.markets()
            .into_iter()
            .filter_map(|m| self.price(m, item))
            .min()
            .unwrap_or_else(|| {
                (self.catalog.base_value(item) as f32 * self.economy.pay_level)
                    .round()
                    .max(1.0) as u32
            })
    }

    /// The player hands over what favour `f` asked: the NPC uses it (food,
    /// an owned good) or stores it at its workplace (materials), then pays
    /// what it can of the reward from its own tokens. Returns the tokens paid.
    fn complete_favour(&mut self, i: usize, f: Favour) -> u32 {
        let removed = self.player.inventory.remove(f.item, f.count);
        let usage = f.item.usage();
        for _ in 0..removed {
            if usage == ItemUse::Food || usage.is_owned() {
                self.gift_effect(i, f.item);
            } else if let Some(place) = self.npcs[i].workplace
                && let Some(c) = self.carriages.get_mut(place.index())
            {
                let cap = self.catalog.storage_cap(&self.params, c.kind, f.item);
                c.stock.add(f.item, 1.0, cap);
            }
        }
        let npc = &mut self.npcs[i];
        let paid = f.reward.min(npc.inventory.tokens);
        npc.inventory.tokens -= paid;
        self.player.tokens += paid;
        if let Some(t) = &mut npc.player {
            t.favour = None;
        }
        paid
    }

    /// NPC `i`, who just greeted the player, may think of a favour to ask:
    /// it keeps it for when the player talks to it, and shows a "!".
    pub(super) fn maybe_offer_favour(&mut self, i: usize) {
        let now = self.clock;
        let ready = self.npcs[i].player.is_some_and(|t| {
            t.favour.is_none()
                && t.favour_offered_at
                    .is_none_or(|at| now.since(at) >= FAVOUR_OFFER_COOLDOWN_MINUTES)
        });
        if !ready || self.open_favours().len() >= MAX_OPEN_FAVOURS {
            return;
        }
        let Some(f) = self.new_favour(i) else {
            return;
        };
        if let Some(t) = &mut self.npcs[i].player {
            t.favour = Some(Favour { told: false, ..f });
            t.favour_offered_at = Some(now);
            t.wants_to_talk = true;
        }
    }

    /// Midnight: expired favours are dropped; the "!" stays only for a
    /// favour still to tell; chats with the dead are forgotten.
    pub(super) fn chat_midnight(&mut self) {
        let now = self.clock;
        for npc in &mut self.npcs {
            if let Some(t) = &mut npc.player {
                if t.favour.is_some_and(|f| f.until <= now) {
                    t.favour = None;
                }
                t.wants_to_talk = t.favour.is_some_and(|f| !f.told);
            }
        }
        let npcs = &self.npcs;
        self.player
            .chats
            .retain(|log| super::index_of(npcs, log.npc).is_some());
    }

    // ------------------------------------------------------------------
    // What the NPC knows
    // ------------------------------------------------------------------

    /// The price NPC `i` knows for an item sold at the Mercati: `named` if
    /// the player asked about one sold there, else one it wants, else any.
    fn known_price(&self, i: usize, salt: u64, named: Option<ItemKind>) -> Known {
        let n = &self.npcs[i];
        let merchant = n.job == Some(Job::Mercante) && n.workplace.is_some();
        let near = [Some(n.home), n.workplace, Some(n.carriage)];
        let known: Vec<CarriageId> = self
            .markets()
            .into_iter()
            .filter(|&m| {
                merchant
                    || near
                        .iter()
                        .flatten()
                        .any(|c| c.distance(m) <= KNOWN_MARKET_REACH)
            })
            .collect();
        let sold: Vec<ItemKind> = self.catalog.sold_items();
        if known.is_empty() || sold.is_empty() {
            return Known::NoPrices;
        }
        let item = named
            .filter(|item| sold.contains(item))
            .or_else(|| sold.iter().copied().find(|&item| n.wants(item)))
            .unwrap_or(sold[(salt % sold.len() as u64) as usize]);
        if merchant
            && let Some(own) = n.workplace
            && let Some(price) = self.price(own, item)
        {
            return Known::Price {
                item,
                market: own,
                price,
                other: None,
                own: true,
            };
        }
        let prices: Vec<(CarriageId, u32)> = known
            .into_iter()
            .filter_map(|m| Some((m, self.price(m, item)?)))
            .collect();
        let Some(&(market, price)) = prices.iter().min_by_key(|&&(m, p)| (p, m)) else {
            return Known::NoPrices;
        };
        let other = prices
            .iter()
            .copied()
            .filter(|&(m, p)| m != market && p > price)
            .max_by_key(|&(m, p)| (p, std::cmp::Reverse(m)));
        Known::Price {
            item,
            market,
            price,
            other,
            own: false,
        }
    }

    /// A piece of news NPC `i` tells the player, or gossip about the player.
    fn known_news(&mut self, i: usize, salt: u64) -> Known {
        self.collect_news();
        let deed = self.player_deed(i);
        let gossip = self.npcs[i].personality().has(Temper::Pettegolo);
        // Gossips start with the player; the others now and then.
        if let Some(d) = deed.as_ref().filter(|_| gossip || salt.is_multiple_of(3)) {
            return Known::Deed {
                deed: d.0,
                item: d.1,
                place: d.2,
                who: d.3.clone(),
            };
        }
        if let Some((news, subject, place)) = self.chat_news(i, salt) {
            let n = &self.npcs[i];
            let place_name = self.carriages[place.index()].name.as_str();
            let line = text::news_line(
                news,
                subject.as_ref(),
                gossip,
                super::conversation::voice(n),
                self.player.first_name(),
                place_name,
                self.clock.hour(),
                salt,
            );
            if let Some(line) = line {
                return Known::News(line);
            }
        }
        match deed {
            Some((deed, item, place, who)) => Known::Deed {
                deed,
                item,
                place,
                who,
            },
            None => Known::NoNews,
        }
    }

    /// The latest thing the player did that NPC `i` heard of (within
    /// [`crate::SimParams::news_days`]): a theft from a storage where it
    /// lives, works or is, a gift to someone it knows (gossips hear of all).
    fn player_deed(&self, i: usize) -> Option<HeardDeed> {
        let n = &self.npcs[i];
        let now = self.clock;
        let window = self.params.news_days.max(1) * MINUTES_PER_DAY;
        let gossip = n.personality().has(Temper::Pettegolo);
        for e in self.events.iter().rev() {
            if now.since(e.time) > window {
                break;
            }
            match &e.kind {
                EventKind::PlayerTook { item, carriage, .. }
                    if gossip
                        || *carriage == n.home
                        || *carriage == n.carriage
                        || n.workplace == Some(*carriage) =>
                {
                    return Some((Deed::Took, *item, *carriage, None));
                }
                EventKind::PlayerGave {
                    npc: to,
                    name,
                    item,
                } if *to != n.id && (gossip || n.relation(*to).is_some()) => {
                    let first = name.split(' ').next().unwrap_or(name).to_string();
                    let (sex, place) = self
                        .npc(*to)
                        .map_or((Sex::Male, n.carriage), |t| (t.sex, t.carriage));
                    return Some((Deed::Gave, *item, place, Some((first, sex))));
                }
                _ => {}
            }
        }
        None
    }

    // ------------------------------------------------------------------
    // Text and memory
    // ------------------------------------------------------------------

    /// The scene for NPC `i`'s lines (band and stranger as of now).
    fn chat_scene<'a>(&'a self, i: usize, known: &'a Known, repeat: bool) -> ChatScene<'a> {
        let n = &self.npcs[i];
        let name = |c: CarriageId| self.carriages[c.index()].name.as_str();
        let fact = match known {
            Known::Nothing => ChatFact::Nothing,
            Known::Job { place, shift } => ChatFact::Job {
                place: place.map(name),
                shift: *shift,
            },
            Known::Price {
                item,
                market,
                price,
                other,
                own,
            } => ChatFact::Price {
                item: *item,
                market: name(*market),
                price: *price,
                other: other.map(|(m, p)| (name(m), p)),
                own: *own,
            },
            Known::NoPrices => ChatFact::NoPrices,
            Known::FavourOffer(f) => ChatFact::FavourOffer(*f),
            Known::FavourReminder(f) => ChatFact::FavourReminder(*f),
            Known::FavourDone { favour, paid } => ChatFact::FavourDone {
                favour: *favour,
                paid: *paid,
            },
            Known::NoFavour => ChatFact::NoFavour,
            Known::Gift { accepts } => ChatFact::Gift { accepts: *accepts },
            Known::Gave(item) => ChatFact::Gave(*item),
            Known::Trade {
                merchant,
                at_counter,
                market,
                opens,
            } => ChatFact::Trade {
                merchant: *merchant,
                at_counter: *at_counter,
                market: market.map(name),
                opens: *opens,
            },
            Known::News(text) => ChatFact::News(text.clone()),
            Known::Deed {
                deed,
                item,
                place,
                who,
            } => ChatFact::Deed {
                deed: *deed,
                item: *item,
                place: name(*place),
                who: who.as_ref().map(|(w, s)| (w.as_str(), *s)),
            },
            Known::NoNews => ChatFact::NoNews,
        };
        ChatScene {
            npc: ChatNpc {
                first: n.first_name(),
                sex: n.sex,
                age: n.age,
                job: n.job,
                personality: n.personality(),
                hungry: n.needs.hunger < HUNGRY_BELOW,
                tired: n.needs.energy < TIRED_BELOW,
                working: matches!(n.action, Action::Work(_)) && self.works_now(n),
            },
            player: self.player.first_name(),
            band: Band::of(n.player.as_ref()),
            stranger: n.player.is_none(),
            repeat,
            hour: self.clock.hour(),
            fact,
        }
    }

    /// A seed for the wording: the NPC, the minute, how much they talked.
    fn chat_seed(&self, npc: NpcId, salt: u64) -> u64 {
        let said = self
            .player
            .chats
            .iter()
            .find(|l| l.npc == npc)
            .map_or(0, |l| l.said);
        (u64::from(npc.0) << 40)
            ^ self.clock.0.wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ said.wrapping_mul(0x2545_F491_4F6C_DD1D)
            ^ salt
    }

    /// Remembers a line in the chat with `npc` (that log becomes the most
    /// recent; the oldest logs are forgotten beyond [`CHAT_LOGS_KEPT`]).
    pub(super) fn remember(&mut self, npc: NpcId, speaker: Speaker, text: String) {
        let now: GameTime = self.clock;
        let chats = &mut self.player.chats;
        let mut log = match chats.iter().position(|l| l.npc == npc) {
            Some(k) => chats.remove(k),
            None => ChatLog {
                npc,
                lines: Vec::new(),
                said: 0,
            },
        };
        log.push(speaker, now, text);
        chats.push(log);
        if chats.len() > CHAT_LOGS_KEPT {
            let extra = chats.len() - CHAT_LOGS_KEPT;
            chats.drain(..extra);
        }
    }
}

/// The catalog item named in `text` (its name or plural as a word,
/// accents and case aside), if any: "quanto costa un vestito?" → Vestito.
pub(crate) fn mentioned_item(cat: &Catalog, text: &str) -> Option<ItemKind> {
    let words = format!(" {} ", normalize(text));
    cat.kinds().find(|item| {
        [item.name(), item.plural()]
            .iter()
            .any(|w| words.contains(&format!(" {} ", normalize(w))))
    })
}
