//! A compact snapshot of the train: the context of the Narratore's prompt.
//!
//! Built only from `sim`'s public API, read-only. It is small on purpose
//! (well under 1.5k tokens as JSON): totals and shares instead of lists of
//! people, the last few notable events, the catalog by name. The JSON keys
//! are Italian because the prompt is.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sim::{
    CarriageKind, DeathCause, EventKind, ItemCategory, ItemKind, Job, LifeStage, MINUTES_PER_DAY,
    RECIPES, Stats, Trend, World,
};

/// Fill (stock / storage capacity on the whole train) under which an item is
/// listed among the shortages.
pub const SHORTAGE_FILL: f32 = 0.2;
/// Most recent notable events listed.
pub const MAX_EVENTS: usize = 8;
/// Longest event line kept (characters).
pub const EVENT_CHARS: usize = 110;
/// Satiety under which someone counts as hungry.
pub const HUNGRY_BELOW: f32 = 0.3;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldSummary {
    #[serde(rename = "giorno")]
    pub day: u64,
    /// "hh:mm".
    #[serde(rename = "ora")]
    pub time: String,
    #[serde(rename = "popolazione")]
    pub population: Population,
    /// Train-wide stock per item, in catalog order.
    #[serde(rename = "scorte")]
    pub stock: Vec<StockLine>,
    /// Items (not intermediates) whose fill is under [`SHORTAGE_FILL`].
    #[serde(rename = "carenze")]
    pub shortages: Vec<String>,
    /// Mercato prices (mean over the Mercati) of what they sell.
    #[serde(rename = "mercato")]
    pub market: Vec<PriceLine>,
    #[serde(rename = "fame")]
    pub hunger: Hunger,
    #[serde(rename = "economia")]
    pub economy: EconomyLine,
    /// Notable events of the last day, by kind.
    #[serde(rename = "eventi_ultimo_giorno")]
    pub event_counts: BTreeMap<String, u32>,
    /// The last [`MAX_EVENTS`] notable events, oldest first.
    #[serde(rename = "eventi_recenti")]
    pub events: Vec<String>,
    /// Carriages per kind.
    #[serde(rename = "carrozze")]
    pub carriages: BTreeMap<String, u32>,
    #[serde(rename = "catalogo")]
    pub catalog: Catalog,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Population {
    #[serde(rename = "totale")]
    pub total: u32,
    #[serde(rename = "per_eta")]
    pub by_stage: BTreeMap<String, u32>,
    /// Adults per job ("nessuno": adults without a job).
    #[serde(rename = "per_lavoro")]
    pub by_job: BTreeMap<String, u32>,
    #[serde(rename = "coppie")]
    pub couples: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StockLine {
    #[serde(rename = "nome")]
    pub name: String,
    /// Whole units on the train.
    #[serde(rename = "qta")]
    pub qty: u32,
    /// Percent of the train's storage capacity for the item.
    #[serde(rename = "pieno_pct")]
    pub fill_pct: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PriceLine {
    #[serde(rename = "nome")]
    pub name: String,
    /// Tokens, mean over the Mercati.
    #[serde(rename = "prezzo")]
    pub price: u32,
    /// "sale", "stabile" or "scende".
    #[serde(rename = "tendenza")]
    pub trend: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hunger {
    /// Mean satiety, percent (100 = everyone full).
    #[serde(rename = "sazieta_media_pct")]
    pub satiety_pct: u32,
    /// People under [`HUNGRY_BELOW`] satiety.
    #[serde(rename = "affamati")]
    pub hungry: u32,
    /// People with empty stomachs, dying of hunger.
    #[serde(rename = "alla_fame")]
    pub starving: u32,
    #[serde(rename = "morti_oggi")]
    pub deaths_today: u32,
    #[serde(rename = "morti_di_fame_totali")]
    pub starved_total: u64,
    #[serde(rename = "nati_oggi")]
    pub births_today: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EconomyLine {
    /// Tokens in the administration's treasury.
    #[serde(rename = "tesoro")]
    pub treasury: u64,
    #[serde(rename = "gettoni_mediani_adulto")]
    pub median_tokens: u32,
    /// Inequality of the tokens, 0–100 (Gini × 100).
    #[serde(rename = "disuguaglianza_pct")]
    pub inequality_pct: u32,
}

/// What exists already, by name. The guard checks references and duplicates
/// against it; the Custode (A2) will resolve these names to stable keys.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    #[serde(rename = "oggetti")]
    pub items: Vec<String>,
    /// Recipe name → "inputs → output (job)".
    #[serde(rename = "ricette")]
    pub recipes: BTreeMap<String, String>,
    /// Job → kind of carriage where it works.
    #[serde(rename = "lavori")]
    pub jobs: BTreeMap<String, String>,
    #[serde(rename = "tipi_di_carrozza")]
    pub carriage_kinds: Vec<String>,
    /// Other spellings of item names (plural → name), for the guard only.
    #[serde(skip)]
    pub item_aliases: Vec<(String, String)>,
    /// Recipe name, output and input items, for the guard only (a new
    /// recipe that makes the same output from the same inputs is a copy).
    #[serde(skip)]
    pub recipe_shapes: Vec<(String, String, Vec<String>)>,
}

impl Catalog {
    /// The catalog of `sim` as it is today (static tables; A2 makes them
    /// grow during the game).
    pub fn of_sim() -> Catalog {
        let items = ItemKind::ALL.iter().map(|i| i.name().to_string()).collect();
        let item_aliases = ItemKind::ALL
            .iter()
            .filter(|i| i.plural() != i.name())
            .map(|i| (i.plural().to_string(), i.name().to_string()))
            .collect();
        let recipes = RECIPES
            .iter()
            .map(|r| {
                let inputs: Vec<&str> = r.inputs.iter().map(|i| i.item.name()).collect();
                let from = if inputs.is_empty() {
                    "dal nulla".to_string()
                } else {
                    inputs.join(" + ")
                };
                let job = r.maker().map_or("giocatore", Job::name);
                (
                    r.name.to_string(),
                    format!("{from} → {} ({job})", r.output.name()),
                )
            })
            .collect();
        let recipe_shapes = RECIPES
            .iter()
            .map(|r| {
                let inputs = r.inputs.iter().map(|i| i.item.name().to_string()).collect();
                (r.name.to_string(), r.output.name().to_string(), inputs)
            })
            .collect();
        let jobs = Job::ALL
            .iter()
            .map(|j| (j.name().to_string(), j.workplace_kind().name().to_string()))
            .collect();
        let carriage_kinds = CarriageKind::ALL
            .iter()
            .map(|k| k.name().to_string())
            .collect();
        Catalog {
            items,
            recipes,
            jobs,
            carriage_kinds,
            item_aliases,
            recipe_shapes,
        }
    }
}

impl WorldSummary {
    /// The summary of `world` now. Deterministic: the same world gives the
    /// same summary (and the same JSON).
    pub fn from_world(world: &World) -> WorldSummary {
        let stats = Stats::of(world);
        let clock = world.clock;

        // Population.
        let stage_name = |s: LifeStage| match s {
            LifeStage::Bambino => "bambini",
            LifeStage::Giovane => "giovani",
            LifeStage::Adulto => "adulti",
            LifeStage::Anziano => "anziani",
        };
        let by_stage = LifeStage::ALL
            .iter()
            .map(|&s| (stage_name(s).to_string(), stats.stage(s) as u32))
            .collect();
        let mut by_job: BTreeMap<String, u32> =
            Job::ALL.iter().map(|j| (j.name().to_string(), 0)).collect();
        for npc in world.npcs.iter().filter(|n| n.stage() == LifeStage::Adulto) {
            let key = npc.job.map_or("nessuno", Job::name);
            *by_job.entry(key.to_string()).or_default() += 1;
        }

        // Stock and shortages.
        let total = world.total_stock();
        let mut stock = Vec::with_capacity(ItemKind::COUNT);
        let mut shortages = Vec::new();
        for item in ItemKind::ALL {
            let cap: f32 = world
                .carriages
                .iter()
                .map(|c| world.params.storage_cap(c.kind, item).max(0.0))
                .sum();
            let have = total.get(item).max(0.0);
            let fill = if cap > 0.0 {
                (have / cap).min(1.0)
            } else {
                0.0
            };
            // Intermediates (metallo, tessuto…) are used as soon as they are
            // made: an empty store is normal, not a shortage.
            let intermediate = item.def().category == ItemCategory::Intermediate;
            if cap > 0.0 && fill < SHORTAGE_FILL && !intermediate {
                shortages.push(item.name().to_string());
            }
            stock.push(StockLine {
                name: item.name().to_string(),
                qty: have.floor() as u32,
                fill_pct: (fill * 100.0).round() as u32,
            });
        }

        // Mercato: mean price over the Mercati, trend by majority.
        let mut prices: BTreeMap<usize, (u64, u32, i32)> = BTreeMap::new();
        for m in world.markets() {
            for q in world.market_quotes(m) {
                let e = prices.entry(q.item.index()).or_default();
                e.0 += u64::from(q.price);
                e.1 += 1;
                e.2 += match q.trend {
                    Trend::Up => 1,
                    Trend::Flat => 0,
                    Trend::Down => -1,
                };
            }
        }
        let market = prices
            .into_iter()
            .map(|(i, (sum, n, trend))| PriceLine {
                name: ItemKind::ALL[i].name().to_string(),
                price: (sum as f64 / f64::from(n.max(1))).round() as u32,
                trend: match trend.signum() {
                    1 => "sale",
                    -1 => "scende",
                    _ => "stabile",
                }
                .to_string(),
            })
            .collect();

        // Hunger.
        let hungry = world
            .npcs
            .iter()
            .filter(|n| n.needs.hunger < HUNGRY_BELOW)
            .count() as u32;
        let starving = world
            .npcs
            .iter()
            .filter(|n| n.needs.hunger <= 0.0 || n.starving_minutes > 0)
            .count() as u32;
        let hunger = Hunger {
            satiety_pct: (stats.avg_needs.hunger.clamp(0.0, 1.0) * 100.0).round() as u32,
            hungry,
            starving,
            deaths_today: world.life.deaths_today,
            starved_total: world.life.deaths_by_cause[DeathCause::Starvation.index()],
            births_today: world.life.births_today,
        };
        let economy = EconomyLine {
            treasury: stats.treasury,
            median_tokens: stats.median_tokens_per_adult,
            inequality_pct: (stats.tokens_gini.clamp(0.0, 1.0) * 100.0).round() as u32,
        };

        // Notable events: counts over the last day, the last few in full.
        let since = clock.minutes().saturating_sub(MINUTES_PER_DAY);
        let mut event_counts = BTreeMap::new();
        let mut events = Vec::new();
        for e in world.events.iter().filter(|e| e.time.minutes() >= since) {
            let Some(kind) = notable(&e.kind) else {
                continue;
            };
            *event_counts.entry(kind.to_string()).or_insert(0) += 1;
            let line = e.to_string();
            let line: String = line.chars().take(EVENT_CHARS).collect();
            events.push(line);
        }
        let events = events.split_off(events.len().saturating_sub(MAX_EVENTS));

        let mut carriages: BTreeMap<String, u32> = BTreeMap::new();
        for c in &world.carriages {
            *carriages.entry(c.kind.name().to_string()).or_default() += 1;
        }

        WorldSummary {
            day: clock.day(),
            time: format!("{:02}:{:02}", clock.hour(), clock.minute()),
            population: Population {
                total: stats.population as u32,
                by_stage,
                by_job,
                couples: stats.couples as u32,
            },
            stock,
            shortages,
            market,
            hunger,
            economy,
            event_counts,
            events,
            carriages,
            catalog: Catalog::of_sim(),
        }
    }

    /// Compact JSON (the form put in the prompt).
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a summary serializes")
    }

    /// Rough token count of [`Self::to_json`] (≈ 3 characters per token for
    /// Italian JSON; errs on the high side).
    pub fn approx_tokens(&self) -> usize {
        self.to_json().chars().count().div_ceil(3)
    }
}

/// The Italian name of an event kind worth telling the Narratore about.
fn notable(kind: &EventKind) -> Option<&'static str> {
    Some(match kind {
        EventKind::Born { .. } => "nascite",
        EventKind::NpcDied {
            cause: DeathCause::Starvation,
            ..
        } => "morti di fame",
        EventKind::NpcDied { .. } => "morti",
        EventKind::NpcStarving { .. } => "alla fame",
        EventKind::Coupled { .. } => "coppie",
        EventKind::Widowed { .. } => "vedovanze",
        EventKind::BirthDenied { .. } => "nascite negate",
        EventKind::Shortage { .. } => "carenze",
        EventKind::Theft { .. } => "furti",
        EventKind::ProtestCalled { .. } => "proteste",
        EventKind::AdminConceded { .. } => "concessioni",
        EventKind::Austerity { .. } => "austerità",
        EventKind::PayChanged { .. } => "paghe cambiate",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use sim::UtilityBrain;

    use super::*;

    fn world_after(seed: u64, carriages: usize, npcs: usize, minutes: u64) -> World {
        let mut world = World::generate(seed, carriages, npcs);
        let mut brain = UtilityBrain::new(seed);
        world.run(&mut brain, minutes);
        world
    }

    #[test]
    fn deterministic() {
        let a = WorldSummary::from_world(&world_after(7, 8, 60, 20 * 60));
        let b = WorldSummary::from_world(&world_after(7, 8, 60, 20 * 60));
        assert_eq!(a, b);
        assert_eq!(a.to_json(), b.to_json());
        // Round trip through JSON (the aliases are not serialized).
        let back: WorldSummary = serde_json::from_str(&a.to_json()).unwrap();
        assert_eq!(back.to_json(), a.to_json());
        assert!(a.population.total > 0);
        assert_eq!(a.stock.len(), ItemKind::COUNT);
    }

    #[test]
    fn size_is_bounded() {
        for world in [
            world_after(3, 8, 60, 20 * 60),
            // A big train: the summary doesn't grow with people or carriages.
            World::generate(5, 40, 600),
        ] {
            let s = WorldSummary::from_world(&world);
            assert!(s.events.len() <= MAX_EVENTS);
            let tokens = s.approx_tokens();
            assert!(tokens < 1200, "{tokens} tokens: {}", s.to_json());
        }
    }

    #[test]
    fn catalog_lists_the_sim() {
        let c = Catalog::of_sim();
        assert!(c.items.iter().any(|i| i == "verdura"));
        assert!(c.jobs.contains_key("contadino"));
        assert!(c.carriage_kinds.iter().any(|k| k == "Mercato"));
        assert_eq!(c.recipes.len(), RECIPES.len());
        assert!(
            c.item_aliases
                .iter()
                .any(|(p, n)| p == "razioni" && n == "razione")
        );
    }
}
