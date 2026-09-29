//! Economia: tesoreria, salari, sussidi, tasse e contabilità.
//!
//! Money is a closed loop: the tokens in circulation are those in the NPCs'
//! pockets plus the administration's [`Economy::treasury`], and their sum
//! ([`World::money_supply`]) only changes when the player (who lives outside
//! the sim) buys something ([`World::player_buy`], money in) or sells
//! something to a Mercato ([`World::player_sell`], money out).
//!
//! - Out of the treasury: wages (per minute of work, at
//!   [`crate::SimParams::wage_per_hour`]) and stipends for who has no job
//!   ([`crate::SimParams::stipend_per_day`]), both times the pay level and
//!   paid every midnight ([`World::payday`]). If the treasury cannot cover
//!   them everyone gets the same share of what is due (austerity).
//! - Into the treasury: purchases at the Mercati, fines for caught thieves,
//!   the estate of who dies without partner or children, and a tax on large
//!   savings ([`crate::SimParams::savings_tax_threshold`]), so that tokens
//!   nobody spends go back into circulation.
//! - Between NPCs: help asked in a deliberation, inheritance.
//!
//! The pay level is a slow feedback: when the treasury holds much more than
//! [`crate::SimParams::treasury_reserve_days`] days of payroll the
//! administration raises wages and stipends, when it holds much less it cuts
//! them (within bounds). Mercato prices and the savings tax threshold follow
//! the pay level, so it sets the value of a token rather than how well off
//! people are: with a fixed amount of money and a growing population, pay
//! and prices go down together.
//!
//! The farm workforce follows a similar feedback on the Serre ([`World::farm_quota`]).

use serde::{Deserialize, Serialize};

use super::{EXPECTED_WORK_MINUTES_PER_DAY, World};
use crate::carriage::CarriageKind;
use crate::event::EventKind;
use crate::ids::NpcId;
use crate::item::ItemKind;
use crate::npc::{Job, Npc};
use crate::params::SimParams;
use crate::time::{GameTime, MINUTES_PER_DAY};

/// Pay is accounted in thousandths of a token until it is paid.
const MILLI: u64 = 1000;
/// Smoothing of the daily payroll estimate (weight of the last day).
const PAYROLL_SMOOTHING: f32 = 0.1;
/// A pay level change is logged once it moved this much since the last log.
const PAY_LOG_STEP: f32 = 0.1;

/// A cumulative amount, kept exactly in thousandths (also when saved as text).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Tally(u64);

impl Tally {
    /// Adds `amount` (negative counts as 0), rounded to a thousandth.
    pub fn add(&mut self, amount: f32) {
        self.0 += (f64::from(amount.max(0.0)) * MILLI as f64).round() as u64;
    }

    pub fn get(self) -> f64 {
        self.0 as f64 / MILLI as f64
    }
}

/// Cumulative production and money counters since the world was generated.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EconomyCounters {
    /// Verdura grown into the Serre, and grown but lost because the Serra was full.
    pub verdura_grown: Tally,
    pub verdura_capped: Tally,
    /// Perishables lost to spoilage at midnight.
    pub verdura_spoiled: Tally,
    pub razioni_spoiled: Tally,
    pub razioni_cooked: Tally,
    /// Rottame shed but lost because the Officine were full.
    pub rottame_capped: Tally,
    /// Attrezzi and Vestiti crafted.
    pub attrezzi_crafted: Tally,
    pub vestiti_crafted: Tally,
    /// Minutes of work per job ([`Job::index`]), and the part of them that
    /// produced nothing (full storage, missing inputs).
    pub work_minutes: [u64; Job::COUNT],
    pub wasted_work_minutes: [Tally; Job::COUNT],
    /// Tokens paid from the treasury as wages and stipends...
    pub pay: u64,
    /// ...and due but not paid (austerity).
    pub pay_withheld: u64,
    /// Tokens into the treasury: NPC purchases, player purchases, fines,
    /// savings tax, estates without heirs.
    pub purchases: u64,
    pub player_purchases: u64,
    /// Tokens out of the treasury to the player, for what it sold.
    pub player_sales: u64,
    pub fines: u64,
    pub taxes: u64,
    pub estates: u64,
    /// Tokens passed between NPCs: help and inheritance.
    pub help: u64,
    pub inherited: u64,
    /// Midnights with austerity.
    pub austerity_days: u64,
    /// Units made by the NPC workers, per item ([`ItemKind::index`]).
    pub made: [Tally; ItemKind::COUNT],
}

impl EconomyCounters {
    /// Books `made` units of `item` out of a `potential` output.
    pub(crate) fn book_made(&mut self, item: ItemKind, made: f32, potential: f32) {
        self.made[item.index()].add(made);
        match item {
            ItemKind::Verdura => {
                self.verdura_grown.add(made);
                self.verdura_capped.add(potential - made);
            }
            ItemKind::Razione => self.razioni_cooked.add(made),
            ItemKind::Attrezzo => self.attrezzi_crafted.add(made),
            ItemKind::Vestito => self.vestiti_crafted.add(made),
            ItemKind::Rottame
            | ItemKind::Cotone
            | ItemKind::Erbe
            | ItemKind::Metallo
            | ItemKind::Tessuto
            | ItemKind::Te
            | ItemKind::Coperta
            | ItemKind::Lampada
            | ItemKind::Giocattolo => {}
        }
    }

    /// Units of `item` made by the NPC workers so far.
    pub fn made(&self, item: ItemKind) -> f64 {
        self.made[item.index()].get()
    }

    /// Share of `job`'s work that produced nothing, in `0..=1`.
    pub fn wasted_share(&self, job: Job) -> f64 {
        let total = self.work_minutes[job.index()];
        if total == 0 {
            0.0
        } else {
            self.wasted_work_minutes[job.index()].get() / total as f64
        }
    }
}

/// Pay earned by an NPC and not paid yet.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct Credit {
    npc: NpcId,
    /// Thousandths of a token.
    milli: u64,
}

/// The train's money: the administration's treasury and its pay policy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Economy {
    /// Tokens held by the train administration.
    pub treasury: u64,
    /// Wages and stipends are their base ([`SimParams::wage_per_hour`],
    /// [`SimParams::stipend_per_day`]) times this.
    pub pay_level: f32,
    /// Wages and stipends due per day at pay level 1 (smoothed): the
    /// treasury aims at [`SimParams::treasury_reserve_days`] of it.
    pub payroll_per_day: f32,
    /// Pay earned since the last payday, sorted by NPC id.
    credits: Vec<Credit>,
    last_austerity_log: Option<GameTime>,
    last_pay_log: Option<GameTime>,
    logged_pay_level: f32,
    pub counters: EconomyCounters,
}

impl Default for Economy {
    fn default() -> Self {
        Self {
            treasury: 0,
            pay_level: 1.0,
            payroll_per_day: 0.0,
            credits: Vec::new(),
            last_austerity_log: None,
            last_pay_log: None,
            logged_pay_level: 1.0,
            counters: EconomyCounters::default(),
        }
    }
}

impl Economy {
    /// A new treasury for `npcs`, holding [`SimParams::treasury_reserve_days`]
    /// of their expected payroll.
    pub(super) fn start(params: &SimParams, npcs: &[Npc]) -> Economy {
        let workers = npcs.iter().filter(|n| n.job.is_some()).count() as f32;
        let jobless = npcs.len() as f32 - workers;
        let payroll = workers * EXPECTED_WORK_MINUTES_PER_DAY / 60.0 * params.wage_per_hour as f32
            + jobless * params.stipend_per_day as f32;
        Economy {
            treasury: (payroll * params.treasury_reserve_days.max(0.0)).round() as u64,
            payroll_per_day: payroll,
            ..Economy::default()
        }
    }

    /// Treasury the administration aims at: some days of the current payroll.
    pub fn target_treasury(&self, params: &SimParams) -> f32 {
        self.payroll_per_day * self.pay_level * params.treasury_reserve_days.max(0.0)
    }

    /// Current wage, tokens per hour of work.
    pub fn wage_per_hour(&self, params: &SimParams) -> f32 {
        params.wage_per_hour as f32 * self.pay_level
    }

    /// Current stipend, tokens per day.
    pub fn stipend_per_day(&self, params: &SimParams) -> f32 {
        params.stipend_per_day as f32 * self.pay_level
    }

    /// Adds `milli` thousandths of a token to what `npc` will be paid.
    fn credit(&mut self, npc: NpcId, milli: u64) {
        match self.credits.binary_search_by_key(&npc, |c| c.npc) {
            Ok(k) => self.credits[k].milli += milli,
            Err(k) => self.credits.insert(k, Credit { npc, milli }),
        }
    }

    /// Pay earned and not paid yet by `npc`, in tokens (rounded down).
    pub fn pending_pay(&self, npc: NpcId) -> u64 {
        self.credits
            .binary_search_by_key(&npc, |c| c.npc)
            .map_or(0, |k| self.credits[k].milli / MILLI)
    }
}

impl World {
    /// All tokens in the sim: the treasury plus every NPC's. Constant except
    /// for player purchases ([`World::player_buy`]), which bring tokens in,
    /// and sales ([`World::player_sell`]), which take them out.
    pub fn money_supply(&self) -> u64 {
        self.economy.treasury
            + self
                .npcs
                .iter()
                .map(|n| u64::from(n.inventory.tokens))
                .sum::<u64>()
    }

    /// Books `minutes` of work by `job`, of which the share `wasted` (`0..=1`)
    /// produced nothing.
    pub(super) fn book_work(&mut self, job: Job, minutes: f32, wasted: f32) {
        let c = &mut self.economy.counters;
        c.work_minutes[job.index()] += minutes.max(0.0) as u64;
        c.wasted_work_minutes[job.index()].add(minutes * wasted.clamp(0.0, 1.0));
    }

    /// NPC `i` worked `minutes`: the wage is credited, paid at midnight.
    pub(super) fn earn_wage(&mut self, i: usize, minutes: u64) {
        let per_hour = self.economy.wage_per_hour(&self.params);
        let milli = (minutes as f32 * per_hour * MILLI as f32 / 60.0).round();
        if milli > 0.0 {
            let id = self.npcs[i].id;
            self.economy.credit(id, milli as u64);
        }
    }

    /// Tokens into the treasury (purchases, fines, taxes, estates).
    pub(super) fn deposit(&mut self, tokens: u32) {
        self.economy.treasury += u64::from(tokens);
    }

    /// Midnight: stipends for who has no job, then everyone is paid what they
    /// earned (all of it, or the same share for all if the treasury cannot
    /// cover it), large savings are taxed and the pay level is adjusted.
    pub(super) fn payday(&mut self) {
        let stipend = self.economy.stipend_per_day(&self.params);
        let stipend = (stipend * MILLI as f32).round().max(0.0) as u64;
        if stipend > 0 {
            let jobless: Vec<NpcId> = self
                .npcs
                .iter()
                .filter(|n| n.job.is_none())
                .map(|n| n.id)
                .collect();
            for id in jobless {
                self.economy.credit(id, stipend);
            }
        }

        // Who died since the last payday loses what was due.
        let mut credits = std::mem::take(&mut self.economy.credits);
        credits.retain(|c| self.npc_index(c.npc).is_some());
        let due: u64 = credits.iter().map(|c| c.milli / MILLI).sum();
        let treasury = self.economy.treasury;
        let mut paid = 0;
        for c in &mut credits {
            let owed = c.milli / MILLI;
            // Pro rata when the treasury falls short (u128: no overflow).
            let pay = if due <= treasury {
                owed
            } else {
                (u128::from(owed) * u128::from(treasury) / u128::from(due)) as u64
            };
            c.milli %= MILLI;
            if pay > 0
                && let Some(i) = self.npc_index(c.npc)
            {
                let tokens = &mut self.npcs[i].inventory.tokens;
                let pay = pay.min(u64::from(u32::MAX - *tokens)) as u32;
                *tokens += pay;
                paid += u64::from(pay);
            }
        }
        credits.retain(|c| c.milli > 0);
        self.economy.credits = credits;
        self.economy.treasury -= paid;
        let counters = &mut self.economy.counters;
        counters.pay += paid;
        counters.pay_withheld += due - paid;
        if paid < due {
            counters.austerity_days += 1;
            let paid_percent = (100 * paid / due.max(1)) as u32;
            if self.may_log_economy(self.economy.last_austerity_log) {
                self.economy.last_austerity_log = Some(self.clock);
                self.push_event(EventKind::Austerity { paid_percent });
            }
        }

        // Base payroll estimate (at pay level 1).
        let level = self.economy.pay_level.max(0.01);
        let e = &mut self.economy;
        e.payroll_per_day += PAYROLL_SMOOTHING * (due as f32 / level - e.payroll_per_day);

        self.tax_savings();
        self.adjust_pay();
    }

    /// Every NPC above [`SimParams::savings_tax_threshold`] (times the pay
    /// level) pays [`SimParams::savings_tax_rate`] of the excess.
    fn tax_savings(&mut self) {
        let threshold = self.params.savings_tax_threshold as f32 * self.economy.pay_level;
        let threshold = threshold.round() as u32;
        let rate = self.params.savings_tax_rate.clamp(0.0, 1.0);
        let mut taxes = 0u64;
        for npc in &mut self.npcs {
            let excess = npc.inventory.tokens.saturating_sub(threshold);
            let tax = (excess as f32 * rate).floor() as u32;
            npc.inventory.tokens -= tax.min(excess);
            taxes += u64::from(tax.min(excess));
        }
        self.economy.treasury += taxes;
        self.economy.counters.taxes += taxes;
    }

    /// Nudges the pay level towards keeping the treasury near its target.
    fn adjust_pay(&mut self) {
        let p = &self.params;
        let e = &mut self.economy;
        let target = e.target_treasury(p);
        if target <= 0.0 {
            return;
        }
        let ratio = e.treasury as f32 / target;
        let band = p.pay_band.max(0.0);
        let step = p.pay_adjust_per_day.max(0.0);
        if ratio > 1.0 + band {
            e.pay_level += step;
        } else if ratio < 1.0 - band {
            e.pay_level -= step;
        }
        let (lo, hi) = (p.pay_level_min.min(p.pay_level_max), p.pay_level_max);
        e.pay_level = e.pay_level.clamp(lo, hi);
        let level = e.pay_level;
        if (level - e.logged_pay_level).abs() < PAY_LOG_STEP - 1e-4 {
            return;
        }
        if self.may_log_economy(self.economy.last_pay_log) {
            let e = &mut self.economy;
            let raised = level > e.logged_pay_level;
            e.logged_pay_level = level;
            e.last_pay_log = Some(self.clock);
            self.push_event(EventKind::PayChanged {
                level_percent: (level * 100.0).round() as u32,
                raised,
            });
        }
    }

    /// Whether an economy event may be logged, the last one being at `last`.
    fn may_log_economy(&self, last: Option<GameTime>) -> bool {
        let every = self.params.economy_log_days.max(1) * MINUTES_PER_DAY;
        last.is_none_or(|t| self.clock.since(t) >= every)
    }

    /// Contadini wanted, given `without_tools` (enough to feed everyone with
    /// a margin if nobody had a tool): fewer for the tools they own, and
    /// more or fewer as the Serre hold less or more than
    /// [`SimParams::verdura_target_fill`] of their storage.
    pub(super) fn farm_quota(&self, without_tools: usize) -> usize {
        if without_tools == 0 {
            return 0;
        }
        let p = &self.params;
        let (mut farmers, mut with_tool) = (0usize, 0usize);
        for npc in self.npcs.iter().filter(|n| n.job == Some(Job::Contadino)) {
            farmers += 1;
            with_tool += usize::from(npc.inventory.tool.is_some());
        }
        let tools = if farmers > 0 {
            with_tool as f32 / farmers as f32
        } else {
            0.0
        };
        let boost = 1.0 + (p.tool_output_bonus - 1.0).max(0.0) * tools;
        let (mut stock, mut cap) = (0.0, 0.0);
        for c in self
            .carriages
            .iter()
            .filter(|c| c.kind == CarriageKind::Serra)
        {
            stock += c.stock.get(ItemKind::Verdura);
            cap += p.storage_cap(c.kind, ItemKind::Verdura);
        }
        let correction = if cap > 0.0 {
            (1.0 + p.farm_staffing_gain * (p.verdura_target_fill - stock / cap)).clamp(0.6, 1.5)
        } else {
            1.0
        };
        ((without_tools as f32 * correction / boost).ceil() as usize).max(1)
    }
}
