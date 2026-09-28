//! Statistiche aggregate sul mondo.

use std::fmt;

use crate::action::ActionKind;
use crate::carriage::CarriageKind;
use crate::item::{ItemKind, Stock};
use crate::npc::Needs;
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
    /// NPC count per action, indexed like [`ActionKind::ALL`].
    pub actions: [usize; ActionKind::ALL.len()],
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
        let mut tokens = 0;
        for npc in &world.npcs {
            sum.hunger += npc.needs.hunger;
            sum.energy += npc.needs.energy;
            sum.social += npc.needs.social;
            actions[npc.action.kind() as usize] += 1;
            tokens += u64::from(npc.inventory.tokens);
            for item in ItemKind::ALL {
                if npc.inventory.has(item) {
                    owned[item.index()] += 1;
                }
            }
        }
        let n = population.max(1) as f32;
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
            actions,
        }
    }

    pub fn count(&self, kind: ActionKind) -> usize {
        self.actions[kind as usize]
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
        for kind in ActionKind::ALL {
            write!(f, " {} {}", kind.name(), self.count(kind))?;
        }
        Ok(())
    }
}
