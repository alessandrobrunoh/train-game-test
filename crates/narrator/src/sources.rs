//! What a panel or a statistic may read from the train: a closed whitelist of
//! numbers ([`Source`]) and lists ([`ListSource`]), evaluated **read-only**
//! on `&World`.
//!
//! **Draft for the Custode.** Everything refers to items, jobs, recipes and
//! kinds of carriage by name, resolved here against the sim's catalog
//! ([`item`], [`job`], [`carriage_kind`], [`recipe`]). A name the sim does not
//! know yet (a proposal the Custode has not applied) reads as
//! [`Reading::Pending`]: the game shows "—" with "in attesa del Custode".
//! Once A2 makes the catalogs grow at run time these resolvers read the live
//! catalogs instead of the static tables.
//!
//! The JSON is compact, one key naming the source and its argument:
//!
//! ```json
//! {"scorta": "verdura", "carrozza": "Serra"}
//! {"bisogno": "sazieta"}
//! {"giocatore": "gettoni"}
//! ```
//!
//! Parsing never fails on an unknown source: it becomes
//! [`Source::Invalid`] (or [`ListSource::Invalid`]) with an Italian reason
//! that [`crate::guard::precheck`] sends back to the model.

use std::fmt;

use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sim::{CarriageKind, ItemKind, Job, LifeStage, RECIPES, RecipeDef, World};

use crate::guard::normalize;
use crate::proposal::Need;
use crate::statistic::StatBook;

// --- Name resolution ----------------------------------------------------------------

/// The sim's item called `name` (singular or plural, case and accents
/// ignored); `None` for an item that doesn't exist yet.
pub fn item(name: &str) -> Option<ItemKind> {
    let key = normalize(name);
    ItemKind::ALL
        .into_iter()
        .find(|i| normalize(i.name()) == key || normalize(i.plural()) == key)
}

/// The sim's job called `name`.
pub fn job(name: &str) -> Option<Job> {
    let key = normalize(name);
    Job::ALL.into_iter().find(|j| normalize(j.name()) == key)
}

/// Italian plural of a carriage kind: "Dormitori", "Mense", "Mercati".
fn kind_plural(name: &str) -> String {
    let key = normalize(name);
    if let Some(stem) = key.strip_suffix("io") {
        format!("{stem}i")
    } else if let Some(stem) = key.strip_suffix('o') {
        format!("{stem}i")
    } else if let Some(stem) = key.strip_suffix('a') {
        format!("{stem}e")
    } else {
        key
    }
}

/// The kind of carriage called `name` (singular or plural).
pub fn carriage_kind(name: &str) -> Option<CarriageKind> {
    let key = normalize(name);
    CarriageKind::ALL
        .into_iter()
        .find(|k| normalize(k.name()) == key || kind_plural(k.name()) == key)
}

/// The sim's recipe called `name`, or failing that the first recipe that
/// makes the item called `name`.
pub fn recipe(name: &str) -> Option<&'static RecipeDef> {
    let key = normalize(name);
    RECIPES
        .iter()
        .find(|r| normalize(r.name) == key || normalize(r.key) == key)
        .or_else(|| item(name).and_then(|i| RECIPES.iter().find(|r| r.output == i)))
}

// --- Readings ---------------------------------------------------------------------------

/// Why a value is shown as "—" while an item or job is not real yet.
pub const PENDING: &str = "in attesa del Custode";

/// The value of a source now.
#[derive(Clone, Debug, PartialEq)]
pub enum Reading {
    Value(f32),
    /// Refers to a name the sim doesn't have yet (a proposal not applied):
    /// what is missing, e.g. "l'oggetto «Scialle»".
    Pending(String),
    /// Readable, but there is nothing to read now (e.g. no Mercato sells it).
    Missing(String),
}

impl Reading {
    pub fn value(&self) -> Option<f32> {
        match self {
            Reading::Value(v) => Some(*v),
            _ => None,
        }
    }

    /// The tooltip of a "—".
    pub fn why(&self) -> Option<String> {
        match self {
            Reading::Value(_) => None,
            Reading::Pending(what) => Some(format!("{what}: {PENDING}")),
            Reading::Missing(why) => Some(why.clone()),
        }
    }
}

fn pending_item(name: &str) -> Reading {
    Reading::Pending(format!("l'oggetto «{name}»"))
}

// --- Source -------------------------------------------------------------------------------

/// A group of people by age.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgeGroup {
    All,
    Stage(LifeStage),
}

impl AgeGroup {
    pub const NAMES: [&'static str; 5] = ["tutti", "bambini", "giovani", "adulti", "anziani"];

    fn parse(value: &str) -> Option<AgeGroup> {
        Some(match normalize(value).as_str() {
            "tutti" | "treno" | "totale" => AgeGroup::All,
            "bambini" | "bambino" => AgeGroup::Stage(LifeStage::Bambino),
            "giovani" | "giovane" => AgeGroup::Stage(LifeStage::Giovane),
            "adulti" | "adulto" => AgeGroup::Stage(LifeStage::Adulto),
            "anziani" | "anziano" => AgeGroup::Stage(LifeStage::Anziano),
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            AgeGroup::All => "tutti",
            AgeGroup::Stage(LifeStage::Bambino) => "bambini",
            AgeGroup::Stage(LifeStage::Giovane) => "giovani",
            AgeGroup::Stage(LifeStage::Adulto) => "adulti",
            AgeGroup::Stage(LifeStage::Anziano) => "anziani",
        }
    }
}

/// Which Mercato quotes a price.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarketPick {
    /// The cheapest on the train.
    Cheapest,
    /// The nearest to the player.
    Nearest,
}

/// What of the player.
#[derive(Clone, Debug, PartialEq)]
pub enum PlayerValue {
    Tokens,
    /// Mean affinity (-1..1) of the NPCs who know the player.
    Affinity,
    /// Units of an item in the player's inventory.
    Holds(String),
}

/// A number of the train's economy (as in the summary).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EconomyValue {
    /// Tokens in the administration's treasury.
    Treasury,
    /// Inequality of the tokens, 0–1 (Gini).
    Inequality,
    /// Median tokens of an adult.
    MedianTokens,
}

impl EconomyValue {
    pub const NAMES: [&'static str; 3] = ["tesoro", "disuguaglianza", "gettoni_mediani"];

    fn name(self) -> &'static str {
        Self::NAMES[self as usize]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeValue {
    Day,
    /// Hour of the day with the minutes as a fraction (14.5 = 14:30).
    Hour,
}

/// A number read from the train. Units: stock in whole units, fill and
/// needs 0–1, prices and tokens in gettoni, people as counts, affinity -1–1.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// `{"scorta": item, "carrozza"?: kind}`: units on the train or in the
    /// carriages of a kind.
    Stock {
        item: String,
        carriage: Option<String>,
    },
    /// `{"riempimento": item, "carrozza"?: kind}`: stock over storage
    /// capacity, 0–1.
    Fill {
        item: String,
        carriage: Option<String>,
    },
    /// `{"prezzo": item, "mercato"?: "economico"|"vicino"}`.
    Price { item: String, market: MarketPick },
    /// `{"lavoratori": job}`: people with the job.
    Workers { job: String },
    /// `{"popolazione": "tutti"|"bambini"|"giovani"|"adulti"|"anziani"}`.
    Population(AgeGroup),
    /// `{"bisogno": need, "carrozza"?: kind}`: mean over the people (in the
    /// carriages of a kind), 0–1.
    Need {
        need: Need,
        carriage: Option<String>,
    },
    /// `{"economia": "tesoro"|"disuguaglianza"|"gettoni_mediani"}`.
    Economy(EconomyValue),
    /// `{"giocatore": "gettoni"|"affinita"|item}`.
    Player(PlayerValue),
    /// `{"tempo": "giorno"|"ora"}`.
    Time(TimeValue),
    /// `{"statistica": name}`: a derived statistic.
    Stat { name: String },
    /// Not a valid source: why, in Italian.
    Invalid(String),
}

/// The keys naming a source, in the prompt's order.
pub const SOURCE_KEYS: [&str; 10] = [
    "scorta",
    "riempimento",
    "prezzo",
    "lavoratori",
    "popolazione",
    "bisogno",
    "economia",
    "giocatore",
    "tempo",
    "statistica",
];

/// The kind key of an object and its argument: `{"scorta": "verdura"}` or
/// the longer `{"tipo": "scorta", "oggetto": "verdura"}` some models write.
fn kind_and_arg<'a>(map: &'a Map<String, Value>, keys: &[&str]) -> Option<(String, &'a Value)> {
    for (k, v) in map {
        let key = normalize(k).replace(' ', "_");
        if keys.contains(&key.as_str()) {
            return Some((key, v));
        }
    }
    let tipo = map.get("tipo")?.as_str()?;
    let key = normalize(tipo).replace(' ', "_");
    if !keys.contains(&key.as_str()) {
        return None;
    }
    let arg = [
        "oggetto", "lavoro", "nome", "bisogno", "gruppo", "valore", "carrozza",
    ]
    .iter()
    .find_map(|k| map.get(*k))
    .unwrap_or(&Value::Null);
    Some((key, arg))
}

fn text_of(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn opt_text(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key).and_then(text_of).filter(|s| {
        let n = normalize(s);
        n != "treno" && n != "tutto" && n != "tutti"
    })
}

impl Source {
    /// Parses a source; never fails (see [`Source::Invalid`]).
    pub fn from_value(value: &Value) -> Source {
        let invalid = || {
            Source::Invalid(format!(
                "sorgente non valida {value}: usa una di {}",
                SOURCE_KEYS.join(", ")
            ))
        };
        let Some(map) = value.as_object() else {
            return invalid();
        };
        let Some((kind, arg)) = kind_and_arg(map, &SOURCE_KEYS) else {
            return invalid();
        };
        let arg_text = text_of(arg);
        let need_arg =
            |what: &str| Source::Invalid(format!("la sorgente «{kind}» vuole un nome di {what}"));
        match kind.as_str() {
            "scorta" | "riempimento" => {
                let Some(item) = arg_text else {
                    return need_arg("oggetto");
                };
                let carriage = opt_text(map, "carrozza");
                if kind == "scorta" {
                    Source::Stock { item, carriage }
                } else {
                    Source::Fill { item, carriage }
                }
            }
            "prezzo" => {
                let Some(item) = arg_text else {
                    return need_arg("oggetto");
                };
                let market = match map.get("mercato").and_then(Value::as_str).map(normalize) {
                    Some(m) if m.contains("vicin") => MarketPick::Nearest,
                    _ => MarketPick::Cheapest,
                };
                Source::Price { item, market }
            }
            "lavoratori" => match arg_text {
                Some(job) => Source::Workers { job },
                None => need_arg("lavoro"),
            },
            "popolazione" => {
                let group = arg_text
                    .as_deref()
                    .map_or(Some(AgeGroup::All), AgeGroup::parse);
                match group {
                    Some(g) => Source::Population(g),
                    None => Source::Invalid(format!(
                        "«popolazione» vuole uno di {}",
                        AgeGroup::NAMES.join(", ")
                    )),
                }
            }
            "bisogno" => {
                let need = arg_text.as_deref().and_then(Need::parse);
                match need {
                    Some(need) => Source::Need {
                        need,
                        carriage: opt_text(map, "carrozza"),
                    },
                    None => Source::Invalid(
                        "«bisogno» vuole uno di sazieta, energia, socialita".to_string(),
                    ),
                }
            }
            "economia" => match arg_text.as_deref().map(normalize).as_deref() {
                Some("tesoro") | Some("cassa") => Source::Economy(EconomyValue::Treasury),
                Some("disuguaglianza") | Some("disuguaglianza pct") => {
                    Source::Economy(EconomyValue::Inequality)
                }
                Some("gettoni mediani") | Some("gettoni mediani adulto") => {
                    Source::Economy(EconomyValue::MedianTokens)
                }
                _ => Source::Invalid(format!(
                    "«economia» vuole uno di {}",
                    EconomyValue::NAMES.join(", ")
                )),
            },
            "giocatore" => {
                let Some(what) = arg_text else {
                    return Source::Player(PlayerValue::Tokens);
                };
                Source::Player(match normalize(&what).as_str() {
                    "gettoni" | "soldi" => PlayerValue::Tokens,
                    "affinita" | "affetto" | "simpatia" => PlayerValue::Affinity,
                    _ => PlayerValue::Holds(what),
                })
            }
            "tempo" => match arg_text.as_deref().map(normalize).as_deref() {
                Some("ora") | Some("ore") => Source::Time(TimeValue::Hour),
                Some("giorno") | Some("giorni") | None => Source::Time(TimeValue::Day),
                Some(_) => Source::Invalid("«tempo» vuole «giorno» o «ora»".to_string()),
            },
            "statistica" => match arg_text {
                Some(name) => Source::Stat { name },
                None => need_arg("statistica"),
            },
            _ => invalid(),
        }
    }

    pub fn to_value(&self) -> Value {
        let with_carriage = |key: &str, item: &str, carriage: &Option<String>| {
            let mut v = json!({ key: item });
            if let Some(c) = carriage {
                v["carrozza"] = json!(c);
            }
            v
        };
        match self {
            Source::Stock { item, carriage } => with_carriage("scorta", item, carriage),
            Source::Fill { item, carriage } => with_carriage("riempimento", item, carriage),
            Source::Price { item, market } => json!({
                "prezzo": item,
                "mercato": match market {
                    MarketPick::Cheapest => "economico",
                    MarketPick::Nearest => "vicino",
                }
            }),
            Source::Workers { job } => json!({ "lavoratori": job }),
            Source::Population(g) => json!({ "popolazione": g.name() }),
            Source::Need { need, carriage } => {
                let mut v = json!({ "bisogno": need.key() });
                if let Some(c) = carriage {
                    v["carrozza"] = json!(c);
                }
                v
            }
            Source::Economy(e) => json!({ "economia": e.name() }),
            Source::Player(p) => json!({
                "giocatore": match p {
                    PlayerValue::Tokens => "gettoni",
                    PlayerValue::Affinity => "affinita",
                    PlayerValue::Holds(item) => item.as_str(),
                }
            }),
            Source::Time(t) => json!({
                "tempo": match t {
                    TimeValue::Day => "giorno",
                    TimeValue::Hour => "ora",
                }
            }),
            Source::Stat { name } => json!({ "statistica": name }),
            Source::Invalid(why) => json!({ "non_valida": why }),
        }
    }

    /// Whether the natural value is a fraction (0–1 or -1–1): "percento"
    /// shows it × 100.
    pub fn is_fraction(&self) -> bool {
        matches!(
            self,
            Source::Fill { .. }
                | Source::Need { .. }
                | Source::Player(PlayerValue::Affinity)
                | Source::Economy(EconomyValue::Inequality)
        )
    }

    /// Names this source refers to: `(kind, name)` with kind "oggetto",
    /// "lavoro", "carrozza" or "statistica" (the guard checks them).
    pub fn references(&self) -> Vec<(&'static str, &str)> {
        let mut out = Vec::new();
        match self {
            Source::Stock { item, carriage } | Source::Fill { item, carriage } => {
                out.push(("oggetto", item.as_str()));
                if let Some(c) = carriage {
                    out.push(("carrozza", c.as_str()));
                }
            }
            Source::Price { item, .. } => out.push(("oggetto", item.as_str())),
            Source::Workers { job } => out.push(("lavoro", job.as_str())),
            Source::Need {
                carriage: Some(c), ..
            } => out.push(("carrozza", c.as_str())),
            Source::Player(PlayerValue::Holds(item)) => out.push(("oggetto", item.as_str())),
            Source::Stat { name } => out.push(("statistica", name.as_str())),
            _ => {}
        }
        out
    }

    /// The value now, as a plain number: `NaN` when it can't be read (a
    /// pending name, a missing Mercato, a statistic: use [`Source::read`]).
    pub fn eval(&self, world: &World) -> f32 {
        self.read(world, None).value().unwrap_or(f32::NAN)
    }

    /// The value now; statistics are looked up in `book`.
    pub fn read(&self, world: &World, book: Option<&StatBook>) -> Reading {
        self.read_at(world, book, 0)
    }

    pub(crate) fn read_at(&self, world: &World, book: Option<&StatBook>, depth: u8) -> Reading {
        match self {
            Source::Stock {
                item: name,
                carriage,
            }
            | Source::Fill {
                item: name,
                carriage,
            } => {
                let Some(it) = item(name) else {
                    return pending_item(name);
                };
                let kind = match carriage.as_deref().map(|c| (c, carriage_kind(c))) {
                    None => None,
                    Some((_, Some(k))) => Some(k),
                    Some((c, None)) => {
                        return Reading::Missing(format!("non c'è il tipo di carrozza «{c}»"));
                    }
                };
                let (mut have, mut cap) = (0.0, 0.0);
                for c in world
                    .carriages
                    .iter()
                    .filter(|c| kind.is_none_or(|k| c.kind == k))
                {
                    have += c.stock.get(it).max(0.0);
                    cap += world.params.storage_cap(c.kind, it).max(0.0);
                }
                if matches!(self, Source::Stock { .. }) {
                    return Reading::Value(have.floor());
                }
                if cap <= 0.0 {
                    return Reading::Missing(format!("lì non si conserva «{}»", it.name()));
                }
                Reading::Value((have / cap).clamp(0.0, 1.0))
            }
            Source::Price { item: name, market } => {
                let Some(it) = item(name) else {
                    return pending_item(name);
                };
                let quote = |m| {
                    world
                        .market_quotes(m)
                        .into_iter()
                        .find(|q| q.item == it)
                        .map(|q| q.price)
                };
                let price = match market {
                    MarketPick::Cheapest => world.markets().into_iter().filter_map(quote).min(),
                    MarketPick::Nearest => world
                        .nearest_market(world.player.place.carriage)
                        .and_then(quote),
                };
                match price {
                    Some(p) => Reading::Value(p as f32),
                    None => Reading::Missing(format!("nessun Mercato vende «{}»", it.name())),
                }
            }
            Source::Workers { job: name } => match job(name) {
                Some(j) => {
                    Reading::Value(world.npcs.iter().filter(|n| n.job == Some(j)).count() as f32)
                }
                None => Reading::Pending(format!("il lavoro «{name}»")),
            },
            Source::Population(group) => Reading::Value(
                world
                    .npcs
                    .iter()
                    .filter(|n| match group {
                        AgeGroup::All => true,
                        AgeGroup::Stage(s) => n.stage() == *s,
                    })
                    .count() as f32,
            ),
            Source::Need { need, carriage } => {
                let kind = match carriage.as_deref() {
                    None => None,
                    Some(c) => match carriage_kind(c) {
                        Some(k) => Some(k),
                        None => {
                            return Reading::Missing(format!("non c'è il tipo di carrozza «{c}»"));
                        }
                    },
                };
                let (mut sum, mut n) = (0.0, 0u32);
                for npc in &world.npcs {
                    let here = world.carriages.get(npc.carriage.index()).map(|c| c.kind);
                    if kind.is_some() && here != kind {
                        continue;
                    }
                    sum += match need {
                        Need::Satiety => npc.needs.hunger,
                        Need::Energy => npc.needs.energy,
                        Need::Social => npc.needs.social,
                    };
                    n += 1;
                }
                if n == 0 {
                    return Reading::Missing("non c'è nessuno".to_string());
                }
                Reading::Value((sum / n as f32).clamp(0.0, 1.0))
            }
            Source::Economy(e) => {
                let stats = sim::Stats::of(world);
                Reading::Value(match e {
                    EconomyValue::Treasury => stats.treasury as f32,
                    EconomyValue::Inequality => stats.tokens_gini.clamp(0.0, 1.0),
                    EconomyValue::MedianTokens => stats.median_tokens_per_adult as f32,
                })
            }
            Source::Player(PlayerValue::Tokens) => Reading::Value(world.player.tokens as f32),
            Source::Player(PlayerValue::Affinity) => {
                let ties: Vec<f32> = world
                    .npcs
                    .iter()
                    .filter_map(|n| n.player.map(|t| t.affinity))
                    .collect();
                if ties.is_empty() {
                    return Reading::Value(0.0);
                }
                Reading::Value(ties.iter().sum::<f32>() / ties.len() as f32)
            }
            Source::Player(PlayerValue::Holds(name)) => match item(name) {
                Some(it) => Reading::Value(world.player.inventory.count(it) as f32),
                None => pending_item(name),
            },
            Source::Time(TimeValue::Day) => Reading::Value(world.clock.day() as f32),
            Source::Time(TimeValue::Hour) => {
                let m = world.clock.minute_of_day();
                Reading::Value(m as f32 / 60.0)
            }
            Source::Stat { name } => match book {
                Some(book) => book.read_at(name, world, depth + 1),
                None => Reading::Missing(format!("la statistica «{name}» non è disponibile")),
            },
            Source::Invalid(why) => Reading::Missing(why.clone()),
        }
    }
}

impl Serialize for Source {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.to_value().serialize(s)
    }
}

impl<'de> Deserialize<'de> for Source {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Source::from_value(&Value::deserialize(d)?))
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let place = |c: &Option<String>| c.as_deref().map_or(String::new(), |c| format!(" in {c}"));
        match self {
            Source::Stock { item, carriage } => write!(f, "scorta di {item}{}", place(carriage)),
            Source::Fill { item, carriage } => {
                write!(f, "riempimento di {item}{}", place(carriage))
            }
            Source::Price { item, market } => write!(
                f,
                "prezzo di {item} ({})",
                match market {
                    MarketPick::Cheapest => "Mercato più economico",
                    MarketPick::Nearest => "Mercato più vicino",
                }
            ),
            Source::Workers { job } => write!(f, "{job} al lavoro"),
            Source::Population(g) => write!(f, "popolazione ({})", g.name()),
            Source::Need { need, carriage } => write!(f, "{}{}", need.name(), place(carriage)),
            Source::Economy(e) => write!(f, "economia: {}", e.name().replace('_', " ")),
            Source::Player(PlayerValue::Tokens) => f.write_str("gettoni del giocatore"),
            Source::Player(PlayerValue::Affinity) => f.write_str("affinità verso il giocatore"),
            Source::Player(PlayerValue::Holds(item)) => write!(f, "{item} del giocatore"),
            Source::Time(TimeValue::Day) => f.write_str("giorno"),
            Source::Time(TimeValue::Hour) => f.write_str("ora"),
            Source::Stat { name } => write!(f, "statistica «{name}»"),
            Source::Invalid(why) => write!(f, "(non valida: {why})"),
        }
    }
}

// --- Lists ------------------------------------------------------------------------------------

/// A list read from the train, for a panel's "lista".
#[derive(Clone, Debug, PartialEq)]
pub enum ListSource {
    /// `{"lavoratori": job}`: the people with the job.
    Workers(String),
    /// `{"carrozze_con": item}`: the carriages with the most of an item.
    CarriagesWith(String),
    /// `{"affamati": "treno"|kind}`: the hungriest people.
    Hungry(Option<String>),
    /// `{"amici": "giocatore"}`: who likes the player most.
    Friends,
    /// `{"prezzi": item}`: the Mercati by price, cheapest first.
    Prices(String),
    Invalid(String),
}

/// The keys naming a list source, in the prompt's order.
pub const LIST_KEYS: [&str; 5] = ["lavoratori", "carrozze_con", "affamati", "amici", "prezzi"];

/// One row of a list.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub label: String,
    pub value: String,
}

/// Rows of a list and how many more there are.
#[derive(Clone, Debug, PartialEq)]
pub struct Rows {
    pub rows: Vec<Row>,
    pub more: usize,
}

fn cut(mut rows: Vec<Row>, max: usize) -> Rows {
    let more = rows.len().saturating_sub(max);
    rows.truncate(max);
    Rows { rows, more }
}

impl ListSource {
    pub fn from_value(value: &Value) -> ListSource {
        let invalid = || {
            ListSource::Invalid(format!(
                "lista non valida {value}: usa una di {}",
                LIST_KEYS.join(", ")
            ))
        };
        let Some(map) = value.as_object() else {
            return invalid();
        };
        let Some((kind, arg)) = kind_and_arg(map, &LIST_KEYS) else {
            return invalid();
        };
        let arg = text_of(arg);
        let need = |what: &str| ListSource::Invalid(format!("la lista «{kind}» vuole un {what}"));
        match kind.as_str() {
            "lavoratori" => arg.map_or_else(|| need("lavoro"), ListSource::Workers),
            "carrozze_con" => arg.map_or_else(|| need("oggetto"), ListSource::CarriagesWith),
            "prezzi" => arg.map_or_else(|| need("oggetto"), ListSource::Prices),
            "affamati" => ListSource::Hungry(arg.filter(|a| {
                let n = normalize(a);
                n != "treno" && n != "tutti" && n != "tutto"
            })),
            "amici" => ListSource::Friends,
            _ => invalid(),
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            ListSource::Workers(job) => json!({ "lavoratori": job }),
            ListSource::CarriagesWith(item) => json!({ "carrozze_con": item }),
            ListSource::Hungry(kind) => json!({ "affamati": kind.as_deref().unwrap_or("treno") }),
            ListSource::Friends => json!({ "amici": "giocatore" }),
            ListSource::Prices(item) => json!({ "prezzi": item }),
            ListSource::Invalid(why) => json!({ "non_valida": why }),
        }
    }

    /// Names referenced, as in [`Source::references`].
    pub fn references(&self) -> Vec<(&'static str, &str)> {
        match self {
            ListSource::Workers(job) => vec![("lavoro", job.as_str())],
            ListSource::CarriagesWith(item) | ListSource::Prices(item) => {
                vec![("oggetto", item.as_str())]
            }
            ListSource::Hungry(Some(kind)) => vec![("carrozza", kind.as_str())],
            _ => Vec::new(),
        }
    }

    /// At most `max` rows now, read-only.
    pub fn rows(&self, world: &World, max: usize) -> Result<Rows, Reading> {
        let carriage_name =
            |id: sim::CarriageId| world.carriage(id).map_or(String::new(), |c| c.name.clone());
        let rows = match self {
            ListSource::Workers(name) => {
                let Some(j) = job(name) else {
                    return Err(Reading::Pending(format!("il lavoro «{name}»")));
                };
                let mut people: Vec<&sim::Npc> =
                    world.npcs.iter().filter(|n| n.job == Some(j)).collect();
                let here = world.player.place.carriage;
                people.sort_by_key(|n| (n.carriage.distance(here), n.name.clone()));
                people
                    .into_iter()
                    .map(|n| Row {
                        label: n.name.clone(),
                        value: carriage_name(n.carriage),
                    })
                    .collect()
            }
            ListSource::CarriagesWith(name) => {
                let Some(it) = item(name) else {
                    return Err(pending_item(name));
                };
                let mut cs: Vec<&sim::Carriage> = world
                    .carriages
                    .iter()
                    .filter(|c| c.stock.count(it) > 0)
                    .collect();
                cs.sort_by(|a, b| b.stock.get(it).total_cmp(&a.stock.get(it)));
                cs.into_iter()
                    .map(|c| Row {
                        label: c.name.clone(),
                        value: c.stock.count(it).to_string(),
                    })
                    .collect()
            }
            ListSource::Hungry(kind) => {
                let kind = match kind.as_deref() {
                    None => None,
                    Some(k) => match carriage_kind(k) {
                        Some(k) => Some(k),
                        None => {
                            return Err(Reading::Missing(format!(
                                "non c'è il tipo di carrozza «{k}»"
                            )));
                        }
                    },
                };
                let mut people: Vec<&sim::Npc> = world
                    .npcs
                    .iter()
                    .filter(|n| {
                        kind.is_none()
                            || world.carriages.get(n.carriage.index()).map(|c| c.kind) == kind
                    })
                    .filter(|n| n.needs.hunger < 0.5)
                    .collect();
                people.sort_by(|a, b| a.needs.hunger.total_cmp(&b.needs.hunger));
                people
                    .into_iter()
                    .map(|n| Row {
                        label: n.name.clone(),
                        value: format!("sazietà {:.0}%", n.needs.hunger.clamp(0.0, 1.0) * 100.0),
                    })
                    .collect()
            }
            ListSource::Friends => {
                let mut people: Vec<(&sim::Npc, f32)> = world
                    .npcs
                    .iter()
                    .filter_map(|n| Some((n, n.player?.affinity)))
                    .filter(|(_, a)| *a > 0.0)
                    .collect();
                people.sort_by(|a, b| b.1.total_cmp(&a.1));
                people
                    .into_iter()
                    .map(|(n, a)| Row {
                        label: n.name.clone(),
                        value: format!("affinità {:+.0}%", a * 100.0),
                    })
                    .collect()
            }
            ListSource::Prices(name) => {
                let Some(it) = item(name) else {
                    return Err(pending_item(name));
                };
                let mut prices: Vec<(sim::CarriageId, u32)> = world
                    .markets()
                    .into_iter()
                    .filter_map(|m| {
                        let q = world.market_quotes(m).into_iter().find(|q| q.item == it)?;
                        Some((m, q.price))
                    })
                    .collect();
                if prices.is_empty() {
                    return Err(Reading::Missing(format!(
                        "nessun Mercato vende «{}»",
                        it.name()
                    )));
                }
                prices.sort_by_key(|&(m, p)| (p, m.index()));
                prices
                    .into_iter()
                    .map(|(m, p)| Row {
                        label: carriage_name(m),
                        value: format!("{p} gettoni"),
                    })
                    .collect()
            }
            ListSource::Invalid(why) => return Err(Reading::Missing(why.clone())),
        };
        Ok(cut(rows, max))
    }
}

impl Serialize for ListSource {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.to_value().serialize(s)
    }
}

impl<'de> Deserialize<'de> for ListSource {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(ListSource::from_value(&Value::deserialize(d)?))
    }
}

impl fmt::Display for ListSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ListSource::Workers(job) => write!(f, "chi fa il {job}"),
            ListSource::CarriagesWith(item) => write!(f, "carrozze con più {item}"),
            ListSource::Hungry(None) => f.write_str("i più affamati"),
            ListSource::Hungry(Some(k)) => write!(f, "i più affamati in {k}"),
            ListSource::Friends => f.write_str("gli amici del giocatore"),
            ListSource::Prices(item) => write!(f, "prezzi di {item}"),
            ListSource::Invalid(why) => write!(f, "(non valida: {why})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(json: &str) -> Source {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn resolves_names_leniently() {
        assert_eq!(item("Verdure"), Some(ItemKind::Verdura));
        assert_eq!(item("TÈ"), Some(ItemKind::Te));
        assert_eq!(item("Scialle"), None);
        assert_eq!(job("Cuoco"), Some(Job::Cuoco));
        assert_eq!(carriage_kind("Dormitori"), Some(CarriageKind::Dormitorio));
        assert_eq!(carriage_kind("serre"), Some(CarriageKind::Serra));
        assert_eq!(carriage_kind("Mercati"), Some(CarriageKind::Mercato));
        assert!(recipe("coperta").is_some());
        assert!(recipe("Scialle di stracci").is_none());
    }

    #[test]
    fn json_forms_round_trip() {
        for json in [
            r#"{"scorta":"verdura","carrozza":"Serra"}"#,
            r#"{"riempimento":"razione"}"#,
            r#"{"prezzo":"tè","mercato":"vicino"}"#,
            r#"{"lavoratori":"cuoco"}"#,
            r#"{"popolazione":"anziani"}"#,
            r#"{"bisogno":"energia","carrozza":"Dormitorio"}"#,
            r#"{"giocatore":"gettoni"}"#,
            r#"{"economia":"disuguaglianza"}"#,
            r#"{"economia":"gettoni_mediani"}"#,
            r#"{"giocatore":"affinita"}"#,
            r#"{"giocatore":"coperta"}"#,
            r#"{"tempo":"ora"}"#,
            r#"{"statistica":"Morale"}"#,
        ] {
            let s = src(json);
            assert!(!matches!(s, Source::Invalid(_)), "{json}: {s:?}");
            let back: Source = serde_json::from_value(s.to_value()).unwrap();
            assert_eq!(back, s, "{json}");
        }
        // The long form some models write.
        assert_eq!(
            src(r#"{"tipo":"scorta","oggetto":"verdura"}"#),
            Source::Stock {
                item: "verdura".into(),
                carriage: None
            }
        );
        // "treno" means everywhere.
        assert_eq!(
            src(r#"{"bisogno":"sazietà","carrozza":"treno"}"#),
            Source::Need {
                need: Need::Satiety,
                carriage: None
            }
        );
        let Source::Invalid(why) = src(r#"{"magia":"x"}"#) else {
            panic!()
        };
        assert!(
            why.contains("scorta") && why.contains("statistica"),
            "{why}"
        );
        assert!(matches!(
            src(r#"{"bisogno":"felicita"}"#),
            Source::Invalid(_)
        ));
        assert!(matches!(src(r#""gettoni""#), Source::Invalid(_)));
        let list: ListSource = serde_json::from_str(r#"{"carrozze_con":"verdura"}"#).unwrap();
        assert_eq!(list, ListSource::CarriagesWith("verdura".into()));
        let bad: ListSource = serde_json::from_str(r#"{"tutti":1}"#).unwrap();
        assert!(matches!(bad, ListSource::Invalid(_)));
    }
}
