//! Custom UI the Narratore attaches to a novelty: a small panel of texts,
//! live values, bars, lists and buttons, written as **declarative data**
//! that the game renders generically (`ai_ui.rs` in `game`). The model never
//! writes code: values come from the closed [`Source`] / [`ListSource`]
//! whitelists, buttons from the closed [`Action`] whitelist, which maps onto
//! actions the player already has.
//!
//! Panels only read the world. The Custode keeps a novelty's panel with
//! its record ([`crate::World::decisions`]); buttons act on what exists, so
//! one that names something not in the world (yet) is disabled.
//!
//! ```json
//! "interfaccia": {"titolo": "Scialli in coda", "elementi": [
//!   {"tipo": "valore", "etichetta": "Tessuto", "sorgente": {"scorta": "tessuto"}, "formato": "numero"},
//!   {"tipo": "pulsante", "etichetta": "Cuci", "azione": {"apri_crafting": "Scialle di stracci"}}]}
//! ```

use std::fmt;

use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::custode::names::normalize;
use crate::custode::sources::{ListSource, Source};

/// Most elements of a panel.
pub const MAX_ELEMENTS: usize = 8;
/// Rows a list shows at most.
pub const MAX_ROWS: usize = 8;

/// A panel attached to a novelty.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Panel {
    #[serde(rename = "titolo")]
    pub title: String,
    #[serde(rename = "elementi", default)]
    pub elements: Vec<Element>,
}

/// How a value is written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Format {
    #[default]
    #[serde(rename = "numero")]
    Number,
    /// Fractions (needs, fill, affinity) × 100.
    #[serde(rename = "percento", alias = "percentuale")]
    Percent,
    #[serde(rename = "gettoni")]
    Tokens,
}

/// One line of a panel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "tipo")]
pub enum Element {
    #[serde(rename = "testo")]
    Text {
        #[serde(rename = "testo")]
        text: String,
    },
    #[serde(rename = "valore")]
    Value {
        #[serde(rename = "etichetta")]
        label: String,
        #[serde(rename = "sorgente")]
        source: Source,
        #[serde(rename = "formato", default)]
        format: Format,
    },
    #[serde(rename = "barra")]
    Bar {
        #[serde(rename = "etichetta")]
        label: String,
        #[serde(rename = "sorgente")]
        source: Source,
        min: f32,
        max: f32,
    },
    #[serde(rename = "lista")]
    List {
        #[serde(rename = "etichetta")]
        label: String,
        #[serde(rename = "sorgente_lista", alias = "sorgente")]
        source: ListSource,
    },
    #[serde(rename = "pulsante")]
    Button {
        #[serde(rename = "etichetta")]
        label: String,
        #[serde(rename = "azione")]
        action: Action,
    },
    /// Any other "tipo": the guard rejects it with the allowed list.
    #[serde(other)]
    Unknown,
}

/// The element kinds, in the prompt's order.
pub const ELEMENT_KINDS: [&str; 5] = ["testo", "valore", "barra", "lista", "pulsante"];

impl Element {
    pub fn kind(&self) -> &'static str {
        match self {
            Element::Text { .. } => "testo",
            Element::Value { .. } => "valore",
            Element::Bar { .. } => "barra",
            Element::List { .. } => "lista",
            Element::Button { .. } => "pulsante",
            Element::Unknown => "sconosciuto",
        }
    }
}

/// What a panel button does: an existing player action.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// `{"apri_crafting": recipe or item}`: the crafting window (C) focused
    /// on that recipe.
    OpenCrafting(String),
    /// `{"apri_mercato": item}`: the Mercato window (M) on that item.
    OpenMarket(String),
    /// `{"apri_inventario": true}`.
    OpenInventory,
    /// `{"parla_con_lavoro": job}`: a chat with the nearest person with
    /// that job, if someone is at hand.
    TalkToJob(String),
    /// `{"mostra_statistica": name}`: the statistics window on it.
    ShowStat(String),
    /// `{"vai_a": kind}`: where the nearest carriage of that kind is (no
    /// teleport).
    GoTo(String),
    Invalid(String),
}

/// The action keys, in the prompt's order.
pub const ACTION_KEYS: [&str; 6] = [
    "apri_crafting",
    "apri_mercato",
    "apri_inventario",
    "parla_con_lavoro",
    "mostra_statistica",
    "vai_a",
];

impl Action {
    pub fn from_value(value: &Value) -> Action {
        let invalid = || {
            Action::Invalid(format!(
                "azione non valida {value}: usa una di {}",
                ACTION_KEYS.join(", ")
            ))
        };
        // "apri_inventario" alone, or {"apri_inventario": true}.
        let (key, arg) = match value {
            Value::String(s) => (normalize(s).replace(' ', "_"), None),
            Value::Object(map) => {
                let found = map.iter().find_map(|(k, v)| {
                    let key = normalize(k).replace(' ', "_");
                    ACTION_KEYS.contains(&key.as_str()).then_some((key, v))
                });
                match found {
                    Some((k, v)) => (k, v.as_str().map(str::trim).map(str::to_string)),
                    None => {
                        // {"tipo": "apri_mercato", "oggetto": "tè"}
                        let Some(tipo) = map.get("tipo").and_then(Value::as_str) else {
                            return invalid();
                        };
                        let arg = ["ricetta", "oggetto", "lavoro", "nome", "carrozza"]
                            .iter()
                            .find_map(|k| map.get(*k).and_then(Value::as_str))
                            .map(str::to_string);
                        (normalize(tipo).replace(' ', "_"), arg)
                    }
                }
            }
            _ => return invalid(),
        };
        let arg = arg.filter(|a| !a.is_empty());
        let need = |what: &str| Action::Invalid(format!("l'azione «{key}» vuole un {what}"));
        match key.as_str() {
            "apri_inventario" => Action::OpenInventory,
            "apri_crafting" => arg.map_or_else(|| need("nome di ricetta"), Action::OpenCrafting),
            "apri_mercato" => arg.map_or_else(|| need("oggetto"), Action::OpenMarket),
            "parla_con_lavoro" => arg.map_or_else(|| need("lavoro"), Action::TalkToJob),
            "mostra_statistica" => arg.map_or_else(|| need("nome di statistica"), Action::ShowStat),
            "vai_a" => arg.map_or_else(|| need("tipo di carrozza"), Action::GoTo),
            _ => invalid(),
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            Action::OpenCrafting(r) => json!({ "apri_crafting": r }),
            Action::OpenMarket(i) => json!({ "apri_mercato": i }),
            Action::OpenInventory => json!({ "apri_inventario": true }),
            Action::TalkToJob(j) => json!({ "parla_con_lavoro": j }),
            Action::ShowStat(s) => json!({ "mostra_statistica": s }),
            Action::GoTo(k) => json!({ "vai_a": k }),
            Action::Invalid(why) => json!({ "non_valida": why }),
        }
    }

    /// The name it refers to: `(kind, name)` as in [`Source::references`]
    /// ("ricetta" for crafting: a recipe or an item).
    pub fn reference(&self) -> Option<(&'static str, &str)> {
        match self {
            Action::OpenCrafting(r) => Some(("ricetta", r)),
            Action::OpenMarket(i) => Some(("oggetto", i)),
            Action::TalkToJob(j) => Some(("lavoro", j)),
            Action::ShowStat(s) => Some(("statistica", s)),
            Action::GoTo(k) => Some(("carrozza", k)),
            Action::OpenInventory | Action::Invalid(_) => None,
        }
    }
}

impl Serialize for Action {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.to_value().serialize(s)
    }
}

impl<'de> Deserialize<'de> for Action {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Action::from_value(&Value::deserialize(d)?))
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Action::OpenCrafting(r) => write!(f, "apri il crafting su «{r}»"),
            Action::OpenMarket(i) => write!(f, "apri il Mercato su «{i}»"),
            Action::OpenInventory => f.write_str("apri l'inventario"),
            Action::TalkToJob(j) => write!(f, "parla con un {j}"),
            Action::ShowStat(s) => write!(f, "mostra la statistica «{s}»"),
            Action::GoTo(k) => write!(f, "dov'è la {k} più vicina"),
            Action::Invalid(why) => write!(f, "(non valida: {why})"),
        }
    }
}

impl fmt::Display for Panel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Pannello «{}»", self.title)?;
        for e in &self.elements {
            match e {
                Element::Text { text } => write!(f, "\n    testo: {text}")?,
                Element::Value {
                    label,
                    source,
                    format,
                } => write!(f, "\n    valore «{label}»: {source} ({format:?})")?,
                Element::Bar {
                    label,
                    source,
                    min,
                    max,
                } => write!(f, "\n    barra «{label}»: {source} da {min} a {max}")?,
                Element::List { label, source } => write!(f, "\n    lista «{label}»: {source}")?,
                Element::Button { label, action } => {
                    write!(f, "\n    pulsante «{label}»: {action}")?
                }
                Element::Unknown => write!(f, "\n    (elemento sconosciuto)")?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_element_and_action() {
        let json = r#"{"titolo":"Scialli in coda","elementi":[
            {"tipo":"testo","testo":"Cuciti con i ritagli."},
            {"tipo":"valore","etichetta":"Tessuto","sorgente":{"scorta":"tessuto"},"formato":"numero"},
            {"tipo":"barra","etichetta":"Energia","sorgente":{"bisogno":"energia","carrozza":"Dormitorio"},"min":0,"max":1},
            {"tipo":"lista","etichetta":"Chi cuce","sorgente_lista":{"lavoratori":"operaio"}},
            {"tipo":"pulsante","etichetta":"Cuci","azione":{"apri_crafting":"Scialle"}},
            {"tipo":"pulsante","etichetta":"Borsa","azione":"apri_inventario"},
            {"tipo":"pulsante","etichetta":"Borsa","azione":{"apri_inventario":true}},
            {"tipo":"slider","etichetta":"x"}]}"#;
        let p: Panel = serde_json::from_str(json).unwrap();
        let kinds: Vec<&str> = p.elements.iter().map(Element::kind).collect();
        assert_eq!(
            kinds,
            [
                "testo",
                "valore",
                "barra",
                "lista",
                "pulsante",
                "pulsante",
                "pulsante",
                "sconosciuto"
            ]
        );
        assert_eq!(
            p.elements[4],
            Element::Button {
                label: "Cuci".into(),
                action: Action::OpenCrafting("Scialle".into())
            }
        );
        assert!(matches!(
            p.elements[5],
            Element::Button {
                action: Action::OpenInventory,
                ..
            }
        ));
        let back: Panel = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back, p);
        let long: Action =
            serde_json::from_str(r#"{"tipo":"apri_mercato","oggetto":"tè"}"#).unwrap();
        assert_eq!(long, Action::OpenMarket("tè".into()));
        let bad: Action = serde_json::from_str(r#"{"teletrasporta":"Serra"}"#).unwrap();
        assert!(matches!(bad, Action::Invalid(why) if why.contains("vai_a")));
    }
}
