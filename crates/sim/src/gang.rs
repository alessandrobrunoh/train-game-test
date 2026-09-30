//! Bande: i tipi (il comportamento è in `world/gang.rs`).
//!
//! Gangs are not scripted: they **emerge** from what the train already
//! simulates. Every midnight the sim looks for clusters of people who like
//! each other ([`crate::Relation::affinity`]), live or work close, and share
//! a discontent: poverty (few tokens), grievances ([`crate::Npc::grudges`]),
//! a violent character ([`crate::Npc::aggression`]), youth. A cluster big
//! and sour enough may found a [`Gang`], led by its most aggressive and
//! best connected member. Then the gang lives on its own:
//! - **membership:** friends of members, the poor, the aggrieved and the
//!   young are recruited near its territory; members leave out of fear
//!   (hurt), a strong tie to a rival gang, low loyalty or the death of the
//!   leader (which also opens a succession: a close contender splits away
//!   with its followers). A gang under 2 members disbands; two small
//!   allied gangs may merge;
//! - **territory:** the carriages where its members live and work
//!   ([`Gang::territory`]); a carriage claimed by two gangs is contested and
//!   makes them rivals ([`GangTie::stance`]);
//! - **pizzo:** members collect tokens from who earns in their territory
//!   (the Mercante, stall sellers, workers): paid into the gang's
//!   [`Gang::treasury`] (part of [`crate::World::money_supply`]); who
//!   refuses is beaten ([`crate::Motive::Pizzo`]) and pays anyway if it
//!   loses. The treasury pays the members a share and helps them buy what
//!   they need;
//! - **protection:** a harm done to a member is a grudge of the whole gang
//!   ([`crate::GrudgeReason::GangMate`]); members around join a fight to
//!   defend each other;
//! - **rivalries:** members of rival gangs fight when they meet in contested
//!   territory ([`crate::Motive::Gang`]); the leader may **order a hit**
//!   ([`Hit`]) on a hated enemy: a lethal fight ([`crate::Motive::Hit`]),
//!   the rare killings of the train ([`crate::EventKind::GangHit`]);
//! - **fear:** violence makes a gang feared ([`GangReputation`]); outsiders
//!   avoid its territory at night; it becomes news in the conversations.
//!
//! The player ([`PlayerGang`]): each gang has an attitude towards it
//! ([`crate::World::gang_attitude`]), from its members' affinity and what
//! the player did to them. A member who is a friend may **invite** the
//! player (the "!" of the chat): a member is protected, gets a share of the
//! treasury and small tasks ([`GangTask`]: collect the pizzo from someone).
//! Leaving costs grudges. Gangs ask the pizzo on what the player sells at
//! the stalls in their territory; attacking a member makes the gang hostile
//! ([`crate::World::is_hostile_to_player`]).
//!
//! Everything is scaled by [`crate::SimParams::violence`] ×
//! [`crate::SimParams::gangs`]: at 0 no gang ever forms and the world is the
//! same as without gangs. Randomness comes from a stream of its own
//! (`gang_rng`). Hooks for guards and laws: a worker of a service job
//! ([`crate::Work::Service`], e.g. a guard the Narratore invented) on duty
//! in a carriage deters pizzo and fights there
//! ([`GangParams::guard_deterrence`]); [`crate::World::found_gang`],
//! [`crate::World::add_gang_member`] and [`crate::World::order_hit`] let
//! scripts and the Narratore act on gangs.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::carriage::CarriageKind;
use crate::combat::Fighter;
use crate::ids::{CarriageId, NpcId};
use crate::time::GameTime;

/// Identifier of a [`Gang`], never reused.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct GangId(pub u32);

impl fmt::Display for GangId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "banda {}", self.0)
    }
}

/// Colours of the gangs (the armbands): Italian name and RGB.
pub const GANG_COLOURS: [(&str, [u8; 3]); 8] = [
    ("rosso", [205, 45, 40]),
    ("blu", [55, 95, 215]),
    ("verde", [55, 170, 70]),
    ("giallo", [235, 195, 40]),
    ("viola", [150, 70, 185]),
    ("arancio", [240, 125, 30]),
    ("turchese", [40, 185, 195]),
    ("rosa", [235, 110, 165]),
];

/// Most acts kept in [`Gang::acts`].
pub const GANG_ACTS_KEPT: usize = 16;

/// Tuning of the gangs ([`crate::SimParams::gang`]). Every chance is also
/// scaled by `violence × gangs`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GangParams {
    /// Founding: who has at least this discontent (`0..=1`: aggression,
    /// poverty, grievances, youth) can found or join a gang...
    pub discontent_min: f32,
    /// ...counting as poor below this many tokens...
    pub poor_tokens: u32,
    /// ...tied to the others by at least this affinity, both ways.
    pub bond: f32,
    /// At least this many founders...
    pub found_min: usize,
    /// ...found a gang with this daily chance (times the cluster's discontent).
    pub found_per_day: f32,
    /// At most this many gangs at once...
    pub max_gangs: usize,
    /// ...of at most this many members each...
    pub max_members: usize,
    /// ...and all together at most this share of the people of 16+.
    pub max_share: f32,
    /// Daily chance that a gang recruits its best candidate (times its score).
    pub recruit_per_day: f32,
    /// Daily chance that a member leaves, times its reasons (fear, a rival
    /// tie, low loyalty; 0.2 with none).
    pub leave_per_day: f32,
    /// Hourly chance that a member in its territory asks someone for the
    /// pizzo...
    pub pizzo_per_hour: f32,
    /// ...this share of the victim's tokens (at least 2, at most 30)...
    pub pizzo_share: f32,
    /// ...at most once every this many days per victim.
    pub pizzo_days: u64,
    /// Share of the treasury paid out to the members every midnight.
    pub member_share: f32,
    /// Grudge every member holds against who hurts one of them (below the
    /// grudge that starts a fight: harm done again and again adds up; a
    /// member killed is a much stronger one).
    pub protect_grudge: f32,
    /// Hourly chance that two members of rival gangs who meet in contested
    /// territory fight (times their aggression and rivalry).
    pub rival_fight_per_hour: f32,
    /// Daily chance that a leader orders a hit on a hated enemy...
    pub hit_per_day: f32,
    /// ...held at least this much (the members' grudges against it, summed;
    /// a rival leader counts its gang's rivalry)...
    pub hit_hatred: f32,
    /// ...to be done within this many days.
    pub hit_days: u64,
    /// After a hit done, no other for this many days (a quarter of that
    /// after one that failed).
    pub hit_rest_days: u64,
    /// Pizzo and gang fights are this much less likely for each worker of a
    /// service job (a guard) on duty in the carriage (at most 90% less).
    pub guard_deterrence: f32,
    /// Outsiders avoid at night the territory of gangs this feared.
    pub night_fear: f32,
    /// A friend who is a member invites the player from this attitude of
    /// its gang (see [`crate::World::gang_attitude`])...
    pub invite_attitude: f32,
    /// ...and a gang is hostile to the player below this one.
    pub hostile_attitude: f32,
    /// Share of what the player sells at the stalls of a gang's territory
    /// the gang asks as pizzo.
    pub player_pizzo_share: f32,
    /// Share of the pizzo the player keeps when it collects for its gang.
    pub player_task_share: f32,
    /// Daily chance that two small allied gangs merge.
    pub merge_per_day: f32,
}

impl Default for GangParams {
    fn default() -> Self {
        Self {
            discontent_min: 0.42,
            poor_tokens: 40,
            bond: 0.3,
            found_min: 3,
            found_per_day: 0.08,
            max_gangs: 5,
            max_members: 10,
            max_share: 0.12,
            recruit_per_day: 0.12,
            leave_per_day: 0.04,
            pizzo_per_hour: 0.015,
            pizzo_share: 0.15,
            pizzo_days: 4,
            member_share: 0.25,
            protect_grudge: 0.15,
            rival_fight_per_hour: 0.0005,
            hit_per_day: 0.01,
            hit_hatred: 1.0,
            hit_days: 3,
            hit_rest_days: 120,
            guard_deterrence: 0.5,
            night_fear: 0.3,
            invite_attitude: 0.25,
            hostile_attitude: -0.4,
            player_pizzo_share: 0.1,
            player_task_share: 0.3,
            merge_per_day: 0.02,
        }
    }
}

/// A member of a gang.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Member {
    pub id: NpcId,
    pub since: GameTime,
    /// `0..=1`: how attached it is to the gang (low: it may leave).
    pub loyalty: f32,
}

/// The role of a member.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GangRole {
    /// The leader.
    Capo,
    Membro,
}

impl GangRole {
    /// "capo", "membro".
    pub fn label(self) -> &'static str {
        match self {
            GangRole::Capo => "capo",
            GangRole::Membro => "membro",
        }
    }
}

/// What a gang thinks of another one ([`Gang::ties`]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GangTie {
    pub other: GangId,
    /// `-1..=1`: rivals below [`RIVAL_BELOW`], allies from [`ALLY_FROM`].
    pub stance: f32,
}

/// Gangs with a stance below this are rivals...
pub const RIVAL_BELOW: f32 = -0.3;
/// ...and from this, allies.
pub const ALLY_FROM: f32 = 0.4;

/// A killing ordered by the leader.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    pub target: Fighter,
    /// The member sent to do it.
    pub by: NpcId,
    pub since: GameTime,
    /// Given up after this...
    pub until: GameTime,
    /// ...or after [`HIT_TRIES`] attacks that didn't kill.
    #[serde(default)]
    pub tries: u8,
}

/// Attacks tried for a hit before giving up.
pub const HIT_TRIES: u8 = 3;

/// Something a gang did, for its history ([`Gang::acts`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GangAct {
    pub time: GameTime,
    /// Italian, e.g. "Marco Rossi riscuote 6 gettoni da Anna Neri".
    pub text: String,
}

/// Counters of one gang since it was founded.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GangTally {
    pub pizzo_paid: u64,
    pub pizzo_tokens: u64,
    pub pizzo_refused: u64,
    /// Fights started by members for the gang (pizzo, rivalry, backup, hits).
    pub fights: u64,
    /// People killed by members in the gang's fights.
    pub kills: u64,
    pub hits_ordered: u64,
    pub hits_done: u64,
    /// Members lost to death, and killed.
    pub members_died: u64,
    pub members_killed: u64,
}

/// A gang ([`crate::World::gangs`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gang {
    pub id: GangId,
    /// e.g. "I Topi della Coda" (see [`gang_name`]).
    pub name: String,
    /// Index into [`GANG_COLOURS`].
    pub colour: u8,
    pub leader: NpcId,
    /// Sorted by id; the leader is one of them.
    pub members: Vec<Member>,
    /// Carriages it claims (where its members live and work), most held first.
    pub territory: Vec<CarriageId>,
    /// Tokens: part of [`crate::World::money_supply`].
    pub treasury: u32,
    /// Stances towards the other gangs (rivals, allies).
    pub ties: Vec<GangTie>,
    pub founded: GameTime,
    /// The last [`GANG_ACTS_KEPT`] acts, oldest first.
    pub acts: Vec<GangAct>,
    /// `0..=1`: how much the train fears it (up with violence, fading).
    pub fear: f32,
    /// What the player did to it (attacks, pizzo refused or paid, tasks):
    /// added to the members' affinity in [`crate::World::gang_attitude`].
    pub player_standing: f32,
    /// A killing ordered by the leader, if any.
    pub hit: Option<Hit>,
    /// Last pizzo asked, per victim.
    pub extorted: Vec<(NpcId, GameTime)>,
    /// Pizzo the player owes on its sales at the stalls of the territory,
    /// and when a member asked it.
    pub player_due: u32,
    pub demanded: Option<GameTime>,
    /// The leader died: a successor is chosen at the next hour.
    pub leaderless: bool,
    /// No new hit before this (see [`GangParams::hit_rest_days`]).
    #[serde(default)]
    pub hit_rest_until: Option<GameTime>,
    pub tally: GangTally,
}

impl Gang {
    pub fn is_member(&self, id: NpcId) -> bool {
        self.members.binary_search_by_key(&id, |m| m.id).is_ok()
    }

    pub fn member(&self, id: NpcId) -> Option<&Member> {
        self.members
            .binary_search_by_key(&id, |m| m.id)
            .ok()
            .map(|k| &self.members[k])
    }

    pub(crate) fn member_mut(&mut self, id: NpcId) -> Option<&mut Member> {
        match self.members.binary_search_by_key(&id, |m| m.id) {
            Ok(k) => Some(&mut self.members[k]),
            Err(_) => None,
        }
    }

    /// The role of NPC `id`, if it is a member.
    pub fn role(&self, id: NpcId) -> Option<GangRole> {
        self.is_member(id).then_some(if id == self.leader {
            GangRole::Capo
        } else {
            GangRole::Membro
        })
    }

    /// Members (NPCs; the player is counted apart, see [`PlayerGang`]).
    pub fn size(&self) -> usize {
        self.members.len()
    }

    /// Stance towards `other` (0: never had to do with each other).
    pub fn stance(&self, other: GangId) -> f32 {
        self.ties
            .iter()
            .find(|t| t.other == other)
            .map_or(0.0, |t| t.stance)
    }

    pub fn is_rival(&self, other: GangId) -> bool {
        self.stance(other) < RIVAL_BELOW
    }

    pub fn is_ally(&self, other: GangId) -> bool {
        self.stance(other) >= ALLY_FROM
    }

    pub(crate) fn shift_stance(&mut self, other: GangId, delta: f32) {
        if other == self.id {
            return;
        }
        match self.ties.iter_mut().find(|t| t.other == other) {
            Some(t) => t.stance = (t.stance + delta).clamp(-1.0, 1.0),
            None => self.ties.push(GangTie {
                other,
                stance: delta.clamp(-1.0, 1.0),
            }),
        }
    }

    /// Whether it claims `carriage`.
    pub fn holds(&self, carriage: CarriageId) -> bool {
        self.territory.contains(&carriage)
    }

    pub fn reputation(&self) -> GangReputation {
        GangReputation::of(self.fear)
    }

    /// "dei Topi della Coda", "della Fratellanza del Vapore" (see [`of_form`]).
    pub fn of_name(&self) -> String {
        of_form(&self.name)
    }

    /// The colour's RGB.
    pub fn rgb(&self) -> [u8; 3] {
        GANG_COLOURS[usize::from(self.colour) % GANG_COLOURS.len()].1
    }

    pub(crate) fn act(&mut self, time: GameTime, text: String) {
        self.acts.push(GangAct { time, text });
        if self.acts.len() > GANG_ACTS_KEPT {
            let extra = self.acts.len() - GANG_ACTS_KEPT;
            self.acts.drain(..extra);
        }
    }
}

/// What the train thinks of a gang, from its [`Gang::fear`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GangReputation {
    /// Barely known.
    Unknown,
    Respected,
    Feared,
    /// The terror of the train.
    Terror,
}

impl GangReputation {
    pub fn of(fear: f32) -> GangReputation {
        match fear {
            f if f >= 0.7 => GangReputation::Terror,
            f if f >= 0.35 => GangReputation::Feared,
            f if f >= 0.12 => GangReputation::Respected,
            _ => GangReputation::Unknown,
        }
    }

    /// "poco conosciuta", "rispettata", "temuta", "il terrore del treno".
    pub fn label(self) -> &'static str {
        match self {
            GangReputation::Unknown => "poco conosciuta",
            GangReputation::Respected => "rispettata",
            GangReputation::Feared => "temuta",
            GangReputation::Terror => "il terrore del treno",
        }
    }
}

/// A gang's invitation to the player, told by a member who is a friend.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Invite {
    pub gang: GangId,
    pub by: NpcId,
    pub since: GameTime,
    pub until: GameTime,
    /// The member already asked (false: it shows a "!" and asks when the
    /// player opens the chat).
    pub told: bool,
}

/// A task of the player's gang: collect the pizzo from `victim`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GangTask {
    pub gang: GangId,
    pub victim: NpcId,
    pub tokens: u32,
    pub since: GameTime,
    pub until: GameTime,
}

/// The player and the gangs ([`crate::World::gang_state`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerGang {
    /// The gang the player belongs to.
    pub gang: Option<GangId>,
    pub since: Option<GameTime>,
    pub invite: Option<Invite>,
    pub task: Option<GangTask>,
    /// When the player last refused an invitation or left a gang (no new
    /// invitation for a while).
    pub refused: Option<GameTime>,
    /// Tasks done, and pizzo collected for the gang (tokens).
    pub tasks_done: u32,
    pub collected: u64,
}

/// Why someone left a gang.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LeaveReason {
    /// Hurt, scared.
    Fear,
    /// A strong tie with someone of a rival gang.
    Rival,
    /// Not attached enough.
    Disloyal,
    /// The leader died.
    LeaderDied,
    /// Followed a contender who split away.
    Split,
    /// The player chose to.
    Chose,
}

impl LeaveReason {
    /// "per paura", "per un legame con una banda rivale"...
    pub fn label(self) -> &'static str {
        match self {
            LeaveReason::Fear => "per paura",
            LeaveReason::Rival => "per un legame con una banda rivale",
            LeaveReason::Disloyal => "perché non ci crede più",
            LeaveReason::LeaderDied => "alla morte del capo",
            LeaveReason::Split => "per seguire un altro capo",
            LeaveReason::Chose => "per scelta",
        }
    }
}

/// Why a gang disbanded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DisbandReason {
    /// Fewer than 2 members left.
    TooFew,
    /// Merged into another gang.
    Merged(GangId),
}

/// Gang counters since the world was generated ([`crate::World::gang_state`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GangCounters {
    pub founded: u64,
    pub splits: u64,
    pub merges: u64,
    pub disbanded: u64,
    pub joined: u64,
    pub left: u64,
    pub successions: u64,
    pub pizzo_paid: u64,
    pub pizzo_tokens: u64,
    pub pizzo_refused: u64,
    /// Pizzo taken after a beating.
    pub pizzo_beaten: u64,
    /// Tokens paid out to members (shares and help).
    pub shares: u64,
    pub rival_fights: u64,
    /// Members who joined a fight to defend another.
    pub backups: u64,
    pub hits_ordered: u64,
    /// Hits that killed their target ([`crate::EventKind::GangHit`]).
    pub hits_done: u64,
    /// Everyone killed by members in gang fights (pizzo, rivalry, backup, hits).
    pub kills: u64,
    /// Members killed (by anyone).
    pub members_killed: u64,
}

/// Everything about the gangs in a world ([`crate::World::gang_state`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GangState {
    /// Gangs alive, sorted by id.
    pub list: Vec<Gang>,
    pub next_id: u32,
    pub player: PlayerGang,
    pub counters: GangCounters,
}

/// Why a gang action of the player failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GangError {
    NoSuchGang,
    NoSuchNpc,
    /// No invitation from that member.
    NoInvite,
    /// Already in a gang.
    AlreadyMember,
    /// Not in a gang.
    NotMember,
    /// No such task, or not about that NPC.
    NoTask,
    /// Not where the player is.
    NotHere,
    /// The victim refuses to pay (a lesson may change its mind).
    Refused,
    /// Nothing owed.
    NothingDue,
    /// Not enough tokens.
    TooPoor,
}

impl fmt::Display for GangError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            GangError::NoSuchGang => "la banda non esiste più",
            GangError::NoSuchNpc => "non c'è più",
            GangError::NoInvite => "nessun invito",
            GangError::AlreadyMember => "sei già in una banda",
            GangError::NotMember => "non sei in una banda",
            GangError::NoTask => "nessun incarico",
            GangError::NotHere => "non è qui",
            GangError::Refused => "si rifiuta di pagare",
            GangError::NothingDue => "non devi niente",
            GangError::TooPoor => "non hai abbastanza gettoni",
        })
    }
}

/// What the player collected for its gang ([`crate::World::player_collect_pizzo`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PizzoOutcome {
    /// Tokens the victim paid.
    pub paid: u32,
    /// The player's share of them.
    pub share: u32,
}

// ----------------------------------------------------------------------
// Names
// ----------------------------------------------------------------------

/// Heads of a gang name (with their article).
const HEADS: [&str; 14] = [
    "I Topi",
    "I Lupi",
    "I Corvi",
    "I Figli",
    "Gli Sciacalli",
    "I Ratti",
    "Le Iene",
    "Le Lame",
    "La Fratellanza",
    "La Compagnia",
    "I Fratelli",
    "Le Volpi",
    "I Cani",
    "Gli Spettri",
];

/// Tails from where the founders live on the train (head, middle, tail).
const PLACE_TAILS: [&[&str]; 3] = [
    &["della Testa", "della Locomotiva", "dei Primi Vagoni"],
    &["di Mezzo", "del Corridoio", "dei Passaggi"],
    &["della Coda", "dell'Ultimo Vagone", "del Fondo"],
];

/// Tails from the kind of carriage where the founders work.
fn trade_tails(kind: CarriageKind) -> &'static [&'static str] {
    match kind {
        CarriageKind::Serra => &["dell'Orto", "della Serra", "delle Radici"],
        CarriageKind::Mensa => &["del Mestolo", "della Pentola", "delle Razioni"],
        CarriageKind::Officina => &["del Vapore", "della Chiave", "del Rottame"],
        CarriageKind::Mercato => &["del Bazar", "della Bilancia", "dei Banchi"],
        CarriageKind::Dormitorio => &["delle Cuccette", "delle Brande", "del Buio"],
    }
}

/// Tails of any gang.
const COLD_TAILS: [&str; 6] = [
    "del Gelo",
    "del Ghiaccio",
    "della Brina",
    "del Binario",
    "della Neve",
    "del Carbone",
];

/// A gang name from its founders: where they live (`position`, `0..=1`
/// from the head to the tail) and the kind of carriage most of them work
/// in; `roll` (any number) picks among the forms. Deterministic. E.g. "I
/// Topi della Coda", "La Fratellanza del Vapore", "I Figli del Gelo".
pub fn gang_name(position: f32, work: Option<CarriageKind>, roll: u64) -> String {
    let head = HEADS[(roll % HEADS.len() as u64) as usize];
    let pick = roll / HEADS.len() as u64;
    let place = PLACE_TAILS[((position.clamp(0.0, 1.0) * 3.0) as usize).min(2)];
    let tails: &[&str] = match (pick % 3, work) {
        (0, _) => place,
        (1, Some(kind)) => trade_tails(kind),
        _ => &COLD_TAILS,
    };
    let tail = tails[((pick / 3) % tails.len() as u64) as usize];
    format!("{head} {tail}")
}

/// A gang name after "di": "I Topi della Coda" → "dei Topi della Coda",
/// "La Fratellanza" → "della Fratellanza", "Gli Sciacalli" → "degli
/// Sciacalli". Names without a known article get "di".
pub fn of_form(name: &str) -> String {
    let (article, rest) = name.split_once(' ').unwrap_or(("", name));
    let di = match article {
        "I" | "i" => "dei",
        "Gli" | "gli" => "degli",
        "La" | "la" => "della",
        "Le" | "le" => "delle",
        "Il" | "il" => "del",
        "Lo" | "lo" => "dello",
        _ => return format!("di {name}"),
    };
    format!("{di} {rest}")
}

/// The head of a gang name after "di", for short lines: "I Topi della
/// Coda" → "dei Topi", "La Fratellanza del Vapore" → "della Fratellanza".
pub fn short_of_form(name: &str) -> String {
    let head: Vec<&str> = name.split(' ').take(2).collect();
    of_form(&head.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_have_an_article_and_a_tail() {
        assert_eq!(short_of_form("I Topi della Coda"), "dei Topi");
        assert_eq!(
            short_of_form("La Fratellanza del Vapore"),
            "della Fratellanza"
        );
        let mut seen = std::collections::BTreeSet::new();
        for roll in 0..300 {
            for (pos, work) in [
                (0.0, None),
                (0.5, Some(CarriageKind::Officina)),
                (1.0, Some(CarriageKind::Serra)),
            ] {
                let name = gang_name(pos, work, roll);
                assert!(name.split(' ').count() >= 3, "{name}");
                assert!(!of_form(&name).starts_with("di "), "{name}");
                seen.insert(name);
            }
        }
        assert!(seen.len() > 60, "{}", seen.len());
        assert_eq!(of_form("I Topi della Coda"), "dei Topi della Coda");
        assert_eq!(
            of_form("La Fratellanza del Vapore"),
            "della Fratellanza del Vapore"
        );
        assert_eq!(
            of_form("Gli Sciacalli del Gelo"),
            "degli Sciacalli del Gelo"
        );
        assert_eq!(of_form("Banditi"), "di Banditi");
    }

    #[test]
    fn stances_and_reputation() {
        let mut g = Gang {
            id: GangId(1),
            name: "I Topi della Coda".into(),
            colour: 0,
            leader: NpcId(3),
            members: vec![
                Member {
                    id: NpcId(3),
                    since: GameTime(0),
                    loyalty: 1.0,
                },
                Member {
                    id: NpcId(7),
                    since: GameTime(0),
                    loyalty: 0.5,
                },
            ],
            territory: vec![CarriageId(2)],
            treasury: 0,
            ties: Vec::new(),
            founded: GameTime(0),
            acts: Vec::new(),
            fear: 0.0,
            player_standing: 0.0,
            hit: None,
            extorted: Vec::new(),
            player_due: 0,
            demanded: None,
            leaderless: false,
            hit_rest_until: None,
            tally: GangTally::default(),
        };
        assert_eq!(g.role(NpcId(3)), Some(GangRole::Capo));
        assert_eq!(g.role(NpcId(7)), Some(GangRole::Membro));
        assert_eq!(g.role(NpcId(8)), None);
        g.shift_stance(GangId(2), -0.5);
        assert!(g.is_rival(GangId(2)));
        g.shift_stance(GangId(2), 1.0);
        assert!(g.is_ally(GangId(2)));
        g.shift_stance(GangId(1), -1.0);
        assert!(g.ties.len() == 1);
        assert_eq!(GangReputation::of(0.0), GangReputation::Unknown);
        assert_eq!(GangReputation::of(0.8).label(), "il terrore del treno");
        for _ in 0..40 {
            g.act(GameTime(1), "x".into());
        }
        assert_eq!(g.acts.len(), GANG_ACTS_KEPT);
    }
}
