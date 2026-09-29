//! Deliberazioni per Laya: domande, miscela con la regola, statistiche e registro.
//!
//! Le deliberazioni del `sim` (proposte di coppia, figli, furti, proteste)
//! sono rare, descritte a parole e con una scadenza di qualche ora di gioco:
//! il caso ideale per un modello "a scelta" come Laya. `LayaBrain` le manda
//! al modello con la precedenza sulle azioni di tutti i giorni (vedi
//! `brain.rs`); qui ci sono i pezzi puri, usati anche dalla valutazione:
//! - [`deliberation_query`]: la domanda (contesto, domanda, descrizioni delle opzioni);
//! - [`blend`]: miscela facoltativa con le probabilità della regola del `sim`;
//! - [`DeliberationStats`], [`DeliberationInfo`]: numeri e stato per l'interfaccia;
//! - [`DeliberationLog`]: il registro per rigiocare una partita ([`crate::ReplayBrain`]).

use std::time::{Duration, Instant};

use sim::{Deliberation, DeliberationId, DeliberationKind, GameTime, NpcId};

use crate::model::ChoiceQuery;

/// Numero di tipi di deliberazione (per gli array per tipo).
pub const KINDS: usize = DeliberationKind::COUNT;

/// La domanda al modello: il contesto della deliberazione, la sua domanda e
/// le descrizioni delle opzioni (senza lettere: con `laya-multilingual`
/// rendono meglio).
pub fn deliberation_query(d: &Deliberation) -> ChoiceQuery {
    ChoiceQuery {
        context: d.context.clone(),
        instructions: d.question.clone(),
        options: d.options.iter().map(|o| o.description.clone()).collect(),
    }
}

/// `(1 - w) · modello + w · regola`, rinormalizzato. `w` = 0: solo il
/// modello; se le lunghezze non coincidono resta il modello.
pub fn blend(model: &[f32], rule: &[f32], prior_weight: f32) -> Vec<f32> {
    let w = if prior_weight.is_finite() {
        prior_weight.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if w == 0.0 || rule.len() != model.len() {
        return model.to_vec();
    }
    let mixed: Vec<f32> = model
        .iter()
        .zip(rule)
        .map(|(m, r)| ((1.0 - w) * m + w * r).max(0.0))
        .collect();
    let sum: f32 = mixed.iter().sum();
    if sum > 0.0 && sum.is_finite() {
        mixed.iter().map(|p| p / sum).collect()
    } else {
        model.to_vec()
    }
}

/// Indice e valore del massimo (a pari valore vince l'indice più basso).
pub fn argmax(values: &[f32]) -> Option<(usize, f32)> {
    values
        .iter()
        .copied()
        .enumerate()
        .fold(None, |best, (i, v)| match best {
            Some((_, b)) if b >= v => best,
            _ if v.is_nan() => best,
            _ => Some((i, v)),
        })
}

/// A che punto è una deliberazione per il modello.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeliberationStatus {
    /// Domanda al modello in corso.
    Pending,
    /// Risposta data al mondo.
    Applied,
    /// Risposta sotto la soglia: decide la regola alla scadenza.
    LowConfidence,
    /// La scadenza è passata prima della risposta: ha deciso la regola.
    Late,
    /// Chiusa prima della risposta per un altro motivo (annullata).
    Closed,
    /// Errore del modello.
    Failed,
}

impl DeliberationStatus {
    pub fn name(self) -> &'static str {
        match self {
            DeliberationStatus::Pending => "ci pensa",
            DeliberationStatus::Applied => "risposta data",
            DeliberationStatus::LowConfidence => "poco sicuro: decidono le regole",
            DeliberationStatus::Late => "in ritardo: hanno deciso le regole",
            DeliberationStatus::Closed => "chiusa prima della risposta",
            DeliberationStatus::Failed => "errore del modello",
        }
    }
}

/// Quello che il cervello sa di una deliberazione (per l'ispettore).
#[derive(Clone, Debug, PartialEq)]
pub struct DeliberationInfo {
    pub id: DeliberationId,
    pub npc: NpcId,
    pub kind: DeliberationKind,
    pub status: DeliberationStatus,
    pub asked: GameTime,
    pub deadline: GameTime,
    /// Probabilità della regola quando si è aperta (stesso ordine delle opzioni).
    pub rule: Vec<f32>,
    /// Probabilità del modello, quando ha risposto.
    pub model: Option<Vec<f32>>,
    /// Quelle usate per decidere (modello miscelato con la regola).
    pub blended: Option<Vec<f32>>,
    /// Probabilità della scelta migliore (di `blended`).
    pub confidence: Option<f32>,
    /// Istante (reale) dell'invio al modello.
    pub sent: Instant,
    /// Tempo reale tra invio e risposta.
    pub latency: Option<Duration>,
}

impl DeliberationInfo {
    /// L'opzione che il modello sceglierebbe (dopo la miscela).
    pub fn model_choice(&self) -> Option<usize> {
        self.blended.as_deref().and_then(argmax).map(|(i, _)| i)
    }
}

/// Contatori delle deliberazioni di `LayaBrain`, per tipo
/// ([`DeliberationKind::index`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeliberationStats {
    /// Mandate al modello.
    pub asked: [u64; KINDS],
    /// Risposte arrivate (valide).
    pub answered: [u64; KINDS],
    /// Risposte date al mondo.
    pub applied: [u64; KINDS],
    /// Risposte sotto la soglia di confidenza (decide la regola).
    pub low_confidence: [u64; KINDS],
    /// Scadenza passata prima della risposta (decide la regola).
    pub late: [u64; KINDS],
    /// Chiuse prima della risposta per altri motivi.
    pub closed: [u64; KINDS],
    pub failed: u64,
    /// Risposte con la stessa scelta più probabile della regola.
    pub agreed: [u64; KINDS],
    pub latency_total: Duration,
    pub latency_max: Duration,
    /// Istogramma della confidenza (10 fasce da 0.1).
    pub confidence_hist: [u64; 10],
}

impl DeliberationStats {
    fn sum(values: &[u64; KINDS]) -> u64 {
        values.iter().sum()
    }

    pub fn asked_total(&self) -> u64 {
        Self::sum(&self.asked)
    }

    pub fn answered_total(&self) -> u64 {
        Self::sum(&self.answered)
    }

    pub fn applied_total(&self) -> u64 {
        Self::sum(&self.applied)
    }

    pub fn low_confidence_total(&self) -> u64 {
        Self::sum(&self.low_confidence)
    }

    pub fn late_total(&self) -> u64 {
        Self::sum(&self.late)
    }

    /// Frazione di risposte d'accordo con la scelta più probabile della
    /// regola, in totale o per un tipo.
    pub fn agreement(&self, kind: Option<usize>) -> Option<f32> {
        let (agreed, answered) = match kind {
            Some(k) => (self.agreed[k], self.answered[k]),
            None => (Self::sum(&self.agreed), self.answered_total()),
        };
        (answered > 0).then(|| agreed as f32 / answered as f32)
    }

    pub fn avg_latency(&self) -> Option<Duration> {
        let n = self.answered_total() + self.failed;
        (n > 0).then(|| self.latency_total / n as u32)
    }
}

/// Una risposta data al mondo, registrata per il replay.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeliberationLogEntry {
    /// Tick in cui è stata data.
    pub time: GameTime,
    /// Quante chiamate a `deliberations_resolved` c'erano state prima nello
    /// stesso tick (un tick può fare più giri, vedi `World::tick`).
    pub round: u8,
    pub id: DeliberationId,
    pub choice: usize,
    pub confidence: f32,
}

/// Il registro delle deliberazioni di una partita (vedi `LayaConfig::record_log`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeliberationLog {
    /// Da quando il cervello rispondeva alle deliberazioni (`true`) o le
    /// lasciava alla regola: cambia quando il modello si carica o si spegne.
    pub modes: Vec<(GameTime, bool)>,
    pub answers: Vec<DeliberationLogEntry>,
}

impl DeliberationLog {
    pub fn is_empty(&self) -> bool {
        self.modes.is_empty() && self.answers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blending_mixes_and_renormalizes() {
        let m = [0.9, 0.1];
        let r = [0.2, 0.8];
        assert_eq!(blend(&m, &r, 0.0), m.to_vec());
        let b = blend(&m, &r, 0.5);
        assert!((b[0] - 0.55).abs() < 1e-6 && (b[1] - 0.45).abs() < 1e-6);
        assert_eq!(blend(&m, &r, 1.0), r.to_vec());
        // Lunghezze diverse: resta il modello.
        assert_eq!(blend(&m, &[1.0], 0.5), m.to_vec());
    }

    #[test]
    fn argmax_prefers_the_first_on_ties() {
        assert_eq!(argmax(&[0.2, 0.5, 0.5]), Some((1, 0.5)));
        assert_eq!(argmax(&[]), None);
        assert_eq!(argmax(&[f32::NAN, 0.1]), Some((1, 0.1)));
    }
}
