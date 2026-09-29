//! Lettura di `encoder/config.json` (ModernBERT, formato transformers 5) e
//! `rl_agent_config.json` di una checkpoint Laya.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use super::sequence::{QuestionKind, clamp_temperature, temperature_bucket};

/// Configurazione dell'encoder ModernBERT / mmBERT.
#[derive(Clone, Debug, Deserialize)]
pub struct EncoderConfig {
    pub model_type: String,
    pub vocab_size: usize,
    pub hidden_size: usize,
    pub intermediate_size: usize,
    pub num_hidden_layers: usize,
    pub num_attention_heads: usize,
    #[serde(default = "default_eps")]
    pub norm_eps: f64,
    #[serde(default)]
    pub norm_bias: bool,
    #[serde(default)]
    pub attention_bias: bool,
    #[serde(default)]
    pub mlp_bias: bool,
    #[serde(default = "default_activation")]
    pub hidden_activation: String,
    /// Ampiezza della finestra locale: ogni token vede ±`local_attention / 2`.
    pub local_attention: usize,
    #[serde(default = "default_global_every")]
    pub global_attn_every_n_layers: usize,
    /// transformers 4.x; transformers 5 usa `rope_parameters`.
    #[serde(default)]
    pub global_rope_theta: Option<f64>,
    #[serde(default)]
    pub local_rope_theta: Option<f64>,
    #[serde(default)]
    pub rope_parameters: Option<serde_json::Value>,
    #[serde(default)]
    pub layer_types: Option<Vec<String>>,
}

fn default_eps() -> f64 {
    1e-5
}
fn default_activation() -> String {
    "gelu".into()
}
fn default_global_every() -> usize {
    3
}

impl EncoderConfig {
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let cfg: Self = read_json(path)?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), String> {
        if self.model_type != "modernbert" {
            return Err(format!("encoder non supportato: {}", self.model_type));
        }
        if self.hidden_activation != "gelu" {
            return Err(format!(
                "attivazione non supportata: {}",
                self.hidden_activation
            ));
        }
        if self.attention_bias || self.mlp_bias {
            return Err("encoder con bias su attenzione/MLP non supportato".into());
        }
        if self.num_attention_heads == 0
            || !self.hidden_size.is_multiple_of(self.num_attention_heads)
        {
            return Err("hidden_size non divisibile per num_attention_heads".into());
        }
        if let Some(types) = &self.layer_types
            && types.len() != self.num_hidden_layers
        {
            return Err("layer_types non ha un elemento per layer".into());
        }
        Ok(())
    }

    pub fn head_dim(&self) -> usize {
        self.hidden_size / self.num_attention_heads
    }

    /// Il layer `i` usa l'attenzione a finestra (locale)?
    pub fn is_local(&self, i: usize) -> bool {
        match &self.layer_types {
            Some(types) => types[i] == "sliding_attention",
            None => !i.is_multiple_of(self.global_attn_every_n_layers.max(1)),
        }
    }

    /// Base della RoPE per i layer globali o locali.
    ///
    /// Come `_apply_rope_config` di Laya: `rope_parameters` (transformers 5)
    /// vince sui campi piatti; i default sono quelli di transformers 4
    /// (160000 globale, 10000 locale). mmBERT usa 160000 per entrambi.
    pub fn rope_theta(&self, local: bool) -> f64 {
        let (key, flat, default) = if local {
            ("sliding_attention", self.local_rope_theta, 10_000.0)
        } else {
            ("full_attention", self.global_rope_theta, 160_000.0)
        };
        if let Some(rope) = &self.rope_parameters {
            if let Some(t) = rope
                .get(key)
                .and_then(|p| p.get("rope_theta"))
                .and_then(serde_json::Value::as_f64)
            {
                return t;
            }
            if let Some(t) = rope.get("rope_theta").and_then(serde_json::Value::as_f64) {
                return t;
            }
        }
        flat.unwrap_or(default)
    }
}

/// `rl_agent_config.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct AgentConfig {
    #[serde(default = "default_head_layers")]
    pub head_layers: usize,
    #[serde(default = "default_head_max_len")]
    pub head_max_len: usize,
    #[serde(default = "default_temperature")]
    pub temperature: [f32; 3],
    #[serde(default)]
    pub temperature_by_options: HashMap<String, f32>,
}

fn default_head_layers() -> usize {
    2
}
fn default_head_max_len() -> usize {
    192
}
fn default_temperature() -> [f32; 3] {
    [1.0; 3]
}

impl AgentConfig {
    pub fn from_file(path: &Path) -> Result<Self, String> {
        read_json(path)
    }

    /// Temperatura per (tipo, numero di opzioni), già limitata come fa Laya.
    pub fn temperature(&self, kind: QuestionKind, options: usize) -> f32 {
        let t = self
            .temperature_by_options
            .get(&temperature_bucket(kind, options))
            .copied()
            .unwrap_or(self.temperature[kind as usize]);
        clamp_temperature(t)
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("lettura {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("parsing {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mmbert_config_parses_rope_parameters() {
        let cfg: EncoderConfig = serde_json::from_str(
            r#"{"model_type":"modernbert","vocab_size":256000,"hidden_size":768,
            "intermediate_size":1152,"num_hidden_layers":4,"num_attention_heads":12,
            "norm_eps":1e-5,"local_attention":128,"global_attn_every_n_layers":3,
            "layer_types":["full_attention","sliding_attention","sliding_attention","full_attention"],
            "rope_parameters":{"full_attention":{"rope_theta":160000},
                               "sliding_attention":{"rope_theta":160000}}}"#,
        )
        .unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.rope_theta(false), 160_000.0);
        assert_eq!(cfg.rope_theta(true), 160_000.0);
        assert_eq!(cfg.head_dim(), 64);
        assert!(!cfg.is_local(0) && cfg.is_local(1) && cfg.is_local(2) && !cfg.is_local(3));
    }

    #[test]
    fn transformers4_defaults() {
        let cfg: EncoderConfig = serde_json::from_str(
            r#"{"model_type":"modernbert","vocab_size":10,"hidden_size":64,
            "intermediate_size":96,"num_hidden_layers":4,"num_attention_heads":4,
            "local_attention":8}"#,
        )
        .unwrap();
        assert_eq!(cfg.rope_theta(false), 160_000.0);
        assert_eq!(cfg.rope_theta(true), 10_000.0);
        assert!(!cfg.is_local(0) && cfg.is_local(1) && !cfg.is_local(3));
    }

    #[test]
    fn temperature_lookup_prefers_buckets() {
        let cfg: AgentConfig = serde_json::from_str(
            r#"{"temperature":[1.6,1.2,1.9],"temperature_by_options":{"choice:3-5":1.75,"choice:11+":0.1}}"#,
        )
        .unwrap();
        assert_eq!(cfg.temperature(QuestionKind::Choice, 3), 1.75);
        assert_eq!(cfg.temperature(QuestionKind::Choice, 2), 1.6);
        assert_eq!(cfg.temperature(QuestionKind::Noul, 2), 1.9);
        // 0.1 affilerebbe le probabilità: Laya la limita a 0.5
        assert_eq!(cfg.temperature(QuestionKind::Choice, 12), 0.5);
    }
}
