//! Statistiche aggregate sul mondo.

use std::fmt;

use crate::action::ActionKind;
use crate::carriage::CarriageKind;
use crate::deliberation::DeliberationCounters;
use crate::dialogue::{ConversationCounters, Tone};
use crate::item::{ItemKind, Stock};
use crate::npc::{LifeStage, Needs};
use crate::time::GameTime;
use crate::world::World;

#[derive(Clone, Debug, PartialEq)]
pub struct Stats {
    pub time: GameTime,
    pub population: usize,
    /// Average needs (all 0 when the population is 0).
    pub avg_needs: Needs,
    /// Everything stored in carriages, anywhere on the train.
    pub stored: Stock,
    /// Stored in the Mercati (on sale).
    pub on_sale: Stock,
    /// Owned units in personal inventories, indexed by [`ItemKind::index`]
    /// (only Attrezzo and Vestito can be owned).
    pub owned: [u32; ItemKind::COUNT],
    /// Tokens held by all NPCs.
    pub tokens: u64,
    /// All tokens in the sim: the treasury plus the NPCs' ([`World::money_supply`]).
    pub money_supply: u64,
    /// Tokens held by the train administration ([`crate::Economy::treasury`]).
    pub treasury: u64,
    /// Tokens per adult (18-64): mean and median (0 without adults).
    pub tokens_per_adult: f32,
    pub median_tokens_per_adult: u32,
    /// Gini index of the NPCs' tokens: 0 all equal, towards 1 one holds all.
    pub tokens_gini: f32,
    /// Pay level ([`crate::Economy::pay_level`]) and the wage it gives, in
    /// tokens per hour of work.
    pub pay_level: f32,
    pub wage_per_hour: f32,
    /// NPC count per action, indexed like [`ActionKind::ALL`].
    pub actions: [usize; ActionKind::ALL.len()],
    /// Population per life stage, indexed by [`LifeStage::index`].
    pub stages: [usize; LifeStage::ALL.len()],
    /// Couples (both partners alive).
    pub couples: usize,
    /// Average age in years (0 when the population is 0).
    pub avg_age: f32,
    /// Founders still alive (see [`World::is_founder`]); the rest were born on the train.
    pub founders: usize,
    /// Beds on the train and the population above which births are denied.
    pub beds: usize,
    pub max_population: usize,
    /// Since the last midnight / since the world was generated.
    pub births_today: u32,
    pub deaths_today: u32,
    pub births_total: u64,
    pub deaths_total: u64,
    /// Deliberations waiting for a decision (see [`World::open_deliberations`]).
    pub deliberations_open: usize,
    /// Deliberations since the world was generated: opened and resolved per
    /// kind, by rules or brain, per choice, thefts, help, protests.
    pub deliberations: DeliberationCounters,
    /// Conversations in progress (see [`World::conversations`]).
    pub conversations_open: usize,
    /// Chats and conversations since the world was generated: two-sided
    /// share, topics, tones.
    pub conversations: ConversationCounters,
}

impl Stats {
    pub fn of(world: &World) -> Stats {
        let population = world.npcs.len();
        let mut sum = Needs {
            hunger: 0.0,
            energy: 0.0,
            social: 0.0,
        };
        let mut actions = [0; ActionKind::ALL.len()];
        let mut owned = [0; ItemKind::COUNT];
        // Only a few kinds can be owned: look those up once.
        let ownable: Vec<ItemKind> = ItemKind::ALL
            .into_iter()
            .filter(|i| i.has_durability())
            .collect();
        let mut tokens = 0;
        let mut stages = [0; LifeStage::ALL.len()];
        let mut partnered = 0;
        let mut ages = 0u64;
        let mut founders = 0;
        let mut all_tokens = Vec::with_capacity(population);
        let mut adult_tokens = Vec::new();
        for npc in &world.npcs {
            all_tokens.push(npc.inventory.tokens);
            if npc.stage() == LifeStage::Adulto {
                adult_tokens.push(npc.inventory.tokens);
            }
            stages[npc.stage().index()] += 1;
            partnered += usize::from(npc.partner().is_some());
            ages += u64::from(npc.age);
            founders += usize::from(world.is_founder(npc.id));
            sum.hunger += npc.needs.hunger;
            sum.energy += npc.needs.energy;
            sum.social += npc.needs.social;
            actions[npc.action.kind() as usize] += 1;
            tokens += u64::from(npc.inventory.tokens);
            for &item in &ownable {
                if npc.inventory.has(item) {
                    owned[item.index()] += 1;
                }
            }
        }
        let n = population.max(1) as f32;
        adult_tokens.sort_unstable();
        let adult_sum: u64 = adult_tokens.iter().map(|&t| u64::from(t)).sum();
        Stats {
            time: world.clock,
            population,
            avg_needs: Needs {
                hunger: sum.hunger / n,
                energy: sum.energy / n,
                social: sum.social / n,
            },
            stored: world.total_stock(),
            on_sale: world.stock_in(CarriageKind::Mercato),
            owned,
            tokens,
            money_supply: world.money_supply(),
            treasury: world.economy.treasury,
            tokens_per_adult: adult_sum as f32 / adult_tokens.len().max(1) as f32,
            median_tokens_per_adult: adult_tokens
                .get(adult_tokens.len() / 2)
                .copied()
                .unwrap_or(0),
            tokens_gini: gini(&mut all_tokens),
            pay_level: world.economy.pay_level,
            wage_per_hour: world.economy.wage_per_hour(&world.params),
            actions,
            stages,
            couples: partnered / 2,
            avg_age: ages as f32 / n,
            founders,
            beds: world.total_beds(),
            max_population: world.max_population(),
            births_today: world.life.births_today,
            deaths_today: world.life.deaths_today,
            births_total: world.life.births_total,
            deaths_total: world.life.deaths_total,
            deliberations_open: world.open_deliberations().len(),
            deliberations: world.deliberation_counters.clone(),
            conversations_open: world.conversations().len(),
            conversations: world.conversation_counters.clone(),
        }
    }

    pub fn count(&self, kind: ActionKind) -> usize {
        self.actions[kind as usize]
    }

    /// Population at a life stage.
    pub fn stage(&self, stage: LifeStage) -> usize {
        self.stages[stage.index()]
    }

    /// Owned units of `item` in personal inventories.
    pub fn owned(&self, item: ItemKind) -> u32 {
        self.owned[item.index()]
    }
}

impl fmt::Display for Stats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = &self.stored;
        write!(
            f,
            "{} | pop {:3} | sazietà {:.2} energia {:.2} social {:.2} | verdura {:4.0} razioni {:4.0} rottame {:3.0} | attrezzi {:3.0} (in vendita {:3.0}, posseduti {:3}) vestiti {:3.0} (in vendita {:3.0}, posseduti {:3}) | gettoni {:5} |",
            self.time,
            self.population,
            self.avg_needs.hunger,
            self.avg_needs.energy,
            self.avg_needs.social,
            s.get(ItemKind::Verdura),
            s.get(ItemKind::Razione),
            s.get(ItemKind::Rottame),
            s.get(ItemKind::Attrezzo),
            self.on_sale.get(ItemKind::Attrezzo),
            self.owned(ItemKind::Attrezzo),
            s.get(ItemKind::Vestito),
            self.on_sale.get(ItemKind::Vestito),
            self.owned(ItemKind::Vestito),
            self.tokens,
        )?;
        write!(
            f,
            " tesoro {} (moneta {}) | gettoni per adulto {:.0} (mediana {}) gini {:.2} | paga {:.2}/h ({:.0}%) |",
            self.treasury,
            self.money_supply,
            self.tokens_per_adult,
            self.median_tokens_per_adult,
            self.tokens_gini,
            self.wage_per_hour,
            self.pay_level * 100.0,
        )?;
        write!(
            f,
            " bambini {} giovani {} adulti {} anziani {} coppie {} età media {:.1} | nati {} morti {} |",
            self.stage(LifeStage::Bambino),
            self.stage(LifeStage::Giovane),
            self.stage(LifeStage::Adulto),
            self.stage(LifeStage::Anziano),
            self.couples,
            self.avg_age,
            self.births_total,
            self.deaths_total,
        )?;
        let c = &self.conversations;
        write!(
            f,
            " conversazioni {} in corso, {} in tutto ({:.0}% a due, {:.0}% tese) |",
            self.conversations_open,
            c.conversations,
            100.0 * c.two_sided_share(),
            100.0 * c.by_tone[Tone::Tense.index()] as f32 / c.conversations.max(1) as f32,
        )?;
        for kind in ActionKind::ALL {
            write!(f, " {} {}", kind.name(), self.count(kind))?;
        }
        Ok(())
    }
}

/// Gini index of `values` (sorted in place): 0 when all are equal (or all 0).
fn gini(values: &mut [u32]) -> f32 {
    values.sort_unstable();
    let n = values.len() as f64;
    let sum: f64 = values.iter().map(|&v| f64::from(v)).sum();
    if sum <= 0.0 {
        return 0.0;
    }
    let weighted: f64 = values
        .iter()
        .enumerate()
        .map(|(i, &v)| (i as f64 + 1.0) * f64::from(v))
        .sum();
    (2.0 * weighted / (n * sum) - (n + 1.0) / n) as f32
}

#[cfg(test)]
mod tests {
    use super::gini;

    #[test]
    fn gini_of_equal_and_concentrated_tokens() {
        assert_eq!(gini(&mut []), 0.0);
        assert_eq!(gini(&mut [0, 0, 0]), 0.0);
        assert!(gini(&mut [5, 5, 5, 5]).abs() < 1e-6);
        // One of four holds everything: (n - 1) / n.
        assert!((gini(&mut [0, 0, 12, 0]) - 0.75).abs() < 1e-6);
    }
}
