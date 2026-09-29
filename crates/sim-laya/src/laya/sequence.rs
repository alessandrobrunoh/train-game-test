//! Costruzione delle righe e dei lotti di Laya, senza dipendenze ML.
//!
//! Porta fedele di `build_sequence`, `render_options`, `collate_items` e
//! `temp_bucket` di `laya/common.py` (Laya, Apache-2.0, commit
//! `9d955671415fc19f069b9cc998928075c1f255ec`). Lavora su id di token già
//! calcolati, così si testa senza tokenizer né pesi.

/// Token per opzione dopo la tokenizzazione (il `[MASK]` iniziale escluso).
pub const OPTION_TOKENS: usize = 48;

/// Tipo di domanda, con l'indice usato da `type_emb` (`QTYPES` in Python).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QuestionKind {
    Choice = 0,
    Score = 1,
    Noul = 2,
}

impl QuestionKind {
    /// Nome usato nel prompt (`"<tipo> question: …"`) e nelle chiavi delle temperature.
    pub fn name(self) -> &'static str {
        match self {
            QuestionKind::Choice => "choice",
            QuestionKind::Score => "score",
            QuestionKind::Noul => "noul",
        }
    }

    pub fn index(self) -> u32 {
        self as u32
    }
}

/// Come rendere le opzioni di una [`crate::ChoiceQuery`] nel testo del modello.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OptionLabels {
    /// Solo la descrizione: come `criteria = [d0, d1, …]` in Python.
    ///
    /// Default: su `laya-multilingual` rende molto meglio delle lettere (es.
    /// ticket in hindi "billing" 0.99 contro 0.80, vedi i test ignorati).
    #[default]
    Plain,
    /// `"A: descrizione"`, `"B: …"`: come `criteria = {"A": …, "B": …}` in Python
    /// (chiavi opache, come suggerisce il README di Laya).
    Letters,
}

/// Etichetta della i-esima opzione in stile foglio di calcolo: A…Z, AA, AB, …
pub fn letter_label(mut i: usize) -> String {
    let mut out = Vec::new();
    loop {
        out.push(b'A' + (i % 26) as u8);
        if i < 26 {
            break;
        }
        i = i / 26 - 1;
    }
    out.reverse();
    String::from_utf8(out).expect("ASCII")
}

/// Testo delle opzioni di una domanda `choice` (`render_options` in Python).
pub fn render_choice_options(options: &[String], labels: OptionLabels) -> Vec<String> {
    options
        .iter()
        .enumerate()
        .map(|(i, d)| match labels {
            OptionLabels::Letters if d.is_empty() => letter_label(i),
            OptionLabels::Letters => format!("{}: {d}", letter_label(i)),
            OptionLabels::Plain => d.clone(),
        })
        .collect()
}

/// Testo della riga di intestazione, prima della tokenizzazione.
pub fn head_text(kind: QuestionKind, instructions: &str) -> String {
    format!("{} question: {instructions}", kind.name())
}

/// Testo di un'opzione prima della tokenizzazione: Python antepone uno spazio.
pub fn option_text(rendered: &str) -> String {
    format!(" {rendered}")
}

/// Id speciali del tokenizer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpecialIds {
    pub cls: u32,
    pub sep: u32,
    pub mask: u32,
    pub pad: u32,
}

/// Una riga pronta per il modello.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub ids: Vec<u32>,
    /// Posizione del `[MASK]` di ciascuna opzione, nell'ordine delle opzioni.
    pub markers: Vec<u32>,
    pub kind: QuestionKind,
}

/// Limiti di lunghezza di una riga.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// Lunghezza massima della riga intera.
    pub max_len: usize,
    /// Budget di intestazione + opzioni (`head_max_len` della checkpoint).
    pub head_max_len: usize,
}

/// Monta `[CLS] head [SEP] [MASK] opt0 [MASK] opt1 … [SEP] state [SEP]`.
///
/// `head_ids` e `option_ids` sono già tokenizzati senza token speciali;
/// `option_ids` già tagliati a [`OPTION_TOKENS`]. Lo stato viene troncato a
/// destra (Python fa lo stesso per gli stati stringa).
///
/// Errore se qualche opzione perde il proprio `[MASK]` (Python solleva
/// "options exceed head_max_len").
pub fn assemble_row(
    special: SpecialIds,
    kind: QuestionKind,
    mut head_ids: Vec<u32>,
    option_ids: &[Vec<u32>],
    state_ids: &[u32],
    budget: Budget,
) -> Result<Row, String> {
    let Budget {
        max_len,
        head_max_len,
    } = budget;
    let mut opts: Vec<Vec<u32>> = option_ids
        .iter()
        .map(|o| {
            let mut v = Vec::with_capacity(o.len().min(OPTION_TOKENS) + 1);
            v.push(special.mask);
            v.extend(o.iter().take(OPTION_TOKENS));
            v
        })
        .collect();
    let used: usize = opts.iter().map(Vec::len).sum();
    let mut opt_budget = head_max_len as isize - used as isize;
    if opt_budget < 16 {
        let per = ((head_max_len as isize - 16) / opts.len().max(1) as isize).max(4) as usize;
        for o in &mut opts {
            o.truncate(per);
        }
        let used: usize = opts.iter().map(Vec::len).sum();
        opt_budget = head_max_len as isize - used as isize;
    }
    head_ids.truncate(opt_budget.max(8) as usize);

    let mut ids = Vec::with_capacity(max_len);
    ids.push(special.cls);
    ids.extend(head_ids);
    ids.push(special.sep);
    let mut markers = Vec::with_capacity(opts.len());
    for o in opts {
        markers.push(ids.len() as u32);
        ids.extend(o);
    }
    ids.push(special.sep);
    let room = max_len.saturating_sub(ids.len() + 1);
    ids.extend(state_ids.iter().take(room));
    ids.push(special.sep);
    ids.truncate(max_len);
    if markers.iter().any(|&m| m as usize >= max_len) {
        return Err(format!(
            "{} opzioni non entrano in max_len={max_len}: accorcia le opzioni o alza max_len",
            markers.len()
        ));
    }
    Ok(Row { ids, markers, kind })
}

/// Un lotto di righe con padding, in formato piatto (row-major).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaddedBatch {
    pub rows: usize,
    /// Lunghezza massima nel lotto.
    pub seq_len: usize,
    /// Numero massimo di opzioni nel lotto.
    pub max_options: usize,
    /// `rows × seq_len`, riempito con `pad`.
    pub ids: Vec<u32>,
    /// Lunghezza reale di ogni riga (la maschera di attenzione è `j < len`).
    pub lengths: Vec<usize>,
    /// `rows × max_options`: posizioni dei marker, 0 (il CLS) per gli slot vuoti.
    pub marker_pos: Vec<u32>,
    /// Numero reale di opzioni per riga (`marker_mask` è `k < count`).
    pub option_counts: Vec<usize>,
    /// Indice di `type_emb` per riga.
    pub kinds: Vec<u32>,
}

/// `collate_items` di Python: padding di id e marker alle dimensioni massime del lotto.
pub fn collate(rows: &[&Row], pad: u32) -> PaddedBatch {
    let n = rows.len();
    let seq_len = rows.iter().map(|r| r.ids.len()).max().unwrap_or(0);
    let max_options = rows.iter().map(|r| r.markers.len()).max().unwrap_or(0);
    let mut ids = vec![pad; n * seq_len];
    let mut marker_pos = vec![0u32; n * max_options];
    for (i, r) in rows.iter().enumerate() {
        ids[i * seq_len..i * seq_len + r.ids.len()].copy_from_slice(&r.ids);
        marker_pos[i * max_options..i * max_options + r.markers.len()].copy_from_slice(&r.markers);
    }
    PaddedBatch {
        rows: n,
        seq_len,
        max_options,
        ids,
        lengths: rows.iter().map(|r| r.ids.len()).collect(),
        marker_pos,
        option_counts: rows.iter().map(|r| r.markers.len()).collect(),
        kinds: rows.iter().map(|r| r.kind.index()).collect(),
    }
}

/// Divide gli indici delle righe in lotti di al più `max_rows`, ordinati per
/// lunghezza decrescente, così righe simili finiscono insieme e il padding cala.
pub fn length_sorted_chunks(lengths: &[usize], max_rows: usize) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..lengths.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(lengths[i]));
    order
        .chunks(max_rows.max(1))
        .map(<[usize]>::to_vec)
        .collect()
}

/// Chiave della temperatura per (tipo, numero di opzioni): `temp_bucket` in Python.
pub fn temperature_bucket(kind: QuestionKind, options: usize) -> String {
    let size = match options {
        0..=2 => "2",
        3..=5 => "3-5",
        6..=10 => "6-10",
        _ => "11+",
    };
    format!("{}:{size}", kind.name())
}

/// Temperatura utilizzabile: limitata a [0.5, 5], 1.0 se non è un numero finito
/// (`clamp_temperature` in Python).
pub fn clamp_temperature(t: f32) -> f32 {
    if t.is_finite() {
        t.clamp(0.5, 5.0)
    } else {
        1.0
    }
}

/// `softmax(logits / t)`, calcolata in f64 per stabilità.
pub fn softmax_with_temperature(logits: &[f32], t: f32) -> Vec<f32> {
    if logits.is_empty() {
        return Vec::new();
    }
    let t = f64::from(t);
    let z: Vec<f64> = logits.iter().map(|&l| f64::from(l) / t).collect();
    let max = z.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let e: Vec<f64> = z.iter().map(|v| (v - max).exp()).collect();
    let sum: f64 = e.iter().sum();
    e.iter().map(|v| (v / sum) as f32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SP: SpecialIds = SpecialIds {
        cls: 1,
        sep: 2,
        mask: 3,
        pad: 0,
    };

    fn budget(max_len: usize, head_max_len: usize) -> Budget {
        Budget {
            max_len,
            head_max_len,
        }
    }

    #[test]
    fn letters_follow_spreadsheet_order() {
        assert_eq!(letter_label(0), "A");
        assert_eq!(letter_label(4), "E");
        assert_eq!(letter_label(25), "Z");
        assert_eq!(letter_label(26), "AA");
        assert_eq!(letter_label(27), "AB");
    }

    #[test]
    fn options_render_like_python_criteria() {
        let opts = vec!["mangia".to_string(), String::new()];
        assert_eq!(
            render_choice_options(&opts, OptionLabels::Letters),
            ["A: mangia", "B"]
        );
        assert_eq!(
            render_choice_options(&opts, OptionLabels::Plain),
            ["mangia", ""]
        );
        assert_eq!(
            head_text(QuestionKind::Choice, "Cosa?"),
            "choice question: Cosa?"
        );
        assert_eq!(option_text("A: x"), " A: x");
    }

    #[test]
    fn row_layout_matches_build_sequence() {
        let row = assemble_row(
            SP,
            QuestionKind::Choice,
            vec![10, 11],
            &[vec![20, 21], vec![30]],
            &[40, 41, 42],
            budget(64, 32),
        )
        .unwrap();
        assert_eq!(row.ids, [1, 10, 11, 2, 3, 20, 21, 3, 30, 2, 40, 41, 42, 2]);
        assert_eq!(row.markers, [4, 7]);
        assert_eq!(row.kind, QuestionKind::Choice);
    }

    #[test]
    fn options_are_capped_at_48_tokens() {
        let long: Vec<u32> = (100..200).collect();
        let row = assemble_row(
            SP,
            QuestionKind::Choice,
            vec![],
            &[long],
            &[],
            budget(512, 192),
        )
        .unwrap();
        // CLS SEP MASK + 48 + SEP SEP
        assert_eq!(row.ids.len(), 3 + OPTION_TOKENS + 2);
        assert_eq!(row.markers, [2]);
    }

    #[test]
    fn state_is_truncated_to_fit_max_len() {
        let state: Vec<u32> = (100..400).collect();
        let row = assemble_row(
            SP,
            QuestionKind::Choice,
            vec![10],
            &[vec![20]],
            &state,
            budget(20, 16),
        )
        .unwrap();
        assert_eq!(row.ids.len(), 20);
        assert_eq!(*row.ids.last().unwrap(), SP.sep);
        // CLS 10 SEP MASK 20 SEP = 6 token, poi 13 di stato e il SEP finale
        assert_eq!(&row.ids[6..19], &state[..13]);
    }

    #[test]
    fn many_options_share_the_head_budget() {
        // 10 opzioni da 20 token con head_max_len 64: budget < 16, per = (64-16)/10 = 4
        let opts: Vec<Vec<u32>> = (0..10).map(|i| vec![100 + i; 20]).collect();
        let head: Vec<u32> = (500..600).collect();
        let row =
            assemble_row(SP, QuestionKind::Choice, head, &opts, &[], budget(512, 64)).unwrap();
        // intestazione: max(8, 64 - 40) = 24 token
        assert_eq!(row.markers[0], 1 + 24 + 1);
        let gaps: Vec<u32> = row.markers.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps.iter().all(|&g| g == 4), "{gaps:?}");
    }

    #[test]
    fn options_that_do_not_fit_are_an_error() {
        let opts: Vec<Vec<u32>> = (0..10).map(|i| vec![100 + i; 10]).collect();
        assert!(
            assemble_row(
                SP,
                QuestionKind::Choice,
                vec![],
                &opts,
                &[],
                budget(30, 192)
            )
            .is_err()
        );
    }

    #[test]
    fn collate_pads_ids_and_markers() {
        let a = Row {
            ids: vec![1, 5, 2, 3, 6, 3, 7, 3, 8, 2, 2],
            markers: vec![3, 5, 7],
            kind: QuestionKind::Choice,
        };
        let b = Row {
            ids: vec![1, 2, 3, 9, 2, 2],
            markers: vec![2],
            kind: QuestionKind::Noul,
        };
        let batch = collate(&[&a, &b], 0);
        assert_eq!((batch.rows, batch.seq_len, batch.max_options), (2, 11, 3));
        assert_eq!(&batch.ids[11..], &[1, 2, 3, 9, 2, 2, 0, 0, 0, 0, 0]);
        assert_eq!(batch.lengths, [11, 6]);
        assert_eq!(batch.marker_pos, [3, 5, 7, 2, 0, 0]);
        assert_eq!(batch.option_counts, [3, 1]);
        assert_eq!(batch.kinds, [0, 2]);
    }

    #[test]
    fn chunks_group_by_length() {
        let chunks = length_sorted_chunks(&[5, 50, 7, 40, 6], 2);
        assert_eq!(chunks, [vec![1, 3], vec![2, 4], vec![0]]);
        assert!(length_sorted_chunks(&[], 4).is_empty());
    }

    #[test]
    fn temperature_buckets_and_clamping() {
        assert_eq!(temperature_bucket(QuestionKind::Choice, 1), "choice:2");
        assert_eq!(temperature_bucket(QuestionKind::Choice, 5), "choice:3-5");
        assert_eq!(temperature_bucket(QuestionKind::Score, 6), "score:6-10");
        assert_eq!(temperature_bucket(QuestionKind::Noul, 11), "noul:11+");
        assert_eq!(clamp_temperature(0.1), 0.5);
        assert_eq!(clamp_temperature(9.0), 5.0);
        assert_eq!(clamp_temperature(f32::NAN), 1.0);
        assert_eq!(clamp_temperature(1.75), 1.75);
    }

    #[test]
    fn softmax_sums_to_one_and_respects_temperature() {
        let p = softmax_with_temperature(&[1.0, 2.0, 3.0], 1.0);
        assert!((p.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        assert!(p[2] > p[1] && p[1] > p[0]);
        let hot = softmax_with_temperature(&[1.0, 2.0, 3.0], 5.0);
        assert!(hot[2] < p[2]);
        assert_eq!(softmax_with_temperature(&[0.7], 1.0), [1.0]);
        assert!(softmax_with_temperature(&[], 1.0).is_empty());
    }
}
