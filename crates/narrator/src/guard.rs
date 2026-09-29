//! Local check of the Narratore's drafts, before the Custode.
//!
//! [`precheck`] rejects the answers that are obviously unusable (shape,
//! lengths, Italian names, closed lists, references to what exists, jobs
//! where there are no work stations), with a readable Italian reason that
//! the Narratore sends back to the model for its one retry. The meaning
//! (values against ingredients, loops of recipes, storage) is the Custode's
//! (`World::review` in `sim`), at the minute the draft is applied.
//!
//! Names are compared the way the Custode resolves them: case, accents,
//! apostrophes and extra spaces don't matter ([`normalize`]).

use std::collections::{BTreeMap, HashMap};

use crate::appearance::{Appearance, COLOURS, DETAILS, SHAPES};
use crate::panel::{Action, ELEMENT_KINDS, Element, MAX_ELEMENTS, Panel};
use crate::proposal::{Draft, Effect, Ingredient, Proposal};
use crate::sources::{ListSource, Source};
use crate::statistic::{
    Formula, MAX_SCALE, MAX_TERMS, MAX_THRESHOLDS, MAX_WEIGHT, Statistic, cycle,
};
use crate::summary::Catalog;

/// Most effects an event may have, unless configured otherwise.
pub const MAX_EFFECTS: usize = 3;
/// Most ingredients of an item or recipe.
pub const MAX_INGREDIENTS: usize = 4;
/// Most items a new job makes.
pub const MAX_MAKES: usize = 4;
/// Name of the example in the system prompt: copying it is not a novelty.
pub const EXAMPLE_NAME: &str = "Scialle di stracci";

/// Why a draft was rejected, in Italian (sent back to the model): the
/// Custode's own type.
pub use sim::Rejection;

fn reject<T>(reason: impl Into<String>) -> Result<T, Rejection> {
    Err(Rejection(reason.into()))
}

/// The key of a name: lowercase, without accents and apostrophes, single
/// spaces (the Custode's own rule).
pub use sim::custode::normalize;

/// What names exist: the catalog plus the novelties accepted so far.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Known {
    /// Key → item name (plurals included).
    items: HashMap<String, String>,
    recipes: HashMap<String, String>,
    jobs: HashMap<String, String>,
    carriages: HashMap<String, String>,
    /// Derived statistics: key → name.
    stats: HashMap<String, String>,
    /// Statistic key → keys of the statistics its formula reads.
    stat_deps: BTreeMap<String, Vec<String>>,
    /// Key → what already uses the name ("oggetto «verdura»").
    taken: HashMap<String, String>,
    /// (output, sorted inputs) as item keys → recipe name.
    shapes: HashMap<(String, Vec<String>), String>,
}

impl Known {
    pub fn from_catalog(catalog: &Catalog) -> Known {
        let mut k = Known::default();
        for item in &catalog.items {
            k.items.insert(normalize(item), item.clone());
            k.take(item, "l'oggetto");
        }
        for (plural, item) in &catalog.item_aliases {
            k.items.insert(normalize(plural), item.clone());
            k.take(plural, "l'oggetto");
        }
        for recipe in catalog.recipes.keys() {
            k.recipes.insert(normalize(recipe), recipe.clone());
            k.take(recipe, "la ricetta");
        }
        for job in catalog.jobs.keys() {
            k.jobs.insert(normalize(job), job.clone());
            k.take(job, "il lavoro");
        }
        for kind in &catalog.carriage_kinds {
            k.carriages.insert(normalize(kind), kind.clone());
            k.take(kind, "la carrozza");
        }
        for (name, output, inputs) in &catalog.recipe_shapes {
            let shape = k.shape(output, inputs.iter().map(String::as_str));
            k.shapes.insert(shape, name.clone());
        }
        k.take(EXAMPLE_NAME, "l'esempio del formato");
        k
    }

    /// The key of a recipe's shape: its output and sorted inputs, resolved.
    fn shape<'a>(
        &self,
        output: &str,
        inputs: impl Iterator<Item = &'a str>,
    ) -> (String, Vec<String>) {
        let resolve = |n: &str| self.item(n).map_or_else(|| normalize(n), normalize);
        let mut inputs: Vec<String> = inputs.map(resolve).collect();
        inputs.sort_unstable();
        (resolve(output), inputs)
    }

    fn take(&mut self, name: &str, what: &str) {
        self.taken
            .entry(normalize(name))
            .or_insert_with(|| format!("{what} «{name}»"));
    }

    /// Adds an accepted novelty: its name is taken, and a new item or job
    /// can be referenced by the next ones (as if the Custode had applied it).
    pub fn add(&mut self, proposal: &Proposal) {
        let name = proposal.name().to_string();
        match proposal {
            Proposal::NewItem { .. } => {
                self.items.insert(normalize(&name), name.clone());
            }
            Proposal::NewRecipe { output, inputs, .. } => {
                self.recipes.insert(normalize(&name), name.clone());
                let shape = self.shape(output, inputs.iter().map(|i| i.item.as_str()));
                self.shapes.insert(shape, name.clone());
            }
            Proposal::NewJob { .. } => {
                self.jobs.insert(normalize(&name), name.clone());
            }
            Proposal::Event { .. } => {}
            Proposal::Statistic(stat) => {
                let key = normalize(&name);
                self.stats.insert(key.clone(), name.clone());
                let deps = stat
                    .formula
                    .stat_refs()
                    .into_iter()
                    .map(normalize)
                    .collect();
                self.stat_deps.insert(key, deps);
            }
        }
        self.take(&name, &format!("la novità ({})", proposal.kind_name()));
    }

    pub fn stat(&self, name: &str) -> Option<&str> {
        self.stats.get(&normalize(name)).map(String::as_str)
    }

    pub fn item(&self, name: &str) -> Option<&str> {
        self.items.get(&normalize(name)).map(String::as_str)
    }

    pub fn job(&self, name: &str) -> Option<&str> {
        self.jobs.get(&normalize(name)).map(String::as_str)
    }

    pub fn carriage(&self, name: &str) -> Option<&str> {
        self.carriages.get(&normalize(name)).map(String::as_str)
    }

    pub fn recipe(&self, name: &str) -> Option<&str> {
        self.recipes.get(&normalize(name)).map(String::as_str)
    }

    fn list(map: &HashMap<String, String>) -> String {
        let mut v: Vec<&str> = map.values().map(String::as_str).collect();
        v.sort_unstable();
        v.dedup();
        v.join(", ")
    }
}

/// Checks `draft` against what exists. `max_effects`: most effects of an
/// event.
pub fn precheck(draft: &Draft, known: &Known, max_effects: usize) -> Result<(), Rejection> {
    text("il motivo", &draft.rationale, 10, 300)?;
    novelty(&draft.proposal, known, max_effects)?;
    if let Some(p) = &draft.panel {
        // The panel may refer to the novelty itself.
        let mut with_own = known.clone();
        with_own.add(&draft.proposal);
        panel(p, &with_own)?;
    }
    Ok(())
}

fn novelty(proposal: &Proposal, known: &Known, max_effects: usize) -> Result<(), Rejection> {
    match proposal {
        Proposal::NewItem {
            name,
            description,
            base_value,
            stack_limit,
            made_from,
            made_by_job,
            category,
            appearance: look,
        } => {
            new_name(name, known)?;
            text("la descrizione", description, 10, 240)?;
            range("il valore", *base_value, 1, 200)?;
            range("la pila", *stack_limit, 1, 50)?;
            job(made_by_job, known)?;
            if made_from.is_empty() && *category != crate::proposal::Category::Raw {
                return reject(format!(
                    "l'oggetto «{name}» non è una materia prima: servono degli ingredienti"
                ));
            }
            ingredients(made_from, known)?;
            match look {
                Some(a) => appearance(a),
                None => Ok(()),
            }
        }
        Proposal::NewRecipe {
            name,
            output,
            output_qty,
            inputs,
            job: maker,
            minutes,
        } => {
            new_name(name, known)?;
            if known.item(output).is_none() {
                return reject(format!(
                    "il prodotto «{output}» non esiste; oggetti esistenti: {}",
                    Known::list(&known.items)
                ));
            }
            range("la quantità prodotta", *output_qty, 1, 10)?;
            range("i minuti", *minutes, 10, 480)?;
            job(maker, known)?;
            if inputs.is_empty() {
                return reject("una ricetta ha bisogno di almeno un ingrediente");
            }
            if inputs
                .iter()
                .any(|i| known.item(&i.item) == known.item(output))
            {
                return reject(format!(
                    "«{output}» non può essere ingrediente di sé stesso"
                ));
            }
            ingredients(inputs, known)?;
            let shape = known.shape(output, inputs.iter().map(|i| i.item.as_str()));
            if let Some(same) = known.shapes.get(&shape) {
                return reject(format!(
                    "la ricetta «{same}» fa già «{output}» con gli stessi ingredienti: \
                     proponi qualcosa di davvero nuovo"
                ));
            }
            Ok(())
        }
        Proposal::NewJob {
            name,
            description,
            workplace_kind,
            makes,
            service,
        } => {
            new_name(name, known)?;
            text("la descrizione", description, 10, 240)?;
            carriage(workplace_kind, known)?;
            if let Some(kind) = sim::custode::sources::carriage_kind(workplace_kind)
                && !kind
                    .def()
                    .stations
                    .iter()
                    .any(|r| matches!(r.count, sim::defs::StationCount::Workers { .. }))
            {
                return reject(format!(
                    "in un {kind} non c'è una postazione di lavoro: scegli Serra, Mensa, \
                     Officina o Mercato"
                ));
            }
            let serves = service.is_some() && makes.is_empty();
            if !serves && (makes.is_empty() || makes.len() > MAX_MAKES) {
                return reject(format!(
                    "un lavoro deve produrre da 1 a {MAX_MAKES} oggetti esistenti, oppure \
                     produrre niente e dare un «servizio» (sazieta, energia o socialita)"
                ));
            }
            for m in makes {
                if known.item(m).is_none() && known.recipe(m).is_none() {
                    return reject(format!(
                        "«{m}» non è un oggetto esistente; oggetti esistenti: {}",
                        Known::list(&known.items)
                    ));
                }
            }
            Ok(())
        }
        Proposal::Event {
            title,
            description,
            effects,
        } => {
            text("il titolo", title, 3, 48)?;
            if let Some(what) = known.taken.get(&normalize(title)) {
                return reject(format!("il titolo «{title}» è già usato da {what}"));
            }
            text("la descrizione", description, 10, 300)?;
            if effects.is_empty() || effects.len() > max_effects {
                return reject(format!(
                    "un evento deve avere da 1 a {max_effects} effetti (ne ha {})",
                    effects.len()
                ));
            }
            for e in effects {
                effect(e, known)?;
            }
            Ok(())
        }
        Proposal::Statistic(stat) => statistic(stat, known),
    }
}

/// A fresh Italian name: 2–32 characters, at most 4 words, letters only, not
/// taken. "Italian" is a heuristic: no k/w/x/y, and the first word ends in a
/// vowel (Italian nouns almost always do).
fn new_name(name: &str, known: &Known) -> Result<(), Rejection> {
    text("il nome", name, 2, 32)?;
    if name.split_whitespace().count() > 4 {
        return reject(format!(
            "il nome «{name}» è troppo lungo: al massimo 4 parole"
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_alphabetic() || matches!(c, ' ' | '\'' | '’' | '-'))
    {
        return reject(format!(
            "il nome «{name}» deve contenere solo lettere, spazi e apostrofi"
        ));
    }
    let key = normalize(name);
    let first = key.split(' ').next().unwrap_or_default();
    let foreign = key.chars().any(|c| matches!(c, 'k' | 'w' | 'x' | 'y'));
    if foreign || !first.ends_with(['a', 'e', 'i', 'o', 'u']) {
        return reject(format!("il nome «{name}» non sembra italiano"));
    }
    if let Some(what) = known.taken.get(&key) {
        return reject(format!(
            "il nome «{name}» esiste già ({what}): scegli un nome nuovo"
        ));
    }
    Ok(())
}

fn text(what: &str, value: &str, min: usize, max: usize) -> Result<(), Rejection> {
    let n = value.trim().chars().count();
    if n < min || n > max {
        return reject(format!(
            "{what} deve avere da {min} a {max} caratteri (ne ha {n})"
        ));
    }
    Ok(())
}

fn range(what: &str, value: u32, min: u32, max: u32) -> Result<(), Rejection> {
    if !(min..=max).contains(&value) {
        return reject(format!("{what} deve essere tra {min} e {max} (è {value})"));
    }
    Ok(())
}

fn job(name: &str, known: &Known) -> Result<(), Rejection> {
    match known.job(name) {
        Some(_) => Ok(()),
        None => reject(format!(
            "il lavoro «{name}» non esiste; lavori esistenti: {}",
            Known::list(&known.jobs)
        )),
    }
}

fn carriage(name: &str, known: &Known) -> Result<(), Rejection> {
    match known.carriage(name) {
        Some(_) => Ok(()),
        None => reject(format!(
            "la carrozza «{name}» non esiste; tipi di carrozza: {}",
            Known::list(&known.carriages)
        )),
    }
}

fn ingredients(list: &[Ingredient], known: &Known) -> Result<(), Rejection> {
    if list.len() > MAX_INGREDIENTS {
        return reject(format!("al massimo {MAX_INGREDIENTS} ingredienti"));
    }
    let mut seen = Vec::new();
    for i in list {
        let Some(item) = known.item(&i.item) else {
            return reject(format!(
                "l'ingrediente «{}» non esiste; oggetti esistenti: {}",
                i.item,
                Known::list(&known.items)
            ));
        };
        if seen.contains(&item) {
            return reject(format!("l'ingrediente «{item}» è ripetuto"));
        }
        seen.push(item);
        range(&format!("la quantità di «{item}»"), i.qty, 1, 10)?;
    }
    Ok(())
}

fn effect(e: &Effect, known: &Known) -> Result<(), Rejection> {
    match e {
        Effect::Stock {
            carriage_kind,
            item,
            delta,
        } => {
            carriage(carriage_kind, known)?;
            if known.item(item).is_none() {
                return reject(format!(
                    "l'oggetto «{item}» dell'effetto non esiste; oggetti esistenti: {}",
                    Known::list(&known.items)
                ));
            }
            if *delta == 0 || delta.abs() > 50 {
                return reject(format!(
                    "la variazione di scorta deve essere tra -50 e 50 e non zero (è {delta})"
                ));
            }
        }
        Effect::Need {
            carriage_kind,
            delta,
            ..
        } => {
            if let Some(kind) = carriage_kind {
                carriage(kind, known)?;
            }
            if !delta.is_finite() || *delta == 0.0 || delta.abs() > 0.3 {
                return reject(format!(
                    "la variazione di un bisogno deve essere tra -0.3 e 0.3 e non zero (è {delta})"
                ));
            }
        }
    }
    Ok(())
}

/// A value from a closed list, or a rejection that lists it.
fn one_of(what: &str, value: &str, allowed: &[&str]) -> Result<(), Rejection> {
    if allowed.contains(&normalize(value).as_str()) {
        return Ok(());
    }
    reject(format!(
        "{what} «{value}» non esiste; valori ammessi: {}",
        allowed.join(", ")
    ))
}

fn appearance(a: &Appearance) -> Result<(), Rejection> {
    one_of("la forma", &a.shape, &SHAPES)?;
    one_of("il colore", &a.colour, &COLOURS)?;
    if a.detail().is_err() {
        one_of(
            "il dettaglio",
            a.detail.as_deref().unwrap_or_default(),
            &DETAILS,
        )?;
    }
    Ok(())
}

/// A name referred to by a source, a list or an action exists (in the
/// catalog or among the accepted novelties).
fn reference(kind: &str, name: &str, known: &Known) -> Result<(), Rejection> {
    let (found, what, list) = match kind {
        "oggetto" => (known.item(name).is_some(), "l'oggetto", &known.items),
        "lavoro" => (known.job(name).is_some(), "il lavoro", &known.jobs),
        "carrozza" => (
            known.carriage(name).is_some(),
            "il tipo di carrozza",
            &known.carriages,
        ),
        "statistica" => (known.stat(name).is_some(), "la statistica", &known.stats),
        "ricetta" => (
            known.recipe(name).is_some() || known.item(name).is_some(),
            "la ricetta",
            &known.recipes,
        ),
        _ => (false, "il nome", &known.items),
    };
    if found {
        return Ok(());
    }
    let listed = Known::list(list);
    reject(if listed.is_empty() {
        format!("{what} «{name}» non esiste (non ce ne sono ancora)")
    } else {
        format!("{what} «{name}» non esiste; esistenti: {listed}")
    })
}

fn source(s: &Source, known: &Known) -> Result<(), Rejection> {
    if let Source::Invalid(why) = s {
        return reject(why.clone());
    }
    for (kind, name) in s.references() {
        reference(kind, name, known)?;
    }
    Ok(())
}

fn finite(what: &str, v: f32) -> Result<(), Rejection> {
    if !v.is_finite() || v.abs() > MAX_SCALE {
        return reject(format!(
            "{what} deve essere un numero tra -{MAX_SCALE} e {MAX_SCALE}"
        ));
    }
    Ok(())
}

fn panel(p: &Panel, known: &Known) -> Result<(), Rejection> {
    text("il titolo del pannello", &p.title, 3, 40)?;
    if p.elements.is_empty() || p.elements.len() > MAX_ELEMENTS {
        return reject(format!(
            "un pannello deve avere da 1 a {MAX_ELEMENTS} elementi (ne ha {})",
            p.elements.len()
        ));
    }
    for e in &p.elements {
        match e {
            Element::Text { text: t } => text("il testo del pannello", t, 1, 160)?,
            Element::Value {
                label, source: s, ..
            } => {
                text("l'etichetta", label, 1, 32)?;
                source(s, known)?;
            }
            Element::Bar {
                label,
                source: s,
                min,
                max,
            } => {
                text("l'etichetta", label, 1, 32)?;
                source(s, known)?;
                finite("il minimo della barra", *min)?;
                finite("il massimo della barra", *max)?;
                if min >= max {
                    return reject(format!(
                        "la barra «{label}» ha il minimo ({min}) non sotto il massimo ({max})"
                    ));
                }
            }
            Element::List { label, source: s } => {
                text("l'etichetta", label, 1, 32)?;
                if let ListSource::Invalid(why) = s {
                    return reject(why.clone());
                }
                for (kind, name) in s.references() {
                    reference(kind, name, known)?;
                }
            }
            Element::Button { label, action } => {
                text("l'etichetta del pulsante", label, 1, 32)?;
                if let Action::Invalid(why) = action {
                    return reject(why.clone());
                }
                if let Some((kind, name)) = action.reference() {
                    reference(kind, name, known)?;
                }
            }
            Element::Unknown => {
                return reject(format!(
                    "tipo di elemento sconosciuto; tipi ammessi: {}",
                    ELEMENT_KINDS.join(", ")
                ));
            }
        }
    }
    Ok(())
}

fn statistic(stat: &Statistic, known: &Known) -> Result<(), Rejection> {
    new_name(&stat.name, known)?;
    text("la descrizione", &stat.description, 10, 240)?;
    if let Some(unit) = &stat.unit {
        text("l'unità", unit, 0, 12)?;
    }
    let [lo, hi] = stat.scale;
    finite("la scala", lo)?;
    finite("la scala", hi)?;
    if lo >= hi {
        return reject(format!(
            "la scala [{lo}, {hi}] deve andare da un minimo a un massimo più grande"
        ));
    }
    if let Formula::Invalid(why) = &stat.formula {
        return reject(why.clone());
    }
    let terms = stat.formula.terms();
    if terms.is_empty() || terms.len() > MAX_TERMS {
        return reject(format!(
            "una formula deve avere da 1 a {MAX_TERMS} termini (ne ha {})",
            terms.len()
        ));
    }
    let own = normalize(&stat.name);
    for t in terms {
        if !t.weight.is_finite() || t.weight == 0.0 || t.weight.abs() > MAX_WEIGHT {
            return reject(format!(
                "il peso {} deve essere tra -{MAX_WEIGHT} e {MAX_WEIGHT} e non zero",
                t.weight
            ));
        }
        // Reading itself is a cycle, not a missing name.
        if matches!(&t.source, Source::Stat { name } if normalize(name) == own) {
            return reject(format!(
                "la statistica «{}» non può leggere sé stessa",
                stat.name
            ));
        }
        source(&t.source, known)?;
    }
    if let Some(path) = cycle(&stat.name, &stat.formula.stat_refs(), &known.stat_deps) {
        return reject(format!("ciclo tra statistiche: {}", path.join(" → ")));
    }
    if stat.thresholds.len() > MAX_THRESHOLDS {
        return reject(format!("al massimo {MAX_THRESHOLDS} soglie"));
    }
    for th in &stat.thresholds {
        text("il testo di una soglia", &th.text, 3, 100)?;
        match (th.below, th.above) {
            (Some(v), None) | (None, Some(v)) => finite("una soglia", v)?,
            _ => return reject("ogni soglia ha «sotto» oppure «sopra» (uno solo) e un «testo»"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proposal::{Category, Need};

    fn known() -> Known {
        Known::from_catalog(&Catalog::of_sim())
    }

    fn item(name: &str, job: &str, from: &[(&str, u32)]) -> Draft {
        Draft {
            panel: None,
            rationale: "Serve qualcosa contro il freddo.".into(),
            proposal: Proposal::NewItem {
                name: name.into(),
                description: "Una cosa utile per il treno.".into(),
                category: Category::Durable,
                base_value: 10,
                stack_limit: 5,
                made_from: from
                    .iter()
                    .map(|&(i, q)| Ingredient {
                        item: i.into(),
                        qty: q,
                    })
                    .collect(),
                made_by_job: job.into(),
                appearance: None,
            },
        }
    }

    #[test]
    fn names_are_normalized() {
        assert_eq!(normalize("  Tè "), "te");
        assert_eq!(normalize("Filtro D'Acqua"), "filtro d acqua");
        assert_eq!(normalize("CAFFÈ  nero"), "caffe nero");
    }

    #[test]
    fn a_good_item_passes() {
        let k = known();
        // Case, accents and plurals don't matter in references.
        let d = item("Borraccia", "OPERAIO", &[("Metallo", 1), ("vestiti", 1)]);
        assert_eq!(precheck(&d, &k, MAX_EFFECTS), Ok(()));
    }

    #[test]
    fn bad_drafts_are_rejected_with_a_reason() {
        let k = known();
        let why = |d: &Draft| precheck(d, &k, MAX_EFFECTS).unwrap_err().0;
        assert!(why(&item("Verdura", "operaio", &[("metallo", 1)])).contains("esiste già"));
        assert!(why(&item("Tè", "cuoco", &[("erbe", 1)])).contains("esiste già"));
        assert!(why(&item(EXAMPLE_NAME, "operaio", &[("tessuto", 1)])).contains("esempio"));
        assert!(why(&item("Borraccia", "alchimista", &[("metallo", 1)])).contains("lavoro"));
        assert!(why(&item("Borraccia", "operaio", &[("oro", 1)])).contains("«oro»"));
        assert!(why(&item("Borraccia", "operaio", &[("metallo", 11)])).contains("tra 1 e 10"));
        assert!(why(&item("Water Bottle", "operaio", &[("metallo", 1)])).contains("italiano"));
        assert!(why(&item("Borraccia", "operaio", &[])).contains("ingredienti"));
        let event = Draft {
            panel: None,
            rationale: "Serve una scossa al treno.".into(),
            proposal: Proposal::Event {
                title: "La gelata".into(),
                description: "Il gelo entra dalle finestre rotte.".into(),
                effects: vec![
                    Effect::Need {
                        carriage_kind: None,
                        need: Need::Energy,
                        delta: -0.1,
                    };
                    4
                ],
            },
        };
        assert!(why(&event).contains("da 1 a 3 effetti"));
    }

    #[test]
    fn accepted_novelties_are_taken_and_usable() {
        let mut k = known();
        let d = item("Borraccia", "operaio", &[("metallo", 1)]);
        k.add(&d.proposal);
        assert!(
            precheck(&d, &k, MAX_EFFECTS)
                .unwrap_err()
                .0
                .contains("novità")
        );
        let recipe = Draft {
            panel: None,
            rationale: "Più borracce per tutti.".into(),
            proposal: Proposal::NewRecipe {
                name: "battere una borraccia".into(),
                output: "borraccia".into(),
                output_qty: 2,
                inputs: vec![Ingredient {
                    item: "rottame".into(),
                    qty: 2,
                }],
                job: "operaio".into(),
                minutes: 60,
            },
        };
        assert_eq!(precheck(&recipe, &k, MAX_EFFECTS), Ok(()));
        // Metal from scrap exists already ("fondere il rottame"), whatever
        // the name and the quantities.
        let copy = Draft {
            panel: None,
            rationale: "Il metallo è finito.".into(),
            proposal: Proposal::NewRecipe {
                name: "Recupero Metallo".into(),
                output: "Metallo".into(),
                output_qty: 1,
                inputs: vec![Ingredient {
                    item: "rottami".into(),
                    qty: 2,
                }],
                job: "operaio".into(),
                minutes: 40,
            },
        };
        let why = precheck(&copy, &k, MAX_EFFECTS).unwrap_err().0;
        assert!(why.contains("fondere il rottame"), "{why}");
    }
}
