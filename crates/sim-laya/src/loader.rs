//! Come ottenere il modello Laya vero, se è compilato (feature `laya`).
//!
//! È l'unico punto in cui cervello, valutazione e gioco toccano
//! [`crate::laya::LayaModel`]: senza la feature restituisce un errore che dice
//! come abilitarla, e tutto il resto compila lo stesso.
//!
//! Variabili d'ambiente:
//! - `LAYA_MODEL_DIR`: carica la checkpoint da una cartella locale invece di
//!   scaricarla da Hugging Face (con la stessa struttura del repository:
//!   `model.safetensors`, `encoder/config.json`, `rl_agent_config.json`,
//!   `tokenizer/tokenizer.json`, `tokenizer/tokenizer_config.json`). È il modo
//!   di usare una checkpoint messa a punto con `tools/laya-finetune/`: le sue
//!   temperature fittate (`temperature`, `temperature_by_options` di
//!   `rl_agent_config.json`) si applicano da sole.

use std::path::{Path, PathBuf};

use crate::brain::ModelLoader;
use crate::model::ChoiceModel;

/// Variabile d'ambiente con la cartella di una checkpoint locale.
pub const MODEL_DIR_ENV: &str = "LAYA_MODEL_DIR";

/// La cartella di `LAYA_MODEL_DIR`, se impostata e non vuota.
pub fn model_dir_from_env() -> Option<PathBuf> {
    std::env::var_os(MODEL_DIR_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Vero se il modello Laya vero è compilato.
pub const REAL_MODEL_AVAILABLE: bool = cfg!(feature = "laya");

/// Perché il modello vero non c'è, se non c'è.
pub fn unavailable_reason() -> Option<&'static str> {
    (!REAL_MODEL_AVAILABLE)
        .then_some("compilato senza la feature `laya` (es. `cargo run -p game --features laya`)")
}

/// Carica il modello Laya vero (bloccante: scarica e legge i pesi): da
/// `LAYA_MODEL_DIR` se impostata, altrimenti `laya-multilingual` da Hugging Face.
pub fn load_real_model() -> Result<Box<dyn ChoiceModel>, String> {
    load_real_model_from(model_dir_from_env().as_deref())
}

/// Carica il modello Laya vero da una cartella locale (es. una checkpoint
/// messa a punto) o, con `None`, da Hugging Face. Bloccante.
#[cfg(feature = "laya")]
pub fn load_real_model_from(dir: Option<&Path>) -> Result<Box<dyn ChoiceModel>, String> {
    use crate::laya::{LayaModel, LayaOptions, ModelSource};
    let mut options = LayaOptions::default();
    if let Some(dir) = dir {
        options.source = ModelSource::Dir(dir.to_path_buf());
    }
    let model = LayaModel::load(options)?;
    Ok(Box::new(model))
}

/// Carica il modello Laya vero da una cartella locale (es. una checkpoint
/// messa a punto) o, con `None`, da Hugging Face. Bloccante.
#[cfg(not(feature = "laya"))]
pub fn load_real_model_from(dir: Option<&Path>) -> Result<Box<dyn ChoiceModel>, String> {
    let _ = dir;
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
        let err = load_real_model_from(Some(Path::new("non-esiste")))
            .err()
            .unwrap();
        assert!(err.contains("--features laya"), "{err}");
    }
}
