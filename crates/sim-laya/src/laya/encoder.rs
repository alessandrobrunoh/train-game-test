//! Encoder ModernBERT / mmBERT, solo inferenza, con i nomi dei tensori originali.
//!
//! Segue `ModernBertModel` di transformers 5 (attenzione `sdpa`, senza unpadding):
//! embedding → LayerNorm → 22 layer pre-norm (il layer 0 senza `attn_norm`),
//! attenzione globale ogni 3 layer e a finestra (±`local_attention/2`) negli altri,
//! RoPE non interlacciata, MLP GeGLU (GELU esatta), LayerNorm finale.
//!
//! Scritto per candle invece di usare `candle_transformers::models::modernbert`
//! perché quella versione somma una maschera F32 ad attivazioni F16 (errore di
//! dtype), porta la maschera a `-inf` in F16 (righe di padding tutte mascherate
//! → NaN) e calcola la RoPE in F16 (posizioni imprecise oltre 2048). Alcune
//! scelte (bias nullo per la LayerNorm fusa, maschera che non lascia mai righe
//! vuote, ultimo layer della testa solo sui marker) sono adattate da
//! b0xtch/laya-candle (Apache-2.0, <https://github.com/b0xtch/laya-candle>).

use candle_core::{D, DType, Device, Module, Result, Tensor};
use candle_nn::{Embedding, LayerNorm, Linear, VarBuilder};

use super::config::EncoderConfig;

/// LayerNorm senza bias come LayerNorm con bias nullo: stesso risultato, ma
/// candle usa il kernel fuso solo quando il bias c'è.
pub(crate) fn layer_norm(size: usize, eps: f64, bias: bool, vb: VarBuilder) -> Result<LayerNorm> {
    let weight = vb.get(size, "weight")?;
    let bias = if bias {
        vb.get(size, "bias")?
    } else {
        Tensor::zeros(size, vb.dtype(), vb.device())?
    };
    Ok(LayerNorm::new(weight, bias, eps))
}

fn linear_no_bias(in_dim: usize, out_dim: usize, vb: VarBuilder) -> Result<Linear> {
    Ok(Linear::new(vb.get((out_dim, in_dim), "weight")?, None))
}

/// Il kernel SDPA di candle su Metal supporta solo alcune dimensioni di testa.
pub(crate) fn fused_attention(device: &Device, head_dim: usize) -> bool {
    device.is_metal() && matches!(head_dim, 32 | 64 | 72 | 80 | 96 | 128 | 256)
}

/// `softmax(q kᵀ / √d + mask) v` su tensori `(B, H, L, d)`.
///
/// `mask` è additiva, `(B, 1, Lq|1, Lk)`, con `-inf` sulle chiavi escluse; non
/// lascia mai una riga interamente mascherata.
pub(crate) fn attention(
    q: &Tensor,
    k: &Tensor,
    v: &Tensor,
    mask: Option<&Tensor>,
) -> Result<Tensor> {
    let (b, h, lq, d) = q.dims4()?;
    let lk = k.dim(2)?;
    let scale = (d as f64).powf(-0.5);
    if fused_attention(q.device(), d) && lq > 1 && lq <= lk {
        let mask = mask.map(|m| m.broadcast_as((b, h, lq, lk))).transpose()?;
        return candle_nn::ops::sdpa(q, k, v, mask.as_ref(), false, scale as f32, 1.0);
    }
    let att = (q.matmul(&k.t()?)? * scale)?.to_dtype(DType::F32)?;
    let att = match mask {
        Some(m) => att.broadcast_add(&m.to_dtype(DType::F32)?)?,
        None => att,
    };
    candle_nn::ops::softmax_last_dim(&att)?
        .to_dtype(v.dtype())?
        .matmul(v)
}

/// `(B, H, L, d)` → `(B, L, H·d)`.
pub(crate) fn merge_heads(x: &Tensor) -> Result<Tensor> {
    let (b, h, l, d) = x.dims4()?;
    x.transpose(1, 2)?.contiguous()?.reshape((b, l, h * d))
}

/// `(B, L, n·H·d)` → n tensori contigui `(B, H, L, d)`.
pub(crate) fn split_heads(x: &Tensor, parts: usize, heads: usize) -> Result<Vec<Tensor>> {
    let (b, l, n) = x.dims3()?;
    let d = n / (parts * heads);
    let x = x
        .reshape((b, l, parts, heads, d))?
        .permute((2, 0, 3, 1, 4))?;
    (0..parts).map(|i| x.get(i)?.contiguous()).collect()
}

/// Tabelle cos/sin della RoPE, calcolate in F32 e poi convertite.
struct Rope {
    cos: Tensor,
    sin: Tensor,
}

impl Rope {
    fn new(
        head_dim: usize,
        theta: f64,
        max_len: usize,
        dtype: DType,
        dev: &Device,
    ) -> Result<Self> {
        let inv: Vec<f64> = (0..head_dim)
            .step_by(2)
            .map(|i| 1.0 / theta.powf(i as f64 / head_dim as f64))
            .collect();
        let freqs: Vec<f32> = (0..max_len)
            .flat_map(|p| inv.iter().map(move |f| (p as f64 * f) as f32))
            .collect();
        let freqs = Tensor::from_vec(freqs, (max_len, head_dim / 2), dev)?;
        Ok(Self {
            cos: freqs.cos()?.to_dtype(dtype)?,
            sin: freqs.sin()?.to_dtype(dtype)?,
        })
    }

    fn apply(&self, x: &Tensor) -> Result<Tensor> {
        let l = x.dim(2)?;
        let cos = self.cos.narrow(0, 0, l)?;
        let sin = self.sin.narrow(0, 0, l)?;
        candle_nn::rotary_emb::rope(&x.contiguous()?, &cos, &sin)
    }
}

/// Maschere additive di un lotto, costruite una volta per forward.
pub(crate) struct Masks {
    /// `(B, 1, 1, L)`: solo padding. `None` se nessuna riga ha padding.
    pub keys: Option<Tensor>,
    /// `(B, 1, L, L)`: padding + finestra locale.
    pub local: Tensor,
}

impl Masks {
    /// `lengths`: lunghezza reale di ogni riga; `half_window`: `local_attention / 2`.
    pub fn new(
        lengths: &[usize],
        seq_len: usize,
        half_window: usize,
        dtype: DType,
        dev: &Device,
    ) -> Result<Self> {
        let b = lengths.len();
        let padded = lengths.iter().any(|&n| n != seq_len);
        let keys = if padded {
            let v: Vec<f32> = lengths
                .iter()
                .flat_map(|&n| {
                    (0..seq_len).map(move |j| if j < n { 0.0 } else { f32::NEG_INFINITY })
                })
                .collect();
            Some(Tensor::from_vec(v, (b, 1, 1, seq_len), dev)?.to_dtype(dtype)?)
        } else {
            None
        };
        let mut v = Vec::with_capacity(b * seq_len * seq_len);
        for &n in lengths {
            for i in 0..seq_len {
                for j in 0..seq_len {
                    // Le query di padding (i ≥ n) guardano tutte le chiavi valide:
                    // il loro output non serve, ma così nessuna riga resta vuota.
                    let masked = j >= n || (i < n && i.abs_diff(j) > half_window);
                    v.push(if masked { f32::NEG_INFINITY } else { 0.0 });
                }
            }
        }
        let local = Tensor::from_vec(v, (b, 1, seq_len, seq_len), dev)?.to_dtype(dtype)?;
        Ok(Self { keys, local })
    }
}

struct Layer {
    attn_norm: Option<LayerNorm>,
    wqkv: Linear,
    wo: Linear,
    mlp_norm: LayerNorm,
    wi: Linear,
    mlp_wo: Linear,
    local: bool,
}

/// Encoder ModernBERT.
pub(crate) struct Encoder {
    embeddings: Embedding,
    emb_norm: LayerNorm,
    layers: Vec<Layer>,
    final_norm: LayerNorm,
    global_rope: Rope,
    local_rope: Rope,
    heads: usize,
    half_window: usize,
    max_len: usize,
}

impl Encoder {
    /// `vb` punta al prefisso `encoder.`; `max_len` limita le tabelle RoPE.
    pub fn load(cfg: &EncoderConfig, max_len: usize, vb: VarBuilder) -> Result<Self> {
        let d = cfg.hidden_size;
        let eps = cfg.norm_eps;
        let nb = cfg.norm_bias;
        let embeddings = Embedding::new(
            vb.get((cfg.vocab_size, d), "embeddings.tok_embeddings.weight")?,
            d,
        );
        let emb_norm = layer_norm(d, eps, nb, vb.pp("embeddings.norm"))?;
        let mut layers = Vec::with_capacity(cfg.num_hidden_layers);
        for i in 0..cfg.num_hidden_layers {
            let lv = vb.pp(format!("layers.{i}"));
            layers.push(Layer {
                // Il layer 0 ha `attn_norm = Identity` (nessun peso nella checkpoint).
                attn_norm: if i == 0 {
                    None
                } else {
                    Some(layer_norm(d, eps, nb, lv.pp("attn_norm"))?)
                },
                wqkv: linear_no_bias(d, 3 * d, lv.pp("attn.Wqkv"))?,
                wo: linear_no_bias(d, d, lv.pp("attn.Wo"))?,
                mlp_norm: layer_norm(d, eps, nb, lv.pp("mlp_norm"))?,
                wi: linear_no_bias(d, 2 * cfg.intermediate_size, lv.pp("mlp.Wi"))?,
                mlp_wo: linear_no_bias(cfg.intermediate_size, d, lv.pp("mlp.Wo"))?,
                local: cfg.is_local(i),
            });
        }
        let final_norm = layer_norm(d, eps, nb, vb.pp("final_norm"))?;
        let hd = cfg.head_dim();
        let (dtype, dev) = (vb.dtype(), vb.device());
        Ok(Self {
            embeddings,
            emb_norm,
            layers,
            final_norm,
            global_rope: Rope::new(hd, cfg.rope_theta(false), max_len, dtype, dev)?,
            local_rope: Rope::new(hd, cfg.rope_theta(true), max_len, dtype, dev)?,
            heads: cfg.num_attention_heads,
            half_window: cfg.local_attention / 2,
            max_len,
        })
    }

    pub fn half_window(&self) -> usize {
        self.half_window
    }

    /// `ids`: `(B, L)` u32. Restituisce `last_hidden_state` `(B, L, D)`.
    pub fn forward(&self, ids: &Tensor, masks: &Masks) -> Result<Tensor> {
        let l = ids.dim(1)?;
        if l > self.max_len {
            candle_core::bail!("sequenza di {l} token oltre max_len={}", self.max_len);
        }
        let mut x = self.embeddings.forward(ids)?.apply(&self.emb_norm)?;
        for layer in &self.layers {
            let h = match &layer.attn_norm {
                Some(n) => x.apply(n)?,
                None => x.clone(),
            };
            let qkv = split_heads(&h.apply(&layer.wqkv)?, 3, self.heads)?;
            let rope = if layer.local {
                &self.local_rope
            } else {
                &self.global_rope
            };
            let q = rope.apply(&qkv[0])?;
            let k = rope.apply(&qkv[1])?;
            let mask = if layer.local {
                Some(&masks.local)
            } else {
                masks.keys.as_ref()
            };
            let att = attention(&q, &k, &qkv[2], mask)?;
            x = (x + merge_heads(&att)?.apply(&layer.wo)?)?;
            let gates = x
                .apply(&layer.mlp_norm)?
                .apply(&layer.wi)?
                .chunk(2, D::Minus1)?;
            let mlp = (gates[0].gelu_erf()? * &gates[1])?.apply(&layer.mlp_wo)?;
            x = (x + mlp)?;
        }
        x.apply(&self.final_norm)
    }
}
