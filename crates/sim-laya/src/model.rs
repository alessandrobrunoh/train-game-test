//! Contratto tra il cervello e il modello di scelta.

/// Una domanda a scelta singola: in quale opzione si trova meglio lo stato?
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ChoiceQuery {
    /// Stato del mondo dal punto di vista dell'NPC (vedi `World::npc_context`).
    pub context: String,
    /// La domanda, ad esempio "Quale azione sceglie Marta adesso?".
    pub instructions: String,
    /// Descrizioni delle opzioni, nell'ordine in cui vanno restituite le probabilità.
    pub options: Vec<String>,
}

/// Risposta a una [`ChoiceQuery`].
#[derive(Clone, Debug, PartialEq)]
pub struct ChoiceAnswer {
    /// Una probabilità per opzione, nello stesso ordine di `ChoiceQuery::options`;
    /// sommano a 1.
    pub probabilities: Vec<f32>,
}

impl ChoiceAnswer {
    /// Indice dell'opzione più probabile e la sua probabilità.
    pub fn best(&self) -> Option<(usize, f32)> {
        self.probabilities
            .iter()
            .copied()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(&b.1))
    }
}

/// Un modello che risponde a domande a scelta singola, a lotti.
///
/// Gira su un thread dedicato (vedi `LayaBrain`): può essere lento e bloccante,
/// ma non deve mai toccare il `World`. Deve rispondere con una `ChoiceAnswer`
/// per ogni query, nello stesso ordine.
pub trait ChoiceModel: Send + 'static {
    /// Nome leggibile, per log e interfaccia (es. "laya-multilingual").
    fn name(&self) -> &str;

    /// Risponde a un lotto di domande in un colpo solo (un forward pass).
    fn predict_batch(&mut self, queries: &[ChoiceQuery]) -> Result<Vec<ChoiceAnswer>, String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn best_picks_highest_probability() {
        let a = ChoiceAnswer {
            probabilities: vec![0.2, 0.5, 0.3],
        };
        assert_eq!(a.best(), Some((1, 0.5)));
        assert_eq!(ChoiceAnswer { probabilities: vec![] }.best(), None);
    }
}
