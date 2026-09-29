//! Tokenizzazione delle domande: da testo a [`Row`].

use std::path::Path;

use tokenizers::Tokenizer;

use super::sequence::{
    Budget, QuestionKind, Row, SpecialIds, assemble_row, head_text, option_text,
};

/// Tokenizer della checkpoint più gli id speciali che usa Laya.
///
/// Gli id speciali vengono dai *nomi* in `tokenizer_config.json`, come fa
/// `AutoTokenizer`: per mmBERT `cls_token` è `<bos>` (2) e `sep_token` è
/// `<eos>` (1), anche se `encoder/config.json` dichiara `cls_token_id: 1`.
pub struct RowBuilder {
    tokenizer: Tokenizer,
    special: SpecialIds,
    mask_text: String,
}

impl RowBuilder {
    /// Carica `tokenizer.json` e `tokenizer_config.json`.
    pub fn load(tokenizer_json: &Path, tokenizer_config: &Path) -> Result<Self, String> {
        let mut tokenizer = Tokenizer::from_file(tokenizer_json)
            .map_err(|e| format!("tokenizer {}: {e}", tokenizer_json.display()))?;
        // Python chiama il tokenizer senza `truncation`/`padding`, che le disattiva.
        tokenizer
            .with_truncation(None)
            .map_err(|e| format!("tokenizer: {e}"))?;
        tokenizer.with_padding(None);
        let text = std::fs::read_to_string(tokenizer_config)
            .map_err(|e| format!("lettura {}: {e}", tokenizer_config.display()))?;
        let cfg: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| format!("parsing {}: {e}", tokenizer_config.display()))?;
        let token = |key: &str| -> Result<(String, u32), String> {
            let v = &cfg[key];
            let text = v
                .as_str()
                .or_else(|| v.get("content").and_then(serde_json::Value::as_str))
                .ok_or_else(|| format!("tokenizer_config.json: manca {key}"))?;
            let id = tokenizer
                .token_to_id(text)
                .ok_or_else(|| format!("tokenizer: {key} {text:?} non è nel vocabolario"))?;
            Ok((text.to_owned(), id))
        };
        let (_, cls) = token("cls_token")?;
        let (_, sep) = token("sep_token")?;
        let (mask_text, mask) = token("mask_token")?;
        let (_, pad) = token("pad_token")?;
        Ok(Self {
            tokenizer,
            special: SpecialIds {
                cls,
                sep,
                mask,
                pad,
            },
            mask_text,
        })
    }

    pub fn special(&self) -> SpecialIds {
        self.special
    }

    /// Tokenizza senza token speciali, dopo aver neutralizzato il testo del `[MASK]`
    /// (Python lo sostituisce con uno spazio, così nessun testo crea marker finti).
    pub fn encode(&self, text: &str) -> Result<Vec<u32>, String> {
        let clean = text.replace(&self.mask_text, " ");
        self.tokenizer
            .encode(clean, false)
            .map(|e| e.get_ids().to_vec())
            .map_err(|e| format!("tokenizzazione: {e}"))
    }

    /// `build_sequence` di Python per una domanda con opzioni già rese in testo
    /// (vedi [`super::sequence::render_choice_options`]).
    pub fn build(
        &self,
        kind: QuestionKind,
        instructions: &str,
        rendered_options: &[String],
        state: &str,
        budget: Budget,
    ) -> Result<Row, String> {
        let head = self.encode(&head_text(
            kind,
            &instructions.replace(&self.mask_text, " "),
        ))?;
        let options = rendered_options
            .iter()
            .map(|o| self.encode(&option_text(o)))
            .collect::<Result<Vec<_>, _>>()?;
        let state = self.encode(state)?;
        assemble_row(self.special, kind, head, &options, &state, budget)
    }
}
