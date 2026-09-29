//! What the Narratore proposes: one novelty per answer, as JSON.
//!
//! These are the Custode's own types: the Narratore writes them,
//! [`crate::World::review`] checks them and [`crate::World::apply`] makes
//! them real.
//!
//! Everything refers to items, recipes, jobs and kinds of carriage **by
//! name**, never by id: the Custode resolves names to stable keys (like
//! `RecipeDef::key`), ignoring case and accents as [`crate::custode::normalize`]
//! does. Effects are a small closed set that maps onto actions the sim
//! already has; the model combines them, it never writes code.
//!
//! The JSON keys are Italian, as the prompt:
//!
//! ```json
//! {"motivo": "...", "novita": {"tipo": "oggetto", "nome": "...", ...}}
//! ```

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::custode::appearance::Appearance;
use crate::custode::panel::Panel;
use crate::custode::statistic::Statistic;

/// Why the Narratore proposes a novelty: the need or tension of the train it
/// answers. Shown to the player in the chronicle.
pub type Rationale = String;

/// A whole answer of the model: the reason, the novelty and optionally a
/// panel for the player to follow or use it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    #[serde(rename = "motivo")]
    pub rationale: Rationale,
    #[serde(rename = "novita")]
    pub proposal: Proposal,
    /// Custom UI attached to the novelty (see [`crate::panel`]).
    #[serde(
        rename = "interfaccia",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub panel: Option<Panel>,
}

impl Draft {
    /// A draft without a panel.
    pub fn new(rationale: impl Into<String>, proposal: Proposal) -> Self {
        Self {
            rationale: rationale.into(),
            proposal,
            panel: None,
        }
    }
}

/// One novelty for the train.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "tipo")]
pub enum Proposal {
    /// A new item, made from existing items by an existing job.
    #[serde(rename = "oggetto")]
    NewItem {
        #[serde(rename = "nome")]
        name: String,
        #[serde(rename = "descrizione")]
        description: String,
        #[serde(rename = "categoria")]
        category: Category,
        /// Reference price in tokens.
        #[serde(rename = "valore")]
        base_value: u32,
        /// Units per inventory slot.
        #[serde(rename = "pila", default = "default_stack")]
        stack_limit: u32,
        /// Inputs for one unit: (item name, quantity). Empty only for raw
        /// materials (grown or gathered).
        #[serde(rename = "ingredienti", default)]
        made_from: Vec<Ingredient>,
        /// Job name.
        #[serde(rename = "lavoro")]
        made_by_job: String,
        /// How its icon looks (see [`crate::appearance`]).
        #[serde(rename = "aspetto", default, skip_serializing_if = "Option::is_none")]
        appearance: Option<Appearance>,
    },
    /// A new way to make an existing item (or one proposed before).
    #[serde(rename = "ricetta")]
    NewRecipe {
        #[serde(rename = "nome")]
        name: String,
        /// Item name.
        #[serde(rename = "prodotto")]
        output: String,
        /// Units made per batch.
        #[serde(rename = "qta", default = "one")]
        output_qty: u32,
        #[serde(rename = "ingredienti", default)]
        inputs: Vec<Ingredient>,
        /// Job name.
        #[serde(rename = "lavoro")]
        job: String,
        /// Game minutes per batch.
        #[serde(rename = "minuti")]
        minutes: u32,
    },
    /// A new job, working in an existing kind of carriage.
    #[serde(rename = "lavoro")]
    NewJob {
        #[serde(rename = "nome")]
        name: String,
        #[serde(rename = "descrizione")]
        description: String,
        /// Kind of carriage.
        #[serde(rename = "carrozza")]
        workplace_kind: String,
        /// Items (or recipes) it makes, by name.
        #[serde(rename = "produce", default)]
        makes: Vec<String>,
        /// A job that makes nothing (a guard, a doctor, a clerk) serves the
        /// people in its carriage: the need it raises.
        #[serde(rename = "servizio", default, skip_serializing_if = "Option::is_none")]
        service: Option<Need>,
    },
    /// Something that happens once, told as a story, with small effects.
    #[serde(rename = "evento")]
    Event {
        #[serde(rename = "titolo")]
        title: String,
        #[serde(rename = "descrizione")]
        description: String,
        #[serde(rename = "effetti", default)]
        effects: Vec<Effect>,
    },
    /// A new derived number that measures a tension of the train (see
    /// [`crate::statistic`]).
    #[serde(rename = "statistica")]
    Statistic(Statistic),
}

fn default_stack() -> u32 {
    10
}

fn one() -> u32 {
    1
}

/// An input of a recipe: the "(item_name, qty)" pair, as an object because
/// models write objects more reliably than tuples.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ingredient {
    #[serde(rename = "oggetto")]
    pub item: String,
    #[serde(rename = "qta")]
    pub qty: u32,
}

/// Broad kind of a new item (as `sim::ItemCategory`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Category {
    #[serde(rename = "materia_prima")]
    Raw,
    #[serde(rename = "semilavorato")]
    Intermediate,
    #[serde(rename = "consumabile")]
    Consumable,
    #[serde(rename = "durevole")]
    Durable,
}

impl Category {
    pub fn name(self) -> &'static str {
        match self {
            Category::Raw => "materia prima",
            Category::Intermediate => "semilavorato",
            Category::Consumable => "consumabile",
            Category::Durable => "durevole",
        }
    }
}

/// A basic action an event performs, from a small closed set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "effetto")]
pub enum Effect {
    /// Adds (or with a negative `delta` removes) units of an item, spread
    /// over the carriages of a kind (the sim's `Stock::add`/`take`).
    #[serde(rename = "scorta")]
    Stock {
        #[serde(rename = "carrozza")]
        carriage_kind: String,
        #[serde(rename = "oggetto")]
        item: String,
        delta: i32,
    },
    /// Nudges a need of the people in the carriages of a kind (everyone when
    /// `None`); positive is better (the sim's `Needs`).
    #[serde(rename = "bisogno")]
    Need {
        #[serde(rename = "carrozza", default)]
        carriage_kind: Option<String>,
        #[serde(rename = "bisogno")]
        need: Need,
        delta: f32,
    },
}

/// A need of the sim's `Needs`, 1 = satisfied.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Need {
    #[serde(rename = "sazieta", alias = "sazietà", alias = "fame")]
    Satiety,
    #[serde(rename = "energia")]
    Energy,
    #[serde(rename = "socialita", alias = "socialità", alias = "compagnia")]
    Social,
}

impl Need {
    pub const ALL: [Need; 3] = [Need::Satiety, Need::Energy, Need::Social];

    /// The JSON key: "sazieta", "energia", "socialita".
    pub fn key(self) -> &'static str {
        match self {
            Need::Satiety => "sazieta",
            Need::Energy => "energia",
            Need::Social => "socialita",
        }
    }

    /// A need from its key or a synonym, case and accents ignored.
    pub fn parse(text: &str) -> Option<Need> {
        match crate::custode::names::normalize(text).as_str() {
            "sazieta" | "fame" | "cibo" => Some(Need::Satiety),
            "energia" | "sonno" | "stanchezza" => Some(Need::Energy),
            "socialita" | "compagnia" | "solitudine" => Some(Need::Social),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Need::Satiety => "sazietà",
            Need::Energy => "energia",
            Need::Social => "socialità",
        }
    }
}

impl Proposal {
    /// Italian name of the kind of novelty ("oggetto", "ricetta"…).
    pub fn kind_name(&self) -> &'static str {
        match self {
            Proposal::NewItem { .. } => "oggetto",
            Proposal::NewRecipe { .. } => "ricetta",
            Proposal::NewJob { .. } => "lavoro",
            Proposal::Event { .. } => "evento",
            Proposal::Statistic(_) => "statistica",
        }
    }

    /// The new name (the title for an event).
    pub fn name(&self) -> &str {
        match self {
            Proposal::NewItem { name, .. }
            | Proposal::NewRecipe { name, .. }
            | Proposal::NewJob { name, .. } => name,
            Proposal::Event { title, .. } => title,
            Proposal::Statistic(s) => &s.name,
        }
    }

    /// The description (for a recipe, what it makes).
    pub fn description(&self) -> String {
        match self {
            Proposal::NewItem { description, .. }
            | Proposal::NewJob { description, .. }
            | Proposal::Event { description, .. } => description.clone(),
            Proposal::NewRecipe {
                inputs,
                output,
                output_qty,
                job,
                ..
            } => format!(
                "{} → {output_qty} {output}, la fa {job}",
                ingredients(inputs)
            ),
            Proposal::Statistic(s) => s.description.clone(),
        }
    }
}

fn ingredients(list: &[Ingredient]) -> String {
    if list.is_empty() {
        return "niente (si raccoglie o si coltiva)".to_string();
    }
    list.iter()
        .map(|i| format!("{} {}", i.qty, i.item))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Readable Italian, for tools and logs.
impl fmt::Display for Proposal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Proposal::NewItem {
                name,
                description,
                category,
                base_value,
                stack_limit,
                made_from,
                made_by_job,
                appearance,
            } => {
                writeln!(
                    f,
                    "Nuovo oggetto «{name}» ({}, valore {base_value} gettoni, {stack_limit} per casella)",
                    category.name()
                )?;
                writeln!(f, "  {description}")?;
                write!(f, "  Lo fa: {made_by_job}, con {}", ingredients(made_from))?;
                if let Some(a) = appearance {
                    write!(f, "\n  Aspetto: {} {}", a.shape, a.colour)?;
                    if let Some(d) = &a.detail {
                        write!(f, " con {d}")?;
                    }
                }
                Ok(())
            }
            Proposal::NewRecipe {
                name,
                output,
                output_qty,
                inputs,
                job,
                minutes,
            } => write!(
                f,
                "Nuova ricetta «{name}»: {} → {output_qty} {output}, la fa {job} in {minutes} minuti",
                ingredients(inputs)
            ),
            Proposal::NewJob {
                name,
                description,
                workplace_kind,
                makes,
                service,
            } => {
                writeln!(f, "Nuovo lavoro «{name}» in {workplace_kind}")?;
                writeln!(f, "  {description}")?;
                if let Some(need) = service {
                    write!(f, "  Servizio: {}", need.name())?;
                    if !makes.is_empty() {
                        writeln!(f)?;
                    }
                }
                if !makes.is_empty() || service.is_none() {
                    write!(f, "  Produce: {}", makes.join(", "))?;
                }
                Ok(())
            }
            Proposal::Event {
                title,
                description,
                effects,
            } => {
                writeln!(f, "Evento «{title}»")?;
                write!(f, "  {description}")?;
                for e in effects {
                    write!(f, "\n  Effetto: {e}")?;
                }
                Ok(())
            }
            Proposal::Statistic(s) => {
                let unit = s.unit.as_deref().map_or(String::new(), |u| format!(" {u}"));
                writeln!(
                    f,
                    "Nuova statistica «{}» (da {} a {}{unit})",
                    s.name, s.scale[0], s.scale[1]
                )?;
                writeln!(f, "  {}", s.description)?;
                write!(f, "  Formula: {}", s.formula)?;
                for t in &s.thresholds {
                    match (t.below, t.above) {
                        (Some(b), _) => write!(f, "\n  Sotto {b}: {}", t.text)?,
                        (None, Some(a)) => write!(f, "\n  Sopra {a}: {}", t.text)?,
                        (None, None) => write!(f, "\n  Soglia senza limite: {}", t.text)?,
                    }
                }
                Ok(())
            }
        }
    }
}

impl fmt::Display for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Effect::Stock {
                carriage_kind,
                item,
                delta,
            } => write!(
                f,
                "{delta:+} {item}, divisi tra le carrozze {carriage_kind}"
            ),
            Effect::Need {
                carriage_kind,
                need,
                delta,
            } => {
                let place = carriage_kind
                    .as_deref()
                    .map_or("tutti".to_string(), |k| format!("chi è in {k}"));
                write!(f, "{} {delta:+.2} per {place}", need.name())
            }
        }
    }
}

impl fmt::Display for Draft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.proposal)?;
        write!(f, "  Perché: {}", self.rationale)?;
        if let Some(p) = &self.panel {
            write!(f, "\n  {p}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_shapes() {
        let item = r#"{"motivo":"Fa freddo nei Dormitori.","novita":{"tipo":"oggetto","nome":"Scialle di stracci","descrizione":"Uno scialle cucito con ritagli di tessuto.","categoria":"durevole","valore":12,"ingredienti":[{"oggetto":"tessuto","qta":2}],"lavoro":"operaio"}}"#;
        let d: Draft = serde_json::from_str(item).unwrap();
        assert_eq!(d.proposal.kind_name(), "oggetto");
        assert_eq!(d.proposal.name(), "Scialle di stracci");
        let Proposal::NewItem { stack_limit, .. } = &d.proposal else {
            panic!()
        };
        assert_eq!(*stack_limit, 10);
        // Round trip.
        let back: Draft = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(back, d);

        let event = r#"{"motivo":"m","novita":{"tipo":"evento","titolo":"La grande gelata","descrizione":"d","effetti":[{"effetto":"scorta","carrozza":"Serra","oggetto":"verdura","delta":-20},{"effetto":"bisogno","bisogno":"sazietà","delta":-0.1}]}}"#;
        let d: Draft = serde_json::from_str(event).unwrap();
        let Proposal::Event { effects, .. } = &d.proposal else {
            panic!()
        };
        assert_eq!(effects.len(), 2);
        assert!(d.to_string().contains("sazietà -0.10 per tutti"), "{d}");

        let unknown = r#"{"motivo":"m","novita":{"tipo":"magia","nome":"x"}}"#;
        assert!(serde_json::from_str::<Draft>(unknown).is_err());
    }
}
