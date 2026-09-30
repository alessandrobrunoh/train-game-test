//! Bande: nascita, reclutamento, territorio, pizzo, protezione, rivalità,
//! regolamenti di conti e il giocatore (vedi [`crate::gang`] per il modello).
//!
//! Every midnight ([`World::gangs_midnight`]): succession of dead leaders,
//! territories, stances between gangs, members who leave, recruitment, a new
//! gang from a sour cluster of friends, merges, gangs too small disband, the
//! treasury pays the members, leaders order hits, the player's invitation,
//! tasks and pizzo expire. Every hour ([`World::gangs_hour`]): who is sent
//! on a hit hunts its target; by day members ask the pizzo in their
//! territory, rivals fight, hostile gangs go after the player, and members
//! ask the player the pizzo on its stall sales. Fights feed back through
//! [`World::gang_fight_opened`] (protection, backup), the end of a pizzo
//! beating and [`World::gang_killing`] (ordered killings).
//!
//! Every random draw comes from `gang_rng`; with `violence × gangs` at 0 no
//! gang forms and nothing here draws or changes anything.

use rand::RngExt;

use super::World;
use crate::action::Action;
use crate::carriage::CarriageKind;
use crate::combat::{Fight, Fighter, GrudgeReason, Motive, Reaction};
use crate::defs::Work;
use crate::event::EventKind;
use crate::gang::{
    DisbandReason, GANG_COLOURS, Gang, GangError, GangId, GangState, GangTally, GangTask,
    HIT_TRIES, Hit, Invite, LeaveReason, Member, PizzoOutcome, gang_name,
};
use crate::ids::{CarriageId, NpcId};
use crate::item::ItemKind;
use crate::npc::Npc;
use crate::player::{Greeting, PlayerTie, Regard};
use crate::time::MINUTES_PER_DAY;

use super::Seller;

/// Gang members are between these ages.
const MIN_AGE: u32 = 16;
const MAX_AGE: u32 = 60;
/// Pizzo is asked from this age, and to who has at least these tokens.
const VICTIM_MIN_AGE: u32 = 18;
const VICTIM_MIN_TOKENS: u32 = 5;
/// Pizzo, rivalries and pursuits of the player happen in these hours.
const ACTIVE_FROM: u32 = 8;
const ACTIVE_TO: u32 = 21;
/// At most this many carriages of territory.
const TERRITORY_MAX: usize = 4;
/// Founders taken from a cluster, at most.
const FOUNDERS_MAX: usize = 6;
/// Pizzo asked, in tokens.
const PIZZO_MIN: u32 = 2;
const PIZZO_MAX: u32 = 30;
/// Grudge of who paid the pizzo against the collector, and affinity lost.
const EXTORTED_GRUDGE: f32 = 0.15;
const EXTORTED_AFFINITY: f32 = 0.1;
/// Grudge of the members against who killed one of them.
const KILLED_MATE_GRUDGE: f32 = 0.6;
/// Fear gained by a gang for a beating, a fight, a killing, an ordered one.
const FEAR_PIZZO: f32 = 0.003;
const FEAR_FIGHT: f32 = 0.02;
const FEAR_KILL: f32 = 0.2;
const FEAR_HIT: f32 = 0.3;
/// Share of its fear a gang keeps every day (it fades by half in about
/// two weeks without violence).
const FEAR_KEPT: f32 = 0.95;
/// Stance change for a fight between members of two gangs, daily for a
/// contested carriage, for neighbouring territories; daily drift to 0.
const STANCE_FIGHT: f32 = -0.15;
const STANCE_CONTESTED: f32 = -0.06;
const STANCE_NEIGHBOURS: f32 = -0.015;
const STANCE_DRIFT: f32 = 0.01;
/// Two gangs rival of the same third one grow closer every day.
const STANCE_COMMON_ENEMY: f32 = 0.03;
/// A rival leader is a hit target from this stance down.
const HIT_RIVAL_STANCE: f32 = -0.5;
/// A contender this close to the best one splits away when a leader dies.
const SUCCESSION_MARGIN: f32 = 0.15;
/// Player: standing changes.
const STANDING_ATTACK: f32 = -0.5;
const STANDING_KILL: f32 = -1.0;
const STANDING_REFUSE_INVITE: f32 = -0.05;
const STANDING_LEAVE: f32 = -0.6;
const STANDING_TASK: f32 = 0.15;
const STANDING_TASK_MISSED: f32 = -0.1;
const STANDING_PIZZO_PAID: f32 = 0.05;
const STANDING_PIZZO_REFUSED: f32 = -0.3;
const STANDING_MEMBER: f32 = 0.3;
/// Grudge of the members against a player who defies the gang.
const DEFIED_GRUDGE: f32 = 0.3;
/// Invitations last this many days; after a refusal none for this many.
const INVITE_DAYS: u64 = 3;
const REINVITE_DAYS: u64 = 10;
/// A task lasts this many days.
const TASK_DAYS: u64 = 2;
/// The player has this long to pay the pizzo once asked.
const DEMAND_MINUTES: u64 = 24 * 60;

/// `(2 × aggression)^power`, like the motives of the fights.
fn aggression_factor(aggression: f32, power: i32) -> f32 {
    (2.0 * aggression).powi(power)
}

/// Pizzo asked of someone with `tokens`.
fn pizzo_of(share: f32, tokens: u32) -> u32 {
    ((tokens as f32 * share).round() as u32)
        .clamp(PIZZO_MIN, PIZZO_MAX)
        .min(tokens)
}

/// How much a grudge weighs in a gang's hatred (for the hits).
fn hatred_weight(reason: GrudgeReason) -> f32 {
    match reason {
        GrudgeReason::KilledLovedOne(_) => 2.0,
        GrudgeReason::GangMate(_) | GrudgeReason::Defied | GrudgeReason::HurtLovedOne(_) => 1.0,
        _ => 0.6,
    }
}

impl World {
    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    /// Gangs alive, by id.
    pub fn gangs(&self) -> &[Gang] {
        &self.gangs.list
    }

    /// Gangs, the player among them, counters.
    pub fn gang_state(&self) -> &GangState {
        &self.gangs
    }

    pub fn gang(&self, id: GangId) -> Option<&Gang> {
        self.gang_index(id).map(|k| &self.gangs.list[k])
    }

    /// The gang NPC `npc` belongs to, if any.
    pub fn gang_of(&self, npc: NpcId) -> Option<&Gang> {
        self.gangs.list.iter().find(|g| g.is_member(npc))
    }

    /// The gang of a fighter (the player's, if it joined one).
    pub fn gang_of_fighter(&self, who: Fighter) -> Option<&Gang> {
        match who {
            Fighter::Npc(id) => self.gang_of(id),
            Fighter::Player => self.gangs.player.gang.and_then(|g| self.gang(g)),
        }
    }

    /// The gang the player belongs to.
    pub fn player_gang(&self) -> Option<&Gang> {
        self.gang_of_fighter(Fighter::Player)
    }

    /// Gangs claiming `carriage` (two or more: it is contested).
    pub fn gangs_holding(&self, carriage: CarriageId) -> Vec<&Gang> {
        self.gangs
            .list
            .iter()
            .filter(|g| g.holds(carriage))
            .collect()
    }

    /// What a gang thinks of the player, `-1..=1`: half the average affinity
    /// of its members (0 for who never met it), half the best one, plus what
    /// the player did to it ([`Gang::player_standing`]); more for a member.
    pub fn gang_attitude(&self, gang: GangId) -> f32 {
        let Some(g) = self.gang(gang) else {
            return 0.0;
        };
        let (mut sum, mut best) = (0.0f32, -1.0f32);
        for m in &g.members {
            let a = self.npc(m.id).map_or(0.0, Npc::player_affinity);
            sum += a;
            best = best.max(a);
        }
        let n = g.members.len().max(1) as f32;
        let member = if self.gangs.player.gang == Some(gang) {
            STANDING_MEMBER
        } else {
            0.0
        };
        (0.5 * sum / n + 0.5 * best.max(0.0) + g.player_standing + member).clamp(-1.0, 1.0)
    }

    /// Whether a gang is hostile to the player: its members attack it on
    /// sight, and hitting them is self-defense.
    pub fn is_gang_hostile(&self, gang: GangId) -> bool {
        self.gangs.player.gang != Some(gang)
            && self.gang_attitude(gang) < self.params.gang.hostile_attitude
    }

    /// The invitation waiting for the player, if any.
    pub fn gang_invite(&self) -> Option<Invite> {
        self.gangs.player.invite
    }

    /// The gang whose member `npc` invites the player (after telling it).
    pub fn gang_invite_from(&self, npc: NpcId) -> Option<&Gang> {
        let invite = self.gangs.player.invite.filter(|i| i.by == npc)?;
        self.gang(invite.gang)
    }

    /// The player's task for its gang, if any.
    pub fn gang_task(&self) -> Option<GangTask> {
        self.gangs.player.task
    }

    /// Pizzo the player owes, per gang: tokens and whether a member asked.
    pub fn gang_pizzo_due(&self) -> Vec<(GangId, u32, bool)> {
        self.gangs
            .list
            .iter()
            .filter(|g| g.player_due > 0)
            .map(|g| (g.id, g.player_due, g.demanded.is_some()))
            .collect()
    }

    /// Tokens in the gangs' treasuries (part of [`World::money_supply`]).
    pub(super) fn gang_treasuries(&self) -> u64 {
        self.gangs.list.iter().map(|g| u64::from(g.treasury)).sum()
    }

    /// "Fa parte della banda «…» (capo). " for [`World::npc_context`].
    pub(super) fn gang_context(&self, npc: &Npc) -> String {
        match self.gang_of(npc.id) {
            Some(g) => format!(
                "Fa parte della banda «{}» ({}). ",
                g.name,
                g.role(npc.id).map_or("membro", |r| r.label())
            ),
            None => String::new(),
        }
    }

    /// Carriages NPC `i` avoids now: at night, the territory of gangs feared
    /// enough that it doesn't belong to (not its home).
    pub(super) fn avoided_at_night(&self, i: usize) -> Vec<CarriageId> {
        if self.gangs.list.is_empty() || !self.params.is_night(self.clock.hour()) {
            return Vec::new();
        }
        let npc = &self.npcs[i];
        let fear = self.params.gang.night_fear;
        self.gangs
            .list
            .iter()
            .filter(|g| g.fear >= fear && !g.is_member(npc.id))
            .flat_map(|g| g.territory.iter().copied())
            .filter(|&c| c != npc.home)
            .collect()
    }

    fn gang_index(&self, id: GangId) -> Option<usize> {
        self.gangs.list.binary_search_by_key(&id, |g| g.id).ok()
    }

    fn gang_index_of(&self, who: Fighter) -> Option<usize> {
        match who {
            Fighter::Npc(id) => self.gangs.list.iter().position(|g| g.is_member(id)),
            Fighter::Player => self.gangs.player.gang.and_then(|g| self.gang_index(g)),
        }
    }

    /// `violence × gangs`: 0 turns the gangs off.
    fn gang_strength(&self) -> f32 {
        (self.params.violence * self.params.gangs).max(0.0)
    }

    fn gang_roll(&mut self) -> f32 {
        self.gang_rng.random()
    }

    /// Gang index per NPC index.
    fn membership(&self) -> Vec<Option<usize>> {
        let mut of = vec![None; self.npcs.len()];
        for (k, g) in self.gangs.list.iter().enumerate() {
            for m in &g.members {
                if let Some(i) = self.npc_index(m.id) {
                    of[i] = Some(k);
                }
            }
        }
        of
    }

    /// How sour NPC `n` is, `0..=1`: aggression, poverty, grievances, youth.
    fn discontent(&self, n: &Npc) -> f32 {
        let gp = &self.params.gang;
        let poor = (1.0 - n.inventory.tokens as f32 / gp.poor_tokens.max(1) as f32).clamp(0.0, 1.0);
        let grievance = n.grudges.iter().map(|g| g.strength).fold(0.0, f32::max);
        let youth = if n.age <= 25 { 0.1 } else { 0.0 };
        (0.5 * n.aggression() + 0.3 * poor + 0.3 * grievance + youth).min(1.0)
    }

    /// Whether NPC `n` could be in a gang (age, health).
    fn gang_age(&self, n: &Npc) -> bool {
        (MIN_AGE..=MAX_AGE).contains(&n.age) && n.health >= self.params.hurt_below
    }

    /// People of 16+ (for the gangs' share of the train).
    fn people_of_gang_age(&self) -> usize {
        self.npcs.iter().filter(|n| n.age >= MIN_AGE).count()
    }

    fn gang_members_total(&self) -> usize {
        self.gangs.list.iter().map(Gang::size).sum()
    }

    /// Pizzo and fights are less likely where a service worker (a guard) is
    /// on duty: the hook for guards and laws.
    fn deterrence(&self, carriage: CarriageId) -> f32 {
        let guards = self
            .npcs_in(carriage)
            .filter(|n| {
                matches!(n.action, Action::Work(_))
                    && n.job
                        .is_some_and(|j| matches!(self.catalog.job(j).work, Work::Service { .. }))
            })
            .count();
        (1.0 - self.params.gang.guard_deterrence * guards as f32).max(0.1)
    }

    /// How fit NPC `i` is to lead: aggression, violence, loyalty, ties.
    fn leader_score(&self, i: usize, loyalty: f32, ties: usize) -> f32 {
        let n = &self.npcs[i];
        n.aggression() + n.violence + 0.5 * loyalty + 0.1 * ties as f32
    }

    // ------------------------------------------------------------------
    // Hooks for scripts and the Narratore
    // ------------------------------------------------------------------

    /// Founds a gang with `members` (at least 2 living NPCs in no gang),
    /// whatever the params say; its leader is the fittest of them. Returns
    /// its id.
    pub fn found_gang(&mut self, members: &[NpcId]) -> Option<GangId> {
        let mut founders: Vec<usize> = members
            .iter()
            .filter_map(|&id| self.npc_index(id))
            .filter(|&i| self.gang_of(self.npcs[i].id).is_none())
            .collect();
        founders.sort_unstable();
        founders.dedup();
        if founders.len() < 2 {
            return None;
        }
        Some(self.create_gang(&founders, None))
    }

    /// Adds NPC `npc` to `gang` (it leaves its gang, if any). False if
    /// either doesn't exist.
    pub fn add_gang_member(&mut self, gang: GangId, npc: NpcId) -> bool {
        let (Some(k), Some(i)) = (self.gang_index(gang), self.npc_index(npc)) else {
            return false;
        };
        if self.gangs.list[k].is_member(npc) {
            return true;
        }
        if let Some(old) = self.gang_index_of(Fighter::Npc(npc)) {
            self.leave_gang(old, npc, LeaveReason::Chose);
        }
        let Some(k) = self.gang_index(gang) else {
            return false;
        };
        self.join_gang(k, i, 0.6);
        true
    }

    /// The leader of `gang` orders a hit on `target` (a member does it
    /// within [`crate::GangParams::hit_days`]). False if the gang has nobody
    /// to send.
    pub fn order_hit(&mut self, gang: GangId, target: Fighter) -> bool {
        match self.gang_index(gang) {
            Some(k) => self.order_hit_at(k, target),
            None => false,
        }
    }

    // ------------------------------------------------------------------
    // Membership
    // ------------------------------------------------------------------

    /// A new gang of NPCs `founders` (indices), led by the fittest; split
    /// from the gang named `split_from` if any. Returns its id.
    fn create_gang(&mut self, founders: &[usize], split_from: Option<String>) -> GangId {
        let now = self.clock;
        let ids: Vec<NpcId> = founders.iter().map(|&i| self.npcs[i].id).collect();
        let ties = |w: &World, i: usize| {
            ids.iter()
                .filter(|&&o| o != w.npcs[i].id && w.npcs[i].affinity(o) >= w.params.gang.bond)
                .count()
        };
        let leader = founders
            .iter()
            .copied()
            .max_by(|&a, &b| {
                let (sa, sb) = (
                    self.leader_score(a, 1.0, ties(self, a)),
                    self.leader_score(b, 1.0, ties(self, b)),
                );
                sa.total_cmp(&sb)
                    .then(self.npcs[b].id.cmp(&self.npcs[a].id))
            })
            .unwrap_or(founders[0]);
        // Name: where they live and what they do.
        let last = self.carriages.len().saturating_sub(1).max(1) as f32;
        let position = founders
            .iter()
            .map(|&i| self.npcs[i].home.index() as f32 / last)
            .sum::<f32>()
            / founders.len() as f32;
        let mut kinds = [0usize; CarriageKind::COUNT];
        for &i in founders {
            if let Some(w) = self.npcs[i].workplace {
                kinds[self.carriages[w.index()].kind.index()] += 1;
            }
        }
        let work = (0..CarriageKind::COUNT)
            .filter(|&k| kinds[k] > 0)
            .max_by_key(|&k| (kinds[k], std::cmp::Reverse(k)))
            .and_then(|k| {
                self.carriages
                    .iter()
                    .map(|c| c.kind)
                    .find(|kind| kind.index() == k)
            });
        // A name no living gang has, nor its head ("I Topi", "Le Lame").
        let head = |n: &str| n.split(' ').take(2).collect::<Vec<_>>().join(" ");
        let mut name = String::new();
        for _ in 0..24 {
            let roll: u64 = self.gang_rng.random();
            name = gang_name(position, work, roll);
            if !self
                .gangs
                .list
                .iter()
                .any(|g| g.name == name || head(&g.name) == head(&name))
            {
                break;
            }
        }
        let id = GangId(self.gangs.next_id);
        self.gangs.next_id += 1;
        let used: Vec<u8> = self.gangs.list.iter().map(|g| g.colour).collect();
        let colour = (0..GANG_COLOURS.len() as u8)
            .find(|c| !used.contains(c))
            .unwrap_or((id.0 % GANG_COLOURS.len() as u32) as u8);
        let mut members: Vec<Member> = founders
            .iter()
            .map(|&i| Member {
                id: self.npcs[i].id,
                since: now,
                loyalty: 0.7,
            })
            .collect();
        members.sort_by_key(|m| m.id);
        let place = self.npcs[leader].home;
        let leader_id = self.npcs[leader].id;
        let mut gang = Gang {
            id,
            name: name.clone(),
            colour,
            leader: leader_id,
            members,
            territory: Vec::new(),
            treasury: 0,
            ties: Vec::new(),
            founded: now,
            acts: Vec::new(),
            fear: 0.05,
            player_standing: 0.0,
            hit: None,
            extorted: Vec::new(),
            player_due: 0,
            demanded: None,
            leaderless: false,
            hit_rest_until: None,
            tally: GangTally::default(),
        };
        let text = match &split_from {
            Some(from) => format!(
                "Fondata da {} staccandosi da «{from}»",
                self.npcs[leader].name
            ),
            None => format!(
                "Fondata da {} con {} compagni",
                self.npcs[leader].name,
                founders.len() - 1
            ),
        };
        gang.act(now, text);
        self.gangs.list.push(gang);
        self.gangs.list.sort_by_key(|g| g.id);
        self.gangs.counters.founded += 1;
        if split_from.is_some() {
            self.gangs.counters.splits += 1;
        }
        if let Some(k) = self.gang_index(id) {
            self.claim_territory(k);
        }
        self.push_event(EventKind::GangFounded {
            gang: id,
            gang_name: name,
            leader: leader_id,
            leader_name: self.npcs[leader].name.clone(),
            members: founders.len() as u32,
            place,
            split_from,
        });
        id
    }

    /// NPC `i` joins gang `k` with `loyalty`.
    fn join_gang(&mut self, k: usize, i: usize, loyalty: f32) {
        let now = self.clock;
        let (id, name) = (self.npcs[i].id, self.npcs[i].name.clone());
        let g = &mut self.gangs.list[k];
        if g.is_member(id) {
            return;
        }
        g.members.push(Member {
            id,
            since: now,
            loyalty: loyalty.clamp(0.0, 1.0),
        });
        g.members.sort_by_key(|m| m.id);
        g.act(now, format!("{name} entra nella banda"));
        let (gang, gang_name) = (g.id, g.name.clone());
        self.gangs.counters.joined += 1;
        self.push_event(EventKind::GangJoined {
            gang,
            gang_name,
            npc: id,
            name,
        });
    }

    /// NPC `id` leaves gang `k` for `reason` (the gang may disband).
    fn leave_gang(&mut self, k: usize, id: NpcId, reason: LeaveReason) {
        let now = self.clock;
        let name = self.npc(id).map(|n| n.name.clone()).unwrap_or_default();
        let g = &mut self.gangs.list[k];
        let Ok(pos) = g.members.binary_search_by_key(&id, |m| m.id) else {
            return;
        };
        g.members.remove(pos);
        if g.leader == id {
            g.leaderless = true;
        }
        if g.hit.is_some_and(|h| h.by == id) {
            g.hit = None;
        }
        g.act(now, format!("{name} se ne va {}", reason.label()));
        let (gang, gang_name) = (g.id, g.name.clone());
        self.gangs.counters.left += 1;
        self.push_event(EventKind::GangLeft {
            gang,
            gang_name,
            npc: Some(id),
            name,
            reason,
        });
        self.disband_if_small(gang);
    }

    /// Gang `gang` disbands if it has fewer than 2 members: its treasury
    /// goes to the one left (or to the administration).
    fn disband_if_small(&mut self, gang: GangId) {
        let Some(k) = self.gang_index(gang) else {
            return;
        };
        if self.gangs.list[k].size() >= 2 {
            return;
        }
        let g = self.gangs.list.remove(k);
        let last = g.members.first().and_then(|m| self.npc_index(m.id));
        match last {
            Some(i) => {
                let tokens = &mut self.npcs[i].inventory.tokens;
                let kept = g.treasury.min(u32::MAX - *tokens);
                *tokens += kept;
                self.deposit(g.treasury - kept);
            }
            None => self.deposit(g.treasury),
        }
        self.gang_gone(g.id);
        self.gangs.counters.disbanded += 1;
        self.push_event(EventKind::GangDisbanded {
            gang: g.id,
            gang_name: g.name,
            reason: DisbandReason::TooFew,
            into_name: None,
        });
    }

    /// Gang `id` is gone: the others forget it, the player too.
    fn gang_gone(&mut self, id: GangId) {
        for g in &mut self.gangs.list {
            g.ties.retain(|t| t.other != id);
        }
        let p = &mut self.gangs.player;
        if p.gang == Some(id) {
            p.gang = None;
            p.since = None;
        }
        if p.invite.is_some_and(|i| i.gang == id) {
            p.invite = None;
        }
        if p.task.is_some_and(|t| t.gang == id) {
            p.task = None;
        }
    }

    /// NPC `id` is dead (already removed): its gang loses it, hits on it
    /// are over.
    pub(super) fn gang_member_gone(&mut self, id: NpcId) {
        if self.gangs.list.is_empty() {
            return;
        }
        let mut emptied = Vec::new();
        for g in &mut self.gangs.list {
            if g.hit
                .is_some_and(|h| h.target == Fighter::Npc(id) || h.by == id)
            {
                g.hit = None;
            }
            g.extorted.retain(|&(v, _)| v != id);
            if let Ok(pos) = g.members.binary_search_by_key(&id, |m| m.id) {
                g.members.remove(pos);
                g.tally.members_died += 1;
                if g.leader == id {
                    g.leaderless = true;
                }
                emptied.push(g.id);
            }
        }
        let p = &mut self.gangs.player;
        if p.invite.is_some_and(|i| i.by == id) {
            p.invite = None;
        }
        if p.task.is_some_and(|t| t.victim == id) {
            p.task = None;
        }
        for gang in emptied {
            self.disband_if_small(gang);
        }
    }

    /// The leaderless gang `k` gets a new leader: the fittest member. A
    /// contender almost as fit splits away with who likes it more.
    fn succession(&mut self, k: usize) {
        let g = &self.gangs.list[k];
        let mut ranked: Vec<(f32, usize)> = g
            .members
            .iter()
            .filter_map(|m| {
                let i = self.npc_index(m.id)?;
                Some((self.leader_score(i, m.loyalty, 0), i))
            })
            .collect();
        ranked.sort_by(|a, b| {
            b.0.total_cmp(&a.0)
                .then(self.npcs[a.1].id.cmp(&self.npcs[b.1].id))
        });
        let Some(&(best_score, best)) = ranked.first() else {
            return;
        };
        let (gang, old_name) = (g.id, g.name.clone());
        let best_id = self.npcs[best].id;
        let mut contested = false;
        if let Some(&(second_score, second)) = ranked.get(1)
            && ranked.len() >= 4
            && second_score >= best_score - SUCCESSION_MARGIN
            && self.gangs.list.len() < self.params.gang.max_gangs
        {
            // The contender leaves with who prefers it.
            let second_id = self.npcs[second].id;
            let followers: Vec<usize> = ranked
                .iter()
                .skip(2)
                .map(|&(_, i)| i)
                .filter(|&i| self.npcs[i].affinity(second_id) > self.npcs[i].affinity(best_id))
                .collect();
            if !followers.is_empty() {
                contested = true;
                let mut split = vec![second];
                split.extend(followers);
                for &i in &split {
                    let id = self.npcs[i].id;
                    if let Some(k) = self.gang_index(gang) {
                        let g = &mut self.gangs.list[k];
                        if let Ok(pos) = g.members.binary_search_by_key(&id, |m| m.id) {
                            g.members.remove(pos);
                        }
                    }
                }
                let new = self.create_gang(&split, Some(old_name.clone()));
                if let (Some(a), Some(b)) = (self.gang_index(gang), self.gang_index(new)) {
                    self.gangs.list[a].shift_stance(new, -0.6);
                    self.gangs.list[b].shift_stance(gang, -0.6);
                }
            }
        }
        let now = self.clock;
        let Some(k) = self.gang_index(gang) else {
            return;
        };
        let leader_name = self.npcs[best].name.clone();
        let g = &mut self.gangs.list[k];
        g.leader = best_id;
        g.leaderless = false;
        if let Some(m) = g.member_mut(best_id) {
            m.loyalty = 1.0;
        }
        g.act(now, format!("{leader_name} prende il comando"));
        let gang_name = g.name.clone();
        self.gangs.counters.successions += 1;
        self.push_event(EventKind::GangLeader {
            gang,
            gang_name,
            leader: best_id,
            leader_name,
            contested,
        });
        // Who was not attached to the gang goes away.
        let doubtful: Vec<NpcId> = self.gangs.list[k]
            .members
            .iter()
            .filter(|m| m.id != best_id && m.loyalty < 0.35)
            .map(|m| m.id)
            .collect();
        for id in doubtful {
            if self.gang_roll() < 0.5
                && let Some(k) = self.gang_index(gang)
            {
                self.leave_gang(k, id, LeaveReason::LeaderDied);
            }
        }
        self.disband_if_small(gang);
    }

    // ------------------------------------------------------------------
    // Midnight
    // ------------------------------------------------------------------

    /// Midnight: see the module docs.
    pub(super) fn gangs_midnight(&mut self) {
        let s = self.gang_strength();
        if self.gangs.list.is_empty() && s <= 0.0 {
            return;
        }
        self.player_gang_midnight();
        for k in (0..self.gangs.list.len()).rev() {
            if self.gangs.list.get(k).is_some_and(|g| g.leaderless) {
                self.succession(k);
            }
        }
        for k in 0..self.gangs.list.len() {
            self.claim_territory(k);
        }
        self.update_stances();
        self.members_leave();
        if s > 0.0 {
            self.recruit(s);
            self.found_gangs(s);
            self.merge_gangs(s);
        }
        self.pay_members();
        for k in 0..self.gangs.list.len() {
            let g = &mut self.gangs.list[k];
            g.fear *= FEAR_KEPT;
            let now = self.clock;
            let days = self.params.gang.pizzo_days * MINUTES_PER_DAY;
            g.extorted.retain(|&(_, t)| now.since(t) < days);
            if g.hit.is_some_and(|h| h.until <= now) {
                g.hit = None;
                g.hit_rest_until = Some(now + self.params.gang.hit_rest_days * MINUTES_PER_DAY / 4);
                g.act(now, "Il colpo ordinato dal capo va a vuoto".to_string());
            }
        }
        if s > 0.0 {
            for k in 0..self.gangs.list.len() {
                self.maybe_order_hit(k, s);
            }
        }
    }

    /// The carriages gang `k` claims: where at least two of its members
    /// live or work (else the one most of them use), most held first.
    fn claim_territory(&mut self, k: usize) {
        let mut count = vec![0usize; self.carriages.len()];
        for m in &self.gangs.list[k].members {
            if let Some(n) = self.npc(m.id) {
                count[n.home.index()] += 1;
                if let Some(w) = n.workplace {
                    count[w.index()] += 1;
                }
            }
        }
        let mut held: Vec<usize> = (0..count.len()).filter(|&c| count[c] >= 2).collect();
        held.sort_by_key(|&c| (std::cmp::Reverse(count[c]), c));
        held.truncate(TERRITORY_MAX);
        if held.is_empty()
            && let Some(c) = (0..count.len())
                .filter(|&c| count[c] > 0)
                .max_by_key(|&c| (count[c], std::cmp::Reverse(c)))
        {
            held.push(c);
        }
        self.gangs.list[k].territory = held.into_iter().map(|c| CarriageId(c as u16)).collect();
    }

    /// Contested carriages make rivals, neighbours grow wary, common
    /// enemies make allies; everything else drifts back to indifference.
    fn update_stances(&mut self) {
        let n = self.gangs.list.len();
        for a in 0..n {
            for b in (a + 1)..n {
                let (ga, gb) = (&self.gangs.list[a], &self.gangs.list[b]);
                let contested = ga.territory.iter().any(|c| gb.holds(*c));
                let neighbours = ga
                    .territory
                    .iter()
                    .any(|c| gb.territory.iter().any(|d| c.distance(*d) == 1));
                let common = self.gangs.list.iter().any(|gc| {
                    gc.id != ga.id && gc.id != gb.id && ga.is_rival(gc.id) && gb.is_rival(gc.id)
                });
                let now = ga.stance(gb.id);
                let delta = if contested {
                    STANCE_CONTESTED
                } else if neighbours {
                    STANCE_NEIGHBOURS
                } else if common {
                    STANCE_COMMON_ENEMY
                } else if now > 0.0 {
                    -STANCE_DRIFT.min(now)
                } else {
                    STANCE_DRIFT.min(-now)
                };
                if delta != 0.0 {
                    let (ida, idb) = (ga.id, gb.id);
                    self.gangs.list[a].shift_stance(idb, delta);
                    self.gangs.list[b].shift_stance(ida, delta);
                }
            }
        }
    }

    /// Loyalty moves with the tie to the leader; members leave out of fear,
    /// a tie with a rival gang or low loyalty.
    fn members_leave(&mut self) {
        let rate = self.params.gang.leave_per_day;
        for k in (0..self.gangs.list.len()).rev() {
            let Some(g) = self.gangs.list.get(k) else {
                continue;
            };
            let (gang, leader) = (g.id, g.leader);
            let rivals: Vec<GangId> = g
                .ties
                .iter()
                .filter(|t| t.stance < crate::gang::RIVAL_BELOW)
                .map(|t| t.other)
                .collect();
            let ids: Vec<NpcId> = g.members.iter().map(|m| m.id).collect();
            for id in ids {
                if id == leader {
                    continue;
                }
                let (Some(i), Some(k)) = (self.npc_index(id), self.gang_index(gang)) else {
                    continue;
                };
                let to_leader = self.npcs[i].affinity(leader);
                let target = (0.35 + 0.5 * to_leader).clamp(0.0, 1.0);
                let loyalty = {
                    let m = self.gangs.list[k].member_mut(id).expect("a member");
                    m.loyalty += 0.1 * (target - m.loyalty);
                    m.loyalty
                };
                let npc = &self.npcs[i];
                let fear = if npc.health < self.params.hurt_below {
                    2.0
                } else {
                    0.0
                };
                let rival_tie = npc
                    .relations
                    .iter()
                    .filter(|r| r.affinity >= 0.6 || r.kind == crate::npc::RelationKind::Partner)
                    .any(|r| {
                        self.gangs
                            .list
                            .iter()
                            .any(|o| rivals.contains(&o.id) && o.is_member(r.other))
                    });
                let rival = if rival_tie { 3.0 } else { 0.0 };
                let disloyal = if loyalty < 0.25 { 3.0 } else { 0.0 };
                let factor = 0.2 + fear + rival + disloyal;
                if self.gang_roll() < rate * factor {
                    let reason = if rival >= fear.max(disloyal) && rival > 0.0 {
                        LeaveReason::Rival
                    } else if fear >= disloyal && fear > 0.0 {
                        LeaveReason::Fear
                    } else {
                        LeaveReason::Disloyal
                    };
                    self.leave_gang(k, id, reason);
                    if self.gang_index(gang).is_none() {
                        break;
                    }
                }
            }
        }
    }

    /// Every gang may take its best candidate: someone near its territory
    /// or friend of a member, sour enough, never a peaceful soul.
    fn recruit(&mut self, s: f32) {
        let gp = self.params.gang.clone();
        let cap = (self.people_of_gang_age() as f32 * gp.max_share).floor() as usize;
        for k in 0..self.gangs.list.len() {
            if self.gang_members_total() >= cap || self.gangs.list[k].size() >= gp.max_members {
                continue;
            }
            let of = self.membership();
            let g = &self.gangs.list[k];
            let mut best: Option<(f32, f32, usize)> = None;
            for (i, n) in self.npcs.iter().enumerate() {
                if of[i].is_some() || !self.gang_age(n) || n.aggression() < 0.3 {
                    continue;
                }
                let link = g
                    .members
                    .iter()
                    .map(|m| n.affinity(m.id))
                    .fold(-1.0f32, f32::max)
                    .max(0.0);
                let local = g.holds(n.home) || n.workplace.is_some_and(|w| g.holds(w));
                if link < 0.2 && !local {
                    continue;
                }
                let sour = self.discontent(n);
                if sour < 0.8 * gp.discontent_min && link < 0.5 {
                    continue;
                }
                let score = 0.4 * link + 0.6 * sour + if local { 0.1 } else { 0.0 };
                if best.is_none_or(|(b, ..)| score > b) {
                    best = Some((score, link, i));
                }
            }
            let Some((score, link, i)) = best else {
                continue;
            };
            if self.gang_roll() < gp.recruit_per_day * s * score {
                self.join_gang(k, i, 0.4 + 0.5 * link);
            }
        }
    }

    /// A cluster of sour friends living or working close may found a gang
    /// (at most one a day).
    fn found_gangs(&mut self, s: f32) {
        let gp = self.params.gang.clone();
        if self.gangs.list.len() >= gp.max_gangs {
            return;
        }
        let cap = (self.people_of_gang_age() as f32 * gp.max_share).floor() as usize;
        let room = cap.saturating_sub(self.gang_members_total());
        if room < gp.found_min {
            return;
        }
        let of = self.membership();
        let sour: Vec<f32> = self
            .npcs
            .iter()
            .enumerate()
            .map(|(i, n)| {
                if of[i].is_none() && self.gang_age(n) {
                    self.discontent(n)
                } else {
                    0.0
                }
            })
            .collect();
        let candidate = |i: usize| sour[i] >= gp.discontent_min;
        let near = |a: &Npc, b: &Npc| {
            a.home.distance(b.home) <= 1 || (a.workplace.is_some() && a.workplace == b.workplace)
        };
        let mut seen = vec![false; self.npcs.len()];
        let mut best: Option<(f32, Vec<usize>)> = None;
        for start in 0..self.npcs.len() {
            if seen[start] || !candidate(start) {
                continue;
            }
            // Friends of friends, sour and close, both ways.
            let mut cluster = vec![start];
            seen[start] = true;
            let mut next = 0;
            while next < cluster.len() {
                let i = cluster[next];
                next += 1;
                let a = &self.npcs[i];
                for r in &a.relations {
                    if r.affinity < gp.bond {
                        continue;
                    }
                    let Some(j) = self.npc_index(r.other) else {
                        continue;
                    };
                    let b = &self.npcs[j];
                    if !seen[j] && candidate(j) && b.affinity(a.id) >= gp.bond && near(a, b) {
                        seen[j] = true;
                        cluster.push(j);
                    }
                }
            }
            if cluster.len() < gp.found_min {
                continue;
            }
            cluster.sort_by(|&x, &y| sour[y].total_cmp(&sour[x]).then(x.cmp(&y)));
            cluster.truncate(FOUNDERS_MAX.min(room));
            let mut score = cluster.iter().map(|&i| sour[i]).sum::<f32>() / cluster.len() as f32;
            // A shared enemy binds them.
            let shared = cluster.iter().any(|&i| {
                self.npcs[i].grudges.iter().any(|g| {
                    cluster.iter().any(|&j| {
                        j != i && self.npcs[j].grudges.iter().any(|h| h.against == g.against)
                    })
                })
            });
            if shared {
                score += 0.15;
            }
            if best.as_ref().is_none_or(|(b, _)| score > *b) {
                best = Some((score, cluster));
            }
        }
        let Some((score, mut founders)) = best else {
            return;
        };
        let chance = (gp.found_per_day * s * (score / 0.5).powi(2)).min(0.5);
        if self.gang_roll() < chance {
            founders.sort_unstable();
            self.create_gang(&founders, None);
        }
    }

    /// Two small allied gangs may merge (the smaller into the bigger).
    fn merge_gangs(&mut self, s: f32) {
        let gp = self.params.gang.clone();
        let n = self.gangs.list.len();
        let mut pair = None;
        'outer: for a in 0..n {
            for b in (a + 1)..n {
                let (ga, gb) = (&self.gangs.list[a], &self.gangs.list[b]);
                if ga.is_ally(gb.id)
                    && gb.is_ally(ga.id)
                    && ga.size() <= 4
                    && gb.size() <= 4
                    && ga.size() + gb.size() <= gp.max_members
                {
                    pair = Some((a, b));
                    break 'outer;
                }
            }
        }
        let Some((a, b)) = pair else {
            return;
        };
        if self.gang_roll() >= gp.merge_per_day * s {
            return;
        }
        let (into, from) = if self.gangs.list[a].size() >= self.gangs.list[b].size() {
            (a, b)
        } else {
            (b, a)
        };
        let now = self.clock;
        let gone = self.gangs.list[from].clone();
        let into_id = self.gangs.list[into].id;
        {
            let g = &mut self.gangs.list[into];
            for m in &gone.members {
                if !g.is_member(m.id) {
                    g.members.push(Member {
                        id: m.id,
                        since: now,
                        loyalty: 0.4,
                    });
                }
            }
            g.members.sort_by_key(|m| m.id);
            g.treasury = g.treasury.saturating_add(gone.treasury);
            g.fear = g.fear.max(gone.fear);
            g.act(now, format!("Si unisce a noi la banda «{}»", gone.name));
        }
        let into_name = self.gangs.list[into].name.clone();
        let player_in = self.gangs.player.gang == Some(gone.id);
        self.gangs.list.remove(from);
        self.gang_gone(gone.id);
        if player_in {
            self.gangs.player.gang = Some(into_id);
            self.gangs.player.since = Some(now);
        }
        if let Some(k) = self.gang_index(into_id) {
            self.claim_territory(k);
        }
        self.gangs.counters.merges += 1;
        self.push_event(EventKind::GangDisbanded {
            gang: gone.id,
            gang_name: gone.name,
            reason: DisbandReason::Merged(into_id),
            into_name: Some(into_name),
        });
    }

    /// The treasury pays every member (and the player, if one) a share,
    /// and helps who lacks an Attrezzo or a Vestito it can't afford.
    fn pay_members(&mut self) {
        let share = self.params.gang.member_share;
        for k in 0..self.gangs.list.len() {
            let g = &self.gangs.list[k];
            let player = self.gangs.player.gang == Some(g.id);
            let ids: Vec<NpcId> = g.members.iter().map(|m| m.id).collect();
            let heads = ids.len() as u32 + u32::from(player);
            let each = (g.treasury as f32 * share / heads.max(1) as f32).floor() as u32;
            if each > 0 {
                for &id in &ids {
                    if let Some(i) = self.npc_index(id) {
                        self.gangs.list[k].treasury -= each;
                        let t = &mut self.npcs[i].inventory.tokens;
                        *t = t.saturating_add(each);
                        self.gangs.counters.shares += u64::from(each);
                        if let Some(m) = self.gangs.list[k].member_mut(id) {
                            m.loyalty = (m.loyalty + 0.02).min(1.0);
                        }
                    }
                }
                if player {
                    self.gangs.list[k].treasury -= each;
                    self.player.tokens = self.player.tokens.saturating_add(each);
                    self.gangs.counters.shares += u64::from(each);
                }
            }
            // Help with what a member needs.
            for &id in &ids {
                let Some(i) = self.npc_index(id) else {
                    continue;
                };
                let need = [ItemKind::Attrezzo, ItemKind::Vestito]
                    .into_iter()
                    .find(|&item| self.npcs[i].wants(item));
                let Some(item) = need else {
                    continue;
                };
                let cost = self.item_value(item);
                let tokens = self.npcs[i].inventory.tokens;
                let gap = cost.saturating_sub(tokens);
                let g = &mut self.gangs.list[k];
                if gap > 0 && gap <= g.treasury / 2 {
                    g.treasury -= gap;
                    self.npcs[i].inventory.tokens += gap;
                    self.gangs.counters.shares += u64::from(gap);
                }
            }
        }
    }

    /// The leader of gang `k` may order a hit on the enemy its members hate
    /// most, if they hate it enough.
    fn maybe_order_hit(&mut self, k: usize, s: f32) {
        let gp = self.params.gang.clone();
        let now = self.clock;
        let g = &self.gangs.list[k];
        if g.hit.is_some()
            || g.leaderless
            || g.size() < 3
            || g.hit_rest_until.is_some_and(|t| now < t)
        {
            return;
        }
        let mut hatred: Vec<(Fighter, f32)> = Vec::new();
        let mut add = |who: Fighter, amount: f32| match hatred.iter_mut().find(|h| h.0 == who) {
            Some(h) => h.1 += amount,
            None => hatred.push((who, amount)),
        };
        for m in &g.members {
            if let Some(n) = self.npc(m.id) {
                // Only real grudges count: a whiff of protection doesn't.
                for gr in &n.grudges {
                    add(
                        gr.against,
                        (gr.strength - 0.1).max(0.0) * hatred_weight(gr.reason),
                    );
                }
            }
        }
        for t in &g.ties {
            if t.stance <= HIT_RIVAL_STANCE
                && let Some(r) = self.gang(t.other)
            {
                add(Fighter::Npc(r.leader), -2.0 * t.stance);
            }
        }
        let target = hatred
            .into_iter()
            .filter(|&(who, _)| match who {
                Fighter::Npc(id) => {
                    !g.is_member(id) && self.npc(id).is_some_and(|n| n.age >= MIN_AGE)
                }
                Fighter::Player => {
                    self.gangs.player.gang != Some(g.id) && self.player.dead.is_none()
                }
            })
            .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)));
        let Some((target, hate)) = target else {
            return;
        };
        if hate < gp.hit_hatred {
            return;
        }
        if self.gang_roll() < gp.hit_per_day * s * (hate / gp.hit_hatred).min(2.0) {
            self.order_hit_at(k, target);
        }
    }

    /// Gang `k` sends its fittest member (not the leader, unless alone) to
    /// kill `target`.
    fn order_hit_at(&mut self, k: usize, target: Fighter) -> bool {
        let now = self.clock;
        let g = &self.gangs.list[k];
        let pick = |w: &World, leader_too: bool| {
            g.members
                .iter()
                .filter(|m| leader_too || m.id != g.leader)
                .filter(|m| Fighter::Npc(m.id) != target)
                .filter_map(|m| {
                    let i = w.npc_index(m.id)?;
                    let n = &w.npcs[i];
                    (n.age >= VICTIM_MIN_AGE && n.health >= w.params.hurt_below)
                        .then_some((n.aggression() + n.violence, m.id))
                })
                .max_by(|a, b| a.0.total_cmp(&b.0).then(b.1.cmp(&a.1)))
                .map(|(_, id)| id)
        };
        let Some(by) = pick(self, false).or_else(|| pick(self, true)) else {
            return false;
        };
        let target_name = self.fighter_name(target);
        let days = self.params.gang.hit_days.max(1);
        let g = &mut self.gangs.list[k];
        g.hit = Some(Hit {
            target,
            by,
            since: now,
            until: now + days * MINUTES_PER_DAY,
            tries: 0,
        });
        g.tally.hits_ordered += 1;
        g.act(now, format!("Il capo ordina di far fuori {target_name}"));
        self.gangs.counters.hits_ordered += 1;
        true
    }

    // ------------------------------------------------------------------
    // Every hour
    // ------------------------------------------------------------------

    /// Hourly: see the module docs.
    pub(super) fn gangs_hour(&mut self) {
        if self.gangs.list.is_empty() {
            return;
        }
        for k in (0..self.gangs.list.len()).rev() {
            if self.gangs.list.get(k).is_some_and(|g| g.leaderless) {
                self.succession(k);
            }
        }
        let s = self.gang_strength();
        if s <= 0.0 {
            return;
        }
        for k in 0..self.gangs.list.len() {
            self.hunt(k);
        }
        let hour = self.clock.hour();
        if !(ACTIVE_FROM..=ACTIVE_TO).contains(&hour) {
            return;
        }
        for k in 0..self.gangs.list.len() {
            self.ask_pizzo(k, s);
        }
        self.rival_fights(s);
        self.go_after_player(s);
        self.demand_player_pizzo();
    }

    /// Who is sent on a hit looks for its target: walks to its carriage,
    /// and strikes when they are together (a lethal fight).
    fn hunt(&mut self, k: usize) {
        let now = self.clock;
        let Some(hit) = self.gangs.list[k].hit else {
            return;
        };
        let alive = match hit.target {
            Fighter::Npc(id) => self.npc(id).is_some(),
            Fighter::Player => self.player.dead.is_none(),
        };
        let Some(e) = self.npc_index(hit.by).filter(|_| alive && now < hit.until) else {
            self.gangs.list[k].hit = None;
            return;
        };
        if !self.can_attack(e) || !self.can_be_attacked(hit.target) {
            return;
        }
        // Tried enough: the target got away.
        if hit.tries >= HIT_TRIES {
            let name = self.fighter_name(hit.target);
            let rest = self.params.gang.hit_rest_days * MINUTES_PER_DAY / 4;
            let g = &mut self.gangs.list[k];
            g.hit = None;
            g.hit_rest_until = Some(now + rest);
            g.act(now, format!("{name} la scampa: il colpo è rimandato"));
            return;
        }
        let Some(place) = self.stand(hit.target) else {
            return;
        };
        let here = self.npcs[e].carriage;
        if here == place.carriage {
            // Up or down the stairs to it, then the blow.
            if self.npcs[e].action.station().is_none() {
                self.npcs[e].floor = place.floor;
            }
            if self.npcs[e].floor == place.floor
                && self.start_fight(e, hit.target, Motive::Hit, true)
                && let Some(h) = &mut self.gangs.list[k].hit
            {
                h.tries += 1;
            }
        } else if !matches!(self.npcs[e].action, Action::Sleep(_)) {
            let minutes = self.walk_minutes(e, place.carriage).max(1);
            self.interrupt(e, Action::Travel { to: place.carriage }, minutes);
        }
    }

    /// Members in the territory of gang `k` may ask the pizzo of who earns
    /// there: paid into the treasury, or refused (a beating).
    fn ask_pizzo(&mut self, k: usize, s: f32) {
        let gp = self.params.gang.clone();
        let g = &self.gangs.list[k];
        let gang = g.id;
        let ids: Vec<NpcId> = g.members.iter().map(|m| m.id).collect();
        for id in ids {
            let Some(k) = self.gang_index(gang) else {
                return;
            };
            let Some(i) = self.npc_index(id) else {
                continue;
            };
            let here = self.npcs[i].carriage;
            if self.npcs[i].age < VICTIM_MIN_AGE
                || !self.gangs.list[k].holds(here)
                || !self.can_attack(i)
            {
                continue;
            }
            let chance = gp.pizzo_per_hour * s * self.deterrence(here);
            if self.gang_roll() >= chance {
                continue;
            }
            let Some(v) = self.pizzo_victim(k, i) else {
                continue;
            };
            self.extort(k, i, v);
        }
    }

    /// Who member `i` of gang `k` asks the pizzo here: someone of 18+ with
    /// tokens who earns in this carriage (works here, sells at its stalls),
    /// in no gang, not asked lately; the richest.
    fn pizzo_victim(&self, k: usize, i: usize) -> Option<usize> {
        let g = &self.gangs.list[k];
        let collector = &self.npcs[i];
        let here = collector.carriage;
        let sells_here = |id: NpcId| {
            self.carriages[here.index()].kind == CarriageKind::Mercato
                && self
                    .listings_of(Seller::Npc(id))
                    .iter()
                    .any(|l| l.market == here)
        };
        self.npcs
            .iter()
            .enumerate()
            .filter(|&(j, v)| {
                j != i
                    && v.age >= VICTIM_MIN_AGE
                    && v.inventory.tokens >= VICTIM_MIN_TOKENS
                    && v.is_awake()
                    && self.same_place(collector, v)
                    && (v.workplace == Some(here) || sells_here(v.id))
                    && !g.extorted.iter().any(|&(x, _)| x == v.id)
                    && self.gang_of(v.id).is_none()
                    && self.fight_of(Fighter::Npc(v.id)).is_none()
            })
            .max_by_key(|&(_, v)| (v.inventory.tokens, std::cmp::Reverse(v.id)))
            .map(|(j, _)| j)
    }

    /// Member `i` of gang `k` asks NPC `v` the pizzo.
    fn extort(&mut self, k: usize, i: usize, v: usize) {
        let now = self.clock;
        let share = self.params.gang.pizzo_share;
        let (collector, victim) = (self.npcs[i].id, self.npcs[v].id);
        let tokens = pizzo_of(share, self.npcs[v].inventory.tokens);
        let fear = self.gangs.list[k].fear;
        let n = &self.npcs[v];
        let hurt = if n.health < self.params.hurt_below {
            0.3
        } else {
            0.0
        };
        let refuse = (0.15 + 0.4 * n.traits.boldness + 0.3 * n.aggression() - 0.5 * fear - hurt)
            .clamp(0.05, 0.9);
        self.gangs.list[k].extorted.push((victim, now));
        if self.gang_roll() < refuse {
            self.gangs.list[k].tally.pizzo_refused += 1;
            self.gangs.counters.pizzo_refused += 1;
            self.log_extortion(k, Fighter::Npc(collector), Fighter::Npc(victim), 0, false);
            if self.start_fight(i, Fighter::Npc(victim), Motive::Pizzo, false) {
                let g = &mut self.gangs.list[k];
                g.fear = (g.fear + FEAR_FIGHT).min(1.0);
            }
            return;
        }
        self.collect_pizzo(k, Fighter::Npc(collector), v, tokens);
    }

    /// `tokens` of the pizzo go from NPC `v` to gang `k`, collected by
    /// `collector`: the victim holds a grudge.
    fn collect_pizzo(&mut self, k: usize, collector: Fighter, v: usize, tokens: u32) {
        let tokens = tokens.min(self.npcs[v].inventory.tokens);
        if tokens == 0 {
            return;
        }
        self.npcs[v].inventory.tokens -= tokens;
        let g = &mut self.gangs.list[k];
        g.treasury = g.treasury.saturating_add(tokens);
        g.fear = (g.fear + FEAR_PIZZO).min(1.0);
        g.tally.pizzo_paid += 1;
        g.tally.pizzo_tokens += u64::from(tokens);
        self.gangs.counters.pizzo_paid += 1;
        self.gangs.counters.pizzo_tokens += u64::from(tokens);
        self.add_grudge(v, collector, GrudgeReason::Extorted, EXTORTED_GRUDGE);
        self.dislike(v, collector, EXTORTED_AFFINITY);
        let victim = Fighter::Npc(self.npcs[v].id);
        self.log_extortion(k, collector, victim, tokens, true);
    }

    fn log_extortion(
        &mut self,
        k: usize,
        collector: Fighter,
        victim: Fighter,
        tokens: u32,
        paid: bool,
    ) {
        let now = self.clock;
        let place = match self.stand(collector).or(self.stand(victim)) {
            Some(p) => p.carriage,
            None => self.player.place.carriage,
        };
        let (collector_name, victim_name) =
            (self.fighter_name(collector), self.fighter_name(victim));
        let g = &mut self.gangs.list[k];
        let text = if paid {
            format!("{collector_name} riscuote {tokens} gettoni da {victim_name}")
        } else {
            format!("{victim_name} si rifiuta di pagare {collector_name}")
        };
        g.act(now, text);
        let (gang, gang_name) = (g.id, g.name.clone());
        self.push_event(EventKind::GangExtortion {
            gang,
            gang_name,
            collector,
            collector_name,
            victim,
            victim_name,
            tokens,
            paid,
            place,
        });
    }

    /// A pizzo beating is over: a victim who gave in or is hurt pays.
    pub(super) fn pizzo_after_beating(&mut self, f: &Fight) {
        let Some(k) = self.gang_index_of(f.attacker) else {
            return;
        };
        let lost = |w: &World, health: f32| {
            f.reaction == Some(Reaction::GiveIn) || health < w.params.hurt_below
        };
        let share = self.params.gang.pizzo_share;
        match f.victim {
            Fighter::Npc(id) => {
                let Some(v) = self.npc_index(id) else {
                    return;
                };
                if lost(self, self.npcs[v].health) {
                    let tokens = pizzo_of(share, self.npcs[v].inventory.tokens);
                    self.gangs.counters.pizzo_beaten += 1;
                    self.collect_pizzo(k, f.attacker, v, tokens);
                }
            }
            Fighter::Player => {
                if lost(self, self.player.health) && !self.player.is_down() {
                    let tokens = pizzo_of(share, self.player.tokens);
                    self.player.tokens -= tokens;
                    let g = &mut self.gangs.list[k];
                    g.treasury = g.treasury.saturating_add(tokens);
                    g.player_due = 0;
                    g.demanded = None;
                    self.gangs.counters.pizzo_beaten += 1;
                    self.log_extortion(k, f.attacker, Fighter::Player, tokens, true);
                }
            }
        }
    }

    /// Members of rival gangs who meet in the territory of one of them may
    /// fight (one fight per pair of gangs an hour).
    fn rival_fights(&mut self, s: f32) {
        let rate = self.params.gang.rival_fight_per_hour;
        let n = self.gangs.list.len();
        for a in 0..n {
            for b in 0..n {
                if a == b || !self.gangs.list[a].is_rival(self.gangs.list[b].id) {
                    continue;
                }
                let stance = self.gangs.list[a].stance(self.gangs.list[b].id);
                let ours: Vec<NpcId> = self.gangs.list[a].members.iter().map(|m| m.id).collect();
                let theirs: Vec<NpcId> = self.gangs.list[b].members.iter().map(|m| m.id).collect();
                'members: for id in ours {
                    let Some(i) = self.npc_index(id) else {
                        continue;
                    };
                    let here = self.npcs[i].carriage;
                    let turf = self.gangs.list[a].holds(here) || self.gangs.list[b].holds(here);
                    if !turf || !self.can_attack(i) {
                        continue;
                    }
                    for &other in &theirs {
                        let Some(j) = self.npc_index(other) else {
                            continue;
                        };
                        if !self.same_place(&self.npcs[i], &self.npcs[j])
                            || !self.can_be_attacked(Fighter::Npc(other))
                        {
                            continue;
                        }
                        let chance = rate
                            * s
                            * -stance
                            * aggression_factor(self.npcs[i].aggression(), 2)
                            * self.deterrence(here);
                        if self.gang_roll() < chance
                            && self.start_fight(i, Fighter::Npc(other), Motive::Gang, false)
                        {
                            self.gangs.counters.rival_fights += 1;
                            let g = &mut self.gangs.list[a];
                            g.fear = (g.fear + FEAR_FIGHT).min(1.0);
                            g.tally.fights += 1;
                            let (x, y) = (self.npcs[i].name.clone(), self.npcs[j].name.clone());
                            let now = self.clock;
                            self.gangs.list[a].act(now, format!("{x} affronta {y}, dei rivali"));
                            break 'members;
                        }
                    }
                }
            }
        }
    }

    /// Members of a gang hostile to the player may attack it when they meet
    /// it (one a gang an hour).
    fn go_after_player(&mut self, s: f32) {
        if self.player.is_down() || self.player.is_asleep() {
            return;
        }
        let rate = self.params.gang.rival_fight_per_hour * 2.0;
        for k in 0..self.gangs.list.len() {
            let gang = self.gangs.list[k].id;
            if !self.is_gang_hostile(gang) {
                continue;
            }
            let ids: Vec<NpcId> = self.gangs.list[k].members.iter().map(|m| m.id).collect();
            for id in ids {
                let Some(i) = self.npc_index(id) else {
                    continue;
                };
                if !self.with_player(&self.npcs[i])
                    || !self.can_attack(i)
                    || !self.can_be_attacked(Fighter::Player)
                {
                    continue;
                }
                let chance = rate * s * aggression_factor(self.npcs[i].aggression().max(0.35), 2);
                if self.gang_roll() < chance
                    && self.start_fight(i, Fighter::Player, Motive::Gang, false)
                {
                    self.gangs.list[k].tally.fights += 1;
                    break;
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Fights and deaths
    // ------------------------------------------------------------------

    /// The first blow of fight `k`: the victim's gang holds a grudge against
    /// the attacker (a player who attacks a member loses its standing),
    /// members around come to help; two gangs fighting grow rivals; gang
    /// business makes the attacker's gang feared.
    pub(super) fn gang_fight_opened(&mut self, k: usize) {
        let f = self.fights[k];
        let (att, vic) = (self.gang_index_of(f.attacker), self.gang_index_of(f.victim));
        if att.is_some() && att == vic {
            return;
        }
        if let (Some(a), Some(b)) = (att, vic) {
            let (ida, idb) = (self.gangs.list[a].id, self.gangs.list[b].id);
            self.gangs.list[a].shift_stance(idb, STANCE_FIGHT);
            self.gangs.list[b].shift_stance(ida, STANCE_FIGHT);
        }
        if let Some(a) = att
            && f.motive.is_gang()
        {
            let g = &mut self.gangs.list[a];
            g.fear = (g.fear + FEAR_FIGHT).min(1.0);
            g.tally.fights += 1;
        }
        let Some(b) = vic else {
            return;
        };
        if f.motive == Motive::Defense {
            return;
        }
        if f.attacker == Fighter::Player {
            let g = &mut self.gangs.list[b];
            g.player_standing = (g.player_standing + STANDING_ATTACK).max(-2.0);
        }
        // Everyone in the gang holds it against the attacker.
        let strength = self.params.gang.protect_grudge;
        let ids: Vec<NpcId> = self.gangs.list[b].members.iter().map(|m| m.id).collect();
        for &id in &ids {
            if Fighter::Npc(id) == f.victim {
                continue;
            }
            if let Some(i) = self.npc_index(id) {
                self.add_grudge(i, f.attacker, GrudgeReason::GangMate(f.victim), strength);
            }
        }
        // Who is around may come to help (one at most).
        if self.gang_strength() <= 0.0 {
            return;
        }
        let mut helpers = 0;
        for id in ids {
            if helpers >= 1 {
                break;
            }
            if Fighter::Npc(id) == f.victim {
                continue;
            }
            let Some(i) = self.npc_index(id) else {
                continue;
            };
            if !self.together(Fighter::Npc(id), f.victim) || !self.can_attack(i) {
                continue;
            }
            let chance = 0.15 + 0.3 * self.npcs[i].aggression();
            if self.gang_roll() < chance && self.join_fight(i, f.attacker, Motive::Gang) {
                helpers += 1;
                self.gangs.counters.backups += 1;
            }
        }
    }

    /// NPC `j` is killed by `killer` (before it is removed): a member's gang
    /// wants revenge, a gang's killing is counted, an ordered hit is done
    /// ([`EventKind::GangHit`]).
    pub(super) fn gang_killing(&mut self, j: usize, killer: Fighter, place: CarriageId) {
        if self.gangs.list.is_empty() {
            return;
        }
        let now = self.clock;
        let victim = self.npcs[j].id;
        let vic = self.gang_index_of(Fighter::Npc(victim));
        if let Some(b) = vic {
            let ids: Vec<NpcId> = self.gangs.list[b].members.iter().map(|m| m.id).collect();
            for id in ids {
                if id == victim {
                    continue;
                }
                if let Some(i) = self.npc_index(id) {
                    self.add_grudge(
                        i,
                        killer,
                        GrudgeReason::GangMate(Fighter::Npc(victim)),
                        KILLED_MATE_GRUDGE,
                    );
                }
            }
            let g = &mut self.gangs.list[b];
            g.tally.members_killed += 1;
            if killer == Fighter::Player {
                g.player_standing += STANDING_KILL;
            }
            self.gangs.counters.members_killed += 1;
        }
        let Some(a) = self.gang_index_of(killer) else {
            return;
        };
        if vic == Some(a) {
            return;
        }
        let gang_fight = self.fights.iter().rev().any(|f| {
            f.attacker == killer && f.victim == Fighter::Npc(victim) && f.motive.is_gang()
        });
        let ordered = self.gangs.list[a]
            .hit
            .is_some_and(|h| h.target == Fighter::Npc(victim));
        if !gang_fight && !ordered {
            return;
        }
        let victim_name = self.npcs[j].name.clone();
        let killer_name = self.fighter_name(killer);
        let g = &mut self.gangs.list[a];
        g.tally.kills += 1;
        g.fear = (g.fear + FEAR_KILL).min(1.0);
        self.gangs.counters.kills += 1;
        if !ordered {
            g.act(now, format!("{killer_name} uccide {victim_name}"));
            return;
        }
        g.hit = None;
        g.hit_rest_until = Some(now + self.params.gang.hit_rest_days * MINUTES_PER_DAY);
        g.fear = (g.fear + FEAR_HIT).min(1.0);
        g.tally.hits_done += 1;
        g.act(
            now,
            format!("{killer_name} esegue l'ordine: {victim_name} è morto"),
        );
        let (gang, gang_name) = (g.id, g.name.clone());
        self.gangs.counters.hits_done += 1;
        if let Fighter::Npc(k) = killer {
            self.push_event(EventKind::GangHit {
                gang,
                gang_name,
                killer: k,
                killer_name,
                victim: Fighter::Npc(victim),
                victim_name,
                place,
            });
        }
    }

    /// The player fainted (or died): hits on it are over.
    pub(super) fn gang_player_down(&mut self) {
        let now = self.clock;
        for g in &mut self.gangs.list {
            if g.hit.is_some_and(|h| h.target == Fighter::Player) {
                g.hit = None;
                g.act(now, "Il giocatore ha avuto la sua lezione".to_string());
            }
        }
    }

    // ------------------------------------------------------------------
    // The player
    // ------------------------------------------------------------------

    /// NPC `i`, who just greeted the player, may invite it into its gang:
    /// a friend, in a gang that likes the player enough (a "!").
    pub(super) fn maybe_invite_player(&mut self, i: usize) {
        let now = self.clock;
        let p = &self.gangs.player;
        if self.gang_strength() <= 0.0
            || p.gang.is_some()
            || p.invite.is_some()
            || p.refused
                .is_some_and(|t| now.since(t) < REINVITE_DAYS * MINUTES_PER_DAY)
        {
            return;
        }
        let npc = &self.npcs[i];
        if npc.regard() != Regard::Friend {
            return;
        }
        let Some(gang) = self.gang_of(npc.id).map(|g| g.id) else {
            return;
        };
        if self.gang_attitude(gang) < self.params.gang.invite_attitude {
            return;
        }
        self.gangs.player.invite = Some(Invite {
            gang,
            by: npc.id,
            since: now,
            until: now + INVITE_DAYS * MINUTES_PER_DAY,
            told: false,
        });
        if let Some(t) = &mut self.npcs[i].player {
            t.wants_to_talk = true;
        }
    }

    /// What NPC `i` wants to tell the player about its gang when the chat
    /// opens (an invitation not told yet, the pizzo asked), if anything.
    pub(super) fn gang_opening(&mut self, i: usize) -> Option<String> {
        let id = self.npcs[i].id;
        let me = self.player.first_name().to_string();
        if let Some(inv) = self.gangs.player.invite.filter(|v| v.by == id && !v.told)
            && let Some(g) = self.gang(inv.gang)
        {
            let name = g.name.clone();
            if let Some(v) = &mut self.gangs.player.invite {
                v.told = true;
            }
            let line = if self.clock.0.is_multiple_of(2) {
                format!(
                    "{me}, vuoi essere dei nostri? Entra nella banda «{name}»: ti copriamo le spalle."
                )
            } else {
                format!("Senti, {me}: a «{name}» farebbe comodo uno come te. Ci stai?")
            };
            return Some(line);
        }
        let g = self.gang_of(id)?;
        (g.player_due > 0 && g.demanded.is_some()).then(|| {
            format!(
                "Sono {} gettoni per quelli {}, {me}. Paghi o no?",
                g.player_due,
                g.of_name()
            )
        })
    }

    /// The player accepts the invitation of NPC `npc`: it joins its gang.
    pub fn player_join_gang(&mut self, npc: NpcId) -> Result<GangId, GangError> {
        let now = self.clock;
        if self.gangs.player.gang.is_some() {
            return Err(GangError::AlreadyMember);
        }
        let invite = self
            .gangs
            .player
            .invite
            .filter(|i| i.by == npc)
            .ok_or(GangError::NoInvite)?;
        let k = self.gang_index(invite.gang).ok_or(GangError::NoSuchGang)?;
        let p = &mut self.gangs.player;
        p.gang = Some(invite.gang);
        p.since = Some(now);
        p.invite = None;
        let ids: Vec<NpcId> = self.gangs.list[k].members.iter().map(|m| m.id).collect();
        for id in ids {
            if let Some(i) = self.npc_index(id) {
                self.add_player_affinity(i, 0.1);
            }
        }
        let by_name = self.npc(npc).map(|n| n.name.clone()).unwrap_or_default();
        let me = self.player.name.clone();
        let g = &mut self.gangs.list[k];
        g.act(now, format!("{me} entra nella banda, portato da {by_name}"));
        let gang_name = g.name.clone();
        self.push_event(EventKind::PlayerJoinedGang {
            gang: invite.gang,
            gang_name,
            by: npc,
            by_name,
        });
        let line = "Allora ci sto. Da oggi sono dei vostri.".to_string();
        self.remember(npc, crate::chat::Speaker::Player, line);
        self.remember(
            npc,
            crate::chat::Speaker::Npc,
            "Benvenuto. Chi tocca uno di noi, tocca tutti.".to_string(),
        );
        Ok(invite.gang)
    }

    /// The player turns down the invitation of NPC `npc`.
    pub fn player_refuse_gang(&mut self, npc: NpcId) -> Result<(), GangError> {
        let invite = self
            .gangs
            .player
            .invite
            .filter(|i| i.by == npc)
            .ok_or(GangError::NoInvite)?;
        let now = self.clock;
        let p = &mut self.gangs.player;
        p.invite = None;
        p.refused = Some(now);
        if let Some(k) = self.gang_index(invite.gang) {
            self.gangs.list[k].player_standing += STANDING_REFUSE_INVITE;
        }
        if let Some(i) = self.npc_index(npc) {
            self.add_player_affinity(i, -0.05);
        }
        self.remember(
            npc,
            crate::chat::Speaker::Player,
            "Grazie, ma non fa per me.".to_string(),
        );
        self.remember(
            npc,
            crate::chat::Speaker::Npc,
            "Peccato. Pensaci, finché sei in tempo.".to_string(),
        );
        Ok(())
    }

    /// The player leaves its gang: the members take it badly (a grudge).
    pub fn player_leave_gang(&mut self) -> Result<(), GangError> {
        let now = self.clock;
        let gang = self.gangs.player.gang.ok_or(GangError::NotMember)?;
        let p = &mut self.gangs.player;
        p.gang = None;
        p.since = None;
        p.task = None;
        p.refused = Some(now);
        let Some(k) = self.gang_index(gang) else {
            return Ok(());
        };
        self.gangs.list[k].player_standing += STANDING_LEAVE;
        let ids: Vec<NpcId> = self.gangs.list[k].members.iter().map(|m| m.id).collect();
        for id in ids {
            if let Some(i) = self.npc_index(id) {
                self.add_grudge(i, Fighter::Player, GrudgeReason::Defied, DEFIED_GRUDGE);
                self.dislike(i, Fighter::Player, 0.15);
            }
        }
        let name = self.player.name.clone();
        let g = &mut self.gangs.list[k];
        g.act(now, format!("{name} ci volta le spalle"));
        let gang_name = g.name.clone();
        self.gangs.counters.left += 1;
        self.push_event(EventKind::GangLeft {
            gang,
            gang_name,
            npc: None,
            name,
            reason: LeaveReason::Chose,
        });
        Ok(())
    }

    /// The player, on its gang's task, collects the pizzo from NPC `npc`
    /// (who must be here): it pays (the player keeps
    /// [`crate::GangParams::player_task_share`]) or refuses.
    pub fn player_collect_pizzo(&mut self, npc: NpcId) -> Result<PizzoOutcome, GangError> {
        let task = self
            .gangs
            .player
            .task
            .filter(|t| t.victim == npc)
            .ok_or(GangError::NoTask)?;
        let k = self.gang_index(task.gang).ok_or(GangError::NoSuchGang)?;
        let v = self.npc_index(npc).ok_or(GangError::NoSuchNpc)?;
        if !self.with_player(&self.npcs[v]) || !self.npcs[v].is_awake() {
            return Err(GangError::NotHere);
        }
        let of = self.gangs.list[k].of_name();
        self.remember(
            npc,
            crate::chat::Speaker::Player,
            format!(
                "Sono qui per il pizzo, quello {of}: {} gettoni.",
                task.tokens
            ),
        );
        let n = &self.npcs[v];
        let scared = n.health < self.params.hurt_below
            || self.player.reputation() >= crate::combat::Reputation::Violent;
        if n.traits.boldness > 0.6 && !scared {
            self.dislike(v, Fighter::Player, 0.05);
            self.remember(
                npc,
                crate::chat::Speaker::Npc,
                "Io a voi non do niente. Sparisci!".to_string(),
            );
            return Err(GangError::Refused);
        }
        let paid = task.tokens.min(self.npcs[v].inventory.tokens);
        let share = (paid as f32 * self.params.gang.player_task_share).round() as u32;
        self.npcs[v].inventory.tokens -= paid;
        self.player.tokens = self.player.tokens.saturating_add(share);
        let g = &mut self.gangs.list[k];
        g.treasury = g.treasury.saturating_add(paid - share);
        g.player_standing += STANDING_TASK;
        g.tally.pizzo_paid += 1;
        g.tally.pizzo_tokens += u64::from(paid);
        self.gangs.counters.pizzo_paid += 1;
        self.gangs.counters.pizzo_tokens += u64::from(paid);
        let p = &mut self.gangs.player;
        p.task = None;
        p.tasks_done += 1;
        p.collected += u64::from(paid);
        self.add_grudge(v, Fighter::Player, GrudgeReason::Extorted, EXTORTED_GRUDGE);
        self.dislike(v, Fighter::Player, EXTORTED_AFFINITY);
        self.remember(
            npc,
            crate::chat::Speaker::Npc,
            "Tieni… e ora lasciami in pace.".to_string(),
        );
        self.log_extortion(k, Fighter::Player, Fighter::Npc(npc), paid, true);
        Ok(PizzoOutcome { paid, share })
    }

    /// The player pays the pizzo it owes `gang`. Returns the tokens paid.
    pub fn player_pay_pizzo(&mut self, gang: GangId) -> Result<u32, GangError> {
        let k = self.gang_index(gang).ok_or(GangError::NoSuchGang)?;
        let due = self.gangs.list[k].player_due;
        if due == 0 {
            return Err(GangError::NothingDue);
        }
        if self.player.tokens < due {
            return Err(GangError::TooPoor);
        }
        self.player.tokens -= due;
        let g = &mut self.gangs.list[k];
        g.treasury = g.treasury.saturating_add(due);
        g.player_due = 0;
        g.demanded = None;
        g.player_standing += STANDING_PIZZO_PAID;
        g.tally.pizzo_paid += 1;
        g.tally.pizzo_tokens += u64::from(due);
        self.gangs.counters.pizzo_paid += 1;
        self.gangs.counters.pizzo_tokens += u64::from(due);
        let collector = self.demander(k).map_or(Fighter::Player, Fighter::Npc);
        self.log_extortion(k, collector, Fighter::Player, due, true);
        Ok(due)
    }

    /// The player refuses the pizzo it owes `gang`: the gang takes it
    /// badly, and a member here may teach it a lesson.
    pub fn player_refuse_pizzo(&mut self, gang: GangId) -> Result<(), GangError> {
        let k = self.gang_index(gang).ok_or(GangError::NoSuchGang)?;
        if self.gangs.list[k].player_due == 0 {
            return Err(GangError::NothingDue);
        }
        self.refuse_player_pizzo(k);
        Ok(())
    }

    fn refuse_player_pizzo(&mut self, k: usize) {
        let g = &mut self.gangs.list[k];
        g.player_due = 0;
        g.demanded = None;
        g.player_standing += STANDING_PIZZO_REFUSED;
        g.tally.pizzo_refused += 1;
        self.gangs.counters.pizzo_refused += 1;
        let collector = self.demander(k);
        self.log_extortion(
            k,
            collector.map_or(Fighter::Player, Fighter::Npc),
            Fighter::Player,
            0,
            false,
        );
        let ids: Vec<NpcId> = self.gangs.list[k].members.iter().map(|m| m.id).collect();
        for id in ids {
            let Some(i) = self.npc_index(id) else {
                continue;
            };
            if self.with_player(&self.npcs[i]) {
                self.add_grudge(i, Fighter::Player, GrudgeReason::Defied, DEFIED_GRUDGE);
            }
        }
        if self.gang_strength() > 0.0
            && let Some(i) = collector.and_then(|id| self.npc_index(id))
            && self.with_player(&self.npcs[i])
        {
            self.start_fight(i, Fighter::Player, Motive::Pizzo, false);
        }
    }

    /// A member of gang `k` with the player (the first one), if any.
    fn demander(&self, k: usize) -> Option<NpcId> {
        self.gangs.list[k].members.iter().map(|m| m.id).find(|&id| {
            self.npc(id)
                .is_some_and(|n| n.is_awake() && self.with_player(n))
        })
    }

    /// The player sold at the stalls of `market` for `cost` tokens: the gangs
    /// holding it (not its own) ask their share.
    pub(super) fn gang_player_sale(&mut self, market: CarriageId, cost: u32) {
        if self.gangs.list.is_empty() || cost == 0 {
            return;
        }
        let share = self.params.gang.player_pizzo_share;
        let mine = self.gangs.player.gang;
        for g in &mut self.gangs.list {
            if g.holds(market) && Some(g.id) != mine {
                let due = (cost as f32 * share).ceil() as u32;
                g.player_due = g.player_due.saturating_add(due);
            }
        }
    }

    /// Hourly: a member with the player asks it the pizzo it owes (a line
    /// and a "!"); unpaid a day later, it counts as refused.
    fn demand_player_pizzo(&mut self) {
        let now = self.clock;
        if self.player.is_asleep() {
            return;
        }
        for k in 0..self.gangs.list.len() {
            let g = &self.gangs.list[k];
            let (due, demanded, of) = (g.player_due, g.demanded, g.of_name());
            if due == 0 {
                continue;
            }
            match demanded {
                Some(t) if now.since(t) >= DEMAND_MINUTES => self.refuse_player_pizzo(k),
                Some(_) => {}
                None => {
                    let Some(id) = self.demander(k) else {
                        continue;
                    };
                    let me = self.player.first_name().to_string();
                    self.gangs.list[k].demanded = Some(now);
                    if let Some(i) = self.npc_index(id) {
                        let t = self.npcs[i].player.get_or_insert_with(PlayerTie::default);
                        t.wants_to_talk = true;
                    }
                    self.player.greetings.push(Greeting {
                        npc: id,
                        text: format!("Ehi {me}: sono {due} gettoni per quelli {of}."),
                        since: now,
                        until: now + super::GREET_MINUTES,
                    });
                }
            }
        }
    }

    /// Midnight: invitations and tasks expire, a member gets a new task.
    fn player_gang_midnight(&mut self) {
        let now = self.clock;
        let p = &mut self.gangs.player;
        if p.invite.is_some_and(|i| i.until <= now) {
            p.invite = None;
        }
        if let Some(t) = p.task
            && t.until <= now
        {
            p.task = None;
            if let Some(k) = self.gang_index(t.gang) {
                self.gangs.list[k].player_standing += STANDING_TASK_MISSED;
            }
        }
        let Some(k) = self
            .gangs
            .player
            .gang
            .filter(|_| self.gangs.player.task.is_none())
            .and_then(|g| self.gang_index(g))
        else {
            return;
        };
        // The richest outsider who earns in the territory.
        let g = &self.gangs.list[k];
        let victim = self
            .npcs
            .iter()
            .filter(|v| {
                v.age >= VICTIM_MIN_AGE
                    && v.inventory.tokens >= 2 * VICTIM_MIN_TOKENS
                    && v.workplace.is_some_and(|w| g.holds(w))
                    && self.gang_of(v.id).is_none()
            })
            .max_by_key(|v| (v.inventory.tokens, std::cmp::Reverse(v.id)))
            .map(|v| (v.id, v.name.clone(), v.inventory.tokens));
        let Some((victim, name, tokens)) = victim else {
            return;
        };
        let tokens = pizzo_of(self.params.gang.pizzo_share, tokens);
        self.gangs.player.task = Some(GangTask {
            gang: g.id,
            victim,
            tokens,
            since: now,
            until: now + TASK_DAYS * MINUTES_PER_DAY,
        });
        self.gangs.list[k].act(now, format!("Al giocatore: riscuotere il pizzo da {name}"));
    }
}
