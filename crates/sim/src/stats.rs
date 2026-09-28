//! Statistiche aggregate sul mondo.

use std::fmt;

use crate::action::ActionKind;
use crate::npc::Needs;
use crate::time::GameTime;
use crate::world::World;

#[derive(Clone, Debug, PartialEq)]
pub struct Stats {
    pub time: GameTime,
    pub population: usize,
    /// Average needs (all 0 when the population is 0).
    pub avg_needs: Needs,
    /// Food stored anywhere on the train.
    pub food_total: f32,
    /// Food stored in the Mense (ready to eat).
    pub food_in_mense: f32,
    pub materials: f32,
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
        for npc in &world.npcs {
            sum.hunger += npc.needs.hunger;
            sum.energy += npc.needs.energy;
            sum.social += npc.needs.social;
            actions[npc.action.kind() as usize] += 1;
        }
        let n = population.max(1) as f32;
        let stock = world.total_stock();
        Stats {
            time: world.clock,
            population,
            avg_needs: Needs {
                hunger: sum.hunger / n,
                energy: sum.energy / n,
                social: sum.social / n,
            },
            food_total: stock.food,
            food_in_mense: world.food_in_mense(),
            materials: stock.materials,
            actions,
        }
    }

    pub fn count(&self, kind: ActionKind) -> usize {
        self.actions[kind as usize]
    }
}

impl fmt::Display for Stats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} | pop {:3} | sazietà {:.2} energia {:.2} social {:.2} | cibo {:6.0} (mense {:5.0}) mat {:5.0} |",
            self.time,
            self.population,
            self.avg_needs.hunger,
            self.avg_needs.energy,
            self.avg_needs.social,
            self.food_total,
            self.food_in_mense,
            self.materials,
        )?;
        for kind in ActionKind::ALL {
            write!(f, " {} {}", kind.name(), self.count(kind))?;
        }
        Ok(())
    }
}
