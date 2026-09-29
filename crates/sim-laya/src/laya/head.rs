//! Decision head di Laya (`DecisionModel` in `laya/common.py`).
//!
//! `h + type_emb[qtype]` → `head_layers` × `nn.TransformerEncoderLayer`
//! (pre-norm, MHA con `in_proj` impacchettato, FFN ReLU 4d, `nhead = d / 64`,
//! maschera sulle chiavi di padding) → gather sui marker → `scorer`
//! (LayerNorm → Linear → GELU → Linear) → un logit per opzione.
//! `act_head` non serve per le domande `choice` e non viene caricato.

use candle_core::{DType, Result, Tensor};
use candle_nn::{LayerNorm, Linear, VarBuilder};

use super::encoder::{attention, layer_norm, merge_heads, split_heads};

fn linear(in_dim: usize, out_dim: usize, vb: VarBuilder) -> Result<Linear> {
    Ok(Linear::new(
        vb.get((out_dim, in_dim), "weight")?,
        Some(vb.get(out_dim, "bias")?),
    ))
}

struct HeadLayer {
    norm1: LayerNorm,
    in_proj: Linear,
    out_proj: Linear,
    norm2: LayerNorm,
    linear1: Linear,
    linear2: Linear,
    heads: usize,
}

impl HeadLayer {
    fn load(d: usize, vb: VarBuilder) -> Result<Self> {
        let heads = (d / 64).max(1);
        Ok(Self {
            norm1: layer_norm(d, 1e-5, true, vb.pp("norm1"))?,
            in_proj: Linear::new(
                vb.get((3 * d, d), "self_attn.in_proj_weight")?,
                Some(vb.get(3 * d, "self_attn.in_proj_bias")?),
            ),
            out_proj: linear(d, d, vb.pp("self_attn.out_proj"))?,
            norm2: layer_norm(d, 1e-5, true, vb.pp("norm2"))?,
            linear1: linear(d, 4 * d, vb.pp("linear1"))?,
            linear2: linear(4 * d, d, vb.pp("linear2"))?,
            heads,
        })
    }

    fn feed_forward(&self, x: &Tensor) -> Result<Tensor> {
        let ff = x
            .apply(&self.norm2)?
            .apply(&self.linear1)?
            .relu()?
            .apply(&self.linear2)?;
        x + ff
    }

    /// Layer completo su tutte le posizioni.
    fn forward(&self, x: &Tensor, key_mask: Option<&Tensor>) -> Result<Tensor> {
        let qkv = split_heads(&x.apply(&self.norm1)?.apply(&self.in_proj)?, 3, self.heads)?;
        let att = attention(&qkv[0], &qkv[1], &qkv[2], key_mask)?;
        let x = (x + merge_heads(&att)?.apply(&self.out_proj)?)?;
        self.feed_forward(&x)
    }

    /// Layer calcolato solo per le posizioni `selected` (`(B·K,)` indici piatti in
    /// `B·L`): tutte le posizioni fanno comunque da chiavi/valori, quindi il
    /// risultato sui marker è identico a quello del layer completo.
    fn forward_selected(
        &self,
        x: &Tensor,
        selected: &Tensor,
        k: usize,
        key_mask: Option<&Tensor>,
    ) -> Result<Tensor> {
        let (b, l, d) = x.dims3()?;
        let h = x.apply(&self.norm1)?;
        let pick = |t: &Tensor| -> Result<Tensor> {
            t.reshape((b * l, d))?
                .index_select(selected, 0)?
                .reshape((b, k, d))
        };
        let x_sel = pick(x)?;
        let h_sel = pick(&h)?;
        let w = self.in_proj.weight();
        let bias = self.in_proj.bias().expect("in_proj ha il bias");
        let q_proj = Linear::new(w.narrow(0, 0, d)?, Some(bias.narrow(0, 0, d)?));
        let kv_proj = Linear::new(w.narrow(0, d, 2 * d)?, Some(bias.narrow(0, d, 2 * d)?));
        let q = split_heads(&h_sel.apply(&q_proj)?, 1, self.heads)?.remove(0);
        let kv = split_heads(&h.apply(&kv_proj)?, 2, self.heads)?;
        let att = attention(&q, &kv[0], &kv[1], key_mask)?;
        let x = (x_sel + merge_heads(&att)?.apply(&self.out_proj)?)?;
        self.feed_forward(&x)
    }
}

/// Testa decisionale: dai hidden state dell'encoder ai logit per opzione.
pub(crate) struct DecisionHead {
    type_emb: Tensor,
    layers: Vec<HeadLayer>,
    // Lo scorer resta in F32: con temperature basse anche un ULP di F16 si vede.
    scorer_norm: LayerNorm,
    scorer1: Linear,
    scorer2: Linear,
}

impl DecisionHead {
    /// `vb` punta alla radice della checkpoint.
    pub fn load(d: usize, head_layers: usize, vb: VarBuilder) -> Result<Self> {
        let layers = (0..head_layers)
            .map(|i| HeadLayer::load(d, vb.pp(format!("head.layers.{i}"))))
            .collect::<Result<_>>()?;
        let f32_vb = vb.to_dtype(DType::F32);
        Ok(Self {
            type_emb: vb.get((3, d), "type_emb.weight")?,
            layers,
            scorer_norm: layer_norm(d, 1e-5, true, f32_vb.pp("scorer.0"))?,
            scorer1: linear(d, d, f32_vb.pp("scorer.1"))?,
            scorer2: linear(d, 1, f32_vb.pp("scorer.3"))?,
        })
    }

    /// `h`: `(B, L, D)`; `kinds`: `(B,)` u32 indici di `type_emb`;
    /// `markers`: `(B·K,)` u32 indici piatti `b·L + pos`. Restituisce `(B, K)` F32.
    pub fn forward(
        &self,
        h: &Tensor,
        kinds: &Tensor,
        markers: &Tensor,
        k: usize,
        key_mask: Option<&Tensor>,
    ) -> Result<Tensor> {
        let (b, l, d) = h.dims3()?;
        let mut x = h.broadcast_add(&self.type_emb.index_select(kinds, 0)?.unsqueeze(1)?)?;
        let selected = match self.layers.split_last() {
            Some((last, rest)) => {
                for layer in rest {
                    x = layer.forward(&x, key_mask)?;
                }
                last.forward_selected(&x, markers, k, key_mask)?
            }
            None => x
                .reshape((b * l, d))?
                .index_select(markers, 0)?
                .reshape((b, k, d))?,
        };
        let logits = selected
            .to_dtype(DType::F32)?
            .apply(&self.scorer_norm)?
            .apply(&self.scorer1)?
            .gelu_erf()?
            .apply(&self.scorer2)?;
        logits.squeeze(2)
    }
}
