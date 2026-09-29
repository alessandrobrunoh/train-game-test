//! `MockModel`: un [`ChoiceModel`] finto, deterministico e senza pesi.
//!
//! Legge dal contesto italiano di `World::npc_context` i bisogni (i numeri tra
//! parentesi dopo "Sazietà", "energia", "socialità"), l'ora, se è notte, se il
//! turno di lavoro è in corso e cosa manca all'NPC; poi dà un punteggio a ogni
//! opzione cercando parole chiave nella descrizione ("mangia", "dormire",
//! "lavora", "chiacchiera", "compra", "va in"...) e restituisce il softmax dei
//! punteggi. Serve a provare `LayaBrain` (anche con una latenza simulata) e
//! come termine di paragone nella valutazione.
//!
//! Le deliberazioni (domande che non sono "Quale azione sceglie…") hanno una
//! loro euristica, sempre a parole chiave: carattere ("audace", "poco
//! scrupoloso"...), affinità, differenza d'età, figli, razioni a persona,
//! gettoni e prezzo, chi potrebbe aiutare (vedi [`Situation`]).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use crate::model::{ChoiceAnswer, ChoiceModel, ChoiceQuery};

/// Modello euristico a parole chiave, con latenza configurabile.
#[derive(Clone, Debug)]
pub struct MockModel {
    /// Attesa per ogni lotto (simula il forward pass).
    pub latency: Duration,
    /// Attesa aggiuntiva per ogni riga del lotto.
    pub per_row: Duration,
    /// Temperatura del softmax: più alta, risposte meno sicure.
    pub temperature: f32,
    rows: Arc<AtomicUsize>,
    batches: Arc<AtomicUsize>,
}

impl Default for MockModel {
    fn default() -> Self {
        Self::new()
    }
}

impl MockModel {
    pub fn new() -> Self {
        Self {
            latency: Duration::ZERO,
            per_row: Duration::ZERO,
            temperature: 0.2,
            rows: Arc::default(),
            batches: Arc::default(),
        }
    }

    pub fn with_latency(mut self, per_batch: Duration, per_row: Duration) -> Self {
        self.latency = per_batch;
        self.per_row = per_row;
        self
    }

    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = temperature;
        self
    }

    /// Contatore delle righe elaborate (condiviso con i cloni, anche dopo che
    /// il modello è passato al worker).
    pub fn rows_counter(&self) -> Arc<AtomicUsize> {
        self.rows.clone()
    }

    /// Contatore dei lotti elaborati.
    pub fn batches_counter(&self) -> Arc<AtomicUsize> {
        self.batches.clone()
    }

    /// Probabilità per le opzioni di una domanda.
    pub fn answer(&self, query: &ChoiceQuery) -> ChoiceAnswer {
        let scores: Vec<f32> = if is_deliberation(query) {
            let s = Situation::parse(&query.context);
            query.options.iter().map(|o| s.score(o)).collect()
        } else {
            let ctx = Context::parse(&query.context);
            query.options.iter().map(|o| ctx.score(o)).collect()
        };
        ChoiceAnswer {
            probabilities: softmax(&scores, self.temperature),
        }
    }
}

impl ChoiceModel for MockModel {
    fn name(&self) -> &str {
        "mock (parole chiave)"
    }

    fn predict_batch(&mut self, queries: &[ChoiceQuery]) -> Result<Vec<ChoiceAnswer>, String> {
        let wait = self.latency + self.per_row * queries.len() as u32;
        if !wait.is_zero() {
            thread::sleep(wait);
        }
        self.rows.fetch_add(queries.len(), Ordering::Relaxed);
        self.batches.fetch_add(1, Ordering::Relaxed);
        Ok(queries.iter().map(|q| self.answer(q)).collect())
    }
}

fn softmax(scores: &[f32], temperature: f32) -> Vec<f32> {
    let t = temperature.max(1e-3);
    let max = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exp: Vec<f32> = scores.iter().map(|s| ((s - max) / t).exp()).collect();
    let sum: f32 = exp.iter().sum();
    exp.iter().map(|e| e / sum).collect()
}

/// Quello che il mock capisce del contesto.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Context {
    hunger: f32,
    energy: f32,
    social: f32,
    hour: u32,
    night: bool,
    in_shift: bool,
    wants_tool: bool,
    wants_clothes: bool,
}

/// Il numero tra parentesi dopo `label`, es. "energia bassa (0.31)" → 0.31.
fn need_after(text: &str, label: &str) -> Option<f32> {
    let rest = &text[text.find(label)? + label.len()..];
    let open = rest.find('(')?;
    let close = rest[open..].find(')')? + open;
    rest[open + 1..close].trim().parse().ok()
}

/// La prima ora "HH:MM" del testo.
fn first_hour(text: &str) -> Option<u32> {
    let bytes = text.as_bytes();
    (2..bytes.len()).find_map(|i| {
        (bytes[i] == b':'
            && bytes[i - 2].is_ascii_digit()
            && bytes[i - 1].is_ascii_digit()
            && bytes.get(i + 1).is_some_and(u8::is_ascii_digit))
        .then(|| text[i - 2..i].parse().ok())
        .flatten()
    })
}

impl Context {
    fn parse(text: &str) -> Context {
        Context {
            hunger: need_after(text, "Sazietà").unwrap_or(1.0),
            energy: need_after(text, "energia").unwrap_or(1.0),
            social: need_after(text, "socialità").unwrap_or(1.0),
            hour: first_hour(text).unwrap_or(12),
            night: text.contains("(notte)"),
            in_shift: text.contains("(in corso)"),
            wants_tool: text.contains("gli servirebbe"),
            wants_clothes: text.contains("nessun vestito"),
        }
    }

    /// Punteggio di un'opzione (descrizione in italiano).
    fn score(&self, option: &str) -> f32 {
        let o = option.to_lowercase();
        let travel = o.starts_with("va in");
        let has = |k: &str| o.contains(k);
        let hunger = 1.0 - self.hunger;
        let tired = 1.0 - self.energy;
        let lonely = 1.0 - self.social;
        let meal_time = matches!(self.hour, 6 | 7 | 12 | 13 | 19 | 20);
        let base = if has("mangia") {
            if hunger < 0.3 && !(meal_time && hunger > 0.15) {
                -0.5
            } else {
                2.0 * hunger * hunger + if meal_time { 0.3 } else { 0.0 }
            }
        } else if has("dormi") || has("pisolino") {
            if self.night {
                0.4 + 1.2 * tired
            } else if tired > 0.7 {
                1.5 * tired * tired * tired
            } else {
                -0.5
            }
        } else if has("lavora") || has("lavorare") {
            if self.in_shift { 0.9 } else { -0.8 }
        } else if has("chiacchier") {
            if self.night {
                0.2 * lonely
            } else {
                1.0 * lonely
            }
        } else if has("compra") || has("acquisti") {
            let tool = self.wants_tool && has("attrezzo");
            let clothes = self.wants_clothes && has("vestito");
            if tool {
                0.85
            } else if clothes {
                0.55
            } else {
                0.2
            }
        } else if has("oziare") {
            0.15
        } else {
            0.0
        };
        // Spostarsi costa un po' rispetto a fare la stessa cosa qui.
        let travel_cost = if travel { 0.12 } else { 0.0 };
        let home = if travel && has("casa") && self.night {
            0.1
        } else {
            0.0
        };
        base - travel_cost + home
    }
}

// --- Deliberazioni -------------------------------------------------------------

/// Le domande delle azioni di tutti i giorni cominciano così (vedi
/// `brain::build_query`); le altre sono deliberazioni.
fn is_deliberation(query: &ChoiceQuery) -> bool {
    !query.instructions.starts_with("Quale azione")
}

/// Il primo numero (anche con la virgola decimale col punto) dopo `label`.
fn number_after(text: &str, label: &str) -> Option<f32> {
    let rest = &text[text.find(label)? + label.len()..];
    let start = rest.find(|c: char| c.is_ascii_digit())?;
    let tail = &rest[start..];
    let end = tail
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(tail.len());
    tail[..end].trim_end_matches('.').parse().ok()
}

/// Il numero subito prima di `label`, es. "0.8 razioni a persona" → 0.8.
fn number_before(text: &str, label: &str) -> Option<f32> {
    let head = &text[..text.find(label)?];
    let head = head.trim_end();
    let start = head
        .rfind(|c: char| !(c.is_ascii_digit() || c == '.'))
        .map_or(0, |i| i + 1);
    head[start..].parse().ok()
}

/// "Ha 2 figli" o "Ha già 3 figli" → il numero.
fn count_of_children(text: &str) -> Option<f32> {
    ["Ha già ", "Ha "].iter().find_map(|prefix| {
        text.match_indices(prefix).find_map(|(at, _)| {
            let rest = &text[at + prefix.len()..];
            let digits = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            (digits > 0 && rest[digits..].starts_with(" figli"))
                .then(|| rest[..digits].parse().ok())
                .flatten()
        })
    })
}

/// Quello che il mock capisce del contesto di una deliberazione.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Situation {
    honesty: f32,
    boldness: f32,
    affinity: f32,
    age_gap: f32,
    kids: f32,
    /// Razioni a persona nelle Mense.
    food: Option<f32>,
    tokens: Option<f32>,
    price: Option<f32>,
    helper_tokens: Option<f32>,
    hunger: f32,
}

impl Situation {
    fn parse(text: &str) -> Situation {
        let honesty = if text.contains("poco scrupolos") {
            0.2
        } else if text.contains("molto onest") {
            0.85
        } else {
            0.5
        };
        let boldness = if text.contains("audace") {
            0.85
        } else if text.contains("prudente") {
            0.2
        } else {
            0.5
        };
        let age_gap = if text.contains("stessa età") {
            0.0
        } else if text.contains("differenza d'età è di un anno") {
            1.0
        } else {
            number_after(text, "differenza d'età è di").unwrap_or(0.0)
        };
        let kids = if text.contains("Non ha ancora figli") {
            0.0
        } else if text.contains("Ha già un figlio") || text.contains("Ha un figlio") {
            1.0
        } else {
            count_of_children(text).unwrap_or(0.0)
        };
        Situation {
            honesty,
            boldness,
            affinity: number_after(text, "(affinità").unwrap_or(0.5),
            age_gap,
            kids,
            food: number_before(text, "razioni a persona"),
            tokens: number_after(text, "ne ha solo"),
            price: number_before(text, "gettoni, ma"),
            helper_tokens: text
                .find("Potrebbe chiedere aiuto a")
                .and_then(|at| number_after(&text[at..], "che ha")),
            hunger: number_after(text, "(sazietà")
                .or_else(|| need_after(text, "Sazietà"))
                .unwrap_or(1.0),
        }
    }

    /// Punteggio di un'opzione di deliberazione (descrizione in italiano).
    fn score(&self, option: &str) -> f32 {
        let o = option.to_lowercase();
        let has = |k: &str| o.contains(k);
        let bold = self.boldness - 0.5;
        if has("accetta e diventa") {
            let gap = (self.age_gap - 5.0).max(0.0);
            1.6 * (self.affinity - 0.55) - 0.1 * gap + 0.3 * bold - 0.15 * self.kids
        } else if has("rifiuta la proposta") {
            0.8 * (0.6 - self.affinity) + 0.08 * (self.age_gap - 5.0).max(0.0)
        } else if has("tempo per pensarci") {
            0.05 - 0.4 * (self.affinity - 0.65).abs()
        } else if has("prova ad avere un figlio") {
            let food = self.food.map_or(0.0, |f| (f - 1.0).clamp(-1.0, 1.0));
            0.45 - 0.2 * self.kids + 0.4 * food + 0.4 * (self.affinity - 0.6)
        } else if has("aspetta ancora") {
            0.1
        } else if o.starts_with("ruba") {
            let short = match (self.tokens, self.price) {
                (Some(t), Some(p)) if p > 0.0 => (1.0 - t / p).clamp(0.0, 1.0),
                _ => 0.5,
            };
            1.4 * short * (1.0 - self.honesty) * (0.5 + self.boldness) - 0.35
        } else if has("rinuncia per ora") {
            let short = match (self.tokens, self.price) {
                (Some(t), Some(p)) if p > 0.0 => (1.0 - t / p).clamp(0.0, 1.0),
                _ => 0.5,
            };
            0.5 * self.honesty + 0.3 * (1.0 - short) - 0.1
        } else if has("chiede qualche gettone") {
            let family = ["madre", "padre", "figli", "sorella", "fratello", "compagn"]
                .iter()
                .any(|w| o.contains(w));
            let rich = self.helper_tokens.map_or(0.0, |t| (t / 50.0).min(1.0));
            0.15 + if family { 0.25 } else { 0.0 } + 0.2 * rich - 0.15 * bold
        } else if has("si unisce alla protesta") {
            let childless = if self.kids == 0.0 { 0.15 } else { 0.0 };
            0.9 * bold + childless + 0.5 * (0.5 - self.hunger).max(0.0)
        } else if has("accetta la decisione") || has("sopporta") {
            -0.6 * bold
        } else if has("si rassegna") {
            -0.6 * bold - 0.1
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(context: &str, options: &[&str]) -> ChoiceQuery {
        ChoiceQuery {
            context: context.to_string(),
            instructions: "Quale azione sceglie Anna adesso?".into(),
            options: options.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn parses_needs_and_hour() {
        let ctx = Context::parse(
            "Giorno 2 13:10 (giorno). Anna ... Lavora come cuoco in X, turno 06:00-15:00 (in corso). \
             Possiede 3 gettoni, nessun attrezzo (gli servirebbe) e nessun vestito caldo. \
             Sazietà critica (0.05), energia media (0.60), socialità buona (0.90).",
        );
        assert_eq!(ctx.hour, 13);
        assert!((ctx.hunger - 0.05).abs() < 1e-6);
        assert!((ctx.energy - 0.6).abs() < 1e-6);
        assert!((ctx.social - 0.9).abs() < 1e-6);
        assert!(ctx.in_shift && ctx.wants_tool && ctx.wants_clothes && !ctx.night);
    }

    #[test]
    fn hungry_npc_eats_and_probabilities_sum_to_one() {
        let q = query(
            "Giorno 2 12:30 (giorno). Sazietà critica (0.05), energia buona (0.90), socialità buona (0.90).",
            &[
                "resta a oziare per 12 minuti",
                "mangia una razione in Mensa",
                "chiacchiera con Marco (20 min)",
            ],
        );
        let a = MockModel::new().answer(&q);
        assert_eq!(a.best().unwrap().0, 1);
        assert!((a.probabilities.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        // Temperatura alta: quasi uniforme, poco sicuro.
        let flat = MockModel::new().with_temperature(100.0).answer(&q);
        assert!(flat.best().unwrap().1 < 0.4);
    }

    fn deliberation(context: &str, question: &str, options: &[&str]) -> ChoiceQuery {
        ChoiceQuery {
            context: context.to_string(),
            instructions: question.to_string(),
            options: options.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn parses_the_situation_of_a_deliberation() {
        let s = Situation::parse(
            "Giorno 3 10:00 (giorno). Anna ... Ha 2 figli. Possiede 3 gettoni. Sazietà media (0.60). Di carattere è poco scrupolosa e audace.\nAl Mercato (carrozza 7) c'è un vestito a 12 gettoni, ma Anna ne ha solo 3. Potrebbe chiedere aiuto a Luca Neri (padre), che ha 40 gettoni.",
        );
        assert_eq!((s.honesty, s.boldness, s.kids), (0.2, 0.85, 2.0));
        assert_eq!(
            (s.tokens, s.price, s.helper_tokens),
            (Some(3.0), Some(12.0), Some(40.0))
        );
        let s = Situation::parse(
            "L'amministrazione oggi permetterebbe una nascita: ci sono cuccette libere e nelle Mense ci sono 0.4 razioni a persona. Ha già 3 figli: A (4 anni), B (2 anni), C (1 anni). Il legame con Luca è forte (affinità 0.71).",
        );
        assert_eq!((s.food, s.kids, s.affinity), (Some(0.4), 3.0, 0.71));
        let s = Situation::parse(
            "Il loro legame è fortissimo (affinità 0.95); la differenza d'età è di 22 anni.",
        );
        assert_eq!((s.affinity, s.age_gap), (0.95, 22.0));
    }

    #[test]
    fn obvious_deliberations_have_obvious_answers() {
        let m = MockModel::new();
        let couple = |ctx: &str| {
            m.answer(&deliberation(
                ctx,
                "Marta accetta la proposta di Luca di diventare una coppia?",
                &[
                    "accetta e diventa la compagna di Luca",
                    "rifiuta la proposta di Luca",
                    "chiede a Luca un po' di tempo per pensarci",
                ],
            ))
            .best()
            .unwrap()
            .0
        };
        assert_eq!(
            couple("Di carattere è onesta e audace. (affinità 0.97); hanno la stessa età."),
            0
        );
        assert_eq!(
            couple(
                "Di carattere è onesta e prudente. (affinità 0.30); la differenza d'età è di 25 anni."
            ),
            1
        );
        let theft = m.answer(&deliberation(
            "Di carattere è poco scrupoloso e audace. c'è un attrezzo a 20 gettoni, ma Leo ne ha solo 0.",
            "Leo non può permettersi un attrezzo: che cosa fa?",
            &["ruba un attrezzo al Mercato", "rinuncia per ora e mette da parte i gettoni"],
        ));
        assert_eq!(theft.best().unwrap().0, 0);
        let protest = m.answer(&deliberation(
            "Di carattere è onesto e audace. È in coppia con Eva.",
            "Leo protesta contro l'amministrazione, che gli ha negato un figlio?",
            &[
                "si unisce alla protesta in Mensa",
                "accetta la decisione dell'amministrazione",
            ],
        ));
        assert_eq!(protest.best().unwrap().0, 0);
    }

    #[test]
    fn counts_rows_and_batches() {
        let mut m = MockModel::new();
        let rows = m.rows_counter();
        let q = query(
            "Sazietà buona (0.9)",
            &[
                "resta a oziare per 10 minuti",
                "fa un pisolino di 30 minuti",
            ],
        );
        let out = m.predict_batch(&[q.clone(), q]).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(rows.load(Ordering::Relaxed), 2);
        assert_eq!(m.batches_counter().load(Ordering::Relaxed), 1);
    }
}
