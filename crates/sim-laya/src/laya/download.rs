//! Dove trovare i file della checkpoint: una cartella locale o Hugging Face.

use std::path::{Path, PathBuf};

use hf_hub::api::sync::ApiBuilder;
use hf_hub::{Repo, RepoType};

/// Repository Hugging Face di `laya-multilingual` (mmBERT-base, 322M, F16).
pub const LAYA_MULTILINGUAL_REPO: &str = "convaiinnovations/laya-multilingual";

/// Commit fissato di [`LAYA_MULTILINGUAL_REPO`]. È lo SHA "reviewed" di
/// `laya/revisions.py` (`PINNED_REVISIONS`) e la `main` al 2026-09-29.
pub const LAYA_MULTILINGUAL_REVISION: &str = "e4e9ddf21a7b1903b7acffd8814ad4307bf63a67";

/// Da dove caricare il modello.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelSource {
    /// Scarica (o riusa dalla cache) un repository Hugging Face a un commit fisso.
    ///
    /// La cache è quella standard di `hf-hub` (`$HF_HOME/hub`, di default
    /// `~/.cache/huggingface/hub`), condivisa con Python; `cache_dir` la sostituisce.
    HuggingFace {
        repo: String,
        revision: String,
        cache_dir: Option<PathBuf>,
    },
    /// Una cartella con lo stesso layout del repository.
    Dir(PathBuf),
}

impl Default for ModelSource {
    fn default() -> Self {
        ModelSource::HuggingFace {
            repo: LAYA_MULTILINGUAL_REPO.into(),
            revision: LAYA_MULTILINGUAL_REVISION.into(),
            cache_dir: None,
        }
    }
}

impl ModelSource {
    /// Nome leggibile: l'ultima parte del repository o della cartella.
    pub fn display_name(&self) -> String {
        match self {
            ModelSource::HuggingFace { repo, .. } => repo.rsplit('/').next().unwrap_or(repo).into(),
            ModelSource::Dir(dir) => dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "laya".into()),
        }
    }
}

/// I file di una checkpoint Laya.
#[derive(Clone, Debug)]
pub struct CheckpointFiles {
    pub encoder_config: PathBuf,
    pub agent_config: PathBuf,
    pub tokenizer: PathBuf,
    pub tokenizer_config: PathBuf,
    pub weights: PathBuf,
}

const FILES: [&str; 5] = [
    "encoder/config.json",
    "rl_agent_config.json",
    "tokenizer/tokenizer.json",
    "tokenizer/tokenizer_config.json",
    "model.safetensors",
];

impl CheckpointFiles {
    fn from_paths(p: [PathBuf; 5]) -> Self {
        let [
            encoder_config,
            agent_config,
            tokenizer,
            tokenizer_config,
            weights,
        ] = p;
        Self {
            encoder_config,
            agent_config,
            tokenizer,
            tokenizer_config,
            weights,
        }
    }

    fn in_dir(dir: &Path) -> Result<Self, String> {
        let paths = FILES.map(|f| dir.join(f));
        if let Some(missing) = paths.iter().find(|p| !p.is_file()) {
            return Err(format!("manca {}", missing.display()));
        }
        Ok(Self::from_paths(paths))
    }
}

/// Risolve i file della checkpoint, scaricandoli se serve (~680 MB la prima volta).
pub fn resolve(source: &ModelSource) -> Result<CheckpointFiles, String> {
    match source {
        ModelSource::Dir(dir) => CheckpointFiles::in_dir(dir),
        ModelSource::HuggingFace {
            repo,
            revision,
            cache_dir,
        } => {
            let mut builder = ApiBuilder::from_env().with_progress(false);
            if let Some(dir) = cache_dir {
                builder = builder.with_cache_dir(dir.clone());
            }
            let api = builder.build().map_err(|e| format!("hf-hub: {e}"))?;
            let api_repo = api.repo(Repo::with_revision(
                repo.clone(),
                RepoType::Model,
                revision.clone(),
            ));
            let mut out = Vec::with_capacity(FILES.len());
            for f in FILES {
                out.push(
                    api_repo
                        .get(f)
                        .map_err(|e| format!("download {repo}@{revision}/{f}: {e}"))?,
                );
            }
            let paths: [PathBuf; 5] = out.try_into().expect("cinque file");
            Ok(CheckpointFiles::from_paths(paths))
        }
    }
}
