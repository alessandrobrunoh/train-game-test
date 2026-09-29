//! `MockModel`: un [`ChoiceModel`] finto, deterministico e senza pesi.
//!
//! Legge dal contesto italiano di `World::npc_context` i bisogni (i numeri tra
//! parentesi dopo "Sazietà", "energia", "socialità"), l'ora, se è notte, se il
//! turno di lavoro è in corso e cosa manca all'NPC; poi dà un punteggio a ogni
//! opzione cercando parole chiave nella descrizione ("mangia", "dormire",
//! "lavora", "chiacchiera", "compra", "va in"...) e restituisce il softmax dei
//! punteggi. Serve a provare `LayaBrain` (anche con una latenza simulata) e
//! come termine di paragone nella valutazione.

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
        let ctx = Context::parse(&query.context);
        let scores: Vec<f32> = query.options.iter().map(|o| ctx.score(o)).collect();
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
