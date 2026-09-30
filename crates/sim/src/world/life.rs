//! Ciclo di vita: età, relazioni, coppie, nascite, morti, forza lavoro.
//!
//! Everything here but chats (see [`World::add_affinity`]) and starvation
//! runs once per game day, at midnight ([`World::daily_life`]), in this order:
//! 1. aging: ages are refreshed; turning 65 retires (job cleared);
//! 2. old-age deaths (Gompertz curve, [`crate::SimParams::mortality_per_year`]);
//! 3. workforce: jobless adults (e.g. who just turned 18) get the job most
//!    needed, Operai are moved to food/trade jobs lacking staff (and back
//!    when those are clearly overstaffed); quotas follow the population;
//! 4. friend ties fade;
//! 5. couple proposals between single adults with enough affinity: the
//!    other one deliberates (accept, refuse, ask for time);
//! 6. births: a couple considering a child is refused if the administration
//!    does not allow it ([`World::birth_permit`]), otherwise the woman
//!    deliberates whether to try now. A refusal may lead to protests.
//!
//! Deliberations are resolved at the end of the tick (see `deliberate.rs`):
//! the couple forms, or the child is born, when the answer comes.
//!
//! Beds: every NPC, babies included, needs a bed (everyone sleeps in a Bed
//! station), so the population is capped by the beds of the Dormitori.

use rand::RngExt;
use rand::seq::IndexedRandom;
use serde::{Deserialize, Serialize};

use super::{NEEDED_JOBS, World, job_quotas, max_population_for};
use crate::action::Action;
use crate::carriage::{CarriageKind, StationKind};
use crate::combat::MAX_HEALTH;
use crate::deliberation::DeliberationKind;
use crate::event::{BirthDenial, DeathCause, Event, EventKind};
use crate::ids::{CarriageId, NpcId};
use crate::item::ItemKind;
use crate::names;
use crate::npc::{
    Inventory, Job, LifeStage, MAX_RELATIONS, Needs, Npc, Relation, RelationKind, Sex, Traits,
};
use crate::personality::Personality;

/// A job is overstaffed (Contadini, Cuochi, Mercanti sent back to the
/// Officine) above its quota plus a tenth of it.
const SURPLUS_SLACK: usize = 10;

/// Life-cycle counters since the world was generated. "Today" means since
/// the last midnight.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifeCounters {
    pub births_total: u64,
    pub deaths_total: u64,
    /// Deaths per cause, indexed by [`DeathCause::index`].
    pub deaths_by_cause: [u64; DeathCause::COUNT],
    /// Births refused by the administration (each one, logged or not).
    pub births_denied_total: u64,
    pub couples_formed_total: u64,
    pub births_today: u32,
    pub deaths_today: u32,
}

impl World {
    /// Bed capacity of a carriage for its NPCs (0 if it is not a
    /// Dormitorio; the player's private bed does not count).
    pub fn beds(&self, carriage: CarriageId) -> usize {
        self.carriage(carriage).map_or(0, |c| {
            c.stations
                .iter()
                .filter(|s| s.kind == StationKind::Bed && s.is_shared())
                .map(|s| usize::from(s.capacity))
                .sum()
        })
    }

    /// Beds on the whole train.
    pub fn total_beds(&self) -> usize {
        self.carriages.iter().map(|c| self.beds(c.id)).sum()
    }

    /// NPCs whose home is `carriage`.
    pub fn residents(&self, carriage: CarriageId) -> usize {
        self.npcs.iter().filter(|n| n.home == carriage).count()
    }

    /// Population above which the administration denies births: a fraction
    /// ([`crate::SimParams::birth_max_bed_occupancy`]) of all beds, plus
    /// [`crate::SimParams::protest_birth_bonus`] while a concession won by
    /// protests lasts ([`World::birth_bonus_until`]).
    pub fn max_population(&self) -> usize {
        let beds = self.total_beds();
        if self.birth_bonus_until().is_some() {
            let share = (self.params.birth_max_bed_occupancy + self.params.protest_birth_bonus)
                .clamp(0.0, 1.0);
            (beds as f32 * share).floor() as usize
        } else {
            max_population_for(&self.params, beds)
        }
    }

    /// Whether `id` was generated with the world (not born in the simulation).
    pub fn is_founder(&self, id: NpcId) -> bool {
        id.0 < self.founders
    }

    /// Whether the administration would allow a birth right now; `Ok` holds
    /// the Dormitorio with the most free beds.
    pub fn birth_permit(&self) -> Result<CarriageId, BirthDenial> {
        self.birth_permit_with(&self.resident_counts())
    }

    fn birth_permit_with(&self, residents: &[usize]) -> Result<CarriageId, BirthDenial> {
        let population = self.npcs.len();
        if population >= self.max_population() {
            return Err(BirthDenial::Overcrowded);
        }
        let dorm = self.free_dorm(residents, None).ok_or(BirthDenial::NoBeds)?;
        let needed = population as f32 * self.params.birth_min_razioni_per_person;
        if self.available(ItemKind::Razione) < needed {
            return Err(BirthDenial::NotEnoughFood);
        }
        Ok(dorm)
    }

    /// Residents per carriage, indexed like `carriages`.
    pub(super) fn resident_counts(&self) -> Vec<usize> {
        let mut counts = vec![0; self.carriages.len()];
        for npc in &self.npcs {
            counts[npc.home.index()] += 1;
        }
        counts
    }

    fn has_free_bed(&self, residents: &[usize], dorm: CarriageId) -> bool {
        residents[dorm.index()] < self.beds(dorm)
    }

    /// `preferred` if it has a free bed, else the Dormitorio with the most
    /// free beds (nearest to `preferred`, then head first, on ties).
    fn free_dorm(&self, residents: &[usize], preferred: Option<CarriageId>) -> Option<CarriageId> {
        if let Some(p) = preferred
            && self.has_free_bed(residents, p)
        {
            return Some(p);
        }
        let near = preferred.unwrap_or(CarriageId(0));
        self.carriages
            .iter()
            .filter(|c| c.kind == CarriageKind::Dormitorio)
            .map(|c| {
                (
                    self.beds(c.id).saturating_sub(residents[c.id.index()]),
                    c.id,
                )
            })
            .filter(|&(free, _)| free > 0)
            .min_by_key(|&(free, id)| (std::cmp::Reverse(free), id.distance(near), id))
            .map(|(_, id)| id)
    }

    /// Pushes an event without trimming the log (trimmed at the end of the tick).
    pub(super) fn push_event(&mut self, kind: EventKind) {
        self.events.push(Event {
            time: self.clock,
            kind,
        });
    }

    // ------------------------------------------------------------------
    // Relations
    // ------------------------------------------------------------------

    /// Changes the affinity between NPCs `i` and `j` (indices) on both sides,
    /// creating a friend tie if there is none (and `delta > 0`). A full list
    /// forgets its weakest friend for a stronger new one.
    pub(super) fn add_affinity(&mut self, i: usize, j: usize, delta: f32) {
        for (a, b) in [(i, j), (j, i)] {
            let other = self.npcs[b].id;
            let relations = &mut self.npcs[a].relations;
            if let Some(r) = relations.iter_mut().find(|r| r.other == other) {
                r.affinity = (r.affinity + delta).clamp(-1.0, 1.0);
                continue;
            }
            if delta <= 0.0 {
                continue;
            }
            let tie = Relation {
                other,
                kind: RelationKind::Friend,
                affinity: delta.min(1.0),
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
                .min_by(|x, y| x.affinity.total_cmp(&y.affinity))
                && weakest.affinity < tie.affinity
            {
                *weakest = tie;
            }
        }
    }

    // ------------------------------------------------------------------
    // Death
    // ------------------------------------------------------------------

    /// Removes NPC `i` (index): frees its station, passes its tokens on
    /// (partner, else children in equal shares, else to the treasury), removes
    /// every tie to it, and logs the death (and widowhood).
    pub(super) fn kill(&mut self, i: usize, cause: DeathCause) {
        let now = self.clock;
        let npc = self.npcs.remove(i);
        if let Some(station) = npc.action.station() {
            self.release(npc.carriage, station);
        }
        let age = npc.age_years(now, self.params.days_per_year);
        self.life.deaths_total += 1;
        self.life.deaths_today += 1;
        self.life.deaths_by_cause[cause.index()] += 1;
        self.push_event(EventKind::NpcDied {
            npc: npc.id,
            name: npc.name.clone(),
            cause,
            age,
            sex: npc.sex,
        });

        let tokens = npc.inventory.tokens;
        if let Some(p) = npc.partner().and_then(|p| self.npc_index(p)) {
            let partner = &mut self.npcs[p];
            let kept = tokens.min(u32::MAX - partner.inventory.tokens);
            partner.inventory.tokens += kept;
            self.economy.treasury += u64::from(tokens - kept);
            self.economy.counters.inherited += u64::from(kept);
            let partner = &self.npcs[p];
            let kind = EventKind::Widowed {
                npc: partner.id,
                name: partner.name.clone(),
                sex: partner.sex,
                partner: npc.id,
                partner_name: npc.name.clone(),
            };
            self.push_event(kind);
        } else {
            let heirs: Vec<usize> = npc.children().filter_map(|c| self.npc_index(c)).collect();
            if heirs.is_empty() {
                self.economy.treasury += u64::from(tokens);
                self.economy.counters.estates += u64::from(tokens);
            } else {
                let n = heirs.len() as u32;
                for (k, &h) in heirs.iter().enumerate() {
                    let share = tokens / n + u32::from((k as u32) < tokens % n);
                    let heir = &mut self.npcs[h].inventory;
                    let kept = share.min(u32::MAX - heir.tokens);
                    heir.tokens += kept;
                    self.economy.treasury += u64::from(share - kept);
                    self.economy.counters.inherited += u64::from(kept);
                }
            }
        }
        for other in &mut self.npcs {
            other.relations.retain(|r| r.other != npc.id);
        }
        // Belongings: home's storage, listings to the Mercato (`stalls.rs`).
        self.estate_goods(&npc);
        self.forget_deliberations_of(npc.id);
        self.drop_conversation_of(npc.id);
        // Grudges against it are dropped, its fights end.
        self.forget_fighter(npc.id);
        // Its gang loses a member (maybe its leader), hits on it are over.
        self.gang_member_gone(npc.id);
    }

    // ------------------------------------------------------------------
    // Daily pass
    // ------------------------------------------------------------------

    /// Midnight: the whole life cycle (see the module docs).
    pub(super) fn daily_life(&mut self) {
        self.life.births_today = 0;
        self.life.deaths_today = 0;
        let came_of_age = self.age_everyone();
        self.old_age_deaths();
        self.staff_workforce();
        let mut residents = self.resident_counts();
        for id in came_of_age {
            let Some(i) = self.npc_index(id) else {
                continue;
            };
            // Crowded home: move to the Dormitorio with most free beds.
            let home = self.npcs[i].home;
            if residents[home.index()] > self.beds(home)
                && let Some(dorm) = self.free_dorm(&residents, None)
            {
                residents[home.index()] -= 1;
                residents[dorm.index()] += 1;
                self.npcs[i].home = dorm;
            }
            let npc = &self.npcs[i];
            let kind = EventKind::CameOfAge {
                npc: npc.id,
                name: npc.name.clone(),
                sex: npc.sex,
                job: npc.job,
            };
            self.push_event(kind);
        }
        self.fade_friendships();
        self.prune_cooldowns();
        self.propose_couples();
        self.births(&residents);
    }

    /// Refreshes every cached age; retires who turned 65. Returns who turned 18.
    fn age_everyone(&mut self) -> Vec<NpcId> {
        let now = self.clock;
        let per_year = self.params.days_per_year;
        let mut came_of_age = Vec::new();
        let mut retired = Vec::new();
        for npc in &mut self.npcs {
            let age = npc.age_years(now, per_year);
            if age == npc.age {
                continue;
            }
            let (before, after) = (npc.stage(), LifeStage::of_age(age));
            npc.age = age;
            if before == after {
                continue;
            }
            match after {
                LifeStage::Adulto => came_of_age.push(npc.id),
                LifeStage::Anziano => {
                    let job = npc.job.take();
                    npc.workplace = None;
                    retired.push(EventKind::Retired {
                        npc: npc.id,
                        name: npc.name.clone(),
                        sex: npc.sex,
                        job,
                    });
                }
                _ => {}
            }
        }
        for kind in retired {
            self.push_event(kind);
        }
        came_of_age
    }

    fn old_age_deaths(&mut self) {
        let per_year = self.params.days_per_year.max(1) as f32;
        let mut dying = Vec::new();
        for (i, npc) in self.npcs.iter().enumerate() {
            let hazard = self.params.mortality_per_year(npc.age as f32 + 0.5) / per_year;
            let chance = 1.0 - (-hazard).exp();
            if self.rng.random::<f32>() < chance {
                dying.push(i);
            }
        }
        for &i in dying.iter().rev() {
            self.kill(i, DeathCause::OldAge);
        }
    }

    /// Friend ties fade towards 0 and are forgotten near it.
    fn fade_friendships(&mut self) {
        let decay = self.params.affinity_decay_per_day;
        for npc in &mut self.npcs {
            npc.relations.retain_mut(|r| {
                if r.kind != RelationKind::Friend {
                    return true;
                }
                r.affinity -= r.affinity.signum() * decay.min(r.affinity.abs());
                r.affinity.abs() > 0.01
            });
        }
    }

    pub(super) fn can_couple(&self, a: &Npc, b: &Npc) -> bool {
        let adult = |n: &Npc| n.age >= LifeStage::ADULTO_FROM;
        adult(a)
            && adult(b)
            && a.partner().is_none()
            && b.partner().is_none()
            && (a.sex != b.sex || self.params.same_sex_couples)
            && a.age.abs_diff(b.age) <= self.params.couple_max_age_gap
    }

    /// Makes NPCs `i` and `j` (indices) partners. They move in together if
    /// one's Dormitorio has a free bed (the woman moves first).
    pub(super) fn couple(&mut self, i: usize, j: usize, residents: &mut [usize]) {
        let (a, b) = (self.npcs[i].id, self.npcs[j].id);
        for (x, other) in [(i, b), (j, a)] {
            if let Some(r) = self.npcs[x].relation_mut(other) {
                r.kind = RelationKind::Partner;
            }
        }
        // Who moves: the woman (or, same sex, the one listed first).
        let (mover, host) =
            if self.npcs[i].sex == Sex::Female || self.npcs[i].sex == self.npcs[j].sex {
                (i, j)
            } else {
                (j, i)
            };
        let mut moved_to = None;
        if self.npcs[mover].home != self.npcs[host].home {
            for (m, h) in [(mover, host), (host, mover)] {
                let dest = self.npcs[h].home;
                if self.has_free_bed(residents, dest) {
                    residents[self.npcs[m].home.index()] -= 1;
                    residents[dest.index()] += 1;
                    self.npcs[m].home = dest;
                    moved_to = Some(dest);
                    break;
                }
            }
        }
        self.life.couples_formed_total += 1;
        let kind = EventKind::Coupled {
            npc: a,
            name: self.npcs[i].name.clone(),
            partner: b,
            partner_name: self.npcs[j].name.clone(),
            moved_to,
        };
        self.push_event(kind);
    }

    /// Couples with a fertile woman may consider a child: refused if the
    /// administration does not allow it, else the woman deliberates.
    fn births(&mut self, residents: &[usize]) {
        let p = &self.params;
        let fertile = p.fertile_min_age..=p.fertile_max_age;
        let daily_chance = p.birth_chance_per_year / p.days_per_year.max(1) as f32;
        let spacing = p.birth_spacing_years;
        let n = self.npcs.len();
        for i in 0..n {
            let mother = &self.npcs[i];
            if mother.sex != Sex::Female || !fertile.contains(&mother.age) {
                continue;
            }
            let Some(j) = mother.partner().and_then(|f| self.npc_index(f)) else {
                continue;
            };
            if self.npcs[j].sex != Sex::Male {
                continue;
            }
            let toddler = mother
                .children()
                .filter_map(|c| self.npc(c))
                .any(|c| c.age < spacing);
            if toddler || self.is_involved(mother.id) || self.rng.random::<f32>() >= daily_chance {
                continue;
            }
            match self.birth_permit_with(residents) {
                Ok(_) => {
                    let (mother, partner) = (self.npcs[i].id, self.npcs[j].id);
                    self.open_deliberation(mother, DeliberationKind::HaveChild { partner });
                }
                Err(reason) => self.deny_birth(i, j, reason),
            }
        }
    }

    /// Mother `i` and father `j` (indices) try for a child: born if the
    /// administration still allows it, else denied.
    pub(super) fn attempt_birth(&mut self, i: usize, j: usize, residents: &mut [usize]) {
        match self.birth_permit_with(residents) {
            Ok(fallback) => {
                let home = self
                    .free_dorm(residents, Some(self.npcs[i].home))
                    .unwrap_or(fallback);
                residents[home.index()] += 1;
                self.give_birth(i, j, home);
            }
            Err(reason) => self.deny_birth(i, j, reason),
        }
    }

    /// The administration refuses mother `i` and father `j` a child (logged at
    /// most every [`crate::SimParams::birth_denied_log_days`]); they may protest.
    fn deny_birth(&mut self, i: usize, j: usize, reason: BirthDenial) {
        self.life.births_denied_total += 1;
        let now = self.clock;
        let every = self.params.birth_denied_log_days.max(1) * crate::MINUTES_PER_DAY;
        if self
            .last_birth_denied_log
            .is_none_or(|last| now.since(last) >= every)
        {
            self.last_birth_denied_log = Some(now);
            let (m, f) = (&self.npcs[i], &self.npcs[j]);
            let kind = EventKind::BirthDenied {
                mother: m.id,
                mother_name: m.name.clone(),
                father: f.id,
                father_name: f.name.clone(),
                reason,
            };
            self.push_event(kind);
        }
        self.protest_on_denial(i, j);
    }

    /// A child of `mother` and `father` (indices) is born, living in `home`.
    fn give_birth(&mut self, mother: usize, father: usize, home: CarriageId) {
        let now = self.clock;
        let sex = if self.rng.random_bool(0.5) {
            Sex::Female
        } else {
            Sex::Male
        };
        // Avoid a living sibling's first name when possible.
        let siblings: Vec<NpcId> = {
            let mut s: Vec<NpcId> = self.npcs[mother]
                .children()
                .chain(self.npcs[father].children())
                .collect();
            s.sort();
            s.dedup();
            s
        };
        let taken: Vec<String> = siblings
            .iter()
            .filter_map(|&c| self.npc(c))
            .map(|n| n.first_name().to_string())
            .collect();
        let pool = names::first_names(sex);
        let mut first = pool.choose(&mut self.rng).copied().unwrap_or("Anna");
        for _ in 0..3 {
            if !taken.iter().any(|t| t == first) {
                break;
            }
            first = pool.choose(&mut self.rng).copied().unwrap_or("Anna");
        }
        let traits = Traits::inherited(
            self.npcs[mother].traits,
            self.npcs[father].traits,
            &mut self.rng,
        );
        let personality = Personality::inherited(
            self.npcs[mother].personality(),
            self.npcs[father].personality(),
            traits,
            &mut self.rng,
        );
        let (m, f) = (&self.npcs[mother], &self.npcs[father]);
        let meal_shift = self.params.meal_shift(m);
        let name = format!("{first} {}", f.surname());
        let id = NpcId(self.next_npc_id);
        self.next_npc_id += 1;
        let mut relations = vec![
            Relation {
                other: m.id,
                kind: RelationKind::Parent,
                affinity: 0.8,
            },
            Relation {
                other: f.id,
                kind: RelationKind::Parent,
                affinity: 0.8,
            },
        ];
        relations.extend(siblings.iter().map(|&other| Relation {
            other,
            kind: RelationKind::Sibling,
            affinity: 0.5,
        }));
        let born = EventKind::Born {
            npc: id,
            name: name.clone(),
            sex,
            mother: m.id,
            father: f.id,
            mother_name: m.name.clone(),
            father_name: f.name.clone(),
        };
        let baby = Npc {
            id,
            name,
            sex,
            born: now.0 as i64,
            age: 0,
            carriage: m.carriage,
            floor: m.floor,
            home,
            job: None,
            workplace: None,
            needs: Needs::default(),
            inventory: Inventory::new(0, None, Some(1.0)),
            action: Action::Idle,
            action_since: now,
            action_until: now,
            starving_minutes: 0,
            relations,
            traits,
            // The family eats together.
            meal_shift: Some(meal_shift),
            personality: Some(personality),
            player: None,
            health: MAX_HEALTH,
            injury: 0.0,
            grudges: Vec::new(),
            violence: 0.0,
            last_attacker: None,
        };
        for &parent in &[mother, father] {
            self.npcs[parent].relations.push(Relation {
                other: id,
                kind: RelationKind::Child,
                affinity: 0.8,
            });
        }
        for &s in &siblings {
            if let Some(k) = self.npc_index(s) {
                self.npcs[k].relations.push(Relation {
                    other: id,
                    kind: RelationKind::Sibling,
                    affinity: 0.5,
                });
            }
        }
        // Ids only grow: `npcs` stays sorted.
        self.npcs.push(baby);
        self.life.births_total += 1;
        self.life.births_today += 1;
        self.push_event(born);
    }

    /// Gives jobless adults (e.g. who just turned 18) the most needed job,
    /// then moves Operai to food/trade jobs lacking staff, and back when those
    /// are clearly overstaffed. Quotas follow the current population (and,
    /// for the Contadini, their tools and the Serre: see [`World::farm_quota`]).
    fn staff_workforce(&mut self) {
        let mut quotas = job_quotas(
            &self.params,
            &self.catalog,
            &self.carriages,
            self.npcs.len(),
        );
        let farm = Job::Contadino.index();
        quotas[farm] = self.farm_quota(quotas[farm]);
        let mut counts = vec![0usize; self.catalog.job_count()];
        let mut staff = vec![0usize; self.carriages.len()];
        for npc in &self.npcs {
            if let Some(job) = npc.job {
                counts[job.index()] += 1;
            }
            if let Some(w) = npc.workplace {
                staff[w.index()] += 1;
            }
        }
        let has_operai = self
            .carriages
            .iter()
            .any(|c| c.kind == Job::Operaio.workplace_kind());
        let most_needed = |counts: &[usize]| {
            NEEDED_JOBS
                .into_iter()
                .find(|j| counts[j.index()] < quotas[j.index()])
                .or(has_operai.then_some(Job::Operaio))
        };

        for i in 0..self.npcs.len() {
            let npc = &self.npcs[i];
            if npc.stage().works()
                && npc.job.is_none()
                && let Some(job) = most_needed(&counts)
            {
                self.assign_job(i, Some(job), &mut counts, &mut staff);
            }
        }
        // Deficits: Operai (the most recent ones first) change job.
        for job in NEEDED_JOBS {
            while counts[job.index()] < quotas[job.index()] {
                let Some(i) = self.npcs.iter().rposition(|n| n.job == Some(Job::Operaio)) else {
                    break;
                };
                self.assign_job(i, Some(job), &mut counts, &mut staff);
            }
        }
        // Clear surpluses (population fell): back to the Officine.
        if has_operai {
            for job in NEEDED_JOBS {
                let quota = quotas[job.index()];
                let slack = (quota / SURPLUS_SLACK).max(2);
                while counts[job.index()] > quota + slack {
                    let Some(i) = self.npcs.iter().rposition(|n| n.job == Some(job)) else {
                        break;
                    };
                    self.assign_job(i, Some(Job::Operaio), &mut counts, &mut staff);
                }
            }
        }
        // Jobs the Custode added (see `custode.rs`).
        self.staff_added_jobs(&mut counts, &mut staff);
    }

    /// Sets NPC `i`'s job and picks its workplace: the carriage of the right
    /// kind with the fewest workers per station seat (nearest to home on ties).
    pub(super) fn assign_job(
        &mut self,
        i: usize,
        job: Option<Job>,
        counts: &mut [usize],
        staff: &mut [usize],
    ) {
        let npc = &self.npcs[i];
        if let Some(old) = npc.job {
            counts[old.index()] -= 1;
        }
        if let Some(w) = npc.workplace {
            staff[w.index()] -= 1;
        }
        let home = npc.home;
        let workplace = job.and_then(|job| {
            self.carriages
                .iter()
                .filter(|c| c.kind == job.workplace_kind())
                .map(|c| {
                    let seats: usize = c
                        .stations
                        .iter()
                        .filter(|s| s.kind == job.station_kind())
                        .map(|s| usize::from(s.capacity))
                        .sum();
                    let load = staff[c.id.index()] as f32 / seats.max(1) as f32;
                    (load, c.id.distance(home), c.id)
                })
                .min_by(|a, b| a.0.total_cmp(&b.0).then((a.1, a.2).cmp(&(b.1, b.2))))
                .map(|(_, _, id)| id)
        });
        if let Some(job) = job {
            counts[job.index()] += 1;
        }
        if let Some(w) = workplace {
            staff[w.index()] += 1;
        }
        let npc = &mut self.npcs[i];
        npc.job = job;
        npc.workplace = workplace;
    }

    /// Family and friends, for [`World::npc_context`].
    pub(super) fn family_context(&self, npc: &Npc) -> String {
        let mut text = String::new();
        let name_of = |id: NpcId| self.npc(id).map(|n| n.name.as_str());
        match npc.partner().and_then(name_of) {
            Some(partner) => text.push_str(&format!("È in coppia con {partner}. ")),
            None if npc.age >= LifeStage::ADULTO_FROM => text.push_str("È single. "),
            None => {}
        }
        if npc.age < LifeStage::ADULTO_FROM {
            let parents: Vec<&str> = npc.parents().filter_map(name_of).collect();
            let child = npc.sex.pick("Figlia", "Figlio");
            match parents.as_slice() {
                [] => text.push_str(&format!(
                    "{} del treno, senza genitori. ",
                    npc.sex.pick("Orfana", "Orfano")
                )),
                [one] => text.push_str(&format!("{child} di {one}. ")),
                [a, b, ..] => text.push_str(&format!("{child} di {a} e {b}. ")),
            }
        }
        match npc.children().count() {
            0 => {}
            1 => text.push_str("Ha un figlio. "),
            n => text.push_str(&format!("Ha {n} figli. ")),
        }
        if let Some(friend) = npc.closest_friend()
            && let Some(name) = name_of(friend.other)
        {
            text.push_str(&format!("Il legame d'amicizia più stretto è con {name}. "));
        }
        text
    }
}
