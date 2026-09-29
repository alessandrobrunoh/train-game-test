//! Derived statistics: new numbers the Narratore invents when the train has
//! a tension no existing number captures ("Morale", "Tensione in coda").
//!
//! A statistic is **data, not code**: a [`Formula`] is a tiny closed
//! expression tree over [`Source`]s (one aggregate of at most
//! [`MAX_TERMS`] weighted terms, or a single source), clamped to the
//! statistic's scale. It is evaluated read-only on `&World` by a
//! [`StatBook`], which also resolves statistics that read other statistics.
//!
//! Statistics don't change the world: once the Custode applies one it
//! lives in the World ([`crate::World::statistics`]), sampled every game
//! hour, with its history saved with the world.
//!
//! ```json
//! {"tipo": "statistica", "nome": "Morale", "descrizione": "...", "unita": "%",
//!  "scala": [0, 100],
//!  "formula": {"media": [{"peso": 100, "sorgente": {"bisogno": "sazieta"}},
//!                        {"peso": 100, "sorgente": {"bisogno": "socialita"}}]},
//!  "soglie": [{"sotto": 30, "testo": "Il treno è allo stremo"}]}
//! ```

use std::collections::{BTreeMap, HashSet};
use std::fmt;

use crate::world::World;
use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::custode::names::normalize;
use crate::custode::sources::{Reading, Source};

/// Most terms of a formula.
pub const MAX_TERMS: usize = 6;
/// Largest absolute weight of a term.
pub const MAX_WEIGHT: f32 = 1000.0;
/// Largest absolute bound of a scale or a threshold.
pub const MAX_SCALE: f32 = 1_000_000.0;
/// Most thresholds of a statistic.
pub const MAX_THRESHOLDS: usize = 4;
/// Deepest chain of statistics reading statistics.
pub const MAX_DEPTH: u8 = 6;

/// How the terms of a formula combine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// Σ(weight × value) / number of terms.
    Mean,
    /// Σ(weight × value).
    Sum,
    /// min(weight × value).
    Min,
    /// max(weight × value).
    Max,
}

impl Op {
    pub const KEYS: [&'static str; 4] = ["media", "somma", "minimo", "massimo"];

    pub fn key(self) -> &'static str {
        Self::KEYS[self as usize]
    }

    fn parse(key: &str) -> Option<Op> {
        Some(match key {
            "media" => Op::Mean,
            "somma" => Op::Sum,
            "minimo" | "min" => Op::Min,
            "massimo" | "max" => Op::Max,
            _ => return None,
        })
    }
}

/// A weighted source.
#[derive(Clone, Debug, PartialEq)]
pub struct Term {
    pub weight: f32,
    pub source: Source,
}

/// The expression of a statistic.
#[derive(Clone, Debug, PartialEq)]
pub enum Formula {
    /// `{"media"|"somma"|"minimo"|"massimo": [{"peso": n, "sorgente": S}]}`.
    Combine { op: Op, terms: Vec<Term> },
    /// `{"sorgente": S, "peso"?: n}`.
    Single(Term),
    /// Not a valid formula: why.
    Invalid(String),
}

fn term_of(v: &Value) -> Result<Term, String> {
    let Some(map) = v.as_object() else {
        return Err(format!(
            "un termine deve essere {{\"peso\": n, \"sorgente\": ...}}, non {v}"
        ));
    };
    let weight = match map.get("peso") {
        None => 1.0,
        Some(w) => w
            .as_f64()
            .ok_or_else(|| format!("il peso {w} non è un numero"))? as f32,
    };
    let source = match map.get("sorgente") {
        Some(s) => Source::from_value(s),
        // A bare source as a term.
        None => Source::from_value(v),
    };
    Ok(Term { weight, source })
}

impl Formula {
    pub fn from_value(value: &Value) -> Formula {
        let Some(map) = value.as_object() else {
            return Formula::Invalid(format!("formula non valida: {value}"));
        };
        for (k, v) in map {
            let key = normalize(k);
            if let Some(op) = Op::parse(&key) {
                let Some(list) = v.as_array() else {
                    return Formula::Invalid(format!("«{key}» vuole una lista di termini"));
                };
                return match list.iter().map(term_of).collect::<Result<Vec<_>, _>>() {
                    Ok(terms) => Formula::Combine { op, terms },
                    Err(why) => Formula::Invalid(why),
                };
            }
        }
        if map.contains_key("sorgente") {
            return match term_of(value) {
                Ok(t) => Formula::Single(t),
                Err(why) => Formula::Invalid(why),
            };
        }
        Formula::Invalid(format!(
            "la formula deve avere una chiave tra {} oppure «sorgente»",
            Op::KEYS.join(", ")
        ))
    }

    pub fn to_value(&self) -> Value {
        let term = |t: &Term| json!({ "peso": t.weight, "sorgente": t.source.to_value() });
        match self {
            Formula::Combine { op, terms } => {
                json!({ op.key(): terms.iter().map(term).collect::<Vec<_>>() })
            }
            Formula::Single(t) => term(t),
            Formula::Invalid(why) => json!({ "non_valida": why }),
        }
    }

    /// Its terms (one for [`Formula::Single`]).
    pub fn terms(&self) -> &[Term] {
        match self {
            Formula::Combine { terms, .. } => terms,
            Formula::Single(t) => std::slice::from_ref(t),
            Formula::Invalid(_) => &[],
        }
    }

    /// Statistics it reads, by name.
    pub fn stat_refs(&self) -> Vec<&str> {
        self.terms()
            .iter()
            .filter_map(|t| match &t.source {
                Source::Stat { name } => Some(name.as_str()),
                _ => None,
            })
            .collect()
    }

    /// The unclamped value; the first term that can't be read makes the
    /// whole formula unreadable.
    fn eval(&self, world: &World, book: Option<&StatBook>, depth: u8) -> Reading {
        let op = match self {
            Formula::Combine { op, .. } => *op,
            Formula::Single(_) => Op::Sum,
            Formula::Invalid(why) => return Reading::Missing(why.clone()),
        };
        let mut values = Vec::with_capacity(self.terms().len());
        for t in self.terms() {
            match t.source.read_at(world, book, depth) {
                Reading::Value(v) => values.push(t.weight * v),
                other => return other,
            }
        }
        if values.is_empty() {
            return Reading::Missing("formula senza termini".to_string());
        }
        Reading::Value(match op {
            Op::Mean => values.iter().sum::<f32>() / values.len() as f32,
            Op::Sum => values.iter().sum(),
            Op::Min => values.iter().copied().fold(f32::INFINITY, f32::min),
            Op::Max => values.iter().copied().fold(f32::NEG_INFINITY, f32::max),
        })
    }
}

impl Serialize for Formula {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.to_value().serialize(s)
    }
}

impl<'de> Deserialize<'de> for Formula {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Formula::from_value(&Value::deserialize(d)?))
    }
}

fn number(v: f32) -> String {
    if v.fract() == 0.0 && v.abs() < 1e7 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

impl fmt::Display for Formula {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let term = |t: &Term| {
            if t.weight == 1.0 {
                t.source.to_string()
            } else {
                format!("{}×{}", number(t.weight), t.source)
            }
        };
        match self {
            Formula::Combine { op, terms } => write!(
                f,
                "{}({})",
                op.key(),
                terms.iter().map(term).collect::<Vec<_>>().join(", ")
            ),
            Formula::Single(t) => f.write_str(&term(t)),
            Formula::Invalid(why) => write!(f, "(non valida: {why})"),
        }
    }
}

/// A line of text shown when the value is under or over a bound.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Threshold {
    #[serde(rename = "sotto", default, skip_serializing_if = "Option::is_none")]
    pub below: Option<f32>,
    #[serde(rename = "sopra", default, skip_serializing_if = "Option::is_none")]
    pub above: Option<f32>,
    #[serde(rename = "testo")]
    pub text: String,
}

impl Threshold {
    pub fn applies(&self, value: f32) -> bool {
        match (self.below, self.above) {
            (Some(b), _) => value < b,
            (None, Some(a)) => value > a,
            (None, None) => false,
        }
    }
}

/// A derived statistic (the body of the "statistica" novelty).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Statistic {
    #[serde(rename = "nome")]
    pub name: String,
    #[serde(rename = "descrizione")]
    pub description: String,
    #[serde(rename = "unita", default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// `[min, max]`: the value is clamped to it and bars are drawn on it.
    #[serde(rename = "scala")]
    pub scale: [f32; 2],
    pub formula: Formula,
    #[serde(rename = "soglie", default)]
    pub thresholds: Vec<Threshold>,
}

impl Statistic {
    /// The value now, clamped to the scale; statistics it reads come from
    /// `book`.
    pub fn read(&self, world: &World, book: Option<&StatBook>) -> Reading {
        self.read_at(world, book, 0)
    }

    fn read_at(&self, world: &World, book: Option<&StatBook>, depth: u8) -> Reading {
        if depth > MAX_DEPTH {
            return Reading::Missing("statistiche annidate troppo a fondo".to_string());
        }
        match self.formula.eval(world, book, depth) {
            Reading::Value(v) if v.is_finite() => {
                let [lo, hi] = self.scale;
                Reading::Value(v.clamp(lo.min(hi), hi.max(lo)))
            }
            Reading::Value(_) => Reading::Missing("valore non finito".to_string()),
            other => other,
        }
    }

    /// The texts of the thresholds that apply to `value`.
    pub fn notes(&self, value: f32) -> Vec<&str> {
        self.thresholds
            .iter()
            .filter(|t| t.applies(value))
            .map(|t| t.text.as_str())
            .collect()
    }

    /// "63 %" with the unit.
    pub fn format(&self, value: f32) -> String {
        let v = if (self.scale[1] - self.scale[0]).abs() <= 10.0 {
            format!("{value:.2}")
        } else {
            format!("{value:.0}")
        };
        match self
            .unit
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
        {
            Some("%") => format!("{v}%"),
            Some(u) => format!("{v} {u}"),
            None => v,
        }
    }
}

/// The derived statistics known now, by normalized name: evaluates them
/// read-only, statistics reading statistics included.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatBook {
    stats: BTreeMap<String, Statistic>,
    /// Keys in the order they were added.
    order: Vec<String>,
}

impl StatBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds (or replaces) a statistic.
    pub fn add(&mut self, stat: Statistic) {
        let key = normalize(&stat.name);
        if !self.stats.contains_key(&key) {
            self.order.push(key.clone());
        }
        self.stats.insert(key, stat);
    }

    pub fn get(&self, name: &str) -> Option<&Statistic> {
        self.stats.get(&normalize(name))
    }

    /// In the order they were added.
    pub fn iter(&self) -> impl Iterator<Item = &Statistic> {
        self.order.iter().filter_map(|k| self.stats.get(k))
    }

    pub fn len(&self) -> usize {
        self.stats.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stats.is_empty()
    }

    /// The value of the statistic called `name` now.
    pub fn read(&self, name: &str, world: &World) -> Reading {
        self.read_at(name, world, 0)
    }

    pub(crate) fn read_at(&self, name: &str, world: &World, depth: u8) -> Reading {
        match self.get(name) {
            Some(stat) => stat.read_at(world, Some(self), depth),
            None => Reading::Pending(format!("la statistica «{name}»")),
        }
    }
}

/// Whether adding a statistic `name` reading `refs` would close a cycle,
/// given `deps` (statistic key → keys it reads). Returns the cycle's path.
pub fn cycle(
    name: &str,
    refs: &[&str],
    deps: &BTreeMap<String, Vec<String>>,
) -> Option<Vec<String>> {
    let start = normalize(name);
    let mut stack: Vec<(String, Vec<String>)> = refs
        .iter()
        .map(|r| (normalize(r), vec![start.clone(), normalize(r)]))
        .collect();
    let mut seen = HashSet::new();
    while let Some((key, path)) = stack.pop() {
        if key == start {
            return Some(path);
        }
        if !seen.insert(key.clone()) {
            continue;
        }
        for next in deps.get(&key).into_iter().flatten() {
            let mut p = path.clone();
            p.push(next.clone());
            stack.push((next.clone(), p));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn morale() -> Statistic {
        serde_json::from_str(
            r#"{"nome":"Morale","descrizione":"Quanto regge l'animo del treno.","unita":"%","scala":[0,100],
            "formula":{"media":[{"peso":100,"sorgente":{"bisogno":"sazieta"}},{"peso":100,"sorgente":{"bisogno":"socialita"}},{"peso":100,"sorgente":{"bisogno":"energia"}}]},
            "soglie":[{"sotto":30,"testo":"Il treno è allo stremo"},{"sopra":80,"testo":"Si canta nelle carrozze"}]}"#,
        )
        .unwrap()
    }

    #[test]
    fn parses_and_round_trips() {
        let m = morale();
        assert_eq!(m.formula.terms().len(), 3);
        assert!(matches!(m.formula, Formula::Combine { op: Op::Mean, .. }));
        let back: Statistic = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
        let single: Formula =
            serde_json::from_str(r#"{"sorgente":{"giocatore":"gettoni"}}"#).unwrap();
        assert!(matches!(single, Formula::Single(Term { weight: 1.0, .. })));
        let bad: Formula = serde_json::from_str(r#"{"prodotto":[]}"#).unwrap();
        assert!(matches!(bad, Formula::Invalid(_)));
        assert!(m.to_string_formula().starts_with("media(100×"));
    }

    impl Statistic {
        fn to_string_formula(&self) -> String {
            self.formula.to_string()
        }
    }

    #[test]
    fn thresholds_and_format() {
        let m = morale();
        assert_eq!(m.notes(20.0), ["Il treno è allo stremo"]);
        assert_eq!(m.notes(50.0), Vec::<&str>::new());
        assert_eq!(m.notes(90.0), ["Si canta nelle carrozze"]);
        assert_eq!(m.format(63.4), "63%");
    }

    #[test]
    fn cycles_are_found() {
        let mut deps = BTreeMap::new();
        deps.insert("a".to_string(), vec!["b".to_string()]);
        deps.insert("b".to_string(), vec!["c".to_string()]);
        assert!(cycle("c", &["a"], &deps).is_some());
        assert!(cycle("d", &["a"], &deps).is_none());
        assert!(cycle("Morale", &["morale"], &BTreeMap::new()).is_some());
    }
}
