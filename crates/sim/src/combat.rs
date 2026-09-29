//! Salute, ferite e combattimento: i tipi.
//!
//! **Health.** NPCs ([`crate::Npc::health`]) and the player
//! ([`crate::PlayerCharacter::health`]) have [`MAX_HEALTH`] hit points.
//! Blows take them away and leave wounds ([`crate::Npc::injury`], the part
//! of the missing health still to heal); starving past
//! [`crate::SimParams::starvation_grace_minutes`] drains them. They come back
//! slowly, faster asleep in a bed, well fed, and with a cup of Tè. By
//! [`Condition`]:
//! - [`Condition::Hurt`] (below [`crate::SimParams::hurt_below`]): walks
//!   slower, works less well, prefers resting;
//! - [`Condition::Bedridden`] (below [`crate::SimParams::bedridden_below`]):
//!   goes home to bed and doesn't work; family and friends bring food;
//! - at 0 an NPC dies ([`crate::DeathCause::Violence`] if a blow killed it,
//!   [`crate::DeathCause::Wounds`] if it bled out, [`crate::DeathCause::Starvation`]);
//!   the player faints (or, with [`crate::SimParams::permadeath`], dies).
//!
//! **Fights** ([`Fight`], see `world/combat.rs`). An attacker
//! ([`crate::Action::Attack`], or the player with
//! [`crate::World::player_attack`]) exchanges one blow a minute with its
//! victim in the same place. At the first blow the victim reacts
//! ([`Reaction`]): fights back, flees (home, or to a crowded carriage) or
//! gives in, by character, health and odds. Everyone there sees it: the
//! victim and its loved ones hold a [`Grudge`], witnesses like the attacker
//! less, and it becomes news. NPCs start fights rarely, scaled by
//! [`crate::SimParams::violence`] (0: never), for a [`Motive`].
//!
//! **Hooks for gangs** (later): grudges with their reason ([`Grudge`]),
//! [`crate::Npc::aggression`], a violence score with its [`Reputation`]
//! ([`crate::Npc::violence`]), the last attacker ([`crate::Npc::last_attacker`])
//! and whom a fight was for ([`Motive::Revenge`]).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::{CarriageId, NpcId};
use crate::player::Place;
use crate::time::GameTime;

/// Full health.
pub const MAX_HEALTH: f32 = 100.0;
/// Most grudges an NPC keeps (the weakest is forgotten for a stronger one).
pub const MAX_GRUDGES: usize = 6;
/// Ended fights stay in [`crate::World::fights`] this many minutes (for the
/// game's shouts and bubbles).
pub const FIGHT_KEPT_MINUTES: u64 = 20;

/// Someone who can fight: an NPC or the player.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Fighter {
    Npc(NpcId),
    Player,
}

impl Fighter {
    pub fn npc(self) -> Option<NpcId> {
        match self {
            Fighter::Npc(id) => Some(id),
            Fighter::Player => None,
        }
    }
}

/// How someone is, from its health.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Condition {
    Healthy,
    /// Below [`crate::SimParams::hurt_below`].
    Hurt,
    /// Below [`crate::SimParams::bedridden_below`].
    Bedridden,
}

impl Condition {
    pub fn of(health: f32, p: &crate::SimParams) -> Condition {
        if health < p.bedridden_below {
            Condition::Bedridden
        } else if health < p.hurt_below {
            Condition::Hurt
        } else {
            Condition::Healthy
        }
    }

    /// "in salute", "ferito", "grave (a letto)".
    pub fn label(self, sex: crate::Sex) -> &'static str {
        match self {
            Condition::Healthy => "in salute",
            Condition::Hurt => sex.pick("ferita", "ferito"),
            Condition::Bedridden => "grave, a letto",
        }
    }
}

/// Why a fight started.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Motive {
    /// A quarrel between two ill-tempered people went too far.
    Quarrel,
    /// An old grudge, met again.
    Grudge,
    /// Revenge for a loved one hurt (or killed) by the victim.
    Revenge,
    /// A Mercante (or a bold witness) caught a thief.
    Thief,
    /// Desperate hunger: food taken by force from someone weaker.
    Robbery,
    /// The player attacked.
    Player,
    /// An NPC defending itself or striking back at the player.
    Defense,
}

impl Motive {
    pub const COUNT: usize = 7;
    pub const ALL: [Motive; Self::COUNT] = [
        Motive::Quarrel,
        Motive::Grudge,
        Motive::Revenge,
        Motive::Thief,
        Motive::Robbery,
        Motive::Player,
        Motive::Defense,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    /// Italian noun: "lite", "rancore", "vendetta"...
    pub fn name(self) -> &'static str {
        match self {
            Motive::Quarrel => "lite",
            Motive::Grudge => "rancore",
            Motive::Revenge => "vendetta",
            Motive::Thief => "ladro sorpreso",
            Motive::Robbery => "rapina per fame",
            Motive::Player => "aggressione del giocatore",
            Motive::Defense => "difesa",
        }
    }
}

/// How the victim of a fight reacted to the first blow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Reaction {
    /// Hits back: both exchange blows until the fight ends.
    FightBack,
    /// Runs away to `to` (home, or a crowded carriage).
    Flee { to: CarriageId },
    /// Gives in (a robber takes what it wanted).
    GiveIn,
}

/// A fight between two people in the same place ([`crate::World::fights`]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fight {
    pub attacker: Fighter,
    pub victim: Fighter,
    pub motive: Motive,
    /// Where it happens.
    pub place: Place,
    pub since: GameTime,
    /// Blows are exchanged until then (unless it ends earlier).
    pub until: GameTime,
    /// The victim's reaction, after the first blow.
    pub reaction: Option<Reaction>,
    /// The attacker does not stop when the victim gives in (revenge for a
    /// killing, desperation): the only fights that often kill.
    pub lethal: bool,
    /// When it ended (kept a little for the game, see [`FIGHT_KEPT_MINUTES`]).
    pub ended: Option<GameTime>,
    /// Damage dealt by the attacker and by the victim so far.
    pub dealt: [f32; 2],
    /// The first blow was struck: the fight is known (witnesses, grudges, news).
    pub opened: bool,
}

impl Fight {
    pub fn is_active(&self) -> bool {
        self.ended.is_none()
    }

    pub fn involves(&self, who: Fighter) -> bool {
        self.attacker == who || self.victim == who
    }

    /// The other side of the fight for `who`.
    pub fn opponent(&self, who: Fighter) -> Option<Fighter> {
        if self.attacker == who {
            Some(self.victim)
        } else if self.victim == who {
            Some(self.attacker)
        } else {
            None
        }
    }
}

/// Why someone holds a grudge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GrudgeReason {
    /// It attacked them.
    Attacked,
    /// It hurt someone they love (`who`).
    HurtLovedOne(NpcId),
    /// It killed someone they love (`who`): the strongest grudge.
    KilledLovedOne(NpcId),
    /// They caught it stealing.
    Theft,
}

/// A grudge against someone ([`crate::Npc::grudges`]): a memory of why,
/// fading slowly ([`crate::SimParams::grudge_decay_per_day`]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Grudge {
    pub against: Fighter,
    pub reason: GrudgeReason,
    /// `0..=1`: how much it weighs (forgotten near 0).
    pub strength: f32,
    pub since: GameTime,
}

impl Grudge {
    /// Whether it is about a loved one (revenge).
    pub fn is_revenge(&self) -> bool {
        matches!(
            self.reason,
            GrudgeReason::HurtLovedOne(_) | GrudgeReason::KilledLovedOne(_)
        )
    }
}

/// What the train thinks of someone's violence, from its score
/// ([`crate::Npc::violence`], [`crate::PlayerCharacter::violence`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Reputation {
    Peaceful,
    /// Has started a fight or two lately.
    Brawler,
    Violent,
    /// Has killed, or fights all the time: people avoid crossing them.
    Feared,
}

impl Reputation {
    pub fn of(score: f32) -> Reputation {
        match score {
            s if s >= 0.7 => Reputation::Feared,
            s if s >= 0.35 => Reputation::Violent,
            s if s >= 0.1 => Reputation::Brawler,
            _ => Reputation::Peaceful,
        }
    }

    /// "pacifico", "attaccabrighe", "violento", "temuto".
    pub fn label(self, sex: crate::Sex) -> &'static str {
        match self {
            Reputation::Peaceful => sex.pick("pacifica", "pacifico"),
            Reputation::Brawler => "attaccabrighe",
            Reputation::Violent => sex.pick("violenta", "violento"),
            Reputation::Feared => sex.pick("temuta", "temuto"),
        }
    }
}

/// Fight counters since the world was generated ([`crate::World::combat`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CombatCounters {
    /// Fights started, per [`Motive::index`].
    pub fights: [u64; Motive::COUNT],
    /// Blows that landed, and their total damage.
    pub hits: u64,
    pub damage: f64,
    /// Victims' reactions: fought back, fled, gave in.
    pub fought_back: u64,
    pub fled: u64,
    pub gave_in: u64,
    /// NPCs killed by a blow, and dead of their wounds.
    pub killed: u64,
    pub died_of_wounds: u64,
    /// Times the player fainted.
    pub player_faints: u64,
    /// Meals brought to bedridden NPCs by family and friends.
    pub meals_brought: u64,
    /// Robbers who got food.
    pub robberies: u64,
}

impl CombatCounters {
    /// Fights started by NPCs (not the player, nor self-defense).
    pub fn npc_fights(&self) -> u64 {
        Motive::ALL
            .iter()
            .filter(|m| !matches!(m, Motive::Player | Motive::Defense))
            .map(|m| self.fights[m.index()])
            .sum()
    }

    pub fn total_fights(&self) -> u64 {
        self.fights.iter().sum()
    }
}

/// Why [`crate::World::player_attack`] failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttackError {
    NoSuchNpc,
    /// Not in the same carriage and floor (or the NPC is walking by).
    NotHere,
    Asleep,
    /// The player is down (fainted or dead).
    Down,
}

impl fmt::Display for AttackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            AttackError::NoSuchNpc => "non c'è più",
            AttackError::NotHere => "è troppo lontano",
            AttackError::Asleep => "stai dormendo",
            AttackError::Down => "non ti reggi in piedi",
        })
    }
}

/// What a blow of the player did ([`crate::World::player_attack`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AttackOutcome {
    /// Damage dealt (0: missed).
    pub damage: f32,
    /// The NPC's health after the blow (0 if it died).
    pub health: f32,
    pub killed: bool,
    /// The NPC's reaction (after the first blow of a fight).
    pub reaction: Option<Reaction>,
}

/// Strength by age, `0..=1`: adults are the strongest.
pub fn strength_at_age(age: u32) -> f32 {
    match age {
        0..=5 => 0.15,
        6..=13 => 0.3 + 0.3 * (age - 6) as f32 / 8.0,
        14..=17 => 0.75,
        18..=45 => 1.0,
        46..=64 => 1.0 - 0.2 * (age - 45) as f32 / 20.0,
        _ => (0.8 - 0.4 * (age.min(90) - 64) as f32 / 26.0).max(0.4),
    }
}

/// Health factor of strength: half at 0 health.
pub fn health_factor(health: f32) -> f32 {
    0.5 + 0.5 * (health / MAX_HEALTH).clamp(0.0, 1.0)
}
