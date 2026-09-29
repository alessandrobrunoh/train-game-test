//! Simulazione del treno: mondo, NPC, economia. Nessuna dipendenza grafica.
//!
//! The sim is location-abstract: an NPC only knows which carriage it is in and
//! which action it performs until when ("Anna è nella carrozza 7, lavora
//! all'aiuola 2 fino alle 14:00"). Decisions happen only when an action ends,
//! so thousands of NPCs are cheap and off-screen carriages never need
//! catching up. The renderer maps `(carriage, action, station)` to sprites.
//!
//! Item flows (see [`item`]):
//! - Food: Contadini grow Verdura into their Serra; Cuochi at a Mensa cook
//!   Verdura taken from the Serre (nearest first) into Razioni stored in that
//!   Mensa; eating takes one Razione. Meals are free for everyone. People
//!   eat in meal shifts ([`Npc::meal_shift`]); in a full Mensa they queue for
//!   a seat ([`Action::Wait`]); see [`World::mensa_occupancy`].
//! - Goods: the train sheds Rottame into the Officine every hour; with food
//!   well stocked, Contadini also grow Cotone and Erbe. Operai cast Rottame
//!   into Metallo and weave Cotone into Tessuto, then make Attrezzi,
//!   Vestiti, Coperte, Lampade and Giocattoli (their carriage's specialties
//!   first, then whatever is scarcest on the train); Mercanti bring Attrezzi
//!   and Vestiti to their Mercato, where NPCs buy them with the tokens earned
//!   as wages. A Mercato price grows with the distance from the nearest
//!   carriage specialized in the item ([`World::market_quotes`]).
//! - Comfort: Cuochi brew Tè from Erbe, drunk with the meals; every
//!   midnight the Officine hand out Coperte, Lampade and Giocattoli to the
//!   Dormitori, where they help sleep and keep residents company.
//! - Recipes (in the world's catalog, [`World::catalog`]) are shared by NPC
//!   workers and the player ([`World::player_craft`]).
//! - Money is a closed loop ([`Economy`]): wages and stipends are paid every
//!   midnight from the administration's treasury; purchases (the player's
//!   too), fines, a tax on large savings and estates without heirs go back
//!   to it. Pay and prices
//!   follow a slow feedback on the treasury.
//! - Owned Attrezzi boost Contadini/Operai output and wear with work; owned
//!   Vestiti slow tiredness and wear daily. Broken ones are bought again.
//!
//! Per-kind properties (items, recipes, carriages, stations, jobs) are data
//! tables in [`defs`]. Items, recipes and jobs grow during a game: every
//! world owns a [`Catalog`] seeded with the builtin rows, and the Custode
//! ([`custode`], [`World::review`], [`World::apply`], [`World::schedule`])
//! adds what the Narratore proposes and makes sense, recording every
//! decision in the world ([`World::decisions`]) so a save replays the game.
//!
//! Every storage is capped per carriage and item; Verdura and Razioni spoil a
//! little every midnight.
//!
//! Life cycle (once per game day, at midnight; a year lasts
//! `SimParams::days_per_year` days): people age ([`LifeStage`]: children and
//! youths don't work, adults do, the elderly retire), make friends by chatting
//! ([`Relation`]), form couples, have children when the train administration
//! allows it (free beds, enough food, below [`World::max_population`]), and die
//! of old age (or hunger). Jobs follow the population: new adults take the job
//! most needed, food first. See [`World::life`] and [`Stats`].
//!
//! Deliberations ([`deliberation`]): rare, meaningful life choices (accepting
//! a couple proposal, having a child, stealing, protesting) are asked as an
//! Italian question with 2–4 options; a brain may answer them, otherwise a
//! built-in rule decides. See [`World::open_deliberations`].
//!
//! The player ([`player`], [`World::player`]) is a character of the train:
//! its place (synced by the game), tokens (part of the money supply),
//! slot inventory, cabin with a private bed and a chest, known recipes;
//! NPCs keep an affinity with it ([`Npc::player`]) that moves prices, gifts
//! and greetings.
//!
//! The player chats with the NPCs ([`chat`], [`World::player_chat`]):
//! suggested replies or free text read by an [`IntentReader`], answers from
//! [`dialogue`] by intent, affinity, personality and state, a memory of the
//! last lines per NPC and small favours paid from the NPC's tokens.
//!
//! Conversations ([`dialogue`]): a chat (`Action::Socialize`) with someone
//! free nearby becomes a two-sided [`Conversation`] with a topic, a tone set
//! by the pair's tie and [`Personality`], and short Italian lines for speech
//! bubbles; at the end it changes their affinity and spreads gossip. See
//! [`World::conversations`].
//!
//! 1 tick = 1 game minute. All randomness comes from seeded ChaCha RNGs, so a
//! world is fully deterministic given its seed and the brain's seed.

pub mod action;
pub mod brain;
pub mod carriage;
pub mod catalog;
pub mod chat;
pub mod custode;
pub mod defs;
pub mod deliberation;
pub mod dialogue;
pub mod event;
pub mod ids;
pub mod item;
pub mod job;
mod names;
pub mod npc;
pub mod params;
pub mod personality;
pub mod player;
pub mod stats;
pub mod time;
pub mod world;

pub use action::{Action, ActionKind, ActionOption, DecisionRequest};
pub use brain::{Brain, RandomBrain, THINK, UtilityBrain, UtilityWeights};
pub use carriage::{Carriage, CarriageKind, Owner, Station, StationKind};
pub use catalog::Catalog;
pub use chat::{
    Band, CHAT_MEMORY_LINES, ChatAction, ChatError, ChatLine, ChatLog, ChatReply, Favour, Intent,
    IntentReader, KeywordReader, Speaker,
};
pub use custode::{Applied, Plan, Rejection};
pub use defs::{
    Amenity, CarriageDef, Consume, ItemCategory, ItemDef, ItemUse, JobDef, Num, RECIPES, RecipeDef,
    RecipeId, Work,
};
pub use deliberation::{
    Choice, Deliberation, DeliberationAnswer, DeliberationCounters, DeliberationId,
    DeliberationKind, DeliberationOption, Gathering, Grievance, ResolvedDeliberation, Resolver,
};
pub use dialogue::{
    Conversation, ConversationCounters, ConversationId, Line, News, Tone, Topic, Valence,
};
pub use event::{BirthDenial, DeathCause, Event, EventKind};
pub use ids::{CarriageId, NpcId, StationId};
pub use item::{ItemInfo, ItemInfoData, ItemKind, Stock};
pub use job::{JobInfo, JobInfoData};
pub use npc::{
    Inventory, Job, LifeStage, MAX_RELATIONS, NPC_ITEM_SLOTS, Needs, Npc, Relation, RelationKind,
    Sex, Traits,
};
pub use params::SimParams;
pub use personality::{Personality, Temper};
pub use player::{
    CHEST_SLOTS, Cabin, ChestError, DEFAULT_PLAYER_NAME, Greeting, INVENTORY_SLOTS, ItemStack,
    MAX_PLAYER_NAME_CHARS, PLAYER_START_TOKENS, Place, PlayerCharacter, PlayerTie, Regard,
    SleepError, SlotInventory, clean_player_name,
};
pub use stats::Stats;
pub use time::{GameTime, MINUTES_PER_DAY, MINUTES_PER_HOUR};
pub use world::{
    BuyError, Comfort, CraftError, Economy, EconomyCounters, GiveError, LifeCounters, PerKind,
    Tally, World,
};
pub use world::{
    Buyer, Listing, ListingId, MarketOffer, Offer, OfferSource, STALL_LOG_KEPT, Seller, StallError,
    StallEvent, StallRecord, StallSale, TradeCounters,
};
pub use world::{
    GIFT_AFFINITY, GREET_AFFINITY, GREET_COOLDOWN_MINUTES, GREET_MINUTES, PRICE_PER_AFFINITY,
    PURCHASE_AFFINITY, THEFT_SEEN_AFFINITY,
};
pub use world::{Market, MarketQuote, PriceSample, SellError, TREND_DAYS, Trend, mercato_price};
pub use world::{MensaOccupancy, MensaRole};
