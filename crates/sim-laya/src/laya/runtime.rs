//! [`LayaModel`]: il modello Laya vero dietro il contratto [`ChoiceModel`].

use std::time::Instant;

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;

use super::config::{AgentConfig, EncoderConfig};
use super::download::{ModelSource, resolve};
use super::encoder::{Encoder, Masks};
use super::head::DecisionHead;
use super::sequence::{
    Budget, OptionLabels, QuestionKind, Row, collate, length_sorted_chunks, render_choice_options,
    softmax_with_temperature,
};
use super::tokenize::RowBuilder;
use crate::model::{ChoiceAnswer, ChoiceModel, ChoiceQuery};

/// Su quale dispositivo far girare il modello.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DevicePreference {
    /// GPU Metal se disponibile (feature `metal`), altrimenti CPU.
    #[default]
    Auto,
    Cpu,
    /// Solo Metal: errore se non c'è.
    Metal,
}

/// Precisione di pesi e attivazioni.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Precision {
    /// F16 su Metal, F32 su CPU.
    #[default]
    Auto,
    F16,
    F32,
}

/// Opzioni di [`LayaModel::load`].
#[derive(Clone, Debug)]
pub struct LayaOptions {
    pub source: ModelSource,
    pub device: DevicePreference,
    pub precision: Precision,
    /// Lunghezza massima di una riga in token (lo stato viene troncato a destra).
    /// La latenza dipende dalla lunghezza reale, questo è solo un tetto.
    pub max_len: usize,
    /// Budget di intestazione + opzioni; `None` = quello della checkpoint (256).
    pub head_max_len: Option<usize>,
    /// Righe massime per forward pass; lotti più grandi vengono spezzati.
    pub max_batch_rows: usize,
    /// Come etichettare le opzioni nel prompt.
    pub option_labels: OptionLabels,
    /// Sostituisce la temperatura della checkpoint (quella di `laya-multilingual` è 1).
    pub temperature: Option<f32>,
    /// Fa un forward a vuoto dopo il caricamento, così la prima domanda vera non
    /// paga la compilazione dei kernel Metal.
    pub warm_up: bool,
}

impl Default for LayaOptions {
    fn default() -> Self {
        Self {
            source: ModelSource::default(),
            device: DevicePreference::Auto,
            precision: Precision::Auto,
            max_len: 384,
            head_max_len: None,
            max_batch_rows: 32,
            option_labels: OptionLabels::Plain,
            temperature: None,
            warm_up: true,
        }
    }
}

/// Laya (encoder ModernBERT/mmBERT + decision head) in-process con candle.
pub struct LayaModel {
    name: String,
    rows: RowBuilder,
    encoder: Encoder,
    head: DecisionHead,
    pub(crate) agent: AgentConfig,
    device: Device,
    dtype: DType,
    budget: Budget,
    max_batch_rows: usize,
    labels: OptionLabels,
    temperature: Option<f32>,
    load_time: std::time::Duration,
}

fn err<E: std::fmt::Display>(context: &'static str) -> impl FnOnce(E) -> String {
    move |e| format!("{context}: {e}")
}

impl LayaModel {
    /// Carica tokenizer e pesi (scaricandoli la prima volta).
    pub fn load(options: LayaOptions) -> Result<Self, String> {
        let start = Instant::now();
        let files = resolve(&options.source)?;
        let enc_cfg = EncoderConfig::from_file(&files.encoder_config)?;
        let agent = AgentConfig::from_file(&files.agent_config)?;
        let rows = RowBuilder::load(&files.tokenizer, &files.tokenizer_config)?;

        let head_max_len = options.head_max_len.unwrap_or(agent.head_max_len);
        if options.max_len < head_max_len + 16 {
            return Err(format!(
                "max_len={} troppo piccolo: serve almeno head_max_len + 16 = {}",
                options.max_len,
                head_max_len + 16
            ));
        }

        let device = match options.device {
            DevicePreference::Cpu => Device::Cpu,
            DevicePreference::Metal => Device::new_metal(0).map_err(err("Metal"))?,
            DevicePreference::Auto => Device::new_metal(0).unwrap_or(Device::Cpu),
        };
        let dtype = match (options.precision, device.is_metal()) {
            (Precision::F16, _) | (Precision::Auto, true) => DType::F16,
            (Precision::F32, _) | (Precision::Auto, false) => DType::F32,
        };

        // SAFETY: il file viene mappato in sola lettura; il contratto di mmap è che
        // nessuno lo modifichi mentre è mappato. Sta nella cache di Hugging Face (o in
        // una cartella scelta dal chiamante) e nessuno lo riscrive durante il gioco.
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[&files.weights], dtype, &device) }
            .map_err(err("pesi"))?;
        let encoder =
            Encoder::load(&enc_cfg, options.max_len, vb.pp("encoder")).map_err(err("encoder"))?;
        let head = DecisionHead::load(enc_cfg.hidden_size, agent.head_layers, vb)
            .map_err(err("decision head"))?;

        let mut model = Self {
            name: options.source.display_name(),
            rows,
            encoder,
            head,
            agent,
            device,
            dtype,
            budget: Budget {
                max_len: options.max_len,
                head_max_len,
            },
            max_batch_rows: options.max_batch_rows.max(1),
            labels: options.option_labels,
            temperature: options.temperature,
            load_time: Default::default(),
        };
        if options.warm_up {
            model.predict_batch(&[ChoiceQuery {
                context: "warm up".into(),
                instructions: "?".into(),
                options: vec!["a".into(), "b".into()],
            }])?;
        }
        model.load_time = start.elapsed();
        Ok(model)
    }

    /// `"metal"` o `"cpu"`.
    pub fn device_name(&self) -> &'static str {
        if self.device.is_metal() {
            "metal"
        } else {
            "cpu"
        }
    }

    /// `"f16"` o `"f32"`.
    pub fn precision_name(&self) -> &'static str {
        if self.dtype == DType::F16 {
            "f16"
        } else {
            "f32"
        }
    }

    /// Tempo di [`LayaModel::load`] (download escluso se già in cache).
    pub fn load_time(&self) -> std::time::Duration {
        self.load_time
    }

    /// La riga che il modello vede per una query (per debug e test).
    pub fn build_row(&self, query: &ChoiceQuery) -> Result<Row, String> {
        let options = render_choice_options(&query.options, self.labels);
        self.rows.build(
            QuestionKind::Choice,
            &query.instructions,
            &options,
            &query.context,
            self.budget,
        )
    }

    /// Accesso al tokenizer (per i test di parità).
    #[cfg(test)]
    pub(crate) fn row_builder(&self) -> &RowBuilder {
        &self.rows
    }

    /// Logit grezzi (prima della temperatura) di un lotto di righe, un forward.
    pub fn raw_logits(&self, rows: &[&Row]) -> Result<Vec<Vec<f32>>, String> {
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        if rows.iter().any(|r| r.markers.is_empty()) {
            return Err("riga senza opzioni".into());
        }
        self.forward(rows).map_err(err("forward"))
    }

    fn forward(&self, rows: &[&Row]) -> candle_core::Result<Vec<Vec<f32>>> {
        let batch = collate(rows, self.rows.special().pad);
        let (b, l, k) = (batch.rows, batch.seq_len, batch.max_options);
        let ids = Tensor::from_vec(batch.ids, (b, l), &self.device)?;
        let masks = Masks::new(
            &batch.lengths,
            l,
            self.encoder.half_window(),
            self.dtype,
            &self.device,
        )?;
        let h = self.encoder.forward(&ids, &masks)?;
        let kinds = Tensor::from_vec(batch.kinds, b, &self.device)?;
        let flat: Vec<u32> = (0..b)
            .flat_map(|i| {
                let pos = &batch.marker_pos[i * k..(i + 1) * k];
                pos.iter().map(move |&p| (i * l) as u32 + p)
            })
            .collect();
        let markers = Tensor::from_vec(flat, b * k, &self.device)?;
        let logits = self
            .head
            .forward(&h, &kinds, &markers, k, masks.keys.as_ref())?
            .reshape((b, k))?
            .to_vec2::<f32>()?;
        Ok(logits
            .into_iter()
            .zip(batch.option_counts)
            .map(|(mut row, n)| {
                row.truncate(n);
                row
            })
            .collect())
    }

    pub(crate) fn temperature_for(&self, kind: QuestionKind, options: usize) -> f32 {
        self.temperature
            .unwrap_or_else(|| self.agent.temperature(kind, options))
    }
}

impl ChoiceModel for LayaModel {
    fn name(&self) -> &str {
        &self.name
    }

    fn predict_batch(&mut self, queries: &[ChoiceQuery]) -> Result<Vec<ChoiceAnswer>, String> {
        let mut answers: Vec<Option<ChoiceAnswer>> = vec![None; queries.len()];
        let mut rows = Vec::new();
        let mut row_query = Vec::new();
        for (i, q) in queries.iter().enumerate() {
            match q.options.len() {
                0 => {
                    answers[i] = Some(ChoiceAnswer {
                        probabilities: vec![],
                    })
                }
                1 => {
                    answers[i] = Some(ChoiceAnswer {
                        probabilities: vec![1.0],
                    })
                }
                _ => {
                    rows.push(self.build_row(q).map_err(|e| format!("query {i}: {e}"))?);
                    row_query.push(i);
                }
            }
        }
        let lengths: Vec<usize> = rows.iter().map(|r| r.ids.len()).collect();
        for chunk in length_sorted_chunks(&lengths, self.max_batch_rows) {
            let batch: Vec<&Row> = chunk.iter().map(|&r| &rows[r]).collect();
            let logits = self.raw_logits(&batch)?;
            for (&r, l) in chunk.iter().zip(logits) {
                let t = self.temperature_for(QuestionKind::Choice, l.len());
                answers[row_query[r]] = Some(ChoiceAnswer {
                    probabilities: softmax_with_temperature(&l, t),
                });
            }
        }
        Ok(answers
            .into_iter()
            .map(|a| a.expect("ogni query ha una risposta"))
            .collect())
    }
}
