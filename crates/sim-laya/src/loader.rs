//! Come ottenere il modello Laya vero, se è compilato (feature `laya`).
//!
//! È l'unico punto in cui cervello, valutazione e gioco toccano
//! [`crate::laya::LayaModel`]: senza la feature restituisce un errore che dice
//! come abilitarla, e tutto il resto compila lo stesso.
//!
//! Variabili d'ambiente:
//! - `LAYA_MODEL_DIR`: carica la checkpoint da una cartella locale invece di
//!   scaricarla da Hugging Face (con la stessa struttura del repository).

use crate::brain::ModelLoader;
use crate::model::ChoiceModel;

/// Vero se il modello Laya vero è compilato.
pub const REAL_MODEL_AVAILABLE: bool = cfg!(feature = "laya");

/// Perché il modello vero non c'è, se non c'è.
pub fn unavailable_reason() -> Option<&'static str> {
    (!REAL_MODEL_AVAILABLE)
        .then_some("compilato senza la feature `laya` (es. `cargo run -p game --features laya`)")
}

/// Carica il modello Laya vero (bloccante: scarica e legge i pesi).
#[cfg(feature = "laya")]
pub fn load_real_model() -> Result<Box<dyn ChoiceModel>, String> {
    use crate::laya::{LayaModel, LayaOptions, ModelSource};
    let mut options = LayaOptions::default();
    if let Some(dir) = std::env::var_os("LAYA_MODEL_DIR") {
        options.source = ModelSource::Dir(dir.into());
    }
    let model = LayaModel::load(options)?;
    Ok(Box::new(model))
}

/// Carica il modello Laya vero (bloccante: scarica e legge i pesi).
#[cfg(not(feature = "laya"))]
pub fn load_real_model() -> Result<Box<dyn ChoiceModel>, String> {
    Err(unavailable_reason().unwrap_or_default().to_string())
}

/// [`load_real_model`] da eseguire sul thread del worker
/// ([`crate::LayaBrain::attach_loader`]).
pub fn real_model_loader() -> ModelLoader {
    Box::new(load_real_model)
}

#[cfg(all(test, not(feature = "laya")))]
mod tests {
    use super::*;

    #[test]
    fn without_the_feature_loading_explains_why() {
        const { assert!(!REAL_MODEL_AVAILABLE) };
        let err = load_real_model().err().unwrap();
        assert!(err.contains("--features laya"), "{err}");
        assert_eq!(unavailable_reason(), Some(err.as_str()));
    }
}
