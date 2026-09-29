//! Combattimento: risse, reazioni, testimoni, rancori, moventi e il giocatore.
//!
//! See [`crate::combat`] for the model. Every random draw here comes from
//! `combat_rng`, a stream of its own: with [`crate::SimParams::violence`] at
//! 0 and no fights started by the player it is never used, so the rest of
//! the world runs exactly as it would without combat.
//!
//! A fight ([`Fight`]) lives in `World::fights`. Every minute
//! ([`World::combat_tick`], after the decisions) the attacker strikes once;
//! at the first blow the victim reacts ([`Reaction`]) and the fight becomes
//! known (witnesses, grudges, news); a victim that fights back strikes too.
//! It ends when its time is up, someone falls, flees or gives in (a lethal
//! fight goes on after the victim gives in), or they are no longer in the
//! same place.

use rand::RngExt;

use super::World;
use crate::action::Action;
use crate::combat::{
    AttackError, AttackOutcome, FIGHT_KEPT_MINUTES, Fight, Fighter, Grudge, GrudgeReason,
    MAX_GRUDGES, MAX_HEALTH, Motive, Reaction, health_factor,
};
use crate::event::{DeathCause, EventKind};
use crate::ids::{CarriageId, NpcId};
use crate::item::ItemKind;
use crate::npc::{LifeStage, MAX_RELATIONS, Relation, RelationKind};
use crate::personality::Temper;
use crate::player::{Place, PlayerTie};
use crate::time::MINUTES_PER_DAY;

/// NPCs start fights from this age...
const ATTACKER_MIN_AGE: u32 = 16;
/// ...only against people of at least this age (children are left alone).
const VICTIM_MIN_AGE: u32 = LifeStage::GIOVANE_FROM;
/// Loved ones of a victim: family, and friends at least this close.
const LOVED_AFFINITY: f32 = 0.6;
/// Grudges weaker than this are forgotten.
const GRUDGE_FORGET: f32 = 0.05;
/// Grudges weaker than this don't lead to a fight.
const GRUDGE_ACT: f32 = 0.2;
/// An NPC with a grudge at least this strong against the player is hostile.
const HOSTILE_GRUDGE: f32 = 0.3;
/// ...and so is one that likes the player less than this.
const HOSTILE_AFFINITY: f32 = -0.5;
/// Loved ones hold a grudge when the victim took at least this damage...
const LOVED_ONES_CARE_FROM: f32 = 10.0;
/// ...full at this much (the victim's own grudge too).
const DAMAGE_FOR_FULL_GRUDGE: f32 = 40.0;
/// Who attacked out of a grudge keeps this share of it afterwards.
const GRUDGE_SETTLED: f32 = 0.5;
/// A grudge fight is lethal for someone this aggressive...
const LETHAL_AGGRESSION: f32 = 0.8;
/// ...holding a grudge at least this strong.
const LETHAL_GRUDGE: f32 = 0.5;
/// A robber is desperate below this hunger.
const DESPERATE_HUNGER: f32 = 0.1;

/// How much the train blames the attacker: not at all for self-defense,
/// half for punishing a thief.
fn blame_of(motive: Motive) -> f32 {
    match motive {
        Motive::Defense => 0.0,
        Motive::Thief => 0.5,
        _ => 1.0,
    }
}

/// Aggression factor of the motives: `(2 × aggression)^power` (1 for the
/// average person, near 0 for the calm, up to 2^power for the most aggressive).
fn aggression_factor(aggression: f32, power: i32) -> f32 {
    (2.0 * aggression).powi(power)
}

impl World {
    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    /// Fights going on, and those that ended in the last
    /// [`FIGHT_KEPT_MINUTES`] (oldest first).
    pub fn fights(&self) -> &[Fight] {
        &self.fights
    }

    /// The fight `who` is in right now, if any.
    pub fn fight_of(&self, who: Fighter) -> Option<&Fight> {
        self.fights
            .iter()
            .find(|f| f.is_active() && f.involves(who))
    }

    /// Name of a fighter ("" for an NPC that is gone).
    pub fn fighter_name(&self, who: Fighter) -> String {
        match who {
            Fighter::Npc(id) => self.npc(id).map(|n| n.name.clone()).unwrap_or_default(),
            Fighter::Player => self.player.name.clone(),
        }
    }

    /// Health of a fighter (None for an NPC that is gone).
    pub fn fighter_health(&self, who: Fighter) -> Option<f32> {
        match who {
            Fighter::Npc(id) => self.npc(id).map(|n| n.health),
            Fighter::Player => Some(self.player.health),
        }
    }

    /// Whether NPC `id` is hostile to the player: it is fighting it, holds a
    /// grudge against it, or can't stand it. Attacking a hostile NPC is
    /// self-defense (no grudges, no loss of reputation).
    pub fn is_hostile_to_player(&self, id: NpcId) -> bool {
        let Some(npc) = self.npc(id) else {
            return false;
        };
        self.fight_of(Fighter::Npc(id))
            .is_some_and(|f| f.involves(Fighter::Player))
            || npc
                .grudge_against(Fighter::Player)
                .is_some_and(|g| g.strength >= HOSTILE_GRUDGE)
            || npc.player_affinity() < HOSTILE_AFFINITY
    }

    /// Where a fighter stands, if it can fight there: an NPC that is not
    /// walking between carriages, the player awake and on its feet.
    fn stand(&self, who: Fighter) -> Option<Place> {
        match who {
            Fighter::Npc(id) => {
                let n = self.npc(id)?;
                (!matches!(n.action, Action::Travel { .. })).then_some(Place {
                    carriage: n.carriage,
                    floor: n.floor,
                })
            }
            Fighter::Player => {
                let p = &self.player;
                (!p.is_asleep() && p.dead.is_none()).then_some(p.place)
            }
        }
    }

    /// Whether `a` and `b` are in the same place and can fight.
    fn together(&self, a: Fighter, b: Fighter) -> bool {
        match (self.stand(a), self.stand(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        }
    }

    fn strength_of(&self, who: Fighter) -> f32 {
        match who {
            Fighter::Npc(id) => self.npc(id).map_or(0.0, |n| n.strength()),
            Fighter::Player => health_factor(self.player.health),
        }
    }

    fn armed(&self, who: Fighter) -> bool {
        match who {
            Fighter::Npc(id) => self.npc(id).is_some_and(|n| n.inventory.tool.is_some()),
            Fighter::Player => self.player.inventory.has(ItemKind::Attrezzo, 1),
        }
    }

    // ------------------------------------------------------------------
    // The player
    // ------------------------------------------------------------------

    /// The player strikes NPC `npc` (same carriage and floor, not walking
    /// by). The first blow opens a fight: the NPC reacts (fights back, flees
    /// or gives in), its loved ones and who saw it think worse of the
    /// player, and it becomes news; later blows keep the fight going. An
    /// NPC at 0 health dies ([`DeathCause::Violence`]). Attacking an NPC
    /// hostile to the player ([`World::is_hostile_to_player`]) is self-defense.
    pub fn player_attack(&mut self, npc: NpcId) -> Result<AttackOutcome, AttackError> {
        if self.player.is_down() || self.player.health <= 0.0 {
            return Err(AttackError::Down);
        }
        if self.player.is_asleep() {
            return Err(AttackError::Asleep);
        }
        let target = Fighter::Npc(npc);
        let j = self.npc_index(npc).ok_or(AttackError::NoSuchNpc)?;
        if !self.together(Fighter::Player, target) {
            return Err(AttackError::NotHere);
        }
        let now = self.clock;
        let minutes = self.params.fight_minutes.max(1);
        let existing = self
            .fights
            .iter()
            .position(|f| f.is_active() && f.involves(Fighter::Player) && f.involves(target));
        let (k, first) = match existing {
            Some(k) => (k, false),
            None => {
                let defense = self.is_hostile_to_player(npc);
                let motive = if defense {
                    Motive::Defense
                } else {
                    Motive::Player
                };
                let place = self.player.place;
                self.fights.push(Fight {
                    attacker: Fighter::Player,
                    victim: target,
                    motive,
                    place,
                    since: now,
                    until: now + minutes,
                    reaction: None,
                    lethal: false,
                    ended: None,
                    dealt: [0.0; 2],
                    opened: true,
                });
                self.combat.fights[motive.index()] += 1;
                if !defense {
                    let p = &self.params;
                    self.player.violence = (self.player.violence + p.violence_per_fight).min(1.0);
                }
                (self.fights.len() - 1, true)
            }
        };
        // Blows keep the fight going (and the NPC fighting back with it).
        let f = &mut self.fights[k];
        f.until = f.until.max(now + minutes);
        let until = f.until;
        let other = &mut self.npcs[j];
        if other.action == Action::Attack(Fighter::Player) {
            other.action_until = other.action_until.max(until);
        }
        let side = usize::from(f.attacker != Fighter::Player);
        let motive = f.motive;
        let damage = self.blow(Fighter::Player, target);
        self.fights[k].dealt[side] += damage;
        self.log_blow(Fighter::Player, target, damage, motive, first);
        if first && motive != Motive::Defense {
            self.fight_known(k);
        }
        let killed = self.hurt(target, damage, Fighter::Player);
        let mut reaction = None;
        if !killed && first && self.fights[k].victim == target {
            let r = self.react(j, Fighter::Player, k);
            reaction = Some(r);
        }
        let health = self.npc(npc).map_or(0.0, |n| n.health);
        Ok(AttackOutcome {
            damage,
            health,
            killed,
            reaction,
        })
    }

    /// The player at 0 health: it faints, loses a share of its tokens and
    /// its most valuable thing (to `by` if an NPC knocked it down, else the
    /// tokens to the treasury) and wakes up in its cabin the next morning
    /// with [`crate::SimParams::faint_health`]. With
    /// [`crate::SimParams::permadeath`] it dies instead (game over).
    fn player_falls(&mut self, by: Fighter) {
        let now = self.clock;
        let place = self.player.place.carriage;
        let by_npc = by.npc().filter(|&id| self.npc(id).is_some());
        let by_name = by_npc.map(|id| self.fighter_name(Fighter::Npc(id)));
        self.end_fights_of(Fighter::Player);
        self.player.greetings.clear();
        if self.params.permadeath {
            self.player.health = 0.0;
            self.player.dead = Some(now);
            let killer_name = self.fighter_name(by);
            self.push_event(EventKind::Killed {
                killer: by,
                killer_name,
                victim: Fighter::Player,
                victim_name: self.player.name.clone(),
                place,
            });
            self.push_event(EventKind::Fainted {
                by: by_npc,
                by_name,
                place,
                tokens: 0,
                item: None,
                dead: true,
            });
            return;
        }
        // Tokens: to who knocked the player down, or back to the treasury.
        let tokens = (self.player.tokens as f32 * self.params.faint_token_share).floor() as u32;
        self.player.tokens -= tokens;
        match by_npc.and_then(|id| self.npc_index(id)) {
            Some(k) => {
                let npc = &mut self.npcs[k].inventory;
                let kept = tokens.min(u32::MAX - npc.tokens);
                npc.tokens += kept;
                self.deposit(tokens - kept);
            }
            None => self.deposit(tokens),
        }
        // The most valuable thing carried, if the attacker has room for it.
        let mut item = None;
        if let Some(k) = by_npc.and_then(|id| self.npc_index(id))
            && let Some((best, _)) = self
                .player
                .inventory
                .items()
                .into_iter()
                .filter(|&(it, _)| self.npcs[k].inventory.items.room_for(it) > 0)
                .max_by_key(|&(it, _)| (self.catalog.base_value(it), std::cmp::Reverse(it)))
            && self.player.inventory.remove(best, 1) == 1
        {
            self.npcs[k].inventory.items.add(best, 1);
            item = Some(best);
        }
        let p = &mut self.player;
        p.health = self.params.faint_health.clamp(1.0, MAX_HEALTH);
        p.injury = MAX_HEALTH - p.health;
        if let Some(home) = p.home {
            p.place = home.place();
        }
        p.asleep_until = Some(now.next_at(self.params.wake_hour, 0));
        p.fainted = Some(now);
        self.combat.player_faints += 1;
        self.push_event(EventKind::Fainted {
            by: by_npc,
            by_name,
            place,
            tokens,
            item,
            dead: false,
        });
    }

    // ------------------------------------------------------------------
    // Every tick
    // ------------------------------------------------------------------

    /// After the decisions: every active fight exchanges its blows; fights
    /// ended long enough ago are dropped.
    pub(super) fn combat_tick(&mut self) {
        let now = self.clock;
        self.fights
            .retain(|f| f.ended.is_none_or(|e| now.since(e) < FIGHT_KEPT_MINUTES));
        let mut k = 0;
        while k < self.fights.len() {
            if self.fights[k].is_active() {
                self.exchange_blows(k);
            }
            k += 1;
        }
    }

    /// One minute of fight `k`.
    fn exchange_blows(&mut self, k: usize) {
        let now = self.clock;
        let f = self.fights[k];
        let attacking = |w: &World, who: Fighter, target: Fighter| match who {
            Fighter::Npc(id) => w
                .npc(id)
                .is_some_and(|n| n.action == Action::Attack(target)),
            // The player's blows come from `player_attack`.
            Fighter::Player => true,
        };
        if now >= f.until
            || !self.together(f.attacker, f.victim)
            || !attacking(self, f.attacker, f.victim)
        {
            self.end_fight(k);
            return;
        }
        let first = !f.opened;
        self.fights[k].opened = true;
        // The attacker strikes (the player strikes with its key).
        if f.attacker != Fighter::Player {
            let damage = self.blow(f.attacker, f.victim);
            self.fights[k].dealt[0] += damage;
            self.log_blow(f.attacker, f.victim, damage, f.motive, first);
            if first {
                self.fight_known(k);
            }
            if self.hurt(f.victim, damage, f.attacker) {
                self.end_fight(k);
                return;
            }
        }
        // The victim reacts to the first blow.
        if self.fights[k].reaction.is_none()
            && let Fighter::Npc(id) = f.victim
            && f.attacker != Fighter::Player
            && let Some(j) = self.npc_index(id)
        {
            let r = self.react(j, f.attacker, k);
            if r != Reaction::FightBack {
                if r == Reaction::GiveIn && f.lethal {
                    return;
                }
                self.end_fight(k);
                return;
            }
        }
        // A victim that fights back strikes too.
        let f = self.fights[k];
        if f.reaction == Some(Reaction::FightBack) && f.is_active() && !first {
            let damage = self.blow(f.victim, f.attacker);
            self.fights[k].dealt[1] += damage;
            if damage > 0.0 {
                self.log_blow(f.victim, f.attacker, damage, f.motive, false);
            }
            if self.hurt(f.attacker, damage, f.victim) {
                self.end_fight(k);
            }
        }
    }

    /// Damage of one blow of `from` against `to` (0: it missed).
    fn blow(&mut self, from: Fighter, to: Fighter) -> f32 {
        let p = &self.params;
        let (mine, theirs) = (self.strength_of(from), self.strength_of(to));
        let chance = (p.hit_chance + 0.2 * (mine - theirs)).clamp(0.3, 0.95);
        let weapon = if self.armed(from) {
            1.0 + p.weapon_bonus
        } else {
            1.0
        };
        let base = p.blow_damage * mine * weapon;
        // Always two draws, so the stream doesn't depend on the outcome.
        let roll: f32 = self.combat_rng.random();
        let spread: f32 = self.combat_rng.random_range(0.75..1.25);
        if roll < chance {
            (base * spread).max(1.0)
        } else {
            0.0
        }
    }

    fn log_blow(&mut self, from: Fighter, to: Fighter, damage: f32, motive: Motive, first: bool) {
        if damage > 0.0 {
            self.combat.hits += 1;
            self.combat.damage += f64::from(damage);
        } else if !first {
            return;
        }
        let place = match self.stand(from).or(self.stand(to)) {
            Some(p) => p.carriage,
            None => self.player.place.carriage,
        };
        let kind = EventKind::Attacked {
            attacker: from,
            attacker_name: self.fighter_name(from),
            victim: to,
            victim_name: self.fighter_name(to),
            damage: damage.round().max(if damage > 0.0 { 1.0 } else { 0.0 }) as u32,
            place,
            motive,
            first,
        };
        self.push_event(kind);
    }

    /// `to` takes `damage` from `by`; true if it fell (an NPC died, the
    /// player fainted or died).
    fn hurt(&mut self, to: Fighter, damage: f32, by: Fighter) -> bool {
        if damage <= 0.0 {
            return false;
        }
        match to {
            Fighter::Npc(id) => {
                let Some(j) = self.npc_index(id) else {
                    return false;
                };
                let npc = &mut self.npcs[j];
                npc.health = (npc.health - damage).max(0.0);
                npc.injury = (npc.injury + damage).min(MAX_HEALTH - npc.health);
                npc.last_attacker = Some(by);
                if npc.health > 0.0 {
                    return false;
                }
                let place = npc.carriage;
                self.combat.killed += 1;
                self.slain(j, by, place, DeathCause::Violence);
                true
            }
            Fighter::Player => {
                let p = &mut self.player;
                p.health = (p.health - damage).max(0.0);
                p.injury = (p.injury + damage).min(MAX_HEALTH - p.health);
                if p.health > 0.0 {
                    return false;
                }
                self.player_falls(by);
                true
            }
        }
    }

    /// NPC `j` dies of `cause` at the hands of `killer`: the killing is
    /// logged, its loved ones will want revenge, the killer is feared.
    pub(super) fn slain(
        &mut self,
        j: usize,
        killer: Fighter,
        place: CarriageId,
        cause: DeathCause,
    ) {
        let victim = self.npcs[j].id;
        let kind = EventKind::Killed {
            killer,
            killer_name: self.fighter_name(killer),
            victim: Fighter::Npc(victim),
            victim_name: self.npcs[j].name.clone(),
            place,
        };
        self.push_event(kind);
        let strength = (self.params.grudge_loved_ones * 2.0).min(1.0);
        for l in self.loved_ones(j) {
            self.add_grudge(l, killer, GrudgeReason::KilledLovedOne(victim), strength);
        }
        let per_kill = self.params.violence_per_kill;
        match killer {
            Fighter::Npc(id) => {
                if let Some(k) = self.npc_index(id) {
                    let v = &mut self.npcs[k].violence;
                    *v = (*v + per_kill).min(1.0);
                }
            }
            Fighter::Player => {
                self.player.violence = (self.player.violence + per_kill).min(1.0);
            }
        }
        self.kill(j, cause);
    }

    /// NPC `j` reacts to the first blow of fight `k` from `attacker`: fights
    /// back, flees or gives in, by character, health and odds.
    fn react(&mut self, j: usize, attacker: Fighter, k: usize) -> Reaction {
        let v = &self.npcs[j];
        let p = &self.params;
        let odds = v.strength() / self.strength_of(attacker).max(0.05);
        let (bold, aggr) = (v.traits.boldness, v.aggression());
        let personality = v.personality();
        let mut fight = (0.3 * bold + 0.4 * aggr + 0.4 * (odds - 1.0)).max(0.0);
        if v.health < p.hurt_below || v.age < LifeStage::ADULTO_FROM {
            fight *= 0.3;
        }
        if v.age < 12 {
            fight = 0.0;
        }
        let flee =
            (0.3 + 0.5 * (1.0 - bold) + 0.3 * (1.0 / odds.max(0.05) - 1.0).max(0.0)).max(0.05);
        let mut give = 0.15;
        if personality.has(Temper::Gentile) || personality.has(Temper::Timido) {
            give += 0.25;
        }
        if v.health < 40.0 {
            give += 0.3;
        }
        let total = fight + flee + give;
        let r = self.combat_rng.random::<f32>() * total;
        let reaction = if r < fight {
            Reaction::FightBack
        } else if r < fight + flee {
            Reaction::Flee { to: self.refuge(j) }
        } else {
            Reaction::GiveIn
        };
        let now = self.clock;
        let until = self.fights[k].until;
        match reaction {
            Reaction::FightBack => {
                self.combat.fought_back += 1;
                self.interrupt(j, Action::Attack(attacker), until.since(now).max(1));
            }
            Reaction::Flee { to } => {
                self.combat.fled += 1;
                let minutes = self.walk_minutes(j, to).max(1);
                self.interrupt(j, Action::Travel { to }, minutes);
            }
            Reaction::GiveIn => {
                self.combat.gave_in += 1;
                self.interrupt(j, Action::Idle, until.since(now).max(1));
            }
        }
        self.fights[k].reaction = Some(reaction);
        reaction
    }

    /// Where NPC `j` flees: home if it is elsewhere, else the most crowded
    /// carriage within three (nearest on ties).
    fn refuge(&self, j: usize) -> CarriageId {
        let npc = &self.npcs[j];
        let here = npc.carriage;
        if npc.home != here {
            return npc.home;
        }
        let mut people = vec![0usize; self.carriages.len()];
        for n in &self.npcs {
            people[n.carriage.index()] += 1;
        }
        let lo = here.index().saturating_sub(3);
        let hi = (here.index() + 3).min(self.carriages.len().saturating_sub(1));
        (lo..=hi)
            .filter(|&c| c != here.index())
            .max_by_key(|&c| (people[c], std::cmp::Reverse(c.abs_diff(here.index()))))
            .map_or(here, |c| CarriageId(c as u16))
    }

    /// Minutes NPC `j` takes to walk to `to` (slower when hurt).
    pub(super) fn walk_minutes(&self, j: usize, to: CarriageId) -> u64 {
        self.trip_minutes(&self.npcs[j], to)
    }

    /// NPC `i` stops what it is doing and starts `action` for `minutes`.
    fn interrupt(&mut self, i: usize, action: Action, minutes: u64) {
        let now = self.clock;
        let (id, here, old) = (self.npcs[i].id, self.npcs[i].carriage, self.npcs[i].action);
        if let Some(station) = old.station() {
            self.release(here, station);
        }
        if self.conversation_of(id).is_some() {
            self.drop_conversation_of(id);
        }
        let npc = &mut self.npcs[i];
        npc.action = action;
        npc.action_since = now;
        npc.action_until = now + minutes.max(1);
    }

    /// Fight `k` ends: who is still fighting stops, a robber takes its loot.
    fn end_fight(&mut self, k: usize) {
        let now = self.clock;
        let f = self.fights[k];
        if !f.is_active() {
            return;
        }
        self.fights[k].ended = Some(now);
        for (who, target) in [(f.attacker, f.victim), (f.victim, f.attacker)] {
            if let Fighter::Npc(id) = who
                && let Some(i) = self.npc_index(id)
                && self.npcs[i].action == Action::Attack(target)
            {
                let npc = &mut self.npcs[i];
                npc.action = Action::Idle;
                npc.action_until = now;
            }
        }
        if f.motive == Motive::Robbery {
            self.loot(&f);
        }
        if f.opened {
            self.settle(&f);
        }
    }

    /// Ends every fight `who` is in (it fell, or it is gone).
    pub(super) fn end_fights_of(&mut self, who: Fighter) {
        for k in 0..self.fights.len() {
            if self.fights[k].is_active() && self.fights[k].involves(who) {
                self.end_fight(k);
            }
        }
    }

    /// A robber that won (the victim gave in or is beaten) eats one of the
    /// victim's food items.
    fn loot(&mut self, f: &Fight) {
        let (Some(a), Some(v)) = (f.attacker.npc(), f.victim.npc()) else {
            return;
        };
        let (Some(i), Some(j)) = (self.npc_index(a), self.npc_index(v)) else {
            return;
        };
        let won =
            f.reaction == Some(Reaction::GiveIn) || self.npcs[j].health < self.params.hurt_below;
        if !won || !self.together(f.attacker, f.victim) {
            return;
        }
        let food = self.npcs[j]
            .inventory
            .items
            .items()
            .into_iter()
            .map(|(item, _)| item)
            .find(|&item| item.usage() == crate::defs::ItemUse::Food);
        let Some(item) = food else {
            return;
        };
        if self.npcs[j].inventory.items.remove(item, 1) == 1 {
            self.gift_effect(i, item);
            self.combat.robberies += 1;
        }
    }

    // ------------------------------------------------------------------
    // Consequences
    // ------------------------------------------------------------------

    /// The first blow of fight `k`: the victim and its loved ones hold a
    /// grudge and like the attacker less, who saw it likes the attacker
    /// less (all this towards the player through [`PlayerTie`]).
    fn fight_known(&mut self, k: usize) {
        let f = self.fights[k];
        let p = &self.params;
        let (hit, witness) = (p.attack_affinity, p.witness_affinity);
        let blame = blame_of(f.motive);
        let place = f.place;
        let witnesses: Vec<usize> = (0..self.npcs.len())
            .filter(|&w| {
                let n = &self.npcs[w];
                let who = Fighter::Npc(n.id);
                !f.involves(who)
                    && n.is_awake()
                    && n.carriage == place.carriage
                    && n.floor == place.floor
                    && !matches!(n.action, Action::Travel { .. })
            })
            .collect();
        for &w in &witnesses {
            self.dislike(w, f.attacker, witness * blame);
        }
        if let Fighter::Npc(v) = f.victim
            && let Some(j) = self.npc_index(v)
        {
            self.dislike(j, f.attacker, hit * blame);
        }
    }

    /// Fight `f` is over: the victim holds a grudge (stronger the more it
    /// was hurt), and so do its loved ones if it was really hurt; who
    /// attacked out of a grudge feels it partly settled.
    fn settle(&mut self, f: &Fight) {
        let p = &self.params;
        let (victim_grudge, loved_grudge, hit) =
            (p.grudge_victim, p.grudge_loved_ones, p.attack_affinity);
        let blame = blame_of(f.motive);
        let damage = f.dealt[0];
        if blame > 0.0
            && let Fighter::Npc(v) = f.victim
            && let Some(j) = self.npc_index(v)
        {
            let strength = victim_grudge * blame * (0.5 + damage / DAMAGE_FOR_FULL_GRUDGE).min(1.0);
            self.add_grudge(j, f.attacker, GrudgeReason::Attacked, strength);
            if damage >= LOVED_ONES_CARE_FROM {
                let strength = loved_grudge * blame * (damage / DAMAGE_FOR_FULL_GRUDGE).min(1.0);
                for l in self.loved_ones(j) {
                    self.add_grudge(l, f.attacker, GrudgeReason::HurtLovedOne(v), strength);
                    self.dislike(l, f.attacker, hit * blame / 2.0);
                }
            }
        }
        if matches!(f.motive, Motive::Grudge | Motive::Revenge | Motive::Quarrel)
            && let Fighter::Npc(a) = f.attacker
            && let Some(i) = self.npc_index(a)
        {
            for g in &mut self.npcs[i].grudges {
                if g.against == f.victim {
                    g.strength *= GRUDGE_SETTLED;
                }
            }
        }
    }

    /// Indices of the loved ones of NPC `j`: family, and friends who like it.
    fn loved_ones(&self, j: usize) -> Vec<usize> {
        let id = self.npcs[j].id;
        self.npcs[j]
            .relations
            .iter()
            .filter(|r| r.kind.is_family() || r.affinity >= LOVED_AFFINITY)
            .filter_map(|r| {
                let k = self.npc_index(r.other)?;
                // Only who loves it back.
                (self.npcs[k].affinity(id) >= 0.0).then_some(k)
            })
            .collect()
    }

    /// NPC `i` holds (or strengthens) a grudge against `against`.
    fn add_grudge(&mut self, i: usize, against: Fighter, reason: GrudgeReason, strength: f32) {
        if strength <= 0.0 || against == Fighter::Npc(self.npcs[i].id) {
            return;
        }
        let now = self.clock;
        let grudges = &mut self.npcs[i].grudges;
        match grudges.iter_mut().find(|g| g.against == against) {
            Some(g) => {
                if strength >= g.strength {
                    g.reason = reason;
                    g.since = now;
                }
                g.strength = (g.strength.max(strength) + 0.1 * strength.min(g.strength)).min(1.0);
            }
            None => {
                grudges.push(Grudge {
                    against,
                    reason,
                    strength: strength.min(1.0),
                    since: now,
                });
                if grudges.len() > MAX_GRUDGES
                    && let Some(weakest) = grudges
                        .iter()
                        .enumerate()
                        .min_by(|a, b| a.1.strength.total_cmp(&b.1.strength))
                        .map(|(k, _)| k)
                {
                    grudges.remove(weakest);
                }
            }
        }
    }

    /// NPC `i` likes `who` less by `amount` (one-sided: a tie is created if
    /// needed, replacing the weakest friend tie when the list is full).
    fn dislike(&mut self, i: usize, who: Fighter, amount: f32) {
        if amount <= 0.0 {
            return;
        }
        let other = match who {
            Fighter::Player => {
                let tie = self.npcs[i].player.get_or_insert_with(PlayerTie::default);
                tie.affinity = (tie.affinity - amount).clamp(-1.0, 1.0);
                tie.wants_to_talk = false;
                return;
            }
            Fighter::Npc(id) if id == self.npcs[i].id => return,
            Fighter::Npc(id) => id,
        };
        let relations = &mut self.npcs[i].relations;
        if let Some(r) = relations.iter_mut().find(|r| r.other == other) {
            r.affinity = (r.affinity - amount).clamp(-1.0, 1.0);
            return;
        }
        let tie = Relation {
            other,
            kind: RelationKind::Friend,
            affinity: (-amount).max(-1.0),
        };
        let friends = relations
            .iter()
            .filter(|r| r.kind == RelationKind::Friend)
            .count();
        if friends < MAX_RELATIONS {
            relations.push(tie);
        } else if let Some(weakest) = relations
            .iter_mut()
            .filter(|r| r.kind == RelationKind::Friend)
            .min_by(|x, y| x.affinity.abs().total_cmp(&y.affinity.abs()))
            && weakest.affinity.abs() < amount
        {
            *weakest = tie;
        }
    }

    // ------------------------------------------------------------------
    // Motives (all scaled by `SimParams::violence`)
    // ------------------------------------------------------------------

    /// Whether NPC `i` can start a fight now.
    fn can_attack(&self, i: usize) -> bool {
        let n = &self.npcs[i];
        n.age >= ATTACKER_MIN_AGE
            && n.is_awake()
            && n.health >= self.params.hurt_below
            && !matches!(n.action, Action::Travel { .. } | Action::Attack(_))
            && self.fight_of(Fighter::Npc(n.id)).is_none()
            && self.protest_of(n.id).is_none()
    }

    /// Whether `who` can be attacked by an NPC now: someone awake of 14+
    /// not already fighting.
    fn can_be_attacked(&self, who: Fighter) -> bool {
        let ok = match who {
            Fighter::Npc(id) => self
                .npc(id)
                .is_some_and(|n| n.age >= VICTIM_MIN_AGE && n.is_awake()),
            Fighter::Player => !self.player.is_down(),
        };
        ok && self.fight_of(who).is_none()
    }

    /// NPC `i` attacks `victim` for `motive`: it drops what it was doing
    /// and the fight starts (the first blow in the next [`World::combat_tick`],
    /// or this tick if it hasn't run yet). False if they can't fight.
    pub(super) fn start_fight(
        &mut self,
        i: usize,
        victim: Fighter,
        motive: Motive,
        lethal: bool,
    ) -> bool {
        let attacker = Fighter::Npc(self.npcs[i].id);
        if attacker == victim
            || !self.can_attack(i)
            || !self.can_be_attacked(victim)
            || !self.together(attacker, victim)
        {
            return false;
        }
        let now = self.clock;
        let minutes = self.params.fight_minutes.max(1) * if lethal { 2 } else { 1 };
        let place = Place {
            carriage: self.npcs[i].carriage,
            floor: self.npcs[i].floor,
        };
        self.interrupt(i, Action::Attack(victim), minutes);
        self.fights.push(Fight {
            attacker,
            victim,
            motive,
            place,
            since: now,
            until: now + minutes,
            reaction: None,
            lethal,
            ended: None,
            dealt: [0.0; 2],
            opened: false,
        });
        self.combat.fights[motive.index()] += 1;
        let gain = match motive {
            Motive::Thief | Motive::Defense => self.params.violence_per_fight / 2.0,
            _ => self.params.violence_per_fight,
        };
        let v = &mut self.npcs[i].violence;
        *v = (*v + gain).min(1.0);
        true
    }

    /// NPC `attacker` attacks `victim` now, for `motive`, whatever
    /// [`crate::SimParams::violence`] says: a hook for scripts, the
    /// Narrator and (later) gangs. False if they can't fight: not in the
    /// same place, asleep, too young, already fighting.
    pub fn npc_attack(&mut self, attacker: NpcId, victim: Fighter, motive: Motive) -> bool {
        match self.npc_index(attacker) {
            Some(i) => self.start_fight(i, victim, motive, false),
            None => false,
        }
    }

    /// End of a tense conversation between NPCs `i` and `j`: rarely, the
    /// more aggressive of the two (16+) throws a punch.
    pub(super) fn quarrel_may_turn_violent(&mut self, i: usize, j: usize) {
        let chance = self.params.violence * self.params.quarrel_fight_chance;
        if chance <= 0.0 {
            return;
        }
        let (a, b) = (&self.npcs[i], &self.npcs[j]);
        if a.age.min(b.age) < VICTIM_MIN_AGE {
            return;
        }
        let (att, vic) = if a.aggression() >= b.aggression() {
            (i, j)
        } else {
            (j, i)
        };
        let factor = aggression_factor(self.npcs[att].aggression(), 3);
        if self.combat_rng.random::<f32>() < chance * factor {
            let victim = Fighter::Npc(self.npcs[vic].id);
            self.start_fight(att, victim, Motive::Quarrel, false);
        }
    }

    /// A caught thief (NPC `i`) at `market`: the Mercante on duty may
    /// punish it with its fists, and holds a grudge anyway.
    pub(super) fn thief_caught(&mut self, i: usize, market: CarriageId) {
        let p = &self.params;
        let chance = p.violence * p.thief_attack_chance;
        if chance <= 0.0 {
            return;
        }
        let Some(m) = self.merchant_on_duty(market).map(|m| m.id) else {
            return;
        };
        let (Some(k), thief) = (self.npc_index(m), Fighter::Npc(self.npcs[i].id)) else {
            return;
        };
        self.add_grudge(k, thief, GrudgeReason::Theft, 0.3);
        let factor = aggression_factor(self.npcs[k].aggression(), 2);
        if self.combat_rng.random::<f32>() < (chance * factor).min(0.9) {
            self.start_fight(k, thief, Motive::Thief, false);
        }
    }

    /// Hourly: who meets the target of a strong grudge may attack it (more
    /// likely for revenge and for the aggressive); starving, dishonest
    /// people may rob food from someone weaker.
    pub(super) fn violence_hour(&mut self) {
        let p = &self.params;
        if p.violence <= 0.0 {
            return;
        }
        let (grudge_rate, robbery_rate) = (
            p.violence * p.grudge_attack_per_hour,
            p.violence * p.robbery_per_hour,
        );
        for i in 0..self.npcs.len() {
            let npc = &self.npcs[i];
            let desperate = npc.needs.hunger < DESPERATE_HUNGER && npc.traits.honesty < 0.5;
            if (npc.grudges.is_empty() && !desperate) || !self.can_attack(i) {
                continue;
            }
            let me = Fighter::Npc(npc.id);
            let target = npc
                .grudges
                .iter()
                .filter(|g| {
                    g.strength >= GRUDGE_ACT
                        && self.can_be_attacked(g.against)
                        && self.together(me, g.against)
                })
                .max_by(|a, b| a.strength.total_cmp(&b.strength))
                .copied();
            if let Some(g) = target {
                // Revenge moves even the calm; a plain grudge needs a temper.
                let aggr = npc.aggression();
                let (factor, motive) = if g.is_revenge() {
                    (2.0 * aggression_factor(aggr.max(0.35), 2), Motive::Revenge)
                } else {
                    (aggression_factor(aggr, 3), Motive::Grudge)
                };
                // Revenge for a killing, or a hot-head with a deep grudge:
                // the fight doesn't stop when the other gives in.
                let lethal = matches!(g.reason, GrudgeReason::KilledLovedOne(_))
                    || (aggr >= LETHAL_AGGRESSION && g.strength >= LETHAL_GRUDGE);
                if self.combat_rng.random::<f32>() < grudge_rate * g.strength * factor {
                    self.start_fight(i, g.against, motive, lethal);
                    continue;
                }
            }
            if desperate {
                self.try_robbery(i, robbery_rate);
            }
        }
    }

    /// Starving NPC `i` without food of its own may rob someone weaker here
    /// who carries some.
    fn try_robbery(&mut self, i: usize, rate: f32) {
        let npc = &self.npcs[i];
        let has_food = |n: &crate::npc::Npc| {
            n.inventory
                .items
                .items()
                .iter()
                .any(|&(item, _)| item.usage() == crate::defs::ItemUse::Food)
        };
        if has_food(npc) {
            return;
        }
        let strength = npc.strength();
        let victim = self
            .npcs
            .iter()
            .filter(|v| {
                v.id != npc.id
                    && self.same_place(npc, v)
                    && v.strength() < strength
                    && has_food(v)
                    && self.can_be_attacked(Fighter::Npc(v.id))
            })
            .min_by(|a, b| a.strength().total_cmp(&b.strength()).then(a.id.cmp(&b.id)))
            .map(|v| v.id);
        let Some(victim) = victim else {
            return;
        };
        let chance = rate * 2.0 * (1.0 - npc.traits.honesty);
        if self.combat_rng.random::<f32>() < chance {
            self.start_fight(i, Fighter::Npc(victim), Motive::Robbery, false);
        }
    }

    /// Midnight: grudges and violence scores fade.
    pub(super) fn combat_midnight(&mut self) {
        let p = &self.params;
        let (grudge, violence) = (p.grudge_decay_per_day, p.violence_decay_per_day);
        for npc in &mut self.npcs {
            if !npc.grudges.is_empty() {
                npc.grudges.retain_mut(|g| {
                    g.strength -= grudge;
                    g.strength >= GRUDGE_FORGET
                });
            }
            if npc.violence > 0.0 {
                npc.violence = (npc.violence - violence).max(0.0);
            }
        }
        let now = self.clock;
        let me = &mut self.player;
        if me.violence > 0.0 {
            me.violence = (me.violence - violence).max(0.0);
        }
        // Old faints are forgotten after a day.
        if me.fainted.is_some_and(|t| now.since(t) >= MINUTES_PER_DAY) && !me.is_asleep() {
            me.fainted = None;
        }
    }

    /// NPC `id` is gone: grudges against it are dropped, its fights end.
    pub(super) fn forget_fighter(&mut self, id: NpcId) {
        let who = Fighter::Npc(id);
        // First the fights (settling them may add grudges), then the grudges.
        if !self.fights.is_empty() {
            self.end_fights_of(who);
        }
        for other in &mut self.npcs {
            if !other.grudges.is_empty() {
                other.grudges.retain(|g| g.against != who);
            }
            if other.last_attacker == Some(who) {
                other.last_attacker = None;
            }
        }
    }
}
